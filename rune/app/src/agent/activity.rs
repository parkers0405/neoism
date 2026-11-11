//! The live turn's status line (`view/user_input.rs`, `render_streaming_status_row`):
//! "Crafting . . . · 4.2s model". The word is Press Start 2P; each letter scrambles through
//! punctuation until it locks in turn, then rides a travelling wave, pulsing between the state's
//! accent and the text colour over two depth copies; three dots bob like a swell; an italic
//! timer follows. Every glyph is a live binding, so the animation never re-runs a view.

use crate::app::App;
use crate::ui::theme::{PIXEL, Palette, rgb};
use neoism_protocol::agent::HistoryMessageKind;
use rune::tok::TextStyle;
use rune::{Color, El, Live, LiveText, el, row, text};
use std::time::Instant;

const SCRAMBLE: &[u8] = b"|/-\\+!?>?<%#=@*~&^$";
const LINE_H: f32 = 26.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Compacting,
    Thinking,
    Working,
    Generating,
    WaitingSubagents,
    /// No turn is running, but background tasks of the conversation are.
    Background,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Compacting => "Compacting",
            State::Thinking => "Pondering",
            State::Working => "Tinkering",
            State::Generating => "Crafting",
            State::WaitingSubagents => "Sub-agents working",
            State::Background => "Background",
        }
    }

    fn detail(self) -> Option<&'static str> {
        match self {
            State::Thinking => Some("thinking"),
            State::Working => Some("tools"),
            State::Compacting | State::Generating | State::Background => None,
            State::WaitingSubagents => Some("subagents"),
        }
    }

    /// The state's accent (`streaming_status_accent`).
    fn accent(self, p: &Palette) -> u32 {
        match self {
            State::Compacting => p.green,
            State::Thinking => p.magenta,
            State::Working | State::WaitingSubagents => p.yellow,
            State::Generating => p.accent,
            State::Background => p.red,
        }
    }
}

struct Glyph {
    target: char,
    shown: LiveText,
    offset: Live<[f32; 2]>,
    color: Live<Color>,
}

struct Dot {
    offset: Live<[f32; 2]>,
    color: Live<Color>,
}

/// One running status line. Rebuilt when the state changes; ticked every frame.
pub struct Activity {
    pub state: State,
    /// Background tasks running when the line was built (it is rebuilt when this changes).
    pub background: usize,
    pub queued: u32,
    glyphs: Vec<Glyph>,
    dots: Vec<Dot>,
    time: LiveText,
    changed: Instant,
    turn: Instant,
}

fn mix(a: Color, b: Color, t: f32) -> Color {
    let t = t.clamp(0.0, 1.0);
    Color { r: a.r + (b.r - a.r) * t, g: a.g + (b.g - a.g) * t, b: a.b + (b.b - a.b) * t, a: a.a + (b.a - a.a) * t }
}

pub fn format_elapsed(seconds: f32) -> String {
    let seconds = seconds.max(0.0);
    if seconds < 60.0 { format!("{seconds:.1}s") }
    else if seconds < 3600.0 { format!("{}m {:.0}s", (seconds / 60.0).floor() as u32, seconds % 60.0) }
    else { format!("{}h {}m", (seconds / 3600.0).floor() as u32, ((seconds % 3600.0) / 60.0).floor() as u32) }
}

impl Activity {
    pub fn new(state: State, turn: Instant) -> Activity {
        let glyphs = state.label().chars().map(|target| Glyph {
            target, shown: LiveText::new(target.to_string()), offset: Live::new([0.0, 0.0]), color: Live::new(Color::WHITE),
        }).collect();
        let dots = (0..3).map(|_| Dot { offset: Live::new([0.0, 0.0]), color: Live::new(Color::WHITE) }).collect();
        Activity { state, background: 0, queued: 0, glyphs, dots, time: LiveText::new(""), changed: Instant::now(), turn }
    }

