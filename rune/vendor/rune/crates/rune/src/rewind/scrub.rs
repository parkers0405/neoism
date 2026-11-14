//! The scrubber: a frozen clip's frames over the window, one at a time, with the elements that
//! moved outlined and labelled, a track along the bottom that lights up where things moved, and
//! Send for the selected stretch.

use std::ops::Range;
use std::sync::Arc;

use crate::edit::{Key, KeyEvent};
use crate::tok::{self, type_};
use crate::{Color, El, Tx, col, el, row, spacer, text, text_input};

use super::bundle::Bundle;
use super::clip::{Change, Clip, Shot};
use super::Rewind;

/// The bar along the window's foot, in points.
pub(crate) const BAR_H: f32 = 92.0;
pub(crate) const TRACK_H: f32 = 34.0;
pub(crate) const PAD: f32 = 12.0;
/// Frames of an element's path shown either side of the playhead when nothing is selected.
const PATH_SPAN: usize = 90;
/// Narrower than this, the bar drops its key hints.
const HINT_MIN_W: f32 = 760.0;

pub(crate) fn moved_color() -> Color { crate::oklch(0.86, 0.15, 85.0) }
pub(crate) fn jump_color() -> Color { crate::oklch(0.68, 0.2, 25.0) }

/// The scrubber's state; keep one in the app's state.
#[derive(Default)]
pub struct Scrub {
    rewind: Option<Rewind>,
    clip: Option<Arc<Clip>>,
    frame: usize,
    anchor: Option<usize>,
    picked: Option<u32>,
    note: crate::TextEdit,
}

/// What a press of a key did ([`Scrub::key`]).
#[derive(Debug)]
#[non_exhaustive]
pub enum Keyed {
    /// Not the scrubber's key.
    Ignored,
    Handled,
    /// Escape: the scrubber closed and recording resumed.
    Closed,
    /// Return: send the selected stretch.
    Send(Sent),
}

/// A stretch of frames to hand on, with the note typed for it.
#[derive(Clone)]
pub struct Sent {
    clip: Arc<Clip>,
    frames: Range<usize>,
    note: String,
}

impl std::fmt::Debug for Sent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Sent").field("frames", &self.frames).field("note", &self.note).finish()
    }
}

impl Sent {
    pub fn clip(&self) -> &Arc<Clip> { &self.clip }
    pub fn frames(&self) -> Range<usize> { self.frames.clone() }
    pub fn note(&self) -> &str { &self.note }
    /// [`Clip::write_bundle`] for this stretch and note.
    pub fn write(&self, dir: &std::path::Path) -> std::io::Result<Bundle> {
        self.clip.write_bundle(dir, self.frames.clone(), &self.note)
    }
}

impl Scrub {
    /// Freezes `rewind` and shows its clip at the last frame anything moved in.
    pub fn open(&mut self, rewind: &Rewind) {
        self.show(rewind.freeze());
        self.rewind = Some(rewind.clone());
    }

    /// Shows `clip` without a recorder behind it (a story, a saved clip).
    pub fn show(&mut self, clip: Arc<Clip>) {
        let last = clip.len().saturating_sub(1);
        self.frame = if clip.activity(last) > 0.0 { last } else { clip.next_active(last, -1).unwrap_or(last) };
        self.clip = Some(clip);
        self.anchor = None;
        self.picked = None;
        self.note = crate::TextEdit::default();
    }

    /// Hides the scrubber and resumes recording.
    pub fn close(&mut self) {
        if let Some(rewind) = self.rewind.take() { rewind.resume(); }
        self.clip = None;
    }

    pub fn is_open(&self) -> bool { self.clip.is_some() }

    pub fn clip(&self) -> Option<&Arc<Clip>> { self.clip.as_ref() }

    /// The frame showing.
    pub fn frame(&self) -> usize { self.frame }

    /// The selected frames: from the anchor ⇧ set to the playhead, or the playhead alone.
    pub fn selection(&self) -> Range<usize> {
        let a = self.anchor.unwrap_or(self.frame);
        a.min(self.frame)..a.max(self.frame) + 1
    }

    /// Shows frame `k`; with `extend`, selects from where the selection started to it.
    pub fn seek(&mut self, k: usize, extend: bool) {
        let Some(clip) = &self.clip else { return };
        let k = k.min(clip.len().saturating_sub(1));
        if extend { self.anchor.get_or_insert(self.frame); } else { self.anchor = None; }
        self.frame = k;
    }

    /// Shows element `id`'s path over time (None hides it).
    pub fn pick(&mut self, id: Option<u32>) { self.picked = id; }

