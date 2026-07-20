use super::*;

#[derive(Debug, Deserialize)]
pub(crate) enum ServiceClientMessage {
    Files {
        request_id: u64,
        #[serde(default)]
        workspace_root: Option<String>,
        message: FilesClientMessage,
    },
    Git {
        request_id: u64,
        /// Absolute repo-root override — a guest asking about a JOINED
        /// workspace names that workspace's root; empty/absent falls
        /// back to the daemon's default files root. Same resolution
        /// rules as the files plane.
        #[serde(default)]
        workspace_root: Option<String>,
        message: GitClientMessage,
    },
    Editor {
        request_id: u64,
        #[serde(default)]
        workspace_root: Option<String>,
        message: EditorClientMessage,
    },
    /// Inbound agent (Claude API proxy) envelope. `request_id` is
    /// echoed on every emitted `AgentReply` so the bridge can route
    /// streaming events through its existing pending-correlation
    /// table.
    Agent {
        request_id: u64,
        message: AgentClientMessage,
    },
    // ---------------- wave-7 web parity additions ----------------
    // The variants below are added by W2-D for the search /
    // workspace / diagnostics subsystems. Each carries its own
    // request_id so the chrome's reply correlation table stays
    // homogeneous across families.
    Search {
        request_id: u64,
        message: SearchClientMessage,
    },
    Workspace {
        #[serde(default)]
        request_id: u64,
        message: WorkspaceClientMessage,
    },
    Diagnostics {
        #[serde(default)]
        request_id: u64,
        message: DiagnosticsClientMessage,
    },
    CursorOverlay {
        #[serde(default)]
        request_id: u64,
        message: CursorOverlayClientMessage,
    },
    Crdt {
        #[serde(default)]
        request_id: u64,
        message: CrdtClientMessage,
    },
}

#[derive(Debug, Serialize)]
pub(crate) enum ServiceServerMessage {
    FilesReply {
        request_id: u64,
        message: FilesServerMessage,
    },
    GitReply {
        request_id: u64,
        message: GitServerMessage,
    },
    EditorReply {
        request_id: u64,
        message: EditorServerMessage,
    },
    /// Outbound agent stream event. `request_id` matches whichever
    /// envelope the chrome most recently submitted — unsolicited
    /// pushes (`Disabled` on connect, deltas after a `SendMessage`)
    /// reuse the same `request_id` so the JS routing stays
    /// reply-shaped.
    AgentReply {
        request_id: u64,
        message: AgentServerMessage,
    },
    // ---------------- wave-7 web parity additions ----------------
    SearchReply {
        request_id: u64,
        message: SearchServerMessage,
    },
    WorkspaceReply {
        request_id: u64,
        message: WorkspaceServerMessage,
    },
    DiagnosticsReply {
        request_id: u64,
        message: DiagnosticsServerMessage,
    },
    /// Unsolicited cursor-overlay push. `request_id` mirrors the
    /// editor session's latest request id so the JS routing stays
    /// reply-shaped — the chrome treats these as fire-and-forget
    /// state mutations rather than per-call responses.
    CursorOverlayReply {
        request_id: u64,
        message: CursorOverlayServerMessage,
    },
    CrdtReply {
        request_id: u64,
        message: CrdtServerMessage,
    },
}

// ---------------------------------------------------------------------
// Wave 7A — ephemeral presence plane (per-socket bookkeeping)
// ---------------------------------------------------------------------

/// Default time-to-live for a presence entry that stops heartbeating.
/// Clients re-publish unchanged cursors at roughly TTL/2, so a healthy
/// connection never expires; a wedged or vanished one does.
pub(crate) const PRESENCE_TTL_MS_DEFAULT: u64 = 10_000;

/// Wall-clock milliseconds since the unix epoch. Presence timestamps
/// are daemon-stamped on receipt (client clocks are advisory only) so
/// the TTL sweep compares values from a single clock.
pub(crate) fn presence_now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or(0)
}

/// Presence TTL, overridable via `NEOISM_PRESENCE_TTL_MS` (integration
/// tests shrink it so expiry is observable without a 10s wait).
pub(crate) fn presence_ttl_ms() -> u64 {
    std::env::var("NEOISM_PRESENCE_TTL_MS")
        .ok()
        .and_then(|raw| raw.parse::<u64>().ok())
        .filter(|ttl| *ttl > 0)
        .unwrap_or(PRESENCE_TTL_MS_DEFAULT)
}

/// Sweep cadence derived from the TTL: half the TTL, clamped to
/// [50ms, 2s] so a tiny test TTL still expires promptly and the
/// default TTL doesn't wake the socket task more than every 2s.
pub(crate) fn presence_sweep_interval(ttl_ms: u64) -> std::time::Duration {
    std::time::Duration::from_millis((ttl_ms / 2).clamp(50, 2_000))
}

/// Per-connection presence bookkeeping.
///
/// Tracks which presence peer ids this socket published so that
/// (a) the broadcast pump can drop the sender's own presence frames —
/// never echo presence back at its publisher (this codebase previously
/// shipped a publish→echo oscillation bug in the workspace plane) —
/// and (b) `Drop` removes those peers from the hub, broadcasting the
/// removal to the other clients. `Drop` runs on every exit path of
/// `handle_socket`, including the early `return`s on send errors.
pub(crate) struct SocketPresenceGuard {
    crdt: CrdtSyncHub,
    peer_ids: std::collections::HashSet<String>,
}

impl SocketPresenceGuard {
    fn new(crdt: CrdtSyncHub) -> Self {
        Self {
            crdt,
            peer_ids: std::collections::HashSet::new(),
        }
    }

    fn register(&mut self, peer_id: &str) {
        if !self.peer_ids.contains(peer_id) {
            self.peer_ids.insert(peer_id.to_string());
        }
    }

    /// True when a hub broadcast is about a peer id THIS socket
    /// publishes — those frames must not be written back to it.
    fn is_own_presence_broadcast(&self, message: &CrdtServerMessage) -> bool {
        match message {
            CrdtServerMessage::Presence {
                update: CrdtPresenceUpdate::Upsert(presence),
            } => self.peer_ids.contains(&presence.peer_id),
            CrdtServerMessage::Presence {
                update: CrdtPresenceUpdate::Remove { peer_id, .. },
            } => self.peer_ids.contains(peer_id),
            _ => false,
        }
    }
}

impl Drop for SocketPresenceGuard {
    fn drop(&mut self) {
        for peer_id in &self.peer_ids {
            let _ = self.crdt.remove_peer_presence_everywhere(peer_id);
        }
    }
}

pub(crate) struct SocketNvimForwarders {
    redraw: HashMap<String, tokio::task::JoinHandle<()>>,
    cursor_overlay: HashMap<String, tokio::task::JoinHandle<()>>,
}

impl SocketNvimForwarders {
    fn new() -> Self {
        Self {
            redraw: HashMap::new(),
            cursor_overlay: HashMap::new(),
        }
    }

