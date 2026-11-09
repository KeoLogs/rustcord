//! Mute settings, mention detection and desktop notifications.
//!
//! Ported from Discord Messenger's `core/state/UserGuildSettings.cpp`,
//! `core/state/NotificationManager.cpp` and `Message::CheckWasMentioned`.
//!
//! - READY `user_guild_settings` (user accounts only): `{ entries: [...],
//!   version, partial }`. Each entry has `guild_id` (null = DMs), `muted`,
//!   `mute_config: { end_time, selected_time_window }`,
//!   `message_notifications` (0 all, 1 mentions, 2 nothing, 3 inherit),
//!   `suppress_everyone`, `suppress_roles` and `channel_overrides` (same
//!   fields per channel; a category override covers its channels).
//! - `USER_GUILD_SETTINGS_UPDATE` sends one full entry.

use std::collections::{HashMap, VecDeque};

use serde_json::Value;

use crate::types::{DM_GUILD_ID, Message};

pub const NOTIFY_ALL: u8 = 0;
pub const NOTIFY_MENTIONS: u8 = 1;
pub const NOTIFY_NOTHING: u8 = 2;
pub const NOTIFY_INHERIT: u8 = 3;

/// Most mentions kept in the inbox.
pub const INBOX_MAX: usize = 50;

// ---------------------------------------------------------------------------
// Mute / notification settings
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Mute {
    pub muted: bool,
    /// ISO-8601 end of a timed mute; None = until turned off.
    pub until: Option<String>,
}

impl Mute {
    fn from_json(v: &Value) -> Self {
        let muted = v.get("muted").and_then(Value::as_bool).unwrap_or(false);
        let cfg = v.get("mute_config").filter(|c| c.is_object());
        // selected_time_window -1 = "until I turn it back on".
        let forever = cfg
            .and_then(|c| c.get("selected_time_window"))
            .and_then(Value::as_i64)
            == Some(-1);
        let until = cfg
            .and_then(|c| c.get("end_time"))
            .and_then(Value::as_str)
            .filter(|_| !forever)
            .map(str::to_string);
        Self { muted, until }
    }

