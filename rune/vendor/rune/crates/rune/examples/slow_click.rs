//! A settings page with two slow buttons, for interaction timing (`RUNE_PERF`): Save sleeps
//! 40 ms in its handler and Sort computes for 40 ms. Each click reaches a local OTLP collector
//! as a `ui.interaction` span named by page and control (`settings`, `settings/toolbar/save`),
//! with the watchdog's stack samples (`rune.hot.0` is `sort_settings` for Sort) and whose time
//! it was (`rune.cause`: `blocked` for Save, `cpu` for Sort).
//!
//! `RUNE_PERF=1 cargo run -p rune --example slow_click -- --script crates/rune/examples/slow_click.json
//! --snap saved.png`

use std::time::{Duration, Instant};

use rune::{AppSpec, Color, Cx, El, TextStyle, col, el, row, text};

#[derive(Default)]
struct Settings {
    saves: u32,
    order: u64,
}

/// Busy for 40 ms.
#[inline(never)]
fn sort_settings(seed: u64) -> u64 {
    let start = Instant::now();
    let mut x = seed | 1;
    while start.elapsed() < Duration::from_millis(40) {
        for _ in 0..1000 { x = std::hint::black_box(x.wrapping_mul(6364136223846793005).wrapping_add(1)); }
    }
    x
}

fn view(s: &Settings, _: &mut Cx<Settings>) -> El {
    let font = TextStyle::new(14.0, 400.0);
    let rows = (0..40).fold(col().key("fields").gap(4.0), |c, i| c.child(row().key(format!("field-{i}")).h(20.0).child(text(format!("Setting {i}")).font(font))));
    col().key("settings").w_full().h_full().padding(16.0).gap(12.0)
        .child(row().key("toolbar").gap(8.0).center_y()
            .child(el().key("save").size(80.0, 30.0).radius(6.0).center().fill(Color::WHITE.alpha(0.12))
                .on_press(|s: &mut Settings| {
                    std::thread::sleep(Duration::from_millis(40));
                    s.saves += 1;
                })
                .child(text("Save").font(font)))
            .child(el().key("sort").size(80.0, 30.0).radius(6.0).center().fill(Color::WHITE.alpha(0.12))
                .on_press(|s: &mut Settings| s.order = sort_settings(s.order))
                .child(text("Sort").font(font)))
            .child(text(format!("saved {}", s.saves)).font(font)))
        .child(rows)
}

fn main() {
    rune::run(AppSpec::new("Slow click", (480.0, 360.0), Settings::default, view));
}
