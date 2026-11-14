use super::*;

const PROSE: &str = "Amber lantern orchard quiet harbor, copper meadow signal drift. Pebble willow \
    ember tide under a juniper kestrel; nimbus opal prism. Ffi fjord Wave AVAT 0123456789.";

fn bits(quads: &[GlyphQuad]) -> Vec<[u32; 9]> {
    quads.iter().map(|q| {
        let [x, y, w, h] = q.rect.map(f32::to_bits);
        [x, y, w, h, q.uv[0].to_bits(), q.uv[1].to_bits(), q.ink as u32, q.byte, q.color]
    }).collect()
}

fn direct(ts: &mut TextSystem, l: &Layout<Ink>, origin: [f32; 2], k: f32, bytes: bool, band: std::ops::Range<f32>) -> Vec<[u32; 9]> {
    let mut out = Vec::new();
    ts.glyphs_in_band(l, origin, k, bytes, band, &mut out);
    bits(&out)
}

fn cached(ts: &mut TextSystem, l: &Rc<Layout<Ink>>, origin: [f32; 2], k: f32, bytes: bool, band: std::ops::Range<f32>) -> Vec<[u32; 9]> {
    let mut out = Vec::new();
    ts.glyphs_in_band_cached(l, origin, k, bytes, band, &mut out);
    bits(&out)
}

fn paragraph(ts: &mut TextSystem) -> Rc<Layout<Ink>> {
    let spans = [Span::new(6, 13).bold().color(Color { r: 0.9, g: 0.2, b: 0.2, a: 1.0 }), Span::new(30, 40).italic(), Span::new(60, 70).size(17.0)];
    ts.layout_rich(&PROSE.repeat(6), &spans, TextStyle::new(13.0, 400.0), 2.0, Some(260.0))
}

/// Replayed quads match a fresh walk bit for bit at whole, fractional and scaled origins, in
/// every band, with and without source bytes, and a replay walks no Parley lines.
#[test]
fn replayed_glyphs_match_a_fresh_walk() {
    let mut ts = TextSystem::default();
    let l = paragraph(&mut ts);
    assert!(l.len() > 8, "wraps onto many lines");
    ts.set_glyph_inks(Color::WHITE, &[]);
    let all = f32::NEG_INFINITY..f32::INFINITY;
    for bytes in [false, true] {
        for frame in 0..3 {
            ts.begin_frame();
            for origin in [[0.0, 0.0], [12.0, 30.0], [12.0, -41.0], [12.25, 30.5], [3.37, 7.81], [-5.6, 900.4]] {
                for band in [all.clone(), 40.0..120.0, 200.0..201.0, 1e6..2e6] {
                    let want = direct(&mut ts, &l, origin, 1.0, bytes, band.clone());
                    let flat = ts.glyph_cache.flattened;
                    assert_eq!(cached(&mut ts, &l, origin, 1.0, bytes, band.clone()), want, "{origin:?} {band:?} bytes {bytes} frame {frame}");
                    if frame > 1 {
                        assert_eq!(ts.glyph_cache.flattened, flat, "a replay walks no Parley lines");
                    }
                }
            }
        }
    }
    // A raster factor at rest caches too.
    for _ in 0..3 {
        ts.begin_frame();
        let want = direct(&mut ts, &l, [0.0, 0.0], 1.5, false, all.clone());
        assert_eq!(cached(&mut ts, &l, [0.0, 0.0], 1.5, false, all.clone()), want);
    }
}

/// After the atlas recycles a page, replay looks masks up again instead of drawing where
/// they used to be.
#[test]
fn replay_follows_atlas_eviction() {
    let mut ts = TextSystem::default();
    let l = paragraph(&mut ts);
    ts.set_glyph_inks(Color::WHITE, &[]);
    let all = f32::NEG_INFINITY..f32::INFINITY;
    let before = cached(&mut ts, &l, [0.0, 0.0], 1.0, false, all.clone());
    ts.begin_frame();
    ts.atlas.evict_page(0);
    // Other glyphs take the freed space first, so the paragraph's masks move.
    let other = ts.layout_rich("QXZJ qxzj 98765 @#%&", &[], TextStyle::new(23.0, 700.0), 2.0, None);
    let mut scratch = Vec::new();
    ts.glyphs_in_band(&other, [0.0, 0.0], 1.0, false, all.clone(), &mut scratch);
    let want = direct(&mut ts, &l, [0.0, 0.0], 1.0, false, all.clone());
    assert_ne!(want, before, "the masks moved");
    assert_eq!(cached(&mut ts, &l, [0.0, 0.0], 1.0, false, all), want);
}

