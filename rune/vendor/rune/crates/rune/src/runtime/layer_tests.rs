use super::*;
use crate::{Color, Cx, El, Env, Live, Runtime, el};

pub(super) fn rect(bounds: [f32; 4], color: [f32; 4]) -> Instance {
    Instance { rect: bounds, fill: color, clip: NO_CLIP, params: [0.0, KIND_RECT, 0.0, 1.0], ..Instance::zeroed() }
}

fn group(bounds: [f32; 4], opacity: f32, color: [f32; 4]) -> Vec<Instance> {
    vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity, ..Default::default() }),
        rect(bounds, color), rect([bounds[0] + bounds[2] * 0.25, bounds[1], bounds[2] * 0.5, bounds[3]], color),
        Instance::end_layer()]
}

#[test]
fn raised_children_of_cached_clipped_groups_stay_visible_and_clipped() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(32.0, 32.0).child(el().size(16.0, 16.0).clip_rounded(8.0).cache().opacity(0.5)
            .child(el().size(24.0, 24.0).raise().fill(Color::WHITE)))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut rt = Runtime::new((), view, (32.0, 32.0), Env::new(true, 1.0));
    for _ in 0..3 {
        rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
        let out = pixels(&gpu, &renderer, &target);
        near(&out[(8 * 32 + 8) * 4..(8 * 32 + 9) * 4], rgba([0.5; 4]));
        near(&out[(8 * 32 + 20) * 4..(8 * 32 + 21) * 4], [0; 4]);
        near(&out[0..4], [0; 4]);
    }
}

pub(super) fn pixels(gpu: &Gpu, renderer: &Renderer, target: &Offscreen) -> Vec<u8> {
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut enc, &target.view, [0.0; 4]);
    target.copy(&mut enc);
    renderer.submit(&gpu.queue, enc);
    target.read(&gpu.device)
}

pub(super) fn rgba(linear: [f32; 4]) -> [u8; 4] {
    std::array::from_fn(|i| {
        let v = linear[i];
        let v = if i == 3 { v } else if v <= 0.0031308 { v * 12.92 } else { 1.055 * v.powf(1.0 / 2.4) - 0.055 };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    })
}

pub(super) fn near(got: &[u8], want: [u8; 4]) {
    assert!(got.iter().zip(want).all(|(&a,b)| a.abs_diff(b) <= 1), "{got:?} != {want:?}");
}

#[test]
fn isolated_layers_match_scalar_overlap_crossfade_and_nesting() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let mut instances = group(bounds, 0.5, [1.0; 4]);
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    for x in [6, 16, 26] { near(&out[(8 * 32 + x) * 4..(8 * 32 + x + 1) * 4], rgba([0.5; 4])); }
    assert_eq!(renderer.layer_stats().targets, 1);
    assert_eq!(renderer.layer_stats().bytes, 64 * 64 * 8, "minimum bucket");
    let allocations = renderer.layer_stats().allocations;

    // Two opaque states, each with internal overlap. Source-over crossfade is B(1-t) over
    // A(t): alpha = 1-t+t², NOT 1. Internal child overlap must never alter that oracle.
    for t in [0.25, 0.5, 0.75] {
        instances = group(bounds, t, [1.0, 0.0, 0.0, 1.0]);
        instances.extend(group(bounds, 1.0 - t, [0.0, 0.0, 1.0, 1.0]));
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
        let out = pixels(&gpu, &renderer, &target);
        let want = rgba([t * t, 0.0, 1.0 - t, 1.0 - t + t * t]);
        for x in [6, 16, 26] { near(&out[(8 * 32 + x) * 4..(8 * 32 + x + 1) * 4], want); }
    }
    assert_eq!(renderer.layer_stats().allocations, allocations + 1, "reuse targets while opacity changes");

    instances = vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() })];
    instances.extend(group(bounds, 0.5, [1.0; 4]));
    instances.push(Instance::end_layer());
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([0.25; 4]));

    instances = group(bounds, 0.5, [1.0; 4]);
    for i in &mut instances[..3] { i.clip = [8.0, 5.0, 24.0, 11.0]; }
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
    assert_eq!(renderer.layer_stats().bytes, 64 * 64 * 8, "clip uses the minimum bucket");
    assert_eq!(renderer.layers.plan[0].bounds, [8.0, 5.0, 16.0, 6.0]);
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8 * 32 + 6) * 4..(8 * 32 + 7) * 4], [0; 4]);
    near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([0.5; 4]));

    for alpha in [1.0, 0.0] {
        instances = group(bounds, alpha, [1.0; 4]);
        renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &instances, (32, 16));
        let out = pixels(&gpu, &renderer, &target);
        near(&out[(8 * 32 + 16) * 4..(8 * 32 + 17) * 4], rgba([alpha; 4]));
        assert_eq!(renderer.layer_stats().targets, 0, "release on both endpoints");
        assert_eq!(renderer.layer_stats().extra_passes, 0);
    }
    // Fully opaque streams allocate no planner storage and issue only the ordinary pass.
    let mut ordinary = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    ordinary.prepare(&gpu.device, &gpu.queue, &mut atlas, &[rect(bounds, [1.0; 4])], (32, 16));
    assert_eq!(ordinary.layer_stats(), LayerStats::default());
    assert_eq!(ordinary.layers.plan.capacity(), 0);
    assert_eq!(ordinary.layers.targets.capacity(), 0);
    assert_eq!(ordinary.layers.stack.capacity(), 0);
    assert_eq!(ordinary.layers.pipelines.capacity(), 0);
}

