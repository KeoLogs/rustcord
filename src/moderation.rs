//! Server Settings → Members and → Bans, plus the moderation actions shared
//! with the member list and message authors:
//! [`member_context_menu`] → [`ModAction`] → [`ModDialogs`] (nickname,
//! timeout, kick and ban dialogs) → `AdminAction`.
//!
//! - Nickname: `PATCH /guilds/{g}/members/{u}` `{ nick }` (Manage Nicknames)
//!   or `/members/@me` for my own (Change Nickname). Empty resets it.
//! - Timeout: `PATCH /guilds/{g}/members/{u}`
//!   `{ communication_disabled_until: ISO-8601 | null }`, at most 28 days.
//! - Kick: `DELETE /guilds/{g}/members/{u}`; ban: `PUT /guilds/{g}/bans/{u}`
//!   `{ delete_message_seconds }` (0-604800). Reasons go in
//!   `X-Audit-Log-Reason`.
//!
//! Kick, ban and timeout always go through their dialog (which is the
//! confirmation); unban asks first too. Giving / taking a role is a direct
//! click like in Discord.
//!
//! Uncertainty: `members::Role` doesn't know whether a role is managed, so
//! the context menu's Roles submenu may offer a managed role; Discord then
//! refuses and the error comes back as a failed `AdminAction`.

use chrono::{SecondsFormat, TimeZone, Utc};
use eframe::egui::{self, RichText, Sense, vec2};
use serde_json::Value;

use crate::guild_admin::{
    self as ga, AdminAction, AdminCtx, AdminOutcome, Icon, ImageUris, Load, SearchFor,
};
use crate::members::{Member, role_color};
use crate::theme::{Bold, Palette};
use crate::timefmt;
use crate::types::User;

/// Discord's timeout choices: (seconds, label).
pub const TIMEOUT_PRESETS: [(u32, &str); 6] = [
    (60, "60 secs"),
    (5 * 60, "5 mins"),
    (10 * 60, "10 mins"),
    (60 * 60, "1 hour"),
    (24 * 60 * 60, "1 day"),
    (7 * 24 * 60 * 60, "1 week"),
];
/// Longest timeout Discord allows.
pub const MAX_TIMEOUT_SECS: u32 = 28 * 24 * 60 * 60;
/// "Delete message history" choices in the ban dialog: (seconds, label).
pub const BAN_DELETE_OPTIONS: [(u32, &str); 7] = [
    (0, "Don't Delete Any"),
    (60 * 60, "Previous Hour"),
    (6 * 60 * 60, "Previous 6 Hours"),
    (12 * 60 * 60, "Previous 12 Hours"),
    (24 * 60 * 60, "Previous 24 Hours"),
    (3 * 24 * 60 * 60, "Previous 3 Days"),
    (7 * 24 * 60 * 60, "Previous 7 Days"),
];
/// Nicknames: at most 32 characters.
pub const MAX_NICK: usize = 32;
/// Bans per page (Discord's maximum).
const BANS_PAGE: usize = 1000;
const MEMBERS_PAGE: usize = 100;

