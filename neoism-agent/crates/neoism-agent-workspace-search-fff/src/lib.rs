//! FFF-backed workspace-search adapter for the transport-neutral Agent service API.

use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{File, Metadata};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

use anyhow::Context;
use fff_search::{
    has_regex_metacharacters, AiGrepConfig, FFFMode, FilePicker, FilePickerOptions,
    FuzzySearchOptions, GrepMode, GrepSearchOptions, PaginationArgs, QueryParser,
    SharedFilePicker, SharedFrecency,
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
    ".git", ".claude/worktrees", ".codex", ".neoism/cache", "target",
    "node_modules", "dist", ".tmp",
];

struct Entry { picker: SharedFilePicker, generation: u64, last_used: u64 }
#[derive(Default)]
struct RegistryState {
    entries: HashMap<PathBuf, Entry>, pins: HashMap<PathBuf, usize>,
    clock: u64, next_generation: u64,
}
struct PickerRegistry { capacity: usize, state: Mutex<RegistryState> }

impl PickerRegistry {
    fn new(capacity: usize) -> Self {
        Self { capacity: capacity.clamp(1, MAX_CAPACITY), state: Mutex::new(RegistryState::default()) }
    }
    fn picker(&self, root: &Path) -> anyhow::Result<(PathBuf, u64, SharedFilePicker)> {
        let root = canonical_root(root);
        let (picker, generation, evicted) = {
            let mut state = self.state.lock().map_err(|_| anyhow::anyhow!("workspace-search registry lock was poisoned"))?;
            state.clock = state.clock.wrapping_add(1);
            let used = state.clock;
            if let Some(entry) = state.entries.get_mut(&root) {
                entry.last_used = used;
                (entry.picker.clone(), entry.generation, Vec::new())
            } else {
                let picker = build_picker(&root)?;
                state.next_generation = state.next_generation.wrapping_add(1);
                let generation = state.next_generation;
                state.entries.insert(root.clone(), Entry { picker: picker.clone(), generation, last_used: used });
                let evicted = evict_lru(&mut state, self.capacity);
                (picker, generation, evicted)
            }
        };
        drop(evicted);
        Ok((root, generation, picker))
    }
    fn with_picker<T>(&self, root: &Path, operation: impl FnOnce(&FilePicker) -> T) -> anyhow::Result<T> {
        let (root, generation, shared) = self.picker(root)?;
        if !shared.wait_for_scan(INITIAL_SCAN_WAIT) {
            anyhow::bail!("workspace index for {} is still scanning; retry in a moment", root.display());
        }
        let outcome = {
            let guard = shared.read().map_err(|error| anyhow::anyhow!("workspace picker read lock failed: {error}"))?;
            let picker = guard.as_ref().ok_or_else(|| anyhow::anyhow!("workspace picker for {} was dropped", root.display()))?;
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| operation(picker)))
        };
        outcome.map_err(|payload| {
            let removed = self.state.lock().ok().and_then(|mut state| {
                state.entries.get(&root).is_some_and(|entry| entry.generation == generation)
                    .then(|| state.entries.remove(&root)).flatten()
            });
            drop(removed);
            anyhow::anyhow!("workspace search engine panicked ({}); narrow the path/pattern or lower the limit", panic_message(payload.as_ref()))
        })
    }
    fn warm(&self, root: &Path) -> anyhow::Result<()> { self.picker(root).map(|_| ()) }
    fn pin(self: &Arc<Self>, root: &Path) -> PickerRootPin {
        let root = canonical_root(root);
        if let Ok(mut state) = self.state.lock() { *state.pins.entry(root.clone()).or_default() += 1; }
        PickerRootPin { root, registry: self.clone() }
    }
    fn unpin(&self, root: &Path) {
        let removed = if let Ok(mut state) = self.state.lock() {
            if let Some(count) = state.pins.get_mut(root) {
                *count = count.saturating_sub(1);
                if *count == 0 { state.pins.remove(root); }
            }
            evict_lru(&mut state, self.capacity)
        } else { Vec::new() };
        drop(removed);
    }
    #[cfg(test)] fn len(&self) -> usize { self.state.lock().map(|s| s.entries.len()).unwrap_or_default() }
    #[cfg(test)] fn contains(&self, root: &Path) -> bool { self.state.lock().map(|s| s.entries.contains_key(&canonical_root(root))).unwrap_or(false) }
}

