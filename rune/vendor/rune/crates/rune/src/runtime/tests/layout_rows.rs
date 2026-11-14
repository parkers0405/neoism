use super::*;


#[test]
fn duplicate_sibling_keys_keep_the_first_and_lay_out_without_a_panic() {
    fn view(s: &u32, _: &mut Cx<u32>) -> El {
        col().children((0..*s).map(|i| el().key("row:R-94").size(10.0, 10.0 + i as f32)))
    }
    let mut rt = Runtime::new(2, view, (100.0, 100.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let h = |rt: &Runtime<u32>, key: &str| rt.presented(key).map(|(r, _)| r[3]);
    assert_eq!(h(&rt, "row:R-94"), Some(10.0), "the first keeps the key");
    assert_eq!(h(&rt, "row:R-94#dup1"), Some(11.0), "the duplicate still draws");
    rt.update(|s, _| *s = 3);
    rt.frame(0.1);
    assert_eq!(h(&rt, "row:R-94#dup2"), Some(12.0));
}

#[test]
fn duplicate_sibling_keys_tooling_reports_and_lays_out() {
    let mut rt = Runtime::new((), duplicate_view, (100.0, 100.0), Env::new(true, 1.0));
    rt.key_warnings = Some(Vec::new());
    rt.frame(0.0);
    assert_eq!(rt.key_warnings.as_ref().unwrap().len(), 1);
    assert_eq!(rt.nodes.len(), 3);
    assert_eq!(rt.nodes[0].local.w, 100.0);
}

#[test]
fn side_panel_wraps_once_and_releases_height_after_stable_width() {
    use crate::el::{row, text};
    fn view(s: &(f32, bool), cx: &mut Cx<(f32, bool)>) -> El {
        cx.reduced_motion(s.1);
        row().key("window").w_full().h_full()
            .child(col().key("viewport").grow(1.0).shrink().h_full()
                .scroll(ScrollSpec { stick_bottom: true }).clip_soft(18.0)
                .child(col().key("stack").w_full().growth(GrowthSpec::default())
                    .child(col().key("message").w_full().child(text("Wrap once at the landing width. ".repeat(30)).wrap().w_full()))))
            .child(el().key("side-panel").w(s.0).h_full())
    }
    fn run(fps: usize, reduced: bool) -> Vec<f32> {
        let mut rt = Runtime::new((0.0, reduced), view, (480.0, 700.0), Env::new(true, 2.0));
        rt.frame(0.0);
        let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("message")).unwrap().id;
        let original = rt.grows[&id].h;
        let mut trace = std::env::var_os("RUNE_WIDTH_TRACE").map(|dir| {
            let dir = std::path::PathBuf::from(dir).join(format!("width-{fps}-{}", if reduced { "reduced" } else { "normal" }));
            std::fs::create_dir_all(&dir).unwrap();
            crate::tooling::Trace::open(&dir.join("trace.jsonl"), (480.0, 700.0), fps as f64, 2.0)
        });
        if let Some(trace) = &mut trace { trace.frame(&mut rt, 0.0, 0.0); }
        rt.state.0 = 160.0;
        rt.invalidate();
        rt.frame(0.0);
        let commits = rt.stats.commits;
        let measured = rt.nodes.iter().find(|n| n.id == id).unwrap().local.h;
        assert!(measured > original);
        if reduced { assert_eq!(rt.grows[&id].h, measured); }
        else { assert_eq!(rt.grows[&id].target, original); }
        let mut samples = vec![];
        for frame in 1..=fps * 2 {
            rt.frame(1.0 / fps as f64);
            assert_eq!(rt.stats.commits, commits, "presentation must not remeasure the wrapping");
            assert_eq!(rt.stats.shaped, 0);
            if frame as f64 / (fps as f64) < 0.12 && !reduced {
                assert_eq!(rt.grows[&id].h, original);
                assert!(rt.animating, "the hold must keep the shared clock awake");
            }
            if frame % (fps / 30) == 0 { samples.push(rt.grows[&id].h); }
            if let Some(trace) = &mut trace { trace.frame(&mut rt, frame as f64 * 1000.0 / fps as f64, 0.0); }
        }
        assert!((rt.grows[&id].h - measured).abs() < 0.05);
        // A second target during a hold restarts the stable-width interval.
        rt.state.0 = 80.0;
        rt.invalidate();
        rt.frame(0.0);
        for _ in 0..fps / 10 { rt.frame(1.0 / fps as f64); }
        rt.state.0 = 120.0;
        rt.invalidate();
        rt.frame(0.0);
        for _ in 0..fps / 10 { rt.frame(1.0 / fps as f64); }
        if !reduced {
            assert_eq!(rt.grows[&id].h, measured, "a changing width must restart the hold");
            assert!(!rt.stacks.values().next().unwrap().pending_heights.is_empty());
        }
        // Settle/resume bypasses even an active hold completely.
        rt.settle_all();
        rt.frame(0.0);
        assert!(rt.stacks.values().all(|s| s.pending_heights.is_empty()));
        assert!(!rt.animating);
        samples
    }
    for reduced in [false, true] {
        let a = run(30, reduced);
        let b = run(120, reduced);
        for (a, b) in a.into_iter().zip(b) { assert!((a - b).abs() < 0.05, "frame-rate dependent hold: {a} vs {b}"); }
    }
}

#[test]
fn measurement_cannot_change_presented_geometry_or_motion_policy() {
    fn view(changed: &bool, cx: &mut Cx<bool>) -> El {
        cx.reduced_motion(*changed);
        col().key("stack").w_full().growth(GrowthSpec::default())
            .child(el().key("row").w_full().h(if *changed { 160.0 } else { 40.0 }))
    }
    let mut rt = Runtime::new(false, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("row")).unwrap().id;
    let before = rt.scene.presented(id).unwrap().rect;
    rt.state = true;
    let measured = rt.measure(&Tx::default());
    assert_eq!(rt.scene.presented(id).unwrap().rect, before);
    assert_eq!(rt.scene.store().active_len(), 0);
    assert_eq!(rt.grows[&id].h, 40.0);
    assert!(!rt.reduced_motion && !rt.scene.store().reduced_motion());
    rt.apply(measured, &Tx::default());
    assert!(rt.reduced_motion && rt.scene.store().reduced_motion());
    assert_eq!(rt.grows[&id].h, 160.0);
}

#[test]
fn send_flight_follows_the_measured_bubble_while_earlier_rows_land() {
    struct State { draft: crate::edit::TextEdit, sent: bool, height: f32 }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut rows = col().w_full().growth(GrowthSpec::default())
            .child(el().key("earlier").w_full().h(s.height));
        if s.sent { rows = rows.child(col().key("bubble").w_full().py(8.0)
            .child(crate::el::text("héllo flight").key("destination").wrap())); }
        col().w_full().h_full()
            .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true }).child(rows))
            .child(crate::el::text_input(&s.draft, crate::tok::TextStyle::new(14.0, 400.0)).key("source").w_full().h(60.0)
                .on_edit(|s: &mut State| &mut s.draft)
                .on_submit(|s: &mut State, tx: &mut Tx| {
                    s.sent = true;
                    s.draft.clear();
                    tx.fly_text("source", "destination");
                }))
    }
    let mut draft = crate::edit::TextEdit::new("héllo flight");
    draft.focused = true;
    let mut rt = Runtime::new(State { draft, sent: false, height: 80.0 }, view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.height = 400.0;
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    rt.key(KeyEvent { key: crate::edit::Key::Enter, mods: Default::default() });
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.flights.len(), 1);
    assert!(!rt.flights[0].glyphs.is_empty(), "the visible composer glyphs survive clearing the model");
    let mut previous_target = rt.flights[0].destination;
    let mut retargets = 0;
    for _ in 0..240 {
        rt.frame(1.0 / 120.0);
        if let Some(f) = rt.flights.first() {
            assert!(rt.instances.iter().any(|q| q.params[1] == KIND_GLYPH && q.uv[2] > 0.0 && q.fill[3] > 0.1
                && q.rect[0].max(q.clip[0]) + 1.0 < (q.rect[0] + q.rect[2]).min(q.clip[2])
                && q.rect[1].max(q.clip[1]) + 1.0 < (q.rect[1] + q.rect[3]).min(q.clip[3])),
                "the source/destination handoff never loses every visible glyph");
            let target = rt.nodes.iter().position(|n| n.key.as_deref() == Some("destination")).unwrap();
            assert!((f.destination.y - rt.drawn[target][1]).abs() < 0.01, "destination is this frame's post-scroll geometry");
            if f.destination != previous_target { retargets += 1; }
            previous_target = f.destination;
        }
    }
    assert!(retargets > 3, "earlier row growth actually moved the destination");
    assert!(rt.flights.is_empty(), "flight lands and releases the captured glyphs");
    assert!(!rt.animating);
}

