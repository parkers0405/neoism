//! Conversions and equality between [`CompactStyle`] and [`taffy::Style`].

use super::*;

impl CompactStyle {
    pub fn from_style(s: Style) -> Self {
        let mut vals = [0u32; HOT_VAL_COUNT];
        let mut len_tags: u16 = 0;
        let mut needs_extra = has_cold_fields(&s);

        let dims = [
            s.size.width.into_raw(),
            s.size.height.into_raw(),
            s.min_size.width.into_raw(),
            s.min_size.height.into_raw(),
            s.max_size.width.into_raw(),
            s.max_size.height.into_raw(),
        ];
        for (i, raw) in dims.into_iter().enumerate() {
            let (v, t) = pack_raw_len(raw);
            vals[i] = v;
            if let Some(code) = encode_dim_tag(t) {
                len_tags |= code << (i * 2);
            } else {
                needs_extra = true;
            }
        }

        let pads = [
            s.padding.left.into_raw(),
            s.padding.right.into_raw(),
            s.padding.top.into_raw(),
            s.padding.bottom.into_raw(),
        ];
        for (i, raw) in pads.into_iter().enumerate() {
            let (v, t) = pack_raw_len(raw);
            vals[IDX_PAD_L + i] = v;
            if t as usize == CompactLength::PERCENT_TAG {
                len_tags |= 1 << (12 + i);
            } else if t as usize != CompactLength::LENGTH_TAG {
                needs_extra = true;
            }
        }

        let (gw_bits, gw_tag) = pack_raw_len(s.gap.width.into_raw());
        let (gh_bits, gh_tag) = pack_raw_len(s.gap.height.into_raw());
        let gap_w_bf16 = if (gw_bits & 0xFFFF) == 0 {
            (gw_bits >> 16) as u16
        } else {
            needs_extra = true;
            0
        };
        let gap_h_bf16 = if (gh_bits & 0xFFFF) == 0 {
            (gh_bits >> 16) as u16
        } else {
            needs_extra = true;
            0
        };

        let flex_grow_bf16 = match pack_bf16_exact(s.flex_grow) {
            Some(b) => b,
            None => {
                needs_extra = true;
                0
            }
        };
        let flex_shrink_bf16 = match pack_bf16_exact(s.flex_shrink) {
            Some(b) => b,
            None => {
                needs_extra = true;
                0
            }
        };

        let (basis_bits, basis_tag) = pack_raw_len(s.flex_basis.into_raw());
        let pos_code = match encode_position(s.position) {
            Some(c) => c,
            None => {
                needs_extra = true;
                0b11
            }
        };
        let mut mode_flags = encode_display(s.display) | (pos_code << MODE_POS_SHIFT);
        if gw_tag as usize == CompactLength::PERCENT_TAG {
            mode_flags |= MODE_GAP_W_PCT;
        } else if gw_tag as usize != CompactLength::LENGTH_TAG {
            needs_extra = true;
        }
        if gh_tag as usize == CompactLength::PERCENT_TAG {
            mode_flags |= MODE_GAP_H_PCT;
        } else if gh_tag as usize != CompactLength::LENGTH_TAG {
            needs_extra = true;
        }
        if basis_tag as usize == CompactLength::LENGTH_TAG && basis_bits == 0 {
            mode_flags |= MODE_BASIS_ZERO;
        } else if basis_tag as usize != CompactLength::AUTO_TAG {
            needs_extra = true;
        }

        let box_flags = pack_box_flags(s.flex_direction, s.flex_wrap, s.overflow.x, s.overflow.y);
        let align_items_u8 = pack_align_items(s.align_items);
        let align_self_u8 = pack_align_self(s.align_self);
        let justify_content_u8 = pack_align_content(s.justify_content);
        let align_content_u8 = pack_align_content(s.align_content);
        let extra = needs_extra.then(|| Box::new(s));

        Self {
            vals,
            gap_w_bf16,
            gap_h_bf16,
            flex_grow_bf16,
            flex_shrink_bf16,
            len_tags,
            box_flags,
            mode_flags,
            align_items_u8,
            align_self_u8,
            justify_content_u8,
            align_content_u8,
            extra,
        }
    }

