use super::*;
use crate::el::GrowthSpec;

/// A growth stack whose middle row is 600 pt open or 40 pt shut, with `El::fold_glide` or else
/// `El::arrives_whole`.
fn stack(open: bool, fold: bool) -> El {
    let mut row = el().key("group").h(if open { 600.0 } else { 40.0 }).w_full();
    row = if fold { row.fold_glide(rune_motion::Spring::new(0.18, 0.0)) } else { row.arrives_whole() };
    col().key("root").child(col().key("stack").growth(GrowthSpec::default()).children([
        el().key("above").h(100.0).w_full(),
        row,
        el().key("below").h(100.0).w_full(),
    ]))
}

/// Seconds until the row shut from 600 pt is within half a point of shut, at 120 Hz.
fn shut_secs(fold: bool) -> f32 {
    fn folded(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        stack(s.0, s.1)
    }
    let mut rt = Runtime::new((true, fold), folded, (400.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.0 = false;
    rt.dirty = true;
    for k in 1..240 {
        rt.frame(1.0 / 120.0);
        if fold && k < 12 {
            assert!(rt.row_mask.iter().any(|&m| m >= 0.0), "shutting, the row clips what it held");
        }
        let below = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("below")).unwrap();
        if (below.rect[1] - 140.0).abs() < 0.5 {
            return k as f32 / 120.0;
        }
    }
    panic!("still moving after 2 s");
}

#[test]
fn a_fold_glide_row_folds_on_its_own_spring() {
    let own = shut_secs(true);
    let stack = shut_secs(false);
    assert!(own < 0.3, "the row's own 0.18 s spring: {own} s");
    assert!(stack > own + 0.2, "the stack's 0.32 s spring, stretched for the distance: {stack} s");
}

/// The row's drawn top and height each frame of opening, in a 300 pt window with the stack
/// drawn at `zoom`.
fn opening(zoom: f32) -> Vec<(f32, f32)> {
    fn folded(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
        col().key("zoom").scale(s.1).child(stack(s.0, true))
    }
    let mut rt = Runtime::new((false, zoom), folded, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.state.0 = true;
    rt.dirty = true;
    (0..60).map(|_| {
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("group")).unwrap();
        (row.rect[1], row.rect[3])
    }).collect()
}

#[test]
fn a_zoomed_fold_moves_only_what_shows_of_it() {
    let frames = opening(1.5);
    let (top, full) = (frames[0].0, 600.0 * 1.5);
    assert!(top < 300.0, "{frames:?}");
    assert!(frames.iter().all(|&(y, h)| y + h <= 300.5 || h == full), "glides to the window's edge, then lands: {frames:?}");
    assert!(frames[20].0 + frames[20].1 > 290.0 && frames[59].1 == full, "{frames:?}");
}

#[test]
fn a_fold_moves_only_what_shows_of_it() {
    fn folded(open: &bool, _: &mut Cx<bool>) -> El {
        stack(*open, true)
    }
    // The row starts 100 pt down a 300 pt window, so 200 pt of it shows.
    let mut rt = Runtime::new(false, folded, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    rt.state = true;
    rt.dirty = true;
    let mut heights = Vec::new();
    for _ in 0..60 {
        rt.frame(1.0 / 120.0);
        let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("group")).unwrap();
        heights.push(row.rect[3]);
    }
    assert!(heights.iter().all(|&h| h <= 200.5 || h == 600.0), "glides to the window's edge, then lands: {heights:?}");
    assert!(heights[0] < 100.0 && heights[20] > 190.0 && heights[59] == 600.0, "{heights:?}");
}

/// Rows `a` (a fold, 600 pt open or 40 pt shut) and `b` (100 pt), in either order, between a
/// 100 pt row above and one below.
fn reordering(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
    let a = el().key("a").w_full().h(if s.0 { 600.0 } else { 40.0 }).fold_glide(rune_motion::Spring::new(0.18, 0.0));
    let b = el().key("b").w_full().h(100.0);
    let (first, second) = if s.1 { (b, a) } else { (a, b) };
    col().key("root").child(col().key("stack").w_full().growth(GrowthSpec::default()).children([
        el().key("above").h(100.0).w_full(),
        first,
        second,
        el().key("below").h(100.0).w_full(),
    ]))
}

/// Each key's (top, height) every frame for one second after `a` shuts and swaps with `b` in
/// one commit, in a window `window_h` tall.
fn shut_and_swap(window_h: f32) -> (Runtime<(bool, bool)>, Vec<HashMap<&'static str, (f32, f32)>>) {
    let mut rt = Runtime::new((true, false), reordering, (400.0, window_h), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    rt.update(|s, _| *s = (false, true));
    let frames = (0..120).map(|_| {
        rt.frame(1.0 / 120.0);
        let els = rt.elements();
        ["above", "a", "b", "below"].into_iter().map(|k| {
            let e = els.iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap();
            (k, (e.rect[1], e.rect[3]))
        }).collect()
    }).collect();
    (rt, frames)
}

#[test]
fn a_fold_that_reorders_in_the_same_commit_slides_and_folds_at_once() {
    // All on screen: the fold shuts on its spring while it and `b` slide into their new places,
    // without a jump, and everything lands where the new order lays it out.
    let (rt, frames) = shut_and_swap(900.0);
    for w in frames.windows(2) {
        for k in ["above", "a", "b", "below"] {
            let ((y0, h0), (y1, h1)) = (w[0][k], w[1][k]);
            assert!((y1 - y0).abs() < 60.0 && (h1 - h0).abs() < 60.0, "{k} jumps from {y0}+{h0} to {y1}+{h1}: {frames:?}");
        }
        assert!(w[1]["a"].1 <= w[0]["a"].1 + 1e-3, "the fold only shuts: {frames:?}");
    }
    let last = frames.last().unwrap();
    assert_eq!((last["above"], last["b"], last["a"], last["below"]), ((0.0, 100.0), (100.0, 100.0), (200.0, 40.0), (240.0, 100.0)));
    assert!(frames[36]["a"].1 < 41.0, "the fold shuts on its own 0.18 s spring: {:?}", frames[36]["a"]);
    assert!(!rt.animating);
    // The fold past the window's bottom: what shows of it still shuts and slides without a jump,
    // and `b`, drawn below the window, comes in from its edge rather than from far below it.
    let (_, frames) = shut_and_swap(300.0);
    for w in frames.windows(2) {
        let ((y0, h0), (y1, h1)) = (w[0]["a"], w[1]["a"]);
        assert!((y1 - y0).abs() < 60.0 && ((y1 + h1).min(300.0) - (y0 + h0).min(300.0)).abs() < 60.0, "a jumps from {y0}+{h0} to {y1}+{h1}: {frames:?}");
        assert!((w[1]["b"].0 - w[0]["b"].0).abs() < 60.0, "b jumps: {frames:?}");
    }
    assert!(frames[0]["b"].0 <= 300.0, "{:?}", frames[0]["b"]);
    let last = frames.last().unwrap();
    assert_eq!((last["b"], last["a"]), ((100.0, 100.0), (200.0, 40.0)));
}

/// A section in a fold row: a 28 pt header, sticky to the top, over 1,120 pt of steps when open,
/// 200 pt down a 200 pt scroll whose rows below are 100 pt each.
fn section(open: &bool, _: &mut Cx<bool>) -> El {
    let mut group = col().key("group").w_full().child(el().key("hdr").w_full().h(28.0).sticky_top(0.0));
    if *open {
        group = group.child(col().key("steps").w_full().children((0..40).map(|k| el().key(format!("step-{k}")).w_full().h(28.0))));
    }
    let stack = col().key("stack").w_full().growth(GrowthSpec::default()).children([
        el().key("before").w_full().h(200.0),
        col().key("turn").w_full().fold_glide(rune_motion::Spring::new(0.18, 0.0)).child(group),
        el().key("after").w_full().h(100.0),
        el().key("tail").w_full().h(100.0),
    ]);
    col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
}

#[test]
fn keep_on_screen_holds_a_stuck_header_while_its_fold_shuts() {
    let mut rt = Runtime::new(true, section, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 700.0));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let find = |rt: &Runtime<bool>, k: &str| rt.elements().into_iter().find(|e| e.key.as_deref() == Some(k) && !e.exiting).unwrap();
    assert!(find(&rt, "hdr").rect[1] == 0.0 && find(&rt, "hdr").stuck > 400.0, "{:?}", find(&rt, "hdr"));
    rt.update(|open, tx| { *open = false; tx.keep_on_screen("hdr"); });
    let mut after = Vec::new();
    for f in 0..120 {
        rt.frame(1.0 / 120.0);
        let hdr = find(&rt, "hdr").rect[1];
        assert!(hdr.abs() < 0.5, "frame {f}: the header moved to {hdr}");
        let a = find(&rt, "after").rect[1];
        // The row below never shows under the header, nor drops once it shows.
        assert!(a >= 28.0 - 0.5, "frame {f}: the row below rose to {a}, over the header");
        after.push(a);
    }
    let shown: Vec<f32> = after.iter().copied().skip_while(|&a| a >= 200.0).collect();
    assert!(shown.windows(2).all(|w| w[1] <= w[0] + 0.5), "the row below only rises once it shows: {after:?}");
    assert!(after[36] < 28.5, "it reaches the header on the fold's own spring: {after:?}");
    assert!(*after.last().unwrap() == 28.0 && rt.keeps.is_empty() && !rt.animating, "{after:?}");
}

/// A fold shutting from 600 pt feathers its moving bottom edge to its `fold_feather` depth,
/// and nothing once it lands.
#[test]
fn a_fold_feathers_its_edge_shutting_too() {
    fn folded(open: &bool, _: &mut Cx<bool>) -> El {
        let row = el().key("group").h(if *open { 600.0 } else { 40.0 }).w_full().fold_glide(rune_motion::Spring::new(0.18, 0.0)).fold_feather(16.0);
        col().key("root").child(col().key("stack").growth(GrowthSpec::default()).children([el().key("above").h(100.0).w_full(), row]))
    }
    let mut rt = Runtime::new(true, folded, (400.0, 900.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| *s = false);
    let group = |rt: &Runtime<bool>| rt.nodes.iter().position(|n| n.named("group")).unwrap();
    let masks: Vec<f32> = (0..60).map(|_| {
        rt.frame(1.0 / 120.0);
        rt.row_mask[group(&rt)]
    }).collect();
    let peak = masks.iter().copied().fold(0.0, f32::max);
    assert!((peak - 16.0).abs() < 1e-3, "shutting, the moving edge feathers {peak} pt deep: {masks:?}");
    assert!(masks.last().is_some_and(|&m| m < 0.0), "and nothing is feathered once it lands: {masks:?}");
}

/// A fold of 40 steps under a sticky 28 pt header, between a 200 pt row above and a row `after`
/// points tall below, in a 200 pt scroll.
fn held_section(s: &(bool, f32), _: &mut Cx<(bool, f32)>) -> El {
    let mut group = col().key("group").w_full().child(el().key("hdr").w_full().h(28.0).sticky_top(0.0));
    if s.0 {
        group = group.child(col().key("steps").w_full().children((0..40).map(|k| el().key(format!("step-{k}")).w_full().h(28.0))));
    }
    let stack = col().key("stack").w_full().growth(GrowthSpec::default()).children([
        el().key("before").w_full().h(200.0),
        col().key("turn").w_full().fold_glide(rune_motion::Spring::new(0.18, 0.0)).child(group),
        el().key("after").w_full().h(s.1),
    ]);
    col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
}

/// Shuts the fold from its stuck header with the timeline scrolled to `offset`, holding the
/// header, and returns each frame's tops of the header, the rows around the fold and every step
/// that draws in the window.
fn shut_from_stuck(after: f32, offset: f32) -> Vec<HashMap<String, f32>> {
    let mut rt = Runtime::new((true, after), held_section, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, offset));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    let hdr = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("hdr")).unwrap();
    assert!(hdr.rect[1] == 0.0 && hdr.stuck > 400.0, "{hdr:?}");
    rt.update(|s, tx| { s.0 = false; tx.keep_on_screen("hdr"); });
    let frames = (0..90).map(|_| {
        rt.frame(1.0 / 120.0);
        rt.elements().into_iter().filter_map(|e| {
            let k = e.key.clone()?;
            let shows = e.rect[1] < 200.0 && e.rect[1] + e.rect[3] > 28.0;
            (["hdr", "before", "after"].contains(&k.as_str()) || (k.starts_with("step-") && shows)).then_some((k, e.rect[1]))
        }).collect()
    }).collect();
    assert!(!rt.animating && rt.keeps.is_empty());
    frames
}

