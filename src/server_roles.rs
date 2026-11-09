//! Server Settings → Roles: the role list (drag to reorder), create, and the
//! role editor with Discord's three tabs (Display / Permissions / Manage
//! Members), plus delete with a confirm.
//!
//! The page keeps its own copy of the roles from `GET /guilds/{g}/roles`
//! (it needs `mentionable` / `managed` / `tags`, which `members::Role`
//! doesn't keep). Gateway role events can be fed in with
//! [`RolesPage::apply_gateway`].
//!
//! Reordering sends `PATCH /guilds/{g}/roles` with only the roles whose
//! position changed ([`reorder_payload`]). A role can only be moved if it is
//! below my highest role, and only to a place that is still below it
//! (owner: anywhere above @everyone).
//!
//! Protocol notes / uncertainties:
//! - Managed roles (bots, boosters, integrations) can't be given, taken or
//!   deleted; their name is greyed out here like Discord's client does.
//! - "Manage Members" lists ids from `GET /guilds/{g}/roles/{r}/member-ids`
//!   (undocumented, what Discord's client uses, up to 100). If that fails
//!   (bots), only members found with the search box are shown.

use std::collections::HashMap;

use eframe::egui::{self, Color32, RichText, Sense, Stroke, vec2};
use serde_json::{Map, Value, json};

use crate::guild_admin::{
    self as ga, AdminAction, AdminCtx, AdminOutcome, Icon, ImageUris, Load, SearchFor,
};
use crate::members::{Role, rgb, sort_roles};
use crate::moderation::MemberRow;
use crate::permissions_table::{self as pt, Actor, PermGroup};
use crate::theme::{Bold, Palette};

/// Discord's preset role colours (two rows of ten).
pub const PRESET_COLORS: [u32; 20] = [
    0x1abc9c, 0x2ecc71, 0x3498db, 0x9b59b6, 0xe91e63, 0xf1c40f, 0xe67e22, 0xe74c3c, 0x95a5a6,
    0x607d8b, 0x11806a, 0x1f8b4c, 0x206694, 0x71368a, 0xad1457, 0xc27c0e, 0xa84300, 0x992d22,
    0x979c9f, 0x546e7a,
];
/// The swatch shown for "no colour" (Discord's default role grey).
pub const DEFAULT_SWATCH: u32 = 0x99aab5;
/// Role names: 1-100 characters.
pub const MAX_ROLE_NAME: usize = 100;

/// Why a role is managed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleKind {
    Normal,
    Bot,
    Booster,
    Integration,
    Subscription,
    Linked,
}

/// A role as the editor needs it.
#[derive(Debug, Clone, PartialEq)]
pub struct RoleInfo {
    pub role: Role,
    pub mentionable: bool,
    pub managed: bool,
    pub kind: RoleKind,
}

impl RoleInfo {
    pub fn from_json(v: &Value) -> Self {
        let tags = &v["tags"];
        let has = |k: &str| tags.get(k).is_some();
        let kind = if has("bot_id") {
            RoleKind::Bot
        } else if has("premium_subscriber") {
            RoleKind::Booster
        } else if has("subscription_listing_id") {
            RoleKind::Subscription
        } else if has("guild_connections") {
            RoleKind::Linked
        } else if has("integration_id") {
            RoleKind::Integration
        } else {
            RoleKind::Normal
        };
        Self {
            role: Role::from_json(v),
            mentionable: v["mentionable"].as_bool().unwrap_or(false),
            managed: v["managed"].as_bool().unwrap_or(false),
            kind,
        }
    }

    /// Roles highest first (@everyone last).
    pub fn parse_list(v: &Value) -> Vec<RoleInfo> {
        let mut list: Vec<RoleInfo> = v
            .as_array()
            .map(|a| a.iter().map(RoleInfo::from_json).filter(|r| !r.role.id.is_empty()).collect())
            .unwrap_or_default();
        sort_infos(&mut list);
        list
    }

    pub fn id(&self) -> &str {
        &self.role.id
    }
}

fn sort_infos(list: &mut [RoleInfo]) {
    list.sort_by(|a, b| pt::rank_cmp(&b.role, &a.role));
}

// ---------------------------------------------------------------------------
// Reordering (pure)
// ---------------------------------------------------------------------------

/// Roles that can be reordered: everything but @everyone, highest first.
fn movable<'a>(roles: &'a [RoleInfo], everyone_id: &str) -> Vec<&'a RoleInfo> {
    roles.iter().filter(|r| r.role.id != everyone_id).collect()
}

/// May I move the role at `from` to `to` (indexes in the list without
/// @everyone, highest first; `to` is its index after the move)?
pub fn can_move(actor: &Actor, roles: &[RoleInfo], from: usize, to: usize) -> bool {
    let list = movable(roles, actor.guild_id);
    if from >= list.len() || to >= list.len() || from == to {
        return false;
    }
    if !actor.can_edit_role(&list[from].role) {
        return false;
    }
    if actor.is_owner() {
        return true;
    }
    // My highest role's index: the moved role must end up below it.
    let mine = pt::highest_role(actor.roles, actor.my_roles);
    match mine.and_then(|m| list.iter().position(|r| r.role.id == m.id)) {
        Some(top) => to > top,
        None => false,
    }
}

