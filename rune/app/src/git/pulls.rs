//! Pull requests: the panel's list and, in the diff tab, one pull request with its checks,
//! conversation, review threads and the actions the daemon offers (review, merge, checkout).

use super::*;
use neoism_protocol::pull_request::*;

#[derive(Default)]
pub struct Pulls {
    pub(super) filter: Option<PullRequestFilter>,
    pub(super) query: TextEdit,
    pub(super) items: Vec<PullRequestInboxItem>,
    pub(super) total: u32,
    pub(super) loaded: bool,
    pub(super) error: Option<String>,
    generation: u64,
    listing: Option<Pending>,
    /// The pull request open in the diff tab, and what is known of it.
    pub open: Option<u64>,
    pub detail: Option<PullRequestDetail>,
    reading: Option<Pending>,
    acting: Option<Pending>,
    pub(super) working: Option<&'static str>,
    pub(super) notice: Option<String>,
    pub(super) detail_error: Option<String>,
    pub(super) body: TextEdit,
    pub(super) merge_menu: bool,
    /// The pull request on screen, on its Conversation tab (`open`) or its files.
    pub viewing: Option<u64>,
    /// Line comments written but not yet sent; they go out with the review.
    pub(super) drafts: Vec<PullRequestInlineComment>,
    /// Pull requests already read, so opening one again shows at once while it refreshes.
    cache: std::collections::HashMap<u64, PullRequestDetail>,
    /// Pull requests still to read ahead of a click, and the one being read.
    ahead: std::collections::VecDeque<u64>,
    fetching: Option<Pending>,
}

impl Pulls {
    pub(super) fn filter(&self) -> PullRequestFilter { self.filter.unwrap_or(PullRequestFilter::Open) }
    pub(super) fn busy(&self) -> bool { self.acting.is_some() }
    pub(super) fn disconnected(&mut self) {
        (self.listing, self.reading, self.fetching) = (None, None, None);
        self.ahead.clear();
        if self.acting.take().is_some() { self.detail_error = Some("Connection lost. Refresh the pull request before trying again.".into()); }
        self.working = None;
    }
}

/// How many of a list's pull requests are read ahead of a click.
const AHEAD: usize = 5;

/// `2026-11-20T10:00:00Z` as `2026-11-20`.
pub(super) fn day(stamp: &str) -> &str { stamp.get(..10).unwrap_or(stamp) }

/// A pull request's state as a word and a colour.
pub(super) fn state_of(summary: &PullRequestSummary, p: &crate::ui::palettes::Palette) -> (&'static str, u32) {
    if summary.draft && summary.state.eq_ignore_ascii_case("open") { ("draft", p.muted) }
    else if summary.state.eq_ignore_ascii_case("open") { ("open", p.green) }
    else if summary.state.eq_ignore_ascii_case("merged") { ("merged", p.magenta) }
    else { ("closed", p.red) }
}

fn checks_of(state: &str, p: &crate::ui::palettes::Palette) -> Option<(&'static str, u32)> {
    match state.to_ascii_uppercase().as_str() {
        "SUCCESS" => Some(("checks pass", p.green)),
        "FAILURE" | "ERROR" => Some(("checks fail", p.red)),
        "PENDING" | "EXPECTED" => Some(("checks running", p.yellow)),
        _ => None,
    }
}

impl App {
    pub fn git_pulls_load(&mut self) {
        let pulls = &mut self.git_panel.pulls;
        pulls.generation = pulls.generation.wrapping_add(1);
        let request = PullRequestRequest::List { query: pulls.query.text.trim().to_owned(), sort: PullRequestSort::Updated, filter: pulls.filter(), cursor: None, generation: pulls.generation };
        pulls.error = None;
        self.git_panel.pulls.listing = self.git_request(GitClientMessage::PullRequest { request }, Operation::Remote);
        if self.git_panel.pulls.listing.is_none() { self.git_panel.pulls.error = self.git_panel.error.take(); }
    }