/// The most anything that shows moved in one frame, and the frame.
fn biggest_step(frames: &[HashMap<String, f32>]) -> (f32, usize) {
    let mut most = (0.0, 0);
    for (f, w) in frames.windows(2).enumerate() {
        for (k, &y1) in &w[1] {
            let Some(&y0) = w[0].get(k) else { continue };
            let seen = |y: f32| y > -28.0 && y < 200.0;
            if (seen(y0) || seen(y1)) && (y1 - y0).abs() > most.0 { most = ((y1 - y0).abs(), f + 1); }
        }
    }
    most
}

#[test]
fn a_fold_shut_from_its_stuck_header_at_the_end_slides_up_under_it() {
    // 1,120 pt of steps, the header stuck at the top, the last steps and the row below in view:
    // shut, the timeline ends 128 pt down what it shows, so the header comes down to 72.
    let frames = shut_from_stuck(100.0, 1100.0);
    let (most, at) = biggest_step(&frames);
    assert!(most < 24.0, "nothing that shows moves {most} pt in a frame (frame {at}): {:?}", &frames[at.saturating_sub(1)..=at]);
    let after: Vec<f32> = frames.iter().map(|f| f["after"]).collect();
    assert!(after.windows(2).all(|w| w[1] <= w[0] + 0.01), "the row below only rises: {after:?}");
    // The steps that show ride up with the row below the whole way, slowing as the header
    // comes down over them.
    for w in frames.windows(2) {
        for (k, &y1) in w[1].iter().filter(|(k, _)| k.starts_with("step-")) {
            if let Some(&y0) = w[0].get(k) { assert!(y1 <= y0 + 0.01, "{k} moves down, {y0} to {y1}"); }
        }
    }
    let last = frames.last().unwrap();
    assert_eq!((last["hdr"], last["after"], last["before"]), (72.0, 100.0, -128.0));
    assert!(frames[40]["hdr"] > 71.0, "it lands on the fold's own spring: {:?}", frames[40]);
}

