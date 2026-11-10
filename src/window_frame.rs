//! The merged window frame: RustCord runs without the OS title bar and
//! draws – □ X into the header of whichever pane sits in the window's
//! top-right corner (usually the chat, or Members when it's on the right).
//! Dragging empty space in a top-row header moves the window, a
//! double-click there toggles maximise, and thin invisible bands along the
//! window border resize it.
//!
//! Headers find out for themselves whether they touch the top-right corner
//! (`header`), so the buttons follow tile moves and hidden panes with no
//! bookkeeping. When no header claims them in a pass (login screen,
//! settings page, a pane without a header in the corner) `end_pass` draws a
//! small floating strip there instead, so the window can always be closed.
//!
//! Settings → Appearance → "Use system title bar" brings the native frame
//! back (`Config::system_title_bar`); everything here is then skipped.
//!
//! Every window (the main one and each account window) runs its own
//! `begin_pass` / `end_pass`; the frame's state is kept per viewport, and
//! the buttons act on the window they are drawn in.

use eframe::egui::{self, Color32, CursorIcon, Pos2, Rect, ResizeDirection, Sense, vec2};

use crate::icons;
use crate::theme::Palette;

/// Platforms where RustCord draws its own window controls.
///
/// macOS keeps its native title bar (traffic lights): winit has no
/// `drag_resize_window` there, so an undecorated window couldn't be resized
/// from its edges, and Mac users expect the traffic lights on the left.
/// Windows, X11 and Wayland support undecorated windows plus `StartDrag` /
/// `BeginResize` (xdg-toplevel move / resize on Wayland).
pub const SUPPORTED: bool = !cfg!(target_os = "macos");

/// Width of the invisible resize band along the window border.
const BAND: f32 = 5.0;
/// How far along an edge from a corner the diagonal resize applies.
const CORNER: f32 = 14.0;
/// A header counts as "top row" / "top-right" when it is this close to the
/// window's top / right edge (themes may inset panes a little).
const SLACK: f32 = 32.0;
/// Window button size (same square as `widgets::icon_button`).
const BUTTON: f32 = 32.0;
const BUTTON_GAP: f32 = 2.0;
/// Space between the last button and the window's right edge.
const RIGHT_PAD: f32 = 8.0;
/// Space between the header's own content and the buttons.
const CONTENT_GAP: f32 = 8.0;

/// Total width the buttons take from a header.
pub const RESERVED: f32 = CONTENT_GAP + 3.0 * BUTTON + 2.0 * BUTTON_GAP + RIGHT_PAD;

// Per-viewport memory keys (each window has its own frame state).
fn active_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("rc_window_frame_active").with(ctx.viewport_id())
}
fn decorations_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("rc_window_frame_decorations").with(ctx.viewport_id())
}
fn claimed_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("rc_window_frame_claimed").with(ctx.viewport_id())
}

/// RustCord draws its own frame (no OS decorations) for this setting.
pub fn custom_frame(system_title_bar: bool) -> bool {
    SUPPORTED && !system_title_bar
}

/// Call at the start of every pass. `custom` is false when the user picked
/// the system title bar. Switches the OS decorations when that changes.
pub fn begin_pass(ctx: &egui::Context, system_title_bar: bool) {
    let custom = custom_frame(system_title_bar);
    let (decorations, active) = (decorations_id(ctx), active_id(ctx));
    let last = ctx.data_mut(|d| d.get_temp::<bool>(decorations));
    if last != Some(custom) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(!custom));
        ctx.data_mut(|d| d.insert_temp(decorations, custom));
    }
    ctx.data_mut(|d| d.insert_temp(active, custom));
}

/// RustCord draws the window frame this pass.
fn active(ctx: &egui::Context) -> bool {
    let id = active_id(ctx);
    ctx.data(|d| d.get_temp::<bool>(id).unwrap_or(false))
}

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false))
}

fn fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

fn toggle_maximized(ctx: &egui::Context) {
    let max = maximized(ctx);
    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
}

