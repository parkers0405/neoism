//! Shared, source-preserving geometry for Markdown table cells.
use crate::editor::markdown::MarkdownWrapRow;
use unicode_segmentation::UnicodeSegmentation;

/// Word wrapping with exact character offsets. Unlike reconstructing offsets
/// from rendered strings, this handles repeated whitespace and hard-wrapped URLs.
pub(super) fn wrap_cell(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<MarkdownWrapRow> {
    let chars: Vec<(usize, char)> = text.char_indices().collect();
    if chars.is_empty() {
        return vec![MarkdownWrapRow { start: 0, len: 0 }];
    }
    let mut grapheme_boundary = vec![false; chars.len() + 1];
    grapheme_boundary[chars.len()] = true;
    for (byte, _) in text.grapheme_indices(true) {
        if let Ok(index) = chars.binary_search_by_key(&byte, |(byte, _)| *byte) {
            grapheme_boundary[index] = true;
        }
    }
    let byte_at = |index: usize| chars.get(index).map_or(text.len(), |(byte, _)| *byte);
    let mut rows = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let mut end = start;
        let mut word_break = None;
        while end < chars.len() {
            if chars[end].1 == '\n' {
                break;
            }
            let mut candidate_end = end + 1;
            while !grapheme_boundary[candidate_end] {
                candidate_end += 1;
            }
            if end > start
                && measure(&text[byte_at(start)..byte_at(candidate_end)]) > width.max(1.0)
            {
                break;
            }
            end = candidate_end;
            if chars[end - 1].1.is_whitespace() {
                word_break = Some(end);
            }
        }
        let mut next = end;
        if end < chars.len() && chars[end].1 == '\n' {
            next += 1;
        } else if end < chars.len() {
            if let Some(boundary) = word_break.filter(|boundary| *boundary > start) {
                end = boundary;
                next = boundary;
            }
            while next < chars.len()
                && chars[next].1.is_whitespace()
                && chars[next].1 != '\n'
            {
                let mut after = next + 1;
                while !grapheme_boundary[after] {
                    after += 1;
                }
                if !chars[next..after].iter().all(|(_, ch)| ch.is_whitespace()) {
                    break;
                }
                next = after;
            }
        }
        while end > start && chars[end - 1].1.is_whitespace() {
            end -= 1;
        }
        rows.push(MarkdownWrapRow {
            start,
            len: end.saturating_sub(start),
        });
        start = next.max(start + 1);
    }
    if text.ends_with('\n') {
        rows.push(MarkdownWrapRow {
            start: chars.len(),
            len: 0,
        });
    }
    rows
}

pub(super) fn reveal_column(
    scroll: f32,
    viewport: f32,
    widths: &[f32],
    column: usize,
    caret: f32,
) -> f32 {
    let max = (widths.iter().sum::<f32>() - viewport).max(0.0);
    let Some(&width) = widths.get(column) else {
        return scroll.clamp(0.0, max);
    };
    let left = widths.iter().take(column).sum::<f32>();
    let right = left + width;
    let margin = 12.0_f32.min(viewport * 0.05);
    let mut next = scroll;
    if width <= viewport - margin * 2.0 {
        if left < next + margin {
            next = left - margin;
        }
        if right > next + viewport - margin {
            next = right - viewport + margin;
        }
    } else {
        if caret < next + margin {
            next = caret - margin;
        }
        if caret + margin > next + viewport {
            next = caret + margin - viewport;
        }
    }
    next.clamp(0.0, max)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn wrapping_preserves_offsets_for_words_urls_unicode_and_spaces() {
        let rows = wrap_cell("abc defghijkl", 5.0, |text| text.chars().count() as f32);
        assert_eq!(
            rows.iter().map(|r| (r.start, r.len)).collect::<Vec<_>>(),
            vec![(0, 3), (4, 5), (9, 3)]
        );
        let rows = wrap_cell("ab   cd", 4.0, |text| text.chars().count() as f32);
        assert_eq!(
            rows.iter().map(|r| (r.start, r.len)).collect::<Vec<_>>(),
            vec![(0, 2), (5, 2)]
        );
        let rows = wrap_cell("\u{03b1}\u{03b2}\u{03b3}\u{03b4}\u{03b5}", 3.0, |text| {
            text.chars().count() as f32
        });
        assert_eq!(
            rows.iter().map(|r| (r.start, r.len)).collect::<Vec<_>>(),
            vec![(0, 3), (3, 2)]
        );
        assert_eq!(wrap_cell("", 20.0, |_| 0.0).len(), 1);
    }
    #[test]
    fn wrapping_keeps_combining_marks_and_emoji_clusters_together() {
        let rows =
            wrap_cell("a\u{301}b", 1.0, |text| text.graphemes(true).count() as f32);
        assert_eq!(
            rows.iter()
                .map(|row| (row.start, row.len))
                .collect::<Vec<_>>(),
            vec![(0, 2), (2, 1)]
        );
        let rows = wrap_cell("\u{1f469}\u{200d}\u{1f4bb}x", 1.0, |text| {
            text.graphemes(true).count() as f32
        });
        assert_eq!(
            rows.iter()
                .map(|row| (row.start, row.len))
                .collect::<Vec<_>>(),
            vec![(0, 3), (3, 1)]
        );
    }

    #[test]
    fn table_parser_retains_declared_alignment_and_dash_data_rows() {
        let source = vec![
            "| A | B | C |".into(),
            "| :--- | :---: | ---: |".into(),
            "| --- | --- | --- |".into(),
        ];
        let table = super::super::table::parse_table(&source, 0).unwrap();
        assert_eq!(table.alignments, vec![0.0, 0.5, 1.0]);
        assert_eq!(table.rows.len(), 1);
        assert_eq!(table.end_line, 3);
    }

    #[test]
    fn keyboard_follow_reveals_the_whole_column_when_it_fits() {
        let widths = [180.0; 6];
        let scroll = reveal_column(0.0, 420.0, &widths, 3, 550.0);
        assert!(540.0 >= scroll && 720.0 <= scroll + 420.0);
        let back = reveal_column(scroll, 420.0, &widths, 0, 16.0);
        assert_eq!(back, 0.0);
        assert_eq!(reveal_column(400.0, 900.0, &[120.0, 120.0], 1, 140.0), 0.0);
    }
}
