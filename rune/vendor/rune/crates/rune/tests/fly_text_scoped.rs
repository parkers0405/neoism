//! `fly_text` destinations can be row-scoped key paths, so every message row can key its text
//! `body` and the flight still lands on the new row's.
use rune::{Cx, El, Env, GrowthSpec, Key, KeyEvent, Runtime, ScrollSpec, TextEdit, TextStyle, Tx, col, text, text_input};

struct State { draft: TextEdit, sent: Vec<String>, destination: &'static str }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut rows = col().w_full().growth(GrowthSpec::default());
    for (i, m) in s.sent.iter().enumerate() {
        rows = rows.child(col().key(format!("msg-{i}")).w_full().py(8.0).child(col().child(text(m.clone()).key("body").wrap())));
    }
    col().w_full().h_full()
        .child(col().key("scroll").w_full().grow(1.0).shrink().scroll(ScrollSpec { stick_bottom: true }).child(rows))
        .child(text_input(&s.draft, TextStyle::new(14.0, 400.0)).key("composer").w_full().h(60.0)
            .on_edit(|s: &mut State| &mut s.draft)
            .on_submit(|s: &mut State, tx: &mut Tx| {
                s.sent.push(s.draft.text.clone());
                s.draft.clear();
                tx.fly_text("composer", s.destination);
            }))
}

fn flight(rt: &Runtime<State>, key: &str) -> Option<[f32; 4]> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).map(|e| e.target)
}

#[test]
fn a_scoped_destination_lands_on_its_own_row() {
    for (destination, lands) in [("msg-2/body", true), ("msg-0/body", true), ("msg-9/body", false)] {
        let mut draft = TextEdit::new("third");
        draft.focused = true;
        let sent = vec!["first".to_string(), "second".to_string()];
        let mut rt = Runtime::new(State { draft, sent, destination }, view, (400.0, 300.0), Env::new(true, 2.0));
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
        rt.frame(1.0 / 120.0);
        rt.frame(1.0 / 120.0);
        let key = format!("flight-source/{destination}");
        let Some(target) = flight(&rt, &key) else {
            assert!(!lands, "{destination}: no row has it, so nothing flies");
            continue;
        };
        assert!(lands, "{destination}");
        // Every row's body shares the key; the flight's destination is the named row's.
        let row = match destination { "msg-2/body" => 2, _ => 0 };
        let bodies: Vec<_> = rt.elements().into_iter().filter(|e| e.key.as_deref() == Some("body")).collect();
        let body = bodies[row].target;
        assert_eq!(target[..2], body[..2], "{destination}: lands on msg-{row}'s text");
        assert!(target[2] <= body[2] && target[3] <= body[3], "{destination}: within msg-{row}'s text box");
        for (i, b) in bodies.iter().enumerate() {
            assert_eq!(b.rect != b.target, i == row, "{destination}: only msg-{row}'s text rides the flight");
        }
        for _ in 0..120 { rt.frame(1.0 / 120.0); }
        assert!(flight(&rt, &key).is_none(), "{destination}: lands and ends");
    }
}
