//! `El::inert`: a subtree that draws and lays out while the pointer and keyboard pass through.
mod support;

use rune::tok::TextStyle;
use rune::{Color, Cursor, Cx, El, Env, Key, KeyEvent, Mods, Runtime, TextEdit, el, text_input};
use support::{click, find as info};

#[derive(Default)]
struct S {
    inert: bool,
    log: Vec<&'static str>,
    hover: Vec<(&'static str, bool)>,
    rects: Vec<[f32; 4]>,
    field: TextEdit,
}

/// A canvas that takes presses and shows a crosshair, with a document drawn over it whose
/// own elements would take presses, hovers and focus.
fn view(s: &S, _: &mut Cx<S>) -> El {
    let doc = el().key("doc").absolute(0.0, 0.0).size(100.0, 100.0).fill(Color::WHITE)
        .on_layout(|s: &mut S, r| s.rects.push(r))
        .child(el().key("button").size(40.0, 40.0).fill(Color::BLACK).cursor(Cursor::Pointer)
            .focusable(true)
            .on_press(|s: &mut S| s.log.push("button"))
            .on_hover_change(|s: &mut S, inside, _| s.hover.push(("button", inside))))
        .child(text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").w(60.0)
            .on_edit(|s: &mut S| &mut s.field));
    el().key("canvas").size(200.0, 200.0).cursor(Cursor::Crosshair)
        .on_press(|s: &mut S| s.log.push("canvas"))
        .on_hover_change(|s: &mut S, inside, _| s.hover.push(("canvas", inside)))
        .child(if s.inert { doc.inert() } else { doc })
}

fn runtime(inert: bool) -> Runtime<S> {
    let mut rt = Runtime::new(S { inert, ..S::default() }, view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt
}

fn tab(rt: &mut Runtime<S>) {
    rt.key(KeyEvent { key: Key::Tab, mods: Mods::default() });
    rt.frame(1.0 / 60.0);
}

#[test]
fn presses_hovers_and_cursors_fall_through_to_what_is_under() {
    let mut rt = runtime(false);
    click(&mut rt, (20.0, 20.0));
    assert_eq!(rt.state.log, ["button"], "without inert, the document takes the press");
    assert_eq!(rt.cursor(), Cursor::Pointer);

    let mut rt = runtime(true);
    click(&mut rt, (20.0, 20.0));
    assert_eq!(rt.state.log, ["canvas"], "the press goes to the canvas under the document");
    assert_eq!(rt.cursor(), Cursor::Crosshair, "with the canvas's cursor");
    assert!(rt.state.hover.contains(&("canvas", true)));
    assert!(!rt.state.hover.iter().any(|(k, _)| *k == "button"), "the document never hovers");
    assert_eq!(info(&rt, "button").hover, 0.0);
}

#[test]
fn it_still_draws_and_lays_out() {
    let mut rt = runtime(true);
    rt.frame(1.0 / 60.0);
    let doc = info(&rt, "doc");
    assert_eq!(doc.rect, [0.0, 0.0, 100.0, 100.0]);
    assert!(doc.opacity > 0.99);
    assert!(rt.instances.iter().any(|i| i.rect[2] == 100.0 && i.rect[3] == 100.0), "the document's fill draws");
    assert_eq!(rt.state.rects.last(), Some(&[0.0, 0.0, 100.0, 100.0]), "on_layout still hears its rect");
    assert!(doc.inert && info(&rt, "button").inert, "accessibility reports the subtree inert");
    assert!(!info(&runtime(false), "button").inert);
}

#[test]
fn nothing_inside_takes_or_keeps_focus() {
    let mut rt = runtime(false);
    tab(&mut rt);
    tab(&mut rt);
    assert!(info(&rt, "field").focused || info(&rt, "button").focused, "the document is focusable");

    let mut rt = runtime(true);
    rt.set_full_keyboard_access(true);
    tab(&mut rt);
    assert!(!info(&rt, "button").focused && !info(&rt, "field").focused, "Tab skips the inert subtree");

    // A field inside asking for focus doesn't get it.
    let mut rt = runtime(true);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(1.0 / 60.0);
    assert!(!info(&rt, "field").focused);
    assert!(!rt.state.field.focused, "and the model hears it didn't");

    // Focus inside drops when the subtree turns inert.
    let mut rt = runtime(false);
    rt.update(|s, _| s.field.focused = true);
    rt.frame(1.0 / 60.0);
    assert!(info(&rt, "field").focused);
    rt.update(|s, _| s.inert = true);
    rt.frame(1.0 / 60.0);
    assert!(!info(&rt, "field").focused);
    assert!(!rt.state.field.focused);
}

#[test]
fn canvas_gestures_help_selection_and_held_hover_respect_inert() {
    use rune::{MagnifyEvent, PointerButton, WheelEvent, col, text};

    #[derive(Default)]
    struct Canvas {
        inert: bool,
        wheels: Vec<&'static str>,
        pinches: Vec<&'static str>,
        middle: Vec<&'static str>,
    }

    fn canvas_view(s: &Canvas, _: &mut Cx<Canvas>) -> El {
        let doc = col().key("doc").absolute(0.0, 0.0).size(100.0, 100.0)
            .hover(Color::WHITE.alpha(0.2))
            .press(Color::WHITE.alpha(0.4))
            .press_scale(0.95)
            .help("Document tooltip")
            .pointer_buttons([PointerButton::Middle])
            .on_press(|s: &mut Canvas| s.middle.push("doc"))
            .on_wheel(|s: &mut Canvas, _, _| { s.wheels.push("doc"); true })
            .on_magnify(|s: &mut Canvas, _, _| { s.pinches.push("doc"); true })
            .child(text("selectable copy").key("copy").selectable(true));
        el().key("canvas").size(200.0, 200.0)
            .pointer_buttons([PointerButton::Middle])
            .on_press(|s: &mut Canvas| s.middle.push("canvas"))
            .on_wheel(|s: &mut Canvas, _, _| { s.wheels.push("canvas"); true })
            .on_magnify(|s: &mut Canvas, _, _| { s.pinches.push("canvas"); true })
            .child(if s.inert { doc.inert() } else { doc })
    }

    let mut rt = Runtime::new(Canvas { inert: true, ..Canvas::default() }, canvas_view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.wheel_event(WheelEvent::new(0.0, -10.0).at(20.0, 20.0));
    rt.magnify_event(MagnifyEvent::new(0.1).at(20.0, 20.0));
    rt.button_down(20.0, 20.0, PointerButton::Middle);
    rt.button_up(20.0, 20.0, PointerButton::Middle);
    rt.pointer_move(20.0, 20.0);
    rt.frame(1.0);
    assert_eq!(rt.state.wheels, ["canvas"], "on_wheel passes through inert");
    assert_eq!(rt.state.pinches, ["canvas"], "on_magnify passes through inert");
    assert_eq!(rt.state.middle, ["canvas"], "middle-button press passes through inert");
    assert!(rt.help_tooltip().is_none(), "inert element shows no tooltip");

    // Turning inert while hovered/pressed/selected drops the visual hover, press and selection.
    let mut rt = Runtime::new(Canvas::default(), canvas_view, (200.0, 200.0), Env::default());
    rt.frame(0.0);
    rt.pointer_move(10.0, 5.0);
    rt.pointer_down(10.0, 5.0);
    rt.pointer_up(10.0, 5.0);
    rt.pointer_down(10.0, 5.0);
    rt.frame(1.0 / 60.0);
    let doc = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("doc")).unwrap();
    assert!(doc.hover > 0.0 && doc.press > 0.0);
    rt.update(|s, _| s.inert = true);
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    let doc = rt.elements().into_iter().find(|e| e.key.as_deref() == Some("doc")).unwrap();
    assert_eq!((doc.hover, doc.press), (0.0, 0.0), "turning inert clears hover and press tracks");
}
