//! Home views (`BlockTerminalView.tsx`): a bottom-anchored history whose header is the splash and
//! whose body is command blocks, over the command composer. A full-screen program or REPL swaps
//! the whole pane for Ghostty's grid (`grid.rs`).

use super::composer::{Menu, MenuKind, typed_prefix};
use super::splash::{MENU, layout};
use super::{Block, Cells, Terminal};
use crate::app::App;
use crate::ghostty::Row;
use crate::ui::feather::Icon;
use crate::ui::theme::{NERD, Palette, rgb};
use crate::wordmark;
use rune::tok::TextStyle;
use rune::{Color, El, ScrollSpec, Span, Spring, Tx, WheelEvent, col, el, row, text};

/// The agent timeline's wheel glide: critically damped, omega 16.
const WHEEL_SPRING: Spring = Spring::new(0.39, 0.0);
/// Rune reports a wheel notch as 40 points; terminals move three rows per notch.
const NOTCH: f32 = 40.0;
/// The filter row a filtered block adds under its command (`34 * s`).
const FILTER_ROW: f32 = 34.0;
/// `TerminalAction`: a 26 pt button holding a 16 pt icon, 5 pt corners.
const ACTION: f32 = 26.0;

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
    let input = !(term.shell.busy() && term.long_running());
    if input {
        pane = pane.child(composer(app, term, composer_h));
    }
    // `block-jump-latest`: away from the bottom, one press glides back to the newest output.
    let (offset, max) = term.scroll.get();
    if max > 0.0 && offset < max - 1.0 {
        let s = app.ty.scale();
        let bottom = if input { composer_h } else { 0.0 } + 10.0 * s;
        pane = pane.child(el().radius(4.0 * s).fill(rgb(p.surface))
            .child(action_button(app, format!("block-jump-latest:{tab_id}"), Icon::Down, false, None)
                .on_press_tx(move |a: &mut App, tx: &mut Tx| {
                    if let Some(term) = a.terminals.get_mut(&tab_id) {
                        let max = term.scroll.get().1;
                        term.wheel_target = Some(max);
                        term.glide = None;
                        tx.scroll_to_with(format!("terminal-scroll:{tab_id}"), 0.0, max, WHEEL_SPRING);
                    }
                }))
            .pin(None, None, Some(16.0 * s), Some(bottom)));
    }
    pane
}

