use rune::{App, AppSpec, Chrome, Cx, El, WindowSpec};

// Exactly the public field set at 32a1715. Deliberately exhaustive: adding a required field
// must fail compilation here rather than first in a downstream Hex or Rooms checkout.
fn legacy_spec() -> AppSpec<()> {
    AppSpec {
        title: "Legacy consumer",
        size: (640.0, 480.0),
        min_size: None,
        init: || (),
        view: |_: &(), _: &mut Cx<()>| -> El { rune::el() },
        scenes: &[],
        fonts: &[],
        families: ("Inter", "Inter"),
        setup: None,
        poll: None,
        on_key: None,
    }
}

#[test]
fn legacy_exhaustive_literal_and_chrome_builders_compile() {
    let _ = App::new(legacy_spec()).chrome(Chrome::DEFAULT);
    let _ = WindowSpec::new("legacy", legacy_spec()).chrome(Chrome::DEFAULT);
}
