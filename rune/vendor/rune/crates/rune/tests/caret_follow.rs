//! Editing keeps the caret line inside the field's nearest scroll viewport, as a browser
//! textarea does.
use rune::runtime::ElementInfo;
use rune::{Color, El, Env, Ime, Key, KeyEvent, Runtime, ScrollSpec, TextEdit, TextStyle, col, el, text_input};

struct S { draft: TextEdit }

fn view(s: &S) -> El {
    col().w_full().h_full().padding(20.0).child(
        col().key("viewport").w_full().h(60.0).scroll(ScrollSpec { stick_bottom: false })
            .child(text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0)).key("input").w_full().on_edit(|s: &mut S| &mut s.draft)),
    )
}

fn find(rt: &Runtime<S>, key: &str) -> ElementInfo {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap()
}

fn assert_visible(rt: &Runtime<S>, what: &str) -> [f32; 4] {
    let c = find(rt, "input").caret.expect("caret");
    let v = find(rt, "viewport").rect;
    assert!(c[1] >= v[1] - 0.01 && c[1] + c[3] <= v[1] + v[3] + 0.01, "{what}: caret {c:?} inside viewport {v:?}");
    c
}

fn key(rt: &mut Runtime<S>, key: Key) {
    rt.key(KeyEvent { key, mods: Default::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn editing_scrolls_the_caret_into_its_viewport() {
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, |s: &S, _: &mut rune::Cx<S>| view(s), (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert_visible(&rt, "empty");
    let mut last = 0.0;
    for n in 1..8 {
        rt.ime(Ime::Commit(format!("\nline {n}")));
        rt.frame(1.0 / 60.0);
        let c = assert_visible(&rt, &format!("line {n}"));
        if n > 3 { assert!((c[1] - last).abs() < 0.01, "pinned to the bottom edge once it overflows"); }
        last = c[1];
    }
    assert!(find(&rt, "input").rect[1] < find(&rt, "viewport").rect[1] - 60.0, "the field scrolled up");

    // Walking the caret back up scrolls just far enough to show each line.
    for n in 0..7 {
        key(&mut rt, Key::Up);
        assert_visible(&rt, &format!("up {n}"));
    }
    let top = find(&rt, "input").rect[1];
    assert!((top - find(&rt, "viewport").rect[1]).abs() < 1.0, "back at the first line: {top}");

    // A wheel scroll away is left alone until the next edit brings the caret back.
    rt.wheel(150.0, 40.0, -100.0);
    rt.frame(1.0 / 60.0);
    assert!(find(&rt, "input").rect[1] < top - 50.0, "the wheel moved the field");
    rt.ime(Ime::Commit("x".into()));
    rt.frame(1.0 / 60.0);
    assert_visible(&rt, "typing after a wheel");
}

fn caret_ink() -> Color { rune::color::hex("#ff0077").unwrap() }

fn single_line_view(s: &S) -> El {
    col().w_full().h_full().padding(20.0).child(
        col().key("viewport").w_full().h(60.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().child(el().h(100.0))
                .child(text_input(&s.draft, TextStyle::new(14.0, 400.0).lh(20.0)).key("input").w(120.0)
                    .single_line().caret(caret_ink(), caret_ink().alpha(0.3)).on_edit(|s: &mut S| &mut s.draft))),
    )
}

#[test]
fn single_line_fields_follow_the_caret_both_ways() {
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, |s: &S, _: &mut rune::Cx<S>| single_line_view(s), (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for word in ["a long single line ", "that runs well past ", "the field's width"] {
        rt.ime(Ime::Commit(word.into()));
        rt.frame(1.0 / 60.0);
    }
    // Down into view vertically, and sideways inside the field.
    let c = assert_visible(&rt, "below the fold");
    let field = find(&rt, "input").rect;
    assert_eq!(field[3], 20.0, "still one line: {field:?}");
    assert!(c[0] > field[0] + 60.0 && c[0] + c[2] <= field[0] + field[2] + 0.01, "caret {c:?} at the field's end {field:?}");
    // The reported caret is the drawn one.
    let want = caret_ink().premul(1.0);
    let drawn = rt.instances.iter().find(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).expect("drawn caret").rect;
    assert!((drawn[0] / 2.0 - c[0]).abs() < 1.0, "drawn {drawn:?} vs {c:?}");
    assert!(drawn[1] / 2.0 >= c[1] && (drawn[1] + drawn[3]) / 2.0 <= c[1] + c[3], "drawn {drawn:?} inside the caret line {c:?}");
    // Home shifts back to the start.
    key(&mut rt, Key::Home);
    let c = assert_visible(&rt, "home");
    assert!((c[0] - field[0]).abs() < 4.0, "caret {c:?} back at the start of {field:?}");
}

fn popup_view(s: &S, cx: &mut rune::Cx<S>) -> El {
    let mut root = view(s);
    if let Some([x, y, _, h]) = cx.caret("input") {
        root = root.child(el().key("popup").absolute(x, y + h).size(80.0, 30.0));
    }
    root
}

#[test]
fn a_caret_popup_lands_where_following_scrolled_the_caret() {
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, popup_view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for n in 1..8 {
        rt.ime(Ime::Commit(format!("\nline {n}")));
        rt.frame(1.0 / 60.0);
        let c = assert_visible(&rt, &format!("line {n}"));
        let p = find(&rt, "popup").target;
        assert_eq!([p[0], p[1]], [c[0], c[1] + c[3]], "line {n}: popup under the scrolled caret");
    }
}
