//! `Cursor::Image` on web: the canvas's CSS cursor names the PNG itself at 2x, so the browser
//! decodes it and draws it at its size in points, and the wasm build carries no PNG decoder for
//! cursors. winit's custom cursors would need the pixels and draw them in CSS pixels, twice too
//! big on a 2x display.
//!
//! winit holds the arrow while Rune's CSS shows, so it has no cursor of its own to put back over
//! it, and [`forget`] clears the CSS before winit sets the next shape.

use std::cell::RefCell;
use std::sync::Arc;

use winit::event_loop::ActiveEventLoop;
use winit::platform::web::WindowExtWebSys;
use winit::window::{CursorIcon, Window};

use crate::cursor::CursorImage;
use crate::cursor::image::{MAX_ENTRIES, css};
use crate::platform::web::platform::{object_url, revoke_url, set_cursor};

thread_local! {
    /// The CSS for each image set so far, oldest first, with the object URL it holds; `None` for
    /// an image that isn't a PNG Rune takes, logged once.
    static MADE: RefCell<Vec<(CursorImage, Option<(String, String)>)>> = const { RefCell::new(Vec::new()) };
}

pub(super) fn set(window: &Arc<Window>, image: CursorImage, _scale: f32) {
    window.set_cursor(CursorIcon::Default);
    let Some(canvas) = window.canvas() else { return };
    if let Some(css) = made(image) { set_cursor(&canvas, &css); }
}

pub(super) fn forget(window: &Arc<Window>) {
    if let Some(canvas) = window.canvas() { set_cursor(&canvas, ""); }
}

pub(super) fn make_pending(_: &ActiveEventLoop) {}

fn made(image: CursorImage) -> Option<String> {
    MADE.with(|made| {
        let mut made = made.borrow_mut();
        let entry = match made.iter().position(|(key, _)| *key == image) {
            Some(i) => made.remove(i),
            None => {
                let url = object_url(image.bytes(), "image/png");
                let entry = match css(image, &url) {
                    Ok(css) => Some((css, url)),
                    Err(problem) => {
                        revoke_url(&url);
                        eprintln!("rune: cursor image ({} bytes) shows the arrow instead: {problem}", image.bytes().len());
                        None
                    }
                };
                (image, entry)
            }
        };
        let css = entry.1.as_ref().map(|(css, _)| css.clone());
        made.push(entry);
        if made.len() > MAX_ENTRIES && let (_, Some((_, url))) = made.remove(0) { revoke_url(&url); }
        css
    })
}
