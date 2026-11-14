use super::*;

struct State {
    x: f32,
    pressed: u32,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    el().child(el().key("button").size(40.0, 20.0).absolute(s.x, 0.0).on_press(|s: &mut State| s.pressed += 1))
}

fn press_then_slide(move_after: Option<(f32, f32)>) -> u32 {
    let mut rt = Runtime::new(State { x: 0.0, pressed: 0 }, view, (300.0, 100.0), Env::new(true, 2.0));
    rt.frame(0.0);
    rt.pointer_move(20.0, 10.0);
    rt.pointer_down(20.0, 10.0);
    rt.state.x = 200.0;
    rt.invalidate();
    rt.frame(1.0 / 60.0);
    let (x, y) = move_after.unwrap_or((20.0, 10.0));
    rt.pointer_move(x, y);
    rt.pointer_up(x, y);
    rt.frame(0.0);
    rt.state.pressed
}

#[test]
fn a_button_that_moves_out_from_under_a_still_pointer_still_takes_the_press() {
    assert_eq!(press_then_slide(None), 1);
}

#[test]
fn a_pointer_that_moves_off_the_button_still_cancels_the_press() {
    assert_eq!(press_then_slide(Some((25.0, 60.0))), 0);
}
