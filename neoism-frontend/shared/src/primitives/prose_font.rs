//! Proportional UI documentation text, independent of terminal and notes fonts.

use std::sync::OnceLock;
use sugarloaf::font::{constants::FONT_UI_PROSE, FontData, FontLibrary};

/// Resolve against this library, including after a browser font-registry reset.
/// Cache only the bundled identity, never an ID belonging to another library.
pub fn ui_prose_font_id(fonts: &FontLibrary) -> Option<usize> {
    static NAME: OnceLock<Option<String>> = OnceLock::new();
    if let Some(name) = NAME.get_or_init(|| {
        let face = FontData::from_static_slice(FONT_UI_PROSE).ok()?;
        face.postscript_name().map(str::to_owned)
    }) {
        if let Some(id) = fonts.font_id_for_postscript_name(name) {
            return Some(id);
        }
    }
    fonts.ensure_static_font(FONT_UI_PROSE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use sugarloaf::font::constants::{FONT_CASCADIAMONO_NF_REGULAR, FONT_PRESS_START_2P};
    use sugarloaf::text::{DrawOpts, Text};

    #[test]
    fn prose_font_registers_without_fixed_ids_and_deduplicates() {
        let fonts = FontLibrary {
            inner: Default::default(),
        };
        fonts
            .ensure_static_font(FONT_CASCADIAMONO_NF_REGULAR)
            .unwrap();
        let pixel_id = fonts.ensure_static_font(FONT_PRESS_START_2P).unwrap();
        let before = fonts.inner.read().inner.len();
        let id = ui_prose_font_id(&fonts).expect("bundled prose font parses");
        assert_ne!(id, pixel_id);
        assert_eq!(fonts.font_id_for_family("Geist"), Some(id));
        assert_eq!(fonts.inner.read().inner.len(), before + 1);
        assert_eq!(ui_prose_font_id(&fonts), Some(id));
        assert_eq!(fonts.inner.read().inner.len(), before + 1);

        let reset = FontLibrary {
            inner: Default::default(),
        };
        let reset_id = ui_prose_font_id(&reset).unwrap();
        assert_eq!(reset.font_id_for_family("Geist"), Some(reset_id));
        assert_eq!(reset.inner.read().inner.len(), 1);
    }

    #[test]
    fn prose_font_has_proportional_advances() {
        let fonts = FontLibrary {
            inner: Default::default(),
        };
        fonts
            .ensure_static_font(FONT_CASCADIAMONO_NF_REGULAR)
            .unwrap();
        let id = ui_prose_font_id(&fonts).unwrap();
        let mut text = Text::new(&fonts);
        text.init_cpu();
        let opts = DrawOpts {
            font_size: 16.0,
            font_id: Some(id),
            ..DrawOpts::default()
        };
        assert!(text.measure("WWWW", &opts) > text.measure("iiii", &opts) * 2.0);
        assert!(!opts.bold);
    }
}