    /// Advance every glyph, dot and the timer to `now` (seconds on the frame clock).
    pub fn tick(&self, p: &Palette, scale: f32) {
        let now = Instant::now();
        let time = self.turn.elapsed().as_secs_f32() + 1000.0;
        let transition = now.duration_since(self.changed).as_secs_f32();
        let accent = rgb(self.state.accent(p));
        let fg = rgb(p.fg);
        let count = self.glyphs.len().max(1);
        let motion = time * 3.0;
        let (drift_x, drift_y) = (motion.sin() * 1.8 * scale, (motion * 0.72).cos() * 0.8 * scale);
        let mut trailing = accent;
        for (i, glyph) in self.glyphs.iter().enumerate() {
            let locked = transition >= (i as f32 + 1.0) * (0.7 / count as f32);
            let shown = if locked || glyph.target == ' ' { glyph.target } else { SCRAMBLE[((time * 44.0) as usize + i * 5) % SCRAMBLE.len()] as char };
            glyph.shown.set(shown.to_string());
            // Colour: a wave and a pulse between the accent and the text colour.
            let (speed, spread) = if locked { (3.4, 0.62) } else { (8.2, 0.91) };
            let wave = (time * speed + i as f32 * spread).sin() * 0.5 + 0.5;
            let pulse = (time * 6.2 + i as f32 * 0.9).sin() * 0.5 + 0.5;
            let amount = if locked { 0.06 + wave * 0.18 + pulse * 0.08 } else { 0.12 + wave * 0.34 };
            let color = mix(accent, fg, amount);
            glyph.color.set(color);
            trailing = color;
            // Motion: locked letters sway on a travelling wave; scrambling ones shake.
            let phase = time * 5.6 + i as f32 * 0.82;
            let (sway, lift) = if locked {
                (phase.cos() * 1.5 * scale, phase.sin() * 2.4 * scale + drift_y)
            } else {
                ((phase * 1.7).sin() * 0.8 * scale, (phase * 1.9).cos() * 0.9 * scale)
            };
            glyph.offset.set([drift_x + sway, -lift]);
        }
        for (i, dot) in self.dots.iter().enumerate() {
            let phase = time * 4.0 + i as f32 * 0.95;
            let swell = phase.sin();
            let lift = (swell * 0.5 + 0.5) * 1.7 * scale;
            let drift = (phase * 0.55 + 1.2).cos() * scale;
            dot.offset.set([drift_x + drift, -lift]);
            dot.color.set(trailing.alpha(0.40 + (swell * 0.5 + 0.5) * 0.45));
        }
        let elapsed = format_elapsed(self.turn.elapsed().as_secs_f32());
        self.time.set(match self.state.detail() {
            Some(detail) => format!("\u{b7} {elapsed} model \u{b7} {detail}"),
            None => format!("\u{b7} {elapsed} model"),
        });
    }