/// `PATCH /guilds/{g}/roles` body for moving `from` to `to` (see
/// [`can_move`]): `(id, position)` of every role whose position changes.
///
/// The roles in the moved range take over each other's positions, so
/// nothing outside it changes. If those positions aren't distinct (old
/// servers can have duplicates) every role is renumbered `n..1` instead.
pub fn reorder_payload(
    roles: &[RoleInfo],
    everyone_id: &str,
    from: usize,
    to: usize,
) -> Vec<(String, i64)> {
    let list = movable(roles, everyone_id);
    if from >= list.len() || to >= list.len() || from == to {
        return Vec::new();
    }
    let mut order: Vec<&RoleInfo> = list.clone();
    let moved = order.remove(from);
    order.insert(to, moved);
    let (lo, hi) = (from.min(to), from.max(to));
    let slots: Vec<i64> = list[lo..=hi].iter().map(|r| r.role.position).collect();
    let distinct = slots.windows(2).all(|w| w[0] > w[1]) && slots.last().is_some_and(|p| *p > 0);
    let mut out = Vec::new();
    if distinct {
        for (k, r) in order.iter().enumerate().take(hi + 1).skip(lo) {
            let pos = slots[k - lo];
            if r.role.position != pos {
                out.push((r.role.id.clone(), pos));
            }
        }
    } else {
        let n = order.len() as i64;
        for (k, r) in order.iter().enumerate() {
            let pos = n - k as i64;
            if r.role.position != pos {
                out.push((r.role.id.clone(), pos));
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Draft (pure)
// ---------------------------------------------------------------------------

/// The editor's unsaved copy of a role.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RoleDraft {
    pub id: String,
    pub name: String,
    /// 0 = no colour.
    pub color: u32,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: u64,
}

impl RoleDraft {
    pub fn from_role(r: &RoleInfo) -> Self {
        Self {
            id: r.role.id.clone(),
            name: r.role.name.clone(),
            color: r.role.color,
            hoist: r.role.hoist,
            mentionable: r.mentionable,
            permissions: r.role.permissions,
        }
    }

    /// Only the fields that differ from `orig` (permissions as a string,
    /// like Discord sends them). Empty object: nothing to save.
    pub fn patch(&self, orig: &RoleInfo) -> Value {
        let mut m = Map::new();
        let name = self.name.trim();
        if name != orig.role.name && !name.is_empty() {
            m.insert("name".into(), json!(name));
        }
        if self.color != orig.role.color {
            m.insert("color".into(), json!(self.color));
        }
        if self.hoist != orig.role.hoist {
            m.insert("hoist".into(), json!(self.hoist));
        }
        if self.mentionable != orig.mentionable {
            m.insert("mentionable".into(), json!(self.mentionable));
        }
        if self.permissions != orig.role.permissions {
            m.insert("permissions".into(), json!(self.permissions.to_string()));
        }
        Value::Object(m)
    }

    pub fn is_dirty(&self, orig: &RoleInfo) -> bool {
        self.patch(orig).as_object().is_some_and(|m| !m.is_empty())
    }

    /// Problem that blocks saving, if any.
    pub fn problem(&self) -> Option<&'static str> {
        let n = self.name.trim().chars().count();
        if n == 0 {
            Some("A role needs a name.")
        } else if n > MAX_ROLE_NAME {
            Some("Role names can be at most 100 characters.")
        } else {
            None
        }
    }

    /// Flip a permission bit, but only bits I'm allowed to change.
    pub fn toggle_permission(&mut self, bit: u64, grantable: u64) {
        if grantable & bit == bit {
            self.permissions ^= bit;
        }
    }

    /// "Clear permissions": switch off everything I may change.
    pub fn clear_permissions(&mut self, grantable: u64) {
        self.permissions &= !grantable;
    }
}

// ---------------------------------------------------------------------------
// The page
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RoleTab {
    #[default]
    Display,
    Permissions,
    Members,
}

/// Drag-and-drop payload: index in the movable list.
#[derive(Debug, Clone, Copy)]
struct RoleDrag(usize);

#[derive(Default)]
pub struct RolesPage {
    roles: Load<Vec<RoleInfo>>,
    /// `roles` as plain `Role`s (hierarchy checks against fresh data).
    plain: Vec<Role>,
    filter: String,
    selected: Option<String>,
    tab: RoleTab,
    draft: Option<RoleDraft>,
    perm_filter: String,
    confirm_delete: Option<(String, String)>,
    member_query: String,
    member_ids: HashMap<String, Load<Vec<String>>>,
    /// Members found with the Manage Members search.
    found: Load<Vec<MemberRow>>,
    /// Everyone we've seen: id → member.
    known: HashMap<String, MemberRow>,
    error: Option<String>,
    uris: ImageUris,
}

impl RolesPage {
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }

    /// Open the editor for a role (e.g. from a role mention).
    pub fn select(&mut self, role_id: &str) {
        self.selected = Some(role_id.to_string());
        self.draft = None;
        self.tab = RoleTab::Display;
    }

    pub fn forget_images(&mut self, ctx: &egui::Context) {
        self.uris.forget(ctx);
    }

    /// Ids in "Manage Members" we have no name for (the caller may ask the
    /// gateway for them with op 8 and pass names through `AdminCtx::name_of`).
    pub fn unknown_member_ids(&self) -> Vec<String> {
        self.member_ids
            .values()
            .filter_map(Load::get)
            .flatten()
            .filter(|id| !self.known.contains_key(*id))
            .cloned()
            .collect()
    }

    fn set_roles(&mut self, list: Vec<RoleInfo>) {
        self.plain = list.iter().map(|r| r.role.clone()).collect();
        sort_roles(&mut self.plain);
        if let Some(sel) = &self.selected
            && !list.iter().any(|r| &r.role.id == sel)
        {
            self.selected = None;
            self.draft = None;
        }
        self.roles = Load::Loaded(list);
    }

    fn upsert(&mut self, info: RoleInfo) {
        let mut list = self.roles.get().cloned().unwrap_or_default();
        match list.iter_mut().find(|r| r.role.id == info.role.id) {
            Some(r) => *r = info,
            None => list.push(info),
        }
        sort_infos(&mut list);
        self.set_roles(list);
    }

    fn remove(&mut self, role_id: &str) {
        if let Some(list) = self.roles.get() {
            let list = list.iter().filter(|r| r.role.id != role_id).cloned().collect();
            self.set_roles(list);
        }
        self.member_ids.remove(role_id);
    }

    /// GUILD_ROLE_CREATE / GUILD_ROLE_UPDATE (`{ guild_id, role }`) and
    /// GUILD_ROLE_DELETE (`{ guild_id, role_id }`) for this server.
    pub fn apply_gateway(&mut self, event: &str, data: &Value) {
        if !matches!(self.roles, Load::Loaded(_)) {
            return;
        }
        match event {
            "GUILD_ROLE_CREATE" | "GUILD_ROLE_UPDATE" => {
                let info = RoleInfo::from_json(&data["role"]);
                if !info.role.id.is_empty() {
                    self.upsert(info);
                }
            }
            "GUILD_ROLE_DELETE" => {
                if let Some(id) = data["role_id"].as_str() {
                    self.remove(id);
                }
            }
            _ => {}
        }
    }

    pub fn on_result(&mut self, action: &AdminAction, result: &Result<AdminOutcome, String>) {
        use AdminAction::*;
        let relevant = matches!(
            action,
            LoadRoles { .. }
                | CreateRole { .. }
                | EditRole { .. }
                | ReorderRoles { .. }
                | DeleteRole { .. }
                | LoadRoleMemberIds { .. }
                | AddMemberRole { .. }
                | RemoveMemberRole { .. }
                | SearchMembers {
                    purpose: SearchFor::RoleMembers,
                    ..
                }
        );
        if !relevant {
            return;
        }
        let value = match result {
            Ok(AdminOutcome::Json(v)) => v,
            Ok(_) => return,
            Err(e) => {
                match action {
                    LoadRoles { .. } => self.roles = Load::Failed(e.clone()),
                    LoadRoleMemberIds { role_id, .. } => {
                        self.member_ids.insert(role_id.clone(), Load::Failed(e.clone()));
                    }
                    SearchMembers { .. } => self.found = Load::Failed(e.clone()),
                    _ => self.error = Some(e.clone()),
                }
                return;
            }
        };
        self.error = None;
        match action {
            LoadRoles { .. } => self.set_roles(RoleInfo::parse_list(value)),
            CreateRole { .. } => {
                let info = RoleInfo::from_json(value);
                if !info.role.id.is_empty() {
                    let id = info.role.id.clone();
                    self.upsert(info);
                    self.select(&id);
                }
            }
            EditRole { role_id, .. } => {
                let info = RoleInfo::from_json(value);
                if !info.role.id.is_empty() {
                    self.upsert(info);
                }
                if self.draft.as_ref().is_some_and(|d| &d.id == role_id) {
                    self.draft = None;
                }
            }
            ReorderRoles { .. } => {
                // The answer is the full role list.
                if value.is_array() {
                    self.set_roles(RoleInfo::parse_list(value));
                }
            }
            DeleteRole { role_id, .. } => self.remove(role_id),
            LoadRoleMemberIds { role_id, .. } => {
                let ids = value
                    .as_array()
                    .map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect())
                    .unwrap_or_default();
                self.member_ids.insert(role_id.clone(), Load::Loaded(ids));
            }
            AddMemberRole {
                user_id, role_id, ..
            }
            | RemoveMemberRole {
                user_id, role_id, ..
            } => {
                let add = matches!(action, AddMemberRole { .. });
                if let Some(Load::Loaded(ids)) = self.member_ids.get_mut(role_id) {
                    ids.retain(|i| i != user_id);
                    if add {
                        ids.insert(0, user_id.clone());
                    }
                }
                if let Some(m) = self.known.get_mut(user_id) {
                    m.roles.retain(|r| r != role_id);
                    if add {
                        m.roles.push(role_id.clone());
                    }
                }
                if let Load::Loaded(found) = &mut self.found {
                    for m in found.iter_mut().filter(|m| &m.user.id == user_id) {
                        m.roles.retain(|r| r != role_id);
                        if add {
                            m.roles.push(role_id.clone());
                        }
                    }
                }
            }
            SearchMembers { .. } => {
                let rows = MemberRow::parse_list(value);
                for m in &rows {
                    self.known.insert(m.user.id.clone(), m.clone());
                }
                self.found = Load::Loaded(rows);
            }
            _ => {}
        }
    }

    /// Draw the page.
    pub fn show(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        match &self.roles {
            Load::Idle => {
                self.roles = Load::Loading;
                return Some(AdminAction::LoadRoles { guild_id: gid });
            }
            Load::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(RichText::new("Loading roles…").color(pal.muted));
                });
                return None;
            }
            Load::Failed(e) => {
                let e = e.clone();
                ga::error_line(ui, pal, &e);
                if ui.button("Try again").clicked() {
                    self.roles = Load::Loading;
                    return Some(AdminAction::LoadRoles { guild_id: gid });
                }
                return None;
            }
            Load::Loaded(_) => {}
        }
        let plain = self.plain.clone();
        let actor = Actor {
            roles: &plain,
            ..ctx.actor
        };
        let mut action = None;

        if let Some(e) = self.error.clone() {
            ga::error_line(ui, pal, &e);
            ui.add_space(6.0);
        }

        if let Some(sel) = self.selected.clone() {
            action = self.editor(ui, ctx, &actor, &sel);
        } else {
            action = action.or(self.overview(ui, ctx, &actor));
        }

        if let Some((id, name)) = self.confirm_delete.clone() {
            match ga::confirm(
                ui.ctx(),
                pal,
                "delete_role",
                "Delete Role",
                &format!("Are you sure you want to delete the role '{name}'? This cannot be undone."),
                "Okay",
            ) {
                Some(true) => {
                    self.confirm_delete = None;
                    self.selected = None;
                    self.draft = None;
                    action = Some(AdminAction::DeleteRole {
                        guild_id: gid,
                        role_id: id,
                    });
                }
                Some(false) => self.confirm_delete = None,
                None => {}
            }
        }
        action
    }

    /// The role list.
    fn overview(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx, actor: &Actor) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let mut action = None;
        let roles = self.roles.get().cloned().unwrap_or_default();

        ui.heading("Roles");
        ui.label(
            RichText::new(
                "Use roles to group your server members and assign permissions. \
                 Members use the colour of the highest role they have.",
            )
            .color(pal.muted),
        );
        ui.add_space(10.0);

        // @everyone: default permissions.
        if let Some(everyone) = roles.iter().find(|r| r.role.id == gid) {
            let resp = egui::Frame::new()
                .fill(pal.sidebar)
                .corner_radius(6)
                .inner_margin(10)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), Sense::hover());
                        ga::paint_icon(ui.painter(), r, Icon::Shield, pal.muted);
                        ui.vertical(|ui| {
                            ui.label(RichText::new("Default Permissions").bold().color(pal.strong));
                            ui.label(
                                RichText::new("@everyone · applies to all server members")
                                    .small()
                                    .color(pal.muted),
                            );
                        });
                    });
                })
                .response
                .interact(Sense::click());
            if resp.clicked() {
                self.select(&everyone.role.id.clone());
                self.tab = RoleTab::Permissions;
            }
            ui.add_space(10.0);
        }

        ui.horizontal(|ui| {
            ga::search_box(ui, &mut self.filter, "Search Roles", 260.0, pal);
            let can_create = actor.has(pt::MANAGE_ROLES);
            if ui
                .add_enabled(can_create, pal.primary_button("Create Role"))
                .clicked()
            {
                action = Some(AdminAction::CreateRole {
                    guild_id: gid.clone(),
                    name: "new role".into(),
                    color: 0,
                });
            }
        });
        ui.add_space(8.0);

        let list = movable(&roles, &gid);
        ui.label(pal.section_label(&format!("Roles — {}", list.len())));
        ui.label(
            RichText::new("Drag roles to reorder them. You can only move roles below your highest role.")
                .small()
                .color(pal.muted),
        );
        ui.add_space(4.0);
        let filter = self.filter.trim().to_lowercase();
        let dragging = egui::DragAndDrop::has_payload_of_type::<RoleDrag>(ui.ctx());

        egui::ScrollArea::vertical()
            .id_salt("rc_roles_list")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                for (idx, info) in list.iter().enumerate() {
                    if !filter.is_empty() && !info.role.name.to_lowercase().contains(&filter) {
                        continue;
                    }
                    let editable = actor.can_edit_role(&info.role);
                    let (rect, resp) = ui.allocate_exact_size(
                        vec2(ui.available_width(), 40.0),
                        if editable && filter.is_empty() {
                            Sense::click_and_drag()
                        } else {
                            Sense::click()
                        },
                    );
                    if editable && filter.is_empty() {
                        resp.dnd_set_drag_payload(RoleDrag(idx));
                    }
                    let being_dragged = resp.dragged();
                    if ui.is_rect_visible(rect) {
                        let bg = if being_dragged {
                            pal.selected
                        } else if resp.hovered() && !dragging {
                            pal.hover
                        } else {
                            Color32::TRANSPARENT
                        };
                        ui.painter().rect_filled(rect, 4, bg);
                        let mut x = rect.left() + 6.0;
                        let cy = rect.center().y;
                        if editable {
                            ga::paint_icon(
                                ui.painter(),
                                egui::Rect::from_center_size(egui::pos2(x + 6.0, cy), vec2(12.0, 14.0)),
                                Icon::Grip,
                                pal.muted,
                            );
                        } else {
                            // Locked: above my highest role.
                            lock_icon(ui.painter(), egui::pos2(x + 6.0, cy), pal.muted);
                        }
                        x += 22.0;
                        let col = if info.role.color == 0 {
                            pal.muted
                        } else {
                            rgb(info.role.color)
                        };
                        ui.painter().circle_filled(egui::pos2(x + 7.0, cy), 7.0, col);
                        x += 22.0;
                        let galley = ui.painter().layout_no_wrap(
                            info.role.name.clone(),
                            crate::theme::bold_font(14.5),
                            pal.text,
                        );
                        let w = galley.size().x;
                        ui.painter()
                            .galley(egui::pos2(x, cy - galley.size().y / 2.0), galley, pal.text);
                        if let Some(tag) = kind_label(info.kind) {
                            ui.painter().text(
                                egui::pos2(x + w + 8.0, cy),
                                egui::Align2::LEFT_CENTER,
                                tag,
                                egui::FontId::proportional(11.5),
                                pal.muted,
                            );
                        }
                        ga::paint_icon(
                            ui.painter(),
                            egui::Rect::from_center_size(
                                egui::pos2(rect.right() - 16.0, cy),
                                vec2(14.0, 14.0),
                            ),
                            Icon::More,
                            pal.muted,
                        );
                        ui.painter().hline(
                            rect.x_range(),
                            rect.bottom() - 0.5,
                            Stroke::new(1.0, pal.divider),
                        );
                    }
                    // Drop target: insert above or below this row.
                    if let Some(p) = resp.dnd_hover_payload::<RoleDrag>() {
                        let below = ui
                            .ctx()
                            .pointer_hover_pos()
                            .is_some_and(|pos| pos.y > rect.center().y);
                        let slot = if below { idx + 1 } else { idx };
                        let to = if slot > p.0 { slot - 1 } else { slot };
                        let ok = can_move(actor, &roles, p.0, to);
                        let y = if below { rect.bottom() } else { rect.top() };
                        let col = if ok { pal.accent } else { pal.danger };
                        ui.painter()
                            .hline(rect.x_range(), y, Stroke::new(2.0, col));
                    }
                    if let Some(p) = resp.dnd_release_payload::<RoleDrag>() {
                        let below = ui
                            .ctx()
                            .pointer_interact_pos()
                            .is_some_and(|pos| pos.y > rect.center().y);
                        let slot = if below { idx + 1 } else { idx };
                        let to = if slot > p.0 { slot - 1 } else { slot };
                        if can_move(actor, &roles, p.0, to) {
                            let positions = reorder_payload(&roles, &gid, p.0, to);
                            if !positions.is_empty() {
                                action = Some(AdminAction::ReorderRoles {
                                    guild_id: gid.clone(),
                                    positions,
                                });
                            }
                        } else {
                            self.error =
                                Some("You can only move roles below your highest role.".into());
                        }
                    } else if resp.clicked() {
                        self.select(&info.role.id.clone());
                    }
                }
            });
        action
    }

    /// The role editor: role list on the left, tabs on the right.
    fn editor(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx, actor: &Actor, sel: &str) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let roles = self.roles.get().cloned().unwrap_or_default();
        let Some(orig) = roles.iter().find(|r| r.role.id == sel).cloned() else {
            self.selected = None;
            return None;
        };
        let is_everyone = orig.role.id == gid;
        let editable = actor.can_edit_role(&orig.role);
        let mut action = None;
        let draft_for_orig = self.draft.as_ref().is_some_and(|d| d.id == orig.role.id);
        if !draft_for_orig {
            self.draft = Some(RoleDraft::from_role(&orig));
        }

        ui.horizontal_top(|ui| {
            // Left: back + role list.
            ui.vertical(|ui| {
                ui.set_width(180.0);
                ui.horizontal(|ui| {
                    if ga::icon_button(ui, Icon::Back, "Back", pal, true).clicked() {
                        self.selected = None;
                        self.draft = None;
                    }
                    ui.label(RichText::new("BACK").small().color(pal.muted).bold());
                });
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .id_salt("rc_roles_side")
                    .max_height(ui.available_height() - 4.0)
                    .show(ui, |ui| {
                        for info in &roles {
                            let is_sel = info.role.id == orig.role.id;
                            let name = if info.role.id == gid {
                                "@everyone".to_string()
                            } else {
                                info.role.name.clone()
                            };
                            let resp = ui
                                .horizontal(|ui| {
                                    ga::role_dot(ui, info.role.color, pal);
                                    ui.add(
                                        egui::Label::new(RichText::new(name).color(if is_sel {
                                            pal.strong
                                        } else {
                                            pal.text
                                        }))
                                        .truncate()
                                        .sense(Sense::click()),
                                    )
                                })
                                .inner;
                            if resp.clicked() && !is_sel {
                                self.select(&info.role.id.clone());
                            }
                        }
                    });
            });
            ui.separator();
            // Right: the editor.
            ui.vertical(|ui| {
                let title = if is_everyone {
                    "@everyone".to_string()
                } else {
                    orig.role.name.clone()
                };
                ui.label(pal.section_label(&format!("Edit role — {title}")));
                if !editable {
                    ui.label(
                        RichText::new("You can't edit this role: it's at or above your highest role.")
                            .color(pal.idle),
                    );
                }
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    for (tab, label) in [
                        (RoleTab::Display, "Display"),
                        (RoleTab::Permissions, "Permissions"),
                        (RoleTab::Members, "Manage Members"),
                    ] {
                        if is_everyone && tab != RoleTab::Permissions {
                            continue;
                        }
                        let on = self.tab == tab;
                        let resp = ui.add(
                            egui::Label::new(RichText::new(label).color(if on {
                                pal.strong
                            } else {
                                pal.muted
                            }))
                            .sense(Sense::click()),
                        );
                        if on {
                            ui.painter().hline(
                                resp.rect.x_range(),
                                resp.rect.bottom() + 3.0,
                                Stroke::new(2.0, pal.accent),
                            );
                        }
                        if resp.clicked() {
                            self.tab = tab;
                        }
                        ui.add_space(12.0);
                    }
                });
                if is_everyone {
                    self.tab = RoleTab::Permissions;
                }
                ui.add_space(4.0);
                ui.separator();
                let save_bar_h = 52.0;
                egui::ScrollArea::vertical()
                    .id_salt(("rc_role_edit", self.tab as u8))
                    .max_height((ui.available_height() - save_bar_h).max(120.0))
                    .auto_shrink([false, false])
                    .show(ui, |ui| match self.tab {
                        RoleTab::Display => {
                            if let Some(a) = self.display_tab(ui, ctx, actor, &orig, editable) {
                                action = Some(a);
                            }
                        }
                        RoleTab::Permissions => self.permissions_tab(ui, ctx, actor, editable),
                        RoleTab::Members => {
                            if let Some(a) = self.members_tab(ui, ctx, actor, &orig) {
                                action = Some(a);
                            }
                        }
                    });
                // Unsaved changes bar.
                if let Some(d) = &self.draft
                    && d.is_dirty(&orig)
                {
                    let d = d.clone();
                    egui::Frame::new()
                        .fill(pal.servers)
                        .corner_radius(6)
                        .inner_margin(10)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new("Careful — you have unsaved changes!")
                                        .color(pal.text),
                                );
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        let problem = d.problem();
                                        let save = ui
                                            .add_enabled(
                                                editable && problem.is_none(),
                                                egui::Button::new(
                                                    RichText::new("Save Changes")
                                                        .color(Color32::WHITE),
                                                )
                                                .fill(pal.online)
                                                .corner_radius(4),
                                            )
                                            .on_disabled_hover_text(problem.unwrap_or(
                                                "You can't edit this role.",
                                            ));
                                        if save.clicked() {
                                            action = Some(AdminAction::EditRole {
                                                guild_id: gid.clone(),
                                                role_id: orig.role.id.clone(),
                                                patch: d.patch(&orig),
                                            });
                                        }
                                        if ui.link("Reset").clicked() {
                                            self.draft = Some(RoleDraft::from_role(&orig));
                                        }
                                    },
                                );
                            });
                        });
                }
            });
        });
        action
    }

    fn display_tab(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &AdminCtx,
        actor: &Actor,
        orig: &RoleInfo,
        editable: bool,
    ) -> Option<AdminAction> {
        let pal = ctx.pal;
        let Some(d) = &mut self.draft else {
            return None;
        };
        ui.add_space(6.0);
        ui.label(pal.section_label("Role name *"));
        let name_ok = editable && !orig.managed;
        ui.add_enabled(
            name_ok,
            egui::TextEdit::singleline(&mut d.name)
                .char_limit(MAX_ROLE_NAME)
                .desired_width(f32::INFINITY),
        );
        if orig.managed {
            ui.label(
                RichText::new("This role is managed by an integration; its name can't be changed.")
                    .small()
                    .color(pal.muted),
            );
        }
        ui.add_space(14.0);
        ui.label(pal.section_label("Role colour *"));
        ui.label(
            RichText::new("Members use the colour of the highest role they have on the roles list.")
                .small()
                .color(pal.muted),
        );
        ui.add_space(4.0);
        ui.add_enabled_ui(editable, |ui| {
            color_picker(ui, &mut d.color, pal);
        });
        ui.add_space(14.0);
        ui.separator();
        ui.add_enabled_ui(editable, |ui| {
            setting_row(
                ui,
                pal,
                "Display role members separately from online members",
                None,
                &mut d.hoist,
                true,
            );
            ui.separator();
            setting_row(
                ui,
                pal,
                "Allow anyone to @mention this role",
                Some(
                    "Note: Members with the \"Mention @everyone, @here, and All Roles\" \
                     permission will always be able to ping this role.",
                ),
                &mut d.mentionable,
                true,
            );
        });
        ui.add_space(16.0);
        if actor.can_delete_role(&orig.role, orig.managed)
            && ui.add(ga::danger_button(pal, "Delete Role")).clicked()
        {
            self.confirm_delete = Some((orig.role.id.clone(), orig.role.name.clone()));
        }
        None
    }

    fn permissions_tab(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx, actor: &Actor, editable: bool) {
        let pal = ctx.pal;
        let grantable = actor.grantable();
        let Some(d) = &mut self.draft else {
            return;
        };
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ga::search_box(ui, &mut self.perm_filter, "Search permissions", 260.0, pal);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .add_enabled(editable, egui::Button::new("Clear permissions").frame(false))
                    .clicked()
                {
                    d.clear_permissions(grantable);
                }
            });
        });
        if d.permissions & pt::ADMINISTRATOR != 0 {
            ui.add_space(6.0);
            egui::Frame::new()
                .fill(pal.idle.gamma_multiply(0.15))
                .stroke(Stroke::new(1.0, pal.idle))
                .corner_radius(4)
                .inner_margin(8)
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.label(
                        RichText::new(
                            "This role has Administrator: its members have every permission \
                             and bypass all channel overrides, whatever is switched off below.",
                        )
                        .color(pal.text),
                    );
                });
        }
        let filter = self.perm_filter.clone();
        let mut any = false;
        for group in PermGroup::ALL {
            let perms: Vec<_> = pt::in_group(group).filter(|p| p.matches(&filter)).collect();
            if perms.is_empty() {
                continue;
            }
            any = true;
            ui.add_space(14.0);
            ui.label(pal.section_label(group.label()));
            ui.add_space(4.0);
            for p in perms {
                let allowed = editable && grantable & p.value() == p.value();
                let mut on = d.permissions & p.value() != 0;
                let before = on;
                ui.horizontal(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - 56.0).max(100.0));
                        ui.label(RichText::new(p.name).bold().color(pal.text));
                        ui.add(
                            egui::Label::new(RichText::new(p.description).small().color(pal.muted))
                                .wrap(),
                        );
                        if group == PermGroup::Advanced {
                            ui.label(
                                RichText::new("Dangerous: grants every permission.")
                                    .small()
                                    .color(pal.danger),
                            );
                        }
                    });
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let r = ga::toggle(ui, &mut on, allowed, pal);
                        if !allowed && editable {
                            r.on_hover_text("You can only change permissions you have yourself.");
                        }
                    });
                });
                if on != before {
                    d.toggle_permission(p.value(), grantable);
                }
                ui.separator();
            }
        }
        if !any {
            ui.add_space(10.0);
            ui.label(RichText::new("No permissions match.").color(pal.muted));
        }
    }

    fn members_tab(&mut self, ui: &mut egui::Ui, ctx: &AdminCtx, actor: &Actor, orig: &RoleInfo) -> Option<AdminAction> {
        let pal = ctx.pal;
        let gid = ctx.guild_id().to_string();
        let rid = orig.role.id.clone();
        let mut action = None;
        if !self.member_ids.contains_key(&rid) && !ctx.is_bot {
            self.member_ids.insert(rid.clone(), Load::Loading);
            action = Some(AdminAction::LoadRoleMemberIds {
                guild_id: gid.clone(),
                role_id: rid.clone(),
            });
        }
        let can_assign = actor.can_assign_role(&orig.role, orig.managed);
        if orig.managed {
            ui.label(
                RichText::new("This role is managed by an integration and can't be given to or taken from members.")
                    .color(pal.idle),
            );
        }
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let r = ga::search_box(ui, &mut self.member_query, "Search members to add", 260.0, pal);
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if (ui.button("Search").clicked() || enter) && !self.member_query.trim().is_empty() {
                self.found = Load::Loading;
                action = Some(AdminAction::SearchMembers {
                    guild_id: gid.clone(),
                    query: self.member_query.trim().to_string(),
                    purpose: SearchFor::RoleMembers,
                });
            }
        });
        match &self.found {
            Load::Loading => {
                ui.spinner();
            }
            Load::Failed(e) => ga::error_line(ui, pal, e),
            Load::Loaded(rows) => {
                ui.add_space(6.0);
                if rows.is_empty() {
                    ui.label(RichText::new("No members found.").color(pal.muted));
                }
                for m in rows.clone() {
                    let has = m.roles.contains(&rid);
                    ui.horizontal(|ui| {
                        ga::avatar(ui, 24.0, &m.user.id, m.name(), &m.user.avatar_url, ctx, &mut self.uris);
                        ui.label(RichText::new(m.name()).color(pal.text));
                        ui.label(RichText::new(&m.user.username).small().color(pal.muted));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if has {
                                ui.label(RichText::new("Has role").small().color(pal.muted));
                            } else if ui.add_enabled(can_assign, egui::Button::new("Add")).clicked() {
                                action = Some(AdminAction::AddMemberRole {
                                    guild_id: gid.clone(),
                                    user_id: m.user.id.clone(),
                                    role_id: rid.clone(),
                                });
                            }
                        });
                    });
                }
            }
            Load::Idle => {}
        }
        ui.add_space(12.0);
        // Members with the role: ids from Discord plus anyone we found.
        let mut ids: Vec<String> = match self.member_ids.get(&rid) {
            Some(Load::Loaded(ids)) => ids.clone(),
            _ => Vec::new(),
        };
        for m in self.known.values() {
            if m.roles.contains(&rid) && !ids.contains(&m.user.id) {
                ids.push(m.user.id.clone());
            }
        }
        ui.label(pal.section_label(&format!("Members — {}", ids.len())));
        match self.member_ids.get(&rid) {
            Some(Load::Loading) => {
                ui.spinner();
            }
            Some(Load::Failed(_)) | None => {
                ui.label(
                    RichText::new("Discord didn't send this role's member list. Search above to find members.")
                        .small()
                        .color(pal.muted),
                );
            }
            _ => {}
        }
        for id in ids {
            let name = self
                .known
                .get(&id)
                .map(|m| m.name().to_string())
                .or_else(|| ctx.known_name(&id))
                .unwrap_or_else(|| format!("User {id}"));
            ui.horizontal(|ui| {
                let url = self
                    .known
                    .get(&id)
                    .map(|m| m.user.avatar_url.clone())
                    .unwrap_or_default();
                ga::avatar(ui, 24.0, &id, &name, &url, ctx, &mut self.uris);
                ui.label(RichText::new(&name).color(pal.text));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if can_assign
                        && ga::icon_button(ui, Icon::Close, "Remove member", pal, true).clicked()
                    {
                        action = Some(AdminAction::RemoveMemberRole {
                            guild_id: gid.clone(),
                            user_id: id.clone(),
                            role_id: rid.clone(),
                        });
                    }
                });
            });
        }
        action
    }
}

