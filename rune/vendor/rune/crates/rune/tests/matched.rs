use rune::{Color, Cx, El, Env, Runtime, Spring, Transition, col, el, row};

/// A tile that lives in the left box (40 pt) or, zoomed, in the right box (160 pt).
fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
    let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE).radius(4.0);
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
    row().size(300.0, 200.0).child(left).child(right)
}

const GRID: [f32; 4] = [0.0, 0.0, 40.0, 40.0];
const ONE: [f32; 4] = [100.0, 0.0, 160.0, 160.0];

fn rt() -> Runtime<bool> {
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn tile(rt: &Runtime<bool>) -> [f32; 4] {
    rt.presented("tile").expect("tile").0
}

fn moving(rt: &Runtime<bool>) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some("tile") && e.moving)
}

fn jump(a: [f32; 4], b: [f32; 4]) -> f32 {
    (0..4).map(|k| (a[k] - b[k]).abs()).fold(0.0, f32::max)
}

#[test]
fn a_matched_element_moving_between_parents_flies_from_where_it_was() {
    let mut rt = rt();
    assert_eq!(tile(&rt), GRID);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let first = tile(&rt);
    assert!(jump(first, GRID) < 8.0, "starts at the old rect: {first:?}");
    assert!(moving(&rt), "a flight reports `moving`");
    let mut last = first;
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        let r = tile(&rt);
        assert!(jump(r, last) < 12.0, "continuous: {last:?} -> {r:?}");
        last = r;
    }
    assert_eq!(tile(&rt), ONE);
    assert!(!moving(&rt));
}

#[test]
fn toggling_mid_flight_turns_around_without_a_jump() {
    let mut rt = rt();
    let mut last = tile(&rt);
    let mut worst = 0.0f32;
    for round in 0..4 {
        rt.update(|zoomed, _| *zoomed = !*zoomed);
        for _ in 0..12 + round * 5 {
            rt.frame(1.0 / 120.0);
            let r = tile(&rt);
            worst = worst.max(jump(r, last));
            last = r;
        }
        assert!(moving(&rt), "still in flight when toggled again");
    }
    assert!(worst < 12.0, "no rect jumps between frames: {worst}");
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(tile(&rt), GRID, "an even number of toggles lands back in the grid");
}

#[test]
fn reduced_motion_swaps_in_place() {
    let mut rt = rt();
    rt.set_system_reduced_motion(true);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    assert_eq!(tile(&rt), ONE);
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        assert_eq!(tile(&rt), ONE);
    }
}

#[test]
fn children_scale_with_the_flight() {
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .child(el().key("dot").size(size / 4.0, size / 4.0).fill(Color::BLACK));
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    for _ in 0..10 {
        rt.frame(1.0 / 120.0);
    }
    let (t, _) = rt.presented("tile").unwrap();
    let (d, _) = rt.presented("dot").unwrap();
    assert!(t[2] > 50.0 && t[2] < 150.0, "mid-flight: {t:?}");
    assert!((d[0] - t[0]).abs() < 1.0 && (d[1] - t[1]).abs() < 1.0, "rides the tile's origin: {t:?} {d:?}");
    assert!((d[2] - t[2] / 4.0).abs() < 1.0 && (d[3] - t[3] / 4.0).abs() < 1.0, "a quarter of the tile: {t:?} {d:?}");
}

#[test]
fn unmatched_elements_are_untouched() {
    fn plain(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").size(size, size).fill(Color::WHITE);
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, plain, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let (r, _) = rt.presented("tile").unwrap();
    assert_eq!([r[0], r[1]], [ONE[0], ONE[1]], "a plain remount enters at its own place");
}

fn moving_key(rt: &Runtime<bool>, key: &str) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some(key) && e.moving)
}

#[test]
fn a_matched_element_inside_a_matched_one_flies_its_own_straight_path() {
    // The badge sits at the card's top-left in the grid and below a gap when zoomed, in a card
    // whose aspect changes, so riding the card's cover would bend or offset its path.
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let badge = |size: f32| el().key("badge").matched("badge").size(size, size).fill(Color::BLACK);
        let mut left = el().key("left").size(100.0, 200.0);
        let mut right = el().key("right").size(200.0, 200.0);
        if *zoomed {
            right = right.child(col().key("card").matched("card").size(160.0, 80.0).fill(Color::WHITE)
                .child(el().key("gap").size(10.0, 40.0)).child(badge(40.0)));
        } else {
            left = left.child(col().key("card").matched("card").size(40.0, 40.0).fill(Color::WHITE).child(badge(10.0)));
        }
        row().size(300.0, 200.0).child(left).child(right)
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let start = rt.presented("badge").unwrap().0;
    assert_eq!(start, [0.0, 0.0, 10.0, 10.0]);
    rt.update(|zoomed, _| *zoomed = true);
    rt.frame(1.0 / 120.0);
    let first = rt.presented("badge").unwrap().0;
    assert!(jump(first, start) < 3.0, "starts where the old badge was: {first:?}");
    assert!(moving_key(&rt, "badge") && moving_key(&rt, "card"), "both fly");
    let mut mid = false;
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        let b = rt.presented("badge").unwrap().0;
        let t = b[0] / 100.0;
        mid |= t > 0.3 && t < 0.7;
        assert!((b[1] - 40.0 * t).abs() < 1.0, "on the straight path from its start to its end: {b:?}");
        assert!((b[2] - (10.0 + 30.0 * t)).abs() < 1.5, "grows with its own progress: {b:?}");
    }
    assert!(mid, "sampled mid-flight");
    assert_eq!(rt.presented("badge").unwrap().0, [100.0, 40.0, 40.0, 40.0]);
    assert_eq!(rt.presented("card").unwrap().0, [100.0, 0.0, 160.0, 80.0]);
}

