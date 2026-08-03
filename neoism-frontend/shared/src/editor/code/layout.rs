//! Pure cell-grid layout math for the code pane: tab expansion and
//! byte↔display-column mapping. Kept renderer-agnostic — the GUI
//! multiplies display columns by a measured cell width, a tty host
//! uses them as terminal columns directly.
//!
//! v1 counts every non-tab char as one cell (wide CJK/emoji glyphs
//! drift; measured hit-stops come later with the polish pass — the
//! draw path uses the same math, so caret and text stay consistent).

pub const TAB_DISPLAY_WIDTH: usize = 4;

/// Display column of a byte offset, expanding tabs to the next stop.
pub fn display_col_for_byte(line: &str, byte: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut col = 0usize;
    for (i, c) in line.char_indices() {
        if i >= byte {
            break;
        }
        col += if c == '\t' { tab - (col % tab) } else { 1 };
    }
    col
}

/// Byte offset whose cell contains `target` (mouse hit-testing).
/// Clamps to the line end when the click lands past the last char.
pub fn byte_for_display_col(line: &str, target: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut col = 0usize;
    for (i, c) in line.char_indices() {
        let width = if c == '\t' { tab - (col % tab) } else { 1 };
        if col + width > target {
            return i;
        }
        col += width;
    }
    line.len()
}

/// Expand a slice of a line for drawing, given the display column its
/// first byte starts at (tab stops depend on the running column).
pub fn expand_tabs_from(slice: &str, start_col: usize, tab: usize) -> String {
    let tab = tab.max(1);
    if !slice.contains('\t') {
        return slice.to_string();
    }
    let mut out = String::with_capacity(slice.len() + tab * 2);
    let mut col = start_col;
    for c in slice.chars() {
        if c == '\t' {
            let pad = tab - (col % tab);
            for _ in 0..pad {
                out.push(' ');
            }
            col += pad;
        } else {
            out.push(c);
            col += 1;
        }
    }
    out
}

/// Total display width of a line in cells.
pub fn display_width(line: &str, tab: usize) -> usize {
    display_col_for_byte(line, line.len(), tab)
}

/// Byte offset for an LSP UTF-16 column on a line (diagnostic ranges
/// arrive UTF-16-encoded; the buffer is byte-addressed).
pub fn byte_for_utf16_col(line: &str, utf16: usize) -> usize {
    let mut units = 0usize;
    for (i, c) in line.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16();
    }
    line.len()
}

/// Find the delimiter paired with the bracket at the exact buffer position.
///
/// Unlike the vim `%` motion, this never searches ahead for a bracket: it is
/// intended for pointer/caret feedback, so ordinary text under the pointer
/// must not light up an unrelated pair later on the line. Positions are byte
/// columns and the returned pair keeps the hovered/caret endpoint first.
pub(crate) fn matching_bracket_at(
    lines: &[String],
    line_ix: usize,
    byte_col: usize,
) -> Option<((usize, usize), (usize, usize))> {
    const PAIRS: [(char, char); 3] = [('(', ')'), ('[', ']'), ('{', '}')];

    let line = lines.get(line_ix)?;
    if byte_col > line.len() || !line.is_char_boundary(byte_col) {
        return None;
    }
    let bracket = line.get(byte_col..)?.chars().next()?;
    let (open, close, forward) = PAIRS.iter().find_map(|&(open, close)| {
        (bracket == open)
            .then_some((open, close, true))
            .or_else(|| (bracket == close).then_some((open, close, false)))
    })?;
    let origin = (line_ix, byte_col);
    let mut depth = 1usize;

    if forward {
        for (scan_line_ix, scan_line) in lines.iter().enumerate().skip(line_ix) {
            let start = if scan_line_ix == line_ix {
                byte_col + bracket.len_utf8()
            } else {
                0
            };
            for (relative, ch) in scan_line[start..].char_indices() {
                if ch == open {
                    depth += 1;
                } else if ch == close {
                    depth -= 1;
                    if depth == 0 {
                        return Some((origin, (scan_line_ix, start + relative)));
                    }
                }
            }
        }
    } else {
        for scan_line_ix in (0..=line_ix).rev() {
            let scan_line = &lines[scan_line_ix];
            let end = if scan_line_ix == line_ix {
                byte_col
            } else {
                scan_line.len()
            };
            for (col, ch) in scan_line[..end].char_indices().rev() {
                if ch == close {
                    depth += 1;
                } else if ch == open {
                    depth -= 1;
                    if depth == 0 {
                        return Some((origin, (scan_line_ix, col)));
                    }
                }
            }
        }
    }

    None
}

