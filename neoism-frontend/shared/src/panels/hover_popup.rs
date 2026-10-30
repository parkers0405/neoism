//! Shared, measured Markdown LSP hover. Geometry and input use logical pixels.
use crate::panels::agent_pane::selection_model::{SelectableCaretStop, SelectableLine};
use crate::primitives::ide_theme::IdeTheme;
use crate::syntax::{highlight_line, syn_color, Lang, SynTok};
use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use std::ops::Range;
use neoism_paint::text::DrawOpts;
use crate::primitives::paint::Paint;
use unicode_segmentation::UnicodeSegmentation;

const DEPTH: f32 = 0.02;
const ORDER: u8 = 40;

pub struct HoverPopupLayout {
    pub anchor_x: f32,
    pub anchor_y: f32,
    pub cell_h: f32,
    pub window_w: f32,
    pub window_h: f32,
    pub scale: f32,
    /// Source language for unlabelled fences and indented examples.
    pub document_language: crate::syntax::Lang,
}

#[derive(Clone, Debug, Default, PartialEq)]
struct Style {
    bold: bool,
    italic: bool,
    code: bool,
    inline: bool,
    syntax: Option<SynTok>,
    link: Option<String>,
}
impl Style {
    fn padding(&self, chip_pad: f32) -> f32 {
        if self.inline {
            chip_pad
        } else {
            0.0
        }
    }
    fn opts(
        &self,
        font_size: f32,
        theme: &IdeTheme,
        prose_font: Option<usize>,
    ) -> DrawOpts {
        DrawOpts {
            font_id: if self.code { None } else { prose_font },
            font_size,
            bold: self.bold,
            italic: self.italic,
            color: if self.link.as_deref().is_some_and(safe_link) {
                theme.u8(theme.readable_accent(theme.blue))
            } else {
                self.syntax.map_or_else(
                    || theme.u8(theme.fg),
                    |token| syn_color(token, theme, false),
                )
            },
            ..DrawOpts::default()
        }
    }
}
#[derive(Clone, Debug)]
struct Span {
    range: Range<usize>,
    style: Style,
}
#[derive(Default)]
struct Document {
    text: String,
    spans: Vec<Span>,
    code: Vec<(Range<usize>, Lang)>,
    /// Canonical offsets with presentation-only space before the block.
    blocks: Vec<usize>,
}
impl Document {
    fn push(&mut self, text: &str, style: &Style) {
        let start = self.text.len();
        self.text.push_str(text);
        if !text.is_empty() {
            self.spans.push(Span {
                range: start..self.text.len(),
                style: style.clone(),
            });
        }
    }
    fn newline(&mut self) {
        if !self.text.is_empty() && !self.text.ends_with('\n') {
            self.text.push('\n');
        }
    }
}
fn safe_link(url: &str) -> bool {
    let lower = url.to_ascii_lowercase();
    (lower.starts_with("https://") || lower.starts_with("http://"))
        && url
            .split_once("://")
            .is_some_and(|(_, rest)| !rest.is_empty() && !rest.starts_with('/'))
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}
fn parse(lines: &[String], document_language: Lang) -> Document {
    let input = lines.join("\n");
    let mut doc = Document::default();
    let mut style = Style::default();
    let mut stack = Vec::new();
    let mut lists: Vec<Option<u64>> = Vec::new();
    let mut code = None;
    let mut code_start = 0;
    for event in Parser::new_ext(
        &input,
        Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TABLES,
    ) {
        match event {
            Event::Start(tag) => {
                if matches!(
                    tag,
                    Tag::Paragraph | Tag::Heading { .. } | Tag::CodeBlock(_)
                ) {
                    doc.newline();
                    if !doc.text.is_empty() {
                        doc.blocks.push(doc.text.len());
                    }
                }
                stack.push(style.clone());
                match tag {
                    Tag::List(start) => lists.push(start),
                    Tag::Heading { .. } => {
                        doc.newline();
                        style.bold = true;
                    }
                    Tag::Paragraph | Tag::Item | Tag::BlockQuote(_) => {
                        doc.newline();
                        if matches!(tag, Tag::Item) {
                            if let Some(Some(number)) = lists.last_mut() {
                                doc.push(&format!("{number}. "), &style);
                                *number = number.saturating_add(1);
                            } else {
                                doc.push("• ", &style);
                            }
                        }
                    }
                    Tag::Strong => style.bold = true,
                    Tag::Emphasis => style.italic = true,
                    Tag::Link { dest_url, .. } => {
                        style.link = safe_link(&dest_url).then(|| dest_url.to_string());
                    }
                    Tag::CodeBlock(kind) => {
                        doc.newline();
                        code_start = doc.text.len();
                        code = Some(match kind {
                            CodeBlockKind::Fenced(info) if !info.trim().is_empty() => {
                                lang_from_fence(&info)
                            }
                            _ => document_language,
                        });
                        style.code = true;
                    }
                    _ => {}
                }
            }
            Event::End(tag) => {
                if matches!(tag, TagEnd::List(_)) {
                    lists.pop();
                }
                if tag == TagEnd::CodeBlock {
                    if let Some(lang) = code.take() {
                        doc.code.push((code_start..doc.text.len(), lang));
                    }
                }
                if matches!(
                    tag,
                    TagEnd::Paragraph
                        | TagEnd::Heading(_)
                        | TagEnd::Item
                        | TagEnd::CodeBlock
                        | TagEnd::BlockQuote(_)
                ) {
                    doc.newline();
                }
                style = stack.pop().unwrap_or_default();
            }
            Event::Text(text) => doc.push(&text, &style),
            Event::Html(text) | Event::InlineHtml(text) => {
                // Hovers frequently contain unescaped generics such as Option<T>.
                // Render unsupported HTML literally, never interpret or execute it.
                let tag = text.trim().to_ascii_lowercase();
                if matches!(tag.as_str(), "<br>" | "<br/>" | "<br />") {
                    doc.text.push('\n');
                } else {
                    doc.push(&text, &style);
                }
            }
            Event::Code(text) => {
                let mut inline = style.clone();
                inline.code = true;
                inline.inline = true;
                doc.push(&text, &inline);
            }
            Event::SoftBreak => doc.push(" ", &style),
            Event::HardBreak | Event::Rule => {
                doc.text.push('\n');
            }
            Event::TaskListMarker(done) => {
                doc.push(if done { "☑ " } else { "☐ " }, &style)
            }
            _ => {}
        }
    }
    while doc.text.ends_with('\n') {
        doc.text.pop();
    }
    // Highlight original source lines once, before soft wrapping. Measure and
    // draw these same token runs (including shaping boundaries).
    doc.spans.retain(|span| {
        !doc.code.iter().any(|(range, _)| {
            span.range.start >= range.start && span.range.start < range.end
        })
    });
    for (range, lang) in &doc.code {
        let end = range.end.min(doc.text.len());
        if range.start >= end {
            continue;
        }
        let mut offset = range.start;
        for line in doc.text[range.start..end].split_inclusive('\n') {
            for (token, text) in highlight_line(line.trim_end_matches('\n'), *lang) {
                doc.spans.push(Span {
                    range: offset..offset + text.len(),
                    style: Style {
                        code: true,
                        syntax: Some(token),
                        ..Style::default()
                    },
                });
                offset += text.len();
            }
            if line.ends_with('\n') {
                offset += 1;
            }
        }
    }
    doc.spans.sort_by_key(|span| span.range.start);
    doc
}

