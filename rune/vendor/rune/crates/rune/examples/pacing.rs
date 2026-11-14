//! Timestamp-only native spring probe. Run on a worker with a display:
//! cargo run --profile perf -p rune --example pacing -- --native --trace pacing.jsonl --frames 240
use rune::{App, AppSpec, Color, Cx, El, Spring, el};

fn view(wide: &bool, _: &mut Cx<bool>) -> El {
    el().key("root").size(480.0, 120.0).child(el().key("pill")
        .size(if *wide { 400.0 } else { 100.0 }, 60.0)
        .fill(Color { r: 0.4, g: 0.65, b: 1.0, a: 1.0 }).motion(Spring::new(0.6, 0.0)))
}

fn main() {
    let app = App::new(AppSpec::new("Rune pacing", (480.0, 120.0), || false, view));
    let handle = app.handle();
    // Deliberately off-refresh commits, including several retargets in one refresh.
    let updater = std::thread::spawn(move || {
        for i in 0..48 {
            std::thread::sleep(std::time::Duration::from_millis(if i % 3 == 0 { 173 } else { 3 }));
            if i == 15 {
                handle.hide();
                std::thread::sleep(std::time::Duration::from_millis(35));
                handle.show();
            }
            handle.update(move |wide, _| *wide = i % 2 == 0);
        }
        std::thread::sleep(std::time::Duration::from_secs(5));
        handle.quit();
    });
    app.run();
    updater.join().unwrap();
}
