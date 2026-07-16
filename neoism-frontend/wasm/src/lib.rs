//! wasm-bindgen wrapper around the real `neoism-terminal-core` engine.
//!
//! Two surfaces:
//!
//! 1. `Terminal` — synchronous Crosswords + Processor wrapper, no GPU.
//!    Hosts can feed bytes, drain effects, and read snapshots; they
//!    paint pixels themselves (e.g. canvas2d, xterm.js, etc.). This is
//!    the data-only surface, available on every build.
//!
//! 2. `RenderedTerminal` (wasm32 only) — owns a `sugarloaf::Sugarloaf`
//!    bound to a browser canvas and drives it from the same Crosswords.
//!    JS calls `await RenderedTerminal.new(canvas, ...)`, then `feed`
//!    + `render` per RAF. This is the libghostty endgame: sugarloaf
//!    paints cells via WebGPU/WebGL, no xterm.js in the loop.

use neoism_terminal_core::ansi::CursorShape;
use neoism_terminal_core::handler::Processor;
use neoism_terminal_core::{Crosswords, TerminalEffect, TerminalId};
use wasm_bindgen::prelude::*;

#[cfg(target_arch = "wasm32")]
use neoism_terminal_core::snapshot::{
    CellFlags, CellSnapshot, ColorIndex, CursorShape as SnapshotCursorShape,
    CursorSnapshot, RgbTriple, ThemeSnapshot,
};

#[wasm_bindgen(start)]
pub fn start() {
    console_error_panic_hook::set_once();
}

#[wasm_bindgen]
pub fn island_chrome_spec(scale: f32) -> JsValue {
    serde_wasm_bindgen::to_value(&neoism_ui::widgets::island::island_chrome_spec(scale))
        .unwrap_or(JsValue::NULL)
}

#[wasm_bindgen]
pub fn island_tab_label(content: &str, program: Option<String>) -> String {
    neoism_ui::widgets::island::island_tab_label(content, program.as_deref())
}

#[wasm_bindgen]
pub struct Terminal {
    inner: Crosswords,
    processor: Processor,
}

#[wasm_bindgen]
impl Terminal {
    #[wasm_bindgen(constructor)]
    pub fn new(cols: u32, rows: u32) -> Terminal {
        let inner = Crosswords::new(
            (rows as usize, cols as usize),
            CursorShape::Block,
            TerminalId::new(0),
            1000,
        );
        Terminal {
            inner,
            processor: Processor::new(),
        }
    }

    pub fn feed(&mut self, bytes: &[u8]) {
        self.processor.advance(&mut self.inner, bytes);
    }

    pub fn resize(&mut self, cols: u32, rows: u32) {
        self.inner.resize((rows as usize, cols as usize));
    }

    /// Drains bytes the terminal wants written back to the PTY
    /// (DSR/cursor-position/clipboard-OSC responses) since the last
    /// call. Other side effects (bell, title, etc.) flow through
    /// `drain_effects_json`.
    pub fn take_pty_writes(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        let drained: Vec<_> = self.inner.drain_effects().collect();
        for e in drained {
            if let TerminalEffect::PtyWrite(bytes) = e {
                out.extend_from_slice(&bytes);
            }
        }
        out
    }

    /// Drains non-PTY side effects (bell, title, clipboard, etc.) as a
    /// JSON-serialized array. Effects with closure or opaque payloads
    /// (`GraphicsUpdate`) are skipped — JS hosts that need them can
    /// extend this surface later.
    ///
    /// CALL ORDER matters: this drains the same buffer as
    /// `take_pty_writes`. Call this first, OR call `take_pty_writes`
    /// first and accept that this returns an empty array. Mixing both
    /// per-feed only works if you pick one consistently.
    pub fn drain_effects_json(&mut self) -> JsValue {
        let drained: Vec<_> = self.inner.drain_effects().collect();
        let mapped: Vec<WasmEffect> = drained
            .into_iter()
            .filter_map(WasmEffect::from_core)
            .collect();
        serde_wasm_bindgen::to_value(&mapped).unwrap_or(JsValue::NULL)
    }

    /// Full visible state for the renderer. Serialized as JSON-friendly
    /// JS object via `serde-wasm-bindgen`.
    pub fn snapshot(&self) -> JsValue {
        serde_wasm_bindgen::to_value(&self.inner.snapshot()).unwrap_or(JsValue::NULL)
    }

    // Host-only accessors used by the native rlib unit test.
    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn cols_host(&self) -> u32 {
        self.inner.columns() as u32
    }

