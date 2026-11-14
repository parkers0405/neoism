//! Compact node style representation (64 bytes inline — one CPU cache line — vs 552 bytes for `taffy::Style`).
//!
//! Stores the hot Flexbox/Block/Core properties inline in 56 bytes plus an 8-byte `Option<Box<Style>>`
//! for cold CSS properties (Grid tracks/placement, non-zero margin/border/inset, `aspect_ratio`,
//! or exotic sizing keywords).

use taffy::geometry::{Line, Point, Rect, Size};
use taffy::style::{
    AlignContent, AlignContentKeyword, AlignItems, AlignItemsKeyword, AlignSelf, AlignmentSafety, BlockContainerStyle,
    BlockItemStyle, BoxGenerationMode, BoxSizing, CompactLength, Contain, CoreStyle, Dimension, Direction, Display,
    FlexDirection, FlexWrap, FlexboxContainerStyle, FlexboxItemStyle, GridAutoFlow, GridContainerStyle, GridItemStyle,
    GridPlacement, JustifyContent, LengthPercentage, LengthPercentageAuto, OofItemStyle, Overflow, Position, Style,
    TextAlign,
};
use taffy::style_helpers::{TaffyAuto, TaffyZero, fr, line, span};

const IDX_SIZE_W: usize = 0;
const IDX_SIZE_H: usize = 1;
const IDX_MIN_W: usize = 2;
const IDX_MIN_H: usize = 3;
const IDX_MAX_W: usize = 4;
const IDX_MAX_H: usize = 5;
const IDX_PAD_L: usize = 6;
const IDX_PAD_R: usize = 7;
const IDX_PAD_T: usize = 8;
const IDX_PAD_B: usize = 9;
const HOT_VAL_COUNT: usize = 10;

const MODE_DISPLAY_MASK: u8 = 0b0000_0111;
const MODE_POS_SHIFT: u8 = 3;
const MODE_POS_MASK: u8 = 0b0001_1000;
const MODE_GAP_W_PCT: u8 = 1 << 5;
const MODE_GAP_H_PCT: u8 = 1 << 6;
const MODE_BASIS_ZERO: u8 = 1 << 7;

const BASE_LEN_TAGS: u16 = 0b0000_1010_1010_1010;
const ONE_BF16: u16 = 0x3F80;

#[inline(always)]
fn pack_raw_len(len: CompactLength) -> (u32, u8) {
    #[cfg(target_pointer_width = "64")]
    {
        // SAFETY: on 64-bit targets `CompactLength` is a single pointer-sized word (`usize`).
        let raw: usize = unsafe { core::mem::transmute(len) };
        ((raw >> 32) as u32, (raw & 0xFF) as u8)
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        let tag = len.tag() as u8;
        let bits = if (tag as usize & CompactLength::KEYWORD_BIT) == 0
            || matches!(
                tag as usize,
                CompactLength::FR_TAG | CompactLength::FIT_CONTENT_PX_TAG | CompactLength::FIT_CONTENT_PERCENT_TAG
            ) {
            len.value().to_bits()
        } else {
            0
        };
        (bits, tag)
    }
}

#[inline(always)]
fn encode_dim_tag(raw_tag: u8) -> Option<u16> {
    match raw_tag as usize {
        CompactLength::LENGTH_TAG => Some(0),
        CompactLength::PERCENT_TAG => Some(1),
        CompactLength::AUTO_TAG => Some(2),
        CompactLength::MAX_CONTENT_TAG => Some(3),
        _ => None,
    }
}

#[inline(always)]
fn decode_dim_tag(code: u16) -> u8 {
    match code & 0b11 {
        0 => CompactLength::LENGTH_TAG as u8,
        1 => CompactLength::PERCENT_TAG as u8,
        2 => CompactLength::AUTO_TAG as u8,
        _ => CompactLength::MAX_CONTENT_TAG as u8,
    }
}

#[inline(always)]
fn unpack_len(bits: u32, tag: u8) -> CompactLength {
    #[cfg(target_pointer_width = "64")]
    {
        let raw: usize = ((bits as usize) << 32) | (tag as usize);
        // SAFETY: `tag` is one of `LENGTH_TAG`, `PERCENT_TAG`, `AUTO_TAG`, or `MAX_CONTENT_TAG`,
        // matching `CompactLength`'s 64-bit tagged representation.
        unsafe { core::mem::transmute(raw) }
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        let val = f32::from_bits(bits);
        match tag as usize {
            CompactLength::LENGTH_TAG => CompactLength::length(val),
            CompactLength::PERCENT_TAG => CompactLength::percent(val),
            CompactLength::AUTO_TAG => CompactLength::auto(),
            CompactLength::MAX_CONTENT_TAG => CompactLength::max_content(),
            _ => CompactLength::auto(),
        }
    }
}

