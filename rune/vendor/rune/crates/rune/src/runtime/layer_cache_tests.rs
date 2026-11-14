use super::*;
use super::tests::{near, pixels, rgba};
use crate::{Color, Cx, El, Env, Live, Runtime, el};

#[test]
fn cache_matches_uncached_at_rest_and_skips_rerasterize_on_ancestor_live_offset() {
    type State = (bool, Live<[f32; 2]>);
    fn crisp_view(s: &State, _: &mut Cx<State>) -> El {
        let mut card = crate::col()
            .size(40.0, 28.0)
            .padding(4.0)
            .gap(2.0)
            .fill(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 })
            .child(el().size(24.0, 8.0).fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }))
            .child(el().size(16.0, 8.0).fill(Color::WHITE));
        if s.0 {
            card = card.cache();
        }
        el().size(64.0, 48.0).padding(6.0).live_offset(&s.1).child(card)
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut card = crate::col()
            .size(40.0, 28.0)
            .padding(4.0)
            .gap(2.0)
            .fill(Color { r: 0.2, g: 0.35, b: 0.6, a: 1.0 })
            .radius(6.0)
            .border(1.0, Color::WHITE)
            .child(crate::text("Cached").color(Color::WHITE))
            .child(el().size(24.0, 8.0).fill(Color { r: 0.9, g: 0.3, b: 0.2, a: 1.0 }).radius(3.0));
        if s.0 {
            card = card.cache();
        }
        el().size(64.0, 48.0)
            .padding(6.0)
            .live_offset(&s.1)
            .child(card)
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 64, 48);

    // 1. Crisp card is byte-for-byte identical between cached and uncached at rest and under pan.
    let mut rt_u = Runtime::new((false, Live::new([0.0, 0.0])), crisp_view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
    let mut ren_u = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    rt_u.frame(0.0);
    ren_u.prepare(&gpu.device, &gpu.queue, &mut rt_u.text.atlas, &rt_u.instances, (64, 48));
    let mut rt_c = Runtime::new((true, Live::new([0.0, 0.0])), crisp_view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
    let mut ren_c = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    rt_c.frame(0.0);
    ren_c.prepare(&gpu.device, &gpu.queue, &mut rt_c.text.atlas, &rt_c.instances, (64, 48));
    assert_eq!(pixels(&gpu, &ren_c, &target), pixels(&gpu, &ren_u, &target), "cached vs uncached must be pixel-for-pixel identical at rest");

    // 2. Rich card with text and antialiased rounded corners under ancestor live_offset pan.
    let off_uncached = Live::new([0.0, 0.0]);
    let mut rt_uncached = Runtime::new((false, off_uncached.clone()), view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
    let mut ren_uncached = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    rt_uncached.frame(0.0);
    ren_uncached.prepare(&gpu.device, &gpu.queue, &mut rt_uncached.text.atlas, &rt_uncached.instances, (64, 48));
    let px_uncached = pixels(&gpu, &ren_uncached, &target);

    let off_cached = Live::new([0.0, 0.0]);
    let mut rt_cached = Runtime::new((true, off_cached.clone()), view, (64.0, 48.0), Env { scale: 1.0, system_dark: true });
    let mut ren_cached = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    rt_cached.frame(0.0);
    ren_cached.prepare(&gpu.device, &gpu.queue, &mut rt_cached.text.atlas, &rt_cached.instances, (64, 48));
    assert_eq!(ren_cached.layer_stats().targets, 1);
    assert_eq!(ren_cached.layer_stats().extra_passes, 1, "initial frame rasterizes the cache");
    let px_cached = pixels(&gpu, &ren_cached, &target);
    let max_diff = px_cached.iter().zip(&px_uncached).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
    assert!(max_diff <= 1, "cached vs uncached at rest max_diff={max_diff}");
    assert_eq!(rt_cached.elements().len(), rt_uncached.elements().len(), "elements() unchanged");

    // Panning via an ancestor's live_offset must elide child instances and not re-rasterize.
    for delta in [[4.0, 2.0], [10.0, 6.0], [3.0, 8.0], [0.0, 0.0]] {
        off_uncached.set(delta);
        rt_uncached.frame(1.0 / 60.0);
        ren_uncached.prepare(&gpu.device, &gpu.queue, &mut rt_uncached.text.atlas, &rt_uncached.instances, (64, 48));
        let want = pixels(&gpu, &ren_uncached, &target);

        off_cached.set(delta);
        rt_cached.frame(1.0 / 60.0);
        assert_eq!(rt_cached.instances.len(), 2, "cached subtree elides to [KIND_LAYER, KIND_LAYER_END]");
        ren_cached.prepare(&gpu.device, &gpu.queue, &mut rt_cached.text.atlas, &rt_cached.instances, (64, 48));
        assert_eq!(ren_cached.layer_stats().extra_passes, 0, "ancestor live_offset must not re-rasterize");
        assert_eq!(ren_cached.layer_stats().allocations, 1);
        let got = pixels(&gpu, &ren_cached, &target);
        let max_diff = got.iter().zip(&want).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
        assert!(max_diff <= 1, "panned cached output must match uncached at {delta:?} (max_diff={max_diff})");
        if delta == [0.0, 0.0] {
            assert_eq!(got, px_cached, "returning to origin is bit-for-bit identical");
        }
    }
}

#[test]
fn cache_invalidates_on_state_and_hover_changes() {
    fn view(active: &bool, _: &mut Cx<bool>) -> El {
        let fill = if *active {
            Color { r: 0.1, g: 0.8, b: 0.2, a: 1.0 }
        } else {
            Color { r: 0.8, g: 0.1, b: 0.2, a: 1.0 }
        };
        el().size(32.0, 32.0).child(
            el().size(24.0, 24.0)
                .cache()
                .child(
                    el().key("btn")
                        .size(24.0, 24.0)
                        .fill(fill)
                        .hover(Color { r: 0.1, g: 0.2, b: 0.9, a: 1.0 })
                        .on_press(|active: &mut bool| *active = !*active),
                ),
        )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut rt = Runtime::new(false, view, (32.0, 32.0), Env { scale: 1.0, system_dark: true });

    // Frame 1: initial rasterize (red).
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(12 * 32 + 12) * 4..(12 * 32 + 13) * 4], rgba([0.8, 0.1, 0.2, 1.0]));

    // Frame 2: steady at rest (cache hit).
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 0);

    // Frame 3: state change inside cached subtree invalidates and re-renders once settled (green).
    rt.state = true;
    rt.invalidate();
    rt.frame(1.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "state change must invalidate cache");
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(12 * 32 + 12) * 4..(12 * 32 + 13) * 4], rgba([0.1, 0.8, 0.2, 1.0]));

    // Frame 4: hover inside cached subtree invalidates immediately and re-renders (blue hover).
    rt.pointer_move(12.0, 12.0);
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "hover change must invalidate cache");
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(12 * 32 + 12) * 4..(12 * 32 + 13) * 4], rgba([0.1, 0.2, 0.9, 1.0]));

    // Frame 5: settled while hovered hits the cache again.
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 0, "settled hover must hit cache");
}

