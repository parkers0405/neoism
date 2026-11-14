//! Virtual-window shifts (`CommitWhy::VIRTUAL`) remount only newly entering `virtual_rows`,
//! leaving `view`, sibling subtrees and already-mounted rows untouched unless a read moves.
use super::*;
use crate::el::GrowthSpec;
use crate::{ScrollSpec, memo, row, text};
use std::borrow::Cow;
use std::cell::{Cell, RefCell};

thread_local! {
    static VIEWS: Cell<u32> = const { Cell::new(0) };
    static SIDES: Cell<u32> = const { Cell::new(0) };
    static BUILT_ROWS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn take_views<R>(f: impl FnOnce() -> R) -> (u32, R) {
    let before = VIEWS.get();
    let r = f();
    (VIEWS.get() - before, r)
}

fn take_built() -> Vec<usize> {
    BUILT_ROWS.with_borrow_mut(std::mem::take)
}

fn shown_elements<S>(rt: &Runtime<S>) -> Vec<(String, [i32; 4], Option<String>)> {
    let mut out: Vec<_> = rt
        .elements()
        .into_iter()
        .filter(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0 && !e.virtual_extent)
        .map(|e| (e.path, e.rect.map(|x| (x * 100.0).round() as i32), e.text))
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn plain_workspace(tick: &u32, _: &mut Cx<u32>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    SIDES.set(SIDES.get() + 1);
    let side = col().key("side").w(180.0).h_full().gap(4.0).children(
        (0..24).map(|i| text(format!("Side {i}: {tick}")).key(format!("s{i}")).h(18.0)),
    );
    let list = col().key("rows").w_full().px(8.0).growth(GrowthSpec::default()).virtual_rows(
        500,
        36.0,
        |_: &u32, i| Cow::Owned(format!("r{i}")),
        |t: &u32, i| {
            BUILT_ROWS.with_borrow_mut(|b| b.push(i));
            row().w_full().h(36.0).gap(6.0).children([
                text(format!("Row {i}")).key("title").grow(1.0),
                text(format!("v{t}")).key("ver"),
            ])
        },
    );
    let footer = text(format!("Footer {tick}")).key("footer").h(28.0);
    row().w_full().h_full().children([
        side,
        col().key("main").grow(1.0).h_full().children([
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
            footer,
        ]),
    ])
}

#[test]
fn scrolling_virtual_rows_without_memo_skips_view_and_keeps_mounted_rows() {
    let mut rt = Runtime::new(0u32, plain_workspace, (640.0, 420.0), Env::new(true, 2.0));
    let mut full = Runtime::new(0u32, plain_workspace, (640.0, 420.0), Env::new(true, 2.0));
    rt.frame(0.0);
    full.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    while full.frame(1.0 / 120.0) {}

    SIDES.set(0);
    take_built();
    let mut scroll_commits = 0u32;
    let mut full_commits = 0u32;
    for step in 0..40 {
        take_built();
        let c0 = rt.stats.commits;
        let (views_ran, _) = take_views(|| {
            rt.wheel(400.0, 200.0, -90.0);
            rt.frame(1.0 / 120.0);
        });
        assert_eq!(views_ran, 0, "step {step}: scrolling must not re-run view");
        let built = take_built();
        if rt.stats.commits > c0 {
            scroll_commits += 1;
            let mounted_len = rt.virtuals["#0/main/scroll/rows"].range.len();
            assert!(
                built.len() < mounted_len,
                "step {step}: built {} rows for a {mounted_len}-row window",
                built.len()
            );
        } else {
            assert!(built.is_empty(), "step {step}: non-committing frame built {built:?}");
        }

        // Force full view on the reference runtime and check exact visible parity.
        let fc0 = full.stats.commits;
        full.reuse.scale = f32::NAN;
        full.wheel(400.0, 200.0, -90.0);
        full.frame(1.0 / 120.0);
        if full.stats.commits > fc0 {
            full_commits += 1;
        }
        assert_eq!(shown_elements(&rt), shown_elements(&full), "step {step}");
    }
    assert!(scroll_commits >= 10, "expected window shifts, got {scroll_commits}");
    assert_eq!(SIDES.get(), full_commits, "only full-commit reference steps ran the sidebar; rt ran 0");

    // Hover and press on both a kept row and a newly scrolled-in row still work, and a state
    // mutation runs the view again with updated content everywhere.
    rt.update(|t, _| *t = 7);
    let (views_ran, _) = take_views(|| {
        rt.frame(1.0 / 120.0);
    });
    assert_eq!(views_ran, 1);
    full.update(|t, _| *t = 7);
    full.frame(1.0 / 120.0);
    assert_eq!(shown_elements(&rt), shown_elements(&full));
}

#[test]
fn virtual_remount_reruns_view_when_a_layout_read_of_a_culled_row_changes() {
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let r0_mounted = cx.layout("r0").is_some();
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            200,
            40.0,
            |_: &(), i| Cow::Owned(format!("r{i}")),
            |_: &(), i| text(format!("Row {i}")).h(40.0),
        );
        col().w_full().h_full().children([
            text(format!("r0={r0_mounted}")).key("status").h(24.0),
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        ])
    }

    let mut rt = Runtime::new((), view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    let status = |rt: &Runtime<()>| {
        rt.elements().into_iter().find(|e| e.key.as_deref() == Some("status")).and_then(|e| e.text)
    };
    assert_eq!(status(&rt).as_deref(), Some("r0=true"));

    // Scroll far enough down that `r0` leaves the mounted window; `settle_layout_reads` sees
    // `cx.layout("r0")` become `None` and re-runs `view` to update `status`.
    rt.wheel(150.0, 120.0, -3000.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(status(&rt).as_deref(), Some("r0=false"));
}

#[test]
fn virtual_remount_preserves_text_selection_across_scrolled_out_rows() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        VIEWS.set(VIEWS.get() + 1);
        let list = col()
            .key("rows")
            .w_full()
            .selection_group(true)
            .growth(GrowthSpec::default())
            .virtual_rows(
                200,
                32.0,
                |_: &(), i| Cow::Owned(format!("r{i}")),
                |_: &(), i| text(format!("Line {i}")).key("txt").selectable(true).h(32.0),
            );
        col().w_full().h_full().child(
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}

    // Drag-select starting at x=0 in row 0, then scroll row 0 out of the mounted window while dragging.
    rt.pointer_down(0.0, 10.0);
    rt.pointer_move(120.0, 50.0);
    rt.frame(1.0 / 120.0);
    for _ in 0..10 {
        let (views_ran, _) = take_views(|| {
            rt.wheel(150.0, 100.0, -300.0);
            rt.pointer_move(120.0, 150.0);
            rt.frame(1.0 / 120.0);
        });
        assert_eq!(views_ran, 0);
    }
    rt.pointer_up(120.0, 150.0);
    rt.frame(1.0 / 120.0);
    assert!(rt.find("r0").is_none(), "r0 scrolled out of the mounted window");
    let copied = rt.selected_text().unwrap_or_default();
    assert!(copied.starts_with("Line 0\n"), "pinned row 0 stays in selection: {copied:?}");
}

#[test]
fn virtual_remount_keeps_view_and_mounted_row_memos_and_evicts_culled_ones() {
    thread_local! {
        static HEADER_BUILDS: Cell<u32> = const { Cell::new(0) };
        static ROW_MEMO_BUILDS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
    }
    struct S {
        tick: u32,
        clicked: Option<usize>,
        edit: crate::TextEdit,
    }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let _ = s.tick;
        let header = memo("hdr", 0u32, || {
            HEADER_BUILDS.set(HEADER_BUILDS.get() + 1);
            text("Header").h(24.0)
        });
        let list = col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
            300,
            32.0,
            |_: &S, i| Cow::Owned(format!("r{i}")),
            |s: &S, i| {
                memo(format!("m{i}"), 0u32, || {
                    ROW_MEMO_BUILDS.with_borrow_mut(|b| b.push(i));
                    row()
                        .w_full()
                        .h(32.0)
                        .on_press(move |s: &mut S| s.clicked = Some(i))
                        .children([
                            text(format!("Row {i}")).grow(1.0),
                            crate::text_input(&s.edit, crate::TextStyle::new(13.0, 400.0)).w(60.0),
                        ])
                })
            },
        );
        col().w_full().h_full().children([
            header,
            col()
                .key("scroll")
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(list),
        ])
    }

    let mut rt = Runtime::new(
        S { tick: 0, clicked: None, edit: crate::TextEdit::default() },
        view,
        (320.0, 220.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    while rt.frame(1.0 / 120.0) {}
    assert_eq!(HEADER_BUILDS.get(), 1);

    // Scroll far down across many windows: non-reusable `text_input` row memos that leave must be
    // evicted, while `hdr` and currently mounted rows stay cached when `tick` finally runs `view`.
    for _ in 0..30 {
        rt.wheel(160.0, 120.0, -240.0);
        rt.frame(1.0 / 120.0);
    }
    while rt.frame(1.0 / 120.0) {}

    let mounted = rt.virtuals["#0/scroll/rows"].range.clone();
    assert!(mounted.start > 50, "scrolled past early rows: {mounted:?}");
    ROW_MEMO_BUILDS.with_borrow_mut(Vec::clear);

    // A state mutation runs `view`: neither `hdr` nor any row already in `mounted` rebuilds.
    rt.update(|s, _| s.tick += 1);
    rt.frame(1.0 / 120.0);
    assert_eq!(HEADER_BUILDS.get(), 1, "view-level memo survived partial commits");
    let rebuilt: Vec<usize> = ROW_MEMO_BUILDS
        .with_borrow_mut(std::mem::take)
        .into_iter()
        .filter(|i| mounted.contains(i))
        .collect();
    assert!(rebuilt.is_empty(), "mounted row memos survived partial commits, got {rebuilt:?}");
    // And culled early rows (`0..mounted.start`) were evicted rather than accumulating in MemoCache.
    assert!(
        rt.memo.len() <= rt.virtuals["#0/scroll/rows"].range.len() + 2,
        "culled row memos evicted: memo len {}, mounted {:?}",
        rt.memo.len(),
        rt.virtuals["#0/scroll/rows"].range
    );

    // Press an on-screen scrolled-in row and confirm hit testing + handler dispatch works.
    let (expected_idx, vis_row) = rt
        .elements()
        .into_iter()
        .find_map(|e| {
            let idx = e.key.as_deref()?.strip_prefix('r')?.parse::<usize>().ok()?;
            (e.visible[3] >= 20.0).then_some((idx, e))
        })
        .expect("on-screen row");
    let (cx, cy) = (vis_row.visible[0] + 10.0, vis_row.visible[1] + 10.0);
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    rt.pointer_up(cx, cy);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.clicked, Some(expected_idx));
}