#[test]
fn leaf_dimming_keeps_container_opacity_isolated() {
    fn view(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha)
            .child(el().size(24.0, 8.0).absolute(4.0, 4.0).fill(Color::WHITE))
            .child(el().size(12.0, 8.0).absolute(10.0, 4.0).fill(Color::WHITE))
            .dim(0.5)
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut rt = Runtime::new(Live::new(0.5), view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    for (alpha, targets) in [(0.5, 1), (1.0, 0)] {
        rt.state.set(alpha); rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 16));
        assert_eq!(renderer.layer_stats().targets, targets);
        let out = pixels(&gpu, &renderer, &target);
        near(&out[(8*32+6)*4..(8*32+7)*4], rgba([0.5 * alpha; 4]));
        near(&out[(8*32+16)*4..(8*32+17)*4], rgba([0.75 * alpha; 4]));
    }
}

#[test]
fn dissolve_keeps_opaque_coverage_and_isolates_each_state() {
    type State = (Live<f32>, Live<f32>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let state = |alpha: &Live<f32>, color: Color| {
            el().size(24.0, 8.0).absolute(4.0, 4.0).live_opacity(alpha)
                .child(el().size(24.0, 8.0).fill(color))
                .child(el().size(12.0, 8.0).absolute(6.0, 0.0).fill(color))
        };
        el().size(32.0, 16.0).dissolve()
            .child(state(&s.0, Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }))
            .child(state(&s.1, Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }))
    }
    let gpu = Gpu::headless();
    // Linear destination makes the exact colour-average assertion independent of sRGB encoding.
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 32, 16, format);
    let mut rt = Runtime::new((Live::new(0.5), Live::new(0.5)), view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    for t in [0.5, 0.0, 0.25, 0.75, 1.0] {
        rt.state.0.set(1.0 - t); rt.state.1.set(t); rt.frame(0.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 16));
        if t == 0.0 || t == 1.0 {
            assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER), "no dissolve scope at rest");
            assert_eq!(renderer.layer_stats().extra_passes, 0);
        } else {
            assert!(renderer.layers.targets[0].is_some(), "mid-fade dissolve must isolate");
        }
        let out = pixels(&gpu, &renderer, &target);
        let want = [((1.0-t)*255.0).round() as u8, 0, (t*255.0).round() as u8, 255];
        for x in [6, 16, 26] {
            near(&out[(8*32+x)*4..(8*32+x+1)*4], want);
            assert_eq!(out[(8*32+x)*4+3], 255, "opaque coverage at t={t}");
        }
        near(&out[..4], [0; 4]);
    }
    // The first/last isolated frame must not introduce a rasterization jump. These deltas
    // are below an output byte but above zero, so they exercise both sides of scope selection.
    for (rest, moving) in [(0.0, 0.00001), (1.0, 0.99999)] {
        let mut frames = Vec::new();
        for t in [rest, moving] {
            rt.state.0.set(1.0 - t); rt.state.1.set(t); rt.frame(0.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 16));
            assert_eq!(renderer.layer_stats().targets > 0, t == moving);
            frames.push(pixels(&gpu, &renderer, &target));
        }
        assert_eq!(frames[0], frames[1], "scope boundary at {rest}");
    }
    // A sole multi-draw state at opacity 1 still composites internally with source-over.
    // Using translucent primitives distinguishes correct isolation from additive saturation.
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let mut all = vec![Instance::begin_layer(bounds, NO_CLIP, LayerSpec { blend: Blend::PlusLighter, ..Default::default() })];
    all.extend(group(bounds, 1.0, [0.5, 0.0, 0.0, 0.5]));
    all.push(Instance::end_layer());
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &all, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8*32+6)*4..(8*32+7)*4], [128,0,0,128]);
    near(&out[(8*32+16)*4..(8*32+17)*4], [191,0,0,191]);
    // Saturate colour as well as alpha, before either the dissolve or enclosing group's fade.
    let nested = [Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, ..Default::default() }),
        Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() }),
        rect(bounds, [1.0; 4]), rect(bounds, [1.0; 4]), Instance::end_layer(), Instance::end_layer()];
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &nested, (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8*32+16)*4..(8*32+17)*4], [64; 4]);
}

