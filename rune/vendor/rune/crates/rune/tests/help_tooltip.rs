//! `El::help` tooltips where Rune draws them: the pointer resting on an element for
//! `help_delay` shows its tooltip under it; a press, scroll or key hides it until the pointer
//! leaves; leaving hides it; it follows its element; and it takes no hover or press.
mod support;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col, el, row};
use support::find;

#[derive(Default)]
struct S { presses: u32, hovered: bool, inset: f32 }

fn view(s: &S, _: &mut Cx<S>) -> El {
    col().w_full().h_full().padding(20.0).gap(10.0)
        .child(row().pl(s.inset)
            .child(el().key("copy").size(28.0, 28.0)
                .help("Copy the command")
                .on_press(|s: &mut S| s.presses += 1)
                .on_hover_change(|s: &mut S, on, _| s.hovered = on)))
        .child(el().key("plain").size(100.0, 28.0))
        // Help at the window's bottom edge flips its tooltip above it.
        .child(el().w_full().grow(1.0))
        .child(el().key("low").size(60.0, 24.0).help("Near the bottom"))
}

fn runtime() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (400.0, 300.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt
}

fn centre(rt: &Runtime<S>, key: &str) -> (f32, f32) {
    support::centre(&find(rt, key))
}

fn hover(rt: &mut Runtime<S>, key: &str) {
    let (x, y) = centre(rt, key);
    rt.pointer_move(x, y);
    rt.frame(1.0 / 60.0);
}

fn tip(rt: &Runtime<S>) -> Option<String> { rt.help_tooltip().map(|(t, _)| t.to_owned()) }

#[test]
fn shows_after_the_delay_and_hides_on_exit() {
    let mut rt = runtime();
    assert_eq!(find(&rt, "copy").help.as_deref(), Some("Copy the command"), "tools read the help text");
    hover(&mut rt, "copy");
    assert!(rt.state.hovered, "the element keeps its own hover");
    rt.frame(rt.help_delay - 0.1);
    assert_eq!(tip(&rt), None, "not before the delay");
    rt.frame(0.15);
    assert_eq!(tip(&rt).as_deref(), Some("Copy the command"));

    // Under the element, centred on it, inside the window.
    let (_, r) = rt.help_tooltip().unwrap();
    let e = find(&rt, "copy");
    assert!(r[1] >= e.rect[1] + e.rect[3], "below: {r:?} {e:?}");
    assert!(r[0] >= 4.0 && r[0] + r[2] <= 396.0, "{r:?}");
    assert!(((r[0] + r[2] / 2.0) - (e.rect[0] + e.rect[2] / 2.0)).abs() < 1.0 || r[0] == 4.0, "centred: {r:?}");

    // Moving within the element keeps it; leaving hides it.
    rt.pointer_move(e.rect[0] + 3.0, e.rect[1] + 3.0);
    rt.frame(1.0 / 60.0);
    assert!(tip(&rt).is_some());
    hover(&mut rt, "plain");
    assert_eq!(tip(&rt), None);
    rt.frame(1.0);
    assert_eq!(tip(&rt), None, "no help on plain");

    // Leaving the window hides it too, and coming back waits the delay again.
    hover(&mut rt, "copy");
    rt.frame(1.0);
    assert!(tip(&rt).is_some());
    rt.pointer_leave();
    rt.frame(1.0 / 60.0);
    assert_eq!(tip(&rt), None);
    hover(&mut rt, "copy");
    assert_eq!(tip(&rt), None);

    // The tree reports it.
    rt.frame(1.0);
    let tree = rt.elements();
    assert!(tree.iter().any(|e| e.help.is_some()));
}

#[test]
fn press_scroll_and_key_hide_it_until_the_pointer_leaves() {
    let mut rt = runtime();
    for hide in 0..3 {
        hover(&mut rt, "copy");
        rt.frame(1.0);
        assert!(tip(&rt).is_some(), "shown ({hide})");
        let (x, y) = centre(&rt, "copy");
        match hide {
            0 => { rt.pointer_down(x, y); rt.pointer_up(x, y); }
            1 => rt.wheel(x, y, 10.0),
            _ => rt.key(KeyEvent { key: Key::Escape, mods: Mods::default() }),
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(tip(&rt), None, "hidden ({hide})");
        rt.frame(2.0);
        assert_eq!(tip(&rt), None, "stays hidden while the pointer rests ({hide})");
        hover(&mut rt, "plain");
    }
    assert_eq!(rt.state.presses, 1, "the press went to the element, not the tooltip");
    hover(&mut rt, "copy");
    rt.frame(1.0);
    assert!(tip(&rt).is_some(), "back after leaving");
}

#[test]
fn follows_its_element_and_flips_above_at_the_bottom() {
    let mut rt = runtime();
    hover(&mut rt, "copy");
    rt.frame(1.0);
    let x0 = rt.help_tooltip().unwrap().1[0];
    // The element moves a little under the resting pointer: the tooltip goes with it.
    rt.state.inset = 6.0;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let (_, r) = rt.help_tooltip().expect("still shown");
    assert!((r[0] - (x0 + 6.0)).abs() < 1.0 || x0 == 4.0, "{x0} -> {r:?}");

    hover(&mut rt, "low");
    rt.frame(1.0);
    let (text, r) = rt.help_tooltip().unwrap();
    let e = find(&rt, "low");
    assert_eq!(text, "Near the bottom");
    assert!(r[1] + r[3] <= e.rect[1], "above: {r:?} {e:?}");
}
