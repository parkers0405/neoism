//! The entry points that lay out rich text: [`TextSystem::layout_rich`] and its aligned,
//! option, block-indented and per-spec variants, all over `layout_blocks_inner`.
use super::*;

/// How [`TextSystem::layout_rich_with`] lays out a paragraph. The default is
/// [`layout_rich`](TextSystem::layout_rich): start-aligned, breaking only between words.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RichLayoutOptions {
    align: Alignment,
    wrap_anywhere: bool,
    direction: parley::BaseDirection,
}

impl Default for RichLayoutOptions {
    fn default() -> Self {
        RichLayoutOptions { align: Alignment::Start, wrap_anywhere: false, direction: parley::BaseDirection::Ltr }
    }
}

impl RichLayoutOptions {
    pub fn new() -> Self {
        Self::default()
    }

    /// Aligns each line within the wrap width, as [`layout_aligned`](TextSystem::layout_aligned) does.
    pub fn align(mut self, align: Alignment) -> Self {
        self.align = align;
        self
    }

    /// Breaks inside a word that has no break opportunity and is wider than the wrap width
    /// (a long URL, hash or token) instead of letting it run past the line, as
    /// [`El::wrap_anywhere`](crate::El::wrap_anywhere) does. Words that fit still break at spaces.
    pub fn wrap_anywhere(mut self) -> Self {
        self.wrap_anywhere = true;
        self
    }

    /// The paragraph's direction (its bidi base level), as a text element inside
    /// [`El::dir`](crate::El::dir) lays out in: `Rtl` aligns `Start` to the right edge and gives
    /// neutral characters at its ends the right-to-left side, `Auto` takes the first strong
    /// character's. Left to right by default.
    pub fn direction(mut self, direction: crate::Direction) -> Self {
        self.direction = match direction {
            crate::Direction::Ltr => parley::BaseDirection::Ltr,
            crate::Direction::Rtl => parley::BaseDirection::Rtl,
            crate::Direction::Auto => parley::BaseDirection::Auto,
        };
        self
    }

    /// [`Self::direction`] of a resolved flow.
    pub(crate) fn flow(mut self, flow: crate::el::Flow) -> Self {
        self.direction = flow.base();
        self
    }
}

impl TextSystem {
    /// A layout in device pixels, wrapped at `max_width` device px when given.
    pub fn layout_rich(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
    ) -> Rc<Layout<Ink>> {
        self.layout_aligned(text, spans, style, scale, max_width, Alignment::Start)
    }

    /// [`layout_rich`](Self::layout_rich) with each line aligned within `max_width`, so wrapped
    /// centred or end-aligned text lines up per line rather than as one left-aligned block.
    pub fn layout_aligned(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        self.layout_blocks(text, spans, &[], &[], style, scale, max_width, align)
    }

    /// [`layout_rich`](Self::layout_rich) with [`RichLayoutOptions`]: line alignment, and
    /// wrapping inside a word wider than `max_width`.
    pub fn layout_rich_with(
        &mut self,
        text: &str,
        spans: &[Span],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        options: RichLayoutOptions,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, &[], &[], style, scale, max_width, options.align, options.wrap_anywhere, true, options.direction)
    }

    /// [`layout_aligned`](Self::layout_aligned) with per-paragraph [`Block`](crate::Block)
    /// indentation applied during line breaking, and room around each pill's label for its
    /// chip. A pill never wraps inside; one that starts a line starts its chip at the line's start.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_blocks(
        &mut self,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        self.layout_blocks_inner(text, spans, blocks, pills, style, scale, max_width, align, false, true)
    }

    /// [`layout_blocks`](Self::layout_blocks) with overflow-wrap, and out of the shared cache
    /// when `cache` is false.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_blocks_inner(
        &mut self,
        text: &str,
        spans: &[Span],
        blocks: &[crate::edit::Block],
        pills: &[PillChip],
        style: TextStyle,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        wrap_anywhere: bool,
        cache: bool,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, blocks, pills, style, scale, max_width, align, wrap_anywhere, cache, parley::BaseDirection::Ltr)
    }

    /// Secure display runs must not enter the shared layout cache or leave character
    /// scratch in the reusable shaping context. Layouts contain glyphs and byte ranges,
    /// not source strings; only the mounted node owns a secure run's layout.
    pub(crate) fn layout_for_spec(
        &mut self,
        spec: &crate::el::TextSpec,
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
    ) -> Rc<Layout<Ink>> {
        let shape = spec.shape_key(scale);
        self.layout_keyed(shape, &spec.content, &spec.spans, &spec.blocks, &spec.pills, spec.style, scale, max_width, align, spec.wrap_anywhere, !spec.redacted, spec.base())
    }

    /// [`Self::layout_blocks_inner`] of a cut of `spec`'s content, in its direction.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn layout_cut(
        &mut self,
        spec: &crate::el::TextSpec,
        text: &str,
        spans: &[Span],
        scale: f32,
        max_width: Option<f32>,
        align: Alignment,
        cache: bool,
    ) -> Rc<Layout<Ink>> {
        self.layout_keyed(None, text, spans, &spec.blocks, &[], spec.style, scale, max_width, align, spec.wrap_anywhere, cache, spec.base())
    }
}

#[cfg(test)]
#[path = "rich_layout_tests.rs"]
mod tests;
