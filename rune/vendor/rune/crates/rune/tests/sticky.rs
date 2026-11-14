//! Headless tests for `El::sticky_top`, `El::sticky_bottom`, `El::stuck_shadow`,
//! `ElementInfo::stuck`, and `Tx::keep_on_screen`.
use rune::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use rune::runtime::ElementInfo;
use rune::{
    Align, Color, Cx, El, Env, GrowthSpec, Runtime, ScrollSpec, Shadow, Side, Tx, col, el, memo,
    oklch, row, text,
};

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key))
        .unwrap_or_else(|| panic!("missing element {key:?}"))
}

#[derive(Default)]
struct Basic {
    drawn_y: f32,
    hits: Vec<&'static str>,
}

fn basic_view(_: &Basic, _: &mut Cx<Basic>) -> El {
    let shadow = Shadow { y: 1.0, color: Color::BLACK.alpha(0.75), ..Default::default() };
    col()
        .w_full()
        .h_full()
        .child(
            col()
                .key("scroll")
                .w(300.0)
                .h(200.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(
                    col()
                        .w_full()
                        .child(el().key("lead").w_full().h(12.0))
                        // Section 0: 12..272, bottom padding 10 so content box ends at 262.
                        .child(
                            col()
                                .key("sec-0")
                                .w_full()
                                .pb(10.0)
                                .child(
                                    col()
                                        .key("hdr-0")
                                        .w_full()
                                        .h(30.0)
                                        .fill(oklch(0.24, 0.02, 260.0))
                                        .sticky_top(8.0)
                                        .stuck_shadow(shadow)
                                        .on_drawn_layout(|s: &mut Basic, r| s.drawn_y = r[1])
                                        .on_press(|s: &mut Basic| s.hits.push("hdr-0"))
                                        .child(
                                            el().key("hdr-0-btn")
                                                .w(60.0)
                                                .h(20.0)
                                                .on_press(|s: &mut Basic| s.hits.push("hdr-0-btn")),
                                        ),
                                )
                                .child(
                                    el().key("row-0")
                                        .w_full()
                                        .h(220.0)
                                        .on_press(|s: &mut Basic| s.hits.push("row-0")),
                                ),
                        )
                        // Section 1: 260..554, with both a top sticky header and a bottom sticky footer.
                        .child(
                            col()
                                .key("sec-1")
                                .w_full()
                                .child(
                                    el().key("hdr-1")
                                        .w_full()
                                        .h(30.0)
                                        .fill(oklch(0.28, 0.02, 260.0))
                                        .sticky_top(8.0),
                                )
                                .child(el().key("row-1").w_full().h(240.0))
                                .child(
                                    el().key("ftr-1")
                                        .w_full()
                                        .h(24.0)
                                        .fill(oklch(0.28, 0.02, 260.0))
                                        .sticky_bottom(6.0),
                                ),
                        )
                        .child(el().key("tail").w_full().h(300.0)),
                ),
        )
}

#[test]
fn sticky_top_and_bottom_hold_push_out_hit_above_siblings_and_report_drawn_layout() {
    let mut rt = Runtime::new(Basic::default(), basic_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // At offset 0, hdr-0 sits at y=12 (below its 8pt inset), so it is not stuck yet.
    // Run one more frame so the initial `on_drawn_layout` action dispatches.
    rt.frame(1.0 / 120.0);
    let h0 = find(&rt, "hdr-0");
    assert_eq!(h0.rect, [0.0, 12.0, 300.0, 30.0]);
    assert_eq!(h0.stuck, 0.0);
    assert_eq!(rt.state.drawn_y, 12.0);
    let base_instances = rt.instances.len();

    // Fractional scroll at 120 Hz: offset = 16.5, natural top = -4.5 < 8.0, so it sticks at 8.0
    // with shift = 12.5 in the same frame as wheel().
    rt.wheel(100.0, 100.0, -16.5);
    rt.frame(1.0 / 120.0);
    let h0 = find(&rt, "hdr-0");
    assert!((h0.rect[1] - 8.0).abs() < 1e-3, "held at inset 8: {:?}", h0.rect);
    assert!((h0.stuck - 12.5).abs() < 1e-3, "stuck shift is 12.5: {}", h0.stuck);
    assert!(rt.instances.len() > base_instances, "stuck_shadow draws while stuck");
    rt.frame(1.0 / 120.0);
    assert!((rt.state.drawn_y - 8.0).abs() < 1e-3, "on_drawn_layout sees the stuck y");

    // Scroll to offset = 92: row-0 now spans -50..170, directly under hdr-0 at y=8..38.
    rt.wheel(100.0, 100.0, -75.5);
    rt.frame(1.0 / 120.0);
    let h0 = find(&rt, "hdr-0");
    let btn = find(&rt, "hdr-0-btn");
    assert_eq!(h0.rect[1], 8.0);
    assert_eq!(h0.stuck, 88.0);
    assert_eq!(btn.rect[1], 8.0, "children inside the header move with it");

    // Clicking the button and the header body over row-0 hits the header subtree, not row-0.
    rt.pointer_down(20.0, 15.0);
    rt.pointer_up(20.0, 15.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hits, vec!["hdr-0-btn"]);

    rt.pointer_down(200.0, 20.0);
    rt.pointer_up(200.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.hits, vec!["hdr-0-btn", "hdr-0"]);

    // sec-0's content box ends at 262 (272 - 10pt bottom padding). Once offset > 262 - 8 - 30 = 224,
    // sec-0's bottom edge pushes hdr-0 up. At offset = 242, hdr-0's bottom is at 262 - 242 = 20, so y = -10.
    rt.wheel(100.0, 100.0, -150.0);
    rt.frame(1.0 / 120.0);
    let h0 = find(&rt, "hdr-0");
    assert_eq!(h0.rect[1], -10.0, "pushed up by sec-0's padded bottom edge");
    assert_eq!(h0.stuck, 220.0);

    // At offset = 242, sec-1 spans 30..324 (height 294). Its footer (24pt tall, sticky_bottom(6.0))
    // naturally sits at 300..324, which is below 200 - 6 = 194, so it sticks at y = 170 (194 - 24).
    let f1 = find(&rt, "ftr-1");
    assert_eq!(f1.rect[1], 170.0, "footer held 6pt above viewport bottom");
    assert_eq!(f1.stuck, -130.0, "upward sticky shift is negative");
}

#[test]
fn nested_scroll_viewports_stick_each_header_to_its_nearest_viewport() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().child(
            col()
                .key("outer")
                .w(300.0)
                .h(240.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(
                    col()
                        .w_full()
                        .child(
                            col()
                                .key("outer-sec")
                                .w_full()
                                .child(el().key("outer-hdr").w_full().h(28.0).sticky_top(0.0))
                                .child(el().w_full().h(60.0))
                                .child(
                                    col()
                                        .key("inner")
                                        .w_full()
                                        .h(120.0)
                                        .scroll(ScrollSpec { stick_bottom: false })
                                        .child(
                                            col()
                                                .w_full()
                                                .child(
                                                    col()
                                                        .w_full()
                                                        .child(el().key("inner-hdr").w_full().h(20.0).sticky_top(4.0))
                                                        .child(el().w_full().h(260.0)),
                                                ),
                                        ),
                                )
                                .child(el().w_full().h(300.0)),
                        ),
                ),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 240.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(find(&rt, "outer-hdr").rect[1], 0.0);
    assert_eq!(find(&rt, "inner").rect[1], 88.0);
    assert_eq!(find(&rt, "inner-hdr").rect[1], 92.0);

    // Scroll inner viewport (at y=120): inner-hdr sticks at inner.top + 4 = 92.
    rt.wheel(150.0, 120.0, -50.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "inner-hdr").rect[1], 92.0);
    assert_eq!(find(&rt, "inner-hdr").stuck, 54.0);
    assert_eq!(find(&rt, "outer-hdr").stuck, 0.0);

    // Scroll outer viewport (at y=10, outside inner): outer-hdr sticks at 0, inner moves up by 30,
    // and inner-hdr stays stuck to inner's new top + 4 = 62.
    rt.wheel(150.0, 10.0, -30.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "outer-hdr").rect[1], 0.0);
    assert_eq!(find(&rt, "outer-hdr").stuck, 30.0);
    assert_eq!(find(&rt, "inner").rect[1], 58.0);
    assert_eq!(find(&rt, "inner-hdr").rect[1], 62.0);
}

#[derive(Default)]
struct CollapseApp {
    collapsed: [bool; 8],
}

fn virtual_sections(_: &CollapseApp, _: &mut Cx<CollapseApp>) -> El {
    col().w_full().h_full().child(
        col()
            .key("scroll")
            .w(320.0)
            .h(200.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .child(
                col()
                    .key("list")
                    .w_full()
                    .growth(GrowthSpec::default())
                    .virtual_rows(
                        8,
                        160.0,
                        |_: &CollapseApp, i| format!("sec-{i}").into(),
                        |s: &CollapseApp, i| {
                            let collapsed = s.collapsed[i];
                            let hdr_key = format!("hdr-{i}");
                            let keep_key = hdr_key.clone();
                            let mut sec = col().w_full().child(
                                el().key(hdr_key)
                                    .w_full()
                                    .h(32.0)
                                    .fill(oklch(0.25, 0.02, 260.0))
                                    .sticky_top(0.0)
                                    .on_press_tx(move |s: &mut CollapseApp, tx: &mut Tx| {
                                        s.collapsed[i] = !s.collapsed[i];
                                        tx.keep_on_screen(&keep_key);
                                    }),
                            );
                            if !collapsed {
                                sec = sec.child(el().key(format!("body-{i}")).w_full().h(600.0));
                            }
                            sec
                        },
                    ),
            ),
    )
}

#[test]
fn virtual_rows_keep_stuck_header_on_screen_across_collapse_and_expand() {
    let mut rt = Runtime::new(CollapseApp::default(), virtual_sections, (320.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Scroll deep into sec-1 (starts at 632, ends at 1264): offset = 1050 puts us 418pt into sec-1.
    rt.wheel(160.0, 100.0, -1050.0);
    for _ in 0..4 {
        rt.frame(1.0 / 120.0);
    }
    let h1 = find(&rt, "hdr-1");
    assert_eq!(h1.rect[1], 0.0, "hdr-1 is stuck at the top of the viewport");
    assert!((h1.stuck - 418.0).abs() < 0.5, "hdr-1 stuck shift: {}", h1.stuck);

    // Click hdr-1 (at y=16) to collapse sec-1 from 632pt tall to 32pt tall.
    // Without keep_on_screen, sec-1 (632..664) would end 386pt above offset 1050 and be culled.
    rt.pointer_down(100.0, 16.0);
    rt.pointer_up(100.0, 16.0);
    for step in 0..10 {
        rt.frame(1.0 / 120.0);
        let h1 = find(&rt, "hdr-1");
        assert!((h1.rect[1] - 0.0).abs() < 0.5, "frame {step}: hdr-1 stayed at y=0 after collapse: {:?}", h1.rect);
    }
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert_eq!(find(&rt, "hdr-2").rect[1], 32.0, "sec-2 sits immediately below collapsed sec-1");

    // Click hdr-1 again to expand sec-1: hdr-1 stays at y=0 while its body opens below it.
    rt.pointer_down(100.0, 16.0);
    rt.pointer_up(100.0, 16.0);
    for step in 0..10 {
        rt.frame(1.0 / 120.0);
        let h1 = find(&rt, "hdr-1");
        assert!((h1.rect[1] - 0.0).abs() < 0.5, "frame {step}: hdr-1 stayed at y=0 after expand: {:?}", h1.rect);
    }
}

#[derive(Default)]
struct Timeline {
    collapsed: bool,
    extra_rows: usize,
    menu_open: bool,
}

fn timeline_view(s: &Timeline, _: &mut Cx<Timeline>) -> El {
    let mut group = col().key("group").w_full().child(
        el().key("group-hdr")
            .w_full()
            .h(30.0)
            .fill(oklch(0.24, 0.02, 260.0))
            .sticky_top(0.0)
            .on_press_tx(|s: &mut Timeline, tx: &mut Tx| {
                s.collapsed = !s.collapsed;
                tx.keep_on_screen("group-hdr");
            }),
    );
    if !s.collapsed {
        group = group.child(el().key("group-body").w_full().h(400.0));
    }
    let mut stack = col()
        .key("stack")
        .w_full()
        .growth(GrowthSpec::default())
        .child(el().key("row-a").w_full().h(200.0))
        .child(group)
        .child(el().key("row-b").w_full().h(120.0));
    for i in 0..s.extra_rows {
        stack = stack.child(text(format!("line {i}")).key(format!("extra-{i}")).w_full().h(50.0));
    }
    let mut root = col().w_full().h_full().child(
        col()
            .key("scroll")
            .w(300.0)
            .h(200.0)
            .scroll(ScrollSpec { stick_bottom: true })
            .child(stack),
    );
    if s.menu_open {
        root = root.child(
            el().key("popover")
                .w(100.0)
                .h(40.0)
                .anchored("group-hdr", Side::Bottom, Align::Start, 4.0),
        );
    }
    root
}

#[test]
fn stick_bottom_growth_reveal_and_anchored_overlay_work_with_sticky_headers() {
    let init = Timeline { collapsed: false, extra_rows: 2, menu_open: false };
    let mut rt = Runtime::new(init, timeline_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);

    // Opens pinned at the bottom (total height = 200 + 430 + 120 + 100 = 850, viewport = 200 -> offset = 650).
    // Scroll back into `group` (200..630) at offset = 450 so `group-hdr` is stuck at y=0, and open an
    // anchored overlay on `group-hdr`.
    rt.wheel(150.0, 100.0, 200.0);
    rt.update(|s, _| s.menu_open = true);
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "group-hdr").rect[1], 0.0);
    assert_eq!(find(&rt, "popover").rect[1], 34.0, "anchored overlay sits 4pt below the stuck header");

    // Scrolling further while the overlay is open keeps the overlay attached to the stuck header,
    // and once the header is pushed up (at offset = 610, group ends at 630 -> hdr at -10), the
    // overlay moves with it (to -10 + 30 + 4 = 24).
    rt.wheel(150.0, 100.0, -160.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "group-hdr").rect[1], -10.0);
    assert_eq!(find(&rt, "popover").rect[1], 24.0, "anchored overlay tracks the header as it pushes out");

    // At offset = 400 (with extra_rows cleared), `group-hdr` is stuck at y=0..30. Collapse `group`
    // (height 430 -> 30): total content becomes 200 + 30 + 120 = 350, max offset = 150.
    // `keep_on_screen("group-hdr")` wants offset 200 - 0 = 200, which clamps to max offset 150
    // and re-pins `stick_bottom`!
    rt.update(|s, tx| {
        s.menu_open = false;
        s.extra_rows = 0;
        tx.scroll_to_y("scroll", 400.0);
    });
    for _ in 0..40 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(find(&rt, "group-hdr").rect[1], 0.0);
    rt.pointer_down(100.0, 15.0);
    rt.pointer_up(100.0, 15.0);
    rt.frame(1.0 / 120.0);

    // The collapse now keeps its spring; let the end clamp land before appending.
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    // Because collapsing brought the scroll offset to `max` (10.0), `stick_bottom` is re-pinned:
    // appending new growth rows keeps the bottom in view!
    rt.update(|s, _| s.extra_rows = 2);
    for _ in 0..60 {
        rt.frame(1.0 / 60.0);
    }
    let last = find(&rt, "extra-1");
    assert!((last.rect[1] + last.rect[3] - 200.0).abs() < 1.0, "stick_bottom followed new growth rows: {:?}", last.rect);
}

#[test]
fn scroll_into_view_lands_row_clear_of_stuck_header() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().h_full().child(
            col()
                .key("scroll")
                .w(300.0)
                .h(200.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .child(
                    col()
                        .w_full()
                        .child(
                            col()
                                .w_full()
                                .child(el().key("hdr").w_full().h(40.0).sticky_top(0.0))
                                .child(el().key("r0").w_full().h(80.0))
                                .child(el().key("r1").w_full().h(80.0))
                                .child(el().key("r2").w_full().h(80.0))
                                .child(el().key("r3").w_full().h(80.0)),
                        ),
                ),
        )
    }

    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    // Scroll down to offset = 150: `hdr` is stuck at y=0..40, and `r1` (120..200) sits at y=-30..50
    // (its top is hidden behind `hdr` and above the viewport).
    rt.update(|_, tx| tx.scroll_to_y("scroll", 150.0));
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "hdr").rect[1], 0.0);
    assert_eq!(find(&rt, "r1").rect[1], -30.0);

    // Revealing `r1` scrolls far enough that `r1`'s top clears the 40pt stuck `hdr` (landing at y=40).
    rt.update(|_, tx| tx.scroll_into_view("r1"));
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "hdr").rect[1], 0.0);
    assert_eq!(find(&rt, "r1").rect[1], 40.0, "r1 lands clear of the 40pt stuck header");
}

