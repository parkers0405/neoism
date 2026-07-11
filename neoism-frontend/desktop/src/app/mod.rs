use crate::app::bell::{play_audio_bell, send_desktop_notification};
use crate::app::daemon_pump::DesktopDaemonConnection;
use crate::app::daemon_rehome::HostHomingTracker;
use crate::app::scheduler::{Scheduler, TimerId, Topic};
use crate::app::window_event::touch::on_touch;
use crate::bridges::utils::apply_theme_to_config;
use crate::daemon_client::DaemonServerMessage;
use crate::router::{routes::RoutePath, Router};
use crate::terminal::watcher::configuration_file_updates;
use neoism_backend::clipboard::Clipboard;
use neoism_backend::event::{EventPayload, EventProxy, RioEvent, RioEventType};
use neoism_protocol::workspace::{
    HostSummary, WorkspaceClientMessage, WorkspaceServerMessage, WorkspaceSummary,
    WorkspaceWindowSummary,
};
#[cfg(target_os = "macos")]
use neoism_ui::user_event_policy::should_exit_event_loop_after_close_window;
use neoism_ui::user_event_policy::{
    close_terminal_action, quit_request_action, refresh_redraw_action,
    should_apply_progress_report, should_exit_event_loop_after_route_removed,
    should_play_audio_bell, should_send_desktop_notification, should_store_clipboard,
    CloseTerminalAction, QuitRequestAction, RefreshRedrawAction,
};
use neoism_window::application::ApplicationHandler;
use neoism_window::event::{StartCause, WindowEvent};
use neoism_window::event_loop::ActiveEventLoop;
use neoism_window::event_loop::ControlFlow;
use neoism_window::event_loop::{DeviceEvents, EventLoop};
#[cfg(target_os = "macos")]
use neoism_window::platform::macos::ActiveEventLoopExtMacOS;
#[cfg(target_os = "macos")]
use neoism_window::platform::macos::WindowExtMacOS;
use neoism_window::platform::modifier_supplement::KeyEventExtModifierSupplement;
use neoism_window::window::WindowId;
use raw_window_handle::HasDisplayHandle;
use std::error::Error;
use std::path::PathBuf;
use std::time::{Duration, Instant};

const NOTEBOOK_STATUS_TICK_MS: u64 = 500;
const FRAME_WATCHDOG_NOTE_INTERVAL: Duration = Duration::from_secs(1);

pub mod bell;
pub mod daemon_pump;
pub mod daemon_rehome;
pub mod editor_grid_diag;
pub mod freeze_watchdog;
pub mod ime;
pub mod messenger;
pub mod scheduler;
pub mod user_event_dispatch;
pub mod window_event;

pub struct Application<'a> {
    config: neoism_backend::config::Config,
    event_proxy: EventProxy,
    router: Router<'a>,
    daemon: Option<DesktopDaemonConnection>,
    /// "Follow the workspace to its new home" state (Wave 4D). Caches
    /// `host_id -> daemon_url` from host-tree pushes and watches each
    /// workspace's `running_on_host_id` so we can re-dial `daemon` when the
    /// active workspace is promoted/demoted to a different host. Mirrors the
    /// web `WorkplaceService` re-home watcher.
    homing: HostHomingTracker,
    /// A peer-workspace join waiting for the freshly-dialled daemon's
    /// tree: `(window, workspace_id)`. Set when a Workspaces pick
    /// re-dials `daemon` to the workspace's owning host; consumed by
    /// the first tree push that carries the workspace, which re-enters
    /// the screen's open/adopt path over the new link.
    pending_peer_adopt: Option<(WindowId, String)>,
    /// The daemon endpoint this desktop started on. Leaving the last
    /// joined peer workspace re-dials back here so the daemon plane
    /// returns home.
    home_daemon_endpoint: Option<String>,
    scheduler: Scheduler,
    app_id: Option<String>,
    initial_open_paths: Vec<PathBuf>,
    _external_command_listener: Option<crate::ipc::ExternalCommandListener>,
}

