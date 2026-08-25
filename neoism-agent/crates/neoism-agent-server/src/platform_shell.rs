use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(windows, allow(dead_code))]
pub(crate) enum ShellKind {
    Posix,
    PowerShell,
    Cmd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellRuntime {
    kind: ShellKind,
    program: PathBuf,
}

static RUNTIME: OnceLock<ShellRuntime> = OnceLock::new();

pub(crate) fn runtime() -> &'static ShellRuntime {
    RUNTIME.get_or_init(resolve)
}

impl ShellRuntime {
    pub(crate) fn kind(&self) -> ShellKind {
        self.kind
    }

    pub(crate) fn program(&self) -> &Path {
        &self.program
    }

    pub(crate) fn display_name(&self) -> &'static str {
        match self.kind {
            ShellKind::Posix => "shell",
            ShellKind::PowerShell => "PowerShell",
            ShellKind::Cmd => "Command Prompt",
        }
    }

    pub(crate) fn tool_description(&self) -> &'static str {
        match self.kind {
            ShellKind::Posix => "Run shell commands",
            ShellKind::PowerShell => {
                "Run PowerShell commands on Windows. Use PowerShell syntax and cmdlets, quote Windows paths, and use registry providers such as HKCU:. Do not emit bash-only commands such as export, sed, or rm."
            }
            ShellKind::Cmd => {
                "Run Command Prompt commands on Windows. Use cmd.exe syntax and Windows paths."
            }
        }
    }

    pub(crate) fn apply_command(
        &self,
        process: &mut Command,
        command: &str,
        login: bool,
    ) {
        match self.kind {
            ShellKind::Posix => {
                process.args([if login { "-lc" } else { "-c" }, command]);
            }
            ShellKind::PowerShell => {
                let command = format!(
                    "$OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); {command}"
                );
                process.args([
                    "-NoLogo",
                    "-NoProfile",
                    "-NonInteractive",
                    "-ExecutionPolicy",
                    "Bypass",
                    "-Command",
                    &command,
                ]);
            }
            ShellKind::Cmd => {
                let command = format!("chcp 65001>nul & {command}");
                process.args(["/d", "/s", "/c", &command]);
            }
        }
    }
}

fn resolve() -> ShellRuntime {
    #[cfg(windows)]
    {
        for (name, kind) in [
            ("pwsh.exe", ShellKind::PowerShell),
            ("powershell.exe", ShellKind::PowerShell),
            ("cmd.exe", ShellKind::Cmd),
        ] {
            if let Some(program) = resolve_command(name) {
                return ShellRuntime { kind, program };
            }
        }
        return ShellRuntime {
            kind: ShellKind::Cmd,
            program: std::env::var_os("COMSPEC")
                .filter(|value| !value.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("cmd.exe")),
        };
    }

    #[cfg(not(windows))]
    {
        ShellRuntime {
            kind: ShellKind::Posix,
            program: std::env::var_os("SHELL")
                .filter(|value| !value.is_empty())
                .and_then(|value| resolve_command(&value.to_string_lossy()))
                .unwrap_or_else(|| PathBuf::from("/bin/sh")),
        }
    }
}

pub(crate) fn resolve_command(name: &str) -> Option<PathBuf> {
    let request = neoism_agent_service_api::ExecutableRequest::new(
        name,
        neoism_agent_service_api::ExecutablePurpose::PlatformShell,
    );
    crate::lsp::agent_services()
        .executables
        .resolve(&request)
        .ok()
        .map(|result| result.path)
}

pub(crate) fn program() -> String {
    runtime().program().to_string_lossy().into_owned()
}

pub(crate) fn tool_description() -> &'static str {
    runtime().tool_description()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_shell_has_a_program_and_description() {
        assert!(!runtime().program().as_os_str().is_empty());
        assert!(!runtime().tool_description().is_empty());
    }

    #[test]
    fn shell_kind_names_are_stable() {
        assert_eq!(
            ShellRuntime {
                kind: ShellKind::PowerShell,
                program: PathBuf::from("pwsh.exe"),
            }
            .display_name(),
            "PowerShell"
        );
    }
}
