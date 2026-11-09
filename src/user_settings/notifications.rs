//! Notification settings dialog for a server (opened from the server
//! dropdown) or one channel / category (opened from its context menu).
//!
//! Protocol: `PATCH users/@me/guilds/{guild_id}/settings` (`@me` for DMs) with
//! a partial user guild settings object; Discord answers with the full
//! updated entry and sends `USER_GUILD_SETTINGS_UPDATE`. Body fields (as
//! Discord's client sends them; documented by docs.discord.food, not
//! officially):
//! - `muted: bool`, `mute_config: { selected_time_window, end_time } | null`
//!   — `selected_time_window` is the duration in seconds or `-1` for "until I
//!   turn it back on"; `end_time` is the ISO-8601 end (null when `-1`).
//! - `message_notifications`: 0 all, 1 only @mentions, 2 nothing, 3 inherit
//!   (channel overrides only: use the server setting).
//! - `suppress_everyone`, `suppress_roles`, `mobile_push`: bools.
//! - `channel_overrides`: an OBJECT keyed by channel id (the READY /
//!   response form is an array): `{ "<id>": { muted, mute_config,
//!   message_notifications } }`. A category override covers its channels.
//!
//! UNCERTAIN: removing an override. Discord's client has no delete; we send
//! the override back to defaults (`muted: false, mute_config: null,
//! message_notifications: 3`), which makes it inert. Also `mobile_push` is
//! not kept by `notify::GuildSettings`; [`NotifDraft::from_json`] reads it
//! from a raw entry, otherwise it is assumed on (Discord's default).

use std::collections::BTreeMap;

use chrono::{DateTime, SecondsFormat, Utc};
use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};

use super::widgets::{self, Icon};
use super::{Action, Ctx, Outcome, Request};
use crate::notify::{GuildSettings, NOTIFY_ALL, NOTIFY_INHERIT, NOTIFY_MENTIONS, NOTIFY_NOTHING};
use crate::theme::Bold;
use crate::types::{Channel, DM_GUILD_ID, Guild};

const CHANNEL_CATEGORY: u64 = 4;

/// How long a new mute lasts (Discord's choices).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MuteDuration {
    Minutes15,
    Hour1,
    Hours3,
    Hours8,
    Hours24,
    #[default]
    Forever,
}

impl MuteDuration {
    pub const ALL: [MuteDuration; 6] = [
        MuteDuration::Minutes15,
        MuteDuration::Hour1,
        MuteDuration::Hours3,
        MuteDuration::Hours8,
        MuteDuration::Hours24,
        MuteDuration::Forever,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MuteDuration::Minutes15 => "For 15 Minutes",
            MuteDuration::Hour1 => "For 1 Hour",
            MuteDuration::Hours3 => "For 3 Hours",
            MuteDuration::Hours8 => "For 8 Hours",
            MuteDuration::Hours24 => "For 24 Hours",
            MuteDuration::Forever => "Until I turn it back on",
        }
    }

    /// `selected_time_window`: seconds, or -1 for forever.
    pub fn seconds(self) -> i64 {
        match self {
            MuteDuration::Minutes15 => 15 * 60,
            MuteDuration::Hour1 => 3600,
            MuteDuration::Hours3 => 3 * 3600,
            MuteDuration::Hours8 => 8 * 3600,
            MuteDuration::Hours24 => 24 * 3600,
            MuteDuration::Forever => -1,
        }
    }

    /// The `mute_config` object for a mute starting `now`.
    pub fn mute_config(self, now: DateTime<Utc>) -> Value {
        let secs = self.seconds();
        let end = (secs > 0).then(|| {
            (now + chrono::Duration::seconds(secs)).to_rfc3339_opts(SecondsFormat::Millis, true)
        });
        json!({ "selected_time_window": secs, "end_time": end })
    }
}

/// Mute state being edited.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MuteDraft {
    pub muted: bool,
    /// End of an existing timed mute (ISO-8601).
    pub until: Option<String>,
    /// Set when the user turned the mute on in this dialog.
    pub new_window: Option<MuteDuration>,
}

