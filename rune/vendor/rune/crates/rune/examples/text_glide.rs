//! A card that drifts a few pixels on a slow spring when tapped. Pixel snapping glides its box
//! between device pixels, and its text glides with it: the words keep their place against the
//! card's edges instead of stepping a whole pixel at a time.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example text_glide -- --script scripts/motion-audit/scenarios/text-glide.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, Spring, TextStyle, col, oklch, text};

struct Card {
    down: bool,
}

fn view(s: &Card, _: &mut Cx<Card>) -> El {
    let label = text("Quiet harbor glides").font(TextStyle::new(13.0, 500.0)).color(Color::WHITE);
    let card = col().key("card").absolute(40.0, 60.0).padding(12.0).radius(10.0).fill(oklch(0.3, 0.02, 250.0))
        .border(1.0, oklch(0.42, 0.02, 250.0))
        .offset(0.0, if s.down { 2.5 } else { 0.0 })
        .offset_motion(Spring::new(1.2, 0.0))
        .on_press(|s: &mut Card| s.down = !s.down)
        .child(label);
    col().w_full().h_full().fill(oklch(0.2, 0.01, 250.0)).child(card)
}

fn main() {
    rune::run(AppSpec::new("Text glide", (320.0, 180.0), || Card { down: false }, view));
}
