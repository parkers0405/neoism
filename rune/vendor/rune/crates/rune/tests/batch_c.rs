use std::sync::{Arc, atomic::{AtomicUsize, Ordering}};
use rune::{Cx, El, Env, LiveText, Runtime, TextStyle, el, row, text};

struct State { value: LiveText, views: AtomicUsize, bound: bool }
fn view(s: &State, _: &mut Cx<State>) -> El {
    s.views.fetch_add(1, Ordering::Relaxed);
    row().child(if s.bound { text("00:00").key("clock").live_text(&s.value) } else { el().key("empty") })
        .child(text("untouched sibling").key("sibling"))
}
fn info(rt: &Runtime<State>, key: &str) -> rune::runtime::ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

#[test]
fn live_text_shapes_only_changed_run_without_layout_and_coalesces_visible_wakes() {
    let value = LiveText::new("00:00");
    let mut a = Runtime::new(State { value: value.clone(), views: AtomicUsize::new(0), bound: true }, view, (400.0, 80.0), Env::default());
    let mut b = Runtime::new(State { value: value.clone(), views: AtomicUsize::new(0), bound: true }, view, (400.0, 80.0), Env::default());
    let wakes = Arc::new(AtomicUsize::new(0));
    for rt in [&mut a, &mut b] {
        let wakes = wakes.clone();
        rt.set_live_waker(Arc::new(move || { wakes.fetch_add(1, Ordering::Relaxed); }));
        rt.frame(0.0);
    }
    let rect = info(&a, "clock").rect;
    let sibling = info(&a, "sibling").rect;
    let commits = a.stats.commits;
    b.set_live_visible(false);
    assert!(!value.set("00:00"));
    assert!(!a.live_pending());
    value.set("00:01"); value.set("00:02");
    assert_eq!(wakes.load(Ordering::Relaxed), 1);
    a.frame(0.0); b.frame(0.0);
    assert_eq!(a.stats.shaped, 1);
    assert_eq!(b.stats.shaped, 0);
    assert_eq!(info(&a, "clock").text.as_deref(), Some("00:02"));
    assert_eq!(info(&b, "clock").text.as_deref(), Some("00:00"));
    assert_eq!(a.stats.commits, commits);
    assert_eq!(a.state.views.load(Ordering::Relaxed), 1);
    b.set_live_visible(true);
    assert_eq!(wakes.load(Ordering::Relaxed), 2);
    b.frame(0.0);
    assert_eq!(info(&b, "clock").text.as_deref(), Some("00:02"));
    value.set("a much longer clock value"); a.frame(0.0);
    assert_eq!(info(&a, "clock").rect, rect);
    assert_eq!(info(&a, "sibling").rect, sibling);
    a.frame(0.0);
    assert_eq!(a.stats.shaped, 0);
    a.invalidate(); a.frame(0.0);
    assert_eq!(info(&a, "clock").rect, rect, "placeholder still reserves the same box on rebuild");
    assert_eq!(info(&a, "clock").text.as_deref(), Some("a much longer clock value"));
    a.state.bound = false; a.invalidate(); a.frame(0.0);
    // The clock still draws while it fades out; once it's gone it stops waking.
    for _ in 0..240 { if !a.frame(1.0 / 120.0) { break; } }
    b.set_live_visible(false);
    let before = wakes.load(Ordering::Relaxed);
    value.set("00:03");
    assert_eq!(wakes.load(Ordering::Relaxed), before, "unmounted and hidden hosts don't wake");
    a.state.bound = true; a.invalidate(); a.frame(0.0);
    assert_eq!(info(&a, "clock").text.as_deref(), Some("00:03"));
}

