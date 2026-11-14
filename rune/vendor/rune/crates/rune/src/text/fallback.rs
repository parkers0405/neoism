//! System fallbacks loaded on first use: fonts for symbol blocks the eager symbol fallbacks
//! miss (math alphanumerics, enclosed alphanumerics, ...), and byte ranges of a text that should
//! try a fallback's fonts before the style's own.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

/// Why a byte range is routed ahead of the style's own fonts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Route {
    /// Mathematical Alphanumeric Symbols: one math font for a whole formula, so bold, italic
    /// and script letters don't mix designs when an earlier fallback covers a few of them.
    Math,
    /// A grapheme asking for emoji presentation with `U+FE0F` (`❤️`, `☀️`, `✔️`): the colour
    /// emoji font first, so a text font with a plain `❤` doesn't take it.
    Emoji,
}

// Math alphanumerics and other symbol blocks. Loaded only when a text has a character in
// `needs_symbols`, and appended after the eager symbol families so existing picks don't move.
#[cfg(target_os = "macos")]
pub(super) const MATH_FONT_FILES: &[&str] = &["stixtwomath"];
#[cfg(target_os = "macos")]
pub(super) const MATH_FAMILIES: &[&str] = &["STIX Two Math"];
#[cfg(windows)]
pub(super) const MATH_FONT_FILES: &[&str] = &["cambriattc"];
#[cfg(windows)]
pub(super) const MATH_FAMILIES: &[&str] = &["Cambria Math"];
#[cfg(not(any(target_os = "macos", windows)))]
pub(super) const MATH_FONT_FILES: &[&str] = &["notosansmath", "stixtwomath", "notomusic", "symbola"];
#[cfg(not(any(target_os = "macos", windows)))]
pub(super) const MATH_FAMILIES: &[&str] = &["Noto Sans Math", "STIX Two Math", "Noto Music", "Symbola"];

/// A script the app's fonts usually lack, the blocks that use it and the system font files
/// (lowercase, without spaces or dots, most preferred first) that cover it. Loaded the first
/// time a text uses one of the blocks, as fallbacks for that script only.
pub(super) struct ScriptFonts {
    pub script: &'static [u8; 4],
    pub blocks: &'static [(u32, u32)],
    pub files: &'static [&'static str],
}

impl ScriptFonts {
    pub(super) fn covers(&self, ch: char) -> bool {
        self.blocks.iter().any(|&(lo, hi)| (lo..=hi).contains(&(ch as u32)))
    }
}

const HEBREW: &[(u32, u32)] = &[(0x0590, 0x05FF), (0xFB1D, 0xFB4F)];
const ARABIC: &[(u32, u32)] = &[(0x0600, 0x06FF), (0x0750, 0x077F), (0x0870, 0x08FF), (0xFB50, 0xFDFF), (0xFE70, 0xFEFF)];
const DEVANAGARI: &[(u32, u32)] = &[(0x0900, 0x097F), (0xA8E0, 0xA8FF)];
const BENGALI: &[(u32, u32)] = &[(0x0980, 0x09FF)];
const THAI: &[(u32, u32)] = &[(0x0E00, 0x0E7F)];

#[cfg(target_os = "macos")]
pub(super) const SCRIPT_FONTS: &[ScriptFonts] = &[
    ScriptFonts { script: b"Hebr", blocks: HEBREW, files: &["sfhebrewttf", "arialhb"] },
    ScriptFonts { script: b"Arab", blocks: ARABIC, files: &["sfarabicttf", "geezapro"] },
    ScriptFonts { script: b"Deva", blocks: DEVANAGARI, files: &["kohinoorttc", "itfdevanagari"] },
    ScriptFonts { script: b"Beng", blocks: BENGALI, files: &["kohinoorbangla", "banglamn"] },
    ScriptFonts { script: b"Thai", blocks: THAI, files: &["thonburi"] },
];
#[cfg(windows)]
pub(super) const SCRIPT_FONTS: &[ScriptFonts] = &[
    ScriptFonts { script: b"Hebr", blocks: HEBREW, files: &["segoeuittf", "arialttf"] },
    ScriptFonts { script: b"Arab", blocks: ARABIC, files: &["segoeuittf", "arialttf"] },
    ScriptFonts { script: b"Deva", blocks: DEVANAGARI, files: &["nirmala"] },
    ScriptFonts { script: b"Beng", blocks: BENGALI, files: &["nirmala"] },
    ScriptFonts { script: b"Thai", blocks: THAI, files: &["leelawui", "tahomattf"] },
];
#[cfg(not(any(target_os = "macos", windows)))]
pub(super) const SCRIPT_FONTS: &[ScriptFonts] = &[
    ScriptFonts { script: b"Hebr", blocks: HEBREW, files: &["notosanshebrew", "freesans"] },
    ScriptFonts { script: b"Arab", blocks: ARABIC, files: &["notosansarabic", "notonaskharabic", "freesans"] },
    ScriptFonts { script: b"Deva", blocks: DEVANAGARI, files: &["notosansdevanagari", "lohitdevanagari", "freesans"] },
    ScriptFonts { script: b"Beng", blocks: BENGALI, files: &["notosansbengali", "lohitbengali", "freesans"] },
    ScriptFonts { script: b"Thai", blocks: THAI, files: &["notosansthai", "tlwg", "freesans"] },
];

/// Whether `ch` is in a symbol block the eager symbol fallbacks often miss. General punctuation
/// and currency (dashes, quotes, €) are left out: Latin prose is full of them.
pub(super) fn needs_symbols(ch: char) -> bool {
    matches!(ch as u32,
        0x2100..=0x2BFF | 0x2E00..=0x2E7F | 0x1D000..=0x1D7FF | 0x1F000..=0x1F0FF | 0x1F100..=0x1F1E5
        | 0x1F650..=0x1F67F | 0x1F700..=0x1F8FF | 0x1FA00..=0x1FA6F | 0x1FB00..=0x1FBFF)
}

/// Math alphanumerics, plus the Letterlike Symbols that fill that block's holes (`ℝ`, `ℎ`,
/// `ℬ`, ...), so double-struck and script runs keep one design.
pub(super) fn is_math_alphanumeric(ch: char) -> bool {
    matches!(ch as u32,
        0x1D400..=0x1D7FF | 0x2102 | 0x210A..=0x2113 | 0x2115 | 0x2119..=0x211D | 0x2124 | 0x2128 | 0x212C..=0x212D
        | 0x212F..=0x2131 | 0x2133..=0x2134 | 0x2145..=0x2149)
}

/// Routed byte ranges of `text`, merging neighbours on the same route.
pub(super) fn routes(text: &str) -> Vec<(Range<usize>, Route)> {
    let mut out = Vec::new();
    if text.is_ascii() {
        return out;
    }
    let mut push = |range: Range<usize>, route| match out.last_mut() {
        Some((last, r)) if *r == route && last.end == range.start => last.end = range.end,
        _ => out.push((range, route)),
    };
    // Whole graphemes, so a ZWJ sequence with an `FE0F` inside keeps one font.
    for (i, g) in text.grapheme_indices(true) {
        if g.contains('\u{FE0F}') {
            push(i..i + g.len(), Route::Emoji);
            continue;
        }
        for (j, ch) in g.char_indices() {
            if is_math_alphanumeric(ch) {
                push(i + j..i + j + ch.len_utf8(), Route::Math);
            }
        }
    }
    out
}

