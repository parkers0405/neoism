//! Default keyboard / mouse bindings in POD form.
//!
//! These were lifted verbatim from
//! `frontends/neoism/src/bindings/defaults.rs`. They use the
//! platform-neutral [`super::BindingKey`] +
//! [`crate::mouse_policy::MouseButtonClass`] +
//! [`super::ActionModifiersState`] trio so the desktop fork (winit)
//! and web (DOM) can share the lookup tables and only translate
//! triggers at the I/O boundary.
//!
//! The hint payload type is left generic because the desktop fork
//! pins it to `Rc<neoism_backend::config::hints::Hint>` while the
//! web frontend uses a smaller POD value; default-binding tables
//! contain no `Action::Hint` rows themselves, so this is free.

use crate::key_policy::KeyPolicyNamedKey;
use crate::mouse_policy::MouseButtonClass;
use neoism_terminal_core::crosswords::vi_mode::ViMotion;

use super::{
    char_key, mouse_binding, named_key, Action, ActionModifiersState, BindingMode,
    KeyBindingPod, MouseAction, MouseBindingPod, SearchAction, ViAction,
};

/// Default mouse bindings.
pub fn default_mouse_bindings<H>() -> Vec<MouseBindingPod<H>> {
    let mut v = Vec::new();
    v.push(mouse_binding(
        MouseButtonClass::Right,
        ActionModifiersState::empty(),
        BindingMode::empty(),
        BindingMode::empty(),
        MouseAction::ExpandSelection,
    ));
    v.push(mouse_binding(
        MouseButtonClass::Right,
        ActionModifiersState::CONTROL,
        BindingMode::empty(),
        BindingMode::empty(),
        MouseAction::ExpandSelection,
    ));
    v.push(mouse_binding(
        MouseButtonClass::Middle,
        ActionModifiersState::empty(),
        BindingMode::empty(),
        BindingMode::VI,
        Action::<H>::PasteSelection,
    ));
    v
}

