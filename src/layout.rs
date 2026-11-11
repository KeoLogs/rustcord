//! The movable / resizable panel layout, built on egui_tiles.
//!
//! - Normal mode: panels are fixed (the dividers can still be dragged to
//!   resize, if the layout editor module is on).
//! - Edit mode (Settings → RustCord → Layout → Edit layout): hovering a panel
//!   makes it glow; press and hold to pick it up (it grows while you drag),
//!   then drop it on an edge of another panel to dock it there, or on its
//!   middle to stack them as tabs.
//! - The layout is saved between runs; "Reset layout" restores the default.
//! - Every frame `tile_fit` makes the tree fit the window: panes get pixel
//!   sizes within their min / max (only the chat absorbs window changes),
//!   and on narrow windows Members, then the lists and user panel, fold
//!   into overlay drawers ([`draw_drawers`]) instead of being squashed.
//!   Every pane clips to its tile ([`draw_pane`]).

use eframe::egui::{self, Color32, Sense, Stroke, StrokeKind, vec2};
use egui_tiles::{
    Behavior, Container, SimplificationOptions, Tabs, Tile, TileId, Tiles, Tree, UiResponse,
};
use serde::{Deserialize, Serialize};

use crate::app::State;
use crate::config::Config;
use crate::theme::Palette;

pub const LAYOUT_KEY: &str = "layout_v2";

/// One movable part of the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Servers,
    Profile,
    Channels,
    Messages,
    Composer,
    Members,
}

impl Pane {
    pub const ALL: [Pane; 6] = [
        Pane::Servers,
        Pane::Profile,
        Pane::Channels,
        Pane::Messages,
        Pane::Composer,
        Pane::Members,
    ];

    pub fn title(self) -> &'static str {
        match self {
            Pane::Servers => "Servers",
            Pane::Profile => "Profile",
            Pane::Channels => "Channels",
            Pane::Messages => "Messages",
            Pane::Composer => "Message box",
            Pane::Members => "Members",
        }
    }

    /// The short lowercase name cut into a framed pane's top border.
    pub fn label(self) -> &'static str {
        match self {
            Pane::Servers => "nav",
            Pane::Profile => "user",
            Pane::Channels => "channels",
            Pane::Messages => "chat",
            Pane::Composer => "input",
            Pane::Members => "members",
        }
    }

    pub fn icon(self) -> &'static str {
        use crate::icons;
        match self {
            Pane::Servers => icons::SQUARES_FOUR,
            Pane::Profile => icons::USER_CIRCLE,
            Pane::Channels => icons::HASH,
            Pane::Messages => icons::CHATS,
            Pane::Composer => icons::PENCIL,
            Pane::Members => icons::USERS,
        }
    }
}

/// The default layout (Discord's: the user panel spans the bottom of the
/// server rail and channel list):
///
/// ```text
/// ┌────────┬──────────┬─────────────────────────┬─────────┐
/// │Servers │ Channels │ Messages                │ Members │
/// │        │          │                         │         │
/// │        │          ├─────────────────────────┤         │
/// ├────────┴──────────┤ Message box             │         │
/// │ Profile           │                         │         │
/// └───────────────────┴─────────────────────────┴─────────┘
/// ```
pub fn default_tree() -> Tree<Pane> {
    // The Discord preset, in pixel shares (rail 72, channels 240, members
    // 240, user panel 52); `tile_fit` keeps them and the chat absorbs the
    // window.
    crate::layout_presets::build(crate::layout_presets::Preset::Discord)
}

