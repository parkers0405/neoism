//! The inline picker card (`widgets/inline_picker.rs`) and the skit layer. The card locks to
//! the composer card's width and x and sits 6 pt above it: a 54 pt header (bold 13 pt title,
//! `esc`, the query row), then up to eight 46 pt rows (bold 14 pt title, 12 pt description,
//! 11 pt footer on the right), the selected one on a hover fill with an accent bar.

use super::effects::{self, Op};
use super::logos;
use super::picker::{Icon, Kind, MAX_ROWS, Picker, Status};
use crate::app::App;
use crate::ui::theme::{NERD, rgb};
use rune::tok::TextStyle;
use rune::{Color, El, Shape, Tx, WheelEvent, canvas, el, text};

const TITLE_H: f32 = 54.0;
const ROW_H: f32 = 46.0;
const RADIUS: f32 = 14.0;
/// Geist Mono advances 0.6 em; used to wrap activity text and right-align footers.
const ADVANCE: f32 = 0.6;

fn width_of(text: &str, size: f32) -> f32 { text.chars().count() as f32 * size * ADVANCE }

fn mono(size: f32, bold: bool) -> TextStyle { TextStyle::new(size, if bold { 700.0 } else { 400.0 }).mono() }

/// Greedy word wrap at `width` points.
fn wrap(text: &str, width: f32, size: f32) -> Vec<String> {
    let max = ((width / (size * ADVANCE)).floor() as usize).max(1);
    let mut lines = Vec::new();
    for paragraph in text.lines() {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            if !line.is_empty() && line.chars().count() + 1 + word.chars().count() > max {
                lines.push(std::mem::take(&mut line));
            }
            if !line.is_empty() { line.push(' '); }
            line.push_str(word);
        }
        lines.push(line);
    }
    lines
}

/// Card height pieces for `picker` in a `width` point card.
struct Layout {
    header_h: f32,
    activity: Vec<String>,
    visible: usize,
}

fn layout(picker: &Picker, width: f32, s: f32) -> Layout {
    let rows = picker.rows().len();
    let reserve_empty = picker.loading || (picker.activity.is_none() && !picker.kind.is_entry());
    let visible = if rows == 0 && !reserve_empty { 0 } else { rows.min(picker.limit.get().clamp(1, MAX_ROWS)).max(1) };
    let (header_h, activity) = match &picker.activity {
        Some(a) => {
            let lines = wrap(&format!("{}\n{}", a.body, a.status), width - 48.0 * s, 13.0 * s);
            let lines = lines.into_iter().rev().take(12).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>();
            let h = (36.0 + lines.len() as f32 * 18.0) * s + if a.show_input { 22.0 * s } else { 0.0 } + if a.url.is_some() { 24.0 * s } else { 0.0 };
            (h, lines)
        }
        None => (TITLE_H * s, Vec::new()),
    };
    Layout { header_h, activity, visible }
}

