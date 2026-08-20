use super::*;

/// Outbound server messages — daemon workspace manager -> chrome.
//
// `Eq` is intentionally omitted because [`WorkspaceServerMessage::PaneLayoutChanged`]
// transitively embeds a `PaneLayoutOp` (whose `ResizeRatio` carries an
// `f32`); `PartialEq` is still derived for the roundtrip tests.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkspaceServerMessage {
    /// Reply to `RequestShareTarget`. `url` is absent when the daemon has
    /// no address another device could reach (offline, or loopback-only),
    /// in which case `hint` explains why so the UI can say something
    /// useful instead of showing a dead QR code.
    ShareTarget {
        url: Option<String>,
        hint: Option<String>,
    },
    /// Reply to `ListProjectRoots` — the full known registry, ordered by
    /// most-recently-opened first (mirrors the on-disk registry sort).
    ProjectRootList {
        project_roots: Vec<ProjectRootSummary>,
    },
    /// Reply to `GetProjectRootInfo` — everything the chrome needs to
    /// render a project-root detail panel.
    ProjectRootInfo {
        id: String,
        name: String,
        path: PathBuf,
        /// Sessions currently live in this workspace.
        sessions: Vec<SessionSummary>,
        /// True iff this workspace is the connection's active one.
        active: bool,
    },
    /// Emitted on successful `OpenProjectRoot` (including re-open of an
    /// already-known root). Always followed by an implicit switch.
    ProjectRootOpened { project_root: ProjectRootSummary },
    /// Emitted on successful `CloseProjectRoot`.
    ProjectRootClosed { id: String },
    /// Push notification: the active project root for this connection
    /// changed (either via `SwitchProjectRoot`, an `OpenProjectRoot`, or
    /// because the previous active root was forgotten). The
    /// chrome should refresh its file tree, git status, etc.
    ProjectRootChanged { id: Option<String> },
    /// Reply to `ListSessions`.
    SessionList { sessions: Vec<SessionSummary> },
    /// Reply to `GetSessionState`.
    SessionState {
        id: String,
        workspace_id: String,
        cwd: String,
        label: Option<String>,
        /// Unix-seconds timestamp of the most recent input/output.
        last_active: i64,
    },
    /// Push notification: the connection's active session changed.
    SessionChanged { session_id: Option<String> },
    /// Push notification: a session was created (typically in response
    /// to a `NewSession` but may also fire for sessions created on
    /// the daemon side — e.g. an auto-restored session).
    SessionCreated { session: SessionSummary },
    /// Push notification: a session was closed.
    SessionClosed { session_id: String },
    /// Reply to `ListEditorSurfaces`.
    EditorSurfaceList { surfaces: Vec<EditorSurfaceSummary> },
    /// Push notification / ack: one pane/editor surface was bound or
    /// retargeted to a session/path.
    EditorSurfaceChanged { surface: EditorSurfaceSummary },
    /// Push notification / ack: one pane/editor surface was removed.
    EditorSurfaceClosed { surface_id: String },
    /// Reply to [`WorkspaceClientMessage::ListWindows`].
    WindowList {
        windows: Vec<WorkspaceWindowSummary>,
    },
    /// Push notification / ack: a logical window was opened.
    WindowOpened { window: WorkspaceWindowSummary },
    /// Push notification / ack: a logical window was closed.
    WindowClosed { window_id: String },
    /// Push notification / ack: a logical window's metadata changed.
    WindowChanged { window: WorkspaceWindowSummary },
    /// Workspace action completed.
    WorkspaceActionCompleted {
        action: WorkspaceAction,
        /// Optional workspace/file path produced by the action.
        path: Option<PathBuf>,
        message: String,
    },
    /// Reply to `LoadClipboard` or acknowledgement of
    /// `StoreClipboard`.
    ClipboardPayload { payload: Option<ClipboardPayload> },
    /// Reply to `MaterializeClipboardImage`. `path` is the absolute
    /// daemon-side path the image bytes were written to (suitable for
    /// `:edit <path>` against an nvim that shares the daemon's
    /// filesystem). `mime_type` echoes the request so the client can
    /// pick the right ex command (e.g. neovim image plugins or a
    /// fallback `:edit`). `request_id` echoes the inbound correlation
    /// token (or is `None` if the request omitted one) so multi-pane
    /// frontends can dispatch the reply to the originating pane —
    /// the focused surface at reply time may not be the one that
    /// pasted. On failure the daemon emits a sibling `Error` instead.
    ClipboardImageMaterialized {
        path: PathBuf,
        mime_type: String,
        filename: Option<String>,
        #[serde(default)]
        request_id: Option<String>,
    },
    /// Broadcast notification: a pane-layout mutation submitted via
    /// [`WorkspaceClientMessage::PaneLayoutOp`] landed successfully.
    ///
    /// The daemon fans this out to *every* connected client (not just
    /// the submitter) so paired surfaces (laptop chrome + phone agent
    /// + web) converge on the same layout. `op` echoes the applied op
    /// so receivers can decide whether to replay it locally or treat
    /// the message as a poke to refetch layout state.
    /// `new_layout_snapshot` is an opaque JSON string serialized from
    /// [`PaneLayoutSnapshot`]. Older clients may ignore it; newer
    /// web/phone clients can parse it to converge on the daemon's
    /// current pane/surface inventory.
    PaneLayoutChanged {
        pane_external_id: u64,
        op: PaneLayoutOp,
        #[serde(default)]
        new_layout_snapshot: Option<String>,
    },
    /// Generic error response. The daemon emits this for any
    /// command-level failure (unknown id, path traversal, missing
    /// active workspace, etc.).
    Error { message: String },
    /// Reply to a [`WorkspaceClientMessage::Hello`] handshake.
    ///
    /// `accepted = true` means the daemon was happy with the presented
    /// token (or no auth was required); `accepted = false` means the
    /// daemon will close the connection shortly. `reason` is a
    /// human-readable explanation suitable for the client to surface in
    /// a toast or log line — never include the raw token in it.
    ///
    /// `peer_identity` carries an optional, log-only attribution string
    /// the daemon may have resolved server-side (e.g. via `tailscale
    /// whois`). The client can ignore it; it exists so a paired chrome
    /// can render "connected to laptop-A (you@tailnet)" without making
    /// its own tailscale calls.
    HelloAck {
        accepted: bool,
        #[serde(default)]
        reason: Option<String>,
        #[serde(default)]
        peer_identity: Option<String>,
    },
    /// Reply to [`WorkspaceClientMessage::GetWorkplacePreferences`].
    /// `prefs` is the daemon's persisted value (or the default empty
    /// struct when nothing has ever been set for this workspace).
    WorkplacePreferences {
        workspace_id: String,
        prefs: WorkplacePreferences,
    },
    /// Broadcast notification: the per-workplace preferences for
    /// `workspace_id` were updated via
    /// [`WorkspaceClientMessage::SetWorkplacePreferences`]. The daemon
    /// fans this out to **every** connected client (including the
    /// submitter) so paired surfaces re-apply theme / font-size /
    /// sidebar widths without polling. Clients that aren't currently
    /// focused on `workspace_id` should still cache the update so a
    /// subsequent switch applies the latest value immediately.
    WorkplacePreferencesChanged {
        workspace_id: String,
        prefs: WorkplacePreferences,
    },
    /// Reply to [`WorkspaceClientMessage::ListHosts`].
    HostList { hosts: Vec<HostSummary> },
    /// Reply/broadcast for the real top-level workspace list.
    HostWorkspaceList { workspaces: Vec<WorkspaceSummary> },
    /// Reply for tabs/sessions under one real top-level workspace.
    WorkspaceTabList { tabs: Vec<WorkspaceTabSummary> },
    /// Full shared tree used by web/desktop navigators.
    HostWorkspaceTree {
        hosts: Vec<HostSummary>,
        workspaces: Vec<WorkspaceSummary>,
        tabs: Vec<WorkspaceTabSummary>,
    },
    /// Reply to [`WorkspaceClientMessage::ResolveInitialWorkspace`].
    /// The workspace is guaranteed to exist in the daemon-owned
    /// host-workspace registry by the time this reply is sent.
    InitialWorkspaceResolved {
        workspace: WorkspaceSummary,
        reason: InitialWorkspaceReason,
    },
    /// Reply/broadcast when a top-level daemon workspace is created or
    /// its metadata is upserted. This does not imply the host's active
    /// workspace changed; active changes are `HostWorkspaceChanged`.
    HostWorkspaceUpserted { workspace: WorkspaceSummary },
    /// Broadcast when a host's active top-level workspace changes.
    HostWorkspaceChanged {
        host_id: String,
        workspace_id: Option<String>,
    },
    /// Broadcast when control/ownership metadata changes for a workspace.
    WorkspaceControlChanged { workspace: WorkspaceSummary },
    /// Broadcast when a tab has moved to another workspace.
    WorkspaceTabMoved { tab: WorkspaceTabSummary },
    /// Reply to [`WorkspaceClientMessage::RequestFullSnapshot`].
    ///
    /// Carries everything a fresh (or reconnecting) client needs to
    /// rehydrate its view of the daemon's authoritative state without a
    /// flurry of follow-up `List*` round-trips:
    ///
    /// * `client_id` is the daemon-assigned stable id for this
    ///   connection. When the client presented [`Uuid::nil`] in its
    ///   `Hello`, this is the freshly minted id it must persist and
    ///   replay on the next reconnect. When the client presented a
    ///   real id the daemon echoes it verbatim so the client can
    ///   confirm the assignment took.
    /// * `sessions` mirrors a synchronous [`SessionList`] for the
    ///   currently active workspace; empty when no workspace is
    ///   active.
    /// * `layout` is the canonical pane-layout snapshot for the
    ///   active workspace (the same value
    ///   [`PaneLayoutChanged::new_layout_snapshot`] gradually broadcasts
    ///   on diff), or `None` when the workspace has no bound editor
    ///   surfaces yet.
    /// * `prefs` is the per-workplace preferences map keyed by
    ///   workspace id — clients cache the full map so a subsequent
    ///   `SwitchProjectRoot` applies the right theme/font without
    ///   re-fetching.
    /// * `pty_offsets` is the next offset the daemon would write for
    ///   each route's PTY history ring. Clients persist the map; on
    ///   the next reconnect they pass any one of these values back via
    ///   [`WorkspaceClientMessage::RequestFullSnapshot::since_offset`]
    ///   so the daemon can replay everything between the saved offset
    ///   and the current head as
    ///   [`WorkspaceServerMessage::PtyBacklog`] frames.
    FullSnapshot {
        client_id: Uuid,
        sessions: Vec<SessionSummary>,
        #[serde(default)]
        layout: Option<PaneLayoutSnapshot>,
        prefs: HashMap<String, WorkplacePreferences>,
        pty_offsets: HashMap<RouteId, u64>,
    },
    /// Replay PTY output that landed on `route_id` while the client was
    /// disconnected.
    ///
    /// The daemon retains the last ~1 MB of bytes per route (configurable
    /// at daemon boot via `NEOISM_PTY_HISTORY_BYTES`). When a client
    /// reconnects and sends
    /// [`WorkspaceClientMessage::RequestFullSnapshot::since_offset`],
    /// the daemon walks each known route's ring buffer and emits one
    /// `PtyBacklog` per route whose head moved past the requested offset.
    /// `from_offset` is the absolute byte offset at which `bytes` begins
    /// (i.e. the offset the client should advance its cursor *to* once
    /// the slice is applied); the next live `PtyOutput` frame the
    /// daemon pushes will continue from `from_offset + bytes.len()`. If
    /// the requested offset has already fallen out of the ring (the
    /// disconnect was longer than the buffer can cover) `from_offset`
    /// will be the oldest still-retained offset and the client should
    /// treat the gap as lost output.
    PtyBacklog {
        route_id: RouteId,
        bytes: Vec<u8>,
        from_offset: u64,
    },
    /// Reply to [`WorkspaceClientMessage::ListPairings`] — a snapshot
    /// of every currently accepted pairing token rendered as a
    /// [`PairingSummary`]. The list is daemon-ordered (typically
    /// `created_at` ascending) so a UI can render rows in a stable
    /// sequence.
    PairingList { pairings: Vec<PairingSummary> },
    /// Reply to [`WorkspaceClientMessage::RevokePairing`] echoing back
    /// the prefix and a `removed` flag (`true` when the daemon dropped
    /// a matching entry, `false` when nothing matched — typically
    /// because another client raced the revoke or the prefix was
    /// already stale).
    PairingRevoked {
        fingerprint_prefix: String,
        removed: bool,
    },
    /// Broadcast the daemon fans out to *every* connected client when
    /// the host is ending the shared session — the daemon is shutting
    /// down (SIGTERM / SIGINT / console-close) or the host explicitly
    /// stopped sharing this workspace. A guest uses it to detach its
    /// adopted workspace (WITHOUT closing the host's live shells),
    /// re-dial its own home daemon, and surface a short notice instead
    /// of silently reconnecting forever against a dead / withdrawn
    /// host. `reason` is a human-readable string suitable for a toast
    /// or modal (e.g. "The host ended the session").
    HostEnded { reason: String },
}
