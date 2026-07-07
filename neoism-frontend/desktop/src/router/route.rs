use crate::router::routes::{assistant, RoutePath};
use crate::router::window::RouteWindow;
use neoism_backend::clipboard::Clipboard;
use neoism_backend::config::Config as RioConfig;
use neoism_backend::error::{RioError, RioErrorType};
use neoism_ui::editor::scroll_model::{parse_ex_command, GlobalExCommandPlan};
use neoism_window::keyboard::{Key, NamedKey};
use neoism_window::platform::modifier_supplement::KeyEventExtModifierSupplement;
use std::time::{Duration, Instant};

const REDRAW_RETRY_INITIAL_AFTER: Duration = Duration::from_millis(250);
const REDRAW_RETRY_MAX_AFTER: Duration = Duration::from_secs(2);

#[derive(Debug, Clone)]
struct RedrawRequestState {
    pending: bool,
    requested_at: Option<Instant>,
    retry_after: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RedrawRequestDecision {
    Request,
    Retry {
        age: Duration,
        previous_retry_after: Duration,
    },
    Suppress {
        retry_at: Instant,
    },
}

impl RedrawRequestState {
    fn new() -> Self {
        Self {
            pending: false,
            requested_at: None,
            retry_after: REDRAW_RETRY_INITIAL_AFTER,
        }
    }

    #[inline]
    fn is_pending(&self) -> bool {
        self.pending
    }

    #[inline]
    fn retry_deadline(&self) -> Option<Instant> {
        self.requested_at
            .filter(|_| self.pending)
            .map(|requested_at| requested_at + self.retry_after)
    }

    fn request(&mut self, now: Instant) -> RedrawRequestDecision {
        let Some(requested_at) = self.requested_at.filter(|_| self.pending) else {
            self.pending = true;
            self.requested_at = Some(now);
            self.retry_after = REDRAW_RETRY_INITIAL_AFTER;
            return RedrawRequestDecision::Request;
        };

        let retry_at = requested_at + self.retry_after;
        if now < retry_at {
            return RedrawRequestDecision::Suppress { retry_at };
        }

        let previous_retry_after = self.retry_after;
        self.requested_at = Some(now);
        self.retry_after = self
            .retry_after
            .saturating_mul(2)
            .min(REDRAW_RETRY_MAX_AFTER);
        RedrawRequestDecision::Retry {
            age: now.saturating_duration_since(requested_at),
            previous_retry_after,
        }
    }

    #[inline]
    fn delivered(&mut self) {
        self.pending = false;
        self.requested_at = None;
        self.retry_after = REDRAW_RETRY_INITIAL_AFTER;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redraw_request_state_suppresses_duplicate_until_retry_deadline() {
        let mut state = RedrawRequestState::new();
        let t0 = Instant::now();

        assert_eq!(state.request(t0), RedrawRequestDecision::Request);
        assert!(state.is_pending());

        let early = t0 + REDRAW_RETRY_INITIAL_AFTER / 2;
        assert_eq!(
            state.request(early),
            RedrawRequestDecision::Suppress {
                retry_at: t0 + REDRAW_RETRY_INITIAL_AFTER,
            }
        );
        assert_eq!(
            state.retry_deadline(),
            Some(t0 + REDRAW_RETRY_INITIAL_AFTER)
        );
    }

    #[test]
    fn redraw_request_state_retries_stale_delivery_with_backoff() {
        let mut state = RedrawRequestState::new();
        let t0 = Instant::now();
        assert_eq!(state.request(t0), RedrawRequestDecision::Request);

        let first_retry = t0 + REDRAW_RETRY_INITIAL_AFTER;
        assert_eq!(
            state.request(first_retry),
            RedrawRequestDecision::Retry {
                age: REDRAW_RETRY_INITIAL_AFTER,
                previous_retry_after: REDRAW_RETRY_INITIAL_AFTER,
            }
        );
        assert_eq!(
            state.retry_deadline(),
            Some(first_retry + REDRAW_RETRY_INITIAL_AFTER.saturating_mul(2))
        );

        let second_retry = first_retry + REDRAW_RETRY_INITIAL_AFTER.saturating_mul(2);
        assert_eq!(
            state.request(second_retry),
            RedrawRequestDecision::Retry {
                age: REDRAW_RETRY_INITIAL_AFTER.saturating_mul(2),
                previous_retry_after: REDRAW_RETRY_INITIAL_AFTER.saturating_mul(2),
            }
        );
    }

    #[test]
    fn redraw_request_state_delivery_resets_backoff() {
        let mut state = RedrawRequestState::new();
        let t0 = Instant::now();
        assert_eq!(state.request(t0), RedrawRequestDecision::Request);
        assert_eq!(
            state.request(t0 + REDRAW_RETRY_INITIAL_AFTER),
            RedrawRequestDecision::Retry {
                age: REDRAW_RETRY_INITIAL_AFTER,
                previous_retry_after: REDRAW_RETRY_INITIAL_AFTER,
            }
        );

        state.delivered();
        assert!(!state.is_pending());
        assert_eq!(state.retry_deadline(), None);
        assert_eq!(
            state.request(t0 + Duration::from_secs(10)),
            RedrawRequestDecision::Request
        );
        assert_eq!(
            state.retry_deadline(),
            Some(t0 + Duration::from_secs(10) + REDRAW_RETRY_INITIAL_AFTER)
        );
    }
}

fn is_enter_key(key: &Key) -> bool {
    match key {
        Key::Named(NamedKey::Enter) => true,
        Key::Character(text) => text == "\r" || text == "\n",
        _ => false,
    }
}

fn palette_close_plan(query: &str) -> Option<GlobalExCommandPlan> {
    let cmd = query.trim().trim_start_matches(':').trim();
    let (head, tail) = parse_ex_command(cmd)?;
    match GlobalExCommandPlan::classify(&head, &tail) {
        plan @ (GlobalExCommandPlan::CloseFocusedBufferTab
        | GlobalExCommandPlan::CloseAllBuffersInFocusedPaneOrWorkspace) => Some(plan),
        _ => None,
    }
}

pub struct Route<'a> {
    pub assistant: assistant::Assistant,
    pub path: RoutePath,
    pub window: RouteWindow<'a>,
    redraw_request: RedrawRequestState,
}

impl Route<'_> {
    /// Create a performer.
    #[inline]
    pub fn new(
        assistant: assistant::Assistant,
        path: RoutePath,
        window: RouteWindow,
    ) -> Route {
        Route {
            assistant,
            path,
            window,
            redraw_request: RedrawRequestState::new(),
        }
    }
}

