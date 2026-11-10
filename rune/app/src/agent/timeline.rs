//! One agent tab: the wordmark home before the first prompt, then the transcript with markdown,
//! code frames, tool cards and the pinned composer (`reference/05..08`).

use super::composer::composer;
use super::prose;
use super::tools;
use crate::app::App;
use crate::ui::theme::rgb;
use crate::wordmark;
use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind};
use rune::{Color, Cx, El, ScrollSpec, Tx, WheelEvent, col, el, row, text};
use rune_kit::markdown::{CodeFrame, Markdown, MarkdownStyle};
use rune_kit::syntax::Syntax;

pub const COLUMN_PAD_X: f32 = 102.0;

/// Rune reports a wheel notch as 40 points; the original moves three text rows per notch.

/// One agent tab: the wordmark home before the first prompt, then the transcript.
pub fn pane(app: &App, session: Option<&str>, _cx: &mut Cx<App>) -> El {
    let p = app.pal;
    let tab_id = app.active_tab().id;
    let available = app.rects.get(&format!("agent-area:{tab_id}")).map_or(app.window[0] - 55.5, |r| r[2]);
    let Some(session_id) = session else {
        return col().key("agent-pane").w_full().h_full().fill(app.surface(p.bg)).items_center().justify_center()
            .on_layout(move |a: &mut App, r| { a.rects.insert(format!("agent-area:{tab_id}"), r); })
            .child(wordmark::view(app, &wordmark::host_key("agent", tab_id), 470.5_f32.min((available - 32.0).max(16.0))))
            .child(el().h(47.0))
            .child(composer(app, 845.0_f32.min((available - 24.0).max(16.0))).key("agent-composer-shell"));
    };
    let session = app.sessions.get(session_id);
    let messages = session.map(|s| s.messages.as_slice()).unwrap_or(&[]);
    let streaming = session.is_some_and(|s| s.streaming);
    let live_from = session.and_then(|s| s.live_from).unwrap_or(usize::MAX);
    let pad = if available >= 700.0 { COLUMN_PAD_X } else { 24.0 };
    let width_id = session_id.to_owned();
    // The transcript's rows: only those in the mounted window (the viewport and up to two
    // viewports either side) are built, so a scroll frame or a streamed delta costs the rows on
    // screen, not the whole history (`virtual_rows`; heights are measured as rows mount).
    let items = std::rc::Rc::new(row_items(messages, live_from, streaming && app.activity.is_some()));
    let (key_items, build_items, visible_items) = (items.clone(), items.clone(), items.clone());
    let (key_id, build_id, visible_id) = (session_id.to_owned(), session_id.to_owned(), session_id.to_owned());
    let rows = col().key(format!("timeline:{session_id}")).w_full().px(pad).pt(16.0).pb(200.0).gap(0.0)
        .growth(rune::GrowthSpec::default())
        .virtual_rows(items.len(), 120.0,
            move |a: &App, i| row_key(a, &key_id, key_items[i]).into(),
            move |a: &App, i| build_row(a, &build_id, build_items[i], tab_id))
        .on_visible_rows(move |a: &mut App, range: std::ops::Range<usize>| a.timeline_rows_visible(&visible_id, &visible_items, range))
        .on_layout(move |a: &mut App, [_, _, w, _]: [f32; 4]| { if let Some(session) = a.sessions.get_mut(&width_id) { session.width = (w - 2.0 * pad).max(0.0); } })
        // Press, hold and drag selects across the transcript; letting go copies it.
        .selection_group(true)
        // The composer keeps the keyboard through a press here: refocusing it afterwards
        // would clear the selection being made.
        .preserve_focus_on_press(true)
        .on_selection_change(|a: &mut App, change: &rune::SelectionChange, tx: &mut Tx| a.timeline_selection_changed(change, tx));
    let scroll_key = format!("conversation:{session_id}");
    let mirror = session.map(|s| s.scroll.clone()).unwrap_or_default();
    let wheel_id = session_id.to_owned();
    let view_id = session_id.to_owned();
    // A notch moves the same distance as in a code file (three code lines), on the same spring.
    let line_h = crate::editor::view::line_height(app);
    let timeline = col().key(scroll_key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_layout(move |a: &mut App, r: [f32; 4]| {
            a.rects.insert(format!("timeline-view:{tab_id}"), r);
            if let Some(session) = a.sessions.get_mut(&view_id) { session.viewport_h = r[3]; }
        })
        .on_pointer_move_local(|a: &mut App, p: rune::PointerEvent, _: &mut Tx| {
            if a.selection.copy.dragging() { a.selection.pointer_y = Some(p.window_y); }
        })
        .on_wheel(move |s: &mut App, e: WheelEvent, tx: &mut Tx| {
            // Notches move three code lines; trackpads move with the original's reading gain
            // and coast. Both glide on Rune's clock (`scroll_to_with`), retargeting mid-glide so
            // consecutive deltas are one motion, and Rune keeps the reader's row in place as
            // older pages prepend and estimated rows measure.
            if std::env::var_os("NEOISM_RUNE_TRACE_WHEEL").is_some() { eprintln!("wheel timeline dy={} precise={} phase={:?}", e.dy, e.precise, e.phase); }
            let Some(session) = s.sessions.get_mut(&wheel_id) else { return false };
            let (offset, _) = session.scroll.get();
            if e.dy > 0.0 { session.scrolled_up_at = Some(std::time::Instant::now()); }
            // Heading up within a viewport of the top asks for the page above.
            let near_top = e.dy > 0.0 && offset < session.viewport_h;
            let mirror = session.scroll.clone();
            let taken = crate::ui::scroll::handle(&mut s.scrollers, &format!("conversation:{wheel_id}"), &e, 3.0 * line_h, std::rc::Rc::new(move || mirror.get()), tx);
            if near_top { s.request_older(&wheel_id); }
            taken
        })
        .child(rows);
    let mut pane = col().key("agent-pane").w_full().h_full().fill(app.surface(p.bg))
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("agent-area:{tab_id}"), r); });
    if let Some(header) = super::header::view(app, session_id) {
        pane = pane.child(col().w_full().px(pad).child(header));
    }
    // The question rail sits in the transcript's right padding, over the scroll view.
    let rail = match (app.rects.get(&format!("timeline-view:{tab_id}")), app.rects.get(&format!("agent-area:{tab_id}"))) {
        (Some(view), Some(area)) => {
            // The visible transcript: the scroll view runs on under the pinned composer.
            let mut visible = *view;
            if let Some(card) = app.rects.get(&format!("composer-card:{tab_id}")) {
                visible[3] = (card[1] - 6.0 - view[1]).clamp(0.0, view[3]);
            }
            super::ticks::view(app, session_id, visible, *area, pad)
        }
        _ => el(),
    };
    // Following the end, the activity line holds the bottom row just above the composer while
    // the streamed text scrolls up under it (`activity_geometry`).
    let pinned = match (app.activity.as_ref().filter(|_| app.activity_pinned), app.rects.get(&format!("composer-card:{tab_id}")), app.rects.get(&format!("agent-area:{tab_id}"))) {
        (Some(activity), Some(card), Some(area)) => {
            let top = card[1] - area[1] - app.activity_row_height() - 6.0;
            col().key("activity-pinned").pin(Some(pad), Some(top.max(0.0)), Some(pad), None).child(activity.view(app))
        }
        _ => el(),
    };
    pane.child(timeline)
        .child(pinned)
        .child(rail)
        .child(col().w_full().px((pad - 16.0).max(12.0)).pb(6.0).pin(Some(0.0), None, Some(0.0), Some(0.0))
            .child(composer(app, f32::INFINITY)))
}

