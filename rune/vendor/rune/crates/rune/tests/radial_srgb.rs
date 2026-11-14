use std::f32::consts::{FRAC_PI_4, PI};
use std::sync::Arc;

use rune::color::{hex, srgb_encode};
use rune::compose::{Compositor, Frame};
use rune::native_surface::{ByteFrame, ColorInfo, Format};
use rune::render::{Gpu, KIND_RADIAL, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::{
    App, AppSpec, Color, Cx, El, Env, Image, ImageCache,
    NativeSurface, Pixels, Runtime, SurfaceFit, el,
};

fn render_rt<S: 'static>(gpu: &Gpu, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let format = if rt.srgb_blend() {
        OFFSCREEN_FORMAT.remove_srgb_suffix()
    } else {
        OFFSCREEN_FORMAT
    };
    let mut renderer = Renderer::new(&gpu.device, format);
    renderer.set_srgb_blend(rt.srgb_blend());
    let target = Offscreen::with_format(&gpu.device, size.0, size.1, format);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, size);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, rt.clear_color());
    target.copy(&mut encoder);
    renderer.submit(&gpu.queue, encoder);
    target.read(&gpu.device)
}

fn snap_app<S: 'static>(app: App<S>, name: &str, extra_args: &[&str]) -> (u32, u32, Vec<u8>) {
    let dir = std::env::temp_dir().join(format!("rune-radial-srgb-{name}-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let png_path = dir.join("snap.png");
    let mut args = vec![
        "--snap".to_string(),
        png_path.to_str().unwrap().to_string(),
        "--scale".to_string(),
        "1".to_string(),
    ];
    for a in extra_args {
        args.push((*a).to_string());
    }
    app.args(args).run();
    let file = std::fs::File::open(&png_path).unwrap();
    let decoder = png::Decoder::new(std::io::BufReader::new(file));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    std::fs::remove_dir_all(&dir).unwrap();
    (info.width, info.height, buf)
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * width + x) * 4) as usize;
    rgba[at..at + 4].try_into().unwrap()
}

#[test]
fn radial_gradient_centre_edge_and_monotonic_falloff() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(120.0, 120.0).radial_gradient(
            [0.5, 0.5],
            50.0,
            [(0.0, Color::WHITE), (1.0, Color::BLACK)],
        )
    }

    let mut rt = Runtime::new((), view, (120.0, 120.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1);
    let inst = rt.instances[0];
    assert_eq!(inst.params[1], KIND_RADIAL);
    assert_eq!(inst.uv, [60.0, 60.0, 50.0, 50.0]);

    let gpu = Gpu::headless();
    let out = render_rt(&gpu, &mut rt, (120, 120));

    let center = pixel(&out, 120, 60, 60);
    let edge = pixel(&out, 120, 115, 60);
    assert!(center[0] >= 253 && center[1] >= 253 && center[2] >= 253 && center[3] == 255, "centre: {center:?}");
    assert_eq!(edge, [0, 0, 0, 255], "outside radius clamps to outer stop");

    let mut prev = center[0];
    for x in 60..115 {
        let p = pixel(&out, 120, x, 60)[0];
        assert!(p <= prev, "non-monotonic at x={x}: {p} > {prev}");
        prev = p;
    }
    assert!(pixel(&out, 120, 85, 60)[0] > 80 && pixel(&out, 120, 85, 60)[0] < 220);
}

