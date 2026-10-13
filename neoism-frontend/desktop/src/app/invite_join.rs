//! Application-owned native invitations. Parent owns event/module wiring.
use super::Application;
use neoism_ui::panels::notifications::NotificationLevel;
use neoism_window::{event_loop::ActiveEventLoop, window::WindowId};
use std::collections::VecDeque;

pub(super) const MAX_PENDING: usize = 32;
#[derive(Default)]
pub(super) struct InviteJoinState {
    pending: VecDeque<String>,
    active: Option<InviteAttempt>,
    candidate_window: Option<WindowId>,
}
impl InviteJoinState {
    pub(super) fn active_window_for_endpoint(&self, endpoint: &str) -> Option<WindowId> {
        self.active
            .as_ref()
            .filter(|attempt| attempt.endpoint == endpoint)
            .map(|attempt| attempt.window)
    }

    fn enqueue(&mut self, link: String) -> Result<(), ()> {
        if self.pending.contains(&link)
            || self.active.as_ref().is_some_and(|a| a.link == link)
        {
            return Ok(());
        }
        if self.pending.len() >= MAX_PENDING {
            return Err(());
        }
        self.pending.push_back(link);
        Ok(())
    }
}

struct InviteAttempt {
    window: WindowId,
    link: String,
    name: String,
    endpoint: String,
    phase: Phase,
    generation: Option<u64>,
}
#[derive(PartialEq)]
enum Phase {
    Prompt,
    Dial,
    Retry,
}

