//! A `motion(Spring::NONE)` written on the element itself marks what that spring governs there
//! as landing at once by design; one that arrives any other way marks nothing.
use super::*;
use crate::{Cx, El, Env, Spring, col, el, oklch};

#[derive(Clone, Copy, Default, PartialEq)]
enum Source {
    #[default]
    Own,
    Parent,
    Token,
    Reduced,
    /// Inherited `Spring::NONE`, with the swatch's own `blur_motion(Spring::NONE)`.
    OwnBlur,
}

#[derive(Clone, Copy, Default)]
struct S {
    source: Source,
    warm: bool,
    thick: bool,
    blurred: bool,
}

fn view(s: &S, cx: &mut Cx<S>) -> El {
    let (blue, orange) = (oklch(0.6, 0.15, 250.0), oklch(0.7, 0.16, 50.0));
    let swatch = el().key("swatch").absolute(40.0, 40.0).size(120.0, 80.0)
        .fill(if s.warm { orange } else { blue })
        .radius(if s.warm { 24.0 } else { 4.0 })
        .border(if s.thick { 4.0 } else { 1.0 }, if s.warm { oklch(0.4, 0.1, 50.0) } else { oklch(0.9, 0.02, 250.0) })
        .blur(if s.blurred { 6.0 } else { 0.0 });
    let swatch = match s.source {
        Source::Own => swatch.motion(Spring::NONE),
        Source::Token => swatch.token_motion(Spring::NONE),
        Source::OwnBlur => swatch.blur_motion(Spring::NONE),
        Source::Parent | Source::Reduced => swatch,
    };
    if s.source == Source::Reduced { cx.reduced_motion(true); }
    let page = col().key("page").w_full().h_full().fill(oklch(0.18, 0.01, 260.0));
    let page = if matches!(s.source, Source::Parent | Source::OwnBlur) { page.motion(Spring::NONE) } else { page };
    page.child(swatch)
}

/// Settles with `from`, then makes `change` on a frame of its own with no input, and returns
/// what the swatch's snaps changed.
fn snaps(from: Source, change: impl FnOnce(&mut S)) -> Vec<String> {
    let mut r = Run::new(Runtime::new(S { source: from, ..S::default() }, view, (400.0, 300.0), Env::new(true, 1.0)), 120.0);
    r.idle(0.5);
    r.step();
    r.rt.update(|s, _| change(s));
    r.step();
    r.play(1.0);
    let f = r.finish();
    assert!(f.iter().all(|f| f.kind == "snap" && f.element == "page/swatch"), "{f:#?}");
    let mut changed: Vec<String> = f.iter().map(|f| f.detail.split(' ').next().unwrap_or_default().to_string()).collect();
    changed.sort();
    changed
}

#[test]
fn an_explicit_spring_none_on_the_element_is_an_authored_instant_change() {
    assert_eq!(snaps(Source::Own, |s| s.warm = true), Vec::<String>::new());
    assert_eq!(snaps(Source::Own, |s| s.blurred = true), Vec::<String>::new());
}

#[test]
fn a_spring_none_inherited_from_a_parent_still_snaps() {
    assert_eq!(snaps(Source::Parent, |s| s.warm = true), ["border", "fill", "radius"]);
}

#[test]
fn a_spring_none_from_a_theme_token_still_snaps() {
    assert_eq!(snaps(Source::Token, |s| s.warm = true), ["border", "fill", "radius"]);
}

#[test]
fn a_change_reduce_motion_lands_at_once_still_snaps() {
    // Reduce Motion lands corners and blur at once and gives colours a short fade.
    assert_eq!(snaps(Source::Reduced, |s| { s.warm = true; s.blurred = true; }), ["blur", "radius"]);
}

#[test]
fn an_explicit_spring_none_excuses_only_what_it_governs() {
    // `motion` doesn't spring border width, so its `Spring::NONE` says nothing about it.
    assert_eq!(snaps(Source::Own, |s| s.thick = true), ["border-width"]);
    // An instant blur curve of its own excuses its blur, not its inherited colours and corners.
    assert_eq!(snaps(Source::OwnBlur, |s| { s.warm = true; s.blurred = true; }), ["border", "fill", "radius"]);
}
