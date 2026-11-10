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

/// Widest a picture in the message list may be: `min(cap, avail - 16)`
/// (doc §3.4), never below 32 so a very narrow column still shows
/// something. Images, video previews, embeds and stickers all use it.
pub fn media_width(avail: f32, cap: f32) -> f32 {
    cap.min(avail - 16.0).max(32.0)
}

/// A one-line label that never wraps: it is cut off with "…" and shows the
/// full text as a tooltip when cut. `reserve` keeps that much room on the
/// right for whatever follows on the same line (a timestamp, a BOT tag),
/// so the name shrinks instead of pushing those out of the pane.
pub fn truncated_label(ui: &mut egui::Ui, label: egui::Label, reserve: f32) -> egui::Response {
    let max = (ui.available_width() - reserve).max(16.0);
    ui.scope(|ui| {
        ui.set_max_width(max);
        ui.add(label.truncate())
    })
    .inner
}

// ---------------------------------------------------------------------------
// Server rail buttons
// ---------------------------------------------------------------------------

/// Gap between the rail's edges and its 48 px icons (Discord: 72 px rail).
const RAIL_INSET: f32 = 12.0;
/// How far past the icon the button stretches before its pill background is
/// fully shown.
const RAIL_FADE: f32 = 48.0;
/// Width of the fade where a name is clipped at the button's right edge.
const RAIL_LABEL_FADE: f32 = 18.0;

/// Server icons are 48 px; a rail squeezed below Discord's 72 px (small
/// window, custom layout) shrinks them instead of clipping them.
pub fn rail_icon_size(width: f32) -> f32 {
    (width - 20.0).clamp(28.0, 48.0)
}

/// Where one server-rail button goes. The icon never moves (12 px from the
/// left, like the icon-only rail); as the rail is dragged wider the button
/// stretches right from a circle into a pill that fills the rail, and the
/// name is revealed beside the icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RailButton {
    /// The whole row (allocated, clickable).
    pub row: egui::Rect,
    /// The icon square.
    pub icon: egui::Rect,
    /// The pill: from the icon's left edge to the rail's right inset. Equal
    /// to `icon` in the icon-only rail.
    pub button: egui::Rect,
    /// 0 = icon only … 1 = stretched far enough to show the pill fully.
    pub widen: f32,
}

impl RailButton {
    pub fn new(row: egui::Rect) -> Self {
        let size = rail_icon_size(row.width()).min(row.height());
        let inset = ((row.width() - size) / 2.0).clamp(0.0, RAIL_INSET);
        let icon =
            egui::Rect::from_min_size(egui::pos2(row.left() + inset, row.top()), vec2(size, size));
        let right = (row.right() - inset).max(icon.right());
        let button = egui::Rect::from_x_y_ranges(icon.left()..=right, icon.y_range());
        let widen = ((button.width() - size) / RAIL_FADE).clamp(0.0, 1.0);
        Self {
            row,
            icon,
            button,
            widen,
        }
    }

    /// Allocate a full-width row for one button.
    pub fn allocate(ui: &mut egui::Ui) -> (Self, egui::Response) {
        let width = ui.available_width();
        let (row, resp) =
            ui.allocate_exact_size(vec2(width, rail_icon_size(width)), egui::Sense::click());
        (
            Self::new(row),
            resp.on_hover_cursor(egui::CursorIcon::PointingHand),
        )
    }

    /// Corner radius: a circle / full pill at rest, easing to Discord's
    /// rounded square when `active` (0..1, hovered or selected).
    pub fn radius(&self, active: f32) -> f32 {
        let size = self.icon.height();
        egui::lerp(size / 2.0..=size / 3.0, active)
    }

    /// Paint the stretched pill behind the icon (nothing in the icon-only
    /// rail). `hover` (0..1) brightens it.
    pub fn paint_pill(&self, ui: &egui::Ui, radius: f32, selected: bool, pal: &Palette) {
        if self.widen <= 0.0 {
            return;
        }
        let fill = if selected {
            pal.selected
        } else {
            pal.rail_icon.gamma_multiply(0.55)
        };
        ui.painter()
            .rect_filled(self.button, radius, fill.gamma_multiply(self.widen));
    }

    /// A light wash over the whole button while hovered, so it reads as
    /// clickable; drawn after the icon.
    pub fn paint_hover(&self, ui: &egui::Ui, radius: f32, hover: f32) {
        let a = (hover * 14.0).round() as u8;
        if a > 0 {
            ui.painter()
                .rect_filled(self.button, radius, Color32::from_white_alpha(a));
        }
    }

