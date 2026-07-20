//! Editor-derived data shapes shared by chrome panels.
//!
//! Native panels under `frontends/neoism/src/chrome/panels/` used to
//! pull editor state straight out of `neoism_backend::performer::nvim`
//! types. Lifting those panels into this crate would otherwise pull
//! `neoism_backend` in and break the native + web split. Instead, the
//! host (native: nvim performer; web: daemon wire) builds these
//! snapshots once per frame and hands them to the panels.

use std::collections::BTreeMap;

// -----------------------------------------------------------------------
// Popup menu (LSP completion / nvim `pum_show`).
// -----------------------------------------------------------------------

/// Single completion candidate. Mirrors the four-field shape nvim
/// publishes through `pum_show`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenuItem {
    pub word: String,
    pub kind: String,
    pub menu: String,
    pub info: String,
}

/// Snapshot of the nvim popup menu at the current frame.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PopupMenu {
    pub items: Vec<PopupMenuItem>,
    /// Currently highlighted item, or `None` if browsing without a
    /// selection (matches `pum_select_item` with -1).
    pub selected: Option<usize>,
    pub anchor_row: u32,
    pub anchor_col: u32,
    /// Identifies the grid the popup is anchored to. The menu's
    /// animation signature keys off this so a popup appearing in a
    /// different split resets scroll state.
    pub grid: u64,
    /// Longest word width hint published by nvim's `pum_show`. The
    /// popup uses it to size itself so the word column doesn't truncate
    /// on the first frame before items render.
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
    /// Translate from nvim's `vim.diagnostic.severity` codes
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
    /// 1-based line number — matches nvim's `vim.diagnostic.get`
    /// representation so the popup can pass it straight to `:<lnum>`
    /// jumps.
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

/// Per-line git change marker on the minimap rail. Mirrors
/// `neoism_backend::performer::nvim::MinimapGitChange` field-for-field.
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

// -----------------------------------------------------------------------
// Editor grid snapshot (nvim ext_linegrid mirror).
// -----------------------------------------------------------------------

/// One painted grid cell. Mirrors `neoism_protocol::editor::GridCell`
/// field-for-field so the bridge can decode the wire shape without a
/// `From` round-trip dance.
///
/// `ch` is whatever grapheme nvim publishes (usually a single
/// character; `""` for the trailing half of a double-width glyph).
/// `fg`/`bg` are packed `0x00RRGGBB`. `attrs` is the nvim attribute
/// bitfield (bit 0 = bold, bit 1 = italic, bit 2 = underline, bit 3 =
/// undercurl, bit 4 = strikethrough, bit 5 = reverse). The renderer
/// only honours `bg != default_bg` for now; richer attribute decoding
/// lands in a follow-up wave.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct GridCell {
    pub ch: String,
    pub fg: u32,
    pub bg: u32,
    pub attrs: u8,
}