/// `rect` is a top-row header that ends at the window's top-right corner.
pub fn touches_top_right(rect: Rect, window: Rect) -> bool {
    rect.top() <= window.top() + SLACK && rect.right() >= window.right() - SLACK
}

/// `rect` sits in the window's top row.
pub fn in_top_row(rect: Rect, window: Rect) -> bool {
    rect.top() <= window.top() + SLACK
}

/// Which way the window resizes when pressed at `pos`: only inside the
/// `band` along the border, diagonal within `corner` of a corner.
pub fn resize_direction(
    pos: Pos2,
    window: Rect,
    band: f32,
    corner: f32,
) -> Option<ResizeDirection> {
    if !window.contains(pos) {
        return None;
    }
    let left = pos.x - window.left() < band;
    let right = window.right() - pos.x < band;
    let top = pos.y - window.top() < band;
    let bottom = window.bottom() - pos.y < band;
    if !(left || right || top || bottom) {
        return None;
    }
    // Near a corner along either edge counts as that corner.
    let near_left = pos.x - window.left() < corner;
    let near_right = window.right() - pos.x < corner;
    let near_top = pos.y - window.top() < corner;
    let near_bottom = window.bottom() - pos.y < corner;
    use ResizeDirection as D;
    Some(
        match (
            near_top && (left || right) || top && (near_left || near_right),
            near_bottom && (left || right) || bottom && (near_left || near_right),
        ) {
            (true, _) if near_left => D::NorthWest,
            (true, _) if near_right => D::NorthEast,
            (_, true) if near_left => D::SouthWest,
            (_, true) if near_right => D::SouthEast,
            _ if top => D::North,
            _ if bottom => D::South,
            _ if left => D::West,
            _ => D::East,
        },
    )
}

fn resize_cursor(dir: ResizeDirection) -> CursorIcon {
    use ResizeDirection as D;
    match dir {
        D::North | D::South => CursorIcon::ResizeVertical,
        D::East | D::West => CursorIcon::ResizeHorizontal,
        D::NorthEast | D::SouthWest => CursorIcon::ResizeNeSw,
        D::NorthWest | D::SouthEast => CursorIcon::ResizeNwSe,
    }
}

/// Nothing floats over the press point except `layer` itself (menus,
/// popups and drawers win).
fn press_on_layer(ctx: &egui::Context, layer: egui::LayerId) -> bool {
    let origin = ctx.input(|i| i.pointer.press_origin());
    let hit = origin.and_then(|p| ctx.layer_id_at(p));
    hit.is_none() || hit == Some(layer)
}

/// Window drag + double-click maximise on `rect` (registered before the
/// caller's widgets, so they keep their clicks).
fn drag_area(ui: &mut egui::Ui, rect: Rect, id: egui::Id) {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    if resp.drag_started_by(egui::PointerButton::Primary) && press_on_layer(&ctx, ui.layer_id()) {
        ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
    }
    if resp.double_clicked() && press_on_layer(&ctx, ui.layer_id()) {
        toggle_maximized(&ctx);
    }
}

/// Hook for every pane header (`app::header_bar`, `friends::header_bar`).
/// Call right after allocating the header's `rect`, before drawing its
/// contents. A top-row header becomes a window drag handle; the one in the
/// top-right corner also gets – □ X. Returns the part of `rect` left for
/// the header's own content.
pub fn header(ui: &mut egui::Ui, rect: Rect, pal: &Palette) -> Rect {
    let ctx = ui.ctx().clone();
    if !active(&ctx) {
        return rect;
    }
    let window = ctx.content_rect();
    if !in_top_row(rect, window) {
        return rect;
    }
    let id = ui.unique_id().with("rc_header_drag");
    drag_area(ui, rect, id);
    if !touches_top_right(rect, window) || rect.width() < RESERVED * 2.0 {
        return rect;
    }
    // Read the pass number first: no `ctx` calls inside `data_mut`.
    let pass = ctx.cumulative_pass_nr();
    let claimed = claimed_id(&ctx);
    ctx.data_mut(|d| d.insert_temp(claimed, pass));
    let strip = Rect::from_min_max(
        egui::pos2(rect.right() - RESERVED + CONTENT_GAP, rect.top()),
        rect.right_bottom(),
    );
    controls(ui, strip, pal);
    let mut left = rect;
    left.max.x = rect.right() - RESERVED;
    left
}