    pub fn view(&self, app: &App) -> El {
        let p = app.pal;
        let s = app.ty.scale();
        let size = 12.0 * s;
        let word = TextStyle::new(size, 400.0).family(PIXEL);
        let layer = |glyph: &Glyph, dx: f32, ink: Option<Color>| {
            let mut t = text(glyph.target.to_string()).font(word).live_text(&glyph.shown).absolute(dx, dx);
            t = match ink { Some(ink) => t.color(ink), None => t.live_text_color(&glyph.color) };
            t
        };
        let mut letters = row().h(LINE_H * s).center_y().pl(3.3 * s);
        for glyph in &self.glyphs {
            // The glyph box keeps the font's one-em advance; the layers move inside it.
            letters = letters.child(el().w(size).h(size).live_offset(&glyph.offset)
                .child(layer(glyph, 3.5 * s, Some(rgb(p.bg).alpha(210.0 / 255.0))))
                .child(layer(glyph, 1.75 * s, Some(rgb(p.dim).alpha(240.0 / 255.0))))
                .child(layer(glyph, 0.0, None)));
        }
        let dot_font = app.ty.sized_bold(14.0);
        let mut dots = row().ml(7.0 * s).gap(2.0 * s).items_end();
        for dot in &self.dots {
            let ink = |dx: f32, color: Color| text(".").font(dot_font).color(color).absolute(dx, dx);
            dots = dots.child(el().w(8.4 * s).h(size).live_offset(&dot.offset)
                .child(ink(3.0 * s, rgb(p.bg).alpha(0.8)))
                .child(ink(1.5 * s, rgb(p.dim)))
                .child(text(".").font(dot_font).live_text_color(&dot.color)));
        }
        let time = text("\u{b7} 00.0s model \u{b7} subagents").font(app.ty.sized(13.0).italic()).color(rgb(p.muted))
            .live_text(&self.time).ml(8.0 * s);
        // Padding, not margin: a virtual row drops its top margin, which slid this line (and its
        // dark glyph shadows) up over the reply's last line.
        let line = row().w_full().center_y().child(letters).child(dots).child(time);
        let queued = (self.queued > 0).then(|| {
            let label = if self.queued == 1 { "Queued".into() } else { format!("Queued x{}", self.queued) };
            let branch = if self.background > 0 { "\u{251c}\u{2500}" } else { "\u{2570}\u{2500}" };
            text(format!("{branch} {label}")).key("activity-queued").font(app.ty.sized(13.0).italic()).color(rgb(p.accent))
                .h(LINE_H * s).pl(3.3 * s)
        });
        // Under it, how many background tasks are still running (`╰─ 2 background tasks running`).
        let background = (self.background > 0).then(|| {
            let noun = if self.background == 1 { "task" } else { "tasks" };
            text(format!("\u{2570}\u{2500} {} background {noun} running", self.background)).font(app.ty.sized(13.0)).color(rgb(p.muted))
                .h(LINE_H * s).pl(3.3 * s)
        });
        rune::col().key("activity").w_full().pt(18.0).child(line).children(queued).children(background)
    }
}

impl App {
    /// The live turn's state from its newest rows: reasoning → Pondering, a running tool →
    /// Tinkering (sub-agent tasks → Sub-agents working), otherwise Crafting.
    pub fn activity_state(&self, session_id: &str) -> Option<State> {
        if self.sessions.get(session_id).is_some_and(|s| s.compacting) { return Some(State::Compacting); }
        let background = (self.background_count(session_id) > 0).then_some(State::Background);
        let Some(session) = self.sessions.get(session_id).filter(|_| self.session_live(session_id)) else { return background };
        let from = session.live_from.unwrap_or(0);
        let running = |s: &str| matches!(s, "" | "running" | "pending");
        let live = &session.messages[from.min(session.messages.len())..];
        match live.last() {
            Some(m) if m.kind == HistoryMessageKind::Reasoning => Some(State::Thinking),
            // Waiting on sub-agents says nothing here: their rows in the sidebar and their
            // task cards already do.
            Some(m) if matches!(m.kind, HistoryMessageKind::Tool | HistoryMessageKind::Subtask) && running(&m.status) && m.tool == "task" => background,
            Some(m) if matches!(m.kind, HistoryMessageKind::Tool | HistoryMessageKind::Subtask) && running(&m.status) => Some(State::Working),
            _ => Some(State::Generating),
        }
    }

    /// A turn is running in this conversation: its own, or (a sub-agent being viewed) the work
    /// its family says it is still doing (`viewed_subagent_outstanding`). A sub-agent's turn
    /// is reported through its family, not always as a streaming state of its own.
    pub fn session_live(&self, session_id: &str) -> bool {
        self.sessions.get(session_id).is_some_and(|s| s.streaming)
            || (self.families.working_now(session_id) && self.families.member(session_id).is_some_and(|m| m.status == "running"))
    }

    /// Background tasks still running in the conversation's family (`RuntimeSnapshot`).
    pub fn background_count(&self, session_id: &str) -> usize {
        self.background_jobs.get(self.families.root_of(session_id)).copied().unwrap_or(0)
    }

    /// The transcript is scrolled to its end and overflows its view (`activity_geometry`'s
    /// `following`): the activity line then holds the bottom row instead of riding the content.
    pub fn activity_follows_end(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get(session_id).filter(|_| self.session_live(session_id)) else { return false };
        let (offset, max) = session.scroll.get();
        max > 1.0 && max - offset <= 2.0 && crate::ui::follow::wheel_base(session.wheel_target, offset) == offset
    }

