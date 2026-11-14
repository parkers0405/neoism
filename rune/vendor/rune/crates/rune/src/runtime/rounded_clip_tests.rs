use super::*;
use super::tests::pixels;
use crate::{Color, Cx, El, Env, Live, Runtime, el};

#[test]
fn packed_clip_radii_boundaries_and_fade_lanes_do_not_collide() {
    let rect = [0.0, 0.0, 2000.0, 2000.0];
    for (radii, want) in [
        ([0.0, 0.24, 12.26, 63.5], [0.0, 0.0, 12.5, 63.5]),
        ([0.0, 63.6, 127.0, 254.0], [0.0, 64.0, 128.0, 254.0]),
        ([0.0, 256.0, 500.0, 2000.0], [0.0, 254.0, 254.0, 254.0]),
        ([-1.0, -12.5, 0.0, -63.5], [-1.0, -12.5, 0.0, -63.5]),
    ] {
        assert_eq!(unpack_clip_radii(pack_clip_radii(rect, radii)), want);
    }
    for r in [0.0, 0.125, 63.75, 254.5, 999.0, -999.0] {
        assert_eq!(unpack_clip_radii(pack_clip_radii(rect, [r; 4])), [r; 4]);
    }
    for r in [1e30, -1e30, f32::INFINITY] {
        let word = pack_clip_radii([0.0, 0.0, 1e35, 1e35], [r; 4]);
        assert_ne!(word & 0x6000_0000, 0x6000_0000, "uniform radius cannot become a tag");
        assert!(unpack_clip_radii(word).iter().all(|v| v.is_finite() && v.abs() > 1e18));
    }
    let fade = pack_fade([65504.0; 4], rect, rect);
    assert_eq!(fade[3], 0, "even maximal fade widths never occupy the radius word");
}

#[test]
fn rounded_clip_follows_paint_radii_unless_explicitly_overridden() {
    fn view(mode: &u8, _: &mut Cx<u8>) -> El {
        let card = el().size(80.0, 40.0).corners([12.0; 4]).clip_rounded(4.0);
        let card = match mode {
            1 => card.clip_corners([4.0; 4]),
            2 => card.clip_corners([4.0; 4]).clip_rounded(8.0),
            _ => card,
        };
        card.child(el().size(80.0, 40.0).fill(Color::WHITE))
    }
    for (mode, want) in [(0, 12.0), (1, 4.0), (2, 12.0)] {
        let mut rt = Runtime::new(mode, view, (80.0, 40.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert!(rt.instances.iter().any(|i| unpack_clip_radii(i.fade[3]) == [want; 4]));
    }
}

#[test]
fn per_corner_video_clip_matches_the_primitive_mask() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
    type State = (NativeSurface, bool, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut card = el().size(48.0, 32.0).absolute(8.0, 8.0).corners([12.0, 4.0, 0.0, 8.0]);
        if s.2 { card = card.continuous_corners(); }
        if s.1 {
            card.clip_rounded(2.0).child(el().size(64.0, 48.0).absolute(-8.0, -8.0).native_surface(&s.0, SurfaceFit::Fill))
        } else {
            card.clip_soft(0.0).fill(Color::WHITE)
        }
    }
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 1, height: 1, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255; 4], vec![]] }, 1).unwrap();
    let gpu = Gpu::headless();
    // Compare mask coverage directly, without sRGB amplification of near-zero AA residuals.
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 64, 48, format);
    for continuous in [false, true] {
        let mut outs = Vec::new();
        for video in [false, true] {
            let mut rt = Runtime::new((surface.clone(), video, continuous), view, (64.0, 48.0), Env::new(true, 1.0));
            rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 48));
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (64, 48));
            outs.push(pixels(&gpu, &renderer, &target));
        }
        for (byte, (a, b)) in outs[0].iter().zip(&outs[1]).enumerate() {
            assert!(a.abs_diff(*b) <= 1, "continuous={continuous} byte={byte} primitive={a} video={b}");
        }
    }
}