/// Demo mode only (`RUSTCORD_DEMO_LAYOUT=alt`): a custom arrangement like
/// Leo's (wide server list over the channels, members over the profile
/// bar in the middle, chat on the right), to check the UI in layouts other
/// than the default.
pub fn alt_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let tab = |tiles: &mut Tiles<Pane>, pane| {
        let id = tiles.insert_pane(pane);
        tiles.insert_tab_tile(vec![id])
    };
    let servers = tab(&mut tiles, Pane::Servers);
    let profile = tab(&mut tiles, Pane::Profile);
    let channels = tab(&mut tiles, Pane::Channels);
    let messages = tab(&mut tiles, Pane::Messages);
    let composer = tab(&mut tiles, Pane::Composer);
    let members = tab(&mut tiles, Pane::Members);
    let left = tiles.insert_vertical_tile(vec![servers, channels]);
    set_shares(&mut tiles, left, &[(servers, 1.0), (channels, 1.0)]);
    let middle = tiles.insert_vertical_tile(vec![members, profile]);
    set_shares(&mut tiles, middle, &[(members, 11.0), (profile, 1.0)]);
    let chat = tiles.insert_vertical_tile(vec![messages, composer]);
    set_shares(&mut tiles, chat, &[(messages, 9.0), (composer, 1.2)]);
    let root = tiles.insert_horizontal_tile(vec![left, middle, chat]);
    set_shares(&mut tiles, root, &[(left, 2.5), (middle, 2.9), (chat, 6.8)]);
    Tree::new(egui::Id::unique("rustcord_layout"), root, tiles)
}

/// Demo mode (`RUSTCORD_DEMO_LAYOUT=leo5`) and tests: Leo's leo-5 layout,
/// the server rail as a strip across the top, a wide DM list, the chat,
/// Members over the user panel on the right, and the message box as a
/// full-width row along the bottom (shares as Leo's window had them).
pub fn leo5_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();
    let tab = |tiles: &mut Tiles<Pane>, pane| {
        let id = tiles.insert_pane(pane);
        tiles.insert_tab_tile(vec![id])
    };
    let servers = tab(&mut tiles, Pane::Servers);
    let channels = tab(&mut tiles, Pane::Channels);
    let messages = tab(&mut tiles, Pane::Messages);
    let members = tab(&mut tiles, Pane::Members);
    let profile = tab(&mut tiles, Pane::Profile);
    let composer = tab(&mut tiles, Pane::Composer);
    let right = tiles.insert_vertical_tile(vec![members, profile]);
    set_shares(&mut tiles, right, &[(members, 7.0), (profile, 1.0)]);
    let middle = tiles.insert_horizontal_tile(vec![channels, messages, right]);
    set_shares(
        &mut tiles,
        middle,
        &[(channels, 590.0), (messages, 300.0), (right, 300.0)],
    );
    let root = tiles.insert_vertical_tile(vec![servers, middle, composer]);
    set_shares(
        &mut tiles,
        root,
        &[(servers, 95.0), (middle, 580.0), (composer, 44.0)],
    );
    Tree::new(egui::Id::unique("rustcord_layout"), root, tiles)
}

/// A saved layout is only usable if it still has every panel (new versions
/// may add panels).
pub fn is_complete(tree: &Tree<Pane>) -> bool {
    let present: Vec<Pane> = tree
        .tiles
        .iter()
        .filter_map(|(_, tile)| match tile {
            Tile::Pane(p) => Some(*p),
            Tile::Container(_) => None,
        })
        .collect();
    Pane::ALL.iter().all(|p| present.contains(p))
}

/// Is this panel shown? (The members button in the chat header hides the
/// Members panel, « / » the Channels panel.)
pub fn pane_visible(tree: &Tree<Pane>, pane: Pane) -> bool {
    tree.tiles
        .find_pane(&pane)
        .is_none_or(|id| tree.tiles.is_visible(id))
}

