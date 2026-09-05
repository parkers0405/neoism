//! Session registry backed by real PTYs (via `neoism-terminal-pty`).
//!
//! Each session owns a `PtySession` plus a reader task that pumps PTY
//! output into a registry-wide broadcast channel. Every WebSocket
//! connection subscribes to the same registry so PTY sessions survive
//! client reconnects and can be viewed by multiple clients.
//!
//! ## Workspace-session vs PTY-session id
//!
//! The id this registry mints in [`SessionRegistry::create`] names a
//! *live shell process*. It is distinct from a workspace-registry tab
//! id ([`neoism_protocol::workspace::SessionSummary::id`], owned by
//! [`crate::workspace::WorkspaceManager`]), which names a *logical tab*
//! that outlives any single shell. The two were historically unlinked,
//! so a roaming client could not tell which live PTY backed a tab.
//!
//! [`SessionRegistry::link_workspace_session`] records the bridge from
//! this side (PTY id -> workspace tab id); the workspace manager keeps
//! the inverse map. Both registries sit on `AppState`, so a caller
//! holding both can resolve a tab to its shell and back.
//!
//! ## Respawn-in-cwd policy
//!
//! PTYs never migrate (locked architecture decision #3). On a host move
//! or a tab `cwd` change the live shell is dropped and a fresh one is
//! spawned in the recorded `cwd` — `create` already takes a `cwd`, so a
//! respawn is just another `create` followed by a re-`link`. Agents
//! resume from serialized state.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use dashmap::DashMap;
use neoism_protocol::pty::{ClientMessage, ServerMessage};
use neoism_terminal_pty::{PtySession, PtySessionConfig};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use uuid::Uuid;

/// How long the reader task sleeps between empty (`Ok(0)`) reads. Small
/// enough to feel responsive, large enough not to spin a tokio worker.
const READER_IDLE_SLEEP: Duration = Duration::from_millis(5);

/// Minimum gap between foreground-cwd polls per session. The reader loop
/// already wakes every few ms; this throttles the `/proc/<pgrp>/cwd`
/// readlink + `tcgetpgrp` syscalls to a human-perceptible cadence so a
/// `cd` re-roots the tree promptly without spinning syscalls on idle
/// shells. Only a *changed* cwd is broadcast, so steady state is silent.
const CWD_POLL_INTERVAL: Duration = Duration::from_millis(400);

/// How many bytes the reader task pulls per read syscall.
const READ_BUFFER_SIZE: usize = 4096;
const OUTPUT_BROADCAST_CAPACITY: usize = 1024;
const BACKLOG_LIMIT_BYTES: usize = 1024 * 1024;

struct SessionEntry {
    pty: Arc<Mutex<PtySession>>,
    backlog: Arc<Mutex<Vec<u8>>>,
    workspace_root: Option<String>,
    /// Workspace-registry tab id this live PTY backs, once bridged. See
    /// the module-level "Workspace-session vs PTY-session id" note. A
    /// PTY can exist before it is attached to a logical tab (web spawns
    /// a shell first, then binds it), so this starts `None` and is set
    /// by [`SessionRegistry::link_workspace_session`]. It rides on the
    /// entry so closing the PTY drops the link with no extra bookkeeping.
    workspace_session_id: Option<String>,
}

#[derive(Clone)]
pub struct SessionRegistry {
    inner: Arc<DashMap<String, SessionEntry>>,
    output_tx: broadcast::Sender<ServerMessage>,
}

impl SessionRegistry {
    /// Construct a registry plus an initial output receiver. Additional
    /// websocket clients should call [`Self::subscribe`] on the shared
    /// registry stored in daemon state.
    pub fn new() -> (Self, broadcast::Receiver<ServerMessage>) {
        let (tx, rx) = broadcast::channel(OUTPUT_BROADCAST_CAPACITY);
        (Self::from_sender(tx), rx)
    }

