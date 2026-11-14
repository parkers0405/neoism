//! Single and rich text-field editing: the attributed model an app owns (`TextEdit`) and the
//! key/IME events the shell routes to the focused field. Movement that depends on wrapping
//! (up/down, line start/end) uses the field's current Parley layout, rebuilt by the runtime
//! before each event.
//!
//! Invariants: `anchor` and `focus` are byte offsets on char boundaries of `text`; preedit text
//! is never part of `text` until committed; inline `marks` are non-empty char-boundary byte
//! ranges sorted by start; `blocks` has one entry per `\n`-separated line of `text`.

use std::ops::Range;
use std::sync::Arc;
use zeroize::{Zeroize, Zeroizing};

use crate::color::Color;
use crate::text::Link;

mod caret_motion;
mod display;
mod history;
mod html;
mod ime;
pub use ime::Ime;
mod input;
mod keys;
pub use keys::{Key, KeyEvent, KeySide, Modifier, Mods};
mod marks;
mod model;
pub use model::{Block, MAX_LIST_LEVEL, Mark, MarkRange, ParagraphInfo, RichStyle};
pub(crate) use history::History;
use history::{ActiveGroup, EditKind, EditedTag};

/// Fixed mask shown by a hidden multi-line secure field when non-empty, hiding both byte length
/// and line count.
pub(crate) const MULTILINE_SECURE_MASK: &str = "••••••••••••";

/// Mirrors whether the bound [`text_input`](crate::text_input) has [`El::single_line`](crate::El::single_line).
/// Before a field mounts, standalone [`TextEdit::secure`] defaults to single-line.
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum LineMode {
    #[default]
    Default,
    Single,
    Multi,
}

#[derive(Clone, Default, PartialEq)]
pub struct TextEdit {
    /// Use `replace_text` to replace a secret without leaving its old allocation behind.
    pub text: String,
    pub anchor: usize,
    pub focus: usize,
    /// The input method's composition (`TextEdit::preedit`).
    composition: ime::Composition,
    pub focused: bool,
    marks: Vec<MarkRange>,
    blocks: Vec<Block>,
    /// Line indices (`> 0`) created by `Shift+Return` (`<br>`): indented with their block,
    /// without starting a new list marker or incrementing list numbering.
    pub(crate) soft_breaks: Vec<u32>,
    /// Pending typing style armed at `(anchor, focus)` when toggling marks on an empty selection.
    pending: Option<((usize, usize), Vec<Mark>)>,
    history: Arc<History>,
    /// Set inside [`TextEdit::undo_group`]: edits record nothing until the outermost group ends.
    group: ActiveGroup,
    secure: bool,
    revealed: bool,
    /// Mirrors the field's `El::single_line()`; the runtime sets it before each edit.
    line_mode: LineMode,
    /// Formatting keys and rich paste are on (see [`TextEdit::rich`]).
    rich: bool,
    /// The mark kinds this field keeps, one bit per [`Mark::kind_order`]; `None` keeps all.
    allowed_marks: Option<u16>,
    /// The block kinds this field keeps ([`Block::kind_bit`]); `None` keeps all.
    allowed_blocks: Option<u8>,
    /// Whether a user has edited the value since `replace_text`.
    edited_tag: EditedTag,
}

impl std::fmt::Debug for TextEdit {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextEdit").field("text", &if self.secure { "[redacted]" } else { &self.text })
            .field("anchor", &self.anchor).field("focus", &self.focus).field("focused", &self.focused)
            .field("marks", &self.marks).field("blocks", &self.blocks)
            .field("secure", &self.secure).field("revealed", &self.revealed).finish_non_exhaustive()
    }
}

/// Hashes everything a field draws: text, selection, IME composition, focus, marks, blocks and
/// whether it's secure or revealed. Pass the edit itself in a [`memo`](crate::memo) version so
/// the field redraws as the caret or rich formatting changes. A hidden secure field hashes its
/// length (single-line) or non-empty state (multi-line), not its text.
impl std::hash::Hash for TextEdit {
    fn hash<H: std::hash::Hasher>(&self, h: &mut H) {
        if self.is_hidden_multiline() {
            (!self.text.is_empty()).hash(h);
            (self.anchor != self.focus).hash(h);
        } else if self.secure && !self.revealed {
            self.text.chars().count().hash(h);
            self.anchor.hash(h);
            self.focus.hash(h);
        } else {
            self.text.hash(h);
            self.anchor.hash(h);
            self.focus.hash(h);
        }
        self.composition.hash(h);
        self.focused.hash(h);
        self.marks.hash(h);
        if !self.secure {
            self.blocks.hash(h);
        }
        self.soft_breaks.hash(h);
        self.pending.hash(h);
        self.secure.hash(h);
        self.revealed.hash(h);
        self.is_single_line().hash(h);
    }
}

impl Drop for TextEdit {
    fn drop(&mut self) {
        self.text.zeroize();
        self.cancel_composition();
    }
}

