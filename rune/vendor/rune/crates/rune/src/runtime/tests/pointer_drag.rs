use super::*;


#[test]
fn a_drag_reports_pointer_travel_when_its_window_moves() {
    type State = Vec<(f32, f32)>;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().key("pill").size(100.0, 40.0).on_drag(|s: &mut State, d, _| s.push((d.dx, d.dy)))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 14.0);
    rt.window_moved(30.0, 4.0);
    rt.pointer_move(15.0, 10.0);
    rt.pointer_up(15.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.last(), Some(&(35.0, 4.0)), "travel is measured on screen, not in the moving window");
    assert!(rt.state.contains(&(30.0, 4.0)));
}

#[test]
fn taking_the_pointer_away_ends_a_held_drag_cancelled() {
    type State = Vec<(Phase, bool)>;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().key("pill").size(100.0, 40.0).on_drag(|s: &mut State, d, _| s.push((d.phase, d.cancelled())))
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    rt.cancel_pointer();
    rt.frame(0.0);
    assert_eq!(rt.state.last(), Some(&(Phase::Up, true)));
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.iter().filter(|d| d.0 == Phase::Up).count(), 1, "the release after it does nothing");
}

#[test]
fn native_escape_with_a_logical_fallback_cancels_the_drag() {
    fn view(_: &Vec<bool>, _: &mut Cx<Vec<bool>>) -> El {
        el().size(100.0, 100.0).on_drag(|s: &mut Vec<bool>, d, _| {
            if d.phase == Phase::Up { s.push(d.cancelled()); }
        })
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_down(10.0, 10.0);
    rt.frame(0.0);
    rt.physical_key_input(crate::PhysicalInput::Key { key: crate::PhysicalKeyCode::Escape,
        mods: crate::Mods::default(), pressed: true, repeat: false }, KeyEvent { key: crate::Key::Escape, mods: crate::Mods::default() }, None);
    rt.frame(0.0);
    assert_eq!(rt.state, [true]);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state, [true]);
}

#[derive(Default)]
struct DragOutState { presses: u32, drags: u32, offer: bool, ended: Vec<crate::DragOutcome>, dragging: bool, starts: u32, log: Vec<String> }

fn drag_out_view(_: &DragOutState, _: &mut Cx<DragOutState>) -> El {
    col().size(200.0, 200.0).child(el().key("file").size(100.0, 40.0)
        .on_press(|s: &mut DragOutState| s.presses += 1)
        .on_drag(|s: &mut DragOutState, _, _| s.drags += 1)
        .drag_out(|s: &DragOutState| s.offer.then(|| crate::DragItem::new().file("/Applications/Example.app")))
        .on_drag_start(|s: &mut DragOutState, _| { s.dragging = true; s.starts += 1; s.log.push("start".into()); })
        .on_drag_end(|s: &mut DragOutState, outcome, _| { s.dragging = false; s.ended.push(outcome); s.log.push(format!("end {outcome:?}")); }))
}