#[test]
fn keep_follows_drawn_motion_without_snapping_other_rows_and_scroll_cancels_it() {
    fn view(s: &bool, _: &mut Cx<bool>) -> El {
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().key("stack").w_full().growth(GrowthSpec::default())
                .child(el().key("before").w_full().h(if *s { 120.0 } else { 240.0 }))
                .child(el().key("kept").w_full().h(30.0))
                .child(el().key("other").w_full().h(if *s { 220.0 } else { 120.0 }))
                .child(el().w_full().h(600.0)))
    }
    let mut rt = Runtime::new(false, view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 220.0));
    rt.frame(0.0);
    let y = find(&rt, "kept").rect[1];
    rt.update(|s, tx| { *s = true; tx.keep_on_screen("stack/kept"); });
    rt.frame(1.0 / 120.0);
    let h = find(&rt, "other").rect[3];
    assert!(h > 120.0 && h < 220.0, "unrelated growth keeps its spring: {h}");
    for _ in 0..12 {
        rt.frame(1.0 / 120.0);
        assert!((find(&rt, "kept").rect[1] - y).abs() < 0.05);
    }
    rt.wheel(100.0, 100.0, -20.0);
    rt.frame(1.0 / 120.0);
    assert!(find(&rt, "kept").rect[1] < y - 19.0, "wheel wins over the keep");
    rt.update(|s, tx| {
        *s = false;
        tx.keep_on_screen("stack/kept");
        tx.scroll_to_instant("scroll", 0.0, 50.0);
    });
    rt.frame(1.0 / 120.0);
    assert_eq!(find(&rt, "before").rect[1], -50.0, "explicit scroll in the same handler wins");
}

