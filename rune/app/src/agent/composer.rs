//! The floating composer card, its chip row and path bar (`reference/09`), and the slash,
//! model and agent menus that open above it (`reference/10..13`).

use super::slash::Kind;
use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::icon;
use crate::ui::theme::{glyph, rgb};
use rune::{Color, El, Tx, col, el, row, text};

/// The agent chip's colour (`agent_chip_accent`): Build red, Plan yellow, others the accent.
fn agent_accent(app: &App, label: &str) -> u32 {
    let p = app.pal;
    if label.eq_ignore_ascii_case("build") { p.red } else if label.eq_ignore_ascii_case("plan") { p.yellow } else { p.accent }
}

impl App {
    /// Ctrl+C clears the composer, or with nothing typed stops the running chat. Esc stops
    /// at once when work is queued, otherwise twice within a second. A selection still copies.
    pub fn composer_interrupt_key(&mut self, e: &rune::KeyEvent) -> bool {
        let i = self.active;
        let session = match &self.tabs[i].kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None };
        let running = session.as_ref().is_some_and(|id| self.sessions.get(id).is_some_and(|s| s.streaming));
        if crate::keys::ctrl_letter(e) == Some('c') && !e.mods.shift {
            let draft = &self.tabs[i].draft;
            if draft.anchor != draft.focus || self.selection.copy.selected.is_some() { return false; }
            if !draft.text.is_empty() {
                self.tabs[i].draft.clear();
                self.composer_changed();
                return true;
            }
            if let (true, Some(id)) = (running, &session) { self.stop(id); return true; }
            return false;
        }
        if e.key == rune::Key::Escape && running && !e.mods.ctrl && !e.mods.alt {
            let now = std::time::Instant::now();
            let twice = self.esc_at.take().is_some_and(|at| now.duration_since(at) < std::time::Duration::from_secs(1));
            let queued = session.as_ref().is_some_and(|id| self.sessions.get(id).is_some_and(|s| s.queued_prompts > 0));
            if queued || twice {
                // Interrupt only the active run; the server hands off to the queued steer.
                if let Some(id) = &session { self.stop(id); }
            } else {
                self.esc_at = Some(now);
            }
            return true;
        }
        false
    }

    fn chip_index(kind: Kind) -> usize { match kind { Kind::Agent => 0, Kind::Model => 1, _ => 2 } }

    /// Milliseconds into a chip's scramble, while it plays.
    pub fn chip_scramble_ms(&self, kind: Kind) -> Option<f32> {
        let (_, started) = &self.chip_scramble[Self::chip_index(kind)];
        let ms = ((self.tools.clock - started) * 1000.0) as f32;
        (ms >= 0.0 && ms < crate::ui::status::SCRAMBLE_MS).then_some(ms)
    }

    /// Start a chip's scramble when its value changes (the first value appears at rest).
    /// Returns whether one is playing.
    pub fn tick_chip_scramble(&mut self) -> bool {
        let now = self.tools.clock;
        let labels = [self.agent.clone(), self.model.clone(), self.effort.clone()];
        for (slot, label) in self.chip_scramble.iter_mut().zip(labels) {
            if slot.0 != label {
                let first = slot.0.is_empty();
                *slot = (label, if first { f64::NEG_INFINITY } else { now });
            }
        }
        let moving = [Kind::Agent, Kind::Model, Kind::Thinking].iter().any(|k| self.chip_scramble_ms(*k).is_some());
        // One more tick after the last scramble ends, so the chips are drawn at rest.
        moving | std::mem::replace(&mut self.chip_scramble_moving, moving)
    }
}

#[cfg(test)]
mod interrupt_tests {
    use super::*;

