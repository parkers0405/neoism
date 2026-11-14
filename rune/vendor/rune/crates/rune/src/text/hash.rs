//! The shaped-layout caches' keys. A text is hashed once per commit into its shape key (see
//! [`crate::el::TextSpec`]), and each width it breaks at only mixes that key with the width, so
//! measuring and re-breaking a long paragraph doesn't rehash its bytes.
use std::hash::{Hash, Hasher};

use super::{PillChip, Span};
use crate::tok::{OpticalSize, TextStyle};

/// A fast, non-cryptographic hasher: bytes go through `rustc_hash`'s string hash and each word
/// through a folded multiply, as foldhash and wyhash mix. Keys are 64 bits and never leave the
/// process, so it needs to avoid accidental collisions, not adversarial ones.
#[derive(Default)]
pub(crate) struct KeyHasher(u64);

const SEED: u64 = 0x243f_6a88_85a3_08d3;
const MUL: u64 = 0x9e37_79b9_7f4a_7c15;

impl Hasher for KeyHasher {
    fn write(&mut self, bytes: &[u8]) {
        let mut fx = rustc_hash::FxHasher::default();
        fx.write(bytes);
        self.write_u64(fx.finish() ^ bytes.len() as u64);
    }
    fn write_u8(&mut self, i: u8) { self.write_u64(i as u64) }
    fn write_u16(&mut self, i: u16) { self.write_u64(i as u64) }
    fn write_u32(&mut self, i: u32) { self.write_u64(i as u64) }
    fn write_usize(&mut self, i: usize) { self.write_u64(i as u64) }
    fn write_u64(&mut self, i: u64) {
        let full = ((self.0 ^ i ^ SEED) as u128).wrapping_mul(MUL as u128);
        self.0 = (full as u64) ^ ((full >> 64) as u64);
    }
    fn finish(&self) -> u64 { self.0 }
}

/// A key of `text` in `style` at `scale`, with `width` when given (see [`shape_key`]).
pub fn text_hash(text: &str, spans: &[Span], style: TextStyle, scale: f32, width: Option<f32>) -> u64 {
    let mut h = KeyHasher::default();
    text.hash(&mut h);
    for s in spans {
        (s.start, s.end, s.mono, s.weight.map(f32::to_bits), s.size.map(f32::to_bits), s.italic).hash(&mut h);
    }
    (style.size.to_bits(), style.weight.to_bits(), style.family, style.line_height.to_bits(), style.tracking.to_bits())
        .hash(&mut h);
    (scale.to_bits(), width.map(f32::to_bits)).hash(&mut h);
    let optical = match style.optical_size {
        OpticalSize::Auto => (0, 0), OpticalSize::FontDefault => (1, 0), OpticalSize::Fixed(size) => (2, size.to_bits()),
    };
    (style.variations, style.features, optical, style.italic).hash(&mut h);
    h.finish()
}

/// What shaping depends on: the text, its spans, style and scale, overflow-wrap and pills.
pub(crate) fn shape_key(text: &str, spans: &[Span], style: TextStyle, scale: f32, wrap_anywhere: bool, pills: &[PillChip]) -> u64 {
    let key = text_hash(text, spans, style, scale, None);
    if !wrap_anywhere && pills.is_empty() {
        return key;
    }
    let mut h = KeyHasher::default();
    (key, wrap_anywhere).hash(&mut h);
    for p in pills {
        (p.start, p.end, p.icon.is_some(), p.bare.as_ref().map(|b| (b.size.to_bits(), b.gap.to_bits()))).hash(&mut h);
    }
    h.finish()
}

/// [`shape_key`] for a paragraph laid out in `base`: left to right keeps the key, the others
/// salt it, as their bidi levels shape the same text differently.
pub(crate) fn directed(key: u64, base: parley::BaseDirection) -> u64 {
    match base {
        parley::BaseDirection::Ltr => key,
        parley::BaseDirection::Rtl => key ^ 0x6a09_e667_f3bc_c909,
        parley::BaseDirection::Auto => key ^ 0xbb67_ae85_84ca_a73b,
    }
}

/// A shaping broken at `max_width`, aligned and block-indented: the shape key itself when the
/// layout is unwrapped, start-aligned and unindented.
pub(crate) fn break_key(shape: u64, max_width: Option<f32>, align: parley::Alignment, blocks: Option<&[crate::edit::Block]>) -> u64 {
    if max_width.is_none() && align == parley::Alignment::Start && blocks.is_none() {
        return shape;
    }
    let mut h = KeyHasher::default();
    (shape, max_width.map(f32::to_bits), align as u8, blocks).hash(&mut h);
    h.finish()
}

/// A truncated or line-clamped cut of `shape` at `max_width`.
pub(crate) fn elide_key(
    shape: u64,
    mode: u8,
    param: usize,
    max_width: f32,
    align: parley::Alignment,
    blocks: Option<&[crate::edit::Block]>,
) -> u64 {
    let mut h = KeyHasher::default();
    (shape, mode, param, max_width.to_bits(), align as u8, blocks).hash(&mut h);
    h.finish()
}

/// A run's text hashed alone, so each style and scale only mixes it in (see [`super::Run`]).
pub(super) fn run_hash(text: &str) -> u64 {
    let mut h = KeyHasher::default();
    text.hash(&mut h);
    h.finish()
}

/// A run's key at one style and scale.
pub(super) fn run_key(run: u64, style: TextStyle, scale: f32) -> u64 {
    let mut h = KeyHasher::default();
    (run, text_hash("", &[], style, scale, None)).hash(&mut h);
    h.finish()
}