fn kind_label(kind: RoleKind) -> Option<&'static str> {
    match kind {
        RoleKind::Normal => None,
        RoleKind::Bot => Some("BOT"),
        RoleKind::Booster => Some("SERVER BOOSTER"),
        RoleKind::Integration => Some("INTEGRATION"),
        RoleKind::Subscription => Some("SUBSCRIPTION"),
        RoleKind::Linked => Some("LINKED ROLE"),
    }
}

/// A small padlock.
fn lock_icon(painter: &egui::Painter, c: egui::Pos2, col: Color32) {
    let body = egui::Rect::from_center_size(c + vec2(0.0, 2.5), vec2(10.0, 7.0));
    painter.rect_filled(body, 1, col);
    let s = Stroke::new(1.5, col);
    painter.line_segment([c + vec2(-3.0, -1.0), c + vec2(-3.0, -4.0)], s);
    painter.line_segment([c + vec2(3.0, -1.0), c + vec2(3.0, -4.0)], s);
    painter.line_segment([c + vec2(-3.0, -4.0), c + vec2(3.0, -4.0)], s);
}

/// A title (+ optional note) with a toggle on the right.
fn setting_row(ui: &mut egui::Ui, pal: &Palette, title: &str, note: Option<&str>, on: &mut bool, enabled: bool) {
    ui.horizontal(|ui| {
        ui.vertical(|ui| {
            ui.set_width((ui.available_width() - 56.0).max(100.0));
            ui.label(RichText::new(title).color(pal.text));
            if let Some(n) = note {
                ui.add(egui::Label::new(RichText::new(n).small().color(pal.muted)).wrap());
            }
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ga::toggle(ui, on, enabled, pal);
        });
    });
}