#[test]
fn virtual_rows_match_full_layout_while_paging_and_keep_a_keyed_anchor() {
    struct State { virtualized: bool, keys: Vec<usize> }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        let mut stack = col().key("rows").w_full().pt(8.0).pb(24.0).growth(GrowthSpec::default());
        if s.virtualized { stack = stack.virtualize(80.0); }
        for &i in &s.keys {
            stack = stack.child(col().key(format!("r{i}")).w_full().pb(9.0).child(
                crate::el::text(format!("Row {i}: {}", "variable height héllo naïve ".repeat(i % 7 + 1))).wrap().w_full()
            ));
        }
        col().key("scroll").w_full().h_full().clip_fade(0.0, 8.0, 0.0, 8.0).scroll(ScrollSpec { stick_bottom: true }).child(stack)
    }
    let env = Env::new(true, 2.0);
    let mut full = Runtime::new(State { virtualized: false, keys: (0..600).collect() }, view, (240.0, 300.0), env);
    let mut window = Runtime::new(State { virtualized: true, keys: (0..600).collect() }, view, (240.0, 300.0), env);
    full.frame(0.0);
    window.frame(0.0);
    assert!(window.nodes.len() < 100, "offscreen rows aren't laid out or retained in the scene");
    assert!(window.text.shaped < full.text.shaped / 5, "offscreen text isn't shaped");
    let visible = |r: &Runtime<State>| r.elements().into_iter()
        .filter(|e| e.key.as_ref().is_some_and(|k| k.starts_with('r') && k != "rows") && e.visible[3] > 0.0)
        .map(|e| (e.key.unwrap(), e.rect)).collect::<Vec<_>>();
    for step in 0..135 {
        let a = visible(&full);
        let b = visible(&window);
        assert_eq!(a.len(), b.len(), "visible count at step {step}: {a:?} vs {b:?}");
        for ((ak, ar), (bk, br)) in a.iter().zip(&b) {
            assert_eq!(ak, bk, "visible identity at step {step}");
            assert!(ar.iter().zip(br).all(|(a, b)| (a - b).abs() < 0.05), "anchor at step {step}: {ak} {ar:?} vs {br:?}");
        }
        let dy = if step < 90 { 137.25 } else { -137.25 };
        full.wheel(120.0, 150.0, dy);
        window.wheel(120.0, 150.0, dy);
        full.frame(1.0 / 120.0);
        window.frame(1.0 / 120.0);
    }
    let before = visible(&window);
    window.state.keys.insert(0, 9000);
    window.invalidate();
    window.frame(1.0 / 120.0);
    assert_eq!(before, visible(&window), "prepending outside the window preserves its keyed anchor");
    assert!(window.nodes.len() < 150 && window.grows.len() < 70, "paging releases old row state");
    let anchor = visible(&window)[0].clone();
    window.set_viewport((320.0, 300.0), 2.0);
    window.frame(0.0);
    for _ in 0..60 {
        let after = visible(&window);
        let rect = after.iter().find(|(key, _)| key == &anchor.0).expect("resize retains the visible keyed anchor");
        assert!((rect.1[1] - anchor.1[1]).abs() < 0.05, "resize anchor moved: {:?} vs {:?}", anchor, rect);
        window.frame(1.0 / 120.0);
    }
    window.settle_all();
    window.frame(0.0);
    let last = visible(&window).pop().unwrap();
    assert_eq!(last.0, "r599");
    assert!((last.1[1] + last.1[3] - 276.0).abs() < 0.05, "return lands at the exact bottom");
    assert!(!window.animating, "moving: {:?}; active: {}", window.elements().into_iter().filter(|e| e.moving).map(|e| (e.path, e.rect, e.target, e.opacity)).collect::<Vec<_>>(), window.scene.store().active_len());
}

