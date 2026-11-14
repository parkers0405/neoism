//! A selection a handler asks for (`Tx::select_text`) is the one a key later in the same frame
//! copies, on the physical key path native windows use.
use super::*;
use crate::{Cx, El, Env, Key, Mods, PhysicalInput, PhysicalKeyCode, TextStyle, col, text};

#[derive(Default)]
struct S { seen: Option<String> }

fn view(_: &S, _: &mut Cx<S>) -> El {
    col()
        .key("root")
        .w(300.0)
        .padding(10.0)
        .focusable(true)
        .on_physical_input(|s: &mut S, _, tx| {
            s.seen = tx.text_selection().map(|sel| sel.text().to_owned());
            false
        })
        .child(text("lone text here").key("lone").font(TextStyle::new(14.0, 400.0)).w_full().selectable(true))
}

#[test]
fn a_physical_copy_in_the_same_frame_copies_what_code_selected() {
    let mut rt = Runtime::new(S::default(), view, (320.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let clipboard = crate::clipboard::memory();
    rt.use_clipboard(&clipboard);
    rt.set_focus(Some("root".to_owned()), false);
    rt.update(|_, tx| tx.select_text("lone", 5..9));
    let cmd = Mods { cmd: true, ..Mods::default() };
    rt.physical_with_fallback(
        PhysicalInput::Key { key: PhysicalKeyCode::KeyC, mods: cmd, pressed: true, repeat: false },
        Some(KeyEvent { key: Key::Command('c'), mods: cmd }),
    );
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.seen.as_deref(), Some("text"));
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("text"));
}