/// Default swatch, custom picker, then Discord's 20 presets.
pub fn color_picker(ui: &mut egui::Ui, color: &mut u32, pal: &Palette) {
    ui.horizontal(|ui| {
        let r = swatch(ui, vec2(60.0, 46.0), rgb(DEFAULT_SWATCH), *color == 0, pal);
        if r.on_hover_text("Default").clicked() {
            *color = 0;
        }
        let custom = *color != 0 && !PRESET_COLORS.contains(color);
        let mut rgb3 = if *color == 0 {
            [0x99, 0xaa, 0xb5]
        } else {
            [(*color >> 16) as u8, (*color >> 8) as u8, *color as u8]
        };
        ui.vertical(|ui| {
            let r = egui::color_picker::color_edit_button_srgb(ui, &mut rgb3);
            if r.changed() {
                *color = (u32::from(rgb3[0]) << 16) | (u32::from(rgb3[1]) << 8) | u32::from(rgb3[2]);
                // 0 would mean "no colour".
                if *color == 0 {
                    *color = 1;
                }
            }
            ui.label(
                RichText::new(if custom { "Custom (selected)" } else { "Custom" })
                    .small()
                    .color(pal.muted),
            );
        });
        ui.add_space(6.0);
        ui.vertical(|ui| {
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            for row in PRESET_COLORS.chunks(10) {
                ui.horizontal(|ui| {
                    for c in row {
                        let r = swatch(ui, vec2(20.0, 20.0), rgb(*c), *color == *c, pal);
                        if r.on_hover_text(format!("#{c:06x}")).clicked() {
                            *color = *c;
                        }
                    }
                });
            }
        });
    });
    ui.label(
        RichText::new(if *color == 0 {
            "No colour".to_string()
        } else {
            format!("#{:06x}", *color)
        })
        .small()
        .color(pal.muted),
    );
}

