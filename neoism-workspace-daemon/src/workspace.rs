//! Daemon-side handler for [`WorkspaceClientMessage`].
//!
//! Phase 11: holds the daemon's authoritative workspace registry and
//! the per-connection session state. The registry is persisted to
//! `~/.local/share/neoism/workspaces.json` so workspaces survive
//! daemon restarts; sessions are in-memory only (a session's PTY /
//! nvim attach lives elsewhere in the daemon, and rebinding them on
//! restart is a separate problem).
//!
//! Per-connection state is held by [`ConnectionWorkspace`]: the active
//! workspace id and active session id for *this* WebSocket. The shared
//! [`WorkspaceManager`] holds the cross-connection registry behind a
//! mutex.
//!
//! ## Workspace-session id vs PTY-session id
//!
//! A workspace [`SessionSummary::id`] is a *logical tab* in the
//! registry: it survives daemon restarts (it is persisted in the
//! snapshot) and carries the recorded `cwd`/`label`. A PTY session id —
//! minted in [`crate::sessions::SessionRegistry`] — names a *live*
//! shell process; it is ephemeral and dies with the process or the host
//! move. These were historically independent UUIDs with no link, so a
//! roaming client could not tell which live PTY backed a given tab.
//!
//! [`ManagerInner::session_pty_links`] closes that gap: it is a side
//! table mapping `workspace_session_id -> pty_session_id`, maintained
//! when a tab is bridged onto a live PTY (see
//! [`WorkspaceManager::link_pty_session`]) and cleared when either side
//! goes away. `SessionRegistry` lives next to this manager on
//! `AppState`, so a caller holding both can resolve a tab to its real
//! PTY via [`WorkspaceManager::pty_session_for`].
//!
//! ## Respawn-in-cwd policy (locked decision #3)
//!
//! PTYs never migrate. On a host move (or a `SetCwd` that changes a
//! tab's directory) the logical workspace session is preserved and a
//! fresh shell is respawned in the recorded `cwd`; agents resume from
//! serialized state. So `SetCwd` only updates the durable registry +
//! link bookkeeping here — the actual respawn is performed by whoever
//! owns the live `SessionRegistry` (the `/session` PTY socket task),
//! using the recorded `cwd`. See `docs/daemon-session-model.md`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::SystemTime;

use neoism_protocol::workspace::{
    ClipboardPayload, EditorSurfaceSummary, HostSummary, InitialWorkspaceReason,
    PaneLayoutOp, PaneLayoutSnapshot, ProjectRootSummary, SessionSummary,
    WorkplacePreferences, WorkspaceAction, WorkspaceClientMessage,
    WorkspaceServerMessage, WorkspaceSummary, WorkspaceTabSummary, WorkspaceWindowKind,
    WorkspaceWindowSummary,
};
use neoism_workspace_index::notes::WorkspaceNoteIndex;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::handshake::{self, HandshakeOutcome, PairingTokenStore};
use crate::persistence::{self, Snapshot, SnapshotProducer, SnapshotWriter};

mod clipboard;
mod dispatch;
mod handlers;
mod manager;
mod shell_ops;

#[cfg(test)]
mod tests;

// Re-export the moved public API so external paths
// (`crate::workspace::Foo`, `neoism_workspace_daemon::workspace::foo`)
// keep resolving after the god-file was split into child modules.
pub use clipboard::{
    clipboard_dir, resolve_clipboard_image_path, sweep_clipboard_dir_on_startup,
};
pub use dispatch::{handle, handle_preauthenticated};
pub(crate) use shell_ops::{
    cleanup_local_docker_sandbox, export_workspace_snapshot, start_local_docker_sandbox,
};

/// Buffer depth for the per-workplace preferences broadcast channel.
/// We never expect more than a handful of preferences mutations per
/// second (theme/font-size toggles, sidebar resize end events) and the
/// receiver side is a websocket task that drains as fast as it can, so
/// 64 leaves comfortable slack for transient bursts without pegging
/// memory. A `RecvError::Lagged` on the consumer side is benign — the
/// client either already re-fetches via `GetWorkplacePreferences` or
/// can wait for the next `WorkplacePreferencesChanged` to converge.
const PREFERENCES_BROADCAST_CAPACITY: usize = 64;

