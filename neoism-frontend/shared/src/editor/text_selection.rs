//! Shared canvas-text word targeting.

use unicode_segmentation::UnicodeSegmentation;

/// Unicode word span at a byte position, falling back to one complete
/// grapheme for emoji and punctuation. Whitespace is intentionally not
/// selectable by hard hold. The returned range is always on UTF-8 and
/// extended-grapheme boundaries.
pub fn unicode_word_or_grapheme_span(
    text: &str,
    byte_offset: usize,
) -> Option<(usize, usize)> {
    if text.is_empty() {
        return None;
    }
    let mut probe = byte_offset.min(text.len());
    while probe > 0 && !text.is_char_boundary(probe) {
        probe -= 1;
    }
    if probe == text.len() {
        probe = text.grapheme_indices(true).next_back()?.0;
    }

    for (start, word) in text.unicode_word_indices() {
        let end = start + word.len();
        if probe >= start && probe < end {
            return Some((start, end));
        }
    }

    text.grapheme_indices(true).find_map(|(start, grapheme)| {
        let end = start + grapheme.len();
        (probe >= start && probe < end && !grapheme.chars().all(char::is_whitespace))
            .then_some((start, end))
    })
}

/// Keep a long-held word selected while the moving edge crosses it. Leftward
/// motion fixes the original end; rightward motion fixes the original start.
pub fn anchored_word_selection<T: Copy + Ord>(start: T, end: T, target: T) -> (T, T) {
    if target < start {
        (end, target)
    } else if target > end {
        (start, target)
    } else {
        (start, end)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unicode_words_combining_marks_and_emoji_stay_intact() {
        let text = "naïve e\u{301}lan 👩🏽‍💻!";
        let span = unicode_word_or_grapheme_span(text, text.find('ï').unwrap()).unwrap();
        assert_eq!(&text[span.0..span.1], "naïve");
        let span =
            unicode_word_or_grapheme_span(text, text.find("e\u{301}").unwrap()).unwrap();
        assert_eq!(&text[span.0..span.1], "e\u{301}lan");
        let span = unicode_word_or_grapheme_span(text, text.find('👩').unwrap()).unwrap();
        assert_eq!(&text[span.0..span.1], "👩🏽‍💻");
    }

    #[test]
    fn punctuation_is_one_grapheme_and_whitespace_is_not_selected() {
        let text = "hello… world";
        let span = unicode_word_or_grapheme_span(text, text.find('…').unwrap()).unwrap();
        assert_eq!(&text[span.0..span.1], "…");
        assert_eq!(
            unicode_word_or_grapheme_span(text, text.find(' ').unwrap()),
            None
        );
    }

    #[test]
    fn held_word_drag_anchors_the_opposite_edge_in_both_directions() {
        assert_eq!(anchored_word_selection(4, 9, 2), (9, 2));
        assert_eq!(anchored_word_selection(4, 9, 12), (4, 12));
        assert_eq!(anchored_word_selection(4, 9, 7), (4, 9));
    }

    #[test]
    fn unicode_word_edges_remain_grapheme_stops_while_dragging_across_them() {
        let text = "x e\u{301}lan 👩🏽‍💻 y";
        let (start, end) =
            unicode_word_or_grapheme_span(text, text.find('\u{301}').unwrap())
                .expect("combined word");
        let emoji = text.find('👩').unwrap();
        let (_, right) = anchored_word_selection(start, end, emoji);
        assert!(text.is_char_boundary(right));
        assert_eq!(&text[start..end], "e\u{301}lan");
        let (left_anchor, left_focus) = anchored_word_selection(start, end, 0);
        assert_eq!((left_anchor, left_focus), (end, 0));
    }
}
