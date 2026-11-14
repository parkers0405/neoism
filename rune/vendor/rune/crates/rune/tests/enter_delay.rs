//! `El::enter_delay`: a new element holds its whole insertion effect, then offset and opacity
//! play on one progress. Waiting draws nothing and asks for one wake; an entrance nobody would
//! see ignores the delay.
use rune::{Cx, El, Env, Runtime, Stagger, Transition, col, el};

const RISE: f32 = 12.0;
const HZ: f64 = 120.0;

#[derive(Default)]
struct S {
    rows: usize,
    delay: f32,
    own_scale: f32,
    window_scale: f32,
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    if s.window_scale > 0.0 { cx.time_scale(s.window_scale); }
    let stagger = Stagger::new(s.delay);
    let rows = (0..s.rows).map(|i| {
        let mut row = el()
            .key(format!("row{i}"))
            .size(100.0, 20.0)
            .fill(rune::Color::BLACK)
            .transition(Transition::offset(0.0, RISE).combined(Transition::opacity()))
            .enter_delay(stagger.start(s.delay).delay(i, s.rows));
        if s.own_scale > 0.0 { row = row.time_scale(s.own_scale); }
        row
    });
    col().w_full().h_full().children(rows)
}

fn app(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

/// (offset progress, opacity) of row `i`, or None once it's gone.
fn row(rt: &Runtime<S>, i: usize) -> Option<(f32, f32)> {
    let key = format!("row{i}");
    let e = rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key.as_str()))?;
    Some((1.0 - (e.rect[1] - e.target[1]) / RISE, e.opacity))
}

#[test]
fn waits_whole_then_moves_offset_and_opacity_in_lockstep() {
    let mut rt = app(S { delay: 0.2, ..S::default() });
    rt.update(|s, _| s.rows = 1);
    let mut t = 0.0;
    let mut moved = false;
    while t < 1.5 {
        rt.frame(1.0 / HZ);
        t += 1.0 / HZ;
        let (offset, opacity) = row(&rt, 0).unwrap();
        if t < 0.2 - 1e-6 {
            assert_eq!((offset, opacity), (0.0, 0.0), "t={t:.3}: waits at the insertion effect");
        } else {
            assert!((offset - opacity).abs() < 0.02, "t={t:.3}: offset {offset} and opacity {opacity}");
            moved |= opacity > 0.5;
        }
    }
    assert!(moved);
    assert_eq!(row(&rt, 0), Some((1.0, 1.0)));
}

#[test]
fn waiting_sleeps_until_the_start() {
    let mut rt = app(S { delay: 0.5, ..S::default() });
    rt.update(|s, _| s.rows = 1);
    assert!(!rt.frame(1.0 / HZ), "waiting draws nothing new");
    let wake = rt.wake_deadline().expect("a wake for the start");
    assert!((wake - 0.5).abs() < 1.5 / HZ, "wakes when it starts: {wake}");
    // A host that sleeps until then gets one frame there, the start.
    rt.frame(wake - 1.0 / HZ);
    assert_eq!(row(&rt, 0).unwrap().1, 0.0);
    assert!(rt.frame(0.1), "the entrance runs");
    assert!(row(&rt, 0).unwrap().1 > 0.0);
}

#[test]
fn time_scales_stretch_the_delay() {
    for (own_scale, window_scale) in [(0.5, 0.0), (0.0, 0.5)] {
        let mut rt = app(S { delay: 0.2, own_scale, window_scale, ..S::default() });
        rt.update(|s, _| s.rows = 1);
        rt.frame(0.38);
        assert_eq!(row(&rt, 0).unwrap().1, 0.0, "{own_scale} {window_scale}: still waits at 0.38 s");
        rt.frame(0.1);
        assert!(row(&rt, 0).unwrap().1 > 0.0, "{own_scale} {window_scale}: enters after 0.4 s");
    }
}

#[test]
fn removed_while_waiting_doesnt_flash() {
    let mut rt = app(S { delay: 0.3, ..S::default() });
    rt.update(|s, _| s.rows = 1);
    rt.frame(0.1);
    rt.update(|s, _| s.rows = 0);
    for _ in 0..120 {
        rt.frame(1.0 / HZ);
        let Some((_, opacity)) = row(&rt, 0) else { return };
        assert_eq!(opacity, 0.0, "no flash");
    }
    panic!("never left");
}

#[test]
fn unseen_entrances_ignore_the_delay() {
    // The first frame.
    let rt = app(S { rows: 1, delay: 0.3, ..S::default() });
    assert_eq!(row(&rt, 0), Some((1.0, 1.0)));
    // An instant commit.
    let mut rt = app(S { delay: 0.3, ..S::default() });
    rt.update(|s, tx| {
        s.rows = 1;
        tx.instant();
    });
    rt.frame(1.0 / HZ);
    assert_eq!(row(&rt, 0), Some((1.0, 1.0)));
    // A settle while it waits.
    let mut rt = app(S { delay: 0.3, ..S::default() });
    rt.update(|s, _| s.rows = 1);
    rt.frame(0.1);
    rt.settle_all();
    assert!(!rt.frame(1.0 / HZ));
    assert_eq!(row(&rt, 0), Some((1.0, 1.0)));
}

#[test]
fn reduce_motion_keeps_the_delay_and_only_fades() {
    let mut rt = app(S { delay: 0.3, ..S::default() });
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, _| s.rows = 1);
    let mut t = 0.0;
    let mut shown = None;
    while t < 1.0 {
        rt.frame(1.0 / HZ);
        t += 1.0 / HZ;
        let (offset, opacity) = row(&rt, 0).unwrap();
        assert_eq!(offset, 1.0, "t={t:.3}: nothing travels");
        if t < 0.3 - 1e-6 { assert_eq!(opacity, 0.0, "t={t:.3}: the fade waits"); }
        if opacity >= 1.0 && shown.is_none() { shown = Some(t); }
    }
    let shown = shown.unwrap();
    assert!(shown > 0.3 && shown < 0.6, "the short fade, after the delay: {shown}");
}

#[test]
fn a_staggered_list_of_five_enters_in_order() {
    let mut rt = app(S { delay: 0.05, ..S::default() });
    rt.update(|s, _| s.rows = 5);
    let mut starts = [None; 5];
    let mut t = 0.0;
    while t < 1.5 {
        rt.frame(1.0 / HZ);
        t += 1.0 / HZ;
        for (i, start) in starts.iter_mut().enumerate() {
            let (offset, opacity) = row(&rt, i).unwrap();
            assert!((offset - opacity).abs() < 0.02, "row {i} t={t:.3}: offset {offset}, opacity {opacity}");
            if opacity > 0.0 && start.is_none() { *start = Some(t); }
        }
    }
    let starts = starts.map(Option::unwrap);
    for (i, w) in starts.windows(2).enumerate() {
        assert!((w[1] - w[0] - 0.05).abs() < 1.5 / HZ, "row {} starts 50 ms after row {i}: {starts:?}", i + 1);
    }
}
