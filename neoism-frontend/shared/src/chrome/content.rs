use super::*;

use crate::panels::buffer_tabs::BufferTabTarget;

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
    /// For File surfaces, the vi mode (`Normal`/`Insert`/…) is a
    /// host-pushed signal — the web host can push it via the
    /// `set_status_mode_*` bridge setters. We preserve
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
            } else if matches!(target, Some(BufferTabTarget::File(_))) {
                // Code-backed file viewer. The vi mode is host-driven;
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

    /// Wave 7-web: mutable handle to the live markdown pane so the host
    /// bridge can route input (wheel, clicks, cursor keys) into it —
    /// the pane owns its own scroll/cursor state.
    pub fn markdown_pane_mut(
        &mut self,
    ) -> Option<&mut crate::editor::markdown::MarkdownPane> {
        self.markdown_pane.as_mut()
    }

    pub fn animations_active(&self) -> bool {
        self.rainbow_cursor_active()
            || self.trail_cursor.is_animating()
            || self.yank_flash.is_animating()
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