/// – □ X, right-aligned in `strip` (vertically centred).
fn controls(ui: &mut egui::Ui, strip: Rect, pal: &Palette) {
    let ctx = ui.ctx().clone();
    let y = strip.center().y - BUTTON / 2.0;
    let mut x = strip.right() - RIGHT_PAD - BUTTON;
    let at = |x: f32| Rect::from_min_size(egui::pos2(x, y), vec2(BUTTON, BUTTON));
    if window_button(ui, at(x), icons::X, "Close", true, pal).clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
    }
    x -= BUTTON + BUTTON_GAP;
    let max = maximized(&ctx);
    let (icon, tip) = if max {
        (icons::COPY, "Restore")
    } else {
        (icons::SQUARE, "Maximise")
    };
    if window_button(ui, at(x), icon, tip, false, pal).clicked() {
        toggle_maximized(&ctx);
    }
    x -= BUTTON + BUTTON_GAP;
    if window_button(ui, at(x), icons::MINUS, "Minimise", false, pal).clicked() {
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true));
    }
    // A thin divider between the header's content and the buttons.
    let div_x = x - CONTENT_GAP / 2.0;
    ui.painter().vline(
        div_x,
        egui::Rangef::new(strip.center().y - 10.0, strip.center().y + 10.0),
        egui::Stroke::new(1.0, pal.divider),
    );
}

/// One window button, styled like `widgets::icon_button` (hover fade,
/// pressed glyph sinks a pixel). `danger`: the close button turns red.
fn window_button(
    ui: &mut egui::Ui,
    rect: Rect,
    icon: &str,
    tip: &str,
    danger: bool,
    pal: &Palette,
) -> egui::Response {
    let id = ui.unique_id().with(("rc_window_button", tip));
    let resp = ui.interact(rect, id, Sense::click());
    let hovered = resp.hovered();
    let pressed = resp.is_pointer_button_down_on();
    let t = ui
        .ctx()
        .animate_bool_with_time(id.with("hover"), hovered, 0.08);
    if t > 0.0 {
        let fill = match (danger, pressed) {
            (true, true) => pal.danger.gamma_multiply(0.8),
            (true, false) => pal.danger,
            (false, true) => pal.selected,
            (false, false) => pal.hover,
        };
        ui.painter()
            .rect_filled(rect.shrink(2.0), 4, fill.gamma_multiply(t));
    }
    let col = if danger && hovered {
        Color32::WHITE
    } else if hovered {
        pal.strong
    } else {
        pal.muted
    };
    let at = if pressed {
        rect.translate(vec2(0.0, 1.0))
    } else {
        rect
    };
    icons::paint(ui, at, icon, icons::font(20.0), col);
    resp.on_hover_text(tip)
}

/// Call at the end of every pass: the floating – □ X when no header showed
/// them, and the resize bands along the window border.
pub fn end_pass(ctx: &egui::Context, pal: &Palette) {
    if !active(ctx) {
        return;
    }
    let window = ctx.content_rect();
    let id = claimed_id(ctx);
    let claimed = ctx.data(|d| d.get_temp::<u64>(id));
    if claimed != Some(ctx.cumulative_pass_nr()) {
        fallback_controls(ctx, window, pal);
    }
    if !maximized(ctx) && !fullscreen(ctx) {
        resize_bands(ctx, window);
    }
}

/// The buttons on a small strip of their own in the top-right corner; its
/// padding on the left drags the window.
fn fallback_controls(ctx: &egui::Context, window: Rect, pal: &Palette) {
    let size = vec2(RESERVED + 8.0, 40.0);
    egui::Area::new(egui::Id::unique("rc_window_controls"))
        .order(egui::Order::Foreground)
        .pivot(egui::Align2::RIGHT_TOP)
        .fixed_pos(window.right_top())
        .constrain(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
            ui.painter().rect_filled(
                rect,
                egui::CornerRadius {
                    sw: 6,
                    ..Default::default()
                },
                pal.servers,
            );
            let id = ui.unique_id().with("rc_fallback_drag");
            drag_area(ui, rect, id);
            controls(ui, rect, pal);
        });
}

