//! G1: persistent-state snapshotter for the workspace daemon.
//!
//! Goals (per `TASKS.md::G1`):
//!
//! * Snapshot *all* live `WorkspaceManager` state (workspaces, sessions,
//!   editor surfaces, per-workplace preferences, and the next route id
//!   counter) to `$STATE_DIR/state.json` on every mutation, with the
//!   writes coalesced behind a **200ms debounce** so a burst of pane
//!   ops doesn't thrash the disk.
//! * Snapshot again, *synchronously*, on graceful shutdown (`SIGTERM`)
//!   so we don't lose the last few hundred milliseconds of in-flight
//!   mutations.
//! * On startup, load the snapshot iff it exists **and** is younger
//!   than 24h — older snapshots are treated as stale and discarded so
//!   a daemon that's been off for a week doesn't resurrect a long-dead
//!   pane tree.
//! * Provide an `--ephemeral` knob that skips persistence entirely (for
//!   tests + cloud one-shot containers).
//!
//! Wire-up shape: the [`SnapshotWriter`] owns a tokio task and an
//! `mpsc::UnboundedSender<()>` that callers ping after every mutation.
//! The task drains all pending pings, sleeps 200ms (the debounce
//! window), then takes a fresh snapshot via a closure the
//! [`WorkspaceManager`] supplies at construction time and writes it to
//! disk atomically (tmp-file + rename). On `shutdown()` the writer
//! awaits a final flush before the daemon exits.
//!
//! All file IO goes through [`save_snapshot`] / [`load_snapshot`] so
//! tests can drive the round-trip without spinning up the async task.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use neoism_protocol::workspace::{
    EditorSurfaceSummary, HostSummary, PaneLayoutSnapshot, ProjectRootSummary,
    SessionSummary, WorkplacePreferences, WorkspaceSummary, WorkspaceTabSummary,
    WorkspaceWindowSummary,
};
use serde::{Deserialize, Serialize};
use tokio::sync::{mpsc, oneshot, Mutex as AsyncMutex};

/// Debounce window. A burst of N mutations within this window collapses
/// to a single disk write `DEBOUNCE_MS` after the *last* ping.
pub const DEBOUNCE_MS: u64 = 200;

/// Snapshot age beyond which we discard the on-disk state and start
/// fresh. 24h matches the spec; an idle laptop overnight survives, a
/// daemon that's been off for days does not.
pub const MAX_SNAPSHOT_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// On-disk file name within `state_dir`.
pub const SNAPSHOT_FILE: &str = "state.json";

/// Subdirectory under `$XDG_STATE_HOME` (or `~/.local/state`) that
/// holds the daemon's runtime state.
pub const STATE_SUBDIR: &str = "neoism";

/// On-disk shape of the daemon's persistent state. Every field is
/// `#[serde(default)]` so a snapshot written by an older daemon
/// (missing fields the current daemon expects) still parses cleanly.
///
/// `saved_at_unix_secs` is the wall-clock time the snapshot was
/// written; consulted by [`load_snapshot`] to drop snapshots older
/// than [`MAX_SNAPSHOT_AGE`].
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub saved_at_unix_secs: i64,
    #[serde(default)]
    pub hosts: Vec<HostSummary>,
    #[serde(default)]
    pub host_workspaces: Vec<WorkspaceSummary>,
    #[serde(default)]
    pub workspace_tabs: Vec<WorkspaceTabSummary>,
    #[serde(default)]
    pub active_workspace_by_host: HashMap<String, String>,
    #[serde(default)]
    pub workspaces: Vec<ProjectRootSummary>,
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
    #[serde(default)]
    pub editor_surfaces: Vec<EditorSurfaceSummary>,
    #[serde(default)]
    pub pane_layouts: Vec<PaneLayoutSnapshot>,
    #[serde(default)]
    pub windows: Vec<WorkspaceWindowSummary>,
    #[serde(default)]
    pub preferences: HashMap<String, WorkplacePreferences>,
    #[serde(default = "default_next_route_id")]
    pub next_route_id: u64,
}

fn default_next_route_id() -> u64 {
    1
}

