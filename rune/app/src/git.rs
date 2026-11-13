//! Native Git changes panel and unified review. All repository IO belongs to the daemon;
//! request identities are retained with their cwd, and patches are parsed only on replies.

use crate::app::{App, Focus};
use crate::tabs::TabKind;
use crate::daemon::Family;
use crate::ui::theme::{panel_bg, rgb};
use crate::ui::ChildIf;
use neoism_protocol::git::{GitChangeStatus, GitClientMessage, GitFileChange, GitServerMessage};
use rune::{El, Key, KeyEvent, ScrollSpec, TextEdit, Tx, WheelEvent, col, el, row, text};
use std::{cell::Cell, rc::Rc};

pub const WIDTH: f32 = 300.0;

#[derive(Default)]
pub struct State {
    pub(crate) repo_root: Option<String>,
    pub open: bool,
    pub message: TextEdit,
    files: Vec<GitFileChange>,
    branch: Option<String>,
    error: Option<String>,
    loaded: bool,
    totals: (u64, u64, usize),
    epoch: u64,
    pending: Option<Pending>,
    selected: Option<String>,
    diff_pending: Option<Pending>,
    diff_error: Option<String>,
    diff_dirty: bool,
    cursor: usize,
    lines: Vec<Line>,
    file_scroll: Rc<Cell<(f32, f32)>>,
    diff_scroll: Rc<Cell<(f32, f32)>>,
}

#[derive(Clone, Debug)]
struct Pending {
    id: u64,
    cwd: String,
    kind: Operation,
}

#[derive(Clone, Debug)]
enum Operation { Refresh, Write, Commit(String), Diff(String) }

impl Pending {
    fn matches(&self, id: u64, cwd: &str) -> bool { self.id == id && self.cwd == cwd }
}

impl State {
    pub fn review_open(&self) -> bool { self.selected.is_some() }

    fn set_files(&mut self, files: Vec<GitFileChange>, branch: Option<String>) {
        if self.files != files {
            self.totals = files.iter().fold((0, 0, 0), |(a, d, n), f|
                (a + f.additions as u64, d + f.deletions as u64, n + usize::from(f.staged)));
            self.files = files;
            self.cursor = self.cursor.min(self.files.len().saturating_sub(1));
            self.epoch = self.epoch.wrapping_add(1);
        }
        self.branch = branch;
        self.loaded = true;
    }
}

impl App {
    pub fn reset_git_panel(&mut self) {
        let open = self.git_panel.open;
        self.git_panel = State { open, ..State::default() };
        if open { self.refresh_git_panel(); }
    }

    pub fn toggle_git_panel(&mut self) {
        self.git_panel.open = !self.git_panel.open;
        if self.git_panel.open {
            self.focus = Focus::Details;
            self.refresh_git_panel();
        } else {
            self.git_panel.message.focused = false;
            if self.focus == Focus::Details { self.focus = Focus::Content; }
        }
    }

    pub fn git_disconnected(&mut self) {
        for buffer in self.buffers.values_mut() { buffer.git = Default::default(); }
        if let Some(pending) = self.git_panel.pending.take() {
            self.git_panel.error = Some(if matches!(pending.kind, Operation::Commit(_)) {
                "Connection lost during commit. Check repository status before retrying."
            } else { "Connection lost. Reconnect to refresh repository status." }.into());
        }
        self.git_panel.diff_pending = None;
        self.git_panel.diff_dirty = false;
    }

    pub fn git_connected(&mut self) {
        self.refresh_editor_git_bases();
        if self.git_panel.open { self.refresh_git_panel(); }
        if self.git_panel.review_open() { self.load_git_review(); }
    }

    fn git_request(&mut self, message: GitClientMessage, kind: Operation) -> Option<Pending> {
        let Some(daemon) = &self.daemon else {
            self.git_panel.error = Some("Not connected to the workspace daemon.".into());
            return None;
        };
        let id = daemon.request(Family::Git, serde_json::to_value(message).expect("Git wire message"), Some(self.cwd.clone()));
        Some(Pending { id, cwd: self.cwd.clone(), kind })
    }

