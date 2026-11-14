use super::*;

#[derive(Default)]
struct Seen {
    log: Vec<bool>,
}

fn page(_: &Seen, _: &mut Cx<Seen>) -> El {
    crate::col().w_full()
        .child(crate::col().key("top").w_full().h(500.0))
        .child(crate::col().key("demo").w_full().h(100.0).on_visible(|s: &mut Seen, on, _| s.log.push(on)))
        .child(crate::col().key("bottom").w_full().h(600.0))
}

#[test]
fn an_element_hears_when_the_page_scrolls_it_on_screen_and_off() {
    let mut rt = Runtime::new(Seen::default(), page, (400.0, 200.0), Env::default());
    rt.page_mode();
    rt.frame(0.0);
    rt.frame(0.0);
    assert!(rt.state.log.is_empty(), "off screen at first, and nothing to say");

    rt.set_page_scroll(330.0);
    rt.frame(0.0);
    rt.frame(0.0);
    assert!(rt.state.log.is_empty(), "30 of its 100 points show: not yet");

    rt.set_page_scroll(360.0);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [true], "60 points show");

    rt.set_page_scroll(450.0);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [true], "still on screen: no second report");

    rt.set_page_scroll(800.0);
    rt.frame(0.0);
    rt.frame(0.0);
    assert_eq!(rt.state.log, [true, false]);
}

#[test]
fn half_of_it_or_half_the_window_counts() {
    let view = (400.0, 200.0);
    assert!(on_screen([0.0, 150.0, 400.0, 100.0], view));
    assert!(!on_screen([0.0, 151.0, 400.0, 100.0], view));
    assert!(on_screen([0.0, -400.0, 400.0, 1000.0], view), "taller than the window, covering it");
    assert!(!on_screen([0.0, 200.0, 400.0, 100.0], view));
}