#[test]
fn a_fold_shut_from_its_stuck_header_mid_timeline_still_glides() {
    // The rows below fill the window: the header stays, and what shows slides up under it on
    // the fold's spring with the row below, instead of vanishing.
    let frames = shut_from_stuck(300.0, 1100.0);
    let (most, at) = biggest_step(&frames);
    assert!(most < 24.0, "nothing that shows moves {most} pt in a frame (frame {at}): {:?}", &frames[at.saturating_sub(1)..=at]);
    assert!(frames.iter().all(|f| f["hdr"].abs() < 0.01), "the header holds");
    let after: Vec<f32> = frames.iter().map(|f| f["after"]).collect();
    assert!(after[2] > 100.0 && after[12] < 100.0 && after[40] < 28.5, "glides, rather than landing at once: {after:?}");
    assert!(frames[4].keys().any(|k| k.starts_with("step-")), "the steps still show as it shuts: {:?}", frames[4]);
    assert_eq!(*after.last().unwrap(), 28.0);
}

#[test]
fn a_long_fold_shut_from_its_stuck_header_at_the_end_is_one_motion() {
    // The rehearsal's long tool group: shut from its stuck header with the timeline's end just
    // below the window. What shows used to ride the bottom edge up for 2 or 3 frames, stop dead
    // when the end held the row below, and leave the header to come down over it alone, so the
    // steps it covered landed out of sight. Now the steps slow as the header comes down, without
    // a dead stop, nothing moves faster than the fold's own spring, and nothing lands unseen.
    // Down to a few points short of the end, where a frame's shut is more than the end has
    // left to give, and the viewport clamps at its end before the header's hold moves it.
    for short in [130.0, 80.0, 40.0, 20.0, 8.0] {
        let mut rt = Runtime::new((true, 100.0), held_section, (300.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        let max = rt.scrolls.values().next().unwrap().max as f32;
        let offset = max - short;
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, offset));
        for _ in 0..4 { rt.frame(1.0 / 120.0); }
        rt.update(|s, tx| { s.0 = false; tx.keep_on_screen("hdr"); });
        // Each frame: the fold's height, and each element's top, visible height and height.
        let mut frames: Vec<(f32, HashMap<String, (f32, f32, f32)>)> = Vec::new();
        for _ in 0..120 {
            rt.frame(1.0 / 120.0);
            let els = rt.elements();
            let turn = els.iter().find(|e| e.key.as_deref() == Some("turn")).unwrap().rect[3];
            frames.push((turn, els.into_iter().filter_map(|e| {
                let k = e.key.clone()?;
                (k == "hdr" || k == "after" || k.starts_with("step-")).then_some((k, (e.rect[1], e.visible[3], e.rect[3])))
            }).collect()));
        }
        assert!(!rt.animating, "{offset}: it settles");
        let at = |f: usize, k: &str| frames[f].1.get(k).copied();
        for f in 2..frames.len() {
            let spring = (frames[f].0 - frames[f - 1].0).abs();
            for (k, &(y, seen, _)) in &frames[f].1 {
                let (Some((y1, seen1, _)), Some((y0, _, _))) = (at(f - 1, k), at(f - 2, k)) else { continue };
                if seen < 0.5 && seen1 < 0.5 { continue; }
                let (v0, v) = (y1 - y0, y - y1);
                assert!(v.abs() <= spring + 0.5, "{offset}: {k} moves {v} pt in frame {f}, the fold {spring}");
                // Still in view, it doesn't go from moving to stopped (or turn back) in a frame.
                if seen > 0.5 && v0.abs() > 2.0 {
                    assert!(v * v0.signum() >= 0.5 * v0.abs(), "{offset}: {k} goes from {v0} to {v} pt a frame in frame {f}");
                }
            }
        }
        // Nothing travels 40 pt in plain view and then drops out of it in one frame where it rests.
        let last = &frames.last().unwrap().1;
        for (k, &(rest, _, _)) in last.iter().filter(|(k, _)| k.starts_with("step-")) {
            let mut travelled = 0.0;
            for f in 1..frames.len() {
                let (Some((y0, seen0, h)), Some((y, seen, _))) = (at(f - 1, k), at(f, k)) else { continue };
                if seen0 >= h - 0.5 { travelled += (y - y0).abs(); }
                assert!(!(travelled >= 40.0 && seen0 > h * 0.5 && seen < 0.5 && (y - rest).abs() < 0.5),
                    "{offset}: {k} travels {travelled} pt in view, then is cut from view where it rests in frame {f}");
            }
        }
    }
}