    /// The row the activity line takes (its top margin and line).
    pub fn activity_row_height(&self) -> f32 {
        let lines = self.activity.as_ref().map_or(1, |a| 1 + usize::from(a.queued > 0) + usize::from(a.background > 0));
        18.0 + LINE_H * self.ty.scale() * lines as f32
    }

    /// Keep the activity line in step with the active conversation. Returns whether it
    /// animates (so the frame clock keeps running) and whether its structure changed.
    pub fn tick_activity(&mut self) -> (bool, bool) {
        let session = match &self.active_tab().kind { crate::tabs::TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None };
        let state = session.as_deref().and_then(|id| self.activity_state(id));
        let background = session.as_deref().map_or(0, |id| self.background_count(id));
        let queued = session.as_deref().and_then(|id| self.sessions.get(id)).map_or(0, |s| s.queued_prompts);
        let structure = match (state, &self.activity) {
            (None, None) => false,
            (None, Some(_)) => { self.activity = None; true }
            (Some(state), Some(current)) if current.state == state && current.background == background && current.queued == queued => false,
            (Some(state), current) => {
                // The same word keeps its letters where they are; only a new one scrambles in.
                let (turn, changed) = current.as_ref().map_or_else(|| (Instant::now(), None), |c| (c.turn, (c.state == state).then_some(c.changed)));
                let mut activity = Activity::new(state, turn);
                activity.background = background;
                activity.queued = queued;
                if let Some(changed) = changed { activity.changed = changed; }
                self.activity = Some(activity);
                true
            }
        };
        if let Some(activity) = &self.activity { activity.tick(self.pal, self.ty.scale()); }
        // Following the end of an overflowing transcript, the line pins above the composer.
        let pinned = self.activity.is_some() && session.as_deref().is_some_and(|id| self.activity_follows_end(id));
        let flipped = pinned != self.activity_pinned;
        self.activity_pinned = pinned;
        (self.activity.is_some(), structure || flipped)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_snapshots_update_the_activity_and_reserved_space_per_conversation() {
        use neoism_protocol::agent::AgentServerMessage as M;
        let mut app = App::new();
        app.load_fixture("active", "Steering", "[]");
        app.sessions.get_mut("active").unwrap().streaming = true;
        app.tick_activity();
        let base = app.activity_row_height();
        let extra = LINE_H * app.ty.scale();
        let update = |session: &str, count| M::QueueUpdate {
            session_id: session.into(), count, preview: None, started_at: None,
        };

        app.apply_agent(update("active", 1));
        assert!(app.tick_activity().1);
        assert_eq!(app.activity.as_ref().unwrap().queued, 1);
        assert_eq!(app.activity_row_height(), base + extra);
        let revision = app.sessions["active"].structure;
        app.apply_agent(update("active", 1));
        assert!(!app.tick_activity().1);
        assert_eq!(app.sessions["active"].structure, revision);

        app.apply_agent(M::SessionEvent { session_id: "active".into(), kind: "session.queue.updated".into(),
            properties: serde_json::json!({"queue": {"count": 2}, "action": "enqueued"}) });
        app.tick_activity();
        assert_eq!(app.activity.as_ref().unwrap().queued, 2);
        assert_eq!(app.activity_row_height(), base + extra);
        app.apply_agent(update("other", 8));
        assert!(!app.tick_activity().1);
        assert_eq!(app.activity.as_ref().unwrap().queued, 2);

        app.apply_agent(update("active", 0));
        app.tick_activity();
        assert_eq!(app.activity.as_ref().unwrap().queued, 0);
        assert_eq!(app.activity_row_height(), base);
        app.apply_agent(M::SessionIdle { session_id: "active".into() });
        app.tick_activity();
        app.apply_agent(update("active", 1));
        app.tick_activity();
        assert!(app.activity.is_none(), "waiting work must not resurrect a completed turn's loader");
    }

    #[test]
    fn labels_and_elapsed() {
        assert_eq!(State::Generating.label(), "Crafting");
        assert_eq!(State::Thinking.label(), "Pondering");
        assert_eq!(format_elapsed(4.24), "4.2s");
        assert_eq!(format_elapsed(61.0), "1m 1s");
    }
}