#[test]
fn raised_children_keep_the_ancestors_per_corner_clip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().child(el().size(48.0, 32.0).absolute(8.0, 8.0)
            .corners([12.0, 4.0, 0.0, 8.0]).continuous_corners().clip_rounded(2.0).opacity(0.8)
            .child(el().size(64.0, 48.0).absolute(-8.0, -8.0).fill(Color::WHITE).raise()))
    }
    let mut rt = Runtime::new((), view, (64.0, 48.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let masks: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == KIND_LAYER && i.fade[3] != 0).collect();
    assert!(!masks.is_empty());
    for mask in masks {
        assert_eq!(mask.clip, [8.0, 8.0, 56.0, 40.0]);
        assert_eq!(unpack_clip_radii(mask.fade[3]), [-12.0, -4.0, 0.0, -8.0]);
    }
}

// Circle-corner coverage oracle, independent of the shader's rounded-box distance expression.
fn mask(x: f32, y: f32, rect: [f32; 4], radius: f32) -> f32 {
    let cx = x.clamp(rect[0] + radius, rect[2] - radius);
    let cy = y.clamp(rect[1] + radius, rect[3] - radius);
    let corner = (radius + 0.5 - (x - cx).hypot(y - cy)).clamp(0.0, 1.0);
    corner * (x - rect[0] + 0.5).clamp(0.0, 1.0) * (rect[2] - x + 0.5).clamp(0.0, 1.0)
        * (y - rect[1] + 0.5).clamp(0.0, 1.0) * (rect[3] - y + 0.5).clamp(0.0, 1.0)
}

#[test]
fn rounded_clip_direct_nested_and_rectangular_intersections() {
    fn view(mode: &u8, _: &mut Cx<u8>) -> El {
        let white = el().size(100.0, 100.0).absolute(-30.0, -30.0).fill(Color::WHITE);
        let child = if *mode == 0 { white } else {
            el().size(40.0, 48.0).absolute(14.0, -8.0).clip_rounded(10.0).child(white)
        };
        let capsule = el().size(48.0, 32.0).absolute(8.0, 8.0).clip_rounded(16.0).child(child);
        if *mode == 2 {
            // Clip starts at x=24 without recentering either rounded rectangle.
            el().child(el().size(40.0, 48.0).absolute(24.0, 0.0).clip_soft(0.0)
                .child(el().size(64.0, 48.0).absolute(-24.0, 0.0).child(capsule)))
        } else { el().child(capsule) }
    }
    assert_eq!(std::mem::size_of::<Instance>(), 128);
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 48);
    for mode in 0..3 {
        let mut rt = Runtime::new(mode, view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
        rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 48));
        if mode == 0 { assert_eq!(renderer.layer_stats().extra_passes, 0, "ordinary rounded clip stays direct"); }
        let out = pixels(&gpu, &renderer, &target);
        for y in 0..48 { for x in 0..64 {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let mut a = mask(px, py, [8.0, 8.0, 56.0, 40.0], 16.0);
            if mode > 0 { a *= mask(px, py, [22.0, 0.0, 62.0, 48.0], 10.0); }
            if mode == 2 && x < 24 { a = 0.0; }
            let got = out[(y * 64 + x) * 4 + 3];
            assert!(got.abs_diff((a * 255.0).round() as u8) <= 1, "mode={mode} ({x},{y}): {got} vs {a}");
        } }
    }
}

