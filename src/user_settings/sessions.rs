//! Settings → Devices: where you're logged in, and logging those sessions
//! out.
//!
//! Protocol (docs.discord.food; not official):
//! - `GET auth/sessions` → `{ user_sessions: [{ id_hash,
//!   approx_last_used_time, client_info: { os, platform, location } }] }`
//!   (up to 50, any field may be null).
//! - `POST auth/sessions/logout` with `{ session_id_hashes: [..] }` (1–64)
//!   → 204.
//! - The current session's hash is READY `auth_session_id_hash`; it is
//!   marked "This device" and can't be logged out here (use Log Out).
//!
//! UNCERTAIN: Discord may ask for MFA (error code 60003) before logging out
//! other sessions on some accounts; the error text is shown as is.

use eframe::egui::{self, RichText};
use serde_json::Value;

use super::widgets::{self, Confirm, Icon};
use super::{Action, Ctx, Loadable, Outcome, Request};
use crate::theme::Bold;

#[derive(Debug, Clone, PartialEq)]
pub struct AuthSession {
    pub id_hash: String,
    /// Unix seconds (0 if unknown).
    pub last_used: i64,
    pub os: String,
    pub platform: String,
    pub location: String,
}

impl AuthSession {
    fn from_json(v: &Value) -> Option<Self> {
        let s = |o: &Value, k: &str| o.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let id_hash = s(v, "id_hash");
        if id_hash.is_empty() {
            return None;
        }
        let info = &v["client_info"];
        let used = s(v, "approx_last_used_time");
        Some(Self {
            id_hash,
            last_used: if used.is_empty() {
                0
            } else {
                crate::timefmt::parse_iso(&used)
            },
            os: s(info, "os"),
            platform: s(info, "platform"),
            location: s(info, "location"),
        })
    }

    /// `GET auth/sessions` body, most recently used first.
    pub fn parse_list(v: &Value) -> Vec<Self> {
        let mut list: Vec<Self> = v
            .get("user_sessions")
            .and_then(Value::as_array)
            .or(v.as_array())
            .map(|a| a.iter().filter_map(Self::from_json).collect())
            .unwrap_or_default();
        list.sort_by_key(|s| std::cmp::Reverse(s.last_used));
        list
    }

    /// "Windows · Discord Client" style title.
    pub fn title(&self) -> String {
        match (self.os.is_empty(), self.platform.is_empty()) {
            (false, false) => format!("{} · {}", self.os, self.platform),
            (false, true) => self.os.clone(),
            (true, false) => self.platform.clone(),
            (true, true) => "Unknown device".into(),
        }
    }

    pub fn icon(&self) -> Icon {
        let os = self.os.to_lowercase();
        let platform = self.platform.to_lowercase();
        if ["android", "ios", "ipad", "iphone"]
            .iter()
            .any(|m| os.contains(m))
        {
            Icon::Phone
        } else if [
            "chrome", "firefox", "safari", "edge", "opera", "browser", "web",
        ]
        .iter()
        .any(|b| platform.contains(b))
        {
            Icon::Browser
        } else {
            Icon::Desktop
        }
    }
}