#[test]
fn reduced_motion_rows_land_immediately_but_keep_the_presence_fade() {
    fn view(show: &bool, cx: &mut Cx<bool>) -> El {
        cx.reduced_motion(true);
        let mut stack = col().w_full().growth(GrowthSpec::default());
        if *show {
            stack = stack.child(el().key("row").size(200.0, 100.0));
        }
        stack
    }
    let mut rt = Runtime::new(false, view, (200.0, 300.0), Env::new(true, 2.0));
    assert!(!rt.frame(0.0));
    rt.state = true;
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
    assert_eq!(row.rect[3], 100.0, "reduced motion removes growth travel");
    assert!(row.opacity > 0.0 && row.opacity < 1.0, "new rows still fade in");
    rt.frame(0.2);
    assert!(!rt.animating);
    assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap().opacity, 1.0);
    rt.state = false;
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    let row = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("row")).unwrap();
    assert!(row.opacity > 0.0 && row.opacity < 1.0, "removed rows stay for their fade");
    for _ in 0..60 { rt.frame(1.0 / 120.0); }
    assert!(!rt.animating);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("row")));
}

#[test]
fn streaming_fill_and_width_rewrap_land_on_completion() {
    fn view(state: &(String, bool), _: &mut Cx<(String, bool)>) -> El {
        col().w_full().growth(GrowthSpec::default()).child(
            col().key("row").w_full().streaming(state.1).child(crate::el::text(state.0.clone()).wrap().w_full().reveal(0.18))
        )
    }
    let mut rt = Runtime::new(("one".into(), true), view, (180.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.state.0 = "one two three four five six seven eight nine ten eleven".into();
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    let row = rt.nodes.iter().find(|n| n.key.as_deref() == Some("row")).unwrap().id;
    let n = &rt.nodes[rt.node_index[&row]];
    let full = n.local.h;
    assert!(rt.grows[&row].target < full, "new line is held back while filling");
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert!(!rt.animating, "partial line at rest must stop ticking");
    let old_h = rt.grows[&row].h;
    rt.set_viewport((600.0, 500.0), 2.0);
    rt.state.0.push_str(" twelve");
    rt.invalidate();
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.grows[&row].target, old_h, "rewrap waits for stable width");
    for _ in 0..15 { rt.frame(1.0 / 120.0); }
    assert!(rt.grows[&row].target < old_h, "width change may shrink a streaming row");
    assert!(rt.grows[&row].h > rt.grows[&row].target, "rewrapped height eases instead of snapping");
    rt.state.0 = "one two".into();
    rt.invalidate();
    rt.frame(0.0);
    assert!(rt.grows[&row].target < rt.grows[&row].h, "streaming may still shrink inside the 400 ms rewrap window");
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    let landed = rt.grows[&row].h;
    rt.state.0 = "o".into();
    rt.invalidate();
    rt.frame(0.0);
    assert_eq!(rt.grows[&row].target, landed, "streaming cannot shrink after the rewrap window");
    rt.state.1 = false;
    rt.invalidate();
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert!(!rt.animating);
    assert_eq!(rt.grows[&row].h, rt.nodes[rt.node_index[&row]].local.h);
}

#[test]
fn retained_layout_matches_fresh_tree_across_edits() {
    fn view(seed: &usize, _: &mut Cx<usize>) -> El {
        let mut root = col().key("root").w_full();
        for k in 0..8 {
            let i = (k + seed) % 8;
            if (i + seed) % 5 == 0 { continue; }
            let body = "some words ".repeat((i + seed) % 10 + 1);
            let child = if (i + seed) % 7 == 0 { el().h(31.0) } else { crate::el::text(body).wrap() };
            root = root.child(child.key(format!("row-{i}")).w_full().px((seed % 3) as f32));
        }
        root
    }
    let env = Env::new(true, 2.0);
    let mut cached = Runtime::new(0usize, view, (320.0, 600.0), env);
    for seed in 0..40 {
        let size = (240.0 + (seed % 4) as f32 * 30.0, 600.0);
        let scale = if seed % 3 == 0 { 1.0 } else { 2.0 };
        cached.state = seed;
        cached.set_viewport(size, scale);
        cached.settle_all();
        cached.frame(0.0);
        let mut fresh = Runtime::new(seed, view, size, Env { scale, ..env });
        fresh.frame(0.0);
        let rects = |r: &Runtime<usize>| r.elements().into_iter().filter(|e| e.opacity > 0.0).map(|e| (e.path, e.target)).collect::<Vec<_>>();
        assert_eq!(rects(&cached), rects(&fresh), "edit {seed}");
    }
}
