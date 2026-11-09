//! Channel permission overwrite editor: the roles / members list on the
//! left, Discord's grouped permission list with ✕ / / / ✓ (deny / neutral /
//! allow) switches on the right.
//!
//! The editor works on a draft copy; [`OverwriteEditor::changes`] diffs it
//! against what the channel had and gives the PUT / DELETE calls to make
//! (`PUT channels/{id}/permissions/{overwrite_id}` with
//! `{type, allow, deny}`; bits as decimal strings).

use eframe::egui::{self, Color32, RichText, vec2};
use serde_json::{Value, json};

use super::permissions::{self as p, PermState};
use super::widgets::{self, Icon};
use crate::perms::{Overwrite, Target};
use crate::theme::{Bold, Palette};
use crate::types::Guild;

/// One API call needed to save the editor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OverwriteChange {
    Put {
        id: String,
        target: Target,
        allow: u64,
        deny: u64,
    },
    Delete {
        id: String,
    },
}

impl OverwriteChange {
    pub fn id(&self) -> &str {
        match self {
            OverwriteChange::Put { id, .. } | OverwriteChange::Delete { id } => id,
        }
    }

    /// Body for the PUT (`type` 0 = role, 1 = member).
    pub fn put_body(&self) -> Value {
        match self {
            OverwriteChange::Put {
                target,
                allow,
                deny,
                ..
            } => overwrite_body(*target, *allow, *deny),
            OverwriteChange::Delete { .. } => Value::Null,
        }
    }
}

/// `PUT channels/{id}/permissions/{overwrite_id}` body.
pub fn overwrite_body(target: Target, allow: u64, deny: u64) -> Value {
    json!({
        "type": if target == Target::Member { 1 } else { 0 },
        "allow": allow.to_string(),
        "deny": deny.to_string(),
    })
}

/// What has to be sent to turn `original` into `draft`. Entries new in the
/// draft with nothing set are skipped (Discord keeps no empty overwrite for
/// someone who never had one); removed entries are deleted.
pub fn diff(original: &[Overwrite], draft: &[Overwrite]) -> Vec<OverwriteChange> {
    let mut out = Vec::new();
    for d in draft {
        let before = original.iter().find(|o| o.id == d.id);
        let unchanged = match before {
            Some(o) => o.allow == d.allow && o.deny == d.deny && o.target == d.target,
            None => d.allow == 0 && d.deny == 0,
        };
        if !unchanged {
            out.push(OverwriteChange::Put {
                id: d.id.clone(),
                target: d.target,
                allow: d.allow,
                deny: d.deny,
            });
        }
    }
    for o in original {
        if !draft.iter().any(|d| d.id == o.id) {
            out.push(OverwriteChange::Delete { id: o.id.clone() });
        }
    }
    out
}

/// The editor's state for one channel.
#[derive(Debug, Clone)]
pub struct OverwriteEditor {
    pub guild_id: String,
    pub channel_kind: u64,
    pub original: Vec<Overwrite>,
    pub draft: Vec<Overwrite>,
    /// Selected overwrite id (@everyone = guild id).
    pub selected: String,
    add_query: String,
}

impl OverwriteEditor {
    pub fn new(guild_id: &str, channel_kind: u64, overwrites: &[Overwrite]) -> Self {
        let mut e = Self {
            guild_id: guild_id.to_string(),
            channel_kind,
            original: overwrites.to_vec(),
            draft: overwrites.to_vec(),
            selected: guild_id.to_string(),
            add_query: String::new(),
        };
        e.ensure_everyone();
        e
    }

    /// @everyone is always listed (as an empty overwrite if it has none).
    fn ensure_everyone(&mut self) {
        if !self.draft.iter().any(|o| o.id == self.guild_id) {
            self.draft.insert(
                0,
                Overwrite {
                    id: self.guild_id.clone(),
                    target: Target::Role,
                    allow: 0,
                    deny: 0,
                },
            );
        }
    }

    /// Replace both copies (after saving, or a CHANNEL_UPDATE when there
    /// are no local edits).
    pub fn reset_to(&mut self, overwrites: &[Overwrite]) {
        self.original = overwrites.to_vec();
        self.draft = overwrites.to_vec();
        self.ensure_everyone();
        if !self.draft.iter().any(|o| o.id == self.selected) {
            self.selected = self.guild_id.clone();
        }
    }

    pub fn reset(&mut self) {
        let o = self.original.clone();
        self.reset_to(&o);
    }

    pub fn changes(&self) -> Vec<OverwriteChange> {
        diff(&self.original, &self.draft)
    }

    pub fn is_dirty(&self) -> bool {
        !self.changes().is_empty()
    }

