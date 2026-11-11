//! Server settings → Audit Log: who changed what, newest first, with
//! filters by user and by action. Read-only; pages load on "Apply" and
//! "Load more" clicks only.

use eframe::egui::{self, RichText};
use serde_json::Value;

use super::{Cx, Kind, Method, Out, Request, n, s, ui as aui};
use crate::icons;
use crate::theme::Bold;

/// Entries per page (Discord's maximum is 100).
pub const PAGE: usize = 50;

/// Action types Discord documents, with how they read after a name.
pub const ACTIONS: &[(u64, &str)] = &[
    (1, "updated the server"),
    (10, "created channel"),
    (11, "updated channel"),
    (12, "deleted channel"),
    (13, "created channel permissions for"),
    (14, "updated channel permissions for"),
    (15, "deleted channel permissions for"),
    (20, "kicked"),
    (21, "pruned members"),
    (22, "banned"),
    (23, "unbanned"),
    (24, "updated member"),
    (25, "updated roles for"),
    (26, "moved a member to another voice channel"),
    (27, "disconnected a member from voice"),
    (28, "added the app"),
    (30, "created role"),
    (31, "updated role"),
    (32, "deleted role"),
    (40, "created invite"),
    (41, "updated invite"),
    (42, "deleted invite"),
    (50, "created webhook"),
    (51, "updated webhook"),
    (52, "deleted webhook"),
    (60, "created emoji"),
    (61, "updated emoji"),
    (62, "deleted emoji"),
    (72, "deleted messages by"),
    (73, "bulk deleted messages in"),
    (74, "pinned a message by"),
    (75, "unpinned a message by"),
    (80, "added integration"),
    (81, "updated integration"),
    (82, "removed integration"),
    (83, "started stage"),
    (84, "updated stage"),
    (85, "ended stage"),
    (90, "created sticker"),
    (91, "updated sticker"),
    (92, "deleted sticker"),
    (100, "created event"),
    (101, "updated event"),
    (102, "cancelled event"),
    (110, "created thread"),
    (111, "updated thread"),
    (112, "deleted thread"),
    (121, "updated command permissions for"),
    (140, "created AutoMod rule"),
    (141, "updated AutoMod rule"),
    (142, "deleted AutoMod rule"),
    (143, "AutoMod blocked a message by"),
    (144, "AutoMod flagged a message by"),
    (145, "AutoMod timed out"),
    (163, "created an onboarding question"),
    (164, "updated an onboarding question"),
    (165, "deleted an onboarding question"),
    (166, "created onboarding"),
    (167, "updated onboarding"),
];

pub fn action_text(kind: u64) -> String {
    ACTIONS
        .iter()
        .find(|(k, _)| *k == kind)
        .map_or_else(|| format!("did action {kind} on"), |(_, t)| t.to_string())
}

/// Created / deleted / anything else, for the row icon.
fn action_class(kind: u64) -> u8 {
    match kind {
        10 | 13 | 30 | 40 | 50 | 60 | 80 | 83 | 90 | 100 | 110 | 140 | 163 | 166 | 28 => 0,
        12 | 15 | 20 | 21 | 22 | 32 | 42 | 52 | 62 | 72 | 73 | 82 | 85 | 92 | 102 | 112 | 142
        | 165 => 2,
        _ => 1,
    }
}

/// One page of the log. Empty filters = everything; `before` continues
/// below the oldest entry shown.
pub fn load(guild_id: &str, user_id: &str, action: u64, before: Option<&str>) -> Request {
    let mut path = format!("/guilds/{guild_id}/audit-logs?limit={PAGE}");
    let user_id = user_id.trim();
    if !user_id.is_empty() && user_id.chars().all(|c| c.is_ascii_digit()) {
        path.push_str(&format!("&user_id={user_id}"));
    }
    if action != 0 {
        path.push_str(&format!("&action_type={action}"));
    }
    if let Some(b) = before.filter(|b| !b.is_empty()) {
        path.push_str(&format!("&before={b}"));
    }
    Request::new(
        guild_id,
        if before.is_some() {
            Kind::LoadAuditMore
        } else {
            Kind::LoadAudit
        },
        Method::Get,
        path,
    )
}

/// Unix seconds a snowflake id was made at.
pub fn snowflake_time(id: &str) -> i64 {
    id.parse::<u64>()
        .map_or(0, |v| ((v >> 22) + 1_420_070_400_000) as i64 / 1000)
}

