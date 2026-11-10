//! The main window: its panes, and the column host that lays them out.
//!
//! The window is a row of columns (`layout_v3`): rail, sidebar, chat and
//! aside. [`host_columns`] draws the docked ones as pixel-width egui side
//! panels around a central chat panel (only the chat flexes), docks the
//! user panel, and lets the user drag the sidebar / aside dividers (pixel
//! widths, clamped per tier) and the rail edge (snaps between icons and
//! names). [`host_drawers`] draws the columns that `responsive::compute`
//! moved into overlay drawers on narrow windows.
//!
//! Panes draw through [`draw_pane`] / [`draw_column`], which only need a
//! `Ui` sized to the pane, so a framed or detached host (layout_v3 `Mode`)
//! can reuse them.

use eframe::egui::{self, Color32, Rect, Sense, Stroke, pos2, vec2};
use egui_tiles::{Container, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::app::State;
use crate::config::Config;
use crate::layout_v3::{Column, LayoutV3, UserPanelDock};
use crate::responsive::{self, ASIDE, Responsive, SIDEBAR, Slot, Tier, UserPanelSpot};
use crate::theme::Palette;

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
#[cfg(test)]
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

    // The server rail is icon-only like Discord's (~72 px at the default
    // window size); drag its edge wider to show server names.
    let lists = tiles.insert_horizontal_tile(vec![servers, channels]);
    set_shares(&mut tiles, lists, &[(servers, 0.72), (channels, 2.1)]);
    let left = tiles.insert_vertical_tile(vec![lists, profile]);
    set_shares(&mut tiles, left, &[(lists, 12.5), (profile, 1.0)]);

    let chat = tiles.insert_vertical_tile(vec![messages, composer]);
    set_shares(&mut tiles, chat, &[(messages, 9.0), (composer, 1.5)]);

    let root = tiles.insert_horizontal_tile(vec![left, chat, members]);
    set_shares(
        &mut tiles,
        root,
        &[(left, 2.82), (chat, 7.0), (members, 2.0)],
    );

    Tree::new(egui::Id::unique("rustcord_layout"), root, tiles)
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

/// A saved layout is only usable if it still has every panel (new versions
/// may add panels).
#[cfg(test)]
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
#[cfg(test)]
pub fn pane_visible(tree: &Tree<Pane>, pane: Pane) -> bool {
    tree.tiles
        .find_pane(&pane)
        .is_none_or(|id| tree.tiles.is_visible(id))
}

/// Show / hide a panel (Members, Channels). Hidden tiles keep their place in
/// the tree, so showing it again puts it back where it was (and the choice
/// is saved with the layout).
#[cfg(test)]
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
        Pane::Composer => egui::Margin {
            left: 16,
            right: 16,
            top: 0,
            bottom: 0,
        },
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

/// Everything a pane needs to draw itself, whatever hosts it.
pub struct PaneCtx<'a> {
    pub state: &'a mut State,
    pub config: &'a Config,
    pub pal: &'a Palette,
    /// The active theme (gradients, glass, chat background).
    pub theme: &'a crate::themes::ActiveTheme,
}

/// Draw one pane's content into `ui`: theme background, fill, margin, and
/// the pane itself, **clipped to `ui.max_rect()`** so nothing draws past
/// its edge (popups and menus are separate layers).
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
    egui::Frame::new()
        .fill(ctx.theme.pane_fill(pane))
        .inner_margin(pane_margin(pane))
        .show(ui, |ui| {
            ui.set_min_size(ui.available_size());
            ctx.state.pane_ui(ui, pane, ctx.config, ctx.pal);
        });
}