#[test]
fn live_viewport_and_ancestor_transforms_keep_header_and_overlay_together() {
    struct S { offset: rune::Live<[f32; 2]>, scale: rune::Live<f32> }
    fn view(s: &S, _: &mut Cx<S>) -> El {
        col().w_full().h_full()
            .child(col().key("ancestor").size(300.0, 200.0).live_offset(&s.offset).live_scale(&s.scale)
                .child(col().key("scroll").size(300.0, 200.0).live_offset(&s.offset).live_scale(&s.scale).scroll(ScrollSpec { stick_bottom: false })
                    .child(col().w_full()
                        .child(el().key("hdr").w_full().h(30.0).sticky_top(8.0))
                        .child(el().w_full().h(600.0)))))
            .child(el().key("tip").size(60.0, 20.0).anchored("hdr", Side::Bottom, Align::Start, 4.0))
    }
    let mut rt = Runtime::new(S { offset: rune::Live::new([0.0, 0.0]), scale: rune::Live::new(1.0) }, view, (500.0, 400.0), Env::default());
    rt.frame(0.0);
    rt.wheel(100.0, 100.0, -100.0);
    rt.frame(0.0);
    rt.state.offset.set([30.0, 40.0]);
    rt.state.scale.set(0.8);
    rt.frame(1.0 / 120.0);
    let (h, v, tip) = (find(&rt, "hdr"), find(&rt, "scroll"), find(&rt, "tip"));
    assert!((h.rect[1] - v.rect[1] - 8.0 * 0.8 * 0.8).abs() < 0.05, "{h:?} {v:?}");
    assert!((tip.rect[1] - h.rect[1] - h.rect[3] - 4.0).abs() < 0.05, "overlay tracks drawn header: {:?} {:?}", tip.rect, h.rect);
}

