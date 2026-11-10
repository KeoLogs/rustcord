//! Layout presets for the panel layout (Settings → Layout), plus sharing a
//! layout as text.
//!
//! - [`Preset`] / [`PRESETS`]: ready-made arrangements of the six panels,
//!   built by [`build`] as an `egui_tiles::Tree<Pane>` in the same shape
//!   `layout::default_tree()` uses (every pane wrapped in a `Tabs`
//!   container, shares on the linear containers).
//! - [`preset_picker`]: a row of cards with a small vector schematic of each
//!   preset (as a `layout_v3::LayoutV3`). Returns the preset the user
//!   clicked; the caller swaps in `LayoutV3::from_preset`. [`paint_preview`]
//!   draws the same schematic for any v3 layout (the current one, an
//!   imported one).
//! - [`detect`]: which preset (if any) a tree still is, so the picker can
//!   highlight it. Hidden panels are ignored (the Members button only hides
//!   the panel); resizing a divider makes the layout "custom" (`None`).
//! - [`export`] / [`import`]: a small JSON document users can paste to each
//!   other. [`import`] goes through [`sanitize`], which rejects anything that
//!   isn't a well-formed tree with every panel exactly once.
//!
//! The "Discord Messenger (classic)" preset follows the arrangement of
//! Discord Messenger (MIT, by iProgramInCpp): narrow server column, channel
//! tree with the profile box under it, chat with the message box under it,
//! member list on the right.

use std::collections::HashSet;

use anyhow::{Context as _, Result, bail};
use eframe::egui::{self, Color32, Rect, Sense, Stroke, StrokeKind, Vec2, pos2, vec2};
use egui_tiles::{Container, LinearDir, Tile, TileId, Tiles, Tree};
use serde::{Deserialize, Serialize};

use crate::layout::Pane;
use crate::layout_v3::{Column, LayoutV3, UserPanelDock};
use crate::responsive::{self, UserPanelSpot};
use crate::theme::{Palette, bold_font};

/// The tree id `layout::default_tree()` uses. Every tree built or imported
/// here gets it, so swapping trees keeps egui state keyed the same way.
pub fn tree_id() -> egui::Id {
    egui::Id::unique("rustcord_layout")
}

/// A ready-made layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Preset {
    Discord,
    Messenger,
    Sketch,
    Compact,
    WideChat,
}

/// Name and one-line description of a preset, for the picker.
pub struct PresetInfo {
    pub preset: Preset,
    pub name: &'static str,
    pub description: &'static str,
}

pub const PRESETS: [PresetInfo; 5] = [
    PresetInfo {
        preset: Preset::Discord,
        name: "Discord",
        description: "Server rail and channels, profile underneath, members on the right.",
    },
    PresetInfo {
        preset: Preset::Messenger,
        name: "Discord Messenger",
        description: "Classic: full-height server column, profile under the channel tree.",
    },
    PresetInfo {
        preset: Preset::Sketch,
        name: "Leo's sketch",
        description: "Profile under the server rail, channels in their own column.",
    },
    PresetInfo {
        preset: Preset::Compact,
        name: "Compact",
        description: "Narrow channels and no member list (the members button brings it back).",
    },
    PresetInfo {
        preset: Preset::WideChat,
        name: "Wide chat",
        description: "Channels and servers stacked as tabs, so the chat gets the room.",
    },
];

impl Preset {
    pub const ALL: [Preset; 5] = [
        Preset::Discord,
        Preset::Messenger,
        Preset::Sketch,
        Preset::Compact,
        Preset::WideChat,
    ];

    pub fn info(self) -> &'static PresetInfo {
        PRESETS
            .iter()
            .find(|i| i.preset == self)
            .unwrap_or(&PRESETS[0])
    }

    pub fn name(self) -> &'static str {
        self.info().name
    }

    #[cfg(test)]
    pub fn description(self) -> &'static str {
        self.info().description
    }
}

// ---------------------------------------------------------------------------
// Building

/// Small helper so the presets read like the diagrams in their docs.
struct Builder {
    tiles: Tiles<Pane>,
}

impl Builder {
    fn new() -> Self {
        Self {
            tiles: Tiles::default(),
        }
    }

    /// A pane in its own tab container (egui_tiles needs that for dragging;
    /// the tab bar is hidden while there's only one tab).
    fn pane(&mut self, pane: Pane) -> TileId {
        let id = self.tiles.insert_pane(pane);
        self.tiles.insert_tab_tile(vec![id])
    }

    /// Several panes stacked as tabs; the first one is shown.
    fn tabs(&mut self, panes: &[Pane]) -> TileId {
        let ids: Vec<TileId> = panes.iter().map(|p| self.tiles.insert_pane(*p)).collect();
        self.tiles.insert_tab_tile(ids)
    }

    fn linear(&mut self, dir: LinearDir, items: &[(TileId, f32)]) -> TileId {
        let children: Vec<TileId> = items.iter().map(|(id, _)| *id).collect();
        let id = match dir {
            LinearDir::Horizontal => self.tiles.insert_horizontal_tile(children),
            LinearDir::Vertical => self.tiles.insert_vertical_tile(children),
        };
        if let Some(Tile::Container(Container::Linear(linear))) = self.tiles.get_mut(id) {
            for &(child, share) in items {
                linear.shares.set_share(child, share);
            }
        }
        id
    }