/// Shuts the fold from its stuck header as `shut_from_stuck` does, then `at` frames later (0:
/// in the same tick, before a frame draws) opens it again holding its top, and returns each
/// frame's tops of the header, the rows around the fold and every step, from the shut on. No
/// step ever shows above the header.
fn reopen_from_stuck(after: f32, offset: f32, at: usize) -> Vec<HashMap<String, f32>> {
    let mut rt = Runtime::new((true, after), held_section, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, offset));
    for _ in 0..4 { rt.frame(1.0 / 120.0); }
    rt.update(|s, tx| { s.0 = false; tx.keep_on_screen("hdr"); });
    let mut frames = Vec::new();
    for f in 0..at + 120 {
        if f == at { rt.update(|s, tx| { s.0 = true; tx.keep_on_screen("turn"); }); }
        rt.frame(1.0 / 120.0);
        let els = rt.elements();
        let hdr = els.iter().find(|e| e.key.as_deref() == Some("hdr")).unwrap().rect[1];
        for e in els.iter().filter(|e| e.key.as_deref().is_some_and(|k| k.starts_with("step-")) && e.visible[3] > 0.5) {
            assert!(e.visible[1] >= hdr - 0.5, "frame {f}: {:?} shows from {} above the header at {hdr}", e.key, e.visible[1]);
        }
        frames.push(els.into_iter().filter_map(|e| {
            let k = e.key.clone()?;
            (["hdr", "before", "after"].contains(&k.as_str()) || k.starts_with("step-")).then_some((k, e.rect[1]))
        }).collect());
    }
    assert!(!rt.animating, "it settles");
    let turn = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("turn")).unwrap();
    assert_eq!(turn.rect[3], 28.0 + 40.0 * 28.0, "it ends fully open");
    frames
}

