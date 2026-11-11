//! Make the free tile layout (egui_tiles) fit any window without cropping
//! or overlapping (design doc §2 / §3, applied to the drag-anywhere editor).
//!
//! egui_tiles splits a container by proportional shares, so every pane used
//! to grow and shrink with the window, and could get narrower or shorter
//! than its content (leo-4 / leo-5). Every frame, before the tree is drawn:
//!
//! 1. [`plan_folds`] decides which panes fold into overlay drawers for this
//!    width: Members below the docking breakpoint, then the server rail,
//!    channel list and user panel below 700 px (`responsive::Tier`, with
//!    hysteresis), and earlier if the remaining panes can't fit their
//!    minimum widths. [`apply_folds`] hides them for the frame (and a user
//!    panel left alone in its column goes with them, so there's never an
//!    empty block); [`restore`] puts the saved visibility back.
//! 2. [`fit`] walks the tree top-down with the real rects and rewrites each
//!    linear container's shares as **pixel sizes**: every pane is clamped to
//!    its min / max along that axis ([`pane_range`]), the child holding the
//!    chat absorbs the rest, and the shares always add up to the container,
//!    so there are no gaps. Side panes therefore keep their width when the
//!    window shrinks, and a divider the user drags is clamped next frame.
//!    A container without the chat gives and takes space through the pane
//!    nearest the chat (the channel list in `[rail | channels]`).
//! 3. Divider drags ([`fit_with`] + [`FitMemory`]): egui_tiles moves a
//!    divider by trading space between neighbours in one container, so
//!    widening the server rail used to narrow the channel list next to it.
//!    The pane on the far side of the chat keeps the drag, the panes
//!    between it and the chat keep their size, and the chat absorbs the
//!    change (up through the containers between them).
//!
//! Panes can sit in either orientation: the server rail and the user panel
//! lay out as a row when docked as a strip, so their minimums are per axis.
//! Everything here is pure (no egui UI) and tested; `layout.rs` calls it.

use std::collections::HashMap;

use eframe::egui::Rect;
use egui_tiles::{Container, LinearDir, Tile, TileId, Tiles, Tree};

use crate::layout::Pane;
use crate::responsive::{
    ASIDE, COMPOSER_MIN_HEIGHT, RAIL, RAIL_NAMES, SIDEBAR, Tier, USER_PANEL_HEIGHT, WINDOW_MIN,
};

const INF: f32 = f32::INFINITY;

/// Smallest chat (messages) width / height before panes fold.
pub const MESSAGES_MIN: [f32; 2] = [240.0, 120.0];
/// Composer height range when it's its own tile: one flat row, as tall as
/// the user panel so the bottom row lines up (the reply banner, typing line
/// and upload cards float above it), up to a few lines.
pub const COMPOSER_RANGE: (f32, f32) = (USER_PANEL_HEIGHT, 220.0);
/// The composer's minimum before 0.6.1 (box plus a reserved typing line).
const OLD_COMPOSER_MIN: f32 = COMPOSER_MIN_HEIGHT + 28.0;
/// A list pane (channels, members) is never shorter than this.
const LIST_MIN_HEIGHT: f32 = 120.0;

/// The axis a linear container splits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Axis {
    X,
    Y,
}

impl Axis {
    fn of(dir: LinearDir) -> Self {
        match dir {
            LinearDir::Horizontal => Axis::X,
            LinearDir::Vertical => Axis::Y,
        }
    }

    fn len(self, r: Rect) -> f32 {
        match self {
            Axis::X => r.width(),
            Axis::Y => r.height(),
        }
    }

    fn index(self) -> usize {
        match self {
            Axis::X => 0,
            Axis::Y => 1,
        }
    }
}

/// Which side of the chat a subtree sits on, along one axis.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    /// Left of (or above) the chat.
    Before,
    /// Right of (or below) the chat.
    After,
}

/// Where a linear container sits relative to the chat, per axis: the side,
/// and the same-axis containers (with their child on the way down) from
/// the nearest one that holds the chat. A container without the chat grows
/// through these when one of its panes is dragged wider.
#[derive(Clone, Debug, Default)]
struct Place {
    side: [Option<Side>; 2],
    path: [Vec<(TileId, TileId)>; 2],
}

/// Each linear container's fitted pixel sizes from the last frame, so
/// [`fit_with`] can tell a divider drag (egui_tiles traded space between
/// neighbours) from a window resize. One per window; small.
#[derive(Clone, Debug, Default)]
pub struct FitMemory {
    sizes: HashMap<TileId, Vec<(TileId, f32)>>,
}

/// Min / max size of a pane along `axis` (logical points). Panes that can
/// be strips (the rail, the user panel, the composer) have no max across
/// their strip.
pub fn pane_range(pane: Pane, axis: Axis) -> (f32, f32) {
    match (pane, axis) {
        // A column of icons (72) or icons + names (220); as a top / bottom
        // strip it lays its icons out in a row and needs one icon of height.
        (Pane::Servers, Axis::X) => (RAIL.min, RAIL_NAMES.max),
        (Pane::Servers, Axis::Y) => (RAIL.min, INF),
        (Pane::Channels, Axis::X) => (SIDEBAR.min, SIDEBAR.max),
        (Pane::Members, Axis::X) => (ASIDE.min, ASIDE.max),
        (Pane::Channels | Pane::Members, Axis::Y) => (LIST_MIN_HEIGHT, INF),
        // Avatar + gear at 72 wide, the full row from 150.
        (Pane::Profile, Axis::X) => (RAIL.min, INF),
        (Pane::Profile, Axis::Y) => (USER_PANEL_HEIGHT, USER_PANEL_HEIGHT),
        (Pane::Messages, Axis::X) => (MESSAGES_MIN[0], INF),
        (Pane::Messages, Axis::Y) => (MESSAGES_MIN[1], INF),
        (Pane::Composer, Axis::X) => (MESSAGES_MIN[0], INF),
        (Pane::Composer, Axis::Y) => COMPOSER_RANGE,
    }
}

/// Layout constants the tile host uses (gap between tiles, tab bar).
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub gap: f32,
    pub tab_bar: f32,
    /// Extra width / height each pane's frame takes (the blocky look's
    /// border, label and half gaps), added to every pane's min / max.
    pub chrome: [f32; 2],
}

fn visible_children(tiles: &Tiles<Pane>, children: &[TileId]) -> Vec<TileId> {
    children
        .iter()
        .copied()
        .filter(|&c| tiles.is_visible_in_layout(c))
        .collect()
}

/// The child a Tabs container shows (like egui_tiles: the active one if
/// visible, else the first visible).
fn shown_tab(tiles: &Tiles<Pane>, tabs: &egui_tiles::Tabs) -> Option<TileId> {
    let visible = visible_children(tiles, &tabs.children);
    tabs.active
        .filter(|a| visible.contains(a))
        .or_else(|| visible.first().copied())
}

