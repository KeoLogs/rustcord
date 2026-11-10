//! The saved window layout, version 3 (design doc §3.1 / §8 item 7).
//!
//! Version 2 was a free-form `egui_tiles` tree with proportional shares:
//! any pane could dock anywhere (a server rail as a row, the composer under
//! two columns, the user panel under the member list; Leo's leo-5). Version
//! 3 describes **columns** instead: their left-to-right order, where the
//! user panel docks, the rail style, whether rail and sidebar are stacked as
//! tabs, pixel widths for the two resizable side columns, and which side
//! columns the user hid. [`crate::responsive::compute`] turns it into pixel
//! widths for a given window width.
//!
//! `mode` says what hosts the columns: `columns` (one window, side panels;
//! the only mode built now), `framed` or `detached` (later). Pane drawing is
//! container-agnostic (`layout::draw_pane` / `layout::draw_column`), so
//! those modes don't need another rebuild.
//!
//! - [`LayoutV3::from_preset`]: the five presets in this model.
//! - [`LayoutV3::from_v2`]: migrate a saved v2 tree (nearest preset, or
//!   Discord's arrangement with the user's column order kept).
//! - [`load`] / [`save`]: eframe storage (`layout_v3`, falling back to a
//!   migrated `layout_v2`; `migrated` asks the UI for the one-time toast).
//! - [`export`] / [`import`]: shareable JSON; old v1 (tree) exports still
//!   import and are migrated. Everything goes through [`LayoutV3::sanitize`].

// The panel host (rustcord-ui, next on this branch) calls most of this; until
// then only the tests do. Remove this allow once it's wired.
#![allow(dead_code)]

use anyhow::{Context as _, Result, bail};
use eframe::egui::{Rect, pos2};
use egui_tiles::{Container, LinearDir, Tile, TileId, Tree};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::layout::Pane;
use crate::layout_presets::{self, Preset};
use crate::responsive::{ASIDE, SIDEBAR};

/// eframe storage key of the v3 layout.
pub const LAYOUT_V3_KEY: &str = "layout_v3";
/// eframe storage key of the old tree (read once, for the migration).
pub const LAYOUT_V2_KEY: &str = "layout_v2";
/// Shown once after a v2 layout was migrated.
pub const MIGRATION_TOAST: &str =
    "Your layout was updated for resizing. Reset it in Settings \u{2192} Layout if it looks off.";

/// What hosts the columns.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// One window; columns are side panels around the chat (built now).
    #[default]
    Columns,
    /// Columns drawn as framed, inset cards (Phase 2).
    Framed,
    /// Columns may be detached into their own windows (Phase 2).
    Detached,
}

/// One vertical column of the main window.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Column {
    /// Server rail.
    Rail,
    /// Channels / DMs.
    Sidebar,
    /// Header, messages, typing line and composer (one column).
    Chat,
    /// Members / profile / search.
    Aside,
}

impl Column {
    /// Default left-to-right order.
    pub const ALL: [Column; 4] = [Column::Rail, Column::Sidebar, Column::Chat, Column::Aside];

    /// The v2 panes drawn in this column, top to bottom (the user panel is
    /// docked separately).
    pub fn panes(self) -> &'static [Pane] {
        match self {
            Column::Rail => &[Pane::Servers],
            Column::Sidebar => &[Pane::Channels],
            Column::Chat => &[Pane::Messages, Pane::Composer],
            Column::Aside => &[Pane::Members],
        }
    }

    pub fn title(self) -> &'static str {
        match self {
            Column::Rail => "Servers",
            Column::Sidebar => "Channels",
            Column::Chat => "Chat",
            Column::Aside => "Members",
        }
    }
}

/// Where the 52 px user panel docks. It never floats.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserPanelDock {
    /// Under the rail and the sidebar together (Discord).
    #[default]
    RailAndSidebar,
    /// Under the sidebar only (Discord Messenger).
    Sidebar,
    /// Under the rail only (Leo's sketch).
    Rail,
    /// Under the aside.
    Aside,
}

/// The rail's two states.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RailStyle {
    #[default]
    Icons,
    Names,
}