#[test]
fn dissolve_samples_sequence_opacity_before_selecting_scopes() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        const OUT: &[crate::Beat] = &[crate::Beat::new(0.0),
            crate::Beat::new(1.0).opacity(0.0).easing(crate::Easing::LINEAR)];
        const IN: &[crate::Beat] = &[crate::Beat::new(0.0).opacity(0.0),
            crate::Beat::new(1.0).easing(crate::Easing::LINEAR)];
        el().size(32.0, 16.0).dissolve()
            .child(el().size(32.0, 16.0).absolute(0.0, 0.0).fill(Color::WHITE).sequence(crate::Sequence::new(OUT)))
            .child(el().size(32.0, 16.0).absolute(0.0, 0.0).fill(Color::WHITE).sequence(crate::Sequence::new(IN)))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 16);
    let mut rt = Runtime::new((), view, (32.0, 16.0), Env::new(true, 1.0));
    for (dt, active) in [(0.0, false), (0.5, true), (0.5, false)] {
        rt.frame(dt);
        assert_eq!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER), active);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 16));
        let out = pixels(&gpu, &renderer, &target);
        assert!(out.chunks_exact(4).all(|p| p[3] == 255), "complementary sequences retain opaque coverage");
    }
}

#[test]
fn dissolve_adds_direct_primitives_and_native_surfaces_then_clamps_before_opacity() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceDraw, SurfaceFit};
    let gpu = Gpu::headless();
    let format = wgpu::TextureFormat::Rgba8Unorm;
    let mut renderer = Renderer::new(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 32, 16, format);
    let mut atlas = crate::text::TextSystem::default().atlas;
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let content = [rect([0.0, 0.0, 32.0, 16.0], [0.0, 1.0, 0.0, 1.0]),
        Instance::begin_layer(bounds, NO_CLIP, LayerSpec { opacity: 0.5, blend: Blend::PlusLighter, ..Default::default() }),
        rect(bounds, [1.0, 0.0, 0.0, 1.0]), Instance::end_layer()];
    renderer.prepare(&gpu.device, &gpu.queue, &mut atlas, &content, (32, 16));
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 1, height: 1, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255, 0, 0, 255], vec![]] }, 1).unwrap();
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &[SurfaceDraw { surface, fit: SurfaceFit::Fill,
        before_instance: 3, rect: bounds, radii: [0.0; 4], clip: NO_CLIP, fade: [0; 4], opacity: 1.0 }], (32, 16));
    let out = pixels(&gpu, &renderer, &target);
    // Red + blue saturates alpha to 1, then .5 opacity composites over green with source-over.
    near(&out[(8*32+16)*4..(8*32+17)*4], [128,128,128,255]);
    near(&out[..4], [0,255,0,255]);
}

