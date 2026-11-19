//! AppKit's shared `NSSpellChecker`, which picks each text's language itself as Chromium's
//! does on macOS.
use objc2_app_kit::NSSpellChecker;
use objc2_foundation::{NSRange, NSString};
use std::sync::Mutex;

// The shared checker is mutable. Serialize *all* Rune access, including field
// checks/learning on the UI thread and Markdown checks on a worker. Only the
// non-panel spelling API is used here; no AppKit views cross threads.
static CHECKER: Mutex<()> = Mutex::new(());

fn with_checker<T>(f: impl FnOnce(&NSSpellChecker) -> T) -> T {
    let _guard = CHECKER.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // A long-lived Rust worker has no Cocoa event loop to drain autoreleases.
    // Convert all returned objects to Rust values before this pool is drained.
    objc2::rc::autoreleasepool(|_| {
        // SAFETY: all use of the mutable shared checker in this module is
        // serialized. We neither use its panel nor send Objective-C objects to
        // another thread; the retained checker lives through this call.
        let checker = unsafe { NSSpellChecker::sharedSpellChecker() };
        f(&checker)
    })
}

/// `NSNotFound`, the location of a range that found nothing.
const NOT_FOUND: usize = isize::MAX as usize;

pub(super) fn misspelled(text: &str) -> Vec<(usize, usize)> {
    with_checker(|checker| misspelled_with(checker, text))
}

fn misspelled_with(checker: &NSSpellChecker, text: &str) -> Vec<(usize, usize)> {
    // Each UTF-16 unit's byte offset, and the text's end.
    let mut bytes = Vec::with_capacity(text.len() + 1);
    for (i, c) in text.char_indices() {
        bytes.extend(std::iter::repeat_n(i, c.len_utf16()));
    }
    bytes.push(text.len());
    let string = NSString::from_str(text);
    let mut found = Vec::new();
    let mut from = 0usize;
    // SAFETY: with_checker serializes access and owns the autorelease pool;
    // string and its valid range outlive each call. No spelling panel is used.
    unsafe {
        checker.setAutomaticallyIdentifiesLanguages(true);
        while from < bytes.len() - 1 {
            let range = checker.checkSpellingOfString_startingAt_language_wrap_inSpellDocumentWithTag_wordCount(
                &string, from as isize, None, false, 0, std::ptr::null_mut(),
            );
            if range.location == NOT_FOUND || range.length == 0 || range.location < from {
                break;
            }
            let a = range.location;
            let Some(b) = a.checked_add(range.length) else { break };
            if let (Some(&a), Some(&b)) = (bytes.get(a), bytes.get(b)) {
                if a < b && text.is_char_boundary(a) && text.is_char_boundary(b) {
                    found.push((a, b));
                }
            }
            from = b;
        }
    }
    found
}

pub(super) fn suggest(word: &str) -> Vec<String> {
    with_checker(|checker| suggest_with(checker, word))
}

fn suggest_with(checker: &NSSpellChecker, word: &str) -> Vec<String> {
    let string = NSString::from_str(word);
    let range = NSRange::new(0, string.length());
    // SAFETY: as in `misspelled`; `range` is all of `string`.
    unsafe {
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
    with_checker(|checker| {
        // SAFETY: serialized non-panel access, with a live string and pool.
        unsafe { checker.learnWord(&NSString::from_str(word)) }
    });
}