impl Application<'_> {
    pub fn new<'app>(
        config: neoism_backend::config::Config,
        config_error: Option<neoism_backend::config::ConfigError>,
        event_loop: &EventLoop<EventPayload>,
        app_id: Option<String>,
        initial_open_paths: Vec<PathBuf>,
        daemon_url: Option<String>,
    ) -> Application<'app> {
        // SAFETY: Since this takes a pointer to the winit event loop, it MUST be dropped first,
        // which is done in `exiting`.
        let clipboard =
            unsafe { Clipboard::new(event_loop.display_handle().unwrap().as_raw()) };

        let mut router = Router::new(config.fonts.to_owned(), clipboard);
        if let Some(error) = config_error {
            router.propagate_error_to_next_route(error.into());
        }

        let proxy = event_loop.create_proxy();
        let event_proxy = EventProxy::new(proxy.clone());
        let daemon =
            daemon_url
                .as_deref()
                .and_then(|url| {
                    match DesktopDaemonConnection::connect(url, event_proxy.clone()) {
                        Ok(daemon) => Some(daemon),
                        Err(error) => {
                            tracing::warn!(
                                target: "neoism::desktop_daemon",
                                daemon = url,
                                %error,
                                "failed to start desktop daemon protocol pump"
                            );
                            None
                        }
                    }
                });
        if let Some(daemon) = daemon.as_ref() {
            router.attach_daemon_client(daemon.handle(), daemon.runtime_handle());
            router.request_window_list();
        }
        let external_command_listener =
            crate::ipc::listen_for_external_commands(event_proxy.clone());
        let _ = configuration_file_updates(
            neoism_backend::config::config_dir_path(),
            event_proxy.clone(),
        );
        let scheduler = Scheduler::new(proxy);
        let home_daemon_endpoint =
            daemon.as_ref().map(|daemon| daemon.endpoint().to_string());
        event_loop.listen_device_events(DeviceEvents::Never);

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        event_loop.set_confirm_before_quit(config.confirm_before_quit);

        neoism_notifier::request_authorization();

        Application {
            config,
            event_proxy,
            router,
            daemon,
            homing: HostHomingTracker::new(),
            pending_peer_adopt: None,
            home_daemon_endpoint,
            scheduler,
            app_id,
            initial_open_paths,
            _external_command_listener: external_command_listener,
        }
    }

    /// True when the live daemon connection is the one this desktop
    /// started on (its own daemon), false while dialled into a peer's
    /// daemon for a joined workspace.
    fn daemon_is_home(&self) -> bool {
        match (self.daemon.as_ref(), self.home_daemon_endpoint.as_deref()) {
            (Some(daemon), Some(home)) => daemon.endpoint() == home,
            _ => true,
        }
    }

    fn attach_daemon_to_window(&mut self, window_id: WindowId) {
        let is_home = self.daemon_is_home();
        let Some(daemon) = self.daemon.as_ref() else {
            return;
        };
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.window.screen.attach_daemon_client(
                daemon.handle(),
                daemon.runtime_handle(),
                daemon.endpoint().to_string(),
                is_home,
            );
        }
    }

    fn pump_daemon(&mut self, event_loop: &ActiveEventLoop) {
        let messages = self
            .daemon
            .as_ref()
            .map(DesktopDaemonConnection::drain_messages)
            .unwrap_or_default();
        for message in messages {
            match message {
                DaemonServerMessage::Workspace { message, .. } => {
                    self.apply_daemon_workspace_message(event_loop, message);
                }
                DaemonServerMessage::Editor { message, .. } => {
                    self.apply_daemon_editor_message(message);
                }
                DaemonServerMessage::Pty { message } => {
                    self.apply_daemon_pty_message(message);
                }
                DaemonServerMessage::Crdt { message, .. } => {
                    self.apply_daemon_crdt_message(message);
                }
                DaemonServerMessage::Files {
                    request_id,
                    message,
                } => {
                    for route in self.router.routes.values_mut() {
                        if route
                            .window
                            .screen
                            .apply_daemon_files_message(request_id, &message)
                        {
                            route.request_redraw();
                        }
                    }
                }
                DaemonServerMessage::Git {
                    request_id,
                    message,
                } => {
                    for route in self.router.routes.values_mut() {
                        if route
                            .window
                            .screen
                            .apply_daemon_git_message(request_id, &message)
                        {
                            route.request_redraw();
                        }
                    }
                }
            }
        }

        // Peer-workspace join (Workspaces pick on a tailnet peer's
        // workspace): re-dial the daemon connection to the owning host
        // — the host owns the daemon; joining means becoming a client
        // of it — then adopt once its tree lands (deferred via
        // `pending_peer_adopt`). When we're ALREADY dialled into that
        // host, skip the redial and adopt on the next tree push.
        let mut join_request: Option<(WindowId, String, String)> = None;
        for (window_id, route) in self.router.routes.iter_mut() {
            if let Some((workspace_id, daemon_url)) =
                route.window.screen.take_peer_workspace_join()
            {
                join_request = Some((*window_id, workspace_id, daemon_url));
            }
        }
        if let Some((window_id, workspace_id, daemon_url)) = join_request {
            self.pending_peer_adopt = Some((window_id, workspace_id));
            let already_connected = self
                .daemon
                .as_ref()
                .is_some_and(|daemon| daemon.endpoint() == daemon_url);
            if already_connected {
                // Same daemon — just re-request the tree; its arrival
                // completes the pending adopt.
                if let Some(daemon) = self.daemon.as_ref() {
                    daemon.send(WorkspaceClientMessage::RequestHostWorkspaceTree);
                }
            } else {
                self.redial_daemon(&daemon_url);
                // Ask the fresh daemon for its tree explicitly — the
                // pending adopt completes on its arrival.
                if let Some(daemon) = self.daemon.as_ref() {
                    daemon.send(WorkspaceClientMessage::RequestHostWorkspaceTree);
                }
            }
        }

        // Left the last joined workspace → return the daemon plane to
        // the home daemon this desktop started on.
        let go_home = self
            .router
            .routes
            .values_mut()
            .any(|route| route.window.screen.take_daemon_go_home());
        if go_home {
            if let Some(home) = self.home_daemon_endpoint.clone() {
                tracing::info!(
                    target: "neoism::workspaces",
                    daemon = %home,
                    "left last joined workspace; re-dialling home daemon"
                );
                self.pending_peer_adopt = None;
                self.redial_daemon(&home);
                if let Some(daemon) = self.daemon.as_ref() {
                    daemon.send(WorkspaceClientMessage::RequestHostWorkspaceTree);
                }
            }
        }

        let Some(daemon) = self.daemon.as_ref() else {
            return;
        };
        let mut outbound = Vec::new();
        let mut outbound_crdt = Vec::new();
        for route in self.router.routes.values_mut() {
            outbound.extend(route.window.screen.drain_daemon_pane_layout_requests());
            // Wave 7A presence publishes (coalesced cursor updates) +
            // Wave 7B local-edit choke point: flush markdown pane
            // mutations into CRDT ops once per pump turn.
            outbound_crdt.extend(route.window.screen.drain_daemon_presence_messages());
            let (markdown_crdt_messages, markdown_pane_changed) =
                route.window.screen.drain_markdown_crdt_messages();
            outbound_crdt.extend(markdown_crdt_messages);
            let (markdown_disk_messages, markdown_disk_changed) =
                route.window.screen.reload_open_markdown_files_from_disk();
            outbound_crdt.extend(markdown_disk_messages);
            if markdown_pane_changed || markdown_disk_changed {
                // Wave 7D: undo/redo intents mutate the pane inside the
                // drain (CRDT-routed history) — repaint that window.
                route.request_redraw();
            }
        }
        for message in outbound {
            daemon.send(message);
        }
        daemon.send_crdt_batch(outbound_crdt);
    }

    /// Fan an inbound CRDT message out to every window: the document
    /// plane (Wave 7B markdown bindings — snapshot seeds + remote syncs)
    /// and the presence plane (Wave 7A remote-cursor stores). Redraws
    /// windows whose visible state changed.
    fn apply_daemon_crdt_message(
        &mut self,
        message: neoism_protocol::crdt::CrdtServerMessage,
    ) {
        for route in self.router.routes.values_mut() {
            let markdown_changed =
                route.window.screen.apply_markdown_crdt_message(&message);
            let presence_changed =
                route.window.screen.apply_presence_crdt_message(&message);
            if markdown_changed || presence_changed {
                route.request_redraw();
            }
        }
    }

    fn apply_daemon_workspace_message(
        &mut self,
        event_loop: &ActiveEventLoop,
        message: WorkspaceServerMessage,
    ) {
        // Wave 4D: before fanning the message out to the chrome, watch for
        // the active workspace's home host flipping to a different machine.
        // If it moved to a host that advertises a dialable `daemon_url`, we
        // re-point this desktop's daemon connection there so the workspace
        // keeps showing at its new home. The daemon stays the source of
        // truth — we only re-dial which daemon we talk to.
        self.maybe_follow_workspace_rehome(&message);

        // MULTI-USER GUARD: window summaries only drive native window
        // bind/materialisation when we're dialled into our HOME
        // daemon. A peer's daemon (joined workspace) ships the HOST's
        // window inventory — materialising those spawned phantom OS
        // windows on the guest, and binding could hijack an existing
        // local window's identity.
        let windows_from_home = self.daemon_is_home();
        match &message {
            WorkspaceServerMessage::WindowList { windows } if windows_from_home => {
                for window in windows {
                    self.apply_daemon_window_summary(event_loop, window);
                }
            }
            WorkspaceServerMessage::WindowOpened { window }
            | WorkspaceServerMessage::WindowChanged { window }
                if windows_from_home =>
            {
                self.apply_daemon_window_summary(event_loop, window);
            }
            WorkspaceServerMessage::WindowClosed { window_id } if windows_from_home => {
                if let Some(native_id) = self.router.unbind_daemon_window(window_id) {
                    self.router.routes.remove(&native_id);
                    crate::app::freeze_watchdog::unregister_window(native_id);
                }
            }
            _ => {}
        }

        for route in self.router.routes.values_mut() {
            let context_changed = route
                .window
                .screen
                .context_manager
                .apply_workspace_server_message(message.clone());
            let screen_changed =
                route.window.screen.apply_daemon_server_message(&message);
            if context_changed || screen_changed {
                route.request_redraw();
            }
        }

        // Peer-workspace join, step 2: the redialled daemon's tree just
        // landed in the manager caches above. If it carries the picked
        // workspace, re-enter the open/adopt path — this time
        // `peer_workspace_daemon_url` resolves None (the workspace is
        // now on the linked daemon) and the normal adopt attaches its
        // live sessions over the new connection.
        if let Some((window_id, workspace_id)) = self.pending_peer_adopt.clone() {
            let carried = Self::workspace_summaries_from_message(&message)
                .iter()
                .any(|workspace| workspace.id == workspace_id);
            if carried {
                self.pending_peer_adopt = None;
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    tracing::info!(
                        target: "neoism::workspaces",
                        workspace_id = %workspace_id,
                        "peer workspace tree landed; adopting over the new daemon link"
                    );
                    route
                        .window
                        .screen
                        .open_or_adopt_daemon_workspace(workspace_id);
                    route.request_redraw();
                }
            }
        }
    }

    /// Wave 4D: cache `host_id -> daemon_url` and detect when the workspace
    /// we're viewing has been re-homed to a different host. On a resolvable
    /// flip, rebuild the daemon connection against the new host's
    /// `daemon_url` and swap it into `self.daemon`. Mirrors the web's
    /// `recordHostDaemonUrls` + `observeWorkspaceHoming` + `switchTo`.
    fn maybe_follow_workspace_rehome(&mut self, message: &WorkspaceServerMessage) {
        // Cache every host's advertised daemon_url (Wave 4E) so a re-home can
        // resolve `running_on_host_id` -> dialable URL directly.
        let hosts = Self::hosts_from_message(message);
        if !hosts.is_empty() {
            self.homing.record_host_daemon_urls(hosts);
        }

        // The summaries carried by this push (tree / list / control change).
        let workspaces = Self::workspace_summaries_from_message(message);
        if workspaces.is_empty() {
            return;
        }

        // Only meaningful while a connection exists — the endpoint is both
        // the loop guard ("don't re-dial the URL we're already on") and the
        // auth-bearing URL we fall back to.
        let Some(current_endpoint) = self
            .daemon
            .as_ref()
            .map(|daemon| daemon.endpoint().to_string())
        else {
            return;
        };

        let Some(target) = self
            .homing
            .observe_workspace_homing(workspaces, &current_endpoint)
        else {
            return;
        };

        tracing::info!(
            target: "neoism::desktop_daemon",
            workspace_id = %target.workspace_id,
            new_host = %target.new_host_id,
            from = %current_endpoint,
            to = %target.daemon_url,
            "active workspace re-homed; re-dialling daemon to follow it"
        );
        self.redial_daemon(&target.daemon_url);
    }

    /// Rebuild `DesktopDaemonConnection` against `daemon_url`, swap it into
    /// `self.daemon`, and re-run the fresh-connection bring-up that
    /// `App::new` does (attach the client to every route + request the
    /// window list). Auth/token travels in the `daemon_url` itself (the
    /// daemon advertises a dialable URL with any `?token=` baked in), so no
    /// separate credential plumbing is needed here. On failure we keep the
    /// existing connection rather than dropping the user offline.
    fn redial_daemon(&mut self, daemon_url: &str) {
        // Loop guard (belt and braces — `observe_workspace_homing` already
        // skips a no-op move, but guard again in case the live endpoint
        // changed between observation and here).
        if self
            .daemon
            .as_ref()
            .is_some_and(|daemon| daemon.endpoint() == daemon_url)
        {
            return;
        }

        let connection = match DesktopDaemonConnection::connect(
            daemon_url,
            self.event_proxy.clone(),
        ) {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(
                    target: "neoism::desktop_daemon",
                    daemon = daemon_url,
                    %error,
                    "failed to re-dial daemon on workspace re-home; keeping current connection"
                );
                return;
            }
        };

        // Swap first so the attach below points the chrome at the new client.
        // Dropping the old `DesktopDaemonConnection` shuts its runtime down.
        self.daemon = Some(connection);

        // Re-attach exactly like `App::new`: bind the new client+runtime to
        // every live route's screen, then request the window list so the new
        // daemon re-ships its window/workspace inventory.
        let is_home = self.daemon_is_home();
        if let Some(daemon) = self.daemon.as_ref() {
            let handle = daemon.handle();
            let runtime_handle = daemon.runtime_handle();
            let endpoint = daemon.endpoint().to_string();
            for route in self.router.routes.values_mut() {
                route.window.screen.attach_daemon_client(
                    handle.clone(),
                    runtime_handle.clone(),
                    endpoint.clone(),
                    is_home,
                );
            }
            self.router
                .attach_daemon_client(daemon.handle(), daemon.runtime_handle());
            self.router.request_window_list();
        }
    }

    /// Hosts carried by a workspace-tree / host-list message — the source of
    /// `daemon_url` advertisements. Mirrors the web `hostsFromMessage`.
    fn hosts_from_message(message: &WorkspaceServerMessage) -> &[HostSummary] {
        match message {
            WorkspaceServerMessage::HostWorkspaceTree { hosts, .. }
            | WorkspaceServerMessage::HostList { hosts } => hosts,
            _ => &[],
        }
    }

    /// Workspace summaries carried by a tree / list / control-change message
    /// — the source of `running_on_host_id`. Mirrors the web
    /// `workspaceSummariesFromMessage`.
    fn workspace_summaries_from_message(
        message: &WorkspaceServerMessage,
    ) -> &[WorkspaceSummary] {
        match message {
            WorkspaceServerMessage::HostWorkspaceTree { workspaces, .. }
            | WorkspaceServerMessage::HostWorkspaceList { workspaces } => workspaces,
            WorkspaceServerMessage::WorkspaceControlChanged { workspace } => {
                std::slice::from_ref(workspace)
            }
            _ => &[],
        }
    }

    fn apply_daemon_editor_message(
        &mut self,
        message: neoism_protocol::editor::EditorServerMessage,
    ) {
        for route in self.router.routes.values_mut() {
            if route
                .window
                .screen
                .apply_daemon_editor_message(message.clone())
            {
                route.request_redraw();
            }
        }
    }

    fn apply_daemon_pty_message(&mut self, message: neoism_protocol::pty::ServerMessage) {
        for route in self.router.routes.values_mut() {
            if route
                .window
                .screen
                .context_manager
                .apply_pty_server_message(message.clone())
            {
                route.request_redraw();
            }
        }
    }

    fn apply_daemon_window_summary(
        &mut self,
        event_loop: &ActiveEventLoop,
        window: &WorkspaceWindowSummary,
    ) {
        // Belt-and-braces with the gate in
        // `apply_daemon_workspace_message`: never bind/materialise
        // native windows from a PEER daemon's window inventory.
        if !self.daemon_is_home() {
            return;
        }
        // MULTI-CLIENT GUARD: several desktops can share one daemon
        // (two clients of the same host daemon). A window whose
        // workspace belongs to ANOTHER host is that desktop's window —
        // materialising it here spawned phantom OS windows mirroring
        // the other user's session.
        if let Some(workspace_id) = window.workspace_id.as_deref() {
            let foreign = self.router.routes.values().next().is_some_and(|route| {
                let manager = &route.window.screen.context_manager;
                !manager.workspace_owned_locally(workspace_id)
                    && manager.daemon_workspace_host_id(workspace_id).is_some()
            });
            if foreign {
                tracing::info!(
                    target: "neoism::workspaces",
                    window_id = %window.id,
                    workspace_id = %workspace_id,
                    "skipping daemon window owned by another desktop"
                );
                return;
            }
        }
        if let Some(native_id) = self.router.bind_or_materialize_daemon_window(
            event_loop,
            self.event_proxy.clone(),
            &self.config,
            window,
            self.app_id.as_deref(),
        ) {
            self.attach_daemon_to_window(native_id);
            if let Some(route) = self.router.routes.get_mut(&native_id) {
                route.request_redraw();
            }
        }
    }

    fn skip_window_event(event: &WindowEvent) -> bool {
        matches!(
            event,
            WindowEvent::KeyboardInput {
                is_synthetic: true,
                ..
            } | WindowEvent::ActivationTokenDone { .. }
                | WindowEvent::DoubleTapGesture { .. }
                | WindowEvent::TouchpadPressure { .. }
                | WindowEvent::RotationGesture { .. }
                | WindowEvent::CursorEntered { .. }
                | WindowEvent::AxisMotion { .. }
                | WindowEvent::PanGesture { .. }
                | WindowEvent::HoveredFileCancelled
                | WindowEvent::HoveredFile(_)
                | WindowEvent::Moved(_)
        )
    }

    fn rio_event_name(event: &RioEventType) -> &'static str {
        match event {
            RioEventType::Frame => "Frame",
            RioEventType::Rio(event) => match event {
                RioEvent::Render => "RioEvent::Render",
                RioEvent::RenderRoute(_) => "RioEvent::RenderRoute",
                RioEvent::TerminalDamaged(_) => "RioEvent::TerminalDamaged",
                RioEvent::UpdateGraphics { .. } => "RioEvent::UpdateGraphics",
                RioEvent::PrepareRender(_) => "RioEvent::PrepareRender",
                RioEvent::PrepareRenderOnRoute(_, _) => "RioEvent::PrepareRenderOnRoute",
                RioEvent::UpdateTitles => "RioEvent::UpdateTitles",
                RioEvent::UpdateConfig => "RioEvent::UpdateConfig",
                RioEvent::CreateWindow(_) => "RioEvent::CreateWindow",
                RioEvent::CreateWindowWithOptions { .. } => {
                    "RioEvent::CreateWindowWithOptions"
                }
                RioEvent::CloseWindow => "RioEvent::CloseWindow",
                RioEvent::PtyWrite(_, _) => "RioEvent::PtyWrite",
                RioEvent::Scroll(_) => "RioEvent::Scroll",
                RioEvent::MouseCursorDirty => "RioEvent::MouseCursorDirty",
                RioEvent::AcpWake => "RioEvent::AcpWake",
                RioEvent::WorkspaceNotesWake => "RioEvent::WorkspaceNotesWake",
                RioEvent::NotebookStatusTick => "RioEvent::NotebookStatusTick",
                _ => "RioEvent::Other",
            },
        }
    }

    fn window_event_name(event: &WindowEvent) -> &'static str {
        match event {
            WindowEvent::RedrawRequested => "WindowEvent::RedrawRequested",
            WindowEvent::CloseRequested => "WindowEvent::CloseRequested",
            WindowEvent::Resized(_) => "WindowEvent::Resized",
            WindowEvent::ScaleFactorChanged { .. } => "WindowEvent::ScaleFactorChanged",
            WindowEvent::Focused(_) => "WindowEvent::Focused",
            WindowEvent::Occluded(_) => "WindowEvent::Occluded",
            WindowEvent::KeyboardInput { .. } => "WindowEvent::KeyboardInput",
            WindowEvent::MouseInput { .. } => "WindowEvent::MouseInput",
            WindowEvent::MouseWheel { .. } => "WindowEvent::MouseWheel",
            WindowEvent::CursorMoved { .. } => "WindowEvent::CursorMoved",
            WindowEvent::CursorLeft { .. } => "WindowEvent::CursorLeft",
            WindowEvent::Ime(_) => "WindowEvent::Ime",
            WindowEvent::Touch(_) => "WindowEvent::Touch",
            WindowEvent::ThemeChanged(_) => "WindowEvent::ThemeChanged",
            WindowEvent::DroppedFile(_) => "WindowEvent::DroppedFile",
            _ => "WindowEvent::Other",
        }
    }

    pub fn run(
        &mut self,
        event_loop: EventLoop<EventPayload>,
    ) -> Result<(), Box<dyn Error>> {
        let result = event_loop.run_app(self);
        result.map_err(Into::into)
    }

    fn request_event_loop_redraws(&mut self) -> Option<Instant> {
        if self.config.renderer.strategy.is_game() {
            return None;
        }

        let mut next_deadline: Option<Instant> = None;
        for (window_id, route) in self.router.routes.iter_mut() {
            if self.config.renderer.disable_unfocused_render && !route.window.is_focused {
                continue;
            }
            if self.config.renderer.disable_occluded_render
                && route.window.is_occluded
                && !route.window.needs_render_after_occlusion
            {
                continue;
            }

            let now = Instant::now();
            let pending_dirty = route
                .window
                .screen
                .ctx()
                .current()
                .renderable_content
                .pending_update
                .is_dirty();
            let redraw_reason = route.window.screen.renderer.redraw_reason();
            let animating = redraw_reason.is_some();
            let redraw_pending = route.redraw_request_pending();
            let redraw_retry_deadline = route.redraw_retry_deadline();
            let redraw_retry_due =
                redraw_retry_deadline.is_some_and(|deadline| deadline <= now);
            let current_route = route.window.screen.ctx().current_route();
            let notebook_running = route
                .window
                .screen
                .ctx()
                .current()
                .notebook
                .as_ref()
                .is_some_and(|notebook| notebook.has_running_cells());
            if notebook_running {
                let timer_id = TimerId::new(Topic::NotebookStatus, current_route);
                if !self.scheduler.scheduled(timer_id) {
                    crate::app::freeze_watchdog::note_sampled(
                        format!("notebook_status_schedule:{window_id:?}"),
                        Duration::from_secs(2),
                        format!(
                            "notebook_status_schedule window={window_id:?} route_id={current_route} tick_ms={NOTEBOOK_STATUS_TICK_MS}"
                        ),
                    );
                }
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    timer_id,
                    NOTEBOOK_STATUS_TICK_MS,
                    RioEvent::NotebookStatusTick,
                    *window_id,
                );
            }

            if route.path == RoutePath::Welcome
                || route.path == RoutePath::ConfirmQuit
                || pending_dirty
                || animating
                || redraw_pending
            {
                let redraw_detail = if let Some(reason) = redraw_reason {
                    format!("event_loop:{reason}")
                } else if pending_dirty {
                    "event_loop:pending_dirty".to_string()
                } else if redraw_pending {
                    "event_loop:redraw_pending_retry".to_string()
                } else {
                    format!("event_loop:{:?}", route.path)
                };
                if redraw_pending && !redraw_retry_due {
                    crate::app::freeze_watchdog::note_sampled(
                        format!("redraw_pending_wait:{window_id:?}"),
                        FRAME_WATCHDOG_NOTE_INTERVAL,
                        format!(
                            "redraw_pending_wait window={window_id:?} route_path={:?} pending_dirty={} animating={} reason={} retry_deadline_ms={}",
                            route.path,
                            pending_dirty,
                            animating,
                            redraw_reason.unwrap_or("none"),
                            redraw_retry_deadline
                                .map(|deadline| deadline.saturating_duration_since(now).as_millis())
                                .unwrap_or_default()
                        ),
                    );
                    if let Some(deadline) = redraw_retry_deadline {
                        next_deadline =
                            Some(next_deadline.map_or(deadline, |old| old.min(deadline)));
                    }
                    continue;
                }

                if animating && !redraw_pending && !redraw_retry_due {
                    if let Some(wait) = route.window.wait_until() {
                        if let Some(reason) = redraw_reason {
                            crate::app::freeze_watchdog::note_sampled(
                                format!("frame_source:{window_id:?}:{reason}"),
                                FRAME_WATCHDOG_NOTE_INTERVAL,
                                format!(
                                    "frame_source window={window_id:?} route_path={:?} reason={reason} wait_ms={} pending_dirty={} redraw_pending={} redraw_retry_due={} notebook_running={notebook_running}",
                                    route.path,
                                    wait.as_millis(),
                                    pending_dirty,
                                    redraw_pending,
                                    redraw_retry_due
                                ),
                            );
                        }
                        let timer_id = TimerId::new(
                            Topic::Render,
                            route.window.screen.ctx().current_route(),
                        );
                        Self::debounce_follow_up(
                            &mut self.scheduler,
                            timer_id,
                            wait.as_millis().max(1) as u64,
                            RioEvent::Render,
                            *window_id,
                        );
                        let deadline = redraw_retry_deadline
                            .map(|retry| retry.min(now + wait))
                            .unwrap_or(now + wait);
                        next_deadline =
                            Some(next_deadline.map_or(deadline, |old| old.min(deadline)));
                        continue;
                    }
                }
                let requested = route.request_redraw_with_reason(&redraw_detail);
                if !requested {
                    if let Some(deadline) = redraw_retry_deadline {
                        next_deadline =
                            Some(next_deadline.map_or(deadline, |old| old.min(deadline)));
                    }
                    continue;
                }

                tracing::trace!(
                    target: "neoism::frame_pacing",
                    ?window_id,
                    pending_dirty,
                    animating,
                    redraw_pending,
                    redraw_retry_due,
                    redraw_reason,
                    route_path = ?route.path,
                    "requesting redraw from event loop"
                );

                let deadline = if redraw_retry_due {
                    now
                } else {
                    route
                        .window
                        .wait_until()
                        .map(|wait| now + wait)
                        .unwrap_or(now)
                };
                next_deadline =
                    Some(next_deadline.map_or(deadline, |old| old.min(deadline)));
            }
        }
        next_deadline
    }

    fn schedule_next_event(&mut self, event_loop: &ActiveEventLoop) {
        let redraw_deadline = self.request_event_loop_redraws();
        let scheduler_deadline = self.scheduler.update();
        let next_deadline = match (redraw_deadline, scheduler_deadline) {
            (Some(redraw), Some(scheduled)) => Some(redraw.min(scheduled)),
            (Some(redraw), None) => Some(redraw),
            (None, Some(scheduled)) => Some(scheduled),
            (None, None) => None,
        };
        let control_flow = match next_deadline {
            Some(instant) => ControlFlow::WaitUntil(instant),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(control_flow);
    }
}

