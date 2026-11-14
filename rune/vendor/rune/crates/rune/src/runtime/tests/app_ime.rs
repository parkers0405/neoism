use super::*;
use crate::{Key, KeyEvent, Mods, TextEdit, TextStyle, text_input};

#[derive(Default)]
struct Editor {
    /// What `App::on_ime` and `on_key` heard, in order.
    log: Vec<String>,
    field: TextEdit,
    /// The app's editor draws its caret (and so has the input method).
    caret: bool,
}

fn view(s: &Editor, _: &mut Cx<Editor>) -> El {
    col().size(300.0, 200.0).children([
        text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(200.0, 30.0).on_edit(|s: &mut Editor| &mut s.field),
        el().key("page").size(300.0, 100.0)
            .children(s.caret.then(|| el().key("caret").absolute(40.0, 20.0).size(2.0, 18.0).ime_cursor())),
    ])
}

fn editor() -> Runtime<Editor> {
    let mut rt = Runtime::new(Editor { caret: true, ..Default::default() }, view, (300.0, 200.0), Env::new(true, 1.0));
    rt.app_ime.hook = Some(|s, ev, _| { s.log.push(format!("{ev:?}")); true });
    rt.on_key = Some(|s, ev, _| { s.log.push(format!("key {:?}", ev.key)); true });
    rt.frame(0.0);
    rt
}

fn press(rt: &mut Runtime<Editor>, key: Key) {
    rt.key(KeyEvent { key, mods: Mods::default() });
    rt.frame(0.0);
}

#[test]
fn an_ime_cursor_turns_the_input_method_on_at_its_rect_unless_a_field_has_focus() {
    let mut rt = editor();
    assert_eq!(rt.ime_area, Some([40.0, 50.0, 2.0, 18.0]), "the caret's drawn rect, below the 30 pt field");
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    let field = rt.ime_area.expect("the field's caret");
    assert!(field[1] < 30.0, "a focused field's caret wins: {field:?}");
    rt.update(|s, _| { s.field.focused = false; s.caret = false; });
    rt.frame(0.0);
    assert_eq!(rt.ime_area, None, "no caret, no input method");
}

#[test]
fn the_app_hears_the_composition_and_typing_arrives_once() {
    let mut rt = editor();
    press(&mut rt, Key::Text("a".into()));
    rt.ime(Ime::Enabled);
    rt.ime(Ime::Preedit("か".into(), Some((0, 3))));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    press(&mut rt, Key::Escape);
    rt.ime(Ime::Commit("家".into()));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, [
        "key Text(\"a\")", "Enabled", "Preedit(\"か\", Some((0, 3)))", "Commit(\"家\")", "key Enter",
    ], "plain typing is a key; Return and Escape mid-composition are the input method's");
}

#[test]
fn a_focused_field_takes_the_input_method_but_the_apps_composition_ends_in_the_app() {
    let mut rt = editor();
    rt.ime(Ime::Preedit("か".into(), None));
    rt.frame(0.0);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(0.0);
    // Turning the input method over to the field ends the app's composition.
    rt.ime(Ime::Preedit(String::new(), None));
    rt.ime(Ime::Disabled);
    rt.ime(Ime::Preedit("ね".into(), None));
    rt.ime(Ime::Commit("猫".into()));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["Preedit(\"か\", None)", "Preedit(\"\", None)", "Disabled"]);
    assert_eq!(rt.state.field.text, "猫", "the field's own composition stays the field's");
}

#[test]
fn without_on_ime_an_ime_cursor_hears_nothing() {
    let mut rt = editor();
    rt.app_ime.hook = None;
    rt.ime(Ime::Preedit("か".into(), None));
    rt.frame(0.0);
    press(&mut rt, Key::Enter);
    assert_eq!(rt.state.log, ["key Enter"], "nothing composing holds Return back");
}
