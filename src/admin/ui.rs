//! Small shared widgets for the server admin screens: labels, fields,
//! select boxes, buttons, list rows and the drag-to-reorder helper. Colours
//! come from the `Palette` only.

use eframe::egui::{self, RichText, vec2};

use crate::icons;
use crate::theme::{Bold, Palette, corner};
use crate::widgets;

/// A small uppercase label over a field.
pub fn label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(8.0);
    ui.label(pal.section_label(text));
}

/// Muted helper text.
pub fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

/// A thin divider line with room around it.
pub fn divider(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter().hline(
        r.x_range(),
        r.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
    ui.add_space(10.0);
}

/// One-line text field, as wide as `width` allows.
pub fn text(ui: &mut egui::Ui, value: &mut String, hint: &str, limit: usize) -> egui::Response {
    let w = ui.available_width().min(420.0);
    ui.add(
        egui::TextEdit::singleline(value)
            .hint_text(hint)
            .desired_width(w)
            .char_limit(limit),
    )
}

/// Multi-line text field.
pub fn text_area(
    ui: &mut egui::Ui,
    value: &mut String,
    hint: &str,
    limit: usize,
    rows: usize,
) -> egui::Response {
    let w = ui.available_width().min(520.0);
    ui.add(
        egui::TextEdit::multiline(value)
            .hint_text(hint)
            .desired_width(w)
            .desired_rows(rows)
            .char_limit(limit),
    )
}

/// A select box over `(value, label)` pairs. Returns true when changed.
pub fn select<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    pal: &Palette,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    options: &[(T, String)],
) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map_or("Select…", |(_, l)| l.as_str())
        .to_string();
    let mut changed = false;
    let w = ui.available_width().min(280.0);
    widgets::field_style(ui, pal, |ui| {
        egui::ComboBox::from_id_salt(id)
            .selected_text(current)
            .width(w)
            .show_ui(ui, |ui| {
                for (v, l) in options {
                    if ui.selectable_label(v == value, l.as_str()).clicked() && v != value {
                        *value = v.clone();
                        changed = true;
                    }
                }
            });
    });
    changed
}

/// `&[(T, &str)]` → owned options for [`select`].
pub fn opts<T: Clone>(list: &[(T, &str)]) -> Vec<(T, String)> {
    list.iter()
        .map(|(v, l)| (v.clone(), l.to_string()))
        .collect()
}

