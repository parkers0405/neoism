//! Process-wide lifecycle management for `fff-search` file pickers.
//!
//! A picker owns several filesystem-watcher and indexing threads. Finder,
//! file mentions, and agent tools all search the same roots, so keeping a
//! separate unbounded cache in each surface multiplies both watchers and
//! threads. This registry canonicalizes roots, shares a picker across every
//! caller in the process, and retains only the most recently used roots.
//!
//! Watching intentionally remains enabled. Evicting an idle registry entry
//! drops its shared handle (which lets fff shut its watcher down); an in-flight
//! search owns a clone and therefore remains valid until that search finishes.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use anyhow::Context;
use fff_search::{
    FFFMode, FilePicker, FilePickerOptions, SharedFilePicker, SharedFrecency,
};

const DEFAULT_CAPACITY: usize = 8;
const MAX_CAPACITY: usize = 64;
const INITIAL_SCAN_WAIT: Duration = Duration::from_secs(15);

struct Entry {
    picker: SharedFilePicker,
    generation: u64,
    last_used: u64,
}

#[derive(Default)]
struct RegistryState {
    entries: HashMap<PathBuf, Entry>,
    pins: HashMap<PathBuf, usize>,
    clock: u64,
    next_generation: u64,
}

/// A bounded, thread-safe cache of live filesystem-search indexes.
pub struct PickerRegistry {
    capacity: usize,
    state: Mutex<RegistryState>,
}

/// Keeps one canonical root resident while a UI surface is actively using it.
/// The picker itself is still created lazily on the first query.
pub struct PickerRootPin {
    root: PathBuf,
    registry: &'static PickerRegistry,
}

impl PickerRootPin {
    /// Whether this pin protects `root` (path aliases are canonicalized).
    pub fn is_for(&self, root: &Path) -> bool {
        self.root == root || self.root == canonical_root(root)
    }
}

impl Drop for PickerRootPin {
    fn drop(&mut self) {
        self.registry.unpin(&self.root);
    }
}

impl PickerRegistry {
    fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.clamp(1, MAX_CAPACITY),
            state: Mutex::new(RegistryState::default()),
        }
    }

    fn picker(&self, root: &Path) -> anyhow::Result<(PathBuf, u64, SharedFilePicker)> {
        let root = canonical_root(root);
        let (picker, generation, evicted) = {
            let mut state = self
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("FFF picker registry lock was poisoned"))?;
            state.clock = state.clock.wrapping_add(1);
            let last_used = state.clock;

            if let Some(entry) = state.entries.get_mut(&root) {
                entry.last_used = last_used;
                (entry.picker.clone(), entry.generation, Vec::new())
            } else {
                let picker = build_picker(&root)?;
                state.next_generation = state.next_generation.wrapping_add(1);
                let generation = state.next_generation;
                state.entries.insert(
                    root.clone(),
                    Entry {
                        picker: picker.clone(),
                        generation,
                        last_used,
                    },
                );
                let evicted = evict_lru(&mut state, self.capacity);
                (picker, generation, evicted)
            }
        };

        // FilePicker teardown may signal and join background workers. Never do
        // that while holding the registry mutex needed by other search roots.
        drop(evicted);
        Ok((root, generation, picker))
    }

    fn invalidate(&self, root: &Path, generation: u64) {
        let removed = self.state.lock().ok().and_then(|mut state| {
            let matches_generation = state
                .entries
                .get(root)
                .is_some_and(|entry| entry.generation == generation);
            matches_generation
                .then(|| state.entries.remove(root))
                .flatten()
        });
        // As above, keep picker teardown outside the registry lock.
        drop(removed);
    }

    fn pin(&self, root: &Path) -> PathBuf {
        let root = canonical_root(root);
        if let Ok(mut state) = self.state.lock() {
            *state.pins.entry(root.clone()).or_default() += 1;
        }
        root
    }

    fn unpin(&self, root: &Path) {
        let evicted = if let Ok(mut state) = self.state.lock() {
            if let Some(count) = state.pins.get_mut(root) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    state.pins.remove(root);
                }
            }
            evict_lru(&mut state, self.capacity)
        } else {
            Vec::new()
        };
        drop(evicted);
    }

    fn warm(&self, root: &Path) -> anyhow::Result<()> {
        self.picker(root).map(|_| ())
    }

    fn with_picker<T>(
        &self,
        root: &Path,
        operation: impl FnOnce(&FilePicker) -> T,
    ) -> anyhow::Result<T> {
        let (root, generation, shared) = self.picker(root)?;
        if !shared.wait_for_scan(INITIAL_SCAN_WAIT) {
            anyhow::bail!(
                "FFF index for {} is still scanning; retry in a moment",
                root.display()
            );
        }
        // A completed initial scan is enough to search current files. The
        // watcher is only needed for live updates; waiting on it on Windows
        // (notify + Defender) can stall every grep/glob for 15s or fail
        // permanently if the watcher never becomes ready.

        let outcome = {
            let guard = shared.read().map_err(|error| {
                anyhow::anyhow!("FFF picker read lock failed: {error}")
            })?;
            let picker = guard.as_ref().ok_or_else(|| {
                anyhow::anyhow!("FFF picker for {} was dropped", root.display())
            })?;
            // Keep a third-party panic from poisoning the shared picker lock.
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(picker)))
        };

        match outcome {
            Ok(value) => Ok(value),
            Err(payload) => {
                // Only remove the exact generation that panicked. Another
                // thread may already have rebuilt a healthy picker for root.
                self.invalidate(&root, generation);
                Err(anyhow::anyhow!(
                    "fff search engine panicked ({}); narrow the path/pattern, lower the limit, or switch grep mode",
                    panic_payload_message(payload.as_ref())
                ))
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .map(|state| state.entries.len())
            .unwrap_or_default()
    }

    #[cfg(test)]
    fn contains(&self, root: &Path) -> bool {
        let root = canonical_root(root);
        self.state
            .lock()
            .map(|state| state.entries.contains_key(&root))
            .unwrap_or(false)
    }
}