#[test]
fn cache_draws_live_while_subtree_animates_and_recaches_when_settled() {
    fn view(moved: &bool, _: &mut Cx<bool>) -> El {
        let x = if *moved { 16.0 } else { 0.0 };
        el().size(48.0, 24.0).child(
            el().size(40.0, 20.0)
                .cache()
                .child(
                    el().key("chip")
                        .size(16.0, 16.0)
                        .absolute(x, 0.0)
                        .motion(crate::Spring::DEFAULT)
                        .fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }),
                ),
        )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 48, 24);
    let mut rt = Runtime::new(false, view, (48.0, 24.0), Env { scale: 1.0, system_dark: true });

    // Initial rest: cached.
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (48, 24));
    assert_eq!(renderer.layer_stats().targets, 1);
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (48, 24));
    assert_eq!(renderer.layer_stats().extra_passes, 0);

    // Start animation inside cached subtree: must bypass cache and draw live without re-rasterizing offscreen targets.
    rt.state = true;
    rt.invalidate();
    rt.frame(0.0);
    for _ in 0..3 {
        rt.frame(0.05);
        assert!(rt.instances.iter().all(|i| i.params[1] != KIND_LAYER), "must draw live while animating");
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (48, 24));
        assert_eq!(renderer.layer_stats().extra_passes, 0, "no offscreen pass thrash while animating");
    }

    // Advance past animation duration so it settles: re-caches once at the new position, then hits cache.
    rt.frame(2.0);
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_LAYER && i.params[3] == 1.0), "re-cached once settled");
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (48, 24));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "rasterize once on settle");
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(8 * 48 + 24) * 4..(8 * 48 + 25) * 4], rgba([1.0, 0.0, 0.0, 1.0]));

    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (48, 24));
    assert_eq!(renderer.layer_stats().extra_passes, 0, "subsequent frame hits settled cache");
}

