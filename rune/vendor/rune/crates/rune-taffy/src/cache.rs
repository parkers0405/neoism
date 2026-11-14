//! Compact layout cache (96 bytes inline vs 408 bytes in `taffy::Cache`) with the exact 9-slot
//! second-chance clock semantics of `taffy::Cache`.
//!
//! Stores the final-layout entry and the first two `ComputeSize` entries inline (since Flexbox
//! measures a child at most twice per pass), and lazily allocates slots 2..9 or cold final-layout
//! fields (`oof_candidates`, non-zero top/left overflow, `baselines.last`, collapsible margins)
//! only when needed.

use taffy::geometry::{Point, Rect, Size};
use taffy::style::AvailableSpace;
use taffy::tree::{
    Baselines, ClearState, CollapsibleMarginSet, LayoutInput, LayoutOutput, OofCandidates, RequestedAxis, RunMode,
    SizingMode,
};

const CACHE_SIZE: usize = 9;
const INLINE_MEASURES: usize = 2;
const OVERFLOW_MEASURES: usize = CACHE_SIZE - INLINE_MEASURES;

const MEASURE_MASK_BITS: u32 = (1 << CACHE_SIZE) - 1; // bits 0..9
const FINAL_VALID_BIT: u32 = 1 << 9; // bit 9
const VALID_BITS_MASK: u32 = MEASURE_MASK_BITS | FINAL_VALID_BIT; // bits 0..10
const RECENT_SHIFT: u32 = 10; // bits 10..19
const NEXT_SHIFT: u32 = 19; // bits 19..23 (0..9)
const MODE_SHIFT: u32 = 23; // bits 23..26 (0..8)
const M0_DEF_SHIFT: u32 = 26; // bits 26..28
const M1_DEF_SHIFT: u32 = 28; // bits 28..30
const FINAL_DEF_SHIFT: u32 = 30; // bits 30..32

const INFINITY_BITS: u32 = f32::INFINITY.to_bits();
const NEG_INFINITY_BITS: u32 = f32::NEG_INFINITY.to_bits();

const SIGN_BIT_1: u64 = 1u64 << 63;
const SIGN_BIT_2: u64 = 1u64 << 31;
const BOTH_SIGN_BITS_MASK: u64 = SIGN_BIT_1 | SIGN_BIT_2;
const NON_SIGN_BITS_MASK: u64 = !BOTH_SIGN_BITS_MASK;
const X_AXIS_VALUE_MASK: u64 = (u32::MAX as u64) << 32;

#[inline(always)]
fn option_cache_key(input: Option<f32>) -> u32 {
    match input {
        Some(value) => value.to_bits(),
        None => INFINITY_BITS,
    }
}

#[inline(always)]
fn size_option_cache_key(input: Size<Option<f32>>) -> u64 {
    (option_cache_key(input.width) as u64) << 32 | option_cache_key(input.height) as u64
}

#[inline(always)]
fn available_space_cache_key(input: AvailableSpace) -> u32 {
    match input {
        AvailableSpace::Definite(value) => (-value).to_bits(),
        AvailableSpace::MinContent => NEG_INFINITY_BITS,
        AvailableSpace::MaxContent => INFINITY_BITS,
    }
}

#[inline(always)]
fn mixed_cache_key(kd: Option<f32>, avs: AvailableSpace) -> u32 {
    kd.map(f32::to_bits).unwrap_or_else(|| available_space_cache_key(avs))
}