    /// The note typed so far.
    pub fn note(&self) -> &str { &self.note.text }

    pub fn set_note(&mut self, note: &str) {
        self.note = crate::TextEdit::default();
        self.note.insert(note);
    }

    /// What Send would hand on now.
    pub fn sent(&self) -> Option<Sent> {
        Some(Sent { clip: self.clip.clone()?, frames: self.selection(), note: self.note.text.clone() })
    }

    /// ←/→ step a frame (⇧ selects, ⌥ jumps to the next frame anything moved in), Home/End go
    /// to the ends, Return sends and Escape closes. Route the app's unhandled keys here while
    /// [`Scrub::is_open`].
    pub fn key(&mut self, ev: &KeyEvent) -> Keyed {
        let Some(clip) = self.clip.clone() else { return Keyed::Ignored };
        let extend = ev.mods.shift;
        let step = |dir: isize| if ev.mods.alt { clip.next_active(self.frame, dir) } else { self.frame.checked_add_signed(dir) };
        match ev.key {
            Key::Left => { if let Some(k) = step(-1) { self.seek(k, extend); } }
            Key::Right => { if let Some(k) = step(1) { self.seek(k, extend); } }
            Key::Home => self.seek(0, extend),
            Key::End => self.seek(clip.len(), extend),
            Key::Escape => { self.close(); return Keyed::Closed; }
            Key::Enter => return self.sent().map_or(Keyed::Handled, Keyed::Send),
            _ => return Keyed::Ignored,
        }
        Keyed::Handled
    }

    /// The scrubber over the whole window, or nothing while closed. Lay it over the app's view;
    /// it lines its outlines up with the window wherever it sits, so a story can show it on a
    /// stage. Pause the app's subtree (`El::time_scale(0.0)`) while it shows, not the window's
    /// clock: the scrubber itself still animates. `lens` finds this state in the app's; Send
    /// (the button, or Return in the note) calls `on_send`.
    pub fn view<S: 'static>(&self, lens: fn(&mut S) -> &mut Scrub, on_send: fn(&mut S, Sent, &mut Tx)) -> El {
        let Some(clip) = &self.clip else { return el() };
        let (vw, vh) = clip.viewport();
        let [ox, oy, w, _] = crate::memo::layout_with("rewind", |r| r.map(|v| (v * 2.0).round() / 2.0)).unwrap_or([0.0, 0.0, vw, vh]);
        let changes = clip.changes(self.frame);
        // The live view under it keeps changing; dimmed, the recorded frame reads over it.
        let layer = el().key("rewind").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0))
            .child(el().key("rewind-dim").pin(Some(0.0), Some(0.0), Some(0.0), Some(0.0)).fill(Color::BLACK.alpha(0.55))
                .on_press(move |s: &mut S| lens(s).picked = None));
        // Recorded frames: they land where they were, never springing between steps.
        let mut marks = el().key("rewind-marks").absolute(-ox, -oy).size(vw, vh).motion(crate::Spring::NONE);
        for (id, shot) in clip.frame(self.frame) {
            if changes.iter().any(|c| c.id == id) { continue; }
            let r = shot.rect;
            marks = marks.child(el().key(format!("rewind-el-{id}")).absolute(r[0], r[1]).size(r[2].max(1.0), r[3].max(1.0))
                .border(0.5, Color::WHITE.alpha(0.08 + 0.14 * shot.opacity.clamp(0.0, 1.0))));
        }
        if let Some(id) = self.picked { marks = marks.children(self.path_view(clip, id)); }
        for c in changes.iter().rev() { marks = marks.children(outline(c, lens)); }
        if let Some(id) = self.picked.filter(|id| !changes.iter().any(|c| c.id == *id))
            && let Some((_, s)) = clip.frame(self.frame).into_iter().find(|(i, _)| *i == id)
        {
            marks = marks.child(el().key("rewind-picked").absolute(s.rect[0], s.rect[1]).size(s.rect[2].max(1.0), s.rect[3].max(1.0)).border(1.0, Color::WHITE.alpha(0.7)));
        }
        layer.child(marks).child(self.bar(clip, w, &changes, lens, on_send))
    }

    fn path_view(&self, clip: &Clip, id: u32) -> Vec<El> {
        let sel = self.selection();
        let range = if sel.len() > 1 { sel } else { self.frame.saturating_sub(PATH_SPAN)..(self.frame + PATH_SPAN + 1).min(clip.len()) };
        let path = clip.path(id, range);
        let mut last: Option<Shot> = None;
        path.into_iter().filter_map(|(k, s)| {
            let moved = last.is_none_or(|l| l != s);
            last = Some(s);
            if !moved && k != self.frame { return None; }
            let (cx, cy) = (s.rect[0] + s.rect[2] / 2.0, s.rect[1] + s.rect[3] / 2.0);
            let r = if k == self.frame { 3.5 } else { 1.5 };
            let c = if clip.has_jump(k) { jump_color() } else { Color::WHITE.alpha(if k == self.frame { 1.0 } else { 0.55 }) };
            Some(el().key(format!("rewind-path-{k}")).absolute(cx - r, cy - r).size(r * 2.0, r * 2.0).radius(r).fill(c))
        }).collect()
    }

    fn bar<S: 'static>(&self, clip: &Clip, w: f32, changes: &[Change], lens: fn(&mut S) -> &mut Scrub, on_send: fn(&mut S, Sent, &mut Tx)) -> El {
        let t = tok::theme(true);
        let sel = self.selection();
        let moved = changes.iter().filter(|c| c.before.is_some() && c.after.is_some()).count();
        let mut facts = vec![format!("{:+.2} s", clip.time(self.frame)), format!("frame {} of {}", self.frame + 1, clip.len())];
        if moved > 0 { facts.push(format!("{moved} moved")); }
        if sel.len() > 1 { facts.push(format!("{} frames selected", sel.len())); }
        let track_w = (w - PAD * 2.0).max(1.0);
        let send = row().key("rewind-send").h(24.0).px(10.0).center_y().radius(tok::radius::SM).fill(t.card)
            .border(0.5, t.card_border).hover(t.hover).press(t.press)
            .role("button").label("Send these frames")
            .child(text("Send").font(type_::LABEL).color(t.text))
            .on_press_tx(move |s: &mut S, tx: &mut Tx| if let Some(sent) = lens(s).sent() { on_send(s, sent, tx); });
        let note = text_input(&self.note, type_::BODY).key("rewind-note").single_line().grow(1.0)
            .placeholder("What's wrong here?", t.text_secondary)
            .on_edit(move |s: &mut S| &mut lens(s).note)
            .on_submit(move |s: &mut S, tx: &mut Tx| if let Some(sent) = lens(s).sent() { on_send(s, sent, tx); });
        let mut controls = row().w_full().h(24.0).gap(12.0).center_y()
            .child(text(facts.join("  ·  ")).font(type_::MONO_CAPTION.mono()).color(t.text_secondary))
            .child(note);
        if w >= HINT_MIN_W {
            controls = controls.child(text("←→ step  ⇧ select  ⌥ next move  esc resume").font(type_::LABEL).color(t.text_secondary));
        }
        col().key("rewind-bar").pin(Some(0.0), None, Some(0.0), Some(0.0)).h(BAR_H).px(PAD).pt(8.0).gap(8.0)
            .fill(t.bg.alpha(0.96)).border(0.5, t.separator)
            .child(super::track::view(clip, self.frame, sel, track_w, lens))
            .child(controls.child(send))
            .child(spacer())
    }
}

