//! Read-only snapshots of terminal state for renderers and remote
//! consumers.
//!
//! `TerminalSnapshot` is the boundary between the terminal engine
//! (`Crosswords`, ANSI performer, grid) and everything that wants to
//! *display* a terminal: the native renderer today, plus future
//! consumers like a WASM renderer or a workspace daemon shipping
//! frames over a wire. The shape is intentionally:
//!
//! - **Pure data.** No `Arc<dyn Any>`, no callbacks, no engine refs.
//!   Snapshots can travel between threads, processes, and machines.
//! - **`Clone + Serialize + Deserialize`.** Roundtrips through JSON
//!   (and any other serde format) so the wire protocol comes for
//!   free.
//! - **Compact.** Each `CellSnapshot` is small enough that copying a
//!   200x60 viewport per frame is acceptable. Heavy data
//!   (scrollback, graphics, hyperlinks) is not duplicated wholesale;
//!   only the visible viewport plus the metadata renderers need is
//!   captured.
//!
//! Phase 3b will wire `Crosswords::snapshot(&self) -> TerminalSnapshot`.
//! This module just defines the types.

use serde::{Deserialize, Serialize};

/// A renderable color reference. Mirrors `AnsiColor` from
/// `neoism-backend::config::colors` but as a plain data shape:
///
/// - `Default` — use the theme's default fg/bg for the relevant slot.
/// - `Named(n)` — one of the named palette slots (Black, Red, ...).
///   The encoding matches `NamedColor` ordinal; consumers that don't
///   care about the name treat it as an indexed lookup.
/// - `Indexed(n)` — 256-color palette index.
/// - `Spec { r, g, b }` — direct RGB (24-bit truecolor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum ColorIndex {
    Default,
    Named(u8),
    Indexed(u8),
    Spec { r: u8, g: u8, b: u8 },
}

bitflags::bitflags! {
    /// Per-cell render flags. Combines what Crosswords splits between
    /// `CellFlags` (wide/spacer) and `StyleFlags` (SGR attributes)
    /// into a single flat bitset that renderers can test directly.
    ///
    /// Bit layout is stable for wire compatibility; new bits append.
    #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
    pub struct CellFlags: u16 {
        const BOLD              = 1 << 0;
        const ITALIC            = 1 << 1;
        const UNDERLINE         = 1 << 2;
        const UNDERCURL         = 1 << 3;
        const STRIKEOUT         = 1 << 4;
        const REVERSE           = 1 << 5;
        const DIM               = 1 << 6;
        const HIDDEN            = 1 << 7;
        const BLINK             = 1 << 8;
        const WIDE_CHAR         = 1 << 9;
        const WIDE_CHAR_SPACER  = 1 << 10;
        // Extension flags (beyond the original spec) — kept to avoid
        // losing information renderers already use. See Square / Style
        // in neoism-backend/src/crosswords/{square,style}.rs.
        const DOUBLE_UNDERLINE  = 1 << 11;
        const DOTTED_UNDERLINE  = 1 << 12;
        const DASHED_UNDERLINE  = 1 << 13;
        /// Soft-wrap continuation marker on the last cell of a row.
        const WRAPLINE          = 1 << 14;
    }
}

/// A single rendered cell.
///
/// Fields beyond the original spec:
///
/// - `underline_color` — Crosswords' `Style` carries an optional
///   underline color (SGR 58/59); losing it would degrade fancy
///   underline rendering. `None` means "same as fg".
/// - `hyperlink_id` — opaque id matching an entry in
///   `TerminalSnapshot::hyperlinks`. We don't inline the URI per
///   cell (would blow up the payload for long URLs on hover ranges);
///   instead we intern. `None` means "no hyperlink".
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CellSnapshot {
    pub c: char,
    pub fg: ColorIndex,
    pub bg: ColorIndex,
    pub flags: CellFlags,
    pub underline_color: Option<ColorIndex>,
    pub hyperlink_id: Option<u32>,
}

