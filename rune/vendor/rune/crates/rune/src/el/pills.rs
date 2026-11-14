//! `El::pill` and `El::pills`: rounded chips around byte ranges of static text, drawn and
//! wrapped like a [`text_input`](crate::text_input)'s [`Mark::Pill`](crate::Mark::Pill) chips.
use std::ops::Range;

use crate::Color;
use crate::el::{El, PillStyle};
use crate::text::{BareIcon, PillChip, Span};

impl El {
    /// Draws bytes `range` of a [`text`](crate::text) or [`rich`](crate::rich) element as a
    /// pill: a rounded chip in `style`'s colour (default: the rich style's link colour at 20%,
    /// or [`El::pill_colors`]) with its label in `style`'s text colour and its icon before it,
    /// the way a field draws a [`Mark::Pill`](crate::Mark::Pill). The label never splits across
    /// lines, and the chip's padding takes room in the line, so the text around it moves aside.
    /// Selection and copy treat the label as text. Ranges are authored UTF-8 bytes on char
    /// boundaries; empty, out-of-range or overlapping ones are skipped. On a field this does
    /// nothing: style its pills with [`El::pill_style`].
    ///
    /// ```ignore
    /// text("ask @reviewer to look").pill(4..13, PillStyle::new().bg(chip).fg(ink))
    /// ```
    pub fn pill(self, range: Range<usize>, style: PillStyle) -> Self {
        self.pills([(range, style)])
    }

    /// [`El::pill`] for several ranges.
    pub fn pills(mut self, pills: impl IntoIterator<Item = (Range<usize>, PillStyle)>) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        for (r, st) in pills {
            let ok = r.start < r.end && t.content.is_char_boundary(r.start) && t.content.is_char_boundary(r.end) && r.end <= t.content.len();
            let (start, end) = (r.start as u32, r.end as u32);
            if !ok || t.pills.iter().any(|p| p.start < end && start < p.end) {
                continue;
            }
            let at = t.pills.partition_point(|p| p.start < start);
            t.pills.insert(at, PillChip { start, end, icon: st.icon, bg: st.bg, fg: st.fg, bare: None });
        }
        t.spans = with_pill_colors(&t.spans, &t.pills);
        self
    }
}

impl El {
    /// Sets `icon` inline in a [`text`](crate::text) or [`rich`](crate::rich) element, before
    /// byte `at` (a char boundary), in room of its own that moves the text after it aside. It
    /// stays on the line of the word that follows it, as an icon before a link does on a web
    /// page, and sits centred on that word's letters. Its glyph takes `icon`'s colour or the
    /// element's; an image (a site's favicon) draws as it is. Selection and copy skip it. With no
    /// word after `at`, or over a pill, it draws nothing.
    ///
    /// ```ignore
    /// rich("see the docs", spans).inline_icon(8, InlineIcon::glyph(&GLOBE).size(14.0).gap(4.0))
    /// ```
    pub fn inline_icon(mut self, at: usize, icon: InlineIcon) -> Self {
        if self.cold.input.is_some() {
            return self;
        }
        let Some(t) = &mut self.text else { return self };
        if at >= t.content.len() || !t.content.is_char_boundary(at) {
            return self;
        }
        let word = t.content[at..].find(char::is_whitespace).unwrap_or(t.content.len() - at);
        let (start, end) = (at as u32, (at + word) as u32);
        if word == 0 || t.pills.iter().any(|p| p.start < end && start < p.end) {
            return self;
        }
        let at = t.pills.partition_point(|p| p.start < start);
        let InlineIcon { icon, image, size, gap, radius, color } = icon;
        let bare = Some(Box::new(BareIcon { size, gap, radius, image }));
        t.pills.insert(at, PillChip { start, end, icon, bg: None, fg: color, bare });
        self
    }
}

/// An icon set inline in text by [`El::inline_icon`]: a glyph or an image, `size` points square
/// (14 by default), `gap` points before the text after it (4).
#[derive(Clone, Debug)]
pub struct InlineIcon {
    icon: Option<&'static crate::Icon>,
    image: Option<crate::Pixels>,
    size: f32,
    gap: f32,
    radius: f32,
    color: Option<Color>,
}

impl InlineIcon {
    /// `icon`, drawn in [`Self::color`] or the element's text colour.
    pub fn glyph(icon: &'static crate::Icon) -> Self {
        Self { icon: Some(icon), image: None, size: 14.0, gap: 4.0, radius: 0.0, color: None }
    }
    /// An image (decoded pixels, such as a site's favicon), scaled to fit the square.
    pub fn image(pixels: crate::Pixels) -> Self {
        Self { icon: None, image: Some(pixels), size: 14.0, gap: 4.0, radius: 0.0, color: None }
    }
    /// Its side in points.
    pub fn size(mut self, points: f32) -> Self {
        self.size = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// Its room after it, before the text, in points.
    pub fn gap(mut self, points: f32) -> Self {
        self.gap = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// An image's corner radius in points.
    pub fn radius(mut self, points: f32) -> Self {
        self.radius = if points.is_finite() { points.max(0.0) } else { 0.0 };
        self
    }
    /// A glyph's colour.
    pub fn color(mut self, c: Color) -> Self {
        self.color = Some(c);
        self
    }
}

/// `spans` with each pill's label colour over it.
pub(crate) fn with_pill_colors(spans: &[Span], pills: &[PillChip]) -> Vec<Span> {
    if pills.iter().all(|p| p.fg.is_none()) {
        return crate::text::flatten_spans(spans);
    }
    let mut all = spans.to_vec();
    all.extend(pills.iter().filter_map(|p| Some(Span { start: p.start, end: p.end, color: Some(p.fg?), ..Span::default() })));
    crate::text::flatten_spans(&all)
}

#[cfg(test)]
mod tests;
