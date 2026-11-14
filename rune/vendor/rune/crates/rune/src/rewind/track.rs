//! The scrub bar's track: one bar per few frames, as tall and bright as the most anything moved
//! in them (red where something jumped), the selection behind and the playhead over it. Dragging
//! scrubs; ⇧-dragging selects.

use std::ops::Range;

use crate::el::Phase;
use crate::{Color, El, Tx, el};

use super::clip::Clip;
use super::scrub::{Scrub, TRACK_H, jump_color, moved_color};

/// Points per bar, gap included.
const BAR_PITCH: f32 = 3.0;
/// A move this many points tall fills the track.
const FULL: f32 = 40.0;

/// Which frames bar `b` of `bins` covers.
pub(crate) fn bin(len: usize, bins: usize, b: usize) -> Range<usize> {
    (b * len / bins)..((b + 1) * len / bins).max(b * len / bins + 1)
}

pub(crate) fn view<S: 'static>(clip: &Clip, frame: usize, sel: Range<usize>, w: f32, lens: fn(&mut S) -> &mut Scrub) -> El {
    let len = clip.len().max(1);
    let bins = ((w / BAR_PITCH) as usize).clamp(1, len);
    let pitch = w / bins as f32;
    let x_of = |k: usize| k as f32 / len as f32 * w;
    let mut track = el().key("rewind-track").w(w).h(TRACK_H).radius(4.0).fill(Color::WHITE.alpha(0.04))
        .role("slider").label("Frame")
        .on_drag(move |s: &mut S, d: &mut crate::Drag, tx: &mut Tx| {
            if d.phase == Phase::Up { return; }
            let scrub = lens(s);
            let Some(len) = scrub.clip().map(|c| c.len()) else { return };
            let k = ((d.x / d.w.max(1.0)).clamp(0.0, 1.0) * len as f32) as usize;
            scrub.seek(k, tx.mods().shift);
        });
    if sel.len() > 1 {
        track = track.child(el().key("rewind-selection").absolute(x_of(sel.start), 0.0).size((x_of(sel.end) - x_of(sel.start)).max(1.0), TRACK_H)
            .fill(Color::WHITE.alpha(0.1)));
    }
    for b in 0..bins {
        let frames = bin(len, bins, b);
        let most = frames.clone().map(|k| clip.activity(k)).fold(0.0f32, f32::max);
        if most <= 0.0 { continue; }
        let jump = frames.clone().any(|k| clip.has_jump(k));
        let level = ((1.0 + most).ln() / (1.0 + FULL).ln()).clamp(0.08, 1.0);
        let h = 3.0 + (TRACK_H - 6.0) * level;
        let color = if jump { jump_color() } else { moved_color().alpha(0.35 + 0.65 * level) };
        track = track.child(el().key(format!("rewind-bin-{b}")).absolute(b as f32 * pitch, TRACK_H - 3.0 - h).size((pitch - 1.0).max(1.0), h)
            .radius(1.0).fill(color));
    }
    track.child(el().key("rewind-playhead").absolute((x_of(frame) + x_of(frame + 1)) / 2.0 - 1.0, 0.0).size(2.0, TRACK_H).fill(Color::WHITE))
}
