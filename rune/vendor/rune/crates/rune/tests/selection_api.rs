//! The text selection from code: `Tx::select_text`, `Tx::select_all_text` and
//! `Tx::clear_text_selection` set it after the handler's commit (in time for a key in the same
//! frame), `Tx::text_selection` reads it, `El::selection_copy` decides what copying takes and
//! `El::on_selection_change` hears it change.
use std::cell::RefCell;
use std::ops::Range;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, TextSelection, TextStyle, col, row, text, text_input};

#[derive(Default)]
struct S { extra: bool, seen: Option<TextSelection>, field: TextEdit, app_keys: Vec<char>, heard: Vec<Heard> }

/// One `on_selection_change` call: started, cleared, and the selection's text.
#[derive(Debug, PartialEq)]
struct Heard(bool, bool, Option<String>);

thread_local! {
    /// What the `selection_copy` provider saw last: (path, range, whole).
    static ASKED: RefCell<Vec<(String, Range<usize>, bool)>> = const { RefCell::new(Vec::new()) };
}

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let t = |content: &str, key: &str| text(content.to_owned()).key(key).font(style).w_full().selectable(true);
    col().w_full().h_full().padding(10.0).gap(12.0)
        .child(col().key("doc").w(400.0).gap(12.0).selection_group(true)
            .on_selection_change(|s: &mut S, c, _| s.heard.push(Heard(c.started(), c.cleared(), c.selection().map(|t| t.text().to_owned()))))
            .selection_copy(|sel| {
                ASKED.with_borrow_mut(|a| *a = sel.texts().iter().map(|t| (t.path().to_owned(), t.range(), t.is_whole())).collect());
                sel.texts().iter().all(|t| t.is_whole()).then(|| "SOURCE".to_owned())
            })
            .child(t("first paragraph", "a"))
            .child(row().key("r").w_full().child(text("•").font(style).w(20.0)).child(t("second", "b").grow(1.0).shrink())))
        .child(t("lone text here", "lone").w(300.0))
        .child(t("café ok", "cafe").w(300.0))
        .children(s.extra.then(|| t("extra words", "extra").w(300.0)))
        .child(text_input(&s.field, style).single_line().key("field").w(300.0).on_edit(|s: &mut S| &mut s.field))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (500.0, 500.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn tick(rt: &mut Runtime<S>) { rt.frame(1.0 / 60.0); }

fn command(rt: &mut Runtime<S>, c: char) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..Mods::default() } });
    tick(rt);
}

fn act(rt: &mut Runtime<S>, f: impl FnOnce(&mut rune::Tx)) {
    rt.update(|_, tx| f(tx));
    tick(rt);
}

#[test]
fn code_selects_a_range_reads_it_and_clears_it() {
    let mut rt = runtime();
    act(&mut rt, |tx| tx.select_text("lone", 5..9));
    assert_eq!(rt.selected_text().as_deref(), Some("text"));
    rt.update(|s, tx| s.seen = tx.text_selection().cloned());
    let seen = rt.state.seen.take().expect("a handler reads the selection");
    assert_eq!(seen.text(), "text");
    let [t] = seen.texts() else { panic!("{seen:?}") };
    assert!(t.path().ends_with("/lone") && t.key() == Some("lone"), "{t:?}");
    assert_eq!((t.range(), t.len(), t.text(), t.is_whole()), (5..9, 14, "text", false));

    // Clamped to the text and its character boundaries.
    act(&mut rt, |tx| tx.select_text("lone", 10..999));
    assert_eq!(rt.selected_text().as_deref(), Some("here"));
    act(&mut rt, |tx| tx.select_text("cafe", 0..4));
    assert_eq!(rt.selected_text().as_deref(), Some("caf"), "byte 4 is inside the é");
    // A key that names no selectable text leaves it be.
    act(&mut rt, |tx| tx.select_text("r", 0..2));
    act(&mut rt, |tx| tx.select_text("nothing", 0..2));
    assert_eq!(rt.selected_text().as_deref(), Some("caf"));

    act(&mut rt, |tx| tx.clear_text_selection());
    assert_eq!(rt.selected_text(), None);
    rt.update(|s, tx| s.seen = tx.text_selection().cloned());
    assert_eq!(rt.state.seen, None);
}