/// One layout painted in two colours keeps an entry per ink, each matching a fresh walk.
#[test]
fn inks_key_their_own_entries() {
    let mut ts = TextSystem::default();
    let l = paragraph(&mut ts);
    let all = f32::NEG_INFINITY..f32::INFINITY;
    for _ in 0..2 {
        ts.begin_frame();
        for ink in [Color::WHITE, Color::BLACK] {
            ts.set_glyph_inks(ink, &[]);
            let want = direct(&mut ts, &l, [4.0, 4.0], 1.0, false, all.clone());
            assert_eq!(cached(&mut ts, &l, [4.0, 4.0], 1.0, false, all.clone()), want);
        }
    }
    assert!(ts.glyph_cache.len() <= 2);
}

/// Entries go with their layouts, and many paragraphs stay inside the byte budget.
#[test]
fn entries_are_bounded() {
    let mut ts = TextSystem::default();
    ts.set_glyph_inks(Color::WHITE, &[]);
    let all = f32::NEG_INFINITY..f32::INFINITY;
    // Held only here, not by the layout cache.
    let l = Rc::new((*paragraph(&mut ts)).clone());
    cached(&mut ts, &l, [0.0, 0.0], 1.0, false, all.clone());
    assert_eq!(ts.glyph_cache.len(), 1);
    drop(l);
    ts.begin_frame();
    assert_eq!(ts.glyph_cache.len(), 0, "a dropped layout's entry goes");
    assert_eq!(ts.glyph_cache.bytes(), 0);
    let mut keep = Vec::new();
    for i in 0..400 {
        let l = ts.layout_rich(&format!("{i} {}", PROSE.repeat(8)), &[], TextStyle::new(13.0, 400.0), 2.0, Some(300.0));
        cached(&mut ts, &l, [0.0, 0.0], 1.0, false, all.clone());
        keep.push(l);
        if i % 20 == 0 { ts.begin_frame(); }
    }
    ts.begin_frame();
    assert!(ts.glyph_cache.bytes() <= MAX_BYTES, "{} bytes", ts.glyph_cache.bytes());
    assert_eq!(ts.glyph_cache.bytes(), ts.glyph_cache.entries.values().map(Entry::size).sum::<usize>());
}

/// A layout that replaces one that went this frame, as a streamed text's next version does,
/// flattens into the room the old entry had, so its lines coming into view over the next
/// frames don't allocate. Room nobody takes for a frame goes.
#[test]
fn a_new_entry_grows_into_the_room_of_one_that_went() {
    let mut ts = TextSystem::default();
    ts.set_glyph_inks(Color::WHITE, &[]);
    let all = f32::NEG_INFINITY..f32::INFINITY;
    let old = Rc::new((*paragraph(&mut ts)).clone());
    cached(&mut ts, &old, [0.0, 0.0], 1.0, false, all.clone());
    let room = ts.glyph_cache.entries.values().map(|e| e.glyphs.capacity()).sum::<usize>();
    assert!(room > 0);
    drop(old);
    ts.begin_frame();
    let new = Rc::new((*paragraph(&mut ts)).clone());
    cached(&mut ts, &new, [0.0, 0.0], 1.0, false, 0.0..20.0);
    let e = ts.glyph_cache.entries.values().next().unwrap();
    assert!(e.glyphs.capacity() >= room && e.glyphs.len() < room, "{} of {room}", e.glyphs.capacity());
    assert_eq!(cached(&mut ts, &new, [0.0, 0.0], 1.0, false, all.clone()), direct(&mut ts, &new, [0.0, 0.0], 1.0, false, all.clone()));
    assert_eq!(ts.glyph_cache.bytes(), ts.glyph_cache.entries.values().map(Entry::size).sum::<usize>());
    drop(new);
    ts.begin_frame();
    assert!(ts.glyph_cache.spare.is_some());
    ts.begin_frame();
    assert!(ts.glyph_cache.spare.is_none(), "room nobody took goes");
}
