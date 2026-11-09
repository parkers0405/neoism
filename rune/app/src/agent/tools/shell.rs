//! A command the user ran from the composer (`user_shell`, `tool_message/shell.rs`): hairlines
//! above and below, the directory (muted) and exit status on one line, the command under it, then
//! the output, sixteen rows tall, scrolling past that.

use super::Cx;
use super::text::{Text, fit, width};
use neoism_protocol::agent::HistoryMessage;
use rune::{El, ScrollSpec, col, el};

const HEADER: f32 = 58.0;
const ROW: f32 = 20.0;
const VISIBLE_ROWS: usize = 16;

pub fn card(cx: &Cx, m: &HistoryMessage) -> (El, f32) {
    let p = cx.p;
    let w = cx.w;
    let size = cx.pt(13.0);
    let inner_w = (w - cx.pt(32.0)).max(1.0);
    let cols = ((inner_w / (size * super::text::ADVANCE)).floor() as usize).max(1);
    let rows: Vec<String> = m.detail.split('\n').flat_map(|line| {
        let chars: Vec<char> = line.trim_end_matches('\r').replace('\t', "    ").chars().collect();
        if chars.is_empty() { vec![String::new()] } else { chars.chunks(cols).map(|c| c.iter().collect()).collect() }
    }).collect();
    let h = cx.pt(HEADER + 16.0) + cx.pt(ROW) * rows.len().clamp(1, VISIBLE_ROWS) as f32;
    let accent = match m.status.as_str() { "error" | "interrupted" => p.red, "completed" => p.muted, _ => p.accent };
    let rule = cx.color(p.fg).alpha(0.1);
    let mut out = el().w(w).h(h)
        .child(el().w(w).h(cx.s).fill(rule).absolute(0.0, 0.0))
        .child(el().w(w).h(cx.s).fill(rule).absolute(0.0, h - cx.s));
    let (directory, status) = m.text.rsplit_once("  |  ").unwrap_or((m.text.as_str(), ""));
    let status_size = cx.pt(11.7);
    let status_w = width(status, status_size);
    let show_status = status_w + cx.pt(48.0) < inner_w;
    let dir_budget = (inner_w - if show_status { status_w + cx.pt(16.0) } else { 0.0 }).max(0.0);
    out = out.child(Text::new(fit(directory, dir_budget, status_size), status_size, cx.app).color(cx.color(p.muted)).at(cx.pt(16.0), cx.pt(13.0)));
    if show_status {
        out = out.child(Text::new(status, status_size, cx.app).color(cx.color(accent)).at(w - cx.pt(16.0) - status_w, cx.pt(13.0)));
    }
    let command = fit(&m.title.replace('\n', " "), inner_w, size);
    out = out.child(Text::new(command, size, cx.app).color(cx.color(p.fg)).at(cx.pt(16.0), cx.pt(34.0)));
    let body_h = h - cx.pt(HEADER + 16.0);
    let mut lines = el().w(inner_w).h(rows.len() as f32 * cx.pt(ROW));
    for (i, row) in rows.into_iter().enumerate() {
        lines = lines.child(Text::new(row, size, cx.app).color(cx.color(p.fg)).at(0.0, i as f32 * cx.pt(ROW)));
    }
    out = out.child(col().w(inner_w).h(body_h).absolute(cx.pt(16.0), cx.pt(HEADER + 8.0))
        .scroll(ScrollSpec { stick_bottom: true })
        .child(lines));
    (out, h)
}
