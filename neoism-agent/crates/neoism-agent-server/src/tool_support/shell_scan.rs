use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::platform_shell::ShellKind;

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ShellScan {
    pub(crate) external_dirs: BTreeSet<String>,
    pub(crate) command_patterns: BTreeSet<String>,
    pub(crate) always_patterns: BTreeSet<String>,
}

const CWD_COMMANDS: &[&str] = &["cd", "chdir", "popd", "pushd"];
const FILE_COMMANDS: &[&str] = &[
    "cd", "chdir", "popd", "pushd", "rm", "cp", "mv", "mkdir", "touch", "chmod", "chown",
    "cat",
];
const POWERSHELL_FILE_COMMANDS: &[&str] = &[
    "cd",
    "chdir",
    "pop-location",
    "push-location",
    "set-location",
    "copy-item",
    "move-item",
    "remove-item",
    "new-item",
    "get-content",
    "set-content",
    "add-content",
    "out-file",
];
const POWERSHELL_CWD_COMMANDS: &[&str] = &[
    "cd",
    "chdir",
    "pop-location",
    "push-location",
    "set-location",
];
const CMD_FILE_COMMANDS: &[&str] = &[
    "cd", "chdir", "copy", "del", "erase", "move", "ren", "rename", "type",
];
const CMD_CWD_COMMANDS: &[&str] = &["cd", "chdir"];

pub(crate) fn scan(
    command: &str,
    cwd: &Path,
    project_root: &Path,
    shell: ShellKind,
) -> ShellScan {
    let mut scan = ShellScan::default();
    for segment in command_segments(command, shell) {
        let mut tokens = shell_words(&segment, shell);
        if shell == ShellKind::PowerShell
            && tokens.first().is_some_and(|token| token == "&")
        {
            tokens.remove(0);
        }
        let Some(name) = tokens.first() else {
            continue;
        };
        let lowered = name.trim_start_matches('&').to_ascii_lowercase();
        let (file_commands, cwd_commands): (&[&str], &[&str]) = match shell {
            ShellKind::PowerShell => (POWERSHELL_FILE_COMMANDS, POWERSHELL_CWD_COMMANDS),
            ShellKind::Cmd => (CMD_FILE_COMMANDS, CMD_CWD_COMMANDS),
            ShellKind::Posix => (FILE_COMMANDS, CWD_COMMANDS),
        };

        if file_commands.contains(&lowered.as_str()) {
            for arg in path_args(&tokens, shell) {
                if let Some(path) = resolve_shell_path(arg, cwd, shell) {
                    if !contained_by(&path, project_root) {
                        scan.external_dirs.insert(external_pattern(
                            &path,
                            cwd_commands.contains(&lowered.as_str()),
                        ));
                    }
                }
            }
        }

        if !cwd_commands.contains(&lowered.as_str()) {
            scan.command_patterns.insert(segment.clone());
            scan.always_patterns.insert(always_pattern(&tokens));
        }
    }
    scan
}

fn command_segments(command: &str, shell: ShellKind) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut chars = command.chars().peekable();
    let mut quote = None;
    let mut escaped = false;

    while let Some(ch) = chars.next() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        let escape = match shell {
            ShellKind::PowerShell => '`',
            ShellKind::Cmd => '^',
            ShellKind::Posix => '\\',
        };
        if ch == escape && quote != Some('\'') {
            current.push(ch);
            escaped = true;
            continue;
        }
        if matches!(quote, Some(q) if q == ch) {
            current.push(ch);
            quote = None;
            continue;
        }
        if quote.is_none() && (ch == '\'' || ch == '"') {
            current.push(ch);
            quote = Some(ch);
            continue;
        }
        if quote.is_none() && (ch == '\n' || (ch == ';' && shell != ShellKind::Cmd)) {
            push_segment(&mut segments, &mut current);
            continue;
        }
        if quote.is_none() && (ch == '&' || ch == '|') {
            if chars.peek() == Some(&ch) {
                chars.next();
                push_segment(&mut segments, &mut current);
                continue;
            }
            if matches!(shell, ShellKind::PowerShell | ShellKind::Cmd) && ch == '|' {
                push_segment(&mut segments, &mut current);
                continue;
            }
            if shell == ShellKind::Cmd && ch == '&' {
                push_segment(&mut segments, &mut current);
                continue;
            }
        }
        current.push(ch);
    }

    push_segment(&mut segments, &mut current);
    segments
}

fn push_segment(segments: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        segments.push(trimmed.to_string());
    }
    current.clear();
}

fn shell_words(command: &str, shell: ShellKind) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut quote = None;
    let mut escaped = false;

    for ch in command.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        let escape = match shell {
            ShellKind::PowerShell => '`',
            ShellKind::Cmd => '^',
            ShellKind::Posix => '\\',
        };
        if ch == escape && quote != Some('\'') {
            escaped = true;
            continue;
        }
        if matches!(quote, Some(q) if q == ch) {
            quote = None;
            continue;
        }
        if quote.is_none() && (ch == '\'' || ch == '"') {
            quote = Some(ch);
            continue;
        }
        if quote.is_none() && ch.is_whitespace() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        current.push(ch);
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

fn path_args(tokens: &[String], shell: ShellKind) -> impl Iterator<Item = &str> {
    tokens.iter().skip(1).filter_map(move |token| {
        if token == "&" {
            return None;
        }
        if token.starts_with('-')
            || token.starts_with('+')
            || (shell == ShellKind::Cmd && token.starts_with('/'))
            || token.contains('=')
        {
            return None;
        }
        if matches!(token.as_str(), ">" | ">>" | "<" | "2>" | "2>>" | "&>") {
            return None;
        }
        Some(token.as_str())
    })
}

