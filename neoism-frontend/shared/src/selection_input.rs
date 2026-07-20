//! Shared selection-input / hint-mode policy.
//!
//! Pure decision functions extracted from the desktop fork's
//! `screen/selection.rs` so the web frontend can apply the same logic.
//! Sibling to [`crate::key_policy`] — that one owns IME math +
//! key-release gating; this one owns the hint-mode keystroke state
//! machine, the highlight-mode mutation plan, the mouse-binding
//! paste-selection rule, and the "build kitty key sequence vs raw
//! UTF-8" output-shape policy.
//!
//! What lives here:
//!
//! * Hint-mode key classification ([`HintKeyAction`],
//!   [`classify_hint_key`]).
//! * Hint label-matching state machine ([`HintKeystrokeDecision`],
//!   [`hint_keystroke_decision`]).
//! * Highlighted-hint mutation planner ([`HintHighlightTransition`],
//!   [`hint_highlight_transition`]).
//! * Mouse-binding helper ([`mouse_binding_effective_modifiers`]).
//! * Output-shape decision: kitty sequence vs raw UTF-8 bytes
//!   ([`KeySequenceShapeInput`], [`should_build_key_sequence`]).
//!
//! What stays in the desktop fork:
//!
//! * The actual `KeyEvent` / `Modifiers` / terminal lock acquisition /
//!   PTY write side effects. Callers translate their winit / web event
//!   into the POD inputs below and apply the returned decision.

// ---------------------------------------------------------------------------
// Hint-mode key classification
// ---------------------------------------------------------------------------

/// Logical-key kind tag the hint-mode dispatcher cares about.
///
/// Decouples the policy from `neoism_window::keyboard::NamedKey`; the
/// desktop fork (or web event bridge) folds its key event into this
/// before asking the policy what to do.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintLogicalKey {
    /// `Escape` — always exits hint mode.
    Escape,
    /// `Backspace` — pops one character from the keystroke buffer.
    Backspace,
    /// Anything else (printable, named, etc.) — caller feeds the
    /// associated text into the label matcher one char at a time.
    Other,
}

/// What the hint-mode loop should do with this key event.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintKeyAction {
    /// Stop hint mode immediately.
    StopHintMode,
    /// Feed `'\x08'` (backspace sentinel) into the label matcher.
    FeedBackspace,
    /// Feed each char of the key's text into the label matcher.
    FeedText,
}

/// Classify a hint-mode key event into the action the dispatcher takes.
///
/// Lifted straight out of `Screen::process_key_event`'s
/// `hint_state.is_active()` branch — Escape stops, Backspace pops a
/// char, anything else is treated as label-matching text input.
pub const fn classify_hint_key(key: HintLogicalKey) -> HintKeyAction {
    match key {
        HintLogicalKey::Escape => HintKeyAction::StopHintMode,
        HintLogicalKey::Backspace => HintKeyAction::FeedBackspace,
        HintLogicalKey::Other => HintKeyAction::FeedText,
    }
}

// ---------------------------------------------------------------------------
// Hint-mode label-matching state machine
// ---------------------------------------------------------------------------

/// Decision returned by [`hint_keystroke_decision`].
///
/// Mirrors the four branches of `HintState::keyboard_input`: cancel,
/// pop, no-op (label found but more chars needed), or fire a match.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintKeystrokeDecision {
    /// Caller should stop hint mode (`'\x1b'` Escape / `'\x03'`
    /// Ctrl+C sentinel).
    StopHintMode,
    /// Caller should pop the last entered key and re-run the matcher
    /// (`'\x08'` Backspace / `'\x1f'` Unit-Separator sentinel).
    PopKey,
    /// `c` matched the next char of exactly one visible label and
    /// completed it. Caller should fire the hint at `match_index`.
    /// If `persist`, the caller clears the keystroke buffer; otherwise
    /// it stops hint mode entirely (same rule as the desktop code).
    FireMatch { match_index: usize, persist: bool },
    /// `c` partially matched at least one visible label. Caller should
    /// push `c` onto the keystroke buffer and re-render.
    PushKey,
    /// `c` matched no visible label. Caller should ignore the event.
    Ignore,
}

/// Pure label-matching state machine for hint-mode keystrokes.
///
/// `visible_labels` is the output of `HintState::visible_labels()` —
/// the list of `(match_index, remaining_chars_after_keys_so_far)`
/// pairs. `persist` is the active hint's persist flag (copied out of
/// `Hint::persist`).
///
/// Returns the decision the caller should apply; the caller still
/// owns the actual `HintState` mutation (push key / pop key / stop)
/// because that state lives in the desktop fork's hint subsystem.
pub fn hint_keystroke_decision(
    c: char,
    visible_labels: &[(usize, Vec<char>)],
    persist: bool,
) -> HintKeystrokeDecision {
    // Backspace / Unit-Separator → pop last entered key.
    if c == '\x08' || c == '\x1f' {
        return HintKeystrokeDecision::PopKey;
    }
    // Escape / Ctrl+C → stop hint mode.
    if c == '\x1b' || c == '\x03' {
        return HintKeystrokeDecision::StopHintMode;
    }

    // Iterate in reverse to match the desktop code's
    // `visible_labels.iter().rev()` — keeps tie-break behavior
    // identical when multiple labels share a starting char.
    let mut matched: Option<(usize, &Vec<char>)> = None;
    for (idx, remaining) in visible_labels.iter().rev() {
        if !remaining.is_empty() && remaining[0] == c {
            matched = Some((*idx, remaining));
            break;
        }
    }

    let Some((idx, remaining)) = matched else {
        return HintKeystrokeDecision::Ignore;
    };

    if remaining.len() == 1 {
        HintKeystrokeDecision::FireMatch {
            match_index: idx,
            persist,
        }
    } else {
        HintKeystrokeDecision::PushKey
    }
}

// ---------------------------------------------------------------------------
// Highlighted-hint mutation planner
// ---------------------------------------------------------------------------

/// The mutation the desktop fork's `update_highlighted_hints` should
/// apply to renderable content + damage tracking. POD so the same
/// planner can drive the web frontend's hint highlight state.
///
/// `had_highlight` here is the "did the previous frame already have a
/// highlight?" question — the desktop code returns it as the function
/// result so callers can mark-dirty conditionally. The web bridge
/// will use the same value the same way.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintHighlightTransition {
    /// Don't highlight (modifiers/config gate failed) and the previous
    /// frame had no highlight either → nothing to do.
    NoChange,
    /// Don't highlight, but a previous highlight exists → clear it,
    /// damage the lines, force a full re-render.
    ClearHighlight {
        /// Carries the previous frame's `had_highlight` so the caller
        /// can return it from `update_highlighted_hints`.
        had_highlight: bool,
    },
    /// The mouse is no longer over a hint match (or off-grid) but a
    /// previous highlight exists → same as `ClearHighlight` but the
    /// returned `had_highlight` propagates.
    NoMatchClearHighlight { had_highlight: bool },
    /// A new hint highlight should be applied at `match_index`-style
    /// position. Caller damages the hint range and sets renderable
    /// `highlighted_hint`.
    SetHighlight,
}

