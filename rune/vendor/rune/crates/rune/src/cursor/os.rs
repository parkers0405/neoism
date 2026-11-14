//! The OS cursor a winit host last applied.
//!
//! `Cursor::Hidden` sets a transparent cursor image rather than winit's
//! `set_cursor_visible(false)`: on Windows that call also confines the pointer to the centre of
//! a focused window, and its `ShowCursor` count is process-wide. A cursor image is per-window
//! like any other shape, so the OS drops it when the pointer leaves the window or the window
//! goes away.
//!
//! `Cursor::Image` goes through winit's custom cursors, made at the window's scale so the
//! image keeps its size in points. winit makes those only with the event loop at hand, so a
//! window's first use of an image waits for [`make_pending`] before the loop sleeps. On macOS
//! winit would size the image in pixels, twice too big on Retina, so `macos_cursor` sets it.
//! With the window's cursor rects off, nothing in AppKit puts the image back once the system
//! shows another cursor (a window coming or going over the pointer, another app's click-through
//! overlay), so while it holds the pointer the host sets it again every [`REASSERT`], frame or
//! no frame. On web the canvas's CSS names the PNG itself (`cursor/web.rs`).

use std::sync::Arc;
use std::time::Duration;

use crate::time::Instant;

use winit::event_loop::ActiveEventLoop;
use winit::window::{CursorIcon, CustomCursor, Window};

use crate::el::Cursor;

/// How often an image cursor is set again while it holds the pointer and nothing redraws.
pub(crate) const REASSERT: Duration = Duration::from_millis(50);

pub(crate) struct OsCursor {
    applied: (bool, Cursor),
    /// The scale an image cursor was last made at: a window moving to a display with another
    /// scale makes it again.
    scale: f32,
    transparent: CustomCursor,
}

impl OsCursor {
    pub(crate) fn new(event_loop: &ActiveEventLoop) -> Self {
        OsCursor { applied: (false, Cursor::Default), scale: 0.0, transparent: transparent(event_loop) }
    }

    /// Applies `want` while the host is `active` (the pointer is over its window or its hit
    /// area), and the arrow once when it stops being active. winit sets some cursors at once
    /// (Win32 `SetCursor`), so an inactive host must leave the cursor to whichever window has
    /// the pointer.
    pub(crate) fn apply(&mut self, window: &Arc<Window>, want: Cursor, active: bool) {
        let scale = image_scale(window);
        let rescaled = active && matches!(want, Cursor::Image(_)) && std::mem::replace(&mut self.scale, scale) != scale;
        let Some(next) = next(&mut self.applied, want, active).or(rescaled.then_some(want)) else {
            self.reassert();
            return;
        };
        #[cfg(target_os = "macos")]
        let Some(next) = crate::platform::macos::cursor::window_cursor(window, next) else { return };
        #[cfg(not(target_os = "macos"))]
        images::forget(window);
        match next {
            Cursor::Hidden => window.set_cursor(self.transparent.clone()),
            #[cfg(not(target_os = "macos"))]
            Cursor::Image(image) => images::set(window, image, scale),
            other => window.set_cursor(icon(other)),
        }
    }

    /// When to set the image cursor again: while one holds the pointer, on macOS.
    pub(crate) fn reassert_at(&self, now: Instant) -> Option<Instant> {
        (cfg!(target_os = "macos") && holds_image(self.applied)).then(|| now + REASSERT)
    }

    /// Sets the image cursor again, if one holds the pointer.
    pub(crate) fn reassert(&self) {
        #[cfg(target_os = "macos")]
        if let (true, Cursor::Image(image)) = self.applied { crate::platform::macos::cursor::reassert(image); }
    }
}

/// A 1×1 clear cursor. On web it is made once per page and kept: winit makes a custom cursor
/// asynchronously, and dropping one before it's ready leaves the browser calling a freed
/// closure, as a web app unmounted just after it mounts would.
fn transparent(event_loop: &ActiveEventLoop) -> CustomCursor {
    let make = || {
        let source = CustomCursor::from_rgba(vec![0u8; 4], 1, 1, 0, 0).expect("a 1x1 cursor image is valid");
        event_loop.create_custom_cursor(source)
    };
    #[cfg(target_arch = "wasm32")]
    {
        thread_local! { static CLEAR: std::cell::OnceCell<CustomCursor> = const { std::cell::OnceCell::new() }; }
        CLEAR.with(|clear| clear.get_or_init(make).clone())
    }
    #[cfg(not(target_arch = "wasm32"))]
    make()
}

fn holds_image(applied: (bool, Cursor)) -> bool {
    matches!(applied, (true, Cursor::Image(_)))
}

/// Pixels per point in the OS cursors made for `window`.
fn image_scale(window: &Window) -> f32 {
    if cfg!(any(target_os = "macos", target_arch = "wasm32")) { crate::cursor::image::SOURCE_SCALE } else { window.scale_factor() as f32 }
}

/// Makes the image cursors windows asked for since the loop last slept, and sets them.
pub(crate) fn make_pending(event_loop: &ActiveEventLoop) {
    #[cfg(not(target_os = "macos"))]
    images::make_pending(event_loop);
    #[cfg(target_os = "macos")]
    let _ = event_loop;
}