/// 0: the tile in the left box, 1: in the right box, 2: gone, 3: the right box gone with it.
fn removable(state: &u8, _: &mut Cx<u8>) -> El {
    let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
        .child(el().key("dot").size(size / 4.0, size / 4.0).fill(Color::BLACK));
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    match state {
        0 => left = left.child(tile(40.0)),
        1 => right = right.child(tile(160.0)),
        _ => {}
    }
    let r = row().size(300.0, 200.0).child(left);
    if *state == 3 { r } else { r.child(right) }
}

#[test]
fn a_flight_removed_mid_way_exits_without_a_pop() {
    for gone in [2u8, 3] {
        let mut rt = Runtime::new(0u8, removable, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|s, _| *s = 1);
        for _ in 0..10 {
            rt.frame(1.0 / 120.0);
        }
        let mut last = rt.presented("tile").unwrap();
        assert!(last.0[2] > 50.0 && last.0[2] < 150.0, "mid-flight: {last:?}");
        rt.update(move |s, _| *s = gone);
        let mut faded = false;
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let Some(now) = rt.presented("tile") else { break };
            let dot = rt.presented("dot").unwrap().0;
            assert!((dot[2] - now.0[2] / 4.0).abs() < 1.5, "children stay scaled with the flight ({gone}): {now:?} {dot:?}");
            assert!(jump(now.0, last.0) < 12.0, "continuous ({gone}): {last:?} -> {now:?}");
            assert!(now.1 <= last.1 + 1e-3, "fades out ({gone}): {} -> {}", last.1, now.1);
            faded |= now.1 > 0.2 && now.1 < 0.8;
            last = now;
        }
        assert!(faded, "an exit fade was sampled ({gone})");
        assert!(rt.presented("tile").is_none(), "gone once faded ({gone})");
    }
}

#[test]
fn a_fast_flight_waits_for_its_entering_parent() {
    // The flight lands well before the right box finishes entering with a scale and fade.
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let tile = |size: f32| el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .motion(Spring::new(0.08, 0.0));
        let r = row().size(300.0, 200.0);
        if *zoomed {
            r.child(el().key("left").size(100.0, 200.0))
                .child(el().key("right").size(200.0, 200.0).transition(Transition::scale(0.9)).child(tile(160.0)))
        } else {
            r.child(el().key("left").size(100.0, 200.0).child(tile(40.0)))
        }
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    let mut landed = None;
    for frame in 0..120 {
        rt.frame(1.0 / 120.0);
        let (r, opacity) = rt.presented("tile").unwrap();
        assert!(opacity > 0.99, "keeps its own opacity at frame {frame}: {opacity}");
        if landed.is_some() {
            assert!(jump(r, ONE) < 0.5, "stays where it landed at frame {frame}: {r:?}");
        } else if jump(r, ONE) < 0.5 {
            landed = Some(rt.presented("right").unwrap().1);
        }
    }
    let parent = landed.expect("landed");
    assert!(parent < 0.9, "landed while the parent was still entering: its opacity {parent}");
}

#[test]
fn reduced_motion_fades_the_old_one_with_the_parent_it_leaves_with() {
    fn view(zoomed: &bool, _: &mut Cx<bool>) -> El {
        let r = row().size(300.0, 200.0);
        if *zoomed {
            r.child(el().key("spacer").size(100.0, 200.0)).child(el().key("right").size(200.0, 200.0)
                .child(el().key("new").matched("tile").size(160.0, 160.0).fill(Color::WHITE)))
        } else {
            r.child(col().key("left").size(100.0, 200.0)
                .child(el().key("old").matched("tile").size(40.0, 40.0).fill(Color::WHITE))
                .child(el().key("peer").size(40.0, 40.0).fill(Color::WHITE)))
        }
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    let mut sampled = false;
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let (Some(old), Some(peer)) = (rt.presented("old"), rt.presented("peer")) else { break };
        assert!((old.1 - peer.1).abs() < 0.02, "fades with its sibling: {} vs {}", old.1, peer.1);
        sampled |= peer.1 > 0.2 && peer.1 < 0.8;
        assert_eq!(rt.presented("new").unwrap().0, ONE, "the new one is in place");
    }
    assert!(sampled, "the fade was sampled");
}

/// A tile two parents deep in content, in the left box or, zoomed, in the right box.
fn nested(zoomed: &bool, _: &mut Cx<bool>) -> El {
    let tile = |size: f32| {
        el().key("tile").matched("tile").size(size, size).fill(Color::WHITE)
            .child(row().key("pill").w_full().h(20.0).child(el().key("mark").size(8.0, 8.0).child(el().key("dot").size(4.0, 4.0).fill(Color::BLACK))))
    };
    let mut left = el().key("left").size(100.0, 200.0);
    let mut right = el().key("right").size(200.0, 200.0);
    if *zoomed { right = right.child(tile(160.0)) } else { left = left.child(tile(40.0)) }
    row().size(300.0, 200.0).child(left).child(right)
}

#[test]
fn a_flights_content_flies_with_it_without_fading_in() {
    let mut rt = Runtime::new(false, nested, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|zoomed, _| *zoomed = true);
    for frame in 0..12 {
        rt.frame(1.0 / 120.0);
        assert!(moving(&rt), "still in flight");
        for key in ["tile", "pill", "mark", "dot"] {
            let o = rt.elements().iter().find(|e| e.key.as_deref() == Some(key)).map_or(0.0, |e| e.opacity);
            assert!(o > 0.999, "frame {frame}: {key} drawn at {o:.3}");
        }
    }
}
