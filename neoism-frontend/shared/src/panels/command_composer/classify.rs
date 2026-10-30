//! Input classification + wrap helpers.
//!
//! - `styled_spans` walks the buffered command and assigns each byte
//!   range a `StyledSpan` based on the per-frame `InputClassification`.
//! - `wrap_lines_for_width` performs cell-aware soft-wrap so the
//!   composer can size itself to fit multi-line input.
//! - `style_at` / `line_for_byte` are tiny lookup helpers used by the
//!   render pass.

use neoism_paint::text::DrawOpts;
use unicode_width::UnicodeWidthChar;

use super::types::{InputClassification, InputTextStyle, StyledSpan, WrappedLine};

pub(super) fn draw_opts_for_style(
    font_size: f32,
    style: InputTextStyle,
    clip_rect: Option<[f32; 4]>,
) -> DrawOpts {
    DrawOpts {
        font_size,
        color: style.color,
        bold: style.bold,
        clip_rect,
        ..DrawOpts::default()
    }
}

pub(super) fn styled_spans(
    text: &str,
    classification: InputClassification,
) -> Vec<StyledSpan> {
    if text.is_empty() {
        return Vec::new();
    }
    let cmd_end = text.find(char::is_whitespace).unwrap_or(text.len());
    let mut spans = Vec::new();
    if cmd_end > 0 {
        spans.push(StyledSpan {
            start: 0,
            end: cmd_end,
            style: classification.command,
        });
    }

    let mut iter = text[cmd_end..].char_indices().peekable();
    while let Some((rel_start, ch)) = iter.next() {
        let start = cmd_end + rel_start;
        if ch.is_whitespace() {
            let mut end = start + ch.len_utf8();
            while let Some(&(rel, next)) = iter.peek() {
                if !next.is_whitespace() {
                    break;
                }
                iter.next();
                end = cmd_end + rel + next.len_utf8();
            }
            spans.push(StyledSpan {
                start,
                end,
                style: classification.arg,
            });
            continue;
        }

        if ch == '\'' || ch == '"' {
            let quote = ch;
            let mut end = start + ch.len_utf8();
            while let Some((rel, next)) = iter.next() {
                end = cmd_end + rel + next.len_utf8();
                if next == quote {
                    break;
                }
            }
            spans.push(StyledSpan {
                start,
                end,
                style: classification.string,
            });
            continue;
        }

        if is_redirection_char(ch) {
            spans.push(StyledSpan {
                start,
                end: start + ch.len_utf8(),
                style: classification.redirection,
            });
            continue;
        }

        let mut end = start + ch.len_utf8();
        while let Some(&(rel, next)) = iter.peek() {
            if next.is_whitespace()
                || next == '\''
                || next == '"'
                || is_redirection_char(next)
            {
                break;
            }
            iter.next();
            end = cmd_end + rel + next.len_utf8();
        }
        let token = &text[start..end];
        let style = if token.contains('*') || token.contains('?') || token.contains('[') {
            classification.glob
        } else if token.starts_with('-') {
            classification.arg
        } else if token.contains('/')
            || token.starts_with("~/")
            || token.starts_with("./")
            || token.starts_with("../")
        {
            classification.path
        } else {
            classification.arg
        };
        spans.push(StyledSpan { start, end, style });
    }

    spans
}

pub(super) fn is_redirection_char(ch: char) -> bool {
    matches!(ch, '|' | '&' | ';' | '<' | '>')
}

pub(super) fn style_at(spans: &[StyledSpan], byte: usize) -> Option<InputTextStyle> {
    spans
        .iter()
        .find(|span| byte >= span.start && byte < span.end)
        .or_else(|| spans.iter().rev().find(|span| byte == span.end))
        .map(|span| span.style)
}

pub(super) fn line_for_byte(lines: &[WrappedLine], byte: usize) -> usize {
    // Inclusive end, first match: a byte on a soft-wrap boundary
    // belongs to the EARLIER row — the same convention
    // `TerminalInputBuffer::current_visual_range_index` uses for
    // Up/Down movement. If the two disagree, the caret paints on one
    // row while movement resolves another and arrow keys visibly
    // bounce between rows.
    for (idx, line) in lines.iter().enumerate() {
        if byte >= line.start && byte <= line.end {
            return idx;
        }
    }
    lines.len().saturating_sub(1)
}

pub(super) fn wrap_lines(
    text: &str,
    first_width: f32,
    wrapped_width: f32,
    cell_width: f32,
    max_lines: usize,
) -> Vec<WrappedLine> {
    wrap_lines_for_width(text, first_width, wrapped_width, cell_width, max_lines)
}

pub(super) fn wrap_lines_for_width(
    text: &str,
    first_width: f32,
    wrapped_width: f32,
    cell_width: f32,
    max_lines: usize,
) -> Vec<WrappedLine> {
    if text.is_empty() {
        return vec![WrappedLine {
            start: 0,
            end: 0,
            width_limit: first_width,
        }];
    }

    let max_lines = max_lines.max(1);
    let mut lines = Vec::new();
    let mut line_start = 0usize;
    let mut line_width = 0.0;
    let cell_width = cell_width.max(1.0);
    let mut width_limit = first_width.max(1.0);

    for (idx, ch) in text.char_indices() {
        let next = idx + ch.len_utf8();
        if ch == '\n' {
            lines.push(WrappedLine {
                start: line_start,
                end: idx,
                width_limit,
            });
            if lines.len() >= max_lines {
                return lines;
            }
            line_start = next;
            line_width = 0.0;
            width_limit = wrapped_width.max(1.0);
            continue;
        }

        let glyph_w = composer_char_width(ch, cell_width);
        if idx > line_start && line_width + glyph_w > width_limit {
            lines.push(WrappedLine {
                start: line_start,
                end: idx,
                width_limit,
            });
            if lines.len() >= max_lines {
                if let Some(last) = lines.last_mut() {
                    last.end = text.len();
                }
                return lines;
            }
            line_start = idx;
            line_width = 0.0;
            width_limit = wrapped_width.max(1.0);
        }
        line_width += glyph_w;
    }

    lines.push(WrappedLine {
        start: line_start,
        end: text.len(),
        width_limit,
    });
    lines
}

fn composer_char_width(ch: char, cell_width: f32) -> f32 {
    if ch == '\t' {
        return cell_width * 4.0;
    }
    UnicodeWidthChar::width(ch).unwrap_or(1).max(1) as f32 * cell_width
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_lines_breaks_long_single_line_input_by_cells() {
        let lines = wrap_lines_for_width("abcdefghijkl", 5.0, 5.0, 1.0, 16);

        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0].start, lines[0].end), (0, 5));
        assert_eq!((lines[1].start, lines[1].end), (5, 10));
        assert_eq!((lines[2].start, lines[2].end), (10, 12));
    }

    #[test]
    fn wrap_lines_keeps_trailing_pasted_newline_visible() {
        let lines = wrap_lines_for_width("one\ntwo\n", 20.0, 20.0, 1.0, 16);

        assert_eq!(lines.len(), 3);
        assert_eq!((lines[0].start, lines[0].end), (0, 3));
        assert_eq!((lines[1].start, lines[1].end), (4, 7));
        assert_eq!((lines[2].start, lines[2].end), (8, 8));
    }
}
