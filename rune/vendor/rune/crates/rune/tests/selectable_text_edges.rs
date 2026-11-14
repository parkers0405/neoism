//! Selectable text in the places it meets the rest of the app: inside a clickable, draggable or
//! context-menu row, with links beside a focused field, under app shortcuts, at line ends, and
//! when the view changes the text under a live selection.
mod support;

use std::cell::Cell;

use rune::{Color, Cursor, Cx, Drag, El, Env, Key, KeyEvent, Link, Mods, Runtime, TextEdit, TextStyle, col, rich, row, text, text_input};
use support::{click, drag, find, glyphs, tick, wait};

const ROW: &str = "open the project";
const DRAG: &str = "drag this card";
const OWN: &str = "pressable words";
const LINKED: &str = "read the docs today";
const LINES: &str = "first line\nsecond";

#[derive(Default)]
struct S {
    field: TextEdit,
    row_presses: u32,
    row_doubles: u32,
    row_hovers: u32,
    row_menus: u32,
    drags: u32,
    own_presses: u32,
    keys: Vec<String>,
    stale: String,
    stale_selectable: bool,
    stale_shown: bool,
}

thread_local! { static VIEWS: Cell<u32> = const { Cell::new(0) }; }

fn shade() -> Color { Color::WHITE.alpha(0.1) }

fn view(s: &S, _: &mut Cx<S>) -> El {
    VIEWS.with(|v| v.set(v.get() + 1));
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let docs = LINKED.find("docs").unwrap();
    let mut root = col().w_full().h_full().padding(10.0).gap(10.0)
        .child(row().key("row").w(300.0).h(30.0).hover(shade()).press(shade())
            .on_press(|s: &mut S| s.row_presses += 1)
            .on_double_press(|s: &mut S| s.row_doubles += 1)
            .on_hover(|s: &mut S| s.row_hovers += 1)
            .on_secondary_press(|s: &mut S, _, _| s.row_menus += 1)
            .child(text(ROW).key("rowtext").font(style).selectable(true)))
        .child(row().key("dragrow").w(300.0).h(30.0)
            .on_drag(|s: &mut S, _: &mut Drag, _| s.drags += 1)
            .child(text(DRAG).key("dragtext").font(style).selectable(true)))
        .child(text(OWN).key("own").font(style).selectable(true).on_press(|s: &mut S| s.own_presses += 1))
        .child(rich(LINKED, Vec::new()).key("linked").font(style).selectable(true)
            .link(Link::new(docs..docs + 4, "docs")))
        .child(text_input(&s.field, style).key("field").w(200.0).on_edit(|s: &mut S| &mut s.field))
        .child(text(LINES).key("lines").font(style).w(200.0).selectable(true));
    if s.stale_shown {
        root = root.child(text(s.stale.clone()).key("stale").font(style).w(200.0).selectable(s.stale_selectable));
    }
    root
}

fn runtime() -> Runtime<S> {
    let s = S { field: TextEdit::new("field text"), stale: "— a".into(), stale_selectable: true, stale_shown: true, ..S::default() };
    let mut rt = Runtime::new(s, view, (500.0, 400.0), Env::new(true, 2.0));
    // The app sees every key first and takes all but plain Cmd+A and Cmd+C, which it leaves to
    // the selection.
    rt.on_key = Some(|s, ev, _| {
        s.keys.push(format!("{:?} shift={} alt={}", ev.key, ev.mods.shift, ev.mods.alt));
        let m = ev.mods;
        !(matches!(ev.key, Key::Command('a' | 'c')) && !m.shift && !m.alt && !m.ctrl)
    });
    rt.frame(0.0);
    rt
}

fn chord(rt: &mut Runtime<S>, c: char, mods: Mods) {
    rt.key(KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ..mods } });
    tick(rt);
}