/// The picker card, placed in window coordinates over the composer card.
pub fn picker_card(app: &App) -> El {
    let (Some(picker), Some(&composer)) = (app.composer.picker.as_ref(), app.rects.get("composer-card")) else { return el() };
    let p = app.pal;
    let s = app.ty.scale();
    let [cx, cy, cw, _] = composer;
    // As many rows as fit between the window top (8 pt) and 6 pt above the composer.
    let fit = (((cy - 8.0 * s - 6.0 * s - TITLE_H * s) / (ROW_H * s)).floor().max(1.0) as usize).min(MAX_ROWS);
    picker.limit.set(fit);
    let Layout { header_h, activity, visible } = layout(picker, cw, s);
    let height = header_h + visible as f32 * ROW_H * s;
    let y = (cy - height - 6.0 * s).max(8.0 * s);

    let mut card = el().key("prompt-picker").w(cw).h(height).fill(rgb(p.bg)).radius(RADIUS * s).border(s, rgb(p.border))
        .clip_rounded(RADIUS * s)
        .on_press(|_: &mut App| {})
        .on_wheel(|a: &mut App, e: WheelEvent, _: &mut Tx| {
            if let Some(picker) = a.composer.picker.as_mut() { picker.scroll(if e.dy < 0.0 { 1 } else { -1 }); }
            true
        });

    // Header: title and esc; then the query row, or the activity in its place.
    card = card
        .child(text(picker.title.clone()).font(mono(13.0 * s, true)).color(rgb(p.fg)).truncate().w(cw - 62.0 * s).absolute(14.0 * s, 9.0 * s))
        .child(text("esc").font(mono(12.0 * s, false)).color(rgb(p.muted)).absolute(cw - 14.0 * s - width_of("esc", 12.0 * s), 9.0 * s)
            .on_press(|a: &mut App| { if !a.picker_back() { a.close_prompt_picker(); } }));
    if let Some(a) = &picker.activity {
        let x = if a.busy { 32.0 } else { 14.0 } * s;
        for (i, line) in activity.iter().enumerate() {
            card = card.child(text(line.clone()).font(mono(13.0 * s, false)).color(rgb(p.fg)).absolute(x, (31.0 + i as f32 * 18.0) * s));
        }
        if a.busy {
            card = card.child(el().w(4.0 * s).h(4.0 * s).radius(s).fill(rgb(p.fg)).absolute(17.0 * s, 35.5 * s));
        }
        if let Some(url) = a.url.clone() {
            let link_y = header_h - if a.show_input { 22.0 * s } else { 0.0 } - 24.0 * s;
            card = card.child(text("Open sign-in page").font(mono(12.0 * s, false)).color(rgb(p.accent)).absolute(14.0 * s, link_y + 3.0 * s)
                .on_press(move |_: &mut App| { let _ = std::process::Command::new("xdg-open").arg(&url).spawn(); }));
        }
    }
    if picker.activity.as_ref().is_none_or(|a| a.show_input) {
        let query_y = if picker.activity.is_some() { header_h - 22.0 * s } else { 31.0 * s };
        card = card.child(query_row(app, picker, cw - 28.0 * s).absolute(14.0 * s, query_y));
    }

    // Rows.
    let rows = picker.rows();
    let grouped = rows.iter().any(|r| r.header && r.icon.is_some());
    let first = picker.top.min(rows.len().saturating_sub(visible));
    for (slot, (index, row)) in rows.iter().enumerate().skip(first).take(visible).enumerate() {
        let row_y = header_h + slot as f32 * ROW_H * s;
        let mut item = el().key(format!("picker-row:{index}")).w(cw).h(ROW_H * s).absolute(0.0, row_y);
        if row.header {
            if !row.title.is_empty() {
                let x = if row.icon.is_some() { 48.0 } else { 22.0 } * s;
                item = item.child(text(row.title.clone()).font(mono(13.5 * s, true)).color(rgb(p.cyan)).truncate().w(cw - x - 22.0 * s).absolute(x, 16.0 * s));
            }
            card = card.child(item);
            continue;
        }
        if index == picker.selected {
            item = item
                .child(el().w(cw - 12.0 * s).h(ROW_H * s - 6.0 * s).radius(9.0 * s).fill(rgb(p.hover)).absolute(6.0 * s, 3.0 * s))
                .child(el().w(3.0 * s).h(ROW_H * s - 18.0 * s).radius(2.0 * s).fill(rgb(p.accent)).absolute(10.0 * s, 9.0 * s));
        }
        if row.current {
            item = item.child(text("\u{2713}").font(mono(11.0 * s, false)).color(rgb(p.accent)).absolute(cw - 30.0 * s, 15.0 * s));
        }
        if let Some(icon) = row.icon {
            item = item.child(row_icon(app, icon, 18.0 * s).absolute(22.0 * s, (ROW_H - 18.0) * 0.5 * s));
        }
        let title_x = if row.icon.is_some() || grouped { 48.0 } else { 22.0 } * s;
        let footer_w = if row.footer.is_empty() { 0.0 } else { width_of(&row.footer, 11.0 * s) + 22.0 * s + if row.status.is_some() { 12.0 * s } else { 0.0 } };
        let footer_x = cw - footer_w - if row.current { 24.0 * s } else { 0.0 };
        let title_y = if row.description.is_empty() { 13.0 } else { 5.0 } * s;
        item = item.child(text(row.title.clone()).font(mono(14.0 * s, true)).color(rgb(p.fg)).truncate()
            .max_w((footer_x - title_x - 14.0 * s).max(48.0)).absolute(title_x, title_y));
        if !row.description.is_empty() {
            item = item.child(text(row.description.clone()).font(mono(12.0 * s, false)).color(rgb(p.dim)).truncate()
                .max_w((footer_x - title_x - 12.0 * s).max(48.0)).absolute(title_x, 25.0 * s));
        }
        if !row.footer.is_empty() {
            if let Some(status) = row.status {
                let color = match status { Status::Good => p.green, Status::Warning => p.yellow, Status::Error => p.red, Status::Muted => p.muted };
                item = item.child(el().w(6.0 * s).h(6.0 * s).radius(3.0 * s).fill(rgb(color)).absolute(footer_x - 12.0 * s, (ROW_H - 6.0) * 0.5 * s));
            }
            item = item.child(text(row.footer.clone()).font(mono(11.0 * s, false)).color(rgb(p.muted)).absolute(footer_x, 15.0 * s));
        }
        let pick = index;
        card = card.child(item
            // Only a moving pointer selects (`onMouseMove`): a row appearing under a resting
            // pointer must not steal the keyboard selection.
            .on_pointer_move(move |a: &mut App, _| {
                if let Some(picker) = a.composer.picker.as_mut().filter(|p| p.selected != pick) { picker.selected = pick; }
            })
            .on_press_tx(move |a: &mut App, tx: &mut Tx| a.accept_picker_row(Some(pick), tx)));
    }
    if rows.is_empty() && visible > 0 {
        if picker.loading {
            for i in 0..visible {
                let w = (cw - 44.0 * s) * [0.42, 0.6, 0.35][i % 3];
                card = card.child(el().w(w).h(10.0 * s).radius(5.0 * s).fill(rgb(p.surface)).absolute(22.0 * s, header_h + i as f32 * ROW_H * s + 18.0 * s));
            }
        } else if picker.activity.is_none() {
            card = card.child(text("No results").font(mono(13.0 * s, false)).color(rgb(p.muted)).absolute(22.0 * s, header_h + (ROW_H - 14.0) * 0.5 * s));
        }
    }

    let scrim = el().key("prompt-picker-scrim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .on_press(|a: &mut App| a.close_prompt_picker());
    let layer = el().key("prompt-picker-layer").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0));
    let layer = if picker.kind == Kind::Slash { layer } else { layer.child(scrim) };
    layer.child(card.absolute(cx, y))
}