/// Show / hide a panel (Members, Channels). Hidden tiles keep their place in
/// the tree, so showing it again puts it back where it was (and the choice
/// is saved with the layout).
pub fn toggle_pane(tree: &mut Tree<Pane>, pane: Pane) {
    let Some(id) = tree.tiles.find_pane(&pane) else {
        return;
    };
    let show = !tree.tiles.is_visible(id);
    tree.tiles.set_visible(id, show);
    // Stacked as a tab with other panels: switch tabs so one stays shown.
    if let Some(parent) = tree.tiles.parent_of(id)
        && let Some(Tile::Container(Container::Tabs(tabs))) = tree.tiles.get(parent)
    {
        let children = tabs.children.clone();
        let target = if show {
            Some(id)
        } else {
            children.into_iter().find(|c| *c != id)
        };
        if let (Some(t), Some(Tile::Container(Container::Tabs(tabs)))) =
            (target, tree.tiles.get_mut(parent))
        {
            tabs.set_active(t);
        }
    }
}

/// Inner margin of each panel. Panels that draw full-width headers or
/// hover rows (channels, messages) handle their own padding.
fn pane_margin(pane: Pane) -> egui::Margin {
    match pane {
        Pane::Servers => egui::Margin::symmetric(0, 12),
        Pane::Channels | Pane::Messages => egui::Margin::ZERO,
        // The same small side padding as the user panel, so the "+" and
        // the avatar line up with their pane edges (blocky look).
        Pane::Composer => egui::Margin::symmetric(8, 0),
        Pane::Members => egui::Margin::ZERO,
        Pane::Profile => egui::Margin::symmetric(8, 0),
    }
}

fn set_shares(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for &(id, share) in shares {
            linear.shares.set_share(id, share);
        }
    }
}

/// Memory key for the drag "pick up" animation start time.
fn drag_anim_id() -> egui::Id {
    egui::Id::unique("rustcord_drag_started_at")
}

/// Call once per frame after drawing the tree, so the pick-up animation
/// restarts on the next drag.
pub fn end_frame(tree: &Tree<Pane>, ctx: &egui::Context) {
    if tree.dragged_id(ctx).is_none() {
        ctx.memory_mut(|m| m.data.remove::<f64>(drag_anim_id()));
    }
}

// ---- Frame drag ---------------------------------------------------------
//
// Framed panes (the default blocky look) and themes with an `inset` draw
// each pane as a card with a strip of backdrop (the "frame") around it. Pressing on that empty strip and dragging moves
// the OS window, like a title bar. The card itself and every widget keep
// their clicks: the strips are registered *before* the tiles draw, so
// anything egui_tiles or a pane registers on top (resize handles, tab bars)
// wins, and the strips never overlap a card. Popups, menus and drawers are
// separate layers and also win.
//
// `ViewportCommand::StartDrag` is supported by winit on Windows, macOS, X11
// and Wayland, so it isn't gated by platform.

/// Memory key for the card frames drawn last frame (one list per window).
fn frame_rects_id(ctx: &egui::Context) -> egui::Id {
    egui::Id::unique("rustcord_frame_rects").with(ctx.viewport_id())
}

/// Remember a pane's tile rect and card (only panes whose card is smaller
/// than the tile leave empty frame space). Called from `pane_ui`, so only
/// panes on screen count.
fn note_frame(ctx: &egui::Context, tile: egui::Rect, card: egui::Rect) {
    if card == tile || !tile.contains_rect(card) {
        return;
    }
    let id = frame_rects_id(ctx);
    ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(egui::Rect, egui::Rect)>>(id)
            .push((tile, card))
    });
}

/// The empty strips of a tile around its card (`tile` shrunk by `inset`).
#[cfg(test)]
pub fn frame_strips(tile: egui::Rect, inset: f32) -> [egui::Rect; 4] {
    frame_strips_around(tile, tile.shrink(inset))
}

/// The empty strips of a tile around `card` (top, bottom, left, right).
pub fn frame_strips_around(tile: egui::Rect, card: egui::Rect) -> [egui::Rect; 4] {
    let r = |x0: f32, y0: f32, x1: f32, y1: f32| {
        egui::Rect::from_min_max([x0, y0].into(), [x1, y1].into())
    };
    [
        r(tile.min.x, tile.min.y, tile.max.x, card.min.y),
        r(tile.min.x, card.max.y, tile.max.x, tile.max.y),
        r(tile.min.x, card.min.y, card.min.x, card.max.y),
        r(card.max.x, card.min.y, tile.max.x, card.max.y),
    ]
}

