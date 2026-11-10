//! The project finder (`Alt+S`, palette "Search Files / Words / Git Changes";
//! `shared/src/panels/finder`): one query over the daemon's `Search` family. Files mode is the
//! compact 480 pt list; grep and git modes are 880 pt with a preview of the selected match.

mod view;

pub use view::view;

use crate::app::App;
use crate::daemon::Family;
use neoism_protocol::search::{SearchClientMessage, SearchFileMode, SearchGitStatus, SearchGrepMode, SearchServerMessage};
use rune::TextEdit;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Files,
    Grep,
    Git,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Hit {
    File { path: String },
    Grep { path: String, line: u32, text: String },
    Git { path: String, status: SearchGitStatus, line: u32, text: String },
}

impl Hit {
    pub fn path(&self) -> &str {
        match self { Hit::File { path } | Hit::Grep { path, .. } | Hit::Git { path, .. } => path }
    }
    pub fn line(&self) -> Option<u32> {
        match self { Hit::File { .. } => None, Hit::Grep { line, .. } | Hit::Git { line, .. } => Some(*line) }
    }
}

pub struct Finder {
    pub mode: Mode,
    pub query: TextEdit,
    pub exact: bool,
    pub grep_mode: SearchGrepMode,
    pub hits: Vec<Hit>,
    pub selected: usize,
    pub loading: bool,
    pub error: Option<String>,
    /// File text behind the preview, by path.
    pub preview: Option<(String, Vec<String>)>,
    request: u64,
    due: Option<Instant>,
}

const FILE_DEBOUNCE: Duration = Duration::from_millis(75);
const GREP_DEBOUNCE: Duration = Duration::from_millis(220);
const MIN_GREP_CHARS: usize = 2;

impl Finder {
    fn new(mode: Mode) -> Finder {
        let mut query = TextEdit::new("");
        query.focused = true;
        Finder { mode, query, exact: false, grep_mode: SearchGrepMode::Fuzzy, hits: Vec::new(), selected: 0, loading: false, error: None, preview: None, request: 0, due: Some(Instant::now()) }
    }

    pub fn placeholder(&self) -> &'static str {
        match self.mode { Mode::Files => "Find file in project...", Mode::Grep => "Grep across project...", Mode::Git => "Filter git changes..." }
    }

    /// The search-mode badge at the input's right edge.
    pub fn badge(&self) -> &'static str {
        match self.mode {
            Mode::Files => if self.exact { "[exact]" } else { "[fuzzy]" },
            Mode::Grep => match self.grep_mode { SearchGrepMode::Fuzzy => "[fuzzy]", SearchGrepMode::Exact => "[exact]", SearchGrepMode::Regex => "[regex]" },
            Mode::Git => "",
        }
    }

    pub fn has_preview(&self) -> bool { matches!(self.mode, Mode::Grep | Mode::Git) }

    pub fn selected_hit(&self) -> Option<&Hit> { self.hits.get(self.selected) }

    /// A query edit: search again after the mode's debounce.
    fn edited(&mut self) {
        let delay = if self.mode == Mode::Grep { GREP_DEBOUNCE } else { FILE_DEBOUNCE };
        self.due = Some(Instant::now() + delay);
    }
}

impl App {
    pub fn open_finder(&mut self, mode: Mode) {
        self.picker = None;
        self.finder = Some(Finder::new(mode));
        if let Some(wake) = &self.waker { wake(); }
    }

    pub fn finder_edited(&mut self) {
        if let Some(finder) = &mut self.finder { finder.edited(); }
    }

    /// `Ctrl+Tab`: fuzzy ⇄ exact (files), fuzzy → exact → regex (grep).
    pub fn cycle_finder_mode(&mut self) {
        let Some(finder) = &mut self.finder else { return };
        match finder.mode {
            Mode::Files => finder.exact = !finder.exact,
            Mode::Grep => finder.grep_mode = match finder.grep_mode { SearchGrepMode::Fuzzy => SearchGrepMode::Exact, SearchGrepMode::Exact => SearchGrepMode::Regex, SearchGrepMode::Regex => SearchGrepMode::Fuzzy },
            Mode::Git => return,
        }
        finder.due = Some(Instant::now());
    }

    pub fn move_finder(&mut self, delta: isize) {
        let Some(finder) = &mut self.finder else { return };
        if finder.hits.is_empty() { return; }
        let last = finder.hits.len() as isize - 1;
        finder.selected = (finder.selected as isize + delta).clamp(0, last) as usize;
        self.finder_follow.request();
        self.request_preview();
    }

    pub fn select_finder(&mut self, index: usize) {
        if let Some(finder) = &mut self.finder { finder.selected = index; }
        self.request_preview();
    }