impl Default for CellSnapshot {
    fn default() -> Self {
        Self {
            c: ' ',
            fg: ColorIndex::Default,
            bg: ColorIndex::Default,
            flags: CellFlags::empty(),
            underline_color: None,
            hyperlink_id: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum CursorShape {
    Block,
    Beam,
    Underline,
    Hidden,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CursorSnapshot {
    pub col: u16,
    pub row: u16,
    pub shape: CursorShape,
    pub visible: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MouseReporting {
    Off,
    X10,
    Normal,
    ButtonEvent,
    AnyEvent,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModesSnapshot {
    pub alt_screen: bool,
    pub origin: bool,
    pub auto_wrap: bool,
    pub bracketed_paste: bool,
    pub focus_events: bool,
    pub mouse_reporting: MouseReporting,
}

/// Damage hint for the renderer. `full` means "redraw everything"
/// (used after resize, mode changes, etc.); otherwise `dirty_rows`
/// holds the viewport-relative row indices that changed since the
/// previous snapshot. Renderers may ignore the hint and always
/// repaint full.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DamageSnapshot {
    pub full: bool,
    pub dirty_rows: Vec<u16>,
}

impl Default for DamageSnapshot {
    fn default() -> Self {
        Self {
            full: true,
            dirty_rows: Vec::new(),
        }
    }
}

/// A single 24-bit RGB triple, used for the theme palette emitted on
/// every snapshot. Kept as plain `u8` triples so the structure is
/// trivially serializable and cheap to copy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct RgbTriple {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

/// Active theme as seen by the engine. The web renderer (and any
/// other consumer that resolves `ColorIndex::Named` / `Indexed` /
/// `Default`) reads this off the snapshot rather than carrying its
/// own copy.
///
/// `palette` is always 256 entries long: indices 0-15 are the named
/// ANSI colors (in `NamedColor` ordinal order), 16-255 are the
/// standard xterm 256-color extension. Slots the running program has
/// not customised are emitted as `RgbTriple::default()` (black); the
/// renderer is expected to apply its own static fallback for those.
///
/// `default_fg`, `default_bg`, `cursor`, `selection_bg`, and
/// `selection_fg` are resolved truecolor values for the well-known
/// theme slots so a downstream renderer never has to guess.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ThemeSnapshot {
    pub palette: Vec<RgbTriple>,
    pub default_fg: RgbTriple,
    pub default_bg: RgbTriple,
    pub cursor: RgbTriple,
    pub selection_bg: RgbTriple,
    pub selection_fg: RgbTriple,
}

impl Default for ThemeSnapshot {
    fn default() -> Self {
        Self {
            palette: vec![RgbTriple::default(); 256],
            default_fg: RgbTriple {
                r: 0xe6,
                g: 0xed,
                b: 0xf3,
            },
            default_bg: RgbTriple {
                r: 0x0b,
                g: 0x0d,
                b: 0x10,
            },
            cursor: RgbTriple {
                r: 0x58,
                g: 0xa6,
                b: 0xff,
            },
            selection_bg: RgbTriple {
                r: 0x26,
                g: 0x37,
                b: 0x52,
            },
            selection_fg: RgbTriple {
                r: 0xe6,
                g: 0xed,
                b: 0xf3,
            },
        }
    }
}

/// A hyperlink entry referenced by `CellSnapshot::hyperlink_id`.
/// Kept as a flat table on the snapshot so per-cell payload stays
/// small even when a 200-char URL is hovered across many cells.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct HyperlinkSnapshot {
    pub id: u32,
    pub uri: String,
    /// Optional OSC 8 `id=` parameter (groups cells of the same
    /// logical link). Empty string when not provided.
    pub group: String,
}

/// Everything a renderer needs to draw one frame.
///
/// `viewport[row][col]` indexes a `CellSnapshot` in row-major order.
/// `viewport.len() == rows`, every row has length `cols`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalSnapshot {
    pub cols: u16,
    pub rows: u16,
    pub viewport: Vec<Vec<CellSnapshot>>,
    pub display_offset: usize,
    pub scrollback_size: usize,
    pub cursor: CursorSnapshot,
    pub modes: ModesSnapshot,
    pub damage: DamageSnapshot,
    pub title: String,
    /// Hyperlink intern table. Empty when no cell carries a
    /// hyperlink. Ids in `CellSnapshot::hyperlink_id` index here by
    /// `HyperlinkSnapshot::id`.
    pub hyperlinks: Vec<HyperlinkSnapshot>,
    /// Active theme palette + well-known slots. Single source of
    /// truth for downstream renderers (native, web/wasm, remote).
    pub theme: ThemeSnapshot,
}

impl TerminalSnapshot {
    /// Build an empty snapshot of the given size, filled with default
    /// cells. Useful for tests and for the initial frame before
    /// Crosswords has produced anything.
    pub fn empty(cols: u16, rows: u16) -> Self {
        let row = vec![CellSnapshot::default(); cols as usize];
        let viewport = vec![row; rows as usize];
        Self {
            cols,
            rows,
            viewport,
            display_offset: 0,
            scrollback_size: 0,
            cursor: CursorSnapshot {
                col: 0,
                row: 0,
                shape: CursorShape::Block,
                visible: true,
            },
            modes: ModesSnapshot {
                alt_screen: false,
                origin: false,
                auto_wrap: true,
                bracketed_paste: false,
                focus_events: false,
                mouse_reporting: MouseReporting::Off,
            },
            damage: DamageSnapshot::default(),
            title: String::new(),
            hyperlinks: Vec::new(),
            theme: ThemeSnapshot::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time check: snapshot is thread-safe and cloneable so
    /// it can move across channels and (de)serialize on either side.
    fn assert_send_sync_clone<T: Send + Sync + Clone>() {}

    #[test]
    fn trait_bounds_hold() {
        assert_send_sync_clone::<TerminalSnapshot>();
        assert_send_sync_clone::<CellSnapshot>();
        assert_send_sync_clone::<CursorSnapshot>();
    }
}
