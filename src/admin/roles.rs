//! Server settings → Roles: create, edit, delete and reorder roles
//! (colour, hoist, mentionable, icon), the full permission grid with
//! descriptions, and giving a role to / taking it from members.
//!
//! Hierarchy: only roles below my highest role can be edited, moved or
//! assigned, and only permissions I have myself can be toggled (the owner
//! is exempt), as Discord enforces.

use std::sync::Arc;

use eframe::egui::{self, RichText};
use serde_json::{Map, Value, json};

use super::overview::ImageEdit;
use super::{Confirm, Cx, Kind, Method, Out, PickSlot, Picked, Request, b, n, s, ui as aui};
use crate::icons;
use crate::perms;
use crate::theme::Bold;

/// Discord's role colour presets.
const SWATCHES: &[u32] = &[
    0x1abc9c, 0x2ecc71, 0x3498db, 0x9b59b6, 0xe91e63, 0xf1c40f, 0xe67e22, 0xe74c3c, 0x95a5a6,
    0x607d8b, 0x11806a, 0x1f8b4c, 0x206694, 0x71368a, 0xad1457, 0xc27c0e, 0xa84300, 0x992d22,
];

#[derive(Clone, PartialEq, Default)]
pub struct Draft {
    pub name: String,
    pub color: u32,
    pub hoist: bool,
    pub mentionable: bool,
    pub permissions: u64,
    pub emoji: String,
    pub icon: ImageEdit,
}

impl Draft {
    pub fn from_json(v: &Value) -> Self {
        Self {
            name: s(v, "name").to_string(),
            color: n(v, "color") as u32,
            hoist: b(v, "hoist"),
            mentionable: b(v, "mentionable"),
            permissions: perms::parse_bits(&v["permissions"]),
            emoji: s(v, "unicode_emoji").to_string(),
            icon: ImageEdit::Keep,
        }
    }

    /// Fields that differ from `base` (everything for a new role).
    pub fn patch(&self, base: Option<&Draft>) -> Map<String, Value> {
        let mut m = Map::new();
        let differs = |f: fn(&Draft) -> Value| base.is_none_or(|b| f(b) != f(self));
        if differs(|d| json!(d.name.trim())) {
            m.insert("name".into(), json!(self.name.trim()));
        }
        if differs(|d| json!(d.color)) {
            m.insert("color".into(), json!(self.color));
        }
        if differs(|d| json!(d.hoist)) {
            m.insert("hoist".into(), json!(self.hoist));
        }
        if differs(|d| json!(d.mentionable)) {
            m.insert("mentionable".into(), json!(self.mentionable));
        }
        if differs(|d| json!(d.permissions)) {
            m.insert("permissions".into(), json!(self.permissions.to_string()));
        }
        if base.is_some_and(|b| b.emoji.trim() != self.emoji.trim())
            || (base.is_none() && !self.emoji.trim().is_empty())
        {
            let e = self.emoji.trim();
            m.insert(
                "unicode_emoji".into(),
                if e.is_empty() { Value::Null } else { json!(e) },
            );
        }
        if let Some(v) = self.icon.patch() {
            m.insert("icon".into(), v);
        }
        m
    }

    pub fn valid(&self) -> bool {
        (1..=100).contains(&self.name.trim().chars().count())
    }
}

pub fn load(guild_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::LoadRoles,
        Method::Get,
        format!("/guilds/{guild_id}/roles"),
    )
}

pub fn create(guild_id: &str, draft: &Draft) -> Option<Request> {
    draft.valid().then(|| {
        Request::new(
            guild_id,
            Kind::CreateRole,
            Method::Post,
            format!("/guilds/{guild_id}/roles"),
        )
        .body(Value::Object(draft.patch(None)))
    })
}

pub fn save(guild_id: &str, role_id: &str, base: &Draft, draft: &Draft) -> Option<Request> {
    let patch = draft.patch(Some(base));
    if patch.is_empty() || !draft.valid() {
        return None;
    }
    Some(
        Request::new(
            guild_id,
            Kind::SaveRole,
            Method::Patch,
            format!("/guilds/{guild_id}/roles/{role_id}"),
        )
        .key(role_id)
        .body(Value::Object(patch)),
    )
}

