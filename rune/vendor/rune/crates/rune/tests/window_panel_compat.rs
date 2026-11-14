use rune::*;

#[test]
fn window_panel_builders_preserve_exhaustive_literals_and_default_inference() {
    fn view(_: &(), _: &mut Cx<()>) -> El { el() }
    let spec = AppSpec {
        title: "compat", size: (300.0, 100.0), min_size: None, init: || (), view,
        scenes: &[("scene", |_| {})], fonts: &[], families: ("Inter", "Inter"),
        setup: None, poll: Some(|_, _, _| false), on_key: Some(|_, _, _| false),
    };
    let options = WindowOptions {
        transparent: true, borderless: true, click_through: false, always_on_top: false,
        visible: false, position: None, physical_size: None, min_size: None, max_size: None,
        resizable: false,
    };
    let app = App::new(spec).on_focus(|_, _, _| {}).macos_window(MacWindowOptions::default().nonactivating_panel(true));
    let handle = app.handle();
    // Existing WindowSpec literals remain exhaustive too.
    let window = WindowSpec { key: "quick".into(), app, options, on_created: None }
        .on_focus(|_, _, _| {}).options(Default::default());
    let _ = window.configured_options(WindowOptions::default().macos(MacWindowOptions::default().nonactivating_panel(true)
        .corner_radius(16.0).corner_curve_circular()));
    handle.place(DisplayTarget::Primary, OverlayPlacement::FullScreen);
    let overlay = OverlaySpec { name: "compat", size: (50.0, 50.0), edge: ScreenEdge::Top,
        margin: 0.0, align: Align::Center, inset: 0.0, interactive: true, view };
    let _ = overlay.options(Default::default()).on_focus(|_, _, _| {});
    let _ = WindowSpec::new("launcher", AppSpec::new("launcher", (300.0, 100.0), || (), view))
        .configured_options(WindowOptions::launcher_panel()).on_ready(|w| { assert!(w.is_ready()); });
    let _ = App::new(AppSpec::new("launcher", (300.0, 100.0), || (), view))
        .configured_options(WindowOptions::launcher_panel()).on_ready(|_| {}).window_options(Default::default());
}