/// Does the tab strip show? (Our Behavior shows it only for stacked panes.)
fn tab_bar_shown(tabs: &egui_tiles::Tabs) -> bool {
    tabs.children.len() > 1
}

/// Min / max of a subtree along `axis`.
pub fn range(tiles: &Tiles<Pane>, id: TileId, axis: Axis, m: Metrics) -> (f32, f32) {
    range_depth(tiles, id, axis, m, 0)
}

fn range_depth(
    tiles: &Tiles<Pane>,
    id: TileId,
    axis: Axis,
    m: Metrics,
    depth: usize,
) -> (f32, f32) {
    if depth > 32 {
        return (0.0, INF);
    }
    match tiles.get(id) {
        Some(Tile::Pane(p)) => {
            let (lo, hi) = pane_range(*p, axis);
            let extra = match axis {
                Axis::X => m.chrome[0],
                Axis::Y => m.chrome[1],
            };
            (lo + extra, hi + extra)
        }
        Some(Tile::Container(Container::Tabs(tabs))) => {
            let Some(child) = shown_tab(tiles, tabs) else {
                return (0.0, 0.0);
            };
            let (lo, hi) = range_depth(tiles, child, axis, m, depth + 1);
            let bar = if axis == Axis::Y && tab_bar_shown(tabs) {
                m.tab_bar
            } else {
                0.0
            };
            (lo + bar, hi + bar)
        }
        Some(Tile::Container(Container::Linear(lin))) => {
            let kids = visible_children(tiles, &lin.children);
            if kids.is_empty() {
                return (0.0, 0.0);
            }
            let ranges: Vec<(f32, f32)> = kids
                .iter()
                .map(|&c| range_depth(tiles, c, axis, m, depth + 1))
                .collect();
            if Axis::of(lin.dir) == axis {
                let gaps = m.gap * (kids.len() - 1) as f32;
                (
                    ranges.iter().map(|r| r.0).sum::<f32>() + gaps,
                    ranges.iter().map(|r| r.1).sum::<f32>() + gaps,
                )
            } else {
                let lo = ranges.iter().map(|r| r.0).fold(0.0, f32::max);
                let hi = ranges.iter().map(|r| r.1).fold(INF, f32::min);
                (lo, hi.max(lo))
            }
        }
        // Grids never come out of our presets / sanitize.
        Some(Tile::Container(Container::Grid(_))) | None => (0.0, INF),
    }
}

/// Is `pane` anywhere in the visible part of this subtree?
fn contains(tiles: &Tiles<Pane>, id: TileId, pane: Pane, depth: usize) -> bool {
    if depth > 32 || !tiles.is_visible_in_layout(id) {
        return false;
    }
    match tiles.get(id) {
        Some(Tile::Pane(p)) => *p == pane,
        Some(Tile::Container(c)) => c.children().any(|&ch| contains(tiles, ch, pane, depth + 1)),
        None => false,
    }
}

/// Which child gives up space first when the container is too small: the
/// lowest-priority content (members, then lists, then the user panel).
/// The chat's own place in this order only matters when the window is
/// under every minimum: the scrolling lists are squashed before it, and
/// the composer and user panel last.
fn shrink_rank(tiles: &Tiles<Pane>, id: TileId) -> u8 {
    [
        Pane::Members,
        Pane::Channels,
        Pane::Messages,
        Pane::Servers,
        Pane::Profile,
        Pane::Composer,
    ]
    .iter()
    .position(|&p| contains(tiles, id, p, 0))
    .unwrap_or(9) as u8
}

/// New sizes for children currently `sizes` with `ranges`, adding up to
/// `extent`. Each is clamped to its range; the flex child (the chat) takes
/// the difference first, then the others (shrinking lowest-priority first).
/// If every child is at its limit, the flex child (or the first unbounded
/// one) still takes the rest, so the sizes always fill `extent`. Under
/// every minimum the children are squashed in `shrink_order` (which lists
/// the flex child too, after the scrolling lists).
pub fn allocate(
    sizes: &[f32],
    ranges: &[(f32, f32)],
    extent: f32,
    flex: Option<usize>,
    shrink_order: &[usize],
) -> Vec<f32> {
    let n = sizes.len();
    let mut s: Vec<f32> = sizes
        .iter()
        .zip(ranges)
        .map(|(&v, &(lo, hi))| {
            let v = if v.is_finite() { v } else { lo };
            v.clamp(lo, hi.max(lo))
        })
        .collect();
    let mut diff = extent - s.iter().sum::<f32>();
    let mut order: Vec<usize> = flex.into_iter().collect();
    if diff > 0.0 {
        order.extend((0..n).filter(|&i| Some(i) != flex));
        for &i in &order {
            let room = (ranges[i].1 - s[i]).max(0.0);
            let d = diff.min(room);
            s[i] += d;
            diff -= d;
        }
        if diff > 0.0 {
            let i = flex
                .or_else(|| (0..n).find(|&i| ranges[i].1.is_infinite()))
                .unwrap_or(0);
            if let Some(v) = s.get_mut(i) {
                *v += diff;
            }
        }
    } else if diff < 0.0 {
        order.extend(shrink_order.iter().copied().filter(|&i| Some(i) != flex));
        for &i in &order {
            let room = (s[i] - ranges[i].0).max(0.0);
            let d = (-diff).min(room);
            s[i] -= d;
            diff += d;
        }
        // Still too big (the window is under every minimum, e.g. the frame
        // before a new minimum window size lands): squash in priority order,
        // lists first, the composer and user panel last. Panes clip, so
        // this never overlaps.
        let squash = shrink_order.iter().copied().chain(flex);
        for i in squash {
            if diff >= 0.0 {
                break;
            }
            let d = (-diff).min(s[i]);
            s[i] -= d;
            diff += d;
        }
    }
    s
}

/// Rewrite the shares of every linear container in `tree` (laid out in
/// `rect`) as clamped pixel sizes. See the module docs.
#[cfg(test)]
pub fn fit(tree: &mut Tree<Pane>, rect: Rect, m: Metrics) {
    fit_with(tree, rect, m, &mut FitMemory::default());
}

/// [`fit`], plus divider drags since the last frame (`mem`) are redirected
/// so the chat absorbs them (module docs, step 3).
pub fn fit_with(tree: &mut Tree<Pane>, rect: Rect, m: Metrics, mem: &mut FitMemory) {
    let Some(root) = tree.root.filter(|&r| tree.tiles.is_visible_in_layout(r)) else {
        mem.sizes.clear();
        return;
    };
    let mut places = HashMap::new();
    place_walk(&tree.tiles, root, Place::default(), &mut places, 0);
    redirect_drag(&mut tree.tiles, &places, m, mem);
    let mut fitted = HashMap::new();
    fit_tile(&mut tree.tiles, root, rect, m, &places, &mut fitted, 0);
    mem.sizes = fitted;
}

