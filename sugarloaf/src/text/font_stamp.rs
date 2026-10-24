//! Observe public FontLibrary mutations without hashing font bytes or text.
//! FontLibraryData has no mutation epoch and exposes its maps publicly: exact
//! observation therefore requires O(registered faces + symbol ranges) metadata
//! comparisons under one read lock. Warm observation allocates nothing.
use super::*;

struct FaceStamp {
    key: swash::CacheKey,
    data: Option<crate::font::SharedData>,
    offset: u32,
    path: Option<std::path::PathBuf>,
    postscript_name: Option<String>,
    family_name: Option<String>,
    weight: swash::Weight,
    style: swash::Style,
    stretch: swash::Stretch,
    synth: swash::Synthesis,
    bold: bool,
    italic: bool,
    emoji: bool,
}
impl FaceStamp {
    fn new(font: &crate::font::FontData) -> Self {
        Self {
            key: font.key,
            data: font.data().clone(),
            offset: font.offset(),
            path: font.path().cloned(),
            postscript_name: font.postscript_name().map(str::to_owned),
            family_name: font.family_name().map(str::to_owned),
            weight: font.weight,
            style: font.style,
            stretch: font.stretch,
            synth: font.synth,
            bold: font.should_embolden,
            italic: font.should_italicize,
            emoji: font.is_emoji,
        }
    }
    fn matches(&self, font: &crate::font::FontData) -> bool {
        let same_data = match (&self.data, font.data()) {
            (None, None) => true,
            (Some(a), Some(b)) => {
                a.as_ref().as_ptr() == b.as_ref().as_ptr()
                    && a.as_ref().len() == b.as_ref().len()
            }
            _ => false,
        };
        self.key == font.key
            && same_data
            && self.offset == font.offset()
            && self.path.as_ref() == font.path()
            && self.postscript_name.as_deref() == font.postscript_name()
            && self.family_name.as_deref() == font.family_name()
            && self.weight == font.weight
            && self.style == font.style
            && self.stretch == font.stretch
            && self.synth.embolden() == font.synth.embolden()
            && self.synth.skew() == font.synth.skew()
            && self.synth.variations().len() == font.synth.variations().len()
            && self
                .synth
                .variations()
                .iter()
                .zip(font.synth.variations())
                .all(|(a, b)| a.tag == b.tag && a.value.to_bits() == b.value.to_bits())
            && self.bold == font.should_embolden
            && self.italic == font.should_italicize
            && self.emoji == font.is_emoji
    }
}

// One bounded-by-current-library snapshot, not a history of past identities.
// SharedData pins immutable allocations so replacement cannot alias a freed
// allocation even when a caller reuses FontData's public swash key.
pub(super) struct FontStamp {
    faces: FxHashMap<usize, FaceStamp>,
    symbols: Option<Vec<(usize, std::ops::Range<char>)>>,
    hinting: bool,
}
pub(super) enum FontChange {
    Unchanged,
    Appended,
    Replaced,
}

impl FontStamp {
    pub(super) fn new(library: &FontLibrary) -> Self {
        let lib = library.inner.read();
        Self {
            faces: lib
                .inner
                .iter()
                .map(|(id, font)| (*id, FaceStamp::new(font)))
                .collect(),
            symbols: lib.symbol_maps.as_ref().map(|maps| {
                maps.iter()
                    .map(|m| (m.font_index, m.range.clone()))
                    .collect()
            }),
            hinting: lib.hinting,
        }
    }
    pub(super) fn change(&self, library: &FontLibrary) -> FontChange {
        let lib = library.inner.read();
        let preserves_existing = self.hinting == lib.hinting
            && self
                .faces
                .iter()
                .all(|(id, face)| lib.inner.get(id).is_some_and(|f| face.matches(f)))
            && match (&self.symbols, &lib.symbol_maps) {
                (None, None) => true,
                (Some(a), Some(b)) => {
                    a.len() == b.len()
                        && a.iter()
                            .zip(b)
                            .all(|(a, b)| a.0 == b.font_index && a.1 == b.range)
                }
                _ => false,
            };
        if !preserves_existing {
            FontChange::Replaced
        } else if self.faces.len() != lib.inner.len() {
            FontChange::Appended
        } else {
            FontChange::Unchanged
        }
    }
}