#[derive(Clone)]
struct Piece {
    range: Range<usize>,
    style: Style,
    x: f32,
    width: f32,
}
struct Row {
    range: Range<usize>,
    line: SelectableLine,
    pieces: Vec<Piece>,
    code: Option<Lang>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct LayoutKey {
    font_size: f32,
    width: f32,
    line_h: f32,
    chip_pad: f32,
    prose_font: Option<usize>,
}

/// Cached document/layout and canonical (not soft-wrapped) selection offsets.
#[derive(Default)]
pub struct HoverPopupState {
    bounds: Option<[f32; 4]>,
    source: Option<[f32; 4]>,
    viewport: [f32; 4],
    input: Vec<String>,
    doc: Document,
    rows: Vec<Row>,
    language: Option<Lang>,
    key: Option<LayoutKey>,
    line_h: f32,
    scroll_y: f32,
    max_scroll: f32,
    anchor: Option<usize>,
    focus: Option<usize>,
    press: Option<(f32, f32, Option<String>)>,
    pointer: (f32, f32),
    moved: bool,
    activated: Option<String>,
}
impl HoverPopupState {
    pub fn bounds(&self) -> Option<[f32; 4]> { self.bounds }
    pub fn viewport(&self) -> [f32; 4] { self.viewport }
    pub fn scroll_y(&self) -> f32 { self.scroll_y }
    pub fn max_scroll(&self) -> f32 { self.max_scroll }
    /// A host font-library change must remeasure without losing selection.
    pub fn invalidate_layout(&mut self) { self.key = None; }
    fn set_document(&mut self, lines: &[String], language: Lang) {
        if self.input == lines && self.language == Some(language) {
            return;
        }
        self.input = lines.to_vec();
        self.language = Some(language);
        self.doc = parse(lines, language);
        self.key = None;
        self.anchor = None;
        self.focus = None;
        self.press = None;
        self.activated = None;
        self.scroll_y = 0.0;
    }
    pub fn contains(&self, x: f32, y: f32) -> bool {
        self.bounds.is_some_and(|r| contains(r, x, y))
    }
    pub fn retains_pointer(&self, x: f32, y: f32) -> bool {
        if self.is_dragging() || self.contains(x, y) {
            return true;
        }
        let (Some(source), Some([bx, by, bw, bh])) = (self.source, self.bounds) else {
            return false;
        };
        if contains(source, x, y) {
            return true;
        }
        let [sx, sy, sw, sh] = source;
        let (from, to) = if by >= sy + sh {
            (sy + sh, by)
        } else if by + bh <= sy {
            (sy, by + bh)
        } else {
            return false;
        };
        if (to - from).abs() < f32::EPSILON {
            return false;
        }
        let t = (y - from) / (to - from);
        (0.0..=1.0).contains(&t)
            && x >= sx + (bx - sx) * t
            && x <= sx + sw + (bx + bw - sx - sw) * t
    }
    /// A press is captured immediately, including before the drag threshold.
    pub fn is_dragging(&self) -> bool {
        self.press.is_some()
    }
    pub fn hide(&mut self) {
        self.bounds = None;
        self.source = None;
        self.viewport = [0.0; 4];
        self.press = None;
        self.activated = None;
        self.moved = false;
    }
    fn row_at(&self, y: f32) -> usize {
        let y = y - self.viewport[1] + self.scroll_y;
        self.rows
            .partition_point(|row| row.line.content_y <= y)
            .saturating_sub(1)
    }
    fn caret_at(&self, x: f32, y: f32) -> Option<usize> {
        let index = self.row_at(y);
        let row = self.rows.get(index)?;
        Some(row.range.start + row.line.caret_at_x(x - self.viewport[0]).byte_offset)
    }
    pub fn begin_selection(&mut self, x: f32, y: f32) -> bool {
        if !self.contains(x, y) {
            return false;
        }
        let link = self.link_at(x, y).map(str::to_owned);
        self.activated = None;
        self.anchor = self.caret_at(x, y);
        self.focus = self.anchor;
        self.press = Some((x, y, link));
        self.pointer = (x, y);
        self.moved = false;
        true
    }
    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = (x, y);
        if let Some((px, py, _)) = self.press {
            self.moved |= (x - px).hypot(y - py) >= 4.0;
            if self.moved {
                self.focus = self.caret_at(x, y);
            }
        }
        self.retains_pointer(x, y)
    }
    pub fn end_selection(&mut self) -> bool {
        let Some((_, _, link)) = self.press.take() else {
            return false;
        };
        if !self.moved && self.selection().is_none() {
            if let Some(link) = link {
                if self.link_at(self.pointer.0, self.pointer.1) == Some(link.as_str()) {
                    self.activated = Some(link);
                }
            }
        }
        true
    }
    fn selection(&self) -> Option<Range<usize>> {
        let (a, b) = (self.anchor?, self.focus?);
        (a != b).then_some(a.min(b)..a.max(b))
    }
    pub fn selected_text(&self) -> Option<String> {
        self.bounds?;
        self.doc.text.get(self.selection()?).map(str::to_owned)
    }
    pub fn link_at(&self, x: f32, y: f32) -> Option<&str> {
        if self.is_dragging() || !self.contains(x, y) || !contains(self.viewport, x, y) {
            return None;
        }
        let row = self.rows.get(self.row_at(y))?;
        let content_y = y - self.viewport[1] + self.scroll_y;
        if content_y < row.line.content_y || content_y >= row.line.content_y + self.line_h
        {
            return None;
        }
        let local = x - self.viewport[0];
        row.pieces
            .iter()
            .find(|p| local >= p.x && local < p.x + p.width)
            .and_then(|p| p.style.link.as_deref())
    }
    pub fn take_activated_link(&mut self) -> Option<String> {
        self.activated.take()
    }
    /// Consume wheel input inside the card, even when it has no overflow.
    pub fn scroll(&mut self, x: f32, y: f32, delta_y: f32) -> bool {
        let scrolling_press =
            self.press.is_some() && delta_y.is_finite() && delta_y != 0.0;
        // Latch the gesture, even at a scroll limit or if a later wheel event
        // returns to the original offset/href. A wheel gesture is not a click.
        self.moved |= scrolling_press;
        if !self.contains(x, y) {
            return false;
        }
        if delta_y.is_finite() {
            self.scroll_y = (self.scroll_y + delta_y).clamp(0.0, self.max_scroll);
        }
        if scrolling_press {
            self.pointer = (x, y);
            self.focus = self.caret_at(x, y);
        }
        true
    }
}
fn contains([x, y, w, h]: [f32; 4], mx: f32, my: f32) -> bool {
    w > 0.0 && h > 0.0 && mx >= x && mx < x + w && my >= y && my < y + h
}
fn intersect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x = a[0].max(b[0]);
    let y = a[1].max(b[1]);
    let w = (a[0] + a[2]).min(b[0] + b[2]) - x;
    let h = (a[1] + a[3]).min(b[1] + b[3]) - y;
    (w > 0.0 && h > 0.0).then_some([x, y, w, h])
}