#[test]
fn rounded_clip_blurred_exiting_chip_stays_inside_capsule() {
    type State = (bool, Live<[f32; 2]>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut capsule = el().size(400.0, 40.0).absolute(24.0, 24.0);
        if s.0 { capsule = capsule.clip_rounded(20.0); }
        el().child(capsule.child(el().size(72.0, 28.0).absolute(370.0, 6.0)
            .live_offset(&s.1).opacity(0.65).blur(1.5).radius(8.0)
            .fill(Color { r: 0.06, g: 0.32, b: 1.0, a: 1.0 })
            .child(crate::text("Leaving").color(Color::WHITE).absolute(8.0, 6.0))))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 896, 176);
    let mut plain = Runtime::new((false, Live::new([0.0; 2])), view, (448.0, 88.0), Env { scale: 2.0, system_dark: true });
    let mut clipped = Runtime::new((true, Live::new([0.0; 2])), view, (448.0, 88.0), Env { scale: 2.0, system_dark: true });
    let mut sheet = vec![0; 1792 * 528 * 4];
    for (row, offset) in [0.0, 26.0, 32.0].into_iter().enumerate() {
        let mut outputs = Vec::new();
        for rt in [&mut plain, &mut clipped] {
            rt.state.1.set([offset, 0.0]); rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (896, 176));
            outputs.push(pixels(&gpu, &renderer, &target));
        }
        let mut retained = false;
        let mut removed = false;
        for y in 0..176 { for x in 0..896 {
            let a = mask(x as f32 + 0.5, y as f32 + 0.5, [48.0, 48.0, 848.0, 128.0], 40.0);
            let at = (y * 896 + x) * 4;
            let want = (outputs[0][at + 3] as f32 * a).round() as u8;
            assert!(outputs[1][at + 3].abs_diff(want) <= 1, "offset={offset} ({x},{y})");
            if a == 0.0 { assert_eq!(&outputs[1][at..at + 4], &[0; 4]); removed |= outputs[0][at + 3] > 0; }
            if a == 1.0 && outputs[1][at + 3] > 0 { retained = true; }
            // Dark review sheet with a subtle capsule guide, left unmasked / right clipped.
            for (column, out) in outputs.iter().enumerate() {
                let dst = ((row * 176 + y) * 1792 + column * 896 + x) * 4;
                let alpha = out[at + 3] as f32 / 255.0;
                for c in 0..3 {
                    let v = out[at + c] as f32 / 255.0;
                    let linear = if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
                    let linear = linear + (0.008 + a * 0.025) * (1.0 - alpha);
                    sheet[dst + c] = ((1.055 * linear.powf(1.0 / 2.4) - 0.055) * 255.0).clamp(0.0, 255.0) as u8;
                }
                sheet[dst + 3] = 255;
            }
        } }
        assert!(removed && retained, "clip removes the halo but keeps off-wall source contributions");
        let layer = clipped.instances.iter().find(|i| i.params[1] == KIND_LAYER && i.params[2] > 0.0).unwrap();
        assert_eq!(layer.clip, [48.0, 48.0, 848.0, 128.0]);
        assert_eq!(f32::from_bits(layer.fade[3]), 40.0);
        assert_eq!(layer.uv, [0.0; 4]);
        assert_eq!(layer.radii, [0.0; 4]);
    }
    if let Some(path) = std::env::var_os("RUNE_CLIP_STILL") {
        write_png(&std::path::PathBuf::from(path), 1792, 528, &sheet).unwrap();
    }
}

#[test]
fn rounded_clip_preserves_every_primitive_lane_and_feathers() {
    let gpu = Gpu::headless();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 64, 48, format);
    let mut atlas = crate::text::TextSystem::default().atlas;
    for y in 0..48 { atlas.data[y * crate::text::ATLAS_SIZE as usize..y * crate::text::ATLAS_SIZE as usize + 64].fill(128); }
    atlas.dirty = Some((0, 48));
    let mut images = crate::pixels::ImageAtlas::default();
    let image = crate::pixels::Pixels::new(); image.set(1, 1, vec![80, 160, 240, 200]);
    let image = images.place(&image, [64.0, 48.0]).unwrap();
    renderer.upload_images(&gpu.device, &gpu.queue, &mut images);
    for kind in 0..8 {
        let mut instance = Instance { rect: [0.0, 0.0, 64.0, 48.0], radii: [3.0, 5.0, 7.0, 9.0],
            fill: [0.2, 0.4, 0.6, 0.8], border: [0.8, 0.1, 0.2, 0.9], params: [2.0, kind as f32, 2.0, 1.4],
            uv: [0.0; 4], clip: [8.0, 8.0, 56.0, 40.0],
            fade: pack_fade([1.0, 2.0, 3.0, 4.0], [8.0, 8.0, 56.0, 40.0], [8.0, 8.0, 56.0, 40.0]) };
        match kind {
            2 => { instance.params[0] = 0.0; instance.radii = [0.0; 4]; instance.uv = [0.0, 0.0, 64.0, 48.0]; }
            3 => { instance.uv = [4.0, 6.0, 0.0, 0.0]; instance.params[0] = 12.0; }
            4 => instance.uv = [0.8, 0.2, 0.3, 1.0],
            5 => { instance.radii = [4.0, 24.0, 60.0, 24.0]; instance.params[0] = 24.0; instance.params[2] = 1.0; }
            6 => { instance.radii = [-20.0, -20.0, 100.0, -20.0]; instance.uv = [32.0, 100.0, 0.0, 0.0]; instance.params[0] = 7.0; }
            7 => { instance.uv = image.rect.map(|v| v as f32); instance.params[0] = image.page as f32; instance.params[2] = 0.0; }
            _ => {}
        }
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[instance], (64, 48));
        let plain = pixels(&gpu, &renderer, &target);
        instance.round_clip(16.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &[instance], (64, 48));
        let clipped = pixels(&gpu, &renderer, &target);
        assert_eq!(renderer.layer_stats().extra_passes, 0);
        let mut cut = false;
        for y in 0..48 { for x in 0..64 {
            let a = mask(x as f32 + 0.5, y as f32 + 0.5, instance.clip, 16.0);
            let at = (y * 64 + x) * 4;
            for c in 0..4 {
                let want = (plain[at + c] as f32 * a).round() as u8;
                assert!(clipped[at + c].abs_diff(want) <= 1, "kind={kind} ({x},{y}) channel={c}");
            }
            cut |= plain[at + 3] > clipped[at + 3];
        } }
        assert!(cut, "kind {kind} exercised a rounded corner");
    }
}

