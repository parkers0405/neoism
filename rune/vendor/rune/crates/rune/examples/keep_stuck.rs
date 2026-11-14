//! A long tool group in a chat timeline pinned to its end. Its header sticks to the top while
//! the group is open, and a tap on it opens or shuts the group with `Tx::keep_on_screen`.
//! Shut from where it's stuck, the header stays there while the rows below fold up to it.
//!
//! Headless trace and motion audit (`scripts/motion-audit/run.sh` records both):
//!   cargo run -p rune --example keep_stuck -- --script scripts/motion-audit/scenarios/keep-stuck.json --trace t.jsonl --trace-all
use rune::{AppSpec, Color, Cx, El, GrowthSpec, ScrollSpec, TextStyle, Tx, col, oklch, row, text};

const STEPS: usize = 40;

struct Chat {
    open: bool,
}

fn bubble(key: &str, h: f32) -> El {
    col().key(key).w_full().h(h).radius(10.0).padding(10.0).fill(oklch(0.32, 0.02, 250.0))
        .child(text(key.to_owned()).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
}

fn group(s: &Chat) -> El {
    let label = if s.open { "Used 40 tools  ▾" } else { "Used 40 tools  ▸" };
    let mut header = row().key("hdr").w_full().h(28.0).px(10.0).items_center().fill(oklch(0.2, 0.01, 250.0))
        .child(text(label).font(TextStyle::new(13.0, 500.0)).color(Color::WHITE))
        .on_press_tx(|s: &mut Chat, tx: &mut Tx| {
            s.open = !s.open;
            tx.keep_on_screen("hdr");
        });
    if s.open {
        header = header.sticky_top(0.0);
    }
    let steps = col().key("steps").w_full().px(10.0).children((0..STEPS).map(|k| {
        row().key(format!("step-{k}")).w_full().h(28.0).items_center()
            .child(text(format!("Read src/net/stream_{k:02}.rs")).font(TextStyle::new(12.0, 400.0)).color(oklch(0.75, 0.01, 250.0)))
    }));
    // Shut, the steps stay laid out under a box with no height, and the row's clip folds them.
    let held = col().key("steps-box").w_full();
    col().key("turn").w_full().clip_soft(0.0)
        .child(col().key("group").w_full().child(header).child(if s.open { held } else { held.h(0.0) }.child(steps)))
}

fn view(s: &Chat, _: &mut Cx<Chat>) -> El {
    let rows = vec![bubble("m0", 120.0), bubble("m1", 60.0), group(s), bubble("answer", 90.0), bubble("actions", 36.0)];
    let stack = col().key("timeline").w_full().padding(16.0).gap(8.0).growth(GrowthSpec::default()).virtualize(56.0);
    col().w_full().h_full().fill(oklch(0.2, 0.01, 250.0)).child(
        col().key("scroll").w_full().grow(1.0).scroll(ScrollSpec { stick_bottom: true }).child(stack.children(rows)),
    )
}

fn main() {
    rune::run(AppSpec::new("Keep a stuck header", (420.0, 360.0), || Chat { open: true }, view));
}
