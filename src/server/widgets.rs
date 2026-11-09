//! Small shared widgets for the server admin windows: vector icons (no
//! emoji), the settings sidebar, switches, the unsaved-changes bar and
//! confirm dialogs. Colours always come from the `Palette`.

use eframe::egui::{self, Color32, Pos2, Rect, RichText, Stroke, pos2, vec2};

use crate::theme::{Bold, Palette};

/// Vector icons drawn with painter shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Hash,
    Speaker,
    Megaphone,
    Forum,
    Stage,
    ChevronDown,
    ChevronUp,
    ChevronRight,
    Plus,
    PlusCircle,
    FolderPlus,
    PersonPlus,
    Gear,
    Bell,
    Shield,
    Pencil,
    Leave,
    Close,
    Check,
    Slash,
    Lock,
    Copy,
    Trash,
}

/// Draw `icon` centred in `rect` (roughly square) with colour `c`.
pub fn paint_icon(p: &egui::Painter, rect: Rect, icon: Icon, c: Color32) {
    let s = rect.width().min(rect.height());
    let o = rect.center();
    let st = Stroke::new((s / 10.0).clamp(1.2, 2.2), c);
    // Point in a -1..1 box around the centre.
    let at = |x: f32, y: f32| pos2(o.x + x * s / 2.0, o.y + y * s / 2.0);
    let line = |a: Pos2, b: Pos2| {
        p.line_segment([a, b], st);
    };
    match icon {
        Icon::Hash => {
            line(at(-0.25, -0.8), at(-0.45, 0.8));
            line(at(0.35, -0.8), at(0.15, 0.8));
            line(at(-0.75, -0.3), at(0.75, -0.3));
            line(at(-0.8, 0.3), at(0.7, 0.3));
        }
        Icon::Speaker => {
            let body = vec![
                at(-0.8, -0.3),
                at(-0.4, -0.3),
                at(0.1, -0.75),
                at(0.1, 0.75),
                at(-0.4, 0.3),
                at(-0.8, 0.3),
            ];
            p.add(egui::Shape::convex_polygon(body, c, Stroke::NONE));
            p.add(egui::Shape::line(
                arc(at(0.1, 0.0), s * 0.25, -0.9, 0.9),
                st,
            ));
            p.add(egui::Shape::line(
                arc(at(0.1, 0.0), s * 0.45, -0.9, 0.9),
                st,
            ));
        }
        Icon::Megaphone => {
            let body = vec![
                at(-0.75, -0.25),
                at(0.55, -0.75),
                at(0.55, 0.75),
                at(-0.75, 0.25),
            ];
            p.add(egui::Shape::closed_line(body, st));
            line(at(-0.45, 0.3), at(-0.3, 0.8));
        }
        Icon::Forum => {
            let r = Rect::from_min_max(at(-0.85, -0.7), at(0.55, 0.35));
            p.rect_stroke(r, 3.0, st, egui::StrokeKind::Middle);
            line(at(-0.5, 0.35), at(-0.6, 0.7));
            line(at(-0.1, 0.55), at(0.85, 0.55));
            line(at(0.85, -0.4), at(0.85, 0.55));
        }
        Icon::Stage => {
            p.circle_stroke(at(0.0, -0.35), s * 0.2, st);
            p.add(egui::Shape::line(
                arc(at(0.0, 0.1), s * 0.42, 0.2, 2.94),
                st,
            ));
        }
        Icon::ChevronDown => {
            p.add(egui::Shape::line(
                vec![at(-0.5, -0.2), at(0.0, 0.3), at(0.5, -0.2)],
                st,
            ));
        }
        Icon::ChevronUp => {
            p.add(egui::Shape::line(
                vec![at(-0.5, 0.2), at(0.0, -0.3), at(0.5, 0.2)],
                st,
            ));
        }
        Icon::ChevronRight => {
            p.add(egui::Shape::line(
                vec![at(-0.2, -0.5), at(0.3, 0.0), at(-0.2, 0.5)],
                st,
            ));
        }
        Icon::Plus => {
            line(at(0.0, -0.7), at(0.0, 0.7));
            line(at(-0.7, 0.0), at(0.7, 0.0));
        }
        Icon::PlusCircle => {
            p.circle_stroke(o, s * 0.45, st);
            line(at(0.0, -0.45), at(0.0, 0.45));
            line(at(-0.45, 0.0), at(0.45, 0.0));
        }
        Icon::FolderPlus => {
            let pts = vec![
                at(-0.85, -0.6),
                at(-0.3, -0.6),
                at(-0.15, -0.4),
                at(0.85, -0.4),
                at(0.85, 0.7),
                at(-0.85, 0.7),
            ];
            p.add(egui::Shape::closed_line(pts, st));
            line(at(0.0, -0.05), at(0.0, 0.45));
            line(at(-0.25, 0.2), at(0.25, 0.2));
        }
        Icon::PersonPlus => {
            p.circle_stroke(at(-0.2, -0.35), s * 0.2, st);
            p.add(egui::Shape::line(
                arc(at(-0.2, 0.75), s * 0.38, 3.4, 6.0),
                st,
            ));
            line(at(0.65, -0.2), at(0.65, 0.3));
            line(at(0.4, 0.05), at(0.9, 0.05));
        }
        Icon::Gear => {
            p.circle_stroke(o, s * 0.17, st);
            p.circle_stroke(o, s * 0.32, st);
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::FRAC_PI_4;
                let d = vec2(a.cos(), a.sin());
                line(o + d * s * 0.32, o + d * s * 0.46);
            }
        }
        Icon::Bell => {
            p.add(egui::Shape::line(
                arc(at(0.0, -0.1), s * 0.3, 3.14, 6.28),
                st,
            ));
            line(at(-0.6, -0.1), at(-0.6, 0.4));
            line(at(0.6, -0.1), at(0.6, 0.4));
            line(at(-0.8, 0.45), at(0.8, 0.45));
            line(at(-0.15, 0.7), at(0.15, 0.7));
        }
        Icon::Shield => {
            let pts = vec![
                at(0.0, -0.85),
                at(0.7, -0.55),
                at(0.6, 0.2),
                at(0.0, 0.85),
                at(-0.6, 0.2),
                at(-0.7, -0.55),
            ];
            p.add(egui::Shape::closed_line(pts, st));
        }
        Icon::Pencil => {
            line(at(-0.65, 0.65), at(0.45, -0.45));
            line(at(-0.4, 0.75), at(0.6, -0.25));
            line(at(0.45, -0.45), at(0.6, -0.25));
            line(at(-0.65, 0.65), at(-0.75, 0.8));
            line(at(-0.75, 0.8), at(-0.4, 0.75));
        }
        Icon::Leave => {
            let pts = vec![at(0.1, -0.8), at(-0.7, -0.8), at(-0.7, 0.8), at(0.1, 0.8)];
            p.add(egui::Shape::line(pts, st));
            line(at(-0.25, 0.0), at(0.85, 0.0));
            p.add(egui::Shape::line(
                vec![at(0.5, -0.35), at(0.85, 0.0), at(0.5, 0.35)],
                st,
            ));
        }
        Icon::Close => {
            line(at(-0.55, -0.55), at(0.55, 0.55));
            line(at(0.55, -0.55), at(-0.55, 0.55));
        }
        Icon::Check => {
            p.add(egui::Shape::line(
                vec![at(-0.6, 0.0), at(-0.15, 0.45), at(0.6, -0.45)],
                st,
            ));
        }
        Icon::Slash => {
            line(at(0.45, -0.6), at(-0.45, 0.6));
        }
        Icon::Lock => {
            let r = Rect::from_min_max(at(-0.6, -0.1), at(0.6, 0.8));
            p.rect_filled(r, 2.0, c);
            p.add(egui::Shape::line(
                arc(at(0.0, -0.1), s * 0.3, 3.14, 6.28),
                st,
            ));
        }
        Icon::Copy => {
            let a = Rect::from_min_max(at(-0.7, -0.7), at(0.3, 0.3));
            let b = Rect::from_min_max(at(-0.3, -0.3), at(0.7, 0.7));
            p.rect_stroke(a, 2.0, st, egui::StrokeKind::Middle);
            p.rect_stroke(b, 2.0, st, egui::StrokeKind::Middle);
        }
        Icon::Trash => {
            line(at(-0.75, -0.55), at(0.75, -0.55));
            line(at(-0.25, -0.8), at(0.25, -0.8));
            let pts = vec![
                at(-0.55, -0.55),
                at(-0.45, 0.8),
                at(0.45, 0.8),
                at(0.55, -0.55),
            ];
            p.add(egui::Shape::line(pts, st));
        }
    }
}