/// Call once per frame in the tiles' `Ui`, **before** `tree.ui`: makes last
/// frame's empty card frames drag the window (egui hit-tests against last
/// frame's widgets anyway), then clears the list for this frame's panes.
/// `enabled` is false while editing the layout (dragging there moves
/// panels) or while a drawer covers the tiles.
pub fn frame_drag(ui: &mut egui::Ui, enabled: bool) {
    let ctx = ui.ctx().clone();
    let id = frame_rects_id(&ctx);
    let frames = ctx.data_mut(|d| {
        d.get_temp_mut_or_default::<Vec<(egui::Rect, egui::Rect)>>(id)
            .split_off(0)
    });
    if !enabled {
        return;
    }
    let base = ui.scope_id().with("frame_drag");
    for (n, (tile, card)) in frames.into_iter().enumerate() {
        for (k, strip) in frame_strips_around(tile, card).into_iter().enumerate() {
            if strip.width() <= 0.0 || strip.height() <= 0.0 {
                continue;
            }
            let resp = ui.interact(strip, base.with((n, k)), Sense::drag());
            if resp.drag_started_by(egui::PointerButton::Primary) {
                // Only when nothing floats over this spot (menus, drawers).
                let origin = ctx.input(|i| i.pointer.press_origin());
                let layer = origin.and_then(|p| ctx.layer_id_at(p));
                if layer.is_none() || layer == Some(ui.layer_id()) {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                }
            }
        }
    }
}

/// Everything a pane needs to draw itself, whatever hosts it.
pub struct PaneCtx<'a> {
    pub state: &'a mut State,
    pub config: &'a Config,
    pub pal: &'a Palette,
    /// The active theme (gradients, glass, chat background).
    pub theme: &'a crate::themes::ActiveTheme,
}

/// Draw one pane's content into `ui`: theme background, fill, margin, the
/// pane itself and (in the blocky look) its border with the notch label,
/// **clipped to `ui.max_rect()`** so nothing draws past its edge (popups
/// and menus are separate layers).
///
/// Container-agnostic: the caller can be an egui_tiles tile, a side panel,
/// a framed card or a viewport (layout_v3 `Mode`); it only has to hand over
/// a `Ui` sized to the pane.
pub fn draw_pane(ui: &mut egui::Ui, pane: Pane, ctx: &mut PaneCtx<'_>) {
    let rect = ui.max_rect();
    ui.set_clip_rect(rect.intersect(ui.clip_rect()));
    // Theme gradients / glass / background image first; the frame fill is
    // transparent for panes the theme paints.
    ctx.theme.paint_pane_background(ui.painter(), rect, pane);
    let content = ctx.theme.pane_content(rect, pane);
    if pane == Pane::Composer {
        ctx.state.composer_frame_top = Some(rect.top());
    }
    let show = |ui: &mut egui::Ui, ctx: &mut PaneCtx<'_>| {
        egui::Frame::new()
            .fill(ctx.theme.pane_fill(pane))
            .inner_margin(pane_margin(pane))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ctx.state.pane_ui(ui, pane, ctx.config, ctx.pal);
            });
    };
    if content == rect {
        show(ui, ctx);
    } else {
        // Framed: the pane lives inside its border, below the label.
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(content)
                .layout(egui::Layout::top_down(egui::Align::Min)),
            |ui| {
                ui.set_clip_rect(content.intersect(ui.clip_rect()));
                show(ui, ctx);
            },
        );
        ctx.theme
            .paint_pane_frame(ui.painter(), rect, pane, pane.label());
    }
}