    /// Construct a shared registry when the caller does not need the
    /// first receiver immediately.
    pub fn shared() -> Self {
        Self::new().0
    }

    fn from_sender(output_tx: broadcast::Sender<ServerMessage>) -> Self {
        Self {
            inner: Arc::new(DashMap::new()),
            output_tx,
        }
    }

    /// Subscribe to all future PTY lifecycle and output messages.
    pub fn subscribe(&self) -> broadcast::Receiver<ServerMessage> {
        self.output_tx.subscribe()
    }

    /// Synthetic attach stream for a new client. `PtyCreated` announces
    /// every live daemon-owned PTY; `PtyOutput` replays the retained
    /// in-memory backlog for that session.
    pub fn backlog_messages(&self) -> Vec<ServerMessage> {
        let mut messages = Vec::new();
        for entry in self.inner.iter() {
            let session_id = entry.key().clone();
            messages.push(ServerMessage::PtyCreated {
                session_id: session_id.clone(),
                workspace_root: entry.value().workspace_root.clone(),
            });
            let bytes = entry.value().backlog.lock().clone();
            if !bytes.is_empty() {
                messages.push(ServerMessage::PtyOutput { session_id, bytes });
            }
        }
        messages
    }

    /// Bridge a live PTY to the workspace tab it backs. Returns `false`
    /// if `pty_session_id` is unknown (the shell already exited, or the
    /// id is wrong) — callers should treat that as "respawn needed".
    /// Idempotent and last-write-wins, so re-binding a tab to a freshly
    /// respawned shell just overwrites the previous association.
    ///
    /// The inverse half of the link lives on
    /// [`crate::workspace::WorkspaceManager::link_pty_session`]; a caller
    /// holding both (they share `AppState`) keeps the two in step.
    pub fn link_workspace_session(
        &self,
        pty_session_id: &str,
        workspace_session_id: String,
    ) -> bool {
        match self.inner.get_mut(pty_session_id) {
            Some(mut entry) => {
                entry.workspace_session_id = Some(workspace_session_id);
                true
            }
            None => {
                tracing::warn!(
                    %pty_session_id,
                    "link_workspace_session for unknown pty session"
                );
                false
            }
        }
    }

    /// Resolve the workspace tab id this live PTY is bridged to, if any.
    /// `None` means the shell exists but has not been attached to a tab.
    pub fn workspace_session_for(&self, pty_session_id: &str) -> Option<String> {
        self.inner
            .get(pty_session_id)
            .and_then(|entry| entry.workspace_session_id.clone())
    }

    /// Reverse lookup: find the live PTY currently backing workspace tab
    /// `workspace_session_id`. `None` means the tab has no live shell and
    /// the caller should respawn one in the tab's recorded `cwd`.
    pub fn pty_for_workspace_session(
        &self,
        workspace_session_id: &str,
    ) -> Option<String> {
        self.inner.iter().find_map(|entry| {
            (entry.value().workspace_session_id.as_deref() == Some(workspace_session_id))
                .then(|| entry.key().clone())
        })
    }

    /// Test-only constructor that drops the output stream.
    #[cfg(test)]
    pub fn new_for_test() -> Self {
        Self::new().0
    }

    /// Handle a client message. Control replies (PtyCreated, PtyClosed,
    /// Error) are returned synchronously; PtyOutput flows through the
    /// registry's output channel as bytes arrive from the shell.
    pub fn handle(&self, msg: ClientMessage) -> Vec<ServerMessage> {
        match msg {
            ClientMessage::CreatePty {
                cwd,
                cols,
                rows,
                shell,
            } => self.create(cwd, cols, rows, shell),
            ClientMessage::PtyInput { session_id, bytes } => {
                self.input(session_id, bytes)
            }
            ClientMessage::Resize {
                session_id,
                cols,
                rows,
            } => self.resize(session_id, cols, rows),
            ClientMessage::ClosePty { session_id } => self.close(session_id),
            ClientMessage::AttachPty { session_id } => self.attach(session_id),
        }
    }

