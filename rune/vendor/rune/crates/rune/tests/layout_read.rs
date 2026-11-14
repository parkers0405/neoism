//! Views read another element's laid-out rect (`Cx::layout`) and handlers hear about it
//! (`El::on_layout`), re-running only when it moves, and settling when a view reads
//! something its own output resizes.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Runtime, ScrollSpec, col, el, row, text};
use std::cell::Cell;

#[derive(Default)]
struct S {
    banner: bool,
    views: Cell<u32>,
    rects: Vec<[f32; 4]>,
    grow_by: f32,
    width: f32,
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}"))
}

fn frames(rt: &mut Runtime<S>, n: usize) {
    for _ in 0..n { rt.frame(1.0 / 120.0); }
}

/// A timeline below a banner; a badge hangs from the banner's bottom by reading its rect.
fn banner_view(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    assert_eq!(cx.layout("no-such-key"), None);
    let mut root = col().w_full().h_full().padding(10.0)
        .child(el().key("banner").w_full().h(if s.banner { 55.0 } else { 20.0 })
            .on_layout(|s: &mut S, r| s.rects.push(r)))
        .child(col().key("timeline").w_full().grow(1.0).shrink());
    if let Some([x, y, w, h]) = cx.layout("banner") {
        root = root.child(el().key("badge").absolute(x + w - 30.0, y + h).size(30.0, 10.0));
    }
    root
}

#[test]
fn a_view_places_from_another_elements_rect_in_the_same_commit() {
    let mut rt = Runtime::new(S::default(), banner_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "badge").target, [260.0, 30.0, 30.0, 10.0], "placed on the first frame");
    rt.state.banner = true;
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "badge").target, [260.0, 65.0, 30.0, 10.0], "moved in the commit that grew the banner");

    // Animation frames re-run nothing and report nothing; settled, nothing re-runs.
    let views = rt.state.views.get();
    frames(&mut rt, 120);
    assert_eq!(rt.state.views.get(), views + 1, "one re-run, for on_layout's state change");
    assert_eq!(rt.state.rects, [[10.0, 10.0, 280.0, 20.0], [10.0, 10.0, 280.0, 55.0]], "once on layout, once on change");
    let commits = rt.stats.commits;
    frames(&mut rt, 10);
    assert_eq!(rt.stats.commits, commits);
}

/// Rows in a scroll; a marker outside it follows row 10 as it scrolls.
fn scroll_view(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    let rows = (0..40).fold(col().w_full(), |c, i| c.child(el().key(format!("r{i}")).w_full().h(30.0)));
    let mut root = col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows));
    if let Some([_, y, _, _]) = cx.window_layout("r10") {
        root = root.child(el().key("marker").absolute(0.0, y).size(10.0, 10.0));
    }
    root
}

#[test]
fn scrolling_moves_a_read_rect_and_re_runs_its_reader() {
    let mut rt = Runtime::new(S::default(), scroll_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "marker").target[1], 300.0);
    rt.wheel(100.0, 100.0, -120.0);
    frames(&mut rt, 60);
    let row = find(&rt, "r10").rect[1];
    assert!(row < 250.0, "scrolled: {row}");
    assert_eq!(find(&rt, "marker").target[1], row, "the marker follows the row");
    let views = rt.state.views.get();
    frames(&mut rt, 10);
    assert_eq!(rt.state.views.get(), views, "still once scrolling stops");
}

/// A card reads its own width to pick a layout: two columns when wide, one when narrow.
fn responsive(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    let wide = cx.layout("card").is_some_and(|r| r[2] >= 400.0);
    let body = if wide { row().key("body").gap(10.0) } else { col().key("body").gap(10.0) };
    col().w_full().padding(10.0).child(
        col().key("card").w_full().child(body.child(el().key("a").w(150.0).h(40.0)).child(el().key("b").w(150.0).h(40.0))),
    )
}

#[test]
fn a_view_reading_its_own_size_settles() {
    let mut rt = Runtime::new(S::default(), responsive, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "b").target[1], find(&rt, "a").target[1], "wide: side by side on the first frame");
    rt.set_viewport((300.0, 400.0), 2.0);
    frames(&mut rt, 2);
    assert!(find(&rt, "b").target[1] > find(&rt, "a").target[1], "narrow: stacked");
    frames(&mut rt, 60);
    let (views, commits) = (rt.state.views.get(), rt.stats.commits);
    frames(&mut rt, 30);
    assert_eq!((rt.state.views.get(), rt.stats.commits), (views, commits), "settled");
}

