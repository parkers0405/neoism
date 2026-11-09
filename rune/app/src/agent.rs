//! Agent surfaces: the conversations sidebar, the transcript, the floating composer and its
//! menus. References: `rune/reference/04..13*.png`.

use crate::state::{App, Overlay, TabKind};
use crate::theme::{glyph, rgb};
use crate::wordmark;
use neoism_protocol::agent::{HistoryMessage, HistoryMessageKind, ThreadSummary};
use rune::{Color, Cx, El, ScrollSpec, Spring, Tx, WheelEvent, col, el, row, text};
use rune_kit::markdown::{CodeFrame, Markdown, MarkdownStyle};
use rune_kit::syntax::Syntax;

pub const COLUMN_PAD_X: f32 = 102.0;

/// The original wheel spring: critically damped, omega 16 (`timeline_scroll_policy.rs`), whose
/// response time 2π/16 ≈ 0.39 s is a zero-bounce Rune spring of that duration.
const WHEEL_SPRING: Spring = Spring::new(0.39, 0.0);
/// Rune reports a wheel notch as 40 points; the original moves three text rows per notch.

fn icon(app: &App, g: &str, scale: f32, color: Color) -> El {
    text(g.to_owned()).font(app.ty.icon(scale)).color(color)
}

fn ago(updated_ms: u64) -> String {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0);
    if updated_ms == 0 || updated_ms > now { return String::new(); }
    let s = (now - updated_ms) / 1000;
    match s {
        0..=59 => "just now".into(),
        60..=3599 => format!("{}m ago", s / 60),
        3600..=86399 => format!("{}h ago", s / 3600),
        86400..=604799 => format!("{}d ago", s / 86400),
        _ => format!("{}w ago", s / 604800),
    }
}

