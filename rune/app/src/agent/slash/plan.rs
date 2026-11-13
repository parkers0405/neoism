//! What a `/` line means: the original command spelling, aliases and argument rules
//! (`shared/src/panels/agent_pane/command_controller.rs`), and the catalog the `/` menu lists.

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SlashAction {
    Noop,
    ShowHelp,
    ApplyModel(String),
    OpenModelPicker,
    OpenConnectPicker,
    OpenUsagePicker,
    OpenTasksPicker,
    OpenGoalsPicker,
    ApplyThinking(String),
    OpenThinkingPicker,
    ApplyAgent(String),
    OpenAgentPicker,
    SwitchSession(String),
    OpenSessionsPicker,
    ChangeDirectory(String),
    OpenDirectoryPicker,
    OpenSubagentPicker,
    ShowSkills,
    ShowSkill(String),
    ShowSkillUsage,
    InsertSkillMention(String),
    OpenSkillPicker,
    HandleQueue(Option<String>),
    ShowMcp,
    ShowPermissions,
    ShowQuestions,
    HandlePermit(Vec<String>),
    HandleAnswer(String),
    HandleReject(Option<String>),
    CompactSession,
    ShowGoal,
    SetGoal(String),
    ClearGoal,
    PauseGoal,
    ResumeGoal,
    UndoSession,
    RedoSession,
    ToggleInputHelp,
    ToggleSidebar,
    ToggleSkipPermissions,
    Effect(super::effects::Effect),
    AbortSession,
    CreateNewSession,
    RequestCloseTab,
    RunServerCommand { command: String, args: String },
}

/// Parse a composer line that starts with `/`.
pub fn plan(text: &str) -> SlashAction {
    use super::effects::Effect;
    use SlashAction::*;
    let mut parts = text.trim().split_whitespace();
    let raw = parts.next().unwrap_or_default();
    let args: Vec<String> = parts.map(str::to_owned).collect();
    let first = || args.first().cloned();
    match raw {
        "/help" => ShowHelp,
        "/model" | "/models" => first().map_or(OpenModelPicker, ApplyModel),
        "/connect" => OpenConnectPicker,
        "/usage" => OpenUsagePicker,
        "/tasks" => OpenTasksPicker,
        "/goals" => OpenGoalsPicker,
        "/think" | "/reasoning" => first().map_or(OpenThinkingPicker, ApplyThinking),
        "/agent" | "/agents" => first().map_or(OpenAgentPicker, ApplyAgent),
        "/sessions" | "/session" => first().map_or(OpenSessionsPicker, SwitchSession),
        "/cd" if args.is_empty() => OpenDirectoryPicker,
        "/cd" => ChangeDirectory(args.join(" ")),
        "/sub-agent" | "/subagents" | "/sub" => OpenSubagentPicker,
        "/skill" | "/skills" => match args.first().map(String::as_str) {
            None => OpenSkillPicker,
            Some("list") => ShowSkills,
            Some("info") => args.get(1).cloned().map_or(ShowSkillUsage, ShowSkill),
            Some(skill) => InsertSkillMention(skill.to_owned()),
        },
        "/queue" => HandleQueue(first()),
        "/mcp" | "/mcps" => ShowMcp,
        "/permissions" => ShowPermissions,
        "/questions" => ShowQuestions,
        "/permit" => HandlePermit(args),
        "/yolo" | "/dangerously-skip-permissions" | "/skip-permissions" => ToggleSkipPermissions,
        "/answer" => HandleAnswer(args.join(" ")),
        "/reject" | "/deny" => HandleReject(first()),
        "/compact" | "/compaction" | "/comapction" => CompactSession,
        "/goal" => match args.first().map(String::as_str) {
            None => ShowGoal,
            Some("clear") if args.len() == 1 => ClearGoal,
            Some("pause") if args.len() == 1 => PauseGoal,
            Some("resume") if args.len() == 1 => ResumeGoal,
            Some(_) => SetGoal(args.join(" ")),
        },
        "/undo" => UndoSession,
        "/redo" => RedoSession,
        "/hints" | "/helper" | "/help-strip" | "/footer" => ToggleInputHelp,
        "/sidebar" => ToggleSidebar,
        "/piss" => Effect(Effect::Piss),
        "/cuss" | "/swear" => Effect(Effect::Cuss),
        "/glitch" => Effect(Effect::Glitch),
        "/disco" | "/dance" => Effect(Effect::Disco),
        "/gangfight" | "/shootout" => Effect(Effect::GangFight),
        "/praise" | "/worship" | "/amen" => Effect(Effect::Praise),
        "/abort" => AbortSession,
        "/new" => CreateNewSession,
        "/exit" => RequestCloseTab,
        other if other.starts_with('/') => RunServerCommand { command: other.trim_start_matches('/').to_owned(), args: args.join(" ") },
        _ => Noop,
    }
}

