use super::*;

pub(super) trait CeilTo {
    fn ceil_to(self, scale: f32) -> f32;
}

impl CeilTo for f32 {
    /// Rounds up to whole device pixels.
    fn ceil_to(self, scale: f32) -> f32 {
        (self * scale - 1e-3).ceil() / scale
    }
}

/// How far a single-line field's text shifts left (device px) so its caret stays inside
/// `visible` device px. Unfocused fields rest at their start, like a native text field.
pub(super) fn single_line_scroll(scrolls: &mut HashMap<String, f32>, path: &str, inp: &crate::el::InputSpec, content: &str, layout: &Layout<Ink>, s: f32, visible: f32) -> f32 {
    if !inp.edit.focused {
        scrolls.remove(path);
        return 0.0;
    }
    let scroll = single_line_offset(scrolls.get(path).copied().unwrap_or(0.0), inp, content, layout, s, visible);
    scrolls.insert(path.to_owned(), scroll / s);
    scroll
}

/// [`single_line_scroll`] for a focused field, from its previous shift `prev` in points.
pub(super) fn single_line_offset(prev: f32, inp: &crate::el::InputSpec, content: &str, layout: &Layout<Ink>, s: f32, visible: f32) -> f32 {
    let caret_w = (1.5 * s).round();
    let caret = Cursor::from_byte_index(layout, caret_index(&inp.edit, content), Affinity::Downstream);
    let cx = crate::text::cursor_geometry(&caret, layout, 1.5 * s).x0 as f32;
    // A right-to-left field runs from its right edge: measure from there and shift rightward.
    let (cx, sign) = match crate::el::Flow::of_layout(layout) {
        crate::el::Flow::Ltr => (cx, 1.0),
        crate::el::Flow::Rtl => (layout.full_width() - cx, -1.0),
    };
    let mut scroll = prev * s * sign;
    if cx - scroll > visible - caret_w {
        scroll = cx - visible + caret_w;
    }
    sign * scroll.min(cx).clamp(0.0, (layout.full_width() + caret_w - visible).max(0.0))
}

/// The display byte index the caret sits at, inside any IME preedit.
pub(super) fn caret_index(e: &crate::edit::TextEdit, content: &str) -> usize {
    match e.preedit() {
        Some((p, Some((_, c)))) => (e.range().0 + c.min(p.len())).min(content.len()),
        Some((p, None)) => (e.range().0 + p.len()).min(content.len()),
        None => e.display_index(e.focus).min(content.len()),
    }
}

/// Parley line alignment for a text node: only wrapped text aligns per line.
pub(super) fn line_alignment(spec: &TextSpec) -> parley::Alignment {
    match spec.align {
        _ if spec.wrap && spec.justify => parley::Alignment::Justify,
        Align::Center if spec.wrap => parley::Alignment::Center,
        Align::End if spec.wrap => parley::Alignment::End,
        _ => parley::Alignment::Start,
    }
}

/// The rectangle a text node's laid-out lines cover inside its drawn box `r` (logical points),
/// and the layout origin its glyphs are placed from. Lines aligned inside a wrap width count
/// from their own offsets, so the rect hugs the text rather than the box.
pub(super) fn text_extent(n: &Node, spec: &TextSpec, layout: &Layout<Ink>, r: [f32; 4], scale: f32) -> (MRect, [f32; 2]) {
    let (x0, x1) = (r[0] + n.inset[0], r[0] + r[2] - n.inset[2]);
    let (lo, hi) = layout.lines().map(|l| (l.metrics().offset, l.metrics().advance))
        .fold((f32::MAX, 0.0f32), |(lo, hi), (offset, advance)| (lo.min(offset), hi.max(offset + advance)));
    let (lo, hi) = if lo > hi { (0.0, layout.full_width()) } else { (lo, hi) };
    let origin = [text_x(spec, x0, x1, layout.full_width() / scale, crate::el::Flow::of_layout(layout)), r[1] + n.inset[1]];
    let x = origin[0] + lo / scale;
    (MRect::new(x, origin[1], ((hi - lo) / scale).min(x1 - x).max(0.0),
        (crate::text::spec_height(layout, spec.style, scale, spec.exact_lines) / scale).min(r[3] - n.inset[1] - n.inset[3])), origin)
}