/// The active terminal's block actions menu, attached at the window root (an anchored overlay
/// placed inside the pane would be positioned from the pane's corner).
pub fn block_menu_overlay(app: &App) -> El {
    let tab = app.active_tab().id;
    app.terminals.get(&tab).and_then(|term| block_menu(app, term)).unwrap_or_else(el)
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
    let mut content = col().w_full().min_h(viewport_h).justify_end().selection_group(true);
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
    let line_h = Cells::of(app).row;
    col().key(key).w_full().grow(1.0).shrink()
        .scroll(ScrollSpec { stick_bottom: true })
        .on_scroll_offset(move |pos| mirror.set((pos.offset[1], pos.max[1])))
        .on_wheel(move |a: &mut App, e: WheelEvent, tx: &mut Tx| {
            // Trackpads keep Rune's 1:1 scroll and fling; notches glide on the spring.
            if e.precise || e.dy == 0.0 { return false; }
            let Some(term) = a.terminals.get_mut(&tab_id) else { return false };
            // An open block menu would drift from its toolbar: scrolling closes it.
            term.more = None;
            let (offset, max) = term.scroll.get();
            // Stepped on the frame clock (`App::step_terminal_glides`): Rune lands scroll
            // requests at once while the composer holds focus.
            let base = term.glide.map_or(offset, |g| g.target);
            let target = (base - e.dy / NOTCH * 3.0 * line_h).clamp(0.0, max);
            term.glide = Some(crate::ui::follow::Glide::retarget(term.glide, offset, target));
            tx.wake_after(std::time::Duration::ZERO);
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
    // Collapsed or filtered blocks show only the rows they keep (`BlockTerminalView` layout).
    let filtering = term.filter.as_ref().filter(|(id, _)| *id == block.id).map(|(_, query)| query);
    let reduced = term.collapsed.contains(&block.id) || filtering.is_some_and(|q| !q.text.is_empty());
    let shown: Vec<Row> = if reduced { term.visible_lines(block).into_iter().cloned().collect() } else { Vec::new() };
    let lines: &[Row] = if reduced { &shown } else { &block.lines };
    let filter_h = if filtering.is_some() { FILTER_ROW * s } else { 0.0 };
    let output = row_h * (1.1 + 0.9 + 0.19 + command.len() as f32 + if lines.is_empty() { 0.0 } else { 0.5 }) + filter_h;
    // A truncated block says so in one muted row above the rows it kept.
    let note = if block.truncated && !reduced { row_h } else { 0.0 };
    let height = output + note + lines.len() as f32 * row_h + row_h * if block.running() { 0.2 } else { 1.0 };
    let failed = block.failed();
    let selected = term.selected == Some(block.id);
    let block_id = block.id;
    let tab_id = app.active_tab().id;
    let tint = (failed || selected).then_some(if failed { p.red } else { p.accent });
    // Tinted blocks wash the whole block and carry a 2 pt bar on the left.
    let wash = |el: El, h: f32| match tint {
        Some(tint) => el.child(el_fill(rgb(tint).alpha(0.06), h).key("wash")).child(el_bar(rgb(tint), 2.0 * s, h).key("bar")),
        None => el,
    };
    // The header (directory, status, command) sticks to the top while its output scrolls
    // under it, as GPUiX's sticky header did; the block's end pushes it out.
    let label = if block.background { "Shell output".to_owned() } else { directory_label(&block.cwd, home) };
    let mut header = wash(el().w_full().h(output).fill(rgb(p.bg)), output).sticky_top(0.0)
        .child(text(label).key("cwd").font(ink(app, 0.9, None)).color(rgb(p.dim)).absolute(cells.inset, row_h * 1.1));
    // The status gives way to the block's actions while it is hovered, selected, filtered or
    // its menu is open.
    let engaged = [term.hovered, term.selected, term.more, term.filter.as_ref().map(|(id, _)| *id)].contains(&Some(block.id));
    if !block.background && !engaged && width > 260.0 * s {
        let ms = block.elapsed_ms.unwrap_or_else(|| block.started.elapsed().as_millis() / 1000 * 1000);
        let duration = duration_label(ms);
        let (status, color) = if block.running() {
            (format!("Running  {duration}"), p.accent)
        } else if let Some(code) = block.exit.filter(|c| *c != 0) {
            (format!("exit {code}  {duration}"), p.red)
        } else {
            (duration, p.muted)
        };
        header = header.child(text(status).key("status").font(ink(app, 0.9, None)).color(rgb(color))
            .pin(None, Some(row_h * 1.1), Some(cells.inset), None));
    }
    for (index, line) in command.iter().enumerate() {
        header = header.child(text((*line).to_owned()).key(format!("command:{index}")).selectable(true).font(ink(app, 1.0, None)).color(rgb(p.fg))
            .absolute(cells.inset, row_h * (2.19 + index as f32)));
    }
    if !block.background && term.toolbar_block() == Some(block.id) {
        header = header.child(actions(app, term, block, home, width).key("actions"));
    }
    if let Some(query) = filtering {
        header = header.child(filter_row(app, tab_id, block_id, query, lines.len(), block.lines.len()).key("filter")
            .pin(Some(cells.inset), Some(output - FILTER_ROW * s), Some(cells.inset), None));
    }
    let body_h = height - output;
    let mut body = wash(el().w_full().h(body_h), body_h);
    if block.truncated && !reduced {
        body = body.child(text(format!("Earlier output truncated; showing the last {} lines", block.lines.len()))
            .key("truncated").font(ink(app, 0.85, None)).color(rgb(p.muted)).absolute(cells.inset, 0.0));
    }
    if !lines.is_empty() {
        body = body.child(rows_view(app, lines, cells, lift_for(p), true).key("rows").absolute(cells.inset, note));
    }
    if let (Some((row, col)), true, false) = (block.cursor, block.running() && term.long_running(), reduced) {
        body = body.child(el().key("cursor").w(cells.cell).h(row_h).fill(rgb(p.accent).alpha(0.45))
            .absolute(cells.inset + col as f32 * cells.cell, note + row as f32 * row_h));
    }
    col().key(format!("terminal-block:{}", block.id)).w_full().h(height)
        .on_hover_change(move |a: &mut App, inside: bool, _: &mut Tx| {
            if let Some(term) = a.terminals.get_mut(&tab_id) {
                if inside { term.hovered = Some(block_id); } else if term.hovered == Some(block_id) { term.hovered = None; }
            }
        })
        .on_press(move |a: &mut App| {
            a.select_tab(tab_id);
            if let Some(term) = a.terminals.get_mut(&tab_id) {
                // A press outside an open menu only dismisses it.
                if term.more.take().is_some() { return; }
                term.selected = if term.selected == Some(block_id) { None } else { Some(block_id) };
            }
        })
        .child(header)
        .child(body)
}

/// A `TerminalAction`: the icon at 60% (95% hovered or active) over a faint 5 pt wash when
/// active or hovered, in the accent when active unless the action names its own colour.
pub(super) fn action_button(app: &App, key: String, icon: Icon, active: bool, color: Option<u32>) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let ink = color.unwrap_or(if active { p.accent } else { p.fg });
    el().key(key).w(ACTION * s).h(ACTION * s).radius(5.0 * s).center()
        .fill(if active { rgb(p.fg).alpha(0.085 * 0.5) } else { Color::TRANSPARENT })
        .hover(rgb(p.fg).alpha(0.085))
        .child(crate::ui::feather::view(app, icon, 16.0 * s, ink).opacity(if active { 0.95 } else { 0.6 }))
}

/// The block's toolbar (`terminal-block-actions`): Filter output and Block actions, 16 pt from
/// the right and 8 pt into the header, backed by the page when the header text runs under it.
fn actions(app: &App, term: &Terminal, block: &Block, home: &str, width: f32) -> El {
    let s = app.ty.scale();
    let cells = Cells::of(app);
    let tab_id = app.active_tab().id;
    let id = block.id;
    let header_w = directory_label(&block.cwd, home).chars().count() as f32 * cells.cell * 0.9 + cells.inset;
    let filtering = term.filter.as_ref().is_some_and(|(f, _)| *f == id);
    row().key(format!("block-actions:{tab_id}:{id}")).gap(4.0 * s).padding(4.0 * s).radius(4.0 * s)
        .on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(format!("block-actions:{tab_id}:{id}"), r); })
        .fill(if header_w > width - 76.0 * s { rgb(app.pal.bg) } else { Color::TRANSPARENT })
        // Its menu is anchored here: once the toolbar scrolls out of view the menu closes rather
        // than losing its anchor.
        .on_visible(move |a: &mut App, visible: bool, _: &mut Tx| {
            if visible { return; }
            if let Some(term) = a.terminals.get_mut(&tab_id).filter(|t| t.more == Some(id)) { term.more = None; }
        })
        .child(action_button(app, format!("block-filter:{tab_id}:{id}"), Icon::Filter, filtering, None)
            .on_press(move |a: &mut App| a.toggle_block_filter(tab_id, id)))
        .child(action_button(app, format!("block-more:{tab_id}:{id}"), Icon::More, term.more == Some(id), None)
            .on_press(move |a: &mut App| {
                if let Some(term) = a.terminals.get_mut(&tab_id) { term.more = if term.more == Some(id) { None } else { Some(id) }; }
            }))
        .pin(None, Some(8.0 * s), Some(16.0 * s), None)
}