fn drag_out_rt(offer: bool) -> Runtime<DragOutState> {
    let mut rt = Runtime::new(DragOutState { offer, ..Default::default() }, drag_out_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt
}

#[test]
fn crossing_the_threshold_hands_a_drag_out_to_the_host_and_releases_the_pointer() {
    let mut rt = drag_out_rt(true);
    let t = crate::drag_out::DRAG_THRESHOLD;
    rt.pointer_move(10.0 + t, 10.0);
    assert!(rt.take_drag_out().is_none(), "at the threshold it's still a press");
    assert!(rt.pointer_captured());
    rt.pointer_move(12.0 + t, 10.0);
    let start = rt.take_drag_out().expect("drag-out requested");
    assert_eq!(start.item.paths(), [std::path::PathBuf::from("/Applications/Example.app")]);
    assert_eq!(start.press, (10.0, 10.0));
    assert_eq!(start.rect, [0.0, 0.0, 100.0, 40.0]);
    assert!(rt.events.iter().any(|e| e.starts_with("drag-out ") && e.ends_with("file")), "{:?}", rt.events);
    assert!(!rt.pointer_captured() && rt.pointer().is_none(), "nothing stays pressed or hovered");
    rt.pointer_move(40.0, 30.0);
    assert!(rt.take_drag_out().is_none(), "one request per press");
    rt.pointer_up(40.0, 30.0);
    rt.frame(0.0);
    assert_eq!((rt.state.presses, rt.state.drags), (0, 0), "neither on_press nor on_drag runs");
}

#[test]
fn an_element_sets_its_own_drag_threshold() {
    fn view(_: &DragOutState, _: &mut Cx<DragOutState>) -> El {
        col().size(200.0, 200.0)
            .child(el().key("near").size(100.0, 40.0).drag_threshold(3.0)
                .drag_out(|_: &DragOutState| Some(crate::DragItem::new().text("near"))))
            .child(el().key("far").size(100.0, 40.0).drag_threshold(12.0)
                .drag_out(|_: &DragOutState| Some(crate::DragItem::new().text("far"))))
    }
    let mut rt = Runtime::new(DragOutState::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(13.0, 10.0);
    assert!(rt.take_drag_out().is_none(), "exactly 3 pt is still a press");
    rt.pointer_move(13.5, 10.0);
    assert!(rt.take_drag_out().is_some_and(|s| s.path.ends_with("near")), "3.5 pt passes a 3 pt threshold, under the default");
    rt.pointer_up(13.5, 10.0);
    rt.frame(0.0);
    rt.pointer_move(10.0, 50.0);
    rt.pointer_down(10.0, 50.0);
    rt.pointer_move(18.0, 50.0);
    assert!(rt.take_drag_out().is_none(), "8 pt is past the default but under this element's 12");
    rt.pointer_move(23.0, 50.0);
    assert!(rt.take_drag_out().is_some_and(|s| s.path.ends_with("far")));
}

#[test]
fn a_drag_out_released_before_the_threshold_is_a_press() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(11.0, 11.0);
    rt.pointer_up(11.0, 11.0);
    rt.frame(0.0);
    assert!(rt.take_drag_out().is_none());
    assert_eq!((rt.state.presses, rt.state.drags), (1, 0), "on_drag never runs on a drag-out element");
    assert!(rt.events.iter().all(|e| !e.starts_with("drag-out")));
}

#[test]
fn a_drag_out_that_offers_nothing_stays_a_press() {
    let mut rt = drag_out_rt(false);
    rt.pointer_move(40.0, 10.0);
    assert!(rt.take_drag_out().is_none());
    assert!(rt.pointer_captured(), "the press holds on");
    rt.pointer_move(20.0, 10.0);
    rt.pointer_up(20.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.presses, 1);
}

#[test]
fn a_drag_out_that_offers_nothing_tracks_leaving_the_element() {
    let mut rt = drag_out_rt(false);
    assert!(rt.press.inside());
    rt.pointer_move(150.0, 10.0);
    assert!(rt.pointer_captured(), "the press holds on");
    assert!(!rt.press.inside(), "the move that declined the drag left the element, so it no longer shows pressed");
}

#[test]
fn hosts_without_os_drags_keep_on_drag() {
    let mut rt = Runtime::new(DragOutState { offer: true, ..Default::default() }, drag_out_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.set_os_drag(false);
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.0);
    assert!(rt.take_drag_out().is_none());
    assert!(rt.state.drags > 0, "on_drag runs as before");
}

#[test]
fn a_reported_drag_end_reaches_on_drag_end_next_frame() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    let start = rt.take_drag_out().unwrap();
    let sink = rt.drag_end_sink();
    sink.borrow_mut().push((start.path, crate::DragOutcome::Copied));
    assert!(rt.drag_ends_pending());
    rt.frame(0.0);
    assert_eq!(rt.state.ended, [crate::DragOutcome::Copied]);
    assert!(!rt.drag_ends_pending());
    assert!(rt.events.iter().any(|e| e.starts_with("drag-end Copied ")));
    assert!(rt.elements().iter().any(|e| e.key.as_deref() == Some("file") && e.drag_out));
}

