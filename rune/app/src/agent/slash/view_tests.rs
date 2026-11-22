//! Exercise the actual inline card after first paint, not Rune's instant initial mount.
use super::*;
use super::super::picker::Row;
use rune::{Cx, Env, Runtime};

const DT: f64 = 1.0 / 120.0;

fn picker(kind: Kind) -> Picker {
    let mut rows = vec![Row::header("Providers").icon(Some(Icon::Glyph("+")))];
    rows.extend((0..30).map(|i| Row::new(format!("Alpha model {i}"), "Provider description", "26 models", format!("model-{i}"))
        .icon(Some(Icon::Glyph("+"))).status(Some(Status::Good)).current(i == 0)));
    Picker::new(kind, if kind == Kind::Model { "Select model" } else { "Connect provider" }, rows)
}

fn settle(rt: &mut Runtime<App>) { for _ in 0..120 { rt.frame(DT); } }

fn y(rt: &Runtime<App>, key: &str) -> f32 {
    rt.elements().iter().find(|e| e.key.as_deref() == Some(key) && !e.exiting).unwrap().rect[1]
}

fn fully_drawn(rt: &Runtime<App>) {
    let (mut titles, mut descriptions, mut footers, mut icons) = (0, 0, 0, 0);
    for e in rt.elements() {
        assert!(!e.exiting, "old query must disappear immediately: {e:?}");
        if e.visible[2] <= 0.0 || e.visible[3] <= 0.0 { continue; }
        assert!((e.opacity - 1.0).abs() < 0.001, "picker descendants must not fade: {e:?}");
        assert!(!e.revealing, "picker text must not morph/reveal: {e:?}");
        match e.text.as_deref() {
            Some(t) if t.starts_with("Alpha model") => titles += 1,
            Some("Provider description") => descriptions += 1,
            Some("26 models") => footers += 1,
            _ => {},
        }
        if e.key.as_deref().is_some_and(|k| k.starts_with("picker-row-icon:")) { icons += 1; }
    }
    assert!(titles >= 3 && descriptions >= 3 && footers >= 3 && icons >= 3,
        "must inspect nonempty rows with descriptions, nested glyph icons and footers");
}

fn no_query_morph(rt: &Runtime<App>) {
    for e in rt.elements() {
        if e.visible[2] <= 0.0 || e.visible[3] <= 0.0 { continue; }
        if e.text.is_some() || e.key.as_deref().is_some_and(|k| k.starts_with("picker-row-icon:")) {
            for (actual, target) in e.rect.iter().zip(e.target) {
                assert!((actual - target).abs() < 0.03, "query content must land, not morph: {e:?}");
            }
        }
    }
}

fn exercise(kind: Kind) {
    let mut app = App::new();
    app.ty.size = 14.0;
    let tab = app.active_tab().id.clone();
    app.rects.insert(format!("composer-card:{tab}"), [100.0, 600.0, 700.0, 60.0]);
    app.rects.insert(format!("agent-area:{tab}"), [55.0, 60.0, 845.0, 600.0]);
    let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| {
        let root = el().w_full().h_full();
        if a.composer.picker.is_some() { root.child(picker_card(a)) } else { root }
    }, (900.0, 674.0), Env::new(true, 1.0));
    settle(&mut rt);
    for _ in 0..2 {
        rt.update(move |a, _| a.composer.picker = Some(picker(kind)));
        // The card itself grows in when it first appears; everything after that is instant.
        settle(&mut rt);
        fully_drawn(&rt);
        no_query_morph(&rt);
        for query in ["a", "al", "alpha", "model 1", ""] {
            settle(&mut rt);
            let before = y(&rt, "picker-highlight");
            rt.update(|a, _| a.composer.picker.as_mut().unwrap().selected += 1);
            rt.frame(DT);
            let first = y(&rt, "picker-highlight");
            settle(&mut rt);
            let after = y(&rt, "picker-highlight");
            assert!(after > before + ROW_H - 1.0);
            assert!(first < after - 1.0, "cursor must spring rather than teleport");

            let before = y(&rt, "picker-row:3");
            rt.update(|a, _| a.composer.picker.as_mut().unwrap().scroll(2));
            rt.frame(DT);
            let first = y(&rt, "picker-row:3");
            fully_drawn(&rt);
            settle(&mut rt);
            let after = y(&rt, "picker-row:3");
            assert!(after < before - ROW_H);
            assert!(first > after + 1.0, "list offset must still spring");

            // Retarget while moving, then replace the query-keyed content. Neither spring
            // may leak into the new result tree, and no descendant may fade on mount.
            rt.update(|a, _| a.composer.picker.as_mut().unwrap().scroll(1));
            rt.frame(DT);
            rt.update(move |a, _| a.composer.picker.as_mut().unwrap().set_query(query));
            rt.frame(DT);
            fully_drawn(&rt);
            no_query_morph(&rt);
            let first = y(&rt, "picker-row:0");
            let highlight = y(&rt, "picker-highlight");
            for _ in 0..24 {
                rt.frame(DT);
                fully_drawn(&rt);
                no_query_morph(&rt);
                assert!((y(&rt, "picker-row:0") - first).abs() < 0.01, "query resets list spring");
                assert!((y(&rt, "picker-highlight") - highlight).abs() < 0.01, "query resets cursor spring");
            }
        }
        rt.update(|a, _| a.composer.picker = None);
        settle(&mut rt);
        assert!(!rt.elements().iter().any(|e| e.exiting), "the card has finished shrinking away");
    }
}