/// Default platform-neutral key bindings (without per-OS additions and
/// without user-config merges). Hosts splice in platform + hint + user
/// bindings on top.
pub fn default_key_bindings_base<H>() -> Vec<KeyBindingPod<H>> {
    use KeyPolicyNamedKey::*;

    let no_mods = ActionModifiersState::empty();
    let empty = BindingMode::empty();

    let mut v: Vec<KeyBindingPod<H>> = Vec::new();

    // Note: the desktop fork additionally seeds rows for
    // `Key::Named(Copy)` / `Key::Named(Paste)` (winit XF86 media keys)
    // — those don't exist in our POD enum, so the desktop wrapper
    // splices them in via its own helper after this baseline runs.

    // We'll build the table in tabular form; helper closures keep the
    // call sites readable.
    let mut add =
        |trigger, mods: ActionModifiersState, mode: BindingMode, notmode: BindingMode, action: Action<H>| {
            v.push(KeyBindingPod {
                trigger,
                mods,
                mode,
                notmode,
                action,
            });
        };

    // ---- Scroll/escape sequences that don't need a NamedKey row ----
    add(
        char_key("l"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::ClearLogNotice,
    );
    add(
        char_key("l"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::Esc("\x0c".into()),
    );

    add(
        named_key(Home),
        ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::ALT_SCREEN,
        Action::<H>::ScrollToTop,
    );
    add(
        named_key(End),
        ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::ALT_SCREEN,
        Action::<H>::ScrollToBottom,
    );
    add(
        named_key(PageUp),
        ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::ALT_SCREEN,
        Action::<H>::ScrollPageUp,
    );
    add(
        named_key(PageDown),
        ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::ALT_SCREEN,
        Action::<H>::ScrollPageDown,
    );

    add(
        named_key(Home),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOH".into()),
    );
    add(
        named_key(End),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOF".into()),
    );
    add(
        named_key(ArrowUp),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOA".into()),
    );
    add(
        named_key(ArrowDown),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOB".into()),
    );
    add(
        named_key(ArrowRight),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOC".into()),
    );
    add(
        named_key(ArrowLeft),
        no_mods,
        BindingMode::APP_CURSOR,
        BindingMode::VI,
        Action::<H>::Esc("\x1bOD".into()),
    );

    // IDE chrome
    add(
        char_key("e"),
        ActionModifiersState::ALT,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::ToggleFileTree,
    );
    add(
        char_key("n"),
        ActionModifiersState::ALT,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::OpenNeoismNotes,
    );
    add(
        char_key("g"),
        ActionModifiersState::ALT,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::ToggleGitDiffPanel,
    );

    // VI mode
    add(
        named_key(Space),
        ActionModifiersState::ALT | ActionModifiersState::SHIFT,
        empty.clone(),
        empty.clone(),
        Action::<H>::ToggleViMode,
    );
    add(
        char_key("/"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::SearchForward,
    );
    add(
        char_key("n"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        SearchAction::SearchFocusNext.into(),
    );
    add(
        char_key("n"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        SearchAction::SearchFocusPrevious.into(),
    );
    add(
        named_key(Enter),
        no_mods,
        BindingMode::SEARCH,
        BindingMode::VI,
        SearchAction::SearchFocusNext.into(),
    );
    add(
        named_key(Enter),
        no_mods,
        BindingMode::SEARCH | BindingMode::VI,
        empty.clone(),
        SearchAction::SearchConfirm.into(),
    );
    add(
        named_key(Escape),
        no_mods,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchCancel.into(),
    );
    add(
        named_key(Enter),
        ActionModifiersState::SHIFT,
        BindingMode::SEARCH,
        BindingMode::VI,
        SearchAction::SearchFocusPrevious.into(),
    );
    add(
        char_key("i"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ToggleViMode,
    );
    add(
        char_key("c"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        empty.clone(),
        Action::<H>::ToggleViMode,
    );
    add(
        named_key(Escape),
        no_mods,
        BindingMode::VI,
        empty.clone(),
        Action::<H>::ClearSelection,
    );
    add(
        char_key("i"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollToBottom,
    );
    add(
        char_key("g"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollToTop,
    );
    add(
        char_key("g"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollToBottom,
    );
    add(
        char_key("b"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollPageUp,
    );
    add(
        char_key("f"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollPageDown,
    );
    add(
        char_key("u"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollHalfPageUp,
    );
    add(
        char_key("d"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ScrollHalfPageDown,
    );
    add(
        char_key("y"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::Scroll(1),
    );
    add(
        char_key("e"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::Scroll(-1),
    );
    add(
        char_key("y"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::Copy,
    );
    add(
        char_key("y"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        Action::<H>::ClearSelection,
    );
    add(
        char_key("v"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViAction::ToggleNormalSelection.into(),
    );
    add(
        char_key("v"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViAction::ToggleLineSelection.into(),
    );
    add(
        char_key("v"),
        ActionModifiersState::CONTROL,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViAction::ToggleBlockSelection.into(),
    );
    add(
        char_key("v"),
        ActionModifiersState::ALT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViAction::ToggleSemanticSelection.into(),
    );
    add(
        char_key("z"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViAction::CenterAroundViCursor.into(),
    );
    add(
        char_key("k"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Up.into(),
    );
    add(
        char_key("j"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Down.into(),
    );
    add(
        char_key("h"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Left.into(),
    );
    add(
        char_key("l"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Right.into(),
    );
    add(
        named_key(ArrowUp),
        no_mods,
        BindingMode::VI,
        empty.clone(),
        ViMotion::Up.into(),
    );
    add(
        named_key(ArrowDown),
        no_mods,
        BindingMode::VI,
        empty.clone(),
        ViMotion::Down.into(),
    );
    add(
        named_key(ArrowLeft),
        no_mods,
        BindingMode::VI,
        empty.clone(),
        ViMotion::Left.into(),
    );
    add(
        named_key(ArrowRight),
        no_mods,
        BindingMode::VI,
        empty.clone(),
        ViMotion::Right.into(),
    );
    add(
        named_key(ArrowUp),
        ActionModifiersState::SUPER,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::None,
    );
    add(
        named_key(ArrowDown),
        ActionModifiersState::SUPER,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::None,
    );
    add(
        named_key(ArrowLeft),
        ActionModifiersState::SUPER,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::None,
    );
    add(
        named_key(ArrowRight),
        ActionModifiersState::SUPER,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::None,
    );
    add(
        char_key("0"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::First.into(),
    );
    add(
        char_key("4"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Last.into(),
    );
    add(
        char_key("6"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::FirstOccupied.into(),
    );
    add(
        char_key("h"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::High.into(),
    );
    add(
        char_key("m"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Middle.into(),
    );
    add(
        char_key("l"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Low.into(),
    );
    add(
        char_key("b"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::SemanticLeft.into(),
    );
    add(
        char_key("w"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::SemanticRight.into(),
    );
    add(
        char_key("e"),
        no_mods,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::SemanticRightEnd.into(),
    );
    add(
        char_key("b"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::WordLeft.into(),
    );
    add(
        char_key("w"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::WordRight.into(),
    );
    add(
        char_key("e"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::WordRightEnd.into(),
    );
    add(
        char_key("5"),
        ActionModifiersState::SHIFT,
        BindingMode::VI,
        BindingMode::SEARCH,
        ViMotion::Bracket.into(),
    );

    // ---- "Escape" tail block (non-APP_CURSOR, non-VI/SEARCH/ALL_KEYS_AS_ESC) ----
    let tail_notmode_nav = BindingMode::VI
        | BindingMode::SEARCH
        | BindingMode::ALL_KEYS_AS_ESC;
    let tail_notmode_full = BindingMode::VI
        | BindingMode::SEARCH
        | BindingMode::ALL_KEYS_AS_ESC
        | BindingMode::DISAMBIGUATE_KEYS;

    add(
        named_key(ArrowUp),
        no_mods,
        empty.clone(),
        BindingMode::APP_CURSOR | tail_notmode_nav.clone(),
        Action::<H>::Esc("\x1b[A".into()),
    );
    add(
        named_key(ArrowDown),
        no_mods,
        empty.clone(),
        BindingMode::APP_CURSOR | tail_notmode_nav.clone(),
        Action::<H>::Esc("\x1b[B".into()),
    );
    add(
        named_key(ArrowRight),
        no_mods,
        empty.clone(),
        BindingMode::APP_CURSOR | tail_notmode_nav.clone(),
        Action::<H>::Esc("\x1b[C".into()),
    );
    add(
        named_key(ArrowLeft),
        no_mods,
        empty.clone(),
        BindingMode::APP_CURSOR | tail_notmode_nav,
        Action::<H>::Esc("\x1b[D".into()),
    );

    add(
        named_key(Insert),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b[2~".into()),
    );
    add(
        named_key(Delete),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b[3~".into()),
    );
    add(
        named_key(PageUp),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b[5~".into()),
    );
    add(
        named_key(PageDown),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b[6~".into()),
    );
    add(
        named_key(Backspace),
        no_mods,
        empty.clone(),
        BindingMode::VI | BindingMode::SEARCH | BindingMode::ALL_KEYS_AS_ESC,
        Action::<H>::Esc("\x7f".into()),
    );
    add(
        named_key(Backspace),
        ActionModifiersState::ALT,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b\x7f".into()),
    );
    add(
        named_key(Backspace),
        ActionModifiersState::SHIFT,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x7f".into()),
    );
    add(
        named_key(F1),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1bOP".into()),
    );
    add(
        named_key(F2),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1bOQ".into()),
    );
    add(
        named_key(F3),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1bOR".into()),
    );
    add(
        named_key(F4),
        no_mods,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1bOS".into()),
    );
    add(
        named_key(Tab),
        ActionModifiersState::SHIFT,
        empty.clone(),
        tail_notmode_full.clone(),
        Action::<H>::Esc("\x1b[Z".into()),
    );
    add(
        named_key(Tab),
        ActionModifiersState::SHIFT | ActionModifiersState::ALT,
        empty.clone(),
        tail_notmode_full,
        Action::<H>::Esc("\x1b\x1b[Z".into()),
    );

    v
}

/// Platform-neutral platform default bindings (Linux / BSD / Windows
/// share these; macOS overrides them via the desktop adapter). Mirrors
/// `frontends/neoism/src/bindings/platform/linux.rs`.
pub fn platform_key_bindings_unix<H>(
    use_navigation_key_bindings: bool,
    use_splits: bool,
) -> Vec<KeyBindingPod<H>> {
    use KeyPolicyNamedKey::*;
    let no_mods = ActionModifiersState::empty();
    let empty = BindingMode::empty();

    let mut v: Vec<KeyBindingPod<H>> = Vec::new();
    let mut add =
        |trigger, mods: ActionModifiersState, mode: BindingMode, notmode: BindingMode, action: Action<H>| {
            v.push(KeyBindingPod {
                trigger,
                mods,
                mode,
                notmode,
                action,
            });
        };

    add(
        char_key("v"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::Paste,
    );
    add(
        char_key("c"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        empty.clone(),
        Action::<H>::Copy,
    );
    add(
        char_key("c"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        BindingMode::VI,
        empty.clone(),
        Action::<H>::ClearSelection,
    );
    add(
        char_key("c"),
        ActionModifiersState::SUPER,
        empty.clone(),
        empty.clone(),
        Action::<H>::Copy,
    );
    add(
        char_key("c"),
        ActionModifiersState::SUPER,
        BindingMode::VI,
        empty.clone(),
        Action::<H>::ClearSelection,
    );
    add(
        named_key(Insert),
        ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::VI,
        Action::<H>::Paste,
    );
    add(
        char_key("0"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::ResetFontSize,
    );
    add(
        char_key("="),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::IncreaseFontSize,
    );
    add(
        char_key("+"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::IncreaseFontSize,
    );
    add(
        char_key("+"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::IncreaseFontSize,
    );
    add(
        char_key("-"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::DecreaseFontSize,
    );
    add(
        char_key("-"),
        ActionModifiersState::CONTROL,
        empty.clone(),
        empty.clone(),
        Action::<H>::DecreaseFontSize,
    );
    add(
        char_key("n"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        empty.clone(),
        Action::<H>::WindowCreateNew,
    );
    add(
        char_key(","),
        ActionModifiersState::ALT,
        empty.clone(),
        empty.clone(),
        Action::<H>::ConfigEditor,
    );
    add(
        char_key("p"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        empty.clone(),
        Action::<H>::OpenCommandPalette,
    );

    // Search
    add(
        char_key("f"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::SEARCH,
        Action::<H>::SearchForward,
    );
    add(
        char_key("b"),
        ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
        empty.clone(),
        BindingMode::SEARCH,
        Action::<H>::SearchBackward,
    );
    add(
        char_key("c"),
        ActionModifiersState::CONTROL,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchCancel.into(),
    );
    add(
        char_key("u"),
        ActionModifiersState::CONTROL,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchClear.into(),
    );
    add(
        char_key("w"),
        ActionModifiersState::CONTROL,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchDeleteWord.into(),
    );
    add(
        char_key("p"),
        ActionModifiersState::CONTROL,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchHistoryPrevious.into(),
    );
    add(
        char_key("n"),
        ActionModifiersState::CONTROL,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchHistoryNext.into(),
    );
    add(
        named_key(ArrowUp),
        no_mods,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchHistoryPrevious.into(),
    );
    add(
        named_key(ArrowDown),
        no_mods,
        BindingMode::SEARCH,
        empty.clone(),
        SearchAction::SearchHistoryNext.into(),
    );

    if use_navigation_key_bindings {
        add(
            char_key("t"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::WorkspaceTerminalTabCreateNew,
        );
        add(
            named_key(Tab),
            ActionModifiersState::CONTROL,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectNextTab,
        );
        add(
            named_key(Tab),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectPrevTab,
        );
        add(
            named_key(ArrowLeft),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectPrevBufferTab,
        );
        add(
            named_key(ArrowRight),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectNextBufferTab,
        );
        add(
            named_key(ArrowLeft),
            ActionModifiersState::ALT | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::MoveActiveBufferTabToPrev,
        );
        add(
            named_key(ArrowRight),
            ActionModifiersState::ALT | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::MoveActiveBufferTabToNext,
        );
        add(
            char_key("["),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectPrevBufferTab,
        );
        add(
            char_key("]"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::SelectNextBufferTab,
        );
        add(
            char_key("w"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            empty.clone(),
            Action::<H>::TabCreateNew,
        );
    }

    if use_splits {
        let split_notmode = BindingMode::SEARCH | BindingMode::VI;
        add(
            char_key("r"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::SplitRight,
        );
        add(
            char_key("d"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::SplitDown,
        );
        add(
            char_key("]"),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::SelectNextSplit,
        );
        add(
            char_key("["),
            ActionModifiersState::CONTROL | ActionModifiersState::SHIFT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::SelectPrevSplit,
        );
        add(
            named_key(ArrowUp),
            ActionModifiersState::CONTROL
                | ActionModifiersState::SHIFT
                | ActionModifiersState::ALT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::MoveDividerUp,
        );
        add(
            named_key(ArrowDown),
            ActionModifiersState::CONTROL
                | ActionModifiersState::SHIFT
                | ActionModifiersState::ALT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::MoveDividerDown,
        );
        add(
            named_key(ArrowLeft),
            ActionModifiersState::CONTROL
                | ActionModifiersState::SHIFT
                | ActionModifiersState::ALT,
            empty.clone(),
            split_notmode.clone(),
            Action::<H>::MoveDividerLeft,
        );
        add(
            named_key(ArrowRight),
            ActionModifiersState::CONTROL
                | ActionModifiersState::SHIFT
                | ActionModifiersState::ALT,
            empty.clone(),
            split_notmode,
            Action::<H>::MoveDividerRight,
        );
    }

    v
}

// Note: the desktop fork's `default_key_bindings()` additionally
// includes two rows keyed on `Key::Named(Copy)` and `Key::Named(Paste)`
// which are winit-specific named keys not represented in our POD
// `KeyPolicyNamedKey` enum (they're the XF86Copy / XF86Paste media
// keys). The desktop wrapper splices those two rows in on top of
// `default_key_bindings_base()` so web parity is preserved while desktop
// still honours the OS clipboard media keys.

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
#[allow(unused_imports)]
pub(crate) use platform_key_bindings_unix as platform_key_bindings_pod;
