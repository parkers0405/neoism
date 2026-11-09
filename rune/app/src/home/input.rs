//! Rune key events as the terminal encoder's keys (GPUiX names: "enter", "up", "a", "f5", ...).

use crate::ghostty::KeyInput;
use rune::{Key, KeyEvent};

/// A key and the text it types, ready for `Shell::encode_key`.
pub struct TermKey {
    name: String,
    text: String,
    shift: bool,
    ctrl: bool,
    alt: bool,
    logo: bool,
}

impl TermKey {
    pub fn from_event(event: &KeyEvent) -> Option<TermKey> {
        let m = event.mods;
        let (name, text) = match &event.key {
            Key::Text(text) if text == " " => ("space".to_owned(), " ".to_owned()),
            Key::Text(text) => {
                let mut chars = text.chars();
                let name = match (chars.next(), chars.next()) {
                    (Some(c), None) => c.to_lowercase().collect(),
                    _ => String::new(),
                };
                (name, text.clone())
            }
            // Ctrl+letter off macOS (and Cmd+letter on it): the key, no text.
            Key::Command(c) => (c.to_ascii_lowercase().to_string(), String::new()),
            Key::Enter => ("enter".into(), String::new()),
            Key::Backspace => ("backspace".into(), String::new()),
            Key::Delete => ("delete".into(), String::new()),
            Key::Left => ("left".into(), String::new()),
            Key::Right => ("right".into(), String::new()),
            Key::Up => ("up".into(), String::new()),
            Key::Down => ("down".into(), String::new()),
            Key::Home => ("home".into(), String::new()),
            Key::End => ("end".into(), String::new()),
            Key::PageUp => ("pageup".into(), String::new()),
            Key::PageDown => ("pagedown".into(), String::new()),
            Key::Insert => ("insert".into(), String::new()),
            Key::Escape => ("escape".into(), String::new()),
            Key::Tab => ("tab".into(), String::new()),
            Key::Function(n) => (format!("f{n}"), String::new()),
            _ => return None,
        };
        Some(TermKey { name, text, shift: m.shift, ctrl: m.ctrl, alt: m.alt, logo: m.logo })
    }

    pub fn input(&self) -> KeyInput<'_> {
        KeyInput { key: &self.name, text: &self.text, shift: self.shift, ctrl: self.ctrl, alt: self.alt, logo: self.logo }
    }

    pub fn is(&self, name: &str) -> bool { self.name == name }
    pub fn shift(&self) -> bool { self.shift }
    pub fn ctrl(&self) -> bool { self.ctrl }
    pub fn alt(&self) -> bool { self.alt }
    pub fn logo(&self) -> bool { self.logo }
    pub fn ctrl_only(&self) -> bool { self.ctrl && !self.alt && !self.shift && !self.logo }
}
