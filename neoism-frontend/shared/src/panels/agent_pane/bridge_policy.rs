//! Host-neutral agent bridge event mapping.
//!
//! This module translates portable key snapshots into agent-pane intents.
//! Hosts still own native event adaptation, clipboard IO, route changes, and
//! other platform glue.

use crate::widgets::modal::{ModalAction, ModalButton, ModalSpec};
use neoism_protocol::ide_tools::AgentInstallSpec;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentBridgeModifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub super_key: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentBridgeElementState {
    Pressed,
    Released,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentBridgeNamedKey {
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    Backspace,
    End,
    Enter,
    Escape,
    Home,
    Insert,
    Paste,
    Tab,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentBridgeKey {
    Named(AgentBridgeNamedKey),
    Character(String),
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentBridgePhysicalKey {
    Insert,
    KeyD,
    KeyU,
    KeyV,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentBridgeKeyEvent {
    pub state: AgentBridgeElementState,
    pub logical_key: AgentBridgeKey,
    pub key_without_modifiers: AgentBridgeKey,
    pub physical_key: Option<AgentBridgePhysicalKey>,
    pub text: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AgentKeyContext {
    pub side_panel_focused: bool,
    pub pending_permission: bool,
    /// A `question` tool request is awaiting an answer — the prompt
    /// picker owns the keyboard (arrows pick, typing filters/free-
    /// answers, Enter commits, Esc rejects).
    pub pending_question: bool,
    pub picker_open: bool,
    /// The open picker is the `/sessions` picker — enables the
    /// pin/delete/rename shortcuts (Ctrl+F / Ctrl+D / Ctrl+R).
    pub session_picker_open: bool,
    /// An inline session rename is being edited — every key routes to
    /// the rename buffer until Enter commits or Esc cancels.
    pub session_rename_active: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentPermissionReply {
    Once,
    Always,
    Reject,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AgentKeyIntent {
    Backspace,
    /// Ctrl+R on the `/sessions` picker — start the inline rename of
    /// the selected session.
    BeginSelectedSessionRename,
    ClearOrAbort,
    ClosePicker,
    /// Ctrl+D on the `/sessions` picker — delete the selected session.
    DeleteSelectedSession,
    InsertNewline,
    InsertText(String),
    MoveInputDownOrHistory,
    MoveInputEnd,
    MoveInputHome,
    MoveInputLeft,
    MoveInputRight,
    MoveInputUpOrHistory,
    MovePermissionSelection(isize),
    MovePickerSelection(isize),
    /// Arrows / Tab while a `question` prompt is pending.
    MoveQuestionSelection(isize),
    Paste,
    /// Backspace into the question prompt's typed filter/answer.
    QuestionBackspace,
    /// Typed characters filter the question's options and double as
    /// the free-typed answer.
    QuestionInput(String),
    /// Esc while a `question` prompt is pending — reject it so the
    /// parked run resumes instead of waiting forever.
    RejectPendingQuestion,
    RespondPendingPermission(AgentPermissionReply),
    /// Backspace in the inline session-rename buffer.
    SessionRenameBackspace,
    /// Esc — abandon the inline session rename.
    SessionRenameCancel,
    /// Enter — commit the inline session rename (emits `SetTitle`).
    SessionRenameCommit,
    /// Typed characters append to the inline session-rename buffer.
    SessionRenameInput(String),
    SidePanelActivateSelection,
    SidePanelBlur,
    SidePanelSelectNext,
    SidePanelSelectPrev,
    ScrollTimelineHalfPageDown,
    ScrollTimelineHalfPageUp,
    Submit,
    SubmitPendingPermission,
    /// Enter while a `question` prompt is pending — commit the
    /// selected row (or typed answer) for the current question.
    SubmitPendingQuestion,
    ToggleMode,
    /// Ctrl+F on the `/sessions` picker — pin/unpin the selected
    /// session.
    ToggleSelectedSessionPin,
    ToggleSidePanel,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AgentKeyDecision {
    pub handled: bool,
    pub dirty: bool,
    pub reapply_chrome_layout: bool,
    pub intents: Vec<AgentKeyIntent>,
}

impl AgentKeyDecision {
    pub fn passthrough() -> Self {
        Self::default()
    }

    pub fn swallow() -> Self {
        Self {
            handled: true,
            ..Self::default()
        }
    }

    fn dirty_intents(intents: Vec<AgentKeyIntent>) -> Self {
        Self {
            handled: true,
            dirty: true,
            intents,
            ..Self::default()
        }
    }

    fn dirty_relayout(intent: AgentKeyIntent) -> Self {
        Self {
            handled: true,
            dirty: true,
            reapply_chrome_layout: true,
            intents: vec![intent],
        }
    }
}

pub fn agent_key_decision(
    event: &AgentBridgeKeyEvent,
    mods: AgentBridgeModifiers,
    ctx: AgentKeyContext,
) -> AgentKeyDecision {
    if is_arrow_left_or_right(&event.logical_key) {
        let tab_switch = mods.control && mods.shift && !mods.alt && !mods.super_key;
        let tab_move = mods.alt && mods.shift && !mods.control && !mods.super_key;
        if tab_switch || tab_move {
            return AgentKeyDecision::passthrough();
        }
    }

    if event.state == AgentBridgeElementState::Released {
        return AgentKeyDecision::swallow();
    }

    if is_paste_key(event, mods) {
        return AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::Paste]);
    }

    // Session picker: inline-rename editing + pin/delete/rename
    // shortcuts. Handled ahead of the Ctrl+D history-scroll binding
    // below so Ctrl+D deletes the selected session while the picker is
    // open (mirrors desktop `handle_neoism_agent_key`).
    if ctx.session_picker_open {
        if ctx.session_rename_active {
            return session_rename_key_decision(event, mods);
        }
        if mods.control && !mods.alt && !mods.super_key && !mods.shift {
            if let AgentBridgeKey::Character(ch) = &event.key_without_modifiers {
                let intent = match ch.to_ascii_lowercase().as_str() {
                    "f" => Some(AgentKeyIntent::ToggleSelectedSessionPin),
                    "d" => Some(AgentKeyIntent::DeleteSelectedSession),
                    "r" => Some(AgentKeyIntent::BeginSelectedSessionRename),
                    _ => None,
                };
                if let Some(intent) = intent {
                    return AgentKeyDecision::dirty_intents(vec![intent]);
                }
            }
        }
    }

    if mods.alt
        && !mods.control
        && !mods.shift
        && !mods.super_key
        && character_eq_ignore_ascii_case(&event.logical_key, "h")
    {
        return AgentKeyDecision::dirty_relayout(AgentKeyIntent::ToggleSidePanel);
    }

    if ctx.side_panel_focused {
        return side_panel_key_decision(event, mods);
    }

    if mods.control
        && !mods.alt
        && !mods.super_key
        && character_eq_ignore_ascii_case(&event.key_without_modifiers, "c")
    {
        return AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::ClearOrAbort]);
    }

    if ctx.pending_permission {
        return permission_key_decision(event, mods, ctx.picker_open);
    }

    // Model question prompt (the `question` tool). Same modal
    // precedence as permissions: arrows pick an option, typing
    // filters / free-answers into the prompt picker's search row,
    // Enter commits, Esc rejects so the run resumes.
    if ctx.pending_question {
        return question_key_decision(event, mods, ctx.picker_open);
    }

    if mods.control && !mods.alt && !mods.super_key && !mods.shift {
        match ctrl_u_d_history_direction(event) {
            Some(true) => {
                return AgentKeyDecision::dirty_intents(vec![
                    AgentKeyIntent::ScrollTimelineHalfPageUp,
                ]);
            }
            Some(false) => {
                return AgentKeyDecision::dirty_intents(vec![
                    AgentKeyIntent::ScrollTimelineHalfPageDown,
                ]);
            }
            None => {}
        }
    }

    match event.logical_key {
        AgentBridgeKey::Named(AgentBridgeNamedKey::Enter) => {
            if mods.shift && !ctx.picker_open {
                AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::InsertNewline])
            } else {
                AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::Submit])
            }
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Tab)
            if !mods.control && !mods.alt && !mods.super_key =>
        {
            if ctx.picker_open {
                let delta = if mods.shift { -1 } else { 1 };
                AgentKeyDecision::dirty_intents(vec![
                    AgentKeyIntent::MovePickerSelection(delta),
                ])
            } else {
                AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::ToggleMode])
            }
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Tab) => {
            AgentKeyDecision::passthrough()
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowDown) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputDownOrHistory])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowUp) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputUpOrHistory])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowLeft) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputLeft])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowRight) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputRight])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Home) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputHome])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::End) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::MoveInputEnd])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Backspace) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::Backspace])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Escape) => {
            let intent = if ctx.picker_open {
                AgentKeyIntent::ClosePicker
            } else {
                AgentKeyIntent::ClearOrAbort
            };
            AgentKeyDecision::dirty_intents(vec![intent])
        }
        _ if mods.control || mods.alt || mods.super_key => {
            AgentKeyDecision::passthrough()
        }
        _ => {
            if !event.text.is_empty() && !event.text.chars().any(char::is_control) {
                AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::InsertText(
                    event.text.clone(),
                )])
            } else {
                AgentKeyDecision::swallow()
            }
        }
    }
}

