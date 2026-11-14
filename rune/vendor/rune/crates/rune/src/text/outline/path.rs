//! A text's raster path, as a state machine: text at rest draws atlas masks, text whose scale
//! moves draws from its outlines, and on the frame it comes to rest it still draws from outlines
//! (at its resting pen and size, where both look the same) while its masks are placed, so the
//! next frame finds them.

use crate::machine::Effects;

/// Where a text's glyphs come from this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum RasterPath {
    /// Atlas masks.
    #[default]
    Still,
    /// Outlines, magnified by the moving scale.
    Moving,
    /// Outlines at rest, while the masks are placed.
    Landing,
}

/// What a frame says about a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Frame {
    /// Its scale holds still, or outlines aren't drawn here.
    Resting,
    /// Its scale moves.
    Moving,
}

/// What a step asks the painter to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PathEffect {
    /// Place the text's atlas masks without drawing them.
    PlaceMasks,
}

/// Which way the glyphs placed next are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Emit {
    #[default]
    Atlas,
    Outline,
}

impl RasterPath {
    pub(crate) fn step(&mut self, frame: Frame) -> Effects<PathEffect> {
        match (*self, frame) {
            (_, Frame::Moving) => {
                *self = RasterPath::Moving;
                Effects::none()
            }
            (RasterPath::Moving, Frame::Resting) => {
                *self = RasterPath::Landing;
                Effects::one(PathEffect::PlaceMasks)
            }
            (RasterPath::Landing | RasterPath::Still, Frame::Resting) => {
                *self = RasterPath::Still;
                Effects::none()
            }
        }
    }

    pub(crate) fn emit(self) -> Emit {
        match self {
            RasterPath::Still => Emit::Atlas,
            RasterPath::Moving | RasterPath::Landing => Emit::Outline,
        }
    }
}

/// Each text's path this frame and last, by node: a text not painted in a frame starts again
/// at `Still`. Texts at `Still` aren't kept.
#[derive(Default)]
pub(crate) struct Paths {
    now: std::collections::HashMap<u64, RasterPath, std::hash::BuildHasherDefault<rune_motion::Fx>>,
    last: std::collections::HashMap<u64, RasterPath, std::hash::BuildHasherDefault<rune_motion::Fx>>,
}

impl Paths {
    pub(crate) fn begin_frame(&mut self) {
        std::mem::swap(&mut self.now, &mut self.last);
        self.now.clear();
    }

    /// Steps `node`'s path once a frame; later asks in the same frame get the same path and no
    /// effects.
    pub(crate) fn step(&mut self, node: u64, frame: Frame) -> (RasterPath, Effects<PathEffect>) {
        if let Some(&path) = self.now.get(&node) {
            return (path, Effects::none());
        }
        let mut path = self.last.remove(&node).unwrap_or_default();
        let effects = path.step(frame);
        if path != RasterPath::Still {
            self.now.insert(node, path);
        }
        (path, effects)
    }
}