/// Draw a whole layout_v3 column into `ui` (container-agnostic, like
/// [`draw_pane`]). The chat column is one thing: the composer sits at its
/// bottom and grows upward with its content (44 px up to 40% of the
/// column, then it scrolls inside), the messages take the rest. The user
/// panel is docked by the host, not here.
pub fn draw_column(ui: &mut egui::Ui, column: Column, ctx: &mut PaneCtx<'_>) {
    match column {
        Column::Chat => {
            let rect = ui.max_rect();
            ui.set_clip_rect(rect.intersect(ui.clip_rect()));
            if ctx.state.composer_shown() {
                let h =
                    responsive::composer_height(ctx.state.composer_content_height(), rect.height());
                egui::Panel::bottom(ui.scope_id().with("chat_composer"))
                    .resizable(false)
                    .show_separator_line(false)
                    .frame(egui::Frame::NONE)
                    .exact_size(h)
                    .show(ui, |ui| draw_pane(ui, Pane::Composer, ctx));
            }
            egui::CentralPanel::no_frame().show(ui, |ui| draw_pane(ui, Pane::Messages, ctx));
        }
        other => {
            for &pane in other.panes() {
                draw_pane(ui, pane, ctx);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Column host (layout_v3 `Mode::Columns`)

/// Which tab a stacked rail + sidebar column shows (Wide chat).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StackTab {
    Servers,
    #[default]
    Channels,
}

/// The host's per-session state, kept in `State` so the chat and channel
/// headers can open / close drawers.
#[derive(Debug, Default)]
pub struct HostState {
    /// Last frame's width tier (breakpoint hysteresis).
    pub tier: Option<Tier>,
    /// Rail + sidebar live in the left drawer this frame (narrow window).
    pub lists_in_drawer: bool,
    /// The aside is a drawer this frame (below the docking width).
    pub aside_in_drawer: bool,
    /// The left drawer is open.
    pub left_open: bool,
    /// The aside drawer is open.
    pub aside_open: bool,
    /// Stacked rail + sidebar: the tab shown.
    pub stack_tab: StackTab,
    /// The chat header's LIST button was clicked (the app toggles the left
    /// drawer).
    pub toggle_left: bool,
}

/// Height of the stacked column's tab strip.
const STACK_TABS_HEIGHT: f32 = 40.0;
/// Width of a divider's grab area.
const DIVIDER_GRAB: f32 = 6.0;
/// Drawer slide time (doc §3.3 / §7: 120 ms ease-out).
const DRAWER_SECONDS: f32 = 0.12;

/// Side columns that are drawn as one panel: a single column, or the rail
/// and sidebar together when the user panel spans both (Discord).
#[derive(Debug, Clone, Copy, PartialEq)]
enum Block {
    One(Column, f32),
    Pair([(Column, f32); 2]),
}

impl Block {
    fn width(self) -> f32 {
        match self {
            Block::One(_, w) => w,
            Block::Pair([(_, a), (_, b)]) => a + b,
        }
    }

    fn has(self, c: Column) -> bool {
        match self {
            Block::One(x, _) => x == c,
            Block::Pair([(a, _), (b, _)]) => a == c || b == c,
        }
    }
}

/// Group `r.row` into blocks: the rail and sidebar become one block when the
/// user panel docks under both.
fn blocks(r: &Responsive) -> Vec<Block> {
    let span = r.user_panel == UserPanelSpot::Docked(UserPanelDock::RailAndSidebar);
    let lists = |x: Column| matches!(x, Column::Rail | Column::Sidebar);
    let mut out: Vec<Block> = Vec::with_capacity(r.row.len());
    for &(c, w) in &r.row {
        if span
            && lists(c)
            && let Some(Block::One(prev, pw)) = out.last().copied()
            && lists(prev)
            && prev != c
        {
            out.pop();
            out.push(Block::Pair([(prev, pw), (c, w)]));
            continue;
        }
        out.push(Block::One(c, w));
    }
    out
}

/// Does `block` carry the user panel (52 px at its bottom)?
fn hosts_user_panel(block: Block, r: &Responsive) -> bool {
    let UserPanelSpot::Docked(dock) = r.user_panel else {
        return false;
    };
    match (dock, block) {
        (UserPanelDock::RailAndSidebar, Block::Pair(_)) => true,
        (UserPanelDock::Sidebar, Block::One(c, _)) => c == Column::Sidebar,
        (UserPanelDock::Rail, Block::One(c, _)) => c == Column::Rail,
        (UserPanelDock::Aside, Block::One(c, _)) => c == Column::Aside,
        _ => false,
    }
}

/// Lay out the docked columns of `r` in `ui` (the area between the top bar
/// and the status bar): side columns are pixel-width egui side panels, the
/// chat is the central panel. With `resizable` (the Layout editor module),
/// the sidebar / aside dividers drag (pixel widths, clamped to the tier) and
/// the rail edge snaps between icons and names. Every column clips to its
/// own rect, so nothing overlaps.
pub fn host_columns(
    ui: &mut egui::Ui,
    layout: &mut LayoutV3,
    r: &Responsive,
    resizable: bool,
    ctx: &mut PaneCtx<'_>,
) {
    let blocks = blocks(r);
    let chat_at = blocks
        .iter()
        .position(|b| b.has(Column::Chat))
        .unwrap_or(blocks.len());
    let user_docked = blocks.iter().any(|&b| hosts_user_panel(b, r));
    // (column, its rect, is the chat to its right?) for the dividers.
    let mut edges: Vec<(Column, Rect, bool)> = Vec::new();
    for (i, &b) in blocks.iter().enumerate().take(chat_at) {
        let user = hosts_user_panel(b, r);
        let inner = side_panel(egui::Panel::left(("rc_column", i)), b)
            .show(ui, |ui| draw_block(ui, b, user, r.stacked, ctx))
            .inner;
        edges.extend(inner.into_iter().map(|(c, rect)| (c, rect, true)));
    }
    // Right of the chat: the outermost panel goes first.
    for (i, &b) in blocks.iter().enumerate().skip(chat_at + 1).rev() {
        let user = hosts_user_panel(b, r);
        let inner = side_panel(egui::Panel::right(("rc_column", i)), b)
            .show(ui, |ui| draw_block(ui, b, user, r.stacked, ctx))
            .inner;
        edges.extend(inner.into_iter().map(|(c, rect)| (c, rect, false)));
    }
    egui::CentralPanel::no_frame().show(ui, |ui| {
        if !user_docked && r.user_panel != UserPanelSpot::LeftDrawer {
            // Both lists hidden and the aside a drawer: the user panel
            // still never floats; it sits under the chat.
            egui::Panel::bottom("rc_user_panel_chat")
                .resizable(false)
                .show_separator_line(false)
                .frame(egui::Frame::NONE)
                .exact_size(responsive::USER_PANEL_HEIGHT)
                .show(ui, |ui| draw_pane(ui, Pane::Profile, ctx));
        }
        draw_column(ui, Column::Chat, ctx);
    });
    if resizable {
        for (column, rect, chat_right) in edges {
            divider(ui, layout, r, column, rect, chat_right, ctx.pal);
        }
    }
}

fn side_panel(panel: egui::Panel, b: Block) -> egui::Panel {
    panel
        .resizable(false)
        .show_separator_line(false)
        .frame(egui::Frame::NONE)
        .exact_size(b.width())
}

/// Draw one block into its panel; returns each column's rect.
fn draw_block(
    ui: &mut egui::Ui,
    block: Block,
    user_panel: bool,
    stacked: bool,
    ctx: &mut PaneCtx<'_>,
) -> Vec<(Column, Rect)> {
    let full = ui.max_rect();
    let body = if user_panel {
        let (body, user) =
            full.split_top_bottom_at_y(full.bottom() - responsive::USER_PANEL_HEIGHT);
        let mut child = ui.new_child(
            egui::UiBuilder::new()
                .id_salt("rc_user_panel")
                .max_rect(user),
        );
        draw_pane(&mut child, Pane::Profile, ctx);
        body
    } else {
        full
    };
    match block {
        Block::One(c, _) => {
            place_column(ui, body, c, stacked, ctx);
            vec![(c, full)]
        }
        Block::Pair([(a, wa), (b, _)]) => {
            let (ra, rb) = body.split_left_right_at_x(body.left() + wa);
            place_column(ui, ra, a, stacked, ctx);
            place_column(ui, rb, b, stacked, ctx);
            let (fa, fb) = full.split_left_right_at_x(full.left() + wa);
            vec![(a, fa), (b, fb)]
        }
    }
}

/// Draw `column` into `rect` of `ui`.
fn place_column(
    ui: &mut egui::Ui,
    rect: Rect,
    column: Column,
    stacked: bool,
    ctx: &mut PaneCtx<'_>,
) {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt(("rc_place", column))
            .max_rect(rect)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    child.set_clip_rect(rect.intersect(ui.clip_rect()));
    if column == Column::Sidebar && stacked {
        stacked_column(&mut child, ctx);
    } else {
        draw_column(&mut child, column, ctx);
    }
}

/// The stacked rail + sidebar column (Wide chat): a two-tab strip
/// (servers / channels) over the pane of the chosen tab.
fn stacked_column(ui: &mut egui::Ui, ctx: &mut PaneCtx<'_>) {
    let full = ui.max_rect();
    let (bar, body) = full.split_top_bottom_at_y(full.top() + STACK_TABS_HEIGHT);
    let pal = *ctx.pal;
    ctx.theme
        .paint_pane_background(ui.painter(), bar, Pane::Servers);
    ui.painter()
        .rect_filled(bar, 0, ctx.theme.pane_fill(Pane::Servers));
    let tabs = [
        (
            StackTab::Servers,
            Pane::Servers.icon(),
            Column::Rail.title(),
        ),
        (
            StackTab::Channels,
            Pane::Channels.icon(),
            Column::Sidebar.title(),
        ),
    ];
    let inner = bar.shrink2(vec2(8.0, 6.0));
    let w = inner.width() / tabs.len() as f32;
    for (i, (tab, icon, label)) in tabs.into_iter().enumerate() {
        let r = Rect::from_min_size(
            pos2(inner.left() + w * i as f32, inner.top()),
            vec2(w, inner.height()),
        )
        .shrink2(vec2(2.0, 0.0));
        let resp = ui
            .interact(r, ui.scope_id().with(("rc_stack_tab", i)), Sense::click())
            .on_hover_text(label);
        let on = ctx.state.host.stack_tab == tab;
        let fill = if on {
            pal.selected
        } else if resp.hovered() {
            pal.hover
        } else {
            Color32::TRANSPARENT
        };
        ui.painter().rect_filled(r, 4, fill);
        let color = if on { pal.strong } else { pal.muted };
        let text = crate::icons::label(icon, label);
        let galley = crate::app::truncated(
            ui,
            &text,
            egui::FontId::proportional(14.0),
            color,
            r.width() - 12.0,
        );
        ui.painter()
            .galley(r.center() - galley.size() / 2.0, galley, color);
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        if resp.clicked() {
            ctx.state.host.stack_tab = tab;
        }
    }
    let pane = match ctx.state.host.stack_tab {
        StackTab::Servers => Pane::Servers,
        StackTab::Channels => Pane::Channels,
    };
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .id_salt("rc_stack_body")
            .max_rect(body)
            .layout(egui::Layout::top_down(egui::Align::Min)),
    );
    draw_pane(&mut child, pane, ctx);
}

/// The drag handle on `column`'s chat-facing edge.
fn divider(
    ui: &mut egui::Ui,
    layout: &mut LayoutV3,
    r: &Responsive,
    column: Column,
    rect: Rect,
    chat_right: bool,
    pal: &Palette,
) {
    if column == Column::Chat || (column == Column::Rail && !r.tier.rail_names_allowed()) {
        return;
    }
    let x = if chat_right {
        rect.right()
    } else {
        rect.left()
    };
    let hit = Rect::from_x_y_ranges(
        x - DIVIDER_GRAB / 2.0..=x + DIVIDER_GRAB / 2.0,
        rect.y_range(),
    );
    let id = ui.scope_id().with(("rc_divider", column));
    let resp = ui.interact(hit, id, Sense::drag());
    let sign = if chat_right { 1.0 } else { -1.0 };
    let dx = resp.drag_delta().x * sign;
    let mut line_x = x;
    match column {
        Column::Sidebar | Column::Aside if resp.dragged() => {
            // Never past the tier's max, nor so far the chat drops below
            // its min width.
            let (size, tier_max) = if column == Column::Sidebar {
                (SIDEBAR, r.tier.sidebar_max())
            } else {
                (ASIDE, r.tier.aside_max())
            };
            let current = r.slot(column).width();
            let room = current + (r.chat_width - responsive::CHAT_MIN_WIDTH).max(0.0);
            let max = tier_max.min(room).max(size.min);
            let w = (current + dx).clamp(size.min, max);
            if column == Column::Sidebar {
                layout.sidebar_width = w;
            } else {
                layout.aside_width = w;
            }
        }
        Column::Rail => {
            // A ghost line follows the drag; on release the rail snaps to
            // icons or names (its only two widths).
            let start = r.rail.width();
            if resp.dragged() || resp.drag_stopped() {
                let w = ui.ctx().memory_mut(|m| {
                    let w = m.data.get_temp_mut_or_insert_with(id, || start);
                    *w = (*w + dx).clamp(responsive::RAIL.min, responsive::RAIL_NAMES.max);
                    *w
                });
                line_x = x + (w - start) * sign;
                if resp.drag_stopped() {
                    layout.rail = responsive::snap_rail(w);
                    ui.ctx().memory_mut(|m| m.data.remove::<f32>(id));
                }
            }
        }
        _ => {}
    }
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::ResizeHorizontal);
        let alpha = if resp.dragged() { 1.0 } else { 0.6 };
        ui.painter().vline(
            line_x,
            rect.y_range(),
            Stroke::new(2.0, pal.accent.gamma_multiply(alpha)),
        );
    }
}

