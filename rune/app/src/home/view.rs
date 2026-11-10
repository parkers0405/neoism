//! Home views (`BlockTerminalView.tsx`): a bottom-anchored history whose header is the splash and
//! whose body is command blocks, over the command composer. A full-screen program or REPL swaps
//! the whole pane for Ghostty's grid (`grid.rs`).

use super::composer::{Menu, MenuKind, typed_prefix};
use super::splash::{MENU, layout};
use super::{Block, Cells, Terminal};
use crate::app::App;
use crate::ghostty::Row;
use crate::ui::theme::{NERD, Palette, rgb};
use crate::wordmark;
use rune::tok::TextStyle;
use rune::{Color, El, ScrollSpec, Span, Spring, Tx, WheelEvent, col, el, row, text};

/// The agent timeline's wheel glide: critically damped, omega 16.
const WHEEL_SPRING: Spring = Spring::new(0.39, 0.0);
/// Rune reports a wheel notch as 40 points; terminals move three rows per notch.
const NOTCH: f32 = 40.0;

pub fn view(app: &App) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let tab_id = tab.id;
    let Some(term) = app.terminals.get(&tab_id) else { return el() };
    let pane = col().key(format!("home-pane:{tab_id}")).w_full().h_full().fill(app.surface(p.bg))
        .on_layout(move |a: &mut App, [_, _, w, h]: [f32; 4]| {
            if let Some(term) = a.terminals.get_mut(&tab_id) { term.area = [w, h]; }
        });
    if term.raw() { return pane.child(super::grid::view(app, term)); }
    let composer_h = composer_height(app);
    let mut pane = pane.child(history(app, term, composer_h));
    if !(term.shell.busy() && term.long_running()) {
        pane = pane.child(composer(app, term, composer_h));
    }
    pane
}

fn composer_height(app: &App) -> f32 {
    let cells = Cells::of(app);
    cells.row * 0.9 + cells.row + 52.0 * app.ty.scale()
}

/// The text style of terminal ink: the UI font at the configured size, one row per line.
fn ink(app: &App, scale: f32, weight: Option<f32>) -> TextStyle {
    let cells = Cells::of(app);
    TextStyle::new(app.ty.size * scale, weight.unwrap_or(app.ty.weight)).mono().lh(cells.row)
}

/// The scrollback: the splash as its first screen, then the blocks, glued to the bottom.
fn history(app: &App, term: &Terminal, composer_h: f32) -> El {
    let p = app.pal;
    let tab_id = app.active_tab().id;
    let key = format!("terminal-scroll:{tab_id}");
    let viewport_h = (term.area[1] - if term.shell.busy() && term.long_running() { 0.0 } else { composer_h }).max(0.0);
    let home = std::env::var("HOME").unwrap_or_default();
    // Home heads its scrollback with the splash; a plain Terminal tab starts at the prompt.
    let mut content = col().w_full().min_h(viewport_h).justify_end();
    if app.active_tab().kind == crate::tabs::TabKind::Home {
        content = content.child(splash(app).h(viewport_h).w_full());
    }
    for block in term.shell.blocks.iter().chain(term.shell.live.iter()) {
        content = content
            .child(block_view(app, term, block, &home))
            .child(el().w_full().h(1.0).fill(rgb(p.fg).alpha(0.1)));
    }
    if let Some(error) = &term.shell.error {
        content = content.child(text(error.clone()).font(ink(app, 0.9, None)).color(rgb(p.red)).px(Cells::of(app).inset).py(12.0));
    }
    let mirror = term.scroll.clone();
    let wheel_key = key.clone();
    let line_h = Cells::of(app).row;
    col().key(key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: true })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            // Trackpads keep Rune's 1:1 scroll and fling; notches glide on the spring.
            if e.precise || e.dy == 0.0 { return false; }
            let Some(term) = a.terminals.get_mut(&tab_id) else { return false };
            let (offset, max) = term.scroll.get();
            let base = term.wheel_target.filter(|t| (t - offset).abs() > 0.5).unwrap_or(offset);
            let target = (base - e.dy / NOTCH * 3.0 * line_h).clamp(0.0, max);
            term.wheel_target = Some(target);
            tx.scroll_to_with(wheel_key.clone(), 0.0, target, WHEEL_SPRING);
            tx.live_only();
            true
        })
        .child(content)
}