    fn row(&mut self, items: &[(TileId, f32)]) -> TileId {
        self.linear(LinearDir::Horizontal, items)
    }

    fn col(&mut self, items: &[(TileId, f32)]) -> TileId {
        self.linear(LinearDir::Vertical, items)
    }

    fn hide(&mut self, pane: Pane) {
        if let Some(id) = self.tiles.find_pane(&pane) {
            self.tiles.set_visible(id, false);
        }
    }

    fn finish(self, root: TileId) -> Tree<Pane> {
        Tree::new(tree_id(), root, self.tiles)
    }
}

/// Build the tile tree for a preset.
pub fn build(preset: Preset) -> Tree<Pane> {
    let mut b = Builder::new();
    match preset {
        // ┌──┬────────┬──────────────────┬───────┐
        // │Sv│Channels│ Messages         │Members│
        // ├──┴────────┼──────────────────┤       │
        // │ Profile   │ Message box      │       │
        // └───────────┴──────────────────┴───────┘
        Preset::Discord => {
            let servers = b.pane(Pane::Servers);
            let channels = b.pane(Pane::Channels);
            let profile = b.pane(Pane::Profile);
            let messages = b.pane(Pane::Messages);
            let composer = b.pane(Pane::Composer);
            let members = b.pane(Pane::Members);
            let lists = b.row(&[(servers, 0.72), (channels, 2.1)]);
            let left = b.col(&[(lists, 12.5), (profile, 1.0)]);
            let chat = b.col(&[(messages, 9.0), (composer, 1.5)]);
            let root = b.row(&[(left, 2.82), (chat, 7.0), (members, 2.0)]);
            b.finish(root)
        }
        // ┌──┬────────┬──────────────────┬───────┐
        // │Sv│Channels│ Messages         │Members│
        // │  ├────────┼──────────────────┤       │
        // │  │Profile │ Message box      │       │
        // └──┴────────┴──────────────────┴───────┘
        Preset::Messenger => {
            let servers = b.pane(Pane::Servers);
            let channels = b.pane(Pane::Channels);
            let profile = b.pane(Pane::Profile);
            let messages = b.pane(Pane::Messages);
            let composer = b.pane(Pane::Composer);
            let members = b.pane(Pane::Members);
            let tree = b.col(&[(channels, 9.0), (profile, 1.1)]);
            let chat = b.col(&[(messages, 9.0), (composer, 1.2)]);
            let root = b.row(&[(servers, 0.62), (tree, 2.0), (chat, 5.2), (members, 1.9)]);
            b.finish(root)
        }
        // ┌───────┬────────┬────────────────┬───────┐
        // │Servers│Channels│ Messages       │Members│
        // ├───────┤        ├────────────────┤       │
        // │Profile│        │ Message box    │       │
        // └───────┴────────┴────────────────┴───────┘
        Preset::Sketch => {
            let servers = b.pane(Pane::Servers);
            let profile = b.pane(Pane::Profile);
            let channels = b.pane(Pane::Channels);
            let messages = b.pane(Pane::Messages);
            let composer = b.pane(Pane::Composer);
            let members = b.pane(Pane::Members);
            let left = b.col(&[(servers, 11.0), (profile, 1.5)]);
            let chat = b.col(&[(messages, 9.0), (composer, 1.5)]);
            let root = b.row(&[(left, 0.72), (channels, 2.1), (chat, 7.0), (members, 2.0)]);
            b.finish(root)
        }
        // Discord's arrangement with narrower channels and the member list
        // hidden (it keeps its place, so the members button shows it again).
        Preset::Compact => {
            let servers = b.pane(Pane::Servers);
            let channels = b.pane(Pane::Channels);
            let profile = b.pane(Pane::Profile);
            let messages = b.pane(Pane::Messages);
            let composer = b.pane(Pane::Composer);
            let members = b.pane(Pane::Members);
            let lists = b.row(&[(servers, 0.72), (channels, 1.55)]);
            let left = b.col(&[(lists, 12.5), (profile, 1.0)]);
            let chat = b.col(&[(messages, 9.0), (composer, 1.5)]);
            let root = b.row(&[(left, 2.27), (chat, 7.0), (members, 1.8)]);
            b.hide(Pane::Members);
            b.finish(root)
        }
        // ┌─[Channels|Servers]─┬──────────────────────┬─────┐
        // │ Channels           │ Messages             │Memb.│
        // ├────────────────────┼──────────────────────┤     │
        // │ Profile            │ Message box          │     │
        // └────────────────────┴──────────────────────┴─────┘
        Preset::WideChat => {
            let lists = b.tabs(&[Pane::Channels, Pane::Servers]);
            let profile = b.pane(Pane::Profile);
            let messages = b.pane(Pane::Messages);
            let composer = b.pane(Pane::Composer);
            let members = b.pane(Pane::Members);
            let left = b.col(&[(lists, 12.5), (profile, 1.0)]);
            let chat = b.col(&[(messages, 9.0), (composer, 1.5)]);
            let root = b.row(&[(left, 2.3), (chat, 8.5), (members, 1.7)]);
            b.finish(root)
        }
    }
}

// ---------------------------------------------------------------------------
// Detecting

/// Which preset `tree` still matches (structure and divider positions;
/// hidden panels and the active tab are ignored). `None` = custom layout.
pub fn detect(tree: &Tree<Pane>) -> Option<Preset> {
    let root = tree.root?;
    Preset::ALL.into_iter().find(|&p| {
        let preset = build(p);
        preset
            .root
            .is_some_and(|proot| same_shape(tree, root, &preset, proot, 0))
    })
}

