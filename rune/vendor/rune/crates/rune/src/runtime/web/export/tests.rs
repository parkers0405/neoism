use super::*;

fn site(_: &(), cx: &mut Cx<()>) -> El {
    let wide = cx.layout_size("root").is_some_and(|s| s[0] >= 700.0);
    let cards = if wide { crate::row() } else { crate::col() };
    crate::col().key("root").w_full().role("main").fill(crate::color::hex("#0b0d12").unwrap()).padding(24.0).gap(16.0)
        .child(crate::text("Rune").key("title").role("heading").level(1).font(crate::tok::type_::TITLE).color(crate::Color::WHITE))
        .child(crate::rich("Read the <docs> & more.", vec![]).font(crate::tok::type_::BODY)
            .link(crate::Link::new(9..15, "https://example.com/docs?a=1&b=2")))
        .child(cards.key("views").role("region").label("Views").gap(12.0)
            .child(crate::col().key("a").grow(1.0).radius(12.0).border(1.0, crate::color::hex("#2a2f3a").unwrap()).child(crate::text("Plain Rust")))
            .child(crate::col().key("b").grow(1.0).child(crate::text("Tested")))
            .children(wide.then(|| crate::col().key("c").grow(1.0).child(crate::text("Wide only")))))
        .child(crate::row().key("gh").href("https://github.com/example/rune").child(crate::text("GitHub")))
        .child(crate::col().key("idle").opacity(0.0).child(crate::text("Hidden")))
}

fn taken(rt: &mut Runtime<()>, w: f32) -> Vec<ExportNode> {
    rt.set_viewport((w, 600.0), 2.0);
    rt.frame(0.0);
    rt.frame(0.0);
    rt.export_nodes()
}

#[test]
fn exports_landmarks_headings_links_and_text_with_their_css() {
    let mut rt = Runtime::new((), site, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let tags: Vec<&str> = nodes.iter().map(|n| n.tag).collect();
    assert_eq!(tags[..2], ["main", "h1"]);
    assert!(nodes.iter().any(|n| n.tag == "section" && n.attrs.contains("id=\"views\"")), "{tags:?}");
    let link = nodes.iter().find(|n| n.tag == "a").expect("href element");
    assert!(link.attrs.contains("href=\"https://github.com/example/rune\"") && link.attrs.contains("id=\"gh\""));
    let lede = nodes.iter().find(|n| n.inner.contains("docs")).expect("rich text");
    assert_eq!(lede.inner, "Read the <a href=\"https://example.com/docs?a=1&amp;b=2\">&lt;docs&gt;</a> &amp; more.");
    assert!(!nodes.iter().any(|n| n.inner.contains("Hidden")), "an invisible subtree is left out");
    let root = &nodes[0].decls;
    assert!(root.contains(&("flex-direction", "column".into())) && root.contains(&("width", "100%".into())));
    assert!(root.contains(&("padding", "24px".into())) && root.contains(&("row-gap", "16px".into())));
    let card = nodes.iter().find(|n| n.path.ends_with("a") && n.decls.iter().any(|d| d.0 == "border-radius")).expect("card");
    assert!(card.decls.contains(&("border-radius", "12px".into())));
    assert!(card.decls.iter().any(|d| d.0 == "box-shadow" && d.1.starts_with("inset 0 0 0 1px")), "a paint-only border is an inset shadow");
    let title = &nodes[1];
    assert!(title.decls.contains(&("color", "#ffffff".into())) && title.decls.iter().any(|d| d.0 == "font-size"));
}

#[test]
fn folds_widths_into_media_rules_and_hides_what_a_width_lacks() {
    let mut rt = Runtime::new((), site, (400.0, 600.0), Env::default());
    rt.page_mode();
    let narrow = taken(&mut rt, 400.0);
    let wide = taken(&mut rt, 900.0);
    let (body, css) = page(&[(400.0, narrow), (900.0, wide.clone())]);
    assert!(body.starts_with("<main data-rune-static"));
    assert!(css.starts_with(page::BASE_CSS));
    let media = css.split_once("@media (min-width:900px){").expect("a wide block").1;
    assert!(media.contains("flex-direction:row"), "{media}");
    assert!(!body.contains("Wide only"), "the body is the narrowest tree: {body}");
    assert!(wide.iter().any(|n| n.inner == "Wide only"));
    assert!(css.contains(".t>*{display:inline"), "a text element's runs stay inline: {css}");
    assert!(body.contains("<p class=\"t"), "{body}");
    let opened = body.matches('<').count() - body.matches("</").count();
    assert_eq!(opened, body.matches("</").count(), "every element closes: {body}");
}

#[test]
fn a_node_missing_at_a_wider_width_is_hidden_there() {
    let narrow = vec![ExportNode { path: "0".into(), depth: 0, tag: "div", attrs: String::new(), inner: String::new(), decls: vec![], width: 100.0, text: false },
        ExportNode { path: "0.1".into(), depth: 1, tag: "p", attrs: String::new(), inner: "x".into(), decls: vec![("color", "#fff".into())], width: 100.0, text: true }];
    let wide = vec![narrow[0].clone()];
    let (body, css) = page(&[(360.0, narrow), (800.0, wide)]);
    assert_eq!(body, "<div data-rune-static><p class=\"t a0 b0\">x</p></div>");
    assert!(css.ends_with(".a0{color:#fff;display:flex}@media (min-width:800px){.b0{display:none}}"), "{css}");
}

#[test]
fn radial_gradients_scale_with_the_box_and_soft_clips_become_masks() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        let black = crate::Color::BLACK;
        crate::col().key("root").w_full().role("main")
            .child(crate::col().key("vignette").w(200.0).h(100.0).fill(black)
                .radial_gradient([0.5, 0.5], [100.0, 50.0], [(0.58, crate::Color::TRANSPARENT), (1.0, black)]))
            .child(crate::row().key("trace").w(200.0).h(20.0).clip_fade_linear(40.0, 0.0, 0.0, 0.0)
                .child(crate::text("x")))
    }
    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 400.0);
    let decl = |prop: &str| nodes.iter().flat_map(|n| &n.decls).find(|d| d.0 == prop).map(|d| d.1.clone()).unwrap_or_default();
    assert_eq!(decl("background"), "radial-gradient(50% 50% at 50% 50%,#00000000 58%,#000000 100%),#000000");
    assert_eq!(decl("mask-image"), "linear-gradient(to right,transparent 0,#000 40px,#000 100%)");
}