#[test]
fn select_all_takes_a_group_a_part_of_one_or_a_text() {
    let mut rt = runtime();
    act(&mut rt, |tx| tx.select_all_text("doc"));
    assert_eq!(rt.selected_text().as_deref(), Some("first paragraph\nsecond"));
    act(&mut rt, |tx| tx.select_all_text("r"));
    assert_eq!(rt.selected_text().as_deref(), Some("second"), "the texts inside a part of the group");
    act(&mut rt, |tx| tx.select_all_text("lone"));
    assert_eq!(rt.selected_text().as_deref(), Some("lone text here"));
    // Then Cmd+A widens a text's selection to its group, as after a click.
    act(&mut rt, |tx| tx.select_text("a", 0..5));
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("first paragraph\nsecond"));
}

#[test]
fn a_text_the_same_change_builds_selects_and_keeps_its_selection_across_rebuilds() {
    let mut rt = runtime();
    rt.update(|s, tx| {
        s.extra = true;
        tx.select_all_text("extra");
    });
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("extra words"));
    for _ in 0..3 {
        rt.invalidate();
        tick(&mut rt);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("extra words"));
}

#[test]
fn a_key_in_the_same_frame_copies_what_code_selected() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    rt.update(|_, tx| tx.select_text("lone", 0..4));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("lone"));
}

#[test]
fn selection_copy_says_what_copying_takes() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    act(&mut rt, |tx| tx.select_all_text("doc"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("SOURCE"));
    assert_eq!(ASKED.take(), [("a".to_owned(), 0..15, true), ("r/b".to_owned(), 0..6, true)], "paths relative to the element");
    rt.update(|s, tx| s.seen = tx.text_selection().cloned());
    assert_eq!(rt.state.seen.as_ref().map(TextSelection::text), Some("first paragraph\nsecond"), "reading is as drawn");

    // It returns None for part of the group: the drawn text, and it sees what isn't selected.
    act(&mut rt, |tx| tx.select_text("b", 0..3));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("sec"));
    assert_eq!(ASKED.take(), [("a".to_owned(), 0..0, false), ("r/b".to_owned(), 0..3, false)]);

    // The Copy menu asks it too.
    act(&mut rt, |tx| tx.select_all_text("doc"));
    let a = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("a")).unwrap();
    let (x, y) = (a.rect[0] + 20.0, a.rect[1] + a.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.secondary_down(x, y);
    rt.secondary_up(x, y);
    tick(&mut rt);
    rt.resolve_popup(Some("copy")).unwrap();
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("SOURCE"));

    // A text outside it copies as drawn.
    ASKED.take();
    act(&mut rt, |tx| tx.select_all_text("lone"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("lone text here"));
    assert!(ASKED.take().is_empty());
}

/// An app's `on_key` that takes Cmd+A and Cmd+C copies the note its own way.
fn app_takes_copy_keys(rt: &mut Runtime<S>) {
    rt.on_key = Some(|s, ev, tx| match ev.key {
        Key::Command(c @ ('a' | 'c')) => {
            s.app_keys.push(c);
            if c == 'c' { tx.copy("APP"); }
            true
        }
        _ => false,
    });
}

#[test]
fn an_apps_on_key_takes_the_copy_keys_before_the_selection() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    app_takes_copy_keys(&mut rt);
    act(&mut rt, |tx| tx.select_text("lone", 0..4));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("APP"));
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("lone"), "Cmd+A was the app's");
    assert_eq!(rt.state.app_keys, ['c', 'a']);

    // An app that passes them on leaves them to the selection.
    rt.on_key = Some(|s, ev, _| { if let Key::Command(c) = ev.key { s.app_keys.push(c); } false });
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some("lone text here"));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("lone text here"));
    assert_eq!(rt.state.app_keys, ['c', 'a', 'a', 'c'], "the app saw them first");
}