    /// Open a pull request in the diff tab.
    pub fn open_git_pull(&mut self, number: u64) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::GitDiff).map(|t| t.id) { self.select_tab(id); } else { self.open_tab(TabKind::GitDiff); }
        self.focus = Focus::Content;
        let keep = self.git_panel.pulls.cache.get(&number).cloned();
        self.git_panel.issues.open = None;
        let pulls = &mut self.git_panel.pulls;
        if pulls.viewing != Some(number) { pulls.drafts.clear(); }
        pulls.open = Some(number);
        pulls.viewing = Some(number);
        pulls.detail = keep;
        pulls.detail_error = None;
        pulls.notice = None;
        pulls.body = TextEdit::default();
        pulls.merge_menu = false;
        self.git_panel.selected = Some(String::new());
        self.git_pull_read();
    }

    /// Read the next listed pull request that is not known yet, one at a time, so opening
    /// it later is instant. A click always goes first: this waits while one is being read.
    fn git_pull_ahead(&mut self) {
        let pulls = &mut self.git_panel.pulls;
        if pulls.fetching.is_some() || pulls.reading.is_some() { return; }
        while let Some(number) = pulls.ahead.pop_front() {
            if pulls.cache.contains_key(&number) { continue; }
            self.git_panel.pulls.fetching = self.git_request(GitClientMessage::PullRequest { request: PullRequestRequest::Read { number } }, Operation::Remote);
            return;
        }
    }

    pub fn git_pull_read(&mut self) {
        let Some(number) = self.git_panel.pulls.viewing else { return; };
        self.git_panel.pulls.reading = self.git_request(GitClientMessage::PullRequest { request: PullRequestRequest::Read { number } }, Operation::Remote);
    }

    /// Run a change to the open pull request; it is read again when the host confirms.
    pub(super) fn git_pull_act(&mut self, working: &'static str, request: PullRequestRequest) {
        if self.git_panel.pulls.busy() { return; }
        let pulls = &mut self.git_panel.pulls;
        (pulls.detail_error, pulls.notice, pulls.merge_menu) = (None, None, false);
        self.git_panel.pulls.acting = self.git_request(GitClientMessage::PullRequest { request }, Operation::Remote);
        self.git_panel.pulls.working = self.git_panel.pulls.acting.is_some().then_some(working);
    }

    pub(super) fn git_pull_review(&mut self, event: PullRequestReviewEvent) {
        let Some(detail) = &self.git_panel.pulls.detail else { return; };
        let body = self.git_panel.pulls.body.text.trim().to_owned();
        let comments = self.git_panel.pulls.drafts.clone();
        if body.is_empty() && comments.is_empty() && event != PullRequestReviewEvent::Approve {
            self.git_panel.pulls.detail_error = Some("Write something to send with the review.".into());
            return;
        }
        let request = PullRequestRequest::SubmitReview { number: detail.summary.number, expected_head: detail.summary.head_sha.clone(), body, event, comments };
        self.git_pull_act("Sending review\u{2026}", request);
    }

    /// Show the open pull request's changes in the diff view, line by line.
    pub(super) fn git_pull_diff(&mut self) {
        let Some(detail) = &self.git_panel.pulls.detail else { return; };
        let (number, files, diffs, truncated) = (detail.summary.number, detail.files.clone(), detail.diffs.clone(), detail.truncated);
        self.git_panel.review.scope = Scope::Pull(number);
        self.git_panel.review.set(files, diffs, truncated);
        self.git_panel.diff_error = None;
        self.git_panel.diff_pending = None;
        self.git_panel.pulls.open = None;
        self.git_pull_notes();
    }

    /// For the headless scenes: the files tab with a comment waiting.
    pub fn git_pull_files_fixture(&mut self) {
        self.git_pull_diff();
        self.git_panel.pulls.drafts.push(PullRequestInlineComment { path: "src/git.rs".into(), side: PullRequestSide::Right, line: 41, start_line: None, body: "Name this after the panel?".into() });
        self.git_pull_notes();
    }

    /// Put the posted threads and the waiting drafts under the lines they are on.
    pub(super) fn git_pull_notes(&mut self) {
        let state = &mut self.git_panel;
        let review = &state.review;
        let place = |path: &str, side: PullRequestSide, line: u32| -> Option<(usize, usize)> {
            let f = review.files.iter().position(|f| f.path == path)?;
            let i = review.files[f].lines.iter().position(|l| match side {
                PullRequestSide::Right => l.kind != review::Kind::Remove && l.new == Some(line as usize),
                PullRequestSide::Left => l.kind != review::Kind::Add && l.old == Some(line as usize),
            })?;
            Some((f, i))
        };
        let mut notes = Vec::new();
        for thread in state.pulls.detail.iter().flat_map(|d| &d.threads) {
            let Some((file, line)) = thread.line.and_then(|line| place(&thread.path, thread.side, line)) else { continue; };
            notes.extend(thread.comments.iter().map(|c| review::Note { file, line, author: c.author.clone(), body: c.body.clone(), pending: false }));
        }
        for draft in &state.pulls.drafts {
            if let Some((file, line)) = place(&draft.path, draft.side, draft.line) {
                notes.push(review::Note { file, line, author: "You".into(), body: draft.body.clone(), pending: true });
            }
        }
        state.review.notes = notes;
        state.review.rebuild();
        state.epoch = state.epoch.wrapping_add(1);
    }

    /// Keep the comment being written on a line as a draft of the review.
    pub(super) fn git_pull_draft(&mut self) {
        let review = &self.git_panel.review;
        let (Some((f, i)), body) = (review.target, review.comment.text.trim().to_owned()) else { return; };
        let Some((file, line)) = review.files.get(f).and_then(|file| Some((file, file.lines.get(i)?))) else { return; };
        if body.is_empty() { return; }
        let (side, number) = match (line.kind, line.old, line.new) {
            (review::Kind::Remove, Some(old), _) => (PullRequestSide::Left, old),
            (_, _, Some(new)) => (PullRequestSide::Right, new),
            _ => return,
        };
        self.git_panel.pulls.drafts.push(PullRequestInlineComment { path: file.path.clone(), side, line: number as u32, start_line: None, body });
        self.git_panel.review.target = None;
        self.git_panel.review.comment = TextEdit::default();
        self.git_pull_notes();
    }

    /// A reply to one of this tab's requests. `false` when it is not one of them.
    pub(super) fn git_pulls_reply(&mut self, message: &GitServerMessage, request_id: u64) -> bool {
        let cwd = self.cwd.clone();
        let hit = |p: &Option<Pending>| p.as_ref().is_some_and(|p| p.matches(request_id, &cwd));
        let pulls = &mut self.git_panel.pulls;
        let error = |message: &GitServerMessage| match message {
            GitServerMessage::PullRequest { reply: PullRequestReply::Error { message, .. } } | GitServerMessage::Error { message } => Some(message.clone()),
            _ => None,
        };
        if hit(&pulls.listing) {
            pulls.listing = None;
            pulls.loaded = true;
            match message {
                GitServerMessage::PullRequest { reply: PullRequestReply::Listed { page } } if page.generation == pulls.generation => {
                    (pulls.items, pulls.total, pulls.error) = (page.items.clone(), page.total_count, None);
                    pulls.ahead = page.items.iter().take(AHEAD).map(|i| i.summary.number).collect();
                }
                other => if let Some(e) = error(other) { pulls.error = Some(e); },
            }
            self.git_pull_ahead();
            return true;
        }
        if hit(&pulls.fetching) {
            pulls.fetching = None;
            if let GitServerMessage::PullRequest { reply: PullRequestReply::Detail { detail } } = message {
                pulls.cache.insert(detail.summary.number, detail.clone());
                if pulls.viewing == Some(detail.summary.number) && pulls.detail.is_none() { pulls.detail = Some(detail.clone()); }
            }
            self.git_pull_ahead();
            return true;
        }
        if hit(&pulls.reading) {
            pulls.reading = None;
            match message {
                GitServerMessage::PullRequest { reply: PullRequestReply::Detail { detail } } => {
                    pulls.cache.insert(detail.summary.number, detail.clone());
                    if pulls.viewing == Some(detail.summary.number) {
                        (pulls.detail, pulls.detail_error) = (Some(detail.clone()), None);
                        if self.git_panel.review.scope == Scope::Pull(detail.summary.number) { self.git_pull_notes(); }
                    }
                }
                other => if let Some(e) = error(other) { pulls.detail_error = Some(e); },
            }
            self.git_pull_ahead();
            return true;
        }
        if hit(&pulls.acting) {
            pulls.acting = None;
            pulls.working = None;
            match message {
                GitServerMessage::PullRequest { reply } => match reply {
                    PullRequestReply::ReviewSubmitted { .. } => { pulls.notice = Some("Review sent.".into()); pulls.body = TextEdit::default(); pulls.drafts.clear(); }
                    PullRequestReply::ThreadUpdated { .. } => pulls.notice = Some("Thread updated.".into()),
                    PullRequestReply::Merged { number } => pulls.notice = Some(format!("Merged #{number}.")),
                    PullRequestReply::CheckedOut { number, worktree, path } => {
                        pulls.notice = Some(if *worktree { format!("Checked out #{number} in {path}.") } else { format!("Checked out #{number}.") });
                    }
                    PullRequestReply::Error { message, .. } => pulls.detail_error = Some(message.clone()),
                    _ => {}
                },
                other => if let Some(e) = error(other) { pulls.detail_error = Some(e); },
            }
            self.git_pull_read();
            if self.git_panel.open { self.refresh_git_panel(); }
            return true;
        }
        false
    }
}

