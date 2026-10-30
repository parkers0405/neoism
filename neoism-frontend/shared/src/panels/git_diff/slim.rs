//! Slim git diff panel.
//!
//! The shared crate hosts a **slim** diff viewer: a left-column file
//! list with `+X / -Y` summary stats, a right-column hunk view that
//! tints added/removed lines, and a host-driven refresh path. The
//! panel does not shell out to `git`, parse unified-diff text, or
//! query the filesystem — those concerns stay in the native shim
//! (`frontends/neoism/src/editor/git_diff_panel/io.rs`) where
//! `git2`/`std::process::Command`/`std::fs` live, or in the
//! sibling `parse.rs` (which only handles already-fetched diff text).
//!
//! ## Refresh model
//!
//! The host pushes structured data into [`GitDiff::set_files`]
//! whenever its native worker resolves a new diff. The panel can also
//! pull via [`GitDiff::refresh`], which calls
//! [`GitService::diff`](crate::services::GitService::diff). The slim
//! panel does not own a unified-diff parser, so on a successful sync
//! reply the host is responsible for handing back already-structured
//! [`DiffFile`]s — either by parsing in the native worker, or by
//! parsing on the daemon side and sending JSON over the wire. The
//! pending-request path is wired so that on web the panel re-enters
//! through [`UiEvent::ServiceReply`] with a deserialized payload.
//!
//! ## Coordinate model
//!
//! `layout.bounds` is the panel's window-space rect. The slim panel
//! paints a frame + inner card, then splits the inner area into the
//! files column on the left and the hunks column on the right.

use std::cell::Cell;
use std::path::Path;

use serde::{Deserialize, Serialize};
use crate::primitives::paint::Paint;

use crate::event::{LogicalKey, NamedKey, UiEvent};
use crate::layout::{PanelLayout, Rect};
use crate::panels::{Panel, PanelContext};
use crate::services::{IoError, RequestId};
use crate::theme::RgbTriple;

/// A single file in the changeset.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffFile {
    pub path: String,
    pub hunks: Vec<DiffHunk>,
    pub added: u32,
    pub removed: u32,
}

impl DiffFile {
    /// Convenience constructor that sums `added`/`removed` from
    /// `hunks` if the caller hasn't precomputed them.
    pub fn new(path: impl Into<String>, hunks: Vec<DiffHunk>) -> Self {
        let added: u32 = hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| matches!(l, DiffLine::Added(_)))
            .count() as u32;
        let removed: u32 = hunks
            .iter()
            .flat_map(|h| h.lines.iter())
            .filter(|l| matches!(l, DiffLine::Removed(_)))
            .count() as u32;
        Self {
            path: path.into(),
            hunks,
            added,
            removed,
        }
    }
}

/// One `@@ -A,B +C,D @@` block worth of changes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

/// Per-line classification inside a hunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffLine {
    Context(String),
    Added(String),
    Removed(String),
}

impl DiffLine {
    pub fn text(&self) -> &str {
        match self {
            DiffLine::Context(s) | DiffLine::Added(s) | DiffLine::Removed(s) => {
                s.as_str()
            }
        }
    }
}

const FRAME_RADIUS: f32 = 8.0;
const FRAME_INSET: f32 = 1.0;
const FILES_COLUMN_RATIO: f32 = 0.34;
const FILES_COLUMN_MIN_W: f32 = 160.0;
const FILES_COLUMN_MAX_W: f32 = 360.0;
const ROW_HEIGHT: f32 = 24.0;
const HUNK_LINE_HEIGHT: f32 = 18.0;
const PADDING: f32 = 8.0;
const SEPARATOR: f32 = 1.0;

const FRAME_DEPTH: f32 = 0.0;
const CARD_DEPTH: f32 = 0.05;
const ROW_DEPTH: f32 = 0.1;
const LINE_BG_DEPTH: f32 = 0.15;

const FRAME_ORDER: u8 = 18;
const CARD_ORDER: u8 = 19;
const ROW_ORDER: u8 = 20;
const LINE_BG_ORDER: u8 = 21;
const ACCENT_ORDER: u8 = 22;

