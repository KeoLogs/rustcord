//! Small shared widgets for the user settings pages (Discord-style rows,
//! switches, confirm dialogs and vector icons). Copies of the private
//! helpers in `settings.rs` so this module stays standalone.

use eframe::egui::{self, Color32, Pos2, RichText, Stroke, pos2, vec2};

use crate::theme::{Bold, Palette};

/// Small uppercase section label.
pub fn label(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.add_space(4.0);
    ui.label(pal.section_label(text));
}

/// Muted explanatory text.
pub fn hint(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.muted));
}

/// Red error text.
pub fn error(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).size(13.0).color(pal.danger));
}

/// Full-width 1px divider.
pub fn divider(ui: &mut egui::Ui, pal: &Palette) {
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .hline(r.x_range(), r.center().y, Stroke::new(1.0, pal.divider));
}

/// Title + switch on one line, description under it, divider. Returns true
/// when the user flipped it.
pub fn switch_row(
    ui: &mut egui::Ui,
    on: &mut bool,
    title: &str,
    desc: &str,
    enabled: bool,
    pal: &Palette,
) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let col = if enabled { pal.strong } else { pal.muted };
        ui.label(RichText::new(title).size(15.0).color(col));
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            changed = toggle(ui, on, enabled, pal).changed();
        });
    });
    if !desc.is_empty() {
        ui.scope(|ui| {
            ui.set_max_width(ui.available_width() - 60.0);
            hint(ui, pal, desc);
        });
    }
    ui.add_space(10.0);
    divider(ui, pal);
    ui.add_space(10.0);
    changed
}

/// On/off switch.
pub fn toggle(ui: &mut egui::Ui, on: &mut bool, enabled: bool, pal: &Palette) -> egui::Response {
    let size = vec2(40.0, 22.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, mut resp) = ui.allocate_exact_size(size, sense);
    if enabled && resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.12);
    let off_col = Color32::from_gray(if pal.dark { 0x50 } else { 0xb0 });
    let mut bg = lerp_color(off_col, pal.online, t);
    if !enabled {
        bg = bg.gamma_multiply(0.5);
    }
    ui.painter().rect_filled(rect, 11, bg);
    let x = egui::lerp(rect.left() + 11.0..=rect.right() - 11.0, t);
    ui.painter()
        .circle_filled(pos2(x, rect.center().y), 8.0, Color32::WHITE);
    resp
}

fn lerp_color(a: Color32, b: Color32, t: f32) -> Color32 {
    let l = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(l(a.r(), b.r()), l(a.g(), b.g()), l(a.b(), b.b()))
}

/// A rounded card (sidebar colour) around `add`.
pub fn card<R>(ui: &mut egui::Ui, pal: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(pal.sidebar)
        .corner_radius(8)
        .inner_margin(12)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// A red "danger" button.
pub fn danger_button(pal: &Palette, text: &str) -> egui::Button<'static> {
    egui::Button::new(RichText::new(text.to_string()).color(Color32::WHITE).bold())
        .fill(pal.danger)
        .corner_radius(4)
}

/// What a confirm dialog decided this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    Yes,
    No,
}

/// Modal yes / no dialog. `danger` makes the confirm button red.
pub fn confirm_modal(
    ctx: &egui::Context,
    id: &str,
    pal: &Palette,
    title: &str,
    body: &str,
    yes: &str,
    danger: bool,
) -> Option<Confirm> {
    let mut out = None;
    let modal = egui::Modal::new(egui::Id::unique(("rc_us_confirm", id))).show(ctx, |ui| {
        ui.set_width(400.0);
        ui.label(RichText::new(title).bold().size(18.0).color(pal.strong));
        ui.add_space(8.0);
        ui.label(RichText::new(body).color(pal.text));
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            if ui.button("Cancel").clicked() {
                out = Some(Confirm::No);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let b = if danger {
                    danger_button(pal, yes)
                } else {
                    pal.primary_button(yes)
                };
                if ui.add(b).clicked() {
                    out = Some(Confirm::Yes);
                }
            });
        });
    });
    if out.is_none() && modal.should_close() {
        out = Some(Confirm::No);
    }
    out
}