/// Points along a circle arc (angles in radians, y down).
fn arc(c: Pos2, r: f32, from: f32, to: f32) -> Vec<Pos2> {
    let n = 12;
    (0..=n)
        .map(|i| {
            let a = from + (to - from) * i as f32 / n as f32;
            pos2(c.x + r * a.cos(), c.y + r * a.sin())
        })
        .collect()
}

/// An icon-only button with hover background.
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: Icon,
    size: f32,
    tip: &str,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let col = if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
        pal.strong
    } else {
        pal.muted
    };
    paint_icon(ui.painter(), rect.shrink(size * 0.22), icon, col);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text(tip)
}

/// Settings sidebar entry (hover / selected background like Discord).
pub fn nav_item(
    ui: &mut egui::Ui,
    text: &str,
    selected: bool,
    color: Color32,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(vec2(ui.available_width(), 32.0), egui::Sense::click());
    if selected {
        ui.painter().rect_filled(rect, 4, pal.selected);
    } else if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let col = if color != pal.text {
        color
    } else if selected || resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    ui.painter().text(
        rect.left_center() + vec2(10.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(14.5),
        col,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Discord's round close button with "ESC" under it (vector cross).
pub fn close_button(ui: &mut egui::Ui, pal: &Palette) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(36.0, 54.0), egui::Sense::click());
    let c = pos2(rect.center().x, rect.top() + 18.0);
    let col = if resp.hovered() {
        pal.strong
    } else {
        pal.muted
    };
    ui.painter().circle_stroke(c, 17.0, Stroke::new(2.0, col));
    paint_icon(
        ui.painter(),
        Rect::from_center_size(c, vec2(14.0, 14.0)),
        Icon::Close,
        col,
    );
    ui.painter().text(
        pos2(c.x, rect.bottom() - 6.0),
        egui::Align2::CENTER_CENTER,
        "ESC",
        crate::theme::bold_font(11.5),
        pal.muted,
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
        .on_hover_text("Close (Esc)")
}

/// On/off switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, pal: &Palette) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), egui::Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off = if pal.dark { pal.selected } else { pal.offline };
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    let bg = Color32::from_rgb(
        l(off.r(), pal.online.r()),
        l(off.g(), pal.online.g()),
        l(off.b(), pal.online.b()),
    );
    ui.painter().rect_filled(rect, 11, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// A labelled switch row: title + hint on the left, switch on the right.
pub fn toggle_row(
    ui: &mut egui::Ui,
    title: &str,
    hint: &str,
    on: &mut bool,
    pal: &Palette,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            changed = toggle(ui, on, pal).changed();
            ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.label(RichText::new(title).color(pal.strong));
                if !hint.is_empty() {
                    ui.label(RichText::new(hint).size(13.0).color(pal.muted));
                }
            });
        });
    });
    changed
}

