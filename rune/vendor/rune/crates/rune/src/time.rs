//! Clocks. Native clock types remain unchanged; browsers read the page's `performance.now()`
//! ([`page_clock`]). [`LocalTime`] reads an instant off the user's wall clock.
#[cfg(not(target_arch = "wasm32"))]
pub use std::time::Instant;
#[cfg(target_arch = "wasm32")]
pub use page_clock::Instant;

mod local;
#[cfg(any(test, target_arch = "wasm32"))]
#[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
mod page_clock;

pub use local::LocalTime;

/// The same moment on winit's clock, for `ControlFlow::WaitUntil`.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn for_winit(at: Instant) -> Instant { at }
#[cfg(target_arch = "wasm32")]
pub(crate) fn for_winit(at: Instant) -> web_time::Instant { at.to_web_time() }
