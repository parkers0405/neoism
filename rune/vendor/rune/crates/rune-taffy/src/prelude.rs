//! Drop-in prelude matching `taffy::prelude`, re-exporting [`crate::LayoutEngine`] and compact types.

pub use crate::{BoxLayout, CompactStyle, LayoutEngine, SlotId};
pub use taffy::{
    geometry::{Line, Rect, Size},
    style::{
        AlignContent, AlignContentKeyword, AlignItems, AlignItemsKeyword, AlignSelf, AlignmentSafety, AvailableSpace,
        BoxSizing, CompactLength, Dimension, Display, FlexboxItemStyle, JustifyContent, JustifyItems, JustifySelf,
        LengthPercentage, LengthPercentageAuto, Position, Style,
    },
    style_helpers::{
        FromFr, FromLength, FromPercent, TaffyAuto, TaffyFitContent, TaffyMaxContent, TaffyMinContent, TaffyZero, auto,
        fit_content, length, max_content, min_content, percent, zero,
    },
    tree::{Layout, LayoutPartialTree, NodeId, PrintTree, RoundTree, TraversePartialTree, TraverseTree},
};

pub use taffy::style::{FlexDirection, FlexWrap};
pub use taffy::style::{
    GridAutoFlow, GridPlacement, GridTemplateComponent, MaxTrackSizingFunction, MinTrackSizingFunction,
    RepetitionCount, TrackSizingFunction,
};
pub use taffy::style_helpers::{
    TaffyGridLine, TaffyGridSpan, evenly_sized_tracks, flex, fr, line, minmax, repeat, span,
};
