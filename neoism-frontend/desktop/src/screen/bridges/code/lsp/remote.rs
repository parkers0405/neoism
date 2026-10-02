//! Owning-daemon transport for the same native LSP jobs/results as local panes.
//! No host path is canonicalized or opened on the guest. Pending requests are
//! registered before send and keyed by endpoint + window + request ID; pane,
//! workspace, document and revision are checked again at delivery.
use super::*;
use neoism_protocol::editor::{
    EditorClientMessage as Request, EditorLspAction as Action,
    EditorLspActionCapability, EditorLspBufferSnapshot, EditorLspEditOperation,
    EditorLspMutationPlan, EditorLspOpenBuffer, EditorServerMessage as Reply,
};
use std::time::{Duration, Instant};

type Pill = (
    neoism_ui::panels::status_line::LspStatus,
    String,
    Vec<neoism_ui::panels::lsp_popup::LspServerRow>,
);

#[derive(Clone, Debug)]
struct Document {
    window: WindowId,
    endpoint: String,
    workspace: String,
    root: PathBuf,
    route: usize,
    file: PathBuf,
}

#[derive(Clone)]
pub(crate) struct RemoteLuaLspCodeAction {
    pub(super) target: neoism_lua::LuaLspTarget,
    document: Document,
    capability: EditorLspActionCapability,
    open_revisions: Vec<(usize, PathBuf, u64)>,
}

pub(crate) struct RemoteLuaLspPreparedMutation {
    pub(crate) target: neoism_lua::LuaLspTarget,
    document: Document,
    operation: neoism_lua::LuaLspOperation,
    plan: EditorLspMutationPlan,
    open_revisions: Vec<(usize, PathBuf, u64)>,
}

impl Document {
    fn identity(
        &self,
    ) -> (
        WindowId,
        &str,
        &str,
        &std::ffi::OsStr,
        usize,
        &std::ffi::OsStr,
    ) {
        (
            self.window,
            &self.endpoint,
            &self.workspace,
            self.root.as_os_str(),
            self.route,
            self.file.as_os_str(),
        )
    }
}
impl PartialEq for Document {
    fn eq(&self, other: &Self) -> bool {
        self.identity() == other.identity()
    }
}
impl Eq for Document {}
impl std::hash::Hash for Document {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        std::hash::Hash::hash(&self.identity(), state);
    }
}

#[derive(Clone)]
enum Kind {
    Sync,
    Query,
    Apply,
    Format(u64),
    LuaRead {
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        target: neoism_lua::LuaLspTarget,
    },
    LuaEditPrepare {
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        target: neoism_lua::LuaLspTarget,
    },
    LuaEditCommit {
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        target: neoism_lua::LuaLspTarget,
        plan: EditorLspMutationPlan,
    },
    LuaEditFinalize {
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        target: neoism_lua::LuaLspTarget,
        mutation: neoism_lua::LuaLspMutation,
    },
}
#[derive(Clone)]
struct Pending {
    document: Document,
    surface: String,
    revision: u64,
    seq: u64,
    kind: Kind,
    action: Option<Action>,
    open_revisions: Vec<(usize, PathBuf, u64)>,
    since: Instant,
}
struct Subscription {
    transport: Option<(
        crate::daemon_client::DaemonClientHandle,
        tokio::runtime::Handle,
    )>,
    document: Document,
    surface: String,
    revision: u64,
    synced: Option<u64>,
    attempts: u8,
    due: Instant,
    pill: Option<Pill>,
    reported_errors: std::collections::HashSet<String>,
}
#[derive(Default)]
struct RemoteState {
    pending: HashMap<(WindowId, String, u64), Pending>,
    subscriptions: HashMap<(WindowId, usize), Subscription>,
    focus: HashMap<WindowId, Document>,
    failures: Vec<(WindowId, String, u64, String)>,
}
fn state() -> &'static Mutex<RemoteState> {
    static STATE: OnceLock<Mutex<RemoteState>> = OnceLock::new();
    STATE.get_or_init(Default::default)
}
fn lock() -> std::sync::MutexGuard<'static, RemoteState> {
    state().lock().unwrap_or_else(|p| p.into_inner())
}

pub(crate) fn cancel_remote_lua_lsp_request(
    owner: &neoism_lua::PluginOwner,
    id: &str,
) -> bool {
    let mut removed = false;
    lock().pending.retain(|_, pending| {
        let matches = match &pending.kind {
            Kind::LuaRead {
                owner: pending_owner,
                id: pending_id,
                ..
            }
            | Kind::LuaEditPrepare {
                owner: pending_owner,
                id: pending_id,
                ..
            }
            | Kind::LuaEditCommit {
                owner: pending_owner,
                id: pending_id,
                ..
            }
            | Kind::LuaEditFinalize {
                owner: pending_owner,
                id: pending_id,
                ..
            } => pending_owner == owner && pending_id == id,
            _ => false,
        };
        removed |= matches;
        !matches
    });
    removed
}

impl Drop for Subscription {
    fn drop(&mut self) {
        if let Some((handle, runtime)) = self.transport.take() {
            let surface_id = Some(self.surface.clone());
            let root = self.document.root.clone();
            runtime.spawn(async move {
                let id = handle.allocate_request_id();
                let _ = handle
                    .send_editor_with_request_id(
                        id,
                        Request::CloseLspBuffer { surface_id },
                        Some(root),
                    )
                    .await;
            });
        }
    }
}

impl Subscription {
    fn begin_sync(
        &mut self,
        revision: u64,
        pending: bool,
        changed_focus: bool,
        now: Instant,
    ) -> bool {
        if changed_focus || self.revision != revision {
            self.revision = revision;
            self.attempts = 0;
            self.due = now;
        }
        if pending || self.attempts >= 3 || now < self.due {
            return false;
        }
        self.attempts += 1;
        self.due = now + Duration::from_secs(3);
        true
    }
}
fn interactive_matches(
    pending: &Pending,
    focused: Option<&Document>,
    revision: Option<u64>,
) -> bool {
    focused == Some(&pending.document) && revision == Some(pending.revision)
}
fn owner_matches_link(document: &Document, endpoint: Option<&str>) -> bool {
    endpoint == Some(document.endpoint.as_str())
}

fn passive_lsp_request(kind: &Kind, action: Option<Action>) -> bool {
    matches!(kind, Kind::Sync)
        || matches!(
            action,
            Some(
                Action::Hover
                    | Action::Completion
                    | Action::SignatureHelp
                    | Action::DocumentHighlight
            )
        )
}

fn report_passive_error(sub: &mut Subscription, message: &str) -> bool {
    sub.reported_errors.insert(message.to_owned())
}

