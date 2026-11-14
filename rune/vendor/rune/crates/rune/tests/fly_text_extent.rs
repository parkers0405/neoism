//! `fly_text` measures its landing spot by the destination's laid-out text, as it does the
//! source, so a body stretched across a column doesn't scale the flight.
use rune::{Cx, El, Env, Key, KeyEvent, Runtime, TextEdit, TextStyle, Tx, col, text, text_input};

const STYLE: TextStyle = TextStyle::new(14.0, 400.0);
const MESSAGE: &str = "hello there, world";

struct State { draft: TextEdit, sent: bool }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut page = col().w_full().h_full();
    if s.sent { page = page.child(text(MESSAGE).font(STYLE).key("body").w(600.0).wrap()); }
    page.child(text_input(&s.draft, STYLE).key("composer").w(400.0).h(60.0)
        .on_edit(|s: &mut State| &mut s.draft)
        .on_submit(|s: &mut State, tx: &mut Tx| {
            s.sent = true;
            s.draft.clear();
            tx.fly_text("composer", "body");
        }))
}

#[test]
fn a_wide_destination_lands_on_its_text_at_rest_scale() {
    let mut draft = TextEdit::new(MESSAGE);
    draft.focused = true;
    let mut rt = Runtime::new(State { draft, sent: false }, view, (800.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    rt.frame(1.0 / 120.0);
    let flight = |rt: &Runtime<State>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("flight-source/body"));
    let start = flight(&rt).expect("the composer's text flies");
    let source_w = start.rect[2];
    let body = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("body")).unwrap();
    assert_eq!(body.target[2], 600.0, "the destination box is the full 600");
    let text_w = start.target[2];
    assert!(text_w > 60.0 && text_w < 200.0, "the landing spot is the text's extent, not the box: {text_w}");
    assert!((text_w - source_w).abs() < 2.0, "the same text at the same style is the same size: {source_w} vs {text_w}");
    assert_eq!(start.target[0], body.target[0], "a start-aligned text starts at its box");
    for _ in 0..30 {
        rt.frame(1.0 / 120.0);
        let Some(f) = flight(&rt) else { break };
        assert!((f.rect[2] / source_w - 1.0).abs() < 0.02, "the flight stays at rest scale: {} vs {source_w}", f.rect[2]);
        assert_eq!(f.target, start.target, "the landing spot holds still");
    }
    for _ in 0..120 { rt.frame(1.0 / 120.0); }
    assert!(flight(&rt).is_none(), "the flight lands and ends");
}

#[test]
fn the_destination_lands_on_its_own_box() {
    let mut draft = TextEdit::new(MESSAGE);
    draft.focused = true;
    let mut rt = Runtime::new(State { draft, sent: false }, view, (800.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
    let body = |rt: &Runtime<State>| rt.elements().into_iter().find(|e| e.key.as_deref() == Some("body")).unwrap().rect;
    let mut last = None;
    for _ in 0..150 {
        rt.frame(1.0 / 120.0);
        let r = body(&rt);
        if let Some(l) = last {
            let l: [f32; 4] = l;
            assert!((r[2] - l[2]).abs() < 2.0 && (r[0] - l[0]).abs() < 2.0, "the body's frame jumped: {l:?} → {r:?}");
        }
        last = Some(r);
    }
    assert_eq!(last.unwrap()[2], 600.0, "at rest it is its box");
}
