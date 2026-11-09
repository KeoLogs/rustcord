//! Online status and activities ("presence").
//!
//! Protocol notes (from Discord Messenger, `DiscordInstance.cpp`):
//! - `READY_SUPPLEMENTAL` (user accounts): `merged_presences.guilds[i]` is a
//!   list of `{ user_id, status, activities }` for `guilds[i]`, and
//!   `merged_presences.friends` the same for friends (DI:2357).
//! - `PRESENCE_UPDATE`: `{ user: { id, ... }, status, activities, guild_id? }`
//!   (DI:3006). Bots only get these with the GUILD_PRESENCES intent.
//! - Member list items carry a `presence` object of the same shape.
//! - Activities: a custom status (type 4) wins over games/music; otherwise
//!   the first activity is shown ("Playing X", "Listening to X", …).
//!
//! Status is global (not per server) in practice, so one map is enough.

use std::collections::HashMap;

use eframe::egui::Color32;
use serde_json::Value;

use crate::theme::Palette;

/// Most presences we keep. Past this, offline entries are dropped first.
const MAX_PRESENCES: usize = 5000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Status {
    Online,
    Idle,
    Dnd,
    #[default]
    Offline,
}

impl Status {
    /// "online" / "idle" / "dnd"; anything else (offline, invisible) is offline.
    pub fn parse(s: &str) -> Self {
        match s {
            "online" => Status::Online,
            "idle" => Status::Idle,
            "dnd" => Status::Dnd,
            _ => Status::Offline,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Status::Online => "Online",
            Status::Idle => "Idle",
            Status::Dnd => "Do Not Disturb",
            Status::Offline => "Offline",
        }
    }