/// "just now", "5 minutes ago", "3 days ago".
pub fn ago(then: i64, now: i64) -> String {
    if then <= 0 {
        return "a while ago".into();
    }
    let d = (now - then).max(0);
    let (n, unit) = match d {
        0..60 => return "just now".into(),
        60..3600 => (d / 60, "minute"),
        3600..86_400 => (d / 3600, "hour"),
        86_400..2_592_000 => (d / 86_400, "day"),
        2_592_000..31_536_000 => (d / 2_592_000, "month"),
        _ => (d / 31_536_000, "year"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

#[derive(Default)]
pub struct SessionsPage {
    pub list: Loadable<Vec<AuthSession>>,
    /// Hashes waiting for the confirm dialog.
    confirm: Option<Vec<String>>,
    busy: bool,
    error: Option<String>,
}

impl SessionsPage {
    pub fn apply(&mut self, outcome: Outcome) {
        match outcome {
            Outcome::Sessions(r) => self.list.set(r),
            Outcome::SessionsLoggedOut(r) => {
                self.busy = false;
                match r {
                    Ok(gone) => {
                        if let Some(l) = self.list.get_mut() {
                            l.retain(|s| !gone.contains(&s.id_hash));
                        }
                    }
                    Err(e) => self.error = Some(e),
                }
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        let pal = c.pal;
        if c.is_bot {
            widgets::hint(ui, pal, "Bot accounts don't have device sessions.");
            return None;
        }
        widgets::hint(
            ui,
            pal,
            "Here are all the devices that are currently logged in with your Discord \
             account. You can log out of each one individually or all other devices at once.",
        );
        widgets::hint(
            ui,
            pal,
            "If you see an entry you don't recognise, log out of that device and change \
             your Discord password immediately.",
        );
        ui.add_space(10.0);
        if self.list.start() {
            return Some(Action::Request(Request::LoadSessions));
        }
        let mut action = None;
        match &self.list {
            Loadable::Failed(e) => {
                widgets::error(ui, pal, &format!("Couldn't load your devices: {e}"));
                if ui.button("Try again").clicked() {
                    self.list = Loadable::Loading;
                    return Some(Action::Request(Request::LoadSessions));
                }
            }
            Loadable::Loaded(list) => {
                let now = chrono::Utc::now().timestamp();
                let current = c.current_session_hash;
                let (mine, others): (Vec<_>, Vec<_>) = list
                    .iter()
                    .partition(|s| Some(s.id_hash.as_str()) == current);
                if !mine.is_empty() {
                    widgets::label(ui, pal, "CURRENT DEVICE");
                    for s in &mine {
                        session_row(ui, c, s, now, true);
                    }
                    ui.add_space(10.0);
                }
                widgets::label(ui, pal, "OTHER DEVICES");
                if others.is_empty() {
                    widgets::hint(ui, pal, "No other devices are logged in.");
                }
                for s in &others {
                    if session_row(ui, c, s, now, false) && !self.busy {
                        self.confirm = Some(vec![s.id_hash.clone()]);
                    }
                }
                if others.len() > 1 {
                    ui.add_space(14.0);
                    widgets::heading(ui, pal, "Log out of all known devices");
                    widgets::hint(
                        ui,
                        pal,
                        "You'll have to log back in on all logged-out devices.",
                    );
                    let b = ui.add_enabled(
                        !self.busy,
                        widgets::danger_button(pal, "Log out all known devices"),
                    );
                    if b.clicked() {
                        self.confirm =
                            Some(others.iter().take(64).map(|s| s.id_hash.clone()).collect());
                    }
                }
                if self.busy {
                    ui.spinner();
                }
            }
            _ => {
                ui.spinner();
            }
        }
        if let Some(e) = &self.error {
            widgets::error(ui, pal, e);
        }
        if let Some(hashes) = &self.confirm {
            let (title, body) = if hashes.len() == 1 {
                (
                    "Log out device?",
                    "That device will be logged out of Discord and will need to log in again.",
                )
            } else {
                (
                    "Log out all known devices?",
                    "Every device except this one will be logged out of Discord.",
                )
            };
            match widgets::confirm_modal(ui.ctx(), "sessions", pal, title, body, "Log Out", true) {
                Some(Confirm::Yes) => {
                    self.busy = true;
                    self.error = None;
                    action = Some(Action::Request(Request::LogoutSessions(
                        self.confirm.take().unwrap_or_default(),
                    )));
                }
                Some(Confirm::No) => self.confirm = None,
                None => {}
            }
        }
        action
    }
}

/// One device. Returns true when "Log out" was clicked.
fn session_row(ui: &mut egui::Ui, c: &Ctx, s: &AuthSession, now: i64, current: bool) -> bool {
    let pal = c.pal;
    let mut clicked = false;
    widgets::card(ui, pal, |ui| {
        ui.horizontal(|ui| {
            egui::Frame::new()
                .fill(pal.input)
                .corner_radius(20)
                .inner_margin(8)
                .show(ui, |ui| widgets::icon(ui, s.icon(), 22.0, pal.strong));
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.label(
                    RichText::new(s.title().to_uppercase())
                        .bold()
                        .size(13.0)
                        .color(pal.strong),
                );
                let mut line = Vec::new();
                if !s.location.is_empty() {
                    line.push(s.location.clone());
                }
                if !current {
                    line.push(ago(s.last_used, now));
                }
                ui.label(RichText::new(line.join(" · ")).size(13.0).color(pal.muted));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if current {
                    ui.label(RichText::new("This device").size(13.0).color(pal.online));
                } else if widgets::icon_button(ui, Icon::Close, pal, "Log out this device")
                    .clicked()
                {
                    clicked = true;
                }
            });
        });
    });
    ui.add_space(6.0);
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_sessions() {
        let v = json!({ "user_sessions": [
            { "id_hash": "a", "approx_last_used_time": "2026-11-01T10:00:00+00:00",
              "client_info": { "os": "Windows", "platform": "Discord Client", "location": "London, United Kingdom" } },
            { "id_hash": "b", "approx_last_used_time": "2026-11-09T10:00:00+00:00",
              "client_info": { "os": "iOS", "platform": null, "location": null } },
            { "approx_last_used_time": "2026-11-09T10:00:00+00:00" },
            { "id_hash": "c", "client_info": { "os": null, "platform": "Chrome" } }
        ]});
        let l = AuthSession::parse_list(&v);
        assert_eq!(
            l.iter().map(|s| s.id_hash.as_str()).collect::<Vec<_>>(),
            ["b", "a", "c"]
        );
        assert_eq!(l[1].title(), "Windows · Discord Client");
        assert_eq!(l[0].title(), "iOS");
        assert_eq!(l[0].icon(), Icon::Phone);
        assert_eq!(l[1].icon(), Icon::Desktop);
        assert_eq!(l[2].icon(), Icon::Browser);
        assert_eq!(l[2].last_used, 0);
        assert!(AuthSession::parse_list(&json!({ "message": "401" })).is_empty());
    }

    #[test]
    fn ago_text() {
        let now = 1_000_000_000;
        assert_eq!(ago(now - 5, now), "just now");
        assert_eq!(ago(now - 60, now), "1 minute ago");
        assert_eq!(ago(now - 7200, now), "2 hours ago");
        assert_eq!(ago(now - 3 * 86_400, now), "3 days ago");
        assert_eq!(ago(now - 400 * 86_400, now), "1 year ago");
        assert_eq!(ago(0, now), "a while ago");
    }
}