const MAX_DEPTH: usize = 32;
const SHARE_TOLERANCE: f32 = 0.01;

fn same_shape(a: &Tree<Pane>, a_id: TileId, b: &Tree<Pane>, b_id: TileId, depth: usize) -> bool {
    if depth > MAX_DEPTH {
        return false;
    }
    match (a.tiles.get(a_id), b.tiles.get(b_id)) {
        (Some(Tile::Pane(pa)), Some(Tile::Pane(pb))) => pa == pb,
        (
            Some(Tile::Container(Container::Tabs(ta))),
            Some(Tile::Container(Container::Tabs(tb))),
        ) => {
            ta.children.len() == tb.children.len()
                && ta
                    .children
                    .iter()
                    .zip(&tb.children)
                    .all(|(&x, &y)| same_shape(a, x, b, y, depth + 1))
        }
        (
            Some(Tile::Container(Container::Linear(la))),
            Some(Tile::Container(Container::Linear(lb))),
        ) => {
            if la.dir != lb.dir || la.children.len() != lb.children.len() {
                return false;
            }
            let fa = fractions(&la.shares, &la.children);
            let fb = fractions(&lb.shares, &lb.children);
            fa.iter()
                .zip(&fb)
                .all(|(x, y)| (x - y).abs() <= SHARE_TOLERANCE)
                && la
                    .children
                    .iter()
                    .zip(&lb.children)
                    .all(|(&x, &y)| same_shape(a, x, b, y, depth + 1))
        }
        _ => false,
    }
}

fn fractions(shares: &egui_tiles::Shares, children: &[TileId]) -> Vec<f32> {
    let total: f32 = children.iter().map(|&c| shares[c]).sum();
    let total = if total > 0.0 { total } else { 1.0 };
    children.iter().map(|&c| shares[c] / total).collect()
}

// ---------------------------------------------------------------------------
// Sharing (export / import)

/// Value of the `format` field in exported layouts.
pub const EXPORT_FORMAT: &str = "rustcord-layout";
/// Current export version; imports from newer versions are refused.
pub const EXPORT_VERSION: u32 = 1;
/// Pasted text larger than this is refused before parsing.
pub const MAX_IMPORT_BYTES: usize = 64 * 1024;
/// More tiles than this can't come from a real layout of six panels.
const MAX_TILES: usize = 64;

#[cfg(test)]
#[derive(Serialize)]
struct ExportDoc<'a> {
    format: &'a str,
    version: u32,
    tree: &'a Tree<Pane>,
}

#[derive(Deserialize)]
struct ImportDoc {
    format: String,
    version: u32,
    tree: Tree<Pane>,
}

/// The layout as pretty JSON, for copying to the clipboard / a file.
#[cfg(test)]
pub fn export(tree: &Tree<Pane>) -> String {
    let doc = ExportDoc {
        format: EXPORT_FORMAT,
        version: EXPORT_VERSION,
        tree,
    };
    // Serializing plain structs/maps to a String can't fail.
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// Parse a layout made by [`export`]. The result is checked by [`sanitize`],
/// so it is safe to hand straight to the layout.
pub fn import(text: &str) -> Result<Tree<Pane>> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Nothing to import: paste a layout first.");
    }
    if text.len() > MAX_IMPORT_BYTES {
        bail!("That's too long to be a RustCord layout.");
    }
    let doc: ImportDoc =
        serde_json::from_str(text).context("This isn't a RustCord layout (couldn't read it)")?;
    if doc.format != EXPORT_FORMAT {
        bail!("This isn't a RustCord layout (format \"{}\").", doc.format);
    }
    if doc.version > EXPORT_VERSION {
        bail!(
            "This layout was made by a newer RustCord (version {}).",
            doc.version
        );
    }
    sanitize(&doc.tree)
}

