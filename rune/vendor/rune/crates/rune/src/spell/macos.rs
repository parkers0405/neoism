//! AppKit's shared `NSSpellChecker`, which picks each text's language itself as Chromium's
//! does on macOS.
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};

/// `NSNotFound`, the location of a range that found nothing.
const NOT_FOUND: usize = isize::MAX as usize;

pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    // Each UTF-16 unit's byte offset, and the text's end.
    let mut bytes = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        bytes.extend(std::iter::repeat_n(i, c.len_utf16()));
    }
    bytes.push(text.len());
    let string = NSString::from_str(text);
    let mut found = Vec::new();
    let mut from = 0usize;
    // SAFETY: the shared checker is AppKit's own, and `string` outlives each call on it.
    unsafe {
        let checker = NSSpellChecker::sharedSpellChecker();
        while from < bytes.len() - 1 {
            let range = checker.checkSpellingOfString_startingAt(&string, from as isize);
            if range.location == NOT_FOUND || range.length == 0 || range.location < from {
                break;
            }
            let (a, b) = (range.location, range.location + range.length);
            if let (Some(&a), Some(&b)) = (bytes.get(a), bytes.get(b)) {
                found.push((a, b));
            }
            from = b;
        }
    }
    found
}

pub(super) fn suggest(word: &str) -> Vec<String> {
    let string = NSString::from_str(word);
    let range = NSRange::new(0, string.length());
    // SAFETY: as in `misspelled`; `range` is all of `string`.
    unsafe {
        let checker = NSSpellChecker::sharedSpellChecker();
        let Some(guesses) = checker.guessesForWordRange_inString_language_inSpellDocumentWithTag(range, &string, None, 0) else {
            return Vec::new();
        };
        guesses.iter().map(|g| g.to_string()).collect()
    }
}

/// None to choose: AppKit finds each text's language itself, as Desktop leaves it on macOS.
pub(super) fn available() -> Vec<String> { Vec::new() }

pub(super) fn preferred() -> Vec<String> { Vec::new() }

pub(super) fn english_name(tag: &str) -> String { tag.to_owned() }

/// Unused: macOS has no dictionaries to choose, so no list names them.
pub(super) fn name_in(_: &str, _: &str) -> Option<String> { None }

pub(super) fn sort_in<T>(_: &mut [(String, T)], _: &str) -> bool { false }

pub(super) fn learn(word: &str) {
    // SAFETY: the shared checker is AppKit's own and the word a live string.
    unsafe { NSSpellChecker::sharedSpellChecker().learnWord(&NSString::from_str(word)) }
}