/// `commandBlockHeader` and `terminalDurationLabel`.
fn duration_label(ms: u128) -> String {
    if ms < 1000 { format!("{ms} ms") } else { format!("{:.1} s", ms as f64 / 1000.0) }
}

fn block_view(app: &App, term: &Terminal, block: &Block, home: &str) -> El {
    let p = app.pal;
    let cells = Cells::of(app);
    let row_h = cells.row;
    let s = app.ty.scale();
    let width = term.area[0];
    let command: Vec<&str> = if block.background { Vec::new() } else { block.command.split('\n').collect() };
    let output = row_h * (1.1 + 0.9 + 0.19 + command.len() as f32 + if block.lines.is_empty() { 0.0 } else { 0.5 });
    // A truncated block says so in one muted row above the rows it kept.
    let note = if block.truncated { row_h } else { 0.0 };
    let height = output + note + block.lines.len() as f32 * row_h + row_h * if block.running() { 0.2 } else { 1.0 };
    let failed = block.failed();
    let selected = term.selected == Some(block.id);
    let block_id = block.id;
    let tab_id = app.active_tab().id;
    let tint = (failed || selected).then_some(if failed { p.red } else { p.accent });
    // Tinted blocks wash the whole block and carry a 2 pt bar on the left.
    let wash = |el: El, h: f32| match tint {
        Some(tint) => el.child(el_fill(rgb(tint).alpha(0.06), h)).child(el_bar(rgb(tint), 2.0 * s, h)),
        None => el,
    };
    // The header (directory, status, command) sticks to the top while its output scrolls
    // under it, as GPUiX's sticky header did; the block's end pushes it out.
    let label = if block.background { "Shell output".to_owned() } else { directory_label(&block.cwd, home) };
    let mut header = wash(el().w_full().h(output).fill(rgb(p.bg)), output).sticky_top(0.0)
        .child(text(label).font(ink(app, 0.9, None)).color(rgb(p.dim)).absolute(cells.inset, row_h * 1.1));
    // The status hides under the pointer (where GPUiX shows the block's actions).
    if !block.background && term.hovered != Some(block.id) && width > 260.0 * s {
        let ms = block.elapsed_ms.unwrap_or_else(|| block.started.elapsed().as_millis() / 1000 * 1000);
        let duration = duration_label(ms);
        let (status, color) = if block.running() {
            (format!("Running  {duration}"), p.accent)
        } else if let Some(code) = block.exit.filter(|c| *c != 0) {
            (format!("exit {code}  {duration}"), p.red)
        } else {
            (duration, p.muted)
        };
        header = header.child(text(status).font(ink(app, 0.9, None)).color(rgb(color))
            .pin(None, Some(row_h * 1.1), Some(cells.inset), None));
    }
    for (index, line) in command.iter().enumerate() {
        header = header.child(text((*line).to_owned()).font(ink(app, 1.0, None)).color(rgb(p.fg))
            .absolute(cells.inset, row_h * (2.19 + index as f32)));
    }
    let body_h = height - output;
    let mut body = wash(el().w_full().h(body_h), body_h);
    if block.truncated {
        body = body.child(text(format!("Earlier output truncated; showing the last {} lines", block.lines.len()))
            .font(ink(app, 0.85, None)).color(rgb(p.muted)).absolute(cells.inset, 0.0));
    }
    if !block.lines.is_empty() {
        body = body.child(rows_view(app, &block.lines, cells, lift_for(p)).absolute(cells.inset, note));
    }
    if let (Some((row, col)), true) = (block.cursor, block.running() && term.long_running()) {
        body = body.child(el().w(cells.cell).h(row_h).fill(rgb(p.accent).alpha(0.45))
            .absolute(cells.inset + col as f32 * cells.cell, note + row as f32 * row_h));
    }
    col().key(format!("terminal-block:{}", block.id)).w_full().h(height)
        .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
            if let Some(term) = a.terminals.get_mut(&tab_id) {
                if inside { term.hovered = Some(block_id); } else if term.hovered == Some(block_id) { term.hovered = None; }
            }
        })
        .on_press(move |a: &mut App| {
            if let Some(term) = a.terminals.get_mut(&tab_id) {
                term.selected = if term.selected == Some(block_id) { None } else { Some(block_id) };
            }
        })
        .child(header)
        .child(body)
}