    pub fn refresh_git_panel(&mut self) {
        if self.git_panel.pending.is_some() { return; }
        self.git_panel.pending = self.git_request(GitClientMessage::ChangedFiles, Operation::Refresh);
    }

    fn git_stage(&mut self, path: &str) {
        self.focus = Focus::Details;
        self.git_panel.message.focused = false;
        if self.git_panel.pending.is_some() { return; }
        let Some(file) = self.git_panel.files.iter().find(|f| f.path == path) else { return; };
        let message = if file.staged { GitClientMessage::Unstage { path: path.into() } }
            else { GitClientMessage::Stage { path: path.into() } };
        self.git_panel.error = None;
        self.git_panel.pending = self.git_request(message, Operation::Write);
    }

    fn git_commit(&mut self) {
        let message = self.git_panel.message.text.trim().to_owned();
        if message.is_empty() || self.git_panel.totals.2 == 0 || self.git_panel.pending.is_some() { return; }
        self.git_panel.error = None;
        self.git_panel.pending = self.git_request(GitClientMessage::Commit { message }, Operation::Commit(self.git_panel.message.text.clone()));
    }

    pub fn open_git_review(&mut self, path: &str) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::GitDiff).map(|t| t.id) {
            self.select_tab(id);
        } else { self.open_tab(TabKind::GitDiff); }
        self.focus = Focus::Content;
        self.git_panel.message.focused = false;
        self.git_panel.selected = Some(path.into());
        self.git_panel.lines.clear();
        self.git_panel.diff_error = None;
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
        self.git_panel.diff_scroll = Rc::default();
        self.git_panel.diff_pending = None;
        self.load_git_review();
    }

    fn load_git_review(&mut self) {
        let Some(path) = self.git_panel.selected.clone() else { return; };
        if self.git_panel.diff_pending.is_some() {
            self.git_panel.diff_dirty = true;
            return;
        }
        self.git_panel.diff_dirty = false;
        self.git_panel.diff_pending = self.git_request(GitClientMessage::DiffFiles { paths: vec![path.clone()] }, Operation::Diff(path));
        if self.git_panel.diff_pending.is_none() {
            self.git_panel.diff_error = self.git_panel.error.clone();
        }
    }

    pub fn close_git_review(&mut self) {
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::GitDiff) {
            self.close_tab_now(index);
        } else { self.clear_git_review(); }
        self.focus = Focus::Content;
    }

    pub fn clear_git_review(&mut self) {
        self.git_panel.selected = None;
        self.git_panel.diff_pending = None;
        self.git_panel.diff_dirty = false;
        self.git_panel.lines.clear();
    }

    pub fn apply_git_panel(&mut self, message: GitServerMessage, request_id: u64) {
        if self.apply_editor_git(&message, request_id) { return; }
        // Watch events are scoped independently from request/reply traffic. Never accept a
        // previous workspace's broadcast, even if it arrives after its panel was reset.
        if let GitServerMessage::RepoStatus { token, snapshot } = &message {
            if token != "neoism-rune-git-status" || snapshot.workspace_root != self.cwd { return; }
            if (self.git_panel.open || self.git_panel.review_open()) && self.git_panel.pending.is_none() {
                self.git_panel.set_files(snapshot.files.clone(), snapshot.branch.clone());
                if snapshot.error.is_some() { self.git_panel.error = snapshot.error.clone(); }
                if self.git_panel.review_open() { self.load_git_review(); }
            }
            return;
        }
        if self.git_panel.diff_pending.as_ref().is_some_and(|p| p.matches(request_id, &self.cwd)) {
            let pending = self.git_panel.diff_pending.take().unwrap();
            let Operation::Diff(path) = pending.kind else { return; };
            if self.git_panel.selected.as_deref() != Some(&path) { return; }
            match message {
                GitServerMessage::FileDiffs { diffs } => {
                    self.git_panel.lines = diffs.into_iter().find(|d| d.path == path)
                        .map(|d| parse_patch(&d.patch)).unwrap_or_default();
                    self.git_panel.diff_error = None;
                    self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
                }
                GitServerMessage::Error { message } => self.git_panel.diff_error = Some(message),
                _ => self.git_panel.diff_error = Some("Unexpected Git diff response.".into()),
            }
            if self.git_panel.diff_dirty { self.load_git_review(); }
            return;
        }
        if !self.git_panel.pending.as_ref().is_some_and(|p| p.matches(request_id, &self.cwd)) { return; }
        let pending = self.git_panel.pending.take().unwrap();
        match message {
            GitServerMessage::ChangedFiles { files, branch, error, .. } => {
                // Don't erase text edited while the commit was in flight, or after a failure.
                if error.is_none() {
                    if let Operation::Commit(sent) = pending.kind {
                        if self.git_panel.message.text == sent { self.git_panel.message = TextEdit::default(); }
                    }
                }
                self.git_panel.error = error;
                self.git_panel.set_files(files, branch);
                if self.git_panel.review_open() { self.load_git_review(); }
            }
            GitServerMessage::Error { message } => self.git_panel.error = Some(message),
            _ => self.git_panel.error = Some("Unexpected Git status response.".into()),
        }
    }

    pub fn git_panel_key(&mut self, event: &KeyEvent, tx: &mut Tx) -> bool {
        if self.focus == Focus::Details && self.git_panel.open {
            match event.key {
                Key::Escape => self.toggle_git_panel(),
                Key::Up => self.git_panel.cursor = self.git_panel.cursor.saturating_sub(1),
                Key::Down => self.git_panel.cursor = (self.git_panel.cursor + 1).min(self.git_panel.files.len().saturating_sub(1)),
                Key::Home => self.git_panel.cursor = 0,
                Key::End => self.git_panel.cursor = self.git_panel.files.len().saturating_sub(1),
                Key::Enter => {
                    if let Some(path) = self.git_panel.files.get(self.git_panel.cursor).map(|f| f.path.clone()) { self.open_git_review(&path); }
                }
                Key::Text(ref t) if t == " " => {
                    if let Some(path) = self.git_panel.files.get(self.git_panel.cursor).map(|f| f.path.clone()) { self.git_stage(&path); }
                }
                Key::Tab => self.git_panel.message.focused = true,
                _ => {},
            }
            self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
            let row = self.git_panel.cursor as f32 * 24.0 * self.ty.scale();
            tx.scroll_to(format!("git-files:{}", self.cwd), 0.0, row);
            return true;
        }
        if self.focus != Focus::Content || self.active_tab().kind != TabKind::GitDiff { return false; }
        if event.key == Key::Escape { self.close_git_review(); return true; }
        // Let window shortcuts run first; no review keystroke may reach the terminal below.
        let s = self.ty.scale();
        let delta = match event.key {
            Key::Up => Some(-24.0 * s), Key::Down => Some(24.0 * s),
            Key::PageUp => Some(-self.window[1] * 0.7), Key::PageDown => Some(self.window[1] * 0.7),
            Key::Home => Some(-f32::MAX), Key::End => Some(f32::MAX), _ => None,
        };
        if let Some(delta) = delta {
            let (offset, max) = self.git_panel.diff_scroll.get();
            tx.scroll_to(review_key(self), 0.0, (offset + delta).clamp(0.0, max));
        }
        true
    }

    pub fn git_review_wheel(&mut self, e: &WheelEvent, tx: &mut Tx) -> bool {
        let source = self.git_panel.diff_scroll.clone();
        let key = review_key(self);
        crate::ui::scroll::handle(&mut self.scrollers, &key, e, 66.0 * self.ty.scale(), Rc::new(move || source.get()), tx)
    }
}

