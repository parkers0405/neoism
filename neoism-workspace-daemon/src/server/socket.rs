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
    /// Wire-protocol variant kept for clients that still parse it; the
    /// nvim-fed diagnostics poller that produced these pushes is gone,
    /// so nothing constructs it until the native editor's daemon path
    /// lands.
    #[allow(dead_code)]
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

pub(crate) async fn handle_socket(
    socket: WebSocket,
    registry: SessionRegistry,
    mut output_rx: tokio::sync::broadcast::Receiver<ServerMessage>,
    device: Option<crate::auth::DeviceRecord>,
    upgrade_auth_reason: Option<&'static str>,
    peer_ip: Option<String>,
    workspace_manager: WorkspaceManager,
    pairing_tokens: PairingTokenStore,
    crdt: CrdtSyncHub,
) {
    let (mut sink, mut stream) = socket.split();
    tracing::debug!("websocket connection established");

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
    let (push_tx, mut push_rx) =
        tokio::sync::mpsc::unbounded_channel::<ServiceServerMessage>();
    let poll_task = tokio::spawn(status_poll_loop(
        push_tx,
        initial_branch_name(&initial_branch),
    ));

    // Real-time `publishDiagnostics` bus (event-driven — no polling). The
    // engine pushes here the instant a language server publishes; we forward
    // to the editor from the select loop below. `active_editor_file` gates it
    // so a workspace-wide push for a non-focused buffer isn't shown.
    let mut lsp_diagnostics_rx = crate::language_server::subscribe_diagnostics();
    let mut active_editor_file: Option<String> = None;
    // Latest `Editor` envelope request id. Unsolicited engine pushes are
    // tagged with it so the client's reply-correlation routing still works.
    let mut editor_request_id: u64 = 0;

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
                                request_id: editor_request_id,
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
                    // Editor envelopes reuse the file-read permission — a
                    // session that can read files can also drive the editor.
                    if let Err(denial) = check_permission(&device, Permission::ReadFiles)
                    {
                        let _ = send_json(&mut sink, &denial).await;
                        continue;
                    }
                    // Track the latest request_id so unsolicited engine
                    // pushes (diagnostics) route through the same channel
                    // on the JS side.
                    editor_request_id = request_id;
                    // Fire-and-forget teardown/cancel: nothing to tear down
                    // without a backing editor session, and no reply is
                    // expected.
                    if matches!(
                        message,
                        EditorClientMessage::Close
                            | EditorClientMessage::CancelLspCompletion { .. }
                    ) {
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

                    // Track the focused editor file so real-time diagnostics
                    // pushes are shown only for the buffer the user is in.
                    if let EditorClientMessage::OpenBuffer { path, .. } = &message {
                        active_editor_file = Some(path.to_string_lossy().into_owned());
                    }

                    // The embedded-nvim backend behind this envelope is gone.
                    // LSP requests are still answered by the Neoism engine
                    // where that is possible without a live buffer; grid and
                    // input messages get the standard error reply until the
                    // native editor's daemon path lands and rewires them.
                    let reply = match message {
                        EditorClientMessage::LspAction {
                            action,
                            text,
                            surface_id,
                        } => match crate::language_server::run_action(
                            &root,
                            action,
                            text.as_deref(),
                        ) {
                            Ok(mut message) => {
                                if let EditorServerMessage::LspActionResult {
                                    surface_id: target,
                                    ..
                                } = &mut message
                                {
                                    *target = surface_id;
                                }
                                message
                            }
                            Err(message) => EditorServerMessage::Error {
                                surface_id,
                                message,
                            },
                        },
                        EditorClientMessage::ApplyLspCodeAction {
                            action,
                            surface_id,
                        } => match crate::language_server::run_code_action(&root, action)
                        {
                            Ok(mut message) => {
                                if let EditorServerMessage::LspActionResult {
                                    surface_id: target,
                                    ..
                                } = &mut message
                                {
                                    *target = surface_id;
                                }
                                message
                            }
                            Err(message) => EditorServerMessage::Error {
                                surface_id,
                                message,
                            },
                        },
                        EditorClientMessage::ApplyLspCompletion {
                            item,
                            replace_prefix,
                            surface_id,
                        } => match crate::language_server::run_completion(
                            &root,
                            item,
                            &replace_prefix,
                        ) {
                            // Success needs no reply (the edit stream is the
                            // acknowledgement); only failures are reported.
                            Ok(()) => continue,
                            Err(message) => EditorServerMessage::Error {
                                surface_id,
                                message,
                            },
                        },
                        EditorClientMessage::LspComplete {
                            seq,
                            trigger_character,
                            surface_id,
                        } => {
                            let mut reply = crate::language_server::completion(
                                &root,
                                seq,
                                trigger_character.as_deref(),
                            );
                            if let EditorServerMessage::LspCompletions {
                                surface_id: target,
                                ..
                            } = &mut reply
                            {
                                *target = surface_id;
                            }
                            reply
                        }
                        EditorClientMessage::LspHoverAt {
                            seq,
                            grid,
                            row,
                            col,
                            surface_id,
                        } => {
                            let mut reply = crate::language_server::hover_at(
                                &root, seq, grid, row, col,
                            );
                            if let EditorServerMessage::LspHoverResult {
                                surface_id: target,
                                ..
                            } = &mut reply
                            {
                                *target = surface_id;
                            }
                            reply
                        }
                        // OpenBuffer / SendKeys / Command / MouseInput /
                        // Resize drove the embedded nvim grid. The native
                        // editor will reuse this wire; until then the client
                        // gets the standard error shape instead of hanging.
                        other => EditorServerMessage::Error {
                            surface_id: other.surface_id().map(str::to_owned),
                            message: "editor backend unavailable: embedded nvim was \
                                      removed; the native editor daemon path is pending"
                                .to_string(),
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
                    // The diagnostics-route poller was fed by the embedded
                    // nvim session, which is gone. Subscribe/unsubscribe are
                    // accepted and ignored (no reply is expected); real-time
                    // engine diagnostics still flow through the Editor
                    // envelope's publishDiagnostics forwarding above.
                    tracing::trace!(
                        request_id,
                        ?message,
                        "diagnostics subscription ignored (native editor path pending)"
                    );
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

