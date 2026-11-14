//! Selectable read-only text (`El::selectable`): a drag selects, a double-click takes a word, a
//! triple-click a paragraph and Shift extends; Cmd+C copies, Cmd+A selects all and a right-click
//! offers Copy. Selecting never moves layout, a press elsewhere clears it, links still open, and
//! the highlight is drawn exactly as a text field's.
mod support;

use rune::{Color, Cursor, Cx, El, Env, Link, Mods, PhysicalInput, Runtime, Span, TextEdit, TextStyle, col, el, rich, text, text_input};
use support::{centre, click, command, drag, find, glyphs, tick, wait};

const CMD: &str = "opencode --port 4096";
const PARA: &str = "first line\nsecond line that wraps around";
const LINKED: &str = "read the docs today";
const BIDI: &str = "abc שלום def";

struct S { field: TextEdit, presses: u32, opened: Vec<String> }

fn highlight() -> Color { rune::color::hex("#3080ff").unwrap().alpha(0.3) }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let docs = LINKED.find("docs").unwrap();
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(text(CMD).key("cmd").font(style.mono()).selectable(true).selection_color(highlight()))
        .child(rich(PARA, vec![Span { color: Some(Color::WHITE), ..Span::new(0, 5) }]).key("para").font(style).w(140.0)
            .selectable(true).selection_color(highlight()))
        .child(rich(LINKED, Vec::new()).key("linked").font(style).selectable(true)
            .link(Link::new(docs..docs + 4, "docs"))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
        .child(text(BIDI).key("bidi").font(style).selectable(true))
        .child(text_input(&s.field, style).key("field").w(200.0).selection_color(highlight()).on_edit(|s: &mut S| &mut s.field))
        .child(el().key("other").size(80.0, 30.0).on_press(|s: &mut S| s.presses += 1))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S { field: TextEdit::new("field text"), presses: 0, opened: Vec::new() }, view, (500.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn rects(rt: &Runtime<S>) -> Vec<(Option<String>, [f32; 4])> {
    rt.elements().into_iter().map(|e| (e.key, e.rect)).collect()
}

/// Instances filled with `c` (the selection boxes): their rect heights and corner radii.
fn boxes(rt: &Runtime<S>, c: Color) -> Vec<(f32, f32)> {
    let want = c.premul(1.0);
    rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).map(|i| (i.rect[3], i.radii[0])).collect()
}

#[test]
fn drag_double_and_triple_click_select_without_moving_layout() {
    let mut rt = runtime();
    let before = rects(&rt);
    let g = glyphs(&rt, "cmd");
    assert_eq!(g.len(), CMD.chars().filter(|c| *c != ' ').count(), "one glyph per letter: {g:?}");

    // The I-beam over selectable text; the text isn't a Tab stop.
    rt.pointer_move(g[3].0, g[3].1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Text);
    assert!(!find(&rt, "cmd").focusable);

    // A drag from the left edge of "o" into "port" selects up to the pointer.
    let left = find(&rt, "cmd").rect[0] + 0.5;
    drag(&mut rt, (left, g[0].1), (g[12].0 + 2.0, g[12].1));
    assert_eq!(rt.selected_text().as_deref(), Some("opencode --por"));
    assert_eq!(find(&rt, "cmd").selected.as_deref(), Some("opencode --por"), "tools see the selection");
    assert_eq!(boxes(&rt, highlight()).len(), 1, "one highlight box");
    assert_eq!(rects(&rt), before, "selecting never moves anything");
    wait(&mut rt);

    // Double-click takes the word, triple-click the whole line.
    click(&mut rt, g[11]);
    click(&mut rt, g[11]);
    assert_eq!(rt.selected_text().as_deref(), Some("port"));
    click(&mut rt, g[11]);
    assert_eq!(rt.selected_text().as_deref(), Some(CMD));
    wait(&mut rt);

    // A double-click drag extends by whole words.
    rt.pointer_move(g[1].0, g[1].1);
    rt.pointer_down(g[1].0, g[1].1);
    rt.pointer_up(g[1].0, g[1].1);
    tick(&mut rt);
    rt.pointer_down(g[1].0, g[1].1);
    tick(&mut rt);
    rt.pointer_move(g[11].0, g[11].1);
    tick(&mut rt);
    rt.pointer_up(g[11].0, g[11].1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("opencode --port"));
    wait(&mut rt);

    // Shift-click extends from the anchor.
    click(&mut rt, (left, g[0].1));
    assert_eq!(rt.text_selection().map(|(_, r)| r), Some(0..0));
    assert_eq!(rt.selected_text(), None, "a single click selects nothing");
    rt.physical_input(PhysicalInput::Modifiers(Mods { shift: true, ..Mods::default() }));
    click(&mut rt, (g[7].0 + 3.0, g[7].1));
    rt.physical_input(PhysicalInput::Modifiers(Mods::default()));
    assert_eq!(rt.selected_text().as_deref(), Some("opencode"));
    wait(&mut rt);

    // Triple-click in a wrapped paragraph takes its paragraph, line break included; a drag
    // across the wrapped second paragraph highlights each line.
    let p = glyphs(&rt, "para");
    click(&mut rt, p[2]);
    click(&mut rt, p[2]);
    click(&mut rt, p[2]);
    assert_eq!(rt.selected_text().as_deref(), Some("first line\n"));
    wait(&mut rt);
    let second = PARA.find("second").unwrap();
    let last = *p.last().unwrap();
    let start = find(&rt, "para").rect[0] + 0.5;
    drag(&mut rt, (start, p[9].1), (last.0 + 4.0, last.1));
    let got = rt.selected_text().unwrap();
    assert!(got.starts_with("second") && got.ends_with("around"), "{got:?}");
    assert!(boxes(&rt, highlight()).len() >= 2, "a box per wrapped line");
    assert_eq!(rt.text_selection().map(|(k, r)| (k.to_owned(), r.start)).unwrap().1, second);
    assert_eq!(rects(&rt), before);

    // A press elsewhere clears it, and still presses.
    let other = centre(&find(&rt, "other"));
    click(&mut rt, other);
    assert_eq!(rt.text_selection(), None);
    assert_eq!(rt.state.presses, 1);
    assert!(boxes(&rt, highlight()).is_empty());
}

#[test]
fn copy_select_all_and_the_context_menu() {
    let mut rt = runtime();
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let g = glyphs(&rt, "cmd");

    // Cmd+C copies the selection; with nothing selected the key falls through.
    command(&mut rt, 'c');
    assert!(clipboard.text().is_none());
    click(&mut rt, g[11]);
    click(&mut rt, g[11]);
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("port"));

    // Cmd+A selects all of the text holding the selection.
    command(&mut rt, 'a');
    assert_eq!(rt.selected_text().as_deref(), Some(CMD));
    command(&mut rt, 'c');
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some(CMD));
    wait(&mut rt);

    // A right-click outside the selection selects the word there and offers Copy.
    let p = glyphs(&rt, "para");
    rt.pointer_move(p[1].0, p[1].1);
    rt.secondary_down(p[1].0, p[1].1);
    rt.secondary_up(p[1].0, p[1].1);
    tick(&mut rt);
    assert_eq!(rt.selected_text().as_deref(), Some("first"));
    let copy = rt.popup_menu().and_then(|m| m.find("copy")).expect("a Copy menu");
    assert!(copy.title() == "Copy" && copy.is_enabled());
    rt.resolve_popup(Some("copy")).unwrap();
    tick(&mut rt);
    assert_eq!(clipboard.text().as_deref().map(String::as_str), Some("first"));
    assert_eq!(rt.selected_text().as_deref(), Some("first"), "picking Copy keeps the selection");
}

