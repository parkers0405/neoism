//! Text builders: font, spans and decorations, input pills, wrapping and truncation, reveal,
//! links, placeholder and caret, selection, and edit callbacks.

use super::*;

impl El {
    /// Keeps this text's height its lines' own, unrounded (22.4 points for a line at a 22.4
    /// line height, 24.375 device pixels for a 19.5 point line at 1.25x), as a browser lays out
    /// a paragraph: a stack of texts lands where a web page's does rather than gaining up to a
    /// pixel per text. Every text does this unless it asks for [`El::rounded_lines`].
    pub fn exact_lines(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.exact_lines = true;
        }
        self
    }
    /// Rounds this text's height up to whole device pixels instead of keeping its lines' own
    /// ([`El::exact_lines`]), as Rune laid out every text before: a box sized by one text keeps
    /// its edges on the pixel grid at any scale.
    pub fn rounded_lines(mut self) -> Self {
        if let Some(t) = &mut self.text {
            t.exact_lines = false;
        }
        self
    }
    #[inline]
    pub fn font(mut self, s: TextStyle) -> Self {
        self.set_font(s);
        self
    }
    /// [`El::font`] in place.
    #[inline]
    pub fn set_font(&mut self, s: TextStyle) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.style = s;
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// Per-frame styled or coloured byte ranges (syntax highlights, mentions, inline styles)
    /// painted over a [`text`], [`rich`] or [`text_input`] element without mutating [`TextEdit`].
    /// On a [`text_input`], byte offsets are relative to `edit.text` and shift automatically
    /// across any active IME preedit.
    pub fn spans(mut self, spans: impl Into<Vec<Span>>) -> Self {
        let spans = spans.into();
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.decorations = spans;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.spans = crate::el::pills::with_pill_colors(&spans, &t.pills);
        }
        self
    }
    /// Alias for [`El::spans`]: per-frame syntax or highlight [`Span`]s painted over text
    /// (for example `SyntaxTheme::from_kit(&kit).spans(&highlights, code_offset)`).
    pub fn decorations(self, spans: impl Into<Vec<Span>>) -> Self {
        self.spans(spans)
    }
    /// An inset `color` border ring `width` points wide inside the rounded [`Span::bg`] of bytes
    /// `start..end`. Touching runs of the range on one line share one ring; wrapped lines each
    /// get their own. No background means no ring. Later overlapping rings win.
    /// Ranges refer to authored UTF-8 bytes and follow text case and truncation. For static
    /// text only: editable text does not rebase these ranges after edits. Rebuild ranges when
    /// replacing the content. During streaming reveal a ringed chip fades with its last word
    /// on each line; selection is painted over the chip and below the glyphs.
    pub fn span_ring(mut self, start: usize, end: usize, color: Color, width: f32) -> Self {
        if let Some(t) = &mut self.text && start < end && width > 0.0 && width.is_finite() {
            t.rings.push((start as u32, end as u32, color, width));
        }
        self
    }
    /// The corner radius in points of this text's rounded [`Span::bg`] backgrounds (4 by
    /// default), as a stylesheet's `border-radius` on one kind of span: a diff's changed words
    /// at 3, say, while inline code keeps 4.
    pub fn span_radius(mut self, radius: f32) -> Self {
        if let Some(t) = &mut self.text && radius.is_finite() {
            t.span_radius = radius.max(0.0);
        }
        self
    }
    /// Inset border rings `(start, end, color, width)` for multiple byte ranges (see [`El::span_ring`]).
    pub fn span_rings(mut self, rings: impl IntoIterator<Item = (usize, usize, Color, f32)>) -> Self {
        for (start, end, color, width) in rings {
            self = self.span_ring(start, end, color, width);
        }
        self
    }
    /// A strikethrough line across bytes `start..end` in `color` (`None`: the span's or text's
    /// colour). Wrapped lines each get their own segment.
    pub fn strike(mut self, start: usize, end: usize, color: Option<Color>) -> Self {
        if let Some(t) = &mut self.text && start < end {
            t.strikes.push((start as u32, end as u32, color));
        }
        self
    }
    /// Strikethrough lines `(start, end, color)` for multiple byte ranges (see [`El::strike`]).
    pub fn strikes(mut self, strikes: impl IntoIterator<Item = (usize, usize, Option<Color>)>) -> Self {
        for (start, end, color) in strikes {
            self = self.strike(start, end, color);
        }
        self
    }
    /// A paint-only [`TextHighlight`](crate::TextHighlight) over bytes of the text.
    ///
    /// Drawn from the shaped layout's glyph boxes without splitting runs, so adding, moving or
    /// clearing highlights never reshapes text or shifts line breaks.
    pub fn text_highlight(mut self, highlight: crate::text::TextHighlight) -> Self {
        if let Some(t) = &mut self.text && highlight.start < highlight.end {
            t.highlights.push(highlight);
        }
        self
    }
    /// Paint-only [`TextHighlight`](crate::TextHighlight)s over bytes of the text (see [`El::text_highlight`]).
    pub fn text_highlights(mut self, highlights: impl IntoIterator<Item = crate::text::TextHighlight>) -> Self {
        for h in highlights {
            self = self.text_highlight(h);
        }
        self
    }
    /// Set the visual palette for rich inline marks (`code`, `link`) and paragraph blocks
    /// (`quote`, `code block`, list markers) on a [`text_input`].
    pub fn rich_style(mut self, style: crate::edit::RichStyle) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.rich_style = style;
            self.sync_input_rich();
        } else if let Some(t) = &mut self.text {
            t.rich_style = style;
        }
        self
    }
    /// The chip colour of each [`Mark::Pill`](crate::Mark::Pill) in a [`text_input`], and its
    /// label and icon colour (`None`: the text's). The chip defaults to the rich style's link
    /// colour at 20% opacity.
    pub fn pill_colors(mut self, bg: Color, fg: Option<Color>) -> Self {
        if let Some(t) = &mut self.text {
            t.pill_colors = Some((bg, fg));
        }
        if self.cold.input.is_some() {
            self.sync_input_rich();
        }
        self
    }
    /// The leading icon for each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`],
    /// by the pill's id (`None`: no icon). The icon takes the label's colour.
    ///
    /// ```ignore
    /// text_input(s.draft.clone(), body).pill_icon(|id| id.starts_with("file:").then_some(&FILE))
    /// ```
    ///
    /// The closure may capture state; it runs each time the field is built, so a pill follows
    /// whatever it captured. For colours too, see [`El::pill_style`].
    pub fn pill_icon(mut self, icon: impl Fn(&str) -> Option<&'static crate::Icon> + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_icon = Some(Rc::new(icon));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    /// The look of each [`Mark::Pill`](crate::Mark::Pill) chip in a [`text_input`] by the
    /// pill's id: chip colour, label colour and icon, so a person and an agent can differ.
    /// What the style leaves unset falls back to [`El::pill_colors`] and [`El::pill_icon`].
    /// The closure may capture app state; it runs each time the field is built, and what it
    /// returns is drawn, so a cached subtree restyles when that state changes.
    ///
    /// ```ignore
    /// text_input(s.draft.clone(), body).pill_style({
    ///     let agents = s.agents.clone();
    ///     move |id| if agents.contains(id) { PillStyle::default().bg(PURPLE).icon(&BOT) } else { PillStyle::default() }
    /// })
    /// ```
    pub fn pill_style(mut self, style: impl Fn(&str) -> PillStyle + 'static) -> Self {
        if let Some(inp) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            inp.pill_style = Some(Rc::new(style));
            self.sync_input_pills();
            self.sync_input_rich();
        }
        self
    }
    pub(super) fn sync_input_pills(&mut self) {
        let (Some(inp), Some(t)) = (&self.cold.input, &mut self.text) else { return };
        if inp.edit.is_plain() {
            t.pills.clear();
            return;
        }
        t.pills = inp
            .edit
            .display_pills()
            .into_iter()
            .map(|(start, end, id)| {
                let st = inp.pill_style.as_ref().map(|f| f(id)).unwrap_or_default();
                let icon = st.icon.or_else(|| inp.pill_icon.as_ref().and_then(|f| f(id)));
                crate::text::PillChip { start, end, icon, bg: st.bg, fg: st.fg, bare: None }
            })
            .collect();
    }
    pub(crate) fn sync_input_rich(&mut self) {
        let (Some(inp), Some(t)) = (&self.cold.input, &mut self.text) else { return };
        t.rich_style = inp.rich_style;
        if inp.edit.is_plain() && inp.decorations.is_empty() {
            // A plain field lays out and draws exactly as before rich editing existed.
            t.spans.clear();
            t.strikes.clear();
            t.blocks.clear();
            t.soft_breaks.clear();
            t.pills.clear();
            return;
        }
        let (mut base_spans, strikes, rich_links, blocks) =
            inp.edit.rich_display_specs(t.style, inp.rich_style);
        for d in &inp.decorations {
            for (ds, de) in inp.edit.map_range_to_display(d.start as usize, d.end as usize) {
                let mut sp = d.clone();
                sp.start = ds;
                sp.end = de;
                base_spans.push(sp);
            }
        }
        let pill_fg = t.pill_colors.and_then(|(_, fg)| fg);
        base_spans.extend(t.pills.iter().filter_map(|p| {
            Some(Span { start: p.start, end: p.end, color: Some(p.fg.or(pill_fg)?), ..Span::default() })
        }));
        let mut flat = crate::text::flatten_spans(&base_spans);
        for s in &mut flat {
            if s.underline == Some(Color::TRANSPARENT) {
                s.underline = Some(s.color.unwrap_or(t.color));
            }
        }
        t.spans = flat;
        t.strikes = strikes;
        t.blocks = blocks;
        t.soft_breaks = inp.edit.soft_breaks.clone();
        // Replace links generated from `Mark::Link` while keeping any explicit `.link(...)` calls.
        t.links = rich_links;
    }
    /// Wrap text at the element's width.
    #[inline]
    pub fn wrap(mut self) -> Self {
        self.set_wrap();
        self
    }
    /// [`El::wrap`] in place.
    #[inline]
    pub fn set_wrap(&mut self) -> &mut Self {
        if let Some(t) = &mut self.text {
            t.wrap = true;
        }
        self
    }
    /// Wrap text at the element's width, breaking inside a word only when the word alone would
    /// overflow the line (`overflow-wrap: anywhere`) while breaking normal words at spaces, and
    /// lowering the text's min-content width. Does not change flex shrinking; use
    /// [`El::shrink`] to allow a flex row to narrow the element.
    pub fn wrap_anywhere(mut self) -> Self {
        if self.cold.input.as_ref().is_some_and(|i| i.single_line) {
            return self;
        }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.wrap_anywhere = true;
        }
        self
    }
    /// Whether this is text that wraps (`.wrap()`), for containers that let such text shrink
    /// to their width.
    pub fn wraps(&self) -> bool {
        self.text.as_ref().is_some_and(|t| t.wrap)
    }
    /// One line, cut with "…" when wider than the element; the element may shrink below its
    /// text's width. A [`text_input`] ignores it and the other truncations.
    #[inline]
    pub fn truncate(mut self) -> Self {
        self.set_truncate();
        self
    }
    /// [`El::truncate`] in place.
    #[inline]
    pub fn set_truncate(&mut self) -> &mut Self {
        self.set_elide(Elide::End);
        self
    }
    /// [`El::truncate`] keeping both ends: "…" replaces the middle, as AppKit's
    /// `NSLineBreakByTruncatingMiddle` does for paths and file names. Cuts fall between
    /// graphemes and refit when the element resizes; copying and the accessibility text keep
    /// the whole string.
    pub fn truncate_middle(mut self) -> Self {
        self.set_elide(Elide::Middle);
        self
    }
    /// [`El::truncate`] keeping the end: "…" replaces the start.
    pub fn truncate_head(mut self) -> Self {
        self.set_elide(Elide::Head);
        self
    }
    fn set_elide(&mut self, elide: Elide) {
        // A field's caret and selection index its own text, which a cut would break.
        if self.cold.input.is_some() { return; }
        self.set_one_line(|t| {
            t.truncate = true;
            t.elide = elide;
            t.truncate_fade = false;
        });
    }
    /// One unwrapped line that shrinks with its parent; `cut` sets how an overflow is cut.
    fn one_line(mut self, cut: impl FnOnce(&mut TextSpec)) -> Self {
        self.set_one_line(cut);
        self
    }
    fn set_one_line(&mut self, cut: impl FnOnce(&mut TextSpec)) {
        if let Some(t) = &mut self.text {
            t.wrap = false;
            t.wrap_anywhere = false;
            t.max_lines = 0;
            cut(t);
        }
        self.style.set_flex_shrink(1.0);
        self.style.set_min_width(length(0.0));
    }
    /// Wrapped text that stops after `lines` lines, "…" ending the last when more would follow,
    /// as CSS's `line-clamp`. The element measures at most `lines` lines tall. The cut falls
    /// between graphemes and refits when the element resizes; spans and links keep their
    /// looks up to it, and copying and the accessibility text keep the whole string. 0 shows
    /// every line. A [`text_input`] ignores it; use [`auto_grow`] there.
    pub fn max_lines(mut self, lines: usize) -> Self {
        if self.cold.input.is_some() { return self; }
        if let Some(t) = &mut self.text {
            t.wrap = true;
            t.max_lines = lines;
            t.truncate = false;
            t.truncate_fade = false;
        }
        self
    }
    /// One line, clipped with a 1.5 em trailing fade only when it overflows. No ellipsis.
    pub fn truncate_fade(self) -> Self {
        self.truncate_fade_width(TRUNCATE_FADE_EM)
    }
    /// [`El::truncate_fade`] with a fade `em` ems wide.
    pub fn truncate_fade_width(self, em: f32) -> Self {
        self.one_line(|t| {
            t.truncate = false;
            t.truncate_fade = true;
            t.fade_em = em.max(0.0);
        })
    }
    /// Streaming word reveal (see [`TextSpec::reveal`]).
    pub fn reveal(mut self, word_fade: f32) -> Self {
        if let Some(t) = &mut self.text {
            t.reveal = word_fade.max(0.0);
        }
        self
    }
    /// Keep a text field to one line: long text scrolls sideways under the caret instead of
    /// wrapping. Fields are multi-line by default.
    pub fn single_line(mut self) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.single_line = true;
            i.edit.set_single_line(true);
            if let Some(t) = &mut self.text {
                t.wrap = false;
                t.wrap_anywhere = false;
                if i.edit.is_secure() && !i.edit.is_revealed() {
                    t.replace_content(i.edit.display().0);
                }
            }
        }
        self
    }
    /// Make a byte range of this text pressable (see [`Link`]). Only the range's shaped glyph
    /// boxes take the pointer, which shows a hand over them; the rest of the text stays
    /// transparent to input. Each link is a Tab stop that Enter or Space presses.
    pub fn link(mut self, link: Link) -> Self {
        if let Some(t) = &mut self.text {
            t.links.push(link);
            self.cold_mut().focus.enabled = true;
            if self.handlers.cursor.is_none() {
                self.handlers.cursor = Some(Cursor::Pointer);
            }
        }
        self
    }
    /// Called with a [`Link`]'s id when it is clicked, or pressed with Enter or Space while
    /// focused.
    pub fn on_link<S: 'static>(mut self, f: impl Fn(&mut S, &str, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.on_link = Some(LinkFn(Rc::new(move |s, id, tx| f(downcast::<S>(s), id, tx))));
        }
        self
    }
    /// Called with a [`Link`]'s id and the point (window points) when a secondary press
    /// (right-click, macOS control-click) lands on its glyphs: a menu for that link. It takes
    /// the press, so ancestors' [`El::on_secondary_press`] and the text's Copy menu don't see
    /// it; elsewhere in the text they still do.
    pub fn on_link_secondary<S: 'static>(mut self, f: impl Fn(&mut S, &str, PointerPress, &mut Tx) + 'static) -> Self {
        if let Some(t) = &mut self.text {
            t.on_link_secondary = Some(LinkSecondaryFn(Rc::new(move |s, id, p, tx| f(downcast::<S>(s), id, p, tx))));
        }
        self
    }
    /// Mark this text field's misspelled words, as a browser's `spellcheck` does: the platform's
    /// checker (Windows' spell checking API, AppKit's `NSSpellChecker`; none on Linux or the
    /// web) puts a red marker under each word the caret has left, and the field's menu offers its
    /// suggestions and Learn Spelling. Off by default; a secure field never checks.
    pub fn spellcheck(mut self, on: bool) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.spellcheck = on;
        }
        self
    }
    pub fn placeholder(mut self, s: impl Into<String>, c: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.placeholder = s.into();
            i.placeholder_color = c;
            i.placeholder_inked = true;
        }
        self
    }
    pub fn caret(mut self, caret: Color, selection: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.caret = caret;
            i.selection = selection;
            i.caret_inked = true;
        }
        self
    }
    /// Lets people select this read-only text and copy it, as SwiftUI's
    /// `.textSelection(.enabled)`: drag across it, double-click a word, triple-click a paragraph,
    /// shift-click to extend; Cmd+C (Ctrl+C elsewhere) copies and Cmd+A selects all of it while it
    /// holds the selection; a right-click offers Copy on macOS. It shows the I-beam, isn't a Tab
    /// stop, and a press anywhere else clears the selection. Selecting never re-lays out the
    /// text. Set the highlight with [`El::selection_color`].
    ///
    /// Inside a row that handles input, the row keeps it: a click on the text presses the row,
    /// hover and press paint reach it, and its `on_secondary_press` wins over Copy. A drag that
    /// selects text doesn't press. Inside a draggable (`on_drag`, `drag_out`) row, dragging
    /// drags the row, and double- and triple-clicks still select.
    #[inline]
    pub fn selectable(mut self, on: bool) -> Self {
        self.set_selectable(on);
        self
    }
    /// [`El::selectable`] in place.
    pub fn set_selectable(&mut self, on: bool) -> &mut Self {
        if let Some(t) = &mut self.text {
            if self.cold.input.is_some() || t.redacted { return self; }
            t.selectable = on;
            if on && (self.handlers.cursor.is_none() || (!t.links.is_empty() && self.handlers.cursor == Some(Cursor::Pointer))) {
                self.handlers.cursor = Some(Cursor::Text);
            }
        }
        self
    }
    /// Makes the [`El::selectable`] texts inside this element one selection, in tree order, as
    /// the paragraphs of a web page are: a drag from one into another selects across them (the
    /// gaps between included), Shift-click extends into any of them, Cmd+A selects them all and
    /// Cmd+C copies their text joined by a newline (or the blank line [`El::copy_breaks`] asks
    /// for, after any [`El::copy_prefix`]), or a tab between texts side by side (table cells). Double- and triple-clicks still take a word or paragraph of one text. Text that
    /// isn't selectable (a list marker) is skipped. Outside a group each text is its own.
    pub fn selection_group(mut self, on: bool) -> Self {
        self.flags.set(ElFlags::SELECTION_GROUP, on);
        self
    }
    /// What copying a text selection inside this element puts on the clipboard (Cmd+C and the
    /// Copy menu) instead of the selected text as drawn, so a document drawn from source can
    /// copy the source. `f` sees every [`El::selectable`] text inside this element that the
    /// selection could reach (those of its selection group), in tree order, each with its
    /// selected range (empty when it isn't selected) and its path relative to this element.
    /// Return `None` to copy the drawn text after all ([`TextSelection::text`]). The nearest
    /// such element around both ends of the selection answers. Kit's `Markdown::copy_source`
    /// is one.
    ///
    /// ```ignore
    /// col().selection_group(true)
    ///     .selection_copy(move |sel| sel.texts().iter().all(|t| t.is_whole()).then(|| note.clone()))
    /// ```
    ///
    /// [`TextSelection::text`]: crate::TextSelection::text
    pub fn selection_copy(mut self, f: impl Fn(&crate::TextSelection) -> Option<String> + 'static) -> Self {
        self.handlers.selection_copy = Some(Rc::new(f));
        self
    }
    /// Hears the [`El::selectable`] text selection change inside this element: when one starts
    /// (the first selected character of a drag, a double-click's word, Cmd+A,
    /// [`Tx::select_text`]), changes, leaves this element or clears. [`SelectionChange`] holds
    /// the whole selection while any of it is inside this element, `None` otherwise, and says
    /// whether it [`started`](crate::SelectionChange::started) or
    /// [`cleared`](crate::SelectionChange::cleared) here. It runs at most once a frame, however
    /// many times a drag moved the selection, before that frame's commit, and once more when the
    /// press making it releases ([`dragging`](crate::SelectionChange::dragging)). A press leaving
    /// only a caret isn't heard. Call [`Tx::live_only`] when the view reads nothing that changed.
    ///
    /// ```ignore
    /// col().selection_group(true).on_selection_change(|s: &mut Notes, change, _| {
    ///     if change.started() { s.whole_note_highlight = false; }
    /// })
    /// ```
    ///
    /// [`SelectionChange`]: crate::SelectionChange
    pub fn on_selection_change<S: 'static>(mut self, f: impl Fn(&mut S, &crate::SelectionChange, &mut Tx) + 'static) -> Self {
        self.handlers.on_selection_change = Some(Rc::new(move |s, change, tx| f(downcast::<S>(s), change, tx)));
        self
    }
    /// Draws and lays out, but the pointer and keyboard pass through this element and
    /// everything inside it, as with HTML's `inert`: a press, hover, scroll, drop or tooltip
    /// goes to whatever is under it (an ancestor or an element drawn below), along with that
    /// element's cursor. Handlers inside never run from input, nothing inside takes focus (Tab
    /// skips it, a field asking for focus doesn't get it, and focus already inside drops), and
    /// accessibility reports it inert. `on_layout` and `Cx::layout` still see its rects. For a
    /// document drawn under an editor that takes its presses itself.
    pub fn inert(mut self) -> Self {
        self.flags.set(ElFlags::INERT, true);
        self
    }
    /// Bytes of [`El::selectable`] text that draw but aren't copied, such as the spacing
    /// characters Markdown pads inline code with. A double-click's word stops at them.
    pub fn uncopied(mut self, range: std::ops::Range<usize>) -> Self {
        if let Some(t) = &mut self.text && range.start < range.end {
            let at = t.uncopied.partition_point(|r| (r.0 as usize) < range.start);
            t.uncopied.insert(at, (range.start as u32, range.end as u32));
        }
        self
    }
    /// How many newlines copying puts between this [`El::selectable`] text and the text copied
    /// before it, and the text after it, when a [`El::selection_group`] selection takes more than
    /// one: the larger of the two texts' asks, one by default. Two leaves a blank line, as a web
    /// page's paragraphs copy. Texts side by side still take a tab.
    ///
    /// ```ignore
    /// text(para).selectable(true).copy_breaks(2, 2) // "first para\n\nsecond para"
    /// ```
    pub fn copy_breaks(mut self, before: u8, after: u8) -> Self {
        if let Some(t) = &mut self.text { t.copy_breaks = [before, after]; }
        self
    }
    /// Text copying puts before this [`El::selectable`] text when a selection takes it from its
    /// start along with other texts, without drawing it: a list item's `"- "` or `"1. "`, whose
    /// drawn marker isn't selectable. A copy of this text alone leaves it out.
    pub fn copy_prefix(mut self, prefix: impl Into<std::rc::Rc<str>>) -> Self {
        if let Some(t) = &mut self.text { t.copy_prefix = Some(prefix.into()); }
        self
    }
    /// A text field's selection as a browser paints a field's: its highlight and the ink its
    /// selected glyphs take, one pair while the window is active and another while it isn't
    /// ([`SelectionStyle::chromium`](crate::SelectionStyle::chromium) is Chromium's). It takes
    /// over [`El::caret`]'s and [`El::selection_color`]'s highlight.
    pub fn selection_style(mut self, style: crate::SelectionStyle) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) { i.selection_style = Some(style); }
        self
    }
    /// The selection highlight of [`El::selectable`] text or a text field, drawn alike for both.
    pub fn selection_color(mut self, color: Color) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) { i.selection = color; }
        if let Some(t) = &mut self.text { t.selection = color; }
        self
    }
    /// Binds the field to a `TextEdit` in app state. `f` may capture (a row index, say); wrap
    /// a closure in [`edit_access`] so its returned borrow is tied to the state.
    pub fn on_edit<S: 'static>(mut self, f: impl for<'a> Fn(&'a mut S) -> &'a mut TextEdit + 'static) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.access = Some(Rc::new(edit_fn(move |s| f(downcast::<S>(s)))));
        }
        self
    }
    /// Called when Return submits this text field. In a multi-line field (the default),
    /// Shift+Return and Option+Return insert a newline (as does plain Return inside a list, quote
    /// or code block), while plain Return in a paragraph calls `on_submit` — or presses the
    /// [`El::default_button`] when the field has none.
    pub fn on_submit<S: 'static>(mut self, f: impl Fn(&mut S, &mut Tx) + 'static) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) {
            i.submit = Some(Rc::new(move |s, tx| f(downcast::<S>(s), tx)));
        }
        self
    }
    /// Let dependent layout reflow when this input changes; the typed text remains immediate.
    pub fn animate_edit_layout(mut self) -> Self {
        if let Some(i) = self.cold.get_mut_if_some().and_then(|c| c.input.as_deref_mut()) { i.animate_layout = true; }
        self
    }
}
