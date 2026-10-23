//! Conversations projects are authorized registry + saved-chat paths, not workspace tabs.
//! Both sources load off-thread; directory changes use the normal picker policy.
use crate::screen::Screen;
use neoism_ui::widgets::conversation_navigator::ConversationNavigatorRow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

mod terminal_rebase;
use terminal_rebase::PendingTerminalDirectory;

#[derive(Clone, Debug)]
struct Project {
    value: String,
    title: String,
    root: PathBuf,
}

#[derive(Default)]
pub(crate) struct ConversationWorkspaceState {
    pub(crate) system_browser: Vec<super::file_browser::SystemBrowserJob>,
    pub(crate) system_sources: Vec<crate::neoism::agent::system::AgentSystemSource>,
    scope: String,
    rows: Vec<Project>,
    registry: Vec<Project>,
    chat_projects: Vec<Project>,
    homes: HashMap<String, PathBuf>,
    pending: Option<RegistryJob>,
    refreshed: Option<Instant>,
    error: Option<String>,
    // An explicit picker change must not be undone by the unchanged first shell.
    terminal_directories: HashMap<(String, String), PendingTerminalDirectory>,
    explicit_roots: HashMap<(String, String), ExplicitRoot>,
}
struct ExplicitRoot {
    root: PathBuf,
    previous_cwd: Option<PathBuf>,
}
impl ConversationWorkspaceState {
    fn retire_explicit_root_for_cwd(
        &mut self,
        key: &(String, String),
        cwd: Option<&Path>,
    ) {
        // Missing cwd is not evidence of shell movement. Once movement is
        // observed, remove the pin rather than letting a later round trip revive it.
        if let Some(cwd) = cwd {
            if self
                .explicit_roots
                .get(key)
                .is_some_and(|explicit| explicit.previous_cwd.as_deref() != Some(cwd))
            {
                self.explicit_roots.remove(key);
            }
        }
    }
}

struct RegistryJob {
    scope: String,
    origin: String,
    origin_root: Option<PathBuf>,
    browser_generation: u64,
    activate: Option<PathBuf>,
    receiver: Receiver<ProjectDiscoveryReply>,
}

struct ProjectDiscoveryReply {
    // None means the optional management listing route returned HTTP 404.
    registry: Result<Option<Vec<Project>>, String>,
    chats: Result<Vec<Project>, String>,
}

fn chat_project_rows(directories: impl IntoIterator<Item = String>) -> Vec<Project> {
    directories
        .into_iter()
        .filter_map(|directory| {
            let root = PathBuf::from(directory);
            if !root.is_absolute() {
                return None;
            }
            Some(Project {
                value: format!("directory:{}", root.display()),
                title: root
                    .file_name()
                    .unwrap_or(root.as_os_str())
                    .to_string_lossy()
                    .into_owned(),
                root,
            })
        })
        .collect()
}

fn apply_project_discovery(
    state: &mut ConversationWorkspaceState,
    reply: ProjectDiscoveryReply,
) -> bool {
    // An absent optional listing is not a permission denial. A user-chosen
    // folder still goes through commit_workspace_directory's normal guards;
    // auth/network/server errors must prevent that pending activation.
    let registry_ok = reply.registry.is_ok();
    let mut errors = Vec::new();
    // Each cache represents this refresh's authorized response. Never retain a
    // failed source's old permission snapshot or let it mask the other source.
    state.registry = reply
        .registry
        .unwrap_or_else(|error| {
            errors.push(format!("Workspace registry: {error}"));
            None
        })
        .unwrap_or_default();
    state.chat_projects = reply.chats.unwrap_or_else(|error| {
        errors.push(format!("Saved chat projects: {error}"));
        Vec::new()
    });
    state.error = (!errors.is_empty()).then(|| errors.join("; "));
    registry_ok
}