/// The header's query row: the composer's own text for `/`, else this picker's field.
fn query_row(app: &App, picker: &Picker, width: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    if picker.kind == Kind::Slash {
        let query = app.active_tab().draft.text.trim_start_matches('/').to_owned();
        return if query.is_empty() {
            text(picker.placeholder.clone()).font(mono(13.0 * s, false)).color(rgb(p.muted)).ml(5.0 * s)
        } else {
            text(query).font(mono(13.0 * s, false)).color(rgb(p.fg))
        };
    }
    let mut input = rune::text_input(&picker.query, mono(13.0 * s, false))
        .key("prompt-picker-query")
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder(picker.placeholder.clone(), rgb(p.muted))
        .color(rgb(p.fg)).w(width)
        .on_edit(|a: &mut App| &mut a.composer.picker.as_mut().expect("picker open").query)
        .on_input_change(|a: &mut App, _: &mut Tx| {
            if let Some(picker) = a.composer.picker.as_mut() { let q = picker.query.text.clone(); picker.set_query(&q); }
            a.directory_query_changed();
        })
        .on_input_key(|a: &mut App, e: &rune::KeyEvent, tx: &mut Tx| a.picker_key(e, tx))
        .on_submit(|a: &mut App, tx: &mut Tx| a.accept_picker_row(None, tx));
    if picker.kind == Kind::ConnectSecret { input = input.secure(); }
    input
}