/// Check a tree and return a clean copy:
///
/// - the root exists, every container's children exist, and no tile is
///   reachable twice (no cycles / shared children);
/// - only tab and row/column containers (no grids), none empty;
/// - divider shares are finite and not negative;
/// - every [`Pane`] appears exactly once;
/// - unreachable tiles are dropped, a tab container's active tab is one of
///   its children, and only the Members panel may stay hidden (it's the only
///   one the UI can hide and show again).
///
/// The copy gets [`tree_id`] and fresh bookkeeping (next tile id).
pub fn sanitize(tree: &Tree<Pane>) -> Result<Tree<Pane>> {
    let root = tree.root.context("The layout is empty.")?;
    if tree.tiles.len() > MAX_TILES {
        bail!("The layout has too many tiles.");
    }

    let mut seen: HashSet<TileId> = HashSet::new();
    let mut order: Vec<TileId> = Vec::new();
    let mut panes: Vec<Pane> = Vec::new();
    let mut stack = vec![(root, 0usize)];
    while let Some((id, depth)) = stack.pop() {
        if depth > MAX_DEPTH {
            bail!("The layout is nested too deeply.");
        }
        let Some(tile) = tree.tiles.get(id) else {
            bail!("The layout refers to a missing tile.");
        };
        if !seen.insert(id) {
            bail!("The layout uses the same tile twice.");
        }
        order.push(id);
        match tile {
            Tile::Pane(p) => panes.push(*p),
            Tile::Container(Container::Tabs(tabs)) => {
                if tabs.children.is_empty() {
                    bail!("The layout has an empty tab group.");
                }
                stack.extend(tabs.children.iter().rev().map(|&c| (c, depth + 1)));
            }
            Tile::Container(Container::Linear(linear)) => {
                if linear.children.is_empty() {
                    bail!("The layout has an empty row or column.");
                }
                for &c in &linear.children {
                    let share = linear.shares[c];
                    if !share.is_finite() || share < 0.0 {
                        bail!("The layout has an invalid panel size.");
                    }
                }
                stack.extend(linear.children.iter().rev().map(|&c| (c, depth + 1)));
            }
            Tile::Container(Container::Grid(_)) => {
                bail!("Grid layouts aren't supported.");
            }
        }
    }

    for pane in Pane::ALL {
        match panes.iter().filter(|p| **p == pane).count() {
            0 => bail!("The layout is missing the {} panel.", pane.title()),
            1 => {}
            _ => bail!("The layout has the {} panel more than once.", pane.title()),
        }
    }

    let mut tiles = Tiles::default();
    for &id in &order {
        let Some(mut tile) = tree.tiles.get(id).cloned() else {
            continue;
        };
        if let Tile::Container(Container::Tabs(tabs)) = &mut tile
            && tabs.active.is_none_or(|a| !tabs.children.contains(&a))
        {
            tabs.active = tabs.children.first().copied();
        }
        let hidden = !tree.tiles.is_visible(id) && matches!(tile, Tile::Pane(Pane::Members));
        tiles.insert(id, tile);
        if hidden {
            tiles.set_visible(id, false);
        }
    }
    tiles.recompute_next_tile_id();
    Ok(Tree::new(tree_id(), root, tiles))
}

// ---------------------------------------------------------------------------
// Schematic previews

/// One rectangle of a schematic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PreviewPart {
    Pane(Pane),
    /// The tab strip of stacked panels: `count` tabs, `active` is shown.
    TabBar {
        count: usize,
        active: usize,
    },
}

/// Window size the schematic is laid out at (then scaled into the rect):
/// wide enough that every column is docked and the rail may show names.
const PREVIEW_WINDOW: Vec2 = vec2(1280.0, 800.0);

/// Lay out `layout` inside `rect` like the column host would at a 1280×800
/// window (docked columns, user panel, stacked tabs, composer), without any
/// UI. Hidden columns get no space; the parts tile `rect` exactly.
pub fn schematic(layout: &LayoutV3, rect: Rect) -> Vec<(PreviewPart, Rect)> {
    let r = responsive::compute(layout, PREVIEW_WINDOW.x, None);
    let sx = rect.width() / PREVIEW_WINDOW.x;
    let sy = rect.height() / PREVIEW_WINDOW.y;
    // The 52 px user panel and composer would be slivers at preview size.
    let user_h = (responsive::USER_PANEL_HEIGHT * sy).max(rect.height() * 0.1);
    let composer_h = (64.0 * sy).max(rect.height() * 0.12);
    let tabs_h = (40.0 * sy).max(rect.height() * 0.08);

    let mut out = Vec::new();
    let mut x = rect.left();
    let mut cols: Vec<(Column, Rect)> = Vec::new();
    for (i, &(c, w)) in r.row.iter().enumerate() {
        // The last column takes the rest (no rounding gap).
        let right = if i + 1 == r.row.len() {
            rect.right()
        } else {
            x + w * sx
        };
        cols.push((c, Rect::from_x_y_ranges(x..=right, rect.y_range())));
        x = right;
    }
    // The user panel's x range: under its dock column(s).
    let dock_cols: &[Column] = match r.user_panel {
        UserPanelSpot::Docked(UserPanelDock::RailAndSidebar) => &[Column::Rail, Column::Sidebar],
        UserPanelSpot::Docked(UserPanelDock::Sidebar) => &[Column::Sidebar],
        UserPanelSpot::Docked(UserPanelDock::Rail) => &[Column::Rail],
        UserPanelSpot::Docked(UserPanelDock::Aside) => &[Column::Aside],
        UserPanelSpot::LeftDrawer => &[],
    };
    let mut user_x: Option<(f32, f32)> = None;
    for &(c, cr) in &cols {
        if dock_cols.contains(&c) {
            user_x = Some(match user_x {
                Some((a, b)) => (a.min(cr.left()), b.max(cr.right())),
                None => (cr.left(), cr.right()),
            });
        }
    }
    let user_top = rect.bottom() - user_h;
    for (c, cr) in cols {
        let mut body = cr;
        if dock_cols.contains(&c) && user_x.is_some() {
            body.max.y = user_top;
        }
        match c {
            Column::Rail => out.push((PreviewPart::Pane(Pane::Servers), body)),
            Column::Sidebar if r.stacked => {
                let (bar, rest) = body.split_top_bottom_at_y(body.top() + tabs_h);
                out.push((
                    PreviewPart::TabBar {
                        count: 2,
                        active: 1,
                    },
                    bar,
                ));
                out.push((PreviewPart::Pane(Pane::Channels), rest));
            }
            Column::Sidebar => out.push((PreviewPart::Pane(Pane::Channels), body)),
            Column::Aside => out.push((PreviewPart::Pane(Pane::Members), body)),
            Column::Chat => {
                let (msgs, comp) = body.split_top_bottom_at_y(body.bottom() - composer_h);
                out.push((PreviewPart::Pane(Pane::Messages), msgs));
                out.push((PreviewPart::Pane(Pane::Composer), comp));
            }
        }
    }
    if let Some((a, b)) = user_x {
        out.push((
            PreviewPart::Pane(Pane::Profile),
            Rect::from_x_y_ranges(a..=b, user_top..=rect.bottom()),
        ));
    }
    out
}

