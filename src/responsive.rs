//! Responsive layout maths (design doc §3.2 / §3.3), pure and tested.
//!
//! The main window is a row of columns: rail, sidebar, chat, aside. Only the
//! chat flexes. Given the window width, [`compute`] says which columns are
//! docked (and at what pixel width), which live in a drawer, and which are
//! hidden. Breakpoints use ±[`HYSTERESIS`] so dragging the window edge
//! across one doesn't flicker: pass the previous [`Tier`] back in.
//!
//! Collapse order is fixed: Aside → sidebar shrinks to its min → rail and
//! sidebar move into one left drawer. A pane the user hid stays hidden at
//! every width.
//!
//! Nothing here touches egui state; the panel host (side panels today, a
//! frame or viewport later) only reads the [`Responsive`] it returns.

// The panel host (rustcord-ui, next on this branch) calls most of this; until
// then only the tests do. Remove this allow once it's wired.
#![allow(dead_code)]

use crate::layout_v3::{Column, LayoutV3, RailStyle, UserPanelDock};

/// Min / default / max of one pane dimension, in logical points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PaneSize {
    pub default: f32,
    pub min: f32,
    pub max: f32,
}

impl PaneSize {
    const fn new(default: f32, min: f32, max: f32) -> Self {
        Self { default, min, max }
    }

    /// `v` clamped to min..=max (the default when `v` isn't a number).
    pub fn clamp(&self, v: f32) -> f32 {
        if v.is_finite() {
            v.clamp(self.min, self.max)
        } else {
            self.default
        }
    }
}

// §3.2 pane sizes.
/// Rail, icons only.
pub const RAIL: PaneSize = PaneSize::new(72.0, 72.0, 72.0);
/// Rail with server names (the other of its two states).
pub const RAIL_NAMES: PaneSize = PaneSize::new(220.0, 220.0, 220.0);
/// Channels / DMs column.
pub const SIDEBAR: PaneSize = PaneSize::new(240.0, 200.0, 360.0);
/// Members / profile / search column.
pub const ASIDE: PaneSize = PaneSize::new(240.0, 200.0, 320.0);
/// User panel height.
pub const USER_PANEL_HEIGHT: f32 = 52.0;
/// Chat column header height.
pub const CHAT_HEADER_HEIGHT: f32 = 48.0;
/// Typing line height (always reserved).
pub const TYPING_LINE_HEIGHT: f32 = 20.0;
/// Composer: 44 high with one line, grows up to 40% of the chat column.
pub const COMPOSER_MIN_HEIGHT: f32 = 44.0;
pub const COMPOSER_MAX_FRACTION: f32 = 0.4;
/// Status bar height (module).
pub const STATUS_BAR_HEIGHT: f32 = 24.0;
/// The chat column never gets narrower than this while side columns are
/// docked.
pub const CHAT_MIN_WIDTH: f32 = 380.0;
/// Smallest window (`ViewportBuilder::with_min_inner_size`).
pub const WINDOW_MIN: [f32; 2] = [480.0, 400.0];
/// Overlay drawer for the aside.
pub const ASIDE_DRAWER_WIDTH: f32 = 280.0;
/// Width around each breakpoint where the tier doesn't change.
pub const HYSTERESIS: f32 = 16.0;
/// Rail drags at or past this width snap to names, below it to icons.
pub const RAIL_SNAP_WIDTH: f32 = (72.0 + 220.0) / 2.0;

/// Composer height for `content` (text + padding) in a chat column
/// `column_height` high: at least 44, at most 40% of the column.
pub fn composer_height(content: f32, column_height: f32) -> f32 {
    let max = (column_height * COMPOSER_MAX_FRACTION).max(COMPOSER_MIN_HEIGHT);
    if content.is_finite() {
        content.clamp(COMPOSER_MIN_HEIGHT, max)
    } else {
        COMPOSER_MIN_HEIGHT
    }
}

/// A dragged rail width snaps to one of the two rail states.
pub fn snap_rail(dragged_width: f32) -> RailStyle {
    if dragged_width >= RAIL_SNAP_WIDTH {
        RailStyle::Names
    } else {
        RailStyle::Icons
    }
}

