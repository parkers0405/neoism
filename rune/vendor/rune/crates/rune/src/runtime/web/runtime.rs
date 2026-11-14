//! Browser-only access to the focused edit, kept beside Runtime so the shell sees no nodes.
#![cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
use super::*;

/// The last context sent to the DOM. Secure text never enters this cache.
#[derive(Default)]
pub(crate) struct Mirror {
    pub key: String,
    pub text: String,
    /// UTF-8 byte offset in the model; DOM offsets are UTF-16 relative to this window.
    pub base: usize,
    pub anchor: u32,
    pub focus: u32,
    pub secure: bool,
}

/// Read at most 512 UTF-16 units on each side, without scanning the document prefix.
fn context_window(edit: &crate::TextEdit) -> (usize, &str, u32, u32) {
    let (a, b) = edit.range();
    let (raw_anchor, raw_focus) = if edit.anchor <= edit.focus { (a, b) } else { (b, a) };
    let mut start = a;
    let mut units = 0;
    for (i, ch) in edit.text[..a].char_indices().rev() {
        if units + ch.len_utf16() > 512 { break; }
        units += ch.len_utf16();
        start = i;
    }
    let mut end = b;
    let mut units = 0;
    for ch in edit.text[b..].chars() {
        if units + ch.len_utf16() > 512 { break; }
        units += ch.len_utf16();
        end += ch.len_utf8();
    }
    let anchor = edit.text[start..raw_anchor].encode_utf16().count() as u32;
    let focus = edit.text[start..raw_focus].encode_utf16().count() as u32;
    (start, &edit.text[start..end], anchor, focus)
}

