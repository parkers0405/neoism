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
use crate::ssh_hosts::DaemonAttach;
use crate::system_registry::{same_system_route, SavedSystem, SystemRegistry};

struct Attempt {
    generation: u64,
    window: WindowId,
    recipe: SavedSystem,
    cancelled: Arc<AtomicBool>,
    open_on_ready: Option<WindowId>,
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
}

struct ConnectedSystem {
    endpoint: String,
    // Only newly bootstrapped tunnels travel through the channel. Reused guards
    // stay in App::ssh_attaches throughout authentication.
    attach: Option<DaemonAttach>,
    connection: DesktopDaemonConnection,
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

    pub(super) fn cancel_system_activation(&mut self, window: WindowId) {
        for attempt in self.systems.attempts.values_mut() {
            if attempt.open_on_ready == Some(window) {
                attempt.open_on_ready = None;
            }
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
        match request {
            SystemRequest::Show => self.open_system_manager(window_id),
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
                    // Add must never implicitly replace the current workspace,
                    // even when upsert found an already-connected recipe.
                    Ok(recipe) => self.connect_saved_system(window_id, recipe, false),
                    Err(error) => self.system_error(
                        window_id,
                        format!("Could not save system: {error}"),
                    ),
                }
                self.refresh_system_palettes();
            }
            SystemRequest::Connect { id } => {
                self.ssh_attach_inflight.remove(&window_id);
                self.pending_ssh_workspaces.remove(&window_id);
                self.server_switch_inflight.remove(&window_id);
                self.cancel_system_activation(window_id);
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route
                        .window
                        .screen
                        .cancel_pending_ssh_workspace_activation();
                }
                if id == "local" {
                    if let Some(endpoint) = self.home_daemon_endpoint.clone() {
                        self.switch_window_server(window_id, &endpoint, None, None);
                    }
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
                    self.connect_saved_system(window_id, recipe, true);
                }
                self.refresh_system_palettes();
                self.open_system_manager(window_id);
            }
            SystemRequest::Disconnect {
                id,
                remove,
                discard,
            } => {
                self.disconnect_saved_system(window_id, &id, remove, discard);
            }
        }
    }

    fn connect_saved_system(
        &mut self,
        window: WindowId,
        recipe: SavedSystem,
        open_if_connected: bool,
    ) {
        if let Some(attempt) = self.systems.attempts.get_mut(&recipe.id) {
            if open_if_connected {
                attempt.open_on_ready = Some(window);
            }
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
                attach.is_running().then(|| {
                    (
                        endpoint.clone(),
                        attach.workspace_id.clone(),
                        attach.credential.clone(),
                    )
                })
            });
        if let Some((endpoint, workspace, credential)) = &existing {
            let authenticated =
                self.system_endpoint_status(endpoint) == DaemonClientStatus::Open;
            if authenticated && open_if_connected {
                self.begin_ssh_workspace_switch(
                    window,
                    recipe.target.clone(),
                    endpoint.clone(),
                    workspace.clone(),
                    credential.clone(),
                );
                return;
            }
            if self
                .systems
                .connections
                .get(&recipe.id)
                .is_some_and(|connection| {
                    connection.endpoint == *endpoint
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
                open_on_ready: open_if_connected.then_some(window),
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
                        Some((endpoint, _, credential)) => (endpoint, credential, None),
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
                        Some(credential),
                        proxy.clone(),
                    )
                    .map_err(|error| error.to_string())?;
                    // Background links do not own workspace subscriptions or PTYs.
                    connection.set_parked(true);
                    Ok(ConnectedSystem {
                        endpoint,
                        attach,
                        connection,
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
                    title: if remove { "Remove system with unsaved edits?" } else { "Disconnect with unsaved edits?" }.into(),
                    body: "This system has unsaved edits in an open window. Keep it connected to save or copy them locally, or disconnect and discard those edits. Saved files on the remote host are not deleted.".into(),
                    meta: recipe.name.clone(), input: None, busy: false, blocking: true,
                    buttons: vec![
                        ModalButton::new("Keep connected", "Esc", ModalAction::Close),
                        ModalButton::new("Disconnect and discard edits", "", ModalAction::SystemDisconnectConfirm { id: id.into(), remove }),
                    ],
                });
                route.request_redraw();
            }
            return;
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
                .is_some_and(|recipe| same_route(&recipe, &attempt.recipe))
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
                        self.connect_saved_system(
                            attempt.open_on_ready.unwrap_or(pending.window),
                            attempt.recipe.clone(),
                            attempt.open_on_ready.is_some(),
                        );
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
                    self.systems.connections.insert(
                        pending.id,
                        BackgroundConnection {
                            recipe: attempt.recipe.clone(),
                            endpoint: connected.endpoint,
                            connection: connected.connection,
                        },
                    );
                    if let Some(window) = attempt.open_on_ready {
                        if self.router.routes.contains_key(&window) {
                            self.connect_saved_system(
                                window,
                                attempt.recipe.clone(),
                                true,
                            );
                        }
                    }
                }
            }
        }
        for connection in self.systems.connections.values() {
            // Discard hello/snapshots/presence here. They must never be applied
            // to the window's currently selected (possibly local) workspace.
            drop(connection.connection.drain_messages());
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
                status,
            });
        }
        if entries == self.systems.entries {
            return;
        }
        self.systems.entries = entries;
        for route in self.router.routes.values_mut() {
            let palette = &mut route.window.screen.renderer.command_palette;
            if palette.systems_mode()
                && palette.refresh_systems(self.systems.entries.clone())
            {
                route.request_redraw();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
            },
            cancelled: cancelled.clone(),
            open_on_ready: None,
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