/// Draw a whole layout_v3 column into `ui` (container-agnostic, like
/// [`draw_pane`]). The chat column is one thing: the composer sits at its
/// bottom and grows upward (44 px up to 40% of the column), the messages
/// take the rest. The user panel is docked by the host, not here.
#[allow(dead_code)] // Called by the panel host (rustcord-ui, next on this branch).
pub fn draw_column(ui: &mut egui::Ui, column: crate::layout_v3::Column, ctx: &mut PaneCtx<'_>) {
    use crate::layout_v3::Column;
    match column {
        Column::Chat => {
            let rect = ui.max_rect();
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            let max = crate::responsive::composer_height(rect.height(), rect.height());
            egui::Panel::bottom(ui.scope_id().with("chat_composer"))
                .resizable(false)
                .frame(egui::Frame::NONE)
                .size_range(crate::responsive::COMPOSER_MIN_HEIGHT..=max)
                .show(ui, |ui| draw_pane(ui, Pane::Composer, ctx));
            egui::CentralPanel::no_frame().show(ui, |ui| draw_pane(ui, Pane::Messages, ctx));
        }
        other => {
            for &pane in other.panes() {
                draw_pane(ui, pane, ctx);
            }
        }
    }
}

/// Connects the tile tree to the app state.
pub struct LayoutBehavior<'a> {
    pub state: &'a mut State,
    pub config: &'a Config,
    pub pal: Palette,
    /// The active theme (gradients, glass, chat background).
    pub theme: &'a crate::themes::ActiveTheme,
    pub editing: bool,
}

impl Behavior<Pane> for LayoutBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let rect = ui.max_rect();
        note_frame(ui.ctx(), rect, self.theme.pane_card(rect, *pane));
        draw_pane(
            ui,
            *pane,
            &mut PaneCtx {
                state: self.state,
                config: self.config,
                pal: &self.pal,
                theme: self.theme,
            },
        );

        if !self.editing {
            return UiResponse::None;
        }

        // ---- Edit mode: the whole panel becomes a drag handle that glows.
        let resp = ui.interact(
            rect,
            ui.scope_id().with(("edit_pane", tile_id)),
            Sense::click_and_drag(),
        );
        let hot = ui
            .ctx()
            .animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), 0.12);
        let painter = ui.painter_at(rect);
        let accent = self.pal.accent;

        // Dim every panel a little so it's obvious we're editing.
        painter.rect_filled(rect, 0, Color32::from_black_alpha(110));
        // Glow: a few inner rings fading inwards, plus a light tint.
        if hot > 0.0 {
            painter.rect_filled(rect, 0, accent.gamma_multiply(0.10 * hot));
            for i in 0..6 {
                let alpha = hot * (0.9 - i as f32 * 0.15);
                painter.rect_stroke(
                    rect.shrink(i as f32 * 2.0),
                    6,
                    Stroke::new(2.0, accent.gamma_multiply(alpha.max(0.0))),
                    StrokeKind::Inside,
                );
            }
        } else {
            painter.rect_stroke(
                rect.shrink(1.0),
                6,
                Stroke::new(1.0, accent.gamma_multiply(0.35)),
                StrokeKind::Inside,
            );
        }
        // Label in the middle.
        painter.text(
            rect.center() - vec2(0.0, 10.0),
            egui::Align2::CENTER_CENTER,
            format!("{}  {}", pane.icon(), pane.title()),
            egui::FontId::proportional(16.0 + 2.0 * hot),
            Color32::WHITE.gamma_multiply(0.7 + 0.3 * hot),
        );
        if hot > 0.5 {
            painter.text(
                rect.center() + vec2(0.0, 12.0),
                egui::Align2::CENTER_CENTER,
                "Hold and drag to move",
                egui::FontId::proportional(12.0),
                Color32::WHITE.gamma_multiply(0.6),
            );
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::Grab);
        }

        if resp.drag_started() {
            UiResponse::DragStarted
        } else {
            UiResponse::None
        }
    }

    /// The card under the cursor while dragging: it "picks up" by growing
    /// from 70% to 115% size over ~0.2s.
    fn drag_ui(&mut self, tiles: &Tiles<Pane>, ui: &mut egui::Ui, tile_id: TileId) {
        let (icon, title) = match tiles.get(tile_id) {
            Some(Tile::Pane(p)) => (p.icon(), p.title()),
            _ => (crate::icons::SQUARES_FOUR, "Panels"),
        };
        let now = ui.input(|i| i.time);
        let started = ui
            .ctx()
            .memory_mut(|m| *m.data.get_temp_mut_or_insert_with(drag_anim_id(), || now));
        let t = ((now - started) / 0.2).clamp(0.0, 1.0) as f32;
        let ease = 1.0 - (1.0 - t).powi(3);
        let scale = 0.7 + 0.45 * ease;
        if t < 1.0 {
            ui.ctx().request_repaint();
        }

        egui::Frame::new()
            .fill(self.pal.sidebar.gamma_multiply(0.95))
            .stroke(Stroke::new(2.0, self.pal.accent))
            .corner_radius(10)
            .shadow(egui::Shadow {
                offset: [0, 8],
                blur: 24,
                spread: 2,
                color: Color32::from_black_alpha(120),
            })
            .show(ui, |ui| {
                let size = vec2(220.0, 120.0) * scale;
                ui.set_min_size(size);
                ui.set_max_size(size);
                ui.centered_and_justified(|ui| {
                    ui.label(
                        egui::RichText::new(format!("{icon}  {title}"))
                            .size(18.0 * scale)
                            .strong(),
                    );
                });
            });
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        format!("{} {}", pane.icon(), pane.title()).into()
    }

    /// Tab bars only show when panels are stacked (otherwise the hidden tabs
    /// would be unreachable).
    fn is_tab_bar_visible(&self, _tile_id: TileId, tabs: &Tabs) -> bool {
        tabs.children.len() > 1
    }

    fn is_tile_draggable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        self.editing
    }

    fn is_container_resizable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        self.config.modules.layout_editor
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        // No lines between panels (like Discord); they're still resizable
        // by dragging the edge.
        if self.editing { 6.0 } else { 0.0 }
    }

    fn resize_stroke(
        &self,
        _style: &egui::Style,
        _resize_state: egui_tiles::ResizeState,
    ) -> Stroke {
        // No glow on the divider (Leo): hovering only changes the cursor to
        // the ↔ / ↕ resize arrow, which egui_tiles sets itself.
        Stroke::NONE
    }

    fn min_size(&self) -> f32 {
        44.0
    }

    fn tab_bar_color(&self, _visuals: &egui::Visuals) -> Color32 {
        self.pal.servers
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}

