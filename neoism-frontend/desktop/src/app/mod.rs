use crate::app::bell::{play_audio_bell, send_desktop_notification};
use crate::app::daemon_pump::DesktopDaemonConnection;
use crate::app::scheduler::{Scheduler, TimerId, Topic};
use crate::app::window_event::touch::on_touch;
use crate::bridges::utils::apply_theme_to_config;
use crate::daemon_client::{DaemonServerMessage, PtyFailureClass};
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
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::server_registry::ServerRegistry;

/// Result of an off-thread server-switch dial, completed by the pump.
struct PendingServerSwitch {
    window_id: WindowId,
    daemon_url: String,
    server_id: Option<String>,
    result: Result<DesktopDaemonConnection, String>,
}

/// Result of the SSH bootstrap/tunnel phase. Daemon websocket dialing stays
/// in the existing server-switch pipeline; keeping the phases separate means
/// neither SSH authentication nor daemon startup can block the UI thread.
struct PendingSshAttach {
    window_id: WindowId,
    target: String,
    result: Result<crate::ssh_hosts::DaemonAttach, String>,
}

#[derive(Clone)]
struct PendingSshWorkspace {
    daemon_url: String,
    workspace_id: String,
    title: String,
}

struct PendingLuaLspRequest {
    window_id: WindowId,
    operation: neoism_lua::LuaLspOperation,
    target: neoism_lua::LuaLspTarget,
}

struct LuaLspActionLease {
    window_id: WindowId,
    target: neoism_lua::LuaLspTarget,
    created_at: Instant,
    action: crate::screen::bridges::code::lsp::LuaLspRetainedCodeAction,
}

#[derive(Clone)]
struct LuaPluginTimerLease {
    owner: neoism_lua::PluginOwner,
    due: Instant,
    interval: Option<Duration>,
    command: String,
    arguments: serde_json::Value,
}

fn advance_lua_timer_deadline(due: Instant, interval: Duration, now: Instant) -> Instant {
    let overdue = now.saturating_duration_since(due);
    let skipped = (overdue.as_millis() / interval.as_millis().max(1)) as u32 + 1;
    due + interval.saturating_mul(skipped)
}

#[cfg(test)]
mod lua_timer_tests {
    use super::*;

    #[test]
    fn repeating_deadline_coalesces_missed_ticks_without_cadence_drift() {
        let due = Instant::now();
        let interval = Duration::from_millis(20);
        let now = due + Duration::from_millis(95);
        assert_eq!(
            advance_lua_timer_deadline(due, interval, now),
            due + Duration::from_millis(100)
        );
    }

    #[test]
    fn lua_state_polling_is_reserved_for_state_event_subscribers() {
        assert!(lua_autocmd_needs_state_poll("DocumentChanged"));
        assert!(lua_autocmd_needs_state_poll("*"));
        assert!(!lua_autocmd_needs_state_poll("Startup"));
        assert!(!lua_autocmd_needs_state_poll("Command"));
        assert!(!lua_autocmd_needs_state_poll("AsyncResult"));
    }

    #[test]
    fn composer_changes_publish_immediately_only_for_agent_subscribers() {
        assert!(lua_autocmd_needs_urgent_composer_publish("AgentChanged"));
        assert!(lua_autocmd_needs_urgent_composer_publish("*"));
        assert!(!lua_autocmd_needs_urgent_composer_publish(
            "DocumentChanged"
        ));
    }
}

#[derive(Clone)]
struct LuaPluginStickyAnchor {
    window_id: WindowId,
    buffer_id: String,
    anchor: neoism_ui::editor::crdt::CrdtStickyAnchor,
}

#[derive(Clone)]
struct LuaPluginAnchorLease {
    owner: neoism_lua::PluginOwner,
    namespace: neoism_lua::PluginNamespaceHandle,
    resource: neoism_lua::PluginResourceId,
    sticky: Option<LuaPluginStickyAnchor>,
    window_id: WindowId,
}

#[derive(Clone)]
struct LuaEditorOptionBaseline {
    window_id: WindowId,
    route_id: usize,
    wrap: bool,
    input_mode: neoism_ui::editor::code::CodeInputMode,
    indent: neoism_ui::editor::code::CodeIndent,
}

#[derive(Default)]
struct LuaEditorResources {
    registry: neoism_lua::PluginResourceRegistry,
    namespaces:
        HashMap<String, (neoism_lua::PluginOwner, neoism_lua::PluginNamespaceHandle)>,
    anchors: HashMap<String, LuaPluginAnchorLease>,
    decorations: HashMap<
        String,
        (
            neoism_lua::PluginOwner,
            neoism_lua::PluginNamespaceHandle,
            neoism_lua::PluginResourceId,
            neoism_lua::DecorationLayer,
        ),
    >,
    published_targets: HashMap<neoism_lua::DocumentHandle, (WindowId, usize)>,
    document_text: HashMap<neoism_lua::DocumentHandle, String>,
}

#[derive(Clone)]
struct LuaResultList {
    owner: neoism_lua::PluginOwner,
    window_id: WindowId,
    title: String,
    kind: String,
    entries: Vec<neoism_lua::PluginResultEntry>,
}

fn ssh_server_id(workspace_id: &str) -> String {
    format!("ssh:{workspace_id}")
}

const NOTEBOOK_STATUS_TICK_MS: u64 = 500;
const FRAME_WATCHDOG_NOTE_INTERVAL: Duration = Duration::from_secs(1);
const LUA_STATE_PUBLISH_INTERVAL: Duration = Duration::from_millis(100);

fn commit_validated_mashup_candidate<T, E>(
    candidate: Result<T, E>,
    commit: impl FnOnce(T) -> Result<(), E>,
) -> Result<(), E> {
    commit(candidate?)
}
const LUA_LSP_PENDING_PER_OWNER: usize = 64;
const LUA_LSP_PENDING_GLOBAL: usize = 512;
const LUA_LSP_ACTIONS_PER_OWNER: usize = 256;
const LUA_LSP_ACTIONS_GLOBAL: usize = 2048;
const LUA_LSP_ACTION_TTL: Duration = Duration::from_secs(5 * 60);

fn lua_autocmd_needs_state_poll(event: &str) -> bool {
    matches!(
        event,
        "*" | "BufferChanged"
            | "WorkspaceChanged"
            | "TabChanged"
            | "PanelChanged"
            | "FileTreeChanged"
            | "NotesChanged"
            | "AgentChanged"
            | "TerminalChanged"
            | "GitChanged"
            | "ThemeChanged"
            | "PluginsChanged"
            | "ConfigChanged"
            | "DocumentOpened"
            | "DocumentClosed"
            | "DocumentFocused"
            | "DocumentChanged"
            | "SelectionChanged"
            | "PaneFocused"
            | "PaneChanged"
    )
}

fn lua_autocmd_needs_urgent_composer_publish(event: &str) -> bool {
    matches!(event, "*" | "AgentChanged")
}

#[cfg(target_os = "linux")]
pub(crate) mod agent_tray;
pub mod bell;
pub mod daemon_pump;
pub mod freeze_watchdog;
pub mod ime;
pub mod messenger;
pub mod scheduler;
pub mod user_event_dispatch;
pub mod window_event;
pub mod window_server_session;

use window_server_session::{ServerConnectionStatus, WindowServerSession};