fn normalize_projects(
    mut projects: Vec<Project>,
    current: Option<&Path>,
) -> Vec<Project> {
    // Path components normalize redundant separators/dots without filesystem I/O.
    for project in &mut projects {
        project.root = project.root.components().collect();
        // Selection identifies a directory, not which discovery source won.
        project.value = format!("directory:{}", project.root.display());
    }
    let mut seen = std::collections::HashSet::new();
    projects.retain(|p| seen.insert(p.root.clone()));
    projects.sort_by(|a, b| {
        a.title
            .to_lowercase()
            .cmp(&b.title.to_lowercase())
            .then_with(|| a.root.cmp(&b.root))
            .then_with(|| a.value.cmp(&b.value))
    });
    projects.sort_by_key(|p| current != Some(p.root.as_path()));
    projects
}

fn registry_rows(value: serde_json::Value) -> Result<Vec<Project>, String> {
    let rows = value
        .as_array()
        .ok_or("Workspace registry returned an invalid list")?;
    Ok(rows
        .iter()
        .filter_map(|row| {
            let id = row.get("id")?.as_str()?;
            let root = PathBuf::from(row.get("root")?.as_str()?);
            if !root.is_absolute() {
                return None;
            }
            Some(Project {
                value: format!("project:{id}"),
                title: row
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or(id)
                    .into(),
                root,
            })
        })
        .collect())
}

const PROJECT_DISCOVERY_INTERVAL: Duration = Duration::from_secs(15);

fn project_discovery_due(
    pending: bool,
    refreshed: Option<Instant>,
    now: Instant,
) -> bool {
    !pending
        && refreshed.is_none_or(|at| {
            now.saturating_duration_since(at) >= PROJECT_DISCOVERY_INTERVAL
        })
}

fn home_anchor(root: &Path, local_home: Option<&Path>, joined: bool) -> bool {
    // '/' is a known filesystem-root path, not a guess at the remote user's home.
    root == Path::new("/") || (!joined && local_home == Some(root))
}
fn anchor_for_change(
    saved: Option<&Path>,
    current: Option<&Path>,
    home: Option<&Path>,
    joined: bool,
) -> Option<PathBuf> {
    saved
        .map(Path::to_path_buf)
        .or_else(|| {
            current
                .filter(|root| home_anchor(root, home, joined))
                .map(Path::to_path_buf)
        })
        .or_else(|| {
            if joined {
                None
            } else {
                home.map(Path::to_path_buf)
            }
        })
}
fn retain_explicit_root(
    explicit: &ExplicitRoot,
    active: Option<&Path>,
    cwd: Option<&Path>,
) -> bool {
    active == Some(explicit.root.as_path())
        && (cwd.is_none() || cwd == explicit.previous_cwd.as_deref())
}

