//! `/piss`, `/cuss`, `/glitch`, `/disco`, `/gangfight`, `/praise`: tiny pixel-fella skits played
//! over the transcript (`view/fx.rs`). Once the deed is done the model hears about it through a
//! follow-up prompt.

mod scenes;
mod sprites;

use crate::ui::theme::Palette;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Effect {
    Piss,
    Cuss,
    Glitch,
    Disco,
    GangFight,
    Praise,
}

/// What the model is told when the skit's deed is done.
pub fn followup_prompt(effect: Effect) -> &'static str {
    match effect {
        Effect::Piss => "user just pissed on you",
        Effect::Cuss => "user just cussed you out \u{2014} he is SO mad at you right now",
        Effect::Glitch => "user briefly unplugged you. remember what that felt like",
        Effect::Disco => "user is so happy with you he's dancing under a disco ball \u{2014} celebrate with him",
        Effect::GangFight => "a gang shootout just went down in your chat and the user's crew won. he's feeling dangerous",
        Effect::Praise => "the user is praising God right now \u{2014} Jesus on the throne, everyone bowing, a whole worship scene in your chat. rejoice with him. Amen.",
    }
}

/// Full runtime of a skit.
pub fn total_seconds(effect: Effect) -> f32 {
    match effect {
        Effect::Piss => 7.4,
        Effect::Cuss => 6.6,
        Effect::Glitch => 4.2,
        Effect::Disco => 7.0,
        Effect::GangFight => 9.0,
        Effect::Praise => 8.0,
    }
}

/// When the follow-up prompt is due: after the deed, so the reply reads as a reaction.
pub fn prompt_at(effect: Effect) -> f32 {
    match effect {
        Effect::Piss => scenes::PISS_END,
        Effect::Cuss => scenes::RANT_END,
        Effect::Glitch => scenes::GLITCH_END,
        Effect::Disco => 2.0,
        Effect::GangFight => scenes::SHOOTOUT_END,
        Effect::Praise => 5.0,
    }
}

/// A playing skit.
pub struct Running {
    pub effect: Effect,
    pub started: Instant,
    /// The session the follow-up prompt goes to (`None`: start a new conversation).
    pub session: Option<String>,
    pub prompted: bool,
}

impl Running {
    pub fn new(effect: Effect, session: Option<String>) -> Running {
        Running { effect, started: Instant::now(), session, prompted: false }
    }

    pub fn elapsed(&self) -> f32 { self.started.elapsed().as_secs_f32() }
}

/// One drawing operation of a frame, in window points with straight sRGB colours.
#[derive(Clone, Debug, PartialEq)]
pub enum Op {
    Rect([f32; 4], sprites::Rgba),
    Rounded([f32; 4], f32, sprites::Rgba),
    Text { at: [f32; 2], text: &'static str, size: f32, color: sprites::Rgba },
}

/// A frame being drawn.
#[derive(Default)]
pub struct Frame {
    pub ops: Vec<Op>,
}

impl Frame {
    fn rect(&mut self, x: f32, y: f32, w: f32, h: f32, color: sprites::Rgba) {
        if w > 0.0 && h > 0.0 { self.ops.push(Op::Rect([x, y, w, h], color)); }
    }

    fn rounded(&mut self, x: f32, y: f32, w: f32, h: f32, color: sprites::Rgba, radius: f32) {
        if w > 0.0 && h > 0.0 { self.ops.push(Op::Rounded([x, y, w, h], radius, color)); }
    }

    fn text(&mut self, x: f32, y: f32, text: &'static str, size: f32, color: sprites::Rgba) {
        self.ops.push(Op::Text { at: [x, y], text, size, color });
    }
}

/// The theme colours the skits use.
pub struct Theme {
    fg: u32,
    bg: u32,
    surface: u32,
    muted: u32,
    accent: u32,
    red: u32,
    green: u32,
    yellow: u32,
    blue: u32,
    magenta: u32,
    cyan: u32,
}

impl Theme {
    pub fn new(p: &Palette) -> Theme {
        Theme { fg: p.fg, bg: p.bg, surface: p.surface, muted: p.muted, accent: p.accent, red: p.red, green: p.green, yellow: p.yellow, blue: p.blue, magenta: p.magenta, cyan: p.cyan }
    }

    fn f32(&self, c: u32) -> sprites::Rgba { self.f32_alpha(c, 1.0) }

    fn f32_alpha(&self, c: u32, a: f32) -> sprites::Rgba {
        [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0, a]
    }
}

/// The scene area: the pane above the composer, 8 pt clear of it.
pub fn scene_rect(pane: [f32; 4], composer_top: f32, scale: f32) -> [f32; 4] {
    [pane[0], pane[1], pane[2], (composer_top - pane[1] - 8.0 * scale).max(0.0)]
}

/// Draw `effect` at `elapsed` seconds into `rect`. Too-short panes show nothing.
pub fn draw(effect: Effect, rect: [f32; 4], elapsed: f32, scale: f32, theme: &Theme) -> Frame {
    let mut frame = Frame::default();
    if rect[3] < 96.0 * scale.clamp(0.5, 3.0) { return frame; }
    let f = &mut frame;
    match effect {
        Effect::Piss => scenes::render_piss(f, rect, elapsed, scale, theme),
        Effect::Cuss => scenes::render_cuss(f, rect, elapsed, scale, theme),
        Effect::Glitch => scenes::render_glitch(f, rect, elapsed, scale, theme),
        Effect::Disco => scenes::render_disco(f, rect, elapsed, scale, theme),
        Effect::GangFight => scenes::render_gang_fight(f, rect, elapsed, scale, theme),
        Effect::Praise => scenes::render_praise(f, rect, elapsed, scale, theme),
    }
    frame
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::theme::palette;

    #[test]
    fn every_skit_draws_through_its_run() {
        let theme = Theme::new(palette("pastel_dark"));
        let rect = [0.0, 0.0, 900.0, 400.0];
        for effect in [Effect::Piss, Effect::Cuss, Effect::Glitch, Effect::Disco, Effect::GangFight, Effect::Praise] {
            assert!(prompt_at(effect) < total_seconds(effect));
            for step in 0..=20 {
                let t = total_seconds(effect) * step as f32 / 20.0;
                let frame = draw(effect, rect, t, 1.0, &theme);
                assert!(!frame.ops.is_empty(), "{effect:?} at {t}");
            }
        }
    }

    #[test]
    fn short_panes_draw_nothing() {
        let theme = Theme::new(palette("pastel_dark"));
        assert!(draw(Effect::Piss, [0.0, 0.0, 900.0, 60.0], 1.0, 1.0, &theme).ops.is_empty());
    }

    #[test]
    fn scene_stops_above_the_composer() {
        assert_eq!(scene_rect([10.0, 40.0, 800.0, 600.0], 500.0, 1.0), [10.0, 40.0, 800.0, 452.0]);
    }
}