    fn ensure(
        &mut self,
        key: &str,
        handle: &NvimSessionHandle,
        redraw_tx: &tokio::sync::mpsc::UnboundedSender<(String, EditorServerMessage)>,
        cursor_overlay_tx: &tokio::sync::mpsc::UnboundedSender<(
            String,
            CursorOverlayServerMessage,
        )>,
    ) {
        if !self.redraw.contains_key(key) {
            let mut rx = handle.subscribe_redraw();
            let tx = redraw_tx.clone();
            let task_key = key.to_string();
            let send_key = task_key.clone();
            self.redraw.insert(
                task_key,
                tokio::spawn(async move {
                    while let Some(message) = recv_broadcast(&mut rx, "nvim redraw").await
                    {
                        if tx.send((send_key.clone(), message)).is_err() {
                            break;
                        }
                    }
                }),
            );
        }

        if !self.cursor_overlay.contains_key(key) {
            let mut rx = handle.subscribe_cursor_overlay();
            let tx = cursor_overlay_tx.clone();
            let task_key = key.to_string();
            let send_key = task_key.clone();
            self.cursor_overlay.insert(
                task_key,
                tokio::spawn(async move {
                    while let Some(message) =
                        recv_broadcast(&mut rx, "nvim cursor overlay").await
                    {
                        if tx.send((send_key.clone(), message)).is_err() {
                            break;
                        }
                    }
                }),
            );
        }
    }

    fn abort_key(&mut self, key: &str) {
        if let Some(task) = self.redraw.remove(key) {
            task.abort();
        }
        if let Some(task) = self.cursor_overlay.remove(key) {
            task.abort();
        }
    }
}

impl Drop for SocketNvimForwarders {
    fn drop(&mut self) {
        for task in self.redraw.values() {
            task.abort();
        }
        for task in self.cursor_overlay.values() {
            task.abort();
        }
    }
}

/// Auto-completion is intentionally a short-lived, latest-value operation.
/// Keep one pending task per editor surface so a fast `d` → `de` → `det`
/// burst does not enqueue three full buffer reads and three serialized LSP
/// round-trips. Aborting during the short debounce happens before the blocking
/// language-server call starts; once a server request is on the wire it may
/// finish, but all intermediate requests that have not started are coalesced.
struct SocketCompletionTasks {
    pending: HashMap<String, tokio::task::JoinHandle<()>>,
}

impl SocketCompletionTasks {
    fn new() -> Self {
        Self {
            pending: HashMap::new(),
        }
    }

    fn replace(&mut self, key: String, task: tokio::task::JoinHandle<()>) {
        if let Some(superseded) = self.pending.insert(key, task) {
            superseded.abort();
        }
    }

    fn abort_key(&mut self, key: &str) {
        if let Some(task) = self.pending.remove(key) {
            task.abort();
        }
    }

    fn abort_all(&mut self) {
        for (_, task) in self.pending.drain() {
            task.abort();
        }
    }
}

impl Drop for SocketCompletionTasks {
    fn drop(&mut self) {
        self.abort_all();
    }
}

const LSP_COMPLETION_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(24);

