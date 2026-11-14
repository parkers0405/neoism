//! `Tx::set_font_families` swaps the families behind the sans and mono slots while the app
//! runs: text measures as a fresh launch with those families would, memoized subtrees build
//! again, and asking for the families already there rebuilds nothing.
use std::cell::Cell;

use rune::text::TextSystem;
use rune::{App, AppSpec, El, Env, Runtime, TextStyle, col, memo, row, text};

const INTER: &[u8] = include_bytes!("../assets/Inter-Regular.ttf");
const PLEX: &[u8] = include_bytes!("../../../themes/opencode/assets/fonts/IBMPlexMono-Regular.ttf");
const LAUNCH: (&str, &str) = ("Inter", "IBM Plex Mono");
static APP_FONTS: &[&[u8]] = &[INTER, PLEX];

thread_local! {
    static MEMO_BUILDS: Cell<u32> = const { Cell::new(0) };
}

const STYLE: TextStyle = TextStyle::new(14.0, 400.0);

fn view(_: &(), _: &mut rune::Cx<()>) -> El {
    let style = STYLE;
    col().children([
        row().child(text("Hamburgefonstiv 0123").key("label").font(style)),
        row().child(memo("card", 0, || {
            MEMO_BUILDS.set(MEMO_BUILDS.get() + 1);
            row().child(text("Memoized card title").key("memo-label").font(style))
        })),
        row().child(text("let mono = 42;").key("code").font(style.mono())),
    ])
}

fn launch(families: (&str, &str)) -> Runtime<()> {
    let text = TextSystem::new(&[INTER, PLEX], families);
    let mut rt = Runtime::with_text((), view, (600.0, 400.0), Env::new(true, 2.0), text);
    rt.frame(0.0);
    rt
}

fn widths(rt: &Runtime<()>) -> [f32; 3] {
    let elements = rt.elements();
    ["label", "memo-label", "code"].map(|key| {
        elements.iter().find(|e| e.key.as_deref() == Some(key)).unwrap_or_else(|| panic!("{key}")).target[2]
    })
}

fn switch(rt: &mut Runtime<()>, sans: &str, mono: &str) -> bool {
    let stale = rt.update_if(|_, tx| {
        tx.set_font_families(sans, mono);
        false
    });
    rt.frame(1.0 / 120.0);
    stale
}

#[test]
fn a_switch_measures_like_a_fresh_launch_and_rebuilds_memoized_subtrees() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let fresh = widths(&launch(("IBM Plex Mono", "Inter")));
    let mut rt = launch(LAUNCH);
    let before = widths(&rt);
    let builds = MEMO_BUILDS.get();
    assert!(switch(&mut rt, "IBM Plex Mono", "Inter"), "a switch asks for a frame");
    let after = widths(&rt);
    assert_eq!(rt.font_families(), ("IBM Plex Mono", "Inter"));
    for i in 0..3 {
        assert!((after[i] - before[i]).abs() > 1.0, "width {i} changed: {before:?} -> {after:?}");
    }
    assert_eq!(after, fresh, "matches a fresh launch with those families");
    assert_eq!(MEMO_BUILDS.get(), builds + 1, "the memoized subtree built again, once");
    assert!(rt.stats.shaped > 0, "text shaped again in the new families");
    let (w, _) = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None);
    assert!((w - after[0]).abs() < 0.51, "measure follows: {w} vs {}", after[0]);

    // And back again.
    switch(&mut rt, LAUNCH.0, LAUNCH.1);
    assert_eq!(widths(&rt), before);
}

#[test]
fn asking_for_the_families_already_there_rebuilds_nothing() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let mut rt = launch(LAUNCH);
    rt.frame(1.0 / 120.0);
    let (builds, commits) = (MEMO_BUILDS.get(), rt.stats.commits);
    assert!(!switch(&mut rt, LAUNCH.0, LAUNCH.1), "no frame asked for");
    assert_eq!(rt.stats.commits, commits, "no commit");
    assert_eq!(MEMO_BUILDS.get(), builds, "no memo rebuild");
    assert_eq!(rt.stats.shaped, 0, "nothing shaped");
}

#[test]
fn every_runtime_on_the_thread_follows() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let mut a = launch(LAUNCH);
    let mut b = launch(LAUNCH);
    let fresh = widths(&launch(("IBM Plex Mono", "IBM Plex Mono")));
    switch(&mut a, "IBM Plex Mono", "IBM Plex Mono");
    b.frame(1.0 / 120.0);
    assert_eq!(widths(&a), fresh);
    assert_eq!(widths(&b), fresh, "another window follows on its next frame");
}

#[test]
fn a_missing_family_falls_back_as_at_launch() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let fresh = widths(&launch(("Rune Missing Family", "Rune Missing Mono")));
    let mut rt = launch(LAUNCH);
    switch(&mut rt, "Rune Missing Family", "Rune Missing Mono");
    assert_eq!(widths(&rt), fresh);
    assert!(widths(&rt).iter().all(|w| *w > 0.0));
}

/// A window of an app launched in `LAUNCH`, as a window opens it, before its first frame.
fn open_window() -> Runtime<()> {
    let mut spec = AppSpec::new("fonts", (600.0, 400.0), || (), view);
    spec.fonts = APP_FONTS;
    spec.families = LAUNCH;
    App::new(spec).headless_runtime((600.0, 400.0), Env::new(true, 2.0))
}

#[test]
fn a_window_opened_after_a_switch_starts_in_the_new_families() {
    let _g = rune::text::with_families(LAUNCH.0, LAUNCH.1);
    let switched = ("IBM Plex Mono", "Inter");
    let fresh = widths(&launch(switched));
    let switched_width = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0;
    let mut first = open_window();
    first.frame(0.0);
    let launch_width = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0;
    assert!((launch_width - switched_width).abs() > 1.0, "the families measure apart");
    switch(&mut first, switched.0, switched.1);
    let mut second = open_window();
    let (w, _) = rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None);
    assert_eq!(w, switched_width, "measure stays in the chosen families before its first frame");
    assert_eq!(second.font_families(), switched, "the new window starts in the chosen families");
    second.frame(0.0);
    assert_eq!(widths(&second), fresh);
    assert_eq!(rune::text::measure("Hamburgefonstiv 0123", &[], STYLE, None).0, switched_width);
}
