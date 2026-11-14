//! A text field's own menu (right-click, macOS control-click, the Menu key and Shift+F10), as a
//! browser's menu over an editable field: a misspelled word's suggestions and Learn Spelling,
//! then Cut, Copy, Paste and, off macOS, Select All, then the app's Spellcheck setting
//! (`El::on_spellcheck`) and, where there are dictionaries to choose, its Spellcheck Languages
//! (`El::on_spellcheck_languages`). A field with its own `on_secondary_press` shows that instead.
//! The items read the app's names for them (`rune::set_labels`) each time the menu opens.
use super::input_click::hit;
use super::*;
use crate::labels::label;
use crate::popup_menu::{PopupAnchor, Request};
use crate::{Key, Label, Mods, PopupItem, PopupMenu};

/// What a field menu's items act on once one is picked.
pub(crate) struct FieldMenu {
    path: String,
    /// The misspelled word the menu offers suggestions for: its byte range and text.
    word: Option<(usize, usize, String)>,
    suggestions: Vec<String>,
    toggle: Option<(bool, crate::el::WindowFocusFn)>,
    /// The dictionaries checking text when the menu opened, and what keeps a new choice.
    languages: Option<(Vec<String>, crate::el::LanguagesFn)>,
}

impl<S: 'static> Runtime<S> {
    /// Opens field `path`'s menu at window point `at`, pressed at `local` in the field (points
    /// from its text's top-left), or from the keyboard (`None`).
    pub(super) fn field_menu(&mut self, path: String, at: (f32, f32), local: Option<(f32, f32)>, one: &mut Tx) -> bool {
        let Some(i) = self.live(&path) else { return false };
        let Some(input) = self.nodes[i].input.as_deref() else { return false };
        let Some(access) = input.access.clone() else { return false };
        let (single, secure) = (input.single_line, input.edit.is_secure());
        let spell = input.spellcheck && !secure;
        let layout = self.nodes[i].text_layout.clone();
        let scroll = self.input_scroll.get(&path).copied().unwrap_or(0.0);
        let s = self.env.scale;
        let toggle = std::iter::successors(Some(i), |&j| self.nodes[j].parent).find_map(|j| self.nodes[j].handlers.on_spellcheck.clone());
        let keep = std::iter::successors(Some(i), |&j| self.nodes[j].parent).find_map(|j| self.nodes[j].handlers.on_spellcheck_languages.clone());
        if self.focus.path() != Some(path.as_str()) {
            self.set_focus(Some(path.clone()), false);
        }
        let e = access(states!(self));
        e.set_single_line(single);
        if let (Some((x, y)), Some(l)) = (local, layout.as_deref())
            && e.preedit().is_none()
        {
            let caret = hit(e, l, x + scroll, y, s, single).caret;
            let (a, b) = e.range();
            // A press in the selection keeps it for Cut and Copy. Elsewhere it moves the caret
            // there, and on a misspelled word selects the word, as Chromium does.
            if a == b || caret < a || caret > b {
                match spell.then(|| crate::spell::misspelled_at(&e.text, caret)).flatten() {
                    Some((wa, wb)) => e.set_selection(wa, wb),
                    None => e.set_caret(caret),
                }
            }
        }
        let (a, b) = e.range();
        let word = spell
            .then(|| crate::spell::misspelled(&e.text))
            .filter(|found| found.contains(&(a, b)))
            .map(|_| (a, b, e.text[a..b].to_owned()));
        let suggestions = word.as_ref().map(|w| crate::spell::suggest(&w.2)).unwrap_or_default();
        let mut menu = PopupMenu::new();
        if word.is_some() {
            if suggestions.is_empty() {
                menu = menu.item(PopupItem::new("no-guesses", label(Label::NoGuesses)).enabled(false));
            }
            menu = menu.items(suggestions.iter().enumerate().map(|(n, s)| PopupItem::new(format!("suggestion-{n}"), s.clone())));
            menu = menu.separator().item(PopupItem::new("learn-spelling", label(Label::LearnSpelling))).separator();
        }
        let selected = a != b && !secure;
        menu = menu
            .item(PopupItem::new("cut", label(Label::Cut)).enabled(selected))
            .item(PopupItem::new("copy", label(Label::Copy)).enabled(selected))
            .item(PopupItem::new("paste", label(Label::Paste)));
        if !cfg!(target_os = "macos") {
            menu = menu.item(PopupItem::new("select-all", label(Label::SelectAll)));
        }
        let mut languages = None;
        if let Some((on, _)) = &toggle {
            menu = menu.separator().item(PopupItem::new("spellcheck", label(Label::Spellcheck)).checked(*on));
            // Desktop's dictionary picker (spellcheck.ts), where the platform has dictionaries
            // to choose: the last one stays, as the Spellcheck item is how to stop checking.
            let available = crate::spell::available_languages();
            if let Some(keep) = keep.filter(|_| !available.is_empty()) {
                let current = crate::spell::languages();
                let only = |tag: &String| current.len() == 1 && current[0] == *tag;
                // Named in the app's language, in the order of those names, as Desktop's
                // `nativeLanguageOptions` sorts its `Intl.DisplayNames`.
                let locale = crate::labels::locale();
                let name = |tag: &str| {
                    crate::labels::named(Label::Language(tag))
                        .or_else(|| locale.as_deref().and_then(|locale| crate::spell::language_name_in(tag, locale)))
                        .unwrap_or_else(|| crate::spell::language_name(tag))
                };
                let mut named: Vec<(String, &String)> = available.iter().map(|tag| (name(tag), tag)).collect();
                crate::spell::sort_names_in(&mut named, locale.as_deref().unwrap_or("en"));
                let items = named.into_iter().map(|(name, tag)| {
                    PopupItem::new(format!("language-{tag}"), name).checked(current.contains(tag)).enabled(!only(tag))
                });
                menu = menu.submenu_enabled(label(Label::SpellcheckLanguages), PopupMenu::new().items(items), *on);
                languages = Some((current, keep));
            }
        }
        one.instant = true;
        log_event!(self, "field menu {path}");
        self.popups.push_back(Request {
            menu,
            anchor: PopupAnchor::Point(at.0, at.1),
            on_pick: Box::new(|_, _, _| {}),
            field: Some(Box::new(FieldMenu { path, word, suggestions, toggle, languages })),
        });
        true
    }

    /// Does what `picked` in field menu `menu` asks, on its field.
    pub(super) fn field_menu_pick(&mut self, menu: FieldMenu, picked: &str, one: &mut Tx) {
        if self.focus.path() != Some(menu.path.as_str()) && self.live(&menu.path).is_some() {
            self.set_focus(Some(menu.path.clone()), false);
        }
        let command = |c| KeyEvent { key: Key::Command(c), mods: Mods { cmd: true, ctrl: !cfg!(target_os = "macos"), ..Mods::default() } };
        match picked {
            "cut" | "copy" | "select-all" => {
                let ev = command(match picked {
                    "cut" => 'x',
                    "copy" => 'c',
                    _ => 'a',
                });
                let layout = self.focused_layout();
                self.with_focused(|e| e.key(&ev, layout.as_deref()), one);
            }
            "paste" => one.request_paste(),
            "learn-spelling" => {
                if let Some((_, _, word)) = &menu.word {
                    crate::spell::learn(word);
                    self.dirty = true;
                }
            }
            "spellcheck" => {
                if let Some((on, f)) = menu.toggle {
                    f(states!(self), !on, one);
                }
            }
            id if id.starts_with("language-") => {
                if let (Some((mut current, keep)), Some(tag)) = (menu.languages, id.strip_prefix("language-")) {
                    match current.iter().position(|t| t == tag) {
                        Some(n) => _ = current.remove(n),
                        None => current.push(tag.to_owned()),
                    }
                    crate::spell::set_languages(&current);
                    keep(states!(self), current, one);
                    self.dirty = true;
                }
            }
            id => {
                let n = id.strip_prefix("suggestion-").and_then(|n| n.parse::<usize>().ok());
                if let (Some((a, b, _)), Some(suggestion)) = (menu.word, n.and_then(|n| menu.suggestions.get(n).cloned())) {
                    self.with_focused(
                        |e| {
                            e.set_selection(a, b);
                            e.paste(&suggestion)
                        },
                        one,
                    );
                }
            }
        }
    }
}
