//! Scrollbar helpers for the completion popup.
//!
//! Historically a verbatim fork of `crate::widgets::scrollbar` made
//! before the composer moved into the shared crate; now that both live
//! here it simply re-exports the canonical helpers so the popup picks
//! up look-pack restyling (width/color/radius/track overrides) like
//! every other scrollbar.

pub use crate::widgets::scrollbar::{
    compute_thumb, draw_thumb, draw_track, opacity_from_last_scroll, width,
    SCROLLBAR_MARGIN,
};
