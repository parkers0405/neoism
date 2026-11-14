use super::*;


#[test]
fn text_inside_a_scale_draws_at_that_size() {
    #[derive(Clone, Copy, PartialEq)]
    enum How { Static, Live, Press }
    struct State { k: f32, how: How, pressed: u32 }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let label = crate::el::text("Hamburgefonstiv").key("label");
        let card = el().key("card").size(200.0, 80.0).on_press(|s: &mut State| s.pressed += 1).child(label);
        let card = match s.how {
            How::Static => card.scale(s.k),
            How::Live => card.live_scale(&crate::live::Live::new(s.k)),
            How::Press => card.press_scale(s.k),
        };
        col().size(300.0, 200.0).child(card)
    }
    // Device px: the glyphs' bounding box, and whether all of them sample their atlas 1:1.
    fn glyphs(rt: &Runtime<State>) -> ([f32; 4], bool) {
        let gs: Vec<_> = rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).collect();
        assert!(gs.len() >= 10, "the label draws its glyphs");
        let b = gs.iter().fold([f32::MAX, f32::MAX, f32::MIN, f32::MIN], |b, q| {
            [b[0].min(q.rect[0]), b[1].min(q.rect[1]), b[2].max(q.rect[0] + q.rect[2]), b[3].max(q.rect[1] + q.rect[3])]
        });
        (b, gs.iter().all(|q| q.uv[2] == 0.0))
    }
    let run = |k: f32, how: How| {
        let mut rt = Runtime::new(State { k, how, pressed: 0 }, view, (300.0, 200.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 120.0);
        rt
    };
    let (rest, _) = glyphs(&run(1.0, How::Static));
    let (w, h) = (rest[2] - rest[0], rest[3] - rest[1]);
    // The card's centre in device px, which the scale is about.
    let c = [200.0, 80.0];
    for how in [How::Static, How::Live] {
        let rt = run(0.5, how);
        let (half, crisp) = glyphs(&rt);
        let (hw, hh) = (half[2] - half[0], half[3] - half[1]);
        assert!((hw / w - 0.5).abs() < 0.03 && (hh / h - 0.5).abs() < 0.06, "glyphs draw at half size: {hw}x{hh} vs {w}x{h}");
        for axis in 0..2 {
            let want = c[axis] + (rest[axis] - c[axis]) * 0.5;
            assert!((half[axis] - want).abs() < 1.5, "about the card's centre: {} vs {want}", half[axis]);
        }
        assert!(crisp, "a still scale rasterizes at the scaled size");
    }

    // Static scale needs no layout change, and hits follow the drawn rect: 50..150 x 20..60.
    let mut rt = run(0.5, How::Static);
    let card = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("card")).unwrap();
    assert_eq!(card.target, [0.0, 0.0, 200.0, 80.0], "layout keeps the unscaled size");
    assert!((card.rect[0] - 50.0).abs() < 0.05 && (card.rect[2] - 100.0).abs() < 0.05, "drawn about its centre: {:?}", card.rect);
    for (x, hit) in [(20.0, 0), (100.0, 1)] {
        rt.pointer_move(x, 40.0);
        rt.pointer_down(x, 40.0);
        rt.pointer_up(x, 40.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.pressed, hit, "a press at x {x}");
    }

    // A press scale's text shrinks too; while it springs back, it samples rest-size glyphs.
    let mut rt = run(0.9, How::Press);
    rt.pointer_move(100.0, 40.0);
    rt.pointer_down(100.0, 40.0);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    let (held, crisp) = glyphs(&rt);
    assert!(((held[2] - held[0]) / w - 0.9).abs() < 0.02, "held text is 90%: {held:?}");
    assert!(crisp, "the held size rasterizes crisp");
    rt.pointer_up(100.0, 40.0);
    rt.frame(1.0 / 120.0);
    let (moving, crisp) = glyphs(&rt);
    let mw = (moving[2] - moving[0]) / w;
    assert!(mw > 0.9 && mw < 1.0, "springing back: {mw}");
    assert!(!crisp, "a moving scale samples its rest-size glyphs");
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert_eq!(glyphs(&rt), (rest, true), "back at rest exactly as before");
}

