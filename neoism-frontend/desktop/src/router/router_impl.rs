use crate::event::EventProxy;
use crate::router::route::Route;
use crate::router::routes::{assistant::Assistant, RoutePath};
use crate::router::window::RouteWindow;
use neoism_backend::clipboard::Clipboard;
use neoism_backend::config::Config as RioConfig;
use neoism_backend::error::{RioError, RioErrorLevel, RioErrorType};
use neoism_protocol::workspace::{WorkspaceWindowKind, WorkspaceWindowSummary};
use neoism_window::dpi::{PhysicalPosition, PhysicalSize};
use neoism_window::event_loop::ActiveEventLoop;
#[cfg(target_os = "macos")]
use neoism_window::platform::macos::WindowExtMacOS;
use neoism_window::window::WindowId;
use rustc_hash::FxHashMap;

// 𜱭𜱭 unicode is not available yet for all OS
// https://www.unicode.org/charts/PDF/Unicode-16.0/U160-1CC00.pdf
// #[cfg(any(target_os = "macos", target_os = "windows"))]
// const NEOISM_TITLE: &str = "𜱭𜱭";
// #[cfg(not(any(target_os = "macos", target_os = "windows")))]
const NEOISM_TITLE: &str = "▲";

pub struct Router<'a> {
    pub routes: FxHashMap<WindowId, Route<'a>>,
    #[allow(dead_code)]
    pub daemon_to_native: FxHashMap<String, WindowId>,
    #[allow(dead_code)]
    pub native_to_daemon: FxHashMap<WindowId, String>,
    propagated_report: Option<RioError>,
    pub font_library: Box<neoism_backend::sugarloaf::font::FontLibrary>,
    pub config_route: Option<WindowId>,
    pub clipboard: Clipboard,
    current_tab_id: u64,
}

