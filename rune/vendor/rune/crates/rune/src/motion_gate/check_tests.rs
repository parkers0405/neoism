use super::*;

fn rec(tracks: Vec<(&str, Vec<(usize, [f32; 4], f32)>)>, frames: usize) -> Recording {
    let mut r = Recording::new((1000.0, 1000.0), 120.0);
    r.frames = frames;
    for (path, fs) in tracks {
        let i = r.track(path, None);
        r.tracks[i].frames = fs.into_iter().map(|(k, rect, op)| (k, Snap { rect, vis: rect, op, scale: 1.0, flags: INK, phase: f32::NAN, scrolled: [0.0; 2] })).collect();
    }
    r
}

fn kinds(r: &Recording) -> Vec<&'static str> {
    check(r).into_iter().map(|f| f.kind).collect()
}

#[test]
fn a_spring_passes_and_a_snap_jumps() {
    let smooth: Vec<_> = (0..60).map(|k| { let p = 1.0 - (-(k as f32) / 8.0).exp(); (k, [0.0, 300.0 - 200.0 * p, 100.0, 40.0], 1.0) }).collect();
    assert!(kinds(&rec(vec![("a", smooth)], 60)).is_empty());
    let snap: Vec<_> = (0..60).map(|k| (k, [0.0, if k < 30 { 300.0 } else { 100.0 }, 100.0, 40.0], 1.0)).collect();
    assert_eq!(kinds(&rec(vec![("a", snap)], 60)), ["jump"]);
    // Gliding at 4 pt a frame, then lurching to 20.
    let mut y = 0.0;
    let lurch: Vec<_> = (0..60).map(|k| { if k > 0 && k < 40 { y += if k < 20 { 4.0 } else { 20.0 } } (k, [0.0, y, 100.0, 40.0], 1.0) }).collect();
    assert_eq!(kinds(&rec(vec![("a", lurch)], 60)), ["jerk"]);
}

/// Climbing 3 pt per word, a word every 0.3 s: as kicks that glide out and rest, or one glide.
#[test]
fn a_climb_in_kicks_is_stop_start_and_a_steady_one_passes() {
    let n = 600;
    // Drawn rects move in 1/128 pt steps.
    let path = |y: fn(usize) -> f32| -> Vec<_> { (0..n).map(|k| (k, [0.0, ((500.0 - y(k)) * 128.0).round() / 128.0, 100.0, 40.0], 1.0)).collect() };
    let mut kicks = rec(vec![("reply", path(|k| 3.0 * (k / 36) as f32 + 3.0 * (1.0 - (-((k % 36) as f32) / 3.0).exp())))], n);
    for k in (0..n).step_by(36) { kicks.inputs.push((k, "event".into())); }
    assert!(kinds(&kicks).contains(&"stop-start"), "{:?}", kinds(&kicks));
    let steady = rec(vec![("reply", path(|k| k.min(480) as f32 / 12.0))], n);
    assert!(kinds(&steady).is_empty(), "{:?}", kinds(&steady));
    // The last of a glide creeping in under a pixel a frame rests between steps, but isn't kicking.
    let creep = rec(vec![("reply", path(|k| 3.0 - 3.0 * (-(k as f32) / 240.0).exp()))], n);
    assert!(!kinds(&creep).contains(&"stop-start"), "{:?}", kinds(&creep));
    // Each kick answering a hand on the app is that hand's.
    for i in &mut kicks.inputs { i.1 = "tap".into(); }
    assert!(!kinds(&kicks).contains(&"stop-start"));
}

#[test]
fn an_instant_toggle_is_reported_where_the_height_changed() {
    let group: Vec<_> = (0..60).map(|k| (k, [0.0, 100.0, 100.0, if k < 30 { 28.0 } else { 84.0 }], 1.0)).collect();
    let below: Vec<_> = (0..60).map(|k| (k, [0.0, if k < 30 { 128.0 } else { 184.0 }, 100.0, 40.0], 1.0)).collect();
    let mut r = rec(vec![("g", group), ("b", below)], 60);
    assert_eq!(kinds(&r), ["jump", "jump"], "with no input, the row below jumps on its own");
    r.inputs.push((30, "tap".into()));
    let f = check(&r);
    assert_eq!(f.iter().map(|f| (f.kind, f.element.as_str())).collect::<Vec<_>>(), [("jump", "g")]);
}