    /// One-shot backlog replay for a client binding to an
    /// already-running session mid-stream (8C adopt). The reply goes
    /// only to the requesting connection; live broadcasts continue
    /// unaffected. A racing live chunk can, worst case, appear once
    /// out of order around the snapshot — cosmetic, self-corrects on
    /// the next output.
    fn attach(&self, session_id: String) -> Vec<ServerMessage> {
        let Some(entry) = self.inner.get(&session_id) else {
            return vec![ServerMessage::Error {
                message: format!("unknown session {session_id}"),
            }];
        };
        let bytes = entry.value().backlog.lock().clone();
        if bytes.is_empty() {
            return Vec::new();
        }
        vec![ServerMessage::PtyOutput { session_id, bytes }]
    }

    fn create(
        &self,
        cwd: Option<String>,
        cols: u16,
        rows: u16,
        explicit_shell: Option<String>,
    ) -> Vec<ServerMessage> {
        let (shell, args) = shell_for_create(explicit_shell);
        let mut env: Vec<(String, String)> = std::env::vars().collect();
        // Headless daemons (containers, systemd units) often run with no
        // TERM at all — child shells then break every curses program and
        // even `clear` ("TERM environment variable not set"). The client
        // renders xterm-256color escapes, so advertise that when the
        // daemon's own environment has nothing better.
        if !env.iter().any(|(key, _)| key == "TERM") {
            env.push(("TERM".to_string(), "xterm-256color".to_string()));
        }
        ensure_neoism_pty_env(&mut env);
        let config = PtySessionConfig {
            shell,
            args,
            cwd: cwd.map(std::path::PathBuf::from),
            env,
            cols,
            rows,
        };

        let pty = match PtySession::spawn(config) {
            Ok(p) => p,
            Err(err) => {
                tracing::warn!(error = %err, "failed to spawn pty");
                return vec![ServerMessage::Error {
                    message: format!("failed to spawn pty: {err}"),
                }];
            }
        };

        let session_id = Uuid::new_v4().to_string();
        let backlog = Arc::new(Mutex::new(Vec::new()));
        let workspace_root = Some(workspace_root_label());
        let entry = SessionEntry {
            pty: Arc::new(Mutex::new(pty)),
            backlog: backlog.clone(),
            workspace_root: workspace_root.clone(),
            // Bridged later by `link_workspace_session` once the client
            // attaches this shell to a workspace tab.
            workspace_session_id: None,
        };
        let pty_arc = entry.pty.clone();
        self.inner.insert(session_id.clone(), entry);
        tracing::info!(%session_id, cols, rows, "spawned pty session");

        let output_tx = self.output_tx.clone();
        let id_for_task = session_id.clone();
        let registry_for_cleanup = self.inner.clone();
        tokio::task::spawn_blocking(move || {
            reader_loop(
                id_for_task,
                pty_arc,
                backlog,
                output_tx,
                registry_for_cleanup,
            );
        });

        // Creation belongs only to the requesting client. Broadcasting this
        // control reply let another client with a pending terminal consume
        // the session id and bind its input to the wrong shell. Live output
        // remains broadcast by `reader_loop`, and late explicit attaches use
        // `backlog_messages` / `AttachPty`.
        vec![ServerMessage::PtyCreated {
            session_id,
            workspace_root,
        }]
    }

    fn input(&self, session_id: String, bytes: Vec<u8>) -> Vec<ServerMessage> {
        let Some(entry) = self.inner.get(&session_id) else {
            tracing::warn!(%session_id, "input for unknown session");
            return vec![ServerMessage::Error {
                message: format!("unknown session {session_id}"),
            }];
        };
        let byte_count = bytes.len();
        if let Err(err) = entry.pty.lock().write(&bytes) {
            tracing::warn!(%session_id, error = %err, "pty write failed");
            return vec![ServerMessage::Error {
                message: format!("pty write failed: {err}"),
            }];
        }
        tracing::trace!(%session_id, byte_count, "forwarded pty input");
        Vec::new()
    }

