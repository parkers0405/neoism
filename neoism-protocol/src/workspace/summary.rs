use super::*;

/// Compact project-root descriptor used by `ProjectRootList`,
/// `ProjectRootOpened`, etc. This is a directory binding, not the
/// top-level Ctrl+Shift+W workspace identity.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectRootSummary {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    /// Unix-seconds timestamp from the registry. `0` for never-opened.
    pub last_opened: i64,
}

/// A machine/runtime that can own or control top-level workspaces.
/// Examples: `framework`, `mac`, an embedded desktop daemon, or a web
/// controller. Hosts own workspaces; clients may still remotely control
/// a workspace running on another host.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HostSummary {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub online: bool,
    #[serde(default)]
    pub peer_identity: Option<String>,
    #[serde(default)]
    pub last_seen: i64,
    /// Canonical, client-dialable daemon endpoint for this host, e.g.
    /// `ws://100.64.0.2:7878/session`. Lets a client resolve a
    /// workspace's `running_on_host_id` to a reachable URL so it can
    /// re-dial when a workspace is promoted/demoted between hosts.
    /// `None` for hosts whose address isn't known (e.g. a local
    /// bootstrap host with no `NEOISM_HOST_URL` set).
    #[serde(default)]
    pub daemon_url: Option<String>,
    /// The workspace this host is currently sitting in, when known.
    /// Filled by the daemon from its active-workspace pointer when it
    /// builds `HostWorkspaceTree`. Startup clients should use
    /// `ResolveInitialWorkspace`; this field is for rendering host
    /// state, not for guessing boot routing.
    #[serde(default)]
    pub active_workspace_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceHostKind {
    Local,
    Tailscale,
    DockerSandbox,
    CloudSandbox,
}

impl Default for WorkspaceHostKind {
    fn default() -> Self {
        Self::Local
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceVisibility {
    Private,
    Shared,
    Team,
}

impl Default for WorkspaceVisibility {
    fn default() -> Self {
        Self::Private
    }
}

/// Real top-level UI workspace created by `Ctrl+Shift+W`. This is not a
/// project-root record: `root_dir` is just the directory binding, and
/// multiple workspaces may point at the same directory.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceSummary {
    pub id: String,
    pub host_id: String,
    pub title: String,
    /// Where the workspace currently runs: this device, a Tailscale peer,
    /// a local Docker sandbox, or Neoism Cloud. Open tabs remain per-client.
    #[serde(default)]
    pub host_kind: WorkspaceHostKind,
    /// Who can discover/attach to this workspace. Workspaces are private by
    /// default; sharing/cloud promotion makes this explicit.
    #[serde(default)]
    pub visibility: WorkspaceVisibility,
    /// The terminal session whose cwd controls `root_dir`. Secondary terminal
    /// cwd changes stay local and must not move the workspace tree.
    #[serde(default)]
    pub main_session_id: Option<String>,
    #[serde(default)]
    pub root_dir: Option<PathBuf>,
    #[serde(default)]
    pub active_tab_id: Option<String>,
    #[serde(default)]
    pub running_on_host_id: Option<String>,
    #[serde(default)]
    pub controlled_by_host_id: Option<String>,
    #[serde(default)]
    pub layout_snapshot: Option<String>,
    #[serde(default)]
    pub last_active: i64,
}

/// Why the daemon selected a startup workspace for a client.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum InitialWorkspaceReason {
    ClientRemembered,
    HostActive,
    MostRecent,
    CreatedDefault,
}

/// Child tab/session entry owned by a top-level workspace. It may point
/// at a PTY session, editor surface, or future agent pane.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceTabSummary {
    pub id: String,
    pub workspace_id: String,
    pub title: String,
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub surface_id: Option<String>,
    #[serde(default)]
    pub cwd: Option<PathBuf>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub last_active: i64,
}

/// Compact session descriptor used inside `SessionList` /
/// `ProjectRootInfo`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionSummary {
    pub id: String,
    pub workspace_id: String,
    /// Workspace-relative cwd of the session.
    pub cwd: String,
    /// Optional human-facing label; chrome falls back to `id` when
    /// `None`.
    pub label: Option<String>,
    /// Unix-seconds timestamp of the most recent input/output.
    pub last_active: i64,
}

/// Durable-enough editor pane binding used by web multi-pane parity.
/// `surface_id` is usually the shared `SessionLayout` external route id.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EditorSurfaceSummary {
    pub surface_id: String,
    pub workspace_id: String,
    pub session_id: String,
    /// Workspace-relative path of the active buffer, if known.
    pub path: Option<PathBuf>,
    /// Daemon-assigned PTY / diagnostics route id for this surface.
    /// The chrome uses this as the `route_id` it passes to
    /// `SubscribeDiagnostics`, replacing the hard-coded `1` it used
    /// while every web client ran a single embedded nvim. Optional so
    /// older daemons (which don't assign route ids) stay
    /// wire-compatible; newer daemons always populate it.
    #[serde(default)]
    pub route_id: Option<u64>,
    /// Unix-seconds timestamp of the most recent bind/retarget.
    pub last_active: i64,
}