#[test]
fn pops_overshoot_reversal_and_flicker() {
    let pop: Vec<_> = (20..60).map(|k| (k, [0.0, 0.0, 100.0, 40.0], 1.0)).collect();
    assert_eq!(kinds(&rec(vec![("a", pop)], 60)), ["pop-in"]);
    // A row mounting a sliver tall that opens slowly grows in.
    let opens: Vec<_> = (20..60).map(|k| (k, [0.0, 0.0, 100.0, 0.4 + 0.1 * (k - 20) as f32], 1.0)).collect();
    assert!(kinds(&rec(vec![("a", opens)], 60)).is_empty());
    let gone: Vec<_> = (0..20).map(|k| (k, [0.0, 0.0, 100.0, 40.0], 1.0)).collect();
    assert_eq!(kinds(&rec(vec![("a", gone)], 60)), ["pop-out"]);
    let over: Vec<_> = (0..60).map(|k| { let t = k as f32 / 8.0; (k, [0.0, 100.0 + 100.0 * (1.0 - (-t).exp() * (t * 1.5).cos()), 100.0, 40.0], 1.0) }).collect();
    let k = kinds(&rec(vec![("a", over)], 60));
    assert!(k.contains(&"overshoot"), "{k:?}");
    let dogleg: Vec<_> = (0..60).map(|k| { let t = k as f32 / 59.0; (k, [0.0, 100.0 - 40.0 * (t * 3.0).min(1.0) * (1.0 - t) * 4.0 + 200.0 * t * t, 100.0, 40.0], 1.0) }).collect();
    let k = kinds(&rec(vec![("a", dogleg)], 60));
    assert!(k.contains(&"reversal"), "{k:?}");
    let blink: Vec<_> = (0..60).map(|k| (k, [0.0, 0.0, 100.0, 40.0], if (20..24).contains(&k) { 0.3 } else { 1.0 })).collect();
    assert_eq!(kinds(&rec(vec![("a", blink.clone())], 60)), ["flicker"]);
    // A container whose contents are all hidden draws nothing as it dips.
    let mut r = rec(vec![("a", blink)], 60);
    for f in &mut r.tracks[0].frames { f.1.flags = 0; }
    assert!(kinds(&r).is_empty());
}

#[test]
fn words_that_swap_in_place_are_a_text_swap_but_a_ticking_count_is_not() {
    let still: Vec<(usize, [f32; 4], f32)> = (0..60).map(|k| (k, [0.0, 0.0, 100.0, 20.0], 1.0)).collect();
    let with = |words: &[(usize, &str)]| {
        let mut r = rec(vec![("a", still.clone())], 60);
        r.tracks[0].words = words.iter().map(|(k, w)| (*k, w.to_string())).collect();
        kinds(&r)
    };
    assert_eq!(with(&[(0, "Read backoff.ts"), (30, "Grep retryBudget")]), ["text-swap"]);
    assert!(with(&[(0, "9s"), (30, "10s")]).is_empty(), "a clock ticks");
    assert!(with(&[(0, "Looks"), (30, "Looks right")]).is_empty(), "streaming words grow");
    // A streaming link's box widens with its next word.
    let link: Vec<_> = (0..60).map(|k| (k, [0.0, 0.0, if k < 30 { 40.0 } else { 85.0 }, 20.0], 1.0)).collect();
    let mut r = rec(vec![("a", link)], 60);
    assert_eq!(kinds(&r), ["jump"]);
    r.tracks[0].words = vec![(0, "health".into()), (30, "health check".into())];
    assert!(kinds(&r).is_empty(), "{:?}", kinds(&r));
}

#[test]
fn a_loop_that_restarts_jumps_and_a_finished_run_goes_quiet() {
    let still: Vec<(usize, [f32; 4], f32)> = (0..120).map(|k| (k, [0.0, 0.0, 100.0, 20.0], 1.0)).collect();
    let looping = |restart: Option<usize>| {
        let mut r = rec(vec![("a", still.clone())], 120);
        for (k, f) in r.tracks[0].frames.iter_mut() {
            let t = restart.filter(|&at| *k >= at).map_or(*k, |at| *k - at);
            f.phase = (t as f32 / 144.0).rem_euclid(1.0);
        }
        r
    };
    // Wrapping round from 1 to 0 is a usual step.
    assert!(kinds(&looping(None)).is_empty());
    assert_eq!(kinds(&looping(Some(60))), ["loop-jump"]);
    let mut r = rec(vec![("a", (0..300).map(|k| (k, [0.0, 0.0, 100.0, 20.0], 1.0)).collect())], 300);
    r.quiet_from = Some(100);
    r.awake = (0..300).map(|k| k < 180).collect();
    assert!(kinds(&r).is_empty(), "settling within a second is fine");
    r.awake[260] = true;
    assert_eq!(kinds(&r), ["wakes-after-end"]);
}