/// The saved layout. Unknown / missing fields fall back to Discord's.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayoutV3 {
    pub mode: Mode,
    /// Left-to-right order; each column exactly once.
    pub columns: Vec<Column>,
    pub user_panel: UserPanelDock,
    pub rail: RailStyle,
    /// Rail and sidebar share one column as two tabs (Wide chat).
    pub stack_rail_sidebar: bool,
    /// Sidebar width the user dragged to (points; clamped per tier).
    pub sidebar_width: f32,
    /// Aside width the user dragged to (points; clamped per tier).
    pub aside_width: f32,
    /// The user hid the sidebar (« in the channel header).
    pub sidebar_hidden: bool,
    /// The user hid the aside (members button).
    pub aside_hidden: bool,
}

impl Default for LayoutV3 {
    fn default() -> Self {
        Self::from_preset(Preset::Discord)
    }
}

/// Sidebar width of the Compact preset.
const COMPACT_SIDEBAR: f32 = 200.0;
/// Two widths this close count as the same (detect).
const WIDTH_TOLERANCE: f32 = 1.0;

impl LayoutV3 {
    /// A preset, expressed in columns (doc §8 item 7).
    pub fn from_preset(preset: Preset) -> Self {
        let base = Self {
            mode: Mode::Columns,
            columns: Column::ALL.to_vec(),
            user_panel: UserPanelDock::RailAndSidebar,
            rail: RailStyle::Icons,
            stack_rail_sidebar: false,
            sidebar_width: SIDEBAR.default,
            aside_width: ASIDE.default,
            sidebar_hidden: false,
            aside_hidden: false,
        };
        match preset {
            // User panel under rail + sidebar.
            Preset::Discord => base,
            // Full-height rail, user panel under the sidebar.
            Preset::Messenger => Self {
                user_panel: UserPanelDock::Sidebar,
                ..base
            },
            // Names rail, user panel under the rail.
            Preset::Sketch => Self {
                user_panel: UserPanelDock::Rail,
                rail: RailStyle::Names,
                ..base
            },
            // Sidebar at 200, aside hidden.
            Preset::Compact => Self {
                sidebar_width: COMPACT_SIDEBAR,
                aside_hidden: true,
                ..base
            },
            // Rail + sidebar tabbed in one column.
            Preset::WideChat => Self {
                stack_rail_sidebar: true,
                user_panel: UserPanelDock::Sidebar,
                ..base
            },
        }
    }

    /// Which preset this still is (mode and hidden panes are ignored, so
    /// the members button doesn't make a layout "custom"). `None` = custom.
    pub fn detect(&self) -> Option<Preset> {
        let me = self.clone().sanitize();
        Preset::ALL.into_iter().find(|&p| {
            let preset = Self::from_preset(p);
            me.columns == preset.columns
                && me.user_panel == preset.user_panel
                && me.rail == preset.rail
                && me.stack_rail_sidebar == preset.stack_rail_sidebar
                && (me.sidebar_width - preset.sidebar_width).abs() <= WIDTH_TOLERANCE
                && (me.aside_width - preset.aside_width).abs() <= WIDTH_TOLERANCE
        })
    }

    /// A clean copy: every column exactly once (missing ones go back to
    /// their default place, duplicates are dropped), the rail next to the
    /// sidebar whenever the user panel spans both or they're stacked, and
    /// widths finite and within §3.2's ranges.
    pub fn sanitize(mut self) -> Self {
        let mut cols: Vec<Column> = Vec::with_capacity(4);
        for c in self.columns.iter().copied() {
            if !cols.contains(&c) {
                cols.push(c);
            }
        }
        for (i, c) in Column::ALL.into_iter().enumerate() {
            if !cols.contains(&c) {
                cols.insert(i.min(cols.len()), c);
            }
        }
        let needs_adjacent =
            self.stack_rail_sidebar || self.user_panel == UserPanelDock::RailAndSidebar;
        if needs_adjacent {
            let pos = |cols: &[Column], c| cols.iter().position(|&x| x == c).unwrap_or(0);
            if pos(&cols, Column::Rail).abs_diff(pos(&cols, Column::Sidebar)) != 1 {
                cols.retain(|&c| c != Column::Rail);
                let at = pos(&cols, Column::Sidebar);
                cols.insert(at, Column::Rail);
            }
        }
        self.columns = cols;
        self.sidebar_width = SIDEBAR.clamp(self.sidebar_width);
        self.aside_width = ASIDE.clamp(self.aside_width);
        self
    }