/// Width bands of §3.3, narrowest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tier {
    /// < 700: rail + sidebar in a left drawer, aside in a drawer.
    Narrow,
    /// 700–899: sidebar at its min, aside in a drawer.
    Small,
    /// 900–1199: sidebar 220, aside in a drawer.
    Medium,
    /// 1200–1599: sidebar 240, aside docked at 240.
    Large,
    /// ≥ 1600: user widths (sidebar up to 360, aside up to 320).
    Wide,
}

impl Tier {
    const ALL: [Tier; 5] = [
        Tier::Narrow,
        Tier::Small,
        Tier::Medium,
        Tier::Large,
        Tier::Wide,
    ];

    /// Lowest window width of this tier (without hysteresis).
    pub fn lower_bound(self) -> f32 {
        match self {
            Tier::Narrow => 0.0,
            Tier::Small => 700.0,
            Tier::Medium => 900.0,
            Tier::Large => 1200.0,
            Tier::Wide => 1600.0,
        }
    }

    fn up(self) -> Option<Tier> {
        Self::ALL.get(self as usize + 1).copied()
    }

    fn down(self) -> Option<Tier> {
        (self as usize).checked_sub(1).map(|i| Self::ALL[i])
    }

    /// The tier for `width` with no history.
    pub fn for_width(width: f32) -> Tier {
        Self::ALL
            .into_iter()
            .rev()
            .find(|t| width >= t.lower_bound())
            .unwrap_or(Tier::Narrow)
    }

    /// Is the aside docked (rather than a drawer)?
    pub fn aside_docked(self) -> bool {
        self >= Tier::Large
    }

    /// Are the rail and sidebar docked (rather than in the left drawer)?
    pub fn lists_docked(self) -> bool {
        self >= Tier::Small
    }

    /// Largest sidebar width in this tier.
    pub fn sidebar_max(self) -> f32 {
        match self {
            Tier::Wide => SIDEBAR.max,
            Tier::Large | Tier::Narrow => 240.0,
            Tier::Medium => 220.0,
            Tier::Small => SIDEBAR.min,
        }
    }

    /// Largest docked aside width in this tier.
    pub fn aside_max(self) -> f32 {
        match self {
            Tier::Wide => ASIDE.max,
            _ => ASIDE.default,
        }
    }

    /// May the rail show names in this tier? Only while it's docked and the
    /// chat keeps its min width with everything else at its widest
    /// (≥ 1200 px; doc decision: the table says 72, but Leo's sketch preset
    /// keeps names at his 1205 px window where it still fits).
    pub fn rail_names_allowed(self) -> bool {
        self >= Tier::Large
    }
}

/// The tier for `width`, staying in `prev` until the width is more than
/// [`HYSTERESIS`] past the breakpoint (no flicker while dragging).
pub fn tier_with_hysteresis(width: f32, prev: Option<Tier>) -> Tier {
    let Some(mut tier) = prev else {
        return Tier::for_width(width);
    };
    if !width.is_finite() {
        return tier;
    }
    while let Some(up) = tier.up() {
        if width >= up.lower_bound() + HYSTERESIS {
            tier = up;
        } else {
            break;
        }
    }
    while let Some(down) = tier.down() {
        if width < tier.lower_bound() - HYSTERESIS {
            tier = down;
        } else {
            break;
        }
    }
    tier
}

/// Where one column goes this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Slot {
    /// Docked in the row at this pixel width.
    Docked(f32),
    /// In an overlay drawer this wide (opened from the chat header).
    Drawer(f32),
    /// Not shown (the user hid it, or it's a tab of another column).
    Hidden,
}

impl Slot {
    pub fn is_docked(self) -> bool {
        matches!(self, Slot::Docked(_))
    }

    pub fn width(self) -> f32 {
        match self {
            Slot::Docked(w) | Slot::Drawer(w) => w,
            Slot::Hidden => 0.0,
        }
    }
}

/// Where the user panel goes this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserPanelSpot {
    /// Docked under these columns (52 high).
    Docked(UserPanelDock),
    /// At the bottom of the left drawer (and the avatar shows in the chat
    /// header).
    LeftDrawer,
}

