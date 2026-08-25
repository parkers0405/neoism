#[cfg(not(windows))]
use std::collections::HashMap;
use std::process::Stdio;
#[cfg(not(windows))]
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use serde_json::{json, Value};
use tokio::process::Command;
#[cfg(not(windows))]
use tokio::sync::Mutex;

use super::args::{optional_string, required_string, usize_arg};
use super::paths::{display_path, existing_project_path};
use super::{process, shell_scan, truncate, ToolContext, ToolExecutionResult};

const MAX_CAPTURE_BYTES_PER_STREAM: usize = 1024 * 1024;

/// One runtime's cached login-shell environment. The single-entry cache is
/// bounded and is invalidated when the resolved shell changes or its TTL
/// expires. Running `$SHELL -lc <cmd>` for every tool call re-sources
/// the entire login profile each time (`path_helper`, Homebrew init,
/// nvm/pyenv/rbenv, oh-my-zsh …) — commonly 0.5–2s of pure startup per
/// command on macOS. Instead we source the profile a single time, cache
/// the resulting environment, and run each command in a NON-login `-c`
/// shell that inherits it: same PATH/tooling, none of the per-command
/// re-sourcing tax.
#[cfg(not(windows))]
const LOGIN_ENV_TTL: Duration = Duration::from_secs(5 * 60);

#[cfg(not(windows))]
struct CachedLoginEnvironment {
    shell: String,
    captured_at: std::time::Instant,
    environment: Arc<HashMap<String, String>>,
}

#[cfg(not(windows))]
#[derive(Default)]
pub(crate) struct LoginShellEnvironment {
    cached: Mutex<Option<CachedLoginEnvironment>>,
}

#[cfg(windows)]
#[derive(Default)]
pub(crate) struct LoginShellEnvironment;

#[cfg(not(windows))]
impl LoginShellEnvironment {
    pub(crate) async fn get(&self, shell: &str) -> Arc<HashMap<String, String>> {
        let mut cached = self.cached.lock().await;
        if let Some(entry) = cached.as_ref() {
            if entry.shell == shell && entry.captured_at.elapsed() < LOGIN_ENV_TTL {
                return entry.environment.clone();
            }
        }
        let environment = Arc::new(capture_login_env(shell.to_string()).await);
        *cached = Some(CachedLoginEnvironment {
            shell: shell.to_string(),
            captured_at: std::time::Instant::now(),
            environment: environment.clone(),
        });
        environment
    }

    #[cfg(test)]
    pub(crate) async fn cache_len(&self) -> usize {
        usize::from(self.cached.lock().await.is_some())
    }
}

#[cfg(not(windows))]
async fn capture_login_env(shell: String) -> HashMap<String, String> {
    let captured = Command::new(&shell)
        .arg("-lc")
        .arg("env")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .await;
    let Ok(output) = captured else {
        return HashMap::new();
    };
    if !output.status.success() {
        return HashMap::new();
    }
    let mut env = parse_env(&String::from_utf8_lossy(&output.stdout));
    // Volatile per-invocation vars must not be pinned to capture time —
    // the real cwd is set via `current_dir`, and the shell manages these.
    for volatile in ["PWD", "OLDPWD", "SHLVL", "_"] {
        env.remove(volatile);
    }
    env
}

#[cfg(not(windows))]
fn parse_env(text: &str) -> HashMap<String, String> {
    let mut env = HashMap::new();
    let mut last_key: Option<String> = None;
    for line in text.split('\n') {
        if let Some(eq) = line.find('=') {
            let key = &line[..eq];
            if is_env_name(key) {
                env.insert(key.to_string(), line[eq + 1..].to_string());
                last_key = Some(key.to_string());
                continue;
            }
        }
        // A line with no leading `KEY=` continues the previous variable's
        // multi-line value — reattach it so such values survive intact.
        if let Some(key) = &last_key {
            if let Some(value) = env.get_mut(key) {
                value.push('\n');
                value.push_str(line);
            }
        }
    }
    env
}

#[cfg(not(windows))]
fn is_env_name(candidate: &str) -> bool {
    let mut bytes = candidate.bytes();
    match bytes.next() {
        Some(byte) if byte == b'_' || byte.is_ascii_alphabetic() => {}
        _ => return false,
    }
    bytes.all(|byte| byte == b'_' || byte.is_ascii_alphanumeric())
}

