//! Editor-derived data shapes shared by chrome panels.
//!
//! The host (native code-editor bridge; web: daemon wire) builds these
//! snapshots once per frame and hands them to the panels, keeping the
//! panels renderer-neutral without dragging host crates in.

// -----------------------------------------------------------------------
// Popup menu (LSP completion).
// -----------------------------------------------------------------------

/// Single completion candidate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenuItem {
    pub word: String,
    pub kind: String,
    pub menu: String,
    pub info: String,
}

/// Snapshot of the completion popup menu at the current frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenu {
    pub items: Vec<PopupMenuItem>,
    /// Currently highlighted item, or `None` if browsing without a
    /// selection.
    pub selected: Option<usize>,
    pub anchor_row: u32,
    pub anchor_col: u32,
    /// Identifies the grid the popup is anchored to. The menu's
    /// animation signature keys off this so a popup appearing in a
    /// different split resets scroll state.
    pub grid: u64,
    /// Longest word width hint. The popup uses it to size itself so
    /// the word column doesn't truncate on the first frame before
    /// items render.
    pub max_word_chars: usize,
}

impl PopupMenu {
    /// Resolve `selected` to a bounded index, returning `None` when no
    /// item is highlighted or the index is out of range.
    pub fn selected_index(&self) -> Option<usize> {
        let s = self.selected?;
        (s < self.items.len()).then_some(s)
    }
}

// -----------------------------------------------------------------------
// Diagnostics (LSP).
// -----------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticSeverity {
    Error,
    Warn,
    Info,
    Hint,
}

impl DiagnosticSeverity {
    /// Translate from LSP severity codes
    /// (1=error, 2=warn, 3=info, 4=hint).
    pub fn from_u8(s: u8) -> Self {
        match s {
            1 => DiagnosticSeverity::Error,
            2 => DiagnosticSeverity::Warn,
            3 => DiagnosticSeverity::Info,
            _ => DiagnosticSeverity::Hint,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticItem {
    pub severity: DiagnosticSeverity,
    pub message: String,
    pub source: Option<String>,
    /// 0-based line number (aligns with the rest of the snapshot model).
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    /// 1-based line number so the popup can pass it straight to
    /// `:<lnum>` jumps.
    pub lnum: u32,
    pub code: Option<String>,
    pub code_description: Option<String>,
    pub tags: Vec<String>,
    pub related_information: Vec<DiagnosticRelatedInformation>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticRelatedInformation {
    pub path: String,
    pub line: u32,
    pub col: u32,
    pub end_line: u32,
    pub end_col: u32,
    pub message: String,
}

// -----------------------------------------------------------------------
// Minimap.
// -----------------------------------------------------------------------

/// Minimap data the lifted `panels::minimap::Minimap` consumes per
/// snapshot push. Fields beyond the basic viewport scalars are kept so
/// the panel's render math ports verbatim.
#[derive(Clone, Debug, Default)]
pub struct MinimapData {
    /// Used in the "changed?" check so unrelated buffer updates don't
    /// bust the line-shape cache.
    pub path: Option<std::path::PathBuf>,
    /// Same dirty-key purpose as `path`.
    pub changedtick: u64,
    pub total_lines: u64,
    pub top_line: u64,
    pub bottom_line: u64,
    pub cursor_line: u64,
    /// `lines` is the sampled tail, not the whole buffer; stride lets
    /// the panel reconstruct true source-line indices.
    pub sample_stride: u64,
    /// Optional sampled lines the panel classifies via `classify_line`.
    pub lines: Option<Vec<String>>,
    /// Per-line gutter markers on the rail.
    pub git_changes: Vec<MinimapGitChange>,
}

/// Per-line git change marker on the minimap rail.
#[derive(Clone, Debug)]
pub struct MinimapGitChange {
    pub line: u64,
    pub kind: String,
}

impl MinimapData {
    /// Viewport height in lines, derived from `top_line`/`bottom_line`.
    pub fn viewport_height(&self) -> u64 {
        self.bottom_line
            .saturating_sub(self.top_line)
            .saturating_add(1)
    }

    pub fn viewport_top(&self) -> u64 {
        self.top_line
    }
}