/// The most anything with more than two points in view moved in one frame (a glide opening
/// past the window lands the rest once its edge is within a point or so), and the frame.
fn biggest_reopen_step(frames: &[HashMap<String, f32>]) -> (f32, usize, String) {
    let mut most = (0.0, 0, String::new());
    for (f, w) in frames.windows(2).enumerate() {
        for (k, &y1) in &w[1] {
            let Some(&y0) = w[0].get(k) else { continue };
            let seen = |y: f32| y > -26.0 && y < 198.0;
            if (seen(y0) || seen(y1)) && (y1 - y0).abs() > most.0 { most = ((y1 - y0).abs(), f + 1, k.clone()); }
        }
    }
    most
}

#[test]
fn a_fold_reopened_as_it_shuts_from_its_stuck_header_never_jumps() {
    // At the end (the header comes down once the end holds the row below) and mid-timeline,
    // opened again in the shut's own tick, while its contents ride up under the header, and
    // once the header is coming down: nothing in view moves more than a frame's spring, and
    // what was hidden above comes back above.
    for (after, label) in [(100.0, "at the end"), (300.0, "mid-timeline")] {
        for at in [0, 1, 3, 8, 16, 30] {
            let frames = reopen_from_stuck(after, 1100.0, at);
            let (most, f, key) = biggest_reopen_step(&frames);
            assert!(most < 24.0, "{label}, reopened {at} frames in: {key} moves {most} pt in frame {f}: {:?} -> {:?}",
                frames[f - 1].get(&key), frames[f].get(&key));
            let last = frames.last().unwrap();
            assert!(last.contains_key("step-0") && last["after"] > 0.0, "{label}, {at}: {last:?}");
        }
    }
}