fn side_panel_key_decision(
    event: &AgentBridgeKeyEvent,
    mods: AgentBridgeModifiers,
) -> AgentKeyDecision {
    if mods.alt || mods.control || mods.super_key {
        return AgentKeyDecision::passthrough();
    }
    match event.logical_key {
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowDown) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::SidePanelSelectNext])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowUp) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::SidePanelSelectPrev])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Enter) => {
            AgentKeyDecision::dirty_intents(vec![
                AgentKeyIntent::SidePanelActivateSelection,
            ])
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Escape) => {
            AgentKeyDecision::dirty_intents(vec![AgentKeyIntent::SidePanelBlur])
        }
        _ => AgentKeyDecision::swallow(),
    }
}

fn permission_key_decision(
    event: &AgentBridgeKeyEvent,
    mods: AgentBridgeModifiers,
    picker_open: bool,
) -> AgentKeyDecision {
    let mut intents = Vec::new();
    if picker_open {
        intents.push(AgentKeyIntent::ClosePicker);
    }

    match event.logical_key {
        AgentBridgeKey::Named(AgentBridgeNamedKey::Enter) => {
            intents.push(AgentKeyIntent::SubmitPendingPermission);
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowDown)
        | AgentBridgeKey::Named(AgentBridgeNamedKey::Tab) => {
            let delta = if mods.shift { -1 } else { 1 };
            intents.push(AgentKeyIntent::MovePermissionSelection(delta));
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowUp) => {
            intents.push(AgentKeyIntent::MovePermissionSelection(-1));
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Escape) => {
            intents.push(AgentKeyIntent::RespondPendingPermission(
                AgentPermissionReply::Reject,
            ));
        }
        AgentBridgeKey::Character(ref text) => {
            if let Some(ch) = text.chars().next().map(|ch| ch.to_ascii_lowercase()) {
                let reply = match ch {
                    'y' => Some(AgentPermissionReply::Once),
                    'a' => Some(AgentPermissionReply::Always),
                    'n' => Some(AgentPermissionReply::Reject),
                    _ => None,
                };
                if let Some(reply) = reply {
                    intents.push(AgentKeyIntent::RespondPendingPermission(reply));
                }
            }
        }
        _ => {}
    }

    AgentKeyDecision {
        handled: true,
        dirty: true,
        intents,
        ..AgentKeyDecision::default()
    }
}

