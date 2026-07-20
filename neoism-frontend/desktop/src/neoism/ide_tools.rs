use std::fs;
use std::path::Path;
use std::process::Command;

#[cfg(windows)]
use std::ffi::OsString;

pub fn agent_command_exists(binary: &str) -> bool {
    command_exists(binary)
}

#[derive(Clone, Copy, Debug)]
pub struct AgentInstallSpec {
    pub binary: &'static str,
    pub display_name: &'static str,
    pub manager: &'static str,
}

pub fn agent_install_spec(id: &str) -> Option<AgentInstallSpec> {
    match id {
        "claude" => Some(AgentInstallSpec {
            binary: "claude",
            display_name: "Claude Code",
            manager: "npm",
        }),
        "codex" => Some(AgentInstallSpec {
            binary: "codex",
            display_name: "Codex",
            manager: "npm",
        }),
        "opencode" => Some(AgentInstallSpec {
            binary: "opencode",
            display_name: "OpenCode",
            manager: "curl | bash",
        }),
        _ => None,
    }
}

pub fn install_agent(id: &str) -> Result<String, String> {
    let spec = agent_install_spec(id)
        .ok_or_else(|| format!("Neoism does not know how to install `{id}`."))?;
    let message = match id {
        "claude" => install_npm_global(&spec, "@anthropic-ai/claude-code"),
        "codex" => install_npm_global(&spec, "@openai/codex"),
        "opencode" => install_via_shell_pipe(&spec, "https://opencode.ai/install"),
        _ => Err(format!("unsupported agent `{id}`")),
    }?;

    if !command_exists(spec.binary) {
        return Err(format!(
            "{} install finished, but `{}` is still not on PATH. The package may have installed somewhere not on your shell PATH — open a new shell or extend PATH and retry.",
            spec.display_name, spec.binary,
        ));
    }
    Ok(format!(
        "{message}\n\nVerified `{}` is on PATH.",
        spec.binary
    ))
}

/// Install a package globally via the user's existing `npm`. Uses the
/// default global prefix (whatever `npm config get prefix` returns)
/// so the binary lands somewhere the user's shell already discovers
/// — same strategy `npm install -g @foo/bar` uses by hand.
fn install_npm_global(spec: &AgentInstallSpec, package: &str) -> Result<String, String> {
    ensure_command(
        "npm",
        "Install Node.js/npm first, then retry the install from Neoism.",
    )?;
    let mut cmd = Command::new("npm");
    cmd.arg("install").arg("-g").arg(package);
    run_command(&mut cmd, &format!("npm install -g {package}"))?;
    Ok(format!(
        "Installed {} via `npm install -g {package}`.",
        spec.display_name,
    ))
}

/// `curl -fsSL <url> | bash` — the de-facto install path for tools
/// like opencode that ship a hosted shell installer. We don't shell
/// out to a literal pipe; instead we read the body with curl, then
/// hand it to bash on stdin.
fn install_via_shell_pipe(spec: &AgentInstallSpec, url: &str) -> Result<String, String> {
    ensure_command(
        "curl",
        "Install curl first, then retry the install from Neoism.",
    )?;
    ensure_command(
        "bash",
        "bash is required to run the upstream installer script.",
    )?;
    let curl_out = Command::new("curl")
        .args(["-fsSL", url])
        .output()
        .map_err(|err| format!("curl failed: {err}"))?;
    if !curl_out.status.success() {
        return Err(format!(
            "curl could not fetch {url}: {}",
            String::from_utf8_lossy(&curl_out.stderr).trim()
        ));
    }
    use std::io::Write;
    let mut child = Command::new("bash")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|err| format!("bash spawn failed: {err}"))?;
    if let Some(mut stdin) = child.stdin.take() {
        stdin
            .write_all(&curl_out.stdout)
            .map_err(|err| format!("failed to feed installer to bash: {err}"))?;
    }
    let out = child
        .wait_with_output()
        .map_err(|err| format!("bash wait failed: {err}"))?;
    if !out.status.success() {
        return Err(format!(
            "{} installer exited with status {}: {}",
            spec.display_name,
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(format!(
        "Ran the upstream {} installer from {url}.",
        spec.display_name
    ))
}

fn run_command(cmd: &mut Command, label: &str) -> Result<(), String> {
    let output = cmd
        .output()
        .map_err(|err| format!("failed to run {label}: {err}"))?;

    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let details = if !stderr.is_empty() {
        stderr
    } else if !stdout.is_empty() {
        stdout
    } else {
        format!("exit status {}", output.status)
    };
    Err(format!("{label} failed: {details}"))
}

fn ensure_command(command: &str, missing_message: &str) -> Result<(), String> {
    if command_exists(command) {
        Ok(())
    } else {
        Err(missing_message.to_string())
    }
}

fn command_exists(command: &str) -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| executable_candidate(&dir, command))
}

#[cfg(windows)]
fn executable_candidate(dir: &Path, command: &str) -> bool {
    let pathext =
        std::env::var_os("PATHEXT").unwrap_or_else(|| OsString::from(".EXE;.CMD;.BAT"));
    let exts = pathext.to_string_lossy();
    exts.split(';')
        .map(str::trim)
        .filter(|ext| !ext.is_empty())
        .any(|ext| dir.join(format!("{command}{ext}")).is_file())
        || dir.join(command).is_file()
}

#[cfg(not(windows))]
fn executable_candidate(dir: &Path, command: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;

    let path = dir.join(command);
    let Ok(meta) = fs::metadata(path) else {
        return false;
    };
    meta.is_file() && meta.permissions().mode() & 0o111 != 0
}
