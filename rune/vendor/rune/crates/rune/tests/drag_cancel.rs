//! A held drag ends cancelled, with `Phase::Up` and `Drag::cancelled`, when Escape is pressed or
//! the window loses focus. The release that follows does nothing, and other keys pass through.
use rune::{Cx, El, Env, Key, KeyEvent, Mods, Phase, Runtime, col, el};

#[derive(Default)]
struct S {
    drags: Vec<(Phase, bool, f32)>,
    keys: Vec<Key>,
    presses: usize,
}

fn view(_: &S, _: &mut Cx<S>) -> El {
    col().w_full().h_full()
        .child(el().key("handle").w(100.0).h(40.0)
            .on_drag(|s: &mut S, d, _| s.drags.push((d.phase, d.cancelled(), d.dx)))
            .on_press(|s: &mut S| s.presses += 1))
}

fn held() -> Runtime<S> {
    let mut rt = Runtime::new(S::default(), view, (300.0, 300.0), Env::new(true, 1.0));
    rt.on_key = Some(|s, ev, _| { s.keys.push(ev.key.clone()); true });
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    rt.frame(0.0);
    rt
}

fn esc() -> KeyEvent {
    KeyEvent { key: Key::Escape, mods: Mods::default() }
}

#[test]
fn escape_cancels_a_held_drag_and_the_release_does_nothing() {
    let mut rt = held();
    rt.key(esc());
    rt.frame(0.0);
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, true, 30.0)));
    assert!(rt.state.keys.is_empty(), "the cancel took the Escape");
    rt.pointer_move(60.0, 10.0);
    rt.pointer_up(60.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.iter().filter(|d| d.0 == Phase::Up).count(), 1, "exactly one end");
    assert_eq!(rt.state.presses, 0);
}

#[test]
fn losing_window_focus_cancels_a_held_drag() {
    let mut rt = held();
    rt.window_focus(true);
    rt.window_focus(false);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, true, 30.0)));
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.iter().filter(|d| d.0 == Phase::Up).count(), 1);
}

#[test]
fn physical_escape_cancels_a_held_drag_too() {
    let mut rt = held();
    rt.physical_input(rune::PhysicalInput::Key {
        key: rune::PhysicalKeyCode::Escape, mods: Mods::default(), pressed: true, repeat: false,
    });
    rt.frame(0.0);
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, true, 30.0)));
    assert!(rt.state.keys.is_empty());
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.iter().filter(|d| d.0 == Phase::Up).count(), 1);
}

#[test]
fn a_release_ends_uncancelled_and_escape_without_a_drag_passes_through() {
    let mut rt = held();
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.drags.last(), Some(&(Phase::Up, false, 30.0)));
    rt.key(esc());
    rt.frame(0.0);
    assert_eq!(rt.state.keys, [Key::Escape]);
}