pub(super) async fn bash_tool(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let command = required_string(&arguments, "command")?.to_string();
    let cwd = if let Some(workdir) = optional_string(&arguments, "workdir") {
        existing_project_path(&context, &workdir)?
    } else {
        context.cwd.clone()
    };
    let project_root = crate::windows_process::canonicalize_path(&context.cwd)
        .with_context(|| {
            format!(
                "failed to resolve project directory {}",
                context.cwd.display()
            )
        })?;
    let scan = shell_scan::scan(
        &command,
        &cwd,
        &project_root,
        context.utilities().shell.kind(),
    );
    for dir in &scan.external_dirs {
        context.ensure_explicit_allowed("external_directory", dir)?;
    }
    for pattern in &scan.command_patterns {
        context.ensure_allowed("bash", pattern)?;
    }
    let snapshot_before = crate::snapshot::bash_before(&context.cwd);
    let timeout_ms = usize_arg(&arguments, "timeout").unwrap_or(120_000).max(1) as u64;
    let description =
        optional_string(&arguments, "description").unwrap_or_else(|| command.clone());
    let runtime = &context.utilities().shell;
    let shell = runtime.program().to_string_lossy().into_owned();
    // Non-login `-c` + this runtime's cached login environment: the
    // profile is already resolved, so we skip re-sourcing it per command.
    #[cfg(not(windows))]
    let login_env = tokio::select! {
        env = context.utilities().login_shell_environment.get(&shell) => env,
        _ = process::wait_for_cancel(context.cancel.clone()) => {
            anyhow::bail!("{} command aborted\n(no output)", runtime.display_name());
        }
    };
    let mut process = Command::new(&shell);
    runtime.apply_command(&mut process, &command, false);
    process
        .current_dir(&cwd)
        .env("TERM", "xterm-256color")
        .env("NEOISM_TERMINAL", "1")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(not(windows))]
    process.envs(login_env.iter());
    process.envs(context.env.clone());
    process::set_new_process_group(&mut process);
    let mut child = process
        .spawn()
        .with_context(|| format!("failed to spawn shell {shell}"))?;
    let child_id = child.id();
    let stdout_task =
        process::read_child_output(child.stdout.take(), MAX_CAPTURE_BYTES_PER_STREAM);
    let stderr_task =
        process::read_child_output(child.stderr.take(), MAX_CAPTURE_BYTES_PER_STREAM);
    let timeout = tokio::time::sleep(Duration::from_millis(timeout_ms));
    tokio::pin!(timeout);
    let wait_result: anyhow::Result<std::process::ExitStatus> = tokio::select! {
        status = child.wait() => {
            status.with_context(|| format!("failed to wait for shell {shell}"))
        }
        _ = &mut timeout => {
            process::terminate_child(&mut child, child_id).await;
            Err(anyhow::anyhow!("{} command timed out after {timeout_ms}ms", runtime.display_name()))
        }
        _ = process::wait_for_cancel(context.cancel.clone()) => {
            process::terminate_child(&mut child, child_id).await;
            Err(anyhow::anyhow!("{} command aborted", runtime.display_name()))
        }
    };

    let stdout = stdout_task.await??;
    let stderr = stderr_task.await??;
    let capture_truncated = stdout.truncated || stderr.truncated;
    let stdout = String::from_utf8_lossy(&stdout.bytes);
    let stderr = String::from_utf8_lossy(&stderr.bytes);
    let mut rendered = String::new();
    if !stdout.is_empty() {
        rendered.push_str(&stdout);
    }
    if !stderr.is_empty() {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        rendered.push_str(&stderr);
    }
    if rendered.is_empty() {
        rendered.push_str("(no output)");
    }
    if capture_truncated {
        rendered.push_str(
            "\n\n[process output capture truncated at the 1 MiB per-stream safety limit]",
        );
    }

    let status = match wait_result {
        Ok(status) => status,
        Err(error) => {
            let rendered = truncate::truncate_output(&rendered)?.output;
            anyhow::bail!("{error}\n{rendered}")
        }
    };
    let exit = status.code();
    if !status.success() {
        let rendered = truncate::truncate_output(&rendered)?.output;
        anyhow::bail!("bash command failed with status {:?}\n{}", exit, rendered);
    }
    let snapshots = crate::snapshot::bash_after(snapshot_before);
    let mut metadata = json!({
        "command": command,
        "description": description.clone(),
        "exit": exit,
        "timeout": timeout_ms,
        "workdir": display_path(&context.cwd, &cwd),
        "truncated": capture_truncated,
        "captureTruncated": capture_truncated,
        "alwaysPatterns": scan.always_patterns.into_iter().collect::<Vec<_>>(),
        "commandPatterns": scan.command_patterns.into_iter().collect::<Vec<_>>(),
        "externalDirectories": scan.external_dirs.into_iter().collect::<Vec<_>>(),
    });
    crate::snapshot::add_metadata_snapshots(&mut metadata, snapshots);

    Ok(ToolExecutionResult {
        title: description.clone(),
        output: rendered,
        metadata: Some(metadata),
    })
}

#[cfg(all(test, not(windows)))]
mod tests {
    use super::*;

    #[tokio::test]
    async fn login_environment_cache_stays_single_entry_when_shell_changes() {
        let cache = LoginShellEnvironment::default();
        let _ = cache.get("/definitely/missing-shell-a").await;
        let _ = cache.get("/definitely/missing-shell-b").await;
        assert_eq!(cache.cache_len().await, 1);
    }

    #[tokio::test]
    async fn expired_login_environment_is_recaptured() {
        let cache = LoginShellEnvironment::default();
        let first = cache.get("/definitely/missing-shell").await;
        cache.cached.lock().await.as_mut().unwrap().captured_at =
            std::time::Instant::now() - LOGIN_ENV_TTL;
        let second = cache.get("/definitely/missing-shell").await;
        assert!(!Arc::ptr_eq(&first, &second));
        assert_eq!(cache.cache_len().await, 1);
    }
}