/// The Pull requests tab of the panel.
pub(super) fn list(app: &App, card: El) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let pulls = &app.git_panel.pulls;
    let mut filters = row().w_full().gap(2.0 * s).padding(3.0 * s).radius(9.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border));
    for (name, filter) in [("Open", PullRequestFilter::Open), ("Merged", PullRequestFilter::Merged), ("Closed", PullRequestFilter::Closed), ("All", PullRequestFilter::All)] {
        let on = pulls.filter() == filter;
        filters = filters.child(el().key(format!("git-pulls-filter:{name}")).w(0.0).grow(1.0).h(24.0 * s).center().radius(6.0 * s)
            .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT })
            .child(text(name).font(app.ty.sans(12.0, if on { 600.0 } else { 450.0 })).color(rgb(if on { p.fg } else { p.muted })))
            .on_press(move |a: &mut App| { a.git_panel.pulls.filter = Some(filter); a.git_pulls_load(); }));
    }
    let search = rune::text_input(&pulls.query, app.ty.sans(12.5, 400.0)).key("git-pulls-query").w_full().h(30.0 * s).px(10.0 * s).py(7.0 * s).single_line()
        .radius(8.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Search pull requests, then Enter", rgb(p.muted))
        .on_edit(|a: &mut App| { a.focus = Focus::Details; &mut a.git_panel.pulls.query })
        .on_input_key(|a: &mut App, e: &KeyEvent, tx: &mut Tx| {
            if e.key == Key::Enter { a.git_pulls_load(); return true; }
            if e.key == Key::Escape { a.git_panel.pulls.query.focused = false; return true; }
            crate::keys::shortcut(a, e, tx)
        });
    let mut list = col().key("git-pulls").w_full().grow(1.0).shrink().h(0.0).scroll(ScrollSpec { stick_bottom: false }).pt(4.0 * s);
    if let Some(error) = &pulls.error {
        list = list.child(text(error.clone()).font(app.ty.sans(12.0, 400.0)).color(rgb(p.red)).padding(14.0 * s).wrap());
    } else if !pulls.loaded {
        list = list.child(skeleton(app, 5));
    } else if pulls.items.is_empty() {
        list = list.child(text("No pull requests here.").font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).padding(14.0 * s));
    }
    for item in &pulls.items {
        let summary = &item.summary;
        let number = summary.number;
        let (state, color) = state_of(summary, p);
        let on = pulls.viewing == Some(number);
        let mut meta = row().gap(6.0 * s).center_y().child(pill(app, state, color));
        if let Some((label, color)) = checks_of(&summary.checks_state, p) {
            meta = meta.child(row().gap(4.0 * s).center_y().child(el().w(6.0 * s).h(6.0 * s).radius(3.0 * s).fill(rgb(color)))
                .child(text(label).font(app.ty.sans(10.5, 450.0)).color(rgb(p.muted))));
        }
        meta = meta.child(el().grow(1.0))
            .child(text(format!("+{}", item.additions)).font(app.ty.sized(10.0)).color(rgb(p.green)))
            .child(text(format!("\u{2212}{}", item.deletions)).font(app.ty.sized(10.0)).color(rgb(p.red)));
        list = list.child(col().key(format!("git-pull:{number}")).w_full().px(14.0 * s).py(9.0 * s).gap(4.0 * s)
            .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(text(summary.title.clone()).font(app.ty.sans(13.0, 600.0)).color(rgb(p.fg)).truncate().w_full())
            .child(text(format!("#{number} \u{b7} {} \u{b7} {}", summary.author, day(&item.updated_at))).font(app.ty.sans(11.5, 400.0)).color(rgb(p.muted)).truncate().w_full())
            .child(meta.w_full())
            .on_press(move |a: &mut App| a.open_git_pull(number)));
    }
    let note = if pulls.loaded && pulls.error.is_none() && pulls.total as usize > pulls.items.len() { format!("Showing {} of {}", pulls.items.len(), pulls.total) } else { String::new() };
    card.child(col().w_full().padding(12.0 * s).gap(8.0 * s).child(filters).child(search))
        .child(list.border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)))
        .child_if(!note.is_empty(), || text(note.clone()).font(app.ty.sans(11.0, 400.0)).color(rgb(p.muted)).px(14.0 * s).py(8.0 * s))
}

