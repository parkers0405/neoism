//! SSH recipes are durable; loopback endpoints, credentials and connections are not.
//! This manager never routes its background daemon messages into a workspace.
use std::collections::HashMap;
use std::io;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender},
    Arc,
};

use neoism_ui::panels::command_palette::{PaletteSystemEntry, SystemConnectionStatus};
use neoism_ui::panels::notifications::NotificationLevel;
use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
use neoism_window::window::WindowId;

use super::{daemon_pump::DesktopDaemonConnection, Application as App};
use crate::daemon_client::DaemonClientStatus;
use crate::event::{RioEvent, RioEventType};
use crate::neoism::agent::system::{discover_system_source, AgentSystemSource};
use crate::ssh_hosts::DaemonAttach;
use crate::system_registry::{same_system_route, SavedSystem, SystemRegistry};

struct Attempt {
    generation: u64,
    window: WindowId,
    recipe: SavedSystem,
    cancelled: Arc<AtomicBool>,
}

impl Drop for Attempt {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

struct BackgroundConnection {
    recipe: SavedSystem,
    endpoint: String,
    connection: DesktopDaemonConnection,
    source: Option<AgentSystemSource>,
}

impl Drop for BackgroundConnection {
    fn drop(&mut self) {
        if let Some(source) = &self.source {
            source.revoke();
            crate::neoism::agent::register_agent_server_credential(&source.server, None);
        }
    }
}

struct ConnectedSystem {
    endpoint: String,
    // Only newly bootstrapped tunnels travel through the channel. Reused guards
    // stay in App::ssh_attaches throughout authentication.
    attach: Option<DaemonAttach>,
    connection: DesktopDaemonConnection,
    source: Result<AgentSystemSource, String>,
    credential: String,
}

struct AttemptResult {
    id: String,
    generation: u64,
    window: WindowId,
    result: Result<ConnectedSystem, String>,
}

pub(super) struct SystemConnections {
    // Never fall back to an empty writable registry after a corrupt-file error.
    registry: io::Result<SystemRegistry>,
    attempts: HashMap<String, Attempt>,
    connections: HashMap<String, BackgroundConnection>,
    next_generation: u64,
    tx: Sender<AttemptResult>,
    rx: Receiver<AttemptResult>,
    entries: Vec<PaletteSystemEntry>,
}

impl SystemConnections {
    pub(super) fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            registry: SystemRegistry::load(),
            attempts: HashMap::new(),
            connections: HashMap::new(),
            next_generation: 0,
            tx,
            rx,
            entries: Vec::new(),
        }
    }

    fn reload(&mut self) -> Result<(), String> {
        match &mut self.registry {
            Ok(registry) => registry.reload().map_err(|error| error.to_string()),
            Err(_) => {
                self.registry = SystemRegistry::load();
                self.registry
                    .as_ref()
                    .map(|_| ())
                    .map_err(ToString::to_string)
            }
        }
    }

    fn recipe(&self, id: &str) -> Option<SavedSystem> {
        self.registry.as_ref().ok()?.system(id).cloned()
    }
}

fn same_route(a: &SavedSystem, b: &SavedSystem) -> bool {
    a.id == b.id && a.target == b.target && a.ssh_args == b.ssh_args
}

fn accepts_connection(current: &SavedSystem, attempted: &SavedSystem) -> bool {
    current.enabled && same_route(current, attempted)
}

fn connection_status(
    running: bool,
    status: DaemonClientStatus,
) -> SystemConnectionStatus {
    if !running {
        return SystemConnectionStatus::Disconnected;
    }
    match status {
        DaemonClientStatus::Open => SystemConnectionStatus::Connected,
        DaemonClientStatus::Connecting | DaemonClientStatus::BackingOff => {
            SystemConnectionStatus::Connecting
        }
        DaemonClientStatus::Closed => SystemConnectionStatus::Disconnected,
    }
}