/// One transcript row: a message, a folded run of tool calls starting at a message, or the
/// live turn's activity line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowItem {
    Message(usize),
    Group(usize),
    Activity,
}

/// The transcript's rows in order: visible messages, runs of reads and settled edits folded
/// into one row (`tools::group_at`), then the activity line while a turn streams.
pub fn row_items(messages: &[HistoryMessage], live_from: usize, activity: bool) -> Vec<RowItem> {
    let mut items = Vec::with_capacity(messages.len() + 1);
    let mut index = 0;
    while index < messages.len() {
        if !tools::visible(&messages[index], index, live_from) { index += 1; continue; }
        if let Some((end, _)) = tools::group_at(messages, index) {
            items.push(RowItem::Group(index));
            index = end;
            continue;
        }
        items.push(RowItem::Message(index));
        index += 1;
    }
    if activity { items.push(RowItem::Activity); }
    items
}

fn row_key(app: &App, session_id: &str, item: RowItem) -> String {
    let messages = app.sessions.get(session_id).map(|s| s.messages.as_slice()).unwrap_or(&[]);
    match item {
        RowItem::Message(i) => messages.get(i).map_or_else(|| format!("row:{i}"), |m| format!("msg:{}", m.id)),
        RowItem::Group(i) => messages.get(i).map_or_else(|| format!("group:{i}"), |m| format!("group:{}", m.id)),
        RowItem::Activity => "activity".into(),
    }
}