#[test]
fn links_open_and_plain_words_select() {
    let mut rt = runtime();
    let docs = find(&rt, "docs");
    assert_eq!(docs.role, "link");
    rt.pointer_move(centre(&docs).0, centre(&docs).1);
    tick(&mut rt);
    assert_eq!(rt.cursor(), Cursor::Pointer, "a hand over the link");
    click(&mut rt, centre(&docs));
    assert_eq!(rt.state.opened, ["docs"]);
    assert_eq!(rt.selected_text(), None, "clicking a link selects nothing");
    wait(&mut rt);

    let g = glyphs(&rt, "linked");
    click(&mut rt, g[0]);
    click(&mut rt, g[0]);
    assert_eq!(rt.selected_text().as_deref(), Some("read"));
    assert_eq!(rt.state.opened, ["docs"]);
}

#[test]
fn bidi_text_selects_by_logical_words() {
    let mut rt = runtime();
    let g = glyphs(&rt, "bidi");
    if g.len() != 10 {
        eprintln!("skipping bidi checks: no Hebrew font ({} glyphs)", g.len());
        return;
    }
    // Visual order: a b c, the Hebrew word (drawn right to left), d e f.
    click(&mut rt, g[5]);
    click(&mut rt, g[5]);
    assert_eq!(rt.selected_text().as_deref(), Some("שלום"));
    wait(&mut rt);
    let e = find(&rt, "bidi");
    drag(&mut rt, (e.rect[0] + 0.5, g[0].1), (g[9].0 + 6.0, g[9].1));
    assert_eq!(rt.selected_text().as_deref(), Some(BIDI));
}

#[test]
fn the_highlight_matches_a_text_fields() {
    let mut rt = runtime();
    rt.state.field.focused = true;
    rt.state.field.anchor = 0;
    rt.state.field.focus = 5;
    rt.invalidate();
    tick(&mut rt);
    let field = boxes(&rt, highlight());
    assert_eq!(field.len(), 1, "the field's selection box");

    let g = glyphs(&rt, "cmd");
    click(&mut rt, g[1]);
    click(&mut rt, g[1]);
    let selectable = boxes(&rt, highlight());
    assert_eq!(rt.selected_text().as_deref(), Some("opencode"));
    assert_eq!(selectable, field, "same fill, height and corners");
}
