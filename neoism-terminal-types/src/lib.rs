//! Terminal colors, coordinates, snapshots, and input declarations.
//! There is no VT parser, screen grid, renderer, or PTY owner here.
pub mod ansi;
pub mod colors;
pub mod dimensions;
pub mod mode;
pub mod position;
pub mod snapshot;
pub type Match = std::ops::RangeInclusive<position::Pos>;
pub use snapshot::*;
