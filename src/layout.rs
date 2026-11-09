//! The movable / resizable panel layout, built on egui_tiles.
//!
//! - Normal mode: panels are fixed (the dividers can still be dragged to
//!   resize, if the layout editor module is on).
//! - Edit mode (Settings → RustCord → Layout → Edit layout): hovering a panel
//!   makes it glow; press and hold to pick it up (it grows while you drag),
//!   then drop it on an edge of another panel to dock it there, or on its
//!   middle to stack them as tabs.
//! - The layout is saved between runs; "Reset layout" restores the default.

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

    pub fn icon(self) -> &'static str {
        match self {
            Pane::Servers => "🏠",
            Pane::Profile => "👤",
            Pane::Channels => "#",
            Pane::Messages => "💬",
            Pane::Composer => "✏",
            Pane::Members => "👥",
        }
    }
}

/// The default layout (Leo's sketch):
///
/// ```text
/// ┌────────┬──────────┬─────────────────────────┬─────────┐
/// │Servers │ Channels │ Messages                │ Members │
/// │        │          │                         │         │
/// ├────────┤          ├─────────────────────────┤         │
/// │Profile │          │ Message box             │         │
/// └────────┴──────────┴─────────────────────────┴─────────┘
/// ```
pub fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();

    // Wrap every pane in its own tab container (required by egui_tiles for
    // dragging; the tab bar itself is hidden unless panels are stacked).
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

    let left = tiles.insert_vertical_tile(vec![servers, profile]);
    set_shares(&mut tiles, left, &[(servers, 11.0), (profile, 1.5)]);

    let chat = tiles.insert_vertical_tile(vec![messages, composer]);
    set_shares(&mut tiles, chat, &[(messages, 9.0), (composer, 1.5)]);

    // The server rail is icon-only like Discord's (~72 px at the default
    // window size); drag its edge wider to show server names.
    let root = tiles.insert_horizontal_tile(vec![left, channels, chat, members]);
    set_shares(
        &mut tiles,
        root,
        &[(left, 0.72), (channels, 2.1), (chat, 7.0), (members, 2.0)],
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

/// Is the Members panel shown? (The 👥 button in the chat header hides it.)
pub fn members_visible(tree: &Tree<Pane>) -> bool {
    tree.tiles
        .find_pane(&Pane::Members)
        .is_none_or(|id| tree.tiles.is_visible(id))
}

/// Show / hide the Members panel. Hidden tiles keep their place in the
/// tree, so showing it again puts it back where it was (and the choice is
/// saved with the layout).
pub fn toggle_members(tree: &mut Tree<Pane>) {
    let Some(id) = tree.tiles.find_pane(&Pane::Members) else {
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
        Pane::Composer => egui::Margin {
            left: 16,
            right: 16,
            top: 0,
            bottom: 0,
        },
        Pane::Members => egui::Margin {
            left: 8,
            right: 4,
            top: 12,
            bottom: 0,
        },
        Pane::Profile => egui::Margin::symmetric(8, 6),
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

/// Connects the tile tree to the app state.
pub struct LayoutBehavior<'a> {
    pub state: &'a mut State,
    pub config: &'a Config,
    pub pal: Palette,
    pub editing: bool,
}

impl Behavior<Pane> for LayoutBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, tile_id: TileId, pane: &mut Pane) -> UiResponse {
        let rect = ui.max_rect();
        egui::Frame::new()
            .fill(self.pal.pane_fill(*pane))
            .inner_margin(pane_margin(*pane))
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                self.state.pane_ui(ui, *pane, self.config, &self.pal);
            });

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
            _ => ("▦", "Panels"),
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

    fn resize_stroke(&self, _style: &egui::Style, resize_state: egui_tiles::ResizeState) -> Stroke {
        match resize_state {
            egui_tiles::ResizeState::Idle => Stroke::NONE,
            egui_tiles::ResizeState::Hovering => {
                Stroke::new(2.0, self.pal.accent.gamma_multiply(0.6))
            }
            egui_tiles::ResizeState::Dragging => Stroke::new(2.0, self.pal.accent),
        }
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