/// Where a `width`-wide text line starts between `x0` and `x1`. Wrapped text aligns each line
/// inside the wrap width itself, so it starts at `x0`. A right-to-left paragraph (`flow`, its
/// layout's base level) starts at its right edge; wrapped, its lines already end there.
pub(super) fn text_x(spec: &TextSpec, x0: f32, x1: f32, width: f32, flow: crate::el::Flow) -> f32 {
    use crate::el::Flow;
    if line_alignment(spec) != parley::Alignment::Start || (flow == Flow::Rtl && spec.wrap) {
        return x0;
    }
    match (spec.align, flow) {
        (Align::Start, Flow::Ltr) | (Align::End, Flow::Rtl) => x0,
        (Align::Center, _) => x0 + (x1 - x0 - width) * 0.5,
        (Align::End, Flow::Ltr) | (Align::Start, Flow::Rtl) => x1 - width,
    }
}

impl Node {
    /// Whether the text shows less than its whole content: cut by `El::truncate` or a line
    /// clamp, or running past its box under `El::truncate_fade`. A fade keeps the whole
    /// content in its layout, so it has no [`Elision`]; it only reads as cut.
    pub(crate) fn text_cut(&self, scale: f32) -> bool {
        self.elided.is_some()
            || self.text.as_ref().is_some_and(|t| t.truncate_fade) && self.run_layout.is_none()
                && self.text_layout.as_ref().is_some_and(|l| l.full_width() > (self.local.w - self.inset[0] - self.inset[2]).max(0.0) * scale + 0.5)
    }
}

/// Where truncation cut a text: it draws `content[..head]`, "…", then `content[tail..]`.
/// Selections and links stay in content bytes and map through this onto the drawn layout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Elision {
    pub(super) head: usize,
    pub(super) tail: usize,
}

impl Elision {
    pub(super) const ELLIPSIS: &str = "…";

    /// The drawn byte just past the ellipsis.
    pub(super) fn after(self) -> usize { self.head + Self::ELLIPSIS.len() }

    /// The content byte at drawn byte `i`. Past the ellipsis is the end of what it hides.
    pub(crate) fn content(self, i: usize) -> usize {
        if i <= self.head { i } else if i < self.after() { self.tail } else { self.tail + (i - self.after()) }
    }

    /// The drawn byte at content byte `i`. A hidden byte lands before the ellipsis, or after it
    /// for a range's `end`, so a range reaching into the cut covers the ellipsis.
    pub(crate) fn drawn(self, i: usize, end: bool) -> usize {
        if i <= self.head { i } else if i < self.tail { if end { self.after() } else { self.head } } else { self.after() + (i - self.tail) }
    }

    /// Selected content bytes `a..b` as drawn bytes: a selection reaching into the cut
    /// highlights the ellipsis.
    pub(crate) fn range(self, a: usize, b: usize) -> (usize, usize) { (self.drawn(a, false), self.drawn(b, true)) }

    /// A span's or link's content bytes `a..b` as drawn bytes. A range wholly inside the cut
    /// comes back empty and one crossing an edge keeps its visible side. A link takes the
    /// ellipsis only when it shows on both sides of it. A span also takes it when it covers the
    /// first byte the cut hides and shows next to it, so "…" wears the style of the text it
    /// replaces, as AppKit's truncation draws it.
    pub(crate) fn mark(self, a: usize, b: usize, link: bool) -> (usize, usize) {
        let (h, t, after) = (self.head, self.tail, self.after());
        if b <= h { return (a, b) }
        if a >= t { return (after + a - t, after + b - t) }
        let (before, beyond) = (a < h, b > t);
        let ellipsis = if link { before && beyond } else { a <= h && (before || beyond) };
        let start = if before { a } else if ellipsis { h } else { after };
        let end = if beyond { after + b - t } else if ellipsis { after } else { h };
        if start < end { (start, end) } else { (start, start) }
    }

