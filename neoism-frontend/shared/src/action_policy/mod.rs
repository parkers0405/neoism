//! Shared keyboard / mouse binding policy.
//!
//! Action types, binding records, and the default + user-config binding
//! tables originally lived in `frontends/neoism/src/bindings/`. They were
//! parameterised on `winit` key types and lived in the desktop fork
//! alongside the run-loop. Web frontend re-implemented the same lookup
//! tables from scratch; the two slowly drifted.
//!
//! This module lifts the *policy* (action variants, mode bitflags,
//! binding tables) to plain-old-data types that depend only on
//! `neoism_terminal_core` and the existing shared `key_policy` /
//! `mouse_policy` POD enums. The desktop fork converts the POD lists
//! into its winit-typed structs via `From` adapters; the web frontend
//! consumes the same POD lists directly.
//!
//! Shape:
//! * [`Action`] is generic over the hint payload type `H` (desktop
//!   passes `Rc<neoism_backend::config::hints::Hint>`, web passes a
//!   smaller POD config).
//! * [`Binding`] is generic over the trigger type `T` (keyboard
//!   bindings use [`BindingKey`], mouse bindings use
//!   [`crate::mouse_policy::MouseButtonClass`]).
//! * [`BindingMode`] is the platform-neutral mode bitset, derivable
//!   from `neoism_terminal_core::crosswords::Mode`.
//! * [`defaults`] holds the static default bindings + user-config
//!   conversion + hint-binding builder.

use bitflags::bitflags;
use neoism_terminal_core::crosswords::vi_mode::ViMotion;
use neoism_terminal_core::crosswords::Mode;

use crate::key_policy::{
    normalize_config_key_name, ConfigKeyName, KeyPolicyKey, KeyPolicyLocation,
    KeyPolicyNamedKey,
};
use crate::mouse_policy::MouseButtonClass;

pub mod defaults;

bitflags! {
    /// Platform-neutral keyboard modifier bitset.
    ///
    /// Mirrors `winit::keyboard::ModifiersState` so the desktop fork can
    /// convert in either direction.
    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
    pub struct ActionModifiersState: u8 {
        const SHIFT   = 0b0000_0001;
        const CONTROL = 0b0000_0010;
        const ALT     = 0b0000_0100;
        const SUPER   = 0b0000_1000;
    }
}

/// Mouse binding specific actions.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MouseAction {
    /// Expand the selection to the current mouse cursor position.
    ExpandSelection,
}

impl<H> From<MouseAction> for Action<H> {
    fn from(action: MouseAction) -> Self {
        Self::Mouse(action)
    }
}

/// Search mode specific actions.
#[allow(clippy::enum_variant_names)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum SearchAction {
    /// Move the focus to the next search match.
    SearchFocusNext,
    /// Move the focus to the previous search match.
    SearchFocusPrevious,
    /// Confirm the active search.
    SearchConfirm,
    /// Cancel the active search.
    SearchCancel,
    /// Reset the search regex.
    SearchClear,
    /// Delete the last word in the search regex.
    SearchDeleteWord,
    /// Go to the previous regex in the search history.
    SearchHistoryPrevious,
    /// Go to the next regex in the search history.
    SearchHistoryNext,
}

impl<H> From<SearchAction> for Action<H> {
    fn from(action: SearchAction) -> Self {
        Self::Search(action)
    }
}

/// Vi mode specific actions.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum ViAction {
    /// Toggle normal vi selection.
    ToggleNormalSelection,
    /// Toggle line vi selection.
    ToggleLineSelection,
    /// Toggle block vi selection.
    ToggleBlockSelection,
    /// Toggle semantic vi selection.
    ToggleSemanticSelection,
    /// Centers the screen around the vi mode cursor.
    CenterAroundViCursor,
}

impl<H> From<ViAction> for Action<H> {
    fn from(action: ViAction) -> Self {
        Self::Vi(action)
    }
}

impl<H> From<ViMotion> for Action<H> {
    fn from(motion: ViMotion) -> Self {
        Self::ViMotion(motion)
    }
}

/// Program reference for `Action::Run`.
#[derive(Debug, Clone, Eq, PartialEq)]
#[allow(unused)]
pub enum Program {
    Just(String),
    WithArgs { program: String, args: Vec<String> },
}

