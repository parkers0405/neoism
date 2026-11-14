use super::*;
use serde_json::json;

#[test]
fn mirrors_headings_inline_links_and_buttons_and_skips_idle_frames() {
    fn view(open: &(bool, usize), _: &mut Cx<(bool, usize)>) -> El {
        let mut body = crate::rich("Read spec for details.", vec![]).font(crate::tok::type_::BODY)
            .link(crate::Link::new(5..9, "https://example.com/spec"))
            .on_link(|s: &mut (bool, usize), _, _| s.1 += 1);
        body = body.selectable(true);
        let toggle = crate::row().key("fold").role("button").label(if open.0 { "Hide details" } else { "Show details" })
            .expanded(open.0)
            .on_press(|s: &mut (bool, usize)| s.0 = !s.0)
            .child(crate::text("Details"));
        crate::col().w_full().gap(8.0)
            .child(crate::text("Release notes").role("heading").level(1).font(crate::tok::type_::TITLE))
            .child(body)
            .child(toggle)
    }
    let mut rt = Runtime::new((false, 0usize), view, (480.0, 320.0), Env::default());
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    let first = rt.web_a11y(&mut mirror).expect("initial tree").to_owned();
    assert!(rt.web_a11y(&mut mirror).is_none(), "unchanged frame emits nothing");

    let items = rt.web_a11y_items();
    assert_eq!(items.len(), 3);
    assert_eq!((items[0].kind, items[0].level, items[0].text.as_str()), ("heading", Some(1), "Release notes"));
    assert_eq!(items[1].kind, "text");
    assert_eq!(items[1].runs.len(), 3);
    assert_eq!((items[1].runs[1].text.as_str(), items[1].runs[1].href.as_deref()), ("spec", Some("https://example.com/spec")));
    assert_eq!((items[2].kind, items[2].text.as_str(), items[2].expanded), ("button", "Show details", Some(false)));
    assert!(items[1].font.is_none(), "outside a page the canvas selects text itself");

    let btn_path = items[2].path.clone();
    let link_path = items[1].path.clone();
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": btn_path})));
    rt.frame(0.0);
    assert!(rt.state.0);
    let updated = rt.web_a11y(&mut mirror).expect("button flip updates expanded state");
    assert_ne!(first, updated);
    assert!(updated.contains("Hide details"));

    assert!(rt.web_a11y_action(&json!({"kind": "a11y_link", "path": link_path, "link": 0})));
    rt.frame(0.0);
    assert_eq!(rt.state.1, 1);
}

fn page(_: &(), _: &mut Cx<()>) -> El {
    crate::col().w_full().role("main")
        .child(crate::col().key("intro").role("region").label("Intro").w_full().h(400.0)
            .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE))
            .child(crate::text("A Rust UI framework.").key("lede").font(crate::tok::type_::BODY).selectable(true).padding(4.0)))
        .child(crate::col().key("links").role("navigation").w_full()
            .child(crate::row().key("gh").href("https://github.com/example/rune").child(crate::text("GitHub")))
            .child(crate::col().key("still").role("img").label("Liquid metal").w(200.0).h(100.0)))
}

#[test]
fn a_page_mirrors_landmarks_ids_links_and_images_in_document_coordinates() {
    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    rt.set_page_scroll(120.0);
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let kinds: Vec<_> = items.iter().map(|i| (i.kind, i.tag, i.parent)).collect();
    assert_eq!(kinds, [
        ("group", Some("main"), None),
        ("group", Some("section"), Some(0)),
        ("heading", None, Some(1)),
        ("text", None, Some(1)),
        ("group", Some("nav"), Some(0)),
        ("link", None, Some(4)),
        ("image", None, Some(4)),
    ]);
    assert_eq!((items[1].id.as_deref(), items[1].text.as_str()), (Some("intro"), "Intro"));
    assert_eq!(items[2].id.as_deref(), Some("title"), "a heading is an anchor");
    assert_eq!(items[3].id, None);
    assert_eq!((items[5].href.as_deref(), items[5].text.as_str()), (Some("https://github.com/example/rune"), "GitHub"));
    assert_eq!(items[6].text, "Liquid metal");
    assert_eq!(items[4].rect[1], 400.0, "rects are in the document, whatever the scroll");

    let font = items[3].font.as_ref().expect("text in a page carries its font");
    assert_eq!((font.family.as_str(), font.align), ("sans", "left"));
    assert!(items[3].selectable);
    assert!(!items[2].selectable);
    assert_eq!(items[3].rect[0], 4.0, "selectable text sits at its text box, inside the padding");
}

