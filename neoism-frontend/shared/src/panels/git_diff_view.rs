//! Repository-wide Git review, independent of the sidebar's selection/lifetime.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[path = "git_diff_view_blocks.rs"]
mod blocks;
#[path = "git_diff_view_focus.rs"]
mod column_focus;
#[path = "git_diff_view_toolbar.rs"]
mod toolbar;
#[path = "git_diff_view_live.rs"]
mod live;

use sugarloaf::{text::DrawOpts, Sugarloaf};

#[cfg(test)]
use crate::editor::code::gitdiff::build_review_diff;
use crate::editor::code::projected::render_projected_code;
use crate::editor::code::{CodePane, ProjectedCodeRow, ProjectedCodeSpec};
use crate::layout::Rect;
use crate::panels::git_diff::{FileChange, FileStatus};
use crate::primitives::{draw_text_with_occlusion, IdeTheme};
use crate::syntax::Lang;
use crate::widgets::diff_card::{self, CardSpec, DiffLine, DiffLineKind, PaintParams};

struct FileSection {
    change: FileChange,
    lines: Option<Vec<DiffLine>>,
    left: Vec<DiffLine>,
    right: Vec<DiffLine>,
    split_origins: Vec<usize>,
    runs: Vec<blocks::ChangeRun>,
    collapsed: bool,
    wrapped_offsets: Vec<usize>,
    wrap_width: f32,
    wrap_scale: f32,
    top: f32,
    body_height: f32,
    max_line_number: u32,
    content_widths: [f32; 2],
    metrics_scale: f32,
    base_text: Option<std::sync::Arc<str>>,
    base_revision: u64,
    live_owned: bool,
    live_revision: Option<u64>,
}

impl FileSection {
    fn new(change: FileChange) -> Self {
        Self {
            change,
            lines: None,
            left: Vec::new(),
            right: Vec::new(),
            split_origins: Vec::new(),
            runs: Vec::new(),
            collapsed: false,
            wrapped_offsets: Vec::new(),
            wrap_width: 0.0,
            wrap_scale: 0.0,
            top: 0.0,
            body_height: 0.0,
            max_line_number: 1,
            content_widths: [0.0; 2],
            metrics_scale: 0.0,
            base_text: None,
            base_revision: 0,
            live_owned: false,
            live_revision: None,
        }
    }

    fn set_lines(&mut self, mut lines: Vec<DiffLine>) {
        // EOF markers carry no source text or layout row. Removing them before
        // alignment also lets an unterminated remove/add pair stay adjacent.
        lines.retain(|line| !line.text.starts_with("\\ No newline at end of file"));
        if self
            .lines
            .as_ref()
            .is_some_and(|old| same_lines(old, &lines))
        {
            return;
        }
        self.max_line_number = lines
            .iter()
            .flat_map(|line| {
                [line.line_number, line.old_line_number, line.new_line_number]
            })
            .flatten()
            .max()
            .unwrap_or(1);
        self.metrics_scale = 0.0;
        self.content_widths = [0.0; 2];
        self.runs = blocks::change_runs(&lines);
        (self.left, self.right, self.split_origins) = split_lines(&lines);
        self.wrapped_offsets.clear();
        self.lines = Some(lines);
    }

    fn measure_body(
        &mut self,
        width: f32,
        scale: f32,
        split: bool,
        gutter_width: f32,
    ) -> f32 {
        let width = diff_card::body_text_width_with_gutter(width, scale, gutter_width);
        if self.collapsed {
            return 0.0;
        }
        let Some(lines) = &self.lines else {
            return 38.0 * scale;
        };
        if lines.is_empty() {
            return 38.0 * scale;
        }
        if !split
            && (self.wrapped_offsets.is_empty()
                || self.wrap_width != width
                || self.wrap_scale != scale)
        {
            self.wrapped_offsets = diff_card::visual_row_offsets(lines, width, scale);
            self.wrap_width = width;
            self.wrap_scale = scale;
        }
        let rows = if split {
            self.left.len()
        } else {
            self.wrapped_offsets.last().copied().unwrap_or(0)
        };
        (rows as f32 * diff_card::LINE_HEIGHT
            + diff_card::BODY_TOP_PAD
            + diff_card::BODY_BOTTOM_PAD)
            * scale
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitDiffEditRequest {
    pub root: PathBuf,
    pub path: PathBuf,
    pub line: usize,
    pub column: usize,
}

struct SourceHit {
    path: String,
    line: usize,
    rect: Rect,
}

struct EditHit {
    path: PathBuf,
    line: usize,
    byte_start: usize,
    byte_end: usize,
    text: String,
    text_x: f32,
    scroll_x: f32,
    cell_w: f32,
    rect: Rect,
}

/// Root-scoped intents; Restore must be confirmed by the host before mutation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitDiffViewAction {
    /// Open a normal source tab at a zero-based working-tree location.
    OpenSource {
        root: PathBuf,
        path: String,
        line: usize,
        column: usize,
    },
    ChangeBlock {
        root: PathBuf,
        path: String,
        block: neoism_protocol::git::GitChangeBlock,
        restore: bool,
    },
    Stage {
        root: PathBuf,
        path: String,
        unstage: bool,
    },
    Restore {
        root: PathBuf,
        path: String,
    },
    StageAll {
        root: PathBuf,
    },
    /// Open/focus the repository's commit editor; never submit a commit.
    Commit {
        root: PathBuf,
    },
}

pub struct GitDiffView {
    /// Historical tabs never share working-tree identity or mutation controls.
    pub commit_id: Option<String>,
    pub commit_metadata: Option<crate::panels::git_commit_view::CommitMetadata>,
    pub root: PathBuf,
    /// The file selected by the most recent sidebar activation.
    pub path: PathBuf,
    sections: Vec<FileSection>,
    indices: HashMap<PathBuf, usize>,
    error: Option<String>,
    scroll_top: f32,
    horizontal: [f32; 2],
    max_horizontal: [f32; 2],
    active_column: usize,
    column_focus: Option<usize>,
    host_focused: bool,
    effective_split: bool,
    reveal_edit_on_focus: bool,
    horizontal_tracks: [Rect; 2],
    horizontal_thumbs: [Rect; 2],
    column_rects: [Rect; 2],
    horizontal_drag: Option<(usize, f32)>,
    vertical_drag: Option<f32>,
    vertical_thumb: Rect,
    gutter_width: f32,
    max_scroll: f32,
    page_height: f32,
    line_height: f32,
    split: bool,
    toolbar_hits: Vec<toolbar::Hit>,
    scrollbar_rect: Rect,
    header_rects: Vec<(usize, Rect)>,
    header_actions: Vec<(usize, bool, Rect, bool)>,
    header_pointer: Option<(f32, f32)>,
    actions_loading: bool,
    pending_actions: Vec<GitDiffViewAction>,
    pending_reveal: Option<PathBuf>,
    text_occlusions: Vec<[f32; 4]>,
    edit_path: Option<PathBuf>,
    edit_focused: bool,
    edit_request: Option<GitDiffEditRequest>,
    edit_hits: Vec<EditHit>,
    source_hits: Vec<SourceHit>,
    block_controls: Vec<blocks::BlockControl>,
    block_hover: Option<usize>,
    base_revision: u64,
    live_diff: live::LiveDiff,
    live_diff_pending: bool,
}

impl GitDiffView {
    pub fn from_commit(
        root: PathBuf,
        detail: neoism_protocol::git::GitCommitDetail,
    ) -> Self {
        use neoism_protocol::git::GitChangeStatus as S;
        let mut view = Self::new(root, PathBuf::new());
        view.commit_id = Some(detail.sha.clone());
        view.set_files(
            detail
                .files
                .iter()
                .map(|file| FileChange {
                    path: file.path.clone(),
                    additions: file.additions,
                    deletions: file.deletions,
                    staged: false,
                    status: match file.status {
                        S::Added => FileStatus::Added,
                        S::Deleted => FileStatus::Deleted,
                        S::Renamed => FileStatus::Renamed,
                        _ => FileStatus::Modified,
                    },
                })
                .collect(),
        );
        for diff in &detail.diffs {
            let mut lines = Vec::new();
            crate::panels::git_diff::parse_diff_into_with_headers(
                diff.patch.as_bytes(),
                &mut lines,
            );
            view.set_file_lines(Path::new(&diff.path), lines);
        }
        view.pending_reveal = None;
        view.commit_metadata =
            Some(crate::panels::git_commit_view::CommitMetadata::new(detail));
        view
    }
    pub fn take_commit_copy(&mut self) -> Option<String> {
        self.commit_metadata
            .as_mut()
            .and_then(|metadata| metadata.take_copy())
    }

    pub fn new(root: PathBuf, path: PathBuf) -> Self {
        let path = relative_path(&root, &path);
        let mut view = Self {
            root,
            commit_id: None,
            commit_metadata: None,
            path: path.clone(),
            sections: Vec::new(),
            indices: HashMap::new(),
            error: None,
            scroll_top: 0.0,
            horizontal: [0.0; 2],
            max_horizontal: [0.0; 2],
            active_column: 0,
            column_focus: Some(1),
            host_focused: true,
            effective_split: true,
            reveal_edit_on_focus: false,
            horizontal_tracks: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            horizontal_thumbs: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            column_rects: [Rect::new(0.0, 0.0, 0.0, 0.0); 2],
            horizontal_drag: None,
            vertical_drag: None,
            vertical_thumb: Rect::new(0.0, 0.0, 0.0, 0.0),
            gutter_width: diff_card::GUTTER_WIDTH,
            max_scroll: 0.0,
            page_height: 400.0,
            line_height: diff_card::LINE_HEIGHT,
            split: true,
            toolbar_hits: Vec::new(),
            scrollbar_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            header_rects: Vec::new(),
            header_actions: Vec::new(),
            header_pointer: None,
            actions_loading: false,
            pending_actions: Vec::new(),
            pending_reveal: Some(path.clone()),
            text_occlusions: Vec::new(),
            edit_path: None,
            edit_focused: false,
            edit_request: None,
            edit_hits: Vec::new(),
            source_hits: Vec::new(),
            block_controls: Vec::new(),
            block_hover: None,
            base_revision: 0,
            live_diff: live::LiveDiff::default(),
            live_diff_pending: false,
        };
        view.set_files(vec![FileChange {
            path: path.to_string_lossy().into_owned(),
            status: FileStatus::Modified,
            additions: 0,
            deletions: 0,
            staged: false,
        }]);
        view
    }