pub(crate) fn socket_scoped_nvim_key(
    socket_namespace: uuid::Uuid,
    message: &EditorClientMessage,
) -> String {
    format!(
        "{socket_namespace}:{}",
        NvimSessionRegistry::key_for_message(message)
    )
}
pub(crate) async fn handle_socket(
    socket: WebSocket,
    registry: SessionRegistry,
    mut output_rx: tokio::sync::broadcast::Receiver<ServerMessage>,
    device: Option<crate::auth::DeviceRecord>,
    upgrade_auth_reason: Option<&'static str>,
    peer_ip: Option<String>,
    workspace_manager: WorkspaceManager,
    pairing_tokens: PairingTokenStore,
    nvim_sessions: NvimSessionRegistry,
    crdt: CrdtSyncHub,
) {
    let (mut sink, mut stream) = socket.split();
    let nvim_socket_namespace = uuid::Uuid::new_v4();
    tracing::debug!(
        nvim_socket_namespace = %nvim_socket_namespace,
        "websocket connection established"
    );

    /// Reaps this connection's nvim sessions on EVERY exit path of the
    /// websocket task (there are a dozen `return`s). Without it, each
    /// reconnect left orphan sessions whose CRDT appliers kept
    /// replaying every edit into headless nvims nobody rendered.
    struct NvimNamespaceGuard {
        registry: NvimSessionRegistry,
        namespace: uuid::Uuid,
    }
    impl Drop for NvimNamespaceGuard {
        fn drop(&mut self) {
            let registry = self.registry.clone();
            let prefix = format!("{}:", self.namespace);
            tokio::spawn(async move {
                let reaped = registry.remove_prefix(&prefix).await;
                if reaped > 0 {
                    tracing::info!(
                        prefix = %prefix,
                        reaped,
                        "reaped nvim sessions for closed connection"
                    );
                }
            });
        }
    }
    let _nvim_namespace_guard = NvimNamespaceGuard {
        registry: nvim_sessions.clone(),
        namespace: nvim_socket_namespace,
    };

    // Unsolicited status snapshot: the chrome status line wants a real
    // branch on first paint without having to query. We send a `GitReply`
    // tagged with the reserved `request_id = 0` (clients never allocate
    // 0; their counter starts at 1) so the JS side knows it's a push.
    let initial_branch = git_handler::current_branch_snapshot().await;
    let snapshot = ServiceServerMessage::GitReply {
        request_id: 0,
        message: initial_branch.clone(),
    };
    if let Err(err) = send_json(&mut sink, &snapshot).await {
        tracing::warn!(error = %err, "websocket send error on branch snapshot");
        return;
    }

    for message in registry.backlog_messages() {
        if let Err(err) = send_json(&mut sink, &message).await {
            tracing::warn!(error = %err, "websocket send error on pty backlog");
            return;
        }
    }

    // Poll the workspace every 2 seconds and re-push any field whose
    // value has changed since the last tick. We send through a local
    // mpsc so the polling task doesn't have to share the sink with the
    // main client-frame loop below.
    //
    // TODO(wave-cutover-pending): lsp + diagnostics need an editor-side
    // Neoism-owned LSP diagnostics/status source. Neovim still supplies
    // active buffer text during migration, but LSP server ownership,
    // diagnostics, and lifecycle state come from the Rust runtime.
    let (push_tx, mut push_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServiceServerMessage>();
    let poll_task = tokio::spawn(status_poll_loop(
        push_tx,
        initial_branch_name(&initial_branch),
    ));

    // Daemon-owned embedded nvim sessions. The websocket may touch
    // multiple editor surface ids during its lifetime, so each acquired
    // session gets a small forwarder task that keeps its redraw stream
    // connected to this socket even after focus moves to another surface.
    let mut nvim_session: Option<NvimSessionHandle> = None;
    let (nvim_redraw_tx, mut nvim_redraw_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, EditorServerMessage)>();
    // Real-time `publishDiagnostics` bus (event-driven — no polling). The
    // engine pushes here the instant a language server publishes; we forward
    // to the editor from the select loop below. `active_editor_file` gates it
    // so a workspace-wide push for a non-focused buffer isn't shown.
    let mut lsp_diagnostics_rx = crate::language_server::subscribe_diagnostics();
    let mut active_editor_file: Option<String> = None;
    let (nvim_cursor_overlay_tx, mut nvim_cursor_overlay_rx) =
        tokio::sync::mpsc::unbounded_channel::<(String, CursorOverlayServerMessage)>();
    let mut nvim_forwarders = SocketNvimForwarders::new();
    let mut completion_tasks = SocketCompletionTasks::new();
    let mut nvim_request_ids: HashMap<String, u64> = HashMap::new();
    let mut nvim_request_id: u64 = 0;

    // Per-socket Claude API proxy. Spawned eagerly so the chrome
    // sees an immediate `Disabled` event when `NEOISM_AGENT_API_KEY`
    // is unset; the proxy task self-parks until the first
    // `SendMessage` arrives. We tag every emitted event with the
    // latest `request_id` the chrome submitted (or `0` for the
    // unsolicited pre-prompt `Disabled` push).
    let (agent_tx, mut agent_rx) =
        tokio::sync::mpsc::unbounded_channel::<AgentServerMessage>();
    let agent_api_key = std::env::var("NEOISM_AGENT_API_KEY").ok();
    let agent_model = std::env::var("NEOISM_AGENT_MODEL").unwrap_or_default();
    let agent_session = AgentSession::spawn(agent_api_key, agent_model, agent_tx);
    let mut agent_request_id: u64 = 0;

    // ---------------- wave-7 web parity per-socket state ----------------
    //
    // Search dispatch: each `SearchClientMessage` spawns a tokio task
    // whose reply lands on `search_rx`. The registry tracks the
    // `AbortHandle` per request so `CancelSearch` can abort an
    // in-flight `rg` / `git` subprocess.
    let (search_tx, mut search_rx) =
        tokio::sync::mpsc::unbounded_channel::<SearchServerMessage>();
    let search_registry = SearchRegistry::new();
    let mut search_request_id: u64 = 0;

    // Workspace dispatch: per-connection cwd / session pointer. The
    // cross-connection registry lives on `workspace_manager`.
    let mut connection_workspace = ConnectionWorkspace::default();

    // Diagnostics dispatch: subscription set keyed by `RouteId`.
    // Pushed events arrive on `diagnostics_rx`; the `diagnostics_tick`
    // interval below spawns a `DiagnosticsSubscriptions::fetch` on a
    // 2-second cadence so subscribers see hash-suppressed diagnostic +
    // LSP-state pushes whenever nvim surfaces a change. The
    // subscription table is owned directly (no Mutex needed) because
    // every mutation point lives inside this single-task `select!`
    // loop — subscribe/unsubscribe arms and the fetch-result `apply`
    // run in strict sequence here.
    let (diagnostics_tx, mut diagnostics_rx) =
        tokio::sync::mpsc::unbounded_channel::<DiagnosticsServerMessage>();
    let mut diagnostics_subscriptions = DiagnosticsSubscriptions::new();
    // This timer is status/cache recovery only. OpenBuffer and on_lines/CRDT
    // events feed didOpen/didChange immediately through the ordered live-sync
    // worker, so insert-mode diagnostic latency never waits for this tick.
    // The probe reads only file identity/cursor/size metadata, not buffer text.
    let mut diagnostics_tick =
        tokio::time::interval(std::time::Duration::from_millis(400));
    // Skip the spurious immediate-first-tick that `interval` emits; we
    // don't want a synchronous tick the instant a client connects with
    // zero subscriptions and no nvim session.
    diagnostics_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut diagnostics_request_id: u64 = 0;
    // Single-flight snapshot poll, spawned OFF this loop. The fetch
    // round-trips into nvim, and nvim defers non-fast RPC while the
    // user has a count/operator pending — awaiting it inline here kept
    // `stream.next()` from reading the very keys that would unblock
    // nvim (the digit-key freeze). One flight at a time also keeps
    // deferred polls from stacking up behind each other.
    let mut diagnostics_fetch: Option<
        tokio::task::JoinHandle<(DiagnosticsFetch, Vec<EditorServerMessage>)>,
    > = None;
    let mut last_language_server_messages_hash: Option<u64> = None;

    // F3: subscribe this websocket to per-workplace preferences updates
    // so a `SetWorkplacePreferences` from any client (this socket
    // included, for echo-back convergence) shows up here as a
    // `WorkplacePreferencesChanged` broadcast. The subscription is
    // process-wide; `recv()` returns `RecvError::Lagged` if the
    // channel overflows, which we degrade to "skip and keep listening"
    // — chrome can always re-request via `GetWorkplacePreferences`.
    let mut preferences_rx = workspace_manager.subscribe_preferences();
    // D2: subscribe to pane-layout broadcasts so an accepted
    // `PaneLayoutOp` submitted on any connected websocket reaches
    // every paired surface as a `PaneLayoutChanged` push. Same
    // `Lagged` handling as the preferences pump above.
    let mut pane_layout_rx = workspace_manager.subscribe_pane_layout();
    // 8D-live: subscribe to tree-changed notifications so a tab opened
    // in a shared workspace by ANY client (web publishing its strip,
    // another desktop's snapshot) reaches this one as a fresh
    // `HostWorkspaceTree` push, no polling. The origin connection is
    // skipped — it published the change.
    let mut tree_rx = workspace_manager.subscribe_tree_changes();
    let mut crdt_rx = crdt.subscribe();
    // Files-plane liveness: debounced fs-change bursts for every root
    // some client has listed. Forwarded as `FilesReply { request_id:
    // 0, Changed { root, paths } }`; clients not browsing that root
    // ignore it.
    let mut fs_watch_rx = crate::fs_watch::hub().subscribe();

    // Wave 7A presence plane: per-socket peer-id registry + TTL sweep.
    // The guard's Drop clears this connection's presence (broadcasting
    // the removals) on EVERY exit path below. The sweep tick prunes
    // peers whose connection is technically alive but stopped
    // heartbeating (~TTL); pruning is idempotent across sockets.
    let mut presence_guard = SocketPresenceGuard::new(crdt.clone());
    let presence_ttl = presence_ttl_ms();
    let mut presence_ttl_tick =
        tokio::time::interval(presence_sweep_interval(presence_ttl));
    presence_ttl_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let frame = tokio::select! {
            biased;
            out = output_rx.recv() => {
                match out {
                    Ok(out) => {
                        // A workspace's directory follows its terminal's live
                        // cwd. When the daemon reports a session moved (a
                        // `cd`), re-point its workspace's root_dir and
                        // broadcast so every client re-roots. The change-guard
                        // inside `track_pty_cwd` + the registry mutex dedupe
                        // this across the per-connection pumps, so only one
                        // broadcast fires per real move.
                        if let ServerMessage::SessionCwd { session_id, cwd } = &out {
                            if workspace_manager.track_pty_cwd(session_id, cwd.clone()) {
                                workspace_manager.broadcast_tree_changed(None);
                            }
                        }
                        if let Err(err) = send_json(&mut sink, &out).await {
                            tracing::warn!(error = %err, "websocket send error draining output");
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "pty output broadcast lagged; client should reconnect for retained backlog");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
                continue;
            }
            Some(push) = push_rx.recv() => {
                if let Err(err) = send_json(&mut sink, &push).await {
                    tracing::warn!(error = %err, "websocket send error draining status push");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            Some((key, cursor_overlay)) = nvim_cursor_overlay_rx.recv() => {
                let request_id = nvim_request_ids
                    .get(&key)
                    .copied()
                    .unwrap_or(nvim_request_id);
                let resp = ServiceServerMessage::CursorOverlayReply {
                    request_id,
                    message: cursor_overlay,
                };
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error draining cursor overlay");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            diag = lsp_diagnostics_rx.recv() => {
                // Event-driven inline diagnostics: forward the instant the
                // engine gets a `publishDiagnostics` push. Only the active
                // buffer's file is shown (a server publishes for many files).
                match diag {
                    Ok(event) => {
                        let matches_active = active_editor_file
                            .as_deref()
                            .map(|active| {
                                let file = crate::language_server::diagnostics_event_file(&event);
                                // Tolerant match: the engine's canonical path
                                // and the frontend's OpenBuffer path may differ
                                // in absolute/relative form or symlinks.
                                file == active
                                    || file.ends_with(active)
                                    || active.ends_with(file)
                            })
                            .unwrap_or(true);
                        if matches_active {
                            let message = crate::language_server::diagnostics_event_message(event);
                            let resp = ServiceServerMessage::EditorReply {
                                request_id: nvim_request_id,
                                message,
                            };
                            if let Err(err) = send_json(&mut sink, &resp).await {
                                tracing::warn!(error = %err, "websocket send error draining lsp diagnostics push");
                                poll_task.abort();
                                return;
                            }
                        }
                    }
                    // Lagged: the frontend just missed some pushes; the next
                    // one (or the poll's pill refresh) recovers. Closed: engine
                    // gone — stop listening.
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {}
                }
                continue;
            }
            Some((key, redraw)) = nvim_redraw_rx.recv() => {
                let request_id = nvim_request_ids
                    .get(&key)
                    .copied()
                    .unwrap_or(nvim_request_id);
                if let EditorServerMessage::BufferOpened { path, .. } = &redraw {
                    active_editor_file = Some(path.to_string_lossy().into_owned());
                }
                tracing::debug!(
                    request_id,
                    key = %key,
                    variant = std::any::type_name_of_val(&redraw),
                    "[nvim-trace] forwarding redraw frame over WebSocket"
                );
                let resp = ServiceServerMessage::EditorReply {
                    request_id,
                    message: redraw,
                };
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error draining nvim redraw");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            Some(agent_event) = agent_rx.recv() => {
                let resp = ServiceServerMessage::AgentReply {
                    request_id: agent_request_id,
                    message: agent_event,
                };
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error draining agent event");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            Some(search_event) = search_rx.recv() => {
                let resp = ServiceServerMessage::SearchReply {
                    request_id: search_request_id,
                    message: search_event,
                };
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error draining search reply");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            Some(diag_event) = diagnostics_rx.recv() => {
                let resp = ServiceServerMessage::DiagnosticsReply {
                    request_id: diagnostics_request_id,
                    message: diag_event,
                };
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error draining diagnostics push");
                    poll_task.abort();
                    return;
                }
                continue;
            }
            prefs = preferences_rx.recv() => {
                // F3 broadcast pump: translate the in-process
                // `PreferencesBroadcast` payload into the matching
                // `WorkplacePreferencesChanged` wire variant and write
                // it to the client. A `Lagged` error means the
                // channel overflowed; the chrome can converge by
                // resending `GetWorkplacePreferences`, so we just
                // keep listening. A `Closed` error means the daemon
                // is shutting down — bail.
                match prefs {
                    Ok(payload) => {
                        let inner =
                            WorkspaceServerMessage::WorkplacePreferencesChanged {
                                workspace_id: payload.workspace_id,
                                prefs: payload.prefs,
                            };
                        let resp = ServiceServerMessage::WorkspaceReply {
                            // No specific request_id correlates with a
                            // broadcast; reuse `0` (matching the
                            // unsolicited git push convention above).
                            request_id: 0,
                            message: inner,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining workplace prefs broadcast");
                            poll_task.abort();
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "workplace prefs broadcast lagged; client may need to re-fetch");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        // Sender dropped — daemon shutting down.
                        return;
                    }
                }
                continue;
            }
            tree = tree_rx.recv() => {
                // 8D-live broadcast pump: the tree changed (a publish
                // landed). Push a fresh `HostWorkspaceTree` to this
                // client unless IT was the publisher.
                match tree {
                    Ok(payload) => {
                        if payload.origin == Some(connection_workspace.client_id) {
                            continue;
                        }
                        let (hosts, workspaces, tabs) =
                            workspace_manager.host_workspace_tree();
                        let resp = ServiceServerMessage::WorkspaceReply {
                            request_id: 0,
                            message: WorkspaceServerMessage::HostWorkspaceTree {
                                hosts,
                                workspaces,
                                tabs,
                            },
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining tree broadcast");
                            poll_task.abort();
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "tree broadcast lagged; client refreshes on next push");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
                continue;
            }
            fs_event = fs_watch_rx.recv() => {
                // Files-plane liveness pump: forward debounced fs
                // change bursts so remote file trees update live.
                match fs_event {
                    Ok(payload) => {
                        let resp = ServiceServerMessage::FilesReply {
                            request_id: 0,
                            message: FilesServerMessage::Changed {
                                root: payload.root,
                                paths: payload.paths,
                            },
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining fs-watch broadcast");
                            poll_task.abort();
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "fs-watch broadcast lagged; client re-lists on next push");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
                continue;
            }
            pane = pane_layout_rx.recv() => {
                // D2 broadcast pump: translate the in-process
                // `PaneLayoutBroadcast` payload into the matching
                // `PaneLayoutChanged` wire variant and write it to the
                // client. Same `Lagged`/`Closed` handling as the
                // preferences pump above.
                match pane {
                    Ok(payload) => {
                        let inner = WorkspaceServerMessage::PaneLayoutChanged {
                            pane_external_id: payload.pane_external_id,
                            op: payload.op,
                            new_layout_snapshot: payload.new_layout_snapshot,
                        };
                        let resp = ServiceServerMessage::WorkspaceReply {
                            request_id: 0,
                            message: inner,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining pane-layout broadcast");
                            poll_task.abort();
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "pane-layout broadcast lagged; client may need to re-fetch");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
                continue;
            }
            crdt_event = crdt_rx.recv() => {
                match crdt_event {
                    Ok(message) => {
                        // Echo guard: never write a presence frame back
                        // to the socket that published that peer.
                        if presence_guard.is_own_presence_broadcast(&message) {
                            continue;
                        }
                        let resp = ServiceServerMessage::CrdtReply {
                            request_id: 0,
                            message,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining crdt broadcast");
                            poll_task.abort();
                            return;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(skipped = n, "crdt broadcast lagged; client should request snapshot");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        return;
                    }
                }
                continue;
            }
            _ = presence_ttl_tick.tick() => {
                // Expire presence entries that stopped heartbeating.
                // The hub broadcasts a Presence Remove for each pruned
                // peer; removal is idempotent so concurrent sweeps from
                // other sockets are harmless.
                let _ = crdt.prune_stale_presence(presence_now_ms(), presence_ttl);
                continue;
            }
            _ = diagnostics_tick.tick() => {
                // Only poll when (a) we have an active nvim session,
                // (b) at least one route is subscribed, and (c) the
                // previous poll already finished. The `snapshot_*`
                // round-trip into nvim runs detached so it can never
                // block this loop's SendKeys dispatch.
                if diagnostics_fetch.is_none() {
                    if let Some(session) = nvim_session.as_ref() {
                        // Poll whenever an editor session is live — NOT only
                        // when a (web-only) diagnostics route is subscribed.
                        // The desktop editor never sends SubscribeDiagnostics,
                        // yet it consumes the Neoism-owned pill snapshot AND the
                        // inline diagnostics this poll emits as
                        // `EditorServerMessage`s. Gating on the web
                        // subscription meant the desktop got neither (and the
                        // legacy nvim `rio_diagnostics` is now retired).
                        let session = session.clone();
                        let workspace_root = files_handler::workspace_root();
                        diagnostics_fetch = Some(tokio::spawn(async move {
                            // The Neoism language-server engine owns both surfaces: one poll yields
                            // the diagnostics fetch AND the status-bar
                            // `LspSnapshot` + inline `Diagnostics`. Fall back
                            // to the nvim diagnostics snapshot only when the
                            // engine has no file-backed buffer to read.
                            let (rust_fetch, messages) =
                                crate::language_server::poll(&session, &workspace_root).await;
                            let fetch = match rust_fetch {
                                Some(fetch) => fetch,
                                None => DiagnosticsSubscriptions::fetch(&session).await,
                            };
                            (fetch, messages)
                        }));
                    }
                }
                continue;
            }
            fetched = async { diagnostics_fetch.as_mut().expect("guarded by is_some").await }, if diagnostics_fetch.is_some() => {
                diagnostics_fetch = None;
                if let Ok((fetch, messages)) = fetched {
                    diagnostics_subscriptions.apply(fetch, &diagnostics_tx);
                    // Push the Neoism-owned LSP snapshot + inline diagnostics to
                    // the editor client so the status-bar pill/popup AND the
                    // inline error chips reflect the real engine (not the
                    // legacy nvim/Lua `rio_lsp_snapshot`/`rio_diagnostics`).
                    let current_hash = editor_messages_hash(&messages);
                    let changed = !messages.is_empty()
                        && current_hash
                            .map(|hash| Some(hash) != last_language_server_messages_hash)
                            .unwrap_or(true);
                    if changed {
                        last_language_server_messages_hash = current_hash;
                        for message in messages {
                            let resp = ServiceServerMessage::EditorReply {
                                request_id: nvim_request_id,
                                message,
                            };
                            if let Err(err) = send_json(&mut sink, &resp).await {
                                tracing::warn!(error = %err, "websocket send error draining rust lsp message");
                                poll_task.abort();
                                return;
                            }
                        }
                    }
                }
                continue;
            }
            frame = stream.next() => frame,
        };
        let Some(frame) = frame else { break };
        let frame = match frame {
            Ok(f) => f,
            Err(err) => {
                tracing::warn!(error = %err, "websocket recv error");
                break;
            }
        };

        let text = match frame {
            Message::Text(t) => t,
            Message::Binary(b) => match String::from_utf8(b) {
                Ok(t) => t,
                Err(_) => {
                    let err = ServerMessage::Error {
                        message: "binary frame is not valid utf-8 json".into(),
                    };
                    let _ = send_json(&mut sink, &err).await;
                    continue;
                }
            },
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Close(_) => {
                tracing::debug!("client requested close");
                break;
            }
        };

        // Try each known client message shape. Variant names across pty /
        // files / git are disjoint, so the first successful parse wins.
        if let Ok(msg) = serde_json::from_str::<ClientMessage>(&text) {
            // PtyCreate is the privileged op; PtyInput/Resize/Close
            // operate on an already-created session implicitly authorized
            // at create time.
            if matches!(msg, ClientMessage::CreatePty { .. }) {
                if let Err(denial) = check_permission(&device, Permission::PtyCreate) {
                    let _ = send_json(&mut sink, &denial).await;
                    continue;
                }
            }
            let responses = registry.handle(msg);
            for resp in responses {
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error");
                    return;
                }
            }
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<ServiceClientMessage>(&text) {
            match msg {
                ServiceClientMessage::Files {
                    request_id,
                    workspace_root,
                    message,
                } => {
                    let required = match &message {
                        FilesClientMessage::WriteFile { .. }
                        | FilesClientMessage::CreateFile { .. }
                        | FilesClientMessage::CreateDir { .. }
                        | FilesClientMessage::Rename { .. }
                        | FilesClientMessage::Delete { .. } => Permission::WriteFiles,
                        FilesClientMessage::ListDir { .. }
                        | FilesClientMessage::Stat { .. }
                        | FilesClientMessage::ReadFile { .. }
                        | FilesClientMessage::WalkTree { .. }
                        | FilesClientMessage::ReadShellHistory { .. } => {
                            Permission::ReadFiles
                        }
                    };
                    if let Err(denial) = check_permission(&device, required) {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    let root =
                        match resolve_request_workspace_root(workspace_root.as_deref()) {
                            Ok(root) => root,
                            Err(message) => {
                                let resp = ServiceServerMessage::FilesReply {
                                    request_id,
                                    message: FilesServerMessage::Error { message },
                                };
                                let _ = send_json(&mut sink, &resp).await;
                                continue;
                            }
                        };
                    // First files activity for a root arms the fs
                    // watcher for it — from then on every connected
                    // client gets `Changed` pushes for that root, so
                    // a guest's remote tree stays live without polls.
                    crate::fs_watch::hub().ensure_watched(&root);
                    for message in files_handler::handle_with_root(&root, message).await {
                        let resp = ServiceServerMessage::FilesReply {
                            request_id,
                            message,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error");
                            return;
                        }
                    }
                }
                ServiceClientMessage::Git {
                    request_id,
                    workspace_root,
                    message,
                } => {
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    let root =
                        match resolve_request_workspace_root(workspace_root.as_deref()) {
                            Ok(root) => root,
                            Err(message) => {
                                let resp = ServiceServerMessage::GitReply {
                                    request_id,
                                    message: GitServerMessage::Error { message },
                                };
                                let _ = send_json(&mut sink, &resp).await;
                                continue;
                            }
                        };
                    for message in git_handler::handle_with_root(root, message).await {
                        let resp = ServiceServerMessage::GitReply {
                            request_id,
                            message,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error");
                            return;
                        }
                    }
                }
                ServiceClientMessage::Editor {
                    request_id,
                    workspace_root,
                    message,
                } => {
                    let surface_id = message.surface_id().map(str::to_owned);
                    // Wave 5 item 5A: an `OpenBuffer` is the seam where the
                    // daemon learns a buffer's authoritative content. Capture
                    // the flag before `message` is consumed by `session.handle`
                    // so we can seed the CRDT hub afterward without re-parsing.
                    let is_open_buffer =
                        matches!(message, EditorClientMessage::OpenBuffer { .. });
                    tracing::info!(
                        request_id,
                        surface_id = surface_id.as_deref(),
                        variant = std::any::type_name_of_val(&message),
                        "[nvim-trace] Editor envelope received from client"
                    );
                    // Editor proxy reuses the file-read permission —
                    // a session that can read files can also open
                    // them in nvim. Write permissions are checked
                    // separately on `nvim_buf_set_lines` etc. once we
                    // expose a "save buffer" surface.
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    // Track the latest request_id so unsolicited
                    // redraw pushes route through the same channel
                    // on the JS side.
                    nvim_request_id = request_id;
                    if matches!(message, EditorClientMessage::Close) {
                        completion_tasks.abort_all();
                        if let Some(session) = nvim_session.take() {
                            let key = session.key().to_string();
                            let _ = session.handle(EditorClientMessage::Close).await;
                            nvim_sessions.remove(&key).await;
                            nvim_forwarders.abort_key(&key);
                            nvim_request_ids.remove(&key);
                        }
                        continue;
                    }

                    let root =
                        match resolve_request_workspace_root(workspace_root.as_deref()) {
                            Ok(root) => root,
                            Err(message) => {
                                let resp = ServiceServerMessage::EditorReply {
                                    request_id,
                                    message: EditorServerMessage::Error {
                                        surface_id: surface_id.clone(),
                                        message,
                                    },
                                };
                                let _ = send_json(&mut sink, &resp).await;
                                continue;
                            }
                        };

                    let desired_key =
                        socket_scoped_nvim_key(nvim_socket_namespace, &message);
                    nvim_request_ids.insert(desired_key.clone(), request_id);
                    let needs_session = nvim_session
                        .as_ref()
                        .map(|session| session.key() != desired_key)
                        .unwrap_or(true);
                    if needs_session {
                        match nvim_sessions.get_or_spawn(desired_key.clone(), &crdt).await
                        {
                            Ok(handle) => {
                                tracing::info!(
                                    request_id,
                                    key = %desired_key,
                                    "[nvim-trace] embedded nvim session acquired"
                                );
                                nvim_forwarders.ensure(
                                    &desired_key,
                                    &handle,
                                    &nvim_redraw_tx,
                                    &nvim_cursor_overlay_tx,
                                );
                                nvim_session = Some(handle);
                            }
                            Err(NvimError::NotImplemented) => {
                                tracing::error!(
                                    request_id,
                                    "[nvim-trace] nvim not installed on daemon host; \
                                     emitting EditorServerMessage::Error to client"
                                );
                                let resp = ServiceServerMessage::EditorReply {
                                    request_id,
                                    message: EditorServerMessage::Error {
                                        surface_id: surface_id.clone(),
                                        message: "nvim not installed on daemon host"
                                            .into(),
                                    },
                                };
                                let _ = send_json(&mut sink, &resp).await;
                                continue;
                            }
                            Err(err) => {
                                tracing::error!(
                                    request_id,
                                    error = %err,
                                    "[nvim-trace] nvim spawn failed; emitting Error to client"
                                );
                                let resp = ServiceServerMessage::EditorReply {
                                    request_id,
                                    message: EditorServerMessage::Error {
                                        surface_id: surface_id.clone(),
                                        message: format!("nvim spawn failed: {err}"),
                                    },
                                };
                                let _ = send_json(&mut sink, &resp).await;
                                continue;
                            }
                        }
                    }

                    let session = nvim_session.as_ref().expect("just acquired");
                    // A failed `:cd` (bad root, or nvim deferring the
                    // command behind a pending count) must NOT eat the
                    // user's message — dropping keystrokes here is how
                    // a pending count became unclearable. Log and
                    // deliver the message regardless; absolute-path
                    // opens still work without the cwd anchor.
                    if let Err(err) = session.set_workspace_root(&root).await {
                        tracing::error!(
                            request_id,
                            error = %err,
                            workspace_root = %root.display(),
                            "[nvim-trace] failed to set nvim workspace root; \
                             delivering message anyway"
                        );
                    }
                    if let EditorClientMessage::LspAction {
                        action,
                        text,
                        surface_id,
                    } = message.clone()
                    {
                        let reply = match crate::language_server::run_action(
                            session,
                            &root,
                            action,
                            text.as_deref(),
                        )
                        .await
                        {
                            Ok(mut message) => {
                                if let EditorServerMessage::LspActionResult {
                                    surface_id: target,
                                    ..
                                } = &mut message
                                {
                                    *target = surface_id.clone();
                                }
                                if matches!(action, neoism_protocol::editor::EditorLspAction::Definition | neoism_protocol::editor::EditorLspAction::Implementation) {
                                    if let EditorServerMessage::LspActionResult { locations, .. } = &message {
                                        if let Some(first) = locations.first() {
                                            let path = std::path::PathBuf::from(first.uri.clone());
                                            let _ = session
                                                .handle(EditorClientMessage::OpenBuffer {
                                                    path,
                                                    line: Some(first.line),
                                                    character: Some(first.character),
                                                    surface_id: surface_id.clone(),
                                                })
                                                .await;
                                        }
                                    }
                                }
                                message
                            }
                            Err(message) => EditorServerMessage::Error {
                                surface_id,
                                message,
                            },
                        };
                        let resp = ServiceServerMessage::EditorReply {
                            request_id,
                            message: reply,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error");
                            return;
                        }
                        continue;
                    }
                    if let EditorClientMessage::ApplyLspCodeAction {
                        action,
                        surface_id,
                    } = message.clone()
                    {
                        let reply = match crate::language_server::run_code_action(
                            session, &root, action,
                        )
                        .await
                        {
                            Ok(mut message) => {
                                if let EditorServerMessage::LspActionResult {
                                    surface_id: target,
                                    ..
                                } = &mut message
                                {
                                    *target = surface_id.clone();
                                }
                                message
                            }
                            Err(message) => EditorServerMessage::Error {
                                surface_id,
                                message,
                            },
                        };
                        let resp = ServiceServerMessage::EditorReply {
                            request_id,
                            message: reply,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error");
                            return;
                        }
                        continue;
                    }
                    if let EditorClientMessage::ApplyLspCompletion {
                        item,
                        replace_prefix,
                        surface_id,
                    } = message.clone()
                    {
                        // Resolve/application may perform a server round-trip;
                        // never hold the websocket input loop behind it. The
                        // nvim edit itself produces the normal redraw/on_lines
                        // stream. Only failures need an explicit reply.
                        let session = session.clone();
                        let root = root.clone();
                        let tx = nvim_redraw_tx.clone();
                        let key = surface_id.clone().unwrap_or_default();
                        completion_tasks.abort_key(&key);
                        let send_key = key.clone();
                        tokio::spawn(async move {
                            if let Err(message) = crate::language_server::run_completion(
                                &session,
                                &root,
                                item,
                                &replace_prefix,
                            )
                            .await
                            {
                                let _ = tx.send((
                                    send_key,
                                    EditorServerMessage::Error {
                                        surface_id,
                                        message,
                                    },
                                ));
                            }
                        });
                        continue;
                    }
                    if let EditorClientMessage::CancelLspCompletion { surface_id } =
                        message.clone()
                    {
                        let key = surface_id.unwrap_or_default();
                        completion_tasks.abort_key(&key);
                        continue;
                    }
                    if let EditorClientMessage::LspComplete {
                        seq,
                        trigger_character,
                        surface_id,
                    } = message.clone()
                    {
                        // Serve completion OFF the socket loop and coalesce a
                        // typing burst per editor surface. The small debounce
                        // is shorter than two 60 Hz frames, but prevents each
                        // intermediate character from starting an expensive
                        // buffer read + language-server request. `seq` remains
                        // the final frontend guard for a request that was
                        // already on the LSP wire when it was superseded.
                        let session = session.clone();
                        let root = root.clone();
                        let tx = nvim_redraw_tx.clone();
                        let key = surface_id.clone().unwrap_or_default();
                        let send_key = key.clone();
                        let task = tokio::spawn(async move {
                            tokio::time::sleep(LSP_COMPLETION_DEBOUNCE).await;
                            let mut reply = crate::language_server::completion(
                                &session,
                                &root,
                                seq,
                                trigger_character.as_deref(),
                            )
                            .await;
                            if let EditorServerMessage::LspCompletions {
                                surface_id: target,
                                ..
                            } = &mut reply
                            {
                                *target = surface_id;
                            }
                            let _ = tx.send((send_key, reply));
                        });
                        completion_tasks.replace(key, task);
                        continue;
                    }
                    if let EditorClientMessage::LspHoverAt {
                        seq,
                        grid,
                        row,
                        col,
                        surface_id,
                    } = message.clone()
                    {
                        // Same off-loop pattern as completion: hover at the
                        // mouse's cell must never block the socket loop, and the
                        // `seq` guard drops it if the mouse already moved on.
                        let session = session.clone();
                        let root = root.clone();
                        let tx = nvim_redraw_tx.clone();
                        let key = surface_id.clone().unwrap_or_default();
                        tokio::spawn(async move {
                            let mut reply = crate::language_server::hover_at(
                                &session, &root, seq, grid, row, col,
                            )
                            .await;
                            if let EditorServerMessage::LspHoverResult {
                                surface_id: target,
                                ..
                            } = &mut reply
                            {
                                *target = surface_id;
                            }
                            let _ = tx.send((key, reply));
                        });
                        continue;
                    }
                    // Track the focused editor file so real-time diagnostics
                    // pushes are shown only for the buffer the user is in.
                    if let EditorClientMessage::OpenBuffer { path, .. } = &message {
                        active_editor_file = Some(path.to_string_lossy().into_owned());
                    }
                    if let Err(err) = session.handle(message).await {
                        tracing::error!(
                            request_id,
                            error = %err,
                            "[nvim-trace] session.handle() failed; emitting Error to client"
                        );
                        let resp = ServiceServerMessage::EditorReply {
                            request_id,
                            message: EditorServerMessage::Error {
                                surface_id,
                                message: format!("nvim rpc failed: {err}"),
                            },
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error");
                            return;
                        }
                    } else if is_open_buffer {
                        // Wave 5 item 5A: seed/refresh the daemon-authoritative
                        // CRDT replica from nvim's freshly-opened buffer. This is
                        // strictly additive — the redraw path above already drove
                        // the client's view; here we make the buffer *shareable*
                        // so a future second client (web/peer) can subscribe over
                        // `/crdt` and receive the same authoritative document.
                        seed_crdt_from_open_buffer_in_workspace(
                            &crdt, session, &root, request_id,
                        )
                        .await;
                    }
                }
                ServiceClientMessage::Agent {
                    request_id,
                    message,
                } => {
                    // Agent envelopes piggy-back on the `ReadFiles`
                    // permission today — every paired device that can
                    // read files can also drive the agent. Once we
                    // gate on tool-use, this branches on the inner
                    // variant.
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    // Remember the most recent agent request_id so
                    // outbound stream events (which arrive via
                    // `agent_rx`) can be tagged for the chrome's
                    // pending-correlation table.
                    agent_request_id = request_id;
                    agent_handler::dispatch(&agent_session, message);
                }
                // ---------- wave-7 web parity dispatch arms ----------
                // Each new family follows the same pattern: gate on
                // `ReadFiles`, stamp the request_id for the select!
                // arm above, then hand off to the per-module handler.
                ServiceClientMessage::Search {
                    request_id,
                    message,
                } => {
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    search_request_id = request_id;
                    search_handler::dispatch(
                        &search_registry,
                        message,
                        search_tx.clone(),
                    );
                }
                ServiceClientMessage::Workspace {
                    request_id,
                    message,
                } => {
                    if !matches!(message, WorkspaceClientMessage::Hello { .. }) {
                        if let Err(denial) =
                            check_permission(&device, Permission::ReadFiles)
                        {
                            let _ = send_json(&mut sink, &denial).await;
                            continue;
                        }
                    }
                    let outcome = workspace_handler::handle_preauthenticated(
                        &workspace_manager,
                        &mut connection_workspace,
                        Some(&pairing_tokens),
                        upgrade_auth_reason,
                        peer_ip.as_deref(),
                        message,
                    );
                    for reply in outcome.replies {
                        let resp = ServiceServerMessage::WorkspaceReply {
                            request_id,
                            message: reply,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining workspace reply");
                            return;
                        }
                    }
                    if outcome.disconnect {
                        // A rejected `Hello` ack just hit the wire;
                        // drop the socket so the rejected client can
                        // observe the close (and so any subsequent
                        // frames it might have queued never run any
                        // service code).
                        tracing::info!("closing websocket after handshake rejection");
                        return;
                    }
                }
                ServiceClientMessage::Diagnostics {
                    request_id,
                    message,
                } => {
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    diagnostics_request_id = request_id;
                    match message {
                        DiagnosticsClientMessage::SubscribeDiagnostics { route_id } => {
                            diagnostics_subscriptions.subscribe(route_id);
                            // First push lands on the next
                            // `diagnostics_tick` interval (≤2s); the
                            // `tick` impl hashes the snapshot so we
                            // don't re-publish unchanged frames.
                            let _ = &diagnostics_tx;
                        }
                        DiagnosticsClientMessage::UnsubscribeDiagnostics { route_id } => {
                            diagnostics_subscriptions.unsubscribe(route_id);
                        }
                    }
                }
                ServiceClientMessage::CursorOverlay {
                    request_id,
                    message,
                } => {
                    let message = match message {
                        CursorOverlayClientMessage::CustomCursor { x, y, visible } => {
                            CursorOverlayServerMessage::CustomCursor {
                                x: Some(x),
                                y: Some(y),
                                visible,
                            }
                        }
                    };
                    let resp = ServiceServerMessage::CursorOverlayReply {
                        request_id,
                        message,
                    };
                    if let Err(err) = send_json(&mut sink, &resp).await {
                        tracing::warn!(error = %err, "websocket send error draining cursor overlay reply");
                        return;
                    }
                }
                ServiceClientMessage::Crdt {
                    request_id,
                    message,
                } => {
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    // Presence intercept: stamp the entry with the
                    // daemon's clock (client timestamps are advisory —
                    // the TTL sweep must compare a single clock) and
                    // remember the peer id so the broadcast pump can
                    // suppress echoes and Drop can clean up on
                    // disconnect.
                    let message = match message {
                        CrdtClientMessage::PublishPresence { mut presence } => {
                            presence.updated_at_ms = presence_now_ms();
                            presence_guard.register(&presence.peer_id);
                            CrdtClientMessage::PublishPresence { presence }
                        }
                        CrdtClientMessage::ClearPresence { buffer_id, peer_id } => {
                            presence_guard.register(&peer_id);
                            CrdtClientMessage::ClearPresence { buffer_id, peer_id }
                        }
                        other => other,
                    };
                    for message in crdt.handle_client_message(message) {
                        let resp = ServiceServerMessage::CrdtReply {
                            request_id,
                            message,
                        };
                        if let Err(err) = send_json(&mut sink, &resp).await {
                            tracing::warn!(error = %err, "websocket send error draining crdt reply");
                            return;
                        }
                    }
                }
            }
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<CursorOverlayClientMessage>(&text) {
            let message = match msg {
                CursorOverlayClientMessage::CustomCursor { x, y, visible } => {
                    CursorOverlayServerMessage::CustomCursor {
                        x: Some(x),
                        y: Some(y),
                        visible,
                    }
                }
            };
            let resp = ServiceServerMessage::CursorOverlayReply {
                request_id: 0,
                message,
            };
            if let Err(err) = send_json(&mut sink, &resp).await {
                tracing::warn!(error = %err, "websocket send error draining bare cursor overlay reply");
                return;
            }
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<FilesClientMessage>(&text) {
            let required = match &msg {
                FilesClientMessage::WriteFile { .. }
                | FilesClientMessage::CreateFile { .. }
                | FilesClientMessage::CreateDir { .. }
                | FilesClientMessage::Rename { .. }
                | FilesClientMessage::Delete { .. } => Permission::WriteFiles,
                FilesClientMessage::ListDir { .. }
                | FilesClientMessage::Stat { .. }
                | FilesClientMessage::ReadFile { .. }
                | FilesClientMessage::WalkTree { .. }
                | FilesClientMessage::ReadShellHistory { .. } => Permission::ReadFiles,
            };
            if let Err(denial) = check_permission(&device, required) {
                let _ = send_json(&mut sink, &denial).await;
                continue;
            }
            let responses = files_handler::handle(msg).await;
            for resp in responses {
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error");
                    return;
                }
            }
            continue;
        }
        if let Ok(msg) = serde_json::from_str::<GitClientMessage>(&text) {
            // All current git ops are read-only; future writes (commit,
            // stage, etc.) will branch on the variant to require GitWrite.
            if let Err(denial) = check_permission(&device, Permission::ReadFiles) {
                let _ = send_json(&mut sink, &denial).await;
                continue;
            }
            let responses = git_handler::handle(msg).await;
            for resp in responses {
                if let Err(err) = send_json(&mut sink, &resp).await {
                    tracing::warn!(error = %err, "websocket send error");
                    return;
                }
            }
            continue;
        }

        let resp = ServerMessage::Error {
            message: "invalid client message: did not match any known shape".into(),
        };
        if let Err(err) = send_json(&mut sink, &resp).await {
            tracing::warn!(error = %err, "websocket send error");
            return;
        }
    }

    poll_task.abort();
    tracing::debug!("websocket connection closed");
}

/// Pull the branch string out of an initial `Branch` snapshot. Used to
/// seed the poll loop so the first tick doesn't re-push an unchanged
/// branch.
pub(crate) fn initial_branch_name(msg: &GitServerMessage) -> Option<String> {
    match msg {
        GitServerMessage::Branch { name } => name.clone(),
        _ => None,
    }
}

/// Poll the workspace every 2 seconds and push `GitReply { request_id = 0 }`
/// frames whenever a status field has changed since the last tick.
///
/// Only fields whose values changed are emitted, so an idle workspace
/// produces zero traffic after the first tick. The reserved request id
/// `0` is the same channel the initial branch snapshot uses, so the JS
/// side can route everything through the existing unsolicited-push
/// branch in `TerminalPanel::serviceReply`.
pub(crate) async fn status_poll_loop(
    tx: tokio::sync::mpsc::UnboundedSender<ServiceServerMessage>,
    seed_branch: Option<String>,
) {
    let mut last_branch = seed_branch;
    let mut last_changes: Option<(u64, u64)> = None;
    let mut tick = tokio::time::interval(std::time::Duration::from_secs(2));
    // Skip the first immediate tick — we just sent the initial snapshot
    // synchronously from the caller.
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    tick.tick().await;
    loop {
        tick.tick().await;

        // Branch can change when the user checks out a different ref.
        // Cheap to compute (libgit2 HEAD read on a blocking task).
        let branch_msg = git_handler::current_branch_snapshot().await;
        let new_branch = match &branch_msg {
            GitServerMessage::Branch { name } => name.clone(),
            _ => last_branch.clone(),
        };
        if new_branch != last_branch {
            last_branch = new_branch;
            if tx
                .send(ServiceServerMessage::GitReply {
                    request_id: 0,
                    message: branch_msg,
                })
                .is_err()
            {
                return;
            }
        }

        // Working-tree change counts. Shelled out to `git status
        // --porcelain` on a blocking task so the reactor stays free.
        let root = files_handler::workspace_root();
        let counts =
            tokio::task::spawn_blocking(move || git_handler::git_changes_snapshot(&root))
                .await
                .unwrap_or((0, 0));
        if Some(counts) != last_changes {
            last_changes = Some(counts);
            let (added, deleted) = counts;
            if tx
                .send(ServiceServerMessage::GitReply {
                    request_id: 0,
                    message: GitServerMessage::Changes { added, deleted },
                })
                .is_err()
            {
                return;
            }
        }
    }
}

pub(crate) async fn send_json<S, M>(sink: &mut S, msg: &M) -> Result<(), axum::Error>
where
    S: SinkExt<Message, Error = axum::Error> + Unpin,
    M: Serialize,
{
    let payload = serde_json::to_string(msg).map_err(|e| {
        axum::Error::new(std::io::Error::new(std::io::ErrorKind::Other, e))
    })?;
    sink.send(Message::Text(payload)).await
}

pub(crate) fn editor_messages_hash(messages: &[EditorServerMessage]) -> Option<u64> {
    let bytes = serde_json::to_vec(messages).ok()?;
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    Some(hasher.finish())
}

pub(crate) async fn recv_broadcast<T: Clone>(
    rx: &mut tokio::sync::broadcast::Receiver<T>,
    label: &'static str,
) -> Option<T> {
    loop {
        match rx.recv().await {
            Ok(message) => return Some(message),
            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                tracing::warn!(skipped, stream = label, "websocket broadcast lagged");
            }
            Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
        }
    }
}

#[cfg(test)]
mod completion_task_tests {
    use super::*;

    #[tokio::test]
    async fn replacing_completion_task_aborts_debounced_predecessor() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let mut tasks = SocketCompletionTasks::new();

        let old_tx = tx.clone();
        tasks.replace(
            "pane:editor".to_string(),
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                let _ = old_tx.send(1u8);
            }),
        );
        tasks.replace(
            "pane:editor".to_string(),
            tokio::spawn(async move {
                let _ = tx.send(2u8);
            }),
        );

        assert_eq!(
            tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv())
                .await
                .expect("replacement task should run"),
            Some(2)
        );
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
        assert!(rx.try_recv().is_err(), "superseded task still ran");
    }
}