/// End of a timeout of `secs` from `now` (unix seconds) as Discord wants
/// it, clamped to 28 days.
pub fn timeout_until(now: i64, secs: u32) -> Option<String> {
    let secs = secs.min(MAX_TIMEOUT_SECS) as i64;
    Utc.timestamp_opt(now + secs, 0)
        .single()
        .map(|t| t.to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// "4m", "2h 5m", "3d 4h" — what's left of a timeout.
pub fn remaining(until: i64, now: i64) -> String {
    let s = (until - now).max(0);
    let (d, h, m) = (s / 86_400, (s % 86_400) / 3600, (s % 3600) / 60);
    if d > 0 {
        format!("{d}d {h}h")
    } else if h > 0 {
        format!("{h}h {m}m")
    } else if m > 0 {
        format!("{m}m")
    } else {
        format!("{s}s")
    }
}

fn now() -> i64 {
    Utc::now().timestamp()
}

// ---------------------------------------------------------------------------
// Members
// ---------------------------------------------------------------------------

/// A server member as the admin pages show them.
#[derive(Debug, Clone, Default)]
pub struct MemberRow {
    pub user: User,
    pub nick: Option<String>,
    pub roles: Vec<String>,
    /// Unix seconds (0 unknown).
    pub joined_at: i64,
    /// End of their timeout (unix seconds, 0 = none).
    pub timeout_until: i64,
}

impl MemberRow {
    pub fn from_json(v: &Value) -> Self {
        let m = Member::from_json(v);
        Self {
            joined_at: timefmt::parse_iso(&m.joined_at),
            timeout_until: v["communication_disabled_until"]
                .as_str()
                .map(timefmt::parse_iso)
                .unwrap_or(0),
            user: m.user,
            nick: m.nick,
            roles: m.roles,
        }
    }

    /// A list of member objects (search / list answers).
    pub fn parse_list(v: &Value) -> Vec<MemberRow> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(MemberRow::from_json)
                    .filter(|m| !m.user.id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn name(&self) -> &str {
        self.nick.as_deref().unwrap_or(&self.user.name)
    }

    pub fn timed_out(&self, now: i64) -> bool {
        self.timeout_until > now
    }

    pub fn target(&self) -> ModTarget {
        ModTarget {
            user_id: self.user.id.clone(),
            name: self.name().to_string(),
            nick: self.nick.clone(),
            roles: self.roles.clone(),
            timeout_until: self.timeout_until,
        }
    }
}

/// Who a moderation action is about.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ModTarget {
    pub user_id: String,
    /// Display name (nick or global name).
    pub name: String,
    pub nick: Option<String>,
    pub roles: Vec<String>,
    /// Unix seconds, 0 = not timed out.
    pub timeout_until: i64,
}

/// What the user picked in a member menu.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModAction {
    /// Opens the nickname dialog.
    Nickname(ModTarget),
    /// Opens the timeout dialog.
    Timeout(ModTarget),
    /// Direct.
    RemoveTimeout(ModTarget),
    /// Opens the kick dialog.
    Kick(ModTarget),
    /// Opens the ban dialog.
    Ban(ModTarget),
    /// Direct: give / take a role.
    SetRole {
        user_id: String,
        role_id: String,
        add: bool,
    },
}

/// Is there anything I may do to this member (show the menu section)?
pub fn has_mod_actions(ctx: &AdminCtx, t: &ModTarget) -> bool {
    let a = &ctx.actor;
    a.can_change_nick(&t.user_id, &t.roles)
        || a.can_timeout(&t.user_id, &t.roles)
        || a.can_kick(&t.user_id, &t.roles)
        || a.can_ban(&t.user_id, &t.roles)
        || a.has(crate::permissions_table::MANAGE_ROLES)
}

/// Moderation entries for a member's right-click menu (member list,
/// message authors, the Members page). Call inside a menu `Ui`.
pub fn member_context_menu(ui: &mut egui::Ui, ctx: &AdminCtx, t: &ModTarget) -> Option<ModAction> {
    let a = &ctx.actor;
    let pal = ctx.pal;
    let mut out = None;
    let me = t.user_id == a.user_id;
    if a.can_change_nick(&t.user_id, &t.roles) {
        let label = if me { "Change Nickname" } else { "Change Nickname…" };
        if ui.button(label).clicked() {
            out = Some(ModAction::Nickname(t.clone()));
            ui.close();
        }
    }
    let assignable: Vec<_> = a
        .roles
        .iter()
        .filter(|r| r.id != a.guild_id && a.can_assign_role(r, false))
        .collect();
    if !assignable.is_empty() {
        ui.menu_button("Roles", |ui| {
            egui::ScrollArea::vertical().max_height(320.0).show(ui, |ui| {
                for r in assignable {
                    let mut has = t.roles.contains(&r.id);
                    let text = RichText::new(&r.name).color(r.color32().unwrap_or(pal.text));
                    if ui.checkbox(&mut has, text).changed() {
                        out = Some(ModAction::SetRole {
                            user_id: t.user_id.clone(),
                            role_id: r.id.clone(),
                            add: has,
                        });
                    }
                }
            });
        });
    }
    let can_timeout = a.can_timeout(&t.user_id, &t.roles);
    let can_kick = a.can_kick(&t.user_id, &t.roles);
    let can_ban = a.can_ban(&t.user_id, &t.roles);
    if can_timeout || can_kick || can_ban {
        ui.separator();
    }
    if can_timeout {
        if t.timeout_until > now() {
            if ui.button("Remove Timeout").clicked() {
                out = Some(ModAction::RemoveTimeout(t.clone()));
                ui.close();
            }
        } else if ui
            .button(RichText::new(format!("Timeout {}", t.name)).color(pal.danger))
            .clicked()
        {
            out = Some(ModAction::Timeout(t.clone()));
            ui.close();
        }
    }
    if can_kick
        && ui
            .button(RichText::new(format!("Kick {}", t.name)).color(pal.danger))
            .clicked()
    {
        out = Some(ModAction::Kick(t.clone()));
        ui.close();
    }
    if can_ban
        && ui
            .button(RichText::new(format!("Ban {}", t.name)).color(pal.danger))
            .clicked()
    {
        out = Some(ModAction::Ban(t.clone()));
        ui.close();
    }
    out
}

#[derive(Debug, Clone)]
enum Dialog {
    Nickname { t: ModTarget, text: String },
    Timeout { t: ModTarget, secs: u32, reason: String },
    Kick { t: ModTarget, reason: String },
    Ban { t: ModTarget, delete_secs: u32, reason: String },
}

/// The nickname / timeout / kick / ban dialogs.
#[derive(Default)]
pub struct ModDialogs {
    open: Option<Dialog>,
}

impl ModDialogs {
    pub fn is_open(&self) -> bool {
        self.open.is_some()
    }

    /// Open the dialog for `action`, or turn a direct one straight into an
    /// `AdminAction`.
    pub fn handle(&mut self, action: ModAction, guild_id: &str) -> Option<AdminAction> {
        let gid = guild_id.to_string();
        match action {
            ModAction::Nickname(t) => {
                let text = t.nick.clone().unwrap_or_default();
                self.open = Some(Dialog::Nickname { t, text });
                None
            }
            ModAction::Timeout(t) => {
                self.open = Some(Dialog::Timeout {
                    t,
                    secs: TIMEOUT_PRESETS[0].0,
                    reason: String::new(),
                });
                None
            }
            ModAction::Kick(t) => {
                self.open = Some(Dialog::Kick {
                    t,
                    reason: String::new(),
                });
                None
            }
            ModAction::Ban(t) => {
                self.open = Some(Dialog::Ban {
                    t,
                    delete_secs: 0,
                    reason: String::new(),
                });
                None
            }
            ModAction::RemoveTimeout(t) => Some(AdminAction::Timeout {
                guild_id: gid,
                user_id: t.user_id,
                until: None,
                reason: String::new(),
            }),
            ModAction::SetRole {
                user_id,
                role_id,
                add,
            } => Some(if add {
                AdminAction::AddMemberRole {
                    guild_id: gid,
                    user_id,
                    role_id,
                }
            } else {
                AdminAction::RemoveMemberRole {
                    guild_id: gid,
                    user_id,
                    role_id,
                }
            }),
        }
    }

    /// Draw the open dialog; returns the action when the user confirms.
    pub fn show(&mut self, ectx: &egui::Context, ctx: &AdminCtx) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let me = ctx.actor.user_id.to_string();
        let dialog = self.open.as_mut()?;
        let mut action = None;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("rc_mod_dialog")).show(ectx, |ui| {
            ui.set_width(420.0);
            match dialog {
                Dialog::Nickname { t, text } => {
                    ui.heading("Change Nickname");
                    ui.add_space(6.0);
                    ui.label(pal.section_label("Nickname"));
                    let r = ui.add(
                        egui::TextEdit::singleline(text)
                            .char_limit(MAX_NICK)
                            .hint_text(t.name.as_str())
                            .desired_width(f32::INFINITY),
                    );
                    r.request_focus();
                    if ui.link("Reset Nickname").clicked() {
                        text.clear();
                    }
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        if ui.add(pal.primary_button("Save")).clicked() || enter {
                            let nick = text.trim();
                            action = Some(AdminAction::SetNickname {
                                guild_id: gid.clone(),
                                user_id: t.user_id.clone(),
                                nick: (!nick.is_empty()).then(|| nick.to_string()),
                                is_me: t.user_id == me,
                            });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Timeout { t, secs, reason } => {
                    ui.heading(format!("Timeout {}", t.name));
                    ui.label(
                        RichText::new(
                            "Members who are in timeout can't send messages, reply, react, \
                             or speak in voice or stage channels.",
                        )
                        .color(pal.muted),
                    );
                    ui.add_space(8.0);
                    ui.label(pal.section_label("Duration"));
                    ui.horizontal_wrapped(|ui| {
                        for (s, label) in TIMEOUT_PRESETS {
                            ui.selectable_value(secs, s, label);
                        }
                    });
                    ui.add_space(8.0);
                    reason_field(ui, pal, reason);
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ga::danger_button(pal, "Timeout")).clicked() {
                            action = Some(AdminAction::Timeout {
                                guild_id: gid.clone(),
                                user_id: t.user_id.clone(),
                                until: timeout_until(now(), *secs),
                                reason: reason.clone(),
                            });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Kick { t, reason } => {
                    ui.heading(format!("Kick '{}' from Server", t.name));
                    ui.label(format!(
                        "Are you sure you want to kick {} from the server? \
                         They will be able to rejoin again with a new invite.",
                        t.name
                    ));
                    ui.add_space(8.0);
                    reason_field(ui, pal, reason);
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ga::danger_button(pal, "Kick")).clicked() {
                            action = Some(AdminAction::Kick {
                                guild_id: gid.clone(),
                                user_id: t.user_id.clone(),
                                reason: reason.clone(),
                            });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
                Dialog::Ban {
                    t,
                    delete_secs,
                    reason,
                } => {
                    ui.heading(format!("Ban '{}'", t.name));
                    ui.label(format!("Are you sure you want to ban {}?", t.name));
                    ui.add_space(8.0);
                    ui.label(pal.section_label("Delete message history"));
                    let current = BAN_DELETE_OPTIONS
                        .iter()
                        .find(|(s, _)| s == delete_secs)
                        .map_or("Don't Delete Any", |(_, l)| l);
                    egui::ComboBox::from_id_salt("rc_ban_delete")
                        .selected_text(current)
                        .width(220.0)
                        .show_ui(ui, |ui| {
                            for (s, label) in BAN_DELETE_OPTIONS {
                                ui.selectable_value(delete_secs, s, label);
                            }
                        });
                    ui.add_space(8.0);
                    reason_field(ui, pal, reason);
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.add(ga::danger_button(pal, "Ban")).clicked() {
                            action = Some(AdminAction::Ban {
                                guild_id: gid.clone(),
                                user_id: t.user_id.clone(),
                                delete_message_seconds: *delete_secs,
                                reason: reason.clone(),
                            });
                            close = true;
                        }
                        if ui.button("Cancel").clicked() {
                            close = true;
                        }
                    });
                }
            }
        });
        if close || modal.should_close() {
            self.open = None;
        }
        action
    }
}

fn reason_field(ui: &mut egui::Ui, pal: &Palette, reason: &mut String) {
    ui.label(pal.section_label("Reason"));
    ui.add(
        egui::TextEdit::multiline(reason)
            .char_limit(ga::MAX_REASON)
            .desired_rows(2)
            .hint_text("Shown in the audit log (optional)")
            .desired_width(f32::INFINITY),
    );
}

/// Server Settings → Members.
#[derive(Default)]
pub struct MembersPage {
    list: Load<Vec<MemberRow>>,
    query: String,
    /// Paging for `ListMembers` (bots).
    more: bool,
    error: Option<String>,
    uris: ImageUris,
}

impl MembersPage {
    /// Fill the list from elsewhere (e.g. an op 8 `query` chunk for user
    /// accounts, as `members` objects).
    pub fn set_members(&mut self, rows: Vec<MemberRow>) {
        self.more = false;
        self.list = Load::Loaded(rows);
    }

    pub fn forget_images(&mut self, ctx: &egui::Context) {
        self.uris.forget(ctx);
    }

    fn update_row(&mut self, user_id: &str, f: impl Fn(&mut MemberRow)) {
        if let Load::Loaded(rows) = &mut self.list {
            for r in rows.iter_mut().filter(|r| r.user.id == user_id) {
                f(r);
            }
        }
    }

    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        use AdminAction::*;
        let relevant = matches!(
            action,
            ListMembers { .. }
                | SearchMembers {
                    purpose: SearchFor::MembersPage,
                    ..
                }
                | SetNickname { .. }
                | Timeout { .. }
                | Kick { .. }
                | Ban { .. }
                | AddMemberRole { .. }
                | RemoveMemberRole { .. }
        );
        if !relevant {
            return;
        }
        let value = match result {
            Ok(AdminOutcome::Json(v)) => v,
            Ok(_) => return,
            Err(e) => {
                match action {
                    ListMembers { .. } | SearchMembers { .. } => {
                        self.list = Load::Failed(e.clone());
                    }
                    _ => self.error = Some(e.clone()),
                }
                return;
            }
        };
        self.error = None;
        match action {
            ListMembers { after, .. } => {
                let rows = MemberRow::parse_list(value);
                self.more = rows.len() >= MEMBERS_PAGE;
                match (&mut self.list, after) {
                    (Load::Loaded(list), Some(_)) => list.extend(rows),
                    _ => self.list = Load::Loaded(rows),
                }
            }
            SearchMembers { .. } => {
                self.more = false;
                self.list = Load::Loaded(MemberRow::parse_list(value));
            }
            SetNickname { user_id, nick, .. } => {
                // Answer: the member (both @me and {id} forms).
                let nick = value
                    .get("nick")
                    .map(|n| n.as_str().map(str::to_string))
                    .unwrap_or_else(|| nick.clone());
                self.update_row(user_id, |r| r.nick = nick.clone().filter(|n| !n.is_empty()));
            }
            Timeout { user_id, until, .. } => {
                let t = until.as_deref().map(timefmt::parse_iso).unwrap_or(0);
                self.update_row(user_id, |r| r.timeout_until = t);
            }
            Kick { user_id, .. } | Ban { user_id, .. } => {
                if let Load::Loaded(rows) = &mut self.list {
                    rows.retain(|r| &r.user.id != user_id);
                }
            }
            AddMemberRole {
                user_id, role_id, ..
            } => self.update_row(user_id, |r| {
                if !r.roles.contains(role_id) {
                    r.roles.push(role_id.clone());
                }
            }),
            RemoveMemberRole {
                user_id, role_id, ..
            } => self.update_row(user_id, |r| r.roles.retain(|x| x != role_id)),
            _ => {}
        }
    }

    /// Draw the page. Returns a REST action and/or a moderation pick (pass
    /// the latter to `ModDialogs::handle`).
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx) -> (Option<AdminAction>, Option<ModAction>) {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let mut action = None;
        let mut pick = None;
        if self.list.is_idle() && ctx.is_bot {
            self.list = Load::Loading;
            action = Some(AdminAction::ListMembers {
                guild_id: gid.clone(),
                after: None,
            });
        }
        ui.heading("Server Members");
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ga::search_box(ui, &mut self.query, "Search by username or nickname", 300.0, pal);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if ui.button("Search").clicked() || enter {
                let q = self.query.trim().to_string();
                self.list = Load::Loading;
                action = Some(if q.is_empty() {
                    AdminAction::ListMembers {
                        guild_id: gid.clone(),
                        after: None,
                    }
                } else {
                    AdminAction::SearchMembers {
                        guild_id: gid.clone(),
                        query: q,
                        purpose: SearchFor::MembersPage,
                    }
                });
            }
        });
        if let Some(e) = &self.error {
            ui.add_space(4.0);
            ga::error_line(ui, pal, e);
        }
        ui.add_space(8.0);
        let rows = match &self.list {
            Load::Idle => {
                ui.label(
                    RichText::new("Search for a member by name to manage them.").color(pal.muted),
                );
                return (action, pick);
            }
            Load::Loading => {
                ui.spinner();
                return (action, pick);
            }
            Load::Failed(e) => {
                ga::error_line(ui, pal, e);
                if !ctx.is_bot {
                    ui.label(
                        RichText::new("User accounts can't list every member; try a search.")
                            .small()
                            .color(pal.muted),
                    );
                }
                return (action, pick);
            }
            Load::Loaded(rows) => rows.clone(),
        };
        ui.label(pal.section_label(&format!("{} members", rows.len())));
        let now = now();
        egui::ScrollArea::vertical()
            .id_salt("rc_admin_members")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for m in &rows {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 4, pal.hover);
                    }
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(vec2(8.0, 6.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    let t = m.target();
                    let ui = &mut child;
                    ga::avatar(ui, 32.0, &m.user.id, m.name(), &m.user.avatar_url, ctx, &mut self.uris);
                    ui.vertical(|ui| {
                        ui.horizontal(|ui| {
                            let col = role_color(ctx.actor.roles, &m.roles).unwrap_or(pal.text);
                            ui.label(RichText::new(m.name()).bold().color(col));
                            if m.user.bot {
                                ui.label(RichText::new("BOT").small().color(pal.accent));
                            }
                            if m.timed_out(now) {
                                ui.label(
                                    RichText::new(format!(
                                        "Timed out · {}",
                                        remaining(m.timeout_until, now)
                                    ))
                                    .small()
                                    .color(pal.danger),
                                );
                            }
                        });
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(&m.user.username).small().color(pal.muted));
                            if let Some(t) = timefmt::local(m.joined_at) {
                                ui.label(
                                    RichText::new(format!("· joined {}", t.format("%d/%m/%Y")))
                                        .small()
                                        .color(pal.muted),
                                );
                            }
                        });
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let more = ga::icon_button(ui, Icon::More, "More", pal, true);
                        egui::Popup::menu(&more).show(|ui| {
                            if let Some(a) = member_context_menu(ui, ctx, &t) {
                                pick = Some(a);
                            }
                        });
                        // Up to three role chips.
                        let mine: Vec<_> = ctx
                            .actor
                            .roles
                            .iter()
                            .filter(|r| m.roles.contains(&r.id))
                            .collect();
                        if mine.len() > 3 {
                            ui.label(
                                RichText::new(format!("+{}", mine.len() - 3))
                                    .small()
                                    .color(pal.muted),
                            );
                        }
                        for r in mine.iter().take(3).rev() {
                            role_chip(ui, pal, &r.name, r.color);
                        }
                    });
                    egui::Popup::context_menu(&resp).show(|ui| {
                        if let Some(a) = member_context_menu(ui, ctx, &t) {
                            pick = Some(a);
                        }
                    });
                }
                if self.more && ui.button("Load more").clicked() {
                    action = Some(AdminAction::ListMembers {
                        guild_id: gid.clone(),
                        after: rows.last().map(|r| r.user.id.clone()),
                    });
                }
            });
        (action, pick)
    }
}