    #[cfg(not(target_arch = "wasm32"))]
    #[doc(hidden)]
    pub fn rows_host(&self) -> u32 {
        self.inner.screen_lines() as u32
    }
}

#[derive(serde::Serialize)]
#[serde(tag = "type")]
enum WasmEffect {
    Bell,
    SetTitle { title: String },
    ResetTitle,
    Dirty,
    RenderRequest,
    MouseCursorDirty,
    CursorBlinkingChange,
    Exit,
    DesktopNotification { title: String, body: String },
    OpenEditorTab { path: Option<String> },
    // PtyWrite is intentionally absent — drain via take_pty_writes().
    // GraphicsUpdate, callback-bearing OSC requests, and progress
    // reports are skipped: opaque payloads or terminator-state best
    // handled on the Rust side via a later host extension.
}

impl WasmEffect {
    fn from_core(e: TerminalEffect) -> Option<Self> {
        match e {
            TerminalEffect::Bell => Some(WasmEffect::Bell),
            TerminalEffect::SetTitle(title) => Some(WasmEffect::SetTitle { title }),
            TerminalEffect::ResetTitle => Some(WasmEffect::ResetTitle),
            TerminalEffect::Dirty => Some(WasmEffect::Dirty),
            TerminalEffect::RenderRequest => Some(WasmEffect::RenderRequest),
            TerminalEffect::MouseCursorDirty => Some(WasmEffect::MouseCursorDirty),
            TerminalEffect::CursorBlinkingChange => {
                Some(WasmEffect::CursorBlinkingChange)
            }
            TerminalEffect::Exit => Some(WasmEffect::Exit),
            TerminalEffect::DesktopNotification { title, body } => {
                Some(WasmEffect::DesktopNotification { title, body })
            }
            TerminalEffect::OpenEditorTab { path } => Some(WasmEffect::OpenEditorTab {
                path: path.map(|p| p.to_string_lossy().into_owned()),
            }),
            // Skipped — see WasmEffect doc comment.
            TerminalEffect::PtyWrite(_)
            | TerminalEffect::GraphicsUpdate(_)
            | TerminalEffect::ClipboardStore { .. }
            | TerminalEffect::ClipboardLoad { .. }
            | TerminalEffect::ColorRequest { .. }
            | TerminalEffect::ColorChange { .. }
            | TerminalEffect::TextAreaSizeRequest { .. }
            | TerminalEffect::ProgressReport(_) => None,
        }
    }
}