#[test]
fn a_row_that_loses_its_key_remounts_but_one_that_becomes_another_element_does_not() {
    let fade = |from: usize, to: usize, out: bool| -> Vec<(usize, [f32; 4], f32)> {
        (from..to).map(|k| (k, [0.0, 0.0, 300.0, 20.0], if out && k >= 30 { 1.0 - (k - 29) as f32 / 10.0 } else if !out { ((k - 29) as f32 / 10.0).min(1.0) } else { 1.0 })).collect()
    };
    let found = |old: &str, new: &str| {
        let mut r = rec(vec![(old, fade(0, 40, true)), (new, fade(30, 60, false))], 60);
        for f in r.tracks[0].frames.iter_mut().filter(|f| f.0 >= 30) { f.1.flags |= EXITING; }
        for t in &mut r.tracks { t.text = Some("Got it, I'll look.".into()); }
        kinds(&r).contains(&"remount")
    };
    assert!(found("list/row-0/label", "list/row-1/label"));
    assert!(!found("tray/pending/label", "list/row/text"));
}

#[test]
fn a_pinned_scroll_that_stops_following_is_unpinned_unless_someone_scrolled() {
    let view: Vec<_> = (0..60).map(|k| (k, [0.0, 0.0, 100.0, 200.0], 1.0)).collect();
    let grows = |follows: bool| -> Vec<(usize, [f32; 4], f32)> {
        (0..60usize).map(|k| { let h = 400.0 + k.saturating_sub(30) as f32; (k, [0.0, if follows { 200.0 - h } else { -200.0 }, 100.0, h], 1.0) }).collect()
    };
    let unpinned = |follows: bool, input: Option<&str>| {
        let mut r = rec(vec![("scroll", view.clone()), ("scroll/content", grows(follows))], 60);
        r.tracks[1].parent = Some(0);
        for f in &mut r.tracks[1].frames { f.1.vis = [0.0, 0.0, 100.0, 200.0]; }
        r.inputs.extend(input.map(|what| (31, what.to_owned())));
        kinds(&r).contains(&"unpinned")
    };
    assert!(!unpinned(true, None));
    assert!(unpinned(false, None));
    assert!(unpinned(false, Some("event (3 left)")), "data arriving isn't a hand on the scroll");
    assert!(!unpinned(false, Some("scroll up")));
}

#[test]
fn a_second_sibling_under_one_key_shows_twice() {
    let row = |y: f32| -> Vec<(usize, [f32; 4], f32)> { (0..60).map(|k| (k, [0.0, y, 100.0, 20.0], 1.0)).collect() };
    assert!(kinds(&rec(vec![("list/row-7", row(0.0))], 60)).is_empty());
    assert_eq!(kinds(&rec(vec![("list/row-7", row(0.0)), ("list/row-7#dup1", row(20.0))], 60)), ["twice"]);
}

#[test]
fn a_flight_that_lands_under_a_clip_lands_unseen() {
    let flight: Vec<_> = (0..60).map(|k| {
        let t = (k as f32 / 30.0).min(1.0);
        (k, [0.0, 100.0 + 300.0 * t * t * (3.0 - 2.0 * t), 100.0, 40.0], 1.0)
    }).collect();
    let landed = |clipped: bool, moved: bool| {
        let fs = if moved { flight.clone() } else { (0..60).map(|k| (k, [0.0, 400.0, 100.0, 40.0], 1.0)).collect() };
        let mut r = rec(vec![("flight", fs)], 60);
        for f in r.tracks[0].frames.iter_mut().filter(|f| clipped && f.0 >= 40) { f.1.vis = [0.0, 0.0, 0.0, 0.0]; }
        kinds(&r)
    };
    assert!(landed(false, true).is_empty(), "{:?}", landed(false, true));
    assert_eq!(landed(true, true), ["lands-unseen"]);
    assert!(landed(true, false).is_empty(), "a still element a clip covers didn't fly there");
}

#[test]
fn where_counts_roll_a_ticking_count_is_a_text_swap_but_a_rolling_label_is_not() {
    let still: Vec<(usize, [f32; 4], f32)> = (0..60).map(|k| (k, [0.0, 0.0, 100.0, 20.0], 1.0)).collect();
    let mut r = rec(vec![("count", still.clone())], 60);
    r.tracks[0].words = vec![(0, "9s".into()), (30, "10s".into())];
    assert!(kinds(&r).is_empty());
    r.counts_roll = true;
    assert_eq!(kinds(&r), ["text-swap"], "a clock that snaps instead of rolling");
    // The label over its rolling glyphs reads "9s" then "10s" as its digits roll.
    let glyph = r.track("count/digit-1", Some(0));
    r.tracks[glyph].frames = (0..60).map(|k| (k, Snap { rect: [0.0, 0.0, 8.0, 20.0], vis: [0.0, 0.0, 8.0, 20.0], op: 1.0, scale: 1.0, flags: INK, phase: f32::NAN, scrolled: [0.0; 2] })).collect();
    assert!(kinds(&r).is_empty());
}