// ---------------------------------------------------------------------------
// Drawers (narrow windows; `tile_fit` decides what folds)

/// The tile host's per-session state, kept in `State` so the chat and
/// channel headers can open / close drawers.
#[derive(Debug, Default)]
pub struct HostState {
    /// Last frame's width tier (breakpoint hysteresis).
    pub tier: Option<crate::responsive::Tier>,
    /// Some panes are in the left drawer this frame (LIST button shows).
    pub lists_in_drawer: bool,
    /// Members is folded into the aside drawer this frame.
    pub aside_in_drawer: bool,
    /// The left drawer is open.
    pub left_open: bool,
    /// The aside drawer is open.
    pub aside_open: bool,
    /// The chat header's LIST button was clicked (the app toggles the left
    /// drawer).
    pub toggle_left: bool,
    /// The minimum window size last sent to the OS ([`keep_window_min`]).
    pub min_window: Option<[f32; 2]>,
}

/// Keep the window's minimum inner size at what the layout needs: every
/// pane's minimum plus its frame (border, label), the gaps and the bars
/// around the tiles (`bars`: window size minus the tile area), never under
/// `responsive::WINDOW_MIN`. Sent only when it changes; the OS then grows a
/// window that's smaller (winit re-applies the bounds at once), and
/// `tile_fit` squashes the lists, not the chat or composer, until it does.
pub fn keep_window_min(
    ctx: &egui::Context,
    host: &mut HostState,
    tree: &Tree<Pane>,
    m: crate::tile_fit::Metrics,
    bars: egui::Vec2,
) {
    let monitor = ctx.input(|i| i.viewport().monitor_size);
    // Widths assume folding even in the layout editor (which doesn't fold),
    // so entering it never widens the window.
    let min = crate::tile_fit::window_min(tree, m, true, [bars.x, bars.y], monitor);
    let changed = host
        .min_window
        .is_none_or(|old| (old[0] - min[0]).abs() > 0.5 || (old[1] - min[1]).abs() > 0.5);
    if changed {
        host.min_window = Some(min);
        ctx.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(egui::vec2(
            min[0], min[1],
        )));
    }
}

