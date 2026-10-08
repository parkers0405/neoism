//! FFF-backed workspace-search adapter for the transport-neutral Agent service API.

use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, Metadata};
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::Context;
use fff_search::{
    has_regex_metacharacters, FFFMode, FilePicker, FilePickerOptions, FuzzySearchOptions,
    GrepMode, PaginationArgs, QueryParser, SharedFilePicker, SharedFrecency,
};
use globset::{GlobBuilder, GlobMatcher};
use ignore::WalkBuilder;
use neoism_agent_service_api::{
    DirectorySearchRequest, DirectorySearchResult, FindFilesRequest, FindFilesResult,
    GrepWorkspaceRequest, GrepWorkspaceResult, ServiceError, WorkspaceFileMatch,
    WorkspaceGrepMatch, WorkspaceSearchBounds, WorkspaceSearchMode,
    WorkspaceSearchRootPin, WorkspaceSearchService,
};
use regex::RegexBuilder;

pub const ENGINE_ID: &str = "fff";
const STREAMING_ENGINE_ID: &str = "ripgrep";
const DEFAULT_CAPACITY: usize = 8;
const MAX_CAPACITY: usize = 64;
const INITIAL_SCAN_WAIT: Duration = Duration::from_secs(15);
const MAX_SEARCH_FILE_BYTES: u64 = 16 * 1024 * 1024;
const DEFAULT_EXCLUDES: &[&str] = &[
    ".git",
    ".claude/worktrees",
    ".codex",
    ".neoism/cache",
    "target",
    "node_modules",
    "dist",
    ".tmp",
];

struct Entry {
    picker: SharedFilePicker,
    generation: u64,
    last_used: u64,
    watching: bool,
}
#[derive(Default)]
struct RegistryState {
    entries: HashMap<PathBuf, Entry>,
    pins: HashMap<PathBuf, usize>,
    clock: u64,
    next_generation: u64,
    // Retain only candidate paths in stable order, never matches/content.
    grep_snapshots: HashMap<u64, (Instant, Arc<Vec<PathBuf>>)>,
    // Random capabilities select immutable, server-issued positions. No client
    // input can set a line/byte anchor, and caches never cross service instances.
    grep_cursors: HashMap<String, (Instant, GrepCursor)>,
}
struct PickerRegistry {
    capacity: usize,
    state: Mutex<RegistryState>,
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
            let mut state = self.state.lock().map_err(|_| {
                anyhow::anyhow!("workspace-search registry lock was poisoned")
            })?;
            state.clock = state.clock.wrapping_add(1);
            let used = state.clock;
            let watch = state.pins.contains_key(&root) && watch_safe_root(&root);
            // A watcherless index is a snapshot, not a cache we can safely reuse:
            // rebuild on every acquisition (including warm) so additions, removals,
            // renames, metadata and cached content all reflect a new scan. Pinning
            // an already-warmed root promotes it on its next acquisition.
            if watch && state.entries.get(&root).is_some_and(|entry| entry.watching) {
                let entry = state.entries.get_mut(&root).unwrap();
                entry.last_used = used;
                (entry.picker.clone(), entry.generation, Vec::new())
            } else {
                let picker = build_picker(&root, watch)?;
                state.next_generation = state.next_generation.wrapping_add(1);
                let generation = state.next_generation;
                let replaced = state.entries.insert(
                    root.clone(),
                    Entry {
                        picker: picker.clone(),
                        generation,
                        last_used: used,
                        watching: watch,
                    },
                );
                let mut evicted = evict_lru(&mut state, self.capacity);
                evicted.extend(replaced);
                (picker, generation, evicted)
            }
        };
        retire_entries(evicted);
        Ok((root, generation, picker))
    }
    fn with_picker<T>(
        &self,
        root: &Path,
        operation: impl FnOnce(&FilePicker) -> T,
    ) -> anyhow::Result<T> {
        let (root, generation, shared) = self.picker(root)?;
        if !shared.wait_for_scan(INITIAL_SCAN_WAIT) {
            anyhow::bail!(
                "workspace index for {} is still scanning; retry in a moment",
                root.display()
            );
        }
        let outcome = {
            let guard = shared.read().map_err(|error| {
                anyhow::anyhow!("workspace picker read lock failed: {error}")
            })?;
            let picker = guard.as_ref().ok_or_else(|| {
                anyhow::anyhow!("workspace picker for {} was dropped", root.display())
            })?;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(picker)))
        };
        outcome.map_err(|payload| {
            let removed = self.state.lock().ok().and_then(|mut state| {
                state.entries.get(&root).is_some_and(|entry| entry.generation == generation)
                    .then(|| state.entries.remove(&root)).flatten()
            });
            retire_entries(removed);
            anyhow::anyhow!("workspace search engine panicked ({}); narrow the path/pattern or lower the limit", panic_message(payload.as_ref()))
        })
    }
    fn warm(&self, root: &Path) -> anyhow::Result<()> {
        self.picker(root).map(|_| ())
    }
    fn pin(self: &Arc<Self>, root: &Path) -> PickerRootPin {
        let root = canonical_root(root);
        if let Ok(mut state) = self.state.lock() {
            *state.pins.entry(root.clone()).or_default() += 1;
        }
        PickerRootPin {
            root,
            registry: self.clone(),
        }
    }
    fn unpin(&self, root: &Path) {
        let removed = if let Ok(mut state) = self.state.lock() {
            let mut removed = Vec::new();
            if let Some(count) = state.pins.get_mut(root) {
                *count = count.saturating_sub(1);
                if *count == 0 {
                    state.pins.remove(root);
                    // Do not leave a workspace watcher alive in the unpinned LRU.
                    removed.extend(state.entries.remove(root));
                }
            }
            removed.extend(evict_lru(&mut state, self.capacity));
            removed
        } else {
            Vec::new()
        };
        retire_entries(removed);
    }
    #[cfg(test)]
    fn len(&self) -> usize {
        self.state
            .lock()
            .map(|s| s.entries.len())
            .unwrap_or_default()
    }
    #[cfg(test)]
    fn contains(&self, root: &Path) -> bool {
        self.state
            .lock()
            .map(|s| s.entries.contains_key(&canonical_root(root)))
            .unwrap_or(false)
    }
}

// Stop outside the registry mutex: searches can hold the picker lock. Cancel
// also prevents an initial scan from installing its watcher after retirement.
fn retire_entries(entries: impl IntoIterator<Item = Entry>) {
    for entry in entries {
        if entry.watching {
            if let Ok(mut guard) = entry.picker.write() {
                if let Some(picker) = guard.as_mut() {
                    picker.cancel();
                    picker.stop_background_monitor();
                }
            }
        }
    }
}

fn evict_lru(state: &mut RegistryState, capacity: usize) -> Vec<Entry> {
    let mut removed = Vec::new();
    while state.entries.len() > capacity {
        let Some(root) = state
            .entries
            .iter()
            .filter(|(root, _)| !state.pins.contains_key(*root))
            .min_by_key(|(_, entry)| entry.last_used)
            .map(|(root, _)| root.clone())
        else {
            break;
        };
        if let Some(entry) = state.entries.remove(&root) {
            removed.push(entry);
        }
    }
    removed
}

struct PickerRootPin {
    root: PathBuf,
    registry: Arc<PickerRegistry>,
}
impl Drop for PickerRootPin {
    fn drop(&mut self) {
        self.registry.unpin(&self.root);
    }
}
impl WorkspaceSearchRootPin for PickerRootPin {
    fn root(&self) -> &Path {
        &self.root
    }
}

/// Instance-owned FFF adapter. Each instance has independent indexes and pins.
/// Only pinned roots outside Neoism's log tree reuse watched indexes. Other
/// roots get a fresh watcherless index per acquisition; `warm` is not a pin.
#[derive(Clone)]
pub struct FffWorkspaceSearchService {
    registry: Arc<PickerRegistry>,
}
impl FffWorkspaceSearchService {
    pub fn new() -> Self {
        Self::with_capacity(configured_capacity())
    }
    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            registry: Arc::new(PickerRegistry::new(capacity)),
        }
    }
    pub fn with_picker<T>(
        &self,
        root: &Path,
        operation: impl FnOnce(&FilePicker) -> T,
    ) -> anyhow::Result<T> {
        self.registry.with_picker(root, operation)
    }
}
impl Default for FffWorkspaceSearchService {
    fn default() -> Self {
        Self::new()
    }
}

impl WorkspaceSearchService for FffWorkspaceSearchService {
    fn warm(&self, root: &Path) -> Result<(), ServiceError> {
        self.registry.warm(root).map_err(service_error)
    }
    fn pin_root(
        &self,
        root: &Path,
    ) -> Result<Arc<dyn WorkspaceSearchRootPin>, ServiceError> {
        Ok(Arc::new(self.registry.pin(root)))
    }
    fn find_files(
        &self,
        request: &FindFilesRequest,
    ) -> Result<FindFilesResult, ServiceError> {
        find_files(&self.registry, request).map_err(service_error)
    }
    fn grep(
        &self,
        request: &GrepWorkspaceRequest,
    ) -> Result<GrepWorkspaceResult, ServiceError> {
        grep_workspace(&self.registry, request).map_err(service_error)
    }
    fn search_directories(
        &self,
        request: &DirectorySearchRequest,
    ) -> Result<DirectorySearchResult, ServiceError> {
        search_directories(&self.registry, request).map_err(service_error)
    }
}

fn find_files(
    registry: &PickerRegistry,
    request: &FindFilesRequest,
) -> anyhow::Result<FindFilesResult> {
    if request.query.trim().is_empty() {
        return directory_entries(request);
    }
    // The index deliberately favors the fast, ordinary-project path. An
    // explicit hidden-file query uses the bounded streaming walker so its
    // behavior does not depend on platform/indexer dotfile defaults.
    if request.include_hidden {
        return streaming_find(request, "hidden-file discovery requested");
    }
    if broad_root(&request.root) {
        return streaming_find(
            request,
            "indexed search is disabled for home and filesystem roots",
        );
    }
    let started = Instant::now();
    match registry.with_picker(&request.root, |picker| {
        let parser = QueryParser::default();
        let mut results = picker.fuzzy_search(
            &parser.parse(&request.query),
            None,
            FuzzySearchOptions {
                max_threads: 0,
                current_file: None,
                project_path: Some(&request.root),
                pagination: PaginationArgs {
                    offset: request.offset,
                    limit: request.limit,
                },
                ..Default::default()
            },
        );
        if results.items.is_empty() {
            if let Some(token) = request
                .query
                .split_whitespace()
                .filter(|s| s.len() >= 3)
                .max_by_key(|s| s.len())
            {
                if token != request.query {
                    results = picker.fuzzy_search(
                        &parser.parse(token),
                        None,
                        FuzzySearchOptions {
                            max_threads: 0,
                            current_file: None,
                            project_path: Some(&request.root),
                            pagination: PaginationArgs {
                                offset: request.offset,
                                limit: request.limit,
                            },
                            ..Default::default()
                        },
                    );
                }
            }
        }
        let items = results
            .items
            .iter()
            .zip(&results.scores)
            .filter_map(|(item, score)| {
                let path = item.relative_path(picker);
                discoverable_path(&path, false).then(|| WorkspaceFileMatch {
                    path,
                    score: score.total,
                    git_status: item.git_status.map(git_status_label),
                    size: item.size,
                    modified: item.modified,
                })
            })
            .collect::<Vec<_>>();
        (items, results.total_matched)
    }) {
        Ok((items, total)) => Ok(FindFilesResult {
            bounds: bounds(Some(total), total, request.offset, items.len(), false),
            items,
            engine: Some(ENGINE_ID.into()),
            fallback_reason: None,
        }),
        Err(error) => {
            let mut fallback = request.clone();
            fallback.control.timeout_ms = request
                .control
                .timeout_ms
                .saturating_sub(started.elapsed().as_millis() as u64)
                .saturating_sub(500)
                .max(1);
            streaming_find(&fallback, &error.to_string())
        }
    }
}

