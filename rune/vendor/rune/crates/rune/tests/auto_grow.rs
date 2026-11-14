//! `auto_grow` fits a plain `text_input` to its lines between a minimum and maximum, springs
//! between heights, then scrolls inside with the caret in view.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Ime, Key, KeyEvent, Mods, Runtime, TextEdit, TextStyle, auto_grow, col, text_input};

const LINE: f32 = 20.0;

struct S { draft: TextEdit }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let field = text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(LINE)).key("input").w_full().on_edit(|s: &mut S| &mut s.draft);
    col().w_full().h_full().padding(20.0).child(col().key("composer").w_full().child(auto_grow(field, 2, 4)))
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn newline(rt: &mut Runtime<S>, text: &str) {
    rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } });
    rt.ime(Ime::Commit(text.into()));
    rt.frame(1.0 / 60.0);
}

fn settle(rt: &mut Runtime<S>) {
    for _ in 0..90 { rt.frame(1.0 / 60.0); }
}

#[test]
fn a_plain_field_fits_its_lines_springs_and_then_scrolls() {
    let mut draft = TextEdit::new("one");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, view, (400.0, 400.0), Env::new(true, 2.0));
    settle(&mut rt);
    assert_eq!(find(&rt, "composer").rect[3], 2.0 * LINE, "at least two lines");

    newline(&mut rt, "two");
    newline(&mut rt, "three");
    let shell = find(&rt, "composer");
    assert_eq!(shell.target[3], 3.0 * LINE, "targets three lines");
    assert!(shell.rect[3] > 2.0 * LINE + 0.5 && shell.rect[3] < 3.0 * LINE - 0.5, "mid-spring: {:?}", shell.rect);

    for n in 4..=7 { newline(&mut rt, &format!("line {n}")); }
    settle(&mut rt);
    assert_eq!(find(&rt, "composer").rect[3], 4.0 * LINE, "capped at four lines");
    let (viewport, input) = (find(&rt, "scroll"), find(&rt, "input"));
    let caret = input.caret.expect("caret");
    assert!(caret[1] >= viewport.rect[1] - 0.01 && caret[1] + caret[3] <= viewport.rect[1] + viewport.rect[3] + 0.01,
        "caret {caret:?} inside {:?}", viewport.rect);
}