/// Input to [`hint_highlight_transition`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HintHighlightInput {
    /// Result of `hint_highlight_eligible` — config + modifiers say
    /// hint highlighting is on right now.
    pub should_highlight: bool,
    /// Did the previous frame already have a highlight?
    pub had_highlight: bool,
    /// Could the caller resolve a mouse position over the terminal
    /// body? `None` when the mouse is outside the grid or the
    /// terminal lock was contested.
    pub mouse_in_grid: bool,
    /// Did the caller find a hint match at that mouse position?
    pub hint_match_found: bool,
}

/// Pure planner for the desktop fork's `update_highlighted_hints`
/// state machine.
///
/// Encodes the four branches:
///
/// 1. `!should_highlight` → clear (NoChange if already empty).
/// 2. `should_highlight && !mouse_in_grid` → clear (returns
///    `had_highlight`).
/// 3. `should_highlight && mouse_in_grid && hint_match_found` →
///    `SetHighlight`.
/// 4. `should_highlight && mouse_in_grid && !hint_match_found` →
///    `NoMatchClearHighlight` (returns `had_highlight`).
pub const fn hint_highlight_transition(
    input: HintHighlightInput,
) -> HintHighlightTransition {
    if !input.should_highlight {
        if input.had_highlight {
            return HintHighlightTransition::ClearHighlight {
                had_highlight: true,
            };
        }
        return HintHighlightTransition::NoChange;
    }
    if !input.mouse_in_grid {
        return HintHighlightTransition::NoMatchClearHighlight {
            had_highlight: input.had_highlight,
        };
    }
    if input.hint_match_found {
        return HintHighlightTransition::SetHighlight;
    }
    HintHighlightTransition::NoMatchClearHighlight {
        had_highlight: input.had_highlight,
    }
}

// ---------------------------------------------------------------------------
// Mouse-binding helper
// ---------------------------------------------------------------------------

/// Modifier-bit POD used by [`mouse_binding_effective_modifiers`].
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ModBits {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

impl ModBits {
    pub const fn new(shift: bool, control: bool, alt: bool, super_key: bool) -> Self {
        Self {
            shift,
            control,
            alt,
            super_key,
        }
    }
}

/// Compute the binding-modifier mask the mouse-binding matcher should
/// require for `binding_mods` given the current `mouse_mode` flag.
///
/// Mirrors the inline rule in `Screen::process_mouse_bindings`:
/// > Require shift for all modifiers when mouse mode is active.
///
/// In mouse-reporting mode the terminal owns mouse events; only a
/// shift-augmented binding can override that. Returns `binding_mods`
/// unchanged when `mouse_mode` is off, otherwise OR's in `shift`.
pub const fn mouse_binding_effective_modifiers(
    binding_mods: ModBits,
    mouse_mode: bool,
) -> ModBits {
    if mouse_mode {
        ModBits {
            shift: true,
            control: binding_mods.control,
            alt: binding_mods.alt,
            super_key: binding_mods.super_key,
        }
    } else {
        binding_mods
    }
}

// ---------------------------------------------------------------------------
// Output-shape decision: kitty key sequence vs raw UTF-8 bytes
// ---------------------------------------------------------------------------

/// Logical-key kind tag the output-shape policy cares about.
///
/// Decouples the decision from `neoism_window::keyboard::{NamedKey,
/// Key}` while preserving the exact branch shape of the original
/// `Screen::should_build_sequence`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputLogicalKey {
    /// `Escape`.
    Escape,
    /// `Tab`.
    Tab,
    /// `Enter`.
    Enter,
    /// `Backspace`.
    Backspace,
    /// Any other `Key::Named(_)` whose `.to_text()` is `Some(_)` — the
    /// named key has a textual representation (e.g. `Space`).
    NamedWithText,
    /// Any other `Key::Named(_)` whose `.to_text()` is `None` (e.g.
    /// `ArrowLeft`, `F1`).
    NamedWithoutText,
    /// `Key::Character(_)` or `Key::Unidentified(_)` — non-named.
    NonNamed,
}

impl OutputLogicalKey {
    const fn is_named_no_text(self) -> bool {
        matches!(self, OutputLogicalKey::NamedWithoutText)
    }

    const fn is_tab_enter_or_backspace(self) -> bool {
        matches!(
            self,
            OutputLogicalKey::Tab | OutputLogicalKey::Enter | OutputLogicalKey::Backspace
        )
    }

    const fn is_escape(self) -> bool {
        matches!(self, OutputLogicalKey::Escape)
    }
}

/// Input to [`should_build_key_sequence`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct KeySequenceShapeInput {
    pub key: OutputLogicalKey,
    /// True when the physical key is on the numpad.
    pub key_on_numpad: bool,
    /// True when the printable `text` for the event is empty.
    pub text_empty: bool,
    pub mods_empty: bool,
    pub mods_shift_only: bool,
    pub report_all_keys_as_esc: bool,
    pub disambiguate_esc_codes: bool,
}

/// Whether the desktop output stage should hand the key to
/// `build_key_sequence` (kitty-protocol CSI) or fall through to the
/// raw UTF-8 byte path (`bytes.push(0x1b)?` + `text.as_bytes()`).
///
/// Lifted from `Screen::should_build_sequence`. The same predicate
/// drives the web frontend so its outbound terminal stream matches
/// desktop byte-for-byte.
pub fn should_build_key_sequence(input: KeySequenceShapeInput) -> bool {
    if input.report_all_keys_as_esc {
        return true;
    }

    let disambiguate = input.disambiguate_esc_codes
        && (input.key.is_escape()
            || input.key_on_numpad
            || (!input.mods_empty
                && (!input.mods_shift_only || input.key.is_tab_enter_or_backspace())));

    if disambiguate {
        return true;
    }

    if input.key.is_named_no_text() {
        return true;
    }

    matches!(
        input.key,
        OutputLogicalKey::NamedWithText
            | OutputLogicalKey::Tab
            | OutputLogicalKey::Enter
            | OutputLogicalKey::Backspace
            | OutputLogicalKey::Escape
    )
    .then_some(false)
    .unwrap_or(input.text_empty)
}

// ---------------------------------------------------------------------------
// Early `process_key_event` branch dispatch
// ---------------------------------------------------------------------------