fn evict_lru(state: &mut RegistryState, capacity: usize) -> Vec<Entry> {
    let mut removed = Vec::new();
    while state.entries.len() > capacity {
        let Some(root) = state.entries.iter().filter(|(root, _)| !state.pins.contains_key(*root))
            .min_by_key(|(_, entry)| entry.last_used).map(|(root, _)| root.clone()) else { break; };
        if let Some(entry) = state.entries.remove(&root) { removed.push(entry); }
    }
    removed
}

struct PickerRootPin { root: PathBuf, registry: Arc<PickerRegistry> }
impl Drop for PickerRootPin { fn drop(&mut self) { self.registry.unpin(&self.root); } }
impl WorkspaceSearchRootPin for PickerRootPin { fn root(&self) -> &Path { &self.root } }

/// Instance-owned FFF adapter. Each instance has independent indexes and pins.
#[derive(Clone)]
pub struct FffWorkspaceSearchService { registry: Arc<PickerRegistry> }
impl FffWorkspaceSearchService {
    pub fn new() -> Self { Self::with_capacity(configured_capacity()) }
    pub fn with_capacity(capacity: usize) -> Self { Self { registry: Arc::new(PickerRegistry::new(capacity)) } }
    pub fn with_picker<T>(&self, root: &Path, operation: impl FnOnce(&FilePicker) -> T) -> anyhow::Result<T> {
        self.registry.with_picker(root, operation)
    }
}
impl Default for FffWorkspaceSearchService { fn default() -> Self { Self::new() } }

impl WorkspaceSearchService for FffWorkspaceSearchService {
    fn warm(&self, root: &Path) -> Result<(), ServiceError> { self.registry.warm(root).map_err(service_error) }
    fn pin_root(&self, root: &Path) -> Result<Arc<dyn WorkspaceSearchRootPin>, ServiceError> {
        Ok(Arc::new(self.registry.pin(root)))
    }
    fn find_files(&self, request: &FindFilesRequest) -> Result<FindFilesResult, ServiceError> {
        find_files(&self.registry, request).map_err(service_error)
    }
    fn grep(&self, request: &GrepWorkspaceRequest) -> Result<GrepWorkspaceResult, ServiceError> {
        grep_workspace(&self.registry, request).map_err(service_error)
    }
    fn search_directories(&self, request: &DirectorySearchRequest) -> Result<DirectorySearchResult, ServiceError> {
        search_directories(&self.registry, request).map_err(service_error)
    }
}

fn find_files(registry: &PickerRegistry, request: &FindFilesRequest) -> anyhow::Result<FindFilesResult> {
    if request.query.trim().is_empty() { return directory_entries(request); }
    if broad_root(&request.root) { return streaming_find(request, "indexed search is disabled for home and filesystem roots"); }
    let started = Instant::now();
    match registry.with_picker(&request.root, |picker| {
        let parser = QueryParser::default();
        let mut results = picker.fuzzy_search(&parser.parse(&request.query), None, FuzzySearchOptions {
            max_threads: 0, current_file: None, project_path: Some(&request.root),
            pagination: PaginationArgs { offset: request.offset, limit: request.limit }, ..Default::default()
        });
        if results.items.is_empty() {
            if let Some(token) = request.query.split_whitespace().filter(|s| s.len() >= 3).max_by_key(|s| s.len()) {
                if token != request.query { results = picker.fuzzy_search(&parser.parse(token), None, FuzzySearchOptions {
                    max_threads: 0, current_file: None, project_path: Some(&request.root),
                    pagination: PaginationArgs { offset: request.offset, limit: request.limit }, ..Default::default()
                }); }
            }
        }
        let items = results.items.iter().zip(&results.scores).map(|(item, score)| WorkspaceFileMatch {
            path: item.relative_path(picker), score: score.total,
            git_status: item.git_status.map(git_status_label), size: item.size, modified: item.modified,
        }).collect::<Vec<_>>();
        (items, results.total_matched)
    }) {
        Ok((items, total)) => Ok(FindFilesResult {
            bounds: bounds(Some(total), total, request.offset, items.len(), false), items,
            engine: Some(ENGINE_ID.into()), fallback_reason: None,
        }),
        Err(error) => {
            let mut fallback = request.clone();
            fallback.control.timeout_ms = request.control.timeout_ms.saturating_sub(started.elapsed().as_millis() as u64).saturating_sub(500).max(1);
            streaming_find(&fallback, &error.to_string())
        }
    }
}

