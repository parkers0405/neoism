// Default key/mouse bindings, the user-config converter, and the hint
// binding builder. Pulled out of the historical `bindings/mod.rs` so the
// long match-tables live separately from the type definitions.

use super::action::{
    Action, BindingKey, BindingMode, KeyBinding, ModeWrapper, MouseAction, MouseBinding,
    SearchAction, ViAction,
};
use super::macros::{bindings, trigger};
use super::platform::platform_key_bindings;
use neoism_backend::config::bindings::KeyBinding as ConfigKeyBinding;
use neoism_terminal_core::crosswords::vi_mode::ViMotion;
use neoism_window::event::MouseButton;
use neoism_window::keyboard::Key::*;
use neoism_window::keyboard::NamedKey::*;
use neoism_window::keyboard::{Key, KeyLocation, ModifiersState};

pub fn default_mouse_bindings() -> Vec<MouseBinding> {
    bindings!(
        MouseBinding;
        MouseButton::Right;                            MouseAction::ExpandSelection;
        MouseButton::Right,   ModifiersState::CONTROL; MouseAction::ExpandSelection;
        MouseButton::Middle, ~BindingMode::VI;         Action::PasteSelection;
    )
}

pub fn default_key_bindings(config: &neoism_backend::config::Config) -> Vec<KeyBinding> {
    let mut bindings = bindings!(
        KeyBinding;
        Key::Named(Copy);  Action::Copy;
        Key::Named(Copy),  +BindingMode::VI; Action::ClearSelection;
        Key::Named(Paste), ~BindingMode::VI; Action::Paste;
        Key::Character("l".into()), ModifiersState::CONTROL; Action::ClearLogNotice;
        "l",  ModifiersState::CONTROL, ~BindingMode::VI; Action::Esc("\x0c".into());
        Key::Named(Home),     ModifiersState::SHIFT, ~BindingMode::ALT_SCREEN; Action::ScrollToTop;
        Key::Named(End),      ModifiersState::SHIFT, ~BindingMode::ALT_SCREEN; Action::ScrollToBottom;
        Key::Named(PageUp),   ModifiersState::SHIFT, ~BindingMode::ALT_SCREEN; Action::ScrollPageUp;
        Key::Named(PageDown), ModifiersState::SHIFT, ~BindingMode::ALT_SCREEN; Action::ScrollPageDown;
        Key::Named(Home),  +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOH".into());
        Key::Named(End),   +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOF".into());
        Key::Named(ArrowUp),    +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOA".into());
        Key::Named(ArrowDown),  +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOB".into());
        Key::Named(ArrowRight), +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOC".into());
        Key::Named(ArrowLeft),  +BindingMode::APP_CURSOR, ~BindingMode::VI;
            Action::Esc("\x1bOD".into());

        // IDE chrome — Alt+E stands in for the planned `<space>e` leader
        // sequence. Toggles file-tree visibility + focus bidirectionally.
        "e", ModifiersState::ALT, ~BindingMode::VI; Action::ToggleFileTree;
        "g", ModifiersState::ALT, ~BindingMode::VI; Action::ToggleGitDiffPanel;
        "n", ModifiersState::ALT, ~BindingMode::VI; Action::OpenNeoismNotes;

        // VI Mode
        Key::Named(Space), ModifiersState::ALT | ModifiersState::SHIFT; Action::ToggleViMode;
        "/", +BindingMode::VI, ~BindingMode::SEARCH; Action::SearchForward;
        "n", +BindingMode::VI, ~BindingMode::SEARCH; SearchAction::SearchFocusNext;
        "n",  ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH; SearchAction::SearchFocusPrevious;
        Key::Named(Enter), +BindingMode::SEARCH, ~BindingMode::VI; SearchAction::SearchFocusNext;
        Key::Named(Enter), +BindingMode::SEARCH, +BindingMode::VI; SearchAction::SearchConfirm;
        Key::Named(Escape), +BindingMode::SEARCH; SearchAction::SearchCancel;
        Key::Named(Enter), ModifiersState::SHIFT, +BindingMode::SEARCH, ~BindingMode::VI; SearchAction::SearchFocusPrevious;
        "i", +BindingMode::VI, ~BindingMode::SEARCH; Action::ToggleViMode;
        "c", ModifiersState::CONTROL, +BindingMode::VI; Action::ToggleViMode;
        Key::Named(Escape), +BindingMode::VI; Action::ClearSelection;
        "i", +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollToBottom;
        "g", +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollToTop;
        "g", ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollToBottom;
        "b", ModifiersState::CONTROL, +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollPageUp;
        "f", ModifiersState::CONTROL, +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollPageDown;
        "u", ModifiersState::CONTROL, +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollHalfPageUp;
        "d", ModifiersState::CONTROL, +BindingMode::VI, ~BindingMode::SEARCH; Action::ScrollHalfPageDown;
        "y", ModifiersState::CONTROL,  +BindingMode::VI, ~BindingMode::SEARCH; Action::Scroll(1);
        "e", ModifiersState::CONTROL,  +BindingMode::VI, ~BindingMode::SEARCH; Action::Scroll(-1);
        "y", +BindingMode::VI, ~BindingMode::SEARCH; Action::Copy;
        "y", +BindingMode::VI, ~BindingMode::SEARCH; Action::ClearSelection;
        "v", +BindingMode::VI, ~BindingMode::SEARCH; ViAction::ToggleNormalSelection;
        "v", ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH; ViAction::ToggleLineSelection;
        "v", ModifiersState::CONTROL, +BindingMode::VI, ~BindingMode::SEARCH; ViAction::ToggleBlockSelection;
        "v", ModifiersState::ALT, +BindingMode::VI, ~BindingMode::SEARCH; ViAction::ToggleSemanticSelection;
        "z", +BindingMode::VI, ~BindingMode::SEARCH; ViAction::CenterAroundViCursor;
        "k", +BindingMode::VI, ~BindingMode::SEARCH; ViMotion::Up;
        "j", +BindingMode::VI, ~BindingMode::SEARCH; ViMotion::Down;
        "h", +BindingMode::VI, ~BindingMode::SEARCH; ViMotion::Left;
        "l", +BindingMode::VI, ~BindingMode::SEARCH; ViMotion::Right;
        Key::Named(ArrowUp), +BindingMode::VI; ViMotion::Up;
        Key::Named(ArrowDown), +BindingMode::VI; ViMotion::Down;
        Key::Named(ArrowLeft), +BindingMode::VI; ViMotion::Left;
        Key::Named(ArrowRight), +BindingMode::VI; ViMotion::Right;
        Key::Named(ArrowUp), ModifiersState::SUPER, ~BindingMode::VI; Action::None;
        Key::Named(ArrowDown), ModifiersState::SUPER, ~BindingMode::VI; Action::None;
        Key::Named(ArrowLeft), ModifiersState::SUPER, ~BindingMode::VI; Action::None;
        Key::Named(ArrowRight), ModifiersState::SUPER, ~BindingMode::VI; Action::None;
        "0",                          +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::First;
        "4",   ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::Last;
        "6",   ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::FirstOccupied;
        "h",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::High;
        "m",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::Middle;
        "l",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::Low;
        "b",                             +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::SemanticLeft;
        "w",                             +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::SemanticRight;
        "e",                             +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::SemanticRightEnd;
        "b",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::WordLeft;
        "w",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::WordRight;
        "e",      ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::WordRightEnd;
        "5",   ModifiersState::SHIFT, +BindingMode::VI, ~BindingMode::SEARCH;
            ViMotion::Bracket;
    );

    bindings.extend(bindings!(
        KeyBinding;
        Key::Named(ArrowUp), ~BindingMode::APP_CURSOR, ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC; Action::Esc("\x1b[A".into());
        Key::Named(ArrowDown), ~BindingMode::APP_CURSOR, ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC; Action::Esc("\x1b[B".into());
        Key::Named(ArrowRight), ~BindingMode::APP_CURSOR, ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC; Action::Esc("\x1b[C".into());
        Key::Named(ArrowLeft),  ~BindingMode::APP_CURSOR, ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC; Action::Esc("\x1b[D".into());
        Key::Named(Insert),     ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b[2~".into());
        Key::Named(Delete),     ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b[3~".into());
        Key::Named(PageUp),     ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b[5~".into());
        Key::Named(PageDown),   ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b[6~".into());
        Key::Named(Backspace),  ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC; Action::Esc("\x7f".into());
        Key::Named(Backspace), ModifiersState::ALT,     ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b\x7f".into());
        Key::Named(Backspace), ModifiersState::SHIFT,   ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x7f".into());
        Key::Named(F1), ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1bOP".into());
        Key::Named(F2), ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1bOQ".into());
        Key::Named(F3), ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1bOR".into());
        Key::Named(F4), ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1bOS".into());
        Key::Named(Tab),       ModifiersState::SHIFT,   ~BindingMode::VI,   ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b[Z".into());
        Key::Named(Tab),       ModifiersState::SHIFT | ModifiersState::ALT, ~BindingMode::VI, ~BindingMode::SEARCH, ~BindingMode::ALL_KEYS_AS_ESC, ~BindingMode::DISAMBIGUATE_KEYS; Action::Esc("\x1b\x1b[Z".into());
    ));

    bindings.extend(platform_key_bindings(
        config.ui.navigation.has_navigation_key_bindings(),
        config.ui.navigation.use_split,
        config.terminal.keyboard,
    ));

    // Add hint bindings
    bindings.extend(create_hint_bindings(&config.terminal.hints.rules));

    config_key_bindings(config.keybinds.keys.to_owned(), bindings)
}

#[inline]
fn convert(config_key_binding: ConfigKeyBinding) -> Result<KeyBinding, String> {
    let (key, location) = if config_key_binding.key.chars().count() == 1 {
        (
            Key::Character(config_key_binding.key.to_lowercase().into()),
            KeyLocation::Standard,
        )
    } else {
        match config_key_binding.key.to_lowercase().as_str() {
            "home" => (Key::Named(Home), KeyLocation::Standard),
            "space" => (Key::Named(Space), KeyLocation::Standard),
            "delete" => (Key::Named(Delete), KeyLocation::Standard),
            "esc" => (Key::Named(Escape), KeyLocation::Standard),
            "insert" => (Key::Named(Insert), KeyLocation::Standard),
            "pageup" => (Key::Named(PageUp), KeyLocation::Standard),
            "pagedown" => (Key::Named(PageDown), KeyLocation::Standard),
            "end" => (Key::Named(End), KeyLocation::Standard),
            "up" => (Key::Named(ArrowUp), KeyLocation::Standard),
            "back" => (Key::Named(Backspace), KeyLocation::Standard),
            "down" => (Key::Named(ArrowDown), KeyLocation::Standard),
            "left" => (Key::Named(ArrowLeft), KeyLocation::Standard),
            "right" => (Key::Named(ArrowRight), KeyLocation::Standard),
            "@" => (Key::Character("@".into()), KeyLocation::Standard),
            "colon" => (Key::Character(":".into()), KeyLocation::Standard),
            "." => (Key::Character(".".into()), KeyLocation::Standard),
            "return" => (Key::Named(Enter), KeyLocation::Standard),
            "[" => (Key::Character("[".into()), KeyLocation::Standard),
            "]" => (Key::Character("]".into()), KeyLocation::Standard),
            "{" => (Key::Character("{".into()), KeyLocation::Standard),
            "}" => (Key::Character("}".into()), KeyLocation::Standard),
            ";" => (Key::Character(";".into()), KeyLocation::Standard),
            "\\" => (Key::Character("\\".into()), KeyLocation::Standard),
            "+" => (Key::Character("+".into()), KeyLocation::Standard),
            "," => (Key::Character(",".into()), KeyLocation::Standard),
            "/" => (Key::Character("/".into()), KeyLocation::Standard),
            "=" => (Key::Character("=".into()), KeyLocation::Standard),
            "-" => (Key::Character("-".into()), KeyLocation::Standard),
            "*" => (Key::Character("*".into()), KeyLocation::Standard),
            "1" => (Key::Character("1".into()), KeyLocation::Standard),
            "2" => (Key::Character("2".into()), KeyLocation::Standard),
            "3" => (Key::Character("3".into()), KeyLocation::Standard),
            "4" => (Key::Character("4".into()), KeyLocation::Standard),
            "5" => (Key::Character("5".into()), KeyLocation::Standard),
            "6" => (Key::Character("6".into()), KeyLocation::Standard),
            "7" => (Key::Character("7".into()), KeyLocation::Standard),
            "8" => (Key::Character("8".into()), KeyLocation::Standard),
            "9" => (Key::Character("9".into()), KeyLocation::Standard),
            "0" => (Key::Character("0".into()), KeyLocation::Standard),

            // Special case numpad.
            "numpadenter" => (Key::Named(Enter), KeyLocation::Numpad),
            "numpadadd" => (Key::Character("+".into()), KeyLocation::Numpad),
            "numpadcomma" => (Key::Character(",".into()), KeyLocation::Numpad),
            "numpaddecimal" => (Key::Character(".".into()), KeyLocation::Numpad),
            "numpaddivide" => (Key::Character("/".into()), KeyLocation::Numpad),
            "numpadequals" => (Key::Character("=".into()), KeyLocation::Numpad),
            "numpadsubtract" => (Key::Character("-".into()), KeyLocation::Numpad),
            "numpadmultiply" => (Key::Character("*".into()), KeyLocation::Numpad),
            "numpad1" => (Key::Character("1".into()), KeyLocation::Numpad),
            "numpad2" => (Key::Character("2".into()), KeyLocation::Numpad),
            "numpad3" => (Key::Character("3".into()), KeyLocation::Numpad),
            "numpad4" => (Key::Character("4".into()), KeyLocation::Numpad),
            "numpad5" => (Key::Character("5".into()), KeyLocation::Numpad),
            "numpad6" => (Key::Character("6".into()), KeyLocation::Numpad),
            "numpad7" => (Key::Character("7".into()), KeyLocation::Numpad),
            "numpad8" => (Key::Character("8".into()), KeyLocation::Numpad),
            "numpad9" => (Key::Character("9".into()), KeyLocation::Numpad),
            "numpad0" => (Key::Character("0".into()), KeyLocation::Numpad),

            // Special cases
            "tab" => (Key::Named(Tab), KeyLocation::Standard),
            _ => return Err("Unable to find defined 'keycode'".to_string()),
        }
    };

    let trigger = BindingKey::Keycode { key, location };

    let mut res = ModifiersState::empty();
    for modifier in config_key_binding.with.split('|') {
        match modifier.trim().to_lowercase().as_str() {
            "command" | "super" => res.insert(ModifiersState::SUPER),
            "shift" => res.insert(ModifiersState::SHIFT),
            "alt" | "option" => res.insert(ModifiersState::ALT),
            "control" => res.insert(ModifiersState::CONTROL),
            "none" => (),
            _ => (),
        }
    }

    let mut action: Action = config_key_binding.action.into();
    if !config_key_binding.esc.is_empty() {
        action = Action::Esc(config_key_binding.esc);
    }

    let mut res_mode = ModeWrapper {
        mode: BindingMode::empty(),
        not_mode: BindingMode::empty(),
    };

    for modifier in config_key_binding.mode.split('|') {
        match modifier.trim().to_lowercase().as_str() {
            "appcursor" => res_mode.mode |= BindingMode::APP_CURSOR,
            "~appcursor" => res_mode.not_mode |= BindingMode::APP_CURSOR,
            "appkeypad" => res_mode.mode |= BindingMode::APP_KEYPAD,
            "~appkeypad" => res_mode.not_mode |= BindingMode::APP_KEYPAD,
            "alt" => res_mode.mode |= BindingMode::ALT_SCREEN,
            "~alt" => res_mode.not_mode |= BindingMode::ALT_SCREEN,
            "vi" => res_mode.mode |= BindingMode::VI,
            "~vi" => res_mode.not_mode |= BindingMode::VI,
            _ => {
                res_mode.not_mode |= BindingMode::empty();
                res_mode.mode |= BindingMode::empty();
            }
        }
    }

    Ok(KeyBinding {
        trigger,
        mods: res,
        action,
        mode: res_mode.mode,
        notmode: res_mode.not_mode,
    })
}

pub fn config_key_bindings(
    config_key_bindings: Vec<ConfigKeyBinding>,
    mut bindings: Vec<KeyBinding>,
) -> Vec<KeyBinding> {
    if config_key_bindings.is_empty() {
        return bindings;
    }

    for ckb in config_key_bindings {
        match convert(ckb) {
            Ok(key_binding) => {
                // Remove any default binding that would conflict with this user binding
                // This ensures user bindings always take precedence and prevents conflicts
                bindings.retain(|b| !b.triggers_match(&key_binding));

                tracing::info!("added a new key_binding: {:?}", key_binding);
                bindings.push(key_binding)
            }
            Err(err_message) => {
                tracing::error!("error loading a key binding: {:?}", err_message);
            }
        }
    }

    bindings
}

/// Create hint bindings from configuration
pub fn create_hint_bindings(
    hints_config: &[neoism_backend::config::hints::Hint],
) -> Vec<KeyBinding> {
    let mut hint_bindings = Vec::new();

    for hint_config in hints_config {
        if let Some(binding_config) = &hint_config.binding {
            // Parse key using the same logic as in convert()
            let (key, location) = match binding_config.key.to_lowercase().as_str() {
                // Letters
                single_char if single_char.len() == 1 => {
                    (Key::Character(single_char.into()), KeyLocation::Standard)
                }
                // Named keys
                "space" => (Key::Named(Space), KeyLocation::Standard),
                "enter" | "return" => (Key::Named(Enter), KeyLocation::Standard),
                "escape" | "esc" => (Key::Named(Escape), KeyLocation::Standard),
                "tab" => (Key::Named(Tab), KeyLocation::Standard),
                "backspace" => (Key::Named(Backspace), KeyLocation::Standard),
                "delete" => (Key::Named(Delete), KeyLocation::Standard),
                "insert" => (Key::Named(Insert), KeyLocation::Standard),
                "home" => (Key::Named(Home), KeyLocation::Standard),
                "end" => (Key::Named(End), KeyLocation::Standard),
                "pageup" => (Key::Named(PageUp), KeyLocation::Standard),
                "pagedown" => (Key::Named(PageDown), KeyLocation::Standard),
                "up" => (Key::Named(ArrowUp), KeyLocation::Standard),
                "down" => (Key::Named(ArrowDown), KeyLocation::Standard),
                "left" => (Key::Named(ArrowLeft), KeyLocation::Standard),
                "right" => (Key::Named(ArrowRight), KeyLocation::Standard),
                // Function keys
                "f1" => (Key::Named(F1), KeyLocation::Standard),
                "f2" => (Key::Named(F2), KeyLocation::Standard),
                "f3" => (Key::Named(F3), KeyLocation::Standard),
                "f4" => (Key::Named(F4), KeyLocation::Standard),
                "f5" => (Key::Named(F5), KeyLocation::Standard),
                "f6" => (Key::Named(F6), KeyLocation::Standard),
                "f7" => (Key::Named(F7), KeyLocation::Standard),
                "f8" => (Key::Named(F8), KeyLocation::Standard),
                "f9" => (Key::Named(F9), KeyLocation::Standard),
                "f10" => (Key::Named(F10), KeyLocation::Standard),
                "f11" => (Key::Named(F11), KeyLocation::Standard),
                "f12" => (Key::Named(F12), KeyLocation::Standard),
                _ => {
                    tracing::warn!(
                        "Unknown key '{}' in hint binding",
                        binding_config.key
                    );
                    continue;
                }
            };

            // Parse modifiers
            let mut mods = ModifiersState::empty();
            for mod_str in &binding_config.mods {
                match mod_str.to_lowercase().as_str() {
                    "control" | "ctrl" => mods |= ModifiersState::CONTROL,
                    "shift" => mods |= ModifiersState::SHIFT,
                    "alt" | "option" => mods |= ModifiersState::ALT,
                    "super" | "cmd" | "command" => mods |= ModifiersState::SUPER,
                    _ => {
                        tracing::warn!("Unknown modifier '{}' in hint binding", mod_str);
                    }
                }
            }

            let hint_binding = KeyBinding {
                trigger: BindingKey::Keycode { key, location },
                mods,
                mode: BindingMode::empty(),
                notmode: BindingMode::SEARCH | BindingMode::VI,
                action: Action::Hint(std::rc::Rc::new(hint_config.clone())),
            };

            hint_bindings.push(hint_binding);
        }
    }

    hint_bindings
}
