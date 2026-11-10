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
//!
//! Panes can sit in either orientation: the server rail and the user panel
//! lay out as a row when docked as a strip, so their minimums are per axis.
//! Everything here is pure (no egui UI) and tested; `layout.rs` calls it.

use eframe::egui::Rect;
use egui_tiles::{Container, LinearDir, Tile, TileId, Tiles, Tree};

use crate::layout::Pane;
use crate::responsive::{
    ASIDE, COMPOSER_MIN_HEIGHT, RAIL, RAIL_NAMES, SIDEBAR, Tier, USER_PANEL_HEIGHT,
};

const INF: f32 = f32::INFINITY;

/// Smallest chat (messages) width / height before panes fold.
pub const MESSAGES_MIN: [f32; 2] = [240.0, 120.0];
/// Composer height range when it's its own tile: one line of text plus the
/// typing line, up to a few lines.
pub const COMPOSER_RANGE: (f32, f32) = (COMPOSER_MIN_HEIGHT + 28.0, 220.0);
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
        Some(Tile::Pane(p)) => pane_range(*p, axis),
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
fn shrink_rank(tiles: &Tiles<Pane>, id: TileId) -> u8 {
    [
        Pane::Members,
        Pane::Channels,
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
/// one) still takes the rest, so the sizes always fill `extent`.
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
        // Still too big (the window is under every minimum): squash, chat
        // first. Panes clip, so this never overlaps.
        for &i in &order {
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
pub fn fit(tree: &mut Tree<Pane>, rect: Rect, m: Metrics) {
    if let Some(root) = tree.root
        && tree.tiles.is_visible_in_layout(root)
    {
        fit_tile(&mut tree.tiles, root, rect, m, 0);
    }
}

fn fit_tile(tiles: &mut Tiles<Pane>, id: TileId, rect: Rect, m: Metrics, depth: usize) {
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
                fit_tile(tiles, child, r, m, depth + 1);
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
    let flex = kids
        .iter()
        .position(|&c| contains(tiles, c, Pane::Messages, 0));
    let mut shrink: Vec<usize> = (0..kids.len()).collect();
    shrink.sort_by_key(|&i| shrink_rank(tiles, kids[i]));
    let sizes = allocate(&current, &ranges, extent, flex, &shrink);
    if let Some(Tile::Container(Container::Linear(lin))) = tiles.get_mut(id) {
        for (&c, &s) in kids.iter().zip(&sizes) {
            // A zero share would make egui_tiles split evenly; keep a sliver.
            lin.shares.set_share(c, s.max(0.01));
        }
    }
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
        fit_tile(tiles, c, r, m, depth + 1);
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_presets::{self, Preset, PreviewPart, schematic};
    use crate::responsive::tier_with_hysteresis;
    use eframe::egui::{pos2, vec2};

    const M: Metrics = Metrics {
        gap: 0.0,
        tab_bar: 24.0,
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

    /// Fold + fit `tree` at `size` and check: the panes tile the window
    /// with no overlap and no gap, nothing is under its minimum (no
    /// cropping), the chat is there and the user panel is somewhere.
    fn check(tree: &Tree<Pane>, size: [f32; 2], tier: Tier, name: &str) {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(size[0], size[1]));
        let mut t = tree.clone();
        let folds = plan_folds(&t, size[0], tier, M);
        let folded = apply_folds(&mut t, &folds);
        fit(&mut t, rect, M);
        let parts = pane_rects(&t, rect);
        let ctx = format!("{name} at {}x{} ({tier:?}, {folds:?})", size[0], size[1]);
        let area: f32 = schematic(&t, rect).iter().map(|(_, r)| r.area()).sum();
        assert!((area - rect.area()).abs() < 2.0, "{ctx}: gap ({area})");
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
                r.width() >= wx - 0.5,
                "{ctx}: {p:?} cropped to width {}",
                r.width()
            );
            assert!(
                r.height() >= hy - 0.5,
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
                    r.width() <= pane_range(*p, Axis::X).1 + 0.5,
                    "{ctx}: {p:?} too wide"
                );
            }
        }
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

    #[test]
    fn allocate_fills_exactly() {
        let r = [(72.0, 220.0), (200.0, 360.0), (240.0, INF)];
        let s = allocate(&[100.0, 600.0, 100.0], &r, 1000.0, Some(2), &[1, 0, 2]);
        assert_eq!(s, vec![100.0, 360.0, 540.0]);
        // Too small: the chat gives first, then the list, then the rail.
        let s = allocate(&[100.0, 300.0, 600.0], &r, 500.0, Some(2), &[1, 0, 2]);
        assert!((s.iter().sum::<f32>() - 500.0).abs() < 0.01);
        // The list and rail go down to their minimums; only then is the
        // chat squashed (folding normally prevents that).
        assert_eq!((s[0], s[1]), (72.0, 200.0));
        let s = allocate(&[100.0, 300.0, 600.0], &r, 700.0, Some(2), &[1, 0, 2]);
        assert_eq!(s, vec![100.0, 300.0, 300.0]);
        // Under every minimum: still exactly the extent (squashed, no gap).
        let s = allocate(&[100.0, 300.0, 600.0], &r, 300.0, Some(2), &[1, 0, 2]);
        assert!((s.iter().sum::<f32>() - 300.0).abs() < 0.01);
    }
}