#[test]
fn rounded_clip_native_video_inherits_capsule_without_a_layer() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
    fn view(surface: &NativeSurface, _: &mut Cx<NativeSurface>) -> El {
        el().child(el().size(48.0, 32.0).absolute(8.0, 8.0).clip_rounded(16.0)
            .child(el().size(64.0, 48.0).absolute(-8.0, -8.0).native_surface(surface, SurfaceFit::Fill)))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 48);
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 1, height: 1, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255; 4], vec![]] }, 1).unwrap();
    let mut rt = Runtime::new(surface, view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 48));
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (64, 48));
    assert_eq!(renderer.layer_stats().extra_passes, 0);
    let out = pixels(&gpu, &renderer, &target);
    for y in 0..48 { for x in 0..64 {
        let a = mask(x as f32 + 0.5, y as f32 + 0.5, [8.0, 8.0, 56.0, 40.0], 16.0);
        assert!(out[(y * 64 + x) * 4 + 3].abs_diff((a * 255.0).round() as u8) <= 1);
    } }
}

#[test]
fn rounded_clip_live_truncated_text_preserves_corner_centres() {
    type State = (bool, crate::LiveText, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut capsule = el().size(48.0, 32.0).absolute(8.0, 8.0);
        if s.0 { capsule = capsule.clip_rounded(16.0); }
        let mut label = crate::text("MMMMMMMM").live_text(&s.1).size(44.0, 32.0)
            .absolute(2.0, -2.0).font(crate::TextStyle::new(28.0, 700.0)).color(Color::WHITE);
        if s.2 { label = label.truncate_fade(); }
        el().child(capsule.child(label))
    }
    let gpu = Gpu::headless();
    for scale in [1.0, 2.0] { for truncate in [false, true] {
        let size = ((64.0 * scale) as u32, (48.0 * scale) as u32);
        let target = Offscreen::new(&gpu.device, size.0, size.1);
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let value = crate::LiveText::new("MMMMMMMM");
        let mut plain = Runtime::new((false, value.clone(), truncate), view, (64.0, 48.0), Env::new(true, scale));
        let mut clipped = Runtime::new((true, value.clone(), truncate), view, (64.0, 48.0), Env::new(true, scale));
        for words in ["MMMMMMMM", "WW", "MMMMMMMMMMMM"] {
            value.set(words);
            let mut outputs = Vec::new();
            for rt in [&mut plain, &mut clipped] {
                rt.frame(0.0);
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
                outputs.push(pixels(&gpu, &renderer, &target));
            }
            let mut cut = false;
            let mut kept = false;
            for y in 0..size.1 { for x in 0..size.0 {
                let a = mask(x as f32 + 0.5, y as f32 + 0.5,
                    [8.0, 8.0, 56.0, 40.0].map(|v| v * scale), 16.0 * scale);
                let at = ((y * size.0 + x) * 4 + 3) as usize;
                let want = (outputs[0][at] as f32 * a).round() as u8;
                assert!(outputs[1][at].abs_diff(want) <= 1,
                    "scale={scale} truncate={truncate} words={words} ({x},{y})");
                cut |= outputs[0][at] > outputs[1][at];
                kept |= outputs[1][at] > 0;
            } }
            assert!(cut && kept, "exercise glyph coverage inside and outside the corner");
        }
    } }
}

