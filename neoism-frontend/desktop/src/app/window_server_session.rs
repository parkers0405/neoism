use super::daemon_pump::DesktopDaemonConnection;
use crate::daemon_client::DaemonClientStatus;
use neoism_protocol::workspace::{HostSummary, WorkspaceSummary};
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerConnectionStatus {
    Connecting,
    Online,
    Reconnecting,
    Offline,
}

pub struct WindowServerSession {
    pub profile_id: String,
    pub connection: DesktopDaemonConnection,
    pub active_server_id: Option<String>,
    pub status: ServerConnectionStatus,
    pub pending_peer_adopt: Option<String>,
    /// Set on an explicit server switch: when the new server's workspace
    /// tree first arrives and no subscription restored a workspace, the
    /// window adopts the server's most recent workspace so a join always
    /// lands IN a workspace (fresh grid + daemon shell) instead of
    /// leaving the previous server's panes on screen.
    pub needs_initial_workspace_adopt: bool,
    /// The HOME daemon connection, kept alive while this window visits a
    /// guest server. Dropping it closed the websocket, and the daemon
    /// reaps every nvim session of a closed connection's namespace — so
    /// switching away executed all the local editors. Switching home
    /// reuses this connection (same namespace, nvims intact) instead of
    /// dialling a fresh one.
    pub parked_home: Option<DesktopDaemonConnection>,
    host_daemon_urls: HashMap<String, String>,
    workspace_homes: HashMap<String, String>,
}

impl WindowServerSession {
    pub fn new(
        profile_id: String,
        connection: DesktopDaemonConnection,
        active_server_id: Option<String>,
    ) -> Self {
        Self {
            profile_id,
            connection,
            active_server_id,
            status: ServerConnectionStatus::Online,
            pending_peer_adopt: None,
            needs_initial_workspace_adopt: false,
            parked_home: None,
            host_daemon_urls: HashMap::new(),
            workspace_homes: HashMap::new(),
        }
    }

    pub fn refresh_status(&mut self) {
        self.status = match self.connection.status() {
            DaemonClientStatus::Connecting => ServerConnectionStatus::Connecting,
            DaemonClientStatus::Open => ServerConnectionStatus::Online,
            DaemonClientStatus::BackingOff => ServerConnectionStatus::Reconnecting,
            DaemonClientStatus::Closed => ServerConnectionStatus::Offline,
        };
    }

    pub fn is_home(&self, home_endpoint: Option<&str>) -> bool {
        home_endpoint.is_some_and(|home| self.connection.endpoint() == home)
    }

    pub fn observe_rehome(
        &mut self,
        hosts: &[HostSummary],
        workspaces: &[WorkspaceSummary],
    ) -> Option<String> {
        for host in hosts {
            if let Some(url) = host
                .daemon_url
                .as_deref()
                .map(str::trim)
                .filter(|url| !url.is_empty())
            {
                self.host_daemon_urls
                    .insert(host.id.clone(), url.to_string());
            }
        }
        let mut target = None;
        for workspace in workspaces {
            let Some(home) = workspace
                .running_on_host_id
                .as_deref()
                .filter(|home| !home.is_empty())
            else {
                continue;
            };
            let previous = self
                .workspace_homes
                .insert(workspace.id.clone(), home.to_string());
            if previous.is_none() || previous.as_deref() == Some(home) || target.is_some()
            {
                continue;
            }
            if let Some(url) = self.host_daemon_urls.get(home) {
                if url != self.connection.endpoint() {
                    target = Some(url.clone());
                }
            }
        }
        target
    }
}
