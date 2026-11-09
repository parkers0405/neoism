//! The floating composer card, its chip row and path bar (`reference/09`), and the slash,
//! model and agent menus that open above it (`reference/10..13`).

use crate::app::{App, Overlay};
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
