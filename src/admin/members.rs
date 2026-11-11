//! Server settings → Members and Bans: member search, nickname changes,
//! role changes, timeout, kick, ban (with a delete-messages window) and
//! the bans list with unban.
//!
//! Each tool needs its own permission, and a member can only be acted on
//! when their highest role is below mine (never the owner, never myself;
//! `perms::can_moderate`). Kick, ban, unban and timeout go through a
//! confirm; banning needs the member's name typed in.

use chrono::{DateTime, Duration, Utc};
use eframe::egui::{self, RichText};
use serde_json::{Value, json};

use super::roles::{member_has_role, member_name};
use super::{Confirm, Cx, Kind, Method, Out, Request, b, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

pub const TIMEOUTS: &[(i64, &str)] = &[
    (60, "60 seconds"),
    (300, "5 minutes"),
    (600, "10 minutes"),
    (3600, "1 hour"),
    (86_400, "1 day"),
    (604_800, "1 week"),
];

pub fn search(guild_id: &str, query: &str) -> Option<Request> {
    super::roles::member_search(guild_id, Kind::SearchMembers, query)
}

pub fn kick(guild_id: &str, user_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::Kick,
        Method::Delete,
        format!("/guilds/{guild_id}/members/{user_id}"),
    )
    .key(user_id)
}

/// The ban itself; the confirm dialog adds `delete_message_seconds`.
pub fn ban(guild_id: &str, user_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::Ban,
        Method::Put,
        format!("/guilds/{guild_id}/bans/{user_id}"),
    )
    .key(user_id)
    .body(json!({ "delete_message_seconds": 0 }))
}

pub fn unban(guild_id: &str, user_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::Unban,
        Method::Delete,
        format!("/guilds/{guild_id}/bans/{user_id}"),
    )
    .key(user_id)
}

pub fn load_bans(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadBans,
        Method::Get,
        format!("/guilds/{guild_id}/bans?limit=1000"),
    )
}

fn patch_member(guild_id: &str, kind: Kind, user_id: &str, body: Value) -> Request {
    Request::new(
        guild_id,
        kind,
        Method::Patch,
        format!("/guilds/{guild_id}/members/{user_id}"),
    )
    .key(user_id)
    .body(body)
}

/// Time a member out for `secs` from `now`, or lift it (`None`).
pub fn timeout(guild_id: &str, user_id: &str, secs: Option<i64>, now: DateTime<Utc>) -> Request {
    let until = match secs {
        Some(secs) => json!((now + Duration::seconds(secs)).to_rfc3339()),
        None => Value::Null,
    };
    patch_member(
        guild_id,
        Kind::Timeout,
        user_id,
        json!({ "communication_disabled_until": until }),
    )
}

/// Change a nickname (empty = remove it). My own goes to `@me`.
pub fn set_nick(guild_id: &str, user_id: &str, is_me: bool, nick: &str) -> Request {
    let nick = nick.trim();
    let body = json!({ "nick": if nick.is_empty() { Value::Null } else { json!(nick) } });
    let mut r = patch_member(
        guild_id,
        Kind::SetNick,
        if is_me { "@me" } else { user_id },
        body,
    );
    r.key = user_id.to_string();
    r
}

/// Replace a member's roles. `None` when nothing changed.
pub fn set_roles(guild_id: &str, user_id: &str, was: &[String], now: &[String]) -> Option<Request> {
    let mut a = was.to_vec();
    let mut b = now.to_vec();
    a.sort();
    b.sort();
    (a != b).then(|| {
        patch_member(
            guild_id,
            Kind::SetMemberRoles,
            user_id,
            json!({ "roles": b }),
        )
    })
}

fn user_id(m: &Value) -> &str {
    m.pointer("/user/id").and_then(Value::as_str).unwrap_or("")
}