fn button(app: &App, label: &str) -> El {
    el().h(26.0 * app.ty.scale()).px(8.0 * app.ty.scale()).center().radius(4.0)
        .hover(rgb(app.pal.hover)).child(text(label.to_owned()).font(app.ty.sized(12.0)).color(rgb(app.pal.dim)))
}

pub fn view(app: &App) -> El {
    if !app.git_panel.open { return el(); }
    let p = app.pal;
    let s = app.ty.scale();
    let state = &app.git_panel;
    let header = row().h(38.0 * s).w_full().px(14.0 * s).center_y()
        .child(text("Git").font(app.ty.sized_bold(13.0)).color(rgb(p.fg)).grow(1.0))
        .child(button(app, "↻").on_press(|a: &mut App| a.refresh_git_panel()))
        .child(button(app, "×").on_press(|a: &mut App| a.toggle_git_panel()));
    let stats = row().h(24.0 * s).px(14.0 * s).gap(8.0 * s).center_y()
        .child(text(format!("{} files", state.files.len())).font(app.ty.sized(11.0)).color(rgb(p.dim)).grow(1.0))
        .child(text(format!("+{}", state.totals.0)).font(app.ty.sized(11.0)).color(rgb(p.green)))
        .child(text(format!("−{}", state.totals.1)).font(app.ty.sized(11.0)).color(rgb(p.red)));
    let rows = col().w_full().growth(rune::GrowthSpec::default()).virtual_rows(state.files.len(), 24.0 * s,
        |a: &App, i| a.git_panel.files[i].path.clone().into(),
        |a: &App, i| file_row(a, i)).virtual_rows_version(state.epoch);
    let key = format!("git-files:{}", app.cwd);
    let mirror = state.file_scroll.clone();
    let source = mirror.clone();
    let wheel_key = key.clone();
    let mut list = col().key(key).w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            let source = source.clone();
            crate::ui::scroll::handle(&mut a.scrollers, &wheel_key, &e, 72.0 * s, Rc::new(move || source.get()), tx)
        }).child(rows);
    if state.files.is_empty() {
        list = list.child(text(if !state.loaded && state.pending.is_some() { "Loading changes…" } else if !state.loaded { "Refresh to load changes" } else { "No uncommitted changes." })
            .font(app.ty.sized(12.0)).color(rgb(p.muted)).padding(14.0 * s));
    }
    let input = rune::text_input(&state.message, app.ty.sized(12.0)).key("git-commit-message")
        .w_full().h(76.0 * s).padding(8.0 * s).radius(6.0 * s).fill(rgb(p.surface))
        .color(rgb(p.fg)).placeholder("Type a commit message…", rgb(p.muted))
        .on_edit(|a: &mut App| { a.focus = Focus::Details; &mut a.git_panel.message })
        .on_input_key(|a: &mut App, e: &KeyEvent, tx: &mut Tx| {
            if crate::keys::shortcut(a, e, tx) { return true; }
            if e.key == Key::Escape { a.git_panel.message.focused = false; return true; }
            false
        });
    let enabled = state.pending.is_none() && state.totals.2 > 0 && !state.message.text.trim().is_empty();
    let mut commit = button(app, if state.pending.as_ref().is_some_and(|p| matches!(p.kind, Operation::Commit(_))) { "Committing…" } else { "Commit" })
        .fill(rgb(if enabled { p.hover } else { p.surface }));
    if enabled { commit = commit.on_press(|a: &mut App| a.git_commit()); }
    let mut footer = col().w_full().padding(14.0 * s).gap(8.0 * s);
    if let Some(error) = &state.error {
        footer = footer.child(text(error.clone()).font(app.ty.sized(11.0)).color(rgb(p.red)).max_h(70.0 * s));
    }
    footer = footer.child(input).child(row().w_full().gap(8.0 * s).center_y()
        .child(text(state.branch.clone().unwrap_or_else(|| "Detached / no repository".into())).font(app.ty.sized(11.0)).color(rgb(p.dim)).truncate().grow(1.0).shrink())
        .child(commit));
    col().key("git-panel").w(WIDTH * s).h_full().padding(6.0 * s)
        .child(col().w_full().h_full().fill(rgb(panel_bg(p))).border(2.25 * s, rgb(p.border)).radius(14.0 * s).clip_rounded(14.0 * s)
            .child(header).child(stats).child(list).child(footer))
}

