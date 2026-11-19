//! Greedy, language-independent code wrapping. Continuations retain source indentation
//! (VS Code's `wrappingIndent: same`), never alignment inferred from brackets.

use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

fn layout_clusters(line: &str) -> impl Iterator<Item = (usize, &str)> {
    let ascii = line.bytes().all(|byte| byte == b'\t' || (b' '..=b'~').contains(&byte));
    let mut unicode = (!ascii).then(|| line.grapheme_indices(true));
    let mut byte = 0;
    std::iter::from_fn(move || {
        if let Some(unicode) = unicode.as_mut() { return unicode.next(); }
        if byte == line.len() { return None; }
        let offset = byte; byte += 1;
        Some((offset, &line[offset..byte]))
    })
}

fn cluster_width(cluster: &str, col: usize, tab: usize) -> usize {
    if cluster == "\t" {
        tab.max(1) - col % tab.max(1)
    } else if cluster.len() == 1 && (b' '..=b'~').contains(&cluster.as_bytes()[0]) {
        1
    } else {
        UnicodeWidthStr::width(cluster)
    }
}

pub const TAB_DISPLAY_WIDTH: usize = 4;

/// Display column of a byte offset; offsets inside a grapheme snap to its start.
pub fn display_col_for_byte(line: &str, byte: usize, tab: usize) -> usize {
    display_col_for_byte_from(line, byte, 0, tab)
}

/// A segment already knows its source column: do not rescan the logical line.
pub fn display_col_for_byte_from(line: &str, byte: usize, start_col: usize, tab: usize) -> usize {
    if line.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return start_col + byte.min(line.len());
    }
    let mut col = start_col;
    for (i, cluster) in layout_clusters(line) {
        if i >= byte || i + cluster.len() > byte { break; }
        col += cluster_width(cluster, col, tab);
    }
    col
}

/// Byte whose cell contains `target`; clamps to the end past the last cell.
pub fn byte_for_display_col(line: &str, target: usize, tab: usize) -> usize {
    let mut col = 0;
    for (i, cluster) in layout_clusters(line) {
        let width = cluster_width(cluster, col, tab);
        if col + width > target { return i; }
        col += width;
    }
    line.len()
}

pub fn expand_tabs_from(slice: &str, start_col: usize, tab: usize) -> String {
    if !slice.contains('\t') { return slice.to_string(); }
    let mut out = String::with_capacity(slice.len());
    let mut col = start_col;
    for cluster in layout_clusters(slice).map(|(_, cluster)| cluster) {
        let width = cluster_width(cluster, col, tab);
        if cluster == "\t" { out.extend(std::iter::repeat_n(' ', width)); }
        else { out.push_str(cluster); }
        col += width;
    }
    out
}