#[test]
fn cache_rerasterizes_at_new_resolution_after_ancestor_scale_settles() {
    fn view(scale: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(200.0, 200.0)
            .padding(60.0)
            .live_scale(scale)
            .child(
                el().size(48.0, 48.0)
                    .cache()
                    .fill(Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 })
                    .child(el().size(24.0, 24.0).fill(Color::WHITE)),
            )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 200, 200);
    let scale = Live::new(1.0);
    let mut rt = Runtime::new(scale.clone(), view, (200.0, 200.0), Env { scale: 1.0, system_dark: true });

    // Frame 1: rasterize at 1.0x (48x48 -> 64x64 bucket).
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    assert_eq!(renderer.layer_stats().bytes, 64 * 64 * 8);
    let _ = pixels(&gpu, &renderer, &target);

    // Frame 2: during scale gesture to 2.0x (dt = 0.016 < STATIC_SCALE_SETTLE), composites scaled without re-rasterizing.
    scale.set(2.0);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 0, "composite existing texture during scale gesture");
    assert_eq!(renderer.layer_stats().bytes, 64 * 64 * 8, "still 1x texture before settle");

    // Frame 3: after scale settles (dt = 0.15 >= STATIC_SCALE_SETTLE), re-rasterizes at 2.0x (96x96 -> 128x128 bucket).
    rt.frame(0.15);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "re-rasterize at new resolution once settled");
    assert_eq!(renderer.layer_stats().bytes, 128 * 128 * 8, "upgraded to 2x texture bucket");
    let out = pixels(&gpu, &renderer, &target);
    assert_eq!(out[(100 * 200 + 100) * 4 + 3], 255);

    // Frame 4: hits the new 2x cache.
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 0);
}

#[test]
fn cache_lru_eviction_under_small_budget_evicts_oldest_and_recaches_cleanly() {
    fn scroll_view(offset: &Live<[f32; 2]>, _: &mut Cx<Live<[f32; 2]>>) -> El {
        let card = |key: &str, y: f32, c: Color| {
            el().key(key)
                .size(24.0, 24.0)
                .absolute(4.0, y)
                .cache()
                .fill(c)
                .child(el().size(10.0, 10.0).fill(Color::WHITE))
        };
        el().size(32.0, 32.0).child(
            el().size(32.0, 300.0)
                .live_offset(offset)
                .child(card("a", 0.0, Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }))
                .child(card("b", 100.0, Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }))
                .child(card("c", 200.0, Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 })),
        )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let one_bucket = 64 * 64 * 8;
    // Budget fits at most 2 cached 64x64 textures.
    renderer.set_cache_budget(one_bucket * 2);
    assert_eq!(renderer.cache_budget(), one_bucket * 2);
    let target = Offscreen::new(&gpu.device, 32, 32);
    let offset = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new(offset.clone(), scroll_view, (32.0, 32.0), Env { scale: 1.0, system_dark: true });

    // Visit card A (y=0): caches A.
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    assert!(renderer.layer_stats().bytes <= one_bucket * 2);
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(16 * 32 + 16) * 4..(16 * 32 + 17) * 4], rgba([1.0, 0.0, 0.0, 1.0]));

    // Visit card B (y=-100): caches B, retains A (2 targets <= budget).
    offset.set([0.0, -100.0]);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    assert_eq!(renderer.layer_stats().targets, 2);
    assert!(renderer.layer_stats().bytes <= one_bucket * 2);
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(16 * 32 + 16) * 4..(16 * 32 + 17) * 4], rgba([0.0, 1.0, 0.0, 1.0]));

    // Return to card A (y=0): A is still resident in budget, so extra_passes == 0!
    offset.set([0.0, 0.0]);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 0, "A was retained in budget");

    // Now visit card C (y=-200): A was used most recently, so oldest (B) is evicted!
    offset.set([0.0, -200.0]);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "C rasterizes and evicts LRU B");
    assert_eq!(renderer.layer_stats().targets, 2);
    assert!(renderer.layer_stats().bytes <= one_bucket * 2);
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(16 * 32 + 16) * 4..(16 * 32 + 17) * 4], rgba([0.0, 0.0, 1.0, 1.0]));

    // Card A (y=0) is still resident (extra_passes == 0), while card B (y=-100) was evicted and re-caches cleanly!
    offset.set([0.0, 0.0]);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 0, "MRU card A stayed resident");

    offset.set([0.0, -100.0]);
    rt.frame(0.016);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "evicted card B re-caches cleanly");
    let out = pixels(&gpu, &renderer, &target);
    near(&out[(16 * 32 + 16) * 4..(16 * 32 + 17) * 4], rgba([0.0, 1.0, 0.0, 1.0]));
}

