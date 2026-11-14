//! What CSS gives these boxes, in sRGB bytes: `El::dim` fades a solid box as `opacity` would,
//! with its outer shadows outside it and its fill and gradient fading as one, and an outer
//! shadow paints at its authored alpha over a transparent window or an opaque page.
use rune::color::{srgb_decode, srgb_encode};
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{Color, Cx, El, Env, Runtime, Shadow, el};

const W: u32 = 120;
const H: u32 = 140;

fn render(gpu: &Gpu, view: fn(&(), &mut Cx<()>) -> El, srgb: bool, transparent: bool) -> Vec<u8> {
    let mut rt = Runtime::new((), view, (W as f32, H as f32), Env::new(false, 1.0));
    rt.set_srgb_blend(srgb);
    rt.transparent = transparent;
    rt.frame(0.0);
    let format = if srgb { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(srgb);
    let target = Offscreen::with_format(&gpu.device, W, H, format);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (W, H));
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, rt.clear_color());
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    target.read(&gpu.device)
}

fn pixel(rgba: &[u8], x: u32, y: u32) -> [u8; 4] {
    let at = ((y * W + x) * 4) as usize;
    rgba[at..at + 4].try_into().unwrap()
}

fn byte(v: u8) -> Color {
    let c = srgb_decode(v as f32 / 255.0);
    Color { r: c, g: c, b: c, a: 1.0 }
}

/// CSS source-over in sRGB bytes.
fn mix(fg: f32, bg: f32, a: f32) -> f32 {
    a * fg + (1.0 - a) * bg
}

/// A contrast button's skin, 100 points tall: #242424 under white fading from 20% to 0, over a
/// 20%-black drop shadow and a crisp 30%-white edge half a point up, at `dim` on `page`.
fn button(page: u8, dim: f32, sheen: bool) -> El {
    let mut b = el().size(100.0, 100.0).radius(8.0).fill(byte(0x24))
        .box_shadow(Shadow { y: 1.0, blur: 1.5, color: Color::BLACK.alpha(0.2), ..Shadow::default() })
        .box_shadow(Shadow { y: -0.5, color: Color::WHITE.alpha(0.3), ..Shadow::default() });
    if sheen {
        b = b.gradient(Color::WHITE.alpha(0.2), Color::WHITE.alpha(0.0));
    }
    el().size(W as f32, H as f32).fill(byte(page)).padding(10.0).child(b.dim(dim))
}

#[test]
fn a_dimmed_button_fades_as_css_opacity_does() {
    let gpu = Gpu::headless();
    fn plain_light(_: &(), _: &mut Cx<()>) -> El { button(0xff, 0.4, false) }
    fn plain_dark(_: &(), _: &mut Cx<()>) -> El { button(0x16, 0.4, false) }
    fn sheened(_: &(), _: &mut Cx<()>) -> El { button(0xff, 0.4, true) }
    for (view, page) in [(plain_light as fn(&(), &mut Cx<()>) -> El, 255.0), (plain_dark, 22.0)] {
        for srgb in [true, false] {
            // No shadow shows through the faded fill, in either blend mode.
            let got = pixel(&render(&gpu, view, srgb, false), 60, 60)[0] as f32;
            let fill = if srgb { mix(36.0, page, 0.4) } else {
                srgb_encode(mix(srgb_decode(36.0 / 255.0), srgb_decode(page / 255.0), 0.4)) * 255.0
            };
            assert!((got - fill).abs() <= 1.0, "page {page}, sRGB blending {srgb}: {got} vs {fill}");
        }
    }
    // The fill and its sheen fade together: 40% of the sheened fill over white.
    let px = render(&gpu, sheened, true, false);
    for y in [30u32, 60, 90] {
        let t = (y as f32 + 0.5 - 10.0) / 100.0;
        let want = mix(mix(255.0, 36.0, 0.2 * (1.0 - t)), 255.0, 0.4);
        let got = pixel(&px, 60, y)[0] as f32;
        assert!((got - want).abs() <= 1.0, "row {y}: {got} vs {want}");
    }
}

#[test]
fn a_shadow_paints_its_authored_alpha() {
    // A card with a shadow 40 points down: well below the card the shadow is at full strength.
    fn card(_: &(), _: &mut Cx<()>) -> El {
        el().size(W as f32, H as f32).padding(10.0).child(el().size(100.0, 60.0).fill(Color::WHITE)
            .box_shadow(Shadow { y: 40.0, blur: 4.0, color: Color::BLACK.alpha(31.0 / 255.0), ..Shadow::default() }))
    }
    fn page(_: &(), cx: &mut Cx<()>) -> El { card(&(), cx).fill(Color::WHITE) }
    let gpu = Gpu::headless();
    let over_window = pixel(&render(&gpu, card, true, true), 60, 85);
    assert!(over_window[..3] == [0, 0, 0] && over_window[3].abs_diff(31) <= 1, "over a transparent window, the authored #0000001f: {over_window:?}");
    let over_page = pixel(&render(&gpu, page, true, false), 60, 85)[0] as f32;
    assert!((over_page - mix(0.0, 255.0, 31.0 / 255.0)).abs() <= 1.0, "over white: {over_page}");
}