fn search_directories(
    registry: &PickerRegistry,
    request: &DirectorySearchRequest,
) -> anyhow::Result<DirectorySearchResult> {
    registry.with_picker(&request.root, |picker| {
        let parser = QueryParser::new(fff_search::DirSearchConfig);
        let results = picker.fuzzy_search_directories(
            &parser.parse(&request.query),
            FuzzySearchOptions {
                max_threads: 0,
                project_path: Some(&request.root),
                pagination: PaginationArgs {
                    offset: request.offset,
                    limit: request.limit,
                },
                ..Default::default()
            },
        );
        let paths = results
            .items
            .iter()
            .map(|item| item.relative_path(picker))
            .collect::<Vec<_>>();
        DirectorySearchResult {
            bounds: bounds(
                Some(results.total_matched),
                results.total_matched,
                request.offset,
                paths.len(),
                false,
            ),
            paths,
            engine: Some(ENGINE_ID.into()),
        }
    })
}

fn grep_workspace(
    registry: &PickerRegistry,
    request: &GrepWorkspaceRequest,
) -> anyhow::Result<GrepWorkspaceResult> {
    if request.context_lines > MAX_GREP_CONTEXT {
        anyhow::bail!("grep context must be at most {MAX_GREP_CONTEXT}");
    }
    check_cancel(request.control.cancel.as_ref(), "grep")?;
    // Exact modes share the same bounded matcher on indexed and walk candidates.
    let mode = effective_mode(request);
    LineMatcher::new(&request.patterns, mode, request.case_sensitive)?;
    let cursor = GrepCursor::parse(registry, request)?;
    if cursor.as_ref().is_some_and(|c| c.engine == "walk") {
        return streaming_grep(registry, request, "streaming continuation");
    }
    let fallback = if request.path.is_file() {
        Some("exact file requested")
    } else if request.include_hidden {
        Some("hidden-file discovery requested")
    } else if !request.path.starts_with(&request.root) {
        Some("search path is outside the workspace index")
    } else if broad_root(&request.root) {
        Some("indexed search is disabled for home and filesystem roots")
    } else {
        None
    };
    if let Some(reason) = fallback {
        if cursor.is_some() {
            anyhow::bail!("grep cursor engine changed; restart without cursor");
        }
        return streaming_grep(registry, request, reason);
    }
    let deadline = Instant::now() + Duration::from_millis(request.control.timeout_ms);
    if let Some(c) = &cursor {
        let paths = {
            let mut state = registry
                .state
                .lock()
                .map_err(|_| anyhow::anyhow!("search registry poisoned"))?;
            let entry = state
                .grep_snapshots
                .get_mut(&c.snapshot)
                .filter(|(used, _)| used.elapsed() < Duration::from_secs(300))
                .ok_or_else(|| {
                    anyhow::anyhow!("grep cursor expired; restart without cursor")
                })?;
            entry.0 = Instant::now();
            entry.1.clone()
        };
        return scan_grep_paths(
            registry,
            request,
            paths.iter().cloned().map(Ok),
            "fff",
            c.snapshot,
            None,
            deadline,
        );
    }
    let (_, shared) = match registry.picker(&request.root) {
        Ok((_, generation, shared)) => (generation, shared),
        Err(error) => return streaming_grep(registry, request, &error.to_string()),
    };
    // Unlike with_picker's fixed 15-second wait, this cooperates with request
    // cancellation and deadline during the initial index traversal.
    while !shared.wait_for_scan(Duration::from_millis(10)) {
        check_cancel(request.control.cancel.as_ref(), "grep")?;
        if Instant::now() >= deadline {
            return Ok(empty_grep(mode, true, ENGINE_ID));
        }
    }
    check_cancel(request.control.cancel.as_ref(), "grep")?;
    let guard = shared
        .read()
        .map_err(|e| anyhow::anyhow!("picker read failed: {e}"))?;
    let picker = guard
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("picker dropped"))?;
    let mut paths = Vec::new();
    for file in picker
        .get_files()
        .iter()
        .chain(picker.get_overflow_files())
        .filter(|f| !f.is_deleted())
    {
        check_cancel(request.control.cancel.as_ref(), "grep")?;
        if Instant::now() >= deadline {
            return Ok(empty_grep(mode, true, ENGINE_ID));
        }
        let path = request.root.join(file.relative_path(picker));
        if path.starts_with(&request.path) {
            paths.push(path);
        }
    }
    paths.sort();
    paths.dedup();
    let paths = Arc::new(paths);
    drop(guard);
    let id = {
        let mut state = registry
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("search registry poisoned"))?;
        state.next_generation = state.next_generation.wrapping_add(1);
        state.next_generation
    };
    // FFF discovers/indexes candidates; its grep API cannot resume inside a
    // file. Use the existing streaming matcher for bounded match/context pages.
    let result = scan_grep_paths(
        registry,
        request,
        paths.iter().cloned().map(Ok),
        "fff",
        id,
        None,
        deadline,
    )?;
    if result.total_files_searched == 0 && !result.bounds.timed_out {
        // An initial index can legitimately be empty/stale or omit searchable
        // files. An exact content miss after searching files is NOT a fallback.
        let mut fallback = request.clone();
        fallback.control.timeout_ms = deadline
            .saturating_duration_since(Instant::now())
            .as_millis()
            .min(u64::MAX as u128) as u64;
        return streaming_grep(
            registry,
            &fallback,
            "indexed search returned no searchable files",
        );
    }
    if result.next_cursor.is_some() {
        let mut state = registry
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("search registry poisoned"))?;
        state
            .grep_snapshots
            .retain(|_, (used, _)| used.elapsed() < Duration::from_secs(300));
        if state.grep_snapshots.len() >= registry.capacity {
            if let Some(oldest) = state
                .grep_snapshots
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(id, _)| *id)
            {
                state.grep_snapshots.remove(&oldest);
            }
        }
        state.grep_snapshots.insert(id, (Instant::now(), paths));
    }
    Ok(result)
}

fn effective_mode(request: &GrepWorkspaceRequest) -> WorkspaceSearchMode {
    match request.mode {
        WorkspaceSearchMode::Auto
            if request.patterns.iter().any(|p| has_regex_metacharacters(p)) =>
        {
            WorkspaceSearchMode::Regex
        }
        WorkspaceSearchMode::Auto => WorkspaceSearchMode::Plain,
        mode => mode,
    }
}

#[derive(Clone)]
struct GrepCursor {
    query: u64,
    engine: String,
    // Hash of the current absolute file path, independent of traversal offset.
    offset: u64,
    line: u64,
    byte: u64,
    anchor_line: u64,
    state: u64,
    snapshot: u64,
}
const MAX_GREP_CURSORS: usize = 4096;
const GREP_CURSOR_TTL: Duration = Duration::from_secs(300);
impl GrepCursor {
    fn encode(&self, registry: &PickerRegistry) -> anyhow::Result<String> {
        let mut state = registry
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("search registry poisoned"))?;
        state
            .grep_cursors
            .retain(|_, (used, _)| used.elapsed() < GREP_CURSOR_TTL);
        if state.grep_cursors.len() >= MAX_GREP_CURSORS {
            if let Some(oldest) = state
                .grep_cursors
                .iter()
                .min_by_key(|(_, (used, _))| *used)
                .map(|(token, _)| token.clone())
            {
                state.grep_cursors.remove(&oldest);
            }
        }
        // 256 random bits. Positions never leave the service or get decoded
        // from user input; modifying a token only selects an unknown capability.
        let token = loop {
            let candidate = rand::random::<[u8; 32]>()
                .iter()
                .map(|byte| format!("{byte:02x}"))
                .collect::<String>();
            if !state.grep_cursors.contains_key(&candidate) {
                break candidate;
            }
        };
        state
            .grep_cursors
            .insert(token.clone(), (Instant::now(), self.clone()));
        Ok(token)
    }
    fn parse(
        registry: &PickerRegistry,
        request: &GrepWorkspaceRequest,
    ) -> anyhow::Result<Option<Self>> {
        let Some(raw) = &request.cursor else {
            return Ok(None);
        };
        if raw.len() != 64 || !raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            anyhow::bail!("invalid grep cursor");
        }
        let mut state = registry
            .state
            .lock()
            .map_err(|_| anyhow::anyhow!("search registry poisoned"))?;
        let (used, cursor) = state
            .grep_cursors
            .get_mut(raw)
            .filter(|(used, _)| used.elapsed() < GREP_CURSOR_TTL)
            .ok_or_else(|| {
                anyhow::anyhow!("invalid or expired grep cursor; restart without cursor")
            })?;
        if cursor.query != query_hash(request) {
            anyhow::bail!(
                "grep cursor does not match query/root/options; restart without cursor"
            );
        }
        *used = Instant::now();
        Ok(Some(cursor.clone()))
    }
}
fn query_hash(request: &GrepWorkspaceRequest) -> u64 {
    let mut hash = DefaultHasher::new();
    (
        &request.root,
        &request.path,
        &request.patterns,
        &request.include,
        &request.excludes,
        request.include_hidden,
        request.context_lines,
        request.case_sensitive,
        format!("{:?}", request.mode),
    )
        .hash(&mut hash);
    hash.finish()
}
fn hash_metadata(path: &Path, hash: &mut DefaultHasher) {
    let metadata = path.metadata().ok();
    metadata.as_ref().map(|m| m.len()).hash(hash);
    metadata.and_then(|m| m.modified().ok()).hash(hash);
}
fn empty_grep(
    mode: WorkspaceSearchMode,
    timed_out: bool,
    engine: &str,
) -> GrepWorkspaceResult {
    GrepWorkspaceResult {
        next_cursor: None,
        items: Vec::new(),
        files_with_matches: 0,
        total_files_searched: 0,
        bounds: WorkspaceSearchBounds {
            timed_out,
            truncated: timed_out,
            ..Default::default()
        },
        mode: mode_label(grep_mode(mode, "")).into(),
        engine: Some(engine.into()),
        fallback_reason: None,
    }
}

