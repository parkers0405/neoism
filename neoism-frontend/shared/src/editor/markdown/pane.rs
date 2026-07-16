use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
};

use neoism_terminal_core::ansi::CursorShape;
use web_time::Instant;

use super::helpers::{parse_blocks, source_from_lines, source_len_from_lines};
use super::types::*;
use super::vim::VimState;

const LARGE_MARKDOWN_FAST_PARSE_LINES: usize = 20_000;
const LARGE_MARKDOWN_FAST_PARSE_BYTES: usize = 2 * 1024 * 1024;

impl MarkdownPane {
    /// Construct a pane from in-memory source text (no filesystem read).
    /// Used by the web/wasm chrome where the daemon ships the file body
    /// over the wire — there is no on-disk path to `MarkdownPane::load`.
    /// `path` may be a stub (e.g. `PathBuf::from(title)`) since the
    /// renderer only needs `blocks` + `lines`.
    pub fn from_source(path: PathBuf, source: &str) -> Self {
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut lines: Vec<String> = source.lines().map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        let blocks = if large_markdown_source(source, lines.len()) {
            Vec::new()
        } else {
            parse_blocks(source)
        };
        let title = first_heading_title(source).unwrap_or_else(|| {
            if let Some(MarkdownBlock::Heading { text, .. }) = blocks.first() {
                text.clone()
            } else {
                title
            }
        });
        let saved_baseline = lines.clone();
        Self {
            path,
            title,
            source_len_bytes: source.len(),
            lines,
            blocks,
            source_revision: 1,
            pending_line_edit: None,
            mode: MarkdownMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            visual_anchor: None,
            mouse_select_anchor: None,
            cursor_rect: None,
            follow_cursor: false,
            goal_visual_col: None,
            scroll_y: 0.0,
            target_scroll_y: 0.0,
            cursor_scroll_remainder: 0.0,
            scroll_viewport_height: 0.0,
            scroll_velocity_px_s: 0.0,
            scroll_velocity_moves_cursor: false,
            remote_cursors: Vec::new(),
            scroll_last_tick_at: None,
            content_height: 0.0,
            block_rects: Vec::new(),
            notebook_image_preview_dimensions: HashMap::new(),
            block_wrap_rows: HashMap::new(),
            block_wrap_hit_stops: HashMap::new(),
            table_rects: Vec::new(),
            table_cell_rects: Vec::new(),
            table_action_rects: Vec::new(),
            task_rects: Vec::new(),
            roster_rects: Vec::new(),
            pending_reveal_line: None,
            outline_rects: Vec::new(),
            table_scrollbar_rects: Vec::new(),
            link_rects: Vec::new(),
            copy_rects: Vec::new(),
            notebook_run_rects: Vec::new(),
            notebook_action_hovered: None,
            table_scroll_x: HashMap::new(),
            task_toggle_animations: HashMap::new(),
            yank_flashes: Vec::new(),
            enter_continuation_lines: HashSet::new(),
            hovered_line: None,
            dragging_line: None,
            dragging_table_scroll: None,
            scrollbar_rect: None,
            dragging_scrollbar: None,
            scrollbar_hovered: false,
            table_action_hovered: false,
            drag_mouse_y: 0.0,
            drag_start_y: 0.0,
            drag_moved: false,
            drag_drop_flash: None,
            pending_block_menu_rect: None,
            vim: VimState::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            doc_history_bound: false,
            pending_doc_history: Vec::new(),
            wrap_cache: std::cell::RefCell::new(HashMap::new()),
            code_fence_cache: std::cell::RefCell::new(MarkdownCodeFenceCache::default()),
            link_target_cache: std::cell::RefCell::new(HashMap::new()),
            virtual_render: MarkdownVirtualRenderState::default(),
            saved_baseline,
            error: None,
            remote_content_pending: false,
            remote_loading_started: None,
            cover_overlay_rect: None,
            value_picker: None,
            available_covers: Vec::new(),
            value_picker_suppressed: None,
            title_edit: None,
            pending_title_rename: None,
        }
    }