fn file_row(app: &App, index: usize) -> El {
    let f = &app.git_panel.files[index];
    let p = app.pal;
    let s = app.ty.scale();
    let path = f.path.clone();
    let stage_path = path.clone();
    let (tag, color) = match f.status {
        GitChangeStatus::Added | GitChangeStatus::Untracked => ("A", p.green),
        GitChangeStatus::Deleted => ("D", p.red), GitChangeStatus::Renamed => ("R", p.blue),
        GitChangeStatus::Conflict => ("!", p.red), _ => ("M", p.yellow),
    };
    let mut check = el().key(format!("git-stage:{}", f.path)).w(24.0 * s).h(24.0 * s).center().hover(rgb(p.hover))
        .child(el().w(12.0 * s).h(12.0 * s).radius(2.0 * s).border(1.0, rgb(if f.staged { p.green } else { p.muted })).center()
            .child_if(f.staged, || text("\u{f00c}").font(app.ty.icon_sized(9.0)).color(rgb(p.green))));
    if app.git_panel.pending.is_none() { check = check.on_press(move |a: &mut App| a.git_stage(&stage_path)); }
    // Checkbox and file label have disjoint hit regions: staging never opens a review.
    row().w_full().h(24.0 * s).px(6.0 * s).center_y()
        .fill(rgb(if app.focus == Focus::Details && app.git_panel.cursor == index { p.hover } else { panel_bg(p) })).child(check)
        .child(row().key(format!("git-file:{}", f.path)).h_full().grow(1.0).shrink().gap(6.0 * s).center_y().hover(rgb(p.hover))
            .on_press(move |a: &mut App| a.open_git_review(&path))
            .child(text(tag).font(app.ty.sized(11.0)).color(rgb(color)))
            .child(text(f.path.clone()).font(app.ty.sized(12.0)).color(rgb(p.dim)).truncate().grow(1.0).shrink())
            .child(text(format!("+{}", f.additions)).font(app.ty.sized(10.0)).color(rgb(p.green)))
            .child(text(format!("−{}", f.deletions)).font(app.ty.sized(10.0)).color(rgb(p.red))))
}