/// Keys while a `question` prompt is pending. Mirrors the desktop key
/// bridge's question block (`bridges/agent.rs`): a lingering picker is
/// closed first, then Enter commits, arrows/Tab move, Esc rejects,
/// Backspace edits, and plain typing filters / free-answers.
fn question_key_decision(
    event: &AgentBridgeKeyEvent,
    mods: AgentBridgeModifiers,
    picker_open: bool,
) -> AgentKeyDecision {
    let mut intents = Vec::new();
    if picker_open {
        intents.push(AgentKeyIntent::ClosePicker);
    }

    match event.logical_key {
        AgentBridgeKey::Named(AgentBridgeNamedKey::Enter) => {
            intents.push(AgentKeyIntent::SubmitPendingQuestion);
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowDown)
        | AgentBridgeKey::Named(AgentBridgeNamedKey::Tab) => {
            let delta = if mods.shift { -1 } else { 1 };
            intents.push(AgentKeyIntent::MoveQuestionSelection(delta));
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowUp) => {
            intents.push(AgentKeyIntent::MoveQuestionSelection(-1));
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Escape) => {
            intents.push(AgentKeyIntent::RejectPendingQuestion);
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Backspace) => {
            intents.push(AgentKeyIntent::QuestionBackspace);
        }
        AgentBridgeKey::Character(_) if !mods.control && !mods.alt && !mods.super_key => {
            if !event.text.is_empty() && !event.text.chars().any(char::is_control) {
                intents.push(AgentKeyIntent::QuestionInput(event.text.clone()));
            }
        }
        _ => {}
    }

    AgentKeyDecision {
        handled: true,
        dirty: true,
        intents,
        ..AgentKeyDecision::default()
    }
}

