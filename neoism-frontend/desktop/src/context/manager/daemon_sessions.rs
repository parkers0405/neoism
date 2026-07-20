use super::*;
use crate::ansi::CursorShape;
use crate::app::ime::Ime;
use crate::app::messenger::Messenger;
#[cfg(not(target_os = "windows"))]
use crate::context::factories::neoism_block_shell_for_spawn;
use crate::context::factories::ROUTE_ID_COUNTER;
use crate::context::renderable::{Cursor, RenderableContent};
use crate::context::tab::Context;
use crate::event::sync::FairMutex;
use crate::layout::{ContextDimension, ContextGrid};
use crate::performer::Machine;
use crate::terminal::blocks::input::TerminalInputBufferHostExt;
use neoism_backend::event::EventListener;
use neoism_backend::event::WindowId;
use neoism_backend::sugarloaf::Sugarloaf;
use neoism_protocol::pty::ClientMessage as PtyClientMessage;
use neoism_protocol::workspace::{
    PaneLayoutOp, WorkspaceClientMessage, WorkspaceTabSummary,
};
use neoism_terminal_core::crosswords::{Crosswords, MIN_COLUMNS, MIN_LINES};
use std::error::Error;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

use neoism_terminal_pty::{PtySession, PtySessionConfig};

impl<T: EventListener + Clone + std::marker::Send + Sync + 'static> ContextManager<T> {
    fn daemon_request(&mut self, message: WorkspaceClientMessage) -> bool {
        let Some(link) = self.daemon.link.clone() else {
            return false;
        };
        self.daemon.cache.last_request_at = Some(Instant::now());
        self.daemon.cache.pending_request_count =
            self.daemon.cache.pending_request_count.saturating_add(1);
        link.send(message);
        true
    }

    pub fn send_workspace_request(&mut self, message: WorkspaceClientMessage) -> bool {
        self.daemon_request(message)
    }

    pub(crate) fn request_switch_session_for_tab(&mut self, tab_index: usize) -> bool {
        let Some(session_id) = self.cached_session_id_for_tab(tab_index) else {
            return false;
        };
        self.daemon.cache.active_session_id = Some(session_id.clone());
        self.daemon_request(WorkspaceClientMessage::SwitchSession { session_id })
    }

    pub(crate) fn request_new_session(
        &mut self,
        cwd: Option<String>,
        label: Option<String>,
    ) -> bool {
        self.daemon_request(WorkspaceClientMessage::NewSession { cwd, label })
    }

    fn request_daemon_pty_for_route(&mut self, route_id: usize) -> bool {
        let Some(link) = self.daemon.link.as_ref() else {
            return false;
        };
        self.daemon.cache.pending_session_routes.push(route_id);
        link.send_pty(PtyClientMessage::CreatePty {
            cwd: self.config.working_dir.clone(),
            cols: MIN_COLUMNS as u16,
            rows: MIN_LINES as u16,
            shell: None,
        });
        true
    }

    fn ensure_daemon_session_for_route(&mut self, route_id: usize) -> bool {
        if self.daemon.cache.route_sessions.contains_key(&route_id)
            || self.daemon.cache.pending_session_routes.contains(&route_id)
            // A pane whose context already carries a remote-PTY binding is
            // running against a connection we kept alive (the parked HOME
            // link across a server round trip). The cache maps were wiped
            // by the switch reset, but the shell is fine — spawning a
            // fresh one here would bury the user's running session under
            // a duplicate.
            || self.route_has_remote_binding(route_id)
        {
            return false;
        }
        self.request_daemon_pty_for_route(route_id)
    }

    fn route_has_remote_binding(&self, route_id: usize) -> bool {
        self.contexts.iter().any(|grid| {
            grid.contexts().values().any(|item| {
                let context = item.context();
                context.route_id == route_id && context.remote_pty.is_some()
            })
        })
    }

    pub(crate) fn ensure_daemon_sessions_for_all_routes(&mut self) {
        let routes: Vec<usize> = self
            .contexts
            .iter()
            .flat_map(|grid| grid.contexts().values().map(|item| item.context().route_id))
            .collect();
        for route_id in routes {
            self.ensure_daemon_session_for_route(route_id);
        }
    }

    /// 8A: build the input-sink half of a daemon-backed pane, or
    /// `None` when the cutover is disabled (`NEOISM_DAEMON_TABS`) or
    /// no usable link exists. Pass the result into `create_context`,
    /// then hand the created context to [`Self::register_remote_context`]
    /// so the daemon spawns its shell.
    pub(super) fn prepared_remote_pty(
        &self,
    ) -> Option<crate::context::remote_pty::PreparedRemotePty> {
        if !crate::context::remote_pty::daemon_tabs_enabled() {
            return None;
        }
        let link = self.daemon.link.as_ref()?;
        let (handle, runtime) = link.handle_and_runtime()?;
        Some(crate::context::remote_pty::prepare(handle, runtime))
    }

    /// 8A: after creating a daemon-backed context, ask the daemon for
    /// its shell. Reuses the ordered pending-route queue that
    /// `PtyCreated` replies resolve against (the same correlation the
    /// mirror-session path uses), so remote panes and mirrors can
    /// interleave safely.
    pub(super) fn register_remote_context(&mut self, context: &Context<T>) {
        let cwd = self.config.working_dir.clone();
        self.register_remote_context_with_cwd(context, cwd);
    }

    /// [`Self::register_remote_context`] with an explicit shell cwd —
    /// the adopt flow spawns the fresh shell in the ADOPTED
    /// workspace's root, not this window's.
    pub(super) fn register_remote_context_with_cwd(
        &mut self,
        context: &Context<T>,
        cwd: Option<String>,
    ) {
        let Some(binding) = context.remote_pty.as_ref() else {
            return;
        };
        let Some(link) = self.daemon.link.as_ref() else {
            return;
        };
        self.daemon
            .cache
            .remote_routes
            .insert(context.route_id, binding.clone());
        self.daemon
            .cache
            .pending_session_routes
            .push(context.route_id);
        link.send_pty(PtyClientMessage::CreatePty {
            cwd,
            cols: context
                .dimension
                .columns
                .try_into()
                .unwrap_or(MIN_COLUMNS as u16),
            rows: context
                .dimension
                .lines
                .try_into()
                .unwrap_or(MIN_LINES as u16),
            shell: None,
        });
    }

    /// 8C: like [`Self::prepared_remote_pty`] but NOT gated on
    /// `NEOISM_DAEMON_TABS` — adopting an existing daemon session is an
    /// explicit user action (a Workspaces-modal pick), not the ambient
    /// new-tab cutover.
    pub(super) fn prepared_remote_pty_for_adopt(
        &self,
    ) -> Option<crate::context::remote_pty::PreparedRemotePty> {
        let link = self.daemon.link.as_ref()?;
        let (handle, runtime) = link.handle_and_runtime()?;
        Some(crate::context::remote_pty::prepare(handle, runtime))
    }

    /// 8C: bind a freshly created context to an EXISTING daemon
    /// session (no `CreatePty` — the shell is already running, e.g. it
    /// was spawned by a web client or by another desktop window).
    /// Maps route↔session, resolves the pane's input sink, and sends
    /// a `Resize` with our geometry so the remote shell repaints its
    /// prompt for this brand-new (empty) grid.
    pub(super) fn register_adopted_context(
        &mut self,
        context: &Context<T>,
        session_id: &str,
    ) {
        let Some(binding) = context.remote_pty.as_ref() else {
            return;
        };
        let Some(link) = self.daemon.link.as_ref() else {
            return;
        };
        self.daemon
            .cache
            .remote_routes
            .insert(context.route_id, binding.clone());
        self.daemon
            .cache
            .route_sessions
            .insert(context.route_id, session_id.to_string());
        self.daemon
            .cache
            .session_routes
            .insert(session_id.to_string(), context.route_id);
        if let Some((handle, runtime)) = link.handle_and_runtime() {
            crate::context::remote_pty::bind_session(
                binding, session_id, handle, runtime,
            );
        }
        // Scrollback first (one-shot backlog reply), then a resize so
        // the remote shell repaints its prompt at our geometry.
        link.send_pty(PtyClientMessage::AttachPty {
            session_id: session_id.to_string(),
        });
        link.send_pty(PtyClientMessage::Resize {
            session_id: session_id.to_string(),
            cols: context
                .dimension
                .columns
                .try_into()
                .unwrap_or(MIN_COLUMNS as u16),
            rows: context
                .dimension
                .lines
                .try_into()
                .unwrap_or(MIN_LINES as u16),
        });
    }

    /// The workspace id a grid publishes/answers to: the DAEMON's id
    /// for adopted grids (8C), the derived desktop id otherwise.
    pub(crate) fn workspace_id_for_grid(
        &self,
        grid: &ContextGrid<T>,
        index: usize,
    ) -> String {
        if let Some(stable) = grid.workspace_route_id() {
            if let Some(adopted) = self.daemon.cache.adopted_workspaces.get(&stable) {
                return adopted.clone();
            }
        }
        desktop_workspace_id(self.window_id, grid, index)
    }

    /// Sync the current grid's FILE buffer-tab list (paths in the
    /// strip) into the publish cache. Re-publishes when it changed so
    /// other clients see the workspace's documents, not just its
    /// shells.
    pub fn set_workspace_buffer_files(
        &mut self,
        grid_root_route: usize,
        files: Vec<PathBuf>,
    ) {
        let changed = self
            .daemon
            .cache
            .workspace_buffer_files
            .get(&grid_root_route)
            .map(|existing| existing != &files)
            .unwrap_or(!files.is_empty());
        if !changed {
            return;
        }
        self.daemon
            .cache
            .workspace_buffer_files
            .insert(grid_root_route, files);
        self.sync_daemon_workspaces();
    }

    /// File-like tabs (editor/markdown/drawing) of a daemon-tree
    /// workspace, in tree order, deduped. Adopt re-opens these so the
    /// adopted Island carries the workspace's FILES, not just its
    /// shells — a workspace holds it all.
    pub fn daemon_workspace_file_paths(&self, workspace_id: &str) -> Vec<PathBuf> {
        // Guest side: a joined workspace's files live on the HOST's
        // disk — the local `is_file` existence check would filter every
        // one of them out. Existence is the host daemon's problem
        // there (its nvim/CRDT opens them); check locally only for
        // workspaces this machine owns.
        let local = self.local_host_id();
        let remote_owned = self
            .daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .is_some_and(|workspace| workspace.host_id != local);
        let mut seen = std::collections::HashSet::new();
        self.daemon
            .cache
            .daemon_workspace_tabs
            .iter()
            .filter(|tab| tab.workspace_id == workspace_id)
            .filter(|tab| {
                matches!(
                    tab.kind.as_deref(),
                    Some("editor") | Some("markdown") | Some("drawing")
                ) || tab.surface_id.is_some()
            })
            .filter_map(|tab| tab.cwd.clone())
            .filter(|path| remote_owned || path.is_file())
            .filter(|path| seen.insert(path.clone()))
            .collect()
    }

    /// The daemon tree's `root_dir` for a workspace, if known. The
    /// adopt flow uses it to seed the file tree / workspace root.
    pub fn daemon_host_workspace_root(&self, workspace_id: &str) -> Option<PathBuf> {
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| workspace.root_dir.clone())
    }

    /// The CURRENT grid's tree identity (daemon id when adopted).
    #[allow(dead_code)]
    pub fn current_workspace_tree_id(&self) -> String {
        self.workspace_id_for_grid(self.current_grid(), self.current_index)
    }

    pub fn workspace_tree_id_for_index(&self, index: usize) -> Option<String> {
        self.contexts
            .get(index)
            .map(|grid| self.workspace_id_for_grid(grid, index))
    }

    pub fn workspace_visibility_for_index(
        &self,
        index: usize,
    ) -> neoism_ui::panels::context_menu::WorkspaceChromeVisibility {
        use neoism_protocol::workspace::WorkspaceVisibility as ProtocolVisibility;
        use neoism_ui::panels::context_menu::WorkspaceChromeVisibility as UiVisibility;

        let Some(workspace_id) = self.workspace_tree_id_for_index(index) else {
            return UiVisibility::Private;
        };
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| match workspace.visibility {
                ProtocolVisibility::Private => UiVisibility::Private,
                ProtocolVisibility::Shared => UiVisibility::Shared,
                ProtocolVisibility::Team => UiVisibility::Team,
            })
            .unwrap_or(UiVisibility::Private)
    }

    pub fn workspace_icon_kind_for_index(&self, index: usize) -> Option<String> {
        use neoism_protocol::workspace::{WorkspaceHostKind, WorkspaceVisibility};

        let workspace_id = self.workspace_tree_id_for_index(index)?;
        let local_host = self.local_host_id();
        self.daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .map(|workspace| {
                // A workspace owned by ANOTHER host that we're showing
                // is one we JOINED — the guest side of sharing gets its
                // own glyph (the owner keeps seeing "shared").
                if workspace.host_id != local_host {
                    return "joined".to_string();
                }
                match workspace.host_kind {
                    WorkspaceHostKind::CloudSandbox => "cloud_sandbox".to_string(),
                    WorkspaceHostKind::DockerSandbox => "docker_sandbox".to_string(),
                    WorkspaceHostKind::Tailscale => "tailscale".to_string(),
                    WorkspaceHostKind::Local => match workspace.visibility {
                        WorkspaceVisibility::Shared | WorkspaceVisibility::Team => {
                            "shared".to_string()
                        }
                        WorkspaceVisibility::Private => "local".to_string(),
                    },
                }
            })
    }

    pub fn workspace_tree_id_for_route(&self, route_id: usize) -> Option<String> {
        self.contexts.iter().enumerate().find_map(|(index, grid)| {
            grid.workspace_route_id()
                .filter(|workspace_route| *workspace_route == route_id)
                .map(|_| self.workspace_id_for_grid(grid, index))
        })
    }

    /// Index of the grid that answers to `workspace_id` (own or
    /// adopted), if any. Used by the Workspaces-modal pick to decide
    /// between "select that tab" and "adopt from the tree".
    pub fn grid_index_for_workspace_id(&self, workspace_id: &str) -> Option<usize> {
        self.contexts.iter().enumerate().find_map(|(index, grid)| {
            (self.workspace_id_for_grid(grid, index) == workspace_id).then_some(index)
        })
    }

    /// 8C adopt: build a real top-level workspace grid out of a
    /// daemon-tree workspace's live terminal sessions. The FIRST
    /// session becomes the grid's root pane; the rest stack as
    /// additional contexts in the same grid. Returns `false` (and
    /// creates nothing) when the workspace has no live sessions to
    /// attach or no usable daemon link exists — callers fall back to
    /// the plain pointer switch.
    ///
    /// `rich_text_id` is the pre-allocated sugarloaf slot for the root
    /// pane (mirrors `create_tab_inner`); stacked panes allocate their
    /// own inside.
    pub fn adopt_daemon_workspace(
        &mut self,
        workspace_id: &str,
        rich_text_id: usize,
        sugarloaf: &mut Sugarloaf,
    ) -> bool {
        // Live, terminal-kind tabs of that workspace, active first.
        // Empty is FINE — a workspace with no live shells still adopts
        // as a real Island with a fresh daemon shell in its root, so
        // picking "Workspace 2" always lands you IN Workspace 2.
        //
        // MULTI-USER: tabs are PERSONAL. Joining someone else's
        // workspace must NOT mirror the owner's open tabs — a guest
        // enters with one fresh shell in the workspace root and builds
        // their own strip. Session re-attach stays for the single-user
        // flow (re-adopting your OWN workspace from another screen).
        let mut tabs: Vec<WorkspaceTabSummary> =
            if self.workspace_owned_locally(workspace_id) {
                self.daemon
                    .cache
                    .daemon_workspace_tabs
                    .iter()
                    .filter(|tab| {
                        tab.workspace_id == workspace_id
                            && tab.session_id.is_some()
                            && matches!(tab.kind.as_deref(), None | Some("terminal"))
                    })
                    .cloned()
                    .collect()
            } else {
                Vec::new()
            };
        tabs.sort_by_key(|tab| std::cmp::Reverse(tab.active));
        if self.prepared_remote_pty_for_adopt().is_none() {
            return false;
        }
        if self.contexts.len() >= self.capacity {
            tracing::warn!("workspace not adopted: capacity reached");
            return false;
        }

        let root_dir = self
            .daemon
            .cache
            .daemon_host_workspaces
            .iter()
            .find(|workspace| workspace.id == workspace_id)
            .and_then(|workspace| workspace.root_dir.clone());

        let mut cloned_config = self.config.clone();
        if let Some(root) = root_dir.as_ref() {
            cloned_config.working_dir = Some(root.to_string_lossy().to_string());
        }

        let current = self.current();
        let cursor = current.cursor_from_ref();
        let blinking = self.config.cursor_blinking;
        let mut dimension = current.dimension;
        if self.current_grid().len() > 1 {
            dimension = self.current_grid().grid_dimension();
        }

        // Root pane = the workspace's active session, or a FRESH
        // daemon shell in the workspace's root when none is live.
        let root_session = tabs.first().and_then(|tab| tab.session_id.clone());
        let root_context = match ContextManager::create_context(
            (&cursor, blinking),
            self.event_proxy.clone(),
            self.window_id,
            rich_text_id,
            dimension,
            &cloned_config,
            self.prepared_remote_pty_for_adopt(),
        ) {
            Ok(context) => context,
            Err(error) => {
                tracing::error!(?error, "adopt: root context creation failed");
                return false;
            }
        };
        match root_session.as_deref() {
            Some(session_id) => self.register_adopted_context(&root_context, session_id),
            None => self.register_remote_context_with_cwd(
                &root_context,
                cloned_config.working_dir.clone(),
            ),
        }

        let last_index = self.contexts.len();
        let previous_scaled_margin = self.contexts[self.current_index].scaled_margin;
        self.contexts.push(ContextGrid::new(
            root_context,
            previous_scaled_margin,
            self.config.split_color,
            self.config.split_active_color,
            self.config.panel,
        ));
        self.current_index = last_index;
        self.current_route = self.current().route_id;

        // Remember the daemon identity BEFORE publishing, so the
        // snapshot re-homes the EXISTING workspace here instead of
        // minting a desktop-flavored duplicate.
        if let Some(stable) = self.contexts[last_index].workspace_route_id() {
            self.daemon
                .cache
                .adopted_workspaces
                .insert(stable, workspace_id.to_string());
        }

        // Remaining sessions stack as sibling tabs in the new grid.
        for tab in tabs.iter().skip(1) {
            let Some(session_id) = tab.session_id.clone() else {
                continue;
            };
            let stacked_rich_text_id = crate::context::next_rich_text_id();
            let _ = sugarloaf.text(Some(stacked_rich_text_id));
            let stacked_dimension = self.current_grid().grid_dimension();
            let stacked_cursor = self.current().cursor_from_ref();
            match ContextManager::create_context(
                (&stacked_cursor, blinking),
                self.event_proxy.clone(),
                self.window_id,
                stacked_rich_text_id,
                stacked_dimension,
                &cloned_config,
                self.prepared_remote_pty_for_adopt(),
            ) {
                Ok(stacked) => {
                    self.register_adopted_context(&stacked, &session_id);
                    if self.contexts[self.current_index]
                        .add_stacked_context(stacked, sugarloaf)
                        .is_none()
                    {
                        tracing::warn!("adopt: stacked tab attach failed");
                    }
                }
                Err(error) => {
                    tracing::warn!(?error, "adopt: stacked context creation failed");
                }
            }
        }

        // Re-publish: the adopted workspace now runs (also) here. The
        // grid answers to the daemon id, so the daemon upserts the
        // EXISTING entry rather than growing a copy.
        self.sync_daemon_workspaces();
        true
    }

    pub(crate) fn request_close_current_session(&mut self) -> bool {
        let Some(session_id) = self.current_cached_session_id() else {
            return false;
        };
        self.daemon_request(WorkspaceClientMessage::CloseSession { session_id })
    }

    pub(crate) fn request_pane_layout_op(
        &mut self,
        pane_external_id: u64,
        op: PaneLayoutOp,
    ) -> bool {
        self.daemon_request(WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id,
            op,
        })
    }

    #[inline]
    pub(crate) fn create_context(
        cursor_state: (&Cursor, bool),
        event_proxy: T,
        window_id: WindowId,
        rich_text_id: usize,
        dimension: ContextDimension,
        config: &ContextManagerConfig,
        // 8A: when `Some`, the pane's shell lives in the workspace
        // daemon — build a remote `PtySession` (channel bridge) instead
        // of spawning a local process. Editor-source contexts ignore it.
        remote_pty: Option<crate::context::remote_pty::PreparedRemotePty>,
    ) -> Result<Context<T>, Box<dyn Error>> {
        let route_id = ROUTE_ID_COUNTER.fetch_add(1, Ordering::SeqCst);
        let cols: u16 = dimension.columns.try_into().unwrap_or(MIN_COLUMNS as u16);
        let rows: u16 = dimension.lines.try_into().unwrap_or(MIN_LINES as u16);

        let terminal_dimension = crate::bridges::utils::ResizeDimensions {
            columns: dimension.columns.max(1),
            lines: dimension.lines.max(1),
        };
        let mut terminal = Crosswords::new(
            terminal_dimension,
            CursorShape::from_char(cursor_state.0.content),
            neoism_backend::TerminalId::from(route_id),
            config.scrollback_history_limit,
        );
        terminal.blinking_cursor = cursor_state.1;
        // Also the DEFAULT, so DECSCUSR 0 (cursor reset, e.g. nvim on
        // exit) restores the config's blink instead of disabling it.
        terminal.default_blinking_cursor = cursor_state.1;

        let terminal: Arc<FairMutex<Crosswords>> = Arc::new(FairMutex::new(terminal));

        let pty;
        let mut remote_binding = None;
        #[cfg(not(target_os = "windows"))]
        {
            if let Some(prepared) = remote_pty {
                // 8A: daemon-hosted shell. Same Machine/Messenger
                // machinery; bytes are bridged by the context manager
                // (daemon `PtyOutput` → feed, `Msg::Input`/`Resize` →
                // sink → daemon link).
                tracing::info!(
                    route_id,
                    "rio -> neoism_terminal_pty: PtySession::remote (daemon-hosted shell)"
                );
                let (session, feed) = PtySession::remote(prepared.sink);
                remote_binding = Some(crate::context::remote_pty::RemotePtyBinding {
                    feed,
                    shared: prepared.shared,
                });
                pty = session;
            } else {
                let spawn_shell = neoism_block_shell_for_spawn(&config.shell, route_id)
                    .unwrap_or_else(|| config.shell.clone());
                tracing::info!("rio -> neoism_terminal_pty: PtySession::spawn");
                let session_config = PtySessionConfig {
                    shell: Some(spawn_shell.program.clone()),
                    args: spawn_shell.args.clone(),
                    cwd: config.working_dir.as_ref().map(PathBuf::from),
                    env: Vec::new(),
                    cols,
                    rows,
                };
                pty = match PtySession::spawn(session_config) {
                    Ok(session) => session,
                    Err(err) => {
                        tracing::error!("{err:?}");
                        return Err(Box::new(err));
                    }
                };
            }
        }

        #[cfg(not(target_os = "windows"))]
        let main_fd = pty.main_fd();
        #[cfg(not(target_os = "windows"))]
        let shell_pid = pty.shell_pid();

        #[cfg(target_os = "windows")]
        {
            // Remote (daemon-hosted) panes are unix-only for now.
            let _ = remote_pty;
            tracing::info!("rio -> neoism_terminal_pty: PtySession::spawn (windows)");
            let session_config = PtySessionConfig {
                shell: Some(config.shell.program.clone()),
                args: config.shell.args.clone(),
                cwd: config.working_dir.as_ref().map(PathBuf::from),
                env: Vec::new(),
                cols,
                rows,
            };
            pty = match PtySession::spawn(session_config) {
                Ok(session) => session,
                Err(err) => {
                    tracing::error!("{err:?}");
                    return Err(Box::new(err));
                }
            };
        }

        let machine = Machine::new(
            Arc::clone(&terminal),
            pty,
            event_proxy.clone(),
            window_id,
            route_id,
        )?;
        let channel = machine.channel();
        let io_thread = if config.spawn_performer {
            Some(machine.spawn())
        } else {
            None
        };

        let messenger = Messenger::new(channel);

        let mut terminal_input = crate::terminal::blocks::TerminalInputBuffer::default();
        let terminal_shell_kind =
            crate::terminal::blocks::TerminalShellKind::detect(&config.shell.program);
        terminal_input.enable_persistent_history_for_shell(terminal_shell_kind);
        terminal_input.enable_persistent_favorites_default();

        Ok(Context {
            route_id,
            #[cfg(not(target_os = "windows"))]
            main_fd,
            #[cfg(not(target_os = "windows"))]
            shell_pid,
            messenger,
            terminal,
            terminal_input,
            terminal_shell_kind,
            rich_text_id,
            renderable_content: RenderableContent::new(cursor_state.0.clone()),
            dimension,
            pending_terminal_resize: false,
            pending_splash: true,
            splash_dim_stable_frames: 0,
            splash_last_dim: (0, 0),
            splash_last_cursor_row: 0,
            splash_injection: None,
            ime: Ime::new(),
            remote_pty: remote_binding,
            _io_thread: io_thread,
            markdown: None,
            code: None,
            draw: None,
            notebook: None,
            neoism_agent: None,
            neoism_tags: None,
            neoism_extensions: None,
        })
    }

}