#[test]
fn scale_and_fade_reuse_bucket_for_sixty_growing_frames() {
    type State = (Live<f32>, Live<f32>);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        el().size(256.0, 256.0).child(el().size(180.0, 180.0)
            .live_scale(&s.0).live_opacity(&s.1)
            .child(el().size(180.0, 180.0).fill(Color::WHITE))
            .child(el().size(90.0, 90.0).fill(Color::WHITE)))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 256, 256);
    let mut rt = Runtime::new((Live::new(0.9), Live::new(0.1)), view, (256.0, 256.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (256, 256));
    let allocations = renderer.layer_stats().allocations;
    let first_bounds = renderer.layers.plan[0].bounds;
    assert_eq!(allocations, 1);
    for frame in 1..=60 {
        let t = frame as f32 / 60.0;
        rt.state.0.set(0.9 + 0.1 * t);
        rt.state.1.set(0.1 + 0.8 * t);
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (256, 256));
        let out = pixels(&gpu, &renderer, &target);
        near(&out[(90 * 256 + 90) * 4..(90 * 256 + 91) * 4], rgba([0.1 + 0.8 * t; 4]));
        assert_eq!(renderer.layer_stats().allocations, allocations, "frame {frame}");
    }
    assert!(renderer.layers.plan[0].bounds[2] > first_bounds[2]);
    // A genuine shrink drops an oversized allocation, then the tiny minimum bucket stays put.
    for size in [20.0, 19.0, 20.0] {
        let content = group([4.0, 4.0, size, size], 0.5, [1.0; 4]);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &content, (256, 256));
        assert_eq!(renderer.layer_stats().bytes, 64 * 64 * 8);
        assert_eq!(renderer.layer_stats().allocations, allocations + 1);
    }
}