/// Gutter digit count: room for the last line number, never narrower
/// than nvim's default 3-cell `numberwidth`.
pub fn gutter_digits(line_count: usize) -> usize {
    let mut digits = 1usize;
    let mut n = line_count.max(1);
    while n >= 10 {
        digits += 1;
        n /= 10;
    }
    digits.max(3)
}

// --- soft wrap (DisplayMap-lite) ---
//
// Wraps prefer readable code boundaries (whitespace, commas, operators,
// member chains) and give continuation rows a small hanging indent. A hard
// cell cut is only used when one token is wider than the row. The resulting
// segments are the single source of truth for paint, carets, selections and
// hit-testing. `cols == 0` means NoWrap everywhere below.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub source_col: usize,
    pub source_end_col: usize,
    /// Synthetic screen-space indentation on continuation rows.
    pub visual_indent: usize,
}

impl WrapSegment {
    pub fn visual_col(self, source_col: usize) -> usize {
        self.visual_indent + source_col.saturating_sub(self.source_col)
    }

    pub fn source_col_at_visual(self, visual_col: usize) -> usize {
        self.source_col + visual_col.saturating_sub(self.visual_indent)
    }
}

#[derive(Clone, Copy)]
struct BreakCandidate {
    byte: usize,
    col: usize,
    priority: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LexState {
    Normal,
    SingleQuote,
    DoubleQuote,
    Template,
    BlockComment,
    LineComment,
}

fn break_candidates(line: &str, tab: usize) -> Vec<BreakCandidate> {
    fn is_operator(ch: char) -> bool {
        matches!(
            ch,
            '=' | '>' | '<' | '|' | '&' | '?' | '+' | '-' | '*' | '/' | '%' | '!'
        )
    }

    let tab = tab.max(1);
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut candidates = Vec::new();
    let mut state = LexState::Normal;
    let mut escaped = false;
    let mut col = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        let (byte, ch) = chars[i];
        let next = chars.get(i + 1).map(|(_, ch)| *ch);
        let previous = i
            .checked_sub(1)
            .and_then(|i| chars.get(i))
            .map(|(_, ch)| *ch);
        let width = if ch == '\t' { tab - col % tab } else { 1 };
        let next_byte = byte + ch.len_utf8();

        match state {
            LexState::Normal => {
                if ch == '/' && next == Some('/') {
                    state = LexState::LineComment;
                } else if ch == '/' && next == Some('*') {
                    state = LexState::BlockComment;
                } else if ch == '\'' {
                    state = LexState::SingleQuote;
                } else if ch == '"' {
                    state = LexState::DoubleQuote;
                } else if ch == '`' {
                    state = LexState::Template;
                } else if ch.is_whitespace() {
                    // Keep a whitespace run on the preceding visual row so
                    // continuation text begins on a real token boundary.
                    let mut end_i = i;
                    let mut end_byte = next_byte;
                    let mut end_col = col + width;
                    while let Some(&(candidate_byte, candidate_ch)) = chars.get(end_i + 1)
                    {
                        if !candidate_ch.is_whitespace() {
                            break;
                        }
                        let candidate_width = if candidate_ch == '\t' {
                            tab - end_col % tab
                        } else {
                            1
                        };
                        end_i += 1;
                        end_byte = candidate_byte + candidate_ch.len_utf8();
                        end_col += candidate_width;
                    }
                    let priority = match previous {
                        Some(',' | ';') => 6,
                        Some(':' | '=' | '>' | '|' | '&' | '?' | '+' | '-') => 5,
                        Some('(' | '[' | '{') => 4,
                        _ => 3,
                    };
                    candidates.push(BreakCandidate {
                        byte: end_byte,
                        col: end_col,
                        priority,
                    });
                    col = end_col;
                    i = end_i + 1;
                    continue;
                } else {
                    let priority = match ch {
                        ',' | ';' => 6,
                        ':' | '=' | '>' | '<' | '|' | '&' | '?' | '!' => 5,
                        '+' | '-' | '*' | '/' | '%' => 4,
                        '(' | '[' | '{' => 3,
                        _ => 0,
                    };
                    if ch == '.'
                        && !matches!(previous, Some('.' | '?'))
                        && next != Some('.')
                    {
                        // A fluent chain reads best with the dot leading the
                        // continuation (`.map`, `.await`, optional chains).
                        candidates.push(BreakCandidate {
                            byte,
                            col,
                            priority: 5,
                        });
                    } else if ch == '?' && next == Some('.') {
                        candidates.push(BreakCandidate {
                            byte,
                            col,
                            priority: 5,
                        });
                    } else if priority > 0 && !next.is_some_and(is_operator) {
                        candidates.push(BreakCandidate {
                            byte: next_byte,
                            col: col + width,
                            priority,
                        });
                    }
                }
            }
            LexState::SingleQuote | LexState::DoubleQuote | LexState::Template => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if (state == LexState::SingleQuote && ch == '\'')
                    || (state == LexState::DoubleQuote && ch == '"')
                    || (state == LexState::Template && ch == '`')
                {
                    state = LexState::Normal;
                }
            }
            LexState::BlockComment => {
                if ch.is_whitespace() {
                    candidates.push(BreakCandidate {
                        byte: next_byte,
                        col: col + width,
                        priority: 2,
                    });
                }
                if ch == '*' && next == Some('/') {
                    state = LexState::Normal;
                }
            }
            LexState::LineComment => {
                if ch.is_whitespace() {
                    candidates.push(BreakCandidate {
                        byte: next_byte,
                        col: col + width,
                        priority: 2,
                    });
                }
            }
        }

