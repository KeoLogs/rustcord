//! Server Settings → Audit Log: `GET /guilds/{g}/audit-logs` with user and
//! action filters, entries turned into readable sentences ("Leo banned
//! Sam"), local times via `timefmt`, and the list of changes on click.
//!
//! An answer is `{ audit_log_entries: [{ id, user_id, target_id,
//! action_type, changes: [{ key, old_value, new_value }], options, reason }],
//! users, webhooks, integrations, threads, guild_scheduled_events,
//! auto_moderation_rules, application_commands }`. Entries have no time
//! field: it comes from the entry's snowflake id. Paging: `before=<oldest
//! id>`, 50 per page.
//!
//! Action types follow Discord's documented `AuditLogEvent` table; unknown
//! ones still show as "did something (type n)".

use std::collections::{HashMap, HashSet};

use eframe::egui::{self, RichText, Sense, vec2};
use serde_json::Value;

use crate::guild_admin::{self as ga, AdminAction, AdminCtx, AdminOutcome, ImageUris, Load};
use crate::members::Role;
use crate::permissions_table as pt;
use crate::timefmt;
use crate::types::User;

const PAGE: usize = 50;

/// What the log is filtered by.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuditFilter {
    pub user_id: Option<String>,
    pub action_type: Option<u16>,
}

/// Every documented action type with the label for the filter menu.
pub const ACTIONS: &[(u16, &str)] = &[
    (1, "Update Server"),
    (10, "Create Channel"),
    (11, "Update Channel"),
    (12, "Delete Channel"),
    (13, "Create Channel Permission"),
    (14, "Update Channel Permission"),
    (15, "Delete Channel Permission"),
    (20, "Kick Member"),
    (21, "Prune Members"),
    (22, "Ban Member"),
    (23, "Unban Member"),
    (24, "Update Member"),
    (25, "Update Member Roles"),
    (26, "Move Member"),
    (27, "Disconnect Member"),
    (28, "Add Bot"),
    (30, "Create Role"),
    (31, "Update Role"),
    (32, "Delete Role"),
    (40, "Create Invite"),
    (41, "Update Invite"),
    (42, "Delete Invite"),
    (50, "Create Webhook"),
    (51, "Update Webhook"),
    (52, "Delete Webhook"),
    (60, "Create Emoji"),
    (61, "Update Emoji"),
    (62, "Delete Emoji"),
    (72, "Delete Message"),
    (73, "Bulk Delete Messages"),
    (74, "Pin Message"),
    (75, "Unpin Message"),
    (80, "Create Integration"),
    (81, "Update Integration"),
    (82, "Delete Integration"),
    (83, "Start Stage"),
    (84, "Update Stage"),
    (85, "End Stage"),
    (90, "Create Sticker"),
    (91, "Update Sticker"),
    (92, "Delete Sticker"),
    (100, "Create Event"),
    (101, "Update Event"),
    (102, "Cancel Event"),
    (110, "Create Thread"),
    (111, "Update Thread"),
    (112, "Delete Thread"),
    (121, "Update App Command Permissions"),
    (130, "Create Soundboard Sound"),
    (131, "Update Soundboard Sound"),
    (132, "Delete Soundboard Sound"),
    (140, "Create AutoMod Rule"),
    (141, "Update AutoMod Rule"),
    (142, "Delete AutoMod Rule"),
    (143, "AutoMod Block Message"),
    (144, "AutoMod Flag Message"),
    (145, "AutoMod Timeout Member"),
    (150, "Creator Monetization Request"),
    (151, "Creator Monetization Terms Accepted"),
    (163, "Create Onboarding Question"),
    (164, "Update Onboarding Question"),
    (165, "Delete Onboarding Question"),
    (166, "Create Onboarding"),
    (167, "Update Onboarding"),
    (190, "Create Server Guide"),
    (191, "Update Server Guide"),
];

pub fn action_label(t: u16) -> String {
    ACTIONS
        .iter()
        .find(|(n, _)| *n == t)
        .map(|(_, l)| l.to_string())
        .unwrap_or_else(|| format!("Action {t}"))
}

