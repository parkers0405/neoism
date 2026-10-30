//! Per-terminal palette storage.
//!
//! `Crosswords` holds a `TermColors` field representing the 269-slot
//! palette overrides the running program has issued via OSC 4 / OSC 10
//! / OSC 11 / OSC 12 etc. The renderer-side `List` (which knows how
//! to fold this together with the configured theme) stays in
//! `neoism-backend`.

use super::{ColorArray, NamedColor};
use std::ops::{Index, IndexMut};

/// Number of terminal colors.
pub const COUNT: usize = 269;

/// Factor for automatic computation of dim colors.
pub const DIM_FACTOR: f32 = 0.66;

#[derive(Copy, Debug, Clone, PartialEq)]
pub struct TermColors([Option<ColorArray>; COUNT]);

impl Default for TermColors {
    fn default() -> Self {
        Self([None; COUNT])
    }
}

impl Index<usize> for TermColors {
    type Output = Option<ColorArray>;

    #[inline]
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}

impl IndexMut<usize> for TermColors {
    #[inline]
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}

impl Index<NamedColor> for TermColors {
    type Output = Option<ColorArray>;

    #[inline]
    fn index(&self, index: NamedColor) -> &Self::Output {
        &self.0[index as usize]
    }
}

impl IndexMut<NamedColor> for TermColors {
    #[inline]
    fn index_mut(&mut self, index: NamedColor) -> &mut Self::Output {
        &mut self.0[index as usize]
    }
}
