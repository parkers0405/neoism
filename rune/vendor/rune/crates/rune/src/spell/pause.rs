//! When a field marks the misspelled word its caret is in, as Chromium's idle spell checker
//! does (`idle_spell_check_controller.cc`, `hot_mode_spell_check_requester.cc`): while the caret
//! stands where typing left it, the word it's in waits (`CurrentWordIfTypingInPartialWord`);
//! the full check that marks it runs a second after the typing stops (`kColdModeTimerInterval`).
//! Moving the caret without typing checks the word it leaves at once, as the selection change
//! does.

/// From the last keystroke to the check that marks the word being typed.
pub(crate) const PAUSE: f64 = 1.0;

/// The focused field's word at the caret.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Pause {
    /// The caret at byte `caret` is where typing left it; its word shows unmarked until `until`
    /// (the runtime clock, seconds).
    Typing { caret: usize, until: f64 },
    /// Every misspelled word shows marked.
    Settled,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Event {
    /// The text changed under the caret, now at byte `caret`.
    Typed { caret: usize, now: f64 },
    /// The caret moved, or a range was selected, with no typing.
    CaretMoved,
    /// The clock reached `now`.
    Tick { now: f64 },
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Effect {
    /// Wake the runtime at `at` (the runtime clock) to end the pause.
    WakeAt(f64),
    /// The word at the caret now shows its mark: draw again.
    Redraw,
}

pub(crate) fn step(state: Pause, event: Event) -> (Pause, Option<Effect>) {
    match (state, event) {
        (_, Event::Typed { caret, now }) => (Pause::Typing { caret, until: now + PAUSE }, Some(Effect::WakeAt(now + PAUSE))),
        (Pause::Typing { .. }, Event::CaretMoved) => (Pause::Settled, Some(Effect::Redraw)),
        (Pause::Typing { until, .. }, Event::Tick { now }) if now >= until => (Pause::Settled, Some(Effect::Redraw)),
        (state, _) => (state, None),
    }
}

/// The byte whose word waits unmarked: the caret, while typing has it.
pub(crate) fn waiting(state: Pause) -> Option<usize> {
    match state {
        Pause::Typing { caret, .. } => Some(caret),
        Pause::Settled => None,
    }
}

/// What the runtime last saw of the focused spellchecked field: its path, text (hashed) and
/// selection, the machine, and a wake the paint asked for, which the next frame schedules.
#[derive(Clone, Debug)]
pub(crate) struct Watch {
    path: String,
    text: u64,
    range: (usize, usize),
    pause: Pause,
    pub(crate) wake: Option<f64>,
}

/// Steps the focused field `path`'s machine by what changed since the last paint, and returns
/// the byte whose word waits unmarked. A field just focused starts settled, as the selection
/// change that focuses it checks it.
pub(crate) fn observe(watch: &mut Option<Watch>, path: &str, text: &str, range: (usize, usize), now: f64) -> Option<usize> {
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::collections::hash_map::DefaultHasher::new();
        text.hash(&mut h);
        h.finish()
    };
    let w = match watch {
        Some(w) if w.path == path => w,
        _ => watch.insert(Watch { path: path.to_owned(), text: hash, range, pause: Pause::Settled, wake: None }),
    };
    let event = if w.text != hash {
        Some(Event::Typed { caret: range.1, now })
    } else if w.range != range {
        Some(Event::CaretMoved)
    } else {
        None
    };
    (w.text, w.range) = (hash, range);
    if let Some(e) = event {
        let (next, effect) = step(w.pause, e);
        w.pause = next;
        if let Some(Effect::WakeAt(at)) = effect {
            w.wake = Some(at);
        }
    }
    waiting(w.pause).filter(|_| range.0 == range.1)
}

/// The frame's turn: hands over the wake the paint asked for, and ends a pause that's due.
/// Returns whether the field must draw again.
pub(crate) fn tick(watch: &mut Option<Watch>, now: f64) -> (Option<f64>, bool) {
    let Some(w) = watch else { return (None, false) };
    let wake = w.wake.take();
    let (next, effect) = step(w.pause, Event::Tick { now });
    w.pause = next;
    (wake, effect == Some(Effect::Redraw))
}