fn streaming_find(
    request: &FindFilesRequest,
    reason: &str,
) -> anyhow::Result<FindFilesResult> {
    let deadline = Instant::now() + Duration::from_millis(request.control.timeout_ms);
    let matcher = PathMatcher::query(&request.query)?;
    let excluded = PathMatcher::patterns(DEFAULT_EXCLUDES.iter().copied())?;
    let root = request.root.clone();
    let (filter_root, filter_excluded) = (root.clone(), excluded.clone());
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(!request.include_hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .filter_entry(move |entry| {
            relative_path(&filter_root, entry.path())
                .is_none_or(|path| !filter_excluded.excludes(&path))
        });
    let wanted = request
        .offset
        .saturating_add(request.limit)
        .saturating_add(1);
    let mut items = Vec::new();
    let mut timed_out = false;
    for entry in builder.build() {
        check_cancel(request.control.cancel.as_ref(), "glob")?;
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let Ok(entry) = entry else { continue };
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Some(path) = relative_path(&root, entry.path()) else {
            continue;
        };
        if !matcher.matches(&path) {
            continue;
        }
        let metadata = entry.metadata().ok();
        items.push(file_match(path, metadata.as_ref()));
        if items.len() >= wanted {
            break;
        }
    }
    let discovered = items.len();
    let items = items
        .into_iter()
        .skip(request.offset)
        .take(request.limit)
        .collect::<Vec<_>>();
    Ok(FindFilesResult {
        bounds: WorkspaceSearchBounds {
            total: None,
            total_at_least: discovered,
            next_cursor: None,
            truncated: timed_out || discovered > request.offset + items.len(),
            timed_out,
        },
        items,
        engine: Some(STREAMING_ENGINE_ID.into()),
        fallback_reason: Some(reason.into()),
    })
}

fn grep_filter_root(request: &GrepWorkspaceRequest) -> &Path {
    if request.path.starts_with(&request.root) {
        &request.root
    } else if request.path.is_file() {
        request.path.parent().unwrap_or(&request.path)
    } else {
        &request.path
    }
}

fn streaming_grep(
    registry: &PickerRegistry,
    request: &GrepWorkspaceRequest,
    reason: &str,
) -> anyhow::Result<GrepWorkspaceResult> {
    let deadline = Instant::now() + Duration::from_millis(request.control.timeout_ms);
    let excluded = PathMatcher::patterns(
        DEFAULT_EXCLUDES
            .iter()
            .copied()
            .chain(request.excludes.iter().map(String::as_str)),
    )?;
    let prune_root = grep_filter_root(request).to_path_buf();
    let cancel = request.control.cancel.clone();
    let mut builder = WalkBuilder::new(&request.path);
    builder
        .hidden(!request.include_hidden)
        .git_ignore(true)
        .git_exclude(true)
        .git_global(true)
        .ignore(true)
        .follow_links(false)
        .max_filesize(Some(MAX_SEARCH_FILE_BYTES))
        .sort_by_file_path(|a, b| a.cmp(b))
        .filter_entry(move |entry| {
            Instant::now() < deadline
                && !cancel.as_ref().is_some_and(|c| c.load(Ordering::SeqCst))
                && relative_path(&prune_root, entry.path())
                    .is_none_or(|path| !excluded.excludes(&path))
        });
    // Include directories in the iterator so cancellation/deadline checks also
    // run during traversal of trees containing no eligible files.
    let paths = builder.build().map(|entry| {
        entry
            .map(|entry| entry.into_path())
            .map_err(anyhow::Error::from)
    });
    scan_grep_paths(registry, request, paths, "walk", 0, Some(reason), deadline)
}

const MAX_GREP_CONTEXT: usize = 100;
// Leave room for headings, cursor advice and per-file separators below the
// central 51,200-byte / 2,000-line output limits. A single oversized record is
// returned intact and may use the central artifact mechanism; never clip context.
const GREP_PAGE_BYTES: usize = 44_000;
const GREP_PAGE_ROWS: usize = 1_800;

fn path_key(path: &Path) -> u64 {
    let mut hash = DefaultHasher::new();
    path.hash(&mut hash);
    hash.finish()
}
fn file_state(path: &Path) -> u64 {
    let mut hash = DefaultHasher::new();
    hash_metadata(path, &mut hash);
    #[cfg(unix)]
    if let Ok(metadata) = path.metadata() {
        use std::os::unix::fs::MetadataExt;
        (
            metadata.dev(),
            metadata.ino(),
            metadata.ctime(),
            metadata.ctime_nsec(),
        )
            .hash(&mut hash);
    }
    hash.finish()
}

fn grep_record_cost(item: &WorkspaceGrepMatch) -> (usize, usize) {
    let rows = 1 + item.context_before.len() + item.context_after.len();
    // Upper bound on the tool renderer's per-record output. Charge a file
    // heading for every record, even when the renderer shares the heading.
    let bytes = item.path.len()
        + item.text.len()
        + 100
        + item
            .context_before
            .iter()
            .chain(&item.context_after)
            .map(|s| s.len() + 40)
            .sum::<usize>();
    (bytes, rows + 2)
}

fn scan_grep_paths(
    registry: &PickerRegistry,
    request: &GrepWorkspaceRequest,
    paths: impl IntoIterator<Item = anyhow::Result<PathBuf>>,
    engine: &str,
    snapshot: u64,
    reason: Option<&str>,
    deadline: Instant,
) -> anyhow::Result<GrepWorkspaceResult> {
    if request.context_lines > MAX_GREP_CONTEXT {
        anyhow::bail!("grep context must be at most {MAX_GREP_CONTEXT}; reduce context rather than silently losing lines");
    }
    let mut matcher = LineMatcher::new(
        &request.patterns,
        effective_mode(request),
        request.case_sensitive,
    )?;
    let cursor = GrepCursor::parse(registry, request)?;
    if cursor.as_ref().is_some_and(|c| c.engine != engine) {
        anyhow::bail!("grep cursor engine changed; restart without cursor");
    }
    let filter_root = grep_filter_root(request);
    let include = request
        .include
        .as_deref()
        .map(PathMatcher::pattern)
        .transpose()?;
    let excluded = PathMatcher::patterns(
        DEFAULT_EXCLUDES
            .iter()
            .copied()
            .chain(request.excludes.iter().map(String::as_str)),
    )?;
    let mut seeking = cursor.is_some();
    let mut items = Vec::<WorkspaceGrepMatch>::new();
    let mut searched = 0usize;
    let mut timed_out = false;
    let mut incomplete = false;
    let mut more = false;
    let mut checkpoint = None;
    let mut page_bytes = 0usize;
    let mut page_rows = 0usize;
    'files: for entry in paths {
        check_cancel(request.control.cancel.as_ref(), "grep")?;
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        let path = match entry {
            Ok(path) => path,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        let key = path_key(&path);
        if seeking && cursor.as_ref().is_some_and(|c| c.offset != key) {
            continue;
        }
        let metadata = match path.symlink_metadata() {
            Ok(metadata) => metadata,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        if !metadata.is_file() {
            continue;
        }
        let state = file_state(&path);
        let resuming = seeking;
        let resume_line = if seeking {
            let c = cursor.as_ref().unwrap();
            if state != c.state {
                anyhow::bail!("grep cursor is stale (checkpoint file changed); restart without cursor");
            }
            seeking = false;
            c.line
        } else {
            0
        };
        let relative = relative_path(filter_root, &path)
            .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));
        if metadata.len() > MAX_SEARCH_FILE_BYTES
            || excluded.excludes(&relative)
            || include.as_ref().is_some_and(|m| !m.matches(&relative))
            || !discoverable_path(&path.to_string_lossy(), true)
            || (!request.path.is_file()
                && !discoverable_path(&relative, request.include_hidden))
            || fff_search::is_known_binary_extension(&path)
        {
            continue;
        }
        let (anchor_byte, anchor_line) = if resuming {
            let c = cursor.as_ref().unwrap();
            (c.byte, c.anchor_line)
        } else {
            (0, 1)
        };
        let mut file = match File::open(&path) {
            Ok(file) => file,
            Err(_) => {
                incomplete = true;
                continue;
            }
        };
        if file.seek(SeekFrom::Start(anchor_byte)).is_err() {
            incomplete = true;
            continue;
        }
        searched += 1;
        let output_path = relative_path(&request.root, &path)
            .unwrap_or_else(|| path.to_string_lossy().replace('\\', "/"));
        let mut reader =
            BufReader::new(file.take(MAX_SEARCH_FILE_BYTES - anchor_byte + 1));
        let mut bytes = Vec::new();
        let mut bytes_read = anchor_byte;
        let mut line_no = anchor_line - 1;
        let mut window = VecDeque::<(u64, String, u64)>::new();
        let mut pending = VecDeque::<(u64, Option<u16>)>::new();
        let mut eof = false;
        loop {
            check_cancel(request.control.cancel.as_ref(), "grep")?;
            if Instant::now() >= deadline {
                timed_out = true;
                break 'files;
            }
            if !eof {
                bytes.clear();
                let read = match reader.read_until(b'\n', &mut bytes) {
                    Ok(n) => n,
                    Err(_) => {
                        incomplete = true;
                        break;
                    }
                };
                bytes_read += read as u64;
                if bytes_read > MAX_SEARCH_FILE_BYTES {
                    incomplete = true;
                    break;
                }
                eof = read == 0;
                if !eof {
                    if bytes.contains(&0) {
                        break;
                    }
                    line_no += 1;
                    while bytes.last().is_some_and(|b| matches!(*b, b'\n' | b'\r')) {
                        bytes.pop();
                    }
                    let line = String::from_utf8_lossy(&bytes);
                    if line_no > resume_line {
                        if let Some(score) = matcher.match_line(&line) {
                            pending.push_back((line_no, score));
                        }
                    }
                    window.push_back((
                        line_no,
                        truncate_line(&line),
                        bytes_read - read as u64,
                    ));
                }
            }
            // Delay each match until its entire after-context is available.
            // Retain at most 2*context+1 lines and context+1 pending line IDs.
            while pending.front().is_some_and(|(number, _)| {
                eof || line_no.saturating_sub(*number) >= request.context_lines as u64
            }) {
                let (number, score) = pending.pop_front().unwrap();
                let text = window
                    .iter()
                    .find(|(line, _, _)| *line == number)
                    .map(|(_, text, _)| text.as_str())
                    .unwrap_or("");
                let mut item = grep_match(&output_path, number, text, score);
                item.context_before = window
                    .iter()
                    .filter(|(line, _, _)| {
                        *line < number && number - *line <= request.context_lines as u64
                    })
                    .map(|(_, text, _)| text.clone())
                    .collect();
                item.context_after = window
                    .iter()
                    .filter(|(line, _, _)| {
                        *line > number && *line - number <= request.context_lines as u64
                    })
                    .map(|(_, text, _)| text.clone())
                    .collect();
                let (cost_bytes, cost_rows) = grep_record_cost(&item);
                if !items.is_empty()
                    && (items.len() >= request.limit.max(1)
                        || page_bytes.saturating_add(cost_bytes) > GREP_PAGE_BYTES
                        || page_rows.saturating_add(cost_rows) > GREP_PAGE_ROWS)
                {
                    more = true;
                    if file_state(&path) != state {
                        anyhow::bail!(
                            "grep checkpoint file changed during search; retry"
                        );
                    }
                    break 'files;
                }
                page_bytes += cost_bytes;
                page_rows += cost_rows;
                items.push(item);
                // Resume by byte at the small before-context anchor, not by
                // rescanning all earlier matches on every page.
                let (anchor_line, _, anchor_byte) = window
                    .iter()
                    .find(|(line, _, _)| {
                        *line >= number.saturating_sub(request.context_lines as u64)
                    })
                    .unwrap();
                checkpoint = Some(GrepCursor {
                    query: query_hash(request),
                    engine: engine.into(),
                    offset: key,
                    line: number,
                    byte: *anchor_byte,
                    anchor_line: *anchor_line,
                    state,
                    snapshot,
                });
            }
            if eof {
                break;
            }
            while window.len() > request.context_lines.saturating_mul(2) + 1 {
                window.pop_front();
            }
        }
        if file_state(&path) != state {
            anyhow::bail!("grep file changed during search; retry");
        }
    }
    check_cancel(request.control.cancel.as_ref(), "grep")?;
    timed_out |= Instant::now() >= deadline;
    if seeking && !timed_out {
        anyhow::bail!(
            "grep cursor is stale (checkpoint file removed); restart without cursor"
        );
    }
    // Completed match records are safe checkpoints, even on a deadline. Any
    // not-yet-finalized match/context is reread from that checkpoint next time.
    let next = if !incomplete && (more || timed_out) {
        checkpoint
            .map(|checkpoint| checkpoint.encode(registry))
            .transpose()?
            .or_else(|| request.cursor.clone())
    } else {
        None
    };
    let files = items.iter().map(|m| &m.path).collect::<HashSet<_>>().len();
    Ok(GrepWorkspaceResult {
        next_cursor: next.clone(),
        bounds: WorkspaceSearchBounds {
            total: if cursor.is_none() && !more && !timed_out && !incomplete {
                Some(items.len())
            } else {
                None
            },
            total_at_least: items.len(),
            next_cursor: None,
            truncated: more || timed_out || incomplete,
            timed_out,
        },
        items,
        files_with_matches: files,
        total_files_searched: searched,
        mode: if request.patterns.len() > 1
            && effective_mode(request) == WorkspaceSearchMode::Plain
        {
            "multi".into()
        } else {
            matcher.label().into()
        },
        engine: Some(
            if engine == "fff" {
                ENGINE_ID
            } else {
                STREAMING_ENGINE_ID
            }
            .into(),
        ),
        fallback_reason: if incomplete {
            Some(format!(
                "{}; traversal/read errors: partial results, retry from input cursor",
                reason.unwrap_or("indexed candidates")
            ))
        } else {
            reason.map(str::to_string)
        },
    })
}

