//! The context panel above the composer, opened from the token count under it: how full the
//! model's context window is, what the last turn spent, and a way to compact.
use super::{Kind, Picker};
use crate::{app::App, tabs::TabKind, ui::theme::rgb};
use rune::{El, Tx, col, el, row, text};

fn count(n: u64) -> String {
    match n {
        n if n >= 1_000_000 => format!("{:.2}m", n as f64 / 1_000_000.0),
        n if n >= 10_000 => format!("{:.1}k", n as f64 / 1000.0),
        n => n.to_string(),
    }
}

pub(super) fn view(app: &App, composer: [f32; 4]) -> El {
    let (p, s) = (app.pal, app.ty.scale());
    let cw = composer[2];
    let session = match &app.active_tab().kind { TabKind::Agent { session: Some(id) } => app.sessions.get(id), _ => None };
    let usage = session.and_then(|session| session.messages.iter().rev().filter_map(|m| m.usage.as_ref()).find(|u| u.input > 0 || u.output > 0 || u.total > 0));
    let mut content = col().w((cw - 2.0 * s).max(0.0)).px(16.0 * s).py(12.0 * s).gap(12.0 * s);
    let Some(usage) = usage else {
        let content = content.child(text("No usage yet. It appears after the first reply.").font(app.ty.sans(13.0, 400.0)).color(rgb(p.muted)).w_full().wrap());
        return super::panel::view(app, composer, "context", "Context".to_owned(), "Esc close", content);
    };
    let total = if usage.total > 0 { usage.total } else { usage.input + usage.output };
    let model = app.composer.selection.model.to_lowercase();
    let limit = session.and_then(|s| s.context_limit).or_else(|| app.composer.context_limit(&model)).or(usage.context_limit.filter(|l| *l > 0));
    let share = limit.map(|limit| (total as f32 / limit.max(1) as f32).clamp(0.0, 1.0));
    // The meter: green, then yellow from 70%, red from 90%.
    let tone = match share { Some(x) if x >= 0.9 => p.red, Some(x) if x >= 0.7 => p.yellow, _ => p.green };
    let track_w = (cw - 34.0 * s).max(10.0);
    content = content.child(row().w_full().items_end().gap(8.0 * s)
        .child(text(count(total)).font(app.ty.sans(24.0, 600.0)).color(rgb(p.fg)))
        .child(text(match limit { Some(limit) => format!("of {} tokens", count(limit)), None => "tokens".to_owned() }).font(app.ty.sans(13.0, 400.0)).color(rgb(p.muted)).mb(3.0 * s))
        .child(el().grow(1.0))
        .children(share.map(|x| text(format!("{}%", (x * 100.0).round() as u32)).font(app.ty.sans(15.0, 600.0)).color(rgb(tone)).mb(2.0 * s))));
    if let Some(x) = share {
        content = content.child(el().w(track_w).h(6.0 * s).radius(3.0 * s).fill(rgb(p.fg).alpha(0.1))
            .child(el().w((track_w * x).max(6.0 * s)).h(6.0 * s).radius(3.0 * s).fill(rgb(tone))));
    }
    let line = |label: &'static str, value: u64| row().w_full().h(22.0 * s).center_y()
        .child(text(label).font(app.ty.sans(13.0, 400.0)).color(rgb(p.dim)).grow(1.0))
        .child(text(count(value)).font(app.ty.sized(12.5)).color(rgb(p.fg)));
    let mut rows = col().w_full().gap(2.0 * s).child(line("Input", usage.input)).child(line("Output", usage.output));
    if usage.reasoning > 0 { rows = rows.child(line("Reasoning", usage.reasoning)); }
    if usage.cache_read > 0 { rows = rows.child(line("Read from cache", usage.cache_read)); }
    if usage.cache_write > 0 { rows = rows.child(line("Written to cache", usage.cache_write)); }
    content = content.child(text("Last turn").font(app.ty.sans(11.5, 600.0)).color(rgb(p.muted))).child(rows);
    if usage.cost_micros > 0 {
        content = content.child(row().w_full().h(22.0 * s).center_y()
            .child(text("Cost of that turn").font(app.ty.sans(13.0, 400.0)).color(rgb(p.dim)).grow(1.0))
            .child(text(format!("${:.4}", usage.cost_micros as f64 / 1_000_000.0)).font(app.ty.sized(12.5)).color(rgb(p.fg))));
    }
    content = content.child(row().w_full().pt(2.0 * s)
        .child(row().key("context-compact").h(28.0 * s).px(12.0 * s).radius(8.0 * s).center()
            .fill(rgb(p.fg).alpha(0.08)).hover(rgb(p.fg).alpha(0.14)).cursor(rune::Cursor::Pointer)
            .child(text("Compact conversation").font(app.ty.sans(12.5, 500.0)).color(rgb(p.fg)))
            .on_press_tx(|a: &mut App, tx: &mut Tx| { a.close_prompt_picker(); a.run_slash("/compact", tx); })));
    let title = if model.is_empty() { "Context".to_owned() } else { format!("Context  \u{b7}  {}", app.composer.selection.model) };
    super::panel::view(app, composer, "context", title, "Esc close", content)
}

impl App {
    /// The token count under the composer: open the context panel, or close it.
    pub fn toggle_context_panel(&mut self) {
        if self.composer.picker.as_ref().is_some_and(|p| p.kind == Kind::Context) { return self.close_prompt_picker(); }
        self.focus_picker(Picker::new(Kind::Context, "Context", Vec::new()));
    }
}