impl Program {
    pub fn program(&self) -> &str {
        match self {
            Program::Just(program) => program,
            Program::WithArgs { program, .. } => program,
        }
    }

    pub fn args(&self) -> &[String] {
        match self {
            Program::Just(_) => &[],
            Program::WithArgs { args, .. } => args,
        }
    }
}

/// Platform-neutral key trigger for keyboard bindings.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum BindingKey {
    #[allow(dead_code)]
    Scancode(u32),
    Keycode {
        key: KeyPolicyKey,
        location: KeyPolicyLocation,
    },
}

bitflags! {
    /// Modes available for key bindings.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct BindingMode: u8 {
        const APP_CURSOR          = 0b0000_0001;
        const APP_KEYPAD          = 0b0000_0010;
        const ALT_SCREEN          = 0b0000_0100;
        const VI                  = 0b0000_1000;
        const SEARCH              = 0b0001_0000;
        const DISAMBIGUATE_KEYS   = 0b0010_0000;
        const ALL_KEYS_AS_ESC     = 0b0100_0000;
    }
}

impl BindingMode {
    pub fn new(mode: &Mode, search: bool) -> BindingMode {
        let mut binding_mode = BindingMode::empty();
        binding_mode.set(BindingMode::APP_CURSOR, mode.contains(Mode::APP_CURSOR));
        binding_mode.set(BindingMode::APP_KEYPAD, mode.contains(Mode::APP_KEYPAD));
        binding_mode.set(BindingMode::ALT_SCREEN, mode.contains(Mode::ALT_SCREEN));
        binding_mode.set(BindingMode::SEARCH, search);
        binding_mode.set(
            BindingMode::DISAMBIGUATE_KEYS,
            mode.contains(Mode::DISAMBIGUATE_ESC_CODES),
        );
        binding_mode.set(
            BindingMode::ALL_KEYS_AS_ESC,
            mode.contains(Mode::REPORT_ALL_KEYS_AS_ESC),
        );
        binding_mode.set(BindingMode::VI, mode.contains(Mode::VI));
        binding_mode
    }
}

/// Mode wrapper used while parsing user-config mode strings.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeWrapper {
    pub mode: BindingMode,
    pub not_mode: BindingMode,
}

/// A single binding row. Generic over trigger type `T` and hint payload `H`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding<T, H> {
    /// Modifier keys required to activate binding.
    pub mods: ActionModifiersState,

    /// String to send to PTY if mods and mode match.
    pub action: Action<H>,

    /// Binding mode required to activate binding.
    pub mode: BindingMode,

    /// Excluded binding modes where the binding won't be activated.
    pub notmode: BindingMode,

    /// This property is used as part of the trigger detection code.
    pub trigger: T,
}

impl<T: Eq, H: PartialEq> Binding<T, H> {
    #[inline]
    pub fn is_triggered_by(
        &self,
        mode: BindingMode,
        mods: ActionModifiersState,
        input: &T,
    ) -> bool {
        // Check input first since bindings are stored in one big list. This is
        // the most likely item to fail so prioritizing it here allows more
        // checks to be short circuited.
        self.trigger == *input
            && self.mods == mods
            && mode.contains(self.mode.clone())
            && !mode.intersects(self.notmode.clone())
    }

    #[inline]
    pub fn triggers_match(&self, binding: &Binding<T, H>) -> bool {
        // Check the binding's key and modifiers.
        if self.trigger != binding.trigger || self.mods != binding.mods {
            return false;
        }

        let selfmode = if self.mode.is_empty() {
            BindingMode::all()
        } else {
            self.mode.clone()
        };
        let bindingmode = if binding.mode.is_empty() {
            BindingMode::all()
        } else {
            binding.mode.clone()
        };

        if !selfmode.intersects(bindingmode) {
            return false;
        }

        // The bindings are never active at the same time when the required modes of one binding
        // are part of the forbidden bindings of the other.
        if self.mode.intersects(binding.notmode.clone())
            || binding.mode.intersects(self.notmode.clone())
        {
            return false;
        }

        true
    }
}

/// Convenience type aliases for the POD keyboard / mouse bindings.
pub type KeyBindingPod<H> = Binding<BindingKey, H>;
pub type MouseBindingPod<H> = Binding<MouseButtonClass, H>;

