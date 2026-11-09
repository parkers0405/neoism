//! Text for tool rows: Geist Mono at an exact point size with a one-em line box, so a label
//! placed at `y` has its em box top at `y` like the original painter's `draw(x, y, …)`. The mono
//! advance (0.6 em) makes widths, truncation and wrapping exact without measuring.

use crate::app::App;
use crate::ui::theme::NERD;
use rune::tok::TextStyle;
use rune::{Color, El, text};

pub const ADVANCE: f32 = 0.6;

pub struct Text {
    content: String,
    size: f32,
    weight: f32,
    italic: bool,
    color: Color,
}

impl Text {
    pub fn new(content: impl Into<String>, size: f32, app: &App) -> Text {
        Text { content: content.into(), size, weight: app.ty.weight, italic: false, color: Color::WHITE }
    }

    pub fn color(mut self, color: Color) -> Text { self.color = color; self }
    pub fn bold(mut self) -> Text { self.weight = 800.0; self }
    pub fn italic(mut self) -> Text { self.italic = true; self }

    pub fn width(&self) -> f32 { width(&self.content, self.size) }

    /// Cut to `max` points with an ellipsis (`truncate_to_fit`).
    pub fn fit(mut self, max: f32) -> Text {
        self.content = fit(&self.content, max, self.size);
        self
    }

    pub fn el(self) -> El {
        let nerd = self.content.chars().any(is_nerd_glyph);
        let mut style = TextStyle::new(self.size, self.weight).lh(self.size);
        style = if nerd { style.family(NERD) } else { style.mono() };
        if self.italic { style = style.italic(); }
        text(self.content).font(style).color(self.color)
    }

    /// Placed with its em box top-left at `(x, y)` in the row.
    pub fn at(self, x: f32, y: f32) -> El { self.el().absolute(x, y) }
}

pub fn width(content: &str, size: f32) -> f32 {
    content.chars().count() as f32 * size * ADVANCE
}

pub fn fit(content: &str, max: f32, size: f32) -> String {
    let chars = (max / (size * ADVANCE)).floor().max(0.0) as usize;
    let count = content.chars().count();
    if count <= chars {
        return content.to_owned();
    }
    if chars == 0 {
        return String::new();
    }
    let mut out: String = content.chars().take(chars - 1).collect();
    out.push('\u{2026}');
    out
}

/// Private-use code points come from the Nerd icon face.
pub fn is_nerd_glyph(c: char) -> bool {
    matches!(c as u32, 0xE000..=0xF8FF | 0xF0000..=0xFFFFD)
}

/// Greedy word wrap to `max` points (long words break mid-word), at most `limit` rows.
pub fn wrap(content: &str, max: f32, size: f32, limit: usize) -> Vec<String> {
    let cols = ((max / (size * ADVANCE)).floor() as usize).max(1);
    let mut rows = Vec::new();
    for line in content.split('\n') {
        let line = line.trim_end_matches('\r').replace('\t', "    ");
        let mut current = String::new();
        let mut used = 0;
        for word in line.split_inclusive(' ') {
            let len = word.chars().count();
            if used + len > cols && used > 0 {
                rows.push(std::mem::take(&mut current).trim_end().to_owned());
                used = 0;
                if rows.len() >= limit { return rows; }
            }
            let mut rest: Vec<char> = word.chars().collect();
            while rest.len() > cols {
                let head: String = rest.drain(..cols).collect();
                rows.push(head);
                if rows.len() >= limit { return rows; }
            }
            used += rest.len();
            current.extend(rest);
        }
        rows.push(current.trim_end().to_owned());
        if rows.len() >= limit { return rows; }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_ellipsizes_to_whole_columns() {
        assert_eq!(fit("abcdef", 10.0 * 0.6 * 4.0, 10.0), "abc\u{2026}");
        assert_eq!(fit("abc", 100.0, 10.0), "abc");
    }

    #[test]
    fn wrap_breaks_words_then_long_runs() {
        assert_eq!(wrap("one two three", 6.0 * 7.0, 10.0, 9), ["one", "two", "three"]);
        assert_eq!(wrap("abcdefghij", 6.0 * 4.0, 10.0, 9), ["abcd", "efgh", "ij"]);
        assert_eq!(wrap("a\nb\nc", 60.0, 10.0, 2), ["a", "b"]);
    }
}
