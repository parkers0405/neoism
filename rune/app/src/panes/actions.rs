//! The app side of splits: which tab the window shows follows the focused pane, and the split
//! commands (`Split Right`, `Split Down`, next/previous split, directional focus, nudging).

use super::{Axis, Direction, PaneId};
use crate::app::{App, Focus};
use crate::tabs::TabKind;

/// `Ctrl+Alt+arrows` move a split boundary by 3% (`SPLIT_RESIZE_STEP`).
const NUDGE: f32 = 0.03;

impl App {
    /// Point `active` at the focused pane's tab.
    pub(crate) fn show_focused_pane(&mut self) {
        let Some(tab) = self.layout.focused().active_tab() else { return };
        if let Some(index) = self.tabs.iter().position(|t| t.id == tab) { self.active = index; }
    }

    /// Show a tab (by id) in its pane and focus that pane.
    pub fn select_tab(&mut self, tab: u64) {
        self.layout.select(tab);
        self.show_focused_pane();
    }

    pub fn focus_pane(&mut self, pane: PaneId) {
        if self.layout.focus == pane { return; }
        self.layout.focus = pane;
        self.show_focused_pane();
    }

    /// Split the focused pane with a new terminal beside (`Horizontal`) or below it.
    pub fn split(&mut self, axis: Axis) {
        let id = self.next_tab;
        self.open_tab(TabKind::Terminal);
        // `open_tab` filed the tab in the focused pane; move it into the new split.
        self.layout.remove(id);
        self.layout.split(axis, id);
        self.show_focused_pane();
        self.focus = Focus::Content;
    }

    /// Next / previous split in visual order; returns whether focus moved.
    pub fn focus_split(&mut self, forward: bool) -> bool {
        let moved = self.layout.focus_step(forward);
        self.show_focused_pane();
        moved
    }

    /// The nearest pane in `direction` on screen; returns whether focus moved.
    pub fn focus_split_toward(&mut self, direction: Direction) -> bool {
        let rects = super::view::pane_rects(self);
        let moved = self.layout.focus_direction(direction, &rects);
        self.show_focused_pane();
        moved
    }

    pub fn nudge_split(&mut self, direction: Direction) {
        let (axis, delta) = match direction {
            Direction::Left => (Axis::Horizontal, -NUDGE),
            Direction::Right => (Axis::Horizontal, NUDGE),
            Direction::Up => (Axis::Vertical, -NUDGE),
            Direction::Down => (Axis::Vertical, NUDGE),
        };
        self.layout.nudge(axis, delta);
    }

    /// `Ctrl+Tab`: the focused pane's next (or previous) tab, wrapping.
    pub fn step_tab(&mut self, forward: bool) {
        let pane = self.layout.focused_mut();
        let count = pane.tabs.len();
        if count == 0 { return; }
        pane.active = if forward { (pane.active + 1) % count } else { (pane.active + count - 1) % count };
        self.show_focused_pane();
    }

    /// The tabs of the focused pane as global indices, in strip order.
    pub fn pane_tab_indices(&self) -> Vec<usize> {
        self.layout.focused().tabs.iter()
            .filter_map(|id| self.tabs.iter().position(|t| t.id == *id))
            .collect()
    }

    /// Per frame: views and commands may point `active` at any tab directly; show that tab in
    /// its pane and focus the pane, so the layout always agrees with `active`.
    pub fn sync_panes(&mut self) -> bool {
        let Some(tab) = self.tabs.get(self.active).map(|t| t.id) else { return false };
        let focused = self.layout.focused();
        if focused.active_tab() == Some(tab) { return false; }
        if self.layout.pane_of(tab).is_none() { self.layout.open(tab); }
        self.layout.select(tab);
        true
    }
}
