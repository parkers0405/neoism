//! Taffy style trait implementations (`CoreStyle`, `Flexbox*`, `Grid*`, `Block*`, `OofItemStyle`) for [`CompactStyle`].

use super::*;

impl CoreStyle for CompactStyle {
    type CustomIdent = <Style as CoreStyle>::CustomIdent;

    #[inline(always)]
    fn box_generation_mode(&self) -> BoxGenerationMode {
        match self.display() {
            Display::None => BoxGenerationMode::None,
            _ => BoxGenerationMode::Normal,
        }
    }

    #[inline(always)]
    fn is_block(&self) -> bool {
        matches!(self.display(), Display::Block)
    }

    #[inline(always)]
    fn is_compressible_replaced(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_replaced || s.item_is_compressible_replaced)
    }

    #[inline(always)]
    fn is_replaced(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_replaced)
    }

    #[inline(always)]
    fn box_sizing(&self) -> BoxSizing {
        self.extra.as_ref().map_or(BoxSizing::BorderBox, |s| s.box_sizing)
    }

    #[inline(always)]
    fn direction(&self) -> Direction {
        self.extra.as_ref().map_or(Direction::Ltr, |s| s.direction)
    }

    #[inline(always)]
    fn overflow(&self) -> Point<Overflow> {
        Point {
            x: decode_overflow(self.box_flags >> 4),
            y: decode_overflow(self.box_flags >> 6),
        }
    }

    #[inline(always)]
    fn scrollbar_width(&self) -> f32 {
        self.extra.as_ref().map_or(0.0, |s| s.scrollbar_width)
    }

    #[inline(always)]
    fn position(&self) -> Position {
        decode_position(self.mode_flags, self.extra.as_deref())
    }

    #[inline(always)]
    fn inset(&self) -> Rect<LengthPercentageAuto> {
        self.extra.as_ref().map_or(Rect::auto(), |s| s.inset)
    }

    #[inline(always)]
    fn size(&self) -> Size<Dimension> {
        if let Some(s) = &self.extra {
            return s.size;
        }
        Size {
            width: unpack_dim(self.vals[IDX_SIZE_W], self.len_tags),
            height: unpack_dim(self.vals[IDX_SIZE_H], self.len_tags >> 2),
        }
    }

    #[inline(always)]
    fn min_size(&self) -> Size<LengthPercentageAuto> {
        if let Some(s) = &self.extra {
            return s.min_size;
        }
        Size {
            width: unpack_lpa(self.vals[IDX_MIN_W], self.len_tags >> 4),
            height: unpack_lpa(self.vals[IDX_MIN_H], self.len_tags >> 6),
        }
    }

    #[inline(always)]
    fn max_size(&self) -> Size<LengthPercentageAuto> {
        if let Some(s) = &self.extra {
            return s.max_size;
        }
        Size {
            width: unpack_lpa(self.vals[IDX_MAX_W], self.len_tags >> 8),
            height: unpack_lpa(self.vals[IDX_MAX_H], self.len_tags >> 10),
        }
    }

    #[inline(always)]
    fn aspect_ratio(&self) -> Option<f32> {
        self.extra.as_ref().and_then(|s| s.aspect_ratio)
    }

    #[inline(always)]
    fn margin(&self) -> Rect<LengthPercentageAuto> {
        self.extra.as_ref().map_or(Rect::zero(), |s| s.margin)
    }

    #[inline(always)]
    fn padding(&self) -> Rect<LengthPercentage> {
        if let Some(s) = &self.extra {
            return s.padding;
        }
        Rect {
            left: unpack_lp(self.vals[IDX_PAD_L], (self.len_tags & (1 << 12)) != 0),
            right: unpack_lp(self.vals[IDX_PAD_R], (self.len_tags & (1 << 13)) != 0),
            top: unpack_lp(self.vals[IDX_PAD_T], (self.len_tags & (1 << 14)) != 0),
            bottom: unpack_lp(self.vals[IDX_PAD_B], (self.len_tags & (1 << 15)) != 0),
        }
    }

    #[inline(always)]
    fn border(&self) -> Rect<LengthPercentage> {
        self.extra.as_ref().map_or(Rect::zero(), |s| s.border)
    }

    #[inline(always)]
    fn contain(&self) -> Contain {
        self.extra.as_ref().map_or(Contain::NONE, |s| s.contain)
    }
}

impl OofItemStyle for CompactStyle {
    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        unpack_align_self(self.align_self_u8)
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }

    #[inline(always)]
    fn is_table(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_table)
    }

    #[inline(always)]
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        self.extra
            .as_ref()
            .map_or(Line { start: GridPlacement::Auto, end: GridPlacement::Auto }, |s| s.grid_row.clone())
    }

    #[inline(always)]
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        self.extra
            .as_ref()
            .map_or(Line { start: GridPlacement::Auto, end: GridPlacement::Auto }, |s| s.grid_column.clone())
    }
}

impl FlexboxContainerStyle for CompactStyle {
    #[inline(always)]
    fn flex_direction(&self) -> FlexDirection {
        decode_dir(self.box_flags)
    }

    #[inline(always)]
    fn flex_wrap(&self) -> FlexWrap {
        decode_wrap(self.box_flags >> 2)
    }

    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> {
        if let Some(s) = &self.extra {
            return s.gap;
        }
        Size {
            width: unpack_lp((self.gap_w_bf16 as u32) << 16, (self.mode_flags & MODE_GAP_W_PCT) != 0),
            height: unpack_lp((self.gap_h_bf16 as u32) << 16, (self.mode_flags & MODE_GAP_H_PCT) != 0),
        }
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        unpack_align_content(self.align_content_u8)
    }