    /// The list Discord will have after saving (empty new entries dropped).
    pub fn result(&self) -> Vec<Overwrite> {
        self.draft
            .iter()
            .filter(|d| d.allow != 0 || d.deny != 0 || self.original.iter().any(|o| o.id == d.id))
            .cloned()
            .collect()
    }

    pub fn add(&mut self, id: &str, target: Target) {
        if !self.draft.iter().any(|o| o.id == id) {
            self.draft.push(Overwrite {
                id: id.to_string(),
                target,
                allow: 0,
                deny: 0,
            });
        }
        self.selected = id.to_string();
    }

    /// @everyone can't be removed (only cleared).
    pub fn remove(&mut self, id: &str) {
        if id == self.guild_id {
            return;
        }
        self.draft.retain(|o| o.id != id);
        if self.selected == id {
            self.selected = self.guild_id.clone();
        }
    }

    pub fn state(&self, id: &str, bit: u64) -> PermState {
        self.draft
            .iter()
            .find(|o| o.id == id)
            .map_or(PermState::Neutral, |o| p::state_of(o.allow, o.deny, bit))
    }

    pub fn set(&mut self, id: &str, bit: u64, state: PermState) {
        if let Some(o) = self.draft.iter_mut().find(|o| o.id == id) {
            p::set_state(&mut o.allow, &mut o.deny, bit, state);
        }
    }