#[test]
fn text_that_wraps_anywhere_breaks_inside_a_long_word() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        crate::row().key("root").w_full().role("main")
            .child(crate::text("crates/rune-motion/tests/spring.rs:124").key("path").wrap_anywhere().shrink())
            .child(crate::text("words that wrap at spaces").key("prose").wrap())
    }
    let mut rt = Runtime::new((), view, (400.0, 600.0), Env::default());
    rt.page_mode();
    let nodes = taken(&mut rt, 120.0);
    let decls = |text: &str| nodes.iter().find(|n| n.inner.starts_with(text)).map(|n| n.decls.clone()).unwrap_or_default();
    assert!(decls("crates/").contains(&("overflow-wrap", "anywhere".into())), "{:?}", decls("crates/"));
    assert!(!decls("words").iter().any(|d| d.0 == "overflow-wrap"));
}

#[test]
fn an_image_source_set_gets_sizes_from_every_width() {
    let img = |w: f32| ExportNode { path: "0.0".into(), depth: 1, tag: "img", attrs: " src=\"a-640.avif\" srcset=\"a-640.avif 640w, a-1280.avif 1280w\"".into(), inner: String::new(), decls: vec![], width: w, text: false };
    let root = ExportNode { path: "0".into(), depth: 0, tag: "div", attrs: String::new(), inner: String::new(), decls: vec![], width: 0.0, text: false };
    let (body, css) = page(&[(360.0, vec![root.clone(), img(328.0)]), (1000.0, vec![root, img(500.0)])]);
    assert!(body.contains(" sizes=\"(min-width:1000px) 50vw,92vw\">"), "{body}");
    // The `height` attribute is the size it was laid out at; at any other width its class's
    // `aspect-ratio` or height decides, so the reset sits under every class (`:where`).
    assert!(css.contains(":where([data-rune-static]) img{height:auto}"), "{css}");
}
