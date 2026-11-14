//! `El::truncate_middle` and `El::truncate_head`: the ellipsis replaces the middle or the start,
//! cuts fall between graphemes, a resize refits the cut, and copying and tools keep the whole
//! string.
use super::*;
use crate::el::Elide;
use crate::{Color, Cx, El, Env, Key, KeyEvent, Link, Mods, Span, TextStyle, col, rich};
use unicode_segmentation::UnicodeSegmentation;

const PATH: &str = "~/Projects/rune/crates/rune-kit/src/components/settings/appearance_panel.rs";

fn style() -> TextStyle { TextStyle::new(13.0, 400.0).lh(16.0) }

/// Content, how it's cut, and a byte range that is both an inline-code span and a link.
type S = (&'static str, Elide, Option<(usize, usize)>);

fn pill() -> Color { crate::color::hex("#e6339a").unwrap() }

/// `content` in a `width` pt window, cut in the middle (or at its head).
fn runtime(content: &'static str, head: bool, width: f32) -> Runtime<S> {
    cut_at(content, if head { Elide::Head } else { Elide::Middle }, None, width)
}

fn cut_at(content: &'static str, elide: Elide, marked: Option<(usize, usize)>, width: f32) -> Runtime<S> {
    fn view(s: &S, _: &mut Cx<S>) -> El {
        let spans = s.2.map(|(a, b)| Span { mono: true, bg: Some(pill()), ..Span::new(a, b) }).into_iter().collect();
        let mut t = rich(s.0, spans).key("t").font(style()).selectable(true);
        if let Some((a, b)) = s.2 { t = t.link(Link::new(a..b, "marked")).on_link(|_: &mut S, _, _| {}); }
        col().w_full().child(match s.1 { Elide::End => t.truncate(), Elide::Middle => t.truncate_middle(), Elide::Head => t.truncate_head() })
    }
    let mut rt = Runtime::new((content, elide, marked), view, (width, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn node<'a>(rt: &'a Runtime<S>) -> &'a Node {
    rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap()
}

/// The string node "t" draws, and its cut.
fn drawn(rt: &Runtime<S>) -> (String, Option<Elision>) {
    let n = node(rt);
    let content = &n.text.as_ref().unwrap().content;
    match n.elided {
        Some(e) => ([&content[..e.head], "…", &content[e.tail..]].concat(), Some(e)),
        None => (content.clone(), None),
    }
}

fn fits(rt: &Runtime<S>) -> bool {
    let n = node(rt);
    let w = (n.target[2] - n.target[0]) * 2.0;
    n.text_layout.as_ref().unwrap().full_width() <= w + 0.5
}

#[test]
fn middle_truncation_keeps_both_ends_and_refits_on_resize() {
    let mut rt = runtime(PATH, false, 200.0);
    let (shown, e) = drawn(&rt);
    let e = e.expect("the path overflows 200 pt");
    assert!(fits(&rt), "{shown}");
    assert!(shown.starts_with("~/Projects") && shown.ends_with("panel.rs") && shown.contains('…'), "{shown}");
    let (head, tail) = (PATH[..e.head].graphemes(true).count(), PATH[e.tail..].graphemes(true).count());
    assert!(head.abs_diff(tail) <= 1, "both ends keep half: {shown}");
    let el = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("t")).unwrap();
    assert_eq!(el.text.as_deref(), Some(PATH), "tools and accessibility read the whole path");

    rt.set_viewport((320.0, 40.0), 2.0);
    rt.frame(0.1);
    let (wider, _) = drawn(&rt);
    assert!(fits(&rt) && wider.len() > shown.len(), "a wider box keeps more: {wider}");

    rt.set_viewport((900.0, 40.0), 2.0);
    rt.frame(0.2);
    assert_eq!(drawn(&rt), (PATH.to_owned(), None), "room for it all: no cut");

    rt.set_viewport((200.0, 40.0), 2.0);
    rt.frame(0.3);
    assert_eq!(drawn(&rt).0, shown, "narrowing again cuts as before");
}

#[test]
fn head_truncation_keeps_the_end() {
    let rt = runtime(PATH, true, 160.0);
    let (shown, e) = drawn(&rt);
    assert_eq!(e.map(|e| e.head), Some(0));
    assert!(fits(&rt) && shown.starts_with('…') && shown.ends_with("appearance_panel.rs"), "{shown}");
}

#[test]
fn cuts_fall_between_graphemes() {
    const MIXED: &str = "👩‍👩‍👧‍👦 cafe\u{301} שלום עולם 🇯🇵 नमस्ते e\u{301}e\u{301}e\u{301} 👩‍👩‍👧‍👦 done";
    let mut rt = runtime(MIXED, false, 60.0);
    let bounds: Vec<usize> = MIXED.grapheme_indices(true).map(|(i, _)| i).chain([MIXED.len()]).collect();
    let mut cuts = 0;
    for w in (60..400).step_by(7) {
        rt.set_viewport((w as f32, 40.0), 2.0);
        rt.frame(1.0);
        let (shown, e) = drawn(&rt);
        let Some(e) = e else { continue };
        cuts += 1;
        assert!(fits(&rt), "{w}: {shown}");
        assert!(bounds.contains(&e.head) && bounds.contains(&e.tail), "{w}: {e:?} splits a grapheme: {shown}");
    }
    assert!(cuts > 10, "most widths cut: {cuts}");
}

#[test]
fn copying_a_cut_text_copies_all_of_it() {
    let mut rt = runtime(PATH, false, 200.0);
    let r = node(&rt).target;
    let (y, left, right) = ((r[1] + r[3]) / 2.0, r[0] + 0.5, r[2] - 0.5);

    // A drag across the ellipsis selects what it hides too.
    rt.pointer_move(left, y);
    rt.pointer_down(left, y);
    rt.frame(1.0 / 60.0);
    rt.pointer_move(right, y);
    rt.frame(2.0 / 60.0);
    rt.pointer_up(right, y);
    rt.frame(3.0 / 60.0);
    assert_eq!(rt.selected_text().as_deref(), Some(PATH));

    // So does Select All.
    rt.pointer_move(left + 20.0, y);
    rt.pointer_down(left + 20.0, y);
    rt.pointer_up(left + 20.0, y);
    rt.frame(1.0);
    rt.key(KeyEvent { key: Key::Command('a'), mods: Mods { cmd: true, ..Mods::default() } });
    rt.frame(1.1);
    assert_eq!(rt.selected_text().as_deref(), Some(PATH));
}

/// Instances filled with the code span's pill colour.
fn pills(rt: &Runtime<S>) -> usize {
    let want = pill().premul(1.0);
    rt.instances.iter().filter(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).count()
}

#[test]
fn a_cut_hides_the_spans_and_links_inside_it() {
    // A link that is also an inline-code span, at the end, in the middle and at the start.
    let cases = [
        ("See the docs", Elide::End, (8, 12)),
        ("Open the config file in your editor today", Elide::Middle, (9, 15)),
        ("docs are here for you to read", Elide::Head, (0, 4)),
    ];
    for (content, elide, (a, b)) in cases {
        let mut rt = cut_at(content, elide, Some((a, b)), 20.0);
        let mut hidden = 0;
        for w in (20..300).step_by(3) {
            rt.set_viewport((w as f32, 40.0), 2.0);
            rt.frame(1.0);
            let (shown, e) = drawn(&rt);
            let gone = e.is_some_and(|e| a >= e.head && b <= e.tail);
            hidden += usize::from(gone);
            let link = rt.elements().iter().any(|e| e.path.ends_with("#link-0"));
            assert_eq!(link, !gone, "{w}: {shown}: the link is pressable only while it shows");
            assert_eq!(pills(&rt) > 0, !gone, "{w}: {shown}: the code pill draws only while it shows");
        }
        assert!(hidden > 0, "{content}: some width hides the link");
    }
}

#[test]
fn trimming_spaces_keeps_graphemes_whole() {
    // A space that begins a grapheme (before a combining accent) or ends one (after a
    // prepended mark) stays with it rather than being trimmed off beside the ellipsis.
    const SPACED: &str = "ab\u{600} cd \u{301}ef gh \u{301}ij";
    let bounds: Vec<usize> = SPACED.grapheme_indices(true).map(|(i, _)| i).chain([SPACED.len()]).collect();
    for elide in [Elide::End, Elide::Middle, Elide::Head] {
        let mut rt = cut_at(SPACED, elide, None, 10.0);
        for w in 10..140 {
            rt.set_viewport((w as f32, 40.0), 2.0);
            rt.frame(1.0);
            let (shown, e) = drawn(&rt);
            let Some(e) = e else { continue };
            assert!(bounds.contains(&e.head) && bounds.contains(&e.tail), "{elide:?} {w}: {shown:?}");
        }
    }
}

#[test]
fn elision_maps_ranges_across_the_cut() {
    // "abcdefgh" drawn "ab…gh".
    let e = Elision { head: 2, tail: 6 };
    assert_eq!(e.content(0), 0);
    assert_eq!(e.content(2), 2, "before the ellipsis");
    assert_eq!(e.content(5), 6, "after it: where the hidden middle ends");
    assert_eq!(e.content(7), 8);

    // Selections reaching into the cut highlight the ellipsis.
    assert_eq!(e.range(1, 4), (1, 5));
    assert_eq!(e.range(3, 8), (2, 7));
    assert_eq!(e.range(6, 8), (5, 7));

    // Links: hidden ones are empty, edges clamp, and only one showing on both sides takes "…".
    let empty = |(a, b): (usize, usize)| a == b;
    assert!(empty(e.mark(3, 5, true)) && empty(e.mark(2, 6, true)));
    assert_eq!(e.mark(1, 4, true), (1, 2));
    assert_eq!(e.mark(4, 8, true), (5, 7));
    assert_eq!(e.mark(1, 7, true), (1, 6));
    assert_eq!((e.mark(0, 2, true), e.mark(6, 8, true)), ((0, 2), (5, 7)));

    // Spans: also hidden when wholly cut, but "…" takes the style of the text it replaces
    // when that span shows beside it.
    assert!(empty(e.mark(3, 5, false)) && empty(e.mark(2, 6, false)));
    assert_eq!(e.mark(1, 4, false), (1, 5));
    assert_eq!(e.mark(2, 8, false), (2, 7));
    assert_eq!(e.mark(4, 8, false), (5, 7), "starting after the cut's first byte: not the ellipsis");
    assert_eq!(e.mark(1, 7, false), (1, 6));
}

#[test]
fn elision_cache_reuses_cut_evicts_superseded_and_bypasses_secure() {
    fn view(s: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(crate::text(s.as_str()).key("t").font(style()).truncate_middle())
    }
    let mut rt = Runtime::new(PATH.to_owned(), view, (140.0, 40.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let cut0 = rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided;
    assert!(cut0.is_some());
    assert_eq!(rt.text.cached_elision_count(), 1);
    let shaped_before = rt.text.shaped;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.shaped, shaped_before, "repeat commit reuses cached truncated cut");
    assert_eq!(rt.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided, cut0);

    // Editing the string supersedes the old shaping and drops its cached cut on end_commit.
    rt.update(|s, _| s.push_str("/extra_leaf_module.rs"));
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.text.cached_elision_count(), 1, "superseded cut is replaced rather than accumulating");

    // Redacted (`secure`) text never enters the elision cache.
    fn secure_view(s: &String, _: &mut Cx<String>) -> El {
        col().w_full().child(crate::text(s.as_str()).key("t").font(style()).secure().revealed(true).truncate())
    }
    let mut sec = Runtime::new(PATH.to_owned(), secure_view, (140.0, 40.0), Env::new(true, 2.0));
    sec.frame(0.0);
    assert!(sec.nodes.iter().find(|n| n.key.as_deref() == Some("t")).unwrap().elided.is_some());
    assert_eq!(sec.text.cached_elision_count(), 0, "redacted text bypasses elision cache");
}
