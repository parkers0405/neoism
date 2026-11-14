//! `Tx::launch` into a panel pinned to the bottom of the window: each new row grows the panel,
//! which springs up, and the rows already there go up with it.
use rune::{Cx, El, Env, Key, KeyEvent, Runtime, TextEdit, TextStyle, Tx, col, el, text_input};

struct State { draft: TextEdit, rows: usize }

fn view(s: &State, _: &mut Cx<State>) -> El {
    let mut list = col().key("list").w_full();
    for i in 0..s.rows {
        list = list.child(el().key(format!("row-{i}")).w_full().h(32.0));
    }
    col().w_full().h_full()
        .child(el().grow(1.0))
        .child(col().key("panel").w_full().pt(6.0).child(list))
        .child(text_input(&s.draft, TextStyle::new(14.0, 400.0)).key("composer").w_full().h(60.0)
            .on_edit(|s: &mut State| &mut s.draft)
            .on_submit(|s: &mut State, tx: &mut Tx| {
                s.draft.clear();
                tx.launch("composer", format!("row-{}", s.rows));
                s.rows += 1;
            }))
}

fn y(rt: &Runtime<State>, key: &str) -> f32 {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key)).unwrap().rect[1]
}

fn send(rt: &mut Runtime<State>) {
    rt.update(|s, _| s.draft = TextEdit::new("next"));
    rt.update(|s, _| s.draft.focused = true);
    rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
}

/// Sends a first row, then a second `after` frames later, and returns the first row's drawn
/// top on the frame before the second send and every frame from it on, with the list's.
fn second_send(after: usize) -> Vec<(f32, f32)> {
    let mut rt = Runtime::new(State { draft: TextEdit::new(""), rows: 0 }, view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    send(&mut rt);
    for _ in 0..after { rt.frame(1.0 / 120.0); }
    let before = (y(&rt, "row-0"), y(&rt, "list"));
    send(&mut rt);
    std::iter::once(before).chain((0..120).map(|_| { rt.frame(1.0 / 120.0); (y(&rt, "row-0"), y(&rt, "list")) })).collect()
}

#[test]
fn a_launched_row_goes_up_with_its_springing_panel_without_a_jump() {
    for after in [20, 90] {
        let frames = second_send(after);
        let at: Vec<f32> = frames.iter().map(|f| f.0).collect();
        let steps: Vec<f32> = at.windows(2).map(|w| w[1] - w[0]).collect();
        let worst = steps.iter().copied().fold(0.0f32, |a, s| if s.abs() > a.abs() { s } else { a });
        assert!(worst.abs() < 4.0, "second send {after} frames on: the first row steps {worst} pt in a frame: {at:?}");
        let (row, list) = frames[frames.len() - 1];
        assert_eq!(row, list, "it lands at the top of the list");
    }
}

#[test]
fn a_landed_launch_rides_its_springing_panel() {
    let frames = second_send(90);
    for (f, (row, list)) in frames.iter().enumerate() {
        assert!((row - list).abs() < 0.25, "frame {f}: the first row sits on the list's top: {row} vs {list}");
    }
}

