use super::*;

const DT: f64 = 1.0 / 120.0;

/// What the rows heard: `on_hover` enters and `on_hover_change` changes.
#[derive(Default)]
struct Rows {
    growth: bool, open: bool, gone: bool, modal: bool,
    /// Rows with only a hover fill, and a pressable spot over row 5 that `pan` moves.
    plain: bool, pan: Option<crate::live::Live<[f32; 2]>>, spot: Vec<bool>,
    /// Rows with tooltips, and cursors that differ from their neighbours'.
    tips: bool,
    enters: Vec<usize>, changes: Vec<(usize, bool)>,
}

const CURSORS: [crate::el::Cursor; 3] = [crate::el::Cursor::Pointer, crate::el::Cursor::Move, crate::el::Cursor::Grab];

/// Twelve 20-point rows, each revealing an action on hover and darkening while pressed.
/// Opening row 0 grows it by 200 points, and the rows below slide down under the pointer.
fn view(s: &Rows, _: &mut Cx<Rows>) -> El {
    let rows = (0..12).filter(|&k| !(s.gone && k == 5)).map(|k| {
        let row = crate::row().key(format!("r{k}")).w_full().h(if k == 0 && s.open { 220.0 } else { 20.0 });
        let row = if s.plain { row.hover(Color::WHITE) } else {
            row.hover_group().press(Color::WHITE).press_scale(0.9)
                .on_press(|_: &mut Rows| {})
                .on_hover(move |s: &mut Rows| s.enters.push(k))
                .on_hover_change(move |s: &mut Rows, inside, _| s.changes.push((k, inside)))
                .child(el().key(format!("a{k}")).size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0))
        };
        if s.tips { row.help(format!("row {k}")).cursor(CURSORS[k % 3]) } else { row }
    });
    let mut list = col().key("rows").w_full().children(rows);
    if s.growth { list = list.growth(crate::el::GrowthSpec::default()); }
    let mut root = col().size(200.0, 400.0).child(list);
    if let Some(pan) = &s.pan {
        root = root.child(el().key("spot").absolute(90.0, 100.0).size(20.0, 20.0).live_offset(pan)
            .on_press(|_: &mut Rows| {}).on_hover_change(|s: &mut Rows, inside, _| s.spot.push(inside)));
    }
    if s.modal { root.child(col().key("sheet").absolute(150.0, 350.0).size(40.0, 40.0).modal(true, |s: &mut Rows| s.modal = false)) } else { root }
}

/// The rows whose action shows at all this frame.
fn shown(rt: &Runtime<Rows>) -> Vec<String> {
    rt.elements().into_iter().filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('a')) && e.opacity > 0.0)
        .filter_map(|e| e.key).collect()
}

/// The row drawn under `y` this frame.
fn row_at(rt: &Runtime<Rows>, y: f32) -> Option<usize> {
    rt.elements().into_iter().filter(|e| e.rect[1] <= y && y < e.rect[1] + e.rect[3])
        .find_map(|e| e.key?.strip_prefix('r')?.parse().ok())
}

fn top(rt: &Runtime<Rows>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect[1]
}

