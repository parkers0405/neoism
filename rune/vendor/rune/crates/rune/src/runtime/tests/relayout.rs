//! A resize with nothing else changed lays the last tree out again without running the view;
//! anything the view reads that the resize moves still runs it.
use super::*;
use crate::{TextEdit, TextStyle, memo, row, text, text_input};
use std::cell::Cell;

mod live;

thread_local! {
    static VIEWS: Cell<u32> = const { Cell::new(0) };
    static MEMOS: Cell<u32> = const { Cell::new(0) };
}

fn ran() { VIEWS.set(VIEWS.get() + 1); }

/// How many times `f` ran a view.
fn views(f: impl FnOnce()) -> u32 {
    let before = VIEWS.get();
    f();
    VIEWS.get() - before
}

fn resize<S>(rt: &mut Runtime<S>, width: f32, scale: f32) -> u32 {
    views(|| {
        rt.set_viewport((width, 600.0), scale);
        rt.frame(1.0 / 120.0);
    })
}

fn paragraphs(n: &u32, _: &mut Cx<u32>) -> El {
    ran();
    let para = "Paragraph text long enough to wrap over several lines as the window narrows.";
    col().w_full().children((0..3 + *n).map(|i| text(format!("{i}. {para}")).w_full().wrap().key(format!("t{i}"))))
        .child(memo("tail", 0, || {
            MEMOS.set(MEMOS.get() + 1);
            text("tail").key("tail")
        }))
}

fn node<'a, S>(rt: &'a Runtime<S>, key: &str) -> &'a Node {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap()
}

fn content<S>(rt: &Runtime<S>, key: &str) -> String {
    node(rt, key).text.as_ref().unwrap().content.clone()
}

/// Every node's path, layout rect and text line count.
fn layout<S>(rt: &Runtime<S>) -> Vec<(String, [f32; 4], usize)> {
    rt.nodes.iter().map(|n| (n.path.clone(), n.target, n.text_layout.as_ref().map_or(0, |l| l.len()))).collect()
}

#[test]
fn a_resize_alone_lays_the_tree_out_again_without_the_view_as_a_full_commit_would() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    let mut full = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    for w in [600.0, 420.0, 300.0, 520.0] {
        assert_eq!(resize(&mut rt, w, 2.0), 0, "at {w}");
        full.invalidate();
        assert_eq!(resize(&mut full, w, 2.0), 1);
        assert_eq!(layout(&rt), layout(&full), "at {w}");
        if w == 300.0 { assert!(node(&rt, "t0").text_layout.as_ref().unwrap().len() > 1, "the narrow window wrapped the text"); }
    }
}

/// A resize lays out into the node buffer the last commit let go of, as a flatten does.
#[test]
fn a_resize_reuses_the_last_commits_node_buffer() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    // The first commit let go of an empty buffer.
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    for w in [600.0, 420.0, 300.0] {
        let (spare, last) = (rt.spare_nodes.as_ptr(), rt.nodes.as_ptr());
        assert!(rt.spare_nodes.capacity() >= rt.nodes.len());
        assert_eq!(resize(&mut rt, w, 2.0), 0, "at {w}");
        assert_eq!(rt.nodes.as_ptr(), spare, "at {w}");
        assert_eq!(rt.spare_nodes.as_ptr(), last, "at {w}");
    }
}

#[test]
fn a_resize_keeps_the_memos_the_view_built() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(resize(&mut rt, 600.0, 2.0), 0);
    let memos = MEMOS.get();
    rt.invalidate();
    assert_eq!(resize(&mut rt, 500.0, 2.0), 1);
    assert_eq!(MEMOS.get(), memos, "the memo built before the resize is still cached");
}

#[test]
fn a_resize_with_a_state_change_runs_the_view() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|n, _| *n = 1);
    assert_eq!(resize(&mut rt, 600.0, 2.0), 1);
    assert_eq!(content(&rt, "t3").get(..2), Some("3."));
}

/// A view can read the scale (`Cx::env`), and text lays out at it.
#[test]
fn a_new_scale_runs_the_view() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_eq!(resize(&mut rt, 700.0, 1.0), 1);
    assert_eq!(resize(&mut rt, 600.0, 1.0), 0, "and the next resize at that scale doesn't");
}

