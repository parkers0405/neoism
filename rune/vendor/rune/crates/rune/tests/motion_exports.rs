//! Every type the re-exported motion API takes or returns can be named through `rune`.
use rune::{AnimExt, Blend, CubicBezier, Decay, Easing, Edge, Effect, Reveal, Spring, StepPosition, Transition, Vec2, Velocity};

#[test]
fn transition_and_anim_arguments_are_nameable_from_rune() {
    let anchored = Transition::scale_anchor(0.96, Vec2::new(0.5, 0.0));
    assert_eq!(anchored.insertion.anchor, Vec2::new(0.5, 0.0));
    let _: Vec2 = Edge::Top.direction();
    let _: Effect = Effect::scale(0.9).combined(Effect::opacity(0.0));
    let _ = Reveal { edge: Edge::Bottom, feather: 4.0 };
    let _ = Easing::CubicBezier(CubicBezier::new(0.2, 0.0, 0.0, 1.0));
    let _ = Easing::Steps(4, StepPosition::End);
    let _ = Decay::IOS_NORMAL;
    let _ = Spring::from_response(0.3, 1.0).blend(Blend::Additive).velocity(Velocity::Zero);
}
