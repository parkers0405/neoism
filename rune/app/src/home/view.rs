//! Home views: the splash (wordmark and shortcut grid) until the first command, then blocks;
//! the command composer is always pinned below.

use super::splash::{MENU, layout};
use crate::app::App;
use crate::ui::icon;
use crate::ui::theme::{NERD, glyph, rgb};
use crate::wordmark;
use rune::tok::TextStyle;
use rune::{Color, El, Tx, col, el, row, text};

pub fn view(app: &App) -> El {
    let p = app.pal;
    let body = if app.visible_wordmark().is_some() { splash(app) } else { terminal_blocks(app) };
    col().w_full().h_full().fill(app.surface(p.bg))
        .child(body)
        .child(terminal_composer(app))
}

/// The wordmark and shortcut menu, placed by `splash::layout` in the measured area.
fn splash(app: &App) -> El {
    let p = app.pal;
    let [width, height] = app.splash.area;
    let mut area = el().key("splash").w_full().grow(1.0).shrink()
        .on_layout(|a: &mut App, [_, _, w, h]: [f32; 4]| a.splash.area = [w, h]);
    // The Home pane insets its splash by 40 x 8 pt a side (`HomeTerminalView`, 80 x 16 total).
    let s = app.ty.scale();
    let (inset_x, inset_y) = (40.0 * s, 8.0 * s);
    let Some(g) = layout(width - 2.0 * inset_x, height - 2.0 * inset_y, app.ty.size, s) else { return area };
    let [x, y, w, _] = g.wordmark;
    area = area.child(wordmark::view(app, "splash-wordmark", w).absolute(inset_x + x, inset_y + y));
    for (index, (item, rect)) in MENU.iter().zip(&g.buttons).enumerate() {
        let hovered = app.splash.hover == Some(index);
        let ink = if hovered { rgb(p.fg) } else { rgb(p.dim) };
        let action = item.action;
        let button = row().key(format!("splash-menu:{index}")).w(rect[2]).h(rect[3]).px(g.pad).center_y()
            .radius(g.radius)
            .fill(if hovered { rgb(p.hover).alpha(0.95) } else { Color::TRANSPARENT })
            .child(el().w(g.icon_slot).center().child(text(item.icon).font(TextStyle::new(g.icon_font, 400.0).family(NERD)).color(ink)))
            .child(text(item.label).font(TextStyle::new(g.label_font, 700.0).mono()).color(ink).ml(g.label_gap).grow(1.0).truncate().shrink())
            .child(text(format!("[{}]", item.keys)).font(TextStyle::new(g.key_font, 700.0).mono()).color(rgb(p.green)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.splash.hover = Some(index); } else if a.splash.hover == Some(index) { a.splash.hover = None; }
            })
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.splash_action(action, tx))
            .absolute(inset_x + rect[0], inset_y + rect[1]);
        area = area.child(button);
    }
    area
}

fn terminal_composer(app: &App) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let input = rune::text_input(&tab.draft, app.ty.small())
        .key(format!("terminal-command:{}", tab.id))
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Enter a command", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|s: &mut App| { let i = s.active; &mut s.tabs[i].draft })
        .on_input_key(crate::keys::shortcut)
        .on_submit(|s: &mut App, _: &mut Tx| s.submit());
    col().key("terminal-block-input").w_full().h(93.0).px(16.5).pt(16.0).gap(14.0)
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.border))
        .child(row().w_full().center_y()
            .child(text(app.tab_cwd(tab)).font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
            .child(el().w(28.0).h(28.0).radius(6.0).center()
                .child(icon(app, glyph::STAR_OUTLINE, 1.1, rgb(p.dim)))))
        .child(input)
}