#[test]
fn motion_inside_a_scroll_is_judged_against_the_scroll() {
    // A list follows its growing end, carrying its content up 2 pt a frame, while a node in it
    // moves on its own. `scrolled` is how far the scroll carries the node.
    let offset = |k: usize| 2.0 * k as f32;
    let run = |own: &dyn Fn(usize) -> f32, flags: u8, told: bool| {
        let fs: Vec<_> = (0..60).map(|k| (k, [0.0, 300.0 + own(k) - offset(k), 100.0, 40.0], 1.0)).collect();
        let mut r = rec(vec![("list/node", fs)], 60);
        for (k, f) in &mut r.tracks[0].frames {
            f.flags |= flags;
            if told { f.scrolled = [0.0, offset(*k)]; }
        }
        kinds(&r)
    };
    // Gliding down 60 pt: fast at first, so on screen it falls, then rises with the scroll.
    let glide = |k: usize| 60.0 * (1.0 - (-(k as f32) / 8.0).exp());
    assert_eq!(run(&glide, 0, false), ["reversal"], "on screen the glide and the scroll turn back");
    assert!(run(&glide, 0, true).is_empty(), "against the scroll it is one glide");
    assert!(run(&glide, SCROLLING, true).is_empty());
    // Wobbling in place: on screen the scroll's 2 pt a frame hides the turns.
    let wobble = |k: usize| 2.5 * (k as f32 * 0.6).sin();
    assert!(run(&wobble, 0, false).is_empty(), "on screen it only ever rises");
    assert_eq!(run(&wobble, 0, true), ["jiggle"], "against the scroll it wobbles");
    assert_eq!(run(&wobble, SCROLLING, true), ["jiggle"], "a scroll that moves doesn't excuse it");
}

/// A list's content, scrolled into a 400 pt viewport at the window's top, and rows in it: each
/// track's rect per frame, its visible part the rect cut to the viewport.
fn scrolled_list(tracks: Vec<(&str, Vec<[f32; 4]>)>) -> Recording {
    let mut r = Recording::new((1000.0, 1000.0), 120.0);
    r.frames = 60;
    for (path, rects) in tracks {
        let parent = path.rsplit_once('/').map(|(p, _)| r.tracks.iter().position(|t| t.path == p).unwrap());
        let i = r.track(path, parent);
        r.tracks[i].frames = rects.into_iter().enumerate().map(|(k, rect)| {
            let (y0, y1) = (rect[1].max(0.0), (rect[1] + rect[3]).min(400.0));
            let vis = [rect[0], y0, rect[2], (y1 - y0).max(0.0)];
            (k, Snap { rect, vis, op: 1.0, scale: 1.0, flags: INK, phase: f32::NAN, scrolled: [0.0, 600.0] })
        }).collect();
    }
    r
}

#[test]
fn a_jump_is_what_shows_jumping() {
    let at = |k: usize, a: [f32; 4], b: [f32; 4]| if k < 30 { a } else { b };
    let still = |rect: [f32; 4]| vec![rect; 60];
    // A row teleported 100 pt while it shows, the list still.
    let list = still([0.0, -600.0, 100.0, 2000.0]);
    let row: Vec<_> = (0..60).map(|k| at(k, [0.0, 100.0, 100.0, 40.0], [0.0, 200.0, 100.0, 40.0])).collect();
    assert_eq!(kinds(&scrolled_list(vec![("list", list.clone()), ("list/row", row)])), ["jump"]);
    // A fold drops 900 pt above the window with the offset: the content's top and height step,
    // its visible part and the rows that show stay put.
    let content: Vec<_> = (0..60).map(|k| at(k, [0.0, -1100.0, 100.0, 1500.0], [0.0, -200.0, 100.0, 600.0])).collect();
    let fold: Vec<_> = (0..60).map(|k| at(k, [0.0, -900.0, 100.0, 1200.0], [0.0, 0.0, 100.0, 300.0])).collect();
    let shown = |y: f32| still([0.0, y, 100.0, 40.0]);
    let r = scrolled_list(vec![("list", content), ("list/fold", fold), ("list/fold/a", shown(100.0)), ("list/fold/b", shown(200.0)), ("list/c", shown(320.0))]);
    assert!(kinds(&r).is_empty(), "{:?}", check(&r));
    // The content jumps 100 pt and the rows that show jump with it: one offset jump.
    let content: Vec<_> = (0..60).map(|k| at(k, [0.0, -600.0, 100.0, 2000.0], [0.0, -500.0, 100.0, 2000.0])).collect();
    let row = |y: f32| (0..60).map(|k| at(k, [0.0, y, 100.0, 40.0], [0.0, y + 100.0, 100.0, 40.0])).collect::<Vec<_>>();
    assert_eq!(kinds(&scrolled_list(vec![("list", content), ("list/a", row(0.0)), ("list/b", row(100.0)), ("list/c", row(200.0))])), ["offset-jump"]);
}
