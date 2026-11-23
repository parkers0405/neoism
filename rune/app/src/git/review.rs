//! The diff: every changed file in one scroll (after t3code's diff panel), unified or side
//! by side, with a file list and comments that go to the agent's next message.

use super::*;
use neoism_protocol::git::GitFileDiff;

/// Every row of the diff is this tall, so a long diff mounts only what is on screen.
pub(super) const ROW: f32 = 22.0;
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
pub(super) enum Row { Gap, Header(usize), Line(usize, usize), Pair(usize, Option<usize>, Option<usize>), Note(usize) }

/// A comment shown under a line of a pull request's diff: a posted one, or one waiting to
/// go out with the review.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Note { pub file: usize, pub line: usize, pub author: String, pub body: String, pub pending: bool }

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
    pub(super) notes: Vec<Note>,
}

impl Review {
    /// Lay the files out as rows, pairing removed with added lines when side by side.
    pub(super) fn rebuild(&mut self) {
        self.rows.clear();
        let mut noted: std::collections::HashMap<(usize, usize), Vec<usize>> = Default::default();
        for (k, note) in self.notes.iter().enumerate() { noted.entry((note.file, note.line)).or_default().push(k); }
        for (f, file) in self.files.iter().enumerate() {
            if f > 0 { self.rows.push(Row::Gap); }
            self.rows.push(Row::Header(f));
            if file.collapsed { continue; }
            let notes = |rows: &mut Vec<Row>, i: usize| if let Some(found) = noted.get(&(f, i)) { rows.extend(found.iter().map(|k| Row::Note(*k))); };
            let mut i = 0;
            while i < file.lines.len() {
                // A hunk that opens the file needs no "skipped lines" marker above it.
                if file.lines[i].kind == Kind::Hunk && file.lines[i].new.is_some_and(|n| n <= 1) { i += 1; continue; }
                if !self.split || (file.lines[i].kind != Kind::Remove && file.lines[i].kind != Kind::Add) {
                    self.rows.push(Row::Line(f, i));
                    notes(&mut self.rows, i);
                    i += 1;
                    continue;
                }
                let removed = (i..file.lines.len()).take_while(|&k| file.lines[k].kind == Kind::Remove).count();
                let added = (i + removed..file.lines.len()).take_while(|&k| file.lines[k].kind == Kind::Add).count();
                for k in 0..removed.max(added) {
                    let (old, new) = ((k < removed).then_some(i + k), (k < added).then_some(i + removed + k));
                    self.rows.push(Row::Pair(f, old, new));
                    for side in [old, new].into_iter().flatten() { notes(&mut self.rows, side); }
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
                    old = a; new = b; in_hunk = true; kind = Kind::Hunk; new_no = Some(b);
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
        self.git_panel.pulls.viewing = None;
        self.git_panel.issues.open = None;
        if matches!(self.git_panel.review.scope, Scope::Pull(_)) { self.git_panel.review.notes.clear(); self.git_panel.review.scope = Scope::Uncommitted; self.git_panel.review.files.clear(); self.git_panel.review.rows.clear(); }
        self.git_panel.diff_error = None;
        self.git_panel.epoch = self.git_panel.epoch.wrapping_add(1);
        self.load_git_review();
    }

    /// Open the diff of what changed, starting at the file under the cursor.
    pub fn open_git_changes(&mut self) {
        let path = match self.git_panel.tree.get(self.git_panel.cursor) {
            Some(tree::Node::File { index, .. }) => self.git_panel.files.get(*index).map(|f| f.path.clone()),
            _ => None,
        }.unwrap_or_default();
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
        self.git_panel.review.notes.clear();
        self.git_panel.pulls.viewing = None;
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
        if matches!(self.git_panel.review.scope, Scope::Pull(_)) { return self.git_pull_draft(); }
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

/// A square icon button of a toolbar; its name appears under it while the pointer is on it.
pub(super) fn icon_tool(app: &App, key: &str, icon: crate::ui::feather::Icon, name: &str, on: bool) -> El {
    let s = app.ty.scale();
    let p = app.pal;
    el().key(key.to_owned()).w(30.0 * s).h(28.0 * s).center().radius(8.0 * s).hover_group()
        .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT }).border(s, rgb(if on { p.border } else { p.bg })).hover(rgb(p.hover))
        .child(crate::ui::feather::view(app, icon, 15.0 * s, if on { p.fg } else { p.dim }))
        .child(el().absolute(-30.0 * s, 32.0 * s).w(90.0 * s).center().group_opacity(0.0, 1.0)
            .child(el().h(20.0 * s).px(7.0 * s).center().radius(6.0 * s).fill(rgb(p.fg))
                .child(text(name.to_owned()).font(app.ty.sans(11.0, 500.0)).color(rgb(p.bg)))))
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
    let pull = matches!(review.scope, Scope::Pull(_));
    let mut bar = row().w_full().h(BAR_H * s).px(10.0 * s).gap(6.0 * s).center_y().border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border));
    if !pull {
        bar = bar.child(row().key("git-review-scope").h(28.0 * s).px(10.0 * s).gap(6.0 * s).center_y().radius(8.0 * s).border(s, rgb(p.border))
            .fill(if review.scope_menu { rgb(p.hover) } else { rgb(p.surface) }).hover(rgb(p.hover))
            .child(text(scope_name(review.scope)).font(app.ty.sans(12.5, 550.0)).color(rgb(p.fg)))
            .child(feather::view(app, if review.scope_menu { Icon::Up } else { Icon::Down }, 13.0 * s, p.muted))
            .on_press(|a: &mut App| a.git_panel.review.scope_menu = !a.git_panel.review.scope_menu));
    }
    let bar = bar
        .child(text(format!("{} {}", review.files.len(), if review.files.len() == 1 { "file" } else { "files" })).font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).ml(6.0 * s).truncate().shrink())
        .child(text(format!("+{added}")).font(app.ty.sized(11.0)).color(rgb(p.green)))
        .child(text(format!("\u{2212}{deleted}")).font(app.ty.sized(11.0)).color(rgb(p.red)))
        .child(el().grow(1.0))
        .child(icon_tool(app, "git-review-split", if review.split { Icon::Columns } else { Icon::Rows }, if review.split { "Side by side" } else { "Stacked" }, review.split).on_press(|a: &mut App| {
            a.git_panel.review.split = !a.git_panel.review.split;
            a.git_panel.review.target = None;
            a.git_review_relayout();
        }))
        .child(icon_tool(app, "git-review-fold", if all_collapsed { Icon::Unfold } else { Icon::Fold }, if all_collapsed { "Expand all" } else { "Collapse all" }, false).on_press(move |a: &mut App| {
            for file in &mut a.git_panel.review.files { file.collapsed = !all_collapsed; }
            a.git_review_relayout();
        }))
        .child(icon_tool(app, "git-review-files", Icon::SidePane, "File list", !review.hide_files).on_press(|a: &mut App| a.git_panel.review.hide_files = !a.git_panel.review.hide_files))
        // A pull request's own bar already has Refresh.
        .child_if(!pull, || icon_tool(app, "git-review-refresh", Icon::Refresh, "Refresh", false).on_press(|a: &mut App| a.load_git_review()));
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
        list = if state.diff_pending.is_some() { list.child(el().w_full().padding(18.0 * s).child(skeleton_page(app))) } else {
            list.child(text("No changes in this selection.").font(app.ty.sans(12.5, 400.0)).color(rgb(p.muted)).padding(18.0 * s))
        };
    } else if review.truncated {
        list = list.child(text("Only the first 300 files are shown.").font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).padding(14.0 * s));
    }
    let mut body = row().w_full().grow(1.0).shrink().h(0.0).child(list);
    if !review.hide_files && !review.files.is_empty() {
        let mut tree = col().key("git-review-tree").w(TREE_W * s).h_full().px(4.0 * s).border_sides([0.0, 0.0, 0.0, 1.0], rgb(p.border)).scroll(ScrollSpec { stick_bottom: false })
            .child(text("Files").font(app.ty.sans(11.5, 500.0)).color(rgb(p.muted)).px(8.0 * s).pt(10.0 * s).pb(6.0 * s));
        for node in tree::rows(review.files.iter().map(|f| f.path.as_str()), &state.closed) {
            let (path, name, ink, is_dir) = match &node {
                tree::Node::Dir { path, name, .. } => (path.clone(), name.clone(), p.fg, true),
                tree::Node::File { index, .. } => { let f = &review.files[*index]; (f.path.clone(), f.path.rsplit('/').next().unwrap_or(&f.path).to_owned(), p.dim, false) }
            };
            let tag = match &node { tree::Node::File { index, .. } => Some(status_tag(review.files[*index].status, p)), _ => None };
            let mut line = row().key(format!("git-review-file:{path}")).w_full().h(26.0 * s).px(8.0 * s).gap(6.0 * s).center_y().radius(6.0 * s).hover(rgb(p.hover))
                .child(tree::label(app, &node, &name, ink));
            if let Some((tag, color)) = tag { line = line.child(text(tag).font(app.ty.sized(10.5)).color(rgb(color))); }
            tree = tree.child(line.on_press(move |a: &mut App| {
                if is_dir {
                    if !a.git_panel.closed.remove(&path) { a.git_panel.closed.insert(path.clone()); }
                    a.git_panel.retree();
                    return;
                }
                if let Some(file) = a.git_panel.review.files.iter_mut().find(|f| f.path == path) { file.collapsed = false; }
                a.git_review_relayout();
                a.git_panel.review.jump = Some(path.clone());
            }));
        }
        body = body.child(tree);
    }
    let mut surface = col().key("git-review-surface").w_full().h_full().fill(rgb(p.bg)).clip_rounded(0.0);
    if pull { surface = surface.child(super::pulls::strip(app, true)); }
    let mut surface = surface.child(bar).child(body);
    // The comment being written, or what became of the last one.
    if let Some((f, i)) = review.target {
        if let (Some(file), Some(line)) = (review.files.get(f), review.files.get(f).and_then(|file| file.lines.get(i))) {
            let place = format!("{}{}", file.path, line.new.or(line.old).map(|n| format!(":{n}")).unwrap_or_default());
            surface = surface.child(col().key("git-comment").w_full().px(14.0 * s).py(12.0 * s).gap(8.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).fill(rgb(panel_bg(p)))
                .child(row().w_full().gap(8.0 * s).center_y()
                    .child(feather::view(app, Icon::Comment, 14.0 * s, p.dim))
                    .child(text(place).font(app.ty.sized(11.5)).color(rgb(p.fg)).truncate().shrink())
                    .child(text(line.text.trim().to_owned()).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().grow(1.0).shrink()))
                .child(rune::text_input(&review.comment, app.ty.sans(13.0, 400.0).lh(19.0 * s)).key("git-comment-input").w_full().h(92.0 * s).padding(10.0 * s)
                    .radius(10.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg))
                    .placeholder(if pull { "Comment on this line. Cmd+Enter adds it to your review." } else { "Comment for the agent. Cmd+Enter adds it to the chat." }, rgb(p.muted))
                    .on_edit(|a: &mut App| &mut a.git_panel.review.comment)
                    .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| match e.key {
                        Key::Enter if e.mods.cmd => { a.git_comment_submit(); true }
                        Key::Escape => { a.git_panel.review.target = None; a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1); true }
                        _ => false,
                    }))
                .child(row().w_full().gap(8.0 * s).center_y().child(el().grow(1.0))
                    .child(tool(app, "git-comment-cancel", "Cancel", false).on_press(|a: &mut App| { a.git_panel.review.target = None; a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1); }))
                    .child(el().key("git-comment-add").h(30.0 * s).px(14.0 * s).center().radius(8.0 * s).fill(rgb(p.fg))
                        .child(text(if pull { "Add to review" } else { "Add to chat" }).font(app.ty.sans(12.5, 600.0)).color(rgb(p.bg))).on_press(|a: &mut App| a.git_comment_submit()))));
        }
    } else if pull {
        surface = surface.child(super::pulls::finish(app));
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

/// A changed line's colour, and the wash behind its code and behind its number.
fn tint(kind: Kind, p: &crate::ui::palettes::Palette) -> Option<(u32, rune::Color, rune::Color)> {
    match kind {
        Kind::Add => Some((p.green, rgb(p.green).alpha(0.11), rgb(p.green).alpha(0.2))),
        Kind::Remove => Some((p.red, rgb(p.red).alpha(0.11), rgb(p.red).alpha(0.2))),
        _ => None,
    }
}

/// The width of a line-number column, in characters.
const DIGITS: usize = 5;

/// One side of a diff line as a single run of text: its numbers, its sign and its code in
/// the theme's syntax colours. One element per line keeps a fast scroll at the display's rate.
fn code(app: &App, lang: Option<&str>, line: &Line, numbers: &[Option<usize>]) -> El {
    use rune_kit::syntax::{Paint, Syntax, highlight};
    let p = app.pal;
    let mut out = String::with_capacity(line.text.len() + 16);
    for number in numbers {
        match number { Some(n) => out.push_str(&format!("{n:>DIGITS$} ")), None => out.push_str(&" ".repeat(DIGITS + 1)) }
    }
    let gutter = out.len();
    let (sign, sign_color) = match line.kind { Kind::Add => ("+ ", p.green), Kind::Remove => ("\u{2212} ", p.red), _ => ("  ", p.muted) };
    out.push(' ');
    out.push_str(sign);
    let start = out.len();
    out.push_str(&line.text);
    let mut spans = vec![rune::Span::new(0, gutter).color(rgb(p.muted)), rune::Span::new(gutter, start).color(rgb(sign_color))];
    spans.extend(highlight(lang, &line.text).iter().map(|(range, paint)| {
        let color = match paint {
            Paint::Kind(kind) => match kind {
                Syntax::Keyword => p.syn_keyword,
                Syntax::String => p.syn_string,
                Syntax::Number => p.syn_number,
                Syntax::Comment => p.syn_comment,
                Syntax::Type => p.syn_type,
                Syntax::Function => p.syn_func,
                Syntax::Property | Syntax::Attribute => p.syn_property,
                Syntax::Punctuation | Syntax::Operator => p.syn_special,
                _ => p.fg,
            },
            Paint::Color(_) => p.fg,
        };
        rune::Span::new(start + range.start, start + range.end).color(rgb(color))
    }));
    // A rich run wraps at its own measure; give it the line's real width so it never does.
    let style = app.ty.sized(12.0);
    let width = rune::text::advance("m", style).max(1.0) * out.chars().count() as f32 + 2.0;
    rune::rich(out, spans).font(style).color(rgb(p.fg)).w(width)
}

fn diff_row(app: &App, index: usize) -> El {
    use crate::ui::feather::{self, Icon};
    let p = app.pal;
    let s = app.ty.scale();
    let review = &app.git_panel.review;
    let Some(row_kind) = review.rows.get(index).copied() else { return el().h(ROW * s); };
    let lang = |f: usize| rune_kit::syntax::lang_for_path(&review.files[f].path);
    let pick = move |f: usize, i: usize| move |a: &mut App| {
        let review = &mut a.git_panel.review;
        review.target = if review.target == Some((f, i)) { None } else { Some((f, i)) };
        review.comment.focused = review.target.is_some();
        review.note = None;
        a.git_panel.epoch = a.git_panel.epoch.wrapping_add(1);
    };
    // One side of a line: a coloured edge and wash on the row, and one run of text in it.
    let side = |f: usize, i: usize, numbers: &[Option<usize>]| {
        let line = &review.files[f].lines[i];
        let chosen = review.target == Some((f, i));
        let (edge, wash) = tint(line.kind, p).map_or((rune::Color::TRANSPARENT, rune::Color::TRANSPARENT), |(c, w, _)| (rgb(c), w));
        row().w(0.0).grow(1.0).h_full().center_y().fill(if chosen { rgb(p.blue).alpha(0.18) } else { wash })
            .border_sides([0.0, 0.0, 0.0, 3.0], edge)
            .child(code(app, lang(f), line, numbers))
            .on_press(pick(f, i))
    };
    match row_kind {
        Row::Gap => el().w_full().h(ROW * s),
        Row::Header(f) => {
            let file = &review.files[f];
            let (tag, color) = status_tag(file.status, p);
            let (path, copy) = (file.path.clone(), file.path.clone());
            let (dir, name) = file.path.rsplit_once('/').map_or((String::new(), file.path.clone()), |(d, n)| (format!("{d}/"), n.to_owned()));
            row().key(format!("git-diff-file:{}", file.path)).w_full().h(ROW * s).px(10.0 * s).gap(8.0 * s).center_y()
                .fill(rgb(panel_bg(p))).border_sides([1.0, 0.0, 1.0, 0.0], rgb(p.border)).hover(rgb(p.hover))
                .child(feather::view(app, if file.collapsed { Icon::Right } else { Icon::Down }, 13.0 * s, p.muted))
                .child(row().key(format!("git-diff-open:{}", file.path)).center_y().shrink()
                    .child(text(dir).font(app.ty.sans(12.5, 400.0)).color(rgb(p.muted)).truncate().shrink())
                    .child(text(name).font(app.ty.sans(12.5, 600.0)).color(rgb(p.fg)).truncate().shrink())
                    .on_press(move |a: &mut App| a.open_file(&path, None)))
                .child(el().h(16.0 * s).px(5.0 * s).center().radius(4.0 * s).fill(rgb(color).alpha(0.16)).child(text(tag).font(app.ty.sized(10.0)).color(rgb(color))))
                .child(el().key(format!("git-diff-copy:{}", file.path)).h(18.0 * s).px(6.0 * s).center().radius(5.0 * s).hover(rgb(p.border))
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
            match line.kind {
                // Where lines were skipped: a quiet band, with the function it lands in.
                Kind::Hunk => {
                    let context = line.text.splitn(3, "@@").nth(2).unwrap_or("").trim().to_owned();
                    row().w_full().h(ROW * s).px(14.0 * s).gap(10.0 * s).center_y().fill(rgb(p.surface).alpha(0.7))
                        .child(text("\u{22ef}").font(app.ty.sized(12.0)).color(rgb(p.muted)))
                        .child(text(context).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().shrink())
                }
                Kind::Meta => row().w_full().h(ROW * s).px(14.0 * s).center_y()
                    .child(text(line.text.clone()).font(app.ty.sans(11.5, 400.0).italic()).color(rgb(p.muted)).truncate()),
                _ if review.split => row().w_full().h(ROW * s).child(side(f, i, &[line.old])).child(el().w(s).h_full().fill(rgb(p.border))).child(side(f, i, &[line.new])),
                _ => side(f, i, &[line.old, line.new]).w_full().h(ROW * s),
            }
        }
        Row::Pair(f, old, new) => {
            let lines = &review.files[f].lines;
            let half = |i: Option<usize>, left: bool| match i {
                Some(i) => side(f, i, &[if left { lines[i].old } else { lines[i].new }]),
                None => row().w(0.0).grow(1.0).h_full().fill(rgb(p.surface).alpha(0.45)),
            };
            row().w_full().h(ROW * s).child(half(old, true)).child(el().w(s).h_full().fill(rgb(p.border))).child(half(new, false))
        }
        Row::Note(k) => {
            let Some(note) = review.notes.get(k) else { return el().h(ROW * s); };
            let color = if note.pending { p.yellow } else { p.blue };
            row().key(format!("git-diff-note:{k}")).w_full().h(ROW * s).pl(96.0 * s).pr(12.0 * s).gap(8.0 * s).center_y().fill(rgb(color).alpha(0.1))
                .child(el().w(3.0 * s).h(14.0 * s).radius(1.5 * s).fill(rgb(color)))
                .child(text(note.author.clone()).font(app.ty.sans(12.0, 600.0)).color(rgb(p.fg)))
                .child(text(note.body.lines().next().unwrap_or("").to_owned()).font(app.ty.sans(12.0, 400.0)).color(rgb(p.dim)).truncate().grow(1.0).shrink())
                .child_if(note.pending, || pill(app, "pending", p.yellow))
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
        // a.rs: header + 5 lines; a gap; b.rs: header + 2 lines. A hunk that opens a file is not drawn.
        assert_eq!(review.rows.len(), 6 + 1 + 3);
        assert_eq!((review.row_of("a.rs"), review.row_of("b.rs")), (Some(0), Some(7)));
        review.files[0].collapsed = true;
        review.rebuild();
        assert_eq!((review.rows.len(), review.row_of("b.rs")), (1 + 1 + 3, Some(2)));
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
        // a.rs: header, keep, (old|new), (-|more), tail.
        let a: Vec<Row> = review.rows[..5].to_vec();
        assert_eq!(a[2], Row::Pair(0, Some(2), Some(3)));
        assert_eq!(a[3], Row::Pair(0, None, Some(4)));
        assert_eq!(a[4], Row::Line(0, 5));
        // A comment sits under the line it is on, in either layout.
        review.notes = vec![Note { file: 0, line: 4, author: "lin".into(), body: "Why?".into(), pending: true }];
        review.rebuild();
        assert_eq!((review.rows[3], review.rows[4]), (Row::Pair(0, None, Some(4)), Row::Note(0)));
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