#[test]
fn live_text_clear_of_rounded_corners_draws_without_a_layer() {
    type State = (bool, crate::LiveText);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut card = el().size(120.0, 64.0).absolute(8.0, 8.0).radius(16.0).fill(crate::oklch(0.25, 0.04, 260.0));
        if s.0 { card = card.clip_rounded(16.0); }
        let label = crate::text("0000").live_text(&s.1).size(80.0, 24.0)
            .absolute(10.0, 8.0).font(crate::TextStyle::new(20.0, 700.0)).color(Color::WHITE);
        el().child(card.child(label))
    }
    let gpu = Gpu::headless();
    for scale in [1.0, 2.0] {
        let size = ((136.0 * scale) as u32, (80.0 * scale) as u32);
        let target = Offscreen::new(&gpu.device, size.0, size.1);
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let value = crate::LiveText::new("0000");
        let mut plain = Runtime::new((false, value.clone()), view, (136.0, 80.0), Env::new(true, scale));
        let mut clipped = Runtime::new((true, value.clone()), view, (136.0, 80.0), Env::new(true, scale));
        for words in ["0000", "8888", "WWWW"] {
            value.set(words);
            let mut outputs = Vec::new();
            for rt in [&mut plain, &mut clipped] {
                rt.frame(0.0);
                assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER), "scale={scale} words={words}: no layer");
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
                outputs.push(pixels(&gpu, &renderer, &target));
            }
            // The glyphs match; the card's own edge differs, a hard clip over its feathered fill.
            let (x0, y0, x1, y1) = ((18.0 * scale) as u32, (16.0 * scale) as u32, (98.0 * scale) as u32, (40.0 * scale) as u32);
            let mut lit = false;
            for y in y0..y1 { for x in x0..x1 {
                let at = ((y * size.0 + x) * 4) as usize;
                assert!((0..4).all(|c| outputs[0][at + c].abs_diff(outputs[1][at + c]) <= 1), "scale={scale} words={words} ({x},{y})");
                lit |= outputs[1][at] > 200;
            } }
            assert!(lit, "the glyphs drew");
        }
    }
}

#[test]
fn rounded_clip_and_dissolve_follow_retained_presence() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        el().size(80.0, 40.0).clip_rounded(20.0).dissolve()
            .children(show.then(|| el().key("chip").size(40.0, 28.0).fill(Color::WHITE)
                .transition(rune_motion::Transition::blur_replace())))
    }
    let mut rt = Runtime::new(true, view, (80.0, 40.0), Env { scale: 2.0, system_dark: true });
    rt.frame(0.0);
    assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER));
    rt.state = false; rt.invalidate(); rt.frame(0.0); rt.frame(0.04);
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER && i.params[0] == 1.0 && f32::from_bits(i.fade[3]) == 40.0));
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER && i.params[2] > 0.0));
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER));
}

#[test]
fn rounded_clip_leaves_central_scroll_rectangles_on_the_direct_path() {
    type State = (bool, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut capsule = el().size(80.0, 48.0).absolute(8.0, 8.0);
        if s.0 { capsule = capsule.clip_rounded(16.0); }
        let (x, y, w, h) = if s.1 { (0.0, 16.0, 80.0, 16.0) } else { (16.0, 0.0, 48.0, 48.0) };
        el().child(capsule.child(el().absolute(x, y).size(w, h).scroll(crate::ScrollSpec { stick_bottom: false })
            .child(el().size(120.0, 120.0).fill(Color::WHITE))))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 96, 64);
    for horizontal in [false, true] {
        let mut frames = Vec::new();
        for rounded in [false, true] {
            let mut rt = Runtime::new((rounded, horizontal), view, (96.0, 64.0), Env::new(true, 1.0));
            rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (96, 64));
            assert_eq!(renderer.layer_stats().targets, 0);
            frames.push(pixels(&gpu, &renderer, &target));
        }
        assert_eq!(frames[0], frames[1], "central strip misses every rounded corner");
    }
}