impl Snapshot {
    /// Stamp the wall-clock save time and bump the schema version so
    /// future migrations can branch on it.
    pub fn stamped_now(mut self) -> Self {
        self.version = 1;
        self.saved_at_unix_secs = now_unix_secs();
        self
    }
}

/// Pick the state directory. Resolution order:
///
/// 1. Explicit override passed in from `--state-dir`.
/// 2. `$NEOISM_STATE_DIR` (test override).
/// 3. `$XDG_STATE_HOME/neoism/` if `XDG_STATE_HOME` is set.
/// 4. `~/.local/state/neoism/` as the spec'd default fallback.
/// 5. On Windows (no `$HOME`): `dirs::state_dir()` then
///    `dirs::data_local_dir()` (`%LOCALAPPDATA%\neoism\`).
/// 6. `./.neoism-state/` if all of the above fail.
pub fn resolve_state_dir(explicit: Option<&Path>) -> PathBuf {
    if let Some(p) = explicit {
        return p.to_path_buf();
    }
    if let Ok(p) = std::env::var("NEOISM_STATE_DIR") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    if let Ok(xdg) = std::env::var("XDG_STATE_HOME") {
        if !xdg.is_empty() {
            return PathBuf::from(xdg).join(STATE_SUBDIR);
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join(STATE_SUBDIR);
        }
    }
    // Windows-only so a unix host without $HOME keeps the historical
    // `./.neoism-state` behavior.
    #[cfg(windows)]
    if let Some(dir) = dirs::state_dir().or_else(dirs::data_local_dir) {
        return dir.join(STATE_SUBDIR);
    }
    PathBuf::from(".neoism-state")
}

/// Write a snapshot to `dir/state.json` atomically (tmp-file + rename).
/// Creates `dir` if needed. Stamps `saved_at_unix_secs` to "now".
pub fn save_snapshot(dir: &Path, snapshot: Snapshot) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let snapshot = snapshot.stamped_now();
    let payload = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    let final_path = dir.join(SNAPSHOT_FILE);
    let tmp_path = dir.join(format!("{SNAPSHOT_FILE}.tmp"));
    std::fs::write(&tmp_path, payload)?;
    std::fs::rename(&tmp_path, &final_path)?;
    Ok(())
}

/// Load `dir/state.json` if present AND younger than
/// [`MAX_SNAPSHOT_AGE`]. Returns:
///
/// * `Ok(Some(snapshot))` — file present, parsed, and fresh.
/// * `Ok(None)` — file missing, unreadable, malformed, or stale.
/// * `Err(_)` — only for IO errors *other than* `NotFound`; today the
///   only realistic error is a permissions denial that the caller
///   probably wants to log and continue from.
///
/// Callers should treat `Ok(None)` as "start with a blank slate".
pub fn load_snapshot(dir: &Path) -> std::io::Result<Option<Snapshot>> {
    let path = dir.join(SNAPSHOT_FILE);
    let raw = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let snapshot: Snapshot = match serde_json::from_str(&raw) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                error = %err,
                path = %path.display(),
                "could not parse daemon snapshot; starting empty"
            );
            return Ok(None);
        }
    };
    let now = now_unix_secs();
    let age = now.saturating_sub(snapshot.saved_at_unix_secs);
    if age < 0 || age as u64 > MAX_SNAPSHOT_AGE.as_secs() {
        tracing::info!(
            age_secs = age,
            max_age_secs = MAX_SNAPSHOT_AGE.as_secs(),
            "discarding stale daemon snapshot (older than max age)"
        );
        return Ok(None);
    }
    Ok(Some(snapshot))
}

/// Mode for the [`SnapshotWriter`]. `Ephemeral` is the test / cloud
/// path: every `notify_changed` and `flush_blocking` becomes a no-op
/// so we never touch the filesystem.
#[derive(Debug, Clone)]
pub enum PersistenceMode {
    Persistent { state_dir: PathBuf },
    Ephemeral,
}

/// Closure type for "give me the current state". The
/// [`WorkspaceManager`] supplies this at writer construction so the
/// async flush task can sample fresh state without re-borrowing the
/// manager itself.
pub type SnapshotProducer = Arc<dyn Fn() -> Snapshot + Send + Sync>;

