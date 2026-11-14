//! Why a frame committed, as bits for the frame log (`RUNE_FRAME_LOG`'s `why`). A frame that
//! committed with no bits committed outside `frame_step`'s checks (an update, focus, hot reload).

/// What made a frame commit.
pub(crate) struct CommitWhy;

impl CommitWhy {
    /// The app's `poll` (or a playground stage's) returned true.
    pub const POLL: u16 = 1;
    /// A queued event's handler (pointer, hover, key, wheel, selection) returned true without
    /// `Tx::live_only`.
    pub const INPUT: u16 = 2;
    /// An `on_layout`, `on_drawn_layout` or `on_visible_rows` handler returned true.
    pub const LAYOUT: u16 = 4;
    /// A rect the last view read (`Cx::layout_size`, `Cx::layout_with`) moved.
    pub const READ: u16 = 8;
    /// State was marked changed outside a handler: an update, a live value, a caret move.
    pub const DIRTY: u16 = 16;
    /// The viewport, scale or a pixel aspect changed.
    pub const RELAYOUT: u16 = 32;
    /// A `Tx::settle` is pending.
    pub const SETTLE: u16 = 64;
    /// A virtual list's mounted window changed.
    pub const VIRTUAL: u16 = 128;
    /// A key or IME event had to see the state an earlier handler left before routing.
    pub const ROUTE: u16 = 256;
    /// A commit landed at once (`Tx::instant`): what it changed didn't animate, by design.
    pub const INSTANT: u16 = 512;
    pub const NAMES: [&'static str; 10] = ["poll", "input", "layout", "read", "dirty", "relayout", "settle", "virtual", "route", "instant"];

    pub fn names(bits: u16) -> Vec<&'static str> {
        Self::NAMES.iter().enumerate().filter(|(i, _)| bits & (1 << i) != 0).map(|(_, n)| *n).collect()
    }
}