#[test]
fn on_drag_start_runs_once_the_drag_is_handed_over_and_before_its_end() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(12.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.starts, 0, "not before the threshold");
    rt.pointer_move(40.0, 10.0);
    let start = rt.take_drag_out().unwrap();
    assert_eq!(rt.state.starts, 0, "handlers run between frames, after the host took the drag image");
    rt.frame(0.0);
    assert!(rt.state.dragging && rt.state.starts == 1);
    rt.drag_end_sink().borrow_mut().push((start.path, crate::DragOutcome::Cancelled));
    rt.frame(0.0);
    assert!(!rt.state.dragging);
    assert_eq!((rt.state.starts, rt.state.ended.as_slice()), (1, &[crate::DragOutcome::Cancelled][..]));
}

#[test]
fn a_drag_start_and_end_reported_together_run_in_order() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    let start = rt.take_drag_out().unwrap();
    rt.drag_end_sink().borrow_mut().push((start.path, crate::DragOutcome::Copied));
    rt.frame(0.0);
    assert!(!rt.state.dragging, "the end runs after the start");
    assert_eq!((rt.state.starts, rt.state.ended.as_slice()), (1, &[crate::DragOutcome::Copied][..]));
}

#[test]
fn a_drag_out_no_host_took_ends_cancelled_on_the_next_frame() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    rt.frame(0.0);
    assert!(!rt.state.dragging);
    assert_eq!(rt.state.log, ["start", "end Cancelled"]);
    assert!(rt.events.iter().any(|e| e.starts_with("drag-end Cancelled ")));
    rt.frame(0.0);
    assert_eq!(rt.state.log.len(), 2, "exactly one end");
}

#[test]
fn a_cancelled_press_ends_a_drag_out_no_host_took() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    rt.cancel_pointer();
    assert!(rt.take_drag_out().is_none(), "the cancel ended the request");
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["start", "end Cancelled"]);
}

#[test]
fn untaken_drags_started_in_one_turn_end_in_order() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    rt.pointer_up(40.0, 10.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["start", "end Cancelled", "start", "end Cancelled"]);
}

#[test]
fn a_stale_sessions_end_runs_before_the_next_drags_start() {
    let mut rt = drag_out_rt(true);
    rt.pointer_move(40.0, 10.0);
    let first = rt.take_drag_out().unwrap();
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    let second = rt.take_drag_out().unwrap();
    // macOS reports a session it never ended when the next one replaces it.
    rt.drag_end_sink().borrow_mut().push((first.path, crate::DragOutcome::Cancelled));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["start", "end Cancelled", "start"]);
    assert!(rt.state.dragging, "the second drag is still running");
    rt.drag_end_sink().borrow_mut().push((second.path, crate::DragOutcome::Copied));
    rt.frame(0.0);
    assert_eq!(rt.state.log, ["start", "end Cancelled", "start", "end Copied"]);
}

