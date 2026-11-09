//! Soft wrap for code (`shared/src/editor/code/layout.rs`): breaks prefer separators outside
//! strings and comments, and continuation rows hang at the open bracket or one indent deeper.

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

/// Display column of a byte offset, expanding tabs to the next stop.
/// Offsets inside a grapheme snap to its beginning.
pub fn display_col_for_byte(line: &str, byte: usize, tab: usize) -> usize {
    display_col_for_byte_from(line, byte, 0, tab)
}

/// Map within a slice that starts at a known source column. Wrap segments
/// already store that column; rescanning the entire line is unnecessary.
pub fn display_col_for_byte_from(line: &str, byte: usize, start_col: usize, tab: usize) -> usize {
    // Printable ASCII has one cell per byte. Keep tabs, controls and Unicode
    // on the grapheme-aware path, including offsets within a CRLF cluster.
    if line.bytes().all(|byte| (b' '..=b'~').contains(&byte)) {
        return start_col + byte.min(line.len());
    }
    let tab = tab.max(1);
    let mut col = start_col;
    for (i, cluster) in layout_clusters(line) {
        if i >= byte || i + cluster.len() > byte {
            break;
        }
        col += cluster_width(cluster, col, tab);
    }
    col
}

/// Byte offset whose cell contains `target` (mouse hit-testing).
/// Clamps to the line end when the click lands past the last char.
pub fn byte_for_display_col(line: &str, target: usize, tab: usize) -> usize {
    let tab = tab.max(1);
    let mut col = 0usize;
    for (i, cluster) in layout_clusters(line) {
        let width = cluster_width(cluster, col, tab);
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
    for cluster in slice.graphemes(true) {
        if cluster == "\t" {
            let pad = tab - (col % tab);
            for _ in 0..pad {
                out.push(' ');
            }
            col += pad;
        } else {
            out.push_str(cluster);
            col += cluster_width(cluster, col, tab);
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
    pub fn source_col_for_byte(self, line: &str, byte: usize, tab: usize) -> usize {
        let byte = byte.clamp(self.byte_start, self.byte_end);
        display_col_for_byte_from(
            &line[self.byte_start..self.byte_end],
            byte - self.byte_start,
            self.source_col,
            tab,
        )
    }

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

fn break_candidates(clusters: &[(usize, &str)], chars: &[(usize, char)], tab: usize) -> Vec<BreakCandidate> {
    fn is_operator(ch: char) -> bool {
        matches!(
            ch,
            '=' | '>' | '<' | '|' | '&' | '?' | '+' | '-' | '*' | '/' | '%' | '!'
        )
    }

    let tab = tab.max(1);
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
        let width = cluster_width(clusters[i].1, col, tab);
        let next_byte = byte + clusters[i].1.len();

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
                        let candidate_width =
                            cluster_width(clusters[end_i + 1].1, end_col, tab);
                        end_i += 1;
                        end_byte = candidate_byte + clusters[end_i].1.len();
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

fn delimiter_snapshots(clusters: &[(usize, &str)], chars: &[(usize, char)], tab: usize) -> Vec<(usize, Option<usize>)> {
    let mut stack: Vec<(char, usize)> = Vec::new();
    let mut snapshots = vec![(0, None)];
    let mut state = LexState::Normal;
    let mut escaped = false;
    let mut col = 0usize;
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
        col += cluster_width(clusters[i].1, col, tab);
        snapshots.push((
            byte + clusters[i].1.len(),
            stack.last().map(|(_, col)| *col),
        ));
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
    if cols == 0 || line_width <= cols {
        return vec![WrapSegment {
            byte_start: 0,
            byte_end: line.len(),
            source_col: 0,
            source_end_col: line_width,
            visual_indent: 0,
        }];
    }

    let clusters: Vec<(usize, &str)> = layout_clusters(line).collect();
    let chars: Vec<(usize, char)> = clusters
        .iter()
        .map(|(byte, cluster)| (*byte, cluster.chars().next().unwrap()))
        .collect();
    let candidates = break_candidates(&clusters, &chars, tab);
    let delimiter_snapshots = delimiter_snapshots(&clusters, &chars, tab);
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
        for (relative, cluster) in layout_clusters(&line[byte_start..]) {
            let byte = byte_start + relative;
            let width = cluster_width(cluster, hard_col, tab);
            if hard_col + width > limit && byte > byte_start {
                break;
            }
            hard_byte = byte + cluster.len();
            hard_col += width;
            if hard_col >= limit {
                break;
            }
        }
        if hard_byte == byte_start {
            // Defensive progress; never split a grapheme cluster.
            let cluster = line[byte_start..].graphemes(true).next().unwrap();
            hard_byte += cluster.len();
            hard_col = display_col_for_byte(line, hard_byte, tab);
        }

        let min_pretty_col = source_col + available.saturating_mul(2) / 5;
        let candidate_start = candidates.partition_point(|candidate| candidate.byte <= byte_start);
        let candidate_end = candidates.partition_point(|candidate| candidate.byte <= hard_byte);
        let best = candidates[candidate_start..candidate_end]
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
