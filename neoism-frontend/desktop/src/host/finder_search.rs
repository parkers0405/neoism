use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use neoism_ui::services::{
    IoError, SearchFileHit, SearchFileMode, SearchGitHit, SearchGrepHit, SearchGrepMode,
    SearchService,
};

use crate::screen::panes::is_project_workspace;

const ALWAYS_EXCLUDE_GLOBS: &[&str] = &["!.git/", "!.DS_Store"];
const LOOSE_EXCLUDE_GLOBS: &[&str] = &[
    "!node_modules/",
    "!target/",
    "!.cache/",
    "!.npm/",
    "!.pnpm-store/",
    "!.yarn/",
    "!.cargo/",
    "!.rustup/",
    "!.gem/",
    "!.local/share/Trash/",
    "!Library/",
    "!.Trash/",
    "!.fseventsd/",
    "!.Spotlight-V100/",
    "!Movies/",
    "!Music/",
    "!Pictures/",
    "!Applications/",
];
const LOOSE_MAX_DEPTH: &str = "6";
const LOOSE_MAX_FILES: usize = 20_000;
const MAX_HITS: usize = 500;

/// Daemon route for finder searches in a JOINED workspace: the files
/// live on the HOST's disk, so `rg`/`fff` must run there. Mirrors
/// `RemoteFiles` — requests go out on the daemon link with a
/// pre-allocated id, the trait returns `IoError::Pending(id)`, and the
/// correlated `SearchReply` lands in
/// `Screen::apply_daemon_search_message` → the finder's
/// `handle_service_reply`.
pub struct RemoteSearchRoute {
    pub root: PathBuf,
    pub handle: crate::daemon_client::DaemonClientHandle,
    pub runtime: tokio::runtime::Handle,
}

pub struct NativeSearchService {
    pickers: Mutex<HashMap<PathBuf, fff_search::FilePicker>>,
    remote: Mutex<Option<RemoteSearchRoute>>,
}

impl NativeSearchService {
    pub fn new() -> Self {
        Self {
            pickers: Mutex::new(HashMap::new()),
            remote: Mutex::new(None),
        }
    }

    /// Install (or clear) the daemon route — set on workspace switch
    /// alongside the file tree's `set_remote_files`.
    pub fn set_remote(&self, route: Option<RemoteSearchRoute>) {
        if let Ok(mut remote) = self.remote.lock() {
            *remote = route;
        }
    }

    /// When `cwd` lives in a joined workspace, ship the search to the
    /// daemon and hand back the pending request id. `None` means local.
    fn remote_dispatch(
        &self,
        cwd: &Path,
        build: impl FnOnce(u64, String) -> neoism_protocol::search::SearchClientMessage,
    ) -> Option<u64> {
        let guard = self.remote.lock().ok()?;
        let route = guard.as_ref()?;
        if !cwd.starts_with(&route.root) {
            return None;
        }
        let request_id = route.handle.allocate_request_id();
        let message = build(request_id, cwd.to_string_lossy().into_owned());
        let handle = route.handle.clone();
        route.runtime.spawn(async move {
            if let Err(error) = handle
                .send_search_with_request_id(request_id, message)
                .await
            {
                tracing::warn!(
                    target: "neoism::remote_search",
                    %error,
                    request_id,
                    "remote search request send failed"
                );
            }
        });
        Some(request_id)
    }

    fn with_picker<T>(
        &self,
        cwd: &Path,
        op: impl FnOnce(&fff_search::FilePicker) -> T,
    ) -> Option<T> {
        if !is_project_workspace(cwd) {
            return None;
        }
        let mut pickers = self.pickers.lock().ok()?;
        if !pickers.contains_key(cwd) {
            let picker = build_fff_picker(cwd)?;
            pickers.insert(cwd.to_path_buf(), picker);
        }
        pickers.get(cwd).map(op)
    }
}

impl Default for NativeSearchService {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchService for NativeSearchService {
    fn collect_files(&self, cwd: &Path) -> Result<Vec<String>, IoError> {
        if let Some(id) = self.remote_dispatch(cwd, |req_id, cwd| {
            neoism_protocol::search::SearchClientMessage::CollectFiles { req_id, cwd }
        }) {
            return Err(IoError::Pending(id));
        }
        if let Some(paths) = self.with_picker(cwd, |picker| {
            picker
                .get_files()
                .iter()
                .map(|item| item.relative_path(picker))
                .collect::<Vec<_>>()
        }) {
            return Ok(paths);
        }
        Ok(collect_files_with_rg(cwd))
    }

