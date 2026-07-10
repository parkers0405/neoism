use super::*;

use web_time::Duration;

use sugarloaf::Sugarloaf;

use crate::chrome_policy::TrailCursorOverlayTarget;
use crate::event::{KeyState, LogicalKey, Modifiers, NamedKey, UiEvent, WheelMode};
use crate::layout::{ChromeLayout, Rect};
use crate::panels::buffer_tabs::BUFFER_TABS_HEIGHT;
use crate::panels::status_line::STATUS_LINE_HEIGHT;

use crate::panels::git_diff::PanelHit as GitPanelHit;
use crate::panels::notes_sidebar::NotesSidebarHit;
use crate::panels::{Panel, PanelContext};
use crate::services::Services;

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Recompute every panel's rect against `viewport`. The viewport
    /// is the full window content area in logical pixels (top-left
    /// origin). Panels that are currently hidden (modals) still get
    /// their rect resolved here so showing them is a state flip, not
    /// a layout pass.
    pub fn set_layout(&mut self, viewport: Rect) {
        self.last_viewport = Some(viewport);
        let scale = self.chrome_scale.clamp(0.5, 3.0);
        let tabs_h = BUFFER_TABS_HEIGHT * scale;
        let status_h = STATUS_LINE_HEIGHT * scale;

        // Top bar spans the full viewport width, pinned to the top
        // edge above everything else (its rect is built below). The
        // side panels (tree / notes / git) are confined to the band
        // beneath the top chrome rather than running the full window
        // height, so they no longer push the top bar / tabs inward.
        // Right-edge toggle is only useful when an agent tab is the
        // current target, so flag the bar's right button accordingly
        // before the layout pass reads the bar's reservation.
        self.top_bar
            .set_right_button_visible(self.is_neoism_agent_tab_active());
        let top_bar_h = if self.top_bar.is_visible() {
            self.top_bar.layout_reservation()
        } else {
            0.0
        };

        // === Full-width top chrome ===
        // Only the top bar and the workspace island strip span the
        // entire viewport width, pinned to the top edge. The buffer
        // tabs / breadcrumbs below them stay scoped to the content
        // column so the file tree (and the other side panels) push
        // them inward, exactly as before.
        let top_bar_rect = if self.top_bar.is_visible() {
            Some(Rect::new(viewport.x, viewport.y, viewport.w, top_bar_h))
        } else {
            None
        };
        // Workspace island strip sits directly under the top bar. The
        // side-panel band (tree / notes / git / tabs / terminal) begins
        // right below it.
        let strip_top = top_bar_rect.map(|r| r.y + r.h).unwrap_or(viewport.y);
        let band_top = strip_top + self.top_workspace_strip_h;

        // === Full-width status bar ===
        // Status line spans the entire width along the bottom edge; the
        // side panels stop at its top rather than running underneath.
        let status_line = Rect::new(
            viewport.x,
            viewport.y + viewport.h - status_h,
            viewport.w,
            status_h,
        );
        let band_bottom = status_line.y;
        let band_h = (band_bottom - band_top).max(0.0);

        // Sidebar column for the file tree — spans the middle band
        // (between the full-width top chrome and the full-width status
        // bar). The tree is installed via `install_file_tree` but its
        // visibility is per-frame via `FileTree::is_visible` — when
        // closed the slot returns `None` and the content column reclaims
        // the full band width. Native toggles this with Ctrl+Shift+B.
        let file_tree_rect =
            self.file_tree
                .as_ref()
                .filter(|t| t.is_visible())
                .map(|tree| {
                    Rect::new(viewport.x, band_top, tree.width().min(viewport.w), band_h)
                });

        // Small gap between the tree's right edge and the content
        // column so the composer chassis / status pill rounded corners
        // don't overhang into the tree column.
        const TREE_CONTENT_GAP: f32 = 4.0;
        // Notes sidebar docks right of the file tree (desktop parity:
        // both columns can be open at once). It renders itself from
        // these same inputs in `draw`, so layout only needs its width.
        let notes_w = if self.notes_sidebar.is_visible() {
            self.notes_sidebar.width().min(viewport.w * 0.8)
        } else {
            0.0
        };
        let content_x = match file_tree_rect {
            Some(ft) => ft.x + ft.w + notes_w + TREE_CONTENT_GAP,
            None if notes_w > 0.0 => viewport.x + notes_w + TREE_CONTENT_GAP,
            None => viewport.x,
        };
        // Rich git side panel reserves a right column in the middle
        // band while visible — the content column must not paint
        // underneath it (chrome reflow, not z-order).
        let right_inset = self.git_diff_panel.effective_width(viewport.w);
        let content_w = (viewport.x + viewport.w - right_inset) - content_x;

        // Buffer tabs — top of the content column, pushed inward by the
        // tree / notes (left) and git panel (right).
        let buffer_tabs = Rect::new(content_x, band_top, content_w, tabs_h);
        let breadcrumbs = self.buffer_tabs.active_shows_breadcrumbs().then(|| {
            Rect::new(
                content_x,
                buffer_tabs.y + buffer_tabs.h,
                content_w,
                self.breadcrumbs.height(),
            )
        });

        let composer_h =
            if self.command_composer.is_visible() && self.is_terminal_tab_active() {
                let pane_rows = ((status_line.y - band_top) / self.cell_h.max(1.0))
                    .floor()
                    .max(0.0) as usize;
                let raw_h = self.command_composer.actual_chassis_height_for_input(
                    self.cell_h.max(1.0),
                    content_w,
                    self.cell_w.max(1.0),
                    pane_rows,
                    self.terminal_input.text(),
                );
                let top_pad = crate::panels::command_composer::COMPOSER_TOP_OVERHANG
                    * self.command_composer.scale();
                (raw_h - top_pad).max(raw_h * 0.5)
            } else {
                COMMAND_COMPOSER_HEIGHT * scale
            };

        // Sticky composer docks just above the status line when shown,
        // and is confined to the live terminal tab. File/nvim/agent
        // tabs own their whole content rect and should not inherit the
        // terminal command bar.
        let composer_rect =
            if self.command_composer.is_visible() && self.is_terminal_tab_active() {
                Some(Rect::new(
                    content_x,
                    status_line.y - composer_h,
                    content_w,
                    composer_h,
                ))
            } else {
                None
            };

        // Remaining center rect: the terminal canvas fills the content
        // column below the tabs / breadcrumbs. Composer eats a slice off
        // the bottom when visible.
        let terminal_top = breadcrumbs
            .map(|rect| rect.y + rect.h)
            .unwrap_or(buffer_tabs.y + buffer_tabs.h);
        let terminal_bottom = match composer_rect {
            Some(c) => c.y,
            None => status_line.y,
        };
        let terminal = Rect::new(
            content_x,
            terminal_top,
            content_w.max(0.0),
            (terminal_bottom - terminal_top).max(0.0),
        );

        // Modal overlays: centered cards. Only assign a rect when the
        // underlying panel is currently visible — `None` for hidden.
        let center_modal = |w: f32, h: f32| {
            let w = w.min(content_w);
            let h = h.min(viewport.h);
            let x = content_x + (content_w - w) * 0.5;
            let y = viewport.y + (viewport.h - h) * 0.25;
            Rect::new(x, y, w, h)
        };

        let command_palette = self
            .command_palette
            .is_visible()
            .then(|| center_modal(MODAL_WIDTH, MODAL_HEIGHT));
        let finder = self
            .finder
            .is_visible()
            .then(|| center_modal(MODAL_WIDTH, MODAL_HEIGHT));
        // Git diff is a full-window overlay rather than a centered
        // card — it needs the room for two columns of hunks.
        let git_diff = self.git_diff.is_visible().then(|| viewport);

        self.layout = ChromeLayout {
            top_bar: top_bar_rect,
            file_tree: file_tree_rect,
            buffer_tabs,
            breadcrumbs,
            status_line,
            terminal,
            command_palette,
            finder,
            git_diff,
            command_composer: composer_rect,
        };
    }

    /// Push a panel onto the focus stack. Idempotent: pushing a
    /// panel that is already top-of-stack is a no-op. Pushing a
    /// panel that is somewhere below the top moves it to the top.
    pub fn focus(&mut self, key: PanelKey) {
        if self.focus_stack.last() == Some(&key) {
            return;
        }
        self.focus_stack.retain(|k| *k != key);
        self.focus_stack.push(key);
    }

    /// Remove the top of the focus stack. Returns the popped key if
    /// there was one.
    pub fn pop_focus(&mut self) -> Option<PanelKey> {
        self.focus_stack.pop()
    }

    /// Remove a panel from the focus stack, wherever it currently is.
    pub fn blur(&mut self, key: PanelKey) {
        self.focus_stack.retain(|k| *k != key);
    }

    /// Current top of the focus stack, if any.
    pub fn focused(&self) -> Option<PanelKey> {
        self.focus_stack.last().copied()
    }

    /// Return chrome focus to the editor/terminal content surface.
    /// Web uses this when a click or wheel gesture lands in the nvim
    /// grid; otherwise a previously focused tab strip can keep
    /// swallowing editor keys after the user has clearly returned to
    /// the buffer.
    pub fn focus_content_surface(&mut self) {
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
        self.buffer_tabs.set_focused(false);
        self.blur(PanelKey::FileTree);
        self.blur(PanelKey::BufferTabs);
        self.blur(PanelKey::CommandComposer);
    }

    pub(crate) fn chrome_trail_cursor_rect(
        &self,
        target: TrailCursorOverlayTarget,
        tab_cursor_rect: Option<[f32; 4]>,
    ) -> Option<[f32; 4]> {
        match target {
            TrailCursorOverlayTarget::Finder => self.finder.selected_cursor_rect(),
            TrailCursorOverlayTarget::CommandPalette => {
                self.command_palette.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::ContextMenu => {
                self.context_menu.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::FileTree => self
                .file_tree
                .as_ref()
                .and_then(|tree| tree.selected_cursor_rect()),
            TrailCursorOverlayTarget::NotesSidebar => {
                self.notes_sidebar.selected_cursor_rect()
            }
            TrailCursorOverlayTarget::AgentSidePanel => {
                if !self.is_neoism_agent_tab_active() {
                    return None;
                }
                self.agent_pane
                    .as_ref()
                    .and_then(|pane| pane.side_panel().selected_cursor_rect())
            }
            TrailCursorOverlayTarget::Tabs => tab_cursor_rect,
            TrailCursorOverlayTarget::GitDiffPanel => self
                .git_diff_panel
                .selected_cursor_rect()
                .or_else(|| self.git_diff.selected_cursor_rect()),
            TrailCursorOverlayTarget::AgentInput => {
                if !self.is_neoism_agent_tab_active() {
                    return None;
                }
                self.agent_pane.as_ref().and_then(|pane| pane.cursor_rect())
            }
            TrailCursorOverlayTarget::TerminalBlockInput => {
                self.command_composer.last_frame().caret_rect
            }
            _ => None,
        }
    }

    pub(crate) fn draw_block_trail_cursor_rect(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        [x, y, w, h]: [f32; 4],
        cell_w: f32,
        cell_h: f32,
        dt: f32,
        cursor_color: [f32; 4],
    ) {
        self.trail_cursor
            .set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);
        self.trail_cursor.set_destination(x, y, w, h);
        self.trail_cursor.animate(cell_w, cell_h, dt);
        self.trail_cursor.draw_always(sugarloaf, 1.0, cursor_color);
    }

    pub(crate) fn draw_content_trail_cursor_rect(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        [x, y, w, h]: [f32; 4],
        shape: neoism_terminal_core::ansi::CursorShape,
        dt: f32,
        cursor_color: [f32; 4],
    ) {
        self.trail_cursor.set_cursor_shape(shape);
        self.trail_cursor.set_destination(x, y, w, h);
        self.trail_cursor.animate(w, h, dt);
        if self.trail_cursor.is_animating() {
            self.trail_cursor.draw(sugarloaf, 1.0, cursor_color);
        } else {
            self.trail_cursor.draw_always(sugarloaf, 1.0, cursor_color);
        }
    }

    /// Resolve the dispatch order for a single event. Visible modals
    /// come first (in a fixed priority chain so e.g. opening the
    /// command palette while the finder is open routes Escape to the
    /// palette), then the focus-stack-top, then the remaining
    /// background panels in z-order.
    pub fn event_priority_order(&self, event: &UiEvent) -> Vec<PanelKey> {
        let mut order: Vec<PanelKey> = Vec::with_capacity(7);
        let keyboard_like = matches!(
            event,
            UiEvent::Key(_) | UiEvent::Text(_) | UiEvent::Composition(_)
        );
        let focused_file_tree =
            keyboard_like && self.focus_stack.last() == Some(&PanelKey::FileTree);

        // True modal overlays first.
        if self.command_palette.is_visible() {
            order.push(PanelKey::CommandPalette);
        }
        if self.finder.is_visible() {
            order.push(PanelKey::Finder);
        }
        if self.git_diff.is_visible() {
            order.push(PanelKey::GitDiff);
        }

        // The composer is a sticky input surface, not a full-screen
        // modal. A clicked/focused tree must be able to own
        // j/k/arrows/Enter while the composer remains visible below.
        if focused_file_tree {
            order.push(PanelKey::FileTree);
        }
        if self.command_composer.is_visible() {
            order.push(PanelKey::CommandComposer);
        }

        if self.top_bar.is_menu_open() {
            order.push(PanelKey::TopBar);
        }

        // Focus-stack top (if not already enqueued as a modal).
        if let Some(top) = self.focus_stack.last().copied() {
            if !order.contains(&top) {
                order.push(top);
            }
        }

        // Background panels last, in painting z-order (bottom up).
        for key in [
            PanelKey::FileTree,
            PanelKey::BufferTabs,
            PanelKey::StatusLine,
            PanelKey::TopBar,
        ] {
            if !order.contains(&key) {
                // FileTree is only present when installed.
                if key == PanelKey::FileTree && self.file_tree.is_none() {
                    continue;
                }
                order.push(key);
            }
        }

        order
    }

    /// Dispatch a single event to the panels in priority order.
    ///
    /// Keyboard-shaped events (`Key`, `Text`, `Composition`) stop
    /// after the first visible modal consumes them, because modals
    /// swallow the keyboard. Pointer-shaped events propagate
    /// through every panel whose layout rect contains the pointer
    /// position — that way a click outside a visible modal can still
    /// reach a background panel without the modal having to forward
    /// it.
    ///
    /// Tick / Resize / Theme / Focus / ServiceReply are broadcast to
    /// every panel regardless of visibility (they are panel-wide
    /// lifecycle events).
    pub fn handle_event(
        &mut self,
        event: &UiEvent,
        services: Services<'_>,
        time: Duration,
    ) {
        let theme = self.theme.clone();
        let mut ctx = PanelContext {
            services,
            theme: &theme,
            time,
        };

        let order = self.event_priority_order(event);
        let keyboard_like = matches!(
            event,
            UiEvent::Key(_) | UiEvent::Text(_) | UiEvent::Composition(_)
        );
        let pointer_like = matches!(
            event,
            UiEvent::PointerMove { .. }
                | UiEvent::PointerDown { .. }
                | UiEvent::PointerUp { .. }
                | UiEvent::PointerLeave
                | UiEvent::Wheel { .. }
        );

        if self.handle_chrome_key_shortcut(event, &mut ctx) {
            return;
        }

        if let UiEvent::PointerDown { x, y, .. } = event {
            let inside_tree = self
                .layout
                .file_tree
                .is_some_and(|rect| rect.contains(*x, *y));
            if let Some(tree) = self.file_tree.as_mut() {
                if !inside_tree {
                    tree.set_focused(false);
                    self.blur(PanelKey::FileTree);
                }
            }
        }

        // Track pointer position so subsequent Wheel events (which
        // don't carry coords in this vocabulary) can be routed to the
        // panel under the cursor — specifically, the file-viewer
        // smooth scroll below.
        if let UiEvent::PointerMove { x, y, .. }
        | UiEvent::PointerDown { x, y, .. }
        | UiEvent::PointerUp { x, y, .. } = event
        {
            self.last_pointer_pos = (*x, *y);
        }

        // Wheel events don't carry x/y in this vocabulary; route them
        // ONLY to the panel whose rect contains the last-known pointer
        // position. Without this gate, every visible panel (tree,
        // file-viewer, agent pane) scrolls in lockstep on every wheel
        // tick — matches the desktop behaviour where the panel under
        // the cursor is the one that scrolls.
        let (wheel_px, wheel_py) = self.last_pointer_pos;

        if pointer_like {
            if let Some(blocker) = self.active_pointer_modal_rect() {
                let inside = if matches!(event, UiEvent::Wheel { .. }) {
                    blocker.contains(wheel_px, wheel_py)
                } else {
                    pointer_inside(event, blocker)
                };
                if !inside {
                    // While a modal is open, pointer hover/click/wheel
                    // belongs to the modal layer. Letting PointerMove
                    // leak through keeps mutating hover colors in the
                    // tree/tabs/composer behind the opaque overlay,
                    // which reads as blinking at the modal edges.
                    //
                    // A press outside the card dismisses the modal —
                    // same light-dismiss behaviour as desktop (and what
                    // every touch user expects). The press itself is
                    // still swallowed so it can't also click whatever
                    // sat underneath.
                    if matches!(event, UiEvent::PointerDown { .. }) {
                        self.command_palette.set_enabled(false);
                        self.finder.set_enabled(false);
                        self.git_diff.hide();
                        self.relayout();
                    }
                    return;
                }
            }

            if self.top_bar.is_menu_open() {
                let inside_top_bar =
                    self.rect_for(PanelKey::TopBar).is_some_and(|rect| {
                        if matches!(event, UiEvent::Wheel { .. }) {
                            rect.contains(wheel_px, wheel_py)
                        } else {
                            pointer_inside(event, rect)
                        }
                    });
                if !inside_top_bar {
                    if !matches!(
                        event,
                        UiEvent::PointerMove { .. } | UiEvent::PointerLeave
                    ) {
                        self.top_bar.close_menu();
                    }
                    return;
                }
            }

            if self.handle_side_panel_pointer(event, wheel_px, wheel_py) {
                return;
            }
        }

        for key in order {
            // Pointer events only land on panels whose layout rect
            // contains the cursor. Modal panels with a `None` rect
            // (because they are hidden) are skipped by the priority
            // builder, so reaching them here means they are
            // currently visible and got a layout slot.
            if pointer_like {
                let inside = match self.rect_for(key) {
                    Some(r) => {
                        if matches!(event, UiEvent::Wheel { .. }) {
                            r.contains(wheel_px, wheel_py)
                        } else {
                            pointer_inside(event, r)
                        }
                    }
                    None => false,
                };
                if !inside {
                    continue;
                }
            }

            let top_bar_menu_was_open =
                key == PanelKey::TopBar && self.top_bar.is_menu_open();
            self.dispatch_to(key, event, &mut ctx);

            if keyboard_like && key == PanelKey::FileTree && self.focused() == Some(key) {
                break;
            }

            if pointer_like && (is_modal_key(key) || top_bar_menu_was_open) {
                break;
            }

            // Keyboard-shaped events stop at the first modal that
            // saw them, because modals swallow keyboard input.
            if keyboard_like && is_modal_key(key) {
                break;
            }
        }

        // Apply any side effect the top bar queued (panel toggle or a
        // hamburger-menu pick). Settings/Themes/Extensions don't have
        // destinations yet — they're stored in `pending_top_bar_action`
        // for the host bridge to drain and route to a future screen.
        if let Some(action) = self.top_bar.take_action() {
            self.apply_top_bar_action(action);
        }

        // Pick up any tab-click intents the buffer-tabs panel queued
        // during dispatch. Activate is mirrored into chrome's own
        // active_tab_index immediately; the close list is queued for
        // the host bridge to drain.
        if let Some(idx) = self.buffer_tabs.drain_active_change() {
            self.set_active_tab_index(idx);
            self.pending_buffer_tab_activate = Some(idx);
        }
        for ix in self.buffer_tabs.drain_close_requests() {
            self.close_buffer_tab(ix);
        }
        if self.buffer_tabs.drain_new_tab_request() {
            self.pending_buffer_tab_new = true;
        }
        if !self.buffer_tabs.is_focused() {
            self.blur(PanelKey::BufferTabs);
        }

        // File-viewer smooth scroll. The Wheel event itself doesn't
        // carry x/y; gate on the last-known pointer position so the
        // scroll only fires when the cursor was actually over the
        // terminal rect at the time of the wheel tick.
        if let UiEvent::Wheel { dy, mode, .. } = event {
            if !self.is_terminal_tab_active() {
                let terminal_rect = self.layout.terminal;
                let (px, py) = self.last_pointer_pos;
                let inside = terminal_rect.contains(px, py);
                if inside {
                    let line_h = self.cell_h.max(14.0);
                    let pixels = match mode {
                        WheelMode::Pixel => *dy,
                        WheelMode::Line => *dy * line_h,
                        WheelMode::Page => *dy * terminal_rect.h.max(line_h),
                    };
                    // Wheel dy is positive when scrolling down on
                    // most hosts; that should move content *up*,
                    // i.e. increase the scroll offset.
                    let max_scroll = self.max_file_viewer_scroll(line_h);
                    let prev = self.scroll_offset_px;
                    self.scroll_offset_px =
                        (self.scroll_offset_px + pixels).clamp(0.0, max_scroll);
                    let delta = self.scroll_offset_px - prev;
                    // Feed the spring the *negative* delta so its
                    // chase-to-zero animation tracks back toward the
                    // resolved offset. The render path subtracts the
                    // spring's residual position from the rendered y
                    // so the motion feels rubber-banded.
                    self.scroll_spring.position -= delta;
                }
            }
        }
    }

    /// Maximum logical-pixel scroll for the file-viewer pane given a
    /// line height. Computed from `tab_content`'s line count and the
    /// available terminal rect height (minus the same vertical
    /// padding the draw path uses).
    pub(crate) fn max_file_viewer_scroll(&self, line_h: f32) -> f32 {
        let Some(text) = self.tab_content.as_deref() else {
            return 0.0;
        };
        let lines = text.lines().count() as f32;
        let pad_y = 12.0_f32;
        let viewport_h = (self.layout.terminal.h - pad_y * 2.0).max(0.0);
        (lines * line_h - viewport_h).max(0.0)
    }

    pub(crate) fn handle_chrome_key_shortcut(
        &mut self,
        event: &UiEvent,
        _ctx: &mut PanelContext,
    ) -> bool {
        let UiEvent::Key(key) = event else {
            return false;
        };
        if key.state != KeyState::Pressed {
            return false;
        }

        let shift = key.modifiers.contains(Modifiers::SHIFT);
        let ctrl = key.modifiers.contains(Modifiers::CTRL);
        let alt = key.modifiers.contains(Modifiers::ALT);
        let meta = key.modifiers.contains(Modifiers::META);

        if self.handle_side_panel_key(key) {
            return true;
        }

        if self.focused() == Some(PanelKey::FileTree)
            && !ctrl
            && !alt
            && !meta
            && is_colon_or_semicolon_key(&key.logical)
        {
            self.command_palette.set_enabled(true);
            return true;
        }

        if meta && !ctrl && !alt {
            if is_character_key(&key.logical, "p") {
                // The two center modals are mutually exclusive.
                self.finder.set_enabled(false);
                self.command_palette.set_enabled(true);
                return true;
            }
            if !shift && is_character_key(&key.logical, "s") {
                self.command_palette.set_enabled(false);
                self.finder.set_enabled(true);
                return true;
            }
            if !shift && is_character_key(&key.logical, "a") {
                self.open_neoism_agent_tab(0);
                return true;
            }
            if is_colon_or_semicolon_key(&key.logical) {
                self.finder.set_enabled(false);
                self.command_palette.set_enabled(true);
                return true;
            }
        }

        if alt && !ctrl && !shift && !meta {
            if is_character_key(&key.logical, "e") {
                self.toggle_file_tree();
                return true;
            }
            if is_character_key(&key.logical, "g") {
                // Desktop parity: Alt+G owns the rich right-side git
                // panel, not the slim full-window overlay.
                self.toggle_git_diff_panel();
                return true;
            }
            if is_character_key(&key.logical, "n") {
                self.toggle_notes_sidebar();
                return true;
            }
            match &key.logical {
                LogicalKey::Named(NamedKey::ArrowUp) => {
                    self.hide_focus_modals();
                    if self.buffer_tabs.is_focused() {
                        self.buffer_tabs.move_focused(false);
                    } else {
                        self.focus_buffer_tabs();
                    }
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowDown) => {
                    self.hide_focus_modals();
                    if self.buffer_tabs.is_focused() {
                        self.buffer_tabs.set_focused(false);
                        self.blur(PanelKey::BufferTabs);
                    }
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowLeft) => {
                    self.hide_focus_modals();
                    if self.buffer_tabs.is_focused() {
                        if self.buffer_tabs.focused_index() == 0 {
                            self.buffer_tabs.set_focused(false);
                            self.blur(PanelKey::BufferTabs);
                            self.show_file_tree();
                        } else {
                            self.buffer_tabs.move_focused(true);
                        }
                        return true;
                    }
                    self.show_file_tree();
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowRight) => {
                    self.hide_focus_modals();
                    if self.buffer_tabs.is_focused() {
                        self.buffer_tabs.move_focused(false);
                        return true;
                    }
                    if self.focused() == Some(PanelKey::FileTree) {
                        if let Some(tree) = self.file_tree.as_mut() {
                            tree.set_focused(false);
                        }
                        self.blur(PanelKey::FileTree);
                        return true;
                    }
                }
                _ => {}
            }
        }

        false
    }

    pub(crate) fn hide_focus_modals(&mut self) {
        self.command_palette.set_enabled(false);
        self.finder.set_enabled(false);
    }

    pub(crate) fn focus_buffer_tabs(&mut self) -> bool {
        if !self.buffer_tabs.is_visible() || self.buffer_tabs.tabs().is_empty() {
            return false;
        }
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
        self.blur(PanelKey::FileTree);
        self.buffer_tabs.set_focused(true);
        self.focus(PanelKey::BufferTabs);
        true
    }

    pub fn open_neoism_agent_tab(&mut self, route_id: usize) -> usize {
        let idx = self.buffer_tabs.open_neoism_agent(route_id);
        self.set_active_tab_index(idx);
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
        self.blur(PanelKey::FileTree);
        idx
    }

    /// Re-run layout against the last viewport. Side-panel toggles
    /// change column widths mid-frame, so they relayout immediately
    /// instead of waiting for the host's next resize.
    pub(crate) fn relayout(&mut self) {
        if let Some(viewport) = self.last_viewport {
            self.set_layout(viewport);
        }
    }

    /// Workspace root the host dialed into. The git side panel uses
    /// it as repo root; the notes sidebar lists `<root>/notes`.
    pub fn set_workspace_root_path(&mut self, root: Option<std::path::PathBuf>) {
        self.workspace_root_path = root;
    }

    /// Toggle the rich right-side git diff panel (desktop Alt+G).
    /// Returns the new visibility. On open, queues a refresh intent so
    /// hosts without a native `GitDiffIo` (web) fetch via the daemon.
    pub fn toggle_git_diff_panel(&mut self) -> bool {
        let repo_root = self.workspace_root_path.clone();
        let branch = self.status_line.info().branch.clone();
        self.git_diff_panel.toggle(repo_root, branch);
        let visible = self.git_diff_panel.is_visible();
        if visible {
            self.pending_git_panel_refresh = true;
            if let Some(tree) = self.file_tree.as_mut() {
                tree.set_focused(false);
            }
            self.blur(PanelKey::FileTree);
        }
        self.relayout();
        visible
    }

    /// Toggle the notes sidebar (desktop Alt+N). Desktop resolves a
    /// notes workspace from `.neoism/workspace.toml`; the daemon's
    /// convention is `<root>/notes`, which the host lists and pushes
    /// back via `notes_sidebar.set_entries_from_host`.
    pub fn toggle_notes_sidebar(&mut self) -> bool {
        if !self.notes_sidebar.is_visible() {
            let root = self.workspace_root_path.clone();
            let name = root
                .as_deref()
                .and_then(|r| r.file_name())
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "Default".to_string());
            let notes_dir = root.map(|r| r.join("notes"));
            self.notes_sidebar.set_workspace(name, notes_dir);
            self.pending_notes_refresh = true;
        }
        let changed = self.notes_sidebar.toggle_focus_or_visibility();
        if self.notes_sidebar.is_visible() {
            if let Some(tree) = self.file_tree.as_mut() {
                tree.set_focused(false);
            }
            self.blur(PanelKey::FileTree);
        }
        if changed {
            self.relayout();
        }
        self.notes_sidebar.is_visible()
    }

    /// Drain paths activated in the git side panel / notes sidebar.
    pub fn drain_panel_open_paths(&mut self) -> Vec<String> {
        std::mem::take(&mut self.pending_panel_open_paths)
    }

    /// One-shot "git side panel wants data" flag for the web host.
    pub fn take_git_panel_refresh(&mut self) -> bool {
        std::mem::take(&mut self.pending_git_panel_refresh)
    }

    /// One-shot "notes sidebar wants a listing" flag for the web host.
    /// Drains both the open-time flag and the panel's own dirty flag
    /// (raised by [`mark_notes_dirty`] on external vault mutations), so a
    /// live add/delete refreshes without a manual close/open.
    pub fn take_notes_refresh(&mut self) -> bool {
        let queued = std::mem::take(&mut self.pending_notes_refresh);
        let dirtied = self.notes_sidebar.take_refresh();
        queued || dirtied
    }

    /// Tell the notes sidebar its vault changed on disk — e.g. an agent
    /// or a file operation added/deleted a page. No-op while the panel is
    /// hidden. The native host can additionally call
    /// `notes_sidebar.refresh_notes()` directly (local fs); the web host
    /// answers the drained refresh flag with a fresh daemon listing.
    pub fn mark_notes_dirty(&mut self) {
        self.notes_sidebar.mark_dirty();
    }

    /// The notes sidebar's window rect for the current layout, or
    /// `None` while hidden. Layout reserves the column; this mirrors
    /// the same math for hit-testing.
    pub(crate) fn notes_sidebar_rect(&self) -> Option<Rect> {
        if !self.notes_sidebar.is_visible() {
            return None;
        }
        let viewport = self.last_viewport?;
        let x_left = self
            .layout
            .file_tree
            .map(|ft| ft.x + ft.w)
            .unwrap_or(viewport.x);
        Some(Rect::new(
            x_left,
            viewport.y,
            self.notes_sidebar.width().min(viewport.w * 0.8),
            viewport.h,
        ))
    }

    /// Half-page row count for the notes sidebar's PageUp/PageDown jumps,
    /// derived from the live panel height (falls back to 1 while hidden).
    pub(crate) fn notes_half_page_rows(&self) -> usize {
        let rows = self
            .notes_sidebar_rect()
            .map(|rect| self.notes_sidebar.visible_rows_for_panel_height(rect.h))
            .unwrap_or(1);
        (rows / 2).max(1)
    }

    /// Pointer / wheel routing for the two side panels. Returns true
    /// when the event was consumed and must not fall through to the
    /// panel priority loop.
    pub(crate) fn handle_side_panel_pointer(
        &mut self,
        event: &UiEvent,
        wheel_px: f32,
        wheel_py: f32,
    ) -> bool {
        // Wheel: route to whichever panel owns the pointer position.
        if let UiEvent::Wheel { dy, mode, .. } = event {
            let line_h = self.cell_h.max(14.0);
            let pixels = match mode {
                WheelMode::Pixel => *dy,
                WheelMode::Line => *dy * line_h,
                WheelMode::Page => *dy * self.layout.terminal.h.max(line_h),
            };
            if self.git_diff_panel.is_visible()
                && self
                    .git_diff_panel
                    .active_rect()
                    .is_some_and(|[x, y, w, h]| {
                        wheel_px >= x
                            && wheel_px <= x + w
                            && wheel_py >= y
                            && wheel_py <= y + h
                    })
            {
                // Host wheel dy is positive scrolling down (DOM); the
                // panel's springs use the desktop positive-up sign.
                self.git_diff_panel.scroll_at(wheel_px, wheel_py, -pixels);
                return true;
            }
            if let Some(rect) = self.notes_sidebar_rect() {
                if rect.contains(wheel_px, wheel_py) {
                    // Trackpad PIXEL scrolling, same accumulator model as
                    // the file tree: feed raw pixels so a slow drag eases
                    // a row at a time instead of jumping per wheel event.
                    let rows_visible =
                        self.notes_sidebar.visible_rows_for_panel_height(rect.h);
                    self.notes_sidebar.scroll_pixels(pixels, rows_visible);
                    return true;
                }
            }
            return false;
        }

        let UiEvent::PointerDown { x, y, .. } = event else {
            return false;
        };

        if self.git_diff_panel.is_visible() {
            let hit = self.git_diff_panel.hit_test(*x, *y);
            // A click outside the branch dropdown closes it first.
            if self.git_diff_panel.branch_menu_is_open()
                && !matches!(
                    hit,
                    GitPanelHit::BranchMenuRow(_)
                        | GitPanelHit::BranchFilterBox
                        | GitPanelHit::BranchButton
                )
            {
                self.git_diff_panel.close_branch_menu();
            }
            match hit {
                GitPanelHit::Close => {
                    self.git_diff_panel.close();
                    self.relayout();
                    return true;
                }
                GitPanelHit::FileRow(idx) => {
                    self.git_diff_panel.set_focused(true);
                    self.git_diff_panel.focus_files_section();
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.select_file(idx);
                    return true;
                }
                GitPanelHit::FileCheckbox(idx) => {
                    self.git_diff_panel.set_focused(true);
                    self.git_diff_panel.focus_files_section();
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.toggle_stage(idx);
                    return true;
                }
                GitPanelHit::CommitBox => {
                    self.git_diff_panel.focus_commit_box(true);
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    return true;
                }
                GitPanelHit::CommitButton => {
                    self.git_diff_panel.set_focused(true);
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.commit();
                    return true;
                }
                GitPanelHit::StageAllButton => {
                    self.git_diff_panel.set_focused(true);
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.stage_all();
                    return true;
                }
                GitPanelHit::FolderToggle(visual_ix) => {
                    self.git_diff_panel.set_focused(true);
                    self.git_diff_panel.focus_files_section();
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.toggle_folder(visual_ix);
                    return true;
                }
                GitPanelHit::BranchButton => {
                    self.git_diff_panel.set_focused(true);
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    self.git_diff_panel.toggle_branch_menu();
                    return true;
                }
                GitPanelHit::BranchFilterBox => {
                    // Keep the dropdown open; clicks in the search box are
                    // consumed without further action.
                    return true;
                }
                GitPanelHit::BranchMenuRow(slot) => {
                    self.git_diff_panel.activate_branch_row(slot);
                    return true;
                }
                GitPanelHit::Inside => {
                    self.git_diff_panel.set_focused(true);
                    self.git_diff_panel.focus_files_section();
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    return true;
                }
                GitPanelHit::Outside => {
                    self.git_diff_panel.set_focused(false);
                }
            }
        }

        if let Some(rect) = self.notes_sidebar_rect() {
            if rect.contains(*x, *y) {
                if let Some(hit) = self.notes_sidebar.hit_test(*x, *y) {
                    self.notes_sidebar.set_focused(true);
                    if let Some(tree) = self.file_tree.as_mut() {
                        tree.set_focused(false);
                    }
                    self.blur(PanelKey::FileTree);
                    if let NotesSidebarHit::Note(index)
                    | NotesSidebarHit::NoteIcon(index) = hit
                    {
                        self.notes_sidebar.set_selected(index);
                        if self.notes_sidebar.note_is_dir(index) {
                            self.notes_sidebar.toggle_selected_dir();
                        } else if let Some(path) = self.notes_sidebar.note_path(index) {
                            self.notes_sidebar.set_focused(false);
                            self.pending_panel_open_paths
                                .push(path.to_string_lossy().into_owned());
                        }
                    }
                }
                // Clicks anywhere on the sidebar belong to it.
                return true;
            }
            self.notes_sidebar.set_focused(false);
        }

        false
    }

    /// Keyboard handling for a focused side panel: arrows move the
    /// selection, Enter activates, Escape closes. Returns true when
    /// the key was consumed.
    pub(crate) fn handle_side_panel_key(
        &mut self,
        key: &crate::event::KeyDescriptor,
    ) -> bool {
        let plain = !key.modifiers.contains(Modifiers::CTRL)
            && !key.modifiers.contains(Modifiers::ALT)
            && !key.modifiers.contains(Modifiers::META);
        if !plain {
            return false;
        }
        if self.git_diff_panel.is_focused() {
            match &key.logical {
                LogicalKey::Named(NamedKey::ArrowUp) => {
                    self.git_diff_panel.select_prev();
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowDown) => {
                    self.git_diff_panel.select_next();
                    return true;
                }
                LogicalKey::Named(NamedKey::Enter) => {
                    if let Some((path, _root)) =
                        self.git_diff_panel.selected_file_target()
                    {
                        self.git_diff_panel.set_focused(false);
                        self.pending_panel_open_paths
                            .push(path.to_string_lossy().into_owned());
                    }
                    return true;
                }
                LogicalKey::Named(NamedKey::Escape) => {
                    self.git_diff_panel.close();
                    self.relayout();
                    return true;
                }
                _ => {}
            }
            return false;
        }
        if self.notes_sidebar.is_visible() && self.notes_sidebar.is_focused() {
            match &key.logical {
                LogicalKey::Named(NamedKey::ArrowUp) => {
                    self.notes_sidebar.select_prev();
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowDown) => {
                    self.notes_sidebar.select_next();
                    return true;
                }
                // Half-page jumps mirror the file tree's Ctrl+D / Ctrl+U
                // (and PageDown / PageUp). Compute the page from the live
                // panel height so it tracks the visible row count.
                LogicalKey::Named(NamedKey::PageDown) => {
                    let half = self.notes_half_page_rows();
                    self.notes_sidebar.select_next_by(half);
                    return true;
                }
                LogicalKey::Named(NamedKey::PageUp) => {
                    let half = self.notes_half_page_rows();
                    self.notes_sidebar.select_prev_by(half);
                    return true;
                }
                // Vault selector → share icon → ⋮ menu caret walk; consumed
                // either way so arrows never leak into the pane below.
                LogicalKey::Named(NamedKey::ArrowRight) => {
                    let _ = self.notes_sidebar.move_horizontal_focus(true);
                    return true;
                }
                LogicalKey::Named(NamedKey::ArrowLeft) => {
                    let _ = self.notes_sidebar.move_horizontal_focus(false);
                    return true;
                }
                LogicalKey::Named(NamedKey::Enter) => {
                    let index = self.notes_sidebar.selected_index();
                    if self.notes_sidebar.note_is_dir(index) {
                        self.notes_sidebar.toggle_selected_dir();
                    } else if let Some(path) = self.notes_sidebar.note_path(index) {
                        self.notes_sidebar.set_focused(false);
                        self.pending_panel_open_paths
                            .push(path.to_string_lossy().into_owned());
                    }
                    return true;
                }
                LogicalKey::Named(NamedKey::Escape) => {
                    self.notes_sidebar.set_visible(false);
                    self.notes_sidebar.set_focused(false);
                    self.relayout();
                    return true;
                }
                _ => {}
            }
        }
        false
    }

    pub(crate) fn rect_for(&self, key: PanelKey) -> Option<Rect> {
        match key {
            PanelKey::StatusLine => Some(self.layout.status_line),
            PanelKey::BufferTabs => Some(self.layout.buffer_tabs),
            PanelKey::TopBar => self.layout.top_bar.map(|strip| {
                // When the dropdown is open, extend the hit area to
                // cover the menu so clicks on items still route to us
                // instead of falling through to the panels below.
                match self.top_bar.menu_overlay_rect() {
                    Some(menu) => Rect::new(
                        strip.x.min(menu.x),
                        strip.y.min(menu.y),
                        strip.w.max(menu.x + menu.w - strip.x.min(menu.x)),
                        (menu.y + menu.h - strip.y.min(menu.y))
                            .max(strip.y + strip.h - strip.y.min(menu.y)),
                    ),
                    None => strip,
                }
            }),
            PanelKey::Breadcrumbs => self.layout.breadcrumbs,
            PanelKey::FileTree => self.layout.file_tree,
            PanelKey::CommandPalette => self.layout.command_palette,
            PanelKey::Finder => self.layout.finder,
            PanelKey::GitDiff => self.layout.git_diff,
            PanelKey::CommandComposer => self.layout.command_composer,
            // Slim panels don't own their own layout rect yet — they
            // paint over existing rects (terminal column, tab strip,
            // etc.) or are popovers that self-position. Returning
            // `None` keeps them out of pointer-hit dispatch until a
            // future routing wave assigns proper rects.
            PanelKey::CompletionMenu
            | PanelKey::Minimap
            | PanelKey::Notifications
            | PanelKey::DiagnosticsPopup
            | PanelKey::ContextMenu
            | PanelKey::Search
            | PanelKey::GitBranch
            | PanelKey::CustomCursor
            | PanelKey::CursorlineOverlay
            | PanelKey::TrailCursor
            | PanelKey::YankFlash
            | PanelKey::EditorScroll => None,
        }
    }

    pub(crate) fn active_pointer_modal_rect(&self) -> Option<Rect> {
        if self.command_palette.is_visible() {
            return self.layout.command_palette;
        }
        if self.finder.is_visible() {
            return self.layout.finder;
        }
        if self.git_diff.is_visible() {
            return self.layout.git_diff;
        }
        None
    }

    pub(crate) fn dispatch_to(
        &mut self,
        key: PanelKey,
        event: &UiEvent,
        ctx: &mut PanelContext,
    ) {
        match key {
            PanelKey::StatusLine => self.status_line.handle_event(event, ctx),
            PanelKey::TopBar => self.top_bar.handle_event(event, ctx),
            PanelKey::BufferTabs => {
                // The buffer-tabs `Panel` impl assumes pointer coords are
                // strip-local (its `hit_test` is called with `x_left = 0`,
                // `y_top = 0`). The event vocabulary, however, delivers
                // window-global x/y. Translate pointer events by the
                // strip's origin so a click at global `(content_x + 30, …)`
                // becomes local `(30, …)` regardless of where the strip
                // sits (e.g. after the file-tree sidebar shifts it right).
                // Non-pointer events are forwarded unchanged.
                let origin = self.layout.buffer_tabs;
                let translated;
                let event_ref = match event {
                    UiEvent::PointerMove { x, y, modifiers } => {
                        translated = UiEvent::PointerMove {
                            x: *x - origin.x,
                            y: *y - origin.y,
                            modifiers: *modifiers,
                        };
                        &translated
                    }
                    UiEvent::PointerDown {
                        button,
                        x,
                        y,
                        modifiers,
                        click_count,
                    } => {
                        translated = UiEvent::PointerDown {
                            button: *button,
                            x: *x - origin.x,
                            y: *y - origin.y,
                            modifiers: *modifiers,
                            click_count: *click_count,
                        };
                        &translated
                    }
                    UiEvent::PointerUp {
                        button,
                        x,
                        y,
                        modifiers,
                    } => {
                        translated = UiEvent::PointerUp {
                            button: *button,
                            x: *x - origin.x,
                            y: *y - origin.y,
                            modifiers: *modifiers,
                        };
                        &translated
                    }
                    other => other,
                };
                self.buffer_tabs.handle_event(event_ref, ctx);
            }
            PanelKey::FileTree => {
                let bounds = self.layout.file_tree;
                let handled = if let Some(tree) = self.file_tree.as_mut() {
                    tree.handle_ui_event(event, ctx, bounds)
                } else {
                    false
                };
                if handled && matches!(event, UiEvent::PointerDown { .. }) {
                    if self
                        .file_tree
                        .as_ref()
                        .is_some_and(|tree| tree.is_focused())
                    {
                        self.focus(PanelKey::FileTree);
                    } else {
                        self.blur(PanelKey::FileTree);
                    }
                }
            }
            PanelKey::CommandPalette => self.command_palette.handle_event(event, ctx),
            PanelKey::Finder => self.finder.handle_event(event, ctx),
            PanelKey::GitDiff => self.git_diff.handle_event(event, ctx),
            PanelKey::CommandComposer => self.command_composer.handle_event(event, ctx),
            // Slim panels don't yet receive routed `UiEvent`s — they
            // are driven directly by the host (bridge state pushes /
            // free-function calls in `draw`). This arm is a no-op
            // placeholder so the match stays exhaustive; a future
            // routing wave will turn each into a real dispatch.
            PanelKey::Breadcrumbs
            | PanelKey::CompletionMenu
            | PanelKey::Minimap
            | PanelKey::Notifications
            | PanelKey::DiagnosticsPopup
            | PanelKey::ContextMenu
            | PanelKey::Search
            | PanelKey::GitBranch
            | PanelKey::CustomCursor
            | PanelKey::CursorlineOverlay
            | PanelKey::TrailCursor
            | PanelKey::YankFlash
            | PanelKey::EditorScroll => {}
        }
    }
}
