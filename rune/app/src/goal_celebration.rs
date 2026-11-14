//! A centered, bounded, non-interactive Press Start 2P / shared pixel-fire celebration.
//! Only a locally submitted new goal followed by its persisted server notice can start it.
use crate::{app::App, ui::theme::rgb};
use neoism_protocol::agent::{AgentServerMessage as Server, HistoryMessageKind};
use rune::{El, Live, el, row, text};
use std::time::{Duration, Instant};
use crate::agent::activity_flames as flames;
use std::cell::Cell;

const LIFE: f32 = 1.8;

pub(crate) fn plain() -> bool {
    if std::env::var("NEOISM_REDUCED_TRANSPARENCY").as_deref() == Ok("1")
        || std::env::var("NEOISM_HIGH_CONTRAST").as_deref() == Ok("1") { return true; }
    #[cfg(target_os = "macos")]
    {
        unsafe extern "C" { fn neoism_glass_selection_allowed() -> bool; }
        return !(unsafe { neoism_glass_selection_allowed() });
    }
    #[cfg(not(target_os = "macos"))]
    false
}

#[derive(Default)]
pub struct Celebration {
    pending: Option<(String, String, String)>,
    playing: Option<Playing>,
}

struct Playing {
    session: String,
    start: Instant,
    reduced: bool,
    opacity: Live<f32>,
    flames: [rune::Pixels; 12],
    fire_frame: Cell<Option<(u32, [u32; 5])>>,
}

impl Celebration {
    pub fn arm(&mut self, session: &str, goal: &str, message_id: &str) {
        self.pending = Some((session.into(), goal.trim().into(), format!("prt_goal_started_{message_id}")));
    }

    pub fn cancel(&mut self) { self.pending = None; self.playing = None; }

    pub fn observe(&mut self, event: &Server) {
        if matches!(event, Server::Error { .. }) { self.pending = None; }
        let Server::MessageUpdated { session_id, message, .. } = event else { return };
        if message.tool != "goal_started" || message.status != "completed" { return; }
        let Some((session, goal, notice_id)) = &self.pending else { return };
        // The daemon preserves SubmitPrompt.message_id through goal admission; the server
        // derives this notice ID from it. No clocks, receipt-time heuristics or bounded
        // replay caches: a fresh UUID per submission and consuming intent give one shot.
        if session != session_id || message.kind != HistoryMessageKind::System
            || message.title != "Goal Started" || message.id != *notice_id
            || message.tool != "goal_started" || message.status != "completed"
            || message.text.trim() != goal { return; }
        self.pending = None;
        self.start(session_id, plain() || crate::glass_cursor::reduce_motion()
            || std::env::var("NEOISM_REDUCED_MOTION").as_deref() == Ok("1"));
    }

    fn start(&mut self, session: &str, reduced: bool) {
        self.playing = Some(Playing {
            session: session.into(), start: Instant::now(), reduced,
            opacity: Live::new(1.0),
            flames: std::array::from_fn(|_| rune::Pixels::new()),
            fire_frame: Cell::new(None),
        });
    }
}

fn alpha(t: f32, reduced: bool) -> f32 {
    if t >= LIFE { 0.0 } else if reduced { 1.0 } else { ((LIFE - t) / 0.45).clamp(0.0, 1.0) }
}

fn geometry(window: [f32; 2], scale: f32) -> ([f32; 4], f32) {
    let size = (28.0 * scale)
        .min((window[0] - 32.0).max(1.0) / 16.0)
        .min((window[1] - 32.0).max(1.0) / 5.0);
    let (w, h) = (16.0 * size, 5.0 * size);
    ([(window[0] - w) / 2.0, (window[1] - h) / 2.0, w, h], size)
}

