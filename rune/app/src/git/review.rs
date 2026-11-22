//! The diff: every changed file in one scroll (after t3code's diff panel), unified or side
//! by side, with a file list and comments that go to the agent's next message.

use super::*;
use neoism_protocol::git::GitFileDiff;

/// Every row of the diff is this tall, so a long diff mounts only what is on screen.
pub(super) const ROW: f32 = 24.0;
const TREE_W: f32 = 250.0;
const BAR_H: f32 = 42.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Scope {
    /// The working tree against HEAD.
    #[default]
    Uncommitted,
    /// Everything on this branch against the repository's default branch.
    Branch,
    /// A pull request's changes, as the host reports them.
    Pull(u64),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind { Context, Add, Remove, Hunk, Meta }

#[derive(Debug)]
pub(super) struct Line { pub kind: Kind, pub old: Option<usize>, pub new: Option<usize>, pub text: String }

#[derive(Debug)]
pub(super) struct FileDiff {
    pub path: String,
    pub status: GitChangeStatus,
    pub additions: u32,
    pub deletions: u32,
    pub lines: Vec<Line>,
    pub collapsed: bool,
}

/// One row of the scroll: a file's header, a unified line, or a side-by-side pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Row { Header(usize), Line(usize, usize), Pair(usize, Option<usize>, Option<usize>) }

#[derive(Default)]
pub struct Review {
    pub scope: Scope,
    pub(super) files: Vec<FileDiff>,
    pub(super) rows: Vec<Row>,
    pub split: bool,
    /// The file list is shown unless hidden.
    pub hide_files: bool,
    pub(super) scope_menu: bool,
    /// The line a comment is being written on: file and line index.
    pub(super) target: Option<(usize, usize)>,
    pub(super) comment: TextEdit,
    pub(super) note: Option<String>,
    /// The file to bring to the top once its rows exist.
    pub(super) jump: Option<String>,
    pub(super) truncated: bool,
}

impl Review {
    /// Lay the files out as rows, pairing removed with added lines when side by side.
    pub(super) fn rebuild(&mut self) {
        self.rows.clear();
        for (f, file) in self.files.iter().enumerate() {
            self.rows.push(Row::Header(f));
            if file.collapsed { continue; }
            if !self.split {
                self.rows.extend((0..file.lines.len()).map(|i| Row::Line(f, i)));
                continue;
            }
            let mut i = 0;
            while i < file.lines.len() {
                if file.lines[i].kind != Kind::Remove && file.lines[i].kind != Kind::Add {
                    self.rows.push(Row::Line(f, i));
                    i += 1;
                    continue;
                }
                let removed = (i..file.lines.len()).take_while(|&k| file.lines[k].kind == Kind::Remove).count();
                let added = (i + removed..file.lines.len()).take_while(|&k| file.lines[k].kind == Kind::Add).count();
                for k in 0..removed.max(added) {
                    self.rows.push(Row::Pair(f, (k < removed).then_some(i + k), (k < added).then_some(i + removed + k)));
                }
                i += removed + added;
            }
        }
    }

    pub(super) fn row_of(&self, path: &str) -> Option<usize> {
        let file = self.files.iter().position(|f| f.path == path)?;
        self.rows.iter().position(|r| *r == Row::Header(file))
    }

    pub(super) fn totals(&self) -> (u64, u64) {
        self.files.iter().fold((0, 0), |(a, d), f| (a + f.additions as u64, d + f.deletions as u64))
    }

    /// What the request for this scope is called, so an answer for another scope is dropped.
    pub(super) fn request_key(&self, base: Option<&str>) -> String {
        match self.scope { Scope::Uncommitted => "uncommitted".into(), Scope::Branch => format!("branch:{}", base.unwrap_or("")), Scope::Pull(n) => format!("pull:{n}") }
    }

    pub(super) fn set(&mut self, files: Vec<GitFileChange>, diffs: Vec<GitFileDiff>, truncated: bool) {
        let collapsed: std::collections::HashSet<String> = self.files.iter().filter(|f| f.collapsed).map(|f| f.path.clone()).collect();
        let mut patches: std::collections::HashMap<String, String> = diffs.into_iter().map(|d| (d.path, d.patch)).collect();
        self.files = files.into_iter().map(|f| FileDiff {
            lines: parse_patch(&patches.remove(&f.path).unwrap_or_default()),
            collapsed: collapsed.contains(&f.path),
            path: f.path, status: f.status, additions: f.additions, deletions: f.deletions,
        }).collect();
        self.truncated = truncated;
        self.target = None;
        self.rebuild();
    }
}

/// Unified patch parser. File headers aren't deletion/addition lines; zero-count ranges,
/// disjoint hunks, EOF notices and binary/rename-only patches retain their real semantics.
pub(super) fn parse_patch(patch: &str) -> Vec<Line> {
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
        // The path already heads the file; retain meaningful metadata (binary, rename,
        // modes) but omit redundant transport headers.
        if kind == Kind::Meta && (raw.starts_with("diff --git ") || raw.starts_with("index ") || raw.starts_with("--- ") || raw.starts_with("+++ ")) { continue; }
        rows.push(Line { kind, old: old_no, new: new_no, text: content.into() });
    }
    rows
}

