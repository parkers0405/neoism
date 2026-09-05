use super::*;

/// Inbound client messages — web/desktop chrome -> daemon workspace
/// manager.
///
/// Host/workspace/session control messages. `Workspace` means the real
/// top-level UI container created by Ctrl+Shift+W. Directory/project-root
/// bindings use `ProjectRoot` names so the model stays unambiguous:
/// Host -> Workspace -> recursive Splits -> per-pane Tabs/Sessions.
//
// `Eq` is intentionally omitted because [`PaneLayoutOp::ResizeRatio`]
// carries a `f32`; `PartialEq` is enough for the roundtrip tests below.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum WorkspaceClientMessage {
    /// Open an existing project root (must have a `.neoism/workspace.toml`)
    /// or initialise a new one at `path`. The daemon registers the
    /// directory binding and emits [`WorkspaceServerMessage::ProjectRootOpened`].
    OpenProjectRoot {
        path: PathBuf,
        /// If true and `path` has no `.neoism/` marker, the daemon
        /// scaffolds a fresh workspace (mirrors `init_workspace`).
        /// Otherwise an `Error` is returned for unmarked paths.
        #[serde(default)]
        init_if_missing: bool,
    },
    /// Close `id` and drop any in-memory state (sessions, watchers,
    /// caches). The project root stays in the on-disk registry so a future
    /// `OpenProjectRoot` can resurrect it.
    CloseProjectRoot {
        id: String,
    },
    /// Request the full registry of known project roots. The daemon
    /// replies with [`WorkspaceServerMessage::ProjectRootList`].
    ListProjectRoots,
    /// Make `id` the active project root for this connection. All
    /// subsequent `files` / `git` / `editor` envelopes resolve their
    /// paths relative to the active root.
    SwitchProjectRoot {
        id: String,
    },
    /// Request a single project root's metadata + session inventory.
    GetProjectRootInfo {
        id: String,
    },
    /// Update the human-facing name of a project root. Persisted to the
    /// on-disk registry.
    RenameProjectRoot {
        id: String,
        name: String,
    },
    /// Remove a project root from the registry (does NOT delete files on
    /// disk). If `id` is the active root, the connection falls back to
    /// no active project root until another `SwitchProjectRoot`.
    ForgetProjectRoot {
        id: String,
    },
    /// Enumerate the sessions belonging to the active project root. The
    /// daemon replies with [`WorkspaceServerMessage::SessionList`].
    ListSessions,
    /// Bind the connection to an existing session. `session_id` must
    /// belong to the active workspace.
    SwitchSession {
        session_id: String,
    },
    /// Spawn a new session in the active workspace. `cwd` is
    /// workspace-relative; `None` means the workspace root.
    NewSession {
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        label: Option<String>,
    },
    /// Tear down `session_id` and free its resources.
    CloseSession {
        session_id: String,
    },
    /// Snapshot the current state (cwd, label, last activity) of one
    /// session. Daemon replies with [`WorkspaceServerMessage::SessionState`].
    GetSessionState {
        session_id: String,
    },
    /// Update the cwd of `session_id` to `path` (workspace-relative).
    SetCwd {
        session_id: String,
        path: String,
    },
    /// Update the human-facing label of a session.
    RenameSession {
        session_id: String,
        label: String,
    },
    /// Bind a web/editor pane route to a workspace session and optional
    /// active buffer. This is intentionally independent from the nvim
    /// redraw stream: it lets clients model desktop-style panes before
    /// each surface has its own rendered grid snapshot.
    BindEditorSurface {
        surface_id: String,
        session_id: String,
        #[serde(default)]
        path: Option<PathBuf>,
    },
    /// Enumerate editor surfaces for the active workspace.
    ListEditorSurfaces,
    /// Remove an editor surface binding. The backing session remains
    /// alive unless the client closes it separately.
    CloseEditorSurface {
        surface_id: String,
    },
    /// Request that the daemon allocate a logical top-level terminal
    /// window. Native clients materialise the returned summary as a
    /// winit window; web clients may render it as a browser route.
    RequestOpenWindow {
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    /// Request a daemon-owned logical window that should be
    /// materialised as a native OS tab when the host supports it.
    RequestOpenNativeTab {
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default)]
        parent_window_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
    /// Request a daemon-owned logical config-editor route.
    RequestOpenConfigEditor {
        #[serde(default)]
        workspace_id: Option<String>,
    },
    /// Close a logical window. Native clients should tear down the
    /// corresponding winit window/tab when they observe the
    /// [`WorkspaceServerMessage::WindowClosed`] ack/broadcast.
    RequestCloseWindow {
        window_id: String,
    },
    /// Enumerate the daemon's logical window registry.
    ListWindows,
    /// Execute a daemon-side workspace action selected from the
    /// command palette. Frontends can still handle UI-local palette
    /// actions themselves; these are the ones with durable workspace
    /// side effects.
    RunWorkspaceAction {
        action: WorkspaceAction,
    },
    /// Store the latest clipboard payload observed by the client.
    /// Text-only consumers can use `text`; image-capable consumers can
    /// preserve `mime_type` + `bytes` without lossy re-encoding.
    StoreClipboard {
        payload: ClipboardPayload,
    },
    /// Fetch the last payload stored through `StoreClipboard` on this
    /// connection.
    LoadClipboard,
    /// Materialise an image clipboard payload to a daemon-controlled
    /// temp file so frontends can drop the bytes into nvim with a
    /// plain `:edit <path>` (instead of base64-encoding into a paste
    /// register). The daemon replies with a `ClipboardImageMaterialized`
    /// containing the absolute path on its filesystem; the path lives
    /// until the daemon's tempdir GC sweeps it (24h TTL + LRU
    /// eviction once the temp dir grows past ~100 files). The payload
    /// is *not* cached in `conn.clipboard_payload` — callers that want
    /// that too should send a sibling `StoreClipboard`.
    ///
    /// `request_id` is an opaque correlation token echoed back in the
    /// `ClipboardImageMaterialized` reply. Multi-pane frontends use it
    /// to route the reply to the pane that initiated the paste — the
    /// user may have switched focus while the daemon was writing the
    /// file. `None` opts out of correlation (single-pane callers).
    MaterializeClipboardImage {
        payload: ClipboardPayload,
        #[serde(default)]
        request_id: Option<String>,
    },
    /// Mutate the pane layout of the active workspace's session-layout
    /// tree. `pane_external_id` is the integer external id used by the
    /// shared `SessionLayout` (mirrored by the chrome's pane overlay).
    /// Idempotent for ops that target an already-correct state.
    ///
    /// This is the "phone agent drives the laptop" surface — the
    /// daemon broadcasts the resulting [`WorkspaceServerMessage::PaneLayoutChanged`]
    /// to *every* connected client so the laptop chrome (or any other
    /// paired surface) materialises the same layout change.
    PaneLayoutOp {
        pane_external_id: u64,
        op: PaneLayoutOp,
    },
    /// Replace one workspace's daemon-owned recursive pane tree with the
    /// complete snapshot materialized by its native host. This is used for
    /// drag operations that move an existing tab, where replaying `Split`
    /// would incorrectly allocate another leaf.
    PublishPaneLayout {
        layout: PaneLayoutSnapshot,
    },
    /// Multi-machine pairing handshake. A client SHOULD send this as
    /// its first frame on a fresh websocket so the daemon can
    /// (a) record the peer identity for audit/logging and (b) enforce
    /// pairing-token-based auth when `NEOISM_REQUIRE_AUTH=1`.
    ///
    /// `token` is the short-lived pairing token printed on daemon
    /// startup (or read from `~/.config/neoism/pairing-tokens`). The
    /// field is `#[serde(default)]` so existing clients that never
    /// learned to send `Hello` keep working — the daemon falls back to
    /// "trust local" semantics when the env-var gate is off.
    ///
    /// `client_name` is a human-facing label (e.g. `neoism-desktop`,
    /// `iPhone`, `web/Chrome`) used only for log/audit lines. It never
    /// participates in the auth decision.
    ///
    /// `client_id` is a stable per-installation identifier the daemon
    /// uses to recognise reconnecting clients and resume their server-
    /// side state (PTY offsets, layout, prefs) instead of treating
    /// every websocket as fresh. Clients that don't yet have a stored
    /// id should send [`Uuid::nil`] on their very first `Hello`; the
    /// daemon will then mint one and return it in the subsequent
    /// [`WorkspaceServerMessage::FullSnapshot::client_id`]. Persist
    /// that id locally and replay it verbatim on the next reconnect.
    /// The field is `#[serde(default)]` so pre-G2 clients (and the
    /// integration tests written before this wave) keep parsing — a
    /// missing field deserialises to [`Uuid::nil`] which the daemon
    /// treats identically to "fresh client, mint me an id".
    Hello {
        #[serde(default)]
        token: Option<String>,
        #[serde(default)]
        client_name: Option<String>,
        #[serde(default)]
        client_id: Uuid,
    },
    /// Application-level liveness probe. Browsers cannot originate WebSocket
    /// control Ping frames, and Safari may retain an OPEN-looking socket after
    /// page suspension even though the transport is dead.
    Ping { nonce: String },
    /// Ask the daemon to send a [`WorkspaceServerMessage::FullSnapshot`]
    /// describing the connection's current authoritative view (sessions,
    /// pane layout, persisted preferences, per-route PTY offsets). The
    /// daemon resolves the snapshot against the currently active
    /// workspace; when no workspace is active the reply still carries
    /// empty collections so the client can render a "no workspace"
    /// state without polling.
    ///
    /// `since_offset` is an optional opaque cursor the client received
    /// on its **previous** session through
    /// [`WorkspaceServerMessage::FullSnapshot::pty_offsets`]. When
    /// `Some(offset)` the daemon will additionally fan out
    /// [`WorkspaceServerMessage::PtyBacklog`] frames for every route
    /// whose history ring advanced past `offset`, so the client
    /// catches up on missed terminal output before any new PTY frames
    /// arrive. `None` is the cold-start path (no offset known) — only
    /// the snapshot is returned.
    RequestFullSnapshot {
        #[serde(default)]
        since_offset: Option<u64>,
    },
    /// Request the daemon's list of currently accepted pairing tokens,
    /// surfaced as renderable [`PairingSummary`] entries (device label,
    /// last-seen, short fingerprint prefix). Never includes the raw
    /// token / private key — see the security note on
    /// [`PairingSummary`].
    ///
    /// Used by the desktop settings panel that lists paired devices
    /// with a revoke button. The daemon replies with
    /// [`WorkspaceServerMessage::PairingList`].
    ListPairings,
    /// Revoke the pairing token whose SHA-256 fingerprint matches
    /// `fingerprint_prefix`. The prefix length the daemon recognises is
    /// driven by its store implementation (currently 12 hex chars);
    /// callers should send exactly what
    /// [`PairingSummary::fingerprint_prefix`] returned to them.
    ///
    /// The daemon replies with a
    /// [`WorkspaceServerMessage::PairingRevoked`] carrying the prefix
    /// echo + a `removed` flag (false when no matching token was
    /// found, e.g. because another client raced the revoke).
    RevokePairing {
        fingerprint_prefix: String,
    },
    /// Fetch the persisted per-workplace UI preferences (theme, font
    /// size, sidebar widths, last session-layout snapshot) for
    /// `workspace_id`. The daemon replies with a
    /// [`WorkspaceServerMessage::WorkplacePreferences`] carrying the
    /// stored `prefs` (or the default empty struct if nothing has been
    /// persisted yet). Missing workspaces still produce a default
    /// response — chrome never has to special-case "first run".
    GetWorkplacePreferences {
        workspace_id: String,
    },
    /// Replace the persisted per-workplace UI preferences for
    /// `workspace_id`. The daemon writes the new value through to the
    /// existing workspace registry file and fans out a
    /// [`WorkspaceServerMessage::WorkplacePreferencesChanged`] to
    /// **every** connected client so paired surfaces converge on the
    /// same theme / layout without needing to re-poll.
    SetWorkplacePreferences {
        workspace_id: String,
        prefs: WorkplacePreferences,
    },
    /// List machines/runtimes that can own or control top-level UI
    /// workspaces. This is the real product model: a host (framework,
    /// mac, web controller) owns many workspaces.
    ListHosts,
    /// Upsert host/device presence and dial metadata. Hosts are separate
    /// from workspaces: this does not create, delete, or switch any
    /// workspace.
    UpsertHost {
        host: HostSummary,
    },
    /// List the daemon-owned top-level workspaces. These are the
    /// `Ctrl+Shift+W` UI workspaces, not project-root directory records.
    /// A workspace has identity independent from its `root_dir`, so two
    /// workspaces may point at the same directory and remain separate.
    ListHostWorkspaces {
        host_id: Option<String>,
    },
    /// List tabs/sessions owned by one top-level workspace.
    ListWorkspaceTabs {
        workspace_id: String,
    },
    /// Fetch the full shared host/workspace/tab tree in one message so
    /// web and desktop can render the same navigator without piecemeal
    /// polling.
    RequestHostWorkspaceTree,
    /// Ask the daemon to choose the workspace this client should open
    /// on boot. This is the canonical startup path for web/desktop:
    /// prefer the reconnecting client's remembered workspace, then a
    /// host's active workspace, then the most-recent daemon workspace,
    /// creating a daemon-owned default if none exist.
    ResolveInitialWorkspace {
        #[serde(default)]
        preferred_host_id: Option<String>,
    },
    /// Create a real top-level workspace on `host_id`. `root_dir` is a
    /// directory binding, not identity.
    CreateHostWorkspace {
        host_id: String,
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        root_dir: Option<PathBuf>,
    },
    /// Close/remove a top-level host workspace from this daemon. This is the
    /// destructive local-owner close path; remote/cloud subscriptions should
    /// use `UnsubscribeWorkspace` instead.
    CloseHostWorkspace {
        workspace_id: String,
    },
    /// Create/upsert a top-level workspace on the daemon's default host.
    /// Browser clients use this so they do not infer host ids from a
    /// render tree.
    CreateWorkspace {
        #[serde(default)]
        workspace_id: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        root_dir: Option<PathBuf>,
    },
    /// Switch a host's active top-level workspace. Frontends can choose
    /// to materialise/control it after receiving the broadcast.
    SwitchHostWorkspace {
        workspace_id: String,
    },
    /// Re-point an existing workspace's working directory — the explicit
    /// ":cd" for a workspace. The daemon updates `root_dir` (creating the
    /// directory if missing) and broadcasts a tree change so every client
    /// in that workspace re-roots its Explorer. Distinct from a terminal
    /// `cd`, which is local to one user's shell and never moves the tree.
    SetWorkspaceRoot {
        workspace_id: String,
        root_dir: PathBuf,
    },
    /// Create a dedicated notes vault on the workspace's running host and
    /// link that vault to the workspace directory. This is used by a joined
    /// guest's no-vault empty state; the mutation must happen on the daemon
    /// machine, never against the guest's local Default vault.
    CreateWorkspaceVault {
        workspace_id: String,
    },
    /// Ask the daemon for a URL a PHONE (or any other device on the
    /// network) can open to reach this same workspace. Only the daemon
    /// can answer: the browser sees whatever origin it was loaded from,
    /// which for a local session is loopback and useless to a phone.
    RequestShareTarget {
        workspace_id: Option<String>,
    },
    /// Advertise a local workspace to other clients/peers. MVP targets a
    /// direct/Tailscale connection; future registries can discover the same
    /// shared workspace metadata.
    ShareWorkspace {
        workspace_id: String,
    },
    /// Stop advertising a previously shared workspace. Local state remains.
    StopSharingWorkspace {
        workspace_id: String,
    },
    /// Create a local Docker sandbox from the workspace snapshot and register
    /// it as a docker-hosted workspace.
    SendWorkspaceToDockerSandbox {
        workspace_id: String,
    },
    /// Upload the workspace snapshot to Neoism Cloud and register the returned
    /// sandbox as a cloud-hosted workspace.
    SendWorkspaceToCloud {
        workspace_id: String,
    },
    /// Add/remove a workspace from this client's/window group's top chrome.
    /// The full workspace registry can contain more local/shared/cloud entries
    /// than are currently subscribed/open in a given window.
    SubscribeWorkspace {
        workspace_id: String,
    },
    UnsubscribeWorkspace {
        workspace_id: String,
    },
    /// Attach this client/host as a controller for a workspace that may
    /// still be running on another host.
    ControlWorkspace {
        workspace_id: String,
        controller_host_id: String,
    },
    /// Stop controlling a workspace without moving or closing it.
    ReleaseWorkspaceControl {
        workspace_id: String,
        controller_host_id: String,
    },
    /// Transfer workspace ownership/running host. Live process migration
    /// is host-dependent; this updates daemon ownership metadata and the
    /// destination host can adopt/hydrate from the workspace snapshot.
    MoveWorkspaceToHost {
        workspace_id: String,
        target_host_id: String,
    },
    /// Move a tab/session into another workspace on the same or another host.
    MoveTabToWorkspace {
        tab_id: String,
        target_workspace_id: String,
    },
    /// Move a tab/session into a workspace, asserting the destination host.
    /// This is a UI convenience for cross-host send flows; the daemon
    /// validates that the target workspace belongs to `target_host_id`.
    MoveTabToHostWorkspace {
        tab_id: String,
        target_host_id: String,
        target_workspace_id: String,
    },
    /// Replace ONE workspace's tab list without touching any other
    /// tree state. The web client (a controller, not a host — it owns
    /// no host entry to snapshot) uses this to record its open tabs so
    /// other clients can adopt the workspace with its buffers and
    /// sessions intact.
    PublishWorkspaceTabs {
        workspace_id: String,
        tabs: Vec<WorkspaceTabSummary>,
    },
}
