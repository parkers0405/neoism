//! [`TextSpec`]: what a text element shows and how it lays out.
use crate::color::Color;
use crate::el::{Align, LinkFn, LinkSecondaryFn};
use crate::text::{Link, Span};
use crate::tok::TextStyle;

#[derive(Clone)]
pub struct TextSpec {
    pub content: String,
    pub style: TextStyle,
    pub color: Color,
    /// `color` came from [`El::color`](crate::El::color); without it the text takes the
    /// window's foreground ([`Cx::foreground`](crate::Cx::foreground)) when the view sets one.
    pub(crate) inked: bool,
    pub align: Align,
    pub spans: Vec<Span>,
    /// Wrap at the element's width (its height follows the text).
    pub wrap: bool,
    /// Streaming word reveal: words that arrive after mount fade in over this many seconds
    /// (negative: no reveal tracking).
    pub reveal: f32,
    /// One line, cut with an ellipsis when wider than the element.
    pub truncate: bool,
    /// Which part `truncate` cuts (see [`El::truncate_middle`](crate::El::truncate_middle)).
    pub(crate) elide: Elide,
    /// Wrapped lines shown before "…" ends the last (see [`El::max_lines`](crate::El::max_lines)); 0 for all.
    pub(crate) max_lines: usize,
    /// Fade the overflowing trailing edge rather than replacing text with an ellipsis.
    pub truncate_fade: bool,
    /// The `truncate_fade` edge's width in ems (see [`El::truncate_fade_width`](crate::El::truncate_fade_width)).
    pub(crate) fade_em: f32,
    /// Spread wrapped lines to fill the width (see [`El::justify`](crate::El::justify)).
    pub justify: bool,
    /// Plain live run. Layout is measured from the authored placeholder before latching it.
    pub live: Option<crate::LiveText>,
    pub(crate) links: Vec<Link>,
    pub(crate) on_link: Option<LinkFn>,
    pub(crate) on_link_secondary: Option<LinkSecondaryFn>,
    /// Measured as its slice at this byte of the run (see [`El::advance_in`](crate::El::advance_in)).
    pub advance_in: Option<(crate::text::Run, usize)>,
    /// See [`El::text_case`](crate::El::text_case); applied when the element is laid out.
    pub(crate) case: Option<crate::TextCase>,
    /// The authored string once `case` has replaced `content` with the drawn one.
    pub(crate) source: Option<Box<crate::text::CaseSource>>,
    /// See [`El::selectable`](crate::El::selectable).
    pub(crate) selectable: bool,
    /// See [`El::selection_color`](crate::El::selection_color).
    pub(crate) selection: Color,
    /// See [`El::uncopied`](crate::El::uncopied): byte ranges of `content`, sorted.
    pub(crate) uncopied: Vec<(u32, u32)>,
    /// See [`El::copy_breaks`](crate::El::copy_breaks): newlines before and after it.
    pub(crate) copy_breaks: [u8; 2],
    /// See [`El::copy_prefix`](crate::El::copy_prefix).
    pub(crate) copy_prefix: Option<std::rc::Rc<str>>,
    /// Inset border rings `(start, end, color, width)` on rounded span backgrounds.
    pub(crate) rings: Vec<(u32, u32, Color, f32)>,
    /// See [`El::span_radius`](crate::El::span_radius), in points.
    pub(crate) span_radius: f32,
    /// Strikethrough byte ranges `(start, end, color)` of `content`.
    pub(crate) strikes: Vec<(u32, u32, Option<Color>)>,
    /// Paint-only highlight byte ranges of `content`.
    pub(crate) highlights: Vec<crate::text::TextHighlight>,
    /// Per-paragraph [`Block`](crate::Block) attributes for `content`.
    pub(crate) blocks: Vec<crate::edit::Block>,
    /// Line indices (`> 0`) created by `Shift+Return` (`<br>`).
    pub(crate) soft_breaks: Vec<u32>,
    /// Visual palette for rich block and inline decorations.
    /// [`Mark::Pill`](crate::Mark::Pill) chips in `content`, sorted.
    pub(crate) pills: Vec<crate::text::PillChip>,
    /// See [`El::pill_colors`](crate::El::pill_colors).
    pub(crate) pill_colors: Option<(Color, Option<Color>)>,
    pub(crate) rich_style: crate::edit::RichStyle,
    pub(crate) redacted: bool,
    pub(crate) secure_revealed: bool,
    pub(crate) wrap_anywhere: bool,
    /// See [`El::exact_lines`](crate::El::exact_lines): the height is its lines' own, unrounded,
    /// unless [`El::rounded_lines`](crate::El::rounded_lines) rounds it to device pixels.
    pub(crate) exact_lines: bool,
    /// Lays out with a right-to-left paragraph direction: the element's resolved `El::dir`,
    /// set when it's flattened.
    pub(crate) flow: crate::el::Flow,
    /// The scale's bits and [`crate::text::shape_key`] at it, once the content is final (see
    /// [`TextSpec::key_shape`]).
    pub(crate) shape: Option<(u32, u64)>,
}