fn role_ids(m: &Value) -> Vec<String> {
    m["roles"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| r.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Unix time a member is timed out until (0 = not timed out).
pub fn timed_out_until(m: &Value, now: i64) -> i64 {
    let until = crate::timefmt::parse_iso(s(m, "communication_disabled_until"));
    if until > now { until } else { 0 }
}

/// The member opened in the list.
struct Open {
    id: String,
    nick: String,
    roles: Vec<String>,
    timeout: i64,
}

#[derive(Default)]
pub struct State {
    query: String,
    found: Option<Vec<Value>>,
    open: Option<Open>,
    bans: Option<Vec<Value>>,
    ban_filter: String,
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::SearchMembers => {
                self.found = Some(super::list(v));
                self.open = None;
            }
            Kind::Kick | Kind::Ban => {
                if let Some(found) = &mut self.found {
                    let gone = found.iter().position(|m| user_id(m) == key);
                    if let Some(i) = gone {
                        let m = found.remove(i);
                        if kind == Kind::Ban
                            && let Some(bans) = &mut self.bans
                            && bans.len() < super::LIST_CAP
                        {
                            bans.push(json!({ "user": m["user"], "reason": Value::Null }));
                        }
                    }
                }
                self.open = None;
            }
            Kind::LoadBans => self.bans = Some(super::list(v)),
            Kind::Unban => {
                if let Some(bans) = &mut self.bans {
                    bans.retain(|x| user_id(x) != key);
                }
            }
            Kind::Timeout | Kind::SetNick | Kind::SetMemberRoles => {
                if let Some(m) = self.found.iter_mut().flatten().find(|m| user_id(m) == key) {
                    // The reply is the member (the demo echoes the change).
                    for field in ["nick", "roles", "communication_disabled_until"] {
                        if let Some(x) = v.get(field) {
                            m[field] = x.clone();
                        }
                    }
                    if let Some(o) = self.open.as_mut().filter(|o| o.id == key) {
                        o.nick = s(m, "nick").to_string();
                        o.roles = role_ids(m);
                    }
                }
            }
            _ => {}
        }
    }

    /// Demo screenshots: the demo members with one opened.
    pub fn demo_open(&mut self) {
        let found = super::list(&super::demo::members(""));
        self.open = found
            .iter()
            .find(|m| s(&m["user"], "username") == "troll")
            .map(|m| Open {
                id: user_id(m).to_string(),
                nick: String::new(),
                roles: role_ids(m),
                timeout: 600,
            });
        self.found = Some(found);
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        ui.horizontal(|ui| {
            let r = ui.add(
                egui::TextEdit::singleline(&mut self.query)
                    .hint_text("Search members by name or nickname")
                    .desired_width(ui.available_width().min(320.0)),
            );
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if aui::primary(ui, pal, "Search", !self.query.trim().is_empty()).clicked() || enter {
                out.send.extend(search(&cx.guild.id, &self.query));
            }
        });
        ui.add_space(6.0);
        let Some(found) = &self.found else {
            aui::empty(
                ui,
                pal,
                icons::MAGNIFYING_GLASS,
                "Search for a member to change their nickname or roles, time them out, kick or ban them.",
            );
            return;
        };
        if found.is_empty() {
            aui::empty(ui, pal, icons::USERS, "No members found.");
            return;
        }
        let now = Utc::now();
        let mut toggle_open = None;
        for m in found {
            let uid = user_id(m);
            let is_open = self.open.as_ref().is_some_and(|o| o.id == uid);
            let until = timed_out_until(m, now.timestamp());
            let (_, clicked) = aui::row(ui, pal, is_open, |ui| {
                let color = cx
                    .guild
                    .roles
                    .iter()
                    .filter(|r| r.color != 0 && member_has_role(m, &r.id))
                    .max_by_key(|r| r.position)
                    .map_or(0, |r| r.color);
                ui.label(
                    RichText::new(member_name(m))
                        .color(if color == 0 {
                            pal.strong
                        } else {
                            aui::rgb_color(pal, color)
                        })
                        .bold(),
                );
                ui.label(
                    RichText::new(format!("@{}", s(&m["user"], "username")))
                        .size(12.0)
                        .color(pal.muted),
                );
                if b(&m["user"], "bot") {
                    ui.label(RichText::new("APP").size(11.5).color(pal.accent).bold());
                }
                if uid == cx.guild.owner_id {
                    ui.label(RichText::new("owner").size(12.0).color(pal.muted));
                }
                if until > 0 {
                    ui.label(
                        RichText::new(icons::TIMER)
                            .font(icons::font(14.0))
                            .color(pal.danger),
                    )
                    .on_hover_text(format!("Timed out until {}", crate::timefmt::full(until)));
                }
                aui::right(ui, |ui| {
                    crate::widgets::icon_button(
                        ui,
                        if is_open {
                            icons::CARET_DOWN
                        } else {
                            icons::CARET_RIGHT
                        },
                        "Manage member",
                        false,
                        true,
                        pal,
                    )
                    .clicked()
                })
            });
            if clicked {
                toggle_open = Some((uid.to_string(), m.clone()));
            }
            if is_open && let Some(open) = &mut self.open {
                aui::card(ui, pal, |ui| member_ui(ui, cx, m, open, until, now, out));
                ui.add_space(4.0);
            }
        }
        if let Some((uid, m)) = toggle_open {
            self.open = match &self.open {
                Some(o) if o.id == uid => None,
                _ => Some(Open {
                    id: uid,
                    nick: s(&m, "nick").to_string(),
                    roles: role_ids(&m),
                    timeout: 600,
                }),
            };
        }
    }

    pub fn bans_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(bans) = &self.bans else {
            aui::loading(ui, pal);
            return;
        };
        if bans.is_empty() {
            aui::empty(
                ui,
                pal,
                icons::PROHIBIT,
                "Nobody is banned from this server.",
            );
            return;
        }
        ui.add(
            egui::TextEdit::singleline(&mut self.ban_filter)
                .hint_text("Filter bans by name")
                .desired_width(ui.available_width().min(320.0)),
        );
        ui.add_space(6.0);
        let needle = self.ban_filter.trim().to_lowercase();
        for ban in bans {
            let name = member_name(ban);
            let username = s(&ban["user"], "username");
            if !needle.is_empty()
                && !name.to_lowercase().contains(&needle)
                && !username.to_lowercase().contains(&needle)
            {
                continue;
            }
            aui::row(ui, pal, false, |ui| {
                ui.label(RichText::new(&name).color(pal.strong).bold());
                ui.label(
                    RichText::new(format!("@{username}"))
                        .size(12.0)
                        .color(pal.muted),
                );
                let reason = s(ban, "reason");
                ui.label(
                    RichText::new(if reason.is_empty() {
                        "No reason given"
                    } else {
                        reason
                    })
                    .size(13.0)
                    .color(pal.muted),
                );
                aui::right(ui, |ui| {
                    if aui::plain(ui, "Unban").clicked() {
                        out.confirm = Some(
                            Confirm::new(
                                "Unban member",
                                &format!("Let {name} join this server again?"),
                                "Unban",
                                unban(&cx.guild.id, user_id(ban)),
                            )
                            .reason(),
                        );
                    }
                });
            });
        }
    }
}

