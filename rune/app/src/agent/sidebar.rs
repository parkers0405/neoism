//! `Alt+C`: the conversations sidebar (`reference/04-sidebar-conversations.png`).

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::icon;
use crate::ui::theme::{glyph, rgb};
use neoism_protocol::agent::ThreadSummary;
use rune::{Color, El, ScrollSpec, col, el, row, text};

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