    /// Replace the pane's source text and re-parse blocks. Cheap to
    /// call every time the host pushes a new content snapshot.
    pub fn set_source(&mut self, source: &str) {
        self.lines = source.lines().map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.source_len_bytes = source.len();
        self.pending_line_edit = None;
        self.enter_continuation_lines.clear();
        self.link_target_cache.borrow_mut().clear();
        self.clear_notebook_image_preview_dimensions();
        if large_markdown_source(source, self.lines.len()) {
            self.blocks.clear();
        } else {
            self.blocks = parse_blocks(source);
        }
        if let Some(title) = first_heading_title(source) {
            self.title = title;
        } else if let Some(MarkdownBlock::Heading { text, .. }) = self.blocks.first() {
            self.title = text.clone();
        }
        self.source_revision = self.source_revision.saturating_add(1);
        self.clamp_cursor();
        self.saved_baseline = self.lines.clone();
        self.error = None;
    }

    pub fn set_source_preserving_view(&mut self, source: &str) {
        let cursor_line = self.cursor_line;
        let cursor_col = self.cursor_col;
        let mode = self.mode;
        let scroll_y = self.scroll_y;
        let target_scroll_y = self.target_scroll_y;
        let follow_cursor = self.follow_cursor;
        let goal_visual_col = self.goal_visual_col;
        self.set_source(source);
        self.mode = mode;
        self.cursor_line = cursor_line.min(self.lines.len().saturating_sub(1));
        self.cursor_col = cursor_col.min(self.lines[self.cursor_line].len());
        self.scroll_y = scroll_y;
        self.target_scroll_y = target_scroll_y;
        self.follow_cursor = follow_cursor;
        self.goal_visual_col = goal_visual_col;
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.virtual_render = MarkdownVirtualRenderState::default();
    }

