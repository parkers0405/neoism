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

fn chrome_focus_cursor_animation_size(rect: [f32; 4]) -> (f32, f32) {
    (rect[2], rect[3])
}

/// Smallest useful surface beside a sidebar. Above the phone breakpoint the
/// sidebar widths are clamped to preserve this column; below it a visible
/// sidebar takes over the middle band and the content rect becomes empty.
const RESPONSIVE_CONTENT_MIN_W: f32 = 320.0;
const RESPONSIVE_SIDEBAR_MIN_W: f32 = 120.0;
const TREE_CONTENT_GAP: f32 = 4.0;

impl<A: Send + Copy + 'static> Chrome<A> {
    /// Recompute every panel's rect against `viewport`. The viewport
    /// is the full window content area in logical pixels (top-left
    /// origin). Panels that are currently hidden (modals) still get
    /// their rect resolved here so showing them is a state flip, not
    /// a layout pass.
    pub fn set_layout(&mut self, viewport: Rect) {
        self.last_viewport = Some(viewport);
        let scale = self.chrome_scale.clamp(0.5, 3.0);
        let mobile_agent_narrow = self.mobile_web_agent_panel_enabled
            && viewport.w
                < crate::panels::agent_pane::state::side_panel::SIDE_PANEL_MIN_PANE_WIDTH
                    * scale;
        if mobile_agent_narrow != self.mobile_agent_narrow {
            if mobile_agent_narrow {
                if let Some(pane) = self.agent_pane.as_mut() {
                    self.desktop_agent_panel_open_before_narrow =
                        Some(!pane.side_panel().user_hidden());
                    pane.side_panel_mut().set_user_hidden(true);
                }
            } else if let Some(was_open) =
                self.desktop_agent_panel_open_before_narrow.take()
            {
                if let Some(pane) = self.agent_pane.as_mut() {
                    pane.side_panel_mut().set_user_hidden(!was_open);
                }
            }
            self.mobile_agent_narrow = mobile_agent_narrow;
        }
        self.top_bar.set_mobile_agent_panel_button_visible(
            mobile_agent_narrow && self.is_neoism_agent_tab_active(),
        );
        let tabs_h = BUFFER_TABS_HEIGHT * scale;
        let status_h = STATUS_LINE_HEIGHT * scale;

        // Top bar spans the full viewport width, pinned to the top
        // edge above everything else (its rect is built below). The
        // side panels (tree / notes / git) are confined to the band
        // beneath the top chrome rather than running the full window
        // height, so they no longer push the top bar / tabs inward.
        // Agent is a global open/focus action, not an active-tab control.
        // Keep its affordance available from every surface, matching desktop.
        self.top_bar.set_right_button_visible(true);
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
        // With no obstruction, content stops at the status line as usual.
        // With a mobile keyboard, status stays at the physical bottom (behind
        // the keyboard) and content stops exactly at the keyboard's top — do
        // not reserve status_h a second time or a visible gap appears.
        let band_bottom = if self.bottom_content_inset > 0.0 {
            (viewport.y + viewport.h - self.bottom_content_inset).max(band_top)
        } else {
            status_line.y
        };
        let band_h = (band_bottom - band_top).max(0.0);

        // Sidebar column for the file tree — spans the middle band
        // (between the full-width top chrome and the full-width status
        // bar). The tree is installed via `install_file_tree` but its
        // visibility is per-frame via `FileTree::is_visible` — when
        // closed the slot returns `None` and the content column reclaims
        // the full band width. Native toggles this with Ctrl+Shift+B.
        // Rich git side panel reserves a right column in the middle
        // band while visible — the content column must not paint
        // underneath it (chrome reflow, not z-order).
        let right_inset = self
            .git_diff_panel
            .effective_width(viewport.w)
            .clamp(0.0, viewport.w);
        let middle_right = viewport.x + viewport.w - right_inset;
        let middle_w = (middle_right - viewport.x).max(0.0);
        let tree_natural = self
            .file_tree
            .as_ref()
            .filter(|tree| tree.is_visible())
            .map(|tree| tree.width().min(middle_w));
        let notes_natural = self
            .notes_sidebar
            .is_visible()
            .then(|| self.notes_sidebar.width().min(middle_w * 0.8));
        let left_panel_count =
            usize::from(tree_natural.is_some()) + usize::from(notes_natural.is_some());
        let takeover = left_panel_count > 0
            && viewport.w
                < (RESPONSIVE_CONTENT_MIN_W
                    + RESPONSIVE_SIDEBAR_MIN_W * left_panel_count as f32)
                    * scale
                    + TREE_CONTENT_GAP;

        // In takeover mode only the focused/most-recent sidebar owns the band.
        // Notes toggling gives Notes focus; otherwise the tree wins. The other
        // visible panel deliberately receives no paint/hit rect.
        let (file_tree_rect, notes_sidebar_rect, content_x, content_w) = if takeover {
            let notes_owns = notes_natural.is_some()
                && (self.notes_sidebar.is_focused() || tree_natural.is_none());
            let panel = Rect::new(viewport.x, band_top, middle_w, band_h);
            (
                (!notes_owns)
                    .then_some(panel)
                    .filter(|_| tree_natural.is_some()),
                notes_owns.then_some(panel),
                middle_right,
                0.0,
            )
        } else {
            let min_content = (RESPONSIVE_CONTENT_MIN_W * scale).min(middle_w);
            let budget = (middle_w
                - min_content
                - if left_panel_count > 0 {
                    TREE_CONTENT_GAP
                } else {
                    0.0
                })
            .max(0.0);
            let tree_min =
                tree_natural.map_or(0.0, |w| w.min(RESPONSIVE_SIDEBAR_MIN_W * scale));
            let notes_min =
                notes_natural.map_or(0.0, |w| w.min(RESPONSIVE_SIDEBAR_MIN_W * scale));
            let remaining = (budget - tree_min - notes_min).max(0.0);
            let tree_extra = (tree_natural.unwrap_or(0.0) - tree_min).max(0.0);
            let notes_extra = (notes_natural.unwrap_or(0.0) - notes_min).max(0.0);
            let extra_total = tree_extra + notes_extra;
            let distributable = remaining.min(extra_total);
            let tree_w = tree_min
                + if extra_total > 0.0 {
                    distributable * tree_extra / extra_total
                } else {
                    0.0
                };
            let notes_w = notes_min
                + if extra_total > 0.0 {
                    distributable * notes_extra / extra_total
                } else {
                    0.0
                };
            let tree_rect =
                tree_natural.map(|_| Rect::new(viewport.x, band_top, tree_w, band_h));
            let notes_x = viewport.x + tree_w;
            let notes_rect =
                notes_natural.map(|_| Rect::new(notes_x, band_top, notes_w, band_h));
            let used = tree_w + notes_w;
            let gap = if left_panel_count > 0 {
                TREE_CONTENT_GAP
            } else {
                0.0
            };
            let x = (viewport.x + used + gap).min(middle_right);
            (tree_rect, notes_rect, x, (middle_right - x).max(0.0))
        };

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

        let composer_h = if content_w > 0.0 && self.terminal_composer_eligible() {
            let pane_rows = ((band_bottom - band_top) / self.cell_h.max(1.0))
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
        let composer_rect = if content_w > 0.0 && self.terminal_composer_eligible() {
            Some(Rect::new(
                content_x,
                band_bottom - composer_h,
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
            None => band_bottom,
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
            notes_sidebar: notes_sidebar_rect,
            buffer_tabs,
            breadcrumbs,
            status_line,
            terminal,
            command_palette,
            finder,
            git_diff,
            command_composer: composer_rect,
            panes: Vec::new(),
        };

        // Re-solve the shared pane grid against the freshly computed
        // content rect so pane rects / divider bands / drop zones hit
        // and paint in window coordinates every relayout. Zero gap:
        // pane boundaries stay flush (the web surfaces already fill
        // their normalized rects edge-to-edge) while the solver's
        // divider tolerance still inflates each band for grabbing.
        self.pane_grid.set_content(terminal, 0.0);

        // Per-pane chrome rects (desktop `apply_pane_chrome_offsets`
        // rules): top-aligned panes use the workspace strip; stacked
        // panes reserve a local tab strip (+ breadcrumbs row when the
        // active tab shows a document) inside their own rect.
        let mut pane_chrome = Vec::new();
        if self.pane_grid.is_split() {
            let min_top = self
                .pane_grid
                .panes()
                .iter()
                .map(|p| p.rect.y)
                .fold(f32::INFINITY, f32::min);
            for pane in self.pane_grid.panes() {
                let Some(ext) = pane.external_id else {
                    continue;
                };
                let rect = pane.rect;
                let top_aligned =
                    crate::session_layout::is_pane_top_aligned(rect.y, min_top);
                if top_aligned {
                    pane_chrome.push(crate::layout::PaneChromeLayout {
                        external_id: ext,
                        rect,
                        tabs: None,
                        breadcrumbs: None,
                        content: rect,
                    });
                    continue;
                }
                let strip_h = (BUFFER_TABS_HEIGHT * scale).min(rect.h);
                let tabs_rect = Rect::new(rect.x, rect.y, rect.w, strip_h);
                let crumbs_h = if self
                    .pane_tabs
                    .get(&ext)
                    .is_some_and(|tabs| tabs.active_shows_breadcrumbs())
                {
                    self.pane_breadcrumbs
                        .get(&ext)
                        .map(|crumbs| crumbs.height())
                        .unwrap_or(0.0)
                        .min((rect.h - strip_h).max(0.0))
                } else {
                    0.0
                };
                let crumbs_rect = (crumbs_h > 0.0)
                    .then(|| Rect::new(rect.x, rect.y + strip_h, rect.w, crumbs_h));
                pane_chrome.push(crate::layout::PaneChromeLayout {
                    external_id: ext,
                    rect,
                    tabs: Some(tabs_rect),
                    breadcrumbs: crumbs_rect,
                    content: Rect::new(
                        rect.x,
                        rect.y + strip_h + crumbs_h,
                        rect.w,
                        (rect.h - strip_h - crumbs_h).max(0.0),
                    ),
                });
            }
        }
        self.layout.panes = pane_chrome;
    }

    pub fn set_bottom_content_inset(&mut self, inset: f32) {
        self.bottom_content_inset = inset.max(0.0);
        if let Some(viewport) = self.last_viewport {
            self.set_layout(viewport);
        }
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
                if !self.is_neoism_agent_tab_active()
                    || self.agent_side_panel_takeover_active()
                {
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
        _cell_w: f32,
        _cell_h: f32,
        dt: f32,
        cursor_color: [f32; 4],
    ) {
        self.trail_cursor
            .set_cursor_shape(neoism_terminal_core::ansi::CursorShape::Block);
        self.trail_cursor.set_destination(x, y, w, h);
        // Chrome focus rects carry their own geometry. In particular, a
        // focused buffer tab publishes a narrow left-edge insertion bar.
        // Animating with the terminal cell dimensions expanded that 2–3 px
        // rect back into a full block on web; desktop animates with `w, h`.
        let (cursor_w, cursor_h) = chrome_focus_cursor_animation_size([x, y, w, h]);
        self.trail_cursor.animate(cursor_w, cursor_h, dt);
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
        if self.terminal_composer_eligible() {
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

        // The file browser is a true modal and owns every input shape. It is
        // checked before global shortcuts and every canvas surface.
        if self.file_browser.is_active() && self.file_browser.handle_event(event) {
            return;
        }

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

        // Chrome-page layer first: the full-screen settings overlay
        // and the About modal own input outright while active, and an
        // active Extensions/NeoWorld tab owns input inside its page
        // rect. Runs BEFORE the chrome key shortcuts so Alt+E & co.
        // can't reach the panels underneath an overlay — desktop
        // parity with router/route.rs's settings/modal arms.
        if self.handle_chrome_page_event(event) {
            return;
        }

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
            // Hosted editor panes (markdown / code / notebook / draw)
            // own their scroll through the bridge routes — the legacy
            // plain-painter offset below must not fight them.
            if !self.is_terminal_tab_active()
                && self.active_editor_pane_kind().is_none()
                && !(self.tab_lang == crate::syntax::Lang::Markdown
                    && self.markdown_pane.is_some())
            {
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
                        if self.buffer_tabs.focused_index() == 0
                            && self.file_tree.as_ref().is_some_and(|t| t.is_visible())
                        {
                            self.buffer_tabs.set_focused(false);
                            self.blur(PanelKey::BufferTabs);
                            self.show_file_tree();
                        } else {
                            self.buffer_tabs.move_focused(true);
                        }
                        return true;
                    }
                    // Step off the agent side panel back onto the agent
                    // body (timeline / composer) before leaving for the
                    // tree - desktop's `focus_horizontal_chrome` orders
                    // it the same way.
                    if self.agent_side_panel_focused() {
                        if let Some(pane) = self.agent_pane.as_mut() {
                            pane.side_panel_mut().set_focused(false);
                        }
                        return true;
                    }
                    // Notes sidebar sits between the file tree and the
                    // editor in desktop's spatial chain
                    // (tree -> notes -> editor -> git panel), so Alt+Left
                    // walks notes -> tree and editor -> notes.
                    if self.notes_sidebar.is_visible() && self.notes_sidebar.is_focused()
                    {
                        if self.file_tree.as_ref().is_some_and(|t| t.is_visible()) {
                            self.notes_sidebar.set_focused(false);
                            self.show_file_tree();
                        }
                        return true;
                    }
                    if self.notes_sidebar.is_visible() {
                        self.focus_notes_sidebar();
                        return true;
                    }
                    if self.file_tree.as_ref().is_some_and(|t| t.is_visible()) {
                        self.show_file_tree();
                    }
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
                        // Tree -> notes when the sidebar is open, matching
                        // desktop's spatial walk; otherwise straight on to
                        // the editor / agent body.
                        if self.notes_sidebar.is_visible() {
                            self.focus_notes_sidebar();
                        }
                        // Leaving the tree lands on the agent body, so
                        // the composer takes the caret. Without this the
                        // tree simply blurred and focus went nowhere -
                        // Alt+Right could never reach the agent pane on
                        // web, unlike desktop where the same step runs
                        // `focus_main_workspace()`.
                        return true;
                    }
                    if self.notes_sidebar.is_visible() && self.notes_sidebar.is_focused()
                    {
                        // Notes -> editor / agent body.
                        self.notes_sidebar.set_focused(false);
                        return true;
                    }
                    // Already on the agent body: the next step right is
                    // the agent's own side panel (sessions / subagents),
                    // matching desktop's per-pane slot between the agent
                    // body and the global git panel.
                    if self.agent_side_panel_focusable() {
                        if let Some(pane) = self.agent_pane.as_mut() {
                            pane.side_panel_mut().set_focused(true);
                            if pane.side_panel().only_back_focusable() {
                                pane.side_panel_mut().focus_back();
                            }
                        }
                        return true;
                    }
                }
                _ => {}
            }
        }

        false
    }

    /// The share sheet is modal while open: it swallows the next click
    /// and Escape. Returns true when it consumed the input.
    pub fn dismiss_share_sheet_if_open(&mut self) -> bool {
        if !self.share_sheet.is_visible() {
            return false;
        }
        self.share_sheet.hide();
        true
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

    /// Workspace root the host dialed into. The git side panel uses it
    /// as repo root. The notes sidebar does NOT derive from this - notes
    /// live in vaults, so the host pushes that separately via
    /// [`Chrome::set_notes_vault_root`].
    pub fn set_workspace_root_path(&mut self, root: Option<std::path::PathBuf>) {
        self.workspace_root_path = root;
    }

    /// Vault directory the notes sidebar lists. The host resolves it the
    /// same way every other notes surface does - the daemon advertises
    /// `WorkspaceSummary::linked_vault_dir`, which is
    /// `linked_project_for_code_dir(root_dir)` resolved where the vaults
    /// physically live. Pass `None` when no vault is linked; the sidebar
    /// then shows the "no linked vault" empty state rather than silently
    /// listing something else. Re-lists when the vault changes while the
    /// panel is open.
    pub fn set_notes_vault_root(&mut self, vault: Option<std::path::PathBuf>) {
        if self.notes_vault_root == vault {
            return;
        }
        self.notes_vault_root = vault;
        if self.notes_sidebar.is_visible() {
            self.apply_notes_vault();
            self.pending_notes_refresh = true;
        }
    }

    /// Give the notes sidebar the caret, taking it off the file tree so
    /// two left-hand panels never look focused at once.
    fn focus_notes_sidebar(&mut self) {
        if let Some(tree) = self.file_tree.as_mut() {
            tree.set_focused(false);
        }
        self.blur(PanelKey::FileTree);
        self.notes_sidebar.set_focused(true);
    }

    /// True when the agent's side panel currently owns the caret.
    fn agent_side_panel_focused(&self) -> bool {
        self.agent_pane
            .as_ref()
            .is_some_and(|pane| pane.side_panel().is_focused())
    }

    /// True when Alt+Right from the agent body should step INTO the
    /// agent side panel: the agent tab is showing, the panel has been
    /// laid out, it has something focusable, and nothing on the left
    /// (tree / notes) still holds the caret - otherwise Alt+Right from
    /// the tree would teleport past the composer straight into the
    /// panel. Same guard set desktop uses.
    fn agent_side_panel_focusable(&self) -> bool {
        if !self.is_neoism_agent_tab_active() {
            return false;
        }
        if self.focused() == Some(PanelKey::FileTree) {
            return false;
        }
        if self.notes_sidebar.is_visible() && self.notes_sidebar.is_focused() {
            return false;
        }
        self.agent_pane.as_ref().is_some_and(|pane| {
            !pane.side_panel().is_focused()
                && pane.side_panel().last_panel_rect().is_some()
                && pane.side_panel().focusable()
        })
    }

    /// Point the sidebar at the current vault and reflect the
    /// "no linked vault" state. Shared by the Alt+N open path and by
    /// [`Chrome::set_notes_vault_root`] when the vault moves underneath
    /// an already-open panel.
    fn apply_notes_vault(&mut self) {
        let vault = self.notes_vault_root.clone();
        let name = vault
            .as_deref()
            .and_then(|v| v.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Default".to_string());
        self.notes_sidebar.set_vault_actions(vault.is_none());
        self.notes_sidebar.set_workspace(name, vault);
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

    /// Toggle the notes sidebar (desktop Alt+N). Lists the vault the
    /// host pushed via [`Chrome::set_notes_vault_root`] - the SAME
    /// directory the desktop sidebar, the daemon's note-create action
    /// and the agent's notes tools all resolve. This used to guess
    /// `<workspace_root>/notes`, a directory the vault model never
    /// writes, so the panel listed an empty (or unrelated) folder while
    /// new notes landed in `~/Neoism/Vaults/...` and never appeared.
    pub fn toggle_notes_sidebar(&mut self) -> bool {
        if !self.notes_sidebar.is_visible() {
            self.apply_notes_vault();
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
        self.layout.notes_sidebar
    }

    /// Full pointer-owned area for the top bar, including its open menu.
    pub fn top_bar_pointer_rect(&self) -> Option<Rect> {
        self.rect_for(PanelKey::TopBar)
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

    /// Direct mobile scroll routing for chrome-owned surfaces. Deltas are
    /// finger movement in logical pixels, not wheel deltas. Every branch
    /// updates a bounded visual position immediately and creates no velocity.
    pub fn touch_scroll_at(&mut self, x: f32, y: f32, dx: f32, dy: f32) -> bool {
        if self.is_neoism_agent_tab_active()
            && self.agent_pane_mut().is_some_and(|pane| {
                if pane.side_panel().contains_point(x, y) {
                    let rows = pane.side_panel().last_panel_height_rows();
                    pane.scroll_side_panel_pixels(dy, rows);
                    true
                } else {
                    false
                }
            })
        {
            return true;
        }
        if self
            .agent_pane_mut()
            .is_some_and(|pane| pane.scroll_question_at(x, y, dy))
        {
            return true;
        }
        if self.settings_page.is_active() {
            let _ = self.settings_page.scroll_by(dy);
            return true;
        }
        if self.git_diff_panel.is_visible()
            && self.git_diff_panel.scroll_touch_at(x, y, dy)
        {
            return true;
        }
        let tabs = self.layout.buffer_tabs;
        if tabs.contains(x, y) {
            return self.buffer_tabs.scroll_touch_by(dx, tabs.w);
        }
        if let Some(bounds) = self.layout.file_tree {
            if bounds.contains(x, y) {
                if let Some(tree) = self.file_tree.as_mut() {
                    let rows = tree.visible_rows_for_panel_height(bounds.h);
                    return tree.scroll_touch_pixels(dy, rows);
                }
                return false;
            }
        }
        if let Some(bounds) = self.notes_sidebar_rect() {
            if bounds.contains(x, y) {
                let rows = self.notes_sidebar.visible_rows_for_panel_height(bounds.h);
                return self.notes_sidebar.scroll_touch_pixels(dy, rows);
            }
        }
        false
    }

    /// Position-owned wheel routing used by canvas hosts before dispatching
    /// to the active editor/agent/terminal surface. Returning `true` means the
    /// chrome region owns the event, not merely that its bounded offset moved.
    /// This distinction prevents scroll chaining at a sidebar/tab boundary.
    pub fn wheel_scroll_at(
        &mut self,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
        mode: WheelMode,
        shift: bool,
    ) -> bool {
        let event = UiEvent::Wheel {
            // BufferTabs historically receives inverted DOM horizontal input.
            dx: -dx,
            dy,
            mode,
            modifiers: if shift {
                Modifiers::SHIFT
            } else {
                Modifiers::empty()
            },
        };
        if self.handle_side_panel_pointer(&event, x, y) {
            return true;
        }

        let tabs = self.layout.buffer_tabs;
        let horizontal = dx.abs() > 0.5 || (shift && dy.abs() > 0.5);
        if horizontal && tabs.contains(x, y) {
            self.buffer_tabs.scroll_wheel(-dx, dy, mode);
            return true;
        }
        false
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
            if let Some(rect) = self.layout.file_tree {
                if rect.contains(wheel_px, wheel_py) {
                    if let Some(tree) = self.file_tree.as_mut() {
                        let panel_pixels = match mode {
                            WheelMode::Pixel => -*dy,
                            WheelMode::Line => -*dy * tree.row_height(),
                            WheelMode::Page => -*dy * rect.h,
                        };
                        let rows = tree.visible_rows_for_panel_height(rect.h);
                        tree.scroll_pixels(panel_pixels, rows);
                    }
                    return true;
                }
            }
            if let Some(rect) = self.notes_sidebar_rect() {
                if rect.contains(wheel_px, wheel_py) {
                    // Trackpad PIXEL scrolling, same accumulator model as
                    // the file tree: feed raw pixels so a slow drag eases
                    // a row at a time instead of jumping per wheel event.
                    let panel_pixels = match mode {
                        WheelMode::Pixel => -*dy,
                        WheelMode::Line => -*dy * self.notes_sidebar.row_height(),
                        WheelMode::Page => -*dy * rect.h,
                    };
                    let rows_visible =
                        self.notes_sidebar.visible_rows_for_panel_height(rect.h);
                    self.notes_sidebar.scroll_pixels(panel_pixels, rows_visible);
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
                    self.git_diff_panel.stage_all_toggle();
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
                    None => match self.top_bar.mobile_agent_panel_hit_rect() {
                        Some(hit) => Rect::new(
                            strip.x.min(hit.x),
                            strip.y.min(hit.y),
                            (strip.x + strip.w).max(hit.x + hit.w) - strip.x.min(hit.x),
                            (strip.y + strip.h).max(hit.y + hit.h) - strip.y.min(hit.y),
                        ),
                        None => strip,
                    },
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
            | PanelKey::TrailCursor
            | PanelKey::YankFlash => None,
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
            | PanelKey::TrailCursor
            | PanelKey::YankFlash => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn chrome_with_tree(width: f32) -> Chrome<()> {
        let mut tree = crate::panels::FileTree::new(PathBuf::from("/workspace"));
        tree.set_visible(true);
        tree.set_width(width);
        let mut chrome = Chrome::<()>::new();
        chrome.install_file_tree(tree);
        chrome
    }

    fn assert_surface_column_is_bounded(chrome: &Chrome<()>, viewport: Rect) {
        let layout = chrome.layout();
        let surface = layout.terminal;
        assert!(surface.w >= 0.0 && surface.h >= 0.0);
        assert!(surface.x >= viewport.x);
        assert!(surface.x + surface.w <= viewport.x + viewport.w);
        assert_eq!(layout.buffer_tabs.x, surface.x);
        assert_eq!(layout.buffer_tabs.w, surface.w);
        assert_eq!(layout.status_line.x, viewport.x);
        assert_eq!(layout.status_line.w, viewport.w);
        assert!(layout
            .top_bar
            .is_none_or(|bar| bar.x == viewport.x && bar.w == viewport.w));
    }

    #[test]
    fn alt_up_tab_focus_owns_the_chrome_focus_slot() {
        let mut chrome = Chrome::<()>::new();
        chrome.buffer_tabs.ensure_terminal_tab();

        // This is the state transition used by the Alt+Up event branch.
        assert!(chrome.focus_buffer_tabs());
        assert!(chrome.buffer_tabs.is_focused());
        assert_eq!(chrome.focused(), Some(PanelKey::BufferTabs));
    }

    #[test]
    fn tab_focus_cursor_keeps_its_thin_published_geometry() {
        let rect = [18.0, 42.0, 3.0, 20.0];
        assert_eq!(chrome_focus_cursor_animation_size(rect), (3.0, 20.0));
    }

    #[test]
    fn notes_hit_rect_is_confined_to_the_middle_band() {
        let viewport = Rect::new(0.0, 0.0, 1024.0, 768.0);
        let mut chrome = Chrome::<()>::new();
        chrome.notes_sidebar.set_visible(true);
        chrome.set_layout(viewport);

        let rect = chrome.notes_sidebar_rect().expect("visible notes sidebar");
        assert_eq!(rect.y, chrome.layout.buffer_tabs.y);
        assert_eq!(rect.y + rect.h, chrome.layout.status_line.y);
        assert!(chrome
            .layout
            .top_bar
            .is_none_or(|top| rect.y >= top.y + top.h));
    }

    #[test]
    fn screenshot_width_clamps_tree_and_notes_before_every_surface_column() {
        let viewport = Rect::new(0.0, 0.0, 820.0, 720.0);
        for open in ["tree", "notes", "both"] {
            let mut chrome = chrome_with_tree(600.0);
            if open == "notes" {
                chrome.file_tree.as_mut().unwrap().set_visible(false);
            }
            if open != "tree" {
                chrome.notes_sidebar.set_visible(true);
            }
            chrome.command_composer.set_visible(true);
            chrome.set_layout(viewport);
            assert_surface_column_is_bounded(&chrome, viewport);
            assert!(chrome.layout.terminal.w >= RESPONSIVE_CONTENT_MIN_W);
            let composer = chrome.layout.command_composer.expect("terminal composer");
            assert_eq!(composer.x, chrome.layout.terminal.x);
            assert_eq!(composer.w, chrome.layout.terminal.w);
            assert!(composer.x >= viewport.x);
            assert!(composer.x + composer.w <= viewport.x + viewport.w);
            for _surface in ["terminal", "code", "markdown", "agent"] {
                assert!(chrome.content_surface_contains(
                    chrome.layout.terminal.x + 1.0,
                    chrome.layout.terminal.y + 1.0
                ));
                assert!(!chrome.content_surface_contains(
                    chrome.layout.terminal.x - 1.0,
                    chrome.layout.terminal.y + 1.0
                ));
            }
        }
    }

    #[test]
    fn wide_viewport_does_not_expand_tree_past_its_configured_width() {
        let viewport = Rect::new(0.0, 0.0, 2560.0, 1440.0);
        let mut chrome = chrome_with_tree(280.0);
        chrome.set_layout(viewport);

        assert_eq!(chrome.layout.file_tree.unwrap().w, 280.0);
        assert_eq!(chrome.layout.terminal.x, 284.0);
    }

    #[test]
    fn agent_top_bar_action_stays_visible_outside_agent_tabs() {
        let mut chrome = Chrome::<()>::new();
        chrome.set_layout(Rect::new(0.0, 0.0, 1200.0, 800.0));

        assert!(!chrome.is_neoism_agent_tab_active());
        assert!(chrome.top_bar.is_right_button_visible());
        assert!(!chrome.top_bar.is_mobile_agent_panel_button_visible());
    }

    fn chrome_with_active_agent() -> Chrome<()> {
        let mut chrome = Chrome::<()>::new();
        chrome.install_agent_pane(
            crate::panels::agent_pane::state::NeoismAgentPane::default(),
        );
        chrome.buffer_tabs.set_tabs(
            vec![crate::panels::buffer_tabs::BufferTab {
                title: "Neoism Agent".into(),
                modified: false,
                custom_icon: None,
                path: None,
                markdown: false,
                terminal_route_id: None,
                neoism_agent_route_id: Some(1),
                chrome_page: None,
                agent_kind: None,
            }],
            0,
        );
        chrome.set_active_tab_index(0);
        chrome
    }

    #[test]
    fn mobile_agent_toggle_opens_and_closes_full_content_takeover() {
        let viewport = Rect::new(0.0, 0.0, 390.0, 844.0);
        let mut chrome = chrome_with_active_agent();
        chrome.set_mobile_web_agent_panel_enabled(true);
        chrome.set_layout(viewport);

        assert!(chrome.top_bar.is_mobile_agent_panel_button_visible());
        assert!(!chrome.agent_side_panel_takeover_active());
        assert!(chrome.content_surface_available());

        chrome.apply_top_bar_action(TopBarAction::ToggleAgentSidePanel);
        assert!(chrome.agent_side_panel_takeover_active());
        assert!(!chrome.content_surface_available());
        chrome
            .agent_pane_mut()
            .unwrap()
            .set_cursor_rect(Some([20.0, 700.0, 2.0, 18.0]));
        assert_eq!(
            chrome.chrome_trail_cursor_rect(
                crate::chrome_policy::TrailCursorOverlayTarget::AgentInput,
                None,
            ),
            None
        );
        assert!(chrome.layout.top_bar.is_some());
        assert!(chrome.layout.status_line.h > 0.0);
        let content = chrome.focused_content_rect();
        assert!(
            content.y
                >= chrome.layout.top_bar.unwrap().y + chrome.layout.top_bar.unwrap().h
        );
        assert!(content.y + content.h <= chrome.layout.status_line.y);

        chrome.apply_top_bar_action(TopBarAction::ToggleAgentSidePanel);
        assert!(!chrome.agent_side_panel_takeover_active());
        assert!(chrome.content_surface_available());
    }

    #[test]
    fn desktop_never_exposes_mobile_agent_toggle_and_resize_restores_policy() {
        let phone = Rect::new(0.0, 0.0, 390.0, 844.0);
        let desktop = Rect::new(0.0, 0.0, 1200.0, 800.0);

        let mut native = chrome_with_active_agent();
        native.set_layout(phone);
        assert!(!native.top_bar.is_mobile_agent_panel_button_visible());
        assert!(!native.agent_side_panel_takeover_active());

        let mut web = chrome_with_active_agent();
        assert!(!web.agent_pane().unwrap().side_panel().user_hidden());
        web.set_mobile_web_agent_panel_enabled(true);
        web.set_layout(phone);
        assert!(web.agent_pane().unwrap().side_panel().user_hidden());
        web.apply_top_bar_action(TopBarAction::ToggleAgentSidePanel);
        assert!(web.agent_side_panel_takeover_active());
        web.set_layout(desktop);
        assert!(!web.top_bar.is_mobile_agent_panel_button_visible());
        assert!(!web.agent_side_panel_takeover_active());
        assert!(!web.agent_pane().unwrap().side_panel().user_hidden());
        assert!(web.content_surface_available());
    }

    #[test]
    fn desktop_sidebars_stay_partial_when_git_reduces_the_content_band() {
        let viewport = Rect::new(0.0, 0.0, 820.0, 720.0);
        for open in ["tree", "notes", "both"] {
            let mut chrome = chrome_with_tree(280.0);
            if open == "notes" {
                chrome.file_tree.as_mut().unwrap().set_visible(false);
            }
            if open != "tree" {
                chrome.notes_sidebar.set_visible(true);
            }
            assert!(chrome.toggle_git_diff_panel());
            chrome.set_layout(viewport);

            assert!(chrome.layout.terminal.w > 0.0);
            assert!(chrome.content_surface_available());
            if let Some(tree) = chrome.layout.file_tree {
                assert!(tree.w < viewport.w);
            }
            if let Some(notes) = chrome.layout.notes_sidebar {
                assert!(notes.w < viewport.w);
            }
        }
    }

    #[test]
    fn phone_tree_and_notes_take_over_and_disable_underlying_surfaces() {
        let viewport = Rect::new(0.0, 0.0, 390.0, 844.0);
        for panel in ["tree", "notes"] {
            let mut chrome = chrome_with_tree(600.0);
            if panel == "notes" {
                chrome.file_tree.as_mut().unwrap().set_visible(false);
                chrome.notes_sidebar.set_visible(true);
            }
            chrome.set_layout(viewport);
            assert_surface_column_is_bounded(&chrome, viewport);
            assert_eq!(chrome.layout.terminal.w, 0.0);
            assert_eq!(chrome.layout.command_composer, None);
            let sidebar = chrome
                .layout
                .file_tree
                .or(chrome.layout.notes_sidebar)
                .unwrap();
            assert_eq!(sidebar.x, viewport.x);
            assert_eq!(sidebar.w, viewport.w);
            for _surface in ["terminal", "code", "markdown", "agent"] {
                assert!(!chrome.content_surface_contains(200.0, 300.0));
            }
        }
    }

    #[test]
    fn mobile_keyboard_inset_excludes_full_width_status_line() {
        let viewport = Rect::new(0.0, 0.0, 390.0, 844.0);
        let mut chrome = Chrome::<()>::new();
        chrome.set_layout(viewport);
        let status = chrome.layout.status_line;
        chrome.set_bottom_content_inset(301.0);
        assert_eq!(chrome.layout.status_line, status);
        assert_eq!(chrome.layout.status_line.w, viewport.w);
        if let Some(composer) = chrome.layout.command_composer {
            assert_eq!(composer.y + composer.h, 543.0);
            assert_eq!(
                chrome.layout.terminal.y + chrome.layout.terminal.h,
                composer.y
            );
        } else {
            assert_eq!(chrome.layout.terminal.y + chrome.layout.terminal.h, 543.0);
        }
    }

    #[test]
    fn keyboard_overlay_suppresses_terminal_composer_without_moving_status() {
        let viewport = Rect::new(0.0, 0.0, 390.0, 844.0);
        let mut chrome = Chrome::<()>::new();
        chrome.command_composer.set_visible(true);
        chrome.set_bottom_content_inset(301.0);
        chrome.set_layout(viewport);
        let status = chrome.layout.status_line;
        assert!(chrome.terminal_composer_eligible());
        assert!(chrome.layout.command_composer.is_some());

        chrome.command_palette.set_enabled(true);
        chrome.set_layout(viewport);
        assert!(!chrome.terminal_composer_eligible());
        assert_eq!(chrome.layout.command_composer, None);
        assert_eq!(chrome.layout.terminal.y + chrome.layout.terminal.h, 543.0);
        assert_eq!(chrome.layout.status_line, status);

        chrome.command_palette.set_enabled(false);
        chrome.set_layout(viewport);
        assert!(chrome.terminal_composer_eligible());
        assert!(chrome.layout.command_composer.is_some());
        assert_eq!(chrome.layout.status_line, status);
    }

    #[test]
    fn visible_center_modal_is_first_in_pointer_and_wheel_z_order() {
        let mut chrome = Chrome::<()>::new();
        chrome.set_layout(Rect::new(0.0, 0.0, 1000.0, 700.0));
        chrome.command_palette.set_enabled(true);

        for event in [
            UiEvent::PointerDown {
                button: crate::event::PointerButton::Left,
                x: 500.0,
                y: 120.0,
                modifiers: Modifiers::empty(),
                click_count: 1,
            },
            UiEvent::Wheel {
                dx: 0.0,
                dy: 30.0,
                mode: WheelMode::Pixel,
                modifiers: Modifiers::empty(),
            },
        ] {
            assert_eq!(
                chrome.event_priority_order(&event).first(),
                Some(&PanelKey::CommandPalette)
            );
        }
        assert_eq!(
            chrome.active_pointer_modal_rect(),
            chrome.layout.command_palette
        );
    }
}