/// The selected model's provider mark, for the composer's model chip (`ProviderLogo`).
pub fn provider_mark(app: &App, size: f32) -> El {
    let provider = app.composer.selection.model.split_once('/').map(|(p, _)| p).unwrap_or_default();
    match super::catalog::provider_logo(provider) {
        Some(index) => row_icon(app, Icon::Provider(index), size),
        None => el(),
    }
}

fn row_icon(app: &App, icon: Icon, size: f32) -> El {
    match icon {
        Icon::Provider(index) => {
            let image = app.images.as_ref().and_then(|images| app.composer.logos.image(images, index, logos::color(index, app.pal)));
            match image {
                Some(image) => el().w(size).h(size).pixels(image.pixels()),
                None => el().w(size).h(size),
            }
        }
        // The `+` mark: two bars, two thirds of the box long, a ninth thick.
        Icon::Glyph("+") => {
            let (length, stroke) = (size * 2.0 / 3.0, size / 9.0);
            el().w(size).h(size)
                .child(el().w(length).h(stroke).fill(rgb(app.pal.fg)).absolute((size - length) / 2.0, (size - stroke) / 2.0))
                .child(el().w(stroke).h(length).fill(rgb(app.pal.fg)).absolute((size - stroke) / 2.0, (size - length) / 2.0))
        }
        Icon::Glyph(glyph) => text(glyph).font(TextStyle::new(size, 400.0).family(NERD)).color(rgb(app.pal.fg)),
    }
}

fn linear(c: [f32; 4]) -> Color {
    let f = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    Color { r: f(c[0]), g: f(c[1]), b: f(c[2]), a: c[3] }
}

/// The playing skit over the transcript, above the composer.
pub fn effect_layer(app: &App) -> El {
    let (Some(running), Some(&pane), Some(&composer)) = (app.composer.effect.as_ref(), app.rects.get("agent-pane"), app.rects.get("composer-card")) else { return el() };
    let s = app.ty.scale();
    let rect = effects::scene_rect(pane, composer[1], s);
    let frame = effects::draw(running.effect, rect, running.elapsed(), s, &effects::Theme::new(app.pal));
    let [ox, oy, w, h] = rect;
    let mut shape = Shape::new().view_box(w, h);
    let (mut under, mut over) = (Vec::new(), Vec::new());
    for op in frame.ops {
        match op {
            Op::Rect([x, y, rw, rh], color) => {
                let (x, y) = (x - ox, y - oy);
                shape = shape.fill(vec![[x, y], [x + rw, y], [x + rw, y + rh], [x, y + rh]], linear(color));
            }
            Op::Rounded([x, y, rw, rh], radius, color) => {
                under.push(el().w(rw).h(rh).radius(radius).fill(linear(color)).absolute(x - ox, y - oy));
            }
            Op::Text { at: [x, y], text: t, size, color } => {
                over.push(text(t).font(mono(size, true)).color(linear(color)).absolute(x - ox, y - oy));
            }
        }
    }
    // Rounded props (throne, puddle, bubble) under the pixels; notes and grawlix over them.
    let mut layer = el().key("slash-effect").w(w).h(h).clip_rounded(0.0).absolute(ox, oy);
    for prop in under { layer = layer.child(prop); }
    layer = layer.child(canvas(shape).w(w).h(h).absolute(0.0, 0.0));
    for label in over { layer = layer.child(label); }
    layer
}