/// The tools for one member.
fn member_ui(
    ui: &mut egui::Ui,
    cx: &Cx,
    m: &Value,
    open: &mut Open,
    until: i64,
    now: DateTime<Utc>,
    out: &mut Out,
) {
    let pal = cx.pal;
    let uid = user_id(m);
    let name = member_name(m);
    let is_me = uid == cx.me_id;
    let was_roles = role_ids(m);
    let their_top = perms::top_role_position(&cx.guild.roles, &was_roles);
    let lower = perms::can_moderate(
        cx.owner,
        cx.top,
        cx.me_id,
        &cx.guild.owner_id,
        uid,
        their_top,
    );

    let joined = s(m, "joined_at");
    if !joined.is_empty() {
        aui::hint(
            ui,
            pal,
            &format!("Joined {}", crate::timefmt::full_iso(joined)),
        );
    }
    if !lower && !is_me {
        aui::hint(
            ui,
            pal,
            "This member's highest role is not below yours, so you can't moderate them.",
        );
    }

    // Nickname: Manage Nicknames for others, Change Nickname for myself.
    let can_nick = if is_me {
        cx.can(perms::CHANGE_NICKNAME) || cx.can(perms::MANAGE_NICKNAMES)
    } else {
        cx.can(perms::MANAGE_NICKNAMES) && lower
    };
    if can_nick {
        aui::label(ui, pal, "Nickname");
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut open.nick)
                    .hint_text(s(&m["user"], "username"))
                    .desired_width(ui.available_width().min(240.0))
                    .char_limit(32),
            );
            let changed = open.nick.trim() != s(m, "nick");
            if aui::save(ui, pal, "Save Nickname", changed).clicked() {
                out.send
                    .push(set_nick(&cx.guild.id, uid, is_me, &open.nick));
            }
        });
    }

    // Roles: only roles below my highest one; managed roles belong to apps.
    if cx.can(perms::MANAGE_ROLES) {
        aui::label(ui, pal, "Roles");
        ui.horizontal_wrapped(|ui| {
            for r in cx.guild.roles.iter().filter(|r| r.id != cx.guild.id) {
                let mine = perms::outranks(cx.owner, cx.top, r.position);
                let mut on = open.roles.contains(&r.id);
                let resp = ui.add_enabled(
                    mine,
                    egui::Checkbox::new(
                        &mut on,
                        RichText::new(&r.name).color(aui::rgb_color(pal, r.color)),
                    ),
                );
                if resp.changed() {
                    open.roles.retain(|x| x != &r.id);
                    if on {
                        open.roles.push(r.id.clone());
                    }
                }
                resp.on_disabled_hover_text("This role is not below your highest role");
            }
        });
        let req = set_roles(&cx.guild.id, uid, &was_roles, &open.roles);
        ui.horizontal(|ui| {
            if aui::save(ui, pal, "Save Roles", req.is_some()).clicked() {
                out.send.extend(req);
            }
            if open.roles != was_roles && aui::plain(ui, "Reset").clicked() {
                open.roles = was_roles.clone();
            }
        });
    }

    if !lower {
        return;
    }
    let (can_time, can_kick, can_ban) = (
        cx.can(perms::MODERATE_MEMBERS),
        cx.can(perms::KICK_MEMBERS),
        cx.can(perms::BAN_MEMBERS),
    );
    if !(can_time || can_kick || can_ban) {
        return;
    }
    aui::divider(ui, pal);
    if can_time {
        ui.horizontal_wrapped(|ui| {
            if until > 0 {
                ui.label(
                    RichText::new(format!(
                        "Timed out until {}",
                        crate::timefmt::full(until)
                    ))
                    .color(pal.danger),
                );
                if aui::plain(ui, "Remove Timeout").clicked() {
                    out.confirm = Some(
                        Confirm::new(
                            "Remove timeout",
                            &format!("Let {name} talk again now?"),
                            "Remove Timeout",
                            timeout(&cx.guild.id, uid, None, now),
                        )
                        .reason(),
                    );
                }
            } else {
                aui::select(
                    ui,
                    pal,
                    ("member_timeout", uid),
                    &mut open.timeout,
                    &aui::opts(TIMEOUTS),
                );
                if aui::icon_text(ui, icons::TIMER, "Time Out").clicked() {
                    let label = TIMEOUTS
                        .iter()
                        .find(|(t, _)| *t == open.timeout)
                        .map_or("a while", |(_, l)| l);
                    out.confirm = Some(
                        Confirm::new(
                            "Time out member",
                            &format!(
                                "Time out {name} for {label}? They can't send messages, react or speak until it ends."
                            ),
                            "Time Out",
                            timeout(&cx.guild.id, uid, Some(open.timeout), now),
                        )
                        .reason(),
                    );
                }
            }
        });
    }
    ui.horizontal(|ui| {
        if can_kick && aui::danger(ui, pal, "Kick", true).clicked() {
            out.confirm = Some(
                Confirm::new(
                    "Kick member",
                    &format!("Kick {name} from the server? They can rejoin with a new invite."),
                    "Kick",
                    kick(&cx.guild.id, uid),
                )
                .reason(),
            );
        }
        if can_ban && aui::danger(ui, pal, "Ban", true).clicked() {
            let username = s(&m["user"], "username");
            let mut c = Confirm::new(
                "Ban member",
                &format!("Ban {name} (@{username})? They can't rejoin until they are unbanned."),
                "Ban",
                ban(&cx.guild.id, uid),
            )
            .typed(username)
            .reason();
            c.ban_window = Some(0);
            out.confirm = Some(c);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 11, 12, 0, 0).unwrap()
    }

    #[test]
    fn request_shapes() {
        let k = kick("1", "u");
        assert_eq!(
            (k.method, k.path.as_str()),
            (Method::Delete, "/guilds/1/members/u")
        );
        let b = ban("1", "u");
        assert_eq!(
            (b.method, b.path.as_str()),
            (Method::Put, "/guilds/1/bans/u")
        );
        let u = unban("1", "u");
        assert_eq!(
            (u.method, u.path.as_str()),
            (Method::Delete, "/guilds/1/bans/u")
        );
        assert!(!load_bans("1").is_write());
        assert!(search("1", "").is_none());
    }

    #[test]
    fn timeout_sets_an_iso_time_or_null() {
        let t = timeout("1", "u", Some(600), now());
        assert_eq!(t.method, Method::Patch);
        assert_eq!(
            t.body,
            Some(json!({ "communication_disabled_until": "2026-11-11T12:10:00+00:00" }))
        );
        let lift = timeout("1", "u", None, now());
        assert_eq!(
            lift.body,
            Some(json!({ "communication_disabled_until": null }))
        );
    }

    #[test]
    fn my_own_nickname_goes_to_me() {
        let mine = set_nick("1", "me", true, " Leo ");
        assert_eq!(mine.path, "/guilds/1/members/@me");
        assert_eq!(mine.key, "me");
        assert_eq!(mine.body, Some(json!({ "nick": "Leo" })));
        let other = set_nick("1", "u", false, "");
        assert_eq!(other.path, "/guilds/1/members/u");
        assert_eq!(other.body, Some(json!({ "nick": null })));
    }

    #[test]
    fn roles_are_only_sent_when_changed() {
        let was = vec!["a".to_string(), "b".to_string()];
        assert!(set_roles("1", "u", &was, &["b".to_string(), "a".to_string()]).is_none());
        let r = set_roles("1", "u", &was, &["b".to_string()]).unwrap();
        assert_eq!(r.body, Some(json!({ "roles": ["b"] })));
    }

    #[test]
    fn a_ban_needs_the_name_typed_and_carries_the_window() {
        let mut c = Confirm::new("Ban", "", "Ban", ban("1", "u"))
            .typed("troll")
            .reason();
        c.ban_window = Some(86_400);
        assert!(!c.ready());
        c.typed = "troll".into();
        c.reason = "spam".into();
        let r = c.into_request().unwrap();
        assert_eq!(r.body, Some(json!({ "delete_message_seconds": 86_400 })));
        assert_eq!(r.reason.as_deref(), Some("spam"));
    }

    #[test]
    fn replies_update_the_lists() {
        let member = json!({ "user": { "id": "u", "username": "troll" }, "roles": [],
                             "nick": null });
        let mut st = State {
            found: Some(vec![member]),
            bans: Some(Vec::new()),
            ..Default::default()
        };
        st.on_response(Kind::SetNick, "u", &json!({ "nick": "T" }));
        assert_eq!(st.found.as_ref().unwrap()[0]["nick"], "T");
        st.on_response(
            Kind::Timeout,
            "u",
            &json!({ "communication_disabled_until": "2026-11-11T12:10:00+00:00" }),
        );
        let m = &st.found.as_ref().unwrap()[0];
        assert!(timed_out_until(m, now().timestamp()) > 0);
        assert_eq!(timed_out_until(m, now().timestamp() + 3600), 0);
        st.on_response(Kind::Ban, "u", &Value::Null);
        assert!(st.found.as_ref().unwrap().is_empty());
        assert_eq!(st.bans.as_ref().unwrap().len(), 1);
        st.on_response(Kind::Unban, "u", &Value::Null);
        assert!(st.bans.as_ref().unwrap().is_empty());
    }
}