/// Keys while an inline session rename is being edited on the
/// `/sessions` picker: Enter commits, Esc cancels, Backspace edits,
/// unmodified typing appends. Everything else is swallowed so it never
/// leaks into the composer behind the picker.
fn session_rename_key_decision(
    event: &AgentBridgeKeyEvent,
    mods: AgentBridgeModifiers,
) -> AgentKeyDecision {
    let mut intents = Vec::new();
    match event.logical_key {
        AgentBridgeKey::Named(AgentBridgeNamedKey::Enter) => {
            intents.push(AgentKeyIntent::SessionRenameCommit);
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Escape) => {
            intents.push(AgentKeyIntent::SessionRenameCancel);
        }
        AgentBridgeKey::Named(AgentBridgeNamedKey::Backspace) => {
            intents.push(AgentKeyIntent::SessionRenameBackspace);
        }
        _ => {
            if !mods.control
                && !mods.alt
                && !mods.super_key
                && !event.text.is_empty()
                && !event.text.chars().any(char::is_control)
            {
                intents.push(AgentKeyIntent::SessionRenameInput(event.text.clone()));
            }
        }
    }

    AgentKeyDecision {
        handled: true,
        dirty: true,
        intents,
        ..AgentKeyDecision::default()
    }
}

pub fn is_paste_key(event: &AgentBridgeKeyEvent, mods: AgentBridgeModifiers) -> bool {
    if matches!(
        event.logical_key,
        AgentBridgeKey::Named(AgentBridgeNamedKey::Paste)
    ) {
        return true;
    }

    if event.physical_key == Some(AgentBridgePhysicalKey::KeyV)
        && !mods.alt
        && ((mods.control && !mods.super_key) || (mods.super_key && !mods.control))
    {
        return true;
    }

    mods.shift
        && !mods.control
        && !mods.alt
        && !mods.super_key
        && (matches!(
            event.logical_key,
            AgentBridgeKey::Named(AgentBridgeNamedKey::Insert)
        ) || event.physical_key == Some(AgentBridgePhysicalKey::Insert))
}

fn is_arrow_left_or_right(key: &AgentBridgeKey) -> bool {
    matches!(
        key,
        AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowLeft)
            | AgentBridgeKey::Named(AgentBridgeNamedKey::ArrowRight)
    )
}

fn ctrl_u_d_history_direction(event: &AgentBridgeKeyEvent) -> Option<bool> {
    if character_eq_ignore_ascii_case(&event.key_without_modifiers, "u") {
        return Some(true);
    }
    if character_eq_ignore_ascii_case(&event.key_without_modifiers, "d") {
        return Some(false);
    }
    match event.physical_key {
        Some(AgentBridgePhysicalKey::KeyU) => Some(true),
        Some(AgentBridgePhysicalKey::KeyD) => Some(false),
        _ => None,
    }
}

fn character_eq_ignore_ascii_case(key: &AgentBridgeKey, expected: &str) -> bool {
    matches!(key, AgentBridgeKey::Character(ch) if ch.eq_ignore_ascii_case(expected))
}

/// Modal copy/buttons shown while an agent CLI install is running.
/// Host wraps this around the actual install thread spawn.
pub fn agent_install_modal_spec(spec: &AgentInstallSpec) -> ModalSpec {
    ModalSpec {
        title: format!("Installing {}", spec.display_name),
        body: format!(
            "Neoism is installing `{}` using {}. Once the binary is on PATH the new terminal tab will launch it.",
            spec.binary, spec.manager
        ),
        meta: "This can take a moment.".to_string(),
        input: None,
        buttons: vec![ModalButton::new("Dismiss", "Esc", ModalAction::Close)],
        busy: true,
        blocking: false,
    }
}

/// Modal shown when the requested agent has no installer registered
/// — copy mirrors the prior hard-coded string in
/// `Screen::start_agent_install`.
pub fn agent_no_installer_modal(display_name: &str) -> (String, String) {
    (
        "No Installer".to_string(),
        format!("Neoism does not know how to install {display_name} yet."),
    )
}