impl Router<'_> {
    pub fn new<'b>(
        fonts: neoism_backend::sugarloaf::font::SugarloafFonts,
        clipboard: Clipboard,
    ) -> Router<'b> {
        let (font_library, fonts_not_found) =
            neoism_backend::sugarloaf::font::FontLibrary::new(fonts);

        let mut propagated_report = None;

        if let Some(err) = fonts_not_found {
            propagated_report = Some(RioError {
                report: RioErrorType::FontsNotFound(err.fonts_not_found),
                level: RioErrorLevel::Warning,
            });
        }

        Router {
            routes: FxHashMap::default(),
            daemon_to_native: FxHashMap::default(),
            native_to_daemon: FxHashMap::default(),
            propagated_report,
            config_route: None,
            font_library: Box::new(font_library),
            clipboard,
            current_tab_id: 0,
        }
    }

    #[allow(dead_code)]
    pub fn daemon_window_for_native(&self, native_id: WindowId) -> Option<&str> {
        self.native_to_daemon.get(&native_id).map(String::as_str)
    }

    #[allow(dead_code)]
    pub fn native_window_for_daemon(&self, daemon_id: &str) -> Option<WindowId> {
        self.daemon_to_native.get(daemon_id).copied()
    }

    #[allow(dead_code)]
    pub fn bind_logical_window(
        &mut self,
        daemon_id: impl Into<String>,
        native_id: WindowId,
    ) {
        let daemon_id = daemon_id.into();
        self.daemon_to_native.insert(daemon_id.clone(), native_id);
        self.native_to_daemon.insert(native_id, daemon_id);
    }

    #[allow(dead_code)]
    pub fn unbind_native_window(&mut self, native_id: WindowId) -> Option<String> {
        let daemon_id = self.native_to_daemon.remove(&native_id)?;
        self.daemon_to_native.remove(&daemon_id);
        Some(daemon_id)
    }

    #[allow(dead_code)]
    pub fn unbind_daemon_window(&mut self, daemon_id: &str) -> Option<WindowId> {
        let native_id = self.daemon_to_native.remove(daemon_id)?;
        self.native_to_daemon.remove(&native_id);
        Some(native_id)
    }

    #[allow(dead_code)]
    pub fn materialize_daemon_window<'a>(
        &'a mut self,
        event_loop: &'a ActiveEventLoop,
        event_proxy: EventProxy,
        config: &'a RioConfig,
        window: &WorkspaceWindowSummary,
        app_id: Option<&str>,
    ) -> Option<WindowId> {
        if let Some(native_id) = self.native_window_for_daemon(&window.id) {
            if let Some(route) = self.routes.get(&native_id) {
                route.window.winit_window.focus_window();
            }
            return Some(native_id);
        }

        let native_id = match window.kind {
            WorkspaceWindowKind::Terminal => {
                self.create_window(event_loop, event_proxy, config, None, app_id)
            }
            WorkspaceWindowKind::ConfigEditor => {
                self.open_config_window(event_loop, event_proxy, config)?
            }
            WorkspaceWindowKind::NativeTab => {
                #[cfg(target_os = "macos")]
                {
                    let tab_id = window
                        .parent_window_id
                        .as_deref()
                        .and_then(|id| self.native_window_for_daemon(id))
                        .and_then(|native_id| self.routes.get(&native_id))
                        .map(|route| route.window.winit_window.tabbing_identifier());
                    self.create_native_tab(
                        event_loop,
                        event_proxy,
                        config,
                        tab_id.as_deref(),
                        None,
                    )
                }
                #[cfg(not(target_os = "macos"))]
                {
                    self.create_window(event_loop, event_proxy, config, None, app_id)
                }
            }
        };
        self.bind_logical_window(window.id.clone(), native_id);
        Some(native_id)
    }

    pub fn bind_or_materialize_daemon_window<'a>(
        &'a mut self,
        event_loop: &'a ActiveEventLoop,
        event_proxy: EventProxy,
        config: &'a RioConfig,
        window: &WorkspaceWindowSummary,
        app_id: Option<&str>,
    ) -> Option<WindowId> {
        if let Some(native_id) = self.native_window_for_daemon(&window.id) {
            return Some(native_id);
        }

        if let Some(native_id) = self.unbound_native_window_for_daemon(window) {
            self.bind_logical_window(window.id.clone(), native_id);
            return Some(native_id);
        }

        self.materialize_daemon_window(event_loop, event_proxy, config, window, app_id)
    }

    fn unbound_native_window_for_daemon(
        &self,
        window: &WorkspaceWindowSummary,
    ) -> Option<WindowId> {
        match window.kind {
            WorkspaceWindowKind::ConfigEditor => self.config_route.filter(|id| {
                !self.native_to_daemon.contains_key(id) && self.routes.contains_key(id)
            }),
            WorkspaceWindowKind::Terminal | WorkspaceWindowKind::NativeTab => {
                self.routes.iter().find_map(|(id, route)| {
                    (!self.native_to_daemon.contains_key(id)
                        && route.path == RoutePath::Terminal)
                        .then_some(*id)
                })
            }
        }
    }

    #[inline]
    pub fn propagate_error_to_next_route(&mut self, error: RioError) {
        self.propagated_report = Some(error);
    }

    #[inline]
    pub fn update_titles(&mut self) {
        for route in self.routes.values_mut() {
            if route.window.is_focused {
                route.window.screen.context_manager.update_titles();
            }
        }
    }

    #[inline]
    pub fn get_focused_route(&self) -> Option<WindowId> {
        self.routes
            .iter()
            .find_map(|(key, val)| {
                if val.window.winit_window.has_focus() {
                    Some(key)
                } else {
                    None
                }
            })
            .copied()
    }

    pub fn open_config_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        event_proxy: EventProxy,
        config: &RioConfig,
    ) -> Option<WindowId> {
        // In case configuration window does exists already
        if let Some(route_id) = self.config_route {
            if let Some(route) = self.routes.get(&route_id) {
                route.window.winit_window.focus_window();
                return Some(route_id);
            }
        }

        let current_config: RioConfig = config.clone();
        let editor = config.editor.clone();
        let mut args = editor.args;
        args.push(
            neoism_backend::config::config_file_path()
                .display()
                .to_string(),
        );
        let new_config = RioConfig {
            shell: neoism_backend::config::Shell {
                program: editor.program,
                args,
            },
            ..current_config
        };

        let window = RouteWindow::from_target(
            event_loop,
            event_proxy,
            &new_config,
            &self.font_library,
            "Neoism Settings",
            None,
            None,
            None,
        );
        let id = window.winit_window.id();
        let route = Route::new(Assistant::new(), RoutePath::Terminal, window);
        self.routes.insert(id, route);
        self.config_route = Some(id);
        Some(id)
    }

    pub fn open_config_split(&mut self, config: &RioConfig) {
        let current_config: RioConfig = config.clone();
        let editor = config.editor.clone();
        let mut args = editor.args;
        args.push(
            neoism_backend::config::config_file_path()
                .display()
                .to_string(),
        );
        let new_config = RioConfig {
            shell: neoism_backend::config::Shell {
                program: editor.program,
                args,
            },
            ..current_config
        };

        let window_id = match self.get_focused_route() {
            Some(window_id) => window_id,
            None => return,
        };

        let route = match self.routes.get_mut(&window_id) {
            Some(window) => window,
            None => return,
        };

        route.window.screen.split_right_with_config(new_config);
    }

    #[inline]
    pub fn create_window<'a>(
        &'a mut self,
        event_loop: &'a ActiveEventLoop,
        event_proxy: EventProxy,
        config: &'a neoism_backend::config::Config,
        open_url: Option<String>,
        app_id: Option<&str>,
    ) -> WindowId {
        let tab_id = if config.navigation.is_native() {
            let id = self.current_tab_id;
            self.current_tab_id = self.current_tab_id.wrapping_add(1);
            Some(id.to_string())
        } else {
            None
        };

        let window = RouteWindow::from_target(
            event_loop,
            event_proxy,
            config,
            &self.font_library,
            NEOISM_TITLE,
            tab_id.as_deref(),
            open_url,
            app_id,
        );
        let id = window.winit_window.id();

        let mut route = Route::new(Assistant::new(), RoutePath::Terminal, window);

        if let Some(err) = &self.propagated_report {
            route.report_error(err);
            self.propagated_report = None;
        }

        self.routes.insert(id, route);
        id
    }

    #[cfg(target_os = "macos")]
    #[inline]
    pub fn create_native_tab<'a>(
        &'a mut self,
        event_loop: &'a ActiveEventLoop,
        event_proxy: EventProxy,
        config: &'a neoism_backend::config::Config,
        tab_id: Option<&str>,
        open_url: Option<String>,
    ) -> WindowId {
        let window = RouteWindow::from_target(
            event_loop,
            event_proxy,
            config,
            &self.font_library,
            NEOISM_TITLE,
            tab_id,
            open_url,
            None,
        );
        let id = window.winit_window.id();
        self.routes.insert(
            id,
            Route::new(Assistant::new(), RoutePath::Terminal, window),
        );
        id
    }

    /// Cross-window tab-drop handoff (C5 / R4).
    ///
    /// Called from the mouse-release path when the source window has
    /// an in-progress buffer-tab drag. Projects the captured-pointer
    /// cursor through `Window::inner_position` to global physical
    /// screen coordinates, asks the shared
    /// [`neoism_ui::panels::cross_window_drag::pick_destination_window`]
    /// for a destination, and if one is found:
    ///   1. drains the source window's tab payload (which internally
    ///      runs the same `bwipeout` + strip-cleanup
    ///      `tear_out_file_tab_to_pane` does),
    ///   2. calls `accept_cross_window_tab_drop` on the destination's
    ///      `Screen`,
    /// Returns `true` when the drop was handled (caller should return
    /// early), `false` when the in-window release pipeline should run.
    pub fn try_cross_window_tab_drop(&mut self, source_id: WindowId) -> bool {
        use neoism_ui::panels::cross_window_drag::{
            pick_destination_window, CrossWindowCandidate, WindowRect,
        };
        // 1. Project the source window's captured-pointer cursor to
        //    global screen coords. Winit captures the pointer in the
        //    source window for the whole drag, so the raw cursor
        //    coordinates are logical-to-the-source-window physical
        //    pixels — adding the source window's `inner_position`
        //    yields global screen physical pixels.
        let (cursor_global_x, cursor_global_y) = {
            let Some(source_route) = self.routes.get(&source_id) else {
                return false;
            };
            let inner_pos = match source_route.window.winit_window.inner_position() {
                Ok(p) => p,
                Err(_) => return false,
            };
            let mouse = &source_route.window.screen.mouse;
            (
                inner_pos.x + mouse.raw_x as i32,
                inner_pos.y + mouse.raw_y as i32,
            )
        };

        // 2. Build the candidate list.
        let mut candidates: Vec<CrossWindowCandidate> =
            Vec::with_capacity(self.routes.len());
        for (id, route) in &self.routes {
            let Ok(outer_pos) = route.window.winit_window.outer_position() else {
                continue;
            };
            let outer_size = route.window.winit_window.outer_size();
            candidates.push(CrossWindowCandidate {
                id: u64::from(*id),
                rect: WindowRect {
                    x: outer_pos.x,
                    y: outer_pos.y,
                    width: outer_size.width,
                    height: outer_size.height,
                },
            });
        }

        let source_u64 = u64::from(source_id);
        let Some(dest_u64) = pick_destination_window(
            cursor_global_x,
            cursor_global_y,
            Some(source_u64),
            &candidates,
        ) else {
            return false;
        };
        // The router's `WindowId` is opaque — find the actual key
        // whose `Into<u64>` matches the policy's pick.
        let dest_id = self
            .routes
            .keys()
            .find(|id| u64::from(**id) == dest_u64)
            .copied();
        let Some(dest_id) = dest_id else {
            return false;
        };

        // 3. Drain source-side, then hand to destination. The two
        //    routes live in the same `FxHashMap`, so we have to
        //    temporarily drop the borrow between operations — borrow
        //    source mutably first, take the payload, drop the borrow,
        //    then borrow the destination mutably.
        let payload = match self.routes.get_mut(&source_id) {
            Some(source_route) => source_route
                .window
                .screen
                .take_active_cross_window_payload(),
            None => return false,
        };
        let Some(payload) = payload else {
            return false;
        };
        if let Some(dest_route) = self.routes.get_mut(&dest_id) {
            dest_route
                .window
                .screen
                .accept_cross_window_tab_drop(payload);
            dest_route.request_redraw();
        }
        if let Some(source_route) = self.routes.get_mut(&source_id) {
            source_route.request_redraw();
        }
        true
    }

    /// Workspaces in every window *except* `exclude`, for the
    /// cross-window "Move to Workspace" menu. Lets a detached workspace
    /// in another OS window be a first-class move target.
    pub fn cross_window_workspaces(
        &self,
        exclude: WindowId,
    ) -> Vec<crate::screen::CrossWindowWorkspace> {
        let mut out = Vec::new();
        for (id, route) in &self.routes {
            if *id == exclude {
                continue;
            }
            let manager = &route.window.screen.context_manager;
            let window_id = u64::from(*id);
            for ws in 0..manager.len() {
                let title = manager
                    .titles
                    .titles
                    .get(&ws)
                    .map(|t| t.content.clone())
                    .filter(|content| !content.is_empty())
                    .unwrap_or_default();
                out.push(crate::screen::CrossWindowWorkspace {
                    window_id,
                    workspace: ws,
                    title,
                });
            }
        }
        out
    }

    /// Complete a cross-window buffer-tab move: lift the tab out of the
    /// source window and adopt it into `target_window`'s `target_workspace`.
    /// The two routes live in the same map, so drain the source first
    /// (drop the borrow), then borrow the destination.
    pub fn move_buffer_tab_across_windows(
        &mut self,
        source_id: WindowId,
        tab_index: usize,
        target_window: u64,
        target_workspace: usize,
    ) -> bool {
        let Some(target_id) = self
            .routes
            .keys()
            .find(|id| u64::from(**id) == target_window)
            .copied()
        else {
            return false;
        };
        if target_id == source_id {
            return false;
        }
        let payload = match self.routes.get_mut(&source_id) {
            Some(route) => route
                .window
                .screen
                .extract_buffer_tab_for_cross_window(tab_index),
            None => return false,
        };
        let Some(payload) = payload else {
            return false;
        };
        if let Some(target) = self.routes.get_mut(&target_id) {
            target
                .window
                .screen
                .accept_cross_window_tab(payload, target_workspace);
            target.request_redraw();
        }
        if let Some(source) = self.routes.get_mut(&source_id) {
            source.request_redraw();
        }
        true
    }
}