#[test]
fn a_clickable_row_keeps_its_clicks_hover_press_and_menu() {
    let mut rt = runtime();
    let g = glyphs(&rt, "rowtext");

    // Hovering the text hovers the row (its handler and its paint) and still shows the I-beam.
    rt.pointer_move(g[2].0, g[2].1);
    tick(&mut rt);
    assert_eq!(rt.state.row_hovers, 1);
    assert_eq!(find(&rt, "row").hover, 1.0);
    assert_eq!(rt.cursor(), Cursor::Text);

    // A held press on the text presses the row; an unmoved click presses it.
    rt.pointer_down(g[2].0, g[2].1);
    tick(&mut rt);
    assert!(find(&rt, "row").press > 0.0, "the row shows pressed");
    rt.pointer_up(g[2].0, g[2].1);
    tick(&mut rt);
    assert_eq!(rt.state.row_presses, 1);
    wait(&mut rt);

    // A double-click is the row's double press and selects the word.
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert_eq!((rt.state.row_presses, rt.state.row_doubles), (3, 1));
    assert_eq!(rt.selected_text().as_deref(), Some("open"));
    wait(&mut rt);

    // A drag that selects text is a selection, not a press.
    let left = find(&rt, "rowtext").rect[0] + 0.5;
    drag(&mut rt, (left, g[0].1), (g[6].0 + 3.0, g[6].1));
    assert_eq!(rt.selected_text().as_deref(), Some("open the"));
    assert_eq!(rt.state.row_presses, 3);

    // The row's own context menu wins over the text's Copy menu.
    rt.secondary_down(g[2].0, g[2].1);
    rt.secondary_up(g[2].0, g[2].1);
    tick(&mut rt);
    assert_eq!(rt.state.row_menus, 1);
    assert!(rt.popup_menu().is_none());
}

#[test]
fn dragging_a_draggable_row_drags_it_and_clicks_still_select() {
    let mut rt = runtime();
    let g = glyphs(&rt, "dragtext");
    drag(&mut rt, g[0], (g[0].0 + 60.0, g[0].1 + 40.0));
    assert!(rt.state.drags >= 3, "down, moves and up reach the row: {}", rt.state.drags);
    assert_eq!(rt.selected_text(), None, "the drag moved the row, not a selection");
    wait(&mut rt);
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert_eq!(rt.selected_text().as_deref(), Some("drag"));
}

#[test]
fn a_drag_select_on_text_with_its_own_press_doesnt_press() {
    let mut rt = runtime();
    let g = glyphs(&rt, "own");
    let left = find(&rt, "own").rect[0] + 0.5;
    drag(&mut rt, (left, g[0].1), (g[4].0 + 3.0, g[4].1));
    assert_eq!(rt.selected_text().as_deref(), Some("press"));
    assert_eq!(rt.state.own_presses, 0);
    wait(&mut rt);
    click(&mut rt, g[1]);
    assert_eq!(rt.state.own_presses, 1, "a click still presses it");
}

#[test]
fn pressing_plain_words_beside_a_link_neither_focuses_nor_rebuilds() {
    let mut rt = runtime();
    let g = glyphs(&rt, "linked");
    let before = VIEWS.with(Cell::get);
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert_eq!(rt.selected_text().as_deref(), Some("read"));
    assert_eq!(VIEWS.with(Cell::get), before, "selecting never re-runs the view");
    assert!(!find(&rt, "linked").focused);
    wait(&mut rt);

    // With a field focused, the press blurs it, as a click on other content does.
    let f = find(&rt, "field");
    click(&mut rt, (f.rect[0] + 10.0, f.rect[1] + f.rect[3] / 2.0));
    assert!(rt.state.field.focused);
    wait(&mut rt);
    click(&mut rt, g[1]);
    assert!(!rt.state.field.focused, "the field lets go");
    assert!(!find(&rt, "linked").focused, "and the text doesn't take focus");
}

