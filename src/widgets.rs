//! Small Discord-style widgets shared by several screens: menu rows
//! (context menus, the status picker, popups), separators and icon buttons.
//! Colours always come from the `Palette`.

use eframe::egui::{self, Color32, vec2};

use crate::icons;
use crate::theme::{self, Palette};

/// Height of a menu row (Discord: 32 px).
pub const MENU_ROW: f32 = 32.0;

/// How a menu row looks.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub enum Tone {
    #[default]
    Normal,
    /// Destructive (Delete, Leave Server): red text, red hover.
    Danger,
    /// Greyed out and not clickable.
    Disabled,
}

/// A full-width menu row: label on the left, optional icon on the right
/// (like Discord's context menus), accent background on hover. The whole
/// row is clickable. Closes the surrounding popup when clicked.
pub fn menu_item(
    ui: &mut egui::Ui,
    icon: Option<&str>,
    text: &str,
    tone: Tone,
    pal: &Palette,
) -> egui::Response {
    let sense = if tone == Tone::Disabled {
        egui::Sense::hover()
    } else {
        egui::Sense::click()
    };
    let w = ui.available_width().clamp(180.0, 240.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, MENU_ROW), sense);
    let hovered = resp.hovered() && tone != Tone::Disabled;
    let (bg, fg) = match (tone, hovered) {
        (Tone::Disabled, _) => (None, pal.muted_text()),
        (Tone::Danger, true) => (Some(pal.danger), Color32::WHITE),
        (Tone::Danger, false) => (None, pal.danger),
        (Tone::Normal, true) => (Some(pal.accent), Color32::WHITE),
        (Tone::Normal, false) => (None, pal.text),
    };
    if let Some(bg) = bg {
        ui.painter().rect_filled(rect, 4, bg);
    }
    let galley = crate::app::truncated(
        ui,
        text,
        egui::FontId::proportional(14.0),
        fg,
        rect.width() - if icon.is_some() { 44.0 } else { 16.0 },
    );
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.center().y - galley.size().y / 2.0),
        galley,
        fg,
    );
    if let Some(icon) = icon {
        ui.painter().text(
            egui::pos2(rect.right() - 10.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            icon,
            icons::font(18.0),
            fg,
        );
    }
    if resp.clicked() {
        ui.close();
    }
    if tone == Tone::Disabled {
        resp
    } else {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    }
}

/// A menu row with a coloured dot in front (status picker).
pub fn dot_item(
    ui: &mut egui::Ui,
    dot: Color32,
    text: &str,
    selected: bool,
    pal: &Palette,
) -> egui::Response {
    let w = ui.available_width().clamp(180.0, 240.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(w, MENU_ROW), egui::Sense::click());
    let hovered = resp.hovered();
    if hovered {
        ui.painter().rect_filled(rect, 4, pal.accent);
    } else if selected {
        ui.painter().rect_filled(rect, 4, pal.hover);
    }
    let fg = if hovered { Color32::WHITE } else { pal.text };
    let c = egui::pos2(rect.left() + 16.0, rect.center().y);
    ui.painter().circle_filled(c, 5.0, dot);
    let font = if selected {
        theme::bold_font(14.0)
    } else {
        egui::FontId::proportional(14.0)
    };
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        font,
        fg,
    );
    if resp.clicked() {
        ui.close();
    }
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Thin line between menu groups.
pub fn menu_separator(ui: &mut egui::Ui, pal: &Palette) {
    let w = ui.available_width().clamp(180.0, 240.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, 9.0), egui::Sense::hover());
    ui.painter().hline(
        r.shrink2(vec2(6.0, 0.0)).x_range(),
        r.center().y,
        egui::Stroke::new(1.0, pal.divider),
    );
}

/// Muted label row inside a menu (e.g. "Muted").
pub fn menu_note(ui: &mut egui::Ui, icon: &str, text: &str, pal: &Palette) {
    let (rect, _) = ui.allocate_exact_size(
        vec2(ui.available_width().clamp(180.0, 240.0), 28.0),
        egui::Sense::hover(),
    );
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("{icon}  {text}"),
        egui::FontId::proportional(13.0),
        pal.muted,
    );
}

/// A square icon button like Discord's header / user-panel icons: muted
/// line icon, brighter with a soft background on hover, strong when active.
/// `size` is the square's side (32 in headers).
pub fn icon_button(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    active: bool,
    enabled: bool,
    pal: &Palette,
) -> egui::Response {
    icon_button_sized(ui, icon, tooltip, active, enabled, 32.0, 20.0, pal)
}

#[allow(clippy::too_many_arguments)]
pub fn icon_button_sized(
    ui: &mut egui::Ui,
    icon: &str,
    tooltip: &str,
    active: bool,
    enabled: bool,
    size: f32,
    glyph: f32,
    pal: &Palette,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), egui::Sense::click());
    let hovered = resp.hovered() && enabled;
    if hovered {
        ui.painter().rect_filled(rect.shrink(2.0), 4, pal.hover);
    }
    let col = if !enabled {
        pal.muted_text()
    } else if active || hovered {
        pal.strong
    } else {
        pal.muted
    };
    icons::paint(ui, rect, icon, icons::font(glyph), col);
    let resp = if enabled {
        resp.on_hover_cursor(egui::CursorIcon::PointingHand)
    } else {
        resp
    };
    if tooltip.is_empty() {
        resp
    } else {
        resp.on_hover_text(tooltip)
    }
}

/// Frame for Discord-style popups / menus: darkest background, 8 px radius,
/// 8 px padding, soft shadow.
pub fn menu_frame(pal: &Palette) -> egui::Frame {
    egui::Frame::new()
        .fill(pal.menu_bg())
        .corner_radius(8)
        .inner_margin(8)
        .stroke(egui::Stroke::new(1.0, pal.divider.gamma_multiply(0.6)))
        .shadow(egui::Shadow {
            offset: [0, 8],
            blur: 16,
            spread: 0,
            color: Color32::from_black_alpha(if pal.dark { 100 } else { 40 }),
        })
}

/// Draw `add` with combo boxes / dropdown buttons styled as Discord's
/// select fields: the input fill, a hover fill, 4 px radius.
pub fn field_style<R>(ui: &mut egui::Ui, pal: &Palette, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        let w = &mut ui.visuals_mut().widgets;
        w.inactive.weak_bg_fill = pal.input;
        w.inactive.fg_stroke = egui::Stroke::new(1.0, pal.text);
        w.hovered.weak_bg_fill = pal.input;
        w.hovered.bg_stroke = egui::Stroke::new(1.0, pal.divider);
        w.open.weak_bg_fill = pal.input;
        w.active.weak_bg_fill = pal.input;
        ui.spacing_mut().button_padding = egui::vec2(10.0, 6.0);
        add(ui)
    })
    .inner
}
