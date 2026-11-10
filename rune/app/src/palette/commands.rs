//! The `Alt+P` command catalog. Titles, shortcut hints, order and namespaces are the original's
//! (`neoism-frontend/shared/src/panels/command_palette/commands.rs`); like the GPUiX host, only
//! commands this app can run are listed.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    NewTab,
    CloseTab,
    CloseOtherTabs,
    NextTab,
    PreviousTab,
    SplitRight,
    SplitDown,
    NextSplit,
    PreviousSplit,
    CloseSplitOrTab,
    IncreaseFontSize,
    DecreaseFontSize,
    ResetFontSize,
    ThemePicker,
    OpenSettings,
    Backgrounds,
    SearchFiles,
    SearchWords,
    SearchGitChanges,
    OpenAgent,
    ToggleConversations,
    Quit,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Service {
    Neoism,
    Agent,
    Workspace,
}

impl Service {
    pub fn prefix(self) -> &'static str {
        match self {
            Service::Neoism => "neoism",
            Service::Agent => "neoism-agent",
            Service::Workspace => "workspace",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Service::Neoism => "\u{f0c9}",
            Service::Agent => "\u{f135}",
            Service::Workspace => "\u{f07b}",
        }
    }
}

pub struct Command {
    pub title: &'static str,
    pub shortcut: &'static str,
    pub action: Action,
    pub service: Service,
}

const fn cmd(title: &'static str, shortcut: &'static str, action: Action, service: Service) -> Command {
    Command { title, shortcut, action, service }
}

pub const COMMANDS: &[Command] = &[
    cmd("New Tab", "Ctrl+Shift+T terminal tab", Action::NewTab, Service::Neoism),
    cmd("Close Tab", "Cmd+W", Action::CloseTab, Service::Neoism),
    cmd("Close Other Tabs", "", Action::CloseOtherTabs, Service::Neoism),
    cmd("Next Tab", "Ctrl+Tab", Action::NextTab, Service::Neoism),
    cmd("Previous Tab", "Ctrl+Shift+Tab", Action::PreviousTab, Service::Neoism),
    cmd("Split Right", "Cmd+D", Action::SplitRight, Service::Neoism),
    cmd("Split Down", "Cmd+Shift+D", Action::SplitDown, Service::Neoism),
    cmd("Next Split", "", Action::NextSplit, Service::Neoism),
    cmd("Previous Split", "", Action::PreviousSplit, Service::Neoism),
    cmd("Close Split or Tab", "q close current", Action::CloseSplitOrTab, Service::Neoism),
    cmd("Increase Font Size", "Cmd++", Action::IncreaseFontSize, Service::Neoism),
    cmd("Decrease Font Size", "Cmd+-", Action::DecreaseFontSize, Service::Neoism),
    cmd("Reset Font Size", "Cmd+0", Action::ResetFontSize, Service::Neoism),
    cmd("Settings", "", Action::OpenSettings, Service::Neoism),
    cmd("Theme Picker", "", Action::ThemePicker, Service::Neoism),
    cmd("Backgrounds", "", Action::Backgrounds, Service::Neoism),
    cmd("Search Files", "<leader>ff", Action::SearchFiles, Service::Workspace),
    cmd("Search Words", "<leader>fw", Action::SearchWords, Service::Workspace),
    cmd("Search Git Changes", "<leader>fg", Action::SearchGitChanges, Service::Workspace),
    cmd("Neoism", "Alt+A", Action::OpenAgent, Service::Agent),
    cmd("Conversations", "Alt+C", Action::ToggleConversations, Service::Agent),
    cmd("Quit", "Cmd+Q", Action::Quit, Service::Neoism),
];

/// In-order subsequence match with consecutive, word-boundary and early-start bonuses; `None`
/// when some query character is missing (the original `fuzzy_score`).
pub fn fuzzy_score(query: &str, target: &str) -> Option<i32> {
    let query: Vec<char> = query.to_lowercase().chars().collect();
    if query.is_empty() { return Some(0); }
    let target: Vec<char> = target.to_lowercase().chars().collect();
    let (mut qi, mut score, mut previous, mut first) = (0, 0i32, false, None);
    for (ti, &c) in target.iter().enumerate() {
        if qi < query.len() && c == query[qi] {
            first.get_or_insert(ti);
            if previous { score += 5; }
            if ti == 0 || !target[ti - 1].is_alphanumeric() { score += 10; }
            previous = true;
            qi += 1;
        } else {
            previous = false;
        }
    }
    if qi < query.len() { return None; }
    Some(score + 20i32.saturating_sub(first.unwrap_or(0) as i32))
}

/// A title scores in full; a match only in the shortcut column is slightly demoted.
pub fn command_score(query: &str, command: &Command) -> Option<i32> {
    match (fuzzy_score(query, command.title), fuzzy_score(query, command.shortcut)) {
        (Some(a), Some(b)) => Some(a.max(b - 4)),
        (Some(a), None) => Some(a),
        (None, Some(b)) => Some(b - 4),
        (None, None) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fuzzy_prefers_word_starts() {
        assert!(fuzzy_score("nt", "New Tab").unwrap() > fuzzy_score("nt", "Reset Font").unwrap_or(-1));
        assert_eq!(fuzzy_score("xyz", "New Tab"), None);
        assert_eq!(fuzzy_score("", "anything"), Some(0));
    }
}