impl App<'_> {
    fn system_error(&mut self, window: WindowId, message: impl Into<String>) {
        if let Some(route) = self.router.routes.get_mut(&window) {
            route
                .window
                .screen
                .renderer
                .notifications
                .push(message.into(), NotificationLevel::Error);
            route.request_redraw();
        }
    }

    pub(super) fn open_system_manager(&mut self, window_id: WindowId) {
        let Some(route) = self.router.routes.get(&window_id) else {
            return;
        };
        if route.window.screen.renderer.modal.is_active() {
            return;
        }
        if let Err(error) = self.systems.reload() {
            self.system_error(
                window_id,
                format!("Could not load saved systems: {error}"),
            );
        }
        self.refresh_system_palettes();
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route
                .window
                .screen
                .renderer
                .command_palette
                .enter_systems_mode(self.systems.entries.clone());
            route.request_redraw();
        }
    }

    /// Details are a local registry/UI operation, never a connection request.
    pub(super) fn open_system_settings(&mut self, window: WindowId, id: &str) {
        let Some(route) = self.router.routes.get(&window) else {
            return;
        };
        if route.window.screen.renderer.modal.is_active() {
            return;
        }
        if let Err(error) = self.systems.reload() {
            self.system_error(window, format!("Could not load saved systems: {error}"));
            return;
        }
        self.refresh_system_palettes();
        let entry = self.systems.entries.iter().find(|entry| entry.id == id).cloned();
        if let Some(route) = self.router.routes.get_mut(&window) {
            let palette = &mut route.window.screen.renderer.command_palette;
            if let Some(entry) = entry {
                palette.enter_system_settings_mode(entry);
            } else {
                palette.enter_systems_mode(self.systems.entries.clone());
            }
            route.request_redraw();
        }
    }

    fn restore_system_view(&mut self, window: WindowId, settings: Option<&str>) {
        if let Some(id) = settings {
            self.open_system_settings(window, id);
        } else {
            self.open_system_manager(window);
        }
    }

    fn cancel_system_workspace_adoption(&mut self, window: WindowId) {
        self.pending_ssh_workspaces.remove(&window);
        self.server_switch_inflight.remove(&window);
        if let Some(session) = self.window_sessions.get_mut(&window) {
            session.pending_peer_adopt = None;
        }
    }

    fn open_system_ssh_workspace(&mut self, window: WindowId, id: &str) {
        if id == "local" {
            return;
        }
        if let Err(error) = self.systems.reload() {
            self.system_error(window, format!("Could not load saved systems: {error}"));
            return;
        }
        let Some(recipe) = self.systems.recipe(id) else {
            self.system_error(window, "Saved system no longer exists.");
            return;
        };
        // Latest explicit intent wins; retain a same-route authentication worker
        // so duplicate clicks coalesce through the normal attach pipeline.
        if self.ssh_attach_inflight.get(&window).is_some_and(|request| {
            !same_system_route(&request.target, &request.ssh_args, &recipe.target, &recipe.ssh_args)
        }) {
            self.ssh_attach_inflight.remove(&window);
        }
        self.cancel_system_workspace_adoption(window);
        if let Some(route) = self.router.routes.get_mut(&window) {
            route.window.screen.request_ssh_workspace_open(recipe.target, recipe.ssh_args);
            route.request_redraw();
        }
    }

    pub(super) fn process_system_request(
        &mut self,
        window_id: WindowId,
        request: crate::screen::SystemRequest,
    ) {
        use crate::screen::SystemRequest;
        if !self.router.routes.contains_key(&window_id) {
            return;
        }
        let settings = self.router.routes.get(&window_id).and_then(|route| {
            route.window.screen.renderer.command_palette
                .system_settings_id().map(str::to_owned)
        });
        match request {
            SystemRequest::Show => self.open_system_manager(window_id),
            SystemRequest::Settings { id } => self.open_system_settings(window_id, &id),
            SystemRequest::SshWorkspace { id } => self.open_system_ssh_workspace(window_id, &id),
            SystemRequest::Add { name, target } => {
                if let Err(error) = self.systems.reload() {
                    self.system_error(
                        window_id,
                        format!("Could not load saved systems: {error}"),
                    );
                    return;
                }
                let saved = self
                    .systems
                    .registry
                    .as_mut()
                    .map_err(|e| e.to_string())
                    .and_then(|registry| {
                        registry
                            .upsert(&name, &target, Vec::new())
                            .map_err(|e| e.to_string())
                    });
                match saved {
                    Ok(_) => {} // Saving a recipe never dials or changes activation intent.
                    Err(error) => self.system_error(
                        window_id,
                        format!("Could not save system: {error}"),
                    ),
                }
                self.refresh_system_palettes();
                self.open_system_manager(window_id);
            }
            SystemRequest::Connect { id } => {
                if id == "local" {
                    self.restore_system_view(window_id, settings.as_deref());
                    return;
                }
                if let Err(error) = self.systems.reload() {
                    self.system_error(
                        window_id,
                        format!("Could not load saved systems: {error}"),
                    );
                    return;
                }
                if let Some(recipe) = self.systems.recipe(&id) {
                    self.connect_saved_system(window_id, recipe);
                }
                self.refresh_system_palettes();
                self.restore_system_view(window_id, settings.as_deref());
            }
            SystemRequest::SetEnabled {
                id,
                enabled,
                discard,
            } => {
                if id == "local" {
                    return;
                }
                if !enabled {
                    self.disconnect_saved_system(window_id, &id, false, discard, true);
                } else {
                    if let Err(error) = self.systems.reload() {
                        self.system_error(
                            window_id,
                            format!("Could not load saved systems: {error}"),
                        );
                        return;
                    }
                    let saved = self
                        .systems
                        .registry
                        .as_mut()
                        .map_err(|error| error.to_string())
                        .and_then(|registry| {
                            registry
                                .set_enabled(&id, true)
                                .map_err(|error| error.to_string())
                        });
                    match saved {
                        Ok(recipe) => self.connect_saved_system(window_id, recipe),
                        Err(error) => self.system_error(
                            window_id,
                            format!("Could not activate system: {error}"),
                        ),
                    }
                }
                self.refresh_system_palettes();
                self.restore_system_view(window_id, settings.as_deref());
            }
            SystemRequest::Disconnect {
                id,
                remove,
                discard,
            } => {
                self.disconnect_saved_system(window_id, &id, remove, discard, false);
                self.restore_system_view(window_id, if remove { None } else { settings.as_deref() });
            }
        }
    }

    // Manager connections are background-only, including reconnects and reuse.
    // Workspace selection belongs exclusively to explicit workspace actions.
    fn connect_saved_system(&mut self, window: WindowId, recipe: SavedSystem) {
        // Consult current intent, not the worker/request's potentially stale copy.
        if !self
            .systems
            .recipe(&recipe.id)
            .is_some_and(|current| accepts_connection(&current, &recipe))
        {
            self.system_error(window, "Activate this saved system before connecting.");
            return;
        }
        if self.systems.attempts.contains_key(&recipe.id) {
            return;
        }
        let existing = self
            .ssh_attaches
            .iter_mut()
            .filter(|(_, attach)| {
                same_system_route(
                    &attach.alias,
                    &attach.ssh_args,
                    &recipe.target,
                    &recipe.ssh_args,
                )
            })
            .find_map(|(endpoint, attach)| {
                attach
                    .is_running()
                    .then(|| (endpoint.clone(), attach.credential.clone()))
            });
        if let Some((endpoint, _)) = &existing {
            if self
                .systems
                .connections
                .get(&recipe.id)
                .is_some_and(|connection| {
                    connection.endpoint == *endpoint
                        && connection.source.is_some()
                        && connection.connection.status() == DaemonClientStatus::Open
                })
            {
                return;
            }
        }
        // A single monotonic sequence prevents remove/re-add and reconnect ABA.
        self.systems.next_generation = self
            .systems
            .next_generation
            .checked_add(1)
            .expect("system connection generation exhausted");
        let generation = self.systems.next_generation;
        let cancelled = Arc::new(AtomicBool::new(false));
        self.systems.attempts.insert(
            recipe.id.clone(),
            Attempt {
                generation,
                window,
                recipe: recipe.clone(),
                cancelled: cancelled.clone(),
            },
        );
        let tx = self.systems.tx.clone();
        let proxy = self.event_proxy.clone();
        let id = recipe.id.clone();
        let thread_id = id.clone();
        let spawn = std::thread::Builder::new()
            .name("neoism-system-connect".into())
            .spawn(move || {
                let result = (|| {
                    if cancelled.load(Ordering::Acquire) {
                        return Err("Connection cancelled".into());
                    }
                    let (endpoint, credential, attach) = match existing {
                        Some((endpoint, credential)) => (endpoint, credential, None),
                        None => {
                            let attach = crate::ssh_hosts::attach_workspace_over_ssh(
                                &recipe.target,
                                recipe.ssh_args.clone(),
                            )
                            .map_err(|error| error.to_string())?;
                            (
                                attach.daemon_url.clone(),
                                attach.credential.clone(),
                                Some(attach),
                            )
                        }
                    };
                    if cancelled.load(Ordering::Acquire) {
                        return Err("Connection cancelled".into());
                    }
                    let connection = DesktopDaemonConnection::connect_with_token(
                        &endpoint,
                        Some(credential.clone()),
                        proxy.clone(),
                    )
                    .map_err(|error| error.to_string())?;
                    // Background links do not own workspace subscriptions or PTYs.
                    connection.set_parked(true);
                    if cancelled.load(Ordering::Acquire) {
                        return Err("Connection cancelled".into());
                    }
                    let source = discover_system_source(
                        recipe.id.clone(),
                        recipe.name.clone(),
                        generation,
                        &endpoint,
                        credential.clone(),
                    );
                    Ok(ConnectedSystem {
                        endpoint,
                        attach,
                        connection,
                        source,
                        credential,
                    })
                })();
                if cancelled.load(Ordering::Acquire) {
                    // Drop new tunnels/connections here, never revive them on the UI thread.
                    return;
                }
                let _ = tx.send(AttemptResult {
                    id: thread_id,
                    generation,
                    window,
                    result,
                });
                proxy.send_event(RioEventType::Rio(RioEvent::Render), unsafe {
                    WindowId::dummy()
                });
            });
        if let Err(error) = spawn {
            self.systems.attempts.remove(&id);
            self.system_error(
                window,
                format!("Could not start system connection: {error}"),
            );
        }
    }

    // Only authenticated DesktopDaemonConnections count. No HTTP health probes.
    fn system_endpoint_status(&self, endpoint: &str) -> DaemonClientStatus {
        let statuses = self
            .systems
            .connections
            .values()
            .filter(|connection| connection.endpoint == endpoint)
            .map(|connection| connection.connection.status())
            .chain(
                self.window_sessions
                    .values()
                    .flat_map(|session| {
                        std::iter::once(&session.connection)
                            .chain(session.parked_connections.values())
                    })
                    .filter(|connection| connection.endpoint() == endpoint)
                    .map(|connection| connection.status()),
            )
            .chain(
                self.bootstrap_daemon
                    .iter()
                    .filter(|connection| connection.endpoint() == endpoint)
                    .map(|connection| connection.status()),
            );
        let mut status = DaemonClientStatus::Closed;
        for candidate in statuses {
            if candidate == DaemonClientStatus::Open {
                return candidate;
            }
            if matches!(
                candidate,
                DaemonClientStatus::Connecting | DaemonClientStatus::BackingOff
            ) {
                status = candidate;
            }
        }
        status
    }

    fn disconnect_saved_system(
        &mut self,
        window: WindowId,
        id: &str,
        remove: bool,
        discard: bool,
        deactivate: bool,
    ) {
        if let Err(error) = self.systems.reload() {
            self.system_error(window, format!("Could not load saved systems: {error}"));
            return;
        }
        let recipe = self
            .systems
            .recipe(id)
            .or_else(|| {
                self.systems
                    .attempts
                    .get(id)
                    .map(|attempt| attempt.recipe.clone())
            })
            .or_else(|| {
                self.systems
                    .connections
                    .get(id)
                    .map(|connection| connection.recipe.clone())
            });
        let Some(recipe) = recipe else { return };
        let mut endpoints: Vec<_> = self
            .ssh_attaches
            .iter()
            .filter(|(_, attach)| {
                same_system_route(
                    &attach.alias,
                    &attach.ssh_args,
                    &recipe.target,
                    &recipe.ssh_args,
                )
            })
            .map(|(endpoint, _)| endpoint.clone())
            .collect();
        endpoints.extend(self.ssh_routes.iter().filter_map(
            |(endpoint, (target, args))| {
                same_system_route(target, args, &recipe.target, &recipe.ssh_args)
                    .then(|| endpoint.clone())
            },
        ));
        if let Some(connection) = self.systems.connections.get(id) {
            endpoints.push(connection.endpoint.clone());
        }
        endpoints.sort();
        endpoints.dedup();
        // Check ALL endpoints in ALL windows before cancelling attempts, leaving
        // any workspace, removing credentials, or dropping a tunnel guard.
        let dirty = !discard
            && self.router.routes.values().any(|route| {
                endpoints.iter().any(|endpoint| {
                    route.window.screen.has_unsaved_server_workspaces(endpoint)
                })
            });
        if dirty {
            if let Some(route) = self.router.routes.get_mut(&window) {
                route.window.screen.renderer.modal.open(ModalSpec {
                    title: if deactivate { "Deactivate system with unsaved edits?" } else if remove { "Remove system with unsaved edits?" } else { "Disconnect with unsaved edits?" }.into(),
                    body: "This system has unsaved edits in an open window. Keep it connected to save or copy them locally, or disconnect and discard those edits. Saved files on the remote host are not deleted.".into(),
                    meta: recipe.name.clone(), input: None, busy: false, blocking: true,
                    buttons: vec![
                        ModalButton::new("Keep connected", "Esc", ModalAction::Close),
                        ModalButton::new(if deactivate { "Deactivate and discard edits" } else { "Disconnect and discard edits" }, "", if deactivate { ModalAction::SystemDeactivateConfirm { id: id.into() } } else { ModalAction::SystemDisconnectConfirm { id: id.into(), remove } }),
                    ],
                });
                route.request_redraw();
            }
            return;
        }
        // The dirty preflight must precede persistence; persistence must succeed
        // before touching transports, owner tokens, or pending work.
        if deactivate {
            let result = self
                .systems
                .registry
                .as_mut()
                .map_err(|error| error.to_string())
                .and_then(|registry| {
                    registry
                        .set_enabled(id, false)
                        .map_err(|error| error.to_string())
                });
            if let Err(error) = result {
                self.system_error(
                    window,
                    format!("Could not deactivate system: {error}"),
                );
                return;
            }
        }
        // Snapshot pending endpoint ownership before leave removes tunnel guards.
        let mut cancelled_windows: Vec<_> = self
            .ssh_attach_inflight
            .iter()
            .filter(|(_, request)| {
                same_system_route(
                    &request.target,
                    &request.ssh_args,
                    &recipe.target,
                    &recipe.ssh_args,
                )
            })
            .map(|(window, _)| *window)
            .collect();
        cancelled_windows.extend(
            self.pending_ssh_workspaces
                .iter()
                .filter(|(_, pending)| endpoints.contains(&pending.daemon_url))
                .map(|(window, _)| *window),
        );
        for endpoint in &endpoints {
            // Integration contract: SSH endpoints leave every window, and the
            // shared guard is removed only once all of those leaves succeed.
            if !self.leave_server_connection_with_discard(window, endpoint, discard) {
                self.refresh_system_palettes();
                return;
            }
        }
        self.ssh_attach_inflight.retain(|_, request| {
            !same_system_route(
                &request.target,
                &request.ssh_args,
                &recipe.target,
                &recipe.ssh_args,
            )
        });
        self.pending_ssh_workspaces
            .retain(|_, pending| !endpoints.contains(&pending.daemon_url));
        for (id, route) in self.router.routes.iter_mut() {
            // Helper returns true if the queue is now empty, false if an
            // unrelated queued SSH request must retain its replacement state.
            let empty = route
                .window
                .screen
                .cancel_queued_ssh_workspace_attach(&recipe.target, &recipe.ssh_args);
            if empty && cancelled_windows.contains(id) {
                route.window.screen.cancel_ssh_workspace_replacement();
                route.request_redraw();
            }
        }
        self.systems.attempts.remove(id); // invalidates any queued/late result
        self.systems.connections.remove(id);
        for endpoint in endpoints {
            self.ssh_attaches.remove(&endpoint);
        }
        if remove {
            let result = self
                .systems
                .registry
                .as_mut()
                .map_err(|e| e.to_string())
                .and_then(|registry| registry.remove(id).map_err(|e| e.to_string()));
            if let Err(error) = result {
                self.system_error(
                    window,
                    format!("Disconnected, but could not remove saved system: {error}"),
                );
            }
        }
        self.refresh_system_palettes();
    }

    pub(super) fn pump_system_connections(&mut self) {
        // Closing the initiating window invalidates even successful queued work.
        self.systems
            .attempts
            .retain(|_, attempt| self.router.routes.contains_key(&attempt.window));
        while let Ok(pending) = self.systems.rx.try_recv() {
            let valid = self
                .systems
                .attempts
                .get(&pending.id)
                .is_some_and(|attempt| {
                    attempt.generation == pending.generation
                        && attempt.window == pending.window
                        && self.router.routes.contains_key(&pending.window)
                });
            if !valid {
                continue;
            }
            // A disk read per COMPLETION (not per frame) also rejects recipes
            // removed by another desktop process while SSH was authenticating.
            let reload = self.systems.reload();
            let attempt = self
                .systems
                .attempts
                .remove(&pending.id)
                .expect("validated attempt");
            if let Err(error) = reload {
                self.system_error(
                    pending.window,
                    format!("Could not verify saved system: {error}"),
                );
                continue;
            }
            if !self
                .systems
                .recipe(&pending.id)
                .is_some_and(|recipe| accepts_connection(&recipe, &attempt.recipe))
            {
                continue;
            }
            match pending.result {
                Err(error) => self.system_error(
                    pending.window,
                    format!("Could not connect to {}: {error}", attempt.recipe.name),
                ),
                Ok(mut connected) => {
                    // Quick SSH can finish while our worker is authenticating.
                    // Prefer its live guard instead of installing a duplicate.
                    let reuse = connected.attach.is_some()
                        && self.ssh_attaches.iter_mut().any(|(endpoint, attach)| {
                            *endpoint != connected.endpoint
                                && same_system_route(
                                    &attach.alias,
                                    &attach.ssh_args,
                                    &attempt.recipe.target,
                                    &attempt.recipe.ssh_args,
                                )
                                && attach.is_running()
                        });
                    if reuse {
                        drop(connected);
                        self.connect_saved_system(pending.window, attempt.recipe.clone());
                        continue;
                    }
                    if let Some(attach) = connected.attach.take() {
                        self.ssh_routes.insert(
                            connected.endpoint.clone(),
                            (attach.alias.clone(), attach.ssh_args.clone()),
                        );
                        self.ssh_attaches
                            .entry(connected.endpoint.clone())
                            .or_insert(attach);
                    }
                    let running = self
                        .ssh_attaches
                        .get_mut(&connected.endpoint)
                        .is_some_and(|attach| {
                            same_system_route(
                                &attach.alias,
                                &attach.ssh_args,
                                &attempt.recipe.target,
                                &attempt.recipe.ssh_args,
                            ) && attach.is_running()
                        });
                    if !running {
                        continue;
                    }
                    self.systems.connections.remove(&pending.id);
                    let source = match connected.source {
                        Ok(source) => {
                            crate::neoism::agent::register_agent_server_credential(
                                &source.server,
                                Some(&connected.credential),
                            );
                            Some(source)
                        }
                        Err(error) => {
                            self.system_error(
                                pending.window,
                                format!("{}: {error}", attempt.recipe.name),
                            );
                            None
                        }
                    };
                    self.systems.connections.insert(
                        pending.id,
                        BackgroundConnection {
                            recipe: attempt.recipe.clone(),
                            endpoint: connected.endpoint,
                            connection: connected.connection,
                            source,
                        },
                    );
                }
            }
        }
        // A registry reload may have observed deactivation/removal by another
        // desktop process. Never retain or publish its agent authority.
        let registry = self.systems.registry.as_ref().ok();
        self.systems.attempts.retain(|id, attempt| {
            registry
                .and_then(|registry| registry.system(id))
                .is_some_and(|current| accepts_connection(current, &attempt.recipe))
        });
        self.systems.connections.retain(|id, connection| {
            registry
                .and_then(|registry| registry.system(id))
                .is_some_and(|current| accepts_connection(current, &connection.recipe))
        });
        for connection in self.systems.connections.values() {
            // Discard hello/snapshots/presence here. They must never be applied
            // to the window's currently selected (possibly local) workspace.
            drop(connection.connection.drain_messages());
        }
        let sources: Vec<_> = self
            .systems
            .connections
            .values()
            .filter(|connection| {
                connection.connection.status() == DaemonClientStatus::Open
            })
            .filter_map(|connection| connection.source.clone())
            .filter(AgentSystemSource::authorized)
            .collect();
        for route in self.router.routes.values_mut() {
            route
                .window
                .screen
                .set_agent_system_sources(sources.clone());
        }
        self.refresh_system_palettes();
    }

    fn refresh_system_palettes(&mut self) {
        let local_status = self
            .home_daemon_endpoint
            .as_deref()
            .map(|endpoint| {
                connection_status(true, self.system_endpoint_status(endpoint))
            })
            .unwrap_or(SystemConnectionStatus::Disconnected);
        let mut entries = vec![PaletteSystemEntry {
            id: "local".into(),
            name: "This computer".into(),
            address: "Local".into(),
            local: true,
            enabled: true,
            status: local_status,
        }];
        let recipes = self
            .systems
            .registry
            .as_ref()
            .map(|registry| registry.systems().to_vec())
            .unwrap_or_default();
        for recipe in recipes {
            let endpoints: Vec<_> = self
                .ssh_attaches
                .iter_mut()
                .filter(|(_, attach)| {
                    same_system_route(
                        &attach.alias,
                        &attach.ssh_args,
                        &recipe.target,
                        &recipe.ssh_args,
                    )
                })
                .filter_map(|(endpoint, attach)| {
                    attach.is_running().then(|| endpoint.clone())
                })
                .collect();
            let connecting = self.systems.attempts.contains_key(&recipe.id)
                || self.ssh_attach_inflight.values().any(|request| {
                    same_system_route(
                        &request.target,
                        &request.ssh_args,
                        &recipe.target,
                        &recipe.ssh_args,
                    )
                });
            let mut status = if connecting {
                SystemConnectionStatus::Connecting
            } else {
                SystemConnectionStatus::Disconnected
            };
            for endpoint in endpoints {
                let candidate =
                    connection_status(true, self.system_endpoint_status(&endpoint));
                if candidate == SystemConnectionStatus::Connected {
                    status = candidate;
                    break;
                }
                if candidate == SystemConnectionStatus::Connecting {
                    status = candidate;
                }
            }
            entries.push(PaletteSystemEntry {
                id: recipe.id,
                name: recipe.name,
                address: recipe.target,
                local: false,
                enabled: recipe.enabled,
                status,
            });
        }
        if entries == self.systems.entries {
            return;
        }
        self.systems.entries = entries;
        for route in self.router.routes.values_mut() {
            let palette = &mut route.window.screen.renderer.command_palette;
            let changed = if palette.system_settings_id().is_some() {
                palette.refresh_system_settings(self.systems.entries.clone())
            } else {
                palette.refresh_systems(self.systems.entries.clone())
            };
            if changed {
                route.request_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manager_connect_and_add_have_no_workspace_activation_intent() {
        // Guard the request, connect/reuse, and completion paths without starting
        // SSH or constructing a desktop window. Disconnect intentionally retains
        // its separate all-window workspace cleanup responsibilities.
        let source = include_str!("systems.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let requests_and_connect = production
            .split("pub(super) fn process_system_request(")
            .nth(1)
            .unwrap()
            .split("// Only authenticated DesktopDaemonConnections count.")
            .next()
            .unwrap();
        let completion = production
            .split("pub(super) fn pump_system_connections(")
            .nth(1)
            .unwrap()
            .split("fn refresh_system_palettes(")
            .next()
            .unwrap();
        let add = requests_and_connect.split("SystemRequest::Add { name, target } => {").nth(1).unwrap()
            .split("SystemRequest::Connect").next().unwrap();
        let connect = requests_and_connect.split("SystemRequest::Connect { id } => {").nth(1).unwrap()
            .split("SystemRequest::SetEnabled").next().unwrap();
        let active = requests_and_connect.split("SystemRequest::SetEnabled {").nth(1).unwrap()
            .split("SystemRequest::Disconnect").next().unwrap();
        let background = requests_and_connect.split("fn connect_saved_system(").nth(1).unwrap();
        for path in [add, connect, active, background, completion] {
            for forbidden in [
                "open_if_connected",
                "open_on_ready",
                "begin_ssh_workspace_switch",
                "switch_window_server",
                "cancel_system_activation",
                "cancel_pending_ssh_workspace_activation",
                "ssh_attach_inflight.remove",
                "pending_ssh_workspaces.remove",
                "server_switch_inflight.remove",
            ] {
                assert!(
                    !path.contains(forbidden),
                    "manager must not use {forbidden}"
                );
            }
        }
        assert!(requests_and_connect.contains("connection.set_parked(true)"));
        assert!(completion.contains("drop(connection.connection.drain_messages())"));
    }

    #[test]
    fn system_settings_navigation_does_not_dial_or_change_activation() {
        let production = include_str!("systems.rs").split("#[cfg(test)]").next().unwrap();
        let details = production.split("pub(super) fn open_system_settings(").nth(1).unwrap()
            .split("fn restore_system_view(").next().unwrap();
        assert!(details.contains("enter_system_settings_mode"));
        for forbidden in ["connect_saved_system", "request_ssh_workspace", "set_enabled", "switch_window_server"] {
            assert!(!details.contains(forbidden), "settings navigation must not use {forbidden}");
        }
    }

    #[test]
    fn explicit_system_ssh_preserves_activation_args_and_local_workspace() {
        let production = include_str!("systems.rs").split("#[cfg(test)]").next().unwrap();
        let ssh = production.split("fn open_system_ssh_workspace(").nth(1).unwrap()
            .split("pub(super) fn process_system_request(").next().unwrap();
        assert!(ssh.contains("self.systems.recipe(id)"));
        assert!(ssh.contains("request_ssh_workspace_open(recipe.target, recipe.ssh_args)"));
        for forbidden in ["set_enabled", "accepts_connection", "connect_saved_system", "begin_ssh_workspace_switch"] {
            assert!(!ssh.contains(forbidden), "explicit SSH must not use {forbidden}");
        }
        let layout = include_str!("../screen/daemon_layout.rs");
        let open = layout.split("pub fn request_ssh_workspace_open(").nth(1).unwrap()
            .split("pub fn take_ssh_workspace_attach(").next().unwrap();
        assert!(open.contains("self.pending_ssh_workspace_replacement = None"));
        assert!(!open.contains("current_workspace_id()"));
        let takeover = layout.split("pub fn request_ssh_workspace_attach(").nth(1).unwrap()
            .split("pub fn request_ssh_workspace_open(").next().unwrap();
        assert!(takeover.contains("self.pending_ssh_workspace_replacement = self.current_workspace_id()"));
    }

    #[test]
    fn disabled_recipe_rejects_late_completion_and_retry() {
        let attempted = SavedSystem {
            id: "host".into(),
            name: "Host".into(),
            target: "host".into(),
            ssh_args: vec![],
            enabled: true,
        };
        let mut current = attempted.clone();
        assert!(accepts_connection(&current, &attempted));
        current.enabled = false;
        assert!(!accepts_connection(&current, &attempted));
        assert!(!accepts_connection(&current, &current));
        current.enabled = true;
        current.name = "Renamed".into();
        assert!(accepts_connection(&current, &attempted));
    }

    #[test]
    fn add_only_saves_and_connect_checks_intent_before_dial() {
        let source = include_str!("systems.rs");
        let production = source.split("#[cfg(test)]").next().unwrap();
        let add = production
            .split("SystemRequest::Add { name, target } => {")
            .nth(1)
            .unwrap()
            .split("SystemRequest::Connect")
            .next()
            .unwrap();
        assert!(add.contains(".upsert("));
        for forbidden in [
            "connect_saved_system",
            "attach_workspace_over_ssh",
            "discover_system_source",
            "set_enabled",
        ] {
            assert!(!add.contains(forbidden), "Add must not use {forbidden}");
        }
        let connect = production.split("fn connect_saved_system(").nth(1).unwrap();
        assert!(
            connect.find("accepts_connection").unwrap()
                < connect.find(".ssh_attaches").unwrap()
        );
        let completion = production
            .split("pub(super) fn pump_system_connections(")
            .nth(1)
            .unwrap();
        assert!(
            completion.find("accepts_connection").unwrap()
                < completion.find("match pending.result").unwrap()
        );
    }

    #[test]
    fn deactivation_preflight_and_write_precede_all_teardown() {
        let source = include_str!("systems.rs");
        let disconnect = source
            .split("fn disconnect_saved_system(")
            .nth(1)
            .unwrap()
            .split("pub(super) fn pump_system_connections(")
            .next()
            .unwrap();
        let dirty = disconnect.find("let dirty = !discard").unwrap();
        let persist = disconnect.find(".set_enabled(id, false)").unwrap();
        assert!(dirty < persist);
        assert!(disconnect[dirty..persist].contains("self.router.routes.values().any"));
        assert!(
            disconnect[dirty..persist].contains("ModalAction::SystemDeactivateConfirm")
        );
        assert!(disconnect[dirty..persist].contains("return;"));
        let failed_write = disconnect[persist..]
            .find("if let Err(error) = result")
            .unwrap()
            + persist;
        for teardown in [
            "leave_server_connection_with_discard",
            "self.ssh_attach_inflight.retain",
            "self.pending_ssh_workspaces",
            "self.systems.attempts.remove",
            "self.systems.connections.remove",
            "self.ssh_attaches.remove",
        ] {
            let position = disconnect.find(teardown).unwrap();
            assert!(failed_write < position, "write must precede {teardown}");
            assert!(disconnect[failed_write..position].contains("return;"));
        }
    }

    #[test]
    fn dropping_an_attempt_cancels_its_background_worker() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let attempt = Attempt {
            generation: 1,
            window: unsafe { WindowId::dummy() },
            recipe: SavedSystem {
                id: "system".into(),
                name: "Buildbox".into(),
                target: "buildbox".into(),
                ssh_args: Vec::new(),
                enabled: true,
            },
            cancelled: cancelled.clone(),
        };
        assert!(!cancelled.load(Ordering::Acquire));
        drop(attempt);
        assert!(cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn tunnel_without_authenticated_socket_is_not_connected() {
        assert_eq!(
            connection_status(true, DaemonClientStatus::Closed),
            SystemConnectionStatus::Disconnected
        );
        assert_eq!(
            connection_status(true, DaemonClientStatus::Connecting),
            SystemConnectionStatus::Connecting
        );
        assert_eq!(
            connection_status(true, DaemonClientStatus::Open),
            SystemConnectionStatus::Connected
        );
    }

    #[test]
    fn dead_tunnel_overrides_socket_state() {
        for status in [
            DaemonClientStatus::Open,
            DaemonClientStatus::Connecting,
            DaemonClientStatus::BackingOff,
            DaemonClientStatus::Closed,
        ] {
            assert_eq!(
                connection_status(false, status),
                SystemConnectionStatus::Disconnected
            );
        }
    }

    #[test]
    fn changed_transport_invalidates_attempt_but_rename_does_not() {
        let recipe = SavedSystem {
            id: "one".into(),
            name: "Host".into(),
            target: "user@host".into(),
            ssh_args: vec![],
            enabled: true,
        };
        let mut changed = recipe.clone();
        changed.name = "Renamed".into();
        assert!(same_route(&recipe, &changed));
        changed.ssh_args = vec!["-p".into(), "2222".into()];
        assert!(!same_route(&recipe, &changed));
        changed = recipe.clone();
        changed.id = "re-added".into();
        assert!(!same_route(&recipe, &changed));
    }
}