impl App {
    /// Open the diff tab on everything that changed, with `path` brought to the top.
    pub fn open_git_review(&mut self, path: &str) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::GitDiff).map(|t| t.id) {
            self.select_tab(id);
        } else { self.open_tab(TabKind::GitDiff); }
        self.focus = Focus::Content;
        self.git_panel.message.focused = false;
        self.git_panel.selected = Some(path.into());
        self.git_panel.review.jump = Some(path.into());
        self.git_panel.pulls.open = None;
        self.git_panel.issues.open = None;
        if matches!(self.git_panel.review.scope, Scope::Pull(_)) { self.git_panel.review.scope = Scope::Uncommitted; self.git_panel.review.files.clear(); self.git_panel.review.rows.clear(); }
        self.git_panel.diff_error = None;
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
        self.load_git_review();
    }

    /// Open the diff of what changed, starting at the file under the cursor.
    pub fn open_git_changes(&mut self) {
        let path = self.git_panel.files.get(self.git_panel.cursor).map(|f| f.path.clone()).unwrap_or_default();
        self.open_git_review(&path);
    }

    pub(super) fn load_git_review(&mut self) {
        if self.git_panel.selected.is_none() { return; }
        if self.git_panel.diff_pending.is_some() {
            self.git_panel.diff_dirty = true;
            return;
        }
        self.git_panel.diff_dirty = false;
        let base = self.git_panel.sync.as_ref().and_then(|s| s.default_branch.clone());
        let key = self.git_panel.review.request_key(base.as_deref());
        let message = match (self.git_panel.review.scope, base) {
            (Scope::Branch, Some(base)) => GitClientMessage::DiffRange { base },
            (Scope::Branch, None) => {
                self.git_panel.diff_error = Some("This repository's default branch is unknown, so there is nothing to compare the branch against.".into());
                return;
            }
            // A pull request's files came with it; there is nothing to ask for.
            (Scope::Pull(_), _) => return,
            (Scope::Uncommitted, _) => GitClientMessage::DiffFiles { paths: self.git_panel.files.iter().map(|f| f.path.clone()).collect() },
        };
        self.git_panel.diff_pending = self.git_request(message, Operation::Diff(key));
        if self.git_panel.diff_pending.is_none() { self.git_panel.diff_error = self.git_panel.error.clone(); }
    }

    /// The answer to the diff request named `key`.
    pub(super) fn git_review_reply(&mut self, message: GitServerMessage, key: &str) {
        let base = self.git_panel.sync.as_ref().and_then(|s| s.default_branch.clone());
        if self.git_panel.review.request_key(base.as_deref()) != key { return; }
        match message {
            GitServerMessage::FileDiffs { diffs } => {
                let files = self.git_panel.files.clone();
                self.git_panel.review.set(files, diffs, false);
                self.git_panel.diff_error = None;
            }
            GitServerMessage::RangeDiff { files, diffs, truncated, .. } => {
                self.git_panel.review.set(files, diffs, truncated);
                self.git_panel.diff_error = None;
            }
            GitServerMessage::Error { message } => self.git_panel.diff_error = Some(message),
            _ => self.git_panel.diff_error = Some("Unexpected Git diff response.".into()),
        }
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
    }

    pub fn git_review_scope(&mut self, scope: Scope) {
        self.git_panel.review.scope_menu = false;
        if self.git_panel.review.scope == scope { return; }
        self.git_panel.review.scope = scope;
        self.git_panel.review.files.clear();
        self.git_panel.review.rows.clear();
        self.git_panel.review.target = None;
        self.git_panel.diff_pending = None;
        self.git_panel.diff_error = None;
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
        self.load_git_review();
    }

    /// For the headless scenes: lay the rows out again after changing a view option.
    pub fn git_review_fixture_relayout(&mut self) { self.git_review_relayout(); }

    fn git_review_relayout(&mut self) {
        self.git_panel.review.rebuild();
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
    }

    /// Bring a file's header to the top of the diff once it has rows (called each frame).
    pub fn git_review_tick(&mut self, tx: &mut Tx) {
        let Some(path) = self.git_panel.review.jump.clone() else { return; };
        if self.git_panel.diff_pending.is_some() { return; }
        self.git_panel.review.jump = None;
        if let Some(row) = self.git_panel.review.row_of(&path) {
            tx.scroll_to(review_key(self), 0.0, row as f32 * ROW * self.ty.scale());
        }
    }

    /// Put the comment being written into the agent's draft, quoted with its file and line.
    pub fn git_comment_submit(&mut self) {
        let review = &self.git_panel.review;
        let Some((f, i)) = review.target else { return; };
        let (Some(file), body) = (review.files.get(f), review.comment.text.trim().to_owned()) else { return; };
        let Some(line) = file.lines.get(i) else { return; };
        if body.is_empty() { return; }
        let number = line.new.or(line.old).map(|n| format!(" line {n}")).unwrap_or_default();
        let block = format!("In `{}`{number}:\n> {}\n{body}\n", file.path, line.text.trim_end());
        let Some(tab) = self.tabs.iter_mut().rev().find(|t| matches!(t.kind, TabKind::Agent { .. })) else {
            self.git_panel.review.note = Some("Open an agent chat first; the comment goes into its message box.".into());
            return;
        };
        let mut draft = tab.draft.text.clone();
        if !draft.is_empty() && !draft.ends_with('\n') { draft.push('\n'); }
        if !draft.is_empty() { draft.push('\n'); }
        draft.push_str(&block);
        tab.draft.replace_text(&draft);
        self.git_panel.review.target = None;
        self.git_panel.review.comment = TextEdit::default();
        self.git_panel.review.note = Some("Added to the agent's message. Send it from the chat.".into());
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
    }
}

