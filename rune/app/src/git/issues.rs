//! Issues: the panel's list and, in the diff tab, one issue with its comments, a reply box
//! and a way to start an agent chat on it.

use super::pulls::{action, comment, day, section};
use super::*;
use neoism_protocol::issue::*;

/// How long a list may go unanswered before the panel says the daemon is too old.
const UNANSWERED: std::time::Duration = std::time::Duration::from_secs(8);

#[derive(Default)]
pub struct Issues {
    pub(super) filter: IssueFilter,
    pub(super) mine: bool,
    pub(super) query: TextEdit,
    pub(super) items: Vec<IssueSummary>,
    pub(super) loaded: bool,
    pub(super) error: Option<String>,
    listing: Option<Pending>,
    pub open: Option<u64>,
    pub(super) detail: Option<IssueDetail>,
    reading: Option<Pending>,
    acting: Option<Pending>,
    pub(super) notice: Option<String>,
    pub(super) detail_error: Option<String>,
    pub(super) body: TextEdit,
    /// Issues already read, so opening one again shows at once while it refreshes.
    cache: std::collections::HashMap<u64, IssueDetail>,
    /// When the list was asked for: a daemon older than issues never answers.
    asked: Option<std::time::Instant>,
}

impl Issues {
    pub(super) fn disconnected(&mut self) {
        (self.listing, self.reading) = (None, None);
        if self.acting.take().is_some() { self.detail_error = Some("Connection lost. Check the issue before commenting again.".into()); }
    }
}

fn state_of(state: &str, p: &crate::ui::palettes::Palette) -> (&'static str, u32) {
    if state.eq_ignore_ascii_case("open") { ("open", p.green) } else { ("closed", p.magenta) }
}

/// The first message of a chat about an issue.
pub(super) fn prompt(detail: &IssueDetail) -> String {
    let mut body: String = detail.body.trim().chars().take(4000).collect();
    if detail.body.trim().chars().count() > 4000 { body.push_str("\n\u{2026}"); }
    let mut text = format!("Work on issue #{}: {}\n{}\n", detail.summary.number, detail.summary.title, detail.summary.url);
    if !body.is_empty() { text.push_str(&format!("\n{body}\n")); }
    text
}

impl App {
    pub fn git_issues_load(&mut self) {
        let issues = &mut self.git_panel.issues;
        let request = IssueRequest::List { query: issues.query.text.trim().to_owned(), filter: issues.filter, mine: issues.mine };
        issues.error = None;
        self.git_panel.issues.listing = self.git_request(GitClientMessage::Issue { request }, Operation::Remote);
        self.git_panel.issues.asked = Some(std::time::Instant::now());
        if self.git_panel.issues.listing.is_none() { self.git_panel.issues.error = self.git_panel.error.take(); }
    }

    /// Give up on a list a daemon never answered, and say why (called each frame).
    pub fn git_issues_tick(&mut self, tx: &mut Tx) {
        let issues = &mut self.git_panel.issues;
        let (Some(_), Some(asked)) = (&issues.listing, issues.asked) else { return; };
        if asked.elapsed() < UNANSWERED { tx.wake_after(std::time::Duration::from_millis(500)); return; }
        (issues.listing, issues.asked, issues.loaded) = (None, None, true);
        issues.error = Some("No answer from the workspace daemon. Issues need the daemon from this version of Neoism; update and restart it.".into());
    }

    /// Open an issue in the diff tab.
    pub fn open_git_issue(&mut self, number: u64) {
        if let Some(id) = self.tabs.iter().find(|t| t.kind == TabKind::GitDiff).map(|t| t.id) { self.select_tab(id); } else { self.open_tab(TabKind::GitDiff); }
        self.focus = Focus::Content;
        self.git_panel.pulls.open = None;
        let keep = self.git_panel.issues.cache.get(&number).cloned();
        let issues = &mut self.git_panel.issues;
        (issues.open, issues.detail, issues.detail_error, issues.notice, issues.body) = (Some(number), keep, None, None, TextEdit::default());
        self.git_panel.selected = Some(String::new());
        self.git_issue_read();
    }