/// The answer to "what does the window look like at this width".
#[derive(Debug, Clone, PartialEq)]
pub struct Responsive {
    pub tier: Tier,
    pub window_width: f32,
    /// Rail (Hidden when it's a tab of the sidebar column, or user-hidden).
    pub rail: Slot,
    pub rail_style: RailStyle,
    /// Sidebar (channels / DMs). When `stacked`, it also holds the rail as
    /// a tab.
    pub sidebar: Slot,
    /// Aside (members / profile / search).
    pub aside: Slot,
    /// Remaining width for the chat column.
    pub chat_width: f32,
    pub stacked: bool,
    pub user_panel: UserPanelSpot,
    /// Docked columns left to right with their widths (chat included):
    /// what the panel host lays out.
    pub row: Vec<(Column, f32)>,
    /// Width of the left drawer (rail + sidebar), when they're drawers.
    pub left_drawer: Option<f32>,
    /// Width of the aside drawer, when the aside is a drawer.
    pub aside_drawer: Option<f32>,
}

impl Responsive {
    pub fn slot(&self, column: Column) -> Slot {
        match column {
            Column::Rail => self.rail,
            Column::Sidebar => self.sidebar,
            Column::Aside => self.aside,
            Column::Chat => Slot::Docked(self.chat_width),
        }
    }
}

/// Lay out `layout` in a window `width` points wide. `prev` is last frame's
/// tier (for hysteresis); store `result.tier` and pass it next frame.
pub fn compute(layout: &LayoutV3, width: f32, prev: Option<Tier>) -> Responsive {
    let width = if width.is_finite() {
        width.max(0.0)
    } else {
        WINDOW_MIN[0]
    };
    let tier = tier_with_hysteresis(width, prev);
    let stacked = layout.stack_rail_sidebar;

    let rail_style = if tier.rail_names_allowed() {
        layout.rail
    } else {
        RailStyle::Icons
    };
    let rail_w = match rail_style {
        RailStyle::Icons => RAIL.default,
        RailStyle::Names => RAIL_NAMES.default,
    };
    // Sidebar: the user's width, clamped to the tier's band. When stacked,
    // the rail is a tab of this column, so it must fit the rail too.
    let mut sidebar_w = layout
        .sidebar_width
        .clamp(SIDEBAR.min, tier.sidebar_max().max(SIDEBAR.min));
    if stacked {
        sidebar_w = sidebar_w.max(rail_w);
    }
    let aside_w = layout
        .aside_width
        .clamp(ASIDE.min, tier.aside_max().max(ASIDE.min));

    let (rail, sidebar, left_drawer) = if tier.lists_docked() {
        let rail = if stacked {
            Slot::Hidden
        } else {
            Slot::Docked(rail_w)
        };
        let sidebar = if layout.sidebar_hidden {
            Slot::Hidden
        } else {
            Slot::Docked(sidebar_w)
        };
        (rail, sidebar, None)
    } else {
        // One left drawer: rail + sidebar (or the stacked column).
        let rail_d = if stacked { 0.0 } else { RAIL.default };
        let side_d = if layout.sidebar_hidden && !stacked {
            0.0
        } else {
            SIDEBAR.default
        };
        let total = (rail_d + side_d).min(width);
        let rail = if stacked {
            Slot::Hidden
        } else {
            Slot::Drawer(rail_d)
        };
        let sidebar = if side_d > 0.0 {
            Slot::Drawer((total - rail_d).max(0.0))
        } else {
            Slot::Hidden
        };
        (rail, sidebar, Some(total))
    };

    let (aside, aside_drawer) = if layout.aside_hidden {
        (Slot::Hidden, None)
    } else if tier.aside_docked() {
        (Slot::Docked(aside_w), None)
    } else {
        let w = ASIDE_DRAWER_WIDTH.min(width);
        (Slot::Drawer(w), Some(w))
    };

    let docked_sum: f32 = [rail, sidebar, aside]
        .iter()
        .filter(|s| s.is_docked())
        .map(|s| s.width())
        .sum();
    let chat_width = (width - docked_sum).max(0.0);

    let row = layout
        .columns
        .iter()
        .filter_map(|&c| {
            let slot = match c {
                Column::Chat => Slot::Docked(chat_width),
                Column::Rail => rail,
                Column::Sidebar => sidebar,
                Column::Aside => aside,
            };
            match slot {
                Slot::Docked(w) => Some((c, w)),
                _ => None,
            }
        })
        .collect();

    let user_panel = if tier.lists_docked() {
        UserPanelSpot::Docked(effective_dock(layout, rail, sidebar, aside))
    } else {
        UserPanelSpot::LeftDrawer
    };

    Responsive {
        tier,
        window_width: width,
        rail,
        rail_style,
        sidebar,
        aside,
        chat_width,
        stacked,
        user_panel,
        row,
        left_drawer,
        aside_drawer,
    }
}

