//! World-space text layout. Camera zoom never participates in line breaking.
use super::scene::Vec2;
use unicode_segmentation::UnicodeSegmentation;

/// One bounded atlas size, scaled by the canvas renderer rather than re-rasterized
/// at every zoom level. Large text cannot exhaust the atlas with giant glyphs.
pub(super) const TEXT_RASTER_SIZE: f32 = 128.0;

#[derive(Clone, Debug)]
pub(super) struct TextRow {
    pub text: String,
    pub advance: f32,
}

#[derive(Clone, Debug)]
pub(super) struct TextLayout {
    pub rows: Vec<TextRow>,
    pub bounds: Vec2,
    pub font_size: f32,
    pub line_height: f32,
}

pub(super) fn font_size(size: f32) -> f32 {
    if size.is_finite() {
        size.clamp(6.0, 4000.0)
    } else {
        28.0
    }
}

pub(super) fn layout_text(
    content: &str,
    size: f32,
    width: Option<f32>,
    mut measure: impl FnMut(&str) -> f32,
) -> TextLayout {
    let size = font_size(size);
    let width = width
        .filter(|width| width.is_finite())
        .map(|width| width.max(16.0));
    let mut rows = Vec::new();
    for paragraph in content.split('\n') {
        if let Some(width) = width {
            let clusters: Vec<_> = paragraph.grapheme_indices(true).collect();
            let mut start = 0;
            if clusters.is_empty() {
                rows.push(TextRow {
                    text: String::new(),
                    advance: 0.0,
                });
            }
            while start < clusters.len() {
                let byte = |index: usize| {
                    clusters
                        .get(index)
                        .map_or(paragraph.len(), |(byte, _)| *byte)
                };
                let mut end = start;
                let mut word_break = None;
                while end < clusters.len() {
                    if end > start
                        && measure(&paragraph[byte(start)..byte(end + 1)]) > width
                    {
                        break;
                    }
                    end += 1;
                    if clusters[end - 1].1.chars().all(char::is_whitespace) {
                        word_break = Some(end);
                    }
                }
                if end < clusters.len() {
                    if let Some(boundary) = word_break {
                        end = boundary;
                    }
                }
                let text = paragraph[byte(start)..byte(end)].to_string();
                rows.push(TextRow {
                    advance: measure(&text),
                    text,
                });
                start = end;
            }
        } else {
            rows.push(TextRow {
                text: paragraph.to_string(),
                advance: measure(paragraph),
            });
        }
    }
    let natural_width = rows
        .iter()
        .map(|row| row.advance)
        .fold(size * 0.3, f32::max);
    let line_height = size * 1.25;
    TextLayout {
        bounds: Vec2::new(
            width.unwrap_or(natural_width).max(natural_width),
            rows.len() as f32 * line_height,
        ),
        rows,
        font_size: size,
        line_height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn layout_wraps_at_box_width_and_preserves_explicit_blank_lines() {
        let layout = layout_text("hello world\n\nlast\n", 10.0, Some(40.0), |text| {
            text.graphemes(true).count() as f32 * 5.0
        });
        assert_eq!(
            layout
                .rows
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            vec!["hello ", "world", "", "last", ""]
        );
        assert_eq!(layout.bounds, Vec2::new(40.0, 62.5));
    }
    #[test]
    fn legacy_labels_remain_auto_sized_and_long_words_keep_graphemes() {
        let layout =
            layout_text("long label", 10.0, None, |text| text.len() as f32 * 5.0);
        assert_eq!(layout.bounds.x, 50.0);
        let layout = layout_text(
            "a\u{301}b\u{1f469}\u{200d}\u{1f4bb}",
            10.0,
            Some(16.0),
            |text| text.graphemes(true).count() as f32 * 16.0,
        );
        assert_eq!(layout.rows.len(), 3);
        assert_eq!(layout.rows[0].text, "a\u{301}");
        assert_eq!(layout.rows[2].text, "\u{1f469}\u{200d}\u{1f4bb}");
    }
}
