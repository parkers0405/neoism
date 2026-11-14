//! `App::float_on_open`: tiling window managers float a toplevel that maps at a fixed size, so
//! the window maps with its minimum equal to its maximum and takes its own limits once its
//! first frame is shown.

use winit::dpi::{LogicalSize, PhysicalSize, Size};
use winit::window::{Window, WindowAttributes};

use crate::windows::WindowOptions;

/// `attrs` with the window's size as both its minimum and its maximum.
pub(super) fn fixed(attrs: WindowAttributes, options: &WindowOptions, size: (f32, f32)) -> WindowAttributes {
    let fixed: Size = match options.physical_size {
        Some((w, h)) => PhysicalSize::new(w, h).into(),
        None => LogicalSize::new(size.0, size.1).into(),
    };
    attrs.with_min_inner_size(fixed).with_max_inner_size(fixed)
}

/// Mapped at a fixed size, so a tiling window manager floated it: now the user can resize it.
pub(super) fn release(window: &Window, options: &WindowOptions, app_min: Option<(f32, f32)>) {
    let (min, max) = options.size_limits(app_min);
    window.set_min_inner_size(Some(LogicalSize::new(min.0, min.1)));
    window.set_max_inner_size(max.map(|(w, h)| LogicalSize::new(w, h)));
}
