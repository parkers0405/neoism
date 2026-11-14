//! A web app embeds Inter cut to Latin, Greek and Cyrillic text (`scripts/web-fonts.sh`) and
//! fetches the full faces after its first frame. When they arrive, every text system registers
//! them in place of the cut ones and drops what it shaped. The cut faces keep each of their
//! glyphs' outline, advance, kerning and layout features, so text they cover stays where it is
//! and only text they lacked moves.

use std::sync::OnceLock;

use parley::fontique::Blob;

use super::TextSystem;

static ARRIVED: OnceLock<Vec<Blob<u8>>> = OnceLock::new();

/// The full faces' files, which a web build serves beside its `app.js`.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(crate) const FILES: &[&str] = &["Inter-Regular.ttf", "Inter-Medium.ttf", "Inter-SemiBold.ttf"];

/// Hands over the full faces: measurement uses them from now on, and every text system from
/// its next frame. Only the first call counts.
#[cfg(all(target_arch = "wasm32", feature = "embedded-fonts"))]
pub(crate) fn arrive(faces: Vec<Vec<u8>>) {
    if ARRIVED.set(faces.into_iter().map(|f| Blob::new(std::sync::Arc::new(f))).collect()).is_ok() {
        super::measure::note_faces_arrived();
    }
}

impl TextSystem {
    /// Takes up the full faces if they arrived since it last looked; true when it did.
    pub(crate) fn follow_full_faces(&mut self) -> bool {
        let Some(faces) = ARRIVED.get().filter(|_| !self.full_faces) else { return false };
        self.full_faces = true;
        self.swap_faces(faces);
        self.drop_shaped();
        true
    }

    /// Registers each face in place of those with its family, width, style and weight.
    pub(super) fn swap_faces(&mut self, faces: &[Blob<u8>]) {
        let collection = &mut self.font_cx.collection;
        for blob in faces {
            for (family, fonts) in collection.register_fonts(blob.clone(), None) {
                for font in fonts {
                    collection.unregister_font(family, font.width(), font.style(), font.weight());
                }
            }
            collection.register_fonts(blob.clone(), None);
        }
    }
}

#[cfg(test)]
#[path = "full_faces_tests.rs"]
mod tests;