/// All actions Neoism's binding tables can dispatch.
///
/// Generic over the hint payload `H` so the desktop fork (which uses
/// `Rc<neoism_backend::config::hints::Hint>`) and the web frontend (which
/// uses a smaller POD config) can share the same enum shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action<H> {
    /// Write an escape sequence.
    Esc(String),

    /// Run given command.
    Run(Program),

    /// Scroll
    Scroll(i32),

    /// Activate hint mode with the given hint payload.
    Hint(H),

    // Move vi mode cursor.
    ViMotion(ViMotion),

    // Perform vi mode action.
    Vi(ViAction),
    /// Perform mouse binding exclusive action.
    Mouse(MouseAction),

    /// Paste contents of system clipboard.
    Paste,

    /// Store current selection into clipboard.
    Copy,

    #[cfg(not(any(target_os = "macos", windows)))]
    #[allow(dead_code)]
    /// Store current selection into selection buffer.
    CopySelection,

    /// Paste contents of selection buffer.
    PasteSelection,

    /// Increase font size.
    IncreaseFontSize,

    /// Decrease font size.
    DecreaseFontSize,

    /// Reset font size to the config value.
    ResetFontSize,

    /// Scroll exactly one page up.
    ScrollPageUp,

    /// Scroll exactly one page down.
    ScrollPageDown,

    /// Scroll half a page up.
    ScrollHalfPageUp,

    /// Scroll half a page down.
    ScrollHalfPageDown,

    /// Scroll all the way to the top.
    ScrollToTop,

    /// Scroll all the way to the bottom.
    ScrollToBottom,

    /// Clear the display buffer(s) to remove history.
    ClearHistory,

    /// Hide the Rio window.
    #[allow(dead_code)]
    Hide,

    /// Hide all windows other than Rio on macOS.
    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    HideOtherApplications,

    /// Minimize the Rio window.
    #[allow(dead_code)]
    Minimize,

    /// Quit Rio.
    Quit,

    /// Clear warning and error notices.
    ClearLogNotice,

    /// Spawn a new instance of Rio.
    #[allow(dead_code)]
    SpawnNewInstance,

    /// Create a new Rio window.
    #[allow(dead_code)]
    WindowCreateNew,

    /// Create config editor.
    ConfigEditor,

    /// Create a new Rio tab.
    TabCreateNew,

    /// Create a new terminal tab in the current workspace.
    WorkspaceTerminalTabCreateNew,

    /// Move current tab to previous slot.
    MoveCurrentTabToPrev,

    /// Move current tab to next slot.
    MoveCurrentTabToNext,

    /// Move current buffer tab to previous slot.
    MoveActiveBufferTabToPrev,

    /// Move current buffer tab to next slot.
    MoveActiveBufferTabToNext,

    /// Move the active top-level workspace tab to the previous slot.
    MoveWorkspaceTabToPrev,

    /// Move the active top-level workspace tab to the next slot.
    MoveWorkspaceTabToNext,

    /// Switch to next top-level workspace tab.
    SelectNextTab,

    /// Switch to previous top-level workspace tab.
    SelectPrevTab,

    /// Switch to next buffer tab in the active strip.
    SelectNextBufferTab,

    /// Switch to previous buffer tab in the active strip.
    SelectPrevBufferTab,

    /// Close tab.
    TabCloseCurrent,

    CloseCurrentSplitOrTab,

    /// Close all other tabs (leave only the current tab).
    TabCloseUnfocused,

    /// Toggle fullscreen.
    #[allow(dead_code)]
    ToggleFullscreen,

    /// Toggle maximized.
    #[allow(dead_code)]
    ToggleMaximized,

    /// Toggle simple fullscreen on macOS.
    #[cfg(target_os = "macos")]
    #[allow(dead_code)]
    ToggleSimpleFullscreen,

    /// Clear active selection.
    ClearSelection,

    /// Toggle vi mode.
    ToggleViMode,

    /// Toggle appearance theme (dark/light).
    ToggleAppearanceTheme,

    // Tab selections
    SelectTab(usize),
    SelectLastTab,

    Search(SearchAction),
    /// Start a forward buffer search.
    SearchForward,

    /// Start a backward buffer search.
    SearchBackward,

    /// Split horizontally
    SplitRight,

    /// Split vertically
    SplitDown,

    /// Select next split
    SelectNextSplit,

    /// Select previous split
    SelectPrevSplit,

    /// Select next split if available if not next tab
    SelectNextSplitOrTab,

    /// Select previous split if available if not previous tab
    SelectPrevSplitOrTab,

    /// Move divider up
    MoveDividerUp,

    /// Move divider down
    MoveDividerDown,

    /// Move divider left
    MoveDividerLeft,

    /// Move divider right
    MoveDividerRight,

    /// Toggle the command palette overlay.
    OpenCommandPalette,

    /// Toggle the file tree side panel (visibility + focus). When the
    /// tree is hidden, this opens it and gives it focus. When the tree
    /// is visible and focused, this hides it. When visible but not
    /// focused, this focuses it without hiding. Stand-in default for
    /// the planned `<space>e` leader sequence.
    ToggleFileTree,

    /// Open the global Neoism notes sidebar.
    OpenNeoismNotes,

    /// Toggle the git diff side panel.
    ToggleGitDiffPanel,

    /// Allow receiving char input.
    ReceiveChar,

    /// No action.
    None,
}