    /// The drawn string's length for content `len` bytes long.
    pub(crate) fn drawn_len(self, len: usize) -> usize { self.drawn(len, true) }

    /// `spans` moved onto the drawn string.
    pub(super) fn spans(self, spans: &[crate::text::Span]) -> Vec<crate::text::Span> {
        let mut out = Vec::with_capacity(spans.len());
        self.spans_into(spans, &mut out);
        out
    }

    pub(super) fn spans_into(self, spans: &[crate::text::Span], out: &mut Vec<crate::text::Span>) {
        out.clear();
        if spans.is_empty() {
            return;
        }
        out.extend(spans.iter().map(|sp| {
            let (start, end) = self.mark(sp.start as usize, sp.end as usize, false);
            crate::text::Span { start: start as u32, end: end as u32, ..sp.clone() }
        }));
    }

    /// `rings` moved onto the drawn string.
    pub(super) fn rings(self, rings: &[(u32, u32, Color, f32)]) -> Vec<(u32, u32, Color, f32)> {
        rings.iter().filter_map(|&(a, b, c, w)| {
            let (start, end) = self.mark(a as usize, b as usize, false);
            (start < end).then_some((start as u32, end as u32, c, w))
        }).collect()
    }

    /// `strikes` moved onto the drawn string.
    pub(super) fn strikes(self, strikes: &[(u32, u32, Option<Color>)]) -> Vec<(u32, u32, Option<Color>)> {
        strikes.iter().filter_map(|&(a, b, c)| {
            let (start, end) = self.mark(a as usize, b as usize, false);
            (start < end).then_some((start as u32, end as u32, c))
        }).collect()
    }

    /// `highlights` moved onto the drawn string.
    pub(super) fn highlights(self, highlights: &[crate::text::TextHighlight]) -> Vec<crate::text::TextHighlight> {
        highlights.iter().filter_map(|h| {
            let (start, end) = self.mark(h.start as usize, h.end as usize, false);
            (start < end).then(|| crate::text::TextHighlight { start: start as u32, end: end as u32, ..h.clone() })
        }).collect()
    }
}

fn spec_elide_keys(spec: &TextSpec, scale: f32, mode: u8, param: usize, max_w: f32, align: parley::Alignment) -> Option<(u64, u64)> {
    if spec.redacted {
        return None;
    }
    let shape = spec.shape_key(scale).unwrap_or_else(|| spec.fresh_shape_key(scale));
    let blocks = spec.blocks.iter().any(|&b| b != crate::edit::Block::Paragraph).then_some(spec.blocks.as_slice());
    Some((shape, crate::text::elide_key(shape, mode, param, max_w, align, blocks)))
}