#[test]
fn live_group_opacity_changes_scopes_without_view_or_layout_work() {
    fn view(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha)
            .child(el().size(24.0, 12.0).fill(Color::WHITE))
            .child(el().size(24.0, 12.0).pin(Some(4.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let alpha = Live::new(1.0);
    let mut rt = Runtime::new(alpha.clone(), view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    let commits = rt.stats.commits;
    let original = bytemuck::cast_slice::<Instance, u8>(&rt.instances).to_vec();
    alpha.set(0.5);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(rt.instances.first().unwrap().params[1], KIND_LAYER);
    assert_eq!(rt.instances.last().unwrap().params[1], KIND_LAYER_END);
    assert_eq!(rt.instances[1].fill[3], 1.0);
    alpha.set(0.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.instances.is_empty());
    alpha.set(0.0009);
    rt.frame(1.0 / 60.0);
    assert!(rt.instances.is_empty(), "hide sub-threshold tails");
    alpha.set(1.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.stats.commits, commits);
    assert_eq!(bytemuck::cast_slice::<Instance, u8>(&rt.instances), original);
}

#[test]
fn isolated_exit_descendants_stay_inside_their_fading_parent() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        let mut fading = el().key("fading").size(24.0, 16.0).opacity(0.5)
            .child(el().key("kept").size(16.0, 16.0).fill(Color::WHITE));
        if *show { fading = fading.child(el().key("leaving").size(16.0, 16.0).fill(Color::WHITE)); }
        el().size(32.0, 16.0).child(fading)
            .child(el().key("sibling").size(8.0, 16.0).pin(Some(24.0), Some(0.0), None, None).fill(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }))
    }
    let mut rt = Runtime::new(true, view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    rt.state = false; rt.invalidate(); rt.frame(0.0);
    let start = rt.instances.iter().position(|i| i.params[1] == KIND_LAYER).unwrap();
    let end = rt.instances.iter().position(|i| i.params[1] == KIND_LAYER_END).unwrap();
    assert_eq!(end - start, 3, "both retained descendants share the scope");
    assert_eq!(rt.instances[end + 1].fill, [0.0, 0.0, 1.0, 1.0], "sibling paints outside the group");
}

#[test]
fn single_leaf_groups_keep_the_direct_path_and_springs_stay_live() {
    fn leaf(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha).child(el().size(16.0, 16.0).fill(Color::WHITE))
    }
    fn group_view(alpha: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(32.0, 16.0).live_opacity(alpha.smooth(crate::Spring::DEFAULT))
            .child(el().size(16.0, 16.0).fill(Color::WHITE))
            .child(el().size(16.0, 16.0).pin(Some(8.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let alpha = Live::new(0.5);
    let env = Env { scale: 1.0, system_dark: true };
    let mut rt = Runtime::new(alpha.clone(), leaf, (32.0, 16.0), env);
    rt.frame(0.0);
    assert_eq!(rt.instances.len(), 1);
    assert_eq!(rt.instances[0].fill[3], 0.5);
    alpha.set(1.0);
    let mut rt = Runtime::new(alpha.clone(), group_view, (32.0, 16.0), env);
    rt.frame(0.0);
    let commits = rt.stats.commits;
    alpha.set(0.5);
    rt.frame(1.0 / 60.0);
    let a = rt.instances[0].fill[3];
    rt.frame(1.0 / 60.0);
    let b = rt.instances[0].fill[3];
    assert_eq!(rt.instances[0].params[1], KIND_LAYER);
    assert!(b < a && b > 0.5, "spring samples change the group each presentation: {a} -> {b}");
    assert_eq!(rt.stats.commits, commits);
}

#[test]
fn clipped_single_draw_fades_do_not_allocate_targets() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceFit};
    type State = (Option<NativeSurface>, u8);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let child = el().size(64.0, 48.0).absolute(-8.0, -8.0);
        let child = if let Some(surface) = &s.0 { child.native_surface(surface, SurfaceFit::Fill) }
            else { child.fill(Color::WHITE) };
        let clip = el().size(48.0, 32.0).absolute(8.0, 8.0);
        let clip = match s.1 {
            0 => clip.clip_soft(0.0),
            1 => clip.clip_fade(4.0, 4.0, 4.0, 4.0),
            _ => clip.scroll(crate::ScrollSpec { stick_bottom: false }),
        };
        el().child(clip.child(el().size(48.0, 32.0).opacity(0.5).child(child)))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 64, 48);
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 1, height: 1, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255; 4], vec![]] }, 1).unwrap();
    for mode in 0..3 {
        let mut frames = Vec::new();
        for video in [false, true] {
            let mut rt = Runtime::new((video.then(|| surface.clone()), mode), view, (64.0, 48.0), Env::new(true, 1.0));
            rt.frame(0.0);
            assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER));
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (64, 48));
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (64, 48));
            assert_eq!(renderer.layer_stats().targets, 0, "mode={mode} video={video}");
            let out = pixels(&gpu, &renderer, &target);
            assert_eq!(out[(24 * 64 + 32) * 4 + 3], 128);
            assert_eq!(out[(24 * 64 + 4) * 4 + 3], 0);
            if mode == 1 { assert!(out[(24 * 64 + 8) * 4 + 3] < 32); }
            frames.push(out);
        }
        assert_eq!(frames[0], frames[1], "primitive and video inherit identical clips/fades");
    }
}