    pub fn color(self, pal: &Palette) -> Color32 {
        match self {
            Status::Online => pal.online,
            Status::Idle => pal.idle,
            Status::Dnd => pal.dnd,
            Status::Offline => pal.offline,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Presence {
    pub status: Status,
    /// Custom status or "Playing …" line. None if there's nothing to show.
    pub activity: Option<Box<str>>,
}

impl Presence {
    /// A presence object (PRESENCE_UPDATE `d`, member `presence`, or an
    /// entry of READY_SUPPLEMENTAL `merged_presences`).
    pub fn from_json(v: &Value) -> Self {
        let status = Status::parse(v.get("status").and_then(Value::as_str).unwrap_or(""));
        // Old payloads use `game` instead of `activities`.
        let activity = match v.get("activities").and_then(Value::as_array) {
            Some(list) => activity_text(list),
            None => v
                .get("game")
                .filter(|g| g.is_object())
                .and_then(activity_line),
        };
        Self {
            status,
            activity: activity.map(String::into_boxed_str),
        }
    }
}

/// Text for a list of activities: custom status first, else the first one.
pub fn activity_text(list: &[Value]) -> Option<String> {
    list.iter()
        .find(|a| a.get("type").and_then(Value::as_u64) == Some(4))
        .and_then(activity_line)
        .or_else(|| list.iter().find_map(activity_line))
}

fn activity_line(a: &Value) -> Option<String> {
    let s = |k: &str| a.get(k).and_then(Value::as_str).unwrap_or("").trim();
    let name = s("name");
    let text = match a.get("type").and_then(Value::as_u64)? {
        0 => format!("Playing {name}"),
        1 => format!(
            "Streaming {}",
            if s("details").is_empty() {
                name
            } else {
                s("details")
            }
        ),
        2 => format!("Listening to {name}"),
        3 => format!("Watching {name}"),
        4 => {
            // Custom status: optional unicode emoji + text. Custom (server)
            // emoji have an id and are skipped here.
            let emoji = a
                .get("emoji")
                .filter(|e| e.get("id").is_none_or(Value::is_null))
                .and_then(|e| e.get("name"))
                .and_then(Value::as_str)
                .unwrap_or("");
            let state = s("state");
            match (emoji.is_empty(), state.is_empty()) {
                (true, true) => return None,
                (false, false) => format!("{emoji} {state}"),
                (true, false) => state.to_string(),
                (false, true) => emoji.to_string(),
            }
        }
        5 => format!("Competing in {name}"),
        _ => return None,
    };
    (!text.trim().is_empty()).then_some(text)
}

/// Everyone's last known presence, by user id.
#[derive(Default)]
pub struct Presences {
    map: HashMap<String, Presence>,
}

impl Presences {
    pub fn get(&self, user_id: &str) -> Option<&Presence> {
        self.map.get(user_id)
    }

    pub fn status(&self, user_id: &str) -> Option<Status> {
        self.map.get(user_id).map(|p| p.status)
    }

    pub fn set(&mut self, user_id: &str, p: Presence) {
        if let Some(old) = self.map.get_mut(user_id) {
            *old = p;
            return;
        }
        if self.map.len() >= MAX_PRESENCES {
            self.map.retain(|_, p| p.status != Status::Offline);
            if self.map.len() >= MAX_PRESENCES {
                self.map.clear();
            }
        }
        self.map.insert(user_id.to_string(), p);
    }

    pub fn clear(&mut self) {
        self.map = HashMap::new();
    }

    pub fn len(&self) -> usize {
        self.map.len()
    }
}

/// READY_SUPPLEMENTAL `merged_presences`: friends plus every server's list.
pub fn parse_supplemental(d: &Value) -> Vec<(String, Presence)> {
    let merged = &d["merged_presences"];
    let guilds = merged["guilds"].as_array().into_iter().flatten();
    merged["friends"]
        .as_array()
        .into_iter()
        .flatten()
        .chain(guilds.flat_map(|g| g.as_array().into_iter().flatten()))
        .filter_map(|p| {
            let id = p["user_id"].as_str().or(p["user"]["id"].as_str())?;
            Some((id.to_string(), Presence::from_json(p)))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn custom_status_wins() {
        let p = Presence::from_json(&json!({
            "status": "dnd",
            "activities": [
                { "type": 0, "name": "Minecraft" },
                { "type": 4, "name": "Custom Status", "state": "busy coding", "emoji": { "name": "🦀" } },
            ]
        }));
        assert_eq!(p.status, Status::Dnd);
        assert_eq!(p.activity.as_deref(), Some("🦀 busy coding"));
    }

    #[test]
    fn activity_kinds() {
        let one = |a: Value| activity_text(&[a]);
        assert_eq!(
            one(json!({ "type": 0, "name": "Minecraft" })).as_deref(),
            Some("Playing Minecraft")
        );
        assert_eq!(
            one(json!({ "type": 2, "name": "Spotify" })).as_deref(),
            Some("Listening to Spotify")
        );
        // Custom emoji only (has an id) and no text: nothing to show.
        assert_eq!(
            one(json!({ "type": 4, "emoji": { "name": "pepe", "id": "1" } })),
            None
        );
        assert_eq!(one(json!({ "type": 99, "name": "?" })), None);
    }

    #[test]
    fn status_parse() {
        assert_eq!(Status::parse("online"), Status::Online);
        assert_eq!(Status::parse("idle"), Status::Idle);
        assert_eq!(Status::parse("invisible"), Status::Offline);
        assert_eq!(Presence::from_json(&json!({})).status, Status::Offline);
    }

    #[test]
    fn supplemental() {
        let d = json!({ "merged_presences": {
            "friends": [{ "user_id": "1", "status": "online", "activities": [] }],
            "guilds": [[{ "user_id": "2", "status": "idle" }], []],
        }});
        let list = parse_supplemental(&d);
        assert_eq!(list.len(), 2);
        assert_eq!(list[1].0, "2");
        assert_eq!(list[1].1.status, Status::Idle);
    }

    #[test]
    fn cap_drops_offline_first() {
        let mut p = Presences::default();
        for i in 0..MAX_PRESENCES {
            let status = if i == 0 {
                Status::Online
            } else {
                Status::Offline
            };
            p.set(
                &i.to_string(),
                Presence {
                    status,
                    activity: None,
                },
            );
        }
        p.set("new", Presence::default());
        assert_eq!(p.len(), 2);
        assert_eq!(p.status("0"), Some(Status::Online));
    }
}