#[test]
fn a_fold_shut_from_its_stuck_header_makes_no_jump_that_shows() {
    // What drops above the window drops in one frame, with the scroll offset by as much: the
    // fold's top and height step, but nothing that shows moves. Shut by a tap or by data, with
    // no input to excuse what the offset brings into view.
    for (after, tap) in [(100.0, true), (300.0, true), (100.0, false), (300.0, false)] {
        let mut run = crate::motion_gate::Run::new(Runtime::new((true, after), held_section, (300.0, 200.0), Env::new(true, 1.0)), 120.0);
        run.rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 1100.0));
        run.idle(0.1);
        run.step();
        let shut = |rt: &mut Runtime<(bool, f32)>| rt.update(|s, tx| { s.0 = false; tx.keep_on_screen("hdr"); });
        if tap { run.act("shut", shut) } else { shut(&mut run.rt); run.step() }
        run.play(0.75);
        let jumps: Vec<String> = run.finish().iter().filter(|f| ["jump", "jerk", "offset-jump"].contains(&f.kind)).map(|f| f.to_string()).collect();
        assert!(jumps.is_empty(), "after {after}, tap {tap}: {jumps:#?}");
    }
}

/// Where `below` is drawn on each of the first `n` frames at 120 Hz after the middle row (a
/// fold, or a plain row) leaves the stack.
fn after_removal(fold: bool, n: usize) -> Vec<f32> {
    fn view(s: &(bool, bool), _: &mut Cx<(bool, bool)>) -> El {
        let (present, fold) = *s;
        let mut rows = vec![el().key("above").h(100.0).w_full()];
        if present {
            let row = el().key("group").h(40.0).w_full();
            rows.push(if fold { row.fold_glide(rune_motion::Spring::new(0.3, 0.0)) } else { row });
        }
        rows.push(el().key("below").h(100.0).w_full());
        col().key("root").child(col().key("stack").growth(GrowthSpec::default()).children(rows))
    }
    let mut rt = Runtime::new((true, fold), view, (400.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.0 = false;
    rt.dirty = true;
    (0..n).map(|_| {
        rt.frame(1.0 / 120.0);
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("below")).unwrap().rect[1]
    }).collect()
}

#[test]
fn a_fold_glide_row_that_leaves_shuts_at_once() {
    let fold = after_removal(true, 12);
    assert!(fold[1] < 140.0 - 0.5, "the fold starts shutting within two frames: {fold:?}");
    assert!(fold.windows(2).all(|w| w[1] <= w[0]), "and shuts monotonically: {fold:?}");
    let plain = after_removal(false, 12);
    assert!(plain.iter().all(|&y| (y - 140.0).abs() < 1e-3), "a plain row still holds its space while it fades: {plain:?}");
}

/// Each frame at 120 Hz after rows `r0`–`r2` (20 pt folds) come into or leave the stack
/// together: their drawn heights, and where `below` is drawn.
fn block(opening: bool, frames: usize) -> Vec<([f32; 3], f32)> {
    fn view(present: &bool, _: &mut Cx<bool>) -> El {
        let mut rows = vec![el().key("above").h(100.0).w_full()];
        if *present {
            rows.extend((0..3).map(|i| el().key(format!("r{i}")).h(20.0).w_full().fold_glide(rune_motion::Spring::new(0.3, 0.0))));
        }
        rows.push(el().key("below").h(100.0).w_full());
        col().key("root").child(col().key("stack").growth(GrowthSpec::default()).children(rows))
    }
    let mut rt = Runtime::new(!opening, view, (400.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state = opening;
    rt.dirty = true;
    (0..frames).map(|_| {
        rt.frame(1.0 / 120.0);
        let els = rt.elements();
        let h = |k: &str| els.iter().find(|e| e.key.as_deref() == Some(k)).map_or(0.0, |e| e.rect[3]);
        ([h("r0"), h("r1"), h("r2")], els.iter().find(|e| e.key.as_deref() == Some("below")).unwrap().rect[1])
    }).collect()
}

#[test]
fn fold_rows_that_open_together_fill_top_to_bottom() {
    let frames = block(true, 90);
    for (k, (hs, _)) in frames.iter().enumerate() {
        let part = hs.iter().filter(|&&h| h > 0.01 && h < 19.99).count();
        assert!(part <= 1, "frame {k}: one row at the edge at a time: {hs:?}");
        assert!(hs.windows(2).all(|w| w[1] <= 0.01 || w[0] >= 19.99), "frame {k}: a row opens once the one above is open: {hs:?}");
    }
    assert!(frames.windows(2).all(|w| w[1].1 >= w[0].1), "below moves down, never back: {frames:?}");
    assert!(frames[8].0[0] > 19.99 && frames[8].0[2] < 0.01, "a third in, the top row is open and the last still shut: {:?}", frames[8]);
    assert_eq!(frames.last().unwrap(), &([20.0; 3], 160.0));
}

#[test]
fn fold_rows_that_leave_together_empty_bottom_to_top() {
    let frames = block(false, 90);
    for (k, (hs, _)) in frames.iter().enumerate() {
        let part = hs.iter().filter(|&&h| h > 0.01 && h < 19.99).count();
        assert!(part <= 1, "frame {k}: one row at the edge at a time: {hs:?}");
    }
    assert!(frames.windows(2).all(|w| w[1].1 <= w[0].1), "below moves up, never back: {frames:?}");
    assert!(frames[1].1 < 160.0, "the block starts shutting at once: {:?}", frames[1]);
    assert!((frames.last().unwrap().1 - 100.0).abs() < 1e-3);
}