/// A filled or outlined button of the detail pages.
pub(super) fn action(app: &App, key: &str, label: &str, filled: bool, on: bool) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let ink = if !on { p.muted } else if filled { p.bg } else { p.fg };
    let mut b = el().key(key.to_owned()).h(30.0 * s).px(12.0 * s).center().radius(8.0 * s)
        .fill(rgb(if filled && on { p.fg } else { p.surface })).border(s, rgb(if filled && on { p.fg } else { p.border }))
        .child(text(label.to_owned()).font(app.ty.sans(12.5, 550.0)).color(rgb(ink)));
    if on && !filled { b = b.hover(rgb(p.hover)); }
    b
}

/// A titled block of a detail page.
pub(super) fn section(app: &App, title: &str, count: Option<usize>) -> El {
    let s = app.ty.scale();
    let mut head = row().w_full().gap(8.0 * s).center_y().child(text(title.to_owned()).font(app.ty.sans(13.0, 650.0)).color(rgb(app.pal.fg)));
    if let Some(n) = count { head = head.child(text(n.to_string()).font(app.ty.sans(11.0, 550.0)).color(rgb(app.pal.muted))); }
    col().w_full().gap(8.0 * s).child(head)
}

/// One comment: who, when, what.
pub(super) fn comment(app: &App, author: &str, when: &str, body: &str) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    col().w_full().padding(12.0 * s).gap(6.0 * s).radius(10.0 * s).fill(rgb(panel_bg(p))).border(s, rgb(p.border))
        .child(row().w_full().gap(8.0 * s).center_y()
            .child(text(author.to_owned()).font(app.ty.sans(12.5, 600.0)).color(rgb(p.fg)))
            .child(text(day(when).to_owned()).font(app.ty.sans(11.5, 400.0)).color(rgb(p.muted))))
        .child(text(body.trim().to_owned()).font(app.ty.sans(13.0, 400.0).lh(19.0 * s)).color(rgb(p.dim)).wrap().w_full())
}