/// POD inputs to [`early_key_event_dispatch`].
///
/// Each `Option<bool>` / `bool` field is the resolved predicate the
/// desktop fork already computes from a winit `KeyEvent` +
/// `ModifiersState` (see `Screen::is_chrome_focus_key` and friends in
/// `screen/lifecycle.rs`). The web frontend can compute the same
/// predicates from its DOM event source.
///
/// The dispatch is pure: no state mutation, no key/event translation,
/// no allocation. Each branch's side effects live in the matching
/// [`EarlyKeyDispatchAction`] variant the caller executes.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Default)]
pub struct EarlyKeyDispatchInput {
    /// True for a pressed key event; false for a release.
    pub is_pressed: bool,
    /// `Some(right)` when the chrome-focus predicate matched: `true` to
    /// move chrome focus right, `false` for left.
    pub chrome_focus: Option<bool>,
    /// `Some(grow)` when the chrome-resize predicate matched: `true` to
    /// grow the focused chrome/split, `false` to shrink.
    pub chrome_resize: Option<bool>,
    /// `true` when the top-level workspace tab switch predicate matched
    /// (Cmd/Super + digit, etc.).
    pub is_top_level_workspace_tab_switch: bool,
    /// `true` when the workspace buffer tab switch predicate matched.
    pub is_workspace_buffer_tab_switch: bool,
    /// Shift modifier; consumed by the tab-switch branches to pick
    /// direction.
    pub shift: bool,
    /// `true` when the Ctrl+Insert predicate matched (Hyprland Super+C →
    /// Ctrl+Insert leak prevention).
    pub is_control_insert: bool,
    /// `true` when the Shift+Insert predicate matched.
    pub is_shift_insert: bool,
    /// `true` when the split-stack toggle predicate matched.
    pub is_split_stack_toggle: bool,
    /// True when there is more than one cell in the active grid — the
    /// stack-toggle branch is gated on this in the desktop fork.
    pub split_stack_toggle_unlocked: bool,
    /// `true` when the split-stack auto-tab predicate matched.
    pub is_split_stack_auto_tab: bool,
}

/// Action returned by [`early_key_event_dispatch`].
///
/// The desktop fork builds an [`EarlyKeyDispatchInput`] from the raw
/// `KeyEvent` + modifiers, calls [`early_key_event_dispatch`], then
/// matches the returned action to run the corresponding `&mut self`
/// method. [`EarlyKeyDispatchAction::PassThrough`] means no branch
/// matched — the caller continues with the rest of `process_key_event`.
/// [`EarlyKeyDispatchAction::ConsumeRelease`] means a branch matched but
/// the event was a release — the caller short-circuits without mutating
/// (matching the original code which unconditionally returned once the
/// predicate matched, regardless of press/release).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EarlyKeyDispatchAction {
    /// No early branch matched — caller falls through to the rest of
    /// `process_key_event`.
    PassThrough,
    /// An early branch matched, but the event is a release — caller
    /// returns from `process_key_event` without further work.
    ConsumeRelease,
    /// Move horizontal chrome focus; `right=true` to advance forward.
    FocusHorizontalChrome { right: bool },
    /// Resize the focused chrome/split; `grow=true` to enlarge.
    ResizeFocusedChromeOrSplit { grow: bool },
    /// Switch top-level workspace; `shift` selects the direction in the
    /// desktop fork.
    SelectTopLevelWorkspace { shift: bool },
    /// Switch workspace buffer tab; `shift` selects the direction.
    SelectWorkspaceBufferTab { shift: bool },
    /// Ctrl+Insert (Hyprland Super+C → Ctrl+Insert) — copy active
    /// selection to the system clipboard.
    ControlInsert,
    /// Shift+Insert — paste from the system clipboard into the active
    /// surface (editor / markdown / terminal).
    ShiftInsert,
    /// Toggle split-stack focus (only valid when more than one cell).
    ToggleSplitStackFocus,
    /// Move the active tab into the split stack.
    MoveActiveTabToSplitStack,
}