    fn search_files(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchFileMode,
    ) -> Result<Vec<SearchFileHit>, IoError> {
        if let Some(id) = self.remote_dispatch(cwd, |req_id, cwd| {
            neoism_protocol::search::SearchClientMessage::SearchFiles {
                req_id,
                query: query.to_string(),
                cwd,
                mode: match mode {
                    SearchFileMode::Fuzzy => {
                        neoism_protocol::search::SearchFileMode::Fuzzy
                    }
                    SearchFileMode::Exact => {
                        neoism_protocol::search::SearchFileMode::Exact
                    }
                },
            }
        }) {
            return Err(IoError::Pending(id));
        }
        match mode {
            SearchFileMode::Fuzzy => {
                if let Some(hits) = self.with_picker(cwd, |picker| {
                    let parser = fff_search::QueryParser::default();
                    let query = parser.parse(query);
                    let search = picker.fuzzy_search(
                        &query,
                        None,
                        fff_search::FuzzySearchOptions {
                            max_threads: 0,
                            current_file: None,
                            pagination: fff_search::PaginationArgs {
                                offset: 0,
                                limit: MAX_HITS,
                            },
                            ..Default::default()
                        },
                    );
                    search
                        .items
                        .iter()
                        .zip(search.scores.iter())
                        .map(|(item, score)| SearchFileHit {
                            score: score.total,
                            path: item.relative_path(picker),
                        })
                        .collect::<Vec<_>>()
                }) {
                    return Ok(hits);
                }
                Ok(fuzzy_file_hits(query, collect_files_with_rg(cwd)))
            }
            SearchFileMode::Exact => {
                let paths = self.collect_files(cwd)?;
                Ok(exact_file_hits(query, paths.iter().map(String::as_str)))
            }
        }
    }

    fn search_grep(
        &self,
        cwd: &Path,
        query: &str,
        mode: SearchGrepMode,
    ) -> Result<Vec<SearchGrepHit>, IoError> {
        if let Some(id) = self.remote_dispatch(cwd, |req_id, cwd| {
            neoism_protocol::search::SearchClientMessage::SearchGrep {
                req_id,
                query: query.to_string(),
                cwd,
                mode: match mode {
                    SearchGrepMode::Fuzzy => {
                        neoism_protocol::search::SearchGrepMode::Fuzzy
                    }
                    SearchGrepMode::Exact => {
                        neoism_protocol::search::SearchGrepMode::Exact
                    }
                    SearchGrepMode::Regex => {
                        neoism_protocol::search::SearchGrepMode::Regex
                    }
                },
                case_sensitive: None,
                file_patterns: Vec::new(),
            }
        }) {
            return Err(IoError::Pending(id));
        }
        if let Some(hits) = self.with_picker(cwd, |picker| {
            let parser = fff_search::QueryParser::new(fff_search::GrepConfig);
            let query = parser.parse(query);
            let search = picker.grep(
                &query,
                &fff_search::GrepSearchOptions {
                    mode: grep_mode(mode),
                    page_limit: 200,
                    max_matches_per_file: 25,
                    time_budget_ms: 35,
                    ..Default::default()
                },
            );
            search
                .matches
                .iter()
                .filter_map(|m| {
                    let file = search.files.get(m.file_index)?;
                    Some(SearchGrepHit {
                        score: m.fuzzy_score.map(i32::from).unwrap_or_default(),
                        path: file.relative_path(picker),
                        line: m.line_number.min(u32::MAX as u64) as u32,
                        column: (m.col + 1).min(u32::MAX as usize) as u32,
                        text: m.line_content.clone(),
                    })
                })
                .collect::<Vec<_>>()
        }) {
            return Ok(hits);
        }
        Ok(Vec::new())
    }

    fn collect_git_changes(&self, cwd: &Path) -> Result<Vec<SearchGitHit>, IoError> {
        if let Some(id) = self.remote_dispatch(cwd, |req_id, cwd| {
            neoism_protocol::search::SearchClientMessage::SearchGitChanges { req_id, cwd }
        }) {
            return Err(IoError::Pending(id));
        }
        Ok(Vec::new())
    }