fn el_fill(color: Color, h: f32) -> El { el().w_full().h(h).fill(color).absolute(0.0, 0.0) }

fn el_bar(color: Color, w: f32, h: f32) -> El { el().w(w).h(h).fill(color).absolute(0.0, 0.0) }

/// `terminalDirectoryLabel`: `~` for the home directory.
fn directory_label(directory: &str, home: &str) -> String {
    if directory.is_empty() { return "Terminal".into(); }
    if home.is_empty() { return directory.to_owned(); }
    let home = home.trim_end_matches('/');
    if directory == home { return "~".into(); }
    match directory.strip_prefix(&format!("{home}/")) {
        Some(rest) => format!("~/{rest}"),
        None => directory.to_owned(),
    }
}

/// Captured rows as one text element with a line per row, runs padded to their cell column,
/// plus background rectangles. `lift` brightens low-contrast default-background ink.
pub fn rows_view(app: &App, rows: &[Row], cells: Cells, lift: impl Fn(u32) -> u32) -> El {
    let mut body = String::new();
    let mut spans = Vec::new();
    let mut fills = Vec::new();
    for (y, line) in rows.iter().enumerate() {
        if y > 0 { body.push('\n'); }
        let mut col = 0;
        let last = line.iter().rposition(|r| !r.text.trim_end().is_empty() || r.bg.is_some());
        for (index, run) in line.iter().enumerate() {
            if last.is_none_or(|last| index > last) { break; }
            if let Some(bg) = run.bg { fills.push((run.col, y, run.width, bg)); }
            body.extend(std::iter::repeat_n(' ', run.col.saturating_sub(col)));
            col = run.col + run.width;
            let text = if Some(index) == last && run.bg.is_none() { run.text.trim_end() } else { run.text.as_str() };
            let start = body.len();
            body.push_str(text);
            let fg = run.fg.map(|c| if run.bg.is_none() { lift(c) } else { c });
            if fg.is_some() || run.bold || run.italic || run.underline {
                let mut span = Span::new(start, body.len());
                if let Some(c) = fg { span = span.color(rgb(c)); }
                if run.bold { span = span.weight(700.0); }
                if run.italic { span = span.italic(); }
                if run.underline { span.underline = Some(fg.map(rgb).unwrap_or(rgb(app.pal.fg))); }
                spans.push(span);
            }
        }
    }
    let mut layer = el().w_full().h(rows.len() as f32 * cells.row);
    for (col, y, width, bg) in fills {
        layer = layer.child(el().w(width as f32 * cells.cell).h(cells.row).fill(rgb(bg))
            .absolute(col as f32 * cells.cell, y as f32 * cells.row));
    }
    layer.child(text(body).spans(spans).font(ink(app, 1.0, None)).color(rgb(app.pal.fg)).absolute(0.0, 0.0))
}

/// `blockOutputForeground`: on dark themes, blend ink toward white until it reaches 4.5:1.
fn lift_for(p: &'static Palette) -> impl Fn(u32) -> u32 {
    let bg = luminance(p.bg);
    move |color| {
        if bg >= 0.175 || contrast(luminance(color), bg) >= 4.5 { return color; }
        let blend = |amount: f32| {
            let channel = |shift: u32| {
                let c = ((color >> shift) & 0xff) as f32;
                ((c + (255.0 - c) * amount).round() as u32) << shift
            };
            channel(16) | channel(8) | channel(0)
        };
        let (mut low, mut high) = (0.0, 1.0);
        for _ in 0..12 {
            let mid = (low + high) / 2.0;
            if contrast(luminance(blend(mid)), bg) >= 4.5 { high = mid; } else { low = mid; }
        }
        blend(high)
    }
}