/// Buffer depth for the pane-layout broadcast channel. Pane mutations
/// can arrive in tight bursts (a phone client driving rapid focus
/// nudges, a desktop user dragging a split divider with the keyboard)
/// so we size the buffer a touch higher than the preferences channel.
/// Same `RecvError::Lagged` story applies: a lagged receiver just
/// re-syncs on its next inbound frame.
const PANE_LAYOUT_BROADCAST_CAPACITY: usize = 256;

/// Buffer depth for the tree-changed broadcast channel. Fired on every
/// host-snapshot / workspace-tabs publish; the payload is just an
/// origin marker (receivers rebuild the tree from the manager at send
/// time), so lagging only costs a redundant refresh.
const TREE_BROADCAST_CAPACITY: usize = 64;

/// 8D-live: notification that the HOST>WORKSPACE>TABS tree changed.
/// Carries only the ORIGIN connection (which skips the push — it
/// published the change and re-delivering it would loop its own
/// publish); every other websocket responds by sending the client a
/// fresh `HostWorkspaceTree`.
#[derive(Clone, Copy, Debug)]
pub struct TreeChangedBroadcast {
    pub origin: Option<uuid::Uuid>,
}

/// On-disk shape of the persisted workspaces registry. F3 extends the
/// schema in place with a sibling `preferences` map keyed by workspace
/// id so the daemon stays a single-file store (no new top-level
/// config). Older daemons that wrote this file without the `preferences`
/// field still parse cleanly via the `#[serde(default)]`.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct PersistedRegistry {
    #[serde(default)]
    workspaces: Vec<ProjectRootSummary>,
    /// Per-workplace UI preferences (theme, font size, sidebar
    /// widths, last session-layout snapshot). Keyed by
    /// `ProjectRootSummary::id`. The map is kept on the same JSON blob
    /// as `workspaces` per F3 — a separate file would bring its own
    /// migration / locking story for zero benefit.
    #[serde(default)]
    preferences: HashMap<String, WorkplacePreferences>,
}

/// Shared state for the workspace subsystem. One instance lives on
/// `AppState` and is cloned into every WebSocket connection handler.
#[derive(Clone)]
pub struct WorkspaceManager {
    inner: Arc<Mutex<ManagerInner>>,
    persistence_path: PathBuf,
    /// Broadcast bus for per-workplace preferences updates. Each
    /// connected websocket subscribes via
    /// [`WorkspaceManager::subscribe_preferences`] and forwards every
    /// event onto its sink, so a `SetWorkplacePreferences` from one
    /// client reaches every paired surface without a separate poll.
    /// Wrapped in `Arc` so the `Clone` impl on `WorkspaceManager`
    /// stays cheap (we deliberately do not derive `Clone` for the
    /// `Sender` directly — sharing the same allocation keeps every
    /// clone routing onto the same channel).
    preferences_tx: Arc<tokio::sync::broadcast::Sender<PreferencesBroadcast>>,
    /// Broadcast bus for pane-layout mutations. The dispatcher fans an
    /// accepted `PaneLayoutOp` to every connected websocket so a phone
    /// client driving the laptop (or two laptops sharing a workspace)
    /// converges on the same pane tree without polling. Mirrors the
    /// `preferences_tx` plumbing in shape so the websocket task can
    /// drain both in the same `select!` loop.
    pane_layout_tx: Arc<tokio::sync::broadcast::Sender<PaneLayoutBroadcast>>,
    /// 8D-live: tree-changed bus. Same shape as the two above so the
    /// websocket task drains it in the same `select!` loop.
    tree_tx: Arc<tokio::sync::broadcast::Sender<TreeChangedBroadcast>>,
    /// G1 snapshot writer. Pinged on every mutation; debounces writes
    /// to `$STATE_DIR/state.json`. Defaults to
    /// [`SnapshotWriter::ephemeral`] for tests + the legacy `bootstrap`
    /// path; the daemon binary swaps in a persistent writer via
    /// [`WorkspaceManager::install_snapshot_writer`] before serving any
    /// traffic.
    snapshot_writer: SnapshotWriter,
}

