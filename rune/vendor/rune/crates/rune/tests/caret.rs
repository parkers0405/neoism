//! The focused field's caret is readable from the view (`Cx::caret`) and tools
//! (`ElementInfo::caret`) in window points, after scrolling and inside an IME composition.
use rune::runtime::ElementInfo;
use rune::{Cx, El, Env, Ime, Key, KeyEvent, Runtime, ScrollSpec, TextEdit, TextStyle, col, el, text_input};

struct S { draft: TextEdit }

fn view(s: &S, cx: &mut Cx<S>) -> El {
    let field = col().key("composer").w_full().padding(6.0)
        .child(text_input(&s.draft, TextStyle::new(14.0, 400.0)).key("input").w_full().on_edit(|s: &mut S| &mut s.draft));
    let mut root = col().w_full().h_full()
        .child(col().key("scroll").w_full().h(120.0).scroll(ScrollSpec { stick_bottom: false })
            .child(col().w_full().child(el().h(60.0)).child(field).child(el().h(200.0))));
    // An @mention popup hangs from the caret line.
    if let Some([x, y, _, h]) = cx.caret("composer") {
        root = root.child(el().key("popup").absolute(x, y + h).size(80.0, 30.0));
    }
    root
}

fn find(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

fn caret(rt: &Runtime<S>) -> [f32; 4] {
    find(rt, "input").and_then(|e| e.caret).expect("the focused field reports its caret")
}

/// The popup sits under the caret line in the same frame the caret moved.
fn popup_follows(rt: &mut Runtime<S>, what: &str) -> [f32; 4] {
    rt.frame(1.0 / 60.0);
    let c = caret(rt);
    let p = find(rt, "popup").unwrap_or_else(|| panic!("{what}: popup")).target;
    assert_eq!([p[0], p[1]], [c[0], c[1] + c[3]], "{what}: popup under the caret");
    let drawn = rt.ime_area.expect("the drawn caret's IME area");
    assert!((drawn[0] - c[0]).abs() < 0.01 && (drawn[1] - c[1]).abs() < 0.01 && (drawn[3] - c[3]).abs() < 0.01,
        "{what}: matches the drawn caret {drawn:?} vs {c:?}");
    c
}

#[test]
fn caret_follows_typing_scrolling_and_composition() {
    let mut draft = TextEdit::new("hi");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let start = popup_follows(&mut rt, "first frame");
    let input = find(&rt, "input").unwrap();
    assert!(start[0] > input.rect[0] + 5.0 && start[0] < input.rect[0] + 30.0, "after \"hi\": {start:?} in {:?}", input.rect);
    assert!(start[1] >= input.rect[1] && start[1] + start[3] <= input.rect[1] + input.rect[3] + 0.01, "on the field's line");
    assert!(start[3] > 10.0 && start[2] > 0.0, "a caret line, not a point: {start:?}");

    rt.ime(Ime::Commit(" there".into()));
    let typed = popup_follows(&mut rt, "typing");
    assert!(typed[0] > start[0] + 20.0, "moves right as text is typed");
    assert_eq!(typed[1], start[1]);

    rt.wheel(150.0, 60.0, -40.0);
    let scrolled = popup_follows(&mut rt, "scrolling");
    assert_eq!(scrolled[0], typed[0]);
    assert!((scrolled[1] - (typed[1] - 40.0)).abs() < 0.01, "rides the scroll: {scrolled:?}");

    // Composing あい with the IME cursor after あ: the caret sits inside the composition.
    rt.ime(Ime::Preedit("あい".into(), Some((3, 3))));
    let mid = popup_follows(&mut rt, "composition");
    rt.ime(Ime::Preedit("あい".into(), Some((6, 6))));
    let end = popup_follows(&mut rt, "composition end");
    assert!(mid[0] > scrolled[0] + 5.0 && end[0] > mid[0] + 5.0, "{scrolled:?} {mid:?} {end:?}");

    rt.update(|s, _| s.draft.focused = false);
    for _ in 0..60 { rt.frame(1.0 / 60.0); }
    assert!(find(&rt, "input").unwrap().caret.is_none(), "no caret without focus");
    assert!(find(&rt, "popup").is_none(), "the view sees it go");
}

/// Arrow keys step whole grapheme clusters; the reported caret lands where the caret is drawn.
#[test]
fn caret_rect_tracks_grapheme_steps() {
    let clusters = ["a", "👨‍👩‍👧", "🇯🇵", "👋🏽", "❤️", "b"];
    let mut draft = TextEdit::new(clusters.concat());
    draft.focused = true;
    let mut rt = Runtime::new(S { draft }, view, (300.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let key = |rt: &mut Runtime<S>, key| rt.key(KeyEvent { key, mods: Default::default() });

    let mut xs = vec![popup_follows(&mut rt, "end")[0]];
    for i in (0..clusters.len()).rev() {
        key(&mut rt, Key::Left);
        let c = popup_follows(&mut rt, &format!("before cluster {i}"));
        assert_eq!(rt.state.draft.focus, clusters[..i].concat().len(), "Left lands before cluster {i}");
        assert!(c[0] < xs.last().unwrap() - 2.0, "cluster {i} moves the caret left: {c:?} after {xs:?}");
        xs.push(c[0]);
    }
    xs.reverse();
    for i in 1..=clusters.len() {
        key(&mut rt, Key::Right);
        let c = popup_follows(&mut rt, &format!("after cluster {i}"));
        assert_eq!(rt.state.draft.focus, clusters[..i].concat().len(), "Right lands after cluster {i}");
        assert!((c[0] - xs[i]).abs() < 0.01, "Right retraces Left at {i}: {} vs {}", c[0], xs[i]);
    }

    // Backspace from just after the family removes the whole cluster and the caret follows.
    for _ in 0..4 { key(&mut rt, Key::Left); }
    key(&mut rt, Key::Backspace);
    let c = popup_follows(&mut rt, "after backspace");
    assert_eq!(rt.state.draft.text, ["a", "🇯🇵", "👋🏽", "❤️", "b"].concat());
    assert!((c[0] - xs[1]).abs() < 0.01, "back after \"a\": {} vs {}", c[0], xs[1]);
}