/// Drives a cached and an uncached runtime through the same steps (`frames` frames each, 1/60 s
/// apart); the cached one must draw the same pixels after every frame. Returns the cached
/// renderer's stats per frame.
fn same_as_uncached<T: Clone + 'static>(state: T, view: fn(&(bool, T), &mut Cx<(bool, T)>) -> El, size: (u32, u32),
    steps: &[(&str, usize, &dyn Fn(&mut Runtime<(bool, T)>))]) -> Vec<(String, LayerStats)> {
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    let env = Env { scale: 1.0, system_dark: true };
    let viewport = (size.0 as f32, size.1 as f32);
    let mut runs: Vec<_> = [false, true].map(|cached| {
        (Runtime::new((cached, state.clone()), view, viewport, env), Renderer::new(&gpu.device, OFFSCREEN_FORMAT))
    }).into();
    let mut stats = Vec::new();
    let initial: &dyn Fn(&mut Runtime<(bool, T)>) = &|_| {};
    for (name, frames, step) in [("rest", 2, initial)].into_iter().chain(steps.iter().copied()) {
        let mut out = Vec::new();
        for (rt, _) in &mut runs { step(rt); }
        for f in 0..frames {
            for (rt, renderer) in &mut runs {
                rt.frame(1.0 / 60.0);
                renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
                out.push(pixels(&gpu, renderer, &target));
            }
            let (want, got) = (&out[out.len() - 2], &out[out.len() - 1]);
            let max_diff = got.iter().zip(want).map(|(a, b)| a.abs_diff(*b)).max().unwrap_or(0);
            assert!(max_diff <= 1, "{name} frame {f}: cached differs from uncached (max_diff={max_diff})");
            stats.push((format!("{name}/{f}"), runs[1].1.layer_stats()));
        }
    }
    assert!(runs[1].0.instances.iter().any(|i| i.params[1] == KIND_LAYER && i.params[3] == 1.0), "the subtree stays cached");
    stats
}

#[test]
fn cache_follows_press_paint_that_lands_without_a_view_run() {
    fn view(s: &(bool, ()), _: &mut Cx<(bool, ())>) -> El {
        let mut card = el().size(24.0, 24.0).child(
            el().size(24.0, 24.0)
                .fill(Color { r: 0.8, g: 0.1, b: 0.2, a: 1.0 })
                .press(Color { r: 0.1, g: 0.2, b: 0.9, a: 1.0 })
                .on_press(|_: &mut (bool, ())| {}),
        );
        if s.0 { card = card.cache(); }
        el().size(32.0, 32.0).child(card)
    }
    let stats = same_as_uncached((), view, (32, 32), &[
        ("hover", 2, &|rt| rt.pointer_move(12.0, 12.0)),
        ("down", 2, &|rt| rt.pointer_down(12.0, 12.0)),
        ("up", 40, &|rt| rt.pointer_up(12.0, 12.0)),
    ]);
    assert_eq!(stats.last().unwrap().1.extra_passes, 0, "the released card hits its cache again");
}

#[test]
fn cache_follows_link_hover_moving_between_links_of_one_text() {
    fn view(s: &(bool, ()), _: &mut Cx<(bool, ())>) -> El {
        let green = crate::LinkPaint::new().bg(Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 });
        let label = crate::text("MMMM MMMM")
            .font(crate::TextStyle::new(16.0, 400.0))
            .color(Color::WHITE)
            .link(crate::Link::new(0..4, "a").hover(green))
            .link(crate::Link::new(5..9, "b").hover(green))
            .on_link(|_: &mut (bool, ()), _, _| {});
        let mut card = el().size(150.0, 24.0).child(label);
        if s.0 { card = card.cache(); }
        el().size(160.0, 32.0).child(card)
    }
    same_as_uncached((), view, (160, 32), &[
        ("first link", 2, &|rt| rt.pointer_move(16.0, 10.0)),
        ("second link", 2, &|rt| rt.pointer_move(90.0, 10.0)),
    ]);
}

#[test]
fn cache_follows_an_outside_hover_group_fading_its_descendants() {
    fn view(s: &(bool, ()), _: &mut Cx<(bool, ())>) -> El {
        let mut card = el().size(24.0, 24.0).fill(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 }).child(
            el().size(16.0, 16.0).fill(Color::WHITE).group_opacity(0.2, 1.0),
        );
        if s.0 { card = card.cache(); }
        el().size(64.0, 32.0).child(crate::row().size(64.0, 32.0).hover_group().child(card))
    }
    same_as_uncached((), view, (64, 32), &[
        ("enter", 3, &|rt| rt.pointer_move(48.0, 16.0)),
        ("leave", 12, &|rt| rt.pointer_move(200.0, 200.0)),
    ]);
}

#[test]
fn cache_redraws_a_same_length_text_change() {
    fn view(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let mut card = el().size(60.0, 24.0).child(crate::text(if s.1 { "BBBB" } else { "AAAA" }).color(Color::WHITE));
        if s.0 { card = card.cache(); }
        el().size(64.0, 32.0).child(card)
    }
    let stats = same_as_uncached(false, view, (64, 32), &[
        ("swap", 40, &|rt| { rt.state.1 = true; rt.invalidate(); }),
    ]);
    let passes: Vec<_> = stats.iter().map(|(_, s)| s.extra_passes).collect();
    assert_eq!(passes[..2], [1, 0]);
    assert_eq!(passes[2..].iter().sum::<usize>(), 1, "the new text rasterizes once: {passes:?}");
    assert_eq!(passes.last(), Some(&0));
}