pub fn poll(app: &mut App, tx: &mut rune::Tx) -> bool {
    let active = app.active_session();
    let Some(p) = &app.goal_celebration.playing else { return false };
    let t = p.start.elapsed().as_secs_f32();
    if t >= LIFE || active.as_deref() != Some(p.session.as_str()) {
        app.goal_celebration.playing = None;
        return true;
    }
    p.opacity.set(alpha(t, p.reduced));
    if p.reduced {
        tx.wake_after(Duration::from_secs_f32(LIFE - t));
        return false;
    }
    let colors = crate::ui::fire::colors(app.pal);
    let frame = ((t * 16.0) as u32, colors);
    if app.wordmark.fire_enabled && p.fire_frame.get() != Some(frame) {
        p.fire_frame.set(Some(frame));
        for (i, c) in "Goal started".chars().enumerate() {
            if let Some(fire) = flames::glyph(c) {
                p.flames[i].set(40, 88, flames::rgba(fire, t, colors));
            }
        }
    }
    tx.wake_after(Duration::from_millis(16));
    false // Live bindings, not a view commit per frame.
}

pub fn view(app: &App) -> El {
    let Some(p) = &app.goal_celebration.playing else { return el() };
    if app.active_session().as_deref() != Some(p.session.as_str()) { return el(); }
    let ([x, y, w, h], size) = geometry(app.window, app.ty.scale());
    let letters = lettering(app, "Goal started", size, p.reduced, &p.flames);
    let overlay = el().key("goal-celebration").absolute(x, y).w(w).h(h);
    let overlay = if p.reduced { overlay.fill(rgb(app.pal.bg)) } else { overlay };
    overlay
        .live_opacity(&p.opacity).instant_changes().inert()
        .child(letters.absolute(2.0 * size, if p.reduced || !app.wordmark.fire_enabled { 2.0 * size } else { 2.5 * size }))
}

pub(crate) fn lettering(app: &App, label: &str, size: f32, reduced: bool, pixels: &[rune::Pixels]) -> El {
    let mut letters = row().gap(0.0);
    for (i, c) in label.chars().enumerate() {
        let mut glyph = el().w(size).h(size);
        if !reduced && app.wordmark.fire_enabled && c != ' ' {
            glyph = glyph.child(el().pixels(&pixels[i])
                .absolute(-size / 8.0, -12.5 * size / 8.0)
                .w(size * 10.0 / 8.0).h(size * 22.0 / 8.0));
        }
        letters = letters.child(glyph.child(text(c.to_string()).font(flames::font(size))
            .color(rgb(app.pal.fg)).absolute(0.0, 0.0)));
    }
    letters
}