    fn git_repo_root(&self, cwd: &Path) -> Option<PathBuf> {
        let output = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .arg("rev-parse")
            .arg("--show-toplevel")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .ok()?;
        if !output.status.success() {
            return None;
        }
        let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
        (!path.is_empty()).then(|| PathBuf::from(path))
    }
}

fn build_fff_picker(cwd: &Path) -> Option<fff_search::FilePicker> {
    let mut picker = fff_search::FilePicker::new(fff_search::FilePickerOptions {
        base_path: cwd.display().to_string(),
        mode: fff_search::FFFMode::Ai,
        enable_mmap_cache: false,
        ..Default::default()
    })
    .inspect_err(|error| {
        tracing::warn!(
            target: "neoism::finder",
            ?error,
            "fff-search picker initialization failed"
        );
    })
    .ok()?;

    picker
        .collect_files()
        .inspect_err(|error| {
            tracing::warn!(
                target: "neoism::finder",
                ?error,
                "fff-search file scan failed"
            );
        })
        .ok()?;

    Some(picker)
}

fn collect_files_with_rg(cwd: &Path) -> Vec<String> {
    let loose = !is_project_workspace(cwd);
    let mut command = Command::new("rg");
    command.arg("--files").arg("--hidden");
    for glob in ALWAYS_EXCLUDE_GLOBS {
        command.arg("--glob").arg(*glob);
    }
    if loose {
        for glob in LOOSE_EXCLUDE_GLOBS {
            command.arg("--glob").arg(*glob);
        }
        command.arg("--max-depth").arg(LOOSE_MAX_DEPTH);
    }
    let Ok(output) = command
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    let cap = if loose { LOOSE_MAX_FILES } else { usize::MAX };
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .take(cap)
        .map(str::to_string)
        .collect()
}

fn grep_mode(mode: SearchGrepMode) -> fff_search::GrepMode {
    match mode {
        SearchGrepMode::Fuzzy => fff_search::GrepMode::Fuzzy,
        SearchGrepMode::Exact => fff_search::GrepMode::PlainText,
        SearchGrepMode::Regex => fff_search::GrepMode::Regex,
    }
}

fn fuzzy_file_hits(query: &str, paths: Vec<String>) -> Vec<SearchFileHit> {
    let mut hits: Vec<_> = paths
        .into_iter()
        .filter_map(|path| {
            let score = fuzzy_score(query, &path)?;
            Some(SearchFileHit { score, path })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(MAX_HITS);
    hits
}

fn exact_file_hits<'a, I>(query: &str, paths: I) -> Vec<SearchFileHit>
where
    I: IntoIterator<Item = &'a str>,
{
    let query = query.trim();
    if query.is_empty() {
        return paths
            .into_iter()
            .take(MAX_HITS)
            .map(|path| SearchFileHit {
                score: 0,
                path: path.to_string(),
            })
            .collect();
    }
    let smart_case = query.chars().any(|c| c.is_ascii_uppercase());
    let mut hits: Vec<_> = paths
        .into_iter()
        .filter_map(|path| {
            let score = exact_file_match_score(query, path, smart_case)?;
            Some(SearchFileHit {
                score,
                path: path.to_string(),
            })
        })
        .collect();
    hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
    hits.truncate(MAX_HITS);
    hits
}

fn fuzzy_score(query: &str, path: &str) -> Option<i32> {
    let q = query.trim();
    if q.is_empty() {
        return Some(0);
    }
    let q = q.to_ascii_lowercase();
    let h = path.to_ascii_lowercase();
    let mut score = 0i32;
    let mut last = None;
    let mut i = 0usize;
    for qc in q.bytes() {
        let found = h.as_bytes()[i..]
            .iter()
            .position(|c| *c == qc)
            .map(|pos| pos + i)?;
        if let Some(prev) = last {
            score -= (found - prev) as i32;
        }
        last = Some(found);
        i = found + 1;
        score += 100;
    }
    Some(score - path.len() as i32)
}

fn exact_file_match_score(query: &str, path: &str, smart_case: bool) -> Option<i32> {
    let file_name = path.rsplit('/').next().unwrap_or(path);
    if exact_eq(path, query, smart_case) {
        return Some(12_000 - path.len() as i32);
    }
    if exact_eq(file_name, query, smart_case) {
        return Some(11_000 - path.len() as i32);
    }
    if let Some(pos) = exact_find(file_name, query, smart_case) {
        return Some(10_000 - pos as i32 * 10 - path.len() as i32);
    }
    if let Some(pos) = exact_find(path, query, smart_case) {
        return Some(9_000 - pos as i32 - path.len() as i32);
    }
    None
}

fn exact_eq(haystack: &str, needle: &str, smart_case: bool) -> bool {
    if smart_case {
        haystack == needle
    } else {
        haystack.eq_ignore_ascii_case(needle)
    }
}

fn exact_find(haystack: &str, needle: &str, smart_case: bool) -> Option<usize> {
    if smart_case {
        haystack.find(needle)
    } else {
        if needle.is_empty() {
            return Some(0);
        }
        let h = haystack.as_bytes();
        let n = needle.as_bytes();
        h.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
    }
}
