//! Translation at the application/framework boundary. NeoUI never imports the
//! application's wire protocol or native window types.

use neoui::input::{InputEvent, Key, Modifiers};
use crate::event::{CompositionEvent, KeyState, LogicalKey, NamedKey, UiEvent};

pub fn key_from_name(name: &str) -> Option<Key> {
    Some(match name {
        "Backspace" => Key::Backspace, "Delete" => Key::Delete,
        "Enter" | "Return" => Key::Enter, "Escape" => Key::Escape,
        "Tab" => Key::Tab, "Space" | " " => Key::Space,
        "ArrowLeft" => Key::Left, "ArrowRight" => Key::Right,
        "ArrowUp" => Key::Up, "ArrowDown" => Key::Down,
        "Home" => Key::Home, "End" => Key::End,
        "PageUp" => Key::PageUp, "PageDown" => Key::PageDown,
        _ => return None,
    })
}

pub fn from_ui_event(event: &UiEvent) -> Option<InputEvent> {
    Some(match event {
        UiEvent::Key(event) if event.state == KeyState::Pressed => {
            let key = match &event.logical {
                LogicalKey::Character(text) => Key::Character(text.to_string()),
                LogicalKey::Named(named) => match named {
                    NamedKey::Backspace => Key::Backspace, NamedKey::Delete => Key::Delete,
                    NamedKey::Enter => Key::Enter, NamedKey::Escape => Key::Escape,
                    NamedKey::Tab => Key::Tab, NamedKey::Space => Key::Space,
                    NamedKey::ArrowLeft => Key::Left, NamedKey::ArrowRight => Key::Right,
                    NamedKey::ArrowUp => Key::Up, NamedKey::ArrowDown => Key::Down,
                    NamedKey::Home => Key::Home, NamedKey::End => Key::End,
                    NamedKey::PageUp => Key::PageUp, NamedKey::PageDown => Key::PageDown,
                    _ => return None,
                },
                _ => return None,
            };
            InputEvent::Key { key, repeat: event.repeat, modifiers: Modifiers {
                shift: event.modifiers.contains(crate::event::Modifiers::SHIFT),
                control: event.modifiers.contains(crate::event::Modifiers::CTRL),
                alt: event.modifiers.contains(crate::event::Modifiers::ALT),
                meta: event.modifiers.contains(crate::event::Modifiers::META),
            } }
        }
        UiEvent::Text(text) => InputEvent::Text(text.clone()),
        UiEvent::Composition(CompositionEvent::Update { preedit, cursor }) =>
            InputEvent::ImePreedit { text: preedit.clone(), selection: Some(*cursor..*cursor) },
        UiEvent::Composition(CompositionEvent::Commit(text)) => InputEvent::ImeCommit(text.clone()),
        UiEvent::Composition(CompositionEvent::End) => InputEvent::ImeCancel,
        UiEvent::Focus(focused) => InputEvent::Focus(*focused),
        _ => return None,
    })
}