#[test]
fn opacity_motion_samples_group_alpha_until_the_terminal_frame() {
    fn view(show: &bool, _: &mut Cx<bool>) -> El {
        el().size(32.0, 16.0).opacity(if *show { 1.0 } else { 0.0 })
            .opacity_motion(rune_motion::Curve::ease_in_out(0.2))
            .child(el().size(16.0, 16.0).fill(Color::WHITE))
            .child(el().size(16.0, 16.0).pin(Some(8.0), Some(0.0), None, None).fill(Color::WHITE))
    }
    let mut rt = Runtime::new(true, view, (32.0, 16.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    rt.state = false; rt.invalidate(); rt.frame(0.0);
    let commits = rt.stats.commits;
    rt.frame(0.05);
    let first = rt.instances[0].fill[3];
    assert_eq!(rt.instances[0].params[1], KIND_LAYER);
    rt.frame(0.05);
    assert!(rt.instances[0].fill[3] < first);
    assert_eq!(rt.instances[1].fill[3], 1.0);
    for _ in 0..24 { rt.frame(1.0 / 60.0); }
    assert!(rt.instances.is_empty());
    assert_eq!(rt.stats.commits, commits);
}

#[test]
fn isolated_layers_preserve_image_video_spring_and_list_order() {
    use crate::native_surface::{ByteFrame, ColorInfo, Format, NativeSurface, SurfaceDraw, SurfaceFit};
    use crate::gpu::springs::{Node, Track};
    use crate::pixels::Pixels;
    let gpu = Gpu::headless();
    let bounds = [4.0, 4.0, 24.0, 8.0];
    let mut content = group(bounds, 0.5, [1.0, 0.0, 0.0, 1.0]);
    // A glyph, a vector and a page-1 image inside the nonzero-origin layer. Absolute spring
    // indices differ from local draw indices, so a shifted subrange would use the wrong node.
    let mut atlas = crate::text::TextSystem::default().atlas;
    atlas.data[0] = 255; atlas.dirty = Some((0, 1));
    content.insert(3, Instance { rect: [9.0, 6.0, 1.0, 1.0], fill: [1.0; 4], clip: NO_CLIP,
        params: [0.0, KIND_GLYPH, 0.0, 1.0], ..Instance::zeroed() });
    let px = Pixels::new(); px.set(1, 1, vec![0, 255, 0, 255]);
    let mut images = crate::pixels::ImageAtlas::default();
    let full_page = Pixels::new(); full_page.set(1024, 1024, vec![255; 1024 * 1024 * 4]);
    images.place(&full_page, [1024.0; 2]).unwrap();
    let image = images.place(&px, [2.0, 2.0]).unwrap();
    assert_eq!(image.page, 1);
    content.insert(4, Instance { rect: [15.0, 6.0, 2.0, 2.0], fill: [0.0, 0.0, 0.0, 1.0],
        params: [image.page as f32, KIND_IMAGE, 0.0, 1.0], uv: image.rect.map(|v| v as f32), clip: NO_CLIP, ..Instance::zeroed() });
    let prefix = [rect([0.0, 0.0, 2.0, 2.0], [1.0; 4])];
    let zero = Track::constant(0.0);
    let nodes = [Node { x: zero, y: zero, opacity: Track::constant(1.0) },
        Node { x: Track::constant(1.0), y: zero, opacity: Track::constant(1.0) }];
    let mut ids = vec![0; prefix.len() + content.len()];
    ids[4] = 1; // glyph at absolute index 4, translated from x=9 to x=10
    let mut renderer = Renderer::with_springs(&gpu.device, OFFSCREEN_FORMAT, &nodes, &ids);
    let ranges = renderer.prepare_lists(&gpu.device, &gpu.queue, &mut atlas, &[&prefix, &content], (32, 16));
    renderer.upload_images(&gpu.device, &gpu.queue, &mut images);
    let surface = NativeSurface::new();
    surface.push_bytes(ByteFrame { width: 2, height: 2, format: Format::Bgra, color: ColorInfo::default(),
        planes: [vec![255, 0, 0, 255].repeat(4), vec![]] }, 1).unwrap();
    renderer.prepare_surfaces(&gpu.device, &gpu.queue, &[SurfaceDraw { surface, fit: SurfaceFit::Fill,
        before_instance: 4, rect: [22.0, 6.0, 2.0, 2.0], radii: [0.0; 4], clip: NO_CLIP, fade: [0; 4], opacity: 1.0 }], (32, 16));
    let format = wgpu::TextureFormat::Rgba8Unorm;
    renderer.add_format(&gpu.device, format);
    let target = Offscreen::with_format(&gpu.device, 32, 16, format);
    let mut enc = gpu.device.create_command_encoder(&Default::default());
    renderer.encode_layers(&mut enc);
    {
        let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: None, color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.view, depth_slice: None, resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
            })], depth_stencil_attachment: None, timestamp_writes: None, occlusion_query_set: None, multiview_mask: None,
        });
        for range in ranges { renderer.draw_range(&mut pass, format, range); }
    }
    target.copy(&mut enc); renderer.submit(&gpu.queue, enc);
    let out = target.read(&gpu.device);
    for (x, y, want) in [(0,0,[255;4]), (9,6,[128,0,0,128]), (10,6,[128;4]), (15,6,[0,128,0,128]), (22,6,[0,0,128,128])] {
        near(&out[(y*32+x)*4..(y*32+x+1)*4], want);
    }
}
