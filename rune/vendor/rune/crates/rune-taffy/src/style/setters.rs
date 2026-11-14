//! Builder setters for [`CompactStyle`].

use super::*;

impl CompactStyle {
    #[inline(always)]
    fn set_dim_slot(&mut self, idx: usize, raw: CompactLength, cold_set: impl FnOnce(&mut Style)) {
        if self.extra.is_none() {
            let (v, t) = pack_raw_len(raw);
            if let Some(code) = encode_dim_tag(t) {
                self.vals[idx] = v;
                let shift = idx * 2;
                self.len_tags = (self.len_tags & !(0b11 << shift)) | (code << shift);
                return;
            }
        }
        self.modify_cold(cold_set);
    }

    #[inline(always)]
    pub fn set_width(&mut self, d: Dimension) {
        self.set_dim_slot(IDX_SIZE_W, d.into_raw(), |s| s.size.width = d);
    }

    #[inline(always)]
    pub fn set_height(&mut self, d: Dimension) {
        self.set_dim_slot(IDX_SIZE_H, d.into_raw(), |s| s.size.height = d);
    }

    #[inline(always)]
    pub fn set_size(&mut self, size: Size<Dimension>) {
        self.set_width(size.width);
        self.set_height(size.height);
    }

    #[inline(always)]
    pub fn set_min_width(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MIN_W, d.into_raw(), |s| s.min_size.width = d);
    }

    #[inline(always)]
    pub fn set_min_height(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MIN_H, d.into_raw(), |s| s.min_size.height = d);
    }

    #[inline(always)]
    pub fn set_max_width(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MAX_W, d.into_raw(), |s| s.max_size.width = d);
    }

    #[inline(always)]
    pub fn set_max_height(&mut self, d: LengthPercentageAuto) {
        self.set_dim_slot(IDX_MAX_H, d.into_raw(), |s| s.max_size.height = d);
    }

    #[inline(always)]
    pub fn set_pad_left(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_L] = p.to_bits();
            self.len_tags &= !(1 << 12);
        } else {
            self.modify_cold(|s| s.padding.left = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_right(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_R] = p.to_bits();
            self.len_tags &= !(1 << 13);
        } else {
            self.modify_cold(|s| s.padding.right = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_top(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_T] = p.to_bits();
            self.len_tags &= !(1 << 14);
        } else {
            self.modify_cold(|s| s.padding.top = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_pad_bottom(&mut self, p: f32) {
        if self.extra.is_none() {
            self.vals[IDX_PAD_B] = p.to_bits();
            self.len_tags &= !(1 << 15);
        } else {
            self.modify_cold(|s| s.padding.bottom = LengthPercentage::length(p));
        }
    }

    #[inline(always)]
    pub fn set_padding_all(&mut self, p: f32) {
        if self.extra.is_none() {
            let bits = p.to_bits();
            self.vals[IDX_PAD_L] = bits;
            self.vals[IDX_PAD_R] = bits;
            self.vals[IDX_PAD_T] = bits;
            self.vals[IDX_PAD_B] = bits;
            self.len_tags &= !0xF000;
        } else {
            let lp = LengthPercentage::length(p);
            self.modify_cold(|s| s.padding = Rect { left: lp, right: lp, top: lp, bottom: lp });
        }
    }

    #[inline(always)]
    pub fn set_gap(&mut self, column: f32, row: f32) {
        if self.extra.is_none()
            && let (Some(cw), Some(rh)) = (pack_bf16_exact(column), pack_bf16_exact(row))
        {
            self.gap_w_bf16 = cw;
            self.gap_h_bf16 = rh;
            self.mode_flags &= !(MODE_GAP_W_PCT | MODE_GAP_H_PCT);
            return;
        }
        self.modify_cold(|s| {
            s.gap = Size {
                width: LengthPercentage::length(column),
                height: LengthPercentage::length(row),
            };
        });
    }

    #[inline(always)]
    pub fn set_grow(&mut self, g: f32) {
        if self.extra.is_none()
            && let Some(gb) = pack_bf16_exact(g)
        {
            self.flex_grow_bf16 = gb;
            self.flex_shrink_bf16 = ONE_BF16;
            self.mode_flags |= MODE_BASIS_ZERO;
            return;
        }
        self.modify_cold(|s| {
            s.flex_grow = g;
            s.flex_shrink = 1.0;
            s.flex_basis = Dimension::ZERO;
        });
    }

    #[inline(always)]
    pub fn set_flex_grow(&mut self, g: f32) {
        if self.extra.is_none()
            && let Some(gb) = pack_bf16_exact(g)
        {
            self.flex_grow_bf16 = gb;
            return;
        }
        self.modify_cold(|s| s.flex_grow = g);
    }

    #[inline(always)]
    pub fn set_flex_shrink(&mut self, sh: f32) {
        if self.extra.is_none()
            && let Some(sb) = pack_bf16_exact(sh)
        {
            self.flex_shrink_bf16 = sb;
            return;
        }
        self.modify_cold(|s| s.flex_shrink = sh);
    }

    #[inline(always)]
    pub fn set_flex_basis(&mut self, basis: Dimension) {
        if self.extra.is_none() {
            let (bits, tag) = pack_raw_len(basis.into_raw());
            if tag as usize == CompactLength::AUTO_TAG {
                self.mode_flags &= !MODE_BASIS_ZERO;
                return;
            }
            if tag as usize == CompactLength::LENGTH_TAG && bits == 0 {
                self.mode_flags |= MODE_BASIS_ZERO;
                return;
            }
        }
        self.modify_cold(|s| s.flex_basis = basis);
    }

    #[inline(always)]
    pub fn set_flex_direction(&mut self, dir: FlexDirection) {
        self.box_flags = (self.box_flags & !0b11) | encode_dir(dir);
        if let Some(s) = &mut self.extra {
            s.flex_direction = dir;
        }
    }

    #[inline(always)]
    pub fn set_flex_wrap(&mut self, wrap: FlexWrap) {
        self.box_flags = (self.box_flags & !(0b11 << 2)) | (encode_wrap(wrap) << 2);
        if let Some(s) = &mut self.extra {
            s.flex_wrap = wrap;
        }
    }

    #[inline(always)]
    pub fn set_overflow(&mut self, x: Overflow, y: Overflow) {
        self.box_flags = (self.box_flags & 0x0F) | (encode_overflow(x) << 4) | (encode_overflow(y) << 6);
        if let Some(s) = &mut self.extra {
            s.overflow = Point { x, y };
        }
    }

    #[inline(always)]
    pub fn set_align_items(&mut self, a: AlignItems) {
        self.align_items_u8 = pack_align_items(a);
        if let Some(s) = &mut self.extra {
            s.align_items = a;
        }
    }

    #[inline(always)]
    pub fn set_align_self(&mut self, a: Option<AlignSelf>) {
        self.align_self_u8 = pack_align_self(a);
        if let Some(s) = &mut self.extra {
            s.align_self = a;
        }
    }

    #[inline(always)]
    pub fn set_justify_content(&mut self, j: JustifyContent) {
        self.justify_content_u8 = pack_align_content(j);
        if let Some(s) = &mut self.extra {
            s.justify_content = j;
        }
    }

    #[inline(always)]
    pub fn set_aspect_ratio(&mut self, ratio: Option<f32>) {
        if self.extra.is_none() && ratio.is_none() {
            return;
        }
        self.modify_cold(|s| s.aspect_ratio = ratio);
    }

    #[inline(always)]
    pub fn set_equal_columns(&mut self, n: usize) {
        self.modify_cold(|s| {
            s.display = Display::Grid;
            s.grid_template_columns = vec![fr(1.0); n.max(1)];
        });
    }

    #[inline(always)]
    pub fn set_grid_cell(&mut self, column: usize) {
        self.modify_cold(|s| {
            s.grid_column = Line {
                start: line(column as i16 + 1),
                end: span(1),
            };
            s.grid_row = Line {
                start: line(1),
                end: span(1),
            };
        });
    }

    #[inline(always)]
    pub fn set_position_absolute(&mut self) {
        if let Some(s) = &mut self.extra {
            s.position = Position::Absolute;
        }
        self.mode_flags = (self.mode_flags & !MODE_POS_MASK) | (2 << MODE_POS_SHIFT);
    }

    #[inline(always)]
    pub fn set_absolute_inset(&mut self, inset: Rect<LengthPercentageAuto>) {
        if self.extra.is_none() && rect_is_auto_lpa(&inset) {
            self.mode_flags = (self.mode_flags & !MODE_POS_MASK) | (2 << MODE_POS_SHIFT);
            return;
        }
        self.modify_cold(|s| {
            s.position = Position::Absolute;
            s.inset = inset;
        });
    }

    /// Border widths in points, `[top, right, bottom, left]`, which the box's padding and
    /// content lay out inside.
    #[inline(always)]
    pub fn set_border(&mut self, [top, right, bottom, left]: [f32; 4]) {
        if self.extra.is_none() && top == 0.0 && right == 0.0 && bottom == 0.0 && left == 0.0 { return; }
        let l = LengthPercentage::length;
        self.modify_cold(|s| s.border = Rect { left: l(left), right: l(right), top: l(top), bottom: l(bottom) });
    }

    #[inline(always)]
    pub fn set_margin_left(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.left = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_right(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.right = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_top(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.top = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_bottom(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        self.modify_cold(|s| s.margin.bottom = LengthPercentageAuto::length(m));
    }

    #[inline(always)]
    pub fn set_margin_x(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        let v = LengthPercentageAuto::length(m);
        self.modify_cold(|s| { s.margin.left = v; s.margin.right = v; });
    }

    #[inline(always)]
    pub fn set_margin_y(&mut self, m: f32) {
        if self.extra.is_none() && m == 0.0 { return; }
        let v = LengthPercentageAuto::length(m);
        self.modify_cold(|s| { s.margin.top = v; s.margin.bottom = v; });
    }

    /// Moves this style's outer box sizing and placement onto a fresh column wrapper style and
    /// resets this style's outer box to fill that column (`flex: 1 1 auto`, `min_size: 0`). The
    /// wrapper keeps the overflow too, so a scroll viewport's wrapper is a scroll container: its
    /// automatic minimum size is zero and its content doesn't size the ancestors around it.
    pub fn take_box_placement(&mut self) -> Self {
        if self.extra.is_none() {
            let hidden = (self.mode_flags & MODE_DISPLAY_MASK) == encode_display(Display::None);
            let mut outer = Self::rune_base(FlexDirection::Column);
            outer.vals[..6].copy_from_slice(&self.vals[..6]);
            outer.len_tags = (outer.len_tags & !0x0FFF) | (self.len_tags & 0x0FFF);
            outer.flex_grow_bf16 = self.flex_grow_bf16;
            outer.flex_shrink_bf16 = self.flex_shrink_bf16;
            outer.mode_flags = (outer.mode_flags & !(MODE_DISPLAY_MASK | MODE_POS_MASK | MODE_BASIS_ZERO))
                | (if hidden { encode_display(Display::None) } else { encode_display(Display::Flex) })
                | (self.mode_flags & (MODE_POS_MASK | MODE_BASIS_ZERO));
            outer.align_self_u8 = self.align_self_u8;
            outer.box_flags = (outer.box_flags & 0x0F) | (self.box_flags & 0xF0);

            self.vals[..6].fill(0);
            self.len_tags = (self.len_tags & 0xF000) | 0b0000_1010_0000_1010;
            self.flex_grow_bf16 = ONE_BF16;
            self.flex_shrink_bf16 = ONE_BF16;
            self.mode_flags = (self.mode_flags & !(MODE_DISPLAY_MASK | MODE_POS_MASK | MODE_BASIS_ZERO))
                | encode_display(Display::Flex)
                | (1 << MODE_POS_SHIFT);
            self.align_self_u8 = ALIGN_SELF_NONE;
            return outer;
        }
        let mut s = match self.extra.take() {
            Some(b) => *b,
            None => self.to_style(),
        };
        let outer = Style {
            display: if s.display == Display::None { Display::None } else { Display::Flex },
            flex_direction: FlexDirection::Column,
            overflow: s.overflow,
            flex_shrink: s.flex_shrink,
            flex_grow: s.flex_grow,
            flex_basis: s.flex_basis,
            position: s.position,
            inset: s.inset,
            size: s.size,
            min_size: s.min_size,
            max_size: s.max_size,
            margin: s.margin,
            align_self: s.align_self,
            justify_self: s.justify_self,
            aspect_ratio: s.aspect_ratio,
            grid_row: s.grid_row,
            grid_column: s.grid_column,
            ..Style::DEFAULT
        };
        if s.display == Display::None {
            s.display = Display::Flex;
        }
        s.position = Position::Relative;
        s.inset = Rect {
            left: LengthPercentageAuto::AUTO,
            right: LengthPercentageAuto::AUTO,
            top: LengthPercentageAuto::AUTO,
            bottom: LengthPercentageAuto::AUTO,
        };
        s.size = Size { width: Dimension::AUTO, height: Dimension::AUTO };
        s.min_size = Size { width: LengthPercentageAuto::ZERO, height: LengthPercentageAuto::ZERO };
        s.max_size = Size { width: LengthPercentageAuto::AUTO, height: LengthPercentageAuto::AUTO };
        s.margin = Rect {
            left: LengthPercentageAuto::ZERO,
            right: LengthPercentageAuto::ZERO,
            top: LengthPercentageAuto::ZERO,
            bottom: LengthPercentageAuto::ZERO,
        };
        s.flex_grow = 1.0;
        s.flex_shrink = 1.0;
        s.flex_basis = Dimension::AUTO;
        s.align_self = None;
        s.justify_self = None;
        s.aspect_ratio = None;
        s.grid_row = Line { start: GridPlacement::Auto, end: GridPlacement::Auto };
        s.grid_column = Line { start: GridPlacement::Auto, end: GridPlacement::Auto };
        *self = Self::from_style(s);
        Self::from_style(outer)
    }
}