        col += width;
        i += 1;
    }
    candidates
}

fn leading_indent(line: &str, tab: usize) -> usize {
    let first_content = line
        .char_indices()
        .find_map(|(byte, ch)| (!ch.is_whitespace()).then_some(byte))
        .unwrap_or(line.len());
    display_col_for_byte(line, first_content, tab)
}

fn delimiter_snapshots(line: &str, tab: usize) -> Vec<(usize, Option<usize>)> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut snapshots = vec![(0, None)];
    let mut state = LexState::Normal;
    let mut escaped = false;
    let mut col = 0usize;
    let chars: Vec<(usize, char)> = line.char_indices().collect();
    let mut i = 0usize;
    while let Some(&(byte, ch)) = chars.get(i) {
        let next = chars.get(i + 1).map(|(_, ch)| *ch);
        match state {
            LexState::Normal => {
                if ch == '/' && next == Some('/') {
                    state = LexState::LineComment;
                } else if ch == '/' && next == Some('*') {
                    state = LexState::BlockComment;
                } else if ch == '\'' {
                    state = LexState::SingleQuote;
                } else if ch == '"' {
                    state = LexState::DoubleQuote;
                } else if ch == '`' {
                    state = LexState::Template;
                } else if matches!(ch, '(' | '[' | '{') {
                    stack.push((ch, col));
                } else if matches!(ch, ')' | ']' | '}') {
                    let expected = match ch {
                        ')' => '(',
                        ']' => '[',
                        _ => '{',
                    };
                    if stack.last().is_some_and(|(open, _)| *open == expected) {
                        stack.pop();
                    }
                }
            }
            LexState::SingleQuote | LexState::DoubleQuote | LexState::Template => {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if (state == LexState::SingleQuote && ch == '\'')
                    || (state == LexState::DoubleQuote && ch == '"')
                    || (state == LexState::Template && ch == '`')
                {
                    state = LexState::Normal;
                }
            }
            LexState::BlockComment if ch == '*' && next == Some('/') => {
                state = LexState::Normal;
            }
            LexState::BlockComment | LexState::LineComment => {}
        }
        col += if ch == '\t' {
            tab.max(1) - col % tab.max(1)
        } else {
            1
        };
        snapshots.push((byte + ch.len_utf8(), stack.last().map(|(_, col)| *col)));
        i += 1;
    }
    snapshots
}

fn continuation_indent(
    leading: usize,
    delimiter_col: Option<usize>,
    cols: usize,
    tab: usize,
) -> usize {
    // Very narrow grids need every cell and retain the old hard-wrap shape.
    if cols < 12 {
        return 0;
    }
    let base = leading.saturating_add(tab.max(2));
    let delimiter = delimiter_col.map(|col| col + 1).unwrap_or(0);
    // Deep call sites should not push useful text halfway across the pane.
    // Preserve the source indent, then allow a modest extra hanging step.
    let max_indent = (cols / 3)
        .max(leading.saturating_add(tab.saturating_mul(2)))
        .min(cols.saturating_sub(8));
    base.max(delimiter).min(max_indent)
}