/// `Alt+C` / rail sidebar: New chat, scope, Pinned and Recent (`reference/04-sidebar-conversations.png`).
pub fn conversations(app: &App) -> El {
    let p = app.pal;
    let heading = |label: &'static str| text(label).font(app.ty.pixel(1.2)).color(rgb(p.fg)).ml(16.0).mt(22.0).mb(14.0);
    let row_for = |t: &ThreadSummary| {
        let id = t.session_id.clone();
        let selected = matches!(&app.active_tab().kind, TabKind::Agent { session: Some(s) } if *s == t.session_id);
        col().key(format!("thread:{}", t.session_id)).w_full().px(17.0).py(6.0).gap(3.0)
            .fill(if selected { rgb(p.surface).alpha(0.6) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(row().w_full().center_y().gap(8.0)
                .child(icon(app, "\u{f0a1e}", 0.7, rgb(p.dim)))
                .child(text("Neoism").font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
                .child(text(ago(t.updated_at)).font(app.ty.small()).color(rgb(p.muted)).mr(46.0)))
            .child(text(t.title.clone()).font(app.ty.body()).color(rgb(p.fg)).truncate().max_w(240.0))
            .on_press(move |s: &mut App| s.open_conversation(&id))
    };
    let mut list = col().w_full();
    let pinned: Vec<&ThreadSummary> = app.threads.iter().filter(|t| t.pinned).collect();
    if !pinned.is_empty() {
        list = list.child(heading("Pinned"));
        for t in pinned { list = list.child(row_for(t)); }
    }
    list = list.child(heading("Recent"));
    for t in app.threads.iter().filter(|t| !t.pinned) { list = list.child(row_for(t)); }
    col().key("sidebar").h_full().fill(rgb(p.bg)).border_sides([0.0, 1.0, 0.0, 0.0], rgb(p.border))
        .child(row().key("sidebar-heading").w_full().h(38.5).px(16.0).center_y()
            .border_sides([0.0, 0.0, 1.5, 0.0], rgb(p.border))
            .child(text("Agent").font(app.ty.body()).color(rgb(p.fg))))
        .child(col().w_full().px(18.0).pt(12.0).gap(4.0)
            .child(row().key("new-chat").w_full().h(36.0).px(16.0).gap(16.0).center_y().radius(6.0).hover(rgb(p.hover))
                .child(icon(app, glyph::EDIT, 0.9, rgb(p.fg)))
                .child(text("New chat").font(app.ty.body()).color(rgb(p.fg)))
                .on_press(|s: &mut App| { s.open_tab(TabKind::Agent { session: None }); }))
            .child(row().w_full().h(36.0).px(16.0).gap(16.0).center_y()
                .child(icon(app, glyph::FOLDER, 0.9, rgb(p.folder)))
                .child(text(app.workspace_name.clone()).font(app.ty.body()).color(rgb(p.fg)).grow(1.0))
                .child(icon(app, glyph::CHEVRON_DOWN, 0.8, rgb(p.dim))))
            .child(el().w_full().h(1.0).mt(6.0).fill(rgb(p.border))))
        .child(col().key("conversation-list").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: false }).child(list))
}

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
    let mut rows = col().key(format!("timeline:{session_id}")).w_full().px(COLUMN_PAD_X).pt(16.0).pb(200.0).gap(0.0);
    for (index, message) in messages.iter().enumerate() {
        let trace = matches!(message.kind, HistoryMessageKind::Tool | HistoryMessageKind::Subtask | HistoryMessageKind::Reasoning);
        if message.kind == HistoryMessageKind::System && message.title == "Context" { continue; }
        if trace && index < live_from { continue; }
        let last = index + 1 == messages.len();
        rows = rows.child(message_row(app, message, last && streaming));
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
    col().key("agent-pane").w_full().h_full().fill(rgb(p.bg))
        .child(timeline)
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
        HistoryMessageKind::User => row().key(key).w_full().justify_end().items_end().gap(12.0).mt(26.0)
            .child(col().max_w(830.0).shrink().px(20.0).py(12.0).radius(22.0).fill(rgb(0x0a84ff))
                .child(text(message.text.clone()).font(app.ty.body()).color(Color::WHITE).w_full()))
            .child(orb(app)),
        HistoryMessageKind::Reasoning => col().key(key).w_full().mt(24.0).gap(10.0)
            .child(text(if message.title.is_empty() { "Thinking".to_owned() } else { message.title.clone() })
                .font(app.ty.body().italic()).color(rgb(p.yellow).alpha(0.85)))
            .child(text(first_line(&message.text)).font(app.ty.body().italic()).color(rgb(p.dim))),
        HistoryMessageKind::Tool | HistoryMessageKind::Subtask => tool_card(app, message).key(key),
        HistoryMessageKind::System | HistoryMessageKind::Compaction => text(message.text.clone()).key(key)
            .font(app.ty.small()).color(rgb(p.muted)).mt(16.0).w_full().max_lines(6).truncate(),
        HistoryMessageKind::Assistant => col().key(key).w_full().mt(24.0).gap(14.0)
            .child(Markdown::new(message.text.clone())
                .highlights(crate::prose::highlights(&message.text, p))
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

fn tool_card(app: &App, message: &HistoryMessage) -> El {
    let p = app.pal;
    let done = matches!(message.status.as_str(), "completed" | "complete" | "success" | "");
    let title = if message.title.is_empty() { message.tool.clone() } else { message.title.clone() };
    row().w_full().mt(18.0).gap(12.0).center_y()
        .child(el().w(8.0).h(8.0).radius(4.0).fill(if done { rgb(p.green) } else { rgb(p.yellow) }))
        .child(icon(app, glyph::EDIT, 0.85, rgb(p.dim)))
        .child(text(title).font(app.ty.body()).color(rgb(p.fg)).truncate().shrink())
}

/// The animated pixel orb beside user messages; a static stand-in until the orb port lands.
fn orb(app: &App) -> El {
    let p = app.pal;
    let colors = [p.blue, p.green, p.magenta, p.yellow, p.cyan, p.red];
    let mut grid = col().w(32.0).h(32.0).clip_rounded(16.0);
    for y in 0..6 {
        let mut line = row();
        for x in 0..6 {
            line = line.child(el().w(32.0 / 6.0).h(32.0 / 6.0).fill(rgb(colors[(x * 7 + y * 3) % colors.len()])));
        }
        grid = grid.child(line);
    }
    grid
}

/// The floating composer card with its chip row and path bar (`reference/09`).
pub fn composer(app: &App, max_w: f32) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let streaming = matches!(&tab.kind, TabKind::Agent { session: Some(id) } if app.sessions.get(id).is_some_and(|s| s.streaming));
    let input = rune::text_input(&tab.draft, app.ty.composer())
        .key(format!("composer:{}", tab.id))
        .placeholder("Ask anything", rgb(p.dim))
        .color(rgb(p.fg)).w_full().min_h(26.0)
        .on_edit(|s: &mut App| { let i = s.active; &mut s.tabs[i].draft })
        .on_input_change(|s: &mut App, _: &mut Tx| {
            let i = s.active;
            let slash = s.tabs[i].draft.text.starts_with('/');
            if slash { s.overlay = Some(Overlay::Slash); } else if s.overlay == Some(Overlay::Slash) { s.overlay = None; }
        })
        .on_input_key(crate::keys::shortcut)
        .on_submit(|s: &mut App, _: &mut Tx| s.submit());
    let chip = |label: String, color: Color, overlay: Overlay| row().gap(6.0).center_y()
        .child(text(label).font(app.ty.composer()).color(color))
        .child(icon(app, glyph::CHEVRON_DOWN, 0.5, rgb(p.muted)))
        .on_press(move |s: &mut App| s.overlay = if s.overlay == Some(overlay) { None } else { Some(overlay) });
    let send = el().key("composer-send").w(36.0).h(36.0).radius(8.0).center()
        .fill(if streaming { rgb(p.fg) } else { rgb(p.muted).alpha(0.6) })
        .child(icon(app, if streaming { glyph::STOP } else { glyph::ARROW_UP }, 0.9, if streaming { rgb(p.bg) } else { rgb(p.fg) }))
        .on_press(|s: &mut App| {
            let i = s.active;
            match s.tabs[i].kind.clone() {
                TabKind::Agent { session: Some(id) } if s.sessions.get(&id).is_some_and(|x| x.streaming) => s.stop(&id),
                _ => s.submit(),
            }
        });
    let card = col().key("agent-input-card").w_full().radius(18.0).fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .px(20.0).pt(16.0).pb(12.0).gap(22.0)
        .child(stars(app))
        .child(input)
        .child(row().key("composer-chip-row").w_full().gap(26.0).center_y()
            .child(chip(app.agent.clone(), rgb(p.red), Overlay::Agents))
            .child(row().gap(8.0).center_y()
                .child(icon(app, glyph::OPENAI, 0.9, rgb(p.fg)))
                .child(chip(app.model.clone(), rgb(p.blue), Overlay::Models)))
            .child(chip(app.effort.clone(), rgb(p.magenta), Overlay::Models))
            .child(el().grow(1.0))
            .child(icon(app, glyph::PAPERCLIP, 0.9, rgb(p.dim)))
            .child(send));
    let footer = row().key("composer-footer").w_full().h(32.0).mx(28.0).px(18.0).center_y().justify_between()
        .fill(rgb(p.surface).alpha(0.7)).corners([0.0, 0.0, 12.0, 12.0])
        .child(row().gap(10.0).center_y()
            .child(icon(app, glyph::FOLDER, 0.85, rgb(p.dim)))
            .child(text(format!("{}/", app.display_cwd())).font(app.ty.small()).color(rgb(p.fg))))
        .child(row().gap(10.0)
            .child(text("/").font(app.ty.small()).color(rgb(p.fg)))
            .child(text("commands").font(app.ty.small()).color(rgb(p.muted))));
    col().w_full().max_w(max_w).child(card).child(footer)
}

/// Faint stars inside the composer card. Deterministic positions; twinkle is a later pass.
fn stars(app: &App) -> El {
    let p = app.pal;
    let mut layer = el().pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let mut seed: u32 = 0x9e37_79b9;
    for i in 0..48 {
        seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
        let x = (seed % 1000) as f32 / 1000.0;
        let y = ((seed >> 10) % 1000) as f32 / 1000.0;
        let size = if i % 7 == 0 { 2.0 } else { 1.2 };
        layer = layer.child(el().w(size).h(size).radius(size / 2.0).fill(rgb(p.fg).alpha(0.35))
            .absolute(x * 1000.0 % 820.0, y * 90.0));
    }
    layer
}

pub fn overlay(app: &App) -> El {
    let p = app.pal;
    let Some(kind) = app.overlay else { return el() };
    let (title, rows): (&str, Vec<(String, String, String)>) = match kind {
        Overlay::Slash => ("Commands", SLASH.iter().map(|(c, d, k)| (c.to_string(), d.to_string(), k.to_string())).collect()),
        Overlay::Models => ("Select model", vec![
            ("GPT-6.1 Sol".into(), "OpenAI".into(), "selected".into()),
            ("OpenAI".into(), String::new(), "26 models".into()),
            ("Anthropic".into(), String::new(), "22 models".into()),
            ("DeepSeek".into(), String::new(), "2 models".into()),
            ("OpenRouter".into(), String::new(), "361 models".into()),
        ]),
        Overlay::Agents => ("Agents", vec![
            ("build".into(), "Default software engineering agent with normal tool permissions.".into(), String::new()),
            ("plan".into(), "Read-only planning agent that can inspect context and write plan files.".into(), String::new()),
        ]),
        Overlay::Palette => ("Type a command...", Vec::new()),
    };
    let mut list = col().w_full().gap(2.0).px(10.0).pb(10.0);
    for (i, (name, description, kind)) in rows.into_iter().enumerate() {
        let selected = i == 0;
        let agent_pick = name.clone();
        list = list.child(row().w_full().px(16.0).py(8.0).radius(8.0).center_y()
            .fill(if selected { rgb(p.surface) } else { Color::TRANSPARENT })
            .hover(rgb(p.hover))
            .child(col().grow(1.0).gap(4.0)
                .child(text(name).font(app.ty.body()).color(rgb(p.fg)))
                .child_text_if(!description.is_empty(), app, description))
            .child(text(kind).font(app.ty.small()).color(rgb(p.muted)))
            .on_press(move |s: &mut App| {
                if s.overlay == Some(Overlay::Agents) { s.agent = title_word(&agent_pick); }
                s.overlay = None;
            }));
    }
    col().key("overlay").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).items_center().justify_end().pb(200.0)
        .on_press(|s: &mut App| s.overlay = None)
        .child(col().w(1030.0).radius(14.0).fill(rgb(p.bg)).border(1.0, rgb(p.border))
            .child(row().w_full().px(22.0).pt(14.0).pb(8.0).justify_between()
                .child(text(title).font(app.ty.body()).color(rgb(p.fg)))
                .child(text("esc").font(app.ty.small()).color(rgb(p.muted))))
            .child(text("Search").font(app.ty.body()).color(rgb(p.muted)).px(30.0).pb(10.0))
            .child(list))
}

fn title_word(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

trait TextIf {
    fn child_text_if(self, condition: bool, app: &App, value: String) -> Self;
}

impl TextIf for El {
    fn child_text_if(self, condition: bool, app: &App, value: String) -> El {
        if condition { self.child(text(value).font(app.ty.body()).color(rgb(app.pal.dim))) } else { self }
    }
}

const SLASH: &[(&str, &str, &str)] = &[
    ("/help", "Show available commands", "docs"),
    ("/model", "Switch model", "modal"),
    ("/connect", "Connect a provider (OAuth or API key)", "modal"),
    ("/usage", "View Codex account usage and reset times", "modal"),
    ("/think", "Set reasoning effort", "modal"),
    ("/agent", "Switch agent", "modal"),
    ("/compact", "Compact session context", "server"),
];
