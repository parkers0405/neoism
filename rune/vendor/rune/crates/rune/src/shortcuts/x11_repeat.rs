//! X11 autorepeat filter for grabbed shortcuts. Without detectable autorepeat the server sends
//! each repeat as a KeyRelease and a KeyPress with the same timestamp; with it, as KeyPresses
//! with no release between. Either way only the first press of a hold fires, as on macOS and
//! Windows. A release is held back until the next event shows whether a repeat follows it.

#[derive(Default)]
pub(super) struct Repeats {
    down: Vec<u8>,
    release: Option<(u8, u32)>,
}

impl Repeats {
    /// Whether this press starts a hold (fire) rather than repeating one.
    pub(super) fn press(&mut self, code: u8, time: u32) -> bool {
        match self.release.take() {
            Some(release) if release == (code, time) => return false,
            Some((released, _)) => self.forget(released),
            None => {}
        }
        if self.down.contains(&code) { return false; }
        self.down.push(code);
        true
    }

    pub(super) fn release(&mut self, code: u8, time: u32) {
        if let Some((released, _)) = self.release.replace((code, time)) {
            self.forget(released);
        }
    }

    /// The first key down started the active grab, which ends with its release; keys pressed
    /// during the grab send their releases elsewhere, so they are forgotten with it.
    fn forget(&mut self, released: u8) {
        if self.down.first() == Some(&released) { self.down.clear(); } else { self.down.retain(|&c| c != released); }
    }
}

#[cfg(test)]
mod tests {
    use super::Repeats;

    #[test]
    fn a_held_key_fires_once_in_either_autorepeat_mode() {
        let mut r = Repeats::default();
        // Release/press pairs with matching timestamps.
        assert!(r.press(38, 10));
        r.release(38, 40); assert!(!r.press(38, 40));
        r.release(38, 70); assert!(!r.press(38, 70));
        r.release(38, 90);
        assert!(r.press(38, 300), "a new press after a real release fires");
        // Detectable autorepeat: presses without releases.
        assert!(!r.press(38, 330));
        assert!(!r.press(38, 360));
        r.release(38, 400);
        assert!(r.press(39, 410), "another key fires and settles the earlier release");
        assert!(r.press(38, 420));
    }

    #[test]
    fn keys_pressed_during_a_grab_are_forgotten_when_the_grab_key_is_released() {
        let mut r = Repeats::default();
        assert!(r.press(65, 10)); // the grabbed chord's key starts the active grab
        assert!(r.press(38, 20)); // another key arrives through the grab
        r.release(65, 30); // the grab ends; 38's release goes to the focused window instead
        assert!(r.press(38, 400), "a later fresh press of the other key still fires");
        assert!(r.press(65, 410));
    }
}
