//! The floating composer card, its chip row and path bar (`reference/09`), and the slash,
//! model and agent menus that open above it (`reference/10..13`).

use super::slash::Kind;
use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::icon;
use crate::ui::theme::{glyph, rgb};
use rune::{Color, El, Tx, col, el, row, text};

/// The floating composer card with its chip row and path bar (`reference/09`).
pub fn composer(app: &App, max_w: f32) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let streaming = matches!(&tab.kind, TabKind::Agent { session: Some(id) } if app.sessions.get(id).is_some_and(|s| s.streaming));
    let input = rune::text_input(&tab.draft, app.ty.composer())
        .key(format!("composer:{}", tab.id))
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Ask anything", rgb(p.dim))
        .color(rgb(p.fg)).w_full().min_h(26.0)
        .on_edit(|s: &mut App| { let i = s.active; &mut s.tabs[i].draft })
        .on_input_change(|s: &mut App, _: &mut Tx| s.composer_changed())
        .on_input_key(|s: &mut App, e: &rune::KeyEvent, tx: &mut Tx| s.agent_composer_key(e, tx) || crate::keys::shortcut(s, e, tx))
        .on_submit(|s: &mut App, tx: &mut Tx| { if !s.submit_command(tx) { s.submit(); } });
    // Chips and their `/model` `/agent` `/think` aliases open the same pickers.
    // In a narrow split the chips give way (labels truncate) instead of widening the card.
    let chip = |label: String, color: Color, kind: Kind| row().gap(6.0).center_y().shrink()
        .child(text(label).font(app.ty.composer()).color(color).truncate().shrink())
        .child(icon(app, glyph::CHEVRON_DOWN, 0.5, rgb(p.muted)))
        .on_press(move |s: &mut App| {
            if s.composer.picker.as_ref().is_some_and(|picker| picker.kind == kind) { s.close_prompt_picker(); } else { s.open_selection(kind); }
        });
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
    let card = col().key("agent-input-card")
        .on_layout(|s: &mut App, r: [f32; 4]| { s.rects.insert("composer-card".into(), r); }).w_full().radius(18.0).fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .px(20.0).pt(16.0).pb(12.0).gap(22.0)
        .child(stars(app))
        .child(input)
        .child(row().key("composer-chip-row").w_full().gap(26.0).center_y()
            .child(chip(app.agent.clone(), rgb(p.red), Kind::Agent))
            .child(row().gap(8.0).center_y().shrink()
                .child(super::slash::provider_mark(app, 17.0 * app.ty.scale()))
                .child(chip(app.model.clone(), rgb(p.blue), Kind::Model)))
            .child(chip(app.effort.clone(), rgb(p.magenta), Kind::Thinking))
            .child(el().grow(1.0))
            .child(icon(app, glyph::PAPERCLIP, 0.9, rgb(p.dim)))
            .child(send));
    let footer = row().key("composer-footer").grow(1.0).shrink().h(32.0).px(18.0).center_y().justify_between()
        .fill(rgb(p.surface).alpha(0.7)).corners([0.0, 0.0, 12.0, 12.0])
        .child(row().gap(10.0).center_y()
            .child(icon(app, glyph::FOLDER, 0.85, rgb(p.dim)))
            .child(text(format!("{}/", app.display_cwd())).font(app.ty.small()).color(rgb(p.fg)).truncate().shrink()).shrink())
        .child(row().gap(10.0).center_y()
            .child_opt(app.context_label().map(|label| text(label).font(app.ty.small()).color(rgb(p.muted)).mr(18.0)))
            .child(text("/").font(app.ty.small()).color(rgb(p.fg)))
            .child(text("commands").font(app.ty.small()).color(rgb(p.muted))));
    let column = col().w_full().max_w(max_w).child(card);
    if app.composer.hints_hidden { column } else { column.child(row().w_full().px(28.0).child(footer)) }
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

/// What floats over the agent pane: the picker card above the composer, and a playing skit.
pub fn overlay(app: &App) -> El {
    el().key("agent-overlay").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(super::slash::effect_layer(app))
        .child(super::slash::picker_card(app))
}

impl App {
    /// The footer's context reading (`context-usage.ts`): the newest usage with tokens, as
    /// `14.5k (2%)` against the session's window, else the catalog's, else the usage's own.
    pub fn context_label(&self) -> Option<String> {
        let TabKind::Agent { session: Some(id) } = &self.active_tab().kind else { return None };
        let session = self.sessions.get(id)?;
        let usage = session.messages.iter().rev().filter_map(|m| m.usage.as_ref()).find(|u| u.input > 0 || u.output > 0 || u.total > 0)?;
        let total = if usage.total > 0 { usage.total } else { usage.input + usage.output };
        let model = self.composer.selection.model.to_lowercase();
        let limit = session.context_limit.or_else(|| self.composer.context_limit(&model)).or(usage.context_limit.filter(|l| *l > 0));
        let count = match total {
            n if n >= 1_000_000 => format!("{:.1}m", n as f64 / 1_000_000.0),
            n if n >= 10_000 => format!("{:.1}k", n as f64 / 1000.0),
            n => n.to_string(),
        };
        Some(match limit { Some(limit) => format!("{count} ({}%)", (total as f64 / limit as f64 * 100.0).round() as u64), None => count })
    }
}

trait ChildOpt {
    fn child_opt(self, child: Option<El>) -> Self;
}

impl ChildOpt for El {
    fn child_opt(self, child: Option<El>) -> El {
        match child { Some(c) => self.child(c), None => self }
    }
}