fn search_directories(registry: &PickerRegistry, request: &DirectorySearchRequest) -> anyhow::Result<DirectorySearchResult> {
    registry.with_picker(&request.root, |picker| {
        let parser = QueryParser::new(fff_search::DirSearchConfig);
        let results = picker.fuzzy_search_directories(&parser.parse(&request.query), FuzzySearchOptions {
            max_threads: 0, project_path: Some(&request.root),
            pagination: PaginationArgs { offset: request.offset, limit: request.limit }, ..Default::default()
        });
        let paths = results.items.iter().map(|item| item.relative_path(picker)).collect::<Vec<_>>();
        DirectorySearchResult { bounds: bounds(Some(results.total_matched), results.total_matched, request.offset, paths.len(), false), paths, engine: Some(ENGINE_ID.into()) }
    })
}

fn grep_workspace(registry: &PickerRegistry, request: &GrepWorkspaceRequest) -> anyhow::Result<GrepWorkspaceResult> {
    if broad_root(&request.root) { return streaming_grep(request, "indexed search is disabled for home and filesystem roots"); }
    let pattern = request.patterns.first().map(String::as_str).unwrap_or("");
    let requested = grep_mode(request.mode, pattern);
    let query_text = grep_query(request, if request.patterns.len() == 1 { pattern } else { "" });
    let parser = QueryParser::<AiGrepConfig>::new(AiGrepConfig);
    let query = parser.parse(&query_text);
    let options = |mode| GrepSearchOptions {
        page_limit: request.limit, mode, smart_case: !request.case_sensitive,
        before_context: request.context_lines, after_context: request.context_lines,
        classify_definitions: true, trim_whitespace: false,
        time_budget_ms: request.control.timeout_ms.saturating_sub(2_000).max(500),
        abort_signal: request.control.cancel.clone(), ..Default::default()
    };
    match registry.with_picker(&request.root, |picker| {
        let (results, mode) = if request.patterns.len() > 1 {
            let refs = request.patterns.iter().map(String::as_str).collect::<Vec<_>>();
            (picker.multi_grep(&refs, &query.constraints, &options(GrepMode::PlainText)), "multi".to_string())
        } else {
            let mut results = picker.grep(&query, &options(requested));
            let mut label = mode_label(requested).to_string();
            if results.matches.is_empty() && requested != GrepMode::Fuzzy && pattern.len() <= 1024 {
                let fuzzy = picker.grep(&query, &options(GrepMode::Fuzzy));
                if !fuzzy.matches.is_empty() { results = fuzzy; label = "fuzzy".into(); }
            }
            (results, label)
        };
        let items = results.matches.iter().filter_map(|item| {
            let file = results.files.get(item.file_index)?;
            Some(WorkspaceGrepMatch { path: file.relative_path(picker), line: item.line_number,
                text: truncate_line(&item.line_content), definition: item.is_definition, fuzzy_score: item.fuzzy_score })
        }).collect::<Vec<_>>();
        (items, results.files_with_matches, results.total_files_searched, results.next_file_offset, mode)
    }) {
        Ok((mut items, files, searched, next, mode)) if !items.is_empty() || searched > 0 => {
            let overflow = items.len() > request.limit; items.truncate(request.limit);
            Ok(GrepWorkspaceResult { bounds: WorkspaceSearchBounds { total: None, total_at_least: items.len(), next_cursor: (next != 0).then_some(next), truncated: next != 0 || overflow || items.len() >= request.limit, timed_out: false }, items, files_with_matches: files, total_files_searched: searched, mode, engine: Some(ENGINE_ID.into()), fallback_reason: None })
        }
        Ok(_) => streaming_grep(request, "indexed search returned no searchable files"),
        Err(error) => streaming_grep(request, &error.to_string()),
    }
}