/// Payload carried on [`WorkspaceManager::preferences_tx`]. The
/// daemon converts this into a [`WorkspaceServerMessage::WorkplacePreferencesChanged`]
/// on each receiver side.
#[derive(Debug, Clone)]
pub struct PreferencesBroadcast {
    pub workspace_id: String,
    pub prefs: WorkplacePreferences,
}

/// Payload carried on [`WorkspaceManager::pane_layout_tx`]. The daemon
/// converts this into a [`WorkspaceServerMessage::PaneLayoutChanged`]
/// on each receiver side. `new_layout_snapshot` carries the serialized
/// canonical pane tree after the accepted mutation.
#[derive(Debug, Clone)]
pub struct PaneLayoutBroadcast {
    pub pane_external_id: u64,
    pub op: PaneLayoutOp,
    pub new_layout_snapshot: Option<String>,
}

struct ManagerInner {
    hosts: HashMap<String, HostSummary>,
    host_workspaces: HashMap<String, WorkspaceSummary>,
    workspace_tabs: HashMap<String, WorkspaceTabSummary>,
    active_workspace_by_host: HashMap<String, String>,
    workspaces: HashMap<String, ProjectRootSummary>,
    sessions: HashMap<String, SessionSummary>,
    editor_surfaces: HashMap<String, EditorSurfaceSummary>,
    pane_layouts: HashMap<String, PaneLayoutSnapshot>,
    windows: HashMap<String, WorkspaceWindowSummary>,
    /// Persisted per-workplace UI preferences (theme, font size,
    /// sidebar widths, last session-layout snapshot). Mirrors
    /// [`PersistedRegistry::preferences`] one-to-one.
    preferences: HashMap<String, WorkplacePreferences>,
    /// Best-effort in-memory resume table keyed by the stable
    /// client_id introduced in G2. This lets a reconnecting client
    /// recover its active workspace/session pointer before asking for a
    /// full snapshot. It is intentionally not persisted yet; G1 owns the
    /// durable daemon snapshot shape and currently does not include
    /// per-client ephemeral cursors.
    client_states: HashMap<Uuid, ClientResumeState>,
    /// Link table from a workspace-registry session id (a logical tab)
    /// to the live PTY session id that currently backs it in
    /// [`crate::sessions::SessionRegistry`]. Maintained by
    /// [`WorkspaceManager::link_pty_session`] when a tab is bridged onto
    /// a real shell, and cleared when either side closes. Intentionally
    /// *not* persisted: PTY ids are ephemeral and a restart respawns the
    /// shell in the recorded `cwd` (respawn-in-cwd policy), at which
    /// point a fresh link is recorded.
    session_pty_links: HashMap<String, String>,
    /// Monotonic per-daemon counter handed out by
    /// `bind_editor_surface` so each editor surface gets its own
    /// stable `route_id`. Starts at `1` so the chrome's legacy
    /// `ACTIVE_EDITOR_ROUTE_ID = 1` slot is the first allocation —
    /// keeps single-pane web clients identical to their pre-routing
    /// behaviour and avoids gaps the older daemon never produced.
    next_route_id: u64,
}

/// One host = one MACHINE. The daemon and a desktop running on the
/// same machine resolve the same id (env override, else the real
/// hostname), so every picker shows a single group per machine instead
/// of an artificial desktop/local split.
pub fn machine_host_id() -> String {
    std::env::var("NEOISM_HOST_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(machine_host_label)
}

/// Human label for this machine: env override, else the hostname.
pub fn machine_host_label() -> String {
    std::env::var("NEOISM_HOST_LABEL")
        .ok()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| {
            let host = gethostname::gethostname().to_string_lossy().into_owned();
            if host.trim().is_empty() {
                "local".to_string()
            } else {
                host
            }
        })
}