impl ApplicationHandler<EventPayload> for Application<'_> {
    fn resumed(&mut self, _active_event_loop: &ActiveEventLoop) {}

    fn new_events(&mut self, event_loop: &ActiveEventLoop, cause: StartCause) {
        self.pump_daemon(event_loop);
        crate::app::freeze_watchdog::mark_global("new_events", format!("{cause:?}"));
        if cause != StartCause::Init
            && cause != StartCause::CreateWindow
            && cause != StartCause::MacOSReopen
        {
            self.schedule_next_event(event_loop);
            return;
        }

        if cause == StartCause::MacOSReopen && !self.router.routes.is_empty() {
            return;
        }

        let theme = self
            .config
            .force_theme
            .map(|t| t.to_window_theme())
            .or(event_loop.system_theme());
        apply_theme_to_config(&mut self.config, theme);

        self.router.request_open_window(None, None);
        let window_id = self.router.create_window(
            event_loop,
            self.event_proxy.clone(),
            &self.config,
            None,
            self.app_id.as_deref(),
        );
        self.attach_daemon_to_window(window_id);
        crate::app::freeze_watchdog::mark_window_event(
            window_id,
            "window_created",
            format!("cause={cause:?}"),
        );

        let paths = std::mem::take(&mut self.initial_open_paths);
        if !paths.is_empty() {
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                for path in paths {
                    route.window.screen.open_path_in_editor(path);
                }
                route.request_redraw();
            }
        }

        // Schedule title updates every 2s
        let timer_id = TimerId::new(Topic::UpdateTitles, 0);
        if !self.scheduler.scheduled(timer_id) {
            self.scheduler.schedule(
                EventPayload::new(RioEventType::Rio(RioEvent::UpdateTitles), unsafe {
                    neoism_window::window::WindowId::dummy()
                }),
                Duration::from_secs(2),
                true,
                timer_id,
            );
        }

        tracing::info!("Initialisation complete");
        self.schedule_next_event(event_loop);
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: EventPayload) {
        self.pump_daemon(event_loop);
        let window_id = event.window_id;
        let event_name = Self::rio_event_name(&event.payload);
        let _watchdog_span = crate::app::freeze_watchdog::global_span(
            "user_event",
            format!("{event_name} window_id={window_id:?}"),
        );
        crate::app::freeze_watchdog::mark_window_event(
            window_id,
            event_name,
            "user_event",
        );
        match event.payload {
            RioEventType::Rio(RioEvent::Render) => {
                self.apply_render_arm(window_id);
            }
            RioEventType::Rio(RioEvent::RenderRoute(route_id)) => {
                self.apply_render_route_arm(window_id, route_id);
            }

            RioEventType::Rio(RioEvent::TerminalDamaged(route_id)) => {
                self.apply_terminal_damaged_arm(window_id, route_id);
            }
            RioEventType::Rio(RioEvent::UpdateGraphics {
                route_id: _,
                queues,
            }) => {
                self.apply_update_graphics(window_id, queues);
            }
            RioEventType::Rio(RioEvent::PrepareUpdateConfig) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::UpdateConfig, 0),
                    250,
                    RioEvent::UpdateConfig,
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::ReportToAssistant(error)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.report_error(&error);
                }
            }
            RioEventType::Rio(RioEvent::UpdateConfig) => {
                self.apply_update_config();
            }
            RioEventType::Rio(RioEvent::Exit | RioEvent::Quit) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    match quit_request_action(self.config.confirm_before_quit) {
                        QuitRequestAction::ConfirmQuitAndRedraw => {
                            route.confirm_quit();
                            route.request_redraw();
                        }
                        QuitRequestAction::QuitImmediately => {
                            route.quit();
                        }
                    }
                }
            }
            RioEventType::Rio(RioEvent::CloseTerminal(route_id)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    let handled = route.window.screen.handle_terminal_exit(route_id);
                    match close_terminal_action(handled) {
                        CloseTerminalAction::RemoveRouteAndMaybeExit => {
                            self.router.unbind_native_window(window_id);
                            self.router.routes.remove(&window_id);
                            crate::app::freeze_watchdog::unregister_window(window_id);
                            // Unschedule pending events.
                            self.scheduler.unschedule_window(route_id);
                            if should_exit_event_loop_after_route_removed(
                                self.router.routes.len(),
                            ) {
                                event_loop.exit();
                            }
                        }
                        CloseTerminalAction::ResizeAfterClose => {
                            let size = route.window.screen.context_manager.len();
                            route.window.screen.resize_top_or_bottom_line(size);
                        }
                    }
                }
            }
            RioEventType::Rio(RioEvent::CursorBlinkingChange) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::CursorBlinkingChangeOnRoute(route_id)) => {
                self.apply_cursor_blinking_change_on_route(window_id, route_id);
            }
            RioEventType::Rio(RioEvent::ProgressReport(report)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    let has_island = route.window.screen.renderer.island.is_some();
                    if should_apply_progress_report(has_island) {
                        if let Some(island) = &mut route.window.screen.renderer.island {
                            island.set_progress_report(
                                crate::app::window_event::keyboard::island_progress_report_from_backend(report),
                            );
                            route.request_redraw();
                        }
                    }
                }
            }
            RioEventType::Rio(RioEvent::SelectionScrollTick) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.selection_scroll_tick();
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::NotebookStatusTick) => {
                self.apply_notebook_status_tick(window_id);
            }
            RioEventType::Rio(RioEvent::PrepareRefreshFileTreeGitStatus) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::FileTreeGitStatus, 0),
                    100,
                    RioEvent::RefreshFileTreeGitStatus,
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::PrepareRefreshFileTree) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::FileTree, 0),
                    200,
                    RioEvent::RefreshFileTree,
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::RefreshFileTreeGitStatus) => {
                for route in self.router.routes.values_mut() {
                    if matches!(
                        refresh_redraw_action(
                            route.window.screen.refresh_file_tree_git_status()
                        ),
                        RefreshRedrawAction::Redraw
                    ) {
                        route.request_redraw();
                    }
                }
            }
            RioEventType::Rio(RioEvent::RefreshFileTree) => {
                for route in self.router.routes.values_mut() {
                    let tree_redraw = matches!(
                        refresh_redraw_action(route.window.screen.refresh_file_tree()),
                        RefreshRedrawAction::Redraw
                    );
                    // The fs watcher is rooted at the workspace, which
                    // also houses the notes vault — keep the open Alt+N
                    // panel live on the same signal.
                    let notes_redraw =
                        route.window.screen.refresh_notes_sidebar_if_visible();
                    if tree_redraw || notes_redraw {
                        route.request_redraw();
                    }
                }
            }
            RioEventType::Rio(RioEvent::RemoteFileTreeCheck) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.retry_remote_file_tree_if_stalled();
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::ApplyFileTreeGitStatus) => {
                for route in self.router.routes.values_mut() {
                    if matches!(
                        refresh_redraw_action(
                            route.window.screen.apply_file_tree_git_status_refresh()
                        ),
                        RefreshRedrawAction::Redraw
                    ) {
                        route.request_redraw();
                    }
                }
            }
            RioEventType::Rio(RioEvent::AcpWake) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.drain_acp_events();
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::WorkspaceNotesWake) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.drain_workspace_note_index_events();
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::Bell) => {
                if should_play_audio_bell(self.config.bell.audio) {
                    play_audio_bell();
                }
            }
            RioEventType::Rio(RioEvent::DesktopNotification { title, body }) => {
                if should_send_desktop_notification() {
                    send_desktop_notification(&title, &body);
                }
            }
            RioEventType::Rio(RioEvent::PrepareRender(millis)) => {
                if let Some(route) = self.router.routes.get(&window_id) {
                    let route_id = route.window.screen.ctx().current_route();
                    Self::debounce_follow_up(
                        &mut self.scheduler,
                        TimerId::new(Topic::Render, route_id),
                        millis,
                        RioEvent::Render,
                        window_id,
                    );
                }
            }
            RioEventType::Rio(RioEvent::PrepareRenderOnRoute(millis, route_id)) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::RenderRoute, route_id),
                    millis,
                    RioEvent::RenderRoute(route_id),
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::BlinkCursor(millis, route_id)) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::CursorBlinking, route_id),
                    millis,
                    RioEvent::CursorBlinkingChangeOnRoute(route_id),
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::Title(title)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.set_window_title(&title);
                }
            }
            RioEventType::Rio(RioEvent::TitleWithSubtitle(title, subtitle)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.set_window_title(&title);
                    route.set_window_subtitle(&subtitle);
                }
            }
            RioEventType::Rio(RioEvent::UpdateTitles) => {
                self.router.update_titles();
            }
            RioEventType::Rio(RioEvent::MouseCursorDirty) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.reset_mouse();
                }
            }
            RioEventType::Rio(RioEvent::Scroll(scroll)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    let mut terminal = route
                        .window
                        .screen
                        .context_manager
                        .current_mut()
                        .terminal
                        .lock();
                    terminal.scroll_display(scroll);
                    drop(terminal);
                }
            }
            RioEventType::Rio(RioEvent::ClipboardLoad(
                route_id,
                clipboard_type,
                format,
            )) => {
                self.apply_clipboard_load(window_id, route_id, clipboard_type, format);
            }
            RioEventType::Rio(RioEvent::ClipboardStore(clipboard_type, content)) => {
                let Router {
                    routes, clipboard, ..
                } = &mut self.router;
                if let Some(route) = routes.get_mut(&window_id) {
                    if should_store_clipboard(route.window.is_focused) {
                        clipboard.set(clipboard_type, content);
                    }
                }
            }
            RioEventType::Rio(RioEvent::IdeToolInstallFinished {
                tool,
                success,
                message,
            }) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route
                        .window
                        .screen
                        .handle_ide_tool_install_finished(tool, success, message);
                    route.request_redraw();
                }
            }
            RioEventType::Rio(RioEvent::OpenEditorTab { route_id: _, path }) => {
                self.apply_open_editor_tab(window_id, path);
            }
            RioEventType::Rio(RioEvent::PtyWrite(route_id, text)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    if let Some(context_item) =
                        route.window.screen.ctx_mut().get_by_route_id(route_id)
                    {
                        context_item
                            .context_mut()
                            .messenger
                            .send_bytes(text.into_bytes());
                    }
                }
            }
            RioEventType::Rio(RioEvent::TextAreaSizeRequest(route_id, format)) => {
                self.apply_text_area_size_request(window_id, route_id, format);
            }
            RioEventType::Rio(RioEvent::ColorRequest(route_id, index, format)) => {
                self.apply_color_request(window_id, route_id, index, format);
            }
            RioEventType::Rio(RioEvent::CreateWindow(working_dir_override)) => {
                self.apply_create_window(event_loop, working_dir_override);
            }
            RioEventType::Rio(RioEvent::CreateWindowWithOptions {
                working_dir,
                open_paths,
            }) => {
                self.apply_create_window_with_options(
                    event_loop,
                    working_dir,
                    open_paths,
                );
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::CreateNativeTab(working_dir_overwrite)) => {
                self.apply_create_native_tab(
                    event_loop,
                    window_id,
                    working_dir_overwrite,
                );
            }
            RioEventType::Rio(RioEvent::CreateConfigEditor) => {
                self.apply_create_config_editor(event_loop);
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::CloseWindow) => {
                self.router.request_close_native_window(window_id);
                self.router.unbind_native_window(window_id);
                self.router.routes.remove(&window_id);
                crate::app::freeze_watchdog::unregister_window(window_id);
                if should_exit_event_loop_after_close_window(
                    self.router.routes.len(),
                    self.config.confirm_before_quit,
                ) {
                    event_loop.exit();
                }
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::SelectNativeTabByIndex(tab_index)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.winit_window.select_tab_at_index(tab_index);
                }
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::SelectNativeTabLast) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route
                        .window
                        .winit_window
                        .select_tab_at_index(route.window.winit_window.num_tabs() - 1);
                }
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::SelectNativeTabNext) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.winit_window.select_next_tab();
                }
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::SelectNativeTabPrev) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.winit_window.select_previous_tab();
                }
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::Hide) => {
                event_loop.hide_application();
            }
            #[cfg(target_os = "macos")]
            RioEventType::Rio(RioEvent::HideOtherApplications) => {
                event_loop.hide_other_applications();
            }
            RioEventType::Rio(RioEvent::Minimize(set_minimize)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.winit_window.set_minimized(set_minimize);
                }
            }
            RioEventType::Rio(RioEvent::ToggleFullScreen) => {
                self.apply_toggle_fullscreen(window_id);
            }
            RioEventType::Rio(RioEvent::ToggleAppearanceTheme) => {
                self.apply_toggle_appearance_theme(window_id);
            }
            RioEventType::Rio(RioEvent::ColorChange(route_id, index, color)) => {
                self.apply_color_change(window_id, route_id, index, color);
            }
            _ => {}
        }
    }

    #[cfg(target_os = "macos")]
    fn open_urls(&mut self, active_event_loop: &ActiveEventLoop, urls: Vec<String>) {
        if !self.config.navigation.is_native() {
            for url in urls {
                self.router.request_open_window(None, None);
                let window_id = self.router.create_window(
                    active_event_loop,
                    self.event_proxy.clone(),
                    &self.config,
                    Some(url),
                    self.app_id.as_deref(),
                );
                self.attach_daemon_to_window(window_id);
            }
            return;
        }

        let mut tab_id = None;

        // In case only have one window
        for (_, route) in self.router.routes.iter() {
            if tab_id.is_none() {
                tab_id = Some(route.window.winit_window.tabbing_identifier());
            }

            if route.window.is_focused {
                tab_id = Some(route.window.winit_window.tabbing_identifier());
                break;
            }
        }

        if tab_id.is_some() {
            for url in urls {
                let parent_window_id = self
                    .router
                    .get_focused_route()
                    .and_then(|id| self.router.daemon_window_for_native(id))
                    .map(str::to_string);
                self.router
                    .request_open_native_tab(None, parent_window_id, None);
                let window_id = self.router.create_native_tab(
                    active_event_loop,
                    self.event_proxy.clone(),
                    &self.config,
                    tab_id.as_deref(),
                    Some(url),
                );
                self.attach_daemon_to_window(window_id);
            }
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        self.pump_daemon(event_loop);
        // Ignore all events we do not care about.
        if Self::skip_window_event(&event) {
            return;
        }

        let event_name = Self::window_event_name(&event);
        let _watchdog_span = crate::app::freeze_watchdog::global_span(
            "window_event",
            format!("{event_name} window_id={window_id:?}"),
        );

        {
            let route_path = match self.router.routes.get(&window_id) {
                Some(window) => window.path,
                None => return,
            };
            crate::app::freeze_watchdog::mark_window_event(
                window_id,
                event_name,
                format!("route_path={route_path:?}"),
            );
        }

        match event {
            WindowEvent::CloseRequested => {
                self.handle_close_requested(event_loop, window_id);
            }
            WindowEvent::Destroyed => {
                self.handle_destroyed(event_loop, window_id);
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.handle_modifiers_changed(window_id, modifiers);
            }
            WindowEvent::MouseInput { state, button, .. } => {
                self.handle_mouse_input(window_id, state, button);
                // A workspace-strip detach release parks a lifted grid
                // on the source screen; complete the hand-off here where
                // `event_loop` is available to spawn the new window.
                self.finish_pending_workspace_detaches(event_loop);
                // A right-click "Move to Workspace (other window)" parks a
                // cross-window tab move; complete it where the router can
                // borrow both windows.
                self.finish_pending_cross_window_tab_moves();
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.handle_cursor_moved(window_id, position);
            }
            WindowEvent::CursorLeft { .. } => {
                self.handle_cursor_left(window_id);
            }
            WindowEvent::MouseWheel { delta, phase, .. } => {
                self.handle_mouse_wheel(window_id, delta, phase);
            }
            WindowEvent::PinchGesture { delta, .. } => {
                self.handle_pinch_gesture(window_id, delta);
            }
            WindowEvent::KeyboardInput {
                is_synthetic: false,
                event: key_event,
                ..
            } => {
                self.handle_keyboard_input(window_id, key_event);
            }
            WindowEvent::Ime(ime) => {
                self.handle_ime(window_id, ime);
            }
            WindowEvent::Touch(touch) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    on_touch(route, touch, &mut self.router.clipboard);
                }
            }
            WindowEvent::Focused(focused) => {
                self.handle_focused(window_id, focused);
            }
            WindowEvent::Occluded(occluded) => {
                self.handle_occluded(window_id, occluded);
            }
            WindowEvent::ThemeChanged(new_theme) => {
                self.handle_theme_changed(window_id, new_theme);
            }
            WindowEvent::DroppedFile(path) => {
                self.handle_dropped_file(window_id, path);
            }
            WindowEvent::Resized(new_size) => {
                self.handle_resized(window_id, new_size);
            }
            WindowEvent::ScaleFactorChanged {
                inner_size_writer: _,
                scale_factor,
            } => {
                self.handle_scale_factor_changed(window_id, scale_factor);
            }
            WindowEvent::RedrawRequested => {
                self.handle_redraw_requested(event_loop, window_id);
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let _watchdog_span =
            crate::app::freeze_watchdog::global_span("about_to_wait", "");
        self.pump_daemon(event_loop);
        self.schedule_next_event(event_loop);
    }

    fn open_config(&mut self, event_loop: &ActiveEventLoop) {
        if self.config.navigation.open_config_with_split {
            self.router.open_config_split(&self.config);
        } else {
            self.router.request_open_config_editor(None);
            self.router.open_config_window(
                event_loop,
                self.event_proxy.clone(),
                &self.config,
            );
        }
    }

    fn hook_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        key: &neoism_window::event::KeyEvent,
        modifiers: &neoism_window::event::Modifiers,
    ) {
        let window_id = match self.router.get_focused_route() {
            Some(window_id) => window_id,
            None => {
                tracing::trace!(target: "neoism::input", "hook_event ignored: no focused route");
                return;
            }
        };

        let route = match self.router.routes.get_mut(&window_id) {
            Some(window) => window,
            None => {
                tracing::trace!(
                    target: "neoism::input",
                    ?window_id,
                    "hook_event ignored: focused route missing"
                );
                return;
            }
        };

        // For menu-triggered events, we need to temporarily set the correct modifiers
        // since menu events don't trigger ModifiersChanged events.
        let original_modifiers = route.window.screen.modifiers;

        // Use the modifiers passed from the menu action
        route.window.screen.set_modifiers(*modifiers);
        tracing::trace!(
            target: "neoism::input",
            ?window_id,
            state = ?key.state,
            repeat = key.repeat,
            logical_key = ?key.logical_key,
            physical_key = ?key.physical_key,
            location = ?key.location,
            text = ?key.text,
            text_with_all_modifiers = ?key.text_with_all_modifiers(),
            modifiers = ?modifiers.state(),
            "hook_event dispatching menu keyboard input"
        );

        // Process the key event
        route
            .window
            .screen
            .process_key_event(key, &mut self.router.clipboard);

        // Restore the original modifiers
        route.window.screen.set_modifiers(original_modifiers);
    }

    // Emitted when the event loop is being shut down.
    // This is irreversible - if this event is emitted, it is guaranteed to be the last event that gets emitted.
    // You generally want to treat this as an “do on quit” event.
    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        // SAFETY: The clipboard must be dropped before the event loop, so
        // replace it with a safe no-op placeholder.
        self.router.clipboard = Clipboard::new_nop();

        // Do not synchronously drop routes here. Native Vulkan renderer
        // teardown waits for the device to idle in several Drop impls,
        // which makes "close last window" visibly stall while the process
        // is already committed to exiting. `process::exit` lets the OS
        // reclaim window/GPU resources without blocking the UI thread.
        std::process::exit(0);
    }
}
