// Linux (and other non-macOS, non-Windows unix) default key bindings.

use crate::bindings::action::KeyBinding;
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
use crate::bindings::action::{Action, BindingKey, BindingMode, SearchAction};
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
use crate::bindings::macros::{bindings, trigger};
use neoism_backend::config::keyboard::Keyboard as ConfigKeyboard;
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
use neoism_window::keyboard::Key::*;
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
use neoism_window::keyboard::NamedKey::*;
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
use neoism_window::keyboard::{Key, KeyLocation, ModifiersState};

// Not Windows, Macos
#[cfg(not(any(target_os = "macos", target_os = "windows", test)))]
pub fn platform_key_bindings(
    use_navigation_key_bindings: bool,
    use_splits: bool,
    _: ConfigKeyboard,
) -> Vec<KeyBinding> {
    let mut key_bindings = bindings!(
        KeyBinding;
        "v", ModifiersState::CONTROL | ModifiersState::SHIFT, ~BindingMode::VI; Action::Paste;
        "c", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::Copy;
        "c", ModifiersState::CONTROL | ModifiersState::SHIFT,
            +BindingMode::VI; Action::ClearSelection;
        "c", ModifiersState::SUPER; Action::Copy;
        "c", ModifiersState::SUPER, +BindingMode::VI; Action::ClearSelection;
        Key::Named(Insert),   ModifiersState::SHIFT, ~BindingMode::VI; Action::Paste;
        "0", ModifiersState::CONTROL;  Action::ResetFontSize;
        "=", ModifiersState::CONTROL;  Action::IncreaseFontSize;
        "+", ModifiersState::CONTROL;  Action::IncreaseFontSize;
        "+", ModifiersState::CONTROL;  Action::IncreaseFontSize;
        "-", ModifiersState::CONTROL;  Action::DecreaseFontSize;
        "-", ModifiersState::CONTROL;  Action::DecreaseFontSize;
        "n", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::WindowCreateNew;
        ",", ModifiersState::ALT; Action::ConfigEditor;
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
            "[", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectPrevBufferTab;
            "]", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectNextBufferTab;
            "w", ModifiersState::CONTROL | ModifiersState::SHIFT; Action::TabCreateNew;
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
            // Ctrl+Alt+arrows: resize the divider from the focused pane
            // (grow/shrink whichever side has focus).
            Key::Named(ArrowUp), ModifiersState::CONTROL | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerUp;
            Key::Named(ArrowDown), ModifiersState::CONTROL | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerDown;
            Key::Named(ArrowLeft), ModifiersState::CONTROL | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerLeft;
            Key::Named(ArrowRight), ModifiersState::CONTROL | ModifiersState::ALT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerRight;
        ));
    }

    key_bindings
}

#[cfg(test)]
pub fn platform_key_bindings(_: bool, _: bool, _: ConfigKeyboard) -> Vec<KeyBinding> {
    vec![]
}