/// Handle handed to the [`WorkspaceManager`] for the lifetime of the
/// daemon. Cloning is cheap (it's just an `mpsc::Sender` wrap) so the
/// manager can keep one inside its `Arc<Mutex<...>>` without bloating
/// the lock surface.
#[derive(Clone)]
pub struct SnapshotWriter {
    inner: Arc<SnapshotWriterInner>,
}

struct SnapshotWriterInner {
    mode: PersistenceMode,
    /// `None` in `Ephemeral` mode. Each ping schedules a debounced
    /// flush; we use unbounded because pings are tiny and the tail
    /// coalesces anyway, so backpressure here would just deadlock
    /// mutation paths.
    tx: Option<mpsc::UnboundedSender<()>>,
    /// Held by the background flush task. We keep a handle so
    /// [`SnapshotWriter::shutdown`] can `await` final-flush completion.
    /// Wrapped in `AsyncMutex` so multiple concurrent `shutdown` calls
    /// (which shouldn't happen in production, but tests are tests)
    /// serialise rather than racing the close.
    shutdown_tx: AsyncMutex<Option<oneshot::Sender<oneshot::Sender<()>>>>,
    /// Held alongside `shutdown_tx`: closure the *shutdown path* uses
    /// to take a final, synchronous snapshot. Stored as `Arc` for
    /// cheap cloning into the background task.
    producer: SnapshotProducer,
}

impl SnapshotWriter {
    /// Construct an ephemeral writer (every operation is a no-op).
    /// Used by `--ephemeral` and by the unit tests.
    pub fn ephemeral() -> Self {
        Self {
            inner: Arc::new(SnapshotWriterInner {
                mode: PersistenceMode::Ephemeral,
                tx: None,
                shutdown_tx: AsyncMutex::new(None),
                producer: Arc::new(|| Snapshot::default()),
            }),
        }
    }

    /// Construct a persistent writer that flushes to
    /// `state_dir/state.json` on a 200ms debounce. The `producer`
    /// closure is called inside the flush task to sample fresh state
    /// off the manager — it must be cheap and lock-respecting (the
    /// manager's `Mutex` strategy already serialises this).
    ///
    /// `producer` is *not* called inside the writer's own lock, so a
    /// producer that briefly acquires the manager's lock is safe.
    pub fn spawn_persistent(state_dir: PathBuf, producer: SnapshotProducer) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel::<()>();
        let (shutdown_tx, mut shutdown_rx) = oneshot::channel::<oneshot::Sender<()>>();

