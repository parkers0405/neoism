//! Application lifecycle for temporary servers and endpoint-scoped leaving.
use super::Application;
use neoism_ui::panels::notifications::NotificationLevel;
use neoism_window::window::WindowId;

pub(super) struct PendingFastHost {
    pub window_id: WindowId,
    pub result: Result<crate::fast_servers::FastHost, String>,
}

fn open_fast_server_message(
    palette: &mut neoism_ui::panels::command_palette::CommandPalette,
    modal: &mut neoism_ui::widgets::modal::UniversalModal,
    title: &str,
    body: impl Into<String>,
) {
    // Closing through the palette API drops its cursor and drag state too.
    palette.set_enabled(false);
    if title == "Fast Server ready" {
        modal.open_fast_server_invite(body.into());
    } else {
        modal.open_message(title, body);
    }
}

fn validate_fast_join_dial(join: &crate::fast_servers::FastJoin) -> Result<(), String> {
    if join
        .token
        .as_deref()
        .is_none_or(|token| token.trim().is_empty())
    {
        return Err("Fast Join requires a password or an invite with an access code. Ask the host for the password or complete invite link.".into());
    }
    Ok(())
}

fn competing_selection(
    target: Option<(&str, Option<&str>, Option<&str>)>,
    endpoint: &str,
    server_id: &str,
    credential: Option<&str>,
) -> bool {
    target.is_some_and(|(target, id, token)| {
        target != endpoint || id != Some(server_id) || token != credential
    })
}

#[cfg(test)]
mod selection_tests {
    use super::{competing_selection, open_fast_server_message};

    #[test]
    fn fast_join_submission_requires_auth_and_preserves_password_bytes() {
        let bare = "https://team.example/";
        let normalize = |target: &str, password: Option<String>| {
            crate::fast_servers::normalize_fast_join(
                target.into(),
                password.filter(|value| !value.trim().is_empty()),
            )
            .unwrap()
        };
        for password in [None, Some(String::new()), Some(" \t ".into())] {
            let join = normalize(bare, password);
            // Metadata is available for owned/adopted workspace lookup even
            // without invite auth; only a subsequent dial is rejected.
            assert_eq!(join.endpoint, "wss://team.example/session");
            assert!(join.token.is_none());
            let error = super::validate_fast_join_dial(&join)
                .expect_err("missing authentication must fail before dialing");
            assert!(error.contains("password or an invite"));
        }
        let password = "  actual secret  ";
        let join = normalize(bare, Some(password.into()));
        assert_eq!(join.token.as_deref(), Some(password));
        super::validate_fast_join_dial(&join).unwrap();
        let capability = "a".repeat(64);
        let invite = format!("{bare}#neoism-fast={capability}");
        for password in [None, Some("  ".into())] {
            let join = normalize(&invite, password);
            assert_eq!(join.token.as_deref(), Some(capability.as_str()));
            super::validate_fast_join_dial(&join).unwrap();
        }
        // Defense in depth: a normalized blank token cannot authorize a dial.
        let join =
            crate::fast_servers::normalize_fast_join(bare.into(), Some("  ".into()))
                .unwrap();
        assert!(super::validate_fast_join_dial(&join).is_err());
        // Identity comparisons intentionally still accept public URLs.
        assert!(crate::fast_servers::normalize_fast_join(bare.into(), None).is_ok());
    }

    #[test]
    fn fast_server_dialogs_close_an_open_servers_picker() {
        use neoism_ui::panels::command_palette::CommandPalette;
        use neoism_ui::widgets::modal::UniversalModal;

        let mut palette = CommandPalette::new();
        let mut modal = UniversalModal::new();
        for title in ["Fast Server ready", "Fast Server failed"] {
            palette.enter_servers_mode(Vec::new());
            assert!(palette.is_enabled());
            open_fast_server_message(&mut palette, &mut modal, title, "Details");
            assert!(!palette.is_enabled());
            assert!(modal.is_active());
            assert!(modal.is_blocking());
            if title == "Fast Server ready" {
                assert!(
                    matches!(modal.selected_action(), Some(neoism_ui::widgets::modal::ModalAction::CopyInvite { url }) if url == "Details")
                );
            }
            modal.close();
        }
    }
    #[test]
    fn focus_supersedes_competitor_but_preserves_matching_restoration() {
        let endpoint = "wss://a.example/session";
        assert!(competing_selection(
            Some(("wss://b.example/session", Some("b"), Some("b-secret"))),
            endpoint,
            "a",
            Some("a-secret")
        ));
        assert!(!competing_selection(
            Some((endpoint, Some("a"), Some("a-secret"))),
            endpoint,
            "a",
            Some("a-secret")
        ));
        assert!(!competing_selection(None, endpoint, "a", Some("a-secret")));
        // Same origin does not mean same identity or guest capability.
        assert!(competing_selection(
            Some((endpoint, Some("saved"), Some("master"))),
            endpoint,
            "fast-a",
            Some("guest")
        ));
        assert!(competing_selection(
            Some((endpoint, Some("a"), Some("old"))),
            endpoint,
            "a",
            Some("a-secret")
        ));
    }
}

