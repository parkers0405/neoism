//! A title too long for its place slides to show the rest while the pointer is on it
//! (`primitives/text.rs::hover_title_offset`, GPUiX `HoverTabTitle`): 0.6 s of rest, then it
//! travels to its end and back at 35 pt a second (never under 2 s a leg), holding at both
//! ends. Only the text moves: its clip box, the icon beside it and the close target stay put.
//!
//! The travel is a live offset. The views re-run twice per hover, when the title swaps its
//! ellipsis for the whole text and when the pointer leaves.

use crate::app::App;
use rune::{Color, El, Live, TextStyle, el, text};
use std::time::Duration;

/// The rest before a hovered title starts to move.
const LEAD_IN: f32 = 0.6;
/// Points a second, at the default type size.
const SPEED: f32 = 35.0;
/// The shortest one-way trip.
const MIN_LEG: f32 = 2.0;
/// The share of a leg held at each end.
const HOLD: f32 = 0.15;
/// The app's type is monospaced: a glyph advances this share of the font size.
const ADVANCE: f32 = 0.6;

pub struct Marquee {
    /// The title under the pointer, its full width, and when the pointer came (frame clock;
    /// unset until the first frame after).
    hovered: Option<(String, f32, Option<f64>)>,
    /// The hovered title is past its rest and too long: it is drawn whole and moving.
    full: bool,
    offset: Live<[f32; 2]>,
}

impl Default for Marquee {
    fn default() -> Self { Marquee { hovered: None, full: false, offset: Live::new([0.0, 0.0]) } }
}

/// How wide `label` is drawn whole.
pub fn natural(label: &str, style: TextStyle) -> f32 {
    label.chars().count() as f32 * style.size * ADVANCE
}

/// How far a title `distance` points too long is slid, `elapsed` seconds into a hover, and how
/// long it then holds still (`None`: in motion).
pub fn frame(elapsed: f32, distance: f32, scale: f32) -> (f32, Option<f32>) {
    if elapsed < LEAD_IN { return (0.0, Some(LEAD_IN - elapsed)); }
    let leg = (distance / (SPEED * scale)).max(MIN_LEG);
    let phase = ((elapsed - LEAD_IN) / leg) % 2.0;
    let progress = if phase <= 1.0 { phase } else { 2.0 - phase };
    let offset = distance * ((progress - HOLD) / (1.0 - 2.0 * HOLD)).clamp(0.0, 1.0);
    let hold = if phase < HOLD { HOLD - phase }
        else if (1.0 - HOLD..1.0 + HOLD).contains(&phase) { 1.0 + HOLD - phase }
        else if phase >= 2.0 - HOLD { 2.0 + HOLD - phase }
        else { return (offset, None) };
    (offset, Some(hold * leg))
}

fn rect_key(key: &str) -> String { format!("title:{key}") }

impl App {
    /// A one-line title: cut with an ellipsis at rest, whole and sliding while it is the
    /// hovered title (`title_hover`). `max_w` bounds it; without one it shrinks to its row.
    pub fn title(&self, key: &str, label: String, style: TextStyle, color: Color, max_w: Option<f32>) -> El {
        let rect = rect_key(key);
        let running = self.marquee.full && self.marquee.hovered.as_ref().is_some_and(|(k, ..)| k == key);
        if running && let Some(r) = self.rects.get(&rect) {
            let whole = natural(&label, style).ceil() + 2.0;
            return el().key(format!("title-run:{key}")).w(r[2]).h(r[3]).clip_soft(0.0)
                .child(text(label).font(style).color(color).w(whole).absolute(0.0, 0.0).live_offset(&self.marquee.offset));
        }
        let title = text(label).key(rect.clone()).font(style).color(color).truncate();
        let title = match max_w { Some(w) => title.max_w(w), None => title.shrink() };
        title.on_layout(move |a: &mut App, r: [f32; 4]| { a.rects.insert(rect.clone(), r); })
    }