fn streaming_find(request: &FindFilesRequest, reason: &str) -> anyhow::Result<FindFilesResult> {
    let deadline = Instant::now() + Duration::from_millis(request.control.timeout_ms);
    let matcher = PathMatcher::query(&request.query)?;
    let excluded = PathMatcher::patterns(DEFAULT_EXCLUDES.iter().copied())?;
    let root = request.root.clone();
    let (filter_root, filter_excluded) = (root.clone(), excluded.clone());
    let mut builder = WalkBuilder::new(&root);
    builder.hidden(true).git_ignore(true).git_exclude(true).git_global(true).ignore(true).follow_links(false)
        .filter_entry(move |entry| relative_path(&filter_root, entry.path()).is_none_or(|path| !filter_excluded.matches(&path)));
    let wanted = request.offset.saturating_add(request.limit).saturating_add(1);
    let mut items = Vec::new(); let mut timed_out = false;
    for entry in builder.build() {
        check_cancel(request.control.cancel.as_ref(), "glob")?;
        if Instant::now() >= deadline { timed_out = true; break; }
        let Ok(entry) = entry else { continue }; if !entry.file_type().is_some_and(|t| t.is_file()) { continue; }
        let Some(path) = relative_path(&root, entry.path()) else { continue }; if !matcher.matches(&path) { continue; }
        let metadata = entry.metadata().ok();
        items.push(file_match(path, metadata.as_ref()));
        if items.len() >= wanted { break; }
    }
    let discovered = items.len();
    let items = items.into_iter().skip(request.offset).take(request.limit).collect::<Vec<_>>();
    Ok(FindFilesResult { bounds: WorkspaceSearchBounds { total: None, total_at_least: discovered, next_cursor: None, truncated: timed_out || discovered > request.offset + items.len(), timed_out }, items, engine: Some(STREAMING_ENGINE_ID.into()), fallback_reason: Some(reason.into()) })
}