pub fn delete(guild_id: &str, role_id: &str) -> Request {
    Request::new(
        guild_id,
        Kind::DeleteRole,
        Method::Delete,
        format!("/guilds/{guild_id}/roles/{role_id}"),
    )
    .key(role_id)
}

/// New positions for a reordered list. `old` is `(id, position)`; `order`
/// is every role except @everyone, highest first. Only roles whose
/// position changes are sent; `None` when nothing moved.
pub fn reorder(guild_id: &str, old: &[(String, i64)], order: &[String]) -> Option<Request> {
    let count = order.len() as i64;
    let moved: Vec<Value> = order
        .iter()
        .enumerate()
        .filter_map(|(i, id)| {
            let pos = count - i as i64;
            let was = old.iter().find(|(o, _)| o == id)?.1;
            (was != pos).then(|| json!({ "id": id, "position": pos }))
        })
        .collect();
    if moved.is_empty() {
        return None;
    }
    Some(
        Request::new(
            guild_id,
            Kind::MoveRoles,
            Method::Patch,
            format!("/guilds/{guild_id}/roles"),
        )
        .body(Value::Array(moved)),
    )
}

/// May this order be saved? Roles at or above my highest role must stay
/// exactly where they were (the owner may move anything).
pub fn order_allowed(old: &[(String, i64)], order: &[String], owner: bool, top: i64) -> bool {
    if owner {
        return true;
    }
    let count = order.len() as i64;
    order.iter().enumerate().all(|(i, id)| {
        let pos = count - i as i64;
        match old.iter().find(|(o, _)| o == id) {
            Some((_, was)) => *was == pos || (*was < top && pos < top),
            None => false,
        }
    })
}

pub fn member_search(guild_id: &str, kind: Kind, query: &str) -> Option<Request> {
    let q = query.trim();
    (!q.is_empty()).then(|| {
        Request::new(
            guild_id,
            kind,
            Method::Get,
            format!(
                "/guilds/{guild_id}/members/search?query={}&limit=25",
                crate::emoji_picker::percent_encode(q)
            ),
        )
    })
}

/// Give (`add`) or take a role. The reply key is `user:role`.
pub fn member_role(guild_id: &str, user_id: &str, role_id: &str, add: bool) -> Request {
    Request::new(
        guild_id,
        if add {
            Kind::AddMemberRole
        } else {
            Kind::RemoveMemberRole
        },
        if add { Method::Put } else { Method::Delete },
        format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"),
    )
    .key(&format!("{user_id}:{role_id}"))
}

/// Apply an Add / RemoveMemberRole reply to a list of member objects.
pub fn apply_member_role(members: &mut [Value], key: &str, add: bool) {
    let Some((uid, rid)) = key.split_once(':') else {
        return;
    };
    for m in members.iter_mut() {
        if m.pointer("/user/id").and_then(Value::as_str) != Some(uid) {
            continue;
        }
        let Some(roles) = m.get_mut("roles").and_then(Value::as_array_mut) else {
            continue;
        };
        roles.retain(|r| r.as_str() != Some(rid));
        if add {
            roles.push(json!(rid));
        }
    }
}

/// A member object's display name: nick, global name, then username.
pub fn member_name(m: &Value) -> String {
    let nick = s(m, "nick");
    if !nick.is_empty() {
        return nick.to_string();
    }
    let u = &m["user"];
    let global = s(u, "global_name");
    if global.is_empty() {
        s(u, "username").to_string()
    } else {
        global.to_string()
    }
}

pub fn member_has_role(m: &Value, role_id: &str) -> bool {
    m["roles"]
        .as_array()
        .is_some_and(|r| r.iter().any(|x| x.as_str() == Some(role_id)))
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Display,
    Permissions,
    Members,
}

struct Edit {
    /// `None` = a role being created.
    id: Option<String>,
    base: Draft,
    draft: Draft,
    page: Page,
    filter: String,
    has_icon: bool,
}

#[derive(Default)]
pub struct State {
    /// Highest first.
    roles: Option<Vec<Value>>,
    /// A drag-reordered list not saved yet (ids, highest first, no @everyone).
    order: Option<Vec<String>>,
    edit: Option<Edit>,
    query: String,
    found: Option<Vec<Value>>,
}

