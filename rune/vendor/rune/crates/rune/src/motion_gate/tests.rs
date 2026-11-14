use super::*;
use crate::{Cx, El, Env, Spring, col, el, oklch};

#[derive(Default)]
struct S {
    right: bool,
    shown: bool,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let ink = oklch(0.6, 0.15, 25.0);
    let mover = el().key("mover").size(100.0, 40.0).fill(ink).motion(Spring::new(0.3, 0.0));
    let mover = if s.right { mover.ml(300.0) } else { mover };
    let mut page = col().w_full().h_full().child(mover);
    if s.shown {
        page = page.child(el().key("box").size(100.0, 40.0).fill(ink));
    }
    page
}

fn cell(change: fn(&mut S, &mut crate::Tx)) -> Recording {
    let mut r = Run::new(Runtime::new(S::default(), view, (600.0, 400.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    r.act("change", |rt| rt.update(change));
    r.play(1.0);
    r.rec
}

fn kinds(rec: &Recording) -> Vec<&'static str> {
    check(rec).into_iter().map(|f| f.kind).collect()
}

#[test]
fn a_run_records_a_spring_as_clean_and_a_snap_as_a_jump() {
    let spring = cell(|s, _| s.right = true);
    assert_eq!(spring.frames, 122, "the settled start, the input's frame and a second");
    assert!(kinds(&spring).is_empty(), "{:?}", check(&spring));
    let snap = cell(|s, tx| {
        s.right = true;
        tx.instant();
    });
    assert_eq!(kinds(&snap), ["jump"], "{:?}", check(&snap));
}

#[test]
fn a_box_that_mounts_with_no_transition_pops_in_unless_allowed() {
    let rec = cell(|s, tx| {
        s.shown = true;
        tx.instant();
    });
    let findings = check(&rec);
    assert!(findings.iter().any(|f| f.kind == "pop-in" && f.element.contains("box")), "{findings:?}");
    let cells = [Cell { name: "show/wide".into(), frames: rec.frames, skipped: false, findings }];
    let open = report(&cells, &[], false, 0.0);
    assert!(open.failing > 0 && open.text.contains("pop-in") && open.text.contains("mounts at or above"));
    const ALLOW: &[Allow] = &[("pop-in", "box", "show/", "the box appears at once, by design")];
    let allowed = report(&cells, ALLOW, true, 0.0);
    assert_eq!((allowed.failing, allowed.allowed), (0, 1));
    assert!(allowed.text.contains("allowed") && allowed.text.contains("by design"));
    assert_eq!(report(&cells, &[("pop-in", "box", "narrow", "")], false, 0.0).failing, 1, "the cell must match too");
}

#[test]
fn allow_rows_cap_what_they_cover_and_report_when_unused() {
    let f = |el: &str| Finding { kind: "flash", element: el.into(), frame: 3, detail: String::new() };
    let cells = [
        Cell { name: "island-auto/dark".into(), frames: 10, skipped: false, findings: vec![f("plate"), f("plate")] },
        Cell { name: "parks/dark".into(), frames: 10, skipped: false, findings: vec![] },
    ];
    let one = [AllowRow::new("flash", "plate", "island-auto|island-click", "the plate's hand-off").max(1)];
    let r = report_rows(&cells, &one, false, 0.0);
    assert_eq!((r.failing, r.allowed, r.unused), (1, 1, 0));
    assert!(r.text.contains("past its allow row's max of 1"), "{}", r.text);
    let rows = [AllowRow::new("", "plate", "island-auto", "any kind"), AllowRow::new("jump", "x", "parks", "stale"), AllowRow::new("jump", "x", "game", "not run")];
    let r = report_rows(&cells, &rows, false, 0.0);
    assert_eq!((r.failing, r.allowed, r.unused), (0, 2, 1), "only a row whose cells ran counts as unused");
    assert!(r.text.contains("unused allow row (\"jump\", \"x\", \"parks\")"), "{}", r.text);
}

#[test]
fn run_keeps_the_filtered_cells_in_order_and_flags_one_that_recorded_nothing() {
    let jobs: Vec<(String, Job)> = vec![
        ("move/wide".into(), Box::new(|| cell(|s, _| s.right = true))),
        ("move/narrow".into(), Box::new(|| cell(|s, _| s.right = true))),
        ("empty/wide".into(), Box::new(|| Recording::new((10.0, 10.0), 120.0))),
        ("show/wide".into(), Box::new(|| panic!("filtered out"))),
    ];
    let cells = run(jobs, "m");
    let names: Vec<&str> = cells.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["move/wide", "move/narrow", "empty/wide"]);
    assert!(cells[2].skipped && !cells[0].skipped);
    assert_eq!(report(&cells, &[], false, 0.0).failing, 1);
}

struct Feed {
    polls_change: bool,
}

fn feed(_: &Feed, _: &mut Cx<Feed>) -> El {
    let rows = (0..40).fold(col().w_full(), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0).fill(oklch(0.3, 0.02, 250.0))));
    col().w_full().h_full().child(col().key("feed").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

fn scroll_cell(polls_change: bool) -> Recording {
    let mut rt = Runtime::new(Feed { polls_change }, feed, (400.0, 300.0), Env::new(true, 1.0));
    rt.poll = Some(|s, _, _| s.polls_change);
    let mut r = Run::new(rt, 120.0);
    r.idle(0.5);
    r.step();
    for _ in 0..30 {
        r.act("wheel", |rt| rt.wheel(200.0, 150.0, -12.0));
    }
    r.play(0.5);
    r.rec
}

#[test]
fn an_app_that_runs_its_view_through_a_scroll_fails_the_gate() {
    let still = scroll_cell(false);
    assert!(!kinds(&still).contains(&"scroll-commit"), "{:?}", check(&still));
    let busy = scroll_cell(true);
    let f: Vec<_> = check(&busy).into_iter().filter(|f| f.kind == "scroll-commit").collect();
    assert_eq!(f.len(), 1, "{f:?}");
    assert!(f[0].detail.contains("(poll)"), "it names the cause: {}", f[0].detail);
}

/// Rows in a list drawn at 1.6×, each gliding on `spring` when the first drops by `drop`.
#[derive(Clone, Copy)]
struct Zoomed {
    drop: f32,
    spring: Spring,
    down: bool,
}

fn zoomed(s: &Zoomed, _: &mut Cx<Zoomed>) -> El {
    let ink = oklch(0.6, 0.15, 25.0);
    let rows = (0..30).fold(col().key("rows").w_full(), |c, i| {
        let row = el().key(format!("row{i}")).w_full().h(40.0).fill(ink).motion(s.spring);
        c.child(if i == 0 && s.down { row.mt(s.drop) } else { row })
    });
    col().key("zoom").w_full().h_full().scale(1.6)
        .child(col().key("feed").w_full().h_full().scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

fn zoomed_cell(drop: f32, spring: Spring) -> Recording {
    let mut r = Run::new(Runtime::new(Zoomed { drop, spring, down: false }, zoomed, (600.0, 600.0), Env::new(true, 1.0)), 120.0);
    r.rt.update(|_, tx| tx.scroll_to_instant("feed", 0.0, 200.0));
    r.idle(0.5);
    r.step();
    // Data arriving, not a hand: an input would excuse a path that turns.
    r.rt.update(|s, tx| { s.down = true; tx.scroll_to("feed", 0.0, 400.0); });
    r.play(1.0);
    r.rec
}

#[test]
fn motion_inside_a_zoomed_scroll_is_judged_against_the_scroll() {
    // A glide carries the rows up 200 pt, 320 pt on screen, while they glide 60 pt down the
    // list: on screen they only rise, and against the list they only move down. Taking the
    // scroll back at the list's own scale, not the window's, turns that into a dip and climb.
    let paths = |rec: &Recording, kinds: &[&str]| -> Vec<String> {
        check(rec).into_iter().filter(|f| kinds.contains(&f.kind)).map(|f| f.to_string()).collect()
    };
    let glide = zoomed_cell(60.0, Spring::new(0.2, 0.0));
    let row = glide.tracks.iter().find(|t| t.path.ends_with("row12")).unwrap();
    assert!(row.frames.windows(2).all(|w| w[1].1.rect[1] <= w[0].1.rect[1] + 0.01), "on screen it only rises");
    let turns = paths(&glide, &["reversal", "jiggle", "overshoot"]);
    assert!(turns.is_empty(), "{turns:#?}");
    // A row that overshoots its place in the list still does, the scroll subtracted.
    let wobble = zoomed_cell(60.0, Spring::new(0.3, 0.5));
    let turns = paths(&wobble, &["reversal", "jiggle", "overshoot"]);
    assert!(turns.iter().any(|f| f.contains("row12")), "{turns:#?}");
}

/// A 300 pt list whose first row starts just under the viewport, below 300 pt of padding.
fn padded(_: &(), _: &mut Cx<()>) -> El {
    let rows = (0..20).fold(col().key("rows").w_full().pt(300.0), |c, i| c.child(el().key(format!("row{i}")).w_full().h(40.0).fill(oklch(0.3, 0.02, 250.0))));
    col().w_full().h_full().child(col().key("list").w_full().h(300.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows))
}

/// The jumps, jerks and steps `scroll` makes in the padded list, as the gate reports them.
fn padded_jumps(scroll: fn(&mut Run<()>)) -> Vec<String> {
    let mut r = Run::new(Runtime::new((), padded, (400.0, 400.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    scroll(&mut r);
    r.play(1.0);
    r.finish().into_iter().filter(|f| ["jump", "offset-jump", "jerk", "appear-then-jump"].contains(&f.kind)).map(|f| f.to_string()).collect()
}

#[test]
fn a_scroll_that_teleports_a_row_into_view_with_no_input_jumps() {
    // The offset jumps 40 pt with no hand on the app: the first row, just off screen, pops
    // 40 pt into view, though nothing showed on both frames to be seen moving.
    let instant = padded_jumps(|r| {
        r.rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 40.0));
        r.step();
    });
    assert_eq!(instant.len(), 1, "{instant:#?}");
    assert!(instant[0].starts_with("jump") && instant[0].contains("rows"), "{instant:#?}");
    // A wheel, a scroll a tap asks for and a glide bring it in as you'd expect.
    let wheel = padded_jumps(|r| for _ in 0..4 { r.act("wheel", |rt| rt.wheel(200.0, 150.0, -10.0)) });
    assert!(wheel.is_empty(), "{wheel:#?}");
    let tapped = padded_jumps(|r| r.act("tap", |rt| rt.update(|_, tx| tx.scroll_to_instant("list", 0.0, 40.0))));
    assert!(tapped.is_empty(), "{tapped:#?}");
    let glide = padded_jumps(|r| {
        r.rt.update(|_, tx| tx.scroll_to("list", 0.0, 40.0));
        r.step();
    });
    assert!(glide.is_empty(), "{glide:#?}");
}