pub fn fixture(app: &mut App) {
    app.load_fixture("celebration", "Goal", "[]");
    app.goal_celebration.start("celebration", plain() || crate::glass_cursor::reduce_motion()
        || std::env::var("NEOISM_REDUCED_MOTION").as_deref() == Ok("1"));
    // Sample the shared flames mid-animation for the headless fixture.
    if std::env::var_os("NEOISM_RUNE_DRIVE").is_none() {
        app.goal_celebration.playing.as_mut().unwrap().start -= Duration::from_millis(550);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn notice(prompt: &str, created: u64) -> Server {
        Server::MessageUpdated { session_id: "s".into(), parent_message_id: None,
            message: serde_json::from_value(serde_json::json!({"id":format!("prt_goal_started_{prompt}"), "role":"System", "kind":"System", "title":"Goal Started", "tool":"goal_started", "status":"completed", "text":"ship", "created_at":created})).unwrap() }
    }
    #[test]
    fn remote_clock_behind_still_confirms_exact_request_once_not_replay() {
        let mut c = Celebration::default();
        c.observe(&notice("old", 1));
        assert!(c.playing.is_none());
        c.arm("s", "ship", "new");
        c.observe(&notice("old", u64::MAX)); // even a future timestamp cannot match
        assert!(c.playing.is_none());
        let Server::MessageUpdated { message, .. } = notice("new", 1) else { unreachable!() };
        c.observe(&Server::HistoryChunk { session_id: "s".into(), messages: vec![message], next_cursor: None });
        assert!(c.playing.is_none());
        c.observe(&notice("new", 1)); // server clock decades behind client
        let start = c.playing.as_ref().unwrap().start;
        c.observe(&notice("new", 1));
        assert_eq!(c.playing.as_ref().unwrap().start, start);
        c.arm("s", "ship", "next");
        c.observe(&notice("new", 1)); // same text, new submission: old event still rejected
        assert_eq!(c.playing.as_ref().unwrap().start, start);
        c.observe(&notice("next", 1));
        assert!(c.pending.is_none());
        assert!(c.playing.is_some());
        c.arm("s", "ship", "disconnected");
        c.cancel();
        c.observe(&notice("disconnected", 1)); // reconnect abandons even a matching in-flight intent
        assert!(c.playing.is_none());
    }
    #[test]
    fn reduced_motion_static_and_finite() {
        assert_eq!(alpha(0.0, true), 1.0);
        assert_eq!(alpha(1.7, true), 1.0);
        for reduced in [false, true] {
            assert_eq!(alpha(LIFE, reduced), 0.0);
            assert_eq!(alpha(100.0, reduced), 0.0);
        }
    }

    #[test]
    fn centered_for_window_sizes_and_font_scales() {
        for window in [[1252.0, 674.0], [480.0, 320.0], [1800.0, 1000.0]] {
            for scale in [0.8, 1.0, 2.0] {
                let ([x, y, w, h], _) = geometry(window, scale);
                assert_eq!([x + w / 2.0, y + h / 2.0], [window[0] / 2.0, window[1] / 2.0]);
                assert!(x >= 0.0 && y >= 0.0);
            }
        }
    }

    #[test]
    fn goal_uses_animated_activity_glyph_fire_in_each_theme() {
        for theme in ["neoism_dark", "neoism_light", "catppuccin_mocha", "one_light"] {
            let colors = crate::ui::fire::colors(crate::ui::theme::palette(theme));
            for c in "Goal started".chars().filter(|c| *c != ' ') {
                let fire = flames::glyph(c).expect("every goal glyph has its font outline");
                let a = flames::rgba(fire, 0.5, colors);
                assert_ne!(a, flames::rgba(fire, 0.75, colors));
                fire.cells(0.5, colors, |x, y, color| {
                    let index = (((y + 13.0) as usize * 4) * 40 + (x + 1.0) as usize * 4) * 4;
                    assert_eq!(&a[index..index + 4], &[(color >> 16) as u8, (color >> 8) as u8, color as u8, 255]);
                });
            }
        }
    }

    #[test]
    fn error_disarms_without_celebrating() {
        let mut c = Celebration::default();
        c.arm("s", "ship", "new");
        c.observe(&Server::Error { message: "failed".into() });
        c.observe(&notice("new", 1));
        assert!(c.playing.is_none());
    }

    #[test]
    fn poll_removes_finished_subtree_and_stops_waking() {
        let mut app = App::new();
        fixture(&mut app);
        app.goal_celebration.playing.as_mut().unwrap().start -= Duration::from_secs(2);
        assert!(poll(&mut app, &mut rune::Tx::default()));
        assert!(app.goal_celebration.playing.is_none());
        assert!(!poll(&mut app, &mut rune::Tx::default()));
    }

    #[test]
    fn app_only_arms_new_goal_submissions_and_preserves_input() {
        let mut app = App::new();
        app.load_fixture("s", "Goal", "[]");
        app.tabs[app.active].draft.paste("/goal ship");
        assert!(app.goal_celebration.pending.is_none());
        for command in ["/goal", "/goals", "/goal show", "/goal pause", "/goal resume"] {
            assert!(!app.send_new_goal("s", command));
            assert!(app.goal_celebration.pending.is_none(), "{command}");
        }
        assert!(app.send_new_goal("s", "/goal ship"));
        let sent: neoism_protocol::agent::AgentClientMessage = serde_json::from_str(app.agent_sent.borrow().last().unwrap()).unwrap();
        let neoism_protocol::agent::AgentClientMessage::SubmitPrompt { message_id, .. } = sent else { panic!("must use correlated goal admission") };
        let focus = app.focus;
        let draft = app.tabs[app.active].draft.text.clone();
        app.apply_agent(notice(&message_id, 1));
        assert!(app.goal_celebration.playing.is_some());
        assert_eq!(app.focus, focus);
        assert_eq!(app.tabs[app.active].draft.text, draft);
        let started = app.goal_celebration.playing.as_ref().unwrap().start;
        app.apply_agent(notice(&message_id, 1));
        assert_eq!(app.goal_celebration.playing.as_ref().unwrap().start, started);
    }
}