pub(crate) fn centered_position(
    event_loop: &ActiveEventLoop,
    width: u32,
    height: u32,
) -> Option<PhysicalPosition<i32>> {
    let monitor = event_loop.primary_monitor()?;
    let monitor_size = monitor.size();
    let monitor_pos = monitor.position();
    let (x, y) = neoism_ui::lifecycle_policy::centered_window_position(
        monitor_pos.x,
        monitor_pos.y,
        monitor_size.width,
        monitor_size.height,
        width,
        height,
    );
    Some(PhysicalPosition::new(x, y))
}

pub(crate) fn compute_window_size_from_grid(
    columns: Option<u16>,
    rows: Option<u16>,
    panel: &neoism_backend::config::layout::Panel,
    dim: &crate::layout::ContextDimension,
    window_size: PhysicalSize<u32>,
) -> (u32, u32) {
    let scale = dim.dimension.scale;
    let dims = neoism_ui::lifecycle_policy::GridSizeDims {
        cell_width: dim.dimension.width,
        cell_height: dim.dimension.height,
        scale,
        terminal_margin_left: dim.margin.left,
        terminal_margin_right: dim.margin.right,
        terminal_margin_top: dim.margin.top,
        terminal_margin_bottom: dim.margin.bottom,
        panel_padding_left: panel.padding.left,
        panel_padding_right: panel.padding.right,
        panel_padding_top: panel.padding.top,
        panel_padding_bottom: panel.padding.bottom,
        panel_margin_left: panel.margin.left,
        panel_margin_right: panel.margin.right,
        panel_margin_top: panel.margin.top,
        panel_margin_bottom: panel.margin.bottom,
    };
    let min_w = (crate::router::window::DEFAULT_MINIMUM_WINDOW_WIDTH as f32 * scale)
        .ceil() as u32;
    let min_h = (crate::router::window::DEFAULT_MINIMUM_WINDOW_HEIGHT as f32 * scale)
        .ceil() as u32;
    neoism_ui::lifecycle_policy::compute_window_size_from_grid_dims(
        columns,
        rows,
        &dims,
        window_size.width,
        window_size.height,
        min_w,
        min_h,
    )
}