pub(super) fn review_key(app: &App) -> String { format!("git-review:{}", app.cwd) }

fn tool(app: &App, key: &str, label: &str, on: bool) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    el().key(key.to_owned()).h(28.0 * s).px(10.0 * s).center().radius(8.0 * s)
        .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT }).border(s, rgb(if on { p.border } else { p.bg })).hover(rgb(p.hover))
        .child(text(label.to_owned()).font(app.ty.sans(12.0, 500.0)).color(rgb(if on { p.fg } else { p.dim })))
}

fn status_tag(status: GitChangeStatus, p: &crate::ui::palettes::Palette) -> (&'static str, u32) {
    match status {
        GitChangeStatus::Added | GitChangeStatus::Untracked => ("A", p.green),
        GitChangeStatus::Deleted => ("D", p.red),
        GitChangeStatus::Renamed => ("R", p.blue),
        GitChangeStatus::Conflict => ("!", p.red),
        _ => ("M", p.yellow),
    }
}

/// A normal pane surface, so switching tabs gives their keyboard and content back.
pub fn review(app: &App) -> El {
    use crate::ui::feather::{self, Icon};
    let p = app.pal;
    let s = app.ty.scale();
    let state = &app.git_panel;
    if state.selected.is_none() {
        return col().w_full().h_full().center().fill(rgb(p.bg))
            .child(tool(app, "git-review-open", "Open Git changes", true).on_press(|a: &mut App| {
                if !a.git_panel.open { a.toggle_git_panel(); }
                a.open_git_changes();
            }));
    }
    if state.pulls.open.is_some() { return super::pulls::detail(app); }
    if state.issues.open.is_some() { return super::issues::detail(app); }
    let review = &state.review;
    let base = state.sync.as_ref().and_then(|s| s.default_branch.clone());
    let scope_name = |scope: Scope| match scope {
        Scope::Uncommitted => "Uncommitted changes".to_owned(),
        Scope::Branch => format!("Branch against {}", base.as_deref().unwrap_or("the default branch")),
        Scope::Pull(n) => format!("Pull request #{n}"),
    };
    let (added, deleted) = review.totals();
    let all_collapsed = !review.files.is_empty() && review.files.iter().all(|f| f.collapsed);
    let bar = row().w_full().h(BAR_H * s).px(10.0 * s).gap(6.0 * s).center_y().border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border))
        .child(row().key("git-review-scope").h(28.0 * s).px(10.0 * s).gap(6.0 * s).center_y().radius(8.0 * s).border(s, rgb(p.border))
            .fill(if review.scope_menu { rgb(p.hover) } else { rgb(p.surface) }).hover(rgb(p.hover))
            .child(text(scope_name(review.scope)).font(app.ty.sans(12.5, 550.0)).color(rgb(p.fg)))
            .child(feather::view(app, if review.scope_menu { Icon::Up } else { Icon::Down }, 13.0 * s, p.muted))
            .on_press(|a: &mut App| a.git_panel.review.scope_menu = !a.git_panel.review.scope_menu))
        .child(text(format!("{} {}", review.files.len(), if review.files.len() == 1 { "file" } else { "files" })).font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).ml(6.0 * s).truncate().shrink())
        .child(text(format!("+{added}")).font(app.ty.sized(11.0)).color(rgb(p.green)))
        .child(text(format!("\u{2212}{deleted}")).font(app.ty.sized(11.0)).color(rgb(p.red)))
        .child(el().grow(1.0))
        .child(tool(app, "git-review-split", if review.split { "Side by side" } else { "Stacked" }, review.split).on_press(|a: &mut App| {
            a.git_panel.review.split = !a.git_panel.review.split;
            a.git_panel.review.target = None;
            a.git_review_relayout();
        }))
        .child(tool(app, "git-review-fold", if all_collapsed { "Expand all" } else { "Collapse all" }, false).on_press(move |a: &mut App| {
            for file in &mut a.git_panel.review.files { file.collapsed = !all_collapsed; }
            a.git_review_relayout();
        }))
        .child(tool(app, "git-review-files", "Files", !review.hide_files).on_press(|a: &mut App| a.git_panel.review.hide_files = !a.git_panel.review.hide_files))
        .child(tool(app, "git-review-refresh", "Refresh", false).on_press(|a: &mut App| a.load_git_review()));
    let rows = col().w_full().growth(rune::GrowthSpec::default()).virtual_rows(review.rows.len(), ROW * s,
        |_: &App, i| format!("diff:{i}").into(), |a: &App, i| diff_row(a, i))
        .virtual_rows_version(state.epoch);
    let mirror = state.diff_scroll.clone();
    let mut list = col().key(review_key(app)).h_full().grow(1.0).shrink().w(0.0)
        .scroll(ScrollSpec { stick_bottom: false }).scroll_anchor()
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(|a: &mut App, e: WheelEvent, tx: &mut Tx| a.git_review_wheel(&e, tx)).child(rows);
    if let Some(error) = &state.diff_error {
        list = list.child(text(error.clone()).font(app.ty.sans(12.5, 400.0)).color(rgb(p.red)).padding(18.0 * s).wrap());
    } else if review.files.is_empty() {
        list = list.child(text(if state.diff_pending.is_some() { "Loading diff\u{2026}" } else { "No changes in this selection." })
            .font(app.ty.sans(12.5, 400.0)).color(rgb(p.muted)).padding(18.0 * s));
    } else if review.truncated {
        list = list.child(text("Only the first 300 files are shown.").font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).padding(14.0 * s));
    }
    let mut body = row().w_full().grow(1.0).shrink().h(0.0).child(list);
    if !review.hide_files && !review.files.is_empty() {
        let mut tree = col().key("git-review-tree").w(TREE_W * s).h_full().border_sides([0.0, 0.0, 0.0, 1.0], rgb(p.border)).scroll(ScrollSpec { stick_bottom: false })
            .child(text("Files").font(app.ty.sans(11.5, 500.0)).color(rgb(p.muted)).px(12.0 * s).pt(10.0 * s).pb(6.0 * s));
        for file in &review.files {
            let (tag, color) = status_tag(file.status, p);
            let (dir, name) = file.path.rsplit_once('/').map_or(("", file.path.as_str()), |(d, n)| (d, n));
            let path = file.path.clone();
            tree = tree.child(row().key(format!("git-review-file:{}", file.path)).w_full().h(28.0 * s).px(12.0 * s).gap(8.0 * s).center_y().hover(rgb(p.hover))
                .child(text(tag).font(app.ty.sized(11.0)).color(rgb(color)))
                .child(text(name.to_owned()).font(app.ty.sans(12.5, 500.0)).color(rgb(p.fg)).truncate().shrink())
                .child(text(dir.to_owned()).font(app.ty.sans(11.5, 400.0)).color(rgb(p.muted)).truncate().grow(1.0).shrink())
                .on_press(move |a: &mut App| {
                    if let Some(file) = a.git_panel.review.files.iter_mut().find(|f| f.path == path) { if file.collapsed { file.collapsed = false; } }
                    a.git_review_relayout();
                    a.git_panel.review.jump = Some(path.clone());
                }));
        }
        body = body.child(tree);
    }
    let mut surface = col().key("git-review-surface").w_full().h_full().fill(rgb(p.bg)).clip_rounded(0.0).child(bar).child(body);
    // The comment being written, or what became of the last one.
    if let Some((f, i)) = review.target {
        if let (Some(file), Some(line)) = (review.files.get(f), review.files.get(f).and_then(|file| file.lines.get(i))) {
            let place = format!("{}{}", file.path, line.new.or(line.old).map(|n| format!(":{n}")).unwrap_or_default());
            surface = surface.child(row().key("git-comment").w_full().px(12.0 * s).py(10.0 * s).gap(8.0 * s).center_y().border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).fill(rgb(panel_bg(p)))
                .child(text(place).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().max_w(260.0 * s))
                .child(rune::text_input(&review.comment, app.ty.sans(12.5, 400.0)).key("git-comment-input").grow(1.0).w(0.0).h(32.0 * s).px(10.0 * s).single_line()
                    .radius(8.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Add a comment for the agent\u{2026}", rgb(p.muted))
                    .on_edit(|a: &mut App| &mut a.git_panel.review.comment)
                    .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| match e.key {
                        Key::Enter => { a.git_comment_submit(); true }
                        Key::Escape => { a.git_panel.review.target = None; a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1); true }
                        _ => false,
                    }))
                .child(tool(app, "git-comment-cancel", "Cancel", false).on_press(|a: &mut App| { a.git_panel.review.target = None; a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1); }))
                .child(el().key("git-comment-add").h(30.0 * s).px(12.0 * s).center().radius(8.0 * s).fill(rgb(p.fg))
                    .child(text("Add to chat").font(app.ty.sans(12.5, 600.0)).color(rgb(p.bg))).on_press(|a: &mut App| a.git_comment_submit())));
        }
    } else if let Some(note) = &review.note {
        surface = surface.child(row().key("git-comment-note").w_full().h(34.0 * s).px(12.0 * s).center_y().border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
            .child(text(note.clone()).font(app.ty.sans(12.0, 400.0)).color(rgb(p.dim)).grow(1.0))
            .child(tool(app, "git-comment-dismiss", "Dismiss", false).on_press(|a: &mut App| a.git_panel.review.note = None)));
    }
    if review.scope_menu {
        let mut menu = col().key("git-review-scope-menu").absolute(10.0 * s, (BAR_H - 2.0) * s).w(280.0 * s).padding(4.0 * s).gap(s)
            .radius(10.0 * s).fill(rgb(p.bg)).border(s, rgb(p.border)).on_press(|_: &mut App| {});
        if let Scope::Pull(number) = review.scope {
            menu = menu.child(row().key("git-scope:back").w_full().h(30.0 * s).px(8.0 * s).center_y().radius(7.0 * s).hover(rgb(p.hover))
                .child(text(format!("Back to pull request #{number}")).font(app.ty.sans(12.5, 500.0)).color(rgb(p.fg)))
                .on_press(move |a: &mut App| { a.git_panel.review.scope_menu = false; a.open_git_pull(number); }));
        }
        for scope in [Scope::Uncommitted, Scope::Branch] {
            let on = review.scope == scope;
            menu = menu.child(row().key(format!("git-scope:{scope:?}")).w_full().h(30.0 * s).px(8.0 * s).gap(8.0 * s).center_y().radius(7.0 * s)
                .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT }).hover(rgb(p.hover))
                .child(text(scope_name(scope)).font(app.ty.sans(12.5, if on { 600.0 } else { 400.0 })).color(rgb(if on { p.fg } else { p.dim })).grow(1.0))
                .child_if(on, || text("\u{f00c}").font(app.ty.icon_sized(10.0)).color(rgb(p.fg)))
                .on_press(move |a: &mut App| a.git_review_scope(scope)));
        }
        surface = surface.child(menu);
    }
    surface
}

