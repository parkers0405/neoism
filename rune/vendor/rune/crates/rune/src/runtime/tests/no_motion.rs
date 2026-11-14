use super::*;
use crate::{SpringPair, TextEdit, TextStyle, text_input};

fn find<S: 'static>(rt: &Runtime<S>, key: &str) -> Option<ElementInfo> {
    rt.elements().into_iter().find(|e| e.key.as_deref() == Some(key))
}

/// (shown, transition): a pill over a row that its mount pushes down.
fn pill_view(s: &(bool, Transition), _: &mut Cx<(bool, Transition)>) -> El {
    col().size(200.0, 200.0)
        .children(s.0.then(|| el().key("pill").size(100.0, 40.0).transition(s.1)))
        .child(el().key("row").size(100.0, 40.0))
}

#[test]
fn a_none_transition_lands_in_the_frame_while_the_rest_animates() {
    let short = crate::Spring::new(0.001, 0.0);
    for reduced in [false, true] {
        let mut rt = Runtime::new((false, Transition::none()), pill_view, (200.0, 200.0), Env::new(true, 1.0));
        rt.set_system_reduced_motion(reduced);
        rt.frame(0.0);
        rt.update(|s, _| s.0 = true);
        rt.frame(0.0);
        let pill = find(&rt, "pill").unwrap();
        assert_eq!(pill.opacity, 1.0, "whole in the frame it mounts (reduced motion: {reduced})");
        if !reduced {
            let row = find(&rt, "row").unwrap();
            assert!(row.rect[1] < row.target[1] - 1.0, "the row it pushes still springs: {:?}", row.rect);
        }
        rt.update(|s, _| s.0 = false);
        rt.frame(0.0);
        assert!(find(&rt, "pill").is_none_or(|p| p.opacity == 0.0), "gone in the frame it's removed");
        rt.frame(0.0);
        assert!(find(&rt, "pill").is_none(), "and unmounted by the next");
    }
    // Under Reduce Motion a very short spring becomes a fade, so its node lingers; `none` doesn't.
    let mut rt = Runtime::new((true, Transition::identity().springs(SpringPair::new(short, short))), pill_view, (200.0, 200.0), Env::new(true, 1.0));
    rt.set_system_reduced_motion(true);
    rt.frame(0.0);
    rt.update(|s, _| s.0 = false);
    rt.frame(0.0);
    rt.frame(0.0);
    assert!(find(&rt, "pill").is_some());
}

#[derive(Default)]
struct Form { open: bool, field: TextEdit, modal: bool, none: bool, instant: bool }

fn button(key: &str) -> El { el().key(key).size(30.0, 30.0).focusable(true).role("button") }

fn form_view(s: &Form, _: &mut Cx<Form>) -> El {
    let field = text_input(&s.field, TextStyle::new(14.0, 400.0)).key("field").size(120.0, 30.0).on_edit(|s: &mut Form| &mut s.field);
    let mut popup = col().key("popup").absolute(100.0, 0.0).size(150.0, 100.0).child(field);
    popup = if s.modal { popup.modal(true, |s: &mut Form| s.open = false) } else { popup.focus_return("opener") };
    if s.none { popup = popup.transition(Transition::none()); }
    col().key("root").size(300.0, 300.0)
        .child(button("opener").on_press(|s: &mut Form| { s.open = true; s.field.focused = true; }))
        .children(s.open.then_some(popup))
}

#[test]
fn closing_at_once_returns_focus_to_the_opener() {
    let focused = |rt: &Runtime<Form>| rt.elements().into_iter().find(|e| e.focused).and_then(|e| e.key);
    for modal in [false, true] {
        for (none, instant) in [(false, true), (true, false), (true, true)] {
            let mut rt = Runtime::new(Form { modal, none, instant, ..Form::default() }, form_view, (300.0, 300.0), Env::new(true, 1.0));
            rt.frame(0.0);
            let (x, y) = rt.find("opener").unwrap();
            rt.pointer_down(x, y); rt.pointer_up(x, y); rt.frame(0.0);
            assert_eq!(focused(&rt).as_deref(), Some("field"));
            // The handler leaves the field's own `focused` set: it unmounts without a blur.
            rt.update(|s, tx| { if s.instant { tx.instant(); } s.open = false; });
            rt.frame(0.0);
            rt.frame(1.0 / 60.0);
            assert!(find(&rt, "popup").is_none());
            assert_eq!(focused(&rt).as_deref(), Some("opener"), "modal {modal}, none {none}, instant {instant}");
        }
    }
}
