//! Tall memoized rows must retain visual positions through window remounts and layout reports.
use rune::{Cx, El, Env, GrowthSpec, LiveText, Runtime, ScrollSpec, col, memo, text};
use std::{borrow::Cow, cell::Cell, collections::HashMap};

thread_local! { static VIEWS: Cell<usize> = const { Cell::new(0) }; }

struct State {
    live: LiveText,
    reports: HashMap<String, [f32; 4]>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    col().w_full().h_full().children([
        text("00").h(24.0).live_text(&s.live),
        col().key("scroll").w_full().grow(1.0).min_h(0.0)
            .scroll(ScrollSpec { stick_bottom: true }).scroll_anchor().child(
                col().key("rows").w_full().pt(16.0).pb(80.0).growth(GrowthSpec::default())
                    .virtual_rows(60, 120.0, |_: &State, i| Cow::Owned(format!("r{i}")), |_: &State, i| {
                        memo(format!("memo{i}"), (), || {
                            col().w_full().h(if i % 7 == 3 { 2400.0 } else { 37.0 + (i % 6) as f32 * 31.0 })
                                .child(text(format!("Row {i}")).key("label"))
                                .on_layout_tx(move |s: &mut State, r, tx| {
                                    tx.live_only();
                                    s.reports.insert(format!("r{i}"), r);
                                })
                        })
                    }),
            ),
    ])
}

fn rows(rt: &Runtime<State>) -> HashMap<String, [f32; 4]> {
    rt.elements().into_iter().filter_map(|e| {
        let key = e.key?;
        key.strip_prefix('r')?.parse::<usize>().ok()?;
        Some((key, e.rect))
    }).collect()
}

fn check(anchored: bool) {
    let state = || State { live: LiveText::new("60 fps"), reports: HashMap::new() };
    let mut rt = Runtime::new(state(), view, (400.0, 624.0), Env::new(true, 2.0));
    let mut full = Runtime::new(state(), view, (400.0, 624.0), Env::new(true, 2.0));
    for r in [&mut rt, &mut full] {
        r.frame(0.0);
        for _ in 0..240 { r.frame(1.0 / 120.0); }
    }
    let (mut fast_views, mut full_views, mut remounts) = (0, 0, 0);
    for step in 0..360 {
        let before = rows(&rt);
        let (key, rect) = before.iter().filter(|(_, r)| r[1] < 624.0 && r[1] + r[3] > 24.0)
            .min_by(|a, b| (a.1[1] - 24.0).abs().total_cmp(&(b.1[1] - 24.0).abs())).unwrap();
        let delta = if step < 180 { -37.0 } else { 37.0 };
        let at = rect[1] - 24.0 - delta;
        for r in [&mut rt, &mut full] {
            if anchored {
                r.update_if(|_, tx| { tx.scroll_key_to(key, at); false });
            } else {
                r.wheel(200.0, 300.0, -delta);
            }
        }
        full.invalidate();
        let (views, commits) = (VIEWS.get(), rt.stats.commits);
        rt.frame(1.0 / 120.0);
        fast_views += VIEWS.get() - views;
        remounts += rt.stats.commits - commits;
        let views = VIEWS.get();
        full.frame(1.0 / 120.0);
        full_views += VIEWS.get() - views;
        let after = rows(&rt);
        let reference = rows(&full);
        assert!((after[key][1] - rect[1] + delta).abs() < 0.1,
            "step {step}: {key} moved {} instead of {}", after[key][1] - rect[1], -delta);
        for (key, rect) in &after {
            if let Some(reference) = reference.get(key) {
                for axis in 0..4 { assert!((rect[axis] - reference[axis]).abs() < 0.1, "step {step}: {key} axis {axis}: {rect:?} != {reference:?}"); }
            }
        }
        // Layout notifications delivered this frame describe the previous drawn frame.
        for (key, report) in &rt.state.reports {
            if let Some(previous) = before.get(key) {
                assert!((report[1] - previous[1]).abs() < 0.5,
                    "step {step}: stale memo layout report {key}: {report:?} != {previous:?}");
            }
        }
    }
    assert!(remounts > 5, "exercise changing virtual windows: {remounts}");
    assert!(fast_views < full_views / 4, "retained rows skip views: {fast_views} versus {full_views}");
    // Previously unmeasured, variable-height rows can legitimately retry a full build.
    // At least some wheel commits must nevertheless take the retained-tree fast path.
    if !anchored { assert!(fast_views < remounts as usize, "exercise retained-tree remounts"); }
    eprintln!("anchored={anchored}: {remounts} window commits, {fast_views} views versus {full_views} forced-full views");
}

#[test]
fn anchored_steps_preserve_tall_memo_rows_and_deliver_current_layout_reports() { check(true); }

#[test]
fn wheel_remounts_preserve_tall_memo_rows_and_deliver_current_layout_reports() { check(false); }