/// The overlay drawers (narrow windows): rail + sidebar + user panel from
/// the left, the aside from the right. A 40% scrim covers the rest; a click
/// on it or Esc closes the drawer. They slide in over 120 ms (ease-out).
/// `area` is the region between the top bar and the status bar.
pub fn host_drawers(egui_ctx: &egui::Context, area: Rect, r: &Responsive, ctx: &mut PaneCtx<'_>) {
    if r.left_drawer.is_none() {
        ctx.state.host.left_open = false;
    }
    if r.aside_drawer.is_none() {
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
    if t > 0.0
        && let Some(w) = r.left_drawer
        && let Some(rect) = drawer(egui_ctx, left_id, area, w, t, true)
    {
        drawer_body(
            egui_ctx,
            left_id,
            rect,
            area,
            |ui, ctx| {
                let (body, user) =
                    rect.split_top_bottom_at_y(rect.bottom() - responsive::USER_PANEL_HEIGHT);
                let mut child = ui.new_child(
                    egui::UiBuilder::new()
                        .id_salt("rc_drawer_user")
                        .max_rect(user),
                );
                draw_pane(&mut child, Pane::Profile, ctx);
                if r.stacked {
                    place_column(ui, body, Column::Sidebar, true, ctx);
                } else {
                    let rail_w = r.rail.width().min(body.width());
                    let (ra, rb) = body.split_left_right_at_x(body.left() + rail_w);
                    if rail_w > 0.0 {
                        place_column(ui, ra, Column::Rail, false, ctx);
                    }
                    if r.sidebar != Slot::Hidden {
                        place_column(ui, rb, Column::Sidebar, false, ctx);
                    }
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
        && let Some(w) = r.aside_drawer
        && let Some(rect) = drawer(egui_ctx, aside_id, area, w, t, false)
    {
        drawer_body(
            egui_ctx,
            aside_id,
            rect,
            area,
            |ui, ctx| {
                place_column(ui, rect, Column::Aside, false, ctx);
            },
            ctx,
        );
        if scrim_clicked(egui_ctx, aside_id) {
            ctx.state.host.aside_open = false;
        }
    }
}

/// Paint the scrim (40% black at `t` = 1) over `area` and return the
/// drawer's rect, sliding in from the left or right edge.
fn drawer(
    egui_ctx: &egui::Context,
    id: egui::Id,
    area: Rect,
    w: f32,
    t: f32,
    left: bool,
) -> Option<Rect> {
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
    Some(Rect::from_min_size(
        pos2(x, area.top()),
        vec2(w, area.height()),
    ))
}

/// The drawer itself: an area above the scrim (same order, shown later, so
/// it's on top) whose empty parts swallow clicks.
fn drawer_body(
    egui_ctx: &egui::Context,
    id: egui::Id,
    rect: Rect,
    area: Rect,
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
            // Soft edge shadow towards the scrim.
            ui.painter()
                .rect_filled(rect.expand(1.0), 0, Color32::from_black_alpha(60));
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