    fn resize(&self, session_id: String, cols: u16, rows: u16) -> Vec<ServerMessage> {
        let Some(entry) = self.inner.get(&session_id) else {
            return vec![ServerMessage::Error {
                message: format!("unknown session {session_id}"),
            }];
        };
        if let Err(err) = entry.pty.lock().resize(cols, rows) {
            tracing::warn!(%session_id, error = %err, "pty resize failed");
            return vec![ServerMessage::Error {
                message: format!("pty resize failed: {err}"),
            }];
        }
        tracing::debug!(%session_id, cols, rows, "resized pty");
        Vec::new()
    }

    fn close(&self, session_id: String) -> Vec<ServerMessage> {
        let removed = self.inner.remove(&session_id);
        match removed {
            Some((_, _)) => {
                tracing::info!(%session_id, "closed pty session");
                let _ = self.output_tx.send(ServerMessage::PtyClosed {
                    session_id: session_id.clone(),
                    exit_code: None,
                });
                vec![ServerMessage::PtyClosed {
                    session_id,
                    exit_code: None,
                }]
            }
            None => vec![ServerMessage::Error {
                message: format!("unknown session {session_id}"),
            }],
        }
    }
}

fn ensure_neoism_pty_env(env: &mut Vec<(String, String)>) {
    if let Some((_, value)) = env.iter_mut().find(|(key, _)| key == "NEOISM") {
        *value = "1".to_string();
    } else {
        env.push(("NEOISM".to_string(), "1".to_string()));
    }
}

#[cfg(test)]
mod pty_env_tests {
    #[test]
    fn daemon_pty_environment_preserves_shell_env_and_marks_neoism() {
        let mut env = vec![("PATH".to_string(), "/bin".to_string()), ("TERM".to_string(), "screen".to_string())];
        super::ensure_neoism_pty_env(&mut env);
        assert!(env.contains(&("PATH".into(), "/bin".into())));
        assert!(env.contains(&("TERM".into(), "screen".into())));
        assert!(env.contains(&("NEOISM".into(), "1".into())));
    }
}

fn workspace_root_label() -> String {
    let root = crate::files::workspace_root();
    if root.is_absolute() {
        return root.to_string_lossy().into_owned();
    }
    std::env::current_dir()
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(root)
        .to_string_lossy()
        .into_owned()
}

fn shell_for_create(explicit_shell: Option<String>) -> (Option<String>, Vec<String>) {
    #[cfg(windows)]
    {
        let requested = explicit_shell.unwrap_or_else(default_windows_shell);
        return (Some(requested), Vec::new());
    }

    #[cfg(not(windows))]
    {
        // Only zsh and bash get OSC 133 block-prompt integration (see
        // `block_shell_for_spawn`). A shell without it makes every command
        // show as perpetually running — the block's timer never stops
        // because no `OSC 133;D` is ever emitted. So prefer an integrable
        // shell over `$SHELL` whenever one exists, rather than only as a
        // last resort: a daemon launched over `ssh host cmd` runs with a
        // stripped env where `$SHELL` is unset or points at `/bin/sh`, yet
        // the absolute-path probes in `preferred_*` still resolve bash/zsh
        // (they stat fixed paths, so they don't need PATH).
        let requested = explicit_shell
            .or_else(preferred_zsh)
            .or_else(preferred_bash)
            .or_else(|| std::env::var("SHELL").ok())
            .unwrap_or_else(|| "/bin/sh".to_string());

        match block_shell_for_spawn(&requested) {
            Some((program, args)) => (Some(program), args),
            None => (Some(requested), Vec::new()),
        }
    }
}