    /// Draw the editor. `members` are known `(user id, display name)` pairs
    /// for adding member overwrites and naming existing ones.
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        guild: &Guild,
        members: &[(String, String)],
        pal: &Palette,
    ) {
        let list_w = 200.0;
        ui.horizontal_top(|ui| {
            ui.allocate_ui(vec2(list_w, ui.available_height()), |ui| {
                ui.set_width(list_w);
                self.target_list(ui, guild, members, pal);
            });
            ui.add_space(16.0);
            ui.vertical(|ui| {
                self.perm_list(ui, guild, members, pal);
            });
        });
    }

    fn target_name(
        &self,
        o: &Overwrite,
        guild: &Guild,
        members: &[(String, String)],
    ) -> (String, Option<Color32>) {
        if o.id == self.guild_id {
            return ("@everyone".into(), None);
        }
        match o.target {
            Target::Role => guild
                .roles
                .iter()
                .find(|r| r.id == o.id)
                .map(|r| (r.name.clone(), r.color32()))
                .unwrap_or_else(|| ("Deleted role".into(), None)),
            Target::Member => members
                .iter()
                .find(|(id, _)| *id == o.id)
                .map(|(_, n)| (n.clone(), None))
                .unwrap_or_else(|| (format!("User {}", o.id), None)),
        }
    }

    fn target_list(
        &mut self,
        ui: &mut egui::Ui,
        guild: &Guild,
        members: &[(String, String)],
        pal: &Palette,
    ) {
        ui.horizontal(|ui| {
            ui.label(pal.section_label("Roles / Members"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = widgets::icon_button(ui, Icon::Plus, 20.0, "Add a role or member", pal);
                egui::Popup::menu(&add)
                    .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                    .show(|ui| self.add_popup(ui, guild, members, pal));
            });
        });
        ui.add_space(4.0);
        // @everyone first, then roles (highest first), then members.
        let mut order: Vec<Overwrite> = self.draft.clone();
        let role_rank = |id: &str| {
            guild
                .roles
                .iter()
                .position(|r| r.id == id)
                .unwrap_or(usize::MAX)
        };
        order.sort_by_key(|o| {
            (
                o.id != self.guild_id,
                o.target == Target::Member,
                role_rank(&o.id),
            )
        });
        let mut remove = None;
        for o in &order {
            let (name, color) = self.target_name(o, guild, members);
            let selected = self.selected == o.id;
            let (rect, resp) =
                ui.allocate_exact_size(vec2(ui.available_width(), 30.0), egui::Sense::click());
            let hovered = resp.hovered();
            if selected {
                ui.painter().rect_filled(rect, 4, pal.selected);
            } else if hovered {
                ui.painter().rect_filled(rect, 4, pal.hover);
            }
            let dot = rect.left_center() + vec2(12.0, 0.0);
            if o.target == Target::Role {
                ui.painter()
                    .circle_filled(dot, 5.0, color.unwrap_or(pal.muted));
            } else {
                widgets::paint_icon(
                    ui.painter(),
                    egui::Rect::from_center_size(dot, vec2(12.0, 12.0)),
                    Icon::PersonPlus,
                    pal.muted,
                );
            }
            let text_w = rect.width() - 52.0;
            let galley = ui.painter().layout(
                name,
                egui::FontId::proportional(14.5),
                if selected { pal.strong } else { pal.text },
                text_w,
            );
            ui.painter().galley(
                egui::pos2(
                    rect.left() + 26.0,
                    rect.center().y - galley.size().y.min(20.0) / 2.0,
                ),
                galley,
                pal.text,
            );
            if resp.clicked() {
                self.selected = o.id.clone();
            }
            if o.id != self.guild_id && (hovered || selected) {
                let x = egui::Rect::from_center_size(
                    rect.right_center() - vec2(14.0, 0.0),
                    vec2(18.0, 18.0),
                );
                let xr = ui.interact(
                    x,
                    ui.scope_id().with(("ow_remove", &o.id)),
                    egui::Sense::click(),
                );
                let col = if xr.hovered() { pal.danger } else { pal.muted };
                widgets::paint_icon(ui.painter(), x.shrink(3.0), Icon::Close, col);
                if xr.on_hover_text("Remove").clicked() {
                    remove = Some(o.id.clone());
                }
            }
        }
        if let Some(id) = remove {
            self.remove(&id);
        }
    }

    fn add_popup(
        &mut self,
        ui: &mut egui::Ui,
        guild: &Guild,
        members: &[(String, String)],
        pal: &Palette,
    ) {
        ui.set_width(240.0);
        ui.add(
            egui::TextEdit::singleline(&mut self.add_query)
                .hint_text("Search roles and members")
                .desired_width(f32::INFINITY),
        );
        let q = self.add_query.to_lowercase();
        let mut picked = None;
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .min_scrolled_height(260.0)
            .show(ui, |ui| {
                ui.label(pal.section_label("Roles"));
                for r in guild
                    .roles
                    .iter()
                    .filter(|r| r.id != self.guild_id && !self.draft.iter().any(|o| o.id == r.id))
                    .filter(|r| q.is_empty() || r.name.to_lowercase().contains(&q))
                {
                    let text = RichText::new(&r.name).color(r.color32().unwrap_or(pal.text));
                    if ui.selectable_label(false, text).clicked() {
                        picked = Some((r.id.clone(), Target::Role));
                    }
                }
                ui.label(pal.section_label("Members"));
                for (id, name) in members
                    .iter()
                    .filter(|(id, _)| !self.draft.iter().any(|o| o.id == *id))
                    .filter(|(_, n)| q.is_empty() || n.to_lowercase().contains(&q))
                    .take(50)
                {
                    if ui.selectable_label(false, name).clicked() {
                        picked = Some((id.clone(), Target::Member));
                    }
                }
            });
        if let Some((id, target)) = picked {
            self.add(&id, target);
            self.add_query.clear();
            ui.close();
        }
    }

    fn perm_list(
        &mut self,
        ui: &mut egui::Ui,
        guild: &Guild,
        members: &[(String, String)],
        pal: &Palette,
    ) {
        let Some(sel) = self.draft.iter().find(|o| o.id == self.selected).cloned() else {
            return;
        };
        let (name, _) = self.target_name(&sel, guild, members);
        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!("Permissions for {name}"))
                    .bold()
                    .color(pal.strong),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if (sel.allow != 0 || sel.deny != 0) && ui.small_button("Clear all").clicked() {
                    if let Some(o) = self.draft.iter_mut().find(|o| o.id == sel.id) {
                        o.allow = 0;
                        o.deny = 0;
                    }
                }
            });
        });
        let id = sel.id.clone();
        for (group, perms) in p::channel_groups(self.channel_kind) {
            widgets::field_label(ui, pal, group.channel_label());
            for info in perms {
                let state = self.state(&id, info.bit);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                        if let Some(s) = tri_state(ui, state, ("tri", &id, info.bit), pal) {
                            self.set(&id, info.bit, s);
                        }
                        ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                            ui.label(RichText::new(info.label(true)).color(pal.strong));
                            ui.add(
                                egui::Label::new(
                                    RichText::new(info.description).size(13.0).color(pal.muted),
                                )
                                .wrap(),
                            );
                        });
                    });
                });
                widgets::divider(ui, pal);
            }
        }
    }
}