impl std::fmt::Debug for TextSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextSpec")
            .field("content", &if self.redacted { "[redacted]" } else { &self.content })
            .field("style", &self.style)
            .field("color", &self.color)
            .field("align", &self.align)
            .field("spans", &self.spans)
            .field("wrap", &self.wrap)
            .field("reveal", &self.reveal)
            .field("truncate", &self.truncate)
            .field("max_lines", &self.max_lines)
            .field("truncate_fade", &self.truncate_fade)
            .field("justify", &self.justify)
            .field("selectable", &self.selectable)
            .finish_non_exhaustive()
    }
}

impl Drop for TextSpec {
    fn drop(&mut self) {
        if self.redacted {
            use zeroize::Zeroize;
            self.content.zeroize();
            if let Some(src) = &mut self.source {
                src.text.zeroize();
            }
        }
    }
}

impl TextSpec {
    /// Hashes what shaping depends on once, so measuring and laying the text out at each width
    /// this commit reuses it. Secure text keeps no content-derived key.
    pub(crate) fn key_shape(&mut self, scale: f32) {
        self.shape = (!self.redacted).then(|| (scale.to_bits(), self.fresh_shape_key(scale)));
    }

    /// The key [`Self::key_shape`] kept at `scale`.
    pub(crate) fn shape_key(&self, scale: f32) -> Option<u64> {
        let (_, key) = self.shape.filter(|s| s.0 == scale.to_bits())?;
        debug_assert_eq!(key, self.fresh_shape_key(scale), "a text changed after its shape key was taken");
        Some(key)
    }

    /// [`crate::text::shape_key`] of this text in its direction.
    pub(crate) fn fresh_shape_key(&self, scale: f32) -> u64 {
        let key = crate::text::shape_key(&self.content, &self.spans, self.style, scale, self.wrap_anywhere, &self.pills);
        crate::text::directed(key, self.base())
    }

    /// The paragraph direction it lays out in.
    pub(crate) fn base(&self) -> parley::BaseDirection {
        self.flow.base()
    }

    /// Zeroizes the shown string and shows `content` instead.
    pub(crate) fn replace_content(&mut self, content: String) {
        use zeroize::Zeroize;
        self.content.zeroize();
        self.content = content;
        self.shape = None;
    }

    /// The string as authored, before [`El::text_case`](crate::El::text_case).
    pub(crate) fn source_text(&self) -> &str {
        self.source.as_ref().map_or(&self.content, |s| &s.text)
    }

    /// The authored text of `content[start..end]`.
    pub(crate) fn source_slice(&self, start: u32, end: u32) -> Option<&str> {
        match &self.source {
            None => self.content.get(start as usize..end as usize),
            Some(s) => s.text.get(s.source(start, false) as usize..s.source(end, true) as usize),
        }
    }