/// Record every linear container's [`Place`] (top-down).
fn place_walk(
    tiles: &Tiles<Pane>,
    id: TileId,
    place: Place,
    out: &mut HashMap<TileId, Place>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    match tiles.get(id) {
        Some(Tile::Container(Container::Tabs(tabs))) => {
            if let Some(child) = shown_tab(tiles, tabs) {
                place_walk(tiles, child, place, out, depth + 1);
            }
        }
        Some(Tile::Container(Container::Linear(lin))) => {
            let a = Axis::of(lin.dir).index();
            let kids = visible_children(tiles, &lin.children);
            let flex = kids
                .iter()
                .position(|&c| contains(tiles, c, Pane::Messages, 0));
            for (k, &child) in kids.iter().enumerate() {
                let mut p = place.clone();
                match flex {
                    Some(f) if k == f => {
                        p.side[a] = None;
                        p.path[a].clear();
                    }
                    Some(f) => {
                        p.side[a] = Some(if k < f { Side::Before } else { Side::After });
                        p.path[a] = vec![(id, child)];
                    }
                    None if p.side[a].is_some() => p.path[a].push((id, child)),
                    None => {}
                }
                place_walk(tiles, child, p, out, depth + 1);
            }
            out.insert(id, place);
        }
        _ => {}
    }
}

/// The child that gives and takes space first: the chat's, else (in a
/// container beside the chat) the one nearest the chat.
fn absorber(tiles: &Tiles<Pane>, kids: &[TileId], side: Option<Side>) -> Option<usize> {
    if let Some(f) = kids
        .iter()
        .position(|&c| contains(tiles, c, Pane::Messages, 0))
    {
        return Some(f);
    }
    match side? {
        Side::Before => kids.len().checked_sub(1),
        Side::After => (!kids.is_empty()).then_some(0),
    }
}

/// A divider drag in one container: `prev` / `now` sizes of its children.
/// Returns the new sizes, the change to pass up to the containers holding
/// it (0 when the chat is in this container) and nothing for no drag.
fn plan_drag(
    prev: &[f32],
    now: &[f32],
    ranges: &[(f32, f32)],
    flex: Option<usize>,
    side: Option<Side>,
) -> Option<(Vec<f32>, f32)> {
    let n = prev.len();
    let delta: Vec<f32> = now.iter().zip(prev).map(|(a, b)| a - b).collect();
    if n < 2 || delta.iter().all(|d| d.abs() < 0.5) || delta.iter().sum::<f32>().abs() > 1.0 {
        return None;
    }
    // How far each divider moved: the edge before the chat of a pane
    // before it, the edge after the chat of a pane after it.
    let before = |j: usize| delta[..=j].iter().sum::<f32>();
    let after = |j: usize| delta[j..].iter().sum::<f32>();
    let (lo, hi): (Vec<usize>, Vec<usize>) = match (flex, side) {
        (Some(f), _) => ((0..f).collect(), (f + 1..n).collect()),
        (None, Some(Side::Before)) => ((0..n - 1).collect(), Vec::new()),
        (None, Some(Side::After)) => (Vec::new(), (1..n).collect()),
        (None, None) => return None,
    };
    let moved = lo
        .iter()
        .map(|&j| (j, before(j)))
        .chain(hi.iter().map(|&j| (j, after(j))))
        .filter(|(_, d)| d.abs() >= 0.5)
        .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))?;
    let (j, d) = moved;
    let (r_lo, r_hi) = ranges[j];
    let size = (prev[j] + d).clamp(r_lo, r_hi.max(r_lo));
    let grow = size - prev[j];
    let mut out = prev.to_vec();
    out[j] = size;
    match flex {
        Some(f) => {
            out[f] = prev[f] - grow;
            Some((out, 0.0))
        }
        None => Some((out, grow)),
    }
}

/// Step 3 of the module docs: turn a divider drag since the last frame
/// into "this pane changes, the chat absorbs it".
fn redirect_drag(
    tiles: &mut Tiles<Pane>,
    places: &HashMap<TileId, Place>,
    m: Metrics,
    mem: &FitMemory,
) {
    let mut plan = None;
    for (&id, prev) in &mem.sizes {
        let Some(Tile::Container(Container::Linear(lin))) = tiles.get(id) else {
            continue;
        };
        let kids = visible_children(tiles, &lin.children);
        if kids.len() != prev.len() || kids.iter().zip(prev).any(|(k, p)| *k != p.0) {
            continue;
        }
        let axis = Axis::of(lin.dir);
        let now: Vec<f32> = kids.iter().map(|&c| lin.shares[c]).collect();
        let before: Vec<f32> = prev.iter().map(|p| p.1).collect();
        let ranges: Vec<(f32, f32)> = kids.iter().map(|&c| range(tiles, c, axis, m)).collect();
        let flex = kids
            .iter()
            .position(|&c| contains(tiles, c, Pane::Messages, 0));
        let place = places.get(&id).cloned().unwrap_or_default();
        let side = place.side[axis.index()];
        if let Some((sizes, grow)) = plan_drag(&before, &now, &ranges, flex, side) {
            let path = place.path[axis.index()].clone();
            // Beside the chat but no container to grow through: leave it.
            if grow != 0.0 && path.is_empty() {
                continue;
            }
            plan = Some((id, kids, sizes, grow, path));
            break;
        }
    }
    let Some((id, kids, sizes, grow, path)) = plan else {
        return;
    };
    if let Some(Tile::Container(Container::Linear(lin))) = tiles.get_mut(id) {
        for (&c, &s) in kids.iter().zip(&sizes) {
            lin.shares.set_share(c, s.max(0.01));
        }
    }
    for (container, child) in path {
        if let Some(Tile::Container(Container::Linear(lin))) = tiles.get_mut(container) {
            let s = lin.shares[child] + grow;
            lin.shares.set_share(child, s.max(0.01));
        }
    }
}

