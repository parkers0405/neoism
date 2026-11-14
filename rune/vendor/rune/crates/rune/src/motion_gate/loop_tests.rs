use super::*;
use crate::{Beat, Cx, El, Env, GrowthSpec, ScrollSpec, Sequence, col, el, oklch};

fn kinds(rec: &Recording) -> Vec<&'static str> {
    check(rec).into_iter().map(|f| f.kind).collect()
}

#[derive(Default)]
struct Badge {
    shown: bool,
    right: bool,
}

const FADE_IN: &[Beat] = &[Beat::new(0.0).opacity(0.0), Beat::new(0.2).easing(rune_motion::Easing::EASE_OUT)];

fn badge(s: &Badge, _: &mut Cx<Badge>) -> El {
    let b = el().key("badge").size(100.0, 40.0).fill(oklch(0.6, 0.1, 250.0)).sequence(Sequence::new(FADE_IN));
    let b = if s.right { b.ml(200.0) } else { b };
    col().w_full().h_full().children(s.shown.then_some(b))
}

#[test]
fn a_one_shot_fade_that_settles_is_not_a_loop() {
    let mut r = Run::new(Runtime::new(Badge::default(), badge, (400.0, 200.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    r.act("show", |rt| rt.update(|s, _| s.shown = true));
    r.play(1.0);
    assert!(kinds(&r.rec).is_empty(), "{:?}", check(&r.rec));
    // Nor does it hide what the element does after: a snap is still a jump.
    r.act("move", |rt| rt.update(|s, tx| { s.right = true; tx.instant(); }));
    r.play(0.5);
    assert_eq!(kinds(&r.rec), ["jump"], "{:?}", check(&r.rec));
}

#[derive(Default)]
struct Spinner {
    on: bool,
}

fn spinner(s: &Spinner, _: &mut Cx<Spinner>) -> El {
    let spin = el().key("spin").size(20.0, 20.0).fill(oklch(0.6, 0.1, 250.0)).looping(rune_motion::LoopSpec::rotation(1.0));
    col().w_full().h_full().children(s.on.then_some(spin))
}

/// A spinner turning, then, with `restart`, gone for a frame and back: the new one starts its
/// turn again from the top.
fn spin(restart: bool) -> Recording {
    let mut r = Run::new(Runtime::new(Spinner { on: true }, spinner, (200.0, 200.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    r.play(0.6);
    if restart {
        r.act("hide", |rt| rt.update(|s, tx| { s.on = false; tx.instant(); }));
        r.act("show", |rt| rt.update(|s, tx| { s.on = true; tx.instant(); }));
    }
    r.play(0.6);
    r.rec
}

#[test]
fn a_spinner_keeps_its_rhythm_unless_it_restarts() {
    assert!(!kinds(&spin(false)).contains(&"loop-jump"), "{:?}", check(&spin(false)));
    assert!(kinds(&spin(true)).contains(&"loop-jump"), "{:?}", check(&spin(true)));
}

struct Timeline {
    open: bool,
    rows: usize,
    /// Bumped every frame, so the view runs on each, as an app's clock or stream does.
    tick: u32,
}

/// A pinned list of 40 pt rows, its 56th a fold: a 28 pt header over 40 steps of 28 pt when
/// open, in a 300 pt viewport.
fn timeline(s: &Timeline, _: &mut Cx<Timeline>) -> El {
    let open = s.open;
    let rows = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(s.rows, 40.0,
        |_: &Timeline, i| format!("r{i}").into(),
        move |_: &Timeline, i| {
            let ink = oklch(0.6, 0.1, 250.0);
            if i != 55 { return el().w_full().h(40.0).fill(ink); }
            let mut group = col().key("group").w_full().child(el().key("hdr").w_full().h(28.0).fill(ink));
            if open {
                group = group.child(col().key("steps").w_full().children((0..40).map(|k| el().key(format!("step-{k}")).w_full().h(28.0).fill(ink))));
            }
            col().w_full().fold_glide(crate::Spring::new(0.18, 0.0)).child(group)
        });
    col().key("scroll").size(400.0, 300.0).scroll(ScrollSpec { stick_bottom: true }).child(rows)
}

/// Taps the fold, holding each state for `toggles[i]` seconds, then runs `last` if any.
fn fold(toggles: &[f64], last: Option<fn(&mut Timeline, &mut crate::Tx)>) -> Recording {
    let mut r = Run::new(Runtime::new(Timeline { open: false, rows: 60, tick: 0 }, timeline, (400.0, 300.0), Env::new(true, 1.0)), 120.0);
    r.idle(1.0);
    r.step();
    let play = |r: &mut Run<Timeline>, secs: f64| {
        for _ in 0..(secs * 120.0).round() as usize {
            r.rt.update(|s, _| s.tick += 1);
            r.step();
        }
    };
    for &secs in toggles {
        r.act("tap", |rt| rt.update(|s, tx| { s.open = !s.open; tx.keep_on_screen("hdr"); }));
        play(&mut r, secs);
    }
    if let Some(last) = last {
        r.act("remove", |rt| rt.update(last));
        play(&mut r, 0.5);
    }
    r.rec
}

#[test]
fn rows_under_a_fold_gliding_open_stay_mounted_while_they_show() {
    // The open fold is taller than the overscan, so the next window ends at it, while the rows
    // under it still show as it glides open.
    let rec = fold(&[1.0], None);
    assert!(!kinds(&rec).contains(&"pop-out"), "{:?}", check(&rec));
}

#[test]
fn a_list_toggled_over_and_over_is_not_a_loop() {
    // Its height turns back at each tap; that answers the taps, so what goes wrong in it shows.
    let rec = fold(&[0.3, 0.3, 0.3, 0.3, 0.3, 0.06, 0.5, 0.8], Some(|s, tx| { s.rows = 59; tx.instant(); }));
    let found = check(&rec);
    assert!(found.iter().any(|f| f.kind == "pop-out" && f.element.ends_with("r59")), "{found:?}");
}


