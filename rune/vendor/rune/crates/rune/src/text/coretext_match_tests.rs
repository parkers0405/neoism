use super::*;
use crate::text::{GlyphQuad, TextSystem};
use crate::tok::TextStyle;

#[test]
fn the_default_is_on_on_macos_and_windfoil_elsewhere() {
    let want = if cfg!(target_os = "macos") { FontSmoothing::On } else { FontSmoothing::Windfoil };
    assert_eq!(FontSmoothing::default(), want);
    assert_eq!(TextSystem::default().smoothing, want);
    assert_eq!(TextSystem::default().apple_pipeline(), want.pipeline());
}

#[test]
fn a_browser_on_a_mac_smooths_and_an_ipad_or_anything_else_does_not() {
    assert_eq!(browser_default("macOS", 0), FontSmoothing::On, "userAgentData");
    assert_eq!(browser_default("MacIntel", 0), FontSmoothing::On, "navigator.platform");
    assert_eq!(browser_default("MacIntel", 5), FontSmoothing::Off, "an iPad says it's a Mac");
    assert_eq!(browser_default("iPhone", 5), FontSmoothing::Off);
    assert_eq!(browser_default("Windows", 0), FontSmoothing::Off);
    assert_eq!(browser_default("Linux x86_64", 0), FontSmoothing::Off);
    assert_eq!(browser_default("", 0), FontSmoothing::Off);
}

/// A pen that summed advances in a different order lands within f32 noise of the same 1/256 px
/// and so on the same step.
#[test]
fn summation_noise_does_not_move_a_pen_across_a_step() {
    assert_eq!(glyph_x(935.99945, 26.0), glyph_x(936.0, 26.0));
    assert_eq!(glyph_x(1013.9993, 26.0), glyph_x(1014.0, 26.0));
    assert_eq!(glyph_x(353.0 - 1.0 / 256.0, 40.0), (352.0, 0));
}

#[test]
fn pens_floor_to_coregraphics_steps() {
    // 13 pt at 2× (26 ppem): halves.
    assert_eq!(glyph_x(10.49, 26.0), (10.0, 0));
    assert_eq!(glyph_x(10.5, 26.0), (10.0, 30));
    assert_eq!(glyph_x(10.99, 26.0), (10.0, 30));
    // 13 pt at 1×: thirds.
    assert_eq!(glyph_x(10.74, 13.0), (10.0, 40));
    assert_eq!(glyph_x(-0.1, 13.0), (-1.0, 40));
    assert_eq!(glyph_x(10.74, 9.0), (10.0, 30), "quarters at 9 ppem");
    assert_eq!(glyph_x(10.5, 6.0), (10.0, 24), "fifths at 6 ppem");
    assert_eq!(glyph_x(10.99, 40.0), (10.0, 0), "whole pixels at 40 ppem");
}

#[test]
fn quantisation_steps_follow_coregraphics_boundaries() {
    for (ppem, steps) in [(6.0, 5.0), (8.25, 5.0), (8.5, 4.0), (11.0, 4.0), (11.25, 3.0), (16.5, 3.0), (16.75, 2.0), (33.25, 2.0), (33.5, 1.0), (80.0, 1.0)] {
        assert_eq!(quantisation_steps(ppem), steps, "{ppem} ppem");
    }
}

fn quads(text: &mut TextSystem, x: f32) -> Vec<GlyphQuad> {
    let mut out = Vec::new();
    text.glyphs("Hamburgefonstiv", TextStyle::new(13.0, 400.0), 2.0, [x, 0.0], &mut out);
    out
}