#[inline(always)]
fn unpack_dim(bits: u32, code: u16) -> Dimension {
    // SAFETY: `decode_dim_tag` produces only valid `Dimension` tags (`LENGTH`, `PERCENT`, `AUTO`, `MAX_CONTENT`).
    unsafe { Dimension::from_raw(unpack_len(bits, decode_dim_tag(code))) }
}

#[inline(always)]
fn unpack_lpa(bits: u32, code: u16) -> LengthPercentageAuto {
    // SAFETY: `decode_dim_tag` produces only valid `LengthPercentageAuto` tags.
    unsafe { LengthPercentageAuto::from_raw(unpack_len(bits, decode_dim_tag(code))) }
}

#[inline(always)]
fn unpack_lp(bits: u32, is_pct: bool) -> LengthPercentage {
    let tag = if is_pct {
        CompactLength::PERCENT_TAG as u8
    } else {
        CompactLength::LENGTH_TAG as u8
    };
    // SAFETY: `tag` is either `LENGTH_TAG` or `PERCENT_TAG`, both valid for `LengthPercentage`.
    unsafe { LengthPercentage::from_raw(unpack_len(bits, tag)) }
}

#[inline(always)]
fn pack_bf16_exact(v: f32) -> Option<u16> {
    let bits = v.to_bits();
    ((bits & 0xFFFF) == 0).then_some((bits >> 16) as u16)
}

#[inline(always)]
fn unpack_bf16(b: u16) -> f32 {
    f32::from_bits((b as u32) << 16)
}

#[inline(always)]
fn rect_is_zero_lpa(r: &Rect<LengthPercentageAuto>) -> bool {
    const ZERO: Rect<LengthPercentageAuto> = Rect {
        left: LengthPercentageAuto::ZERO,
        right: LengthPercentageAuto::ZERO,
        top: LengthPercentageAuto::ZERO,
        bottom: LengthPercentageAuto::ZERO,
    };
    #[cfg(target_pointer_width = "64")]
    {
        // SAFETY: on 64-bit targets `Rect<LengthPercentageAuto>` is four 8-byte-aligned 64-bit words.
        let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
        // SAFETY: `ZERO` has the same `Rect<LengthPercentageAuto>` layout.
        let b: &[u64; 4] = unsafe { &*(&ZERO as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
        a == b || *r == ZERO
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        *r == ZERO
    }
}

#[inline(always)]
fn rect_is_zero_lp(r: &Rect<LengthPercentage>) -> bool {
    const ZERO: Rect<LengthPercentage> = Rect {
        left: LengthPercentage::ZERO,
        right: LengthPercentage::ZERO,
        top: LengthPercentage::ZERO,
        bottom: LengthPercentage::ZERO,
    };
    #[cfg(target_pointer_width = "64")]
    {
        // SAFETY: on 64-bit targets `Rect<LengthPercentage>` is four 8-byte-aligned 64-bit words.
        let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentage> as *const [u64; 4]) };
        // SAFETY: `ZERO` has the same `Rect<LengthPercentage>` layout.
        let b: &[u64; 4] = unsafe { &*(&ZERO as *const Rect<LengthPercentage> as *const [u64; 4]) };
        a == b || *r == ZERO
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        *r == ZERO
    }
}

