//! [`El::text_case`](crate::El::text_case): draw a cased copy of a string while byte ranges
//! (spans, links) and tools keep the authored one.

/// How [`El::text_case`](crate::El::text_case) draws a text's letters.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TextCase {
    /// Capitals, by `str::to_uppercase`: "Straße" draws "STRASSE".
    Uppercase,
    /// Lower case, by `str::to_lowercase` (a word-final "Σ" draws "ς").
    Lowercase,
}

/// The authored string behind a cased one, and where each of its characters landed.
#[derive(Clone, Debug)]
pub(crate) struct Source {
    pub(crate) text: String,
    /// `(source byte, shown byte)` at each source character's start, then both lengths.
    bounds: Vec<(u32, u32)>,
}

impl Source {
    /// The cased string to draw, and the source that maps back to `text`.
    pub(crate) fn new(case: TextCase, text: String) -> (String, Source) {
        let shown = match case {
            TextCase::Uppercase => text.to_uppercase(),
            TextCase::Lowercase => text.to_lowercase(),
        };
        let mut bounds = Vec::with_capacity(text.len() + 1);
        let mut at = 0u32;
        for (i, c) in text.char_indices() {
            bounds.push((i as u32, at));
            // Per character lengths add up to the whole string's: the one contextual mapping,
            // final sigma, swaps σ for ς, both two bytes.
            at += match case {
                TextCase::Uppercase => c.to_uppercase().map(char::len_utf8).sum::<usize>(),
                TextCase::Lowercase => c.to_lowercase().map(char::len_utf8).sum::<usize>(),
            } as u32;
        }
        debug_assert_eq!(at as usize, shown.len());
        bounds.push((text.len() as u32, shown.len() as u32));
        (shown, Source { text, bounds })
    }

    /// The shown byte where source byte `b` starts. A byte inside a character rounds down to
    /// its start, or up to its end with `up` (for range ends).
    pub(crate) fn shown(&self, b: u32, up: bool) -> u32 {
        let i = if up { self.bounds.partition_point(|&(s, _)| s < b) } else { self.bounds.partition_point(|&(s, _)| s <= b).saturating_sub(1) };
        self.bounds.get(i).map_or(self.bounds.last().map_or(0, |b| b.1), |b| b.1)
    }

    /// The source byte of the character drawn at shown byte `b`, or the next one with `up`.
    pub(crate) fn source(&self, b: u32, up: bool) -> u32 {
        let i = if up { self.bounds.partition_point(|&(_, s)| s < b) } else { self.bounds.partition_point(|&(_, s)| s <= b).saturating_sub(1) };
        self.bounds.get(i).map_or(self.text.len() as u32, |b| b.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanding_capitals_map_both_ways() {
        let (shown, source) = Source::new(TextCase::Uppercase, "aß b".into());
        assert_eq!(shown, "ASS B");
        assert_eq!(source.shown(1, false), 1, "ß starts where SS starts");
        assert_eq!(source.shown(3, false), 3, "the space follows SS");
        assert_eq!(source.shown(2, false), 1, "inside ß rounds down");
        assert_eq!(source.shown(2, true), 3, "and up for a range end");
        assert_eq!(source.shown(5, false), 5);
        assert_eq!(source.source(2, false), 1, "the second S is still ß");
        assert_eq!(source.source(2, true), 3);
        assert_eq!(source.source(4, false), 4);
        assert_eq!(source.source(5, false), 5);
    }

    #[test]
    fn lower_case_keeps_final_sigma() {
        let (shown, source) = Source::new(TextCase::Lowercase, "ΟΔΟΣ Ω".into());
        assert_eq!(shown, "οδος ω");
        assert_eq!(source.shown(8, false), 8);
    }
}