#[inline(always)]
fn size_mixed_cache_key(kd: Size<Option<f32>>, avs: Size<AvailableSpace>) -> u64 {
    (mixed_cache_key(kd.width, avs.width) as u64) << 32 | mixed_cache_key(kd.height, avs.height) as u64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CacheKey {
    kd_available_space: u64,
    parent_size: u64,
    kd_def: u8,
}

impl CacheKey {
    #[inline(always)]
    fn from_input(input: &LayoutInput) -> Self {
        let extra_bits = match input.axis {
            RequestedAxis::Horizontal => SIGN_BIT_1,
            RequestedAxis::Vertical => SIGN_BIT_2,
            RequestedAxis::Both => SIGN_BIT_1 | SIGN_BIT_2,
        };
        let def_w = input.known_dimensions_are_definite.width || input.known_dimensions.width.is_none();
        let def_h = input.known_dimensions_are_definite.height || input.known_dimensions.height.is_none();
        Self {
            kd_available_space: size_mixed_cache_key(input.known_dimensions, input.available_space),
            parent_size: (size_option_cache_key(input.parent_size) & NON_SIGN_BITS_MASK) | extra_bits,
            kd_def: (def_w as u8) | ((def_h as u8) << 1),
        }
    }

    #[inline(always)]
    fn x_axis_parent_size(parent_size: u64) -> u64 {
        parent_size & (X_AXIS_VALUE_MASK & NON_SIGN_BITS_MASK)
    }

    #[inline(always)]
    fn requested_axis_bits(parent_size: u64) -> u64 {
        parent_size & BOTH_SIGN_BITS_MASK
    }

    #[inline(always)]
    fn size_is_valid_for(entry_parent_size: u64, query_parent_size: u64) -> bool {
        let entry_axis = Self::requested_axis_bits(entry_parent_size);
        entry_axis == BOTH_SIGN_BITS_MASK || entry_axis == Self::requested_axis_bits(query_parent_size)
    }
}

#[inline(always)]
fn cache_mode(input: &LayoutInput) -> u8 {
    let inherent = matches!(input.sizing_mode, SizingMode::InherentSize) as u8;
    let c = input.vertical_margins_are_collapsible;
    inherent | ((c.start as u8) << 1) | ((c.end as u8) << 2)
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CompactMeasureEntry {
    kd_available_space: u64,
    parent_size: u64,
    size: Size<f32>,
}

impl CompactMeasureEntry {
    const ZERO: Self = Self {
        kd_available_space: 0,
        parent_size: 0,
        size: Size::ZERO,
    };
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CompactFinalEntry {
    kd_available_space: u64,
    parent_size: u64,
    size: Size<f32>,
    overflow_br: Size<f32>,
    baseline_first: f32,
    meta: u32,
}

impl CompactFinalEntry {
    const ZERO: Self = Self {
        kd_available_space: 0,
        parent_size: 0,
        size: Size::ZERO,
        overflow_br: Size::ZERO,
        baseline_first: f32::NAN,
        meta: 0,
    };
}

#[derive(Debug, Clone, PartialEq)]
struct ColdFinalOutput {
    overflow_lt: Point<f32>,
    baseline_last: Option<f32>,
    top_margin: CollapsibleMarginSet,
    bottom_margin: CollapsibleMarginSet,
    margins_can_collapse_through: bool,
    oof_candidates: OofCandidates,
}

#[derive(Debug, Clone, PartialEq)]
struct CacheOverflow {
    measures: [CompactMeasureEntry; OVERFLOW_MEASURES],
    /// 2-bit `kd_def` for overflow slots `2..9` (14 bits used).
    kd_defs: u16,
    cold_final: Option<ColdFinalOutput>,
}

impl Default for CacheOverflow {
    fn default() -> Self {
        Self {
            measures: [CompactMeasureEntry::ZERO; OVERFLOW_MEASURES],
            kd_defs: 0,
            cold_final: None,
        }
    }
}

/// Compact per-node layout cache (96 bytes inline).
#[derive(Debug, Clone, PartialEq)]
pub struct CompactCache {
    final_entry: CompactFinalEntry,
    measures: [CompactMeasureEntry; INLINE_MEASURES],
    overflow: Option<Box<CacheOverflow>>,
}

const _: () = assert!(core::mem::size_of::<CompactCache>() <= 96);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<CompactCache>() == 96);

impl Default for CompactCache {
    fn default() -> Self {
        Self::new()
    }
}

impl CompactCache {
    pub const fn new() -> Self {
        Self {
            final_entry: CompactFinalEntry::ZERO,
            measures: [CompactMeasureEntry::ZERO; INLINE_MEASURES],
            overflow: None,
        }
    }

    #[inline(always)]
    fn kd_def(&self, slot: usize) -> u8 {
        match slot {
            0 => ((self.final_entry.meta >> M0_DEF_SHIFT) & 0b11) as u8,
            1 => ((self.final_entry.meta >> M1_DEF_SHIFT) & 0b11) as u8,
            CACHE_SIZE => ((self.final_entry.meta >> FINAL_DEF_SHIFT) & 0b11) as u8,
            _ => self
                .overflow
                .as_ref()
                .map_or(0, |ov| ((ov.kd_defs >> ((slot - INLINE_MEASURES) * 2)) & 0b11) as u8),
        }
    }

    #[inline(always)]
    fn set_kd_def(&mut self, slot: usize, kd_def: u8) {
        let bits = (kd_def & 0b11) as u32;
        match slot {
            0 => self.final_entry.meta = (self.final_entry.meta & !(0b11 << M0_DEF_SHIFT)) | (bits << M0_DEF_SHIFT),
            1 => self.final_entry.meta = (self.final_entry.meta & !(0b11 << M1_DEF_SHIFT)) | (bits << M1_DEF_SHIFT),
            CACHE_SIZE => {
                self.final_entry.meta = (self.final_entry.meta & !(0b11 << FINAL_DEF_SHIFT)) | (bits << FINAL_DEF_SHIFT);
            }
            _ => {
                let shift = (slot - INLINE_MEASURES) * 2;
                let ov = self.overflow.get_or_insert_with(Default::default);
                ov.kd_defs = (ov.kd_defs & !(0b11 << shift)) | ((bits as u16) << shift);
            }
        }
    }

    #[inline(always)]
    fn next_measure_entry(&self) -> u8 {
        ((self.final_entry.meta >> NEXT_SHIFT) & 0xF) as u8
    }

    #[inline(always)]
    fn set_next_measure_entry(&mut self, next: u8) {
        self.final_entry.meta =
            (self.final_entry.meta & !(0xF << NEXT_SHIFT)) | (((next & 0xF) as u32) << NEXT_SHIFT);
    }

    #[inline(always)]
    fn mode(&self) -> u8 {
        ((self.final_entry.meta >> MODE_SHIFT) & 0x7) as u8
    }

    #[inline(always)]
    fn set_mode(&mut self, mode: u8) {
        self.final_entry.meta =
            (self.final_entry.meta & !(0x7 << MODE_SHIFT)) | (((mode & 0x7) as u32) << MODE_SHIFT);
    }

    #[inline(always)]
    fn measure_slot(&self, index: usize) -> Option<&CompactMeasureEntry> {
        if index < INLINE_MEASURES {
            Some(&self.measures[index])
        } else {
            self.overflow.as_ref().map(|ov| &ov.measures[index - INLINE_MEASURES])
        }
    }

    #[inline(always)]
    fn measure_slot_mut(&mut self, index: usize) -> &mut CompactMeasureEntry {
        if index < INLINE_MEASURES {
            &mut self.measures[index]
        } else {
            &mut self.overflow.get_or_insert_with(Default::default).measures[index - INLINE_MEASURES]
        }
    }

    #[inline]
    pub fn get(&mut self, input: &LayoutInput) -> Option<LayoutOutput> {
        let meta = self.final_entry.meta;
        if (meta & VALID_BITS_MASK) == 0 || self.mode() != cache_mode(input) {
            return None;
        }
        let key = CacheKey::from_input(input);
        match input.run_mode {
            RunMode::PerformLayout => {
                if (meta & FINAL_VALID_BIT) == 0 {
                    return None;
                }
                if self.final_entry.kd_available_space != key.kd_available_space
                    || self.final_entry.parent_size != key.parent_size
                    || self.kd_def(CACHE_SIZE) != key.kd_def
                {
                    return None;
                }
                let by0 = self.final_entry.baseline_first;
                let cold = self.overflow.as_ref().and_then(|ov| ov.cold_final.as_ref());
                let (lt, baseline_last) = match cold {
                    Some(c) => (c.overflow_lt, c.baseline_last),
                    None => (Point::ZERO, None),
                };
                let baselines = Baselines {
                    first: (!by0.is_nan()).then_some(by0),
                    last: baseline_last,
                };
                let rect = Rect {
                    left: lt.x,
                    top: lt.y,
                    right: self.final_entry.overflow_br.width,
                    bottom: self.final_entry.overflow_br.height,
                };
                let mut out = LayoutOutput::from_sizes_and_baselines(self.final_entry.size, rect, baselines);
                if let Some(c) = cold {
                    out.top_margin = c.top_margin;
                    out.bottom_margin = c.bottom_margin;
                    out.margins_can_collapse_through = c.margins_can_collapse_through;
                    out.oof_candidates = c.oof_candidates.clone();
                }
                Some(out)
            }
            RunMode::ComputeSize => {
                let measure_mask = meta & MEASURE_MASK_BITS;
                if measure_mask == 0 {
                    return None;
                }
                if self.overflow.is_none() {
                    if (measure_mask & 1) != 0 {
                        let entry = &self.measures[0];
                        if entry.kd_available_space == key.kd_available_space
                            && self.kd_def(0) == key.kd_def
                            && CacheKey::x_axis_parent_size(entry.parent_size)
                                == CacheKey::x_axis_parent_size(key.parent_size)
                            && CacheKey::size_is_valid_for(entry.parent_size, key.parent_size)
                        {
                            self.final_entry.meta |= 1 << RECENT_SHIFT;
                            return Some(LayoutOutput::from_outer_size(entry.size));
                        }
                    }
                    if (measure_mask & 2) != 0 {
                        let entry = &self.measures[1];
                        if entry.kd_available_space == key.kd_available_space
                            && self.kd_def(1) == key.kd_def
                            && CacheKey::x_axis_parent_size(entry.parent_size)
                                == CacheKey::x_axis_parent_size(key.parent_size)
                            && CacheKey::size_is_valid_for(entry.parent_size, key.parent_size)
                        {
                            self.final_entry.meta |= 2 << RECENT_SHIFT;
                            return Some(LayoutOutput::from_outer_size(entry.size));
                        }
                    }
                    return None;
                }
                for index in 0..CACHE_SIZE {
                    if (measure_mask & (1 << index)) == 0 {
                        continue;
                    }
                    let Some(entry) = self.measure_slot(index) else { continue };
                    if entry.kd_available_space == key.kd_available_space
                        && self.kd_def(index) == key.kd_def
                        && CacheKey::x_axis_parent_size(entry.parent_size)
                            == CacheKey::x_axis_parent_size(key.parent_size)
                        && CacheKey::size_is_valid_for(entry.parent_size, key.parent_size)
                    {
                        let size = entry.size;
                        self.final_entry.meta |= 1 << (RECENT_SHIFT + index as u32);
                        return Some(LayoutOutput::from_outer_size(size));
                    }
                }
                None
            }
            RunMode::PerformHiddenLayout => None,
        }
    }

    pub fn store(&mut self, input: &LayoutInput, layout_output: LayoutOutput) {
        if input.run_mode == RunMode::PerformHiddenLayout {
            return;
        }
        let mode = cache_mode(input);
        if self.mode() != mode {
            self.clear();
            self.set_mode(mode);
        }
        let key = CacheKey::from_input(input);
        match input.run_mode {
            RunMode::PerformLayout => {
                self.final_entry.meta |= FINAL_VALID_BIT;
                self.set_kd_def(CACHE_SIZE, key.kd_def);
                let meta = self.final_entry.meta;
                let r = layout_output.scrollable_overflow_rect;
                self.final_entry = CompactFinalEntry {
                    kd_available_space: key.kd_available_space,
                    parent_size: key.parent_size,
                    size: layout_output.size,
                    overflow_br: Size {
                        width: r.right,
                        height: r.bottom,
                    },
                    baseline_first: layout_output.baselines.first.unwrap_or(f32::NAN),
                    meta,
                };
                let needs_cold = r.left != 0.0
                    || r.top != 0.0
                    || layout_output.baselines.last.is_some()
                    || layout_output.margins_can_collapse_through
                    || layout_output.top_margin != CollapsibleMarginSet::ZERO
                    || layout_output.bottom_margin != CollapsibleMarginSet::ZERO
                    || !layout_output.oof_candidates.is_empty();
                if needs_cold {
                    self.overflow.get_or_insert_with(Default::default).cold_final = Some(ColdFinalOutput {
                        overflow_lt: Point { x: r.left, y: r.top },
                        baseline_last: layout_output.baselines.last,
                        top_margin: layout_output.top_margin,
                        bottom_margin: layout_output.bottom_margin,
                        margins_can_collapse_through: layout_output.margins_can_collapse_through,
                        oof_candidates: layout_output.oof_candidates,
                    });
                } else if let Some(ov) = &mut self.overflow {
                    ov.cold_final = None;
                }
            }
            RunMode::ComputeSize => {
                if layout_output.margins_can_collapse_through
                    || layout_output.top_margin != CollapsibleMarginSet::ZERO
                    || layout_output.bottom_margin != CollapsibleMarginSet::ZERO
                {
                    return;
                }
                let measure_mask = self.final_entry.meta & MEASURE_MASK_BITS;
                if measure_mask != 0 {
                    let limit = if self.overflow.is_some() { CACHE_SIZE } else { INLINE_MEASURES };
                    for index in 0..limit {
                        if (measure_mask & (1 << index)) == 0 {
                            continue;
                        }
                        if let Some(entry) = self.measure_slot(index)
                            && entry.kd_available_space == key.kd_available_space
                            && entry.parent_size == key.parent_size
                            && self.kd_def(index) == key.kd_def
                        {
                            self.measure_slot_mut(index).size = layout_output.size;
                            self.final_entry.meta |= 1 << (RECENT_SHIFT + index as u32);
                            return;
                        }
                    }
                }
                let mut next = self.next_measure_entry();
                while (self.final_entry.meta & (1 << (RECENT_SHIFT + next as u32))) != 0 {
                    self.final_entry.meta &= !(1 << (RECENT_SHIFT + next as u32));
                    next += 1;
                    if next == CACHE_SIZE as u8 {
                        next = 0;
                    }
                }
                let entry_index = next as usize;
                self.final_entry.meta |= (1 << entry_index) | (1 << (RECENT_SHIFT + entry_index as u32));
                self.set_kd_def(entry_index, key.kd_def);
                *self.measure_slot_mut(entry_index) = CompactMeasureEntry {
                    kd_available_space: key.kd_available_space,
                    parent_size: key.parent_size,
                    size: layout_output.size,
                };
                next += 1;
                if next == CACHE_SIZE as u8 {
                    next = 0;
                }
                self.set_next_measure_entry(next);
            }
            RunMode::PerformHiddenLayout => {}
        }
    }

    #[inline]
    pub fn clear(&mut self) -> ClearState {
        if (self.final_entry.meta & VALID_BITS_MASK) == 0 {
            return ClearState::AlreadyEmpty;
        }
        // Keep `mode` bits intact (same as upstream `taffy::Cache::clear`), clear valid + recent + next + kd_def.
        self.final_entry.meta &= 0x7 << MODE_SHIFT;
        if let Some(ov) = &mut self.overflow {
            ov.kd_defs = 0;
            ov.cold_final = None;
        }
        ClearState::Cleared
    }

    #[inline(always)]
    pub fn is_empty(&self) -> bool {
        (self.final_entry.meta & VALID_BITS_MASK) == 0
    }

    pub fn heap_bytes(&self) -> usize {
        self.overflow.as_ref().map_or(0, |ov| {
            let oof_bytes = ov
                .cold_final
                .as_ref()
                .map_or(0, |c| core::mem::size_of_val(c.oof_candidates.as_slice()));
            core::mem::size_of::<CacheOverflow>() + oof_bytes
        })
    }
}
