//! `El::anchored` inside a zoomed page: the overlay draws at its anchor's zoom, beside where the
//! anchor draws, flips by its zoomed size, and hits where it draws.
use super::*;
use crate::el::{Align, Side};
use crate::{Cx, El, Env, TextStyle, col, el, text};

#[derive(Clone, Copy, Default)]
struct S {
    zoom: f32,
    /// The anchor's top edge in the page.
    top: f32,
    /// How the overlay takes the zoom.
    mode: Mode,
    presses: u32,
}

#[derive(Clone, Copy, Default, PartialEq)]
enum Mode {
    #[default]
    Inherit,
    /// Scales itself, as an app that zooms its popovers by hand does.
    OwnScale,
    Unscaled,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let page = col().key("page").size(800.0, 600.0).scale(s.zoom).origin(0.0, 0.0)
        .child(el().key("target").absolute(100.0, s.top).size(80.0, 20.0));
    let pop = col().key("pop").w(120.0).h(60.0).padding(10.0)
        .child(el().key("btn").size(100.0, 30.0).on_press(|s: &mut S| s.presses += 1)
            .child(text("Rename").key("label").font(TextStyle::new(13.0, 400.0))))
        .anchored("target", Side::Bottom, Align::Start, 5.0);
    let pop = match s.mode {
        Mode::Inherit => pop,
        Mode::OwnScale => pop.scale(s.zoom).origin(0.0, 0.0),
        Mode::Unscaled => pop.anchored_unscaled(),
    };
    col().size(800.0, 600.0).child(page).child(pop)
}

fn run(s: S) -> Runtime<S> {
    let mut rt = Runtime::new(s, view, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    rt
}

fn rect(rt: &Runtime<S>, key: &str) -> [f32; 4] { rt.presented(key).unwrap().0 }

#[test]
fn at_zoom_one_the_overlay_sits_below_its_anchor_at_its_own_size() {
    let rt = run(S { zoom: 1.0, top: 100.0, ..S::default() });
    assert_eq!(rect(&rt, "target"), [100.0, 100.0, 80.0, 20.0]);
    assert_eq!(rect(&rt, "pop"), [100.0, 125.0, 120.0, 60.0]);
    assert_eq!(rect(&rt, "btn"), [110.0, 135.0, 100.0, 30.0]);
}

#[test]
fn at_zoom_one_point_six_the_overlay_draws_at_the_zoom_beside_the_drawn_anchor() {
    let rt = run(S { zoom: 1.6, top: 100.0, ..S::default() });
    assert_eq!(rect(&rt, "target"), [160.0, 160.0, 128.0, 32.0]);
    // The gap zooms too: 5 points at 1.6 is 8.
    assert_eq!(rect(&rt, "pop"), [160.0, 200.0, 192.0, 96.0]);
    assert_eq!(rect(&rt, "btn"), [176.0, 216.0, 160.0, 48.0]);
    // Its text rasterizes at the zoom, as the page's does.
    let label = rt.nodes.iter().position(|n| n.key.as_deref() == Some("label")).unwrap();
    assert!(rt.inh_scratch[label].scale == 1.6 && rt.inh_scratch[label].scale_motion == ScaleMotion::Still);
}

#[test]
fn at_zoom_one_point_six_it_flips_by_its_zoomed_size() {
    // At 1.0 the 60 point overlay fits below an anchor ending at 320.
    let rt = run(S { zoom: 1.0, top: 300.0, ..S::default() });
    assert_eq!(rect(&rt, "pop")[1], 325.0);
    // At 1.6 the anchor ends at 512 and the 96 point overlay would end at 616, past the window,
    // so it goes above: 480 - 8 - 96.
    let rt = run(S { zoom: 1.6, top: 300.0, ..S::default() });
    assert_eq!(rect(&rt, "target"), [160.0, 480.0, 128.0, 32.0]);
    assert_eq!(rect(&rt, "pop"), [160.0, 376.0, 192.0, 96.0]);
}

#[test]
fn at_zoom_one_point_six_a_button_in_the_overlay_hits_where_it_draws() {
    let mut rt = run(S { zoom: 1.6, top: 100.0, ..S::default() });
    // Inside the zoomed button (176..336, 216..264), outside where it would sit unzoomed.
    rt.pointer_down(320.0, 255.0);
    rt.pointer_up(320.0, 255.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(rt.state.presses, 1);
    assert_eq!(rt.element_at(320.0, 255.0).as_ref().and_then(|e| e.key()), Some("btn"));
}

#[test]
fn an_overlay_that_scales_itself_is_not_zoomed_twice() {
    let rt = run(S { zoom: 1.6, top: 100.0, mode: Mode::OwnScale, ..S::default() });
    assert_eq!(rect(&rt, "pop")[2..], [192.0, 96.0]);
}

#[test]
fn an_unscaled_overlay_stays_at_window_scale_beside_the_drawn_anchor() {
    let rt = run(S { zoom: 1.6, top: 100.0, mode: Mode::Unscaled, ..S::default() });
    assert_eq!(rect(&rt, "pop"), [160.0, 197.0, 120.0, 60.0]);
}

fn long_list(zoom: &f32, _: &mut Cx<f32>) -> El {
    let rows = col().key("rows").children((0..40).map(|i| el().key(format!("row{i}")).w(100.0).h(20.0)));
    let list = col().key("scroll").shrink().min_h(0.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(rows);
    let page = col().size(800.0, 600.0).scale(*zoom).origin(0.0, 0.0)
        .child(el().key("target").absolute(100.0, 100.0).size(80.0, 20.0));
    col().size(800.0, 600.0).child(page)
        .child(col().key("menu").w(100.0).child(list).anchored_fit("target", Side::Bottom, Align::Start, 5.0))
}

#[test]
fn at_zoom_one_point_six_a_fitted_list_fits_the_room_it_draws_in() {
    let mut rt = Runtime::new(1.6, long_list, (800.0, 600.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    // The anchor draws down to 192 and the gap is 8, so 600 - 8 - 200 = 392 points are left.
    let r = rect_of(&rt, "menu");
    assert_eq!(r[1], 200.0);
    assert!(r[3] <= 392.0 && r[3] > 390.0, "drawn within the room below: {r:?}");
}

fn rect_of<T>(rt: &Runtime<T>, key: &str) -> [f32; 4] { rt.presented(key).unwrap().0 }