#[test]
fn inert_sticky_passes_input_and_focus_through_and_tooltips_follow_drawn_rect() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("scroll").size(300.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full()
                .child(el().key("hdr").w_full().h(30.0).sticky_top(0.0).help("header").focusable(true))
                .child(el().key("inert").absolute(0.0, 0.0).size(300.0, 30.0).sticky_top(0.0).inert().focusable(true))
                .child(el().w_full().h(600.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel(100.0, 100.0, -100.0);
    rt.frame(0.0);
    rt.set_full_keyboard_access(true);
    rt.key(rune::KeyEvent { key: rune::Key::Tab, mods: rune::Mods::default() });
    rt.frame(0.0);
    assert!(find(&rt, "hdr").focused);
    assert!(!find(&rt, "inert").focused);
    rt.pointer_move(100.0, 15.0);
    rt.frame(0.0);
    rt.frame(0.6);
    assert_eq!(rt.help_tooltip().map(|t| t.0), Some("header"));
    let tip = rt.help_tooltip().unwrap().1;
    assert!((tip[1] - find(&rt, "hdr").rect[3] - 6.0).abs() < 0.05, "tooltip follows stuck header: {tip:?}");
}

#[test]
fn sticky_without_vertical_viewport_is_a_no_op() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(300.0, 200.0).scroll_x()
            .child(col().w(600.0)
                .child(el().key("top").size(50.0, 30.0).sticky_top(20.0))
                .child(el().key("bottom").size(50.0, 30.0).sticky_bottom(20.0)))
    }
    let mut rt = Runtime::new((), view, (300.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel_xy(100.0, 100.0, -50.0, 0.0);
    rt.frame(0.0);
    assert_eq!(find(&rt, "top").stuck, 0.0);
    assert_eq!(find(&rt, "bottom").stuck, 0.0);
}

#[derive(Clone, Copy, Debug)]
struct RevealGrowth {
    expanded: bool,
    stick_bottom: bool,
    virtualized: bool,
    keep: Option<&'static str>,
    instant: bool,
}

fn reveal_growth_view(s: &RevealGrowth, _: &mut Cx<RevealGrowth>) -> El {
    let build_row = |s: &RevealGrowth, i: usize| -> El {
        let key = format!("r{i}");
        let h = if i == 3 {
            if s.expanded { 130.0 } else { 37.0 }
        } else if i < 3 {
            80.0
        } else {
            35.0
        };
        let mut row = el().key(key).w_full().h(h);
        if i == 3 {
            row = row.on_press_tx(|s: &mut RevealGrowth, tx: &mut Tx| {
                s.expanded = true;
                if s.instant {
                    tx.instant();
                }
                if let Some(target) = s.keep {
                    tx.keep_on_screen(target);
                } else {
                    tx.scroll_into_view("r3");
                }
            });
        }
        row
    };
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default());
    if s.virtualized {
        stack = stack.virtual_rows(
            6,
            60.0,
            |_: &RevealGrowth, i| format!("r{i}").into(),
            move |s: &RevealGrowth, i| build_row(s, i),
        );
    } else {
        for i in 0..6 {
            stack = stack.child(build_row(s, i));
        }
    }
    col().w_full().h_full().child(
        col()
            .key("scroll")
            .w(300.0)
            .h(200.0)
            .scroll(ScrollSpec { stick_bottom: s.stick_bottom })
            .child(stack),
    )
}

fn run_reveal_growth(init: RevealGrowth) {
    let mut rt = Runtime::new(init, reveal_growth_view, (300.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    if !init.stick_bottom {
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 120.0));
        rt.frame(0.0);
    }
    let kept_was = init.keep.map(|k| find(&rt, k).rect[1]);
    let snapshot = |rt: &Runtime<RevealGrowth>| -> Vec<(String, [f32; 4])> {
        (0..6)
            .filter_map(|i| {
                let key = format!("r{i}");
                rt.elements().into_iter().find(|e| e.key.as_deref() == Some(&key)).map(|e| (key, e.rect))
            })
            .collect()
    };
    let mut prev = snapshot(&rt);
    let r3 = find(&rt, "r3").rect;
    let press_y = r3[1] + r3[3] * 0.5;
    rt.pointer_down(150.0, press_y);
    rt.pointer_up(150.0, press_y);

    if init.instant {
        rt.frame(1.0 / 120.0);
        let r3 = find(&rt, "r3").rect;
        assert!((r3[3] - 130.0).abs() < 0.05, "instant snaps growth in one frame: {init:?} {r3:?}");
        if let (Some(k), Some(was)) = (init.keep, kept_was) {
            assert!((find(&rt, k).rect[1] - was).abs() < 0.05, "instant keep lands in one frame: {init:?}");
        } else {
            assert!(r3[1] >= -0.05 && (r3[1] + r3[3] - 200.0).abs() < 0.05, "instant reveal lands in one frame: {init:?} {r3:?}");
        }
        assert!(!rt.animating, "instant leaves nothing animating: {init:?}");
        return;
    }

    let mut max_move = 0.0f32;
    let mut growth_frames = 0usize;
    for step in 1..=120 {
        rt.frame(1.0 / 120.0);
        let cur = snapshot(&rt);
        for (key, rect) in &cur {
            if let Some((_, old)) = prev.iter().find(|(k, _)| k == key) {
                let dy_top = (rect[1] - old[1]).abs();
                let dy_bot = ((rect[1] + rect[3]) - (old[1] + old[3])).abs();
                let dh = (rect[3] - old[3]).abs();
                let m = dy_top.max(dy_bot).max(dh);
                max_move = max_move.max(m);
                assert!(
                    m <= 12.0,
                    "frame {step} ({:?}): {key} moved {m:.2}pt in one frame ({old:?} -> {rect:?})",
                    (init.stick_bottom, init.virtualized, init.keep)
                );
            }
        }
        if let (Some(k), Some(was)) = (init.keep, kept_was) {
            let y = find(&rt, k).rect[1];
            assert!(
                (y - was).abs() < 0.5,
                "frame {step} ({:?}): kept {k} moved from {was:.2} to {y:.2}",
                (init.stick_bottom, init.virtualized, init.keep)
            );
        }
        if (find(&rt, "r3").rect[3] - 130.0).abs() >= 0.5 {
            growth_frames = step;
        }
        prev = cur;
    }
    let duration = growth_frames as f32 / 120.0;
    assert!(
        (0.20..=0.50).contains(&duration),
        "growth took {duration:.3}s ({growth_frames} frames, max step {max_move:.2}pt) for {:?}",
        (init.stick_bottom, init.virtualized, init.keep)
    );
    let r3 = find(&rt, "r3").rect;
    assert!((r3[3] - 130.0).abs() < 0.05, "r3 settled at 130pt: {r3:?}");
    if let (Some(k), Some(was)) = (init.keep, kept_was) {
        assert!((find(&rt, k).rect[1] - was).abs() < 0.5, "kept {k} stayed at {was}: {:?}", find(&rt, k).rect);
    } else {
        assert!(
            r3[1] >= -0.05 && (r3[1] + r3[3] - 200.0).abs() < 0.5,
            "final scroll reveals grown r3: {r3:?}"
        );
    }
}

#[test]
fn scroll_into_view_keeps_growth_springing_and_snaps_when_instant() {
    for stick_bottom in [false, true] {
        for virtualized in [false, true] {
            for instant in [false, true] {
                run_reveal_growth(RevealGrowth { expanded: false, stick_bottom, virtualized, keep: None, instant });
            }
        }
    }
}

#[test]
fn keep_on_screen_keeps_growth_springing_and_snaps_when_instant() {
    for stick_bottom in [false, true] {
        for virtualized in [false, true] {
            for keep in [Some("r4"), Some("r3")] {
                for instant in [false, true] {
                    run_reveal_growth(RevealGrowth { expanded: false, stick_bottom, virtualized, keep, instant });
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
struct OccludeState {
    memoized: bool,
    open: bool,
}

fn occlude_view(s: &OccludeState, _: &mut Cx<OccludeState>) -> El {
    let build_group = |open: bool| {
        let mut head = row()
            .key("head")
            .w_full()
            .h(24.0)
            .animate_layout()
            .child(text("Used 3 tools").color(Color::WHITE));
        if open {
            head = head
                .sticky_top(0.0)
                .fill(Color { r: 0.86, g: 0.2, b: 0.2, a: 1.0 })
                .stuck_shadow(Shadow {
                    y: 8.0,
                    blur: 8.0,
                    color: Color::BLACK,
                    ..Default::default()
                });
        }
        let mut g = col().key("group").w_full().pb(12.0).child(head);
        if open {
            let mut items = col().key("items").w_full().pt(4.0);
            for j in 0..16 {
                // Bright green rows that scroll directly under the red stuck header.
                items = items.child(
                    el().key(format!("item-{j}"))
                        .w_full()
                        .h(22.0)
                        .fill(Color { r: 0.0, g: 0.94, b: 0.16, a: 1.0 }),
                );
            }
            g = g.child(items);
        }
        g
    };
    let open = s.open;
    let memoized = s.memoized;
    let stack = col()
        .key("timeline")
        .w_full()
        .growth(GrowthSpec::default())
        .virtual_rows(
            4,
            72.0,
            |_: &OccludeState, i| format!("row-{i}").into(),
            move |_: &OccludeState, i| {
                if i != 1 {
                    return el().key(format!("spacer-{i}")).w_full().h(60.0);
                }
                if memoized {
                    memo("row-1", open, move || {
                        col()
                            .w_full()
                            .max_w(200.0)
                            .streaming(false)
                            .child(build_group(open))
                    })
                } else {
                    col()
                        .w_full()
                        .max_w(200.0)
                        .streaming(false)
                        .child(build_group(open))
                }
            },
        );
    col()
        .w_full()
        .h_full()
        .fill(Color { r: 0.12, g: 0.125, b: 0.14, a: 1.0 })
        .child(
            col()
                .key("scroll")
                .w(200.0)
                .h(200.0)
                .scroll(ScrollSpec { stick_bottom: false })
                .scroll_edge_fade(16.0)
                .child(stack),
        )
}

#[test]
fn sticky_header_stays_opaque_and_occludes_scrolled_rows_under_edge_fade_and_memo() {
    let scale = 2.0;
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let size = ((200.0 * scale) as u32, (200.0 * scale) as u32);
    let mut render_pixels = |memoized: bool| -> Vec<u8> {
        let mut rt = Runtime::new(
            OccludeState { memoized, open: false },
            occlude_view,
            (200.0, 200.0),
            Env::new(true, scale),
        );
        rt.frame(0.0);
        // Expand the group (matching a chat timeline's collapsible tool group inside `memo`)
        // and settle the growth animation.
        rt.update(|s, _| s.open = true);
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        // Scroll 135 pt so `head` sticks at y=0..24 and bright green items sit at y=-3..19
        // directly behind the top edge-fade band (y=0..16).
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 135.0));
        for _ in 0..120 {
            rt.frame(1.0 / 60.0);
        }
        let head = find(&rt, "head");
        assert_eq!(head.rect, [0.0, 0.0, 200.0, 24.0]);
        assert!((head.stuck - 75.0).abs() < 0.5, "stuck shift: {}", head.stuck);

        let before = pixels(&gpu, &mut renderer, &mut rt, size);
        // Rebuild with an unchanged memo version, then scroll away and back. A memo hit
        // must not replay paint geometry, clips or shadow state captured on an earlier frame.
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 80.0));
        rt.frame(0.0);
        rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 135.0));
        rt.frame(0.0);
        let after = pixels(&gpu, &mut renderer, &mut rt, size);
        assert_eq!(before, after, "unchanged memo replay changed sticky fill/shadow pixels");
        after
    };

    let plain = render_pixels(false);
    let memoed = render_pixels(true);
    let px = |buf: &[u8], x_pt: f32, y_pt: f32| -> [u8; 4] {
        let x = (x_pt * scale) as usize;
        let y = (y_pt * scale) as usize;
        let idx = (y * size.0 as usize + x) * 4;
        [buf[idx], buf[idx + 1], buf[idx + 2], buf[idx + 3]]
    };

    for (label, buf) in [("plain", &plain), ("memo", &memoed)] {
        // Sample at x=160pt (to the right of the header's "Used 3 tools" text).
        let top_row = px(buf, 160.0, 1.5);
        let mid_row = px(buf, 160.0, 8.0);
        let bot_row = px(buf, 160.0, 20.0);
        // Problem 2: the top pixels of the stuck header (y = 1..2) must not be faded by
        // `scroll_edge_fade(16.0)`; they match the unfaded bottom of the header.
        assert_eq!(
            top_row, bot_row,
            "{label}: top of stuck header at y=1.5pt was faded ({top_row:?} vs {bot_row:?})"
        );
        assert_eq!(mid_row, bot_row, "{label}: middle of stuck header at y=8pt ({mid_row:?} vs {bot_row:?})");
        // Problem 1: the bright green scrolled-under row must not bleed through the red header fill
        // (whose g and b channels are equal).
        assert!(
            top_row[0] > 200 && top_row[1] == top_row[2] && top_row[3] == 255,
            "{label}: scrolled green row bled through stuck header: {top_row:?}"
        );
        // `stuck_shadow` darkens the row immediately below the 24pt header (y=26pt vs y=38pt).
        let under_shadow = px(buf, 160.0, 26.0);
        let clear_of_shadow = px(buf, 160.0, 38.0);
        assert!(
            under_shadow[1] < clear_of_shadow[1].saturating_sub(25),
            "{label}: stuck_shadow did not darken below header ({under_shadow:?} vs {clear_of_shadow:?})"
        );
    }
    // Problem 3: wrapping the row in `memo` produces identical pixels for the stuck header and shadow.
    assert_eq!(plain, memoed, "memoized sticky header pixels differ from unmemoized row");
}