/// Format the command line written into the workspace terminal when
/// launching `binary` with optional `args` (trimmed). Always ends with
/// `\n` so the terminal runs the command. Pure formatting policy.
pub fn agent_launch_command_line(binary: &str, args: &str) -> String {
    let args = args.trim();
    if args.is_empty() {
        format!("{binary}\n")
    } else {
        format!("{binary} {args}\n")
    }
}

/// Dispatch decision for an agent-pane link click. The host already
/// resolved a filesystem path; the policy here just classifies which
/// open path to take. Web-style URLs short-circuit to `Ignore` so the
/// caller does not have to repeat the prefix check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentLinkOpenAction {
    /// `target` was an `http(s)://` URL — agent pane currently ignores
    /// those (kept as a named variant so callers can re-attach a real
    /// open path later without revisiting the dispatch site).
    Ignore,
    /// Resolved path is a directory — reveal it in the file tree.
    OpenDirectoryInFileTree,
    /// Resolved path is a Markdown file — open it in the markdown view.
    OpenMarkdown,
    /// Anything else — open in the code editor.
    OpenEditor,
}

/// Classify the agent-pane link `target` once the host has resolved it.
/// `is_dir` and `is_markdown` describe the resolved path; both are
/// `false` when no path could be resolved (in which case the host has
/// already returned early before calling this).
pub fn agent_link_open_action(
    target: &str,
    is_dir: bool,
    is_markdown: bool,
) -> AgentLinkOpenAction {
    let target = target.trim();
    if target.starts_with("http://") || target.starts_with("https://") {
        return AgentLinkOpenAction::Ignore;
    }
    if is_dir {
        AgentLinkOpenAction::OpenDirectoryInFileTree
    } else if is_markdown {
        AgentLinkOpenAction::OpenMarkdown
    } else {
        AgentLinkOpenAction::OpenEditor
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn event(logical_key: AgentBridgeKey) -> AgentBridgeKeyEvent {
        AgentBridgeKeyEvent {
            state: AgentBridgeElementState::Pressed,
            key_without_modifiers: logical_key.clone(),
            logical_key,
            physical_key: None,
            text: String::new(),
        }
    }

    fn named(key: AgentBridgeNamedKey) -> AgentBridgeKeyEvent {
        event(AgentBridgeKey::Named(key))
    }

    fn character(value: &str, text: &str) -> AgentBridgeKeyEvent {
        AgentBridgeKeyEvent {
            state: AgentBridgeElementState::Pressed,
            logical_key: AgentBridgeKey::Character(value.to_string()),
            key_without_modifiers: AgentBridgeKey::Character(value.to_string()),
            physical_key: None,
            text: text.to_string(),
        }
    }

    #[test]
    fn side_panel_focus_swallow_plain_text_but_passes_global_modifiers() {
        let ctx = AgentKeyContext {
            side_panel_focused: true,
            ..AgentKeyContext::default()
        };

        assert_eq!(
            agent_key_decision(
                &character("x", "x"),
                AgentBridgeModifiers::default(),
                ctx
            ),
            AgentKeyDecision::swallow()
        );
        assert_eq!(
            agent_key_decision(
                &character("x", "x"),
                AgentBridgeModifiers {
                    alt: true,
                    ..AgentBridgeModifiers::default()
                },
                ctx
            ),
            AgentKeyDecision::passthrough()
        );
    }

    #[test]
    fn pending_permission_closes_picker_then_maps_letter_reply() {
        let decision = agent_key_decision(
            &character("a", "a"),
            AgentBridgeModifiers::default(),
            AgentKeyContext {
                pending_permission: true,
                picker_open: true,
                ..AgentKeyContext::default()
            },
        );

        assert!(decision.handled);
        assert!(decision.dirty);
        assert_eq!(
            decision.intents,
            vec![
                AgentKeyIntent::ClosePicker,
                AgentKeyIntent::RespondPendingPermission(AgentPermissionReply::Always),
            ]
        );
    }

    #[test]
    fn pending_question_routes_typing_arrows_enter_and_escape() {
        let ctx = AgentKeyContext {
            pending_question: true,
            ..AgentKeyContext::default()
        };

        assert_eq!(
            agent_key_decision(
                &character("a", "a"),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::QuestionInput("a".to_string())]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::ArrowDown),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::MoveQuestionSelection(1)]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Tab),
                AgentBridgeModifiers {
                    shift: true,
                    ..AgentBridgeModifiers::default()
                },
                ctx
            )
            .intents,
            vec![AgentKeyIntent::MoveQuestionSelection(-1)]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Enter),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::SubmitPendingQuestion]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Escape),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::RejectPendingQuestion]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Backspace),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::QuestionBackspace]
        );
    }

    #[test]
    fn pending_question_closes_picker_first_and_permission_takes_precedence() {
        let decision = agent_key_decision(
            &named(AgentBridgeNamedKey::Enter),
            AgentBridgeModifiers::default(),
            AgentKeyContext {
                pending_question: true,
                picker_open: true,
                ..AgentKeyContext::default()
            },
        );
        assert_eq!(
            decision.intents,
            vec![
                AgentKeyIntent::ClosePicker,
                AgentKeyIntent::SubmitPendingQuestion,
            ]
        );

        // A pending permission outranks a pending question — same modal
        // precedence as the desktop key bridge.
        let decision = agent_key_decision(
            &named(AgentBridgeNamedKey::Enter),
            AgentBridgeModifiers::default(),
            AgentKeyContext {
                pending_permission: true,
                pending_question: true,
                ..AgentKeyContext::default()
            },
        );
        assert_eq!(
            decision.intents,
            vec![AgentKeyIntent::SubmitPendingPermission]
        );
    }

    #[test]
    fn session_picker_ctrl_shortcuts_map_to_pin_delete_rename() {
        let ctx = AgentKeyContext {
            picker_open: true,
            session_picker_open: true,
            ..AgentKeyContext::default()
        };
        let ctrl = AgentBridgeModifiers {
            control: true,
            ..AgentBridgeModifiers::default()
        };

        assert_eq!(
            agent_key_decision(&character("f", ""), ctrl, ctx).intents,
            vec![AgentKeyIntent::ToggleSelectedSessionPin]
        );
        // Ctrl+D deletes the selected session instead of scrolling the
        // timeline while the /sessions picker is open.
        assert_eq!(
            agent_key_decision(&character("d", ""), ctrl, ctx).intents,
            vec![AgentKeyIntent::DeleteSelectedSession]
        );
        assert_eq!(
            agent_key_decision(&character("r", ""), ctrl, ctx).intents,
            vec![AgentKeyIntent::BeginSelectedSessionRename]
        );
        // Without the session picker, Ctrl+D keeps its scroll binding.
        assert_eq!(
            agent_key_decision(&character("d", ""), ctrl, AgentKeyContext::default())
                .intents,
            vec![AgentKeyIntent::ScrollTimelineHalfPageDown]
        );
    }

    #[test]
    fn session_rename_captures_typing_enter_escape_and_backspace() {
        let ctx = AgentKeyContext {
            picker_open: true,
            session_picker_open: true,
            session_rename_active: true,
            ..AgentKeyContext::default()
        };

        assert_eq!(
            agent_key_decision(
                &character("x", "x"),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::SessionRenameInput("x".to_string())]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Enter),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::SessionRenameCommit]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Escape),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::SessionRenameCancel]
        );
        assert_eq!(
            agent_key_decision(
                &named(AgentBridgeNamedKey::Backspace),
                AgentBridgeModifiers::default(),
                ctx
            )
            .intents,
            vec![AgentKeyIntent::SessionRenameBackspace]
        );
        // Modified keys are swallowed, not typed into the buffer.
        let decision = agent_key_decision(
            &character("c", ""),
            AgentBridgeModifiers {
                control: true,
                ..AgentBridgeModifiers::default()
            },
            ctx,
        );
        assert!(decision.handled);
        assert!(decision.intents.is_empty());
    }

    #[test]
    fn tab_switch_arrows_pass_through_before_normal_handling() {
        let decision = agent_key_decision(
            &named(AgentBridgeNamedKey::ArrowLeft),
            AgentBridgeModifiers {
                shift: true,
                control: true,
                ..AgentBridgeModifiers::default()
            },
            AgentKeyContext::default(),
        );

        assert_eq!(decision, AgentKeyDecision::passthrough());
    }

    #[test]
    fn modifier_shortcuts_pass_through_after_agent_specific_shortcuts() {
        let paste = AgentBridgeKeyEvent {
            physical_key: Some(AgentBridgePhysicalKey::KeyV),
            ..character("v", "")
        };
        assert_eq!(
            agent_key_decision(
                &paste,
                AgentBridgeModifiers {
                    control: true,
                    ..AgentBridgeModifiers::default()
                },
                AgentKeyContext::default()
            )
            .intents,
            vec![AgentKeyIntent::Paste]
        );

        assert_eq!(
            agent_key_decision(
                &character("p", ""),
                AgentBridgeModifiers {
                    alt: true,
                    ..AgentBridgeModifiers::default()
                },
                AgentKeyContext::default()
            ),
            AgentKeyDecision::passthrough()
        );
    }

    #[test]
    fn ctrl_u_and_d_scroll_agent_timeline_history() {
        assert_eq!(
            agent_key_decision(
                &character("u", ""),
                AgentBridgeModifiers {
                    control: true,
                    ..AgentBridgeModifiers::default()
                },
                AgentKeyContext::default()
            )
            .intents,
            vec![AgentKeyIntent::ScrollTimelineHalfPageUp]
        );

        assert_eq!(
            agent_key_decision(
                &character("d", ""),
                AgentBridgeModifiers {
                    control: true,
                    ..AgentBridgeModifiers::default()
                },
                AgentKeyContext::default()
            )
            .intents,
            vec![AgentKeyIntent::ScrollTimelineHalfPageDown]
        );

        assert_eq!(
            agent_key_decision(
                &character("u", ""),
                AgentBridgeModifiers {
                    control: true,
                    shift: true,
                    ..AgentBridgeModifiers::default()
                },
                AgentKeyContext::default()
            ),
            AgentKeyDecision::passthrough()
        );
    }

    #[test]
    fn launch_command_line_trims_args_and_terminates_with_newline() {
        assert_eq!(agent_launch_command_line("claude", ""), "claude\n");
        assert_eq!(agent_launch_command_line("claude", "   "), "claude\n");
        assert_eq!(
            agent_launch_command_line("claude", "  --model x  "),
            "claude --model x\n"
        );
    }

    #[test]
    fn no_installer_modal_uses_display_name() {
        let (title, body) = agent_no_installer_modal("OpenCode");
        assert_eq!(title, "No Installer");
        assert!(body.contains("OpenCode"));
    }

    #[test]
    fn install_modal_spec_uses_busy_dismiss_button() {
        use neoism_protocol::ide_tools::{AgentInstallMethod, AgentInstallSpec};
        let spec = AgentInstallSpec {
            id: "claude",
            binary: "claude",
            display_name: "Claude Code",
            manager: "npm",
            method: AgentInstallMethod::NpmGlobal {
                package: "@example/claude",
            },
        };
        let modal = agent_install_modal_spec(&spec);
        assert!(modal.busy);
        assert!(!modal.blocking);
        assert_eq!(modal.buttons.len(), 1);
        assert!(modal.title.contains("Claude Code"));
        assert!(modal.body.contains("`claude`"));
        assert!(modal.body.contains("npm"));
    }

    #[test]
    fn link_open_action_ignores_web_urls() {
        assert_eq!(
            agent_link_open_action("https://example.com", false, false),
            AgentLinkOpenAction::Ignore
        );
        assert_eq!(
            agent_link_open_action("  http://example.com", false, false),
            AgentLinkOpenAction::Ignore
        );
    }

    #[test]
    fn link_open_action_dispatches_by_resolved_path_kind() {
        assert_eq!(
            agent_link_open_action("docs", true, false),
            AgentLinkOpenAction::OpenDirectoryInFileTree
        );
        assert_eq!(
            agent_link_open_action("notes/readme.md", false, true),
            AgentLinkOpenAction::OpenMarkdown
        );
        assert_eq!(
            agent_link_open_action("src/main.rs", false, false),
            AgentLinkOpenAction::OpenEditor
        );
    }

    #[test]
    fn neoism_tab_move_to_workspace_attaches() {
        let plan = neoism_agent_tab_move_plan(42, AgentTabDestination::Workspace);
        assert_eq!(plan, AgentTabMovePlan::AttachWorkspace { route_id: 42 });
    }

    #[test]
    fn neoism_tab_move_to_same_route_rejected() {
        let plan = neoism_agent_tab_move_plan(7, AgentTabDestination::Pane(7));
        assert_eq!(plan, AgentTabMovePlan::RejectSamePane);
    }

    #[test]
    fn neoism_tab_move_to_different_pane_stacks() {
        let plan = neoism_agent_tab_move_plan(3, AgentTabDestination::Pane(9));
        assert_eq!(
            plan,
            AgentTabMovePlan::StackOnPane {
                route_id: 3,
                dest_route: 9
            }
        );
    }

    #[test]
    fn agent_tab_move_failure_uses_title() {
        let msg = agent_tab_move_failure_message("Claude Code");
        assert!(msg.contains("Claude Code"));
        assert!(msg.contains("split"));
    }

    #[test]
    fn neoism_agent_tab_move_failure_uses_fixed_copy() {
        let msg = neoism_agent_tab_move_failure_message();
        assert!(msg.contains("Neoism Agent"));
    }

    #[test]
    fn neoism_agent_tear_out_failure_uses_fixed_copy() {
        let msg = neoism_agent_tear_out_failure_message();
        assert!(msg.contains("Neoism Agent"));
    }

    #[test]
    fn agent_tear_out_failure_uses_title() {
        let msg = agent_tear_out_failure_message("Codex");
        assert!(msg.contains("Codex"));
    }
}

