//! Invites: parsing, the "Invite friends to <server>" dialog and the invite
//! list (Server Settings → Invites, Channel Settings → Invites).
//!
//! Creating an invite only happens when the user presses "Create Link" /
//! "Generate a New Link"; revoking only after a confirm. Invite objects are
//! parsed leniently: `expires_at` is newer and may be missing, in which case
//! the expiry is computed from `created_at + max_age`.

use eframe::egui::{self, RichText, vec2};
use serde_json::{Value, json};

use super::permissions as p;
use super::rest::ServerRequest;
use super::widgets::{self, Confirm, Icon};
use crate::theme::{Bold, Palette};
use crate::timefmt;
use crate::types::Guild;

/// One invite.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub code: String,
    pub channel_id: String,
    pub channel_name: String,
    pub inviter_id: String,
    pub inviter_name: String,
    pub uses: u32,
    /// 0 = unlimited.
    pub max_uses: u32,
    /// Seconds; 0 = never expires.
    pub max_age: u32,
    pub temporary: bool,
    /// Unix seconds (0 if unknown).
    pub created_at: i64,
    /// Unix seconds; `None` = never expires.
    pub expires_at: Option<i64>,
}

impl Invite {
    /// `None` without a code.
    pub fn from_json(v: &Value) -> Option<Self> {
        let code = v.get("code")?.as_str()?.to_string();
        if code.is_empty() {
            return None;
        }
        let num = |k: &str| v.get(k).and_then(Value::as_u64).unwrap_or(0) as u32;
        let created_at = v
            .get("created_at")
            .and_then(Value::as_str)
            .map(timefmt::parse_iso)
            .unwrap_or(0);
        let max_age = num("max_age");
        let expires_at = match v.get("expires_at") {
            Some(Value::String(s)) => Some(timefmt::parse_iso(s)).filter(|t| *t > 0),
            Some(Value::Null) => None,
            // Not sent: work it out.
            _ => (max_age > 0 && created_at > 0).then(|| created_at + max_age as i64),
        };
        let inviter = v.get("inviter").unwrap_or(&Value::Null);
        let inviter_name = inviter
            .get("global_name")
            .and_then(Value::as_str)
            .or_else(|| inviter.get("username").and_then(Value::as_str))
            .unwrap_or_default()
            .to_string();
        Some(Self {
            code,
            channel_id: v
                .pointer("/channel/id")
                .or_else(|| v.get("channel_id"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            channel_name: v
                .pointer("/channel/name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            inviter_id: inviter
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            inviter_name,
            uses: num("uses"),
            max_uses: num("max_uses"),
            max_age,
            temporary: v.get("temporary").and_then(Value::as_bool).unwrap_or(false),
            created_at,
            expires_at,
        })
    }

    pub fn list_from_json(v: &Value) -> Vec<Self> {
        v.as_array()
            .map(|a| a.iter().filter_map(Self::from_json).collect())
            .unwrap_or_default()
    }

    pub fn link(&self) -> String {
        format!("https://discord.gg/{}", self.code)
    }

    /// Expired, or used up.
    pub fn is_dead(&self, now: i64) -> bool {
        self.expires_at.is_some_and(|t| t <= now)
            || (self.max_uses > 0 && self.uses >= self.max_uses)
    }

    /// "6d 23h", "3h 5m", "Never", "Expired".
    pub fn expires_label(&self, now: i64) -> String {
        match self.expires_at {
            None => "Never".into(),
            Some(t) if t <= now => "Expired".into(),
            Some(t) => format_remaining(t - now),
        }
    }

    /// "3 / 10" or "3".
    pub fn uses_label(&self) -> String {
        if self.max_uses > 0 {
            format!("{} / {}", self.uses, self.max_uses)
        } else {
            self.uses.to_string()
        }
    }
}

/// Compact remaining time: "6d 23h", "5h 12m", "12m", "<1m".
pub fn format_remaining(secs: i64) -> String {
    let secs = secs.max(0);
    let (d, h, m) = (secs / 86_400, secs % 86_400 / 3600, secs % 3600 / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        "<1m".into()
    }
}

/// Choices in Discord's invite settings.
pub const MAX_AGE_OPTIONS: &[(u32, &str)] = &[
    (1800, "30 minutes"),
    (3600, "1 hour"),
    (21_600, "6 hours"),
    (43_200, "12 hours"),
    (86_400, "1 day"),
    (604_800, "7 days"),
    (0, "Never"),
];

pub const MAX_USES_OPTIONS: &[(u32, &str)] = &[
    (0, "No limit"),
    (1, "1 use"),
    (5, "5 uses"),
    (10, "10 uses"),
    (25, "25 uses"),
    (50, "50 uses"),
    (100, "100 uses"),
];

/// Settings for a new invite.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InviteSettings {
    pub max_age: u32,
    pub max_uses: u32,
    /// Members are kicked when they go offline unless given a role.
    pub temporary: bool,
}

impl Default for InviteSettings {
    fn default() -> Self {
        Self {
            max_age: 604_800,
            max_uses: 0,
            temporary: false,
        }
    }
}

impl InviteSettings {
    /// `POST channels/{id}/invites` body. `unique: false` lets Discord hand
    /// back an existing invite with the same settings; "Generate a New
    /// Link" sends `unique: true`.
    pub fn body(&self, unique: bool) -> Value {
        json!({
            "max_age": self.max_age.min(604_800),
            "max_uses": self.max_uses.min(100),
            "temporary": self.temporary,
            "unique": unique,
        })
    }

    /// "Your invite link expires in 7 days." etc.
    pub fn sentence(&self) -> String {
        let age = MAX_AGE_OPTIONS
            .iter()
            .find(|(v, _)| *v == self.max_age)
            .map(|(_, l)| *l)
            .unwrap_or("a while");
        match (self.max_age, self.max_uses) {
            (0, 0) => "Your invite link will never expire.".into(),
            (0, n) => format!("Your invite link will never expire and works for {n} uses."),
            (_, 0) => format!("Your invite link expires in {age}."),
            (_, n) => format!("Your invite link expires in {age} or after {n} uses."),
        }
    }
}

/// Text channels first, then voice; only ones I can create invites for.
pub fn invitable_channels<'a>(guild: &'a Guild, my_id: &str) -> Vec<&'a crate::types::Channel> {
    let mut out: Vec<_> = guild
        .channels
        .iter()
        .filter(|c| p::invitable_kind(c.kind))
        .filter(|c| {
            let perms = p::channel_permissions_in(guild, &c.id, my_id);
            p::has(perms, p::VIEW_CHANNEL | p::CREATE_INSTANT_INVITE)
        })
        .collect();
    out.sort_by_key(|c| c.is_voice());
    out
}

