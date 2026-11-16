//! Public-runtime regression, runnable without the optional upstream unit-test fixtures.
use rune::{Cx, El, Env, GrowthSpec, LiveText, Runtime, ScrollSpec, col, row, text};
use std::{borrow::Cow, cell::{Cell, RefCell}};

thread_local! {
    static VIEWS: Cell<usize> = const { Cell::new(0) };
    static ROWS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

fn view(live: &LiveText, _: &mut Cx<LiveText>) -> El {
    VIEWS.set(VIEWS.get() + 1);
    col().w_full().h_full().children([
        row().children([text("00").key("fps").live_text(live), text("after").key("after")]),
        col().w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).child(
            col().key("rows").w_full().growth(GrowthSpec::default()).virtual_rows(
                500, 36.0,
                |_: &LiveText, i| Cow::Owned(format!("r{i}")),
                |live: &LiveText, i| {
                    ROWS.with_borrow_mut(|rows| rows.push(i));
                    row().h(36.0).children([
                        text("00").key("value").live_text(live),
                        text(format!("Row {i}")).key("title"),
                    ])
                },
            ),
        ),
    ])
}

fn shown(rt: &Runtime<LiveText>) -> Vec<(String, [i32; 4], Option<String>)> {
    let mut result: Vec<_> = rt.elements().into_iter()
        .filter(|e| e.visible[2] > 0.0 && e.visible[3] > 0.0 && !e.virtual_extent)
        .map(|e| (e.path, e.rect.map(|v| (v * 100.0).round() as i32), e.text)).collect();
    result.sort_by(|a, b| a.0.cmp(&b.0));
    result
}

fn glyphs(rt: &Runtime<LiveText>) -> Vec<([i32; 4], [i32; 4])> {
    // Atlas coordinates are runtime-local; compare actual glyph geometry and clipping.
    let mut result: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH)
        .map(|i| (i.rect.map(|v| (v * 100.0).round() as i32), i.clip.map(|v| (v * 100.0).round() as i32)))
        .collect();
    assert!(!result.is_empty());
    result.sort();
    result
}

fn check(updated: bool) {
    let live = LiveText::new("123456789");
    let mut rt = Runtime::new(live.clone(), view, (320.0, 240.0), Env::new(true, 2.0));
    let mut full = Runtime::new(live.clone(), view, (320.0, 240.0), Env::new(true, 2.0));
    for r in [&mut rt, &mut full] {
        r.frame(0.0);
        while r.frame(1.0 / 120.0) {}
    }
    let mut commits = 0;
    for step in 0..24 {
        let expected = if updated && step % 2 == 0 { "1" } else { "123456789" };
        if updated { live.set(expected); }
        let mounted: Vec<_> = rt.elements().into_iter().filter_map(|e| {
            e.key?.strip_prefix('r')?.parse::<usize>().ok()
        }).collect();
        ROWS.with_borrow_mut(Vec::clear);
        let (views, before) = (VIEWS.get(), rt.stats.commits);
        rt.wheel(160.0, 120.0, -90.0);
        rt.frame(1.0 / 120.0);
        assert_eq!(VIEWS.get(), views, "step {step}: scrolling must skip view with live text");
        ROWS.with_borrow(|rows| assert!(rows.iter().all(|i| !mounted.contains(i)), "rebuilt mounted rows"));
        if rt.stats.commits > before { commits += 1; }
        full.invalidate(); // Full build restores the authored placeholders before sampling.
        full.wheel(160.0, 120.0, -90.0);
        full.frame(1.0 / 120.0);
        assert_eq!(shown(&rt), shown(&full), "step {step}: placeholder layout and sampled content");
        assert_eq!(glyphs(&rt), glyphs(&full), "step {step}: sampled glyphs and clipping");
        for e in rt.elements().into_iter().filter(|e| matches!(e.key.as_deref(), Some("fps" | "value"))) {
            assert_eq!(e.text.as_deref(), Some(expected), "{}", e.path);
        }
    }
    assert!(commits >= 6, "expected repeated virtual remounts, got {commits}");
    while rt.frame(1.0 / 120.0) {}
    while full.frame(1.0 / 120.0) {}
    // Change the available width to miss the retained measurement cache. A sampled label
    // must still measure its authored placeholder, not merely rely on warm Taffy entries.
    for width in [260.0, 400.0] {
        full.update(|_, tx| tx.instant()); // Match resize's snapped geometry, not a state-change spring.
        rt.set_viewport((width, 240.0), 2.0);
        full.set_viewport((width, 240.0), 2.0);
        rt.frame(0.0);
        full.frame(0.0);
        assert_eq!(shown(&rt), shown(&full), "remeasure at {width}");
        assert_eq!(glyphs(&rt), glyphs(&full), "remeasured glyphs at {width}");
    }
    while rt.frame(1.0 / 120.0) {}
    let before = rt.stats.commits;
    live.set("updated after scrolling");
    assert!(rt.live_pending(), "remounted subscriptions still wake");
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.stats.commits, before);
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("fps")).unwrap().text.as_deref(), Some("updated after scrolling"));
}

#[test]
fn virtual_remount_with_stable_live_text() { check(false); }

#[test]
fn virtual_remount_with_updated_live_text() { check(true); }