    pub fn git_issue_read(&mut self) {
        let Some(number) = self.git_panel.issues.open else { return; };
        self.git_panel.issues.reading = self.git_request(GitClientMessage::Issue { request: IssueRequest::Read { number } }, Operation::Remote);
    }

    pub(super) fn git_issue_comment(&mut self) {
        let issues = &mut self.git_panel.issues;
        let (Some(number), body) = (issues.open, issues.body.text.trim().to_owned()) else { return; };
        if issues.acting.is_some() { return; }
        if body.is_empty() { issues.detail_error = Some("Write a comment first.".into()); return; }
        (issues.detail_error, issues.notice) = (None, None);
        self.git_panel.issues.acting = self.git_request(GitClientMessage::Issue { request: IssueRequest::Comment { number, body } }, Operation::Remote);
    }

    /// Open a new agent chat with the issue as its first message, ready to send.
    pub(super) fn git_issue_chat(&mut self) {
        let Some(detail) = &self.git_panel.issues.detail else { return; };
        let text = prompt(detail);
        self.open_tab(TabKind::Agent { session: None });
        self.focus = Focus::Content;
        let active = self.active;
        self.tabs[active].draft.replace_text(&text);
    }

    /// A reply to one of this tab's requests. `false` when it is not one of them.
    pub(super) fn git_issues_reply(&mut self, message: &GitServerMessage, request_id: u64) -> bool {
        let cwd = self.cwd.clone();
        let hit = |p: &Option<Pending>| p.as_ref().is_some_and(|p| p.matches(request_id, &cwd));
        let issues = &mut self.git_panel.issues;
        let error = |message: &GitServerMessage| match message {
            GitServerMessage::Issue { reply: IssueReply::Error { message, .. } } | GitServerMessage::Error { message } => Some(message.clone()),
            _ => None,
        };
        if hit(&issues.listing) {
            issues.listing = None;
            issues.loaded = true;
            match message {
                GitServerMessage::Issue { reply: IssueReply::Listed { items } } => (issues.items, issues.error) = (items.clone(), None),
                other => if let Some(e) = error(other) { issues.error = Some(e); },
            }
            return true;
        }
        if hit(&issues.reading) {
            issues.reading = None;
            match message {
                GitServerMessage::Issue { reply: IssueReply::Detail { detail } } if issues.open == Some(detail.summary.number) => {
                    issues.cache.insert(detail.summary.number, detail.clone());
                    (issues.detail, issues.detail_error) = (Some(detail.clone()), None);
                }
                other => if let Some(e) = error(other) { issues.detail_error = Some(e); },
            }
            return true;
        }
        if hit(&issues.acting) {
            issues.acting = None;
            match message {
                GitServerMessage::Issue { reply: IssueReply::Commented { .. } } => { issues.notice = Some("Comment posted.".into()); issues.body = TextEdit::default(); }
                other => if let Some(e) = error(other) { issues.detail_error = Some(e); },
            }
            self.git_issue_read();
            return true;
        }
        false
    }
}