/// Channel the dialog starts on: the open one if invitable, else the first.
pub fn default_invite_channel(
    guild: &Guild,
    my_id: &str,
    selected: Option<&str>,
) -> Option<String> {
    let list = invitable_channels(guild, my_id);
    selected
        .and_then(|s| list.iter().find(|c| c.id == s))
        .or_else(|| list.first())
        .map(|c| c.id.clone())
}

/// The invite dialog state.
#[derive(Debug, Clone)]
pub struct InviteDialog {
    pub guild_id: String,
    pub channel_id: Option<String>,
    pub settings: InviteSettings,
    /// Show the settings form ("Edit invite link").
    editing: bool,
    /// Last created / reused invite.
    pub created: Option<Invite>,
    /// Existing invites (if I may list them), for reuse.
    pub existing: Vec<Invite>,
    creating: bool,
    error: Option<String>,
    /// Link copied and when (shows "Copied" for 2 s).
    copied: Option<(String, f64)>,
}

impl InviteDialog {
    /// Opening does not create anything; returns the read request for
    /// existing invites when I'm allowed to list them.
    pub fn open(
        guild: &Guild,
        my_id: &str,
        channel: Option<&str>,
    ) -> (Self, Option<ServerRequest>) {
        let channel_id = default_invite_channel(guild, my_id, channel);
        let d = Self {
            guild_id: guild.id.clone(),
            channel_id: channel_id.clone(),
            settings: InviteSettings::default(),
            editing: false,
            created: None,
            existing: Vec::new(),
            creating: false,
            error: None,
            copied: None,
        };
        let gp = p::guild_permissions(guild, my_id);
        let load = if p::has(gp, p::MANAGE_GUILD) {
            Some(ServerRequest::LoadGuildInvites {
                guild_id: guild.id.clone(),
            })
        } else {
            channel_id
                .filter(|c| {
                    p::has(
                        p::channel_permissions_in(guild, c, my_id),
                        p::MANAGE_CHANNELS,
                    )
                })
                .map(|channel_id| ServerRequest::LoadChannelInvites { channel_id })
        };
        (d, load)
    }

