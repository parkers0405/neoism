//! The shared motion tokens, from the motion presets (generated as
//! `presets/generated/kit_motion.rs`). Every spring is duration + bounce, bounce 0 by default.
//!
//! Plain names are the enter springs; `*_EXIT` the exit springs; [`pair`] holds both.
//! Aliases follow the JSON's `aka` lists (old palette names).

use crate::curve::Curve;
use crate::easing::Easing;
use crate::spring::{Spring, SpringPair};

/// Pointer-tied. Barely a spring: takes the edge off a jump without lagging the hand.
pub const INSTANT: Spring = Spring::new(0.1, 0.0);
pub const INSTANT_EXIT: Spring = Spring::new(0.1, 0.0);
/// Small controls answering a click: indicators, widths that lead, exits.
pub const SNAP: Spring = Spring::new(0.25, 0.0);
pub const SNAP_EXIT: Spring = Spring::new(0.2, 0.0);
/// The default state change. Arrives and stops, no drama.
pub const SETTLE: Spring = Spring::new(0.3, 0.0);
pub const SETTLE_EXIT: Spring = Spring::new(0.24, 0.0);
/// Layout moving out of the way: siblings sliding into a gap, containers resizing.
pub const REFLOW: Spring = Spring::new(0.4, 0.0);
pub const REFLOW_EXIT: Spring = Spring::new(0.32, 0.0);
/// Big surfaces travelling: drawers, sheets, pages.
pub const GLIDE: Spring = Spring::new(0.32, 0.0);
pub const GLIDE_EXIT: Spring = Spring::new(0.26, 0.0);
/// One shape becoming another: a chip opening into its panel, a menu out of its button.
pub const MORPH: Spring = Spring::new(0.3, 0.16);
pub const MORPH_EXIT: Spring = Spring::new(0.27, 0.08);
/// A release with a little life in it. Keep it to presses and confirmations.
pub const POP: Spring = Spring::new(0.25, 0.15);
pub const POP_EXIT: Spring = Spring::new(0.2, 0.05);
/// Slow reveals and wipes the eye should follow, never utility UI.
pub const DRIFT: Spring = Spring::new(0.85, 0.0);
pub const DRIFT_EXIT: Spring = Spring::new(0.6, 0.0);
/// Explainer camera moves (motion palette: 1.0–1.8 / 0).
pub const CAMERA: Spring = Spring::new(1.2, 0.0);

// Old palette names (the JSON's `aka`).
pub const SNAPPY: Spring = SNAP;
pub const QUICK: Spring = SNAP;
pub const DEFAULT: Spring = SETTLE;
pub const CALM: Spring = REFLOW;
pub const SMOOTH: Spring = GLIDE;
pub const PLAYFUL: Spring = POP;
pub const SLOW: Spring = DRIFT;

/// The fade that replaces travel under Reduce Motion.
pub const REDUCED_FADE: Curve = crate::props::REDUCED_FADE;
/// Kit's CSS enter: 150 ms `cubic-bezier(.2,.8,.2,1)`.
pub const CSS_ENTER: Curve = Curve::Tween { duration: 0.15, easing: Easing::ENTER };
/// Kit's CSS exit: 110 ms `cubic-bezier(.4,0,1,1)`.
pub const CSS_EXIT: Curve = Curve::Tween { duration: 0.11, easing: Easing::EXIT };

/// Drag-follow lead (ω 34, ζ .86): only for opt-in follow, never the user's own hand.
pub fn follow_lead() -> Spring {
    Spring::from_omega_zeta(34.0, 0.86)
}
/// Drag-follow trail (ω 25, ζ 1).
pub fn follow_trail() -> Spring {
    Spring::from_omega_zeta(25.0, 1.0)
}

/// Enter/exit pairs.
pub mod pair {
    use super::*;
    pub const INSTANT: SpringPair = SpringPair::new(super::INSTANT, INSTANT_EXIT);
    pub const SNAP: SpringPair = SpringPair::new(super::SNAP, SNAP_EXIT);
    pub const SETTLE: SpringPair = SpringPair::new(super::SETTLE, SETTLE_EXIT);
    pub const REFLOW: SpringPair = SpringPair::new(super::REFLOW, REFLOW_EXIT);
    pub const GLIDE: SpringPair = SpringPair::new(super::GLIDE, GLIDE_EXIT);
    pub const MORPH: SpringPair = SpringPair::new(super::MORPH, MORPH_EXIT);
    pub const POP: SpringPair = SpringPair::new(super::POP, POP_EXIT);
    pub const DRIFT: SpringPair = SpringPair::new(super::DRIFT, DRIFT_EXIT);
}

/// SwiftUI's named springs.
pub mod swiftui {
    use super::Spring;
    pub const SMOOTH: Spring = Spring::SMOOTH;
    pub const SNAPPY: Spring = Spring::SNAPPY;
    pub const BOUNCY: Spring = Spring::BOUNCY;
    pub const SPRING: Spring = Spring::DEFAULT;
}

/// Every token by name, for tuning panels, traces and tests.
pub const ALL: &[(&str, SpringPair)] = &[
    ("instant", pair::INSTANT),
    ("snap", pair::SNAP),
    ("settle", pair::SETTLE),
    ("reflow", pair::REFLOW),
    ("glide", pair::GLIDE),
    ("morph", pair::MORPH),
    ("pop", pair::POP),
    ("drift", pair::DRIFT),
];