fn streaming_grep(request: &GrepWorkspaceRequest, reason: &str) -> anyhow::Result<GrepWorkspaceResult> {
    let deadline = Instant::now() + Duration::from_millis(request.control.timeout_ms);
    let matcher = LineMatcher::new(&request.patterns, request.mode, request.case_sensitive)?;
    let include = request.include.as_deref().map(PathMatcher::pattern).transpose()?;
    let excluded = PathMatcher::patterns(request.excludes.iter().map(String::as_str))?;
    let root = request.root.clone();
    let mut paths = Vec::new();
    if request.path.is_file() { paths.push(request.path.clone()); }
    else {
        let (filter_root, filter_excluded) = (root.clone(), excluded.clone());
        let mut builder = WalkBuilder::new(&request.path);
        builder.hidden(false).git_ignore(true).git_exclude(true).git_global(true).ignore(true).follow_links(false).max_filesize(Some(MAX_SEARCH_FILE_BYTES))
            .filter_entry(move |entry| relative_path(&filter_root, entry.path()).is_none_or(|path| !filter_excluded.matches(&path)));
        paths.extend(builder.build().filter_map(Result::ok).filter(|e| e.file_type().is_some_and(|t| t.is_file())).map(|e| e.into_path()));
    }
    let mut items = Vec::new(); let mut files = HashSet::new(); let mut searched = 0usize; let mut timed_out = false;
    for path in paths {
        check_cancel(request.control.cancel.as_ref(), "grep")?;
        if Instant::now() >= deadline { timed_out = true; break; }
        let Some(relative) = relative_path(&root, &path) else { continue };
        if excluded.matches(&relative) || include.as_ref().is_some_and(|m| !m.matches(&relative)) { continue; }
        let Ok(metadata) = path.metadata() else { continue }; if metadata.len() > MAX_SEARCH_FILE_BYTES { continue; }
        let Ok(file) = File::open(&path) else { continue }; searched += 1;
        let mut reader = BufReader::new(file); let mut bytes = Vec::new(); let mut line_no = 0u64;
        let mut before = VecDeque::<(u64, String)>::new(); let mut after = 0usize; let mut last = 0u64;
        loop {
            if items.len() > request.limit { break; }
            check_cancel(request.control.cancel.as_ref(), "grep")?;
            if Instant::now() >= deadline { timed_out = true; break; }
            bytes.clear(); if reader.read_until(b'\n', &mut bytes)? == 0 { break; } if bytes.contains(&0) { break; }
            line_no += 1; while bytes.last().is_some_and(|b| matches!(*b, b'\n' | b'\r')) { bytes.pop(); }
            let line = String::from_utf8_lossy(&bytes).into_owned();
            if matcher.matches(&line) {
                files.insert(relative.clone());
                for (number, text) in &before { if *number > last && items.len() <= request.limit { items.push(grep_match(&relative, *number, text, None)); last = *number; } }
                if line_no > last && items.len() <= request.limit { items.push(grep_match(&relative, line_no, &line, matcher.fuzzy_score(&line))); last = line_no; }
                after = request.context_lines;
            } else if after > 0 { if line_no > last && items.len() <= request.limit { items.push(grep_match(&relative, line_no, &line, None)); last = line_no; } after -= 1; }
            if request.context_lines > 0 { before.push_back((line_no, line)); while before.len() > request.context_lines { before.pop_front(); } }
        }
        if timed_out || items.len() > request.limit { break; }
    }
    let overflow = items.len() > request.limit; items.truncate(request.limit);
    Ok(GrepWorkspaceResult { bounds: WorkspaceSearchBounds { total: None, total_at_least: items.len(), next_cursor: None, truncated: timed_out || overflow, timed_out }, items, files_with_matches: files.len(), total_files_searched: searched, mode: matcher.label().into(), engine: Some(STREAMING_ENGINE_ID.into()), fallback_reason: Some(reason.into()) })
}

#[derive(Clone)] struct PathMatcher { patterns: Vec<(GlobMatcher, bool)> }
impl PathMatcher {
    fn query(query: &str) -> anyhow::Result<Self> { let q = query.trim(); let q = if q.contains(['*','?','[','{']) { q.into() } else { format!("*{}*", globset::escape(q)) }; Self::pattern(&q) }
    fn pattern(pattern: &str) -> anyhow::Result<Self> { Self::patterns(std::iter::once(pattern)) }
    fn patterns<'a>(patterns: impl IntoIterator<Item=&'a str>) -> anyhow::Result<Self> {
        let mut compiled = Vec::new();
        for pattern in patterns { let pattern = pattern.trim().trim_start_matches('!'); if pattern.is_empty() { continue; }
            let basename = !pattern.contains(['/','\\']); let matcher = GlobBuilder::new(pattern).case_insensitive(true).literal_separator(true).backslash_escape(false).build()?.compile_matcher(); compiled.push((matcher, basename)); }
        Ok(Self { patterns: compiled })
    }
    fn matches(&self, path: &str) -> bool { let basename = path.rsplit('/').next().unwrap_or(path); self.patterns.iter().any(|(m,b)| m.is_match(path) || (*b && m.is_match(basename))) }
}

