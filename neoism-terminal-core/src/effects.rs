//! Side-effect representation produced by the terminal state.
//!
//! Phase 2 of the libghostty-style migration: instead of having
//! `Crosswords` and the ANSI `Handler` impl reach into a native
//! `EventListener` directly, terminal-originated side effects are now
//! pushed into a buffer of `TerminalEffect` values. The host
//! (native, headless test, future wasm) drains them and dispatches
//! them however it wants.
//!
//! The effect types are intentionally dependency-clean: no native
//! window deps, no sugarloaf, no copypasta. Where a side effect
//! carries data that is rendered by sugarloaf (graphics atlases) the
//! payload is type-erased via [`GraphicsUpdate`], which lets the host
//! pass through any `Send + Sync` value and downcast it at the
//! adapter boundary.

use std::any::Any;
use std::path::PathBuf;

/// Which system clipboard a clipboard effect targets.
///
/// Mirrors the historical `neoism_backend::clipboard::ClipboardType` —
/// the backend re-exports this type so existing call sites keep
/// compiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardType {
    Clipboard,
    Selection,
}

/// Minimal RGB colour used by terminal effects.
///
/// Re-export of the canonical `crate::colors::ColorRgb`
/// — phase 3b consolidated the terminal-engine `ColorRgb` definition
/// into the `colors` module so the ANSI handler, Crosswords, and the
/// effect channel all share one type.
pub use crate::colors::ColorRgb;

/// State portion of an OSC 9;4 progress report (ConEmu/Windows Terminal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgressState {
    /// Remove/hide the progress bar (state 0).
    Remove,
    /// Set progress with a specific percentage (state 1).
    Set,
    /// Show error state (state 2).
    Error,
    /// Indeterminate/pulsing progress (state 3).
    Indeterminate,
    /// Paused progress (state 4).
    Pause,
}

/// Progress report payload emitted via OSC 9;4.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ProgressReport {
    /// The progress bar state.
    pub state: ProgressState,
    /// Optional progress percentage (0-100), only used with Set, Error,
    /// and Pause states.
    pub progress: Option<u8>,
}

/// Opaque graphics update payload.
///
/// `neoism-terminal-core` deliberately does not pull in `sugarloaf`
/// (the source of `GraphicData`/`UpdateQueues`), so graphics updates
/// flow through the effect channel as a type-erased `Box<dyn Any>`.
/// The native adapter downcasts it back to the concrete
/// `UpdateQueues` before forwarding to the renderer.
pub struct GraphicsUpdate(pub Box<dyn Any + Send + Sync>);

impl GraphicsUpdate {
    pub fn new<T: Any + Send + Sync>(payload: T) -> Self {
        Self(Box::new(payload))
    }

    /// Attempt to downcast the type-erased payload to a concrete type.
    pub fn downcast<T: Any + Send + Sync>(
        self,
    ) -> Result<Box<T>, Box<dyn Any + Send + Sync>> {
        self.0.downcast::<T>()
    }
}

impl std::fmt::Debug for GraphicsUpdate {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphicsUpdate").finish_non_exhaustive()
    }
}

/// Kind of OSC text-area-size response the terminal is asking the
/// host to produce.
///
/// Two flavours exist today:
/// * `Pixels` — `CSI 14 t` style response (`\x1b[4;<h>;<w>t`).
/// * `GraphicsAttribute` — `CSI ? Pi ; Pa ; Pv S` response used by
///   the sixel graphics-attribute query (`xterm` quirk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextAreaSizeRequestKind {
    /// Plain `CSI 14 t` reply.
    Pixels,
    /// Graphics-attribute (sixel) reply, requested with the given
    /// `Pi` parameter (always `2` in practice, but we keep it
    /// explicit so the adapter can faithfully reproduce the
    /// historical closure).
    GraphicsAttribute { pi: u16 },
}

/// Every side effect the terminal state can produce.
///
/// Variants intentionally drop the `route_id` and `window_id` fields
/// that historically rode on `RioEvent`: the native adapter
/// re-attaches them when it dispatches to the real event loop.
pub enum TerminalEffect {
    /// Bytes to be written back to the PTY.
    PtyWrite(Vec<u8>),

    /// Set the window/tab title.
    SetTitle(String),

    /// Reset the window/tab title to its default.
    ResetTitle,

    /// Ring the terminal bell.
    Bell,

    /// Store a piece of text in the named clipboard.
    ClipboardStore { ty: ClipboardType, text: String },

    /// Read a piece of text from the named clipboard and write an
    /// OSC 52 response back to the PTY.
    ClipboardLoad {
        ty: ClipboardType,
        /// The raw OSC 52 clipboard selector byte that was requested
        /// (`b'c'`, `b'p'`, or `b's'`). Re-emitted verbatim in the
        /// response so the adapter reproduces the historical
        /// `\x1b]52;<c>;<base64><terminator>` format byte-for-byte.
        clipboard_byte: u8,
        /// The escape-sequence terminator the response should be
        /// suffixed with (`"\x07"` for BEL-terminated OSC, `"\x1b\\"`
        /// for ST-terminated OSC).
        terminator: String,
    },