impl MuteDraft {
    fn from_mute(m: &crate::notify::Mute, now: &str) -> Self {
        let active = m.active(now);
        Self {
            muted: active,
            until: m.until.clone().filter(|_| active),
            new_window: None,
        }
    }

    fn set(&mut self, on: bool) {
        self.muted = on;
        self.until = None;
        self.new_window = on.then_some(MuteDuration::Forever);
    }

    fn changed(&self, orig: &MuteDraft) -> bool {
        self.muted != orig.muted || self.new_window.is_some()
    }

    fn write(&self, body: &mut Map<String, Value>, now: DateTime<Utc>) {
        body.insert("muted".into(), json!(self.muted));
        let cfg = match (self.muted, self.new_window) {
            (true, Some(w)) => w.mute_config(now),
            _ => Value::Null,
        };
        body.insert("mute_config".into(), cfg);
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct OverrideDraft {
    pub mute: MuteDraft,
    /// 0..=3 (3 = use the server setting).
    pub level: u8,
}

impl Default for OverrideDraft {
    fn default() -> Self {
        Self {
            mute: MuteDraft::default(),
            level: NOTIFY_INHERIT,
        }
    }
}

/// One server's notification settings being edited.
#[derive(Debug, Clone, PartialEq)]
pub struct NotifDraft {
    pub guild_id: String,
    pub mute: MuteDraft,
    pub level: u8,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    pub mobile_push: bool,
    pub overrides: BTreeMap<String, OverrideDraft>,
}

impl NotifDraft {
    /// From the app's `Mutes` entry (None = never changed: defaults).
    /// `now` is ISO-8601 UTC.
    pub fn from_settings(guild_id: &str, s: Option<&GuildSettings>, now: &str) -> Self {
        let Some(s) = s else {
            return Self {
                guild_id: guild_id.to_string(),
                mute: MuteDraft::default(),
                level: NOTIFY_INHERIT,
                suppress_everyone: false,
                suppress_roles: false,
                mobile_push: true,
                overrides: BTreeMap::new(),
            };
        };
        Self {
            guild_id: guild_id.to_string(),
            mute: MuteDraft::from_mute(&s.mute, now),
            level: s.level,
            suppress_everyone: s.suppress_everyone,
            suppress_roles: s.suppress_roles,
            mobile_push: true,
            overrides: s
                .overrides
                .iter()
                // Inert overrides (inherit + not muted) aren't shown.
                .filter(|(_, o)| o.level != NOTIFY_INHERIT || o.mute.active(now))
                .map(|(id, o)| {
                    (
                        id.clone(),
                        OverrideDraft {
                            mute: MuteDraft::from_mute(&o.mute, now),
                            level: o.level,
                        },
                    )
                })
                .collect(),
        }
    }

    /// From a raw user guild settings entry (keeps `mobile_push`).
    pub fn from_json(v: &Value, now: &str) -> Self {
        let s = GuildSettings::from_json(v);
        let mut d = Self::from_settings(&s.guild_id, Some(&s), now);
        d.mobile_push = v
            .get("mobile_push")
            .and_then(Value::as_bool)
            .unwrap_or(true);
        d
    }
}

/// `now` as Discord-style ISO-8601 (for [`crate::notify::Mute::active`]).
pub fn iso(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// The PATCH body for what changed between `orig` and `new`, or None.
pub fn patch_body(orig: &NotifDraft, new: &NotifDraft, now: DateTime<Utc>) -> Option<Value> {
    let mut body = Map::new();
    if new.mute.changed(&orig.mute) {
        new.mute.write(&mut body, now);
    }
    if new.level != orig.level {
        body.insert("message_notifications".into(), json!(new.level));
    }
    if new.suppress_everyone != orig.suppress_everyone {
        body.insert("suppress_everyone".into(), json!(new.suppress_everyone));
    }
    if new.suppress_roles != orig.suppress_roles {
        body.insert("suppress_roles".into(), json!(new.suppress_roles));
    }
    if new.mobile_push != orig.mobile_push {
        body.insert("mobile_push".into(), json!(new.mobile_push));
    }
    let mut overrides = Map::new();
    for (id, o) in &new.overrides {
        let before = orig.overrides.get(id).cloned().unwrap_or_default();
        if o.level == before.level && !o.mute.changed(&before.mute) {
            continue;
        }
        let mut e = Map::new();
        e.insert("channel_id".into(), json!(id));
        e.insert("message_notifications".into(), json!(o.level));
        if o.mute.changed(&before.mute) {
            o.mute.write(&mut e, now);
        } else {
            e.insert("muted".into(), json!(o.mute.muted));
        }
        overrides.insert(id.clone(), Value::Object(e));
    }
    for id in orig.overrides.keys() {
        if !new.overrides.contains_key(id) {
            overrides.insert(
                id.clone(),
                json!({ "channel_id": id, "muted": false, "mute_config": null,
                        "message_notifications": NOTIFY_INHERIT }),
            );
        }
    }
    if !overrides.is_empty() {
        body.insert("channel_overrides".into(), Value::Object(overrides));
    }
    (!body.is_empty()).then_some(Value::Object(body))
}

fn level_label(l: u8) -> &'static str {
    match l {
        NOTIFY_ALL => "All Messages",
        NOTIFY_MENTIONS => "Only @mentions",
        NOTIFY_NOTHING => "Nothing",
        _ => "Default",
    }
}

fn channel_name(c: &Channel) -> String {
    if c.kind == CHANNEL_CATEGORY {
        c.name.to_uppercase()
    } else {
        format!("# {}", c.name)
    }
}

/// The dialog. Create with [`NotificationDialog::open`] /
/// [`NotificationDialog::open_channel`], call [`show`](Self::show) every
/// frame until it returns [`Action::CloseDialog`], and route
/// `Outcome::NotificationsSaved` to [`apply`](Self::apply).
pub struct NotificationDialog {
    orig: NotifDraft,
    draft: NotifDraft,
    /// Per-channel mode: only this channel / category's override.
    channel: Option<String>,
    saving: bool,
    error: Option<String>,
    done: bool,
}

impl NotificationDialog {
    /// Server dialog. `settings` from `Mutes::get(guild_id)`.
    pub fn open(guild_id: &str, settings: Option<&GuildSettings>) -> Self {
        let now = iso(Utc::now());
        let orig = NotifDraft::from_settings(guild_id, settings, &now);
        Self {
            draft: orig.clone(),
            orig,
            channel: None,
            saving: false,
            error: None,
            done: false,
        }
    }

    /// Channel / category dialog (DM channels: `guild_id` = `DM_GUILD_ID`).
    pub fn open_channel(
        guild_id: &str,
        channel_id: &str,
        settings: Option<&GuildSettings>,
    ) -> Self {
        let mut d = Self::open(guild_id, settings);
        d.channel = Some(channel_id.to_string());
        d
    }

    pub fn guild_id(&self) -> &str {
        &self.orig.guild_id
    }

    pub fn apply(&mut self, outcome: &Outcome) {
        if let Outcome::NotificationsSaved { guild_id, result } = outcome {
            if guild_id != &self.orig.guild_id || !self.saving {
                return;
            }
            self.saving = false;
            match result {
                Ok(_) => self.done = true,
                Err(e) => self.error = Some(e.clone()),
            }
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, c: &Ctx) -> Option<Action> {
        if self.done {
            return Some(Action::CloseDialog);
        }
        let pal = c.pal;
        let guild = c.guilds.iter().find(|g| g.id == self.orig.guild_id);
        let mut action = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique(("rc_us_notif", &self.orig.guild_id))).show(
            ui.ctx(),
            |ui| {
                ui.set_width(480.0);
                let title = match (&self.channel, guild) {
                    (Some(id), Some(g)) => g
                        .channel(id)
                        .map(|ch| format!("{} notification settings", channel_name(ch)))
                        .unwrap_or_else(|| "Notification Settings".into()),
                    (None, Some(g)) => format!("{} — Notification Settings", g.name),
                    _ => "Notification Settings".into(),
                };
                ui.horizontal(|ui| {
                    ui.label(RichText::new(title).bold().size(18.0).color(pal.strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, pal, "Close").clicked() {
                            close = true;
                        }
                    });
                });
                ui.add_space(10.0);
                egui::ScrollArea::vertical()
                    .max_height(520.0)
                    .show(ui, |ui| match self.channel.clone() {
                        Some(id) => self.channel_ui(ui, c, guild, &id),
                        None => self.server_ui(ui, c, guild),
                    });
                if let Some(e) = &self.error {
                    widgets::error(ui, pal, e);
                }
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if self.saving {
                            ui.spinner();
                        } else if ui.add(pal.primary_button("Done")).clicked() {
                            match patch_body(&self.orig, &self.draft, Utc::now()) {
                                Some(body) => {
                                    self.saving = true;
                                    self.error = None;
                                    action = Some(Action::Request(Request::SaveNotifications {
                                        guild_id: self.orig.guild_id.clone(),
                                        body,
                                    }));
                                }
                                None => close = true,
                            }
                        }
                    });
                });
            },
        );
        if (close || modal.should_close()) && action.is_none() {
            return Some(Action::CloseDialog);
        }
        action
    }

    fn mute_block(ui: &mut egui::Ui, c: &Ctx, mute: &mut MuteDraft, what: &str, desc: &str) {
        let pal = c.pal;
        let mut on = mute.muted;
        if widgets::switch_row(ui, &mut on, &format!("Mute {what}"), desc, true, pal) {
            mute.set(on);
        }
        if mute.muted {
            match (&mut mute.new_window, &mute.until) {
                (Some(w), _) => {
                    egui::ComboBox::from_id_salt(("rc_us_mute_for", what))
                        .selected_text(w.label())
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for d in MuteDuration::ALL {
                                ui.selectable_value(w, d, d.label());
                            }
                        });
                }
                (None, Some(until)) => widgets::hint(
                    ui,
                    pal,
                    &format!("Muted until {}", crate::timefmt::full_iso(until)),
                ),
                (None, None) => widgets::hint(ui, pal, "Muted until you turn it back on."),
            }
            ui.add_space(10.0);
        }
    }

    fn server_ui(&mut self, ui: &mut egui::Ui, c: &Ctx, guild: Option<&Guild>) {
        let pal = c.pal;
        let is_dm = self.orig.guild_id == DM_GUILD_ID;
        let what = guild
            .map(|g| g.name.clone())
            .unwrap_or_else(|| "server".into());
        Self::mute_block(
            ui,
            c,
            &mut self.draft.mute,
            &what,
            "Muting a server prevents unread indicators and notifications from appearing \
             unless you are mentioned.",
        );
        widgets::label(ui, pal, "SERVER NOTIFICATION SETTINGS");
        let default = guild
            .map(|g| g.default_notifications.min(1))
            .unwrap_or(NOTIFY_ALL);
        let shown = if self.draft.level == NOTIFY_INHERIT {
            default
        } else {
            self.draft.level
        };
        for l in [NOTIFY_ALL, NOTIFY_MENTIONS, NOTIFY_NOTHING] {
            let text = if l == default && self.draft.level == NOTIFY_INHERIT {
                format!("{} (server default)", level_label(l))
            } else {
                level_label(l).to_string()
            };
            if ui.radio(shown == l, text).clicked() {
                self.draft.level = l;
            }
        }
        ui.add_space(10.0);
        widgets::divider(ui, pal);
        ui.add_space(10.0);
        if !is_dm {
            widgets::switch_row(
                ui,
                &mut self.draft.suppress_everyone,
                "Suppress @everyone and @here",
                "",
                true,
                pal,
            );
            widgets::switch_row(
                ui,
                &mut self.draft.suppress_roles,
                "Suppress All Role @mentions",
                "",
                true,
                pal,
            );
        }
        widgets::switch_row(
            ui,
            &mut self.draft.mobile_push,
            "Mobile Push Notifications",
            "Applies to Discord's mobile apps; RustCord's desktop notifications follow the \
             settings above.",
            true,
            pal,
        );
        if let Some(g) = guild.filter(|_| !is_dm) {
            self.overrides_ui(ui, c, g);
        }
    }

    fn overrides_ui(&mut self, ui: &mut egui::Ui, c: &Ctx, g: &Guild) {
        let pal = c.pal;
        widgets::label(ui, pal, "NOTIFICATION OVERRIDES");
        widgets::hint(
            ui,
            pal,
            "Add a channel or category to override its server notification settings.",
        );
        let mut add = None;
        egui::ComboBox::from_id_salt("rc_us_override_add")
            .selected_text("Select a channel or category")
            .width(300.0)
            .height(260.0)
            .show_ui(ui, |ui| {
                for ch in g
                    .channels
                    .iter()
                    .filter(|ch| ch.kind == CHANNEL_CATEGORY || ch.is_text() || ch.is_voice())
                    .filter(|ch| !self.draft.overrides.contains_key(&ch.id))
                {
                    if ui.selectable_label(false, channel_name(ch)).clicked() {
                        add = Some(ch.id.clone());
                    }
                }
            });
        if let Some(id) = add {
            self.draft.overrides.insert(id, OverrideDraft::default());
        }
        ui.add_space(6.0);
        let mut remove = None;
        for (id, o) in self.draft.overrides.iter_mut() {
            let name = g
                .channel(id)
                .map(channel_name)
                .unwrap_or_else(|| format!("Unknown channel {id}"));
            widgets::card(ui, pal, |ui| {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(name).bold().color(pal.strong));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if widgets::icon_button(ui, Icon::Close, pal, "Remove override").clicked() {
                            remove = Some(id.clone());
                        }
                        let mut muted = o.mute.muted;
                        if ui.checkbox(&mut muted, "Mute").changed() {
                            o.mute.set(muted);
                        }
                    });
                });
                ui.horizontal(|ui| {
                    for l in [NOTIFY_INHERIT, NOTIFY_ALL, NOTIFY_MENTIONS, NOTIFY_NOTHING] {
                        let text = if l == NOTIFY_INHERIT {
                            "Default"
                        } else {
                            level_label(l)
                        };
                        ui.selectable_value(&mut o.level, l, text);
                    }
                });
            });
            ui.add_space(4.0);
        }
        if let Some(id) = remove {
            self.draft.overrides.remove(&id);
        }
    }

    fn channel_ui(&mut self, ui: &mut egui::Ui, c: &Ctx, guild: Option<&Guild>, id: &str) {
        let pal = c.pal;
        let ch = guild.and_then(|g| g.channel(id));
        let is_cat = ch.is_some_and(|ch| ch.kind == CHANNEL_CATEGORY);
        let what = if is_cat { "category" } else { "channel" };
        let o = self.draft.overrides.entry(id.to_string()).or_default();
        Self::mute_block(
            ui,
            c,
            &mut o.mute,
            what,
            if is_cat {
                "Mutes every channel in this category unless it has its own setting."
            } else {
                "Muting a channel hides its unread indicator and notifications unless you \
                 are mentioned."
            },
        );
        widgets::label(ui, pal, "NOTIFICATION SETTINGS");
        let server = if self.draft.level == NOTIFY_INHERIT {
            guild
                .map(|g| g.default_notifications.min(1))
                .unwrap_or(NOTIFY_ALL)
        } else {
            self.draft.level
        };
        ui.radio_value(
            &mut o.level,
            NOTIFY_INHERIT,
            format!("Use server default ({})", level_label(server)),
        );
        for l in [NOTIFY_ALL, NOTIFY_MENTIONS, NOTIFY_NOTHING] {
            ui.radio_value(&mut o.level, l, level_label(l));
        }
        // An untouched, inert entry isn't an override: drop it again so it
        // doesn't produce a PATCH.
        if *o == OverrideDraft::default() && !self.orig.overrides.contains_key(id) {
            self.draft.overrides.remove(id);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::notify::{ChannelOverride, Mute};
    use std::collections::HashMap;

    fn now() -> DateTime<Utc> {
        DateTime::parse_from_rfc3339("2026-11-09T12:00:00Z")
            .unwrap()
            .with_timezone(&Utc)
    }

    fn settings() -> GuildSettings {
        let mut overrides = HashMap::new();
        overrides.insert(
            "10".to_string(),
            ChannelOverride {
                mute: Mute {
                    muted: true,
                    until: None,
                },
                level: NOTIFY_INHERIT,
            },
        );
        overrides.insert(
            "11".to_string(),
            ChannelOverride {
                mute: Mute::default(),
                level: NOTIFY_INHERIT,
            },
        );
        GuildSettings {
            guild_id: "1".into(),
            mute: Mute {
                muted: true,
                // Expired: shown as not muted.
                until: Some("2026-11-01T00:00:00.000Z".into()),
            },
            level: NOTIFY_MENTIONS,
            suppress_everyone: true,
            suppress_roles: false,
            overrides,
        }
    }

    #[test]
    fn mute_configs() {
        assert_eq!(
            MuteDuration::Hour1.mute_config(now()),
            json!({ "selected_time_window": 3600, "end_time": "2026-11-09T13:00:00.000Z" })
        );
        assert_eq!(
            MuteDuration::Forever.mute_config(now()),
            json!({ "selected_time_window": -1, "end_time": null })
        );
    }

    #[test]
    fn draft_from_settings() {
        let d = NotifDraft::from_settings("1", Some(&settings()), &iso(now()));
        assert!(!d.mute.muted, "expired mute is off");
        assert_eq!(d.level, NOTIFY_MENTIONS);
        assert!(d.suppress_everyone && d.mobile_push);
        // The inert override "11" is hidden.
        assert_eq!(d.overrides.keys().collect::<Vec<_>>(), ["10"]);
        let none = NotifDraft::from_settings("2", None, &iso(now()));
        assert_eq!(none.level, NOTIFY_INHERIT);
        let raw = json!({ "guild_id": "3", "mobile_push": false, "muted": false });
        assert!(!NotifDraft::from_json(&raw, &iso(now())).mobile_push);
    }

    #[test]
    fn patch_bodies() {
        let orig = NotifDraft::from_settings("1", Some(&settings()), &iso(now()));
        assert_eq!(patch_body(&orig, &orig, now()), None);

        let mut d = orig.clone();
        d.mute.set(true);
        d.mute.new_window = Some(MuteDuration::Minutes15);
        d.level = NOTIFY_NOTHING;
        d.suppress_roles = true;
        d.mobile_push = false;
        assert_eq!(
            patch_body(&orig, &d, now()),
            Some(json!({
                "muted": true,
                "mute_config": { "selected_time_window": 900, "end_time": "2026-11-09T12:15:00.000Z" },
                "message_notifications": 2,
                "suppress_roles": true,
                "mobile_push": false
            }))
        );

        // Overrides: change one, add one, remove one.
        let mut d = orig.clone();
        d.overrides.remove("10");
        d.overrides.insert(
            "12".into(),
            OverrideDraft {
                mute: MuteDraft::default(),
                level: NOTIFY_ALL,
            },
        );
        let body = patch_body(&orig, &d, now()).unwrap();
        assert_eq!(
            body,
            json!({ "channel_overrides": {
                "10": { "channel_id": "10", "muted": false, "mute_config": null, "message_notifications": 3 },
                "12": { "channel_id": "12", "muted": false, "message_notifications": 0 }
            }})
        );

        // Unmuting an override sends mute_config null.
        let mut d = orig.clone();
        d.overrides.get_mut("10").unwrap().mute.set(false);
        assert_eq!(
            patch_body(&orig, &d, now()).unwrap()["channel_overrides"]["10"],
            json!({ "channel_id": "10", "muted": false, "mute_config": null, "message_notifications": 3 })
        );

        // Turning the server mute off.
        let mut muted = orig.clone();
        muted.mute.muted = true;
        let mut d = muted.clone();
        d.mute.set(false);
        assert_eq!(
            patch_body(&muted, &d, now()),
            Some(json!({ "muted": false, "mute_config": null }))
        );
    }
}
