//! Blame always uses the owning daemon, including joined desktops.
use super::*;
use neoism_protocol::git::{GitClientMessage, GitServerMessage};

impl Screen<'_> {
    pub(crate) fn pump_code_blame(&mut self) {
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return;
        };
        let Some(root) = self
            .context_manager
            .current_adopted_workspace_id()
            .and_then(|id| self.context_manager.daemon_host_workspace_root(&id))
            .or_else(|| self.active_pane_workspace_root())
            .or_else(|| self.active_workspace_root.clone())
        else {
            return;
        };
        let endpoint = self
            .context_manager
            .daemon_endpoint()
            .unwrap_or_default()
            .to_owned();
        let workspace = self.context_manager.current_adopted_workspace_id();
        let Some(code) = self.context_manager.current_mut().code.as_mut() else {
            return;
        };
        code.blame.apply_default(self.renderer.code_git_blame);
        code.blame.set_options(
            self.renderer.code_git_blame_delay_ms,
            self.renderer.code_git_blame_hide_on_scroll,
        );
        code.observe_blame_viewport();
        // Local-only vaults in joined workspaces must never be sent to that host.
        if code.local_only {
            code.blame.error =
                Some("Blame unavailable for a client-local document on this host".into());
            return;
        }
        let path = code.path.to_string_lossy().into_owned();
        let scope = format!(
            "{endpoint}:{:?}:{workspace:?}:{root:?}:{path}",
            handle.connection_key()
        );
        code.blame.update(&code.buffer);
        if !code.blame.needs_request(scope) {
            return;
        }
        let id = handle.allocate_git_request_id();
        code.blame.requested(id);
        let proxy = self.context_manager.event_proxy_clone();
        let window = self.context_manager.window_id();
        runtime.spawn(async move {
            let _ = handle
                .send_git_with_request_id(
                    id,
                    GitClientMessage::Blame { path },
                    Some(root),
                )
                .await;
            // One bounded timeout wake, not a permanent polling task. Disabling
            // blame never schedules another request; late replies are ignored.
            tokio::time::sleep(std::time::Duration::from_secs(46)).await;
            proxy.send_event(
                neoism_backend::event::RioEventType::Rio(
                    neoism_backend::event::RioEvent::Render,
                ),
                window,
            );
        });
    }

    pub(crate) fn apply_code_blame_reply(
        &mut self,
        id: u64,
        message: &GitServerMessage,
    ) -> bool {
        // Refresh the scope before accepting a queued reply from a replaced link.
        self.pump_code_blame();
        let wake =
            self.context_manager
                .daemon_link_handle_and_runtime()
                .map(|(_, runtime)| {
                    (
                        runtime,
                        self.context_manager.event_proxy_clone(),
                        self.context_manager.window_id(),
                    )
                });
        for grid in self.context_manager.all_grids_mut().iter_mut() {
            for item in grid.contexts_mut().values_mut() {
                if let Some(code) = item.context_mut().code.as_mut() {
                    if code.blame.accept(id, message) {
                        code.blame.update(&code.buffer);
                        if let Some((runtime, proxy, window)) = wake {
                            runtime.spawn(async move {
                                tokio::time::sleep(std::time::Duration::from_secs(15))
                                    .await;
                                proxy.send_event(
                                    neoism_backend::event::RioEventType::Rio(
                                        neoism_backend::event::RioEvent::Render,
                                    ),
                                    window,
                                );
                            });
                        }
                        return true;
                    }
                }
            }
        }
        false
    }
}
