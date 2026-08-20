// Windows default key bindings.

use crate::bindings::action::{
    Action, Binding, BindingKey, BindingMode, KeyBinding, SearchAction,
};
use crate::bindings::macros::{bindings, trigger};
use neoism_backend::config::keyboard::Keyboard as ConfigKeyboard;
use neoism_window::keyboard::Key::*;
use neoism_window::keyboard::NamedKey::*;
use neoism_window::keyboard::{Key, KeyLocation, ModifiersState};

// Windows
#[cfg(all(target_os = "windows", not(test)))]
pub fn platform_key_bindings(
    use_navigation_key_bindings: bool,
    use_splits: bool,
    _: ConfigKeyboard,
) -> Vec<KeyBinding> {
    let mut key_bindings = bindings!(
        KeyBinding;
        "v", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::VI; Action::Paste;
        "c", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::Copy;
        "c", ModifiersState::CONTROL | ModifiersState::SHIFT, +BindingMode::VI; Action::ClearSelection;
        Key::Named(Insert), ModifiersState::SHIFT, ~BindingMode::VI; Action::PasteSelection;
        "0", ModifiersState::CONTROL; Action::ResetFontSize;
        "=", ModifiersState::CONTROL; Action::IncreaseFontSize;
        "+", ModifiersState::CONTROL; Action::IncreaseFontSize;
        "+", ModifiersState::CONTROL; Action::IncreaseFontSize;
        "-", ModifiersState::CONTROL; Action::DecreaseFontSize;
        "-", ModifiersState::CONTROL; Action::DecreaseFontSize;
        Key::Named(Enter), ModifiersState::ALT; Action::ToggleFullscreen;
        "n", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::WindowCreateNew;
        ",", ModifiersState::ALT; Action::ConfigEditor;
        // This is actually a Windows Powershell shortcut
        // https://github.com/alacritty/alacritty/issues/2930
        // https://github.com/raphamorim/rio/issues/220#issuecomment-1761651339
        Key::Named(Backspace), ModifiersState::CONTROL, ~BindingMode::VI; Action::Esc("\u{0017}".into());
        Key::Named(Space), ModifiersState::CONTROL | ModifiersState::SHIFT; Action::ToggleViMode;
        "p", ModifiersState::ALT; Action::OpenCommandPalette;
        "p", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::OpenCommandPalette;

        // Search
        "f", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH; Action::SearchForward;
        "b", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH; Action::SearchBackward;
        "c", ModifiersState::CONTROL, +BindingMode::SEARCH; SearchAction::SearchCancel;
        "u", ModifiersState::CONTROL, +BindingMode::SEARCH; SearchAction::SearchClear;
        "w", ModifiersState::CONTROL,  +BindingMode::SEARCH; SearchAction::SearchDeleteWord;
        "p", ModifiersState::CONTROL,  +BindingMode::SEARCH; SearchAction::SearchHistoryPrevious;
        "n", ModifiersState::CONTROL,  +BindingMode::SEARCH; SearchAction::SearchHistoryNext;
        Key::Named(ArrowUp), +BindingMode::SEARCH; SearchAction::SearchHistoryPrevious;
        Key::Named(ArrowDown), +BindingMode::SEARCH; SearchAction::SearchHistoryNext;
    );

    if use_navigation_key_bindings {
        key_bindings.extend(bindings!(
            KeyBinding;
            "t", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::WorkspaceTerminalTabCreateNew;
            Key::Named(Tab), ModifiersState::CONTROL; Action::SelectNextTab;
            Key::Named(Tab), ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectPrevTab;
            Key::Named(ArrowLeft), ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectPrevBufferTab;
            Key::Named(ArrowRight), ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectNextBufferTab;
            Key::Named(ArrowLeft), ModifiersState::ALT | ModifiersState::SHIFT; Action::MoveActiveBufferTabToPrev;
            Key::Named(ArrowRight), ModifiersState::ALT | ModifiersState::SHIFT; Action::MoveActiveBufferTabToNext;
            "w", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::TabCreateNew;
            "[", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectPrevBufferTab;
            "]", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectNextBufferTab;
        ));
    }

    if use_splits {
        key_bindings.extend(bindings!(
            KeyBinding;
            "r", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SplitRight;
            "d", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SplitDown;
            "]", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SelectNextSplit;
            "[", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SelectPrevSplit;
            Key::Named(ArrowUp), ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerUp;
            Key::Named(ArrowDown), ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerDown;
            Key::Named(ArrowLeft), ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerLeft;
            Key::Named(ArrowRight), ModifiersState::CONTROL | ModifiersState::SHIFT | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerRight;
        ));
    }

    // Note: Hint bindings are added separately in Screen::new() based on config

    key_bindings
}