/// Draw a small vector schematic of `layout` into `rect`: each column in its
/// real background colour with a few shapes hinting at its content.
pub fn paint_preview(painter: &egui::Painter, pal: &Palette, layout: &LayoutV3, rect: Rect) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    painter.rect_filled(rect, 4, pal.servers);
    for (part, r) in schematic(layout, rect) {
        match part {
            PreviewPart::Pane(pane) => paint_pane(&painter, pal, pane, r),
            PreviewPart::TabBar { count, active } => paint_tab_bar(&painter, pal, r, count, active),
        }
    }
    painter.rect_stroke(rect, 4, Stroke::new(1.0, pal.divider), StrokeKind::Inside);
}

fn line_color(pal: &Palette) -> Color32 {
    pal.muted.gamma_multiply(0.55)
}

/// A short rounded "text line" placeholder.
fn text_line(painter: &egui::Painter, left: f32, y: f32, width: f32, h: f32, color: Color32) {
    if width <= 0.5 {
        return;
    }
    let r = Rect::from_min_size(pos2(left, y - h / 2.0), vec2(width, h));
    painter.rect_filled(r, h / 2.0, color);
}

fn paint_tab_bar(painter: &egui::Painter, pal: &Palette, r: Rect, count: usize, active: usize) {
    painter.rect_filled(r, 0, pal.servers);
    let count = count.max(1);
    let tab_w = (r.width() - 2.0) / count as f32;
    for i in 0..count {
        let x = r.left() + 1.0 + tab_w * i as f32;
        let tab = Rect::from_min_max(
            pos2(x + 0.5, r.top() + 1.0),
            pos2(x + tab_w - 0.5, r.bottom()),
        );
        let fill = if i == active { pal.sidebar } else { pal.hover };
        painter.rect_filled(tab, 1, fill);
        text_line(
            painter,
            tab.left() + tab.width() * 0.2,
            tab.center().y,
            tab.width() * 0.5,
            (tab.height() * 0.35).clamp(1.0, 3.0),
            line_color(pal),
        );
    }
}

fn paint_pane(painter: &egui::Painter, pal: &Palette, pane: Pane, r: Rect) {
    painter.rect_filled(r, 0, pal.pane_fill(pane));
    let lines = line_color(pal);
    let pad = (r.width().min(r.height()) * 0.12).clamp(1.5, 6.0);
    match pane {
        Pane::Servers => {
            // A column of round server icons, the first one highlighted.
            let d = (r.width() * 0.62).clamp(2.0, 11.0);
            let step = d + (d * 0.45).max(2.0);
            let mut y = r.top() + pad + d / 2.0;
            let x = r.center().x;
            let mut i = 0;
            while y + d / 2.0 <= r.bottom() - pad {
                let fill = if i == 0 { pal.accent } else { pal.rail_icon };
                painter.circle_filled(pos2(x, y), d / 2.0, fill);
                if i == 0 {
                    // Separator under the home button.
                    let sy = y + d / 2.0 + (step - d) / 2.0;
                    painter.line_segment(
                        [pos2(x - d * 0.3, sy), pos2(x + d * 0.3, sy)],
                        Stroke::new(1.0, pal.divider),
                    );
                }
                y += step;
                i += 1;
            }
        }
        Pane::Channels => {
            let header = header_strip(painter, pal, r);
            let h = (r.height() * 0.025).clamp(1.5, 3.0);
            let step = (h * 3.2).max(5.0);
            let mut y = header + step * 0.8;
            let widths = [0.35, 0.6, 0.5, 0.7, 0.45, 0.55, 0.65, 0.4];
            let mut i = 0;
            while y + h <= r.bottom() - pad {
                let w = widths[i % widths.len()];
                if i == 1 {
                    let sel = Rect::from_min_max(
                        pos2(r.left() + pad * 0.6, y - step * 0.4),
                        pos2(r.right() - pad * 0.6, y + step * 0.4),
                    );
                    painter.rect_filled(sel, 1, pal.selected);
                }
                let indent = if i % 4 == 0 { 0.0 } else { pad };
                text_line(
                    painter,
                    r.left() + pad + indent,
                    y,
                    (r.width() - 2.0 * pad - indent) * w,
                    h,
                    lines,
                );
                y += step;
                i += 1;
            }
        }
        Pane::Messages => {
            let header = header_strip(painter, pal, r);
            let av = (r.height() * 0.06).clamp(2.0, 6.0);
            let step = av * 3.0;
            let h = (av * 0.45).max(1.0);
            let mut y = r.bottom() - pad - av;
            let widths = [0.55, 0.35, 0.7, 0.45, 0.6];
            let mut i = 0;
            // Newest at the bottom, like the real chat.
            while y - av >= header + pad * 0.5 {
                let x = r.left() + pad + av;
                painter.circle_filled(pos2(x, y), av, pal.rail_icon);
                let tx = x + av * 1.8;
                let tw = r.right() - pad - tx;
                text_line(
                    painter,
                    tx,
                    y - av * 0.45,
                    tw * 0.25,
                    h,
                    pal.strong.gamma_multiply(0.45),
                );
                text_line(
                    painter,
                    tx,
                    y + av * 0.45,
                    tw * widths[i % widths.len()],
                    h,
                    lines,
                );
                y -= step;
                i += 1;
            }
        }
        Pane::Composer => {
            let inner = r.shrink2(vec2(pad * 1.5, (r.height() * 0.2).max(1.0)));
            if inner.is_positive() {
                painter.rect_filled(inner, inner.height().min(6.0) / 2.0, pal.input);
                let h = (inner.height() * 0.22).clamp(1.0, 3.0);
                text_line(
                    painter,
                    inner.left() + inner.height() * 0.6,
                    inner.center().y,
                    inner.width() * 0.3,
                    h,
                    lines,
                );
            }
        }
        Pane::Members => {
            let av = (r.width() * 0.08).clamp(1.5, 4.5);
            let step = av * 3.0;
            let h = (av * 0.55).max(1.0);
            let mut y = r.top() + pad + av * 2.0;
            text_line(
                painter,
                r.left() + pad,
                r.top() + pad + h,
                r.width() * 0.35,
                h,
                lines,
            );
            let widths = [0.5, 0.35, 0.6, 0.45];
            let mut i = 0;
            while y + av <= r.bottom() - pad {
                let x = r.left() + pad + av;
                painter.circle_filled(pos2(x, y), av, pal.rail_icon);
                painter.circle_filled(pos2(x + av * 0.7, y + av * 0.7), av * 0.35, pal.online);
                let tx = x + av * 1.8;
                text_line(
                    painter,
                    tx,
                    y,
                    (r.right() - pad - tx) * widths[i % widths.len()],
                    h,
                    lines,
                );
                y += step;
                i += 1;
            }
        }
        Pane::Profile => {
            let av = (r.height() * 0.28).clamp(1.5, 6.0);
            let x = r.left() + pad + av;
            let y = r.center().y;
            painter.circle_filled(pos2(x, y), av, pal.rail_icon);
            painter.circle_filled(pos2(x + av * 0.7, y + av * 0.7), av * 0.38, pal.online);
            let tx = x + av * 1.8;
            let h = (av * 0.4).max(1.0);
            text_line(
                painter,
                tx,
                y - av * 0.35,
                (r.right() - pad - tx) * 0.45,
                h,
                pal.strong.gamma_multiply(0.45),
            );
            text_line(
                painter,
                tx,
                y + av * 0.45,
                (r.right() - pad - tx) * 0.3,
                h,
                lines,
            );
        }
    }
}

