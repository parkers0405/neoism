use super::*;
use crate::motion_gate::check::paint::{C_BORDER, C_GRADIENT, C_SHADOW, C_TEXT};
use crate::{Live, el, text};

struct State {
    ink: Live<Color>,
    edge: Live<Color>,
    glow: Live<Color>,
    top: Live<Color>,
    bottom: Live<Color>,
    smooth: bool,
    views: std::cell::Cell<u32>,
}

fn view(s: &State, _: &mut Cx<State>) -> El {
    s.views.set(s.views.get() + 1);
    let card = el().key("card").size(80.0, 40.0).fill(Color::BLACK)
        .border(1.0, Color::BLACK).shadow(2.0, 6.0, Color::BLACK).gradient(Color::BLACK, Color::BLACK);
    let card = if s.smooth {
        card.live_border(s.edge.smooth(Spring::new(0.25, 0.0)))
    } else {
        card.live_border(&s.edge)
    };
    el().size(200.0, 200.0).child(
        card.live_shadow(&s.glow).live_gradient(&s.top, &s.bottom)
            .child(text("hi").key("ink").color(Color::BLACK).live_text_color(&s.ink)),
    )
}

fn runtime(smooth: bool) -> Runtime<State> {
    let s = State {
        ink: Live::new(Color::WHITE),
        edge: Live::new(Color::WHITE),
        glow: Live::new(Color::WHITE),
        top: Live::new(Color::WHITE),
        bottom: Live::new(Color::WHITE),
        smooth,
        views: Default::default(),
    };
    let mut rt = Runtime::new(s, view, (200.0, 200.0), Env::new(true, 1.0));
    rt.frame(0.0);
    rt
}

fn paint(rt: &Runtime<State>, key: &str) -> crate::motion_gate::check::paint::Paint {
    let n = rt.nodes.iter().find(|n| n.key.as_deref() == Some(key)).unwrap();
    rt.paints().remove(&(n.path.clone(), false)).unwrap()
}

const RED: Color = Color { r: 1.0, g: 0.0, b: 0.0, a: 1.0 };
const GREEN: Color = Color { r: 0.0, g: 1.0, b: 0.0, a: 1.0 };
const BLUE: Color = Color { r: 0.0, g: 0.0, b: 1.0, a: 1.0 };

#[test]
fn text_border_shadow_and_gradient_colours_change_without_a_view_rerun() {
    let mut rt = runtime(false);
    let card = paint(&rt, "card");
    assert_eq!(card.colors[C_BORDER], Color::WHITE, "the binding wins over the authored colour");
    assert_eq!(paint(&rt, "ink").colors[C_TEXT], Color::WHITE);
    let (views, commits) = (rt.state.views.get(), rt.stats.commits);
    rt.state.ink.set(RED);
    rt.state.edge.set(GREEN);
    rt.state.glow.set(BLUE);
    rt.state.top.set(RED);
    rt.state.bottom.set(GREEN);
    assert!(rt.live_pending());
    rt.frame(1.0 / 120.0);
    assert_eq!((rt.state.views.get(), rt.stats.commits, rt.stats.shaped), (views, commits, 0));
    let card = paint(&rt, "card");
    assert_eq!(card.colors[C_BORDER], GREEN);
    assert_eq!(card.colors[C_SHADOW], BLUE);
    assert_eq!(card.colors[C_GRADIENT], RED);
    assert_eq!(card.gradient_end, GREEN);
    assert_eq!(paint(&rt, "ink").colors[C_TEXT], RED);

    rt.invalidate();
    rt.frame(0.0);
    let card = paint(&rt, "card");
    assert_eq!(card.colors[C_BORDER], GREEN, "a rebuild keeps what the binding last set");
    assert_eq!(card.colors[C_SHADOW], BLUE);
    assert_eq!((card.colors[C_GRADIENT], card.gradient_end), (RED, GREEN));
    assert_eq!(paint(&rt, "ink").colors[C_TEXT], RED);
}

#[test]
fn a_smoothed_border_colour_springs_then_lands_exactly() {
    let mut rt = runtime(true);
    rt.state.edge.set(BLUE);
    rt.frame(0.03);
    let mid = paint(&rt, "card").colors[C_BORDER];
    assert!(mid.r > 0.0 && mid.r < 1.0 && mid.b == 1.0, "moving: {mid:?}");
    for _ in 0..240 { rt.frame(1.0 / 120.0); }
    assert_eq!(paint(&rt, "card").colors[C_BORDER], BLUE);
    assert!(!rt.animating);
}