fn fit_tile(
    tiles: &mut Tiles<Pane>,
    id: TileId,
    rect: Rect,
    m: Metrics,
    places: &HashMap<TileId, Place>,
    fitted: &mut HashMap<TileId, Vec<(TileId, f32)>>,
    depth: usize,
) {
    if depth > 32 {
        return;
    }
    let (kids, axis) = match tiles.get(id) {
        Some(Tile::Container(Container::Linear(lin))) => {
            (visible_children(tiles, &lin.children), Axis::of(lin.dir))
        }
        Some(Tile::Container(Container::Tabs(tabs))) => {
            if let Some(child) = shown_tab(tiles, tabs) {
                let mut r = rect;
                if tab_bar_shown(tabs) {
                    r.min.y = (r.min.y + m.tab_bar).min(r.max.y);
                }
                fit_tile(tiles, child, r, m, places, fitted, depth + 1);
            }
            return;
        }
        _ => return,
    };
    if kids.is_empty() {
        return;
    }
    let gaps = m.gap * (kids.len() - 1) as f32;
    let extent = (axis.len(rect) - gaps).max(0.0);
    // Shares are pixel sizes once fitted, so side panes keep their width
    // and only the chat follows the window. A tree that still has small
    // proportional shares (an old save, a hand-written import) is split
    // proportionally once.
    let current = match tiles.get(id) {
        Some(Tile::Container(Container::Linear(lin))) => {
            let raw: Vec<f32> = kids.iter().map(|&c| lin.shares[c]).collect();
            if raw.iter().sum::<f32>() >= extent * 0.5 {
                raw
            } else {
                lin.shares.split(&kids, extent)
            }
        }
        _ => return,
    };
    let ranges: Vec<(f32, f32)> = kids.iter().map(|&c| range(tiles, c, axis, m)).collect();
    let side = places.get(&id).and_then(|p| p.side[axis.index()]);
    let flex = absorber(tiles, &kids, side);
    let mut shrink: Vec<usize> = (0..kids.len()).collect();
    shrink.sort_by_key(|&i| shrink_rank(tiles, kids[i]));
    let sizes = allocate(&current, &ranges, extent, flex, &shrink);
    if let Some(Tile::Container(Container::Linear(lin))) = tiles.get_mut(id) {
        for (&c, &s) in kids.iter().zip(&sizes) {
            // A zero share would make egui_tiles split evenly; keep a sliver.
            lin.shares.set_share(c, s.max(0.01));
        }
    }
    fitted.insert(
        id,
        kids.iter()
            .zip(&sizes)
            .map(|(&c, &s)| (c, s.max(0.01)))
            .collect(),
    );
    let mut pos = match axis {
        Axis::X => rect.left(),
        Axis::Y => rect.top(),
    };
    for (&c, &s) in kids.iter().zip(&sizes) {
        let r = match axis {
            Axis::X => Rect::from_x_y_ranges(pos..=pos + s, rect.y_range()),
            Axis::Y => Rect::from_x_y_ranges(rect.x_range(), pos..=pos + s),
        };
        pos += s + m.gap;
        fit_tile(tiles, c, r, m, places, fitted, depth + 1);
    }
}

/// Smallest tile area `[width, height]` that holds every visible pane at
/// its minimum (with frame chrome, gaps and tab bars). The width assumes
/// every foldable pane is folded (narrower windows fold them into drawers)
/// unless `can_fold` is off (nothing folds); the height is the largest
/// over the fold states, so it holds at every width. The window's minimum
/// inner size is this plus the bars around the tiles.
pub fn min_size(tree: &Tree<Pane>, m: Metrics, can_fold: bool) -> [f32; 2] {
    let lo = |t: &Tree<Pane>, axis| {
        t.root
            .filter(|&r| t.tiles.is_visible_in_layout(r))
            .map_or(0.0, |r| range(&t.tiles, r, axis, m).0)
    };
    if !can_fold {
        return [lo(tree, Axis::X), lo(tree, Axis::Y)];
    }
    let folded = |members, lists| {
        let mut t = tree.clone();
        apply_folds(&mut t, &Folds { members, lists });
        t
    };
    let members = folded(true, false);
    let all = folded(true, true);
    let h = lo(tree, Axis::Y)
        .max(lo(&members, Axis::Y))
        .max(lo(&all, Axis::Y));
    [lo(&all, Axis::X), h]
}

/// The window's minimum inner size: [`min_size`] plus `bars` (the window
/// size minus the tile area: status bar, top bar), never under
/// `WINDOW_MIN`, and never past the monitor (if known), whole points.
pub fn window_min(
    tree: &Tree<Pane>,
    m: Metrics,
    can_fold: bool,
    bars: [f32; 2],
    monitor: Option<eframe::egui::Vec2>,
) -> [f32; 2] {
    let tiles = min_size(tree, m, can_fold);
    let mut out = [0, 1].map(|i| (tiles[i] + bars[i].max(0.0)).max(WINDOW_MIN[i]).ceil());
    if let Some(mon) = monitor.filter(|v| v.x > 0.0 && v.y > 0.0) {
        out[0] = out[0].min(mon.x);
        out[1] = out[1].min(mon.y);
    }
    out
}

// ---------------------------------------------------------------------------
// Folding into drawers

/// Which overlay drawer a folded pane lives in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawer {
    /// From the left (servers, channels, user panel).
    Left,
    /// From the right (members).
    Aside,
}

/// What folds this frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Folds {
    /// Members folds into the aside drawer.
    pub members: bool,
    /// The server rail and channel list (and the user panel) fold into the
    /// left drawer.
    pub lists: bool,
}

/// The panes [`apply_folds`] hid, and where each one is drawn instead.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Folded {
    /// Tiles hidden for this frame (restore them after drawing).
    pub hidden: Vec<TileId>,
    /// Folded panes in drawer order (top to bottom: the user panel last).
    pub panes: Vec<(Pane, Drawer)>,
}

impl Folded {
    pub fn in_drawer(&self, d: Drawer) -> Vec<Pane> {
        self.panes
            .iter()
            .filter(|(_, x)| *x == d)
            .map(|(p, _)| *p)
            .collect()
    }
}

/// Decide what folds at window `width` in `tier` (from
/// `responsive::tier_with_hysteresis`): Members below the aside breakpoint,
/// the lists below 700 px, and earlier when the visible panes' minimum
/// widths don't fit.
pub fn plan_folds(tree: &Tree<Pane>, width: f32, tier: Tier, m: Metrics) -> Folds {
    let mut folds = Folds {
        members: !tier.aside_docked(),
        lists: !tier.lists_docked(),
    };
    let fits = |f: &Folds| {
        let mut t = tree.clone();
        apply_folds(&mut t, f);
        t.root
            .is_none_or(|r| range(&t.tiles, r, Axis::X, m).0 <= width + 0.5)
    };
    if !folds.members && !fits(&folds) {
        folds.members = true;
    }
    if !folds.lists && !fits(&folds) {
        folds.lists = true;
    }
    folds
}