impl<H> From<&'static str> for Action<H> {
    fn from(s: &'static str) -> Action<H> {
        Action::Esc(s.into())
    }
}

/// Parse an `Action` from a user-config string (case-insensitive).
///
/// Unrecognised strings produce `Action::None`. The `hint` variant is
/// **not** parsable from a string (hint bindings come from
/// `create_hint_bindings`).
pub fn parse_action_from_string<H>(action: String) -> Action<H> {
    let action = action.to_lowercase();

    let action_from_string = match action.as_str() {
        "paste" => Some(Action::Paste),
        "quit" => Some(Action::Quit),
        "copy" => Some(Action::Copy),
        "searchforward" => Some(Action::SearchForward),
        "searchbackward" => Some(Action::SearchBackward),
        "searchconfirm" => Some(Action::Search(SearchAction::SearchConfirm)),
        "searchcancel" => Some(Action::Search(SearchAction::SearchCancel)),
        "searchclear" => Some(Action::Search(SearchAction::SearchClear)),
        "searchfocusnext" => Some(Action::Search(SearchAction::SearchFocusNext)),
        "searchfocusprevious" => {
            Some(Action::Search(SearchAction::SearchFocusPrevious))
        }
        "searchdeleteword" => Some(Action::Search(SearchAction::SearchDeleteWord)),
        "searchhistorynext" => Some(Action::Search(SearchAction::SearchHistoryNext)),
        "searchhistoryprevious" => {
            Some(Action::Search(SearchAction::SearchHistoryPrevious))
        }
        "clearhistory" => Some(Action::ClearHistory),
        "resetfontsize" => Some(Action::ResetFontSize),
        "increasefontsize" => Some(Action::IncreaseFontSize),
        "decreasefontsize" => Some(Action::DecreaseFontSize),
        "createwindow" => Some(Action::WindowCreateNew),
        "createtab" => Some(Action::TabCreateNew),
        "createworkspaceterminaltab" => Some(Action::WorkspaceTerminalTabCreateNew),
        "movecurrenttabtoprev" => Some(Action::MoveCurrentTabToPrev),
        "movecurrenttabtonext" => Some(Action::MoveCurrentTabToNext),
        "moveactivebuffertabtoprev" => Some(Action::MoveActiveBufferTabToPrev),
        "moveactivebuffertabtonext" => Some(Action::MoveActiveBufferTabToNext),
        "moveworkspacetabtoprev" => Some(Action::MoveWorkspaceTabToPrev),
        "moveworkspacetabtonext" => Some(Action::MoveWorkspaceTabToNext),
        "closetab" => Some(Action::TabCloseCurrent),
        "closesplitortab" => Some(Action::CloseCurrentSplitOrTab),
        "closeunfocusedtabs" => Some(Action::TabCloseUnfocused),
        "openconfigeditor" => Some(Action::ConfigEditor),
        "selectprevtab" => Some(Action::SelectPrevTab),
        "selectnexttab" => Some(Action::SelectNextTab),
        "selectprevbuffertab" => Some(Action::SelectPrevBufferTab),
        "selectnextbuffertab" => Some(Action::SelectNextBufferTab),
        "selectlasttab" => Some(Action::SelectLastTab),
        "receivechar" => Some(Action::ReceiveChar),
        "scrollpageup" => Some(Action::ScrollPageUp),
        "scrollpagedown" => Some(Action::ScrollPageDown),
        "scrollhalfpageup" => Some(Action::ScrollHalfPageUp),
        "scrollhalfpagedown" => Some(Action::ScrollHalfPageDown),
        "scrolltotop" => Some(Action::ScrollToTop),
        "scrolltobottom" => Some(Action::ScrollToBottom),
        "splitright" => Some(Action::SplitRight),
        "splitdown" => Some(Action::SplitDown),
        "selectnextsplit" => Some(Action::SelectNextSplit),
        "selectprevsplit" => Some(Action::SelectPrevSplit),
        "selectnextsplitortab" => Some(Action::SelectNextSplitOrTab),
        "selectprevsplitortab" => Some(Action::SelectPrevSplitOrTab),
        "movedividerup" => Some(Action::MoveDividerUp),
        "movedividerdown" => Some(Action::MoveDividerDown),
        "movedividerleft" => Some(Action::MoveDividerLeft),
        "movedividerright" => Some(Action::MoveDividerRight),
        "togglevimode" => Some(Action::ToggleViMode),
        "toggleappearancetheme" => Some(Action::ToggleAppearanceTheme),
        "togglefullscreen" => Some(Action::ToggleFullscreen),
        "opencommandpalette" => Some(Action::OpenCommandPalette),
        "openneoismnotes" | "toggleneoismnotes" => Some(Action::OpenNeoismNotes),
        "togglegitdiffpanel" => Some(Action::ToggleGitDiffPanel),
        "none" => Some(Action::None),
        _ => None,
    };

    if let Some(parsed) = action_from_string {
        return parsed;
    }

    let re = regex::Regex::new(r"selecttab\(([^()]+)\)").unwrap();
    for capture in re.captures_iter(&action) {
        if let Some(matched) = capture.get(1) {
            let matched_string = matched.as_str().to_string();
            let parsed_matched_string: usize = matched_string.parse().unwrap_or(0);
            return Action::SelectTab(parsed_matched_string);
        }
    }

    let re = regex::Regex::new(r"run\(([^()]+)\)").unwrap();
    for capture in re.captures_iter(&action) {
        if let Some(matched) = capture.get(1) {
            let matched_string = matched.as_str().to_string();
            if matched_string.contains(' ') {
                let mut vec_program_with_args: Vec<String> =
                    matched_string.split(' ').map(|s| s.to_string()).collect();
                if vec_program_with_args.is_empty() {
                    continue;
                }

                let program = vec_program_with_args[0].to_string();
                vec_program_with_args.remove(0);

                return Action::Run(Program::WithArgs {
                    program,
                    args: vec_program_with_args,
                });
            } else {
                return Action::Run(Program::Just(matched_string));
            }
        }
    }

    let re = regex::Regex::new(r"scroll\(([^()]+)\)").unwrap();
    for capture in re.captures_iter(&action) {
        if let Some(matched) = capture.get(1) {
            let matched_string = matched.as_str().to_string();
            let parsed_matched_string: i32 = matched_string.parse().unwrap_or(1);
            return Action::Scroll(parsed_matched_string);
        }
    }

    Action::None
}