    /// Reconcile a repository snapshot without resetting folds or loaded patches.
    pub fn set_files(&mut self, files: Vec<FileChange>) {
        if self.sections.len() == files.len()
            && self.sections.iter().zip(&files).all(|(old, new)| {
                old.change.path == new.path
                    && old.change.status == new.status
                    && old.change.additions == new.additions
                    && old.change.deletions == new.deletions
                    && old.change.staged == new.staged
            })
        {
            return;
        }
        let mut old: HashMap<_, _> = self
            .sections
            .drain(..)
            .map(|section| (section.change.path.clone(), section))
            .collect();
        self.sections = files
            .into_iter()
            .map(|change| {
                if let Some(mut section) = old.remove(&change.path) {
                    section.change = change;
                    section
                } else {
                    FileSection::new(change)
                }
            })
            .collect();
        if let Some(path) = self.edit_path.as_ref() {
            if !self
                .sections
                .iter()
                .any(|section| Path::new(&section.change.path) == path)
            {
                if let Some(section) = old.remove(path.to_string_lossy().as_ref()) {
                    self.sections.push(section);
                }
            }
        }
        self.indices = self
            .sections
            .iter()
            .enumerate()
            .map(|(index, section)| (PathBuf::from(&section.change.path), index))
            .collect();
        self.header_rects.clear();
        self.header_actions.clear();
        self.block_controls.clear();
        self.block_hover = None;
        self.toolbar_hits.clear();
    }

    pub fn set_file_lines(&mut self, path: &Path, lines: Vec<DiffLine>) {
        let path = relative_path(&self.root, path);
        if let Some(&index) = self.indices.get(&path) {
            if self.sections[index].live_owned {
                return;
            }
            self.sections[index].set_lines(lines);
            self.error = None;
        }
    }

    pub fn set_lines(&mut self, lines: Vec<DiffLine>) {
        self.set_file_lines(&self.path.clone(), lines);
    }

    pub fn reveal_file(&mut self, path: &Path) {
        let path = relative_path(&self.root, path);
        if let Some(&index) = self.indices.get(&path) {
            self.sections[index].collapsed = false;
        }
        self.path = path.clone();
        self.pending_reveal = Some(path);
    }

    pub fn set_text_occlusions(&mut self, rects: &[[f32; 4]]) {
        self.text_occlusions.clear();
        self.text_occlusions.extend_from_slice(rects);
    }

    pub fn set_actions_loading(&mut self, loading: bool) {
        self.actions_loading = loading;
    }

    pub fn take_actions(&mut self) -> Vec<GitDiffViewAction> {
        if self.commit_id.is_some() {
            self.pending_actions.clear();
            return Vec::new();
        }
        std::mem::take(&mut self.pending_actions)
    }

    pub fn set_edit_focus(&mut self, focused: bool) {
        self.edit_focused = focused && self.commit_id.is_none();
        if self.edit_focused {
            self.column_focus = Some(1);
        } else {
            self.blur_columns();
        }
    }

    pub fn editing_path(&self) -> Option<PathBuf> {
        if self.edit_focused {
            self.document_path()
        } else {
            None
        }
    }

    /// The canonical source stays attached for painting after keyboard focus
    /// moves to a header, so unsaved edits never fall back to a disk patch.
    pub fn document_path(&self) -> Option<PathBuf> {
        if self.commit_id.is_some() {
            return None;
        }
        let path = self.edit_path.as_ref()?;
        Some(
            neoism_protocol::host_path::HostPath::new(self.root.to_string_lossy())
                .join(&path.to_string_lossy())
                .as_str()
                .into(),
        )
    }

    pub fn take_edit_request(&mut self) -> Option<GitDiffEditRequest> {
        if self.commit_id.is_some() {
            self.edit_request = None;
            return None;
        }
        self.edit_request.take()
    }

    pub fn set_base_text(&mut self, path: &Path, text: std::sync::Arc<str>) {
        let path = relative_path(&self.root, path);
        self.error = None;
        if let Some(&index) = self.indices.get(&path) {
            let section = &mut self.sections[index];
            if section.base_text.as_ref().is_some_and(|old| {
                std::sync::Arc::ptr_eq(old, &text)
            }) {
                return;
            }
            let changed = section.base_text.as_deref() != Some(text.as_ref());
            section.base_text = Some(text);
            if changed {
                self.base_revision = self.base_revision.wrapping_add(1);
                section.base_revision = self.base_revision;
                section.live_revision = None;
            }
        }
    }

    pub fn needs_redraw(&self) -> bool {
        self.live_diff_pending
    }

    fn apply_live_diff(&mut self, output: live::Output, pane: &CodePane) {
        let path = relative_path(&self.root, &pane.path);
        let Some(&index) = self.indices.get(&path) else {
            return;
        };
        let section = &mut self.sections[index];
        if output.key.path != path
            || output.key.base_revision != section.base_revision
            || output.key.revision != pane.buffer.revision
        {
            return;
        }
        section.change.additions = output.lines.iter()
            .filter(|line| line.kind == DiffLineKind::Add).count() as u32;
        section.change.deletions = output.lines.iter()
            .filter(|line| line.kind == DiffLineKind::Remove).count() as u32;
        section.set_lines(output.lines);
        section.live_revision = Some(output.key.revision);
    }

    fn sync_live_editor(&mut self, pane: &mut CodePane) {
        self.live_diff_pending = false;
        if pane.remote_content_pending || pane.error.is_some() {
            return;
        }
        let path = relative_path(&self.root, &pane.path);
        let Some(&index) = self.indices.get(&path) else {
            return;
        };
        if self.sections[index].base_text.is_none() {
            return;
        }
        self.sections[index].live_owned = true;
        match self.live_diff.poll() {
            Ok(Some(output)) => self.apply_live_diff(output, pane),
            Ok(None) => {}
            Err(error) => {
                self.set_error(error);
                return;
            }
        }
        let section = &self.sections[index];
        let caret_visible = section.lines.as_ref().is_some_and(|lines| {
            lines.iter().any(|line| {
                line.new_line_number == Some(pane.buffer.cursor_line as u32 + 1)
            })
        });
        let current = section.live_revision == Some(pane.buffer.revision) && caret_visible;
        if !current && !self.live_diff.pending() {
            let key = live::Key {
                path,
                base_revision: section.base_revision,
                revision: pane.buffer.revision,
                caret: pane.buffer.cursor_line,
            };
            // Only snapshot when the worker is available. Edits made while it
            // runs coalesce into the latest revision, not a queue of full files.
            if let Err(error) = self.live_diff.start(
                key, section.base_text.as_ref().unwrap().clone(), pane.buffer.text(),
            ) {
                self.set_error(error);
                return;
            }
        }
        self.live_diff_pending = !current;
    }

    pub fn set_error(&mut self, error: String) {
        self.error = Some(error);
        self.edit_focused = false;
    }

    /// Positive deltas move down, in logical pixels.
    pub fn scroll(&mut self, delta: f32) {
        if self
            .commit_metadata
            .as_mut()
            .is_some_and(|metadata| metadata.scroll_at(self.header_pointer, delta))
        {
            return;
        }
        if delta.is_finite() && delta != 0.0 {
            self.pending_reveal = None;
            self.scroll_top = (self.scroll_top + delta).clamp(0.0, self.max_scroll);
        }
    }

    pub fn scroll_horizontal(&mut self, delta: f32) {
        if delta.is_finite() {
            let column = self.active_column;
            self.horizontal[column] =
                (self.horizontal[column] + delta).clamp(0.0, self.max_horizontal[column]);
        }
    }

    pub fn scroll_horizontal_at(&mut self, x: f32, y: f32, delta: f32) {
        self.select_column(x, y);
        self.scroll_horizontal(delta);
    }

    fn select_column(&mut self, x: f32, y: f32) {
        if let Some(column) = self
            .column_rects
            .iter()
            .position(|rect| rect.contains(x, y))
        {
            self.active_column = column;
        }
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        self.select_column(x, y);
        if let Some(grab) = self.vertical_drag {
            let travel = (self.scrollbar_rect.h - self.vertical_thumb.h).max(0.0);
            if travel > 0.0 {
                self.pending_reveal = None;
                self.scroll_top = ((y - self.scrollbar_rect.y - grab) / travel)
                    .clamp(0.0, 1.0)
                    * self.max_scroll;
            }
            return true;
        }
        if let Some((column, grab)) = self.horizontal_drag {
            let track = self.horizontal_tracks[column];
            let travel = (track.w - self.horizontal_thumbs[column].w).max(0.0);
            if travel > 0.0 {
                self.horizontal[column] = ((x - track.x - grab) / travel).clamp(0.0, 1.0)
                    * self.max_horizontal[column];
            }
            return true;
        }
        let old_block_button = self
            .header_pointer
            .and_then(|(px, py)| self.block_button_at(px, py));
        let old_number_hover = self
            .header_pointer
            .and_then(|(px, py)| self.source_hit_index(px, py));
        let number_hover = self.source_hit_index(x, y);
        let old_hover = self.header_pointer.and_then(|(px, py)| {
            self.header_rects
                .iter()
                .find(|(_, r)| r.contains(px, py))
                .map(|(i, _)| *i)
        });
        self.header_pointer = Some((x, y));
        let hover = (!self.occluded(x, y))
            .then(|| {
                self.header_rects
                    .iter()
                    .find(|(_, r)| r.contains(x, y))
                    .map(|(i, _)| *i)
            })
            .flatten();
        let block_hover = self.block_hover_at(x, y);
        let changed = self.block_hover != block_hover;
        self.block_hover = block_hover;
        changed
            || old_block_button != self.block_button_at(x, y)
            || old_hover != hover
            || old_number_hover != number_hover
    }