fn swatch(ui: &mut egui::Ui, size: egui::Vec2, col: Color32, selected: bool, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    if ui.is_rect_visible(rect) {
        ui.painter().rect_filled(rect, 4, col);
        if resp.hovered() {
            ui.painter()
                .rect_stroke(rect, 4, Stroke::new(1.5, pal.strong), egui::StrokeKind::Inside);
        }
        if selected {
            // Check mark in black or white, whichever reads on the colour.
            let lum = 0.299 * col.r() as f32 + 0.587 * col.g() as f32 + 0.114 * col.b() as f32;
            let mark = if lum > 150.0 { Color32::BLACK } else { Color32::WHITE };
            let c = rect.center();
            let s = (size.y * 0.22).min(8.0);
            let st = Stroke::new(2.0, mark);
            ui.painter()
                .line_segment([c + vec2(-s, 0.0), c + vec2(-s * 0.3, s * 0.7)], st);
            ui.painter()
                .line_segment([c + vec2(-s * 0.3, s * 0.7), c + vec2(s, -s * 0.6)], st);
        }
    }
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(id: &str, pos: i64, perms: u64) -> RoleInfo {
        RoleInfo::from_json(&json!({
            "id": id, "name": id, "position": pos, "permissions": perms.to_string()
        }))
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    /// g = @everyone (server id "g").
    fn server() -> Vec<RoleInfo> {
        let mut v = vec![
            info("g", 0, 0),
            info("a", 4, pt::ADMINISTRATOR),
            info("b", 3, pt::MANAGE_ROLES),
            info("c", 2, 0),
            info("d", 1, 0),
        ];
        sort_infos(&mut v);
        v
    }

    #[test]
    fn parse_kinds() {
        let v = json!([
            { "id": "g", "name": "@everyone", "position": 0, "permissions": "0" },
            { "id": "bot", "name": "Bot", "position": 2, "managed": true, "tags": { "bot_id": "1" } },
            { "id": "boost", "name": "Booster", "position": 1, "managed": true, "tags": { "premium_subscriber": null } },
            { "id": "m", "name": "Mods", "position": 3, "mentionable": true, "color": 0x3498db },
            { "name": "no id" }
        ]);
        let list = RoleInfo::parse_list(&v);
        let order: Vec<_> = list.iter().map(|r| r.id().to_string()).collect();
        assert_eq!(order, ["m", "bot", "boost", "g"]);
        assert_eq!(list[1].kind, RoleKind::Bot);
        assert!(list[1].managed);
        assert_eq!(list[2].kind, RoleKind::Booster);
        assert!(list[0].mentionable);
        assert_eq!(list[0].role.color, 0x3498db);
    }

    #[test]
    fn reorder_within_range() {
        let roles = server();
        // Movable list: a(4) b(3) c(2) d(1). Move d (3) to index 1 → a d b c.
        let p = reorder_payload(&roles, "g", 3, 1);
        assert_eq!(p, vec![("d".into(), 3), ("b".into(), 2), ("c".into(), 1)]);
        // Move b down to the bottom → a c d b.
        let p = reorder_payload(&roles, "g", 1, 3);
        assert_eq!(p, vec![("c".into(), 3), ("d".into(), 2), ("b".into(), 1)]);
        // Swap neighbours.
        let p = reorder_payload(&roles, "g", 2, 3);
        assert_eq!(p, vec![("d".into(), 2), ("c".into(), 1)]);
        // No-ops.
        assert!(reorder_payload(&roles, "g", 2, 2).is_empty());
        assert!(reorder_payload(&roles, "g", 9, 0).is_empty());
    }

    #[test]
    fn reorder_with_gaps_and_duplicates() {
        // Gaps keep the existing slots.
        let mut roles = vec![info("g", 0, 0), info("x", 10, 0), info("y", 7, 0), info("z", 2, 0)];
        sort_infos(&mut roles);
        let p = reorder_payload(&roles, "g", 2, 0);
        assert_eq!(p, vec![("z".into(), 10), ("x".into(), 7), ("y".into(), 2)]);
        // Duplicate positions: renumber everything n..1.
        let mut roles = vec![info("g", 0, 0), info("1", 1, 0), info("2", 1, 0), info("3", 1, 0)];
        sort_infos(&mut roles);
        // Order (same position, older id first): 1 2 3. Move 3 to the top.
        let p = reorder_payload(&roles, "g", 2, 0);
        assert_eq!(p, vec![("3".into(), 3), ("1".into(), 2)]);
    }

    #[test]
    fn move_permissions() {
        let roles = server();
        let plain: Vec<Role> = roles.iter().map(|r| r.role.clone()).collect();
        let mine = ids(&["b"]);
        let me = Actor {
            guild_id: "g",
            owner_id: "o",
            user_id: "me",
            roles: &plain,
            my_roles: &mine,
        };
        // c (2) and d (3) are below b (1).
        assert!(can_move(&me, &roles, 3, 2));
        assert!(can_move(&me, &roles, 2, 3));
        assert!(!can_move(&me, &roles, 3, 1), "can't go to my own role's slot");
        assert!(!can_move(&me, &roles, 3, 0), "can't go above my role");
        assert!(!can_move(&me, &roles, 1, 3), "can't move my own top role");
        assert!(!can_move(&me, &roles, 0, 3), "can't move a higher role");
        let none: Vec<String> = Vec::new();
        let owner = Actor {
            user_id: "o",
            my_roles: &none,
            ..me
        };
        assert!(can_move(&owner, &roles, 3, 0));
        assert!(!can_move(&owner, &roles, 3, 4), "nothing below @everyone");
    }

    #[test]
    fn draft_patch() {
        let orig = RoleInfo::from_json(&json!({
            "id": "r", "name": "Mods", "color": 0, "hoist": false,
            "mentionable": false, "permissions": "8192"
        }));
        let mut d = RoleDraft::from_role(&orig);
        assert!(!d.is_dirty(&orig));
        assert_eq!(d.patch(&orig), json!({}));
        d.name = " Moderators ".into();
        d.color = 0x3498db;
        d.hoist = true;
        assert_eq!(
            d.patch(&orig),
            json!({ "name": "Moderators", "color": 0x3498db, "hoist": true })
        );
        // Permissions: only grantable bits flip, sent as a string.
        let mut d = RoleDraft::from_role(&orig);
        d.toggle_permission(pt::KICK_MEMBERS, pt::KICK_MEMBERS);
        d.toggle_permission(pt::ADMINISTRATOR, pt::KICK_MEMBERS);
        assert_eq!(d.patch(&orig), json!({ "permissions": (8192 | 2).to_string() }));
        d.clear_permissions(pt::KICK_MEMBERS);
        assert_eq!(d.permissions, 8192, "can't clear bits I don't have");
        d.clear_permissions(u64::MAX);
        assert_eq!(d.permissions, 0);
        // Name problems.
        d.name = "  ".into();
        assert!(d.problem().is_some());
        d.name = "x".repeat(101);
        assert!(d.problem().is_some());
    }
}