/// What an edit asks of the host.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EditOutcome {
    pub submit: bool,
    pub copy: Option<String>,
    pub blur: bool,
}

/// The previous grapheme boundary, so a caret or backspace takes a ZWJ emoji, flag or skin
/// tone whole.
fn prev_char(s: &str, i: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s[..i].grapheme_indices(true).next_back().map_or(0, |(j, _)| j)
}

fn next_char(s: &str, i: usize) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s[i..].graphemes(true).next().map_or(i, |g| i + g.len())
}

impl TextEdit {
    pub fn new(text: impl Into<String>) -> TextEdit {
        let text = text.into();
        let n = text.len();
        let line_count = 1 + count_newlines(text.as_bytes());
        TextEdit {
            text,
            anchor: n,
            focus: n,
            composition: ime::Composition::Idle,
            focused: false,
            marks: Vec::new(),
            blocks: vec![Block::Paragraph; line_count],
            soft_breaks: Vec::new(),
            pending: None,
            history: Arc::default(),
            group: ActiveGroup::default(),
            secure: false,
            revealed: false,
            line_mode: LineMode::Default,
            rich: false,
            allowed_marks: None,
            allowed_blocks: None,
            edited_tag: EditedTag::default(),
        }
    }

    /// Construct a [`TextEdit`] from plain text, inline [`MarkRange`]s, and per-line [`Block`]s.
    /// Caret starts at the end of `text`. Ranges are clamped to char boundaries and canonicalized,
    /// and `blocks` is padded or truncated to match the number of `\n`-separated lines in `text`.
    pub fn from_parts(text: impl Into<String>, marks: Vec<MarkRange>, blocks: Vec<Block>) -> TextEdit {
        let mut edit = Self::new(text).rich();
        edit.marks = marks;
        edit.blocks = blocks;
        edit.clamp_list_levels();
        edit.sync_invariants();
        edit
    }

    fn clamp_list_levels(&mut self) {
        for b in &mut self.blocks {
            *b = b.with_level(b.list_level().unwrap_or(0));
        }
    }

    /// Rich editing: `⌘B` / `⌘I` / `⌘U` toggle marks, `Shift+Return` inserts a soft break and
    /// [`paste_html`](Self::paste_html) keeps formatting. Off by default, so a plain field leaves
    /// those keys to the app and pastes plain text. [`from_html`](Self::from_html) and
    /// [`from_parts`](Self::from_parts) turn it on. The mark and block methods work either way.
    pub fn rich(mut self) -> Self {
        self.rich = true;
        self
    }

    pub fn is_rich(&self) -> bool {
        self.rich
    }

    /// Keep only these kinds of inline mark, for a field whose storage can't hold the rest (an
    /// app that saves Markdown has no underline). A disallowed mark's shortcut (`⌘U`) goes on
    /// to the app's key handlers, `toggle_mark` and `set_mark` ignore it, and paste drops it.
    /// Kinds match by [`Mark::same_kind`], so any [`Mark::Link`] allows links. The field never
    /// holds a disallowed mark: marks it already has, or that `set_marks` adds, are dropped.
    /// Every kind is allowed by default, and [`Mark::Pill`] always is: pills are the app's own
    /// tokens, not formatting.
    ///
    /// ```ignore
    /// TextEdit::new("").rich().allow_marks([Mark::Bold, Mark::Italic, Mark::Strike, Mark::Code, Mark::Link(String::new())])
    /// ```
    pub fn allow_marks(mut self, marks: impl IntoIterator<Item = Mark>) -> Self {
        let pill = 1 << Mark::Pill(String::new()).kind_order();
        self.allowed_marks = Some(marks.into_iter().fold(pill, |bits, m| bits | 1 << m.kind_order()));
        self.sync_invariants();
        self
    }

    /// Keep only these kinds of paragraph block, as [`allow_marks`](Self::allow_marks) does for
    /// marks: `toggle_block` and `set_block` ignore the rest and pasted ones become paragraphs.
    /// [`Block::Paragraph`] is always allowed, and list levels don't matter (`Block::Bullet(0)`
    /// allows every bullet).
    pub fn allow_blocks(mut self, blocks: impl IntoIterator<Item = Block>) -> Self {
        self.allowed_blocks = Some(blocks.into_iter().fold(Block::Paragraph.kind_bit(), |bits, b| bits | b.kind_bit()));
        self.sync_invariants();
        self
    }

    /// Whether this field keeps marks of `mark`'s kind (see [`allow_marks`](Self::allow_marks)),
    /// to hide a formatting bar's button for one it doesn't (`Mark::Link(String::new())` checks
    /// links).
    pub fn allows_mark(&self, mark: &Mark) -> bool {
        self.allowed_marks.is_none_or(|bits| bits & 1 << mark.kind_order() != 0)
    }

    /// Whether this field keeps blocks of `block`'s kind (see [`allow_blocks`](Self::allow_blocks)).
    pub fn allows_block(&self, block: Block) -> bool {
        self.allowed_blocks.is_none_or(|bits| bits & block.kind_bit() != 0)
    }