    pub fn pointer_leave(&mut self) -> bool {
        let block = self.block_hover.take().is_some();
        for c in &mut self.block_controls {
            c.shown = false;
        }
        self.header_pointer.take().is_some() || block
    }

    pub fn is_dragging(&self) -> bool {
        self.horizontal_drag.is_some() || self.vertical_drag.is_some()
    }

    pub fn pointer_up(&mut self) -> bool {
        let horizontal = self.horizontal_drag.take().is_some();
        self.vertical_drag.take().is_some() || horizontal
    }

    pub fn key(&mut self, key: &str) -> bool {
        match key {
            "ArrowDown" | "Down" | "j" => self.scroll(self.line_height * 3.0),
            "ArrowUp" | "Up" | "k" => self.scroll(-self.line_height * 3.0),
            "PageDown" | " " => self.scroll(self.page_height * 0.9),
            "PageUp" => self.scroll(-self.page_height * 0.9),
            "Home" | "g" => {
                self.pending_reveal = None;
                self.scroll_top = 0.0;
            }
            "End" | "G" => {
                self.pending_reveal = None;
                self.scroll_top = self.max_scroll;
            }
            "ArrowRight" | "Right" | "l" => self.scroll_horizontal(60.0),
            "ArrowLeft" | "Left" | "h" => self.scroll_horizontal(-60.0),
            "Tab" | "s" => {
                self.split = !self.split;
                self.horizontal = [0.0; 2];
                self.horizontal_drag = None;
                self.pending_reveal = Some(self.path.clone());
            }
            "Enter" => {
                if let Some(&index) = self.indices.get(&self.path) {
                    self.toggle_section(index);
                }
            }
            "[" | "]" => self.navigate_file(key == "]"),
            _ => return false,
        }
        true
    }

    fn toggle_section(&mut self, index: usize) {
        let section = &mut self.sections[index];
        section.collapsed = !section.collapsed;
        if section.top < self.scroll_top {
            self.scroll_top = section.top;
        }
        self.path = PathBuf::from(&section.change.path);
        self.pending_reveal = None;
    }

    fn occluded(&self, x: f32, y: f32) -> bool {
        self.text_occlusions
            .iter()
            .any(|r| x >= r[0] && y >= r[1] && x < r[0] + r[2] && y < r[1] + r[3])
    }

    fn source_hit_index(&self, x: f32, y: f32) -> Option<usize> {
        if self.commit_id.is_some() || self.occluded(x, y) {
            return None;
        }
        self.source_hits
            .iter()
            .position(|hit| hit.rect.contains(x, y))
    }

    pub fn pointer_cursor(&self, x: f32, y: f32) -> bool {
        if self.occluded(x, y) {
            return false;
        }
        if self
            .commit_metadata
            .as_ref()
            .is_some_and(|meta| meta.pointer(x, y))
        {
            return true;
        }
        if self.block_panel_at(x, y).is_some() {
            return !self.actions_loading && self.block_button_at(x, y).is_some();
        }
        !self.occluded(x, y)
            && (self.source_hit_index(x, y).is_some()
                || self
                    .toolbar_hits
                    .iter()
                    .any(|hit| hit.enabled && hit.rect.contains(x, y))
                || self
                    .header_rects
                    .iter()
                    .any(|(_, rect)| rect.contains(x, y))
                || self
                    .horizontal_tracks
                    .iter()
                    .any(|rect| rect.contains(x, y))
                || self.scrollbar_rect.contains(x, y))
    }

    pub fn text_cursor(&self, x: f32, y: f32) -> bool {
        if self.commit_id.is_some() {
            return false;
        }
        !self.occluded(x, y)
            && self.block_panel_at(x, y).is_none()
            && self.source_hit_index(x, y).is_none()
            && self.edit_hits.iter().any(|hit| hit.rect.contains(x, y))
    }

    pub fn pointer_down(&mut self, x: f32, y: f32) -> bool {
        if self.occluded(x, y) {
            return false;
        }
        // A fresh pointer gesture supersedes an asynchronous file-header reveal.
        self.pending_reveal = None;
        if self.toolbar_pointer_down(x, y) {
            return true;
        }
        if self.block_pointer_down(x, y) {
            return true;
        }
        if let Some(index) = self.source_hit_index(x, y) {
            let hit = &self.source_hits[index];
            self.path = PathBuf::from(&hit.path);
            self.pending_actions.push(GitDiffViewAction::OpenSource {
                root: self.root.clone(),
                path: hit.path.clone(),
                line: hit.line,
                column: 0,
            });
            self.edit_request = None;
            self.blur_columns();
            return true;
        }
        if !self.text_cursor(x, y)
            && !self.scrollbar_rect.contains(x, y)
            && !self
                .horizontal_tracks
                .iter()
                .any(|rect| rect.contains(x, y))
        {
            self.column_focus = None;
            self.edit_focused = false;
        }
        self.select_column(x, y);
        if let Some(column) = self
            .horizontal_tracks
            .iter()
            .position(|rect| rect.contains(x, y))
        {
            let thumb = self.horizontal_thumbs[column];
            let grab = if thumb.contains(x, y) {
                x - thumb.x
            } else {
                thumb.w / 2.0
            };
            self.horizontal_drag = Some((column, grab));
            self.column_focus = Some(column);
            self.edit_focused = false;
            self.pointer_move(x, y);
            return true;
        }
        if let Some(&(index, restore, _, enabled)) = self
            .header_actions
            .iter()
            .rev()
            .find(|(_, _, rect, _)| rect.contains(x, y))
        {
            if enabled && !self.actions_loading {
                let change = &self.sections[index].change;
                let path = change.path.clone();
                let root = self.root.clone();
                self.pending_actions.push(if restore {
                    GitDiffViewAction::Restore { root, path }
                } else {
                    GitDiffViewAction::Stage {
                        root,
                        path,
                        unstage: change.staged && change.status != FileStatus::Mixed,
                    }
                });
            }
            return true; // Disabled controls still own clicks; never fold the file.
        }
        if let Some((index, _)) = self
            .header_rects
            .iter()
            .find(|(_, rect)| rect.contains(x, y))
        {
            self.toggle_section(*index);
            return true;
        }
        if self.scrollbar_rect.contains(x, y) && self.scrollbar_rect.h > 0.0 {
            self.pending_reveal = None;
            let grab = if self.vertical_thumb.contains(x, y) {
                y - self.vertical_thumb.y
            } else {
                self.vertical_thumb.h / 2.0
            };
            self.vertical_drag = Some(grab);
            self.pointer_move(x, y);
            return true;
        }
        if let Some(hit) = self
            .edit_hits
            .iter()
            .find(|hit| self.commit_id.is_none() && hit.rect.contains(x, y))
        {
            let fragment = hit.text.get(hit.byte_start..hit.byte_end).unwrap_or("");
            let cells = ((x - hit.text_x + hit.scroll_x) / hit.cell_w)
                .max(0.0)
                .floor() as usize;
            let column = hit.byte_start
                + crate::editor::code::layout::byte_for_display_col(
                    fragment,
                    cells,
                    crate::editor::code::layout::TAB_DISPLAY_WIDTH,
                );
            self.edit_request = Some(GitDiffEditRequest {
                root: self.root.clone(),
                path: hit.path.clone(),
                line: hit.line,
                column: column.min(hit.byte_end),
            });
            self.reveal_edit_on_focus = false;
            self.edit_path = Some(hit.path.clone());
            self.path = hit.path.clone();
            self.edit_focused = true;
            self.column_focus = Some(1);
            return true;
        }
        if let Some(column) = self
            .column_rects
            .iter()
            .position(|rect| rect.contains(x, y))
        {
            self.column_focus = Some(column);
            self.active_column = column;
            self.edit_focused = false;
            return true;
        }
        false
    }

    fn measure_sections(&mut self, width: f32, scale: f32, viewport_height: f32) {
        let split = self.split && width >= 520.0 * scale;
        let mut top = 0.0;
        for section in &mut self.sections {
            section.top = top;
            section.body_height =
                section.measure_body(width, scale, split, self.gutter_width);
            top += 32.0 * scale + section.body_height + 8.0 * scale;
        }
        self.max_scroll = (top - viewport_height).max(0.0);
        if let Some(path) = self.pending_reveal.as_ref() {
            if let Some(&index) = self.indices.get(path) {
                self.scroll_top = self.sections[index].top.min(self.max_scroll);
                // Earlier files may still expand from loading placeholders. Keep
                // the explicit reveal anchored until their heights are known.
                if self.sections[..=index]
                    .iter()
                    .all(|s| s.collapsed || s.lines.is_some())
                {
                    self.pending_reveal = None;
                }
            }
        }
        self.scroll_top = self.scroll_top.clamp(0.0, self.max_scroll);
    }

