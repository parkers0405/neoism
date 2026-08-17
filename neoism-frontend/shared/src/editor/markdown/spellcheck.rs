//! Pure spellcheck primitives for the markdown editor.
//!
//! These were previously co-located with the render code under
//! `render/inline.rs` + `render/types.rs`, but they are pure
//! data-only helpers: a one-shot dictionary load (from `/usr/share/dict/words`
//! or equivalents) plus a normalize/lookup pair. The state module's
//! `find_misspelling_at_point` consumes `is_misspelled_word`, and the
//! markdown bridge in the native crate consumes `spelling_suggestions`,
//! so both surfaces need to see the same dictionary.
//!
//! Living in `neoism-ui` keeps the lifted state crate self-contained:
//! no reach-arounds into the still-native render module. The native
//! render's `draw_spellcheck_underlines` re-exports these via its own
//! `pub use` so the public surface
//! (`crate::editor::markdown::render::{is_misspelled_word, spelling_suggestions}`)
//! continues to resolve.

use std::collections::HashSet;
use std::io;
use std::sync::{OnceLock, RwLock};

const SPELLCHECK_DICT_PATHS: &[&str] = &[
    "/usr/share/dict/words",
    "/usr/share/dict/web2",
    "/usr/dict/words",
    "/usr/share/dict/cracklib-small",
];

static SPELLCHECK_DICTIONARY: OnceLock<Option<HashSet<String>>> = OnceLock::new();
static SPELLCHECK_OVERRIDES: OnceLock<RwLock<SpellcheckOverrides>> = OnceLock::new();

#[derive(Default)]
struct SpellcheckOverrides {
    dictionary: HashSet<String>,
    ignored: HashSet<String>,
}

#[derive(Clone, Copy)]
pub struct SpellcheckWord<'a> {
    pub start: usize,
    pub text: &'a str,
}

pub fn spellcheck_words(text: &str) -> Vec<SpellcheckWord<'_>> {
    let mut words = Vec::new();
    let mut start = None;
    let mut last_end = 0usize;

    for (ix, ch) in text.char_indices() {
        let is_word_char = ch.is_alphabetic() || ch == '\'';
        if is_word_char {
            start.get_or_insert(ix);
        } else if let Some(word_start) = start.take() {
            if let Some(word) = text.get(word_start..ix) {
                words.push(SpellcheckWord {
                    start: word_start,
                    text: word,
                });
            }
        }
        last_end = ix + ch.len_utf8();
    }

    if let Some(word_start) = start {
        if let Some(word) = text.get(word_start..last_end) {
            words.push(SpellcheckWord {
                start: word_start,
                text: word,
            });
        }
    }

    words
}

pub fn is_misspelled_word(word: &str) -> bool {
    let Some(normalized) = normalized_spellcheck_word(word) else {
        return false;
    };
    let Some(dictionary) = spellcheck_dictionary() else {
        return false;
    };
    !dictionary.contains(&normalized) && !spellcheck_override_contains(&normalized)
}

pub fn normalized_spellcheck_word(word: &str) -> Option<String> {
    let trimmed = word.trim_matches('\'');
    if trimmed.chars().count() < 4 {
        return None;
    }
    if trimmed.chars().any(|ch| ch.is_ascii_digit() || ch == '_') {
        return None;
    }
    let has_lower = trimmed.chars().any(|ch| ch.is_lowercase());
    let has_upper_after_first = trimmed.chars().skip(1).any(|ch| ch.is_uppercase());
    if has_lower && has_upper_after_first {
        return None;
    }
    if trimmed.chars().all(|ch| ch.is_uppercase()) {
        return None;
    }
    let mut normalized = trimmed.to_lowercase();
    if normalized.ends_with("'s") {
        normalized.truncate(normalized.len().saturating_sub(2));
    }
    if normalized.chars().count() < 4 {
        return None;
    }
    Some(normalized)
}

pub fn spellcheck_dictionary() -> Option<&'static HashSet<String>> {
    SPELLCHECK_DICTIONARY
        .get_or_init(load_spellcheck_dictionary)
        .as_ref()
}

pub fn spelling_suggestions(word: &str) -> Vec<String> {
    let Some(normalized) = normalized_spellcheck_word(word) else {
        return Vec::new();
    };
    let Some(dictionary) = spellcheck_dictionary() else {
        return Vec::new();
    };
    if dictionary.contains(&normalized) || spellcheck_override_contains(&normalized) {
        return Vec::new();
    }
    let first = normalized.chars().next();
    let max_distance = if normalized.chars().count() >= 8 {
        3
    } else {
        2
    };
    let mut scored = dictionary
        .iter()
        .filter_map(|candidate| {
            if first.is_some() && candidate.chars().next() != first {
                return None;
            }
            let len_diff = normalized
                .chars()
                .count()
                .abs_diff(candidate.chars().count());
            if len_diff > max_distance {
                return None;
            }
            let distance = bounded_levenshtein(&normalized, candidate, max_distance)?;
            Some((distance, len_diff, candidate.len(), candidate.as_str()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
            .then(a.3.cmp(&b.3))
    });
    scored
        .into_iter()
        .take(5)
        .map(|(_, _, _, suggestion)| match_spelling_case(word, suggestion))
        .collect()
}

pub fn ignore_spelling_word(word: &str) -> bool {
    let Some(normalized) = normalized_spellcheck_word(word) else {
        return false;
    };
    spellcheck_overrides()
        .write()
        .is_ok_and(|mut overrides| overrides.ignored.insert(normalized))
}

pub fn add_spelling_word_to_dictionary(word: &str) -> io::Result<bool> {
    let Some(normalized) = normalized_spellcheck_word(word) else {
        return Ok(false);
    };
    let Some(path) = global_spellcheck_dictionary_path() else {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "global Neoism config directory is unavailable",
        ));
    };
    add_spelling_word_to_dictionary_at(&path, &normalized)
}