fn sort(roles: &mut [Value]) {
    roles.sort_by_key(|r| std::cmp::Reverse(r["position"].as_i64().unwrap_or(0)));
}

impl State {
    pub fn on_response(&mut self, kind: Kind, key: &str, v: &Value) {
        match kind {
            Kind::LoadRoles => {
                let mut roles = super::list(v);
                sort(&mut roles);
                self.roles = Some(roles);
                self.order = None;
            }
            Kind::CreateRole | Kind::SaveRole => {
                if let Some(roles) = &mut self.roles {
                    super::upsert(roles, v);
                    sort(roles);
                }
                self.edit = None;
            }
            Kind::DeleteRole => {
                if let Some(roles) = &mut self.roles {
                    super::remove(roles, key);
                }
                self.edit = None;
            }
            Kind::MoveRoles => {
                if let Some(roles) = &mut self.roles {
                    for r in v.as_array().into_iter().flatten() {
                        super::upsert(roles, r);
                    }
                    sort(roles);
                }
                self.order = None;
            }
            Kind::RoleMemberSearch => self.found = Some(super::list(v)),
            Kind::AddMemberRole | Kind::RemoveMemberRole => {
                if let Some(found) = &mut self.found {
                    apply_member_role(found, key, kind == Kind::AddMemberRole);
                }
            }
            _ => {}
        }
    }

    pub fn on_picked(&mut self, p: Arc<Picked>) {
        if let Some(e) = &mut self.edit {
            e.draft.icon = ImageEdit::New(p);
        }
    }