#[test]
fn nested_scales_compose_about_each_mapped_centre() {
    #[derive(Clone, Copy, PartialEq, Debug)]
    enum How { Press, Static, ProductOfOne, ThreeLevels }
    struct State { how: How, pressed: u32 }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let leaf = el().key("c").size(40.0, 40.0).on_press(|s: &mut State| s.pressed += 1);
        let inner = match s.how {
            How::Press => leaf.press_scale(0.96),
            How::Static => leaf.scale(0.8),
            How::ProductOfOne => leaf.scale(2.0),
            How::ThreeLevels => el().key("mid").size(100.0, 100.0).items_start()
                .live_scale(&crate::live::Live::new(0.8)).child(leaf.press_scale(0.9)),
        };
        col().size(400.0, 400.0).items_start()
            .child(el().key("p").size(200.0, 200.0).items_start().scale(0.5).child(inner))
    }
    let edges = |rt: &Runtime<State>| {
        let r = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("c")).unwrap().rect;
        [r[0], r[1], r[0] + r[2], r[1] + r[3]]
    };
    let near = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(a, b)| (a - b).abs() < 0.3);
    let run = |how| {
        let mut rt = Runtime::new(State { how, pressed: 0 }, view, (400.0, 400.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.frame(1.0 / 120.0);
        rt
    };
    let press = |rt: &mut Runtime<State>, x: f32, y: f32| {
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 120.0);
    };
    // The parent draws [50, 50, 150, 150]; the child's centre (20, 20) lands at (60, 60).
    for (how, rest, held) in [
        (How::Press, [50.0, 50.0, 70.0, 70.0], Some([50.4, 50.4, 69.6, 69.6])),
        (How::Static, [52.0, 52.0, 68.0, 68.0], None),
        // 0.5 × 2.0 is 1, but the child still moves with its parent.
        (How::ProductOfOne, [40.0, 40.0, 80.0, 80.0], None),
        // The middle level draws [50, 50, 90, 90] at 0.4, putting the child's centre at 63.
        (How::ThreeLevels, [55.0, 55.0, 71.0, 71.0], Some([55.8, 55.8, 70.2, 70.2])),
    ] {
        let mut rt = run(how);
        assert!(near(edges(&rt), rest), "{how:?} at rest: {:?} vs {rest:?}", edges(&rt));
        press(&mut rt, rest[0] - 1.0, rest[1] - 1.0);
        press(&mut rt, rest[0] + 1.0, rest[1] + 1.0);
        assert_eq!(rt.state.pressed, 1, "{how:?}: hits follow the drawn rect");
        let Some(held) = held else { continue };
        let (cx, cy) = ((rest[0] + rest[2]) * 0.5, (rest[1] + rest[3]) * 0.5);
        rt.pointer_move(cx, cy);
        rt.pointer_down(cx, cy);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
        assert!(near(edges(&rt), held), "{how:?} held: {:?} vs {held:?}", edges(&rt));
        // Still hit at its resting size: a release just inside its resting corner presses.
        rt.pointer_up(rest[2] - 0.2, rest[3] - 0.2);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.pressed, 2, "{how:?}: a held press hits at the resting rect");
    }
}

#[test]
fn a_changing_static_scale_keeps_the_glyph_atlas_bounded() {
    struct State { k: f32 }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(300.0, 200.0)
            .child(el().key("card").size(200.0, 80.0).scale(s.k).scale_motion(rune_motion::Curve::Instant)
                .child(crate::el::text("Hamburgefonstiv")))
    }
    let crisp = |rt: &Runtime<State>| rt.instances.iter().filter(|q| q.params[1] == KIND_GLYPH).all(|q| q.uv[2] == 0.0);
    let mut rt = Runtime::new(State { k: 1.0 }, view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let rest = rt.text.atlas.glyph_entries();
    // A slider or pinch: a new scale every frame, holding still now and then.
    for i in 0..1000 {
        rt.state.k = 0.5 + i as f32 * 0.0007;
        rt.invalidate();
        rt.frame(1.0 / 120.0);
        assert!(i == 0 || !crisp(&rt), "a scale changing every frame samples rest-size glyphs");
        if i % 100 == 99 { for _ in 0..30 { rt.frame(1.0 / 120.0); } }
    }
    let grown = rt.text.atlas.glyph_entries() - rest;
    assert!(grown < 1000, "1,000 scales added {grown} glyph entries (one size each would add 15,000)");
    rt.state.k = 0.75;
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert!(!crisp(&rt));
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert!(crisp(&rt), "once it holds still, text rasterizes at the scale");
    assert!(!rt.frame(1.0 / 120.0), "and nothing keeps animating");
}

