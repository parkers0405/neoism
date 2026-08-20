// macOS-specific default key bindings.

#[cfg(test)]
use crate::bindings::action::Binding;
use crate::bindings::action::{
    Action, BindingKey, BindingMode, KeyBinding, SearchAction,
};
use crate::bindings::macros::{bindings, trigger};
use neoism_backend::config::keyboard::Keyboard as ConfigKeyboard;
use neoism_window::keyboard::Key::*;
use neoism_window::keyboard::NamedKey::*;
use neoism_window::keyboard::{Key, KeyLocation, ModifiersState};

// Macos
#[cfg(all(target_os = "macos", not(test)))]
pub fn platform_key_bindings(
    use_navigation_key_bindings: bool,
    use_splits: bool,
    config_keyboard: ConfigKeyboard,
) -> Vec<KeyBinding> {
    let mut key_bindings = bindings!(
        KeyBinding;
        "0", ModifiersState::SUPER; Action::ResetFontSize;
        "=", ModifiersState::SUPER; Action::IncreaseFontSize;
        "+", ModifiersState::SUPER; Action::IncreaseFontSize;
        "+", ModifiersState::SUPER; Action::IncreaseFontSize;
        "-", ModifiersState::SUPER; Action::DecreaseFontSize;
        "-", ModifiersState::SUPER; Action::DecreaseFontSize;
        Key::Named(Insert), ModifiersState::SHIFT, ~BindingMode::VI, ~BindingMode::SEARCH;
            Action::Esc("\x1b[2;2~".into());
        "k", ModifiersState::SUPER, ~BindingMode::VI, ~BindingMode::SEARCH;
            Action::Esc("\x0c".into());
        "k", ModifiersState::SUPER, ~BindingMode::VI;  Action::ClearHistory;
        "v", ModifiersState::SUPER, ~BindingMode::VI; Action::Paste;
        "f", ModifiersState::CONTROL | ModifiersState::SUPER; Action::ToggleFullscreen;
        "c", ModifiersState::SUPER; Action::Copy;
        "c", ModifiersState::SUPER, +BindingMode::VI; Action::ClearSelection;
        "h", ModifiersState::SUPER; Action::Hide;
        "h", ModifiersState::SUPER | ModifiersState::ALT; Action::HideOtherApplications;
        "m", ModifiersState::SUPER; Action::Minimize;
        "q", ModifiersState::SUPER; Action::Quit;
        "n", ModifiersState::SUPER; Action::WindowCreateNew;
        ",", ModifiersState::ALT; Action::ConfigEditor;
        "p", ModifiersState::ALT; Action::OpenCommandPalette;
        "p", ModifiersState::SUPER; Action::OpenCommandPalette;
        "p", ModifiersState::SUPER | ModifiersState::SHIFT; Action::OpenCommandPalette;

        // Search
        "s", ModifiersState::SUPER, ~BindingMode::SEARCH; Action::SearchForward;
        "f", ModifiersState::SUPER, ~BindingMode::SEARCH; Action::SearchForward;
        "b", ModifiersState::SUPER, ~BindingMode::SEARCH; Action::SearchBackward;
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
            "t", ModifiersState::SUPER; Action::TabCreateNew;
            Key::Named(Tab), ModifiersState::CONTROL; Action::SelectNextTab;
            Key::Named(Tab), ModifiersState::CONTROL | ModifiersState::SHIFT; Action::SelectPrevTab;
            "w", ModifiersState::SUPER; Action::CloseCurrentSplitOrTab;
            "[", ModifiersState::SUPER | ModifiersState::SHIFT; Action::SelectPrevTab;
            "]", ModifiersState::SUPER | ModifiersState::SHIFT; Action::SelectNextTab;
            "1", ModifiersState::SUPER; Action::SelectTab(0);
            "2", ModifiersState::SUPER; Action::SelectTab(1);
            "3", ModifiersState::SUPER; Action::SelectTab(2);
            "4", ModifiersState::SUPER; Action::SelectTab(3);
            "5", ModifiersState::SUPER; Action::SelectTab(4);
            "6", ModifiersState::SUPER; Action::SelectTab(5);
            "7", ModifiersState::SUPER; Action::SelectTab(6);
            "8", ModifiersState::SUPER; Action::SelectTab(7);
            "9", ModifiersState::SUPER; Action::SelectLastTab;
        ));
    }

    if config_keyboard.disable_ctlseqs_alt {
        key_bindings.extend(bindings!(
            KeyBinding;
            Key::Named(ArrowLeft), ModifiersState::ALT,  ~BindingMode::VI;
                Action::Esc("\x1bb".into());
            Key::Named(ArrowRight), ModifiersState::ALT,  ~BindingMode::VI;
                Action::Esc("\x1bf".into());
        ));
    }

    if use_splits {
        key_bindings.extend(bindings!(
            KeyBinding;
            "d", ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SplitRight;
            "d", ModifiersState::SUPER | ModifiersState::SHIFT, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SplitDown;
            "]", ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SelectNextSplit;
            "[", ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::SelectPrevSplit;
            Key::Named(ArrowUp), ModifiersState::CONTROL | ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerUp;
            Key::Named(ArrowDown), ModifiersState::CONTROL | ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerDown;
            Key::Named(ArrowLeft), ModifiersState::CONTROL | ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerLeft;
            Key::Named(ArrowRight), ModifiersState::CONTROL | ModifiersState::SUPER, ~BindingMode::SEARCH, ~BindingMode::VI; Action::MoveDividerRight;
        ));
    }

    key_bindings
}