#[cfg(target_arch = "wasm32")]
#[path = "web.rs"]
mod images;

#[cfg(not(any(target_os = "macos", target_arch = "wasm32")))]
mod images {
    use std::cell::RefCell;
    use std::sync::{Arc, Weak};

    use winit::event_loop::ActiveEventLoop;
    use winit::window::{CursorIcon, CustomCursor, Window};

    use crate::cursor::CursorImage;
    use crate::cursor::image::ImageCursors;

    thread_local! {
        static CURSORS: RefCell<ImageCursors<CustomCursor>> = const { RefCell::new(ImageCursors::new()) };
        static PENDING: RefCell<Vec<(Weak<Window>, CursorImage, f32)>> = const { RefCell::new(Vec::new()) };
    }

    pub(super) fn set(window: &Arc<Window>, image: CursorImage, scale: f32) {
        match CURSORS.with(|c| c.borrow_mut().cached(image, scale)) {
            Some(Some(cursor)) => window.set_cursor(cursor),
            Some(None) => window.set_cursor(CursorIcon::Default),
            None => PENDING.with(|p| p.borrow_mut().push((Arc::downgrade(window), image, scale))),
        }
    }

    /// Drops a cursor `window` asked for and no longer wants.
    pub(super) fn forget(window: &Arc<Window>) {
        PENDING.with(|p| p.borrow_mut().retain(|(w, ..)| !std::ptr::eq(w.as_ptr(), Arc::as_ptr(window))));
    }

    pub(super) fn make_pending(event_loop: &ActiveEventLoop) {
        for (window, image, scale) in PENDING.with(|p| std::mem::take(&mut *p.borrow_mut())) {
            let Some(window) = window.upgrade() else { continue };
            let cursor = CURSORS.with(|c| c.borrow_mut().get(image, scale, |d| {
                let side = |n: u32| u16::try_from(n).map_err(|_| format!("{n} px is too big"));
                let source = CustomCursor::from_rgba(d.rgba.clone(), side(d.width)?, side(d.height)?, d.hot[0] as u16, d.hot[1] as u16)
                    .map_err(|e| e.to_string())?;
                Ok(event_loop.create_custom_cursor(source))
            }));
            match cursor {
                Some(cursor) => window.set_cursor(cursor),
                None => window.set_cursor(CursorIcon::Default),
            }
        }
    }
}

/// The cursor to set, if the (active, cursor) pair changed and the host owns the cursor or
/// just gave it up.
fn next(applied: &mut (bool, Cursor), want: Cursor, active: bool) -> Option<Cursor> {
    let want = if active { want } else { Cursor::Default };
    let was = std::mem::replace(applied, (active, want));
    (was != (active, want) && (active || was.0)).then_some(want)
}

/// winit's shape for `c`. An image that never became an OS cursor is the arrow.
fn icon(c: Cursor) -> CursorIcon {
    match c {
        Cursor::Default | Cursor::Hidden | Cursor::Image(_) => CursorIcon::Default,
        Cursor::Pointer => CursorIcon::Pointer,
        Cursor::Text => CursorIcon::Text,
        Cursor::Grab => CursorIcon::Grab,
        Cursor::Grabbing => CursorIcon::Grabbing,
        Cursor::NotAllowed => CursorIcon::NotAllowed,
        Cursor::Move => CursorIcon::Move,
        Cursor::ResizeColumn => CursorIcon::ColResize,
        Cursor::ResizeRow => CursorIcon::RowResize,
        Cursor::Crosshair => CursorIcon::Crosshair,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_sets_its_cursor_while_active_and_the_arrow_once_when_it_stops() {
        let mut applied = (false, Cursor::Default);
        assert_eq!(next(&mut applied, Cursor::Crosshair, false), None, "never while inactive");
        assert_eq!(next(&mut applied, Cursor::Crosshair, true), Some(Cursor::Crosshair));
        assert_eq!(next(&mut applied, Cursor::Hidden, true), Some(Cursor::Hidden));
        assert_eq!(next(&mut applied, Cursor::Hidden, true), None);
        assert_eq!(next(&mut applied, Cursor::Hidden, false), Some(Cursor::Default), "the arrow once on leaving");
        assert_eq!(next(&mut applied, Cursor::Hidden, false), None, "then another window's cursor stays");
        assert_eq!(next(&mut applied, Cursor::Pointer, false), None);
        assert_eq!(next(&mut applied, Cursor::Default, true), Some(Cursor::Default), "entering claims the cursor");
        assert_eq!(next(&mut applied, Cursor::Default, true), None);
    }

    #[test]
    fn only_an_image_holding_the_pointer_is_set_again() {
        static PNG: [u8; 4] = [1, 2, 3, 4];
        let image = Cursor::Image(crate::cursor::CursorImage::png(&PNG, [0.0, 0.0]));
        let mut applied = (false, Cursor::Default);
        next(&mut applied, image, true);
        assert!(holds_image(applied));
        next(&mut applied, image, false);
        assert!(!holds_image(applied), "not once the pointer leaves");
        next(&mut applied, Cursor::Crosshair, true);
        assert!(!holds_image(applied), "nor for a system shape, which the window's cursor rects keep");
    }
}
