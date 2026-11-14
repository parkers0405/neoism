//! What a compositor's hook learns about each effect slot: its `El::effect` tag
//! (`EffectSite::mark`), its `El::effect_data` payload (`EffectSite::data`) frozen with the
//! element through its exit, and where the presented element is, transforms included.
use rune::render::{EffectSite, Gpu, Instance, KIND_LAYER, KIND_LAYER_END, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Runtime, Transition, effect, el, memo};

#[derive(Clone, Copy, Debug)]
struct Seen {
    mark: u32,
    data: [f32; 4],
    rect: [f32; 4],
    scale: f32,
    in_layer: bool,
    /// Product of the opacities of the isolated layers open around the slot.
    opacity: f32,
    /// Turn of the innermost turned layer around the slot, in radians.
    turn: f32,
}

/// Draws the frame through `Renderer::draw_with` and records every slot the hook sees.
fn draw<S: 'static>(gpu: &Gpu, renderer: &mut Renderer, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<Seen> {
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let instances: &[Instance] = &rt.instances;
    let mut seen = Vec::new();
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw_with(&mut encoder, &target.view, [0.0; 4], &mut |_, s: &EffectSite<'_>| {
        let mut open = Vec::new();
        for i in &instances[..s.index as usize] {
            if i.params[1] == KIND_LAYER { open.push(*i); }
            if i.params[1] == KIND_LAYER_END { open.pop(); }
        }
        seen.push(Seen {
            mark: s.mark,
            data: s.data,
            rect: s.instance.rect,
            scale: s.scale,
            in_layer: s.in_layer,
            opacity: open.iter().map(|l| l.fill[3]).product(),
            turn: open.iter().rev().map(|l| l.border[1]).find(|t| *t != 0.0).unwrap_or(0.0),
        });
    });
    gpu.queue.submit([encoder.finish()]);
    seen
}

fn bits(data: [f32; 4]) -> [u32; 4] {
    data.map(f32::to_bits)
}

fn one(seen: &[Seen], mark: u32) -> Seen {
    let hits: Vec<_> = seen.iter().filter(|s| s.mark == mark).collect();
    assert_eq!(hits.len(), 1, "mark {mark} in {seen:?}");
    *hits[0]
}

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("missing {key:?}"))
}

fn near(a: [f32; 4], b: [f32; 4], what: &str) {
    assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.01), "{what}: {a:?} vs {b:?}");
}

/// Within a device pixel: slot rects snap to the pixel grid like the rest of the paint.
fn snapped(a: [f32; 4], b: [f32; 4], what: &str) {
    assert!(a.iter().zip(b).all(|(a, b)| (a - b).abs() <= 1.0), "{what}: {a:?} vs {b:?}");
}

const ID: f32 = f32::from_bits(0xDEAD_BEEF);
const NAN: f32 = f32::from_bits(0x7FC0_1234);

struct Pair { version: u32, other: u32 }

fn pair_slots(version: u32) -> El {
    el().size(96.0, 40.0)
        .child(effect(7).absolute(8.0, 8.0).size(16.0, 16.0).effect_data([1.0, 2.0, 3.0, version as f32]))
        .child(effect(70_000).absolute(40.0, 8.0).size(16.0, 16.0).effect_data([ID, -0.5, NAN, 0.0]))
}

fn pair(s: &Pair, _: &mut Cx<Pair>) -> El {
    let v = s.version;
    el().size(96.0, 96.0)
        .child(el().key("plain").child(pair_slots(0)))
        .child(el().key("cached").absolute(0.0, 48.0).cache().child(memo("slots", v, move || pair_slots(v))))
        .child(el().key("tick").size(s.other as f32, 1.0))
}