#[test]
fn a_held_press_scale_keeps_its_hover_and_a_scroll_cancels_it() {
    struct State { pressed: Vec<usize> }
    fn view(_: &State, _: &mut Cx<State>) -> El {
        let mut list = col().key("list").w_full();
        for i in 0..10 {
            list = list.child(el().key(format!("b{i}")).size(200.0, 40.0).hover(crate::color::oklcha(1.0, 0.0, 0.0, 0.1))
                .press_scale(0.96).on_press(move |s: &mut State| s.pressed.push(i)));
        }
        col().size(200.0, 200.0).child(col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).child(list))
    }
    let mut rt = Runtime::new(State { pressed: Vec::new() }, view, (200.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("b0")).unwrap().id;
    // Held at its right edge, which the shrunk row no longer covers: still hovered.
    rt.pointer_move(199.0, 20.0);
    rt.pointer_down(199.0, 20.0);
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.scene.get(id, P_PRESS_SCALE), Some(0.96));
    assert_eq!(rt.scene.get(id, P_HOVER), Some(1.0), "the held row keeps its highlight");
    rt.pointer_up(199.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, [0]);

    // Scrolling under a held press cancels it: the row lets go, and the release presses nothing.
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    rt.pointer_move(100.0, 20.0);
    rt.pointer_down(100.0, 20.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    assert!(rt.scene.get(id, P_PRESS_SCALE).unwrap() < 0.97, "held");
    rt.wheel(100.0, 100.0, -120.0);
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    assert!((rt.scene.get(id, P_PRESS_SCALE).unwrap() - 1.0).abs() < 1e-3, "the scrolled-away row springs back");
    rt.pointer_up(100.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, [0], "a scroll cancels the press");
    // A fling does too.
    rt.pointer_move(100.0, 20.0);
    rt.pointer_down(100.0, 20.0);
    rt.fling(100.0, 100.0, 600.0);
    rt.pointer_up(100.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, [0]);
}

