//! `Tx::blur` drops keyboard focus from a handler.
use rune::tok::TextStyle;
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, col, text_input};

#[derive(Default)]
struct S {
    field: TextEdit,
    submits: u32,
    keys: u32,
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    col().size(200.0, 100.0).child(
        text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").w(150.0)
            .on_edit(|s: &mut S| &mut s.field)
            .on_submit(|s: &mut S, tx| { s.submits += 1; tx.blur(); }),
    )
}

fn focused(rt: &Runtime<S>) -> bool {
    rt.elements().iter().any(|e| e.key.as_deref() == Some("field") && e.focused)
}

fn runtime() -> Runtime<S> {
    let mut s = S::default();
    s.field.focused = true;
    let mut rt = Runtime::new(s, view, (200.0, 100.0), Env::default());
    rt.on_key = Some(|s, _, _| { s.keys += 1; true });
    rt.frame(0.0);
    assert!(focused(&rt), "the model asked for focus");
    rt
}

fn press(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn a_submit_handler_blurs_its_field() {
    let mut rt = runtime();
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.submits, 1);
    assert!(!focused(&rt), "nothing has focus");
    assert!(!rt.state.field.focused, "the model hears it through on_edit");
    rt.frame(1.0 / 60.0);
    assert!(!focused(&rt), "and focus stays dropped");
    press(&mut rt, Key::Text("z".into()));
    assert_eq!(rt.state.field.text.as_str(), "", "typing no longer reaches the field");
    assert_eq!(rt.state.keys, 1, "keys go to the window");
}

#[test]
fn an_update_blurs_and_asks_for_a_frame() {
    let mut rt = runtime();
    assert!(rt.update_if(|_, tx| { tx.blur(); false }), "a blur that drops focus is frame work");
    rt.frame(1.0 / 60.0);
    assert!(!focused(&rt));
    assert!(!rt.update_if(|_, tx| { tx.blur(); false }), "with nothing focused it's nothing");
}
