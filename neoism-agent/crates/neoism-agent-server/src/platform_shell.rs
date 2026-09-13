use std::path::{Path, PathBuf};

use base64::Engine;
use tokio::process::Command;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ShellKind {
    // Never constructed on Windows, but the variant stays ungated so
    // cross-platform match arms compile on every target.
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
        process.args(self.command_args(command, login));
    }

    pub(crate) fn command_args(&self, command: &str, login: bool) -> Vec<String> {
        match self.kind {
            ShellKind::Posix => {
                vec![
                    if login { "-lc" } else { "-c" }.to_string(),
                    command.to_string(),
                ]
            }
            ShellKind::PowerShell => {
                // `-Command` goes through CreateProcess quoting, which eats JSON
                // quotes/braces. `-EncodedCommand` is UTF-16LE base64 of the
                // script, matching the interactive PTY hook, without `-NoExit`.
                let script = format!(
                    "$OutputEncoding = [Console]::OutputEncoding = [Text.UTF8Encoding]::new($false); {command}"
                );
                vec![
                    "-NoLogo".to_string(),
                    "-NoProfile".to_string(),
                    "-NonInteractive".to_string(),
                    "-ExecutionPolicy".to_string(),
                    "Bypass".to_string(),
                    "-EncodedCommand".to_string(),
                    encode_powershell_command(&script),
                ]
            }
            ShellKind::Cmd => {
                vec![
                    "/d".to_string(),
                    "/s".to_string(),
                    "/c".to_string(),
                    format!("chcp 65001>nul & {command}"),
                ]
            }
        }
    }
}

fn encode_powershell_command(script: &str) -> String {
    base64::engine::general_purpose::STANDARD.encode(
        script
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect::<Vec<_>>(),
    )
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

    fn decode_powershell_command(encoded: &str) -> String {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .expect("powershell encoded command is standard base64");
        let units = bytes
            .chunks_exact(2)
            .map(|chunk| u16::from_le_bytes([chunk[0], chunk[1]]))
            .collect::<Vec<_>>();
        String::from_utf16(&units).expect("powershell encoded command is UTF-16LE")
    }

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

    #[test]
    fn powershell_uses_encoded_command_and_preserves_json() {
        let runtime = ShellRuntime {
            kind: ShellKind::PowerShell,
            program: PathBuf::from("pwsh.exe"),
        };
        let command = r#"curl.exe -sS http://127.0.0.1:4096/mcp -H "Content-Type: application/json" --data-raw '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}'"#;
        let args = runtime.command_args(command, false);
        assert_eq!(
            &args[..6],
            &[
                "-NoLogo",
                "-NoProfile",
                "-NonInteractive",
                "-ExecutionPolicy",
                "Bypass",
                "-EncodedCommand",
            ]
        );
        assert!(!args.iter().any(|arg| arg == "-Command" || arg == "-NoExit"));
        let script = decode_powershell_command(&args[6]);
        assert!(script.contains(command), "{script}");
        assert!(script.contains("$OutputEncoding"));
        assert_eq!(
            runtime.command_args(command, true)[5],
            "-EncodedCommand"
        );
    }

    #[test]
    fn posix_and_cmd_keep_plain_command_argv() {
        let posix = ShellRuntime {
            kind: ShellKind::Posix,
            program: PathBuf::from("/bin/sh"),
        };
        assert_eq!(
            posix.command_args("echo hi", true),
            vec!["-lc".to_string(), "echo hi".to_string()]
        );
        let cmd = ShellRuntime {
            kind: ShellKind::Cmd,
            program: PathBuf::from("cmd.exe"),
        };
        assert_eq!(
            cmd.command_args("echo hi", false),
            vec![
                "/d".to_string(),
                "/s".to_string(),
                "/c".to_string(),
                "chcp 65001>nul & echo hi".to_string(),
            ]
        );
    }
}