    /// Migrate a v2 tile tree. A tree that still is a preset becomes that
    /// preset; anything custom becomes Discord's arrangement with the
    /// user's left-to-right column order (and rail style / widths where the
    /// pane was a real column) kept. Hidden Members / Channels stay hidden.
    pub fn from_v2(tree: &Tree<Pane>) -> Self {
        let hidden = |pane: Pane| {
            tree.tiles
                .find_pane(&pane)
                .is_some_and(|id| !tree.tiles.is_visible(id))
        };
        let mut out = match layout_presets::detect(tree) {
            Some(preset) => Self::from_preset(preset),
            None => Self::custom_from_v2(tree),
        };
        out.aside_hidden = hidden(Pane::Members);
        out.sidebar_hidden = hidden(Pane::Channels);
        out.sanitize()
    }

    fn custom_from_v2(tree: &Tree<Pane>) -> Self {
        let mut out = Self::from_preset(Preset::Discord);
        let rects = pane_rects(
            tree,
            Rect::from_min_size(pos2(0.0, 0.0), V2_REFERENCE.into()),
        );
        let rect_of = |pane: Pane| rects.iter().find(|(p, ..)| *p == pane).map(|(_, r, _)| *r);
        let tab_group = |pane: Pane| rects.iter().find(|(p, ..)| *p == pane).and_then(|t| t.2);

        // Column order: left edge, ties in the default order.
        let mut order: Vec<(Column, f32)> = Column::ALL
            .iter()
            .map(|&c| (c, rect_of(c.panes()[0]).map_or(f32::MAX, |r| r.min.x)))
            .collect();
        order.sort_by(|a, b| a.1.total_cmp(&b.1));
        out.columns = order.into_iter().map(|(c, _)| c).collect();

        let is_column = |r: Rect| r.height() > r.width();
        if let Some(r) = rect_of(Pane::Servers)
            && is_column(r)
            && r.width() >= RAIL_NAMES_V2_WIDTH
        {
            out.rail = RailStyle::Names;
        }
        if let Some(r) = rect_of(Pane::Channels)
            && is_column(r)
        {
            out.sidebar_width = r.width();
        }
        if let Some(r) = rect_of(Pane::Members)
            && is_column(r)
        {
            out.aside_width = r.width();
        }
        // Servers and Channels stacked as tabs: Wide chat's stacking.
        if let (Some(a), Some(b)) = (tab_group(Pane::Servers), tab_group(Pane::Channels))
            && a == b
        {
            out.stack_rail_sidebar = true;
        }
        out
    }
}

/// Window size a v2 tree is laid out at to read its columns (the old
/// default window).
const V2_REFERENCE: [f32; 2] = [1200.0, 760.0];
/// v2's rail showed names from this width on (`app::RAIL_NAMES_WIDTH`).
const RAIL_NAMES_V2_WIDTH: f32 = 160.0;
const MAX_DEPTH: usize = 32;

/// Each pane's rect when `tree` fills `rect`, ignoring visibility (hidden
/// panes keep their place) and giving every tab of a tab group the group's
/// rect. The third field is the tab group, when the pane shares one.
fn pane_rects(tree: &Tree<Pane>, rect: Rect) -> Vec<(Pane, Rect, Option<TileId>)> {
    let mut out = Vec::new();
    if let Some(root) = tree.root {
        walk(tree, root, rect, None, 0, &mut out);
    }
    out
}