#[test]
fn a_focused_field_keeps_its_copy_keys_from_the_app() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    app_takes_copy_keys(&mut rt);
    rt.state.field = TextEdit::new("field words");
    rt.state.field.focused = true;
    rt.invalidate();
    tick(&mut rt);
    command(&mut rt, 'a');
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("field words"));
    assert!(rt.state.app_keys.is_empty(), "{:?}", rt.state.app_keys);
}

fn heard(rt: &mut Runtime<S>) -> Vec<Heard> { std::mem::take(&mut rt.state.heard) }

#[test]
fn on_selection_change_hears_a_drag_start_change_and_clear_once_a_frame() {
    let mut rt = runtime();
    let a = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("a")).unwrap();
    let (x0, y) = (a.rect[0] + 1.0, a.rect[1] + a.rect[3] / 2.0);
    rt.pointer_move(x0, y);
    rt.pointer_down(x0, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [], "a press leaves only a caret");
    // Three moves a frame, heard once a frame: started, then changed.
    let mut frames = 0;
    for step in 1..=3 {
        for k in 1..=3 {
            let x = x0 + (step * 3 + k - 3) as f32 * 6.0;
            rt.pointer_move(x, y);
        }
        tick(&mut rt);
        frames += 1;
    }
    let calls = heard(&mut rt);
    assert_eq!(calls.len(), frames, "{calls:?}");
    assert!(calls[0].0 && !calls[0].1, "the first is the start: {calls:?}");
    assert!(calls[1..].iter().all(|c| !c.0 && !c.1 && c.2.as_ref().is_some_and(|t| "first paragraph".starts_with(t.as_str()))), "{calls:?}");
    let last = calls.last().and_then(|c| c.2.clone());
    rt.pointer_up(x0 + 54.0, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(false, false, last)], "the release is heard once, with the same selection");

    // Cmd+A widens it; a click outside the group clears it.
    command(&mut rt, 'a');
    assert_eq!(heard(&mut rt), [Heard(false, false, Some("first paragraph\nsecond".into()))]);
    let lone = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("lone")).unwrap();
    let (x, y) = (lone.rect[0] + 5.0, lone.rect[1] + lone.rect[3] / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(false, true, None)]);
}

#[test]
fn on_selection_change_hears_code_and_selections_elsewhere_dont_reach_it() {
    let mut rt = runtime();
    act(&mut rt, |tx| tx.select_text("b", 0..3));
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(true, false, Some("sec".into()))]);
    act(&mut rt, |tx| tx.select_all_text("doc"));
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(false, false, Some("first paragraph\nsecond".into()))]);
    // The same selection again is no change.
    act(&mut rt, |tx| tx.select_all_text("doc"));
    tick(&mut rt);
    assert_eq!(heard(&mut rt), []);
    // Moving to a text outside it leaves it: cleared, and nothing more while it's elsewhere.
    act(&mut rt, |tx| tx.select_all_text("lone"));
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(false, true, None)]);
    act(&mut rt, |tx| tx.select_text("lone", 0..4));
    tick(&mut rt);
    assert_eq!(heard(&mut rt), []);
    act(&mut rt, |tx| tx.select_all_text("a"));
    act(&mut rt, |tx| tx.clear_text_selection());
    tick(&mut rt);
    assert_eq!(heard(&mut rt), [Heard(true, false, Some("first paragraph".into())), Heard(false, true, None)]);
}

#[test]
fn a_focused_field_keeps_a_static_selection_across_commits() {
    let mut rt = runtime();
    rt.state.field = TextEdit::new("field words");
    rt.state.field.focused = true;
    rt.invalidate();
    tick(&mut rt);
    act(&mut rt, |tx| tx.select_text("lone", 0..4));
    assert_eq!(rt.selected_text().as_deref(), Some("lone"));
    for _ in 0..3 {
        rt.invalidate();
        tick(&mut rt);
    }
    assert_eq!(rt.selected_text().as_deref(), Some("lone"), "staying focused on the field keeps the static selection");
}

