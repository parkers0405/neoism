//! Pressable spans in rich text: only a link's shaped glyph boxes take the pointer; it shows a
//! hand, paints its hover and pressed looks, is a Tab stop pressed by Enter or Space, and is
//! reported to tools with its role.
mod support;

use rune::runtime::ElementInfo;
use rune::{Color, Cursor, Cx, El, Env, Key, KeyEvent, Link, LinkPaint, Mods, Runtime, Span, TextEdit, TextStyle, col, rich, text_input};
use support::{centre, click, find};

const TEXT: &str = "Hi @kit see docs and notes.md ok";
const WRAPS: &str = "go to a very long link that wraps";

struct S { draft: TextEdit, opened: Vec<String> }

fn at(s: &str, part: &str) -> std::ops::Range<usize> {
    let i = s.find(part).unwrap();
    i..i + part.len()
}

fn hover_bg() -> Color { rune::color::hex("#ff0055").unwrap() }
fn pressed_bg() -> Color { rune::color::hex("#00ff55").unwrap() }

fn view(s: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let accent = rune::color::hex("#3080ff").unwrap();
    let spans = ["@kit", "docs", "notes.md"].map(|p| { let r = at(TEXT, p); Span { color: Some(accent), ..Span::new(r.start, r.end) } }).to_vec();
    let paint = LinkPaint::new().bg(hover_bg()).underline(accent);
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(text_input(&s.draft, style).key("composer").w_full().on_edit(|s: &mut S| &mut s.draft))
        .child(rich(TEXT, spans).key("message").font(style).w(400.0)
            .link(Link::new(at(TEXT, "@kit"), "user:kit").label("Kit").hover(paint).pressed(LinkPaint::new().bg(pressed_bg())))
            .link(Link::new(at(TEXT, "docs"), "https://docs").hover(paint))
            .link(Link::new(at(TEXT, "notes.md"), "file:notes.md").role("button").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
        .child(rich(WRAPS, Vec::new()).key("wrapped").font(style).w(90.0)
            .link(Link::new(at(WRAPS, "a very long link"), "long"))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
}

fn key(rt: &mut Runtime<S>, key: Key, shift: bool) {
    rt.key(KeyEvent { key, mods: Mods { shift, ..Mods::default() } });
    rt.frame(1.0 / 60.0);
}

fn painted(rt: &Runtime<S>, c: Color) -> bool {
    let want = c.premul(1.0);
    rt.instances.iter().any(|i| i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3))
}

fn focused_link(rt: &Runtime<S>) -> Option<String> {
    rt.elements().into_iter().find(|e| e.focused && e.path.contains("#link-")).and_then(|e| e.key)
}

#[test]
fn links_hit_by_their_glyphs_press_and_take_keyboard_focus() {
    let mut draft = TextEdit::new("");
    draft.focused = true;
    let mut rt = Runtime::new(S { draft, opened: Vec::new() }, view, (500.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);

    // Tools see each link with its role and name, inside its text.
    let message = find(&rt, "message");
    let (kit, docs, notes) = (find(&rt, "user:kit"), find(&rt, "https://docs"), find(&rt, "file:notes.md"));
    assert_eq!((kit.role, kit.label.as_deref(), kit.text.as_deref()), ("link", Some("Kit"), Some("@kit")));
    assert_eq!((notes.role, notes.label.as_deref()), ("button", Some("notes.md")));
    for l in [&kit, &docs, &notes] {
        assert!(l.focusable && l.rect[2] > 10.0 && l.rect[3] > 10.0, "{l:?}");
        assert!(l.rect[0] >= message.rect[0] && l.rect[0] + l.rect[2] <= message.rect[0] + message.rect[2], "{l:?} in {:?}", message.rect);
    }
    assert!(kit.rect[0] + kit.rect[2] < docs.rect[0] && docs.rect[0] + docs.rect[2] < notes.rect[0], "in text order");
    assert!(!message.focusable, "the links are the stops, not the paragraph");

    // Hover: a hand and the hover paint over the link only.
    assert!(!painted(&rt, hover_bg()));
    let (x, y) = centre(&docs);
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), Cursor::Pointer);
    assert_eq!(find(&rt, "https://docs").hover, 1.0);
    assert!(painted(&rt, hover_bg()), "hover background");
    let between = (kit.rect[0] + kit.rect[2] + docs.rect[0]) / 2.0;
    rt.pointer_move(between, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.cursor(), Cursor::Default, "plain text between links");
    assert_eq!(find(&rt, "https://docs").hover, 0.0);
    assert!(!painted(&rt, hover_bg()));

    // Pressed look while held; a click opens it and leaves the composer focused.
    let (x, y) = centre(&kit);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.frame(1.0 / 60.0);
    assert!(painted(&rt, pressed_bg()) && find(&rt, "user:kit").press == 1.0, "pressed look");
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["user:kit"]);
    assert!(rt.state.draft.focused, "clicking a link keeps the composer's focus");

    // Plain text takes no press; a press dragged to another link opens nothing.
    click(&mut rt, (between, y));
    let (dx, dy) = centre(&docs);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_move(dx, dy);
    rt.pointer_up(dx, dy);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["user:kit"]);

    // A wrapped link takes its second line but not the plain words beside its first.
    let long = find(&rt, "long");
    assert!(long.rect[3] > 30.0, "two lines: {long:?}");
    click(&mut rt, (long.rect[0] + 3.0, long.rect[1] + 3.0));
    assert_eq!(rt.state.opened, ["user:kit"], "\"go to\" sits inside the link's bounding box but is not the link");
    click(&mut rt, (long.rect[0] + 3.0, long.rect[1] + long.rect[3] - 3.0));
    assert_eq!(rt.state.opened, ["user:kit", "long"]);

    // Keyboard: each link is a Tab stop after the composer; Enter and Space press it;
    // Shift+Tab walks back. (The click on plain text blurred the composer.)
    key(&mut rt, Key::Tab, false);
    assert!(rt.state.draft.focused);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("user:kit"));
    assert!(find(&rt, "user:kit").focus_visible);
    assert!(!rt.state.draft.focused);
    key(&mut rt, Key::Tab, false);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("file:notes.md"));
    key(&mut rt, Key::Enter, false);
    key(&mut rt, Key::Tab, true);
    assert_eq!(focused_link(&rt).as_deref(), Some("https://docs"));
    key(&mut rt, Key::Text(" ".into()), false);
    assert_eq!(rt.state.opened, ["user:kit", "long", "file:notes.md", "https://docs"]);
    assert!(painted(&rt, hover_bg()), "the focused link shows its hover look");
    key(&mut rt, Key::Tab, false);
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt).as_deref(), Some("long"), "on to the next text's link");
    key(&mut rt, Key::Tab, false);
    assert_eq!(focused_link(&rt), None);
    assert!(rt.state.draft.focused, "and round to the composer");
}

