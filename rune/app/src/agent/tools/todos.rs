//! A checklist row (`render_tool_todos`): a 1 pt rail, 15 pt outlined boxes (green `✓` when done,
//! a haloed yellow dot when in progress), and wrapped items: done muted, current bold yellow,
//! pending plain.

use super::Cx;
use super::text::{Text, wrap};
use neoism_protocol::agent::TodoItem;
use rune::{El, el};

pub const ROW_H: f32 = 28.0;

#[derive(Clone, Copy, PartialEq)]
enum State {
    Done,
    Current,
    Pending,
}

fn state(status: &str) -> State {
    match status.trim().replace('-', "_").to_ascii_lowercase().as_str() {
        "completed" | "complete" | "done" | "success" => State::Done,
        "in_progress" | "active" | "running" | "current" => State::Current,
        _ => State::Pending,
    }
}

/// The list (placed by the caller 30 pt in, 28 pt down) and how many text rows it uses.
pub fn list(cx: &Cx, todos: &[TodoItem]) -> (El, usize) {
    let p = cx.p;
    let w = cx.w - cx.pt(40.0);
    let size = cx.pt(14.0);
    let text_w = (w - cx.pt(46.0)).max(cx.pt(40.0));
    let wrapped: Vec<Vec<String>> = todos.iter().map(|t| wrap(&t.content, text_w, size, 24)).collect();
    let rows = wrapped.iter().map(Vec::len).sum::<usize>().max(1);
    let row_h = cx.pt(ROW_H);
    let mut out = el().w(w).h(rows as f32 * row_h)
        .child(el().w(cx.pt(1.0)).h(rows as f32 * row_h).fill(cx.color(p.border)).absolute(0.0, -cx.pt(4.0)));
    let mut y = 0.0;
    for (todo, lines) in todos.iter().zip(&wrapped) {
        let state = state(&todo.status);
        out = out.child(checkbox(cx, state).absolute(cx.pt(16.0), y - cx.pt(1.0)));
        let (color, bold) = match state {
            State::Done => (p.muted, false),
            State::Current => (p.yellow, true),
            State::Pending => (p.fg, false),
        };
        for line in lines {
            let mut t = Text::new(line.clone(), size, cx.app).color(cx.color(color));
            if bold { t = t.bold(); }
            out = out.child(t.at(cx.pt(46.0), y));
            y += row_h;
        }
    }
    (out, rows)
}

fn checkbox(cx: &Cx, state: State) -> El {
    let p = cx.p;
    let size = cx.pt(15.0);
    let stroke = cx.pt(1.0);
    let mut box_ = el().w(size + stroke).h(size + stroke).border(stroke, cx.color(p.muted));
    match state {
        State::Done => {
            box_ = box_.center().child(cx.text("\u{2713}", 12.0).bold().color(cx.color(p.green)).el());
        }
        State::Current => {
            let dot = size - cx.pt(9.0);
            let halo = dot * 1.65;
            let at = (size - dot) * 0.5 + cx.pt(0.5);
            box_ = box_
                .child(el().w(halo).h(halo).radius(halo / 2.0).fill(cx.color(p.yellow).alpha(0.35)).absolute(at + (dot - halo) / 2.0, at + (dot - halo) / 2.0))
                .child(el().w(dot).h(dot).radius(dot / 2.0).fill(cx.color(p.yellow)).absolute(at, at));
        }
        State::Pending => {}
    }
    box_
}
