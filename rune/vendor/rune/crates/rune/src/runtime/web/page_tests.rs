use super::*;

fn tall(_: &u32, _: &mut Cx<u32>) -> El {
    crate::col().w_full()
        .child(crate::col().key("top").w_full().h(300.0))
        .child(crate::col().key("button").w_full().h(100.0).on_press(|n: &mut u32| *n += 1))
        .child(crate::col().key("bottom").w_full().h(600.0))
}

#[test]
fn a_page_lays_out_at_its_height_and_draws_the_band_it_is_scrolled_to() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    assert_eq!(rt.page_height(), 1000.0, "the root keeps its natural height");
    assert_eq!(rt.viewport, (400.0, 200.0), "the window stays the band's size");
    let button = rt.nodes.iter().position(|n| n.key.as_deref() == Some("button")).unwrap();
    assert_eq!(rt.drawn_box(button)[1], 300.0);

    assert!(rt.set_page_scroll(250.4));
    assert!(!rt.set_page_scroll(250.4), "the same offset is no change");
    rt.frame(0.0);
    assert_eq!(rt.page_offset(), 250.0, "rounded to the device-pixel grid");
    assert_eq!(rt.drawn_box(button)[1], 50.0, "the button draws 250 points higher");

    // A press lands where the button draws in the band, not where it sits in the document.
    rt.pointer_down(100.0, 80.0);
    rt.pointer_up(100.0, 80.0);
    rt.frame(0.0);
    assert_eq!(rt.state, 1);
}

#[test]
fn negative_scroll_rubber_bands_the_page_downward() {
    fn doc(_: &u32, _: &mut Cx<u32>) -> El {
        crate::col().w_full()
            .child(crate::text("Top of the page").key("top").w_full().h(300.0))
            .child(crate::col().key("bottom").w_full().h(600.0))
    }
    let mut rt = Runtime::new(0u32, doc, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    // Pulling down at the top reports a negative `scrollY`; the band follows it, as the browser's
    // own scroll indicator does, instead of stopping dead at the first pixel.
    assert!(rt.set_page_scroll(-60.0));
    rt.frame(0.0);
    assert_eq!(rt.page_offset(), -60.0);
    assert_eq!(rt.drawn_box(1)[1], 60.0, "the page rides down with the bounce");
    let items = rt.web_a11y_items();
    let top = items.iter().find(|i| i.text.contains("Top of the page")).expect("the heading mirrors");
    assert_eq!(top.rect[1], 0.0, "the mirror keeps its document position");
}

#[test]
fn page_scroll_is_ignored_outside_page_mode() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.frame(0.0);
    assert!(!rt.set_page_scroll(100.0));
    assert_eq!(rt.page_offset(), 0.0);
}

#[test]
fn resizing_the_band_does_not_lay_the_page_out_again() {
    let mut rt = Runtime::new(0u32, tall, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    let scale = rt.env.scale;
    rt.set_viewport((400.0, 160.0), scale);
    assert!(!rt.relayout, "a mobile toolbar collapsing only changes the band");
    assert_eq!(rt.viewport, (400.0, 160.0));
    assert_eq!(rt.page_height(), 1000.0);
}

/// Ten 500-point blocks, 5,000 points of document.
fn blocks(_: &u32, _: &mut Cx<u32>) -> El {
    let mut page = crate::col().w_full();
    for i in 0..10 {
        page = page.child(crate::col().key(format!("block-{i}")).w_full().h(500.0));
    }
    page
}

#[test]
fn a_band_only_draws_again_when_the_viewport_nears_its_edge() {
    let mut rt = Runtime::new(0u32, blocks, (400.0, 800.0), Env::default());
    rt.page_mode();
    rt.set_band(1312.0, 800.0);
    rt.frame(0.0);
    assert_eq!(rt.page_offset(), 0.0, "the first band starts at the top");

    // Inside the skirt the browser's compositor scrolls the pixels Rune already drew.
    assert!(!rt.set_page_scroll(120.0));
    assert_eq!(rt.page_offset(), 0.0);
    assert!(!rt.set_page_scroll(400.0), "up to 64 points from the band's foot is still inside");
    assert_eq!(rt.page_offset(), 0.0);

    // Nearer than that, a frame draws a new band centred on the window: 256 points of skirt.
    assert!(rt.set_page_scroll(1000.0));
    assert_eq!(rt.page_offset(), 744.0);
    assert!(!rt.set_page_scroll(1100.0), "and the new band holds the next stretch of scroll");

    // The last band stops at the document's foot instead of showing empty page below it.
    assert!(rt.set_page_scroll(4200.0));
    assert_eq!(rt.page_offset(), 3688.0, "5000 - 1312");
    assert_eq!(rt.page_offset() + 1312.0, rt.page_height());
}

#[test]
fn a_band_no_taller_than_the_window_follows_the_page_every_frame() {
    let mut rt = Runtime::new(0u32, blocks, (400.0, 800.0), Env::default());
    rt.page_mode();
    rt.set_band(800.0, 800.0);
    rt.frame(0.0);
    assert!(rt.set_page_scroll(20.0));
    assert_eq!(rt.page_offset(), 20.0, "no skirt to coast in");
}

#[test]
fn a_mirror_keeps_its_document_position_across_band_anchors() {
    let mut rt = Runtime::new(0u32, blocks, (480.0, 300.0), Env::default());
    rt.page_mode();
    rt.set_band(812.0, 300.0);
    rt.frame(0.0);
    let before: Vec<_> = rt.web_a11y_items().iter().map(|i| (i.path.clone(), i.rect)).collect();
    assert!(rt.set_page_scroll(1200.0), "the band is drawn again");
    rt.frame(0.0);
    let after: Vec<_> = rt.web_a11y_items().iter().map(|i| (i.path.clone(), i.rect)).collect();
    assert_eq!(before, after);
}

#[test]
fn a_band_knows_which_part_of_it_the_window_shows() {
    fn tall_visible(_: &u32, _: &mut Cx<u32>) -> El {
        crate::col().w_full()
            .child(crate::col().key("button").w_full().h(100.0).on_visible(|s: &mut u32, on: bool, _| *s = u32::from(on)))
            .child(crate::col().key("fill").w_full().h(900.0))
    }
    let mut rt = Runtime::new(0u32, tall_visible, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.set_band(712.0, 200.0);
    for _ in 0..3 { rt.frame(0.0); }
    assert_eq!(rt.state, 1, "the button is on screen at the top");

    // Inside the skirt no frame is drawn at all: the window moves within the band, and the band
    // still holds the button where it drew it.
    assert!(!rt.set_page_scroll(380.0));
    for _ in 0..3 { rt.frame(0.0); }
    assert_eq!(rt.page_window(), (380.0, 200.0), "the window is inside the band, not at its top");
    assert_eq!(rt.drawn_box(1)[1], 0.0, "the band still draws it where it was drawn");
    assert_eq!(rt.state, 0, "but the window has moved past it");
}