#[cfg(test)]
mod grid_size_tests {
    use super::*;
    use neoism_backend::config::layout::{Margin, Panel};
    use neoism_backend::sugarloaf::layout::TextDimensions;

    fn make_dim(
        width: f32,
        height: f32,
        scale: f32,
        margin: Margin,
    ) -> crate::layout::ContextDimension {
        crate::layout::ContextDimension {
            dimension: TextDimensions {
                width,
                height,
                scale,
            },
            margin,
            ..Default::default()
        }
    }

    fn win(w: u32, h: u32) -> PhysicalSize<u32> {
        PhysicalSize {
            width: w,
            height: h,
        }
    }

    fn panel_zero() -> Panel {
        Panel {
            padding: Margin::all(0.0),
            margin: Margin::all(0.0),
            ..Default::default()
        }
    }

    #[test]
    fn applies_only_columns_override() {
        let dim = make_dim(10.0, 20.0, 2.0, Margin::all(0.0));
        // 80 * 10.0 = 800, next_multiple_of(2) = 800; height stays at window size
        assert_eq!(
            compute_window_size_from_grid(
                Some(80),
                None,
                &panel_zero(),
                &dim,
                win(1000, 600)
            ),
            (800, 600)
        );
    }

    #[test]
    fn applies_only_rows_override() {
        let dim = make_dim(10.0, 20.0, 2.0, Margin::all(0.0));
        // 24 * 20.0 = 480, next_multiple_of(2) = 480; width stays at window size
        assert_eq!(
            compute_window_size_from_grid(
                None,
                Some(24),
                &panel_zero(),
                &dim,
                win(1000, 600)
            ),
            (1000, 480)
        );
    }