    fn reveal_editor_cursor(
        &mut self,
        pane: &mut CodePane,
        viewport: Rect,
        scale: f32,
        cell_w: f32,
    ) {
        if !pane.buffer.follow_cursor || !self.edit_focused || !self.host_focused {
            return;
        }
        let path = relative_path(&self.root, &pane.path);
        let Some(&index) = self.indices.get(&path) else {
            return;
        };
        let section = &self.sections[index];
        let split = self.split && viewport.w >= 520.0 * scale;
        let rows = if split {
            section.right.as_slice()
        } else {
            section.lines.as_deref().unwrap_or(&[])
        };
        let Some(row) = rows.iter().position(|line| {
            line.new_line_number == Some(pane.buffer.cursor_line as u32 + 1)
        }) else {
            return;
        };
        let mut visual = if split {
            row
        } else {
            section.wrapped_offsets.get(row).copied().unwrap_or(row)
        };
        let text = pane
            .buffer
            .lines
            .get(pane.buffer.cursor_line)
            .map(String::as_str)
            .unwrap_or("");
        let width =
            diff_card::body_text_width_with_gutter(viewport.w, scale, self.gutter_width);
        if !split {
            let fragments = diff_card::source_fragments(text, width, scale, false);
            visual += fragments
                .iter()
                .position(|fragment| {
                    pane.buffer.cursor_col >= fragment.byte_start
                        && pane.buffer.cursor_col <= fragment.byte_end
                })
                .unwrap_or(0);
        }
        let top = section.top
            + 32.0 * scale
            + diff_card::BODY_TOP_PAD * scale
            + visual as f32 * self.line_height;
        if top < self.scroll_top + 32.0 * scale {
            self.scroll_top = (top - 32.0 * scale).max(0.0);
        } else if top + self.line_height > self.scroll_top + viewport.h {
            self.scroll_top = top + self.line_height - viewport.h;
        }
        self.scroll_top = self.scroll_top.clamp(0.0, self.max_scroll);
        if split {
            let width = diff_card::body_text_width_with_gutter(
                (viewport.w - 2.0 * scale) / 2.0,
                scale,
                self.gutter_width,
            );
            let x = crate::editor::code::layout::display_col_for_byte(
                text,
                pane.buffer.cursor_col,
                crate::editor::code::layout::TAB_DISPLAY_WIDTH,
            ) as f32
                * cell_w;
            if x < self.horizontal[1] {
                self.horizontal[1] = x;
            } else if x + cell_w > self.horizontal[1] + width {
                self.horizontal[1] = (x + cell_w - width).max(0.0);
            }
        }
        self.pending_reveal = None;
        pane.buffer.follow_cursor = false;
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        rect: Rect,
        font_id: usize,
        font_size: f32,
        theme: &IdeTheme,
    ) {
        self.render_with_editor(sugarloaf, rect, font_id, font_size, theme, None);
    }

