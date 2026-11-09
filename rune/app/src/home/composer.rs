//! The Home command composer's suggestions (`gpuix/src/terminal/suggestions.ts` and
//! `TerminalCommandInput.tsx`): the ghost text after the caret is the newest history entry that
//! extends the draft; `Up`/`Down`/`Ctrl+R` open history, `Tab` opens shell completions (the
//! daemon's `CompleteTerminal`, then argument hints, then history).

use std::collections::HashSet;

const LIMIT: usize = 100;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Command,
    Directory,
    File,
    History,
    Option,
    Argument,
    Branch,
}

impl Kind {
    fn from_wire(kind: &str) -> Kind {
        match kind {
            "command" | "alias" | "function" | "script" => Kind::Command,
            "directory" => Kind::Directory,
            "file" => Kind::File,
            "option" => Kind::Option,
            "branch" | "remote" => Kind::Branch,
            _ => Kind::Argument,
        }
    }

    /// The row icon (Symbols Nerd Font).
    pub fn icon(self) -> &'static str {
        match self {
            Kind::Command => "\u{f120}",
            Kind::Directory => "\u{f07b}",
            Kind::File => "\u{f15b}",
            Kind::History => "\u{f1da}",
            Kind::Option => "\u{f0ad}",
            Kind::Argument => "\u{f0c9}",
            Kind::Branch => "\u{e725}",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Suggestion {
    /// The whole command line after accepting it.
    pub value: String,
    pub label: String,
    pub detail: String,
    pub kind: Kind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuKind {
    History,
    Completion,
}

pub struct Menu {
    pub kind: MenuKind,
    /// The draft when the menu opened; Escape restores it.
    pub query: String,
    pub items: Vec<Suggestion>,
    pub choice: usize,
    /// Waiting for the daemon's completions.
    pub loading: bool,
}

/// Composer state of one Home terminal.
#[derive(Default)]
pub struct Composer {
    /// The shell's history file, oldest first (`Files::ReadShellHistory`).
    pub history: Vec<String>,
    pub history_requested: bool,
    pub menu: Option<Menu>,
}

impl Composer {
    /// History matches for `current`, newest first; `recent` (this session's commands) wins.
    pub fn history(&self, current: &str, recent: &[String]) -> Vec<Suggestion> {
        let mut seen = HashSet::new();
        let mut result = Vec::new();
        for value in self.history.iter().chain(recent.iter()).rev() {
            if result.len() >= LIMIT { break; }
            if value.trim().is_empty() || value.len() > 8192 || value.chars().any(char::is_control) || !seen.insert(value.as_str()) { continue; }
            if current.is_empty() || value.starts_with(current) || subsequence(current, value) {
                result.push(Suggestion { value: value.clone(), label: value.clone(), detail: "History".into(), kind: Kind::History });
            }
        }
        result
    }

    /// The full command the ghost text completes `current` to.
    pub fn ghost(&self, current: &str, recent: &[String]) -> Option<String> {
        if current.is_empty() || current.contains('\n') { return None; }
        self.history(current, recent).into_iter().find(|row| row.value.starts_with(current) && row.value.len() > current.len()).map(|row| row.value)
    }

    pub fn open(&mut self, kind: MenuKind, draft: &str, recent: &[String]) {
        let items = if kind == MenuKind::History { self.history(draft, recent) } else { Vec::new() };
        self.menu = Some(Menu { kind, query: draft.to_owned(), loading: kind == MenuKind::Completion, items, choice: 0 });
    }

    /// The daemon's completions for `line` arrived: merge native rows, argument hints, then history.
    /// Returns the first row's command line, which the composer previews (as Tab in GPUiX did).
    pub fn completions(&mut self, line: &str, native: Vec<(String, String, String, String)>, recent: &[String]) -> Option<String> {
        let history = self.history(line, recent);
        let menu = self.menu.as_mut()?;
        if menu.kind != MenuKind::Completion || menu.query != line || !menu.loading { return None; }
        let mut items: Vec<Suggestion> = Vec::new();
        let mut add = |row: Suggestion| {
            if items.len() < LIMIT && !items.iter().any(|existing| existing.value.trim_end() == row.value.trim_end()) { items.push(row); }
        };
        for (value, label, detail, kind) in native {
            if value.is_empty() || label.is_empty() || (value.clone() + &label).chars().any(char::is_control) { continue; }
            add(Suggestion { value, label, detail, kind: Kind::from_wire(&kind) });
        }
        for row in argument_hints(line) { add(row); }
        for row in history { add(row); }
        menu.items = items;
        menu.choice = 0;
        menu.loading = false;
        if menu.items.is_empty() {
            // No matches: close (GPUiX flashes a red underline).
            self.menu = None;
            return None;
        }
        Some(menu.items[0].value.clone())
    }

    /// Move the highlighted row; completion lists wrap, history stops at the ends.
    pub fn step(&mut self, delta: isize) -> Option<&Suggestion> {
        let menu = self.menu.as_mut()?;
        let count = menu.items.len() as isize;
        if count == 0 { return None; }
        let next = menu.choice as isize + delta;
        menu.choice = match menu.kind {
            MenuKind::Completion => next.rem_euclid(count) as usize,
            MenuKind::History => next.clamp(0, count - 1) as usize,
        };
        menu.items.get(menu.choice)
    }

    pub fn chosen(&self) -> Option<&Suggestion> {
        let menu = self.menu.as_ref()?;
        menu.items.get(menu.choice)
    }
}

/// `needle`'s characters appear in order in `value`.
fn subsequence(needle: &str, value: &str) -> bool {
    let mut needle = needle.chars().peekable();
    for c in value.chars() {
        if needle.peek() == Some(&c) { needle.next(); }
    }
    needle.peek().is_none()
}

/// The part of `label` the user already typed, for the bold prefix in a menu row.
pub fn typed_prefix<'a>(query: &str, item: &'a Suggestion) -> &'a str {
    let token = final_token(query).map(|t| t.text).unwrap_or_else(|| {
        query.rsplit(|c: char| c.is_whitespace() || "'\"`|;&<>".contains(c)).next().unwrap_or("").to_owned()
    });
    let prefix = match item.kind {
        Kind::History => query.to_owned(),
        Kind::Directory | Kind::File => token.rsplit(['/', '\\', '=']).next().unwrap_or("").to_owned(),
        _ => token,
    };
    if item.label.starts_with(&prefix) { &item.label[..prefix.len()] } else { "" }
}

struct Token {
    start: usize,
    text: String,
    first: bool,
}

/// The final word of one simple command (no expansion, quoting kept out), or `None` when the
/// line is beyond this small grammar.
fn final_token(line: &str) -> Option<Token> {
    if line.len() > 8192 || line.chars().any(|c| c.is_control() || "$`;&|<>(){}*?!#=[]".contains(c)) { return None; }
    let (mut start, mut text, mut words, mut active) = (0, String::new(), 0, false);
    for (i, c) in line.char_indices() {
        if c.is_whitespace() {
            if active { words += 1; }
            start = i + c.len_utf8();
            text.clear();
            active = false;
            continue;
        }
        if c == '\'' || c == '"' || c == '\\' { return None; }
        active = true;
        text.push(c);
    }
    Some(Token { start, text, first: words == 0 })
}

/// Subcommand and flag hints for common tools (the GPUiX `ARGUMENTS` table).
fn argument_hints(line: &str) -> Vec<Suggestion> {
    let Some(token) = final_token(line) else { return Vec::new() };
    if token.first { return Vec::new(); }
    let command = line[..token.start].split_whitespace().collect::<Vec<_>>().join(" ");
    let table: &[(&str, &str)] = match command.as_str() {
        "git" => &[("status", "Working tree status"), ("diff", "Show changes"), ("log", "Commit history"), ("add", "Stage changes"), ("commit", "Record staged changes"), ("switch", "Switch branches"), ("branch", "List or manage branches"), ("restore", "Restore files"), ("fetch", "Fetch remote updates"), ("pull", "Integrate remote updates"), ("push", "Publish commits"), ("stash", "Store local changes"), ("--help", "Command help")],
        "git status" => &[("--short", "Compact status"), ("--branch", "Include branch information"), ("--porcelain", "Machine-readable status"), ("--untracked-files", "Show untracked files")],
        "git diff" => &[("--staged", "Show staged changes"), ("--stat", "Change summary"), ("--name-only", "Changed file names"), ("--word-diff", "Highlight changed words")],
        "git log" => &[("--oneline", "One line per commit"), ("--graph", "Show branch graph"), ("--all", "Include all branches"), ("--decorate", "Show branch and tag names")],
        "git commit" => &[("-m", "Commit message"), ("--amend", "Replace the latest commit"), ("--no-edit", "Keep the existing message")],
        "cargo" => &[("check", "Check without producing binaries"), ("test", "Run tests"), ("build", "Build the package"), ("run", "Build and run"), ("fmt", "Format source"), ("clippy", "Run lint checks"), ("doc", "Build documentation"), ("--help", "Command help")],
        "cargo test" => &[("--lib", "Library tests"), ("--test", "Select an integration test"), ("--package", "Select a package"), ("--features", "Enable features"), ("--no-default-features", "Disable default features")],
        "cargo check" => &[("--all-targets", "Check all targets"), ("--workspace", "Check workspace members"), ("--package", "Select a package"), ("--features", "Enable features")],
        "npm" => &[("install", "Install dependencies"), ("run", "Run a package script"), ("test", "Run tests"), ("exec", "Run a package executable"), ("outdated", "Check dependency versions"), ("--help", "Command help")],
        "bun" => &[("install", "Install dependencies"), ("run", "Run a script"), ("test", "Run tests"), ("add", "Add a dependency"), ("remove", "Remove a dependency"), ("--help", "Command help")],
        "docker" => &[("ps", "List containers"), ("images", "List images"), ("run", "Create and start a container"), ("exec", "Run a command in a container"), ("logs", "Read container logs"), ("compose", "Manage a multi-container application"), ("--help", "Command help")],
        "ls" => &[("-l", "Long listing"), ("-a", "Include hidden entries"), ("-h", "Human-readable sizes"), ("-R", "List recursively"), ("-t", "Sort by modification time")],
        "grep" => &[("-n", "Include line numbers"), ("-i", "Ignore case"), ("-r", "Search recursively"), ("-l", "Print matching file names"), ("-v", "Invert matches")],
        "mkdir" => &[("-p", "Create parent directories"), ("-v", "Report created directories")],
        "ssh" => &[("-i", "Identity file"), ("-p", "Remote port"), ("-L", "Local port forwarding"), ("-N", "Do not run a remote command")],
        _ => &[],
    };
    table.iter().filter(|(argument, _)| argument.starts_with(&token.text)).map(|(argument, detail)| Suggestion {
        value: format!("{}{argument} ", &line[..token.start]),
        label: (*argument).to_owned(),
        detail: (*detail).to_owned(),
        kind: if argument.starts_with('-') { Kind::Option } else { Kind::Command },
    }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn composer(history: &[&str]) -> Composer {
        Composer { history: history.iter().map(|s| s.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn ghost_is_the_newest_extension() {
        let c = composer(&["git switch main", "git status", "git switch dusty_fix_2"]);
        assert_eq!(c.ghost("git sw", &[]).as_deref(), Some("git switch dusty_fix_2"));
        assert_eq!(c.ghost("git sw", &["git switch other".into()]).as_deref(), Some("git switch other"));
        assert_eq!(c.ghost("", &[]), None);
        assert_eq!(c.ghost("git switch dusty_fix_2", &[]), None);
    }

    #[test]
    fn completions_merge_native_hints_and_history_without_duplicates() {
        let mut c = composer(&["git status --short"]);
        c.open(MenuKind::Completion, "git st", &[]);
        c.completions("git st", vec![("git status ".into(), "status".into(), String::new(), "argument".into())], &[]);
        let menu = c.menu.as_ref().unwrap();
        assert!(!menu.loading);
        assert_eq!(menu.items.iter().filter(|i| i.value.trim_end() == "git status").count(), 1);
        assert!(menu.items.iter().any(|i| i.value == "git stash "));
        assert_eq!(menu.items.last().unwrap().kind, Kind::History);
    }

    #[test]
    fn stale_completions_are_ignored() {
        let mut c = composer(&[]);
        c.open(MenuKind::Completion, "ls", &[]);
        c.completions("l", vec![("ls ".into(), "ls".into(), String::new(), "command".into())], &[]);
        assert!(c.menu.as_ref().unwrap().loading);
    }

    #[test]
    fn completion_wraps_history_clamps() {
        let mut c = composer(&["a1", "a2"]);
        c.open(MenuKind::History, "a", &[]);
        assert_eq!(c.step(-1).unwrap().value, "a2");
        assert_eq!(c.step(5).unwrap().value, "a1");
        c.open(MenuKind::Completion, "git ", &[]);
        c.completions("git ", Vec::new(), &[]);
        let count = c.menu.as_ref().unwrap().items.len() as isize;
        c.step(count);
        assert_eq!(c.menu.as_ref().unwrap().choice, 0);
    }

    #[test]
    fn prefix_highlights_the_typed_part() {
        let item = Suggestion { value: "cd rune/".into(), label: "rune/".into(), detail: String::new(), kind: Kind::Directory };
        assert_eq!(typed_prefix("cd ru", &item), "ru");
    }
}