#[test]
fn rounded_clip_single_line_input_keeps_the_original_mask() {
    type State = (bool, crate::TextEdit);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut capsule = el().size(80.0, 40.0).absolute(8.0, 8.0);
        if s.0 { capsule = capsule.clip_rounded(20.0); }
        el().child(capsule.child(crate::text_input(&s.1, crate::TextStyle::new(28.0, 700.0))
            .single_line().size(64.0, 40.0).absolute(8.0, 0.0)))
    }
    let gpu = Gpu::headless();
    for scale in [1.0, 2.0] {
        let size = ((96.0 * scale) as u32, (56.0 * scale) as u32);
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        let target = Offscreen::new(&gpu.device, size.0, size.1);
        let mut frames = Vec::new();
        for rounded in [false, true] {
            let mut rt = Runtime::new((rounded, crate::TextEdit::new("MMMMMMMMMMMM")), view,
                (96.0, 56.0), Env::new(true, scale));
            rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
            frames.push(pixels(&gpu, &renderer, &target));
        }
        assert!(frames[0].chunks_exact(4).any(|p| p[3] > 0));
        for y in 0..size.1 { for x in 0..size.0 {
            let a = mask(x as f32 + 0.5, y as f32 + 0.5,
                [8.0, 8.0, 88.0, 48.0].map(|v| v * scale), 20.0 * scale);
            let at = ((y * size.0 + x) * 4 + 3) as usize;
            assert!(frames[1][at].abs_diff((frames[0][at] as f32 * a).round() as u8) <= 1,
                "scale={scale} ({x},{y})");
        } }
    }
}

