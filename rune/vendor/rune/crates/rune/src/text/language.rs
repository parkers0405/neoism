//! The language an app's text is in. Chinese, Japanese and Korean share the Han characters
//! but draw many of them differently, so the language picks which CJK fonts Han text falls
//! back to, and shaping gets it for the font's locale forms (`locl`). Kana always prefers
//! Japanese fonts, Hangul Korean ones and Bopomofo Traditional Chinese ones, whatever the
//! language.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use parley::fontique::{Collection, FallbackKey, FamilyId, Language, Script};

use super::TextSystem;

static CHOSEN: Mutex<Option<Language>> = Mutex::new(None);
/// Counts changes to `CHOSEN`, so a text system notices one with an atomic load.
static EPOCH: AtomicU64 = AtomicU64::new(0);

/// Sets the language of every text system's text: a BCP 47 tag such as `ja`, `zh-Hans`,
/// `zh-Hant` or `ko`, or `None`. An app passes its interface language. Han text then falls
/// back to that language's fonts (Hiragino Sans, Yu Gothic or Noto Sans CJK JP for `ja`)
/// instead of Chinese ones, and shapes with its locale forms. Text systems drop what they
/// shaped in the old language before their next layout. A tag that doesn't parse counts as
/// none.
pub fn set_language(tag: Option<&str>) {
    let language = tag.and_then(|tag| Language::parse(tag).ok());
    let mut chosen = CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
    if *chosen != language {
        *chosen = language;
        EPOCH.fetch_add(1, Ordering::Relaxed);
    }
}

impl TextSystem {
    /// The language this text system shapes in.
    pub fn language(&self) -> Option<Language> {
        self.language
    }

    /// Shapes in `language` from now on, dropping layouts shaped in another. False, dropping
    /// nothing, when it already does.
    pub fn set_language(&mut self, language: Option<Language>) -> bool {
        if self.language == language {
            return false;
        }
        self.language = language;
        self.drop_shaped();
        true
    }

    /// Takes up the language [`set_language`] chose since this text system last looked.
    pub(super) fn follow_language(&mut self) {
        let epoch = EPOCH.load(Ordering::Relaxed);
        if epoch == self.language_epoch {
            return;
        }
        self.language_epoch = epoch;
        let chosen = *CHOSEN.lock().unwrap_or_else(|e| e.into_inner());
        self.set_language(chosen);
    }
}

/// A CJK writing system: the parts of the names of the font families that draw it, most
/// preferred first, and of families that match those but draw another.
struct Writing {
    families: &'static [&'static str],
    not: &'static [&'static str],
}

const JAPANESE: Writing = Writing {
    families: &["Hiragino Sans", "Hiragino Kaku Gothic", "ヒラギノ角ゴ", "Yu Gothic", "Meiryo", "CJK JP", "Noto Sans JP", "Source Han Sans JP"],
    // Hiragino Sans GB is Chinese despite its name.
    not: &[" GB"],
};
const KOREAN: Writing = Writing {
    families: &["SD Gothic", "Malgun", "CJK KR", "Noto Sans KR", " KR", "AppleGothic", "Nanum"],
    not: &[],
};
const SIMPLIFIED: Writing = Writing {
    families: &["PingFang SC", "Microsoft YaHei", "CJK SC", "Noto Sans SC", "Source Han Sans SC", "Hiragino Sans GB"],
    not: &[],
};
const TRADITIONAL: Writing = Writing {
    families: &["PingFang TC", "Microsoft JhengHei", "CJK TC", "Noto Sans TC", "Source Han Sans TC"],
    not: &[],
};
const HONG_KONG: Writing = Writing {
    families: &["PingFang HK", "CJK HK", "Noto Sans HK", "Source Han Sans HC", "PingFang TC", "Microsoft JhengHei", "CJK TC", "Noto Sans TC"],
    not: &[],
};

/// Where `name` stands in `writing`'s preference, if it draws it.
fn draws(writing: &Writing, name: &str) -> Option<usize> {
    if writing.not.iter().any(|part| name.contains(part)) {
        return None;
    }
    writing.families.iter().position(|part| name.contains(part))
}