/// POD mirror of `neoism_backend::config::bindings::KeyBinding`. Used by
/// the shared config-binding converter so it doesn't depend on the
/// backend crate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigKeyBindingPod {
    pub key: String,
    pub action: String,
    pub with: String,
    pub esc: String,
    pub mode: String,
}

/// Convert a single config key binding into a POD `KeyBinding`.
pub fn convert_config_key_binding<H>(
    config_key_binding: ConfigKeyBindingPod,
) -> Result<KeyBindingPod<H>, String> {
    let ConfigKeyName { key, location } =
        normalize_config_key_name(&config_key_binding.key)
            .ok_or_else(|| "Unable to find defined 'keycode'".to_string())?;

    let trigger = BindingKey::Keycode { key, location };

    let mut res = ActionModifiersState::empty();
    for modifier in config_key_binding.with.split('|') {
        match modifier.trim().to_lowercase().as_str() {
            "command" | "super" => res.insert(ActionModifiersState::SUPER),
            "shift" => res.insert(ActionModifiersState::SHIFT),
            "alt" | "option" => res.insert(ActionModifiersState::ALT),
            "control" => res.insert(ActionModifiersState::CONTROL),
            "none" => (),
            _ => (),
        }
    }

    let mut action: Action<H> = parse_action_from_string(config_key_binding.action);
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

    Ok(KeyBindingPod {
        trigger,
        mods: res,
        action,
        mode: res_mode.mode,
        notmode: res_mode.not_mode,
    })
}

