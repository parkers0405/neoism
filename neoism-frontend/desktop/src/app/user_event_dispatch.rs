//! `user_event` dispatch helpers.
//!
//! Each `apply_*` method below is the body of a former `RioEvent::*`
//! match arm extracted out of `app::user_event` (the dispatcher in
//! `app/mod.rs`). They live here so the dispatcher reads as a flat
//! switchboard: every arm is one line that delegates to the matching
//! helper. The signatures mirror the destructured arm payload at the
//! call site.
//!
//! `debounce_follow_up` is the associated helper used by all
//! `Prepare*` / `Render*` debounce arms: it schedules a follow-up
//! `RioEvent` on the [`Scheduler`] after `millis` milliseconds, but
//! only when no timer with the same id is already pending. Centralising
//! the dedup + scheduling boilerplate keeps the dispatcher arms
//! one-liners.

use crate::app::scheduler::{Scheduler, TimerId};
use crate::app::Application;
use crate::bridges::utils::apply_theme_to_config;
use crate::router::Router;
use neoism_backend::clipboard::ClipboardType;
use neoism_backend::config::colors::ColorRgbExt;
use neoism_backend::event::{EventPayload, RioEvent, RioEventType};
use neoism_backend::graphics_adapter::{
    graphic_data_to_sugarloaf, graphic_id_to_sugarloaf,
};
use neoism_terminal_core::ansi::graphics::UpdateQueues;
use neoism_terminal_core::colors::{ColorRgb, NamedColor};
use neoism_ui::user_event_policy::{
    color_change_targets_background, config_editor_target, create_window_strategy,
    open_editor_tab_action, render_event_route_action, should_load_clipboard,
    ConfigEditorTarget, CreateWindowStrategy, OpenEditorTabAction,
    RenderEventRouteAction, ToggleFullscreenAction,
};
use neoism_window::event_loop::ActiveEventLoop;
#[cfg(target_os = "macos")]
use neoism_window::platform::macos::WindowExtMacOS;
use neoism_window::window::{Fullscreen, WindowId};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};
use teletypewriter::WinsizeBuilder;