#[test]
fn radial_gradient_multi_stop_elliptical_quad_shrinking_and_validation() {
    fn multi_view(swap: &bool, _: &mut Cx<bool>) -> El {
        let mid = if *swap {
            Color { r: 1.0, g: 1.0, b: 0.0, a: 1.0 }
        } else {
            Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }
        };
        el().size(140.0, 100.0).child(
            el()
                .absolute(10.0, 10.0)
                .size(120.0, 80.0)
                .radius(20.0)
                .fill(Color { r: 0.1, g: 0.1, b: 0.1, a: 1.0 })
                .radial_gradient(
                    [0.5, 0.5],
                    [50.0, 25.0],
                    [
                        (0.0, Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                        (0.5, mid),
                        (1.0, Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }),
                    ],
                )
                .border(2.0, Color::WHITE)
                .opacity(0.9),
        )
    }

    let mut rt = Runtime::new(false, multi_view, (140.0, 100.0), Env::new(true, 1.0));
    rt.transparent = true;
    rt.frame(0.0);

    // Verify inner radial band's quad is shrunk to the bbox of centre ± t1*r (52×27 vs full 120×80).
    let radial_insts: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_RADIAL).collect();
    assert_eq!(radial_insts.len(), 2);
    assert!(
        radial_insts[0].rect[2] <= 54.0 && radial_insts[0].rect[3] <= 28.0,
        "inner radial band quad should shrink to its stop bbox: {:?}",
        radial_insts[0].rect
    );
    assert_eq!(radial_insts[1].rect, [10.0, 10.0, 120.0, 80.0]);

    let gpu = Gpu::headless();
    let out = render_rt(&gpu, &mut rt, (140, 100));

    // Rounded corner outside the 20pt radius at (11, 11) is clipped transparent.
    assert_eq!(pixel(&out, 140, 11, 11)[3], 0);
    // Centre (70, 50) is dominated by the inner red stop.
    let c_center = pixel(&out, 140, 70, 50);
    assert!(c_center[0] > 200 && c_center[1] < 70 && c_center[2] < 40, "centre: {c_center:?}");
    // Halfway horizontally (70 + 25 = 95, 50) and vertically (70, 50 + 12 = 62) hit the mid green stop (t ≈ 0.5).
    let c_mid_x = pixel(&out, 140, 95, 50);
    let c_mid_y = pixel(&out, 140, 70, 62);
    assert!(c_mid_x[1] > 200 && c_mid_x[0] < 90 && c_mid_x[2] < 90, "mid x: {c_mid_x:?}");
    assert!(c_mid_y[1] > 200 && c_mid_y[0] < 90 && c_mid_y[2] < 90, "mid y: {c_mid_y:?}");
    // At dx = 50 (120, 50) we are at t = 1.0 (blue stop).
    let c_outer = pixel(&out, 140, 120, 50);
    assert!(c_outer[2] > 200 && c_outer[0] < 40 && c_outer[1] < 40, "outer: {c_outer:?}");
    // Border at (70, 11) is white.
    let c_border = pixel(&out, 140, 70, 11);
    assert!(c_border[0] > 210 && c_border[1] > 210 && c_border[2] > 210, "border: {c_border:?}");

    // Stop colours animate via Oklab springs when state updates.
    rt.update(|s, _| *s = true);
    rt.frame(0.04);
    let mid_anim = render_rt(&gpu, &mut rt, (140, 100));
    let p_anim = pixel(&mid_anim, 140, 95, 50);
    assert!(p_anim[0] > c_mid_x[0] + 20 && p_anim[0] < 245, "mid-spring stop colour: {p_anim:?}");

    // Half-valid radii (e.g. [10.0, 0.0]) are dropped and never fall through to linear_gradient;
    // stops are capped at 8 (7 segments).
    fn edge_cases(_: &(), _: &mut Cx<()>) -> El {
        let ten_stops: Vec<(f32, Color)> = (0..10)
            .map(|i| (i as f32 / 9.0, Color::WHITE))
            .collect();
        el().size(100.0, 100.0)
            .child(el().size(50.0, 50.0).radial_gradient([0.5, 0.5], [10.0, 0.0], [(0.0, Color::WHITE), (1.0, Color::BLACK)]))
            .child(el().size(50.0, 50.0).radial_gradient([0.5, 0.5], 25.0, ten_stops))
    }
    let mut rt_edge = Runtime::new((), edge_cases, (100.0, 100.0), Env::new(true, 1.0));
    rt_edge.frame(0.0);
    let edge_radials: Vec<_> = rt_edge.instances.iter().filter(|i| i.params[1] == KIND_RADIAL).collect();
    assert_eq!(edge_radials.len(), 7, "10 stops capped at 8 stops (7 segments), invalid [10, 0] dropped");
    assert!(edge_radials.iter().all(|i| i.uv[2] > 0.0 && i.uv[3] > 0.0));

    // Angled linear_gradient(PI, ..) matches vertical El::gradient(..), and 45° runs corner-to-corner.
    fn linear_pair(angled: &bool, _: &mut Cx<bool>) -> El {
        let a = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
        let b = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };
        let card = el().size(80.0, 60.0).radius(8.0);
        if *angled {
            card.linear_gradient(PI, [(0.0, a), (1.0, b)])
        } else {
            card.gradient(a, b)
        }
    }
    let mut rt_v = Runtime::new(false, linear_pair, (80.0, 60.0), Env::new(true, 1.0));
    let mut rt_l = Runtime::new(true, linear_pair, (80.0, 60.0), Env::new(true, 1.0));
    rt_v.frame(0.0);
    rt_l.frame(0.0);
    let px_v = render_rt(&gpu, &mut rt_v, (80, 60));
    let px_l = render_rt(&gpu, &mut rt_l, (80, 60));
    assert!(
        px_v.iter().zip(&px_l).all(|(a, b)| a.abs_diff(*b) <= 1),
        "linear_gradient(PI) matches gradient()"
    );

    fn diag_blurred(_: &(), _: &mut Cx<()>) -> El {
        el().size(80.0, 80.0).child(
            el()
                .absolute(10.0, 10.0)
                .size(60.0, 60.0)
                .linear_gradient(FRAC_PI_4, [(0.0, Color::WHITE), (1.0, Color::BLACK)])
                .blur(2.0),
        )
    }
    let mut rt_d = Runtime::new((), diag_blurred, (80.0, 80.0), Env::new(true, 1.0));
    rt_d.frame(0.0);
    let px_d = render_rt(&gpu, &mut rt_d, (80, 80));
    assert!(pixel(&px_d, 80, 20, 60)[0] > pixel(&px_d, 80, 60, 20)[0] + 100);
}