fn build_row(app: &App, session_id: &str, item: RowItem, tab_id: u64) -> El {
    let Some(session) = app.sessions.get(session_id) else { return el() };
    let messages = session.messages.as_slice();
    match item {
        // Pinned above the composer, the line keeps its room here so text never runs under it.
        RowItem::Activity if app.activity_pinned => el().key("activity").w_full().h(app.activity_row_height()),
        RowItem::Activity => app.activity.as_ref().map_or_else(el, |activity| activity.view(app)),
        RowItem::Group(start) => match tools::group_at(messages, start) {
            Some((_, group)) => tools::row(app, &group),
            None => el(),
        },
        RowItem::Message(index) => {
            let Some(message) = messages.get(index) else { return el() };
            let last = index + 1 == messages.len();
            let mut row = message_row(app, message, last && session.streaming);
            if message.kind == HistoryMessageKind::User {
                // Questions report where their row sits in the transcript while it is mounted;
                // the rail reveals a row never mounted by its key instead.
                let (sid, mid) = (session_id.to_owned(), message.id.clone());
                // Bookkeeping only: it follows every frame of a scroll without committing.
                row = row.on_layout_tx(move |a: &mut App, r: [f32; 4], tx: &mut Tx| {
                    tx.live_only();
                    let Some(view) = a.rects.get(&format!("timeline-view:{tab_id}")).copied() else { return };
                    let Some(session) = a.sessions.get_mut(&sid) else { return };
                    let top = r[1] - view[1] + session.scroll.get().0;
                    session.question_tops.insert(mid.clone(), top.max(0.0));
                });
            }
            row
        }
    }
}

impl App {
    /// The rows in view changed: load the older page when the first row shows, and light the
    /// question rail's tick for the question in view. Returns whether the view must re-run.
    fn timeline_rows_visible(&mut self, session_id: &str, items: &[RowItem], range: std::ops::Range<usize>) -> bool {
        // Only while someone scrolls up toward it (`request_older` checks): parked at the top,
        // nothing loads by itself.
        if range.start == 0 { self.request_older(session_id); }
        let Some(session) = self.sessions.get_mut(session_id) else { return false };
        let (offset, max) = session.scroll.get();
        let rows: Vec<usize> = items.iter().enumerate().filter_map(|(row, item)| match item {
            RowItem::Message(i) if session.messages.get(*i).is_some_and(|m| m.kind == HistoryMessageKind::User) => Some(row),
            _ => None,
        }).collect();
        session.visible_questions = rows.iter().filter(|r| range.contains(r)).filter_map(|r| match items[*r] {
            RowItem::Message(i) => session.messages.get(i).map(|m| m.id.clone()),
            _ => None,
        }).collect();
        let active = super::ticks::active_index(&rows, range, max - offset <= 1.0);
        if session.rail_active == active { return false; }
        session.rail_active = active;
        true
    }
}

/// The transcript's selection highlight.
pub(super) fn selection_color(app: &App) -> Color { crate::ui::selection::highlight(rgb(app.pal.accent)) }