/// Under a pull request's files: the comments waiting, a line for the review, and how to send it.
pub(super) fn finish(app: &App) -> El {
    use PullRequestReviewEvent::*;
    let p = app.pal;
    let s = app.ty.scale();
    let pulls = &app.git_panel.pulls;
    let busy = pulls.busy();
    let waiting = pulls.drafts.len();
    let note = match (&pulls.detail_error, pulls.working, &pulls.notice) {
        (Some(error), _, _) => Some((error.clone(), p.red)),
        (None, Some(working), _) => Some((working.to_owned(), p.dim)),
        (None, None, Some(notice)) => Some((notice.clone(), p.green)),
        _ => None,
    };
    let mut bar = col().key("git-pull-finish").w_full().px(12.0 * s).py(10.0 * s).gap(6.0 * s).border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)).fill(rgb(panel_bg(p)));
    if let Some((note, color)) = note { bar = bar.child(text(note).font(app.ty.sans(12.0, 400.0)).color(rgb(color)).truncate().w_full()); }
    bar.child(row().w_full().gap(8.0 * s).center_y()
        .child(text(match waiting { 0 => "Click a line to comment".to_owned(), 1 => "1 comment waiting".to_owned(), n => format!("{n} comments waiting") })
            .font(app.ty.sans(12.0, if waiting > 0 { 600.0 } else { 400.0 })).color(rgb(if waiting > 0 { p.yellow } else { p.muted })))
        .child(rune::text_input(&pulls.body, app.ty.sans(12.5, 400.0)).key("git-pull-finish-body").grow(1.0).w(0.0).h(32.0 * s).px(10.0 * s).py(8.0 * s).single_line()
            .radius(8.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Overall comment (optional)", rgb(p.muted))
            .on_edit(|a: &mut App| &mut a.git_panel.pulls.body)
            .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| { if e.key == Key::Escape { a.git_panel.pulls.body.focused = false; true } else { false } }))
        .child(action(app, "git-pull-finish:comment", "Comment", false, !busy).on_press(|a: &mut App| a.git_pull_review(Comment)))
        .child(action(app, "git-pull-finish:changes", "Request changes", false, !busy).on_press(|a: &mut App| a.git_pull_review(RequestChanges)))
        .child(action(app, "git-pull-finish:approve", "Approve", true, !busy).on_press(|a: &mut App| a.git_pull_review(Approve))))
}

/// The head of a pull request's pages: what it is, then one bar with its two tabs on the
/// left and, on the right, what can be done from the tab that is showing.
pub(super) fn strip(app: &App, files: bool) -> El {
    use crate::ui::feather::Icon;
    use review::icon_tool;
    let p = app.pal;
    let s = app.ty.scale();
    let pulls = &app.git_panel.pulls;
    let number = pulls.viewing.unwrap_or_default();
    let detail = pulls.detail.as_ref().filter(|d| d.summary.number == number);
    let known = detail.map(|d| &d.summary).or_else(|| pulls.items.iter().map(|i| &i.summary).find(|i| i.number == number));
    let tab = |key: &str, name: String, on: bool| col().key(key.to_owned()).h_full().px(4.0 * s).center()
        .child(text(name).font(app.ty.sans(12.5, if on { 600.0 } else { 450.0 })).color(rgb(if on { p.fg } else { p.muted })))
        .child(el().absolute(0.0, 36.0 * s).w_full().h(2.0 * s).radius(s).fill(if on { rgb(p.fg) } else { rune::Color::TRANSPARENT }));
    let mut title = row().w_full().px(16.0 * s).pt(12.0 * s).pb(4.0 * s).gap(8.0 * s).center_y();
    if let Some(summary) = known {
        let (state, color) = state_of(summary, p);
        title = title.child(text(summary.title.clone()).font(app.ty.sans(15.0, 650.0)).color(rgb(p.fg)).truncate().shrink())
            .child(text(format!("#{number}")).font(app.ty.sized(12.0)).color(rgb(p.muted)))
            .child(pill(app, state, color))
            .child(text(format!("{}  \u{2192}  {}", summary.head_branch, summary.base_branch)).font(app.ty.sized(11.0)).color(rgb(p.muted)).truncate().shrink());
    } else {
        title = title.child(text(format!("Pull request #{number}")).font(app.ty.sans(15.0, 650.0)).color(rgb(p.fg)));
    }
    let mut bar = row().w_full().h(38.0 * s).px(16.0 * s).gap(14.0 * s).center_y().border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border))
        .child(tab("git-pull-tab:conversation", "Conversation".into(), !files).on_press(move |a: &mut App| a.open_git_pull(number)))
        .child(tab("git-pull-tab:files", detail.map_or("Files changed".into(), |d| format!("Files changed  {}", d.files.len())), files).on_press(|a: &mut App| a.git_pull_diff()))
        .child(el().grow(1.0));
    if let Some(summary) = known {
        let (url, head, busy) = (summary.url.clone(), summary.head_sha.clone(), pulls.busy());
        let open = summary.state.eq_ignore_ascii_case("open");
        let mut actions = row().gap(4.0 * s).center_y();
        if open && !summary.draft && detail.is_some() {
            actions = actions.child(icon_tool(app, "git-pull-merge", Icon::Merge, "Merge", pulls.merge_menu).on_press(|a: &mut App| a.git_panel.pulls.merge_menu = !a.git_panel.pulls.merge_menu));
        }
        actions = actions
            .child(icon_tool(app, "git-pull-checkout", Icon::Download, "Check out", false).on_press(move |a: &mut App| {
                if !busy { a.git_pull_act("Checking out\u{2026}", PullRequestRequest::Checkout { number, expected_head: head.clone(), new_worktree: false }); }
            }))
            .child(icon_tool(app, "git-pull-browser", Icon::External, "Open on GitHub", false).on_press_tx(move |_: &mut App, tx: &mut Tx| { let _ = tx.open_url(&url); }))
            .child(icon_tool(app, "git-pull-refresh", Icon::Refresh, "Refresh", false).on_press(|a: &mut App| a.git_pull_read()));
        bar = bar.child(actions);
    }
    let mut head = col().key("git-pull-strip").w_full().child(title).child(bar);
    // The merge methods drop down under the bar, on either tab.
    if pulls.merge_menu {
        if let Some(summary) = known {
            let mut methods = row().w_full().px(16.0 * s).py(8.0 * s).gap(8.0 * s).center_y().border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border)).fill(rgb(panel_bg(p)))
                .child(text("Merge with").font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)));
            for (name, method) in [("Squash and merge", PullRequestMergeMethod::Squash), ("Merge commit", PullRequestMergeMethod::Merge), ("Rebase and merge", PullRequestMergeMethod::Rebase)] {
                let head_sha = summary.head_sha.clone();
                methods = methods.child(action(app, &format!("git-pull-merge:{name}"), name, false, !pulls.busy()).on_press(move |a: &mut App| {
                    a.git_pull_act("Merging\u{2026}", PullRequestRequest::Merge { number, expected_head: head_sha.clone(), method });
                }));
            }
            head = head.child(methods);
        }
    }
    head
}