fn luminance(color: u32) -> f32 {
    [16, 8, 0].iter().zip([0.2126, 0.7152, 0.0722]).map(|(shift, weight)| {
        let c = ((color >> shift) & 0xff) as f32 / 255.0;
        weight * if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
    }).sum()
}

fn contrast(a: f32, b: f32) -> f32 { (a.max(b) + 0.05) / (a.min(b) + 0.05) }

/// The wordmark and shortcut menu, placed by `splash::layout` in the measured area.
fn splash(app: &App) -> El {
    let p = app.pal;
    let tab = app.active_tab().id;
    let [width, height] = app.splash.area.get(&tab).copied().unwrap_or_default();
    let mut area = el().key(format!("splash:{tab}"))
        .on_layout(move |a: &mut App, [_, _, w, h]: [f32; 4]| { a.splash.area.insert(tab, [w, h]); });
    // The Home pane insets its splash by 40 x 8 pt a side (`HomeTerminalView`, 80 x 16 total).
    let s = app.ty.scale();
    let (inset_x, inset_y) = (40.0 * s, 8.0 * s);
    let Some(g) = layout(width - 2.0 * inset_x, height - 2.0 * inset_y, app.ty.size, s) else { return area };
    let [x, y, w, _] = g.wordmark;
    area = area.child(wordmark::view(app, &format!("splash-wordmark:{tab}"), w).absolute(inset_x + x, inset_y + y)
        .on_visible(move |a: &mut App, visible: bool, _: &mut Tx| {
            if visible { a.splash.hidden.remove(&tab); } else { a.splash.hidden.insert(tab); }
        }));
    for (index, (item, rect)) in MENU.iter().zip(&g.buttons).enumerate() {
        let hovered = app.splash.hover == Some((tab, index));
        let ink = if hovered { rgb(p.fg) } else { rgb(p.dim) };
        let action = item.action;
        let button = row().key(format!("splash-menu:{tab}:{index}")).w(rect[2]).h(rect[3]).px(g.pad).center_y()
            .radius(g.radius)
            .fill(if hovered { rgb(p.hover).alpha(0.95) } else { Color::TRANSPARENT })
            .child(el().w(g.icon_slot).center().child(text(item.icon).font(TextStyle::new(g.icon_font, 400.0).family(NERD)).color(ink)))
            .child(text(item.label).font(TextStyle::new(g.label_font, 700.0).mono()).color(ink).ml(g.label_gap).grow(1.0).truncate().shrink())
            .child(text(format!("[{}]", item.keys)).font(TextStyle::new(g.key_font, 700.0).mono()).color(rgb(p.green)))
            .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
                if inside { a.splash.hover = Some((tab, index)); } else if a.splash.hover == Some((tab, index)) { a.splash.hover = None; }
            })
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.splash_action(action, tx))
            .absolute(inset_x + rect[0], inset_y + rect[1]);
        area = area.child(button);
    }
    area
}

