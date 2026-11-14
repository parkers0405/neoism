//! `El::max_lines`: wrapped text stops after n lines with "…" ending the last, measures n lines
//! tall, cuts between graphemes, refits on resize, and copying and tools keep the whole string.
use super::*;
use crate::{Color, Cx, El, Env, Key, KeyEvent, Link, Mods, Span, TextStyle, col, rich};
use unicode_segmentation::UnicodeSegmentation;

const TITLE: &str = "Move the sidebar's search into the toolbar, keep its results in the list, and let the \
    filter chips wrap onto a second row when the window is narrow enough that they would overflow";

fn style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(16.0) }

/// Content, the line limit, and a byte range that is both a coloured span and a link.
type S = (&'static str, usize, Option<(usize, usize)>);

fn mark() -> Color { crate::color::hex("#e6339a").unwrap() }

fn clamp(content: &'static str, lines: usize, marked: Option<(usize, usize)>, width: f32) -> Runtime<S> {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let spans = s.2.map(|(a, b)| Span { color: Some(mark()), ..Span::new(a, b) }).into_iter().collect();
        let mut t = rich(s.0, spans).key("t").font(style()).selectable(true).max_lines(s.1);
        if let Some((a, b)) = s.2 { t = t.link(Link::new(a..b, "marked")).on_link(|_: &mut S, _, _| {}); }
        col().w_full().child(t)
    }
    let mut rt = Runtime::new((content, lines, marked), view, (width, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn node<'a>(rt: &'a Runtime<S>) -> &'a Node {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap()
}

/// The string node "t" draws, its cut, its line count and its height in points.
fn drawn(rt: &Runtime<S>) -> (String, Option<Elision>, usize, f32) {
    let n = node(rt);
    let content = &n.text.as_ref().unwrap().content;
    let shown = match n.elided {
        Some(e) => [&content[..e.head], "…", &content[e.tail..]].concat(),
        None => content.clone(),
    };
    (shown, n.elided, n.text_layout.as_ref().unwrap().len(), n.target[3] - n.target[1])
}

#[test]
fn stops_after_n_lines_with_an_ellipsis_and_measures_n_lines() {
    let mut rt = clamp(TITLE, 3, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    let e = e.expect("the title runs past three lines at 200 pt");
    assert!(shown.ends_with('…') && !shown[..shown.len() - "…".len()].ends_with(' '), "{shown}");
    assert_eq!(e.tail, TITLE.len(), "everything after the cut is hidden");
    assert_eq!((lines, h), (3, 48.0), "three 16 pt lines: {shown}");
    let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
    assert_eq!(el.text.as_deref(), Some(TITLE), "tools and accessibility read the whole title");

    rt.set_viewport((320.0, 400.0), 2.0);
    rt.frame(0.1);
    let (wider, _, lines, h) = drawn(&rt);
    assert!(wider.len() > shown.len() && (lines, h) == (3, 48.0), "a wider box keeps more: {wider}");

    rt.set_viewport((2000.0, 400.0), 2.0);
    rt.frame(0.2);
    let (all, e, lines, h) = drawn(&rt);
    assert_eq!((all.as_str(), e, lines, h), (TITLE, None, 1, 16.0), "room for it all: no cut, one line tall");

    rt.set_viewport((200.0, 400.0), 2.0);
    rt.frame(0.3);
    assert_eq!(drawn(&rt).0, shown, "narrowing again cuts as before");
}

#[test]
fn text_within_the_limit_is_untouched() {
    let rt = clamp("Short title", 3, None, 200.0);
    assert_eq!(drawn(&rt), ("Short title".to_owned(), None, 1, 16.0));
    // 0 shows every line.
    let rt = clamp(TITLE, 0, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    assert!(shown == TITLE && e.is_none() && lines > 3 && h == 16.0 * lines as f32);
}

#[test]
fn a_line_break_before_the_cut_is_dropped() {
    let rt = clamp("first\nsecond\nthird", 2, None, 400.0);
    assert_eq!(drawn(&rt).0, "first\nsecond…");
    assert_eq!(drawn(&rt).3, 32.0);
}

#[test]
fn cuts_fall_between_graphemes() {
    const MIXED: &str = "👩‍👩‍👧‍👦 cafe\u{301} שלום עולם 🇯🇵 नमस्ते e\u{301}e\u{301}e\u{301} 👩‍👩‍👧‍👦 done and some more words to wrap 👩‍👩‍👧‍👦";
    let bounds: Vec<usize> = MIXED.grapheme_indices(true).map(|(i, _)| i).chain([MIXED.len()]).collect();
    let mut rt = clamp(MIXED, 2, None, 60.0);
    let mut cuts = 0;
    for w in (60..400).step_by(7) {
        rt.set_viewport((w as f32, 400.0), 2.0);
        rt.frame(1.0);
        let (shown, e, lines, _) = drawn(&rt);
        let Some(e) = e else { continue };
        cuts += 1;
        assert!(lines <= 2, "{w}: {shown}");
        assert!(bounds.contains(&e.head), "{w}: {e:?} splits a grapheme: {shown}");
    }
    assert!(cuts > 10, "most widths cut: {cuts}");
}

#[test]
fn copying_a_clamped_text_copies_all_of_it() {
    let mut rt = clamp(TITLE, 2, None, 200.0);
    let r = node(&rt).target;
    let (x, y) = (r[0] + 20.0, r[1] + 8.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.1);
    assert_eq!(rt.selected_text().as_deref(), Some(TITLE));
}

#[test]
fn spans_and_links_past_the_cut_are_hidden() {
    let at = TITLE.find("overflow").unwrap();
    let marked = (at, at + "overflow".len());
    let coloured = |rt: &Runtime<S>| {
        let want = mark().premul(1.0);
        rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count()
    };
    let rt = clamp(TITLE, 2, Some(marked), 200.0);
    assert!(drawn(&rt).1.is_some_and(|e| e.head < marked.0));
    assert!(!rt.elements().iter().any(|e| e.path.ends_with("#link-0")), "a link past the cut isn't pressable");
    assert_eq!(coloured(&rt), 0);
    let rt = clamp(TITLE, 0, Some(marked), 200.0);
    assert!(rt.elements().iter().any(|e| e.path.ends_with("#link-0")));
    assert!(coloured(&rt) > 0, "shown, the span draws its colour");
}

/// One word with no break opportunity, wider than the box.
const TOKEN: &str = "line_clamp_measures_the_whole_identifier_before_it_decides_where_the_ellipsis_falls";

/// The drawn last line's width and the element's, in points.
fn widths(rt: &Runtime<S>) -> (f32, f32) {
    let n = node(rt);
    (last_line_width(n.text_layout.as_ref().unwrap(), n.text.as_ref().unwrap().max_lines.max(1)) / 2.0, n.target[2] - n.target[0])
}

#[test]
fn a_word_wider_than_the_box_is_cut_to_fit() {
    let rt = clamp(TOKEN, 0, None, 200.0);
    assert_eq!(drawn(&rt).2, 1, "the word shapes as one overflowing line");
    let rt = clamp(TOKEN, 1, None, 200.0);
    let (shown, e, lines, h) = drawn(&rt);
    assert!(e.is_some() && shown.ends_with('…') && shown.len() > 10, "{shown}");
    assert_eq!((lines, h), (1, 16.0));
    let (line, element) = widths(&rt);
    assert!(line <= 200.25 && line > 150.0 && element <= 200.0, "{line} {element}: {shown}");
}

#[test]
fn a_wrapped_word_fills_the_last_line() {
    let content: &'static str = Box::leak(format!("See {TOKEN}").into_boxed_str());
    let rt = clamp(content, 1, None, 200.0);
    let (shown, _, lines, _) = drawn(&rt);
    assert!(shown.starts_with("See line_") && shown.ends_with('…') && lines == 1, "{shown}");
    let (line, element) = widths(&rt);
    assert!(line <= 200.25 && line > 150.0 && element <= 200.0, "{line} {element}: {shown}");

    // In two lines the word moves to the second and is cut there.
    let rt = clamp(content, 2, None, 200.0);
    let (shown, _, lines, _) = drawn(&rt);
    assert!(shown.starts_with("See\nline_") || shown.starts_with("See line_"), "{shown}");
    assert!(lines <= 2 && widths(&rt).0 <= 200.25, "{shown}");

    // With `wrap_anywhere`, an unbroken token wraps across lines 1..n and cuts at the end of line n.
    fn anywhere(_: &(), _: &mut Cx<()>) -> El {
        col().w_full().child(crate::text(TOKEN).key("t").font(style()).wrap_anywhere().max_lines(2))
    }
    let mut rt = Runtime::new((), anywhere, (200.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap();
    let e = n.elided.expect("the token wraps past two lines at 200 pt");
    let l = n.text_layout.as_ref().unwrap();
    assert_eq!(l.len(), 2, "wraps onto two lines before cutting");
    assert!(e.head > 40 && l.full_width() <= 400.5, "both lines fill the 200 pt box: head={}, width={}", e.head, l.full_width());
}

/// A field's caret and selection index its own text, so it takes no cut.
#[test]
fn a_text_field_ignores_the_clamp_and_truncation() {
    fn view(e: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
        col().w_full()
            .child(crate::text_input(e, style()).key("t").w_full().max_lines(1))
            .child(crate::text_input(e, style()).key("u").w_full().truncate_middle())
    }
    let mut rt = Runtime::new(crate::TextEdit::new(TITLE), view, (200.0, 400.0), Env::new(true, 2.0));
    rt.frame(0.0);
    for key in ["t", "u"] {
        let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
        let t = n.text.as_ref().unwrap();
        assert!(n.elided.is_none() && t.max_lines == 0 && !t.truncate, "{key}");
        let r = n.target;
        let (x, y) = (r[2] - 4.0, r[1] + 8.0);
        rt.pointer_move(x, y);
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0);
        rt.key(KeyEvent { key: Key::Right, mods: Mods::default() });
        rt.frame(1.1);
    }
}

#[test]
fn clamped_elision_cache_reuses_cut_across_commits() {
    let mut rt = clamp(TITLE, 2, Some((9, 25)), 180.0);
    let (_, cut0, lines0, _) = drawn(&rt);
    assert_eq!(lines0, 2);
    assert!(cut0.is_some());
    assert_eq!(rt.text.cached_elision_count(), 1);
    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "repeat commit reuses cached clamped cut");
    assert_eq!(drawn(&rt).1, cut0);
}