fn add_spelling_word_to_dictionary_at(
    path: &std::path::Path,
    normalized: &str,
) -> io::Result<bool> {
    use std::io::Write;

    let mut overrides = spellcheck_overrides()
        .write()
        .map_err(|_| io::Error::other("spellcheck dictionary lock poisoned"))?;
    if overrides.dictionary.contains(normalized) {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{normalized}")?;
    overrides.dictionary.insert(normalized.to_string());
    overrides.ignored.remove(normalized);
    Ok(true)
}

#[cfg(not(target_arch = "wasm32"))]
pub fn global_spellcheck_dictionary_path() -> Option<std::path::PathBuf> {
    dirs::config_dir().map(|base| base.join("neoism").join("dictionary.txt"))
}

#[cfg(target_arch = "wasm32")]
pub fn global_spellcheck_dictionary_path() -> Option<std::path::PathBuf> {
    None
}

fn spellcheck_override_contains(word: &str) -> bool {
    spellcheck_overrides().read().is_ok_and(|overrides| {
        overrides.dictionary.contains(word) || overrides.ignored.contains(word)
    })
}

fn spellcheck_overrides() -> &'static RwLock<SpellcheckOverrides> {
    SPELLCHECK_OVERRIDES.get_or_init(|| RwLock::new(load_spellcheck_overrides()))
}

fn load_spellcheck_overrides() -> SpellcheckOverrides {
    let dictionary = global_spellcheck_dictionary_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .map(|source| {
            source
                .lines()
                .filter_map(|line| normalized_spellcheck_word(line.trim()))
                .collect()
        })
        .unwrap_or_default();
    SpellcheckOverrides {
        dictionary,
        ignored: HashSet::new(),
    }
}

fn match_spelling_case(original: &str, suggestion: &str) -> String {
    let trimmed = original.trim_matches('\'');
    let was_title = trimmed.chars().next().is_some_and(|ch| ch.is_uppercase());
    if was_title {
        let mut chars = suggestion.chars();
        if let Some(first) = chars.next() {
            return first.to_uppercase().chain(chars).collect::<String>();
        }
    }
    suggestion.to_string()
}

pub fn bounded_levenshtein(a: &str, b: &str, max_distance: usize) -> Option<usize> {
    let a_chars = a.chars().collect::<Vec<_>>();
    let b_chars = b.chars().collect::<Vec<_>>();
    if a_chars.len().abs_diff(b_chars.len()) > max_distance {
        return None;
    }
    let mut prev = (0..=b_chars.len()).collect::<Vec<_>>();
    let mut curr = vec![0; b_chars.len() + 1];
    for (i, a_ch) in a_chars.iter().enumerate() {
        curr[0] = i + 1;
        let mut row_min = curr[0];
        for (j, b_ch) in b_chars.iter().enumerate() {
            let cost = usize::from(a_ch != b_ch);
            curr[j + 1] = (prev[j + 1] + 1).min(curr[j] + 1).min(prev[j] + cost);
            row_min = row_min.min(curr[j + 1]);
        }
        if row_min > max_distance {
            return None;
        }
        std::mem::swap(&mut prev, &mut curr);
    }
    (prev[b_chars.len()] <= max_distance).then_some(prev[b_chars.len()])
}

fn load_spellcheck_dictionary() -> Option<HashSet<String>> {
    for path in SPELLCHECK_DICT_PATHS {
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        let mut words = HashSet::new();
        for line in source.lines() {
            if let Some(word) = normalized_spellcheck_word(line.trim()) {
                words.insert(word);
            }
        }
        if !words.is_empty() {
            return Some(words);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn custom_dictionary_write_is_deduplicated_and_normalized() {
        let dir = tempfile::tempdir().expect("temporary dictionary directory");
        let path = dir.path().join("dictionary.txt");
        let word = format!("neoismcustomword{}", std::process::id());

        assert!(add_spelling_word_to_dictionary_at(&path, &word).unwrap());
        assert!(!add_spelling_word_to_dictionary_at(&path, &word).unwrap());
        assert_eq!(std::fs::read_to_string(path).unwrap(), format!("{word}\n"));
        assert!(spellcheck_override_contains(&word));
    }
}