/// A selecting drag carried past the transcript's top or bottom edge scrolls it, up to 22 pt a
/// frame, faster the further out (reaching full speed 32 pt beyond the edge).
const EDGE_ZONE: f32 = 32.0;
const EDGE_STEP: f32 = 22.0;

/// The transcript selection as it is being made (`state/selection.rs`).
#[derive(Default)]
pub struct Selecting {
    pub copy: crate::ui::selection::CopyOnRelease,
    /// Where the pointer is during a selecting drag, in window points.
    pub pointer_y: Option<f32>,
}

impl App {
    /// A drag selecting in the transcript: remember it while it runs, and copy the selection
    /// when the press lets go (`end_selection`), as the original does.
    pub fn timeline_selection_changed(&mut self, change: &rune::SelectionChange, tx: &mut Tx) {
        if let Some(text) = self.selection.copy.change(change, tx) { self.copied_selection = Some(text); }
        if !self.selection.copy.dragging() { self.selection.pointer_y = None; }
    }

    /// Keep scrolling while a selecting drag rests near the transcript's top or bottom edge.
    /// Returns whether it moved (the frame clock keeps running then).
    pub fn timeline_edge_scroll(&mut self, tx: &mut Tx) -> bool {
        if !self.selection.copy.dragging() { return false; }
        let Some(y) = self.selection.pointer_y else { return false };
        let tab = self.active_tab().id;
        let crate::tabs::TabKind::Agent { session: Some(id) } = &self.active_tab().kind else { return false };
        let Some([_, top, _, h]) = self.rects.get(&format!("timeline-view:{tab}")).copied() else { return false };
        let Some(session) = self.sessions.get(id) else { return false };
        let (offset, max) = session.scroll.get();
        // The composer floats over the transcript's foot: the visible bottom is its top.
        let bottom = self.rects.get(&format!("composer-card:{tab}")).map_or(top + h, |c| (c[1] - 6.0).min(top + h));
        // Only a drag carried past the visible transcript scrolls it (as a browser does); a
        // press resting inside, however near an edge, never moves the page.
        let above = top - y;
        let below = y - bottom;
        let delta = if above > 0.0 { -(above / EDGE_ZONE).clamp(0.1, 1.0) * EDGE_STEP }
            else if below > 0.0 { (below / EDGE_ZONE).clamp(0.1, 1.0) * EDGE_STEP }
            else { return false };
        let next = (offset + delta).clamp(0.0, max);
        if (next - offset).abs() < 0.5 { return false; }
        tx.scroll_to_y(format!("conversation:{id}"), next);
        true
    }
}

fn timeline_width(app: &App) -> f32 {
    if let crate::tabs::TabKind::Agent { session: Some(id) } = &app.active_tab().kind {
        if let Some(session) = app.sessions.get(id).filter(|s| s.width > 0.0) { return session.width; }
    }
    830.0
}

fn markdown_style(app: &App) -> MarkdownStyle {
    let p = app.pal;
    MarkdownStyle::neutral(true)
        .text(rgb(p.fg))
        .selection(selection_color(app))
        // List markers and level 4+ headings take `muted`; the original draws bullets blue.
        .muted(rgb(p.blue))
        .link(rgb(p.blue))
        .inline_code_bg(Color::TRANSPARENT)
        // `code_frame` draws the card; the kit's own block is bare so numbers line up with rows.
        .code_block(Color::TRANSPARENT, Color::TRANSPARENT)
        .code_block_padding(0.0, 0.0)
        .body_font(app.ty.body())
        .heading_font(1, app.ty.heading(1))
        .heading_font(2, app.ty.heading(2))
        .heading_font(3, app.ty.heading(3))
        .code_block_font(app.ty.body())
        .inline_code_font(1.0, app.ty.weight)
        .inline_code_pad(0.0)
        .syntax(Syntax::Keyword, rgb(p.syn_keyword))
        .syntax(Syntax::String, rgb(p.syn_string))
        .syntax(Syntax::Number, rgb(p.syn_number))
        .syntax(Syntax::Comment, rgb(p.syn_comment))
        .syntax(Syntax::Type, rgb(p.syn_type))
        .syntax(Syntax::Function, rgb(p.syn_func))
        .syntax(Syntax::Property, rgb(p.syn_property))
        .syntax(Syntax::Punctuation, rgb(p.syn_special))
        .rule(rgb(p.border))
        .list(18.0, 6.0)
}