#[test]
fn a_drag_image_over_the_element_gets_its_unclipped_bounds() {
    fn view(_: &DragOutState, _: &mut Cx<DragOutState>) -> El {
        col().child(col().size(200.0, 100.0).clip_soft(0.0).child(el().size(200.0, 80.0)).child(el().key("row").size(200.0, 40.0)
            .drag_out(|_: &DragOutState| Some(crate::DragItem::new().text("row")))))
    }
    let mut rt = Runtime::new(DragOutState::default(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 90.0);
    rt.pointer_down(10.0, 90.0);
    rt.pointer_move(40.0, 90.0);
    let start = rt.take_drag_out().unwrap();
    assert_eq!(start.rect, [0.0, 80.0, 200.0, 20.0], "the visible strip");
    assert_eq!(start.bounds, [0.0, 80.0, 200.0, 40.0], "the whole row");
}

#[test]
fn cursor_follows_the_hovered_target_and_its_ancestors() {
    use crate::el::Cursor;
    struct State { field: crate::edit::TextEdit }
    fn view(s: &State, _: &mut Cx<State>) -> El {
        col().size(200.0, 200.0)
            .child(el().key("button").size(200.0, 40.0).cursor(Cursor::Pointer).on_press(|_: &mut State| {}))
            .child(el().key("handle").size(200.0, 40.0).cursor(Cursor::Grab)
                .child(el().key("inner").size(100.0, 40.0).on_press(|_: &mut State| {})))
            .child(crate::el::text_input(&s.field, crate::tok::TextStyle::new(14.0, 400.0)).key("field").size(200.0, 40.0)
                .on_edit(|s: &mut State| &mut s.field))
            .child(el().key("plain").size(200.0, 40.0).on_press(|_: &mut State| {}))
    }
    let mut rt = Runtime::new(State { field: crate::edit::TextEdit::new("") }, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    assert_eq!(rt.cursor(), Cursor::Default);
    for (y, cursor) in [(20.0, Cursor::Pointer), (60.0, Cursor::Grab), (100.0, Cursor::Text), (140.0, Cursor::Default)] {
        rt.pointer_move(50.0, y);
        assert_eq!(rt.cursor(), cursor, "at y {y}");
    }
    rt.pointer_move(50.0, 20.0);
    rt.pointer_down(50.0, 20.0);
    rt.pointer_move(50.0, 140.0);
    assert_eq!(rt.cursor(), Cursor::Pointer, "a held press keeps its cursor");
    rt.pointer_up(50.0, 140.0);
    assert_eq!(rt.cursor(), Cursor::Default);
    rt.pointer_leave();
    assert_eq!(rt.cursor(), Cursor::Default);
}

#[test]
fn hidden_cursor_ends_with_its_hover_and_its_element() {
    use crate::el::Cursor;
    fn view(picking: &bool, _: &mut Cx<bool>) -> El {
        let picker = el().key("picker").size(200.0, 100.0).cursor(Cursor::Hidden).on_press(|_: &mut bool| {})
            .child(el().key("area").size(100.0, 100.0).cursor(Cursor::Crosshair).on_press(|_: &mut bool| {}));
        col().size(200.0, 200.0).child(if *picking { picker } else { el().key("idle").size(200.0, 100.0) })
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Crosshair);
    rt.pointer_move(150.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Hidden);
    rt.pointer_move(150.0, 150.0);
    assert_eq!(rt.cursor(), Cursor::Default, "hover left the element");
    rt.pointer_move(150.0, 50.0);
    rt.pointer_leave();
    assert_eq!(rt.cursor(), Cursor::Default, "pointer left the window");
    rt.pointer_move(150.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Hidden);
    rt.update(|picking: &mut bool, _| *picking = false);
    rt.frame(0.0);
    assert_eq!(rt.cursor(), Cursor::Default, "the element went away");
}

#[test]
fn hidden_cursor_ends_when_a_drag_is_released_outside_the_window() {
    use crate::el::Cursor;
    fn view(_: &(), _: &mut Cx<()>) -> El {
        el().size(200.0, 200.0).child(el().key("picker").size(200.0, 100.0).cursor(Cursor::Hidden).on_press(|_: &mut ()| {}))
    }
    let mut rt = Runtime::new((), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);
    rt.pointer_down(50.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Hidden);
    rt.pointer_move(250.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Default, "never an element's cursor outside the window");
    rt.pointer_up(250.0, 50.0);
    assert_eq!(rt.pointer(), None, "a release outside the window is a leave");

    // The host saw the pointer leave mid-press, then releases at its last inside position.
    rt.pointer_move(50.0, 50.0);
    rt.pointer_down(50.0, 50.0);
    rt.pointer_leave();
    assert_eq!(rt.cursor(), Cursor::Hidden, "the press still holds the pointer");
    rt.pointer_up(50.0, 50.0);
    assert_eq!(rt.pointer(), None, "the deferred leave lands on release");
    assert_eq!(rt.cursor(), Cursor::Default);
}

#[test]
fn cursor_follows_a_sibling_that_replaces_an_exiting_element_under_a_still_pointer() {
    use crate::el::Cursor;
    fn view(first: &bool, _: &mut Cx<bool>) -> El {
        let item = |key: &str, cursor: Cursor| el().key(key).absolute(0.0, 0.0).size(100.0, 100.0).cursor(cursor)
            .on_press(|_: &mut bool| {}).transition(Transition::opacity());
        el().size(200.0, 200.0).child(if *first { item("a", Cursor::Crosshair) } else { item("b", Cursor::Hidden) })
    }
    let mut rt = Runtime::new(true, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(50.0, 50.0);
    assert_eq!(rt.cursor(), Cursor::Crosshair);
    rt.update(|first: &mut bool, _| *first = false);
    rt.frame(1.0 / 120.0);
    assert!(rt.nodes.iter().any(|n| n.key.as_deref() == Some("a")), "a is still exiting");
    assert_eq!(rt.cursor(), Cursor::Hidden, "the sibling under the still pointer takes over");
}

#[test]
fn hover_lands_at_once_and_fades_out() {
    fn view(_: &(), _: &mut Cx<()>) -> El {
        col().size(200.0, 80.0)
            .child(el().key("row").size(200.0, 40.0).hover(crate::color::oklcha(1.0, 0.0, 0.0, 0.1)).on_press(|_: &mut ()| {}))
    }
    let mut rt = Runtime::new((), view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let id = rt.nodes.iter().find(|n| n.key.as_deref() == Some("row")).unwrap().id;
    let hover = |rt: &Runtime<()>| rt.scene.get(id, P_HOVER).unwrap_or(0.0);
    rt.pointer_move(50.0, 20.0);
    rt.frame(1.0 / 120.0);
    assert_eq!(hover(&rt), 1.0, "the first frame under the pointer is fully hovered");
    rt.pointer_move(50.0, 60.0);
    rt.frame(1.0 / 120.0);
    let leaving = hover(&rt);
    assert!(leaving > 0.0 && leaving < 1.0, "leaving fades: {leaving}");
    rt.frame(0.5);
    assert_eq!(hover(&rt), 0.0);
}

#[test]
fn rows_mounting_above_keep_a_still_pointers_hover() {
    // Nodes mounting earlier in the tree shift every later index; last frame's geometry
    // mustn't be read under the new indices (the hover would drop for a frame).
    fn view(n: &usize, _: &mut Cx<usize>) -> El {
        col().size(200.0, 80.0)
            .child(col().size(200.0, 40.0).clip_rounded(0.0).children((0..*n).map(|k| el().key(format!("r{k}")).size(200.0, 10.0))))
            .child(crate::row().key("row").size(200.0, 40.0).hover_group()
                .child(el().key("reveal").size(10.0, 10.0).fill(Color::WHITE).group_opacity(0.0, 1.0)))
    }
    let mut rt = Runtime::new(1usize, view, (200.0, 80.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(150.0, 60.0);
    rt.frame(1.0 / 120.0);
    let group = |rt: &Runtime<usize>| {
        let i = rt.nodes.iter().position(|n| n.key.as_deref() == Some("reveal")).unwrap();
        rt.inh_scratch[i].group
    };
    assert_eq!(group(&rt), 1.0);
    for _ in 0..3 {
        rt.update(|n: &mut usize, _| *n += 1);
        rt.frame(1.0 / 120.0);
        assert_eq!(group(&rt), 1.0, "the reveal stays shown while rows mount above");
    }
}

#[test]
fn a_drag_counts_its_run_of_clicks() {
    type State = Vec<(Phase, u32)>;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        col().children([
            el().key("text").size(100.0, 40.0).on_drag(|s: &mut State, d, _| s.push((d.phase, d.clicks)))
                .on_double_press(|s: &mut State| s.push((Phase::Up, 0))),
            el().key("other").size(100.0, 40.0).on_drag(|s: &mut State, d, _| s.push((d.phase, d.clicks))),
        ])
    }
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    let click = |rt: &mut Runtime<State>, x: f32, y: f32| {
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.1);
        let downs: Vec<u32> = rt.state.iter().filter(|e| e.0 == Phase::Down).map(|e| e.1).collect();
        rt.state.clear();
        downs[0]
    };
    let presses: Vec<u32> = (0..4).map(|i| click(&mut rt, 10.0 + i as f32, 10.0)).collect();
    assert_eq!(presses, [1, 2, 3, 4], "each press within the interval and 4 pt of the last counts on");
    rt.frame(1.0);
    assert_eq!(click(&mut rt, 10.0, 10.0), 1, "a press after the interval starts again");
    assert_eq!(click(&mut rt, 10.0, 50.0), 1, "another element starts again");
    assert_eq!(click(&mut rt, 20.0, 10.0), 1, "farther than 4 pt starts again");

    rt.pointer_down(20.0, 10.0);
    rt.pointer_move(40.0, 10.0);
    rt.pointer_up(40.0, 10.0);
    rt.frame(0.1);
    assert!(rt.state.iter().all(|e| e.1 == 2), "every phase of a double-click drag says 2: {:?}", rt.state);
    rt.state.clear();
    assert_eq!(click(&mut rt, 20.0, 10.0), 1, "a press that dragged ends the run");

    rt.frame(1.0);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.1);
    rt.pointer_down(10.0, 10.0);
    rt.pointer_up(10.0, 10.0);
    rt.frame(0.1);
    assert!(rt.state.contains(&(Phase::Up, 0)), "on_double_press still runs: {:?}", rt.state);
}

#[test]
fn a_held_press_hears_its_drag_again_until_released() {
    type State = Vec<(Phase, bool, f32)>;
    fn view(_: &State, _: &mut Cx<State>) -> El {
        el().key("pill").size(100.0, 40.0).on_drag(|s: &mut State, d, tx| {
            s.push((d.phase, tx.drag_repeat(), d.x));
            if d.phase == Phase::Down || tx.drag_repeat() {
                tx.drag_repeat_after(std::time::Duration::from_millis(if tx.drag_repeat() { 50 } else { 300 }));
            }
        })
    }
    let repeats = |rt: &Runtime<State>| rt.state.iter().filter(|e| e.1).count();
    let mut rt = Runtime::new(Vec::new(), view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt.pointer_move(10.0, 10.0);
    rt.pointer_down(10.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert_eq!(rt.state, vec![(Phase::Down, false, 10.0)]);
    assert!(rt.wake_deadline().is_some(), "a still window wakes for the repeat");
    for _ in 0..15 { rt.frame(1.0 / 60.0); }
    assert_eq!(repeats(&rt), 0, "not before the delay");
    for _ in 0..4 { rt.frame(1.0 / 60.0); }
    assert_eq!(repeats(&rt), 1);
    assert_eq!(rt.state.last(), Some(&(Phase::Move, true, 10.0)), "a still pointer is heard again");

    rt.pointer_move(30.0, 10.0);
    rt.frame(1.0 / 60.0);
    assert!(rt.state.contains(&(Phase::Move, false, 30.0)), "the hand's move isn't a repeat");
    for _ in 0..12 { rt.frame(1.0 / 60.0); }
    assert!(repeats(&rt) >= 4, "then every interval: {:?}", rt.state);
    assert_eq!(rt.state.last().map(|e| (e.1, e.2)), Some((true, 30.0)), "where the pointer is now");

    rt.pointer_up(30.0, 10.0);
    rt.frame(1.0 / 60.0);
    let heard = rt.state.len();
    for _ in 0..30 { rt.frame(1.0 / 60.0); }
    assert_eq!(rt.state.len(), heard, "the release ends it");
    assert_eq!(rt.state.last().map(|e| e.0), Some(Phase::Up));
}
