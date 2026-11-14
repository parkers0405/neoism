//! A feed scrolled partway down, with `El::scroll_anchor` on its viewport. ⏎ adds three
//! messages above everything, growing in; the message at the top of the view stays where it
//! is while they open, as CSS `overflow-anchor` keeps a page still in a browser. Run with
//! `plain` to see the same feed without the anchor, its view pushed down as they open.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example scroll_anchor -- --script scripts/motion-audit/scenarios/scroll-anchor.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, GrowthSpec, Key, KeyEvent, ScrollSpec, TextStyle, Tx, col, oklch, row, text};

const ROWS: usize = 40;

struct Feed {
    /// Messages that arrived above the first.
    earlier: usize,
    anchor: bool,
}

fn message(key: String, label: String, h: f32) -> El {
    row().key(key).w_full().h(h).px(14.0).items_center().radius(10.0).fill(oklch(0.26, 0.015, 250.0))
        .child(text(label).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
}

fn view(s: &Feed, _: &mut Cx<Feed>) -> El {
    let earlier = (0..s.earlier).rev().map(|k| message(format!("earlier-{k}"), format!("Earlier {k}"), 64.0));
    let rows = (0..ROWS).map(|k| message(format!("msg-{k}"), format!("Message {k:02}"), 48.0 + (k % 3) as f32 * 16.0));
    let list = col().key("list").w_full().padding(12.0).gap(8.0).growth(GrowthSpec::default()).children(earlier.chain(rows));
    let feed = col().key("feed").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: false }).child(list);
    col().w_full().h_full().fill(oklch(0.18, 0.01, 250.0)).child(if s.anchor { feed.scroll_anchor() } else { feed })
}

fn on_key(s: &mut Feed, e: &KeyEvent, _: &mut Tx) -> bool {
    if e.key != Key::Enter {
        return false;
    }
    s.earlier += 3;
    true
}

fn main() {
    rune::run(AppSpec { on_key: Some(on_key), ..AppSpec::new("Keep the reading position", (420.0, 400.0), || Feed { earlier: 0, anchor: !std::env::args().any(|a| a == "plain") }, view) });
}