    pub fn render_with_editor(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        rect: Rect,
        _font_id: usize,
        font_size: f32,
        theme: &IdeTheme,
        mut editor: Option<&mut CodePane>,
    ) {
        if self.commit_id.is_some() {
            editor = None;
        }
        let rect = if let Some(metadata) = self.commit_metadata.as_mut() {
            metadata.render(
                sugarloaf,
                rect,
                (font_size / diff_card::FONT_SIZE).clamp(0.7, 2.5),
                theme,
                &self.text_occlusions,
            )
        } else {
            rect
        };
        self.live_diff_pending = false;
        if let Some(pane) = editor.as_deref_mut() {
            if self.reveal_edit_on_focus {
                pane.buffer.follow_cursor = true;
                self.reveal_edit_on_focus = false;
            }
            self.sync_live_editor(pane);
            if self.host_focused {
                pane.cursor_rect = None;
            }
        }
        self.edit_hits.clear();
        let previous_block_controls = std::mem::take(&mut self.block_controls);
        self.source_hits.clear();
        self.header_rects.clear();
        self.header_actions.clear();
        self.block_controls.clear();
        self.block_hover = None;
        self.toolbar_hits.clear();
        if rect.w <= 0.0 || rect.h <= 0.0 {
            return;
        }
        let scale = (font_size / diff_card::FONT_SIZE).clamp(0.7, 2.5);
        let clip = [rect.x, rect.y, rect.w, rect.h];
        sugarloaf.quad(
            None,
            rect.x,
            rect.y,
            rect.w,
            rect.h,
            theme.f32(theme.bg),
            [0.0; 4],
            0.0,
            20,
        );
        let toolbar_h = (36.0 * scale).min(rect.h);
        let opts = DrawOpts {
            font_size: 12.0 * scale,
            color: theme.u8(theme.dim),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        if self.commit_id.is_none() {
            self.render_toolbar(sugarloaf, rect, toolbar_h, scale, theme);
        }
        let notice_h = if self.error.is_some() && !self.sections.is_empty() {
            22.0 * scale
        } else {
            0.0
        };
        if notice_h > 0.0 {
            let notice_clip = [
                rect.x + 8.0 * scale,
                rect.y + toolbar_h,
                (rect.w - 16.0 * scale).max(0.0),
                notice_h,
            ];
            draw_text_with_occlusion(
                sugarloaf,
                notice_clip[0],
                notice_clip[1] + 4.0 * scale,
                self.error.as_deref().unwrap_or(""),
                &DrawOpts {
                    font_size: 11.0 * scale,
                    color: theme.u8(theme.red),
                    clip_rect: Some(notice_clip),
                    ..opts
                },
                &self.text_occlusions,
            );
        }
        let width = (rect.w - 20.0 * scale).max(0.0);
        let split = self.split && width >= 520.0 * scale;
        self.effective_split = split;
        if !split && self.column_focus == Some(0) {
            self.column_focus = Some(1);
            self.active_column = 1;
        }
        let scrollbar_h = if split { 12.0 * scale } else { 0.0 };
        let viewport = Rect::new(
            rect.x + 8.0 * scale,
            rect.y + toolbar_h + notice_h + 4.0 * scale,
            width,
            (rect.h - toolbar_h - notice_h - 4.0 * scale - scrollbar_h).max(0.0),
        );
        let max_line = self
            .sections
            .iter()
            .map(|section| section.max_line_number)
            .max()
            .unwrap_or(1);
        self.gutter_width = diff_card::measured_gutter_width(sugarloaf, max_line, scale);
        self.page_height = viewport.h;
        self.line_height = diff_card::LINE_HEIGHT * scale;
        self.measure_sections(viewport.w, scale, viewport.h);
        if let Some(pane) = editor.as_deref_mut() {
            let cell_w = sugarloaf
                .text_mut()
                .measure(
                    "0",
                    &DrawOpts {
                        font_size: diff_card::FONT_SIZE * scale,
                        ..DrawOpts::default()
                    },
                )
                .max(1.0);
            self.reveal_editor_cursor(pane, viewport, scale, cell_w);
        }
        let viewport_clip = [viewport.x, viewport.y, viewport.w, viewport.h];
        let opts = DrawOpts {
            clip_rect: Some(viewport_clip),
            ..opts
        };
        if self.sections.is_empty() {
            let message = self
                .error
                .as_deref()
                .unwrap_or(if self.commit_id.is_some() {
                    "This commit has no changed files."
                } else {
                    "No uncommitted changes."
                });
            draw_text_with_occlusion(
                sugarloaf,
                viewport.x + 8.0 * scale,
                viewport.y + 16.0 * scale,
                message,
                &opts,
                &self.text_occlusions,
            );
        }
        let column_w = (viewport.w - 2.0 * scale) / 2.0;
        let code_width =
            diff_card::body_text_width_with_gutter(column_w, scale, self.gutter_width);
        if split {
            for section in &mut self.sections {
                let top = viewport.y + section.top - self.scroll_top;
                if section.collapsed
                    || section.lines.is_none()
                    || top > viewport.y + viewport.h
                    || top + 32.0 * scale + section.body_height < viewport.y
                {
                    continue;
                }
                if section.metrics_scale != scale {
                    let lang = Lang::from_path(&section.change.path);
                    section.content_widths = [
                        diff_card::content_width(sugarloaf, &section.left, scale, lang),
                        diff_card::content_width(sugarloaf, &section.right, scale, lang),
                    ];
                    section.metrics_scale = scale;
                }
            }
        }
        for column in 0..2 {
            let content_width = self
                .sections
                .iter()
                .filter(|section| section.metrics_scale == scale && !section.collapsed)
                .map(|section| section.content_widths[column])
                .fold(0.0_f32, f32::max);
            self.max_horizontal[column] = if split {
                (content_width - code_width).max(0.0)
            } else {
                0.0
            };
            self.horizontal[column] =
                self.horizontal[column].clamp(0.0, self.max_horizontal[column]);
            let x = viewport.x + column as f32 * (column_w + 2.0 * scale);
            self.column_rects[column] = if split {
                Rect::new(x, viewport.y, column_w, viewport.h + scrollbar_h)
            } else if column == 1 {
                viewport
            } else {
                Rect::new(0.0, 0.0, 0.0, 0.0)
            };
            let track = if split && self.max_horizontal[column] > 0.0 {
                Rect::new(
                    x + self.gutter_width + diff_card::BODY_PAD_X * scale,
                    viewport.y + viewport.h,
                    code_width,
                    scrollbar_h,
                )
            } else {
                Rect::new(0.0, 0.0, 0.0, 0.0)
            };
            self.horizontal_tracks[column] = track;
            self.horizontal_thumbs[column] = horizontal_thumb(
                track,
                self.max_horizontal[column],
                self.horizontal[column],
                scale,
            );
        }
        for (index, section) in self.sections.iter().enumerate() {
            let natural_header_y = viewport.y + section.top - self.scroll_top;
            let header_h = 32.0 * scale;
            if natural_header_y > viewport.y + viewport.h
                || natural_header_y + header_h + section.body_height < viewport.y
            {
                continue;
            }
            let header_y =
                sticky_header_y(natural_header_y, section.body_height, viewport.y);
            let header = Rect::new(viewport.x, header_y, viewport.w, header_h);
            if let Some(visible) = intersection(header, viewport) {
                self.header_rects.push((index, visible));
                sugarloaf.quad(
                    None,
                    visible.x,
                    visible.y,
                    visible.w,
                    visible.h,
                    theme.f32(theme.border),
                    [3.0 * scale; 4],
                    0.0,
                    31,
                );
                if let Some(inner) = intersection(
                    Rect::new(
                        header.x + scale,
                        header.y + scale,
                        (header.w - 2.0 * scale).max(0.0),
                        header.h - 2.0 * scale,
                    ),
                    viewport,
                ) {
                    sugarloaf.quad(
                        None,
                        inner.x,
                        inner.y,
                        inner.w,
                        inner.h,
                        theme.f32(theme.surface),
                        [2.0 * scale; 4],
                        0.0,
                        32,
                    );
                }
                let text_clip = intersection(header, viewport).unwrap();
                let header_opts = DrawOpts {
                    color: theme.u8(theme.fg),
                    clip_rect: Some([text_clip.x, text_clip.y, text_clip.w, text_clip.h]),
                    ..opts
                };
                let chevron = if section.collapsed {
                    crate::panels::git_diff::chevron_right_glyph()
                } else {
                    crate::panels::git_diff::chevron_down_glyph()
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    header.x + 10.0 * scale,
                    header.y + 9.0 * scale,
                    chevron,
                    &header_opts,
                    &self.text_occlusions,
                );
                let _hovered = self
                    .header_pointer
                    .is_some_and(|(x, y)| visible.contains(x, y) && !self.occluded(x, y));
                let stage_label = if section.change.staged
                    && section.change.status != FileStatus::Mixed
                {
                    "Unstage"
                } else {
                    "Stage"
                };
                let stage_w = sugarloaf.text_mut().measure(stage_label, &header_opts)
                    + 16.0 * scale;
                let restore_w =
                    sugarloaf.text_mut().measure("Restore", &header_opts) + 16.0 * scale;
                let controls_w = stage_w + restore_w + 16.0 * scale;
                // In narrow panes hide the stats, rather than overlapping controls.
                // Mutations belong to the hovered contiguous change run, not
                // the file header. Repository/file commands remain in the toolbar.
                let show_controls = false;
                let controls_left = header.x + header.w - controls_w;
                if show_controls {
                    for (restore, label, x, width, allowed) in [
                        (false, stage_label, controls_left, stage_w, true),
                        (
                            true,
                            "Restore",
                            controls_left + stage_w + 4.0 * scale,
                            restore_w,
                            section.change.status != FileStatus::Untracked
                                && (!section.change.staged
                                    || section.change.status == FileStatus::Mixed),
                        ),
                    ] {
                        let button =
                            Rect::new(x, header.y + 4.0 * scale, width, 24.0 * scale);
                        let enabled =
                            allowed && !self.actions_loading && section.lines.is_some();
                        if let Some(hit) = intersection(button, visible) {
                            self.header_actions.push((index, restore, hit, enabled));
                            sugarloaf.quad(
                                None,
                                hit.x,
                                hit.y,
                                hit.w,
                                hit.h,
                                theme.f32(theme.border),
                                [3.0 * scale; 4],
                                0.0,
                                33,
                            );
                            draw_text_with_occlusion(
                                sugarloaf,
                                button.x + 8.0 * scale,
                                header.y + 9.0 * scale,
                                label,
                                &DrawOpts {
                                    color: theme.u8(if enabled {
                                        theme.fg
                                    } else {
                                        theme.dim
                                    }),
                                    ..header_opts
                                },
                                &self.text_occlusions,
                            );
                        }
                    }
                }
                let add = format!("+{}", section.change.additions);
                let del = format!("-{}", section.change.deletions);
                let add_width = sugarloaf.text_mut().measure(&add, &header_opts);
                let del_width = sugarloaf.text_mut().measure(&del, &header_opts);
                let stats_right = if show_controls {
                    controls_left - 8.0 * scale
                } else {
                    header.x + header.w - 12.0 * scale
                };
                let del_x = stats_right - del_width;
                let add_x = del_x - 10.0 * scale - add_width;
                draw_text_with_occlusion(
                    sugarloaf,
                    add_x,
                    header.y + 9.0 * scale,
                    &add,
                    &DrawOpts {
                        color: theme.u8(theme.green),
                        ..header_opts
                    },
                    &self.text_occlusions,
                );
                draw_text_with_occlusion(
                    sugarloaf,
                    del_x,
                    header.y + 9.0 * scale,
                    &del,
                    &DrawOpts {
                        color: theme.u8(theme.red),
                        ..header_opts
                    },
                    &self.text_occlusions,
                );
                let path = Path::new(&section.change.path);
                let name = path
                    .file_name()
                    .unwrap_or(path.as_os_str())
                    .to_string_lossy();
                let name_x = header.x + 32.0 * scale;
                let name_opts = DrawOpts {
                    clip_rect: Some([
                        name_x,
                        text_clip.y,
                        (add_x - 12.0 * scale - name_x).max(0.0),
                        text_clip.h,
                    ]),
                    ..header_opts
                };
                let name_width = draw_text_with_occlusion(
                    sugarloaf,
                    name_x,
                    header.y + 9.0 * scale,
                    &name,
                    &name_opts,
                    &self.text_occlusions,
                );
                if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                    draw_text_with_occlusion(
                        sugarloaf,
                        name_x + name_width + 10.0 * scale,
                        header.y + 9.0 * scale,
                        &dir.to_string_lossy(),
                        &DrawOpts {
                            color: theme.u8(theme.dim),
                            ..name_opts
                        },
                        &self.text_occlusions,
                    );
                }
            }
            if section.collapsed {
                continue;
            }
            let body_y = natural_header_y + header_h;
            let body_rect =
                Rect::new(viewport.x, body_y, viewport.w, section.body_height);
            let content_top = (header_y + header_h).max(viewport.y);
            let content_viewport = Rect::new(
                viewport.x,
                content_top,
                viewport.w,
                (viewport.y + viewport.h - content_top).max(0.0),
            );
            let Some(body_clip) = intersection(body_rect, content_viewport) else {
                continue;
            };
            let body_opts = DrawOpts {
                clip_rect: Some([body_clip.x, body_clip.y, body_clip.w, body_clip.h]),
                ..opts
            };
            let Some(lines) = &section.lines else {
                draw_text_with_occlusion(
                    sugarloaf,
                    viewport.x + 12.0 * scale,
                    body_y + 10.0 * scale,
                    self.error.as_deref().unwrap_or("Loading diff..."),
                    &body_opts,
                    &self.text_occlusions,
                );
                continue;
            };
            if lines.is_empty() {
                draw_text_with_occlusion(
                    sugarloaf,
                    viewport.x + 12.0 * scale,
                    body_y + 10.0 * scale,
                    "No textual changes (binary or metadata-only file).",
                    &body_opts,
                    &self.text_occlusions,
                );
                continue;
            }
            // Until a new projection is ready, retain the last patch as a
            // read-only snapshot rather than map stale rows onto newer text.
            let live_current = editor.as_ref().is_none_or(|pane| {
                relative_path(&self.root, &pane.path) != Path::new(&section.change.path)
                    || section.live_revision == Some(pane.buffer.revision)
            });
            if self.commit_id.is_none() && live_current {
                let label_opts = DrawOpts {
                    font_size: 12.0 * scale,
                    ..DrawOpts::default()
                };
                let stage_w =
                    sugarloaf.text_mut().measure("Stage", &label_opts) + 12.0 * scale;
                let restore_w =
                    sugarloaf.text_mut().measure("Restore", &label_opts) + 12.0 * scale;
                let panel_w = stage_w + restore_w + 4.0 * scale;
                for run in &section.runs {
                    let Some((first, end)) = blocks::run_rows(
                        run,
                        &section.split_origins,
                        &section.wrapped_offsets,
                        split,
                    ) else {
                        continue;
                    };
                    let top = body_y
                        + diff_card::BODY_TOP_PAD * scale
                        + first as f32 * self.line_height;
                    let Some(body) = blocks::block_body_rect(
                        first,
                        end,
                        body_y,
                        self.line_height,
                        scale,
                        body_clip,
                    ) else {
                        continue;
                    };
                    // Keep the compact island inside the viewport and below sticky headers.
                    let panel = Rect::new(
                        viewport.x + viewport.w - panel_w - 4.0 * scale,
                        top.max(body_clip.y),
                        panel_w,
                        24.0 * scale,
                    );
                    if panel_w + 8.0 * scale > viewport.w
                        || panel.y + panel.h > content_viewport.y + content_viewport.h
                    {
                        continue;
                    }
                    let shown = previous_block_controls.iter().any(|c| {
                        c.shown
                            && c.path == section.change.path
                            && c.block == run.block
                            && self.header_pointer.is_some_and(|(x, y)| {
                                c.panel.contains(x, y) && panel.contains(x, y)
                            })
                    });
                    self.block_controls.push(blocks::BlockControl {
                        path: section.change.path.clone(),
                        block: run.block.clone(),
                        body,
                        panel,
                        stage: Rect::new(
                            panel.x + 2.0 * scale,
                            panel.y,
                            stage_w,
                            panel.h,
                        ),
                        restore: Rect::new(
                            panel.x + 2.0 * scale + stage_w,
                            panel.y,
                            restore_w,
                            panel.h,
                        ),
                        shown,
                    });
                }
            }
            // Text is composited above ordered quads. Explicitly occlude the
            // underlying code under the island, including canonical projection.
            let code_occlusions = self.block_paint_occlusions();
            let columns = if split { 2 } else { 1 };
            let gap = if split { 2.0 * scale } else { 0.0 };
            let column_w = (viewport.w - gap) / columns as f32;
            for column in 0..columns {
                let spec = CardSpec {
                    path: &section.change.path,
                    link_target: None,
                    link_hovered: false,
                    additions: 0,
                    deletions: 0,
                    lang: Lang::from_path(&section.change.path),
                    diff_lines: if !split {
                        lines
                    } else if column == 0 {
                        &section.left
                    } else {
                        &section.right
                    },
                    visual_row_offsets: (!split)
                        .then_some(section.wrapped_offsets.as_slice()),
                    body_scroll: 0.0,
                    horizontal_scroll: split.then_some(self.horizontal[column]),
                };
                let working_tree = self.commit_id.is_none()
                    && live_current
                    && (!split || column == 1);
                let editor_attached = working_tree
                    && section.base_text.is_some()
                    && editor.as_ref().is_some_and(|pane| {
                        relative_path(&self.root, &pane.path)
                            == Path::new(&section.change.path)
                    });
                let column_x = viewport.x + column as f32 * (column_w + gap);
                let text_x = column_x + self.gutter_width + diff_card::BODY_PAD_X * scale;
                let code_width = diff_card::body_text_width_with_gutter(
                    column_w,
                    scale,
                    self.gutter_width,
                );
                let code_clip = intersection(
                    body_clip,
                    Rect::new(text_x, body_clip.y, code_width, body_clip.h),
                );
                let mut projected_rows = Vec::new();
                if working_tree {
                    if let Some(clip) = code_clip {
                        let cell_w = sugarloaf
                            .text_mut()
                            .measure(
                                "0",
                                &DrawOpts {
                                    font_size: diff_card::FONT_SIZE * scale,
                                    ..DrawOpts::default()
                                },
                            )
                            .max(1.0);
                        for (row, line) in spec.diff_lines.iter().enumerate() {
                            let Some(number) =
                                line.new_line_number.filter(|number| *number > 0)
                            else {
                                continue;
                            };
                            let visual = if split {
                                row
                            } else {
                                section.wrapped_offsets.get(row).copied().unwrap_or(row)
                            };
                            let count = if split {
                                1
                            } else {
                                section
                                    .wrapped_offsets
                                    .get(row + 1)
                                    .copied()
                                    .unwrap_or(visual + 1)
                                    .saturating_sub(visual)
                            };
                            let row_top = body_y
                                + diff_card::BODY_TOP_PAD * scale
                                + visual as f32 * self.line_height;
                            if row_top + count as f32 * self.line_height < clip.y
                                || row_top > clip.y + clip.h
                            {
                                continue;
                            }
                            for (fragment_ix, fragment) in diff_card::source_fragments(
                                &line.text, code_width, scale, split,
                            )
                            .into_iter()
                            .enumerate()
                            {
                                let y = row_top + fragment_ix as f32 * self.line_height;
                                let Some(hit_rect) = intersection(
                                    Rect::new(clip.x, y, clip.w, self.line_height),
                                    clip,
                                ) else {
                                    continue;
                                };
                                self.edit_hits.push(EditHit {
                                    path: PathBuf::from(&section.change.path),
                                    line: number.saturating_sub(1) as usize,
                                    byte_start: fragment.byte_start,
                                    byte_end: fragment.byte_end,
                                    text: line.text.clone(),
                                    text_x: text_x + fragment.indent_cols as f32 * cell_w,
                                    scroll_x: if split {
                                        self.horizontal[column]
                                    } else {
                                        0.0
                                    },
                                    cell_w,
                                    rect: hit_rect,
                                });
                                projected_rows.push(ProjectedCodeRow {
                                    source_line: number.saturating_sub(1) as usize,
                                    y,
                                    byte_start: fragment.byte_start,
                                    byte_end: fragment.byte_end,
                                    indent_cols: fragment.indent_cols,
                                });
                            }
                        }
                    }
                }
                // The common file header above owns collapse and metadata. Clip
                // away diff_card's header so both columns share that one header.
                let layout = diff_card::render_with_paint(
                    sugarloaf,
                    viewport.x + column as f32 * (column_w + gap),
                    body_y - diff_card::HEADER_HEIGHT * scale,
                    column_w,
                    section.body_height,
                    &spec,
                    scale,
                    theme,
                    0.0,
                    25,
                    body_clip.y,
                    body_clip.y + body_clip.h,
                    PaintParams {
                        clip: Some([body_clip.x, body_clip.y, body_clip.w, body_clip.h]),
                        occlusion_rects: &code_occlusions,
                        gutter_width: Some(self.gutter_width),
                        hatch_empty_rows: split,
                        review_separators: true,
                        hide_code_text: editor_attached,
                        link_line_numbers: working_tree,
                        line_number_pointer: self.header_pointer,
                        ..PaintParams::default()
                    },
                );
                self.source_hits
                    .extend(layout.line_number_hits.into_iter().map(|hit| SourceHit {
                        path: section.change.path.clone(),
                        line: hit.line,
                        rect: Rect::new(
                            hit.rect[0],
                            hit.rect[1],
                            hit.rect[2],
                            hit.rect[3],
                        ),
                    }));
                if editor_attached {
                    if let (Some(pane), Some(clip)) = (editor.as_deref_mut(), code_clip) {
                        // An unfocused review may share its canonical buffer
                        // with a visible editor; don't replace that editor's hit/caret geometry.
                        let inactive_geometry = (!self.host_focused)
                            .then(|| (pane.geometry.clone(), pane.cursor_rect));
                        render_projected_code(
                            sugarloaf,
                            pane,
                            &ProjectedCodeSpec {
                                viewport: [clip.x, clip.y, clip.w, clip.h],
                                text_x,
                                scroll_x: if split {
                                    self.horizontal[column]
                                } else {
                                    0.0
                                },
                                row_height: self.line_height,
                                font_size: diff_card::FONT_SIZE * scale,
                                rows: projected_rows,
                                focused: self.edit_focused && self.host_focused,
                            },
                            theme,
                            &code_occlusions,
                        );
                        if let Some((geometry, cursor)) = inactive_geometry {
                            pane.geometry = geometry;
                            pane.cursor_rect = cursor;
                        }
                    }
                }
            }
        }
        self.paint_block_controls(sugarloaf, scale, theme);
        for column in 0..2 {
            let track = self.horizontal_tracks[column];
            let thumb = self.horizontal_thumbs[column];
            if track.w > 0.0 && track.h > 0.0 {
                sugarloaf.quad(
                    None,
                    track.x,
                    track.y,
                    track.w,
                    track.h,
                    theme.f32(theme.bg),
                    [0.0; 4],
                    0.0,
                    40,
                );
                sugarloaf.quad(
                    None,
                    thumb.x,
                    thumb.y + 3.0 * scale,
                    thumb.w,
                    (thumb.h - 6.0 * scale).max(2.0 * scale),
                    theme.f32(theme.muted),
                    [2.0 * scale; 4],
                    0.0,
                    45,
                );
            }
        }
        if let Some(column) = self.column_focus.filter(|_| self.host_focused) {
            let focused = self.column_rects[column];
            let x = focused.x + self.gutter_width + diff_card::BODY_PAD_X * scale;
            let w = (focused.w - self.gutter_width - 2.0 * diff_card::BODY_PAD_X * scale)
                .max(0.0);
            if w > 0.0 {
                sugarloaf.rect(
                    None,
                    x,
                    rect.y + rect.h - 2.0 * scale,
                    w,
                    2.0 * scale,
                    theme.f32_alpha(theme.accent, 0.65),
                    0.0,
                    46,
                );
            }
        }
        self.scrollbar_rect = if self.max_scroll > 0.0 {
            Rect::new(
                rect.x + rect.w - 8.0 * scale,
                viewport.y,
                8.0 * scale,
                viewport.h,
            )
        } else {
            Rect::new(0.0, 0.0, 0.0, 0.0)
        };
        self.vertical_thumb = Rect::new(0.0, 0.0, 0.0, 0.0);
        if self.max_scroll > 0.0 && viewport.h > 0.0 {
            let thumb_h = (viewport.h * viewport.h / (viewport.h + self.max_scroll))
                .max(24.0 * scale)
                .min(viewport.h);
            let thumb_y =
                viewport.y + (viewport.h - thumb_h) * self.scroll_top / self.max_scroll;
            self.vertical_thumb = Rect::new(
                self.scrollbar_rect.x,
                thumb_y,
                self.scrollbar_rect.w,
                thumb_h,
            );
            sugarloaf.quad(
                None,
                self.scrollbar_rect.x + 2.0 * scale,
                thumb_y,
                3.0 * scale,
                thumb_h,
                theme.f32(theme.muted),
                [1.5 * scale; 4],
                0.0,
                45,
            );
        }
    }
}

fn sticky_header_y(natural_y: f32, body_height: f32, viewport_top: f32) -> f32 {
    natural_y.max(viewport_top).min(natural_y + body_height)
}

fn horizontal_thumb(track: Rect, max_scroll: f32, offset: f32, scale: f32) -> Rect {
    if track.w <= 0.0 || max_scroll <= 0.0 {
        return Rect::new(0.0, 0.0, 0.0, 0.0);
    }
    let width = (track.w * track.w / (track.w + max_scroll))
        .max(24.0 * scale)
        .min(track.w);
    Rect::new(
        track.x + (track.w - width) * (offset / max_scroll).clamp(0.0, 1.0),
        track.y,
        width,
        track.h,
    )
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
        .relative(&path.to_string_lossy())
        .map(PathBuf::from)
        .unwrap_or_else(|| path.to_path_buf())
}

fn intersection(a: Rect, b: Rect) -> Option<Rect> {
    let x = a.x.max(b.x);
    let y = a.y.max(b.y);
    let right = (a.x + a.w).min(b.x + b.w);
    let bottom = (a.y + a.h).min(b.y + b.h);
    (right > x && bottom > y).then_some(Rect::new(x, y, right - x, bottom - y))
}

fn same_lines(a: &[DiffLine], b: &[DiffLine]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(a, b)| {
            a.kind == b.kind
                && a.text == b.text
                && a.line_number == b.line_number
                && a.old_line_number == b.old_line_number
                && a.new_line_number == b.new_line_number
        })
}