/// A Discord-style role pill: colour dot + name.
pub fn role_chip(ui: &mut egui::Ui, pal: &Palette, name: &str, color: u32) {
    egui::Frame::new()
        .fill(pal.input)
        .corner_radius(4)
        .inner_margin(egui::Margin::symmetric(5, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                let (r, _) = ui.allocate_exact_size(vec2(8.0, 8.0), Sense::hover());
                let col = if color == 0 {
                    pal.muted
                } else {
                    crate::members::rgb(color)
                };
                ui.painter().circle_filled(r.center(), 4.0, col);
                ui.label(RichText::new(name).small().color(pal.text));
            });
        });
}

// ---------------------------------------------------------------------------
// Bans
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct BanRow {
    pub user: User,
    pub reason: Option<String>,
}

impl BanRow {
    pub fn parse_list(v: &Value) -> Vec<BanRow> {
        v.as_array()
            .map(|a| {
                a.iter()
                    .map(|b| BanRow {
                        user: User::from_json(&b["user"]),
                        reason: b["reason"]
                            .as_str()
                            .filter(|r| !r.trim().is_empty())
                            .map(str::to_string),
                    })
                    .filter(|b| !b.user.id.is_empty())
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Search box match: name, username, id or reason.
    pub fn matches(&self, query: &str) -> bool {
        let q = query.trim().to_lowercase();
        q.is_empty()
            || self.user.name.to_lowercase().contains(&q)
            || self.user.username.to_lowercase().contains(&q)
            || self.user.id == q
            || self
                .reason
                .as_deref()
                .is_some_and(|r| r.to_lowercase().contains(&q))
    }
}

/// Server Settings → Bans.
#[derive(Default)]
pub struct BansPage {
    list: Load<Vec<BanRow>>,
    filter: String,
    more: bool,
    open: Option<BanRow>,
    confirm_unban: Option<BanRow>,
    error: Option<String>,
    uris: ImageUris,
}

impl BansPage {
    pub fn forget_images(&mut self, ctx: &egui::Context) {
        self.uris.forget(ctx);
    }

    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        match (action, result) {
            (AdminAction::LoadBans { after, .. }, Ok(AdminOutcome::Json(v))) => {
                let rows = BanRow::parse_list(v);
                self.more = rows.len() >= BANS_PAGE;
                match (&mut self.list, after) {
                    (Load::Loaded(list), Some(_)) => list.extend(rows),
                    _ => self.list = Load::Loaded(rows),
                }
            }
            (AdminAction::LoadBans { .. }, Err(e)) => self.list = Load::Failed(e.clone()),
            (AdminAction::Unban { user_id, .. }, Ok(_)) => {
                self.error = None;
                if let Load::Loaded(list) = &mut self.list {
                    list.retain(|b| &b.user.id != user_id);
                }
            }
            (AdminAction::Unban { .. }, Err(e)) => self.error = Some(e.clone()),
            // A new ban: reload next time the page is shown.
            (AdminAction::Ban { .. }, Ok(_)) => {
                if matches!(self.list, Load::Loaded(_)) {
                    self.list = Load::Idle;
                }
            }
            _ => {}
        }
    }

    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let mut action = None;
        if self.list.is_idle() {
            self.list = Load::Loading;
            return Some(AdminAction::LoadBans {
                guild_id: gid,
                after: None,
            });
        }
        ui.heading("Server Ban List");
        ui.label(
            RichText::new("Bans by default are by account and IP. A user can get around an IP ban by using a proxy.")
                .color(pal.muted),
        );
        ui.add_space(8.0);
        ga::search_box(ui, &mut self.filter, "Search bans by user ID or username", 320.0, pal);
        if let Some(e) = &self.error {
            ui.add_space(4.0);
            ga::error_line(ui, pal, e);
        }
        ui.add_space(8.0);
        let rows = match &self.list {
            Load::Loading | Load::Idle => {
                ui.spinner();
                return None;
            }
            Load::Failed(e) => {
                ga::error_line(ui, pal, e);
                if ui.button("Try again").clicked() {
                    self.list = Load::Idle;
                }
                return None;
            }
            Load::Loaded(rows) => rows.clone(),
        };
        let shown: Vec<&BanRow> = rows.iter().filter(|b| b.matches(&self.filter)).collect();
        ui.label(pal.section_label(&format!("{} bans", shown.len())));
        if rows.is_empty() {
            ui.add_space(20.0);
            ui.label(RichText::new("No bans. Nobody has been banned from this server.").color(pal.muted));
        }
        egui::ScrollArea::vertical()
            .id_salt("rc_admin_bans")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for b in shown {
                    let (rect, resp) =
                        ui.allocate_exact_size(vec2(ui.available_width(), 44.0), Sense::click());
                    if resp.hovered() {
                        ui.painter().rect_filled(rect, 4, pal.hover);
                    }
                    let mut child = ui.new_child(
                        egui::UiBuilder::new()
                            .max_rect(rect.shrink2(vec2(8.0, 4.0)))
                            .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    );
                    ga::avatar(&mut child, 32.0, &b.user.id, &b.user.name, &b.user.avatar_url, ctx, &mut self.uris);
                    child.vertical(|ui| {
                        ui.label(RichText::new(&b.user.name).bold().color(pal.text));
                        ui.label(RichText::new(&b.user.username).small().color(pal.muted));
                    });
                    if resp.clicked() {
                        self.open = Some(b.clone());
                    }
                }
                if self.more && ui.button("Load more").clicked() {
                    action = Some(AdminAction::LoadBans {
                        guild_id: gid.clone(),
                        after: rows.last().map(|b| b.user.id.clone()),
                    });
                }
            });