#[test]
fn a_page_mirror_does_not_change_while_the_page_scrolls() {
    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());
    rt.set_page_scroll(200.0);
    rt.frame(0.0);
    assert!(rt.web_a11y(&mut mirror).is_none(), "scrolling the page leaves the mirror alone");
}

/// A page whose only change between frames is a colour, at a document offset that sits on a
/// rounding boundary (100.05 points rounds the other way either side of it).
fn tinted(tone: &u32, _: &mut Cx<u32>) -> El {
    crate::col().w_full().role("main")
        .child(crate::col().key("intro").role("region").label("Intro").w_full().h(100.05)
            .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE)))
        .child(crate::text("A Rust UI framework.").key("lede").font(crate::tok::type_::BODY).selectable(true)
            .fill(crate::Color::WHITE.alpha(f32::from(u8::try_from(*tone % 200).expect("byte")) / 400.0)))
}

#[test]
fn fractional_scroll_offsets_never_rewrite_a_page_mirror() {
    let mut rt = Runtime::new(0u32, tinted, (480.0, 300.0), Env::new(true, 3.0));
    rt.page_mode();
    rt.frame(0.0);
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());
    // At `@3x` a scroll offset is a third of a pixel, so the band's offset and the document's
    // position never add back exactly in `f32`. A commit that only redraws the band used to round
    // an item's position the other way and rewrite the whole page's mirror.
    for i in 1..20u32 {
        let y = f32::from(u16::try_from(i).expect("small")) * 137.3333;
        assert!(rt.set_page_scroll(y), "each offset is a new page position");
        rt.state = i;
        rt.frame(0.0);
        assert!(rt.web_a11y(&mut mirror).is_none(), "the mirror changed at {y}");
    }
}

/// A page with a wide code snippet in a horizontal viewport, as a site puts one.
fn strip(_: &(), _: &mut Cx<()>) -> El {
    crate::col().w_full().role("main")
        .child(crate::row().key("scroll").w(200.0).h(40.0).scroll_x()
            .child(crate::text("fn main() { println!(\"hello\"); }").key("code").w(600.0).h(40.0)
                .font(crate::tok::type_::BODY).selectable(true)))
}

