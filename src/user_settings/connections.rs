//! Settings → Connections: read-only list of connected accounts.
//!
//! Protocol: `GET users/@me/connections` → array of `{ type, id, name,
//! verified, visibility (0 hidden / 1 everyone), revoked, show_activity,
//! friend_sync, … }`. Adding / removing connections needs OAuth flows in a
//! browser, so RustCord only lists them.

use eframe::egui::{self, RichText};
use serde_json::Value;

use super::widgets::{self, Icon};
use super::{Action, Ctx, Loadable, Request};
use crate::theme::Bold;

#[derive(Debug, Clone, PartialEq)]
pub struct Connection {
    pub kind: String,
    pub id: String,
    pub name: String,
    pub verified: bool,
    /// Shown on the profile.
    pub visible: bool,
    pub revoked: bool,
}

impl Connection {
    pub fn parse_list(v: &Value) -> Vec<Self> {
        let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let b = |o: &Value, k: &str| o.get(k).and_then(Value::as_bool).unwrap_or(false);
        v.as_array()
            .map(|a| {
                a.iter()
                    .filter(|c| c.is_object())
                    .map(|c| Self {
                        kind: s(c, "type"),
                        id: s(c, "id"),
                        name: s(c, "name"),
                        verified: b(c, "verified"),
                        visible: c.get("visibility").and_then(Value::as_u64) == Some(1),
                        revoked: b(c, "revoked"),
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Human name of the service.
    pub fn service(&self) -> String {
        let known = match self.kind.as_str() {
            "battlenet" => "Battle.net",
            "bluesky" => "Bluesky",
            "bungie" => "Bungie.net",
            "crunchyroll" => "Crunchyroll",
            "domain" => "Domain",
            "ebay" => "eBay",
            "epicgames" => "Epic Games",
            "facebook" => "Facebook",
            "github" => "GitHub",
            "instagram" => "Instagram",
            "leagueoflegends" => "League of Legends",
            "mastodon" => "Mastodon",
            "paypal" => "PayPal",
            "playstation" | "playstation-stg" => "PlayStation Network",
            "reddit" => "Reddit",
            "riotgames" => "Riot Games",
            "roblox" => "Roblox",
            "spotify" => "Spotify",
            "skype" => "Skype",
            "steam" => "Steam",
            "tiktok" => "TikTok",
            "twitch" => "Twitch",
            "twitter" => "X",
            "xbox" => "Xbox",
            "youtube" => "YouTube",
            _ => "",
        };
        if known.is_empty() {
            let mut c = self.kind.chars();
            c.next()
                .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
                .unwrap_or_else(|| "Unknown".into())
        } else {
            known.into()
        }
    }

    /// Public profile link, where there's a stable one.
    pub fn url(&self) -> Option<String> {
        let enc = |s: &str| {
            s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
                .then(|| s.to_string())
        };
        Some(match self.kind.as_str() {
            "github" => format!("https://github.com/{}", enc(&self.name)?),
            "twitch" => format!("https://www.twitch.tv/{}", enc(&self.name)?),
            "twitter" => format!("https://x.com/{}", enc(&self.name)?),
            "reddit" => format!("https://www.reddit.com/user/{}", enc(&self.name)?),
            "youtube" => format!("https://www.youtube.com/channel/{}", enc(&self.id)?),
            "steam" => format!("https://steamcommunity.com/profiles/{}", enc(&self.id)?),
            "spotify" => format!("https://open.spotify.com/user/{}", enc(&self.id)?),
            "tiktok" => format!("https://www.tiktok.com/@{}", enc(&self.name)?),
            "bluesky" => format!("https://bsky.app/profile/{}", enc(&self.name)?),
            "domain" => format!("https://{}", enc(&self.name)?),
            _ => return None,
        })
    }
}

#[derive(Default)]
pub struct ConnectionsPage {
    pub list: Loadable<Vec<Connection>>,
}

impl ConnectionsPage {
    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        if c.is_bot {
            widgets::hint(ui, pal, "Bot accounts don't have connections.");
            return None;
        }
        widgets::hint(
            ui,
            pal,
            "Accounts connected to your Discord profile. To add or remove one, use \
             Discord's official app or website (it needs a browser sign-in).",
        );
        ui.add_space(10.0);
        if self.list.start() {
            return Some(Action::Request(Request::LoadConnections));
        }
        let mut action = None;
        match &self.list {
            Loadable::Failed(e) => {
                widgets::error(ui, pal, &format!("Couldn't load your connections: {e}"));
                if ui.button("Try again").clicked() {
                    self.list = Loadable::Loading;
                    return Some(Action::Request(Request::LoadConnections));
                }
            }
            Loadable::Loaded(list) if list.is_empty() => {
                widgets::hint(ui, pal, "You haven't connected any accounts.");
            }
            Loadable::Loaded(list) => {
                for conn in list {
                    widgets::card(ui, pal, |ui| {
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.label(pal.section_label(&conn.service()));
                                ui.horizontal(|ui| {
                                    ui.label(
                                        RichText::new(&conn.name)
                                            .bold()
                                            .size(15.0)
                                            .color(pal.strong),
                                    );
                                    if conn.verified {
                                        widgets::icon(ui, Icon::Check, 14.0, pal.online)
                                            .on_hover_text("Verified");
                                    }
                                });
                                let mut notes = vec![if conn.visible {
                                    "Shown on profile"
                                } else {
                                    "Hidden from profile"
                                }];
                                if conn.revoked {
                                    notes.push("Revoked: reconnect it in Discord");
                                }
                                ui.label(
                                    RichText::new(notes.join(" · "))
                                        .size(12.0)
                                        .color(if conn.revoked { pal.danger } else { pal.muted }),
                                );
                            });
                            if let Some(url) = conn.url() {
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::icon_button(ui, Icon::Link, pal, &url).clicked()
                                        {
                                            action = Some(Action::OpenLink(url.clone()));
                                        }
                                    },
                                );
                            }
                        });
                    });
                    ui.add_space(6.0);
                }
            }
            _ => {
                ui.spinner();
            }
        }
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_and_links() {
        let v = json!([
            { "type": "github", "id": "1", "name": "KeoLogs", "verified": true, "visibility": 1 },
            { "type": "steam", "id": "76561198000000000", "name": "Keo", "revoked": true, "visibility": 0 },
            { "type": "newthing", "id": "x", "name": "a b" },
            "junk"
        ]);
        let l = Connection::parse_list(&v);
        assert_eq!(l.len(), 3);
        assert!(l[0].verified && l[0].visible);
        assert_eq!(l[0].service(), "GitHub");
        assert_eq!(l[0].url().as_deref(), Some("https://github.com/KeoLogs"));
        assert!(l[1].revoked && !l[1].visible);
        assert_eq!(
            l[1].url().as_deref(),
            Some("https://steamcommunity.com/profiles/76561198000000000")
        );
        assert_eq!(l[2].service(), "Newthing");
        assert_eq!(l[2].url(), None);
        // Names that would need escaping get no link.
        let odd = Connection {
            kind: "github".into(),
            name: "a/../b".into(),
            ..l[0].clone()
        };
        assert_eq!(odd.url(), None);
        assert!(Connection::parse_list(&json!({})).is_empty());
    }
}
