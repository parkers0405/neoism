//! ↑ and ↓ move a selection through a long list of cards, and the list scrolls to keep it in
//! view with `Tx::scroll_to_with`. Held under key repeat, each press asks for the next row
//! while the last glide still runs; the glide keeps its speed, so the list moves in one
//! continuous motion rather than a step a press. A still pointer over the cards keeps its
//! hover while they slide under it.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example scroll_keys -- --script scripts/motion-audit/scenarios/scroll-keys.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, Key, KeyEvent, ScrollSpec, Spring, TextStyle, Tx, col, oklch, row, text};

const CARDS: usize = 120;
const CARD: f32 = 52.0;
const GAP: f32 = 8.0;
const PAD: f32 = 12.0;
const VIEW: f32 = 360.0;
/// Quicker than the default glide, so the first press answers at once.
const SPRING: Spring = Spring::new(0.18, 0.0);

struct Board {
    selected: usize,
    /// Where the list was last sent.
    top: f32,
}

fn card(s: &Board, k: usize) -> El {
    let selected = k == s.selected;
    row().key(format!("card-{k}")).w_full().h(CARD).px(14.0).items_center().radius(10.0)
        .fill(if selected { oklch(0.42, 0.09, 250.0) } else { oklch(0.26, 0.015, 250.0) })
        .hover(Color::WHITE.alpha(0.06))
        .child(text(format!("Card {k:03}")).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
}

fn view(s: &Board, _: &mut Cx<Board>) -> El {
    let cards = col().key("cards").w_full().padding(PAD).gap(GAP).children((0..CARDS).map(|k| card(s, k)));
    col().w_full().h_full().fill(oklch(0.18, 0.01, 250.0))
        .child(row().key("bar").w_full().h(40.0).px(14.0).items_center()
            .child(text("↑ ↓ to move").font(TextStyle::new(12.0, 500.0)).color(oklch(0.7, 0.01, 250.0))))
        .child(col().key("board").w_full().h(VIEW).scroll(ScrollSpec { stick_bottom: false }).child(cards))
}

fn on_key(s: &mut Board, e: &KeyEvent, tx: &mut Tx) -> bool {
    s.selected = match e.key {
        Key::Down => (s.selected + 1).min(CARDS - 1),
        Key::Up => s.selected.saturating_sub(1),
        _ => return false,
    };
    // Scroll just far enough that the selected card shows whole, with the list's padding.
    let top = PAD + s.selected as f32 * (CARD + GAP) - PAD;
    let bottom = top + CARD + 2.0 * PAD;
    let to = if top < s.top { top } else if bottom > s.top + VIEW { bottom - VIEW } else { s.top };
    if to != s.top {
        s.top = to;
        tx.scroll_to_with("board", 0.0, to, SPRING);
    }
    true
}

fn main() {
    rune::run(AppSpec { on_key: Some(on_key), ..AppSpec::new("Scroll with the keys", (420.0, 400.0), || Board { selected: 4, top: 0.0 }, view) });
}
