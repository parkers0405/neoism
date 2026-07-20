use super::*;

impl Screen<'_> {
    pub(crate) fn bytes_hex_for_log(bytes: &[u8]) -> String {
        neoism_ui::lifecycle_policy::bytes_hex_for_log(bytes)
    }

    pub(crate) fn bytes_text_for_log(bytes: &[u8]) -> String {
        neoism_ui::lifecycle_policy::bytes_text_for_log(bytes)
    }

    pub(crate) fn should_build_sequence(
        key: &neoism_window::event::KeyEvent,
        text: &str,
        mode: Mode,
        mods: ModifiersState,
    ) -> bool {
        // Translate the winit `KeyEvent` into the shared POD input
        // and defer to `neoism_ui::selection_input::should_build_key_sequence`.
        // Keeps the kitty-vs-raw-UTF-8 fork identical between desktop
        // and web frontends.
        use neoism_ui::selection_input::{
            should_build_key_sequence, KeySequenceShapeInput, OutputLogicalKey,
        };
        let key_tag = match key.logical_key {
            Key::Named(NamedKey::Escape) => OutputLogicalKey::Escape,
            Key::Named(NamedKey::Tab) => OutputLogicalKey::Tab,
            Key::Named(NamedKey::Enter) => OutputLogicalKey::Enter,
            Key::Named(NamedKey::Backspace) => OutputLogicalKey::Backspace,
            Key::Named(named) => {
                if named.to_text().is_some() {
                    OutputLogicalKey::NamedWithText
                } else {
                    OutputLogicalKey::NamedWithoutText
                }
            }
            _ => OutputLogicalKey::NonNamed,
        };
        should_build_key_sequence(KeySequenceShapeInput {
            key: key_tag,
            key_on_numpad: key.location == KeyLocation::Numpad,
            text_empty: text.is_empty(),
            mods_empty: mods.is_empty(),
            mods_shift_only: mods == ModifiersState::SHIFT,
            report_all_keys_as_esc: mode.contains(Mode::REPORT_ALL_KEYS_AS_ESC),
            disambiguate_esc_codes: mode.contains(Mode::DISAMBIGUATE_ESC_CODES),
        })
    }

    pub(crate) fn is_workspace_buffer_tab_switch_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        let is_tab_key = matches!(key.logical_key, Key::Named(NamedKey::Tab))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Tab))
            || key.text.as_deref() == Some("\t");
        is_tab_key && mods.alt_key() && !mods.control_key() && !mods.super_key()
    }

    pub(crate) fn is_top_level_workspace_tab_switch_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        let is_tab_key = matches!(key.logical_key, Key::Named(NamedKey::Tab))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Tab))
            || key.text.as_deref() == Some("\t");
        is_tab_key && mods.control_key() && !mods.alt_key() && !mods.super_key()
    }

    #[allow(dead_code)]
    pub(crate) fn physical_key_binding_char(
        key: &neoism_window::event::KeyEvent,
    ) -> Option<&'static str> {
        let PhysicalKey::Code(code) = key.physical_key else {
            return None;
        };
        match code {
            KeyCode::KeyA => Some("a"),
            KeyCode::KeyB => Some("b"),
            KeyCode::KeyC => Some("c"),
            KeyCode::KeyD => Some("d"),
            KeyCode::KeyE => Some("e"),
            KeyCode::KeyF => Some("f"),
            KeyCode::KeyG => Some("g"),
            KeyCode::KeyH => Some("h"),
            KeyCode::KeyI => Some("i"),
            KeyCode::KeyJ => Some("j"),
            KeyCode::KeyK => Some("k"),
            KeyCode::KeyL => Some("l"),
            KeyCode::KeyM => Some("m"),
            KeyCode::KeyN => Some("n"),
            KeyCode::KeyO => Some("o"),
            KeyCode::KeyP => Some("p"),
            KeyCode::KeyQ => Some("q"),
            KeyCode::KeyR => Some("r"),
            KeyCode::KeyS => Some("s"),
            KeyCode::KeyT => Some("t"),
            KeyCode::KeyU => Some("u"),
            KeyCode::KeyV => Some("v"),
            KeyCode::KeyW => Some("w"),
            KeyCode::KeyX => Some("x"),
            KeyCode::KeyY => Some("y"),
            KeyCode::KeyZ => Some("z"),
            KeyCode::Digit0 | KeyCode::Numpad0 => Some("0"),
            KeyCode::Digit1 | KeyCode::Numpad1 => Some("1"),
            KeyCode::Digit2 | KeyCode::Numpad2 => Some("2"),
            KeyCode::Digit3 | KeyCode::Numpad3 => Some("3"),
            KeyCode::Digit4 | KeyCode::Numpad4 => Some("4"),
            KeyCode::Digit5 | KeyCode::Numpad5 => Some("5"),
            KeyCode::Digit6 | KeyCode::Numpad6 => Some("6"),
            KeyCode::Digit7 | KeyCode::Numpad7 => Some("7"),
            KeyCode::Digit8 | KeyCode::Numpad8 => Some("8"),
            KeyCode::Digit9 | KeyCode::Numpad9 => Some("9"),
            KeyCode::Backquote => Some("`"),
            KeyCode::Backslash | KeyCode::IntlBackslash | KeyCode::IntlRo => Some("\\"),
            KeyCode::BracketLeft => Some("["),
            KeyCode::BracketRight => Some("]"),
            KeyCode::Comma => Some(","),
            KeyCode::Equal | KeyCode::NumpadAdd => Some("="),
            KeyCode::Minus | KeyCode::NumpadSubtract => Some("-"),
            KeyCode::Period => Some("."),
            KeyCode::Quote => Some("'"),
            KeyCode::Semicolon => Some(";"),
            KeyCode::Slash | KeyCode::NumpadDivide => Some("/"),
            KeyCode::Space => Some(" "),
            _ => None,
        }
    }

    pub(crate) fn physical_key_binding_match(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> Option<BindingKey> {
        #[cfg(target_os = "macos")]
        {
            if !mods.alt_key() {
                return None;
            }

            Self::physical_key_binding_char(key).map(|ch| BindingKey::Keycode {
                key: Key::Character(ch.into()),
                location: KeyLocation::Standard,
            })
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _ = (key, mods);
            None
        }
    }

    pub(crate) fn is_arrow_left_key(key: &neoism_window::event::KeyEvent) -> bool {
        matches!(key.logical_key, Key::Named(NamedKey::ArrowLeft))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::ArrowLeft))
    }

    pub(crate) fn is_arrow_right_key(key: &neoism_window::event::KeyEvent) -> bool {
        matches!(key.logical_key, Key::Named(NamedKey::ArrowRight))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::ArrowRight))
    }

    pub(crate) fn is_arrow_up_key(key: &neoism_window::event::KeyEvent) -> bool {
        matches!(key.logical_key, Key::Named(NamedKey::ArrowUp))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::ArrowUp))
    }

    pub(crate) fn is_arrow_down_key(key: &neoism_window::event::KeyEvent) -> bool {
        matches!(key.logical_key, Key::Named(NamedKey::ArrowDown))
            || matches!(key.physical_key, PhysicalKey::Code(KeyCode::ArrowDown))
    }

    pub(crate) fn is_chrome_focus_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> Option<bool> {
        if !mods.alt_key() || mods.control_key() || mods.shift_key() || mods.super_key() {
            return None;
        }
        if Self::is_arrow_left_key(key) {
            return Some(false);
        }
        if Self::is_arrow_right_key(key) {
            return Some(true);
        }
        None
    }

    pub(crate) fn is_chrome_resize_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> Option<bool> {
        if !mods.alt_key() || !mods.control_key() || mods.shift_key() || mods.super_key()
        {
            return None;
        }
        if Self::is_arrow_left_key(key) {
            return Some(false);
        }
        if Self::is_arrow_right_key(key) {
            return Some(true);
        }
        None
    }

    pub(crate) fn is_control_insert_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.control_key()
            && !mods.shift_key()
            && !mods.alt_key()
            && !mods.super_key()
            && (matches!(key.logical_key, Key::Named(NamedKey::Insert))
                || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Insert)))
    }

    pub(crate) fn font_size_action_for_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> Option<FontSizeAction> {
        use neoism_ui::lifecycle_policy::{
            font_size_action_decide, FontSizeAction as SharedAction, FontSizeKeyInput,
            LifecycleMods,
        };

        // Build the shared POD input by collapsing the multiple winit
        // channels for "text equals X" into a single bool per glyph.
        let key_without_mods = key.key_without_modifiers();
        let text = key.text_with_all_modifiers().or(key.text.as_deref());
        let char_is = |needle: &str| {
            text == Some(needle)
                || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == needle)
                || matches!(key_without_mods.as_ref(), Key::Character(ch) if ch == needle)
        };
        let input = FontSizeKeyInput {
            text_is_equal: char_is("="),
            text_is_plus: char_is("+"),
            text_is_minus: char_is("-"),
            text_is_zero: char_is("0"),
            physical_is_equal_or_numpad_add: matches!(
                key.physical_key,
                PhysicalKey::Code(KeyCode::NumpadAdd | KeyCode::Equal)
            ),
            physical_is_minus_or_numpad_subtract: matches!(
                key.physical_key,
                PhysicalKey::Code(KeyCode::NumpadSubtract | KeyCode::Minus)
            ),
            physical_is_minus: matches!(
                key.physical_key,
                PhysicalKey::Code(KeyCode::Minus)
            ),
            physical_is_zero: matches!(
                key.physical_key,
                PhysicalKey::Code(KeyCode::Digit0 | KeyCode::Numpad0)
            ),
        };
        let pod_mods = LifecycleMods::new(
            mods.shift_key(),
            mods.control_key(),
            mods.alt_key(),
            mods.super_key(),
        );
        font_size_action_decide(input, pod_mods).map(|action| match action {
            SharedAction::Increase => FontSizeAction::Increase,
            SharedAction::Decrease => FontSizeAction::Decrease,
            SharedAction::Reset => FontSizeAction::Reset,
        })
    }

    pub(crate) fn is_shift_insert_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.shift_key()
            && !mods.control_key()
            && !mods.alt_key()
            && !mods.super_key()
            && (matches!(key.logical_key, Key::Named(NamedKey::Insert))
                || matches!(key.physical_key, PhysicalKey::Code(KeyCode::Insert)))
    }

    pub(crate) fn is_command_colon_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        if !mods.super_key() || mods.control_key() || mods.alt_key() {
            return false;
        }
        let text_is_colon = key.text_with_all_modifiers() == Some(":")
            || key.text.as_deref() == Some(":")
            || matches!(key.logical_key.as_ref(), Key::Character(ch) if ch == ":");
        // Fallback for systems/IMEs that globally steal Cmd+Shift+;
        // as a "quick phrase" trigger before Rio sees it. Cmd+; opens
        // the same command palette without needing Shift.
        let command_semicolon = !mods.shift_key()
            && matches!(key.physical_key, PhysicalKey::Code(KeyCode::Semicolon));
        let shifted_semicolon = mods.shift_key()
            && matches!(key.physical_key, PhysicalKey::Code(KeyCode::Semicolon));
        text_is_colon || shifted_semicolon || command_semicolon
    }

    pub(crate) fn is_command_files_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.alt_key()
            && !mods.shift_key()
            && !mods.control_key()
            && !mods.super_key()
            && (matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyS))
                || matches!(key.key_without_modifiers().as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("s")))
    }

    pub(crate) fn is_split_stack_toggle_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        mods.alt_key()
            && !mods.shift_key()
            && !mods.control_key()
            && !mods.super_key()
            && (matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyT))
                || matches!(key.key_without_modifiers().as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("t")))
    }

    pub(crate) fn is_split_stack_auto_tab_key(
        key: &neoism_window::event::KeyEvent,
        mods: ModifiersState,
    ) -> bool {
        ((mods.alt_key() && !mods.super_key()) || (mods.super_key() && !mods.alt_key()))
            && mods.shift_key()
            && !mods.control_key()
            && (matches!(key.physical_key, PhysicalKey::Code(KeyCode::KeyT))
                || matches!(key.key_without_modifiers().as_ref(), Key::Character(ch) if ch.eq_ignore_ascii_case("t")))
    }

    pub(crate) fn alt_send_esc(
        &mut self,
        key: &neoism_window::event::KeyEvent,
        text: &str,
    ) -> bool {
        #[cfg(not(target_os = "macos"))]
        let alt_send_esc = self.modifiers.state().alt_key();

        #[cfg(target_os = "macos")]
        let alt_send_esc = {
            let option_as_alt = &self.renderer.option_as_alt;
            self.modifiers.state().alt_key()
                && (option_as_alt == "both"
                    || (option_as_alt == "left"
                        && self.modifiers.lalt_state() == ModifiersKeyState::Pressed)
                    || (option_as_alt == "right"
                        && self.modifiers.ralt_state() == ModifiersKeyState::Pressed))
        };

        match key.logical_key {
            Key::Named(named) => {
                if named.to_text().is_some() {
                    alt_send_esc
                } else {
                    // Treat `Alt` as modifier for named keys without text, like ArrowUp.
                    self.modifiers.state().alt_key()
                }
            }
            _ => alt_send_esc && text.chars().count() == 1,
        }
    }
}