fn bootstrap_hosts() -> HashMap<String, HostSummary> {
    let host_id = machine_host_id();
    let label = machine_host_label();
    // Canonical dialable endpoint for this host, set by the operator
    // (e.g. `scripts/neoism-daemon.sh` exports the tailnet URL). Lets
    // remote clients resolve this host_id to a reachable URL for re-dial.
    let daemon_url = std::env::var("NEOISM_HOST_URL")
        .ok()
        .filter(|url| !url.trim().is_empty());
    let mut hosts = HashMap::new();
    hosts.insert(
        host_id.clone(),
        HostSummary {
            id: host_id,
            label,
            online: true,
            peer_identity: None,
            last_seen: now_secs(),
            daemon_url,
            active_workspace_id: None,
        },
    );
    hosts
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ClientResumeState {
    active_workspace: Option<String>,
    active_session: Option<String>,
}

/// This daemon's own host id in the HOST>WORKSPACE>TABS tree — the same id
/// [`bootstrap_hosts`] seeds (`NEOISM_HOST_ID`, defaulting to `"local"`).
/// The receive/adopt route registers an incoming workspace under this host.
pub fn local_host_id() -> String {
    machine_host_id()
}

/// Per-WebSocket workspace + session pointer. Carries the connection's
/// "current" workspace and session ids; mutated by `SwitchProjectRoot`,
/// `SwitchSession`, etc.
#[derive(Default)]
pub struct ConnectionWorkspace {
    pub client_id: Uuid,
    pub active_workspace: Option<String>,
    pub active_session: Option<String>,
    pub clipboard_payload: Option<ClipboardPayload>,
}

/// Outcome of a single `WorkspaceClientMessage` dispatch. Most arms
/// just yield replies; the `Hello` arm may additionally signal the
/// websocket task to drop the connection after writing the rejection
/// `HelloAck`.
#[derive(Debug, Default)]
pub struct DispatchOutcome {
    pub replies: Vec<WorkspaceServerMessage>,
    /// When `true` the websocket task should drain `replies` to the
    /// client and then close the socket. Today only set by a rejected
    /// `Hello` handshake.
    pub disconnect: bool,
}

impl DispatchOutcome {
    fn just(replies: Vec<WorkspaceServerMessage>) -> Self {
        Self {
            replies,
            disconnect: false,
        }
    }
}

fn err(message: String) -> WorkspaceServerMessage {
    WorkspaceServerMessage::Error { message }
}

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Resolve a workspace's declared directory and ensure it exists on disk.
/// A workspace IS a directory, so this never returns an empty path: an
/// explicit dir is created (`mkdir -p`) and canonicalized; anything
/// missing/empty falls back to the daemon's default workspace root.
fn declare_workspace_dir(root_dir: Option<PathBuf>) -> PathBuf {
    let dir = root_dir
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(crate::files::workspace_root);
    let _ = std::fs::create_dir_all(&dir);
    std::fs::canonicalize(&dir).unwrap_or(dir)
}

/// Fill an empty/None `root_dir` with the daemon's default so a workspace
/// summary never reaches a client without a directory to root at. Used on
/// read paths to normalize legacy records.
fn ensure_workspace_dir(mut workspace: WorkspaceSummary) -> WorkspaceSummary {
    let missing = workspace
        .root_dir
        .as_ref()
        .map(|p| p.as_os_str().is_empty())
        .unwrap_or(true);
    if missing {
        workspace.root_dir = Some(crate::files::workspace_root());
    }
    workspace
}

fn persistence_path() -> PathBuf {
    if let Ok(p) = std::env::var("NEOISM_WORKSPACE_REGISTRY") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    dirs::data_dir()
        .map(|d| d.join("neoism").join("workspaces.json"))
        .unwrap_or_else(|| PathBuf::from("workspaces.json"))
}

#[cfg(test)]
fn load_registry(path: &Path) -> Vec<ProjectRootSummary> {
    load_registry_full(path).workspaces
}

fn load_registry_full(path: &Path) -> PersistedRegistry {
    match std::fs::read_to_string(path) {
        Ok(s) => match serde_json::from_str::<PersistedRegistry>(&s) {
            Ok(r) => r,
            Err(e) => {
                tracing::warn!(
                    error = %e,
                    path = %path.display(),
                    "failed to parse workspace registry; starting empty"
                );
                PersistedRegistry::default()
            }
        },
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            PersistedRegistry::default()
        }
        Err(e) => {
            tracing::warn!(
                error = %e,
                path = %path.display(),
                "failed to read workspace registry; starting empty"
            );
            PersistedRegistry::default()
        }
    }
}

fn save_registry(path: &Path, snapshot: &PersistedRegistry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let json = serde_json::to_string_pretty(snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    std::fs::write(path, json)
}