pub struct Application<'a> {
    #[cfg(target_os = "linux")]
    agent_tray: Option<agent_tray::Service>,
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
    ssh_attach_tx: mpsc::Sender<PendingSshAttach>,
    ssh_attach_rx: mpsc::Receiver<PendingSshAttach>,
    ssh_attach_inflight: HashSet<WindowId>,
    /// Live tunnel guards keyed by their normalized daemon endpoint. They
    /// outlive workspace switches so returning to an SSH workspace can reuse
    /// its parked daemon connection and remote PTYs immediately.
    ssh_attaches: HashMap<String, crate::ssh_hosts::DaemonAttach>,
    /// Workspace to create/adopt once the tunnel's websocket server switch
    /// completes.
    pending_ssh_workspaces: HashMap<WindowId, PendingSshWorkspace>,
    scheduler: Scheduler,
    app_id: Option<String>,
    initial_open_paths: Vec<PathBuf>,
    _external_command_listener: Option<crate::ipc::ExternalCommandListener>,
    update_check_started: bool,
    pending_update_version: Option<String>,
    lua_runtime: Option<neoism_lua::LuaRuntime>,
    lua_host: Arc<neoism_lua::QueuedHost>,
    lua_published: HashMap<String, serde_json::Value>,
    lua_plugins: crate::plugin_manager::LuaPluginManager,
    lua_plugin_jobs: crate::lua_plugin_jobs::LuaPluginJobs,
    lua_lsp_pending: HashMap<(neoism_lua::PluginOwner, String), PendingLuaLspRequest>,
    lua_lsp_actions:
        HashMap<(neoism_lua::PluginOwner, String, String), LuaLspActionLease>,
    lua_command_completions:
        HashMap<(neoism_lua::PluginOwner, String), neoism_lua::PluginCommandCompletion>,
    lua_active_owners: HashSet<neoism_lua::PluginOwner>,
    lua_editor_option_baselines: HashMap<String, LuaEditorOptionBaseline>,
    lua_editor_resources: LuaEditorResources,
    lua_timers: Option<HashMap<String, LuaPluginTimerLease>>,
    lua_async: Option<crate::lua_async::LuaAsyncCoordinator>,
    lua_dap: Option<crate::lua_dap::LuaDap>,
    lua_jobs: Option<crate::lua_jobs::LuaJobs>,
    lua_ptys: Option<crate::lua_ptys::LuaPtys>,
    lua_prompts: Option<HashMap<(WindowId, String), neoism_lua::PluginOwner>>,
    lua_progress: Option<HashMap<String, (neoism_lua::PluginOwner, WindowId)>>,
    lua_result_lists: Option<HashMap<String, LuaResultList>>,
    lua_watchers: Option<crate::lua_watchers::LuaWatchers>,
    lua_state_publish_deadline: Instant,
    lua_composer_text: String,
    lua_composer_revision: u64,
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
        mut lua_runtime: Option<neoism_lua::LuaRuntime>,
        lua_host: Arc<neoism_lua::QueuedHost>,
    ) -> Application<'app> {
        // SAFETY: Since this takes a pointer to the winit event loop, it MUST be dropped first,
        // which is done in `exiting`.
        let clipboard =
            unsafe { Clipboard::new(event_loop.display_handle().unwrap().as_raw()) };

        let config_dir = neoism_backend::config::config_dir_path();
        crate::mashup::seed_first_party_plugins();
        // Eager plugins may make capability-checked read-only decisions during
        // initialization. Seed the same typed config snapshot that the normal
        // state publisher will subsequently maintain.
        lua_host.publish(
            "config",
            serde_json::to_value(&config).unwrap_or(serde_json::Value::Null),
        );
        let (mut lua_plugins, lua_plugin_error) =
            match crate::plugin_manager::resolve_mashup_selection(&config) {
                Ok(selection) => {
                    crate::plugin_manager::LuaPluginManager::discover_for_startup(
                        &config_dir,
                        lua_host.clone(),
                        &config.plugins,
                        selection.as_ref(),
                    )
                }
                Err(error) => (
                    crate::plugin_manager::LuaPluginManager::empty(
                        &config_dir,
                        lua_host.clone(),
                    ),
                    Some(error),
                ),
            };
        if let Some(error) = lua_plugin_error {
            tracing::warn!(%error, "Lua plugin activation failed; retaining plugin diagnostics and user init");
        }
        let plugin_snapshot = Arc::new(crate::plugin_manager::overlay_snapshot(
            lua_plugins.snapshot(),
            lua_runtime.as_ref().map(neoism_lua::LuaRuntime::snapshot),
        ));
        let mut router = Router::new(
            crate::mashup::fonts_with_markdown_family(
                config.appearance.fonts.to_owned(),
                config.appearance.look.markdown.font_family.as_deref(),
            ),
            clipboard,
            plugin_snapshot,
        );
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
        let (ssh_attach_tx, ssh_attach_rx) = mpsc::channel();
        event_loop.listen_device_events(DeviceEvents::Never);

        #[cfg(any(target_os = "macos", target_os = "windows"))]
        event_loop.set_confirm_before_quit(config.ui.confirm_before_quit);

        neoism_notifier::request_authorization();

        if let Some(runtime) = lua_runtime.as_mut() {
            if let Err(error) = runtime.emit(neoism_lua::PluginEvent {
                name: "Startup".into(),
                payload: serde_json::Value::Null,
                scope: neoism_lua::ExecutionScope::Local,
                origin: Some("neoism".into()),
            }) {
                tracing::warn!(%error, "Lua Startup autocmd failed");
            }
            router.set_plugin_snapshot(Arc::new(runtime.snapshot().clone()));
        }
        for failure in lua_plugins.emit(neoism_lua::PluginEvent {
            name: "Startup".into(),
            payload: serde_json::Value::Null,
            scope: neoism_lua::ExecutionScope::Local,
            origin: Some("neoism".into()),
        }) {
            tracing::warn!(plugin = %failure.plugin_id, error = %failure.message, "Lua plugin Startup failed");
        }
        router.set_plugin_snapshot(Arc::new(crate::plugin_manager::overlay_snapshot(
            lua_plugins.snapshot(),
            lua_runtime.as_ref().map(neoism_lua::LuaRuntime::snapshot),
        )));
        let mut lua_active_owners =
            lua_plugins.active_owners().cloned().collect::<HashSet<_>>();
        if let Some(runtime) = &lua_runtime {
            lua_active_owners.insert(runtime.owner().clone());
        }

        Application {
            #[cfg(target_os = "linux")]
            agent_tray: if config.ui.agent_tray {
                agent_tray::Service::start()
                    .map_err(|error| tracing::warn!("Agent tray unavailable: {error}"))
                    .ok()
            } else {
                None
            },
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
            ssh_attach_tx,
            ssh_attach_rx,
            ssh_attach_inflight: HashSet::new(),
            ssh_attaches: HashMap::new(),
            pending_ssh_workspaces: HashMap::new(),
            scheduler,
            app_id,
            initial_open_paths,
            _external_command_listener: external_command_listener,
            update_check_started: false,
            pending_update_version: None,
            lua_runtime,
            lua_host,
            lua_published: HashMap::new(),
            lua_plugins,
            lua_plugin_jobs: Default::default(),
            lua_lsp_pending: HashMap::new(),
            lua_lsp_actions: HashMap::new(),
            lua_command_completions: HashMap::new(),
            lua_active_owners,
            lua_editor_option_baselines: HashMap::new(),
            lua_editor_resources: LuaEditorResources::default(),
            lua_timers: None,
            lua_async: None,
            lua_dap: None,
            lua_jobs: None,
            lua_ptys: None,
            lua_prompts: None,
            lua_progress: None,
            lua_result_lists: None,
            lua_watchers: None,
            lua_state_publish_deadline: Instant::now(),
            lua_composer_text: String::new(),
            lua_composer_revision: 0,
        }
    }

    fn present_pending_update(&mut self, window_id: WindowId) {
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        if route.window.screen.renderer.modal.is_active() {
            return;
        }
        let Some(version) = self.pending_update_version.take() else {
            return;
        };
        use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
        route.window.screen.renderer.modal.open(ModalSpec {
            title: format!("Neoism {version} is available"),
            body: format!(
                "## A fresh build is ready\n\n- Latest fixes and performance improvements\n- Installs through Neoism's platform updater\n- Your workspace stays available when you reopen\n\nInstallation: `{}`",
                std::env::current_exe().map(|path| path.display().to_string()).unwrap_or_else(|_| "current Neoism installation".to_string())
            ),
            meta: format!("Installed: v{}", env!("CARGO_PKG_VERSION")),
            input: None,
            buttons: vec![
                ModalButton::new("Update", "Enter", ModalAction::UpdateNeoism { version: version.clone() }),
                ModalButton::new("Not now", "Esc", ModalAction::Close),
            ],
            busy: false,
            blocking: true,
        });
        route.request_overlay_redraw();
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
                session.connection.token().map(str::to_string),
                is_home,
            );
        }
    }

    pub(in crate::app) fn recycle_stale_window_connection(&self, window_id: WindowId) {
        if let Some(session) = self.window_sessions.get(&window_id) {
            session
                .connection
                .recycle_if_stale(std::time::Duration::from_secs(25));
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
        let perf_start = tracing::enabled!(target: "neoism::frame_work", tracing::Level::DEBUG)
            .then(Instant::now);
        self.pump_lua();
        let lua_us = perf_start
            .map(|start| start.elapsed().as_micros())
            .unwrap_or(0);
        self.drain_server_health_results();
        self.drain_ssh_attach_results();
        self.drain_server_switch_results();
        let window_ids = self.window_sessions.keys().copied().collect::<Vec<_>>();
        for window_id in window_ids {
            let window_start = perf_start.map(|_| Instant::now());
            if let Some(session) = self.window_sessions.get_mut(&window_id) {
                session.refresh_status();
            }
            // PTY subscriptions are websocket-local. A server switch can
            // leave the window session and ContextManager pointing at two
            // different connections whose normalized endpoint strings are
            // identical. Commands then run through one socket while output is
            // drained from the other until a workspace round-trip reattaches
            // every route. Restore the connection-object invariant before
            // accepting messages or allowing more terminal input.
            let active_connection_key = self
                .window_sessions
                .get(&window_id)
                .map(|session| session.connection.connection_key());
            let linked_connection_key =
                self.router.routes.get(&window_id).and_then(|route| {
                    route.window.screen.context_manager.daemon_connection_key()
                });
            if active_connection_key.is_some()
                && active_connection_key != linked_connection_key
            {
                self.attach_session_to_window(window_id);
            }
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                if let Some((handle, _runtime)) = route
                    .window
                    .screen
                    .context_manager
                    .daemon_link_handle_and_runtime()
                {
                    let mut handle = handle;
                    if let Some(open) = handle.take_editor_connection_change() {
                        let generation = handle.generation();
                        if open {
                            route
                                .window
                                .screen
                                .context_manager
                                .resync_after_daemon_reconnect(generation);
                        } else {
                            route
                                .window
                                .screen
                                .context_manager
                                .begin_daemon_generation_gate(generation.max(1));
                        }
                    }
                }
            }
            self.queue_dead_ssh_reconnect(window_id);
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
            // Capture the source endpoint BEFORE processing workspace events:
            // an earlier event in this batch can switch the active connection.
            let drain_start = perf_start.map(|_| Instant::now());
            let (endpoint, connection_key, messages, parked_editors) = self
                .window_sessions
                .get(&window_id)
                .map(|session| {
                    (
                        session.connection.endpoint().to_string(),
                        session.connection.connection_key(),
                        session.connection.drain_messages(),
                        session
                            .parked_connections
                            .values()
                            .flat_map(|connection| {
                                let endpoint = connection.endpoint().to_string();
                                connection
                                    .drain_editor_messages()
                                    .into_iter()
                                    .map(move |message| (endpoint.clone(), message))
                            })
                            .collect::<Vec<_>>(),
                    )
                })
                .unwrap_or_default();
            let drain_us = drain_start
                .map(|start| start.elapsed().as_micros())
                .unwrap_or(0);
            let message_count = messages.len();
            let parked_editor_count = parked_editors.len();
            for (endpoint, message) in parked_editors {
                if let DaemonServerMessage::Editor {
                    request_id,
                    message,
                } = message
                {
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if route.window.screen.apply_remote_code_lsp_message(
                            &endpoint, request_id, &message,
                        ) {
                            route.request_redraw();
                        }
                    }
                }
            }
            for message in messages {
                match message {
                    DaemonServerMessage::Workspace { message, .. } => {
                        self.apply_daemon_workspace_message(
                            window_id, event_loop, message,
                        );
                    }
                    // Native guest code panes reuse the editor envelope for
                    // host-owned LSP snapshots/diagnostics. Grid/nvim
                    // messages remain harmless no-ops in the screen bridge.
                    DaemonServerMessage::Editor {
                        request_id,
                        message,
                    } => {
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            if route.window.screen.apply_remote_code_lsp_message(
                                &endpoint, request_id, &message,
                            ) {
                                route.request_redraw();
                            }
                        }
                    }
                    DaemonServerMessage::PtyFailure {
                        request_id,
                        session_id,
                        message,
                        class,
                        operation,
                    } => {
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            if route.window.screen.context_manager.daemon_endpoint()
                                != Some(endpoint.as_str())
                            {
                                tracing::warn!(
                                    target: "neoism::remote_pty",
                                    source_endpoint = %endpoint,
                                    active_endpoint = ?route.window.screen.context_manager.daemon_endpoint(),
                                    request_id,
                                    ?session_id,
                                    "ignoring PTY failure from a daemon that was parked during this batch"
                                );
                                continue;
                            }
                            if matches!(
                                class,
                                PtyFailureClass::Transport
                                    | PtyFailureClass::NotDelivered
                            ) {
                                let interrupted = route
                                    .window
                                    .screen
                                    .context_manager
                                    .apply_remote_pty_failure(
                                        request_id,
                                        session_id.as_deref(),
                                        &message,
                                        class,
                                        operation,
                                    );
                                if interrupted {
                                    route.window.screen.renderer.notifications.push(
                                        if class == PtyFailureClass::NotDelivered {
                                            format!("Command not sent: {message}. Wait for the terminal to reconnect, then retry.")
                                        } else {
                                            format!("Remote command interrupted: {message}. Delivery/execution is unknown; nothing was replayed.")
                                        },
                                        neoism_ui::panels::notifications::NotificationLevel::Error,
                                    );
                                }
                                route.request_redraw();
                            } else if route
                                .window
                                .screen
                                .context_manager
                                .apply_remote_pty_failure(
                                    request_id,
                                    session_id.as_deref(),
                                    &message,
                                    class,
                                    operation,
                                )
                            {
                                route.window.screen.renderer.notifications.push(
                                    format!("Remote terminal detached: {message}. Nothing was replayed. Reopen/reattach the terminal and check its state before retrying a command."),
                                    neoism_ui::panels::notifications::NotificationLevel::Error,
                                );
                                route.request_redraw();
                            }
                        }
                    }
                    DaemonServerMessage::Pty {
                        request_id,
                        message,
                    } => {
                        self.apply_daemon_pty_message(
                            window_id,
                            &endpoint,
                            connection_key,
                            request_id,
                            message,
                        );
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
            // Parked connections keep receiving into their own queues. Do not
            // feed those frames through the active server's single transport
            // cache: session ids are server-local and could collide. When a
            // workspace activates its server, route/session bindings are
            // rehydrated first and that connection's queued frames are then
            // drained normally.
            self.queue_dead_ssh_reconnect(window_id);
            self.process_window_server_requests(window_id);
            self.flush_window_outbound(window_id);
            if let Some(start) = window_start {
                let elapsed_us = start.elapsed().as_micros();
                if elapsed_us >= 1_000 {
                    tracing::debug!(
                        target: "neoism::frame_work",
                        ?window_id, elapsed_us, drain_us, message_count, parked_editor_count,
                        "slow window daemon pump outside render"
                    );
                }
            }
        }
        if let Some(start) = perf_start {
            let elapsed_us = start.elapsed().as_micros();
            if elapsed_us >= 1_000 {
                tracing::debug!(
                    target: "neoism::frame_work",
                    elapsed_us, lua_us,
                    windows = self.window_sessions.len(),
                    "slow application pump outside render"
                );
            }
        }
    }

    fn pump_lua(&mut self) {
        self.pump_mashup_pack_requests();
        self.drain_lua_lsp_completions();
        self.drain_lua_command_completions();
        let mut plugin_actions = Vec::new();
        for (window_id, route) in &mut self.router.routes {
            plugin_actions.extend(
                route
                    .window
                    .screen
                    .take_lua_plugin_actions()
                    .into_iter()
                    .map(|(id, action)| (*window_id, id, action)),
            );
        }
        for (window_id, id, action) in plugin_actions {
            self.handle_lua_plugin_action(window_id, id, action);
        }
        let completions = self.lua_plugin_jobs.pump(&self.event_proxy);
        let jobs_changed = self.lua_plugin_jobs.take_changed();
        if !completions.is_empty() {
            for completion in completions {
                if completion.success {
                    let candidate =
                        crate::plugin_manager::resolve_mashup_selection(&self.config)
                            .and_then(|selection| {
                                crate::plugin_manager::LuaPluginManager::discover(
                                    &neoism_backend::config::config_dir_path(),
                                    self.lua_host.clone(),
                                    &self.config.plugins,
                                    selection.as_ref(),
                                )
                            });
                    match candidate {
                        Ok(manager) => self.lua_plugins = manager,
                        Err(error) => {
                            let message = format!(
                                "Installed package could not be activated: {error}"
                            );
                            if completion.lock_changed {
                                let store = neoism_extensions::lua_plugins::LuaPluginStore::managed();
                                if let Err(rollback_error) = store.rollback_lock_entry(
                                    &completion.plugin_id,
                                    completion.previous_lock_entry,
                                ) {
                                    tracing::error!(plugin = %completion.plugin_id, %rollback_error, "failed to roll back rejected Lua plugin lock entry");
                                }
                            }
                            self.lua_plugin_jobs.record_failure(
                                completion.plugin_id.clone(),
                                message.clone(),
                            );
                            tracing::warn!(plugin = %completion.plugin_id, %message);
                            continue;
                        }
                    }
                } else {
                    tracing::warn!(plugin = %completion.plugin_id, error = %completion.message, "Lua plugin lifecycle operation failed");
                }
            }
            self.sync_lua_snapshot(None);
        } else if jobs_changed {
            self.refresh_lua_extension_rows();
        }
        if self
            .router
            .routes
            .values()
            .any(|route| route.window.screen.needs_lua_plugin_entries())
        {
            self.refresh_lua_extension_rows();
        }
        self.poll_lua_async();
        if self.lua_runtime.is_none() && self.lua_plugins.is_empty() {
            return;
        }
        self.sync_lua_editor_resources();
        let now = Instant::now();
        self.poll_lua_timers(now);
        let mut state_published = false;
        let mut needs_state_poll = false;
        let mut needs_urgent_composer_publish = false;
        for autocmd in self
            .lua_runtime
            .as_ref()
            .into_iter()
            .flat_map(|runtime| &runtime.snapshot().autocmds)
            .chain(self.lua_plugins.snapshot().autocmds.iter())
        {
            needs_state_poll |= lua_autocmd_needs_state_poll(&autocmd.event);
            needs_urgent_composer_publish |=
                lua_autocmd_needs_urgent_composer_publish(&autocmd.event);
        }
        let composer_changed = needs_urgent_composer_publish
            && self
                .router
                .get_focused_route()
                .and_then(|window_id| self.router.routes.get(&window_id))
                .and_then(|route| {
                    route
                        .window
                        .screen
                        .context_manager
                        .current()
                        .neoism_agent
                        .as_ref()
                        .map(|agent| agent.input().to_string())
                })
                .unwrap_or_default()
                != self.lua_composer_text;
        if self.lua_published.is_empty()
            || composer_changed
            || (needs_state_poll && now >= self.lua_state_publish_deadline)
        {
            self.publish_lua_state();
            self.lua_state_publish_deadline = now + LUA_STATE_PUBLISH_INTERVAL;
            state_published = true;
        }
        let mut commands = Vec::new();
        let mut keys = Vec::new();
        let mut plugin_actions = Vec::new();
        let mut prompt_replies = Vec::new();
        for (window_id, route) in &mut self.router.routes {
            plugin_actions.extend(
                route
                    .window
                    .screen
                    .take_plugin_actions()
                    .into_iter()
                    .map(|action| (*window_id, action)),
            );
            prompt_replies.extend(
                route
                    .window
                    .screen
                    .take_lua_prompt_replies()
                    .into_iter()
                    .map(|reply| (*window_id, reply)),
            );
            commands.extend(
                route
                    .window
                    .screen
                    .take_plugin_commands()
                    .into_iter()
                    .map(|command| (*window_id, command)),
            );
            keys.extend(
                route
                    .window
                    .screen
                    .take_plugin_keys()
                    .into_iter()
                    .map(|key| (*window_id, key)),
            );
        }
        for (window_id, (request_id, value, cancelled)) in prompt_replies {
            let owner = self
                .lua_prompts
                .as_mut()
                .and_then(|prompts| prompts.remove(&(window_id, request_id.clone())));
            let Some(owner) = owner else { continue };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if cancelled {
                coordinator.cancel(&owner, &request_id);
            } else {
                coordinator.sender().complete(
                    owner,
                    request_id,
                    serde_json::json!({ "value": value }),
                );
            }
        }
        for (window_id, action) in plugin_actions {
            self.apply_lua_action(window_id, action);
        }
        for (window_id, key) in keys {
            if !state_published {
                self.publish_lua_state();
                self.lua_state_publish_deadline =
                    Instant::now() + LUA_STATE_PUBLISH_INTERVAL;
                state_published = true;
            }
            let revision_before = self.lua_plugins.revision();
            if let Err(error) = self.lua_plugins.activate_key(&key) {
                tracing::warn!(%key, %error, "lazy Lua plugin key activation failed");
                continue;
            }
            if self.lua_plugins.revision() != revision_before {
                self.sync_lua_snapshot(Some(window_id));
            }
            let command = self
                .router
                .routes
                .get_mut(&window_id)
                .and_then(|route| route.window.screen.replay_plugin_key(&key));
            if let Some(command) = command {
                commands.push((window_id, command));
            } else {
                tracing::warn!(%key, "lazy Lua plugin did not register its declared keymap");
            }
        }
        let mut builtins = Vec::new();
        {
            for (window_id, id) in commands {
                if !state_published {
                    self.publish_lua_state();
                    self.lua_state_publish_deadline =
                        Instant::now() + LUA_STATE_PUBLISH_INTERVAL;
                    state_published = true;
                }
                let revision_before = self.lua_plugins.revision();
                if let Err(error) = self.lua_plugins.activate_command(&id) {
                    tracing::warn!(command = %id, %error, "lazy Lua plugin activation failed");
                }
                if self.lua_plugins.revision() != revision_before {
                    self.sync_lua_snapshot(None);
                }
                let callback = self
                    .lua_runtime
                    .as_ref()
                    .and_then(|runtime| {
                        runtime.snapshot().commands.iter().find(|command| {
                            command.id == id
                                || command.aliases.iter().any(|alias| alias == &id)
                        })
                    })
                    .or_else(|| {
                        self.lua_plugins.snapshot().commands.iter().find(|command| {
                            command.id == id
                                || command.aliases.iter().any(|alias| alias == &id)
                        })
                    })
                    .cloned();
                let Some(command) = callback else {
                    builtins.push((window_id, id));
                    continue;
                };
                if command.callback.is_empty() {
                    tracing::warn!(command = %id, "lazy Lua plugin did not register its declared command");
                    continue;
                }
                let request = neoism_lua::PluginCommandRequest {
                    command: id.clone(),
                    arguments: serde_json::json!({}),
                    range: None,
                    count: None,
                    bang: false,
                };
                if let Err(error) =
                    neoism_lua::validate_command_request(&command, &request)
                {
                    tracing::warn!(command = %id, %error, "Lua palette command arguments rejected");
                    continue;
                }
                let event = neoism_lua::PluginEvent {
                    name: "Command".into(),
                    payload: serde_json::json!({
                        "id": command.id,
                        "requestedAs": id,
                        "arguments": request.arguments,
                        "range": null,
                        "count": null,
                        "bang": false,
                    }),
                    scope: neoism_lua::ExecutionScope::Local,
                    origin: Some("command-palette".into()),
                };
                let result = if command.callback.starts_with("lua:neoism.user-init@") {
                    self.lua_runtime
                        .as_ref()
                        .expect("user-init callback requires runtime")
                        .invoke(&command.callback, event)
                        .map_err(|error| error.to_string())
                } else {
                    self.lua_plugins
                        .invoke(&command.callback, event)
                        .map_err(|error| error.to_string())
                };
                match result {
                    Ok(value) => {
                        if let Err(error) = neoism_lua::validate_command_arguments(
                            &command.result_schema,
                            &value,
                        ) {
                            tracing::warn!(%error, "Lua command returned an invalid structured result");
                        }
                    }
                    Err(error) => tracing::warn!(%error, "Lua command failed"),
                }
            }
        }
        for (window_id, id) in builtins {
            let Some(command) = lua_palette_action(&id) else {
                continue;
            };
            let router = &mut self.router;
            if let Some(route) = router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .execute_palette_action(command, &mut router.clipboard);
                route.request_redraw();
            }
        }

        let actions = self.lua_host.drain_actions();
        let Some(window_id) = self.router.get_focused_route() else {
            self.flush_lua_persistent_state();
            return;
        };
        for action in actions {
            self.apply_lua_action(window_id, action);
        }
        self.flush_lua_persistent_state();
    }

    fn pump_mashup_pack_requests(&mut self) {
        let requests = self
            .router
            .routes
            .iter_mut()
            .filter_map(|(window_id, route)| {
                route
                    .window
                    .screen
                    .take_mashup_pack_request()
                    .map(|id| (*window_id, id))
            })
            .collect::<Vec<_>>();
        for (window_id, requested_id) in requests {
            if let Err(error) =
                self.apply_mashup_pack_transaction(window_id, requested_id)
            {
                tracing::warn!(target: "neoism::mashup", %error, "Mash Up Pack transaction rejected");
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.report_mashup_pack_error(error);
                    route.request_redraw();
                }
            }
        }
    }

    fn apply_mashup_pack_transaction(
        &mut self,
        window_id: WindowId,
        requested_id: Option<String>,
    ) -> Result<(), String> {
        crate::mashup::sync_custom_ide_themes();
        let packs = neoism_backend::config::mashup::load_mashup_packs();
        // Theme picker writes are application-visible through the config file
        // before the watcher necessarily updates `self.config`; resolve and
        // rollback against the latest persisted four-field appearance state.
        let previous_appearance = neoism_backend::config::Config::load().appearance;
        let transition = neoism_backend::config::mashup::resolve_appearance_transition(
            previous_appearance.mashup_pack.as_deref(),
            previous_appearance.mashup_baseline.as_ref(),
            &previous_appearance.theme,
            previous_appearance.fonts.family.as_deref(),
            requested_id.as_deref(),
            &packs,
        )
        .map_err(|error| error.to_string())?;
        let requested_pack = transition
            .mashup_pack
            .as_deref()
            .and_then(|id| packs.iter().find(|pack| pack.id == id))
            .cloned();
        let mut candidate_config = self.config.clone();
        candidate_config.appearance.mashup_pack = transition.mashup_pack;
        candidate_config.appearance.mashup_baseline = transition.mashup_baseline;
        candidate_config.appearance.theme =
            neoism_ui::primitives::ide_theme::IdeTheme::by_name(&transition.theme)
                .name
                .as_str()
                .to_string();
        candidate_config.appearance.fonts.family = transition.font_family;
        let selection_packs = requested_pack
            .as_ref()
            .map(std::slice::from_ref)
            .unwrap_or(&[]);
        let selection = neoism_backend::config::mashup::resolve_editor_plugin_selection(
            candidate_config.appearance.mashup_pack.as_deref(),
            selection_packs,
            &candidate_config.plugins.mashup_overrides,
        )
        .map_err(|error| error.to_string())?;

        let candidate_host = Arc::new(self.lua_host.fork_candidate());
        candidate_host.publish(
            "config",
            serde_json::to_value(&candidate_config).unwrap_or(serde_json::Value::Null),
        );
        let candidate_runtime = if neoism_backend::config::config_dir_path()
            .join("init.lua")
            .is_file()
        {
            Some(
                neoism_lua::LuaRuntime::load(
                    neoism_backend::config::config_dir_path(),
                    candidate_host.clone(),
                )
                .map_err(|error| format!("init.lua: {error}"))?,
            )
        } else {
            None
        };
        let candidate_manager = crate::plugin_manager::LuaPluginManager::discover(
            neoism_backend::config::config_dir_path(),
            candidate_host.clone(),
            &candidate_config.plugins,
            selection.as_ref(),
        )
        .map_err(|error| error.to_string());

        let font_changed =
            candidate_config.appearance.fonts != self.config.appearance.fonts;
        let candidate_font_library = font_changed.then(|| {
            neoism_backend::sugarloaf::font::FontLibrary::new(
                crate::mashup::fonts_with_markdown_family(
                    candidate_config.appearance.fonts.clone(),
                    candidate_config
                        .appearance
                        .look
                        .markdown
                        .font_family
                        .as_deref(),
                ),
            )
            .0
        });

        commit_validated_mashup_candidate(candidate_manager, |candidate_manager| {
            if !self.router.routes.contains_key(&window_id) {
                return Err(
                    "requesting window closed before Mash Up Pack commit".to_string()
                );
            }
            neoism_backend::config::write_mashup_pack_settings(
                candidate_config.appearance.mashup_pack.as_deref(),
                candidate_config.appearance.mashup_baseline.as_ref(),
                &candidate_config.appearance.theme,
                candidate_config.appearance.fonts.family.as_deref(),
            )
            .map_err(|error| {
                format!("failed to persist Mash Up Pack transaction: {error}")
            })?;

            let visual_result = self
                .router
                .routes
                .get_mut(&window_id)
                .expect("requesting route checked immediately before persistence")
                .window
                .screen
                .apply_resolved_mashup_pack(
                    requested_pack.as_ref(),
                    &candidate_config,
                    candidate_font_library.as_ref(),
                );
            if let Err(error) = visual_result {
                if let Err(rollback_error) =
                    neoism_backend::config::write_mashup_pack_settings(
                        previous_appearance.mashup_pack.as_deref(),
                        previous_appearance.mashup_baseline.as_ref(),
                        &previous_appearance.theme,
                        previous_appearance.fonts.family.as_deref(),
                    )
                {
                    tracing::error!(target: "neoism::mashup", %rollback_error, "failed to roll back rejected Mash Up Pack config write");
                }
                return Err(format!("failed to apply Mash Up Pack visuals: {error}"));
            }

            if let Some(font_library) = candidate_font_library {
                *self.router.font_library = font_library;
            }
            self.config = candidate_config;
            self.lua_host = candidate_host;
            self.lua_runtime = candidate_runtime;
            self.lua_plugins = candidate_manager;
            self.lua_published.clear();
            self.sync_lua_snapshot(Some(window_id));
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.request_redraw();
            }
            Ok(())
        })
    }

    fn flush_lua_persistent_state(&self) {
        if !self.lua_host.take_persistent_dirty() {
            return;
        }
        let path = neoism_backend::config::config_dir_path().join("plugin-state.json");
        let temporary = path.with_extension("json.tmp");
        let result =
            serde_json::to_vec_pretty(&self.lua_host.persistent_state_snapshot())
                .map_err(std::io::Error::other)
                .and_then(|bytes| std::fs::write(&temporary, bytes))
                .and_then(|()| std::fs::rename(&temporary, &path));
        if let Err(error) = result {
            self.lua_host.mark_persistent_dirty();
            tracing::warn!(%error, "failed to persist bounded Lua plugin state");
        }
    }

    fn drain_lua_lsp_completions(&mut self) {
        for mut completed in
            crate::screen::bridges::code::lsp::drain_lua_lsp_completions()
        {
            let key = (completed.owner.clone(), completed.completion.id.clone());
            let Some(pending) = self.lua_lsp_pending.remove(&key) else {
                continue;
            };
            if pending.window_id != completed.window_id
                || pending.operation != completed.completion.operation
                || completed.completion.target.as_ref() != Some(&pending.target)
            {
                tracing::warn!(
                    plugin = %completed.owner.plugin_id,
                    request = %completed.completion.id,
                    "discarded mismatched structured LSP completion"
                );
                continue;
            }
            if !self.lua_lsp_owner_is_active(&completed.owner) {
                tracing::debug!(plugin = %completed.owner.plugin_id, request = %completed.completion.id, "discarded stale structured LSP owner before private result installation");
                continue;
            }
            match completed.private.take() {
                Some(crate::screen::bridges::code::lsp::LuaLspPrivateResult::CodeActions(
                    actions,
                )) => {
                    let now = Instant::now();
                    self.lua_lsp_actions.retain(|_, lease| {
                        now.saturating_duration_since(lease.created_at) < LUA_LSP_ACTION_TTL
                    });
                    let owner_actions = self
                        .lua_lsp_actions
                        .keys()
                        .filter(|(owner, _, _)| owner == &completed.owner)
                        .count();
                    if owner_actions.saturating_add(actions.len()) > LUA_LSP_ACTIONS_PER_OWNER
                        || self.lua_lsp_actions.len().saturating_add(actions.len())
                            > LUA_LSP_ACTIONS_GLOBAL
                    {
                        completed.completion = neoism_lua::LuaLspCompletion::failed(
                            completed.completion.id,
                            completed.completion.operation,
                            Some(pending.target.clone()),
                            "action_limit",
                            "too many structured LSP code actions are retained",
                        );
                    } else {
                        let source_request_id = completed.completion.id.clone();
                        for (action_id, action) in actions {
                            if action.target() != &pending.target {
                                tracing::warn!(plugin = %completed.owner.plugin_id, request = %source_request_id, "discarded mismatched structured LSP action material");
                                continue;
                            }
                            self.lua_lsp_actions.insert(
                                (
                                    completed.owner.clone(),
                                    source_request_id.clone(),
                                    action_id,
                                ),
                            LuaLspActionLease {
                                window_id: pending.window_id,
                                target: pending.target.clone(),
                                created_at: now,
                                action,
                            },
                            );
                        }
                    }
                }
                Some(crate::screen::bridges::code::lsp::LuaLspPrivateResult::Mutation(
                    mutation,
                )) => {
                    if mutation.target != pending.target {
                        completed.completion = neoism_lua::LuaLspCompletion::failed(
                            completed.completion.id,
                            completed.completion.operation,
                            Some(pending.target.clone()),
                            "stale_target",
                            "structured LSP mutation target did not match its request",
                        );
                    } else {
                        let deferred_command = mutation.command.clone();
                        let command_root = mutation.root.clone();
                        let applied = self
                            .router
                            .routes
                            .get_mut(&pending.window_id)
                            .ok_or_else(|| "the mutation window is no longer available".to_string())
                            .and_then(|route| {
                                let result = route
                                    .window
                                    .screen
                                    .apply_local_lua_lsp_mutation(mutation);
                                if result.is_ok() {
                                    route.request_redraw();
                                }
                                result
                            });
                        completed.completion = match applied {
                            Ok(mutation) => {
                                if let Some((server_id, command)) = deferred_command {
                                    let dispatched = self
                                        .router
                                        .routes
                                        .get_mut(&pending.window_id)
                                        .ok_or_else(|| {
                                            "the mutation window is no longer available".to_string()
                                        })
                                        .and_then(|route| {
                                            route.window.screen.dispatch_local_lua_lsp_command(
                                                completed.owner.clone(),
                                                completed.completion.id.clone(),
                                                pending.target.clone(),
                                                command_root,
                                                server_id,
                                                command,
                                                mutation,
                                            )
                                        });
                                    if let Err(message) = dispatched {
                                        neoism_lua::LuaLspCompletion::failed(
                                            completed.completion.id,
                                            completed.completion.operation,
                                            Some(pending.target.clone()),
                                            "command_failed",
                                            message,
                                        )
                                    } else {
                                        self.lua_lsp_pending.insert(key, pending);
                                        continue;
                                    }
                                } else {
                                    let result = match completed.completion.operation {
                                        neoism_lua::LuaLspOperation::ApplyCodeAction => {
                                            neoism_lua::LuaLspOutcome::ApplyCodeAction(mutation)
                                        }
                                        neoism_lua::LuaLspOperation::Rename => {
                                            neoism_lua::LuaLspOutcome::Rename(mutation)
                                        }
                                        neoism_lua::LuaLspOperation::Format => {
                                            neoism_lua::LuaLspOutcome::Format(mutation)
                                        }
                                        _ => {
                                            unreachable!("only edit operations produce mutations")
                                        }
                                    };
                                    neoism_lua::LuaLspCompletion::success(
                                        completed.completion.id,
                                        completed.completion.operation,
                                        pending.target.clone(),
                                        result,
                                    )
                                }
                            }
                            Err(message) => neoism_lua::LuaLspCompletion::failed(
                                completed.completion.id,
                                completed.completion.operation,
                                Some(pending.target.clone()),
                                "mutation_rejected",
                                message,
                            ),
                        };
                    }
                }
                Some(
                    crate::screen::bridges::code::lsp::LuaLspPrivateResult::RemoteMutation(
                        mutation,
                    ),
                ) => {
                    if mutation.target != pending.target {
                        completed.completion = neoism_lua::LuaLspCompletion::failed(
                            completed.completion.id,
                            completed.completion.operation,
                            Some(pending.target.clone()),
                            "stale_target",
                            "remote LSP mutation target did not match its request",
                        );
                    } else {
                        let dispatched = self
                            .router
                            .routes
                            .get_mut(&pending.window_id)
                            .ok_or_else(|| {
                                "the mutation window is no longer available".to_string()
                            })
                            .and_then(|route| {
                                route.window.screen.dispatch_remote_lua_mutation(
                                    completed.owner.clone(),
                                    completed.completion.id.clone(),
                                    mutation,
                                )
                            });
                        match dispatched {
                            Ok(target) if target == pending.target => {
                                self.lua_lsp_pending.insert(key, pending);
                                continue;
                            }
                            Ok(_) => {
                                completed.completion = neoism_lua::LuaLspCompletion::failed(
                                    completed.completion.id,
                                    completed.completion.operation,
                                    Some(pending.target.clone()),
                                    "stale_target",
                                    "remote mutation target no longer matches its request",
                                );
                            }
                            Err(message) => {
                                completed.completion = neoism_lua::LuaLspCompletion::failed(
                                    completed.completion.id,
                                    completed.completion.operation,
                                    Some(pending.target.clone()),
                                    "mutation_rejected",
                                    message,
                                );
                            }
                        }
                    }
                }
                None => {}
            }
            self.emit_lua_lsp_completion(&completed.owner, completed.completion);
        }
    }

    fn lua_lsp_owner_is_active(&self, owner: &neoism_lua::PluginOwner) -> bool {
        self.lua_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.owner() == owner)
            || self.lua_plugins.is_active_owner(owner)
    }

    fn emit_lua_lsp_completion(
        &mut self,
        owner: &neoism_lua::PluginOwner,
        completion: neoism_lua::LuaLspCompletion,
    ) {
        if let Err(error) = neoism_lua::validate_async_result(
            neoism_lua::AsyncResultKind::Lsp,
            owner,
            &completion.id,
        ) {
            tracing::debug!(plugin = %owner.plugin_id, %error, "discarded invalid Lua async result");
            return;
        }
        let Ok(payload) = serde_json::to_value(completion) else {
            tracing::warn!(plugin = %owner.plugin_id, "failed to serialize structured LSP completion");
            return;
        };
        let event = neoism_lua::PluginEvent::new(
            neoism_lua::PluginEventKind::LspResult,
            payload,
            neoism_lua::ExecutionScope::Local,
            Some("desktop-lsp".into()),
        )
        .expect("registered LSP result event");
        let user_owner_matches = self
            .lua_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.owner() == owner);
        let changed = if user_owner_matches {
            let before = self
                .lua_runtime
                .as_ref()
                .map(|runtime| runtime.snapshot().clone());
            let result = self
                .lua_runtime
                .as_mut()
                .expect("matched user Lua owner must have a runtime")
                .emit(event);
            if let Err(error) = result {
                tracing::warn!(plugin = %owner.plugin_id, %error, "Lua LspResult autocmd failed");
            }
            self.lua_runtime
                .as_ref()
                .map(neoism_lua::LuaRuntime::snapshot)
                != before.as_ref()
        } else {
            match self.lua_plugins.emit_to_owner(owner, event) {
                Ok(changed) => changed,
                Err(error) => {
                    tracing::debug!(plugin = %owner.plugin_id, revision = %owner.revision.0, %error, "discarded stale Lua LSP completion");
                    false
                }
            }
        };
        if changed {
            self.sync_lua_snapshot(None);
        }
    }

    fn drain_lua_command_completions(&mut self) {
        let completions = std::mem::take(&mut self.lua_command_completions);
        for ((owner, _), completion) in completions {
            if !self.lua_lsp_owner_is_active(&owner) {
                continue;
            }
            if let Err(error) = neoism_lua::validate_async_result(
                neoism_lua::AsyncResultKind::Command,
                &owner,
                &completion.id,
            ) {
                tracing::debug!(plugin = %owner.plugin_id, %error, "discarded invalid Lua command result");
                continue;
            }
            let Ok(payload) = serde_json::to_value(completion) else {
                continue;
            };
            let event = neoism_lua::PluginEvent::new(
                neoism_lua::PluginEventKind::CommandResult,
                payload,
                neoism_lua::ExecutionScope::Local,
                Some("desktop-command".into()),
            )
            .expect("registered command result event");
            let changed = if self
                .lua_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.owner() == &owner)
            {
                let before = self
                    .lua_runtime
                    .as_ref()
                    .map(|runtime| runtime.snapshot().clone());
                if let Err(error) = self
                    .lua_runtime
                    .as_mut()
                    .expect("matched user runtime")
                    .emit(event)
                {
                    tracing::warn!(plugin = %owner.plugin_id, %error, "Lua CommandResult autocmd failed");
                }
                self.lua_runtime
                    .as_ref()
                    .map(neoism_lua::LuaRuntime::snapshot)
                    != before.as_ref()
            } else {
                self.lua_plugins
                    .emit_to_owner(&owner, event)
                    .unwrap_or(false)
            };
            if changed {
                self.sync_lua_snapshot(None);
            }
        }
    }

    fn publish_lua_state(&mut self) {
        let Some(window_id) = self.router.get_focused_route() else {
            return;
        };
        let composer_text = self
            .router
            .routes
            .get(&window_id)
            .and_then(|route| {
                route
                    .window
                    .screen
                    .context_manager
                    .current()
                    .neoism_agent
                    .as_ref()
                    .map(|agent| agent.input().to_string())
            })
            .unwrap_or_default();
        if composer_text != self.lua_composer_text {
            self.lua_composer_text = composer_text;
            self.lua_composer_revision = self.lua_composer_revision.wrapping_add(1);
        }
        let composer_length = self.lua_composer_text.chars().count();
        let composer_revision = self.lua_composer_revision;
        let Some(route) = self.router.routes.get(&window_id) else {
            return;
        };
        let screen = &route.window.screen;
        let manager = &screen.context_manager;
        let current = manager.current();
        let workspace_identity = format!("{:?}", manager.current_workspace_tree_id());
        let window_identity = format!("{window_id:?}");
        let route_identity = current.route_id.to_string();
        let workspace_handle = neoism_lua::WorkspaceHandle(
            neoism_lua::opaque_resource_handle("workspace", &[&workspace_identity]),
        );
        let pane_handle = neoism_lua::PaneHandle(neoism_lua::opaque_resource_handle(
            "pane",
            &[&window_identity, &workspace_identity, &route_identity],
        ));
        let tab_identity = manager.current_index().to_string();
        let tab_handle = neoism_lua::TabHandle(neoism_lua::opaque_resource_handle(
            "tab",
            &[
                &window_identity,
                &workspace_identity,
                &route_identity,
                &tab_identity,
            ],
        ));
        let option_route_id = current.route_id;
        let option_pane = pane_handle.0.clone();
        let option_tab = tab_handle.0.clone();
        let option_workspace = workspace_handle.0.clone();
        let mut document_state = serde_json::Value::Null;
        let mut pane_state = serde_json::to_value(neoism_lua::PaneSnapshot {
            handle: pane_handle.clone(),
            workspace: workspace_handle.clone(),
            focused: true,
            ..Default::default()
        })
        .unwrap_or_default();
        let buffer = if let Some(code) = current.code.as_ref() {
            let filetype = format!("{:?}", code.language).to_ascii_lowercase();
            let host_path = code.path.to_string_lossy().into_owned();
            let document_handle =
                neoism_lua::DocumentHandle(neoism_lua::opaque_resource_handle(
                    "document",
                    &[
                        &window_identity,
                        &workspace_identity,
                        &route_identity,
                        &host_path,
                    ],
                ));
            let revision = code.buffer.revision;
            let cursor_position = neoism_lua::TextPosition {
                line: code.buffer.cursor_line as u32,
                character: code.buffer.cursor_col as u32,
            };
            let cursor_handle =
                neoism_lua::CursorHandle(neoism_lua::opaque_resource_handle(
                    "cursor",
                    &[
                        document_handle.as_str(),
                        &revision.to_string(),
                        &cursor_position.line.to_string(),
                        &cursor_position.character.to_string(),
                    ],
                ));
            let mut selections = code
                .buffer
                .selection_range()
                .map(|(start, end)| {
                    let anchor = neoism_lua::TextPosition {
                        line: start.line as u32,
                        character: start.col as u32,
                    };
                    let active = neoism_lua::TextPosition {
                        line: end.line as u32,
                        character: end.col as u32,
                    };
                    neoism_lua::DocumentSelection {
                        handle: neoism_lua::SelectionHandle(
                            neoism_lua::opaque_resource_handle(
                                "selection",
                                &[
                                    document_handle.as_str(),
                                    &revision.to_string(),
                                    &anchor.line.to_string(),
                                    &anchor.character.to_string(),
                                    &active.line.to_string(),
                                    &active.character.to_string(),
                                ],
                            ),
                        ),
                        anchor,
                        active,
                    }
                })
                .into_iter()
                .collect::<Vec<_>>();
            selections.extend(code.buffer.extra_carets.iter().map(|caret| {
                let anchor =
                    caret
                        .anchor
                        .unwrap_or(neoism_ui::editor::code::CodePosition {
                            line: caret.line,
                            col: caret.col,
                        });
                let anchor = neoism_lua::TextPosition {
                    line: anchor.line as u32,
                    character: anchor.col as u32,
                };
                let active = neoism_lua::TextPosition {
                    line: caret.line as u32,
                    character: caret.col as u32,
                };
                neoism_lua::DocumentSelection {
                    handle: neoism_lua::SelectionHandle(
                        neoism_lua::opaque_resource_handle(
                            "selection",
                            &[
                                document_handle.as_str(),
                                &revision.to_string(),
                                &anchor.line.to_string(),
                                &anchor.character.to_string(),
                                &active.line.to_string(),
                                &active.character.to_string(),
                            ],
                        ),
                    ),
                    anchor,
                    active,
                }
            }));
            let document = neoism_lua::DocumentSnapshot {
                handle: document_handle.clone(),
                pane: pane_handle.clone(),
                tab: tab_handle.clone(),
                workspace: workspace_handle.clone(),
                revision,
                text: code.buffer.text(),
                cursor: neoism_lua::DocumentCursor {
                    handle: cursor_handle,
                    position: cursor_position,
                },
                selections,
                metadata: neoism_lua::DocumentMetadata {
                    title: code.title.clone(),
                    host_path: host_path.clone(),
                    language: filetype.clone(),
                    kind: "code".into(),
                    remote: manager.current_workspace_is_remote_joined(),
                    dirty: code.buffer.is_dirty(),
                    revision,
                    line_count: code.buffer.line_count() as u32,
                },
            };
            document_state = serde_json::to_value(&document).unwrap_or_default();
            pane_state = serde_json::to_value(neoism_lua::PaneSnapshot {
                handle: pane_handle.clone(),
                document: Some(document_handle.clone()),
                workspace: workspace_handle.clone(),
                rect: code.geometry.rect,
                first_visible_line: code.geometry.first_row as u32,
                visible_line_count: code.geometry.viewport_rows() as u32,
                scroll_x: code.geometry.scroll_x,
                scroll_y: code.geometry.scroll_y,
                focused: true,
            })
            .unwrap_or_default();
            serde_json::json!({
                "id": code.path.to_string_lossy(),
                "handle": document_handle,
                "pane": pane_handle,
                "tab": tab_handle,
                "workspace": workspace_handle,
                "path": code.path,
                "title": code.title,
                "kind": "code",
                "filetype": filetype,
                "mode": format!("{:?}", code.buffer.mode).to_ascii_lowercase(),
                "revision": revision,
                "dirty": code.buffer.is_dirty(),
            })
        } else if let Some(markdown) = current.active_markdown() {
            serde_json::json!({
                "id": markdown.path.to_string_lossy(),
                "path": markdown.path,
                "kind": "markdown",
                "filetype": "markdown",
            })
        } else {
            serde_json::json!({ "id": current.route_id, "kind": "terminal" })
        };
        let mut tabs = manager
            .titles
            .titles
            .iter()
            .map(|(id, title)| serde_json::json!({ "id": id, "title": title.content }))
            .collect::<Vec<_>>();
        tabs.sort_by_key(|tab| tab.get("id").and_then(serde_json::Value::as_u64));
        let workspace = serde_json::json!({
            "handle": workspace_handle,
            "currentId": manager.current_workspace_tree_id(),
            "currentIndex": manager.current_index(),
            "collaborative": manager.current_workspace_is_collaborative(),
            "remote": manager.current_workspace_is_remote_joined(),
            "items": manager.local_workspace_summaries(),
        });
        let panel = serde_json::json!({
            "fileTree": { "visible": screen.renderer.file_tree.is_visible(), "focused": screen.renderer.file_tree.is_focused() },
            "notes": { "visible": screen.renderer.notes_sidebar.is_visible(), "focused": screen.renderer.notes_sidebar.is_focused() },
            "git": { "visible": screen.renderer.git_diff_panel.is_visible(), "focused": screen.renderer.git_diff_panel.is_focused() },
            "agentSidebar": { "visible": screen.renderer.conversations_visible },
            "status": { "visible": screen.renderer.status_line.is_visible() },
            "top": { "visible": screen.renderer.top_bar.is_visible() },
            "composer": { "visible": screen.renderer.command_composer.is_visible() },
        });
        let file_tree_entries = screen
            .renderer
            .file_tree
            .entries()
            .iter()
            .map(|entry| {
                let (kind, open) = match entry.kind {
                    neoism_ui::panels::file_tree::NodeKind::File => ("file", None),
                    neoism_ui::panels::file_tree::NodeKind::Dir { open } => {
                        ("directory", Some(open))
                    }
                };
                serde_json::json!({
                    "label": entry.label,
                    "path": entry.path,
                    "depth": entry.depth,
                    "kind": kind,
                    "open": open,
                    "gitStatus": format!("{:?}", entry.git_status).to_ascii_lowercase(),
                    "virtual": entry.virtual_kind.map(|kind| format!("{kind:?}").to_ascii_lowercase()),
                })
            })
            .collect::<Vec<_>>();
        let notes_entries = screen
            .renderer
            .notes_sidebar
            .entries()
            .iter()
            .map(|entry| {
                serde_json::json!({
                    "label": entry.label,
                    "path": entry.path,
                    "depth": entry.depth(),
                    "kind": if entry.is_dir { "directory" } else { "note" },
                    "icon": entry.icon,
                })
            })
            .collect::<Vec<_>>();
        let agent_sessions = current
            .neoism_agent
            .as_ref()
            .map(|agent| {
                agent
                    .side_panel()
                    .sessions()
                    .iter()
                    .filter(|entry| !entry.is_header && !entry.is_excerpt)
                    .map(|entry| {
                        serde_json::json!({
                            "id": entry.id,
                            "title": entry.title,
                            "timeLabel": entry.time_label,
                            "depth": entry.depth,
                            "source": entry.source.provider().unwrap_or("neoism"),
                            "agentKind": entry.agent_kind.map(|kind| format!("{kind:?}").to_ascii_lowercase()),
                            "runtimeStatus": entry.runtime_status,
                            "updatedMs": entry.updated_ms,
                            "pinned": entry.pinned,
                        })
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let terminal_sessions = manager
            .sessions()
            .iter()
            .map(|session| serde_json::to_value(session).unwrap_or_default())
            .collect::<Vec<_>>();
        let git_files = screen
            .renderer
            .git_diff_panel
            .files()
            .into_iter()
            .map(|file| {
                serde_json::json!({
                    "path": file.path,
                    "status": file.status.as_str(),
                    "additions": file.additions,
                    "deletions": file.deletions,
                    "staged": file.staged,
                })
            })
            .collect::<Vec<_>>();
        let (registers, marks, jumplist, macros) = current.code.as_ref().map_or_else(
            || (serde_json::json!({}), serde_json::json!({}), serde_json::json!([]), serde_json::json!({})),
            |code| {
                let register_value = |value: &neoism_ui::editor::markdown::vim::VimRegisterValue| {
                    serde_json::json!({ "text": value.text, "linewise": value.linewise, "blockwise": value.blockwise })
                };
                let mut registers = serde_json::Map::new();
                registers.insert("\"".into(), register_value(&code.buffer.vim.registers.unnamed));
                registers.insert("0".into(), register_value(&code.buffer.vim.registers.yank));
                for (index, value) in code.buffer.vim.registers.deletes.iter().enumerate() {
                    registers.insert((index + 1).to_string(), register_value(value));
                }
                for (name, value) in &code.buffer.vim.registers.named {
                    registers.insert(name.to_string(), register_value(value));
                }
                let marks = code.buffer.vim.marks.iter().map(|(name, mark)| {
                    (name.to_string(), serde_json::json!({ "line": mark.line, "character": mark.col }))
                }).collect::<serde_json::Map<_, _>>();
                let jumps = code.buffer.vim.jumplist.iter().map(|mark| {
                    serde_json::json!({ "line": mark.line, "character": mark.col })
                }).collect::<Vec<_>>();
                let macros = code.buffer.vim.registers.macros.iter().map(|(name, keys)| {
                    (name.to_string(), serde_json::Value::String(keys.clone()))
                }).collect::<serde_json::Map<_, _>>();
                (serde_json::Value::Object(registers), serde_json::Value::Object(marks), serde_json::Value::Array(jumps), serde_json::Value::Object(macros))
            },
        );
        let states = [
            ("buffer", buffer),
            ("document", document_state),
            ("pane", pane_state),
            ("workspace", workspace),
            ("tab", serde_json::json!({ "handle": tab_handle, "current": manager.current_index(), "items": tabs })),
            ("panel", panel),
            ("file_tree", serde_json::json!({
                "visible": screen.renderer.file_tree.is_visible(),
                "focused": screen.renderer.file_tree.is_focused(),
                "root": screen.renderer.file_tree.root(),
                "selectedPath": screen.renderer.file_tree.selected_path(),
                "selectedIndex": screen.renderer.file_tree.selected_index(),
                "showHidden": screen.renderer.file_tree.show_hidden(),
                "remote": screen.renderer.file_tree.is_remote(),
                "entries": file_tree_entries,
            })),
            ("notes", serde_json::json!({
                "visible": screen.renderer.notes_sidebar.is_visible(),
                "focused": screen.renderer.notes_sidebar.is_focused(),
                "workspacePath": screen.renderer.notes_sidebar.workspace_path(),
                "notebookRoot": screen.renderer.notes_sidebar.notebook_root(),
                "selectedPath": screen.renderer.notes_sidebar.selected_note_path(),
                "selectedIndex": screen.renderer.notes_sidebar.selected_index(),
                "remote": screen.renderer.notes_sidebar.is_remote_workspace(),
                "entries": notes_entries,
            })),
            ("agent", current.neoism_agent.as_ref().map_or_else(
                || serde_json::json!({
                    "active": false,
                    "sidebarVisible": screen.renderer.conversations_visible,
                    "composerRevision": composer_revision,
                    "composerLength": composer_length,
                    "composerEmpty": composer_length == 0,
                    "sessions": [],
                }),
                |agent| serde_json::json!({
                    "active": true,
                    "sidebarVisible": screen.renderer.conversations_visible,
                    "composerRevision": composer_revision,
                    "composerLength": composer_length,
                    "composerEmpty": composer_length == 0,
                    "sessionId": agent.session_id_str(),
                    "title": agent.session_title(),
                    "directory": agent.session_directory(),
                    "server": agent.server_address(),
                    "streaming": agent.is_streaming(),
                    "streamingState": format!("{:?}", agent.streaming_state()).to_ascii_lowercase(),
                    "streamingLabel": agent.streaming_label(),
                    "sessions": agent_sessions,
                })
            )),
            ("terminal", serde_json::json!({
                "active": !current.has_non_terminal_surface(),
                "routeId": current.route_id,
                "shellPid": current.shell_pid,
                "activeSessionId": manager.cached_active_session_id(),
                "sessions": terminal_sessions,
            })),
            ("git", serde_json::json!({
                "visible": screen.renderer.git_diff_panel.is_visible(),
                "focused": screen.renderer.git_diff_panel.is_focused(),
                "branch": screen.renderer.git_diff_panel.branch(),
                "repoRoot": screen.renderer.git_diff_panel.repo_root(),
                "selectedIndex": screen.renderer.git_diff_panel.selected_file_index(),
                "loading": screen.renderer.git_diff_panel.loading(),
                "error": screen.renderer.git_diff_panel.error(),
                "commitMessage": screen.renderer.git_diff_panel.commit_input_text(),
                "allStaged": screen.renderer.git_diff_panel.all_files_staged(),
                "files": git_files,
            })),
            ("theme", serde_json::to_value(&self.config.appearance.theme).unwrap_or_default()),
            ("plugins", self.lua_plugins.status_snapshot()),
            ("config", serde_json::to_value(&self.config).unwrap_or_default()),
            ("register", registers),
            ("mark", marks),
            ("jumplist", jumplist),
            ("macro", macros),
        ];
        let mut active_surfaces = vec![states[0]
            .1
            .get("kind")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("terminal")
            .to_string()];
        for (visible, name) in [
            (screen.renderer.file_tree.is_visible(), "file-tree"),
            (screen.renderer.notes_sidebar.is_visible(), "notes"),
            (screen.renderer.git_diff_panel.is_visible(), "git"),
            (screen.renderer.conversations_visible, "agent-sidebar"),
            (screen.renderer.command_palette.is_enabled(), "palette"),
            (screen.renderer.finder.is_enabled(), "finder"),
        ] {
            if visible {
                active_surfaces.push(name.into());
            }
        }
        let surface_revision = self.lua_plugins.revision();
        if let Some(filetype) = states[0]
            .1
            .get("filetype")
            .and_then(serde_json::Value::as_str)
        {
            if let Err(error) = self.lua_plugins.activate_filetype(filetype) {
                tracing::warn!(%filetype, %error, "lazy Lua plugin filetype activation failed");
            }
        }
        for surface in active_surfaces {
            if let Err(error) = self.lua_plugins.activate_surface(&surface) {
                tracing::warn!(%surface, %error, "lazy Lua plugin surface activation failed");
            }
        }
        if self.lua_plugins.revision() != surface_revision {
            self.sync_lua_snapshot(Some(window_id));
        }
        let initialized = !self.lua_published.is_empty();
        let mut events = Vec::new();
        for (namespace, value) in states {
            let previous = self.lua_published.get(namespace).cloned();
            if previous.as_ref() == Some(&value) {
                continue;
            }
            if let Some(scope) = match namespace {
                "document" => Some(neoism_lua::PluginStateScope::Document),
                "pane" => Some(neoism_lua::PluginStateScope::Pane),
                "tab" => Some(neoism_lua::PluginStateScope::Tab),
                "workspace" => Some(neoism_lua::PluginStateScope::Workspace),
                _ => None,
            } {
                let previous_handle = previous
                    .as_ref()
                    .and_then(|value| value.get("handle"))
                    .and_then(serde_json::Value::as_str);
                let next_handle = value.get("handle").and_then(serde_json::Value::as_str);
                if previous_handle != next_handle {
                    if let Some(previous_handle) = previous_handle {
                        self.lua_host.clear_target_state(scope, previous_handle);
                    }
                }
            }
            self.lua_host.publish(namespace, value.clone());
            self.lua_published
                .insert(namespace.to_string(), value.clone());
            if initialized {
                let event = match namespace {
                    "document" => {
                        let old_handle = previous
                            .as_ref()
                            .and_then(|value| value.get("handle"))
                            .and_then(serde_json::Value::as_str);
                        let new_handle =
                            value.get("handle").and_then(serde_json::Value::as_str);
                        if old_handle.is_none() && new_handle.is_some() {
                            "DocumentOpened"
                        } else if old_handle.is_some() && new_handle.is_none() {
                            "DocumentClosed"
                        } else if old_handle != new_handle {
                            "DocumentFocused"
                        } else {
                            "DocumentChanged"
                        }
                    }
                    "pane" => {
                        let old_handle =
                            previous.as_ref().and_then(|value| value.get("handle"));
                        if old_handle != value.get("handle") {
                            "PaneFocused"
                        } else {
                            "PaneChanged"
                        }
                    }
                    "buffer" => "BufferChanged",
                    "workspace" => "WorkspaceChanged",
                    "tab" => "TabChanged",
                    "panel" => "PanelChanged",
                    "file_tree" => "FileTreeChanged",
                    "notes" => "NotesChanged",
                    "agent" => "AgentChanged",
                    "terminal" => "TerminalChanged",
                    "git" => "GitChanged",
                    "theme" => "ThemeChanged",
                    "plugins" => "PluginsChanged",
                    "config" => "ConfigChanged",
                    _ => continue,
                };
                if namespace == "document"
                    && previous.as_ref().and_then(|value| value.get("selections"))
                        != value.get("selections")
                {
                    events.push(("SelectionChanged", value.clone()));
                }
                events.push((event, value));
            }
        }
        if events.is_empty() {
            return;
        }
        let user_snapshot_before_events = self
            .lua_runtime
            .as_ref()
            .map(|runtime| runtime.snapshot().clone());
        let plugin_revision_before_events = self.lua_plugins.revision();
        for (event, payload) in events {
            let plugin_event = neoism_lua::PluginEvent {
                name: event.to_string(),
                payload,
                scope: neoism_lua::ExecutionScope::Local,
                origin: Some("desktop-state".to_string()),
            };
            if event == "BufferChanged" {
                if let Some(filetype) = plugin_event
                    .payload
                    .get("filetype")
                    .and_then(serde_json::Value::as_str)
                {
                    if let Err(error) = self.lua_plugins.activate_filetype(filetype) {
                        tracing::warn!(%filetype, %error, "lazy Lua plugin filetype activation failed");
                    }
                }
            }
            if let Err(error) = self.lua_plugins.activate_trigger(event) {
                tracing::warn!(event, %error, "lazy Lua plugin event activation failed");
            }
            for failure in self.lua_plugins.emit(plugin_event.clone()) {
                tracing::warn!(event, plugin = %failure.plugin_id, error = %failure.message, "Lua plugin autocmd failed");
            }
            if let Some(runtime) = self.lua_runtime.as_mut() {
                if let Err(error) = runtime.emit(plugin_event) {
                    tracing::warn!(event, %error, "Lua autocmd failed");
                }
            }
        }
        let user_changed = self
            .lua_runtime
            .as_ref()
            .map(neoism_lua::LuaRuntime::snapshot)
            != user_snapshot_before_events.as_ref();
        let option_document = self
            .lua_published
            .get("document")
            .and_then(|value| value.get("handle"))
            .and_then(serde_json::Value::as_str)
            .map(str::to_owned);
        self.apply_lua_editor_options(
            window_id,
            option_route_id,
            option_document.as_deref(),
            &option_pane,
            &option_tab,
            &option_workspace,
        );
        if !user_changed && self.lua_plugins.revision() == plugin_revision_before_events {
            return;
        }
        self.sync_lua_snapshot(Some(window_id));
    }

    fn sync_lua_editor_resources(&mut self) {
        let mut active = self
            .lua_plugins
            .active_owners()
            .cloned()
            .collect::<HashSet<_>>();
        if let Some(runtime) = &self.lua_runtime {
            active.insert(runtime.owner().clone());
        }
        for retired in self.lua_active_owners.difference(&active) {
            self.lua_editor_resources.registry.remove_owner(retired);
            self.lua_host.remove_owner_state(retired);
            let _ = crate::credential_broker::broker().revoke_owner(retired);
        }
        self.lua_active_owners = active.clone();
        self.lua_editor_resources
            .namespaces
            .retain(|_, (owner, _)| active.contains(owner));
        self.lua_editor_resources
            .anchors
            .retain(|_, lease| active.contains(&lease.owner));
        self.lua_editor_resources
            .decorations
            .retain(|_, (owner, _, _, _)| active.contains(owner));

        let targets = self.lua_editor_resources.published_targets.clone();
        for (document, (window_id, route_id)) in &targets {
            let current_text = self
                .router
                .routes
                .get_mut(window_id)
                .and_then(|route| {
                    route
                        .window
                        .screen
                        .context_manager
                        .get_by_route_id(*route_id)
                })
                .and_then(|item| item.context().code.as_ref())
                .map(|code| code.buffer.text());
            let Some(current_text) = current_text else {
                continue;
            };
            if let Some(previous) = self
                .lua_editor_resources
                .document_text
                .insert(document.clone(), current_text.clone())
            {
                if let Some(delta) = neoism_ui::editor::markdown::doc_sync::diff_doc_texts(
                    &previous,
                    &current_text,
                ) {
                    let _ = self.lua_editor_resources.registry.apply_text_edit(
                        document,
                        delta.byte_start,
                        delta.byte_start.saturating_add(delta.byte_removed),
                        delta.inserted.len(),
                    );
                }
            }
        }

        let anchors = self
            .lua_editor_resources
            .anchors
            .values()
            .cloned()
            .collect::<Vec<_>>();
        for lease in anchors {
            let Some(sticky) = lease.sticky else { continue };
            let resolved = self.router.routes.get(&sticky.window_id).and_then(|route| {
                route
                    .window
                    .screen
                    .code_crdt
                    .binding_for(&sticky.buffer_id)
                    .and_then(|binding| binding.resolve_sticky_anchor(&sticky.anchor))
            });
            let Some((line, col)) = resolved else {
                continue;
            };
            let offset = self.router.routes.get(&lease.window_id).and_then(|route| {
                route
                    .window
                    .screen
                    .context_manager
                    .all_grids()
                    .iter()
                    .flat_map(|grid| grid.contexts().values())
                    .filter_map(|item| item.context().code.as_ref())
                    .find(|code| {
                        crate::screen::markdown_crdt::buffer_id_for_markdown_path(
                            &code.path,
                        ) == sticky.buffer_id
                    })
                    .map(|code| {
                        neoism_ui::editor::markdown::doc_sync::position_to_doc_byte(
                            &code.buffer.lines,
                            line,
                            col,
                        )
                    })
            });
            if let Some(offset) = offset {
                let _ = self.lua_editor_resources.registry.set_anchor_offset(
                    &lease.owner,
                    lease.namespace,
                    lease.resource,
                    offset,
                );
            }
        }

        for (document, (window_id, route_id)) in targets {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                self.lua_editor_resources
                    .published_targets
                    .remove(&document);
                self.lua_editor_resources.document_text.remove(&document);
                continue;
            };
            let Some(item) = route
                .window
                .screen
                .context_manager
                .get_by_route_id(route_id)
            else {
                self.lua_editor_resources
                    .published_targets
                    .remove(&document);
                self.lua_editor_resources.document_text.remove(&document);
                continue;
            };
            let Some(code) = item.context_mut().code.as_mut() else {
                self.lua_editor_resources
                    .published_targets
                    .remove(&document);
                self.lua_editor_resources.document_text.remove(&document);
                continue;
            };
            let text = code.buffer.text();
            let snapshot = self
                .lua_editor_resources
                .registry
                .snapshot_for_document(Some(&document), &text);
            let previous_revision = code
                .plugin_decorations
                .as_ref()
                .map(|snapshot| snapshot.revision);
            if previous_revision == Some(snapshot.revision)
                || (previous_revision.is_none() && snapshot.decorations.is_empty())
            {
                continue;
            }
            code.plugin_decorations = if snapshot.decorations.is_empty() {
                None
            } else {
                match neoism_ui::editor::code::feed::CodePluginRenderSnapshot::try_from_contract(&snapshot, &code.buffer.lines) {
                    Ok(snapshot) => Some(std::sync::Arc::new(snapshot)),
                    Err(error) => {
                        tracing::warn!(%error, "rejected invalid immutable Lua decoration geometry");
                        None
                    }
                }
            };
            route.window.screen.mark_dirty();
            route.request_redraw();
        }
    }

    fn apply_lua_editor_options(
        &mut self,
        window_id: WindowId,
        route_id: usize,
        document: Option<&str>,
        pane: &str,
        tab: &str,
        workspace: &str,
    ) {
        for (_, baseline) in self.lua_editor_option_baselines.drain() {
            let Some(route) = self.router.routes.get_mut(&baseline.window_id) else {
                continue;
            };
            let Some(item) = route
                .window
                .screen
                .context_manager
                .get_by_route_id(baseline.route_id)
            else {
                continue;
            };
            let Some(code) = item.context_mut().code.as_mut() else {
                continue;
            };
            code.wrap = baseline.wrap;
            code.input_mode = baseline.input_mode;
            code.buffer.indent = baseline.indent;
        }
        let snapshot = crate::plugin_manager::overlay_snapshot(
            self.lua_plugins.snapshot(),
            self.lua_runtime
                .as_ref()
                .map(neoism_lua::LuaRuntime::snapshot),
        );
        let mut options = snapshot
            .editor_options
            .into_iter()
            .filter(|option| {
                if !self.lua_active_owners.contains(&option.owner) {
                    return false;
                }
                let current = match option.scope {
                    neoism_lua::PluginStateScope::Document => document,
                    neoism_lua::PluginStateScope::Pane => Some(pane),
                    neoism_lua::PluginStateScope::Tab => Some(tab),
                    neoism_lua::PluginStateScope::Workspace => Some(workspace),
                    neoism_lua::PluginStateScope::Plugin => None,
                };
                current.is_some_and(|current| {
                    option
                        .target
                        .as_deref()
                        .is_none_or(|target| target == current)
                })
            })
            .collect::<Vec<_>>();
        options.sort_by_key(|option| {
            let specificity = match option.scope {
                neoism_lua::PluginStateScope::Workspace => 0,
                neoism_lua::PluginStateScope::Tab => 1,
                neoism_lua::PluginStateScope::Pane => 2,
                neoism_lua::PluginStateScope::Document => 3,
                neoism_lua::PluginStateScope::Plugin => 0,
            };
            (specificity, option.priority, option.order)
        });
        if options.is_empty() {
            return;
        }
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let Some(item) = route
            .window
            .screen
            .context_manager
            .get_by_route_id(route_id)
        else {
            return;
        };
        let Some(code) = item.context_mut().code.as_mut() else {
            return;
        };
        self.lua_editor_option_baselines.insert(
            pane.to_owned(),
            LuaEditorOptionBaseline {
                window_id,
                route_id,
                wrap: code.wrap,
                input_mode: code.input_mode,
                indent: code.buffer.indent,
            },
        );
        for option in options {
            match option.name {
                neoism_lua::EditorOptionName::Wrap => {
                    code.wrap = option.value.as_bool().unwrap_or(code.wrap)
                }
                neoism_lua::EditorOptionName::TabWidth => {
                    code.buffer.indent.width = option
                        .value
                        .as_u64()
                        .unwrap_or(code.buffer.indent.width as u64)
                        as usize;
                }
                neoism_lua::EditorOptionName::UseTabs => {
                    code.buffer.indent.use_tabs = option
                        .value
                        .as_bool()
                        .unwrap_or(code.buffer.indent.use_tabs);
                }
                neoism_lua::EditorOptionName::InputMode => {
                    code.input_mode = if option.value.as_str() == Some("vim") {
                        neoism_ui::editor::code::CodeInputMode::Vim
                    } else {
                        neoism_ui::editor::code::CodeInputMode::Standard
                    };
                }
            }
        }
    }

    fn poll_lua_timers(&mut self, now: Instant) {
        let active = self
            .lua_plugins
            .active_owners()
            .cloned()
            .chain(
                self.lua_runtime
                    .as_ref()
                    .map(|runtime| runtime.owner().clone()),
            )
            .collect::<HashSet<_>>();
        if let Some(jobs) = self.lua_jobs.as_mut() {
            jobs.retire_inactive(&active);
        }
        if let Some(watchers) = self.lua_watchers.as_mut() {
            watchers.retire_inactive(&active);
        }
        if let Some(ptys) = self.lua_ptys.as_mut() {
            ptys.retire_inactive(&active);
        }
        if let Some(dap) = self.lua_dap.as_mut() {
            dap.retire_inactive(&active);
        }
        if let Some(progress) = self.lua_progress.as_mut() {
            progress.retain(|_, (owner, _)| active.contains(owner));
        }
        if let Some(lists) = self.lua_result_lists.as_mut() {
            lists.retain(|_, list| active.contains(&list.owner));
        }
        if let Some(prompts) = self.lua_prompts.as_mut() {
            let stale_windows = prompts
                .iter()
                .filter(|(_, owner)| !active.contains(owner))
                .map(|((window, _), _)| *window)
                .collect::<HashSet<_>>();
            prompts.retain(|_, owner| active.contains(owner));
            for window in stale_windows {
                if let Some(route) = self.router.routes.get_mut(&window) {
                    route.window.screen.renderer.modal.close();
                }
            }
        }
        let Some(timers) = self.lua_timers.as_mut() else {
            return;
        };
        timers.retain(|_, timer| active.contains(&timer.owner));
        let started = Instant::now();
        let due = timers
            .iter()
            .filter(|(_, timer)| timer.due <= now)
            .take(64)
            .map(|(id, timer)| (id.clone(), timer.clone()))
            .collect::<Vec<_>>();
        let _ = timers;
        for (id, timer) in due {
            if started.elapsed() >= Duration::from_millis(8) {
                break;
            }
            let command = self
                .lua_runtime
                .as_ref()
                .filter(|runtime| runtime.owner() == &timer.owner)
                .and_then(|runtime| {
                    runtime.snapshot().commands.iter().find(|command| {
                        command.id == timer.command
                            || command.aliases.iter().any(|alias| alias == &timer.command)
                    })
                })
                .cloned()
                .or_else(|| {
                    self.lua_plugins
                        .command_contribution(&timer.owner, &timer.command)
                });
            if let Some(command) = command {
                let request = neoism_lua::PluginCommandRequest {
                    command: timer.command.clone(),
                    arguments: timer.arguments.clone(),
                    range: None,
                    count: None,
                    bang: false,
                };
                if neoism_lua::validate_command_request(&command, &request).is_ok() {
                    let event = neoism_lua::PluginEvent::new(
                        neoism_lua::PluginEventKind::Command,
                        serde_json::json!({ "id": command.id, "arguments": timer.arguments, "timer": id }),
                        neoism_lua::ExecutionScope::Local,
                        Some("plugin-timer".into()),
                    ).expect("registered command event");
                    let result = if command.callback.starts_with("lua:neoism.user-init@")
                    {
                        self.lua_runtime
                            .as_ref()
                            .expect("user timer runtime")
                            .invoke(&command.callback, event)
                            .map_err(|error| error.to_string())
                    } else {
                        self.lua_plugins
                            .invoke(&command.callback, event)
                            .map_err(|error| error.to_string())
                    };
                    match result {
                        Ok(value) => {
                            if let Err(error) = neoism_lua::validate_command_arguments(
                                &command.result_schema,
                                &value,
                            ) {
                                tracing::warn!(%error, "Lua timer command returned an invalid structured result");
                            }
                        }
                        Err(error) => tracing::warn!(%error, "Lua timer callback failed"),
                    }
                }
            }
            if let Some(interval) = timer.interval {
                if let Some(live) = self
                    .lua_timers
                    .as_mut()
                    .and_then(|timers| timers.get_mut(&id))
                {
                    live.due = advance_lua_timer_deadline(live.due, interval, now);
                }
            } else {
                if let Some(timers) = self.lua_timers.as_mut() {
                    timers.remove(&id);
                }
            }
        }
    }

    fn poll_lua_async(&mut self) {
        let active = self
            .lua_plugins
            .active_owners()
            .cloned()
            .chain(
                self.lua_runtime
                    .as_ref()
                    .map(|runtime| runtime.owner().clone()),
            )
            .collect::<HashSet<_>>();
        let Some(coordinator) = self.lua_async.as_mut() else {
            return;
        };
        coordinator.retire_inactive(&active);
        let deliveries = coordinator.drain();
        for delivery in deliveries {
            if !active.contains(&delivery.owner)
                || !self.router.routes.contains_key(&delivery.window_id)
            {
                continue;
            }
            if neoism_lua::validate_async_result(
                neoism_lua::AsyncResultKind::Host,
                &delivery.owner,
                delivery
                    .payload
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default(),
            )
            .is_err()
            {
                continue;
            }
            let event = neoism_lua::PluginEvent::new(
                neoism_lua::PluginEventKind::AsyncResult,
                delivery.payload,
                neoism_lua::ExecutionScope::Local,
                Some("desktop-host-service".into()),
            )
            .expect("registered async host result event");
            let changed = if self
                .lua_runtime
                .as_ref()
                .is_some_and(|runtime| runtime.owner() == &delivery.owner)
            {
                let before = self
                    .lua_runtime
                    .as_ref()
                    .map(|runtime| runtime.snapshot().clone());
                if let Err(error) = self
                    .lua_runtime
                    .as_mut()
                    .expect("matched user runtime")
                    .emit(event)
                {
                    tracing::warn!(plugin = %delivery.owner.plugin_id, %error, "Lua AsyncResult autocmd failed");
                }
                self.lua_runtime
                    .as_ref()
                    .map(neoism_lua::LuaRuntime::snapshot)
                    != before.as_ref()
            } else {
                self.lua_plugins
                    .emit_to_owner(&delivery.owner, event)
                    .unwrap_or(false)
            };
            if changed {
                self.sync_lua_snapshot(Some(delivery.window_id));
            }
        }
    }

    fn sync_lua_snapshot(&mut self, window_id: Option<WindowId>) {
        let mut snapshot = crate::plugin_manager::overlay_snapshot(
            self.lua_plugins.snapshot(),
            self.lua_runtime
                .as_ref()
                .map(neoism_lua::LuaRuntime::snapshot),
        );
        let window_id = window_id.or_else(|| self.router.get_focused_route());
        if let Some(window_id) = window_id {
            if let Some(route) = self.router.routes.get(&window_id) {
                for panel in &mut snapshot.panels {
                    if let Some(current) = route
                        .window
                        .screen
                        .renderer
                        .plugins
                        .panels
                        .iter()
                        .find(|current| current.id == panel.id)
                    {
                        panel.visible = current.visible;
                    }
                }
            }
        }
        self.router.set_plugin_snapshot(Arc::new(snapshot));
        self.refresh_lua_extension_rows();
    }

    fn refresh_lua_extension_rows(&mut self) {
        use crate::plugin_manager::LuaPluginLifecycle as ManagerLifecycle;
        use neoism_ui::panels::extensions_page::{
            ExtensionEntry, ExtensionKind, ExtensionStatus, LuaPluginAction,
            LuaPluginLifecycle as UiLifecycle, LuaPluginPresentation,
        };

        let inventory = self.lua_plugins.inventory(&self.config.plugins);
        for error in &inventory.global_errors {
            tracing::warn!(%error, "Lua plugin inventory scan failed");
        }
        let entries = inventory
            .entries
            .into_values()
            .map(|row| {
                let mut lifecycle = match row.lifecycle {
                    ManagerLifecycle::Discovered => UiLifecycle::Discovered,
                    ManagerLifecycle::Lazy => UiLifecycle::Lazy,
                    ManagerLifecycle::Loaded => UiLifecycle::Loaded,
                    ManagerLifecycle::Disabled => UiLifecycle::Disabled,
                    ManagerLifecycle::MashupExcluded => UiLifecycle::Disabled,
                    ManagerLifecycle::UpdateAvailable => UiLifecycle::UpdateAvailable,
                    ManagerLifecycle::PermissionRequired => {
                        UiLifecycle::PermissionRequired
                    }
                    ManagerLifecycle::Approved => UiLifecycle::Approved,
                    ManagerLifecycle::Revoked => UiLifecycle::Revoked,
                    ManagerLifecycle::Incompatible => UiLifecycle::Incompatible,
                    ManagerLifecycle::Blocked => UiLifecycle::Blocked,
                    ManagerLifecycle::Failed => UiLifecycle::Failed,
                    ManagerLifecycle::RestoreRequired => UiLifecycle::RestoreRequired,
                };
                let managed = row.repository_url.is_some();
                let mut status_text = row.status_text.clone();
                let mut retryable = matches!(lifecycle, UiLifecycle::Failed);
                if let Some(job) = self.lua_plugin_jobs.view(&row.id) {
                    use crate::lua_plugin_jobs::LuaPluginJobKind;
                    lifecycle = match job.kind {
                        LuaPluginJobKind::Installing => UiLifecycle::Installing,
                        LuaPluginJobKind::Updating => UiLifecycle::Updating,
                        LuaPluginJobKind::Restoring => UiLifecycle::Restoring,
                        LuaPluginJobKind::Removing => UiLifecycle::Removing,
                        LuaPluginJobKind::Failed => UiLifecycle::Failed,
                    };
                    status_text = job.status_text.clone();
                    retryable = job.retryable;
                }
                let primary_action = match lifecycle {
                    UiLifecycle::Discovered if managed => Some(LuaPluginAction::Install),
                    UiLifecycle::Lazy | UiLifecycle::Loaded => {
                        Some(LuaPluginAction::Disable)
                    }
                    UiLifecycle::Disabled if !row.mashup_controlled => {
                        Some(LuaPluginAction::Enable)
                    }
                    UiLifecycle::UpdateAvailable => Some(LuaPluginAction::Update),
                    UiLifecycle::PermissionRequired => Some(LuaPluginAction::GrantAll),
                    UiLifecycle::Approved => Some(LuaPluginAction::Disable),
                    UiLifecycle::Revoked => Some(LuaPluginAction::GrantAll),
                    UiLifecycle::Failed if retryable => Some(LuaPluginAction::Retry),
                    UiLifecycle::RestoreRequired => Some(LuaPluginAction::Restore),
                    _ => None,
                };
                let mut secondary_actions = Vec::new();
                if managed
                    && !matches!(
                        lifecycle,
                        UiLifecycle::Discovered | UiLifecycle::RestoreRequired
                    )
                {
                    secondary_actions.push(LuaPluginAction::Update);
                    secondary_actions.push(LuaPluginAction::Remove);
                }
                if row.grants.len() > 0 {
                    secondary_actions.push(LuaPluginAction::RevokeAll);
                }
                let status = match lifecycle {
                    UiLifecycle::Discovered => ExtensionStatus::NotInstalled,
                    UiLifecycle::Incompatible | UiLifecycle::Blocked => {
                        ExtensionStatus::Unavailable
                    }
                    UiLifecycle::Failed => ExtensionStatus::Failed {
                        message: status_text.clone(),
                    },
                    UiLifecycle::Installing
                    | UiLifecycle::Updating
                    | UiLifecycle::Restoring => ExtensionStatus::Installing {
                        percent: None,
                        status_text: status_text.clone(),
                    },
                    UiLifecycle::Removing => ExtensionStatus::Uninstalling,
                    _ => ExtensionStatus::Installed {
                        version: row.version.clone(),
                    },
                };
                ExtensionEntry {
                    kind: ExtensionKind::LuaPlugin,
                    id: row.id,
                    name: row.name,
                    version: row.version,
                    description: status_text.clone(),
                    author: "Lua plugin".into(),
                    downloads: None,
                    categories: vec!["Lua Plugin".into()],
                    languages: Vec::new(),
                    status,
                    repository_url: row.repository_url,
                    lsp_source: None,
                    lua_plugin: Some(LuaPluginPresentation {
                        lifecycle,
                        status_text,
                        requested_permissions: row.capabilities,
                        granted_permissions: row.grants,
                        missing_permissions: row.missing_permissions,
                        installed_commit: row.installed_commit,
                        retryable,
                        primary_action,
                        secondary_actions,
                    }),
                }
            })
            .collect::<Vec<_>>();
        for route in self.router.routes.values_mut() {
            route.window.screen.renderer.lua_plugin_job_active =
                self.lua_plugin_jobs.is_active();
            route.window.screen.set_lua_plugin_entries(entries.clone());
            route.request_redraw();
        }
    }

    fn handle_lua_plugin_action(
        &mut self,
        window_id: WindowId,
        plugin_id: String,
        action: neoism_ui::panels::extensions_page::LuaPluginAction,
    ) {
        use neoism_ui::panels::extensions_page::LuaPluginAction;
        let inventory = self.lua_plugins.inventory(&self.config.plugins);
        let row = inventory.entries.get(&plugin_id).cloned();
        match action {
            LuaPluginAction::Install | LuaPluginAction::Update => {
                let Some(row) = row else { return };
                let (Some(repository_url), Some(requested_ref)) =
                    (row.repository_url, row.requested_ref)
                else {
                    return;
                };
                let spec = neoism_extensions::lua_plugins::PluginSpec {
                    plugin_id: plugin_id.clone(),
                    repository_url,
                    requested_ref,
                };
                let operation = if action == LuaPluginAction::Install {
                    crate::lua_plugin_jobs::LuaPluginOperation::Install(spec)
                } else {
                    crate::lua_plugin_jobs::LuaPluginOperation::Update(spec)
                };
                self.lua_plugin_jobs
                    .enqueue(window_id, plugin_id, operation);
            }
            LuaPluginAction::Restore => self.lua_plugin_jobs.enqueue(
                window_id,
                plugin_id,
                crate::lua_plugin_jobs::LuaPluginOperation::Restore,
            ),
            LuaPluginAction::Remove => {
                let dependents = self.lua_plugins.enabled_dependents(&plugin_id);
                if dependents.is_empty() {
                    self.lua_plugin_jobs.enqueue(
                        window_id,
                        plugin_id,
                        crate::lua_plugin_jobs::LuaPluginOperation::Remove,
                    );
                } else {
                    self.lua_plugin_jobs.record_failure(
                        plugin_id,
                        format!(
                            "Required by enabled plugin(s): {}",
                            dependents.join(", ")
                        ),
                    );
                }
            }
            LuaPluginAction::Retry => {
                if !self.lua_plugin_jobs.retry(window_id, &plugin_id) {
                    let candidate =
                        crate::plugin_manager::resolve_mashup_selection(&self.config)
                            .and_then(|selection| {
                                crate::plugin_manager::LuaPluginManager::discover(
                                    &neoism_backend::config::config_dir_path(),
                                    self.lua_host.clone(),
                                    &self.config.plugins,
                                    selection.as_ref(),
                                )
                            });
                    match candidate {
                        Ok(manager) => {
                            self.lua_plugins = manager;
                            self.sync_lua_snapshot(Some(window_id));
                        }
                        Err(error) => self
                            .lua_plugin_jobs
                            .record_failure(plugin_id, error.to_string()),
                    }
                }
            }
            LuaPluginAction::Enable | LuaPluginAction::Disable => {
                let mut policy = self.config.plugins.clone();
                policy.disabled.retain(|id| id != &plugin_id);
                if action == LuaPluginAction::Disable {
                    policy.disabled.push(plugin_id);
                    policy.disabled.sort();
                    policy.disabled.dedup();
                }
                self.apply_lua_plugin_policy(window_id, policy, "plugins.disabled");
            }
            LuaPluginAction::GrantAll | LuaPluginAction::RevokeAll => {
                if let Some(root) = row.as_ref().and_then(|row| row.root.as_deref()) {
                    if let Ok(discovered) = neoism_lua::read_plugin_manifest(root) {
                        let mut artifacts = discovered
                            .manifest
                            .entrypoints
                            .native
                            .iter()
                            .cloned()
                            .collect::<Vec<_>>();
                        artifacts.extend(
                            self.lua_plugins.snapshot().platform.iter().filter_map(
                                |owned| {
                                    if owned.owner.plugin_id != plugin_id {
                                        return None;
                                    }
                                    match &owned.contribution {
                                        neoism_lua::PlatformContribution::TreeSitter(
                                            parser,
                                        ) => Some(parser.parser.clone()),
                                        _ => None,
                                    }
                                },
                            ),
                        );
                        let trust_result = (|| -> Result<(), String> {
                            let revision = neoism_lua::plugin_content_revision(
                                root,
                                discovered.manifest.editor_entrypoint(),
                            )
                            .map_err(|e| e.to_string())?;
                            let capabilities = discovered
                                .manifest
                                .capabilities
                                .iter()
                                .map(|value| value.key())
                                .collect::<std::collections::BTreeSet<_>>();
                            let owner = neoism_lua::PluginOwner {
                                plugin_id: plugin_id.clone(),
                                revision: revision.clone(),
                            };
                            if action == LuaPluginAction::RevokeAll {
                                crate::credential_broker::broker()
                                    .revoke_owner(&owner)?;
                            } else {
                                for alias in capabilities
                                    .iter()
                                    .filter_map(|value| value.strip_prefix("credential:"))
                                {
                                    crate::credential_broker::broker().grant(
                                        owner.clone(),
                                        alias,
                                        crate::credential_broker::CredentialScope::User,
                                        std::collections::BTreeSet::from([
                                            "network.authorize".into(),
                                        ]),
                                    )?;
                                }
                            }
                            if !artifacts.is_empty() {
                                let package_digest =
                                    neoism_extensions::lua_plugins::tree_sha256(root)
                                        .map_err(|e| e.to_string())?;
                                let store = neoism_extensions::trust::ExtensionTrustStore::managed();
                                for artifact in artifacts {
                                    let approval = neoism_extensions::trust::ExtensionApproval {
                                        plugin_id: plugin_id.clone(), revision: revision.0.clone(), package_digest: package_digest.clone(),
                                        artifact_digest: artifact.sha256, abi_version: artifact.abi,
                                        capabilities: capabilities.clone(), scope: neoism_extensions::trust::ApprovalScope::User,
                                        state: neoism_extensions::trust::ApprovalState::PermissionRequired,
                                        decided_at_millis: 0, decided_by: "native-extensions-ui".into(), reason: None,
                                    };
                                    if action == LuaPluginAction::GrantAll {
                                        store.approve(approval)
                                    } else {
                                        store.revoke(
                                            approval,
                                            Some("revoked from Extensions UI".into()),
                                        )
                                    }
                                    .map_err(|e| e.to_string())?;
                                }
                            }
                            Ok(())
                        })();
                        if let Err(error) = trust_result {
                            self.lua_plugin_jobs
                                .record_failure(plugin_id.clone(), error);
                        }
                    }
                }
                let mut policy = self.config.plugins.clone();
                if action == LuaPluginAction::GrantAll {
                    let grants = row.map(|row| row.capabilities).unwrap_or_default();
                    policy.grants.insert(plugin_id, grants);
                } else {
                    policy.grants.remove(&plugin_id);
                }
                self.apply_lua_plugin_policy(window_id, policy, "plugins.grants");
            }
        }
        self.refresh_lua_extension_rows();
    }

    fn apply_lua_plugin_policy(
        &mut self,
        window_id: WindowId,
        policy: neoism_backend::config::PluginPreferences,
        key: &str,
    ) {
        let mut candidate_config = self.config.clone();
        candidate_config.plugins = policy.clone();
        let candidate =
            match crate::plugin_manager::resolve_mashup_selection(&candidate_config)
                .and_then(|selection| {
                    crate::plugin_manager::LuaPluginManager::discover(
                        &neoism_backend::config::config_dir_path(),
                        self.lua_host.clone(),
                        &policy,
                        selection.as_ref(),
                    )
                }) {
                Ok(candidate) => candidate,
                Err(error) => {
                    tracing::warn!(%error, "Lua plugin policy change rejected");
                    return;
                }
            };
        let value = if key == "plugins.disabled" {
            serde_json::to_value(&policy.disabled)
        } else {
            serde_json::to_value(&policy.grants)
        };
        let Ok(value) = value else { return };
        if let Err(error) = neoism_backend::config::write_setting(key, value) {
            tracing::warn!(%error, key, "failed to persist Lua plugin policy");
            return;
        }
        self.config.plugins = policy;
        self.lua_plugins = candidate;
        self.sync_lua_snapshot(Some(window_id));
    }

    fn invoke_lua_owned_command(
        &mut self,
        owner: &neoism_lua::PluginOwner,
        id: &str,
        arguments: serde_json::Value,
    ) -> Result<serde_json::Value, String> {
        let command = self
            .lua_runtime
            .as_ref()
            .filter(|runtime| runtime.owner() == owner)
            .and_then(|runtime| {
                runtime
                    .snapshot()
                    .commands
                    .iter()
                    .find(|command| {
                        command.id == id
                            || command.aliases.iter().any(|alias| alias == id)
                    })
                    .cloned()
            })
            .or_else(|| self.lua_plugins.command_contribution(owner, id))
            .ok_or_else(|| {
                format!("plugin command `{id}` is not registered by the exact owner")
            })?;
        let event = neoism_lua::PluginEvent::new(
            neoism_lua::PluginEventKind::Command,
            serde_json::json!({ "id": command.id, "arguments": arguments }),
            neoism_lua::ExecutionScope::Local,
            Some("plugin-adapter".into()),
        )
        .map_err(|error| error.to_string())?;
        let value = if self
            .lua_runtime
            .as_ref()
            .is_some_and(|runtime| runtime.owner() == owner)
        {
            self.lua_runtime
                .as_mut()
                .expect("matched user runtime")
                .invoke(&command.callback, event)
                .map_err(|error| error.to_string())?
        } else {
            self.lua_plugins
                .invoke(&command.callback, event)
                .map_err(|error| error.to_string())?
        };
        neoism_lua::validate_command_arguments(&command.result_schema, &value)?;
        Ok(value)
    }

    fn apply_lua_action(&mut self, window_id: WindowId, action: neoism_lua::HostAction) {
        use neoism_ui::panels::command_palette::PaletteAction;

        let argument = |name: &str| {
            action.arguments.as_str().map(str::to_owned).or_else(|| {
                action
                    .arguments
                    .get(name)
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_owned)
            })
        };
        let contract = match neoism_lua::action_contract(&action) {
            Ok(contract) => contract,
            Err(error) => {
                tracing::warn!(%error, "rejected unregistered Lua host action");
                return;
            }
        };
        let owner = match neoism_lua::require_exact_owner(&action) {
            Ok(owner) => owner,
            Err(error) => {
                tracing::warn!(%error, "rejected unowned Lua host action");
                return;
            }
        };
        // Candidate VMs may enqueue during initialization. Only the exact
        // revision that survived candidate validation and publication may
        // mutate host state; rejected/retired generations are inert.
        if !self.lua_lsp_owner_is_active(owner) {
            tracing::debug!(plugin = %owner.plugin_id, revision = %owner.revision.0, "discarded stale Lua host action");
            return;
        }
        if contract.operation == neoism_lua::HostOperation::EffectEmit {
            let kind = action
                .arguments
                .get("kind")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if kind != "particles" {
                tracing::warn!(plugin = %owner.plugin_id, %kind, "rejected unknown Lua visual effect");
                return;
            }
            let spec = match serde_json::from_value::<
                neoism_ui::panels::agent_pane::view::fx::ParticleEffectSpec,
            >(action.arguments.clone())
            {
                Ok(spec) => spec,
                Err(error) => {
                    tracing::warn!(plugin = %owner.plugin_id, %error, "rejected malformed Lua particle effect");
                    return;
                }
            };
            if let Err(error) = spec.validate() {
                tracing::warn!(plugin = %owner.plugin_id, %error, "rejected unsafe Lua particle effect");
                return;
            }
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                if let Some(agent) = route
                    .window
                    .screen
                    .context_manager
                    .current_mut()
                    .neoism_agent
                    .as_mut()
                {
                    agent.queue_plugin_particle(spec);
                    route.request_redraw();
                }
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::AsyncCancel {
            let Ok(request) = serde_json::from_value::<
                neoism_lua::PluginAsyncCancelRequest,
            >(action.arguments.clone()) else {
                tracing::warn!(plugin = %owner.plugin_id, "invalid Lua async cancellation request");
                return;
            };
            if let Some(watchers) = self.lua_watchers.as_mut() {
                watchers.cancel(owner, &request.id);
            }
            let prompt_windows = self
                .lua_prompts
                .as_ref()
                .into_iter()
                .flat_map(|prompts| prompts.iter())
                .filter_map(|((window, id), candidate)| {
                    (id == &request.id && candidate == owner).then_some(*window)
                })
                .collect::<Vec<_>>();
            if let Some(prompts) = self.lua_prompts.as_mut() {
                prompts
                    .retain(|(_, id), candidate| id != &request.id || candidate != owner);
            }
            for window in prompt_windows {
                if let Some(route) = self.router.routes.get_mut(&window) {
                    route.window.screen.renderer.modal.close();
                }
            }
            if !self
                .lua_async
                .as_mut()
                .is_some_and(|coordinator| coordinator.cancel(owner, &request.id))
            {
                tracing::debug!(plugin = %owner.plugin_id, request = %request.id, "ignored stale or cross-owner async cancellation");
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::ClipboardRead {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if let Err(error) = coordinator.register(
                owner.clone(),
                request_id.clone(),
                window_id,
                "clipboard",
            ) {
                tracing::warn!(plugin = %owner.plugin_id, %error, "Lua clipboard read rejected");
                return;
            }
            let text = self
                .router
                .clipboard
                .get(neoism_backend::clipboard::ClipboardType::Clipboard);
            coordinator.sender().complete(
                owner.clone(),
                request_id,
                serde_json::json!({ "text": text }),
            );
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::JobSpawn
                | neoism_lua::HostOperation::JobStdin
                | neoism_lua::HostOperation::JobCloseStdin
                | neoism_lua::HostOperation::JobCancel
        ) {
            match contract.operation {
                neoism_lua::HostOperation::JobSpawn => {
                    let Some(request_id) = action.invocation_id.clone() else {
                        return;
                    };
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginJobSpawnRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid managed job request");
                            return;
                        }
                    };
                    let Some(root) = self
                        .router
                        .routes
                        .get(&window_id)
                        .and_then(|route| {
                            route.window.screen.local_plugin_workspace_root()
                        })
                        .map(std::path::Path::to_path_buf)
                    else {
                        tracing::warn!(plugin = %owner.plugin_id, "managed jobs require a local workspace root");
                        return;
                    };
                    let coordinator = self.lua_async.get_or_insert_with(Default::default);
                    let token = match coordinator.register(
                        owner.clone(),
                        request_id.clone(),
                        window_id,
                        "job",
                    ) {
                        Ok(token) => token,
                        Err(error) => {
                            tracing::warn!(plugin = %owner.plugin_id, %error, "managed job rejected");
                            return;
                        }
                    };
                    let sender = coordinator.sender();
                    let jobs = self.lua_jobs.get_or_insert_with(Default::default);
                    if let Err(error) = jobs.spawn(
                        owner.clone(),
                        request_id.clone(),
                        window_id,
                        &root,
                        request,
                        token,
                        sender.clone(),
                        self.event_proxy.clone(),
                    ) {
                        sender.fail(owner.clone(), request_id, "spawn_rejected", error);
                        self.event_proxy.send_event(
                            neoism_backend::event::RioEventType::Rio(
                                neoism_backend::event::RioEvent::Render,
                            ),
                            window_id,
                        );
                    }
                }
                neoism_lua::HostOperation::JobStdin
                | neoism_lua::HostOperation::JobCloseStdin => {
                    let Ok(request) = serde_json::from_value::<
                        neoism_lua::PluginJobControlRequest,
                    >(action.arguments.clone()) else {
                        return;
                    };
                    let Some(jobs) = self.lua_jobs.as_mut() else {
                        return;
                    };
                    if contract.operation == neoism_lua::HostOperation::JobStdin {
                        if let Err(error) = jobs.stdin(
                            owner,
                            &request.job,
                            request.data.as_deref().unwrap_or_default(),
                        ) {
                            tracing::warn!(plugin = %owner.plugin_id, %error, "managed job stdin rejected");
                        }
                    } else if !jobs.close_stdin(owner, &request.job) {
                        tracing::warn!(plugin = %owner.plugin_id, "managed job close_stdin rejected stale handle");
                    }
                }
                neoism_lua::HostOperation::JobCancel => {
                    let Ok(request) = serde_json::from_value::<
                        neoism_lua::PluginJobControlRequest,
                    >(action.arguments.clone()) else {
                        return;
                    };
                    if !self.lua_async.as_mut().is_some_and(|coordinator| {
                        coordinator.cancel(owner, &request.job)
                    }) {
                        tracing::debug!(plugin = %owner.plugin_id, "managed job cancellation rejected stale handle");
                    }
                }
                _ => unreachable!(),
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::TaskRun
                | neoism_lua::HostOperation::TestRun
                | neoism_lua::HostOperation::TaskCancel
                | neoism_lua::HostOperation::TestCancel
        ) {
            if matches!(
                contract.operation,
                neoism_lua::HostOperation::TaskCancel
                    | neoism_lua::HostOperation::TestCancel
            ) {
                let id = action
                    .arguments
                    .get("id")
                    .or_else(|| action.arguments.get("job"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if !self
                    .lua_async
                    .as_mut()
                    .is_some_and(|coordinator| coordinator.cancel(owner, id))
                {
                    tracing::debug!(plugin = %owner.plugin_id, "task/test cancellation rejected stale handle");
                }
                return;
            }
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let request_value = action
                .arguments
                .get("process")
                .cloned()
                .unwrap_or_else(|| action.arguments.clone());
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginJobSpawnRequest>(
                request_value,
            ) else {
                return;
            };
            let Some(root) = self
                .router
                .routes
                .get(&window_id)
                .and_then(|route| route.window.screen.local_plugin_workspace_root())
                .map(std::path::Path::to_path_buf)
            else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            let kind = if contract.operation == neoism_lua::HostOperation::TaskRun {
                "task"
            } else {
                "test"
            };
            let token = match coordinator.register(
                owner.clone(),
                request_id.clone(),
                window_id,
                kind,
            ) {
                Ok(token) => token,
                Err(_) => return,
            };
            let sender = coordinator.sender();
            if let Err(error) = self.lua_jobs.get_or_insert_with(Default::default).spawn(
                owner.clone(),
                request_id.clone(),
                window_id,
                &root,
                request,
                token,
                sender.clone(),
                self.event_proxy.clone(),
            ) {
                sender.fail(owner.clone(), request_id, "spawn_rejected", error);
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::AgentQuery
                | neoism_lua::HostOperation::AgentMutation
        ) {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if coordinator
                .register(owner.clone(), request_id.clone(), window_id, "agent_bridge")
                .is_err()
            {
                return;
            }
            let sender = coordinator.sender();
            if contract.operation == neoism_lua::HostOperation::AgentMutation {
                let requested = action
                    .arguments
                    .get("requestId")
                    .or_else(|| action.arguments.get("request_id"))
                    .and_then(serde_json::Value::as_str);
                let routed = self
                    .router
                    .routes
                    .get_mut(&window_id)
                    .and_then(|route| {
                        route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .neoism_agent
                            .as_mut()
                    })
                    .and_then(|agent| {
                        agent.pending_permission().map(|permission| {
                            requested.is_none_or(|id| id == permission.id)
                                && !permission.responding
                        })
                    })
                    .unwrap_or(false);
                if routed {
                    // Deliberately do not choose an answer. The existing native
                    // permission card owns the human decision and its normal
                    // Agent reply path; Lua receives only a workflow state.
                    sender.complete(
                        owner.clone(),
                        request_id,
                        serde_json::json!({
                            "state": "permission_required",
                            "routedTo": "native_agent_approval"
                        }),
                    );
                } else {
                    sender.fail(owner.clone(), request_id, "workflow_required", "no matching native Agent approval is pending; open the Agent workflow UI");
                }
                return;
            }
            let result = match action.action.as_str() {
                "status" => self
                    .lua_published
                    .get("agent")
                    .cloned()
                    .ok_or_else(|| "agent state is unavailable".to_string()),
                "sessions" => self
                    .lua_published
                    .get("agent")
                    .and_then(|value| value.get("sessions"))
                    .cloned()
                    .ok_or_else(|| "agent sessions are unavailable".to_string()),
                "messages" => {
                    let requested_session = action
                        .arguments
                        .get("sessionId")
                        .or_else(|| action.arguments.get("session_id"))
                        .and_then(serde_json::Value::as_str);
                    let limit = action
                        .arguments
                        .get("limit")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(200)
                        .clamp(1, 1_000) as usize;
                    self.router.routes.get(&window_id)
                        .and_then(|route| route.window.screen.context_manager.current().neoism_agent.as_ref())
                        .ok_or_else(|| "there is no active agent session".to_string())
                        .and_then(|agent| {
                            if requested_session.is_some_and(|id| Some(id) != agent.session_id_str()) {
                                return Err("only the active native agent session can be queried".to_string());
                            }
                            let messages = agent.messages();
                            let start = messages.len().saturating_sub(limit);
                            Ok(serde_json::Value::Array(messages[start..].iter().map(|message| serde_json::json!({
                                "id": message.id,
                                "kind": format!("{:?}", message.kind).to_ascii_lowercase(),
                                "title": message.title,
                                "text": message.text,
                                "status": message.status,
                                "tool": message.tool,
                                "outputKind": format!("{:?}", message.output_kind).to_ascii_lowercase(),
                                "language": message.lang,
                                "lineOffset": message.line_offset,
                                "detail": message.detail,
                                "author": message.author,
                            })).collect()))
                        })
                }
                "checkpoints" => Err(
                    "native agent checkpoints are not exposed by the current agent pane"
                        .to_string(),
                ),
                _ => Err("unknown agent bridge query".to_string()),
            };
            match result {
                Ok(value) => sender.complete(owner.clone(), request_id, value),
                Err(error) => {
                    sender.fail(owner.clone(), request_id, "agent_query_failed", error)
                }
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::PtyCreate
                | neoism_lua::HostOperation::PtySend
                | neoism_lua::HostOperation::PtyResize
                | neoism_lua::HostOperation::PtyStatus
                | neoism_lua::HostOperation::PtyClose
        ) {
            if contract.operation == neoism_lua::HostOperation::PtyCreate {
                let Some(request_id) = action.invocation_id.clone() else {
                    return;
                };
                let Ok(request) = serde_json::from_value::<
                    neoism_lua::PluginPtyCreateRequest,
                >(action.arguments.clone()) else {
                    return;
                };
                let Some(root) = self
                    .router
                    .routes
                    .get(&window_id)
                    .and_then(|route| route.window.screen.local_plugin_workspace_root())
                    .map(std::path::Path::to_path_buf)
                else {
                    return;
                };
                let coordinator = self.lua_async.get_or_insert_with(Default::default);
                let token = match coordinator.register(
                    owner.clone(),
                    request_id.clone(),
                    window_id,
                    "pty",
                ) {
                    Ok(token) => token,
                    Err(_) => return,
                };
                let sender = coordinator.sender();
                if let Err(error) =
                    self.lua_ptys.get_or_insert_with(Default::default).create(
                        owner.clone(),
                        request_id.clone(),
                        window_id,
                        &root,
                        request,
                        token,
                        sender.clone(),
                        self.event_proxy.clone(),
                    )
                {
                    sender.fail(owner.clone(), request_id, "pty_rejected", error);
                }
                return;
            }
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginPtyControlRequest>(
                action.arguments.clone(),
            ) else {
                return;
            };
            let Some(ptys) = self.lua_ptys.as_mut() else {
                return;
            };
            match contract.operation {
                neoism_lua::HostOperation::PtySend => {
                    if let Err(error) = ptys.write(owner, &request.pty, &request.data) {
                        tracing::warn!(plugin = %owner.plugin_id, %error, "plugin PTY write rejected");
                    }
                }
                neoism_lua::HostOperation::PtyResize => {
                    if let Err(error) = ptys.resize(
                        owner,
                        &request.pty,
                        request.cols.unwrap_or(80),
                        request.rows.unwrap_or(24),
                    ) {
                        tracing::warn!(plugin = %owner.plugin_id, %error, "plugin PTY resize rejected");
                    }
                }
                neoism_lua::HostOperation::PtyClose => {
                    if ptys.close(owner, &request.pty) {
                        if let Some(coordinator) = self.lua_async.as_mut() {
                            coordinator.cancel(owner, &request.pty);
                        }
                    }
                }
                neoism_lua::HostOperation::PtyStatus => {
                    let Some(request_id) = action.invocation_id.clone() else {
                        return;
                    };
                    let result = ptys.status(owner, &request.pty);
                    let coordinator = self.lua_async.get_or_insert_with(Default::default);
                    if coordinator
                        .register(
                            owner.clone(),
                            request_id.clone(),
                            window_id,
                            "pty_status",
                        )
                        .is_ok()
                    {
                        let sender = coordinator.sender();
                        match result {
                            Ok(value) => {
                                sender.complete(owner.clone(), request_id, value)
                            }
                            Err(error) => {
                                sender.fail(owner.clone(), request_id, "stale_pty", error)
                            }
                        }
                    }
                }
                _ => {}
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::DebugStart
                | neoism_lua::HostOperation::DebugControl
                | neoism_lua::HostOperation::DebugStop
        ) {
            if contract.operation == neoism_lua::HostOperation::DebugStart {
                let Some(request_id) = action.invocation_id.clone() else {
                    return;
                };
                let Ok(mut request) = serde_json::from_value::<
                    neoism_lua::PluginDapStartRequest,
                >(action.arguments.clone()) else {
                    return;
                };
                let registration = self.lua_runtime.as_ref().filter(|runtime| runtime.owner() == owner).and_then(|runtime| runtime.snapshot().platform.iter().find_map(|value| match &value.contribution { neoism_lua::PlatformContribution::DebugAdapter(adapter) if adapter.id == request.adapter => Some(adapter.clone()), _ => None }))
                    .or_else(|| self.lua_plugins.snapshot().platform.iter().find_map(|value| if &value.owner == owner { match &value.contribution { neoism_lua::PlatformContribution::DebugAdapter(adapter) if adapter.id == request.adapter => Some(adapter.clone()), _ => None } } else { None }));
                let Some(registration) = registration else {
                    return;
                };
                request.command = registration.command;
                let Some(root) = self
                    .router
                    .routes
                    .get(&window_id)
                    .and_then(|route| route.window.screen.local_plugin_workspace_root())
                    .map(std::path::Path::to_path_buf)
                else {
                    return;
                };
                let coordinator = self.lua_async.get_or_insert_with(Default::default);
                let token = match coordinator.register(
                    owner.clone(),
                    request_id.clone(),
                    window_id,
                    "debug",
                ) {
                    Ok(token) => token,
                    Err(_) => return,
                };
                let sender = coordinator.sender();
                let dap = self.lua_dap.get_or_insert_with(Default::default);
                if let Err(error) = dap.start(
                    owner.clone(),
                    request_id.clone(),
                    window_id,
                    &root,
                    request.clone(),
                    token,
                    sender.clone(),
                    self.event_proxy.clone(),
                ) {
                    sender.fail(owner.clone(), request_id, "debug_start", error);
                    return;
                }
                if let Some(message) = request.initialize {
                    if let Err(error) = dap.send(owner, &request_id, &message) {
                        sender.fail(owner.clone(), request_id, "debug_initialize", error);
                    }
                }
                return;
            }
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginDapControlRequest>(
                action.arguments.clone(),
            ) else {
                return;
            };
            let Some(dap) = self.lua_dap.as_mut() else {
                return;
            };
            if contract.operation == neoism_lua::HostOperation::DebugStop {
                if dap.stop(owner, &request.session) {
                    if let Some(coordinator) = self.lua_async.as_mut() {
                        coordinator.cancel(owner, &request.session);
                    }
                }
            } else if let Err(error) = dap.send(owner, &request.session, &request.message)
            {
                tracing::warn!(plugin = %owner.plugin_id, %error, "debug adapter message rejected");
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::GitQuery | neoism_lua::HostOperation::GitMutation
        ) {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let request = match crate::lua_git::request(&action.action, &action.arguments)
            {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(plugin = %owner.plugin_id, %error, "plugin Git operation rejected");
                    return;
                }
            };
            let Some(root) = self
                .router
                .routes
                .get(&window_id)
                .and_then(|route| route.window.screen.local_plugin_workspace_root())
                .map(std::path::Path::to_path_buf)
            else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            let token = match coordinator.register(
                owner.clone(),
                request_id.clone(),
                window_id,
                "git",
            ) {
                Ok(token) => token,
                Err(_) => return,
            };
            let sender = coordinator.sender();
            if let Err(error) = self.lua_jobs.get_or_insert_with(Default::default).spawn(
                owner.clone(),
                request_id.clone(),
                window_id,
                &root,
                request,
                token,
                sender.clone(),
                self.event_proxy.clone(),
            ) {
                sender.fail(owner.clone(), request_id, "git_spawn", error);
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::WatchCreate
                | neoism_lua::HostOperation::WatchCancel
        ) {
            if contract.operation == neoism_lua::HostOperation::WatchCancel {
                let Ok(request) = serde_json::from_value::<
                    neoism_lua::PluginAsyncCancelRequest,
                >(action.arguments.clone()) else {
                    return;
                };
                if self
                    .lua_watchers
                    .as_mut()
                    .is_some_and(|watchers| watchers.cancel(owner, &request.id))
                {
                    if let Some(coordinator) = self.lua_async.as_mut() {
                        coordinator.cancel(owner, &request.id);
                    }
                }
                return;
            }
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginWatchRequest>(
                action.arguments.clone(),
            ) else {
                return;
            };
            let Some(root) = self
                .router
                .routes
                .get(&window_id)
                .and_then(|route| route.window.screen.local_plugin_workspace_root())
                .map(std::path::Path::to_path_buf)
            else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if coordinator
                .register(owner.clone(), request_id.clone(), window_id, "watcher")
                .is_err()
            {
                return;
            }
            let sender = coordinator.sender();
            let watchers = self.lua_watchers.get_or_insert_with(Default::default);
            if let Err(error) = watchers.watch(
                owner.clone(),
                request_id.clone(),
                window_id,
                &root,
                request,
                sender.clone(),
                self.event_proxy.clone(),
            ) {
                sender.fail(owner.clone(), request_id, "watch_rejected", error);
                self.event_proxy.send_event(
                    neoism_backend::event::RioEventType::Rio(
                        neoism_backend::event::RioEvent::Render,
                    ),
                    window_id,
                );
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::NetworkRequest {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginNetworkRequest>(
                action.arguments.clone(),
            ) else {
                return;
            };
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            let token = match coordinator.register(
                owner.clone(),
                request_id.clone(),
                window_id,
                "network",
            ) {
                Ok(token) => token,
                Err(error) => {
                    tracing::warn!(plugin = %owner.plugin_id, %error, "plugin network request rejected");
                    return;
                }
            };
            let sender = coordinator.sender();
            let workspace = self
                .router
                .routes
                .get(&window_id)
                .and_then(|route| route.window.screen.local_plugin_workspace_root())
                .map(|root| root.to_string_lossy().into_owned());
            if let Err(error) = crate::lua_network::spawn_request(
                owner.clone(),
                request_id.clone(),
                window_id,
                request,
                token,
                sender.clone(),
                self.event_proxy.clone(),
                workspace,
            ) {
                sender.fail(owner.clone(), request_id, "request_rejected", error);
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::CredentialStatus {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let alias = action
                .arguments
                .get("alias")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if coordinator
                .register(owner.clone(), request_id.clone(), window_id, "credential")
                .is_ok()
            {
                let sender = coordinator.sender();
                let workspace = self
                    .router
                    .routes
                    .get(&window_id)
                    .and_then(|route| route.window.screen.local_plugin_workspace_root())
                    .map(|root| root.to_string_lossy().into_owned());
                match crate::lua_network::credential_available(
                    owner,
                    alias,
                    workspace.as_deref(),
                ) {
                    Ok(available) => sender.complete(
                        owner.clone(),
                        request_id,
                        serde_json::json!({ "alias": alias, "available": available }),
                    ),
                    Err(error) => {
                        sender.fail(owner.clone(), request_id, "invalid_alias", error)
                    }
                }
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::CompletionRequest
                | neoism_lua::HostOperation::CompletionResolve
                | neoism_lua::HostOperation::CompletionCancel
        ) {
            if contract.operation == neoism_lua::HostOperation::CompletionCancel {
                let id = action
                    .arguments
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                if let Some(coordinator) = self.lua_async.as_mut() {
                    coordinator.cancel(owner, id);
                }
                return;
            }
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            if contract.operation == neoism_lua::HostOperation::CompletionResolve {
                let command = action
                    .arguments
                    .get("command")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default();
                let Some(candidate) = action.arguments.get("candidate").cloned() else {
                    return;
                };
                let result = self.invoke_lua_owned_command(owner, command, candidate);
                let coordinator = self.lua_async.get_or_insert_with(Default::default);
                if coordinator
                    .register(
                        owner.clone(),
                        request_id.clone(),
                        window_id,
                        "completion_resolve",
                    )
                    .is_ok()
                {
                    let sender = coordinator.sender();
                    match result {
                        Ok(value) => sender.complete(owner.clone(), request_id, value),
                        Err(error) => sender.fail(
                            owner.clone(),
                            request_id,
                            "resolve_failed",
                            error,
                        ),
                    }
                }
                return;
            }
            let Ok(request) = serde_json::from_value::<neoism_lua::PluginCompletionRequest>(
                action.arguments.clone(),
            ) else {
                return;
            };
            let source = self.lua_runtime.as_ref().filter(|runtime| runtime.owner() == owner).and_then(|runtime| runtime.snapshot().platform.iter().find_map(|value| match &value.contribution { neoism_lua::PlatformContribution::CompletionSource(source) if source.id == request.source => Some(source.clone()), _ => None }))
                .or_else(|| self.lua_plugins.snapshot().platform.iter().find_map(|value| if &value.owner == owner { match &value.contribution { neoism_lua::PlatformContribution::CompletionSource(source) if source.id == request.source => Some(source.clone()), _ => None } } else { None }));
            let Some(source) = source else { return };
            let result = self
                .invoke_lua_owned_command(
                    owner,
                    &source.request_command,
                    serde_json::to_value(&request).unwrap_or_default(),
                )
                .and_then(|value| {
                    serde_json::from_value::<Vec<neoism_lua::CompletionCandidate>>(value)
                        .map_err(|error| error.to_string())
                });
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if coordinator
                .register(owner.clone(), request_id.clone(), window_id, "completion")
                .is_err()
            {
                return;
            }
            let sender = coordinator.sender();
            match result {
                Ok(candidates) => {
                    let presented = self
                        .router
                        .routes
                        .get_mut(&window_id)
                        .ok_or_else(|| "completion window is unavailable".to_string())
                        .and_then(|route| {
                            route.window.screen.install_plugin_completions(
                                request.target.revision,
                                request.target.position,
                                candidates.clone(),
                            )
                        });
                    match presented { Ok(()) => sender.complete(owner.clone(), request_id, serde_json::json!({ "count": candidates.len(), "presented": true })), Err(error) => sender.fail(owner.clone(), request_id, "stale_target", error) }
                }
                Err(error) => {
                    sender.fail(owner.clone(), request_id, "source_failed", error)
                }
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::SyntaxQuery {
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let Some(document) = action
                .arguments
                .get("document")
                .and_then(serde_json::Value::as_str)
                .map(|value| neoism_lua::DocumentHandle(value.to_owned()))
            else {
                return;
            };
            let Some((target_window, route_id)) = self
                .lua_editor_resources
                .published_targets
                .get(&document)
                .copied()
            else {
                return;
            };
            let Some(item) =
                self.router
                    .routes
                    .get_mut(&target_window)
                    .and_then(|route| {
                        route
                            .window
                            .screen
                            .context_manager
                            .get_by_route_id(route_id)
                    })
            else {
                return;
            };
            let Some(code) = item.context_mut().code.as_ref() else {
                return;
            };
            let expected_revision = action
                .arguments
                .get("expectedRevision")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX);
            if code.buffer.revision != expected_revision {
                return;
            }
            let text = code.buffer.text();
            let language = action
                .arguments
                .get("language")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let query = action
                .arguments
                .get("query")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default()
                .to_owned();
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            let token = match coordinator.register(
                owner.clone(),
                request_id.clone(),
                window_id,
                "syntax",
            ) {
                Ok(token) => token,
                Err(_) => return,
            };
            let sender = coordinator.sender();
            let proxy = self.event_proxy.clone();
            let owner = owner.clone();
            let _ = std::thread::Builder::new().name("lua-syntax-query".into()).spawn(move || {
                if token.is_cancelled() { return; }
                match neoism_ui::syntax::plugin_query(&language, &text, &query) {
                    Ok(captures) => sender.complete(owner, request_id, serde_json::json!({ "document": document, "revision": expected_revision, "captures": captures })),
                    Err(error) => sender.fail(owner, request_id, "query_failed", error),
                }
                proxy.send_event(neoism_backend::event::RioEventType::Rio(neoism_backend::event::RioEvent::Render), window_id);
            });
            return;
        }
        if contract.operation == neoism_lua::HostOperation::NotificationShow {
            let title = action
                .arguments
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("Plugin");
            let message = action
                .arguments
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if title.len() > 256 || message.len() > 16 * 1024 {
                return;
            }
            let level = match action
                .arguments
                .get("level")
                .and_then(serde_json::Value::as_str)
            {
                Some("error") => {
                    neoism_ui::panels::notifications::NotificationLevel::Error
                }
                Some("warning" | "warn") => {
                    neoism_ui::panels::notifications::NotificationLevel::Warn
                }
                _ => neoism_ui::panels::notifications::NotificationLevel::Info,
            };
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .renderer
                    .notifications
                    .push(format!("{title}: {message}"), level);
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::ProgressCreate
                | neoism_lua::HostOperation::ProgressUpdate
                | neoism_lua::HostOperation::ProgressFinish
        ) {
            let handle = action
                .arguments
                .get("progress")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .or_else(|| action.invocation_id.clone());
            let Some(handle) = handle else { return };
            if contract.operation == neoism_lua::HostOperation::ProgressCreate {
                let progress = self.lua_progress.get_or_insert_with(Default::default);
                if progress
                    .values()
                    .filter(|(candidate, _)| candidate == owner)
                    .count()
                    >= 64
                {
                    return;
                }
                progress.insert(handle.clone(), (owner.clone(), window_id));
            } else if !self.lua_progress.as_ref().is_some_and(|progress| {
                progress.get(&handle).is_some_and(|(candidate, target)| {
                    candidate == owner && *target == window_id
                })
            }) {
                return;
            }
            let message = action
                .arguments
                .get("message")
                .and_then(serde_json::Value::as_str)
                .unwrap_or_default();
            if message.len() <= 16 * 1024 && !message.is_empty() {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.renderer.notifications.push(
                        message,
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                }
            }
            if contract.operation == neoism_lua::HostOperation::ProgressFinish {
                if let Some(progress) = self.lua_progress.as_mut() {
                    progress.remove(&handle);
                }
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::PromptRequest
                | neoism_lua::HostOperation::PromptCancel
        ) {
            if contract.operation == neoism_lua::HostOperation::PromptCancel {
                let Ok(request) = serde_json::from_value::<
                    neoism_lua::PluginAsyncCancelRequest,
                >(action.arguments.clone()) else {
                    return;
                };
                if self
                    .lua_prompts
                    .as_ref()
                    .and_then(|prompts| prompts.get(&(window_id, request.id.clone())))
                    != Some(owner)
                {
                    return;
                }
                if let Some(prompts) = self.lua_prompts.as_mut() {
                    prompts.remove(&(window_id, request.id.clone()));
                }
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.renderer.modal.close();
                }
                if let Some(coordinator) = self.lua_async.as_mut() {
                    coordinator.cancel(owner, &request.id);
                }
                return;
            }
            let Some(request_id) = action.invocation_id.clone() else {
                return;
            };
            let request = match serde_json::from_value::<neoism_lua::PluginPromptRequest>(
                action.arguments.clone(),
            ) {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(%error, "invalid plugin prompt request");
                    return;
                }
            };
            if request.title.len() > 256
                || request.message.len() > 16 * 1024
                || request.options.len() > 32
                || request.options.iter().any(|option| option.len() > 1024)
            {
                return;
            }
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            if route.window.screen.renderer.modal.is_active() {
                return;
            }
            let coordinator = self.lua_async.get_or_insert_with(Default::default);
            if coordinator
                .register(owner.clone(), request_id.clone(), window_id, "prompt")
                .is_err()
            {
                return;
            }
            self.lua_prompts
                .get_or_insert_with(Default::default)
                .insert((window_id, request_id.clone()), owner.clone());
            use neoism_ui::widgets::modal::{
                ModalAction, ModalButton, ModalInputSpec, ModalSpec,
            };
            let reply = |value: String, cancelled| ModalAction::LuaPromptReply {
                request_id: request_id.clone(),
                value,
                cancelled,
            };
            let (input, mut buttons) = match action.action.as_str() {
                "input" => (
                    Some(ModalInputSpec {
                        value: request.default.unwrap_or_default(),
                        placeholder: String::new(),
                    }),
                    vec![ModalButton::new(
                        "Submit",
                        "Enter",
                        reply(String::new(), false),
                    )],
                ),
                "confirm" => (
                    None,
                    vec![ModalButton::new(
                        "Confirm",
                        "Enter",
                        reply("true".into(), false),
                    )],
                ),
                "select" => (
                    None,
                    request
                        .options
                        .into_iter()
                        .enumerate()
                        .map(|(index, option)| {
                            ModalButton::new(
                                option.clone(),
                                (index + 1).to_string(),
                                reply(option, false),
                            )
                        })
                        .collect(),
                ),
                _ => return,
            };
            buttons.push(ModalButton::new(
                "Cancel",
                "Esc",
                reply(String::new(), true),
            ));
            route.window.screen.renderer.modal.open(ModalSpec {
                title: request.title,
                body: request.message,
                meta: owner.plugin_id.clone(),
                input,
                buttons,
                busy: false,
                blocking: true,
            });
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::ResultListCreate
                | neoism_lua::HostOperation::ResultListReplace
                | neoism_lua::HostOperation::ResultListAppend
                | neoism_lua::HostOperation::ResultListClear
                | neoism_lua::HostOperation::ResultListDelete
                | neoism_lua::HostOperation::ResultListOpen
                | neoism_lua::HostOperation::ResultListQuery
        ) {
            let request = match serde_json::from_value::<
                neoism_lua::PluginResultListRequest,
            >(action.arguments.clone())
            {
                Ok(request) => request,
                Err(error) => {
                    tracing::warn!(%error, "invalid plugin result-list request");
                    return;
                }
            };
            if request.entries.len() > 2_000
                || request.title.len() > 256
                || request.kind.len() > 64
                || request.entries.iter().any(|entry| {
                    entry.label.len() > 4_096 || entry.detail.len() > 16 * 1024
                })
            {
                return;
            }
            let handle = request
                .list
                .clone()
                .or_else(|| action.invocation_id.clone());
            let Some(handle) = handle else { return };
            if contract.operation == neoism_lua::HostOperation::ResultListCreate {
                let lists = self.lua_result_lists.get_or_insert_with(Default::default);
                if lists.values().filter(|list| &list.owner == owner).count() >= 128
                    || lists.values().map(|list| list.entries.len()).sum::<usize>()
                        + request.entries.len()
                        > 10_000
                {
                    return;
                }
                lists.insert(
                    handle,
                    LuaResultList {
                        owner: owner.clone(),
                        window_id,
                        title: request.title,
                        kind: request.kind,
                        entries: request.entries,
                    },
                );
                return;
            }
            let valid = self
                .lua_result_lists
                .as_ref()
                .and_then(|lists| lists.get(&handle))
                .is_some_and(|list| &list.owner == owner && list.window_id == window_id);
            if !valid {
                return;
            }
            match contract.operation {
                neoism_lua::HostOperation::ResultListReplace => {
                    if let Some(list) = self
                        .lua_result_lists
                        .as_mut()
                        .and_then(|lists| lists.get_mut(&handle))
                    {
                        list.title = request.title;
                        list.kind = request.kind;
                        list.entries = request.entries;
                    }
                }
                neoism_lua::HostOperation::ResultListAppend => {
                    if let Some(list) = self
                        .lua_result_lists
                        .as_mut()
                        .and_then(|lists| lists.get_mut(&handle))
                    {
                        if list.entries.len() + request.entries.len() <= 2_000 {
                            list.entries.extend(request.entries);
                        }
                    }
                }
                neoism_lua::HostOperation::ResultListClear => {
                    if let Some(list) = self
                        .lua_result_lists
                        .as_mut()
                        .and_then(|lists| lists.get_mut(&handle))
                    {
                        list.entries.clear();
                    }
                }
                neoism_lua::HostOperation::ResultListDelete => {
                    if let Some(lists) = self.lua_result_lists.as_mut() {
                        lists.remove(&handle);
                    }
                }
                neoism_lua::HostOperation::ResultListQuery => {
                    let Some(request_id) = action.invocation_id.clone() else {
                        return;
                    };
                    let Some(list) = self
                        .lua_result_lists
                        .as_ref()
                        .and_then(|lists| lists.get(&handle))
                        .cloned()
                    else {
                        return;
                    };
                    let coordinator = self.lua_async.get_or_insert_with(Default::default);
                    if coordinator
                        .register(
                            owner.clone(),
                            request_id.clone(),
                            window_id,
                            "result_list",
                        )
                        .is_ok()
                    {
                        coordinator.sender().complete(owner.clone(), request_id, serde_json::json!({
                            "list": handle, "title": list.title, "kind": list.kind, "entries": list.entries,
                        }));
                    }
                }
                neoism_lua::HostOperation::ResultListOpen => {
                    let index = request.index.unwrap_or(0);
                    let entry = self
                        .lua_result_lists
                        .as_ref()
                        .and_then(|lists| lists.get(&handle))
                        .and_then(|list| list.entries.get(index))
                        .cloned();
                    let Some(entry) = entry else { return };
                    let Some((target_window, route_id)) = self
                        .lua_editor_resources
                        .published_targets
                        .get(&entry.document)
                        .copied()
                    else {
                        return;
                    };
                    let Some(route) = self.router.routes.get_mut(&target_window) else {
                        return;
                    };
                    let Some(item) = route
                        .window
                        .screen
                        .context_manager
                        .get_by_route_id(route_id)
                    else {
                        return;
                    };
                    let Some(code) = item.context_mut().code.as_mut() else {
                        return;
                    };
                    let line = entry.position.line as usize;
                    let col = entry.position.character as usize;
                    if code.buffer.lines.get(line).is_some_and(|text| {
                        col <= text.len() && text.is_char_boundary(col)
                    }) {
                        code.buffer.cursor_line = line;
                        code.buffer.cursor_col = col;
                    }
                }
                _ => {}
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::CommandExecute
                | neoism_lua::HostOperation::CommandCancel
        ) {
            match contract.operation {
                neoism_lua::HostOperation::CommandExecute => {
                    let Some(request_id) = action.invocation_id.clone() else {
                        tracing::warn!("Lua command request is missing its request id");
                        return;
                    };
                    let mut value = action.arguments.clone();
                    if value.get("command").is_none() {
                        if let Some(id) = value.get("id").cloned() {
                            value["command"] = id;
                        }
                    }
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginCommandRequest,
                    >(value)
                    {
                        Ok(request) => request,
                        Err(error) => {
                            self.lua_command_completions.insert(
                                (owner.clone(), request_id.clone()),
                                neoism_lua::PluginCommandCompletion::failed(
                                    request_id,
                                    "unknown".into(),
                                    "invalid_request",
                                    error.to_string(),
                                ),
                            );
                            return;
                        }
                    };
                    let user_command = self.lua_runtime.as_ref().and_then(|runtime| {
                        runtime
                            .snapshot()
                            .commands
                            .iter()
                            .find(|command| {
                                command.id == request.command
                                    || command
                                        .aliases
                                        .iter()
                                        .any(|alias| alias == &request.command)
                            })
                            .cloned()
                            .map(|command| (runtime.owner().clone(), command))
                    });
                    let resolved = user_command
                        .or_else(|| self.lua_plugins.resolve_command(&request.command));
                    let Some((target_owner, command)) = resolved else {
                        if let Some(native) = lua_palette_action(&request.command) {
                            if request.range.is_some()
                                || request.count.is_some()
                                || request.bang
                            {
                                self.lua_command_completions.insert(
                                    (owner.clone(), request_id.clone()),
                                    neoism_lua::PluginCommandCompletion::failed(
                                        request_id, request.command, "invalid_arguments",
                                        "native compatibility commands do not accept range, count, or bang",
                                    ),
                                );
                                return;
                            }
                            if let Some(route) = self.router.routes.get_mut(&window_id) {
                                route.window.screen.execute_palette_action(
                                    native,
                                    &mut self.router.clipboard,
                                );
                                route.request_redraw();
                            }
                            self.lua_command_completions.insert(
                                (owner.clone(), request_id.clone()),
                                neoism_lua::PluginCommandCompletion::succeeded(
                                    request_id,
                                    request.command,
                                    serde_json::Value::Null,
                                ),
                            );
                            return;
                        }
                        self.lua_command_completions.insert(
                            (owner.clone(), request_id.clone()),
                            neoism_lua::PluginCommandCompletion::failed(
                                request_id,
                                request.command,
                                "unknown_command",
                                "command is not registered",
                            ),
                        );
                        return;
                    };
                    if let Err(error) =
                        neoism_lua::validate_command_request(&command, &request)
                    {
                        self.lua_command_completions.insert(
                            (owner.clone(), request_id.clone()),
                            neoism_lua::PluginCommandCompletion::failed(
                                request_id,
                                command.id,
                                "invalid_arguments",
                                error,
                            ),
                        );
                        return;
                    }
                    let event = neoism_lua::PluginEvent::new(
                        neoism_lua::PluginEventKind::Command,
                        serde_json::json!({
                            "id": command.id,
                            "requestedAs": request.command,
                            "requestId": request_id,
                            "arguments": request.arguments,
                            "range": request.range,
                            "count": request.count,
                            "bang": request.bang,
                        }),
                        command.scope,
                        Some(format!("plugin-command:{}", owner.plugin_id)),
                    )
                    .expect("registered command event");
                    let invoked = if self
                        .lua_runtime
                        .as_ref()
                        .is_some_and(|runtime| runtime.owner() == &target_owner)
                    {
                        self.lua_runtime
                            .as_ref()
                            .expect("matched user runtime")
                            .invoke(&command.callback, event)
                            .map_err(|error| error.to_string())
                    } else {
                        self.lua_plugins
                            .invoke(&command.callback, event)
                            .map_err(|error| error.to_string())
                    };
                    let completion = match invoked {
                        Ok(result) => match neoism_lua::validate_command_arguments(
                            &command.result_schema,
                            &result,
                        ) {
                            Ok(()) => neoism_lua::PluginCommandCompletion::succeeded(
                                request_id.clone(),
                                command.id,
                                result,
                            ),
                            Err(error) => neoism_lua::PluginCommandCompletion::failed(
                                request_id.clone(),
                                command.id,
                                "invalid_result",
                                error,
                            ),
                        },
                        Err(error) => neoism_lua::PluginCommandCompletion::failed(
                            request_id.clone(),
                            command.id,
                            "callback_failed",
                            error,
                        ),
                    };
                    self.lua_command_completions
                        .insert((owner.clone(), request_id), completion);
                }
                neoism_lua::HostOperation::CommandCancel => {
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginCommandCancelRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) if !request.id.is_empty() => request,
                        _ => {
                            tracing::warn!(
                                "Lua command cancellation requires a request id"
                            );
                            return;
                        }
                    };
                    let key = (owner.clone(), request.id.clone());
                    if let Some(previous) = self.lua_command_completions.get(&key) {
                        let command = previous.command.clone();
                        self.lua_command_completions.insert(
                            key,
                            neoism_lua::PluginCommandCompletion::cancelled(
                                request.id, command,
                            ),
                        );
                    } else if self
                        .lua_command_completions
                        .keys()
                        .any(|(_, id)| id == &request.id)
                    {
                        tracing::warn!(plugin = %owner.plugin_id, "rejected cross-owner Lua command cancellation");
                    } else {
                        tracing::debug!(plugin = %owner.plugin_id, "Lua command cancellation matched no pending request");
                    }
                }
                _ => unreachable!(),
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::RegisterSet
                | neoism_lua::HostOperation::ClipboardSet
                | neoism_lua::HostOperation::MarkSet
                | neoism_lua::HostOperation::MarkDelete
                | neoism_lua::HostOperation::JumplistJump
                | neoism_lua::HostOperation::MacroSet
                | neoism_lua::HostOperation::MacroPlay
        ) {
            let expected_document = action
                .arguments
                .get("document")
                .and_then(serde_json::Value::as_str);
            let active_document = self
                .lua_published
                .get("document")
                .and_then(|value| value.get("handle"))
                .and_then(serde_json::Value::as_str);
            if contract.operation != neoism_lua::HostOperation::ClipboardSet
                && (expected_document.is_none() || expected_document != active_document)
            {
                tracing::warn!(plugin = %owner.plugin_id, "rejected stale plugin editor resource action");
                return;
            }
            match contract.operation {
                neoism_lua::HostOperation::ClipboardSet => {
                    let Some(text) = action
                        .arguments
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return;
                    };
                    if text.len() > 1024 * 1024 {
                        return;
                    }
                    self.router.clipboard.set(
                        neoism_backend::clipboard::ClipboardType::Clipboard,
                        text.to_owned(),
                    );
                }
                neoism_lua::HostOperation::RegisterSet => {
                    let Some(name) = action
                        .arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|name| {
                            (name.chars().count() == 1)
                                .then(|| name.chars().next())
                                .flatten()
                        })
                    else {
                        return;
                    };
                    let Some(text) = action
                        .arguments
                        .get("text")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return;
                    };
                    if text.len() > 1024 * 1024 {
                        return;
                    }
                    let value = neoism_ui::editor::markdown::vim::VimRegisterValue {
                        text: text.to_owned(),
                        linewise: action
                            .arguments
                            .get("linewise")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                        blockwise: action
                            .arguments
                            .get("blockwise")
                            .and_then(serde_json::Value::as_bool)
                            .unwrap_or(false),
                    };
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if let Some(code) = route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .code
                            .as_mut()
                        {
                            code.buffer.vim.registers.write(name, value, true);
                            if matches!(name, '"' | '+' | '*') {
                                self.router.clipboard.set(
                                    neoism_backend::clipboard::ClipboardType::Clipboard,
                                    text.to_owned(),
                                );
                            }
                        }
                    }
                }
                neoism_lua::HostOperation::MarkSet => {
                    let Some(name) = action
                        .arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|name| name.chars().next())
                        .filter(|name| name.is_ascii_lowercase())
                    else {
                        return;
                    };
                    let (Some(line), Some(col)) = (
                        action
                            .arguments
                            .get("line")
                            .and_then(serde_json::Value::as_u64),
                        action
                            .arguments
                            .get("character")
                            .and_then(serde_json::Value::as_u64),
                    ) else {
                        return;
                    };
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if let Some(code) = route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .code
                            .as_mut()
                        {
                            let line = line as usize;
                            let col = col as usize;
                            if code.buffer.lines.get(line).is_some_and(|text| {
                                col <= text.len() && text.is_char_boundary(col)
                            }) {
                                code.buffer.vim.marks.insert(
                                    name,
                                    neoism_ui::editor::markdown::vim::VimMark {
                                        line,
                                        col,
                                    },
                                );
                            }
                        }
                    }
                }
                neoism_lua::HostOperation::MarkDelete => {
                    let Some(name) = action
                        .arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|name| name.chars().next())
                    else {
                        return;
                    };
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if let Some(code) = route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .code
                            .as_mut()
                        {
                            code.buffer.vim.marks.remove(&name);
                        }
                    }
                }
                neoism_lua::HostOperation::JumplistJump => {
                    let forward = action
                        .arguments
                        .get("direction")
                        .and_then(serde_json::Value::as_str)
                        == Some("forward");
                    let count = action
                        .arguments
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1) as usize;
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if let Some(code) = route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .code
                            .as_mut()
                        {
                            let action = if forward {
                                neoism_ui::editor::markdown::vim::VimAction::JumpForward {
                                    count,
                                }
                            } else {
                                neoism_ui::editor::markdown::vim::VimAction::JumpBack {
                                    count,
                                }
                            };
                            code.buffer.apply_vim_action(&action, None);
                        }
                    }
                }
                neoism_lua::HostOperation::MacroSet => {
                    let Some(name) = action
                        .arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|name| name.chars().next())
                        .filter(|name| name.is_ascii_lowercase())
                    else {
                        return;
                    };
                    let Some(keys) = action
                        .arguments
                        .get("keys")
                        .and_then(serde_json::Value::as_str)
                    else {
                        return;
                    };
                    if keys.chars().count() > 4096 {
                        return;
                    }
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        if let Some(code) = route
                            .window
                            .screen
                            .context_manager
                            .current_mut()
                            .code
                            .as_mut()
                        {
                            code.buffer
                                .vim
                                .registers
                                .macros
                                .insert(name, keys.to_owned());
                        }
                    }
                }
                neoism_lua::HostOperation::MacroPlay => {
                    let Some(name) = action
                        .arguments
                        .get("name")
                        .and_then(serde_json::Value::as_str)
                        .and_then(|name| name.chars().next())
                    else {
                        return;
                    };
                    let count = action
                        .arguments
                        .get("count")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(1)
                        .min(100) as usize;
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        route.window.screen.play_code_macro(
                            name,
                            count,
                            &mut self.router.clipboard,
                        );
                    }
                }
                _ => unreachable!(),
            }
            return;
        }
        if contract.operation == neoism_lua::HostOperation::DiagnosticActionExecute {
            let resource = action.arguments.get("resource").cloned().and_then(|value| {
                serde_json::from_value::<neoism_lua::PluginResourceId>(value).ok()
            });
            let command = action
                .arguments
                .get("command")
                .and_then(serde_json::Value::as_str);
            let (Some(resource), Some(command)) = (resource, command) else {
                tracing::warn!("invalid plugin diagnostic action invocation");
                return;
            };
            if !self
                .lua_editor_resources
                .registry
                .owns_resource(owner, resource)
            {
                tracing::warn!(plugin = %owner.plugin_id, "rejected stale or cross-owner diagnostic action");
                return;
            }
            let command_contribution = self
                .lua_runtime
                .as_ref()
                .filter(|runtime| runtime.owner() == owner)
                .and_then(|runtime| {
                    runtime
                        .snapshot()
                        .commands
                        .iter()
                        .find(|candidate| candidate.id == command)
                })
                .cloned()
                .or_else(|| self.lua_plugins.command_contribution(owner, command));
            let Some(command_contribution) = command_contribution else {
                tracing::warn!(plugin = %owner.plugin_id, %command, "diagnostic action command is stale");
                return;
            };
            let arguments = action
                .arguments
                .get("arguments")
                .cloned()
                .unwrap_or_default();
            if let Err(error) = neoism_lua::validate_command_arguments(
                &command_contribution.arguments_schema,
                &arguments,
            ) {
                tracing::warn!(plugin = %owner.plugin_id, %command, %error, "diagnostic action arguments rejected");
                return;
            }
            let event = neoism_lua::PluginEvent::new(
                neoism_lua::PluginEventKind::Command,
                serde_json::json!({ "id": command, "arguments": arguments }),
                neoism_lua::ExecutionScope::Local,
                Some("plugin-diagnostic-action".into()),
            )
            .expect("registered command event");
            let result = if command_contribution
                .callback
                .starts_with("lua:neoism.user-init@")
            {
                self.lua_runtime
                    .as_ref()
                    .expect("user callback runtime")
                    .invoke(&command_contribution.callback, event)
                    .map_err(|error| error.to_string())
            } else {
                self.lua_plugins
                    .invoke(&command_contribution.callback, event)
                    .map_err(|error| error.to_string())
            };
            match result {
                Ok(value) => {
                    if let Err(error) = neoism_lua::validate_command_arguments(
                        &command_contribution.result_schema,
                        &value,
                    ) {
                        tracing::warn!(%error, "Lua diagnostic action returned an invalid structured result");
                    }
                }
                Err(error) => tracing::warn!(%error, "Lua diagnostic action failed"),
            }
            return;
        }
        if matches!(
            contract.operation,
            neoism_lua::HostOperation::SchedulerCreate
                | neoism_lua::HostOperation::SchedulerCancel
        ) {
            match contract.operation {
                neoism_lua::HostOperation::SchedulerCreate => {
                    let Some(timer_id) = action.invocation_id.clone() else {
                        tracing::warn!("Lua timer is missing its handle");
                        return;
                    };
                    let mut request = match serde_json::from_value::<
                        neoism_lua::PluginTimerRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua timer request");
                            return;
                        }
                    };
                    if action.action == "every" && request.interval_millis.is_none() {
                        request.interval_millis = Some(request.delay_millis);
                    }
                    let timers = self.lua_timers.get_or_insert_with(Default::default);
                    let owner_count = timers
                        .values()
                        .filter(|timer| &timer.owner == owner)
                        .count();
                    const MAX_TIMER_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
                    if owner_count >= 256
                        || timers.len() >= 1_024
                        || request.delay_millis > MAX_TIMER_MS
                        || request.interval_millis.is_some_and(|interval| {
                            !(10..=MAX_TIMER_MS).contains(&interval)
                        })
                    {
                        tracing::warn!(plugin = %owner.plugin_id, "Lua timer budget rejected request");
                        return;
                    }
                    timers.insert(
                        timer_id,
                        LuaPluginTimerLease {
                            owner: owner.clone(),
                            due: Instant::now()
                                + Duration::from_millis(request.delay_millis),
                            interval: request.interval_millis.map(Duration::from_millis),
                            command: request.command,
                            arguments: request.arguments,
                        },
                    );
                }
                neoism_lua::HostOperation::SchedulerCancel => {
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginTimerCancelRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua timer cancellation");
                            return;
                        }
                    };
                    if self
                        .lua_timers
                        .as_ref()
                        .and_then(|timers| timers.get(&request.timer))
                        .is_some_and(|timer| &timer.owner == owner)
                    {
                        if let Some(timers) = self.lua_timers.as_mut() {
                            timers.remove(&request.timer);
                        }
                    } else {
                        tracing::warn!(plugin = %owner.plugin_id, "rejected stale or cross-owner Lua timer cancellation");
                    }
                }
                _ => unreachable!(),
            }
            return;
        }

        if matches!(
            contract.operation,
            neoism_lua::HostOperation::NamespaceCreate
                | neoism_lua::HostOperation::NamespaceClear
                | neoism_lua::HostOperation::NamespaceDelete
                | neoism_lua::HostOperation::AnchorCreate
                | neoism_lua::HostOperation::AnchorDelete
                | neoism_lua::HostOperation::DecorationCreate
                | neoism_lua::HostOperation::DecorationDelete
                | neoism_lua::HostOperation::DecorationClear
                | neoism_lua::HostOperation::DiagnosticPublish
                | neoism_lua::HostOperation::DiagnosticClear
        ) {
            let Some(external_id) = action.invocation_id.clone() else {
                tracing::warn!(
                    "Lua editor resource action is missing its reserved handle"
                );
                return;
            };
            let owner = owner.clone();
            match contract.operation {
                neoism_lua::HostOperation::NamespaceCreate => {
                    match self.lua_editor_resources.registry.create_namespace(&owner) {
                        Ok(namespace) => {
                            self.lua_editor_resources
                                .namespaces
                                .insert(external_id, (owner, namespace));
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to create Lua editor namespace")
                        }
                    }
                }
                neoism_lua::HostOperation::NamespaceClear
                | neoism_lua::HostOperation::NamespaceDelete
                | neoism_lua::HostOperation::DecorationClear
                | neoism_lua::HostOperation::DiagnosticClear => {
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginResourceTargetRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua namespace target");
                            return;
                        }
                    };
                    let Some((namespace_owner, namespace)) = self
                        .lua_editor_resources
                        .namespaces
                        .get(&request.namespace)
                        .cloned()
                    else {
                        tracing::warn!("Lua editor namespace handle is stale");
                        return;
                    };
                    if namespace_owner != owner {
                        tracing::warn!(
                            "Lua editor namespace belongs to another owner revision"
                        );
                        return;
                    }
                    let result = match contract.operation {
                        neoism_lua::HostOperation::NamespaceDelete => self
                            .lua_editor_resources
                            .registry
                            .remove_namespace(&owner, namespace),
                        neoism_lua::HostOperation::DecorationClear => self
                            .lua_editor_resources
                            .registry
                            .clear_decorations(&owner, namespace, None),
                        neoism_lua::HostOperation::DiagnosticClear => {
                            self.lua_editor_resources.registry.clear_decorations(
                                &owner,
                                namespace,
                                Some(neoism_lua::DecorationLayer::Diagnostic),
                            )
                        }
                        _ => self
                            .lua_editor_resources
                            .registry
                            .clear_namespace(&owner, namespace),
                    };
                    if let Err(error) = result {
                        tracing::warn!(%error, "failed to clear Lua editor namespace");
                    }
                    if matches!(
                        contract.operation,
                        neoism_lua::HostOperation::NamespaceClear
                            | neoism_lua::HostOperation::NamespaceDelete
                    ) {
                        self.lua_editor_resources.anchors.retain(|_, lease| {
                            lease.owner != owner || lease.namespace != namespace
                        });
                    }
                    self.lua_editor_resources.decorations.retain(|_, lease| {
                        lease.0 != owner
                            || lease.1 != namespace
                            || (contract.operation
                                == neoism_lua::HostOperation::DiagnosticClear
                                && lease.3 != neoism_lua::DecorationLayer::Diagnostic)
                    });
                    if contract.operation == neoism_lua::HostOperation::NamespaceDelete {
                        self.lua_editor_resources
                            .namespaces
                            .remove(&request.namespace);
                    }
                }
                neoism_lua::HostOperation::AnchorCreate => {
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginAnchorRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua anchor request");
                            return;
                        }
                    };
                    let Some((namespace_owner, namespace)) = self
                        .lua_editor_resources
                        .namespaces
                        .get(&request.namespace)
                        .cloned()
                    else {
                        tracing::warn!("Lua anchor namespace handle is stale");
                        return;
                    };
                    if namespace_owner != owner {
                        tracing::warn!(
                            "Lua anchor namespace belongs to another owner revision"
                        );
                        return;
                    }
                    let Some(route) = self.router.routes.get(&window_id) else {
                        return;
                    };
                    let manager = &route.window.screen.context_manager;
                    let current = manager.current();
                    let Some(code) = current.code.as_ref() else {
                        tracing::warn!("Lua anchor target is not a code document");
                        return;
                    };
                    let workspace_identity =
                        format!("{:?}", manager.current_workspace_tree_id());
                    let window_identity = format!("{window_id:?}");
                    let route_identity = current.route_id.to_string();
                    let host_path = code.path.to_string_lossy().into_owned();
                    let actual_document =
                        neoism_lua::DocumentHandle(neoism_lua::opaque_resource_handle(
                            "document",
                            &[
                                &window_identity,
                                &workspace_identity,
                                &route_identity,
                                &host_path,
                            ],
                        ));
                    if request.document != actual_document
                        || request.expected_revision != code.buffer.revision
                    {
                        tracing::warn!(
                            "Lua anchor rejected a stale document handle or revision"
                        );
                        return;
                    }
                    let line = request.position.line as usize;
                    let col = request.position.character as usize;
                    if code.buffer.lines.get(line).is_none_or(|text| {
                        col > text.len() || !text.is_char_boundary(col)
                    }) {
                        tracing::warn!("Lua anchor position is outside the document or not a UTF-8 boundary");
                        return;
                    }
                    let offset =
                        neoism_ui::editor::markdown::doc_sync::position_to_doc_byte(
                            &code.buffer.lines,
                            line,
                            col,
                        );
                    let buffer_id =
                        crate::screen::markdown_crdt::buffer_id_for_markdown_path(
                            &code.path,
                        );
                    let sticky = route
                        .window
                        .screen
                        .code_crdt
                        .binding_for(&buffer_id)
                        .and_then(|binding| {
                            binding.sticky_anchor_at(
                                line,
                                col,
                                request.bias == neoism_lua::AnchorBias::After,
                            )
                        })
                        .map(|anchor| LuaPluginStickyAnchor {
                            window_id,
                            buffer_id,
                            anchor,
                        });
                    match self.lua_editor_resources.registry.create_anchor(
                        &owner,
                        namespace,
                        request.document,
                        offset,
                        request.bias,
                    ) {
                        Ok(resource) => {
                            self.lua_editor_resources.published_targets.insert(
                                actual_document.clone(),
                                (window_id, current.route_id),
                            );
                            self.lua_editor_resources
                                .document_text
                                .entry(actual_document.clone())
                                .or_insert_with(|| code.buffer.text());
                            self.lua_editor_resources.anchors.insert(
                                external_id,
                                LuaPluginAnchorLease {
                                    owner,
                                    namespace,
                                    resource,
                                    sticky,
                                    window_id,
                                },
                            );
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to create Lua editor anchor")
                        }
                    }
                }
                neoism_lua::HostOperation::AnchorDelete
                | neoism_lua::HostOperation::DecorationDelete => {
                    let request = match serde_json::from_value::<
                        neoism_lua::PluginResourceTargetRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua resource delete");
                            return;
                        }
                    };
                    let Some(resource_handle) = request.resource else {
                        tracing::warn!("Lua resource delete requires a resource handle");
                        return;
                    };
                    let Some((namespace_owner, namespace)) = self
                        .lua_editor_resources
                        .namespaces
                        .get(&request.namespace)
                        .cloned()
                    else {
                        tracing::warn!("Lua resource namespace is stale");
                        return;
                    };
                    if namespace_owner != owner {
                        tracing::warn!(
                            "Lua resource namespace belongs to another owner revision"
                        );
                        return;
                    }
                    let resource = self
                        .lua_editor_resources
                        .anchors
                        .get(&resource_handle)
                        .filter(|lease| {
                            lease.owner == owner && lease.namespace == namespace
                        })
                        .map(|lease| lease.resource)
                        .or_else(|| {
                            self.lua_editor_resources
                                .decorations
                                .get(&resource_handle)
                                .filter(|lease| lease.0 == owner && lease.1 == namespace)
                                .map(|lease| lease.2)
                        });
                    let Some(resource) = resource else {
                        tracing::warn!(
                            "Lua editor resource handle is stale or cross-owner"
                        );
                        return;
                    };
                    if let Err(error) = self
                        .lua_editor_resources
                        .registry
                        .remove_resource(&owner, namespace, resource)
                    {
                        tracing::warn!(%error, "failed to remove Lua editor resource");
                    }
                    self.lua_editor_resources.anchors.remove(&resource_handle);
                    self.lua_editor_resources
                        .decorations
                        .remove(&resource_handle);
                }
                neoism_lua::HostOperation::DecorationCreate
                | neoism_lua::HostOperation::DiagnosticPublish => {
                    let mut request = match serde_json::from_value::<
                        neoism_lua::PluginDecorationRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua decoration request");
                            return;
                        }
                    };
                    if contract.operation == neoism_lua::HostOperation::DiagnosticPublish
                    {
                        request.layer = neoism_lua::DecorationLayer::Diagnostic;
                    }
                    let Some((namespace_owner, namespace)) = self
                        .lua_editor_resources
                        .namespaces
                        .get(&request.namespace)
                        .cloned()
                    else {
                        tracing::warn!("Lua decoration namespace is stale");
                        return;
                    };
                    if namespace_owner != owner {
                        tracing::warn!(
                            "Lua decoration namespace belongs to another owner revision"
                        );
                        return;
                    }
                    let Some(start) = self
                        .lua_editor_resources
                        .anchors
                        .get(&request.start)
                        .filter(|lease| {
                            lease.owner == owner && lease.namespace == namespace
                        })
                        .map(|lease| lease.resource)
                    else {
                        tracing::warn!(
                            "Lua decoration start anchor is stale or cross-owner"
                        );
                        return;
                    };
                    let Some(end) = self
                        .lua_editor_resources
                        .anchors
                        .get(&request.end)
                        .filter(|lease| {
                            lease.owner == owner && lease.namespace == namespace
                        })
                        .map(|lease| lease.resource)
                    else {
                        tracing::warn!(
                            "Lua decoration end anchor is stale or cross-owner"
                        );
                        return;
                    };
                    if action.action == "update" || action.action == "set" {
                        if let Some(existing_handle) = request.resource.as_deref() {
                            let existing = self
                                .lua_editor_resources
                                .decorations
                                .get(existing_handle)
                                .filter(|lease| lease.0 == owner && lease.1 == namespace)
                                .map(|lease| lease.2);
                            let Some(existing) = existing else {
                                tracing::warn!("Lua decoration update target is stale or cross-owner");
                                return;
                            };
                            if let Err(error) = self
                                .lua_editor_resources
                                .registry
                                .remove_resource(&owner, namespace, existing)
                            {
                                tracing::warn!(%error, "failed to replace Lua editor decoration");
                                return;
                            }
                            self.lua_editor_resources
                                .decorations
                                .remove(existing_handle);
                        }
                    }
                    let decoration = neoism_lua::PluginDecoration {
                        id: neoism_lua::PluginResourceId(0),
                        start,
                        end,
                        layer: request.layer,
                        class: request.class,
                        text: request.text,
                        severity: request.severity,
                        style: request.style,
                        related_information: request.related_information,
                        tags: request.tags,
                        actions: request.actions,
                    };
                    match self
                        .lua_editor_resources
                        .registry
                        .create_decoration(&owner, namespace, decoration)
                    {
                        Ok(resource) => {
                            self.lua_editor_resources.decorations.insert(
                                external_id,
                                (owner, namespace, resource, request.layer),
                            );
                        }
                        Err(error) => {
                            tracing::warn!(%error, "failed to create Lua editor decoration")
                        }
                    }
                }
                _ => unreachable!(),
            }
            self.sync_lua_editor_resources();
            if matches!(
                contract.operation,
                neoism_lua::HostOperation::DiagnosticPublish
                    | neoism_lua::HostOperation::DiagnosticClear
            ) {
                let event = neoism_lua::PluginEvent::new(
                    neoism_lua::PluginEventKind::DiagnosticsChanged,
                    action.arguments,
                    neoism_lua::ExecutionScope::Local,
                    Some("desktop-plugin-diagnostics".into()),
                )
                .expect("registered diagnostics event");
                if let Err(error) =
                    self.lua_plugins.activate_trigger("DiagnosticsChanged")
                {
                    tracing::warn!(%error, "lazy Lua diagnostic event activation failed");
                }
                for failure in self.lua_plugins.emit(event.clone()) {
                    tracing::warn!(plugin = %failure.plugin_id, error = %failure.message, "Lua diagnostic autocmd failed");
                }
                if let Some(runtime) = self.lua_runtime.as_mut() {
                    if let Err(error) = runtime.emit(event) {
                        tracing::warn!(%error, "Lua diagnostic autocmd failed");
                    }
                }
            }
            return;
        }

        if matches!(
            contract.operation,
            neoism_lua::HostOperation::DocumentEdit
                | neoism_lua::HostOperation::DocumentSetSelections
                | neoism_lua::HostOperation::DocumentMoveCursor
                | neoism_lua::HostOperation::DocumentFocus
                | neoism_lua::HostOperation::DocumentSave
                | neoism_lua::HostOperation::DocumentClose
        ) {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let manager = &mut route.window.screen.context_manager;
            let workspace_identity = format!("{:?}", manager.current_workspace_tree_id());
            let window_identity = format!("{window_id:?}");
            let route_identity = manager.current().route_id.to_string();
            let Some(code) = manager.current_mut().code.as_mut() else {
                tracing::warn!("Lua document action target is no longer a code document");
                return;
            };
            let host_path = code.path.to_string_lossy().into_owned();
            let handle = neoism_lua::DocumentHandle(neoism_lua::opaque_resource_handle(
                "document",
                &[
                    &window_identity,
                    &workspace_identity,
                    &route_identity,
                    &host_path,
                ],
            ));
            let requested = action
                .arguments
                .get("document")
                .or_else(|| action.arguments.get("handle"))
                .and_then(serde_json::Value::as_str);
            if requested != Some(handle.as_str()) {
                tracing::warn!("Lua document handle is stale; action was not retargeted");
                return;
            }
            let expected_revision = action
                .arguments
                .get("expectedRevision")
                .or_else(|| action.arguments.get("expected_revision"))
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(u64::MAX);
            if expected_revision != code.buffer.revision {
                tracing::warn!(
                    expected_revision,
                    actual_revision = code.buffer.revision,
                    "Lua document action rejected a stale revision"
                );
                return;
            }
            match contract.operation {
                neoism_lua::HostOperation::DocumentEdit => {
                    let request = match serde_json::from_value::<
                        neoism_lua::DocumentEditRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua document edit");
                            return;
                        }
                    };
                    let mut edits = Vec::with_capacity(request.edits.len());
                    for edit in request.edits {
                        let expected_range = neoism_lua::opaque_resource_handle(
                            "range",
                            &[
                                handle.as_str(),
                                &request.expected_revision.to_string(),
                                &edit.range.start.line.to_string(),
                                &edit.range.start.character.to_string(),
                                &edit.range.end.line.to_string(),
                                &edit.range.end.character.to_string(),
                            ],
                        );
                        if !edit.range.handle.is_empty()
                            && edit.range.handle.as_str() != expected_range
                        {
                            tracing::warn!("Lua document range handle is stale");
                            return;
                        }
                        edits.push(neoism_ui::editor::code::buffer::CodeTextEdit {
                            start_line: edit.range.start.line as usize,
                            start_col: edit.range.start.character as usize,
                            end_line: edit.range.end.line as usize,
                            end_col: edit.range.end.character as usize,
                            text: edit.text,
                        });
                    }
                    if let Err(error) = code.buffer.validate_text_edits(&edits) {
                        tracing::warn!(%error, "Lua document edit failed preflight");
                        return;
                    }
                    code.buffer.apply_text_edits(&edits);
                }
                neoism_lua::HostOperation::DocumentMoveCursor => {
                    let request = match serde_json::from_value::<
                        neoism_lua::DocumentCursorRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) => request,
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua cursor move");
                            return;
                        }
                    };
                    let line = request.position.line as usize;
                    let col = request.position.character as usize;
                    if code.buffer.lines.get(line).is_none_or(|text| {
                        col > text.len() || !text.is_char_boundary(col)
                    }) {
                        tracing::warn!("Lua cursor target is outside the document or not a UTF-8 boundary");
                        return;
                    }
                    code.buffer
                        .set_cursor_position(line, col, request.extend_selection);
                }
                neoism_lua::HostOperation::DocumentSetSelections => {
                    let request = match serde_json::from_value::<
                        neoism_lua::DocumentSelectionsRequest,
                    >(action.arguments.clone())
                    {
                        Ok(request) if !request.selections.is_empty() => request,
                        Ok(_) => {
                            code.buffer.clear_selection();
                            code.buffer.extra_carets.clear();
                            route.window.screen.mark_dirty();
                            route.request_redraw();
                            return;
                        }
                        Err(error) => {
                            tracing::warn!(%error, "invalid Lua document selections");
                            return;
                        }
                    };
                    for selection in &request.selections {
                        for position in [selection.anchor, selection.active] {
                            let line = position.line as usize;
                            let col = position.character as usize;
                            if code.buffer.lines.get(line).is_none_or(|text| {
                                col > text.len() || !text.is_char_boundary(col)
                            }) {
                                tracing::warn!("Lua selection is outside the document or not a UTF-8 boundary");
                                return;
                            }
                        }
                        let expected = neoism_lua::opaque_resource_handle(
                            "selection",
                            &[
                                handle.as_str(),
                                &request.expected_revision.to_string(),
                                &selection.anchor.line.to_string(),
                                &selection.anchor.character.to_string(),
                                &selection.active.line.to_string(),
                                &selection.active.character.to_string(),
                            ],
                        );
                        if !selection.handle.is_empty()
                            && selection.handle.as_str() != expected
                        {
                            tracing::warn!("Lua selection handle is stale");
                            return;
                        }
                    }
                    let primary = &request.selections[0];
                    code.buffer.set_cursor_position(
                        primary.anchor.line as usize,
                        primary.anchor.character as usize,
                        false,
                    );
                    code.buffer.set_cursor_position(
                        primary.active.line as usize,
                        primary.active.character as usize,
                        true,
                    );
                    code.buffer.extra_carets = request
                        .selections
                        .iter()
                        .skip(1)
                        .map(|selection| neoism_ui::editor::code::CodeExtraCaret {
                            line: selection.active.line as usize,
                            col: selection.active.character as usize,
                            anchor: Some(neoism_ui::editor::code::CodePosition {
                                line: selection.anchor.line as usize,
                                col: selection.anchor.character as usize,
                            }),
                        })
                        .collect();
                }
                neoism_lua::HostOperation::DocumentFocus => {}
                neoism_lua::HostOperation::DocumentSave => {
                    route.window.screen.execute_palette_action(
                        PaletteAction::SaveDocument,
                        &mut self.router.clipboard,
                    );
                }
                neoism_lua::HostOperation::DocumentClose => {
                    route.window.screen.execute_palette_action(
                        PaletteAction::TabClose,
                        &mut self.router.clipboard,
                    );
                }
                _ => unreachable!(),
            }
            route.window.screen.mark_dirty();
            route.request_redraw();
            return;
        }
        if action.namespace == "lsp" {
            if action.action == "request" {
                let request = match serde_json::from_value::<neoism_lua::LuaLspRequest>(
                    action.arguments.clone(),
                ) {
                    Ok(request) => request,
                    Err(error) => {
                        tracing::warn!(%error, "invalid structured Lua LSP request");
                        return;
                    }
                };
                let (Some(owner), Some(id)) =
                    (action.owner.clone(), action.invocation_id.clone())
                else {
                    tracing::warn!(
                        "structured Lua LSP request is missing owner or invocation id"
                    );
                    return;
                };
                let key = (owner.clone(), id.clone());
                if self.lua_lsp_pending.contains_key(&key) {
                    tracing::warn!(plugin = %owner.plugin_id, request = %id, "duplicate structured Lua LSP request id");
                    return;
                }
                let owner_pending = self
                    .lua_lsp_pending
                    .keys()
                    .filter(|(pending_owner, _)| pending_owner == &owner)
                    .count();
                if owner_pending >= LUA_LSP_PENDING_PER_OWNER
                    || self.lua_lsp_pending.len() >= LUA_LSP_PENDING_GLOBAL
                {
                    self.emit_lua_lsp_completion(
                        &owner,
                        neoism_lua::LuaLspCompletion::failed(
                            id,
                            request.operation,
                            None,
                            "request_limit",
                            "too many structured LSP requests are pending",
                        ),
                    );
                    return;
                }
                if request.operation == neoism_lua::LuaLspOperation::ApplyCodeAction {
                    let request_id = request
                        .arguments
                        .get("requestId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let action_id = request
                        .arguments
                        .get("actionId")
                        .and_then(serde_json::Value::as_str)
                        .map(str::to_owned);
                    let has_override =
                        request.arguments.as_object().is_some_and(|arguments| {
                            arguments
                                .keys()
                                .any(|key| key != "requestId" && key != "actionId")
                        });
                    let Some((request_id, action_id)) = request_id.zip(action_id) else {
                        self.emit_lua_lsp_completion(
                            &owner,
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                request.operation,
                                None,
                                "invalid_action",
                                "apply_code_action requires requestId and actionId",
                            ),
                        );
                        return;
                    };
                    if has_override {
                        self.emit_lua_lsp_completion(
                            &owner,
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                request.operation,
                                None,
                                "invalid_action",
                                "apply_code_action cannot override the captured target",
                            ),
                        );
                        return;
                    }
                    let now = Instant::now();
                    self.lua_lsp_actions.retain(|_, lease| {
                        now.saturating_duration_since(lease.created_at)
                            < LUA_LSP_ACTION_TTL
                    });
                    let lease_key = (owner.clone(), request_id, action_id);
                    let Some(lease) = self.lua_lsp_actions.remove(&lease_key) else {
                        self.emit_lua_lsp_completion(
                            &owner,
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                request.operation,
                                None,
                                "invalid_action",
                                "the code action is stale, unknown, or already consumed",
                            ),
                        );
                        return;
                    };
                    let target = lease.target.clone();
                    let dispatched = self
                        .router
                        .routes
                        .get_mut(&lease.window_id)
                        .ok_or_else(|| "the action window is no longer available".to_string())
                        .and_then(|route| match lease.action {
                            crate::screen::bridges::code::lsp::LuaLspRetainedCodeAction::Local(
                                action,
                            ) => route.window.screen.dispatch_local_lua_code_action(
                                owner.clone(),
                                id.clone(),
                                action,
                            ),
                            crate::screen::bridges::code::lsp::LuaLspRetainedCodeAction::Remote(
                                action,
                            ) => route.window.screen.dispatch_remote_lua_code_action(
                                owner.clone(),
                                id.clone(),
                                action,
                            ),
                        });
                    match dispatched {
                        Ok(dispatched_target) if dispatched_target == target => {
                            self.lua_lsp_pending.insert(
                                key,
                                PendingLuaLspRequest {
                                    window_id: lease.window_id,
                                    operation: request.operation,
                                    target,
                                },
                            );
                        }
                        Ok(_) => self.emit_lua_lsp_completion(
                            &owner,
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                request.operation,
                                Some(target),
                                "stale_target",
                                "the code action target no longer matches its lease",
                            ),
                        ),
                        Err(message) => self.emit_lua_lsp_completion(
                            &owner,
                            neoism_lua::LuaLspCompletion::failed(
                                id,
                                request.operation,
                                Some(target),
                                "stale_target",
                                message,
                            ),
                        ),
                    }
                    return;
                }
                let dispatched = self
                    .router
                    .routes
                    .get_mut(&window_id)
                    .ok_or_else(|| {
                        "the request window is no longer available".to_string()
                    })
                    .and_then(|route| {
                        route.window.screen.dispatch_lua_lsp_request(
                            owner.clone(),
                            id.clone(),
                            request.operation,
                            &request.arguments,
                        )
                    });
                match dispatched {
                    Ok(target) => {
                        self.lua_lsp_pending.insert(
                            key,
                            PendingLuaLspRequest {
                                window_id,
                                operation: request.operation,
                                target,
                            },
                        );
                    }
                    Err(message) => self.emit_lua_lsp_completion(
                        &owner,
                        neoism_lua::LuaLspCompletion::failed(
                            id,
                            request.operation,
                            None,
                            "invalid_target",
                            message,
                        ),
                    ),
                }
                return;
            }
            if action.action == "cancel" {
                let requested_id = action
                    .arguments
                    .as_str()
                    .or_else(|| {
                        action
                            .arguments
                            .get("id")
                            .and_then(serde_json::Value::as_str)
                    })
                    .or_else(|| {
                        action
                            .arguments
                            .get("requestId")
                            .and_then(serde_json::Value::as_str)
                    })
                    .map(str::to_owned);
                let (Some(owner), Some(requested_id)) =
                    (action.owner.as_ref(), requested_id)
                else {
                    tracing::warn!("Lua lsp.cancel requires an owned request id");
                    return;
                };
                let key = (owner.clone(), requested_id.clone());
                if let Some(pending) = self.lua_lsp_pending.remove(&key) {
                    crate::screen::bridges::code::lsp::cancel_remote_lua_lsp_request(
                        owner,
                        &requested_id,
                    );
                    self.emit_lua_lsp_completion(
                        owner,
                        neoism_lua::LuaLspCompletion::cancelled(
                            requested_id,
                            pending.operation,
                            pending.target,
                        ),
                    );
                } else if self
                    .lua_lsp_pending
                    .keys()
                    .any(|(_, id)| id == &requested_id)
                {
                    tracing::warn!(plugin = %owner.plugin_id, request = %requested_id, "Lua plugin cannot cancel another owner's LSP request");
                } else {
                    tracing::debug!(plugin = %owner.plugin_id, request = %requested_id, "Lua LSP cancellation did not match a pending request");
                }
                return;
            }
            if action.action == "signature_help" {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route.window.screen.request_code_signature_help();
                    route.request_redraw();
                }
                return;
            }
            let palette = match action.action.as_str() {
                "definition" => Some(PaletteAction::LspDefinition),
                "references" => Some(PaletteAction::LspReferences),
                "rename" => Some(PaletteAction::LspRename),
                "code_actions" => Some(PaletteAction::LspCodeAction),
                "format" => Some(PaletteAction::LspFormat),
                "hover" => Some(PaletteAction::LspHover),
                "document_symbols" => Some(PaletteAction::LspDocumentSymbols),
                "workspace_symbols" => Some(PaletteAction::LspWorkspaceSymbols),
                _ => None,
            };
            let Some(palette) = palette else {
                tracing::warn!(operation = %action.action, "Lua LSP operation is not wired yet");
                return;
            };
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .execute_palette_action(palette, &mut self.router.clipboard);
                route.request_redraw();
            }
            return;
        }
        if action.namespace == "buffer" && action.action == "edit" {
            let Some(buffer_id) = argument("buffer_id").or_else(|| argument("bufferId"))
            else {
                tracing::warn!("Lua buffer.edit requires buffer_id");
                return;
            };
            let values = action
                .arguments
                .get("edits")
                .and_then(serde_json::Value::as_array)
                .cloned()
                .unwrap_or_else(|| vec![action.arguments.clone()]);
            let mut edits = Vec::with_capacity(values.len());
            for value in values {
                let Some(index) = value
                    .get("index")
                    .or_else(|| value.get("start"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                else {
                    tracing::warn!("Lua buffer.edit requires a UTF-16 index");
                    return;
                };
                let len = value
                    .get("len")
                    .or_else(|| value.get("length"))
                    .and_then(serde_json::Value::as_u64)
                    .and_then(|value| u32::try_from(value).ok())
                    .unwrap_or(0);
                let content = value
                    .get("content")
                    .or_else(|| value.get("text"))
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or_default()
                    .to_string();
                let edit = match value
                    .get("kind")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or(if len == 0 { "insert" } else { "replace" })
                {
                    "insert" => {
                        neoism_protocol::CrdtBufferEdit::Insert { index, content }
                    }
                    "delete" => neoism_protocol::CrdtBufferEdit::Delete { index, len },
                    "replace" => neoism_protocol::CrdtBufferEdit::Replace {
                        index,
                        len,
                        content,
                    },
                    kind => {
                        tracing::warn!(kind, "invalid Lua buffer.edit kind");
                        return;
                    }
                };
                edits.push(edit);
            }
            let expected_state_vector_v1 = action
                .arguments
                .get("expected_state_vector_v1")
                .or_else(|| action.arguments.get("expectedStateVectorV1"))
                .cloned()
                .and_then(|value| serde_json::from_value(value).ok())
                .unwrap_or_default();
            let invocation_id = action
                .invocation_id
                .unwrap_or_else(|| "lua-untracked".to_string());
            let plugin_id = action
                .owner
                .as_ref()
                .map(|owner| owner.plugin_id.clone())
                .unwrap_or_else(|| "user.init.lua".to_string());
            let idempotency_key = argument("idempotency_key")
                .or_else(|| argument("idempotencyKey"))
                .unwrap_or_else(|| invocation_id.clone());
            if let Some(session) = self.window_sessions.get(&window_id) {
                session.connection.send_crdt_batch(vec![
                    neoism_protocol::CrdtClientMessage::ApplyEdits {
                        transaction: neoism_protocol::CrdtEditTransaction {
                            buffer_id,
                            plugin_id,
                            invocation_id,
                            idempotency_key,
                            expected_state_vector_v1,
                            edits,
                        },
                    },
                ]);
            }
            return;
        }
        if action.namespace == "agent" && action.action == "send" {
            let Some(text) = argument("text").or_else(|| argument("message")) else {
                return;
            };
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let Some(agent) = route
                .window
                .screen
                .context_manager
                .current_mut()
                .neoism_agent
                .as_mut()
            else {
                return;
            };
            if !agent.input().is_empty() {
                tracing::warn!(
                    "Lua agent.send ignored while the composer contains unsent text"
                );
                return;
            }
            agent.insert_text(&text);
            agent.submit();
            route.window.screen.mark_dirty();
            route.request_redraw();
            return;
        }
        if matches!(action.namespace.as_str(), "buffer" | "file_tree" | "notes")
            && matches!(action.action.as_str(), "open" | "reveal")
            && argument("path").is_some()
        {
            let path = std::path::PathBuf::from(argument("path").unwrap_or_default());
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.open_path_in_editor(path);
                route.request_redraw();
            }
            return;
        }
        if matches!(action.namespace.as_str(), "file_tree" | "notes")
            && matches!(
                action.action.as_str(),
                "create" | "create_dir" | "rename" | "move" | "delete"
            )
        {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            let screen = &mut route.window.screen;
            let notes = action.namespace == "notes";
            let root = if notes {
                screen.renderer.notes_sidebar.workspace_path()
            } else {
                screen.renderer.file_tree.remote_root().or_else(|| {
                    screen
                        .renderer
                        .file_tree
                        .root()
                        .map(std::path::Path::to_path_buf)
                })
            };
            let Some(root) = root else {
                tracing::warn!(namespace = %action.namespace, "Lua file operation has no active root");
                return;
            };
            let resolve = |value: &str| {
                let path = std::path::PathBuf::from(value);
                if path
                    .components()
                    .any(|part| matches!(part, std::path::Component::ParentDir))
                {
                    return None;
                }
                let absolute = if path.is_absolute() {
                    path
                } else {
                    root.join(path)
                };
                absolute.starts_with(&root).then_some(absolute)
            };
            let relative = |path: &std::path::Path| {
                path.strip_prefix(&root)
                    .ok()
                    .map(|path| path.to_string_lossy().replace('\\', "/"))
            };
            let shared_notes = notes && screen.notes_sidebar_shows_shared_vault();
            let remote_tree = !notes && screen.renderer.file_tree.is_remote();
            let sent = match action.action.as_str() {
                "create" | "create_dir" => {
                    let exact = argument("path").and_then(|path| resolve(&path));
                    let dir = argument("dir")
                        .and_then(|dir| resolve(&dir))
                        .or_else(|| {
                            exact.as_ref().and_then(|path| {
                                path.parent().map(std::path::Path::to_path_buf)
                            })
                        })
                        .unwrap_or_else(|| root.clone());
                    let name = argument("name").or_else(|| {
                        exact.as_ref().and_then(|path| {
                            path.file_name()?.to_str().map(str::to_owned)
                        })
                    });
                    let Some(name) = name else {
                        tracing::warn!(namespace = %action.namespace, "Lua create requires path or name");
                        return;
                    };
                    let Some(dir_rel) = relative(&dir) else {
                        return;
                    };
                    if shared_notes {
                        if action.action == "create_dir" {
                            screen.send_remote_notes_create_dir(
                                root.clone(),
                                dir_rel,
                                name,
                            )
                        } else {
                            screen.send_remote_notes_create(root.clone(), dir_rel, name)
                        }
                    } else if remote_tree {
                        screen.send_remote_files_op(if action.action == "create_dir" {
                            neoism_protocol::files::FilesClientMessage::CreateDir {
                                dir: dir_rel,
                                name,
                            }
                        } else {
                            neoism_protocol::files::FilesClientMessage::CreateFile {
                                dir: dir_rel,
                                name,
                            }
                        })
                    } else {
                        let path = dir.join(name);
                        if action.action == "create_dir" {
                            std::fs::create_dir(&path).is_ok()
                        } else {
                            std::fs::OpenOptions::new()
                                .write(true)
                                .create_new(true)
                                .open(&path)
                                .is_ok()
                        }
                    }
                }
                "rename" | "move" => {
                    let Some(from) = argument("from").and_then(|path| resolve(&path))
                    else {
                        return;
                    };
                    let Some(to) = argument("to").and_then(|path| resolve(&path)) else {
                        return;
                    };
                    let (Some(from_rel), Some(to_rel)) = (relative(&from), relative(&to))
                    else {
                        return;
                    };
                    if shared_notes {
                        screen.send_remote_notes_move(root.clone(), from_rel, to_rel)
                    } else if remote_tree {
                        screen.send_remote_files_op(
                            neoism_protocol::files::FilesClientMessage::Rename {
                                from: from_rel,
                                to: to_rel,
                            },
                        )
                    } else {
                        std::fs::rename(from, to).is_ok()
                    }
                }
                "delete" => {
                    let Some(path) = argument("path").and_then(|path| resolve(&path))
                    else {
                        return;
                    };
                    let Some(path_rel) = relative(&path) else {
                        return;
                    };
                    if shared_notes {
                        screen.send_remote_notes_delete(root.clone(), path_rel)
                    } else if remote_tree {
                        screen.send_remote_files_op(
                            neoism_protocol::files::FilesClientMessage::Delete {
                                path: path_rel,
                            },
                        )
                    } else if path.is_dir() {
                        std::fs::remove_dir_all(path).is_ok()
                    } else {
                        std::fs::remove_file(path).is_ok()
                    }
                }
                _ => false,
            };
            if sent && !shared_notes && !remote_tree {
                if notes {
                    screen.refresh_notes_sidebar_if_visible();
                } else {
                    screen.refresh_file_tree();
                }
            }
            if !sent {
                tracing::warn!(namespace = %action.namespace, operation = %action.action, "Lua file operation failed");
            }
            screen.mark_dirty();
            route.request_redraw();
            return;
        }
        if action.namespace == "terminal"
            && matches!(action.action.as_str(), "send" | "run")
        {
            let Some(mut text) = argument("text").or_else(|| argument("command")) else {
                return;
            };
            if action.action == "run" && !text.ends_with('\n') {
                text.push('\n');
            }
            let Some(route_id) = self
                .router
                .routes
                .get(&window_id)
                .map(|route| route.window.screen.context_manager.current_route())
            else {
                return;
            };
            Self::send_bytes_to_route_context(
                &mut self.router,
                window_id,
                route_id,
                text.into_bytes(),
            );
            return;
        }
        if action.namespace == "tab"
            && matches!(action.action.as_str(), "focus" | "select")
        {
            let Some(index) = action
                .arguments
                .get("index")
                .and_then(serde_json::Value::as_u64)
            else {
                return;
            };
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .context_manager
                    .select_tab(index as usize);
                route.window.screen.mark_dirty();
                route.request_redraw();
            }
            return;
        }
        if action.namespace == "tab" && action.action == "move" {
            let Some(from) = action
                .arguments
                .get("from")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            else {
                return;
            };
            let Some(to) = action
                .arguments
                .get("to")
                .and_then(serde_json::Value::as_u64)
                .and_then(|value| u32::try_from(value).ok())
            else {
                return;
            };
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                let pane_external_id = action
                    .arguments
                    .get("pane_id")
                    .or_else(|| action.arguments.get("paneId"))
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or_else(|| {
                        route.window.screen.context_manager.current_route() as u64
                    });
                route
                    .window
                    .screen
                    .request_move_tab(pane_external_id, from, to);
                route.request_redraw();
            }
            return;
        }
        if action.namespace == "workspace"
            && matches!(action.action.as_str(), "open" | "focus")
        {
            let Some(id) = argument("id").or_else(|| argument("workspace_id")) else {
                return;
            };
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.open_or_adopt_daemon_workspace(id);
                route.request_redraw();
            }
            return;
        }
        if action.namespace == "git"
            && matches!(
                action.action.as_str(),
                "stage" | "unstage" | "commit" | "refresh"
            )
        {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            match action.action.as_str() {
                "stage" => route.window.screen.renderer.git_diff_panel.stage_all(),
                "unstage" => route.window.screen.renderer.git_diff_panel.unstage_all(),
                "commit" => {
                    if let Some(message) = argument("message") {
                        route
                            .window
                            .screen
                            .renderer
                            .git_diff_panel
                            .commit_input_insert(&message);
                    }
                    route.window.screen.renderer.git_diff_panel.commit();
                }
                "refresh" => route.window.screen.sync_host_git(),
                _ => {}
            }
            route.request_redraw();
            return;
        }
        if matches!(action.namespace.as_str(), "config" | "theme")
            && matches!(action.action.as_str(), "set" | "apply")
        {
            let patch = action
                .arguments
                .get("value")
                .cloned()
                .unwrap_or_else(|| action.arguments.clone());
            let mut candidate = self.config.clone();
            if let Err(error) = candidate.apply_json_patch(&patch) {
                tracing::warn!(%error, "Lua config patch rejected");
                return;
            }
            self.config = candidate;
            for route in self.router.routes.values_mut() {
                route.update_config(&self.config, &self.router.font_library, false);
                route.window.configure_window(&self.config);
                route.request_redraw();
            }
            let snapshot = self
                .lua_runtime
                .as_ref()
                .map(|runtime| runtime.snapshot().clone())
                .unwrap_or_else(neoism_lua::PluginSnapshot::empty);
            self.router.set_plugin_snapshot(Arc::new(snapshot));
            return;
        }
        let palette_action = match (action.namespace.as_str(), action.action.as_str()) {
            ("workspace", "split") => match argument("direction").as_deref() {
                Some("down" | "bottom") => Some(PaletteAction::SplitDown),
                _ => Some(PaletteAction::SplitRight),
            },
            ("tab", "create" | "open") => Some(PaletteAction::TabCreate),
            ("tab", "close") => Some(PaletteAction::TabClose),
            ("buffer", "save") => Some(PaletteAction::SaveDocument),
            ("notes", "open") => Some(PaletteAction::OpenNeoismNotes),
            ("notes", "create") => Some(PaletteAction::CreateNeoismNote),
            ("git", "open" | "toggle") => Some(PaletteAction::ToggleGitDiffPanel),
            ("agent", "open" | "create") => Some(PaletteAction::OpenNeoismAgent),
            _ => None,
        };
        if action.namespace == "command" && action.action == "execute" {
            let Some(id) = argument("id") else { return };
            if let Some(command) = lua_palette_action(&id) {
                let router = &mut self.router;
                if let Some(route) = router.routes.get_mut(&window_id) {
                    route
                        .window
                        .screen
                        .execute_palette_action(command, &mut router.clipboard);
                    route.request_redraw();
                }
            } else if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.queue_plugin_command(id);
            }
            return;
        }
        if let Some(action) = palette_action {
            let router = &mut self.router;
            if let Some(route) = router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .execute_palette_action(action, &mut router.clipboard);
                route.request_redraw();
            }
            return;
        }

        if action.namespace != "panel" {
            tracing::warn!(
                namespace = %action.namespace,
                action = %action.action,
                "unsupported Lua host action"
            );
            return;
        }
        let Some(panel) = argument("id") else { return };
        let desired = match action.action.as_str() {
            "show" => Some(true),
            "hide" | "close" => Some(false),
            "toggle" | "open" => None,
            _ => return,
        };
        let built_in = matches!(
            panel.as_str(),
            "file-tree"
                | "file_tree"
                | "notes"
                | "notes-tree"
                | "notes_tree"
                | "agent"
                | "agent-sidebar"
                | "agent_sidebar"
                | "status"
                | "status-line"
                | "status_line"
                | "top"
                | "top-bar"
                | "top_bar"
                | "composer"
                | "bottom"
                | "bottom-chrome"
        );
        if !built_in {
            let Some(route) = self.router.routes.get(&window_id) else {
                return;
            };
            let mut snapshot = (*route.window.screen.renderer.plugins).clone();
            let Some(contribution) =
                snapshot.panels.iter_mut().find(|item| item.id == panel)
            else {
                return;
            };
            contribution.visible = desired.unwrap_or(!contribution.visible);
            self.router.set_plugin_snapshot(Arc::new(snapshot));
            return;
        }
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let screen = &mut route.window.screen;
        match panel.as_str() {
            "file-tree" | "file_tree" => {
                let visible = screen.renderer.file_tree.is_visible();
                screen.renderer.set_left_sidebar_visibility(
                    neoism_ui::panels::left_sidebar_host::LeftSidebarView::Files,
                    desired.unwrap_or(!visible),
                    false,
                );
            }
            "notes" | "notes-tree" | "notes_tree" => {
                let visible = screen.renderer.notes_sidebar.is_visible();
                screen.renderer.set_left_sidebar_visibility(
                    neoism_ui::panels::left_sidebar_host::LeftSidebarView::Notes,
                    desired.unwrap_or(!visible),
                    false,
                );
            }
            "agent" | "agent-sidebar" | "agent_sidebar" => {
                let visible = screen.renderer.conversations_visible;
                screen.renderer.set_left_sidebar_visibility(
                    neoism_ui::panels::left_sidebar_host::LeftSidebarView::Conversations,
                    desired.unwrap_or(!visible),
                    false,
                );
            }
            "status" | "status-line" | "status_line" => {
                let visible = screen.renderer.status_line.is_visible();
                screen
                    .renderer
                    .status_line
                    .set_visible(desired.unwrap_or(!visible));
            }
            "top" | "top-bar" | "top_bar" => {
                let visible = screen.renderer.top_bar.is_visible();
                screen
                    .renderer
                    .top_bar
                    .set_visible(desired.unwrap_or(!visible));
            }
            "composer" | "bottom" | "bottom-chrome" => {
                let visible = screen.renderer.command_composer.is_visible();
                screen
                    .renderer
                    .command_composer
                    .set_visible(desired.unwrap_or(!visible));
            }
            _ => return,
        }
        screen.sync_current_workspace_chrome_snapshot();
        screen.sync_file_tree_watchers();
        screen.reapply_chrome_layout();
        screen.mark_dirty();
        route.request_redraw();
    }

    /// Turn a dead Quick-SSH transport back into the same workspace attach.
    /// The daemon client intentionally retries forever, but a dead `ssh -L`
    /// child can never make that local port live again. Without this bridge
    /// the window remained branded `SSH >>>` while its tree and commands were
    /// permanently disconnected; reopening the workspace was the only code
    /// path that noticed the dead child.
    fn queue_dead_ssh_reconnect(&mut self, window_id: WindowId) {
        if self.ssh_attach_inflight.contains(&window_id) {
            return;
        }
        let Some(endpoint) = self.window_sessions.get(&window_id).and_then(|session| {
            matches!(
                session.status,
                ServerConnectionStatus::Reconnecting | ServerConnectionStatus::Offline
            )
            .then(|| session.connection.endpoint().to_string())
        }) else {
            return;
        };
        let reconnect = self.ssh_attaches.get_mut(&endpoint).and_then(|attach| {
            (!attach.is_running())
                .then(|| (attach.alias.clone(), attach.ssh_args.clone()))
        });
        let Some((target, ssh_args)) = reconnect else {
            return;
        };

        self.ssh_attaches.remove(&endpoint);
        if let Some(session) = self.window_sessions.get_mut(&window_id) {
            session.parked_connections.remove(&endpoint);
        }
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route
                .window
                .screen
                .request_ssh_workspace_attach(target.clone(), ssh_args);
            route.window.screen.renderer.notifications.push(
                format!("SSH connection to {target} dropped; reconnecting…"),
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
            route.request_redraw();
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
            ssh_attach_request,
            add_request,
            edit_request,
            edit_submit,
            remove_request,
            open_manager,
            join_request,
            go_home,
            workspace_subscription,
            workspace_unsubscriptions,
        ) = {
            let Some(route) = self.router.routes.get_mut(&window_id) else {
                return;
            };
            (
                route.window.screen.take_server_connect(),
                route.window.screen.take_ssh_workspace_attach(),
                route.window.screen.take_server_add(),
                route.window.screen.take_server_edit(),
                route.window.screen.take_server_edit_submit(),
                route.window.screen.take_server_remove(),
                route.window.screen.take_server_manager_request(),
                route.window.screen.take_peer_workspace_join(),
                route.window.screen.take_daemon_go_home(),
                route.window.screen.take_workspace_subscription(),
                route.window.screen.take_workspace_unsubscriptions(),
            )
        };

        if let Some((target, ssh_args)) = ssh_attach_request {
            self.start_ssh_workspace_attach(window_id, target, ssh_args);
        }

        if let Some(workspace_id) = workspace_subscription {
            self.persist_workspace_subscription(window_id, workspace_id);
        }
        for workspace_id in workspace_unsubscriptions {
            self.persist_workspace_unsubscription(window_id, workspace_id);
        }

        if let Some(server_id) = server_request {
            if let Err(error) = self.server_registry.reload() {
                tracing::warn!(%error, "could not refresh saved server before connecting");
                return;
            }
            if server_id == "local" {
                if let Some(endpoint) = self.home_daemon_endpoint.clone() {
                    self.switch_window_server(window_id, &endpoint, None, None);
                }
            } else if let Some(server) = self.server_registry.server(&server_id).cloned()
            {
                if server.agent_api {
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        route.window.screen.renderer.modal.open_message("Agent API connection", "This saved server supports agents and chats only. Open it in the Neoism chat GUI; it cannot attach terminals or editors.");
                        route.request_redraw();
                    }
                    return;
                }
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
            // A locally-hosted server (Create & join) leaves a relaunch-spec
            // sidecar in its state dir; fold it onto the SavedServer so a later
            // dial that finds the daemon dead can rehost it. Non-hosted /
            // remote addresses read back None and take the plain add path.
            let added =
                match crate::screen::bridges::palette::read_hosted_sidecar(&address) {
                    Some(spec) => self.server_registry.add_hosted(
                        &address,
                        name.as_deref(),
                        token.as_deref(),
                        spec,
                    ),
                    None => self.server_registry.add(
                        &address,
                        name.as_deref(),
                        token.as_deref(),
                    ),
                };
            match added {
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
            // Grab the hosted spec BEFORE dropping the entry: deleting a
            // server the user HOSTS must also KILL its daemon, or the process
            // and its bound port leak (the orphaned-daemon pileup). A plain
            // remote saved server has no spec, so nothing is killed.
            let hosted = self.server_registry.hosted_spec(&server_id);
            if let Err(error) = self.server_registry.remove(&server_id) {
                tracing::warn!(%error, "failed to remove saved server");
            }
            if let Some(spec) = hosted {
                crate::screen::bridges::palette::stop_hosted_daemon(&spec);
                tracing::info!(
                    target: "neoism::desktop_daemon",
                    port = spec.port,
                    "deleted a hosted server; stopped its daemon"
                );
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
            // A parked Quick-SSH workspace may outlive its tunnel. Recreate
            // that transport lazily when the user selects the workspace;
            // retain the original -p/-i/-F/-J options and never probe the
            // network on the UI thread.
            let stale_ssh = self.ssh_attaches.get_mut(&daemon_url).and_then(|attach| {
                (!attach.is_running())
                    .then(|| (attach.alias.clone(), attach.ssh_args.clone()))
            });
            if let Some((target, ssh_args)) = stale_ssh {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route
                        .window
                        .screen
                        .request_ssh_workspace_attach(target.clone(), ssh_args);
                    route.window.screen.renderer.notifications.push(
                        format!("Reconnecting to {target}…"),
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    );
                    route.request_redraw();
                }
            } else {
                let already_connected = self
                    .window_sessions
                    .get(&window_id)
                    .is_some_and(|session| session.connection.endpoint() == daemon_url);
                if !already_connected {
                    let server_id = self.remote_server_id_for_endpoint(&daemon_url);
                    let token = self
                        .server_registry
                        .token(&server_id)
                        .map(str::to_string)
                        .or_else(|| {
                            self.ssh_attaches
                                .get(&daemon_url)
                                .map(|attach| attach.credential.clone())
                        });
                    self.switch_window_server(
                        window_id,
                        &daemon_url,
                        token,
                        Some(server_id),
                    );
                }
                self.send_window_message(
                    window_id,
                    WorkspaceClientMessage::RequestHostWorkspaceTree,
                );
            }
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

        // Host ended the session (daemon shutdown / stop-sharing): the
        // ingest layer already detached this window's adopted grids
        // (without killing the host's shells). Surface the reason, mark
        // the session offline so it stops showing "Reconnecting", and
        // re-dial home — which replaces the guest connection and drops
        // its reconnect loop.
        let host_ended_reason =
            self.router.routes.get_mut(&window_id).and_then(|route| {
                route.window.screen.context_manager.take_host_ended_reason()
            });
        if let Some(reason) = host_ended_reason {
            if let Some(session) = self.window_sessions.get_mut(&window_id) {
                session.mark_host_ended();
            }
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.renderer.notifications.push(
                    reason,
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
                route.request_redraw();
            }
            if let Some(home) = self.home_daemon_endpoint.clone() {
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

    fn persist_workspace_unsubscription(
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
        if let Err(error) = self.server_registry.remove_workspace_subscription(
            &profile_id,
            &server_id,
            &workspace_id,
        ) {
            tracing::warn!(
                %error,
                %workspace_id,
                "failed to persist workspace unsubscription"
            );
        }
    }

    fn open_server_manager(&mut self, window_id: WindowId) {
        if let Err(error) = self.server_registry.reload() {
            tracing::warn!(%error, "could not refresh shared server registry");
        }
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
            if server.agent_api {
                continue;
            }
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
        if let Err(error) = self.server_registry.reload() {
            tracing::warn!(%error, "could not refresh saved server for editing");
            return;
        }
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
            let (code_crdt_messages, code_pane_changed) =
                route.window.screen.drain_code_crdt_messages();
            outbound_crdt.extend(code_crdt_messages);
            let code_analysis_changed =
                route.window.screen.service_code_revision_changes();
            let (markdown_disk_messages, markdown_disk_changed) =
                route.window.screen.reload_open_markdown_files_from_disk();
            outbound_crdt.extend(markdown_disk_messages);
            (
                outbound,
                outbound_crdt,
                markdown_pane_changed
                    || code_pane_changed
                    || code_analysis_changed
                    || markdown_disk_changed,
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

    fn resync_window_after_reconnect(&mut self, window_id: WindowId) {
        let Some(session) = self.window_sessions.get(&window_id) else {
            return;
        };
        if session.status != ServerConnectionStatus::Online
            && session.connection.status()
                != crate::daemon_client::DaemonClientStatus::Open
        {
            return;
        }
        let generation = session.connection.handle().generation();
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        if !route
            .window
            .screen
            .context_manager
            .resync_after_daemon_reconnect(generation)
        {
            return;
        }
        route.window.screen.request_markdown_crdt_resync();
        route.window.screen.request_code_crdt_resync();
        route.window.screen.sync_host_git();
        route
            .window
            .screen
            .sync_file_tree_root_for_current_workspace();
        route.request_redraw();
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
            let code_changed = route.window.screen.apply_code_crdt_message(&message);
            let presence_changed =
                route.window.screen.apply_presence_crdt_message(&message);
            if presence_changed {
                // Event-driven: fold the new presence into the file
                // tree's path->peers index exactly once per change, so
                // the per-row avatar draw never polls the store.
                route.window.screen.rebuild_file_tree_presence_index();
            }
            if markdown_changed || code_changed || presence_changed {
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
        if matches!(
            &message,
            WorkspaceServerMessage::HelloAck {
                accepted: false,
                ..
            }
        ) {
            if let Some(session) = self.window_sessions.get_mut(&source_window_id) {
                session.mark_host_ended();
            }
            if let Some(route) = self.router.routes.get_mut(&source_window_id) {
                let reason = match &message {
                    WorkspaceServerMessage::HelloAck { reason, .. } => reason
                        .clone()
                        .unwrap_or_else(|| "authentication rejected".into()),
                    _ => "authentication rejected".into(),
                };
                route.window.screen.renderer.notifications.push(
                    reason,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                route.request_redraw();
            }
            return;
        }
        if matches!(
            &message,
            WorkspaceServerMessage::HelloAck { accepted: true, .. }
        ) {
            self.resync_window_after_reconnect(source_window_id);
        }
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

        // Persisted subscriptions may only be pruned by a COMPLETE inventory.
        // `WorkspaceControlChanged` intentionally carries one workspace; the
        // old shared helper treated that singleton as the whole server and
        // silently deleted every other subscription, which later looked like
        // tabs disappearing when another shared workspace changed.
        let authoritative_summaries = match &message {
            WorkspaceServerMessage::HostWorkspaceTree { workspaces, .. }
            | WorkspaceServerMessage::HostWorkspaceList { workspaces } => {
                Some(workspaces.as_slice())
            }
            _ => None,
        };
        if let Some(summaries) = authoritative_summaries {
            let subscription =
                self.window_sessions.get(&source_window_id).map(|session| {
                    let server_id =
                        session.active_server_id.as_deref().unwrap_or("local");
                    self.server_registry.prune_workspace_subscription(
                        &session.profile_id,
                        server_id,
                        summaries.iter().map(|workspace| workspace.id.as_str()),
                    )
                });
            if let Some(subscription) = subscription {
                let subscription = match subscription {
                    Ok(subscription) => subscription,
                    Err(error) => {
                        tracing::warn!(
                            target: "neoism::desktop_daemon",
                            window = ?source_window_id,
                            %error,
                            "failed to persist pruned workspace subscription"
                        );
                        // Persistence errors must not prevent the current tree
                        // from rendering; use the in-memory snapshot as a
                        // best-effort fallback for this pump.
                        let Some(session) = self.window_sessions.get(&source_window_id)
                        else {
                            return;
                        };
                        self.server_registry.workspace_subscription(
                            &session.profile_id,
                            session.active_server_id.as_deref().unwrap_or("local"),
                        )
                    }
                };
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
            let server_id = self.remote_server_id_for_endpoint(&daemon_url);
            let token = self
                .server_registry
                .token(&server_id)
                .map(str::to_string)
                .or_else(|| {
                    self.ssh_attaches
                        .get(&daemon_url)
                        .map(|attach| attach.credential.clone())
                });
            self.switch_window_server(
                source_window_id,
                &daemon_url,
                token,
                Some(server_id),
            );
        }
    }

    /// Stable persistence scope for a non-home daemon, including ad-hoc
    /// tailnet peers that are not saved in the server manager. Falling back to
    /// `local` for those peers mixed their workspace subscriptions with the
    /// home daemon and let one shared-workspace update hide local tabs.
    fn remote_server_id_for_endpoint(&self, daemon_url: &str) -> String {
        self.server_registry
            .servers()
            .iter()
            .find(|server| server.endpoint == daemon_url)
            .map(|server| server.id.clone())
            .or_else(|| {
                self.ssh_attaches
                    .get(daemon_url)
                    .map(|attach| ssh_server_id(&attach.workspace_id))
            })
            .unwrap_or_else(|| format!("peer:{daemon_url}"))
    }

    fn start_ssh_workspace_attach(
        &mut self,
        window_id: WindowId,
        target: String,
        ssh_args: Vec<String>,
    ) {
        let existing = self
            .ssh_attaches
            .iter_mut()
            .find(|(_, attach)| attach.alias == target && attach.ssh_args == ssh_args)
            .map(|(endpoint, attach)| {
                (
                    endpoint.clone(),
                    attach.is_running(),
                    attach.workspace_id.clone(),
                    attach.credential.clone(),
                )
            });
        if let Some((endpoint, true, workspace_id, credential)) = existing {
            self.begin_ssh_workspace_switch(
                window_id,
                target,
                endpoint,
                workspace_id,
                credential,
            );
            return;
        }
        if let Some((endpoint, false, _, _)) = existing {
            self.ssh_attaches.remove(&endpoint);
            if let Some(session) = self.window_sessions.get_mut(&window_id) {
                session.parked_connections.remove(&endpoint);
            }
        }
        if !self.ssh_attach_inflight.insert(window_id) {
            return;
        }
        let tx = self.ssh_attach_tx.clone();
        let event_proxy = self.event_proxy.clone();
        let thread_target = target.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("neoism-ssh-attach-{window_id:?}"))
            .spawn(move || {
                let result =
                    crate::ssh_hosts::attach_workspace_over_ssh(&thread_target, ssh_args)
                        .map_err(|error| error.to_string());
                let _ = tx.send(PendingSshAttach {
                    window_id,
                    target: thread_target,
                    result,
                });
                event_proxy.send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                    neoism_window::window::WindowId::dummy()
                });
            });
        if let Err(error) = spawn {
            self.ssh_attach_inflight.remove(&window_id);
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.cancel_ssh_workspace_replacement();
                route.window.screen.renderer.notifications.push(
                    format!("Could not start SSH connection to {target}: {error}"),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                route.request_redraw();
            }
        }
    }

    fn begin_ssh_workspace_switch(
        &mut self,
        window_id: WindowId,
        target: String,
        daemon_url: String,
        workspace_id: String,
        credential: String,
    ) {
        let server_id = ssh_server_id(&workspace_id);
        let workspace = PendingSshWorkspace {
            daemon_url: daemon_url.clone(),
            workspace_id: workspace_id.clone(),
            title: format!("{target} · Home"),
        };
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route
                .window
                .screen
                .prepare_ssh_workspace_replacement(workspace_id.clone());
        }
        if let Some(session) = self.window_sessions.get_mut(&window_id) {
            session.pending_peer_adopt = Some(workspace_id);
        }
        self.pending_ssh_workspaces.insert(window_id, workspace);
        if self
            .window_sessions
            .get(&window_id)
            .is_some_and(|session| session.connection.endpoint() == daemon_url)
        {
            self.create_pending_ssh_workspace(window_id);
            return;
        }
        self.switch_window_server(
            window_id,
            &daemon_url,
            Some(credential),
            Some(server_id),
        );
    }

    fn create_pending_ssh_workspace(&mut self, window_id: WindowId) {
        let Some(ssh_workspace) = self.pending_ssh_workspaces.remove(&window_id) else {
            return;
        };
        // `~` is resolved by the daemon on the SSH host, not by this desktop.
        // A stable id makes reconnects reopen the same remote home workspace
        // and its daemon-owned PTYs instead of accumulating duplicates.
        self.send_window_message(
            window_id,
            WorkspaceClientMessage::CreateWorkspace {
                workspace_id: Some(ssh_workspace.workspace_id),
                title: Some(ssh_workspace.title.clone()),
                root_dir: Some(PathBuf::from("~")),
            },
        );
        self.send_window_message(
            window_id,
            WorkspaceClientMessage::RequestHostWorkspaceTree,
        );
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route.window.screen.renderer.notifications.push(
                format!("Connected to {}", ssh_workspace.title),
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
            route.request_redraw();
        }
    }

    fn drain_ssh_attach_results(&mut self) {
        while let Ok(pending) = self.ssh_attach_rx.try_recv() {
            self.ssh_attach_inflight.remove(&pending.window_id);
            let attach = match pending.result {
                Ok(attach) => attach,
                Err(error) => {
                    tracing::warn!(
                        target: "neoism::ssh_workspace",
                        window = ?pending.window_id,
                        target_host = %pending.target,
                        %error,
                        "SSH workspace attach failed"
                    );
                    if let Some(route) = self.router.routes.get_mut(&pending.window_id) {
                        route.window.screen.cancel_ssh_workspace_replacement();
                        route.window.screen.renderer.notifications.push(
                            format!(
                                "Could not open SSH workspace {}: {error}",
                                pending.target
                            ),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        );
                        route.request_redraw();
                    }
                    continue;
                }
            };

            let daemon_url = attach.daemon_url.clone();
            let workspace_id = attach.workspace_id.clone();
            let credential = attach.credential.clone();
            self.ssh_attaches.insert(daemon_url.clone(), attach);
            self.begin_ssh_workspace_switch(
                pending.window_id,
                pending.target,
                daemon_url,
                workspace_id,
                credential,
            );
        }
    }

    fn switch_window_server(
        &mut self,
        window_id: WindowId,
        daemon_url: &str,
        token: Option<String>,
        server_id: Option<String>,
    ) {
        let is_ssh_attach = self
            .pending_ssh_workspaces
            .get(&window_id)
            .is_some_and(|pending| pending.daemon_url == daemon_url);
        if !is_ssh_attach
            && self
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

        // Reuse any connection parked for this endpoint. This preserves each
        // server's daemon namespace and remote PTYs while another workspace
        // in the same window is active.
        let parked = self
            .window_sessions
            .get_mut(&window_id)
            .and_then(|session| session.parked_connections.remove(daemon_url));
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
        // Rehosting a dead daemon re-dials, which loops back through here.
        // Remember which hosted servers we've already relaunched this session
        // so a daemon that refuses to come up can't spin an infinite relaunch
        // loop — we relaunch each hosted server at most once per session.
        thread_local! {
            static RELAUNCHED: std::cell::RefCell<std::collections::HashSet<String>> =
                std::cell::RefCell::new(std::collections::HashSet::new());
        }
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
                    if self
                        .pending_ssh_workspaces
                        .get(&window_id)
                        .is_some_and(|pending| pending.daemon_url == daemon_url)
                    {
                        self.pending_ssh_workspaces.remove(&window_id);
                        self.ssh_attaches.remove(&daemon_url);
                        if let Some(route) = self.router.routes.get_mut(&window_id) {
                            route.window.screen.cancel_ssh_workspace_replacement();
                        }
                    }
                    // A server we host may just have died with a previous app
                    // session. Relaunch its daemon ONCE (per session) and
                    // re-dial before giving up — the dial retries with backoff,
                    // so the freshly spawned daemon wins the startup race.
                    if let Some(server_id) = server_id.clone() {
                        if let Some(spec) = self.server_registry.hosted_spec(&server_id) {
                            let first_attempt = RELAUNCHED
                                .with(|set| set.borrow_mut().insert(server_id.clone()));
                            if first_attempt {
                                let token = self
                                    .server_registry
                                    .token(&server_id)
                                    .map(str::to_string);
                                match crate::screen::bridges::palette::spawn_hosted_daemon(
                                    &spec,
                                    token.as_deref(),
                                ) {
                                    Ok(()) => {
                                        tracing::info!(
                                            target: "neoism::desktop_daemon",
                                            window = ?window_id,
                                            daemon = %daemon_url,
                                            server = %server_id,
                                            "hosted daemon was down; relaunched it and re-dialing"
                                        );
                                        self.switch_window_server(
                                            window_id,
                                            &daemon_url,
                                            token,
                                            Some(server_id),
                                        );
                                        continue;
                                    }
                                    Err(error) => tracing::warn!(
                                        target: "neoism::desktop_daemon",
                                        window = ?window_id,
                                        server = %server_id,
                                        %error,
                                        "failed to relaunch hosted daemon"
                                    ),
                                }
                            }
                        }
                    }
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
    /// the parked-connection fast path: swap the window's session (parking the
    /// outgoing connection so its daemon-side panes and namespace stay alive),
    /// reset server-owned chrome, attach, and request the new server's inventory.
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
        connection.set_parked(false);
        let outgoing = self.window_sessions.remove(&window_id);
        let home_endpoint = self.home_daemon_endpoint.clone();
        let switching_home = home_endpoint.as_deref() == Some(connection.endpoint());
        let (profile_id, parked_connections, pending_peer_adopt) = match outgoing {
            Some(mut old) => {
                let profile_id = old.profile_id.clone();
                let pending_peer_adopt = old.pending_peer_adopt.take();
                let mut parked = std::mem::take(&mut old.parked_connections);
                if old.connection.endpoint() != connection.endpoint() {
                    old.connection.set_parked(true);
                    parked.insert(old.connection.endpoint().to_string(), old.connection);
                }
                (profile_id, parked, pending_peer_adopt)
            }
            None => (self.next_window_profile_id(), HashMap::new(), None),
        };
        let mut session = WindowServerSession::new(profile_id, connection, server_id);
        // A foreign server needs an initial adopt so the window lands in one
        // of its workspaces. Returning to the parked HOME connection does
        // not: this window's local grids are already alive. Auto-adopting the
        // home tree here duplicated the remaining local workspace after a
        // guest closed their last joined tab.
        let ssh_workspace = self
            .pending_ssh_workspaces
            .get(&window_id)
            .filter(|pending| pending.daemon_url == session.connection.endpoint())
            .cloned();
        session.pending_peer_adopt = ssh_workspace
            .as_ref()
            .map(|pending| pending.workspace_id.clone())
            .or(pending_peer_adopt);
        session.needs_initial_workspace_adopt =
            !switching_home && session.pending_peer_adopt.is_none();
        session.parked_connections = parked_connections;
        self.window_sessions.insert(window_id, session);
        self.attach_session_to_window(window_id);
        // Do not broadcast this connection's agent endpoint across the
        // window. A window may retain local grids while it adopts a peer grid;
        // switching every pane here clears the local chats and can leave a
        // guest chat executing locally with a host-only working directory.
        // `load_current_workspace_chrome` resolves and applies the correct
        // endpoint to the active grid after adoption and on every workspace
        // switch.
        if ssh_workspace.is_some() {
            self.create_pending_ssh_workspace(window_id);
        }
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

    fn apply_daemon_pty_message(
        &mut self,
        window_id: WindowId,
        source_endpoint: &str,
        source_connection_key: usize,
        request_id: u64,
        message: neoism_protocol::pty::ServerMessage,
    ) {
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            // A workspace message earlier in the drained batch may have
            // switched and parked this connection. Never feed its remaining
            // PTY frames into the newly active endpoint's route/session cache.
            if route.window.screen.context_manager.daemon_endpoint()
                != Some(source_endpoint)
                || route.window.screen.context_manager.daemon_connection_key()
                    != Some(source_connection_key)
            {
                tracing::warn!(
                    target: "neoism::remote_pty",
                    %source_endpoint,
                    source_connection_key,
                    active_endpoint = ?route.window.screen.context_manager.daemon_endpoint(),
                    active_connection_key = ?route.window.screen.context_manager.daemon_connection_key(),
                    request_id,
                    "ignoring PTY frame from a non-owning daemon connection"
                );
                return;
            }
            if let neoism_protocol::pty::ServerMessage::Error { message } = &message {
                route.window.screen.renderer.notifications.push(
                    format!("Remote PTY error: {message}. Command execution is not confirmed; nothing was replayed."),
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
                route.request_redraw();
            }

            if route
                .window
                .screen
                .context_manager
                .apply_pty_server_message(request_id, message.clone())
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
                RioEvent::UpdateAvailable { .. } => "RioEvent::UpdateAvailable",
                RioEvent::SelfUpdateProgress { .. } => "RioEvent::SelfUpdateProgress",
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
                RioEvent::CodeDiagnosticsReady => "RioEvent::CodeDiagnosticsReady",
                RioEvent::CodeGitMarksReady => "RioEvent::CodeGitMarksReady",
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
                    Duration::from_millis(NOTEBOOK_STATUS_TICK_MS),
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
                    // Overdue damage is ready now. Pure animation still yields
                    // briefly so an expensive window cannot monopolize the event
                    // loop, but must not pay another full refresh interval after
                    // already missing its deadline.
                    let paced_wait = if pending_dirty {
                        route.window.wait_until()
                    } else {
                        Some(
                            route
                                .window
                                .wait_until()
                                .unwrap_or(Duration::from_millis(1)),
                        )
                    };
                    if let Some(wait) = paced_wait {
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
                            wait,
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
        let lua_timer_deadline = self
            .lua_timers
            .as_ref()
            .and_then(|timers| timers.values().map(|timer| timer.due).min());
        let next_deadline = [redraw_deadline, scheduler_deadline, lua_timer_deadline]
            .into_iter()
            .flatten()
            .min();
        let control_flow = match next_deadline {
            Some(instant) => ControlFlow::WaitUntil(instant),
            None => ControlFlow::Wait,
        };
        event_loop.set_control_flow(control_flow);
    }
}

impl ApplicationHandler<EventPayload> for Application<'_> {
    fn resumed(&mut self, _active_event_loop: &ActiveEventLoop) {
        for window_id in self.window_sessions.keys().copied().collect::<Vec<_>>() {
            self.recycle_stale_window_connection(window_id);
        }
    }

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
            .appearance
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
                    Duration::from_millis(250),
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
                    match quit_request_action(self.config.ui.confirm_before_quit) {
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
                            if let Some(route) = self.router.routes.get(&window_id) {
                                route.window.screen.clear_remote_code_lsp();
                            }
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
                    Duration::from_millis(100),
                    RioEvent::RefreshFileTreeGitStatus,
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::PrepareRefreshFileTree) => {
                Self::postpone_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::FileTree, 0),
                    Duration::from_millis(200),
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
                for (target_window_id, route) in &mut self.router.routes {
                    let perf_start = tracing::enabled!(target: "neoism::frame_work", tracing::Level::DEBUG)
                        .then(Instant::now);
                    let tree_redraw = matches!(
                        refresh_redraw_action(route.window.screen.refresh_file_tree()),
                        RefreshRedrawAction::Redraw
                    );
                    let tree_us = perf_start
                        .map(|start| start.elapsed().as_micros())
                        .unwrap_or(0);
                    // The fs watcher is rooted at the workspace, which
                    // also houses the notes vault — keep the open Alt+N
                    // panel live on the same signal.
                    let notes_redraw =
                        route.window.screen.refresh_notes_sidebar_if_visible();
                    if let Some(start) = perf_start {
                        let elapsed_us = start.elapsed().as_micros();
                        if elapsed_us >= 1_000 {
                            tracing::debug!(
                                target: "neoism::frame_work",
                                window_id = ?target_window_id, elapsed_us, tree_us,
                                notes_us = elapsed_us.saturating_sub(tree_us),
                                "slow filesystem refresh outside render"
                            );
                        }
                    }
                    if tree_redraw || notes_redraw {
                        route.request_redraw();
                    }
                }
            }
            RioEventType::Rio(RioEvent::RemoteEditorReadTimeout(request_id)) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    if route.window.screen.fail_remote_editor_read(
                        request_id,
                        "Host read timed out; reopen the file to retry",
                    ) {
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
            RioEventType::Rio(RioEvent::CodeDiagnosticsReady) => {
                let snapshots =
                    crate::screen::bridges::code::lsp::drain_code_diagnostic_snapshots();
                for route in self.router.routes.values_mut() {
                    let mut changed = false;
                    for snapshot in &snapshots {
                        changed |=
                            route.window.screen.apply_code_diagnostic_snapshot(snapshot);
                    }
                    if changed {
                        route.request_redraw();
                    }
                }
            }
            RioEventType::Rio(RioEvent::CodeGitMarksReady) => {
                let results = crate::screen::bridges::code::lsp::drain_code_git_results();
                for result in &results {
                    if let Some(route) = self.router.routes.get_mut(&result.window_id()) {
                        if route.window.screen.apply_code_git_result(result) {
                            route.request_redraw();
                        }
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
                // Persistent note indexing is disabled. Keep accepting the
                // legacy wake event so older helpers cannot disrupt routing.
            }
            RioEventType::Rio(RioEvent::Bell) => {
                if should_play_audio_bell(self.config.terminal.bell.audio) {
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
                        Duration::from_millis(millis),
                        RioEvent::Render,
                        window_id,
                    );
                }
            }
            RioEventType::Rio(RioEvent::PrepareRenderOnRoute(millis, route_id)) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::RenderRoute, route_id),
                    Duration::from_millis(millis),
                    RioEvent::RenderRoute(route_id),
                    window_id,
                );
            }
            RioEventType::Rio(RioEvent::BlinkCursor(millis, route_id)) => {
                Self::debounce_follow_up(
                    &mut self.scheduler,
                    TimerId::new(Topic::CursorBlinking, route_id),
                    Duration::from_millis(millis),
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
                if !self.update_check_started {
                    self.update_check_started = true;
                    if let Some(window_id) = self.router.routes.keys().next().copied() {
                        crate::update::spawn_check(self.event_proxy.clone(), window_id);
                    }
                }
                if let Some(window_id) = self.router.routes.keys().next().copied() {
                    self.present_pending_update(window_id);
                }
            }
            RioEventType::Rio(RioEvent::UpdateAvailable { version }) => {
                self.pending_update_version = Some(version);
                self.present_pending_update(window_id);
            }
            RioEventType::Rio(RioEvent::SelfUpdateProgress {
                percent,
                message,
                ready_to_restart,
                failed,
            }) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    if failed {
                        use neoism_ui::widgets::modal::{
                            ModalAction, ModalButton, ModalSpec,
                        };
                        route.window.screen.renderer.modal.open(ModalSpec {
                            title: "Update failed".to_string(),
                            body: message.clone(),
                            meta: "Review the update log before retrying; installation may have partially completed."
                                .to_string(),
                            input: None,
                            buttons: vec![ModalButton::new("Close", "Enter", ModalAction::Close)],
                            busy: false,
                            blocking: true,
                        });
                        route.window.screen.renderer.notifications.push(
                            format!("Neoism update failed: {message}"),
                            neoism_ui::panels::notifications::NotificationLevel::Error,
                        );
                    } else if percent == Some(100) && !ready_to_restart {
                        use neoism_ui::widgets::modal::{
                            ModalAction, ModalButton, ModalSpec,
                        };
                        route.window.screen.renderer.modal.open(ModalSpec {
                            title: "Neoism update".to_string(),
                            body: message,
                            meta: "No restart required.".to_string(),
                            input: None,
                            buttons: vec![ModalButton::new(
                                "Close",
                                "Enter",
                                ModalAction::Close,
                            )],
                            busy: false,
                            blocking: false,
                        });
                    } else {
                        route.window.screen.renderer.modal.update_progress(
                            message,
                            percent.map_or_else(
                                || "Preparing update".to_string(),
                                |value| format!("{value}% complete"),
                            ),
                            percent,
                            !ready_to_restart,
                        );
                    }
                    route.request_overlay_redraw();
                }
                if ready_to_restart && !failed {
                    event_loop.exit();
                }
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
            RioEventType::Rio(RioEvent::ChangeTerminalDirectory { route_id, path }) => {
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    if route.window.screen.is_primary_terminal_route(route_id) {
                        route.window.screen.set_active_workspace_root(path, true);
                        route.request_redraw();
                    }
                }
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
                if let Some(route) = self.router.routes.get(&window_id) {
                    route.window.screen.clear_remote_code_lsp();
                }
                self.router.routes.remove(&window_id);
                self.window_sessions.remove(&window_id);
                crate::app::freeze_watchdog::unregister_window(window_id);
                if should_exit_event_loop_after_close_window(
                    self.router.routes.len(),
                    self.config.ui.confirm_before_quit,
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
        // Finder "Open With" / double-click / drag-onto-Dock deliver files
        // here as file:// URLs (`application:openURLs:` in the app delegate).
        // Route regular files into the same editor path used for CLI-passed
        // paths and drag-and-drop; everything else (directories, custom
        // schemes) keeps the legacy open-a-new-surface behavior below.
        let mut editor_paths: Vec<PathBuf> = Vec::new();
        let mut other_urls: Vec<String> = Vec::new();
        for raw in urls {
            let file_path = url::Url::parse(&raw)
                .ok()
                .filter(|u| u.scheme() == "file")
                .and_then(|u| u.to_file_path().ok())
                .filter(|p| p.is_file());
            match file_path {
                Some(path) => editor_paths.push(path),
                None => other_urls.push(raw),
            }
        }

        if !editor_paths.is_empty() {
            // Prefer the focused window, else any live window. On a cold
            // start from a Finder double-click the URLs can arrive before
            // the first window exists — queue them on initial_open_paths,
            // which drains when the first window is created.
            let target = self
                .router
                .get_focused_route()
                .filter(|id| self.router.routes.contains_key(id))
                .or_else(|| self.router.routes.keys().next().copied());
            match target {
                Some(window_id) => {
                    if let Some(route) = self.router.routes.get_mut(&window_id) {
                        for path in editor_paths {
                            route.window.screen.open_path_in_editor(path);
                        }
                        route.request_redraw();
                        route.window.winit_window.focus_window();
                    }
                }
                None => self.initial_open_paths.extend(editor_paths),
            }
        }

        if other_urls.is_empty() {
            return;
        }
        let urls = other_urls;

        if !self.config.ui.navigation.is_native() {
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
        if self.config.ui.navigation.open_config_with_split {
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

fn lua_palette_action(
    id: &str,
) -> Option<neoism_ui::panels::command_palette::PaletteAction> {
    use neoism_ui::panels::command_palette::PaletteAction::*;
    Some(match id {
        "tab.create" => TabCreate,
        "tab.close" => TabClose,
        "tab.close-others" => TabCloseUnfocused,
        "tab.next" => SelectNextTab,
        "tab.previous" => SelectPrevTab,
        "split.right" => SplitRight,
        "split.down" => SplitDown,
        "split.next" => SelectNextSplit,
        "split.previous" => SelectPrevSplit,
        "split.close" => CloseCurrentSplitOrTab,
        "config.open" | "palette.toggle" => ConfigEditor,
        "settings.open" => OpenSettings,
        "window.create" => WindowCreateNew,
        "font.increase" => IncreaseFontSize,
        "font.decrease" => DecreaseFontSize,
        "font.reset" => ResetFontSize,
        "vim.toggle" => ToggleViMode,
        "git.blame.toggle" => ToggleGitBlame,
        "editor.wrap.toggle" => ToggleWordWrap,
        "editor.replace" => ReplaceInFile,
        "editor.problems" => ProjectProblems,
        "window.fullscreen.toggle" => ToggleFullscreen,
        "theme.toggle" => ToggleAppearanceTheme,
        "theme.pick" => OpenThemePicker,
        "shader.pick" => OpenShaders,
        "mashup.pick" => OpenMashupPacks,
        "edit.copy" => Copy,
        "edit.paste" => Paste,
        "buffer.save" => SaveDocument,
        "search.forward" => SearchForward,
        "search.backward" => SearchBackward,
        "search.files" | "files.find" => SearchFiles,
        "search.workspace" | "workspace.find" => SearchWords,
        "search.git" => SearchGitChanges,
        "goto.line" => GoToLine,
        "goto.symbol" => GoToSymbol,
        "git.panel.toggle" => ToggleGitDiffPanel,
        "notes.create" => CreateNeoismNote,
        "notes.open" => OpenNeoismNotes,
        "notes.draw" => DrawOnNote,
        "epub.toc" => OpenEpubTableOfContents,
        "lsp.hover" => LspHover,
        "lsp.action" => LspCodeAction,
        "lsp.format" => LspFormat,
        "lsp.definition" => LspDefinition,
        "lsp.references" => LspReferences,
        "lsp.rename" => LspRename,
        "lsp.document-symbols" => LspDocumentSymbols,
        "lsp.workspace-symbols" => LspWorkspaceSymbols,
        "editor.inlay-hints.toggle" => ToggleInlayHints,
        "editor.minimap.toggle" => ToggleMinimap,
        "terminal.clear" => ClearHistory,
        "buffers.list" => ListBuffers,
        "fonts.list" => ListFonts,
        "servers.show" => ShowServers,
        "servers.add" => AddServer,
        "servers.create" => CreateServer,
        "workspace.create" => CreateWorkspace,
        "workspace.leave" => LeaveWorkspace,
        "agent.open" => OpenNeoismAgent,
        "neoworld.open" => OpenNeoWorld,
        "agent.claude" => RunClaude,
        "agent.codex" => RunCodex,
        "agent.opencode" => RunOpenCode,
        "notebook.create" => CreateDocumentationNotebook,
        "notebook.open" => OpenDocumentationNotebook,
        "notebook.run-cell" => RunNotebookCell,
        "notebook.run-below" => RunNotebookCellAndBelow,
        "notebook.run-all" => RunAllNotebookCells,
        "notebook.insert-code-above" => InsertNotebookCodeCellAbove,
        "notebook.insert-code-below" => InsertNotebookCodeCellBelow,
        "notebook.insert-markdown-above" => InsertNotebookMarkdownCellAbove,
        "notebook.insert-markdown-below" => InsertNotebookMarkdownCellBelow,
        "notebook.delete-cell" => DeleteNotebookCell,
        "notebook.move-cell-up" => MoveNotebookCellUp,
        "notebook.move-cell-down" => MoveNotebookCellDown,
        "notebook.interrupt" => InterruptNotebookKernel,
        "notebook.clear-output" => ClearNotebookCellOutput,
        "notebook.clear-outputs" => ClearNotebookOutputs,
        "notebook.restart" => RestartNotebookKernel,
        "app.quit" => Quit,
        _ => return None,
    })
}

#[cfg(test)]
mod mashup_transaction_tests {
    use super::commit_validated_mashup_candidate;

    #[test]
    fn rejected_candidate_never_enters_visual_commit() {
        let mut visual_commit_called = false;
        let result = commit_validated_mashup_candidate::<(), _>(
            Err("plugin graph rejected"),
            |_| {
                visual_commit_called = true;
                Ok(())
            },
        );
        assert_eq!(result, Err("plugin graph rejected"));
        assert!(!visual_commit_called);
    }
}