/// Hide the folded panes for this frame. A user panel left alone in its
/// column (nothing else visible next to it) folds too, into the drawer of
/// what it was attached to, so the column never shows an empty block.
pub fn apply_folds(tree: &mut Tree<Pane>, folds: &Folds) -> Folded {
    let mut out = Folded::default();
    let hide = |tree: &mut Tree<Pane>, pane: Pane, drawer: Drawer, out: &mut Folded| {
        if let Some(id) = tree.tiles.find_pane(&pane)
            && tree.tiles.is_visible(id)
        {
            tree.tiles.set_visible(id, false);
            out.hidden.push(id);
            out.panes.push((pane, drawer));
        }
    };
    if folds.lists {
        hide(tree, Pane::Servers, Drawer::Left, &mut out);
        hide(tree, Pane::Channels, Drawer::Left, &mut out);
        hide(tree, Pane::Profile, Drawer::Left, &mut out);
    }
    if folds.members {
        hide(tree, Pane::Members, Drawer::Aside, &mut out);
    }
    if profile_orphaned(tree) {
        let drawer = if folds.members && !folds.lists {
            Drawer::Aside
        } else {
            Drawer::Left
        };
        hide(tree, Pane::Profile, drawer, &mut out);
    }
    out
}

/// Is the user panel visible but alone in its container (its nearest linear
/// ancestor has nothing else visible)?
fn profile_orphaned(tree: &Tree<Pane>) -> bool {
    let tiles = &tree.tiles;
    let Some(pane_id) = tiles.find_pane(&Pane::Profile) else {
        return false;
    };
    if !tiles.is_visible_in_layout(pane_id) {
        return false;
    }
    // Walk up through Tabs wrappers to the first linear container.
    let mut child = pane_id;
    let mut parent = tiles.parent_of(child);
    while let Some(p) = parent {
        match tiles.get(p) {
            Some(Tile::Container(Container::Linear(lin))) => {
                let others = visible_children(tiles, &lin.children)
                    .into_iter()
                    .filter(|&c| c != child)
                    .count();
                return others == 0;
            }
            Some(Tile::Container(Container::Tabs(_))) => {
                child = p;
                parent = tiles.parent_of(p);
            }
            _ => return false,
        }
    }
    // The user panel is the whole window: nothing to attach to.
    false
}

/// Put back the visibility [`apply_folds`] changed.
pub fn restore(tree: &mut Tree<Pane>, folded: &Folded) {
    for &id in &folded.hidden {
        tree.tiles.set_visible(id, true);
    }
}