/// Split at Unicode word boundaries, then at grapheme boundaries for oversized
/// tokens. Measurement is of the exact styled substrings that will be drawn.
fn wrap(
    doc: &Document,
    width: f32,
    mut measure: impl FnMut(&str, &Style) -> f32,
) -> Vec<Range<usize>> {
    fn measured(
        doc: &Document,
        a: usize,
        b: usize,
        measure: &mut impl FnMut(&str, &Style) -> f32,
    ) -> f32 {
        doc.spans
            .iter()
            .filter_map(|s| {
                let start = a.max(s.range.start);
                let end = b.min(s.range.end);
                (start < end).then(|| measure(&doc.text[start..end], &s.style))
            })
            .sum()
    }
    let mut out = Vec::new();
    let mut base = 0;
    for line in doc.text.split('\n') {
        let end = base + line.len();
        let mut start = base;
        let boundaries: Vec<_> = line
            .grapheme_indices(true)
            .map(|(i, g)| base + i + g.len())
            .collect();
        let mut first = 0;
        while start < end {
            let remaining = &doc.text[start..end];
            // Binary-search graphemes, avoiding quadratic measurement of long tokens.
            let mut low = first;
            let mut high = boundaries.len();
            while low < high {
                let mid = (low + high) / 2;
                if measured(doc, start, boundaries[mid], &mut measure) <= width {
                    low = mid + 1;
                } else {
                    high = mid;
                }
            }
            let fit = if low == first {
                boundaries[first]
            } else {
                boundaries[low - 1]
            };
            let mut stop = fit;
            if fit < end && low > first {
                if let Some(boundary) = remaining
                    .split_word_bound_indices()
                    .map(|(i, word)| start + i + word.len())
                    .take_while(|b| *b <= fit)
                    .filter(|b| *b > start)
                    .last()
                {
                    stop = boundary;
                }
            }
            out.push(start..stop);
            start = stop;
            first = boundaries.partition_point(|b| *b <= start);
        }
        if line.is_empty() {
            out.push(base..base);
        }
        base = end + 1;
    }
    out
}

fn build_rows(
    doc: &Document,
    ranges: Vec<Range<usize>>,
    line_h: f32,
    chip_pad: f32,
    mut prefixes: impl FnMut(&str, &Style) -> Vec<f32>,
) -> Vec<Row> {
    let mut next_y = 0.0;
    ranges
        .into_iter()
        .map(|range| {
            let mut x = 0.0;
            let mut pieces = Vec::new();
            let mut stops = vec![SelectableCaretStop {
                byte_offset: 0,
                x: 0.0,
            }];
            for span in &doc.spans {
                let a = range.start.max(span.range.start);
                let b = range.end.min(span.range.end);
                if a >= b {
                    continue;
                }
                let text = &doc.text[a..b];
                let pad = span.style.padding(chip_pad);
                let advances = prefixes(text, &span.style);
                // Padding is geometry, never canonical text. At a style boundary
                // move the existing caret to the chip's text edge.
                if let Some(stop) = stops.last_mut() {
                    if stop.byte_offset == a - range.start {
                        stop.x = x + pad;
                    }
                }
                let char_ends: Vec<_> = std::iter::once(0)
                    .chain(text.char_indices().map(|(i, c)| i + c.len_utf8()))
                    .collect();
                for (offset, g) in text.grapheme_indices(true) {
                    let end = offset + g.len();
                    if let Ok(i) = char_ends.binary_search(&end) {
                        stops.push(SelectableCaretStop {
                            byte_offset: a - range.start + end,
                            x: x + pad + advances[i],
                        });
                    }
                }
                let width = advances.last().copied().unwrap_or(0.0) + 2.0 * pad;
                pieces.push(Piece {
                    range: a..b,
                    style: span.style.clone(),
                    x,
                    width,
                });
                x += width;
            }
            let grapheme_ends: Vec<_> = doc.text[range.clone()]
                .grapheme_indices(true)
                .map(|(i, g)| i + g.len())
                .collect();
            stops.retain(|stop| {
                stop.byte_offset == 0
                    || grapheme_ends.binary_search(&stop.byte_offset).is_ok()
            });
            if doc.blocks.contains(&range.start) {
                next_y += line_h * 0.45;
            }
            let y = next_y;
            next_y += line_h;
            let code = doc
                .code
                .iter()
                .find(|(r, _)| range.start >= r.start && range.start < r.end)
                .map(|(_, lang)| *lang);
            Row {
                line: SelectableLine {
                    text: doc.text[range.clone()].into(),
                    rect: [0.0, y, x, line_h],
                    content_y: y,
                    caret_stops: stops.into(),
                    caret_origin_x: 0.0,
                },
                range,
                pieces,
                code,
            }
        })
        .collect()
}