/// `names` reordered for `writing`: its families first, in its order of preference, then
/// the rest as they were.
fn order(names: &[Option<String>], writing: &Writing) -> Vec<usize> {
    let mut preferred: Vec<(usize, usize)> = names
        .iter()
        .enumerate()
        .filter_map(|(i, name)| Some((draws(writing, name.as_deref()?)?, i)))
        .collect();
    preferred.sort();
    let mut out: Vec<usize> = preferred.into_iter().map(|(_, i)| i).collect();
    let rest: Vec<usize> = (0..names.len()).filter(|i| !out.contains(i)).collect();
    out.extend(rest);
    out
}

/// Registers `fallbacks` (the CJK fonts in file order) for each CJK script, ordered for the
/// writing each script and language implies: Han by language (Simplified Chinese when none),
/// kana for Japanese, Hangul for Korean, Bopomofo for Traditional Chinese.
pub(super) fn register(collection: &mut Collection, fallbacks: &[FamilyId]) {
    let names: Vec<Option<String>> = fallbacks.iter().map(|id| collection.family_name(*id).map(str::to_owned)).collect();
    let ordered = |writing: &Writing| order(&names, writing).into_iter().map(|i| fallbacks[i]).collect::<Vec<_>>();
    let mut add = |script: &[u8; 4], locale: Option<&str>, writing: &Writing| {
        let locale = locale.and_then(|tag| Language::parse(tag).ok());
        collection.append_fallbacks(FallbackKey::new(Script::from_bytes(*script), locale.as_ref()), ordered(writing).into_iter());
    };
    // `zh-CN` is Han's default key, the one text with no language uses.
    add(b"Hani", None, &SIMPLIFIED);
    add(b"Hani", Some("ja"), &JAPANESE);
    add(b"Hani", Some("ko"), &KOREAN);
    add(b"Hani", Some("zh-TW"), &TRADITIONAL);
    add(b"Hani", Some("zh-HK"), &HONG_KONG);
    add(b"Hani", Some("zh-MO"), &HONG_KONG);
    add(b"Hira", None, &JAPANESE);
    add(b"Kana", None, &JAPANESE);
    add(b"Hang", None, &KOREAN);
    add(b"Bopo", None, &TRADITIONAL);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn named(names: &[&str]) -> Vec<Option<String>> {
        names.iter().map(|name| Some((*name).to_owned())).collect()
    }

    /// Each language's fonts come first, in its order, and the rest keep theirs.
    #[test]
    fn fallback_order_follows_the_language() {
        let mac = named(&["PingFang SC", "PingFang TC", "PingFang HK", "Hiragino Sans GB", "Hiragino Sans", "Apple SD Gothic Neo"]);
        assert_eq!(order(&mac, &JAPANESE)[0], 4, "Hiragino Sans, not Hiragino Sans GB");
        assert_eq!(order(&mac, &KOREAN)[0], 5);
        assert_eq!(order(&mac, &SIMPLIFIED)[..2], [0, 3]);
        assert_eq!(order(&mac, &TRADITIONAL)[0], 1);
        assert_eq!(order(&mac, &HONG_KONG)[..2], [2, 1]);
        let windows = named(&["Microsoft YaHei", "Yu Gothic", "Meiryo", "Malgun Gothic", "Microsoft JhengHei"]);
        assert_eq!(order(&windows, &JAPANESE), [1, 2, 0, 3, 4]);
        assert_eq!(order(&windows, &KOREAN)[0], 3);
        assert_eq!(order(&windows, &TRADITIONAL)[0], 4);
        let linux = named(&["Noto Sans CJK SC", "Noto Sans CJK TC", "Noto Sans CJK HK", "Noto Sans CJK JP", "Noto Sans CJK KR"]);
        assert_eq!(order(&linux, &JAPANESE)[0], 3);
        assert_eq!(order(&linux, &KOREAN)[0], 4);
        assert_eq!(order(&linux, &SIMPLIFIED)[0], 0);
        assert_eq!(order(&linux, &TRADITIONAL)[0], 1);
        assert_eq!(order(&linux, &HONG_KONG)[0], 2);
        // A family with no name keeps its place among the rest.
        assert_eq!(order(&[None, Some("Yu Gothic".into())], &JAPANESE), [1, 0]);
    }

    /// Setting the language it already shapes in changes nothing.
    #[test]
    fn a_text_system_changes_language_once() {
        let mut ts = TextSystem::default();
        let ja = Language::parse("ja").ok();
        assert!(ts.set_language(ja));
        assert!(!ts.set_language(ja));
        assert_eq!(ts.language(), ja);
    }
}