/// `spec`'s content cut to fit `max_w` device px with an ellipsis, keeping as many whole
/// graphemes as fit at the start, the end or both ends (half each) as its `elide` says.
/// Whitespace graphemes beside the ellipsis are dropped whole.
pub(super) fn truncated(text: &mut TextSystem, spec: &TextSpec, scale: f32, max_w: f32) -> (Rc<Layout<Ink>>, Elision) {
    use unicode_segmentation::UnicodeSegmentation;
    let keys = spec_elide_keys(spec, scale, 0, spec.elide as usize, max_w, parley::Alignment::Start);
    if let Some((_, key)) = keys
        && let Some(hit) = text.get_elision(key)
    {
        return (hit.layout, Elision { head: hit.head, tail: hit.tail });
    }
    let content = spec.content.as_str();
    let bounds: Vec<usize> = content.grapheme_indices(true).map(|(i, _)| i).chain([content.len()]).collect();
    let n = bounds.len() - 1;
    let blank = |g: usize| content[bounds[g]..bounds[g + 1]].chars().all(char::is_whitespace);
    let cut = |k: usize| {
        let (mut head, tail) = match spec.elide {
            crate::el::Elide::End => (k, 0),
            crate::el::Elide::Middle => (k - k / 2, k / 2),
            crate::el::Elide::Head => (0, k),
        };
        let mut tail = n - tail;
        while head > 0 && blank(head - 1) { head -= 1; }
        while tail < n && blank(tail) { tail += 1; }
        Elision { head: bounds[head], tail: bounds[tail] }
    };
    let mut str_buf = if spec.redacted { String::new() } else { String::with_capacity(content.len() + Elision::ELLIPSIS.len()) };
    let mut span_buf = Vec::new();
    let mut make = |text: &mut TextSystem, e: Elision| {
        e.spans_into(&spec.spans, &mut span_buf);
        if spec.redacted {
            let s = zeroize::Zeroizing::new([&content[..e.head], Elision::ELLIPSIS, &content[e.tail..]].concat());
            text.layout_cut(spec, &s, &span_buf, scale, None, parley::Alignment::Start, false)
        } else {
            str_buf.clear();
            str_buf.push_str(&content[..e.head]);
            str_buf.push_str(Elision::ELLIPSIS);
            str_buf.push_str(&content[e.tail..]);
            text.layout_cut(spec, &str_buf, &span_buf, scale, None, parley::Alignment::Start, true)
        }
    };
    let (mut lo, mut hi) = (0usize, n);
    let mut best: Option<(usize, Rc<Layout<Ink>>, Elision)> = None;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let e = cut(mid);
        let l = make(text, e);
        if l.full_width() <= max_w + 0.5 {
            lo = mid;
            best = Some((mid, l, e));
        } else {
            hi = mid - 1;
        }
    }
    let (layout, e) = match best {
        Some((k, l, e)) if k == lo => (l, e),
        _ => {
            let e = cut(lo);
            (make(text, e), e)
        }
    };
    if let Some((shape, key)) = keys {
        text.insert_elision(
            key,
            shape,
            e.drawn_len(content.len()),
            crate::text::CachedElision { layout: layout.clone(), head: e.head, tail: e.tail },
        );
    }
    (layout, e)
}

/// Drops the text layouts of the last commit's nodes this pass flattened again (`nodes`, laid
/// out in `taffy`), before measuring, so the layout cache holds the only reference to each and a
/// width change re-breaks it in place rather than copying it. Nodes that leave keep theirs to
/// draw their exit. A text that was edited (typed into, streamed onto) tells the cache its old
/// version's layouts are done; one replaced by unrelated text may be shown again.
pub(super) fn release_kept_layouts(
    old: &mut [Node],
    nodes: &[Node],
    taffy: &LayoutEngine,
    text: &mut TextSystem,
    reused: &[memo_reuse::ReusedSpan],
) {
    let mut sorted_buf: Vec<(usize, usize)> = Vec::new();
    let ordered = reused.windows(2).all(|w| w[0].old_start <= w[1].old_start);
    if !ordered {
        sorted_buf.extend(reused.iter().map(|s| (s.old_start, s.len)));
        sorted_buf.sort_unstable_by_key(|s| s.0);
    }
    let span_len = if ordered { reused.len() } else { sorted_buf.len() };
    let span_at = |k: usize| if ordered { (reused[k].old_start, reused[k].len) } else { sorted_buf[k] };
    let mut ri = 0usize;
    let mut oi = 0usize;
    while oi < old.len() {
        while ri < span_len && { let (s, l) = span_at(ri); s + l <= oi } {
            ri += 1;
        }
        if ri < span_len && { let (s, l) = span_at(ri); oi >= s && oi < s + l } {
            let (start, len) = span_at(ri);
            let end = (start + len).min(old.len());
            for o in &mut old[oi..end] {
                o.text_layout = None;
            }
            oi = end;
            continue;
        }
        let o = &mut old[oi];
        oi += 1;
        let (Some(was), Some(now_idx)) = (o.text.as_deref(), taffy.mounted_node_index(o.id.0)) else { continue };
        o.text_layout = None;
        let now = nodes.get(now_idx).filter(|n| n.id == o.id).and_then(|n| n.text.as_deref());
        if let (Some((_, shape)), Some(now)) = (was.shape, now) && now.shape != was.shape && edited(&was.content, &now.content) {
            text.supersede(shape);
        }
    }
}