fn placement(layout: &HoverPopupLayout, w: f32, h: f32, gap: f32) -> [f32; 4] {
    let ww = layout.window_w.max(0.0);
    let wh = layout.window_h.max(0.0);
    let w = w.clamp(0.0, ww);
    let h = h.clamp(0.0, wh);
    let below = layout.anchor_y + layout.cell_h + gap;
    let y = if below + h <= wh {
        below
    } else {
        layout.anchor_y - h - gap
    };
    [
        layout.anchor_x.clamp(0.0, ww - w),
        y.clamp(0.0, wh - h),
        w,
        h,
    ]
}

fn text_width(window_w: f32, pad: f32, scale: f32) -> f32 {
    // The cap includes the card padding, not just its text viewport.
    ((640.0 * scale).min(window_w.max(0.0)) - 2.0 * pad).max(0.0)
}

fn scroll_thumb(
    viewport: [f32; 4],
    pad: f32,
    scale: f32,
    scroll_y: f32,
    max_scroll: f32,
) -> Option<[f32; 4]> {
    let [x, y, w, h] = viewport;
    if max_scroll <= 0.0 || pad <= 0.0 || h <= 0.0 {
        return None;
    }
    let thumb_w = (3.0 * scale).min(pad * 0.5);
    let thumb_h = (h * h / (h + max_scroll)).max(12.0 * scale).min(h);
    Some([
        x + w + (pad - thumb_w) * 0.5,
        y + (h - thumb_h) * (scroll_y / max_scroll).clamp(0.0, 1.0),
        thumb_w,
        thumb_h,
    ])
}

/// Theme-relative opaque colors keep dark and light hover cards in the same hierarchy.
fn blended(theme: &IdeTheme, a: u32, b: u32, amount: f32) -> [f32; 4] {
    let a = theme.f32(a);
    let b = theme.f32(b);
    [
        a[0] + (b[0] - a[0]) * amount,
        a[1] + (b[1] - a[1]) * amount,
        a[2] + (b[2] - a[2]) * amount,
        1.0,
    ]
}

pub fn render(
    sugarloaf: &mut dyn Paint,
    lines: &[String],
    layout: HoverPopupLayout,
    theme: &IdeTheme,
    state: &mut HoverPopupState,
) {
    state.set_document(lines, layout.document_language);
    if state.doc.text.is_empty() || layout.window_w <= 0.0 || layout.window_h <= 0.0 {
        state.hide();
        return;
    }
    let s = layout.scale.clamp(0.5, 3.0);
    let font_size = (13.0 * s).clamp(10.0, 20.0);
    let line_h = font_size + 7.0 * s;
    let chip_pad = 4.0 * s;
    let prose_font =
        crate::primitives::prose_font::ui_prose_font_id(sugarloaf.font_library());
    let pad = (14.0 * s)
        .min(layout.window_w / 4.0)
        .min(layout.window_h / 4.0);
    let width = text_width(layout.window_w, pad, s);
    let key = LayoutKey {
        font_size,
        width,
        line_h,
        chip_pad,
        prose_font,
    };
    if state.key != Some(key) {
        let ranges = wrap(&state.doc, width, |text, style| {
            sugarloaf
                .overlay_text_mut()
                .measure(text, &style.opts(font_size, theme, prose_font))
                + 2.0 * style.padding(chip_pad)
        });
        state.rows = build_rows(&state.doc, ranges, line_h, chip_pad, |text, style| {
            sugarloaf
                .overlay_text_mut()
                .measure_char_prefixes(text, &style.opts(font_size, theme, prose_font))
        });
        state.key = Some(key);
        state.line_h = line_h;
    }
    let content_w = state
        .rows
        .iter()
        .map(|r| r.line.rect[2])
        .fold(0.0, f32::max)
        .min(width);
    let content_h = state
        .rows
        .last()
        .map_or(0.0, |row| row.line.content_y + line_h);
    let box_h = (content_h + pad * 2.0)
        .min((layout.window_h * 0.6).max(line_h + pad * 2.0))
        .min(layout.window_h);
    let bounds = placement(&layout, content_w + 2.0 * pad, box_h, 3.0 * s);
    let [bx, by, bw, bh] = bounds;
    state.bounds = Some(bounds);
    state.source = Some([
        layout.anchor_x - 4.0 * s,
        layout.anchor_y,
        layout.cell_h + 8.0 * s,
        layout.cell_h,
    ]);
    let clip = [
        bx + pad,
        by + pad,
        (bw - 2.0 * pad).max(0.0),
        (bh - 2.0 * pad).max(0.0),
    ];
    state.viewport = clip;
    state.max_scroll = (content_h - clip[3]).max(0.0);
    state.scroll_y = state.scroll_y.clamp(0.0, state.max_scroll);
    sugarloaf.overlay_rounded_rect(
        bx,
        by,
        bw,
        bh,
        theme.f32_alpha(theme.border, 1.0),
        DEPTH,
        8.0 * s,
        ORDER,
    );
    let border = s.min(bw / 2.0).min(bh / 2.0);
    sugarloaf.overlay_rounded_rect(
        bx + border,
        by + border,
        (bw - 2.0 * border).max(0.0),
        (bh - 2.0 * border).max(0.0),
        blended(theme, theme.bg, theme.surface, 0.65),
        DEPTH,
        (8.0 * s - border).max(0.0),
        ORDER,
    );
    if let Some([x, y, w, h]) =
        scroll_thumb(clip, pad, s, state.scroll_y, state.max_scroll)
    {
        sugarloaf.overlay_rounded_rect(
            x,
            y,
            w,
            h,
            theme.f32_alpha(theme.dim, 0.65),
            DEPTH,
            w * 0.5,
            ORDER + 1,
        );
    }
    let selection = state.selection();
    for row in &state.rows {
        let y = clip[1] + row.line.content_y - state.scroll_y;
        let Some(_row_clip) = intersect([clip[0], y, clip[2], line_h], clip) else {
            continue;
        };
        // Fenced examples share the panel surface: no row stripes or
        // translucent per-token backgrounds on top of them.
        if let Some(range) = &selection {
            let a = range.start.max(row.range.start);
            let b = range.end.min(row.range.end);
            if a < b {
                let x = clip[0] + row.line.caret_for_offset(a - row.range.start).x;
                let right = clip[0] + row.line.caret_for_offset(b - row.range.start).x;
                if let Some(r) = intersect([x, y, right - x, line_h], clip) {
                    sugarloaf.overlay_rounded_rect(
                        r[0],
                        r[1],
                        r[2],
                        r[3],
                        theme.f32_alpha(theme.accent, 0.35),
                        DEPTH,
                        0.0,
                        ORDER + 2,
                    );
                }
            }
        }
        {
            for piece in &row.pieces {
                let x = clip[0] + piece.x;
                if piece.style.inline && row.code.is_none() {
                    if let Some(r) =
                        intersect([x, y + s, piece.width, line_h - 2.0 * s], clip)
                    {
                        sugarloaf.overlay_rounded_rect(
                            r[0],
                            r[1],
                            r[2],
                            r[3],
                            blended(theme, theme.surface, theme.fg, 0.06),
                            DEPTH,
                            4.0 * s,
                            ORDER + 1,
                        );
                    }
                }
                if let Some(r) = link_underline(piece, x, y, line_h, s, chip_pad, clip) {
                    sugarloaf.overlay_rounded_rect(
                        r[0],
                        r[1],
                        r[2],
                        r[3],
                        theme.f32(theme.readable_accent(theme.blue)),
                        DEPTH,
                        0.0,
                        ORDER + 3,
                    );
                }
                let opts = DrawOpts {
                    clip_rect: Some(clip),
                    ..piece.style.opts(font_size, theme, prose_font)
                };
                sugarloaf.overlay_text_mut().draw(
                    x + piece.style.padding(chip_pad),
                    y,
                    &state.doc.text[piece.range.clone()],
                    &opts,
                );
            }
        }
    }
}

