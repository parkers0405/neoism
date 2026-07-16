use crate::app::bell::{play_audio_bell, send_desktop_notification};
use crate::app::daemon_pump::DesktopDaemonConnection;
use crate::app::scheduler::{Scheduler, TimerId, Topic};
use crate::app::window_event::touch::on_touch;
use crate::bridges::utils::apply_theme_to_config;
use crate::daemon_client::DaemonServerMessage;
use crate::router::{routes::RoutePath, Router};
use crate::terminal::watcher::configuration_file_updates;
use neoism_backend::clipboard::Clipboard;
use neoism_backend::event::{EventPayload, EventProxy, RioEvent, RioEventType};
use neoism_protocol::workspace::{
    WorkspaceClientMessage, WorkspaceServerMessage, WorkspaceSummary,
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
use std::collections::HashMap;
use std::collections::HashSet;
use std::error::Error;
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use crate::server_registry::ServerRegistry;

/// Result of an off-thread server-switch dial, completed by the pump.
struct PendingServerSwitch {
    window_id: WindowId,
    daemon_url: String,
    server_id: Option<String>,
    result: Result<DesktopDaemonConnection, String>,
}

const NOTEBOOK_STATUS_TICK_MS: u64 = 500;
const FRAME_WATCHDOG_NOTE_INTERVAL: Duration = Duration::from_secs(1);

pub mod bell;
pub mod daemon_pump;
pub mod editor_grid_diag;
pub mod freeze_watchdog;
pub mod ime;
pub mod messenger;
pub mod scheduler;
pub mod user_event_dispatch;
pub mod window_event;
pub mod window_server_session;

use window_server_session::{ServerConnectionStatus, WindowServerSession};

pub struct Application<'a> {
    config: neoism_backend::config::Config,
    event_proxy: EventProxy,
    router: Router<'a>,
    bootstrap_daemon: Option<DesktopDaemonConnection>,
    window_sessions: HashMap<WindowId, WindowServerSession>,
    /// Monotonic ordinal for window profile ids. Profiles key persisted
    /// workspace subscriptions in `servers.json`, so they must be STABLE
    /// across restarts — the Nth window a process creates is always
    /// `window-N`, and the main window (`window-1`) always re-matches
    /// its stored subscriptions. A random id here made every stored
    /// subscription unreachable after a relaunch.
    window_profile_seq: u64,
    /// The daemon endpoint this desktop started on. Leaving the last
    /// joined peer workspace re-dials back here so the daemon plane
    /// returns home.
    home_daemon_endpoint: Option<String>,
    server_registry: ServerRegistry,
    bootstrap_server_id: Option<String>,
    server_health: HashMap<String, neoism_ui::panels::ServerIndicatorStatus>,
    server_health_inflight: HashSet<String>,
    server_health_tx: mpsc::Sender<(String, bool)>,
    server_health_rx: mpsc::Receiver<(String, bool)>,
    /// Completed off-thread server-switch connections. Dialling a daemon
    /// blocks up to the handshake timeout, so `switch_window_server` runs
    /// it on a thread and the pump finishes the swap here — the UI shows
    /// "connecting" instead of freezing.
    server_switch_tx: mpsc::Sender<PendingServerSwitch>,
    server_switch_rx: mpsc::Receiver<PendingServerSwitch>,
    server_switch_inflight: HashSet<WindowId>,
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
        daemon_token: Option<String>,
        initial_server_id: Option<String>,
        home_daemon_endpoint: Option<String>,
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
        let daemon = daemon_url.as_deref().and_then(|url| {
            match DesktopDaemonConnection::connect_with_token(
                url,
                daemon_token.clone(),
                event_proxy.clone(),
            ) {
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
        let external_command_listener =
            crate::ipc::listen_for_external_commands(event_proxy.clone());
        let _ = configuration_file_updates(
            neoism_backend::config::config_dir_path(),
            event_proxy.clone(),
        );
        let scheduler = Scheduler::new(proxy);
        let server_registry =
            ServerRegistry::load(neoism_backend::config::config_dir_path())
                .unwrap_or_else(|error| {
                    tracing::warn!(%error, "failed to load saved server registry");
                    ServerRegistry::load(
                        std::env::temp_dir().join("neoism-server-registry-fallback"),
                    )
                    .expect("fallback server registry path must be readable")
                });
        let (server_health_tx, server_health_rx) = mpsc::channel();
        let (server_switch_tx, server_switch_rx) = mpsc::channel();
        event_loop.listen_device_events(DeviceEvents::Never);

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        event_loop.set_confirm_before_quit(config.confirm_before_quit);

        neoism_notifier::request_authorization();

        Application {
            config,
            event_proxy,
            router,
            bootstrap_daemon: daemon,
            window_sessions: HashMap::new(),
            window_profile_seq: 0,
            home_daemon_endpoint,
            server_registry,
            bootstrap_server_id: initial_server_id,
            server_health: HashMap::new(),
            server_health_inflight: HashSet::new(),
            server_health_tx,
            server_health_rx,
            server_switch_tx,
            server_switch_rx,
            server_switch_inflight: HashSet::new(),
            scheduler,
            app_id,
            initial_open_paths,
            _external_command_listener: external_command_listener,
        }
    }

    fn next_window_profile_id(&mut self) -> String {
        self.window_profile_seq += 1;
        format!("window-{}", self.window_profile_seq)
    }

    fn attach_session_to_window(&mut self, window_id: WindowId) {
        let Some(session) = self.window_sessions.get(&window_id) else {
            return;
        };
        let is_home = session.is_home(self.home_daemon_endpoint.as_deref());
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.window.screen.attach_daemon_client(
                session.connection.handle(),
                session.connection.runtime_handle(),
                session.connection.endpoint().to_string(),
                is_home,
            );
        }
    }

    fn attach_bootstrap_session(&mut self, window_id: WindowId) {
        if self.window_sessions.contains_key(&window_id) {
            self.attach_session_to_window(window_id);
            return;
        }
        let Some(connection) = self.bootstrap_daemon.take() else {
            return;
        };
        let server_id = self.bootstrap_server_id.take();
        let profile_id = self.next_window_profile_id();
        self.window_sessions.insert(
            window_id,
            WindowServerSession::new(profile_id, connection, server_id),
        );
        self.attach_session_to_window(window_id);
        if let Some(session) = self.window_sessions.get(&window_id) {
            session.connection.send(WorkspaceClientMessage::ListWindows);
        }
    }

    fn ensure_local_session(&mut self, window_id: WindowId) {
        if self.window_sessions.contains_key(&window_id) {
            self.attach_session_to_window(window_id);
            return;
        }
        let Some(endpoint) = self.home_daemon_endpoint.clone() else {
            return;
        };
        let connection = match DesktopDaemonConnection::connect_with_token(
            &endpoint,
            None,
            self.event_proxy.clone(),
        ) {
            Ok(connection) => connection,
            Err(error) => {
                tracing::warn!(?window_id, %error, "failed to attach fresh window to Local Server");
                return;
            }
        };
        let profile_id = self.next_window_profile_id();
        self.window_sessions.insert(
            window_id,
            WindowServerSession::new(profile_id, connection, None),
        );
        self.attach_session_to_window(window_id);
        self.send_window_message(window_id, WorkspaceClientMessage::ListWindows);
    }

    fn pump_daemon(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_server_health_results();
        self.drain_server_switch_results();
        let window_ids = self.window_sessions.keys().copied().collect::<Vec<_>>();
        for window_id in window_ids {
            if let Some(session) = self.window_sessions.get_mut(&window_id) {
                session.refresh_status();
            }
            if let (Some(session), Some(route)) = (
                self.window_sessions.get(&window_id),
                self.router.routes.get_mut(&window_id),
            ) {
                let status = match session.status {
                    ServerConnectionStatus::Online => {
                        neoism_ui::panels::ServerIndicatorStatus::Online
                    }
                    ServerConnectionStatus::Connecting
                    | ServerConnectionStatus::Reconnecting => {
                        neoism_ui::panels::ServerIndicatorStatus::Connecting
                    }
                    ServerConnectionStatus::Offline => {
                        neoism_ui::panels::ServerIndicatorStatus::Offline
                    }
                };
                route
                    .window
                    .screen
                    .renderer
                    .top_bar
                    .set_server_status(status);
            }
            let messages = self
                .window_sessions
                .get(&window_id)
                .map(|session| session.connection.drain_messages())
                .unwrap_or_default();
            for message in messages {
                match message {
                    DaemonServerMessage::Workspace { message, .. } => {
                        self.apply_daemon_workspace_message(
                            window_id, event_loop, message,
                        );
                    }
                    DaemonServerMessage::Editor { message, .. } => {
                        self.apply_daemon_editor_message(window_id, message);
                    }
                    DaemonServerMessage::Pty { message, .. } => {
                        self.apply_daemon_pty_message(window_id, message);
                    }
                    DaemonServerMessage::Crdt { message, .. } => {
                        self.apply_daemon_crdt_message(window_id, message);
                    }
                    DaemonServerMessage::Files {
                        request_id,
                        message,
                    } => {
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            if route
                                .window
                                .screen
                                .apply_daemon_files_message(request_id, &message)
                            {
                                route.request_redraw();
                            }
                        }
                    }
                    DaemonServerMessage::Search {
                        request_id,
                        message,
                    } => {
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            if route
                                .window
                                .screen
                                .apply_daemon_search_message(request_id, &message)
                            {
                                route.request_redraw();
                            }
                        }
                    }
                    DaemonServerMessage::Git {
                        request_id,
                        message,
                    } => {
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            if route
                                .window
                                .screen
                                .apply_daemon_git_message(request_id, &message)
                            {
                                route.request_redraw();
                            }
                        }
                    } // Agent HTTP/SSE is rebound separately per window; it is
                      // not a frame variant on the daemon multiplex today.
                }
            }
            // Frames from a PARKED home connection (this window is visiting
            // a guest server): pane-scoped traffic only. PTY output and
            // editor redraws keep the background islands' shells and nvims
            // current instead of freezing until the user switches home.
            // Inventory planes (workspace/files/crdt/git) stay dropped —
            // they describe the HOME daemon and would fight the guest
            // server's state the screen is attached to; the return-home
            // resync re-requests them.
            let parked_messages = self
                .window_sessions
                .get(&window_id)
                .and_then(|session| session.parked_home.as_ref())
                .map(|connection| connection.drain_messages())
                .unwrap_or_default();
            for message in parked_messages {
                match message {
                    DaemonServerMessage::Editor { message, .. } => {
                        self.apply_daemon_editor_message(window_id, message);
                    }
                    DaemonServerMessage::Pty { message, .. } => {
                        self.apply_daemon_pty_message(window_id, message);
                    }
                    _ => {}
                }
            }
            self.process_window_server_requests(window_id);
            self.flush_window_outbound(window_id);
        }
    }

    fn drain_server_health_results(&mut self) {
        while let Ok((server_id, online)) = self.server_health_rx.try_recv() {
            self.server_health_inflight.remove(&server_id);
            let status = if online {
                neoism_ui::panels::ServerIndicatorStatus::Online
            } else {
                neoism_ui::panels::ServerIndicatorStatus::Offline
            };
            self.server_health.insert(server_id.clone(), status);
            for route in self.router.routes.values_mut() {
                if route
                    .window
                    .screen
                    .renderer
                    .command_palette
                    .update_server_status(&server_id, status)
                {
                    route.request_redraw();
                }
            }
        }
    }

    fn send_window_message(
        &self,
        window_id: WindowId,
        message: WorkspaceClientMessage,
    ) -> bool {
        let Some(session) = self.window_sessions.get(&window_id) else {
            return false;
        };
        session.connection.send(message);
        true
    }

    fn process_window_server_requests(&mut self, window_id: WindowId) {
        let (
            server_request,
            add_request,
            edit_request,
            edit_submit,
            remove_request,
            open_manager,
            join_request,
            go_home,
            workspace_subscription,
        ) = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            (
                route.window.screen.take_server_connect(),
                route.window.screen.take_server_add(),
                route.window.screen.take_server_edit(),
                route.window.screen.take_server_edit_submit(),
                route.window.screen.take_server_remove(),
                route.window.screen.take_server_manager_request(),
                route.window.screen.take_peer_workspace_join(),
                route.window.screen.take_daemon_go_home(),
                route.window.screen.take_workspace_subscription(),
            )
        };

        if let Some(workspace_id) = workspace_subscription {
            self.persist_workspace_subscription(window_id, workspace_id);
        }

        if let Some(server_id) = server_request {
            if server_id == "local" {
                if let Some(endpoint) = self.home_daemon_endpoint.clone() {
                    self.switch_window_server(window_id, &endpoint, None, None);
                }
            } else if let Some(server) = self.server_registry.server(&server_id).cloned()
            {
                let token = self.server_registry.token(&server_id).map(str::to_string);
                self.switch_window_server(
                    window_id,
                    &server.endpoint,
                    token,
                    Some(server_id),
                );
            }
        }

        if let Some((address, name, token)) = add_request {
            match self
                .server_registry
                .add(&address, name.as_deref(), token.as_deref())
            {
                Ok(server) => {
                    let token =
                        self.server_registry.token(&server.id).map(str::to_string);
                    self.switch_window_server(
                        window_id,
                        &server.endpoint,
                        token,
                        Some(server.id),
                    );
                }
                Err(error) => tracing::warn!(%error, "failed to add saved server"),
            }
        }

        if let Some(server_id) = edit_request {
            self.open_edit_server_form(window_id, &server_id);
        }

        if let Some((server_id, address, name, token)) = edit_submit {
            match self.server_registry.update(
                &server_id,
                &address,
                name.as_deref(),
                token.as_deref(),
            ) {
                Ok(server) => {
                    let active = self
                        .window_sessions
                        .get(&window_id)
                        .and_then(|session| session.active_server_id.as_deref())
                        == Some(server_id.as_str());
                    if active {
                        let token =
                            self.server_registry.token(&server_id).map(str::to_string);
                        self.switch_window_server(
                            window_id,
                            &server.endpoint,
                            token,
                            Some(server_id),
                        );
                    }
                }
                Err(error) => tracing::warn!(%error, "failed to update saved server"),
            }
        }

        if let Some(server_id) = remove_request {
            let active = self
                .window_sessions
                .get(&window_id)
                .and_then(|session| session.active_server_id.as_deref())
                == Some(server_id.as_str());
            if active {
                if let Some(home) = self.home_daemon_endpoint.clone() {
                    self.switch_window_server(window_id, &home, None, None);
                }
            }
            if let Err(error) = self.server_registry.remove(&server_id) {
                tracing::warn!(%error, "failed to remove saved server");
            }
            self.open_server_manager(window_id);
        }

        if open_manager {
            self.open_server_manager(window_id);
        }

        if let Some((workspace_id, daemon_url)) = join_request {
            if let Some(session) = self.window_sessions.get_mut(&window_id) {
                session.pending_peer_adopt = Some(workspace_id);
            }
            let already_connected = self
                .window_sessions
                .get(&window_id)
                .is_some_and(|session| session.connection.endpoint() == daemon_url);
            if !already_connected {
                self.switch_window_server(window_id, &daemon_url, None, None);
            }
            self.send_window_message(
                window_id,
                WorkspaceClientMessage::RequestHostWorkspaceTree,
            );
        }

        if go_home {
            if let Some(home) = self.home_daemon_endpoint.clone() {
                if let Some(session) = self.window_sessions.get_mut(&window_id) {
                    session.pending_peer_adopt = None;
                }
                self.switch_window_server(window_id, &home, None, None);
                self.send_window_message(
                    window_id,
                    WorkspaceClientMessage::RequestHostWorkspaceTree,
                );
            }
        }
    }

    fn persist_workspace_subscription(
        &mut self,
        window_id: WindowId,
        workspace_id: String,
    ) {
        let Some(session) = self.window_sessions.get(&window_id) else {
            return;
        };
        let profile_id = session.profile_id.clone();
        let server_id = session
            .active_server_id
            .clone()
            .unwrap_or_else(|| "local".to_string());
        let mut subscription = self
            .server_registry
            .workspace_subscription(&profile_id, &server_id);
        if !subscription
            .subscribed_workspace_ids
            .contains(&workspace_id)
        {
            subscription
                .subscribed_workspace_ids
                .push(workspace_id.clone());
        }
        subscription.last_active_workspace_id = Some(workspace_id);
        if let Err(error) = self.server_registry.set_workspace_subscription(
            &profile_id,
            &server_id,
            subscription,
        ) {
            tracing::warn!(%error, "failed to persist workspace subscription");
        }
    }

    fn open_server_manager(&mut self, window_id: WindowId) {
        // The stripe marks where the CURRENT VIEW lives, not merely which
        // connection the window holds: a leftover local island viewed
        // while connected to a guest server still reads as Local.
        let viewing_connected_workspace = self
            .router
            .routes
            .get(&window_id)
            .map(|route| {
                route
                    .window
                    .screen
                    .context_manager
                    .current_adopted_workspace_id()
                    .is_some()
            })
            .unwrap_or(false);
        let active_id = self
            .window_sessions
            .get(&window_id)
            .and_then(|session| session.active_server_id.clone())
            .filter(|_| viewing_connected_workspace);
        let local_active = active_id.is_none();
        let active_status = self
            .window_sessions
            .get(&window_id)
            .map(|session| match session.status {
                ServerConnectionStatus::Online => {
                    neoism_ui::panels::ServerIndicatorStatus::Online
                }
                ServerConnectionStatus::Connecting
                | ServerConnectionStatus::Reconnecting => {
                    neoism_ui::panels::ServerIndicatorStatus::Connecting
                }
                ServerConnectionStatus::Offline => {
                    neoism_ui::panels::ServerIndicatorStatus::Offline
                }
            })
            .unwrap_or(neoism_ui::panels::ServerIndicatorStatus::Unknown);
        let mut entries = vec![neoism_ui::panels::command_palette::PaletteServerEntry {
            id: "local".to_string(),
            name: "Local Server".to_string(),
            address: self
                .home_daemon_endpoint
                .clone()
                .unwrap_or_else(|| "local daemon".to_string()),
            local: true,
            status: if local_active {
                active_status
            } else {
                neoism_ui::panels::ServerIndicatorStatus::Unknown
            },
            active: local_active,
        }];
        entries.extend(self.server_registry.servers().iter().map(|server| {
            let active = active_id.as_deref() == Some(server.id.as_str());
            neoism_ui::panels::command_palette::PaletteServerEntry {
                id: server.id.clone(),
                name: server.name.clone(),
                address: server.endpoint.clone(),
                local: false,
                status: if active {
                    active_status
                } else {
                    self.server_health
                        .get(&server.id)
                        .copied()
                        .unwrap_or(neoism_ui::panels::ServerIndicatorStatus::Unknown)
                },
                active,
            }
        }));
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route
                .window
                .screen
                .renderer
                .command_palette
                .enter_servers_mode(entries);
            route.request_redraw();
        }
        self.probe_saved_servers(active_id.as_deref());
    }

    fn probe_saved_servers(&mut self, active_id: Option<&str>) {
        for server in self.server_registry.servers().to_vec() {
            if active_id == Some(server.id.as_str())
                || !self.server_health_inflight.insert(server.id.clone())
            {
                continue;
            }
            let token = self.server_registry.token(&server.id).map(str::to_string);
            let tx = self.server_health_tx.clone();
            let event_proxy = self.event_proxy.clone();
            std::thread::Builder::new()
                .name(format!("neoism-server-probe-{}", server.id))
                .spawn(move || {
                    let online = DesktopDaemonConnection::connect_with_token(
                        &server.endpoint,
                        token,
                        event_proxy.clone(),
                    )
                    .is_ok();
                    let _ = tx.send((server.id, online));
                    event_proxy.send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                        neoism_window::window::WindowId::dummy()
                    });
                })
                .ok();
        }
    }

    fn open_edit_server_form(&mut self, window_id: WindowId, server_id: &str) {
        let Some(server) = self.server_registry.server(server_id).cloned() else {
            return;
        };
        let token = self
            .server_registry
            .token(server_id)
            .unwrap_or_default()
            .to_string();
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.window.screen.open_edit_server_form(
                server.id,
                server.endpoint,
                server.name,
                token,
            );
            route.request_redraw();
        }
    }

    fn flush_window_outbound(&mut self, window_id: WindowId) {
        let (outbound, outbound_crdt, redraw) = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let outbound = route
                .window
                .screen
                .drain_daemon_pane_layout_requests()
                .collect::<Vec<_>>();
            let mut outbound_crdt = route.window.screen.drain_daemon_presence_messages();
            let (markdown_crdt_messages, markdown_pane_changed) =
                route.window.screen.drain_markdown_crdt_messages();
            outbound_crdt.extend(markdown_crdt_messages);
            let (markdown_disk_messages, markdown_disk_changed) =
                route.window.screen.reload_open_markdown_files_from_disk();
            outbound_crdt.extend(markdown_disk_messages);
            (
                outbound,
                outbound_crdt,
                markdown_pane_changed || markdown_disk_changed,
            )
        };
        if redraw {
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.request_redraw();
            }
        }
        if let Some(session) = self.window_sessions.get(&window_id) {
            for message in outbound {
                session.connection.send(message);
            }
            session.connection.send_crdt_batch(outbound_crdt);
        }
    }

    /// Apply an inbound CRDT message only to the owning window.
    /// windows whose visible state changed.
    fn apply_daemon_crdt_message(
        &mut self,
        window_id: WindowId,
        message: neoism_protocol::crdt::CrdtServerMessage,
    ) {
        if let Some(route) = self.router.routes.get_mut(&window_id) {
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
        source_window_id: WindowId,
        event_loop: &ActiveEventLoop,
        message: WorkspaceServerMessage,
    ) {
        let hosts = match &message {
            WorkspaceServerMessage::HostWorkspaceTree { hosts, .. }
            | WorkspaceServerMessage::HostList { hosts } => hosts.as_slice(),
            _ => &[],
        };
        let workspaces = Self::workspace_summaries_from_message(&message);
        let rehome_target = self
            .window_sessions
            .get_mut(&source_window_id)
            .and_then(|session| session.observe_rehome(hosts, workspaces));
        // Wave 4D: before fanning the message out to the chrome, watch for
        // the active workspace's home host flipping to a different machine.
        // If it moved to a host that advertises a dialable `daemon_url`, we
        // re-point this desktop's daemon connection there so the workspace
        // keeps showing at its new home. The daemon stays the source of
        // truth — we only re-dial which daemon we talk to.
        // MULTI-USER GUARD: window summaries only drive native window
        // bind/materialisation when we're dialled into our HOME
        // daemon. A peer's daemon (joined workspace) ships the HOST's
        // window inventory — materialising those spawned phantom OS
        // windows on the guest, and binding could hijack an existing
        // local window's identity.
        let windows_from_home = self
            .window_sessions
            .get(&source_window_id)
            .is_some_and(|session| session.is_home(self.home_daemon_endpoint.as_deref()));
        match &message {
            WorkspaceServerMessage::WindowList { windows } if windows_from_home => {
                for window in windows {
                    self.apply_daemon_window_summary(
                        source_window_id,
                        event_loop,
                        window,
                    );
                }
            }
            WorkspaceServerMessage::WindowOpened { window }
            | WorkspaceServerMessage::WindowChanged { window }
                if windows_from_home =>
            {
                self.apply_daemon_window_summary(source_window_id, event_loop, window);
            }
            WorkspaceServerMessage::WindowClosed { window_id } if windows_from_home => {
                if let Some(native_id) = self.router.unbind_daemon_window(window_id) {
                    self.router.routes.remove(&native_id);
                    crate::app::freeze_watchdog::unregister_window(native_id);
                }
            }
            _ => {}
        }

        if let Some(route) = self.router.routes.get_mut(&source_window_id) {
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

        let summaries = Self::workspace_summaries_from_message(&message);
        if !summaries.is_empty() {
            let subscription =
                self.window_sessions.get(&source_window_id).map(|session| {
                    let server_id =
                        session.active_server_id.as_deref().unwrap_or("local");
                    self.server_registry
                        .workspace_subscription(&session.profile_id, server_id)
                });
            if let Some(mut subscription) = subscription {
                subscription
                    .subscribed_workspace_ids
                    .retain(|workspace_id| {
                        summaries
                            .iter()
                            .any(|workspace| workspace.id == *workspace_id)
                    });
                if subscription.last_active_workspace_id.as_ref().is_some_and(
                    |workspace_id| {
                        !summaries
                            .iter()
                            .any(|workspace| workspace.id == *workspace_id)
                    },
                ) {
                    subscription.last_active_workspace_id = None;
                }
                let restored_any = !subscription.subscribed_workspace_ids.is_empty()
                    || subscription.last_active_workspace_id.is_some();
                if let Some(route) = self.router.routes.get_mut(&source_window_id) {
                    route.window.screen.restore_subscribed_daemon_workspaces(
                        &subscription.subscribed_workspace_ids,
                        subscription.last_active_workspace_id.as_deref(),
                    );
                    route.request_redraw();
                }
                // A fresh server switch with no subscription to restore
                // must still land the window IN a workspace on the new
                // server — otherwise the previous server's panes stay on
                // screen and file clicks reuse its editor. Adopt the most
                // recent workspace from the arriving tree (the daemon
                // sorts them most-recently-active first).
                let needs_initial = self
                    .window_sessions
                    .get_mut(&source_window_id)
                    .map(|session| {
                        std::mem::take(&mut session.needs_initial_workspace_adopt)
                    })
                    .unwrap_or(false);
                if needs_initial && !restored_any {
                    if let Some(workspace_id) =
                        summaries.first().map(|workspace| workspace.id.clone())
                    {
                        if let Some(route) = self.router.routes.get_mut(&source_window_id)
                        {
                            route
                                .window
                                .screen
                                .open_or_adopt_daemon_workspace(workspace_id);
                            route.request_redraw();
                        }
                    }
                }
            }
        }

        // Peer-workspace join, step 2: the redialled daemon's tree just
        // landed in the manager caches above. If it carries the picked
        // workspace, re-enter the open/adopt path — this time
        // `peer_workspace_daemon_url` resolves None (the workspace is
        // now on the linked daemon) and the normal adopt attaches its
        // live sessions over the new connection.
        let pending_peer_adopt = self
            .window_sessions
            .get(&source_window_id)
            .and_then(|session| session.pending_peer_adopt.clone());
        if let Some(workspace_id) = pending_peer_adopt {
            let carried = Self::workspace_summaries_from_message(&message)
                .iter()
                .any(|workspace| workspace.id == workspace_id);
            if carried {
                if let Some(session) = self.window_sessions.get_mut(&source_window_id) {
                    session.pending_peer_adopt = None;
                }
                if let Some(route) = self.router.routes.get_mut(&source_window_id) {
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
        if let Some(daemon_url) = rehome_target {
            self.switch_window_server(source_window_id, &daemon_url, None, None);
        }
    }

    fn switch_window_server(
        &mut self,
        window_id: WindowId,
        daemon_url: &str,
        token: Option<String>,
        server_id: Option<String>,
    ) {
        if self
            .router
            .routes
            .get_mut(&window_id)
            .is_some_and(|route| route.window.screen.has_unsaved_server_buffers())
        {
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.renderer.notifications.push(
                    "Save changes before switching servers",
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                route.request_redraw();
            }
            return;
        }
        if self.window_sessions.get(&window_id).is_some_and(|session| {
            session.connection.endpoint() == daemon_url
                && token.is_none()
                && server_id == session.active_server_id
        }) {
            return;
        }

        // Returning HOME reuses the parked connection when it is still
        // open — same daemon-side nvim namespace, so the local editors
        // survive the round trip — instead of dialling a fresh one.
        if self.home_daemon_endpoint.as_deref() == Some(daemon_url) {
            let parked = self
                .window_sessions
                .get_mut(&window_id)
                .and_then(|session| session.parked_home.take());
            if let Some(connection) = parked {
                if matches!(
                    connection.status(),
                    crate::daemon_client::DaemonClientStatus::Open
                ) {
                    self.complete_server_switch(window_id, connection, server_id);
                    return;
                }
                // Stale (daemon restarted while we were away) — dial.
            }
        }
        // Dial off-thread: the handshake blocks up to its timeout, and a
        // dead endpoint would freeze the UI for the whole wait. The pump
        // completes the swap in `drain_server_switch_results`.
        if !self.server_switch_inflight.insert(window_id) {
            return;
        }
        if let Some(id) = server_id.as_deref() {
            let status = neoism_ui::panels::ServerIndicatorStatus::Connecting;
            self.server_health.insert(id.to_string(), status);
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .renderer
                    .command_palette
                    .update_server_status(id, status);
                route.request_redraw();
            }
        }
        let tx = self.server_switch_tx.clone();
        let event_proxy = self.event_proxy.clone();
        let daemon_url = daemon_url.to_string();
        std::thread::Builder::new()
            .name(format!("neoism-server-switch-{window_id:?}"))
            .spawn(move || {
                let result = DesktopDaemonConnection::connect_with_token(
                    &daemon_url,
                    token,
                    event_proxy.clone(),
                )
                .map_err(|error| error.to_string());
                let _ = tx.send(PendingServerSwitch {
                    window_id,
                    daemon_url,
                    server_id,
                    result,
                });
                event_proxy.send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                    neoism_window::window::WindowId::dummy()
                });
            })
            .ok();
    }

    fn drain_server_switch_results(&mut self) {
        while let Ok(pending) = self.server_switch_rx.try_recv() {
            let PendingServerSwitch {
                window_id,
                daemon_url,
                server_id,
                result,
            } = pending;
            self.server_switch_inflight.remove(&window_id);
            let connection = match result {
                Ok(connection) => connection,
                Err(error) => {
                    tracing::warn!(
                        target: "neoism::desktop_daemon",
                        window = ?window_id,
                        daemon = %daemon_url,
                        %error,
                        "failed to switch window server; keeping current connection"
                    );
                    if let Some(id) = server_id.as_deref() {
                        let status = neoism_ui::panels::ServerIndicatorStatus::Offline;
                        self.server_health.insert(id.to_string(), status);
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            route
                                .window
                                .screen
                                .renderer
                                .command_palette
                                .update_server_status(id, status);
                        }
                    }
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        route.window.screen.renderer.notifications.push(
                            format!(
                                "Could not connect to {daemon_url}: {error}. Kept the current server."
                            ),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        );
                        route.request_redraw();
                    }
                    continue;
                }
            };
            self.complete_server_switch(window_id, connection, server_id);
        }
    }

    /// Final phase of a server switch, shared by the async-dial path and
    /// the parked-home fast path: swap the window's session (parking the
    /// outgoing HOME connection so its daemon-side nvim namespace stays
    /// alive), reset server-owned chrome, attach, and request the new
    /// server's inventory.
    fn complete_server_switch(
        &mut self,
        window_id: WindowId,
        connection: DesktopDaemonConnection,
        server_id: Option<String>,
    ) {
        // The window may have closed while the dial was in flight.
        if !self.router.routes.contains_key(&window_id) {
            return;
        }
        if let Some(id) = server_id.as_deref() {
            let status = neoism_ui::panels::ServerIndicatorStatus::Online;
            self.server_health.insert(id.to_string(), status);
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .renderer
                    .command_palette
                    .update_server_status(id, status);
            }
        }
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.window.screen.reset_server_owned_state();
        }
        let outgoing = self.window_sessions.remove(&window_id);
        let home_endpoint = self.home_daemon_endpoint.clone();
        let (profile_id, parked_home) = match outgoing {
            Some(old) => {
                let profile_id = old.profile_id.clone();
                // Keep the home daemon's websocket open while away — the
                // daemon reaps the nvim sessions of a closed connection's
                // namespace, which executed every local editor on switch.
                let parked = if old.is_home(home_endpoint.as_deref()) {
                    Some(old.connection)
                } else {
                    old.parked_home
                };
                (profile_id, parked)
            }
            None => (self.next_window_profile_id(), None),
        };
        let mut session = WindowServerSession::new(profile_id, connection, server_id);
        session.needs_initial_workspace_adopt = true;
        session.parked_home = parked_home;
        self.window_sessions.insert(window_id, session);
        self.attach_session_to_window(window_id);
        self.send_window_message(window_id, WorkspaceClientMessage::ListWindows);
        self.send_window_message(
            window_id,
            WorkspaceClientMessage::RequestHostWorkspaceTree,
        );
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            // The reset above cleared the visible tree's entries, but
            // the root pathname often survives the switch unchanged,
            // so every "already there" guard skips repopulation and
            // the tree stays blank until closed and reopened. Force
            // one sync.
            route
                .window
                .screen
                .sync_file_tree_root_for_current_workspace();
            route.request_redraw();
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
        window_id: WindowId,
        message: neoism_protocol::editor::EditorServerMessage,
    ) {
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            if route
                .window
                .screen
                .apply_daemon_editor_message(message.clone())
            {
                route.request_redraw();
            }
        }
    }

    fn apply_daemon_pty_message(
        &mut self,
        window_id: WindowId,
        message: neoism_protocol::pty::ServerMessage,
    ) {
        if let Some(route) = self.router.routes.get_mut(&window_id) {
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
        source_window_id: WindowId,
        event_loop: &ActiveEventLoop,
        window: &WorkspaceWindowSummary,
    ) {
        // Belt-and-braces with the gate in
        // `apply_daemon_workspace_message`: never bind/materialise
        // native windows from a PEER daemon's window inventory.
        if !self
            .window_sessions
            .get(&source_window_id)
            .is_some_and(|session| session.is_home(self.home_daemon_endpoint.as_deref()))
        {
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
            self.ensure_local_session(native_id);
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

        let window_id = self.router.create_window(
            event_loop,
            self.event_proxy.clone(),
            &self.config,
            None,
            self.app_id.as_deref(),
        );
        self.attach_bootstrap_session(window_id);
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
                if let Some(daemon_window_id) =
                    self.router.daemon_window_for_native(window_id)
                {
                    self.send_window_message(
                        window_id,
                        WorkspaceClientMessage::RequestCloseWindow {
                            window_id: daemon_window_id.to_string(),
                        },
                    );
                }
                self.router.unbind_native_window(window_id);
                self.router.routes.remove(&window_id);
                self.window_sessions.remove(&window_id);
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
                let window_id = self.router.create_window(
                    active_event_loop,
                    self.event_proxy.clone(),
                    &self.config,
                    Some(url),
                    self.app_id.as_deref(),
                );
                self.attach_bootstrap_session(window_id);
                self.ensure_local_session(window_id);
                self.send_window_message(
                    window_id,
                    WorkspaceClientMessage::RequestOpenWindow {
                        workspace_id: None,
                        title: None,
                    },
                );
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
                let window_id = self.router.create_native_tab(
                    active_event_loop,
                    self.event_proxy.clone(),
                    &self.config,
                    tab_id.as_deref(),
                    Some(url),
                );
                self.attach_bootstrap_session(window_id);
                self.ensure_local_session(window_id);
                self.send_window_message(
                    window_id,
                    WorkspaceClientMessage::RequestOpenNativeTab {
                        workspace_id: None,
                        parent_window_id,
                        title: None,
                    },
                );
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
            if let Some(window_id) = self.router.get_focused_route() {
                self.send_window_message(
                    window_id,
                    WorkspaceClientMessage::RequestOpenConfigEditor {
                        workspace_id: None,
                    },
                );
            }
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