    pub fn to_style(&self) -> Style {
        if let Some(extra) = &self.extra {
            return (**extra).clone();
        }
        Style {
            display: self.display(),
            position: self.position(),
            overflow: CoreStyle::overflow(self),
            size: CoreStyle::size(self),
            min_size: CoreStyle::min_size(self),
            max_size: CoreStyle::max_size(self),
            aspect_ratio: None,
            padding: CoreStyle::padding(self),
            gap: FlexboxContainerStyle::gap(self),
            align_items: unpack_align_items(self.align_items_u8),
            align_self: unpack_align_self(self.align_self_u8),
            justify_content: unpack_align_content(self.justify_content_u8),
            align_content: unpack_align_content(self.align_content_u8),
            flex_direction: FlexboxContainerStyle::flex_direction(self),
            flex_wrap: FlexboxContainerStyle::flex_wrap(self),
            flex_grow: FlexboxItemStyle::flex_grow(self),
            flex_shrink: FlexboxItemStyle::flex_shrink(self),
            flex_basis: FlexboxItemStyle::flex_basis(self),
            ..Style::DEFAULT
        }
    }

    pub fn heap_bytes(&self) -> usize {
        self.extra.as_ref().map_or(0, |s| {
            type Ident = <Style as CoreStyle>::CustomIdent;
            core::mem::size_of::<Style>()
                + s.grid_template_rows.capacity() * core::mem::size_of::<taffy::GridTemplateComponent<Ident>>()
                + s.grid_template_columns.capacity() * core::mem::size_of::<taffy::GridTemplateComponent<Ident>>()
        })
    }

    #[inline(always)]
    pub fn eq_style(&self, other: &Style) -> bool {
        if let Some(extra) = &self.extra {
            return &**extra == other;
        }
        self.display() == other.display
            && self.position() == other.position
            && self.box_flags == pack_box_flags(other.flex_direction, other.flex_wrap, other.overflow.x, other.overflow.y)
            && self.align_items_u8 == pack_align_items(other.align_items)
            && self.align_self_u8 == pack_align_self(other.align_self)
            && self.justify_content_u8 == pack_align_content(other.justify_content)
            && self.align_content_u8 == pack_align_content(other.align_content)
            && FlexboxItemStyle::flex_grow(self) == other.flex_grow
            && FlexboxItemStyle::flex_shrink(self) == other.flex_shrink
            && other.aspect_ratio.is_none()
            && CoreStyle::size(self) == other.size
            && CoreStyle::min_size(self) == other.min_size
            && CoreStyle::max_size(self) == other.max_size
            && CoreStyle::padding(self) == other.padding
            && FlexboxContainerStyle::gap(self) == other.gap
            && FlexboxItemStyle::flex_basis(self) == other.flex_basis
            && !has_cold_fields(other)
    }
}

impl From<Style> for CompactStyle {
    #[inline]
    fn from(s: Style) -> Self {
        Self::from_style(s)
    }
}

impl From<&Style> for CompactStyle {
    #[inline]
    fn from(s: &Style) -> Self {
        Self::from_style(s.clone())
    }
}

impl From<CompactStyle> for Style {
    #[inline]
    fn from(s: CompactStyle) -> Self {
        match s.extra {
            Some(extra) => *extra,
            None => s.to_style(),
        }
    }
}

impl From<&CompactStyle> for Style {
    #[inline]
    fn from(s: &CompactStyle) -> Self {
        s.to_style()
    }
}

impl PartialEq for CompactStyle {
    #[inline(always)]
    fn eq(&self, other: &Self) -> bool {
        if self.extra.is_none() && other.extra.is_none() {
            return self.vals == other.vals
                && self.gap_w_bf16 == other.gap_w_bf16
                && self.gap_h_bf16 == other.gap_h_bf16
                && self.flex_grow_bf16 == other.flex_grow_bf16
                && self.flex_shrink_bf16 == other.flex_shrink_bf16
                && self.len_tags == other.len_tags
                && self.box_flags == other.box_flags
                && self.mode_flags == other.mode_flags
                && self.align_items_u8 == other.align_items_u8
                && self.align_self_u8 == other.align_self_u8
                && self.justify_content_u8 == other.justify_content_u8
                && self.align_content_u8 == other.align_content_u8;
        }
        self.eq_cold(other)
    }
}

impl CompactStyle {
    #[cold]
    #[inline(never)]
    fn eq_cold(&self, other: &Self) -> bool {
        match (&self.extra, &other.extra) {
            (Some(a), Some(b)) => a == b,
            (Some(a), None) => other.eq_style(a),
            (None, Some(b)) => self.eq_style(b),
            (None, None) => unreachable!(),
        }
    }
}

impl PartialEq<Style> for CompactStyle {
    #[inline(always)]
    fn eq(&self, other: &Style) -> bool {
        self.eq_style(other)
    }
}