fn review_key(app: &App) -> String {
    format!("git-review:{}:{}", app.cwd, app.git_panel.selected.as_deref().unwrap_or(""))
}

/// A normal pane surface, so switching tabs gives their keyboard and content back.
pub fn review(app: &App) -> El {
    let Some(path) = &app.git_panel.selected else {
        return col().w_full().h_full().center().fill(rgb(app.pal.bg))
            .child(button(app, "Choose a file from Git Changes").on_press(|a: &mut App| {
                if !a.git_panel.open { a.toggle_git_panel(); }
                a.focus = Focus::Details;
            }));
    };
    let p = app.pal;
    let s = app.ty.scale();
    let state = &app.git_panel;
    let header = row().w_full().h(36.0 * s).px(8.0 * s).gap(8.0 * s).center_y()
        .child(button(app, "‹ Back").on_press(|a: &mut App| a.close_git_review()))
        .child(text(path.clone()).font(app.ty.sized(12.0)).color(rgb(p.fg)).truncate().grow(1.0).shrink())
        .child(button(app, "Refresh").on_press(|a: &mut App| a.load_git_review()));
    let rows = col().w_full().growth(rune::GrowthSpec::default()).virtual_rows(state.lines.len(), 22.0 * s,
        |_: &App, i| format!("diff:{i}").into(), |a: &App, i| diff_row(a, &a.git_panel.lines[i]))
        .virtual_rows_version(state.epoch);
    let key = review_key(app);
    let mirror = state.diff_scroll.clone();
    let mut body = col().key(key).w_full().grow(1.0).shrink().px(8.0 * s)
        .scroll(ScrollSpec { stick_bottom: false }).scroll_anchor()
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(|a: &mut App, e: WheelEvent, tx: &mut Tx| a.git_review_wheel(&e, tx)).child(rows);
    if let Some(error) = &state.diff_error {
        body = body.child(text(error.clone()).font(app.ty.sized(12.0)).color(rgb(p.red)).padding(16.0 * s));
    } else if state.lines.is_empty() {
        body = body.child(text(if state.diff_pending.is_some() { "Loading diff…" } else { "No textual changes (empty file or unchanged content)." })
            .font(app.ty.sized(12.0)).color(rgb(p.muted)).padding(16.0 * s));
    }
    col().key("git-review-surface").w_full().h_full()
        .fill(rgb(p.bg)).clip_rounded(0.0).child(header).child(body)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind { Context, Add, Remove, Hunk, Meta }

#[derive(Debug)]
struct Line { kind: Kind, old: Option<usize>, new: Option<usize>, text: String }

/// Unified patch parser. File headers aren't deletion/addition lines; zero-count ranges,
/// disjoint hunks, EOF notices and binary/rename-only patches retain their real semantics.
fn parse_patch(patch: &str) -> Vec<Line> {
    let mut rows = Vec::new();
    let (mut old, mut new) = (0, 0);
    let mut in_hunk = false;
    for raw in patch.lines() {
        let mut kind = Kind::Meta;
        let (mut old_no, mut new_no) = (None, None);
        let mut content = raw;
        if let Some(header) = raw.strip_prefix("@@ -") {
            if let Some((before, after)) = header.split_once(" +") {
                if let (Some(a), Some(b)) = (before.split(',').next().and_then(|n| n.parse::<usize>().ok()),
                    after.split([',', ' ']).next().and_then(|n| n.parse::<usize>().ok())) {
                    old = a; new = b; in_hunk = true; kind = Kind::Hunk;
                }
            }
        } else if raw.starts_with("diff --git ") { in_hunk = false; }
        else if in_hunk {
            match raw.as_bytes().first() {
                Some(b'+') => { kind = Kind::Add; new_no = Some(new); new += 1; content = &raw[1..]; }
                Some(b'-') => { kind = Kind::Remove; old_no = Some(old); old += 1; content = &raw[1..]; }
                Some(b' ') => { kind = Kind::Context; old_no = Some(old); new_no = Some(new); old += 1; new += 1; content = &raw[1..]; }
                Some(b'\\') => {},
                _ => { in_hunk = false; }
            }
        }
        // The path already heads the review; retain meaningful metadata (binary, rename,
        // modes) but omit redundant transport headers.
        if kind == Kind::Meta && (raw.starts_with("diff --git ") || raw.starts_with("index ") || raw.starts_with("--- ") || raw.starts_with("+++ ")) { continue; }
        rows.push(Line { kind, old: old_no, new: new_no, text: content.into() });
    }
    rows
}

fn diff_row(app: &App, line: &Line) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let (sign, color, wash) = match line.kind {
        Kind::Add => ("+", p.green, rgb(p.green).alpha(0.08)),
        Kind::Remove => ("−", p.red, rgb(p.red).alpha(0.08)),
        Kind::Hunk => ("", p.blue, rgb(p.surface)),
        Kind::Meta => ("", p.muted, rune::Color::TRANSPARENT),
        Kind::Context => ("", p.dim, rune::Color::TRANSPARENT),
    };
    let number = |n: Option<usize>| text(n.map(|n| n.to_string()).unwrap_or_default())
        .font(app.ty.sized(11.0)).color(rgb(p.muted)).w(46.0 * s);
    row().w_full().h(22.0 * s).fill(wash).center_y().clip_rounded(0.0)
        .child(number(line.old)).child(number(line.new))
        .child(text(sign).font(app.ty.sized(12.0)).color(rgb(color)).w(18.0 * s))
        .child(text(line.text.clone()).font(app.ty.sized(12.0)).color(rgb(color)))
}