#[test]
fn cache_invalidates_the_cached_roots_blur() {
    fn view(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let mut card = el().size(24.0, 24.0).absolute(16.0, 16.0)
            .blur(if s.1 { 3.0 } else { 0.0 }).blur_motion(crate::Anim::INSTANT)
            .child(el().size(16.0, 16.0).fill(Color::WHITE));
        if s.0 { card = card.cache(); }
        el().size(64.0, 64.0).child(card)
    }
    same_as_uncached(false, view, (64, 64), &[
        ("blur", 3, &|rt| { rt.state.1 = true; rt.invalidate(); }),
    ]);
}

#[test]
fn clicks_land_inside_a_cached_card_after_a_pan() {
    type S = (u32, Live<[f32; 2]>);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().size(96.0, 64.0).child(
            el().size(96.0, 64.0).live_offset(&s.1).child(
                el().size(32.0, 32.0).cache().fill(Color::WHITE).child(
                    el().key("button").size(16.0, 16.0).fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 })
                        .on_press(|s: &mut S| s.0 += 1),
                ),
            ),
        )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let pan = Live::new([0.0, 0.0]);
    let mut rt = Runtime::new((0, pan.clone()), view, (96.0, 64.0), Env { scale: 1.0, system_dark: true });
    for _ in 0..2 {
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (96, 64));
    }
    pan.set([50.0, 30.0]);
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (96, 64));
    assert_eq!(rt.instances.len(), 2, "the panned card is elided to its cached quad");
    for (x, y) in [(8.0, 8.0), (58.0, 38.0)] {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (96, 64));
    }
    assert_eq!(rt.state.0, 1, "only the click on the button's panned position presses it");
    let b = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("button")).expect("button listed");
    assert_eq!([b.rect[0], b.rect[1]], [50.0, 30.0], "elements() reports the panned rect");
}

#[test]
fn cache_updates_same_size_image_pixels_after_the_first_raster() {
    fn view(image: &crate::Pixels, _: &mut Cx<crate::Pixels>) -> El {
        el().size(32.0, 32.0).child(el().size(24.0, 24.0).cache().pixels(image))
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let image = crate::Pixels::new();
    let mut rt = Runtime::new(image.clone(), view, (32.0, 32.0), Env::new(true, 1.0));
    for color in [[255, 0, 0, 255], [0, 255, 0, 255]] {
        image.set(2, 2, color.repeat(4));
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
        renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
        let out = pixels(&gpu, &renderer, &target);
        assert_eq!(&out[(12 * 32 + 12) * 4..][..4], &color, "same-size image publication must change the output");
    }
}

#[test]
fn cached_card_stays_elided_during_sixty_live_only_wheel_frames() {
    type S = (u32, Live<[f32; 2]>);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().size(160.0, 100.0)
            .on_wheel(|s: &mut S, e, tx| {
                let p = s.1.get(); s.1.set([p[0] + e.dx, p[1] + e.dy]);
                tx.live_only(); true
            })
            .child(el().size(160.0, 100.0).live_offset(&s.1)
                .child(el().size(32.0, 32.0).cache().fill(Color::WHITE)
                    .child(el().key("button").size(16.0, 16.0)
                        .fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 })
                        .on_press(|s: &mut S| s.0 += 1))))
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let target = Offscreen::new(&gpu.device, 160, 100);
    let mut rt = Runtime::new((0, Live::new([0.0, 0.0])), view, (160.0, 100.0), Env::new(true, 1.0));
    rt.check_live_only(false);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (160, 100));
        pixels(&gpu, &renderer, &target);
    }
    let commits = rt.stats.commits;
    for frame in 0..60 {
        rt.wheel_event(crate::WheelEvent::new(1.0, 0.5).at(120.0, 80.0));
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.stats.commits, commits, "frame {frame} must not rebuild the view");
        assert_eq!(rt.instances.len(), 2, "frame {frame}: child emission stays elided");
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (160, 100));
        let out = pixels(&gpu, &renderer, &target);
        if frame == 59 {
            near(&out[(38 * 160 + 68) * 4..(38 * 160 + 69) * 4], [255, 0, 0, 255]);
            near(&out[(8 * 160 + 8) * 4..(8 * 160 + 9) * 4], [0; 4]);
        }
    }
    let b = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("button")).unwrap();
    assert_eq!([b.rect[0], b.rect[1]], [60.0, 30.0]);
    rt.pointer_down(68.0, 38.0); rt.pointer_up(68.0, 38.0); rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.0, 1, "the click hits the cached child's moved position");
}

