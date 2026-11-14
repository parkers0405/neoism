//! Native pop-up menus: a settings-style picker that opens over its button with the current
//! choice under the pointer (on Windows it drops down below the button), and a context menu on
//! secondary press. Off macOS and Windows both answer `None` straight away. Headless:
//! `--script` with `{"type":"menu-pick","id":"dark"}`.
use rune::*;

struct State { theme: String, last: String }

const THEMES: [(&str, &str); 3] = [("system", "System"), ("light", "Light"), ("dark", "Dark")];

fn theme_title(id: &str) -> &'static str {
    THEMES.iter().find(|(i, _)| *i == id).map_or("System", |(_, t)| t)
}

fn context_menu() -> PopupMenu {
    PopupMenu::new()
        .item(PopupItem::new("copy", "Copy").symbol("doc.on.doc")
            .key_equivalent("mod+c".parse().unwrap()))
        .item(PopupItem::new("paste", "Paste").enabled(false))
        .separator()
        .submenu("Share", PopupMenu::new().item(PopupItem::new("mail", "Mail")).item(PopupItem::new("messages", "Messages")))
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    col().padding(24.0).gap(16.0)
        .child(row().gap(12.0).center_y()
            .child(text("Theme"))
            .child(row().key("theme").padding(6.0).radius(6.0).fill(oklch(0.45, 0.02, 260.0))
                .child(text(theme_title(&s.theme)))
                .on_press_tx(|s: &mut State, tx: &mut Tx| {
                    tx.popup_menu(PopupMenu::picker(THEMES, &s.theme), "theme", |s: &mut State, picked, _| {
                        if let Some(id) = picked { s.theme = id.into(); }
                    });
                })))
        .child(el().key("canvas").size(272.0, 80.0).radius(8.0).fill(oklch(0.3, 0.02, 260.0))
            .child(text("Secondary-click for a context menu").padding(8.0))
            .on_secondary_press(|_: &mut State, press: PointerPress, tx: &mut Tx| {
                tx.popup_menu(context_menu(), press, |s: &mut State, picked, _| {
                    s.last = picked.unwrap_or("(dismissed)").into();
                });
            }))
        .child(text(format!("Last: {}  ·  native menus: {}", s.last, popup_menu_supported())))
}

fn main() {
    App::new(AppSpec::new("Rune pop-up menus", (320.0, 220.0),
        || State { theme: "system".into(), last: "-".into() }, view)).run();
}