/// The pointer resting over row 5.
fn resting(rows: Rows) -> Runtime<Rows> {
    let mut rt = Runtime::new(rows, view, (200.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(100.0, 110.0);
    rt.frame(DT);
    rt.frame(DT);
    rt
}

/// Row 5 hovered under a still pointer, then row 0 opening.
fn sliding(growth: bool) -> Runtime<Rows> {
    let mut rt = resting(Rows { growth, ..Rows::default() });
    assert_eq!(shown(&rt), ["a5"]);
    assert_eq!((&rt.state.enters[..], &rt.state.changes[..]), (&[5][..], &[(5, true)][..]));
    rt.update(|s, _| s.open = true);
    rt
}

/// Runs frames until row 11 stops moving, returning how many it moved on and every action shown.
fn settle(rt: &mut Runtime<Rows>) -> (usize, Vec<String>) {
    let (mut moved, mut seen) = (0, Vec::new());
    let mut y = top(rt, "r11");
    for _ in 0..600 {
        rt.frame(DT);
        for k in shown(rt) { if !seen.contains(&k) { seen.push(k); } }
        let now = top(rt, "r11");
        if (now - y).abs() < 1e-4 && !rt.animating { break; }
        if now != y { moved += 1; }
        y = now;
    }
    for _ in 0..30 { rt.frame(DT); }
    (moved, seen)
}

#[test]
fn rows_sliding_under_a_still_pointer_dont_take_its_hover() {
    for growth in [false, true] {
        let mut rt = sliding(growth);
        let (moved, seen) = settle(&mut rt);
        assert!(moved > 5, "growth {growth}: the rows slid");
        assert_eq!(seen, ["a5", "a0"], "growth {growth}: no row passing under the pointer lit up");
        assert_eq!(rt.state.enters, [5, 0], "growth {growth}");
        assert_eq!(rt.state.changes, [(5, true), (5, false), (0, true)], "growth {growth}");
        assert_eq!(row_at(&rt, 110.0), Some(0), "growth {growth}");
        assert_eq!(shown(&rt), ["a0"], "growth {growth}: once they settle, the row under the pointer has it");
        assert!(!rt.hover_hold.held && !rt.hover_hold.released);
    }
}

#[test]
fn moving_the_pointer_during_a_slide_retargets_at_once() {
    let mut rt = sliding(false);
    for _ in 0..6 { rt.frame(DT); }
    assert_eq!(shown(&rt), ["a5"]);
    assert!(rt.hover_hold.held);
    let under = row_at(&rt, 111.0).unwrap();
    assert_ne!(under, 5, "a row slid under the pointer");
    rt.pointer_move(100.0, 111.0);
    rt.frame(DT);
    assert_eq!(rt.state.enters, [5, under]);
    assert_eq!(rt.state.changes, [(5, true), (5, false), (under, true)]);
    let now = format!("a{}", row_at(&rt, 111.0).unwrap());
    assert!(shown(&rt).contains(&now), "{now}: {:?}", shown(&rt));
}

#[test]
fn the_hovered_row_leaving_mid_slide_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.update(|s, _| s.gone = true);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.state.changes, [(5, true), (5, false)], "the leaving row hears the pointer go");
    let (_, seen) = settle(&mut rt);
    assert!(seen.iter().all(|k| k == "a5" || k == "a0"), "nothing else lit up: {seen:?}");
    assert_eq!(rt.state.enters, [5, 0]);
    assert_eq!(rt.state.changes, [(5, true), (5, false), (0, true)]);
}

#[test]
fn a_modal_opening_mid_slide_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.update(|s, _| s.modal = true);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.state.changes, [(5, true), (5, false)], "the inert row hears the pointer go");
    settle(&mut rt);
    assert_eq!(shown(&rt), Vec::<String>::new());
    assert_eq!(rt.state.enters, [5]);
}

#[test]
fn a_press_during_a_slide_still_shows_and_lets_go() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    let under = row_at(&rt, 110.0).unwrap();
    let id = rt.nodes.iter().find(|n| n.named(&format!("r{under}"))).unwrap().id;
    rt.pointer_down(100.0, 110.0);
    rt.frame(DT);
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(1.0), "the press shows mid-slide");
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(0.9));
    rt.pointer_up(100.0, 110.0);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(rt.scene.store().target(id, P_PRESS), Some(0.0), "the press lets go mid-slide");
    assert_eq!(rt.scene.store().target(id, P_PRESS_SCALE), Some(1.0));
    assert!(!rt.hover_hold.held && rt.hover_hold.released, "the press let go of the hold until the slide settles");
}

/// Each row's enters minus its leaves, from `on_hover_change`.
fn inside(rt: &Runtime<Rows>) -> Vec<usize> {
    let mut net = [0i32; 12];
    for &(k, on) in &rt.state.changes { net[k] += if on { 1 } else { -1 }; }
    assert!(net.iter().all(|&n| n == 0 || n == 1), "every enter is heard once before its leave: {:?}", rt.state.changes);
    (0..12).filter(|&k| net[k] == 1).collect()
}