    pub fn ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        if self.roles.is_none() {
            aui::loading(ui, cx.pal);
            return;
        }
        if self.edit.is_some() {
            self.edit_ui(ui, cx, out);
        } else {
            self.list_ui(ui, cx, out);
        }
    }

    fn list_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(roles) = &self.roles else { return };
        aui::hint(
            ui,
            pal,
            "Roles are listed highest first. Members use the colour of their highest role. \
             Drag a role by its grip to reorder, then save the order.",
        );
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            if aui::primary(ui, pal, "Create Role", true).clicked() {
                let draft = Draft {
                    name: "new role".into(),
                    ..Default::default()
                };
                self.edit = Some(Edit {
                    id: None,
                    base: draft.clone(),
                    draft,
                    page: Page::Display,
                    filter: String::new(),
                    has_icon: false,
                });
            }
        });
        ui.add_space(6.0);

        let everyone = &cx.guild.id;
        let old: Vec<(String, i64)> = roles
            .iter()
            .filter(|r| s(r, "id") != everyone)
            .map(|r| (s(r, "id").to_string(), r["position"].as_i64().unwrap_or(0)))
            .collect();
        let order: Vec<String> = self
            .order
            .clone()
            .unwrap_or_else(|| old.iter().map(|(id, _)| id.clone()).collect());

        let mut reorder = aui::Reorder::new("admin_roles_order");
        let mut open = None;
        for (i, id) in order.iter().enumerate() {
            let Some(r) = roles.iter().find(|r| s(r, "id") == id) else {
                continue;
            };
            let pos = r["position"].as_i64().unwrap_or(0);
            let editable = perms::outranks(cx.owner, cx.top, pos);
            let (resp, clicked) = aui::row(ui, pal, false, |ui| {
                reorder.handle(ui, i, editable && !b(r, "managed"), pal);
                aui::color_dot(ui, pal, n(r, "color") as u32);
                ui.label(RichText::new(s(r, "name")).color(pal.strong));
                if b(r, "managed") {
                    ui.label(
                        RichText::new("managed by an app")
                            .size(12.0)
                            .color(pal.muted),
                    );
                }
                aui::right(ui, |ui| {
                    if editable {
                        crate::widgets::icon_button(
                            ui,
                            icons::PENCIL,
                            "Edit role",
                            false,
                            true,
                            pal,
                        )
                        .clicked()
                    } else {
                        ui.label(
                            RichText::new(icons::LOCK)
                                .font(icons::font(16.0))
                                .color(pal.muted),
                        )
                        .on_hover_text("This role is not below your highest role");
                        false
                    }
                })
            });
            reorder.row(resp.rect);
            if clicked {
                open = Some(r.clone());
            }
        }
        if let Some((from, to)) = reorder.finish(ui, pal) {
            let mut next = order.clone();
            aui::move_item(&mut next, from, to);
            if order_allowed(&old, &next, cx.owner, cx.top) {
                self.order = Some(next);
            }
        }
        // @everyone: always last, never moved.
        if let Some(r) = roles.iter().find(|r| s(r, "id") == everyone) {
            let (_, clicked) = aui::row(ui, pal, false, |ui| {
                ui.add_space(22.0);
                aui::color_dot(ui, pal, 0);
                ui.label(RichText::new("@everyone").color(pal.strong));
                ui.label(
                    RichText::new("default permissions")
                        .size(12.0)
                        .color(pal.muted),
                );
                aui::right(ui, |ui| {
                    crate::widgets::icon_button(
                        ui,
                        icons::PENCIL,
                        "Edit default permissions",
                        false,
                        true,
                        pal,
                    )
                    .clicked()
                })
            });
            if clicked {
                open = Some(r.clone());
            }
        }
        if let Some(r) = open {
            let draft = Draft::from_json(&r);
            self.edit = Some(Edit {
                id: Some(s(&r, "id").to_string()),
                base: draft.clone(),
                draft,
                page: if s(&r, "id") == everyone {
                    Page::Permissions
                } else {
                    Page::Display
                },
                filter: String::new(),
                has_icon: !s(&r, "icon").is_empty(),
            });
            self.found = None;
            self.query.clear();
        }

        if let Some(next) = &self.order {
            aui::divider(ui, pal);
            let mut reset = false;
            ui.horizontal(|ui| {
                if aui::save(ui, pal, "Save Order", true).clicked() {
                    match reorder_request(&cx.guild.id, &old, next) {
                        Some(req) => out.send.push(req),
                        None => reset = true,
                    }
                }
                if aui::plain(ui, "Reset").clicked() {
                    reset = true;
                }
                ui.label(RichText::new("The new order isn't saved yet.").color(pal.muted));
            });
            if reset {
                self.order = None;
            }
        }
    }

    fn edit_ui(&mut self, ui: &mut egui::Ui, cx: &Cx, out: &mut Out) {
        let pal = cx.pal;
        let Some(e) = &mut self.edit else { return };
        let everyone = e.id.as_deref() == Some(cx.guild.id.as_str());
        let mut back = false;
        ui.horizontal(|ui| {
            if aui::icon_text(ui, icons::CARET_LEFT, "Roles").clicked() {
                back = true;
            }
            aui::color_dot(ui, pal, e.draft.color);
            let title = if everyone {
                "@everyone".to_string()
            } else if e.id.is_none() {
                "New role".to_string()
            } else {
                e.base.name.clone()
            };
            ui.label(RichText::new(title).size(16.0).color(pal.strong).bold());
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let mut tab = |ui: &mut egui::Ui, p: Page, label: &str| {
                if ui.selectable_label(e.page == p, label).clicked() {
                    e.page = p;
                }
            };
            if !everyone {
                tab(ui, Page::Display, "Display");
            }
            tab(ui, Page::Permissions, "Permissions");
            if !everyone && e.id.is_some() {
                tab(ui, Page::Members, "Members");
            }
        });
        aui::divider(ui, pal);

        match e.page {
            Page::Display => display_ui(ui, cx, e, out),
            Page::Permissions => permissions_ui(ui, cx, &mut e.draft.permissions, &mut e.filter),
            Page::Members => {
                let role_id = e.id.clone().unwrap_or_default();
                members_ui(ui, cx, &role_id, &mut self.query, &self.found, out);
            }
        }
        let Some(e) = &mut self.edit else { return };

        if e.page != Page::Members {
            aui::divider(ui, pal);
            let dirty = e.draft != e.base || e.id.is_none();
            ui.horizontal(|ui| {
                let label = if e.id.is_none() {
                    "Create Role"
                } else {
                    "Save Changes"
                };
                if aui::save(ui, pal, label, dirty && e.draft.valid()).clicked() {
                    let req = match &e.id {
                        Some(id) => save(&cx.guild.id, id, &e.base, &e.draft),
                        None => create(&cx.guild.id, &e.draft),
                    };
                    out.send.extend(req);
                }
                if e.id.is_some() && e.draft != e.base && aui::plain(ui, "Reset").clicked() {
                    e.draft = e.base.clone();
                }
                if let Some(id) = e.id.as_ref().filter(|_| !everyone) {
                    aui::right(ui, |ui| {
                        if aui::danger(ui, pal, "Delete Role", true).clicked() {
                            out.confirm = Some(
                                Confirm::new(
                                    "Delete role",
                                    &format!(
                                        "Delete the role \"{}\"? Members lose it and its permissions. This can't be undone.",
                                        e.base.name
                                    ),
                                    "Delete Role",
                                    delete(&cx.guild.id, id),
                                )
                                .typed(&e.base.name)
                                .reason(),
                            );
                        }
                    });
                }
            });
        }
        if back {
            self.edit = None;
        }
    }
}