#[test]
fn per_corner_continuous_card_clips_image_child_and_nested_layers() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 128, 96);

    let card_img = crate::pixels::Pixels::new();
    let mut card_rgba = Vec::with_capacity(80 * 56 * 4);
    for y in 0..56u32 {
        for x in 0..80u32 {
            card_rgba.extend_from_slice(&[
                (36 + x * 2) as u8,
                (64 + y * 3) as u8,
                (210 - x * 2) as u8,
                255,
            ]);
        }
    }
    card_img.set(80, 56, card_rgba);

    // Overflowing 96x72 child image: identical to `card_img` inside the 80x56 card (offset by 8,8),
    // with an opaque bleed ring outside that the card's clip must remove completely.
    let child_img = crate::pixels::Pixels::new();
    let mut child_rgba = Vec::with_capacity(96 * 72 * 4);
    for y in 0..72i32 {
        for x in 0..96i32 {
            if (8..88).contains(&x) && (8..64).contains(&y) {
                let (cx, cy) = ((x - 8) as u32, (y - 8) as u32);
                child_rgba.extend_from_slice(&[
                    (36 + cx * 2) as u8,
                    (64 + cy * 3) as u8,
                    (210 - cx * 2) as u8,
                    255,
                ]);
            } else {
                child_rgba.extend_from_slice(&[255, 60, 120, 255]);
            }
        }
    }
    child_img.set(96, 72, child_rgba);

    // 1. Continuous-corner card with mixed radii (`[14, 14, 0, 0]` top banner and `[18, 6, 14, 4]`
    //    all four distinct): `clip_rounded` follows the element's own `corners` + `continuous_corners`
    //    with no extra call, clipping an overflowing `El::pixels` child to its own edge.
    for corners in [[14.0, 14.0, 0.0, 0.0], [18.0, 6.0, 14.0, 4.0]] {
        type State = (crate::pixels::Pixels, crate::pixels::Pixels, [f32; 4], u8);
        fn view(s: &State, _: &mut Cx<State>) -> El {
            let (card_img, child_img, corners, mode) = (&s.0, &s.1, s.2, s.3);
            match mode {
                // Reference: 80x56 image drawn directly on the continuous-corner card inside its
                // hard bounds.
                0 => el().child(
                    el().size(80.0, 56.0)
                        .absolute(24.0, 20.0)
                        .clip_soft(0.0)
                        .child(
                            el().size(80.0, 56.0)
                                .corners(corners)
                                .continuous_corners()
                                .pixels(card_img),
                        ),
                ),
                // Direct clip path: card with `corners` + `continuous_corners` + `clip_rounded`
                // clipping an overflowing 96x72 child image to its own edge.
                1 => el().child(
                    el().size(80.0, 56.0)
                        .absolute(24.0, 20.0)
                        .corners(corners)
                        .continuous_corners()
                        .clip_rounded(corners[0])
                        .child(el().size(96.0, 72.0).absolute(-8.0, -8.0).pixels(child_img)),
                ),
                // Explicit `clip_corners` on an isolated layer (`opacity(0.9999)` forces a layer).
                _ => el().child(
                    el().size(80.0, 56.0)
                        .absolute(24.0, 20.0)
                        .continuous_corners()
                        .clip_corners(corners)
                        .opacity(0.9999)
                        .child(el().size(96.0, 72.0).absolute(-8.0, -8.0).pixels(child_img)),
                ),
            }
        }

        let mut outs = Vec::new();
        for mode in 0..3u8 {
            let mut rt = Runtime::new(
                (card_img.clone(), child_img.clone(), corners, mode),
                view,
                (128.0, 96.0),
                Env::new(true, 1.0),
            );
            rt.frame(0.0);
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (128, 96));
            if mode == 1 {
                assert_eq!(renderer.layer_stats().extra_passes, 0, "mixed continuous clip stays on the direct path");
            } else if mode == 2 {
                assert_eq!(renderer.layer_stats().extra_passes, 1, "isolated layer path exercised");
            }
            outs.push(pixels(&gpu, &renderer, &target));
        }
        // Check that top-left is rounded away while bottom-right is sharp when corners[2] == 0.0.
        if corners[2] == 0.0 {
            let tl_alpha = outs[1][(20 * 128 + 24) * 4 + 3];
            let br_alpha = outs[1][((20 + 55) * 128 + (24 + 79)) * 4 + 3];
            assert_eq!(tl_alpha, 0, "top-left continuous corner is clipped");
            assert_eq!(br_alpha, 255, "bottom-right zero-radius corner stays sharp");
        }
        for mode in [1usize, 2] {
            for (idx, (&got, &want)) in outs[mode].iter().zip(&outs[0]).enumerate() {
                assert!(
                    got.abs_diff(want) <= 1,
                    "corners={corners:?} mode={mode} byte {idx}: got {got} vs want {want}"
                );
            }
        }
    }

    // 2. Nested mixed-corner clips fall back to an outer layer and intersect both masks.
    fn nested(img: &crate::pixels::Pixels, _: &mut Cx<crate::pixels::Pixels>) -> El {
        el().child(
            el().size(80.0, 56.0)
                .absolute(24.0, 20.0)
                .corners([16.0, 16.0, 0.0, 0.0])
                .continuous_corners()
                .clip_rounded(16.0)
                .child(
                    el().size(80.0, 56.0)
                        .corners([0.0, 0.0, 16.0, 16.0])
                        .continuous_corners()
                        .clip_rounded(16.0)
                        .child(el().size(80.0, 56.0).pixels(img)),
                ),
        )
    }
    let mut rt = Runtime::new(card_img, nested, (128.0, 96.0), Env::new(true, 1.0));
    rt.frame(0.0);
    renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (128, 96));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "nested distinct per-corner clips open one outer layer");
    let out = pixels(&gpu, &renderer, &target);
    for (x, y) in [(24, 20), (103, 20), (103, 75), (24, 75)] {
        assert_eq!(out[(y * 128 + x) * 4 + 3], 0, "all four corners are clipped by the nested intersection at ({x},{y})");
    }
    assert_eq!(out[(48 * 128 + 64) * 4 + 3], 255, "interior remains opaque");
}
