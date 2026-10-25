#![doc = include_str!("../README.md")]
#![forbid(unsafe_code)]

pub mod component;
mod element;
mod geometry;
pub mod input;
pub mod routing;
mod runtime;
pub mod scroll;
pub mod task;
mod text;
pub mod widgets;

pub use component::{
    Component, ComponentContext, ComponentError, ComponentHandle, ComponentHost,
    ComponentStats,
};
pub use element::{Align, Element, Insets, Justify, Role, Semantics, Size, Style};
pub use geometry::{Color, Rect};
pub use routing::{Dispatch, EventBinding, EventKind, EventPhase, KeyBinding, RoutedAction};
pub use runtime::{
    Frame, FrameId, HitRegion, LayoutError, PaintCommand, Runtime, RuntimeStats, SemanticNode,
};
pub use text::{MeasurementKey, TextMeasurer, TextStyle};

#[cfg(test)]
mod tests;
