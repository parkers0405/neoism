//! `.neodraw` — a homegrown, Excalidraw-style vector sketch surface.
//!
//! A `.neodraw` file is a JSON [`Scene`]: a z-ordered list of hand-drawn
//! [`Shape`]s (rect / ellipse / line / arrow / freehand / text). It opens
//! in a full interactive [`DrawPane`] editor, and the same [`Scene`] can be
//! rendered read-only inside markdown via a ```draw fence that references a
//! `.neodraw` file.
//!
//! Layering mirrors [`markdown`](crate::editor::markdown): the pure model +
//! viewport state lives here in the shared crate; sugarloaf rendering and
//! pointer input land alongside it in later phases.

mod create;
mod edit;
mod geometry;
mod graph_sim;
mod history;
mod input;
mod pane;
mod render;
mod scene;
mod sidecar;
mod text;
mod toolbar;

pub use create::Draft;
pub use edit::Handle;
pub use geometry::Bounds;
pub use graph_sim::{GraphNode, GraphSim};
pub use history::History;
pub use input::DrawGesture;
pub use pane::{is_neodraw_path, Camera, DrawPane, Tool};
pub use render::{render_pane, render_pane_overlay, render_scene, HANDLE_HALF_PX};
pub use scene::{
    ArrowHead, Color, Scene, Shape, ShapeId, ShapeKind, Style, Vec2, SCENE_VERSION,
};
pub use sidecar::{
    ink_sidecar_path, legacy_ink_sidecar_path, load_ink_layer, migrate_legacy_ink,
    strokes_only,
};