#[test]
fn srgb_blend_matches_css_and_core_animation_glow_reference() {
    let bg = hex("#100e11").unwrap();
    let glow = hex("#40bf80").unwrap().alpha(0.16);

    // Analytical CSS / Core Animation sRGB reference for #40bf80 @ 0.16 over #100e11 with blur 30 (sigma = 15).
    let bg_srgb = [bg.r, bg.g, bg.b].map(srgb_encode);
    let glow_srgb = [glow.r, glow.g, glow.b].map(srgb_encode);

    fn shadow_glow_view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#100e11").unwrap();
        let glow = hex("#40bf80").unwrap().alpha(0.16);
        el().size(240.0, 240.0).fill(bg).child(
            el()
                .absolute(60.0, 60.0)
                .size(120.0, 120.0)
                .radius(24.0)
                .shadow(0.0, 30.0, glow),
        )
    }

    let (_, _, px_linear) = snap_app(
        App::new(AppSpec::new("glow-lin", (240.0, 240.0), || (), shadow_glow_view)).srgb_blend(false),
        "glow-lin",
        &[],
    );
    let (_, _, px_srgb) = snap_app(
        App::new(AppSpec::new("glow-srgb", (240.0, 240.0), || (), shadow_glow_view)).srgb_blend(true),
        "glow-srgb",
        &[],
    );

    // At the centre (120, 120), the shadow weight is ~1.0 so effective alpha is 0.16.
    let expected_center: [u8; 3] = std::array::from_fn(|c| {
        ((glow_srgb[c] * 0.16 + bg_srgb[c] * (1.0 - 0.16)) * 255.0).round() as u8
    });
    assert_eq!(expected_center, [24, 42, 35]);

    let linear_center = pixel(&px_linear, 240, 120, 120);
    let srgb_center = pixel(&px_srgb, 240, 120, 120);

    // Default linear mode stays unchanged (green channel is 83 vs 42 in sRGB reference).
    assert_eq!(linear_center, [29, 83, 56, 255]);
    assert_eq!(&srgb_center[..3], &expected_center, "0 LSB centre error in srgb_blend");

    // Check the Gaussian falloff across the left edge (x = 15..120 at y = 120, where sigma = 15 px).
    let erf = |x: f32| -> f32 {
        let s = x.signum();
        let a = x.abs();
        let r = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
        let r2 = r * r;
        s - s / (r2 * r2)
    };
    let sigma = 15.0f32;
    let mut max_err_linear = 0u8;
    let mut max_err_srgb = 0u8;
    for x in 15..=120 {
        let px = x as f32 + 0.5 - 120.0;
        let scale = 0.5f32.sqrt() / sigma;
        let w = 0.5 * (erf((px + 60.0) * scale) - erf((px - 60.0) * scale));
        let a = 0.16 * w;
        let ref_rgb: [u8; 3] = std::array::from_fn(|c| {
            ((glow_srgb[c] * a + bg_srgb[c] * (1.0 - a)) * 255.0).round() as u8
        });
        let p_lin = pixel(&px_linear, 240, x, 120);
        let p_srgb = pixel(&px_srgb, 240, x, 120);
        for c in 0..3 {
            max_err_linear = max_err_linear.max(p_lin[c].abs_diff(ref_rgb[c]));
            max_err_srgb = max_err_srgb.max(p_srgb[c].abs_diff(ref_rgb[c]));
        }
    }
    assert!(max_err_linear >= 40, "linear error should be large (~41 LSB): {max_err_linear}");
    assert!(max_err_srgb <= 1, "srgb_blend falloff error within 1 LSB: {max_err_srgb}");

    // Also verify layer blur (`.fill(glow).blur(15.0)`) in srgb_blend mode matches the centre and falloff.
    fn blur_glow_view(_: &(), _: &mut Cx<()>) -> El {
        let bg = hex("#100e11").unwrap();
        let glow = hex("#40bf80").unwrap().alpha(0.16);
        el().size(240.0, 240.0).fill(bg).child(
            el()
                .absolute(60.0, 60.0)
                .size(120.0, 120.0)
                .radius(24.0)
                .fill(glow)
                .blur(15.0),
        )
    }
    let (_, _, px_blur_srgb) = snap_app(
        App::new(AppSpec::new("blur-srgb", (240.0, 240.0), || (), blur_glow_view)).srgb_blend(true),
        "blur-srgb",
        &[],
    );
    let blur_center = pixel(&px_blur_srgb, 240, 120, 120);
    for c in 0..3 {
        assert!(
            blur_center[c].abs_diff(expected_center[c]) <= 1,
            "layer blur srgb centre channel {c}: got {blur_center:?}, expected {expected_center:?}"
        );
    }
}