#[test]
fn features_affect_shaping_and_have_separate_cache_entries() {
    let mut system = rune::text::TextSystem::default();
    let style = TextStyle::new(24.0, 400.0);
    let plain = system.layout_rich("111111", &[], style, 1.0, None);
    let tabular = system.layout_rich("111111", &[], style.features("'tnum' 1"), 1.0, None);
    assert!(!std::rc::Rc::ptr_eq(&plain, &tabular));
    let again = system.layout_rich("111111", &[], style, 1.0, None);
    assert!(std::rc::Rc::ptr_eq(&plain, &again));
    let tnum = style.features("'tnum' 1");
    assert_eq!(system.measure("111111", tnum, 1.0).0, system.measure("000000", tnum, 1.0).0);
    let pnum = style.features("'pnum' 1");
    assert_ne!(system.measure("111111", pnum, 1.0).0, system.measure("000000", pnum, 1.0).0);
    assert_ne!(rune::text::text_hash("0", &[], style.features("'ss01' 1"), 1.0, None), rune::text::text_hash("0", &[], style, 1.0, None));
}

#[cfg(target_os = "macos")]
#[test]
fn optical_size_uses_points_and_variations_reach_parley_and_raster_cache() {
    let data: &'static [u8] = Box::leak(std::fs::read("/System/Library/Fonts/SFNS.ttf").unwrap().into_boxed_slice());
    let mut system = rune::text::TextSystem::new(&[data], ("System Font", "System Font"));
    let style = TextStyle::new(13.0, 400.0);
    let point1 = system.layout_rich("Hello 0123", &[], style.optical_size(13.0), 1.0, None);
    let point2 = system.layout_rich("Hello 0123", &[], style.optical_size(13.0), 2.0, None);
    let device = system.layout_rich("Hello 0123", &[], style.optical_sizing(rune::OpticalSize::FontDefault), 2.0, None);
    let auto = system.layout_rich("Hello 0123", &[], style, 2.0, None);
    let explicit = system.layout_rich("Hello 0123", &[], style.variations("'opsz' 13"), 2.0, None);
    let coords = |l: &parley::Layout<rune::text::Ink>| l.lines().next().unwrap().runs().next().unwrap().normalized_coords().to_vec();
    assert_eq!(coords(&point1), coords(&point2));
    assert_eq!(coords(&explicit), coords(&point2));
    assert_eq!(coords(&auto), coords(&point2));
    assert_ne!(coords(&device), coords(&point2));
    let mut a = Vec::new(); let mut b = Vec::new();
    system.glyphs_for_layout(&point2, 2.0, [0.0, 0.0], false, &mut a);
    system.glyphs_for_layout(&device, 2.0, [0.0, 0.0], false, &mut b);
    assert_ne!(a[0].uv, b[0].uv, "different variation outlines must not share atlas entries");
}

#[test]
fn os_motion_preference_updates_and_view_can_override_it() {
    fn view(override_: &Option<bool>, cx: &mut Cx<Option<bool>>) -> El {
        if let Some(value) = override_ { cx.reduced_motion(*value); }
        text(cx.system_reduced_motion().to_string())
    }
    let mut rt = Runtime::new(None, view, (100.0, 100.0), Env::default());
    assert!(!rt.system_reduced_motion());
    rt.set_system_reduced_motion(true);
    rt.frame(0.0); assert!(rt.reduced_motion);
    rt.set_system_reduced_motion(false); rt.frame(0.0); assert!(!rt.reduced_motion);
    rt.set_system_reduced_motion(true); rt.frame(0.0); assert!(rt.reduced_motion);
    rt.update(|s, _| *s = Some(false)); rt.frame(0.0); assert!(!rt.reduced_motion);
    assert!(rt.system_reduced_motion(), "a commit override doesn't change the OS preference");
    assert_eq!(rt.elements()[0].text.as_deref(), Some("true"));
    rt.update(|s, _| *s = None); rt.frame(0.0); assert!(rt.reduced_motion);
}

