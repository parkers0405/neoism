//! One agent tab: the wordmark home before the first prompt, then the transcript with markdown,
//! code frames, tool cards and the pinned composer (`reference/05..08`).

use super::composer::composer;
use super::prose;
use super::tools;
use crate::app::App;
use crate::ui::theme::rgb;
use crate::wordmark;
use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind};
use rune::{Color, Cx, El, ScrollSpec, Spring, Tx, WheelEvent, col, el, row, text};
use rune_kit::markdown::{CodeFrame, Markdown, MarkdownStyle};
use rune_kit::syntax::Syntax;

pub const COLUMN_PAD_X: f32 = 102.0;

/// The original wheel spring: critically damped, omega 16 (`timeline_scroll_policy.rs`), whose
/// response time 2π/16 ≈ 0.39 s is a zero-bounce Rune spring of that duration.
const WHEEL_SPRING: Spring = Spring::new(0.39, 0.0);
/// Rune reports a wheel notch as 40 points; the original moves three text rows per notch.

/// One agent tab: the wordmark home before the first prompt, then the transcript.
pub fn pane(app: &App, session: Option<&str>, _cx: &mut Cx<App>) -> El {
    let p = app.pal;
    let Some(session_id) = session else {
        return col().key("agent-pane").w_full().h_full().fill(rgb(p.bg)).items_center().justify_center()
            .child(wordmark::view(app, "agent-wordmark", 470.5))
            .child(el().h(47.0))
            .child(composer(app, 845.0).key("agent-composer-shell"));
    };
    let session = app.sessions.get(session_id);
    let messages = session.map(|s| s.messages.as_slice()).unwrap_or(&[]);
    let streaming = session.is_some_and(|s| s.streaming);
    let live_from = session.and_then(|s| s.live_from).unwrap_or(usize::MAX);
    let mut rows = col().key(format!("timeline:{session_id}")).w_full().px(COLUMN_PAD_X).pt(16.0).pb(200.0).gap(0.0)
        .on_layout(|a: &mut App, [_, _, w, _]: [f32; 4]| a.timeline_width = (w - 2.0 * COLUMN_PAD_X).max(0.0));
    let mut index = 0;
    while index < messages.len() {
        let message = &messages[index];
        if !tools::visible(message, index, live_from) { index += 1; continue; }
        // Runs of reads and of settled edits fold into one row.
        if let Some((end, group)) = tools::group_at(messages, index) {
            rows = rows.child(tools::row(app, &group));
            index = end;
            continue;
        }
        let last = index + 1 == messages.len();
        rows = rows.child(message_row(app, message, last && streaming));
        index += 1;
    }
    if streaming {
        rows = rows.child(text("Crafting . . .").font(app.ty.pixel(0.95)).color(rgb(p.fg)).mt(18.0).key("activity"));
    }
    let id = session_id.to_owned();
    // Older history pages load when this sentinel near the top comes into view.
    let sentinel = el().key("older-sentinel").w_full().h(1.0)
        .on_visible(move |s: &mut App, visible: bool, _: &mut Tx| if visible { s.request_older(&id); });
    let scroll_key = format!("conversation:{session_id}");
    let mirror = app.scroll.clone();
    let line_h = app.ty.body().line_px();
    let wheel_key = scroll_key.clone();
    let timeline = col().key(scroll_key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: true })
        .scroll_anchor()
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(move |s: &mut App, e: WheelEvent, tx: &mut Tx| {
            // Trackpads keep Rune's direct 1:1 scroll and fling; notches glide on the spring,
            // retargeting mid-glide so consecutive notches are one continuous motion.
            if e.precise || e.dy == 0.0 { return false; }
            let (offset, max) = s.scroll.get();
            let base = s.wheel_target.filter(|t| (t - offset).abs() > 0.5).unwrap_or(offset);
            let target = (base - e.dy / 40.0 * 3.0 * line_h).clamp(0.0, max);
            s.wheel_target = Some(target);
            tx.scroll_to_with(wheel_key.clone(), 0.0, target, WHEEL_SPRING);
            tx.live_only();
            true
        })
        .child(col().w_full().child(sentinel).child(rows));
    let mut pane = col().key("agent-pane").w_full().h_full().fill(rgb(p.bg));
    if let Some(header) = super::header::view(app, session_id) {
        pane = pane.child(col().w_full().px(COLUMN_PAD_X).child(header));
    }
    pane.child(timeline)
        .child(col().w_full().px(86.0).pb(6.0).pin(Some(0.0), None, Some(0.0), Some(0.0))
            .child(composer(app, f32::INFINITY)))
}

fn markdown_style(app: &App) -> MarkdownStyle {
    let p = app.pal;
    MarkdownStyle::neutral(true)
        .text(rgb(p.fg))
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
        HistoryMessageKind::User => super::user::view(app, &message.text, message.author.as_deref(), app.timeline_width)
            .key(key).mt(26.0),
        HistoryMessageKind::Reasoning => col().key(key).w_full().mt(24.0).gap(10.0)
            .child(text(if message.title.is_empty() { "Thinking".to_owned() } else { message.title.clone() })
                .font(app.ty.body().italic()).color(rgb(p.yellow).alpha(0.85)))
            .child(text(first_line(&message.text)).font(app.ty.body().italic()).color(rgb(p.dim))),
        HistoryMessageKind::Tool | HistoryMessageKind::Subtask => tools::row(app, message),
        HistoryMessageKind::System | HistoryMessageKind::Compaction => text(message.text.clone()).key(key)
            .font(app.ty.small()).color(rgb(p.muted)).mt(16.0).w_full().max_lines(6).truncate(),
        HistoryMessageKind::Assistant => col().key(key).w_full().mt(24.0).gap(14.0)
            .child(Markdown::new(message.text.clone())
                .highlights(prose::highlights(&message.text, p))
                .streaming(streaming)
                .reveal(if streaming { 0.18 } else { 0.0 })
                .code_frame(code_frame(app))
                .render(&markdown_style(app)))
            .child_meta(app, message),
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

