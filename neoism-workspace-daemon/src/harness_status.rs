use neoism_protocol::pty::{HarnessAuth, HarnessStatus};
use std::{path::PathBuf, process::Stdio, time::Duration};
use tokio::{io::AsyncReadExt, process::Command};

pub(crate) async fn probe() -> Vec<HarnessStatus> {
    let (claude, codex) = tokio::join!(status("claude", "claude"), status("codex", "codex"));
    let mut rows = vec![claude, codex];
    for (id, executable) in [("antigravity", "agy"), ("cursor", "cursor-agent"), ("opencode", "opencode"),
        ("grok", "grok"), ("kiro", "kiro-cli"), ("fx", "fx")] {
        rows.push(HarnessStatus { id: id.into(), installed: executable_on_path(executable).is_some(), auth: HarnessAuth::Unknown });
    }
    rows
}

fn executable_on_path(name: &str) -> Option<PathBuf> {
    std::env::split_paths(&std::env::var_os("PATH")?).filter(|dir| dir.is_absolute()).find_map(|dir| {
        #[cfg(windows)]
        let names = [format!("{name}.exe"), format!("{name}.cmd"), format!("{name}.bat")];
        #[cfg(not(windows))]
        let names = [name];
        names.iter().map(|file| dir.join(file)).find(|path| path.metadata().is_ok_and(|meta| {
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                meta.is_file() && meta.permissions().mode() & 0o111 != 0
            }
            #[cfg(not(unix))]
            { meta.is_file() }
        }))
    })
}

async fn status(id: &str, name: &str) -> HarnessStatus {
    let executable = executable_on_path(name);
    let auth = match &executable {
        Some(program) => tokio::time::timeout(Duration::from_secs(5), auth_status(id, program)).await.ok().flatten(),
        None => None,
    }.unwrap_or(HarnessAuth::Unknown);
    HarnessStatus { id: id.into(), installed: executable.is_some(), auth }
}

async fn auth_status(id: &str, program: &std::path::Path) -> Option<HarnessAuth> {
    let args: &[&str] = if id == "claude" { &["auth", "status", "--json"] } else { &["login", "status"] };
    let mut command = Command::new(program);
    command.args(args).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).kill_on_drop(true);
    // Avoid project-local configuration and hooks, but keep the daemon account's auth environment.
    #[cfg(unix)]
    command.current_dir("/");
    #[cfg(windows)]
    if let Some(root) = std::env::var_os("SystemRoot") { command.current_dir(root); }
    let mut child = command.spawn().ok()?;
    let (mut stdout, mut stderr) = (Vec::new(), Vec::new());
    let (mut out, mut err) = (child.stdout.take()?.take(16_384), child.stderr.take()?.take(16_384));
    let (exit, _, _) = tokio::try_join!(child.wait(), out.read_to_end(&mut stdout), err.read_to_end(&mut stderr)).ok()?;
    Some(parse_auth_status(id, exit.code(), &stdout, &stderr))
}

fn parse_auth_status(id: &str, exit: Option<i32>, stdout: &[u8], stderr: &[u8]) -> HarnessAuth {
    if id == "claude" {
        return match (exit, serde_json::from_slice::<serde_json::Value>(stdout).ok().and_then(|v| v["loggedIn"].as_bool())) {
            (Some(0), Some(true)) => HarnessAuth::SignedIn,
            (Some(0 | 1), Some(false)) => HarnessAuth::SignedOut,
            _ => HarnessAuth::Unknown,
        };
    }
    for data in [stdout, stderr] {
        for line in String::from_utf8_lossy(data).lines().map(str::trim) {
            match exit {
                Some(0) if line == "Logged in" || line.starts_with("Logged in using ") => return HarnessAuth::SignedIn,
                Some(1) if line == "Not logged in" => return HarnessAuth::SignedOut,
                _ => {}
            }
        }
    }
    HarnessAuth::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn harness_status_only_uses_confirmed_login_results() {
        assert_eq!(parse_auth_status("claude", Some(0), br#"{"loggedIn":true,"email":"ignored@example.test"}"#, b""), HarnessAuth::SignedIn);
        assert_eq!(parse_auth_status("claude", Some(1), br#"{"loggedIn":false}"#, b""), HarnessAuth::SignedOut);
        assert_eq!(parse_auth_status("codex", Some(0), b"", b"Logged in using ChatGPT\n"), HarnessAuth::SignedIn);
        assert_eq!(parse_auth_status("codex", Some(1), b"", b"Not logged in\n"), HarnessAuth::SignedOut);
        assert_eq!(parse_auth_status("codex", Some(1), b"", b"Auth status unavailable"), HarnessAuth::Unknown);
    }
}