/// Theme colours resolve per blend mode, so a resize in the frame the mode switched runs the view.
#[test]
fn a_new_blend_mode_runs_the_view() {
    let mut rt = Runtime::new(0, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.set_srgb_blend(!rt.srgb_blend());
    assert_eq!(resize(&mut rt, 600.0, 2.0), 1);
    assert_eq!(resize(&mut rt, 500.0, 2.0), 0, "and the next resize in that mode doesn't");
}

/// A node still showing its exit has left the layout tree, so the tree can't be laid out again.
#[test]
fn a_resize_during_an_exit_runs_the_view_and_the_exit_carries_on() {
    let mut rt = Runtime::new(1, paragraphs, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|n, _| *n = 0);
    rt.frame(1.0 / 120.0);
    let leaving = node(&rt, "t3").id;
    assert!(rt.is_exiting(leaving));
    assert_eq!(resize(&mut rt, 600.0, 2.0), 1);
    assert!(rt.is_exiting(leaving));
}

/// A transcript of rows that wrap, built by index in a scroll view.
fn transcript(sticky: &bool, _: &mut Cx<bool>) -> El {
    ran();
    let rows = col().key("rows").w_full().px(8.0).growth(crate::el::GrowthSpec::default()).virtual_rows(
        300, 40.0,
        |_: &bool, i| std::borrow::Cow::Owned(format!("m{i}")),
        |_: &bool, i| text(format!("{i}. {}", "Words that wrap over more lines as the window narrows. ".repeat(1 + i % 4))).w_full().wrap(),
    );
    col().w_full().h_full().child(col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(crate::ScrollSpec { stick_bottom: *sticky }).child(rows))
}

/// Resizes by the full path: the view runs, as it did before resizes reused the tree.
fn full_resize<S>(rt: &mut Runtime<S>, size: (f32, f32)) -> u32 {
    rt.reuse.scale = f32::NAN;
    views(|| {
        rt.set_viewport(size, 2.0);
        rt.frame(1.0 / 120.0);
    })
}

/// What shows of each element in the window, by path. A virtual list's extent counts rows it
/// hasn't mounted at their estimates, so it isn't something that shows.
fn shown<S>(rt: &Runtime<S>) -> Vec<(String, [i32; 4])> {
    let mut shown: Vec<_> = rt.elements().into_iter().filter(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0 && !e.virtual_extent)
        .map(|e| (e.path, e.rect.map(|x| (x * 100.0).round() as i32))).collect();
    shown.sort();
    shown
}

fn scrolled_transcript(sticky: bool) -> Runtime<bool> {
    let mut rt = Runtime::new(sticky, transcript, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..8 { rt.frame(1.0 / 120.0); }
    if !sticky {
        rt.wheel(300.0, 300.0, -3000.0);
        for _ in 0..30 { rt.frame(1.0 / 120.0); }
    }
    rt
}

/// A virtual list keeps the rows it mounted: they lay out again at the new width, and the
/// reader stays on the row they were reading, as when the view runs.
#[test]
fn a_resize_lays_a_virtual_lists_mounted_rows_out_again_without_the_view() {
    for sticky in [false, true] {
        let mut rt = scrolled_transcript(sticky);
        let mut full = scrolled_transcript(sticky);
        assert_eq!(shown(&rt), shown(&full));
        let range = rt.virtuals["#0/scroll/rows"].range.clone();
        let mut fast = 0;
        for w in (1..=40).map(|k| 700.0 - 5.0 * k as f32) {
            fast += u32::from(resize(&mut rt, w, 2.0) == 0);
            assert_eq!(full_resize(&mut full, (w, 600.0)), 1);
            assert_eq!(shown(&rt), shown(&full), "at {w}, sticky {sticky}");
        }
        assert!(fast >= 30, "{fast} of 40 steps skipped the view (sticky {sticky})");
        assert!(rt.virtuals["#0/scroll/rows"].range.start >= range.start.saturating_sub(4));
    }
}

/// Rows that grow as the window narrows lay out only until the ones outside the overscan add
/// up to a viewport; then the view runs and the window lets them go.
#[test]
fn a_narrowing_window_lets_go_of_rows_the_overscan_no_longer_holds() {
    let mut rt = scrolled_transcript(false);
    let mut ran = 0;
    for w in (1..=80).map(|k| 700.0 - 5.0 * k as f32) {
        ran += resize(&mut rt, w, 2.0);
        let v = &rt.virtuals["#0/scroll/rows"];
        let offset = rt.scrolls[&rt.path_ids["#0/scroll"]].offset as f32 - v.top;
        assert!(v.spare(offset, false) <= v.viewport, "at {w}");
    }
    assert!((1..20).contains(&ran), "the view ran {ran} times in 80 steps");
}

/// A resize that brings the viewport near the edge of the mounted rows runs the view to mount
/// more.
#[test]
fn a_resize_that_moves_a_virtual_window_runs_the_view() {
    let mut rt = scrolled_transcript(false);
    let mut full = scrolled_transcript(false);
    let before = rt.virtuals["#0/scroll/rows"].range.clone();
    rt.set_viewport((700.0, 1800.0), 2.0);
    assert_eq!(views(|| { rt.frame(1.0 / 120.0); }), 1);
    assert_eq!(full_resize(&mut full, (700.0, 1800.0)), 1);
    assert_ne!(rt.virtuals["#0/scroll/rows"].range, before, "the window moved");
    assert_eq!(shown(&rt), shown(&full));
}

/// A text leaving at a breakpoint fades out with its text, though the resize took the text
/// layouts before the view ran.
#[test]
fn text_leaving_at_a_breakpoint_keeps_its_layout() {
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        let narrow = cx.layout_with("root", |r| r[2] < 500.0).unwrap_or(false);
        let label = (!narrow).then(|| text("Only while wide, long enough to wrap when it has to").key("wide").w_full().wrap());
        col().key("root").w_full().h_full().children(label).child(text("always").key("always"))
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let wide = node(&rt, "wide").id;
    resize(&mut rt, 400.0, 2.0);
    assert!(rt.is_exiting(wide));
    assert!(node(&rt, "wide").text_layout.is_some(), "the exiting text still has lines to draw");
}

/// A view sized from `Cx::layout_size` runs when the resize changes the size it read, and
/// shows the new one.
#[test]
fn a_layout_read_the_resize_moves_runs_the_view() {
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        ran();
        let w = cx.layout_size("bar").map_or(-1.0, |s| s[0]);
        col().w_full().child(el().key("bar").w_full().h(10.0)).child(text(format!("{w}")).key("label"))
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(content(&rt, "label"), "700");
    assert!(resize(&mut rt, 600.0, 2.0) >= 1);
    assert_eq!(content(&rt, "label"), "600");
}

#[test]
fn a_layout_read_the_resize_leaves_alone_keeps_the_view() {
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        ran();
        let w = cx.layout_size("side").map_or(-1.0, |s| s[0]);
        row().w_full().child(el().key("side").w(120.0).h(10.0)).child(text(format!("{w}")).key("label").w_full().wrap())
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(content(&rt, "label"), "120");
    assert_eq!(resize(&mut rt, 600.0, 2.0), 0);
    assert_eq!(content(&rt, "label"), "120");
}

/// A scroll moving what a view read in the frame the window resizes runs the view, though the
/// resize alone leaves the read alone.
#[test]
fn a_resize_in_the_frame_a_scroll_moves_a_layout_read_runs_the_view() {
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        ran();
        let y = cx.window_layout("r5").map_or(-1.0, |r| r[1]);
        let rows = col().w_full().children((0..50).map(|k| el().key(format!("r{k}")).w(100.0).h(20.0)));
        col().w_full().children([
            col().key("scroll").w_full().h(200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows),
            text(format!("{y}")).key("label"),
        ])
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(content(&rt, "label"), "100");
    rt.wheel(100.0, 100.0, -50.0);
    assert!(resize(&mut rt, 600.0, 2.0) >= 1);
    assert_eq!(content(&rt, "label"), "50");
}

/// A breakpoint: the view lays two panes out side by side above 500 pt and stacked below,
/// reading only which side of it the window is on (`Cx::layout_with`).
#[test]
fn crossing_a_breakpoint_runs_the_view_and_resizing_within_one_does_not() {
    fn view(_: &u32, cx: &mut Cx<u32>) -> El {
        ran();
        let narrow = cx.layout_with("root", |r| r[2] < 500.0).unwrap_or(false);
        let panes = [el().key("a").h(50.0).grow(1.0), el().key("b").h(50.0).grow(1.0)];
        let body = if narrow { col().w_full().children(panes) } else { row().w_full().children(panes) };
        col().key("root").w_full().h_full().child(body)
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let stacked = |rt: &Runtime<u32>| node(rt, "b").target[1] > node(rt, "a").target[1];
    assert!(!stacked(&rt));
    assert_eq!(resize(&mut rt, 600.0, 2.0), 0, "still wide");
    assert!(resize(&mut rt, 400.0, 2.0) >= 1, "crossed the breakpoint");
    assert!(stacked(&rt));
    assert_eq!(resize(&mut rt, 450.0, 2.0), 0, "still narrow");
    assert!(stacked(&rt));
    assert!(resize(&mut rt, 650.0, 2.0) >= 1);
    assert!(!stacked(&rt));
}

/// A view showing the caret's place runs when the resize moves the field it is in.
#[test]
fn a_caret_read_the_resize_moves_runs_the_view() {
    fn view(s: &TextEdit, cx: &mut Cx<TextEdit>) -> El {
        ran();
        let x = cx.caret("field").map_or(-1.0, |r| r[0]);
        col().w_full().items_center().children([
            text_input(s, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0).on_edit(|s: &mut TextEdit| s),
            text(format!("{x}")).key("label"),
        ])
    }
    let mut edit = TextEdit::new("hello");
    edit.focused = true;
    let mut rt = Runtime::new(edit, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.frame(1.0 / 120.0);
    let before = content(&rt, "label");
    assert_ne!(before, "-1");
    assert!(resize(&mut rt, 500.0, 2.0) >= 1);
    let after = content(&rt, "label");
    assert_ne!(after, before);
    let caret = rt.caret.as_ref().unwrap().1[0];
    assert_eq!(after, format!("{caret}"), "the view shows where the caret is now");
}

/// Layout fits an `aspect_fit` element by rewriting its style, which a resize has to undo.
#[test]
fn a_resize_refits_aspect_fit_elements_as_a_full_commit_would() {
    fn view(_: &u32, _: &mut Cx<u32>) -> El {
        ran();
        col().w_full().h_full().items_center().child(el().key("pic").aspect_fit(2.0)).child(text("caption").key("cap"))
    }
    let mut rt = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    let mut full = Runtime::new(0, view, (700.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    for w in [600.0, 900.0, 300.0] {
        resize(&mut rt, w, 2.0);
        full.invalidate();
        resize(&mut full, w, 2.0);
        assert_eq!(layout(&rt), layout(&full), "at {w}");
    }
}

/// Moving `Node`s on resize skips cloning paths/specs, keeps a fixed-width sidebar's scene rects
/// and focused `TextEdit` intact, and restores moved nodes cleanly if a virtual window shifts.
#[test]
fn a_resize_moves_nodes_keeps_inputs_and_restores_on_virtual_window_fallback() {
    struct App {
        edit: TextEdit,
    }
    fn view(s: &App, _: &mut Cx<App>) -> El {
        ran();
        let side = col().key("side").w(180.0).h_full().children(
            (0..20).map(|i| row().key(format!("s{i}")).w_full().h(24.0).child(text(format!("Item {i}")))),
        );
        let list = col().key("rows").w_full().growth(crate::el::GrowthSpec::default()).virtual_rows(
            200,
            32.0,
            |_: &App, i| std::borrow::Cow::Owned(format!("r{i}")),
            |_: &App, i| text(format!("{i}: {}", "wrapping row prose ".repeat(1 + i % 3))).w_full().wrap(),
        );
        row().w_full().h_full().children([
            side,
            col().key("main").grow(1.0).h_full().children([
                text_input(&s.edit, TextStyle::new(14.0, 400.0)).key("composer").w_full().h(36.0).on_edit(|a: &mut App| &mut a.edit),
                col().key("scroll").w_full().grow(1.0).min_h(0.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(list),
            ]),
        ])
    }
    let mut edit = TextEdit::new("draft");
    edit.focused = true;
    let mut rt = Runtime::new(App { edit: edit.clone() }, view, (800.0, 600.0), Env::new(true, 2.0));
    let mut full = Runtime::new(App { edit }, view, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    rt.wheel(400.0, 300.0, -600.0);
    full.wheel(400.0, 300.0, -600.0);
    rt.frame(1.0 / 120.0);
    full.frame(1.0 / 120.0);

    // Narrow steps keep the window (0 view runs); a huge jump shifts the window and falls back cleanly.
    for &(w, h) in &[(760.0, 600.0), (700.0, 600.0), (420.0, 1200.0), (780.0, 600.0)] {
        rt.set_viewport((w, h), 2.0);
        rt.frame(1.0 / 120.0);
        full.reuse.scale = f32::NAN;
        full.set_viewport((w, h), 2.0);
        full.frame(1.0 / 120.0);
        assert_eq!(shown(&rt), shown(&full), "at {w}x{h}");
        assert_eq!(rt.focus.path(), Some("#0/main/composer"));
    }
}