/// An element that changed this frame: its old place dim, its new place outlined, and a label.
fn outline<S: 'static>(c: &Change, lens: fn(&mut S) -> &mut Scrub) -> Vec<El> {
    let Some(now) = c.after.or(c.before) else { return Vec::new() };
    let color = if c.jump { jump_color() } else { moved_color() };
    let id = c.id;
    let mut out = Vec::new();
    if let (Some(b), Some(_)) = (c.before, c.after) {
        out.push(el().key(format!("rewind-was-{id}")).absolute(b.rect[0], b.rect[1]).size(b.rect[2].max(1.0), b.rect[3].max(1.0)).border(1.0, color.alpha(0.35)));
    }
    let r = now.rect;
    out.push(el().key(format!("rewind-box-{id}")).absolute(r[0], r[1]).size(r[2].max(1.0), r[3].max(1.0))
        .border(if c.after.is_some() { 1.5 } else { 1.0 }, color.alpha(if c.after.is_some() { 1.0 } else { 0.5 }))
        .role("button").label(format!("Show {}'s path", c.name))
        .on_press(move |s: &mut S| { let scrub = lens(s); scrub.picked = if scrub.picked == Some(id) { None } else { Some(id) }; }));
    let above = r[1] >= 20.0;
    out.push(row().key(format!("rewind-label-{id}")).absolute(r[0], if above { r[1] - 19.0 } else { r[1] + r[3] + 2.0 }).h(17.0).px(5.0).center_y()
        .radius(3.0).fill(color).child(text(c.label()).font(type_::LABEL).color(Color::BLACK)));
    out
}