#[test]
fn sticky_header_inside_memo_growth_row_respects_animated_row_height() {
    let mut rt = Runtime::new(
        OccludeState { memoized: true, open: true },
        occlude_view,
        (200.0, 200.0),
        Env::new(true, 1.0),
    );
    rt.frame(0.0);
    // Scroll to 380pt so the bottom of the expanded group pushes the header near the top.
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 380.0));
    rt.frame(0.0);
    // Collapse the group inside `memo`; on the first animation frame the outer `memo` wrapper row's
    // growth height `g.h` is shrinking, and the nested group's sticky bound must follow it.
    rt.update(|s, _| s.open = false);
    rt.frame(1.0 / 120.0);
    rt.update(|s, _| s.open = true);
    rt.frame(1.0 / 120.0);
    let wrapper = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row-1")).unwrap();
    let head = find(&rt, "head");
    assert!(
        head.rect[1] + head.rect[3] <= wrapper.rect[1] + wrapper.rect[3] + 0.5,
        "sticky header bottom ({:?}) exceeded animated memo row bottom ({:?})",
        head.rect,
        wrapper.rect
    );
}

#[derive(Clone, Copy)]
struct ThreeColumns { rows: bool, alpha: f32 }

fn three_columns(s: &ThreeColumns, _: &mut Cx<ThreeColumns>) -> El {
    let mut root = row().size(700.0, 320.0).padding(20.0).gap(20.0).fill(Color::BLACK);
    for column in 0..3 {
        let state = *s;
        let section = move || {
            let mut section = col().key("section").w_full().child(
                row().key("header").w_full().h(24.0)
                    .fill(Color { r: 1.0, g: 0.0, b: 0.0, a: state.alpha })
                    .sticky_top(0.0).child(text("header").color(Color::WHITE)),
            );
            for i in 0..20 {
                section = section.child(row().h(24.0).child(
                    text(if state.rows { format!("row {i}") } else { String::new() })
                        .color(Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 }),
                ));
            }
            section
        };
        let build = move |_: &ThreeColumns, i| match i {
            0 => el().w_full().h(10.0),
            1 if column == 0 => col().key("section-row").w_full().child(section()),
            1 => memo("section-row", 0, move || col().w_full().child(section())),
            _ => el().w_full().h(400.0),
        };
        let mut stack = col().w_full().pt(40.0).growth(GrowthSpec::default());
        if column != 1 {
            stack = stack.virtual_rows(3, 72.0, |_: &ThreeColumns, i| format!("r{i}").into(), build);
        } else {
            for i in 0..3 { stack = stack.child(build(s, i)); }
        }
        root = root.child(col().key(format!("scroll-{column}")).size(200.0, 280.0)
            .scroll(ScrollSpec { stick_bottom: false })
            .scroll_edge_fade(if column == 2 { 0.0 } else { 16.0 }).child(stack));
    }
    root
}

