use std::sync::Arc;

use parley::fontique::Blob;

use super::super::*;

const PLEX: &[u8] = include_bytes!("../../assets/IBMPlexMono-Regular.ttf");
const FULL: [&[u8]; 3] = [
    include_bytes!("../../assets/web/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/Inter-SemiBold.ttf"),
];
const FIRST: [&[u8]; 3] = [
    include_bytes!("../../assets/web/first/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/first/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/first/Inter-SemiBold.ttf"),
];

/// Each glyph's pen position and advance, and whether it's the missing-glyph box.
fn shaped(ts: &mut TextSystem, texts: &[&str]) -> Vec<(u32, u32, bool)> {
    let mut out = Vec::new();
    for text in texts {
        for style in [TextStyle::new(13.0, 400.0), TextStyle::new(15.0, 500.0), TextStyle::new(24.0, 600.0)] {
            let layout = ts.layout_rich(text, &[], style, 2.0, None);
            for line in layout.lines() {
                for item in line.items() {
                    if let PositionedLayoutItem::GlyphRun(run) = item {
                        out.extend(run.glyphs().map(|g| (g.x.to_bits(), g.advance.to_bits(), g.id == 0)));
                    }
                }
            }
        }
    }
    out
}

const FIRST_TEXTS: [&str; 5] = [
    "Rune → office “quotes” fi ffl AVAWAY 1/2 ½ €100 ™ — …",
    "Ýüñçø Łódź Čeština Ştefan Øresund Œuvre ß",
    "⌘⇧⌥⌃ ⌫ ⎋ ↩ ⏎ «guillemets» ‹›",
    "Ωμέγα Ελληνικά ΐΰ ϊϋ ς; Άλφα",
    "Привет, мир! Ґанок ЇЄІ Ђорђе Љубљана ЎЏ",
];

/// The cut faces set Latin, Greek and Cyrillic text exactly as the full ones do: same glyphs,
/// kerning and ligatures, at the same positions, none of them missing.
#[test]
fn the_first_cut_sets_latin_greek_and_cyrillic_like_the_full_faces() {
    let mut cut = TextSystem::new(&[FIRST[0], FIRST[1], FIRST[2], PLEX], ("Inter", "IBM Plex Mono"));
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let (a, b) = (shaped(&mut cut, &FIRST_TEXTS), shaped(&mut full, &FIRST_TEXTS));
    assert!(a.len() > 400);
    assert!(a.iter().all(|g| !g.2), "a glyph is missing from the cut");
    assert_eq!(a, b);
}

/// Swapping the full faces in sets text the cut lacked as the full faces do, and leaves the
/// text the cut had where it was.
#[test]
fn swapping_in_the_full_faces_sets_what_the_cut_lacked() {
    let texts = ["Tiếng Việt ἀρχή Әліппе"];
    let mut full = TextSystem::new(&[FULL[0], FULL[1], FULL[2], PLEX], ("Inter", "IBM Plex Mono"));
    let want = shaped(&mut full, &texts);
    let mut ts = TextSystem::new(&[FIRST[0], FIRST[1], FIRST[2], PLEX], ("Inter", "IBM Plex Mono"));
    assert_ne!(shaped(&mut ts, &texts), want, "the cut has Vietnamese, polytonic Greek or Kazakh");
    let first = shaped(&mut ts, &FIRST_TEXTS);
    ts.swap_faces(&FULL.map(|f| Blob::new(Arc::new(f))));
    ts.drop_shaped();
    assert_eq!(shaped(&mut ts, &texts), want);
    assert_eq!(shaped(&mut ts, &FIRST_TEXTS), first);
}
