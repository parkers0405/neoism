//! Where Left and Right move a field's caret: by the text's logical order while it runs only left
//! to right, and across the line as it's drawn once it holds right-to-left runs, as a browser's
//! caret moves through mixed Arabic or Hebrew and Latin.

use parley::{Affinity, Cursor, Layout};

use crate::text::Ink;

/// How the arrows move through a field's text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CaretMotion {
    /// By character or word in the text's order (only left-to-right runs, or a hidden secret).
    Logical,
    /// Across the line as drawn: the text holds right-to-left runs.
    Visual,
}

impl CaretMotion {
    pub(super) fn of(layout: &Layout<Ink>, hidden: bool) -> CaretMotion {
        let bidi = layout.is_rtl() || layout.lines().any(|line| line.runs().any(|r| r.is_rtl()));
        if bidi && !hidden { CaretMotion::Visual } else { CaretMotion::Logical }
    }
}

/// Which way, and how far, one arrow press moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Arrow {
    Left,
    Right,
    WordLeft,
    WordRight,
}

/// Seeking the caret's next place across the drawn line. A caret between two runs has two
/// places, one per affinity: the seek tries the downstream one, then the upstream one when
/// that is the line's end, and stops at the first that moves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Seek {
    /// Trying the caret's place on this side.
    Try(Affinity),
    /// It moves to this display byte.
    Found(usize),
    /// Neither place moves: the caret is at the line's edge that way.
    Edge,
}

/// What trying one place gave.
enum Tried {
    Moved(usize),
    Stayed,
}

fn step(state: Seek, event: Tried) -> Seek {
    match (state, event) {
        (Seek::Try(_), Tried::Moved(j)) => Seek::Found(j),
        (Seek::Try(Affinity::Downstream), Tried::Stayed) => Seek::Try(Affinity::Upstream),
        (Seek::Try(Affinity::Upstream), Tried::Stayed) => Seek::Edge,
        (done, _) => done,
    }
}

/// Where an arrow press lands across the drawn line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Landing {
    /// On this display byte.
    At(usize),
    /// The caret is at the line's edge that way: it leaves the line the way its paragraph runs
    /// (left out of a right-to-left line goes on to the next line), or stays at the text's end.
    Edge,
}

/// Where `arrow` takes a caret at display byte `at` across `layout` as drawn.
pub(super) fn visual(layout: &Layout<Ink>, at: usize, arrow: Arrow) -> Landing {
    let mut state = Seek::Try(Affinity::Downstream);
    loop {
        state = match state {
            Seek::Try(affinity) => {
                let c = Cursor::from_byte_index(layout, at, affinity);
                let j = match arrow {
                    Arrow::Left => c.previous_visual(layout),
                    Arrow::Right => c.next_visual(layout),
                    Arrow::WordLeft => c.previous_visual_word(layout),
                    Arrow::WordRight => c.next_visual_word(layout),
                }
                .index();
                step(state, if j == at { Tried::Stayed } else { Tried::Moved(j) })
            }
            Seek::Found(j) => return Landing::At(j),
            Seek::Edge => return Landing::Edge,
        };
    }
}
