//! A keycap that sinks about its bottom edge, and a ring that turns and grows from one edge:
//! `El::scale`, `El::offset`, `El::origin`, `El::rotate` and springing shadows together.
//! `cargo run -p rune --example expressive_press -- --script crates/rune/examples/expressive_press.json
//! --video press.mp4` records it; `-- --snap press.png --scene still` renders rest and pressed side
//! by side.

use rune::tok::{self, type_};
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, Spring, TextStyle, Tx, col, el, row, text};

#[derive(Default)]
struct State {
    pressed: bool,
    grown: bool,
    still: bool,
}

fn keycap(pressed: bool) -> El {
    el().size(96.0, 96.0).radius(18.0).center()
        .fill(Color { r: 0.99, g: 0.98, b: 0.96, a: 1.0 })
        .border(1.0, Color { r: 0.0, g: 0.0, b: 0.0, a: 0.08 })
        .motion(Spring::new(0.22, 0.0))
        .origin(0.5, 1.0)
        .scale(if pressed { 0.96 } else { 1.0 })
        .offset(0.0, if pressed { 2.0 } else { 0.0 })
        .shadow(if pressed { 2.0 } else { 8.0 }, if pressed { 4.0 } else { 18.0 }, Color::BLACK.alpha(if pressed { 0.3 } else { 0.22 }))
        .child(text("K").font(TextStyle::new(34.0, 600.0)).color(Color { r: 0.16, g: 0.17, b: 0.2, a: 1.0 }))
}

fn ring(grown: bool) -> El {
    let accent = tok::accent(0, false);
    col().size(96.0, 96.0).radius(24.0).border(6.0, accent).items_center().padding(10.0)
        .motion(Spring::new(0.5, 0.15))
        .origin(0.5, 1.0)
        .rotate(if grown { std::f32::consts::FRAC_PI_4 } else { 0.0 })
        .scale(if grown { 1.0 } else { 0.55 })
        .child(el().size(14.0, 14.0).radius(7.0).fill(accent))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let t = tok::theme(false);
    let label = |s: &'static str| text(s).font(type_::LABEL).color(t.text_secondary);
    let stage = if s.still {
        row().gap(56.0).items_center()
            .child(col().gap(22.0).items_center().child(keycap(false)).child(label("Rest")))
            .child(col().gap(22.0).items_center().child(keycap(true)).child(label("Pressed")))
    } else {
        row().gap(96.0).items_end().child(keycap(s.pressed)).child(ring(s.grown))
    };
    col().w_full().h_full().center().fill(Color { r: 0.86, g: 0.87, b: 0.89, a: 1.0 }).child(stage)
}

fn on_key(s: &mut State, k: &KeyEvent, _: &mut Tx) -> bool {
    match &k.key {
        Key::Text(c) if c == "p" => s.pressed = true,
        Key::Text(c) if c == "u" => s.pressed = false,
        Key::Text(c) if c == "r" => s.grown = true,
        _ => return false,
    }
    true
}

fn main() {
    rune::run(AppSpec::<State> {
        scenes: &[("still", |s| { s.still = true; s.pressed = true; })],
        on_key: Some(on_key),
        ..AppSpec::new("Expressive press", (440.0, 260.0), State::default, view)
    });
}