#[derive(Clone)]
struct PathMatcher {
    patterns: Vec<(GlobMatcher, bool)>,
}
impl PathMatcher {
    fn query(query: &str) -> anyhow::Result<Self> {
        let q = query.trim();
        let q = if q.contains(['*', '?', '[', '{']) {
            q.into()
        } else {
            format!("*{}*", globset::escape(q))
        };
        Self::pattern(&q)
    }
    fn pattern(pattern: &str) -> anyhow::Result<Self> {
        Self::patterns(std::iter::once(pattern))
    }
    fn patterns<'a>(patterns: impl IntoIterator<Item = &'a str>) -> anyhow::Result<Self> {
        let mut compiled = Vec::new();
        for pattern in patterns {
            let pattern = pattern.trim().trim_start_matches('!');
            if pattern.is_empty() {
                continue;
            }
            let basename = !pattern.contains(['/', '\\']);
            let matcher = GlobBuilder::new(pattern)
                .case_insensitive(true)
                .literal_separator(true)
                .backslash_escape(false)
                .build()?
                .compile_matcher();
            compiled.push((matcher, basename));
        }
        Ok(Self { patterns: compiled })
    }
    fn excludes(&self, path: &str) -> bool {
        if self.matches(path) {
            return true;
        }
        path.match_indices('/')
            .any(|(index, _)| self.matches(&path[..index]))
    }
    fn matches(&self, path: &str) -> bool {
        let basename = path.rsplit('/').next().unwrap_or(path);
        self.patterns
            .iter()
            .any(|(m, b)| m.is_match(path) || (*b && m.is_match(basename)))
    }
}

enum LineMatcher {
    Regex(regex::Regex),
    Literal(Vec<String>, bool),
    Fuzzy(FffFuzzyLineMatcher),
}
impl LineMatcher {
    fn new(
        patterns: &[String],
        mode: WorkspaceSearchMode,
        case: bool,
    ) -> anyhow::Result<Self> {
        // The caller resolves smartcase; this service boolean is authoritative.
        match mode {
            WorkspaceSearchMode::Regex => {
                let expression = if patterns.len() == 1 {
                    patterns[0].clone()
                } else {
                    patterns
                        .iter()
                        .map(|p| format!("(?:{})", p))
                        .collect::<Vec<_>>()
                        .join("|")
                };
                Ok(Self::Regex(
                    RegexBuilder::new(&expression)
                        .case_insensitive(!case)
                        .build()?,
                ))
            }
            WorkspaceSearchMode::Fuzzy => Ok(Self::Fuzzy(FffFuzzyLineMatcher::new(
                &patterns.join(" "),
                !case,
            )?)),
            WorkspaceSearchMode::Plain | WorkspaceSearchMode::Auto => {
                Ok(Self::Literal(patterns.to_vec(), case))
            }
        }
    }
    /// Some(None) is an exact matching line; Some(Some(score)) is a fuzzy
    /// matching line with the actual upstream Smith-Waterman score.
    fn match_line(&mut self, line: &str) -> Option<Option<u16>> {
        let matched = match self {
            Self::Regex(r) => r.is_match(line),
            Self::Literal(n, c) => {
                if *c {
                    n.iter().any(|x| line.contains(x))
                } else {
                    let line = line.to_lowercase();
                    n.iter().any(|x| line.contains(&x.to_lowercase()))
                }
            }
            Self::Fuzzy(matcher) => return matcher.score(line).map(Some),
        };
        matched.then_some(None)
    }
    fn label(&self) -> &'static str {
        match self {
            Self::Regex(_) => "regex",
            Self::Literal(..) => "plain",
            Self::Fuzzy(..) => "fuzzy",
        }
    }
}