/// A keyboard key cap ("Ctrl", "K").
pub fn key_cap(ui: &mut egui::Ui, pal: &Palette, key: &str) {
    let galley = ui.painter().layout_no_wrap(
        key.to_string(),
        egui::FontId::proportional(12.0),
        pal.strong,
    );
    let size = vec2((galley.size().x + 12.0).max(24.0), 22.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect.translate(vec2(0.0, 2.0)), 4, pal.servers);
    p.rect_filled(rect, 4, pal.input);
    p.rect_stroke(rect, 4, Stroke::new(1.0, pal.divider), egui::StrokeKind::Inside);
    p.galley(rect.center() - galley.size() / 2.0, galley, pal.strong);
}

// ---------------------------------------------------------------------------
// Vector icons (no emoji). The integrator may swap in the shared icon font.
// ---------------------------------------------------------------------------

/// Kinds of small icons drawn with painter shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Desktop,
    Phone,
    Browser,
    Check,
    Close,
    Warning,
    Link,
}

/// Allocate `size`×`size` and paint `icon` in `color`.
pub fn icon(ui: &mut egui::Ui, icon: Icon, size: f32, color: Color32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    paint_icon(ui.painter(), rect, icon, color);
    resp
}

pub fn paint_icon(p: &egui::Painter, rect: egui::Rect, icon: Icon, color: Color32) {
    let s = rect.width();
    let w = (s / 12.0).max(1.2);
    let stroke = Stroke::new(w, color);
    let at = |x: f32, y: f32| -> Pos2 { pos2(rect.left() + x * s, rect.top() + y * s) };
    match icon {
        Icon::Desktop => {
            let screen = egui::Rect::from_min_max(at(0.08, 0.15), at(0.92, 0.7));
            p.rect_stroke(screen, 2, stroke, egui::StrokeKind::Inside);
            p.line_segment([at(0.5, 0.7), at(0.5, 0.85)], stroke);
            p.line_segment([at(0.3, 0.87), at(0.7, 0.87)], stroke);
        }
        Icon::Phone => {
            let body = egui::Rect::from_min_max(at(0.28, 0.06), at(0.72, 0.94));
            p.rect_stroke(body, 3, stroke, egui::StrokeKind::Inside);
            p.line_segment([at(0.44, 0.82), at(0.56, 0.82)], stroke);
        }
        Icon::Browser => {
            let c = rect.center();
            p.circle_stroke(c, s * 0.42, stroke);
            p.line_segment([at(0.08, 0.5), at(0.92, 0.5)], stroke);
            p.add(egui::Shape::ellipse_stroke(c, vec2(s * 0.18, s * 0.42), stroke));
        }
        Icon::Check => {
            p.line_segment([at(0.18, 0.52), at(0.42, 0.75)], stroke);
            p.line_segment([at(0.42, 0.75), at(0.84, 0.28)], stroke);
        }
        Icon::Close => {
            p.line_segment([at(0.22, 0.22), at(0.78, 0.78)], stroke);
            p.line_segment([at(0.78, 0.22), at(0.22, 0.78)], stroke);
        }
        Icon::Warning => {
            let tri = vec![at(0.5, 0.08), at(0.94, 0.88), at(0.06, 0.88)];
            p.add(egui::Shape::closed_line(tri, stroke));
            p.line_segment([at(0.5, 0.35), at(0.5, 0.6)], stroke);
            p.circle_filled(at(0.5, 0.74), w * 0.7, color);
        }
        Icon::Link => {
            p.line_segment([at(0.35, 0.65), at(0.65, 0.35)], stroke);
            p.line_segment([at(0.45, 0.25), at(0.75, 0.25)], stroke);
            p.line_segment([at(0.75, 0.25), at(0.75, 0.55)], stroke);
            let frame = egui::Rect::from_min_max(at(0.15, 0.25), at(0.75, 0.85));
            p.line_segment([frame.left_top(), frame.left_bottom()], stroke);
            p.line_segment([frame.left_bottom(), frame.right_bottom()], stroke);
            p.line_segment([frame.left_top(), at(0.35, 0.25)], stroke);
            p.line_segment([at(0.75, 0.65), frame.right_bottom()], stroke);
        }
    }
}

/// A small icon-only button with a hover background.
pub fn icon_button(ui: &mut egui::Ui, icon: Icon, pal: &Palette, tip: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
    let col = if resp.hovered() {
        ui.painter().rect_filled(rect, 4, pal.hover);
        pal.strong
    } else {
        pal.muted
    };
    paint_icon(ui.painter(), rect.shrink(5.0), icon, col);
    resp.on_hover_text(tip)
}

/// Bold heading for a page block.
pub fn heading(ui: &mut egui::Ui, pal: &Palette, text: &str) {
    ui.label(RichText::new(text).bold().size(16.0).color(pal.strong));
}