// ---------------------------------------------------------------------------
// Tab strip move policy (mirrors markdown::bridge_policy::markdown_tab_move_plan).
// ---------------------------------------------------------------------------

/// Where an agent buffer tab currently lives, from the perspective of
/// the desktop `move_*_agent_tab_between_strips` calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTabSource {
    Workspace,
    Pane(usize),
}

/// Where an agent buffer tab is being moved to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTabDestination {
    Workspace,
    Pane(usize),
}

/// What the host should attempt for the destination strip in an agent
/// tab move. Mirrors `markdown_tab_move_plan` but tracks the agent's
/// route_id directly (every agent tab already has one — there is no
/// "ensure pane route" branch).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentTabMovePlan {
    /// Stack `route_id` onto the workspace strip.
    AttachWorkspace { route_id: usize },
    /// Drop into a pane that is the same route the tab already lives
    /// on — the host should refuse, matching the desktop early-return
    /// in `move_agent_tab_between_strips`.
    RejectSamePane,
    /// Stack `route_id` onto `dest_route`.
    StackOnPane { route_id: usize, dest_route: usize },
}

/// Decide the destination action for an agent-pane buffer-tab move.
/// `route_id` is the tab's `terminal_route_id` — callers that lack
/// one have already short-circuited before reaching this policy.
pub fn agent_tab_move_plan(
    route_id: usize,
    dest: AgentTabDestination,
) -> AgentTabMovePlan {
    match dest {
        AgentTabDestination::Workspace => AgentTabMovePlan::AttachWorkspace { route_id },
        AgentTabDestination::Pane(dest_route) => {
            if dest_route == route_id {
                AgentTabMovePlan::RejectSamePane
            } else {
                AgentTabMovePlan::StackOnPane {
                    route_id,
                    dest_route,
                }
            }
        }
    }
}

