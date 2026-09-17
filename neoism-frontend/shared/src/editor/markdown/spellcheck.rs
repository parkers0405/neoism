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
    "/usr/share/dict/american-english",
    "/usr/share/dict/british-english",
    "/usr/share/dict/words",
    "/usr/share/dict/web2",
    "/usr/dict/words",
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
    let mut offset = 0;
    let mut in_code = false;
    for token in text.split_whitespace() {
        let token_start = offset + text[offset..].find(token).unwrap_or(0);
        offset = token_start + token.len();
        let ticks = token.chars().filter(|ch| *ch == '`').count();
        if in_code || ticks > 0 {
            if ticks % 2 == 1 {
                in_code = !in_code;
            }
            continue;
        }
        let candidate = token.trim_matches(|ch: char| {
            matches!(
                ch,
                '.' | ','
                    | ';'
                    | ':'
                    | '!'
                    | '?'
                    | '('
                    | ')'
                    | '['
                    | ']'
                    | '<'
                    | '>'
                    | '"'
            )
        });
        if candidate.contains(['/', '\\', '@', '_'])
            || candidate.contains('.')
            || candidate.chars().any(|ch| ch.is_ascii_digit())
        {
            continue;
        }
        let mut start = None;
        for (ix, ch) in token
            .char_indices()
            .chain(std::iter::once((token.len(), ' ')))
        {
            if ch.is_alphabetic() || matches!(ch, '\'' | '\u{2019}') {
                start.get_or_insert(ix);
            } else if let Some(begin) = start.take() {
                words.push(SpellcheckWord {
                    start: token_start + begin,
                    text: &token[begin..ix],
                });
            }
        }
    }
    words
}

pub fn is_misspelled_word(word: &str) -> bool {
    let Some(normalized) = normalized_spellcheck_word(word) else {
        return false;
    };
    if spellcheck_override_contains(&normalized) {
        return false;
    }
    let dictionary = spellcheck_dictionary();
    if dictionary.is_some_and(|dictionary| dictionary.contains(&normalized)) {
        return false;
    }
    if known_corrections(&normalized).is_some_and(|corrections| !corrections.is_empty()) {
        return true;
    }
    // Unknown names are not proof of misspelling. Without a real dictionary,
    // use curated source-code typos rather than a password word list.
    dictionary.is_some()
        && normalized.chars().count() >= 4
        && !word.chars().next().is_some_and(char::is_uppercase)
}

fn known_corrections(word: &str) -> Option<&'static [&'static str]> {
    typos_dict::WORD.find(&unicase::UniCase::new(word)).copied()
}

pub fn normalized_spellcheck_word(word: &str) -> Option<String> {
    let trimmed = word.trim_matches(['\'', '\u{2019}']);
    if trimmed.chars().count() < 3 {
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
    let mut normalized = trimmed.replace('\u{2019}', "'").to_lowercase();
    if normalized.ends_with("'s") {
        normalized.truncate(normalized.len().saturating_sub(2));
    }
    if normalized.chars().count() < 3 {
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
    if !is_misspelled_word(word) {
        return Vec::new();
    }
    if let Some(corrections) = known_corrections(&normalized) {
        return corrections
            .iter()
            .take(5)
            .map(|suggestion| match_spelling_case(word, suggestion))
            .collect();
    }
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
    let mut words = HashSet::new();
    for path in SPELLCHECK_DICT_PATHS {
        if std::fs::canonicalize(path)
            .is_ok_and(|path| path.to_string_lossy().contains("cracklib"))
        {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(path) else {
            continue;
        };
        for line in source.lines() {
            let word = line
                .trim()
                .trim_matches(['\'', '\u{2019}'])
                .replace('\u{2019}', "'")
                .to_lowercase();
            if !word.is_empty() && word.chars().all(|ch| ch.is_alphabetic() || ch == '\'')
            {
                words.insert(word);
            }
        }
    }
    (!words.is_empty()).then_some(words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn code_and_path_tokens_are_not_checked_as_prose() {
        let text = "ordinary frontend/src/main.rs cache_key64 person@example.com `responce code` we\u{2019}re";
        let words = spellcheck_words(text);
        assert_eq!(
            words.iter().map(|word| word.text).collect::<Vec<_>>(),
            vec!["ordinary", "we\u{2019}re"]
        );
        for word in words {
            assert_eq!(&text[word.start..word.start + word.text.len()], word.text);
        }
        assert_eq!(
            normalized_spellcheck_word("we\u{2019}re"),
            Some("we're".into())
        );
    }

    #[test]
    fn curated_fallback_catches_typos_without_rejecting_names_and_valid_words() {
        for word in [
            "Neoism",
            "Neovide",
            "Parker",
            "rendered",
            "wrapping",
            "doesn\u{2019}t",
        ] {
            assert!(!is_misspelled_word(word), "{word}");
        }
        assert!(is_misspelled_word("recieve"));
        assert!(spelling_suggestions("Recieve").contains(&"Receive".to_string()));
        assert!(!SPELLCHECK_DICT_PATHS
            .iter()
            .any(|path| path.contains("cracklib")));
    }

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