/// Each mode rasterises its own atlas entries: switching draws new coverage rather than
/// reusing another mode's glyphs, and switching back reuses the first mode's.
#[test]
fn each_mode_keys_its_own_glyphs_and_changes_the_pixels() {
    let mut text = TextSystem::default();
    let coverage = |text: &mut TextSystem, mode| {
        text.set_font_smoothing(mode);
        let before = text.atlas.rasterized();
        let q = quads(text, 10.3);
        let sum: u64 = q.iter().map(|q| {
            let (uv, page) = (q.atlas_uv(), text.atlas.page_data(q.atlas_page()).unwrap());
            let mut s = 0u64;
            for y in 0..q.rect[3] as u32 {
                for x in 0..q.rect[2] as u32 {
                    s += page[((uv[1] as u32 + y) * crate::text::ATLAS_SIZE + uv[0] as u32 + x) as usize] as u64;
                }
            }
            s
        }).sum();
        (text.atlas.rasterized() - before, sum, q.iter().map(|q| q.rect[0]).collect::<Vec<_>>())
    };
    let (new_on, ink_on, x_on) = coverage(&mut text, FontSmoothing::On);
    let (new_off, ink_off, x_off) = coverage(&mut text, FontSmoothing::Off);
    let (again, _, _) = coverage(&mut text, FontSmoothing::On);
    assert!(new_on > 0 && new_off > 0, "every mode rasterises its own glyphs: {new_on} {new_off}");
    assert_eq!(again, 0, "switching back reuses the mode's glyphs");
    assert!(ink_off < ink_on, "Off has no dilation: {ink_off} < {ink_on}");
    // Same CoreGraphics pen steps; On's dilation can widen a glyph's box by a pixel to the left.
    assert!(x_on.iter().zip(&x_off).all(|(a, b)| (a - b).abs() <= 1.0), "both modes floor pens to the same CoreGraphics steps: {x_on:?} {x_off:?}");
}

/// On macOS and iOS every mode draws coverage as is, as CoreText with smoothing off and
/// browsers with `-webkit-font-smoothing: antialiased` do; elsewhere `Off` takes a browser's
/// text gamma, heavier for light ink and a little lighter for dark.
#[test]
fn off_draws_coverage_as_is_on_apple_platforms() {
    let inks = [[0.0, 0.0, 0.0, 1.0], [0.11, 0.12, 0.13, 1.0], [0.9, 0.9, 0.9, 1.0], [1.0, 1.0, 1.0, 1.0], [0.4, 0.4, 0.4, 0.5]];
    for mode in [FontSmoothing::On, FontSmoothing::Off, FontSmoothing::DirectWrite, FontSmoothing::Windfoil] {
        for fill in inks {
            assert_eq!(coverage_exponent(mode, fill, true, false), 1.0, "{mode:?} {fill:?} on macOS");
        }
    }
    let off = |fill| coverage_exponent(FontSmoothing::Off, fill, false, false);
    assert!(off([0.0, 0.0, 0.0, 1.0]) > 1.0 && off([1.0, 1.0, 1.0, 1.0]) < 1.0, "Off elsewhere takes the gamma");
    assert_eq!(coverage_exponent(FontSmoothing::On, [1.0; 4], false, false), 1.0);
    assert_eq!(coverage_exponent(FontSmoothing::DirectWrite, [1.0; 4], false, true), 1.0, "DirectWrite masks carry their own gamma");
    if cfg!(any(target_os = "macos", target_os = "ios")) {
        let mut text = TextSystem::default();
        text.set_font_smoothing(FontSmoothing::Off);
        assert_eq!(text.coverage_exponent([0.9, 0.9, 0.9, 1.0]), 1.0, "a window on this Mac draws Off as is");
    }
}

/// `Tx::set_font_smoothing` switches a running window: the next frame rasterises the new
/// mode's glyphs and draws them; asking for the current mode asks for no frame.
#[test]
fn a_handler_switches_font_smoothing_at_run_time() {
    use crate::{Cx, El, Env, Runtime};
    fn view(_: &(), _: &mut Cx<()>) -> El { crate::el::text("Hamburgefonstiv").size(13.0, 400.0) }
    let mut rt = Runtime::new((), view, (300.0, 100.0), Env::new(false, 2.0));
    rt.set_font_smoothing(FontSmoothing::On);
    rt.frame(0.0);
    let glyphs = |rt: &Runtime<()>| rt.instances.iter().filter(|i| i.params[1] == crate::render::KIND_GLYPH).map(|i| (i.rect, i.uv)).collect::<Vec<_>>();
    let before = (glyphs(&rt), rt.text.atlas.rasterized());
    assert!(!rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::On); false }), "the current mode asks for no frame");
    assert!(rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::Off); false }), "a new mode asks for a frame");
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.font_smoothing(), FontSmoothing::Off);
    assert_eq!(rt.text.apple_pipeline(), crate::text::ApplePipeline::UNSMOOTHED);
    assert!(rt.text.atlas.rasterized() > before.1, "Off rasterises its own glyphs");
    assert_ne!(glyphs(&rt), before.0, "and draws them");
    let srgb = rt.srgb_blend();
    assert!(rt.update_if(|_, tx| { tx.set_font_smoothing(FontSmoothing::On); false }));
    assert_eq!(rt.srgb_blend(), srgb, "blending is separate from the mode");
}