/// The header bar channels and messages draw at their top; returns its
/// bottom edge.
fn header_strip(painter: &egui::Painter, pal: &Palette, r: Rect) -> f32 {
    let h = (r.height() * 0.09).clamp(3.0, 14.0);
    let bottom = r.top() + h;
    painter.line_segment(
        [pos2(r.left(), bottom), pos2(r.right(), bottom)],
        Stroke::new(1.0, pal.divider),
    );
    let pad = (r.width() * 0.08).clamp(1.5, 6.0);
    text_line(
        painter,
        r.left() + pad,
        r.top() + h / 2.0,
        r.width() * 0.3,
        (h * 0.3).max(1.0),
        pal.strong.gamma_multiply(0.45),
    );
    bottom
}

// ---------------------------------------------------------------------------
// Picker widget

const CARD_W: f32 = 196.0;
const CARD_PAD: f32 = 10.0;
const PREVIEW_H: f32 = 104.0;
const CARD_H: f32 = 208.0;

/// A wrapping row of preset cards with schematic previews. `current` is
/// highlighted (`LayoutV3::detect` on the live layout). Returns the preset
/// the user clicked; the caller swaps in `LayoutV3::from_preset` and saves it.
pub fn preset_picker(ui: &mut egui::Ui, pal: &Palette, current: Option<Preset>) -> Option<Preset> {
    let mut picked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(12.0, 12.0);
        for info in &PRESETS {
            if preset_card(ui, pal, info, current == Some(info.preset)).clicked() {
                picked = Some(info.preset);
            }
        }
    });
    picked
}