impl GridCell {
    pub fn default_colors(default_fg: u32, default_bg: u32) -> Self {
        Self {
            ch: String::new(),
            fg: default_fg,
            bg: default_bg,
            attrs: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GridScrollEdgeCapture {
    Captured,
    NoScroll,
    Invalid,
}

/// Running snapshot of the nvim grid the daemon proxies to us. The
/// bridge maintains this as the source of truth for the file-viewer
/// pane's paint when nvim has a buffer attached; on every
/// `EditorServerMessage::GridUpdate` the bridge merges the delta cells
/// into `cells` (laid out row-major, `len() == width * height`).
///
/// `cursor` is the post-batch caret position as `(row, col)`, or
/// `None` if nvim hasn't reported one yet.
///
/// `default_fg` / `default_bg` are the resolved-color fallbacks the
/// renderer uses to skip painting unnecessary background quads (cells
/// that match `default_bg` only get their glyph painted).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct EditorGridSnapshot {
    pub width: u32,
    pub height: u32,
    pub cells: Vec<GridCell>,
    pub cursor: Option<(u32, u32)>,
    pub default_fg: u32,
    pub default_bg: u32,
}

impl EditorGridSnapshot {
    pub fn default_cell(&self) -> GridCell {
        GridCell::default_colors(self.default_fg, self.default_bg)
    }

    pub fn normalize_cells(&mut self) {
        let total = (self.width as usize).saturating_mul(self.height as usize);
        if self.cells.len() != total {
            self.cells.resize(total, self.default_cell());
        }
    }

    /// Apply an nvim `grid_scroll` delta to this row-major snapshot.
    /// Positive `rows` shifts the region content up and clears the
    /// exposed bottom rows; negative `rows` shifts content down and
    /// clears the exposed top rows.
    pub fn apply_grid_scroll(
        &mut self,
        top: u32,
        bot: u32,
        left: u32,
        right: u32,
        rows: i32,
        _cols: i32,
    ) {
        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 {
            return;
        }

        self.normalize_cells();
        let default = self.default_cell();

        let top = (top as usize).min(height);
        let bot = (bot as usize).min(height);
        let left = (left as usize).min(width);
        let right = (right as usize).min(width);
        if top >= bot || left >= right || rows == 0 {
            return;
        }

        if rows > 0 {
            let shift = (rows as usize).min(bot - top);
            for row in top..(bot - shift) {
                for col in left..right {
                    let dst = row * width + col;
                    let src = (row + shift) * width + col;
                    self.cells[dst] = self.cells[src].clone();
                }
            }
            for row in (bot - shift)..bot {
                for col in left..right {
                    self.cells[row * width + col] = default.clone();
                }
            }
        } else {
            let shift = ((-rows) as usize).min(bot - top);
            for row in ((top + shift)..bot).rev() {
                for col in left..right {
                    let dst = row * width + col;
                    let src = (row - shift) * width + col;
                    self.cells[dst] = self.cells[src].clone();
                }
            }
            for row in top..(top + shift) {
                for col in left..right {
                    self.cells[row * width + col] = default.clone();
                }
            }
        }
    }

    /// Capture the rows a following `grid_scroll` will move off-screen.
    ///
    /// Hosts use this to keep a short scrollback edge cache for smooth
    /// retained nvim scrolling. The capture is intentionally full-width
    /// because the renderer's retained row source decisions operate on
    /// grid rows, even when nvim scrolls a narrower column range.
    pub fn capture_grid_scroll_edge_rows(
        &self,
        top: u32,
        bot: u32,
        rows: i32,
        max_edge_rows: usize,
        above_rows: &mut Vec<GridCell>,
        below_rows: &mut Vec<GridCell>,
    ) -> GridScrollEdgeCapture {
        if rows == 0 {
            return GridScrollEdgeCapture::NoScroll;
        }

        let width = self.width as usize;
        let height = self.height as usize;
        if width == 0 || height == 0 || self.cells.len() != width * height {
            return GridScrollEdgeCapture::Invalid;
        }

        let top = (top as usize).min(height);
        let bot = (bot as usize).min(height);
        if top >= bot {
            return GridScrollEdgeCapture::Invalid;
        }

        if rows > 0 {
            let shift = (rows as usize).min(bot - top);
            for row in top..(top + shift) {
                let start = row * width;
                above_rows.extend_from_slice(&self.cells[start..start + width]);
            }
            let max_cells = max_edge_rows.saturating_mul(width);
            if above_rows.len() > max_cells {
                let excess = above_rows.len() - max_cells;
                above_rows.drain(0..excess);
            }
            below_rows.clear();
        } else {
            let shift = ((-rows) as usize).min(bot - top);
            let mut captured = Vec::with_capacity(shift.saturating_mul(width));
            for row in (bot - shift)..bot {
                let start = row * width;
                captured.extend_from_slice(&self.cells[start..start + width]);
            }
            below_rows.splice(0..0, captured);
            let max_cells = max_edge_rows.saturating_mul(width);
            if below_rows.len() > max_cells {
                below_rows.truncate(max_cells);
            }
            above_rows.clear();
        }

        GridScrollEdgeCapture::Captured
    }
}

/// Per-editor-surface grid cache.
///
/// Legacy hosts still push redraws without a `surface_id`; those frames
/// continue to update the single `legacy` snapshot. Web hosts that bind
/// pane routes to editor surfaces can update independent snapshots by
/// surface id while the active snapshot accessor preserves the old
/// single-grid read path.
#[derive(Clone, Debug, Default)]
pub struct EditorGridSnapshotStore {
    legacy: Option<EditorGridSnapshot>,
    active_surface_id: Option<String>,
    surfaces: BTreeMap<String, EditorGridSnapshot>,
}

impl EditorGridSnapshotStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// Surface id carried by the most recent surface-scoped redraw.
    /// `None` means the active/legacy path was updated last.
    pub fn active_surface_id(&self) -> Option<&str> {
        self.active_surface_id.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.legacy.is_none() && self.surfaces.is_empty()
    }

    pub fn get(&self, surface_id: Option<&str>) -> Option<&EditorGridSnapshot> {
        match surface_id {
            Some(surface_id) => self.surfaces.get(surface_id),
            None => self.legacy.as_ref(),
        }
    }

    pub fn active(&self) -> Option<&EditorGridSnapshot> {
        self.active_surface_id
            .as_deref()
            .and_then(|surface_id| self.surfaces.get(surface_id))
            .or(self.legacy.as_ref())
    }

    pub fn set(&mut self, surface_id: Option<String>, snapshot: EditorGridSnapshot) {
        match surface_id {
            Some(surface_id) => {
                self.active_surface_id = Some(surface_id.clone());
                self.surfaces.insert(surface_id, snapshot);
            }
            None => {
                self.active_surface_id = None;
                self.legacy = Some(snapshot);
            }
        }
    }

    pub fn remove_surface(&mut self, surface_id: &str) -> Option<EditorGridSnapshot> {
        if self.active_surface_id.as_deref() == Some(surface_id) {
            self.active_surface_id = None;
        }
        self.surfaces.remove(surface_id)
    }

    pub fn surface_ids(&self) -> impl Iterator<Item = &str> {
        self.surfaces.keys().map(String::as_str)
    }

    pub fn surface_count(&self) -> usize {
        self.surfaces.len()
    }
}

// -----------------------------------------------------------------------
// Top-level snapshot.
// -----------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::{
        EditorGridSnapshot, EditorGridSnapshotStore, GridCell, GridScrollEdgeCapture,
    };

    fn snapshot(ch: &str) -> EditorGridSnapshot {
        EditorGridSnapshot {
            width: 1,
            height: 1,
            cells: vec![GridCell {
                ch: ch.to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            }],
            cursor: Some((0, 0)),
            default_fg: 1,
            default_bg: 2,
        }
    }

    fn grid(chars: &[&str], width: u32) -> EditorGridSnapshot {
        let cells = chars
            .iter()
            .map(|ch| GridCell {
                ch: ch.to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            })
            .collect::<Vec<_>>();
        EditorGridSnapshot {
            width,
            height: (chars.len() as u32) / width.max(1),
            cells,
            cursor: None,
            default_fg: 1,
            default_bg: 2,
        }
    }

    fn chars(snapshot: &EditorGridSnapshot) -> Vec<&str> {
        snapshot.cells.iter().map(|cell| cell.ch.as_str()).collect()
    }

    fn cell_chars(cells: &[GridCell]) -> Vec<&str> {
        cells.iter().map(|cell| cell.ch.as_str()).collect()
    }

    #[test]
    fn grid_scroll_positive_rows_shift_region_up() {
        let mut snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);

        snapshot.apply_grid_scroll(0, 3, 0, 2, 1, 0);

        assert_eq!(chars(&snapshot), vec!["c", "d", "e", "f", "", ""]);
    }

    #[test]
    fn grid_scroll_negative_rows_shift_region_down() {
        let mut snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);

        snapshot.apply_grid_scroll(0, 3, 0, 2, -1, 0);

        assert_eq!(chars(&snapshot), vec!["", "", "a", "b", "c", "d"]);
    }

    #[test]
    fn grid_scroll_only_mutates_requested_columns() {
        let mut snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);

        snapshot.apply_grid_scroll(0, 3, 1, 2, 1, 0);

        assert_eq!(chars(&snapshot), vec!["a", "d", "c", "f", "e", ""]);
    }

    #[test]
    fn grid_scroll_edge_capture_positive_rows_stashes_top_edge() {
        let snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);
        let mut above = Vec::new();
        let mut below = vec![GridCell::default()];

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 3, 1, 64, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Captured);
        assert_eq!(cell_chars(&above), vec!["a", "b"]);
        assert!(below.is_empty());
    }

    #[test]
    fn grid_scroll_edge_capture_negative_rows_stashes_bottom_edge() {
        let snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);
        let mut above = vec![GridCell::default()];
        let mut below = Vec::new();

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 3, -1, 64, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Captured);
        assert!(above.is_empty());
        assert_eq!(cell_chars(&below), vec!["e", "f"]);
    }

    #[test]
    fn grid_scroll_edge_capture_negative_rows_prepends_latest_bottom_edge() {
        let snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);
        let mut above = Vec::new();
        let mut below = vec![
            GridCell {
                ch: "x".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
            GridCell {
                ch: "y".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
        ];

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 3, -1, 64, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Captured);
        assert_eq!(cell_chars(&below), vec!["e", "f", "x", "y"]);
    }

    #[test]
    fn grid_scroll_edge_capture_invalid_dimensions_rejects_capture() {
        let snapshot = EditorGridSnapshot {
            width: 2,
            height: 2,
            cells: vec![GridCell::default()],
            cursor: None,
            default_fg: 1,
            default_bg: 2,
        };
        let mut above = Vec::new();
        let mut below = Vec::new();

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 2, 1, 64, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Invalid);
        assert!(above.is_empty());
        assert!(below.is_empty());
    }

    #[test]
    fn grid_scroll_edge_capture_no_rows_is_no_scroll() {
        let snapshot = grid(&["a", "b"], 2);
        let mut above = Vec::new();
        let mut below = Vec::new();

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 1, 0, 64, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::NoScroll);
    }

    #[test]
    fn grid_scroll_edge_capture_trims_to_row_cap() {
        let snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);
        let mut above = vec![
            GridCell {
                ch: "x".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
            GridCell {
                ch: "y".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
        ];
        let mut below = Vec::new();

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 3, 1, 1, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Captured);
        assert_eq!(cell_chars(&above), vec!["a", "b"]);
    }

    #[test]
    fn grid_scroll_edge_capture_negative_rows_trims_oldest_bottom_edge() {
        let snapshot = grid(&["a", "b", "c", "d", "e", "f"], 2);
        let mut above = Vec::new();
        let mut below = vec![
            GridCell {
                ch: "x".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
            GridCell {
                ch: "y".to_string(),
                fg: 1,
                bg: 2,
                attrs: 0,
            },
        ];

        let result =
            snapshot.capture_grid_scroll_edge_rows(0, 3, -1, 1, &mut above, &mut below);

        assert_eq!(result, GridScrollEdgeCapture::Captured);
        assert_eq!(cell_chars(&below), vec!["e", "f"]);
    }

    #[test]
    fn surface_snapshots_do_not_overwrite_legacy_snapshot() {
        let mut store = EditorGridSnapshotStore::new();
        store.set(None, snapshot("legacy"));
        store.set(Some("pane-a".to_string()), snapshot("a"));
        store.set(Some("pane-b".to_string()), snapshot("b"));

        assert_eq!(store.active_surface_id(), Some("pane-b"));
        assert_eq!(store.get(None).unwrap().cells[0].ch, "legacy");
        assert_eq!(store.get(Some("pane-a")).unwrap().cells[0].ch, "a");
        assert_eq!(store.active().unwrap().cells[0].ch, "b");
    }

    #[test]
    fn legacy_update_preserves_old_active_path() {
        let mut store = EditorGridSnapshotStore::new();
        store.set(Some("pane-a".to_string()), snapshot("a"));
        store.set(None, snapshot("legacy"));

        assert_eq!(store.active_surface_id(), None);
        assert_eq!(store.active().unwrap().cells[0].ch, "legacy");
        assert_eq!(store.get(Some("pane-a")).unwrap().cells[0].ch, "a");
    }

    #[test]
    fn removing_active_surface_falls_back_to_legacy() {
        let mut store = EditorGridSnapshotStore::new();
        store.set(None, snapshot("legacy"));
        store.set(Some("pane-a".to_string()), snapshot("a"));

        assert_eq!(store.remove_surface("pane-a").unwrap().cells[0].ch, "a");
        assert_eq!(store.active_surface_id(), None);
        assert_eq!(store.active().unwrap().cells[0].ch, "legacy");
        assert!(store.get(Some("pane-a")).is_none());
    }
}

#[derive(Clone, Debug, Default)]
pub struct EditorSnapshot {
    pub popup_menu: Option<PopupMenu>,
    pub diagnostics: Vec<DiagnosticItem>,
    pub minimap: Option<MinimapData>,
}