impl Mirror {
    fn update(&mut self, key: &str, edit: Option<&crate::TextEdit>) -> bool {
        let secure = edit.is_some_and(|e| e.is_secure());
        let (base, text, anchor, focus) = edit.filter(|_| !secure).map(context_window).unwrap_or((0, "", 0, 0));
        if self.key == key && self.base == base && self.text == text && self.anchor == anchor
            && self.focus == focus && self.secure == secure { return false; }
        self.key.clear(); self.key.push_str(key);
        self.text.clear(); self.text.push_str(text);
        self.base = base; self.anchor = anchor; self.focus = focus; self.secure = secure;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn utf16_replacement_preserves_callbacks_and_rejects_stale_context() {
        fn view(state: &(crate::TextEdit, usize), _: &mut Cx<(crate::TextEdit, usize)>) -> El {
            crate::text_input(&state.0, crate::tok::type_::BODY).key("input")
                .on_edit(|s: &mut (crate::TextEdit, usize)| &mut s.0)
                .on_input_change(|s: &mut (crate::TextEdit, usize), _| s.1 += 1)
        }
        let mut rt = Runtime::new((crate::TextEdit::new("a🦀c"), 0), view, (300.0, 100.0), Env::default());
        rt.frame(0.0);
        let key = rt.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
        rt.set_focus(Some(key.clone()), true);
        let event = json!({"kind":"edit", "key":key, "base":0, "expected":"a🦀c",
            "start":1, "end":3, "text":"é", "anchor":2, "focus":2});
        rt.web_edit(&event);
        rt.frame(0.0);
        assert_eq!((&*rt.state.0.text, rt.state.1, rt.state.0.focus), ("aéc", 1, 3));
        rt.web_edit(&event);
        rt.frame(0.0);
        assert_eq!((&*rt.state.0.text, rt.state.1), ("aéc", 1));
        assert_eq!(byte_at("a🦀c", 2), None);
    }

    #[test]
    fn context_is_bounded_at_unicode_boundaries_and_keeps_the_selection() {
        let mut edit = crate::TextEdit::new("é🦀a".repeat(800));
        let boundaries: Vec<_> = edit.text.char_indices().map(|(i, _)| i).chain([edit.text.len()]).collect();
        for &a in boundaries.iter().step_by(157) {
            for &f in boundaries.iter().step_by(271) {
                edit.anchor = a; edit.focus = f;
                let (base, text, anchor, focus) = context_window(&edit);
                assert_eq!(base + byte_at(text, anchor as usize).unwrap(), a);
                assert_eq!(base + byte_at(text, focus as usize).unwrap(), f);
                let before = edit.text[base..a.min(f)].encode_utf16().count();
                let after = edit.text[a.max(f)..base + text.len()].encode_utf16().count();
                assert!(before <= 512 && after <= 512);
                if base > 0 { assert!(before + edit.text[..base].chars().next_back().unwrap().len_utf16() > 512); }
                if base + text.len() < edit.text.len() {
                    assert!(after + edit.text[base + text.len()..].chars().next().unwrap().len_utf16() > 512);
                }
            }
        }
    }

    #[test]
    fn mirror_skips_unchanged_frames_and_never_caches_secure_text() {
        let mut mirror = Mirror::default();
        let mut edit = crate::TextEdit::new("🦀".repeat(10_000));
        assert!(mirror.update("field", Some(&edit)));
        assert_eq!(mirror.text.encode_utf16().count(), 512);
        let allocation = mirror.text.as_ptr();
        for _ in 0..120 { assert!(!mirror.update("field", Some(&edit))); }
        assert_eq!(mirror.text.as_ptr(), allocation);
        edit.anchor -= 4;
        assert!(mirror.update("field", Some(&edit)), "selection-only changes sync");
        edit.text.push('x');
        assert!(mirror.update("field", Some(&edit)), "edits within the window sync");
        assert!(mirror.update("other", Some(&edit)), "same contents in a different field sync");
        edit.set_secure(true);
        assert!(mirror.update("other", Some(&edit)));
        assert!(mirror.secure && mirror.text.is_empty());
        assert_eq!((mirror.base, mirror.anchor, mirror.focus), (0, 0, 0));
        edit.insert("secret");
        assert!(!mirror.update("other", Some(&edit)), "secret edits don't mirror to the DOM");
        edit.set_revealed(true);
        assert!(!mirror.update("other", Some(&edit)), "revealing changes only the GPU display, not DOM text services");
        assert!(mirror.secure && mirror.text.is_empty());
        assert_eq!((mirror.base, mirror.anchor, mirror.focus), (0, 0, 0));
        assert!(mirror.update("", None));
        assert!(!mirror.update("", None));
    }

    #[test]
    fn recomposition_replaces_its_range_without_changing_committed_text() {
        fn view(edit: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(edit, crate::tok::type_::BODY).key("input").on_edit(|s: &mut crate::TextEdit| s)
        }
        let mut rt = Runtime::new(crate::TextEdit::new("🦀 hello "), view, (300.0, 100.0), Env::default());
        rt.frame(0.0);
        let key = rt.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
        rt.set_focus(Some(key.clone()), true);
        let mut event = json!({"kind":"preedit", "key":key, "base":5, "expected":"hello ",
            "start":0, "end":6, "text":"hello", "anchor":5, "focus":5});
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "🦀 hello ");
        assert_eq!(rt.state.display().0, "🦀 hello", "preedit replaces the word, not appends");
        event["text"] = json!("help");
        event["anchor"] = json!(4); event["focus"] = json!(4);
        rt.web_edit(&event);
        assert_eq!(rt.state.display().0, "🦀 help");
        event["kind"] = json!("edit");
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "🦀 help");
        assert!(rt.state.preedit().is_none());
        assert_eq!(rt.state.focus, rt.state.text.len());
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "🦀 help", "stale context rejected");
        event["base"] = json!(1);
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "🦀 help", "base must be a UTF-8 boundary");
    }

    #[test]
    fn replacement_offsets_are_local_utf16_and_cannot_escape_the_snapshot() {
        fn view(edit: &crate::TextEdit, _: &mut Cx<crate::TextEdit>) -> El {
            crate::text_input(edit, crate::tok::type_::BODY).key("input").on_edit(|s: &mut crate::TextEdit| s)
        }
        let mut rt = Runtime::new(crate::TextEdit::new("é🦀 tail"), view, (300.0, 100.0), Env::default());
        rt.frame(0.0);
        let key = rt.nodes.iter().find(|n| n.key.as_deref() == Some("input")).unwrap().path.clone();
        rt.set_focus(Some(key.clone()), true);
        let mut event = json!({"kind":"edit", "key":key, "base":2, "expected":"🦀 ",
            "start":1, "end":2, "text":"x", "anchor":1, "focus":1});
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "é🦀 tail", "mid-surrogate offsets rejected");
        event["start"] = json!(0); event["end"] = json!(4);
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "é🦀 tail", "replacement must stay inside expected context");
        event["end"] = json!(2);
        rt.web_edit(&event);
        assert_eq!(rt.state.text, "éx tail");
        assert_eq!(rt.state.focus, 3);
    }

    #[test]
    fn web_key_forwards_dom_code_for_digits_letters_and_punctuation() {
        type Seen = Vec<(crate::Key, Option<crate::PhysicalKeyCode>, bool)>;
        fn view(_: &Seen, _: &mut Cx<Seen>) -> El { crate::el() }
        let mut rt = Runtime::new(Seen::new(), view, (100.0, 100.0), Env::default());
        rt.on_key = Some(|s, ev, tx| { s.push((ev.key.clone(), tx.physical_key(), tx.key_repeat())); true });
        rt.frame(0.0);
        for (key, code, repeat) in [("/", "Digit7", false), ("/", "Slash", true), (":", "Period", false), ("k", "KeyK", false)] {
            rt.web_key(&json!({"kind":"key", "key":key, "code":code, "meta":true, "shift":true, "repeat":repeat}));
            rt.frame(0.0);
        }
        use crate::PhysicalKeyCode::*;
        assert_eq!(rt.state, [
            (crate::Key::Command('/'), Some(Digit7), false),
            (crate::Key::Command('/'), Some(Slash), true),
            (crate::Key::Command(':'), Some(Period), false),
            (crate::Key::Command('k'), Some(KeyK), false),
        ]);
    }
}