fn pixels<S: 'static>(gpu: &Gpu, renderer: &mut Renderer, rt: &mut Runtime<S>, size: (u32, u32)) -> Vec<u8> {
    let target = Offscreen::new(&gpu.device, size.0, size.1);
    renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, size);
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    renderer.draw(&mut encoder, &target.view, [0.0; 4]);
    target.copy(&mut encoder);
    gpu.queue.submit([encoder.finish()]);
    target.read(&gpu.device)
}

#[test]
fn sticky_three_column_repro_orders_rows_before_opaque_or_translucent_headers() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    let mut differences = Vec::new();
    for alpha in [1.0, 0.5] {
        let mut snapshots = Vec::new();
        for rows in [false, true] {
            let mut rt = Runtime::new(ThreeColumns { rows, alpha }, three_columns, (700.0, 320.0), Env::default());
            rt.frame(0.0);
            rt.update(|_, tx| {
                for column in 0..3 { tx.scroll_to_instant(format!("scroll-{column}"), 0.0, 150.0); }
            });
            for _ in 0..120 { rt.frame(1.0 / 120.0); }
            // Verify the stream itself, not just an opaque fill hiding a bad ordering.
            for column in 0..3 {
                let x = 20.0 + column as f32 * 220.0;
                let header = rt.instances.iter().position(|p| p.rect[0] == x && p.fill[0] > 0.0
                    && p.fill[1] == 0.0 && p.fill[2] == 0.0).unwrap();
                for (index, p) in rt.instances.iter().enumerate() {
                    if p.rect[0] >= x && p.rect[0] < x + 200.0 && p.fill[1] > 0.0 && p.fill[0] == 0.0 {
                        assert!(index < header, "column {column}, alpha {alpha}: row primitive {index} follows header {header}");
                    }
                }
            }
            snapshots.push(pixels(&gpu, &mut renderer, &mut rt, (700, 320)));
        }
        for column in 0..3 {
            let mut changed = 0;
            for y in 20..44 {
                for x in (20 + column * 220)..(220 + column * 220) {
                    let at = (y * 700 + x) * 4;
                    changed += usize::from(snapshots[0][at..at + 4] != snapshots[1][at..at + 4]);
                }
            }
            eprintln!("column {column}, alpha {alpha}: {changed} header pixels affected by underlying text");
            if alpha == 1.0 { differences.push(changed); }
        }
    }
    assert_eq!(differences, vec![0; 3], "opaque headers must occlude all three columns");
}