const EMOJI: &str = "😀😀 @kit 🎉 docs";
const CENTRED: &str = "go to a very long link that wraps";

fn link_ink() -> Color { rune::color::hex("#ff8800").unwrap() }

fn emoji_view(_: &S, _: &mut Cx<S>) -> El {
    let style = TextStyle::new(14.0, 400.0).lh(20.0);
    let paint = LinkPaint::new().color(link_ink());
    col().w_full().h_full().padding(10.0).gap(10.0)
        .child(rich(EMOJI, Vec::new()).key("emoji").font(style).w(300.0)
            .link(Link::new(at(EMOJI, "@kit"), "user:kit").hover(paint))
            .link(Link::new(at(EMOJI, "🎉 docs"), "party").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
        .child(rich(CENTRED, Vec::new()).key("centred").font(style).w(120.0).align(rune::Align::Center)
            .link(Link::new(at(CENTRED, "a very long link"), "long").hover(paint))
            .on_link(|s: &mut S, id, _| s.opened.push(id.to_owned())))
}

/// Device-px rects `[x, y, w, h]` of glyphs drawn in `c`, and of colour emoji images.
fn drawn(rt: &Runtime<S>, c: Color) -> (Vec<[f32; 4]>, Vec<[f32; 4]>) {
    let want = c.premul(1.0);
    let glyphs = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_GLYPH
        && i.fill.iter().zip(want).all(|(a, b)| (a - b).abs() < 1e-3)).map(|i| i.rect).collect();
    let images = rt.instances.iter().filter(|i| i.params[1] == rune::render::KIND_IMAGE).map(|i| i.rect).collect();
    (glyphs, images)
}

fn mid(r: [f32; 4], s: f32) -> (f32, f32) { ((r[0] + r[2] / 2.0) / s, (r[1] + r[3] / 2.0) / s) }

fn inside((x, y): (f32, f32), e: &ElementInfo) -> bool {
    x >= e.rect[0] && x <= e.rect[0] + e.rect[2] && y >= e.rect[1] && y <= e.rect[1] + e.rect[3]
}

fn hovers(rt: &mut Runtime<S>, (x, y): (f32, f32)) -> bool {
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
    rt.cursor() == Cursor::Pointer
}

#[test]
fn link_boxes_match_drawn_glyphs_beside_emoji_and_in_centred_lines() {
    let mut rt = Runtime::new(S { draft: TextEdit::new(""), opened: Vec::new() }, emoji_view, (500.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    let s = 2.0;

    // Each link's recoloured glyphs, drawn while hovered, sit inside its hit box, and each
    // glyph's centre takes the pointer.
    for (key, glyph_count) in [("user:kit", 4), ("party", 4), ("long", 13)] {
        let link = find(&rt, key);
        // Where a script aims: the centre of the link's first line box. The centre of a wrapped
        // link's union can sit on the edge between its lines, which hits neither.
        let aim = rt.find(key).unwrap();
        assert!(hovers(&mut rt, aim), "{key} centre");
        let (glyphs, _) = drawn(&rt, link_ink());
        assert_eq!(glyphs.len(), glyph_count, "{key}: {glyphs:?}");
        for g in glyphs {
            assert!(inside(mid(g, s), &link), "{key} glyph {g:?} outside {:?}", link.rect);
            assert!(hovers(&mut rt, mid(g, s)), "{key} glyph {g:?} misses");
        }
    }

    // Emoji: the two before @kit take no press, the one inside "🎉 docs" opens it.
    let (_, images) = drawn(&rt, link_ink());
    if images.is_empty() {
        eprintln!("skipping emoji hit checks: no colour emoji font installed");
        return;
    }
    assert_eq!(images.len(), 3, "{images:?}");
    let (kit, party) = (find(&rt, "user:kit"), find(&rt, "party"));
    let mut images = images;
    images.sort_by(|a, b| a[0].total_cmp(&b[0]));
    for e in &images[..2] {
        assert!(!inside(mid(*e, s), &kit) && !hovers(&mut rt, mid(*e, s)), "leading emoji {e:?}");
    }
    assert!(kit.rect[0] * s > images[1][0] + images[1][2] - 1.0, "@kit starts after the emoji");
    assert!(inside(mid(images[2], s), &party) && hovers(&mut rt, mid(images[2], s)), "🎉 is part of its link");
    rt.pointer_down(mid(images[2], s).0, mid(images[2], s).1);
    rt.pointer_up(mid(images[2], s).0, mid(images[2], s).1);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state.opened, ["party"]);
}
