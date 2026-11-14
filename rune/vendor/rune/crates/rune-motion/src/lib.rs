//! # rune-motion
//!
//! Rune's animation runtime: pure math and data structures the render thread drives once per
//! frame. App code states targets; this crate turns them into presented values with no app work
//! per frame (PLAN.md §3, §4.9; research 06).
//!
//! - [`Spring`]: SwiftUI's `Spring(duration:bounce:)`, closed form for every damping regime.
//! - [`Curve`]: springs, CSS tweens ([`Easing`]), decay (fling), keyframes.
//! - [`Animatable`]: f32, [`Vec2`], [`Rect`] (as edges), [`Color`] (premultiplied OKLab),
//!   [`Corners`], [`Transform`], [`Blur`], tuples, arrays.
//! - [`AnimationStore`]: struct-of-arrays tracks, retargeting that keeps position and velocity,
//!   merge or additive blending, delays, settle events, zero allocation per frame.
//! - [`Scene`]: presence (exits that stay mounted), [`Transition`]s incl. blur-replace, layout
//!   animation by edges, screen-space travel, matched geometry, per-frame [`Presented`] output.
//! - [`Choreography`], [`Stagger`], [`PhaseAnimator`], [`Keyframes`]; [`Clock`]; motion-lint
//!   [`MotionTrace`]s.
//!
//! ```
//! use rune_motion::prelude::*;
//!
//! let mut store = AnimationStore::new();
//! let card = NodeId(1);
//! store.set(card, prop::OPACITY, 0.0);
//! store.animate(card, prop::OPACITY, 1.0, motion::SETTLE);
//! store.tick_at(0.15);
//! let o = store.get(card, prop::OPACITY).unwrap();
//! assert!(o > 0.5 && o < 1.0);
//! ```

mod animated;
mod choreo;
mod clock;
mod curve;
mod cycle;
mod easing;
mod gesture;
mod keyframes;
mod layer;
mod looping;
pub mod math;
mod props;
mod scene;
mod sequence;
pub mod snap;
mod spring;
mod store;
pub mod tokens;
mod trace;
mod transition;
mod value;

pub use animated::Animated;
pub use choreo::Choreography;
pub use choreo::Cue;
pub use choreo::Ops;
pub use choreo::PhaseAnimator;
pub use choreo::Stagger;
pub use choreo::StaggerFrom;
pub use choreo::StepId;
pub use choreo::When;
pub use choreo::stagger;
pub use clock::Clock;
pub use curve::Anim;
pub use curve::AnimExt;
pub use curve::Blend;
pub use curve::Curve;
pub use curve::Decay;
pub use curve::Velocity;
pub use cycle::Cycle;
pub use easing::CubicBezier;
pub use easing::Easing;
pub use easing::StepPosition;
pub use gesture::VelocityTracker;
pub use gesture::deceleration;
pub use gesture::project;
pub use gesture::rubber_band;
pub use gesture::rubber_band_clamp;
pub use gesture::rubber_band_with;
pub use keyframes::Keyframe;
pub use keyframes::Keyframes;
pub use layer::Eps;
pub use looping::LoopKind;
pub use looping::LoopSpec;
pub use looping::LoopTrack;
pub use props::NodeId;
pub use props::Prop;
pub use props::PropClass;
pub use props::PropId;
pub use props::REDUCED_FADE;
pub use props::prop;
pub use scene::FrameOutput;
pub use scene::LayoutAnim;
pub use scene::Mask;
pub use scene::Mount;
pub use scene::NodeSpec;
pub use scene::Phase;
pub use scene::Presented;
pub use scene::Scene;
pub use scene::Travel;
pub use scene::WIDTH_LEAD;
pub use sequence::Beat;
pub use sequence::Sequence;
pub use spring::Spring;
pub use spring::SpringPair;
pub use store::AnimationStore;
#[doc(hidden)]
pub use store::Fx;
pub use store::MotionEvent;
pub use store::Release;
pub use store::RetargetAudit;
pub use store::TrackId;
pub use tokens as motion;
pub use trace::Intent;
pub use trace::MotionTrace;
pub use trace::SharedBuffer;
pub use trace::TraceChannels;
pub use trace::TraceHeader;
pub use trace::flags;
pub use transition::Edge;
pub use transition::Effect;
pub use transition::Reveal;
pub use transition::SMALL_SCALE_FLOOR;
pub use transition::Transition;
pub use value::Animatable;
pub use value::Blur;
pub use value::Color;
pub use value::Corners;
pub use value::MAX_LANES;
pub use value::Rect;
pub use value::Sides;
pub use value::Transform;
pub use value::Vec2;
pub use value::linear_to_oklab;
pub use value::oklab_to_linear;
pub use value::srgb_decode;
pub use value::srgb_encode;

/// Everything an animation call site needs.
pub mod prelude {
    pub use crate::Anim;
    pub use crate::AnimExt;
    pub use crate::Animatable;
    pub use crate::Animated;
    pub use crate::AnimationStore;
    pub use crate::Blend;
    pub use crate::Clock;
    pub use crate::Color;
    pub use crate::Corners;
    pub use crate::Cue;
    pub use crate::Curve;
    pub use crate::Decay;
    pub use crate::Easing;
    pub use crate::Edge;
    pub use crate::Keyframes;
    pub use crate::LayoutAnim;
    pub use crate::Mount;
    pub use crate::NodeId;
    pub use crate::NodeSpec;
    pub use crate::Presented;
    pub use crate::Rect;
    pub use crate::Release;
    pub use crate::Scene;
    pub use crate::Spring;
    pub use crate::SpringPair;
    pub use crate::Stagger;
    pub use crate::StaggerFrom;
    pub use crate::Transition;
    pub use crate::Travel;
    pub use crate::Vec2;
    pub use crate::Velocity;
    pub use crate::motion;
    pub use crate::prop;
}