fn evict_lru(state: &mut RegistryState, capacity: usize) -> Vec<Entry> {
    let mut evicted = Vec::new();
    while state.entries.len() > capacity {
        let Some(oldest) = state
            .entries
            .iter()
            .filter(|(root, _)| !state.pins.contains_key(*root))
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(root, _)| root.clone())
        else {
            break;
        };
        if let Some(entry) = state.entries.remove(&oldest) {
            evicted.push(entry);
        }
    }
    evicted
}

fn canonical_root(root: &Path) -> PathBuf {
    crate::windows_process::canonicalize_path_lossy(root)
}

fn build_picker(root: &Path) -> anyhow::Result<SharedFilePicker> {
    let shared = SharedFilePicker::default();
    FilePicker::new_with_shared_state(
        shared.clone(),
        SharedFrecency::default(),
        FilePickerOptions {
            base_path: root.to_string_lossy().to_string(),
            mode: FFFMode::Ai,
            enable_mmap_cache: mmap_enabled(),
            // The path index powers Finder/glob while grep scans content on
            // demand. A resident whole-repository content index adds a large
            // first-hit pause and memory cost without helping these callers.
            enable_content_indexing: false,
            // Live additions, removals, renames, and edits must remain visible.
            watch: true,
            follow_symlinks: false,
            enable_fs_root_scanning: false,
            enable_home_dir_scanning: false,
            cache_budget: None,
        },
    )
    .with_context(|| format!("failed to initialize FFF index for {}", root.display()))?;
    Ok(shared)
}

fn panic_payload_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "unknown panic".to_string()
    }
}

fn mmap_enabled() -> bool {
    std::env::var_os("NEOISM_AGENT_FFF_MMAP")
        .as_deref()
        .is_some_and(|value| {
            matches!(
                value.to_string_lossy().as_ref(),
                "1" | "true" | "TRUE" | "yes" | "YES"
            )
        })
}

fn configured_capacity() -> usize {
    std::env::var("NEOISM_FFF_PICKER_CACHE_CAPACITY")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_CAPACITY)
        .clamp(1, MAX_CAPACITY)
}