/// The Issues tab of the panel.
pub(super) fn list(app: &App, card: El) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let issues = &app.git_panel.issues;
    let mut filters = row().w_full().gap(2.0 * s).padding(3.0 * s).radius(9.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border));
    let chip = |key: String, name: &'static str, on: bool| el().key(key).w(0.0).grow(1.0).h(24.0 * s).center().radius(6.0 * s)
        .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT })
        .child(text(name).font(app.ty.sans(12.0, if on { 600.0 } else { 450.0 })).color(rgb(if on { p.fg } else { p.muted })));
    for (name, filter) in [("Open", IssueFilter::Open), ("Closed", IssueFilter::Closed), ("All", IssueFilter::All)] {
        filters = filters.child(chip(format!("git-issues-filter:{name}"), name, issues.filter == filter && !issues.mine)
            .on_press(move |a: &mut App| { a.git_panel.issues.filter = filter; a.git_panel.issues.mine = false; a.git_issues_load(); }));
    }
    filters = filters.child(chip("git-issues-filter:Mine".into(), "Mine", issues.mine)
        .on_press(|a: &mut App| { a.git_panel.issues.mine = true; a.git_panel.issues.filter = IssueFilter::Open; a.git_issues_load(); }));
    let search = rune::text_input(&issues.query, app.ty.sans(12.5, 400.0)).key("git-issues-query").w_full().h(30.0 * s).px(10.0 * s).py(7.0 * s).single_line()
        .radius(8.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Search", rgb(p.muted)).caret(rune::Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .on_edit(|a: &mut App| { a.focus = Focus::Details; &mut a.git_panel.issues.query })
        .on_input_key(|a: &mut App, e: &KeyEvent, tx: &mut Tx| {
            if e.key == Key::Enter { a.git_issues_load(); return true; }
            if e.key == Key::Escape { a.git_panel.issues.query.focused = false; return true; }
            crate::keys::shortcut(a, e, tx)
        });
    let mut list = col().key("git-issues").w_full().grow(1.0).shrink().h(0.0).scroll(ScrollSpec { stick_bottom: false }).pt(4.0 * s);
    if let Some(error) = &issues.error {
        list = list.child(text(error.clone()).font(app.ty.sans(12.0, 400.0)).color(rgb(p.red)).padding(14.0 * s).wrap());
    } else if !issues.loaded {
        list = list.child(skeleton(app, 5));
    } else if issues.items.is_empty() {
        list = list.child(text("No issues here.").font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)).padding(14.0 * s));
    }
    for item in &issues.items {
        let number = item.number;
        let (state, color) = state_of(&item.state, p);
        let on = issues.open == Some(number);
        let mut meta = row().w_full().gap(6.0 * s).center_y().child(pill(app, state, color));
        for label in item.labels.iter().take(2) {
            meta = meta.child(el().h(18.0 * s).px(7.0 * s).center().radius(9.0 * s).border(s, rgb(p.border))
                .child(text(label.clone()).font(app.ty.sans(10.5, 450.0)).color(rgb(p.dim))));
        }
        meta = meta.child(el().grow(1.0));
        if item.comments > 0 { meta = meta.child(text(format!("{} {}", item.comments, if item.comments == 1 { "comment" } else { "comments" })).font(app.ty.sans(10.5, 400.0)).color(rgb(p.muted))); }
        list = list.child(col().key(format!("git-issue:{number}")).w_full().px(14.0 * s).py(9.0 * s).gap(4.0 * s)
            .fill(if on { rgb(p.hover) } else { rune::Color::TRANSPARENT }).hover(rgb(p.hover))
            .child(text(item.title.clone()).font(app.ty.sans(13.0, 600.0)).color(rgb(p.fg)).truncate().w_full())
            .child(text(format!("#{number} \u{b7} {} \u{b7} {}", item.author, day(&item.updated_at))).font(app.ty.sans(11.5, 400.0)).color(rgb(p.muted)).truncate().w_full())
            .child(meta)
            .on_press(move |a: &mut App| a.open_git_issue(number)));
    }
    card.child(col().w_full().padding(12.0 * s).gap(8.0 * s).child(filters).child(search))
        .child(list.border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border)))
}

