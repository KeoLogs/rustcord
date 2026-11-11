//! Mute and notification settings for one server or channel (right-click →
//! "Notification Settings"; Notifications module, user accounts).
//!
//! The dialog shows what Discord has (`notify::Mutes`) and every button is
//! one PATCH of the user's own settings
//! (`users/@me/guilds/<id>/settings`), sent only on that click. Discord
//! answers with the new settings, which update `Mutes`; nothing is changed
//! locally before that and nothing is ever sent on a timer.

use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use crate::icons;
use crate::notify::{
    GuildSettings, NOTIFY_ALL, NOTIFY_INHERIT, NOTIFY_MENTIONS, NOTIFY_NOTHING, iso_from_unix,
};
use crate::theme::{Bold as _, Palette};

/// What the dialog is about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Target {
    /// A whole server.
    Guild { guild_id: String, name: String },
    /// One channel (or DM; then `guild_id` is `types::DM_GUILD_ID`).
    Channel {
        guild_id: String,
        channel_id: String,
        name: String,
    },
}

impl Target {
    pub fn guild_id(&self) -> &str {
        match self {
            Target::Guild { guild_id, .. } | Target::Channel { guild_id, .. } => guild_id,
        }
    }

    fn name(&self) -> &str {
        match self {
            Target::Guild { name, .. } | Target::Channel { name, .. } => name,
        }
    }

    /// Wrap a server-level body for this target: as is for a server, as a
    /// channel override for a channel.
    fn body(&self, fields: Value) -> Value {
        match self {
            Target::Guild { .. } => fields,
            Target::Channel { channel_id, .. } => {
                json!({ "channel_overrides": { channel_id.as_str(): fields } })
            }
        }
    }
}

/// How long to mute for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MuteFor {
    Minutes(u32),
    /// "Until I turn it back on".
    Forever,
}

/// The choices, as Discord offers them.
pub const DURATIONS: &[(&str, MuteFor)] = &[
    ("For 15 minutes", MuteFor::Minutes(15)),
    ("For 1 hour", MuteFor::Minutes(60)),
    ("For 3 hours", MuteFor::Minutes(180)),
    ("For 8 hours", MuteFor::Minutes(480)),
    ("For 24 hours", MuteFor::Minutes(1440)),
    ("Until I turn it back on", MuteFor::Forever),
];

/// The PATCH body that mutes `target` from `now` (unix seconds).
pub fn mute_body(target: &Target, how_long: MuteFor, now: u64) -> Value {
    let config = match how_long {
        MuteFor::Minutes(m) => {
            let secs = u64::from(m) * 60;
            json!({
                "end_time": format!("{}.000Z", iso_from_unix(now + secs)),
                "selected_time_window": secs,
            })
        }
        MuteFor::Forever => json!({ "end_time": null, "selected_time_window": -1 }),
    };
    target.body(json!({ "muted": true, "mute_config": config }))
}

pub fn unmute_body(target: &Target) -> Value {
    target.body(json!({ "muted": false, "mute_config": null }))
}

/// The PATCH body that sets the notification level (`notify::NOTIFY_*`).
/// "Use the server's setting" (`NOTIFY_INHERIT`) only exists for channels.
pub fn level_body(target: &Target, level: u8) -> Option<Value> {
    let ok = match target {
        Target::Guild { .. } => level <= NOTIFY_NOTHING,
        Target::Channel { .. } => level <= NOTIFY_INHERIT,
    };
    ok.then(|| target.body(json!({ "message_notifications": level })))
}

/// The answer to the PATCH as settings. Discord leaves `guild_id` null for
/// the DM settings; the id we patched fills it in.
pub fn parse_response(v: &Value, guild_id: &str) -> GuildSettings {
    let mut s = GuildSettings::from_json(v);
    if s.guild_id.is_empty() || v.get("guild_id").is_none_or(Value::is_null) {
        s.guild_id = guild_id.to_owned();
    }
    s
}