/// Reject offsets in the middle of a surrogate pair, rather than corrupting the Rust string.
fn byte_at(text: &str, utf16: usize) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == utf16 { return Some(byte); }
        units += ch.len_utf16();
    }
    (units == utf16).then_some(text.len())
}

impl<S: 'static> Runtime<S> {
    pub(crate) fn web_context(&self, mirror: &mut Mirror) -> bool {
        match self.focused_input().filter(|i| i.access.is_some()) {
            Some(input) => mirror.update(self.focus.path().unwrap_or_default(), Some(&input.edit)),
            None => mirror.update("", None),
        }
    }

    pub(crate) fn web_key(&mut self, event: &serde_json::Value) -> bool {
        let flag = |key: &str| event[key].as_bool().unwrap_or(false);
        let (shift, ctrl, alt, logo) = (flag("shift"), flag("ctrl"), flag("alt"), flag("meta"));
        let mods = crate::Mods { shift, alt, ctrl, logo, cmd: logo || ctrl };
        let key = match event["key"].as_str().unwrap_or_default() {
            "Enter" => crate::Key::Enter, "Tab" => crate::Key::Tab, "Escape" => crate::Key::Escape,
            "ArrowLeft" => crate::Key::Left, "ArrowRight" => crate::Key::Right,
            "ArrowUp" => crate::Key::Up, "ArrowDown" => crate::Key::Down,
            "Home" => crate::Key::Home, "End" => crate::Key::End,
            "PageUp" => crate::Key::PageUp, "PageDown" => crate::Key::PageDown,
            "Backspace" => crate::Key::Backspace, "Delete" => crate::Key::Delete,
            "ContextMenu" => crate::Key::ContextMenu,
            "Insert" => crate::Key::Insert, "PrintScreen" => crate::Key::PrintScreen,
            "ScrollLock" => crate::Key::ScrollLock, "Pause" => crate::Key::Pause,
            name if name.chars().count() == 1 && mods.cmd => crate::Key::Command(name.chars().next().unwrap().to_ascii_lowercase()),
            _ => return false,
        };
        let event_key = crate::KeyEvent { key, mods };
        let activate = event_key.key == crate::Key::Tab;
        let repeat = flag("repeat");
        let code = event["code"].as_str().filter(|c| !c.is_empty() && !c.contains('+'))
            .and_then(|c| c.parse::<crate::Shortcut>().ok())
            .map(|s| s.physical_key());
        if let Some(key) = code {
            self.physical_with_fallback(crate::PhysicalInput::Key { key, mods, pressed: true, repeat }, Some(event_key));
        } else if repeat {
            self.repeat_key(event_key);
        } else {
            self.key(event_key);
        }
        activate
    }

    pub(crate) fn web_secure(&mut self, key: &str, text: String) {
        let event = crate::KeyEvent { key: crate::Key::Text(text), mods: crate::Mods::default() };
        if self.focus.path() == Some(key) && self.has_secure_focus() {
            self.key(event);
        }
    }

    pub(crate) fn web_edit(&mut self, event: &serde_json::Value) {
        let Some(key) = event["key"].as_str() else { return };
        if self.focus.path() != Some(key) || self.has_secure_focus() { return; }
        let number = |key: &str| event[key].as_u64().and_then(|n| usize::try_from(n).ok());
        let Some(base) = number("base") else { return };
        let Some(expected) = event["expected"].as_str() else { return };
        let mut tx = self.transaction();
        let composing_before = self.composing();
        let changed = self.with_focused(|edit| {
            // Browser edits belong to the snapshot from which its context was mirrored.
            if !edit.text.get(base..).is_some_and(|s| s.starts_with(expected)) { return EditOutcome::default(); }
            if event["kind"] == "edit" || event["kind"] == "preedit" {
                let Some((start, end)) = number("start").zip(number("end")) else { return EditOutcome::default() };
                let Some((a, b)) = byte_at(expected, start).zip(byte_at(expected, end)) else { return EditOutcome::default() };
                if a > b { return EditOutcome::default(); }
                edit.ime(&Ime::Preedit(String::new(), None));
                let raw = (base + a, base + b);
                edit.anchor = raw.0; edit.focus = raw.1;
                edit.snap_to_pills();
                let snapped = (edit.anchor, edit.focus) != raw;
                let text = event["text"].as_str().unwrap_or_default();
                if event["kind"] == "preedit" {
                    edit.ime(&Ime::Preedit(text.to_owned(), Some((text.len(), text.len()))));
                    return EditOutcome::default();
                }
                edit.insert(text);
                if snapped { return EditOutcome::default(); }
            }
            if let Some((anchor, focus)) = number("anchor").zip(number("focus"))
                && let Some((a, f)) = byte_at(&edit.text[base..], anchor).zip(byte_at(&edit.text[base..], focus)) {
                edit.anchor = base + a; edit.focus = base + f;
                edit.snap_to_pills();
            }
            EditOutcome::default()
        }, &mut tx);
        if composing_before || event["kind"] == "preedit" { self.ime_eat_key_up = true; }
        if changed { self.dirty = true; }
        merge(&mut self.pending, tx);
    }
}