fn split_lines(lines: &[DiffLine]) -> (Vec<DiffLine>, Vec<DiffLine>, Vec<usize>) {
    let blank = || DiffLine {
        text: String::new(),
        kind: DiffLineKind::Context,
        line_number: None,
        old_line_number: None,
        new_line_number: None,
    };
    let mut left = Vec::new();
    let mut right = Vec::new();
    let mut origins = Vec::new();
    let mut input = lines.iter().enumerate().peekable();
    while let Some((origin, line)) = input.next() {
        origins.push(origin);
        match line.kind {
            DiffLineKind::Remove => {
                left.push(line.clone());
                if input
                    .peek()
                    .is_some_and(|(_, next)| next.kind == DiffLineKind::Add)
                {
                    right.push(input.next().unwrap().1.clone());
                } else {
                    right.push(blank());
                }
            }
            DiffLineKind::Add => {
                left.push(blank());
                right.push(line.clone());
            }
            _ => {
                let mut old = line.clone();
                old.line_number = line.old_line_number;
                old.new_line_number = None;
                left.push(old);
                right.push(line.clone());
            }
        }
    }
    (left, right, origins)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(kind: DiffLineKind, text: &str) -> DiffLine {
        DiffLine {
            text: text.into(),
            kind,
            line_number: Some(1),
            old_line_number: Some(1),
            new_line_number: Some(1),
        }
    }
    fn file(path: &str) -> FileChange {
        FileChange {
            path: path.into(),
            status: FileStatus::Modified,
            additions: 1,
            deletions: 1,
            staged: false,
        }
    }

    #[test]
    fn review_paths_follow_host_semantics_not_the_clients_os() {
        let windows =
            GitDiffView::new(r"C:\host\repo".into(), r"C:\host\repo\src\file.rs".into());
        assert_eq!(windows.path, PathBuf::from("src/file.rs"));
        let unix =
            GitDiffView::new("/host/repo".into(), r"/host/repo/literal\name.rs".into());
        assert_eq!(unix.path, PathBuf::from(r"literal\name.rs"));
    }

    #[test]
    fn split_pairs_replacements_and_pads_single_sided_changes() {
        let rows = vec![
            line(DiffLineKind::Remove, "old"),
            line(DiffLineKind::Add, "new"),
            line(DiffLineKind::Add, "extra"),
            line(DiffLineKind::Context, "same"),
            line(DiffLineKind::Remove, "gone"),
        ];
        let (left, right, _) = split_lines(&rows);
        assert_eq!(left.len(), 4);
        assert_eq!(right.len(), 4);
        assert_eq!(left[0].text, "old");
        assert_eq!(right[0].text, "new");
        assert!(left[1].text.is_empty());
        assert_eq!(right[1].text, "extra");
        assert_eq!(left[2].text, right[2].text);
        assert!(right[3].text.is_empty());
    }

    #[test]
    fn snapshots_preserve_folds_scroll_and_loaded_lines() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_files(vec![file("a.rs"), file("b.rs")]);
        view.set_file_lines(Path::new("a.rs"), vec![line(DiffLineKind::Add, "new")]);
        view.toggle_section(0);
        view.max_scroll = 500.0;
        view.scroll(120.0);
        view.set_files(vec![file("a.rs"), file("b.rs"), file("c.rs")]);
        assert!(view.sections[0].collapsed);
        assert!(view.sections[0].lines.is_some());
        assert_eq!(view.scroll_top, 120.0);
        view.reveal_file(Path::new("/repo/a.rs"));
        assert!(!view.sections[0].collapsed);
        assert_eq!(view.path, PathBuf::from("a.rs"));
    }

    #[test]
    fn collapsed_files_only_reserve_their_header_and_reveal_jumps_to_target() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_files(vec![file("a.rs"), file("b.rs")]);
        for path in ["a.rs", "b.rs"] {
            view.set_file_lines(Path::new(path), vec![line(DiffLineKind::Add, "x"); 100]);
        }
        view.reveal_file(Path::new("b.rs"));
        view.measure_sections(800.0, 1.0, 400.0);
        assert_eq!(view.scroll_top, view.sections[1].top);
        let expanded = view.max_scroll;
        view.toggle_section(0);
        view.measure_sections(800.0, 1.0, 400.0);
        assert_eq!(view.sections[0].body_height, 0.0);
        assert_eq!(view.sections[1].top, 40.0);
        assert!(view.max_scroll < expanded);
    }

    #[test]
    fn toolbar_modes_have_independent_explicit_targets() {
        toolbar::test_mode_targets();
    }

    #[test]
    fn changing_diff_mode_does_not_unfold_collapsed_files() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_lines(vec![line(DiffLineKind::Add, "new")]);
        view.toggle_section(0);
        assert!(view.key("Tab"));
        view.measure_sections(800.0, 1.0, 400.0);
        assert!(view.sections[0].collapsed);
        assert_eq!(view.sections[0].body_height, 0.0);
    }

    #[test]
    fn headers_pin_until_the_file_ends_and_collapsed_headers_scroll_normally() {
        assert_eq!(sticky_header_y(180.0, 500.0, 100.0), 180.0);
        assert_eq!(sticky_header_y(-200.0, 500.0, 100.0), 100.0);
        assert_eq!(sticky_header_y(-450.0, 500.0, 100.0), 50.0);
        assert_eq!(sticky_header_y(-20.0, 0.0, 100.0), -20.0);
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.sections[0].top = 40.0;
        view.scroll_top = 300.0;
        view.toggle_section(0);
        assert!(view.sections[0].collapsed);
        assert_eq!(view.scroll_top, 40.0);
    }

    #[test]
    fn number_hover_and_click_open_the_hit_file_and_source_line_not_inline_editing() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.source_hits.push(SourceHit {
            path: "src/b.rs".into(),
            line: 41,
            rect: Rect::new(20.0, 40.0, 24.0, 18.0),
        });
        view.edit_hits.push(EditHit {
            path: "a.rs".into(),
            line: 1,
            byte_start: 0,
            byte_end: 4,
            text: "code".into(),
            text_x: 20.0,
            scroll_x: 0.0,
            cell_w: 10.0,
            rect: Rect::new(20.0, 40.0, 100.0, 18.0),
        });
        view.edit_path = Some("a.rs".into());
        view.set_edit_focus(true);
        assert!(view.pointer_move(30.0, 45.0));
        assert!(!view.pointer_move(31.0, 45.0));
        assert!(view.pointer_cursor(30.0, 45.0));
        assert!(!view.text_cursor(30.0, 45.0));
        assert!(!view.pointer_cursor(70.0, 45.0));
        assert!(view.text_cursor(70.0, 45.0));
        assert!(view.pointer_move(70.0, 45.0));
        assert!(view.pointer_move(30.0, 45.0));
        assert!(view.pointer_leave());
        assert!(view.header_pointer.is_none());
        assert!(!view.pointer_leave());
        assert!(view.pointer_down(30.0, 45.0));
        assert_eq!(
            view.take_actions(),
            vec![GitDiffViewAction::OpenSource {
                root: "/repo".into(),
                path: "src/b.rs".into(),
                line: 41,
                column: 0,
            }]
        );
        assert_eq!(view.path, Path::new("src/b.rs"));
        assert!(view.take_edit_request().is_none());
        assert!(view.editing_path().is_none());
        assert_eq!(view.document_path(), Some(PathBuf::from("/repo/a.rs")));
    }

    #[test]
    fn historical_and_occluded_number_hits_cannot_open_working_sources() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.source_hits.push(SourceHit {
            path: "a.rs".into(),
            line: 41,
            rect: Rect::new(20.0, 40.0, 24.0, 18.0),
        });
        view.set_text_occlusions(&[[20.0, 40.0, 24.0, 18.0]]);
        assert!(!view.pointer_move(30.0, 45.0));
        assert!(!view.pointer_cursor(30.0, 45.0));
        assert!(!view.pointer_down(30.0, 45.0));
        assert!(view.take_actions().is_empty());
        view.set_text_occlusions(&[]);
        view.commit_id = Some("abc123".into());
        assert!(!view.pointer_cursor(30.0, 45.0));
        assert!(!view.pointer_down(30.0, 45.0));
        assert!(view.take_actions().is_empty());
        assert!(view.take_edit_request().is_none());
    }

    #[test]
    fn source_hit_requests_canonical_document_positions_not_patch_offsets() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.edit_hits.push(EditHit {
            path: "a.rs".into(),
            line: 7,
            byte_start: 0,
            byte_end: "α\t新x".len(),
            text: "α\t新x".into(),
            text_x: 20.0,
            scroll_x: 0.0,
            cell_w: 10.0,
            rect: Rect::new(20.0, 40.0, 100.0, 20.0),
        });
        assert!(view.text_cursor(60.0, 50.0));
        view.pending_reveal = Some("old-selection.rs".into());
        assert!(view.pointer_down(60.0, 50.0));
        assert!(view.pending_reveal.is_none());
        assert_eq!(
            view.take_edit_request(),
            Some(GitDiffEditRequest {
                root: "/repo".into(),
                path: "a.rs".into(),
                line: 7,
                column: "α\t".len()
            })
        );
        assert_eq!(view.editing_path(), Some(PathBuf::from("/repo/a.rs")));
        view.set_edit_focus(false);
        assert!(view.editing_path().is_none());
        assert_eq!(view.document_path(), Some(PathBuf::from("/repo/a.rs")));
        view.set_edit_focus(true);
        view.set_error("Baseline unavailable".into());
        assert!(view.editing_path().is_none());
        assert_eq!(view.document_path(), Some(PathBuf::from("/repo/a.rs")));
        assert!(!view.text_cursor(10.0, 50.0));
    }

    #[cfg(not(target_arch = "wasm32"))]
    fn finish_live_review(view: &mut GitDiffView, pane: &mut CodePane) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            view.sync_live_editor(pane);
            if !view.needs_redraw() {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "live review did not finish");
            std::thread::yield_now();
        }
        assert_eq!(view.sections[0].live_revision, Some(pane.buffer.revision));
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn live_review_reads_the_actual_buffer_and_ignores_stale_disk_patches() {
        for mode in [
            crate::editor::code::CodeInputMode::Standard,
            crate::editor::code::CodeInputMode::Vim,
        ] {
            let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
            view.edit_path = Some("a.rs".into());
            view.set_edit_focus(true);
            view.set_base_text(Path::new("a.rs"), "old\n".into());
            let mut pane = CodePane::new("/repo/a.rs".into(), "new\n");
            pane.input_mode = mode;
            view.sync_live_editor(&mut pane);
            assert!(view.needs_redraw());
            pane.buffer.insert_text("live ");
            finish_live_review(&mut view, &mut pane);
            let lines = view.sections[0].lines.as_ref().unwrap();
            assert!(lines
                .iter()
                .any(|line| line.text == "live new" && line.kind == DiffLineKind::Add));
            let text = pane.buffer.text();
            view.set_file_lines(
                Path::new("a.rs"),
                vec![line(DiffLineKind::Add, "stale disk")],
            );
            assert!(view.sections[0]
                .lines
                .as_ref()
                .unwrap()
                .iter()
                .any(|line| line.text == "live new"));
            assert_eq!(pane.buffer.text(), text);
            assert_eq!(pane.input_mode, mode);
            view.set_files(Vec::new());
            assert_eq!(view.sections.len(), 1, "the active full-file document remains editable when disk status is clean");
        }
    }

    #[test]
    fn unchanged_baselines_share_the_latest_snapshot_without_invalidating() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        let first: std::sync::Arc<str> = "old\n".into();
        view.set_base_text(Path::new("a.rs"), first.clone());
        let revision = view.sections[0].base_revision;
        view.sections[0].live_revision = Some(42);
        let refreshed: std::sync::Arc<str> = "old\n".into();
        assert!(!std::sync::Arc::ptr_eq(&first, &refreshed));
        view.set_base_text(Path::new("a.rs"), refreshed.clone());
        assert!(std::sync::Arc::ptr_eq(
            view.sections[0].base_text.as_ref().unwrap(), &refreshed,
        ));
        assert_eq!(view.sections[0].base_revision, revision);
        assert_eq!(view.sections[0].live_revision, Some(42));
    }

    #[test]
    fn live_review_rejects_results_for_old_revisions_baselines_and_paths() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_base_text(Path::new("a.rs"), "old\n".into());
        let mut pane = CodePane::new("/repo/a.rs".into(), "new\n");
        let key = live::Key {
            path: "a.rs".into(),
            base_revision: view.sections[0].base_revision,
            revision: pane.buffer.revision,
            caret: 0,
        };
        let output = |key| live::Output {
            key,
            lines: vec![line(DiffLineKind::Add, "stale")],
        };
        pane.buffer.insert_text("latest ");
        view.apply_live_diff(output(key.clone()), &pane);
        assert!(view.sections[0].lines.is_none());

        let mut key = key;
        key.revision = pane.buffer.revision;
        view.set_base_text(Path::new("a.rs"), "different base\n".into());
        view.apply_live_diff(output(key.clone()), &pane);
        assert!(view.sections[0].lines.is_none());

        key.base_revision = view.sections[0].base_revision;
        key.path = "b.rs".into();
        view.apply_live_diff(output(key), &pane);
        assert!(view.sections[0].lines.is_none());
    }

    #[test]
    #[cfg(not(target_arch = "wasm32"))]
    fn pending_live_review_does_not_accept_disk_patches_or_queue_snapshots() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.set_base_text(Path::new("a.rs"), "old\n".into());
        let mut pane = CodePane::new("/repo/a.rs".into(), "new\n");
        view.sync_live_editor(&mut pane);
        assert!(view.live_diff.pending());
        view.set_file_lines(Path::new("a.rs"), vec![line(DiffLineKind::Add, "disk")]);
        assert!(view.sections[0].lines.is_none());
        for _ in 0..50 {
            pane.buffer.insert_text("x");
        }
        finish_live_review(&mut view, &mut pane);
        assert!(!view.live_diff.pending());
        assert!(view.sections[0].lines.as_ref().unwrap().iter()
            .any(|line| line.text == format!("{}new", "x".repeat(50))));
        view.sync_live_editor(&mut pane);
        assert!(!view.needs_redraw(), "unchanged reviews must stop polling");
    }

    #[test]
    fn vertical_thumb_drag_keeps_grab_offset_and_releases_outside_page() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.scrollbar_rect = Rect::new(500.0, 40.0, 8.0, 400.0);
        view.vertical_thumb = Rect::new(500.0, 40.0, 8.0, 80.0);
        view.max_scroll = 1600.0;
        assert!(view.pointer_down(504.0, 60.0));
        assert_eq!(view.scroll_top, 0.0);
        assert!(view.pointer_move(900.0, 220.0));
        assert_eq!(view.scroll_top, 800.0);
        assert!(view.pointer_move(900.0, 900.0));
        assert_eq!(view.scroll_top, 1600.0);
        assert!(view.pointer_up());
        assert!(!view.is_dragging());
    }

    #[test]
    fn horizontal_scroll_is_bounded_and_independent_for_each_split_column() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.max_horizontal = [400.0, 600.0];
        view.column_rects = [
            Rect::new(0.0, 0.0, 300.0, 400.0),
            Rect::new(302.0, 0.0, 300.0, 400.0),
        ];
        view.scroll_horizontal_at(500.0, 100.0, 80.0);
        assert_eq!(view.horizontal, [0.0, 80.0]);
        view.scroll_horizontal_at(100.0, 100.0, 10_000.0);
        assert_eq!(view.horizontal, [400.0, 80.0]);
        view.scroll_horizontal_at(500.0, 100.0, -10_000.0);
        assert_eq!(view.horizontal, [400.0, 0.0]);
    }

    #[test]
    fn horizontal_thumb_drag_preserves_grab_offset_and_releases_outside_view() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.max_horizontal = [400.0, 600.0];
        let track = Rect::new(100.0, 300.0, 200.0, 12.0);
        let thumb = horizontal_thumb(track, 400.0, 0.0, 1.0);
        view.horizontal_tracks[0] = track;
        view.horizontal_thumbs[0] = thumb;
        assert!(view.pointer_down(thumb.x + 10.0, thumb.y + 5.0));
        assert!(view.pointer_move(thumb.x + 10.0, thumb.y + 5.0));
        assert_eq!(view.horizontal[0], 0.0);
        assert!(view.pointer_move(500.0, 800.0));
        assert_eq!(view.horizontal, [400.0, 0.0]);
        assert!(view.pointer_up());
        assert!(!view.pointer_move(100.0, 300.0));
    }

    #[test]
    fn split_context_numbers_use_the_correct_source_and_track_large_gutters() {
        let mut row = line(DiffLineKind::Context, "same");
        row.old_line_number = Some(123_456);
        row.new_line_number = Some(223_456);
        let (left, right, _) = split_lines(&[row.clone()]);
        assert_eq!(left[0].line_number, Some(123_456));
        assert_eq!(left[0].new_line_number, None);
        assert_eq!(right[0].new_line_number, Some(223_456));
        let mut section = FileSection::new(file("a.rs"));
        section.set_lines(vec![row]);
        assert_eq!(section.max_line_number, 223_456);
    }

    #[test]
    fn header_actions_own_clicks_and_mixed_stages_remainder() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.sections[0].change.status = FileStatus::Mixed;
        view.sections[0].change.staged = true;
        view.header_rects
            .push((0, Rect::new(0.0, 40.0, 500.0, 32.0)));
        view.header_actions
            .push((0, false, Rect::new(350.0, 44.0, 60.0, 24.0), true));
        assert!(view.pointer_down(360.0, 50.0));
        assert!(!view.sections[0].collapsed);
        assert_eq!(
            view.take_actions(),
            vec![GitDiffViewAction::Stage {
                root: "/repo".into(),
                path: "a.rs".into(),
                unstage: false
            }]
        );
        view.actions_loading = true;
        assert!(view.pointer_down(360.0, 50.0));
        assert!(view.take_actions().is_empty());
        assert!(!view.sections[0].collapsed);
    }

    #[test]
    fn disabled_restore_and_occluded_controls_never_dispatch() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.header_actions
            .push((0, true, Rect::new(350.0, 44.0, 60.0, 24.0), false));
        assert!(view.pointer_down(360.0, 50.0));
        assert!(view.take_actions().is_empty());
        view.set_text_occlusions(&[[350.0, 44.0, 60.0, 24.0]]);
        assert!(!view.pointer_down(360.0, 50.0));
        assert!(view.take_actions().is_empty());
    }

    #[test]
    fn header_hover_invalidates_on_enter_and_exit() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.header_rects
            .push((0, Rect::new(0.0, 40.0, 500.0, 32.0)));
        assert!(view.pointer_move(100.0, 50.0));
        assert!(!view.pointer_move(200.0, 50.0));
        assert!(view.pointer_move(200.0, 100.0));
    }

    #[test]
    fn only_visible_headers_and_toolbar_are_click_targets() {
        let mut view = GitDiffView::new("/repo".into(), "a.rs".into());
        view.header_rects = vec![(0, Rect::new(10.0, 40.0, 500.0, 32.0))];
        assert!(view.pointer_cursor(20.0, 50.0));
        assert!(!view.pointer_cursor(20.0, 100.0));
        assert!(view.pointer_down(20.0, 50.0));
        assert!(view.sections[0].collapsed);
        view.set_text_occlusions(&[[0.0, 0.0, 100.0, 100.0]]);
        assert!(!view.pointer_cursor(20.0, 50.0));
        assert!(!view.pointer_down(20.0, 50.0));
    }
}