/// A chip that reads its own width and swaps to the label that flips it back: no fixed point.
fn flip_flop(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    let wide = cx.layout("chip").is_some_and(|r| r[2] > 100.0);
    let label = if wide { "short" } else { "a much longer label than that" };
    col().w_full().items_start().child(row().key("chip").child(text(label)))
}

#[test]
fn a_view_without_a_fixed_point_stops_instead_of_oscillating() {
    let mut rt = Runtime::new(S::default(), flip_flop, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let first = rt.state.views.get();
    assert!(first <= 4, "one commit re-runs the view at most twice more ({first} runs)");
    frames(&mut rt, 120);
    let (views, commits) = (rt.state.views.get(), rt.stats.commits);
    frames(&mut rt, 60);
    assert_eq!((rt.state.views.get(), rt.stats.commits), (views, commits), "no commits of its own");
}

/// A handler feeding an element's width back into it, growing forever if allowed.
fn runaway(s: &S, _: &mut Cx<S>) -> El {
    let grow_by = s.grow_by;
    col().w_full().child(el().key("bar").w(s.width.max(10.0)).h(10.0).on_layout(move |s: &mut S, r| {
        s.rects.push(r);
        s.width = r[2] + grow_by;
    }))
}

#[test]
fn on_layout_feedback_settles() {
    // A handler that reaches a fixed point: once to lay out, once for the change it caused.
    let mut rt = Runtime::new(S { grow_by: 0.0, width: 50.0, ..S::default() }, runaway, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    frames(&mut rt, 30);
    assert_eq!(rt.state.rects.len(), 1, "{:?}", rt.state.rects);

    // One that never would: it stops after a few commits, then nothing re-runs.
    let mut rt = Runtime::new(S { grow_by: 10.0, width: 50.0, ..S::default() }, runaway, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    frames(&mut rt, 60);
    let reports = rt.state.rects.len();
    assert!((2..=6).contains(&reports), "{:?}", rt.state.rects);
    let commits = rt.stats.commits;
    frames(&mut rt, 60);
    assert_eq!((rt.state.rects.len(), rt.stats.commits), (reports, commits));

    // Changes it missed meanwhile are not replayed; the next change from anywhere else is
    // reported, and feedback may run again.
    rt.invalidate();
    frames(&mut rt, 10);
    assert_eq!(rt.state.rects.len(), reports);
    rt.state.width = 200.0;
    rt.invalidate();
    frames(&mut rt, 60);
    assert_eq!(rt.state.rects[reports], [0.0, 0.0, 200.0, 10.0], "{:?}", rt.state.rects);
    assert!(rt.state.rects.len() > reports + 1, "{:?}", rt.state.rects);
}

#[derive(Default)]
struct Editor {
    editing: bool,
    field: rune::TextEdit,
}

/// An inline editor placed over a node by reading its rect, so it first appears on the
/// commit's re-run, never on its first measure.
fn editor_view(s: &Editor, cx: &mut Cx<Editor>) -> El {
    let mut root = col().w_full().h_full().padding(10.0).child(el().key("node").size(120.0, 30.0));
    if s.editing && let Some([x, y, w, _]) = cx.layout("node") {
        root = root.child(rune::text_input(&s.field, rune::tok::TextStyle::new(14.0, 400.0))
            .key("editor").absolute(x, y).w(w).on_edit(|s: &mut Editor| &mut s.field));
    }
    root
}

#[test]
fn a_field_that_appears_on_the_re_run_takes_the_focus_it_asks_for() {
    let mut rt = Runtime::new(Editor::default(), editor_view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| { s.editing = true; s.field.focused = true; });
    rt.frame(1.0 / 120.0);
    let editor = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("editor")).expect("placed in this commit");
    assert_eq!(editor.target[..2], [10.0, 10.0]);
    assert!(editor.focused, "focused in the commit that placed it");
    rt.key(rune::KeyEvent { key: rune::Key::Text("a".into()), mods: Default::default() });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.field.text, "a", "typing reaches it");
}

#[test]
fn a_focused_field_remounting_after_a_layout_read_does_not_steal_newer_focus() {
    #[derive(Default)]
    struct Two {
        step: u8,
        first: rune::TextEdit,
        second: rune::TextEdit,
    }

    fn two_view(s: &Two, cx: &mut Cx<Two>) -> El {
        let mut root = col().w_full().h_full()
            .child(el().key("anchor").size(100.0, 20.0))
            .child(rune::text_input(&s.second, rune::tok::TextStyle::new(14.0, 400.0))
                .key("second").w(100.0).on_edit(|s: &mut Two| &mut s.second));
        if s.step == 0 {
            root = root.child(el().key("early").child(
                rune::text_input(&s.first, rune::tok::TextStyle::new(14.0, 400.0))
                    .key("first").w(100.0).on_edit(|s: &mut Two| &mut s.first),
            ));
        } else if s.step == 2 && cx.layout("anchor").is_some() {
            root = root.child(el().key("late").child(
                rune::text_input(&s.first, rune::tok::TextStyle::new(14.0, 400.0))
                    .key("first").w(100.0).on_edit(|s: &mut Two| &mut s.first),
            ));
        }
        root
    }

    let mut rt = Runtime::new(Two::default(), two_view, (300.0, 200.0), Env::new(true, 2.0));
    rt.update(|s, _| s.first.focused = true);
    rt.frame(0.0);
    // Unmount `first` while it still has `first.focused = true`, and focus `second`.
    rt.update(|s, _| { s.step = 1; s.second.focused = true; });
    rt.frame(1.0 / 60.0);
    // Remount `first` under a new path on a layout-read re-run: `second` keeps focus.
    rt.update(|s, _| s.step = 2);
    rt.frame(1.0 / 60.0);
    let focused: Vec<_> = rt.elements().into_iter().filter(|e| e.focused).filter_map(|e| e.key).collect();
    assert_eq!(focused, ["second"], "remounted field must not steal focus from the newer field");
    assert!(rt.state.second.focused && !rt.state.first.focused);
}

/// How `sized_view` reads row 10.
#[derive(Clone, Copy, Debug, Default)]
enum Read {
    #[default]
    Size,
    Rect,
    Window,
}

#[derive(Default)]
struct SizeRead {
    read: Read,
    tall: bool,
    views: Cell<u32>,
}

/// Rows in a scroll; a bar below it echoes row 10's size, read with `layout_size`, `layout`
/// or `window_layout`.
fn sized_view(s: &SizeRead, cx: &mut Cx<SizeRead>) -> El {
    s.views.set(s.views.get() + 1);
    let rows = (0..40).fold(col().w_full(), |c, i| {
        c.child(el().key(format!("r{i}")).w_full().h(if i == 10 && s.tall { 50.0 } else { 30.0 }))
    });
    let size = match s.read {
        Read::Size => cx.layout_size("r10"),
        Read::Rect => cx.layout("r10").map(|r| [r[2], r[3]]),
        Read::Window => cx.window_layout("r10").map(|r| [r[2], r[3]]),
    };
    let mut root = col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows));
    if let Some([w, h]) = size {
        root = root.child(el().key("echo").w(w).h(h));
    }
    root
}

