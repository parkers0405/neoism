//! Shared command parsing for the agent pane.
//!
//! This module deliberately stops at planning. It never performs IO and
//! never mutates pane state directly; hosts execute the returned action
//! with their own runtime surface.

use super::state::picker::NeoismAgentPickerOption;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommandAction {
    Noop,
    ShowHelp,
    ApplyModel(String),
    OpenModelPicker,
    /// Open the "Connect a provider" flow (provider list → auth method →
    /// OAuth / API-key entry). Mirrors opencode's `auth login`, but in the GUI.
    OpenConnectPicker,
    ApplyThinking(String),
    OpenThinkingPicker,
    ApplyAgent(String),
    OpenAgentPicker,
    SwitchSession(String),
    OpenSessionsPicker,
    OpenSubagentPicker,
    ShowSkills,
    ShowSkill(String),
    ShowSkillUsage,
    InsertSkillMentionByName(String),
    OpenSkillPicker,
    HandleQueue(Option<String>),
    ShowMcp,
    ShowPermissions,
    ShowQuestions,
    HandlePermit(Vec<String>),
    HandleAnswer(String),
    HandleReject(Option<String>),
    CompactSession,
    /// Show the persistent session goal (`/goal` with no args).
    ShowGoal,
    /// Set the persistent session goal (`/goal <text>`).
    SetGoal(String),
    /// Clear the persistent session goal (`/goal clear`).
    ClearGoal,
    /// Pause active-goal autonomous continuation without deleting it.
    PauseGoal,
    /// Resume active-goal autonomous continuation.
    ResumeGoal,
    UndoSession,
    RedoSession,
    /// `/yolo` (`/dangerously-skip-permissions`) — toggle skipping ALL
    /// permission prompts for this pane: every request auto-answers
    /// "Yes" the moment it arrives. The config-level equivalent is
    /// `"dangerouslySkipPermissions": true`, which stops the server
    /// asking at all.
    ToggleSkipPermissions,
    /// `/piss` — the easter egg: a tiny pixel fella jogs across the
    /// pane, waters the timeline, and the model gets told about it.
    PissOnScreen,
    /// `/cuss` — the fella storms in and cusses the model out,
    /// grawlix bubble and all; the model hears how mad the user is.
    CussOnScreen,
    /// `/glitch` — he unplugs the pane for a second.
    GlitchOnScreen,
    /// `/disco` — disco ball, beams, confetti, moonwalk exit.
    DiscoOnScreen,
    /// `/gangfight` — cartoon crew shootout; the fella's crew wins.
    GangFightOnScreen,
    /// `/praise` — Jesus on the throne, worshipers bowing, notes
    /// rising; the model is invited to rejoice.
    PraiseOnScreen,
    AbortSession,
    CreateNewSession,
    RequestCloseTab,
    RunServerCommand {
        command: String,
        args: String,
    },
}

