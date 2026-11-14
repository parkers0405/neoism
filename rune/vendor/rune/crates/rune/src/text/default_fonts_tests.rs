use super::super::*;
use swash::FontRef;
use swash::scale::ScaleContext;

const PLEX: &[u8] = include_bytes!("../../assets/IBMPlexMono-Regular.ttf");
const NATIVE: [&[u8]; 3] = [
    include_bytes!("../../assets/Inter-Regular.ttf"),
    include_bytes!("../../assets/Inter-Medium.ttf"),
    include_bytes!("../../assets/Inter-SemiBold.ttf"),
];
const WEB: [&[u8]; 3] = [
    include_bytes!("../../assets/web/Inter-Regular.ttf"),
    include_bytes!("../../assets/web/Inter-Medium.ttf"),
    include_bytes!("../../assets/web/Inter-SemiBold.ttf"),
];

fn tag(s: &[u8; 4]) -> u32 { u32::from_be_bytes(*s) }

/// The web copies drop only what Rune's own rasterizer never reads (TrueType hinting and glyph
/// names): every glyph's outline and advance, the character map and the metrics match.
#[test]
fn web_fonts_keep_every_outline_advance_and_mapping() {
    let mut cx = ScaleContext::new();
    for (native, web) in NATIVE.iter().zip(WEB) {
        let (a, b) = (FontRef::from_index(native, 0).unwrap(), FontRef::from_index(web, 0).unwrap());
        assert!(web.len() < native.len() * 3 / 4, "web copy should be smaller: {} vs {}", web.len(), native.len());
        for t in [b"fpgm", b"prep", b"cvt "] {
            assert!(b.table(tag(t)).is_none(), "web copy still has hinting table {}", String::from_utf8_lossy(t));
        }
        assert_eq!(format!("{:?}", a.metrics(&[])), format!("{:?}", b.metrics(&[])));
        let count = a.metrics(&[]).glyph_count;
        assert_eq!(count, b.metrics(&[]).glyph_count);
        for c in (0..=0x10FFFFu32).filter_map(char::from_u32) {
            assert_eq!(a.charmap().map(c), b.charmap().map(c), "cmap differs at {c:?}");
        }
        let (ma, mb) = (a.glyph_metrics(&[]), b.glyph_metrics(&[]));
        let mut sa = cx.builder(a).hint(false).build();
        let outlines: Vec<_> = (0..count).map(|g| sa.scale_outline(g).map(|o| (o.points().to_vec(), o.verbs().to_vec()))).collect();
        let mut sb = cx.builder(b).hint(false).build();
        for g in 0..count {
            assert_eq!(ma.advance_width(g), mb.advance_width(g), "advance differs for glyph {g}");
            let other = sb.scale_outline(g).map(|o| (o.points().to_vec(), o.verbs().to_vec()));
            assert_eq!(outlines[g as usize], other, "outline differs for glyph {g}");
        }
    }
}

/// Shaping (ligatures, kerning, weights, mono) gives the same glyphs at the same positions.
#[test]
fn web_fonts_shape_like_the_native_ones() {
    let shaped = |fonts: &[&'static [u8]]| {
        let mut ts = TextSystem::new(fonts, ("Inter", "IBM Plex Mono"));
        let mut out = Vec::new();
        for text in ["Rune → office “quotes” fi ffl AVAWAY 1/2 ½ €100", "Ýüñçø Ωμέγα Привет", "fn main() { let x = a != b; } // -> =>"] {
            for style in [TextStyle::new(13.0, 400.0), TextStyle::new(15.0, 500.0), TextStyle::new(24.0, 600.0), TextStyle::new(13.0, 400.0).mono()] {
                let layout = ts.layout_rich(text, &[], style, 2.0, None);
                for line in layout.lines() {
                    for item in line.items() {
                        if let PositionedLayoutItem::GlyphRun(run) = item {
                            out.extend(run.glyphs().map(|g| (g.id, g.x.to_bits(), g.advance.to_bits())));
                        }
                    }
                }
            }
        }
        out
    };
    let (native, web) = (shaped(&[NATIVE[0], NATIVE[1], NATIVE[2], PLEX]), shaped(&[WEB[0], WEB[1], WEB[2], PLEX]));
    assert!(native.len() > 300);
    assert_eq!(native, web);
}