/// Reconcile the render scale with what the swapchain ACTUALLY got.
///
/// The JS host asks for `requested_scale` (texture-cap-clamped
/// devicePixelRatio) and a physical surface of `css * requested_scale`.
/// `WgpuContext::resize` may still clamp that surface to the device's
/// `max_texture_dimension_2d` — and if sugarloaf's `scale_factor`
/// stayed at the requested value while the surface shrank, chrome
/// (laid out in CSS pixels and multiplied by `scale_factor` at draw
/// time) would paint PAST the swapchain edge: file tree spilling out
/// of frame, bottom cut off, and the browser stretching the undersized
/// backing store across the CSS rect (blur). This pure helper computes
/// the scale that makes `css * scale` fit the actual surface exactly.
///
/// * `css_w` / `css_h` — chrome layout viewport in CSS pixels.
/// * `requested_scale` — the effective DPR the host asked for.
/// * `actual_w` / `actual_h` — swapchain size after any clamping.
///
/// Returns `requested_scale` when nothing was clamped (the common
/// path), otherwise the largest scale that fits both axes. Always
/// strictly positive.
pub fn effective_render_scale(
    css_w: u32,
    css_h: u32,
    requested_scale: f32,
    actual_w: f32,
    actual_h: f32,
) -> f32 {
    let requested = if requested_scale > 0.0 {
        requested_scale
    } else {
        1.0
    };
    let w = css_w.max(1) as f32;
    let h = css_h.max(1) as f32;
    let fit = (actual_w / w).min(actual_h / h);
    if !fit.is_finite() || fit <= 0.0 {
        return requested;
    }
    // floor() in the physical-size computation makes `fit` land a hair
    // under `requested` even when nothing was clamped — keep the exact
    // requested value in that case so the common path is bit-stable.
    if fit >= requested * 0.999 {
        requested
    } else {
        fit
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod host_tests {
    use super::*;

    #[test]
    fn new_terminal_has_requested_dimensions() {
        let term = Terminal::new(80, 24);
        assert_eq!(term.cols_host(), 80);
        assert_eq!(term.rows_host(), 24);
    }

    #[test]
    fn feed_routes_to_crosswords_and_updates_cursor() {
        let mut term = Terminal::new(20, 5);
        term.feed(b"hello");
        let snap = term.inner.snapshot();
        assert_eq!(snap.cursor.col, 5);
        assert_eq!(snap.viewport[0][0].c, 'h');
    }

    #[test]
    fn bell_byte_produces_bell_effect() {
        let mut term = Terminal::new(20, 5);
        term.feed(b"\x07");
        let drained: Vec<_> = term.inner.drain_effects().collect();
        assert!(
            drained.iter().any(|e| matches!(e, TerminalEffect::Bell)),
            "expected Bell, got {drained:?}"
        );
    }

    #[test]
    fn resize_changes_dimensions() {
        let mut term = Terminal::new(80, 24);
        term.resize(100, 30);
        assert_eq!(term.cols_host(), 100);
        assert_eq!(term.rows_host(), 30);
    }

    /// Size contract: when the swapchain got exactly what was asked
    /// for (css x scale, floor-truncated), the render scale stays the
    /// requested DPR bit-for-bit — including fractional browser-zoom
    /// ratios like 1.25 / 1.5.
    #[test]
    fn effective_scale_keeps_requested_dpr_when_unclamped() {
        for &(w, h, scale) in &[
            (1280u32, 800u32, 1.0f32),
            (1280, 800, 2.0),
            (1097, 743, 1.25),
            (1366, 768, 1.5),
            (1920, 1080, 0.8),
        ] {
            let phys_w = (w as f32 * scale).max(1.0) as u32;
            let phys_h = (h as f32 * scale).max(1.0) as u32;
            let got = effective_render_scale(w, h, scale, phys_w as f32, phys_h as f32);
            assert_eq!(
                got, scale,
                "unclamped {w}x{h}@{scale} must keep the requested scale"
            );
        }
    }

    /// Size contract: when the device clamped the swapchain below
    /// css x scale, the returned scale shrinks so chrome (CSS layout x
    /// scale) fits the actual surface on BOTH axes — this is the exact
    /// mismatch that produced "file tree spills out of frame + blurry
    /// stretched canvas".
    #[test]
    fn effective_scale_fits_clamped_swapchain() {
        // 3000x1000 CSS at DPR 2 wants 6000x2000; a 2048 texture cap
        // clamps proportionally to 2048x682 (sugarloaf clamp_surface_size).
        let got = effective_render_scale(3000, 1000, 2.0, 2048.0, 682.0);
        assert!(got < 2.0, "clamped surface must lower the scale, got {got}");
        assert!(
            3000.0 * got <= 2048.0 + 0.5 && 1000.0 * got <= 682.0 + 0.5,
            "css x scale must fit the actual swapchain, got {got}"
        );
    }

    /// Degenerate inputs never produce a non-positive or NaN scale.
    #[test]
    fn effective_scale_survives_degenerate_inputs() {
        for &(w, h, scale, aw, ah) in &[
            (0u32, 0u32, 0.0f32, 0.0f32, 0.0f32),
            (1, 1, -2.0, 1.0, 1.0),
            (100, 100, 1.0, f32::NAN, 100.0),
        ] {
            let got = effective_render_scale(w, h, scale, aw, ah);
            assert!(
                got.is_finite() && got > 0.0,
                "scale must stay positive/finite for ({w},{h},{scale},{aw},{ah}), got {got}"
            );
        }
    }

    /// Gate the theme plumbing — `RenderedTerminal::render` reads the
    /// 256-entry palette and the resolved well-known slots off every
    /// snapshot. If Crosswords ever stopped emitting a full palette we
    /// want the wasm bindings to fail before we ship the regression.
    #[test]
    fn snapshot_carries_full_theme_palette() {
        let mut term = Terminal::new(80, 24);
        term.feed(b"hello");
        let snap = term.inner.snapshot();
        assert_eq!(
            snap.theme.palette.len(),
            256,
            "theme palette should always have 256 entries"
        );
    }
}

// ============================================================
// RenderedTerminal: sugarloaf-backed canvas renderer (wasm32).
// ============================================================
//
// `Sugarloaf::from_canvas` only exists on wasm32 — calling it on host
// would require a real `RawWindowHandle`. Everything below is therefore
// gated on `target_arch = "wasm32"`. The host build sees an empty
// module; `#[wasm_bindgen]` exports are inherently wasm32-only anyway.

#[cfg(target_arch = "wasm32")]
mod rendered;

#[cfg(target_arch = "wasm32")]
pub use rendered::{ChromeBridge, RenderedTerminal};
