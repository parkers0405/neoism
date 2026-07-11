//! Shared editor/terminal scroll math.
//!
//! Host frontends still own input devices, nvim RPC, and renderer side
//! effects. This module keeps the portable viewport and autoscroll
//! rules in one place so desktop and web can make identical decisions.

mod editor;
mod misc;
mod terminal;

pub use editor::*;
pub use misc::*;
pub use terminal::*;

#[cfg(test)]
mod tests;