#[test]
fn a_page_mirrors_a_horizontal_viewport_as_a_scroller_the_browser_scrolls() {
    let mut rt = Runtime::new((), strip, (400.0, 300.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let scroll = items.iter().find(|i| i.kind == "scroll_x").expect("the strip mirrors as a scroller");
    assert_eq!((scroll.tag, scroll.rect[0], scroll.rect[2]), (Some("div"), 0.0, 200.0));
    let code = items.iter().find(|i| i.text.contains("println")).expect("the code still mirrors");
    let placed = code.rect[0];
    let mut mirror = A11yMirror::default();
    assert!(rt.web_a11y(&mut mirror).is_some());

    let path = scroll.path.clone();
    assert!(rt.set_page_scroll_x(&path, 120.0), "the browser scrolled the strip");
    rt.frame(0.0);
    assert!(rt.web_a11y(&mut mirror).is_none(), "the container's own scroll leaves the mirror alone");
    let after = rt.web_a11y_items();
    let code = after.iter().find(|i| i.text.contains("println")).expect("the code still mirrors");
    assert_eq!(code.rect[0], placed, "the text keeps its place inside the scroller");
    assert!(!rt.set_page_scroll_x(&path, 120.0), "the same offset is no change");
}

/// Code drawn one text per token, as an editor that animates tokens does, beside a gutter
/// number; then words a small gap apart, and a label and value set far apart.
fn code(_: &(), _: &mut Cx<()>) -> El {
    let mono = crate::tok::type_::BODY.mono();
    let line = |n: usize, toks: &[&str]| {
        let mut row = crate::row().key("toks");
        for (k, t) in toks.iter().enumerate() {
            row = row.child(crate::text(*t).key(format!("t{k}")).font(mono).w(t.chars().count() as f32 * 8.0).h(20.0));
        }
        crate::row().key(format!("ln{n}")).h(20.0)
            .child(crate::row().key("num").w(32.0).child(crate::text(n.to_string()).font(mono)))
            .child(row)
    };
    crate::col().w_full()
        .child(line(1, &["type", " ", "Port", " = ", "Int"]))
        .child(line(2, &["listen", "(", "port", ")"]))
        .child(crate::row().key("words").gap(4.0).child(crate::text("Hello")).child(crate::text("world")))
        .child(crate::row().key("pair").w(300.0).justify_between().child(crate::text("Name")).child(crate::text("Kit")))
}

#[test]
fn a_row_of_token_texts_mirrors_as_one_line() {
    let mut rt = Runtime::new((), code, (480.0, 300.0), Env::default());
    rt.frame(0.0);
    let items = rt.web_a11y_items();
    let texts: Vec<_> = items.iter().map(|i| (i.kind, i.text.as_str(), i.runs.len())).collect();
    assert_eq!(texts, [
        ("text", "1", 0),
        ("text", "type Port = Int", 0),
        ("text", "2", 0),
        ("text", "listen(port)", 0),
        ("text", "Hello world", 0),
        ("text", "Name", 0),
        ("text", "Kit", 0),
    ]);
    let line = &items[1];
    assert_eq!((line.rect[0], line.rect[2]), (32.0, 15.0 * 8.0), "the line's box spans its tokens");
}

#[test]
fn href_presses_open_absolute_urls() {    let mut rt = Runtime::new((), page, (480.0, 300.0), Env::default());
    rt.frame(0.0);
    let gh = rt.nodes.iter().find(|n| n.key.as_deref() == Some("gh")).unwrap().path.clone();
    assert_eq!(rt.nodes.iter().find(|n| n.key.as_deref() == Some("gh")).unwrap().role, "link");
    assert!(rt.web_a11y_action(&json!({"kind": "a11y_press", "path": gh})));
    rt.frame(0.0);
    assert_eq!(rt.take_url_requests(), ["https://github.com/example/rune"]);
}

#[test]
fn mac_meta_copy_and_select_all_work_on_selectable_text() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::col().key("doc").selection_group(true)
            .child(crate::text("First paragraph").key("p1").font(crate::tok::type_::BODY).selectable(true))
            .child(crate::text("Second paragraph").key("p2").font(crate::tok::type_::BODY).selectable(true))
    }
    let mut rt = Runtime::new((), view, (400.0, 200.0), Env::default());
    let board = crate::clipboard::memory();
    rt.use_clipboard(&board);
    rt.frame(0.0);
    rt.pointer_down(12.0, 8.0);
    rt.pointer_up(12.0, 8.0);
    rt.frame(0.0);
    // Simulate macOS browser Meta+A (meta=true, ctrl=false) then Meta+C.
    rt.web_key(&json!({"kind":"key", "key":"a", "code":"KeyA", "meta":true, "ctrl":false, "shift":false, "alt":false}));
    rt.frame(0.0);
    assert_eq!(rt.copy_text().as_deref(), Some("First paragraph\nSecond paragraph"));
    rt.web_key(&json!({"kind":"key", "key":"c", "code":"KeyC", "meta":true, "ctrl":false, "shift":false, "alt":false}));
    rt.frame(0.0);
    crate::clipboard::flush_memory(&mut rt);
    assert_eq!(board.text().as_deref().map(String::as_str), Some("First paragraph\nSecond paragraph"));
}