/// The composer (`terminal-block-input`): directory, star, one command line with the history
/// ghost after the caret, and the history/completion menu above it.
fn composer(app: &App, term: &Terminal, height: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let cells = Cells::of(app);
    let tab = app.active_tab();
    let draft = &tab.draft;
    let input = rune::text_input(draft, ink(app, 1.0, None))
        .key(format!("terminal-command:{}", tab.id))
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Enter a command", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(|a: &mut App| { let i = a.active; &mut a.tabs[i].draft })
        .on_input_change(|a: &mut App, _: &mut Tx| a.composer_edited())
        .on_input_key(|a: &mut App, e: &rune::KeyEvent, tx: &mut Tx| a.terminal_composer_key(e, tx))
        .on_submit(|a: &mut App, _: &mut Tx| a.submit());
    let line_top = 28.0 * s + cells.row * 0.9;
    let mut card = el().key("terminal-block-input").w_full().h(height)
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.fg).alpha(0.1))
        .opacity(if term.shell.busy() { 0.6 } else { 1.0 })
        .child(text(app.tab_cwd(tab)).font(ink(app, 0.9, None)).color(rgb(p.dim)).absolute(cells.inset, 13.0 * s))
        .child(el().w(28.0 * s).h(28.0 * s).radius(6.0 * s).center()
            .child(text("\u{f006}").font(TextStyle::new(app.ty.size, 400.0).family(NERD)).color(rgb(p.dim)))
            .pin(None, Some(9.0 * s), Some(12.0 * s), None))
        .child(el().h(cells.row).pin(Some(cells.inset), Some(line_top), Some(cells.inset), None).child(input));
    let at_end = draft.anchor == draft.focus && draft.focus == draft.text.len();
    if term.composer.menu.is_none() && at_end && !term.shell.busy() {
        if let Some(ghost) = term.composer.ghost(&draft.text, &term.shell.recent) {
            let rest = ghost[draft.text.len()..].split('\n').next().unwrap_or("").to_owned();
            let x = cells.inset + draft.text.chars().count() as f32 * cells.cell;
            card = card.child(text(rest).font(ink(app, 1.0, None)).color(rgb(p.muted)).absolute(x, line_top));
        }
    }
    let mut stack = el().w_full().h(height).child(card);
    if let Some(menu) = term.composer.menu.as_ref().filter(|m| !m.items.is_empty()) {
        stack = stack.child(menu_view(app, menu, term.area[1] - height).pin(Some(cells.inset), None, Some(cells.inset), Some(height)));
    }
    stack
}

/// The history/completion list (30 pt rows, at most eight, history newest nearest the composer).
fn menu_view(app: &App, menu: &Menu, space: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let item_h = 30.0 * s;
    let count = (((space - 8.0 * s) / item_h).floor() as usize).clamp(1, 8).min(menu.items.len());
    let reversed = menu.kind == MenuKind::History;
    // Keep the chosen row in view: a window of `count` rows around it.
    let first = menu.choice.saturating_sub(count - 1).min(menu.items.len() - count);
    let mut rows: Vec<El> = Vec::new();
    for index in first..first + count {
        let item = &menu.items[index];
        let chosen = index == menu.choice;
        let prefix = typed_prefix(&menu.query, item);
        let mut line = row().key(format!("terminal-suggestion:{index}")).w_full().h(item_h).px(8.0 * s).gap(8.0 * s).center_y()
            .radius(3.0 * s)
            .fill(if chosen { rgb(p.hover) } else { Color::TRANSPARENT })
            .child(el().w(20.0 * s).center().child(text(item.kind.icon()).font(TextStyle::new(14.0 * s, 400.0).family(NERD))
                .color(if chosen { rgb(p.accent) } else { rgb(p.dim) })))
            .child(row().grow(1.0).shrink()
                .child(text(prefix.to_owned()).font(TextStyle::new(12.0 * s, 600.0).mono()).color(rgb(p.fg)))
                .child(text(item.label[prefix.len()..].to_owned()).font(TextStyle::new(12.0 * s, app.ty.weight).mono())
                    .color(if chosen { rgb(p.fg) } else { rgb(p.dim) }).truncate().shrink()));
        if item.kind == super::composer::Kind::History {
            line = line.child(text("\u{f017}").font(TextStyle::new(11.0 * s, 400.0).family(NERD)).color(rgb(p.muted)));
        }
        let value = item.value.clone();
        line = line.on_press(move |a: &mut App| {
            let index = a.active;
            if let Some(term) = a.terminals.get_mut(&a.tabs[index].id) { term.composer.menu = None; }
            let end = value.len();
            a.tabs[index].draft.replace_text(value.clone());
            a.tabs[index].draft.set_caret(end);
        });
        rows.push(line);
    }
    if reversed { rows.reverse(); }
    let mut list = col().w_full().padding(4.0 * s);
    for line in rows { list = list.child(line); }
    el().key("terminal-suggestions").border(1.0, rgb(p.border)).radius(5.0 * s).fill(rgb(p.bg)).child(list)
}