    /// The mark a rich field's `⌘B`, `⌘I` or `⌘U` toggles, if this field takes that chord.
    pub(crate) fn format_chord(&self, ev: &KeyEvent) -> Option<Mark> {
        let c = ev.edit_command()?;
        if ev.mods.shift || !self.rich || self.secure {
            return None;
        }
        let mark = match c {
            'b' => Mark::Bold,
            'i' => Mark::Italic,
            'u' => Mark::Underline,
            _ => return None,
        };
        self.allows_mark(&mark).then_some(mark)
    }

    /// `frag` without the marks and blocks this field doesn't allow.
    fn keep_allowed(&self, mut frag: TextEdit) -> TextEdit {
        frag.allowed_marks = self.allowed_marks;
        frag.allowed_blocks = self.allowed_blocks;
        frag.sync_invariants();
        frag
    }

    /// No marks, pending style or non-paragraph blocks: draws exactly as plain text.
    pub(crate) fn is_plain(&self) -> bool {
        self.marks.is_empty() && self.pending.is_none() && self.blocks.iter().all(|&b| b == Block::Paragraph)
    }

    /// Parse an HTML fragment (`<b>`, `<strong>`, `<i>`, `<em>`, `<u>`, `<s>`, `<del>`, `<strike>`,
    /// `<a href>`, `<code>`, `<pre>`, `<ul>`, `<ol>`, `<li>`, `<blockquote>`, `<p>`, `<div>`, `<br>`,
    /// and inline `style` for `font-weight`, `font-style`, `text-decoration`) into a [`TextEdit`].
    /// Unknown tags keep their text and strip the wrapper; `<script>` and `<style>` are dropped.
    pub fn from_html(html_str: &str) -> TextEdit {
        html::parse_html(html_str).rich()
    }

    /// Serialize the entire document's text, inline marks, and paragraph blocks to safe HTML.
    pub fn to_html(&self) -> String {
        let e = self.synced();
        html::serialize_html(&e, 0..e.text.len())
    }

    /// `self` with marks, blocks and caret fitted to `text`, which an app may have assigned
    /// directly.
    fn synced(&self) -> TextEdit {
        let mut e = self.clone();
        e.sync_invariants();
        e
    }

    /// Serialize the current selection (`self.range()`) to HTML, or `""` when the selection is empty.
    pub fn selected_html(&self) -> String {
        let (a, b) = self.range();
        if a == b || self.secure {
            return String::new();
        }
        html::serialize_html(&self.synced(), a..b)
    }

    /// Inline [`MarkRange`]s on `self.text`, sorted by start byte.
    pub fn marks(&self) -> &[MarkRange] {
        &self.marks
    }

    /// Paragraph [`Block`] attributes, one per `\n`-delimited line of `self.text`.
    pub fn blocks(&self) -> &[Block] {
        if self.blocks.is_empty() {
            &[Block::Paragraph]
        } else {
            &self.blocks
        }
    }

    /// Replace the inline [`MarkRange`]s on `self.text` and canonicalize them.
    pub fn set_marks(&mut self, marks: Vec<MarkRange>) {
        self.marks = marks;
        self.sync_invariants();
    }

    /// Replace the per-line [`Block`] list on `self.text`.
    pub fn set_blocks(&mut self, blocks: Vec<Block>) {
        self.blocks = blocks;
        self.clamp_list_levels();
        self.sync_invariants();
    }

    /// Line indices (`> 0`) created by `Shift+Return` (`<br>`) rather than a hard paragraph break.
    pub fn soft_breaks(&self) -> &[u32] {
        &self.soft_breaks
    }

    /// `self` with soft breaks before lines `lines` (indices into `\n`-delimited lines, as
    /// [`soft_breaks`](Self::soft_breaks) returns them): each of those lines continues the
    /// paragraph, list item or quote above it, as `Shift+Return` makes, so a draft rebuilt with
    /// [`from_parts`](Self::from_parts) keeps them. Line 0 and lines past the end are dropped.
    ///
    /// ```ignore
    /// // "one" then "two" on a new line of the same bullet.
    /// TextEdit::from_parts("one\ntwo", vec![], vec![Block::Bullet(0); 2]).with_soft_breaks(vec![1])
    /// ```
    pub fn with_soft_breaks(mut self, lines: Vec<u32>) -> Self {
        self.set_soft_breaks(lines);
        self
    }

    /// Replace the soft breaks (see [`with_soft_breaks`](Self::with_soft_breaks)). Like
    /// `set_marks` and `set_blocks`, this isn't an undo step.
    pub fn set_soft_breaks(&mut self, lines: Vec<u32>) {
        self.soft_breaks = lines;
        self.sync_invariants();
    }