    /// Where the name goes: from 12 px after the icon to 10 px before the
    /// pill's right end (empty in the icon-only rail).
    fn label_span(&self) -> (f32, f32) {
        (self.icon.right() + 12.0, self.button.right() - 10.0)
    }

    /// The name beside the icon, revealed as the rail widens: clipped at the
    /// pill's right end with a short fade, and cut with "…" only once the
    /// rail is at its widest. Returns true when the whole name shows.
    pub fn paint_label(
        &self,
        ui: &egui::Ui,
        text: &str,
        font: egui::FontId,
        color: Color32,
    ) -> bool {
        let (x0, x1) = self.label_span();
        let room = x1 - x0;
        if room <= 1.0 || text.is_empty() {
            return text.is_empty();
        }
        let fully_wide = self.row.width() >= crate::responsive::RAIL_NAMES.max - 0.5;
        let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font, color);
        if fully_wide {
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
        }
        let galley = ui.painter().layout_job(job);
        let pos = egui::pos2(x0, self.row.center().y - galley.size().y / 2.0);
        let clip = ui.clip_rect();
        if galley.size().x <= room || fully_wide {
            ui.painter().galley(pos, galley.clone(), color);
            return !galley.elided;
        }
        // Clipped: solid up to the fade, then thin slices fading out. Plain
        // opacity, so it works over any theme background or gradient.
        let fade = RAIL_LABEL_FADE.min(room);
        let y = self.row.y_range();
        let solid = egui::Rect::from_x_y_ranges(x0 - 2.0..=x1 - fade, y);
        ui.painter()
            .with_clip_rect(solid.intersect(clip))
            .galley(pos, galley.clone(), color);
        const SLICES: usize = 8;
        let step = fade / SLICES as f32;
        for i in 0..SLICES {
            let left = x1 - fade + step * i as f32;
            let slice = egui::Rect::from_x_y_ranges(left..=left + step, y);
            let alpha = 1.0 - (i as f32 + 0.5) / SLICES as f32;
            ui.painter().with_clip_rect(slice.intersect(clip)).add(
                egui::epaint::TextShape::new(pos, galley.clone(), color).with_opacity_factor(alpha),
            );
        }
        false
    }

    /// The short divider under Home: 32 px under the icon in the icon-only
    /// rail, stretching with the pills.
    pub fn divider(&self) -> egui::Rect {
        let inset = self.icon.width() / 6.0;
        egui::Rect::from_x_y_ranges(
            self.button.left() + inset..=self.button.right() - inset,
            self.row.center().y - 1.0..=self.row.center().y + 1.0,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(width: f32) -> egui::Rect {
        egui::Rect::from_min_size(egui::pos2(0.0, 0.0), vec2(width, rail_icon_size(width)))
    }

    #[test]
    fn rail_button_is_the_icon_rail_at_minimum_width() {
        let b = RailButton::new(row(72.0));
        assert_eq!(
            b.icon,
            egui::Rect::from_min_size(egui::pos2(12.0, 0.0), vec2(48.0, 48.0))
        );
        assert_eq!(b.button, b.icon);
        assert_eq!(b.widen, 0.0);
        // Divider: 32 px centred under the icon.
        let d = b.divider();
        assert_eq!((d.left(), d.right()), (20.0, 52.0));
    }

    #[test]
    fn rail_button_stretches_smoothly_and_keeps_the_icon_still() {
        let mut last = RailButton::new(row(72.0));
        for w in (73..=220).map(|w| w as f32) {
            let b = RailButton::new(row(w));
            assert_eq!(b.icon, last.icon, "icon moved at {w}");
            assert_eq!(b.button.right(), w - 12.0);
            assert!(b.widen >= last.widen && b.widen - last.widen <= 1.0 / 40.0);
            last = b;
        }
        assert_eq!(last.widen, 1.0);
    }

    #[test]
    fn squeezed_rail_centres_a_smaller_icon() {
        let b = RailButton::new(row(56.0));
        assert_eq!(b.icon.width(), 36.0);
        assert_eq!(b.icon.center().x, 28.0);
        assert_eq!(b.button, b.icon);
    }

    #[test]
    fn media_width_caps_and_fits_the_column() {
        // Wide column: the cap wins.
        assert_eq!(media_width(900.0, 400.0), 400.0);
        // Narrow column: 16 px less than what's left.
        assert_eq!(media_width(250.0, 400.0), 234.0);
        // Never collapses to nothing.
        assert_eq!(media_width(10.0, 400.0), 32.0);
        assert_eq!(media_width(120.0, 160.0), 104.0);
    }
}