#[test]
fn a_press_mid_slide_follows_the_pointer_and_pairs_each_enter_with_a_leave() {
    let mut rt = sliding(false);
    for _ in 0..4 { rt.frame(DT); }
    rt.pointer_down(100.0, 110.0);
    assert!(rt.hover_hold.released, "a press lets go of the hold");
    for _ in 0..40 {
        rt.frame(DT);
        let inside = inside(&rt);
        assert!(inside.len() <= 2, "the pressed row and the one under the pointer: {inside:?}");
        assert!(rt.hover_within.len() <= 2, "{:?}", rt.hover_within);
    }
    rt.pointer_up(100.0, 110.0);
    settle(&mut rt);
    assert_eq!(inside(&rt), [0]);
    assert_eq!(rt.hover_within.len(), 1);
}

#[test]
fn a_move_before_the_next_tick_sees_layout_just_set_moving() {
    // A spring a commit between frames started (a scroll's), not yet ticked.
    let mut rt = resting(Rows::default());
    let id = rt.nodes.iter().find(|n| n.named("r3")).unwrap().id;
    rt.scene.animate(id, prop::OFFSET, rune_motion::Vec2::new(0.0, 40.0), motion::SNAPPY);
    rt.pointer_move(100.0, 111.0);
    assert!(rt.hover_hold.released, "an untouched spring");
    // A growing element's height.
    let mut rt = resting(Rows::default());
    let id = rt.nodes.iter().find(|n| n.named("r3")).unwrap().id;
    rt.grows.insert(id, crate::growth::GrowTrack { target: 100.0, ..Default::default() });
    rt.pointer_move(100.0, 111.0);
    assert!(rt.hover_hold.released, "a growth");
}

#[test]
fn a_hold_keeps_all_three_hover_states_together() {
    let pan = crate::live::Live::new([0.0, 0.0]);
    let mut rt = resting(Rows { plain: true, pan: Some(pan.clone()), ..Rows::default() });
    let target = |rt: &Runtime<Rows>, key: &str| {
        let id = rt.nodes.iter().find(|n| n.named(key)).unwrap().id;
        rt.scene.store().target(id, P_HOVER)
    };
    let spot = rt.nodes.iter().find(|n| n.named("spot")).unwrap().path.clone();
    assert_eq!(rt.hovered.as_deref(), Some(spot.as_str()));
    assert_eq!((&rt.state.spot[..], target(&rt, "r5")), (&[true][..], Some(1.0)));
    rt.update(|s, _| s.open = true);
    for _ in 0..6 { rt.frame(DT); }
    assert_ne!(row_at(&rt, 110.0), Some(5), "a row slid under the pointer");
    assert_eq!(target(&rt, "r5"), Some(1.0), "the rows' hover is held");
    // The spot jumps out from under the pointer while the rows' hover is held.
    pan.set([-80.0, 0.0]);
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!(target(&rt, "r5"), Some(1.0));
    assert_eq!(rt.state.spot, [true], "the spot's leave waits with the rows' hover");
    assert_eq!(rt.hovered.as_deref(), Some(spot.as_str()), "and so does the hovered element");
    settle(&mut rt);
    assert_eq!(rt.state.spot, [true, false]);
    assert_eq!(rt.hovered, None);
    assert_eq!((target(&rt, "r5"), target(&rt, "r0")), (Some(0.0), Some(1.0)));
}

#[test]
fn the_cursor_and_tooltip_stay_with_the_held_row() {
    let mut rt = resting(Rows { tips: true, ..Rows::default() });
    rt.help_delay = 0.0;
    rt.frame(DT);
    rt.frame(DT);
    assert_eq!((rt.cursor(), rt.help_tooltip().map(|t| t.0.to_owned())), (CURSORS[2], Some("row 5".to_owned())));
    rt.update(|s, _| s.open = true);
    let (mut cursors, mut tips) = (Vec::new(), Vec::new());
    for _ in 0..120 {
        rt.frame(DT);
        if !cursors.contains(&rt.cursor()) { cursors.push(rt.cursor()); }
        let tip = rt.help_tooltip().map(|t| t.0.to_owned());
        if !tips.contains(&tip) { tips.push(tip); }
    }
    settle(&mut rt);
    assert_eq!(cursors, [CURSORS[2], CURSORS[0]], "row 5's cursor, then row 0's");
    assert!(tips.iter().flatten().all(|t| t == "row 5" || t == "row 0"), "no tooltip for a passing row: {tips:?}");
    assert_eq!((rt.cursor(), rt.help_tooltip().map(|t| t.0.to_owned())), (CURSORS[0], Some("row 0".to_owned())));
}
