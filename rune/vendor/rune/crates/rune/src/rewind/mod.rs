//! Rewind: an opt-in recorder of the last minute of what a window presented, and a scrubber to
//! step back through it and hand a stretch to someone.
//!
//! Attach a [`Rewind`] with [`Tx::record_rewind`](crate::Tx::record_rewind) and every presented
//! frame records the rect and opacity of each keyed or trace-named element that changed, delta
//! encoded, in a ring that keeps the last 60 seconds (or less, within a byte budget). Nothing is
//! attached by default, and then a frame does nothing more than check for it.
//!
//! [`Rewind::freeze`] pauses recording and hands back a [`Clip`]: any frame's elements, what
//! changed in each frame and by how much, and an element's path over time. [`Scrub`] is a
//! ready-made scrubber over a clip: the view lays [`Scrub::view`] over its own, with its own
//! subtree's clock paused (`El::time_scale(0.0)`), and Send gives the app the selected
//! stretch, which [`Clip::write_bundle`] turns into a folder (a motion-lint trace, a contact
//! sheet and a summary).
//!
//! ```ignore
//! // Start recording (a setting turned on):
//! tx.record_rewind(Some(&s.rewind));
//! // A shortcut freezes it:
//! s.scrub.open(&s.rewind);
//! // The view, always wrapped the same way so the app's elements keep their paths:
//! el().child(app_view.time_scale(if s.scrub.is_open() { 0.0 } else { 1.0 }))
//!     .child(s.scrub.view(|s: &mut App| &mut s.scrub, |s, sent, tx| { ... }))
//! ```

mod bundle;
mod clip;
mod scrub;
mod tape;
mod track;

#[cfg(test)]
mod tests;

pub use bundle::Bundle;
pub use clip::{Change, Clip, Shot};
pub use scrub::{Keyed, Scrub, Sent};

pub(crate) use tape::quantize;

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

/// A recorder handle: cheap to clone, shared by the app and the window it's attached to.
#[derive(Clone)]
pub struct Rewind {
    tape: Arc<Mutex<tape::Tape>>,
}

impl Default for Rewind {
    fn default() -> Self { Self::new() }
}

impl std::fmt::Debug for Rewind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rewind").field("frames", &self.frames()).field("bytes", &self.bytes()).finish()
    }
}

impl Rewind {
    /// Keeps the last 60 seconds, within 24 MB.
    pub fn new() -> Self {
        Rewind { tape: Arc::new(Mutex::new(tape::Tape::new(60.0, 24 << 20))) }
    }

    /// Keep the last `seconds` (at least one frame is always kept).
    pub fn seconds(self, seconds: f64) -> Self {
        self.lock().window = seconds.max(0.0);
        self
    }

    /// Drop the oldest frames once the recording holds more than `bytes`.
    pub fn max_bytes(self, bytes: usize) -> Self {
        self.lock().max_bytes = bytes;
        self
    }

    /// Pauses recording and copies out what it holds. [`Rewind::resume`] carries on.
    pub fn freeze(&self) -> Arc<Clip> {
        let mut tape = self.lock();
        tape.paused = true;
        Arc::new(Clip::from_tape(&tape))
    }

    /// Records again after [`Rewind::freeze`], from where it left off.
    pub fn resume(&self) {
        self.lock().paused = false;
    }

    pub fn is_paused(&self) -> bool { self.lock().paused }

    /// Frames held.
    pub fn frames(&self) -> usize { self.lock().frames.len() }

    /// Bytes the recording holds, roughly.
    pub fn bytes(&self) -> usize { self.lock().bytes() }

    pub(crate) fn lock(&self) -> MutexGuard<'_, tape::Tape> {
        self.tape.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