fn reorder_request(guild_id: &str, old: &[(String, i64)], next: &[String]) -> Option<Request> {
    reorder(guild_id, old, next)
}

fn display_ui(ui: &mut egui::Ui, cx: &Cx, e: &mut Edit, out: &mut Out) {
    let pal = cx.pal;
    let d = &mut e.draft;
    aui::label(ui, pal, "Role name");
    aui::text(ui, &mut d.name, "Role name", 100);

    aui::label(ui, pal, "Role colour");
    ui.horizontal_wrapped(|ui| {
        let mut rgb = [(d.color >> 16) as u8, (d.color >> 8) as u8, d.color as u8];
        if ui.color_edit_button_srgb(&mut rgb).changed() {
            d.color = ((rgb[0] as u32) << 16) | ((rgb[1] as u32) << 8) | rgb[2] as u32;
        }
        if aui::plain(ui, "Default").clicked() {
            d.color = 0;
        }
        for c in SWATCHES {
            let (rect, resp) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), egui::Sense::click());
            ui.painter()
                .rect_filled(rect, crate::theme::corner(4), aui::rgb_color(pal, *c));
            if d.color == *c {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    icons::CHECK,
                    icons::font(14.0),
                    egui::Color32::WHITE,
                );
            }
            if resp
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .clicked()
            {
                d.color = *c;
            }
        }
    });
    ui.horizontal(|ui| {
        ui.label(RichText::new("Preview:").color(pal.muted));
        ui.label(
            RichText::new(if d.name.trim().is_empty() {
                "role"
            } else {
                d.name.trim()
            })
            .color(aui::rgb_color(pal, d.color))
            .bold(),
        );
    });

    aui::divider(ui, pal);
    crate::settings::switch_row(
        ui,
        &mut d.hoist,
        "Display role members separately from online members",
        "Members with this role get their own group in the member list.",
        pal,
    );
    crate::settings::switch_row(
        ui,
        &mut d.mentionable,
        "Allow anyone to @mention this role",
        "Members with \"Mention @everyone, @here and All Roles\" can always mention it.",
        pal,
    );

    aui::label(ui, pal, "Role icon");
    if cx.guild.features.iter().any(|f| f == "ROLE_ICONS") {
        ui.horizontal(|ui| {
            let status = match &d.icon {
                ImageEdit::New(p) => format!("{} (uploads when you save)", p.name),
                ImageEdit::Remove => "Will be removed when you save".into(),
                ImageEdit::Keep if e.has_icon => "An icon is set".into(),
                ImageEdit::Keep => "No icon".to_string(),
            };
            ui.label(RichText::new(status).color(pal.text));
            if aui::icon_text(ui, icons::UPLOAD, "Upload").clicked() {
                out.pick = Some(PickSlot::RoleIcon);
            }
            if (e.has_icon || matches!(d.icon, ImageEdit::New(_)))
                && !matches!(d.icon, ImageEdit::Remove)
                && aui::plain(ui, "Remove").clicked()
            {
                d.icon = if e.has_icon && matches!(d.icon, ImageEdit::Keep) {
                    ImageEdit::Remove
                } else {
                    ImageEdit::Keep
                };
            }
        });
        ui.horizontal(|ui| {
            ui.label(RichText::new("or an emoji:").color(pal.muted));
            ui.add(
                egui::TextEdit::singleline(&mut d.emoji)
                    .desired_width(48.0)
                    .char_limit(16),
            );
        });
    } else {
        aui::hint(
            ui,
            pal,
            "Role icons need server boost level 2. This server doesn't have them yet.",
        );
    }
}