    #[test]
    fn applies_both_overrides() {
        let dim = make_dim(10.0, 20.0, 1.0, Margin::all(0.0));
        assert_eq!(
            compute_window_size_from_grid(
                Some(100),
                Some(40),
                &panel_zero(),
                &dim,
                win(500, 300)
            ),
            (1000, 800)
        );
    }

    #[test]
    fn ignores_zero_overrides_and_keeps_window_size() {
        let dim = make_dim(10.0, 20.0, 2.0, Margin::all(0.0));
        assert_eq!(
            compute_window_size_from_grid(
                Some(0),
                Some(0),
                &panel_zero(),
                &dim,
                win(1000, 600)
            ),
            (1000, 600)
        );
    }

    #[test]
    fn rounds_up_on_hidpi() {
        let dim = make_dim(16.41, 33.0, 2.0, Margin::all(0.0));
        // 80 * 16.41 = 1312.8 → ceil = 1313, next_multiple_of(2) = 1314
        // 24 * 33.0 = 792, next_multiple_of(2) = 792
        assert_eq!(
            compute_window_size_from_grid(
                Some(80),
                Some(24),
                &panel_zero(),
                &dim,
                win(1000, 600)
            ),
            (1314, 792)
        );
    }

    #[test]
    fn includes_terminal_and_panel_margins() {
        let panel = Panel {
            padding: Margin::new(3.0, 2.0, 4.0, 1.0),
            margin: Margin::new(7.0, 6.0, 8.0, 5.0),
            ..Default::default()
        };
        let dim = make_dim(10.0, 20.0, 1.0, Margin::new(4.0, 3.0, 5.0, 2.0));
        assert_eq!(
            compute_window_size_from_grid(Some(10), Some(5), &panel, &dim, win(500, 300)),
            (300, 200)
        );
    }

    #[test]
    fn never_goes_under_minimum() {
        let dim = make_dim(1.0, 1.0, 1.0, Margin::all(0.0));
        assert_eq!(
            compute_window_size_from_grid(
                Some(1),
                Some(1),
                &panel_zero(),
                &dim,
                win(50, 50)
            ),
            (300, 200)
        );
    }
}
