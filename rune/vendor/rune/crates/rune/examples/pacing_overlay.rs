//! Matched-size overlay probe for scripts/native-pacing/run.py. No capture or input automation.
use rune::{App, AppSpec, Color, Cx, El, OverlaySpec, RunOptions, WindowSpec, col, el, row, text};

fn view(animate: &bool, _: &mut Cx<bool>) -> El {
    let blue = Color { r: 0.25, g: 0.55, b: 1.0, a: 1.0 };
    let mut pill = row().key("pill").padding(8.0).gap(4.0).radius(28.0)
        .fill(Color { r: 0.055, g: 0.065, b: 0.085, a: 0.97 });
    for label in ["Area", "Window", "Screen", "iPhone", "Paste", "×"] {
        pill = pill.child(el().key(label).padding(10.0).radius(16.0).fill(blue.alpha(0.16))
            .child(text(label).color(Color::WHITE)));
    }
    if *animate { pill = pill.looping(rune_motion::LoopSpec::opacity(2.0, 0.0, 0.8, 1.0)); }
    col().w_full().h_full().center().gap(8.0).child(pill).child(text("").color(blue))
}

fn main() {
    rune::run_windows_with_args(RunOptions { accessory: true }, [], |_| {
        let spec = AppSpec::new("Pacing overlay", (680.0, 160.0), || false, view);
        let mut app = App::new(spec).windowless();
        let animate = std::env::var_os("PACING_NATIVE").is_some();
        let pill = app.overlay(OverlaySpec {
            name: "Pacing pill", size: (680.0, 160.0), edge: rune::ScreenEdge::Top,
            margin: 40.0, align: rune::Align::Center, inset: 0.0, interactive: false, view,
        }, animate);
        pill.show();
        let handle = app.handle();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(14));
            handle.quit();
        });
        vec![WindowSpec::from_app("lab", app)]
    });
}