/// The open issue, in the diff tab.
pub(super) fn detail(app: &App) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let issues = &app.git_panel.issues;
    let number = issues.open.unwrap_or_default();
    let Some(detail) = issues.detail.as_ref().filter(|d| d.summary.number == number) else {
        // What the list already knows, over placeholders for the rest.
        let mut page = col().w_full().max_w(880.0 * s).padding(24.0 * s).gap(20.0 * s);
        if let Some(error) = &issues.detail_error {
            page = page.child(text(error.clone()).key("git-issue-error").font(app.ty.sans(13.0, 400.0)).color(rgb(p.red)).wrap().w_full());
        } else {
            if let Some(item) = issues.items.iter().find(|i| i.number == number) {
                let (state, color) = state_of(&item.state, p);
                page = page.child(col().w_full().gap(10.0 * s)
                    .child(text(item.title.clone()).font(app.ty.sans(21.0, 650.0)).color(rgb(p.fg)).wrap().w_full())
                    .child(row().gap(8.0 * s).center_y().child(pill(app, state, color)).child(text(format!("#{number}")).font(app.ty.sized(12.0)).color(rgb(p.muted)))
                        .child(text(format!("opened by {}", item.author)).font(app.ty.sans(12.5, 400.0)).color(rgb(p.dim)))));
            }
            page = page.child(skeleton_page(app));
        }
        return col().key("git-issue-detail").w_full().h_full().fill(rgb(p.bg)).scroll(ScrollSpec { stick_bottom: false }).items_center().child(page);
    };
    let summary = &detail.summary;
    let (state, color) = state_of(&summary.state, p);
    let url = summary.url.clone();
    let busy = issues.acting.is_some();
    let mut meta = row().w_full().gap(8.0 * s).center_y().wrap()
        .child(pill(app, state, color))
        .child(text(format!("#{number}")).font(app.ty.sized(12.0)).color(rgb(p.muted)))
        .child(text(format!("opened by {} on {}", summary.author, day(&detail.created_at))).font(app.ty.sans(12.5, 400.0)).color(rgb(p.dim)));
    for label in &summary.labels {
        meta = meta.child(el().h(20.0 * s).px(8.0 * s).center().radius(10.0 * s).border(s, rgb(p.border)).child(text(label.clone()).font(app.ty.sans(11.0, 450.0)).color(rgb(p.dim))));
    }
    if !summary.assignees.is_empty() {
        meta = meta.child(text(format!("assigned to {}", summary.assignees.join(", "))).font(app.ty.sans(12.0, 400.0)).color(rgb(p.muted)));
    }
    let mut top = col().w_full().gap(10.0 * s)
        .child(text(summary.title.clone()).font(app.ty.sans(21.0, 650.0)).color(rgb(p.fg)).wrap().w_full())
        .child(meta)
        .child(row().gap(8.0 * s).center_y().wrap()
            .child(action(app, "git-issue-chat", "Start a chat on this issue", true, true).on_press(|a: &mut App| a.git_issue_chat()))
            .child(action(app, "git-issue-browser", "Open on GitHub", false, true).on_press_tx(move |_: &mut App, tx: &mut Tx| { let _ = tx.open_url(&url); }))
            .child(action(app, "git-issue-refresh", "Refresh", false, true).on_press(|a: &mut App| a.git_issue_read())));
    if let Some(error) = &issues.detail_error { top = top.child(text(error.clone()).key("git-issue-error").font(app.ty.sans(12.0, 400.0)).color(rgb(p.red)).wrap().w_full()); }
    if let Some(notice) = &issues.notice { top = top.child(text(notice.clone()).font(app.ty.sans(12.0, 400.0)).color(rgb(p.green))); }
    let mut page = col().w_full().max_w(880.0 * s).padding(24.0 * s).gap(20.0 * s).child(top);
    if !detail.body.trim().is_empty() {
        page = page.child(col().w_full().min_w(0.0).child(prose(app, format!("git-issue-body:{number}"), &detail.body)));
    }
    if !detail.comments.is_empty() {
        let mut block = section(app, "Comments", Some(detail.comments.len()));
        for c in &detail.comments { block = block.child(comment(app, &c.author, &c.created_at, &c.body)); }
        page = page.child(block);
    }
    page = page.child(section(app, "Reply", None)
        .child(rune::text_input(&issues.body, app.ty.sans(13.0, 400.0)).key("git-issue-body").w_full().h(84.0 * s).padding(10.0 * s)
            .radius(10.0 * s).fill(rgb(p.surface)).border(s, rgb(p.border)).color(rgb(p.fg)).placeholder("Comment", rgb(p.muted)).caret(rune::Color::TRANSPARENT, crate::ui::cursor::selection(app))
            .on_edit(|a: &mut App| &mut a.git_panel.issues.body)
            .on_input_key(|a: &mut App, e: &KeyEvent, _: &mut Tx| { if e.key == Key::Escape { a.git_panel.issues.body.focused = false; true } else { false } }))
        .child(row().child(action(app, "git-issue-comment", if busy { "Posting\u{2026}" } else { "Comment" }, true, !busy).on_press(|a: &mut App| a.git_issue_comment()))));
    col().key("git-issue-detail").w_full().h_full().fill(rgb(p.bg)).scroll(ScrollSpec { stick_bottom: false }).items_center().child(page)
}