fn echo(rt: &Runtime<SizeRead>) -> [f32; 4] {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some("echo")).expect("echo").target
}

/// View runs and commits over 60 frames of a trackpad-like scroll, a wheel event a frame.
fn scroll_runs(read: Read) -> (u32, u64) {
    let mut rt = Runtime::new(SizeRead { read, ..SizeRead::default() }, sized_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    let (views, commits) = (rt.state.views.get(), rt.stats.commits);
    let before = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap().rect[1];
    for _ in 0..60 {
        rt.wheel(100.0, 100.0, -3.0);
        rt.frame(1.0 / 120.0);
    }
    let after = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap().rect[1];
    assert!(after < before - 50.0, "scrolled: {before} -> {after}");
    (rt.state.views.get() - views, rt.stats.commits - commits)
}

#[test]
fn only_a_window_read_re_runs_its_view_while_the_element_scrolls() {
    let size = scroll_runs(Read::Size);
    let rect = scroll_runs(Read::Rect);
    let window = scroll_runs(Read::Window);
    eprintln!("60 scroll frames (views, commits): layout_size {size:?}; layout {rect:?}; window_layout {window:?}");
    assert_eq!(size, (0, 0), "position moves alone don't re-run a size read");
    assert_eq!(rect, (0, 0), "a whole-rect read is unscrolled: a scroll doesn't move it");
    assert!(window.0 >= 50 && window.1 >= 50, "a window read follows the scroll ({window:?})");
}

#[test]
fn a_size_read_re_runs_its_view_when_the_element_resizes() {
    let mut rt = Runtime::new(SizeRead::default(), sized_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(echo(&rt)[2..], [300.0, 30.0], "settled on the first frame");

    // Its own height, in the commit that changed it.
    rt.update(|s, _| s.tall = true);
    rt.frame(1.0 / 120.0);
    assert_eq!(echo(&rt)[2..], [300.0, 50.0]);

    // Its width, from a parent resize.
    rt.set_viewport((420.0, 400.0), 2.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(echo(&rt)[2..], [420.0, 50.0]);

    let views = rt.state.views.get();
    for _ in 0..30 { rt.frame(1.0 / 120.0); }
    assert_eq!(rt.state.views.get(), views, "settled");
}

/// A card reads its own width with `layout_size` to pick two columns or one.
fn responsive_size(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    let wide = cx.layout_size("card").is_some_and(|[w, _]| w >= 400.0);
    let body = if wide { row().key("body").gap(10.0) } else { col().key("body").gap(10.0) };
    col().w_full().padding(10.0).child(
        col().key("card").w_full().child(body.child(el().key("a").w(150.0).h(40.0)).child(el().key("b").w(150.0).h(40.0))),
    )
}

#[test]
fn a_view_reading_its_own_size_with_layout_size_settles_on_the_first_frame() {
    let mut rt = Runtime::new(S::default(), responsive_size, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "b").target[1], find(&rt, "a").target[1], "wide: side by side on the first frame");
    rt.set_viewport((300.0, 400.0), 2.0);
    rt.frame(1.0 / 120.0);
    assert!(find(&rt, "b").target[1] > find(&rt, "a").target[1], "narrow in the commit that resized it");
    frames(&mut rt, 60);
    let (views, commits) = (rt.state.views.get(), rt.stats.commits);
    frames(&mut rt, 30);
    assert_eq!((rt.state.views.get(), rt.stats.commits), (views, commits), "settled");
}

#[derive(Default)]
struct Header {
    /// What each view run read: whether row 1 has scrolled past the top.
    seen: std::cell::RefCell<Vec<Option<bool>>>,
}

/// A header that turns solid once row 1 scrolls past the top of the window.
fn header_view(s: &Header, cx: &mut Cx<Header>) -> El {
    let past = cx.window_layout_with("r1", |r| r[1] < 0.0);
    s.seen.borrow_mut().push(past);
    let rows = (0..40).fold(col().w_full(), |c, i| c.child(el().key(format!("r{i}")).w_full().h(30.0)));
    let header = el().key(if past == Some(true) { "solid" } else { "clear" }).w_full().h(20.0);
    col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        .child(header)
}

#[test]
fn a_layout_with_read_settles_on_the_first_layout() {
    let mut rt = Runtime::new(Header::default(), header_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(rt.state.seen.borrow().last(), Some(&Some(false)), "{:?}", rt.state.seen.borrow());
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("clear")));
}

#[test]
fn a_layout_with_read_re_runs_its_view_only_when_its_value_flips() {
    let mut rt = Runtime::new(Header::default(), header_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    rt.state.seen.borrow_mut().clear();
    let r1 = |rt: &Runtime<Header>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r1")).unwrap().rect[1];

    // Down past the top: one run, on the frame it flips.
    let commits = rt.stats.commits;
    let mut flipped_at = None;
    for frame in 0..20 {
        rt.wheel(100.0, 100.0, -3.0);
        rt.frame(1.0 / 120.0);
        if flipped_at.is_none() && r1(&rt) < 0.0 { flipped_at = Some(frame); }
        assert_eq!(rt.state.seen.borrow().len(), usize::from(flipped_at.is_some()), "frame {frame}: y {}", r1(&rt));
    }
    assert!(r1(&rt) < -20.0, "scrolled past: {}", r1(&rt));
    assert_eq!(*rt.state.seen.borrow(), [Some(true)]);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("solid")), "solid in the frame it flipped");
    assert_eq!(rt.stats.commits - commits, 1, "one commit over 20 scrolled frames");

    // And back: one more.
    for frame in 0..20 {
        rt.wheel(100.0, 100.0, 3.0);
        rt.frame(1.0 / 120.0);
        let back = r1(&rt) >= 0.0;
        assert_eq!(rt.state.seen.borrow().len(), 1 + usize::from(back), "frame {frame}: y {}", r1(&rt));
        if back { break; }
    }
    for _ in 0..20 {
        rt.wheel(100.0, 100.0, 3.0);
        rt.frame(1.0 / 120.0);
    }
    assert_eq!(*rt.state.seen.borrow(), [Some(true), Some(false)]);
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("clear")));
    assert_eq!(rt.stats.commits - commits, 2);
}