        let task_state_dir = state_dir.clone();
        let task_producer = Arc::clone(&producer);
        tokio::spawn(async move {
            let mut pending = false;
            loop {
                tokio::select! {
                    biased;
                    // Shutdown: always write one final snapshot, even
                    // if the dirty ping has not been observed by this
                    // task yet. `shutdown()` is the graceful-exit
                    // boundary, so disk should reflect memory at that
                    // point regardless of debounce state.
                    sig = &mut shutdown_rx => {
                        flush(&task_state_dir, &task_producer);
                        if let Ok(ack) = sig {
                            let _ = ack.send(());
                        }
                        break;
                    }
                    maybe_ping = rx.recv() => {
                        match maybe_ping {
                            Some(()) => pending = true,
                            None => {
                                // All senders dropped without an
                                // explicit shutdown. Flush once, then
                                // exit so we don't lose the tail.
                                if pending {
                                    flush(&task_state_dir, &task_producer);
                                }
                                break;
                            }
                        }
                    }
                }

                if !pending {
                    continue;
                }

                // Drain any pings that piled up while we were dispatching,
                // then wait the debounce window. Any *new* ping during
                // the sleep wins (we redo the drain).
                loop {
                    tokio::select! {
                        biased;
                        sig = &mut shutdown_rx => {
                            flush(&task_state_dir, &task_producer);
                            if let Ok(ack) = sig {
                                let _ = ack.send(());
                            }
                            return;
                        }
                        _ = tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS)) => {
                            // Drain anything that arrived during the sleep
                            // so we coalesce the bursty tail.
                            while let Ok(()) = rx.try_recv() {}
                            flush(&task_state_dir, &task_producer);
                            pending = false;
                            break;
                        }
                        maybe_ping = rx.recv() => {
                            match maybe_ping {
                                Some(()) => {
                                    // New ping in-window — restart the
                                    // debounce by re-entering the loop.
                                    continue;
                                }
                                None => {
                                    flush(&task_state_dir, &task_producer);
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        });

        Self {
            inner: Arc::new(SnapshotWriterInner {
                mode: PersistenceMode::Persistent { state_dir },
                tx: Some(tx),
                shutdown_tx: AsyncMutex::new(Some(shutdown_tx)),
                producer,
            }),
        }
    }

    /// Mode inspector (mainly for diagnostics / logging).
    pub fn mode(&self) -> &PersistenceMode {
        &self.inner.mode
    }

    /// True iff this writer will actually touch disk.
    pub fn is_persistent(&self) -> bool {
        matches!(self.inner.mode, PersistenceMode::Persistent { .. })
    }

    /// Ping the flush task. Cheap; intended to be called from inside
    /// every `WorkspaceManager` mutation method. On `Ephemeral` it's a
    /// no-op. If the background task has died (shouldn't happen in
    /// production) we silently drop the ping — losing a snapshot is
    /// better than crashing the daemon under load.
    pub fn notify_changed(&self) {
        if let Some(tx) = &self.inner.tx {
            let _ = tx.send(());
        }
    }

    /// Synchronously write the latest snapshot. Called from the
    /// shutdown path so we don't lose the tail of in-flight writes.
    /// On `Ephemeral` it's a no-op.
    ///
    /// Safe to call from any thread; takes no async lock.
    pub fn flush_blocking(&self) {
        if let PersistenceMode::Persistent { state_dir } = &self.inner.mode {
            flush(state_dir, &self.inner.producer);
        }
    }

    /// Tell the background task to do its final flush and exit. Awaits
    /// the task's ack so callers know the snapshot has hit disk before
    /// the daemon exits. Idempotent — second call returns immediately.
    pub async fn shutdown(&self) {
        let sender = {
            let mut guard = self.inner.shutdown_tx.lock().await;
            guard.take()
        };
        if let Some(sender) = sender {
            let (ack_tx, ack_rx) = oneshot::channel();
            // If the task already exited (e.g. all `tx` dropped) we
            // fall back to a sync flush so we don't lose state on the
            // shutdown path.
            if sender.send(ack_tx).is_err() {
                self.flush_blocking();
                return;
            }
            if ack_rx.await.is_err() {
                self.flush_blocking();
            }
        } else {
            // Shutdown already invoked; still flush synchronously as a
            // belt-and-braces guarantee that disk reflects memory.
            self.flush_blocking();
        }
    }
}

fn flush(state_dir: &Path, producer: &SnapshotProducer) {
    let snapshot = producer();
    if let Err(e) = save_snapshot(state_dir, snapshot) {
        tracing::warn!(
            error = %e,
            path = %state_dir.display(),
            "snapshot write failed"
        );
    } else {
        tracing::debug!(path = %state_dir.display(), "snapshot written");
    }
}

fn now_unix_secs() -> i64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------
// Pidfile guard
// ---------------------------------------------------------------------

/// RAII guard that removes the pidfile when dropped. Created by
/// [`PidfileGuard::create`] which writes the current pid to the path,
/// creating parent dirs as needed. On drop the path is `remove_file`d
/// best-effort (errors logged, not propagated — a stale pidfile is
/// vastly preferable to a panicked shutdown).
pub struct PidfileGuard {
    path: PathBuf,
}

impl PidfileGuard {
    pub fn create(path: PathBuf) -> std::io::Result<Self> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
        let pid = std::process::id();
        std::fs::write(&path, format!("{pid}\n"))?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for PidfileGuard {
    fn drop(&mut self) {
        if let Err(e) = std::fs::remove_file(&self.path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                tracing::warn!(
                    error = %e,
                    path = %self.path.display(),
                    "failed to remove pidfile on shutdown"
                );
            }
        }
    }
}

// ---------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::workspace::WorkspaceWindowKind;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;
    use tempfile::TempDir;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    fn sample_snapshot() -> Snapshot {
        let mut prefs = HashMap::new();
        prefs.insert(
            "ws-1".to_string(),
            WorkplacePreferences {
                theme: Some("noctis".into()),
                font_size: Some(13.0),
                sidebar_widths: Default::default(),
                session_tree: Some("{\"v\":1}".into()),
            },
        );
        Snapshot {
            version: 1,
            saved_at_unix_secs: 0, // overwritten by save
            hosts: Default::default(),
            host_workspaces: Default::default(),
            workspace_tabs: Default::default(),
            active_workspace_by_host: Default::default(),
            workspaces: vec![ProjectRootSummary {
                id: "ws-1".into(),
                name: "Workspace 1".into(),
                path: PathBuf::from("/tmp/ws-1"),
                last_opened: 0,
            }],
            sessions: vec![SessionSummary {
                id: "sess-1".into(),
                workspace_id: "ws-1".into(),
                cwd: "src".into(),
                label: Some("editor".into()),
                last_active: 0,
            }],
            editor_surfaces: vec![EditorSurfaceSummary {
                surface_id: "5".into(),
                workspace_id: "ws-1".into(),
                session_id: "sess-1".into(),
                path: Some(PathBuf::from("src/main.rs")),
                route_id: Some(3),
                last_active: 0,
            }],
            pane_layouts: Vec::new(),
            windows: vec![WorkspaceWindowSummary {
                id: "win-1".into(),
                kind: WorkspaceWindowKind::Terminal,
                workspace_id: Some("ws-1".into()),
                parent_window_id: None,
                title: "Neoism".into(),
                route_id: Some(3),
                created_at: 0,
                last_active: 0,
            }],
            preferences: prefs,
            next_route_id: 7,
        }
    }

    #[test]
    fn round_trip_through_disk_preserves_state() {
        let td = TempDir::new().unwrap();
        let snap = sample_snapshot();
        save_snapshot(td.path(), snap.clone()).unwrap();
        let loaded = load_snapshot(td.path()).unwrap().expect("present");
        // Ignore the stamped time difference.
        let mut expected = snap;
        expected.saved_at_unix_secs = loaded.saved_at_unix_secs;
        expected.version = loaded.version;
        assert_eq!(loaded, expected);
    }

    #[test]
    fn load_snapshot_returns_none_for_missing_file() {
        let td = TempDir::new().unwrap();
        assert!(load_snapshot(td.path()).unwrap().is_none());
    }

    #[test]
    fn load_snapshot_discards_stale_snapshot() {
        let td = TempDir::new().unwrap();
        // Hand-craft a snapshot that is older than MAX_SNAPSHOT_AGE.
        let mut snap = sample_snapshot();
        snap.version = 1;
        snap.saved_at_unix_secs =
            now_unix_secs() - MAX_SNAPSHOT_AGE.as_secs() as i64 - 60; // an extra minute past the cliff
        let path = td.path().join(SNAPSHOT_FILE);
        std::fs::write(&path, serde_json::to_string(&snap).unwrap()).unwrap();
        // Bypass `save_snapshot`'s stamp by writing directly above.
        assert!(load_snapshot(td.path()).unwrap().is_none());
    }

    #[test]
    fn load_snapshot_treats_garbage_as_blank() {
        let td = TempDir::new().unwrap();
        std::fs::write(td.path().join(SNAPSHOT_FILE), "{{ not json").unwrap();
        assert!(load_snapshot(td.path()).unwrap().is_none());
    }

    #[test]
    fn ephemeral_writer_never_touches_disk() {
        let td = TempDir::new().unwrap();
        let writer = SnapshotWriter::ephemeral();
        writer.notify_changed();
        writer.flush_blocking();
        // No file should exist.
        assert!(!td.path().join(SNAPSHOT_FILE).exists());
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn debounce_collapses_burst_into_one_write() {
        let td = TempDir::new().unwrap();
        let state_dir = td.path().to_path_buf();
        let write_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&write_count);
        let producer: SnapshotProducer = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            sample_snapshot()
        });
        let writer = SnapshotWriter::spawn_persistent(state_dir.clone(), producer);

        // Burst of 5 pings well within the debounce window.
        for _ in 0..5 {
            writer.notify_changed();
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        // Wait past the debounce window so the writer flushes.
        tokio::time::sleep(Duration::from_millis(DEBOUNCE_MS * 3)).await;
        // Ensure flush ran exactly once.
        assert_eq!(
            write_count.load(Ordering::SeqCst),
            1,
            "5-ping burst should collapse to one disk write"
        );
        assert!(state_dir.join(SNAPSHOT_FILE).exists());
        writer.shutdown().await;
    }

    #[tokio::test(flavor = "current_thread", start_paused = true)]
    async fn shutdown_flushes_pending_dirty_state() {
        let td = TempDir::new().unwrap();
        let state_dir = td.path().to_path_buf();
        let write_count = Arc::new(AtomicUsize::new(0));
        let counter = Arc::clone(&write_count);
        let producer: SnapshotProducer = Arc::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            sample_snapshot()
        });
        let writer = SnapshotWriter::spawn_persistent(state_dir.clone(), producer);