/// The open pull request's Conversation tab, in the diff tab.
pub(super) fn detail(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let pulls = &app.git_panel.pulls;
    let number = pulls.open.unwrap_or_default();
    let mut page = col().w_full().max_w(880.0 * s).padding(24.0 * s).gap(20.0 * s);
    let shell = |page: El| col().key("git-pull-detail").w_full().h_full().fill(rgb(p.bg)).child(strip(app, false))
        .child(col().w_full().grow(1.0).h(0.0).scroll(ScrollSpec { stick_bottom: false }).items_center().child(page));
    let Some(detail) = pulls.detail.as_ref().filter(|d| d.summary.number == number) else {
        return shell(match &pulls.detail_error {
            Some(error) => page.child(text(error.clone()).key("git-pull-error").font(app.ty.sans(13.0, 400.0)).color(rgb(p.red)).wrap().w_full()),
            None => page.child(skeleton_page(app)),
        });
    };
    let summary = &detail.summary;
    let open = summary.state.eq_ignore_ascii_case("open");
    let busy = pulls.busy();
    let mut top = col().w_full().gap(10.0 * s)
        .child(row().w_full().gap(10.0 * s).center_y().wrap()
            .child(text(format!("{} wants to merge into {}", summary.author, summary.base_branch)).font(app.ty.sans(12.5, 400.0)).color(rgb(p.dim)))
            .child(el().grow(1.0))
            .child(action(app, "git-pull-diff", &format!("Review {} changed files", detail.files.len()), true, true).on_press(|a: &mut App| a.git_pull_diff())));
    if let Some(working) = pulls.working { top = top.child(text(working).font(app.ty.sans(12.0, 400.0)).color(rgb(p.dim))); }
    if let Some(error) = &pulls.detail_error { top = top.child(text(error.clone()).key("git-pull-error").font(app.ty.sans(12.0, 400.0)).color(rgb(p.red)).wrap().w_full()); }
    if let Some(notice) = &pulls.notice { top = top.child(text(notice.clone()).key("git-pull-notice").font(app.ty.sans(12.0, 400.0)).color(rgb(p.green))); }
    page = page.child(top);
    if !detail.body.trim().is_empty() {
        page = page.child(text(detail.body.trim().to_owned()).font(app.ty.sans(13.5, 400.0).lh(20.0 * s)).color(rgb(p.dim)).wrap().w_full());
    }
    if !detail.checks.is_empty() {
        let mut block = section(app, "Checks", Some(detail.checks.len()));
        let mut rows = col().w_full().radius(10.0 * s).border(s, rgb(p.border)).fill(rgb(panel_bg(p)));
        for check in &detail.checks {
            let outcome = if check.conclusion.is_empty() { check.status.to_lowercase() } else { check.conclusion.to_lowercase() };
            let color = match outcome.as_str() { "success" => p.green, "failure" | "timed_out" | "cancelled" | "action_required" => p.red, "skipped" | "neutral" => p.muted, _ => p.yellow };
            rows = rows.child(row().w_full().h(30.0 * s).px(12.0 * s).gap(10.0 * s).center_y()
                .child(el().w(7.0 * s).h(7.0 * s).radius(3.5 * s).fill(rgb(color)))
                .child(text(check.name.clone()).font(app.ty.sans(12.5, 450.0)).color(rgb(p.fg)).truncate().grow(1.0).shrink())
                .child(text(outcome.replace('_', " ")).font(app.ty.sans(11.5, 400.0)).color(rgb(p.muted))));
        }
        block = block.child(rows);
        page = page.child(block);
    }
    if !detail.threads.is_empty() {
        let mut block = section(app, "Review threads", Some(detail.threads.len()));
        for thread in &detail.threads {
            let (id, resolved) = (thread.id.clone(), thread.resolved);
            let place = format!("{}{}", thread.path, thread.line.map(|l| format!(":{l}")).unwrap_or_default());
            let mut card = col().w_full().padding(12.0 * s).gap(8.0 * s).radius(10.0 * s).border(s, rgb(p.border)).fill(rgb(panel_bg(p)))
                .child(row().w_full().gap(8.0 * s).center_y()
                    .child(text(place).font(app.ty.sized(11.5)).color(rgb(p.fg)).truncate().grow(1.0).shrink())
                    .child_if(thread.outdated, || pill(app, "outdated", p.muted))
                    .child_if(resolved, || pill(app, "resolved", p.green))
                    .child(action(app, &format!("git-thread:{}", thread.id), if resolved { "Reopen" } else { "Resolve" }, false, !busy).on_press(move |a: &mut App| {
                        a.git_pull_act("Updating thread\u{2026}", PullRequestRequest::ResolveThread { number, thread_id: id.clone(), resolved: !resolved });
                    })));
            for c in &thread.comments {
                card = card.child(col().w_full().gap(3.0 * s)
                    .child(row().gap(8.0 * s).child(text(c.author.clone()).font(app.ty.sans(12.0, 600.0)).color(rgb(p.fg))).child(text(day(&c.created_at).to_owned()).font(app.ty.sans(11.0, 400.0)).color(rgb(p.muted))))
                    .child(text(c.body.trim().to_owned()).font(app.ty.sans(12.5, 400.0).lh(18.0 * s)).color(rgb(p.dim)).wrap().w_full()));
            }
            block = block.child(card);
        }
        page = page.child(block);
    }
    if !detail.conversation.is_empty() {
        let mut block = section(app, "Conversation", Some(detail.conversation.len()));
        for c in &detail.conversation { block = block.child(comment(app, &c.author, &c.created_at, &c.body)); }
        page = page.child(block);
    }
    if open {
        let review = section(app, "Review", (!pulls.drafts.is_empty()).then_some(pulls.drafts.len()))
            .child(rune::text_input(&pulls.body, app.ty.sans(13.0, 400.0)).key("git-pull-body").w_full().h(84.0 * s).padding(10.0 * s)
                .radius(10.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Leave a comment with your review", rgb(p.muted))
                .on_edit(|a: &mut App| &mut a.git_panel.pulls.body)
                .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| { if e.key == Key::Escape { a.git_panel.pulls.body.focused = false; true } else { false } }))
            .child(row().gap(8.0 * s).wrap()
                .child(action(app, "git-pull-comment", "Comment", false, !busy).on_press(|a: &mut App| a.git_pull_review(PullRequestReviewEvent::Comment)))
                .child(action(app, "git-pull-request-changes", "Request changes", false, !busy).on_press(|a: &mut App| a.git_pull_review(PullRequestReviewEvent::RequestChanges)))
                .child(action(app, "git-pull-approve", "Approve", true, !busy).on_press(|a: &mut App| a.git_pull_review(PullRequestReviewEvent::Approve))));
        page = page.child(review);
    }
    shell(page)
}