impl Screen<'_> {
    fn remote_lua_open_buffers(
        &self,
        document: &Document,
    ) -> Result<(Vec<EditorLspOpenBuffer>, Vec<(usize, PathBuf, u64)>), String> {
        let root = neoism_protocol::host_path::HostPath::new(
            document.root.to_string_lossy().into_owned(),
        );
        let mut snapshots = Vec::new();
        let mut unique = std::collections::BTreeMap::new();
        for item in self
            .context_manager
            .all_grids()
            .iter()
            .flat_map(|grid| grid.contexts().values())
        {
            let route = item.context().route_id;
            if self
                .context_manager
                .adopted_workspace_identity_for_route(route)
                != Some((document.endpoint.as_str(), document.workspace.as_str()))
            {
                continue;
            }
            let Some(code) = item.context().code.as_ref().filter(|code| code_uses_host_lsp(code))
            else {
                continue;
            };
            let path = code.path.to_string_lossy().into_owned();
            if root.relative(&path).is_none() {
                continue;
            }
            if unique.insert(path.clone(), code.buffer.revision).is_some_and(|revision| {
                revision != code.buffer.revision
            }) {
                return Err("remote LSP buffer has conflicting open revisions".into());
            }
            snapshots.push((route, code.path.clone(), code.buffer.revision));
        }
        snapshots.sort_by(|a, b| (a.0, &a.1, a.2).cmp(&(b.0, &b.1, b.2)));
        Ok((
            unique
                .into_iter()
                .map(|(path, revision)| EditorLspOpenBuffer { path, revision })
                .collect(),
            snapshots,
        ))
    }

    fn remote_lua_buffer_snapshots(
        &mut self,
        document: &Document,
    ) -> Result<(Vec<EditorLspBufferSnapshot>, Vec<(usize, PathBuf, u64)>), String> {
        let (_, revisions) = self.remote_lua_open_buffers(document)?;
        let mut unique = std::collections::BTreeMap::new();
        for (route, path, revision) in &revisions {
            let code = self
                .context_manager
                .get_by_route_id(*route)
                .and_then(|item| item.context().code.as_ref())
                .ok_or_else(|| "remote LSP buffer disappeared during synchronization".to_string())?;
            let identity = path.to_string_lossy().into_owned();
            let snapshot = EditorLspBufferSnapshot {
                path: identity.clone(),
                revision: *revision,
                text: code.buffer.text(),
            };
            if unique.insert(identity, snapshot.clone()).is_some_and(|prior| {
                prior.revision != snapshot.revision || prior.text != snapshot.text
            }) {
                return Err("remote LSP buffer has conflicting live content".into());
            }
        }
        Ok((unique.into_values().collect(), revisions))
    }

    fn send_remote_lua_lsp_request(
        &mut self,
        document: Document,
        request_id: u64,
        surface: String,
        revision: u64,
        kind: Kind,
        open_revisions: Vec<(usize, PathBuf, u64)>,
        request: Request,
    ) -> Result<(), String> {
        let Some((handle, runtime)) = self.context_manager.daemon_link_handle_and_runtime() else {
            return Err("workspace LSP connection is unavailable".into());
        };
        let key = (document.window, document.endpoint.clone(), request_id);
        lock().pending.insert(
            key.clone(),
            Pending {
                document: document.clone(),
                surface,
                revision,
                seq: request_id,
                kind,
                action: None,
                open_revisions,
                since: Instant::now(),
            },
        );
        let proxy = self.context_manager.event_proxy_clone();
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_editor_with_request_id(request_id, request, Some(document.root))
                .await
            {
                let mut state = lock();
                if state.pending.contains_key(&key) {
                    state
                        .failures
                        .push((key.0, key.1, key.2, error.to_string()));
                }
            }
            proxy.send_event(RioEventType::Rio(RioEvent::Render), document.window);
            tokio::time::sleep(Duration::from_secs(21)).await;
            proxy.send_event(RioEventType::Rio(RioEvent::Render), document.window);
        });
        Ok(())
    }

    pub(super) fn dispatch_remote_lua_lsp_request(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        operation: neoism_lua::LuaLspOperation,
        arguments: &serde_json::Value,
    ) -> Result<neoism_lua::LuaLspTarget, String> {
        let read_operation = lua_read_operation(operation);
        let edit_operation = lua_edit_operation(operation);
        if read_operation.is_none() && edit_operation.is_none() {
            return Err(format!(
                "structured {operation:?} requests are not implemented remotely yet"
            ));
        }
        let document = self
            .remote_lsp_document()
            .ok_or_else(|| "there is no focused remote code buffer".to_string())?;
        if !owner_matches_link(&document, self.context_manager.daemon_endpoint()) {
            return Err(
                "workspace LSP connection is not attached; retry after reconnecting"
                    .into(),
            );
        }
        let (cursor_line, cursor_col, revision, focused_text) = self
            .context_manager
            .current()
            .code
            .as_ref()
            .map(|code| {
                (
                    code.buffer.cursor_line,
                    code.buffer.cursor_col,
                    code.buffer.revision,
                    code.buffer.text(),
                )
            })
            .ok_or_else(|| "there is no focused remote code buffer".to_string())?;
        let host_root = neoism_protocol::host_path::HostPath::new(
            document.root.to_string_lossy().into_owned(),
        );
        if let Some(requested_root) = super::lua::argument_str(arguments, "root")? {
            if requested_root != host_root.as_str() {
                return Err(
                    "structured LSP requests cannot leave the focused workspace root"
                        .into(),
                );
            }
        }
        let requested_path = super::lua::argument_str(arguments, "path")?;
        let host_file = match requested_path {
            Some(path) => resolve_remote_lua_target_path(&host_root, path)?,
            None => neoism_protocol::host_path::HostPath::new(
                document.file.to_string_lossy().into_owned(),
            ),
        };
        let line =
            super::lua::argument_u32(arguments, &["line"], cursor_line)?;
        let character = super::lua::argument_u32(
            arguments,
            &["character", "column"],
            cursor_col,
        )?;
        let focused = host_path_eq(&PathBuf::from(host_file.as_str()), &document.file);
        let mut target = neoism_lua::LuaLspTarget {
            root: host_root.as_str().to_owned(),
            path: host_file.as_str().to_owned(),
            line,
            character,
            buffer_revision: focused.then_some(revision),
            pane_id: document.route as u64,
        };
        let (open_buffers, open_revisions) = self.remote_lua_open_buffers(&document)?;
        if let Some((route, _, open_revision)) = open_revisions
            .iter()
            .find(|(_, path, _)| host_path_eq(path, &PathBuf::from(host_file.as_str())))
        {
            target.pane_id = *route as u64;
            target.buffer_revision = Some(*open_revision);
        }
        if operation == neoism_lua::LuaLspOperation::Format
            && target.buffer_revision.is_none()
        {
            return Err("structured format requires an open target buffer".into());
        }
        let request_id = (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        let surface = format!("lua-lsp:{}:{request_id}", document.route);
        let (kind, request) = if let Some(wire_operation) = read_operation {
            (
                Kind::LuaRead {
                    owner,
                    id,
                    operation,
                    target: target.clone(),
                },
                Request::LspRead {
                    operation: wire_operation,
                    path: host_file.as_str().to_owned(),
                    line,
                    character,
                    query: super::lua::argument_str(arguments, "query")?
                        .unwrap_or_default()
                        .to_owned(),
                    buffer_text: focused.then_some(focused_text),
                    surface_id: Some(surface.clone()),
                },
            )
        } else {
            let wire_operation = edit_operation.expect("checked structured edit operation");
            let argument = match operation {
                neoism_lua::LuaLspOperation::Rename => {
                    let value = super::lua::argument_str(arguments, "newName")?
                        .filter(|value| !value.trim().is_empty())
                        .ok_or_else(|| "rename requires a non-empty newName".to_string())?;
                    Some(value.to_owned())
                }
                _ => None,
            };
            (
                Kind::LuaEditPrepare {
                    owner,
                    id,
                    operation,
                    target: target.clone(),
                },
                Request::LspEditPrepare {
                    operation: wire_operation,
                    path: host_file.as_str().to_owned(),
                    line,
                    character,
                    argument,
                    action: None,
                    buffer_text: focused.then_some(focused_text),
                    open_buffers,
                    surface_id: Some(surface.clone()),
                },
            )
        };
        self.send_remote_lua_lsp_request(
            document,
            request_id,
            surface,
            revision,
            kind,
            open_revisions,
            request,
        )?;
        Ok(target)
    }

    pub(crate) fn dispatch_remote_lua_code_action(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        action: RemoteLuaLspCodeAction,
    ) -> Result<neoism_lua::LuaLspTarget, String> {
        if !owner_matches_link(&action.document, self.context_manager.daemon_endpoint()) {
            return Err("workspace LSP connection is no longer attached".into());
        }
        let (open_buffers, open_revisions) = self.remote_lua_open_buffers(&action.document)?;
        if open_revisions != action.open_revisions {
            return Err("an open remote buffer changed before action selection".into());
        }
        if action.target.buffer_revision.is_some_and(|revision| {
            self.context_manager
                .get_by_route_id(action.document.route)
                .and_then(|item| item.context().code.as_ref())
                .is_none_or(|code| {
                    !code_uses_host_lsp(code)
                        || !host_path_eq(&code.path, &PathBuf::from(&action.target.path))
                        || code.buffer.revision != revision
                })
        }) {
            return Err("remote LSP target changed before action selection".into());
        }
        let buffer_text = action.target.buffer_revision.and_then(|_| {
            self.context_manager
                .get_by_route_id(action.document.route)
                .and_then(|item| item.context().code.as_ref())
                .map(|code| code.buffer.text())
        });
        let request_id = (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        let surface = format!("lua-lsp:{}:{request_id}", action.document.route);
        let request = Request::LspEditPrepare {
            operation: EditorLspEditOperation::ApplyCodeAction,
            path: action.target.path.clone(),
            line: action.target.line,
            character: action.target.character,
            argument: None,
            action: Some(action.capability),
            buffer_text,
            open_buffers,
            surface_id: Some(surface.clone()),
        };
        let target = action.target;
        self.send_remote_lua_lsp_request(
            action.document,
            request_id,
            surface,
            target.buffer_revision.unwrap_or_default(),
            Kind::LuaEditPrepare {
                owner,
                id,
                operation: neoism_lua::LuaLspOperation::ApplyCodeAction,
                target: target.clone(),
            },
            open_revisions,
            request,
        )?;
        Ok(target)
    }

    pub(crate) fn dispatch_remote_lua_mutation(
        &mut self,
        owner: neoism_lua::PluginOwner,
        id: String,
        prepared: RemoteLuaLspPreparedMutation,
    ) -> Result<neoism_lua::LuaLspTarget, String> {
        if !owner_matches_link(&prepared.document, self.context_manager.daemon_endpoint()) {
            return Err("workspace LSP connection is no longer attached".into());
        }
        let (open_buffers, current) = self.remote_lua_open_buffers(&prepared.document)?;
        if current != prepared.open_revisions {
            return Err("remote buffer state changed before mutation commit".into());
        }
        if prepared.target.buffer_revision.is_some_and(|revision| {
            self.context_manager
                .get_by_route_id(prepared.document.route)
                .and_then(|item| item.context().code.as_ref())
                .is_none_or(|code| {
                    !code_uses_host_lsp(code)
                        || !host_path_eq(&code.path, &PathBuf::from(&prepared.target.path))
                        || code.buffer.revision != revision
                })
        }) {
            return Err("remote LSP target changed before mutation commit".into());
        }
        let request_id = (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        let surface = format!("lua-lsp:{}:{request_id}", prepared.document.route);
        let request = Request::LspEditCommit {
            plan_id: prepared.plan.plan_id.clone(),
            open_buffers,
            surface_id: Some(surface.clone()),
        };
        let target = prepared.target;
        self.send_remote_lua_lsp_request(
            prepared.document,
            request_id,
            surface,
            target.buffer_revision.unwrap_or_default(),
            Kind::LuaEditCommit {
                owner,
                id,
                operation: prepared.operation,
                target: target.clone(),
                plan: prepared.plan,
            },
            current,
            request,
        )?;
        Ok(target)
    }

    /// Menu acceptance can arrive before the next paint after a pane switch.
    /// Do not apply a previously visible host's completion/action to the new
    /// pane merely because both buffers have the same raw path.
    pub(super) fn remote_code_lsp_ui_scope_current(&mut self) -> bool {
        let focused = self.remote_lsp_document();
        let prior = lock().focus.get(&self.context_manager.window_id()).cloned();
        if prior == focused {
            return true;
        }
        self.renderer.code_lsp = Default::default();
        self.mark_dirty();
        false
    }

    pub(crate) fn clear_remote_code_lsp(&self) {
        let window = self.context_manager.window_id();
        let mut state = lock();
        let removed = state
            .pending
            .extract_if(|(w, _, _), _| *w == window)
            .map(|(_, pending)| pending)
            .collect::<Vec<_>>();
        state.subscriptions.retain(|(w, _), _| *w != window);
        state.failures.retain(|(w, _, _, _)| *w != window);
        state.focus.remove(&window);
        drop(state);
        for pending in removed {
            enqueue_remote_lua_error(
                &pending,
                "stale_target",
                "remote LSP target was closed before the reply arrived",
            );
        }
    }

    fn remote_lsp_document(&self) -> Option<Document> {
        if !self.code_lsp_is_remote() {
            return None;
        }
        let (root, file) = self.code_lsp_target()?;
        // Adopted identity is authoritative. Never substitute the bootstrap
        // daemon, even for a share hosted on localhost.
        Some(Document {
            window: self.context_manager.window_id(),
            endpoint: self
                .context_manager
                .current_adopted_workspace_endpoint()?
                .to_string(),
            workspace: self.context_manager.current_adopted_workspace_id()?,
            root,
            file,
            route: self.context_manager.current().route_id,
        })
    }

    pub(super) fn remote_code_lsp_pill(&self, file: &Path) -> Option<Pill> {
        let document = self.remote_lsp_document()?;
        if !host_path_eq(&document.file, file) {
            return None;
        }
        lock()
            .subscriptions
            .get(&(document.window, document.route))
            .filter(|s| s.document == document)
            .and_then(|s| s.pill.clone())
    }

    /// All native input paths (including idle probes and completion commands)
    /// use this single route boundary. Remote jobs can never reach the engine.
    pub(super) fn dispatch_code_lsp(&mut self, job: CodeLspJob) -> Result<(), ()> {
        if !self.code_lsp_is_remote() {
            return self.code_lsp_shared().jobs.send(job).map_err(|_| ());
        }
        let Some(document) = self.remote_lsp_document() else {
            return Err(());
        };
        let code = self.context_manager.current().code.as_ref().ok_or(())?;
        let revision = code.buffer.revision;
        let buffer_text = code.buffer.text();
        let seq_fallback = QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        // Only this workspace's live buffers are eligible for client edits.
        let open_paths = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .filter_map(|i| {
                i.context()
                    .code
                    .as_ref()
                    .filter(|c| code_uses_host_lsp(c))
                    .map(|c| c.path.clone())
            })
            .collect::<Vec<_>>();
        let open_revisions = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .filter_map(|i| {
                i.context()
                    .code
                    .as_ref()
                    .filter(|c| code_uses_host_lsp(c))
                    .map(|c| (i.context().route_id, c.path.clone(), c.buffer.revision))
            })
            .collect();
        let surface = {
            let mut state = lock();
            let sub = state
                .subscriptions
                .entry((document.window, document.route))
                .or_insert_with(|| subscription(document.clone(), revision));
            if sub.document != document {
                *sub = subscription(document.clone(), revision);
            }
            sub.surface.clone()
        };
        let query = |seq, action, line, character, text| Request::LspQueryAt {
            seq,
            action,
            path: document.file.clone(),
            line,
            character,
            text,
            buffer_text: Some(buffer_text.clone()),
            open_paths: open_paths.clone(),
            surface_id: Some(surface.clone()),
        };
        let (request, seq, kind) = match job {
            CodeLspJob::Sync { .. } => (
                Request::OpenBuffer {
                    path: document.file.clone(),
                    text: Some(buffer_text),
                    line: None,
                    character: None,
                    surface_id: Some(surface.clone()),
                },
                seq_fallback,
                Kind::Sync,
            ),
            CodeLspJob::Save { .. } => (
                Request::DidSave {
                    path: document.file.clone(),
                    surface_id: Some(surface.clone()),
                },
                seq_fallback,
                Kind::Query,
            ),
            CodeLspJob::Completion {
                seq,
                line,
                character,
                trigger,
                ..
            } => (
                query(seq, Action::Completion, line, character, trigger),
                seq,
                Kind::Query,
            ),
            CodeLspJob::Hover {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::Hover, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::Definition {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::Definition, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::References {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::References, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::SignatureHelp {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::SignatureHelp, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::DocumentHighlight {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::DocumentHighlight, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::DocumentSymbols { seq, .. } => (
                query(seq, Action::DocumentSymbols, 0, 0, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::CodeActions {
                seq,
                line,
                character,
                ..
            } => (
                query(seq, Action::CodeActions, line, character, None),
                seq,
                Kind::Query,
            ),
            CodeLspJob::Rename {
                seq,
                line,
                character,
                new_name,
                ..
            } => (
                query(seq, Action::Rename, line, character, Some(new_name)),
                seq,
                Kind::Apply,
            ),
            CodeLspJob::FormatThenSave { revision, .. } => (
                query(seq_fallback, Action::Format, 0, 0, None),
                seq_fallback,
                Kind::Format(revision),
            ),
            CodeLspJob::ApplyCodeAction {
                server_id,
                title,
                mut action,
                ..
            } => {
                let expected = action
                    .as_object_mut()
                    .and_then(|a| a.remove("_neoismRevision"))
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default();
                use sha2::{Digest, Sha256};
                if !expected.is_empty()
                    && expected != format!("{:x}", Sha256::digest(buffer_text.as_bytes()))
                {
                    self.remote_lsp_error(
                        "Code action is stale; request fresh actions after editing",
                    );
                    return Err(());
                }
                (
                    Request::ApplyLspCodeActionAt {
                        buffer_text: Some(buffer_text.clone()),
                        seq: seq_fallback,
                        action: neoism_protocol::editor::EditorLspCodeAction {
                            server_id,
                            file_path: document.file.clone(),
                            document_revision: expected,
                            title,
                            kind: None,
                            preferred: false,
                            disabled_reason: None,
                            payload: action,
                        },
                        open_paths,
                        surface_id: Some(surface.clone()),
                    },
                    seq_fallback,
                    Kind::Apply,
                )
            }
            CodeLspJob::CompletionCommand {
                server_id, command, ..
            } => (
                Request::ApplyLspCodeActionAt {
                    buffer_text: Some(buffer_text.clone()),
                    seq: seq_fallback,
                    action: neoism_protocol::editor::EditorLspCodeAction {
                        server_id,
                        file_path: document.file.clone(),
                        document_revision: String::new(),
                        title: "Completion".into(),
                        kind: None,
                        preferred: false,
                        disabled_reason: None,
                        payload: command,
                    },
                    open_paths,
                    surface_id: Some(surface.clone()),
                },
                seq_fallback,
                Kind::Apply,
            ),
        };
        // Only the link to this workspace's owner is allowed. A tab switch may
        // briefly leave a different active link: fail/retry, never misroute.
        let action = match &request {
            Request::LspQueryAt { action, .. } => Some(*action),
            _ => None,
        };
        if !owner_matches_link(&document, self.context_manager.daemon_endpoint()) {
            let message =
                "Workspace LSP connection is not attached; retry after reconnecting";
            let notify = if action == Some(Action::DocumentHighlight) {
                false
            } else if passive_lsp_request(&kind, action) {
                lock()
                    .subscriptions
                    .get_mut(&(document.window, document.route))
                    .is_none_or(|sub| report_passive_error(sub, message))
            } else {
                true
            };
            if notify {
                self.remote_lsp_error(message);
            }
            return Err(());
        }
        let Some((handle, runtime)) =
            self.context_manager.daemon_link_handle_and_runtime()
        else {
            return Err(());
        };
        if let Some(sub) = lock()
            .subscriptions
            .get_mut(&(document.window, document.route))
        {
            if sub
                .transport
                .as_ref()
                .is_none_or(|(old, _)| old.connection_key() != handle.connection_key())
            {
                let mut handle = handle.clone();
                let _ = handle.take_editor_connection_change();
                sub.transport = Some((handle, runtime.clone()));
            }
        }
        // Globally unique native IDs survive connection replacement to the
        // same endpoint; low IDs remain available to ordinary plane senders.
        let request_id = (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
        let key = (document.window, document.endpoint.clone(), request_id);
        let expects_reply = !matches!(request, Request::DidSave { .. });
        if expects_reply {
            let mut state = lock();
            if matches!(kind, Kind::Query) {
                state.pending.retain(|_, p| {
                    !(p.document == document
                        && matches!(p.kind, Kind::Query)
                        && p.action == action)
                });
            }
            state.pending.insert(
                key.clone(),
                Pending {
                    document: document.clone(),
                    surface: surface.clone(),
                    revision,
                    seq,
                    kind,
                    action,
                    open_revisions,
                    since: Instant::now(),
                },
            );
        }
        let proxy = self.context_manager.event_proxy_clone();
        runtime.spawn(async move {
            if let Err(error) = handle
                .send_editor_with_request_id(request_id, request, Some(document.root))
                .await
            {
                let mut state = lock();
                if state.pending.contains_key(&key) {
                    state
                        .failures
                        .push((key.0, key.1, key.2, error.to_string()));
                }
            }
            proxy.send_event(RioEventType::Rio(RioEvent::Render), document.window);
            // Wake even on a lost reply; timeouts must not depend on typing.
            tokio::time::sleep(Duration::from_secs(21)).await;
            proxy.send_event(RioEventType::Rio(RioEvent::Render), document.window);
        });
        Ok(())
    }

    fn remote_lsp_error(&mut self, message: &str) {
        self.renderer.notifications.push(
            format!("LSP: {message}"),
            neoism_ui::panels::notifications::NotificationLevel::Error,
        );
        self.mark_dirty();
    }

    fn recover_remote_lsp_connections(&mut self) {
        let window = self.context_manager.window_id();
        let reconnects = {
            let mut state = lock();
            state
                .subscriptions
                .values_mut()
                .filter_map(|sub| {
                    if sub.document.window != window {
                        return None;
                    }
                    let (handle, runtime) = sub.transport.as_mut()?;
                    if handle.take_editor_connection_change() != Some(true) {
                        return None;
                    }
                    sub.attempts = 0;
                    sub.synced = None;
                    sub.due = Instant::now();
                    Some((
                        sub.document.clone(),
                        sub.surface.clone(),
                        handle.clone(),
                        runtime.clone(),
                    ))
                })
                .collect::<Vec<_>>()
        };
        for (document, surface, handle, runtime) in reconnects {
            if self
                .context_manager
                .adopted_workspace_identity_for_route(document.route)
                != Some((document.endpoint.as_str(), document.workspace.as_str()))
            {
                continue;
            }
            let Some(code) = self
                .context_manager
                .get_by_route_id(document.route)
                .and_then(|item| item.context().code.as_ref())
                .filter(|code| {
                    code_uses_host_lsp(code)
                        && host_path_eq(&code.path, &document.file)
                        && !code.remote_content_pending
                })
            else {
                continue;
            };
            let revision = code.buffer.revision;
            let text = code.buffer.text();
            let id = (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
            let key = (window, document.endpoint.clone(), id);
            {
                let mut state = lock();
                state.pending.retain(|_, p| {
                    !(p.document == document && matches!(p.kind, Kind::Sync))
                });
                state.pending.insert(
                    key.clone(),
                    Pending {
                        document: document.clone(),
                        surface: surface.clone(),
                        revision,
                        seq: id,
                        kind: Kind::Sync,
                        action: None,
                        open_revisions: Vec::new(),
                        since: Instant::now(),
                    },
                );
            }
            let proxy = self.context_manager.event_proxy_clone();
            runtime.spawn(async move {
                let request = Request::OpenBuffer {
                    path: document.file,
                    text: Some(text),
                    line: None,
                    character: None,
                    surface_id: Some(surface),
                };
                if let Err(error) = handle
                    .send_editor_with_request_id(id, request, Some(document.root))
                    .await
                {
                    let mut state = lock();
                    if state.pending.contains_key(&key) {
                        state
                            .failures
                            .push((key.0, key.1, key.2, error.to_string()));
                    }
                }
                proxy.send_event(RioEventType::Rio(RioEvent::Render), window);
                tokio::time::sleep(Duration::from_secs(21)).await;
                proxy.send_event(RioEventType::Rio(RioEvent::Render), window);
            });
        }
    }

    pub(super) fn pump_remote_code_lsp(&mut self) {
        self.recover_remote_lsp_connections();
        let window = self.context_manager.window_id();
        let routes = self
            .context_manager
            .all_grids()
            .iter()
            .flat_map(|g| {
                g.contexts()
                    .values()
                    .filter(|i| i.context().code.as_ref().is_some_and(code_uses_host_lsp))
                    .map(|i| i.context().route_id)
            })
            .collect::<Vec<_>>();
        let failures = {
            let mut state = lock();
            state
                .subscriptions
                .retain(|(w, r), _| *w != window || routes.contains(r));
            state.pending.retain(|(w, _, _), p| {
                *w != window
                    || routes.contains(&p.document.route)
                    || matches!(
                        p.kind,
                        Kind::LuaRead { .. }
                            | Kind::LuaEditPrepare { .. }
                            | Kind::LuaEditCommit { .. }
                            | Kind::LuaEditFinalize { .. }
                    )
            });
            let mut failures = Vec::new();
            state.failures.retain(|(w, endpoint, id, message)| {
                if *w != window {
                    return true;
                }
                failures.push((endpoint.clone(), *id, message.clone()));
                false
            });
            for ((w, endpoint, id), pending) in &state.pending {
                if *w == window && pending.since.elapsed() >= Duration::from_secs(20) {
                    failures.push((endpoint.clone(), *id, "Request timed out; buffer status will retry (actions are never replayed)".into()));
                }
            }
            failures
        };
        for (endpoint, id, message) in failures {
            self.apply_remote_code_lsp_message(
                &endpoint,
                id,
                &Reply::Error {
                    surface_id: None,
                    message,
                },
            );
        }
        let focused = self.remote_lsp_document();
        let changed_focus = {
            let mut state = lock();
            match &focused {
                Some(document) => {
                    state.focus.insert(window, document.clone()).as_ref()
                        != Some(document)
                }
                None => state.focus.remove(&window).is_some(),
            }
        };
        if changed_focus {
            self.renderer.code_lsp = Default::default();
        }
        let Some(document) = focused else {
            return;
        };
        let revision = self
            .context_manager
            .current()
            .code
            .as_ref()
            .unwrap()
            .buffer
            .revision;
        let active_transport = self
            .context_manager
            .daemon_link_handle_and_runtime()
            .filter(|_| {
                owner_matches_link(&document, self.context_manager.daemon_endpoint())
            });
        let due = {
            let mut state = lock();
            let pending = state
                .pending
                .values()
                .any(|p| p.document == document && matches!(p.kind, Kind::Sync));
            let sub = state
                .subscriptions
                .entry((window, document.route))
                .or_insert_with(|| subscription(document.clone(), revision));
            if sub.document != document {
                *sub = subscription(document.clone(), revision);
            }
            let new_connection = active_transport.as_ref().is_some_and(|(handle, _)| {
                sub.transport.as_ref().is_some_and(|(old, _)| {
                    old.connection_key() != handle.connection_key()
                })
            });
            if new_connection {
                sub.transport = None;
            }
            let due = sub.begin_sync(
                revision,
                pending && !new_connection,
                changed_focus || new_connection,
                Instant::now(),
            );
            if new_connection {
                state.pending.retain(|_, p| {
                    !(p.document == document && matches!(p.kind, Kind::Sync))
                });
            }
            due
        };
        if due {
            let text = self
                .context_manager
                .current()
                .code
                .as_ref()
                .unwrap()
                .buffer
                .text();
            if self
                .dispatch_code_lsp(CodeLspJob::Sync {
                    root: document.root,
                    file: document.file,
                    text,
                })
                .is_err()
            {
                self.wake_remote_lsp_after(Duration::from_secs(3));
            }
        }
    }

    pub(crate) fn apply_remote_code_lsp_message(
        &mut self,
        endpoint: &str,
        request_id: u64,
        message: &Reply,
    ) -> bool {
        let window = self.context_manager.window_id();
        if let Reply::Batch { messages, .. } = message {
            // Snapshot batches contain one correlated status and diagnostic
            // pushes with their own surface routes.
            let mut changed = false;
            for message in messages {
                changed |=
                    self.apply_remote_code_lsp_message(endpoint, request_id, message);
            }
            return changed;
        }
        if let Reply::Diagnostics {
            surface_id,
            file_path: Some(file),
            items,
            ..
        } = message
        {
            let document = lock()
                .subscriptions
                .values()
                .find(|s| {
                    s.document.window == window
                        && s.document.endpoint == endpoint
                        && Some(&s.surface) == surface_id.as_ref()
                        && host_path_eq(&s.document.file, file)
                })
                .map(|s| s.document.clone());
            let Some(document) = document else {
                return false;
            };
            if self
                .context_manager
                .adopted_workspace_identity_for_route(document.route)
                != Some((document.endpoint.as_str(), document.workspace.as_str()))
            {
                return false;
            }
            let Some(item) = self.context_manager.get_by_route_id(document.route) else {
                return false;
            };
            let Some(code) = item.context_mut().code.as_mut().filter(|c| {
                code_uses_host_lsp(c) && host_path_eq(&c.path, &document.file)
            }) else {
                return false;
            };
            let diagnostics = items
                .iter()
                .map(|d| DocumentDiagnostic {
                    start_line: d.line as usize,
                    start_byte: d.col as usize,
                    end_line: d.end_line as usize,
                    end_byte: d.end_col as usize,
                    severity: match d.severity {
                        neoism_protocol::editor::DiagnosticSeverity::Error => {
                            CodeDiagnosticSeverity::Error
                        }
                        neoism_protocol::editor::DiagnosticSeverity::Warn => {
                            CodeDiagnosticSeverity::Warn
                        }
                        neoism_protocol::editor::DiagnosticSeverity::Info => {
                            CodeDiagnosticSeverity::Info
                        }
                        neoism_protocol::editor::DiagnosticSeverity::Hint => {
                            CodeDiagnosticSeverity::Hint
                        }
                    },
                    message: d.message.clone(),
                })
                .collect::<Vec<_>>();
            code.diag_anchors.clear();
            code.diagnostic_summaries = diagnostics
                .iter()
                .map(|d| CodeDiagnosticSummary {
                    line: d.start_line,
                    byte: d.start_byte,
                    severity: d.severity,
                    message: d.message.clone(),
                })
                .collect();
            code.diagnostics =
                project_diagnostic_ranges(&code.buffer.lines, &diagnostics);
            code.diagnostics_resolved_revision = Some(code.buffer.revision);
            self.mark_dirty();
            return true;
        }
        let surface = match message {
            Reply::LspSnapshot { surface_id, .. }
            | Reply::LspQueryResult { surface_id, .. }
            | Reply::LspReadResult { surface_id, .. }
            | Reply::LspEditPrepared { surface_id, .. }
            | Reply::LspEditCommitted { surface_id, .. }
            | Reply::LspEditFinalized { surface_id, .. }
            | Reply::LspHoverResult { surface_id, .. }
            | Reply::LspCompletions { surface_id, .. }
            | Reply::Error { surface_id, .. } => surface_id.as_deref(),
            _ => return false,
        };
        let key = (window, endpoint.to_string(), request_id);
        let pending = {
            let mut state = lock();
            if surface.is_some_and(|surface| {
                state.pending.get(&key).is_none_or(|p| p.surface != surface)
            }) {
                return false;
            }
            state.pending.remove(&key)
        };
        let Some(pending) = pending else {
            return false;
        };
        let document = &pending.document;
        let subscription_valid = lock()
            .subscriptions
            .get(&(window, document.route))
            .is_some_and(|s| s.document == *document);
        let route_valid = self
            .context_manager
            .adopted_workspace_identity_for_route(document.route)
            == Some((document.endpoint.as_str(), document.workspace.as_str()));
        let lua_request = matches!(
            pending.kind,
            Kind::LuaRead { .. }
                | Kind::LuaEditPrepare { .. }
                | Kind::LuaEditCommit { .. }
                | Kind::LuaEditFinalize { .. }
        );
        if !route_valid || (!lua_request && !subscription_valid) {
            if lua_request {
                enqueue_remote_lua_error(
                    &pending,
                    "stale_target",
                    "remote LSP target is no longer active",
                );
                return true;
            }
            return false;
        }
        if let Kind::LuaEditPrepare {
            owner,
            id,
            operation,
            target,
        } = &pending.kind
        {
            let stale = target.buffer_revision.is_some_and(|revision| {
                self.context_manager
                    .get_by_route_id(document.route)
                    .and_then(|item| item.context().code.as_ref())
                    .is_none_or(|code| {
                        !code_uses_host_lsp(code)
                            || !host_path_eq(&code.path, &PathBuf::from(&target.path))
                            || code.buffer.revision != revision
                    })
            }) || self
                .remote_lua_open_buffers(document)
                .map_or(true, |(_, current)| current != pending.open_revisions);
            if stale {
                enqueue_remote_lua_error(
                    &pending,
                    "stale_buffer",
                    "remote buffer state changed before edit preparation completed",
                );
                return true;
            }
            let Reply::LspEditPrepared {
                operation: got,
                actions,
                plan,
                ..
            } = message
            else {
                let message = match message {
                    Reply::Error { message, .. } => message.as_str(),
                    _ => "daemon returned an unexpected structured edit reply",
                };
                enqueue_remote_lua_error(&pending, "remote_error", message);
                return true;
            };
            if lua_edit_operation(*operation) != Some(*got) {
                enqueue_remote_lua_error(
                    &pending,
                    "mismatched_operation",
                    "daemon returned a different structured edit operation",
                );
                return true;
            }
            if *operation == neoism_lua::LuaLspOperation::CodeActions {
                if plan.is_some() {
                    enqueue_remote_lua_error(
                        &pending,
                        "invalid_remote_result",
                        "code action listing unexpectedly returned a mutation plan",
                    );
                    return true;
                }
                let public = actions
                    .iter()
                    .map(|action| neoism_lua::LuaLspCodeAction {
                        id: action.action_id.clone(),
                        request_id: id.clone(),
                        title: action.title.clone(),
                        kind: action.kind.clone(),
                        preferred: action.preferred,
                    })
                    .collect::<Vec<_>>();
                let private = actions
                    .iter()
                    .cloned()
                    .map(|action| {
                        (
                            action.action_id.clone(),
                            super::lua::LuaLspRetainedCodeAction::Remote(
                                RemoteLuaLspCodeAction {
                                    target: target.clone(),
                                    document: pending.document.clone(),
                                    capability: action,
                                    open_revisions: pending.open_revisions.clone(),
                                },
                            ),
                        )
                    })
                    .collect();
                super::lua::enqueue_lua_lsp_private_completion(
                    owner.clone(),
                    neoism_lua::LuaLspCompletion::success(
                        id.clone(),
                        *operation,
                        target.clone(),
                        neoism_lua::LuaLspOutcome::CodeActions(public),
                    ),
                    window,
                    super::lua::LuaLspPrivateResult::CodeActions(private),
                );
                return true;
            }
            if !actions.is_empty() {
                enqueue_remote_lua_error(
                    &pending,
                    "invalid_remote_result",
                    "structured mutation preparation unexpectedly returned code actions",
                );
                return true;
            }
            let Some(plan) = plan.clone() else {
                enqueue_remote_lua_error(
                    &pending,
                    "invalid_remote_result",
                    "structured mutation preparation did not return a plan",
                );
                return true;
            };
            let host_root = neoism_protocol::host_path::HostPath::new(
                document.root.to_string_lossy().into_owned(),
            );
            let open_paths = pending
                .open_revisions
                .iter()
                .map(|(_, path, _)| path.to_string_lossy().into_owned())
                .collect::<std::collections::HashSet<_>>();
            let mut plan_paths = std::collections::HashSet::new();
            if plan.files.iter().any(|file| {
                host_root.relative(&file.path).is_none()
                    || !plan_paths.insert(file.path.clone())
                    || file.open != open_paths.contains(&file.path)
            }) {
                enqueue_remote_lua_error(
                    &pending,
                    "invalid_remote_result",
                    "structured mutation plan contains invalid file ownership or paths",
                );
                return true;
            }
            super::lua::enqueue_lua_lsp_private_completion(
                owner.clone(),
                neoism_lua::LuaLspCompletion::success(
                    id.clone(),
                    *operation,
                    target.clone(),
                    match operation {
                        neoism_lua::LuaLspOperation::ApplyCodeAction => {
                            neoism_lua::LuaLspOutcome::ApplyCodeAction(
                                neoism_lua::LuaLspMutation {
                                    title: plan.title.clone(),
                                    changed_files: Vec::new(),
                                    ran_command: false,
                                },
                            )
                        }
                        neoism_lua::LuaLspOperation::Rename => {
                            neoism_lua::LuaLspOutcome::Rename(neoism_lua::LuaLspMutation {
                                title: plan.title.clone(),
                                changed_files: Vec::new(),
                                ran_command: false,
                            })
                        }
                        neoism_lua::LuaLspOperation::Format => {
                            neoism_lua::LuaLspOutcome::Format(neoism_lua::LuaLspMutation {
                                title: plan.title.clone(),
                                changed_files: Vec::new(),
                                ran_command: false,
                            })
                        }
                        _ => unreachable!("only remote mutations produce plans"),
                    },
                ),
                window,
                super::lua::LuaLspPrivateResult::RemoteMutation(
                    RemoteLuaLspPreparedMutation {
                        target: target.clone(),
                        document: pending.document.clone(),
                        operation: *operation,
                        plan,
                        open_revisions: pending.open_revisions.clone(),
                    },
                ),
            );
            return true;
        }
        if let Kind::LuaEditCommit {
            owner,
            id,
            operation,
            target,
            plan,
        } = &pending.kind
        {
            let stale = target.buffer_revision.is_some_and(|revision| {
                self.context_manager
                    .get_by_route_id(document.route)
                    .and_then(|item| item.context().code.as_ref())
                    .is_none_or(|code| {
                        !code_uses_host_lsp(code)
                            || !host_path_eq(&code.path, &PathBuf::from(&target.path))
                            || code.buffer.revision != revision
                    })
            }) || self
                .remote_lua_open_buffers(document)
                .map_or(true, |(_, current)| current != pending.open_revisions);
            if stale {
                enqueue_remote_lua_error(
                    &pending,
                    "stale_buffer",
                    "remote buffer state changed before mutation commit completed",
                );
                return true;
            }
            let Reply::LspEditCommitted {
                title,
                edits,
                applied_files,
                ran_command,
                command_id,
                ..
            } = message
            else {
                let message = match message {
                    Reply::Error { message, .. } => message.as_str(),
                    _ => "daemon returned an unexpected structured commit reply",
                };
                enqueue_remote_lua_error(&pending, "remote_error", message);
                return true;
            };
            let host_root = neoism_protocol::host_path::HostPath::new(
                document.root.to_string_lossy().into_owned(),
            );
            let expected_open = plan
                .files
                .iter()
                .filter(|file| file.open)
                .map(|file| (file.path.as_str(), file.edit_count))
                .collect::<std::collections::HashMap<_, _>>();
            let expected_closed = plan
                .files
                .iter()
                .filter(|file| !file.open)
                .map(|file| file.path.as_str())
                .collect::<std::collections::HashSet<_>>();
            let applied = applied_files.iter().map(String::as_str).collect::<std::collections::HashSet<_>>();
            let mut returned = std::collections::HashSet::new();
            if *ran_command
                || (command_id.is_some()
                    && *operation != neoism_lua::LuaLspOperation::ApplyCodeAction)
                || applied.len() != applied_files.len()
                || applied != expected_closed
                || edits.iter().any(|file| {
                    host_root.relative(&file.path).is_none()
                        || !returned.insert(file.path.as_str())
                        || expected_open.get(file.path.as_str()).copied()
                            != Some(file.edits.len())
                })
                || returned.len() != expected_open.len()
            {
                enqueue_remote_lua_error(
                    &pending,
                    "invalid_remote_result",
                    "daemon commit result did not match its prepared mutation plan",
                );
                return true;
            }
            let mut applications = Vec::new();
            for file in edits {
                let typed = file
                    .edits
                    .iter()
                    .map(|edit| CodeTextEdit {
                        start_line: edit.start_line as usize,
                        start_col: edit.start_col as usize,
                        end_line: edit.end_line as usize,
                        end_col: edit.end_col as usize,
                        text: edit.new_text.clone(),
                    })
                    .collect::<Vec<_>>();
                let routes = pending
                    .open_revisions
                    .iter()
                    .filter(|(_, path, _)| host_path_eq(path, &PathBuf::from(&file.path)))
                    .map(|(route, _, _)| *route)
                    .collect::<Vec<_>>();
                if routes.is_empty() {
                    enqueue_remote_lua_error(
                        &pending,
                        "invalid_remote_result",
                        "daemon returned edits for a buffer not owned by this frontend",
                    );
                    return true;
                }
                for route in routes {
                    let Some(code) = self
                        .context_manager
                        .get_by_route_id(route)
                        .and_then(|item| item.context().code.as_ref())
                    else {
                        enqueue_remote_lua_error(
                            &pending,
                            "stale_buffer",
                            "remote edit buffer disappeared before mutation",
                        );
                        return true;
                    };
                    if let Err(message) = code.buffer.validate_text_edits(&typed) {
                        enqueue_remote_lua_error(&pending, "invalid_remote_result", &message);
                        return true;
                    }
                    applications.push((route, file.path.clone(), typed.clone()));
                }
            }
            let mut changed_files = plan
                .files
                .iter()
                .map(|file| neoism_lua::LuaLspChangedFile {
                    path: file.path.clone(),
                    edit_count: file.edit_count,
                    applied_by: if file.open {
                        neoism_lua::LuaLspMutationOwner::Frontend
                    } else {
                        neoism_lua::LuaLspMutationOwner::Daemon
                    },
                })
                .collect::<Vec<_>>();
            changed_files.sort_by(|a, b| a.path.cmp(&b.path));
            for (route, _, edits) in applications {
                let Some(code) = self
                    .context_manager
                    .get_by_route_id(route)
                    .and_then(|item| item.context_mut().code.as_mut())
                else {
                    enqueue_remote_lua_error(
                        &pending,
                        "stale_buffer",
                        "remote edit buffer disappeared during mutation",
                    );
                    return true;
                };
                code.buffer.apply_text_edits(&edits);
                code.buffer.follow_cursor = true;
            }
            self.sync_active_code_modified();
            self.mark_dirty();
            let mutation = neoism_lua::LuaLspMutation {
                title: title.clone(),
                changed_files,
                ran_command: false,
            };
            if let Some(command_id) = command_id {
                let Ok((buffers, current)) = self.remote_lua_buffer_snapshots(document) else {
                    enqueue_remote_lua_error(
                        &pending,
                        "command_failed",
                        "failed to capture edited remote buffers before command execution",
                    );
                    return true;
                };
                let finalize_id =
                    (1u64 << 63) | QUERY_SEQ.fetch_add(1, Ordering::SeqCst);
                let surface = format!("lua-lsp:{}:{finalize_id}", document.route);
                let request = Request::LspEditFinalize {
                    command_id: command_id.clone(),
                    buffers,
                    surface_id: Some(surface.clone()),
                };
                if let Err(message) = self.send_remote_lua_lsp_request(
                    pending.document.clone(),
                    finalize_id,
                    surface,
                    pending.revision,
                    Kind::LuaEditFinalize {
                        owner: owner.clone(),
                        id: id.clone(),
                        operation: *operation,
                        target: target.clone(),
                        mutation,
                    },
                    current,
                    request,
                ) {
                    enqueue_remote_lua_error(&pending, "command_failed", &message);
                }
                return true;
            }
            let outcome = match operation {
                neoism_lua::LuaLspOperation::ApplyCodeAction => {
                    neoism_lua::LuaLspOutcome::ApplyCodeAction(mutation)
                }
                neoism_lua::LuaLspOperation::Rename => {
                    neoism_lua::LuaLspOutcome::Rename(mutation)
                }
                neoism_lua::LuaLspOperation::Format => {
                    neoism_lua::LuaLspOutcome::Format(mutation)
                }
                _ => unreachable!("only remote mutation operations commit plans"),
            };
            super::lua::enqueue_lua_lsp_completion(
                owner.clone(),
                neoism_lua::LuaLspCompletion::success(
                    id.clone(),
                    *operation,
                    target.clone(),
                    outcome,
                ),
                window,
            );
            return true;
        }
        if let Kind::LuaEditFinalize {
            owner,
            id,
            operation,
            target,
            mutation,
        } = &pending.kind
        {
            let Reply::LspEditFinalized {
                ran_command: true,
                ..
            } = message
            else {
                let message = match message {
                    Reply::Error { message, .. } => message.as_str(),
                    _ => "daemon returned an unexpected structured command reply",
                };
                enqueue_remote_lua_error(&pending, "command_failed", message);
                return true;
            };
            let mut mutation = mutation.clone();
            mutation.ran_command = true;
            super::lua::enqueue_lua_lsp_completion(
                owner.clone(),
                neoism_lua::LuaLspCompletion::success(
                    id.clone(),
                    *operation,
                    target.clone(),
                    neoism_lua::LuaLspOutcome::ApplyCodeAction(mutation),
                ),
                window,
            );
            return true;
        }
        if let Kind::LuaRead {
            owner,
            id,
            operation,
            target,
        } = &pending.kind
        {
            if target.buffer_revision.is_some_and(|revision| {
                self.context_manager
                    .get_by_route_id(document.route)
                    .and_then(|item| item.context().code.as_ref())
                    .is_none_or(|code| {
                        !code_uses_host_lsp(code)
                            || !host_path_eq(&code.path, &PathBuf::from(&target.path))
                            || code.buffer.revision != revision
                    })
            }) {
                enqueue_remote_lua_error(
                    &pending,
                    "stale_buffer",
                    "remote buffer changed before the LSP reply arrived",
                );
                return true;
            }
            let completion = match message {
                Reply::LspReadResult {
                    operation: got,
                    outcome,
                    ..
                } if lua_read_operation(*operation) == Some(*got) => {
                    match lua_read_outcome(outcome.clone()) {
                        Ok(outcome) => neoism_lua::LuaLspCompletion::success(
                            id.clone(),
                            *operation,
                            target.clone(),
                            outcome,
                        ),
                        Err(error) => neoism_lua::LuaLspCompletion::failed(
                            id.clone(),
                            *operation,
                            Some(target.clone()),
                            "invalid_remote_result",
                            error.to_string(),
                        ),
                    }
                }
                Reply::LspReadResult { .. } => neoism_lua::LuaLspCompletion::failed(
                    id.clone(),
                    *operation,
                    Some(target.clone()),
                    "mismatched_operation",
                    "daemon returned a different structured LSP operation",
                ),
                Reply::Error { message, .. } => neoism_lua::LuaLspCompletion::failed(
                    id.clone(),
                    *operation,
                    Some(target.clone()),
                    "remote_error",
                    message.clone(),
                ),
                _ => neoism_lua::LuaLspCompletion::failed(
                    id.clone(),
                    *operation,
                    Some(target.clone()),
                    "invalid_remote_result",
                    "daemon returned an unexpected structured LSP reply",
                ),
            };
            super::lua::enqueue_lua_lsp_completion(owner.clone(), completion, window);
            return true;
        }
        if let Reply::Error { message, .. } = message {
            if self.remote_lsp_document().as_ref() == Some(document) {
                let ui = &mut self.renderer.code_lsp;
                if ui.completion.as_ref().is_some_and(|s| s.seq == pending.seq) {
                    ui.completion = None;
                }
                if ui.hover.as_ref().is_some_and(|s| s.seq == pending.seq) {
                    ui.hover = None;
                }
                if ui.actions.as_ref().is_some_and(|s| s.seq == pending.seq) {
                    ui.actions = None;
                }
                for seq in [
                    &mut ui.definition_seq,
                    &mut ui.references_seq,
                    &mut ui.rename_seq,
                    &mut ui.signature_seq,
                ] {
                    if *seq == Some(pending.seq) {
                        *seq = None;
                    }
                }
                if pending.action == Some(Action::DocumentSymbols)
                    && self.renderer.finder.mode()
                        == neoism_ui::panels::finder::FinderMode::Symbols
                {
                    self.renderer.finder.set_symbol_rows(Vec::new());
                }
            }
            let mut notify = pending.action != Some(Action::DocumentHighlight);
            if matches!(pending.kind, Kind::Sync) {
                if let Some(sub) = lock().subscriptions.get_mut(&(window, document.route))
                {
                    sub.synced = None;
                    sub.pill = Some(error_pill(message));
                    sub.due = Instant::now() + Duration::from_secs(2);
                    notify = report_passive_error(sub, message);
                }
                self.wake_remote_lsp_after(Duration::from_secs(2));
            } else if notify && passive_lsp_request(&pending.kind, pending.action) {
                if let Some(sub) = lock().subscriptions.get_mut(&(window, document.route))
                {
                    notify = report_passive_error(sub, message);
                }
            }
            if notify {
                self.remote_lsp_error(message);
            }
            if matches!(pending.kind, Kind::Format(_))
                && self.remote_lsp_document().as_ref() == Some(document)
            {
                // Local parity: a failed formatter must not swallow Save.
                // Never fall back to writing the host pathname on this guest.
                if !self.save_current_code_via_daemon() {
                    self.remote_lsp_error(
                        "Save remains pending; reconnect and save again",
                    );
                }
            }
            return true;
        }
        if let Reply::LspSnapshot {
            file_path, servers, ..
        } = message
        {
            if !matches!(pending.kind, Kind::Sync)
                || file_path
                    .as_ref()
                    .is_none_or(|file| !host_path_eq(file, &document.file))
            {
                return false;
            }
            if let Some(sub) = lock().subscriptions.get_mut(&(window, document.route)) {
                sub.synced = Some(pending.revision);
                sub.attempts = 0;
                sub.pill = Some(snapshot_pill(servers));
                sub.due = if sub.revision == pending.revision {
                    Instant::now() + Duration::from_secs(10)
                } else {
                    Instant::now()
                };
            }
            if let Some(item) = self.context_manager.get_by_route_id(document.route) {
                if let Some(code) = item.context_mut().code.as_mut().filter(|c| {
                    code_uses_host_lsp(c)
                        && host_path_eq(&c.path, &document.file)
                        && c.buffer.revision == pending.revision
                }) {
                    code.lsp_synced_revision = Some(pending.revision);
                }
            }
            self.wake_remote_lsp_after(Duration::from_secs(10));
            self.mark_dirty();
            return true;
        }
        if matches!(pending.kind, Kind::Format(_))
            && self.remote_lsp_document().as_ref() == Some(document)
        {
            // The common landing skips stale edits but still saves the latest
            // text of this exact document, matching the local formatter.
            if let Some(results) = parse_reply(&pending, message) {
                self.apply_code_lsp_results(results);
                return true;
            }
        }
        // Interactive landings are never allowed to paint another pane, even
        // if it happens to have the same path/revision on a different host.
        if !interactive_matches(
            &pending,
            self.remote_lsp_document().as_ref(),
            self.context_manager
                .current()
                .code
                .as_ref()
                .map(|c| c.buffer.revision),
        ) {
            return false;
        }
        if let Reply::LspQueryResult {
            seq,
            action,
            edits,
            applied_files,
            title,
            ran_command,
            ..
        } = message
        {
            if *seq == pending.seq
                && matches!(pending.kind, Kind::Apply)
                && matches!(action, Action::Rename | Action::CodeActions)
            {
                // Preflight every client edit before mutating any live buffer.
                // Captured routes, not global path lookup, prevent cross-host edits.
                for edit in edits {
                    let matches = pending
                        .open_revisions
                        .iter()
                        .filter(|(_, path, _)| host_path_eq(path, &edit.path))
                        .collect::<Vec<_>>();
                    if matches.is_empty()
                        || matches.iter().any(|(route, path, revision)| {
                            self.context_manager
                                .get_by_route_id(*route)
                                .and_then(|i| i.context().code.as_ref())
                                .is_none_or(|c| {
                                    !code_uses_host_lsp(c)
                                        || !host_path_eq(&c.path, path)
                                        || c.buffer.revision != *revision
                                })
                        })
                    {
                        self.remote_lsp_error("Workspace edit is stale or its pane closed; request the action again");
                        return true;
                    }
                }
                for edit in edits {
                    let parsed = parse_lsp_text_edits(&raw_edits(&edit.edits));
                    for (route, path, _) in &pending.open_revisions {
                        if !host_path_eq(path, &edit.path) {
                            continue;
                        }
                        if let Some(code) = self
                            .context_manager
                            .get_by_route_id(*route)
                            .and_then(|i| i.context_mut().code.as_mut())
                        {
                            code.buffer.apply_text_edits(&parsed);
                        }
                    }
                }
                self.sync_active_code_modified();
                let touched = edits.len() + applied_files.len();
                let (message, level) = if touched > 0 {
                    (
                        format!("{title} — {touched} files"),
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    )
                } else if *ran_command {
                    (
                        title.clone(),
                        neoism_ui::panels::notifications::NotificationLevel::Info,
                    )
                } else {
                    (
                        format!("{title}: no edit returned"),
                        neoism_ui::panels::notifications::NotificationLevel::Warn,
                    )
                };
                self.renderer.notifications.push(message, level);
                self.mark_dirty();
                return true;
            }
        }
        let Some(results) = parse_reply(&pending, message) else {
            if matches!(message, Reply::LspQueryResult { action: Action::Definition, seq, .. } if *seq == pending.seq)
            {
                self.remote_lsp_error(
                    "Cannot decode the definition target using the host's path syntax",
                );
            }
            return false;
        };
        self.apply_code_lsp_results(results);
        true
    }

    fn wake_remote_lsp_after(&self, delay: Duration) {
        if let Some((_, runtime)) = self.context_manager.daemon_link_handle_and_runtime()
        {
            let proxy = self.context_manager.event_proxy_clone();
            let window = self.context_manager.window_id();
            runtime.spawn(async move {
                tokio::time::sleep(delay).await;
                proxy.send_event(RioEventType::Rio(RioEvent::Render), window);
            });
        }
    }
}

fn lua_read_operation(
    operation: neoism_lua::LuaLspOperation,
) -> Option<neoism_protocol::editor::EditorLspReadOperation> {
    use neoism_lua::LuaLspOperation as Lua;
    use neoism_protocol::editor::EditorLspReadOperation as Wire;
    Some(match operation {
        Lua::Hover => Wire::Hover,
        Lua::SignatureHelp => Wire::SignatureHelp,
        Lua::Definition => Wire::Definition,
        Lua::References => Wire::References,
        Lua::DocumentSymbols => Wire::DocumentSymbols,
        Lua::WorkspaceSymbols => Wire::WorkspaceSymbols,
        Lua::Diagnostics => Wire::Diagnostics,
        Lua::Clients => Wire::Clients,
        _ => return None,
    })
}

fn lua_edit_operation(
    operation: neoism_lua::LuaLspOperation,
) -> Option<EditorLspEditOperation> {
    Some(match operation {
        neoism_lua::LuaLspOperation::CodeActions => EditorLspEditOperation::CodeActions,
        neoism_lua::LuaLspOperation::ApplyCodeAction => {
            EditorLspEditOperation::ApplyCodeAction
        }
        neoism_lua::LuaLspOperation::Rename => EditorLspEditOperation::Rename,
        neoism_lua::LuaLspOperation::Format => EditorLspEditOperation::Format,
        _ => return None,
    })
}

fn lua_read_outcome(
    outcome: neoism_protocol::editor::EditorLspReadOutcome,
) -> Result<neoism_lua::LuaLspOutcome, serde_json::Error> {
    serde_json::to_value(outcome).and_then(serde_json::from_value)
}

fn host_path_is_absolute(
    root: &neoism_protocol::host_path::HostPath,
    path: &str,
) -> bool {
    if root.is_windows() {
        neoism_protocol::host_path::HostPath::new(path).is_windows()
    } else {
        path.starts_with('/')
    }
}

fn resolve_remote_lua_target_path(
    root: &neoism_protocol::host_path::HostPath,
    path: &str,
) -> Result<neoism_protocol::host_path::HostPath, String> {
    if invalid_host_relative_path(root, path) {
        return Err("structured LSP request path is not a safe host path".into());
    }
    if host_path_is_absolute(root, path) {
        root.relative(path).ok_or_else(|| {
            "structured LSP requests cannot leave the focused workspace root".to_string()
        })?;
        Ok(neoism_protocol::host_path::HostPath::new(path))
    } else {
        Ok(root.join(path))
    }
}

fn invalid_host_relative_path(
    root: &neoism_protocol::host_path::HostPath,
    path: &str,
) -> bool {
    if path.is_empty() || path.contains('\0') {
        return true;
    }
    let has_parent = if root.is_windows() {
        path.split(['/', '\\']).any(|part| part == "..")
    } else {
        path.split('/').any(|part| part == "..")
    };
    has_parent
}

fn enqueue_remote_lua_error(pending: &Pending, code: &str, message: &str) {
    let (owner, id, operation, target) = match &pending.kind {
        Kind::LuaRead {
            owner,
            id,
            operation,
            target,
        }
        | Kind::LuaEditPrepare {
            owner,
            id,
            operation,
            target,
        }
        | Kind::LuaEditCommit {
            owner,
            id,
            operation,
            target,
            ..
        }
        | Kind::LuaEditFinalize {
            owner,
            id,
            operation,
            target,
            ..
        } => (owner, id, operation, target),
        _ => return,
    };
    super::lua::enqueue_lua_lsp_completion(
        owner.clone(),
        neoism_lua::LuaLspCompletion::failed(
            id.clone(),
            *operation,
            Some(target.clone()),
            code,
            message,
        ),
        pending.document.window,
    );
}

fn subscription(document: Document, revision: u64) -> Subscription {
    Subscription {
        transport: None,
        surface: format!(
            "native-lsp:{}:{}",
            document.route,
            QUERY_SEQ.fetch_add(1, Ordering::SeqCst)
        ),
        document,
        revision,
        synced: None,
        attempts: 0,
        due: Instant::now(),
        pill: None,
        reported_errors: Default::default(),
    }
}
fn snapshot_pill(servers: &[neoism_protocol::editor::LspSnapshotServer]) -> Pill {
    use neoism_ui::panels::{
        lsp_popup::LspServerState as Row, status_line::LspStatus as Status,
    };
    let rows = servers
        .iter()
        .map(|s| neoism_ui::panels::lsp_popup::LspServerRow {
            name: s.name.clone(),
            binary: (!s.binary.is_empty()).then(|| s.binary.clone()),
            filetype: Some(s.filetype.clone()),
            state: match s.state.as_str() {
                "connected" | "active" => Row::Active,
                "available" | "ready" => Row::Ready,
                "initializing" | "starting" => Row::Initializing,
                "error" | "errored" => Row::Errored,
                "disabled" => Row::Disabled,
                _ => Row::Missing,
            },
            message: s.message.clone(),
            level: s.level.clone(),
            diagnostics: Default::default(),
            source: s.source.clone(),
            runtime_source: s.runtime_source.clone(),
            runtime_path: s.runtime_path.clone(),
            runtime_version: s.runtime_version.clone(),
        })
        .collect();
    let active = servers
        .iter()
        .filter(|s| matches!(s.state.as_str(), "connected" | "active"))
        .collect::<Vec<_>>();
    let (status, label) = if let Some(first) = active.first() {
        (
            Status::Active,
            if active.len() > 1 {
                format!("{}+{}", first.name, active.len() - 1)
            } else {
                first.name.clone()
            },
        )
    } else if servers.iter().any(|s| {
        matches!(
            s.state.as_str(),
            "initializing" | "starting" | "available" | "ready"
        )
    }) {
        (
            Status::Initializing,
            servers.first().map(|s| s.name.clone()).unwrap_or_default(),
        )
    } else {
        (
            Status::Missing,
            if servers.is_empty() {
                String::new()
            } else {
                "LSP error".into()
            },
        )
    };
    (status, label, rows)
}
fn error_pill(message: &str) -> Pill {
    snapshot_pill(&[neoism_protocol::editor::LspSnapshotServer {
        name: "LSP error".into(),
        binary: String::new(),
        filetype: String::new(),
        state: "error".into(),
        source: None,
        message: Some(message.into()),
        level: Some("error".into()),
        runtime_source: None,
        runtime_path: None,
        runtime_version: None,
    }])
}
fn raw_edits(
    edits: &[neoism_protocol::editor::EditorLspTextEdit],
) -> Vec<serde_json::Value> {
    edits.iter().map(|e| serde_json::json!({ "range": { "start": {"line":e.start_line,"character":e.start_col}, "end":{"line":e.end_line,"character":e.end_col}}, "newText":e.new_text })).collect()
}
fn parse_reply(pending: &Pending, reply: &Reply) -> Option<CodeLspResults> {
    if let Some(action) = pending.action {
        let matches = match reply {
            Reply::LspQueryResult { action: got, .. } => *got == action,
            Reply::LspCompletions { .. } => action == Action::Completion,
            Reply::LspHoverResult { .. } => {
                matches!(action, Action::Hover | Action::SignatureHelp)
            }
            _ => false,
        };
        if !matches {
            return None;
        }
    }
    let mut out = CodeLspResults::default();
    let file = &pending.document.file;
    match reply {
        Reply::LspCompletions { seq, items, .. } if *seq == pending.seq => {
            out.completion = Some((
                *seq,
                file.clone(),
                items
                    .iter()
                    .map(|i| engine::LspCompletionItem {
                        server_id: i.server_id.clone(),
                        label: i.label.clone(),
                        kind: i.kind.clone(),
                        detail: i.detail.clone(),
                        documentation: i.documentation.clone(),
                        insert_text: i.insert_text.clone(),
                        filter_text: i.filter_text.clone(),
                        sort_text: i.sort_text.clone(),
                        preselect: i.preselect,
                        payload: i.payload.clone().unwrap_or_default(),
                    })
                    .collect(),
            ));
        }
        Reply::LspHoverResult { seq, contents, .. } if *seq == pending.seq => {
            out.hover = Some((
                *seq,
                file.clone(),
                vec![engine::LspHover {
                    path: file.to_string_lossy().into_owned(),
                    contents: contents.clone(),
                    kind: None,
                    range: None,
                    language: None,
                }],
            ));
        }
        Reply::LspQueryResult {
            seq,
            action,
            locations,
            references,
            code_actions,
            symbols,
            highlights,
            edits,
            ..
        } if *seq == pending.seq => match action {
            Action::Definition => {
                out.definition = Some((
                    *seq,
                    locations
                        .iter()
                        .map(|l| {
                            Some(engine::LspLocation {
                                path: l
                                    .resolve_host_path(
                                        &neoism_protocol::host_path::HostPath::new(
                                            pending.document.root.to_string_lossy(),
                                        ),
                                    )?
                                    .as_str()
                                    .to_owned(),
                                range: Some(engine::LspRange {
                                    start: engine::LspPosition {
                                        line: l.line,
                                        character: l.character,
                                    },
                                    end: engine::LspPosition {
                                        line: l.line,
                                        character: l.character,
                                    },
                                }),
                                language: None,
                            })
                        })
                        .collect::<Option<Vec<_>>>()?,
                ))
            }
            Action::References => {
                out.references = Some((
                    *seq,
                    pending.document.root.clone(),
                    references
                        .iter()
                        .map(|r| neoism_ui::panels::finder::ReferenceRow {
                            path: r.path.clone(),
                            line: r.line,
                            column: r.column,
                            text: r.text.clone(),
                        })
                        .collect(),
                ))
            }
            Action::DocumentSymbols => {
                out.document_symbols = Some((
                    *seq,
                    symbols
                        .iter()
                        .map(|s| neoism_ui::panels::finder::SymbolRow {
                            name: s.name.clone(),
                            kind: s.kind.clone(),
                            line: s.line + 1,
                            column: s.character,
                        })
                        .collect(),
                ))
            }
            Action::DocumentHighlight => {
                out.occurrences = Some((
                    *seq,
                    file.clone(),
                    highlights
                        .iter()
                        .map(|&(l, s, e)| (l as usize, s as usize, e as usize))
                        .collect(),
                ))
            }
            Action::CodeActions if matches!(pending.kind, Kind::Query) => {
                out.code_actions = Some((
                    *seq,
                    file.clone(),
                    code_actions
                        .iter()
                        .filter(|a| a.disabled_reason.is_none())
                        .map(|a| CodeActionItem {
                            server_id: a.server_id.clone(),
                            title: a.title.clone(),
                            kind: a.kind.clone().unwrap_or_default(),
                            action: {
                                let mut raw = a.payload.clone();
                                if let Some(object) = raw.as_object_mut() {
                                    object.insert(
                                        "_neoismRevision".into(),
                                        a.document_revision.clone().into(),
                                    );
                                }
                                raw
                            },
                        })
                        .collect(),
                ))
            }
            Action::Format => {
                if let Kind::Format(revision) = pending.kind {
                    out.format_save = Some((
                        file.clone(),
                        revision,
                        edits
                            .iter()
                            .filter(|e| host_path_eq(&e.path, file))
                            .flat_map(|e| raw_edits(&e.edits))
                            .collect(),
                    ));
                }
            }
            _ => return None,
        },
        _ => return None,
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn document() -> Document {
        Document {
            window: unsafe { WindowId::dummy() },
            endpoint: "ws://127.0.0.1:9877/session".into(),
            workspace: "shared-workspace".into(),
            root: "/host/workspace".into(),
            route: 7,
            file: "/host/workspace/main.rs".into(),
        }
    }
    fn pending() -> Pending {
        Pending {
            document: document(),
            surface: "route".into(),
            revision: 4,
            seq: 9,
            kind: Kind::Query,
            action: Some(Action::Hover),
            open_revisions: Vec::new(),
            since: Instant::now(),
        }
    }

    #[test]
    fn remote_lua_cancellation_is_exact_owner_and_request_scoped() {
        let owner = neoism_lua::PluginOwner {
            plugin_id: "dev.neoism.remote-cancel-test".into(),
            revision: neoism_lua::PluginRevision("one".into()),
        };
        let mut item = pending();
        item.kind = Kind::LuaEditPrepare {
            owner: owner.clone(),
            id: "lua-cancel-test".into(),
            operation: neoism_lua::LuaLspOperation::Rename,
            target: neoism_lua::LuaLspTarget {
                root: "/host/workspace".into(),
                path: "/host/workspace/main.rs".into(),
                line: 0,
                character: 0,
                buffer_revision: Some(4),
                pane_id: 7,
            },
        };
        let key = (
            item.document.window,
            "remote-cancel-test-endpoint".to_string(),
            u64::MAX - 17,
        );
        lock().pending.insert(key.clone(), item);
        let other_owner = neoism_lua::PluginOwner {
            plugin_id: owner.plugin_id.clone(),
            revision: neoism_lua::PluginRevision("two".into()),
        };
        assert!(!cancel_remote_lua_lsp_request(
            &other_owner,
            "lua-cancel-test"
        ));
        assert!(lock().pending.contains_key(&key));
        assert!(cancel_remote_lua_lsp_request(
            &owner,
            "lua-cancel-test"
        ));
        assert!(!lock().pending.contains_key(&key));
    }
    #[test]
    fn shared_lsp_local_vault_source_never_uses_joined_host_root() {
        let local_path = std::env::temp_dir().join("vault").join("note.rs");
        let mut code =
            neoism_ui::editor::code::CodePane::new(local_path.clone(), "fn note() {}");
        code.local_only = true;
        let host_root = PathBuf::from("/host/shared/project");
        assert!(!code_uses_host_lsp(&code));
        assert_eq!(
            code_lsp_source_root(&code, true, Some(host_root.clone())).unwrap(),
            local_path.parent().unwrap()
        );
        code.remote_source = true; // explicit local source wins even over a stale grid flag
        assert!(!code_uses_host_lsp(&code));
        code.local_only = false;
        assert!(code_uses_host_lsp(&code));
        assert!(host_path_eq(
            &code_lsp_source_root(&code, true, Some(host_root.clone())).unwrap(),
            &host_root
        ));
    }

    #[test]
    fn shared_lsp_raw_host_paths_are_neither_guest_components_nor_crdt_ids() {
        use neoism_protocol::host_path::HostPath;
        let host = HostPath::new("/host/项目 space").join(r"src\literal/file.rs");
        let mut doc = document();
        doc.file = PathBuf::from(host.as_str());
        let mut alias = doc.clone();
        alias.file = PathBuf::from(host.as_str().replace('\\', "/"));
        assert!(!host_path_eq(&doc.file, &alias.file));
        assert_ne!(doc, alias);
        let mut cache = HashMap::new();
        cache.insert(doc.clone(), 1);
        assert!(!cache.contains_key(&alias));
        assert_eq!(doc.file.to_str().unwrap(), host.as_str());
        assert_ne!(doc.file.to_str().unwrap(), host.buffer_id());
        let mut code =
            neoism_ui::editor::code::CodePane::new(doc.file.clone(), "fn host() {}");
        code.remote_source = true;
        assert!(code_lsp_file_matches(&code, &doc.file));
        assert!(!code_lsp_file_matches(&code, &alias.file));
    }

    #[test]
    fn shared_lsp_definition_uri_and_authoritative_path_land_as_raw_host_paths() {
        let mut p = pending();
        p.action = Some(Action::Definition);
        p.document.root = "/home/host/work".into();
        p.document.file = "/home/host/work/current.rs".into();
        for location in [
            serde_json::json!({"uri":"file:///home/host/work/space%20%E9%A1%B9%E7%9B%AE/%2520.rs","line":4,"character":2}),
            serde_json::json!({"uri":"file:///wrong","host_path":"/home/host/work/space 项目/%20.rs","line":4,"character":2}),
        ] {
            let reply: Reply =
                serde_json::from_value(serde_json::json!({"LspQueryResult":{
                    "seq":9,"action":Action::Definition,"locations":[location]
                }}))
                .unwrap();
            let (_, locations) = parse_reply(&p, &reply).unwrap().definition.unwrap();
            assert_eq!(locations[0].path, "/home/host/work/space 项目/%20.rs");
            assert_eq!(locations[0].range.as_ref().unwrap().start.character, 2);
        }
    }

    #[test]
    fn shared_lsp_endpoint_is_owner_even_when_localhost() {
        let doc = document();
        assert!(owner_matches_link(&doc, Some(&doc.endpoint)));
        assert!(!owner_matches_link(
            &doc,
            Some("unix:///tmp/bootstrap.sock")
        ));
        assert!(!owner_matches_link(&doc, Some("ws://127.0.0.1:4096")));
        assert!(!owner_matches_link(&doc, None));
    }
    #[test]
    fn shared_lsp_stale_reply_cannot_paint_same_path_other_host_workspace_or_pane() {
        let p = pending();
        assert!(interactive_matches(&p, Some(&p.document), Some(4)));
        assert!(!interactive_matches(&p, Some(&p.document), Some(5)));
        assert!(!interactive_matches(&p, None, Some(4)));
        for field in 0..5 {
            let mut other = p.document.clone();
            match field {
                0 => other.endpoint = "ws://other/session".into(),
                1 => other.workspace = "other".into(),
                2 => other.root = "/other".into(),
                3 => other.route += 1,
                _ => other.file = "/other/main.rs".into(),
            }
            assert!(!interactive_matches(&p, Some(&other), Some(4)));
        }
    }
    #[test]
    fn passive_lsp_failures_are_reported_once_per_document() {
        let mut sub = subscription(document(), 4);
        assert!(passive_lsp_request(&Kind::Sync, None));
        for action in [
            Action::Hover,
            Action::Completion,
            Action::SignatureHelp,
            Action::DocumentHighlight,
        ] {
            assert!(passive_lsp_request(&Kind::Query, Some(action)));
        }
        assert!(!passive_lsp_request(&Kind::Query, Some(Action::Definition)));
        assert!(!passive_lsp_request(&Kind::Format(4), Some(Action::Format)));
        assert!(report_passive_error(&mut sub, "taplo unavailable"));
        assert!(!report_passive_error(&mut sub, "taplo unavailable"));
        assert!(report_passive_error(&mut sub, "connection lost"));
        let mut reopened = subscription(document(), 4);
        assert!(report_passive_error(&mut reopened, "taplo unavailable"));
    }
    #[test]
    fn shared_lsp_snapshot_retry_is_bounded_and_not_marked_synced_before_ack() {
        let mut sub = subscription(document(), 4);
        let now = Instant::now();
        assert!(sub.begin_sync(4, false, false, now));
        assert_eq!(sub.synced, None);
        assert!(!sub.begin_sync(4, true, false, now + Duration::from_secs(5)));
        assert!(sub.begin_sync(4, false, false, now + Duration::from_secs(5)));
        assert!(sub.begin_sync(4, false, false, now + Duration::from_secs(10)));
        assert!(!sub.begin_sync(4, false, false, now + Duration::from_secs(30)));
        assert!(sub.begin_sync(5, false, false, now + Duration::from_secs(30)));
        sub.attempts = 3;
        assert!(sub.begin_sync(5, false, true, now + Duration::from_secs(30)));
    }
    #[test]
    fn shared_lsp_status_error_never_initializing_and_caches_are_document_scoped() {
        use neoism_ui::panels::{lsp_popup::LspServerState, status_line::LspStatus};
        let pill = error_pill("initialize failed");
        assert_eq!(pill.0, LspStatus::Missing);
        assert_eq!(pill.2[0].state, LspServerState::Errored);
        let a = document();
        let mut b = a.clone();
        b.endpoint = "ws://other/session".into();
        let mut cache = HashMap::new();
        cache.insert(a.clone(), pill);
        assert!(cache.contains_key(&a));
        assert!(!cache.contains_key(&b));
    }
    #[test]
    fn shared_lsp_typed_parser_checks_sequence_and_operation() {
        let p = pending();
        let reply = Reply::LspHoverResult {
            surface_id: Some("route".into()),
            seq: 9,
            contents: "host hover".into(),
            line: 0,
            character: 0,
        };
        let parsed = parse_reply(&p, &reply).unwrap();
        assert_eq!(parsed.hover.unwrap().2[0].contents, "host hover");
        let mut stale = p.clone();
        stale.seq += 1;
        assert!(parse_reply(&stale, &reply).is_none());
        stale.seq = 9;
        stale.action = Some(Action::Definition);
        assert!(parse_reply(&stale, &reply).is_none());
    }

    #[test]
    fn remote_lua_targets_keep_host_path_spelling_and_reject_escapes() {
        for (root, requested, expected) in [
            (r"C:\Work\项目", r"src\main.rs", r"C:\Work\项目\src\main.rs"),
            (
                r"\\Server\Share\项目",
                r"lib\dep.rs",
                r"\\Server\Share\项目\lib\dep.rs",
            ),
            ("/srv/项目", "src/main.rs", "/srv/项目/src/main.rs"),
        ] {
            let root = neoism_protocol::host_path::HostPath::new(root);
            assert_eq!(
                resolve_remote_lua_target_path(&root, requested)
                    .unwrap()
                    .as_str(),
                expected
            );
        }

        for (root, requested) in [
            (r"C:\Work", r"..\secret.rs"),
            (r"\\Server\Share", r"..\secret.rs"),
            ("/srv/work", "../secret.rs"),
            ("/srv/work", "/srv/other/secret.rs"),
        ] {
            let root = neoism_protocol::host_path::HostPath::new(root);
            assert!(resolve_remote_lua_target_path(&root, requested).is_err());
        }
    }

    #[test]
    fn remote_lua_outcome_conversion_preserves_recursive_data() {
        use neoism_protocol::editor::{
            EditorLspReadDiagnostic, EditorLspReadOutcome, EditorLspReadPosition,
            EditorLspReadRange, EditorLspReadRelatedInformation,
        };
        let outcome = EditorLspReadOutcome::Diagnostics(vec![EditorLspReadDiagnostic {
            path: r"\\Server\Share\项目\main.rs".into(),
            range: Some(EditorLspReadRange {
                start: EditorLspReadPosition {
                    line: 2,
                    character: 4,
                },
                end: EditorLspReadPosition {
                    line: 2,
                    character: 9,
                },
            }),
            severity: "error".into(),
            code: Some("E1".into()),
            code_description: Some("https://example.test/E1".into()),
            source: Some("fixture".into()),
            message: "broken".into(),
            tags: vec!["deprecated".into()],
            related_information: vec![EditorLspReadRelatedInformation {
                path: r"C:\Work\dep.rs".into(),
                range: None,
                message: "related".into(),
            }],
            data: Some(serde_json::json!({"fix": {"id": 7}})),
            language: Some("rust".into()),
        }]);

        let neoism_lua::LuaLspOutcome::Diagnostics(items) =
            lua_read_outcome(outcome).unwrap()
        else {
            panic!("diagnostic outcome changed kind");
        };
        assert_eq!(items[0].path, r"\\Server\Share\项目\main.rs");
        assert_eq!(items[0].range.as_ref().unwrap().start.line, 2);
        assert_eq!(items[0].related_information[0].path, r"C:\Work\dep.rs");
        assert_eq!(items[0].data, Some(serde_json::json!({"fix": {"id": 7}})));
    }
}