impl Application<'_> {
    /// Schedule a follow-up [`RioEvent`] on the scheduler after
    /// `delay_ms` milliseconds, but only if no other timer with the
    /// same id is already pending. Shared by every `Prepare*` and
    /// debounced `Render*` / `BlinkCursor*` arm.
    pub(super) fn debounce_follow_up(
        scheduler: &mut Scheduler,
        timer_id: TimerId,
        delay_ms: u64,
        follow_up_event: RioEvent,
        window_id: WindowId,
    ) {
        if scheduler.scheduled(timer_id) {
            return;
        }
        let event = EventPayload::new(RioEventType::Rio(follow_up_event), window_id);
        scheduler.schedule(event, Duration::from_millis(delay_ms), false, timer_id);
    }

    /// `RioEvent::Render` — repaint the route bound to `window_id`
    /// (honouring the per-route render gating and post-occlusion
    /// redraw).
    pub(super) fn apply_render_arm(&mut self, window_id: WindowId) {
        let disable_unfocused_render = self.config.renderer.disable_unfocused_render;
        let disable_occluded_render = self.config.renderer.disable_occluded_render;
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let action = render_event_route_action(
            disable_unfocused_render,
            disable_occluded_render,
            route.window.is_focused,
            route.window.is_occluded,
            route.window.needs_render_after_occlusion,
        );
        match action {
            RenderEventRouteAction::Skip => {}
            RenderEventRouteAction::ConsumeOcclusionAndRedraw => {
                route.window.needs_render_after_occlusion = false;
                route.request_redraw_with_reason("RioEvent::Render:after_occlusion");
            }
            RenderEventRouteAction::Redraw => {
                route.request_redraw_with_reason("RioEvent::Render");
            }
        }
    }

    /// `RioEvent::RenderRoute(route_id)` — mark the targeted route
    /// context dirty and either redraw immediately or schedule a
    /// follow-up render once the vblank deadline passes.
    pub(super) fn apply_render_route_arm(
        &mut self,
        window_id: WindowId,
        route_id: usize,
    ) {
        if !self.config.renderer.strategy.is_event_based() {
            return;
        }
        let disable_unfocused_render = self.config.renderer.disable_unfocused_render;
        let disable_occluded_render = self.config.renderer.disable_occluded_render;

        let (wait, redraw_pending, redraw_retry_due, redraw_retry_deadline) = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let action = render_event_route_action(
                disable_unfocused_render,
                disable_occluded_render,
                route.window.is_focused,
                route.window.is_occluded,
                route.window.needs_render_after_occlusion,
            );
            if matches!(action, RenderEventRouteAction::Skip) {
                return;
            }
            if matches!(action, RenderEventRouteAction::ConsumeOcclusionAndRedraw) {
                route.window.needs_render_after_occlusion = false;
            }
            if let Some(ctx_item) =
                route.window.screen.ctx_mut().get_by_route_id(route_id)
            {
                ctx_item.val.renderable_content.pending_update.set_dirty();
            } else {
                tracing::trace!(
                    target: "neoism::render_event",
                    ?window_id,
                    route_id,
                    event = "RenderRoute",
                    current_route = route.window.screen.ctx().current_route(),
                    "could not find route context"
                );
            }
            let now = Instant::now();
            let redraw_pending = route.redraw_request_pending();
            let redraw_retry_deadline = route.redraw_retry_deadline();
            let redraw_retry_due =
                redraw_retry_deadline.is_some_and(|deadline| deadline <= now);
            let wait = if redraw_pending || redraw_retry_due {
                None
            } else {
                route.window.wait_until()
            };
            (
                wait,
                redraw_pending,
                redraw_retry_due,
                redraw_retry_deadline,
            )
        };

        if redraw_pending && !redraw_retry_due {
            return;
        }

        // If the renderer asked us to wait until the next vblank,
        // defer the actual redraw via a one-shot `Render` event so
        // we don't burn frames; otherwise repaint right now.
        if let Some(wait) = wait {
            let now = Instant::now();
            let wait = redraw_retry_deadline
                .map(|deadline| deadline.saturating_duration_since(now).min(wait))
                .unwrap_or(wait);
            let timer_id =
                TimerId::new(crate::app::scheduler::Topic::RenderRoute, route_id);
            Self::debounce_follow_up(
                &mut self.scheduler,
                timer_id,
                wait.as_millis().max(1) as u64,
                RioEvent::Render,
                window_id,
            );
        } else if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.request_redraw();
        }
    }

    /// `RioEvent::TerminalDamaged(route_id)` — like `RenderRoute` but
    /// without the post-occlusion consume step and without the
    /// vblank-deadline throttle: damage events always paint as soon
    /// as the route is allowed to render.
    pub(super) fn apply_terminal_damaged_arm(
        &mut self,
        window_id: WindowId,
        route_id: usize,
    ) {
        if !self.config.renderer.strategy.is_event_based() {
            return;
        }
        let disable_unfocused_render = self.config.renderer.disable_unfocused_render;
        let disable_occluded_render = self.config.renderer.disable_occluded_render;
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let action = render_event_route_action(
            disable_unfocused_render,
            disable_occluded_render,
            route.window.is_focused,
            route.window.is_occluded,
            route.window.needs_render_after_occlusion,
        );
        if matches!(action, RenderEventRouteAction::Skip) {
            return;
        }
        let marked = if let Some(ctx_item) =
            route.window.screen.ctx_mut().get_by_route_id(route_id)
        {
            ctx_item.val.renderable_content.pending_update.set_dirty();
            true
        } else {
            tracing::trace!(
                target: "neoism::render_event",
                ?window_id,
                route_id,
                event = "TerminalDamaged",
                "could not find route context"
            );
            false
        };
        if marked {
            route.request_redraw();
        }
    }

    /// `RioEvent::NotebookStatusTick` — low-frequency redraw for running-cell
    /// indicators. Output and result events still redraw immediately; this avoids
    /// rebuilding notebook markdown while cells are running.
    pub(super) fn apply_notebook_status_tick(&mut self, window_id: WindowId) {
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let refreshed = {
            let current = route.window.screen.ctx_mut().current_mut();
            let Some(notebook) = current.notebook.as_mut() else {
                return;
            };
            if !notebook.has_running_cells() {
                return;
            }
            crate::app::freeze_watchdog::note_sampled(
                format!("notebook_status_tick:{window_id:?}"),
                Duration::from_secs(2),
                format!(
                    "notebook_status_tick window={window_id:?} running_cells={}",
                    notebook.running_cells.len()
                ),
            );
            true
        };
        if refreshed {
            route.request_redraw_with_reason("notebook_status_tick");
        }
    }

    /// `RioEvent::UpdateGraphics { queues, .. }` — drain the queued
    /// inserts (sixel / iTerm2 atlas + kitty image textures) and
    /// removals into the route's sugarloaf, then request a redraw.
    pub(super) fn apply_update_graphics(
        &mut self,
        window_id: WindowId,
        queues: UpdateQueues,
    ) {
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let sugarloaf = &mut route.window.screen.sugarloaf;

        // Atlas graphics (sixel/iTerm2).
        for graphic_data in queues.pending {
            sugarloaf
                .graphics
                .insert(graphic_data_to_sugarloaf(graphic_data));
        }

        // Image textures (kitty) → separate store, no clone.
        for (image_id, graphic_data) in queues.pending_images {
            sugarloaf.image_data.insert(
                image_id,
                neoism_backend::sugarloaf::GraphicDataEntry::from_graphic_data(
                    graphic_data_to_sugarloaf(graphic_data),
                ),
            );
        }

        for graphic_data in queues.remove_queue {
            sugarloaf
                .graphics
                .remove(&graphic_id_to_sugarloaf(graphic_data));
        }

        route.request_redraw();
    }

    /// `RioEvent::UpdateConfig` — reload `neoism.toml` from disk,
    /// rebuild the font library if fonts changed, re-apply the
    /// adaptive / forced theme, and push the new config into every
    /// open route.
    pub(super) fn apply_update_config(&mut self) {
        let (config, config_error) = match neoism_backend::config::Config::try_load() {
            Ok(config) => (config, None),
            Err(error) => (neoism_backend::config::Config::default(), Some(error)),
        };

        let has_font_updates = self.config.fonts != config.fonts;
        let has_config_error = config_error.is_some();

        let font_library_errors = if has_font_updates {
            let new_font_library = neoism_backend::sugarloaf::font::FontLibrary::new(
                config.fonts.to_owned(),
            );
            *self.router.font_library = new_font_library.0;
            new_font_library.1
        } else {
            None
        };

        self.config = config;

        let mut has_checked_adaptive_colors = false;
        for (_id, route) in self.router.routes.iter_mut() {
            if !has_checked_adaptive_colors {
                let system_theme = route.window.winit_window.theme();
                let theme = self
                    .config
                    .force_theme
                    .map(|t| t.to_window_theme())
                    .or(system_theme);
                apply_theme_to_config(&mut self.config, theme);
                has_checked_adaptive_colors = true;
            }

            if has_font_updates {
                if let Some(ref err) = font_library_errors {
                    route
                        .window
                        .screen
                        .context_manager
                        .report_error_fonts_not_found(err.fonts_not_found.clone());
                }
            }

            route.update_config(
                &self.config,
                &self.router.font_library,
                has_font_updates,
            );
            route.window.configure_window(&self.config);

            if has_config_error {
                if let Some(error) = &config_error {
                    route.report_error(&error.to_owned().into());
                }
            } else {
                route.clear_errors();
            }
        }
    }

    /// `RioEvent::CursorBlinkingChangeOnRoute(route_id)` — tag the
    /// cursor line as partially damaged on the focused context and
    /// request a redraw so only the cursor row repaints.
    pub(super) fn apply_cursor_blinking_change_on_route(
        &mut self,
        window_id: WindowId,
        route_id: usize,
    ) {
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        if route_id != route.window.screen.ctx().current_route() {
            return;
        }
        let cursor_line = {
            let terminal = route.window.screen.ctx_mut().current_mut().terminal.lock();
            terminal.cursor().pos.row.0 as usize
        };
        route
            .window
            .screen
            .ctx_mut()
            .current_mut()
            .renderable_content
            .pending_update
            .set_terminal_damage(neoism_terminal_core::damage::TerminalDamage::Partial(
                [neoism_terminal_core::crosswords::LineDamage::new(
                    cursor_line,
                    true,
                )]
                .into_iter()
                .collect(),
            ));
        route.request_redraw();
    }

    /// `RioEvent::ClipboardLoad(route_id, kind, format)` — read the
    /// system clipboard (guarded by focus), format the contents with
    /// the supplied formatter, and write the result to the targeted
    /// PTY messenger.
    pub(super) fn apply_clipboard_load(
        &mut self,
        window_id: WindowId,
        route_id: usize,
        clipboard_type: ClipboardType,
        format: Arc<dyn Fn(&str) -> String + Sync + Send + 'static>,
    ) {
        let text = {
            let Router {
                routes, clipboard, ..
            } = &mut self.router;
            let Some(route) = routes.get_mut(&window_id) else {
                return;
            };
            if !should_load_clipboard(route.window.is_focused) {
                return;
            }
            format(clipboard.get(clipboard_type).as_str())
        };
        Self::send_bytes_to_route_context(
            &mut self.router,
            window_id,
            route_id,
            text.into_bytes(),
        );
    }

    /// `RioEvent::OpenEditorTab { path, .. }` — either open the
    /// supplied path in the editor or open a fresh empty buffer when
    /// no path was provided. Always requests a redraw.
    pub(super) fn apply_open_editor_tab(
        &mut self,
        window_id: WindowId,
        path: Option<PathBuf>,
    ) {
        let action = open_editor_tab_action(path.is_some());
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        match action {
            OpenEditorTabAction::OpenPath => {
                if let Some(path) = path {
                    route.window.screen.open_path_in_editor(path);
                }
            }
            OpenEditorTabAction::OpenEmptyBuffer => {
                route.window.screen.open_empty_buffer_tab();
            }
        }
        route.request_redraw();
    }

    /// `RioEvent::TextAreaSizeRequest(route_id, format)` — measure
    /// the target route context's terminal dimensions, hand them to
    /// the formatter, and forward the resulting bytes to that
    /// context's PTY messenger.
    pub(super) fn apply_text_area_size_request(
        &mut self,
        window_id: WindowId,
        route_id: usize,
        format: Arc<dyn Fn(WinsizeBuilder) -> String + Sync + Send + 'static>,
    ) {
        let text = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let Some(context_item) =
                route.window.screen.ctx_mut().get_by_route_id(route_id)
            else {
                return;
            };
            let dimension = context_item.context().dimension;
            format(crate::bridges::utils::terminal_dimensions(&dimension))
        };
        Self::send_bytes_to_route_context(
            &mut self.router,
            window_id,
            route_id,
            text.into_bytes(),
        );
    }

    /// `RioEvent::ColorRequest(route_id, index, format)` — resolve
    /// the requested palette slot (preferring any override the
    /// terminal recorded, falling back to the configured colour),
    /// format it, and write the bytes to the targeted PTY messenger.
    /// Cursor-colour requests with no override are ignored to mirror
    /// xterm.
    pub(super) fn apply_color_request(
        &mut self,
        window_id: WindowId,
        route_id: usize,
        index: usize,
        format: Arc<dyn Fn(ColorRgb) -> String + Sync + Send + 'static>,
    ) {
        let bytes = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let fallback_color = route.window.screen.renderer.colors[index];
            let Some(context_item) =
                route.window.screen.ctx_mut().get_by_route_id(route_id)
            else {
                return;
            };
            let terminal = context_item.context().terminal.lock();
            let color: ColorRgb = match terminal.colors()[index] {
                Some(color) => ColorRgb::from_color_arr(color),
                None if index == NamedColor::Cursor as usize => {
                    return;
                }
                None => ColorRgb::from_color_arr(fallback_color),
            };
            drop(terminal);
            format(color).into_bytes()
        };
        Self::send_bytes_to_route_context(&mut self.router, window_id, route_id, bytes);
    }

    /// `RioEvent::CreateWindow(working_dir_override)` — spawn a new
    /// top-level window using either the app config (default) or a
    /// clone with the working-dir override applied.
    pub(super) fn apply_create_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        working_dir_override: Option<PathBuf>,
    ) {
        let config;
        let config = match create_window_strategy(working_dir_override.is_some()) {
            CreateWindowStrategy::OverrideWorkingDir => {
                config = neoism_backend::config::Config {
                    working_dir: working_dir_override
                        .map(|path| path.to_string_lossy().to_string()),
                    ..self.config.clone()
                };
                &config
            }
            CreateWindowStrategy::UseAppConfig => &self.config,
        };
        let window_id = self.router.create_window(
            event_loop,
            self.event_proxy.clone(),
            config,
            None,
            self.app_id.as_deref(),
        );
        self.ensure_local_session(window_id);
    }

    /// `RioEvent::CreateWindowWithOptions` — same as CreateWindow, but
    /// also applies launch-time editor paths in the new window.
    pub(super) fn apply_create_window_with_options(
        &mut self,
        event_loop: &ActiveEventLoop,
        working_dir_override: Option<PathBuf>,
        open_paths: Vec<PathBuf>,
    ) {
        let config;
        let config = match create_window_strategy(working_dir_override.is_some()) {
            CreateWindowStrategy::OverrideWorkingDir => {
                config = neoism_backend::config::Config {
                    working_dir: working_dir_override
                        .map(|path| path.to_string_lossy().to_string()),
                    ..self.config.clone()
                };
                &config
            }
            CreateWindowStrategy::UseAppConfig => &self.config,
        };
        let window_id = self.router.create_window(
            event_loop,
            self.event_proxy.clone(),
            config,
            None,
            self.app_id.as_deref(),
        );
        self.ensure_local_session(window_id);

        if let Some(route) = self.router.routes.get_mut(&window_id) {
            for path in open_paths {
                route.window.screen.open_path_in_editor(path);
            }
            route.request_redraw();
        }
    }

    /// Complete any workspace-detach gestures parked by
    /// `Screen::handle_island_drag_release`. For each source window
    /// holding a lifted workspace grid, spawn a fresh OS window and
    /// adopt the grid into it — moving the live PTYs across without a
    /// restart. Runs from the window-event loop where `event_loop` and
    /// exclusive `router` access are both available.
    pub(super) fn finish_pending_workspace_detaches(
        &mut self,
        event_loop: &ActiveEventLoop,
    ) {
        let sources: Vec<WindowId> = self
            .router
            .routes
            .iter()
            .filter(|(_, route)| route.window.screen.has_pending_detached_workspace())
            .map(|(id, _)| *id)
            .collect();

        for source_id in sources {
            let inherited_server = self.window_sessions.get(&source_id).map(|session| {
                (
                    session.connection.endpoint().to_string(),
                    session.active_server_id.clone(),
                )
            });
            let Some(detached) =
                self.router.routes.get_mut(&source_id).and_then(|route| {
                    route.window.screen.take_pending_detached_workspace()
                })
            else {
                continue;
            };

            let new_window_id = self.router.create_window(
                event_loop,
                self.event_proxy.clone(),
                &self.config,
                None,
                self.app_id.as_deref(),
            );
            if let Some((endpoint, server_id)) = inherited_server {
                let token = server_id
                    .as_deref()
                    .and_then(|id| self.server_registry.token(id))
                    .map(str::to_string);
                self.switch_window_server(new_window_id, &endpoint, token, server_id);
            } else {
                self.ensure_local_session(new_window_id);
            }

            if let Some(dest) = self.router.routes.get_mut(&new_window_id) {
                dest.window.screen.adopt_detached_workspace(detached);
                dest.request_redraw();
            }
            if let Some(source) = self.router.routes.get_mut(&source_id) {
                source.request_redraw();
            }
        }
    }

    /// Complete any cross-window buffer-tab moves parked by the
    /// right-click menu (moving a tab into a workspace that lives in a
    /// different OS window, e.g. a detached one). Runs from the
    /// window-event loop where the router can borrow both windows.
    pub(super) fn finish_pending_cross_window_tab_moves(&mut self) {
        let sources: Vec<WindowId> = self
            .router
            .routes
            .iter()
            .filter(|(_, route)| route.window.screen.has_pending_cross_window_tab_move())
            .map(|(id, _)| *id)
            .collect();

        for source_id in sources {
            let Some((tab_index, target_window, target_workspace)) =
                self.router.routes.get_mut(&source_id).and_then(|route| {
                    route.window.screen.take_pending_cross_window_tab_move()
                })
            else {
                continue;
            };
            self.router.move_buffer_tab_across_windows(
                source_id,
                tab_index,
                target_window,
                target_workspace,
            );
        }
    }

    /// `RioEvent::CreateNativeTab(working_dir_overwrite)` (macOS
    /// only) — spawn a new native tab anchored to the focused
    /// window's tabbing identifier, optionally with an override
    /// working directory.
    #[cfg(target_os = "macos")]
    pub(super) fn apply_create_native_tab(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        working_dir_overwrite: Option<String>,
    ) {
        let Some(route) = self.router.routes.get(&window_id) else {
            return;
        };
        let tabbing_identifier = route.window.winit_window.tabbing_identifier();
        // `CreateNativeTab` is fired both for fresh tab requests and
        // by `context.use_current_path`-triggered spawns. The arm
        // overrides the cloned config's `working_dir` when present
        // and falls back to the app config otherwise.
        let config = if working_dir_overwrite.is_some() {
            neoism_backend::config::Config {
                working_dir: working_dir_overwrite,
                ..self.config.clone()
            }
        } else {
            self.config.clone()
        };

        let parent_window_id = self
            .router
            .daemon_window_for_native(window_id)
            .map(str::to_string);
        self.send_window_message(
            window_id,
            neoism_protocol::workspace::WorkspaceClientMessage::RequestOpenNativeTab {
                workspace_id: None,
                parent_window_id,
                title: None,
            },
        );
        let native_window_id = self.router.create_native_tab(
            event_loop,
            self.event_proxy.clone(),
            &config,
            Some(&tabbing_identifier),
            None,
        );
        self.ensure_local_session(native_window_id);
    }

    /// `RioEvent::CreateConfigEditor` — open the config editor
    /// either as a split inside the focused route or as a brand-new
    /// window, per `navigation.open_config_with_split`.
    pub(super) fn apply_create_config_editor(&mut self, event_loop: &ActiveEventLoop) {
        match config_editor_target(self.config.navigation.open_config_with_split) {
            ConfigEditorTarget::Split => {
                self.router.open_config_split(&self.config);
            }
            ConfigEditorTarget::NewWindow => {
                if let Some(window_id) = self.router.open_config_window(
                    event_loop,
                    self.event_proxy.clone(),
                    &self.config,
                ) {
                    self.ensure_local_session(window_id);
                }
            }
        }
    }

    /// `RioEvent::ToggleFullScreen` — toggle borderless fullscreen on
    /// the targeted route's winit window.
    pub(super) fn apply_toggle_fullscreen(&mut self, window_id: WindowId) {
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let currently_fullscreen = route.window.winit_window.fullscreen().is_some();
        match neoism_ui::user_event_policy::toggle_fullscreen_action(currently_fullscreen)
        {
            ToggleFullscreenAction::EnterBorderless => route
                .window
                .winit_window
                .set_fullscreen(Some(Fullscreen::Borderless(None))),
            ToggleFullscreenAction::Leave => {
                route.window.winit_window.set_fullscreen(None)
            }
        }
    }

    /// `RioEvent::ToggleAppearanceTheme` — flip Dark ↔ Light on the
    /// configured force-theme, propagate the new theme into the
    /// shared `Colors` config, and push the change through the
    /// route's renderer + window decorations.
    pub(super) fn apply_toggle_appearance_theme(&mut self, window_id: WindowId) {
        use neoism_backend::config::theme::AppearanceTheme;
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let current = self
            .config
            .force_theme
            .or_else(|| {
                route
                    .window
                    .winit_window
                    .theme()
                    .map(AppearanceTheme::from_window_theme)
            })
            .unwrap_or(AppearanceTheme::Dark);
        let toggled = current.toggled();
        self.config.force_theme = Some(toggled);
        apply_theme_to_config(&mut self.config, Some(toggled.to_window_theme()));
        route
            .window
            .screen
            .update_config(&self.config, &self.router.font_library, false);
        route.window.configure_window(&self.config);
    }

    /// `RioEvent::ColorChange(route_id, index, color)` — handle OSC
    /// 11 / 111 (and friends) by updating the targeted pane's
    /// background-state override when `index` lands on the background
    /// palette slot. Other indices are ignored at this layer; they
    /// flow through the renderer's normal color path.
    pub(super) fn apply_color_change(
        &mut self,
        window_id: WindowId,
        route_id: usize,
        index: usize,
        color: Option<ColorRgb>,
    ) {
        if !color_change_targets_background(index, NamedColor::Foreground as usize) {
            return;
        }
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let grid = route.window.screen.context_manager.current_grid_mut();
        // `ContextGrid::get_mut` is keyed on taffy `NodeId` — a
        // different identifier space — so look the panel up by its
        // actual route id instead.
        if let Some(context_item) = grid.get_by_route_id(route_id) {
            use crate::context::renderable::BackgroundState;
            let next_state = match color {
                Some(c) => Some(BackgroundState::Set(c.to_wgpu())),
                None => Some(BackgroundState::Reset),
            };
            context_item.context_mut().renderable_content.background = next_state;
        }
    }

    /// Internal helper: forward `bytes` to the PTY messenger for the
    /// route-context that matches `route_id` inside the route bound
    /// to `window_id`. Free-function style (`&mut Router`) so callers
    /// that already hold a `&mut self.router` borrow can use it
    /// without re-borrowing through `self`.
    fn send_bytes_to_route_context(
        router: &mut Router<'_>,
        window_id: WindowId,
        route_id: usize,
        bytes: Vec<u8>,
    ) {
        if let Some(route) = router.routes.get_mut(&window_id) {
            if let Some(context_item) =
                route.window.screen.ctx_mut().get_by_route_id(route_id)
            {
                context_item.context_mut().messenger.send_bytes(bytes);
            }
        }
    }
}