/// Code blocks: a header with the language and Copy, then numbered lines (`reference/06`).
fn code_frame(app: &App) -> impl Fn(El, &CodeFrame<'_>) -> El + 'static {
    let p = *app.pal;
    let ty = app.ty;
    move |code: El, frame: &CodeFrame<'_>| {
        let lines = frame.source.trim_end_matches('\n').lines().count().max(1);
        let mut numbers = col().pr(18.0).items_end();
        for n in 1..=lines {
            numbers = numbers.child(text(n.to_string()).font(ty.body()).color(rgb(p.muted)));
        }
        let source = frame.source.to_owned();
        col().w_full().radius(14.0).fill(rgb(p.surface).alpha(0.35)).border(1.0, rgb(p.border)).clip_rounded(14.0)
            .child(row().w_full().h(38.0).px(22.0).center_y().justify_between().fill(rgb(p.surface).alpha(0.55))
                .child(text(frame.language.unwrap_or("text").to_owned()).font(ty.small()).color(rgb(p.yellow)))
                .child(text("Copy").font(ty.small()).color(rgb(p.muted))
                    .on_press_tx(move |_: &mut App, tx: &mut Tx| tx.copy(source.clone()))))
            .child(row().w_full().px(22.0).py(12.0).child(numbers).child(code.grow(1.0)))
    }
}

fn message_row(app: &App, message: &HistoryMessage, streaming: bool) -> El {
    let p = app.pal;
    let key = format!("msg:{}", message.id);
    match message.kind {
        HistoryMessageKind::User => super::user::view(app, &message.text, message.author.as_deref(), timeline_width(app))
            .key(key).pt(26.0),
        HistoryMessageKind::Reasoning => col().key(key).w_full().pt(24.0).gap(10.0)
            .child(text(if message.title.is_empty() { "Thinking".to_owned() } else { message.title.clone() })
                .font(app.ty.body().italic()).color(rgb(p.yellow).alpha(0.85)).selectable(true).selection_color(selection_color(app)))
            .child(text(first_line(&message.text)).font(app.ty.body().italic()).color(rgb(p.dim)).selectable(true).selection_color(selection_color(app))),
        HistoryMessageKind::Tool | HistoryMessageKind::Subtask => tools::row(app, message),
        // Bound notices and compaction summaries to six source lines before layout: `max_lines`
        // alone leaves a many-paragraph text its full height, which breaks narrow transcripts.
        HistoryMessageKind::System | HistoryMessageKind::Compaction => text(message.text.lines().take(6).collect::<Vec<_>>().join("\n")).key(key)
            .font(app.ty.small()).color(rgb(p.muted)).pt(16.0).w_full().wrap().max_lines(6).truncate()
            .selectable(true).selection_color(selection_color(app)),
        HistoryMessageKind::Assistant => {
            let revision = match &app.active_tab().kind {
                crate::tabs::TabKind::Agent { session: Some(id) } => app.sessions.get(id).and_then(|s| s.message_versions.get(&message.id)).copied().unwrap_or(0),
                _ => 0,
            };
            let version = (revision, app.pal as *const _ as usize, app.ty.size.to_bits(), app.ty.weight.to_bits(), timeline_width(app).to_bits(), streaming);
            rune::memo(format!("assistant:{}:{}", app.active_tab().id, message.id), version, || {
            // Pipe tables render as the original's data grid; everything else as markdown.
            let style = markdown_style(app);
            // Padding, not margin: a virtual row's top margin is dropped from its measured height.
            let mut body = col().key(key).w_full().pt(24.0).gap(14.0);
            for segment in super::table::segments(&message.text) {
                body = body.child(match segment {
                    super::table::Segment::Table(rows) => super::table::view(app, &rows, timeline_width(app), &style),
                    super::table::Segment::Text(text) => Markdown::new(text.to_owned())
                        .highlights(prose::highlights(text, p))
                        .streaming(streaming)
                        .reveal(if streaming { 0.18 } else { 0.0 })
                        .code_frame(code_frame(app))
                        .selectable(true)
                        // Links show a hand and open on click: web links in the browser, file
                        // links (`path`, `path:12`, `path#L12`, `file://…`) in the editor.
                        .on_link_tx(|a: &mut App, url: &str, _: &mut Tx| a.open_link(url))
                        .render(&style)
                        // One selection spans the whole transcript, not just this block.
                        .selection_group(false),
                });
            }
            body.child_meta(app, message)
            })
        }
    }
}