/// Warp-style command blocks (`reference/02`): cwd, bold command, output, duration or exit
/// status on the right, 1 pt rules between blocks; failed blocks get a red tint and left bar.
fn terminal_blocks(app: &App) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let Some(term) = app.terminals.get(&tab.id) else { return el() };
    let home = std::env::var("HOME").unwrap_or_default();
    let mut list = col().w_full();
    for block in term.blocks.iter().chain(term.live.iter()) {
        list = list.child(terminal_block(app, block, &home));
    }
    if let Some(error) = &term.error {
        list = list.child(text(error.clone()).font(app.ty.small()).color(rgb(p.red)).px(16.5).py(12.0));
    }
    col().key(format!("terminal-scroll:{}", tab.id)).w_full().grow(1.0).shrink()
        .scroll(rune::ScrollSpec { stick_bottom: true })
        .child(list)
}

fn terminal_block(app: &App, block: &super::Block, home: &str) -> El {
    let p = app.pal;
    let cwd = if !home.is_empty() && block.cwd.starts_with(home) { format!("~{}", &block.cwd[home.len()..]) } else { block.cwd.clone() };
    let ms = block.elapsed_ms.unwrap_or_else(|| block.started.elapsed().as_millis());
    let meta = match block.exit {
        Some(code) if code != 0 => format!("exit {code}"),
        _ if block.elapsed_ms.is_none() => format!("{} s", ms / 1000),
        _ if ms >= 1000 => format!("{:.1} s", ms as f32 / 1000.0),
        _ => format!("{ms} ms"),
    };
    let meta_color = if block.failed() { rgb(p.red) } else { rgb(p.dim) };
    let (body, spans) = terminal_rich(&block.lines);
    let mut item = col().key(format!("terminal-block:{}", block.id)).w_full().px(16.5).pt(16.0).pb(18.0).gap(6.0)
        .border_sides([0.0, 0.0, 1.0, 0.0], rgb(p.border));
    if !block.background {
        item = item
            .child(row().w_full().center_y()
                .child(text(cwd).font(app.ty.small()).color(rgb(p.dim)).grow(1.0))
                .child(text(meta).font(app.ty.small()).color(meta_color)))
            .child(text(block.command.clone()).font(app.ty.bold()).color(rgb(p.fg)));
    }
    if !body.is_empty() {
        item = item.child(rune::rich(body, spans).font(app.ty.body()).color(rgb(p.fg)).w_full().mt(if block.background { 0.0 } else { 10.0 }));
    }
    if block.failed() {
        item = item.fill(rgb(p.red).alpha(0.08)).border_sides([0.0, 0.0, 1.0, 3.0], rgb(p.red));
    }
    item
}

/// Ghostty rows as one text element: runs padded to their cell column, coloured by span.
fn terminal_rich(lines: &[crate::ghostty::Row]) -> (String, Vec<rune::Span>) {
    let mut body = String::new();
    let mut spans = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if i > 0 { body.push('\n'); }
        let mut col = 0;
        let last = line.iter().rposition(|r| !r.text.trim_end().is_empty() || r.bg.is_some());
        for (index, run) in line.iter().enumerate() {
            if last.is_none_or(|last| index > last) { break; }
            body.extend(std::iter::repeat_n(' ', run.col.saturating_sub(col)));
            col = run.col + run.width;
            let text = if Some(index) == last && run.bg.is_none() { run.text.trim_end() } else { run.text.as_str() };
            let start = body.len();
            body.push_str(text);
            if run.fg.is_some() || run.bg.is_some() || run.bold || run.italic || run.underline {
                let mut span = rune::Span::new(start, body.len());
                if let Some(c) = run.fg { span = span.color(rgb(c)); }
                if let Some(c) = run.bg { span.bg = Some(rgb(c)); }
                if run.bold { span = span.weight(800.0); }
                if run.italic { span = span.italic(); }
                if run.underline { span.underline = Some(run.fg.map(rgb).unwrap_or(Color::TRANSPARENT)); }
                spans.push(span);
            }
        }
    }
    (body, spans)
}
