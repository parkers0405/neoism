//! Scroll translation must move the actual glyph ink with its box, not just element geometry.
use rune::{Color, Cx, El, Env, Runtime, ScrollSpec, TextStyle, col, el, text};
use rune::render::{KIND_GLYPH, KIND_RECT};

fn view(_: &(), _: &mut Cx<()>) -> El {
    col().key("scroll").w_full().h_full().scroll(ScrollSpec { stick_bottom: false }).child(
        col().w_full().children([
            el().h(200.0),
            col().key("card").padding(12.0).fill(Color::WHITE)
                .child(text("Quiet harbor glides").font(TextStyle::new(13.0, 500.0))),
            el().h(4000.0),
        ]),
    )
}

fn ink_y(rt: &Runtime<()>) -> f32 {
    let size = rune::text::ATLAS_SIZE as usize;
    let (mut sum, mut at) = (0.0f64, 0.0f64);
    for i in rt.instances.iter().filter(|i| i.params[1] == KIND_GLYPH) {
        let data = rt.text.atlas.page_data(i.params[2] as usize).unwrap();
        let (u, v) = (i.uv[0] as usize, i.uv[1] as usize);
        for y in 0..i.rect[3] as usize {
            for x in 0..i.rect[2] as usize {
                let coverage = data[(v + y) * size + u + x] as f64;
                sum += coverage;
                at += coverage * (i.rect[1] as f64 + y as f64 + 0.5);
            }
        }
    }
    assert!(sum > 0.0);
    (at / sum) as f32
}

#[test]
fn instant_scroll_keeps_glyph_ink_with_its_box() {
    for (distance, steps) in [(0.1, 120), (700.0 / 120.0, 24)] {
        let mut rt = Runtime::new((), view, (400.0, 600.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let (mut lo, mut hi) = (f32::MAX, f32::MIN);
        for step in (1..=steps).chain((0..steps).rev()) {
            rt.update_if(|_, tx| { tx.scroll_to_instant("scroll", 0.0, 10.0 + step as f32 * distance); false });
            rt.frame(1.0 / 120.0);
            let card = rt.instances.iter().find(|i| i.params[1] == KIND_RECT && i.fill[3] > 0.0).unwrap();
            let offset = ink_y(&rt) - card.rect[1];
            lo = lo.min(offset);
            hi = hi.max(offset);
        }
        let spread = hi - lo;
        eprintln!("{distance:.3} pt/step: glyph ink spread {spread:.3} device px against its box");
        assert!(spread <= 0.30, "scrolling glyph ink ticks {spread:.3} device px against its box");
    }
}