    pub fn on_invites(&mut self, invites: &[Invite]) {
        self.existing = invites.to_vec();
    }

    pub fn on_created(&mut self, invite: Invite) {
        self.creating = false;
        self.editing = false;
        self.error = None;
        self.existing.retain(|i| i.code != invite.code);
        self.existing.insert(0, invite.clone());
        self.created = Some(invite);
    }

    pub fn on_revoked(&mut self, code: &str) {
        self.existing.retain(|i| i.code != code);
        if self.created.as_ref().is_some_and(|i| i.code == code) {
            self.created = None;
        }
    }

    pub fn on_failed(&mut self, error: String) {
        self.creating = false;
        self.error = Some(error);
    }

    /// Draw the modal. Returns `(close, request)`.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        guild: &Guild,
        my_id: &str,
        pal: &Palette,
    ) -> (bool, Option<ServerRequest>) {
        let mut req = None;
        let mut close = false;
        let now_secs = notify_now();
        let modal = egui::Modal::new(egui::Id::unique("rustcord_invite_dialog")).show(ctx, |ui| {
            ui.set_width(440.0);
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("Invite friends to {}", guild.name))
                        .bold()
                        .size(15.0)
                        .color(pal.strong),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::icon_button(ui, Icon::Close, 22.0, "Close", pal).clicked() {
                        close = true;
                    }
                });
            });
            let channels = invitable_channels(guild, my_id);
            if channels.is_empty() {
                ui.add_space(8.0);
                widgets::hint(
                    ui,
                    pal,
                    "You can't create invites for any channel in this server.",
                );
                return;
            }
            ui.add_space(4.0);
            let mut ch = self.channel_id.clone().unwrap_or_default();
            let opts: Vec<(String, String)> = channels
                .iter()
                .map(|c| {
                    (
                        c.id.clone(),
                        format!("{}{}", if c.is_voice() { "" } else { "#" }, c.name),
                    )
                })
                .collect();
            ui.horizontal(|ui| {
                ui.label(RichText::new("Channel").color(pal.muted));
                if widgets::combo(ui, "invite_channel", &mut ch, &opts, 220.0) {
                    self.channel_id = Some(ch.clone());
                    self.created = None;
                }
            });

            if self.editing || self.created.is_none() {
                widgets::field_label(ui, pal, "Expire after");
                let ages: Vec<(u32, String)> = MAX_AGE_OPTIONS
                    .iter()
                    .map(|(v, l)| (*v, l.to_string()))
                    .collect();
                widgets::combo(ui, "invite_age", &mut self.settings.max_age, &ages, 200.0);
                widgets::field_label(ui, pal, "Max number of uses");
                let uses: Vec<(u32, String)> = MAX_USES_OPTIONS
                    .iter()
                    .map(|(v, l)| (*v, l.to_string()))
                    .collect();
                widgets::combo(ui, "invite_uses", &mut self.settings.max_uses, &uses, 200.0);
                ui.add_space(10.0);
                widgets::toggle_row(
                    ui,
                    "Grant temporary membership",
                    "Members are removed when they go offline unless they get a role.",
                    &mut self.settings.temporary,
                    pal,
                );
                ui.add_space(10.0);
                widgets::hint(ui, pal, &self.settings.sentence());
                ui.add_space(8.0);
                let label = if self.created.is_some() {
                    "Generate a New Link"
                } else {
                    "Create Link"
                };
                let btn = pal.primary_button(if self.creating { "Creating…" } else { label });
                if ui
                    .add_enabled(!self.creating && self.channel_id.is_some(), btn)
                    .clicked()
                    && let Some(channel_id) = self.channel_id.clone()
                {
                    self.creating = true;
                    self.error = None;
                    req = Some(ServerRequest::CreateInvite {
                        channel_id,
                        body: self.settings.body(self.created.is_some()),
                    });
                }
            }

            if let Some(inv) = self.created.clone() {
                widgets::field_label(ui, pal, "Send a server invite link to a friend");
                self.link_row(ui, &inv, pal);
                ui.horizontal(|ui| {
                    widgets::hint(ui, pal, &self.settings.sentence());
                    if !self.editing && ui.link("Edit invite link.").clicked() {
                        self.editing = true;
                    }
                });
            }
            widgets::error_line(ui, pal, &self.error);

            let live: Vec<Invite> = self
                .existing
                .iter()
                .filter(|i| {
                    !i.is_dead(now_secs) && Some(&i.code) != self.created.as_ref().map(|c| &c.code)
                })
                .filter(|i| {
                    self.channel_id
                        .as_deref()
                        .is_none_or(|c| i.channel_id.is_empty() || i.channel_id == c)
                })
                .take(5)
                .cloned()
                .collect();
            if !live.is_empty() {
                widgets::field_label(ui, pal, "Existing invites");
                for inv in &live {
                    self.link_row(ui, inv, pal);
                    widgets::hint(
                        ui,
                        pal,
                        &format!(
                            "Uses {} · expires {}",
                            inv.uses_label(),
                            inv.expires_label(now_secs)
                        ),
                    );
                }
            }
        });
        if modal.should_close() {
            close = true;
        }
        (close, req)
    }

    fn link_row(&mut self, ui: &mut egui::Ui, inv: &Invite, pal: &Palette) {
        let link = inv.link();
        let now = ui.input(|i| i.time);
        let copied = self
            .copied
            .as_ref()
            .is_some_and(|(l, t)| *l == link && now - t < 2.0);
        if copied {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(500));
        }
        egui::Frame::new()
            .fill(pal.servers)
            .corner_radius(4)
            .inner_margin(egui::Margin::symmetric(8, 4))
            .show(ui, |ui| {
                ui.horizontal(|ui| {
                    let mut shown = link.clone();
                    ui.add(
                        egui::TextEdit::singleline(&mut shown)
                            .frame(egui::Frame::NONE)
                            .desired_width(300.0),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let btn = egui::Button::new(
                            RichText::new(if copied { "Copied" } else { "Copy" })
                                .color(egui::Color32::WHITE)
                                .bold(),
                        )
                        .fill(if copied { pal.online } else { pal.accent })
                        .min_size(vec2(72.0, 0.0));
                        if ui.add(btn).clicked() {
                            ui.ctx().copy_text(link.clone());
                            self.copied = Some((link.clone(), now));
                        }
                    });
                });
            });
    }
}

