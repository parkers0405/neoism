use super::composite::{MAX_BACKDROP, fit_backdrop, pixels};
use super::*;

#[test]
fn backdrop_is_cropped_to_the_output_aspect_then_scaled() {
    let dir = std::env::temp_dir().join(format!("rune-backdrop-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    // Red, green and blue thirds side by side: a square output keeps the green middle.
    let wide = dir.join("wide.png");
    ::image::RgbaImage::from_fn(300, 100, |x, _| ::image::Rgba([if x < 100 { 255 } else { 0 }, if (100..200).contains(&x) { 255 } else { 0 }, if x >= 200 { 255 } else { 0 }, 255]))
        .save(&wide).unwrap();
    let px = fit_backdrop(&wide, (50, 50)).unwrap();
    assert_eq!(px.len(), 50 * 50 * 4);
    assert!(px.chunks(4).all(|p| p[1] > 200 && p[0] < 40 && p[2] < 40), "only the centre survives");
    assert_eq!(fit_backdrop(&wide, (300, 100)).unwrap()[..4], [255, 0, 0, 255], "same size: pixels as they are");

    let huge = dir.join("huge.png");
    ::image::GrayImage::new(MAX_BACKDROP + 1, 1).save(&huge).unwrap();
    let error = fit_backdrop(&huge, (10, 10)).unwrap_err();
    assert!(error.contains("16385x1"), "refused before decoding: {error}");
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Each window of a headless multi-window run takes its own Tab opt-in and titlebar inset,
/// and a borderless window has no traffic lights to inset for.
#[test]
fn each_window_takes_its_tab_opt_in_and_titlebar_inset() {
    use crate::{AppSpec, Cx, El, Key, KeyEvent, col, el, row};
    use crate::WindowSpec;
    fn view(_: &(), cx: &mut Cx<()>) -> El {
        col().size(300.0, 100.0)
            .child(row().child(el().key("inset").size(cx.titlebar_inset().max(1.0), 10.0)))
            .child(el().key("button").size(30.0, 30.0).focusable(true).role("button"))
    }
    let window = |tab: bool, borderless: bool| {
        let spec = WindowSpec::new("w", AppSpec::new("W", (300.0, 100.0), || (), view)).tab_visits_controls(tab);
        let spec = WindowSpec { options: crate::WindowOptions { borderless, ..Default::default() }, ..spec };
        let mut w = Window::new(spec, None, &Args::default(), true, true, 1.0);
        w.rt.frame(0.0);
        w.rt.set_full_keyboard_access(false);
        w.rt.key(KeyEvent { key: Key::Tab, mods: Default::default() });
        w.rt.frame(0.1);
        let focused = w.rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
        (focused, w.rt.presented("inset").unwrap().0[2])
    };
    assert_eq!(window(true, false), (Some("button".into()), 67.0));
    assert_eq!(window(false, false).0, None, "keyboard navigation off skips buttons");
    assert_eq!(window(true, true).1, 1.0, "borderless: no inset");
}

#[test]
fn output_pixels_fit_a_texture() {
    assert_eq!(pixels((1512.0, 982.0), 2.0, 8192), Some((3024, 1964)));
    assert_eq!(pixels((0.2, 10.0), 2.0, 8192), None, "under a pixel");
    assert_eq!(pixels((5000.0, 10.0), 2.0, 8192), None, "over the GPU's limit");
    assert_eq!(pixels((f32::NAN, 10.0), 2.0, 8192), None);
    assert_eq!(pixels((10.0, 10.0), f32::INFINITY, 8192), None);
}