    #[inline(always)]
    fn align_items(&self) -> AlignItems {
        unpack_align_items(self.align_items_u8)
    }

    #[inline(always)]
    fn justify_content(&self) -> JustifyContent {
        unpack_align_content(self.justify_content_u8)
    }
}

impl FlexboxItemStyle for CompactStyle {
    #[inline(always)]
    fn flex_basis(&self) -> Dimension {
        if let Some(s) = &self.extra {
            return s.flex_basis;
        }
        if (self.mode_flags & MODE_BASIS_ZERO) != 0 {
            Dimension::ZERO
        } else {
            Dimension::AUTO
        }
    }

    #[inline(always)]
    fn flex_grow(&self) -> f32 {
        if let Some(s) = &self.extra {
            return s.flex_grow;
        }
        unpack_bf16(self.flex_grow_bf16)
    }

    #[inline(always)]
    fn flex_shrink(&self) -> f32 {
        if let Some(s) = &self.extra {
            return s.flex_shrink;
        }
        unpack_bf16(self.flex_shrink_bf16)
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        unpack_align_self(self.align_self_u8)
    }
}

impl GridContainerStyle for CompactStyle {
    type Repetition<'a>
        = <Style as GridContainerStyle>::Repetition<'a>
    where
        Self: 'a;

    type TemplateTrackList<'a>
        = <Style as GridContainerStyle>::TemplateTrackList<'a>
    where
        Self: 'a;

    type AutoTrackList<'a>
        = <Style as GridContainerStyle>::AutoTrackList<'a>
    where
        Self: 'a;

    type TemplateLineNames<'a>
        = <Style as GridContainerStyle>::TemplateLineNames<'a>
    where
        Self: 'a;

    type GridTemplateAreas<'a>
        = <Style as GridContainerStyle>::GridTemplateAreas<'a>
    where
        Self: 'a;

    #[inline(always)]
    fn grid_template_rows(&self) -> Option<Self::TemplateTrackList<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_rows(&**s))
    }

    #[inline(always)]
    fn grid_template_columns(&self) -> Option<Self::TemplateTrackList<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_columns(&**s))
    }

    #[inline(always)]
    fn grid_auto_rows(&self) -> Self::AutoTrackList<'_> {
        self.extra.as_ref().map_or_else(|| [].iter().copied(), |s| GridContainerStyle::grid_auto_rows(&**s))
    }

    #[inline(always)]
    fn grid_auto_columns(&self) -> Self::AutoTrackList<'_> {
        self.extra.as_ref().map_or_else(|| [].iter().copied(), |s| GridContainerStyle::grid_auto_columns(&**s))
    }

    #[inline(always)]
    fn grid_auto_flow(&self) -> GridAutoFlow {
        self.extra.as_ref().map_or(GridAutoFlow::Row, |s| s.grid_auto_flow)
    }

    #[inline(always)]
    fn gap(&self) -> Size<LengthPercentage> {
        FlexboxContainerStyle::gap(self)
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        unpack_align_content(self.align_content_u8)
    }

    #[inline(always)]
    fn justify_content(&self) -> JustifyContent {
        unpack_align_content(self.justify_content_u8)
    }

    #[inline(always)]
    fn align_items(&self) -> AlignItems {
        unpack_align_items(self.align_items_u8)
    }

    #[inline(always)]
    fn justify_items(&self) -> AlignItems {
        self.extra.as_ref().map_or(AlignItems::NORMAL, |s| s.justify_items)
    }

    #[inline(always)]
    fn grid_template_areas(&self) -> Option<Self::GridTemplateAreas<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_areas(&**s))
    }

    #[inline(always)]
    fn grid_template_area_row_count(&self) -> u16 {
        self.extra.as_ref().map_or(0, |s| GridContainerStyle::grid_template_area_row_count(&**s))
    }

    #[inline(always)]
    fn grid_template_area_column_count(&self) -> u16 {
        self.extra.as_ref().map_or(0, |s| GridContainerStyle::grid_template_area_column_count(&**s))
    }

    #[inline(always)]
    fn grid_template_column_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_column_names(&**s))
    }

    #[inline(always)]
    fn grid_template_row_names(&self) -> Option<Self::TemplateLineNames<'_>> {
        self.extra.as_ref().and_then(|s| GridContainerStyle::grid_template_row_names(&**s))
    }
}

impl GridItemStyle for CompactStyle {
    #[inline(always)]
    fn grid_row(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        OofItemStyle::grid_row(self)
    }

    #[inline(always)]
    fn grid_column(&self) -> Line<GridPlacement<Self::CustomIdent>> {
        OofItemStyle::grid_column(self)
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        unpack_align_self(self.align_self_u8)
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }
}

impl BlockContainerStyle for CompactStyle {
    #[inline(always)]
    fn text_align(&self) -> TextAlign {
        self.extra.as_ref().map_or(TextAlign::Auto, |s| s.text_align)
    }

    #[inline(always)]
    fn align_content(&self) -> AlignContent {
        unpack_align_content(self.align_content_u8)
    }

    #[inline(always)]
    fn justify_items(&self) -> AlignItems {
        self.extra.as_ref().map_or(AlignItems::NORMAL, |s| s.justify_items)
    }
}

impl BlockItemStyle for CompactStyle {
    #[inline(always)]
    fn is_table(&self) -> bool {
        self.extra.as_ref().is_some_and(|s| s.item_is_table)
    }

    #[inline(always)]
    fn align_self(&self) -> Option<AlignSelf> {
        unpack_align_self(self.align_self_u8)
    }

    #[inline(always)]
    fn justify_self(&self) -> Option<AlignSelf> {
        self.extra.as_ref().and_then(|s| s.justify_self)
    }
}