/// The user panel's dock, moved to a column that is actually docked (it
/// never floats): the chosen one if possible, else under the sidebar, the
/// rail, then the aside.
fn effective_dock(layout: &LayoutV3, rail: Slot, sidebar: Slot, aside: Slot) -> UserPanelDock {
    let ok = |d: UserPanelDock| match d {
        UserPanelDock::RailAndSidebar => rail.is_docked() && sidebar.is_docked(),
        UserPanelDock::Sidebar => sidebar.is_docked(),
        UserPanelDock::Rail => rail.is_docked(),
        UserPanelDock::Aside => aside.is_docked(),
    };
    [
        layout.user_panel,
        UserPanelDock::Sidebar,
        UserPanelDock::Rail,
        UserPanelDock::Aside,
    ]
    .into_iter()
    .find(|&d| ok(d))
    // Nothing docked at all (both lists hidden, aside in a drawer): under
    // the sidebar's place; the host draws it at the left of the chat row.
    .unwrap_or(UserPanelDock::Sidebar)
}

/// An opened thread's side panel next to the chat.
pub const THREAD_PANEL: PaneSize = PaneSize::new(400.0, 320.0, 460.0);
/// Below this window width an opened thread can't sit next to the chat
/// (rail 72 + sidebar 200 + chat 380 + thread panel 320, plus a little
/// room): it shows as the big panel over the window instead.
pub const THREAD_DOCK_MIN_WIDTH: f32 = 1000.0;

/// Does an opened thread fit as a side panel in a window this wide?
/// `was_docked`: last frame's answer ([`HYSTERESIS`] keeps it from
/// flickering while the window is dragged across the edge).
pub fn thread_docks(window_width: f32, was_docked: bool) -> bool {
    let edge = if was_docked {
        THREAD_DOCK_MIN_WIDTH - HYSTERESIS
    } else {
        THREAD_DOCK_MIN_WIDTH + HYSTERESIS
    };
    window_width >= edge
}