/// A change value as short text.
pub fn show_value(v: &Value) -> String {
    let text = match v {
        Value::Null => "none".to_string(),
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    if text.chars().count() > 80 {
        format!("{}…", text.chars().take(80).collect::<String>())
    } else {
        text
    }
}

#[derive(Default)]
pub struct State {
    entries: Option<Vec<Value>>,
    /// Users named in the loaded pages (id, username, global_name only).
    users: Vec<Value>,
    filter_user: String,
    filter_action: u64,
    /// The filters the shown entries were loaded with.
    applied: (String, u64),
    expanded: Option<String>,
    more: bool,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, _key: &str, v: &Value) {
        if !matches!(kind, Kind::LoadAudit | Kind::LoadAuditMore) {
            return;
        }
        let page = super::list(&v["audit_log_entries"]);
        self.more = page.len() >= PAGE;
        if kind == Kind::LoadAudit {
            self.entries = Some(page);
            self.expanded = None;
        } else if let Some(list) = &mut self.entries {
            let room = super::LIST_CAP.saturating_sub(list.len());
            list.extend(page.into_iter().take(room));
            self.more &= list.len() < super::LIST_CAP;
        }
        // Only names are kept: the reply's webhooks (with their tokens)
        // and integrations are dropped here.
        for u in v["users"].as_array().into_iter().flatten() {
            let slim = serde_json::json!({
                "id": s(u, "id"), "username": s(u, "username"),
                "global_name": s(u, "global_name"),
            });
            if self.users.len() < super::LIST_CAP {
                super::upsert(&mut self.users, &slim);
            }
        }
    }

    fn user_name(&self, id: &str) -> String {
        self.users
            .iter()
            .find(|u| s(u, "id") == id)
            .map(|u| match s(u, "global_name") {
                "" => s(u, "username").to_string(),
                g => g.to_string(),
            })
            .unwrap_or_else(|| {
                if id.is_empty() {
                    "Someone".to_string()
                } else {
                    format!("User {id}")
                }
            })
    }

    /// What an entry acted on: a member, channel, role, or a name from
    /// its changes.
    fn target_name(&self, cx: &Cx, e: &Value) -> String {
        let id = s(e, "target_id");
        let kind = n(e, "action_type");
        // Server-wide actions have no target worth naming.
        if matches!(kind, 1 | 21 | 26 | 27 | 166 | 167) {
            return String::new();
        }
        if matches!(kind, 20..=28 | 72 | 74 | 75 | 143..=145)
            && self.users.iter().any(|u| s(u, "id") == id)
        {
            return self.user_name(id);
        }
        if let Some(c) = cx.guild.channels.iter().find(|c| c.id == id) {
            return super::channel_label(c.kind, &c.name);
        }
        if let Some(r) = cx.guild.roles.iter().find(|r| r.id == id) {
            return format!("@{}", r.name);
        }
        for c in e["changes"].as_array().into_iter().flatten() {
            if matches!(s(c, "key"), "name" | "code") {
                let v = c.get("new_value").or_else(|| c.get("old_value"));
                if let Some(text) = v.and_then(Value::as_str) {
                    return text.to_string();
                }
            }
        }
        id.to_string()
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        ui.horizontal_wrapped(|ui| {
            ui.vertical(|ui| {
                ui.label(pal.section_label("Filter by user"));
                ui.set_max_width(220.0);
                let mut users = vec![(String::new(), "All users".to_string())];
                users.extend(
                    self.users
                        .iter()
                        .map(|u| (s(u, "id").to_string(), self.user_name(s(u, "id")))),
                );
                aui::select(ui, pal, "audit_user", &mut self.filter_user, &users);
            });
            ui.vertical(|ui| {
                ui.label(pal.section_label("Filter by action"));
                ui.set_max_width(260.0);
                let mut actions = vec![(0, "All actions".to_string())];
                actions.extend(ACTIONS.iter().map(|(k, t)| {
                    let mut label = t.to_string();
                    if let Some(first) = label.get_mut(0..1) {
                        first.make_ascii_uppercase();
                    }
                    (
                        *k,
                        label
                            .trim_end_matches(" for")
                            .trim_end_matches(" by")
                            .to_string(),
                    )
                }));
                aui::select(ui, pal, "audit_action", &mut self.filter_action, &actions);
            });
            ui.vertical(|ui| {
                ui.label(pal.section_label(" "));
                let changed = (self.filter_user.clone(), self.filter_action) != self.applied;
                if aui::primary(ui, pal, "Apply", changed).clicked() {
                    self.applied = (self.filter_user.clone(), self.filter_action);
                    out.send.push(load(
                        &cx.guild.id,
                        &self.filter_user,
                        self.filter_action,
                        None,
                    ));
                }
            });
        });
        ui.add_space(8.0);
        let Some(entries) = &self.entries else {
            aui::loading(ui, pal);
            return;
        };
        if entries.is_empty() {
            aui::empty(ui, pal, icons::FILE_TEXT, "No audit log entries match.");
            return;
        }
        let mut toggle = None;
        for e in entries {
            let id = s(e, "id");
            let kind = n(e, "action_type");
            let open = self.expanded.as_deref() == Some(id);
            let (icon, col) = match action_class(kind) {
                0 => (icons::PLUS_CIRCLE, pal.positive()),
                2 => (icons::TRASH, pal.danger),
                _ => (icons::PENCIL, pal.idle),
            };
            let (_, clicked) = aui::row(ui, pal, open, |ui| {
                ui.label(RichText::new(icon).font(icons::font(18.0)).color(col));
                ui.vertical(|ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing.x = 5.0;
                        ui.label(
                            RichText::new(self.user_name(s(e, "user_id")))
                                .color(pal.strong)
                                .bold(),
                        );
                        ui.label(RichText::new(action_text(kind)).color(pal.text));
                        let target = self.target_name(cx, e);
                        if !target.is_empty() {
                            ui.label(RichText::new(target).color(pal.strong).bold());
                        }
                    });
                    ui.label(
                        RichText::new(crate::timefmt::full(snowflake_time(id)))
                            .size(12.0)
                            .color(pal.muted),
                    );
                });
                aui::right(ui, |ui| {
                    crate::widgets::icon_button(
                        ui,
                        if open {
                            icons::CARET_DOWN
                        } else {
                            icons::CARET_RIGHT
                        },
                        "Details",
                        false,
                        true,
                        pal,
                    )
                    .clicked()
                })
            });
            if clicked {
                toggle = Some(id.to_string());
            }
            if open {
                aui::card(ui, pal, |ui| {
                    let changes = e["changes"].as_array();
                    for c in changes.into_iter().flatten() {
                        let key = s(c, "key").replace('_', " ");
                        let line = match (c.get("old_value"), c.get("new_value")) {
                            (Some(old), Some(new)) => {
                                format!("{key}: {} → {}", show_value(old), show_value(new))
                            }
                            (None, Some(new)) => format!("{key}: set to {}", show_value(new)),
                            (Some(old), None) => format!("{key}: removed {}", show_value(old)),
                            (None, None) => key,
                        };
                        ui.label(RichText::new(line).color(pal.text));
                    }
                    if changes.is_none_or(|c| c.is_empty()) {
                        ui.label(RichText::new("No recorded changes.").color(pal.muted));
                    }
                    let reason = s(e, "reason");
                    if !reason.is_empty() {
                        ui.label(RichText::new(format!("Reason: {reason}")).color(pal.muted));
                    }
                });
                ui.add_space(4.0);
            }
        }
        if let Some(id) = toggle {
            self.expanded = (self.expanded.as_deref() != Some(id.as_str())).then_some(id);
        }
        if self.more {
            ui.add_space(8.0);
            if aui::plain(ui, "Load more").clicked() {
                let last = entries.last().map(|e| s(e, "id").to_string());
                out.send.push(load(
                    &cx.guild.id,
                    &self.applied.0,
                    self.applied.1,
                    last.as_deref().or(Some("")),
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn load_builds_the_query() {
        let r = load("1", "", 0, None);
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.kind, Kind::LoadAudit);
        assert_eq!(r.path, "/guilds/1/audit-logs?limit=50");
        let r = load("1", " 42 ", 22, Some("900"));
        assert_eq!(r.kind, Kind::LoadAuditMore);
        assert_eq!(
            r.path,
            "/guilds/1/audit-logs?limit=50&user_id=42&action_type=22&before=900"
        );
        // Only digits ever reach the query string.
        assert!(!load("1", "4&x=1", 0, None).path.contains("user_id"));
    }

    #[test]
    fn pages_append_and_tokens_are_dropped() {
        let mut st = State::default();
        st.on_response(
            Kind::LoadAudit,
            "",
            &json!({
                "audit_log_entries": [{ "id": "5", "user_id": "u", "action_type": 22 }],
                "users": [{ "id": "u", "username": "sam", "avatar": "x" }],
                "webhooks": [{ "id": "w", "token": "SECRET" }],
            }),
        );
        assert_eq!(st.entries.as_ref().unwrap().len(), 1);
        assert_eq!(st.user_name("u"), "sam");
        assert!(!st.more);
        assert!(!serde_json::to_string(&st.users).unwrap().contains("avatar"));
        st.on_response(
            Kind::LoadAuditMore,
            "",
            &json!({ "audit_log_entries": [{ "id": "4" }] }),
        );
        assert_eq!(st.entries.as_ref().unwrap().len(), 2);
        st.on_response(Kind::LoadAudit, "", &json!({ "audit_log_entries": [] }));
        assert!(st.entries.as_ref().unwrap().is_empty());
    }

    #[test]
    fn helpers() {
        assert_eq!(action_text(22), "banned");
        assert!(action_text(9999).contains("9999"));
        // Discord's documented example snowflake.
        assert_eq!(snowflake_time("175928847299117063"), 1_462_015_105);
        assert_eq!(snowflake_time("x"), 0);
        assert_eq!(show_value(&json!(null)), "none");
        assert_eq!(show_value(&json!("a")), "a");
        assert_eq!(show_value(&json!(5)), "5");
        assert!(show_value(&json!("x".repeat(200))).ends_with('…'));
    }
}