enum LineMatcher { Regex(regex::Regex), Literal(Vec<String>, bool), Fuzzy(String, bool) }
impl LineMatcher {
    fn new(patterns: &[String], mode: WorkspaceSearchMode, force_case: bool) -> anyhow::Result<Self> {
        let case = force_case || patterns.iter().flat_map(|p| p.chars()).any(char::is_uppercase);
        match mode {
            WorkspaceSearchMode::Regex => { let expression = if patterns.len() == 1 { patterns[0].clone() } else { patterns.iter().map(|p| format!("(?:{})", regex::escape(p))).collect::<Vec<_>>().join("|") }; Ok(Self::Regex(RegexBuilder::new(&expression).case_insensitive(!case).build()?)) },
            WorkspaceSearchMode::Fuzzy => Ok(Self::Fuzzy(patterns.join(" "), case)),
            WorkspaceSearchMode::Plain | WorkspaceSearchMode::Auto => Ok(Self::Literal(patterns.to_vec(), case)),
        }
    }
    fn matches(&self, line: &str) -> bool { match self { Self::Regex(r) => r.is_match(line), Self::Literal(n,c) => if *c { n.iter().any(|x| line.contains(x)) } else { let line=line.to_lowercase(); n.iter().any(|x| line.contains(&x.to_lowercase())) }, Self::Fuzzy(n,c) => fuzzy_match(line,n,*c) } }
    fn fuzzy_score(&self, line: &str) -> Option<u16> { matches!(self, Self::Fuzzy(..)).then(|| line.chars().count().min(u16::MAX as usize) as u16) }
    fn label(&self) -> &'static str { match self { Self::Regex(_) => "regex", Self::Literal(..) => "plain", Self::Fuzzy(..) => "fuzzy" } }
}