/// The permission grid: every permission with its description and a
/// switch. Permissions I don't have myself can't be changed.
pub fn permissions_ui(ui: &mut egui::Ui, cx: &Cx, bits: &mut u64, filter: &mut String) {
    let pal = cx.pal;
    ui.horizontal(|ui| {
        ui.add(
            egui::TextEdit::singleline(filter)
                .hint_text("Search permissions")
                .desired_width(ui.available_width().min(280.0)),
        );
        if aui::plain(ui, "Clear permissions").clicked() {
            // Only the ones I'm allowed to change.
            *bits &= !cx.perms;
        }
    });
    let needle = filter.trim().to_lowercase();
    let mut group = "";
    for p in perms::PERMISSIONS {
        if !needle.is_empty()
            && !p.name.to_lowercase().contains(&needle)
            && !p.desc.to_lowercase().contains(&needle)
        {
            continue;
        }
        if p.group != group {
            group = p.group;
            aui::label(ui, pal, &format!("{group} permissions"));
            ui.add_space(4.0);
        }
        let mine = cx.owner || cx.perms & p.bit != 0;
        let mut on = *bits & p.bit != 0;
        ui.add_enabled_ui(mine, |ui| {
            ui.horizontal(|ui| {
                ui.label(RichText::new(p.name).size(15.0).color(
                    if p.bit == perms::ADMINISTRATOR {
                        pal.danger
                    } else {
                        pal.strong
                    },
                ));
                aui::right(ui, |ui| {
                    if crate::settings::toggle(ui, &mut on, pal).changed() {
                        if on {
                            *bits |= p.bit;
                        } else {
                            *bits &= !p.bit;
                        }
                    }
                });
            });
        });
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width() - 60.0);
            aui::hint(
                ui,
                pal,
                &if mine {
                    p.desc.to_string()
                } else {
                    format!(
                        "{} (You don't have this permission, so you can't change it.)",
                        p.desc
                    )
                },
            );
        });
        ui.add_space(8.0);
    }
}

