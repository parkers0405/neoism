//! `Tx::resize_window`: an onboarding-style window that grows from 400 to 600 points tall and
//! back with its top edge fixed. The bars pinned to its top and bottom edges show whether the
//! content follows the frame: a stretched, stale or blank strip would show at the bottom.
//! Press "Toggle", or set `RUNE_RESIZE_AUTO=1` to toggle every 1.5 s. `RUNE_RESIZE_FIT=1`
//! lets the content set the height instead (`App::fit_content_height`).
use rune::*;

#[derive(Default)]
struct State { tall: bool }

fn bar(label: &str, color: Color) -> El {
    row().w_full().h(44.0).center().fill(color).child(text(label).color(Color::WHITE))
}

fn toggle(s: &mut State, tx: &mut Tx) {
    s.tall = !s.tall;
    if std::env::var_os("RUNE_RESIZE_FIT").is_none() {
        tx.resize_window((480.0, if s.tall { 600.0 } else { 400.0 }));
    }
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    let fit = std::env::var_os("RUNE_RESIZE_FIT").is_some();
    let height = if s.tall { 600.0 } else { 400.0 };
    let body = col().w_full().center().gap(12.0)
        .child(text(format!("{height} pt")).font(tok::type_::TITLE).color(Color::WHITE))
        .child(row().key("toggle").padding(10.0).radius(8.0).fill(oklch(0.45, 0.02, 260.0))
            .child(text("Toggle").color(Color::WHITE))
            .on_press_tx(toggle));
    let body = if fit { body.h(height - 88.0) } else { body.grow(1.0) };
    let root = col().key("root").w_full().fill(oklch(0.22, 0.02, 260.0)).border(2.0, oklch(0.75, 0.18, 145.0))
        .child(bar("top edge", oklch(0.5, 0.15, 260.0)))
        .child(body)
        .child(bar("bottom edge", oklch(0.55, 0.2, 25.0)));
    if fit { root } else { root.h_full() }
}

fn main() {
    let spec = AppSpec { min_size: Some((200.0, 200.0)), ..AppSpec::new("Window resize", (480.0, 400.0), State::default, view) };
    let mut app = App::new(spec);
    if std::env::var_os("RUNE_RESIZE_FIT").is_some() { app = app.fit_content_height(); }
    if std::env::var_os("RUNE_RESIZE_AUTO").is_some() {
        let handle = app.handle();
        std::thread::spawn(move || loop {
            std::thread::sleep(std::time::Duration::from_millis(1500));
            handle.update(toggle);
        });
    }
    app.run();
}