/// Resolve a slice of `Screen::process_key_event`'s early-return
/// branches — chrome focus/resize, top-level / workspace buffer tab
/// switching, Ctrl+Insert / Shift+Insert clipboard shortcuts, and the
/// split-stack toggle/auto-tab pair — into a single action the desktop
/// fork executes.
///
/// All predicates are resolved at the call site. This function is a
/// precedence-preserving match: the first matching branch wins, in the
/// exact order the original `if` chain ran.
///
/// The output mirrors the original control flow exactly:
///
/// * A press event runs the associated side effect and returns from
///   `process_key_event` (the caller does the same after matching the
///   returned action).
/// * A release event that matches a branch is consumed without effect
///   (returned as [`EarlyKeyDispatchAction::ConsumeRelease`] so the
///   caller can still emit any required tracing before short-circuiting).
/// * No branch matched → [`EarlyKeyDispatchAction::PassThrough`].
pub const fn early_key_event_dispatch(
    input: EarlyKeyDispatchInput,
) -> EarlyKeyDispatchAction {
    // Chrome focus / resize: original code fires for both press and
    // release but only does work on press.
    if let Some(right) = input.chrome_focus {
        return if input.is_pressed {
            EarlyKeyDispatchAction::FocusHorizontalChrome { right }
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }
    if let Some(grow) = input.chrome_resize {
        return if input.is_pressed {
            EarlyKeyDispatchAction::ResizeFocusedChromeOrSplit { grow }
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_top_level_workspace_tab_switch {
        return if input.is_pressed {
            EarlyKeyDispatchAction::SelectTopLevelWorkspace { shift: input.shift }
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_workspace_buffer_tab_switch {
        return if input.is_pressed {
            EarlyKeyDispatchAction::SelectWorkspaceBufferTab { shift: input.shift }
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_control_insert {
        return if input.is_pressed {
            EarlyKeyDispatchAction::ControlInsert
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_shift_insert {
        return if input.is_pressed {
            EarlyKeyDispatchAction::ShiftInsert
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_split_stack_toggle && input.split_stack_toggle_unlocked {
        return if input.is_pressed {
            EarlyKeyDispatchAction::ToggleSplitStackFocus
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    if input.is_split_stack_auto_tab {
        return if input.is_pressed {
            EarlyKeyDispatchAction::MoveActiveTabToSplitStack
        } else {
            EarlyKeyDispatchAction::ConsumeRelease
        };
    }

    EarlyKeyDispatchAction::PassThrough
}

// ---------------------------------------------------------------------------
// Mid `process_key_event` branch dispatch (post-binding consumption gates)
// ---------------------------------------------------------------------------

/// POD inputs to [`mid_key_event_dispatch`].
///
/// Captures the mutually-exclusive consumption gates the desktop
/// fork runs after `process_key_bindings` and the early dispatcher have
/// already had their chance: search-bar input, the code surface, the
/// markdown surface, and the vi-mode no-op short-circuit. Each gate is
/// represented by a single `bool` the caller has already resolved.
///
/// The gates are sequential, not predicate-mixed: the first one whose
/// flag is `true` wins, matching the chained `if … return;` blocks in
/// `Screen::process_key_event`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct MidKeyDispatchInput {
    /// `true` when `Screen::search_active()` is currently on. Routes the
    /// resolved key `text` into the search input one char at a time and
    /// marks dirty.
    pub search_active: bool,
    /// `true` when the active context owns a native code surface
    /// (`code.is_some()`). Routes the key to `dispatch_code_key`.
    pub code_active: bool,
    /// `true` when the active context owns a markdown surface
    /// (`markdown.is_some()`). Routes the key to `dispatch_markdown_key`.
    pub markdown_active: bool,
    /// `true` when the terminal is in `Mode::VI`. Vi mode swallows
    /// keystrokes here because its key input flows through the search
    /// branch (already handled above).
    pub vi_mode: bool,
}

/// Action returned by [`mid_key_event_dispatch`].
///
/// Mirrors the four post-binding return points in
/// `Screen::process_key_event` so the desktop fork (and the web
/// frontend) can collapse the gate chain into a single match.
///
/// [`MidKeyDispatchAction::PassThrough`] means no gate matched — the
/// caller continues into the terminal byte-builder path.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MidKeyDispatchAction {
    /// No gate matched — caller continues into the terminal byte path.
    PassThrough,
    /// Feed each char of the resolved key text into the search input
    /// and mark the screen dirty.
    RouteToSearch,
    /// Forward the key event to the native code surface via
    /// `dispatch_code_key`.
    RouteToCode,
    /// Forward the key event to the markdown surface via
    /// `dispatch_markdown_key`.
    RouteToMarkdown,
    /// Vi mode is active without a search input — swallow the event.
    ConsumeViMode,
}

/// Resolve the post-binding consumption gates in `process_key_event`.
///
/// The gates run in this exact precedence (matching the desktop fork's
/// original chained `if`s):
///
/// 1. `search_active` → [`MidKeyDispatchAction::RouteToSearch`]
/// 2. `code_active` → [`MidKeyDispatchAction::RouteToCode`]
/// 3. `markdown_active` → [`MidKeyDispatchAction::RouteToMarkdown`]
/// 4. `vi_mode` → [`MidKeyDispatchAction::ConsumeViMode`]
/// 5. otherwise → [`MidKeyDispatchAction::PassThrough`]
pub const fn mid_key_event_dispatch(input: MidKeyDispatchInput) -> MidKeyDispatchAction {
    if input.search_active {
        return MidKeyDispatchAction::RouteToSearch;
    }
    if input.code_active {
        return MidKeyDispatchAction::RouteToCode;
    }
    if input.markdown_active {
        return MidKeyDispatchAction::RouteToMarkdown;
    }
    if input.vi_mode {
        return MidKeyDispatchAction::ConsumeViMode;
    }
    MidKeyDispatchAction::PassThrough
}

// ---------------------------------------------------------------------------
// Hint-mode inner per-char loop post-call decision
// ---------------------------------------------------------------------------

/// Decision returned by [`hint_keystroke_result_action`].
///
/// Mirrors the post-call branching inside `Screen::process_key_event`'s
/// hint-mode inner per-character loop: after the caller invoked
/// `HintState::keyboard_input`, the returned `Option<HintMatch>` is
/// either `Some(_)` (drop the terminal lock, execute the action, stop
/// hint mode, update state, mark dirty, return) or `None` (drop the
/// terminal lock and keep feeding the next character).
///
/// The actual `HintMatch` execution / state mutation stays in the
/// desktop fork — this enum just lifts the branch the caller takes
/// after the pure result is observed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HintKeystrokeResultAction {
    /// `keyboard_input` returned `Some(HintMatch)` — caller should
    /// execute the matched hint action, stop hint mode, refresh hint
    /// state, mark the screen dirty and return from
    /// `process_key_event`.
    ExecuteAndStop,
    /// `keyboard_input` returned `None` — caller continues the inner
    /// loop with the next character (no early return).
    Continue,
}

/// Classify the `Option<HintMatch>` produced by `HintState::keyboard_input`
/// into the decision the per-char hint-mode loop takes.
///
/// `matched` is `true` when the result was `Some(_)` (i.e. the next
/// keystroke completed a label and a hint match was produced).
///
/// The desktop fork already owns the `HintMatch` value (and the
/// terminal lock); this helper just collapses the `if let Some(_) /
/// else` into a named decision the web frontend can reuse with the
/// same precedence.
pub const fn hint_keystroke_result_action(matched: bool) -> HintKeystrokeResultAction {
    if matched {
        HintKeystrokeResultAction::ExecuteAndStop
    } else {
        HintKeystrokeResultAction::Continue
    }
}

// ---------------------------------------------------------------------------
// Terminal-block input gate (pre-binding consumption)
// ---------------------------------------------------------------------------

/// POD inputs to [`terminal_block_input_gate`].
///
/// Captures the gate the desktop fork runs immediately after the
/// hint-mode branch and before `process_key_bindings`: when the active
/// pane has a terminal-block composer (think Warp-style command input),
/// keystrokes go to that composer instead of the bindings table —
/// *unless* the search bar or vi mode is already active.
///
/// The desktop fork resolves all three flags from its winit `KeyEvent`
/// + `ModifiersState` + the current `Mode` bitset; this helper just
/// applies the precedence so the web frontend can mirror it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalBlockInputGateInput {
    /// `true` when `Screen::search_active()` is on. Search owns key
    /// input and the block composer steps aside.
    pub search_active: bool,
    /// `true` when the terminal is in `Mode::VI`. Vi mode swallows
    /// keystrokes through its own pipeline.
    pub vi_mode: bool,
    /// `true` when `handle_terminal_block_input_key` consumed the key
    /// event. The desktop side already invoked the handler and
    /// resolved its boolean result; this helper just gates the
    /// short-circuit on the other two predicates so the web frontend
    /// can apply the same rule without re-running the handler.
    pub block_consumed: bool,
}

/// Action returned by [`terminal_block_input_gate`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalBlockInputGateAction {
    /// Block-input handler consumed the key — caller returns from
    /// `process_key_event` (after any required tracing).
    Consume,
    /// Gate did not fire — caller continues into `process_key_bindings`.
    PassThrough,
}

/// Resolve the terminal-block input gate that sits between the
/// hint-mode branch and `process_key_bindings`.
///
/// Equivalent to the original chained predicate:
///
/// ```text
/// if !search_active
///     && !mode.contains(Mode::VI)
///     && self.handle_terminal_block_input_key(...)
/// { return; }
/// ```
///
/// The desktop fork still invokes `handle_terminal_block_input_key`
/// itself (it owns the mutable state and the PTY messenger); this
/// helper just lifts the *decision* to short-circuit out of
/// `process_key_event` so the same precedence drives the web frontend.
pub const fn terminal_block_input_gate(
    input: TerminalBlockInputGateInput,
) -> TerminalBlockInputGateAction {
    if !input.search_active && !input.vi_mode && input.block_consumed {
        TerminalBlockInputGateAction::Consume
    } else {
        TerminalBlockInputGateAction::PassThrough
    }
}

// ---------------------------------------------------------------------------
// Non-kitty terminal byte builder + output-stage dispatch
// ---------------------------------------------------------------------------

/// Build the raw UTF-8 byte stream the desktop fork sends to the PTY
/// when [`should_build_key_sequence`] returned `false`.
///
/// Mirrors the inline `else` arm of `Screen::process_key_event`'s
/// kitty-vs-raw branch:
///
/// ```text
/// let mut bytes = Vec::with_capacity(text.len() + 1);
/// if mods.alt_key() { bytes.push(b'\x1b'); }
/// bytes.extend_from_slice(text.as_bytes());
/// bytes
/// ```
///
/// Pure helper — no `KeyEvent` translation, no allocation beyond the
/// returned `Vec`. The web frontend uses the identical byte shape so
/// its outbound terminal stream stays byte-for-byte with desktop.
pub fn build_non_kitty_terminal_bytes(text: &str, alt: bool) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(text.len() + 1);
    if alt {
        bytes.push(b'\x1b');
    }
    bytes.extend_from_slice(text.as_bytes());
    bytes
}

/// POD inputs to [`terminal_output_dispatch`].
///
/// Captures the post-build decision in `Screen::process_key_event`:
/// once the byte stream has been assembled (either via kitty's CSI
/// builder or the raw UTF-8 path), the caller decides whether to feed
/// the bytes to the PTY or skip the write entirely (empty output).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalOutputDispatchInput {
    /// `true` when the byte stream returned by either branch of the
    /// kitty-vs-raw decision was empty. Empty output skips the
    /// scroll/clear-selection side effects entirely.
    pub bytes_empty: bool,
}

/// Action returned by [`terminal_output_dispatch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TerminalOutputDispatchAction {
    /// Bytes were empty — caller skips the PTY write and only emits a
    /// trace log noting the no-op.
    SkipEmpty,
    /// Default terminal output — caller runs the scroll-to-bottom +
    /// clear-selection side effects, then writes the assembled bytes
    /// to the PTY messenger.
    SendToPty,
}

/// Resolve the post-build output dispatch in `Screen::process_key_event`.
///
/// Precedence mirrors the original `if !bytes.is_empty() { … PTY }
/// else { trace no-op }`:
///
/// 1. `bytes_empty` → [`TerminalOutputDispatchAction::SkipEmpty`]
/// 2. otherwise → [`TerminalOutputDispatchAction::SendToPty`]
///
/// The desktop fork (and the web frontend) still owns the side effects
/// — this helper just collapses the decision tree into a single match.
pub const fn terminal_output_dispatch(
    input: TerminalOutputDispatchInput,
) -> TerminalOutputDispatchAction {
    if input.bytes_empty {
        return TerminalOutputDispatchAction::SkipEmpty;
    }
    TerminalOutputDispatchAction::SendToPty
}

// ---------------------------------------------------------------------------
// Key-release dispatch (suppress / drop-named / emit sequence)
// ---------------------------------------------------------------------------

/// POD inputs to [`key_release_dispatch`].
///
/// Captures the entire decision tree the desktop fork runs inside the
/// `key.state == ElementState::Released` branch of `process_key_event`:
///
/// 1. The terminal-mode / vi / search / hint gates resolved by
///    [`crate::key_policy::should_suppress_key_release`].
/// 2. The named-key reportability gate resolved by
///    [`crate::key_policy::named_key_release_reportable`].
///
/// Both predicates were already pure but their precedence was inlined
/// as two separate `if` branches with `return` early-outs interspersed
/// with an alt-mask side calculation. This POD lets a single planner
/// match resolve the release path in one shot, matching the early/mid
/// dispatch shape used elsewhere in `process_key_event`.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct KeyReleaseDispatchInput {
    /// `Mode::REPORT_EVENT_TYPES` is on (kitty kbd protocol extension
    /// that surfaces key releases).
    pub report_event_types: bool,
    /// `Mode::VI` is on.
    pub vi_mode: bool,
    /// `Screen::search_active()` is on.
    pub search_active: bool,
    /// `Screen::hint_state.is_active()` is on.
    pub hint_active: bool,
    /// `true` when the released key's logical name is one of `Enter`,
    /// `Tab`, or `Backspace`. Those three named keys are not reported
    /// on release unless `REPORT_ALL_KEYS_AS_ESC` is on.
    pub is_enter_tab_or_backspace: bool,
    /// `Mode::REPORT_ALL_KEYS_AS_ESC` is on.
    pub report_all_keys_as_esc: bool,
}

/// Action returned by [`key_release_dispatch`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KeyReleaseDispatchAction {
    /// The terminal-mode/vi/search/hint gate fired — caller skips the
    /// release entirely (only emits a tracing log noting the gate).
    Suppress,
    /// The key is a named release that the terminal does not report
    /// without `REPORT_ALL_KEYS_AS_ESC` — caller drops it after the
    /// alt-mask + text calculation.
    DropUnreportableNamed,
    /// Release should be encoded with `build_key_sequence(key, mods,
    /// mode)` and written to the PTY.
    EmitSequence,
}

/// Resolve the release-branch decision in `Screen::process_key_event`.
///
/// Equivalent to the original chained predicates:
///
/// ```text
/// if should_suppress_key_release(...) { return; }
/// // ... compute alt-masked mods + text ...
/// if !named_key_release_reportable(...) { return; }
/// let bytes = build_key_sequence(...);
/// messenger.send_write(bytes);
/// ```
///
/// The desktop fork (and the web frontend) still owns the alt-mask
/// computation and the actual `build_key_sequence` + PTY write — this
/// helper just lifts the precedence so the same two-gate decision
/// drives both frontends in one match.
pub const fn key_release_dispatch(
    input: KeyReleaseDispatchInput,
) -> KeyReleaseDispatchAction {
    if crate::key_policy::should_suppress_key_release(
        input.report_event_types,
        input.vi_mode,
        input.search_active,
        input.hint_active,
    ) {
        return KeyReleaseDispatchAction::Suppress;
    }
    if !crate::key_policy::named_key_release_reportable(
        input.is_enter_tab_or_backspace,
        input.report_all_keys_as_esc,
    ) {
        return KeyReleaseDispatchAction::DropUnreportableNamed;
    }
    KeyReleaseDispatchAction::EmitSequence
}

#[cfg(test)]
mod tests {
    use super::*;

    // ----- classify_hint_key ------------------------------------------------

    #[test]
    fn hint_key_classification() {
        assert_eq!(
            classify_hint_key(HintLogicalKey::Escape),
            HintKeyAction::StopHintMode
        );
        assert_eq!(
            classify_hint_key(HintLogicalKey::Backspace),
            HintKeyAction::FeedBackspace
        );
        assert_eq!(
            classify_hint_key(HintLogicalKey::Other),
            HintKeyAction::FeedText
        );
    }

    // ----- hint_keystroke_decision ------------------------------------------

    #[test]
    fn hint_keystroke_backspace_pops() {
        assert_eq!(
            hint_keystroke_decision('\x08', &[], false),
            HintKeystrokeDecision::PopKey
        );
        assert_eq!(
            hint_keystroke_decision('\x1f', &[], false),
            HintKeystrokeDecision::PopKey
        );
    }

    #[test]
    fn hint_keystroke_escape_stops() {
        assert_eq!(
            hint_keystroke_decision('\x1b', &[(0, vec!['a'])], false),
            HintKeystrokeDecision::StopHintMode
        );
        assert_eq!(
            hint_keystroke_decision('\x03', &[(0, vec!['a'])], false),
            HintKeystrokeDecision::StopHintMode
        );
    }

    #[test]
    fn hint_keystroke_fires_on_unique_completion() {
        let labels = vec![(7, vec!['k'])];
        assert_eq!(
            hint_keystroke_decision('k', &labels, false),
            HintKeystrokeDecision::FireMatch {
                match_index: 7,
                persist: false,
            }
        );
        // persist flag propagates.
        assert_eq!(
            hint_keystroke_decision('k', &labels, true),
            HintKeystrokeDecision::FireMatch {
                match_index: 7,
                persist: true,
            }
        );
    }

    #[test]
    fn hint_keystroke_push_when_more_chars_remain() {
        let labels = vec![(2, vec!['k', 'b']), (3, vec!['k', 'a'])];
        // Both labels start with 'k' but neither completes on a single
        // char → push the char into the buffer for next iteration.
        assert_eq!(
            hint_keystroke_decision('k', &labels, false),
            HintKeystrokeDecision::PushKey
        );
    }

    #[test]
    fn hint_keystroke_ignore_no_match() {
        let labels = vec![(0, vec!['a']), (1, vec!['b'])];
        assert_eq!(
            hint_keystroke_decision('z', &labels, false),
            HintKeystrokeDecision::Ignore
        );
    }

    #[test]
    fn hint_keystroke_uses_last_matching_label_rev_order() {
        // Two labels share the first char 'k'. The first one (index 1)
        // would also complete, but the desktop code iterates `.rev()`
        // so the later index wins.
        let labels = vec![(1, vec!['k']), (4, vec!['k'])];
        assert_eq!(
            hint_keystroke_decision('k', &labels, false),
            HintKeystrokeDecision::FireMatch {
                match_index: 4,
                persist: false,
            }
        );
    }

    #[test]
    fn hint_keystroke_skips_empty_remaining() {
        // Defensive: a label with no remaining chars is never a match
        // candidate (would `remaining[0]` panic if we forgot).
        let labels = vec![(9, vec![])];
        assert_eq!(
            hint_keystroke_decision('a', &labels, false),
            HintKeystrokeDecision::Ignore
        );
    }

    // ----- hint_highlight_transition ----------------------------------------

    fn highlight(
        should: bool,
        had: bool,
        in_grid: bool,
        found: bool,
    ) -> HintHighlightInput {
        HintHighlightInput {
            should_highlight: should,
            had_highlight: had,
            mouse_in_grid: in_grid,
            hint_match_found: found,
        }
    }

    #[test]
    fn highlight_no_change_when_off_and_no_prev() {
        assert_eq!(
            hint_highlight_transition(highlight(false, false, false, false)),
            HintHighlightTransition::NoChange
        );
    }

    #[test]
    fn highlight_clear_when_off_with_prev() {
        assert_eq!(
            hint_highlight_transition(highlight(false, true, true, true)),
            HintHighlightTransition::ClearHighlight {
                had_highlight: true
            }
        );
    }

    #[test]
    fn highlight_no_match_clears_when_off_grid() {
        assert_eq!(
            hint_highlight_transition(highlight(true, true, false, false)),
            HintHighlightTransition::NoMatchClearHighlight {
                had_highlight: true
            }
        );
        // ...even when there was no previous highlight.
        assert_eq!(
            hint_highlight_transition(highlight(true, false, false, false)),
            HintHighlightTransition::NoMatchClearHighlight {
                had_highlight: false
            }
        );
    }

    #[test]
    fn highlight_set_when_match_found() {
        assert_eq!(
            hint_highlight_transition(highlight(true, false, true, true)),
            HintHighlightTransition::SetHighlight
        );
        assert_eq!(
            hint_highlight_transition(highlight(true, true, true, true)),
            HintHighlightTransition::SetHighlight
        );
    }

    #[test]
    fn highlight_no_match_when_in_grid_but_unmatched() {
        assert_eq!(
            hint_highlight_transition(highlight(true, true, true, false)),
            HintHighlightTransition::NoMatchClearHighlight {
                had_highlight: true
            }
        );
    }

    // ----- mouse_binding_effective_modifiers --------------------------------

    #[test]
    fn mouse_binding_passes_through_when_no_mouse_mode() {
        let bm = ModBits::new(false, true, false, false);
        assert_eq!(mouse_binding_effective_modifiers(bm, false), bm);
    }

    #[test]
    fn mouse_binding_forces_shift_in_mouse_mode() {
        let bm = ModBits::new(false, true, false, false);
        let out = mouse_binding_effective_modifiers(bm, true);
        assert!(out.shift);
        assert!(out.control);
        assert!(!out.alt);
        assert!(!out.super_key);
    }

    #[test]
    fn mouse_binding_preserves_already_shifted() {
        let bm = ModBits::new(true, false, true, true);
        let out = mouse_binding_effective_modifiers(bm, true);
        assert_eq!(out, bm);
    }

    // ----- should_build_key_sequence ----------------------------------------

    fn shape(key: OutputLogicalKey) -> KeySequenceShapeInput {
        KeySequenceShapeInput {
            key,
            key_on_numpad: false,
            text_empty: false,
            mods_empty: true,
            mods_shift_only: false,
            report_all_keys_as_esc: false,
            disambiguate_esc_codes: false,
        }
    }

    #[test]
    fn build_sequence_when_report_all_keys_as_esc() {
        let mut s = shape(OutputLogicalKey::NonNamed);
        s.report_all_keys_as_esc = true;
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_disambiguates_escape() {
        let mut s = shape(OutputLogicalKey::Escape);
        s.disambiguate_esc_codes = true;
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_disambiguates_numpad() {
        let mut s = shape(OutputLogicalKey::NonNamed);
        s.disambiguate_esc_codes = true;
        s.key_on_numpad = true;
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_disambiguates_non_shift_mods() {
        let mut s = shape(OutputLogicalKey::NonNamed);
        s.disambiguate_esc_codes = true;
        s.mods_empty = false;
        s.mods_shift_only = false;
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_shift_only_does_not_disambiguate_plain_char() {
        let mut s = shape(OutputLogicalKey::NonNamed);
        s.disambiguate_esc_codes = true;
        s.mods_empty = false;
        s.mods_shift_only = true;
        s.text_empty = false;
        // No disambiguate trigger, NonNamed with text → false.
        assert!(!should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_shift_only_disambiguates_tab_enter_bksp() {
        let mut s = shape(OutputLogicalKey::Tab);
        s.disambiguate_esc_codes = true;
        s.mods_empty = false;
        s.mods_shift_only = true;
        assert!(should_build_key_sequence(s));

        let mut s = shape(OutputLogicalKey::Enter);
        s.disambiguate_esc_codes = true;
        s.mods_empty = false;
        s.mods_shift_only = true;
        assert!(should_build_key_sequence(s));

        let mut s = shape(OutputLogicalKey::Backspace);
        s.disambiguate_esc_codes = true;
        s.mods_empty = false;
        s.mods_shift_only = true;
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_named_without_text_always_builds() {
        let s = shape(OutputLogicalKey::NamedWithoutText);
        assert!(should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_named_with_text_does_not_build() {
        // Named keys with `.to_text() = Some(_)` (e.g. `Space`) fall
        // through to the raw byte path.
        let s = shape(OutputLogicalKey::NamedWithText);
        assert!(!should_build_key_sequence(s));
    }

    #[test]
    fn build_sequence_non_named_only_builds_when_text_empty() {
        let mut s = shape(OutputLogicalKey::NonNamed);
        s.text_empty = true;
        assert!(should_build_key_sequence(s));

        let mut s = shape(OutputLogicalKey::NonNamed);
        s.text_empty = false;
        assert!(!should_build_key_sequence(s));
    }

    // ----- early_key_event_dispatch -----------------------------------------

    fn empty_early_input() -> EarlyKeyDispatchInput {
        EarlyKeyDispatchInput {
            is_pressed: true,
            ..Default::default()
        }
    }

    #[test]
    fn early_dispatch_pass_through_when_no_match() {
        assert_eq!(
            early_key_event_dispatch(empty_early_input()),
            EarlyKeyDispatchAction::PassThrough
        );
        let mut released = empty_early_input();
        released.is_pressed = false;
        assert_eq!(
            early_key_event_dispatch(released),
            EarlyKeyDispatchAction::PassThrough
        );
    }

    #[test]
    fn early_dispatch_chrome_focus_routes_press_and_swallows_release() {
        let mut input = empty_early_input();
        input.chrome_focus = Some(true);
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::FocusHorizontalChrome { right: true }
        );
        input.is_pressed = false;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ConsumeRelease
        );
    }

    #[test]
    fn early_dispatch_chrome_resize_routes_press_and_swallows_release() {
        let mut input = empty_early_input();
        input.chrome_resize = Some(false);
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ResizeFocusedChromeOrSplit { grow: false }
        );
        input.is_pressed = false;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ConsumeRelease
        );
    }

    #[test]
    fn early_dispatch_workspace_tab_branches_carry_shift() {
        let mut input = empty_early_input();
        input.is_top_level_workspace_tab_switch = true;
        input.shift = true;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::SelectTopLevelWorkspace { shift: true }
        );

        let mut input = empty_early_input();
        input.is_workspace_buffer_tab_switch = true;
        input.shift = false;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::SelectWorkspaceBufferTab { shift: false }
        );
    }

    #[test]
    fn early_dispatch_insert_branches_only_fire_on_press() {
        let mut input = empty_early_input();
        input.is_control_insert = true;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ControlInsert
        );
        input.is_pressed = false;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ConsumeRelease
        );

        let mut input = empty_early_input();
        input.is_shift_insert = true;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ShiftInsert
        );
    }

    #[test]
    fn early_dispatch_split_stack_toggle_requires_unlock() {
        let mut input = empty_early_input();
        input.is_split_stack_toggle = true;
        // Without unlock, branch is skipped — falls through.
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::PassThrough
        );
        input.split_stack_toggle_unlocked = true;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::ToggleSplitStackFocus
        );
    }

    #[test]
    fn early_dispatch_split_stack_auto_tab() {
        let mut input = empty_early_input();
        input.is_split_stack_auto_tab = true;
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::MoveActiveTabToSplitStack
        );
    }

    #[test]
    fn early_dispatch_precedence_chrome_focus_before_resize() {
        // If both predicates somehow matched (shouldn't happen in
        // practice), focus wins — matches the order of the original
        // branches in process_key_event.
        let mut input = empty_early_input();
        input.chrome_focus = Some(false);
        input.chrome_resize = Some(true);
        assert_eq!(
            early_key_event_dispatch(input),
            EarlyKeyDispatchAction::FocusHorizontalChrome { right: false }
        );
    }

    // ----- mid_key_event_dispatch -------------------------------------------

    #[test]
    fn mid_dispatch_pass_through_when_no_gate_set() {
        assert_eq!(
            mid_key_event_dispatch(MidKeyDispatchInput::default()),
            MidKeyDispatchAction::PassThrough
        );
    }

    #[test]
    fn mid_dispatch_routes_search_first() {
        let mut input = MidKeyDispatchInput::default();
        input.search_active = true;
        // Even when other gates are set, search wins.
        input.code_active = true;
        input.markdown_active = true;
        input.vi_mode = true;
        assert_eq!(
            mid_key_event_dispatch(input),
            MidKeyDispatchAction::RouteToSearch
        );
    }

    #[test]
    fn mid_dispatch_code_outranks_markdown_and_vi() {
        let mut input = MidKeyDispatchInput::default();
        input.code_active = true;
        input.markdown_active = true;
        input.vi_mode = true;
        assert_eq!(
            mid_key_event_dispatch(input),
            MidKeyDispatchAction::RouteToCode
        );
    }

    #[test]
    fn mid_dispatch_markdown_outranks_vi() {
        let mut input = MidKeyDispatchInput::default();
        input.markdown_active = true;
        input.vi_mode = true;
        assert_eq!(
            mid_key_event_dispatch(input),
            MidKeyDispatchAction::RouteToMarkdown
        );
    }

    #[test]
    fn mid_dispatch_vi_mode_consumes_when_alone() {
        let mut input = MidKeyDispatchInput::default();
        input.vi_mode = true;
        assert_eq!(
            mid_key_event_dispatch(input),
            MidKeyDispatchAction::ConsumeViMode
        );
    }

    // ----- hint_keystroke_result_action ------------------------------------

    #[test]
    fn hint_keystroke_result_some_executes_and_stops() {
        assert_eq!(
            hint_keystroke_result_action(true),
            HintKeystrokeResultAction::ExecuteAndStop
        );
    }

    #[test]
    fn hint_keystroke_result_none_continues() {
        assert_eq!(
            hint_keystroke_result_action(false),
            HintKeystrokeResultAction::Continue
        );
    }

    // ----- terminal_block_input_gate ---------------------------------------

    #[test]
    fn terminal_block_gate_consumes_when_block_handler_fires() {
        let input = TerminalBlockInputGateInput {
            search_active: false,
            vi_mode: false,
            block_consumed: true,
        };
        assert_eq!(
            terminal_block_input_gate(input),
            TerminalBlockInputGateAction::Consume
        );
    }

    #[test]
    fn terminal_block_gate_passes_when_block_handler_no_op() {
        let input = TerminalBlockInputGateInput {
            search_active: false,
            vi_mode: false,
            block_consumed: false,
        };
        assert_eq!(
            terminal_block_input_gate(input),
            TerminalBlockInputGateAction::PassThrough
        );
    }

    #[test]
    fn terminal_block_gate_search_active_short_circuits() {
        let input = TerminalBlockInputGateInput {
            search_active: true,
            vi_mode: false,
            block_consumed: true,
        };
        assert_eq!(
            terminal_block_input_gate(input),
            TerminalBlockInputGateAction::PassThrough
        );
    }

    #[test]
    fn terminal_block_gate_vi_mode_short_circuits() {
        let input = TerminalBlockInputGateInput {
            search_active: false,
            vi_mode: true,
            block_consumed: true,
        };
        assert_eq!(
            terminal_block_input_gate(input),
            TerminalBlockInputGateAction::PassThrough
        );
    }

    // ----- build_non_kitty_terminal_bytes ----------------------------------

    #[test]
    fn build_non_kitty_terminal_bytes_no_alt_passthrough() {
        let bytes = build_non_kitty_terminal_bytes("ab", false);
        assert_eq!(bytes, b"ab");
    }

    #[test]
    fn build_non_kitty_terminal_bytes_alt_prefixes_esc() {
        let bytes = build_non_kitty_terminal_bytes("x", true);
        assert_eq!(bytes, b"\x1bx");
    }

    #[test]
    fn build_non_kitty_terminal_bytes_empty_text_no_alt() {
        let bytes = build_non_kitty_terminal_bytes("", false);
        assert!(bytes.is_empty());
    }

    #[test]
    fn build_non_kitty_terminal_bytes_empty_text_with_alt_still_emits_esc() {
        let bytes = build_non_kitty_terminal_bytes("", true);
        assert_eq!(bytes, b"\x1b");
    }

    // ----- terminal_output_dispatch ----------------------------------------

    #[test]
    fn terminal_output_dispatch_empty_skips() {
        let input = TerminalOutputDispatchInput { bytes_empty: true };
        assert_eq!(
            terminal_output_dispatch(input),
            TerminalOutputDispatchAction::SkipEmpty
        );
    }

    #[test]
    fn terminal_output_dispatch_sends_to_pty_by_default() {
        let input = TerminalOutputDispatchInput { bytes_empty: false };
        assert_eq!(
            terminal_output_dispatch(input),
            TerminalOutputDispatchAction::SendToPty
        );
    }

    // ----- key_release_dispatch --------------------------------------------

    fn release_input(report_event_types: bool) -> KeyReleaseDispatchInput {
        KeyReleaseDispatchInput {
            report_event_types,
            ..Default::default()
        }
    }

    #[test]
    fn key_release_dispatch_suppresses_without_report_event_types() {
        // The default `report_event_types: false` already trips the
        // suppression gate even when every other flag is clear.
        assert_eq!(
            key_release_dispatch(release_input(false)),
            KeyReleaseDispatchAction::Suppress
        );
    }

    #[test]
    fn key_release_dispatch_suppresses_under_each_gate() {
        for (label, mutator) in [
            (
                "vi_mode",
                Box::new(|i: &mut KeyReleaseDispatchInput| i.vi_mode = true)
                    as Box<dyn Fn(&mut KeyReleaseDispatchInput)>,
            ),
            (
                "search_active",
                Box::new(|i: &mut KeyReleaseDispatchInput| i.search_active = true),
            ),
            (
                "hint_active",
                Box::new(|i: &mut KeyReleaseDispatchInput| i.hint_active = true),
            ),
        ] {
            let mut input = release_input(true);
            mutator(&mut input);
            assert_eq!(
                key_release_dispatch(input),
                KeyReleaseDispatchAction::Suppress,
                "expected Suppress when {label} flips on",
            );
        }
    }

    #[test]
    fn key_release_dispatch_drops_unreportable_named_keys() {
        // Enter / Tab / Backspace released without
        // REPORT_ALL_KEYS_AS_ESC → DropUnreportableNamed.
        let mut input = release_input(true);
        input.is_enter_tab_or_backspace = true;
        assert_eq!(
            key_release_dispatch(input),
            KeyReleaseDispatchAction::DropUnreportableNamed
        );
    }

    #[test]
    fn key_release_dispatch_emits_named_with_report_all_keys_as_esc() {
        let mut input = release_input(true);
        input.is_enter_tab_or_backspace = true;
        input.report_all_keys_as_esc = true;
        assert_eq!(
            key_release_dispatch(input),
            KeyReleaseDispatchAction::EmitSequence
        );
    }

    #[test]
    fn key_release_dispatch_emits_default_when_all_gates_clear() {
        // report_event_types on, no special-name key, no suppression
        // gate → emit the release sequence to the PTY.
        let input = release_input(true);
        assert_eq!(
            key_release_dispatch(input),
            KeyReleaseDispatchAction::EmitSequence
        );
    }

    #[test]
    fn key_release_dispatch_suppression_outranks_named_drop() {
        // Even when the key is an unreportable named release, the
        // suppression gate fires first (matches the order of the
        // original two `if` checks in process_key_event).
        let mut input = release_input(false);
        input.is_enter_tab_or_backspace = true;
        assert_eq!(
            key_release_dispatch(input),
            KeyReleaseDispatchAction::Suppress
        );
    }
}