fn global() -> &'static PickerRegistry {
    static REGISTRY: std::sync::OnceLock<PickerRegistry> = std::sync::OnceLock::new();
    REGISTRY.get_or_init(|| PickerRegistry::new(configured_capacity()))
}

/// Start (or refresh the recency of) the shared live index for `root`.
pub fn warm(root: &Path) -> anyhow::Result<()> {
    global().warm(root)
}

/// Keep `root` resident until the returned pin is dropped.
///
/// Pinning does not start an index by itself; this avoids doing filesystem
/// work for an active workspace until Finder or mentions actually need it.
pub fn pin(root: &Path) -> PickerRootPin {
    let registry = global();
    PickerRootPin {
        root: registry.pin(root),
        registry,
    }
}

/// Run a search against the shared live index for `root`.
pub fn with_picker<T>(
    root: &Path,
    operation: impl FnOnce(&FilePicker) -> T,
) -> anyhow::Result<T> {
    global().with_picker(root, operation)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::thread;
    use std::time::{Duration, Instant};

    use super::*;

    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

    struct TestRoot(PathBuf);

    impl TestRoot {
        fn new(label: &str) -> Self {
            let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "neoism-picker-registry-{label}-{}-{id}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("create test root");
            Self(path)
        }
    }

    impl Drop for TestRoot {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn registry_is_bounded_and_lru() {
        let first = TestRoot::new("first");
        let second = TestRoot::new("second");
        let third = TestRoot::new("third");
        let registry = PickerRegistry::new(2);

        registry.warm(&first.0).expect("warm first");
        registry.warm(&second.0).expect("warm second");
        registry.warm(&first.0).expect("refresh first");
        registry.warm(&third.0).expect("warm third");

        assert_eq!(registry.len(), 2);
        assert!(registry.contains(&first.0));
        assert!(!registry.contains(&second.0));
        assert!(registry.contains(&third.0));
        drop(registry);
    }

    #[test]
    fn pinned_root_is_not_evicted() {
        let first = TestRoot::new("pinned");
        let second = TestRoot::new("unpinned");
        let registry = PickerRegistry::new(1);

        registry.warm(&first.0).expect("warm first");
        let pinned_root = registry.pin(&first.0);
        registry.warm(&second.0).expect("warm second");

        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&first.0));
        assert!(!registry.contains(&second.0));
        registry.unpin(&pinned_root);
        registry.warm(&second.0).expect("warm second after unpin");
        assert_eq!(registry.len(), 1);
        assert!(registry.contains(&second.0));
        drop(registry);
    }

    #[test]
    fn canonical_root_strips_windows_verbatim_prefix() {
        let root = TestRoot::new("verbatim");
        let canonical = canonical_root(&root.0);
        let text = canonical.to_string_lossy();
        assert!(
            !text.starts_with(r"\\?\"),
            "search roots must not keep a verbatim prefix: {text}"
        );
        drop(root);
    }

    #[test]
    fn watched_picker_observes_new_files_and_directories() {
        let root = TestRoot::new("watch");
        let registry = PickerRegistry::new(1);
        fs::write(root.0.join("before.txt"), "before").expect("write initial file");

        let initial = registry
            .with_picker(&root.0, |picker| picker.get_files().len())
            .expect("initial scan");
        fs::create_dir(root.0.join("added")).expect("create watched directory");
        fs::write(root.0.join("added/after.txt"), "after").expect("write watched file");

        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let paths = registry
                .with_picker(&root.0, |picker| {
                    picker
                        .get_files()
                        .iter()
                        .map(|file| file.relative_path(picker))
                        .collect::<Vec<_>>()
                })
                .expect("query watched picker");
            if paths.iter().any(|path| path == "added/after.txt") {
                assert!(paths.len() >= initial + 1);
                break;
            }
            assert!(Instant::now() < deadline, "watcher missed newly added file");
            thread::sleep(Duration::from_millis(50));
        }
        drop(registry);
    }
}
