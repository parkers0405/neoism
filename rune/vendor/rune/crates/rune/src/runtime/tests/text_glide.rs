use super::*;
use crate::render::{KIND_GLYPH, KIND_RECT};
use crate::{Color, TextStyle};
use rune_motion::Spring;

/// Where the app puts the card, and the spring (seconds) it glides there on.
#[derive(Clone, Copy)]
struct Card {
    at: [f32; 2],
    spring: f32,
}

/// A card with a fill, a border and a line of text, which the app moves by `offset` every frame
/// and which glides there on a spring.
fn card(s: &Card, _: &mut Cx<Card>) -> El {
    let label = crate::text("Quiet harbor glides").font(TextStyle::new(13.0, 500.0)).color(Color::WHITE);
    el().size(240.0, 120.0).child(
        col().absolute(20.0, 30.0).padding(12.0).radius(10.0).fill(Color::WHITE.mix(Color::BLACK, 0.8))
            .border(1.0, Color::WHITE.mix(Color::BLACK, 0.6))
            .offset(s.at[0], s.at[1]).offset_motion(Spring::new(s.spring, 0.0)).child(label),
    )
}

/// The card's drawn top-left (device px).
fn card_at(rt: &Runtime<Card>) -> [f32; 2] {
    let r = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).expect("the card's fill").rect;
    [r[0], r[1]]
}

/// The coverage-weighted centre of the text's ink (device px), read from the atlas masks its
/// glyph quads draw.
fn ink_at(rt: &Runtime<Card>) -> [f32; 2] {
    let atlas = &rt.text.atlas;
    let size = crate::text::ATLAS_SIZE as usize;
    let (mut sum, mut at) = (0.0f64, [0.0f64; 2]);
    for i in rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH) {
        let data = atlas.page_data(i.params[2] as usize).expect("an atlas page");
        let (u, v) = (i.uv[0] as usize, i.uv[1] as usize);
        for y in 0..i.rect[3] as usize {
            for x in 0..i.rect[2] as usize {
                let c = data[(v + y) * size + u + x] as f64;
                sum += c;
                at[0] += c * (i.rect[0] as f64 + x as f64 + 0.5);
                at[1] += c * (i.rect[1] as f64 + y as f64 + 0.5);
            }
        }
    }
    assert!(sum > 0.0, "draws glyphs");
    [(at[0] / sum) as f32, (at[1] / sum) as f32]
}

/// The repro: the app bobs the card by `-2.5 * (0.5 - 0.5 cos(2π t / 5))` on `axis` for five
/// seconds at `hz`, on a `spring`. Returns the spread (device px) of where the ink sits against
/// the card's drawn edge, how far the card moved, and how many alpha masks the atlas gained.
fn glide(axis: usize, spring: f32, hz: u32) -> (f32, f32, usize) {
    let mut rt = Runtime::new(Card { at: [0.0; 2], spring }, card, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let masks = rt.text.atlas.glyph_entries();
    let (mut lo, mut hi) = (f32::MAX, f32::MIN);
    let (mut top_lo, mut top_hi) = (f32::MAX, f32::MIN);
    for k in 1..=5 * hz {
        let t = k as f32 / hz as f32;
        let d = -2.5 * (0.5 - 0.5 * (std::f32::consts::TAU * t / 5.0).cos());
        rt.update(|s, _| s.at[axis] = d);
        rt.frame(1.0 / hz as f64);
        let off = ink_at(&rt)[axis] - card_at(&rt)[axis];
        (lo, hi) = (lo.min(off), hi.max(off));
        let top = card_at(&rt)[axis];
        (top_lo, top_hi) = (top_lo.min(top), top_hi.max(top));
    }
    (hi - lo, top_hi - top_lo, rt.text.atlas.glyph_entries() - masks)
}

/// Text glides with its box: the ink keeps its place against the card's drawn edge to within a
/// glide phase while pixel snapping glides the card by fractions of a pixel, on both axes.
#[test]
fn text_glides_with_its_box() {
    let mut ticks = Vec::new();
    for (spring, hz) in [(0.08, 120), (0.08, 60), (0.3, 60), (1.0, 120)] {
        for axis in [1, 0] {
            let (spread, travel, masks) = glide(axis, spring, hz);
            eprintln!("spring {spring} s at {hz} Hz, axis {axis}: the ink spreads {spread:.3} px against the card over {travel:.2} px of travel; {masks} new masks");
            assert!(travel > 4.5, "the card moves five device px: {travel}");
            if spread > 0.3 {
                ticks.push((spring, hz, axis, spread));
            }
        }
    }
    assert!(ticks.is_empty(), "the ink ticks against its card (spring, Hz, axis, px): {ticks:?}");
}

/// The glide phases are few: moving text adds at most a few masks per glyph to the atlas.
#[test]
fn a_glide_keeps_the_atlas_bounded() {
    let mut rt = Runtime::new(Card { at: [0.0; 2], spring: 0.3 }, card, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH).count();
    let (_, _, masks) = glide(1, 0.3, 60);
    assert!(masks <= glyphs * crate::text::GLIDE_PHASES as usize, "{masks} new masks for {glyphs} glyphs");
}

/// Once the card comes to rest, the frame is byte for byte what a card that never moved draws
/// there.
#[test]
fn a_glide_lands_on_the_resting_frame() {
    let mut rt = Runtime::new(Card { at: [0.0; 2], spring: 0.3 }, card, (240.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for k in 1..=40 {
        rt.update(|s, _| s.at = [0.7 * k as f32 / 40.0, -1.3 * k as f32 / 40.0]);
        rt.frame(1.0 / 60.0);
    }
    for _ in 0..120 {
        rt.frame(1.0 / 60.0);
    }
    let mut still = Runtime::new(Card { at: [0.7, -1.3], spring: 0.3 }, card, (240.0, 120.0), Env::new(true, 2.0));
    still.frame(0.0);
    let bytes = |rt: &Runtime<Card>| bytemuck::cast_slice::<_, u8>(&rt.instances).to_vec();
    assert!(bytes(&rt) == bytes(&still), "the landed card draws as a still one");
}

