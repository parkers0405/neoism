//! macOS optical cursor: UI-thread targets and native Metal pre-present encoding.
use crate::app::App;

#[cfg(target_os = "macos")]
unsafe extern "C" {
    fn neoism_glass_attach(view: *mut std::ffi::c_void);
    fn neoism_glass_update(quad: *const f32, velocity: *const f32, clip: *const f32, enabled: bool, placed: bool, visible: bool) -> bool;
    fn neoism_glass_reduce_motion() -> bool;
    fn neoism_glass_encode(buffer: *mut std::ffi::c_void, texture: *mut std::ffi::c_void, scale: f64);
    fn neoism_glass_animating() -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::Focus, tabs::TabKind, ui::tab_drag::Source};
    use std::time::{Duration, Instant};

    #[test]
    fn glass_cursor_tab_and_workspace_layout_handoffs_never_relight_fallback() {
        let mut app = App::new();
        app.cursor.liquid_glass = true;
        let first = app.active_tab().id;
        app.open_tab(TabKind::Agent { session: None });
        let second = app.active_tab().id;
        let mut now = Instant::now();
        app.cursor.tick(Some([100.0, 100.0, 9.0, 22.0]), now);
        // This is the live binding held by an already mounted ordinary cursor.
        let mounted_opacity = app.cursor.blink.clone();
        assert_eq!(mounted_opacity.get(), 1.0);
        assert!(app.cursor.set_native_glass(true));
        assert_eq!(mounted_opacity.get(), 0.0);
        for workspace in [false, true] {
            if workspace { app.new_workspace(); }
            for index in [0, 1, 0, 1] {
                if workspace { app.select_workspace(index); }
                else { app.select_tab([first, second][index]); }
                app.focus = Focus::Buffers(app.active);
                let key = Source::Buffer(app.active_tab().id).rect_key();
                app.rects.remove(&key);
                assert!(app.cursor_target().is_none(), "incoming tab has not laid out yet");
                now += Duration::from_millis(8);
                app.cursor.tick(app.cursor_target(), now);
                assert!(app.cursor.native_glass);
                assert!(!app.cursor.placed());
                assert_eq!(mounted_opacity.get(), 0.0);
                app.rects.insert(key, [100.0 + index as f32 * 180.0, 40.0, 160.0, 32.0]);
                now += Duration::from_millis(8);
                app.cursor.tick(app.cursor_target(), now);
                assert!(app.cursor.placed());
                assert!(app.cursor.visible);
                assert_eq!(mounted_opacity.get(), 0.0, "native layout recovery must not relight a mounted Rune quad");
            }
        }
        // Unsupported backend, accessibility or a real occluding popup restores the
        // ordinary cursor immediately, without waiting for movement or a blink.
        assert!(app.cursor.set_native_glass(false));
        assert_eq!(mounted_opacity.get(), 1.0);
        app.cursor.tick(None, now);
        assert_eq!(mounted_opacity.get(), 0.0, "missing target hides the mounted fallback too");
    }
}

#[cfg(target_os = "macos")]
pub fn attach(window: &rune::WindowHandle) {
    let _ = window.with_raw_handle(|handle| {
        if let rune::raw_window_handle::RawWindowHandle::AppKit(handle) = handle.as_raw() {
            // SAFETY: Rune owns this live NSView. Swift initializes target state on the UI thread;
            // the per-view hook borrows Metal objects only for encoding, with explicit GPU lifetimes.
            unsafe { neoism_glass_attach(handle.ns_view.as_ptr()); }
            unsafe { rune::native_frame::install(handle.ns_view.as_ptr() as usize, neoism_glass_encode); }
        }
    });
}

pub fn animating() -> bool {
    #[cfg(target_os = "macos")]
    { unsafe { neoism_glass_animating() } }
    #[cfg(not(target_os = "macos"))]
    { false }
}

pub fn reduce_motion() -> bool {
    #[cfg(target_os = "macos")]
    { unsafe { neoism_glass_reduce_motion() } }
    #[cfg(not(target_os = "macos"))]
    { false }
}

pub fn update(app: &mut App, target: Option<[f32; 4]>) -> bool {
    #[cfg(target_os = "macos")]
    {
        let clip = app.cursor_clip().unwrap_or([0.0, 0.0, app.window[0], app.window[1]]);
        // A final native pass cannot sit between Rune draw calls. Preserve popup occlusion
        // by using Rune's ordinary cursor whenever an underlying caret has popups over it.
        let enabled = app.cursor.liquid_glass && crate::ui::chrome::glass_cursor_allowed(app);
        // The original four independent corner springs own geometry for BOTH renderers.
        // carry() already transported every corner; Swift must not spring or scroll it again.
        let quad = app.cursor.trail.quad();
        let velocity = app.cursor.trail.velocity();
        let active = unsafe { neoism_glass_update(quad.as_ptr().cast(), velocity.as_ptr(), clip.as_ptr(),
            enabled, target.is_some(), app.cursor.visible) };
        app.cursor.set_native_glass(active)
    }
    #[cfg(not(target_os = "macos"))]
    { let _ = target; app.cursor.set_native_glass(false) }
}