/// Colour group for an entry's icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Create,
    Update,
    Delete,
}

pub fn kind_of(t: u16) -> Kind {
    match t {
        12 | 15 | 20 | 21 | 22 | 27 | 32 | 42 | 52 | 62 | 72 | 73 | 75 | 82 | 85 | 92 | 102
        | 112 | 132 | 142 | 143 | 145 | 165 => Kind::Delete,
        10 | 13 | 23 | 28 | 30 | 40 | 50 | 60 | 74 | 80 | 83 | 90 | 100 | 110 | 130 | 140 | 163
        | 166 | 190 => Kind::Create,
        _ => Kind::Update,
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditChange {
    pub key: String,
    pub old: Value,
    pub new: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AuditEntry {
    pub id: String,
    pub user_id: Option<String>,
    pub target_id: Option<String>,
    pub action_type: u16,
    pub changes: Vec<AuditChange>,
    pub options: Value,
    pub reason: Option<String>,
}

fn opt_str(v: &Value) -> Option<String> {
    match v {
        Value::String(s) if !s.is_empty() => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

impl AuditEntry {
    pub fn from_json(v: &Value) -> Self {
        Self {
            id: opt_str(&v["id"]).unwrap_or_default(),
            user_id: opt_str(&v["user_id"]),
            target_id: opt_str(&v["target_id"]),
            action_type: v["action_type"].as_u64().unwrap_or(0) as u16,
            changes: v["changes"]
                .as_array()
                .map(|a| {
                    a.iter()
                        .map(|c| AuditChange {
                            key: c["key"].as_str().unwrap_or_default().to_string(),
                            old: c.get("old_value").cloned().unwrap_or(Value::Null),
                            new: c.get("new_value").cloned().unwrap_or(Value::Null),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            options: v.get("options").cloned().unwrap_or(Value::Null),
            reason: v["reason"]
                .as_str()
                .filter(|r| !r.trim().is_empty())
                .map(str::to_string),
        }
    }

    pub fn change(&self, key: &str) -> Option<&AuditChange> {
        self.changes.iter().find(|c| c.key == key)
    }

    /// A string field's new value, else its old one (names of deleted
    /// things are only in `old_value`).
    fn changed_str(&self, key: &str) -> Option<String> {
        self.change(key)
            .and_then(|c| opt_str(&c.new).or_else(|| opt_str(&c.old)))
    }

    /// Unix seconds (from the snowflake id).
    pub fn time(&self) -> i64 {
        ga::snowflake_time(&self.id)
    }
}

/// One page of the log, with the lookups that came with it.
#[derive(Debug, Clone, Default)]
pub struct AuditPage {
    pub entries: Vec<AuditEntry>,
    pub users: HashMap<String, User>,
    /// Webhooks, integrations, threads, events, AutoMod rules, commands:
    /// id → name.
    pub names: HashMap<String, String>,
}

impl AuditPage {
    pub fn from_json(v: &Value) -> Self {
        let entries = v["audit_log_entries"]
            .as_array()
            .map(|a| a.iter().map(AuditEntry::from_json).collect())
            .unwrap_or_default();
        let users = v["users"]
            .as_array()
            .map(|a| {
                a.iter()
                    .map(User::from_json)
                    .filter(|u| !u.id.is_empty())
                    .map(|u| (u.id.clone(), u))
                    .collect()
            })
            .unwrap_or_default();
        let mut names = HashMap::new();
        for key in [
            "webhooks",
            "integrations",
            "threads",
            "guild_scheduled_events",
            "auto_moderation_rules",
            "application_commands",
        ] {
            for o in v[key].as_array().into_iter().flatten() {
                if let (Some(id), Some(name)) = (opt_str(&o["id"]), o["name"].as_str()) {
                    names.insert(id, name.to_string());
                }
            }
        }
        Self {
            entries,
            users,
            names,
        }
    }

    fn merge(&mut self, other: AuditPage) {
        self.entries.extend(other.entries);
        self.users.extend(other.users);
        self.names.extend(other.names);
    }
}

/// Name lookups for descriptions.
pub struct Lookup<'a> {
    pub users: &'a HashMap<String, User>,
    pub names: &'a HashMap<String, String>,
    pub roles: &'a [Role],
    pub channel_name: Option<crate::guild_admin::NameFn<'a>>,
}

impl Lookup<'_> {
    fn user(&self, id: Option<&str>) -> String {
        match id {
            Some(id) => self
                .users
                .get(id)
                .map(|u| u.name.clone())
                .unwrap_or_else(|| format!("<@{id}>")),
            None => "Someone".into(),
        }
    }

    fn role(&self, id: &str) -> Option<String> {
        self.roles
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.name.clone())
    }

    fn channel(&self, id: &str) -> Option<String> {
        self.channel_name.and_then(|f| f(id))
    }
}

fn plural(n: u64, one: &str) -> String {
    if n == 1 {
        format!("1 {one}")
    } else {
        format!("{n} {one}s")
    }
}

fn count(o: &Value, key: &str) -> u64 {
    opt_str(&o[key]).and_then(|s| s.parse().ok()).unwrap_or(1)
}

/// "Leo banned Sam", "Leo created the role Mods", …
pub fn describe(e: &AuditEntry, l: &Lookup) -> String {
    let who = l.user(e.user_id.as_deref());
    let target = e.target_id.as_deref().unwrap_or_default();
    let target_user = || l.user(e.target_id.as_deref());
    let name = |fallback: &str| {
        e.changed_str("name")
            .or_else(|| l.names.get(target).cloned())
            .unwrap_or_else(|| fallback.to_string())
    };
    let channel = |id: &str| {
        l.channel(id)
            .or_else(|| (id == target).then(|| e.changed_str("name")).flatten())
            .map(|n| format!("#{n}"))
            .unwrap_or_else(|| "a channel".into())
    };
    let opt_channel = || channel(e.options["channel_id"].as_str().unwrap_or_default());
    let role = || {
        l.role(target)
            .or_else(|| e.changed_str("name"))
            .unwrap_or_else(|| "a role".into())
    };
    let o = &e.options;
    let what = match e.action_type {
        1 => "updated the server".to_string(),
        10 => format!("created {}", channel(target)),
        11 => format!("updated {}", channel(target)),
        12 => format!("deleted {}", channel(target)),
        13..=15 => {
            let verb = match e.action_type {
                13 => "created",
                14 => "updated",
                _ => "removed",
            };
            let id = o["id"].as_str().unwrap_or_default();
            let subject = if o["type"].as_str() == Some("1") || o["type"].as_u64() == Some(1) {
                l.user(Some(id))
            } else {
                o["role_name"]
                    .as_str()
                    .map(str::to_string)
                    .or_else(|| l.role(id))
                    .unwrap_or_else(|| "a role".into())
            };
            format!(
                "{verb} permission overrides for {subject} in {}",
                channel(target)
            )
        }
        20 => format!("kicked {}", target_user()),
        21 => format!(
            "pruned {} inactive for {}",
            plural(count(o, "members_removed"), "member"),
            plural(count(o, "delete_member_days"), "day")
        ),
        22 => format!("banned {}", target_user()),
        23 => format!("unbanned {}", target_user()),
        24 => member_update(e, &who, &target_user()),
        25 => {
            let names = |key: &str| -> Vec<String> {
                e.change(key)
                    .and_then(|c| c.new.as_array())
                    .map(|a| {
                        a.iter()
                            .filter_map(|r| r["name"].as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default()
            };
            let (added, removed) = (names("$add"), names("$remove"));
            let t = target_user();
            match (added.is_empty(), removed.is_empty()) {
                (false, true) => format!(
                    "added the {} {} to {t}",
                    if added.len() == 1 { "role" } else { "roles" },
                    added.join(", ")
                ),
                (true, false) => format!(
                    "removed the {} {} from {t}",
                    if removed.len() == 1 { "role" } else { "roles" },
                    removed.join(", ")
                ),
                _ => format!("updated the roles of {t}"),
            }
        }
        26 => format!(
            "moved {} to {}",
            plural(count(o, "count"), "member"),
            opt_channel()
        ),
        27 => format!(
            "disconnected {} from voice",
            plural(count(o, "count"), "member")
        ),
        28 => format!("added the bot {}", target_user()),
        30 => format!("created the role {}", role()),
        31 => format!("updated the role {}", role()),
        32 => format!("deleted the role {}", role()),
        40..=42 => {
            let code = e.changed_str("code").unwrap_or_default();
            let verb = ["created", "updated", "deleted"][(e.action_type - 40) as usize];
            format!("{verb} the invite {code}").trim_end().to_string()
        }
        50..=52 => {
            let verb = ["created", "updated", "deleted"][(e.action_type - 50) as usize];
            format!("{verb} the webhook {}", name("a webhook"))
        }
        60..=62 => {
            let verb = ["created", "updated", "deleted"][(e.action_type - 60) as usize];
            format!("{verb} the emoji :{}:", name("emoji"))
        }
        72 => format!(
            "deleted {} by {} in {}",
            plural(count(o, "count"), "message"),
            target_user(),
            opt_channel()
        ),
        73 => format!(
            "bulk deleted {} in {}",
            plural(count(o, "count"), "message"),
            channel(target)
        ),
        74 => format!("pinned a message by {} in {}", target_user(), opt_channel()),
        75 => format!(
            "unpinned a message by {} in {}",
            target_user(),
            opt_channel()
        ),
        80..=82 => {
            let verb = ["added", "updated", "removed"][(e.action_type - 80) as usize];
            format!("{verb} the integration {}", name("an integration"))
        }
        83..=85 => {
            let verb = ["started", "updated", "ended"][(e.action_type - 83) as usize];
            let topic = e.changed_str("topic").unwrap_or_else(|| "a stage".into());
            format!("{verb} the stage {topic}")
        }
        90..=92 => {
            let verb = ["created", "updated", "deleted"][(e.action_type - 90) as usize];
            format!("{verb} the sticker {}", name("a sticker"))
        }
        100..=102 => {
            let verb = ["created", "updated", "cancelled"][(e.action_type - 100) as usize];
            format!("{verb} the event {}", name("an event"))
        }
        110..=112 => {
            let verb = ["created", "updated", "deleted"][(e.action_type - 110) as usize];
            format!("{verb} the thread {}", name("a thread"))
        }
        121 => "updated permissions for an app command".to_string(),
        130..=132 => {
            let verb = ["added", "updated", "deleted"][(e.action_type - 130) as usize];
            format!("{verb} the soundboard sound {}", name("a sound"))
        }
        140..=142 => {
            let verb = ["created", "updated", "deleted"][(e.action_type - 140) as usize];
            format!("{verb} the AutoMod rule {}", name("a rule"))
        }
        143..=145 => {
            let rule = o["auto_moderation_rule_name"]
                .as_str()
                .map(|r| format!(" (rule: {r})"))
                .unwrap_or_default();
            let ch = opt_channel();
            return match e.action_type {
                143 => format!(
                    "AutoMod blocked a message from {} in {ch}{rule}",
                    target_user()
                ),
                144 => format!(
                    "AutoMod flagged a message from {} in {ch}{rule}",
                    target_user()
                ),
                _ => format!("AutoMod timed out {}{rule}", target_user()),
            };
        }
        150 => "requested creator monetization".to_string(),
        151 => "accepted the creator monetization terms".to_string(),
        163..=167 => "updated onboarding".to_string(),
        190 | 191 => "updated the server guide".to_string(),
        t => format!("did something (type {t})"),
    };
    format!("{who} {what}")
}

/// Action 24 (member update): nickname, timeout, server mute / deafen.
fn member_update(e: &AuditEntry, who: &str, target: &str) -> String {
    let own = e.user_id.is_some() && e.user_id == e.target_id;
    if let Some(c) = e.change("communication_disabled_until") {
        return match c.new.as_str() {
            Some(until) => format!(
                "timed out {target} until {}",
                timefmt::full(timefmt::parse_iso(until))
            ),
            None => format!("removed the timeout from {target}"),
        };
    }
    if let Some(c) = e.change("nick") {
        let _ = who;
        return match (c.new.as_str(), own) {
            (Some(n), true) => format!("changed their nickname to {n}"),
            (Some(n), false) => format!("changed the nickname of {target} to {n}"),
            (None, true) => "removed their nickname".to_string(),
            (None, false) => format!("removed the nickname of {target}"),
        };
    }
    if let Some(c) = e.change("mute") {
        return if c.new.as_bool() == Some(true) {
            format!("server muted {target}")
        } else {
            format!("server unmuted {target}")
        };
    }
    if let Some(c) = e.change("deaf") {
        return if c.new.as_bool() == Some(true) {
            format!("server deafened {target}")
        } else {
            format!("server undeafened {target}")
        };
    }
    format!("updated {target}")
}

fn show_value(key: &str, v: &Value) -> String {
    match v {
        Value::String(s) if key.ends_with("_until") || key == "communication_disabled_until" => {
            timefmt::full(timefmt::parse_iso(s))
        }
        Value::String(s) => format!("\"{s}\""),
        Value::Number(n) if key == "color" => format!("#{:06x}", n.as_u64().unwrap_or(0)),
        Value::Number(n) => n.to_string(),
        Value::Bool(b) => if *b { "on" } else { "off" }.to_string(),
        Value::Array(a) => {
            let names: Vec<String> = a
                .iter()
                .filter_map(|x| x["name"].as_str().map(str::to_string))
                .collect();
            if names.is_empty() {
                format!("{} items", a.len())
            } else {
                names.join(", ")
            }
        }
        Value::Null => "nothing".to_string(),
        Value::Object(_) => "…".to_string(),
    }
}

/// One line per change, e.g. "Changed name from "a" to "b"".
pub fn describe_change(c: &AuditChange) -> String {
    let key = c.key.replace('_', " ");
    match c.key.as_str() {
        "$add" => format!("Added roles: {}", show_value(&c.key, &c.new)),
        "$remove" => format!("Removed roles: {}", show_value(&c.key, &c.new)),
        "permissions" | "allow" | "deny" => {
            let old = crate::perms::parse_bits(&c.old);
            let new = crate::perms::parse_bits(&c.new);
            let granted = pt::names(new & !old);
            let revoked = pt::names(old & !new);
            let mut parts = Vec::new();
            let label = if c.key == "deny" { "Denied" } else { "Granted" };
            if !granted.is_empty() {
                parts.push(format!("{label}: {}", granted.join(", ")));
            }
            if !revoked.is_empty() {
                let label = if c.key == "deny" {
                    "No longer denied"
                } else {
                    "Revoked"
                };
                parts.push(format!("{label}: {}", revoked.join(", ")));
            }
            if parts.is_empty() {
                format!("Changed {key}")
            } else {
                parts.join("; ")
            }
        }
        _ => match (&c.old, &c.new) {
            (Value::Null, new) => format!("Set {key} to {}", show_value(&c.key, new)),
            (old, Value::Null) => format!("Removed {key} ({})", show_value(&c.key, old)),
            (Value::Bool(_), Value::Bool(b)) => {
                format!("Turned {} {key}", if *b { "on" } else { "off" })
            }
            (old, new) => format!(
                "Changed {key} from {} to {}",
                show_value(&c.key, old),
                show_value(&c.key, new)
            ),
        },
    }
}

/// Server Settings → Audit Log.
#[derive(Default)]
pub struct AuditLogPage {
    log: Load<AuditPage>,
    filter: AuditFilter,
    more: bool,
    loading_more: bool,
    expanded: HashSet<String>,
    /// Users seen in any answer (for the filter menu).
    seen_users: HashMap<String, User>,
    uris: ImageUris,
}

impl AuditLogPage {
    pub fn forget_images(&mut self, ctx: &egui::Context) {
        self.uris.forget(ctx);
    }

    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        let AdminAction::LoadAuditLog { filter, before, .. } = action else {
            return;
        };
        if *filter != self.filter {
            return; // An answer for an old filter.
        }
        self.loading_more = false;
        match result {
            Ok(AdminOutcome::Json(v)) => {
                let page = AuditPage::from_json(v);
                self.more = page.entries.len() >= PAGE;
                self.seen_users
                    .extend(page.users.iter().map(|(k, u)| (k.clone(), u.clone())));
                match (&mut self.log, before) {
                    (Load::Loaded(log), Some(_)) => log.merge(page),
                    _ => self.log = Load::Loaded(page),
                }
            }
            Ok(_) => {}
            Err(e) => self.log = Load::Failed(e.clone()),
        }
    }

    fn reload(&mut self, gid: &str) -> AdminAction {
        self.log = Load::Loading;
        self.expanded.clear();
        AdminAction::LoadAuditLog {
            guild_id: gid.to_string(),
            filter: self.filter.clone(),
            before: None,
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        if self.log.is_idle() {
            return Some(self.reload(&gid));
        }
        let mut action = None;
        ui.heading("Audit Log");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Filter by user").small().color(pal.muted));
            let current = match &self.filter.user_id {
                None => "All".to_string(),
                Some(id) => self
                    .seen_users
                    .get(id)
                    .map(|u| u.name.clone())
                    .unwrap_or_else(|| id.clone()),
            };
            let mut users: Vec<User> = self.seen_users.values().cloned().collect();
            users.sort_by_key(|u| u.name.to_lowercase());
            let mut new_user = self.filter.user_id.clone();
            egui::ComboBox::from_id_salt("rc_audit_user")
                .selected_text(current)
                .width(160.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut new_user, None, "All");
                    for u in &users {
                        ui.selectable_value(&mut new_user, Some(u.id.clone()), &u.name);
                    }
                });
            ui.add_space(12.0);
            ui.label(RichText::new("Filter by action").small().color(pal.muted));
            let mut new_action = self.filter.action_type;
            egui::ComboBox::from_id_salt("rc_audit_action")
                .selected_text(
                    self.filter
                        .action_type
                        .map(action_label)
                        .unwrap_or_else(|| "All".into()),
                )
                .width(200.0)
                .height(360.0)
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut new_action, None, "All");
                    for (t, label) in ACTIONS {
                        ui.selectable_value(&mut new_action, Some(*t), *label);
                    }
                });
            if new_user != self.filter.user_id || new_action != self.filter.action_type {
                self.filter = AuditFilter {
                    user_id: new_user,
                    action_type: new_action,
                };
                action = Some(self.reload(&gid));
            }
        });
        ui.add_space(8.0);
        let log = match &self.log {
            Load::Loading | Load::Idle => {
                ui.spinner();
                return action;
            }
            Load::Failed(e) => {
                ga::error_line(ui, pal, e);
                if ui.button("Try again").clicked() {
                    return Some(self.reload(&gid));
                }
                return action;
            }
            Load::Loaded(log) => log.clone(),
        };
        if log.entries.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("No logs yet.").color(pal.muted));
            return action;
        }
        let lookup = Lookup {
            users: &log.users,
            names: &log.names,
            roles: ctx.actor.roles,
            channel_name: ctx.channel_name,
        };
        egui::ScrollArea::vertical()
            .id_salt("rc_audit_list")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for e in &log.entries {
                    let open = self.expanded.contains(&e.id);
                    let resp = egui::Frame::new()
                        .fill(pal.sidebar)
                        .corner_radius(6)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                let col = match kind_of(e.action_type) {
                                    Kind::Create => pal.online,
                                    Kind::Update => pal.idle,
                                    Kind::Delete => pal.danger,
                                };
                                let (r, _) =
                                    ui.allocate_exact_size(vec2(10.0, 10.0), Sense::hover());
                                ui.painter().circle_filled(r.center(), 5.0, col);
                                let actor = e.user_id.as_deref().and_then(|id| log.users.get(id));
                                if let Some(u) = actor {
                                    ga::avatar(
                                        ui,
                                        24.0,
                                        &u.id,
                                        &u.name,
                                        &u.avatar_url,
                                        ctx,
                                        &mut self.uris,
                                    );
                                }
                                ui.vertical(|ui| {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(describe(e, &lookup)).color(pal.text),
                                        )
                                        .wrap(),
                                    );
                                    ui.label(
                                        RichText::new(timefmt::full(e.time()))
                                            .small()
                                            .color(pal.muted),
                                    );
                                });
                            });
                            if let Some(r) = &e.reason {
                                ui.label(
                                    RichText::new(format!("Reason: {r}"))
                                        .small()
                                        .color(pal.muted),
                                );
                            }
                            if open {
                                ui.add_space(4.0);
                                if e.changes.is_empty() {
                                    ui.label(RichText::new("No details.").small().color(pal.muted));
                                }
                                for c in &e.changes {
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(describe_change(c))
                                                .small()
                                                .color(pal.text),
                                        )
                                        .wrap(),
                                    );
                                }
                            }
                        })
                        .response
                        .interact(Sense::click());
                    if resp.clicked() {
                        if open {
                            self.expanded.remove(&e.id);
                        } else {
                            self.expanded.insert(e.id.clone());
                        }
                    }
                    ui.add_space(4.0);
                }
                if self.more
                    && ui
                        .add_enabled(!self.loading_more, egui::Button::new("Load more"))
                        .clicked()
                {
                    self.loading_more = true;
                    action = Some(AdminAction::LoadAuditLog {
                        guild_id: gid.clone(),
                        filter: self.filter.clone(),
                        before: log.entries.last().map(|e| e.id.clone()),
                    });
                }
            });
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn page() -> AuditPage {
        AuditPage::from_json(&json!({
            "users": [
                { "id": "1", "username": "leo", "global_name": "Leo" },
                { "id": "2", "username": "sam", "global_name": "Sam" }
            ],
            "webhooks": [{ "id": "w", "name": "GitHub" }],
            "audit_log_entries": []
        }))
    }

    fn entry(v: Value) -> AuditEntry {
        AuditEntry::from_json(&v)
    }

    fn roles() -> Vec<Role> {
        vec![Role::from_json(
            &json!({ "id": "r", "name": "Mods", "position": 1 }),
        )]
    }

    fn d(v: Value) -> String {
        let p = page();
        let roles = roles();
        let chan = |id: &str| (id == "c").then(|| "general".to_string());
        let l = Lookup {
            users: &p.users,
            names: &p.names,
            roles: &roles,
            channel_name: Some(&chan),
        };
        describe(&entry(v), &l)
    }

    #[test]
    fn descriptions() {
        assert_eq!(
            d(json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 22 })),
            "Leo banned Sam"
        );
        assert_eq!(
            d(json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 20 })),
            "Leo kicked Sam"
        );
        assert_eq!(
            d(json!({ "id": "9", "user_id": "1", "target_id": "r", "action_type": 30 })),
            "Leo created the role Mods"
        );
        // Deleted role: its name is only in the changes.
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "gone", "action_type": 32,
                      "changes": [{ "key": "name", "old_value": "Old" }] })
            ),
            "Leo deleted the role Old"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 25,
                      "changes": [{ "key": "$add", "new_value": [{ "id": "r", "name": "Mods" }] }] })
            ),
            "Leo added the role Mods to Sam"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 25,
                      "changes": [{ "key": "$remove", "new_value": [{ "name": "A" }, { "name": "B" }] }] })
            ),
            "Leo removed the roles A, B from Sam"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 24,
                      "changes": [{ "key": "communication_disabled_until", "old_value": "2030-01-01T00:00:00+00:00" }] })
            ),
            "Leo removed the timeout from Sam"
        );
        assert!(
            d(json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 24,
                      "changes": [{ "key": "communication_disabled_until", "new_value": "2030-01-01T00:00:00+00:00" }] }))
            .starts_with("Leo timed out Sam until ")
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "2", "target_id": "2", "action_type": 24,
                      "changes": [{ "key": "nick", "new_value": "Sammy" }] })
            ),
            "Sam changed their nickname to Sammy"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "2", "action_type": 72,
                      "options": { "channel_id": "c", "count": "3" } })
            ),
            "Leo deleted 3 messages by Sam in #general"
        );
        assert_eq!(
            d(json!({ "id": "9", "user_id": "1", "target_id": "w", "action_type": 50 })),
            "Leo created the webhook GitHub"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "e", "action_type": 62,
                      "changes": [{ "key": "name", "old_value": "pog" }] })
            ),
            "Leo deleted the emoji :pog:"
        );
        assert_eq!(
            d(json!({ "id": "9", "user_id": "1", "action_type": 21,
                      "options": { "delete_member_days": "7", "members_removed": "1" } })),
            "Leo pruned 1 member inactive for 7 days"
        );
        assert_eq!(
            d(
                json!({ "id": "9", "user_id": "1", "target_id": "c", "action_type": 14,
                      "options": { "id": "r", "type": "0", "role_name": "Mods" } })
            ),
            "Leo updated permission overrides for Mods in #general"
        );
        assert_eq!(
            d(json!({ "id": "9", "user_id": "99", "action_type": 250 })),
            "<@99> did something (type 250)"
        );
        assert_eq!(
            d(json!({ "id": "9", "action_type": 1 })),
            "Someone updated the server"
        );
    }

    #[test]
    fn change_lines() {
        let c = |v: Value| describe_change(&entry(json!({ "changes": [v] })).changes[0]);
        assert_eq!(
            c(json!({ "key": "name", "old_value": "a", "new_value": "b" })),
            "Changed name from \"a\" to \"b\""
        );
        assert_eq!(
            c(json!({ "key": "color", "old_value": 0, "new_value": 0x3498db })),
            "Changed color from #000000 to #3498db"
        );
        assert_eq!(
            c(json!({ "key": "hoist", "old_value": false, "new_value": true })),
            "Turned on hoist"
        );
        assert_eq!(
            c(json!({ "key": "topic", "new_value": "hi" })),
            "Set topic to \"hi\""
        );
        assert_eq!(
            c(json!({ "key": "permissions", "old_value": "2", "new_value": "4" })),
            "Granted: Ban Members; Revoked: Kick, Approve, and Reject Members"
        );
    }

    #[test]
    fn paging_and_filters() {
        let mut p = AuditLogPage::default();
        let g = "g".to_string();
        let entries: Vec<Value> = (0..PAGE)
            .map(|i| json!({ "id": (1000 - i).to_string(), "action_type": 1 }))
            .collect();
        let first = AdminAction::LoadAuditLog {
            guild_id: g.clone(),
            filter: AuditFilter::default(),
            before: None,
        };
        p.on_result(
            &first,
            &Ok(AdminOutcome::Json(json!({ "audit_log_entries": entries, "users": [{ "id": "1", "username": "leo" }] }))),
        );
        assert!(p.more);
        assert_eq!(p.log.get().unwrap().entries.len(), PAGE);
        let next = AdminAction::LoadAuditLog {
            guild_id: g.clone(),
            filter: AuditFilter::default(),
            before: Some("951".into()),
        };
        p.on_result(
            &next,
            &Ok(AdminOutcome::Json(
                json!({ "audit_log_entries": [{ "id": "5", "action_type": 22 }] }),
            )),
        );
        assert!(!p.more);
        assert_eq!(p.log.get().unwrap().entries.len(), PAGE + 1);
        assert!(p.seen_users.contains_key("1"));
        // Answers for another filter are dropped.
        let stale = AdminAction::LoadAuditLog {
            guild_id: g,
            filter: AuditFilter {
                user_id: None,
                action_type: Some(22),
            },
            before: None,
        };
        p.on_result(
            &stale,
            &Ok(AdminOutcome::Json(json!({ "audit_log_entries": [] }))),
        );
        assert_eq!(p.log.get().unwrap().entries.len(), PAGE + 1);
        // Kinds and labels.
        assert_eq!(kind_of(22), Kind::Delete);
        assert_eq!(kind_of(30), Kind::Create);
        assert_eq!(kind_of(31), Kind::Update);
        assert_eq!(action_label(22), "Ban Member");
        assert_eq!(action_label(999), "Action 999");
    }
}