/// Geometry follows each measured, wrapped glyph run, not chip padding.
fn link_underline(
    piece: &Piece,
    x: f32,
    y: f32,
    line_h: f32,
    scale: f32,
    chip_pad: f32,
    clip: [f32; 4],
) -> Option<[f32; 4]> {
    if !piece.style.link.as_deref().is_some_and(safe_link) {
        return None;
    }
    let pad = piece.style.padding(chip_pad);
    intersect(
        [
            x + pad,
            y + line_h - 2.0 * scale,
            (piece.width - 2.0 * pad).max(0.0),
            scale,
        ],
        clip,
    )
}

fn lang_from_fence(label: &str) -> Lang {
    match label
        .split_whitespace()
        .next()
        .unwrap_or_default()
        .trim_start_matches('.')
        .to_ascii_lowercase()
        .as_str()
    {
        "rs" | "rust" => Lang::Rust,
        "js" | "javascript" | "mjs" | "cjs" => Lang::Javascript,
        "jsx" => Lang::Jsx,
        "ts" | "typescript" => Lang::Typescript,
        "tsx" => Lang::Tsx,
        "py" | "python" => Lang::Python,
        "go" | "golang" => Lang::Go,
        "lua" => Lang::Lua,
        "nix" => Lang::Nix,
        "toml" => Lang::Toml,
        "sql" => Lang::Sql,
        "json" | "jsonc" => Lang::Json,
        "md" | "markdown" => Lang::Markdown,
        _ => Lang::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(text: &str) -> Document {
        parse(&[text.into()], Lang::Other)
    }
    fn measured(text: &str, style: &Style) -> f32 {
        text.graphemes(true).count() as f32 * if style.bold { 12.0 } else { 10.0 }
            + 2.0 * style.padding(4.0)
    }
    // Deterministic shaper for interaction tests; production uses overlay
    // measure/measure_char_prefixes with exactly the drawing options.
    fn popup(text: &str, width: f32, height: f32) -> HoverPopupState {
        let doc = document(text);
        let ranges = wrap(&doc, width, measured);
        let rows = build_rows(&doc, ranges, 20.0, 4.0, |text, style| {
            std::iter::once(0.0)
                .chain(text.char_indices().map(|(i, c)| {
                    measured(&text[..i + c.len_utf8()], style) - 2.0 * style.padding(4.0)
                }))
                .collect()
        });
        let max_scroll =
            (rows.last().map_or(0.0, |r| r.line.content_y + 20.0) - height).max(0.0);
        HoverPopupState {
            bounds: Some([10.0, 30.0, width + 20.0, height + 20.0]),
            source: Some([20.0, 10.0, 20.0, 17.0]),
            viewport: [20.0, 40.0, width, height],
            line_h: 20.0,
            rows,
            doc,
            max_scroll,
            ..Default::default()
        }
    }

    #[test]
    fn reference_and_autolinks_only_style_resolved_safe_targets() {
        let doc = document("[Full][target] [Collapsed][] [Shortcut] <https://example.org/auto> [**bold** and `code`](https://example.org/nested) [missing] [bad](javascript:alert) [file](file:///tmp/a)\n\n[target]: https://example.org/full\n[Collapsed]: https://example.org/collapsed\n[Shortcut]: https://example.org/shortcut");
        for (label, url) in [
            ("Full", "https://example.org/full"),
            ("Collapsed", "https://example.org/collapsed"),
            ("Shortcut", "https://example.org/shortcut"),
            ("https://example.org/auto", "https://example.org/auto"),
            ("bold", "https://example.org/nested"),
            ("code", "https://example.org/nested"),
        ] {
            let offset = doc.text.find(label).unwrap();
            let span = doc
                .spans
                .iter()
                .find(|s| s.range.contains(&offset))
                .unwrap();
            assert_eq!(span.style.link.as_deref(), Some(url));
        }
        for label in ["missing", "bad", "file"] {
            let offset = doc.text.find(label).unwrap();
            assert!(doc
                .spans
                .iter()
                .filter(|s| s.range.contains(&offset))
                .all(|s| s.style.link.is_none()));
        }
        assert!(doc.text.contains("[missing]"));
        assert!(!doc.text.contains("[target]:"));
    }

    #[test]
    fn links_use_readable_blue_and_clipped_glyph_underlines_after_wrap() {
        let mut theme = IdeTheme::default();
        theme.accent = 0xffffff;
        let state = popup(
            "[**long label** and `code label`](https://example.org)",
            85.0,
            100.0,
        );
        assert!(state.rows.len() > 1);
        assert_eq!(state.doc.text, "long label and code label");
        let clip = [22.0, 42.0, 60.0, 35.0];
        let mut fragments = 0;
        for row in &state.rows {
            let y = 40.0 + row.line.content_y;
            for piece in &row.pieces {
                assert_eq!(
                    piece.style.opts(14.0, &theme, None).color,
                    theme.u8(theme.readable_accent(theme.blue))
                );
                if let Some(r) =
                    link_underline(piece, 20.0 + piece.x, y, 20.0, 1.0, 4.0, clip)
                {
                    fragments += 1;
                    assert!(r[0] >= clip[0] && r[1] >= clip[1]);
                    assert!(r[0] + r[2] <= clip[0] + clip[2]);
                    assert!(r[1] + r[3] <= clip[1] + clip[3]);
                    assert!(r[0] >= 20.0 + piece.x + piece.style.padding(4.0));
                }
            }
        }
        assert!(fragments > 0);
        let plain = Piece {
            range: 0..4,
            style: Style::default(),
            x: 0.0,
            width: 40.0,
        };
        assert!(link_underline(&plain, 20.0, 40.0, 20.0, 1.0, 4.0, clip).is_none());
    }

    #[test]
    fn hidden_selection_cannot_supply_explicit_copy() {
        let mut state = popup("selected documentation", 240.0, 100.0);
        state.anchor = Some(0);
        state.focus = Some(8);
        assert_eq!(state.selected_text().as_deref(), Some("selected"));
        state.hide();
        assert_eq!(state.selected_text(), None);
    }

    #[test]
    fn nix_fences_and_source_language_defaults_use_existing_tokens() {
        let doc = parse(
            &["```nix\n# a comment\nlet x = \"value\"; in x\n```".into()],
            Lang::Rust,
        );
        assert_eq!(doc.code[0].1, Lang::Nix);
        let token = |needle: &str| {
            let offset = doc.text.find(needle).unwrap();
            doc.spans
                .iter()
                .find(|s| s.range.contains(&offset))
                .unwrap()
                .style
                .syntax
                .unwrap()
        };
        assert_eq!(token("#"), SynTok::Comment);
        assert_eq!(token("let"), SynTok::Keyword);
        assert_eq!(token("value"), SynTok::String);
        let theme = IdeTheme::default();
        assert_eq!(
            syn_color(token("#"), &theme, false),
            theme.u8(theme.syn_comment)
        );
        assert_ne!(
            syn_color(token("let"), &theme, false),
            syn_color(token("value"), &theme, false)
        );
        for source in ["```\nlet x = 1; in x\n```", "    let x = 1; in x"] {
            assert_eq!(parse(&[source.into()], Lang::Nix).code[0].1, Lang::Nix);
        }
        for label in ["plaintext", "text", "unknown"] {
            let source = format!("```{label}\nlet x = 1;\n```");
            assert_eq!(parse(&[source], Lang::Nix).code[0].1, Lang::Other);
        }
    }

    #[test]
    fn prose_font_role_keeps_emphasis_and_code_monospace() {
        let theme = IdeTheme::default();
        let doc =
            document("normal **bold** _italic_ `code`\n\n```nix\nlet x = 1; in x\n```");
        for span in &doc.spans {
            let opts = span.style.opts(13.0, &theme, Some(17));
            assert_eq!(opts.font_id, if span.style.code { None } else { Some(17) });
            assert_eq!(opts.bold, span.style.bold);
            assert_eq!(opts.italic, span.style.italic);
        }
        assert!(!doc.spans[0].style.opts(13.0, &theme, Some(17)).bold);
    }

    #[test]
    fn chip_padding_is_measured_and_hit_tested_without_changing_copy() {
        let mut state = popup("a[`bc`](https://example.com),d", 200.0, 40.0);
        assert_eq!(state.doc.text, "abc,d");
        let row = &state.rows[0];
        assert_eq!(row.pieces[1].x, 10.0);
        assert_eq!(row.pieces[1].width, 28.0);
        assert_eq!(row.pieces[2].x, 38.0);
        assert_eq!(row.line.rect[2], 58.0);
        assert_eq!(row.line.caret_for_offset(1).x, 14.0);
        assert_eq!(row.line.caret_for_offset(3).x, 38.0);
        assert_eq!(state.link_at(31.0, 45.0), Some("https://example.com"));
        assert_eq!(state.link_at(57.0, 45.0), Some("https://example.com"));
        assert_eq!(state.link_at(59.0, 45.0), None);
        state.begin_selection(20.0, 45.0);
        state.pointer_move(500.0, 45.0);
        assert_eq!(state.selected_text().as_deref(), Some("abc,d"));
        let wrapped = popup("a[`bc`](https://example.com),d", 30.0, 80.0);
        assert!(wrapped.rows.len() > 1);
        assert!(wrapped.rows.iter().all(|row| row.line.rect[2] <= 30.0));
        state.rows = wrapped.rows;
        assert_eq!(state.selected_text().as_deref(), Some("abc,d"));
    }

    #[test]
    fn paragraph_gaps_only_at_block_boundaries_and_spaces_are_canonical() {
        let state = popup("abcdefgh\n\nsecond", 40.0, 200.0);
        assert_eq!(state.rows[0].line.content_y, 0.0);
        assert_eq!(state.rows[1].line.content_y, 20.0);
        assert_eq!(state.rows[2].line.content_y, 49.0);
        assert_eq!(state.rows[3].line.content_y, 69.0);
        assert_eq!(state.doc.text, "abcdefgh\nsecond");
        let doc = document("before  `x`,after!\n\n```nix\n  let x = 1;\n    # indent\n```\n\n    indented\n      nested");
        assert_eq!(
            doc.text,
            "before  x,after!\n  let x = 1;\n    # indent\nindented\n  nested"
        );
        let ranges = wrap(&doc, 40.0, measured);
        for (source, _) in &doc.code {
            assert!(doc
                .text
                .get(source.start..source.end.min(doc.text.len()))
                .is_some());
        }
        assert!(ranges.len() > 4);
    }

    #[test]
    fn language_and_resolved_font_metrics_invalidate_caches() {
        let mut state = HoverPopupState::default();
        let lines = vec!["```\n# comment\n```".into()];
        state.set_document(&lines, Lang::Other);
        let key = LayoutKey {
            font_size: 13.0,
            width: 200.0,
            line_h: 20.0,
            chip_pad: 4.0,
            prose_font: Some(3),
        };
        state.key = Some(key);
        state.set_document(&lines, Lang::Other);
        assert_eq!(state.key, Some(key));
        state.set_document(&lines, Lang::Nix);
        assert_eq!(state.key, None);
        assert_eq!(state.doc.code[0].1, Lang::Nix);
        assert_ne!(
            key,
            LayoutKey {
                prose_font: Some(4),
                ..key
            }
        );
        assert_ne!(
            key,
            LayoutKey {
                font_size: 14.0,
                ..key
            }
        );
        assert_ne!(
            key,
            LayoutKey {
                width: 180.0,
                ..key
            }
        );
        assert_ne!(
            key,
            LayoutKey {
                line_h: 22.0,
                ..key
            }
        );
        assert_ne!(
            key,
            LayoutKey {
                chip_pad: 5.0,
                ..key
            }
        );
    }

    #[test]
    fn rich_markdown_labels_nested_markup_and_code() {
        let doc = document("# Title\n\n[**bold _inner_** and `code`](https://example.com)\n\n`*literal*`\n\n```rust\nlet x = 1;\n```\n\n[bad](javascript:alert)");
        assert_eq!(
            doc.text,
            "Title\nbold inner and code\n*literal*\nlet x = 1;\nbad"
        );
        assert!(doc.spans.iter().any(|s| s.style.bold
            && s.style.italic
            && s.style.link.as_deref() == Some("https://example.com")));
        assert!(doc
            .spans
            .iter()
            .any(|s| s.style.code && s.style.link.is_some()));
        assert!(doc.spans.iter().any(|s| s.style.syntax.is_some()));
        assert!(!doc.spans.iter().any(|s| s
            .style
            .link
            .as_deref()
            .is_some_and(|link| link.starts_with("javascript:"))));
        assert!(matches!(doc.code[0].1, Lang::Rust));
    }

    #[test]
    fn literal_html_and_generic_types_are_not_discarded() {
        assert_eq!(
            document("Expected Option<T>, found Result<U, E>.").text,
            "Expected Option<T>, found Result<U, E>."
        );
        assert_eq!(
            document("literal <span>value</span> and <img src=x>").text,
            "literal <span>value</span> and <img src=x>"
        );
        assert_eq!(
            document("<script>alert('literal')</script>").text,
            "<script>alert('literal')</script>"
        );
        assert_eq!(
            document("before<BR />after<br>last").text,
            "before\nafter\nlast"
        );
    }

    #[test]
    fn scrolling_a_link_press_cancels_activation_even_after_scrolling_back() {
        let text = "[label](https://example.com)\n\n[label](https://example.com)\n\n[label](https://example.com)";
        let mut state = popup(text, 100.0, 20.0);
        state.begin_selection(25.0, 45.0);
        assert!(state.scroll(25.0, 45.0, 29.0));
        assert!(state.focus.is_some_and(|offset| offset >= 6));
        state.pointer_move(25.0, 45.0);
        state.end_selection();
        assert_eq!(state.take_activated_link(), None);

        state.scroll(25.0, 45.0, -29.0);
        state.begin_selection(25.0, 45.0);
        state.scroll(25.0, 45.0, 29.0);
        state.scroll(25.0, 45.0, -29.0);
        state.pointer_move(25.0, 45.0);
        state.end_selection();
        assert_eq!(state.take_activated_link(), None);

        // Wheel gestures at the limit or without overflow also cancel clicks.
        let mut state = popup("[label](https://example.com)", 100.0, 40.0);
        for delta in [-20.0, 20.0] {
            state.begin_selection(25.0, 45.0);
            state.scroll(25.0, 45.0, delta);
            state.end_selection();
            assert_eq!(state.take_activated_link(), None);
        }
        state.begin_selection(25.0, 45.0);
        state.scroll(25.0, 45.0, 0.0);
        state.end_selection();
        assert_eq!(
            state.take_activated_link().as_deref(),
            Some("https://example.com")
        );
    }

    #[test]
    fn outer_width_cap_and_scroll_indicator_stay_in_bounds() {
        for scale in [0.5, 1.0, 3.0] {
            for window_w in [1.0_f32, 17.0, 800.0, 3000.0] {
                let pad = (14.0_f32 * scale).min(window_w / 4.0);
                let width = text_width(window_w, pad, scale);
                let outer = width + 2.0 * pad;
                assert!(outer <= (640.0 * scale).min(window_w));
                let viewport = [pad, pad, width, 100.0];
                let top = scroll_thumb(viewport, pad, scale, 0.0, 900.0).unwrap();
                let bottom = scroll_thumb(viewport, pad, scale, 900.0, 900.0).unwrap();
                for [x, y, w, h] in [top, bottom] {
                    assert!(x >= pad + width && x + w <= outer);
                    assert!(y >= pad && y + h <= pad + 100.0);
                }
                assert_eq!(top[1], pad);
                assert_eq!(bottom[1] + bottom[3], pad + 100.0);
                assert!(scroll_thumb(viewport, pad, scale, 0.0, 0.0).is_none());
            }
        }
    }

    #[test]
    fn ordered_lists_keep_their_numbers_and_nested_list_counters() {
        let doc = document("3. third\n   - nested\n4. fourth\n\n- plain\n- next");
        assert_eq!(doc.text, "3. third\n• nested\n4. fourth\n• plain\n• next");
    }

    #[test]
    fn empty_fences_and_unterminated_code_are_safe() {
        assert_eq!(document("before\n\n```rust\n```").text, "before");
        assert!(document("```rust\n```").text.is_empty());
        let doc = document("```rust\nlet x = \"abc\";\nlet y = 2;");
        assert_eq!(doc.text, "let x = \"abc\";\nlet y = 2;");
        assert!(doc.spans.iter().any(|span| span.style.syntax.is_some()));
        for span in &doc.spans {
            assert!(doc.text.get(span.range.clone()).is_some());
        }
    }

    #[test]
    fn wrapping_respects_style_width_and_graphemes_without_ellipsis() {
        let doc = document("**wide words** éé👨‍👩‍👧‍👦e\u{301}abcdef");
        let rows = wrap(&doc, 48.0, measured);
        assert!(rows.len() > 3);
        assert_eq!(
            rows.iter()
                .map(|r| &doc.text[r.clone()])
                .collect::<String>(),
            doc.text
        );
        let boundaries = doc
            .text
            .grapheme_indices(true)
            .map(|(i, _)| i)
            .chain(std::iter::once(doc.text.len()))
            .collect::<Vec<_>>();
        for r in rows {
            assert!(boundaries.contains(&r.start) && boundaries.contains(&r.end));
            let w: f32 = doc
                .spans
                .iter()
                .filter_map(|span| {
                    let a = span.range.start.max(r.start);
                    let b = span.range.end.min(r.end);
                    (a < b).then(|| measured(&doc.text[a..b], &span.style))
                })
                .sum();
            assert!(w <= 48.0, "{w}");
        }
        // An indivisible glyph wider than the viewport is retained and clipped.
        let doc = document("👨‍👩‍👧‍👦");
        assert_eq!(wrap(&doc, 1.0, measured), vec![0..doc.text.len()]);
    }

    #[test]
    fn copy_softwrap_and_keep_selection_across_scroll_and_reflow() {
        let mut state = popup("abcdefghij\n\nsecond", 40.0, 40.0);
        assert!(state.begin_selection(20.0, 45.0));
        state.pointer_move(500.0, 500.0);
        assert_eq!(state.selected_text().as_deref(), Some("abcdefghij\nsecond"));
        state.end_selection();
        assert!(state.scroll(25.0, 45.0, 20.0));
        let replacement = popup("abcdefghij\n\nsecond", 80.0, 40.0);
        state.rows = replacement.rows;
        assert_eq!(state.selected_text().as_deref(), Some("abcdefghij\nsecond"));
        state.hide();
        assert_eq!(state.selected_text(), None);
    }

    #[test]
    fn measured_unicode_selection_and_captured_outside_drag() {
        let mut state = popup("aé中\n\nsecond", 100.0, 40.0);
        assert!(state.begin_selection(30.0, 45.0));
        state.pointer_move(50.0, 45.0);
        assert_eq!(state.selected_text().as_deref(), Some("é中"));
        assert!(state.end_selection());
        assert!(!state.is_dragging());
        state.begin_selection(50.0, 74.0);
        assert!(state.pointer_move(-50.0, -50.0));
        assert_eq!(state.selected_text().as_deref(), Some("aé中\nsec"));
        state.end_selection();
        assert!(!state.pointer_move(-50.0, -50.0));
    }

    #[test]
    fn source_gap_retention_above_and_below() {
        let mut state = popup("text", 100.0, 40.0);
        assert!(state.pointer_move(20.0, 28.0));
        assert!(state.pointer_move(60.0, 29.0));
        assert!(!state.pointer_move(150.0, 40.0));
        assert!(!state.pointer_move(100.0, 15.0));
        state.bounds = Some([10.0, 10.0, 120.0, 60.0]);
        state.source = Some([20.0, 73.0, 20.0, 20.0]);
        assert!(state.pointer_move(20.0, 72.0));
    }

    #[test]
    fn links_click_jitter_drag_back_outside_and_hide() {
        let mut state = popup("[label](https://example.com)", 100.0, 40.0);
        assert_eq!(state.link_at(25.0, 45.0), Some("https://example.com"));
        state.begin_selection(25.0, 45.0);
        assert_eq!(state.link_at(25.0, 45.0), None);
        state.pointer_move(27.0, 45.0);
        state.end_selection();
        assert_eq!(
            state.take_activated_link().as_deref(),
            Some("https://example.com")
        );
        assert_eq!(state.take_activated_link(), None);
        state.begin_selection(25.0, 45.0);
        state.pointer_move(60.0, 45.0);
        state.pointer_move(25.0, 45.0);
        state.end_selection();
        assert_eq!(state.take_activated_link(), None);
        state.begin_selection(25.0, 45.0);
        state.pointer_move(0.0, 0.0);
        state.end_selection();
        assert_eq!(state.take_activated_link(), None);
        state.begin_selection(25.0, 45.0);
        state.end_selection();
        state.hide();
        assert_eq!(state.take_activated_link(), None);
        assert_eq!(state.link_at(25.0, 45.0), None);
        assert!(!state.end_selection());
    }

    #[test]
    fn bounded_scroll_and_clipped_links_with_no_line_cutoff() {
        let text = (0..40)
            .map(|i| format!("[line{i}](https://example.com/{i})\n\n"))
            .collect::<String>();
        let mut state = popup(&text, 100.0, 25.0);
        assert_eq!(state.rows.len(), 40);
        // The paragraph gap is not a link hit target; the next row is clipped.
        assert_eq!(state.link_at(25.0, 64.0), None);
        assert_eq!(state.link_at(25.0, 66.0), None);
        assert!(state.scroll(25.0, 45.0, 29.0));
        assert_eq!(state.link_at(25.0, 45.0), Some("https://example.com/1"));
        assert!(state.scroll(25.0, 45.0, f32::MAX));
        assert_eq!(state.scroll_y, state.max_scroll);
        assert!(state.scroll(25.0, 45.0, -f32::MAX));
        assert_eq!(state.scroll_y, 0.0);
        assert!(!state.scroll(0.0, 0.0, 20.0));
        let mut short = popup("short", 100.0, 40.0);
        assert!(short.scroll(25.0, 45.0, 20.0));
        assert_eq!(short.scroll_y, 0.0);
    }

    #[test]
    fn viewport_clamps_tiny_negative_right_edge_and_scaled_cards() {
        for (ww, wh) in [(1.0, 1.0), (17.0, 9.0), (800.0, 600.0), (0.0, 0.0)] {
            for scale in [0.5, 1.0, 3.0] {
                for (x, y) in [(-100.0, -100.0), (1000.0, 1000.0), (ww - 1.0, wh - 1.0)] {
                    let layout = HoverPopupLayout {
                        anchor_x: x,
                        anchor_y: y,
                        cell_h: 20.0 * scale,
                        window_w: ww,
                        window_h: wh,
                        scale,
                        document_language: Lang::Other,
                    };
                    let [bx, by, bw, bh] =
                        placement(&layout, 640.0 * scale, 200.0 * scale, 3.0 * scale);
                    assert!(bx >= 0.0 && by >= 0.0 && bx + bw <= ww && by + bh <= wh);
                }
            }
        }
        assert_eq!(
            intersect([0.0, 0.0, 40.0, 40.0], [10.0, 20.0, 5.0, 5.0]),
            Some([10.0, 20.0, 5.0, 5.0])
        );
    }
}