    /// The authored text of `content[start..end]` as copying takes it, without its
    /// [`El::uncopied`](crate::El::uncopied) bytes.
    pub(crate) fn copy_slice(&self, start: u32, end: u32) -> Option<String> {
        if self.redacted {
            return None;
        }
        let mut out = String::new();
        let mut at = start;
        for &(a, b) in self.uncopied.iter().filter(|r| r.1 > start && r.0 < end) {
            if a > at { out.push_str(self.source_slice(at, a)?); }
            at = at.max(b);
        }
        if at < end { out.push_str(self.source_slice(at, end)?); }
        Some(out)
    }

    /// Masks `self.content` in place and zeroizes the plain string (for layout of a hidden
    /// secure label or an exiting revealed one).
    pub(crate) fn conceal(&mut self) {
        if !self.redacted {
            return;
        }
        self.secure_revealed = false;
        self.shape = None;
        let mask = if self.content.is_empty() {
            String::new()
        } else {
            crate::edit::MULTILINE_SECURE_MASK.to_owned()
        };
        use zeroize::Zeroize;
        self.content.zeroize();
        self.content = mask;
        if let Some(mut src) = self.source.take() {
            src.text.zeroize();
        }
        self.case = None;
        self.spans.clear();
        self.links.clear();
        self.rings.clear();
        self.strikes.clear();
        self.blocks.clear();
        self.soft_breaks.clear();
        self.pills.clear();
        self.uncopied.clear();
        self.wrap = false;
        self.wrap_anywhere = false;
        self.max_lines = 0;
    }

    /// Replaces `content` (the authored string) with its cased form and moves span and link
    /// ranges onto it.
    pub(crate) fn apply_case(&mut self) {
        self.shape = None;
        if self.redacted {
            self.case = None;
            return;
        }
        // Ring boundaries need distinct brushes even when the surrounding styles match.
        // Do this once before shaping, not once per glyph run at paint time.
        if !self.rings.is_empty() {
            let mut spans = Vec::new();
            for span in &self.spans {
                let mut bounds = vec![span.start, span.end];
                for &(a, b, _, _) in &self.rings {
                    bounds.extend([a, b].into_iter().filter(|&p| span.start < p && p < span.end));
                }
                bounds.sort_unstable();
                bounds.dedup();
                spans.extend(bounds.windows(2).map(|w| Span { start: w[0], end: w[1], ..span.clone() }));
            }
            self.spans = spans;
        }
        let Some(case) = self.case else { return };
        let (shown, source) = crate::text::CaseSource::new(case, std::mem::take(&mut self.content));
        for span in &mut self.spans {
            (span.start, span.end) = (source.shown(span.start, false), source.shown(span.end, true));
        }
        for link in &mut self.links {
            (link.start, link.end) = (source.shown(link.start, false), source.shown(link.end, true));
        }
        for r in &mut self.uncopied {
            *r = (source.shown(r.0, false), source.shown(r.1, true));
        }
        for r in &mut self.rings {
            (r.0, r.1) = (source.shown(r.0, false), source.shown(r.1, true));
        }
        for r in &mut self.strikes {
            (r.0, r.1) = (source.shown(r.0, false), source.shown(r.1, true));
        }
        for h in &mut self.highlights {
            (h.start, h.end) = (source.shown(h.start, false), source.shown(h.end, true));
        }
        self.content = shown;
        self.source = Some(Box::new(source));
    }
}

/// The part of an overflowing `truncate`d text the ellipsis replaces.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Elide {
    /// [`El::truncate`](crate::El::truncate): keeps the start.
    #[default]
    End,
    /// [`El::truncate_middle`](crate::El::truncate_middle): keeps the start and the end.
    Middle,
    /// [`El::truncate_head`](crate::El::truncate_head): keeps the end.
    Head,
}
