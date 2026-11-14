//! The words Rune itself puts on screen: a text field's menu (Cut, Copy, Paste, Select All and
//! the spelling items), selectable text's Copy, and on macOS the menu bar's standard items and
//! menus. They are English until the app names them in its own language with [`set_labels`],
//! and [`set_locale`] says which language that is.
//!
//! Rune asks each time it shows one, so after the app's language changes the next menu opened
//! speaks it. The macOS menu bar takes the new names at the next [`AppHandle::set_menu`].
//!
//! [`AppHandle::set_menu`]: crate::AppHandle::set_menu
use std::sync::{Arc, RwLock};

/// Something Rune names on screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Label<'a> {
    Cut,
    Copy,
    Paste,
    SelectAll,
    Undo,
    Redo,
    /// A misspelled word's item that adds the word to the user's dictionary.
    LearnSpelling,
    /// Shown, disabled, in place of the suggestions when the checker has none.
    NoGuesses,
    /// The field menu's checkbox for the app's spellcheck setting (`El::on_spellcheck`).
    Spellcheck,
    /// The field menu's submenu of dictionaries (`El::on_spellcheck_languages`).
    SpellcheckLanguages,
    /// A dictionary in that submenu, by its language tag (`en-US`). Without a name from the app
    /// it's named in the app's language ([`set_locale`]) where the platform can, else in
    /// English. The submenu lists them in the order of their names.
    Language(&'a str),
    /// A top-level menu of the macOS menu bar, by the name its items give (`MenuItem::menu`):
    /// Rune's own "Edit" and "Window", or one of the app's.
    Menu(&'a str),
    /// The Window menu's items.
    Minimize,
    Close,
    /// The app menu's items; some carry the app's name.
    About(&'a str),
    Services,
    Hide(&'a str),
    HideOthers,
    ShowAll,
    Quit(&'a str),
}

type Namer = Arc<dyn Fn(Label<'_>) -> Option<String> + Send + Sync>;

static NAMER: RwLock<Option<Namer>> = RwLock::new(None);
static LOCALE: RwLock<Option<String>> = RwLock::new(None);

/// Names Rune's own labels with `names`, for every window. `None` keeps a label's English.
pub fn set_labels(names: impl Fn(Label<'_>) -> Option<String> + Send + Sync + 'static) {
    if let Ok(mut namer) = NAMER.write() {
        *namer = Some(Arc::new(names));
    }
}

/// The language the app's labels are in, as a BCP 47 tag (`de`, `zh-Hant`). Rune names the
/// spellcheck dictionaries in it when the app doesn't (`Label::Language`), as a browser's
/// `Intl.DisplayNames(locale)` would, and sorts them as it sorts text (`localeCompare`).
pub fn set_locale(tag: &str) {
    if let Ok(mut locale) = LOCALE.write() {
        *locale = Some(tag.to_owned());
    }
}

/// What `label` reads now: the app's name for it, else Rune's English.
pub(crate) fn label(label: Label<'_>) -> String {
    named(label).unwrap_or_else(|| english(label))
}

/// The app's name for `label`, if it gave one.
pub(crate) fn named(label: Label<'_>) -> Option<String> {
    let namer = NAMER.read().ok().and_then(|namer| namer.clone());
    namer.and_then(|names| names(label))
}

/// The app's language ([`set_locale`]), if it said.
pub(crate) fn locale() -> Option<String> {
    LOCALE.read().ok().and_then(|locale| locale.clone())
}

fn english(label: Label<'_>) -> String {
    match label {
        Label::Cut => "Cut".into(),
        Label::Copy => "Copy".into(),
        Label::Paste => "Paste".into(),
        Label::SelectAll => "Select All".into(),
        Label::Undo => "Undo".into(),
        Label::Redo => "Redo".into(),
        Label::LearnSpelling => "Learn Spelling".into(),
        Label::NoGuesses => "No Guesses Found".into(),
        Label::Spellcheck => "Spellcheck".into(),
        Label::SpellcheckLanguages => "Spellcheck Languages".into(),
        // The field menu names a dictionary itself when the app doesn't (`spell::language_name`).
        Label::Language(tag) => tag.into(),
        Label::Menu(name) => name.into(),
        Label::Minimize => "Minimize".into(),
        Label::Close => "Close".into(),
        Label::About(app) => format!("About {app}"),
        Label::Services => "Services".into(),
        Label::Hide(app) => format!("Hide {app}"),
        Label::HideOthers => "Hide Others".into(),
        Label::ShowAll => "Show All".into(),
        Label::Quit(app) => format!("Quit {app}"),
    }
}