    /// Send a desktop notification (OSC 9 / OSC 777).
    DesktopNotification { title: String, body: String },

    /// Ask the host to open a Neoism editor tab.
    OpenEditorTab { path: Option<PathBuf> },

    /// Change the cwd of the terminal that emitted a private OSC 777 command.
    ChangeTerminalDirectory { path: PathBuf },

    /// Query the current value of palette colour `index` and write
    /// the response back to the PTY.
    ///
    /// `prefix` is the OSC `Ps` portion (e.g. `"4;<n>"` for palette
    /// colours or `"10"`/`"11"`/`"12"` for the named foreground /
    /// background / cursor colours), and `terminator` is the OSC
    /// terminator that closes the response (BEL `"\x07"` or ST
    /// `"\x1b\\"`). Together they let the adapter regenerate the
    /// reply string byte-for-byte the way the old closure did.
    ColorRequest {
        prefix: String,
        index: usize,
        terminator: String,
    },

    /// Change a palette colour (or reset it when `color` is `None`).
    ColorChange {
        index: usize,
        color: Option<ColorRgb>,
    },

    /// Query the text-area size, writing a response back to the PTY.
    TextAreaSizeRequest {
        kind: TextAreaSizeRequestKind,
        /// Only used by `Pixels` today; carried verbatim so the
        /// adapter can reproduce historical behaviour 1:1.
        terminator: String,
    },

    /// New sixel/kitty graphics updates from the parser.
    GraphicsUpdate(GraphicsUpdate),

    /// OSC 9;4 progress bar report.
    ProgressReport(ProgressReport),

    /// Cursor blinking state has changed (config or DEC mode).
    CursorBlinkingChange,

    /// Grid has changed in a way that may require a different mouse
    /// cursor shape.
    MouseCursorDirty,

    /// Shutdown request (DECRST 1049 / `exit` from a guest).
    Exit,

    /// Request a render of this specific terminal's route on the host.
    ///
    /// Historically emitted as `RioEvent::RenderRoute(route_id)` from
    /// `Crosswords::mark_fully_damaged`. The semantics differ from
    /// [`TerminalEffect::Dirty`] in that the host actively asks the
    /// renderer to repaint this route now, rather than merely noting
    /// that the grid has changed; the adapter re-attaches the route id
    /// at dispatch time.
    RenderRequest,

    /// Generic "the terminal got dirty, please redraw" hint.
    Dirty,
}

impl std::fmt::Debug for TerminalEffect {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TerminalEffect::PtyWrite(bytes) => f
                .debug_tuple("PtyWrite")
                .field(&format_args!("<{} bytes>", bytes.len()))
                .finish(),
            TerminalEffect::SetTitle(t) => f.debug_tuple("SetTitle").field(t).finish(),
            TerminalEffect::ResetTitle => write!(f, "ResetTitle"),
            TerminalEffect::Bell => write!(f, "Bell"),
            TerminalEffect::ClipboardStore { ty, .. } => f
                .debug_struct("ClipboardStore")
                .field("ty", ty)
                .finish_non_exhaustive(),
            TerminalEffect::ClipboardLoad {
                ty,
                clipboard_byte,
                terminator,
            } => f
                .debug_struct("ClipboardLoad")
                .field("ty", ty)
                .field("clipboard_byte", clipboard_byte)
                .field("terminator", terminator)
                .finish(),
            TerminalEffect::DesktopNotification { title, body } => f
                .debug_struct("DesktopNotification")
                .field("title", title)
                .field("body", body)
                .finish(),
            TerminalEffect::OpenEditorTab { path } => {
                f.debug_struct("OpenEditorTab").field("path", path).finish()
            }
            TerminalEffect::ChangeTerminalDirectory { path } => f
                .debug_struct("ChangeTerminalDirectory")
                .field("path", path)
                .finish(),
            TerminalEffect::ColorRequest {
                prefix,
                index,
                terminator,
            } => f
                .debug_struct("ColorRequest")
                .field("prefix", prefix)
                .field("index", index)
                .field("terminator", terminator)
                .finish(),
            TerminalEffect::ColorChange { index, color } => f
                .debug_struct("ColorChange")
                .field("index", index)
                .field("color", color)
                .finish(),
            TerminalEffect::TextAreaSizeRequest { kind, terminator } => f
                .debug_struct("TextAreaSizeRequest")
                .field("kind", kind)
                .field("terminator", terminator)
                .finish(),
            TerminalEffect::GraphicsUpdate(_) => write!(f, "GraphicsUpdate(<opaque>)"),
            TerminalEffect::ProgressReport(r) => {
                f.debug_tuple("ProgressReport").field(r).finish()
            }
            TerminalEffect::CursorBlinkingChange => write!(f, "CursorBlinkingChange"),
            TerminalEffect::MouseCursorDirty => write!(f, "MouseCursorDirty"),
            TerminalEffect::Exit => write!(f, "Exit"),
            TerminalEffect::RenderRequest => write!(f, "RenderRequest"),
            TerminalEffect::Dirty => write!(f, "Dirty"),
        }
    }
}