/// Whether `now` reads as an edit of `was`: what they share at the start and end is at least
/// half the longer one.
pub(super) fn edited(was: &str, now: &str) -> bool {
    let (a, b) = (was.as_bytes(), now.as_bytes());
    let prefix = a.iter().zip(b).take_while(|(x, y)| x == y).count();
    let suffix = a[prefix..].iter().rev().zip(b[prefix..].iter().rev()).take_while(|(x, y)| x == y).count();
    a != b && 2 * (prefix + suffix) >= a.len().max(b.len())
}

/// Lays out the flattened tree in `room`, measuring text leaves. Not generic over the app state,
/// so Taffy's layout code is instantiated once, not once per `Runtime<S>`.
pub(super) fn compute_layout(taffy: &mut LayoutEngine, root: SlotId, room: Size<AvailableSpace>, nodes: &[Node], text: &mut TextSystem, scale: f32) {
    taffy.compute_layout(root, room, |inputs, ctx, style| {
        taffy::compute_leaf_layout(inputs, style, |_, _| 0.0, |known, avail| {
            let Some(i) = ctx else { return Size::ZERO };
            let spec = nodes[i].text.as_ref().unwrap();
            // `known` is the border box; `avail` is already the content box (known or styled
            // size, clamped, less padding and border), which is what `lay_out_text` wraps at.
            let w = if spec.wrap {
                match avail.width {
                    AvailableSpace::Definite(w) => Some(w.max(0.0)),
                    AvailableSpace::MinContent => Some(0.0),
                    AvailableSpace::MaxContent => None,
                }
            } else {
                None
            };
            if let Some((run, at)) = &spec.advance_in {
                let r = text.run_layout(run, spec.style, scale);
                if let Some(w) = r.advance(*at, scale) {
                    return Size {
                        width: known.width.unwrap_or(w),
                        height: known.height.unwrap_or(crate::text::spec_height(&r.layout, spec.style, scale, spec.exact_lines) / scale),
                    };
                }
            }
            let l = text.layout_for_spec(spec, scale, w.map(|w| w * scale), parley::Alignment::Start);
            // Spaces hanging past a wrapped line's end take no room, as in CSS.
            let mut lw = ((if w.is_some() { l.width() } else { l.full_width() }) / scale).ceil_to(scale);
            if (spec.truncate || spec.truncate_fade || spec.max_lines > 0)
                && let AvailableSpace::Definite(a) = avail.width
            {
                lw = lw.min(a);
            }
            Size {
                width: known.width.unwrap_or(lw),
                height: known.height.unwrap_or(clamped_height(&l, spec, scale) / scale),
            }
        })
    });
}

/// The height (device px) of `l` as `spec` shows it: its first `max_lines` lines when set.
pub(super) fn clamped_height(l: &Layout<Ink>, spec: &TextSpec, scale: f32) -> f32 {
    let full = crate::text::spec_height(l, spec.style, scale, spec.exact_lines);
    match spec.max_lines.checked_sub(1).and_then(|last| l.lines().nth(last)).filter(|_| l.len() > spec.max_lines) {
        Some(line) => line.metrics().block_max_coord.ceil().min(full),
        None => full,
    }
}