/// Fixture data for the headless scenes.
pub(super) fn fixture(app: &mut App, open: bool) {
    let summary = |number: u64, title: &str, author: &str, state: &str, checks: &str| PullRequestSummary {
        number, title: title.into(), url: format!("https://github.com/neoism/neoism/pull/{number}"), author: author.into(), state: state.into(), draft: false,
        base_branch: "main".into(), head_branch: "rune-frontend".into(), base_sha: "a".repeat(40), head_sha: "b".repeat(40), review_decision: String::new(), checks_state: checks.into(),
    };
    let item = |summary: PullRequestSummary, added: u64, deleted: u64| PullRequestInboxItem {
        summary, repository: "neoism/neoism".into(), updated_at: "2026-11-21T18:00:00Z".into(), additions: added, deletions: deleted,
        labels: vec![], reviewers: vec![], authored: true, review_requested: false, assigned: false,
    };
    let pulls = &mut app.git_panel.pulls;
    pulls.items = vec![
        item(summary(128, "Add native Git review", "parker", "OPEN", "SUCCESS"), 143, 5),
        item(summary(127, "Focused agent view folds a turn to its work row", "lin", "OPEN", "PENDING"), 356, 138),
        item(PullRequestSummary { draft: true, ..summary(124, "Sketch: issues in the Git panel", "ada", "OPEN", "") }, 80, 2),
        item(summary(121, "Fix the glass pill's slot for Servers", "parker", "OPEN", "FAILURE"), 6, 4),
    ];
    (pulls.total, pulls.loaded) = (4, true);
    app.git_panel.tab = Tab::Pulls;
    if !open { return; }
    let comment = |author: &str, body: &str| PullRequestComment { id: author.into(), author: author.into(), body: body.into(), created_at: "2026-11-21T17:30:00Z".into(), url: String::new() };
    app.open_tab(TabKind::GitDiff);
    app.focus = Focus::Content;
    app.git_panel.selected = Some(String::new());
    let pulls = &mut app.git_panel.pulls;
    pulls.open = Some(128);
    pulls.viewing = Some(128);
    pulls.detail = Some(PullRequestDetail {
        summary: summary(128, "Add native Git review", "parker", "OPEN", "SUCCESS"),
        body: "The Git panel names the next step for the branch, and every changed file is in one diff.\n\nTested against a real repository in the daemon's test suite.".into(),
        files: app.git_panel.files.clone(), diffs: vec![GitFileDiff { path: "src/git.rs".into(), patch: "@@ -40,3 +40,4 @@ pub fn view() {\n     let s = app.ty.scale();\n-    let width = 300.0;\n+    let width = WIDTH;\n+    let pad = 6.0 * s;\n     col().w(width * s)\n".into() }],
        conversation: vec![comment("lin", "The side-by-side layout reads well. One thought on the header below."), comment("parker", "Good call, fixed in the last push.")],
        threads: vec![PullRequestThread { id: "t1".into(), path: "src/git.rs".into(), side: PullRequestSide::Right, line: Some(42), start_line: None, outdated: false, resolved: false,
            comments: vec![comment("lin", "Should this width follow the panel's own constant?")] }],
        checks: vec![
            PullRequestCheck { name: "linux bundle".into(), status: "COMPLETED".into(), conclusion: "SUCCESS".into(), url: String::new() },
            PullRequestCheck { name: "macos bundle".into(), status: "COMPLETED".into(), conclusion: "SUCCESS".into(), url: String::new() },
            PullRequestCheck { name: "windows bundle".into(), status: "IN_PROGRESS".into(), conclusion: String::new(), url: String::new() },
        ],
        truncated: false,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pending(id: u64, app: &App) -> Option<Pending> { Some(Pending { id, cwd: app.cwd.clone(), kind: Operation::Remote }) }

    #[test]
    fn a_stale_page_is_dropped_and_an_error_is_shown() {
        let mut app = App::new();
        app.git_panel.pulls.generation = 3;
        app.git_panel.pulls.listing = pending(1, &app);
        let page = |generation| PullRequestPage { repository: "o/r".into(), viewer: "me".into(), generation, items: vec![], next_cursor: None, total_count: 9 };
        assert!(app.git_pulls_reply(&GitServerMessage::PullRequest { reply: PullRequestReply::Listed { page: page(2) } }, 1));
        assert_eq!(app.git_panel.pulls.total, 0, "an answer to an older search is ignored");
        app.git_panel.pulls.listing = pending(2, &app);
        assert!(app.git_pulls_reply(&GitServerMessage::PullRequest { reply: PullRequestReply::Listed { page: page(3) } }, 2));
        assert_eq!(app.git_panel.pulls.total, 9);
        app.git_panel.pulls.listing = pending(4, &app);
        assert!(app.git_pulls_reply(&GitServerMessage::PullRequest { reply: PullRequestReply::Error { kind: PullRequestErrorKind::Authentication, message: "Sign in to GitHub".into() } }, 4));
        assert_eq!(app.git_panel.pulls.error.as_deref(), Some("Sign in to GitHub"));
        assert!(!app.git_pulls_reply(&GitServerMessage::Error { message: "other".into() }, 99), "another request's reply is not taken");
    }

    #[test]
    fn a_review_needs_words_unless_it_approves() {
        let mut app = App::new();
        super::fixture(&mut app, true);
        app.git_pull_review(PullRequestReviewEvent::RequestChanges);
        assert!(app.git_panel.pulls.detail_error.as_deref().is_some_and(|e| e.contains("Write something")));
        assert!(!app.git_panel.pulls.busy());
    }

    #[test]
    fn viewing_the_changes_hands_the_files_to_the_diff() {
        let mut app = App::new();
        crate::git::fixture(&mut app, false);
        super::fixture(&mut app, true);
        app.git_pull_diff();
        assert_eq!(app.git_panel.review.scope, Scope::Pull(128));
        assert_eq!(app.git_panel.review.files.len(), 3);
        assert!(app.git_panel.pulls.open.is_none() && app.git_panel.pulls.viewing == Some(128), "the files tab of the same pull request");
        // The posted thread on src/git.rs line 42 sits under that line.
        let notes = &app.git_panel.review.notes;
        assert_eq!((notes.len(), notes[0].author.as_str(), notes[0].pending), (1, "lin", false));
        // A comment written on the removed line is kept as a draft on the left side.
        let file = app.git_panel.review.files.iter().position(|f| f.path == "src/git.rs").unwrap();
        let removed = app.git_panel.review.files[file].lines.iter().position(|l| l.kind == review::Kind::Remove).unwrap();
        app.git_panel.review.target = Some((file, removed));
        app.git_panel.review.comment = TextEdit::new("Keep the old width behind a flag?");
        app.git_pull_draft();
        let draft = &app.git_panel.pulls.drafts[0];
        assert_eq!((draft.path.as_str(), draft.side, draft.line), ("src/git.rs", PullRequestSide::Left, 41));
        assert!(app.git_panel.review.notes.iter().any(|n| n.pending && n.author == "You"));
        assert!(app.git_panel.review.target.is_none());
    }
}
