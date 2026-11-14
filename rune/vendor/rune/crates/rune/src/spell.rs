//! The platform's spell checker for text fields that ask for it (`El::spellcheck`): Windows'
//! spell checking API (`ISpellChecker`), AppKit's `NSSpellChecker` on macOS, none elsewhere.
//! A field's misspelled words get a red marker under them and their suggestions in its menu.
//!
//! Where the platform checks one language at a time (Windows), an app picks the dictionaries:
//! [`available_languages`], [`set_languages`] and the fields' Spellcheck Languages menu
//! (`El::on_spellcheck_languages`). A word is misspelled when every chosen dictionary says so,
//! as Chromium's multilingual checking does. macOS picks each text's language itself.
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "spell/pause.rs"]
pub(crate) mod pause;
#[cfg(target_os = "windows")]
#[path = "spell/windows.rs"]
mod platform;
#[cfg(target_os = "macos")]
#[path = "spell/macos.rs"]
mod platform;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod platform {
    pub(super) fn misspelled(_: &str) -> Vec<(usize, usize)> { Vec::new() }
    pub(super) fn suggest(_: &str) -> Vec<String> { Vec::new() }
    pub(super) fn learn(_: &str) {}
    pub(super) fn available() -> Vec<String> { Vec::new() }
    pub(super) fn preferred() -> Vec<String> { Vec::new() }
    pub(super) fn english_name(tag: &str) -> String { tag.to_owned() }
    pub(super) fn name_in(_: &str, _: &str) -> Option<String> { None }
    pub(super) fn sort_in<T>(_: &mut [(String, T)], _: &str) -> bool { false }
}

/// The most suggestions a field's menu offers, as Chromium's.
const MAX_SUGGESTIONS: usize = 5;

/// A text's misspelled words, as byte ranges.
type Ranges = Rc<Vec<(usize, usize)>>;

/// The dictionaries [`set_languages`] chose; empty follows the system languages.
static CHOSEN: Mutex<Vec<String>> = Mutex::new(Vec::new());
/// Bumped on each [`set_languages`], so each thread's checkers and checked texts start over.
static GENERATION: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// Each checked text's misspelled words, by the text's hash, for one generation.
    static CHECKED: RefCell<(u64, HashMap<u64, Ranges>)> = RefCell::new((0, HashMap::new()));
}

/// The dictionaries a field's Spellcheck Languages menu offers, as language tags (`en-US`).
/// Empty where the platform finds each text's language itself (macOS) or has no checker.
pub fn available_languages() -> Vec<String> {
    let mut found = platform::available();
    found.sort_by_key(|tag| language_name(tag));
    found
}

/// The dictionaries checking text now: those [`set_languages`] chose that the platform has, else
/// one per system language (matched exactly or by base language, `de-AT` to `de`), with US
/// English added when none of them is English, as Desktop's `defaultSpellcheckLanguages`.
pub fn languages() -> Vec<String> {
    let available = platform::available();
    let chosen = CHOSEN.lock().map(|c| c.clone()).unwrap_or_default();
    let chosen: Vec<String> = available.iter().filter(|tag| chosen.contains(tag)).cloned().collect();
    if !chosen.is_empty() {
        return chosen;
    }
    default_languages(&available, &platform::preferred())
}

/// Check with these dictionaries. None, or none the platform has, follows the system languages.
pub fn set_languages(tags: &[String]) {
    if let Ok(mut chosen) = CHOSEN.lock() {
        if chosen.as_slice() == tags {
            return;
        }
        *chosen = tags.to_vec();
    }
    GENERATION.fetch_add(1, Ordering::Relaxed);
}

/// A language tag's English name as a browser's `Intl.DisplayNames` gives it: `en-US` is
/// "American English", `fr-FR` "French (France)".
pub fn language_name(tag: &str) -> String {
    DIALECTS.iter().find(|(t, _)| t.eq_ignore_ascii_case(tag)).map_or_else(|| platform::english_name(tag), |(_, name)| (*name).to_owned())
}