/// Daemon-owned logical window/route entry. The desktop router maps
/// this id to an irreducibly native `winit::WindowId`; web clients can
/// treat the same id as their route key.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceWindowSummary {
    pub id: String,
    pub kind: WorkspaceWindowKind,
    /// Optional workspace binding. `None` is valid for app-global
    /// windows such as a config editor shown before any workspace is
    /// active.
    #[serde(default)]
    pub workspace_id: Option<String>,
    /// Logical parent window used by native-tab materialisers. `None`
    /// means a top-level window.
    #[serde(default)]
    pub parent_window_id: Option<String>,
    /// Human-facing title hint. Native clients can still override with
    /// platform-specific app title rules.
    pub title: String,
    /// Daemon route id associated with this window when one exists.
    #[serde(default)]
    pub route_id: Option<RouteId>,
    /// Unix-seconds creation timestamp.
    pub created_at: i64,
    /// Unix-seconds timestamp of the most recent mutation/focus hint.
    pub last_active: i64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkspaceWindowKind {
    Terminal,
    NativeTab,
    ConfigEditor,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum WorkspaceAction {
    CreateNeoismNote,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClipboardPayload {
    pub mime_type: String,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub bytes: Vec<u8>,
    #[serde(default)]
    pub filename: Option<String>,
}

/// Renderable, safe-to-leak summary of one accepted pairing token.
///
/// **Security:** the wire shape deliberately omits the raw token (and
/// any private key material). The only identifier the daemon shares
/// with a connected client is a short SHA-256 `fingerprint_prefix`
/// (currently 12 hex chars). Operators revoke a row by sending the
/// same prefix back via [`WorkspaceClientMessage::RevokePairing`].
///
/// `device_label` is the `client_name` the device sent in its first
/// successful `Hello`; it's free-form and never participates in the
/// auth decision — treat it as a UI hint only.
///
/// `last_seen` is the unix-seconds timestamp of the most recent
/// accepted handshake using this token, or `None` if the token has
/// never been used (just minted). `created_at` is the unix-seconds
/// timestamp the token was minted, or `0` for legacy tokens loaded
/// from the pre-F2 plain-text store.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PairingSummary {
    #[serde(default)]
    pub device_label: Option<String>,
    #[serde(default)]
    pub last_seen: Option<i64>,
    pub fingerprint_prefix: String,
    #[serde(default)]
    pub created_at: i64,
}

/// Per-workplace UI preferences persisted by the daemon.
///
/// Each field is `Option`-or-default so a partial update from an older
/// client (or a chrome that doesn't manage a sidebar) round-trips
/// cleanly without losing the fields it didn't touch. The daemon
/// persists this alongside `ProjectRootSummary` in
/// `~/.local/share/neoism/workspaces.json` — see the F3 task notes for
/// the on-disk schema rationale (single registry file, no new
/// top-level config).
///
/// `session_tree` is intentionally an opaque JSON string instead of a
/// typed `SessionTreeSnapshot`: the canonical session-layout type
/// lives in the `neoism-ui` (shared) crate and `neoism-protocol`
/// cannot take a dependency on it without inverting the layering. The
/// daemon treats the field as a blob (write-back of whatever the
/// frontend serialised) so any newer shape lands without protocol
/// changes.
//
// `Eq` is intentionally omitted because [`WorkplacePreferences::font_size`]
// carries an `f32`; `PartialEq` is plenty for the roundtrip tests below.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct WorkplacePreferences {
    /// Theme name the chrome should activate when this workplace
    /// becomes focused. `None` means "inherit the global default" —
    /// callers should not invent a string just to satisfy the type.
    #[serde(default)]
    pub theme: Option<String>,
    /// Editor / chrome font size in CSS pixels (web) or logical
    /// points (desktop). `None` leaves the value to the host's own
    /// default.
    #[serde(default)]
    pub font_size: Option<f32>,
    /// Map of sidebar identifier (e.g. `"file_tree"`, `"diagnostics"`)
    /// → pixel width. The daemon never inspects the keys; the chrome
    /// owns the namespace.
    #[serde(default)]
    pub sidebar_widths: HashMap<String, f32>,
    /// Opaque JSON-serialised session-layout snapshot. Persisted
    /// verbatim by the daemon and handed back on next
    /// `GetWorkplacePreferences`. `None` means "no snapshot recorded
    /// yet" — fresh workplaces start without one until the chrome
    /// emits its first `SetWorkplacePreferences`.
    #[serde(default)]
    pub session_tree: Option<String>,
}
