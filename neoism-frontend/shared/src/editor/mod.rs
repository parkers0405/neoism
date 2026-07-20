//! Editor surfaces lifted into the shared crate.
//!
//! Each submodule mirrors a buffer kind the native frontend hosts in
//! `frontends/neoism/src/editor/`. The pure state + helper logic
//! lives here so the same buffer can drive both the native winit
//! shell and the web wasm shell; render code that needs the native
//! `IdeTheme` and sugarloaf primitives stays in the native shim.

pub mod code;
pub mod crdt;
pub mod markdown;
pub mod neodraw;
pub mod notebook;
pub mod optimistic;
pub mod reconcile;
pub mod scroll_model;
pub mod selection_model;