// The model picker is its own card now (`model_card`), without the generic list's springs.
#[test]
fn connect_query_mounts_are_instant_and_navigation_springs() { exercise(Kind::Connect); }

#[test]
fn model_card_scrolls_small_trackpad_and_touch_moves_inline_and_in_modal() {
    use rune::{GesturePhase, TouchPhase};
    for modal in [false, true] {
        let mut app = App::new();
        app.window = [900.0, 674.0];
        app.ty.size = 14.0;
        let tab = app.active_tab().id.clone();
        app.rects.insert(format!("composer-card:{tab}"), [100.0, 600.0, 700.0, 60.0]);
        app.composer.card_modal = modal;
        app.composer.picker = Some(picker(Kind::Model));
        let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| {
            el().w_full().h_full().child(picker_card(a))
        }, (900.0, 674.0), Env::new(true, 1.0));
        settle(&mut rt);
        let (x, py) = rt.find("picker-row:3").unwrap();
        let initial = y(&rt, "picker-row:3");
        rt.wheel_event(WheelEvent::new(0.0, -12.0).at(x, py).precise(true).phase(GesturePhase::Began));
        rt.frame(DT);
        let after_wheel = y(&rt, "picker-row:3");
        assert!(initial - after_wheel > 1.0 && initial - after_wheel < MODEL_H);
        rt.wheel_event(WheelEvent::new(0.0, 0.0).at(x, py).precise(true).phase(GesturePhase::Ended));
        rt.touch(1, TouchPhase::Started, x, py, 1.0);
        rt.frame(DT);
        rt.touch(1, TouchPhase::Moved, x, py - 18.0, 1.05);
        rt.frame(DT);
        assert!(after_wheel - y(&rt, "picker-row:3") > 10.0);
        rt.touch(1, TouchPhase::Ended, x, py - 18.0, 1.06);
        settle(&mut rt);
        assert!(rt.state.composer.picker.is_some(), "a swipe must not accept a model");
    }
}
#[test]
fn a_finger_drag_scrolls_the_menu_and_a_tap_still_reaches_a_row() {
    use rune::TouchPhase;
    let mut app = App::new();
    app.ty.size = 14.0;
    let tab = app.active_tab().id.clone();
    app.rects.insert(format!("composer-card:{tab}"), [100.0, 600.0, 700.0, 60.0]);
    app.rects.insert(format!("agent-area:{tab}"), [55.0, 60.0, 845.0, 600.0]);
    let mut rt = Runtime::new(app, |a: &App, _: &mut Cx<App>| {
        let root = el().w_full().h_full();
        if a.composer.picker.is_some() { root.child(picker_card(a)) } else { root }
    }, (900.0, 674.0), Env::new(true, 1.0));
    settle(&mut rt);
    rt.update(|a, _| a.composer.picker = Some(picker(Kind::Connect)));
    settle(&mut rt);
    assert_eq!(rt.state.composer.picker.as_ref().unwrap().top, 0);
    // Drag a finger up four rows' worth over the list: the window moves down the list.
    let (x, y) = rt.find("picker-row:3").unwrap();
    rt.touch(1, TouchPhase::Started, x, y, 0.0);
    rt.frame(DT);
    for step in 1..=8 {
        rt.touch(1, TouchPhase::Moved, x, y - step as f32 * ROW_H * 0.5, step as f64 * 0.02);
        rt.frame(DT);
    }
    rt.touch(1, TouchPhase::Ended, x, y - 4.0 * ROW_H, 0.2);
    settle(&mut rt);
    assert!(rt.state.composer.picker.as_ref().unwrap().top >= 3, "the drag scrolled the menu: top {}", rt.state.composer.picker.as_ref().unwrap().top);
    // A mouse press on a row is still that row's, not the card's drag.
    let top = rt.state.composer.picker.as_ref().unwrap().top;
    let (x, y) = rt.find(&format!("picker-row:{}", top + 1)).unwrap();
    rt.pointer_move(x, y);
    rt.frame(DT);
    assert_eq!(rt.state.composer.picker.as_ref().unwrap().selected, top + 1, "moving over a row selects it");
}