/// The advance of the last line `l` shows when clamped to `lines`, in device px.
pub(super) fn last_line_width(l: &Layout<Ink>, lines: usize) -> f32 {
    l.lines().take(lines).last().map_or(0.0, |line| line.metrics().advance - line.metrics().hanging_advance)
}

/// `spec`'s content wrapped at `max_w` device px and cut after `spec.max_lines` lines, "…"
/// ending the last with as many whole graphemes before it as fit within `max_w`, a word that
/// wraps or overflows included. None when `full` (the uncut layout) already fits.
/// Whitespace and line breaks before the ellipsis are dropped.
pub(super) fn clamped(text: &mut TextSystem, spec: &TextSpec, scale: f32, max_w: f32, full: &Layout<Ink>) -> Option<(Rc<Layout<Ink>>, Elision)> {
    use unicode_segmentation::UnicodeSegmentation;
    let lines = spec.max_lines;
    if lines == 0 || full.len() <= lines && last_line_width(full, lines) <= max_w + 0.5 {
        return None;
    }
    let align = line_alignment(spec);
    let keys = spec_elide_keys(spec, scale, 1, lines, max_w, align);
    if let Some((_, key)) = keys
        && let Some(hit) = text.get_elision(key)
    {
        return Some((hit.layout, Elision { head: hit.head, tail: hit.tail }));
    }
    let content = spec.content.as_str();
    // The cut falls in the last shown line or the next, whose first word may fill the last.
    let start = full.lines().nth(lines.min(full.len()).max(1) - 1)?.text_range().start.min(content.len());
    let end = full.lines().nth(lines).map_or(content.len(), |l| l.text_range().end.min(content.len()));
    let bounds: Vec<usize> = content[start..end].grapheme_indices(true).map(|(i, _)| start + i).chain([end]).collect();
    let blank = |g: usize| content[bounds[g]..bounds[g + 1]].chars().all(char::is_whitespace);
    let cut = |mut k: usize| {
        while k > 0 && blank(k - 1) { k -= 1; }
        let head = if k == 0 { content[..start].trim_end().len() } else { bounds[k] };
        Elision { head, tail: content.len() }
    };
    let mut str_buf = if spec.redacted { String::new() } else { String::with_capacity(end + Elision::ELLIPSIS.len()) };
    let mut span_buf = Vec::new();
    let mut make = |text: &mut TextSystem, e: Elision| {
        e.spans_into(&spec.spans, &mut span_buf);
        if spec.redacted {
            let s = zeroize::Zeroizing::new([&content[..e.head], Elision::ELLIPSIS].concat());
            text.layout_cut(spec, &s, &span_buf, scale, Some(max_w), align, false)
        } else {
            str_buf.clear();
            str_buf.push_str(&content[..e.head]);
            str_buf.push_str(Elision::ELLIPSIS);
            text.layout_cut(spec, &str_buf, &span_buf, scale, Some(max_w), align, true)
        }
    };
    let fits = |l: &Layout<Ink>| l.len() <= lines && last_line_width(l, lines) <= max_w + 0.5;
    let (mut lo, mut hi) = (0usize, bounds.len() - 1);
    let mut best: Option<(usize, Rc<Layout<Ink>>, Elision)> = None;
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let e = cut(mid);
        let l = make(text, e);
        if fits(&l) {
            lo = mid;
            best = Some((mid, l, e));
        } else {
            hi = mid - 1;
        }
    }
    let (layout, e) = match best {
        Some((k, l, e)) if k == lo => (l, e),
        _ => {
            let e = cut(lo);
            (make(text, e), e)
        }
    };
    if let Some((shape, key)) = keys {
        text.insert_elision(
            key,
            shape,
            e.drawn_len(content.len()),
            crate::text::CachedElision { layout: layout.clone(), head: e.head, tail: e.tail },
        );
    }
    Some((layout, e))
}