    #[test]
    fn one_escape_interrupts_the_run_without_clearing_its_queued_steers_or_draft() {
        use crate::daemon::{Daemon, Target};
        use std::time::Duration;
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("ws://{}/session", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            let (tcp, _) = listener.accept().unwrap();
            tcp.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut socket = tungstenite::accept(tcp).unwrap();
            socket.read().unwrap(); // Hello
            socket.send(tungstenite::Message::text(r#"{"WorkspaceReply":{"request_id":0,"message":{"HelloAck":{}}}}"#)).unwrap();
            let request: serde_json::Value = serde_json::from_str(socket.read().unwrap().to_text().unwrap()).unwrap();
            assert_eq!(request["Agent"]["message"], serde_json::json!({"CancelInflight": {"session_id": "steer"}}));
        });
        let mut app = App::new();
        app.load_fixture("steer", "Steering", "[]");
        app.sessions.get_mut("steer").unwrap().streaming = true;
        app.apply_agent(neoism_protocol::agent::AgentServerMessage::QueueUpdate {
            session_id: "steer".into(), count: 2, preview: None, started_at: None,
        });
        app.tabs[app.active].draft.text = "An unfinished draft".into();
        let (daemon, _inbox) = Daemon::start_to(Some(Target { url, ..Default::default() }), || {});
        app.daemon = Some(daemon);
        assert!(app.composer_interrupt_key(&rune::KeyEvent { key: rune::Key::Escape, mods: Default::default() }));
        assert_eq!(app.sessions["steer"].queued_prompts, 2);
        assert_eq!(app.active_tab().draft.text, "An unfinished draft");
        server.join().unwrap();
    }
}

/// The floating composer card with its chip row and path bar (`reference/09`).
pub fn composer(app: &App, max_w: f32) -> El {
    let p = app.pal;
    let tab = app.active_tab();
    let tab_id = tab.id;
    let streaming = matches!(&tab.kind, TabKind::Agent { session: Some(id) } if app.sessions.get(id).is_some_and(|s| s.streaming));
    let input = rune::text_input(&tab.draft, app.ty.composer())
        // Keys through this field (list arrows, wheel-free navigation) commit animated: without
        // it Rune lands every change in the commit at once, cutting the slides mid-list.
        .animate_edit_layout()
        .key(format!("composer:{}", tab.id))
        .caret(Color::TRANSPARENT, crate::ui::cursor::selection(app))
        .placeholder("Ask anything", rgb(p.dim))
        .color(rgb(p.fg)).w_full().min_h(26.0)
        .on_edit(move |s: &mut App| { s.select_tab(tab_id); let i = s.active; &mut s.tabs[i].draft })
        .on_input_change(move |s: &mut App, _: &mut Tx| { s.select_tab(tab_id); s.composer_changed(); })
        .on_input_key(move |s: &mut App, e: &rune::KeyEvent, tx: &mut Tx| { s.select_tab(tab_id); s.image_preview_key(e) || s.agent_composer_key(e, tx) || s.timeline_jump_key(e, tx) || s.composer_interrupt_key(e) || crate::keys::shortcut(s, e, tx) })
        // An image on the clipboard attaches; text pastes as usual.
        .on_input_paste(move |s: &mut App, content: &rune::ClipboardContent, _: &mut Tx| { s.select_tab(tab_id); s.paste_image(content) })
        .on_submit(move |s: &mut App, tx: &mut Tx| { s.select_tab(tab_id); if !s.submit_command(tx) { s.submit(); } });
    // Chips and their `/model` `/agent` `/think` aliases open the same pickers.
    // In a narrow split the chips give way (labels truncate) instead of widening the card.
    // A chip whose value just changed scrambles into it (GPUiX's `render_scramble_text`).
    let chip = |label: String, color: Color, kind: Kind| row().gap(6.0).center_y().shrink()
        .child(match app.chip_scramble_ms(kind) {
            Some(ms) => crate::ui::status::scramble_label(app.ty.composer(), app.ty.composer().size * 0.6, &label, color, ms),
            None => text(label).font(app.ty.composer()).color(color).truncate().shrink(),
        })
        .child(icon(app, glyph::CHEVRON_DOWN, 0.5, rgb(p.muted)))
        .on_press(move |s: &mut App| {
            s.select_tab(tab_id);
            if s.composer.picker.as_ref().is_some_and(|picker| picker.kind == kind) { s.close_prompt_picker(); } else { s.open_selection(kind); }
        });
    let send = el().key("composer-send").w(36.0).h(36.0).radius(8.0).center()
        .fill(if streaming { rgb(p.fg) } else { rgb(p.muted).alpha(0.6) })
        .child(icon(app, if streaming { glyph::STOP } else { glyph::ARROW_UP }, 0.9, if streaming { rgb(p.bg) } else { rgb(p.fg) }))
        .on_press(move |s: &mut App| {
            s.select_tab(tab_id);
            let i = s.active;
            match s.tabs[i].kind.clone() {
                TabKind::Agent { session: Some(id) } if s.sessions.get(&id).is_some_and(|x| x.streaming) => s.stop(&id),
                _ => s.submit(),
            }
        });
    // The paperclip: a button the send button's size, its glyph as large as the chips' text.
    let attach = el().key("composer-attach").w(36.0).h(36.0).radius(8.0).center()
        .fill(if app.composer_attach_hover == Some(tab_id) { rgb(p.hover) } else { rgb(p.hover).alpha(0.0) })
        .child(icon(app, glyph::PAPERCLIP, 1.5, rgb(if app.composer_attach_hover == Some(tab_id) { p.fg } else { p.dim })))
        .on_hover_change(move |s: &mut App, inside: bool, _: &mut Tx| {
            if inside { s.composer_attach_hover = Some(tab_id); } else if s.composer_attach_hover == Some(tab_id) { s.composer_attach_hover = None; }
        })
        .on_press(move |s: &mut App| { s.select_tab(tab_id); s.request_attach(); });
    let card = col().key("agent-input-card")
        .on_layout(move |s: &mut App, r: [f32; 4]| { s.rects.insert(format!("composer-card:{tab_id}"), r); }).w_full().radius(18.0).fill(rgb(p.bg)).border(1.0, rgb(p.border))
        .px(20.0).pt(16.0).pb(12.0).gap(22.0)
        .on_drop_files(move |s: &mut App, files: &[std::path::PathBuf], _: &mut Tx| {
            s.select_tab(tab_id);
            s.attach_images(files.iter().map(|f| f.display().to_string()).collect());
        })
        .child(stars(app, tab_id))
        // Attached images ride at the top of the card, over the text.
        .child_opt(super::attach::composer_strip(app, tab_id))
        .child(input)
        .child(row().key("composer-chip-row").w_full().gap(26.0).center_y()
            .child(chip(app.agent.clone(), rgb(agent_accent(app, &app.agent)), Kind::Agent))
            .child(row().gap(8.0).center_y().shrink()
                .child(super::slash::provider_mark(app, 17.0 * app.ty.scale()))
                .child(chip(app.model.clone(), rgb(p.blue), Kind::Model)))
            .child(chip(app.effort.clone(), rgb(p.magenta), Kind::Thinking))
            .child(el().grow(1.0))
            .child(attach)
            .child(send));
    let footer = row().key("composer-footer").grow(1.0).shrink().h(32.0).px(18.0).center_y().justify_between()
        .fill(rgb(p.surface).alpha(0.7)).corners([0.0, 0.0, 12.0, 12.0])
        .child(row().gap(10.0).center_y()
            .child(icon(app, glyph::FOLDER, 0.85, rgb(p.dim)))
            .child(text(format!("{}/", app.display_cwd())).font(app.ty.small()).color(rgb(p.fg)).truncate().shrink()).shrink())
        // The context label appears once a conversation reports usage; the group lands at once
        // so `/ commands` never slides to make room for it.
        .child(row().key("composer-footer-right").gap(10.0).center_y().instant_changes()
            .child_opt(app.context_label().map(|label| text(label).key("context").font(app.ty.small()).color(rgb(p.muted)).mr(18.0)))
            .child(text("/").key("slash").font(app.ty.small()).color(rgb(p.fg)))
            .child(text("commands").key("commands").font(app.ty.small()).color(rgb(p.muted))));
    let column = col().w_full().max_w(max_w).child(card);
    if app.composer.hints_hidden { column } else { column.child(row().w_full().px(28.0).child(footer)) }
}

/// Faint stars inside the composer card: spread over the card's own size (and clipped to its
/// corners), each twinkling on one of a few shared phases (`App::tick_stars`), so the field
/// sparkles without re-running a view.
fn stars(app: &App, tab_id: u64) -> El {
    let p = app.pal;
    let [_, _, w, h] = app.rects.get(&format!("composer-card:{tab_id}")).copied().unwrap_or([0.0, 0.0, 820.0, 112.0]);
    let (w, h) = ((w - 8.0).max(40.0), (h - 8.0).max(24.0));
    let mut layer = el().key("composer-stars").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).clip_rounded(18.0);
    let mut seed: u32 = 0x9e37_79b9;
    // About one star per 1,900 square points, as the reference card has.
    let count = ((w * h / 1900.0) as usize).clamp(18, 72);
    for i in 0..count {
        seed ^= seed << 13; seed ^= seed >> 17; seed ^= seed << 5;
        let x = (seed % 1000) as f32 / 1000.0;
        let y = ((seed >> 10) % 1000) as f32 / 1000.0;
        let size = if i % 7 == 0 { 2.0 } else { 1.2 };
        layer = layer.child(el().w(size).h(size).radius(size / 2.0).fill(rgb(p.fg).alpha(if i % 7 == 0 { 0.85 } else { 0.6 }))
            .absolute(4.0 + x * w, 4.0 + y * h).live_opacity(&app.star_twinkle[i % app.star_twinkle.len()]));
    }
    layer
}

impl App {
    /// Step the stars' twinkle: each phase swells and fades on its own period, with a brief
    /// bright glint near its peak. Returns whether an input bar is on screen to sparkle.
    pub(crate) fn tick_stars(&self, clock: f64) -> bool {
        let showing = self.layout.panes().iter().filter_map(|pane| pane.active_tab())
            .any(|id| self.tabs.iter().any(|t| t.id == id && matches!(t.kind, TabKind::Agent { .. })));
        if !showing { return false; }
        for (i, phase) in self.star_twinkle.iter().enumerate() {
            let t = clock as f32 * (0.55 + 0.17 * i as f32) + i as f32 * 1.9;
            let swell = 0.5 + 0.5 * t.sin();
            let glint = (0.5 + 0.5 * (t * 2.3 + 0.7).sin()).powi(8);
            phase.set((0.22 + 0.48 * swell + 0.55 * glint).min(1.0));
        }
        true
    }
}

/// What floats over the agent pane: the picker card above the composer, and a playing skit.
pub fn overlay(app: &App) -> El {
    el().key("agent-overlay").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
        .child(super::slash::effect_layer(app))
        .child(super::slash::picker_card(app))
        .child(super::attach::preview(app))
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