/// Drawer slide time (doc §3.3 / §7: 120 ms ease-out).
const DRAWER_SECONDS: f32 = 0.12;

/// Draw the overlay drawers holding the panes `tile_fit::apply_folds`
/// folded this frame: servers + channels + user panel from the left,
/// members (+ user panel) from the right. A 40% scrim covers the rest; a
/// click on it or Esc closes the drawer; they slide in over 120 ms.
/// `area` is the region the tiles use.
pub fn draw_drawers(
    egui_ctx: &egui::Context,
    area: egui::Rect,
    folded: &crate::tile_fit::Folded,
    ctx: &mut PaneCtx<'_>,
) {
    use crate::responsive::{ASIDE_DRAWER_WIDTH, RAIL, SIDEBAR, USER_PANEL_HEIGHT};
    use crate::tile_fit::Drawer;
    let left = folded.in_drawer(Drawer::Left);
    let aside = folded.in_drawer(Drawer::Aside);
    // The user panel's content height plus its frame (blocky look).
    let user_h = USER_PANEL_HEIGHT + ctx.theme.pane_chrome().y;
    if left.is_empty() {
        ctx.state.host.left_open = false;
    }
    if aside.is_empty() {
        ctx.state.host.aside_open = false;
    }
    let any_open = ctx.state.host.left_open || ctx.state.host.aside_open;
    if any_open && egui_ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        ctx.state.host.left_open = false;
        ctx.state.host.aside_open = false;
    }
    let ease = egui::emath::easing::cubic_out;

    let left_id = egui::Id::unique("rc_left_drawer");
    let t = egui_ctx.animate_bool_with_time_and_easing(
        left_id,
        ctx.state.host.left_open,
        DRAWER_SECONDS,
        ease,
    );
    let has = |v: &[Pane], p| v.contains(&p);
    let rail_w = if has(&left, Pane::Servers) {
        RAIL.default
    } else {
        0.0
    };
    let left_w = rail_w + SIDEBAR.default;
    if t > 0.0
        && let Some(rect) = drawer(egui_ctx, left_id, area, left_w, t, true)
    {
        drawer_body(
            egui_ctx,
            left_id,
            rect,
            area,
            |ui, ctx| {
                let mut body = rect;
                if has(&left, Pane::Profile) {
                    let (b, user) = rect.split_top_bottom_at_y(rect.bottom() - user_h);
                    place(ui, user, Pane::Profile, ctx);
                    body = b;
                }
                let (ra, rb) = body.split_left_right_at_x(body.left() + rail_w);
                if rail_w > 0.0 {
                    place(ui, ra, Pane::Servers, ctx);
                }
                if has(&left, Pane::Channels) {
                    place(ui, rb, Pane::Channels, ctx);
                } else if rail_w <= 0.0 {
                    // Only the user panel folded: fill with the list colour.
                    ui.painter()
                        .rect_filled(rb, 0, ctx.theme.pane_fill(Pane::Channels));
                }
            },
            ctx,
        );
        if scrim_clicked(egui_ctx, left_id) {
            ctx.state.host.left_open = false;
        }
    }

    let aside_id = egui::Id::unique("rc_aside_drawer");
    let t = egui_ctx.animate_bool_with_time_and_easing(
        aside_id,
        ctx.state.host.aside_open,
        DRAWER_SECONDS,
        ease,
    );
    if t > 0.0
        && let Some(rect) = drawer(egui_ctx, aside_id, area, ASIDE_DRAWER_WIDTH, t, false)
    {
        drawer_body(
            egui_ctx,
            aside_id,
            rect,
            area,
            |ui, ctx| {
                let mut body = rect;
                if has(&aside, Pane::Profile) {
                    let (b, user) = rect.split_top_bottom_at_y(rect.bottom() - user_h);
                    place(ui, user, Pane::Profile, ctx);
                    body = b;
                }
                place(ui, body, Pane::Members, ctx);
            },
            ctx,
        );
        if scrim_clicked(egui_ctx, aside_id) {
            ctx.state.host.aside_open = false;
        }
    }
}

