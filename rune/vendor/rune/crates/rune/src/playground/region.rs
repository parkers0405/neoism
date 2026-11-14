//! Which of the playground's three columns has the keyboard: the story list, the stage or the
//! side column. One at a time, and only it gets keys: the list its arrows, letters and triggers,
//! the side column its controls, variants' digits and `side_keys` hook, the stage the story's
//! own keys. A click in a column, or focus moving into one, gives it the keyboard.

use super::*;

/// A column of the open playground, as the keyboard sees it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Region {
    #[default]
    List,
    Stage,
    Side,
}

impl<S: 'static> Live<S> {
    pub(crate) fn region(&self) -> Region {
        self.region
    }

    /// Gives `region` the keyboard; true when that changed.
    pub(crate) fn set_region(&mut self, region: Region) -> bool {
        std::mem::replace(&mut self.region, region) != region
    }

    /// The column at window point `x`, in a window `width` wide.
    pub(crate) fn region_at(&self, x: f32, width: f32) -> Region {
        let (sidebar_w, side_w) = self.widths(width);
        if x < sidebar_w + 1.0 {
            Region::List
        } else if x >= width - side_w {
            Region::Side
        } else {
            Region::Stage
        }
    }

    /// Focus as the region last saw it, so a focus that only a hover moved (a row under the
    /// pointer) is passed over rather than taken as a move: true when `focused` is new.
    pub(crate) fn see_focus(&mut self, focused: Option<&str>) -> bool {
        if self.region_seen.as_deref() == focused { return false; }
        self.region_seen = focused.map(str::to_owned);
        true
    }
}