/// The thread side panel's width: 30% of the window, within
/// [`THREAD_PANEL`]'s min / max.
pub fn thread_panel_width(window_width: f32) -> f32 {
    THREAD_PANEL.clamp(window_width * 0.3)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::layout_presets::Preset;

    #[test]
    fn thread_panel_docks_only_when_it_fits() {
        assert!(!thread_docks(900.0, false));
        assert!(!thread_docks(1010.0, false));
        assert!(thread_docks(1020.0, false));
        // Hysteresis: stays docked a little below the edge.
        assert!(thread_docks(990.0, true));
        assert!(!thread_docks(980.0, true));
        assert_eq!(thread_panel_width(1000.0), 320.0);
        assert!((thread_panel_width(1400.0) - 420.0).abs() < 0.01);
        assert_eq!(thread_panel_width(2560.0), 460.0);
        // The chat keeps its minimum next to rail + sidebar + panel.
        let edge = THREAD_DOCK_MIN_WIDTH - HYSTERESIS;
        assert!(edge - thread_panel_width(edge) >= RAIL.min + SIDEBAR.min + CHAT_MIN_WIDTH);
    }

    fn discord() -> LayoutV3 {
        LayoutV3::from_preset(Preset::Discord)
    }

    #[test]
    fn tiers_without_history() {
        assert_eq!(Tier::for_width(480.0), Tier::Narrow);
        assert_eq!(Tier::for_width(699.0), Tier::Narrow);
        assert_eq!(Tier::for_width(700.0), Tier::Small);
        assert_eq!(Tier::for_width(899.9), Tier::Small);
        assert_eq!(Tier::for_width(900.0), Tier::Medium);
        assert_eq!(Tier::for_width(1205.0), Tier::Large);
        assert_eq!(Tier::for_width(1599.0), Tier::Large);
        assert_eq!(Tier::for_width(1600.0), Tier::Wide);
        assert_eq!(Tier::for_width(4000.0), Tier::Wide);
    }

    #[test]
    fn hysteresis_holds_near_breakpoints() {
        // Coming down from Large: stays Large until below 1200 - 16.
        assert_eq!(tier_with_hysteresis(1190.0, Some(Tier::Large)), Tier::Large);
        assert_eq!(tier_with_hysteresis(1185.0, Some(Tier::Large)), Tier::Large);
        assert_eq!(
            tier_with_hysteresis(1183.0, Some(Tier::Large)),
            Tier::Medium
        );
        // Going up from Medium: needs 1200 + 16.
        assert_eq!(
            tier_with_hysteresis(1210.0, Some(Tier::Medium)),
            Tier::Medium
        );
        assert_eq!(
            tier_with_hysteresis(1216.0, Some(Tier::Medium)),
            Tier::Large
        );
        // Big jumps cross several tiers in one go.
        assert_eq!(tier_with_hysteresis(500.0, Some(Tier::Wide)), Tier::Narrow);
        assert_eq!(tier_with_hysteresis(2000.0, Some(Tier::Narrow)), Tier::Wide);
        // Dragging back and forth across 700 doesn't flicker.
        let mut t = Some(Tier::Small);
        for w in [705.0, 695.0, 690.0, 700.0, 710.0, 686.0] {
            t = Some(tier_with_hysteresis(w, t));
            assert_eq!(t, Some(Tier::Small), "at {w}");
        }
        assert_eq!(tier_with_hysteresis(683.0, t), Tier::Narrow);
        assert_eq!(
            tier_with_hysteresis(f32::NAN, Some(Tier::Large)),
            Tier::Large
        );
    }

    #[test]
    fn leo_1205_window() {
        let r = compute(&discord(), 1205.0, None);
        assert_eq!(r.tier, Tier::Large);
        assert_eq!(r.rail, Slot::Docked(72.0));
        assert_eq!(r.sidebar, Slot::Docked(240.0));
        assert_eq!(r.aside, Slot::Docked(240.0));
        assert!((r.chat_width - 653.0).abs() < 0.01);
        let cols: Vec<Column> = r.row.iter().map(|(c, _)| *c).collect();
        assert_eq!(
            cols,
            [Column::Rail, Column::Sidebar, Column::Chat, Column::Aside]
        );
        assert_eq!(
            r.user_panel,
            UserPanelSpot::Docked(UserPanelDock::RailAndSidebar)
        );
        assert_eq!(r.left_drawer, None);
        assert_eq!(r.aside_drawer, None);
    }

    #[test]
    fn collapse_order() {
        let l = discord();
        // Wide: user widths, clamped.
        let mut wide = l.clone();
        wide.sidebar_width = 500.0;
        wide.aside_width = 300.0;
        let r = compute(&wide, 1600.0, None);
        assert_eq!(r.sidebar, Slot::Docked(360.0));
        assert_eq!(r.aside, Slot::Docked(300.0));
        // Large clamps the same layout to the table.
        let r = compute(&wide, 1300.0, None);
        assert_eq!(r.sidebar, Slot::Docked(240.0));
        assert_eq!(r.aside, Slot::Docked(240.0));
        // Medium: the aside goes first (drawer 280), sidebar 220.
        let r = compute(&l, 1000.0, None);
        assert_eq!(r.tier, Tier::Medium);
        assert_eq!(r.aside, Slot::Drawer(280.0));
        assert_eq!(r.aside_drawer, Some(280.0));
        assert_eq!(r.sidebar, Slot::Docked(220.0));
        // Small: sidebar at its min.
        let r = compute(&l, 760.0, None);
        assert_eq!(r.sidebar, Slot::Docked(200.0));
        assert!(r.chat_width >= CHAT_MIN_WIDTH);
        // Narrow: rail + sidebar in one left drawer, chat full width.
        let r = compute(&l, 520.0, None);
        assert_eq!(r.tier, Tier::Narrow);
        assert_eq!(r.left_drawer, Some(312.0));
        assert_eq!(r.rail, Slot::Drawer(72.0));
        assert_eq!(r.sidebar, Slot::Drawer(240.0));
        assert_eq!(r.chat_width, 520.0);
        assert_eq!(r.row, vec![(Column::Chat, 520.0)]);
        assert_eq!(r.user_panel, UserPanelSpot::LeftDrawer);
    }

    #[test]
    fn chat_keeps_its_min_width_at_every_width() {
        for preset in Preset::ALL {
            let mut l = LayoutV3::from_preset(preset);
            l.sidebar_width = SIDEBAR.max;
            l.aside_width = ASIDE.max;
            l.rail = RailStyle::Names;
            let mut prev = None;
            // Sweep down and up again, as a live drag would.
            let widths: Vec<f32> = (480..=2000)
                .rev()
                .step_by(7)
                .chain((480..=2000).step_by(7))
                .map(|w| w as f32)
                .collect();
            for w in widths {
                let r = compute(&l, w, prev);
                prev = Some(r.tier);
                let sum: f32 = r.row.iter().map(|(_, w)| w).sum();
                assert!(
                    (sum - w).abs() < 0.01,
                    "{preset:?} at {w}: row sums to {sum}"
                );
                assert!(
                    r.chat_width >= CHAT_MIN_WIDTH,
                    "{preset:?} at {w}: chat {}",
                    r.chat_width
                );
                assert!(r.left_drawer.unwrap_or(0.0) <= w);
            }
        }
    }

    #[test]
    fn hidden_panes_stay_hidden() {
        let mut l = discord();
        l.aside_hidden = true;
        l.sidebar_hidden = true;
        for w in [520.0, 800.0, 1000.0, 1300.0, 1800.0] {
            let r = compute(&l, w, None);
            assert_eq!(r.aside, Slot::Hidden, "{w}");
            assert_eq!(r.aside_drawer, None);
            assert!(!r.sidebar.is_docked());
        }
        // Only the rail is docked: the user panel moves under it.
        let r = compute(&l, 1300.0, None);
        assert_eq!(r.user_panel, UserPanelSpot::Docked(UserPanelDock::Rail));
    }

    #[test]
    fn stacked_and_names_rail() {
        let wide = LayoutV3::from_preset(Preset::WideChat);
        let r = compute(&wide, 1300.0, None);
        assert!(r.stacked);
        assert_eq!(r.rail, Slot::Hidden);
        assert!(r.sidebar.is_docked());
        assert!(!r.row.iter().any(|(c, _)| *c == Column::Rail));
        let r = compute(&wide, 600.0, None);
        assert_eq!(r.left_drawer, Some(240.0));

        let sketch = LayoutV3::from_preset(Preset::Sketch);
        let r = compute(&sketch, 1205.0, None);
        assert_eq!(r.rail_style, RailStyle::Names);
        assert_eq!(r.rail, Slot::Docked(220.0));
        assert_eq!(r.user_panel, UserPanelSpot::Docked(UserPanelDock::Rail));
        // Too narrow for names: icons.
        let r = compute(&sketch, 1000.0, None);
        assert_eq!(r.rail_style, RailStyle::Icons);
        assert_eq!(r.rail, Slot::Docked(72.0));
    }

    #[test]
    fn user_panel_never_floats() {
        let mut l = LayoutV3::from_preset(Preset::Discord);
        l.user_panel = UserPanelDock::Aside;
        // The aside is a drawer at 1000: fall back to the sidebar.
        let r = compute(&l, 1000.0, None);
        assert_eq!(r.user_panel, UserPanelSpot::Docked(UserPanelDock::Sidebar));
        let r = compute(&l, 1300.0, None);
        assert_eq!(r.user_panel, UserPanelSpot::Docked(UserPanelDock::Aside));
    }

    #[test]
    fn small_helpers() {
        assert_eq!(composer_height(10.0, 800.0), 44.0);
        assert_eq!(composer_height(200.0, 800.0), 200.0);
        assert_eq!(composer_height(900.0, 800.0), 320.0);
        assert_eq!(composer_height(f32::NAN, 800.0), 44.0);
        // A tiny column still gets the 44 px composer.
        assert_eq!(composer_height(100.0, 50.0), 44.0);
        assert_eq!(snap_rail(80.0), RailStyle::Icons);
        assert_eq!(snap_rail(150.0), RailStyle::Names);
        assert_eq!(SIDEBAR.clamp(1000.0), 360.0);
        assert_eq!(SIDEBAR.clamp(f32::INFINITY), 240.0);
        assert_eq!(ASIDE.clamp(100.0), 200.0);
    }
}