/// Draw `pane` into `rect` of `ui` (clipped).
fn place(ui: &mut egui::Ui, rect: egui::Rect, pane: Pane, ctx: &mut PaneCtx<'_>) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("rc_drawer_pane", pane.title()))
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    draw_pane(&mut child, pane, ctx);
}

/// Paint the scrim (40% black at `t` = 1) over `area` and return the
/// drawer's rect, sliding in from the left or right edge.
fn drawer(
    egui_ctx: &egui::Context,
    id: egui::Id,
    area: egui::Rect,
    w: f32,
    t: f32,
    left: bool,
) -> Option<egui::Rect> {
    let w = w.min(area.width());
    if w <= 0.0 {
        return None;
    }
    egui::Area::new(id.with("scrim"))
        .order(egui::Order::Middle)
        .fixed_pos(area.min)
        .constrain(false)
        .show(egui_ctx, |ui| {
            ui.set_clip_rect(area);
            let resp = ui.allocate_rect(area, Sense::click());
            ui.painter()
                .rect_filled(area, 0, Color32::from_black_alpha((102.0 * t) as u8));
            if resp.clicked() {
                egui_ctx.data_mut(|d| d.insert_temp(id.with("scrim_clicked"), true));
            }
        });
    let x = if left {
        area.left() - (1.0 - t) * w
    } else {
        area.right() - t * w
    };
    Some(egui::Rect::from_min_size(
        egui::pos2(x, area.top()),
        vec2(w, area.height()),
    ))
}

/// The drawer itself: an area above the scrim whose empty parts swallow
/// clicks.
fn drawer_body(
    egui_ctx: &egui::Context,
    id: egui::Id,
    rect: egui::Rect,
    area: egui::Rect,
    add: impl FnOnce(&mut egui::Ui, &mut PaneCtx<'_>),
    ctx: &mut PaneCtx<'_>,
) {
    let body = egui::Area::new(id.with("body"))
        .order(egui::Order::Middle)
        .fixed_pos(rect.min)
        .constrain(false)
        .show(egui_ctx, |ui| {
            ui.set_clip_rect(area);
            let _ = ui.allocate_rect(rect, Sense::click());
            // The gaps between framed panes.
            ctx.theme.paint_backdrop(ui.painter(), rect);
            add(ui, ctx);
        });
    // The scrim was shown first; keep the drawer above it.
    egui_ctx.move_to_top(body.response.layer_id);
}

fn scrim_clicked(egui_ctx: &egui::Context, id: egui::Id) -> bool {
    egui_ctx.data_mut(|d| {
        d.remove_temp::<bool>(id.with("scrim_clicked"))
            .unwrap_or(false)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frame_strips_surround_the_card_without_overlap() {
        let tile = egui::Rect::from_min_max([0.0, 0.0].into(), [100.0, 50.0].into());
        let card = tile.shrink(4.0);
        let strips = frame_strips(tile, 4.0);
        let area: f32 = strips.iter().map(|s| s.area()).sum();
        assert!((area + card.area() - tile.area()).abs() < 0.01);
        for s in strips {
            assert!(tile.contains_rect(s));
            assert!(!s.intersects(card.shrink(0.01)));
        }
    }
}