/// One `/` menu row: command, what it does, and its kind tag on the right.
pub struct Command {
    pub title: &'static str,
    pub description: &'static str,
    pub footer: &'static str,
}

const fn cmd(title: &'static str, description: &'static str, footer: &'static str) -> Command {
    Command { title, description, footer }
}

/// The original menu, in order.
pub const COMMANDS: &[Command] = &[
    cmd("/help", "Show available commands", "docs"),
    cmd("/model", "Switch model", "modal"),
    cmd("/connect", "Connect a provider (OAuth or API key)", "modal"),
    cmd("/usage", "View Codex account usage and reset times", "modal"),
    cmd("/tasks", "View this conversation's task list", "modal"),
    cmd("/goals", "View the current goal and elapsed time", "modal"),
    cmd("/think", "Set reasoning effort", "modal"),
    cmd("/agent", "Switch agent", "modal"),
    cmd("/compact", "Compact session context", "server"),
    cmd("/cd", "Move this session to another directory", "picker"),
    cmd("/goal", "Show, set, pause, resume, or clear the session goal", "server"),
    cmd("/undo", "Revert the last session change", "session"),
    cmd("/redo", "Restore the reverted session change", "session"),
    cmd("/hints", "Toggle the helper row below the input", "ui"),
    cmd("/sidebar", "Toggle the Agent sidebar and its default", "ui"),
    cmd("/piss", "A tiny visitor waters your session (the model hears about it)", "fun"),
    cmd("/cuss", "The tiny visitor returns, furious (#$@!) \u{2014} the model hears about it", "fun"),
    cmd("/glitch", "He unplugs your session for a second \u{2014} the model remembers", "fun"),
    cmd("/disco", "Disco ball, beams, confetti \u{2014} he came to celebrate", "fun"),
    cmd("/gangfight", "Two crews, one corridor of tracer fire \u{2014} his side wins", "fun"),
    cmd("/praise", "Jesus on the throne, everyone bowing \u{2014} a moment of worship", "fun"),
    cmd("/sessions", "List sessions", "server"),
    cmd("/sub-agent", "List subagent sessions", "server"),
    cmd("/skills", "Search discovered skills", "server"),
    cmd("/mcp", "Manage MCP servers", "picker"),
    cmd("/queue", "Show queued prompts", "server"),
    cmd("/permissions", "Show pending permissions", "server"),
    cmd("/yolo", "Skip ALL permission prompts (dangerous) \u{2014} toggle", "danger"),
    cmd("/questions", "Show pending questions", "server"),
    cmd("/permit", "Reply to permission", "server"),
    cmd("/answer", "Answer pending question", "server"),
    cmd("/reject", "Reject question/permission", "server"),
    cmd("/abort", "Abort current run", "server"),
    cmd("/new", "Create a new session", "server"),
    cmd("/exit", "Close this agent tab", "ui"),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::slash::effects::Effect;

    #[test]
    fn aliases_and_arguments() {
        assert_eq!(plan("/models"), SlashAction::OpenModelPicker);
        assert_eq!(plan("/model openai/gpt-5"), SlashAction::ApplyModel("openai/gpt-5".into()));
        assert_eq!(plan("/reasoning high"), SlashAction::ApplyThinking("high".into()));
        assert_eq!(plan("/cd"), SlashAction::OpenDirectoryPicker);
        assert_eq!(plan("/cd ~/a b"), SlashAction::ChangeDirectory("~/a b".into()));
        assert_eq!(plan("/skill info x"), SlashAction::ShowSkill("x".into()));
        assert_eq!(plan("/skill info"), SlashAction::ShowSkillUsage);
        assert_eq!(plan("/goal pause"), SlashAction::PauseGoal);
        assert_eq!(plan("/goal ship it"), SlashAction::SetGoal("ship it".into()));
        assert_eq!(plan("/tasks"), SlashAction::OpenTasksPicker);
        assert_eq!(plan("/goals"), SlashAction::OpenGoalsPicker);
        assert_eq!(plan("/comapction"), SlashAction::CompactSession);
        assert_eq!(plan("/amen"), SlashAction::Effect(Effect::Praise));
        assert_eq!(plan("/review a b"), SlashAction::RunServerCommand { command: "review".into(), args: "a b".into() });
        assert_eq!(plan("hello"), SlashAction::Noop);
    }

    #[test]
    fn every_menu_row_plans_to_a_known_action() {
        for row in COMMANDS {
            assert!(!matches!(plan(row.title), SlashAction::RunServerCommand { .. } | SlashAction::Noop), "{}", row.title);
        }
        assert_eq!(COMMANDS.len(), 35);
    }
}