impl Screen<'_> {
    pub(crate) fn set_agent_system_sources(&mut self, mut sources: Vec<crate::neoism::agent::system::AgentSystemSource>) {
        sources.sort_by(|a, b| a.id.cmp(&b.id));
        let root = self.renderer.conversations_pane.side_panel().catalog_directory().is_none()
            && !self.context_manager.current_workspace_is_remote_joined();
        let changed = self.conversation_workspaces.system_sources != sources;
        self.conversation_workspaces.system_sources = sources.clone();
        if changed { self.refresh_system_conversation_search(); }
        self.renderer.conversations_pane.set_system_sources(sources.clone(), root);
        let current_grid = self.context_manager.current_index();
        for (index, grid) in self.context_manager.all_grids_mut().into_iter().enumerate() {
            for context in grid.contexts_mut().values_mut() {
                if let Some(agent) = context.context_mut().neoism_agent.as_mut() {
                    // Authorization/transport changes reach parked tabs too. Keep
                    // each inactive pane's existing workspace selection policy.
                    let pane_root = if index == current_grid { root } else { agent.systems.root };
                    agent.set_system_sources(sources.clone(), pane_root);
                }
            }
        }
    }

    // Deliberately exclude the root-scoped /agent/workspaces/... endpoint. An
    // anchor belongs to a host connection, not a directory or workspace tab.
    fn conversation_host_scope(&self) -> String {
        format!(
            "{:?}:{}:{:?}",
            self.context_manager.daemon_connection_key(),
            self.context_manager.current_workspace_is_remote_joined(),
            self.context_manager.daemon_endpoint()
        )
    }
    fn conversation_workspace_scope(&self) -> String {
        format!(
            "{}:{:?}",
            self.conversation_host_scope(),
            self.context_manager.agent_server_override_for_current()
        )
    }
    pub(crate) fn capture_conversation_home_anchor(&mut self) {
        let scope = self.conversation_host_scope();
        let root = self
            .active_workspace_root
            .clone()
            .or_else(|| self.active_pane_workspace_root());
        let joined = self.context_manager.current_workspace_is_remote_joined();
        let home = if joined { None } else { dirs::home_dir() };
        if let Some(anchor) = anchor_for_change(
            self.conversation_workspaces
                .homes
                .get(&scope)
                .map(PathBuf::as_path),
            root.as_deref(),
            home.as_deref(),
            joined,
        ) {
            self.conversation_workspaces
                .homes
                .entry(scope)
                .or_insert(anchor);
        }
    }
    /// Replay the host-scoped observed dashboard identity into all catalog views.
    /// No filesystem probes: the cached home anchor is the same one used by the
    /// dashboard navigator, not a selected chat's execution directory.
    pub(crate) fn sync_conversation_dashboard_directory(&mut self) {
        self.capture_conversation_home_anchor();
        let scope = self.conversation_host_scope();
        let directory = self
            .conversation_workspaces
            .homes
            .get(&scope)
            .map(|root| root.to_string_lossy().into_owned());
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_dashboard_directory(directory.clone());
        for context in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
        {
            if let Some(agent) = context.context_mut().neoism_agent.as_mut() {
                agent
                    .side_panel_mut()
                    .set_dashboard_directory(directory.clone());
            }
        }
    }

    pub(crate) fn retire_conversation_explicit_root_for_cwd(
        &mut self,
        cwd: Option<&Path>,
    ) {
        let key = (
            self.conversation_host_scope(),
            self.context_manager.current_workspace_tree_id(),
        );
        if !self.conversation_workspaces.terminal_directories.contains_key(&key) {
            self.conversation_workspaces
                .retire_explicit_root_for_cwd(&key, cwd);
        }
    }

    pub(crate) fn conversation_explicit_root_for_cwd(
        &self,
        cwd: Option<&Path>,
    ) -> Option<PathBuf> {
        let key = (
            self.conversation_host_scope(),
            self.context_manager.current_workspace_tree_id(),
        );
        let explicit = self.conversation_workspaces.explicit_roots.get(&key)?;
        if self.conversation_workspaces.terminal_directories.contains_key(&key) {
            return Some(explicit.root.clone());
        }
        retain_explicit_root(explicit, self.active_workspace_root.as_deref(), cwd)
            .then(|| explicit.root.clone())
    }
    /// Shared by Workspace picker, project selection, and folder-plus completion.
    /// Joined publication remains subject to ContextManager's owner policy; a
    /// rejected publication never optimistically changes the guest's local root.
    pub(crate) fn commit_workspace_directory(
        &mut self,
        root: PathBuf,
    ) -> Result<(), String> {
        if !root.is_absolute() {
            return Err("Project directory must be absolute".into());
        }
        self.capture_conversation_home_anchor();
        let root = if self.context_manager.current_workspace_is_remote_joined() {
            root
        } else {
            Self::normalize_workspace_dir(root).ok_or("Project directory is unavailable")?
        };
        if self.context_manager.current_workspace_is_remote_joined() {
            let id = self.current_workspace_id().ok_or("No current workspace")?;
            if !self.context_manager.may_publish_workspace(&id)
                || self.context_manager.daemon_link_handle_and_runtime().is_none() {
                return Err("Only the connected workspace owner may change its directory; no terminal request was sent".into());
            }
        }
        self.queue_explicit_terminal_directory(root.clone())?;
        if self.context_manager.current_workspace_is_remote_joined() {
            let id = self.current_workspace_id().ok_or("No current workspace")?;
            self.context_manager.set_daemon_workspace_root(id, root);
        } else {
            if !root.is_dir() {
                return Err("Project directory is unavailable".into());
            }
            let previous_cwd = self
                .active_terminal_process_cwd()
                .or_else(|| {
                    self.context_manager
                        .current()
                        .terminal
                        .try_lock_unfair()
                        .and_then(|terminal| terminal.current_directory.clone())
                })
                .or_else(|| self.active_workspace_root.clone());
            self.set_active_workspace_root(root, true);
            if let Some(root) = self.active_workspace_root.clone() {
                let key = (
                    self.conversation_host_scope(),
                    self.context_manager.current_workspace_tree_id(),
                );
                self.conversation_workspaces
                    .explicit_roots
                    .insert(key, ExplicitRoot { root, previous_cwd });
            }
        }
        self.pump_explicit_terminal_directory();
        if !self.conversation_workspaces.terminal_directories.is_empty() {
            self.renderer.notifications.push(
                "Terminal directory change queued. Commands for that terminal are blocked until its shell confirms the new directory; running jobs are untouched.",
                neoism_ui::panels::notifications::NotificationLevel::Info,
            );
        }
        self.mark_dirty();
        Ok(())
    }

    fn start_conversation_registry_job(
        &mut self,
        activate: Option<PathBuf>,
    ) -> Result<(), String> {
        if self.conversation_workspaces.pending.is_some() {
            return Err("Workspace registry is still loading; try again shortly".into());
        }
        let scope = self.conversation_workspace_scope();
        // Folder-plus may be the first navigator action, before any rows were
        // requested. Claim its source scope now so the next render does not
        // discard this registration as an old-scope refresh.
        if self.conversation_workspaces.scope != scope {
            self.conversation_workspaces.scope = scope.clone();
            self.conversation_workspaces.registry.clear();
            self.conversation_workspaces.chat_projects.clear();
            self.conversation_workspaces.rows.clear();
            self.conversation_workspaces.refreshed = None;
            self.conversation_workspaces.error = None;
        }
        let server = match self.context_manager.agent_server_override_for_current() {
            Some(server) => server,
            None if self.context_manager.current_workspace_is_remote_joined() => {
                return Err("Host workspace management endpoint is unavailable".into())
            }
            None => crate::neoism::agent::neoism_agent_server(),
        };
        let (tx, rx) = mpsc::channel();
        let selected = activate.clone();
        let joined = self.context_manager.current_workspace_is_remote_joined();
        let events = self.context_manager.event_proxy();
        let window = self.context_manager.window_id();
        let route = self.context_manager.current().route_id;
        std::thread::Builder::new().name("conversation-workspace-registry".into()).spawn(move || {
            use crate::neoism::agent::conversation_workspace_registry_request as request;
            let result = (|| {
                let Some(listing) = request(&server, None)? else {
                    // Optional management is disabled. The explicit folder
                    // picker may still rehome via commit_workspace_directory's
                    // normal local/host policy, but nothing is registered here.
                    return Ok(None);
                };
                let mut rows = registry_rows(listing)?;
                if let Some(root) = selected {
                    // Remote paths are validated by the guarded files plane and
                    // scoped management service, never the client's filesystem.
                    let root = if joined { root } else { root.canonicalize().map_err(|e| e.to_string())? };
                    if !rows.iter().any(|p| p.root == root) {
                        if let Err(error) = request(&server, Some(&root)) {
                            rows = registry_rows(request(&server, None)?.ok_or("Workspace management disappeared during registration")?)?;
                            if !rows.iter().any(|p| p.root == root) { return Err(error); }
                        } else { rows = registry_rows(request(&server, None)?.ok_or("Workspace management disappeared during registration")?)?; }
                    }
                }
                Ok(Some(rows))
            })();
            // The summary endpoint is global to this authorized connection: no
            // directory, cursor, recent-session limit, or execution cwd is sent.
            // Run even when registry listing/registration failed.
            let chats = crate::neoism::agent::fetch_session_projects(&server)
                .map(|summary| chat_project_rows(summary.projects.into_iter().map(|(directory, _)| directory)));
            let _ = tx.send(ProjectDiscoveryReply { registry: result, chats });
            events.send_event(neoism_backend::event::RioEvent::RenderRoute(route).into(), window);
        }).map_err(|e| e.to_string())?;
        self.conversation_workspaces.pending = Some(RegistryJob {
            scope,
            origin: self.context_manager.current_workspace_tree_id(),
            origin_root: self.active_workspace_root.clone(),
            browser_generation: self.desktop_browser_generation,
            activate,
            receiver: rx,
        });
        Ok(())
    }
    pub(crate) fn pump_conversation_workspaces(&mut self) {
        let result = self
            .conversation_workspaces
            .pending
            .as_ref()
            .and_then(|job| match job.receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let error = "Project discovery worker stopped".to_string();
                    Some(ProjectDiscoveryReply {
                        registry: Err(error.clone()),
                        chats: Err(error),
                    })
                }
                Err(mpsc::TryRecvError::Empty) => None,
            });
        let Some(result) = result else { return };
        let job = self.conversation_workspaces.pending.take().unwrap();
        if job.scope != self.conversation_workspace_scope() {
            return;
        }
        self.conversation_workspaces.refreshed = Some(Instant::now());
        let registry_ok =
            apply_project_discovery(&mut self.conversation_workspaces, result);
        if registry_ok {
            if let Some(root) = job.activate {
                if job.origin == self.context_manager.current_workspace_tree_id()
                    && job.origin_root == self.active_workspace_root
                    && job.browser_generation == self.desktop_browser_generation
                {
                    if let Err(error) = self.commit_workspace_directory(root) {
                        self.conversation_workspaces.error = Some(error);
                    }
                }
            }
        }
        if let Some(error) = self.conversation_workspaces.error.clone() {
            self.file_tree_notify(
                error,
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
        }
        self.mark_dirty();
    }
    pub(crate) fn conversation_project_rows(&mut self) -> Vec<ConversationNavigatorRow> {
        self.capture_conversation_home_anchor();
        self.pump_conversation_workspaces();
        let scope = self.conversation_workspace_scope();
        if self.conversation_workspaces.scope != scope {
            self.conversation_workspaces.scope = scope;
            self.conversation_workspaces.registry.clear();
            self.conversation_workspaces.chat_projects.clear();
            self.conversation_workspaces.rows.clear();
            self.conversation_workspaces.refreshed = None;
            self.conversation_workspaces.pending = None;
            self.conversation_workspaces.error = None;
        }
        if project_discovery_due(
            self.conversation_workspaces.pending.is_some(),
            self.conversation_workspaces.refreshed,
            Instant::now(),
        ) {
            if let Err(error) = self.start_conversation_registry_job(None) {
                // Keep failures out of the dashboard row's trailing metadata;
                // report once per refresh interval without blocking saved choices.
                self.conversation_workspaces.refreshed = Some(Instant::now());
                self.conversation_workspaces.error = Some(error.clone());
                self.file_tree_notify(
                    error,
                    neoism_ui::panels::notifications::NotificationLevel::Error,
                );
            }
        }
        let mut projects = self.conversation_workspaces.registry.clone();
        projects.extend(self.conversation_workspaces.chat_projects.clone());
        // Include the currently declared directory even before registry refresh
        // (or when its owner opened it outside Conversations). This is path
        // metadata only: no host-workspace ID is ever an activation target.
        if let Some(root) = self.active_workspace_root.clone() {
            if !projects.iter().any(|p| p.root == root) {
                projects.push(Project {
                    value: format!("current-root:{}", root.display()),
                    title: root
                        .file_name()
                        .unwrap_or(root.as_os_str())
                        .to_string_lossy()
                        .into_owned(),
                    root,
                });
            }
        }
        let current = self.active_workspace_root.as_deref();
        let projects = normalize_projects(projects, current);
        self.conversation_workspaces.rows = projects.clone();
        let host_scope = self.conversation_host_scope();
        let home = self.conversation_workspaces.homes.get(&host_scope);
        let mut rows = vec![ConversationNavigatorRow {
            value: "all-projects".into(),
            title: "~".into(),
            description: home
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "Home directory unavailable".into()),
            footer: if home.is_some() {
                "Dashboard"
            } else {
                "Unavailable"
            }
            .into(),
            is_current: home.is_some_and(|root| current == Some(root.as_path())),
            is_disabled: home.is_none(),
        }];
        rows.extend(
            projects
                .into_iter()
                .filter(|p| home != Some(&p.root))
                .map(|p| ConversationNavigatorRow {
                    value: p.value,
                    title: p.title,
                    description: p.root.display().to_string(),
                    footer: String::new(),
                    is_current: current == Some(p.root.as_path()),
                    is_disabled: false,
                }),
        );
        rows
    }
    pub(crate) fn activate_conversation_project(
        &mut self,
        value: &str,
    ) -> Result<(), String> {
        // Capture before *every* first action, not only folder-plus/row building.
        self.capture_conversation_home_anchor();
        let root = if value == "all-projects" {
            self.conversation_workspaces
                .homes
                .get(&self.conversation_host_scope())
                .cloned()
                .ok_or("No authorized dashboard directory is available on this host")?
        } else {
            if self.conversation_workspace_scope() != self.conversation_workspaces.scope {
                return Err("Workspace list changed; reopen the project picker".into());
            }
            self.conversation_workspaces
                .rows
                .iter()
                .find(|p| p.value == value)
                .map(|p| p.root.clone())
                .ok_or("Project is no longer in the registry snapshot")?
        };
        self.commit_workspace_directory(root)
    }
    pub(crate) fn add_conversation_workspace(
        &mut self,
        root: PathBuf,
    ) -> Result<(), String> {
        self.capture_conversation_home_anchor();
        // Supersede read-only refresh; registration independently checks/reuses
        // the persisted root before committing the current workspace directory.
        self.conversation_workspaces.pending = None;
        self.start_conversation_registry_job(Some(root))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn optional_registry_unavailable_is_quiet_but_summary_errors_are_not() {
        let mut state = ConversationWorkspaceState::default();
        for _ in 0..3 {
            assert!(apply_project_discovery(
                &mut state,
                ProjectDiscoveryReply {
                    registry: Ok(None),
                    chats: Ok(chat_project_rows(
                        vec!["/known-authorized-history".into()]
                    )),
                }
            ));
            assert!(
                state.error.is_none(),
                "optional absence must not toast every refresh"
            );
            assert!(state.registry.is_empty());
            assert_eq!(state.chat_projects.len(), 1);
        }
        assert!(apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Ok(None),
                chats: Err("503 index initializing".into()),
            }
        ));
        assert_eq!(
            state.error.as_deref(),
            Some("Saved chat projects: 503 index initializing")
        );
        for status in [401, 403, 503] {
            assert!(!apply_project_discovery(
                &mut state,
                ProjectDiscoveryReply {
                    registry: Err(format!("HTTP {status}")),
                    chats: Ok(chat_project_rows(
                        vec!["/known-authorized-history".into()]
                    )),
                }
            ));
            assert!(state
                .error
                .as_deref()
                .unwrap()
                .contains(&format!("HTTP {status}")));
            assert_eq!(state.chat_projects.len(), 1);
        }
    }

    #[test]
    fn discovery_unions_empty_registry_projects_and_all_chat_directories() {
        let mut state = ConversationWorkspaceState::default();
        let registry = registry_rows(serde_json::json!([
            {"id":"no-history", "name":"Empty project", "root":"/empty"},
            {"id":"busy", "name":"Friendly registry name", "root":"/busy/"}
        ]))
        .unwrap();
        // Summary cardinality is directories, not recent conversations: a busy
        // project's >1000 chats cannot displace an old directory from this API.
        let chats = chat_project_rows(vec!["/busy".into(), "/old-chat-only".into()]);
        assert!(apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Ok(Some(registry)),
                chats: Ok(chats)
            }
        ));
        let projects = normalize_projects(
            state
                .registry
                .into_iter()
                .chain(state.chat_projects)
                .collect(),
            None,
        );
        assert_eq!(projects.len(), 3);
        assert!(projects.iter().any(|p| p.root == Path::new("/empty")));
        assert!(projects
            .iter()
            .any(|p| p.root == Path::new("/old-chat-only")));
        assert_eq!(
            projects
                .iter()
                .find(|p| p.root == Path::new("/busy"))
                .unwrap()
                .title,
            "Friendly registry name"
        );
    }

    #[test]
    fn source_failures_are_independent_and_do_not_reuse_old_permission_snapshots() {
        let mut state = ConversationWorkspaceState::default();
        state.registry = chat_project_rows(vec!["/stale-registry".into()]);
        assert!(!apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Err("denied".into()),
                chats: Ok(chat_project_rows(vec!["/authorized-chat".into()])),
            }
        ));
        assert!(state.registry.is_empty());
        assert_eq!(state.chat_projects[0].root, Path::new("/authorized-chat"));
        assert!(state
            .error
            .as_deref()
            .unwrap()
            .contains("Workspace registry: denied"));
        assert!(apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Ok(Some(chat_project_rows(vec!["/registered".into()]))),
                chats: Err("offline".into()),
            }
        ));
        assert_eq!(state.registry[0].root, Path::new("/registered"));
        assert!(state.chat_projects.is_empty());
        assert!(state
            .error
            .as_deref()
            .unwrap()
            .contains("Saved chat projects: offline"));
    }

    #[test]
    fn initializing_summary_is_retried_after_bounded_ttl_not_per_frame() {
        let now = Instant::now();
        let mut state = ConversationWorkspaceState::default();
        state.refreshed = Some(now);
        apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Ok(Some(chat_project_rows(vec!["/empty".into()]))),
                chats: Err("503 index initializing".into()),
            },
        );
        for frame in 0..1000 {
            assert!(!project_discovery_due(
                false,
                state.refreshed,
                now + Duration::from_millis(frame)
            ));
        }
        assert!(project_discovery_due(
            false,
            state.refreshed,
            now + PROJECT_DISCOVERY_INTERVAL
        ));
        assert!(!project_discovery_due(
            true,
            state.refreshed,
            now + PROJECT_DISCOVERY_INTERVAL
        ));
        apply_project_discovery(
            &mut state,
            ProjectDiscoveryReply {
                registry: Ok(Some(chat_project_rows(vec!["/empty".into()]))),
                chats: Ok(chat_project_rows(vec!["/old-chat-only".into()])),
            },
        );
        assert!(state.error.is_none());
        assert_eq!(state.registry[0].root, Path::new("/empty"));
        assert_eq!(state.chat_projects[0].root, Path::new("/old-chat-only"));
    }

    #[test]
    fn directory_selection_identity_is_stable_across_discovery_sources() {
        let registry = registry_rows(serde_json::json!([
            {"id":"registry-id", "name":"Project", "root":"/project/"}
        ]))
        .unwrap();
        let chats = chat_project_rows(vec!["/project".into()]);
        assert_eq!(
            normalize_projects(registry, None)[0].value,
            normalize_projects(chats, None)[0].value
        );
    }

    #[test]
    fn chat_summary_ignores_empty_or_relative_directories_without_home_guesses() {
        let projects =
            chat_project_rows(vec!["".into(), "relative".into(), "/allowed".into()]);
        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].root, Path::new("/allowed"));
    }

    #[test]
    fn saved_directories_are_deduplicated_current_first_then_alphabetical() {
        let rows = registry_rows(serde_json::json!([
            {"id":"z", "name":"Zulu", "root":"/z"},
            {"id":"b", "name":"beta", "root":"/b/"},
            {"id":"a", "name":"Alpha", "root":"/a"},
            {"id":"duplicate", "name":"beta", "root":"/b/./"}
        ]))
        .unwrap();
        let rows = normalize_projects(rows, Some(Path::new("/z")));
        assert_eq!(
            rows.iter().map(|p| p.root.as_path()).collect::<Vec<_>>(),
            vec![Path::new("/z"), Path::new("/a"), Path::new("/b")]
        );
        assert_eq!(rows[2].value, "directory:/b");
    }

    #[test]
    fn registry_id_resolves_to_path_not_workspace_identity() {
        let rows = registry_rows(
            serde_json::json!([{"id":"persisted","name":"Project","root":"/project"}]),
        )
        .unwrap();
        assert_eq!(rows[0].value, "project:persisted");
        assert_eq!(rows[0].root, Path::new("/project"));
        assert!(!rows.iter().any(|p| p.value == "/arbitrary/path"));
    }
    #[test]
    fn first_plain_selection_captures_home_and_never_replaces_saved_anchor() {
        let home = Path::new("/home/me");
        assert_eq!(
            anchor_for_change(None, Some(home), Some(home), false).as_deref(),
            Some(home)
        );
        assert_eq!(
            anchor_for_change(
                Some(Path::new("/")),
                Some(Path::new("/project")),
                Some(home),
                false
            )
            .as_deref(),
            Some(Path::new("/"))
        );
        assert_eq!(
            anchor_for_change(None, Some(Path::new("/project")), Some(home), false)
                .as_deref(),
            Some(home)
        );
    }
    #[test]
    fn remote_anchor_uses_only_saved_or_observed_filesystem_root() {
        let home = Path::new("/home/client");
        assert!(anchor_for_change(None, Some(home), Some(home), true).is_none());
        assert!(
            anchor_for_change(None, Some(Path::new("/project")), None, true).is_none()
        );
        assert_eq!(
            anchor_for_change(None, Some(Path::new("/")), None, true).as_deref(),
            Some(Path::new("/"))
        );
        let mut anchors = HashMap::new();
        anchors.insert("host-A", PathBuf::from("/"));
        assert!(anchors.get("host-B").is_none());
    }
    #[test]
    fn first_terminal_pin_roundtrip_and_repeat_picker_use_fresh_baseline() {
        fn sync(
            state: &mut ConversationWorkspaceState,
            key: &(String, String),
            active: &mut PathBuf,
            cwd: Option<&str>,
        ) {
            let cwd = cwd.map(Path::new);
            state.retire_explicit_root_for_cwd(key, cwd);
            let pinned = state.explicit_roots.get(key).is_some_and(|pin| {
                retain_explicit_root(pin, Some(active.as_path()), cwd)
            });
            if !pinned {
                if let Some(cwd) = cwd {
                    *active = cwd.to_path_buf();
                }
            }
        }

        let key = ("local-host".into(), "workspace".into());
        let other_key = ("other-host".into(), "workspace".into());
        let mut state = ConversationWorkspaceState::default();
        let mut active = PathBuf::from("/B"); // A -> picker B; shell stays at A.
        for key in [&key, &other_key] {
            state.explicit_roots.insert(
                key.clone(),
                ExplicitRoot {
                    root: "/B".into(),
                    previous_cwd: Some("/A".into()),
                },
            );
        }
        for cwd in [Some("/A"), None, Some("/A")] {
            sync(&mut state, &key, &mut active, cwd);
            assert_eq!(active, Path::new("/B"));
            assert!(state.explicit_roots.contains_key(&key));
        }
        // Exact regression: A -> picker B -> cd C -> cd B -> cd A.
        for cwd in ["/C", "/B", "/A"] {
            sync(&mut state, &key, &mut active, Some(cwd));
            assert_eq!(active, Path::new(cwd));
            assert!(!state.explicit_roots.contains_key(&key));
            assert!(state.explicit_roots.contains_key(&other_key));
        }

        sync(&mut state, &key, &mut active, Some("/C"));
        // A second picker records the NEW baseline, not the retired A baseline.
        active = "/D".into();
        state.explicit_roots.insert(
            key.clone(),
            ExplicitRoot {
                root: active.clone(),
                previous_cwd: Some("/C".into()),
            },
        );
        for cwd in [None, Some("/C"), Some("/C")] {
            sync(&mut state, &key, &mut active, cwd);
            assert_eq!(active, Path::new("/D"));
            assert!(state.explicit_roots.contains_key(&key));
        }
        for cwd in ["/A", "/D", "/C"] {
            sync(&mut state, &key, &mut active, Some(cwd));
            assert_eq!(active, Path::new(cwd));
            assert!(!state.explicit_roots.contains_key(&key));
        }
    }

    #[test]
    fn stale_shell_cwd_does_not_undo_explicit_directory_change() {
        let pin = ExplicitRoot {
            root: "/project".into(),
            previous_cwd: Some("/home/me".into()),
        };
        assert!(retain_explicit_root(
            &pin,
            Some(Path::new("/project")),
            Some(Path::new("/home/me"))
        ));
        assert!(retain_explicit_root(
            &pin,
            Some(Path::new("/project")),
            None
        ));
        assert!(!retain_explicit_root(
            &pin,
            Some(Path::new("/project")),
            Some(Path::new("/other"))
        ));
        assert!(!retain_explicit_root(
            &pin,
            Some(Path::new("/other")),
            Some(Path::new("/home/me"))
        ));
    }
}