/// `block-output-filter`: the query field and the "shown / total lines" count.
fn filter_row(app: &App, tab_id: u64, block_id: u64, query: &rune::TextEdit, shown: usize, total: usize) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let input = rune::text_input(query, app.ty.sized(12.0))
        .key(format!("block-filter-query:{tab_id}:{block_id}"))
        // Only the window's block cursor marks the caret, as in the composer.
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Filter output...", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(move |a: &mut App| {
            let term = a.terminals.get_mut(&tab_id).expect("filtered terminal");
            &mut term.filter.as_mut().expect("filter open").1
        })
        .on_input_key(move |a: &mut App, e: &rune::KeyEvent, _: &mut Tx| {
            if e.key == rune::Key::Escape { a.close_block_filter(tab_id); return true; }
            false
        });
    row().key(format!("block-output-filter:{tab_id}:{block_id}")).h(28.0 * s).gap(10.0 * s).center_y().fill(rgb(p.bg))
        .child(el().grow(1.0).shrink().h(26.0 * s).pl(7.0 * s).center_y().radius(4.0 * s)
            .fill(rgb(p.surface)).border(1.0, rgb(p.border)).child(input))
        .child(text(format!("{shown} / {total} lines")).font(app.ty.sized(10.0)).color(rgb(p.muted)).w(90.0 * s))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BlockAction { CopyCommand, CopyOutput, CopyAll, Reuse, Collapse }

