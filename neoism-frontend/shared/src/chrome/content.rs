use super::*;

use crate::editor_snapshot::GridScrollEdgeCapture;
use crate::panels::buffer_tabs::BufferTabTarget;
use crate::render_policy::{
    editor_grid_hit_cell, editor_scroll_render_offset_for_mutated_snapshot,
    EditorScrollGridRenderState,
};

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Recompute the status-line `mode` / `primary_kind` from whatever
    /// surface is currently focused and push it into the status line.
    ///
    /// Desktop drives this every frame from `context_manager.current()`
    /// (see `desktop/src/screen/render/mod.rs`); the web host has no
    /// equivalent loop, so the mode pill used to be stuck on the
    /// startup `Mode::Terminal`. Calling this each frame mirrors the
    /// desktop behavior: switching tabs/surfaces flips the pill, and the
    /// status line's scramble/rainbow transition fires automatically
    /// because `StatusLine::set_info` starts the animation whenever the
    /// mode changes.
    ///
    /// For File (nvim) surfaces, the vi mode (`Normal`/`Insert`/…) is a
    /// host-pushed signal — desktop reads it from nvim, the web host can
    /// push it via the `set_status_mode_*` bridge setters. We preserve
    /// any such editor mode already on the status line so we don't stomp
    /// it back to `Normal` every frame; only the surface KIND is
    /// authoritative here.
    pub fn sync_status_mode(&mut self) {
        use crate::editor::markdown::MarkdownMode;
        use crate::panels::status_line::{Mode, PrimaryKind};

        let target = self.buffer_tabs.target_at(self.active_tab_index);

        // Decide the surface and the matching mode/primary. Order
        // mirrors the desktop's `render` cascade: agent → markdown →
        // editor/file → terminal.
        let (mode, primary_kind, primary): (Mode, PrimaryKind, String) =
            if self.is_neoism_agent_tab_active() {
                (Mode::Agent, PrimaryKind::Agent, "Neoism Agent".to_string())
            } else if let Some(pane) = self.markdown_pane.as_ref() {
                // A markdown tab paints through the live pane. Map the
                // pane's own edit mode onto the vi-style pill where it
                // makes sense, defaulting to the Markdown surface mode.
                let mode = match pane.mode {
                    MarkdownMode::Insert => Mode::Insert,
                    MarkdownMode::Visual => Mode::Visual,
                    MarkdownMode::Normal => Mode::Markdown,
                };
                let primary = pane
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "Markdown".to_string());
                (mode, PrimaryKind::File, primary)
            } else if matches!(target, Some(BufferTabTarget::Markdown(_))) {
                // Markdown tab whose pane hasn't been seeded yet.
                let primary = match &target {
                    Some(BufferTabTarget::Markdown(path)) => path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "Markdown".to_string()),
                    _ => "Markdown".to_string(),
                };
                (Mode::Markdown, PrimaryKind::File, primary)
            } else if matches!(target, Some(BufferTabTarget::File(_)))
                || self.editor_grid.is_some()
            {
                // nvim-backed file viewer. The vi mode is host-driven;
                // keep whatever editor mode is already shown (so Insert/
                // Visual/etc. survive), but fall back to Normal when the
                // pill is still carrying a non-editor mode (e.g. coming
                // straight from a Terminal tab).
                let current = self.status_line.info().mode;
                let mode = match current {
                    Mode::Normal
                    | Mode::Insert
                    | Mode::Visual
                    | Mode::Replace
                    | Mode::Cmd => current,
                    _ => Mode::Normal,
                };
                let primary = match &target {
                    Some(BufferTabTarget::File(path)) => path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "(no file)".to_string()),
                    _ => "(no file)".to_string(),
                };
                (mode, PrimaryKind::File, primary)
            } else {
                // Terminal tab (targetless) or any other fallback.
                let primary = self
                    .status_line
                    .info()
                    .cwd_label
                    .clone()
                    .unwrap_or_else(|| "Terminal".to_string());
                (Mode::Terminal, PrimaryKind::Terminal, primary)
            };

        let info = self.status_line.info();
        if info.mode == mode
            && info.primary_kind == primary_kind
            && info.primary == primary
        {
            return;
        }
        let mut next = info.clone();
        next.mode = mode;
        next.primary_kind = primary_kind;
        next.primary = primary;
        self.status_line.set_info(next);
    }

    /// Push the plain-text content for the currently-active tab. The
    /// chrome paints it inside the terminal rect when the active tab
    /// is non-Terminal. Pass `None` to clear.
    pub fn set_tab_lang(&mut self, lang: crate::syntax::Lang) {
        self.tab_lang = lang;
    }

    pub fn tab_lang(&self) -> crate::syntax::Lang {
        self.tab_lang
    }

    pub fn set_tab_content(&mut self, text: Option<String>) {
        self.tab_content = text;
        self.scroll_offset_px = 0.0;
        self.scroll_spring.reset();
        if self.is_terminal_tab_active() {
            self.editor_scroll_render_state = None;
            self.editor_scrollback_origin = None;
            self.editor_scrollback_above_rows.clear();
            self.editor_scrollback_below_rows.clear();
            self.last_editor_trail_cursor_cell = None;
        }
    }

    pub fn tab_content(&self) -> Option<&str> {
        self.tab_content.as_deref()
    }

    pub fn set_terminal_input(&mut self, text: String) {
        self.terminal_input.set_text(text);
    }

    pub fn set_terminal_input_snapshot(
        &mut self,
        text: String,
        cursor_byte: usize,
        completion_items: Vec<String>,
    ) {
        self.terminal_input
            .set_snapshot(text, cursor_byte, completion_items);
    }

    pub fn clear_terminal_input(&mut self) {
        self.terminal_input.clear();
    }

    pub fn terminal_input(&self) -> &str {
        self.terminal_input.text()
    }

    pub fn dismiss_terminal_splash(&mut self) {
        self.terminal_splash_dismissed = true;
    }

    pub fn reset_terminal_splash(&mut self) {
        self.terminal_splash_dismissed = false;
        self.splash_overlay.reset();
    }

    pub fn terminal_splash_dismissed(&self) -> bool {
        self.terminal_splash_dismissed
    }

    /// Seed the lazily-constructed `MarkdownPane` with the current
    /// `.md` tab's source. The bridge calls this whenever it pushes
    /// content for a markdown path; on `None` the pane is dropped so
    /// the next non-`.md` tab paints with the plain syntax loop.
    ///
    /// `path_hint` is only used to derive a title — it does not need
    /// to point at a real on-disk file (the wasm chrome has no
    /// filesystem). Pass `None` to use a generic title.
    pub fn set_markdown_content(
        &mut self,
        text: Option<String>,
        path_hint: Option<&str>,
    ) {
        use crate::editor::markdown::MarkdownPane;
        use std::path::PathBuf;
        match text {
            Some(src) => {
                let path = path_hint
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("untitled.md"));
                match self.markdown_pane.as_mut() {
                    Some(pane) => {
                        pane.path = path;
                        pane.set_source(&src);
                    }
                    None => {
                        self.markdown_pane = Some(MarkdownPane::from_source(path, &src));
                    }
                }
            }
            None => {
                self.markdown_pane = None;
            }
        }
    }

    /// Wave 7-web: feed remote collaborators' carets into the live
    /// markdown pane so the shared renderer draws them (colored bar +
    /// name flag + roster), same as desktop. No-op without a pane.
    pub fn set_markdown_remote_cursors(
        &mut self,
        cursors: Vec<crate::editor::markdown::MarkdownRemoteCursor>,
    ) {
        if let Some(pane) = self.markdown_pane.as_mut() {
            pane.remote_cursors = cursors;
        }
    }

    /// 7C-2: replace the editor grid's remote caret cues (screen
    /// rows; host converts from buffer lines via the viewport topline).
    /// Current editor grid dims (cols, rows) — diagnostics.
    pub fn editor_grid_dims(&self) -> Option<(u32, u32)> {
        self.editor_grid
            .as_ref()
            .map(|grid| (grid.width, grid.height))
    }

    pub fn set_editor_viewport_topline(&mut self, topline: u64) {
        self.editor_caret_topline = topline;
    }

    pub fn set_editor_remote_carets(
        &mut self,
        cues: Vec<crate::panels::remote_carets::EditorRemoteCaret>,
        roster: Vec<crate::panels::remote_carets::EditorRemoteCaret>,
    ) {
        self.editor_remote_carets = cues;
        self.editor_remote_roster = roster;
    }

    /// Wave 7-web: mutable handle to the live markdown pane so the host
    /// bridge can route input (wheel, clicks, cursor keys) into it —
    /// the pane owns its own scroll/cursor state.
    pub fn markdown_pane_mut(
        &mut self,
    ) -> Option<&mut crate::editor::markdown::MarkdownPane> {
        self.markdown_pane.as_mut()
    }

    /// Push the latest nvim grid snapshot for the active file-viewer
    /// tab. `Some` enables the grid-cell paint branch in
    /// [`Chrome::draw`]; `None` falls back to the cached
    /// `tab_content` + syntax-highlight paint so file-viewer tabs
    /// without an attached nvim session still render.
    pub fn set_editor_grid(
        &mut self,
        snapshot: Option<crate::editor_snapshot::EditorGridSnapshot>,
    ) {
        let dimensions_changed = match (&self.editor_grid, &snapshot) {
            (Some(prev), Some(next)) => {
                prev.width != next.width
                    || prev.height != next.height
                    || prev.cells.len() != next.cells.len()
            }
            _ => false,
        };

        if snapshot.is_none() || dimensions_changed {
            self.editor_grid_scrollback = None;
            self.editor_scroll_render_state = None;
            self.editor_scrollback_origin = None;
            self.editor_scrollback_above_rows.clear();
            self.editor_scrollback_below_rows.clear();
            self.last_editor_trail_cursor_cell = None;
        }
        if snapshot.is_some() && self.editor_scrollback_origin.is_none() {
            self.editor_scrollback_origin = Some(0);
        }
        self.editor_grid = snapshot;
    }

    pub fn reset_editor_grid_scroll_render_state(&mut self) {
        self.editor_scroll_render_state = None;
        self.editor_scrollback_origin = None;
        self.editor_scrollback_above_rows.clear();
        self.editor_scrollback_below_rows.clear();
        self.last_editor_trail_cursor_cell = None;
        self.editor_grid_scrollback = None;
        self.editor_scroll.reset_all();
    }

    pub fn prime_editor_grid_scrollback(
        &mut self,
        snapshot: crate::editor_snapshot::EditorGridSnapshot,
    ) {
        self.editor_grid_scrollback = Some(snapshot);
    }

    pub fn prime_editor_grid_scrollback_for_scroll(
        &mut self,
        snapshot: crate::editor_snapshot::EditorGridSnapshot,
        top: u32,
        bot: u32,
        rows: i32,
    ) {
        const MAX_EDGE_ROWS: usize = 64;
        match snapshot.capture_grid_scroll_edge_rows(
            top,
            bot,
            rows,
            MAX_EDGE_ROWS,
            &mut self.editor_scrollback_above_rows,
            &mut self.editor_scrollback_below_rows,
        ) {
            GridScrollEdgeCapture::Captured => {
                let origin = self.editor_scrollback_origin.unwrap_or(0);
                self.editor_scrollback_origin =
                    Some(origin.saturating_add(rows as isize));
            }
            GridScrollEdgeCapture::NoScroll => {}
            GridScrollEdgeCapture::Invalid => {
                self.editor_scroll_render_state = None;
                self.editor_scrollback_origin = None;
            }
        }

        self.editor_grid_scrollback = Some(snapshot);
    }

    /// Read-only accessor for the stashed grid snapshot. Mainly for
    /// tests and the bridge to verify it parsed the daemon JSON before
    /// painting.
    pub fn editor_grid(&self) -> Option<&crate::editor_snapshot::EditorGridSnapshot> {
        self.editor_grid.as_ref()
    }

    pub fn set_editor_cursor_shape(
        &mut self,
        shape: neoism_terminal_core::ansi::CursorShape,
    ) {
        self.editor_cursor_shape = shape;
    }

    pub fn push_editor_viewport_scroll(&mut self, rows: i32, viewport_rows: usize) {
        self.editor_scroll.add_grid_scroll(
            EDITOR_GRID_SCROLL_ID,
            rows,
            self.cell_h.max(1.0),
            viewport_rows.max(1),
        );
    }

    pub fn push_editor_wheel_delta(
        &mut self,
        delta_pixels: f32,
        cell_height: f32,
    ) -> i32 {
        self.editor_scroll.add_wheel_delta(
            EDITOR_GRID_SCROLL_ID,
            delta_pixels,
            cell_height.max(1.0),
        )
    }

    pub fn push_editor_wheel_delta_with_viewport_bounds(
        &mut self,
        delta_pixels: f32,
        cell_height: f32,
        bounds: Option<EditorScrollViewportBounds>,
    ) -> i32 {
        if bounds.is_some_and(|bounds| bounds.rejects_delta(delta_pixels)) {
            self.reset_editor_wheel();
            self.push_editor_elastic(-delta_pixels, cell_height);
            return 0;
        }
        self.push_editor_wheel_delta(delta_pixels, cell_height)
    }

    pub fn tick_editor_wheel(&mut self, cell_height: f32) -> i32 {
        self.editor_scroll
            .tick_wheel(EDITOR_GRID_SCROLL_ID, cell_height.max(1.0))
    }

    pub fn tick_editor_wheel_with_viewport_bounds(
        &mut self,
        cell_height: f32,
        bounds: Option<EditorScrollViewportBounds>,
    ) -> i32 {
        let rows = self.tick_editor_wheel(cell_height);
        if bounds.is_some_and(|bounds| bounds.rejects_delta(rows as f32)) {
            self.reset_editor_wheel();
            return 0;
        }
        rows
    }

    pub fn reset_editor_wheel(&mut self) {
        self.editor_scroll.reset_wheel(EDITOR_GRID_SCROLL_ID);
    }

    pub fn push_editor_elastic(&mut self, direction_pixels: f32, cell_height: f32) {
        self.editor_scroll.push_elastic(
            EDITOR_GRID_SCROLL_ID,
            direction_pixels,
            cell_height.max(1.0),
        );
    }

    pub(crate) fn editor_grid_render_state(
        &self,
        cell_h: f32,
    ) -> EditorScrollGridRenderState {
        let offset = editor_scroll_render_offset_for_mutated_snapshot(
            self.editor_scroll
                .current_scroll_offset(EDITOR_GRID_SCROLL_ID),
            self.editor_scroll
                .current_elastic_offset(EDITOR_GRID_SCROLL_ID),
            cell_h,
            self.editor_scroll_render_state
                .map(|state| state.source_line_offset),
        );
        EditorScrollGridRenderState::new(offset, self.editor_scrollback_origin)
    }

    pub(crate) fn remember_editor_grid_render_state(
        &mut self,
        state: EditorScrollGridRenderState,
    ) {
        self.editor_scroll_render_state = Some(state);
    }

    pub fn editor_grid_hit_cell(&self, x: f32, y: f32) -> Option<(u32, u32)> {
        let grid = self.editor_grid.as_ref()?;
        let cell_w = (self.layout.terminal.w / grid.width.max(1) as f32).max(1.0);
        let cell_h = (self.layout.terminal.h / grid.height.max(1) as f32).max(1.0);
        let scroll_state = self.editor_grid_render_state(cell_h);
        editor_grid_hit_cell(
            x,
            y,
            [
                self.layout.terminal.x,
                self.layout.terminal.y,
                self.layout.terminal.w,
                self.layout.terminal.h,
            ],
            grid.width,
            grid.height,
            cell_w,
            cell_h,
            scroll_state,
        )
        .map(|hit| (hit.row, hit.col))
    }

    pub fn editor_grid_scroll_animating(&self) -> bool {
        self.editor_scroll.is_animating()
    }

    pub fn animations_active(&self) -> bool {
        self.editor_scroll.is_animating()
            || self.rainbow_cursor_active()
            || self.trail_cursor.is_animating()
            || self.yank_flash.is_animating()
            || self.cursorline_overlay.is_animating()
            || self.status_line.is_animating()
            || self.buffer_tabs.is_animating()
            || self
                .file_tree
                .as_ref()
                .is_some_and(|tree| tree.is_animating())
            || self.command_palette.is_animating()
            || self.command_composer.is_animating(&self.terminal_input)
            || self.completion_menu.is_animating()
            || self.diagnostics_popup.is_animating()
            || self.notifications.is_active()
            || self.splash_overlay.is_animating()
            || self
                .agent_pane
                .as_ref()
                .is_some_and(|pane| pane.is_animating())
            || (self.git_diff_panel.is_visible() && self.git_diff_panel.needs_redraw())
    }
}