/// Slim git-diff panel. See module docs.
pub struct GitDiff {
    visible: bool,
    files: Vec<DiffFile>,
    /// Index into `files`. Clamped on every `set_files`.
    selected_file: usize,
    /// Vertical scroll inside the hunks column, in logical pixels.
    scroll: f32,
    /// In-flight `GitService::diff` request id, if any. Set by
    /// [`GitDiff::refresh`] when the service replied `Pending`;
    /// cleared when [`UiEvent::ServiceReply`] lands.
    pending: Option<RequestId>,
    /// Last drawn layout bounds, used by `handle_event` for hit
    /// testing. `Cell` because `Panel::draw` takes `&self`.
    last_bounds: Cell<Option<Rect>>,
}

impl Default for GitDiff {
    fn default() -> Self {
        Self::new()
    }
}

impl GitDiff {
    pub fn new() -> Self {
        Self {
            visible: false,
            files: Vec::new(),
            selected_file: 0,
            scroll: 0.0,
            pending: None,
            last_bounds: Cell::new(None),
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn is_focused(&self) -> bool {
        self.visible
    }

    pub fn show(&mut self) {
        self.visible = true;
    }

    pub fn hide(&mut self) {
        self.visible = false;
        self.last_bounds.set(None);
    }

    /// Replace the file list. Clamps the selection and resets the
    /// hunk scroll so a freshly-loaded changeset always starts at
    /// the top of the first file's first hunk.
    pub fn set_files(&mut self, files: Vec<DiffFile>) {
        self.files = files;
        if self.selected_file >= self.files.len() {
            self.selected_file = self.files.len().saturating_sub(1);
        }
        self.scroll = 0.0;
    }

    pub fn files(&self) -> &[DiffFile] {
        &self.files
    }

    pub fn selected_index(&self) -> Option<usize> {
        if self.files.is_empty() {
            None
        } else {
            Some(self.selected_file)
        }
    }

    pub fn selected_file(&self) -> Option<&DiffFile> {
        self.files.get(self.selected_file)
    }

    pub fn selected_path(&self) -> Option<&str> {
        self.selected_file().map(|f| f.path.as_str())
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        None
    }

    pub fn is_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn pending_request(&self) -> Option<RequestId> {
        self.pending
    }

    /// Request a fresh diff from the host. The slim panel doesn't own
    /// a unified-diff parser, so a synchronous reply is discarded —
    /// the host is expected to parse and call [`set_files`] directly
    /// from its native worker. On the web/wasm path the service
    /// returns `Pending` and the panel records the request id; when
    /// the daemon replies as [`UiEvent::ServiceReply`] with a
    /// `Vec<DiffFile>` payload, the panel decodes it and updates.
    pub fn refresh(&mut self, ctx: &mut PanelContext) {
        match ctx.services.git.diff(Path::new("."), None) {
            Ok(_unified_text) => {
                // Native path: host pushes structured data via
                // `set_files` after parsing in its own worker. The
                // raw unified string is intentionally dropped here.
            }
            Err(IoError::Pending(req)) => {
                self.pending = Some(req);
            }
            Err(_) => {}
        }
    }

    fn move_selection(&mut self, delta: i32) {
        if self.files.is_empty() {
            return;
        }
        let max = (self.files.len() - 1) as i32;
        let cur = self.selected_file as i32;
        let next = (cur + delta).clamp(0, max);
        if next as usize != self.selected_file {
            self.selected_file = next as usize;
            // Reset hunk scroll on file change so we land at the top
            // of the new diff (matches the native panel's behaviour).
            self.scroll = 0.0;
        }
    }

    /// Compute the files-column rect inside `bounds`.
    fn files_column_rect(bounds: Rect) -> Rect {
        let inner = inner_rect(bounds);
        let w =
            (inner.w * FILES_COLUMN_RATIO).clamp(FILES_COLUMN_MIN_W, FILES_COLUMN_MAX_W);
        let w = w.min(inner.w * 0.6);
        Rect::new(inner.x, inner.y, w, inner.h)
    }

    /// Compute the hunks-column rect inside `bounds`.
    fn hunks_column_rect(bounds: Rect) -> Rect {
        let inner = inner_rect(bounds);
        let files = Self::files_column_rect(bounds);
        let x = files.x + files.w + SEPARATOR;
        let w = (inner.x + inner.w - x).max(0.0);
        Rect::new(x, inner.y, w, inner.h)
    }
}

/// Inner-card rect — `bounds` shrunk by `FRAME_INSET` on every edge.
fn inner_rect(bounds: Rect) -> Rect {
    Rect::new(
        bounds.x + FRAME_INSET,
        bounds.y + FRAME_INSET,
        (bounds.w - FRAME_INSET * 2.0).max(0.0),
        (bounds.h - FRAME_INSET * 2.0).max(0.0),
    )
}

fn rgba(c: RgbTriple, alpha: f32) -> [f32; 4] {
    [
        c.r as f32 / 255.0,
        c.g as f32 / 255.0,
        c.b as f32 / 255.0,
        alpha,
    ]
}

impl Panel for GitDiff {
    fn handle_event(&mut self, event: &UiEvent, _ctx: &mut PanelContext) {
        if !self.visible {
            // Still drain `ServiceReply` while hidden — a refresh
            // kicked off before close should not leak a stale
            // pending request id forever.
            if let UiEvent::ServiceReply { request_id, .. } = event {
                if Some(*request_id) == self.pending {
                    self.pending = None;
                }
            }
            return;
        }
        match event {
            UiEvent::Key(k) if k.state == crate::event::KeyState::Pressed => {
                match &k.logical {
                    LogicalKey::Named(NamedKey::Escape) => self.hide(),
                    LogicalKey::Named(NamedKey::ArrowDown) => self.move_selection(1),
                    LogicalKey::Named(NamedKey::ArrowUp) => self.move_selection(-1),
                    LogicalKey::Named(NamedKey::PageDown) => self.move_selection(8),
                    LogicalKey::Named(NamedKey::PageUp) => self.move_selection(-8),
                    LogicalKey::Named(NamedKey::Home) => {
                        self.move_selection(i32::MIN / 2)
                    }
                    LogicalKey::Named(NamedKey::End) => self.move_selection(i32::MAX / 2),
                    _ => {}
                }
            }
            UiEvent::Wheel { dy, .. } => {
                // Wheel events report pixel deltas with negative `dy`
                // meaning scroll down on most platforms; subtract so a
                // downward wheel lowers content (intuitive direction).
                self.scroll = (self.scroll - dy).max(0.0);
            }
            UiEvent::ServiceReply {
                request_id,
                payload,
            } if Some(*request_id) == self.pending => {
                if let Ok(files) =
                    serde_json::from_value::<Vec<DiffFile>>(payload.clone())
                {
                    self.set_files(files);
                }
                self.pending = None;
            }
            UiEvent::PointerDown {
                button: crate::event::PointerButton::Left,
                x,
                y,
                ..
            } => {
                if let Some(bounds) = self.last_bounds.get() {
                    let files_col = Self::files_column_rect(bounds);
                    if files_col.contains(*x, *y) {
                        // Map y to a row index inside the files column.
                        let row_y = *y - (files_col.y + PADDING);
                        if row_y >= 0.0 {
                            let idx = (row_y / ROW_HEIGHT) as usize;
                            if idx < self.files.len() {
                                if idx != self.selected_file {
                                    self.selected_file = idx;
                                    self.scroll = 0.0;
                                }
                            }
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn draw(&self, sugarloaf: &mut dyn Paint, layout: &PanelLayout, ctx: &PanelContext) {
        if !self.visible {
            return;
        }
        let bounds = layout.bounds;
        self.last_bounds.set(Some(bounds));
        let theme = ctx.theme;

        // Outer frame (border ring).
        sugarloaf.rounded_rect(
            None,
            bounds.x,
            bounds.y,
            bounds.w,
            bounds.h,
            rgba(theme.border, 1.0),
            FRAME_DEPTH,
            FRAME_RADIUS,
            FRAME_ORDER,
        );
        // Inner card.
        let inner = inner_rect(bounds);
        sugarloaf.rounded_rect(
            None,
            inner.x,
            inner.y,
            inner.w,
            inner.h,
            rgba(theme.bg_elevated, 1.0),
            CARD_DEPTH,
            FRAME_RADIUS - FRAME_INSET,
            CARD_ORDER,
        );

        if self.files.is_empty() {
            // Empty-state — paint nothing more; the inner card alone
            // reads as a "no changes" surface. The native panel still
            // overlays text on top via its own renderer when this
            // slim panel is wired into the host shell.
            return;
        }

        let files_col = Self::files_column_rect(bounds);
        let hunks_col = Self::hunks_column_rect(bounds);

        // Files-column rows.
        for (idx, file) in self.files.iter().enumerate() {
            let y = files_col.y + PADDING + idx as f32 * ROW_HEIGHT;
            if y + ROW_HEIGHT > files_col.y + files_col.h {
                break;
            }
            let is_sel = idx == self.selected_file;
            let bg = if is_sel {
                rgba(theme.accent, 0.20)
            } else {
                rgba(theme.bg_elevated, 1.0)
            };
            sugarloaf.rect(
                None,
                files_col.x + PADDING,
                y,
                files_col.w - PADDING * 2.0,
                ROW_HEIGHT,
                bg,
                ROW_DEPTH,
                ROW_ORDER,
            );
            if is_sel {
                // Leading accent stripe.
                sugarloaf.rect(
                    None,
                    files_col.x + PADDING,
                    y,
                    2.0,
                    ROW_HEIGHT,
                    rgba(theme.accent, 1.0),
                    ROW_DEPTH,
                    ACCENT_ORDER,
                );
            }
            // Indicator strip for added/removed counts on the trailing
            // edge — green segment on top of red so a row with both
            // shows a stacked pair.
            if file.added + file.removed > 0 {
                let bar_x = files_col.x + files_col.w - PADDING - 6.0;
                let total = (file.added + file.removed) as f32;
                let add_ratio = file.added as f32 / total;
                let add_h = ROW_HEIGHT * add_ratio;
                let rem_h = ROW_HEIGHT - add_h;
                if add_h > 0.0 {
                    sugarloaf.rect(
                        None,
                        bar_x,
                        y,
                        4.0,
                        add_h,
                        rgba(theme.success, 0.85),
                        ROW_DEPTH,
                        ACCENT_ORDER,
                    );
                }
                if rem_h > 0.0 {
                    sugarloaf.rect(
                        None,
                        bar_x,
                        y + add_h,
                        4.0,
                        rem_h,
                        rgba(theme.error, 0.85),
                        ROW_DEPTH,
                        ACCENT_ORDER,
                    );
                }
            }
        }

        // Column separator.
        sugarloaf.rect(
            None,
            files_col.x + files_col.w,
            inner.y,
            SEPARATOR,
            inner.h,
            rgba(theme.border, 1.0),
            ROW_DEPTH,
            ROW_ORDER,
        );

        // Hunk lines for the selected file.
        let Some(selected) = self.files.get(self.selected_file) else {
            return;
        };
        let mut cursor_y = hunks_col.y + PADDING - self.scroll;
        let body_bottom = hunks_col.y + hunks_col.h;
        for hunk in &selected.hunks {
            for line in &hunk.lines {
                if cursor_y + HUNK_LINE_HEIGHT < hunks_col.y {
                    cursor_y += HUNK_LINE_HEIGHT;
                    continue;
                }
                if cursor_y > body_bottom {
                    return;
                }
                let bg = match line {
                    DiffLine::Added(_) => Some(rgba(theme.success, 0.18)),
                    DiffLine::Removed(_) => Some(rgba(theme.error, 0.18)),
                    DiffLine::Context(_) => None,
                };
                if let Some(c) = bg {
                    sugarloaf.rect(
                        None,
                        hunks_col.x + PADDING,
                        cursor_y,
                        hunks_col.w - PADDING * 2.0,
                        HUNK_LINE_HEIGHT,
                        c,
                        LINE_BG_DEPTH,
                        LINE_BG_ORDER,
                    );
                }
                cursor_y += HUNK_LINE_HEIGHT;
            }
        }
    }

    fn wants_focus(&self) -> bool {
        self.visible
    }

    fn name(&self) -> &str {
        "git_diff"
    }
}