/// Current unix seconds.
pub fn notify_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// State for an invite list page (server- or channel-wide).
#[derive(Debug, Clone, Default)]
pub struct InviteList {
    pub invites: Option<Vec<Invite>>,
    pub error: Option<String>,
    confirm: Option<String>,
}

impl InviteList {
    pub fn set(&mut self, invites: Vec<Invite>) {
        let mut invites = invites;
        invites.sort_by_key(|i| std::cmp::Reverse(i.created_at));
        self.invites = Some(invites);
        self.error = None;
    }

    pub fn revoked(&mut self, code: &str) {
        if let Some(list) = &mut self.invites {
            list.retain(|i| i.code != code);
        }
    }

    /// Draw the table. `show_channel` adds the channel column (server page).
    /// Returns a revoke request after the user confirmed.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        show_channel: bool,
        pal: &Palette,
    ) -> Option<ServerRequest> {
        let mut out = None;
        widgets::error_line(ui, pal, &self.error);
        let Some(list) = &self.invites else {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading invites…").color(pal.muted));
            });
            return None;
        };
        if list.is_empty() {
            ui.add_space(20.0);
            widgets::hint(
                ui,
                pal,
                "No invites yet. Create one from the server menu's Invite People.",
            );
            return None;
        }
        let now = notify_now();
        let cols: &[(&str, f32)] = if show_channel {
            &[
                ("Inviter", 0.28),
                ("Invite code", 0.22),
                ("Channel", 0.2),
                ("Uses", 0.12),
                ("Expires", 0.18),
            ]
        } else {
            &[
                ("Inviter", 0.36),
                ("Invite code", 0.28),
                ("Uses", 0.14),
                ("Expires", 0.22),
            ]
        };
        let width = ui.available_width() - 30.0;
        ui.horizontal(|ui| {
            for (name, frac) in cols {
                ui.add_sized(
                    vec2(width * frac, 18.0),
                    egui::Label::new(pal.section_label(name)),
                );
            }
        });
        widgets::divider(ui, pal);
        for inv in list {
            let row = ui.horizontal(|ui| {
                let mut cells = vec![
                    if inv.inviter_name.is_empty() {
                        "Unknown".into()
                    } else {
                        inv.inviter_name.clone()
                    },
                    inv.code.clone(),
                ];
                if show_channel {
                    cells.push(format!("#{}", inv.channel_name));
                }
                cells.push(inv.uses_label());
                cells.push(inv.expires_label(now));
                for ((_, frac), text) in cols.iter().zip(cells) {
                    let col = if text == inv.code {
                        pal.strong
                    } else {
                        pal.text
                    };
                    ui.add_sized(
                        vec2(width * frac, 24.0),
                        egui::Label::new(RichText::new(text).color(col)).truncate(),
                    );
                }
                widgets::icon_button(ui, Icon::Close, 22.0, "Revoke invite", pal)
            });
            if row.inner.clicked() {
                self.confirm = Some(inv.code.clone());
            }
        }
        if let Some(code) = self.confirm.clone() {
            let c = Confirm::new(
                "Revoke Invite",
                format!(
                    "Are you sure you want to revoke discord.gg/{code}? Nobody will be able to join with it any more."
                ),
                "Revoke",
            );
            match c.show(ui.ctx(), pal) {
                Some(true) => {
                    self.confirm = None;
                    out = Some(ServerRequest::RevokeInvite { code });
                }
                Some(false) => self.confirm = None,
                None => {}
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_invite_metadata() {
        let v = json!({
            "code": "abc123",
            "guild": { "id": "1", "name": "S" },
            "channel": { "id": "10", "name": "general", "type": 0 },
            "inviter": { "id": "5", "username": "leo", "global_name": "Leo" },
            "uses": 3, "max_uses": 10, "max_age": 86400, "temporary": true,
            "created_at": "2026-11-01T10:00:00+00:00"
        });
        let i = Invite::from_json(&v).unwrap();
        assert_eq!(i.code, "abc123");
        assert_eq!(i.channel_id, "10");
        assert_eq!(i.channel_name, "general");
        assert_eq!(i.inviter_name, "Leo");
        assert_eq!((i.uses, i.max_uses, i.max_age), (3, 10, 86400));
        assert!(i.temporary);
        assert_eq!(i.expires_at, Some(i.created_at + 86400));
        assert_eq!(i.link(), "https://discord.gg/abc123");
        assert_eq!(i.uses_label(), "3 / 10");
        assert_eq!(i.expires_label(i.created_at + 3600), "23h 0m");
        assert_eq!(i.expires_label(i.created_at + 86400), "Expired");
        assert!(i.is_dead(i.created_at + 90000));
        assert!(!i.is_dead(i.created_at + 10));

        // Never expires (explicit null), no inviter, used up.
        let v = json!({ "code": "x", "max_age": 0, "expires_at": null, "uses": 1, "max_uses": 1 });
        let i = Invite::from_json(&v).unwrap();
        assert_eq!(i.expires_at, None);
        assert_eq!(i.expires_label(0), "Never");
        assert!(i.is_dead(0));
        assert_eq!(i.inviter_name, "");

        // expires_at given directly wins.
        let v = json!({ "code": "y", "max_age": 60, "created_at": "2026-11-01T10:00:00+00:00",
                        "expires_at": "2026-11-02T10:00:00+00:00" });
        let i = Invite::from_json(&v).unwrap();
        assert_eq!(i.expires_at.unwrap() - i.created_at, 86400);

        // Junk.
        assert!(Invite::from_json(&json!({ "code": "" })).is_none());
        assert!(Invite::from_json(&json!("abc")).is_none());
        let list = Invite::list_from_json(&json!([{ "code": "a" }, {}, 5, { "code": "b" }]));
        assert_eq!(list.len(), 2);
        assert!(Invite::list_from_json(&json!({ "message": "Missing Permissions" })).is_empty());
    }

    #[test]
    fn remaining_labels() {
        assert_eq!(format_remaining(7 * 86_400 - 1), "6d 23h");
        assert_eq!(format_remaining(3 * 3600 + 5 * 60), "3h 5m");
        assert_eq!(format_remaining(125), "2m");
        assert_eq!(format_remaining(10), "<1m");
        assert_eq!(format_remaining(-5), "<1m");
    }

    #[test]
    fn settings_body_and_sentence() {
        let s = InviteSettings::default();
        assert_eq!(
            s.body(false),
            json!({ "max_age": 604800, "max_uses": 0, "temporary": false, "unique": false })
        );
        assert_eq!(s.sentence(), "Your invite link expires in 7 days.");
        let s = InviteSettings {
            max_age: 0,
            max_uses: 0,
            temporary: true,
        };
        assert_eq!(s.body(true)["unique"], true);
        assert_eq!(s.sentence(), "Your invite link will never expire.");
        let s = InviteSettings {
            max_age: 1800,
            max_uses: 5,
            temporary: false,
        };
        assert_eq!(
            s.sentence(),
            "Your invite link expires in 30 minutes or after 5 uses."
        );
        // Clamped to Discord's limits.
        let s = InviteSettings {
            max_age: 9_999_999,
            max_uses: 1000,
            temporary: false,
        };
        assert_eq!(s.body(false)["max_age"], 604800);
        assert_eq!(s.body(false)["max_uses"], 100);
    }

    #[test]
    fn invite_channel_choice() {
        let g = Guild::from_json(&json!({
            "id": "1", "owner_id": "me",
            "channels": [
                { "id": "4", "type": 4, "name": "cat", "position": 0 },
                { "id": "2", "type": 2, "name": "voice", "position": 0 },
                { "id": "3", "type": 0, "name": "general", "position": 1 }
            ]
        }));
        let ids: Vec<_> = invitable_channels(&g, "me")
            .iter()
            .map(|c| c.id.clone())
            .collect();
        assert_eq!(ids, ["3", "2"]);
        assert_eq!(
            default_invite_channel(&g, "me", Some("2")).as_deref(),
            Some("2")
        );
        assert_eq!(
            default_invite_channel(&g, "me", Some("4")).as_deref(),
            Some("3")
        );
        assert_eq!(default_invite_channel(&g, "someone", None), None);
    }
}