#[cfg(windows)]
fn default_windows_shell() -> String {
    if let Some(pwsh) = command_in_path("pwsh.exe") {
        return pwsh;
    }

    if let Some(powershell) = command_in_path("powershell.exe") {
        return powershell;
    }

    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

#[cfg(not(windows))]
fn preferred_zsh() -> Option<String> {
    ["/usr/bin/zsh", "/bin/zsh"]
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
        .or_else(|| command_in_path("zsh"))
}

#[cfg(not(windows))]
fn preferred_bash() -> Option<String> {
    ["/usr/bin/bash", "/bin/bash"]
        .iter()
        .find(|path| Path::new(path).exists())
        .map(|path| (*path).to_string())
        .or_else(|| command_in_path("bash"))
}

fn command_in_path(command: &str) -> Option<String> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(command))
        .find(|candidate| candidate.exists())
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

#[cfg(not(windows))]
fn block_shell_for_spawn(shell: &str) -> Option<(String, Vec<String>)> {
    let name = Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())?;
    match name {
        "zsh" => block_zsh_for_spawn(shell),
        "bash" => block_bash_for_spawn(shell),
        _ => None,
    }
}

#[cfg(not(windows))]
fn block_bash_for_spawn(shell: &str) -> Option<(String, Vec<String>)> {
    let dir = std::env::temp_dir().join(format!(
        "neoism-web-shell-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    fs::create_dir_all(&dir).ok()?;
    let bash_rc = dir.join("bashrc");
    let script = block_bash_script();
    fs::write(&bash_rc, script).ok()?;
    Some((
        shell.to_string(),
        vec![
            "--rcfile".to_string(),
            bash_rc.display().to_string(),
            "-i".to_string(),
        ],
    ))
}

#[cfg(not(windows))]
fn block_bash_script() -> &'static str {
    r#"if [ -r "$HOME/.bashrc" ]; then
  . "$HOME/.bashrc"
fi
__neoism_hidden_ps1=$'\001\033]133;A\007\002\001\033]133;B\007\002'
neoism() {
  if [ "${1:-}" = cd ]; then
    shift
    if [ "$#" -gt 1 ]; then printf 'usage: neoism cd [directory]\n' >&2; return 2; fi
    if [ "$#" -eq 0 ]; then builtin cd
    elif [ "$1" = - ]; then builtin cd -
    else builtin cd -- "$1"; fi || return
    printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"
  else
    command neoism "$@"
  fi
}
__neoism_preexec() {
  [ "${__neoism_in_prompt:-0}" = 1 ] && return
  case "$BASH_COMMAND" in
    __neoism_prompt_command*|PS1=*) return ;;
  esac
  printf '\033]133;C\007'
}
__neoism_prompt_command() {
  local __neoism_status=$?
  __neoism_in_prompt=1
  if [ -n "${__neoism_saved_prompt_command:-}" ]; then
    eval "$__neoism_saved_prompt_command"
  fi
  PS1="$__neoism_hidden_ps1"
  __neoism_in_prompt=0
  printf '\033]7;file://%s%s\007' "${HOSTNAME:-localhost}" "$PWD"
  printf '\033]133;D;%d\007' "$__neoism_status"
}
__neoism_saved_prompt_command=${PROMPT_COMMAND:-}
bind '"\C-p": kill-whole-line'
PROMPT_COMMAND=__neoism_prompt_command
PS1="$__neoism_hidden_ps1"
trap '__neoism_preexec' DEBUG
"#
}

#[cfg(not(windows))]
fn block_zsh_for_spawn(shell: &str) -> Option<(String, Vec<String>)> {
    let dir = std::env::temp_dir().join(format!(
        "neoism-web-shell-{}-{}",
        std::process::id(),
        Uuid::new_v4()
    ));
    let zsh_rc_dir = dir.join("zsh");
    fs::create_dir_all(&zsh_rc_dir).ok()?;
    let zsh_rc = zsh_rc_dir.join(".zshrc");
    let script = block_zsh_script(&zsh_rc_dir);
    fs::write(&zsh_rc, script).ok()?;
    Some((
        "env".to_string(),
        vec![
            format!("ZDOTDIR={}", zsh_rc_dir.display()),
            shell.to_string(),
            "-i".to_string(),
        ],
    ))
}