    /// Iterate the `\n`-delimited paragraphs of `self.text` with their byte range and [`Block`].
    pub fn paragraphs(&self) -> Vec<ParagraphInfo> {
        let mut out = Vec::new();
        let mut line_start = 0;
        let mut idx = 0;
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'\n' {
                let block = self.blocks.get(idx).copied().unwrap_or(Block::Paragraph);
                let soft_break = self.soft_breaks.binary_search(&(idx as u32)).is_ok();
                out.push(ParagraphInfo { index: idx, range: line_start..i, block, soft_break });
                line_start = i + 1;
                idx += 1;
            }
        }
        let block = self.blocks.get(idx).copied().unwrap_or(Block::Paragraph);
        let soft_break = self.soft_breaks.binary_search(&(idx as u32)).is_ok();
        out.push(ParagraphInfo { index: idx, range: line_start..self.text.len(), block, soft_break });
        out
    }

    /// Password editing: masked display, no undo, copy, cut or IME. All model buffers are
    /// wiped on drop; secure replacements also wipe the previous allocation before freeing it.
    pub fn secure(mut self) -> Self { self.set_secure(true); self }

    pub fn is_secure(&self) -> bool { self.secure }

    pub fn set_secure(&mut self, secure: bool) {
        self.secure = secure;
        if secure {
            self.history = Arc::default();
            if self.group.0.is_some() {
                self.group.0 = Some(None);
            }
            self.marks.clear();
            self.blocks.clear();
            self.soft_breaks.clear();
            self.pending = None;
            self.cancel_composition();
        } else if self.group.0 == Some(None) {
            self.sync_invariants();
            self.group.0 = Some(Some(self.snapshot_now()));
        }
    }

    /// Show the plain characters of a [`secure`](Self::secure) field while keeping all other
    /// secure protections: no undo, copy, cut, IME or accessibility text, redacted [`Debug`],
    /// and zeroized buffers.
    /// Rune-owned source buffers are wiped; third-party shaping scratch used while revealed
    /// is released after shaping but is not guaranteed zeroized.
    pub fn revealed(mut self, revealed: bool) -> Self { self.set_revealed(revealed); self }

    pub fn is_revealed(&self) -> bool { self.revealed }

    pub fn set_revealed(&mut self, revealed: bool) { self.revealed = revealed; }

    /// The input method's composition, drawn at the caret and not yet part of `text`, with the
    /// byte range of its cursor or selection in it.
    pub fn preedit(&self) -> Option<(&str, Option<(usize, usize)>)> {
        self.composition.preedit()
    }

    /// An input method is composing in this field.
    pub fn is_composing(&self) -> bool {
        self.composition != ime::Composition::Idle
    }

    /// Ends the input method's composition without committing it, as a blur does.
    pub fn cancel_composition(&mut self) {
        self.composition.cancel();
    }

    pub(crate) fn clear_pending(&mut self) {
        self.pending = None;
    }

    /// Replace the whole draft and discard its history, preserving focus and password mode.
    pub fn replace_text(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.text.zeroize();
        let line_count = if self.secure { 0 } else { 1 + count_newlines(text.as_bytes()) };
        self.text = text;
        self.anchor = self.text.len();
        self.focus = self.anchor;
        self.marks.clear();
        self.blocks = vec![Block::Paragraph; line_count];
        self.soft_breaks.clear();
        self.pending = None;
        self.cancel_composition();
        self.edited_tag = EditedTag(false);
        self.history = Arc::default();
        if self.group.0.is_some() {
            self.group.0 = Some((!self.secure).then(|| self.snapshot_now()));
        }
    }

    /// Whether `self.text` is non-empty and still holds the value the app last set, with no user
    /// edit since [`TextEdit::new`] or [`Self::replace_text`]. An empty field is never pristine.
    /// Use `replace_text` for app replacements, even with the same value. Direct mutation of
    /// the public `text` string cannot reset this state and does not zeroize its old allocation.
    ///
    /// While a hidden multi-line [`secure`](Self::secure) field is pristine, the first insertion
    /// replaces the entire value with the new text and the first deletion empties it, regardless
    /// of caret or selection.
    pub fn is_pristine(&self) -> bool {
        !self.text.is_empty() && !self.edited_tag.0
    }

    /// Mark the current `self.text` as user-edited (`!self.is_pristine()`).
    pub fn mark_edited(&mut self) {
        self.edited_tag.0 = true;
    }

    #[cfg(test)]
    pub(crate) fn has_history(&self) -> bool { !self.history.undo.is_empty() || !self.history.redo.is_empty() }

    pub(crate) fn same_model(&self, other: &TextEdit) -> bool {
        Arc::ptr_eq(&self.history, &other.history)
    }

    pub(crate) fn weak_history(&self) -> std::sync::Weak<History> {
        Arc::downgrade(&self.history)
    }

    pub(crate) fn matches_history(&self, weak: &std::sync::Weak<History>) -> bool {
        weak.ptr_eq(&Arc::downgrade(&self.history))
    }

    pub(crate) fn is_single_line(&self) -> bool {
        match self.line_mode {
            LineMode::Default => self.secure,
            LineMode::Single => true,
            LineMode::Multi => false,
        }
    }

    /// Whether this is a multi-line secure field in hidden mode (`secure && !revealed && !single_line`).
    pub(crate) fn is_hidden_multiline(&self) -> bool {
        self.secure && !self.revealed && !self.is_single_line()
    }

    /// For a secure field that has started exiting: wipe the secret and keep only its mask, so
    /// the field paints the same dots (no placeholder) through the exit with no plaintext.
    pub(crate) fn conceal(&mut self) {
        if !self.secure { return; }
        self.revealed = false;
        let mask = if self.text.is_empty() {
            String::new()
        } else if self.is_single_line() {
            "•".repeat(self.text.chars().count())
        } else {
            MULTILINE_SECURE_MASK.to_owned()
        };
        self.replace_text(mask);
    }

    /// Map model bytes to the bullet layout's bytes and back. Each Unicode scalar gets one
    /// bullet in single-line mode so UTF-8 length never changes caret/selection alignment;
    /// hidden multi-line fields pin the visual caret to the end of the fixed mask and map
    /// pointer hits to the end of the model.
    pub(crate) fn display_index(&self, byte: usize) -> usize {
        if self.is_hidden_multiline() {
            if self.text.is_empty() { 0 } else { MULTILINE_SECURE_MASK.len() }
        } else if self.secure && !self.revealed {
            self.text[..byte.min(self.text.len())].chars().count() * '•'.len_utf8()
        } else {
            byte.min(self.text.len())
        }
    }
    pub(crate) fn model_index(&self, byte: usize) -> usize {
        if self.is_hidden_multiline() {
            self.text.len()
        } else if self.secure && !self.revealed {
            self.text.char_indices().nth(byte / '•'.len_utf8()).map_or(self.text.len(), |(i, _)| i)
        } else {
            byte.min(self.text.len())
        }
    }
    pub(crate) fn display_selection_range(&self, a: usize, b: usize) -> (usize, usize) {
        if self.is_hidden_multiline() {
            if a == b || self.text.is_empty() { (0, 0) } else { (0, MULTILINE_SECURE_MASK.len()) }
        } else {
            (self.display_index(a), self.display_index(b))
        }
    }

    // A fixed mask has no visible interior positions. Preserve the revealed selection until
    // an edit occurs, then treat it as either the whole secret or an insertion at its end.
    fn normalize_hidden_selection(&mut self) {
        if self.is_hidden_multiline() {
            let selected = self.anchor != self.focus;
            self.focus = self.text.len();
            self.anchor = if selected { 0 } else { self.focus };
        }
    }

    /// Clears the draft and its history after submission, preserving keyboard focus.
    pub fn clear(&mut self) {
        self.replace_text("");
    }

    /// The selection as ordered byte offsets, clamped to character boundaries of `text` (an app
    /// may have replaced `text` without moving the caret).
    pub fn range(&self) -> (usize, usize) {
        let t = &self.text;
        (Self::clamp_char_boundary(t, self.anchor.min(self.focus)), Self::clamp_char_boundary(t, self.anchor.max(self.focus)))
    }

    /// Move the caret to `byte` (clamped to a character boundary of `self.text`) and collapse the
    /// selection, clearing any pending typing style.
    pub fn set_caret(&mut self, byte: usize) {
        let b = Self::clamp_char_boundary(&self.text, byte);
        self.anchor = b;
        self.focus = b;
        self.snap_to_pills();
        self.pending = None;
    }

    /// Set the selection `anchor` and `focus` byte indices (clamped to character boundaries of
    /// `self.text`), clearing any pending typing style. A selection that ends inside a
    /// [`Mark::Pill`] grows to cover it.
    pub fn set_selection(&mut self, anchor: usize, focus: usize) {
        self.anchor = Self::clamp_char_boundary(&self.text, anchor);
        self.focus = Self::clamp_char_boundary(&self.text, focus);
        self.snap_to_pills();
        self.pending = None;
    }

    /// The [`Mark::Pill`]s in `self.text`, in order, with their byte ranges and ids.
    pub fn pills(&self) -> impl Iterator<Item = (Range<usize>, &str)> + '_ {
        let t = &self.text;
        self.marks.iter().filter_map(move |m| {
            let id = m.mark.pill_id()?;
            (m.start < m.end && m.end <= t.len() && t.is_char_boundary(m.start) && t.is_char_boundary(m.end) && !t.as_bytes()[m.start..m.end].contains(&b'\n'))
                .then(|| (m.range(), id))
        })
    }

    /// The start of the pill that ends at or contains `j`, when `j` is past its start.
    fn pill_before(&self, j: usize) -> Option<usize> {
        self.marks.iter().find(|m| m.mark.is_pill() && m.start < j && j <= m.end).map(|m| m.start)
    }

    /// The end of the pill that starts at or contains `j`, when `j` is before its end.
    fn pill_after(&self, j: usize) -> Option<usize> {
        self.marks.iter().find(|m| m.mark.is_pill() && m.start <= j && j < m.end).map(|m| m.end)
    }

    fn prev_word(&self, i: usize) -> usize {
        if self.secure && !self.revealed {
            return 0;
        }
        let s = &self.text;
        let b = s.as_bytes();
        let mut j = i.min(b.len());
        loop {
            if let Some(start) = self.pill_before(j) {
                return start;
            }
            if j == 0 || !b[j - 1].is_ascii_whitespace() {
                break;
            }
            j -= 1;
        }
        while j > 0 && !b[j - 1].is_ascii_whitespace() {
            if self.marks.iter().any(|m| m.mark.is_pill() && m.end == j) {
                break;
            }
            j = prev_char(s, j);
        }
        j
    }

    fn next_word(&self, i: usize) -> usize {
        if self.secure && !self.revealed {
            return self.text.len();
        }
        let s = &self.text;
        let b = s.as_bytes();
        let mut j = i.min(b.len());
        loop {
            if let Some(end) = self.pill_after(j) {
                return end;
            }
            if j >= b.len() || !b[j].is_ascii_whitespace() {
                break;
            }
            j += 1;
        }
        while j < b.len() && !b[j].is_ascii_whitespace() {
            if self.marks.iter().any(|m| m.mark.is_pill() && m.start == j) {
                break;
            }
            j = next_char(s, j);
        }
        j
    }

    /// Replace the selection with `label` as one [`Mark::Pill`] carrying `id`, and put the
    /// caret after it: one undo step. Line breaks in `label` become spaces.
    pub fn insert_pill(&mut self, id: impl Into<String>, label: &str) {
        let (a, b) = self.range();
        self.replace_with_pill(a..b, id, label);
    }

    /// Replace byte range `range` with `label` as one [`Mark::Pill`] carrying `id` (an `@query`
    /// the user typed, say), and put the caret after it: one undo step. The range grows to
    /// cover any pill it cuts into. Line breaks in `label` become spaces; an empty label or a
    /// secure field inserts nothing.
    pub fn replace_with_pill(&mut self, range: Range<usize>, id: impl Into<String>, label: &str) {
        let label: String = label.chars().map(|c| if c.is_control() || matches!(c, '\u{2028}' | '\u{2029}') { ' ' } else { c }).collect();
        if self.secure || label.is_empty() {
            return;
        }
        self.sync_invariants();
        self.break_history_group();
        let before = self.snapshot();
        let a = Self::clamp_char_boundary(&self.text, range.start.min(range.end));
        let b = Self::clamp_char_boundary(&self.text, range.end.max(range.start));
        let (a, b) = self.cover_pills(a, b);
        self.pending = None;
        self.apply_text_and_attr_replace((a, b), &label, &[], None, false);
        // Code blocks keep no inline marks: there the label stays plain text.
        self.marks.push(MarkRange { start: a, end: a + label.len(), mark: Mark::Pill(id.into()) });
        self.normalize_marks();
        self.record_change(before, EditKind::Other);
    }

    /// The pill strictly containing byte `i` (`start < i < end`).
    fn pill_around(&self, i: usize) -> Option<(usize, usize)> {
        self.marks.iter().find(|m| m.mark.is_pill() && m.start < i && i < m.end).map(|m| (m.start, m.end))
    }

    /// `i` moved out of any pill it lands inside, to its end when `forward`, else its start.
    fn out_of_pill(&self, i: usize, forward: bool) -> usize {
        self.pill_around(i).map_or(i, |(s, e)| if forward { e } else { s })
    }

    /// `a..b` grown to cover any pill either end cuts into; an empty range inside one moves
    /// to its end.
    fn cover_pills(&self, a: usize, b: usize) -> (usize, usize) {
        if a == b {
            let i = self.out_of_pill(a, true);
            return (i, i);
        }
        (self.out_of_pill(a, false), self.out_of_pill(b, true))
    }

    /// Keeps the caret off a pill's inside: a caret goes to the nearer edge, a selection grows
    /// to cover the pill.
    pub(crate) fn snap_to_pills(&mut self) {
        if !self.marks.iter().any(|m| m.mark.is_pill()) {
            return;
        }
        if self.anchor == self.focus {
            if let Some((s, e)) = self.pill_around(self.focus) {
                let to = if self.focus - s < e - self.focus { s } else { e };
                (self.anchor, self.focus) = (to, to);
            }
        } else {
            let forward = self.focus > self.anchor;
            self.anchor = self.out_of_pill(self.anchor, !forward);
            self.focus = self.out_of_pill(self.focus, forward);
        }
    }

    pub fn selected(&self) -> &str {
        let (a, b) = self.range();
        &self.text[a..b]
    }

    pub fn insert(&mut self, s: &str) {
        self.replace(self.range(), s, EditKind::Other);
    }

    pub(crate) fn set_single_line(&mut self, single_line: bool) {
        self.line_mode = if single_line { LineMode::Single } else { LineMode::Multi };
    }

    /// Line index (`0..line_count`) containing `byte`.
    pub fn line_of_byte(&self, byte: usize) -> usize {
        let clamped = byte.min(self.text.len());
        count_newlines(&self.text.as_bytes()[..clamped])
    }

    /// Byte range (`start..end`, excluding `\n`) of 0-based `line_idx`.
    pub fn line_range(&self, line_idx: usize) -> Range<usize> {
        let mut cur = 0;
        let mut start = 0;
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'\n' {
                if cur == line_idx {
                    return start..i;
                }
                start = i + 1;
                cur += 1;
            }
        }
        if cur == line_idx {
            start..self.text.len()
        } else {
            self.text.len()..self.text.len()
        }
    }

    fn clamp_char_boundary(s: &str, idx: usize) -> usize {
        s.floor_char_boundary(idx)
    }

    /// Text typed, pasted or committed into this field. Single-line fields drop control
    /// characters, line breaks included (stripped, as `<input type=text>` does); multi-line
    /// fields fold CRLF to LF (and multi-line secure fields also drop control characters other
    /// than `\n` and `\t`). Sized up front so no partial copy of a secret is left behind by a
    /// reallocation.
    fn sanitize(&self, s: &str) -> Zeroizing<String> {
        let mut out = Zeroizing::new(String::with_capacity(s.len()));
        if self.is_single_line() {
            out.extend(s.chars().filter(|c| !c.is_control() && !matches!(c, '\u{2028}' | '\u{2029}')));
        } else if self.secure {
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' {
                    if chars.peek() != Some(&'\n') {
                        out.push('\n');
                    }
                    continue;
                }
                if (!c.is_control() || matches!(c, '\n' | '\t')) && !matches!(c, '\u{2028}' | '\u{2029}') {
                    out.push(c);
                }
            }
        } else {
            let mut chars = s.chars().peekable();
            while let Some(c) = chars.next() {
                if c == '\r' && chars.peek() == Some(&'\n') { continue; }
                out.push(c);
            }
        }
        out
    }

    /// Core replacement engine updating `text`, `marks`, `blocks`, and `soft_breaks` together.
    fn apply_text_and_attr_replace(
        &mut self,
        (a, b): (usize, usize),
        s: &str,
        inserted_marks: &[Mark],
        new_line_block: Option<Block>,
        mark_new_lines_soft: bool,
    ) {
        // Callers sync first; a second whole-text pass here would show on every keystroke.
        let del_len = b - a;
        let ins_len = s.len();
        if self.marks.is_empty()
            && inserted_marks.is_empty()
            && self.soft_breaks.is_empty()
            && !mark_new_lines_soft
            && new_line_block.is_none_or(|b| b == Block::Paragraph)
            && self.blocks.iter().all(|&b| b == Block::Paragraph)
        {
            // Plain text: no per-line passes, so a keystroke in a long plain field costs what
            // it did before blocks and marks existed.
            let removed = count_newlines(&self.text.as_bytes()[a..b]);
            let added = count_newlines(s.as_bytes());
            self.text.replace_range(a..b, s);
            self.blocks.resize(self.blocks.len() + added - removed, Block::Paragraph);
            self.anchor = a + ins_len;
            self.focus = self.anchor;
            self.cancel_composition();
            return;
        }
        let line_a = self.line_of_byte(a);
        let line_b = self.line_of_byte(b);
        let line_a_range = self.line_range(line_a);
        let line_b_range = self.line_range(line_b);

        // Determine the surviving block for `line_a` when `a..b` merges `line_a..=line_b`.
        let merged_block = if line_a < line_b
            && line_a_range.is_empty()
            && a == line_a_range.start
            && b == line_b_range.start
        {
            self.blocks.get(line_b).copied().unwrap_or(Block::Paragraph)
        } else {
            self.blocks.get(line_a).copied().unwrap_or(Block::Paragraph)
        };
        let merged_soft = if line_a < line_b
            && line_a_range.is_empty()
            && a == line_a_range.start
            && b == line_b_range.start
        {
            self.soft_breaks.binary_search(&(line_b as u32)).is_ok() && line_a > 0
        } else {
            self.soft_breaks.binary_search(&(line_a as u32)).is_ok()
        };

        let removed_lines = line_b - line_a;
        let added_lines = count_newlines(s.as_bytes());
        let next_line_blk = new_line_block.unwrap_or(match merged_block {
            Block::Code => Block::Code,
            _ => Block::Paragraph,
        });

        // Update `blocks` for removed and added lines.
        if removed_lines > 0 {
            self.blocks.drain(line_a + 1..=line_b);
        }
        if let Some(slot) = self.blocks.get_mut(line_a) {
            *slot = merged_block;
        }
        if added_lines > 0 {
            let insert_pos = (line_a + 1).min(self.blocks.len());
            self.blocks.splice(insert_pos..insert_pos, std::iter::repeat_n(next_line_blk, added_lines));
        }

        // Update `soft_breaks`.
        let mut next_soft = Vec::new();
        for &sb in &self.soft_breaks {
            let idx = sb as usize;
            if idx < line_a {
                next_soft.push(sb);
            } else if idx > line_b {
                let shifted = idx - removed_lines + added_lines;
                if shifted > 0 {
                    next_soft.push(shifted as u32);
                }
            }
        }
        if merged_soft && line_a > 0 {
            next_soft.push(line_a as u32);
        }
        if mark_new_lines_soft && added_lines > 0 {
            for k in 1..=added_lines {
                next_soft.push((line_a + k) as u32);
            }
        }
        self.soft_breaks = next_soft;

        // Shift and split existing inline marks around `a..b`.
        let mut updated_marks = Vec::with_capacity(self.marks.len() + inserted_marks.len());
        for m in self.marks.drain(..) {
            if m.end <= a {
                updated_marks.push(m);
            } else if m.start >= b {
                updated_marks.push(MarkRange {
                    start: m.start - del_len + ins_len,
                    end: m.end - del_len + ins_len,
                    mark: m.mark,
                });
            } else if m.mark.is_pill() {
                // An edit that cuts into a pill leaves its remains as plain text.
            } else {
                // `m` overlaps `a..b` (or touches inside it).
                if m.start < a {
                    updated_marks.push(MarkRange {
                        start: m.start,
                        end: a,
                        mark: m.mark.clone(),
                    });
                }
                if m.end > b {
                    updated_marks.push(MarkRange {
                        start: a + ins_len,
                        end: m.end - del_len + ins_len,
                        mark: m.mark,
                    });
                }
            }
        }
        if ins_len > 0 {
            // Typed or replacing text never becomes a pill.
            for im in inserted_marks.iter().filter(|m| !m.is_pill()) {
                // Avoid wrapping a pure newline at the trailing edge of a mark unless inside the mark.
                if s == "\n" {
                    let was_inside = updated_marks.iter().any(|m| m.mark == *im && m.start >= a + ins_len);
                    if !was_inside {
                        continue;
                    }
                }
                updated_marks.push(MarkRange {
                    start: a,
                    end: a + ins_len,
                    mark: im.clone(),
                });
            }
        }
        self.marks = updated_marks;
        self.text.replace_range(a..b, s);
        self.anchor = a + ins_len;
        self.focus = self.anchor;
        self.cancel_composition();
        self.sync_invariants();
        self.mark_edited();
    }

    fn replace(&mut self, (mut a, mut b): (usize, usize), s: &str, kind: EditKind) {
        if self.is_hidden_multiline() && self.is_pristine() {
            (a, b) = (0, self.text.len());
        }
        if self.secure {
            let mut next = String::with_capacity(self.text.len() - (b - a) + s.len());
            next.push_str(&self.text[..a]);
            next.push_str(s);
            next.push_str(&self.text[b..]);
            self.replace_text(next);
            self.anchor = a + s.len();
            self.focus = self.anchor;
            self.mark_edited();
            return;
        }
        self.sync_invariants();
        // Backspace or Delete next to a pill, or a selection into one, takes it whole.
        let (a, b) = self.cover_pills(a, b);
        if kind == EditKind::Other { self.break_history_group(); }
        if &self.text[a..b] == s
            && self.pending.is_none()
            && !self.marks.iter().any(|m| m.mark.is_pill() && m.start < b && a < m.end)
        {
            self.anchor = a + s.len();
            self.focus = self.anchor;
            self.cancel_composition();
            self.mark_edited();
            return;
        }
        let before = self.snapshot();
        let inserted_marks: Vec<Mark> = if s.is_empty() {
            self.pending = None;
            Vec::new()
        } else if let Some((armed, p)) = self.pending.take()
            && armed == (self.anchor, self.focus)
        {
            p
        } else if a < b {
            // Replacing a selection inherits marks that covered the first character of the selection.
            let line_idx = self.line_of_byte(a);
            if self.blocks.get(line_idx) == Some(&Block::Code) {
                Vec::new()
            } else {
                self.marks
                    .iter()
                    .filter(|m| m.start <= a && a < m.end)
                    .map(|m| m.mark.clone())
                    .collect()
            }
        } else {
            self.inherited_marks_at(a)
        };

        self.apply_text_and_attr_replace((a, b), s, &inserted_marks, None, false);
        self.record_change(before, kind);
    }

}

/// `\n` bytes in `s`. Counts into `u8` lanes so it vectorizes: `filter().count()` widens every
/// byte to `usize` and runs several times slower on a long field.
fn count_newlines(s: &[u8]) -> usize {
    s.chunks(255).map(|c| c.iter().fold(0u8, |n, &b| n + (b == b'\n') as u8) as usize).sum()
}

#[cfg(test)]
#[path = "edit/tests.rs"]
mod tests;