/// A language tag's name in `locale`'s language as a browser's `Intl.DisplayNames(locale)`
/// gives it, from the platform's ICU: `en-US` in `de` is "Amerikanisches Englisch". `None`
/// where the platform has no ICU to ask (only Windows has dictionaries to name).
pub(crate) fn language_name_in(tag: &str, locale: &str) -> Option<String> {
    platform::name_in(tag, locale)
}

/// Sorts `items` by their names as `locale` sorts text (`String.localeCompare`), else by their
/// lowercased names.
pub(crate) fn sort_names_in<T>(items: &mut [(String, T)], locale: &str) {
    if !platform::sort_in(items, locale) {
        items.sort_by_cached_key(|(name, _)| name.to_lowercase());
    }
}

/// CLDR's English dialect names, which `Intl.DisplayNames` uses over "Language (Region)".
const DIALECTS: &[(&str, &str)] = &[
    ("ar-001", "Modern Standard Arabic"),
    ("de-AT", "Austrian German"),
    ("de-CH", "Swiss High German"),
    ("en-AU", "Australian English"),
    ("en-CA", "Canadian English"),
    ("en-GB", "British English"),
    ("en-US", "American English"),
    ("es-419", "Latin American Spanish"),
    ("es-ES", "European Spanish"),
    ("es-MX", "Mexican Spanish"),
    ("fa-AF", "Dari"),
    ("fr-CA", "Canadian French"),
    ("fr-CH", "Swiss French"),
    ("nl-BE", "Flemish"),
    ("pt-BR", "Brazilian Portuguese"),
    ("pt-PT", "European Portuguese"),
    ("ro-MD", "Moldavian"),
    ("sw-CD", "Congo Swahili"),
    ("zh-Hans", "Simplified Chinese"),
    ("zh-Hant", "Traditional Chinese"),
];

/// Desktop's `defaultSpellcheckLanguages` (spellcheck-languages.ts): one dictionary per preferred
/// language, then US English when none of them is English, as prompts mix prose with English
/// code terms.
fn default_languages(available: &[String], preferred: &[String]) -> Vec<String> {
    let base = |tag: &str| tag.split('-').next().unwrap_or(tag).to_ascii_lowercase();
    let find = |tag: &str| available.iter().find(|a| a.eq_ignore_ascii_case(tag)).cloned();
    let mut found: Vec<String> = Vec::new();
    for tag in preferred {
        if let Some(language) = find(tag).or_else(|| find(&base(tag)))
            && !found.iter().any(|f| base(f) == base(&language))
        {
            found.push(language);
        }
    }
    if found.iter().any(|f| base(f) == "en") || !available.iter().any(|a| a == "en-US") {
        return found;
    }
    found.push("en-US".to_owned());
    found
}

/// The current generation, for a thread's checkers to know when to start over.
pub(crate) fn generation() -> u64 { GENERATION.load(Ordering::Relaxed) }

/// The byte ranges of `text`'s misspelled words, checked once per text.
pub(crate) fn misspelled(text: &str) -> Ranges {
    if text.trim().is_empty() {
        return Rc::default();
    }
    let key = {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        text.hash(&mut h);
        h.finish()
    };
    CHECKED.with_borrow_mut(|(generation, checked)| {
        if *generation != self::generation() {
            *generation = self::generation();
            checked.clear();
        }
        if let Some(found) = checked.get(&key) {
            return found.clone();
        }
        // Typing checks a new text per key: keep the cache to the latest few hundred.
        if checked.len() > 256 {
            checked.clear();
        }
        let found = Rc::new(platform::misspelled(text));
        checked.insert(key, found.clone());
        found
    })
}

/// The misspelled word of `text` at byte `at` (in it or at its end).
pub(crate) fn misspelled_at(text: &str, at: usize) -> Option<(usize, usize)> {
    misspelled(text).iter().copied().find(|&(a, b)| a <= at && at <= b)
}

/// The checker's suggestions for misspelled `word`, best first.
pub(crate) fn suggest(word: &str) -> Vec<String> {
    let mut found = platform::suggest(word);
    found.retain(|s| s != word);
    found.truncate(MAX_SUGGESTIONS);
    found
}

/// Adds `word` to the user's dictionary, so it's no longer marked anywhere.
pub(crate) fn learn(word: &str) {
    platform::learn(word);
    CHECKED.with_borrow_mut(|(_, checked)| checked.clear());
}
