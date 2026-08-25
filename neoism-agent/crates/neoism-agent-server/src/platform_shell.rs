use std::path::{Path, PathBuf};

use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellKind {
    #[cfg(not(windows))]
    Posix,
    PowerShell,
    Cmd,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ShellRuntime {
    kind: ShellKind,
    program: PathBuf,
}

impl ShellRuntime {
    pub(crate) fn resolve(services: &neoism_agent_service_api::AgentServices) -> Self {
        resolve(services)
    }

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

fn resolve(services: &neoism_agent_service_api::AgentServices) -> ShellRuntime {
    #[cfg(windows)]
    {
        for (name, kind) in [
            ("pwsh.exe", ShellKind::PowerShell),
            ("powershell.exe", ShellKind::PowerShell),
            ("cmd.exe", ShellKind::Cmd),
        ] {
            if let Some(program) = resolve_command(services, name) {
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
                .and_then(|value| resolve_command(services, &value.to_string_lossy()))
                .unwrap_or_else(|| PathBuf::from("/bin/sh")),
        }
    }
}

pub(crate) fn resolve_command(
    services: &neoism_agent_service_api::AgentServices,
    name: &str,
) -> Option<PathBuf> {
    let request = neoism_agent_service_api::ExecutableRequest::new(
        name,
        neoism_agent_service_api::ExecutablePurpose::PlatformShell,
    );
    services
        .executables
        .resolve(&request)
        .ok()
        .map(|result| result.path)
}

pub(crate) fn program() -> String {
    resolve(&crate::standard_services())
        .program()
        .to_string_lossy()
        .into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_shell_has_a_program_and_name() {
        let runtime = ShellRuntime::resolve(&crate::standard_services());
        assert!(!runtime.program().as_os_str().is_empty());
        assert!(!runtime.display_name().is_empty());
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