#[test]
fn cache_does_not_hide_a_radius_change_during_scale_settling() {
    type S = (bool, Live<f32>);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        el().size(100.0, 100.0).live_scale(&s.1).child(
            el().size(32.0, 32.0).cache().child(
                el().size(32.0, 32.0).motion(crate::tok::motion::INSTANT).fill(Color::WHITE).radius(if s.0 { 16.0 } else { 0.0 }),
            ),
        )
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 100, 100);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let scale = Live::new(1.0);
    let mut rt = Runtime::new((false, scale.clone()), view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (100, 100));
    let _ = pixels(&gpu, &renderer, &target);
    scale.set(1.01);
    rt.state.0 = true;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (100, 100));
    let out = pixels(&gpu, &renderer, &target);
    assert!(out[3] < 32, "the newly rounded corner must not show the stale square (alpha={})", out[3]);
}

#[test]
fn slow_pinch_composites_until_the_scale_holds_still() {
    fn view(scale: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(200.0, 200.0).padding(60.0).live_scale(scale).child(
            el().size(48.0, 48.0).cache().fill(Color { r: 0.2, g: 0.5, b: 0.9, a: 1.0 })
                .child(el().size(24.0, 24.0).fill(Color::WHITE)),
        )
    }
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let scale = Live::new(1.0);
    let mut rt = Runtime::new(scale.clone(), view, (200.0, 200.0), Env { scale: 1.0, system_dark: true });
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 1);
    for step in 1..=60 {
        scale.set(1.0 + step as f32 * 0.01);
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
        assert_eq!(renderer.layer_stats().extra_passes, 0, "pinch step {step} composites the old texture");
    }
    rt.frame(0.15);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "one raster once the pinch rests");
}

#[test]
fn over_budget_cache_falls_back_to_a_viewport_sized_target() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(32.0, 32.0).child(
            el().size(300.0, 300.0).cache().clip_rounded(8.0).fill(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 })
                .child(el().size(20.0, 20.0).fill(Color::WHITE)),
        )
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 32, 32);
    let mut want = None;
    // In budget it caches a 320 px bucket; the fallback draws the same group through the 32 px it shows.
    for budget in [DEFAULT_CACHE_BUDGET, 64 * 64 * 8] {
        let mut rt = Runtime::new((), view, (32.0, 32.0), Env { scale: 1.0, system_dark: true });
        let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
        renderer.set_cache_budget(budget);
        let mut out = Vec::new();
        for _ in 0..3 {
            rt.frame(1.0 / 60.0);
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (32, 32));
            out = pixels(&gpu, &renderer, &target);
        }
        match &want {
            None => want = Some(out),
            Some(w) => {
                assert!(renderer.layer_stats().bytes <= 64 * 64 * 8, "over-budget fallback stays viewport-sized: {:?}", renderer.layer_stats());
                assert!(out.iter().zip(w).all(|(a, b)| a.abs_diff(*b) <= 1), "fallback draws like the cache");
            }
        }
    }
}

#[test]
fn scale_commits_draw_live_without_repeated_cache_passes() {
    fn view(scale: &f32, _: &mut Cx<f32>) -> El {
        el().size(200.0, 200.0).scale(*scale).scale_motion(rune_motion::Curve::Instant).child(
            el().size(48.0, 48.0).cache().fill(Color::WHITE),
        )
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 200, 200);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new(1.0, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    let _ = pixels(&gpu, &renderer, &target);
    for step in 1..=30 {
        rt.state = 1.0 + step as f32 * 0.01;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
        assert_eq!(renderer.layer_stats().extra_passes, 0, "scale commit {step} must not rasterize another cache");
        let _ = pixels(&gpu, &renderer, &target);
    }
    rt.frame(0.15);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 200));
    assert_eq!(renderer.layer_stats().extra_passes, 1, "cache once the committed scale settles");
}