/// Full wrap geometry for one buffer line.
pub fn wrap_segments(line: &str, cols: usize, tab: usize) -> Vec<WrapSegment> {
    let line_width = display_width(line, tab);
    if cols == 0 || line.is_empty() {
        return vec![WrapSegment {
            byte_start: 0,
            byte_end: line.len(),
            source_col: 0,
            source_end_col: line_width,
            visual_indent: 0,
        }];
    }

    let candidates = break_candidates(line, tab);
    let delimiter_snapshots = delimiter_snapshots(line, tab);
    let source_indent = leading_indent(line, tab);
    let mut segments = Vec::new();
    let mut byte_start = 0usize;
    let mut source_col = 0usize;
    let mut visual_indent = 0usize;

    while byte_start < line.len() {
        let available = cols.saturating_sub(visual_indent).max(1);
        if line_width.saturating_sub(source_col) <= available {
            segments.push(WrapSegment {
                byte_start,
                byte_end: line.len(),
                source_col,
                source_end_col: line_width,
                visual_indent,
            });
            break;
        }

        let limit = source_col + available;
        let mut hard_byte = byte_start;
        let mut hard_col = source_col;
        for (relative, ch) in line[byte_start..].char_indices() {
            let byte = byte_start + relative;
            let width = if ch == '\t' {
                tab.max(1) - hard_col % tab.max(1)
            } else {
                1
            };
            if hard_col + width > limit && byte > byte_start {
                break;
            }
            hard_byte = byte + ch.len_utf8();
            hard_col += width;
            if hard_col >= limit {
                break;
            }
        }
        if hard_byte == byte_start {
            // Defensive progress for malformed widths; always cut on UTF-8.
            let ch = line[byte_start..].chars().next().unwrap();
            hard_byte += ch.len_utf8();
            hard_col = display_col_for_byte(line, hard_byte, tab);
        }

        let min_pretty_col = source_col + available.saturating_mul(2) / 5;
        let best = candidates
            .iter()
            .copied()
            .filter(|candidate| {
                candidate.byte > byte_start
                    && candidate.byte <= hard_byte
                    && candidate.col >= min_pretty_col
            })
            .max_by_key(|candidate| {
                let used = candidate.col.saturating_sub(source_col);
                used + candidate.priority * available.max(8) / 8
            });
        let (byte_end, source_end_col) = best
            .map(|candidate| (candidate.byte, candidate.col))
            .unwrap_or((hard_byte, hard_col));

        segments.push(WrapSegment {
            byte_start,
            byte_end,
            source_col,
            source_end_col,
            visual_indent,
        });
        byte_start = byte_end;
        source_col = source_end_col;
        let delimiter_col = delimiter_snapshots
            .get(
                delimiter_snapshots
                    .partition_point(|(byte, _)| *byte <= byte_start)
                    .saturating_sub(1),
            )
            .and_then(|(_, col)| *col);
        visual_indent = continuation_indent(source_indent, delimiter_col, cols, tab);
    }

    if segments.is_empty() {
        segments.push(WrapSegment {
            byte_start: 0,
            byte_end: 0,
            source_col: 0,
            source_end_col: 0,
            visual_indent: 0,
        });
    }
    segments
}

/// Visual rows `line` occupies wrapped at `cols` text columns.
pub fn wrap_rows(line: &str, cols: usize, tab: usize) -> usize {
    wrap_segments(line, cols, tab).len()
}

/// Compatibility projection of every segment's source start. Always at
/// least one entry `(0, 0)`; the entry count equals [`wrap_rows`]. New paint
/// and input code should retain the complete [`WrapSegment`] geometry.
pub fn wrap_segment_starts(line: &str, cols: usize, tab: usize) -> Vec<(usize, usize)> {
    wrap_segments(line, cols, tab)
        .into_iter()
        .map(|segment| (segment.byte_start, segment.source_col))
        .collect()
}

/// (segment, display column within that segment) of a byte offset —
/// the caret math. `byte == line.len()` (EOL) stays on the segment of
/// the last char, one cell past it.
pub fn wrap_visual_position(
    line: &str,
    byte: usize,
    cols: usize,
    tab: usize,
) -> (usize, usize) {
    let col = display_col_for_byte(line, byte.min(line.len()), tab);
    if cols == 0 {
        return (0, col);
    }
    let segments = wrap_segments(line, cols, tab);
    let seg = segments
        .partition_point(|segment| segment.byte_start <= byte.min(line.len()))
        .saturating_sub(1);
    (seg, segments[seg].visual_col(col))
}