#[test]
fn press_scale_shrinks_while_held_and_springs_back() {
    struct State { pressed: u32, reduced: bool }
    fn view(s: &State, cx: &mut Cx<State>) -> El {
        cx.reduced_motion(s.reduced);
        col().size(200.0, 200.0).child(el().key("tile").size(100.0, 100.0).press_scale(0.96)
            .on_press(|s: &mut State| s.pressed += 1)
            .child(el().key("dot").size(10.0, 10.0)))
    }
    fn info(rt: &Runtime<State>, key: &str) -> ElementInfo {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
    }
    let settle = |rt: &mut Runtime<State>| for _ in 0..90 { rt.frame(1.0 / 120.0); };
    let mut rt = Runtime::new(State { pressed: 0, reduced: false }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let rest = info(&rt, "tile");
    assert_eq!(rest.scale, 1.0);
    // Near the right edge, which the shrunken tile no longer covers.
    rt.pointer_move(99.0, 50.0);
    rt.pointer_down(99.0, 50.0);
    settle(&mut rt);
    let held = info(&rt, "tile");
    assert!((held.scale - 0.96).abs() < 1e-4, "held scale {}", held.scale);
    assert_eq!(held.target, rest.target, "layout keeps the resting size");
    assert!((held.rect[0] - 2.0).abs() < 0.05 && (held.rect[2] - 96.0).abs() < 0.05, "shrinks about its centre: {:?}", held.rect);
    assert!((info(&rt, "dot").scale - 0.96).abs() < 1e-4, "children shrink with it");
    assert!(rt.press.inside(), "a press near the edge stays inside the resting rect");
    rt.pointer_move(150.0, 50.0);
    settle(&mut rt);
    assert!((info(&rt, "tile").scale - 1.0).abs() < 1e-4, "leaving springs back");
    rt.pointer_move(99.0, 50.0);
    rt.frame(1.0 / 120.0);
    let returning = info(&rt, "tile").scale;
    assert!(returning < 1.0 && returning > 0.96, "re-entering shrinks again, from where it was: {returning}");
    settle(&mut rt);
    rt.pointer_up(99.0, 50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.pressed, 1);
    let mut last = info(&rt, "tile").scale;
    assert!(last > 0.96 && last < 1.0, "the release springs, not jumps: {last}");
    let mut peak: f32 = last;
    for _ in 0..90 {
        rt.frame(1.0 / 120.0);
        let s = info(&rt, "tile").scale;
        assert!((s - last).abs() < 0.01, "no frame jumps more than 1%: {last} -> {s}");
        peak = peak.max(s);
        last = s;
    }
    assert!((last - 1.0).abs() < 1e-4, "settles at rest: {last}");
    assert!(peak > 1.0, "the release has a little bounce: {peak}");

    let mut reduced = Runtime::new(State { pressed: 0, reduced: true }, view, (200.0, 200.0), Env::new(true, 1.0));
    reduced.frame(0.0);
    reduced.pointer_move(50.0, 50.0);
    reduced.pointer_down(50.0, 50.0);
    settle(&mut reduced);
    assert_eq!(info(&reduced, "tile").scale, 1.0, "Reduce Motion turns press scale off");
}

/// A picture with one side set takes the other from its image, as SwiftUI's
/// `.resizable().aspectRatio(contentMode:)` does; both sides set keep the box.
#[test]
fn a_picture_with_one_side_sizes_the_other_from_its_image() {
    use crate::Pixels;
    use crate::SurfaceFit::{Fill, Fit};
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        crate::col().w(240.0).items_start()
            .child(el().key("w").w(120.0).pixels(p))
            .child(el().key("h").h(30.0).pixels_fit(p, Fit))
            .child(el().key("full").w_full().pixels_fit(p, Fill))
            .child(el().key("box").size(100.0, 100.0).pixels_fit(p, Fit))
            .child(el().key("neither").pixels(p))
            .child(el().key("explicit").w(120.0).aspect_ratio(4.0).pixels(p))
            .child(el().key("plain").h(25.0).aspect_ratio(2.0))
    }
    let size = |rt: &Runtime<Pixels>, key: &str| {
        let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap();
        [e.target[2], e.target[3]]
    };
    let px = Pixels::new();
    let mut rt = Runtime::new(px.clone(), view, (240.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(size(&rt, "w"), [120.0, 0.0], "an empty (still decoding) image has no ratio yet");
    assert_eq!(size(&rt, "explicit"), [120.0, 30.0]);
    assert_eq!(size(&rt, "plain"), [50.0, 25.0], "any box can set a ratio");

    // The image arrives without the view re-running, as a decode completing does.
    px.set(4, 2, vec![255; 4 * 4 * 2]);
    rt.frame(1.0 / 120.0);
    assert_eq!(size(&rt, "w"), [120.0, 60.0]);
    assert_eq!(size(&rt, "h"), [60.0, 30.0]);
    assert_eq!(size(&rt, "full"), [240.0, 120.0]);
    assert_eq!(size(&rt, "box"), [100.0, 100.0], "a fixed box keeps its size and letterboxes");
    assert_eq!(size(&rt, "neither"), [0.0, 0.0], "a picture with no side set is unchanged");
    assert_eq!(size(&rt, "explicit"), [120.0, 30.0], "an explicit ratio wins");
    assert!(!rt.elements().iter().any(|e| e.moving), "the new shape lands without animating");

    px.set(2, 4, vec![255; 4 * 2 * 4]);
    rt.frame(1.0 / 120.0);
    assert_eq!(size(&rt, "w"), [120.0, 240.0], "a stream that changes shape relays out");
    let commits = rt.commits;
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.commits, commits, "an unchanged image costs no commits");

    // The image lands on a frame that also re-runs the view (`Pixels::set`, then the
    // handle's `update`, or a hover that frame): it still appears at its size.
    let px = Pixels::new();
    let mut rt = Runtime::new(px.clone(), view, (240.0, 600.0), Env::new(true, 1.0));
    rt.frame(0.0);
    px.set(4, 2, vec![255; 4 * 4 * 2]);
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    let w = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("w")).unwrap();
    assert_eq!((w.rect[3], w.target[3]), (60.0, 60.0), "drawn at its size, not springing open");
    assert!(!w.moving);
}