/// Discord's three-way switch: red ✕ (deny), grey / (neutral), green ✓
/// (allow). Returns the clicked state if it differs.
pub fn tri_state(
    ui: &mut egui::Ui,
    state: PermState,
    id: impl std::hash::Hash + std::fmt::Debug,
    pal: &Palette,
) -> Option<PermState> {
    let cell = vec2(28.0, 24.0);
    let (rect, _) = ui.allocate_exact_size(vec2(cell.x * 3.0, cell.y), egui::Sense::hover());
    ui.painter().rect_stroke(
        rect,
        4,
        egui::Stroke::new(1.0, pal.divider),
        egui::StrokeKind::Inside,
    );
    let base = ui.scope_id().with(id);
    let mut out = None;
    for (i, (s, icon, on_col)) in [
        (PermState::Deny, Icon::Close, pal.danger),
        (PermState::Neutral, Icon::Slash, pal.selected),
        (PermState::Allow, Icon::Check, pal.online),
    ]
    .into_iter()
    .enumerate()
    {
        let r = egui::Rect::from_min_size(rect.min + vec2(cell.x * i as f32, 0.0), cell);
        let resp = ui.interact(r, base.with(i), egui::Sense::click());
        let on = state == s;
        if on {
            ui.painter().rect_filled(r.shrink(1.0), 3, on_col);
        } else if resp.hovered() {
            ui.painter().rect_filled(r.shrink(1.0), 3, pal.hover);
        }
        let col = if on { Color32::WHITE } else { pal.muted };
        widgets::paint_icon(
            ui.painter(),
            egui::Rect::from_center_size(r.center(), vec2(12.0, 12.0)),
            icon,
            col,
        );
        let tip = match s {
            PermState::Deny => "Deny",
            PermState::Neutral => "Inherit",
            PermState::Allow => "Allow",
        };
        if resp
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(tip)
            .clicked()
            && !on
        {
            out = Some(s);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ow(id: &str, target: Target, allow: u64, deny: u64) -> Overwrite {
        Overwrite {
            id: id.into(),
            target,
            allow,
            deny,
        }
    }

    #[test]
    fn payloads() {
        assert_eq!(
            overwrite_body(Target::Role, p::VIEW_CHANNEL, p::SEND_MESSAGES),
            json!({ "type": 0, "allow": "1024", "deny": "2048" })
        );
        assert_eq!(
            overwrite_body(Target::Member, p::BYPASS_SLOWMODE, 0),
            json!({ "type": 1, "allow": "4503599627370496", "deny": "0" })
        );
        let c = OverwriteChange::Put {
            id: "5".into(),
            target: Target::Member,
            allow: 0,
            deny: p::VIEW_CHANNEL,
        };
        assert_eq!(
            c.put_body(),
            json!({ "type": 1, "allow": "0", "deny": "1024" })
        );
        assert_eq!(c.id(), "5");
    }

    #[test]
    fn diffs() {
        let original = vec![
            ow("1", Target::Role, 0, p::VIEW_CHANNEL),
            ow("mod", Target::Role, p::VIEW_CHANNEL, 0),
            ow("u", Target::Member, p::SEND_MESSAGES, 0),
        ];
        let mut e = OverwriteEditor::new("1", 0, &original);
        assert!(!e.is_dirty());
        // Tri-state edits.
        e.set("mod", p::MANAGE_MESSAGES, PermState::Allow);
        e.set("1", p::VIEW_CHANNEL, PermState::Neutral);
        e.remove("u");
        e.remove("1"); // ignored: @everyone stays
        e.add("new", Target::Role); // empty: nothing to send
        e.add("helper", Target::Member);
        e.set("helper", p::SEND_MESSAGES, PermState::Deny);
        assert_eq!(e.state("helper", p::SEND_MESSAGES), PermState::Deny);
        assert_eq!(
            e.changes(),
            vec![
                OverwriteChange::Put {
                    id: "1".into(),
                    target: Target::Role,
                    allow: 0,
                    deny: 0
                },
                OverwriteChange::Put {
                    id: "mod".into(),
                    target: Target::Role,
                    allow: p::VIEW_CHANNEL | p::MANAGE_MESSAGES,
                    deny: 0
                },
                OverwriteChange::Put {
                    id: "helper".into(),
                    target: Target::Member,
                    allow: 0,
                    deny: p::SEND_MESSAGES
                },
                OverwriteChange::Delete { id: "u".into() },
            ]
        );
        let ids: Vec<_> = e.result().into_iter().map(|o| o.id).collect();
        assert_eq!(ids, ["1", "mod", "helper"]);
        e.reset();
        assert!(!e.is_dirty());
        assert_eq!(e.selected, "1");
    }

    #[test]
    fn everyone_always_listed() {
        let e = OverwriteEditor::new("1", 2, &[]);
        assert_eq!(e.draft.len(), 1);
        assert_eq!(e.draft[0].id, "1");
        assert!(!e.is_dirty());
        let mut e = e;
        e.set("1", p::CONNECT, PermState::Deny);
        assert_eq!(
            e.changes(),
            vec![OverwriteChange::Put {
                id: "1".into(),
                target: Target::Role,
                allow: 0,
                deny: p::CONNECT
            }]
        );
        e.set("1", p::CONNECT, PermState::Neutral);
        assert!(!e.is_dirty());
    }
}