/// `block-action-menu`: 190 pt under the toolbar, 30 pt rows of 11 pt text.
fn block_menu(app: &App, term: &Terminal) -> Option<El> {
    let id = term.more?;
    term.shell.blocks.iter().chain(term.shell.live.iter()).find(|b| b.id == id)?;
    let p = app.pal;
    let s = app.ty.scale();
    let tab_id = app.active_tab().id;
    let items = [
        ("Copy command", BlockAction::CopyCommand),
        ("Copy output", BlockAction::CopyOutput),
        ("Copy command and output", BlockAction::CopyAll),
        ("Use command", BlockAction::Reuse),
        (if term.collapsed.contains(&id) { "Expand output" } else { "Collapse output" }, BlockAction::Collapse),
    ];
    // Right-aligned 2 pt under the block's toolbar (its rect from the last layout), above it
    // when the window has no room below.
    let anchor = app.rects.get(&format!("block-actions:{tab_id}:{id}")).copied()?;
    let (w, h) = (190.0 * s, items.len() as f32 * 30.0 * s + 8.0 * s);
    let x = (anchor[0] + anchor[2] - w).clamp(8.0, (app.window[0] - w - 8.0).max(8.0));
    let below = anchor[1] + anchor[3] + 2.0 * s;
    let y = if below + h <= app.window[1] - 8.0 { below } else { (anchor[1] - 2.0 * s - h).max(8.0) };
    let mut menu = col().key(format!("block-action-menu:{tab_id}:{id}")).w(w).padding(4.0 * s)
        .fill(rgb(p.surface)).border(1.0, rgb(p.border)).radius(5.0 * s)
        .absolute(x, y);
    for (index, (label, action)) in items.into_iter().enumerate() {
        menu = menu.child(row().key(format!("block-menu:{tab_id}:{id}:{index}")).w_full().h(30.0 * s).pl(8.0 * s).center_y()
            .radius(3.0 * s).hover(rgb(p.hover))
            .child(text(label).font(app.ty.sized(11.0)).color(rgb(p.fg)))
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.block_action(tab_id, id, action, tx)));
    }
    Some(menu)
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
pub fn rows_view(app: &App, rows: &[Row], cells: Cells, lift: impl Fn(u32) -> u32, selectable: bool) -> El {
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
    // Screen content lands at once: scrolling a program redraws rows, and nothing in them may
    // glide or fade between frames. Backgrounds are one canvas, so no cell is an element that
    // could re-pair with another's and tween its place or colour.
    let height = rows.len() as f32 * cells.row;
    let mut layer = el().key("grid-rows").w_full().h(height).instant_changes();
    if !fills.is_empty() {
        let width = fills.iter().map(|(col, _, w, _)| (col + w) as f32 * cells.cell).fold(cells.cell, f32::max);
        let mut shape = rune::Shape::new().view_box(width, height.max(cells.row));
        for (col, y, w, bg) in fills {
            let (x0, y0) = (col as f32 * cells.cell, y as f32 * cells.row);
            let (x1, y1) = (x0 + w as f32 * cells.cell, y0 + cells.row);
            shape = shape.fill(vec![[x0, y0], [x1, y0], [x1, y1], [x0, y1]], rgb(bg));
        }
        layer = layer.child(rune::canvas(shape).key("grid-fills").w(width).h(height.max(cells.row)).absolute(0.0, 0.0));
    }
    layer.child(text(body).key("grid-text").selectable(selectable).spans(spans).font(ink(app, 1.0, None)).color(rgb(app.pal.fg)).absolute(0.0, 0.0))
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
    area = area.child(wordmark::view(app, &wordmark::host_key("splash", tab), w).absolute(inset_x + x, inset_y + y)
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
    let tab_id = tab.id;
    let input = rune::text_input(draft, ink(app, 1.0, None))
        .key(format!("terminal-command:{}", tab.id))
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Enter a command", rgb(p.muted))
        .color(rgb(p.fg)).w_full()
        .on_edit(move |a: &mut App| { a.select_tab(tab_id); let i = a.active; &mut a.tabs[i].draft })
        .on_input_change(move |a: &mut App, _: &mut Tx| { a.select_tab(tab_id); a.composer_edited(); })
        .on_input_key(move |a: &mut App, e: &rune::KeyEvent, tx: &mut Tx| { a.select_tab(tab_id); a.terminal_composer_key(e, tx) })
        .on_submit(move |a: &mut App, _: &mut Tx| { a.select_tab(tab_id); a.submit(); });
    let line_top = 28.0 * s + cells.row * 0.9;
    let mut card = el().key("terminal-block-input").w_full().h(height)
        .border_sides([1.0, 0.0, 0.0, 0.0], rgb(p.fg).alpha(0.1))
        .opacity(if term.shell.busy() { 0.6 } else { 1.0 })
        .child(text(app.tab_cwd(tab)).font(ink(app, 0.9, None)).color(rgb(p.dim)).absolute(cells.inset, 13.0 * s))
        .child({
            // `terminal-directory-authority`: filled yellow on the terminal that owns the
            // working directory; elsewhere, an outline that hands ownership to this one.
            let owner = app.main_terminal_tab() == Some(tab_id);
            let icon = if owner { Icon::StarFilled } else { Icon::Star };
            action_button(app, format!("terminal-directory-authority:{tab_id}"), icon, owner, owner.then_some(p.yellow))
                .on_press(move |a: &mut App| { a.select_tab(tab_id); a.set_main_terminal(tab_id); })
                .pin(None, Some(9.0 * s), Some(12.0 * s), None)
        })
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
