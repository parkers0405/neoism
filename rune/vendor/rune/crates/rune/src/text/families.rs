//! The families behind the sans and mono slots, and changing them while text is cached.

use std::cell::RefCell;

use super::{TextSystem, measure, names_system_mono};

thread_local! {
    /// The families last chosen for every window (`Tx::set_font_families`), numbered by change.
    /// Windows and overlays share the main thread; each text system follows on its next frame.
    static CHOSEN: RefCell<(u64, Option<(String, String)>)> = const { RefCell::new((0, None)) };
}

/// Chooses `sans` and `mono` for every window on this thread. Choosing the families already
/// chosen changes nothing.
pub(crate) fn choose_families(sans: &str, mono: &str) {
    CHOSEN.with_borrow_mut(|(n, chosen)| {
        if chosen.as_ref().is_none_or(|(s, m)| s != sans || m != mono) {
            *n += 1;
            *chosen = Some((sans.to_owned(), mono.to_owned()));
        }
    });
}

/// The slot strings for `sans` and `mono`. Web has no system fonts to fall back on, so its
/// slots end in the bundled faces.
pub(super) fn slots(sans: &str, mono: &str) -> (String, String) {
    if cfg!(target_arch = "wasm32") {
        (format!("{sans}, Inter"), format!("{mono}, IBM Plex Mono"))
    } else {
        (sans.to_owned(), mono.to_owned())
    }
}

impl TextSystem {
    /// A window's or overlay's text system: in the families chosen for every window when the
    /// app has chosen, else in `families`. Opening one after a switch then never sends
    /// [`measure`](super::measure) back to the launch families.
    pub(crate) fn for_window(fonts: &[&'static [u8]], families: (&str, &str)) -> Self {
        CHOSEN.with_borrow(|(n, chosen)| {
            let families = chosen.as_ref().map_or(families, |(sans, mono)| (sans.as_str(), mono.as_str()));
            let mut text = TextSystem::new(fonts, families);
            text.chosen = *n;
            text
        })
    }

    /// The sans and mono slots' families (CSS lists), as [`TextSystem::new`] or
    /// [`Self::set_families`] named them.
    pub fn families(&self) -> (&str, &str) {
        (&self.families.0, &self.families.1)
    }

    /// Puts `sans` and `mono` (CSS lists, as `AppSpec::families` takes them) behind the slots.
    /// Styles name slots, not families, so every `Family::Sans` and `Family::Mono` text follows.
    /// Every layout shaped in the old families goes, and [`measure`](super::measure) follows
    /// too. Glyphs stay in the atlas under their own font's key until they age out. A family
    /// that isn't registered or installed falls back as it would at launch. Returns false,
    /// dropping nothing, when the slots already hold these families.
    pub fn set_families(&mut self, sans: &str, mono: &str) -> bool {
        if self.families.0 == sans && self.families.1 == mono {
            return false;
        }
        measure::note_families(sans, mono);
        self.load_installed(sans);
        self.load_installed(mono);
        self.families = (sans.to_owned(), mono.to_owned());
        (self.sans, self.mono) = slots(sans, mono);
        self.slot_system_mono = names_system_mono(sans) || names_system_mono(mono);
        self.drop_shaped();
        true
    }

    /// Drops every layout, run and measurement shaped so far, for new families or a new
    /// language. Glyphs stay in the atlas under their own font's key until they age out.
    pub(super) fn drop_shaped(&mut self) {
        self.layouts.clear();
        self.shapes.clear();
        self.superseded.clear();
        self.elisions.clear();
        self.runs.clear();
        self.primary_traks.clear();
        self.primary_axes.clear();
        self.glyph_cache.clear();
    }

    /// Whether families were chosen since this text system last followed them.
    pub(crate) fn families_behind(&self) -> bool {
        CHOSEN.with_borrow(|(n, _)| *n != self.chosen)
    }

    /// Takes up the families chosen since it last looked; true when that changed its slots.
    pub(crate) fn follow_chosen(&mut self) -> bool {
        CHOSEN.with_borrow(|(n, chosen)| {
            if *n == self.chosen {
                return false;
            }
            self.chosen = *n;
            chosen.as_ref().is_some_and(|(sans, mono)| self.set_families(sans, mono))
        })
    }
}
