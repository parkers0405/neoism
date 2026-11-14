use super::*;

#[test]
fn a_phone_canvas_reported_in_css_pixels_gets_its_device_height() {
    // Chromium's emulated device scale reports the CSS box: 390 × 844 at 2× used to take the
    // width's fix and leave the height at 844, stretching the canvas 2× vertically.
    assert_eq!(fix_canvas_size((390.0, 844.0), (390, 844), (1200.0, 900.0), 2.0, false), (780, 1688));
}

#[test]
fn a_canvas_still_at_its_intrinsic_size_is_sized_to_its_box() {
    // The canvas exists at the HTML default 300 × 150 until `resumed` styles it; a frame that
    // races the browser's first ResizeObserver sees that size.
    assert_eq!(fix_canvas_size((393.0, 852.0), (300, 150), (1200.0, 900.0), 3.0, false), (1179, 2556));
}

#[test]
fn a_window_that_fits_its_content_keeps_the_height_it_measured() {
    // `fit_content_height`: the canvas is exactly its content, and the app set that height.
    assert_eq!(fix_canvas_size((600.0, 640.0), (600, 640), (1200.0, 900.0), 2.0, true).1, 640);
    assert_eq!(fix_canvas_size((600.0, 640.0), (300, 150), (1200.0, 900.0), 2.0, true).1, 150);
}

#[test]
fn a_canvas_with_no_box_yet_falls_back_to_the_spec() {
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 2.0, false), (2400, 1800));
    assert_eq!(fix_canvas_size((0.0, 0.0), (0, 0), (1200.0, 900.0), 1.0, true), (1200, 900));
}

#[test]
fn a_size_the_box_agrees_with_is_left_alone() {
    // The box's fractional width at 1.5× rounds to a pixel the browser may report ±1.
    assert_eq!(fix_canvas_size((1001.0, 700.0), (1502, 1050), (1200.0, 900.0), 1.5, false), (1502, 1050));
}

#[test]
fn a_page_surface_never_shrinks_for_a_toolbar() {
    let phone = (1179, 2556);
    // Safari's toolbar expanding shrinks the viewport while the page is scrolling.
    assert_eq!(should_reconfigure_surface(phone, (1179, 2400), true), (phone, false));
    // Collapsing again lands on the size it already has.
    assert_eq!(should_reconfigure_surface(phone, phone, true), (phone, false));
    // A rotation has a new width, and does reconfigure.
    assert_eq!(should_reconfigure_surface(phone, (2556, 1179), true), ((2556, 1179), true));
}

#[test]
fn other_windows_reconfigure_on_every_size_change() {
    assert_eq!(should_reconfigure_surface((800, 600), (800, 500), false), ((800, 500), true));
    assert_eq!(should_reconfigure_surface((800, 600), (800, 600), false), ((800, 600), false));
    // A window hidden to nothing keeps its surface until it has a size again.
    assert_eq!(should_reconfigure_surface((800, 600), (0, 0), false), ((800, 600), false));
}