#[test]
fn sibling_slots_each_hand_the_hook_their_own_mark_and_payload() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new(Pair { version: 1, other: 1 }, pair, (96.0, 96.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let check = |seen: &[Seen], version: f32| {
        let marks: Vec<u32> = seen.iter().map(|s| s.mark).collect();
        assert_eq!(marks, [7, 70_000, 7, 70_000], "painter order, in and out of a memo under .cache()");
        for (k, s) in seen.iter().enumerate() {
            let want = if s.mark == 7 { [1.0, 2.0, 3.0, if k < 2 { 0.0 } else { version }] } else { [ID, -0.5, NAN, 0.0] };
            assert_eq!(bits(s.data), bits(want), "slot {k} payload, bit for bit");
            assert!(!s.in_layer && s.opacity == 1.0, "slot {k}: {s:?}");
        }
    };
    check(&draw(&gpu, &mut renderer, &mut rt, (96, 96)), 1.0);
    // A rebuild that reuses the memoized slots keeps their payload.
    rt.update(|s, _| s.other = 2);
    rt.frame(1.0 / 60.0);
    check(&draw(&gpu, &mut renderer, &mut rt, (96, 96)), 1.0);
    // A new version rebuilds them with the new payload.
    rt.update(|s, _| s.version = 5);
    rt.frame(1.0 / 60.0);
    check(&draw(&gpu, &mut renderer, &mut rt, (96, 96)), 5.0);
}

struct Pages { page: u32, phase: f32 }

fn pages(s: &Pages, _: &mut Cx<Pages>) -> El {
    let x = if s.page % 2 == 0 { 48.0 } else { 8.0 };
    el().size(96.0, 96.0).child(
        effect(100 + s.page).key(format!("hud{}", s.page)).absolute(x, 8.0).size(32.0, 32.0)
            .effect_data([s.page as f32, s.phase, 0.0, 0.0]).transition(Transition::opacity()),
    )
}

#[test]
fn an_exiting_slot_keeps_its_mark_and_payload_while_its_replacement_enters() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new(Pages { page: 1, phase: 0.25 }, pages, (96.0, 96.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let seen = draw(&gpu, &mut renderer, &mut rt, (96, 96));
    assert_eq!(seen.len(), 1);
    assert_eq!((seen[0].mark, seen[0].data, seen[0].in_layer), (101, [1.0, 0.25, 0.0, 0.0], false));

    rt.update(|s, _| {
        s.page = 2;
        s.phase = 0.75;
    });
    let mut both = 0;
    for frame in 0..60 {
        rt.frame(1.0 / 60.0);
        let seen = draw(&gpu, &mut renderer, &mut rt, (96, 96));
        let new = one(&seen, 102);
        assert_eq!(new.data, [2.0, 0.75, 0.0, 0.0], "frame {frame}: the new slot's payload");
        let Some(old) = seen.iter().find(|s| s.mark == 101) else { continue };
        both += 1;
        assert_eq!(old.data, [1.0, 0.25, 0.0, 0.0], "frame {frame}: the exiting slot keeps the payload it left with");
        assert!(find(&rt, "hud1").exiting, "frame {frame}");
        assert!(old.in_layer && old.opacity < 1.0, "frame {frame}: the exit fade applies through the slot's layer: {old:?}");
        assert!((old.opacity - find(&rt, "hud1").opacity).abs() < 1e-4, "frame {frame}: at the presented opacity");
        assert_eq!(old.rect, [8.0, 8.0, 32.0, 32.0], "frame {frame}: where it was");
        assert_eq!(new.rect, [48.0, 8.0, 32.0, 32.0], "frame {frame}: beside it");
        if frame == 0 {
            assert!(new.in_layer && new.opacity < 1.0, "the new slot fades in: {new:?}");
        }
    }
    assert!(both >= 3, "the exit played over {both} frames");
    let seen = draw(&gpu, &mut renderer, &mut rt, (96, 96));
    assert_eq!(seen.len(), 1, "the exited slot is gone: {seen:?}");
    assert!(!seen[0].in_layer && seen[0].opacity == 1.0);
}

struct Moves { extra: bool }

fn moves(s: &Moves, _: &mut Cx<Moves>) -> El {
    let mut root = el().size(160.0, 160.0)
        .child(effect(1).key("scaled").absolute(8.0, 8.0).size(16.0, 16.0).scale(1.5))
        .child(effect(2).key("offset").absolute(40.0, 8.0).size(16.0, 16.0).offset(5.0, 3.0))
        .child(effect(3).key("rotated").absolute(8.0, 40.0).size(16.0, 16.0).rotate(0.5))
        .child(el().key("parent").absolute(40.0, 40.0).size(32.0, 32.0).scale(2.0)
            .child(effect(4).key("child").absolute(8.0, 8.0).size(8.0, 8.0)));
    if s.extra {
        root = root.child(effect(5).key("moving").absolute(96.0, 96.0).size(32.0, 32.0)
            .transition(Transition::scale(0.5).combined(Transition::offset(0.0, 24.0))));
    }
    root
}

/// The slot's rect and `scale` are the presented element's, in device pixels.
fn presented(seen: &Seen, e: &ElementInfo, k: f32, what: &str) {
    snapped(seen.rect, e.rect.map(|v| v * k), what);
    assert!((seen.scale - e.scale * k).abs() < 1e-4, "{what}: scale {} vs {}", seen.scale, e.scale * k);
}

#[test]
fn slots_follow_scale_offset_and_transitions_and_turn_in_a_layer() {
    let k = 2.0;
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut rt = Runtime::new(Moves { extra: false }, moves, (160.0, 160.0), Env::new(true, k));
    let size = (320, 320);
    rt.frame(0.0);
    let seen = draw(&gpu, &mut renderer, &mut rt, size);

    let scaled = one(&seen, 1);
    near(scaled.rect, [4.0 * k, 4.0 * k, 24.0 * k, 24.0 * k], "El::scale grows the rect about its centre");
    assert_eq!(scaled.scale, 1.5 * k);
    presented(&scaled, &find(&rt, "scaled"), k, "scaled");
    let offset = one(&seen, 2);
    near(offset.rect, [45.0 * k, 11.0 * k, 16.0 * k, 16.0 * k], "El::offset moves the rect");
    assert_eq!(offset.scale, k);
    let child = one(&seen, 4);
    assert_eq!(child.scale, 2.0 * k, "an ancestor's scale reaches the slot");
    presented(&child, &find(&rt, "child"), k, "child of a scaled parent");

    // Rotation isn't in the slot's record: the slot stays upright inside a turned layer.
    let rotated = one(&seen, 3);
    near(rotated.rect, [8.0 * k, 40.0 * k, 16.0 * k, 16.0 * k], "upright rect");
    assert_eq!(rotated.scale, k);
    assert!(rotated.in_layer && (rotated.turn - 0.5).abs() < 1e-4, "{rotated:?}");

    rt.update(|s, _| s.extra = true);
    let mut entering = 0;
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
        let seen = draw(&gpu, &mut renderer, &mut rt, size);
        let slot = one(&seen, 5);
        let e = find(&rt, "moving");
        presented(&slot, &e, k, "entering");
        if e.scale < 0.999 {
            entering += 1;
            assert!(slot.rect[1] > 96.0 * k, "slides in from below: {slot:?}");
            assert!(slot.in_layer && (slot.opacity - e.opacity).abs() < 1e-4, "fades in through its layer: {slot:?}");
        }
    }
    assert!(entering >= 3, "the entrance played over {entering} frames");

    rt.update(|s, _| s.extra = false);
    let mut exiting = 0;
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
        let seen = draw(&gpu, &mut renderer, &mut rt, size);
        let Some(slot) = seen.iter().find(|s| s.mark == 5) else { continue };
        let e = find(&rt, "moving");
        assert!(e.exiting);
        presented(slot, &e, k, "exiting");
        if e.scale < 0.999 { exiting += 1; }
    }
    assert!(exiting >= 3, "the exit played over {exiting} frames");
}
