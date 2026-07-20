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
// A buffer line wraps into `ceil(display_width / cols)` VISUAL rows
// ("segments"). Segment cuts happen on char boundaries at display
// columns `k * cols` with `byte_for_display_col` semantics: a char
// whose cell span straddles the boundary becomes the FIRST char of the
// next segment (so it may render slightly past the wrap column — only
// tabs can straddle, and only when `cols` isn't a tab-stop multiple).
// `cols == 0` means NoWrap everywhere below (identity: 1 row/line).

/// Visual rows `line` occupies wrapped at `cols` text columns.
pub fn wrap_rows(line: &str, cols: usize, tab: usize) -> usize {
    if cols == 0 {
        return 1;
    }
    display_width(line, tab).div_ceil(cols).max(1)
}

/// Start byte + start display column of every wrap segment of `line`,
/// in one pass. Always at least one entry `(0, 0)`; the entry count
/// equals [`wrap_rows`].
pub fn wrap_segment_starts(
    line: &str,
    cols: usize,
    tab: usize,
) -> Vec<(usize, usize)> {
    let mut starts = vec![(0usize, 0usize)];
    if cols == 0 {
        return starts;
    }
    let tab = tab.max(1);
    let mut col = 0usize;
    for (i, c) in line.char_indices() {
        let width = if c == '\t' { tab - (col % tab) } else { 1 };
        // This char is `byte_for_display_col(line, k * cols)` for every
        // boundary its span covers — it opens those segments.
        while col + width > starts.len() * cols {
            starts.push((i, col));
        }
        col += width;
    }
    starts
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
    let starts = wrap_segment_starts(line, cols, tab);
    let seg = starts
        .partition_point(|(start, _)| *start <= byte.min(line.len()))
        .saturating_sub(1);
    (seg, col.saturating_sub(starts[seg].1))
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
    /// Text columns the index was built for; 0 = NoWrap (identity).
    cols: usize,
}

impl WrapIndex {
    pub fn build(lines: &[String], cols: usize, tab: usize) -> Self {
        let mut row_of_line = Vec::with_capacity(lines.len() + 1);
        let mut acc = 0u32;
        for line in lines {
            row_of_line.push(acc);
            acc += wrap_rows(line, cols, tab) as u32;
        }
        row_of_line.push(acc);
        Self { row_of_line, cols }
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