/// Small uppercase field label with spacing above.
pub fn field_label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(14.0);
    ui.label(pal.section_label(text));
    ui.add_space(2.0);
}

pub fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

pub fn divider(ui: &mut egui::Ui, pal: &Palette) {
    ui.add_space(10.0);
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.center().y, Stroke::new(1.0, pal.divider));
    ui.add_space(10.0);
}

/// Red filled button for destructive actions.
pub fn danger_button(pal: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE).bold())
        .fill(pal.danger)
        .corner_radius(4)
}

/// A combo box over `(value, label)` options. Returns true if changed.
pub fn combo<T: PartialEq + Clone>(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut T,
    options: &[(T, String)],
    width: f32,
) -> bool {
    let current = options
        .iter()
        .find(|(v, _)| v == value)
        .map(|(_, l)| l.clone())
        .unwrap_or_else(|| "Custom".into());
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .width(width)
        .selected_text(current)
        .show_ui(ui, |ui| {
            for (v, label) in options {
                if ui.selectable_label(v == value, label).clicked() && v != value {
                    *value = v.clone();
                    changed = true;
                }
            }
        });
    changed
}

/// What the unsaved-changes bar returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BarAction {
    Reset,
    Save,
}

/// Discord's floating "Careful — you have unsaved changes!" bar at the
/// bottom of `area`. `flash` turns it red (the user tried to leave).
pub fn unsaved_bar(
    ui: &mut egui::Ui,
    area: Rect,
    flash: bool,
    saving: bool,
    pal: &Palette,
) -> Option<BarAction> {
    let w = (area.width() - 40.0).min(700.0);
    let rect = Rect::from_center_size(pos2(area.center().x, area.bottom() - 40.0), vec2(w, 52.0));
    let mut out = None;
    egui::Area::new(ui.scope_id().with("unsaved_bar"))
        .order(egui::Order::Foreground)
        .fixed_pos(rect.min)
        .show(ui.ctx(), |ui| {
            let fill = if flash { pal.danger } else { pal.servers };
            egui::Frame::new()
                .fill(fill)
                .corner_radius(6)
                .inner_margin(egui::Margin::symmetric(14, 10))
                .shadow(ui.visuals().popup_shadow)
                .show(ui, |ui| {
                    ui.set_width(w - 28.0);
                    ui.horizontal_centered(|ui| {
                        let col = if flash { Color32::WHITE } else { pal.text };
                        ui.label(RichText::new("Careful — you have unsaved changes!").color(col));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let save = egui::Button::new(
                                RichText::new(if saving { "Saving…" } else { "Save Changes" })
                                    .color(Color32::WHITE)
                                    .bold(),
                            )
                            .fill(pal.online)
                            .corner_radius(4);
                            if ui.add_enabled(!saving, save).clicked() {
                                out = Some(BarAction::Save);
                            }
                            let reset =
                                egui::Button::new(RichText::new("Reset").color(col)).frame(false);
                            if ui.add_enabled(!saving, reset).clicked() {
                                out = Some(BarAction::Reset);
                            }
                        });
                    });
                });
        });
    out
}