impl Application<'_> {
    pub(super) fn queue_invite_activation(
        &mut self,
        window_id: WindowId,
        endpoint: &str,
    ) {
        let subject = if self.invite_activation.contains_key(endpoint) {
            Some(endpoint.to_string())
        } else {
            self.fast_hosts
                .values()
                .filter(|host| host.endpoint == endpoint)
                .find_map(|host| {
                    crate::fast_servers::normalize_fast_join(
                        host.public_url.clone(),
                        None,
                    )
                    .ok()
                    .map(|public| public.endpoint)
                    .filter(|public| self.invite_activation.contains_key(public))
                })
        };
        let Some(subject) = subject else { return };
        let rendered = if let Some(route) = self.router.routes.get_mut(&window_id) {
            self.invite_activation_targets
                .retain(|_, bound| bound != &subject);
            self.invite_activation_targets.insert(window_id, subject);
            route.request_redraw();
            route.window.has_rendered_frame
        } else {
            false
        };
        // A minimized mapped surface can activate immediately; cold surfaces
        // retain their source-specific authority until their first render.
        if rendered {
            self.apply_invite_activation(window_id);
        }
    }

    pub(super) fn apply_invite_activation(&mut self, window_id: WindowId) {
        let Some(subject) = self.invite_activation_targets.remove(&window_id) else {
            return;
        };
        let Some(token) = self.invite_activation.remove(&subject) else {
            return;
        };
        if let Some(route) = self.router.routes.get(&window_id) {
            super::invite_activation::activate_invite_window(
                &route.window.winit_window,
                &mut Some(token),
            );
        }
    }

    pub(super) fn handle_invite_uri(&mut self, uri: String) -> Option<String> {
        let link = match crate::invite_links::parse_invite_link(&uri) {
            Ok(link) => link,
            Err(_) => {
                // Never echo a URI/capability into notifications or logs.
                if let Some(id) = self
                    .router
                    .get_focused_route()
                    .or_else(|| self.router.routes.keys().next().copied())
                {
                    self.fast_server_error(id, "Invalid Fast invite link".into());
                }
                return None;
            }
        };
        let endpoint = crate::invite_links::validate_invite_link(&link)
            .ok()?
            .endpoint;
        if self.invite_join.enqueue(link).is_err() {
            if let Some(id) = self.router.get_focused_route() {
                self.fast_server_error(
                    id,
                    "Too many pending invites; finish joining before opening another"
                        .into(),
                );
            }
            return None;
        }
        Some(endpoint)
    }

    /// Pump AFTER modal submissions and switch results. Safe before any window
    /// exists: invitations remain FIFO until the bootstrap window is ready.
    pub(super) fn process_pending_invites(&mut self, event_loop: &ActiveEventLoop) {
        let submissions: Vec<_> = self
            .router
            .routes
            .iter_mut()
            .filter_map(|(id, route)| {
                route
                    .window
                    .screen
                    .take_fast_invite_join()
                    .map(|request| (*id, request))
            })
            .collect();
        for (window, (link, name, password)) in submissions {
            self.submit_invite_join(window, link, name, password);
        }
        if let Some(active) = self.invite_join.active.as_ref() {
            let window = active.window;
            let Some(route) = self.router.routes.get(&window) else {
                // Closing the owning window keeps the invitation (not password).
                let active = self.invite_join.active.take().unwrap();
                self.invite_join.pending.push_front(active.link);
                return;
            };
            if active.phase == Phase::Retry {
                if !route.window.screen.fast_invite_ui_busy() {
                    let active = self.invite_join.active.as_mut().unwrap();
                    route.window.winit_window.focus_window();
                    let link = active.link.clone();
                    let name = active.name.clone();
                    let endpoint = active.endpoint.clone();
                    active.phase = Phase::Prompt;
                    let route = self.router.routes.get_mut(&window).unwrap();
                    route.window.screen.open_fast_invite_prompt(link, name);
                    route.request_redraw();
                    self.queue_invite_activation(window, &endpoint);
                }
                return;
            }
            if active.phase == Phase::Dial {
                let still_current =
                    self.server_switch_inflight
                        .get(&window)
                        .is_some_and(|request| {
                            request.endpoint == active.endpoint
                                && Some(request.generation) == active.generation
                        });
                if !still_current {
                    // A superseded generation never reaches the result hooks.
                    // It is a cancellation, not an authentication retry. Do not
                    // resurrect it over the newly selected server or block FIFO.
                    self.invite_join.active = None;
                }
                return;
            }
            if route.window.screen.renderer.modal.is_active() {
                return;
            }
            // Explicit form dismissal cancels only this invitation. Later ones
            // remain queued. Password values were never copied to this state.
            self.invite_join.active = None;
        }
        let Some(link) = self.invite_join.pending.front().cloned() else {
            return;
        };
        let Ok(join) = crate::invite_links::validate_invite_link(&link) else {
            self.invite_join.pending.pop_front();
            self.invite_join.candidate_window = None;
            return;
        };
        // A local host's public origin maps to its local endpoint, not a dial
        // back through its guest URL. No local host credential is exposed.
        let host_endpoint = self.fast_hosts.values().find_map(|host| {
            crate::fast_servers::normalize_fast_join(host.public_url.clone(), None)
                .ok()
                .filter(|public| public.endpoint == join.endpoint)
                .map(|_| host.endpoint.clone())
        });
        let endpoint = host_endpoint.as_deref().unwrap_or(&join.endpoint);
        let existing = self.router.routes.iter().find_map(|(id, route)| {
            route
                .window
                .screen
                .fast_invite_workspace_index(endpoint)
                .map(|index| (*id, index))
        });
        if let Some((window, index)) = existing {
            self.queue_invite_activation(window, endpoint);
            if !self.focus_fast_workspace(window, index, endpoint) {
                return;
            }
            let route = self.router.routes.get_mut(&window).unwrap();
            if host_endpoint.is_some() {
                route.window.screen.renderer.notifications.push(
                    "You are already hosting this workspace",
                    NotificationLevel::Info,
                );
            }
            route.request_redraw();
            self.invite_join.pending.pop_front();
            self.invite_join.candidate_window = None;
            return;
        }
        // If hosting is still adopting its verified root, wait rather than add
        // a second temporary guest entry for our own public origin.
        if host_endpoint.is_some() {
            return;
        }
        let candidate = self
            .invite_join
            .candidate_window
            .filter(|window| self.router.routes.contains_key(window));
        self.invite_join.candidate_window = candidate;
        let Some(mut window) = candidate
            .or_else(|| self.router.get_focused_route())
            .or_else(|| self.router.routes.keys().next().copied())
        else {
            return;
        };
        let screen = &self.router.routes[&window].window.screen;
        let unavailable = screen.fast_invite_ui_busy()
            || self.server_switch_inflight.contains_key(&window)
            || screen.has_unsaved_server_buffers();
        if candidate.is_some() && unavailable {
            self.queue_invite_activation(window, &join.endpoint);
            return;
        }
        if unavailable {
            // Keep busy dialogs and dirty documents intact in their own window.
            window = self.router.create_window(
                event_loop,
                self.event_proxy.clone(),
                &self.config,
                None,
                self.app_id.as_deref(),
            );
            self.attach_bootstrap_session(window);
            self.ensure_local_session(window);
            self.invite_join.candidate_window = Some(window);
            if !self.router.routes.contains_key(&window) {
                return;
            }
        }
        self.queue_invite_activation(window, &join.endpoint);
        let route = self.router.routes.get_mut(&window).unwrap();
        if route.window.screen.fast_invite_ui_busy() {
            return;
        }
        route
            .window
            .screen
            .open_fast_invite_prompt(link.clone(), join.name.clone());
        route.window.winit_window.focus_window();
        route.request_redraw();
        self.invite_join.pending.pop_front();
        self.invite_join.candidate_window = None;
        self.invite_join.active = Some(InviteAttempt {
            window,
            link,
            name: join.name,
            endpoint: join.endpoint.clone(),
            phase: Phase::Prompt,
            generation: None,
        });
        self.queue_invite_activation(window, &join.endpoint);
    }

    fn submit_invite_join(
        &mut self,
        window: WindowId,
        link: String,
        name: Option<String>,
        password: Option<String>,
    ) {
        let valid = crate::invite_links::validate_invite_link(&link).and_then(|join| {
            if join.token.is_none() && password.is_none() {
                Err("Enter the host's password before joining".into())
            } else {
                crate::fast_servers::normalize_fast_join(link.clone(), password.clone())
            }
        });
        let join = match valid {
            Ok(join) => join,
            Err(_) => {
                self.invite_join_failed(window, "");
                return;
            }
        };
        let Some(active) = self
            .invite_join
            .active
            .as_mut()
            .filter(|a| a.window == window && a.phase == Phase::Prompt)
        else {
            return;
        };
        active.link = link.clone(); // Corrected form link survives failed auth.
        active.name = name.clone().unwrap_or(join.name);
        active.endpoint = join.endpoint.clone();
        active.phase = Phase::Dial;
        if self
            .router
            .routes
            .get(&window)
            .is_some_and(|route| route.window.screen.has_unsaved_server_buffers())
        {
            active.phase = Phase::Retry;
            self.fast_server_error(
                window,
                "Save changes before joining; your invitation is kept".into(),
            );
            return;
        }
        self.add_fast_server(window, false, link, name, password);
        if let Some(active) = self
            .invite_join
            .active
            .as_mut()
            .filter(|a| a.window == window && a.phase == Phase::Dial)
        {
            active.generation = self
                .server_switch_inflight
                .get(&window)
                .filter(|request| request.endpoint == active.endpoint)
                .map(|request| request.generation);
        }
        // Spawn/registration failures can happen synchronously.
        if !self.server_switch_inflight.contains_key(&window)
            && self
                .invite_join
                .active
                .as_ref()
                .is_some_and(|a| a.phase == Phase::Dial)
        {
            self.invite_join_failed(window, &join.endpoint);
        }
    }

    pub(super) fn is_invite_join_attempt(
        &self,
        window: WindowId,
        endpoint: &str,
    ) -> bool {
        self.invite_join.active.as_ref().is_some_and(|a| {
            a.window == window && a.endpoint == endpoint && a.phase == Phase::Dial
        })
    }

    /// Parent: call in the current-generation switch Err arm, after removing
    /// inflight. No automatic reconnect: next attempt needs explicit submit.
    /// Ignore the error text: it may contain an endpoint or credential.
    pub(super) fn invite_join_failed(&mut self, window: WindowId, endpoint: &str) {
        if let Some(active) = self.invite_join.active.as_mut().filter(|a| {
            a.window == window && (endpoint.is_empty() || a.endpoint == endpoint)
        }) {
            active.phase = Phase::Retry;
            let endpoint = active.endpoint.clone();
            // A rejected fresh dial has no runner. Remove its temporary entry
            // and in-memory password so the next submit registers a clean one.
            // Never disturb another window's live/parked workspace or a host.
            let connected = self
                .server_switch_inflight
                .values()
                .any(|request| request.endpoint == endpoint)
                || self.window_sessions.values().any(|session| {
                    session.connection.endpoint() == endpoint
                        || session.parked_connections.contains_key(&endpoint)
                });
            if !connected {
                let id = self
                    .server_registry
                    .servers()
                    .iter()
                    .find(|server| {
                        server.endpoint == endpoint
                            && self.server_registry.is_temporary(&server.id)
                            && !self.server_registry.is_temporary_host(&server.id)
                    })
                    .map(|server| server.id.clone());
                if let Some(id) = id {
                    let _ = self.server_registry.remove(&id);
                }
            }
            self.fast_server_error(
                window,
                "Could not join. Check the invite and password, then try again.".into(),
            );
        }
    }

    /// Parent: call from complete_server_switch BEFORE clearing/parking any
    /// state. The next invitation is shown on the next regular pump.
    pub(super) fn invite_join_succeeded(&mut self, window: WindowId, endpoint: &str) {
        if self.invite_join.active.as_ref().is_some_and(|a| {
            a.window == window && a.endpoint == endpoint && a.phase == Phase::Dial
        }) {
            self.invite_join.active = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activation_cannot_select_another_invites_prompt() {
        let window = unsafe { WindowId::dummy() };
        let mut state = InviteJoinState::default();
        state.active = Some(InviteAttempt {
            window,
            link: "https://first.example/".into(),
            name: "First".into(),
            endpoint: "wss://first.example/session".into(),
            phase: Phase::Prompt,
            generation: None,
        });
        assert_eq!(
            state.active_window_for_endpoint("wss://first.example/session"),
            Some(window)
        );
        assert_eq!(
            state.active_window_for_endpoint("wss://second.example/session"),
            None
        );
    }

    #[test]
    fn pending_invites_are_bounded_deduplicated_fifo() {
        let mut state = InviteJoinState::default();
        for i in 0..MAX_PENDING {
            state.enqueue(format!("https://team-{i}.example/")).unwrap();
        }
        state.enqueue("https://team-0.example/".into()).unwrap();
        assert_eq!(state.pending.len(), MAX_PENDING);
        assert!(state.enqueue("https://overflow.example/".into()).is_err());
        for i in 0..MAX_PENDING {
            assert_eq!(
                state.pending.pop_front().unwrap(),
                format!("https://team-{i}.example/")
            );
        }
        assert!(state.pending.is_empty());
    }
}