fn ink(kind: Kind, p: &crate::ui::palettes::Palette) -> (&'static str, u32, rune::Color) {
    match kind {
        Kind::Add => ("+", p.green, rgb(p.green).alpha(0.09)),
        Kind::Remove => ("\u{2212}", p.red, rgb(p.red).alpha(0.09)),
        Kind::Hunk => ("", p.blue, rgb(p.surface)),
        Kind::Meta => ("", p.muted, rune::Color::TRANSPARENT),
        Kind::Context => ("", p.dim, rune::Color::TRANSPARENT),
    }
}

fn diff_row(app: &App, index: usize) -> El {
    use crate::ui::feather::{self, Icon};
    let p = app.pal;
    let s = app.ty.scale();
    let review = &app.git_panel.review;
    let Some(row_kind) = review.rows.get(index).copied() else { return el().h(ROW * s); };
    let number = |n: Option<usize>| text(n.map(|n| n.to_string()).unwrap_or_default()).font(app.ty.sized(11.0)).color(rgb(p.muted)).w(46.0 * s);
    match row_kind {
        Row::Header(f) => {
            let file = &review.files[f];
            let (tag, color) = status_tag(file.status, p);
            let (path, copy) = (file.path.clone(), file.path.clone());
            row().key(format!("git-diff-file:{}", file.path)).w_full().h(ROW * s).px(8.0 * s).gap(8.0 * s).center_y()
                .fill(rgb(p.surface)).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).hover(rgb(p.hover))
                .child(feather::view(app, if file.collapsed { Icon::Right } else { Icon::Down }, 13.0 * s, p.muted))
                .child(text(tag).font(app.ty.sized(11.0)).color(rgb(color)))
                .child(text(file.path.clone()).key(format!("git-diff-open:{}", file.path)).font(app.ty.sans(12.5, 600.0)).color(rgb(p.fg)).truncate().shrink()
                    .on_press(move |a: &mut App| a.open_file(&path, None)))
                .child(el().key(format!("git-diff-copy:{}", file.path)).h(20.0 * s).px(6.0 * s).center().radius(5.0 * s).hover(rgb(p.border))
                    .child(text("Copy path").font(app.ty.sans(11.0, 450.0)).color(rgb(p.muted)))
                    .on_press_tx(move |a: &mut App, tx: &mut Tx| { tx.copy(copy.clone()); a.clipboard = Some(copy.clone()); }))
                .child(el().grow(1.0))
                .child(text(format!("+{}", file.additions)).font(app.ty.sized(11.0)).color(rgb(p.green)))
                .child(text(format!("\u{2212}{}", file.deletions)).font(app.ty.sized(11.0)).color(rgb(p.red)))
                .on_press(move |a: &mut App| {
                    if let Some(file) = a.git_panel.review.files.get_mut(f) { file.collapsed = !file.collapsed; }
                    a.git_review_relayout();
                })
        }
        Row::Line(f, i) => {
            let line = &review.files[f].lines[i];
            let (sign, color, wash) = ink(line.kind, p);
            let chosen = review.target == Some((f, i));
            let mut el_row = row().w_full().h(ROW * s).fill(if chosen { rgb(p.blue).alpha(0.16) } else { wash }).center_y().clip_rounded(0.0)
                .child(number(line.old)).child(number(line.new))
                .child(text(sign).font(app.ty.sized(12.0)).color(rgb(color)).w(18.0 * s))
                .child(text(line.text.clone()).font(app.ty.sized(12.0)).color(rgb(color)));
            if matches!(line.kind, Kind::Add | Kind::Remove | Kind::Context) {
                el_row = el_row.on_press(move |a: &mut App| {
                    let review = &mut a.git_panel.review;
                    review.target = if review.target == Some((f, i)) { None } else { Some((f, i)) };
                    review.comment.focused = review.target.is_some();
                    review.note = None;
                    a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1);
                });
            }
            el_row
        }
        Row::Pair(f, old, new) => {
            let lines = &review.files[f].lines;
            let half = |i: Option<usize>, left: bool| {
                let Some(i) = i else { return row().w(0.0).grow(1.0).h_full().fill(rgb(p.surface).alpha(0.5)); };
                let line = &lines[i];
                let (sign, color, wash) = ink(line.kind, p);
                let chosen = review.target == Some((f, i));
                row().w(0.0).grow(1.0).h_full().center_y().clip_rounded(0.0).fill(if chosen { rgb(p.blue).alpha(0.16) } else { wash })
                    .child(number(if left { line.old } else { line.new }))
                    .child(text(sign).font(app.ty.sized(12.0)).color(rgb(color)).w(18.0 * s))
                    .child(text(line.text.clone()).font(app.ty.sized(12.0)).color(rgb(color)))
                    .on_press(move |a: &mut App| {
                        let review = &mut a.git_panel.review;
                        review.target = if review.target == Some((f, i)) { None } else { Some((f, i)) };
                        review.comment.focused = review.target.is_some();
                        review.note = None;
                        a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1);
                    })
            };
            row().w_full().h(ROW * s).child(half(old, true)).child(el().w(s).h_full().fill(rgb(p.border))).child(half(new, false))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    fn two_files() -> Review {
        let change = |path: &str| GitFileChange { path: path.into(), status: GitChangeStatus::Modified, additions: 2, deletions: 1, staged: false };
        let mut review = Review::default();
        review.set(vec![change("a.rs"), change("b.rs")], vec![
            GitFileDiff { path: "a.rs".into(), patch: "@@ -1,3 +1,4 @@\n keep\n-old\n+new\n+more\n tail\n".into() },
            GitFileDiff { path: "b.rs".into(), patch: "@@ -1 +1 @@\n-x\n+y\n".into() },
        ], false);
        review
    }

    #[test]
    fn files_stack_in_one_scroll_and_fold_away() {
        let mut review = two_files();
        // a.rs: header + hunk + 5 lines; b.rs: header + hunk + 2 lines.
        assert_eq!(review.rows.len(), 7 + 4);
        assert_eq!((review.row_of("a.rs"), review.row_of("b.rs")), (Some(0), Some(7)));
        review.files[0].collapsed = true;
        review.rebuild();
        assert_eq!((review.rows.len(), review.row_of("b.rs")), (1 + 4, Some(1)));
        // A reload keeps what was folded.
        let files: Vec<GitFileChange> = review.files.iter().map(|f| GitFileChange { path: f.path.clone(), status: f.status, additions: 0, deletions: 0, staged: false }).collect();
        review.set(files, vec![], false);
        assert!(review.files[0].collapsed && !review.files[1].collapsed);
    }

    #[test]
    fn side_by_side_pairs_removed_with_added_lines() {
        let mut review = two_files();
        review.split = true;
        review.rebuild();
        // a.rs: header, hunk, keep, (old|new), (-|more), tail.
        let a: Vec<Row> = review.rows[..6].to_vec();
        assert_eq!(a[3], Row::Pair(0, Some(2), Some(3)));
        assert_eq!(a[4], Row::Pair(0, None, Some(4)));
        assert_eq!(a[5], Row::Line(0, 5));
    }

    #[test]
    fn a_comment_goes_into_the_agent_draft_with_its_file_and_line() {
        let mut app = App::new();
        app.git_panel.review = two_files();
        app.git_panel.review.target = Some((0, 4));
        app.git_panel.review.comment = TextEdit::new("Use the helper here");
        app.git_comment_submit();
        assert!(app.git_panel.review.note.as_deref().is_some_and(|n| n.contains("Open an agent chat")), "with no chat open, nothing is lost");
        assert_eq!(app.git_panel.review.target, Some((0, 4)));
        app.open_tab(TabKind::Agent { session: None });
        app.git_comment_submit();
        let draft = app.tabs.iter().find(|t| matches!(t.kind, TabKind::Agent { .. })).unwrap().draft.text.clone();
        assert_eq!(draft, "In `a.rs` line 3:\n> more\nUse the helper here\n");
        assert!(app.git_panel.review.target.is_none() && app.git_panel.review.comment.text.is_empty());
    }
}