fn build_picker(root: &Path, watch: bool) -> anyhow::Result<SharedFilePicker> {
    let shared = SharedFilePicker::default();
    FilePicker::new_with_shared_state(
        shared.clone(),
        SharedFrecency::default(),
        FilePickerOptions {
            base_path: root.to_string_lossy().into(),
            mode: FFFMode::Ai,
            enable_mmap_cache: env_flag("NEOISM_AGENT_FFF_MMAP"),
            enable_content_indexing: false,
            watch,
            follow_symlinks: false,
            enable_fs_root_scanning: false,
            enable_home_dir_scanning: false,
            cache_budget: None,
        },
    )
    .with_context(|| format!("failed to initialize FFF index for {}", root.display()))?;
    Ok(shared)
}
fn directory_entries(request: &FindFilesRequest) -> anyhow::Result<FindFilesResult> {
    let mut entries = std::fs::read_dir(&request.root)?
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|path| {
            path.file_name().is_some_and(|name| {
                discoverable_path(&name.to_string_lossy(), request.include_hidden)
            })
        })
        .collect::<Vec<_>>();
    entries.sort();
    let total = entries.len();
    let items = entries
        .into_iter()
        .skip(request.offset)
        .take(request.limit)
        .map(|path| {
            file_match(
                path.file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into(),
                path.metadata().ok().as_ref(),
            )
        })
        .collect::<Vec<_>>();
    Ok(FindFilesResult {
        bounds: bounds(Some(total), total, request.offset, items.len(), false),
        items,
        engine: Some("directory".into()),
        fallback_reason: None,
    })
}
fn bounds(
    total: Option<usize>,
    at_least: usize,
    offset: usize,
    count: usize,
    timed_out: bool,
) -> WorkspaceSearchBounds {
    let truncated = offset.saturating_add(count) < at_least;
    WorkspaceSearchBounds {
        total,
        total_at_least: at_least,
        next_cursor: truncated.then_some(offset + count),
        truncated,
        timed_out,
    }
}
fn file_match(path: String, metadata: Option<&Metadata>) -> WorkspaceFileMatch {
    WorkspaceFileMatch {
        path,
        score: 0,
        git_status: None,
        size: metadata.map_or(0, Metadata::len),
        modified: metadata
            .and_then(|m| m.modified().ok())
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_secs()),
    }
}
fn grep_match(
    path: &str,
    line: u64,
    text: &str,
    fuzzy_score: Option<u16>,
) -> WorkspaceGrepMatch {
    WorkspaceGrepMatch {
        path: path.into(),
        line,
        text: truncate_line(text),
        definition: fff_search::is_definition_line(text),
        fuzzy_score,
        context_before: Vec::new(),
        context_after: Vec::new(),
    }
}
fn truncate_line(line: &str) -> String {
    const MAX: usize = 4_000;
    if line.len() <= MAX {
        return line.into();
    }
    let mut end = MAX;
    while !line.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &line[..end])
}
fn relative_path(root: &Path, path: &Path) -> Option<String> {
    let path = path
        .strip_prefix(root)
        .ok()?
        .to_string_lossy()
        .replace('\\', "/");
    (!path.is_empty()).then_some(path)
}
fn discoverable_path(path: &str, include_hidden: bool) -> bool {
    let components = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .collect::<Vec<_>>();
    if components
        .iter()
        .any(|component| component.eq_ignore_ascii_case(".git"))
    {
        return false;
    }
    if components.windows(2).any(|pair| {
        pair[0].eq_ignore_ascii_case(".neoism") && pair[1].eq_ignore_ascii_case("cache")
    }) {
        return false;
    }
    include_hidden
        || !components
            .iter()
            .any(|component| component.starts_with('.'))
}
// FFF's WatchOptions::ignore filters subscription delivery only, not the
// indexer's event processing/logging. Until FFF exposes index-level exclusions,
// roots overlapping our logs must use fresh watcherless snapshots, even pinned.
fn watch_safe_root(root: &Path) -> bool {
    !broad_root(root) && neoism_log_dir().is_none_or(|logs| !paths_overlap(root, &logs))
}
fn paths_overlap(root: &Path, logs: &Path) -> bool {
    let root = canonical_root(root);
    let logs = canonical_root(logs);
    logs.starts_with(&root) || root.starts_with(&logs)
}
fn neoism_log_dir() -> Option<PathBuf> {
    let config = std::env::var("NEOISM_CONFIG_HOME").ok().map(PathBuf::from);
    let config = config.or_else(|| {
        #[cfg(target_os = "windows")]
        {
            dirs::home_dir().map(|home| home.join("AppData/Local/neoism"))
        }
        #[cfg(target_os = "macos")]
        {
            dirs::home_dir().map(|home| home.join(".config/neoism"))
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos")))]
        {
            std::env::var("XDG_CONFIG_HOME")
                .ok()
                .map(PathBuf::from)
                .or_else(|| dirs::home_dir().map(|home| home.join(".config")))
                .map(|config| config.join("neoism"))
        }
    });
    config.map(|config| config.join("log"))
}
fn broad_root(root: &Path) -> bool {
    let root = canonical_root(root);
    is_filesystem_root(&root)
        || dirs::home_dir()
            .map(|p| canonical_root(&p) == root)
            .unwrap_or(false)
}
fn canonical_root(root: &Path) -> PathBuf {
    dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}
fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}
fn configured_capacity() -> usize {
    std::env::var("NEOISM_FFF_PICKER_CACHE_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_CAPACITY)
        .clamp(1, MAX_CAPACITY)
}
fn env_flag(name: &str) -> bool {
    std::env::var_os(name).as_deref().is_some_and(|v| {
        matches!(
            v.to_string_lossy().as_ref(),
            "1" | "true" | "TRUE" | "yes" | "YES"
        )
    })
}
fn service_error(error: anyhow::Error) -> ServiceError {
    ServiceError::new(error.to_string())
}
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).into())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".into())
}
fn grep_mode(mode: WorkspaceSearchMode, pattern: &str) -> GrepMode {
    match mode {
        WorkspaceSearchMode::Regex => GrepMode::Regex,
        WorkspaceSearchMode::Fuzzy => GrepMode::Fuzzy,
        WorkspaceSearchMode::Plain => GrepMode::PlainText,
        WorkspaceSearchMode::Auto if has_regex_metacharacters(pattern) => GrepMode::Regex,
        WorkspaceSearchMode::Auto => GrepMode::PlainText,
    }
}
fn mode_label(mode: GrepMode) -> &'static str {
    match mode {
        GrepMode::PlainText => "plain",
        GrepMode::Regex => "regex",
        GrepMode::Fuzzy => "fuzzy",
    }
}
fn git_status_label(status: git2::Status) -> String {
    if status.is_wt_new() {
        "untracked"
    } else if status.is_wt_modified() || status.is_index_modified() {
        "modified"
    } else if status.is_index_new() {
        "staged"
    } else if status.is_wt_deleted() || status.is_index_deleted() {
        "deleted"
    } else if status.is_index_renamed() || status.is_wt_renamed() {
        "renamed"
    } else {
        "tracked"
    }
    .into()
}
fn check_cancel(
    cancel: Option<&Arc<std::sync::atomic::AtomicBool>>,
    tool: &str,
) -> anyhow::Result<()> {
    if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
        anyhow::bail!("{tool} aborted");
    }
    Ok(())
}
// FFF does not export its line-level fuzzy scorer. Reuse its neo_frizbee
// implementation and mirror the verification rules in fff-search's pinned
// grep/fuzzy_grep.rs. Compatibility tests compare scores against picker.grep.
struct FffFuzzyLineMatcher {
    matcher: neo_frizbee::Matcher,
    needle_len: usize,
    max_typos: usize,
}
impl FffFuzzyLineMatcher {
    fn new(needle: &str, case_insensitive: bool) -> anyhow::Result<Self> {
        anyhow::ensure!(
            needle.len() <= 1024,
            "fuzzy grep pattern must be at most 1024 bytes"
        );
        let max_typos = (needle.len() / 3).min(2);
        let matcher = neo_frizbee::Matcher::new(
            needle,
            &neo_frizbee::Config {
                max_typos: Some(max_typos as u16),
                casing: if case_insensitive {
                    neo_frizbee::CaseMatching::Ignore
                } else {
                    neo_frizbee::CaseMatching::Respect
                },
                sort: false,
                scoring: neo_frizbee::Scoring {
                    exact_match_bonus: 100,
                    prefix_bonus: 0,
                    capitalization_bonus: if case_insensitive { 0 } else { 4 },
                    ..Default::default()
                },
                ..Default::default()
            },
        );
        Ok(Self {
            matcher,
            needle_len: needle.len(),
            max_typos,
        })
    }
    fn score(&mut self, line: &str) -> Option<u16> {
        let mut matched = self
            .matcher
            .match_list_indices(&[line])
            .into_iter()
            .next()?;
        if (matched.score as usize) < self.needle_len * 8 {
            return None;
        }
        // FFF re-scores its UTF-8-safe 512-byte display prefix before verifying
        // indices. Keep the adapter's longer display without changing scoring.
        let mut end = line.len().min(512);
        while !line.is_char_boundary(end) {
            end -= 1;
        }
        if end < line.len() {
            matched = self
                .matcher
                .match_list_indices(&[&line[..end]])
                .into_iter()
                .next()?;
        }
        matched.indices.sort_unstable();
        if matched.indices.len() < self.needle_len.saturating_sub(self.max_typos).max(1) {
            return None;
        }
        if let (Some(first), Some(last)) =
            (matched.indices.first(), matched.indices.last())
        {
            let span = last - first + 1;
            if span > self.needle_len * 3 {
                return None;
            }
            let min_density = if matched.indices.len() >= self.needle_len {
                45
            } else {
                65
            };
            if matched.indices.len() * 100 / span < min_density {
                return None;
            }
            let gaps = matched
                .indices
                .windows(2)
                .filter(|w| w[1] != w[0] + 1)
                .count();
            if gaps > (self.needle_len / 3).max(2) {
                return None;
            }
        }
        Some(matched.score)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root {
        fn new(label: &str) -> Self {
            let path = std::env::temp_dir().join(format!(
                "agent-fff-{label}-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
    }
    impl Drop for Root {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[test]
    fn registry_is_bounded_and_lru() {
        let a = Root::new("a");
        let b = Root::new("b");
        let c = Root::new("c");
        let r = PickerRegistry::new(2);
        r.warm(&a.0).unwrap();
        r.warm(&b.0).unwrap();
        r.warm(&a.0).unwrap();
        r.warm(&c.0).unwrap();
        assert_eq!(r.len(), 2);
        assert!(r.contains(&a.0));
        assert!(!r.contains(&b.0));
        assert!(r.contains(&c.0));
    }
    #[test]
    fn pin_lifetime_controls_eviction() {
        let a = Root::new("pin");
        let b = Root::new("other");
        let service = FffWorkspaceSearchService::with_capacity(1);
        service.warm(&a.0).unwrap();
        let pin = service.pin_root(&a.0).unwrap();
        service.warm(&b.0).unwrap();
        assert!(service.registry.contains(&a.0));
        drop(pin);
        service.warm(&b.0).unwrap();
        assert!(service.registry.contains(&b.0));
    }
    fn grep_request(root: &Root) -> GrepWorkspaceRequest {
        GrepWorkspaceRequest {
            root: root.0.clone(),
            path: root.0.clone(),
            patterns: vec!["needle".into()],
            include: None,
            include_hidden: false,
            excludes: Vec::new(),
            context_lines: 1,
            case_sensitive: true,
            mode: WorkspaceSearchMode::Plain,
            limit: 1,
            cursor: None,
            control: Default::default(),
        }
    }

    #[test]
    fn grep_case_policy_is_authoritative_in_all_modes_and_engines() {
        let root = Root::new("grep-explicit-case");
        let file = root.0.join("case.txt");
        std::fs::write(&file, "workspace\nWORKSPACE\n").unwrap();
        let service = FffWorkspaceSearchService::new();
        for mode in [
            WorkspaceSearchMode::Plain,
            WorkspaceSearchMode::Regex,
            WorkspaceSearchMode::Fuzzy,
            WorkspaceSearchMode::Auto,
        ] {
            // Recursive indexed, recursive walk, and exact-file paths must all
            // respect the same boolean contract, including uppercase false.
            for (hidden, exact_file) in [(false, false), (true, false), (false, true)] {
                for pattern in ["WORKSPACE", "workspace"] {
                    for sensitive in [false, true] {
                        let mut request = grep_request(&root);
                        request.path = if exact_file {
                            file.clone()
                        } else {
                            root.0.clone()
                        };
                        request.patterns = vec![pattern.into()];
                        request.mode = mode;
                        request.case_sensitive = sensitive;
                        request.include_hidden = hidden;
                        request.limit = 10;
                        let page = service.grep(&request).unwrap();
                        let lines: Vec<_> =
                            page.items.iter().map(|item| item.line).collect();
                        assert_eq!(
                            lines,
                            if !sensitive { vec![1, 2] } else if pattern == "WORKSPACE" { vec![2] } else { vec![1] },
                            "mode={mode:?}, hidden={hidden}, exact_file={exact_file}, pattern={pattern}, sensitive={sensitive}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn grep_pages_preserve_all_same_file_matches_and_context_in_both_engines() {
        for hidden in [false, true] {
            let root = Root::new("grep-pages");
            let text = format!("before\n{}after\n", "needle\n".repeat(250));
            for name in ["a.txt", "b.txt", "c.txt"] {
                std::fs::write(root.0.join(name), &text).unwrap();
            }
            let service = FffWorkspaceSearchService::new();
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            let mut seen = HashSet::new();
            let mut count = 0;
            for page_number in 0..800 {
                request.limit = [1, 7, 17][page_number % 3];
                let page = service.grep(&request).unwrap();
                assert_eq!(
                    page.engine.as_deref(),
                    Some(if hidden {
                        STREAMING_ENGINE_ID
                    } else {
                        ENGINE_ID
                    })
                );
                assert!(!page.items.is_empty());
                assert!(
                    page.items.len() <= request.limit,
                    "limit is a hard match cap"
                );
                for item in page.items {
                    assert_eq!(
                        item.context_before,
                        [if item.line == 2 { "before" } else { "needle" }]
                    );
                    assert_eq!(
                        item.context_after,
                        [if item.line == 251 { "after" } else { "needle" }]
                    );
                    assert!(seen.insert((item.path.clone(), item.line)));
                    count += 1;
                }
                assert_eq!(page.bounds.truncated, page.next_cursor.is_some());
                let Some(next) = page.next_cursor else { break };
                request.cursor = Some(next);
            }
            assert_eq!(count, 750);
        }
    }

    #[test]
    fn grep_pages_obey_output_budgets_without_losing_utf8_or_context() {
        for (context, wide) in [(3, true), (100, false)] {
            for hidden in [false, true] {
                let root = Root::new("grep-output-budget");
                let lines = (0..220)
                    .map(|n| {
                        format!(
                            "needle {n} {}",
                            if wide { "Ω".repeat(800) } else { "λ".into() }
                        )
                    })
                    .collect::<Vec<_>>();
                std::fs::write(root.0.join("utf8.txt"), lines.join("\n")).unwrap();
                let service = FffWorkspaceSearchService::new();
                let mut request = grep_request(&root);
                request.context_lines = context;
                request.include_hidden = hidden;
                request.limit = 1_000;
                let mut count = 0;
                let mut pages = 0;
                loop {
                    let page = service.grep(&request).unwrap();
                    assert!(page.items.len() <= request.limit);
                    let costs =
                        page.items.iter().map(grep_record_cost).collect::<Vec<_>>();
                    assert!(
                        costs.iter().map(|(bytes, _)| bytes).sum::<usize>()
                            <= GREP_PAGE_BYTES
                    );
                    assert!(
                        costs.iter().map(|(_, rows)| rows).sum::<usize>()
                            <= GREP_PAGE_ROWS
                    );
                    for item in &page.items {
                        let index = item.line as usize - 1;
                        assert_eq!(index, count);
                        assert_eq!(item.text, lines[index]);
                        assert_eq!(
                            item.context_before,
                            lines[index.saturating_sub(context)..index]
                        );
                        assert_eq!(
                            item.context_after,
                            lines[index + 1..(index + context + 1).min(lines.len())]
                        );
                        count += 1;
                    }
                    pages += 1;
                    assert!(pages < 220);
                    let Some(next) = page.next_cursor else { break };
                    request.cursor = Some(next);
                }
                assert!(pages > 1);
                assert_eq!(count, 220);
            }
        }
    }

    #[test]
    fn exact_external_file_pages_preserve_crlf_utf8_context_and_byte_anchors() {
        let workspace = Root::new("exact-page-workspace");
        let outside = Root::new("exact-page-outside");
        let lines = ["needle α", "needle β", "needle γ", "needle δ", "needle ε"];
        let path = outside.0.join("unicode.txt");
        std::fs::write(&path, lines.join("\r\n")).unwrap();
        let service = FffWorkspaceSearchService::new();
        let mut request = grep_request(&workspace);
        request.path = path.clone();
        request.context_lines = 2;
        for index in 0..lines.len() {
            let page = service.grep(&request).unwrap();
            assert_eq!(page.items.len(), 1);
            let item = &page.items[0];
            assert_eq!(item.path, path.to_string_lossy());
            assert_eq!(item.line, index as u64 + 1);
            assert_eq!(item.text, lines[index]);
            assert_eq!(item.context_before, lines[index.saturating_sub(2)..index]);
            assert_eq!(
                item.context_after,
                lines[index + 1..(index + 3).min(lines.len())]
            );
            assert_eq!(page.next_cursor.is_some(), index + 1 < lines.len());
            request.cursor = page.next_cursor;
        }
    }

    #[test]
    fn grep_checkpoint_survives_unrelated_edits_but_rejects_excessive_context() {
        for hidden in [false, true] {
            let root = Root::new("grep-unrelated");
            std::fs::write(root.0.join("a.txt"), "needle\nneedle\nneedle\n").unwrap();
            std::fs::write(root.0.join("b.txt"), "unrelated\n").unwrap();
            let service = FffWorkspaceSearchService::new();
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            let first = service.grep(&request).unwrap();
            request.cursor = first.next_cursor;
            std::fs::write(root.0.join("b.txt"), "changed unrelated content\n").unwrap();
            let second = service.grep(&request).unwrap();
            assert_eq!(second.items[0].line, 2);
            assert_eq!(second.items.len(), 1);
            request.cursor = None;
            request.context_lines = MAX_GREP_CONTEXT + 1;
            assert!(service
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("context must be at most"));
        }
    }

    #[test]
    fn paged_grep_definition_flags_and_fuzzy_scores_match_fff() {
        let root = Root::new("fff-line-compatibility");
        // Use a same-case fixture so FFF's fuzzy scoring-only smart_case=false
        // policy and our strict sensitive policy agree. Mixed-case behavior is
        // covered separately by grep_case_policy_is_authoritative_in_all_modes_and_engines.
        let text = "pub struct schema {}\nlet schema = schema {};\npub(crate) async fn schema() {}\nconst OTHER: usize = 1;\nlet scattered = \"s________________c________________h________________e________________m________________a\";\n";
        std::fs::write(root.0.join("source.rs"), text).unwrap();
        let service = FffWorkspaceSearchService::new();
        for (mode, needle) in [
            (WorkspaceSearchMode::Plain, "schema"),
            (WorkspaceSearchMode::Fuzzy, "schema"),
            (WorkspaceSearchMode::Fuzzy, "shcema"),
            (WorkspaceSearchMode::Fuzzy, "schma"),
        ] {
            for case_sensitive in [false, true] {
                let expected = service
                    .with_picker(&root.0, |picker| {
                        let query =
                            QueryParser::new(fff_search::AiGrepConfig).parse(needle);
                        let result = picker.grep(
                            &query,
                            &fff_search::GrepSearchOptions {
                                mode: grep_mode(mode, needle),
                                smart_case: !case_sensitive,
                                classify_definitions: true,
                                max_matches_per_file: 100,
                                page_limit: 100,
                                ..Default::default()
                            },
                        );
                        result
                            .matches
                            .iter()
                            .map(|m| {
                                (
                                    m.line_number,
                                    (
                                        m.line_content.clone(),
                                        m.fuzzy_score,
                                        m.is_definition,
                                    ),
                                )
                            })
                            .collect::<std::collections::BTreeMap<_, _>>()
                    })
                    .unwrap();
                assert!(
                    !expected.is_empty(),
                    "compatibility fixture must exercise real FFF matches: {needle}"
                );
                for hidden in [false, true] {
                    let mut request = grep_request(&root);
                    request.include_hidden = hidden;
                    request.patterns = vec![needle.into()];
                    request.mode = mode;
                    request.case_sensitive = case_sensitive;
                    let mut actual = std::collections::BTreeMap::new();
                    for _ in 0..10 {
                        let page = service.grep(&request).unwrap();
                        assert!(page.items.len() <= request.limit);
                        for item in page.items {
                            if let Some(score) = item.fuzzy_score {
                                assert_ne!(
                                    score as usize,
                                    item.text.chars().count(),
                                    "a score is not a character count"
                                );
                            }
                            assert!(actual
                                .insert(
                                    item.line,
                                    (item.text, item.fuzzy_score, item.definition)
                                )
                                .is_none());
                        }
                        let Some(next) = page.next_cursor else { break };
                        request.cursor = Some(next);
                    }
                    assert_eq!(
                        actual, expected,
                        "{needle}, hidden={hidden}, case_sensitive={case_sensitive}"
                    );
                    assert!(actual.values().any(|(_, _, definition)| *definition));
                }
            }
        }
    }

    #[test]
    fn internal_scope_filters_remain_workspace_relative_for_index_walk_and_exact_file() {
        let root = Root::new("workspace-relative-grep-filters");
        for name in [
            "src/one.rs",
            "src/nested/two.rs",
            "src/skip.txt",
            "outside.rs",
        ] {
            let path = root.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "needle\n").unwrap();
        }
        let service = FffWorkspaceSearchService::new();
        for hidden in [false, true] {
            for (scope, count) in
                [("src", 2), ("src/one.rs", 1), ("src/nested/two.rs", 1)]
            {
                let mut request = grep_request(&root);
                request.path = root.0.join(scope);
                request.include_hidden = hidden;
                request.include = Some("src/**/*.rs".into());
                request.limit = 20;
                let result = service.grep(&request).unwrap();
                assert_eq!(result.items.len(), count, "{scope} hidden={hidden}");
                assert!(result
                    .items
                    .iter()
                    .all(|item| item.path.starts_with("src/")
                        && item.path.ends_with(".rs")));
                request.excludes = vec!["src/**".into()];
                assert!(
                    service.grep(&request).unwrap().items.is_empty(),
                    "workspace exclusion must not become scope-relative"
                );
            }
        }
    }

    #[test]
    fn random_grep_cursors_reject_tampering_forgery_foreign_service_and_expiry() {
        for hidden in [false, true] {
            let root = Root::new("opaque-grep-cursors");
            std::fs::write(root.0.join("one.txt"), "needle α\nneedle β\nneedle γ\n")
                .unwrap();
            let service = FffWorkspaceSearchService::new();
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            let token = service.grep(&request).unwrap().next_cursor.unwrap();
            assert_eq!(token.len(), 64);
            assert!(!token.contains(':'));
            let mut changed = token.clone();
            changed.replace_range(0..1, if token.starts_with('0') { "1" } else { "0" });
            request.cursor = Some(changed);
            assert!(service
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("invalid"));
            request.cursor = Some(format!(
                "g3:{:016x}:walk:0:999999999999:0:777:0:0",
                query_hash(&request)
            ));
            assert!(service
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("invalid"));
            request.cursor = Some(token.clone());
            assert!(FffWorkspaceSearchService::new()
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("invalid"));
            for _ in 0..2 {
                let next = service.grep(&request).unwrap();
                assert_eq!(next.items[0].line, 2);
                assert_eq!(next.items[0].text, "needle β");
                assert_eq!(next.items[0].context_before, ["needle α"]);
            }
            service
                .registry
                .state
                .lock()
                .unwrap()
                .grep_cursors
                .get_mut(&token)
                .unwrap()
                .0 = Instant::now() - GREP_CURSOR_TTL - Duration::from_secs(1);
            assert!(service
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("expired"));
        }
    }

    #[test]
    fn grep_checkpoint_cache_is_bounded_and_only_issues_page_checkpoints() {
        let root = Root::new("grep-cache-bound");
        std::fs::write(root.0.join("one.txt"), "needle\n".repeat(250)).unwrap();
        let service = FffWorkspaceSearchService::new();
        let mut request = grep_request(&root);
        request.limit = 1_000;
        request.context_lines = 0;
        let result = service.grep(&request).unwrap();
        assert!(result.next_cursor.is_none());
        assert!(
            service
                .registry
                .state
                .lock()
                .unwrap()
                .grep_cursors
                .is_empty(),
            "do not cache per-match checkpoints"
        );
        request.limit = 1;
        let first = service.grep(&request).unwrap().next_cursor.unwrap();
        let checkpoint = service.registry.state.lock().unwrap().grep_cursors[&first]
            .1
            .clone();
        for _ in 0..MAX_GREP_CURSORS {
            checkpoint.encode(&service.registry).unwrap();
        }
        let state = service.registry.state.lock().unwrap();
        assert_eq!(state.grep_cursors.len(), MAX_GREP_CURSORS);
        assert!(!state.grep_cursors.contains_key(&first));
        drop(state);
        request.cursor = Some(first);
        assert!(service
            .grep(&request)
            .unwrap_err()
            .to_string()
            .contains("expired"));
    }

    #[test]
    fn first_request_falls_back_when_index_is_empty_or_has_no_searchable_files() {
        for empty in [false, true] {
            let root = Root::new("empty-index-fallback");
            std::fs::create_dir_all(root.0.join("src")).unwrap();
            if !empty {
                std::fs::write(root.0.join("src/ignored.txt"), "needle\n").unwrap();
            }
            let snapshot = build_picker(&root.0, false).unwrap();
            assert!(snapshot.wait_for_scan(INITIAL_SCAN_WAIT));
            std::fs::write(root.0.join("src/new.rs"), "needle\n").unwrap();
            let service = FffWorkspaceSearchService::new();
            // Model a reusable index snapshot that has not received a new file
            // event. The actual picker is watcherless so this fixture is stable.
            let _pin = service.pin_root(&root.0).unwrap();
            service.registry.state.lock().unwrap().entries.insert(
                canonical_root(&root.0),
                Entry {
                    picker: snapshot,
                    generation: 1,
                    last_used: 0,
                    watching: true,
                },
            );
            let mut request = grep_request(&root);
            request.path = root.0.join("src");
            request.include = Some("src/**/*.rs".into());
            let result = service.grep(&request).unwrap();
            assert_eq!(result.engine.as_deref(), Some(STREAMING_ENGINE_ID));
            assert_eq!(
                result.fallback_reason.as_deref(),
                Some("indexed search returned no searchable files")
            );
            assert_eq!(result.items.len(), 1);
            assert_eq!(result.items[0].path, "src/new.rs");
        }
    }

    #[test]
    fn grep_cursor_rejects_changed_query_scope_and_stale_files() {
        for hidden in [false, true] {
            let root = Root::new("grep-stale");
            std::fs::write(root.0.join("a.txt"), "needle\n").unwrap();
            std::fs::write(root.0.join("b.txt"), "needle\n").unwrap();
            let service = FffWorkspaceSearchService::new();
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            let page = service.grep(&request).unwrap();
            request.cursor = page.next_cursor;
            assert!(request.cursor.is_some());
            let mut changed = request.clone();
            changed.patterns = vec!["different".into()];
            assert!(service
                .grep(&changed)
                .unwrap_err()
                .to_string()
                .contains("does not match"));
            changed = request.clone();
            changed.path = root.0.join("a.txt");
            assert!(service
                .grep(&changed)
                .unwrap_err()
                .to_string()
                .contains("does not match"));
            std::fs::write(root.0.join(&page.items[0].path), "modified longer needle\n")
                .unwrap();
            assert!(service
                .grep(&request)
                .unwrap_err()
                .to_string()
                .contains("stale"));
        }
    }

    #[test]
    fn external_grep_filters_and_pages_have_absolute_unambiguous_paths() {
        let workspace = Root::new("workspace");
        let outside = Root::new("outside");
        for name in [
            "a.txt",
            "b.txt",
            "skip.txt",
            "other.rs",
            "target/generated.txt",
            ".hidden.txt",
        ] {
            let path = outside.0.join(name);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "before\nneedle\nafter\n").unwrap();
        }
        let mut request = grep_request(&workspace);
        request.path = outside.0.clone();
        request.include = Some("*.txt".into());
        request.excludes = vec!["skip.txt".into()];
        let service = FffWorkspaceSearchService::new();
        let first = service.grep(&request).unwrap();
        assert_eq!(first.items.len(), 1);
        assert_eq!(
            first.items[0].path,
            outside.0.join("a.txt").to_string_lossy()
        );
        request.cursor = first.next_cursor;
        assert!(request.cursor.is_some());
        let second = service.grep(&request).unwrap();
        assert_eq!(second.items.len(), 1);
        assert_eq!(
            second.items[0].path,
            outside.0.join("b.txt").to_string_lossy()
        );
        assert!(second.next_cursor.is_none());
        request.cursor = None;
        request.path = outside.0.join("a.txt");
        let exact = service.grep(&request).unwrap();
        assert_eq!(exact.items.len(), 1);
        assert_eq!(exact.items[0].context_before, ["before"]);
        assert_eq!(exact.items[0].context_after, ["after"]);
    }

    #[test]
    fn explicit_exact_misses_never_fuzzy_fallback_or_parse_content_as_filters() {
        let root = Root::new("exact-mode");
        std::fs::write(
            root.0.join("a.txt"),
            "n e e d l e\n*.rs !literal source/path\n",
        )
        .unwrap();
        let service = FffWorkspaceSearchService::new();
        for hidden in [false, true] {
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            for mode in [WorkspaceSearchMode::Plain, WorkspaceSearchMode::Regex] {
                request.mode = mode;
                let result = service.grep(&request).unwrap();
                assert!(result.items.is_empty());
                assert!(!result.bounds.truncated);
                assert!(result.next_cursor.is_none());
            }
            request.mode = WorkspaceSearchMode::Plain;
            request.patterns = vec!["*.rs !literal source/path".into()];
            assert_eq!(service.grep(&request).unwrap().items.len(), 1);
            request.mode = WorkspaceSearchMode::Regex;
            request.patterns = vec!["[".into()];
            assert!(service.grep(&request).is_err());
        }
    }

    #[test]
    fn regex_arrays_are_regex_not_escaped_literals() {
        let root = Root::new("regex-array");
        std::fs::write(root.0.join("a.txt"), "alpha123\nbeta456\nnone\n").unwrap();
        let mut request = grep_request(&root);
        request.mode = WorkspaceSearchMode::Regex;
        request.limit = 2;
        request.patterns = vec!["^alpha[0-9]+$".into(), "^beta[0-9]+$".into()];
        assert_eq!(
            FffWorkspaceSearchService::new()
                .grep(&request)
                .unwrap()
                .items
                .len(),
            2
        );
    }

    #[test]
    fn grep_timeout_is_partial_without_an_unsafe_continuation() {
        let root = Root::new("grep-timeout");
        std::fs::write(root.0.join("a.txt"), "needle\n").unwrap();
        let service = FffWorkspaceSearchService::new();
        for hidden in [false, true] {
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            request.control.timeout_ms = 0;
            let result = service.grep(&request).unwrap();
            assert!(result.bounds.timed_out);
            assert!(result.bounds.truncated);
            assert!(result.next_cursor.is_none());
        }
    }

    #[test]
    fn grep_cancellation_during_scan_and_file_read_is_observed() {
        let root = Root::new("grep-cancel");
        std::fs::write(
            root.0.join("a.txt"),
            "a long nonmatching line\n".repeat(600_000),
        )
        .unwrap();
        for hidden in [false, true] {
            let mut request = grep_request(&root);
            request.include_hidden = hidden;
            let cancel = Arc::new(std::sync::atomic::AtomicBool::new(false));
            request.control.cancel = Some(cancel.clone());
            let trigger = std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(2));
                cancel.store(true, Ordering::SeqCst);
            });
            let result = FffWorkspaceSearchService::new().grep(&request);
            trigger.join().unwrap();
            assert!(result.unwrap_err().to_string().contains("aborted"));
        }
    }

    #[test]
    fn ad_hoc_indexes_are_watcherless_and_rebuilt_after_edits() {
        let root = Root::new("fresh");
        let service = FffWorkspaceSearchService::new();
        let find = || {
            service
                .find_files(&FindFilesRequest {
                    root: root.0.clone(),
                    query: "*.txt".into(),
                    include_hidden: false,
                    offset: 0,
                    limit: 20,
                    control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
                })
                .unwrap()
        };
        let grep = |pattern: &str| {
            service
                .grep(&GrepWorkspaceRequest {
                    root: root.0.clone(),
                    path: root.0.clone(),
                    patterns: vec![pattern.into()],
                    include: None,
                    excludes: Vec::new(),
                    include_hidden: false,
                    case_sensitive: true,
                    context_lines: 0,
                    mode: WorkspaceSearchMode::Plain,
                    cursor: None,
                    limit: 20,
                    control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
                })
                .unwrap()
        };
        std::fs::write(root.0.join("before.txt"), "old content\n").unwrap();
        service.warm(&root.0).unwrap();
        let (_, first, snapshot) = service.registry.picker(&root.0).unwrap();
        assert!(snapshot.wait_for_scan(INITIAL_SCAN_WAIT));
        assert!(!snapshot.read().unwrap().as_ref().unwrap().has_watcher());
        assert!(find().items.iter().any(|item| item.path == "before.txt"));
        assert!(grep("old content")
            .items
            .iter()
            .any(|item| item.text.contains("old content")));
        std::fs::write(root.0.join("before.txt"), "new needle content\n").unwrap();
        assert!(grep("new needle")
            .items
            .iter()
            .any(|item| item.text.contains("new needle")));
        std::fs::rename(root.0.join("before.txt"), root.0.join("after.txt")).unwrap();
        std::fs::write(root.0.join("added.txt"), "new file\n").unwrap();
        let result = find();
        assert_eq!(result.engine.as_deref(), Some(ENGINE_ID));
        assert!(result.items.iter().any(|item| item.path == "after.txt"));
        assert!(result.items.iter().any(|item| item.path == "added.txt"));
        assert!(!result.items.iter().any(|item| item.path == "before.txt"));
        std::fs::remove_file(root.0.join("after.txt")).unwrap();
        assert!(!find().items.iter().any(|item| item.path == "after.txt"));
        let (_, last, _) = service.registry.picker(&root.0).unwrap();
        assert_ne!(first, last);
    }

    #[test]
    fn pins_promote_snapshots_reuse_workspace_index_and_retire_on_last_drop() {
        let root = Root::new("watch");
        let service = FffWorkspaceSearchService::new();
        let (_, snapshot_generation, snapshot) =
            service.registry.picker(&root.0).unwrap();
        assert!(!snapshot.read().unwrap().as_ref().unwrap().has_watcher());
        let pin = service.pin_root(&root.0).unwrap();
        let second_pin = service.pin_root(&root.0).unwrap();
        let (_, generation, watched) = service.registry.picker(&root.0).unwrap();
        assert_ne!(snapshot_generation, generation);
        assert!(watched.read().unwrap().as_ref().unwrap().has_watcher());
        assert_eq!(service.registry.picker(&root.0).unwrap().1, generation);
        drop(pin);
        assert_eq!(service.registry.picker(&root.0).unwrap().1, generation);
        assert!(watched.wait_for_scan(INITIAL_SCAN_WAIT));
        assert!(watched.wait_for_watcher(INITIAL_SCAN_WAIT));
        std::fs::write(root.0.join("live.txt"), "live edit\n").unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if watched
                .read()
                .unwrap()
                .as_ref()
                .unwrap()
                .get_file_by_path(root.0.join("live.txt"))
                .is_some()
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "workspace watcher missed a new file"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(service.registry.picker(&root.0).unwrap().1, generation);
        drop(second_pin);
        assert!(!service.registry.contains(&root.0));
        // Even a caller retaining the old shared handle cannot retain its monitor.
        assert!(!watched.read().unwrap().as_ref().unwrap().is_watcher_ready());
        let (_, next, snapshot) = service.registry.picker(&root.0).unwrap();
        assert_ne!(next, generation);
        assert!(!snapshot.read().unwrap().as_ref().unwrap().has_watcher());
    }

    #[test]
    fn releasing_pin_during_initial_scan_cannot_install_watcher_later() {
        let root = Root::new("unpin-scan");
        let service = FffWorkspaceSearchService::new();
        let pin = service.pin_root(&root.0).unwrap();
        let (_, _, picker) = service.registry.picker(&root.0).unwrap();
        drop(pin);
        assert!(!service.registry.contains(&root.0));
        assert!(picker.wait_for_indexing_complete(INITIAL_SCAN_WAIT));
        assert!(!picker.read().unwrap().as_ref().unwrap().is_watcher_ready());
    }

    #[test]
    fn log_overlap_disables_watching_even_when_pinned() {
        let root = Root::new("logs");
        let logs = root.0.join("neoism/log");
        std::fs::create_dir_all(&logs).unwrap();
        assert!(paths_overlap(&root.0, &logs));
        assert!(paths_overlap(&logs, &logs));
        assert!(paths_overlap(&logs.join("nested"), &logs));
        assert!(!paths_overlap(&root.0.join("project"), &logs));
        assert!(!paths_overlap(&root.0.join("neoism/log-other"), &logs));
        if let Some(logs) = neoism_log_dir() {
            assert!(!watch_safe_root(&logs));
            let service = FffWorkspaceSearchService::new();
            let _pin = service.pin_root(&logs).unwrap();
            // Do not scan the real log directory in this regression test.
            assert!(!watch_safe_root(&canonical_root(&logs)));
        }
        if let Some(home) = dirs::home_dir() {
            assert!(!watch_safe_root(&home));
        }
    }

    #[test]
    fn streaming_is_bounded_and_ignored() {
        let root = Root::new("stream");
        std::fs::create_dir_all(root.0.join("src")).unwrap();
        std::fs::create_dir_all(root.0.join("target")).unwrap();
        std::fs::write(root.0.join("src/lib.rs"), "needle\n").unwrap();
        std::fs::write(root.0.join("target/generated.rs"), "needle\n").unwrap();
        let request = FindFilesRequest {
            root: root.0.clone(),
            query: "*.rs".into(),
            include_hidden: false,
            offset: 0,
            limit: 10,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl {
                timeout_ms: 5000,
                cancel: None,
            },
        };
        let result = streaming_find(&request, "test").unwrap();
        assert!(result.items.iter().any(|i| i.path == "src/lib.rs"));
        assert!(!result.items.iter().any(|i| i.path.contains("target")));
    }
    #[test]
    fn hidden_discovery_is_explicit_and_never_walks_git_metadata() {
        let root = Root::new("hidden");
        std::fs::create_dir_all(root.0.join(".agent")).unwrap();
        std::fs::create_dir_all(root.0.join(".git")).unwrap();
        std::fs::write(root.0.join("visible.txt"), "visible needle\n").unwrap();
        std::fs::write(root.0.join(".agent/agent.txt"), "hidden needle\n").unwrap();
        std::fs::write(root.0.join(".git/config.txt"), "git needle\n").unwrap();
        let service = FffWorkspaceSearchService::new();
        let request = |include_hidden| FindFilesRequest {
            root: root.0.clone(),
            query: "*.txt".into(),
            include_hidden,
            offset: 0,
            limit: 20,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
        };
        let ordinary = service.find_files(&request(false)).unwrap();
        assert!(ordinary
            .items
            .iter()
            .all(|item| !item.path.starts_with('.')));
        let hidden = service.find_files(&request(true)).unwrap();
        assert!(hidden
            .items
            .iter()
            .any(|item| item.path == ".agent/agent.txt"));
        assert!(hidden
            .items
            .iter()
            .all(|item| !item.path.starts_with(".git/")));

        let grep = |path: PathBuf, include_hidden| GrepWorkspaceRequest {
            root: root.0.clone(),
            path,
            patterns: vec!["needle".into()],
            include: Some("*.txt".into()),
            include_hidden,
            excludes: DEFAULT_EXCLUDES.iter().map(|item| (*item).into()).collect(),
            context_lines: 0,
            case_sensitive: false,
            mode: WorkspaceSearchMode::Plain,
            cursor: None,
            limit: 20,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
        };
        let hidden_grep = service.grep(&grep(root.0.clone(), true)).unwrap();
        assert!(hidden_grep
            .items
            .iter()
            .any(|item| item.path == ".agent/agent.txt"));
        assert!(hidden_grep
            .items
            .iter()
            .all(|item| !item.path.starts_with(".git/")));
        let exact = service
            .grep(&grep(root.0.join(".agent/agent.txt"), false))
            .unwrap();
        assert_eq!(exact.items[0].path, ".agent/agent.txt");
    }
    #[test]
    fn indexed_find_and_grep_preserve_bounds_and_identity() {
        let root = Root::new("search");
        std::fs::create_dir_all(root.0.join("src")).unwrap();
        std::fs::write(root.0.join("src/upload.rs"), "pub struct PrepareUpload;\n")
            .unwrap();
        let service = FffWorkspaceSearchService::new();
        let control = neoism_agent_service_api::WorkspaceSearchRequestControl {
            timeout_ms: 5_000,
            cancel: None,
        };
        let found = service
            .find_files(&FindFilesRequest {
                root: root.0.clone(),
                query: "upload".into(),
                include_hidden: false,
                offset: 0,
                limit: 5,
                control: control.clone(),
            })
            .unwrap();
        assert_eq!(found.engine.as_deref(), Some(ENGINE_ID));
        assert_eq!(found.items[0].path, "src/upload.rs");
        let grep = service
            .grep(&GrepWorkspaceRequest {
                root: root.0.clone(),
                path: root.0.clone(),
                patterns: vec!["PrepareUpload".into()],
                include: Some("*.rs".into()),
                include_hidden: false,
                excludes: DEFAULT_EXCLUDES.iter().map(|s| (*s).into()).collect(),
                context_lines: 0,
                case_sensitive: false,
                mode: WorkspaceSearchMode::Plain,
                cursor: None,
                limit: 1,
                control,
            })
            .unwrap();
        assert_eq!(grep.engine.as_deref(), Some(ENGINE_ID));
        assert_eq!(grep.items[0].line, 1);
        assert!(!grep.bounds.truncated);
        assert!(grep.next_cursor.is_none());
    }

    #[test]
    fn scoped_directory_grep_does_not_escape_or_drop_matches() {
        let root = Root::new("scoped-grep");
        let scoped = root.0.join("scoped");
        std::fs::create_dir_all(&scoped).unwrap();
        std::fs::write(
            scoped.join("inside.rs"),
            "const NEEDLE: &str = \"inside\";\n",
        )
        .unwrap();
        std::fs::write(
            root.0.join("outside.rs"),
            "const NEEDLE: &str = \"outside\";\n",
        )
        .unwrap();
        let service = FffWorkspaceSearchService::new();
        let grep = service
            .grep(&GrepWorkspaceRequest {
                root: root.0.clone(),
                path: scoped,
                patterns: vec!["NEEDLE".into()],
                include: Some("*.rs".into()),
                include_hidden: false,
                excludes: DEFAULT_EXCLUDES.iter().map(|item| (*item).into()).collect(),
                context_lines: 0,
                case_sensitive: true,
                mode: WorkspaceSearchMode::Plain,
                cursor: None,
                limit: 20,
                control: Default::default(),
            })
            .unwrap();

        assert_eq!(grep.engine.as_deref(), Some(ENGINE_ID));
        assert_eq!(grep.items.len(), 1);
        assert_eq!(grep.items[0].path, "scoped/inside.rs");
        assert_eq!(grep.fallback_reason, None);
    }

    #[test]
    fn indexed_scope_preserves_modes_and_literal_directory_names() {
        for directory in ["scoped", "parent/scoped", "scope with spaces [x] {a,b}"] {
            let root = Root::new("scope-modes");
            for relative in [
                format!("{directory}/inside.rs"),
                format!("{directory}/nested/deep.rs"),
                format!("{directory}/skip.rs"),
                format!("{directory}/skip.txt"),
                format!("{directory}-sibling/outside.rs"),
                format!("other/{directory}/outside.rs"),
                "outside.rs".into(),
            ] {
                let path = root.0.join(relative);
                std::fs::create_dir_all(path.parent().unwrap()).unwrap();
                std::fs::write(path, "const NEEDLE: usize = 1;\n").unwrap();
            }
            let service = FffWorkspaceSearchService::new();
            let _pin = service.pin_root(&root.0).unwrap();
            for (mode, patterns, label) in [
                (WorkspaceSearchMode::Plain, vec!["NEEDLE".into()], "plain"),
                (WorkspaceSearchMode::Regex, vec!["NEE[D]LE".into()], "regex"),
                (WorkspaceSearchMode::Fuzzy, vec!["NEDLE".into()], "fuzzy"),
                (
                    WorkspaceSearchMode::Plain,
                    vec!["NEEDLE".into(), "absent".into()],
                    "multi",
                ),
            ] {
                for limit in [1, 20] {
                    let grep = service.grep(&GrepWorkspaceRequest {
                        root: root.0.clone(),
                        path: root.0.join(directory),
                        patterns: patterns.clone(),
                        include: Some("*.rs".into()),
                        include_hidden: false,
                        excludes: vec!["**/skip.rs".into()],
                        context_lines: 0,
                        case_sensitive: true,
                        mode,
                        cursor: None,
                        limit,
                        control: neoism_agent_service_api::WorkspaceSearchRequestControl::default(),
                    }).unwrap();
                    assert_eq!(grep.engine.as_deref(), Some(ENGINE_ID));
                    assert_eq!(grep.fallback_reason, None);
                    assert_eq!(grep.mode, label);
                    assert_eq!(grep.items.len(), limit.min(2), "{directory} {label}");
                    assert!(
                        grep.items.iter().all(|item| {
                            item.path == format!("{directory}/inside.rs")
                                || item.path == format!("{directory}/nested/deep.rs")
                        }),
                        "{directory} {label}: {:?}",
                        grep.items
                    );
                    if mode == WorkspaceSearchMode::Fuzzy {
                        assert!(grep.items.iter().all(|item| item.fuzzy_score.is_some()));
                    }
                }
            }
        }
    }

    #[test]
    fn cancellation_is_honored_by_streaming_fallback() {
        let root = Root::new("cancel");
        std::fs::write(root.0.join("file.txt"), "needle\n").unwrap();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let request = FindFilesRequest {
            root: root.0.clone(),
            query: "*".into(),
            include_hidden: false,
            offset: 0,
            limit: 10,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl {
                timeout_ms: 5_000,
                cancel: Some(cancel),
            },
        };
        assert!(streaming_find(&request, "test")
            .unwrap_err()
            .to_string()
            .contains("aborted"));
    }
}