pub fn display_width(line: &str, tab: usize) -> usize {
    display_col_for_byte(line, line.len(), tab)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WrapSegment {
    pub byte_start: usize,
    pub byte_end: usize,
    pub source_col: usize,
    pub source_end_col: usize,
    pub visual_indent: usize,
}

impl WrapSegment {
    pub fn source_col_for_byte(self, line: &str, byte: usize, tab: usize) -> usize {
        display_col_for_byte_from(&line[self.byte_start..self.byte_end],
            byte.clamp(self.byte_start, self.byte_end) - self.byte_start, self.source_col, tab)
    }
    pub fn visual_col(self, source_col: usize) -> usize {
        self.visual_indent + source_col.saturating_sub(self.source_col)
    }
    pub fn source_col_at_visual(self, visual_col: usize) -> usize {
        self.source_col + visual_col.saturating_sub(self.visual_indent)
    }
    /// Shared caret affinity: an internal break belongs to the next row, EOL to the last.
    pub fn contains_caret(self, byte: usize, line_len: usize) -> bool {
        byte >= self.byte_start && (byte < self.byte_end || self.byte_end == line_len)
    }
    /// Hit/movement mapping is local to this segment, including clicks in the
    /// synthetic indent or beyond its right edge. Never land inside a grapheme
    /// or on the next row just because the pointer passed this row's last cell.
    pub fn byte_at_visual(self, line: &str, visual: usize, tab: usize) -> usize {
        let target = self.source_col_at_visual(visual);
        let mut col = self.source_col;
        let mut last = self.byte_start;
        for (relative, cluster) in layout_clusters(&line[self.byte_start..self.byte_end]) {
            last = self.byte_start + relative;
            let width = cluster_width(cluster, col, tab);
            if col + width > target { return last; }
            col += width;
        }
        if self.byte_end == line.len() { self.byte_end } else { last }
    }
}

// VS Code's default break-before/after classes, not syntax-aware token priorities.
// Runs of separators stay together; spaces belong to the preceding word. NBSP is
// deliberately not a boundary. CJK allows word wrapping without ASCII spaces.
fn break_before(ch: char) -> bool { "([{‘“〈《「『【〔（［｛｢£¥＄￡￥+＋".contains(ch) }
fn break_after(ch: char) -> bool { " \t})]?|/&.,;¢°′″‰℃、。｡､￠，．：；？！％・･ゝゞヽヾーァィゥェォッャュョヮヵヶぁぃぅぇぉっゃゅょゎゕゖㇰㇱㇲㇳㇴㇵㇶㇷㇸㇹㇺㇻㇼㇽㇾㇿ々〻ｧｨｩｪｫｬｭｮｯｰ”〉》」』】〕）］｝｣".contains(ch) }
fn ideographic(ch: char) -> bool {
    matches!(ch as u32, 0x3040..=0x30ff | 0x3400..=0x4dbf | 0x4e00..=0x9fff)
}
fn can_break(previous: char, next: char) -> bool {
    next != ' ' && ((break_after(previous) && !break_after(next))
        || (!break_before(previous) && break_before(next))
        || (ideographic(previous) && !break_after(next))
        || (ideographic(next) && !break_before(previous)))
}

/// Full geometry, computed only on text/width changes by CodeCache. Each cluster
/// is measured once. Prefix columns also avoid quadratic scans of huge tokens.
pub fn wrap_segments(line: &str, cols: usize, tab: usize) -> Vec<WrapSegment> {
    let mut cells = Vec::new();
    let mut col = 0;
    let mut leading = 0;
    let mut in_indent = true;
    for (byte, cluster) in layout_clusters(line) {
        cells.push((byte, col, cluster.chars().next().unwrap()));
        col += cluster_width(cluster, col, tab);
        if in_indent && matches!(cluster, " " | "\t") { leading = col; }
        else { in_indent = false; }
    }
    let line_width = col;
    if cols == 0 || line_width <= cols {
        return vec![WrapSegment { byte_start: 0, byte_end: line.len(), source_col: 0, source_end_col: line_width, visual_indent: 0 }];
    }
    cells.push((line.len(), line_width, '\0'));
    // As in VS Code, abandon the synthetic indent if it leaves less than one
    // fullwidth character. Whitespace-only lines have no continuation indent.
    let indent = if !in_indent && leading.saturating_add(2) <= cols { leading } else { 0 };
    let mut segments = Vec::new();
    let mut start = 0;
    while start + 1 < cells.len() {
        let visual_indent = if start == 0 { 0 } else { indent };
        let limit = cells[start].1 + cols.saturating_sub(visual_indent).max(1);
        let mut end = start;
        let mut candidate = None;
        while end + 1 < cells.len() && (cells[end + 1].1 <= limit || end == start) {
            end += 1;
            if end + 1 < cells.len() && can_break(cells[end - 1].2, cells[end].2) {
                candidate = Some(end);
            }
        }
        // Greedy: latest eligible boundary that fits, regardless of punctuation.
        // A word wider than a row is hard-wrapped without splitting a grapheme.
        if end + 1 < cells.len() {
            // Don't emit an indentation-only first row when the overflowing
            // word would not fit on the continuation either.
            let next_limit = cols.saturating_sub(indent).max(1);
            end = candidate.filter(|&at| cells[end + 1].1 - cells[at].1 <= next_limit).unwrap_or(end);
        }
        segments.push(WrapSegment { byte_start: cells[start].0, byte_end: cells[end].0,
            source_col: cells[start].1, source_end_col: cells[end].1, visual_indent });
        start = end;
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_boundaries_inside_strings_comments_and_after_punctuation() {
        for line in ["alpha, beta gamma delta epsilon", "\"alpha beta gamma delta epsilon\"", "// alpha beta gamma delta epsilon"] {
            let rows = wrap_segments(line, 23, 4);
            let first = &line[..rows[0].byte_end];
            assert!(first.len() > 15, "must not prefer early punctuation: {first:?}");
            assert!(first.ends_with(' '), "quoted words wrap too: {first:?}");
            assert_eq!(rows.iter().map(|s| &line[s.byte_start..s.byte_end]).collect::<String>(), line);
        }
    }

    #[test]
    fn continuation_indent_is_source_indent_not_bracket_alignment() {
        for line in ["    call(alpha, nested(beta, gamma), delta, epsilon, zeta)", "\tlet s = \"alpha beta gamma delta epsilon zeta eta theta\";"] {
            let rows = wrap_segments(line, 24, 4);
            assert!(rows.len() > 2);
            assert_eq!(rows[0].visual_indent, 0);
            assert!(rows[1..].iter().all(|s| s.visual_indent == 4));
        }
        assert!(wrap_segments("unindented words words words", 12, 4).iter().all(|s| s.visual_indent == 0));
        assert!(wrap_segments("            deeply indented text", 12, 4).iter().all(|s| s.visual_indent == 0));
    }

    #[test]
    fn geometry_is_lossless_grapheme_safe_and_has_unique_caret_affinity() {
        for line in ["", "\talpha beta\t世界 café e\u{301} 👩‍💻 more words", "identifier_without_any_breaks_0123456789", "日本語の長い文字列です。次の文です。", "a\u{a0}b c d", "          "] {
            for cols in [0, 1, 2, 7, 12, 24, 80] {
                let rows = wrap_segments(line, cols, 4);
                assert_eq!(rows.iter().map(|s| &line[s.byte_start..s.byte_end]).collect::<String>(), line);
                for (byte, _) in layout_clusters(line).chain(std::iter::once((line.len(), ""))) {
                    assert_eq!(rows.iter().filter(|s| s.contains_caret(byte, line.len())).count(), 1);
                }
                for s in rows {
                    assert_eq!(s.source_col, display_col_for_byte(line, s.byte_start, 4));
                    assert_eq!(s.source_end_col, display_col_for_byte(line, s.byte_end, 4));
                    let width = s.source_end_col - s.source_col;
                    let first = layout_clusters(&line[s.byte_start..s.byte_end]).next();
                    assert!(cols == 0 || width + s.visual_indent <= cols || first.is_some_and(|(_, c)| cluster_width(c, s.source_col, 4) > cols.saturating_sub(s.visual_indent)));
                }
            }
        }
    }

    #[test]
    fn tabs_and_graphemes_keep_source_columns_and_hit_affinity() {
        assert_eq!(display_width("a\t界e\u{301}", 4), 7);
        assert_eq!(expand_tabs_from("\t界\tx", 2, 4), "  界  x");
        assert_eq!(byte_for_display_col("\t界", 5, 4), 1);
        assert_eq!(display_col_for_byte("e\u{301}x", 1, 4), 0);
        let line = "aaaa👩‍💻bbbb";
        let rows = wrap_segments(line, 6, 4);
        assert_eq!(rows[0].byte_at_visual(line, 100, 4), 4, "right margin must land on emoji start, not its last scalar");
        assert_eq!(rows[1].byte_at_visual(line, 0, 4), rows[1].byte_start);
        assert_eq!(wrap_segments("    verylongidentifier", 12, 4)[0].byte_end, 12, "no indentation-only row before a long word");
    }
}