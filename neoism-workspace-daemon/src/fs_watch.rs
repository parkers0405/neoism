//! Filesystem watch → live tree pushes for the files plane.
//!
//! A guest browsing a joined workspace lists directories through
//! `FilesClientMessage::ListDir` with a `workspace_root` override.
//! Without a watch, its tree only updates when the guest re-lists —
//! everything else in a shared workspace (shells, CRDT docs) is push
//! driven, and the tree read as frozen next to them. This hub puts a
//! recursive watcher on every files root a client has actually
//! touched and broadcasts debounced `FilesServerMessage::Changed`
//! pushes (request_id 0) that each `/session` socket forwards to its
//! client.
//!
//! Process-global (`hub()`), like the tailnet module: the daemon has
//! several `AppState` construction sites (binary, embedded, tests)
//! and the watch set is genuinely per-process.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use notify::{RecursiveMode, Watcher};
use tokio::sync::broadcast;

/// One debounced change burst under a watched root. Both paths are
/// absolute; `paths` is deduped and capped so a huge build doesn't
/// ship a megabyte of paths (clients re-list directories anyway).
#[derive(Debug, Clone)]
pub struct FsChanged {
    pub root: String,
    pub paths: Vec<String>,
}

const DEBOUNCE: Duration = Duration::from_millis(300);
const MAX_PATHS_PER_BURST: usize = 64;
const BROADCAST_CAPACITY: usize = 256;

/// Path segments whose contents never carry a tree-relevant change but
/// whose churn floods every guest. A recursive watch on a real repo
/// otherwise fires continuously: a running dev server rewrites `.next`
/// / `dist`, an install rewrites `node_modules`, and — the worst
/// offender — every `git status` the client runs re-stats `.git`
/// (index, objects, packed-refs), which the watcher reports right back
/// as a change, which makes the client run `git status` again. On the
/// live synapse repo this measured 3.3 `Changed` bursts/sec, and since
/// the client does a directory re-list + git-status refresh per burst
/// against the 300ms debounce window, a joined guest was starved to
/// ~4fps. Dropping these segments before they reach the debouncer
/// breaks the loop and mirrors the ignore set every editor's file
/// watcher already uses. `.git` is dropped wholesale: real edits touch
/// the worktree (which still fires) and that is what drives a git-badge
/// refresh — a bare branch flip with no working-tree change is the only
/// case that goes unsignalled, and the guest re-lists on focus anyway.
const IGNORED_SEGMENTS: &[&str] = &[
    ".git",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    ".next",
    ".nuxt",
    ".svelte-kit",
    ".output",
    ".vite",
    ".turbo",
    ".parcel-cache",
    ".cache",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".gradle",
    ".idea",
    "vendor",
    ".terraform",
    "coverage",
    ".nyc_output",
];

/// True when `path` lies inside (or is) an ignored directory, checked
/// against the segments *below* `root` only — an ancestor of the
/// watched root that happens to be named `build` must not disqualify
/// the whole tree, and the match is exact per-component so a file named
/// `dist.rs` is kept while `.../dist/bundle.js` is dropped.
fn is_ignored_watch_path(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    rel.components().any(|component| {
        matches!(
            component,
            std::path::Component::Normal(seg)
                if seg.to_str().is_some_and(|s| IGNORED_SEGMENTS.contains(&s))
        )
    })
}

pub struct FsWatchHub {
    tx: broadcast::Sender<FsChanged>,
    /// Root → live watcher. Keeping the watcher alive keeps the watch.
    watchers: Mutex<HashMap<PathBuf, notify::RecommendedWatcher>>,
    /// Raw (root, path) events from every watcher thread, drained by
    /// the debouncer thread.
    event_tx: std::sync::mpsc::Sender<(PathBuf, PathBuf)>,
}

