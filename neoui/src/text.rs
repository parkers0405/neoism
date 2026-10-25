use std::collections::HashMap;

use crate::{Color, LayoutError};

/// Text bounds use a top origin; baseline placement is the renderer adapter's
/// responsibility. `line_height` is the top-to-top distance and the line box
/// height, in logical pixels (not a multiplier).
#[derive(Clone, Debug, PartialEq)]
pub struct TextStyle {
    pub size: f32,
    pub line_height: f32,
    pub color: Color,
}

impl Default for TextStyle {
    fn default() -> Self {
        Self {
            size: 14.0,
            line_height: 20.0,
            color: [1.0; 4],
        }
    }
}

/// Supply the exact advance width the host uses to render a single line.
/// Measurements must be deterministic during a layout and finite/nonnegative.
/// The core measures complete candidate strings, not summed character advances,
/// so kerning/shaping widths match the emitted paint strings. Font selection is
/// owned by the adapter. A stable `cache_key` must identify the complete font,
/// shaping and DPI context across all measurers used with a runtime. Measurement
/// depends on text, size and line height, never color. Change the key before any
/// metric changes. The default `None` permits reuse only within one presentation.
/// Identity of the host's font/shaping/DPI context. Change it whenever advances change.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MeasurementKey(pub [u64; 4]);

pub trait TextMeasurer {
    /// `None` disables cross-presentation measurement reuse.
    fn cache_key(&mut self) -> Option<MeasurementKey> { None }
    fn measure(&mut self, text: &str, style: &TextStyle) -> f32;
}

impl<F: FnMut(&str, &TextStyle) -> f32> TextMeasurer for F {
    fn measure(&mut self, text: &str, style: &TextStyle) -> f32 {
        self(text, style)
    }
}

#[derive(Clone, Debug)]
pub(crate) struct Line {
    pub text: String,
    pub width: f32,
}

#[derive(Clone, Debug)]
pub(crate) struct TextLayout {
    pub lines: Vec<Line>,
    pub width: f32,
    pub height: f32,
}

pub(crate) struct TextContext {
    pub text: String,
    pub style: TextStyle,
    // Taffy asks for several intrinsic/constrained sizes. Keep the exact strings
    // used for each width, and reuse them during frame generation.
    cache: HashMap<u32, TextLayout>,
}

impl TextContext {
    pub fn new(text: String, style: TextStyle) -> Self {
        Self {
            text,
            style,
            cache: HashMap::new(),
        }
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    pub fn matches(&self, text: &str, style: &TextStyle) -> bool {
        self.text == text && self.style.size == style.size
            && self.style.line_height == style.line_height
    }

    pub fn layout(
        &mut self,
        width: Option<f32>,
        measurer: &mut dyn TextMeasurer,
    ) -> Result<TextLayout, LayoutError> {
        let epoch = measurer.cache_key();
        let key = width.map_or(f32::INFINITY.to_bits(), |v| v.max(0.0).to_bits());
        if let Some(layout) = self.cache.get(&key) {
            return Ok(layout.clone());
        }
        let layout = wrap(&self.text, &self.style, width, measurer)?;
        if measurer.cache_key() != epoch {
            return Err(LayoutError::MeasurementContextChanged);
        }
        // Bound per-node width history (e.g. during continuous window resizing).
        if self.cache.len() >= 32 { self.cache.clear(); }
        self.cache.insert(key, layout.clone());
        Ok(layout)
    }
}

fn measure(
    text: &str,
    style: &TextStyle,
    measurer: &mut dyn TextMeasurer,
) -> Result<f32, LayoutError> {
    let epoch = measurer.cache_key();
    let width = measurer.measure(text, style);
    if measurer.cache_key() != epoch {
        return Err(LayoutError::MeasurementContextChanged);
    }
    if width.is_finite() && width >= 0.0 {
        Ok(width)
    } else {
        Err(LayoutError::InvalidMeasurement)
    }
}

/// Word wrapping normalizes intra-paragraph whitespace to one space, with no
/// leading/trailing space. Explicit newlines (including empty/trailing lines)
/// are preserved; CRLF is accepted. Long tokens break only at UTF-8 scalar
/// boundaries. Grapheme segmentation, bidi and hyphenation belong to a future
/// shaped-text adapter; the core never slices through a UTF-8 code point.
fn wrap(
    text: &str,
    style: &TextStyle,
    width: Option<f32>,
    measurer: &mut dyn TextMeasurer,
) -> Result<TextLayout, LayoutError> {
    let limit = width.unwrap_or(f32::INFINITY).max(0.0);
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let paragraph = paragraph.strip_suffix('\r').unwrap_or(paragraph);
        let mut current = String::new();
        let mut current_width = 0.0;
        let first_line = lines.len();
        for word in paragraph.split_whitespace() {
            let candidate = if current.is_empty() {
                word.to_owned()
            } else {
                format!("{current} {word}")
            };
            let candidate_width = measure(&candidate, style, measurer)?;
            if candidate_width <= limit {
                current = candidate;
                current_width = candidate_width;
                continue;
            }
            if !current.is_empty() {
                lines.push(Line {
                    text: std::mem::take(&mut current),
                    width: current_width,
                });
            }
            let word_width = measure(word, style, measurer)?;
            if word_width <= limit {
                current = word.to_owned();
                current_width = word_width;
                continue;
            }
            // An oversized scalar gets its own overflowing line. The text box
            // clips it rather than truncating data or looping at width zero.
            current_width = 0.0;
            for ch in word.chars() {
                let mut candidate = current.clone();
                candidate.push(ch);
                let candidate_width = measure(&candidate, style, measurer)?;
                if !current.is_empty() && candidate_width > limit {
                    lines.push(Line {
                        text: std::mem::take(&mut current),
                        width: current_width,
                    });
                    current.push(ch);
                    current_width = measure(&current, style, measurer)?;
                } else {
                    current = candidate;
                    current_width = candidate_width;
                }
            }
        }
        if !current.is_empty() || lines.len() == first_line {
            lines.push(Line {
                text: current,
                width: current_width,
            });
        }
    }
    let width = lines.iter().map(|line| line.width).fold(0.0_f32, f32::max);
    let height = lines.len() as f32 * style.line_height;
    if !height.is_finite() {
        return Err(LayoutError::InvalidMeasurement);
    }
    Ok(TextLayout {
        lines,
        width,
        height,
    })
}