    /// Open the selected (or clicked) hit and close the finder.
    pub fn accept_finder(&mut self, index: Option<usize>) {
        let Some(finder) = self.finder.take() else { return };
        let Some(hit) = finder.hits.get(index.unwrap_or(finder.selected)) else { return };
        let path = std::path::Path::new(&self.cwd).join(hit.path()).display().to_string();
        self.open_file(&path, hit.line());
    }

    /// Send a debounced search when it is due. Returns when to check again.
    pub fn finder_tick(&mut self, now: Instant) -> Option<Duration> {
        let finder = self.finder.as_mut()?;
        let due = finder.due?;
        if due > now { return Some(due - now); }
        finder.due = None;
        finder.request += 1;
        let req_id = finder.request;
        let query = finder.query.text.trim().to_owned();
        let message = match finder.mode {
            Mode::Files => SearchClientMessage::SearchFiles {
                req_id, query, cwd: String::new(),
                mode: if finder.exact { SearchFileMode::Exact } else { SearchFileMode::Fuzzy },
            },
            Mode::Grep if query.chars().count() < MIN_GREP_CHARS => {
                finder.hits.clear();
                finder.loading = false;
                return None;
            }
            Mode::Grep => SearchClientMessage::SearchGrep {
                req_id, query, cwd: String::new(), mode: finder.grep_mode, case_sensitive: None, file_patterns: Vec::new(),
            },
            Mode::Git => SearchClientMessage::SearchGitChanges { req_id, cwd: String::new() },
        };
        finder.loading = true;
        if let (Some(daemon), Ok(value)) = (&self.daemon, serde_json::to_value(&message)) {
            daemon.send_scoped(Family::Search, value, &self.cwd);
        }
        None
    }

    pub(crate) fn apply_search(&mut self, message: SearchServerMessage) {
        if let SearchServerMessage::SearchDirectoriesResult { req_id, hits } = message {
            let hits: Vec<String> = hits.into_iter().map(|h| h.path).collect();
            if self.apply_composer_directory_hits(req_id, &hits) { return; }
            self.apply_directory_hits(req_id, hits);
            return;
        }
        let Some(finder) = &mut self.finder else { return };
        let (req_id, hits) = match message {
            SearchServerMessage::SearchFilesResult { req_id, hits } => (req_id, hits.into_iter().map(|h| Hit::File { path: h.path }).collect::<Vec<_>>()),
            SearchServerMessage::SearchGrepResult { req_id, hits } => (req_id, hits.into_iter().map(|h| Hit::Grep { path: h.path, line: h.line, text: h.text }).collect()),
            SearchServerMessage::SearchGitChangesResult { req_id, hits } => {
                let query = finder.query.text.trim().to_lowercase();
                (req_id, hits.into_iter().filter(|h| query.is_empty() || h.path.to_lowercase().contains(&query))
                    .map(|h| Hit::Git { path: h.path, status: h.status, line: h.line, text: h.text }).collect())
            }
            SearchServerMessage::SearchError { req_id, message } if req_id == finder.request => {
                finder.loading = false;
                finder.error = Some(message);
                return;
            }
            _ => return,
        };
        if req_id != finder.request { return; }
        finder.hits = hits;
        finder.selected = 0;
        finder.loading = false;
        finder.error = None;
        self.request_preview();
    }

    /// Grep and git modes preview the selected hit's file.
    fn request_preview(&mut self) {
        let Some(finder) = &self.finder else { return };
        if !finder.has_preview() { return; }
        let Some(path) = finder.selected_hit().map(|h| h.path().to_owned()) else { return };
        if finder.preview.as_ref().is_some_and(|(p, _)| *p == path) { return; }
        if let Some(daemon) = &self.daemon {
            daemon.send_scoped(Family::Files, serde_json::json!({"ReadFile": {"path": path}}), &self.cwd);
        }
    }

    pub(crate) fn apply_file_content(&mut self, path: &str, bytes: &[u8]) {
        let Some(finder) = &mut self.finder else { return };
        if finder.selected_hit().map(Hit::path) != Some(path) { return; }
        let text = if bytes.iter().take(8192).any(|b| *b == 0) { "Binary file".to_owned() } else { String::from_utf8_lossy(bytes).into_owned() };
        finder.preview = Some((path.to_owned(), text.lines().map(str::to_owned).collect()));
    }
}

/// Parent directory and file name of a workspace-relative path.
pub fn split_path(path: &str) -> (&str, &str) {
    match path.rsplit_once('/') { Some((dir, name)) => (dir, name), None => ("", path) }
}

pub fn git_marker(status: SearchGitStatus) -> &'static str {
    match status {
        SearchGitStatus::Modified => "M",
        SearchGitStatus::Staged => "S",
        SearchGitStatus::Mixed => "M*",
        SearchGitStatus::Added => "A",
        SearchGitStatus::Deleted => "D",
        SearchGitStatus::Renamed => "R",
        SearchGitStatus::Untracked => "?",
        SearchGitStatus::Conflict => "!",
    }
}