/// The wrap layout index: a prefix sum mapping buffer lines to visual
/// rows. Rebuilt only when (buffer revision, cols) changes — O(lines)
/// — and shared with the pane geometry via `Arc` so hit tests use the
/// exact layout of the painted frame. `Default` (never built) makes
/// every query fall back to the identity mapping (row == line).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WrapIndex {
    /// `row_of_line[i]` = first visual row of buffer line `i`, plus a
    /// final sentinel entry holding the total visual row count.
    row_of_line: Vec<u32>,
    segments_by_line: Vec<Vec<WrapSegment>>,
    /// Text columns the index was built for; 0 = NoWrap (identity).
    cols: usize,
}

impl WrapIndex {
    pub fn build(lines: &[String], cols: usize, tab: usize) -> Self {
        let mut row_of_line = Vec::with_capacity(lines.len() + 1);
        let mut segments_by_line = Vec::with_capacity(lines.len());
        let mut acc = 0u32;
        for line in lines {
            row_of_line.push(acc);
            let segments = wrap_segments(line, cols, tab);
            acc += segments.len() as u32;
            segments_by_line.push(segments);
        }
        row_of_line.push(acc);
        Self {
            row_of_line,
            segments_by_line,
            cols,
        }
    }

    /// Text columns of the build; 0 = NoWrap.
    pub fn cols(&self) -> usize {
        self.cols
    }

    /// The index matches a buffer of `line_count` lines (a stale or
    /// never-built index degrades to the identity mapping).
    pub fn is_valid_for(&self, line_count: usize) -> bool {
        self.row_of_line.len() == line_count + 1
    }

    /// Total visual rows of the buffer.
    pub fn total_rows(&self, line_count: usize) -> usize {
        if self.is_valid_for(line_count) {
            *self.row_of_line.last().unwrap_or(&0) as usize
        } else {
            line_count
        }
    }

    /// First visual row of `line`; `line == line_count` yields the
    /// total row count (half-open span convention).
    pub fn first_row_of_line(&self, line: usize) -> usize {
        self.row_of_line
            .get(line)
            .map(|row| *row as usize)
            .unwrap_or(line)
    }

    /// Visual rows `line` occupies.
    pub fn rows_of_line(&self, line: usize) -> usize {
        match (self.row_of_line.get(line), self.row_of_line.get(line + 1)) {
            (Some(first), Some(next)) => (*next - *first).max(1) as usize,
            _ => 1,
        }
    }

    pub fn segments_of_line(&self, line: usize) -> Option<&[WrapSegment]> {
        self.segments_by_line.get(line).map(Vec::as_slice)
    }

    pub fn segment(&self, line: usize, segment: usize) -> Option<WrapSegment> {
        self.segments_by_line.get(line)?.get(segment).copied()
    }

    pub fn visual_position(
        &self,
        line_ix: usize,
        line: &str,
        byte: usize,
        tab: usize,
    ) -> (usize, usize) {
        let Some(segments) = self.segments_of_line(line_ix) else {
            return wrap_visual_position(line, byte, self.cols, tab);
        };
        let byte = byte.min(line.len());
        let source_col = display_col_for_byte(line, byte, tab);
        let segment = segments
            .partition_point(|segment| segment.byte_start <= byte)
            .saturating_sub(1);
        (segment, segments[segment].visual_col(source_col))
    }

    /// Map a visual row to (buffer line, segment), clamped to the last
    /// row of the buffer.
    pub fn line_of_row(&self, vrow: usize, line_count: usize) -> (usize, usize) {
        if line_count == 0 {
            return (0, 0);
        }
        if !self.is_valid_for(line_count) {
            return (vrow.min(line_count - 1), 0);
        }
        let total = *self.row_of_line.last().unwrap_or(&0) as usize;
        let vrow = vrow.min(total.saturating_sub(1)) as u32;
        let line = self
            .row_of_line
            .partition_point(|row| *row <= vrow)
            .saturating_sub(1)
            .min(line_count - 1);
        (line, (vrow - self.row_of_line[line]) as usize)
    }
}