#[derive(Clone, Copy)]
struct FadeEdges { edge: usize, fade: f32, explicit: bool }

fn fade_edges(s: &FadeEdges, _: &mut Cx<FadeEdges>) -> El {
    let header = el().key("header").size(24.0, 24.0).fill(Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 });
    let header = match s.edge {
        0 => header.absolute(40.0, 40.0).sticky_top(0.0),
        1 => header.absolute(40.0, 536.0).sticky_bottom(0.0),
        _ => header.absolute(40.0, 40.0).sticky_left(0.0),
    };
    let clip = if s.explicit { [12.0; 4] } else { [0.0; 4] };
    col().size(200.0, 200.0).fill(Color::BLACK).clip_fade(clip[0], clip[1], clip[2], clip[3]).child(
        col().key("scroll").size(200.0, 200.0)
            .scroll(ScrollSpec { stick_bottom: false }).scroll_xy().scroll_edge_fade(s.fade)
            .child(col().size(600.0, 600.0).child(header)),
    )
}

#[test]
fn sticky_fade_exemption_is_continuous_on_each_axis_and_preserves_explicit_clips() {
    let gpu = Gpu::headless();
    let mut renderer = Renderer::new(&gpu.device, OFFSCREEN_FORMAT);
    for edge in 0..3 {
        for fade in [0.0, 16.0] {
            for explicit in [false, true] {
                let mut rt = Runtime::new(FadeEdges { edge, fade, explicit }, fade_edges, (200.0, 200.0), Env::default());
                rt.frame(0.0);
                // Approach, reach, then pass the sticky threshold in each direction. The
                // exemption is declarative, not gated on shift != 0: no opacity pop at 0.
                for distance in [20.0, 10.0, 1.0, 0.0, -1.0, -20.0] {
                    rt.update(move |_, tx| {
                        let offset = if edge == 1 { 360.0 + distance } else { 40.0 - distance };
                        tx.scroll_to_instant("scroll", if edge == 2 { offset } else { 0.0 }, if edge == 2 { 0.0 } else { offset });
                    });
                    rt.frame(0.0);
                    let header = find(&rt, "header");
                    if fade > 0.0 && !explicit {
                        let fill = rt.instances.iter().find(|p| p.fill[0] == 1.0 && p.fill[1] == 0.0).unwrap();
                        let widths = rune::render::unpack_fade(fill.fade).0;
                        let (sticking, opposite) = match edge { 0 => (1, 3), 1 => (3, 1), _ => (0, 2) };
                        assert_eq!(widths[sticking], 0.0);
                        assert_eq!(widths[opposite], 16.0, "only the sticking edge is exempt");
                    }
                    let (x, y) = match edge {
                        0 => (50, header.rect[1] as usize + 1),
                        1 => (50, header.rect[1] as usize + 22),
                        _ => (header.rect[0] as usize + 1, 50),
                    };
                    let image = pixels(&gpu, &mut renderer, &mut rt, (200, 200));
                    let red = image[(y * 200 + x) * 4];
                    if explicit && distance <= 1.0 {
                        assert!(red < 100, "explicit clip was erased: edge {edge}, fade {fade}, distance {distance}: {red}");
                    } else if !explicit {
                        assert_eq!(red, 255, "sticky edge faded or popped: edge {edge}, fade {fade}, distance {distance}");
                    }
                }
            }
        }
    }
}