pub fn plan_slash_command(text: &str) -> SlashCommandAction {
    let mut parts = text.trim().split_whitespace();
    let raw = parts.next().unwrap_or_default();
    let args = parts.map(str::to_string).collect::<Vec<_>>();

    match raw {
        "/help" => SlashCommandAction::ShowHelp,
        "/model" => first_arg_or_picker(
            &args,
            SlashCommandAction::ApplyModel,
            SlashCommandAction::OpenModelPicker,
        ),
        "/connect" => SlashCommandAction::OpenConnectPicker,
        "/think" | "/reasoning" => first_arg_or_picker(
            &args,
            SlashCommandAction::ApplyThinking,
            SlashCommandAction::OpenThinkingPicker,
        ),
        "/agent" => first_arg_or_picker(
            &args,
            SlashCommandAction::ApplyAgent,
            SlashCommandAction::OpenAgentPicker,
        ),
        "/sessions" | "/session" => first_arg_or_picker(
            &args,
            SlashCommandAction::SwitchSession,
            SlashCommandAction::OpenSessionsPicker,
        ),
        "/sub-agent" | "/subagents" | "/sub" => SlashCommandAction::OpenSubagentPicker,
        "/skill" | "/skills" => plan_skill_command(&args),
        "/queue" => SlashCommandAction::HandleQueue(args.first().cloned()),
        "/mcp" | "/mcps" => SlashCommandAction::ShowMcp,
        "/permissions" => SlashCommandAction::ShowPermissions,
        "/questions" => SlashCommandAction::ShowQuestions,
        "/permit" => SlashCommandAction::HandlePermit(args),
        "/yolo" | "/dangerously-skip-permissions" | "/skip-permissions" => {
            SlashCommandAction::ToggleSkipPermissions
        }
        "/answer" => SlashCommandAction::HandleAnswer(args.join(" ")),
        "/reject" | "/deny" => SlashCommandAction::HandleReject(args.first().cloned()),
        "/compact" | "/compaction" | "/comapction" => SlashCommandAction::CompactSession,
        "/goal" => plan_goal_command(&args),
        "/undo" => SlashCommandAction::UndoSession,
        "/redo" => SlashCommandAction::RedoSession,
        "/piss" => SlashCommandAction::PissOnScreen,
        "/cuss" | "/swear" => SlashCommandAction::CussOnScreen,
        "/glitch" => SlashCommandAction::GlitchOnScreen,
        "/disco" | "/dance" => SlashCommandAction::DiscoOnScreen,
        "/gangfight" | "/shootout" => SlashCommandAction::GangFightOnScreen,
        "/praise" | "/worship" | "/amen" => SlashCommandAction::PraiseOnScreen,
        "/abort" => SlashCommandAction::AbortSession,
        "/new" => SlashCommandAction::CreateNewSession,
        "/exit" => SlashCommandAction::RequestCloseTab,
        other if other.starts_with('/') => SlashCommandAction::RunServerCommand {
            command: other.trim_start_matches('/').to_string(),
            args: args.join(" "),
        },
        _ => SlashCommandAction::Noop,
    }
}