    /// The pointer came to, or left, the row or tab that shows the title `key`, `natural`
    /// points wide when whole.
    pub fn title_hover(&mut self, key: &str, natural: f32, inside: bool) {
        let current = self.marquee.hovered.as_ref().is_some_and(|(k, ..)| k == key);
        if inside && !current {
            self.marquee.hovered = Some((key.to_owned(), natural, None));
            self.marquee.full = false;
            self.marquee.offset.set([0.0, 0.0]);
        } else if !inside && current {
            self.marquee.hovered = None;
            self.marquee.full = false;
            self.marquee.offset.set([0.0, 0.0]);
        }
    }

    /// Per frame: slide the hovered title. Returns when to look again, whether the views must
    /// re-run (the title just went whole), and whether it is moving this frame.
    pub(crate) fn marquee_tick(&mut self, clock: f64) -> (Option<Duration>, bool, bool) {
        let scale = self.ty.scale();
        let Some((key, natural, since)) = self.marquee.hovered.as_mut() else { return (None, false, false) };
        let elapsed = (clock - *since.get_or_insert(clock)) as f32;
        let Some(shown) = self.rects.get(&rect_key(key)).map(|r| r[2]) else { return (None, false, false) };
        let distance = *natural - shown;
        // It fits: nothing to show.
        if distance <= 0.5 { return (None, false, false); }
        let (offset, hold) = frame(elapsed, distance, scale);
        if elapsed < LEAD_IN { return (hold.map(Duration::from_secs_f32), false, false); }
        let changed = !std::mem::replace(&mut self.marquee.full, true);
        self.marquee.offset.set([-offset, 0.0]);
        match hold {
            Some(hold) => (Some(Duration::from_secs_f32(hold)), changed, false),
            None => (Some(Duration::ZERO), changed, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_title_rests_travels_holds_and_comes_back() {
        // The original's own checkpoints (`tab-scroll-title-check.tsx`), 70 pt too long.
        assert_eq!(frame(0.599, 70.0, 1.0).0, 0.0);
        assert!(frame(0.9, 70.0, 1.0).0.abs() < 1e-4, "held at the start");
        assert!((frame(2.3, 70.0, 1.0).0 - 70.0).abs() < 1e-4, "all the way at the end of a leg");
        assert!(frame(3.6, 70.0, 1.0).0 < 70.0, "on its way back");
        // A longer title keeps the pace instead of the two-second minimum.
        let (halfway, hold) = frame(0.6 + 5.0, 350.0, 1.0);
        assert!((halfway - 175.0).abs() < 1e-3 && hold.is_none());
        // Holding still says for how long, so no frames are drawn for it.
        assert!(frame(0.7, 70.0, 1.0).1.is_some_and(|hold| (hold - 0.2).abs() < 1e-4));
    }

    #[test]
    fn only_the_hovered_title_that_overflows_goes_whole() {
        let mut app = App::new();
        let style = app.ty.tab();
        let long = "A conversation title far too long for its tab";
        app.rects.insert(rect_key("tab:1"), [0.0, 0.0, 120.0, 18.0]);
        app.rects.insert(rect_key("tab:2"), [0.0, 0.0, 400.0, 18.0]);
        app.title_hover("tab:1", natural(long, style), true);
        // Resting: a wake for the end of the lead-in, no change.
        let (wake, changed, moving) = app.marquee_tick(10.0);
        assert!(wake.is_some_and(|w| (w.as_secs_f32() - 0.6).abs() < 1e-3) && !changed && !moving);
        // Past the rest it goes whole once, then moves on its own.
        let (_, changed, _) = app.marquee_tick(10.7);
        assert!(changed && app.marquee.full);
        // (It is some 230 pt too long: a leg of about 6.6 s, held for the first second of it.)
        assert!(app.marquee_tick(11.0).0.is_some_and(|hold| !hold.is_zero()));
        let (wake, changed, moving) = app.marquee_tick(14.0);
        assert!(!changed && moving && wake == Some(Duration::ZERO));
        // Leaving puts the ellipsis back at once.
        app.title_hover("tab:1", 0.0, false);
        assert!(!app.marquee.full && app.marquee_tick(11.6) == (None, false, false));
        // A title that fits never starts.
        app.title_hover("tab:2", natural("Short", style), true);
        assert_eq!(app.marquee_tick(20.0), (None, false, false));
        assert_eq!(app.marquee_tick(22.0), (None, false, false));
    }
}