fn preset_card(
    ui: &mut egui::Ui,
    pal: &Palette,
    info: &PresetInfo,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(CARD_W, CARD_H), Sense::click());
    resp.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::RadioButton, true, selected, info.name)
    });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    if !ui.is_rect_visible(rect) {
        return resp;
    }

    let painter = ui.painter_at(rect);
    let fill = if selected {
        pal.selected
    } else if resp.hovered() {
        pal.hover
    } else {
        pal.sidebar
    };
    painter.rect_filled(rect, 8, fill);
    let stroke = if selected {
        Stroke::new(2.0, pal.accent)
    } else if resp.has_focus() {
        Stroke::new(1.0, pal.accent)
    } else {
        Stroke::new(1.0, pal.divider)
    };
    painter.rect_stroke(rect, 8, stroke, StrokeKind::Inside);

    let preview = Rect::from_min_size(
        rect.min + vec2(CARD_PAD, CARD_PAD),
        vec2(CARD_W - 2.0 * CARD_PAD, PREVIEW_H),
    );
    paint_preview(&painter, pal, &LayoutV3::from_preset(info.preset), preview);

    let text_w = CARD_W - 2.0 * CARD_PAD;
    let mut y = preview.bottom() + 8.0;
    let name = painter.layout_no_wrap(info.name.to_owned(), bold_font(15.0), pal.strong);
    painter.galley(pos2(rect.left() + CARD_PAD, y), name.clone(), pal.strong);
    if selected {
        // A small check mark at the end of the name (vector, no emoji).
        let c = pos2(rect.right() - CARD_PAD - 7.0, y + name.size().y / 2.0);
        painter.circle_filled(c, 7.0, pal.accent);
        painter.line_segment(
            [c + vec2(-3.5, 0.0), c + vec2(-1.0, 2.5)],
            Stroke::new(1.6, Color32::WHITE),
        );
        painter.line_segment(
            [c + vec2(-1.0, 2.5), c + vec2(3.5, -2.5)],
            Stroke::new(1.6, Color32::WHITE),
        );
    }
    y += name.size().y + 2.0;
    let desc = painter.layout(
        info.description.to_owned(),
        egui::FontId::proportional(13.0),
        pal.muted,
        text_w,
    );
    painter.galley(pos2(rect.left() + CARD_PAD, y), desc, pal.muted);
    resp
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn reachable_panes(tree: &Tree<Pane>) -> Vec<Pane> {
        let mut out = Vec::new();
        let mut stack = vec![tree.root.expect("root")];
        while let Some(id) = stack.pop() {
            match tree.tiles.get(id).expect("tile exists") {
                Tile::Pane(p) => out.push(*p),
                Tile::Container(c) => stack.extend(c.children().copied()),
            }
        }
        out
    }

    #[test]
    fn every_preset_has_every_pane_exactly_once() {
        for p in Preset::ALL {
            let tree = build(p);
            let all: Vec<Pane> = tree
                .tiles
                .iter()
                .filter_map(|(_, t)| match t {
                    Tile::Pane(p) => Some(*p),
                    Tile::Container(_) => None,
                })
                .collect();
            let reach = reachable_panes(&tree);
            for pane in Pane::ALL {
                assert_eq!(
                    all.iter().filter(|x| **x == pane).count(),
                    1,
                    "{p:?} {pane:?}"
                );
                assert_eq!(
                    reach.iter().filter(|x| **x == pane).count(),
                    1,
                    "{p:?} {pane:?}"
                );
            }
            assert!(crate::layout::is_complete(&tree));
            assert!(sanitize(&tree).is_ok(), "{p:?} must pass sanitize");
        }
    }

    #[test]
    fn presets_metadata_matches_all() {
        assert_eq!(PRESETS.len(), Preset::ALL.len());
        for (info, p) in PRESETS.iter().zip(Preset::ALL) {
            assert_eq!(info.preset, p);
            assert_eq!(p.name(), info.name);
            assert!(!p.description().is_empty());
        }
    }

    #[test]
    fn detect_finds_each_preset() {
        for p in Preset::ALL {
            assert_eq!(detect(&build(p)), Some(p));
        }
    }

    #[test]
    fn default_tree_is_a_preset() {
        assert!(detect(&crate::layout::default_tree()).is_some());
    }

    #[test]
    fn detect_ignores_hidden_members_but_not_resizes() {
        let mut tree = build(Preset::Discord);
        crate::layout::toggle_pane(&mut tree, Pane::Members);
        assert!(!crate::layout::pane_visible(&tree, Pane::Members));
        assert_eq!(detect(&tree), Some(Preset::Discord));

        let root = tree.root.unwrap();
        if let Some(Tile::Container(Container::Linear(l))) = tree.tiles.get_mut(root) {
            let first = l.children[0];
            l.shares.set_share(first, 5.0);
        }
        assert_eq!(detect(&tree), None);
    }

    #[test]
    fn compact_hides_members_and_wide_chat_stacks_tabs() {
        let compact = build(Preset::Compact);
        assert!(
            compact
                .tiles
                .find_pane(&Pane::Members)
                .is_some_and(|id| !compact.tiles.is_visible(id))
        );
        let parts = schematic(
            &LayoutV3::from_preset(Preset::Compact),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0)),
        );
        assert!(!parts.contains(&(PreviewPart::Pane(Pane::Members), Rect::NOTHING)));
        assert!(
            parts
                .iter()
                .all(|(p, _)| *p != PreviewPart::Pane(Pane::Members))
        );

        let parts = schematic(
            &LayoutV3::from_preset(Preset::WideChat),
            Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0)),
        );
        assert!(parts.iter().any(|(p, _)| matches!(
            p,
            PreviewPart::TabBar {
                count: 2,
                active: 1
            }
        )));
        // Servers is the inactive tab, so it isn't drawn.
        assert!(
            parts
                .iter()
                .all(|(p, _)| *p != PreviewPart::Pane(Pane::Servers))
        );
    }

    #[test]
    fn schematic_tiles_the_whole_rect_without_overlap() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(800.0, 500.0));
        for p in Preset::ALL {
            let parts = schematic(&LayoutV3::from_preset(p), rect);
            let area: f32 = parts.iter().map(|(_, r)| r.area()).sum();
            assert!(
                (area - rect.area()).abs() < 1.0,
                "{p:?}: {area} vs {}",
                rect.area()
            );
            for (i, (_, a)) in parts.iter().enumerate() {
                assert!(rect.expand(0.01).contains_rect(*a), "{p:?}");
                for (_, b) in &parts[i + 1..] {
                    assert!(a.intersect(*b).area() < 0.5, "{p:?}: {a:?} overlaps {b:?}");
                }
            }
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 0.01
    }

    #[test]
    fn messenger_matches_the_classic_arrangement() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1000.0, 600.0));
        let parts = schematic(&LayoutV3::from_preset(Preset::Messenger), rect);
        let at = |pane| {
            parts
                .iter()
                .find(|(p, _)| *p == PreviewPart::Pane(pane))
                .map(|(_, r)| *r)
                .unwrap()
        };
        let (servers, channels, profile) =
            (at(Pane::Servers), at(Pane::Channels), at(Pane::Profile));
        let (messages, composer, members) =
            (at(Pane::Messages), at(Pane::Composer), at(Pane::Members));
        // Full-height narrow server column on the far left.
        assert!(near(servers.left(), 0.0));
        assert!(near(servers.height(), 600.0));
        assert!(servers.width() < 70.0);
        // Profile under the channel tree, in the same column.
        assert!(near(profile.left(), channels.left()) && near(profile.right(), channels.right()));
        assert!(profile.top() >= channels.bottom() - 0.01);
        // Message box under the messages; members full height on the right.
        assert!(near(composer.left(), messages.left()) && near(composer.right(), messages.right()));
        assert!(composer.top() >= messages.bottom() - 0.01);
        assert!(near(members.right(), 1000.0));
        assert!(near(members.height(), 600.0));
    }

    #[test]
    fn export_import_round_trip() {
        for p in Preset::ALL {
            let tree = build(p);
            let text = export(&tree);
            let back = import(&text).expect("import");
            assert!(back == tree, "{p:?} round trip");
            assert_eq!(detect(&back), Some(p));
        }
        // Hidden members and a resized divider survive too.
        let mut tree = build(Preset::Sketch);
        crate::layout::toggle_pane(&mut tree, Pane::Members);
        let root = tree.root.unwrap();
        if let Some(Tile::Container(Container::Linear(l))) = tree.tiles.get_mut(root) {
            let c = l.children[1];
            l.shares.set_share(c, 3.25);
        }
        let back = import(&export(&tree)).unwrap();
        assert!(back == tree);
        assert!(!crate::layout::pane_visible(&back, Pane::Members));
    }

    fn export_raw(tree: &Tree<Pane>) -> String {
        serde_json::to_string(&ExportDoc {
            format: EXPORT_FORMAT,
            version: EXPORT_VERSION,
            tree,
        })
        .unwrap()
    }

    fn first_linear(tree: &mut Tree<Pane>) -> &mut egui_tiles::Linear {
        let root = tree.root.unwrap();
        match tree.tiles.get_mut(root) {
            Some(Tile::Container(Container::Linear(l))) => l,
            _ => panic!("root is linear"),
        }
    }

    #[test]
    fn bad_imports_are_rejected() {
        assert!(import("").is_err());
        assert!(import("not json at all").is_err());
        assert!(import("{}").is_err());
        assert!(import(&"x".repeat(MAX_IMPORT_BYTES + 1)).is_err());

        let good = export(&build(Preset::Discord));
        assert!(import(&good.replace(EXPORT_FORMAT, "something-else")).is_err());
        assert!(import(&good.replace("\"version\": 1", "\"version\": 99")).is_err());
        assert!(import(&good.replace("\"Members\"", "\"Nonsense\"")).is_err());

        // Missing panel.
        let mut tree = build(Preset::Discord);
        let id = tree.tiles.find_pane(&Pane::Members).unwrap();
        tree.tiles.insert(id, Tile::Pane(Pane::Channels));
        let err = import(&export_raw(&tree)).unwrap_err().to_string();
        assert!(err.contains("Members") || err.contains("Channels"), "{err}");

        // Dangling child.
        let mut tree = build(Preset::Discord);
        first_linear(&mut tree).children.push(TileId::from_u64(999));
        assert!(import(&export_raw(&tree)).is_err());

        // Cycle: the root contains itself.
        let mut tree = build(Preset::Discord);
        let root = tree.root.unwrap();
        first_linear(&mut tree).children.push(root);
        assert!(import(&export_raw(&tree)).is_err());

        // Negative share.
        let mut tree = build(Preset::Discord);
        let l = first_linear(&mut tree);
        let c = l.children[0];
        l.shares.set_share(c, -1.0);
        assert!(import(&export_raw(&tree)).is_err());

        // Empty container.
        let mut tree = build(Preset::Discord);
        first_linear(&mut tree).children.clear();
        assert!(import(&export_raw(&tree)).is_err());

        // No root.
        let mut tree = build(Preset::Discord);
        tree.root = None;
        assert!(import(&export_raw(&tree)).is_err());
    }

    #[test]
    fn sanitize_cleans_up() {
        let mut tree = build(Preset::Discord);
        // Hide the message panel (not allowed) and members (allowed).
        let msgs = tree.tiles.find_pane(&Pane::Messages).unwrap();
        tree.tiles.set_visible(msgs, false);
        crate::layout::toggle_pane(&mut tree, Pane::Members);
        // Unreachable leftover tile.
        let orphan = tree.tiles.insert_pane(Pane::Profile);
        let clean = sanitize(&tree).unwrap();
        assert!(clean.tiles.is_visible(msgs));
        assert!(!crate::layout::pane_visible(&clean, Pane::Members));
        assert!(clean.tiles.get(orphan).is_none());
        assert_eq!(clean.id(), tree_id());
    }
}