pub fn slash_options() -> Vec<NeoismAgentPickerOption> {
    slash_option_specs()
        .iter()
        .map(|spec| {
            NeoismAgentPickerOption::new(
                spec.title,
                spec.description,
                spec.footer,
                spec.value,
            )
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlashOptionSpec {
    title: &'static str,
    description: &'static str,
    footer: &'static str,
    value: &'static str,
}

fn first_arg_or_picker(
    args: &[String],
    with_arg: impl FnOnce(String) -> SlashCommandAction,
    without_arg: SlashCommandAction,
) -> SlashCommandAction {
    args.first().cloned().map(with_arg).unwrap_or(without_arg)
}

fn plan_goal_command(args: &[String]) -> SlashCommandAction {
    match args.first().map(String::as_str) {
        None => SlashCommandAction::ShowGoal,
        Some("clear") if args.len() == 1 => SlashCommandAction::ClearGoal,
        Some("pause") if args.len() == 1 => SlashCommandAction::PauseGoal,
        Some("resume") if args.len() == 1 => SlashCommandAction::ResumeGoal,
        Some(_) => SlashCommandAction::SetGoal(args.join(" ")),
    }
}

fn plan_skill_command(args: &[String]) -> SlashCommandAction {
    match args.first().map(String::as_str) {
        None => SlashCommandAction::OpenSkillPicker,
        Some("list") => SlashCommandAction::ShowSkills,
        Some("info") => args
            .get(1)
            .cloned()
            .map(SlashCommandAction::ShowSkill)
            .unwrap_or(SlashCommandAction::ShowSkillUsage),
        Some(skill) => SlashCommandAction::InsertSkillMentionByName(skill.to_string()),
    }
}

fn slash_option_specs() -> &'static [SlashOptionSpec] {
    &[
        SlashOptionSpec {
            title: "/help",
            description: "Show available commands",
            footer: "docs",
            value: "/help",
        },
        SlashOptionSpec {
            title: "/model",
            description: "Switch model",
            footer: "modal",
            value: "/model",
        },
        SlashOptionSpec {
            title: "/connect",
            description: "Connect a provider (OAuth or API key)",
            footer: "modal",
            value: "/connect",
        },
        SlashOptionSpec {
            title: "/think",
            description: "Set reasoning effort",
            footer: "modal",
            value: "/think",
        },
        SlashOptionSpec {
            title: "/agent",
            description: "Switch agent",
            footer: "modal",
            value: "/agent",
        },
        SlashOptionSpec {
            title: "/compact",
            description: "Compact session context",
            footer: "server",
            value: "/compact",
        },
        SlashOptionSpec {
            title: "/goal",
            description: "Show, set, pause, resume, or clear the session goal",
            footer: "server",
            value: "/goal",
        },
        SlashOptionSpec {
            title: "/undo",
            description: "Revert the last session change",
            footer: "session",
            value: "/undo",
        },
        SlashOptionSpec {
            title: "/redo",
            description: "Restore the reverted session change",
            footer: "session",
            value: "/redo",
        },
        SlashOptionSpec {
            title: "/piss",
            description: "A tiny visitor waters your session (the model hears about it)",
            footer: "fun",
            value: "/piss",
        },
        SlashOptionSpec {
            title: "/cuss",
            description:
                "The tiny visitor returns, furious (#$@!) — the model hears about it",
            footer: "fun",
            value: "/cuss",
        },
        SlashOptionSpec {
            title: "/glitch",
            description: "He unplugs your session for a second — the model remembers",
            footer: "fun",
            value: "/glitch",
        },
        SlashOptionSpec {
            title: "/disco",
            description: "Disco ball, beams, confetti — he came to celebrate",
            footer: "fun",
            value: "/disco",
        },
        SlashOptionSpec {
            title: "/gangfight",
            description: "Two crews, one corridor of tracer fire — his side wins",
            footer: "fun",
            value: "/gangfight",
        },
        SlashOptionSpec {
            title: "/praise",
            description: "Jesus on the throne, everyone bowing — a moment of worship",
            footer: "fun",
            value: "/praise",
        },
        SlashOptionSpec {
            title: "/sessions",
            description: "List sessions",
            footer: "server",
            value: "/sessions",
        },
        SlashOptionSpec {
            title: "/sub-agent",
            description: "List subagent sessions",
            footer: "server",
            value: "/sub-agent",
        },
        SlashOptionSpec {
            title: "/skills",
            description: "Search discovered skills",
            footer: "server",
            value: "/skills",
        },
        SlashOptionSpec {
            title: "/mcp",
            description: "Show MCP status",
            footer: "server",
            value: "/mcp",
        },
        SlashOptionSpec {
            title: "/queue",
            description: "Show queued prompts",
            footer: "server",
            value: "/queue",
        },
        SlashOptionSpec {
            title: "/permissions",
            description: "Show pending permissions",
            footer: "server",
            value: "/permissions",
        },
        SlashOptionSpec {
            title: "/yolo",
            description: "Skip ALL permission prompts (dangerous) — toggle",
            footer: "danger",
            value: "/yolo",
        },
        SlashOptionSpec {
            title: "/questions",
            description: "Show pending questions",
            footer: "server",
            value: "/questions",
        },
        SlashOptionSpec {
            title: "/permit",
            description: "Reply to permission",
            footer: "server",
            value: "/permit",
        },
        SlashOptionSpec {
            title: "/answer",
            description: "Answer pending question",
            footer: "server",
            value: "/answer",
        },
        SlashOptionSpec {
            title: "/reject",
            description: "Reject question/permission",
            footer: "server",
            value: "/reject",
        },
        SlashOptionSpec {
            title: "/abort",
            description: "Abort current run",
            footer: "server",
            value: "/abort",
        },
        SlashOptionSpec {
            title: "/new",
            description: "Create a new session",
            footer: "server",
            value: "/new",
        },
        SlashOptionSpec {
            title: "/exit",
            description: "Close this agent tab",
            footer: "ui",
            value: "/exit",
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plans_picker_or_direct_value_commands() {
        assert_eq!(
            plan_slash_command("/model"),
            SlashCommandAction::OpenModelPicker
        );
        assert_eq!(
            plan_slash_command("/model claude-opus"),
            SlashCommandAction::ApplyModel("claude-opus".to_string())
        );
        assert_eq!(
            plan_slash_command("/session sess-1"),
            SlashCommandAction::SwitchSession("sess-1".to_string())
        );
    }

    #[test]
    fn plans_skill_subcommands() {
        assert_eq!(
            plan_slash_command("/skill"),
            SlashCommandAction::OpenSkillPicker
        );
        assert_eq!(
            plan_slash_command("/skill list"),
            SlashCommandAction::ShowSkills
        );
        assert_eq!(
            plan_slash_command("/skill info rust"),
            SlashCommandAction::ShowSkill("rust".to_string())
        );
        assert_eq!(
            plan_slash_command("/skill info"),
            SlashCommandAction::ShowSkillUsage
        );
        assert_eq!(
            plan_slash_command("/skill rust"),
            SlashCommandAction::InsertSkillMentionByName("rust".to_string())
        );
    }

    #[test]
    fn plans_goal_subcommands() {
        assert_eq!(plan_slash_command("/goal"), SlashCommandAction::ShowGoal);
        assert_eq!(
            plan_slash_command("/goal clear"),
            SlashCommandAction::ClearGoal
        );
        assert_eq!(
            plan_slash_command("/goal ship the goal feature"),
            SlashCommandAction::SetGoal("ship the goal feature".to_string())
        );
    }

    #[test]
    fn plans_undo_redo_commands() {
        assert_eq!(plan_slash_command("/undo"), SlashCommandAction::UndoSession);
        assert_eq!(plan_slash_command("/redo"), SlashCommandAction::RedoSession);
    }

    #[test]
    fn plans_skip_permissions_toggle() {
        for spelling in [
            "/yolo",
            "/dangerously-skip-permissions",
            "/skip-permissions",
        ] {
            assert_eq!(
                plan_slash_command(spelling),
                SlashCommandAction::ToggleSkipPermissions,
                "{spelling}"
            );
        }
    }

    #[test]
    fn plans_the_easter_eggs() {
        assert_eq!(
            plan_slash_command("/piss"),
            SlashCommandAction::PissOnScreen
        );
        assert_eq!(
            plan_slash_command("/cuss"),
            SlashCommandAction::CussOnScreen
        );
        assert_eq!(
            plan_slash_command("/swear"),
            SlashCommandAction::CussOnScreen
        );
        assert_eq!(
            plan_slash_command("/glitch"),
            SlashCommandAction::GlitchOnScreen
        );
        assert_eq!(
            plan_slash_command("/dance"),
            SlashCommandAction::DiscoOnScreen
        );
        assert_eq!(
            plan_slash_command("/gangfight"),
            SlashCommandAction::GangFightOnScreen
        );
        assert_eq!(
            plan_slash_command("/praise"),
            SlashCommandAction::PraiseOnScreen
        );
        assert_eq!(
            plan_slash_command("/amen"),
            SlashCommandAction::PraiseOnScreen
        );
    }

    #[test]
    fn preserves_raw_tail_for_runtime_commands() {
        assert_eq!(
            plan_slash_command("/login token=abc now"),
            SlashCommandAction::RunServerCommand {
                command: "login".to_string(),
                args: "token=abc now".to_string(),
            }
        );
        assert_eq!(
            plan_slash_command("/answer one two"),
            SlashCommandAction::HandleAnswer("one two".to_string())
        );
    }

    #[test]
    fn exposes_desktop_slash_picker_options() {
        let options = slash_options();
        assert_eq!(
            options.first().map(|option| option.value.as_str()),
            Some("/help")
        );
        assert!(options.iter().any(|option| option.value == "/compact"));
        assert!(options.iter().any(|option| option.value == "/undo"));
        assert!(options.iter().any(|option| option.value == "/redo"));
        assert!(options.iter().any(|option| option.value == "/exit"));
    }
}