impl Route<'_> {
    #[inline]
    pub fn request_redraw(&mut self) -> bool {
        self.request_redraw_with_reason("Route::request_redraw")
    }

    #[inline]
    pub fn request_redraw_with_reason(&mut self, reason: &str) -> bool {
        let now = Instant::now();
        let decision = self.redraw_request.request(now);
        match decision {
            RedrawRequestDecision::Request => {}
            RedrawRequestDecision::Retry {
                age,
                previous_retry_after,
            } => {
                crate::app::freeze_watchdog::note(format!(
                    "redraw_retry_rearmed window={:?} age_ms={} previous_retry_after_ms={} reason={}",
                    self.window.winit_window.id(),
                    age.as_millis(),
                    previous_retry_after.as_millis(),
                    reason
                ));
                tracing::warn!(
                    target: "neoism::redraw",
                    window_id = ?self.window.winit_window.id(),
                    age_ms = age.as_millis(),
                    retry_after_ms = previous_retry_after.as_millis(),
                    reason,
                    "redraw delivery stale; re-requesting"
                );
            }
            RedrawRequestDecision::Suppress { .. } => return false,
        }
        crate::app::freeze_watchdog::mark_redraw_requested(
            self.window.winit_window.id(),
            reason,
        );
        self.window.winit_window.request_redraw();
        true
    }

    #[inline]
    pub fn mark_redraw_delivered(&mut self) {
        self.redraw_request.delivered();
    }

    #[inline]
    pub fn redraw_request_pending(&self) -> bool {
        self.redraw_request.is_pending()
    }

    #[inline]
    pub fn redraw_retry_deadline(&self) -> Option<Instant> {
        self.redraw_request.retry_deadline()
    }

    /// Mark the active context dirty (UI-only) and request a redraw
    /// at the next vsync. Used by overlay input paths (command palette,
    /// assistant, island rename) where the UI changed but terminal
    /// cells didn't. `set_dirty` passes `Renderer::run`'s per-context
    /// gate; the inner damage match hits
    /// `(None, None) => TerminalDamage::Noop` so rows don't rebuild,
    /// and the overlay itself is drawn unconditionally after the loop.
    #[inline]
    pub fn request_overlay_redraw(&mut self) {
        self.window
            .screen
            .ctx_mut()
            .current_mut()
            .renderable_content
            .pending_update
            .set_dirty();
        self.request_redraw();
    }

    #[inline]
    pub fn begin_render(&mut self) {
        self.window.update_vblank_interval();
        let now = Instant::now();
        self.window.record_frame_cadence(now);
        self.window.render_timestamp = now;
    }

    /// Push the current `/`-search query to the lua side so it can
    /// build the buffer-match list. Empty queries clear the match
    /// list explicitly so the dropdown drops back to recent-history.
    /// `true` when the open Search-mode palette should be answered from
    /// the current *markdown* pane (neoism's own engine) rather than the
    /// nvim editor. The two share the exact same palette modal + preview
    /// contract; only the match source differs.
    fn palette_search_is_markdown(&self) -> bool {
        self.window.screen.renderer.command_palette.is_search_mode()
            && self
                .window
                .screen
                .context_manager
                .current()
                .active_markdown()
                .is_some()
    }

    fn dispatch_palette_search_query(&mut self, query: &str) {
        // Markdown pane: scan the buffer locally and push the matches into
        // the SAME palette `buffer_matches` the nvim path fills, then
        // preview the auto-selected (nearest) match — no nvim round-trip.
        if self.palette_search_is_markdown() {
            let pairs = self
                .window
                .screen
                .context_manager
                .current_mut()
                .active_markdown_mut()
                .map(|md| md.search_scan(query))
                .unwrap_or_default();
            self.window
                .screen
                .renderer
                .command_palette
                .set_buffer_matches(pairs);
            self.preview_palette_search_match_if_any();
            return;
        }
        // No-op when there's no editor pane attached — `send_editor_command`
        // already handles that, but skipping avoids an alloc on terminal-
        // only routes.
        let cmd = neoism_backend::performer::nvim::vim_search_query_command(query);
        self.window.screen.send_editor_command(cmd);
    }

    /// Send the lua side a preview-line command for whichever buffer
    /// match the palette is currently highlighting. No-op when the
    /// selection isn't a buffer match (e.g. recent-history row).
    fn preview_palette_search_match_if_any(&mut self) {
        let location = self
            .window
            .screen
            .renderer
            .command_palette
            .selected_buffer_match_location();
        // Markdown: jump + highlight in the markdown buffer behind the modal.
        if self.palette_search_is_markdown() {
            if let Some((lnum, col)) = location {
                if let Some(md) = self
                    .window
                    .screen
                    .context_manager
                    .current_mut()
                    .active_markdown_mut()
                {
                    md.search_preview(lnum, col);
                }
                self.window.screen.mark_dirty();
            }
            return;
        }
        if let Some((lnum, col)) = location {
            let query = self.window.screen.renderer.command_palette.query.clone();
            let cmd = neoism_backend::performer::nvim::vim_search_preview_command(
                lnum, col, &query,
            );
            self.window.screen.send_editor_command(cmd);
        }
    }

    #[inline]
    pub fn update_config(
        &mut self,
        config: &RioConfig,
        db: &neoism_backend::sugarloaf::font::FontLibrary,
        should_update_font: bool,
    ) {
        self.window
            .screen
            .update_config(config, db, should_update_font);
    }

    #[inline]
    #[allow(unused_variables)]
    pub fn set_window_subtitle(&mut self, subtitle: &str) {
        #[cfg(target_os = "macos")]
        self.window.winit_window.set_subtitle(subtitle);
    }

    #[inline]
    pub fn set_window_title(&mut self, title: &str) {
        self.window.winit_window.set_title(title);
    }

    #[inline]
    pub fn report_error(&mut self, error: &RioError) {
        if error.report == RioErrorType::ConfigurationNotFound {
            self.path = RoutePath::Welcome;
            return;
        }

        self.assistant.set(error.to_owned());
        self.window
            .screen
            .renderer
            .assistant
            .set_error(error.to_owned());
    }

    #[inline]
    pub fn clear_errors(&mut self) {
        self.assistant.clear();
        self.window.screen.renderer.assistant.clear();
        self.path = RoutePath::Terminal;
    }

    #[inline]
    pub fn confirm_quit(&mut self) {
        self.path = RoutePath::ConfirmQuit;
    }

    #[inline]
    pub fn quit(&mut self) {
        std::process::exit(0);
    }

    #[inline]
    pub fn has_key_wait(
        &mut self,
        key_event: &neoism_window::event::KeyEvent,
        clipboard: &mut Clipboard,
    ) -> bool {
        use neoism_window::event::ElementState;

        tracing::trace!(
            target: "neoism::input",
            route_path = ?self.path,
            state = ?key_event.state,
            repeat = key_event.repeat,
            logical_key = ?key_event.logical_key,
            physical_key = ?key_event.physical_key,
            location = ?key_event.location,
            text = ?key_event.text,
            text_with_all_modifiers = ?key_event.text_with_all_modifiers(),
            command_palette_enabled = self.window.screen.renderer.command_palette.is_enabled(),
            assistant_active = self.window.screen.renderer.assistant.is_active(),
            file_tree_focused = self.window.screen.renderer.file_tree.is_focused(),
            "route key-wait entered"
        );

        if key_event.state == ElementState::Pressed
            && matches!(
                key_event.logical_key,
                Key::Named(
                    NamedKey::ArrowUp
                        | NamedKey::ArrowDown
                        | NamedKey::PageUp
                        | NamedKey::PageDown
                )
            )
            && self.window.screen.dismiss_lsp_hover()
        {
            self.request_overlay_redraw();
        }

        if self.window.screen.handle_app_global_shortcut(key_event) {
            if key_event.state == ElementState::Pressed {
                self.assistant.clear();
            }
            self.request_overlay_redraw();
            return true;
        }

        // Handle island color picker / rename input
        if let Some(ref mut island) = self.window.screen.renderer.island {
            if island.is_color_picker_open() {
                let consumed =
                    crate::app::window_event::keyboard::island_rename_key_from_winit(
                        key_event,
                    )
                    .map(|key| island.handle_rename_input(key))
                    .unwrap_or(false);
                tracing::trace!(
                    target: "neoism::input",
                    consumed,
                    "island color picker key handler completed"
                );
                if consumed {
                    self.request_overlay_redraw();
                    tracing::trace!(
                        target: "neoism::input",
                        "route key-wait returning true: island consumed key"
                    );
                    return true;
                }
            }
        }

        // Diagnostics popup keyboard interaction. Treated as a
        // foreground overlay: while open it claims Esc/↑/↓/Enter so
        // the user can scroll the list and jump without nvim seeing
        // the keys, but anything else falls through to whatever's
        // underneath (palette, editor, etc.).
        if self.window.screen.renderer.diagnostics_popup.is_visible()
            && self
                .window
                .screen
                .renderer
                .diagnostics_popup
                .is_interactive()
            && key_event.state == ElementState::Pressed
        {
            match &key_event.logical_key {
                Key::Named(NamedKey::Escape) => {
                    self.window.screen.renderer.diagnostics_popup.close();
                    self.request_overlay_redraw();
                    return true;
                }
                Key::Named(NamedKey::ArrowUp) => {
                    self.window.screen.renderer.diagnostics_popup.move_up();
                    self.request_overlay_redraw();
                    return true;
                }
                Key::Named(NamedKey::ArrowDown) => {
                    self.window.screen.renderer.diagnostics_popup.move_down();
                    self.request_overlay_redraw();
                    return true;
                }
                key if is_enter_key(key) => {
                    if let Some(lnum) = self
                        .window
                        .screen
                        .renderer
                        .diagnostics_popup
                        .selected_lnum()
                    {
                        self.window.screen.jump_to_diagnostic_line(lnum);
                    }
                    self.window.screen.renderer.diagnostics_popup.close();
                    self.request_overlay_redraw();
                    return true;
                }
                _ => {}
            }
        }

        // Right-side git diff panel — claims Esc only. Arrow keys and
        // PgUp/PgDn fall through to the editor underneath so the panel
        // doesn't hijack normal navigation while it's pinned open.
        if self.window.screen.renderer.git_diff_panel.is_visible()
            && key_event.state == ElementState::Pressed
        {
            if let Key::Named(NamedKey::Escape) = &key_event.logical_key {
                // Esc while the branch dropdown is open closes JUST the
                // dropdown — the panel stays open. Only a second Esc (menu
                // already closed) dismisses the whole panel.
                if self
                    .window
                    .screen
                    .renderer
                    .git_diff_panel
                    .branch_menu_is_open()
                {
                    self.window.screen.renderer.git_diff_panel.close_branch_menu();
                } else {
                    self.window.screen.close_git_diff_panel();
                }
                self.request_overlay_redraw();
                return true;
            }
        }

        // Universal modal is Rust-owned chrome. Action pickers are
        // blocking; passive install/status/error overlays are not allowed
        // to steal editor input.
        if self.window.screen.renderer.modal.is_active() {
            let blocking = self.window.screen.renderer.modal.is_blocking();
            if key_event.state == ElementState::Pressed {
                let modal_has_input = self.window.screen.renderer.modal.has_input();
                match &key_event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        if let Some(action) =
                            self.window.screen.renderer.modal.escape_action()
                        {
                            self.window.screen.execute_modal_action(action);
                        } else {
                            self.window.screen.renderer.modal.close();
                        }
                        self.request_overlay_redraw();
                        return true;
                    }
                    Key::Named(NamedKey::Backspace) if blocking && modal_has_input => {
                        self.window.screen.renderer.modal.pop_input();
                        self.request_overlay_redraw();
                        return true;
                    }
                    Key::Named(NamedKey::ArrowUp) if blocking => {
                        self.window.screen.renderer.modal.move_selection_up();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::ArrowDown) if blocking => {
                        self.window.screen.renderer.modal.move_selection_down();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::PageUp) if blocking => {
                        self.window.screen.renderer.modal.scroll_body_page(false);
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::PageDown) if blocking => {
                        self.window.screen.renderer.modal.scroll_body_page(true);
                        self.request_overlay_redraw();
                    }
                    key if blocking && is_enter_key(key) => {
                        if let Some(action) =
                            self.window.screen.renderer.modal.selected_action()
                        {
                            self.window.screen.execute_modal_action(action);
                        }
                        self.request_overlay_redraw();
                    }
                    Key::Character(text) if blocking && !modal_has_input => {
                        if let Some(action) =
                            self.window.screen.renderer.modal.action_for_hint(text)
                        {
                            self.window.screen.execute_modal_action(action);
                            self.request_overlay_redraw();
                            return true;
                        }
                    }
                    _ if blocking && modal_has_input => {
                        let mods = self.window.screen.modifiers.state();
                        if !mods.control_key() && !mods.alt_key() && !mods.super_key() {
                            if let Some(text) = key_event.text.as_deref() {
                                if !text.is_empty()
                                    && text.chars().all(|c| !c.is_control())
                                {
                                    self.window.screen.renderer.modal.push_input(text);
                                    self.request_overlay_redraw();
                                    return true;
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            if blocking {
                return true;
            }
        }

        // Finder overlay (`<leader>f f` / `<leader>f w`) input. Sits
        // ahead of the command palette block so the finder gets
        // exclusive key capture while it's open.
        if self.window.screen.renderer.finder.is_enabled() {
            tracing::trace!(
                target: "neoism::input",
                state = ?key_event.state,
                logical_key = ?key_event.logical_key,
                query = %self.window.screen.renderer.finder.query.as_str(),
                "finder handling key"
            );
            if key_event.state == ElementState::Pressed {
                let mods = self.window.screen.modifiers.state();
                match &key_event.logical_key {
                    Key::Named(NamedKey::Tab)
                        if mods.control_key() && !mods.alt_key() && !mods.super_key() =>
                    {
                        self.window.screen.renderer.finder.cycle_search_mode();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::Escape) => {
                        self.window.screen.renderer.finder.close();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.window.screen.renderer.finder.move_selection_up();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        // Visible-rows count is approximate — the
                        // renderer recomputes its real value from the
                        // overlay height each frame, but for navigation
                        // bookkeeping a generous default is fine.
                        self.window.screen.renderer.finder.move_selection_down(18);
                        self.request_overlay_redraw();
                    }
                    key if is_enter_key(key) => {
                        self.window.screen.open_finder_selection();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::Backspace) => {
                        let current = self.window.screen.renderer.finder.query.clone();
                        if !current.is_empty() {
                            let mut chars = current.chars().collect::<Vec<_>>();
                            chars.pop();
                            self.window
                                .screen
                                .renderer
                                .finder
                                .set_query(chars.into_iter().collect());
                            self.request_overlay_redraw();
                        }
                    }
                    _ => {
                        if let Some(text) = key_event
                            .text_with_all_modifiers()
                            .or(key_event.text.as_deref())
                        {
                            let text_str = text;
                            if !text_str.is_empty()
                                && text_str.chars().all(|c| !c.is_control())
                            {
                                let current =
                                    self.window.screen.renderer.finder.query.clone();
                                self.window
                                    .screen
                                    .renderer
                                    .finder
                                    .set_query(format!("{}{}", current, text_str));
                                self.request_overlay_redraw();
                            }
                        }
                    }
                }
            }
            return true;
        }

        // Handle command palette input first (works in all routes)
        if self.window.screen.renderer.command_palette.is_enabled() {
            tracing::trace!(
                target: "neoism::input",
                state = ?key_event.state,
                logical_key = ?key_event.logical_key,
                query = %self.window.screen.renderer.command_palette.query.as_str(),
                "command palette handling key"
            );
            if key_event.state == ElementState::Pressed {
                match &key_event.logical_key {
                    Key::Named(NamedKey::Escape) => {
                        tracing::trace!(target: "neoism::input", "command palette closing on Escape");
                        let was_search =
                            self.window.screen.renderer.command_palette.is_search_mode();
                        let was_markdown_search = self.palette_search_is_markdown();
                        self.window
                            .screen
                            .renderer
                            .command_palette
                            .set_enabled(false);
                        // Drop the live preview highlight when the user bails
                        // out of `/` without committing — otherwise the last-
                        // previewed match stays highlighted behind us. For a
                        // markdown pane this also restores the pre-search view.
                        if was_search {
                            if was_markdown_search {
                                if let Some(md) = self
                                    .window
                                    .screen
                                    .context_manager
                                    .current_mut()
                                    .active_markdown_mut()
                                {
                                    md.search_cancel();
                                }
                                self.window.screen.mark_dirty();
                            } else {
                                self.window.screen.send_editor_command(
                                    neoism_backend::performer::nvim::vim_search_clear_command(
                                    ),
                                );
                            }
                        }
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        tracing::trace!(target: "neoism::input", "command palette selection up");
                        self.window
                            .screen
                            .renderer
                            .command_palette
                            .move_selection_up();
                        self.preview_palette_search_match_if_any();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        tracing::trace!(target: "neoism::input", "command palette selection down");
                        self.window
                            .screen
                            .renderer
                            .command_palette
                            .move_selection_down();
                        self.preview_palette_search_match_if_any();
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::Tab) => {
                        tracing::trace!(target: "neoism::input", "command palette tab completes selection");
                        // Tab fills the query with the selected row's
                        // title — noice / snacks-style completion.
                        // If nothing changed (e.g. empty list, or
                        // already at full title), fall through to a
                        // selection-down so Tab still feels useful.
                        let was_search =
                            self.window.screen.renderer.command_palette.is_search_mode();
                        let completed =
                            self.window.screen.renderer.command_palette.tab_complete();
                        if completed && was_search {
                            let query =
                                self.window.screen.renderer.command_palette.query.clone();
                            self.dispatch_palette_search_query(&query);
                        } else if !completed {
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .move_selection_down();
                            self.preview_palette_search_match_if_any();
                        }
                        self.request_overlay_redraw();
                    }
                    key if is_enter_key(key) => {
                        tracing::trace!(target: "neoism::input", "command palette activating selection");
                        // Ex / Search modes short-circuit the rest of
                        // Enter handling: snapshot the typed query,
                        // close the palette, then forward to nvim as
                        // if the user had typed `:<query><CR>` (ex) or
                        // `/<query><CR>` (search) themselves.
                        let ex = self.window.screen.renderer.command_palette.is_ex_mode();
                        let search =
                            self.window.screen.renderer.command_palette.is_search_mode();
                        if ex || search {
                            // Search-mode: if the user has typed a
                            // query AND the selected row is a buffer
                            // match, commit that exact match location.
                            if search {
                                let typed = self
                                    .window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .query
                                    .clone();
                                let selected_location = self
                                    .window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .selected_buffer_match_location();
                                if let Some(location) = selected_location {
                                    let is_markdown = self.palette_search_is_markdown();
                                    self.window
                                        .screen
                                        .renderer
                                        .command_palette
                                        .set_enabled(false);
                                    if !typed.is_empty() {
                                        self.window
                                            .screen
                                            .renderer
                                            .command_palette
                                            .push_recent_search(typed.clone());
                                        if is_markdown {
                                            // Land the cursor on the match +
                                            // hand the pattern to `n`/`N`.
                                            if let Some(md) = self
                                                .window
                                                .screen
                                                .context_manager
                                                .current_mut()
                                                .active_markdown_mut()
                                            {
                                                md.search_commit(location.0, location.1);
                                            }
                                            self.window.screen.mark_dirty();
                                        } else {
                                            let cmd = self
                                                .window
                                                .screen
                                                .palette_search_commit_command(
                                                    &typed,
                                                    Some(location),
                                                );
                                            self.window.screen.send_editor_command(cmd);
                                        }
                                    }
                                    self.request_overlay_redraw();
                                    return true;
                                }
                            }
                            // Search: empty query + selected recent
                            // dispatches that term. Ex: no-arg query +
                            // selected suggestion dispatches the
                            // canonical command name, so `lspinfo` / `lsp`
                            // + Enter runs `LspInfo` instead of forwarding
                            // a lowercase command nvim would reject.
                            let typed =
                                self.window.screen.renderer.command_palette.query.clone();
                            let selected_recent = if search && typed.is_empty() {
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .get_selected_search_term()
                            } else {
                                None
                            };
                            let selected_ex = if ex
                                && !typed.trim().is_empty()
                                && !typed.contains(char::is_whitespace)
                            {
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .get_selected_ex_command()
                            } else {
                                None
                            };
                            let payload = selected_recent
                                .or(selected_ex)
                                .unwrap_or_else(|| typed.clone());
                            // Capture before `set_enabled(false)` flips the
                            // palette out of Search mode.
                            let search_is_markdown =
                                search && self.palette_search_is_markdown();
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .set_enabled(false);
                            let ex_payload = payload.trim();
                            if ex
                                && (ex_payload.eq_ignore_ascii_case("ThemePicker")
                                    || ex_payload.eq_ignore_ascii_case("theme picker"))
                            {
                                self.window.screen.open_theme_picker();
                                self.request_overlay_redraw();
                                return true;
                            }
                            if ex
                                && (ex_payload.eq_ignore_ascii_case("Shaders")
                                    || ex_payload.eq_ignore_ascii_case("ShaderPicker")
                                    || ex_payload.eq_ignore_ascii_case("shader picker"))
                            {
                                self.window.screen.open_shader_picker();
                                self.request_overlay_redraw();
                                return true;
                            }
                            if ex
                                && self.window.screen.try_intercept_ex_command(ex_payload)
                            {
                                self.request_overlay_redraw();
                                return true;
                            }
                            if ex && !ex_payload.is_empty() {
                                let cmd =
                                    neoism_backend::performer::nvim::vim_run_ex_command(
                                        ex_payload,
                                    );
                                tracing::trace!(
                                    target: "neoism::input",
                                    mode = "ex",
                                    cmd = %cmd,
                                    "palette dispatching to nvim"
                                );
                                self.window.screen.send_editor_command(cmd);
                            } else if search && !payload.is_empty() {
                                // Record the search so it appears as a
                                // recent next time. Stored pre-`/` so
                                // the suggestion list shows the bare
                                // pattern.
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .push_recent_search(payload.clone());
                                if search_is_markdown {
                                    // Recent/no-selection commit on a markdown
                                    // pane: scan the payload and land on the
                                    // nearest match (or restore if none).
                                    if let Some(md) = self
                                        .window
                                        .screen
                                        .context_manager
                                        .current_mut()
                                        .active_markdown_mut()
                                    {
                                        let first = md
                                            .search_scan(&payload)
                                            .first()
                                            .map(|(lnum, col, _)| (*lnum, *col));
                                        match first {
                                            Some((lnum, col)) => {
                                                md.search_commit(lnum, col)
                                            }
                                            None => md.search_cancel(),
                                        }
                                    }
                                    self.window.screen.mark_dirty();
                                } else {
                                    let cmd = self
                                        .window
                                        .screen
                                        .palette_search_commit_command(&payload, None);
                                    tracing::trace!(
                                        target: "neoism::input",
                                        mode = "search",
                                        cmd = %cmd,
                                        "palette dispatching to nvim"
                                    );
                                    self.window.screen.send_editor_command(cmd);
                                }
                            }
                            self.request_overlay_redraw();
                            return true;
                        }
                        // Snapshot what the palette wants to do FIRST,
                        // before taking a mut-borrow on it, so we can
                        // freely call other `self.window.screen.*`
                        // methods in the match arms without tripping
                        // the borrow checker on nested disjoint borrows.
                        let selected_font = self
                            .window
                            .screen
                            .renderer
                            .command_palette
                            .get_selected_font();
                        let selected_buffer = self
                            .window
                            .screen
                            .renderer
                            .command_palette
                            .get_selected_buffer_target();
                        let selected_action = self
                            .window
                            .screen
                            .renderer
                            .command_palette
                            .get_selected_action();
                        let selected_workspace = self
                            .window
                            .screen
                            .renderer
                            .command_palette
                            .get_selected_workspace_target();
                        use neoism_ui::panels::command_palette::PaletteAction;

                        // Workspaces-mode Enter: same open/adopt path
                        // the mouse pick takes. Without this branch a
                        // workspace row yielded no font/buffer/action,
                        // fell through to the empty ex-query fallback,
                        // and Enter silently did nothing.
                        if let Some(target) = selected_workspace {
                            tracing::trace!(
                                target: "neoism::input",
                                workspace_id = %target.workspace_id,
                                "command palette opening/adopting workspace"
                            );
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .set_enabled(false);
                            self.window
                                .screen
                                .open_or_adopt_daemon_workspace(target.workspace_id);
                            self.request_overlay_redraw();
                            return true;
                        }

                        if let Some(target) = selected_buffer {
                            tracing::trace!(target: "neoism::input", ?target, "command palette activating buffer");
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .set_enabled(false);
                            self.window.screen.activate_palette_buffer(target);
                            self.request_overlay_redraw();
                            return true;
                        }

                        // Fonts-mode Enter: copy the family name to
                        // the system clipboard and close. The copy
                        // icon on each row advertises this.
                        if let Some(font) = selected_font {
                            tracing::trace!(
                                target: "neoism::input",
                                font = %font.as_str(),
                                "command palette copied selected font"
                            );
                            clipboard.set(
                                neoism_backend::clipboard::ClipboardType::Clipboard,
                                font,
                            );
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .set_enabled(false);
                            self.request_overlay_redraw();
                            return true;
                        }

                        // A typed line address (`1`, `42`, `$`) is a vim
                        // jump, ALWAYS — digit-bearing shortcut hints in
                        // the command catalog (":move +1", ":42") would
                        // otherwise fuzzy-steal the Enter and run an
                        // unrelated action instead of jumping.
                        {
                            let typed = self
                                .window
                                .screen
                                .renderer
                                .command_palette
                                .query
                                .trim()
                                .to_string();
                            let is_line_address = !typed.is_empty()
                                && (typed.chars().all(|c| c.is_ascii_digit())
                                    || typed == "$");
                            if is_line_address
                                && self.window.screen.run_palette_ex_query(&typed)
                            {
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_enabled(false);
                                self.request_overlay_redraw();
                                return true;
                            }
                        }

                        let close_plan = if !ex {
                            palette_close_plan(
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .query
                                    .as_str(),
                            )
                        } else {
                            None
                        };

                        match (close_plan, selected_action) {
                            (
                                Some(GlobalExCommandPlan::CloseFocusedBufferTab),
                                _,
                            ) => {
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_enabled(false);
                                self.window.screen.close_split_or_tab(clipboard);
                            }
                            (
                                Some(
                                    GlobalExCommandPlan::CloseAllBuffersInFocusedPaneOrWorkspace,
                                ),
                                _,
                            ) => {
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_enabled(false);
                                let _ = self.window.screen.try_intercept_ex_command("qall");
                            }
                            // `ListFonts` stays inside the palette —
                            // swap the palette's contents from the
                            // command list to the registered font
                            // family names and keep it open.
                            (_, Some(PaletteAction::ListFonts)) => {
                                tracing::trace!(target: "neoism::input", "command palette entering fonts mode");
                                let fonts =
                                    self.window.screen.sugarloaf.font_family_names();
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .enter_fonts_mode(fonts);
                            }
                            (_, Some(PaletteAction::ListBuffers)) => {
                                tracing::trace!(target: "neoism::input", "command palette entering buffers mode");
                                self.window.screen.open_workspace_buffers_picker();
                            }
                            // Any other command is a one-shot: close
                            // the palette first, then dispatch.
                            (_, Some(action)) => {
                                tracing::trace!(
                                    target: "neoism::input",
                                    ?action,
                                    "command palette executing action"
                                );
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_enabled(false);
                                self.window
                                    .screen
                                    .execute_palette_action(action, clipboard);
                            }
                            // No Rio action matched. On editor/markdown
                            // surfaces, treat the query as a Vim Ex
                            // command/address (`5`, `$`, `:w`, etc.).
                            (_, None) => {
                                let query = self
                                    .window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .query
                                    .clone();
                                tracing::trace!(
                                    target: "neoism::input",
                                    query = %query.escape_debug(),
                                    "command palette falling back to ex query"
                                );
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_enabled(false);
                                self.window.screen.run_palette_ex_query(&query);
                            }
                        }
                        self.request_overlay_redraw();
                    }
                    Key::Named(NamedKey::Backspace) => {
                        tracing::trace!(target: "neoism::input", "command palette handling Backspace");
                        let current_query =
                            self.window.screen.renderer.command_palette.query.clone();
                        if !current_query.is_empty() {
                            let mut chars = current_query.chars().collect::<Vec<_>>();
                            chars.pop();
                            let new_query: String = chars.into_iter().collect();
                            let was_search = self
                                .window
                                .screen
                                .renderer
                                .command_palette
                                .is_search_mode();
                            self.window
                                .screen
                                .renderer
                                .command_palette
                                .set_query(new_query.clone());
                            if was_search {
                                self.dispatch_palette_search_query(&new_query);
                            }
                            self.request_overlay_redraw();
                        }
                    }
                    _ => {
                        if let Some(text) = key_event
                            .text_with_all_modifiers()
                            .or(key_event.text.as_deref())
                        {
                            // Filter out control characters
                            let text_str = text;
                            if !text_str.is_empty()
                                && text_str.chars().all(|c| !c.is_control())
                            {
                                tracing::trace!(
                                    target: "neoism::input",
                                    text = %text_str.escape_debug(),
                                    "command palette appending text"
                                );
                                let current_query = self
                                    .window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .query
                                    .clone();
                                let new_query = format!("{}{}", current_query, text_str);
                                let was_search = self
                                    .window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .is_search_mode();
                                self.window
                                    .screen
                                    .renderer
                                    .command_palette
                                    .set_query(new_query.clone());
                                if was_search {
                                    self.dispatch_palette_search_query(&new_query);
                                }
                                self.request_overlay_redraw();
                            } else {
                                tracing::trace!(
                                    target: "neoism::input",
                                    text = %text_str.escape_debug(),
                                    "command palette ignored text"
                                );
                            }
                        } else {
                            tracing::trace!(
                                target: "neoism::input",
                                "command palette received key without text"
                            );
                        }
                    }
                }
            }
            tracing::trace!(
                target: "neoism::input",
                "route key-wait returning true: command palette active"
            );
            return true; // Block all input when command palette is active
        }

        if self.path == RoutePath::Terminal {
            tracing::trace!(
                target: "neoism::input",
                "route key-wait returning false: terminal route"
            );
            return false;
        }

        let is_enter = is_enter_key(&key_event.logical_key);

        // Handle assistant overlay dismiss
        if self.window.screen.renderer.assistant.is_active() {
            if is_enter {
                tracing::trace!(target: "neoism::input", "assistant overlay dismissed on Enter");
                self.assistant.clear();
                self.window.screen.renderer.assistant.clear();
                self.request_overlay_redraw();
            }
            tracing::trace!(
                target: "neoism::input",
                "route key-wait returning true: assistant overlay active"
            );
            return true;
        }

        if self.path == RoutePath::ConfirmQuit {
            tracing::trace!(target: "neoism::input", "confirm-quit route handling key");
            if key_event.state == neoism_window::event::ElementState::Pressed {
                match &key_event.logical_key {
                    Key::Character(c) if c.as_str() == "n" || c.as_str() == "N" => {
                        tracing::trace!(target: "neoism::input", "confirm-quit cancelled by n");
                        self.path = RoutePath::Terminal;
                    }
                    Key::Named(NamedKey::Escape) => {
                        tracing::trace!(target: "neoism::input", "confirm-quit cancelled by Escape");
                        self.path = RoutePath::Terminal;
                    }
                    Key::Character(c) if c.as_str() == "y" || c.as_str() == "Y" => {
                        tracing::trace!(target: "neoism::input", "confirm-quit accepted by y");
                        self.quit();
                        return true;
                    }
                    _ => {}
                }
            }
            tracing::trace!(
                target: "neoism::input",
                "route key-wait returning true: confirm-quit route"
            );
            return true;
        }

        if self.path == RoutePath::Welcome && is_enter {
            tracing::trace!(target: "neoism::input", "welcome route accepted Enter");
            neoism_backend::config::create_config_file(None);
            self.path = RoutePath::Terminal;
        }

        tracing::trace!(
            target: "neoism::input",
            route_path = ?self.path,
            "route key-wait returning false"
        );
        false
    }
}