impl Application<'_> {
    pub(in crate::app) fn finish_fast_host_progress(&mut self, window_id: WindowId) {
        if let Some(handle) = self.fast_host_pending.remove(&window_id) {
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route
                    .window
                    .screen
                    .renderer
                    .notifications
                    .finish_progress(handle);
                route.request_redraw();
            }
        }
    }

    /// Focus an adopted workspace and supersede a competing selection even if
    /// the tab is already current (Screen then emits no peer-switch request).
    /// Restore ONLY from its known active/parked transport, not invite or saved
    /// credentials. A needed same-endpoint restoration generation is preserved.
    pub(super) fn focus_fast_workspace(
        &mut self,
        window: WindowId,
        index: usize,
        endpoint: &str,
    ) -> bool {
        let Some(route) = self.router.routes.get(&window) else {
            return false;
        };
        let Some(session) = self.window_sessions.get(&window) else {
            return false;
        };
        let active = session.connection.endpoint() == endpoint;
        let transport = if active {
            Some(&session.connection)
        } else {
            session.parked_connections.get(endpoint)
        };
        let Some(transport) = transport else {
            return false;
        };
        let credential = transport.token().map(str::to_owned);
        let server_id = if active {
            session.active_server_id.clone()
        } else {
            self.server_registry
                .known_transport_server_id(endpoint, transport.token())
        };
        let Some(server_id) = server_id else {
            return false;
        };
        let unavailable = route.window.screen.fast_invite_ui_busy()
            || (!active && route.window.screen.has_unsaved_server_buffers());
        if competing_selection(
            self.server_switch_inflight.get(&window).map(|request| {
                (
                    request.endpoint.as_str(),
                    request.server_id.as_deref(),
                    request.credential.as_deref(),
                )
            }),
            endpoint,
            &server_id,
            credential.as_deref(),
        ) {
            self.server_switch_inflight.remove(&window);
        }
        if unavailable {
            return false;
        }
        let route = self.router.routes.get_mut(&window).unwrap();
        route.window.screen.select_top_level_workspace_at(index);
        route.window.winit_window.focus_window();
        route.request_redraw();
        self.queue_invite_activation(window, endpoint);
        if !active
            && !self
                .server_switch_inflight
                .get(&window)
                .is_some_and(|request| request.endpoint == endpoint)
        {
            self.switch_window_server(window, endpoint, credential, Some(server_id));
        }
        true
    }

    pub(super) fn add_fast_server(
        &mut self,
        window_id: WindowId,
        host: bool,
        target: String,
        name: Option<String>,
        password: Option<String>,
    ) {
        if !host {
            // Normalize metadata before reuse lookup. Blank input is absence,
            // not a replacement for an invite capability; preserve all bytes
            // of a nonblank password.
            let password = password.filter(|value| !value.trim().is_empty());
            let join = match crate::fast_servers::normalize_fast_join(target, password) {
                Ok(join) => join,
                Err(error) => {
                    self.fast_server_error(window_id, error);
                    return;
                }
            };
            // Public host invites are guest origins; map our own origin back
            // to the already-open local workspace instead of joining ourselves.
            let host_endpoint = self.fast_hosts.values().find_map(|host| {
                crate::fast_servers::normalize_fast_join(host.public_url.clone(), None)
                    .ok()
                    .filter(|public| public.endpoint == join.endpoint)
                    .map(|_| host.endpoint.clone())
            });
            let existing_endpoint = host_endpoint.as_deref().unwrap_or(&join.endpoint);
            let existing_workspace = self.router.routes.iter().find_map(|(id, route)| {
                route
                    .window
                    .screen
                    .fast_invite_workspace_index(existing_endpoint)
                    .map(|index| (*id, index))
            });
            if let Some((owner, index)) = existing_workspace {
                if !self.focus_fast_workspace(owner, index, existing_endpoint) {
                    self.fast_server_error(window_id, "Finish the active dialog or save changes before opening this workspace".into());
                    self.invite_join_failed(window_id, &join.endpoint);
                    return;
                }
                self.invite_join_succeeded(window_id, &join.endpoint);
                return;
            }
            if host_endpoint.is_some() {
                self.fast_server_error(
                    window_id,
                    "Already hosting this workspace; wait for it to finish opening"
                        .into(),
                );
                self.invite_join_failed(window_id, &join.endpoint);
                return;
            }
            // Only a real dial needs invite authentication. Workspace focus
            // above restores exclusively from its authoritative live transport.
            if let Err(error) = validate_fast_join_dial(&join) {
                self.invite_join_failed(window_id, &join.endpoint);
                self.fast_server_error(window_id, error);
                return;
            }
            // Reuse a temporary entry on retry/reattach, but NEVER fetch a
            // persistent saved credential for an untrusted invite endpoint.
            if let Some(server) = self
                .server_registry
                .servers()
                .iter()
                .find(|server| {
                    server.endpoint == join.endpoint
                        && self.server_registry.is_temporary(&server.id)
                })
                .cloned()
            {
                self.switch_window_server(
                    window_id,
                    &server.endpoint,
                    join.token,
                    Some(server.id),
                );
                return;
            }
            let name = name.as_deref().unwrap_or(&join.name);
            match self.server_registry.add_temporary(
                &join.endpoint,
                Some(name),
                join.token.as_deref(),
                false,
            ) {
                Ok(server) => {
                    self.switch_window_server(
                        window_id,
                        &server.endpoint,
                        join.token,
                        Some(server.id),
                    );
                    if !self.is_invite_join_attempt(window_id, &join.endpoint) {
                        self.open_server_manager(window_id);
                    }
                }
                Err(error) => self.fast_server_error(window_id, error),
            }
            return;
        }
        if let Err(error) =
            crate::fast_servers::validate_host_password(password.as_deref())
        {
            self.fast_server_error(window_id, error);
            return;
        }
        if self.fast_host_pending.contains_key(&window_id) {
            self.fast_server_error(
                window_id,
                "A Fast Server is already starting in this window".into(),
            );
            return;
        }
        let Some(route) = self.router.routes.get_mut(&window_id) else {
            return;
        };
        let progress = route
            .window
            .screen
            .renderer
            .notifications
            .start_progress("Starting Fast Server...");
        self.fast_host_pending.insert(window_id, progress);
        route.request_redraw();
        let agent_server = crate::agent_server::configured_server();
        let tx = self.fast_host_tx.clone();
        let proxy = self.event_proxy.clone();
        if let Err(error) = std::thread::Builder::new()
            .name("neoism-fast-host-start".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    if !crate::agent_server::is_owned_local_endpoint(&agent_server) {
                        Err("Fast Host requires the local Agent service for this computer".into())
                    } else {
                        crate::agent_server::ensure_started_for_request(&agent_server)
                            .and_then(|_| {
                                crate::fast_servers::create_fast_server(
                                    target,
                                    name,
                                    password,
                                    agent_server,
                                )
                            })
                    }
                }))
                .unwrap_or_else(|_| Err("Fast Host startup worker panicked".into()));
                let _ = tx.send(PendingFastHost { window_id, result });
                proxy.send_event(
                    neoism_backend::event::RioEvent::Render.into(),
                    window_id,
                );
            })
        {
            self.finish_fast_host_progress(window_id);
            self.fast_server_error(window_id, error.to_string());
        }
    }

    pub(super) fn drain_fast_host_results(&mut self) {
        while let Ok(PendingFastHost { window_id, result }) = self.fast_host_rx.try_recv()
        {
            self.finish_fast_host_progress(window_id);
            let host = match result {
                Ok(host) => host,
                Err(error) => {
                    self.fast_server_error(window_id, error);
                    continue;
                }
            };
            if !self.router.routes.contains_key(&window_id) {
                continue;
            }
            let server = match self.server_registry.add_temporary(
                &host.endpoint,
                Some(&host.name),
                Some(&host.token),
                true,
            ) {
                Ok(server) => server,
                Err(error) => {
                    self.fast_server_error(window_id, error);
                    continue;
                }
            };
            let token = host.token.clone();
            let public_url = host.public_url.clone();
            self.fast_hosts.insert(server.id.clone(), host);
            self.switch_window_server(
                window_id,
                &server.endpoint,
                Some(token),
                Some(server.id),
            );
            let windows = self.router.routes.keys().copied().collect::<Vec<_>>();
            for window in windows {
                self.update_server_manager(window, false);
            }
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                let renderer = &mut route.window.screen.renderer;
                open_fast_server_message(
                    &mut renderer.command_palette,
                    &mut renderer.modal,
                    "Fast Server ready",
                    public_url,
                );
                route.request_redraw();
            }
        }
    }

    pub(super) fn reap_stopped_fast_hosts(&mut self) {
        let ended = self
            .fast_hosts
            .iter()
            .filter(|(id, host)| {
                !host.is_running() && !self.ended_fast_hosts.contains(*id)
            })
            .map(|(id, host)| (id.clone(), host.endpoint.clone()))
            .collect::<Vec<_>>();
        for (id, endpoint) in ended {
            self.ended_fast_hosts.insert(id);
            let owner = self
                .window_sessions
                .iter()
                .find(|(_, session)| session.connection.endpoint() == endpoint)
                .map(|(window, _)| *window)
                .or_else(|| self.router.routes.keys().next().copied());
            for session in self.window_sessions.values_mut() {
                if session.connection.endpoint() == endpoint {
                    session.mark_host_ended();
                }
            }
            if let Some(window_id) = owner {
                self.leave_server_connection(window_id, &endpoint);
                if let Some(route) = self.router.routes.get_mut(&window_id) {
                    route
                        .window
                        .screen
                        .renderer
                        .notifications
                        .push("Fast Server stopped", NotificationLevel::Warn);
                    route.request_redraw();
                }
            }
        }
    }

    pub(super) fn refresh_temporary_server_statuses(&mut self) {
        use crate::daemon_client::DaemonClientStatus;
        use neoism_ui::panels::ServerIndicatorStatus as Status;
        let statuses = self
            .server_registry
            .servers()
            .iter()
            .filter(|server| self.server_registry.is_temporary(&server.id))
            .map(|server| {
                let connections = self
                    .window_sessions
                    .values()
                    .flat_map(|session| {
                        std::iter::once(&session.connection)
                            .chain(session.parked_connections.values())
                    })
                    .filter(|connection| connection.endpoint() == server.endpoint)
                    .map(|connection| connection.status())
                    .collect::<Vec<_>>();
                let status = if connections.contains(&DaemonClientStatus::Open) {
                    Status::Online
                } else if connections.iter().any(|status| {
                    matches!(
                        status,
                        DaemonClientStatus::Connecting | DaemonClientStatus::BackingOff
                    )
                }) {
                    Status::Connecting
                } else {
                    Status::Offline
                };
                (server.id.clone(), status)
            })
            .collect::<Vec<_>>();
        for (id, status) in statuses {
            self.server_health.insert(id.clone(), status);
            for route in self.router.routes.values_mut() {
                if route
                    .window
                    .screen
                    .renderer
                    .command_palette
                    .update_server_status(&id, status)
                {
                    route.request_redraw();
                }
            }
        }
    }

    pub(super) fn copy_fast_server_invite(
        &mut self,
        window_id: WindowId,
        server_id: &str,
    ) {
        let Some(host) = self.fast_hosts.get(server_id) else {
            return;
        };
        self.router.clipboard.set(
            neoism_backend::clipboard::ClipboardType::Clipboard,
            host.public_url.clone(),
        );
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            route
                .window
                .screen
                .renderer
                .notifications
                .push("Fast Server invite copied", NotificationLevel::Info);
            route.request_redraw();
        }
    }

    pub(super) fn fast_server_error(&mut self, window_id: WindowId, error: String) {
        if let Some(route) = self.router.routes.get_mut(&window_id) {
            let renderer = &mut route.window.screen.renderer;
            open_fast_server_message(
                &mut renderer.command_palette,
                &mut renderer.modal,
                "Fast Server failed",
                error,
            );
            route.request_redraw();
        }
    }

    pub(super) fn leave_server_connection(
        &mut self,
        window_id: WindowId,
        endpoint: &str,
    ) -> bool {
        self.leave_server_connection_with_discard(window_id, endpoint, false)
    }

    pub(super) fn leave_server_connection_with_discard(
        &mut self,
        window_id: WindowId,
        endpoint: &str,
        discard: bool,
    ) -> bool {
        if self.home_daemon_endpoint.as_deref() == Some(endpoint) {
            return false;
        }
        let server_id = self.remote_server_id_for_endpoint(endpoint);
        let temporary = self.server_registry.is_temporary(&server_id);
        // Temporary entries are application-owned. Remove their bindings in
        // every window before disposing the single hosting guard/credential.
        let ssh = self.ssh_attaches.contains_key(endpoint)
            || self.ssh_routes.contains_key(endpoint);
        let windows = if temporary || ssh {
            self.router.routes.keys().copied().collect::<Vec<_>>()
        } else {
            vec![window_id]
        };
        if !discard
            && windows.iter().any(|id| {
                self.router.routes.get(id).is_some_and(|route| {
                    route.window.screen.has_unsaved_server_workspaces(endpoint)
                })
            })
        {
            use neoism_ui::widgets::modal::{ModalAction, ModalButton, ModalSpec};
            if let Some(route) = self.router.routes.get_mut(&window_id) {
                route.window.screen.renderer.modal.open(ModalSpec {
                    title: "Leave with unsaved edits?".into(),
                    body: "This server has unsaved edits. Keep the workspace open to save or copy them locally, or leave and discard those edits. Files already saved on the host are not deleted.".into(),
                    meta: endpoint.into(), input: None, busy: false, blocking: true,
                    buttons: vec![
                        ModalButton::new("Keep workspace", "Esc", ModalAction::Close),
                        ModalButton::new("Leave and discard edits", "", ModalAction::ServerReconnectDiscard { endpoint: endpoint.into() }),
                    ],
                });
                route.request_redraw();
            }
            return false;
        }
        for id in &windows {
            if self
                .server_switch_inflight
                .get(id)
                .is_some_and(|request| request.endpoint == endpoint)
            {
                self.server_switch_inflight.remove(id);
            }
        }
        let mut complete = true;
        for id in windows {
            let Some(route) = self.router.routes.get_mut(&id) else {
                continue;
            };
            let Some(workspaces) = route
                .window
                .screen
                .leave_server_workspaces(endpoint, discard)
            else {
                route.window.screen.renderer.notifications.push(
                    "Could not create a local workspace. The server connection was kept.",
                    NotificationLevel::Error,
                );
                route.request_redraw();
                complete = false;
                self.attach_session_to_window(id);
                continue;
            };
            if self
                .window_sessions
                .get(&id)
                .is_some_and(|session| session.connection.endpoint() == endpoint)
            {
                route.window.screen.take_daemon_go_home();
            }
            let target = self
                .server_switch_inflight
                .get(&id)
                .map(|request| request.endpoint.clone())
                .or_else(|| {
                    route
                        .window
                        .screen
                        .context_manager
                        .current_adopted_workspace_endpoint()
                        .map(str::to_string)
                })
                .filter(|owner| owner != endpoint)
                .or_else(|| self.home_daemon_endpoint.clone());
            route.request_redraw();
            let Some(session) = self.window_sessions.get_mut(&id) else {
                continue;
            };
            for workspace in workspaces {
                if let Err(error) = self.server_registry.remove_workspace_subscription(
                    &session.profile_id,
                    &server_id,
                    &workspace,
                ) {
                    tracing::warn!(%error, "could not unsubscribe departed workspace");
                }
            }
            session.connection_recovery.remove(endpoint);
            session.parked_connections.remove(endpoint);
            if session.connection.endpoint() == endpoint {
                session.pending_peer_adopt = None;
                session.needs_initial_workspace_adopt = false;
                session.mark_host_ended();
                if let Some(target) = target {
                    let target_id = (self.home_daemon_endpoint.as_deref()
                        != Some(target.as_str()))
                    .then(|| self.remote_server_id_for_endpoint(&target));
                    let token = target_id
                        .as_deref()
                        .and_then(|id| self.server_registry.token(id))
                        .map(str::to_string);
                    self.switch_window_server(id, &target, token, target_id);
                }
            }
        }
        if ssh && complete {
            self.ssh_attaches.remove(endpoint);
            self.ssh_routes.remove(endpoint);
        }
        if temporary && complete {
            self.fast_hosts.remove(&server_id);
            self.ended_fast_hosts.remove(&server_id);
            if let Err(error) = self.server_registry.remove(&server_id) {
                tracing::warn!(%error, "could not remove temporary server");
            }
            self.server_health.remove(&server_id);
            self.server_health_inflight.remove(&server_id);
            // An already-open picker must not retain a dead temporary row.
            for route in self.router.routes.values_mut() {
                route
                    .window
                    .screen
                    .renderer
                    .command_palette
                    .remove_server_entry(&server_id);
                route.request_redraw();
            }
        }
        complete
    }
}