#[test]
fn sticky_scroll_fade_preserves_an_intervening_explicit_clip() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(16.0)
            .child(col().w_full().clip_fade(0.0, 8.0, 0.0, 0.0)
                .child(el().key("header").w_full().h(24.0).fill(Color::WHITE).sticky_top(0.0))
                .child(el().w_full().h(600.0)))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|_, tx| tx.scroll_to_instant("scroll", 0.0, 100.0));
    rt.frame(0.0);
    let fill = rt.instances.iter().find(|p| p.fill == [1.0; 4]).unwrap();
    assert_eq!(rune::render::unpack_fade(fill.fade).0[1], 8.0, "remove the scroll fade, not the explicit inner clip");
}

#[test]
fn sticky_header_taller_than_a_new_growth_row_keeps_its_natural_size() {
    fn view(s: &bool, _: &mut Cx<bool>) -> El {
        let mut stack = col().key("stack").w_full().growth(GrowthSpec::default())
            .child(el().key("lead").w_full().h(200.0));
        if *s {
            stack = stack.child(memo("new-row", 0, || col().w_full()
                .child(col().w_full().child(el().key("header").w_full().h(48.0).fill(Color::WHITE).sticky_top(0.0))
                    .child(el().w_full().h(200.0)))));
        }
        col().key("scroll").size(200.0, 200.0).scroll(ScrollSpec { stick_bottom: false })
            .child(stack.child(el().key("tail").w_full().h(400.0)))
    }
    let mut rt = Runtime::new(false, view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.update(|s, tx| { *s = true; tx.scroll_to_instant("scroll", 0.0, 200.0); });
    rt.frame(1.0 / 120.0);
    let wrapper = find(&rt, "new-row");
    let header = find(&rt, "header");
    assert!(wrapper.rect[3] < header.rect[3], "exercise a row shorter than its header: {wrapper:?} {header:?}");
    assert_eq!(header.rect[3], 48.0, "bounds do not resize the header");
    assert_eq!(header.rect[1], wrapper.rect[1], "the growing row reveals the header in place rather than lifting it outside the row");
    let fill = rt.instances.iter().find(|p| p.fill == [1.0; 4]).unwrap();
    assert_eq!(fill.rect[3], 48.0, "paint keeps the header size; the existing growth mask owns its reveal");
}

#[derive(Clone, Copy)]
struct RevealThenInsert {
    rows: usize,
    expanded: bool,
    stick_bottom: bool,
}

fn reveal_then_insert_view(s: &RevealThenInsert, _: &mut Cx<RevealThenInsert>) -> El {
    let mut stack = col().key("stack").w_full().growth(GrowthSpec::default()).virtualize(40.0);
    if s.stick_bottom {
        stack = stack.child(el().key("lead").w_full().h(320.0));
    }
    for i in 0..s.rows {
        let h = if i == 1 && s.expanded { 120.0 } else { 40.0 };
        stack = stack.child(el().key(format!("r{i}")).w_full().h(h).streaming(s.stick_bottom && i + 1 == s.rows));
    }
    col().w_full().h_full().child(
        col().key("scroll").w(300.0).h(400.0).scroll(ScrollSpec { stick_bottom: s.stick_bottom }).child(stack),
    )
}

#[test]
fn rows_entering_while_a_reveal_follows_its_row_land_at_full_height() {
    // A reveal outlives its commit while the viewport's springs run. A later commit's new rows
    // land at full height, as they do in a reader's virtual list with no reveal, while the row
    // the reveal follows keeps growing.
    let mut rt = Runtime::new(RevealThenInsert { rows: 3, expanded: false, stick_bottom: false }, reveal_then_insert_view, (300.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| {
        s.expanded = true;
        tx.scroll_into_view("r1");
    });
    for _ in 0..6 {
        rt.frame(1.0 / 120.0);
    }
    let growing = find(&rt, "r1").rect[3];
    assert!(growing > 40.5 && growing < 119.5, "r1 grows: {growing}");
    rt.update(|s, _| s.rows = 4);
    rt.frame(1.0 / 120.0);
    let r1 = find(&rt, "r1").rect[3];
    assert!(r1 > growing && r1 < 119.5, "r1 keeps growing: {growing} -> {r1}");
    let r3 = find(&rt, "r3").rect;
    assert!((r3[3] - 40.0).abs() < 0.05, "the new row lands at full height: {r3:?}");
    assert!((r3[1] - (80.0 + r1)).abs() < 0.05, "below the growing row: {r3:?}");

    // A `stick_bottom` timeline whose reveal is at the tail stays pinned across a later commit:
    // a new row appended while an earlier row still grows springs from zero and carries the end.
    let mut rt = Runtime::new(RevealThenInsert { rows: 3, expanded: false, stick_bottom: true }, reveal_then_insert_view, (300.0, 400.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.update(|s, tx| {
        s.expanded = true;
        tx.scroll_into_view("r2");
    });
    for _ in 0..6 {
        rt.frame(1.0 / 120.0);
    }
    let growing = find(&rt, "r1").rect[3];
    assert!(growing > 40.5 && growing < 119.5, "r1 grows: {growing}");
    rt.update(|s, _| s.rows = 4);
    rt.frame(1.0 / 120.0);
    let r1 = find(&rt, "r1").rect[3];
    let r3 = find(&rt, "r3").rect;
    assert!(r1 > growing && r1 < 119.5, "r1 keeps growing: {growing} -> {r1}");
    assert!(r3[3] > 0.1 && r3[3] < 20.0, "new tail row grows from zero on its spring: {r3:?}");
    assert!((r3[1] + r3[3] - 400.0).abs() < 0.5, "the timeline follows the new tail row: {r3:?}");
    for _ in 0..120 {
        rt.frame(1.0 / 120.0);
    }
    let r3 = find(&rt, "r3").rect;
    assert!((r3[3] - 40.0).abs() < 0.05 && (r3[1] + r3[3] - 400.0).abs() < 0.5, "settled at the bottom: {r3:?}");
}