pub fn hub() -> &'static FsWatchHub {
    static HUB: OnceLock<FsWatchHub> = OnceLock::new();
    HUB.get_or_init(|| {
        let (tx, _) = broadcast::channel(BROADCAST_CAPACITY);
        let (event_tx, event_rx) = std::sync::mpsc::channel::<(PathBuf, PathBuf)>();
        let broadcast_tx = tx.clone();
        // Debouncer: collect raw events per root for DEBOUNCE, then
        // flush one Changed burst per root.
        let spawned = std::thread::Builder::new()
            .name("neoism-fs-watch-debounce".into())
            .spawn(move || {
                let mut pending: HashMap<PathBuf, Vec<PathBuf>> = HashMap::new();
                let mut window_started: Option<Instant> = None;
                loop {
                    let timeout = match window_started {
                        Some(started) => DEBOUNCE
                            .checked_sub(started.elapsed())
                            .unwrap_or(Duration::ZERO),
                        None => Duration::from_secs(3600),
                    };
                    match event_rx.recv_timeout(timeout) {
                        Ok((root, path)) => {
                            let paths = pending.entry(root).or_default();
                            if paths.len() < MAX_PATHS_PER_BURST && !paths.contains(&path)
                            {
                                paths.push(path);
                            }
                            window_started.get_or_insert_with(Instant::now);
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            for (root, paths) in pending.drain() {
                                let _ = broadcast_tx.send(FsChanged {
                                    root: root.to_string_lossy().into_owned(),
                                    paths: paths
                                        .iter()
                                        .map(|p| p.to_string_lossy().into_owned())
                                        .collect(),
                                });
                            }
                            window_started = None;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                    }
                }
            })
            .is_ok();
        if !spawned {
            tracing::warn!(
                target: "neoism::fs_watch",
                "fs watch debouncer thread failed to spawn; live tree pushes disabled"
            );
        }
        FsWatchHub {
            tx,
            watchers: Mutex::new(HashMap::new()),
            event_tx,
        }
    })
}

impl FsWatchHub {
    pub fn subscribe(&self) -> broadcast::Receiver<FsChanged> {
        self.tx.subscribe()
    }

    /// Idempotently start watching `root` (recursive). Called from the
    /// files handler on every request, so the first `ListDir` a client
    /// sends is what arms liveness for that root.
    pub fn ensure_watched(&self, root: &Path) {
        let Ok(mut watchers) = self.watchers.lock() else {
            return;
        };
        if watchers.contains_key(root) {
            return;
        }
        let event_tx = self.event_tx.clone();
        let event_root = root.to_path_buf();
        let watcher = notify::recommended_watcher(
            move |event: Result<notify::Event, notify::Error>| {
                let Ok(event) = event else { return };
                // Content-only modifications don't change the tree
                // shape, but renames/creates/removes do; send them
                // all and let the debounce + client re-list absorb
                // the noise (a listing is cheap). Ignored trees
                // (`node_modules`, `.git`, build output) are dropped
                // here so their churn never reaches the debouncer — see
                // `IGNORED_SEGMENTS`.
                for path in event.paths {
                    if is_ignored_watch_path(&event_root, &path) {
                        continue;
                    }
                    let _ = event_tx.send((event_root.clone(), path));
                }
            },
        );
        match watcher {
            Ok(mut watcher) => {
                if let Err(error) = watcher.watch(root, RecursiveMode::Recursive) {
                    tracing::warn!(
                        target: "neoism::fs_watch",
                        %error,
                        root = %root.display(),
                        "fs watch failed; live tree pushes disabled for this root"
                    );
                    return;
                }
                tracing::info!(
                    target: "neoism::fs_watch",
                    root = %root.display(),
                    "watching files root for live tree pushes"
                );
                watchers.insert(root.to_path_buf(), watcher);
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::fs_watch",
                    %error,
                    root = %root.display(),
                    "fs watcher construction failed"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{is_ignored_watch_path, Path};

    #[test]
    fn drops_churny_trees_under_root() {
        let root = Path::new("/home/me/repo");
        for tail in [
            "node_modules/twilio/lib/rest/pricing/v2/voice",
            ".git/index",
            ".git/objects/63/a9c8bf",
            "apps/frontend/.next/static/chunk.js",
            "target/debug/build/x",
            "dist/bundle.js",
            "__pycache__/mod.cpython-312.pyc",
            ".venv/lib/site-packages/foo.py",
        ] {
            let p = root.join(tail);
            assert!(
                is_ignored_watch_path(root, &p),
                "expected {tail} to be ignored"
            );
        }
    }

    #[test]
    fn keeps_real_source_changes() {
        let root = Path::new("/home/me/repo");
        for tail in [
            "src/main.rs",
            "apps/frontend/pages/index.tsx",
            "README.md",
            "dist.rs",            // file named like an ignored dir, not inside one
            "my-node_modules.md", // segment must match exactly
            "notes/build-log.md",
        ] {
            let p = root.join(tail);
            assert!(
                !is_ignored_watch_path(root, &p),
                "expected {tail} to be kept"
            );
        }
    }

    #[test]
    fn root_ancestor_named_like_ignored_dir_is_not_disqualifying() {
        // The watched root itself lives under a `build/` ancestor; only
        // segments *below* the root should be considered.
        let root = Path::new("/srv/build/my-repo");
        let p = root.join("src/lib.rs");
        assert!(!is_ignored_watch_path(root, &p));
    }
}