#[cfg(not(windows))]
fn block_zsh_script(zsh_rc_dir: &PathBuf) -> String {
    format!(
        r#"if [ -r "$HOME/.zshrc" ]; then
  source "$HOME/.zshrc"
fi
neoism() {{
  if [ "${{1:-}}" = cd ]; then
    shift
    if [ "$#" -gt 1 ]; then printf 'usage: neoism cd [directory]\n' >&2; return 2; fi
    if [ "$#" -eq 0 ]; then builtin cd
    elif [ "$1" = - ]; then builtin cd -
    else builtin cd -- "$1"; fi || return
    printf '\033]7;file://%s%s\007' "$HOST" "$PWD"
  else
    command neoism "$@"
  fi
}}
__neoism_precmd() {{
  local __neoism_status=$?
  printf '\033]7;file://%s%s\007' "$HOST" "$PWD"
  printf '\033]133;D;%d\007' "$__neoism_status"
  # Emit prompt-ready from precmd as well as PROMPT. Frameworks such as
  # Starship replace PROMPT dynamically after this wrapper loads; keeping
  # A/B here preserves Neoism's composer lifecycle in that case.
  printf '\033]133;A\007\033]133;B\007'
  # This hook is deliberately last. Prompt frameworks run before it and may
  # rebuild PROMPT/RPROMPT on every command; clear those values here so their
  # raw prompt (including command-duration timers) never paints behind the
  # Neoism-owned composer.
  PROMPT=''
  RPROMPT=''
}}
__neoism_preexec() {{
  printf '\033]133;C\007'
}}
typeset -ga precmd_functions
typeset -ga preexec_functions
precmd_functions=(${{precmd_functions:#__neoism_precmd}} __neoism_precmd)
preexec_functions=(${{preexec_functions:#__neoism_preexec}} __neoism_preexec)
bindkey '^P' kill-buffer
unsetopt PROMPT_SP
PROMPT_EOL_MARK=''
PROMPT=''
RPROMPT=''
__neoism_zdotdir="{zsh_dir}"
bash() {{
  command bash "$@"
}}
zsh() {{
  if [ "$#" -eq 0 ]; then
    ZDOTDIR="$__neoism_zdotdir" command zsh -i
  else
    command zsh "$@"
  fi
}}
"#,
        zsh_dir = zsh_rc_dir.display(),
    )
}

fn reader_loop(
    session_id: String,
    pty: Arc<Mutex<PtySession>>,
    backlog: Arc<Mutex<Vec<u8>>>,
    output_tx: broadcast::Sender<ServerMessage>,
    registry: Arc<DashMap<String, SessionEntry>>,
) {
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    // Foreground-cwd tracking state. The fd/pid are stable for the
    // session's lifetime, so grab them once and avoid locking the pty on
    // every poll. `last_cwd_poll` is back-dated so the first iteration
    // emits the shell's initial cwd immediately.
    #[cfg(unix)]
    let (cwd_main_fd, cwd_shell_pid) = {
        let guard = pty.lock();
        (*guard.main_fd(), guard.shell_pid())
    };
    #[cfg(unix)]
    let mut last_cwd: Option<String> = None;
    #[cfg(unix)]
    let mut last_cwd_poll = std::time::Instant::now()
        .checked_sub(CWD_POLL_INTERVAL)
        .unwrap_or_else(std::time::Instant::now);
    loop {
        if !registry.contains_key(&session_id) {
            break;
        }
        #[cfg(unix)]
        if last_cwd_poll.elapsed() >= CWD_POLL_INTERVAL {
            last_cwd_poll = std::time::Instant::now();
            if let Ok(path) =
                neoism_terminal_pty::foreground_process_path(cwd_main_fd, cwd_shell_pid)
            {
                let cwd = path.to_string_lossy().into_owned();
                // Only broadcast genuine changes; a `cd` flips this, an
                // idle shell stays silent.
                if last_cwd.as_deref() != Some(cwd.as_str()) {
                    last_cwd = Some(cwd.clone());
                    let _ = output_tx.send(ServerMessage::SessionCwd {
                        session_id: session_id.clone(),
                        cwd,
                    });
                }
            }
        }
        let read_result = pty.lock().read(&mut buf);
        match read_result {
            Ok(0) => {
                let exited = pty.lock().exit_code();
                if let Some(code) = exited {
                    let _ = output_tx.send(ServerMessage::PtyClosed {
                        session_id: session_id.clone(),
                        exit_code: Some(code),
                    });
                    registry.remove(&session_id);
                    break;
                }
                std::thread::sleep(READER_IDLE_SLEEP);
            }
            Ok(n) => {
                let bytes = buf[..n].to_vec();
                append_backlog(&backlog, &bytes);
                tracing::trace!(%session_id, byte_count = n, "forwarded pty output");
                let _ = output_tx.send(ServerMessage::PtyOutput {
                    session_id: session_id.clone(),
                    bytes,
                });
            }
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(READER_IDLE_SLEEP);
            }
            Err(err) => {
                tracing::warn!(%session_id, error = %err, "pty read error");
                let _ = output_tx.send(ServerMessage::PtyClosed {
                    session_id: session_id.clone(),
                    exit_code: None,
                });
                registry.remove(&session_id);
                break;
            }
        }
    }
}

fn append_backlog(backlog: &Mutex<Vec<u8>>, bytes: &[u8]) {
    let mut backlog = backlog.lock();
    backlog.extend_from_slice(bytes);
    if backlog.len() > BACKLOG_LIMIT_BYTES {
        let extra = backlog.len() - BACKLOG_LIMIT_BYTES;
        backlog.drain(..extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zsh_wrapper_emits_prompt_ready_markers_from_precmd() {
        let script = block_zsh_script(&PathBuf::from("/tmp/neoism-zsh-test"));
        let precmd = script
            .split("__neoism_preexec()")
            .next()
            .expect("precmd section");
        assert!(precmd.contains("133;D;%d"));
        assert!(precmd.contains("133;A\\007\\033]133;B"));
        assert!(precmd.contains("PROMPT=''"));
        assert!(precmd.contains("RPROMPT=''"));
    }

    #[test]
    fn bash_shells_get_full_command_lifecycle_markers() {
        let (program, args) = block_shell_for_spawn("/bin/bash").expect("bash wrapper");
        assert_eq!(program, "/bin/bash");
        assert_eq!(args.first().map(String::as_str), Some("--rcfile"));
        let script = block_bash_script();
        assert!(script.contains("133;C"));
        assert!(script.contains("133;D;%d"));
        assert!(script.contains("133;A\\007\\002\\001\\033]133;B"));
    }

    #[test]
    fn integrated_shells_handle_neoism_cd_in_parent_shell() {
        let bash = block_bash_script();
        let zsh = block_zsh_script(&PathBuf::from("/tmp/neoism-zsh-test"));
        for script in [bash.to_string(), zsh] {
            assert!(script.contains("neoism()"));
            assert!(script.contains("builtin cd"));
            assert!(script.contains("command neoism"));
            assert!(script.contains("033]7;file://"));
        }
    }

    #[test]
    fn bash_wrapper_changes_the_parent_shell_and_preserves_oldpwd() {
        let root = std::env::temp_dir().join(format!(
            "neoism-cd-wrapper-test-{}-{}",
            std::process::id(),
            Uuid::new_v4(),
        ));
        let start = root.join("start");
        let target = root.join("target with spaces");
        std::fs::create_dir_all(&start).unwrap();
        std::fs::create_dir_all(&target).unwrap();
        let rc = root.join("bashrc");
        std::fs::write(&rc, block_bash_script()).unwrap();
        let status = std::process::Command::new("bash")
            .args(["--noprofile", "--norc", "-c"])
            .arg(r#"set -u
source "$1"
builtin cd "$2"
neoism cd "$3"
[ "$PWD" = "$3" ]
neoism cd - >/dev/null
[ "$PWD" = "$2" ]"#)
            .arg("neoism-cd-test")
            .arg(&rc)
            .arg(&start)
            .arg(&target)
            .env("HOME", &root)
            .status()
            .unwrap();
        let _ = std::fs::remove_dir_all(&root);
        assert!(status.success());
    }

    #[test]
    fn unknown_session_input_errors() {
        let reg = SessionRegistry::new_for_test();
        let resp = reg.handle(ClientMessage::PtyInput {
            session_id: "nope".into(),
            bytes: vec![],
        });
        assert!(matches!(resp.first(), Some(ServerMessage::Error { .. })));
    }

    #[test]
    fn unknown_session_resize_errors() {
        let reg = SessionRegistry::new_for_test();
        let resp = reg.handle(ClientMessage::Resize {
            session_id: "nope".into(),
            cols: 80,
            rows: 24,
        });
        assert!(matches!(resp.first(), Some(ServerMessage::Error { .. })));
    }

    #[test]
    fn close_unknown_session_errors() {
        let reg = SessionRegistry::new_for_test();
        let resp = reg.handle(ClientMessage::ClosePty {
            session_id: "nope".into(),
        });
        assert!(matches!(resp.first(), Some(ServerMessage::Error { .. })));
    }

    #[test]
    fn link_unknown_pty_session_is_rejected() {
        let reg = SessionRegistry::new_for_test();
        assert!(
            !reg.link_workspace_session("ghost-pty", "tab-1".into()),
            "linking an unknown pty session must fail"
        );
        assert!(reg.workspace_session_for("ghost-pty").is_none());
        assert!(reg.pty_for_workspace_session("tab-1").is_none());
    }

    #[tokio::test]
    async fn link_bridges_pty_to_workspace_tab() {
        // `create` spawns a real shell + a `spawn_blocking` reader task,
        // so this test needs a tokio runtime.
        let reg = SessionRegistry::new_for_test();
        let created = reg.handle(ClientMessage::CreatePty {
            cwd: None,
            cols: 80,
            rows: 24,
            shell: Some("/bin/sh".into()),
        });
        let pty_id = match created.first() {
            Some(ServerMessage::PtyCreated { session_id, .. }) => session_id.clone(),
            other => panic!("expected PtyCreated, got {other:?}"),
        };

        // Bridge it to a workspace tab and resolve both directions.
        assert!(reg.link_workspace_session(&pty_id, "tab-7".into()));
        assert_eq!(reg.workspace_session_for(&pty_id).as_deref(), Some("tab-7"));
        assert_eq!(
            reg.pty_for_workspace_session("tab-7").as_deref(),
            Some(pty_id.as_str())
        );

        // Closing the PTY drops the bridge with no extra bookkeeping.
        reg.handle(ClientMessage::ClosePty {
            session_id: pty_id.clone(),
        });
        assert!(reg.workspace_session_for(&pty_id).is_none());
        assert!(reg.pty_for_workspace_session("tab-7").is_none());
    }

    #[test]
    fn append_backlog_trims_oldest_bytes() {
        let backlog = Mutex::new(Vec::new());
        append_backlog(&backlog, &vec![b'a'; BACKLOG_LIMIT_BYTES + 16]);

        let retained = backlog.lock();
        assert_eq!(retained.len(), BACKLOG_LIMIT_BYTES);
        assert!(retained.iter().all(|b| *b == b'a'));
    }
}