#[inline(always)]
fn rect_is_auto_lpa(r: &Rect<LengthPercentageAuto>) -> bool {
    const AUTO: Rect<LengthPercentageAuto> = Rect {
        left: LengthPercentageAuto::AUTO,
        right: LengthPercentageAuto::AUTO,
        top: LengthPercentageAuto::AUTO,
        bottom: LengthPercentageAuto::AUTO,
    };
    #[cfg(target_pointer_width = "64")]
    {
        // SAFETY: on 64-bit targets `Rect<LengthPercentageAuto>` is four 8-byte-aligned 64-bit words.
        let a: &[u64; 4] = unsafe { &*(r as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
        // SAFETY: `AUTO` has the same `Rect<LengthPercentageAuto>` layout.
        let b: &[u64; 4] = unsafe { &*(&AUTO as *const Rect<LengthPercentageAuto> as *const [u64; 4]) };
        a == b || *r == AUTO
    }
    #[cfg(not(target_pointer_width = "64"))]
    {
        *r == AUTO
    }
}

#[inline(always)]
fn has_cold_fields(s: &Style) -> bool {
    s.display == Display::Grid
        || s.aspect_ratio.is_some()
        || !rect_is_zero_lpa(&s.margin)
        || !rect_is_auto_lpa(&s.inset)
        || !rect_is_zero_lp(&s.border)
        || s.box_sizing != BoxSizing::BorderBox
        || s.direction != Direction::Ltr
        || s.scrollbar_width != 0.0
        || s.contain != Contain::NONE
        || s.justify_items != AlignItems::NORMAL
        || s.justify_self.is_some()
        || s.text_align != TextAlign::Auto
        || s.item_is_table
        || s.item_is_replaced
        || s.item_is_compressible_replaced
        || s.grid_auto_flow != GridAutoFlow::Row
        || s.grid_row != Line { start: GridPlacement::Auto, end: GridPlacement::Auto }
        || s.grid_column != Line { start: GridPlacement::Auto, end: GridPlacement::Auto }
        || !s.grid_template_rows.is_empty()
        || !s.grid_template_columns.is_empty()
        || !s.grid_auto_rows.is_empty()
        || !s.grid_auto_columns.is_empty()
        || s.grid_template_areas.is_some()
        || !s.grid_template_column_names.is_empty()
        || !s.grid_template_row_names.is_empty()
}

#[inline(always)]
const fn encode_display(d: Display) -> u8 {
    match d {
        Display::Flex => 0,
        Display::Block => 1,
        Display::Grid => 2,
        Display::FlowRoot => 3,
        Display::None => 4,
    }
}

#[inline(always)]
const fn decode_display(b: u8) -> Display {
    match b & MODE_DISPLAY_MASK {
        0 => Display::Flex,
        1 => Display::Block,
        2 => Display::Grid,
        3 => Display::FlowRoot,
        _ => Display::None,
    }
}

#[inline(always)]
const fn encode_position(p: Position) -> Option<u8> {
    match p {
        Position::Static => Some(0),
        Position::Relative => Some(1),
        Position::Absolute => Some(2),
        Position::Fixed | Position::Sticky => None,
    }
}

#[inline(always)]
fn decode_position(mode_flags: u8, extra: Option<&Style>) -> Position {
    match (mode_flags & MODE_POS_MASK) >> MODE_POS_SHIFT {
        0 => Position::Static,
        1 => Position::Relative,
        2 => Position::Absolute,
        _ => extra.map_or(Position::Static, |s| s.position),
    }
}

const ALIGN_SELF_NONE: u8 = 0xFF;

#[inline(always)]
const fn pack_align_items(a: AlignItems) -> u8 {
    ((a.safety as u8) << 4) | (a.keyword as u8)
}

#[inline(always)]
const fn unpack_align_items(b: u8) -> AlignItems {
    let safety = match (b >> 4) & 0b11 {
        0 => AlignmentSafety::Default,
        1 => AlignmentSafety::Unsafe,
        _ => AlignmentSafety::Safe,
    };
    let keyword = match b & 0x0F {
        0 => AlignItemsKeyword::Normal,
        1 => AlignItemsKeyword::Start,
        2 => AlignItemsKeyword::End,
        3 => AlignItemsKeyword::FlexStart,
        4 => AlignItemsKeyword::FlexEnd,
        5 => AlignItemsKeyword::SelfStart,
        6 => AlignItemsKeyword::SelfEnd,
        7 => AlignItemsKeyword::Center,
        8 => AlignItemsKeyword::Baseline,
        _ => AlignItemsKeyword::Stretch,
    };
    AlignItems { keyword, safety }
}

#[inline(always)]
const fn pack_align_self(a: Option<AlignSelf>) -> u8 {
    match a {
        Some(v) => pack_align_items(v),
        None => ALIGN_SELF_NONE,
    }
}

#[inline(always)]
const fn unpack_align_self(b: u8) -> Option<AlignSelf> {
    if b == ALIGN_SELF_NONE {
        None
    } else {
        Some(unpack_align_items(b))
    }
}

#[inline(always)]
const fn pack_align_content(a: AlignContent) -> u8 {
    ((a.safety as u8) << 4) | (a.keyword as u8)
}

#[inline(always)]
const fn unpack_align_content(b: u8) -> AlignContent {
    let safety = match (b >> 4) & 0b11 {
        0 => AlignmentSafety::Default,
        1 => AlignmentSafety::Unsafe,
        _ => AlignmentSafety::Safe,
    };
    let keyword = match b & 0x0F {
        0 => AlignContentKeyword::Normal,
        1 => AlignContentKeyword::Start,
        2 => AlignContentKeyword::End,
        3 => AlignContentKeyword::FlexStart,
        4 => AlignContentKeyword::FlexEnd,
        5 => AlignContentKeyword::Center,
        6 => AlignContentKeyword::Stretch,
        7 => AlignContentKeyword::SpaceBetween,
        8 => AlignContentKeyword::SpaceEvenly,
        _ => AlignContentKeyword::SpaceAround,
    };
    AlignContent { keyword, safety }
}

#[inline(always)]
const fn encode_overflow(o: Overflow) -> u8 {
    match o {
        Overflow::Visible => 0,
        Overflow::Clip => 1,
        Overflow::Hidden => 2,
        Overflow::Scroll => 3,
    }
}

#[inline(always)]
const fn decode_overflow(b: u8) -> Overflow {
    match b & 0b11 {
        0 => Overflow::Visible,
        1 => Overflow::Clip,
        2 => Overflow::Hidden,
        _ => Overflow::Scroll,
    }
}

#[inline(always)]
const fn encode_wrap(w: FlexWrap) -> u8 {
    match w {
        FlexWrap::NoWrap => 0,
        FlexWrap::Wrap => 1,
        FlexWrap::WrapReverse => 2,
    }
}

#[inline(always)]
const fn decode_wrap(b: u8) -> FlexWrap {
    match b & 0b11 {
        0 => FlexWrap::NoWrap,
        1 => FlexWrap::Wrap,
        _ => FlexWrap::WrapReverse,
    }
}

#[inline(always)]
const fn encode_dir(d: FlexDirection) -> u8 {
    match d {
        FlexDirection::Row => 0,
        FlexDirection::Column => 1,
        FlexDirection::RowReverse => 2,
        FlexDirection::ColumnReverse => 3,
    }
}

#[inline(always)]
const fn decode_dir(b: u8) -> FlexDirection {
    match b & 0b11 {
        0 => FlexDirection::Row,
        1 => FlexDirection::Column,
        2 => FlexDirection::RowReverse,
        _ => FlexDirection::ColumnReverse,
    }
}

#[inline(always)]
const fn pack_box_flags(dir: FlexDirection, wrap: FlexWrap, ox: Overflow, oy: Overflow) -> u8 {
    encode_dir(dir) | (encode_wrap(wrap) << 2) | (encode_overflow(ox) << 4) | (encode_overflow(oy) << 6)
}

/// Compact representation of `taffy::Style` (64 bytes inline — one cache line).
#[derive(Debug)]
pub struct CompactStyle {
    vals: [u32; HOT_VAL_COUNT],
    gap_w_bf16: u16,
    gap_h_bf16: u16,
    flex_grow_bf16: u16,
    flex_shrink_bf16: u16,
    /// Bits 0..12: 2-bit tags for `size.w/h`, `min_size.w/h`, `max_size.w/h`.
    /// Bits 12..16: 1-bit tags (`0 = Length`, `1 = Percent`) for `pad.l/r/t/b`.
    len_tags: u16,
    box_flags: u8,
    /// Bits 0..3: `Display`.
    /// Bits 3..5: `Position` (`Static`, `Relative`, `Absolute`).
    /// Bit 5: `gap.width` is `Percent`.
    /// Bit 6: `gap.height` is `Percent`.
    /// Bit 7: `flex_basis` is `Length(0.0)` (otherwise `Auto`).
    mode_flags: u8,
    align_items_u8: u8,
    align_self_u8: u8,
    justify_content_u8: u8,
    align_content_u8: u8,
    extra: Option<Box<Style>>,
}

const _: () = assert!(core::mem::size_of::<CompactStyle>() <= 64);
#[cfg(target_pointer_width = "64")]
const _: () = assert!(core::mem::size_of::<CompactStyle>() == 64);

impl Clone for CompactStyle {
    #[inline(always)]
    fn clone(&self) -> Self {
        if self.extra.is_none() {
            Self {
                vals: self.vals,
                gap_w_bf16: self.gap_w_bf16,
                gap_h_bf16: self.gap_h_bf16,
                flex_grow_bf16: self.flex_grow_bf16,
                flex_shrink_bf16: self.flex_shrink_bf16,
                len_tags: self.len_tags,
                box_flags: self.box_flags,
                mode_flags: self.mode_flags,
                align_items_u8: self.align_items_u8,
                align_self_u8: self.align_self_u8,
                justify_content_u8: self.justify_content_u8,
                align_content_u8: self.align_content_u8,
                extra: None,
            }
        } else {
            self.clone_cold()
        }
    }
}

impl Default for CompactStyle {
    #[inline(always)]
    fn default() -> Self {
        Self {
            vals: [0; HOT_VAL_COUNT],
            gap_w_bf16: 0,
            gap_h_bf16: 0,
            flex_grow_bf16: 0,
            flex_shrink_bf16: ONE_BF16,
            len_tags: BASE_LEN_TAGS,
            box_flags: 0,
            mode_flags: encode_display(Display::Flex),
            align_items_u8: pack_align_items(AlignItems::NORMAL),
            align_self_u8: ALIGN_SELF_NONE,
            justify_content_u8: pack_align_content(JustifyContent::NORMAL),
            align_content_u8: pack_align_content(AlignContent::NORMAL),
            extra: None,
        }
    }
}

impl CompactStyle {
    /// Base element style used by Rune (`Display::Flex`, `Position::Relative`, `flex_shrink: 0.0`).
    #[inline(always)]
    pub const fn rune_base(dir: FlexDirection) -> Self {
        Self {
            vals: [0; HOT_VAL_COUNT],
            gap_w_bf16: 0,
            gap_h_bf16: 0,
            flex_grow_bf16: 0,
            flex_shrink_bf16: 0,
            len_tags: BASE_LEN_TAGS,
            box_flags: encode_dir(dir),
            mode_flags: encode_display(Display::Flex) | (1 << MODE_POS_SHIFT),
            align_items_u8: pack_align_items(AlignItems::NORMAL),
            align_self_u8: ALIGN_SELF_NONE,
            justify_content_u8: pack_align_content(JustifyContent::NORMAL),
            align_content_u8: pack_align_content(AlignContent::NORMAL),
            extra: None,
        }
    }

    #[cold]
    #[inline(never)]
    fn clone_cold(&self) -> Self {
        Self {
            vals: self.vals,
            gap_w_bf16: self.gap_w_bf16,
            gap_h_bf16: self.gap_h_bf16,
            flex_grow_bf16: self.flex_grow_bf16,
            flex_shrink_bf16: self.flex_shrink_bf16,
            len_tags: self.len_tags,
            box_flags: self.box_flags,
            mode_flags: self.mode_flags,
            align_items_u8: self.align_items_u8,
            align_self_u8: self.align_self_u8,
            justify_content_u8: self.justify_content_u8,
            align_content_u8: self.align_content_u8,
            extra: self.extra.clone(),
        }
    }

    #[cold]
    #[inline(never)]
    pub fn modify_cold(&mut self, f: impl FnOnce(&mut Style)) {
        let mut s = match self.extra.take() {
            Some(b) => *b,
            None => self.to_style(),
        };
        f(&mut s);
        *self = Self::from_style(s);
    }

    #[inline(always)]
    pub fn display(&self) -> Display {
        decode_display(self.mode_flags)
    }

    #[inline(always)]
    pub fn position(&self) -> Position {
        decode_position(self.mode_flags, self.extra.as_deref())
    }

    #[inline(always)]
    pub fn flex_direction(&self) -> FlexDirection {
        decode_dir(self.box_flags)
    }

    #[inline(always)]
    pub fn flex_wrap(&self) -> FlexWrap {
        decode_wrap(self.box_flags >> 2)
    }

    #[inline(always)]
    pub fn size(&self) -> Size<Dimension> {
        CoreStyle::size(self)
    }

    #[inline(always)]
    pub fn min_size(&self) -> Size<LengthPercentageAuto> {
        CoreStyle::min_size(self)
    }

    #[inline(always)]
    pub fn max_size(&self) -> Size<LengthPercentageAuto> {
        CoreStyle::max_size(self)
    }

    #[inline(always)]
    pub fn padding(&self) -> Rect<LengthPercentage> {
        CoreStyle::padding(self)
    }

    #[inline(always)]
    pub fn inset(&self) -> Rect<LengthPercentageAuto> {
        CoreStyle::inset(self)
    }

    #[inline(always)]
    pub fn gap(&self) -> Size<LengthPercentage> {
        FlexboxContainerStyle::gap(self)
    }

    #[inline(always)]
    pub fn flex_grow(&self) -> f32 {
        FlexboxItemStyle::flex_grow(self)
    }

    #[inline(always)]
    pub fn flex_shrink(&self) -> f32 {
        FlexboxItemStyle::flex_shrink(self)
    }

    #[inline(always)]
    pub fn flex_basis(&self) -> Dimension {
        FlexboxItemStyle::flex_basis(self)
    }

    #[inline(always)]
    pub fn aspect_ratio(&self) -> Option<f32> {
        CoreStyle::aspect_ratio(self)
    }

    #[inline(always)]
    pub fn align_self(&self) -> Option<AlignSelf> {
        unpack_align_self(self.align_self_u8)
    }


}

mod convert;
mod setters;
mod traits;