    pub fn load(path: PathBuf) -> Self {
        let title = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let mut pane = Self {
            path,
            title,
            lines: vec![String::new()],
            blocks: Vec::new(),
            source_len_bytes: 0,
            source_revision: 1,
            pending_line_edit: None,
            mode: MarkdownMode::Normal,
            cursor_line: 0,
            cursor_col: 0,
            visual_anchor: None,
            mouse_select_anchor: None,
            cursor_rect: None,
            follow_cursor: false,
            goal_visual_col: None,
            scroll_y: 0.0,
            target_scroll_y: 0.0,
            cursor_scroll_remainder: 0.0,
            scroll_viewport_height: 0.0,
            scroll_velocity_px_s: 0.0,
            scroll_velocity_moves_cursor: false,
            remote_cursors: Vec::new(),
            scroll_last_tick_at: None,
            content_height: 0.0,
            block_rects: Vec::new(),
            notebook_image_preview_dimensions: HashMap::new(),
            block_wrap_rows: HashMap::new(),
            block_wrap_hit_stops: HashMap::new(),
            table_rects: Vec::new(),
            table_cell_rects: Vec::new(),
            table_action_rects: Vec::new(),
            task_rects: Vec::new(),
            roster_rects: Vec::new(),
            pending_reveal_line: None,
            outline_rects: Vec::new(),
            table_scrollbar_rects: Vec::new(),
            link_rects: Vec::new(),
            copy_rects: Vec::new(),
            notebook_run_rects: Vec::new(),
            notebook_action_hovered: None,
            table_scroll_x: HashMap::new(),
            task_toggle_animations: HashMap::new(),
            yank_flashes: Vec::new(),
            enter_continuation_lines: HashSet::new(),
            hovered_line: None,
            dragging_line: None,
            dragging_table_scroll: None,
            scrollbar_rect: None,
            dragging_scrollbar: None,
            scrollbar_hovered: false,
            table_action_hovered: false,
            drag_mouse_y: 0.0,
            drag_start_y: 0.0,
            drag_moved: false,
            drag_drop_flash: None,
            pending_block_menu_rect: None,
            vim: VimState::default(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            doc_history_bound: false,
            pending_doc_history: Vec::new(),
            wrap_cache: std::cell::RefCell::new(HashMap::new()),
            code_fence_cache: std::cell::RefCell::new(MarkdownCodeFenceCache::default()),
            link_target_cache: std::cell::RefCell::new(HashMap::new()),
            virtual_render: MarkdownVirtualRenderState::default(),
            saved_baseline: vec![String::new()],
            error: None,
            remote_content_pending: false,
            remote_loading_started: None,
            cover_overlay_rect: None,
            value_picker: None,
            available_covers: Vec::new(),
            value_picker_suppressed: None,
            title_edit: None,
            pending_title_rename: None,
        };
        pane.reload();
        pane
    }

    pub fn reload(&mut self) {
        match std::fs::read_to_string(&self.path) {
            Ok(source) => self.apply_source(&source),
            Err(err) => {
                self.blocks.clear();
                self.error = Some(err.to_string());
            }
        }
    }

    fn apply_source(&mut self, source: &str) {
        self.lines = source.lines().map(str::to_string).collect();
        if self.lines.is_empty() {
            self.lines.push(String::new());
        }
        self.source_len_bytes = source.len();
        self.pending_line_edit = None;
        self.enter_continuation_lines.clear();
        self.link_target_cache.borrow_mut().clear();
        if self.should_defer_block_parse() {
            self.blocks.clear();
            self.source_revision = self.source_revision.saturating_add(1);
        } else {
            self.rebuild_blocks();
        }
        if let Some(title) = first_heading_title(source) {
            self.title = title;
        } else if let Some(MarkdownBlock::Heading { text, .. }) = self.blocks.first() {
            self.title = text.clone();
        }
        self.clamp_cursor();
        self.saved_baseline = self.lines.clone();
        self.error = None;
    }

    /// Content fetched from a workspace daemon's files plane (the path
    /// only exists on the HOST machine, so the local `reload` read set
    /// `error`). Replaces the pane content exactly like a successful
    /// local reload would.
    pub fn apply_remote_source(&mut self, source: &str) {
        self.apply_source(source);
        self.remote_content_pending = false;
        self.remote_loading_started = None;
    }

    /// Local read failed because the file lives on a joined server —
    /// render the loading skeleton instead of the raw os error while
    /// the daemon read is in flight, and keep the CRDT drain from
    /// seeding the buffer with placeholder text.
    pub fn mark_remote_loading(&mut self) {
        self.blocks.clear();
        self.error = None;
        self.remote_content_pending = true;
        self.remote_loading_started = Some(Instant::now());
    }

    /// The frontmatter decoration key (`icon:` / `cover:`) the cursor's
    /// line edits, if any.
    fn decoration_key_at_cursor(&self) -> Option<MarkdownDecorationKey> {
        let fm = {
            // Inline frontmatter_range logic — it lives on the render_data
            // impl but the fields are the same struct.
            if self.lines.first().map(|line| line.trim()) != Some("---") {
                return None;
            }
            let end = self
                .lines
                .iter()
                .enumerate()
                .skip(1)
                .take(64)
                .find(|(_, line)| line.trim() == "---")?
                .0;
            1..end
        };
        if !fm.contains(&self.cursor_line) {
            return None;
        }
        let (key, _) = self.lines.get(self.cursor_line)?.split_once(':')?;
        match key.trim().to_ascii_lowercase().as_str() {
            "icon" => Some(MarkdownDecorationKey::Icon),
            "cover" => Some(MarkdownDecorationKey::Cover),
            _ => None,
        }
    }

    /// Keep the value picker in sync with the cursor: open while the
    /// cursor edits an `icon:`/`cover:` frontmatter line in Insert mode,
    /// closed otherwise. Called once per render frame.
    pub fn refresh_value_picker(&mut self) {
        let open = (self.mode == MarkdownMode::Insert)
            .then(|| self.decoration_key_at_cursor())
            .flatten();
        match open {
            Some(key) => {
                // A just-accepted line stays closed until its text
                // changes — otherwise the per-frame refresh reopens the
                // menu immediately and eats the next Enter.
                let suppressed =
                    self.value_picker_suppressed
                        .as_ref()
                        .is_some_and(|(line, text)| {
                            *line == self.cursor_line
                                && self.lines.get(*line).map(String::as_str)
                                    == Some(text.as_str())
                        });
                if suppressed {
                    self.value_picker = None;
                    return;
                }
                let stale = self
                    .value_picker
                    .as_ref()
                    .map(|picker| picker.key != key || picker.line != self.cursor_line)
                    .unwrap_or(true);
                if stale {
                    self.value_picker = Some(MarkdownValuePicker {
                        key,
                        line: self.cursor_line,
                        selected: 0,
                    });
                }
            }
            None => self.value_picker = None,
        }
    }

    /// The picker's candidate list, filtered by the value text already
    /// typed on the line (the line IS the search bar). `(value, label)`.
    pub fn value_picker_candidates(&self) -> Vec<(String, String)> {
        let Some(picker) = self.value_picker.as_ref() else {
            return Vec::new();
        };
        let filter = self
            .lines
            .get(picker.line)
            .and_then(|line| line.split_once(':'))
            .map(|(_, value)| value.trim().to_ascii_lowercase())
            .unwrap_or_default();
        let matches = |label: &str, value: &str| {
            filter.is_empty()
                || label.to_ascii_lowercase().contains(&filter)
                || value.to_ascii_lowercase().contains(&filter)
        };
        match picker.key {
            MarkdownDecorationKey::Icon => EMOJI_CHOICES
                .iter()
                .filter(|(name, emoji)| matches(name, emoji))
                .map(|(name, emoji)| (emoji.to_string(), format!("{emoji}  {name}")))
                .collect(),
            MarkdownDecorationKey::Cover => self
                .available_covers
                .iter()
                .filter(|name| matches(name, name))
                .map(|name| (name.clone(), format!("\u{1f5bc}  {name}")))
                .collect(),
        }
    }

    pub fn value_picker_move(&mut self, delta: isize) {
        let count = self.value_picker_candidates().len();
        if count == 0 {
            return;
        }
        if let Some(picker) = self.value_picker.as_mut() {
            let current = picker.selected.min(count - 1) as isize;
            picker.selected =
                (current + delta).rem_euclid(count as isize) as usize;
        }
    }

    /// Accept the highlighted candidate: rewrite the line's value, close
    /// the picker. Returns false when nothing was accepted (caller lets
    /// the key fall through).
    pub fn value_picker_accept(&mut self) -> bool {
        let candidates = self.value_picker_candidates();
        let Some(picker) = self.value_picker.take() else {
            return false;
        };
        let Some((value, _)) = candidates.get(picker.selected.min(
            candidates.len().saturating_sub(1),
        )) else {
            return false;
        };
        let key = match picker.key {
            MarkdownDecorationKey::Icon => "icon",
            MarkdownDecorationKey::Cover => "cover",
        };
        if let Some(line) = self.lines.get_mut(picker.line) {
            *line = format!("{key}: {value}");
            self.cursor_line = picker.line;
            self.cursor_col = line.len();
        }
        // Keep the menu closed on this line until its text changes.
        self.value_picker_suppressed = Some((
            picker.line,
            self.lines.get(picker.line).cloned().unwrap_or_default(),
        ));
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.source_revision = self.source_revision.saturating_add(1);
        self.rebuild_blocks();
        true
    }

    fn byte_index_at(value: &str, caret_chars: usize) -> usize {
        value
            .char_indices()
            .nth(caret_chars)
            .map(|(index, _)| index)
            .unwrap_or(value.len())
    }

    /// The title the page header shows: `title:` frontmatter, else the
    /// file stem, else the stored pane title.
    pub fn display_title(&self) -> String {
        self.frontmatter_property("title")
            .or_else(|| {
                self.path
                    .file_stem()
                    .map(|stem| stem.to_string_lossy().into_owned())
                    .filter(|stem| !stem.is_empty())
            })
            .unwrap_or_else(|| self.title.clone())
    }

    /// Start editing the virtual title line (ArrowUp/`k` from the top of
    /// the buffer). Caret starts at the end.
    pub fn begin_title_edit(&mut self) {
        let text = self.display_title();
        let count = text.chars().count();
        // Vim parity: Normal parks the block ON the last char, Insert
        // puts the beam after it.
        let caret = if matches!(self.mode, MarkdownMode::Insert) {
            count
        } else {
            count.saturating_sub(1)
        };
        self.title_edit = Some(MarkdownTitleEdit { text, caret });
    }

    pub fn title_edit_insert(&mut self, text: &str) {
        if let Some(edit) = self.title_edit.as_mut() {
            let at = Self::byte_index_at(&edit.text, edit.caret);
            edit.text.insert_str(at, text);
            edit.caret += text.chars().count();
        }
    }

    pub fn title_edit_backspace(&mut self) {
        if let Some(edit) = self.title_edit.as_mut() {
            if edit.caret == 0 {
                return;
            }
            let start = Self::byte_index_at(&edit.text, edit.caret - 1);
            let end = Self::byte_index_at(&edit.text, edit.caret);
            edit.text.replace_range(start..end, "");
            edit.caret -= 1;
        }
    }

    pub fn title_edit_delete(&mut self) {
        if let Some(edit) = self.title_edit.as_mut() {
            let start = Self::byte_index_at(&edit.text, edit.caret);
            let end = Self::byte_index_at(&edit.text, edit.caret + 1);
            if start < end {
                edit.text.replace_range(start..end, "");
            }
        }
    }

    pub fn title_edit_move(&mut self, delta: isize) {
        if let Some(edit) = self.title_edit.as_mut() {
            let count = edit.text.chars().count() as isize;
            edit.caret = (edit.caret as isize + delta).clamp(0, count) as usize;
        }
    }

    pub fn title_edit_home(&mut self) {
        if let Some(edit) = self.title_edit.as_mut() {
            edit.caret = 0;
        }
    }

    pub fn title_edit_end(&mut self) {
        if let Some(edit) = self.title_edit.as_mut() {
            edit.caret = edit.text.chars().count();
        }
    }

    /// Commit the title edit: update `title:` frontmatter when the note
    /// declares one, and queue the file rename for the host.
    pub fn commit_title_edit(&mut self) {
        let Some(edit) = self.title_edit.take() else {
            return;
        };
        let text = edit.text.trim().to_string();
        if text.is_empty() || text == self.display_title() {
            return;
        }
        if self.frontmatter_property("title").is_some() {
            self.set_frontmatter_property("title", &text);
        }
        self.title = text.clone();
        self.pending_title_rename = Some(text);
    }

    pub fn cancel_title_edit(&mut self) {
        self.title_edit = None;
    }

    pub fn take_pending_title_rename(&mut self) -> Option<String> {
        self.pending_title_rename.take()
    }

    /// Set (or replace) a frontmatter property — the write half of the
    /// icon/cover pickers. Creates the properties block when the note
    /// has none. The saved baseline is untouched, so the tab reads dirty
    /// until the user saves, like any other edit.
    pub fn set_frontmatter_property(&mut self, key: &str, value: &str) {
        let line_text = format!("{key}: {value}");
        let has_frontmatter = self.lines.first().map(|line| line.trim()) == Some("---");
        if has_frontmatter {
            let close = self
                .lines
                .iter()
                .enumerate()
                .skip(1)
                .take(64)
                .find(|(_, line)| line.trim() == "---")
                .map(|(index, _)| index);
            if let Some(close) = close {
                let existing = self.lines[1..close].iter().position(|line| {
                    line.split_once(':')
                        .map(|(k, _)| k.trim().eq_ignore_ascii_case(key))
                        .unwrap_or(false)
                });
                match existing {
                    Some(offset) => self.lines[1 + offset] = line_text,
                    None => self.lines.insert(close, line_text),
                }
            } else {
                // Unterminated fence — treat as no frontmatter.
                self.lines
                    .splice(0..0, ["---".to_string(), line_text, "---".to_string()]);
            }
        } else {
            self.lines
                .splice(0..0, ["---".to_string(), line_text, "---".to_string()]);
        }
        self.pending_line_edit = Some(MarkdownPendingLineEdit::Complex);
        self.source_revision = self.source_revision.saturating_add(1);
        self.rebuild_blocks();
        self.clamp_cursor();
    }

    pub fn cursor_shape(&self) -> CursorShape {
        match self.mode {
            MarkdownMode::Normal | MarkdownMode::Visual => CursorShape::Block,
            MarkdownMode::Insert => CursorShape::Beam,
        }
    }

    /// A buffer is dirty when its current content differs from the
    /// last-saved baseline — exactly how nvim reports `modified`. This
    /// is recomputed on every call (cheap pointer/length-first `Vec`
    /// compare), so an undo back to the saved text reads clean again
    /// and a redo into a divergent state reads dirty, without any edit
    /// path having to flip a flag.
    pub fn is_dirty(&self) -> bool {
        self.lines != self.saved_baseline
    }

    /// The document was flushed to disk by the daemon (single-writer
    /// save): the doc-level dirty bit clears without this pane having
    /// written anything itself. Re-anchor the saved baseline to the
    /// current content so `is_dirty()` reads clean.
    pub fn mark_saved(&mut self) {
        self.saved_baseline = self.lines.clone();
        self.error = None;
    }

    pub(crate) fn should_defer_block_parse(&self) -> bool {
        self.lines.len() > LARGE_MARKDOWN_FAST_PARSE_LINES
            || self.source_len_bytes > LARGE_MARKDOWN_FAST_PARSE_BYTES
    }

    pub(crate) fn should_use_local_history(&self) -> bool {
        self.should_defer_block_parse()
    }

    pub(crate) fn adjust_source_len(&mut self, delta: isize) {
        if delta >= 0 {
            self.source_len_bytes = self.source_len_bytes.saturating_add(delta as usize);
        } else {
            self.source_len_bytes =
                self.source_len_bytes.saturating_sub(delta.unsigned_abs());
        }
    }

    pub(crate) fn reset_source_len_from_lines(&mut self) {
        self.source_len_bytes = source_len_from_lines(&self.lines);
    }

    pub(crate) fn record_line_insert(&mut self, line: usize, byte_delta: i64) {
        self.pending_line_edit = match self.pending_line_edit {
            None => Some(MarkdownPendingLineEdit::Insert { line, byte_delta }),
            Some(MarkdownPendingLineEdit::Insert {
                line: existing,
                byte_delta: existing_delta,
            }) if existing == line => Some(MarkdownPendingLineEdit::Insert {
                line,
                byte_delta: existing_delta.saturating_add(byte_delta),
            }),
            Some(MarkdownPendingLineEdit::Insert {
                line: existing,
                byte_delta: existing_delta,
            }) if existing.saturating_add(1) == line => {
                Some(MarkdownPendingLineEdit::Insert {
                    line: existing,
                    byte_delta: existing_delta.saturating_add(byte_delta),
                })
            }
            _ => Some(MarkdownPendingLineEdit::Complex),
        };
    }

    pub(crate) fn record_line_delete(&mut self, line: usize, byte_delta: i64) {
        self.pending_line_edit = match self.pending_line_edit {
            None => Some(MarkdownPendingLineEdit::Delete { line, byte_delta }),
            Some(MarkdownPendingLineEdit::Delete {
                line: existing,
                byte_delta: existing_delta,
            }) if existing == line => Some(MarkdownPendingLineEdit::Delete {
                line,
                byte_delta: existing_delta.saturating_add(byte_delta),
            }),
            _ => Some(MarkdownPendingLineEdit::Complex),
        };
    }

    pub(crate) fn extend_pending_line_edit_bytes(&mut self, byte_delta: i64) {
        self.pending_line_edit = match self.pending_line_edit {
            Some(MarkdownPendingLineEdit::Insert {
                line,
                byte_delta: existing,
            }) => Some(MarkdownPendingLineEdit::Insert {
                line,
                byte_delta: existing.saturating_add(byte_delta),
            }),
            Some(MarkdownPendingLineEdit::Delete {
                line,
                byte_delta: existing,
            }) => Some(MarkdownPendingLineEdit::Delete {
                line,
                byte_delta: existing.saturating_add(byte_delta),
            }),
            other => other,
        };
    }

    pub fn save(&mut self) -> std::io::Result<()> {
        let source = source_from_lines(&self.lines);
        match std::fs::write(&self.path, source) {
            Ok(()) => {
                self.saved_baseline = self.lines.clone();
                self.error = None;
                Ok(())
            }
            Err(err) => {
                self.error = Some(err.to_string());
                Err(err)
            }
        }
    }
}

fn large_markdown_source(source: &str, line_count: usize) -> bool {
    line_count > LARGE_MARKDOWN_FAST_PARSE_LINES
        || source.len() > LARGE_MARKDOWN_FAST_PARSE_BYTES
}

fn first_heading_title(source: &str) -> Option<String> {
    source.lines().take(512).find_map(|line| {
        let trimmed = line.trim_start();
        let level = trimmed.chars().take_while(|ch| *ch == '#').count();
        if !(1..=6).contains(&level)
            || !trimmed.chars().nth(level).is_some_and(|ch| ch == ' ')
        {
            return None;
        }
        let title = trimmed[level..].trim();
        (!title.is_empty()).then(|| title.to_string())
    })
}

/// Curated page-icon set for the `icon:` value picker — searchable by
/// name, rendered with the native emoji font stack. Deliberately a
/// hand-picked working set, not the full Unicode table.
pub(super) const EMOJI_CHOICES: &[(&str, &str)] = &[
    ("rocket", "🚀"),
    ("fire", "🔥"),
    ("sparkles", "✨"),
    ("star", "⭐"),
    ("bolt", "⚡"),
    ("bulb", "💡"),
    ("brain", "🧠"),
    ("target", "🎯"),
    ("check", "✅"),
    ("warning", "⚠️"),
    ("pin", "📌"),
    ("bookmark", "🔖"),
    ("book", "📖"),
    ("books", "📚"),
    ("notebook", "📓"),
    ("memo", "📝"),
    ("clipboard", "📋"),
    ("folder", "📁"),
    ("inbox", "📥"),
    ("package", "📦"),
    ("calendar", "📅"),
    ("clock", "⏰"),
    ("hourglass", "⏳"),
    ("chart up", "📈"),
    ("chart down", "📉"),
    ("bar chart", "📊"),
    ("money", "💰"),
    ("gem", "💎"),
    ("key", "🔑"),
    ("lock", "🔒"),
    ("unlock", "🔓"),
    ("shield", "🛡️"),
    ("tools", "🛠️"),
    ("wrench", "🔧"),
    ("hammer", "🔨"),
    ("gear", "⚙️"),
    ("link", "🔗"),
    ("magnet", "🧲"),
    ("microscope", "🔬"),
    ("telescope", "🔭"),
    ("satellite", "🛰️"),
    ("computer", "💻"),
    ("keyboard", "⌨️"),
    ("robot", "🤖"),
    ("alien", "👾"),
    ("ghost", "👻"),
    ("skull", "💀"),
    ("crown", "👑"),
    ("trophy", "🏆"),
    ("medal", "🏅"),
    ("flag", "🚩"),
    ("map", "🗺️"),
    ("compass", "🧭"),
    ("globe", "🌍"),
    ("mountain", "🏔️"),
    ("volcano", "🌋"),
    ("ocean", "🌊"),
    ("droplet", "💧"),
    ("snowflake", "❄️"),
    ("cloud", "☁️"),
    ("rainbow", "🌈"),
    ("sun", "☀️"),
    ("moon", "🌙"),
    ("comet", "☄️"),
    ("plant", "🌱"),
    ("tree", "🌳"),
    ("leaf", "🍃"),
    ("flower", "🌸"),
    ("mushroom", "🍄"),
    ("bug", "🐛"),
    ("butterfly", "🦋"),
    ("bird", "🐦"),
    ("owl", "🦉"),
    ("fox", "🦊"),
    ("wolf", "🐺"),
    ("cat", "🐱"),
    ("dog", "🐶"),
    ("dragon", "🐉"),
    ("unicorn", "🦄"),
    ("whale", "🐋"),
    ("octopus", "🐙"),
    ("coffee", "☕"),
    ("tea", "🍵"),
    ("pizza", "🍕"),
    ("cake", "🎂"),
    ("apple", "🍎"),
    ("avocado", "🥑"),
    ("beer", "🍺"),
    ("wine", "🍷"),
    ("music", "🎵"),
    ("guitar", "🎸"),
    ("art", "🎨"),
    ("camera", "📷"),
    ("film", "🎬"),
    ("game", "🎮"),
    ("dice", "🎲"),
    ("puzzle", "🧩"),
    ("gift", "🎁"),
    ("party", "🎉"),
    ("balloon", "🎈"),
    ("heart", "❤️"),
    ("purple heart", "💜"),
    ("broken heart", "💔"),
    ("100", "💯"),
    ("eyes", "👀"),
    ("wave", "👋"),
    ("muscle", "💪"),
    ("handshake", "🤝"),
    ("thinking", "🤔"),
    ("smile", "😄"),
    ("cool", "😎"),
    ("mind blown", "🤯"),
    ("home", "🏠"),
    ("office", "🏢"),
    ("factory", "🏭"),
    ("bank", "🏦"),
    ("bus", "🚌"),
    ("car", "🚗"),
    ("bike", "🚲"),
    ("plane", "✈️"),
    ("ship", "🚢"),
    ("train", "🚆"),
    ("anchor", "⚓"),
    ("infinity", "♾️"),
    ("recycle", "♻️"),
    ("atom", "⚛️"),
    ("dna", "🧬"),
    ("pill", "💊"),
    ("syringe", "💉"),
    ("bell", "🔔"),
    ("mega", "📣"),
    ("mail", "📧"),
    ("phone", "📱"),
];