fn members_ui(
    ui: &mut egui::Ui,
    cx: &Cx,
    role_id: &str,
    query: &mut String,
    found: &Option<Vec<Value>>,
    out: &mut Out,
) {
    let pal = cx.pal;
    aui::hint(
        ui,
        pal,
        "Search for a member to give them this role or take it away.",
    );
    ui.horizontal(|ui| {
        let r = ui.add(
            egui::TextEdit::singleline(query)
                .hint_text("Name or nickname")
                .desired_width(ui.available_width().min(280.0)),
        );
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if aui::primary(ui, pal, "Search", !query.trim().is_empty()).clicked() || enter {
            out.send
                .extend(member_search(&cx.guild.id, Kind::RoleMemberSearch, query));
        }
    });
    ui.add_space(6.0);
    let Some(found) = found else { return };
    if found.is_empty() {
        aui::empty(ui, pal, icons::USERS, "No members found.");
        return;
    }
    for m in found {
        let uid = m.pointer("/user/id").and_then(Value::as_str).unwrap_or("");
        let has = member_has_role(m, role_id);
        aui::row(ui, pal, false, |ui| {
            ui.label(RichText::new(member_name(m)).color(pal.strong));
            ui.label(
                RichText::new(format!("@{}", s(&m["user"], "username")))
                    .size(12.0)
                    .color(pal.muted),
            );
            aui::right(ui, |ui| {
                if has {
                    if aui::plain(ui, "Remove role").clicked() {
                        out.send
                            .push(member_role(&cx.guild.id, uid, role_id, false));
                    }
                } else if aui::primary(ui, pal, "Add role", true).clicked() {
                    out.send.push(member_role(&cx.guild.id, uid, role_id, true));
                }
            });
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn old() -> Vec<(String, i64)> {
        vec![
            ("a".into(), 4),
            ("b".into(), 3),
            ("c".into(), 2),
            ("d".into(), 1),
        ]
    }

    fn ids(s: &str) -> Vec<String> {
        s.chars().map(|c| c.to_string()).collect()
    }

    #[test]
    fn reorder_sends_only_moved_roles() {
        assert!(reorder("1", &old(), &ids("abcd")).is_none());
        let r = reorder("1", &old(), &ids("abdc")).unwrap();
        assert_eq!(r.method, Method::Patch);
        assert_eq!(r.path, "/guilds/1/roles");
        assert_eq!(
            r.body,
            Some(json!([{ "id": "d", "position": 2 }, { "id": "c", "position": 1 }]))
        );
    }

    #[test]
    fn reorder_respects_my_highest_role() {
        // My top role is b (3): only c and d may move, and only below b.
        assert!(order_allowed(&old(), &ids("abdc"), false, 3));
        assert!(!order_allowed(&old(), &ids("bacd"), false, 3));
        assert!(!order_allowed(&old(), &ids("acbd"), false, 3));
        assert!(!order_allowed(&old(), &ids("cabd"), false, 3));
        // The owner may do anything.
        assert!(order_allowed(&old(), &ids("dcba"), true, 0));
        // Unknown ids are refused.
        assert!(!order_allowed(&old(), &ids("abcx"), false, 9));
    }

    #[test]
    fn create_sends_everything_save_only_changes() {
        let mut d = Draft {
            name: "Mods".into(),
            color: 0x3498db,
            hoist: true,
            permissions: perms::KICK_MEMBERS,
            ..Default::default()
        };
        let c = create("1", &d).unwrap();
        assert_eq!(c.method, Method::Post);
        assert_eq!(
            c.body,
            Some(json!({ "name": "Mods", "color": 0x3498db, "hoist": true,
                         "mentionable": false, "permissions": "2" }))
        );
        let base = d.clone();
        assert!(save("1", "9", &base, &d).is_none());
        d.permissions |= perms::BAN_MEMBERS;
        d.emoji = "x".into();
        let p = save("1", "9", &base, &d).unwrap();
        assert_eq!(p.path, "/guilds/1/roles/9");
        assert_eq!(p.key, "9");
        assert_eq!(
            p.body,
            Some(json!({ "permissions": "6", "unicode_emoji": "x" }))
        );
        d.name = " ".into();
        assert!(save("1", "9", &base, &d).is_none());
        assert!(create("1", &d).is_none());
    }

    #[test]
    fn member_role_requests_and_replies() {
        let add = member_role("1", "u", "r", true);
        assert_eq!(add.method, Method::Put);
        assert_eq!(add.path, "/guilds/1/members/u/roles/r");
        let del = member_role("1", "u", "r", false);
        assert_eq!(del.method, Method::Delete);
        let mut members = vec![json!({ "user": { "id": "u", "username": "sam" }, "roles": [] })];
        apply_member_role(&mut members, &add.key, true);
        assert!(member_has_role(&members[0], "r"));
        apply_member_role(&mut members, &del.key, false);
        assert!(!member_has_role(&members[0], "r"));
        assert_eq!(member_name(&members[0]), "sam");
    }

    #[test]
    fn search_needs_a_query() {
        assert!(member_search("1", Kind::RoleMemberSearch, "  ").is_none());
        let r = member_search("1", Kind::RoleMemberSearch, "a b").unwrap();
        assert_eq!(r.method, Method::Get);
        assert!(r.path.ends_with("query=a%20b&limit=25"));
    }

    #[test]
    fn delete_goes_through_the_typed_confirm() {
        let c = Confirm::new("Delete role", "", "Delete Role", delete("1", "9")).typed("Mods");
        assert!(c.into_request().is_none());
        let mut c = Confirm::new("Delete role", "", "Delete Role", delete("1", "9")).typed("Mods");
        c.typed = "Mods".into();
        let r = c.into_request().unwrap();
        assert_eq!(
            (r.method, r.path.as_str()),
            (Method::Delete, "/guilds/1/roles/9")
        );
    }
}