#[test]
fn glyph_atlas_survives_four_hundred_scales_and_renders_fresh_and_cached_text() {
    struct State {
        font_size: f32,
        banner: &'static str,
        card_text: &'static str,
    }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        crate::col()
            .size(360.0, 120.0)
            .gap(8.0)
            .child(
                crate::text(s.banner)
                    .font(crate::TextStyle::new(s.font_size, 400.0))
                    .color(Color::WHITE),
            )
            .child(
                el().size(320.0, 40.0)
                    .cache()
                    .child(crate::text(s.card_text).font(crate::TextStyle::new(16.0, 500.0)).color(Color::WHITE)),
            )
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 360, 120);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new(
        State {
            font_size: 16.0,
            banner: "The quick brown fox jumps over the lazy dog 0123456789",
            card_text: "CACHED SUBTREE ALPHA",
        },
        view,
        (360.0, 120.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (360, 120));
    let _ = pixels(&gpu, &renderer, &target);

    for i in 0..400 {
        rt.state.font_size = 18.0 + (i as f32) * 0.45;
        rt.invalidate();
        rt.frame(1.0 / 60.0);
    }
    assert!(rt.text.atlas.resets() >= 2, "400 scales must cycle the glyph atlas (resets={})", rt.text.atlas.resets());

    // Now draw a fresh string at scale 1 (font_size 18.0) and a fresh cached card string.
    rt.state.font_size = 18.0;
    rt.state.banner = "Sphinx of black quartz, judge my vow!";
    rt.state.card_text = "CACHED SUBTREE OMEGA";
    rt.invalidate();
    rt.frame(1.0);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (360, 120));
    let out = pixels(&gpu, &renderer, &target);

    let glyphs: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).collect();
    let expected = rt.state.banner.chars().filter(|c| !c.is_whitespace()).count()
        + rt.state.card_text.chars().filter(|c| !c.is_whitespace()).count();
    assert_eq!(glyphs.len(), expected, "every glyph in both fresh and cached text must be emitted");
    for (idx, g) in glyphs.iter().enumerate() {
        let x0 = (g.rect[0].floor().max(0.0) as u32).min(360);
        let y0 = (g.rect[1].floor().max(0.0) as u32).min(120);
        let x1 = ((g.rect[0] + g.rect[2]).ceil().max(0.0) as u32).min(360);
        let y1 = ((g.rect[1] + g.rect[3]).ceil().max(0.0) as u32).min(120);
        let mut lit = 0u32;
        for y in y0..y1 {
            for x in x0..x1 {
                let p = &out[((y * 360 + x) * 4) as usize..][..4];
                if p[0] > 16 || p[1] > 16 || p[2] > 16 {
                    lit += 1;
                }
            }
        }
        assert!(lit > 0, "glyph {idx} at {:?} (page {}) rendered zero ink pixels", g.rect, g.params[2]);
    }
}

#[test]
fn single_frame_spanning_multiple_glyph_atlas_pages_draws_all_pages() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let mut root = el().size(512.0, 512.0);
        // 24 overlapping lines of large text inside the viewport overflow page 0 into page 1+.
        for i in 0..24 {
            root = root.child(
                crate::text("ABCDEFGHIJKLMN")
                    .absolute(4.0, (i * 8) as f32)
                    .font(crate::TextStyle::new(150.0 + i as f32 * 2.0, 700.0))
                    .color(Color::WHITE),
            );
        }
        root
    }
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 512, 512);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new((), view, (512.0, 512.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert!(rt.text.atlas.page_count() >= 2, "single frame must allocate >= 2 pages (got {})", rt.text.atlas.page_count());
    assert_eq!(rt.text.atlas.resets(), 0, "single frame must not reset a live page mid-frame");
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_GLYPH && i.params[2] == 0.0), "has page 0 glyphs");
    assert!(rt.instances.iter().any(|i| i.params[1] == KIND_GLYPH && i.params[2] >= 1.0), "has page 1+ glyphs");

    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (512, 512));
    let out = pixels(&gpu, &renderer, &target);
    for page in [0.0f32, 1.0f32] {
        let g = rt
            .instances
            .iter()
            .find(|i| i.params[1] == KIND_GLYPH && i.params[2] == page && i.rect[0] >= 0.0 && i.rect[1] >= 0.0 && i.rect[0] + i.rect[2] <= 512.0 && i.rect[1] + i.rect[3] <= 512.0)
            .unwrap_or_else(|| panic!("expected in-viewport glyph on page {page}"));
        let (x0, y0) = (g.rect[0].floor() as u32, g.rect[1].floor() as u32);
        let (x1, y1) = ((g.rect[0] + g.rect[2]).ceil() as u32, (g.rect[1] + g.rect[3]).ceil() as u32);
        let mut lit = 0u32;
        for y in y0..y1 {
            for x in x0..x1 {
                if out[((y * 512 + x) * 4) as usize] > 16 {
                    lit += 1;
                }
            }
        }
        assert!(lit > 0, "glyph on page {page} at {:?} must render visible pixels", g.rect);
    }
}

#[test]
fn live_scale_zoom_sweep_buckets_during_motion_and_settles_crisp() {
    fn view(zoom: &Live<f32>, _: &mut Cx<Live<f32>>) -> El {
        el().size(300.0, 120.0)
            .live_scale(zoom)
            .child(crate::text("Hamburgefonstiv").font(crate::TextStyle::new(14.0, 400.0)).color(Color::WHITE))
    }
    let zoom = Live::new(1.0);
    let mut rt = Runtime::new(zoom.clone(), view, (300.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let per_size = rt.text.atlas.rasterized();
    assert!(per_size >= 10);

    // 10 s zoom sweep at 120 Hz (1,200 frames) across [0.25, 4.0] (4 octaves = 17 quarter-octave buckets).
    for frame in 1..=1200 {
        let u = frame as f32 / 1200.0;
        let k = 0.25 * 16.0f32.powf(u);
        zoom.set(k);
        rt.frame(1.0 / 120.0);
    }
    let sweep_rasterized = rt.text.atlas.rasterized();
    let distinct_sizes = sweep_rasterized / per_size;
    assert!(
        distinct_sizes <= 18,
        "10s zoom sweep across [0.25, 4.0] must rasterize at most 17 bucketed sizes (got {distinct_sizes}, total={sweep_rasterized})"
    );

    // Stop at an off-bucket scale (1.13) and let it settle: must re-rasterize crisp at 1.13 and stop animating.
    zoom.set(1.13);
    rt.frame(1.0 / 120.0);
    assert!(rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).any(|q| q.uv[2] > 0.0), "filtered while moving");
    for _ in 0..20 {
        rt.frame(1.0 / 120.0);
    }
    assert!(
        rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).all(|q| q.uv[2] == 0.0),
        "crisp 1:1 glyphs once live_scale settles at 1.13"
    );
    assert!(!rt.frame(1.0 / 120.0), "nothing keeps animating after live_scale settles");
}