/// One-time move for layouts saved before 0.6.1: a message box left at the
/// old minimum height (72 plus its frame) drops to the new compact one, so
/// it lines up with the user panel. A box the user made taller keeps its
/// extra height. Returns whether anything changed.
pub fn compact_composer(tree: &mut Tree<Pane>) -> bool {
    let Some(mut child) = tree.tiles.find_pane(&Pane::Composer) else {
        return false;
    };
    while let Some(parent) = tree.tiles.parent_of(child) {
        if let Some(Tile::Container(Container::Linear(lin))) = tree.tiles.get_mut(parent)
            && lin.dir == LinearDir::Vertical
        {
            let share = lin.shares[child];
            // The old minimum plus any frame / tab bar (at most ~24 px).
            if (OLD_COMPOSER_MIN - 0.5..=OLD_COMPOSER_MIN + 24.5).contains(&share) {
                lin.shares
                    .set_share(child, share - (OLD_COMPOSER_MIN - COMPOSER_RANGE.0));
                return true;
            }
            return false;
        }
        child = parent;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_presets::{self, Preset, PreviewPart, schematic};
    use crate::responsive::tier_with_hysteresis;
    use eframe::egui::{pos2, vec2};

    const M: Metrics = Metrics {
        gap: 0.0,
        tab_bar: 24.0,
        chrome: [0.0, 0.0],
    };

    fn leo5_tree() -> Tree<Pane> {
        crate::layout::leo5_tree()
    }

    fn pane_rects(tree: &Tree<Pane>, rect: Rect) -> Vec<(Pane, Rect)> {
        schematic(tree, rect)
            .into_iter()
            .filter_map(|(p, r)| match p {
                PreviewPart::Pane(p) => Some((p, r)),
                PreviewPart::TabBar { .. } => None,
            })
            .collect()
    }

    /// Pane rects (and tab bars) the way egui_tiles lays the tree out:
    /// shares split the extent minus the gaps, tab bars on stacked panes.
    fn laid_out(tree: &Tree<Pane>, rect: Rect, m: Metrics) -> (Vec<(Pane, Rect)>, Vec<Rect>) {
        fn walk(
            tiles: &Tiles<Pane>,
            id: TileId,
            rect: Rect,
            m: Metrics,
            out: &mut (Vec<(Pane, Rect)>, Vec<Rect>),
        ) {
            match tiles.get(id) {
                Some(Tile::Pane(p)) => out.0.push((*p, rect)),
                Some(Tile::Container(Container::Tabs(tabs))) => {
                    let Some(child) = shown_tab(tiles, tabs) else {
                        return;
                    };
                    let mut body = rect;
                    if tab_bar_shown(tabs) {
                        let (bar, rest) = rect.split_top_bottom_at_y(rect.top() + m.tab_bar);
                        out.1.push(bar);
                        body = rest;
                    }
                    walk(tiles, child, body, m, out);
                }
                Some(Tile::Container(Container::Linear(lin))) => {
                    let kids = visible_children(tiles, &lin.children);
                    if kids.is_empty() {
                        return;
                    }
                    let axis = Axis::of(lin.dir);
                    let gaps = m.gap * (kids.len() - 1) as f32;
                    let sizes = lin.shares.split(&kids, (axis.len(rect) - gaps).max(0.0));
                    let mut pos = match axis {
                        Axis::X => rect.left(),
                        Axis::Y => rect.top(),
                    };
                    for (&c, &s) in kids.iter().zip(&sizes) {
                        let r = match axis {
                            Axis::X => Rect::from_x_y_ranges(pos..=pos + s, rect.y_range()),
                            Axis::Y => Rect::from_x_y_ranges(rect.x_range(), pos..=pos + s),
                        };
                        pos += s + m.gap;
                        walk(tiles, c, r, m, out);
                    }
                }
                _ => {}
            }
        }
        let mut out = (Vec::new(), Vec::new());
        if let Some(root) = tree.root
            && tree.tiles.is_visible_in_layout(root)
        {
            walk(&tree.tiles, root, rect, m, &mut out);
        }
        out
    }

    /// Area the gaps between tiles take (zero unless editing).
    fn gap_area(tree: &Tree<Pane>, rect: Rect, m: Metrics) -> f32 {
        if m.gap == 0.0 {
            return 0.0;
        }
        let (parts, bars) = laid_out(tree, rect, m);
        let used = parts.iter().map(|(_, r)| r.area()).sum::<f32>()
            + bars.iter().map(|r| r.area()).sum::<f32>();
        rect.area() - used
    }

    /// The default blocky look: 8 px gaps, 1 px borders, notch labels
    /// (`Theme::pane_chrome` of the built-in theme).
    const FRAMED: Metrics = Metrics {
        gap: 0.0,
        tab_bar: 24.0,
        chrome: [10.0, 19.0],
    };
    /// The layout editor on top of that (6 px drag gaps).
    const EDITING: Metrics = Metrics {
        gap: 6.0,
        tab_bar: 24.0,
        chrome: [10.0, 19.0],
    };
    /// Status bar module (24 px) below the tiles.
    const BARS: f32 = 24.0;

    /// Fold + fit `tree` at `size` and check: the panes tile the window
    /// with no overlap and no gap, nothing is under its minimum (no
    /// cropping), the chat is there and the user panel is somewhere.
    fn check(tree: &Tree<Pane>, size: [f32; 2], tier: Tier, name: &str) {
        check_with(tree, size, tier, M, true, name);
    }

    /// [`check`] with frame metrics; returns the fitted tree (shares kept,
    /// folds restored) so a sweep carries it frame to frame like the app.
    fn check_with(
        tree: &Tree<Pane>,
        size: [f32; 2],
        tier: Tier,
        m: Metrics,
        fold: bool,
        name: &str,
    ) -> Tree<Pane> {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(size[0], size[1]));
        let mut t = tree.clone();
        let folds = if fold {
            plan_folds(&t, size[0], tier, m)
        } else {
            Folds::default()
        };
        let folded = apply_folds(&mut t, &folds);
        fit(&mut t, rect, m);
        let (parts, bars) = laid_out(&t, rect, m);
        let ctx = format!("{name} at {}x{} ({tier:?}, {folds:?})", size[0], size[1]);
        let area: f32 = parts.iter().map(|(_, r)| r.area()).sum::<f32>()
            + bars.iter().map(|r| r.area()).sum::<f32>();
        let gaps: f32 = gap_area(&t, rect, m);
        assert!(
            (area + gaps - rect.area()).abs() < 2.0,
            "{ctx}: gap ({area} + {gaps})"
        );
        for (i, (p, r)) in parts.iter().enumerate() {
            assert!(rect.expand(0.5).contains_rect(*r), "{ctx}: {p:?} outside");
            for (q, s) in &parts[i + 1..] {
                assert!(
                    r.intersect(*s).area() < 0.5,
                    "{ctx}: {p:?} {r:?} overlaps {q:?} {s:?}"
                );
            }
            let (wx, _) = pane_range(*p, Axis::X);
            let (hy, _) = pane_range(*p, Axis::Y);
            assert!(
                r.width() >= wx + m.chrome[0] - 0.5,
                "{ctx}: {p:?} cropped to width {}",
                r.width()
            );
            assert!(
                r.height() >= hy + m.chrome[1] - 0.5,
                "{ctx}: {p:?} cropped to height {}",
                r.height()
            );
        }
        assert!(
            parts.iter().any(|(p, _)| *p == Pane::Messages),
            "{ctx}: no chat"
        );
        let profile_shown = parts.iter().any(|(p, _)| *p == Pane::Profile)
            || folded.panes.iter().any(|(p, _)| *p == Pane::Profile);
        assert!(profile_shown, "{ctx}: user panel lost");
        // Side panes keep pixel widths: never past their max.
        for (p, r) in &parts {
            if matches!(p, Pane::Channels | Pane::Members) {
                assert!(
                    r.width() <= pane_range(*p, Axis::X).1 + m.chrome[0] + 0.5,
                    "{ctx}: {p:?} too wide"
                );
            }
        }
        restore(&mut t, &folded);
        t
    }

    fn sweep(tree: &Tree<Pane>, name: &str) {
        for h in [785.0, 400.0] {
            let mut tier = None;
            let mut w = 1600.0;
            while w >= 480.0 {
                let t = tier_with_hysteresis(w, tier);
                tier = Some(t);
                check(tree, [w, h], t, name);
                w -= 10.0;
            }
            // And back up again.
            while w <= 1600.0 {
                let t = tier_with_hysteresis(w, tier);
                tier = Some(t);
                check(tree, [w, h], t, name);
                w += 10.0;
            }
        }
    }

    /// The window the app allows for a requested size: never under
    /// `WINDOW_MIN` or the tree's own minimum plus the bars.
    fn window_for(tree: &Tree<Pane>, m: Metrics, fold: bool, want: [f32; 2]) -> [f32; 2] {
        let min = window_min(tree, m, fold, [0.0, BARS], None);
        [want[0].max(min[0]), want[1].max(min[1])]
    }

    /// Resize like a user dragging the window edge: heights 1000 -> 400 ->
    /// 1000 at several widths, then widths 1600 -> 480 -> 1600 at several
    /// heights, carrying the fitted tree (its pixel shares) from frame to
    /// frame like the app does.
    fn drag_sweep(tree: &Tree<Pane>, m: Metrics, fold: bool, name: &str) {
        let tiles_area = |win: [f32; 2]| [win[0], win[1] - BARS];
        for w in [1600.0, 1200.0, 900.0, 700.0, 480.0] {
            let mut t = tree.clone();
            let mut tier = None;
            let heights = (0..=60)
                .map(|i| 1000.0 - i as f32 * 10.0)
                .chain((0..=60).map(|i| 400.0 + i as f32 * 10.0));
            for h in heights {
                let size = tiles_area(window_for(&t, m, fold, [w, h]));
                let tr = tier_with_hysteresis(size[0], tier);
                tier = Some(tr);
                t = check_with(&t, size, tr, m, fold, name);
            }
        }
        for h in [1000.0, 760.0, 600.0, 400.0] {
            let mut t = tree.clone();
            let mut tier = None;
            let widths = (0..=112)
                .map(|i| 1600.0 - i as f32 * 10.0)
                .chain((0..=112).map(|i| 480.0 + i as f32 * 10.0));
            for w in widths {
                let size = tiles_area(window_for(&t, m, fold, [w, h]));
                let tr = tier_with_hysteresis(size[0], tier);
                tier = Some(tr);
                t = check_with(&t, size, tr, m, fold, name);
            }
        }
    }

    #[test]
    fn framed_metrics_match_the_default_look() {
        let c = crate::themes::ActiveTheme::default_theme().pane_chrome();
        assert_eq!([c.x, c.y], FRAMED.chrome);
    }

    #[test]
    fn framed_resize_never_crops() {
        drag_sweep(&crate::layout::default_tree(), FRAMED, true, "default");
        drag_sweep(&leo5_tree(), FRAMED, true, "leo-5");
        drag_sweep(&crate::layout::alt_tree(), FRAMED, true, "alt");
        for p in Preset::ALL {
            drag_sweep(&layout_presets::build(p), FRAMED, true, &format!("{p:?}"));
        }
    }

    #[test]
    fn editor_resize_never_crops() {
        // No folding while editing, and 6 px gaps between the tiles. The
        // app only enforces the folded width (entering the editor never
        // widens the window), so this checks widths the editor fits.
        drag_sweep(&crate::layout::default_tree(), EDITING, false, "default");
        drag_sweep(&leo5_tree(), EDITING, false, "leo-5");
    }

    #[test]
    fn window_min_fits_the_framed_minimums() {
        // leo-5 stacks the rail strip, the members + user panel column and
        // the composer: the window minimum follows the framed panes' sum
        // (the cropped-chat report). Since 0.6.1 the compact composer
        // (52, like the user panel) brings it back to about the old fixed
        // 400 px minimum.
        let min = min_size(&leo5_tree(), FRAMED, true);
        let need = (RAIL.min + 19.0)
            + (LIST_MIN_HEIGHT + 19.0)
            + (USER_PANEL_HEIGHT + 19.0)
            + (COMPOSER_RANGE.0 + 19.0);
        assert!((min[1] - need).abs() < 0.5, "{min:?} vs {need}");
        assert!((min[1] + BARS - WINDOW_MIN[1]).abs() < 8.0, "{min:?}");
        // Narrow windows fold the side panes, so the width stays small.
        assert!(min[0] <= WINDOW_MIN[0], "{min:?}");
    }

    #[test]
    fn tall_window_shares_do_not_crop_the_composer_when_short() {
        // The user drags the message box to its tallest in a tall window,
        // then makes the window as short as it goes.
        for (tree, name) in [
            (crate::layout::default_tree(), "default"),
            (leo5_tree(), "leo-5"),
        ] {
            let mut t = check_with(&tree, [1400.0, 1400.0], Tier::Large, FRAMED, true, name);
            let mut child = t.tiles.find_pane(&Pane::Composer).unwrap();
            while let Some(parent) = t.tiles.parent_of(child) {
                if let Some(Tile::Container(Container::Linear(lin))) = t.tiles.get_mut(parent)
                    && lin.dir == LinearDir::Vertical
                {
                    lin.shares.set_share(child, 600.0);
                    break;
                }
                child = parent;
            }
            let win = window_for(&t, FRAMED, true, [1400.0, 300.0]);
            let size = [win[0], win[1] - BARS];
            let t = check_with(&t, size, Tier::Large, FRAMED, true, name);
            let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(size[0], size[1]));
            let comp = pane_rects(&t, rect)
                .into_iter()
                .find(|(p, _)| *p == Pane::Composer)
                .unwrap()
                .1;
            assert!(
                comp.height() >= COMPOSER_RANGE.0 + 19.0 - 0.5,
                "{name}: {comp:?}"
            );
        }
    }

    #[test]
    fn old_composer_height_compacts_once() {
        let composer_share = |t: &Tree<Pane>| {
            let child = t.tiles.find_pane(&Pane::Composer).unwrap();
            let tab = t.tiles.parent_of(child).unwrap();
            let col = t.tiles.parent_of(tab).unwrap();
            match t.tiles.get(col) {
                Some(Tile::Container(Container::Linear(lin))) => lin.shares[tab],
                _ => panic!("composer not in a column"),
            }
        };
        let set = |t: &mut Tree<Pane>, v: f32| {
            let child = t.tiles.find_pane(&Pane::Composer).unwrap();
            let tab = t.tiles.parent_of(child).unwrap();
            let col = t.tiles.parent_of(tab).unwrap();
            if let Some(Tile::Container(Container::Linear(lin))) = t.tiles.get_mut(col) {
                lin.shares.set_share(tab, v);
            }
        };
        // Saved at the old minimum in the framed look (72 + 19).
        let mut t = crate::layout::default_tree();
        set(&mut t, OLD_COMPOSER_MIN + 19.0);
        assert!(compact_composer(&mut t));
        assert!((composer_share(&t) - (COMPOSER_RANGE.0 + 19.0)).abs() < 0.01);
        // Already compact, or made taller by the user: left alone.
        assert!(!compact_composer(&mut t));
        set(&mut t, 160.0);
        assert!(!compact_composer(&mut t));
        assert!((composer_share(&t) - 160.0).abs() < 0.01);
        // The new default is already compact and lines up with the user
        // panel.
        assert_eq!(COMPOSER_RANGE.0, USER_PANEL_HEIGHT);
    }

    #[test]
    fn leo5_resolves_from_1600_to_480() {
        sweep(&leo5_tree(), "leo-5");
    }

    #[test]
    fn presets_and_alt_resolve_from_1600_to_480() {
        for p in Preset::ALL {
            sweep(&layout_presets::build(p), &format!("{p:?}"));
        }
        sweep(&crate::layout::alt_tree(), "alt");
    }

    #[test]
    fn leo5_side_panes_keep_their_width() {
        let mut t = leo5_tree();
        let rect = |w| Rect::from_min_size(pos2(0.0, 0.0), vec2(w, 785.0));
        fit(&mut t, rect(1600.0), M);
        let at = |t: &Tree<Pane>, w, pane| {
            pane_rects(t, rect(w))
                .into_iter()
                .find(|(p, _)| *p == pane)
                .map(|(_, r)| r)
                .unwrap()
        };
        // The 590 px DM list is clamped to the sidebar max.
        let ch = at(&t, 1600.0, Pane::Channels);
        assert!((ch.width() - SIDEBAR.max).abs() < 0.5, "{ch:?}");
        let members = at(&t, 1600.0, Pane::Members).width();
        // Shrinking the window: only the chat gets narrower.
        fit(&mut t, rect(1300.0), M);
        assert!((at(&t, 1300.0, Pane::Channels).width() - ch.width()).abs() < 0.5);
        assert!((at(&t, 1300.0, Pane::Members).width() - members).abs() < 0.5);
        // The squashed 44 px message box gets its real height; the user
        // panel is exactly one row.
        assert!(at(&t, 1300.0, Pane::Composer).height() >= COMPOSER_RANGE.0 - 0.5);
        assert!((at(&t, 1300.0, Pane::Profile).height() - USER_PANEL_HEIGHT).abs() < 0.5);
    }

    #[test]
    fn folds_follow_the_breakpoints() {
        let t = leo5_tree();
        let f = plan_folds(&t, 1300.0, Tier::Large, M);
        assert_eq!(f, Folds::default());
        let f = plan_folds(&t, 1000.0, Tier::Medium, M);
        assert!(f.members && !f.lists);
        let f = plan_folds(&t, 600.0, Tier::Narrow, M);
        assert!(f.members && f.lists);
        // Members folding leaves the user panel alone in its column: it
        // goes into the aside drawer with Members.
        let mut t2 = t.clone();
        let folded = apply_folds(&mut t2, &plan_folds(&t, 1000.0, Tier::Medium, M));
        assert_eq!(
            folded.in_drawer(Drawer::Aside),
            vec![Pane::Members, Pane::Profile]
        );
        restore(&mut t2, &folded);
        assert_eq!(t2.tiles.iter().count(), t.tiles.iter().count());
        assert!(t2.tiles.iter().all(|(id, _)| t2.tiles.is_visible(*id)));
    }

    /// Width of each pane in the fitted `tree`.
    fn widths(tree: &Tree<Pane>, rect: Rect) -> Vec<(Pane, f32)> {
        laid_out(tree, rect, M)
            .0
            .into_iter()
            .map(|(p, r)| (p, r.width()))
            .collect()
    }

    /// `pane`'s width in [`widths`].
    fn w(widths: &[(Pane, f32)], pane: Pane) -> f32 {
        widths.iter().find(|(p, _)| *p == pane).unwrap().1
    }

    /// The linear container holding `pane`'s tab, and that tab.
    fn holder(tree: &Tree<Pane>, pane: Pane) -> (TileId, TileId) {
        let tab = tree
            .tiles
            .iter()
            .find_map(|(&id, t)| match t {
                Tile::Container(Container::Tabs(tabs))
                    if tabs.children.iter().any(
                        |c| matches!(tree.tiles.get(*c), Some(Tile::Pane(p)) if *p == pane),
                    ) =>
                {
                    Some(id)
                }
                _ => None,
            })
            .unwrap();
        (parent_of(tree, tab), tab)
    }

    /// The linear container whose children include `id`.
    fn parent_of(tree: &Tree<Pane>, id: TileId) -> TileId {
        tree.tiles
            .iter()
            .find_map(|(&p, t)| match t {
                Tile::Container(Container::Linear(lin)) if lin.children.contains(&id) => Some(p),
                _ => None,
            })
            .unwrap()
    }

    /// egui_tiles' divider drag: `a` grows by `d`, its neighbour `b`
    /// shrinks by `d` (the shares are pixel sizes once fitted).
    fn drag(tree: &mut Tree<Pane>, a: Pane, b: Pane, d: f32) {
        for (pane, d) in [(a, d), (b, -d)] {
            let (parent, tab) = holder(tree, pane);
            if let Some(Tile::Container(Container::Linear(lin))) = tree.tiles.get_mut(parent) {
                let s = lin.shares[tab] + d;
                lin.shares.set_share(tab, s);
            }
        }
    }

    /// Leo's bug: dragging the server rail's edge 100 px wider narrowed the
    /// channel list. Now the rail grows, the channel list keeps its width
    /// and the chat gives up the 100 px.
    #[test]
    fn rail_drag_takes_width_from_the_chat() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let mut tree = crate::layout::default_tree();
        let mut mem = FitMemory::default();
        fit_with(&mut tree, rect, M, &mut mem);
        let before = widths(&tree, rect);
        drag(&mut tree, Pane::Servers, Pane::Channels, 100.0);
        fit_with(&mut tree, rect, M, &mut mem);
        let after = widths(&tree, rect);
        assert!((w(&after, Pane::Servers) - (w(&before, Pane::Servers) + 100.0)).abs() < 0.5);
        assert!((w(&after, Pane::Channels) - w(&before, Pane::Channels)).abs() < 0.5);
        assert!((w(&after, Pane::Messages) - (w(&before, Pane::Messages) - 100.0)).abs() < 0.5);
        assert!((w(&after, Pane::Members) - w(&before, Pane::Members)).abs() < 0.5);
        // Stable: the next frame (no drag) changes nothing.
        fit_with(&mut tree, rect, M, &mut mem);
        assert_eq!(widths(&tree, rect), after);
        // And back: narrowing the rail gives the width to the chat.
        drag(&mut tree, Pane::Servers, Pane::Channels, -100.0);
        fit_with(&mut tree, rect, M, &mut mem);
        let back = widths(&tree, rect);
        assert!((w(&back, Pane::Servers) - w(&before, Pane::Servers)).abs() < 0.5);
        assert!((w(&back, Pane::Channels) - w(&before, Pane::Channels)).abs() < 0.5);
        assert!((w(&back, Pane::Messages) - w(&before, Pane::Messages)).abs() < 0.5);
    }

    /// The channels / chat divider resizes only the channel list against
    /// the chat; the rail keeps its width.
    #[test]
    fn channels_drag_keeps_the_rail() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(1280.0, 800.0));
        let mut tree = crate::layout::default_tree();
        let mut mem = FitMemory::default();
        fit_with(&mut tree, rect, M, &mut mem);
        let before = widths(&tree, rect);
        // In the default tree that divider is between the left column
        // (rail + channels over the user panel) and the chat column.
        let (lists, _) = holder(&tree, Pane::Servers);
        let left = parent_of(&tree, lists);
        let (chat_col, _) = holder(&tree, Pane::Messages);
        let root = parent_of(&tree, chat_col);
        assert_eq!(parent_of(&tree, left), root);
        if let Some(Tile::Container(Container::Linear(lin))) = tree.tiles.get_mut(root) {
            let (l, c) = (lin.shares[left], lin.shares[chat_col]);
            lin.shares.set_share(left, l + 60.0);
            lin.shares.set_share(chat_col, c - 60.0);
        }
        fit_with(&mut tree, rect, M, &mut mem);
        let after = widths(&tree, rect);
        assert!((w(&after, Pane::Servers) - w(&before, Pane::Servers)).abs() < 0.5);
        assert!((w(&after, Pane::Channels) - (w(&before, Pane::Channels) + 60.0)).abs() < 0.5);
        assert!((w(&after, Pane::Messages) - (w(&before, Pane::Messages) - 60.0)).abs() < 0.5);
    }

    #[test]
    fn allocate_fills_exactly() {
        let r = [(72.0, 220.0), (200.0, 360.0), (240.0, INF)];
        let s = allocate(&[100.0, 600.0, 100.0], &r, 1000.0, Some(2), &[1, 0, 2]);
        assert_eq!(s, vec![100.0, 360.0, 540.0]);
        // Too small: the chat gives first, then the list, then the rail.
        // Under every minimum (folding and the window's minimum size
        // normally prevent that) the scrolling list is squashed before the
        // chat.
        let s = allocate(&[100.0, 300.0, 600.0], &r, 500.0, Some(2), &[1, 0, 2]);
        assert!((s.iter().sum::<f32>() - 500.0).abs() < 0.01);
        assert_eq!(s, vec![72.0, 188.0, 240.0]);
        let s = allocate(&[100.0, 300.0, 600.0], &r, 700.0, Some(2), &[1, 0, 2]);
        assert_eq!(s, vec![100.0, 300.0, 300.0]);
        // Under every minimum: still exactly the extent (squashed, no gap).
        let s = allocate(&[100.0, 300.0, 600.0], &r, 300.0, Some(2), &[1, 0, 2]);
        assert!((s.iter().sum::<f32>() - 300.0).abs() < 0.01);
    }
}