#[test]
fn window_focus_delivers_ordered_transitions_including_before_first_frame() {
    fn view(_: &Vec<bool>, _: &mut Cx<Vec<bool>>) -> El {
        el().on_window_focus(|s: &mut Vec<bool>, focused, tx| { s.push(focused); tx.copy("focus"); })
    }
    let mut rt = Runtime::new(Vec::new(), view, (100.0, 100.0), Env::default());
    rt.window_focus(true); rt.window_focus(true); rt.window_focus(false); rt.frame(0.0);
    assert_eq!(rt.state, vec![true, false]);
    assert_eq!(rt.copy_request.as_deref(), Some("focus"));
    let commits = rt.stats.commits;
    rt.window_focus(false); rt.frame(0.0);
    assert_eq!(rt.stats.commits, commits);
    rt.window_focus(true); rt.frame(0.0);
    assert_eq!(rt.state, vec![true, false, true]);
}

#[test]
fn fade_truncation_keeps_full_text_and_only_fades_overflowing_glyphs() {
    fn view(width: &f32, _: &mut Cx<f32>) -> El {
        row().child(text("this is much too long").key("label").w(*width).font(TextStyle::new(14.0, 400.0)).truncate_fade())
    }
    for scale in [1.0, 2.0] {
        let mut rt = Runtime::new(60.0, view, (600.0, 100.0), Env::new(true, scale));
        rt.frame(0.0);
        assert_eq!(rt.elements().into_iter().find(|e| e.key.as_deref() == Some("label")).unwrap().text.as_deref(), Some("this is much too long"));
        let glyphs: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH).collect();
        assert!(!glyphs.is_empty());
        for glyph in glyphs {
            assert_eq!(glyph.clip[2], 60.0 * scale);
            assert_eq!(rune::render::unpack_fade(glyph.fade).0[2], 1.5 * 14.0 * scale, "1.5 em by default");
        }
        rt.update(|s, tx| { *s = 500.0; tx.settle(); }); rt.frame(0.0);
        assert!(rt.instances.iter().all(|i| rune::render::unpack_fade(i.fade).0 == [0.0; 4]));
    }
}

#[test]
fn fade_truncation_overflow_is_independent_of_element_and_display_scale() {
    const LABEL: &str = "clock 0123456789";
    fn view(s: &(f32, rune::Live<f32>), _: &mut Cx<(f32, rune::Live<f32>)>) -> El {
        row().padding(80.0).child(text(LABEL).w(s.0).font(TextStyle::new(14.0, 400.0))
            .live_scale(&s.1).truncate_fade())
    }
    let mut text_system = rune::text::TextSystem::default();
    for display_scale in [1.0, 2.0] {
        let width = text_system.layout_rich(LABEL, &[], TextStyle::new(14.0, 400.0), display_scale, None)
            .full_width() / display_scale;
        for element_scale in [0.5, 1.0, 2.0] {
            for (fraction, overflowing) in [(0.75, true), (1.25, false)] {
                let mut rt = Runtime::new((width * fraction, rune::Live::new(element_scale)), view,
                    (600.0, 300.0), Env::new(true, display_scale));
                rt.frame(0.0);
                let fades: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH)
                    .map(|i| rune::render::unpack_fade(i.fade).0[2]).collect();
                assert!(!fades.is_empty());
                let expected = if overflowing { 1.5 * 14.0 * display_scale * element_scale } else { 0.0 };
                assert!(fades.iter().all(|fade| *fade == expected),
                    "display={display_scale}, element={element_scale}, overflowing={overflowing}: {fades:?}");
            }
        }
    }
}

#[test]
fn fade_truncation_width_is_settable() {
    fn view(s: &(f32, f32), _: &mut Cx<(f32, f32)>) -> El {
        row().child(text("this is much too long").w(s.0).font(TextStyle::new(14.0, 400.0)).truncate_fade_width(s.1))
    }
    for scale in [1.0, 2.0] {
        for (width, em, expected) in [(60.0, 3.0, 42.0), (60.0, 0.5, 7.0)] {
            let mut rt = Runtime::new((width, em), view, (600.0, 100.0), Env::new(true, scale));
            rt.frame(0.0);
            let fades: Vec<_> = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH)
                .map(|i| rune::render::unpack_fade(i.fade).0[2]).collect();
            assert!(!fades.is_empty());
            assert!(fades.iter().all(|f| *f == expected * scale), "width {width}, {em} em at {scale}x: {fades:?}");
        }
    }
}
