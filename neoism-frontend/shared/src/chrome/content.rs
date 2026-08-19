use super::*;

use crate::panels::buffer_tabs::BufferTabTarget;

/// Which hosted editor pane serves the active buffer tab. Mirrors the
/// desktop Context's mutually-exclusive pane slots (`code` / `notebook`
/// / `draw`) for the web chrome, which hosts one pane at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EditorPaneKind {
    Code,
    Notebook,
    Draw,
}

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
        // Hosted editor pane cursor for the right-cluster ruler pill.
        // `None` leaves whatever the host last pushed.
        let mut editor_cursor: Option<(usize, usize)> = None;
        let (mode, primary_kind, primary): (Mode, PrimaryKind, String) =
            if self.is_neoism_agent_tab_active() {
                (Mode::Agent, PrimaryKind::Agent, "Neoism".to_string())
            } else if let Some(page) = self.active_chrome_page() {
                // Chrome helper page (Extensions / NeoWorld) — a
                // Rust-painted surface with no vi mode; show its title
                // as the primary label.
                (Mode::Normal, PrimaryKind::File, page.title().to_string())
            } else if let Some(kind) = self.active_editor_pane_kind() {
                // Hosted code / notebook / draw pane — mode comes from
                // the live pane, mirroring the desktop's status sync.
                match kind {
                    EditorPaneKind::Code => {
                        let pane = self.code_pane.as_ref().expect("kind checked");
                        let mode = match pane.buffer.mode {
                            crate::editor::code::CodeMode::Insert => Mode::Insert,
                            crate::editor::code::CodeMode::Visual => Mode::Visual,
                            crate::editor::code::CodeMode::Normal => Mode::Normal,
                        };
                        let cursor = pane.buffer.cursor();
                        editor_cursor = Some((cursor.line + 1, cursor.col + 1));
                        (mode, PrimaryKind::File, pane.title.clone())
                    }
                    EditorPaneKind::Notebook => {
                        let pane = self.notebook_pane.as_ref().expect("kind checked");
                        let mode = match pane.markdown.mode {
                            MarkdownMode::Insert => Mode::Insert,
                            MarkdownMode::Visual => Mode::Visual,
                            MarkdownMode::Normal => Mode::Markdown,
                        };
                        let primary = pane
                            .path
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_else(|| "Notebook".to_string());
                        (mode, PrimaryKind::File, primary)
                    }
                    EditorPaneKind::Draw => {
                        let pane = self.draw_pane.as_ref().expect("kind checked");
                        (Mode::Normal, PrimaryKind::File, pane.title.clone())
                    }
                }
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
        let cursor_lines = match editor_cursor {
            Some(cursor) => Some(cursor),
            None => info.cursor_lines,
        };
        if info.mode == mode
            && info.primary_kind == primary_kind
            && info.primary == primary
            && info.cursor_lines == cursor_lines
        {
            return;
        }
        let mut next = info.clone();
        next.mode = mode;
        next.primary_kind = primary_kind;
        next.primary = primary;
        next.cursor_lines = cursor_lines;
        self.status_line.set_info(next);
    }

    /// Replace the host-declared per-pane surface descriptors (what
    /// each visible pane leaf displays). See
    /// [`crate::chrome::PaneSurfaceInfo`]; drives the unfocused-pane
    /// render pass while the grid is split.
    pub fn set_pane_surfaces(&mut self, surfaces: Vec<crate::chrome::PaneSurfaceInfo>) {
        self.pane_surfaces = surfaces;
    }

    /// Record which panes the HOST painted this frame (live terminal
    /// grids) so the chrome's unfocused-pane pass leaves them alone.
    /// Hosts call this every frame BEFORE [`Chrome::draw`].
    pub fn set_host_drawn_panes(&mut self, panes: Vec<u64>) {
        self.host_drawn_panes = panes;
    }

    /// Content rect the ACTIVE surface should paint into: the focused
    /// pane's content rect (after its per-pane chrome reservations)
    /// while the pane grid is split, else the whole terminal rect.
    pub fn focused_content_rect(&self) -> crate::layout::Rect {
        if self.pane_grid.is_split() {
            if let Some(pane) = self.pane_grid.panes().iter().find(|p| p.focused) {
                if let Some(content) = pane
                    .external_id
                    .and_then(|ext| self.pane_content_rect(ext))
                {
                    return content;
                }
                return pane.rect;
            }
        }
        self.layout.terminal
    }

    /// Content rect (after per-pane chrome reservations) of the pane
    /// bound to `external_id`, while the grid is split.
    pub fn pane_content_rect(&self, external_id: u64) -> Option<crate::layout::Rect> {
        self.layout
            .panes
            .iter()
            .find(|pane| pane.external_id == external_id)
            .map(|pane| pane.content)
    }

    /// Replace one pane's local tab strip (desktop `pane_tabs` twin).
    /// An empty `tabs` list drops the strip + its breadcrumbs. The
    /// breadcrumbs row derives from the strip's active tab path.
    pub fn set_pane_tabs(
        &mut self,
        external_id: u64,
        tabs: Vec<crate::panels::buffer_tabs::BufferTab<A>>,
        active: usize,
    ) {
        if tabs.is_empty() {
            self.pane_tabs.remove(&external_id);
            self.pane_breadcrumbs.remove(&external_id);
            self.relayout();
            return;
        }
        let chrome_scale = self.chrome_scale;
        let strip = self.pane_tabs.entry(external_id).or_insert_with(|| {
            let mut strip = crate::panels::BufferTabs::<A>::new();
            strip.set_visible(true);
            strip
        });
        strip.set_scale(chrome_scale);
        strip.set_visible(true);
        let active = active.min(tabs.len().saturating_sub(1));
        strip.set_tabs(tabs, active);
        let active_path = strip.active_path().map(|p| p.to_path_buf());
        match active_path {
            Some(path) => {
                let root = self.workspace_root_path.clone();
                let crumbs =
                    self.pane_breadcrumbs.entry(external_id).or_insert_with(|| {
                        crate::panels::breadcrumbs::Breadcrumbs::new()
                    });
                crumbs.set_scale(chrome_scale);
                crumbs.set_from_path(&path, root.as_deref());
            }
            None => {
                self.pane_breadcrumbs.remove(&external_id);
            }
        }
        self.relayout();
    }

    /// Drop pane strips (and breadcrumbs) whose panes went away.
    pub fn retain_pane_tabs(&mut self, keep: &[u64]) {
        let before =
            self.pane_tabs.len() + self.pane_breadcrumbs.len();
        self.pane_tabs.retain(|id, _| keep.contains(id));
        self.pane_breadcrumbs.retain(|id, _| keep.contains(id));
        if before != self.pane_tabs.len() + self.pane_breadcrumbs.len() {
            self.relayout();
        }
    }

    /// Hit-test the per-pane tab strips at a window point. Returns the
    /// pane external id + the strip-local [`TabHit`], or `None` when
    /// the point misses every pane strip.
    pub fn pane_strip_hit(
        &self,
        x: f32,
        y: f32,
    ) -> Option<(u64, crate::panels::buffer_tabs::TabHit)> {
        for pane in &self.layout.panes {
            let Some(rect) = pane.tabs else {
                continue;
            };
            if !rect.contains(x, y) {
                continue;
            }
            let hit = self
                .pane_tabs
                .get(&pane.external_id)
                .and_then(|strip| strip.hit_test(x, y, rect.x, rect.y, rect.w))?;
            return Some((pane.external_id, hit));
        }
        None
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

    /// Seed the hosted editor pane for a non-markdown file tab,
    /// routing by file type exactly like the desktop's context
    /// factories: `.ipynb` → NotebookPane, `.neodraw` → DrawPane,
    /// everything else text-like → CodePane. Re-calling with the SAME
    /// path keeps the live pane (cursor, undo, unsaved edits survive a
    /// tab round-trip); a clean pane whose on-disk content changed is
    /// re-seeded. Returns which pane kind now serves the tab.
    pub fn open_editor_file(
        &mut self,
        tab_index: usize,
        path: &str,
        text: &str,
    ) -> EditorPaneKind {
        use std::path::{Path, PathBuf};
        let path_ref = Path::new(path);
        self.editor_pane_tab = Some(tab_index);
        let kind = if crate::editor::notebook::is_notebook_path(path_ref) {
            EditorPaneKind::Notebook
        } else if crate::editor::neodraw::is_neodraw_path(path_ref) {
            EditorPaneKind::Draw
        } else {
            EditorPaneKind::Code
        };

        // Park panes leaving the hosted slots (different kind or
        // different path) so their cursor/undo/unsaved edits survive
        // the tab round-trip — the web twin of desktop's per-tab
        // panes. The code pane's doc binding is dropped with the
        // slot; a restore re-binds through the crdt pump.
        if let Some(pane) = self.code_pane.take() {
            if kind == EditorPaneKind::Code && pane.path == path_ref {
                self.code_pane = Some(pane);
            } else {
                self.code_doc_binding = None;
                self.parked_code_panes.insert(pane.path.clone(), pane);
            }
        }
        if let Some(pane) = self.notebook_pane.take() {
            if kind == EditorPaneKind::Notebook && pane.path == path_ref {
                self.notebook_pane = Some(pane);
            } else {
                self.parked_notebook_panes.insert(pane.path.clone(), pane);
            }
        }
        if let Some(pane) = self.draw_pane.take() {
            if kind == EditorPaneKind::Draw && pane.path == path_ref {
                self.draw_pane = Some(pane);
            } else {
                self.parked_draw_panes.insert(pane.path.clone(), pane);
            }
        }

        match kind {
            EditorPaneKind::Notebook => {
                if self.notebook_pane.is_none() {
                    self.notebook_pane = self.parked_notebook_panes.remove(path_ref);
                }
                // Reseed only a CLEAN pane whose fetched JSON differs
                // (external change) — a dirty pane keeps its edits.
                let reseed = match self.notebook_pane.as_ref() {
                    Some(pane) => {
                        !pane.is_dirty()
                            && pane.to_json().map(|json| json != text).unwrap_or(true)
                    }
                    None => true,
                };
                if reseed {
                    use crate::editor::notebook::{NotebookDocument, NotebookPane};
                    self.notebook_pane =
                        Some(match NotebookDocument::from_json(text) {
                            Ok(document) => NotebookPane::from_document(
                                PathBuf::from(path),
                                document,
                                text.to_string(),
                                None,
                            ),
                            Err(err) => NotebookPane::error(PathBuf::from(path), err),
                        });
                }
            }
            EditorPaneKind::Draw => {
                if self.draw_pane.is_none() {
                    self.draw_pane = self.parked_draw_panes.remove(path_ref);
                }
                let reseed = match self.draw_pane.as_ref() {
                    Some(pane) => !pane.is_dirty() && pane.to_source() != text,
                    None => true,
                };
                if reseed {
                    self.draw_pane =
                        Some(crate::editor::neodraw::DrawPane::from_source(
                            PathBuf::from(path),
                            text,
                        ));
                }
            }
            EditorPaneKind::Code => {
                if self.code_pane.is_none() {
                    if let Some(pane) = self.parked_code_panes.remove(path_ref) {
                        // A restored pane needs a fresh doc binding
                        // (the old one died with its slot residency).
                        self.code_doc_binding = None;
                        self.code_pane = Some(pane);
                    }
                }
                match self.code_pane.as_mut() {
                    Some(pane) => {
                        // Only fold fresh content into a CLEAN buffer
                        // whose file actually changed — clobbering a
                        // dirty buffer would eat the user's edits.
                        if !pane.is_dirty() && pane.buffer.text_for_disk() != text {
                            pane.apply_remote_source(text);
                            self.code_doc_binding = None;
                        }
                    }
                    None => {
                        self.code_pane = Some(crate::editor::code::CodePane::new(
                            PathBuf::from(path),
                            text,
                        ));
                        self.code_doc_binding = None;
                    }
                }
            }
        }
        kind
    }

    /// Which hosted editor pane serves the ACTIVE tab, if any. `None`
    /// while the active tab is a terminal / markdown / agent surface,
    /// or when the hosted pane belongs to a different tab.
    ///
    /// Also `None` while a full-screen chrome overlay (settings /
    /// modal) is up: the overlay owns the surface, so editor key +
    /// pointer routes (which consult this) stand down instead of
    /// mutating the hidden pane underneath.
    pub fn active_editor_pane_kind(&self) -> Option<EditorPaneKind> {
        if self.chrome_overlay_active() {
            return None;
        }
        if self.editor_pane_tab != Some(self.active_tab_index) {
            return None;
        }
        if self.code_pane.is_some() {
            Some(EditorPaneKind::Code)
        } else if self.notebook_pane.is_some() {
            Some(EditorPaneKind::Notebook)
        } else if self.draw_pane.is_some() {
            Some(EditorPaneKind::Draw)
        } else {
            None
        }
    }

    /// Drop every hosted editor pane (and the code pane's CRDT
    /// binding). The host calls this when the surface goes away for
    /// good (e.g. the backing tab closed).
    pub fn close_editor_panes(&mut self) {
        self.code_pane = None;
        self.notebook_pane = None;
        self.draw_pane = None;
        self.code_doc_binding = None;
        self.editor_pane_tab = None;
        self.editor_pane_animating = false;
        self.parked_code_panes.clear();
        self.parked_notebook_panes.clear();
        self.parked_draw_panes.clear();
    }

    pub fn code_pane_mut(&mut self) -> Option<&mut crate::editor::code::CodePane> {
        self.code_pane.as_mut()
    }

    pub fn code_pane(&self) -> Option<&crate::editor::code::CodePane> {
        self.code_pane.as_ref()
    }

    pub fn notebook_pane_mut(
        &mut self,
    ) -> Option<&mut crate::editor::notebook::NotebookPane> {
        self.notebook_pane.as_mut()
    }

    pub fn notebook_pane(&self) -> Option<&crate::editor::notebook::NotebookPane> {
        self.notebook_pane.as_ref()
    }

    pub fn draw_pane_mut(&mut self) -> Option<&mut crate::editor::neodraw::DrawPane> {
        self.draw_pane.as_mut()
    }

    pub fn draw_pane(&self) -> Option<&crate::editor::neodraw::DrawPane> {
        self.draw_pane.as_ref()
    }

    /// Simultaneous mutable access to the hosted code pane and its CRDT
    /// binding slot — the wasm bridge's crdt pump needs both at once
    /// (mirrors how the markdown binding + pane pair is driven).
    #[allow(clippy::type_complexity)]
    pub fn code_editor_parts_mut(
        &mut self,
    ) -> (
        Option<&mut crate::editor::code::CodePane>,
        &mut Option<crate::editor::code::doc_sync::CodeDocBinding>,
    ) {
        (self.code_pane.as_mut(), &mut self.code_doc_binding)
    }

    pub fn animations_active(&self) -> bool {
        self.editor_pane_animating
            || self.chrome_pages_animating()
            || self.rainbow_cursor_active()
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