#[test]
fn scroll_into_view_and_select_text_in_one_handler_selects_a_culled_row() {
    #[derive(Default)]
    struct V;
    fn vview(_: &V, _: &mut Cx<V>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w(300.0).h(80.0).scroll(rune::ScrollSpec { stick_bottom: false }).child(
            col()
                .key("list")
                .w_full()
                .growth(rune::GrowthSpec::default())
                .virtual_rows(
                    100,
                    24.0,
                    |_: &V, i| format!("r{i}").into(),
                    move |_: &V, i| {
                        row().key(format!("r{i}")).w_full().h(24.0).child(
                            text(format!("row {i} content"))
                                .key(format!("t{i}"))
                                .font(style)
                                .w_full()
                                .selectable(true),
                        )
                    },
                ),
        )
    }
    let mut rt = Runtime::new(V, vview, (320.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    assert!(rt.elements().iter().all(|e| e.key.as_deref() != Some("t80")), "row 80 starts culled");
    rt.update(|_, tx| {
        tx.scroll_into_view("r80");
        tx.select_text("t80", 0..6);
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("row 80"));
}

#[test]
fn on_selection_change_calling_select_does_not_loop() {
    #[derive(Default)]
    struct LoopState { calls: Vec<Option<String>> }
    fn lview(_: &LoopState, _: &mut Cx<LoopState>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().key("doc").w(300.0).selection_group(true)
            .on_selection_change(|s: &mut LoopState, c, tx| {
                s.calls.push(c.selection().map(|sel| sel.text().to_owned()));
                if c.selection().is_some_and(|sel| sel.text() == "hello") {
                    tx.select_all_text("doc");
                }
            })
            .child(text("hello world").key("a").font(style).w_full().selectable(true))
    }
    let mut rt = Runtime::new(LoopState::default(), lview, (320.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.select_text("a", 0..5));
    for _ in 0..5 {
        rt.frame(1.0 / 60.0);
    }
    assert_eq!(rt.state.calls, [Some("hello".to_owned())], "widening inside on_selection_change does not re-trigger it");
    assert_eq!(rt.selected_text().as_deref(), Some("hello world"));
}

#[test]
fn keyed_selection_survives_when_an_unkeyed_ancestor_shifts_index() {
    #[derive(Default)]
    struct Shift { banner: bool, heard: Vec<Heard> }
    fn sview(s: &Shift, _: &mut Cx<Shift>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().w(300.0).gap(8.0)
            .children(s.banner.then(|| row().w_full().child(text("banner").font(style))))
            .child(
                col().w_full().selection_group(true)
                    .on_selection_change(|s: &mut Shift, c, _| {
                        s.heard.push(Heard(c.started(), c.cleared(), c.selection().map(|t| t.text().to_owned())));
                    })
                    .child(text("alpha beta").key("msg").font(style).w_full().selectable(true)),
            )
    }
    let mut rt = Runtime::new(Shift::default(), sview, (320.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.select_text("msg", 0..5));
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha"));
    assert_eq!(std::mem::take(&mut rt.state.heard), [Heard(true, false, Some("alpha".into()))]);

    // Inserting an unkeyed sibling before the unkeyed wrapper shifts `#0/msg` to `#1/msg`.
    rt.update(|s, _| s.banner = true);
    rt.frame(1.0 / 60.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("alpha"), "keyed text keeps its selection across unkeyed wrapper shift");
    assert_eq!(rt.state.heard, [], "no bogus clear/start when the selection did not change");
}

#[test]
fn secure_text_is_never_selectable_or_copied() {
    #[derive(Default)]
    struct Sec;
    fn sec_view(_: &Sec, _: &mut Cx<Sec>) -> El {
        let style = TextStyle::new(14.0, 400.0).lh(20.0);
        col().key("doc").w(300.0).selection_group(true)
            .child(text("public").key("pub").font(style).w_full().selectable(true))
            .child(text("top-secret").key("sec").font(style).w_full().selectable(true).secure())
    }
    let mut rt = Runtime::new(Sec, sec_view, (320.0, 120.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.update(|_, tx| tx.select_all_text("sec"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text(), None);
    rt.update(|_, tx| tx.select_all_text("doc"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some("public"));
}