/// Same shape as [`agent_tab_move_plan`] but used by the dedicated
/// `move_neoism_agent_tab_between_strips` codepath, which never
/// rejects on a same-route Pane (the desktop function unconditionally
/// stacks). Kept as a separate function so the two callers don't share
/// behaviour by accident.
pub fn neoism_agent_tab_move_plan(
    route_id: usize,
    dest: AgentTabDestination,
) -> AgentTabMovePlan {
    match dest {
        AgentTabDestination::Workspace => AgentTabMovePlan::AttachWorkspace { route_id },
        AgentTabDestination::Pane(dest_route) => {
            if dest_route == route_id {
                AgentTabMovePlan::RejectSamePane
            } else {
                AgentTabMovePlan::StackOnPane {
                    route_id,
                    dest_route,
                }
            }
        }
    }
}

/// User-facing warning when an agent tab move into a split fails.
pub fn agent_tab_move_failure_message(title: &str) -> String {
    format!("Could not move `{title}` into that split.")
}

/// Warning shown when the Neoism Agent strip move fails. Mirrors the
/// hard-coded copy in the desktop bridge.
pub fn neoism_agent_tab_move_failure_message() -> &'static str {
    "Could not move Neoism Agent into that split."
}

/// User-facing warning when an agent tab tear-out into a new split
/// fails.
pub fn agent_tear_out_failure_message(title: &str) -> String {
    format!("Could not tear out `{title}` to a split.")
}

/// Warning when the Neoism Agent tear-out fails. Mirrors the
/// hard-coded copy in `tear_out_neoism_agent_tab_to_split`.
pub fn neoism_agent_tear_out_failure_message() -> &'static str {
    "Could not tear out the Neoism Agent into a split."
}