#[test]
fn pill_styles_follow_captured_state_through_a_cached_subtree() {
    use std::cell::Cell;
    use std::rc::Rc;
    use crate::{PillStyle, Span, TextEdit, TextStyle};
    static A: crate::Icon = crate::Icon { name: "a", view: 8.0, body: "<rect width=\"8\" height=\"8\"/>" };
    static B: crate::Icon = crate::Icon { name: "b", view: 8.0, body: "<circle cx=\"4\" cy=\"4\" r=\"4\"/>" };
    type S = (TextEdit, bool, Rc<Cell<usize>>);
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let swap = s.1;
        let calls = s.2.clone();
        let field = crate::text_input(&s.0, TextStyle::new(14.0, 400.0))
            .size(200.0, 30.0)
            .pill_icon(move |_| Some(&B))
            .pill_style(move |id| {
                calls.set(calls.get() + 1);
                let first = (id == "p:1") != swap;
                if first {
                    PillStyle::new().bg(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 }).icon(&A)
                } else {
                    // Leave icon unset so it falls back to `pill_icon` (&B).
                    PillStyle::default().bg(Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 })
                }
            })
            .pill_colors(Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }, Some(Color::WHITE))
            .color(Color::WHITE)
            .decorations([Span::new(0, 1)]);
        el().size(200.0, 40.0).child(el().size(190.0, 30.0).child(field).cache())
    }
    let mut edit = TextEdit::new(" ").rich();
    edit.set_caret(0);
    edit.insert_pill("p:1", "one");
    edit.insert("  ");
    edit.insert_pill("p:2", "two");
    let calls = Rc::new(Cell::new(0));
    let mut rt = Runtime::new((edit, false, calls.clone()), view, (200.0, 40.0), Env { scale: 1.0, system_dark: true });
    let gpu = Gpu::headless();
    let target = Offscreen::new(&gpu.device, 200, 40);
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut step = |rt: &mut Runtime<S>| {
        rt.frame(1.0 / 60.0);
        renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (200, 40));
        (pixels(&gpu, &renderer, &target), renderer.layer_stats().extra_passes)
    };
    let chips = |out: &[u8]| {
        let first = |hit: &dyn Fn(&[u8]) -> bool| (0..200).find(|&x| (0..40).any(|y| hit(&out[(y * 200 + x) * 4..][..4])));
        let (red, blue) = (first(&|p| p[0] > 120 && p[2] < 60), first(&|p| p[2] > 120 && p[0] < 60));
        (red.expect("red chip"), blue.expect("blue chip"))
    };
    let (out, passes) = step(&mut rt);
    assert_eq!(calls.get(), 2, "pill_style runs once per pill on build despite later builder calls");
    assert_eq!(passes, 1, "initial frame rasterizes the cached subtree");
    let (red, blue) = chips(&out);
    assert!(red < blue, "p:1 draws red then p:2 blue: {red} {blue}");
    assert_eq!(step(&mut rt).1, 0, "steady frame hits the cached layer");
    rt.invalidate();
    assert_eq!(step(&mut rt).1, 0, "rebuilding with unchanged pill styles keeps the cached layer");
    let icons = |rt: &Runtime<S>| rt.nodes.iter().find_map(|n| n.text.as_ref().filter(|t| !t.pills.is_empty())).unwrap().pills.iter().map(|p| p.icon.unwrap().name).collect::<Vec<_>>();
    assert_eq!(icons(&rt), ["a", "b"]);
    calls.set(0);
    rt.state.1 = true;
    rt.invalidate();
    let (out, passes) = step(&mut rt);
    assert_eq!(calls.get(), 2, "pill_style runs once per pill when rebuilt");
    assert_eq!(passes, 1, "changed pill styles invalidate the cached layer");
    let (red, blue) = chips(&out);
    assert!(blue < red, "after the captured state flips the cached chips restyle: {red} {blue}");
    assert_eq!(icons(&rt), ["b", "a"]);
    assert_eq!(step(&mut rt).1, 0, "next frame hits the newly cached layer");
}