        writer.notify_changed();
        // Shut down BEFORE the debounce window elapses — the writer
        // must still flush the pending state so the snapshot reflects
        // the in-memory mutation.
        writer.shutdown().await;
        assert!(write_count.load(Ordering::SeqCst) >= 1);
        assert!(state_dir.join(SNAPSHOT_FILE).exists());
    }

    #[test]
    fn pidfile_guard_creates_and_removes() {
        let td = TempDir::new().unwrap();
        let pidpath = td.path().join("daemon.pid");
        {
            let guard = PidfileGuard::create(pidpath.clone()).unwrap();
            assert!(pidpath.exists());
            let body = std::fs::read_to_string(guard.path()).unwrap();
            let pid: u32 = body.trim().parse().unwrap();
            assert_eq!(pid, std::process::id());
        }
        // After drop the pidfile should be gone.
        assert!(
            !pidpath.exists(),
            "pidfile must be removed when guard is dropped"
        );
    }

    #[test]
    fn resolve_state_dir_honours_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Save + restore the env so we don't leak into other tests.
        let prev = std::env::var("NEOISM_STATE_DIR").ok();
        std::env::set_var("NEOISM_STATE_DIR", "/tmp/g1-state-test");
        let dir = resolve_state_dir(None);
        assert_eq!(dir, PathBuf::from("/tmp/g1-state-test"));
        match prev {
            Some(v) => std::env::set_var("NEOISM_STATE_DIR", v),
            None => std::env::remove_var("NEOISM_STATE_DIR"),
        }
    }

    #[test]
    fn resolve_state_dir_explicit_beats_xdg() {
        let _guard = ENV_LOCK.lock().unwrap();
        // Env override must be unset for this assertion.
        let prev_env = std::env::var("NEOISM_STATE_DIR").ok();
        std::env::remove_var("NEOISM_STATE_DIR");
        let prev_xdg = std::env::var("XDG_STATE_HOME").ok();
        std::env::set_var("XDG_STATE_HOME", "/tmp/xdg-state");
        let explicit = PathBuf::from("/tmp/explicit");
        assert_eq!(resolve_state_dir(Some(&explicit)), explicit);
        match prev_env {
            Some(v) => std::env::set_var("NEOISM_STATE_DIR", v),
            None => std::env::remove_var("NEOISM_STATE_DIR"),
        }
        match prev_xdg {
            Some(v) => std::env::set_var("XDG_STATE_HOME", v),
            None => std::env::remove_var("XDG_STATE_HOME"),
        }
    }

    #[test]
    fn resolve_state_dir_explicit_beats_env_override() {
        let _guard = ENV_LOCK.lock().unwrap();
        let prev_env = std::env::var("NEOISM_STATE_DIR").ok();
        std::env::set_var("NEOISM_STATE_DIR", "/tmp/from-env");
        let explicit = PathBuf::from("/tmp/from-cli");
        assert_eq!(resolve_state_dir(Some(&explicit)), explicit);
        match prev_env {
            Some(v) => std::env::set_var("NEOISM_STATE_DIR", v),
            None => std::env::remove_var("NEOISM_STATE_DIR"),
        }
    }
}