    /// `now` is an ISO-8601 UTC time (see [`now_iso`]).
    pub fn active(&self, now: &str) -> bool {
        self.muted
            && match &self.until {
                None => true,
                // Same format, so comparing the "YYYY-MM-DDTHH:MM:SS" prefix works.
                Some(end) => end.get(..19).unwrap_or(end) > now.get(..19).unwrap_or(now),
            }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ChannelOverride {
    pub mute: Mute,
    pub level: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GuildSettings {
    /// Guild id, or [`DM_GUILD_ID`] for DMs.
    pub guild_id: String,
    pub mute: Mute,
    pub level: u8,
    pub suppress_everyone: bool,
    pub suppress_roles: bool,
    pub overrides: HashMap<String, ChannelOverride>,
}

fn flag(v: &Value, key: &str) -> bool {
    v.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn level(v: &Value, default: u8) -> u8 {
    v.get("message_notifications")
        .and_then(Value::as_u64)
        .map_or(default, |n| n.min(3) as u8)
}

impl GuildSettings {
    pub fn from_json(v: &Value) -> Self {
        let overrides = v
            .get("channel_overrides")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|o| {
                        let id = o.get("channel_id").and_then(Value::as_str)?;
                        Some((
                            id.to_string(),
                            ChannelOverride {
                                mute: Mute::from_json(o),
                                level: level(o, NOTIFY_INHERIT),
                            },
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            guild_id: v
                .get("guild_id")
                .and_then(Value::as_str)
                .unwrap_or(DM_GUILD_ID)
                .to_string(),
            mute: Mute::from_json(v),
            level: level(v, NOTIFY_INHERIT),
            suppress_everyone: flag(v, "suppress_everyone"),
            suppress_roles: flag(v, "suppress_roles"),
            overrides,
        }
    }

    /// READY `user_guild_settings`: `{ entries: [...] }` or a bare array.
    pub fn parse_ready(v: &Value) -> Vec<Self> {
        let list = v.get("entries").and_then(Value::as_array).or(v.as_array());
        list.map(|arr| arr.iter().map(Self::from_json).collect())
            .unwrap_or_default()
    }
}

/// Everyone's settings, keyed by guild id ([`DM_GUILD_ID`] for DMs).
#[derive(Debug, Default)]
pub struct Mutes {
    guilds: HashMap<String, GuildSettings>,
}

impl Mutes {
    pub fn replace_all(&mut self, list: Vec<GuildSettings>) {
        self.guilds = list.into_iter().map(|g| (g.guild_id.clone(), g)).collect();
    }

    pub fn update(&mut self, g: GuildSettings) {
        self.guilds.insert(g.guild_id.clone(), g);
    }

    pub fn get(&self, guild_id: &str) -> Option<&GuildSettings> {
        self.guilds.get(guild_id)
    }

    pub fn guild_muted(&self, guild_id: &str, now: &str) -> bool {
        self.get(guild_id).is_some_and(|g| g.mute.active(now))
    }

    /// The channel itself, or its category, is muted (not the whole server).
    pub fn channel_muted(
        &self,
        guild_id: &str,
        channel_id: &str,
        parent_id: Option<&str>,
        now: &str,
    ) -> bool {
        let Some(g) = self.get(guild_id) else {
            return false;
        };
        let muted = |id: &str| g.overrides.get(id).is_some_and(|o| o.mute.active(now));
        muted(channel_id) || parent_id.is_some_and(muted)
    }

    /// Server or channel muted.
    pub fn muted(
        &self,
        guild_id: &str,
        channel_id: &str,
        parent_id: Option<&str>,
        now: &str,
    ) -> bool {
        self.guild_muted(guild_id, now) || self.channel_muted(guild_id, channel_id, parent_id, now)
    }
}

// ---------------------------------------------------------------------------
// Mentions
// ---------------------------------------------------------------------------

/// Was I mentioned? User mention, a role I have, or @everyone/@here (unless
/// suppressed in that server's notification settings).
pub fn mentions_me(
    m: &Message,
    my_id: &str,
    my_roles: &[String],
    settings: Option<&GuildSettings>,
) -> bool {
    let (supp_everyone, supp_roles) =
        settings.map_or((false, false), |s| (s.suppress_everyone, s.suppress_roles));
    if m.author.id == my_id {
        return false;
    }
    if m.mention_everyone && !supp_everyone {
        return true;
    }
    if m.mentions.iter().any(|id| id == my_id) {
        return true;
    }
    !supp_roles && m.mention_roles.iter().any(|r| my_roles.contains(r))
}

/// Inputs for [`should_notify`].
pub struct NotifyCheck<'a> {
    pub guild_id: &'a str,
    pub channel_id: &'a str,
    pub parent_id: Option<&'a str>,
    pub is_dm: bool,
    pub mine: bool,
    pub mentioned: bool,
    /// Channel is open and the window has focus.
    pub viewing: bool,
    /// Server's default_message_notifications (0 all, 1 mentions).
    pub guild_default: u8,
    /// Bot accounts have no notification settings; only notify on mentions/DMs.
    pub is_bot: bool,
    pub now: &'a str,
}

/// Should this message pop a desktop notification? (DM's
/// `NotificationManager::IsNotificationWorthy`.)
pub fn should_notify(mutes: &Mutes, c: &NotifyCheck) -> bool {
    if c.mine || c.viewing {
        return false;
    }
    let settings = mutes.get(c.guild_id);
    let mut lvl = if c.is_dm {
        NOTIFY_ALL
    } else if c.is_bot {
        NOTIFY_MENTIONS
    } else {
        c.guild_default
    };
    if let Some(s) = settings {
        if s.mute.active(c.now) {
            return false;
        }
        if s.level != NOTIFY_INHERIT && !c.is_dm {
            lvl = s.level;
        }
        for id in std::iter::once(c.channel_id).chain(c.parent_id) {
            if let Some(o) = s.overrides.get(id) {
                if o.mute.active(c.now) {
                    return false;
                }
                if o.level != NOTIFY_INHERIT && id == c.channel_id {
                    lvl = o.level;
                }
            }
        }
    }
    match lvl {
        NOTIFY_ALL => true,
        NOTIFY_NOTHING => false,
        _ => c.mentioned,
    }
}

// ---------------------------------------------------------------------------
// Inbox
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct InboxItem {
    pub guild_id: String,
    pub channel_id: String,
    pub message_id: String,
    /// "#general · Server" or "Direct message".
    pub place: String,
    pub author: String,
    pub preview: String,
    pub time: String,
    pub read: bool,
}

/// Recent mentions, newest first. Empty (and unused) when the Notifications
/// module is off.
#[derive(Debug, Default)]
pub struct Inbox {
    pub items: VecDeque<InboxItem>,
}

impl Inbox {
    pub fn push(&mut self, item: InboxItem) {
        if self.items.iter().any(|i| i.message_id == item.message_id) {
            return;
        }
        self.items.push_front(item);
        self.items.truncate(INBOX_MAX);
    }

    pub fn mark_channel_read(&mut self, channel_id: &str) {
        for i in self.items.iter_mut().filter(|i| i.channel_id == channel_id) {
            i.read = true;
        }
    }

    pub fn unread(&self) -> usize {
        self.items.iter().filter(|i| !i.read).count()
    }
}

/// Shorten a message for a notification / inbox line.
pub fn preview(content: &str, max: usize) -> String {
    let flat = content.replace('\n', " ");
    if flat.chars().count() > max {
        flat.chars().take(max).collect::<String>() + "…"
    } else if flat.is_empty() {
        "(attachment)".into()
    } else {
        flat
    }
}

// ---------------------------------------------------------------------------
// Time / desktop
// ---------------------------------------------------------------------------

/// Current UTC time as "YYYY-MM-DDTHH:MM:SS", for comparing mute end times.
pub fn now_iso() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    iso_from_unix(secs)
}

pub fn iso_from_unix(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Howard Hinnant's civil_from_days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!(
        "{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

/// Show a desktop notification. Blocking (D-Bus on Linux), so the backend
/// runs it on a blocking thread. Errors (no notification daemon) are ignored.
pub fn show_desktop(title: &str, body: &str) {
    let _ = notify_rust::Notification::new()
        .appname("RustCord")
        .summary(title)
        .body(body)
        .show();
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn msg(v: Value) -> Message {
        let mut base = json!({
            "id": "10", "channel_id": "c1", "content": "hi",
            "author": { "id": "2", "username": "sam" },
        });
        base.as_object_mut()
            .unwrap()
            .extend(v.as_object().unwrap().clone());
        Message::from_json(&base)
    }

    fn settings(v: Value) -> GuildSettings {
        GuildSettings::from_json(&v)
    }

    #[test]
    fn user_mention() {
        let m = msg(json!({ "mentions": [{ "id": "1", "username": "me" }] }));
        assert!(mentions_me(&m, "1", &[], None));
        assert!(!mentions_me(&m, "3", &[], None));
    }

    #[test]
    fn everyone_and_suppress() {
        let m = msg(json!({ "mention_everyone": true }));
        assert!(mentions_me(&m, "1", &[], None));
        let s = settings(json!({ "guild_id": "g", "suppress_everyone": true }));
        assert!(!mentions_me(&m, "1", &[], Some(&s)));
    }

    #[test]
    fn role_mention_and_suppress() {
        let m = msg(json!({ "mention_roles": ["r1"] }));
        let roles = vec!["r1".to_string()];
        assert!(mentions_me(&m, "1", &roles, None));
        assert!(!mentions_me(&m, "1", &[], None));
        let s = settings(json!({ "guild_id": "g", "suppress_roles": true }));
        assert!(!mentions_me(&m, "1", &roles, Some(&s)));
    }

    #[test]
    fn own_message_never_mentions() {
        let m = msg(json!({ "mention_everyone": true }));
        assert!(!mentions_me(&m, "2", &[], None));
    }

    #[test]
    fn mute_times() {
        let now = "2026-11-09T12:00:00";
        let forever = Mute::from_json(&json!({ "muted": true, "mute_config": null }));
        assert!(forever.active(now));
        let window = Mute::from_json(&json!({
            "muted": true,
            "mute_config": { "end_time": "2020-01-01T00:00:00Z", "selected_time_window": -1 }
        }));
        assert!(window.active(now));
        let expired = Mute::from_json(&json!({
            "muted": true,
            "mute_config": { "end_time": "2026-11-09T11:59:59.000000+00:00", "selected_time_window": 3600 }
        }));
        assert!(!expired.active(now));
        let later = Mute::from_json(&json!({
            "muted": true,
            "mute_config": { "end_time": "2026-11-09T13:00:00.000000+00:00", "selected_time_window": 3600 }
        }));
        assert!(later.active(now));
        assert!(!Mute::default().active(now));
    }

    #[test]
    fn channel_and_category_mutes() {
        let mut mutes = Mutes::default();
        mutes.replace_all(GuildSettings::parse_ready(&json!({
            "entries": [{
                "guild_id": "g",
                "channel_overrides": [
                    { "channel_id": "c1", "muted": true },
                    { "channel_id": "cat", "muted": true },
                ]
            }, { "guild_id": null, "muted": true }],
            "version": 1
        })));
        let now = "2026-11-09T12:00:00";
        assert!(mutes.channel_muted("g", "c1", None, now));
        assert!(mutes.channel_muted("g", "c2", Some("cat"), now));
        assert!(!mutes.channel_muted("g", "c3", None, now));
        assert!(!mutes.guild_muted("g", now));
        assert!(mutes.guild_muted(DM_GUILD_ID, now));
    }

    fn check<'a>(guild_id: &'a str, channel_id: &'a str) -> NotifyCheck<'a> {
        NotifyCheck {
            guild_id,
            channel_id,
            parent_id: None,
            is_dm: false,
            mine: false,
            mentioned: false,
            viewing: false,
            guild_default: NOTIFY_MENTIONS,
            is_bot: false,
            now: "2026-11-09T12:00:00",
        }
    }

    #[test]
    fn notify_levels() {
        let mut mutes = Mutes::default();
        // No settings: server default (mentions only).
        assert!(!should_notify(&mutes, &check("g", "c")));
        assert!(should_notify(
            &mutes,
            &NotifyCheck {
                mentioned: true,
                ..check("g", "c")
            }
        ));
        // DMs always notify, unless viewing.
        let dm = NotifyCheck {
            is_dm: true,
            ..check(DM_GUILD_ID, "d")
        };
        assert!(should_notify(&mutes, &dm));
        assert!(!should_notify(
            &mutes,
            &NotifyCheck {
                viewing: true,
                ..dm
            }
        ));
        // Server set to all messages, one channel set to nothing.
        mutes.update(settings(json!({
            "guild_id": "g", "message_notifications": 0,
            "channel_overrides": [{ "channel_id": "quiet", "message_notifications": 2 }]
        })));
        assert!(should_notify(&mutes, &check("g", "c")));
        assert!(!should_notify(
            &mutes,
            &NotifyCheck {
                mentioned: true,
                ..check("g", "quiet")
            }
        ));
        // Muted server: nothing, even mentions.
        mutes.update(settings(json!({ "guild_id": "g", "muted": true })));
        assert!(!should_notify(
            &mutes,
            &NotifyCheck {
                mentioned: true,
                ..check("g", "c")
            }
        ));
    }

    #[test]
    fn bots_only_mentions() {
        let mutes = Mutes::default();
        let c = NotifyCheck {
            is_bot: true,
            guild_default: NOTIFY_ALL,
            ..check("g", "c")
        };
        assert!(!should_notify(&mutes, &c));
    }

    #[test]
    fn iso_time() {
        assert_eq!(iso_from_unix(0), "1970-01-01T00:00:00");
        assert_eq!(iso_from_unix(1_762_689_600), "2025-11-09T12:00:00");
        assert_eq!(iso_from_unix(951_782_400), "2000-02-29T00:00:00");
    }

    #[test]
    fn inbox_dedup_and_cap() {
        let mut inbox = Inbox::default();
        for i in 0..60 {
            let item = InboxItem {
                guild_id: "g".into(),
                channel_id: "c".into(),
                message_id: i.to_string(),
                place: String::new(),
                author: String::new(),
                preview: String::new(),
                time: String::new(),
                read: false,
            };
            inbox.push(item.clone());
            inbox.push(item);
        }
        assert_eq!(inbox.items.len(), INBOX_MAX);
        assert_eq!(inbox.items[0].message_id, "59");
        inbox.mark_channel_read("c");
        assert_eq!(inbox.unread(), 0);
    }
}