#[derive(Default)]
struct Bar {
    width: f32,
    top: f32,
    views: Cell<u32>,
}

/// Reads one bar's whole rect and its size: the size check must not be lost under the rect read.
fn both_view(s: &Bar, cx: &mut Cx<Bar>) -> El {
    s.views.set(s.views.get() + 1);
    let _ = cx.layout("bar");
    let w = cx.layout_size("bar").map_or(0.0, |[w, _]| w);
    col().w_full().items_start().child(el().key("bar").w(s.width).h(10.0)).child(el().key("echo").w(w).h(10.0))
}

#[test]
fn a_whole_rect_read_keeps_a_size_read_of_the_same_key_exact() {
    let mut rt = Runtime::new(Bar { width: 100.0, ..Bar::default() }, both_view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|s, _| s.width = 100.4);
    rt.frame(1.0 / 120.0);
    let echo = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("echo")).unwrap().target[2];
    assert!((echo - 100.4).abs() < 1e-3, "a 0.4 pt resize reaches the size read: {echo}");
}

#[test]
fn drift_below_the_threshold_adds_up_until_a_whole_rect_read_re_runs() {
    let mut rt = Runtime::new(S::default(), scroll_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..100 {
        rt.wheel(100.0, 100.0, -0.4);
        rt.frame(1.0 / 120.0);
    }
    let (row, marker) = (find(&rt, "r10").rect[1], find(&rt, "marker").target[1]);
    assert!(row < 270.0, "scrolled 40 pt: {row}");
    assert!((marker - row).abs() <= 0.5, "the marker follows within half a point: marker {marker}, row {row}");
}

#[derive(Default)]
struct Gate {
    width: f32,
    views: Cell<u32>,
}

/// Reads `b` only once `a`'s rect matches the state, so `b` goes unread on a commit's first pass.
fn gate_view(s: &Gate, cx: &mut Cx<Gate>) -> El {
    s.views.set(s.views.get() + 1);
    let ready = cx.layout("a").is_some_and(|r| (r[2] - s.width).abs() < 0.01);
    let b = if ready { cx.layout("b") } else { None };
    col().w_full().items_start()
        .child(el().key("a").w(s.width).h(10.0))
        .child(el().key("b").w(50.0).h(10.0))
        .child(el().key("echo").w(b.map_or(0.0, |r| r[2])).h(5.0))
}

#[test]
fn a_key_unread_on_one_settling_pass_keeps_its_rect_for_the_next() {
    let mut rt = Runtime::new(Gate { width: 100.0, ..Gate::default() }, gate_view, (300.0, 200.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..10 { rt.frame(1.0 / 120.0); }
    let views = rt.state.views.get();
    rt.update(|s, _| s.width = 200.0);
    rt.frame(1.0 / 120.0);
    let echo = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("echo")).unwrap().target[2];
    assert_eq!(echo, 50.0);
    assert_eq!(rt.state.views.get() - views, 2, "one pass with a stale, one to settle");
}

/// Echoes the size of an element a third of 100 tall, below a gap the state sets.
fn third_view(s: &Bar, cx: &mut Cx<Bar>) -> El {
    s.views.set(s.views.get() + 1);
    let h = cx.layout_size("third").map_or(0.0, |[_, h]| h);
    col().w_full().items_start()
        .child(el().key("gap").w(10.0).h(s.top))
        .child(el().key("third").w(10.0).h(100.0 / 3.0))
        .child(el().key("echo").w(10.0).h(h))
}

#[test]
fn moving_an_element_does_not_change_the_size_a_read_sees() {
    let mut rt = Runtime::new(Bar::default(), third_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for top in [33.4, 50.0, 31.0, 64.1] {
        let views = rt.state.views.get();
        rt.update(|s, _| s.top = top);
        rt.frame(1.0 / 120.0);
        assert_eq!(rt.state.views.get() - views, 1, "a move to {top} re-ran the size read");
    }
}

/// A marker follows row 10 through a scroll, beside a bar whose `on_layout` grows it forever.
fn capped_view(s: &S, cx: &mut Cx<S>) -> El {
    s.views.set(s.views.get() + 1);
    let rows = (0..40).fold(col().w_full(), |c, i| c.child(el().key(format!("r{i}")).w_full().h(30.0)));
    let grow_by = s.grow_by;
    let mut root = col().w_full().h_full().items_start()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        .child(el().key("bar").w(s.width.max(10.0)).h(10.0).on_layout(move |s: &mut S, r| s.width = r[2] + grow_by));
    if let Some([_, y, _, _]) = cx.window_layout("r10") {
        root = root.child(el().key("marker").absolute(0.0, y).size(10.0, 10.0));
    }
    root
}

#[test]
fn the_first_scroll_frame_after_feedback_stops_still_moves_a_read() {
    let mut rt = Runtime::new(S { grow_by: 10.0, width: 50.0, ..S::default() }, capped_view, (600.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    frames(&mut rt, 60);
    let commits = rt.stats.commits;
    frames(&mut rt, 10);
    assert_eq!(rt.stats.commits, commits, "feedback stopped");
    rt.wheel(100.0, 100.0, -30.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "marker").target[1], find(&rt, "r10").rect[1], "followed on the first scroll frame");
}

#[derive(Default)]
struct Board {
    tall: bool,
    builds: Cell<u32>,
}

/// A memo below a scrolling list shows `memo::layout` of row 10 inside it.
fn board_view(s: &Board, _: &mut Cx<Board>) -> El {
    let rows = (0..40).fold(col().w_full(), |c, i| {
        c.child(el().key(format!("r{i}")).w_full().h(if i == 10 && s.tall { 50.0 } else { 30.0 }))
    });
    col().w_full().h_full()
        .child(col().key("scroll").w_full().h(300.0).scroll(ScrollSpec { stick_bottom: false }).child(rows))
        .child(rune::memo::memo("label", 0, || {
            s.builds.set(s.builds.get() + 1);
            text(format!("{:?}", rune::memo::layout("r10"))).key("label")
        }))
}

#[test]
fn a_memo_layout_read_inside_a_scrolled_viewport_commits_nothing_while_it_scrolls() {
    let mut rt = Runtime::new(Board::default(), board_view, (300.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    frames_of(&mut rt, 10);
    let label = |rt: &Runtime<Board>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label")).and_then(|e| e.text);
    let r10 = |rt: &Runtime<Board>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("r10")).unwrap().rect;
    assert_eq!(label(&rt).as_deref(), Some("Some([0.0, 300.0, 300.0, 30.0])"));
    let (commits, builds) = (rt.stats.commits, rt.state.builds.get());

    // A programmatic, animated scroll, then a wheel scroll.
    rt.update_if(|_, tx| { tx.scroll_to_y("scroll", 200.0); false });
    frames_of(&mut rt, 60);
    assert_eq!(r10(&rt)[1], 100.0, "scrolled 200");
    for _ in 0..60 {
        rt.wheel(100.0, 100.0, -3.0);
        rt.frame(1.0 / 120.0);
    }
    frames_of(&mut rt, 60);
    assert!(r10(&rt)[1] < 0.0, "scrolled on: {:?}", r10(&rt));
    assert_eq!((rt.stats.commits, rt.state.builds.get()), (commits, builds), "no commits, no builds");
    assert_eq!(label(&rt).as_deref(), Some("Some([0.0, 300.0, 300.0, 30.0])"), "unscrolled");

    // A real size change still rebuilds, in the commit that made it.
    rt.update(|s, _| s.tall = true);
    rt.frame(1.0 / 120.0);
    assert_eq!(label(&rt).as_deref(), Some("Some([0.0, 300.0, 300.0, 50.0])"));
    assert!(rt.state.builds.get() > builds);
}

fn frames_of<S: 'static>(rt: &mut Runtime<S>, n: usize) {
    for _ in 0..n { rt.frame(1.0 / 120.0); }
}
