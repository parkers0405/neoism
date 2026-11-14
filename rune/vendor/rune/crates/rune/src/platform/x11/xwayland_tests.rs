use super::*;
use x11rb::protocol::xproto::{ButtonPressEvent, LeaveNotifyEvent, MotionNotifyEvent};

fn read(event: Event) -> Vec<String> {
    let mut input = Vec::new();
    pointer_input(event, 2.0, (200, 100), &mut input);
    input.iter().map(|i| match i {
        Input::Move(x, y) => format!("move {x} {y}"),
        Input::Leave => "leave".into(),
        Input::Button(down) => format!("button {down}"),
        Input::Secondary(down) => format!("secondary {down}"),
        _ => "other".into(),
    }).collect()
}

fn button(detail: u8, x: i16, y: i16) -> ButtonPressEvent {
    ButtonPressEvent { detail, event_x: x, event_y: y, ..Default::default() }
}

#[test]
fn pixels_become_points_and_buttons_follow_a_move() {
    assert_eq!(read(Event::MotionNotify(MotionNotifyEvent { event_x: 40, event_y: 20, ..Default::default() })), ["move 20 10"]);
    assert_eq!(read(Event::ButtonPress(button(1, 40, 20))), ["move 20 10", "button true"]);
    assert_eq!(read(Event::ButtonRelease(button(3, 40, 20))), ["move 20 10", "secondary false"]);
    assert!(read(Event::ButtonPress(button(4, 40, 20))).is_empty(), "scrolling isn't a click");
}

#[test]
fn a_held_button_keeps_the_pointer_until_its_release() {
    let held = LeaveNotifyEvent { state: KeyButMask::BUTTON1, ..Default::default() };
    assert!(read(Event::LeaveNotify(held)).is_empty());
    assert_eq!(read(Event::LeaveNotify(LeaveNotifyEvent::default())), ["leave"]);
    assert_eq!(read(Event::ButtonRelease(button(1, 260, 20))), ["move 130 10", "button false", "leave"]);
}
