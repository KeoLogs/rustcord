//! The movable / resizable panel layout, built on egui_tiles.
//!
//! - Drag the line between two panels to resize them (always on).
//! - In layout edit mode each panel shows a title bar: drag it to move the
//!   panel somewhere else, or drop it onto another panel to stack them as tabs.
//! - The layout is saved between runs; "Reset layout" restores the default.

use eframe::egui;
use egui_tiles::{
    Behavior, Container, SimplificationOptions, Tabs, Tile, TileId, Tiles, Tree, UiResponse,
};
use serde::{Deserialize, Serialize};

use crate::app::State;

/// One movable part of the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Pane {
    Servers,
    Channels,
    Messages,
    Composer,
}

impl Pane {
    pub fn title(self) -> &'static str {
        match self {
            Pane::Servers => "Servers",
            Pane::Channels => "Channels",
            Pane::Messages => "Messages",
            Pane::Composer => "Message box",
        }
    }
}

/// Discord's default layout: servers | channels | (messages over message box).
pub fn default_tree() -> Tree<Pane> {
    let mut tiles = Tiles::default();

    // Wrap every pane in its own tab container; that container's tab bar is
    // the drag handle in edit mode.
    let tab = |tiles: &mut Tiles<Pane>, pane| {
        let id = tiles.insert_pane(pane);
        tiles.insert_tab_tile(vec![id])
    };
    let servers = tab(&mut tiles, Pane::Servers);
    let channels = tab(&mut tiles, Pane::Channels);
    let messages = tab(&mut tiles, Pane::Messages);
    let composer = tab(&mut tiles, Pane::Composer);

    let chat = tiles.insert_vertical_tile(vec![messages, composer]);
    set_shares(&mut tiles, chat, &[(messages, 9.0), (composer, 1.3)]);

    let root = tiles.insert_horizontal_tile(vec![servers, channels, chat]);
    set_shares(
        &mut tiles,
        root,
        &[(servers, 1.3), (channels, 2.0), (chat, 8.0)],
    );

    Tree::new(egui::Id::unique("rustcord_layout"), root, tiles)
}

fn set_shares(tiles: &mut Tiles<Pane>, container: TileId, shares: &[(TileId, f32)]) {
    if let Some(Tile::Container(Container::Linear(linear))) = tiles.get_mut(container) {
        for &(id, share) in shares {
            linear.shares.set_share(id, share);
        }
    }
}

/// Connects the tile tree to the app state.
pub struct LayoutBehavior<'a> {
    pub state: &'a mut State,
    pub editing: bool,
}

impl Behavior<Pane> for LayoutBehavior<'_> {
    fn pane_ui(&mut self, ui: &mut egui::Ui, _tile_id: TileId, pane: &mut Pane) -> UiResponse {
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            self.state.pane_ui(ui, *pane);
        });
        UiResponse::None
    }

    fn tab_title_for_pane(&mut self, pane: &Pane) -> egui::WidgetText {
        pane.title().into()
    }

    /// Title bars only show in edit mode, or when panels are stacked as tabs
    /// (otherwise the hidden tabs would be unreachable).
    fn is_tab_bar_visible(&self, _tile_id: TileId, tabs: &Tabs) -> bool {
        self.editing || tabs.children.len() > 1
    }

    fn is_tile_draggable(&self, _tiles: &Tiles<Pane>, _tile_id: TileId) -> bool {
        self.editing
    }

    fn gap_width(&self, _style: &egui::Style) -> f32 {
        2.0
    }

    fn min_size(&self) -> f32 {
        40.0
    }

    fn simplification_options(&self) -> SimplificationOptions {
        SimplificationOptions {
            all_panes_must_have_tabs: true,
            ..Default::default()
        }
    }
}