/// Fixture data for the headless scenes.
pub(super) fn fixture(app: &mut App, open: bool) {
    let issue = |number: u64, title: &str, author: &str, labels: &[&str], comments: u32| IssueSummary {
        number, title: title.into(), url: format!("https://github.com/neoism/neoism/issues/{number}"), author: author.into(), state: "OPEN".into(),
        labels: labels.iter().map(|l| l.to_string()).collect(), assignees: vec![], comments, updated_at: "2026-11-21T16:00:00Z".into(),
    };
    let issues = &mut app.git_panel.issues;
    issues.items = vec![
        issue(96, "Fork from an answer does nothing on an old daemon", "parker", &["bug", "agent"], 3),
        issue(94, "Text looks soft when the window is zoomed", "lin", &["bug"], 1),
        issue(91, "Collapse the MCP config behind a toggle in Setup", "ada", &["design"], 0),
        issue(88, "Daemon image job loses its runner after an hour", "parker", &["ci"], 2),
    ];
    issues.loaded = true;
    app.git_panel.tab = Tab::Issues;
    if !open { return; }
    app.open_tab(TabKind::GitDiff);
    app.focus = Focus::Content;
    app.git_panel.selected = Some(String::new());
    let issues = &mut app.git_panel.issues;
    issues.open = Some(96);
    issues.detail = Some(IssueDetail {
        summary: IssueSummary { assignees: vec!["parker".into()], ..issue(96, "Fork from an answer does nothing on an old daemon", "parker", &["bug", "agent"], 2) },
        body: "Pressing Fork under an answer does nothing when the running daemon predates the fork message.\n\nExpected: a note that says the daemon needs updating.".into(),
        created_at: "2026-11-20T09:00:00Z".into(),
        comments: vec![
            IssueComment { author: "lin".into(), body: "Reproduced on 0.7.161 with a 0.7.158 daemon.".into(), created_at: "2026-11-20T11:00:00Z".into(), url: String::new() },
            IssueComment { author: "parker".into(), body: "The app can tell from the hello which messages the daemon knows.".into(), created_at: "2026-11-21T08:00:00Z".into(), url: String::new() },
        ],
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_chat_starts_with_the_issue_as_its_draft() {
        let mut app = App::new();
        super::fixture(&mut app, true);
        let tabs = app.tabs.len();
        app.git_issue_chat();
        assert_eq!(app.tabs.len(), tabs + 1);
        assert!(matches!(app.active_tab().kind, TabKind::Agent { session: None }));
        let draft = app.active_tab().draft.text.clone();
        assert!(draft.starts_with("Work on issue #96: Fork from an answer does nothing on an old daemon\nhttps://github.com/neoism/neoism/issues/96\n"));
        assert!(draft.contains("Expected: a note"));
    }

    #[test]
    fn replies_are_matched_to_their_request_and_an_empty_comment_is_not_sent() {
        let mut app = App::new();
        super::fixture(&mut app, true);
        app.git_issue_comment();
        assert_eq!(app.git_panel.issues.detail_error.as_deref(), Some("Write a comment first."));
        app.git_panel.issues.listing = Some(Pending { id: 5, cwd: app.cwd.clone(), kind: Operation::Remote });
        assert!(!app.git_issues_reply(&GitServerMessage::Issue { reply: IssueReply::Listed { items: vec![] } }, 6));
        assert_eq!(app.git_panel.issues.items.len(), 4);
        assert!(app.git_issues_reply(&GitServerMessage::Issue { reply: IssueReply::Listed { items: vec![] } }, 5));
        assert!(app.git_panel.issues.items.is_empty());
    }
}