fn resolve_shell_path(raw: &str, cwd: &Path, shell: ShellKind) -> Option<PathBuf> {
    let prefix = literal_prefix(raw)?;
    if prefix.contains('$') || (shell != ShellKind::PowerShell && prefix.contains('`')) {
        return None;
    }
    if shell == ShellKind::PowerShell && is_registry_path(prefix) {
        return None;
    }
    let expanded = expand_home(prefix);
    let path = Path::new(&expanded);
    Some(if path.is_absolute() {
        normalize(path)
    } else {
        normalize(&cwd.join(path))
    })
}

fn is_registry_path(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    ["hkcu:", "hklm:", "hkcr:", "hku:", "hkcc:", "registry::"]
        .iter()
        .any(|prefix| lower.starts_with(prefix))
}

fn literal_prefix(raw: &str) -> Option<&str> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }
    let wildcard = trimmed.find(['*', '?', '[']);
    let prefix = wildcard.map_or(trimmed, |index| &trimmed[..index]);
    if prefix.is_empty() {
        None
    } else {
        Some(prefix.trim_end_matches(['/', '\\']))
    }
}

fn expand_home(raw: &str) -> String {
    let home = || {
        std::env::var("HOME")
            .ok()
            .or_else(|| std::env::var("USERPROFILE").ok())
            .or_else(|| {
                Some(format!(
                    "{}{}",
                    std::env::var("HOMEDRIVE").ok()?,
                    std::env::var("HOMEPATH").ok()?
                ))
            })
    };
    if raw == "~" {
        return home().unwrap_or_else(|| raw.to_string());
    }
    if let Some(rest) = raw.strip_prefix("~/").or_else(|| raw.strip_prefix("~\\")) {
        if let Some(home) = home() {
            return Path::new(&home).join(rest).to_string_lossy().into_owned();
        }
    }
    raw.to_string()
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                out.pop();
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn contained_by(path: &Path, root: &Path) -> bool {
    #[cfg(windows)]
    {
        return path
            .to_string_lossy()
            .to_ascii_lowercase()
            .starts_with(&root.to_string_lossy().to_ascii_lowercase());
    }
    #[cfg(not(windows))]
    path.starts_with(root)
}

fn external_pattern(path: &Path, directory_arg: bool) -> String {
    let dir = if directory_arg || path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    format!("{}/*", dir.display())
}

fn always_pattern(tokens: &[String]) -> String {
    let command = tokens.first().map_or("*", String::as_str);
    format!("{command} *")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_multiple_command_patterns_and_always_prefixes() {
        let root = Path::new("/repo");
        let scan = scan("git status && echo done", root, root, ShellKind::Posix);
        assert!(scan.command_patterns.contains("git status"));
        assert!(scan.command_patterns.contains("echo done"));
        assert!(scan.always_patterns.contains("git *"));
        assert!(scan.always_patterns.contains("echo *"));
    }

    #[test]
    fn skips_bash_permission_for_cd_only_but_tracks_external_dir() {
        let scan = scan(
            "cd ../outside",
            Path::new("/repo/app"),
            Path::new("/repo/app"),
            ShellKind::Posix,
        );
        assert!(scan.command_patterns.is_empty());
        assert!(scan.external_dirs.contains("/repo/outside/*"));
    }

    #[test]
    fn detects_external_file_command_paths() {
        let scan = scan(
            "cat /etc/hosts",
            Path::new("/repo"),
            Path::new("/repo"),
            ShellKind::Posix,
        );
        assert!(scan.external_dirs.contains("/etc/*"));
    }

    #[test]
    fn scans_powershell_statements_and_paths() {
        let root = Path::new(r"C:\repo");
        let scan = scan(
            r#"Set-Content "C:\Users\Parker\Desktop\neoism.json" '{}'; Get-Content .\Cargo.toml | Out-Null"#,
            root,
            root,
            ShellKind::PowerShell,
        );
        assert!(scan
            .command_patterns
            .iter()
            .any(|value| value.starts_with("Set-Content")));
        assert!(scan
            .command_patterns
            .iter()
            .any(|value| value.starts_with("Get-Content")));
    }

    #[test]
    fn powershell_registry_provider_is_not_treated_as_a_file_path() {
        let root = Path::new(r"C:\repo");
        let scan = scan(
            r#"Set-ItemProperty HKCU:\Software\Neoism -Name Enabled -Value 1"#,
            root,
            root,
            ShellKind::PowerShell,
        );
        assert!(scan.external_dirs.is_empty());
        assert_eq!(scan.command_patterns.len(), 1);
    }

    #[test]
    fn powershell_call_operator_uses_invoked_command_for_permissions() {
        let root = Path::new(r"C:\repo");
        let scan = scan(
            r#"& "C:\Program Files\Git\bin\git.exe" status"#,
            root,
            root,
            ShellKind::PowerShell,
        );
        assert_eq!(
            scan.always_patterns,
            BTreeSet::from([r"C:\Program Files\Git\bin\git.exe *".to_string()])
        );
    }

    #[test]
    fn cmd_single_separators_create_distinct_permission_patterns() {
        let root = Path::new(r"C:\repo");
        let scan = scan(
            "type Cargo.toml & echo done | findstr done",
            root,
            root,
            ShellKind::Cmd,
        );
        assert_eq!(
            scan.always_patterns,
            BTreeSet::from([
                "echo *".to_string(),
                "findstr *".to_string(),
                "type *".to_string(),
            ])
        );
    }
}
