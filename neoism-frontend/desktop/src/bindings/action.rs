// Binding<T>, MouseAction, BindingMode, Action, default_key_bindings and including their comments
// was originally taken from https://github.com/alacritty/alacritty/blob/e35e5ad14fce8456afdd89f2b392b9924bb27471/alacritty/src/config/bindings.rs
// which is licensed under Apache 2.0 license.

use bitflags::bitflags;
use neoism_terminal_core::crosswords::vi_mode::ViMotion;
use neoism_terminal_core::crosswords::Mode;
use neoism_window::event::MouseButton;
use neoism_window::keyboard::{Key, KeyLocation, ModifiersState, PhysicalKey};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontSizeAction {
    Increase,
    Decrease,
    Reset,
}

/// Mouse binding specific actions.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum MouseAction {
    /// Expand the selection to the current mouse cursor position.
    ExpandSelection,
}

impl From<MouseAction> for Action {
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

impl From<SearchAction> for Action {
    fn from(action: SearchAction) -> Self {
        Self::Search(action)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding<T> {
    /// Modifier keys required to activate binding.
    pub mods: ModifiersState,

    /// String to send to PTY if mods and mode match.
    pub action: Action,

    /// Binding mode required to activate binding.
    pub mode: BindingMode,

    /// Excluded binding modes where the binding won't be activated.
    pub notmode: BindingMode,

    /// This property is used as part of the trigger detection code.
    ///
    /// For example, this might be a key like "G", or a mouse button.
    pub trigger: T,
}

impl<T: Eq> Binding<T> {
    #[inline]
    pub fn is_triggered_by(
        &self,
        mode: BindingMode,
        mods: ModifiersState,
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
    pub fn triggers_match(&self, binding: &Binding<T>) -> bool {
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

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub enum BindingKey {
    #[allow(dead_code)]
    Scancode(PhysicalKey),
    Keycode {
        key: Key,
        location: KeyLocation,
    },
}

pub type KeyBinding = Binding<BindingKey>;
pub type KeyBindings = Vec<KeyBinding>;

/// Bindings that are triggered by a mouse button.
pub type MouseBinding = Binding<MouseButton>;

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

impl From<String> for Action {
    fn from(action: String) -> Action {
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

        if action_from_string.is_some() {
            return action_from_string.unwrap_or(Action::None);
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
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Write an escape sequence.
    Esc(String),

    /// Run given command.
    Run(Program),

    /// Scroll
    Scroll(i32),

    /// Activate hint mode with the given hint index
    Hint(std::rc::Rc<neoism_backend::config::hints::Hint>),

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

impl From<&'static str> for Action {
    fn from(s: &'static str) -> Action {
        Action::Esc(s.into())
    }
}

impl From<ViMotion> for Action {
    fn from(motion: ViMotion) -> Self {
        Self::ViMotion(motion)
    }
}

impl From<ViAction> for Action {
    fn from(action: ViAction) -> Self {
        Self::Vi(action)
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeWrapper {
    pub mode: BindingMode,
    pub not_mode: BindingMode,
}