/// A confirmation dialog. `show` returns `Some(true)` on confirm,
/// `Some(false)` on cancel / Esc / click outside.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub title: String,
    pub body: String,
    pub button: String,
}

impl Confirm {
    pub fn new(
        title: impl Into<String>,
        body: impl Into<String>,
        button: impl Into<String>,
    ) -> Self {
        Self {
            title: title.into(),
            body: body.into(),
            button: button.into(),
        }
    }

    pub fn show(&self, ctx: &egui::Context, pal: &Palette) -> Option<bool> {
        let mut out = None;
        let modal = egui::Modal::new(egui::Id::unique("rustcord_server_confirm")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(
                RichText::new(&self.title)
                    .bold()
                    .size(20.0)
                    .color(pal.strong),
            );
            ui.add_space(8.0);
            ui.label(&self.body);
            ui.add_space(16.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(danger_button(pal, &self.button)).clicked() {
                    out = Some(true);
                }
                if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                    out = Some(false);
                }
            });
        });
        if out.is_none() && modal.should_close() {
            out = Some(false);
        }
        out
    }
}

/// A one-line error in the danger colour.
pub fn error_line(ui: &mut egui::Ui, pal: &Palette, err: &Option<String>) {
    if let Some(e) = err {
        ui.label(RichText::new(e).size(13.0).color(pal.danger));
    }
}