/// Four thin foreground areas along the border: they only cover the outer
/// band, so content keeps every click a few pixels inside.
fn resize_bands(ctx: &egui::Context, window: Rect) {
    let edges = [
        Rect::from_min_size(window.left_top(), vec2(window.width(), BAND)),
        Rect::from_min_max(
            egui::pos2(window.left(), window.bottom() - BAND),
            window.max,
        ),
        Rect::from_min_size(window.left_top(), vec2(BAND, window.height())),
        Rect::from_min_max(egui::pos2(window.right() - BAND, window.top()), window.max),
    ];
    for (n, edge) in edges.into_iter().enumerate() {
        egui::Area::new(egui::Id::unique(("rc_resize_band", n)))
            .order(egui::Order::Foreground)
            .fixed_pos(edge.min)
            .constrain(false)
            .show(ctx, |ui| {
                let (_, resp) = ui.allocate_exact_size(edge.size(), Sense::drag());
                let Some(pos) = resp.hover_pos().or(resp.interact_pointer_pos()) else {
                    return;
                };
                let Some(dir) = resize_direction(pos, window, BAND, CORNER) else {
                    return;
                };
                ui.ctx().set_cursor_icon(resize_cursor(dir));
                if resp.drag_started_by(egui::PointerButton::Primary) {
                    ui.ctx()
                        .send_viewport_cmd(egui::ViewportCommand::BeginResize(dir));
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::pos2;

    fn window() -> Rect {
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1200.0, 760.0))
    }

    #[test]
    fn top_right_header_is_found() {
        let w = window();
        // Chat header ending at the window's right edge.
        let chat = Rect::from_min_max(pos2(312.0, 0.0), pos2(1200.0, 48.0));
        assert!(touches_top_right(chat, w));
        // Chat header next to Members: not the corner.
        let chat_mid = Rect::from_min_max(pos2(312.0, 0.0), pos2(960.0, 48.0));
        assert!(!touches_top_right(chat_mid, w));
        assert!(in_top_row(chat_mid, w));
        // A themed inset still counts.
        let inset = Rect::from_min_max(pos2(968.0, 8.0), pos2(1192.0, 56.0));
        assert!(touches_top_right(inset, w));
        // A header lower down (pane stacked under another) doesn't.
        let low = Rect::from_min_max(pos2(960.0, 400.0), pos2(1200.0, 448.0));
        assert!(!touches_top_right(low, w));
        assert!(!in_top_row(low, w));
    }

    #[test]
    fn resize_only_in_the_band() {
        let w = window();
        use ResizeDirection as D;
        let dir = |x: f32, y: f32| resize_direction(pos2(x, y), w, BAND, CORNER);
        assert_eq!(dir(600.0, 400.0), None);
        assert_eq!(dir(600.0, 6.0), None);
        assert_eq!(dir(600.0, 2.0), Some(D::North));
        assert_eq!(dir(600.0, 758.0), Some(D::South));
        assert_eq!(dir(1.0, 400.0), Some(D::West));
        assert_eq!(dir(1198.0, 400.0), Some(D::East));
        assert_eq!(dir(-1.0, 400.0), None);
    }

    #[test]
    fn corners_resize_diagonally() {
        let w = window();
        use ResizeDirection as D;
        let dir = |x: f32, y: f32| resize_direction(pos2(x, y), w, BAND, CORNER);
        assert_eq!(dir(1.0, 1.0), Some(D::NorthWest));
        // Along the top edge, a little in from the corner.
        assert_eq!(dir(10.0, 2.0), Some(D::NorthWest));
        // Along the right edge, a little below the corner.
        assert_eq!(dir(1198.0, 10.0), Some(D::NorthEast));
        assert_eq!(dir(2.0, 750.0), Some(D::SouthWest));
        assert_eq!(dir(1190.0, 758.0), Some(D::SouthEast));
    }
}