fn walk(
    tree: &Tree<Pane>,
    id: TileId,
    rect: Rect,
    group: Option<TileId>,
    depth: usize,
    out: &mut Vec<(Pane, Rect, Option<TileId>)>,
) {
    if depth > MAX_DEPTH {
        return;
    }
    match tree.tiles.get(id) {
        Some(Tile::Pane(p)) => out.push((*p, rect, group)),
        Some(Tile::Container(Container::Tabs(tabs))) => {
            let group = (tabs.children.len() > 1).then_some(id).or(group);
            for &c in &tabs.children {
                walk(tree, c, rect, group, depth + 1, out);
            }
        }
        Some(Tile::Container(Container::Linear(lin))) => {
            let total: f32 = lin
                .children
                .iter()
                .map(|&c| lin.shares[c].max(0.0))
                .filter(|s| s.is_finite())
                .sum();
            let total = if total > 0.0 { total } else { 1.0 };
            let mut at = 0.0;
            for &c in &lin.children {
                let share = lin.shares[c];
                let f = if share.is_finite() {
                    share.max(0.0) / total
                } else {
                    0.0
                };
                let r = match lin.dir {
                    LinearDir::Horizontal => {
                        let x = rect.min.x + rect.width() * at;
                        Rect::from_min_max(
                            pos2(x, rect.min.y),
                            pos2(x + rect.width() * f, rect.max.y),
                        )
                    }
                    LinearDir::Vertical => {
                        let y = rect.min.y + rect.height() * at;
                        Rect::from_min_max(
                            pos2(rect.min.x, y),
                            pos2(rect.max.x, y + rect.height() * f),
                        )
                    }
                };
                at += f;
                walk(tree, c, r, None, depth + 1, out);
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Storage

/// The layout read at startup.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub layout: LayoutV3,
    /// A v2 layout was migrated: show [`MIGRATION_TOAST`] once (and save,
    /// so it isn't migrated again).
    pub migrated: bool,
}

/// Read `layout_v3`, else migrate a saved `layout_v2` tree, else Discord's.
pub fn load(storage: Option<&dyn eframe::Storage>) -> Loaded {
    let Some(s) = storage else {
        return Loaded {
            layout: LayoutV3::default(),
            migrated: false,
        };
    };
    if let Some(l) = eframe::get_value::<LayoutV3>(s, LAYOUT_V3_KEY) {
        return Loaded {
            layout: l.sanitize(),
            migrated: false,
        };
    }
    match s.get_string(LAYOUT_V2_KEY) {
        Some(_) => Loaded {
            // A v2 tree that no longer parses / is incomplete still counts
            // as migrated (to the default) so the toast explains the change.
            layout: eframe::get_value::<Tree<Pane>>(s, LAYOUT_V2_KEY)
                .and_then(|t| layout_presets::sanitize(&t).ok())
                .map_or_else(LayoutV3::default, |t| LayoutV3::from_v2(&t)),
            migrated: true,
        },
        None => Loaded {
            layout: LayoutV3::default(),
            migrated: false,
        },
    }
}

/// Save under `layout_v3` (the v2 key is left alone, so going back to an
/// older build still finds its layout).
pub fn save(storage: &mut dyn eframe::Storage, layout: &LayoutV3) {
    eframe::set_value(storage, LAYOUT_V3_KEY, layout);
}

// ---------------------------------------------------------------------------
// Sharing (export / import)

/// Export version of v3 layouts (v1 = the old tile tree, still imported).
pub const EXPORT_VERSION: u32 = 3;

#[derive(Serialize)]
struct ExportDoc<'a> {
    format: &'a str,
    version: u32,
    layout: &'a LayoutV3,
}

/// The layout as pretty JSON, for the clipboard / a file.
pub fn export(layout: &LayoutV3) -> String {
    let doc = ExportDoc {
        format: layout_presets::EXPORT_FORMAT,
        version: EXPORT_VERSION,
        layout,
    };
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// Parse a shared layout: a v3 export, or an old v1 tree export (migrated).
/// The result is sanitized, so it's safe to apply.
pub fn import(text: &str) -> Result<LayoutV3> {
    let text = text.trim();
    if text.is_empty() {
        bail!("Nothing to import: paste a layout first.");
    }
    if text.len() > layout_presets::MAX_IMPORT_BYTES {
        bail!("That's too long to be a RustCord layout.");
    }
    let doc: Value =
        serde_json::from_str(text).context("This isn't a RustCord layout (couldn't read it)")?;
    let format = doc["format"].as_str().unwrap_or_default();
    if format != layout_presets::EXPORT_FORMAT {
        bail!("This isn't a RustCord layout (format \"{format}\").");
    }
    match doc["version"].as_u64() {
        Some(1) => {
            let tree = layout_presets::import(text)?;
            Ok(LayoutV3::from_v2(&tree))
        }
        Some(v) if v == u64::from(EXPORT_VERSION) => {
            let layout: LayoutV3 = serde_json::from_value(doc["layout"].clone())
                .context("This isn't a RustCord layout (couldn't read it)")?;
            Ok(layout.sanitize())
        }
        Some(v) if v > u64::from(EXPORT_VERSION) => {
            bail!("This layout was made by a newer RustCord (version {v}).")
        }
        _ => bail!("This isn't a RustCord layout (unknown version)."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::Storage as _;
    use egui_tiles::Tiles;
    use std::collections::HashMap;

    /// In-memory eframe storage.
    #[derive(Default)]
    struct Mem(HashMap<String, String>);

    impl eframe::Storage for Mem {
        fn get_string(&self, key: &str) -> Option<String> {
            self.0.get(key).cloned()
        }
        fn set_string(&mut self, key: &str, value: String) {
            self.0.insert(key.to_string(), value);
        }
        fn remove_string(&mut self, key: &str) {
            self.0.remove(key);
        }
        fn flush(&mut self) {}
    }

    fn tab(tiles: &mut Tiles<Pane>, pane: Pane) -> TileId {
        let id = tiles.insert_pane(pane);
        tiles.insert_tab_tile(vec![id])
    }

    fn shares(tiles: &mut Tiles<Pane>, c: TileId, s: &[(TileId, f32)]) {
        if let Some(Tile::Container(Container::Linear(l))) = tiles.get_mut(c) {
            for &(id, v) in s {
                l.shares.set_share(id, v);
            }
        }
    }

    /// Leo's leo-5 layout (notes/leo-test-1/leo-5.png): the server rail
    /// docked as a row across the top, a ~590 px DM list, the chat, Members
    /// over the user panel on the right, and the composer as a full-width
    /// row under all of them.
    fn leo5_tree() -> Tree<Pane> {
        let mut t = Tiles::default();
        let servers = tab(&mut t, Pane::Servers);
        let channels = tab(&mut t, Pane::Channels);
        let messages = tab(&mut t, Pane::Messages);
        let members = tab(&mut t, Pane::Members);
        let profile = tab(&mut t, Pane::Profile);
        let composer = tab(&mut t, Pane::Composer);
        let right = t.insert_vertical_tile(vec![members, profile]);
        shares(&mut t, right, &[(members, 7.0), (profile, 1.0)]);
        let middle = t.insert_horizontal_tile(vec![channels, messages, right]);
        shares(
            &mut t,
            middle,
            &[(channels, 6.0), (messages, 3.0), (right, 3.0)],
        );
        let root = t.insert_vertical_tile(vec![servers, middle, composer]);
        shares(
            &mut t,
            root,
            &[(servers, 1.0), (middle, 10.0), (composer, 0.8)],
        );
        Tree::new(layout_presets::tree_id(), root, t)
    }

    #[test]
    fn presets_round_trip_through_detect() {
        for p in Preset::ALL {
            let l = LayoutV3::from_preset(p);
            assert_eq!(l.clone().sanitize(), l, "{p:?} is already clean");
            assert_eq!(l.detect(), Some(p), "{p:?}");
            assert_eq!(l.mode, Mode::Columns);
            assert_eq!(l.columns.len(), 4);
        }
        // Hiding the aside doesn't make Discord "custom".
        let mut l = LayoutV3::from_preset(Preset::Discord);
        l.aside_hidden = true;
        assert_eq!(l.detect(), Some(Preset::Discord));
        l.sidebar_width = 300.0;
        assert_eq!(l.detect(), None);
        assert_eq!(LayoutV3::default().detect(), Some(Preset::Discord));
    }

    #[test]
    fn preset_details_match_the_doc() {
        let m = LayoutV3::from_preset(Preset::Messenger);
        assert_eq!(m.user_panel, UserPanelDock::Sidebar);
        let s = LayoutV3::from_preset(Preset::Sketch);
        assert_eq!(
            (s.rail, s.user_panel),
            (RailStyle::Names, UserPanelDock::Rail)
        );
        let c = LayoutV3::from_preset(Preset::Compact);
        assert_eq!(c.sidebar_width, 200.0);
        assert!(c.aside_hidden);
        let w = LayoutV3::from_preset(Preset::WideChat);
        assert!(w.stack_rail_sidebar);
    }

    #[test]
    fn every_v2_preset_migrates_to_its_v3_preset() {
        for p in Preset::ALL {
            let l = LayoutV3::from_v2(&layout_presets::build(p));
            assert_eq!(l.detect(), Some(p), "{p:?}");
            assert_eq!(l, LayoutV3::from_preset(p), "{p:?}");
        }
        // And the old default tree is Discord.
        let l = LayoutV3::from_v2(&crate::layout::default_tree());
        assert_eq!(l, LayoutV3::from_preset(Preset::Discord));
    }

    #[test]
    fn leo5_migrates_cleanly() {
        let tree = leo5_tree();
        // It is a valid v2 tree (it loaded for Leo) and not a preset.
        let tree = layout_presets::sanitize(&tree).expect("leo-5 is a valid v2 tree");
        assert_eq!(layout_presets::detect(&tree), None);

        let l = LayoutV3::from_v2(&tree);
        assert_eq!(l.mode, Mode::Columns);
        // The rail is a column again, at the far left, icons only.
        assert_eq!(
            l.columns,
            [Column::Rail, Column::Sidebar, Column::Chat, Column::Aside]
        );
        assert_eq!(l.rail, RailStyle::Icons);
        // The user panel docks like Discord's, not under Members.
        assert_eq!(l.user_panel, UserPanelDock::RailAndSidebar);
        assert!(!l.stack_rail_sidebar);
        // The ~590 px DM list is clamped to the sidebar's max.
        assert_eq!(l.sidebar_width, SIDEBAR.max);
        assert!((ASIDE.min..=ASIDE.max).contains(&l.aside_width));
        assert!(!l.aside_hidden && !l.sidebar_hidden);
        assert_eq!(l.clone().sanitize(), l);

        // At Leo's 1205 px window nothing overlaps and the chat is wide.
        let r = crate::responsive::compute(&l, 1205.0, None);
        let sum: f32 = r.row.iter().map(|(_, w)| w).sum();
        assert!((sum - 1205.0).abs() < 0.01);
        assert!(r.chat_width >= 600.0, "chat {}", r.chat_width);
    }

    #[test]
    fn leo5_migrates_from_saved_storage_with_toast() {
        let mut mem = Mem::default();
        let mut tree = leo5_tree();
        // He had hidden the member list.
        let members = tree.tiles.find_pane(&Pane::Members).unwrap();
        tree.tiles.set_visible(members, false);
        eframe::set_value(&mut mem, LAYOUT_V2_KEY, &tree);

        let loaded = load(Some(&mem));
        assert!(loaded.migrated);
        assert!(loaded.layout.aside_hidden);
        assert_eq!(
            loaded.layout.columns,
            [Column::Rail, Column::Sidebar, Column::Chat, Column::Aside]
        );

        // Saved once, it loads as v3 without another toast.
        save(&mut mem, &loaded.layout);
        let again = load(Some(&mem));
        assert!(!again.migrated);
        assert_eq!(again.layout, loaded.layout);
        // The old key is kept for older builds.
        assert!(mem.get_string(LAYOUT_V2_KEY).is_some());
    }

    #[test]
    fn load_fallbacks() {
        // Nothing saved: Discord, no toast.
        assert_eq!(
            load(Some(&Mem::default())),
            Loaded {
                layout: LayoutV3::default(),
                migrated: false
            }
        );
        assert!(!load(None).migrated);
        // A broken v2 entry: default, but the toast still explains.
        let mut mem = Mem::default();
        mem.set_string(LAYOUT_V2_KEY, "not ron".into());
        let l = load(Some(&mem));
        assert!(l.migrated);
        assert_eq!(l.layout, LayoutV3::default());
        // A v3 entry with junk widths is sanitized.
        let mut mem = Mem::default();
        let bad = LayoutV3 {
            sidebar_width: 5000.0,
            columns: vec![Column::Chat, Column::Chat],
            ..LayoutV3::default()
        };
        save(&mut mem, &bad);
        let l = load(Some(&mem)).layout;
        assert_eq!(l.sidebar_width, SIDEBAR.max);
        assert_eq!(l.columns.len(), 4);
    }

    #[test]
    fn custom_v2_keeps_column_order_and_shapes() {
        // The demo's alt layout: wide server list over the channels, members
        // over the profile in the middle, chat on the right.
        let l = LayoutV3::from_v2(&crate::layout::alt_tree());
        assert_eq!(
            l.columns,
            [Column::Rail, Column::Sidebar, Column::Aside, Column::Chat]
        );
        assert_eq!(l.rail, RailStyle::Names);
        assert_eq!(l.detect(), None);

        // Servers and Channels stacked as tabs (not the Wide chat preset's
        // shares): stacked.
        let mut t = Tiles::default();
        let s = t.insert_pane(Pane::Servers);
        let c = t.insert_pane(Pane::Channels);
        let lists = t.insert_tab_tile(vec![c, s]);
        let profile = tab(&mut t, Pane::Profile);
        let messages = tab(&mut t, Pane::Messages);
        let composer = tab(&mut t, Pane::Composer);
        let members = tab(&mut t, Pane::Members);
        let left = t.insert_vertical_tile(vec![lists, profile]);
        let chat = t.insert_vertical_tile(vec![messages, composer]);
        let root = t.insert_horizontal_tile(vec![members, chat, left]);
        let tree = Tree::new(layout_presets::tree_id(), root, t);
        let l = LayoutV3::from_v2(&tree);
        assert!(l.stack_rail_sidebar);
        assert_eq!(l.columns[0], Column::Aside);
        assert_eq!(l.columns[1], Column::Chat);
    }

    #[test]
    fn sanitize_fixes_columns_and_widths() {
        let l = LayoutV3 {
            columns: vec![Column::Aside, Column::Aside, Column::Chat],
            sidebar_width: f32::NAN,
            aside_width: -4.0,
            ..LayoutV3::default()
        }
        .sanitize();
        assert_eq!(l.columns.len(), 4);
        assert_eq!(l.sidebar_width, SIDEBAR.default);
        assert_eq!(l.aside_width, ASIDE.min);
        // Rail and sidebar adjacent because the user panel spans both.
        let rail = l.columns.iter().position(|&c| c == Column::Rail).unwrap();
        let side = l
            .columns
            .iter()
            .position(|&c| c == Column::Sidebar)
            .unwrap();
        assert_eq!(rail + 1, side);

        // Apart is fine when the panel docks under one of them.
        let apart = LayoutV3 {
            columns: vec![Column::Rail, Column::Chat, Column::Sidebar, Column::Aside],
            user_panel: UserPanelDock::Sidebar,
            ..LayoutV3::default()
        };
        assert_eq!(apart.clone().sanitize().columns, apart.columns);
        // ...but not when stacked.
        let stacked = LayoutV3 {
            stack_rail_sidebar: true,
            ..apart
        }
        .sanitize();
        assert_eq!(
            stacked.columns,
            [Column::Chat, Column::Rail, Column::Sidebar, Column::Aside]
        );
    }

    #[test]
    fn export_import_round_trip() {
        for p in Preset::ALL {
            let mut l = LayoutV3::from_preset(p);
            l.mode = Mode::Framed;
            l.aside_width = 300.0;
            let text = export(&l);
            assert!(text.contains("\"version\": 3"));
            assert!(text.contains("\"mode\": \"framed\""));
            assert_eq!(import(&text).unwrap(), l, "{p:?}");
        }
    }

    #[test]
    fn imports_old_v1_tree_exports() {
        let v1 = layout_presets::export(&leo5_tree());
        let l = import(&v1).unwrap();
        assert_eq!(l.columns[0], Column::Rail);
        let v1 = layout_presets::export(&layout_presets::build(Preset::Sketch));
        assert_eq!(import(&v1).unwrap().detect(), Some(Preset::Sketch));
    }

    #[test]
    fn bad_imports_are_rejected() {
        assert!(import("").is_err());
        assert!(import("hello").is_err());
        assert!(import(r#"{"format": "other", "version": 3}"#).is_err());
        let newer = import(r#"{"format": "rustcord-layout", "version": 9, "layout": {}}"#);
        assert!(newer.unwrap_err().to_string().contains("newer"));
        assert!(import(r#"{"format": "rustcord-layout", "version": 2}"#).is_err());
        assert!(
            import(r#"{"format": "rustcord-layout", "version": 3, "layout": {"mode": "x"}}"#)
                .is_err()
        );
        let huge = format!(
            r#"{{"format": "rustcord-layout", "version": 3, "pad": "{}"}}"#,
            "x".repeat(layout_presets::MAX_IMPORT_BYTES)
        );
        assert!(import(&huge).is_err());
        // Missing fields fall back to Discord's; junk is sanitized.
        let l = import(
            r#"{"format": "rustcord-layout", "version": 3,
                "layout": {"columns": ["chat"], "sidebar_width": 99999}}"#,
        )
        .unwrap();
        assert_eq!(l.columns.len(), 4);
        assert_eq!(l.sidebar_width, SIDEBAR.max);
    }
}