/// Demo mode's stand-in for Discord: merge a PATCH body into the stored
/// settings object (channel overrides are a list there).
pub fn apply_patch(stored: &mut Value, body: &Value) {
    if !stored.is_object() {
        *stored = json!({});
    }
    let Some(fields) = body.as_object() else {
        return;
    };
    for (key, value) in fields {
        if key != "channel_overrides" {
            stored[key.as_str()] = value.clone();
            continue;
        }
        let mut list = stored["channel_overrides"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        for (channel_id, patch) in value.as_object().into_iter().flatten() {
            let at = list
                .iter()
                .position(|o| o["channel_id"].as_str() == Some(channel_id));
            let at = at.unwrap_or_else(|| {
                list.push(json!({ "channel_id": channel_id }));
                list.len() - 1
            });
            for (k, v) in patch.as_object().into_iter().flatten() {
                list[at][k.as_str()] = v.clone();
            }
        }
        stored["channel_overrides"] = Value::Array(list);
    }
}

/// What Discord currently has for the target: (muted now, level).
pub fn current(target: &Target, settings: Option<&GuildSettings>, now_iso: &str) -> (bool, u8) {
    match target {
        // A server level of "inherit" means Discord didn't say: no radio
        // is selected then.
        Target::Guild { .. } => settings.map_or((false, NOTIFY_INHERIT), |g| {
            (g.mute.active(now_iso), g.level.min(NOTIFY_INHERIT))
        }),
        Target::Channel { channel_id, .. } => settings
            .and_then(|g| g.overrides.get(channel_id))
            .map_or((false, NOTIFY_INHERIT), |o| {
                (o.mute.active(now_iso), o.level.min(NOTIFY_INHERIT))
            }),
    }
}

/// What the dialog wants done after a frame.
#[derive(Debug, PartialEq)]
pub enum Event {
    Close,
    /// A button was clicked: PATCH this body to the target's server.
    Patch(Value),
}

/// Draw the dialog. `Patch` only ever comes from a click this frame.
pub fn show(
    ctx: &egui::Context,
    pal: &Palette,
    target: &Target,
    settings: Option<&GuildSettings>,
    now_unix: u64,
) -> Option<Event> {
    let (muted, level) = current(target, settings, &iso_from_unix(now_unix));
    let mut event = None;
    let modal = egui::Modal::new(egui::Id::unique("rc_notif_settings")).show(ctx, |ui| {
        ui.set_width(340.0);
        ui.label(
            RichText::new("Notification Settings")
                .bold()
                .size(16.0)
                .color(pal.strong),
        );
        ui.label(RichText::new(target.name()).color(pal.muted));
        ui.add_space(8.0);

        ui.label(pal.section_label("Mute"));
        if muted {
            ui.label(RichText::new(icons::label(icons::BELL_SLASH, "Muted")).color(pal.muted));
            if ui.button(icons::label(icons::BELL, "Unmute")).clicked() {
                event = Some(Event::Patch(unmute_body(target)));
            }
        } else {
            ui.horizontal_wrapped(|ui| {
                for (label, how_long) in DURATIONS {
                    if ui.button(*label).clicked() {
                        event = Some(Event::Patch(mute_body(target, *how_long, now_unix)));
                    }
                }
            });
        }
        ui.add_space(10.0);

        ui.label(pal.section_label("Notify me about"));
        let mut levels = vec![
            (NOTIFY_ALL, "All messages"),
            (NOTIFY_MENTIONS, "Only @mentions"),
            (NOTIFY_NOTHING, "Nothing"),
        ];
        if matches!(target, Target::Channel { .. }) {
            levels.insert(0, (NOTIFY_INHERIT, "Use the server's setting"));
        }
        for (value, label) in levels {
            if ui.radio(level == value, label).clicked()
                && level != value
                && let Some(body) = level_body(target, value)
            {
                event = Some(Event::Patch(body));
            }
        }
        ui.add_space(10.0);
        ui.label(
            RichText::new("Each click changes your own Discord settings right away.")
                .small()
                .color(pal.muted),
        );
        ui.add_space(6.0);
        if ui.button("Done").clicked() {
            event = Some(Event::Close);
        }
    });
    if event.is_none() && modal.should_close() {
        event = Some(Event::Close);
    }
    event
}

#[cfg(test)]
mod tests {
    use super::*;

    fn guild() -> Target {
        Target::Guild {
            guild_id: "100".into(),
            name: "Server".into(),
        }
    }

    fn channel() -> Target {
        Target::Channel {
            guild_id: "100".into(),
            channel_id: "200".into(),
            name: "#general".into(),
        }
    }

    #[test]
    fn timed_server_mute() {
        // 2023-11-14T22:13:20Z
        let b = mute_body(&guild(), MuteFor::Minutes(60), 1_700_000_000);
        assert_eq!(
            b,
            json!({
                "muted": true,
                "mute_config": {
                    "end_time": "2023-11-14T23:13:20.000Z",
                    "selected_time_window": 3600,
                },
            })
        );
    }

    #[test]
    fn forever_and_unmute() {
        assert_eq!(
            mute_body(&guild(), MuteFor::Forever, 5),
            json!({
                "muted": true,
                "mute_config": { "end_time": null, "selected_time_window": -1 },
            })
        );
        assert_eq!(
            unmute_body(&guild()),
            json!({ "muted": false, "mute_config": null })
        );
    }

    #[test]
    fn channels_go_in_channel_overrides() {
        let b = mute_body(&channel(), MuteFor::Forever, 5);
        assert_eq!(b["channel_overrides"]["200"]["muted"], json!(true));
        assert!(b.get("muted").is_none());
        assert_eq!(
            level_body(&channel(), NOTIFY_MENTIONS),
            Some(json!({ "channel_overrides": { "200": { "message_notifications": 1 } } }))
        );
        assert_eq!(
            unmute_body(&channel()),
            json!({ "channel_overrides": { "200": { "muted": false, "mute_config": null } } })
        );
    }

    #[test]
    fn levels() {
        assert_eq!(
            level_body(&guild(), NOTIFY_NOTHING),
            Some(json!({ "message_notifications": 2 }))
        );
        // A server has no "inherit"; a channel does. Nothing above 3 exists.
        assert_eq!(level_body(&guild(), NOTIFY_INHERIT), None);
        assert!(level_body(&channel(), NOTIFY_INHERIT).is_some());
        assert_eq!(level_body(&channel(), 4), None);
    }

    #[test]
    fn a_mute_body_round_trips_through_the_parser() {
        // What we send is what `notify` reads back as "muted until then".
        let now = 1_700_000_000;
        let mut answer = mute_body(&guild(), MuteFor::Minutes(15), now);
        answer["guild_id"] = json!("100");
        answer["message_notifications"] = json!(1);
        let s = parse_response(&answer, "100");
        assert_eq!(s.guild_id, "100");
        assert_eq!(current(&guild(), Some(&s), &iso_from_unix(now)), (true, 1));
        // 16 minutes later the mute is over.
        let later = iso_from_unix(now + 16 * 60);
        assert_eq!(current(&guild(), Some(&s), &later), (false, 1));
    }

    #[test]
    fn dm_settings_get_their_id_back() {
        let s = parse_response(&json!({ "guild_id": null, "muted": true }), "@me");
        assert_eq!(s.guild_id, "@me");
        let s = parse_response(&json!({ "muted": true }), "@me");
        assert_eq!(s.guild_id, "@me");
    }

    #[test]
    fn demo_patches_merge_like_discord() {
        let mut stored = json!({ "guild_id": "100", "message_notifications": 0 });
        apply_patch(&mut stored, &mute_body(&channel(), MuteFor::Forever, 5));
        apply_patch(
            &mut stored,
            &level_body(&channel(), NOTIFY_MENTIONS).unwrap(),
        );
        apply_patch(&mut stored, &level_body(&guild(), NOTIFY_NOTHING).unwrap());
        let s = parse_response(&stored, "100");
        let now = iso_from_unix(100);
        assert_eq!(current(&guild(), Some(&s), &now), (false, NOTIFY_NOTHING));
        assert_eq!(current(&channel(), Some(&s), &now), (true, NOTIFY_MENTIONS));
        assert_eq!(stored["channel_overrides"].as_array().unwrap().len(), 1);
        apply_patch(&mut stored, &unmute_body(&channel()));
        let s = parse_response(&stored, "100");
        assert_eq!(
            current(&channel(), Some(&s), &now),
            (false, NOTIFY_MENTIONS)
        );
    }

    #[test]
    fn current_state_defaults() {
        let now = iso_from_unix(100);
        assert_eq!(current(&guild(), None, &now), (false, NOTIFY_INHERIT));
        assert_eq!(current(&channel(), None, &now), (false, NOTIFY_INHERIT));
        let s = parse_response(
            &json!({
                "guild_id": "100", "message_notifications": 2,
                "channel_overrides": [
                    { "channel_id": "200", "muted": true, "message_notifications": 0 }
                ],
            }),
            "100",
        );
        assert_eq!(current(&guild(), Some(&s), &now), (false, NOTIFY_NOTHING));
        assert_eq!(current(&channel(), Some(&s), &now), (true, NOTIFY_ALL));
    }
}