        // Ban details.
        if let Some(b) = self.open.clone() {
            let mut close = false;
            let can_unban = ctx.actor.has(crate::permissions_table::BAN_MEMBERS);
            let modal = egui::Modal::new(egui::Id::unique("rc_ban_details")).show(ui.ctx(), |ui| {
                ui.set_width(400.0);
                ui.heading(format!("'{}'", b.user.name));
                ui.label(RichText::new(&b.user.username).color(pal.muted));
                ui.add_space(8.0);
                ui.label(pal.section_label("Ban reason"));
                ui.label(b.reason.as_deref().unwrap_or("No reason given."));
                ui.add_space(12.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Done").clicked() {
                        close = true;
                    }
                    if can_unban && ui.add(ga::danger_button(pal, "Revoke Ban")).clicked() {
                        self.confirm_unban = Some(b.clone());
                        close = true;
                    }
                });
            });
            if close || modal.should_close() {
                self.open = None;
            }
        }
        if let Some(b) = self.confirm_unban.clone() {
            match ga::confirm(
                ui.ctx(),
                pal,
                "unban",
                "Revoke Ban",
                &format!("Unban {}? They will be able to rejoin with an invite.", b.user.name),
                "Revoke Ban",
            ) {
                Some(true) => {
                    self.confirm_unban = None;
                    action = Some(AdminAction::Unban {
                        guild_id: gid,
                        user_id: b.user.id,
                    });
                }
                Some(false) => self.confirm_unban = None,
                None => {}
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
    fn timeout_durations() {
        let now = 1_762_689_600; // 2025-11-09T12:00:00Z
        assert_eq!(timeout_until(now, 60).unwrap(), "2025-11-09T12:01:00Z");
        assert_eq!(timeout_until(now, 300).unwrap(), "2025-11-09T12:05:00Z");
        assert_eq!(timeout_until(now, 600).unwrap(), "2025-11-09T12:10:00Z");
        assert_eq!(timeout_until(now, 3600).unwrap(), "2025-11-09T13:00:00Z");
        assert_eq!(timeout_until(now, 86_400).unwrap(), "2025-11-10T12:00:00Z");
        assert_eq!(timeout_until(now, 604_800).unwrap(), "2025-11-16T12:00:00Z");
        // Clamped to 28 days.
        assert_eq!(timeout_until(now, u32::MAX).unwrap(), "2025-12-07T12:00:00Z");
        // Round trip through the ISO parser.
        let s = timeout_until(now, 3600).unwrap();
        assert_eq!(timefmt::parse_iso(&s), now + 3600);
        let secs: Vec<u32> = TIMEOUT_PRESETS.iter().map(|p| p.0).collect();
        assert_eq!(secs, [60, 300, 600, 3600, 86_400, 604_800]);
        assert!(BAN_DELETE_OPTIONS.iter().all(|(s, _)| *s <= 604_800));
    }

    #[test]
    fn remaining_labels() {
        assert_eq!(remaining(100, 70), "30s");
        assert_eq!(remaining(400, 100), "5m");
        assert_eq!(remaining(3600 * 2 + 300, 0), "2h 5m");
        assert_eq!(remaining(86_400 * 3 + 3600 * 4, 0), "3d 4h");
        assert_eq!(remaining(0, 100), "0s");
    }

    #[test]
    fn member_rows() {
        let v = json!([
            {
                "user": { "id": "1", "username": "sam", "global_name": "Sam" },
                "nick": "Sammy", "roles": ["r1"],
                "joined_at": "2024-01-01T00:00:00+00:00",
                "communication_disabled_until": "2030-01-01T00:00:00+00:00"
            },
            { "user": { "id": "2", "username": "kai" }, "nick": null, "roles": [] },
            { "nick": "no user" }
        ]);
        let rows = MemberRow::parse_list(&v);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].name(), "Sammy");
        assert!(rows[0].timed_out(now()));
        assert_eq!(rows[1].name(), "kai");
        assert!(!rows[1].timed_out(now()));
        let t = rows[0].target();
        assert_eq!(t.nick.as_deref(), Some("Sammy"));
        assert_eq!(t.roles, ["r1"]);
    }

    #[test]
    fn dialogs_turn_into_actions() {
        let mut d = ModDialogs::default();
        let t = ModTarget {
            user_id: "5".into(),
            name: "Sam".into(),
            ..Default::default()
        };
        // Dialog-backed actions open a dialog and send nothing yet.
        assert_eq!(d.handle(ModAction::Ban(t.clone()), "g"), None);
        assert!(d.is_open());
        assert_eq!(d.handle(ModAction::Kick(t.clone()), "g"), None);
        // Direct ones.
        assert_eq!(
            d.handle(ModAction::RemoveTimeout(t.clone()), "g"),
            Some(AdminAction::Timeout {
                guild_id: "g".into(),
                user_id: "5".into(),
                until: None,
                reason: String::new()
            })
        );
        assert_eq!(
            d.handle(
                ModAction::SetRole {
                    user_id: "5".into(),
                    role_id: "r".into(),
                    add: false
                },
                "g"
            ),
            Some(AdminAction::RemoveMemberRole {
                guild_id: "g".into(),
                user_id: "5".into(),
                role_id: "r".into()
            })
        );
    }

    #[test]
    fn members_page_results() {
        let mut p = MembersPage::default();
        let g = "g".to_string();
        p.on_result(
            &AdminAction::SearchMembers {
                guild_id: g.clone(),
                query: "s".into(),
                purpose: SearchFor::MembersPage,
            },
            &Ok(AdminOutcome::Json(json!([
                { "user": { "id": "1", "username": "sam" }, "roles": [] },
                { "user": { "id": "2", "username": "kai" }, "roles": [] }
            ]))),
        );
        p.on_result(
            &AdminAction::AddMemberRole {
                guild_id: g.clone(),
                user_id: "1".into(),
                role_id: "r".into(),
            },
            &Ok(AdminOutcome::Json(Value::Null)),
        );
        p.on_result(
            &AdminAction::Kick {
                guild_id: g.clone(),
                user_id: "2".into(),
                reason: String::new(),
            },
            &Ok(AdminOutcome::Json(Value::Null)),
        );
        let rows = p.list.get().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].roles, ["r"]);
        // Role-member searches don't touch this page.
        p.on_result(
            &AdminAction::SearchMembers {
                guild_id: g,
                query: "x".into(),
                purpose: SearchFor::RoleMembers,
            },
            &Ok(AdminOutcome::Json(json!([]))),
        );
        assert_eq!(p.list.get().unwrap().len(), 1);
    }

    #[test]
    fn bans() {
        let v = json!([
            { "user": { "id": "1", "username": "spammer", "global_name": "Spam Bot" }, "reason": "ads" },
            { "user": { "id": "2", "username": "troll" }, "reason": null },
            { "reason": "no user" }
        ]);
        let rows = BanRow::parse_list(&v);
        assert_eq!(rows.len(), 2);
        assert!(rows[0].matches("spam"));
        assert!(rows[0].matches("ADS"));
        assert!(rows[1].matches("2"));
        assert!(!rows[1].matches("ads"));
        assert!(rows[1].matches(""));

        let mut page = BansPage::default();
        let g = "g".to_string();
        page.on_result(
            &AdminAction::LoadBans {
                guild_id: g.clone(),
                after: None,
            },
            &Ok(AdminOutcome::Json(v)),
        );
        page.on_result(
            &AdminAction::Unban {
                guild_id: g,
                user_id: "1".into(),
            },
            &Ok(AdminOutcome::Json(Value::Null)),
        );
        assert_eq!(page.list.get().unwrap().len(), 1);
        assert!(!page.more);
    }
}
