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
    Thinking,
    Working,
    Generating,
    WaitingSubagents,
}

impl State {
    pub fn label(self) -> &'static str {
        match self {
            State::Thinking => "Pondering",
            State::Working => "Tinkering",
            State::Generating => "Crafting",
            State::WaitingSubagents => "Sub-agents working",
        }
    }

    fn detail(self) -> Option<&'static str> {
        match self {
            State::Thinking => Some("thinking"),
            State::Working => Some("tools"),
            State::Generating => None,
            State::WaitingSubagents => Some("subagents"),
        }
    }

    /// The state's accent (`streaming_status_accent`).
    fn accent(self, p: &Palette) -> u32 {
        match self {
            State::Thinking => p.magenta,
            State::Working | State::WaitingSubagents => p.yellow,
            State::Generating => p.accent,
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
        Activity { state, glyphs, dots, time: LiveText::new(""), changed: Instant::now(), turn }
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
        row().key("activity").w_full().pt(18.0).center_y().child(letters).child(dots).child(time)
    }
}

impl App {
    /// The live turn's state from its newest rows: reasoning → Pondering, a running tool →
    /// Tinkering (sub-agent tasks → Sub-agents working), otherwise Crafting.
    pub fn activity_state(&self, session_id: &str) -> Option<State> {
        let session = self.sessions.get(session_id).filter(|s| s.streaming)?;
        let from = session.live_from.unwrap_or(0);
        let running = |s: &str| matches!(s, "" | "running" | "pending");
        let live = &session.messages[from.min(session.messages.len())..];
        Some(match live.last() {
            Some(m) if m.kind == HistoryMessageKind::Reasoning => State::Thinking,
            Some(m) if matches!(m.kind, HistoryMessageKind::Tool | HistoryMessageKind::Subtask) && running(&m.status) => {
                if m.tool == "task" { State::WaitingSubagents } else { State::Working }
            }
            _ => State::Generating,
        })
    }

    /// The transcript is scrolled to its end and overflows its view (`activity_geometry`'s
    /// `following`): the activity line then holds the bottom row instead of riding the content.
    pub fn activity_follows_end(&self, session_id: &str) -> bool {
        let Some(session) = self.sessions.get(session_id).filter(|s| s.streaming) else { return false };
        let (offset, max) = session.scroll.get();
        max > 1.0 && max - offset <= 2.0 && crate::ui::follow::wheel_base(session.wheel_target, offset) == offset
    }

    /// The row the activity line takes (its top margin and line).
    pub fn activity_row_height(&self) -> f32 { 18.0 + LINE_H * self.ty.scale() }

    /// Keep the activity line in step with the active conversation. Returns whether it
    /// animates (so the frame clock keeps running) and whether its structure changed.
    pub fn tick_activity(&mut self) -> (bool, bool) {
        let session = match &self.active_tab().kind { crate::tabs::TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None };
        let state = session.as_deref().and_then(|id| self.activity_state(id));
        let structure = match (state, &self.activity) {
            (None, None) => false,
            (None, Some(_)) => { self.activity = None; true }
            (Some(state), Some(current)) if current.state == state => false,
            (Some(state), current) => {
                let turn = current.as_ref().map_or_else(Instant::now, |c| c.turn);
                self.activity = Some(Activity::new(state, turn));
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
    fn labels_and_elapsed() {
        assert_eq!(State::Generating.label(), "Crafting");
        assert_eq!(State::Thinking.label(), "Pondering");
        assert_eq!(format_elapsed(4.24), "4.2s");
        assert_eq!(format_elapsed(61.0), "1m 1s");
    }
}
