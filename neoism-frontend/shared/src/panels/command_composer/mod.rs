// Warp-style sticky command composer — drawn as a sugarloaf overlay
// pinned to the bottom edge of the active terminal pane. Lives outside
// the terminal cell grid: every visible element (rounded chassis, cwd
// chip, animated `>>>` chevrons, editable text, ghost suggestion,
// caret, submit chip, hint line) is composed from sugarloaf primitives
// and `text_mut().draw` calls.
//
// The composer is sized in logical pixels (a function of font size +
// chrome scale). Callers translate that height into "cell rows" so the
// terminal viewport can shrink by the same amount above the chassis,
// keeping pixel scrolling math correct without the prompt eating PTY
// rows.
//
// ── Lift status (cross-frontend, post-shim) ──────────────────────
//
// This is the verbatim port of the rich native composer that used to
// live in `frontends/neoism/src/chrome/panels/command_composer/`.
// Both native winit and the web wasm frontend render through the
// types defined here, so the Warp-style bottom command bar paints
// identically across surfaces. The host (native shim / web bridge)
// owns the disk-persisted history, the shell-spawn
// `CommandService::run` implementation, and the cell-grid scrolling
// math that integrates the composer with the terminal pane.

mod classify;
mod completion;
mod render;
mod scrollbar;
mod shell_badge;
mod state;
mod text_viewport;
mod types;
mod update;
mod util;

pub use state::CommandComposer;
pub use types::{
    InputClassification, InputTextStyle, COMPOSER_BOTTOM_PAD, COMPOSER_TOP_OVERHANG,
};