/// A decode or stream landing after this frame read the picture's size still gets a frame
/// that sizes it, though the frame latched live values after the set.
#[test]
fn a_picture_set_mid_frame_wakes_another_frame_to_size_it() {
    use crate::Pixels;
    fn view(p: &Pixels, _: &mut Cx<Pixels>) -> El {
        col().w(240.0).items_start().child(el().key("w").w(120.0).pixels(p))
    }
    let height = |rt: &Runtime<Pixels>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("w")).unwrap().target[3];
    let px = Pixels::new();
    let mut rt = Runtime::new(px.clone(), view, (240.0, 600.0), Env::new(true, 1.0));
    let wakes = counting_waker(&mut rt);
    rt.frame(0.0);
    assert_eq!(height(&rt), 0.0);

    let set = px.clone();
    MID_FRAME.with(|hook| *hook.borrow_mut() = Some(Box::new(move || set.set(4, 2, vec![255; 4 * 4 * 2]))));
    assert!(!rt.frame(1.0 / 120.0));
    assert_eq!(wakes.load(std::sync::atomic::Ordering::Relaxed), 1);
    assert!(rt.live_pending(), "the set's wake outlives the frame it landed in");
    rt.frame(1.0 / 120.0);
    assert_eq!(height(&rt), 60.0);
    assert!(!rt.live_pending());
}

fn counting_waker<S: 'static>(rt: &mut Runtime<S>) -> Arc<std::sync::atomic::AtomicUsize> {
    let wakes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = wakes.clone();
    rt.set_live_waker(Arc::new(move || { count.fetch_add(1, std::sync::atomic::Ordering::Relaxed); }));
    wakes
}

#[test]
fn a_picture_removed_outright_stops_waking_in_the_frame_that_removes_it() {
    use crate::Pixels;
    fn view(s: &(Pixels, bool), _: &mut Cx<(Pixels, bool)>) -> El {
        let root = el().size(8.0, 8.0);
        if s.1 { root.child(el().key("pic").size(4.0, 4.0).pixels(&s.0)) } else { root }
    }
    let px = Pixels::new();
    let mut rt = Runtime::new((px.clone(), true), view, (8.0, 8.0), Env::new(true, 1.0));
    let wakes = counting_waker(&mut rt);
    rt.frame(0.0);
    rt.update(|s, tx| { s.1 = false; tx.settle(); });
    let set = px.clone();
    MID_FRAME.with(|hook| *hook.borrow_mut() = Some(Box::new(move || set.set(1, 1, vec![255; 4]))));
    rt.frame(0.0);
    assert_eq!(wakes.load(std::sync::atomic::Ordering::Relaxed), 0, "unmounted before the set");
    assert!(!rt.live_pending());
}

/// Everything an exit still draws keeps updating: an exiting picture and a picture inside
/// an exiting element both wake until the exit ends.
#[test]
fn pictures_in_an_exit_wake_until_it_ends() {
    use crate::Pixels;
    use std::sync::atomic::Ordering::Relaxed;
    type State = (Pixels, Pixels, bool);
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let root = el().size(20.0, 20.0);
        if !s.2 { return root; }
        root.child(el().key("direct").size(4.0, 4.0).pixels(&s.0).transition(Transition::opacity()))
            .child(el().key("card").size(8.0, 8.0).transition(Transition::opacity())
                .child(el().key("child").size(4.0, 4.0).pixels(&s.1)))
    }
    let (direct, child) = (Pixels::new(), Pixels::new());
    let mut rt = Runtime::new((direct.clone(), child.clone(), true), view, (20.0, 20.0), Env::new(true, 1.0));
    let wakes = counting_waker(&mut rt);
    rt.frame(0.0);
    rt.update(|s, _| s.2 = false);
    assert!(rt.frame(0.0), "the exit runs");
    direct.set(1, 1, vec![1; 4]);
    assert_eq!(wakes.load(Relaxed), 1, "an exiting picture wakes");
    rt.frame(1.0 / 120.0);
    child.set(1, 1, vec![1; 4]);
    assert_eq!(wakes.load(Relaxed), 2, "a picture inside an exiting element wakes");
    for _ in 0..240 { if !rt.frame(1.0 / 120.0) { break; } }
    assert!(!rt.elements().iter().any(|e| e.key.as_deref() == Some("child")), "the exit ended");
    direct.set(1, 1, vec![2; 4]);
    child.set(1, 1, vec![2; 4]);
    assert_eq!(wakes.load(Relaxed), 2, "gone pictures stay asleep");
    assert!(!rt.live_pending());
}