/// Merge user-config bindings into the default list, evicting any
/// defaults that would conflict with a user binding.
pub fn config_key_bindings<H: PartialEq + std::fmt::Debug>(
    config_key_bindings: Vec<ConfigKeyBindingPod>,
    mut bindings: Vec<KeyBindingPod<H>>,
) -> Vec<KeyBindingPod<H>> {
    if config_key_bindings.is_empty() {
        return bindings;
    }

    for ckb in config_key_bindings {
        match convert_config_key_binding::<H>(ckb) {
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

/// Config description of a single hint binding.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HintBindingPod {
    pub key: String,
    pub mods: Vec<String>,
}

/// Build the hint-binding list from `(hint_config, payload)` pairs.
///
/// The caller threads its own hint config alongside the shared
/// description: desktop passes the original `Hint` payload wrapped in
/// `Rc`; web passes its smaller POD value. Returned bindings are sorted
/// in the same order as the input.
pub fn create_hint_bindings<H: Clone>(
    hints: impl IntoIterator<Item = (HintBindingPod, H)>,
) -> Vec<KeyBindingPod<H>> {
    let mut hint_bindings = Vec::new();

    for (binding_config, payload) in hints {
        let ConfigKeyName { key, location } =
            match normalize_config_key_name(&binding_config.key) {
                Some(name) => name,
                None => {
                    tracing::warn!(
                        "Unknown key '{}' in hint binding",
                        binding_config.key
                    );
                    continue;
                }
            };

        // Parse modifiers
        let mut mods = ActionModifiersState::empty();
        for mod_str in &binding_config.mods {
            match mod_str.to_lowercase().as_str() {
                "control" | "ctrl" => mods |= ActionModifiersState::CONTROL,
                "shift" => mods |= ActionModifiersState::SHIFT,
                "alt" | "option" => mods |= ActionModifiersState::ALT,
                "super" | "cmd" | "command" => mods |= ActionModifiersState::SUPER,
                _ => {
                    tracing::warn!("Unknown modifier '{}' in hint binding", mod_str);
                }
            }
        }

        hint_bindings.push(KeyBindingPod {
            trigger: BindingKey::Keycode { key, location },
            mods,
            mode: BindingMode::empty(),
            notmode: BindingMode::SEARCH | BindingMode::VI,
            action: Action::Hint(payload),
        });
    }

    hint_bindings
}

// ----- Helper builders used by `defaults` ------------------------------

/// Build a key binding from a `BindingKey` + mods + mode bits.
#[inline]
pub fn key_binding<A, H>(
    trigger: BindingKey,
    mods: ActionModifiersState,
    mode: BindingMode,
    notmode: BindingMode,
    action: A,
) -> KeyBindingPod<H>
where
    A: Into<Action<H>>,
{
    KeyBindingPod {
        trigger,
        mods,
        mode,
        notmode,
        action: action.into(),
    }
}

/// Build a mouse binding from a button + mods + mode bits.
#[inline]
pub fn mouse_binding<A, H>(
    trigger: MouseButtonClass,
    mods: ActionModifiersState,
    mode: BindingMode,
    notmode: BindingMode,
    action: A,
) -> MouseBindingPod<H>
where
    A: Into<Action<H>>,
{
    MouseBindingPod {
        trigger,
        mods,
        mode,
        notmode,
        action: action.into(),
    }
}

/// Trigger key from a literal character.
#[inline]
pub fn char_key(c: &str) -> BindingKey {
    BindingKey::Keycode {
        key: KeyPolicyKey::Character(c.into()),
        location: KeyPolicyLocation::Standard,
    }
}

/// Trigger key from a named key.
#[inline]
pub fn named_key(named: KeyPolicyNamedKey) -> BindingKey {
    BindingKey::Keycode {
        key: KeyPolicyKey::Named(named),
        location: KeyPolicyLocation::Standard,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type HintPod = u32;
    type MockBinding = Binding<usize, HintPod>;

    impl Default for MockBinding {
        fn default() -> Self {
            Self {
                mods: ActionModifiersState::empty(),
                action: Action::None,
                mode: BindingMode::empty(),
                notmode: BindingMode::empty(),
                trigger: 0,
            }
        }
    }

    #[test]
    fn binding_matches_itself() {
        let binding = MockBinding::default();
        let identical_binding = MockBinding::default();

        assert!(binding.triggers_match(&identical_binding));
        assert!(identical_binding.triggers_match(&binding));
    }

    #[test]
    fn binding_matches_different_action() {
        let binding = MockBinding::default();
        let different_action = MockBinding {
            action: Action::ClearHistory,
            ..MockBinding::default()
        };

        assert!(binding.triggers_match(&different_action));
        assert!(different_action.triggers_match(&binding));
    }

    #[test]
    fn mods_binding_requires_strict_match() {
        let superset_mods = MockBinding {
            mods: ActionModifiersState::all(),
            ..MockBinding::default()
        };
        let subset_mods = MockBinding {
            mods: ActionModifiersState::ALT,
            ..MockBinding::default()
        };

        assert!(!superset_mods.triggers_match(&subset_mods));
        assert!(!subset_mods.triggers_match(&superset_mods));
    }

    #[test]
    fn binding_matches_identical_mode() {
        let b1 = MockBinding {
            mode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            mode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };

        assert!(b1.triggers_match(&b2));
        assert!(b2.triggers_match(&b1));
    }

    #[test]
    fn binding_without_mode_matches_any_mode() {
        let b1 = MockBinding::default();
        let b2 = MockBinding {
            mode: BindingMode::APP_KEYPAD,
            notmode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };

        assert!(b1.triggers_match(&b2));
    }

    #[test]
    fn binding_with_mode_matches_empty_mode() {
        let b1 = MockBinding {
            mode: BindingMode::APP_KEYPAD,
            notmode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };
        let b2 = MockBinding::default();

        assert!(b1.triggers_match(&b2));
        assert!(b2.triggers_match(&b1));
    }

    #[test]
    fn binding_matches_modes() {
        let b1 = MockBinding {
            mode: BindingMode::ALT_SCREEN | BindingMode::APP_KEYPAD,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            mode: BindingMode::APP_KEYPAD,
            ..MockBinding::default()
        };

        assert!(b1.triggers_match(&b2));
        assert!(b2.triggers_match(&b1));
    }

    #[test]
    fn binding_matches_partial_intersection() {
        let b1 = MockBinding {
            mode: BindingMode::ALT_SCREEN | BindingMode::APP_KEYPAD,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            mode: BindingMode::APP_KEYPAD | BindingMode::APP_CURSOR,
            ..MockBinding::default()
        };

        assert!(b1.triggers_match(&b2));
        assert!(b2.triggers_match(&b1));
    }

    #[test]
    fn binding_mismatches_notmode() {
        let b1 = MockBinding {
            mode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            notmode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };

        assert!(!b1.triggers_match(&b2));
        assert!(!b2.triggers_match(&b1));
    }

    #[test]
    fn binding_mismatches_unrelated() {
        let b1 = MockBinding {
            mode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            mode: BindingMode::APP_KEYPAD,
            ..MockBinding::default()
        };

        assert!(!b1.triggers_match(&b2));
        assert!(!b2.triggers_match(&b1));
    }

    #[test]
    fn binding_matches_notmodes() {
        let subset_notmodes = MockBinding {
            notmode: BindingMode::VI | BindingMode::APP_CURSOR,
            ..MockBinding::default()
        };
        let superset_notmodes = MockBinding {
            notmode: BindingMode::APP_CURSOR,
            ..MockBinding::default()
        };

        assert!(subset_notmodes.triggers_match(&superset_notmodes));
        assert!(superset_notmodes.triggers_match(&subset_notmodes));
    }

    #[test]
    fn binding_matches_mode_notmode() {
        let b1 = MockBinding {
            mode: BindingMode::VI,
            notmode: BindingMode::APP_CURSOR,
            ..MockBinding::default()
        };
        let b2 = MockBinding {
            notmode: BindingMode::APP_CURSOR,
            ..MockBinding::default()
        };

        assert!(b1.triggers_match(&b2));
        assert!(b2.triggers_match(&b1));
    }

    #[test]
    fn binding_trigger_modes() {
        let binding = MockBinding {
            mode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };

        let t = binding.trigger;
        let mods = binding.mods;

        assert!(!binding.is_triggered_by(BindingMode::VI, mods, &t));
        assert!(binding.is_triggered_by(BindingMode::ALT_SCREEN, mods, &t));
        assert!(binding.is_triggered_by(BindingMode::ALT_SCREEN | BindingMode::VI, mods, &t));
    }

    #[test]
    fn binding_trigger_notmodes() {
        let binding = MockBinding {
            notmode: BindingMode::ALT_SCREEN,
            ..MockBinding::default()
        };

        let t = binding.trigger;
        let mods = binding.mods;

        assert!(binding.is_triggered_by(BindingMode::VI, mods, &t));
        assert!(!binding.is_triggered_by(BindingMode::ALT_SCREEN, mods, &t));
        assert!(!binding.is_triggered_by(
            BindingMode::ALT_SCREEN | BindingMode::VI,
            mods,
            &t
        ));
    }

    #[test]
    fn bindings_overwrite() {
        let bindings: Vec<KeyBindingPod<HintPod>> = vec![
            key_binding(
                char_key("q"),
                ActionModifiersState::SUPER,
                BindingMode::empty(),
                BindingMode::empty(),
                Action::<HintPod>::Quit,
            ),
            key_binding(
                char_key(","),
                ActionModifiersState::SUPER,
                BindingMode::empty(),
                BindingMode::empty(),
                Action::<HintPod>::ConfigEditor,
            ),
        ];

        let config_bindings = vec![ConfigKeyBindingPod {
            key: String::from("q"),
            action: String::from("receivechar"),
            with: String::from("super"),
            esc: String::from(""),
            mode: String::from(""),
        }];

        let new_bindings = config_key_bindings::<HintPod>(config_bindings, bindings);

        assert_eq!(new_bindings.len(), 2);
        assert_eq!(new_bindings[1].action, Action::<HintPod>::ReceiveChar);
    }

    #[test]
    fn bindings_conflict_resolution() {
        let bindings: Vec<KeyBindingPod<HintPod>> = vec![
            key_binding(
                named_key(KeyPolicyNamedKey::PageUp),
                ActionModifiersState::empty(),
                BindingMode::empty(),
                BindingMode::empty(),
                Action::<HintPod>::Esc("\x1b[5~".into()),
            ),
            key_binding(
                named_key(KeyPolicyNamedKey::PageDown),
                ActionModifiersState::empty(),
                BindingMode::empty(),
                BindingMode::empty(),
                Action::<HintPod>::Esc("\x1b[6~".into()),
            ),
        ];

        let config_bindings = vec![
            ConfigKeyBindingPod {
                key: String::from("pageup"),
                action: String::from("scroll(1)"),
                with: String::from(""),
                esc: String::from(""),
                mode: String::from(""),
            },
            ConfigKeyBindingPod {
                key: String::from("pagedown"),
                action: String::from("scroll(-1)"),
                with: String::from(""),
                esc: String::from(""),
                mode: String::from(""),
            },
        ];

        let new_bindings = config_key_bindings::<HintPod>(config_bindings, bindings);

        assert_eq!(new_bindings.len(), 2);

        let has_scroll_actions = new_bindings
            .iter()
            .any(|b| matches!(b.action, Action::Scroll(_)));
        assert!(has_scroll_actions);
    }

    #[test]
    fn bindings_alt_enter_conflict_resolution() {
        let bindings: Vec<KeyBindingPod<HintPod>> = vec![key_binding(
            named_key(KeyPolicyNamedKey::Enter),
            ActionModifiersState::ALT,
            BindingMode::empty(),
            BindingMode::empty(),
            Action::<HintPod>::ToggleFullscreen,
        )];

        let config_bindings = vec![ConfigKeyBindingPod {
            key: String::from("return"),
            action: String::from("scroll(1)"),
            with: String::from("alt"),
            esc: String::from(""),
            mode: String::from(""),
        }];

        let new_bindings = config_key_bindings::<HintPod>(config_bindings, bindings);

        assert_eq!(new_bindings.len(), 1);

        assert_eq!(&new_bindings[0].action, &Action::<HintPod>::Scroll(1));
    }
}