#[test]
fn only_the_plain_chords_belong_to_the_selection() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let g = glyphs(&rt, "rowtext");
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert_eq!(rt.selected_text().as_deref(), Some("open"));

    // Shift or Option added: the app's shortcut, the selection untouched.
    chord(&mut rt, 'c', Mods { shift: true, ..Mods::default() });
    chord(&mut rt, 'a', Mods { alt: true, ..Mods::default() });
    assert_eq!(rt.state.keys.len(), 2, "{:?}", rt.state.keys);
    assert!(clipboard.text().is_none());
    assert_eq!(rt.selected_text().as_deref(), Some("open"));
    if cfg!(target_os = "macos") {
        chord(&mut rt, 'a', Mods { ctrl: true, ..Mods::default() });
        assert_eq!(rt.state.keys.len(), 3, "Cmd+Ctrl+A is the app's");
    }
    let app = rt.state.keys.len();
    wait(&mut rt);

    // A single click makes the text the holder: Cmd+C has nothing to copy; Cmd+A, which the
    // app passed on, selects all of it.
    click(&mut rt, g[5]);
    chord(&mut rt, 'c', Mods::default());
    assert!(clipboard.text().is_none(), "an empty selection copies nothing");
    chord(&mut rt, 'a', Mods::default());
    assert_eq!(rt.selected_text().as_deref(), Some(ROW));
    assert_eq!(rt.state.keys.len(), app + 2, "the app saw both first");

    // A click on nothing clears it; Cmd+A then goes back to the text last clicked.
    click(&mut rt, (480.0, 390.0));
    assert_eq!(rt.text_selection(), None);
    chord(&mut rt, 'a', Mods::default());
    assert_eq!(rt.selected_text().as_deref(), Some(ROW));
}

#[test]
fn line_ends_take_the_lines_last_word_and_paragraph() {
    let mut rt = runtime();
    let g = glyphs(&rt, "lines");
    // "first line" is 9 glyphs; right of its last one is the line's end.
    let end = (g[8].0 + 30.0, g[8].1);
    click(&mut rt, end);
    click(&mut rt, end);
    assert_eq!(rt.selected_text().as_deref(), Some("line"));
    click(&mut rt, end);
    assert_eq!(rt.selected_text().as_deref(), Some("first line\n"));
    wait(&mut rt);
    let last = *g.last().unwrap();
    let past = (last.0 + 40.0, last.1);
    click(&mut rt, past);
    click(&mut rt, past);
    assert_eq!(rt.selected_text().as_deref(), Some("second"));
    click(&mut rt, past);
    assert_eq!(rt.selected_text().as_deref(), Some("second"));
}

#[test]
fn a_selection_drops_when_its_text_changes_or_goes() {
    let mut rt = runtime();
    let g = glyphs(&rt, "stale");
    // Triple-click and hold on "— a", then the view changes it to "a —" mid-drag.
    click(&mut rt, g[1]);
    click(&mut rt, g[1]);
    rt.pointer_down(g[1].0, g[1].1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("— a"));
    rt.state.stale = "a —".into();
    rt.invalidate();
    tick(&mut rt);
    let e = find(&rt, "stale");
    for k in 0..6 {
        rt.pointer_move(e.rect[0] + 2.0 + k as f32 * 3.0, g[1].1);
        tick(&mut rt);
    }
    rt.pointer_up(e.rect[0] + 20.0, g[1].1);
    tick(&mut rt);
    assert_eq!(rt.text_selection(), None, "a content change drops the selection");
    assert_eq!(rt.selected_text(), None);
    wait(&mut rt);

    // Selected, then no longer selectable: gone, and Cmd+A is the app's.
    let g = glyphs(&rt, "stale");
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert!(rt.selected_text().is_some());
    rt.state.stale_selectable = false;
    rt.invalidate();
    tick(&mut rt);
    assert_eq!(rt.text_selection(), None);
    chord(&mut rt, 'a', Mods::default());
    assert_eq!(rt.text_selection(), None);
    wait(&mut rt);

    // Selected, then removed.
    rt.state.stale_selectable = true;
    rt.invalidate();
    tick(&mut rt);
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert!(rt.selected_text().is_some());
    rt.state.stale_shown = false;
    rt.invalidate();
    tick(&mut rt);
    assert_eq!(rt.text_selection(), None);
}