/// The accent "primary" button.
pub fn primary(ui: &mut egui::Ui, pal: &Palette, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(enabled, pal.primary_button(text))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The green "Save Changes" button.
pub fn save(ui: &mut egui::Ui, pal: &Palette, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(egui::Color32::WHITE).bold())
            .fill(pal.positive())
            .corner_radius(corner(4)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A red destructive button.
pub fn danger(ui: &mut egui::Ui, pal: &Palette, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(egui::Color32::WHITE).bold())
            .fill(pal.danger)
            .corner_radius(corner(4)),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A quiet secondary button.
pub fn plain(ui: &mut egui::Ui, text: &str) -> egui::Response {
    ui.button(text)
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A button with a leading icon.
pub fn icon_text(ui: &mut egui::Ui, icon: &str, text: &str) -> egui::Response {
    ui.button(format!("{icon}  {text}"))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A card: the sidebar surface with a soft radius, full width.
pub fn card<R>(ui: &mut egui::Ui, pal: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(corner(8))
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A clickable list row (hover / selected backgrounds). `add` draws the
/// contents left to right inside it.
pub fn row<R>(
    ui: &mut egui::Ui,
    pal: &Palette,
    selected: bool,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (egui::Response, R) {
    let fill_idx = ui.painter().add(egui::Shape::Noop);
    let inner = egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(10, 6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.allocate_ui_with_layout(
                vec2(ui.available_width(), 30.0),
                egui::Layout::left_to_right(egui::Align::Center),
                add,
            )
            .inner
        });
    let rect = inner.response.rect;
    let resp = ui.interact(
        rect,
        ui.scope_id().with(("admin_row", rect.min.y as i32)),
        egui::Sense::hover(),
    );
    let fill = if selected {
        Some(pal.selected)
    } else if resp.hovered() || ui.rect_contains_pointer(rect) {
        Some(pal.hover)
    } else {
        None
    };
    if let Some(fill) = fill {
        ui.painter()
            .set(fill_idx, egui::Shape::rect_filled(rect, corner(4), fill));
    }
    (resp, inner.inner)
}

/// "Nothing here" block: a big muted icon over a line of text.
pub fn empty(ui: &mut egui::Ui, pal: &Palette, icon: &str, text: &str) {
    ui.add_space(28.0);
    ui.vertical_centered(|ui| {
        ui.label(RichText::new(icon).font(icons::font(40.0)).color(pal.muted));
        ui.add_space(6.0);
        ui.label(RichText::new(text).color(pal.muted));
    });
    ui.add_space(28.0);
}

/// A "Loading…" line with a spinner.
pub fn loading(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(16.0);
    ui.horizontal(|ui| {
        ui.add(egui::Spinner::new().size(14.0).color(pal.muted));
        ui.label(RichText::new("Loading…").color(pal.muted));
    });
}

/// A sub-heading inside a page.
pub fn heading(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(6.0);
    ui.label(RichText::new(text).size(16.0).color(pal.strong).bold());
    ui.add_space(2.0);
}

/// A small coloured dot (role colours). `rgb` 0 = the muted colour.
pub fn color_dot(ui: &mut egui::Ui, pal: &Palette, rgb: u32) {
    let (r, _) = ui.allocate_exact_size(vec2(12.0, 12.0), egui::Sense::hover());
    crate::theme::shape().dot(ui.painter(), r.center(), 6.0, rgb_color(pal, rgb));
}

/// 0xRRGGBB → colour (0 = muted).
pub fn rgb_color(pal: &Palette, rgb: u32) -> egui::Color32 {
    if rgb == 0 {
        pal.muted
    } else {
        egui::Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
    }
}

/// Right-aligned contents on the rest of a horizontal row.
pub fn right<R>(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), add)
        .inner
}

/// Drag-to-reorder for a vertical list. Call [`Reorder::handle`] in each
/// row (it draws the grip), [`Reorder::row`] with the row's rect, then
/// [`Reorder::finish`] after the list: it returns `(from, to)` when a row
/// was dropped at a new place (`to` = index in the list before removal).
pub struct Reorder {
    id: egui::Id,
    rects: Vec<egui::Rect>,
}

impl Reorder {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug) -> Self {
        Self {
            id: egui::Id::unique(id),
            rects: Vec::new(),
        }
    }

    /// The grip. Dragging it starts moving row `idx`.
    pub fn handle(&mut self, ui: &mut egui::Ui, idx: usize, enabled: bool, pal: &Palette) {
        let (rect, resp) = ui.allocate_exact_size(
            vec2(18.0, 20.0),
            if enabled {
                egui::Sense::drag()
            } else {
                egui::Sense::hover()
            },
        );
        let col = if !enabled {
            pal.muted_text()
        } else if resp.hovered() || resp.dragged() {
            pal.strong
        } else {
            pal.muted
        };
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icons::LIST,
            icons::font(16.0),
            col,
        );
        if enabled {
            let resp = resp
                .on_hover_cursor(egui::CursorIcon::Grab)
                .on_hover_text("Drag to reorder");
            if resp.drag_started() {
                ui.data_mut(|d| d.insert_temp(self.id, idx));
            }
        }
    }

    pub fn row(&mut self, rect: egui::Rect) {
        self.rects.push(rect);
    }

    pub fn finish(self, ui: &mut egui::Ui, pal: &Palette) -> Option<(usize, usize)> {
        let from: usize = ui.data(|d| d.get_temp(self.id))?;
        let pointer = ui.input(|i| i.pointer.interact_pos());
        let released = ui.input(|i| i.pointer.any_released() || !i.pointer.any_down());
        let to = pointer.map(|p| self.rects.iter().filter(|r| r.center().y < p.y).count());
        if let Some(to) = to {
            let y = if to < self.rects.len() {
                self.rects[to].top()
            } else {
                self.rects.last().map_or(0.0, |r| r.bottom())
            };
            if let Some(first) = self.rects.first() {
                ui.painter()
                    .hline(first.x_range(), y, egui::Stroke::new(2.0, pal.accent));
            }
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            ui.ctx().request_repaint();
        }
        if !released {
            return None;
        }
        ui.data_mut(|d| d.remove::<usize>(self.id));
        let to = to?;
        (to != from && to != from + 1).then_some((from, to))
    }
}

/// Move item `from` so it sits before what was at `to` (see [`Reorder`]).
pub fn move_item<T>(list: &mut Vec<T>, from: usize, to: usize) {
    if from >= list.len() {
        return;
    }
    let item = list.remove(from);
    let at = if to > from { to - 1 } else { to };
    list.insert(at.min(list.len()), item);
}

/// A tri-state permission cell: deny / inherit / allow.
pub fn tri_state(ui: &mut egui::Ui, pal: &Palette, allow: &mut bool, deny: &mut bool) -> bool {
    let mut changed = false;
    let cell = |ui: &mut egui::Ui, icon: &str, on: bool, col: egui::Color32, tip: &str| {
        let (rect, resp) = ui.allocate_exact_size(vec2(26.0, 22.0), egui::Sense::click());
        let fill = if on {
            col
        } else if resp.hovered() {
            pal.hover
        } else {
            pal.input
        };
        ui.painter().rect_filled(rect, corner(4), fill);
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            icon,
            icons::font(14.0),
            if on { egui::Color32::WHITE } else { pal.muted },
        );
        resp.on_hover_text(tip)
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .clicked()
    };
    ui.spacing_mut().item_spacing.x = 2.0;
    if cell(ui, icons::X, *deny, pal.danger, "Deny") {
        *deny = !*deny;
        *allow = false;
        changed = true;
    }
    if cell(
        ui,
        icons::MINUS,
        !*allow && !*deny,
        pal.muted_text(),
        "Inherit",
    ) {
        *allow = false;
        *deny = false;
        changed = true;
    }
    if cell(ui, icons::CHECK, *allow, pal.positive(), "Allow") {
        *allow = !*allow;
        *deny = false;
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn move_item_places_before_target() {
        let mut v = vec!['a', 'b', 'c', 'd'];
        move_item(&mut v, 0, 3);
        assert_eq!(v, ['b', 'c', 'a', 'd']);
        move_item(&mut v, 3, 0);
        assert_eq!(v, ['d', 'b', 'c', 'a']);
        move_item(&mut v, 1, 4);
        assert_eq!(v, ['d', 'c', 'a', 'b']);
        move_item(&mut v, 9, 0);
        assert_eq!(v.len(), 4);
    }
}