/// Deterministic, daemon-free scenes for `git-panel` / `git-review` headless captures.
pub fn fixture(app: &mut App, review: bool) {
    app.git_panel = State { open: true, ..State::default() };
    app.git_panel.set_files(vec![
        GitFileChange { path: "src/main.rs".into(), status: GitChangeStatus::Mixed, additions: 3, deletions: 1, staged: true },
        GitFileChange { path: "README.md".into(), status: GitChangeStatus::Modified, additions: 12, deletions: 4, staged: false },
        GitFileChange { path: "src/git.rs".into(), status: GitChangeStatus::Untracked, additions: 128, deletions: 0, staged: false },
    ], Some("rune-frontend".into()));
    app.git_panel.message = TextEdit::new("Add native Git review");
    if review {
        app.open_tab(TabKind::GitDiff);
        app.focus = Focus::Content;
        app.git_panel.selected = Some("src/main.rs".into());
        app.git_panel.lines = parse_patch("diff --git a/src/main.rs b/src/main.rs\n--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,4 +1,6 @@\n mod app;\n-mod legacy;\n+mod git;\n+mod workspace;\n \n+// Native views, daemon-owned data.\n fn main() {\n");
    }
}

pub fn long_fixture(app: &mut App) {
    fixture(app, true);
    let patch = format!("@@ -1,10000 +1,10000 @@\n{}", "-let old = false;\n+let current = true;\n".repeat(10_000));
    app.git_panel.lines = parse_patch(&patch);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn large_review_renders_only_a_viewport_of_rows() {
        let mut app = App::new();
        long_fixture(&mut app);
        let mut rt = rune::Runtime::new(app, |a: &App, _: &mut rune::Cx<App>| {
            row().w_full().h_full().child(review(a).grow(1.0).shrink()).child(view(a))
        }, (1252.0, 674.0), rune::Env::new(true, 1.0));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert!(rt.elements().len() < 1000, "20,000 diff lines must not mount at once");
        rt.update(|a, tx| tx.scroll_to(review_key(a), 0.0, f32::MAX));
        for _ in 0..3 { rt.frame(1.0 / 120.0); }
        assert!(rt.elements().len() < 1000, "scrolling must keep a bounded row window");
    }

    #[test]
    fn unified_coordinates_headers_eof_and_disjoint_hunks() {
        let rows = parse_patch("--- a/file\n+++ b/file\n@@ -3,2 +7,2 @@ name\n-old\n+new\n same\n\\ No newline at end of file\n@@ -20,0 +25,1 @@\n+++ literal pluses\n");
        assert_eq!(rows.len(), 7);
        assert_eq!((rows[1].kind, rows[1].old, rows[1].new), (Kind::Remove, Some(3), None));
        assert_eq!((rows[2].old, rows[2].new), (None, Some(7)));
        assert_eq!((rows[3].old, rows[3].new), (Some(4), Some(8)));
        assert_eq!(rows[4].kind, Kind::Meta);
        assert_eq!((rows[6].new, rows[6].text.as_str()), (Some(25), "++ literal pluses"));
    }

    #[test]
    fn empty_binary_and_rename_only_are_not_fake_hunks() {
        assert!(parse_patch("").is_empty());
        let binary = parse_patch("diff --git a/a b/a\nindex 123..456 100644\nBinary files a/a and b/a differ\n");
        assert_eq!(binary.len(), 1);
        assert_eq!(binary[0].kind, Kind::Meta);
        let rename = parse_patch("similarity index 100%\nrename from old\nrename to new\n");
        assert!(rename.iter().all(|l| l.old.is_none() && l.new.is_none()));
    }

    #[test]
    fn pending_identity_requires_both_request_and_workspace() {
        let pending = Pending { id: 42, cwd: "/first".into(), kind: Operation::Diff("file".into()) };
        assert!(pending.matches(42, "/first"));
        assert!(!pending.matches(41, "/first"));
        assert!(!pending.matches(42, "/second"));
    }

    #[test]
    fn switching_files_rejects_old_diff_and_empty_reply_clears_content() {
        let mut app = App::new();
        fixture(&mut app, true);
        app.git_panel.selected = Some("README.md".into());
        app.git_panel.diff_pending = Some(Pending { id: 9, cwd: app.cwd.clone(), kind: Operation::Diff("README.md".into()) });
        app.apply_git_panel(GitServerMessage::FileDiffs { diffs: vec![neoism_protocol::git::GitFileDiff { path: "src/main.rs".into(), patch: "stale".into() }] }, 8);
        assert!(app.git_panel.diff_pending.is_some());
        assert!(!app.git_panel.lines.is_empty());
        app.apply_git_panel(GitServerMessage::FileDiffs { diffs: vec![neoism_protocol::git::GitFileDiff { path: "README.md".into(), patch: String::new() }] }, 9);
        assert!(app.git_panel.diff_pending.is_none());
        assert!(app.git_panel.lines.is_empty());
    }

    #[test]
    fn failed_commit_preserves_draft_and_success_never_erases_new_typing() {
        let mut app = App::new();
        fixture(&mut app, false);
        let sent = app.git_panel.message.text.clone();
        let reply = |error| GitServerMessage::ChangedFiles { files: vec![], branch: Some("main".into()), upstream: None, error };
        app.git_panel.pending = Some(Pending { id: 5, cwd: app.cwd.clone(), kind: Operation::Commit(sent.clone()) });
        app.apply_git_panel(reply(Some("hook rejected commit".into())), 5);
        assert_eq!(app.git_panel.message.text, sent);
        assert!(app.git_panel.error.is_some());
        app.git_panel.pending = Some(Pending { id: 6, cwd: app.cwd.clone(), kind: Operation::Commit(sent.clone()) });
        app.git_panel.message = TextEdit::new("Next commit draft");
        app.apply_git_panel(reply(None), 6);
        assert_eq!(app.git_panel.message.text, "Next commit draft");
        app.git_panel.pending = Some(Pending { id: 7, cwd: app.cwd.clone(), kind: Operation::Commit("Next commit draft".into()) });
        app.apply_git_panel(reply(None), 7);
        assert!(app.git_panel.message.text.is_empty());
    }

    #[test]
    fn workspace_change_and_reset_reject_in_flight_mutation() {
        let mut app = App::new();
        fixture(&mut app, false);
        app.git_panel.pending = Some(Pending { id: 17, cwd: "/previous".into(), kind: Operation::Write });
        app.apply_git_panel(GitServerMessage::Error { message: "old error".into() }, 17);
        assert!(app.git_panel.error.is_none());
        app.git_panel = State::default();
        app.apply_git_panel(GitServerMessage::ChangedFiles { files: vec![], branch: Some("stale".into()), upstream: None, error: None }, 17);
        assert!(!app.git_panel.loaded);
        assert!(app.git_panel.branch.is_none());
    }

    #[test]
    fn disconnect_releases_requests_without_replaying_an_uncertain_commit() {
        let mut app = App::new();
        fixture(&mut app, true);
        let draft = app.git_panel.message.text.clone();
        app.git_panel.pending = Some(Pending { id: 19, cwd: app.cwd.clone(), kind: Operation::Commit(draft.clone()) });
        app.git_panel.diff_pending = Some(Pending { id: 20, cwd: app.cwd.clone(), kind: Operation::Diff("src/main.rs".into()) });
        app.git_disconnected();
        assert!(app.git_panel.pending.is_none());
        assert!(app.git_panel.diff_pending.is_none());
        assert_eq!(app.git_panel.message.text, draft);
        assert!(app.git_panel.error.as_deref().unwrap().contains("before retrying"));
        app.apply_git_panel(GitServerMessage::ChangedFiles { files: vec![], branch: Some("stale".into()), upstream: None, error: None }, 19);
        assert_eq!(app.git_panel.files.len(), 3);
        assert_eq!(app.git_panel.message.text, draft);
    }
}