#[test]
fn srgb_blend_images_surfaces_keycaps_and_compositor() {
    #[derive(Clone)]
    struct MediaState {
        premul_img: Image,
        straight_px: Pixels,
        bgra_surface: NativeSurface,
        nv12_surface: NativeSurface,
    }

    fn init_media() -> MediaState {
        let cache = ImageCache::new(Arc::new(|| {}));
        // White at alpha 128/255 (~0.502): ImageCache stores sRGB-encoded premultiplied (188, 188, 188, 128).
        let premul_img = cache.rgba(16, 16, [255u8, 255, 255, 128].repeat(16 * 16));
        premul_img.wait().unwrap();

        let straight_px = Pixels::new();
        straight_px.set(16, 16, [255u8, 255, 255, 128].repeat(16 * 16));

        let bgra_surface = NativeSurface::new();
        bgra_surface
            .push_bytes(
                ByteFrame {
                    width: 16,
                    height: 16,
                    format: Format::Bgra,
                    color: ColorInfo::default(),
                    planes: [[128u8, 128, 128, 255].repeat(16 * 16), Vec::new()],
                },
                1,
            )
            .unwrap();

        let nv12_surface = NativeSurface::new();
        nv12_surface
            .push_bytes(
                ByteFrame {
                    width: 16,
                    height: 16,
                    format: Format::Nv12Full,
                    color: ColorInfo::default(),
                    planes: [vec![128u8; 16 * 16], vec![128u8; 8 * 8 * 2]],
                },
                1,
            )
            .unwrap();

        MediaState {
            premul_img,
            straight_px,
            bgra_surface,
            nv12_surface,
        }
    }

    fn media_view(s: &MediaState, _: &mut Cx<MediaState>) -> El {
        el().size(120.0, 40.0)
            .fill(hex("#808080").unwrap())
            .child(el().absolute(4.0, 4.0).size(16.0, 16.0).pixels(s.premul_img.pixels()))
            .child(el().absolute(24.0, 4.0).size(16.0, 16.0).pixels(&s.straight_px))
            .child(el().absolute(44.0, 4.0).size(16.0, 16.0).native_surface(&s.bgra_surface, SurfaceFit::Fill))
            .child(el().absolute(64.0, 4.0).size(16.0, 16.0).native_surface(&s.nv12_surface, SurfaceFit::Fill))
            .child(
                el().absolute(84.0, 4.0)
                    .size(16.0, 16.0)
                    .native_surface(&s.bgra_surface, SurfaceFit::Fill)
                    .blur(0.5),
            )
    }

    let (_, _, px_srgb) = snap_app(
        App::new(AppSpec::new("media-srgb", (120.0, 40.0), init_media, media_view)).srgb_blend(true),
        "media-srgb",
        &[],
    );

    // 1. Premultiplied image: white @ alpha 128/255 over mid-grey 128/255 => 128 + 127 * (128/255) ≈ 191.
    let p_premul = pixel(&px_srgb, 120, 12, 12);
    assert!(
        p_premul[0].abs_diff(191) <= 1 && p_premul[1].abs_diff(191) <= 1 && p_premul[2].abs_diff(191) <= 1,
        "premultiplied image edge texel in srgb_blend should be 191 ± 1, got {p_premul:?}"
    );

    // 2. Straight-alpha Pixels: white @ alpha 128/255 over mid-grey 128/255 => 191 ± 1.
    let p_straight = pixel(&px_srgb, 120, 32, 12);
    assert!(
        p_straight[0].abs_diff(191) <= 1 && p_straight[1].abs_diff(191) <= 1 && p_straight[2].abs_diff(191) <= 1,
        "straight-alpha pixels in srgb_blend should be 191 ± 1, got {p_straight:?}"
    );

    // 3. BGRA and NV12 video/native surfaces at 50% grey (128/255) in srgb_blend => 128 ± 1 (not ~55).
    let p_bgra = pixel(&px_srgb, 120, 52, 12);
    let p_nv12 = pixel(&px_srgb, 120, 72, 12);
    let p_layer_surface = pixel(&px_srgb, 120, 92, 12);
    assert!(
        p_bgra[0].abs_diff(128) <= 1 && p_bgra[1].abs_diff(128) <= 1 && p_bgra[2].abs_diff(128) <= 1,
        "BGRA surface 50% grey in srgb_blend should be 128 ± 1, got {p_bgra:?}"
    );
    assert!(
        p_nv12[0].abs_diff(128) <= 1 && p_nv12[1].abs_diff(128) <= 1 && p_nv12[2].abs_diff(128) <= 1,
        "NV12 surface 50% grey in srgb_blend should be 128 ± 1, got {p_nv12:?}"
    );
    assert!(
        p_layer_surface[0].abs_diff(128) <= 1 && p_layer_surface[1].abs_diff(128) <= 1 && p_layer_surface[2].abs_diff(128) <= 1,
        "Blurred Rgba16Float layer surface 50% grey in srgb_blend should be 128 ± 1, got {p_layer_surface:?}"
    );

    // 4. Secondary runtime (`--keys` and `--pointer` overlay) propagates srgb_blend.
    let dir = std::env::temp_dir().join(format!("rune-keys-srgb-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let script_path = dir.join("script.json");
    std::fs::write(
        &script_path,
        r#"[{"t": 0.0, "type": "key", "name": "a"}, {"t": 0.0, "type": "click", "x": 90, "y": 20}]"#,
    )
    .unwrap();
    let (_, _, px_keys_srgb) = snap_app(
        App::new(AppSpec::new("keys-srgb", (160.0, 80.0), || (), |_, _| el().size(160.0, 80.0).fill(hex("#808080").unwrap())))
            .srgb_blend(true),
        "keys-srgb",
        &[
            "--script",
            script_path.to_str().unwrap(),
            "--keys",
            "bottom-left",
            "--keys-theme",
            "dark",
            "--pointer",
            "--at",
            "0.25",
        ],
    );
    std::fs::remove_dir_all(&dir).unwrap();
    // Dark keycap at bottom-left (x=30, y=45) has fill oklcha(0.22, 0, 0, 0.96):
    // sRGB-encoded fill is ~24/255 (blended ~26), whereas unpropagated linear fill into non-sRGB was ~4/255.
    let p_cap = pixel(&px_keys_srgb, 160, 30, 45);
    assert!(
        p_cap[0] >= 20 && p_cap[0] <= 35,
        "keycaps overlay runtime should encode cap fill in sRGB space (~26, not ~4), got {p_cap:?}"
    );

    // 5. A Compositor sees the blend mode and the matching non-sRGB target format.
    static COMP_STATE: std::sync::OnceLock<(bool, wgpu::TextureFormat)> = std::sync::OnceLock::new();
    struct CheckComp;
    impl Compositor<()> for CheckComp {
        fn render(&mut self, f: &mut Frame<'_, ()>) {
            let _ = COMP_STATE.set((f.srgb_blend(), f.format));
            f.draw_ui();
        }
    }
    let _ = snap_app(
        App::new(AppSpec::new("comp-srgb", (64.0, 64.0), || (), |_, _| el().size(64.0, 64.0).fill(hex("#808080").unwrap())))
            .compositor(CheckComp)
            .srgb_blend(true),
        "comp-srgb",
        &[],
    );
    assert_eq!(
        COMP_STATE.get().copied(),
        Some((true, OFFSCREEN_FORMAT.remove_srgb_suffix())),
        "a compositor must see srgb_blend and the non-sRGB target format"
    );
}