fn build_picker(root: &Path) -> anyhow::Result<SharedFilePicker> {
    let shared = SharedFilePicker::default();
    FilePicker::new_with_shared_state(shared.clone(), SharedFrecency::default(), FilePickerOptions {
        base_path: root.to_string_lossy().into(), mode: FFFMode::Ai,
        enable_mmap_cache: env_flag("NEOISM_AGENT_FFF_MMAP"), enable_content_indexing: false,
        watch: true, follow_symlinks: false, enable_fs_root_scanning: false,
        enable_home_dir_scanning: false, cache_budget: None,
    }).with_context(|| format!("failed to initialize FFF index for {}", root.display()))?;
    Ok(shared)
}
fn directory_entries(request: &FindFilesRequest) -> anyhow::Result<FindFilesResult> {
    let mut entries = std::fs::read_dir(&request.root)?.filter_map(Result::ok).map(|e| e.path()).collect::<Vec<_>>(); entries.sort();
    let total = entries.len(); let items = entries.into_iter().skip(request.offset).take(request.limit).map(|path| file_match(path.file_name().unwrap_or_default().to_string_lossy().into(), path.metadata().ok().as_ref())).collect::<Vec<_>>();
    Ok(FindFilesResult { bounds: bounds(Some(total), total, request.offset, items.len(), false), items, engine: Some("directory".into()), fallback_reason: None })
}
fn bounds(total: Option<usize>, at_least: usize, offset: usize, count: usize, timed_out: bool) -> WorkspaceSearchBounds { let truncated = offset.saturating_add(count) < at_least; WorkspaceSearchBounds { total, total_at_least: at_least, next_cursor: truncated.then_some(offset + count), truncated, timed_out } }
fn file_match(path: String, metadata: Option<&Metadata>) -> WorkspaceFileMatch { WorkspaceFileMatch { path, score: 0, git_status: None, size: metadata.map_or(0, Metadata::len), modified: metadata.and_then(|m| m.modified().ok()).and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()) } }
fn grep_match(path: &str, line: u64, text: &str, fuzzy_score: Option<u16>) -> WorkspaceGrepMatch { WorkspaceGrepMatch { path: path.into(), line, text: truncate_line(text), definition: false, fuzzy_score } }
fn truncate_line(line: &str) -> String { const MAX: usize = 4_000; if line.len() <= MAX { return line.into(); } let mut end=MAX; while !line.is_char_boundary(end) { end-=1; } format!("{}…", &line[..end]) }
fn relative_path(root: &Path, path: &Path) -> Option<String> { let path=path.strip_prefix(root).ok()?.to_string_lossy().replace('\\', "/"); (!path.is_empty()).then_some(path) }
fn broad_root(root: &Path) -> bool { let root=canonical_root(root); is_filesystem_root(&root) || dirs::home_dir().map(|p| canonical_root(&p)==root).unwrap_or(false) }
fn canonical_root(root: &Path) -> PathBuf { dunce::canonicalize(root).unwrap_or_else(|_| root.to_path_buf()) }
fn is_filesystem_root(path: &Path) -> bool { path.parent().is_none() }
fn configured_capacity() -> usize { std::env::var("NEOISM_FFF_PICKER_CACHE_CAPACITY").ok().and_then(|v| v.parse().ok()).unwrap_or(DEFAULT_CAPACITY).clamp(1,MAX_CAPACITY) }
fn env_flag(name: &str) -> bool { std::env::var_os(name).as_deref().is_some_and(|v| matches!(v.to_string_lossy().as_ref(), "1"|"true"|"TRUE"|"yes"|"YES")) }
fn service_error(error: anyhow::Error) -> ServiceError { ServiceError::new(error.to_string()) }
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String { payload.downcast_ref::<&str>().map(|s|(*s).into()).or_else(||payload.downcast_ref::<String>().cloned()).unwrap_or_else(||"unknown panic".into()) }
fn grep_mode(mode: WorkspaceSearchMode, pattern: &str) -> GrepMode { match mode { WorkspaceSearchMode::Regex => GrepMode::Regex, WorkspaceSearchMode::Fuzzy => GrepMode::Fuzzy, WorkspaceSearchMode::Plain => GrepMode::PlainText, WorkspaceSearchMode::Auto if has_regex_metacharacters(pattern) => GrepMode::Regex, WorkspaceSearchMode::Auto => GrepMode::PlainText } }
fn mode_label(mode: GrepMode) -> &'static str { match mode { GrepMode::PlainText=>"plain", GrepMode::Regex=>"regex", GrepMode::Fuzzy=>"fuzzy" } }
fn grep_query(request: &GrepWorkspaceRequest, pattern: &str) -> String { let mut parts=Vec::new(); if request.path != request.root { if let Ok(path)=request.path.strip_prefix(&request.root) { parts.push(path.to_string_lossy().replace('\\',"/")); } } if let Some(include)=&request.include {parts.push(include.clone());} parts.extend(request.excludes.iter().map(|e|format!("!{}",e.trim_start_matches('!')))); if !pattern.trim().is_empty(){parts.push(pattern.trim().into());} parts.join(" ") }
fn git_status_label(status: git2::Status) -> String { if status.is_wt_new(){"untracked"}else if status.is_wt_modified()||status.is_index_modified(){"modified"}else if status.is_index_new(){"staged"}else if status.is_wt_deleted()||status.is_index_deleted(){"deleted"}else if status.is_index_renamed()||status.is_wt_renamed(){"renamed"}else{"tracked"}.into() }
fn check_cancel(cancel: Option<&Arc<std::sync::atomic::AtomicBool>>, tool: &str) -> anyhow::Result<()> { if cancel.is_some_and(|c|c.load(Ordering::SeqCst)){anyhow::bail!("{tool} aborted");} Ok(()) }
fn fuzzy_match(haystack:&str,needle:&str,case:bool)->bool{let(h,n)=if case{(haystack.into(),needle.into())}else{(haystack.to_lowercase(),needle.to_lowercase())};let mut chars=h.chars();n.chars().all(|wanted|chars.by_ref().any(|c|c==wanted))}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    static NEXT: AtomicU64 = AtomicU64::new(0);
    struct Root(PathBuf);
    impl Root { fn new(label:&str)->Self{let path=std::env::temp_dir().join(format!("agent-fff-{label}-{}-{}",std::process::id(),NEXT.fetch_add(1,Ordering::Relaxed)));std::fs::create_dir_all(&path).unwrap();Self(path)} }
    impl Drop for Root { fn drop(&mut self){let _=std::fs::remove_dir_all(&self.0);} }
    #[test] fn registry_is_bounded_and_lru(){let a=Root::new("a");let b=Root::new("b");let c=Root::new("c");let r=PickerRegistry::new(2);r.warm(&a.0).unwrap();r.warm(&b.0).unwrap();r.warm(&a.0).unwrap();r.warm(&c.0).unwrap();assert_eq!(r.len(),2);assert!(r.contains(&a.0));assert!(!r.contains(&b.0));assert!(r.contains(&c.0));}
    #[test] fn pin_lifetime_controls_eviction(){let a=Root::new("pin");let b=Root::new("other");let service=FffWorkspaceSearchService::with_capacity(1);service.warm(&a.0).unwrap();let pin=service.pin_root(&a.0).unwrap();service.warm(&b.0).unwrap();assert!(service.registry.contains(&a.0));drop(pin);service.warm(&b.0).unwrap();assert!(service.registry.contains(&b.0));}
    #[test] fn streaming_is_bounded_and_ignored(){let root=Root::new("stream");std::fs::create_dir_all(root.0.join("src")).unwrap();std::fs::create_dir_all(root.0.join("target")).unwrap();std::fs::write(root.0.join("src/lib.rs"),"needle\n").unwrap();std::fs::write(root.0.join("target/generated.rs"),"needle\n").unwrap();let request=FindFilesRequest{root:root.0.clone(),query:"*.rs".into(),offset:0,limit:10,control:neoism_agent_service_api::WorkspaceSearchRequestControl{timeout_ms:5000,cancel:None}};let result=streaming_find(&request,"test").unwrap();assert!(result.items.iter().any(|i|i.path=="src/lib.rs"));assert!(!result.items.iter().any(|i|i.path.contains("target")));}
    #[test]
    fn indexed_find_and_grep_preserve_bounds_and_identity() {
        let root = Root::new("search");
        std::fs::create_dir_all(root.0.join("src")).unwrap();
        std::fs::write(root.0.join("src/upload.rs"), "pub struct PrepareUpload;\n").unwrap();
        let service = FffWorkspaceSearchService::new();
        let control = neoism_agent_service_api::WorkspaceSearchRequestControl {
            timeout_ms: 5_000,
            cancel: None,
        };
        let found = service.find_files(&FindFilesRequest {
            root: root.0.clone(), query: "upload".into(), offset: 0, limit: 5,
            control: control.clone(),
        }).unwrap();
        assert_eq!(found.engine.as_deref(), Some(ENGINE_ID));
        assert_eq!(found.items[0].path, "src/upload.rs");
        let grep = service.grep(&GrepWorkspaceRequest {
            root: root.0.clone(), path: root.0.clone(), patterns: vec!["PrepareUpload".into()],
            include: Some("*.rs".into()), excludes: DEFAULT_EXCLUDES.iter().map(|s| (*s).into()).collect(),
            context_lines: 0, case_sensitive: false, mode: WorkspaceSearchMode::Plain,
            limit: 1, control,
        }).unwrap();
        assert_eq!(grep.engine.as_deref(), Some(ENGINE_ID));
        assert_eq!(grep.items[0].line, 1);
        assert!(grep.bounds.truncated);
    }

    #[test]
    fn cancellation_is_honored_by_streaming_fallback() {
        let root = Root::new("cancel");
        std::fs::write(root.0.join("file.txt"), "needle\n").unwrap();
        let cancel = Arc::new(std::sync::atomic::AtomicBool::new(true));
        let request = FindFilesRequest {
            root: root.0.clone(), query: "*".into(), offset: 0, limit: 10,
            control: neoism_agent_service_api::WorkspaceSearchRequestControl {
                timeout_ms: 5_000, cancel: Some(cancel),
            },
        };
        assert!(streaming_find(&request, "test").unwrap_err().to_string().contains("aborted"));
    }
}