trait Meta {
    fn child_meta(self, app: &App, message: &HistoryMessage) -> Self;
}

impl Meta for El {
    /// `Build · model · time · tok/s` under a finished answer (`reference/08`).
    fn child_meta(self, app: &App, message: &HistoryMessage) -> El {
        // The daemon sends the finished footer in `status`: `Build · GPT-6.1 Sol · 1m 1s · 33.1 tok/s`.
        let Some((agent, rest)) = message.status.split_once(" · ") else { return self };
        let p = app.pal;
        self.child(row().gap(0.0).mt(4.0)
            .child(text(agent.to_owned()).font(app.ty.body()).color(rgb(p.blue)))
            .child(text(format!(" · {rest}")).font(app.ty.body()).color(rgb(p.muted))))
    }
}

fn first_line(text: &str) -> String {
    text.lines().map(str::trim).find(|l| !l.is_empty()).unwrap_or("").trim_matches('*').to_owned()
}


impl App {
    /// Follow a transcript link: a web or mail link opens in the default browser, anything else
    /// is a file in the workspace (absolute, or relative to it), with an optional line.
    pub fn open_link(&mut self, url: &str) {
        let url = url.trim();
        if url.starts_with("http://") || url.starts_with("https://") || url.starts_with("mailto:") {
            super::slash::actions::open_in_browser(url);
            return;
        }
        let Some((path, line)) = file_link(url) else { return };
        let path = if std::path::Path::new(&path).is_absolute() { path } else { format!("{}/{}", self.cwd.trim_end_matches('/'), path.trim_start_matches("./")) };
        self.open_file(&path, line);
    }
}

/// `path`, `path:12`, `path:12:3`, `path#L12` or `file:///path…` → (path, line).
fn file_link(url: &str) -> Option<(String, Option<u32>)> {
    let url = url.strip_prefix("file://").unwrap_or(url);
    if url.is_empty() || url.contains("://") || url.starts_with('#') { return None; }
    let url = url.replace("%20", " ");
    if let Some((path, anchor)) = url.split_once("#L") {
        let line = anchor.split(|c: char| !c.is_ascii_digit()).next().and_then(|n| n.parse().ok());
        return Some((path.to_owned(), line));
    }
    let mut parts = url.rsplitn(3, ':').collect::<Vec<_>>();
    parts.reverse();
    let numeric = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_digit());
    Some(match parts.as_slice() {
        [path, line, col] if numeric(line) && numeric(col) => ((*path).to_owned(), line.parse().ok()),
        [head, a, b] if numeric(b) => (format!("{head}:{a}"), b.parse().ok()),
        [path, line] if numeric(line) => ((*path).to_owned(), line.parse().ok()),
        _ => (url.clone(), None),
    })
}

#[cfg(test)]
mod link_tests {
    use super::file_link;

    #[test]
    fn file_links_carry_their_line() {
        assert_eq!(file_link("src/main.rs"), Some(("src/main.rs".into(), None)));
        assert_eq!(file_link("src/main.rs:42"), Some(("src/main.rs".into(), Some(42))));
        assert_eq!(file_link("src/main.rs:42:7"), Some(("src/main.rs".into(), Some(42))));
        assert_eq!(file_link("/abs/a.rs#L9-L12"), Some(("/abs/a.rs".into(), Some(9))));
        assert_eq!(file_link("file:///abs/a.rs"), Some(("/abs/a.rs".into(), None)));
        assert_eq!(file_link("ftp://x"), None);
    }
}
