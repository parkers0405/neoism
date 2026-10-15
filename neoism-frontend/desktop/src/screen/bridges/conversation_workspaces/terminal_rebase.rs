//! Explicit GUI directory changes only. Never called by root synchronization.
use super::*;
use neoism_terminal_core::crosswords::Mode;
use neoism_terminal_core::handler::Handler;
use neoism_ui::TerminalShellKind;

pub(super) struct PendingTerminalDirectory {
    route: usize,
    shell_pid: u32,
    rich_text_id: usize,
    desired: PathBuf,
    remote_session: Option<String>,
    flight: Option<DirectoryFlight>,
    failed: bool,
    queued_at: Instant,
}
struct DirectoryFlight {
    root: PathBuf,
    finished_generation: u64,
    sent_at: Instant,
    request_id: Option<String>,
}

impl PendingTerminalDirectory {
    fn matches_identity(
        &self,
        route: usize,
        shell_pid: u32,
        rich_text_id: usize,
        session: Option<&str>,
    ) -> bool {
        self.route == route
            && self.shell_pid == shell_pid
            && self.rich_text_id == rich_text_id
            && self.remote_session.as_deref() == session
    }

    fn observe_local_directory(
        &mut self,
        actual: Option<&Path>,
        returned_prompt: bool,
    ) -> bool {
        if self.flight.as_ref().is_some_and(|flight| {
            flight.request_id.is_none()
                && returned_prompt
                && actual == Some(flight.root.as_path())
        }) {
            self.flight = None;
        }
        self.flight.is_none() && actual == Some(self.desired.as_path())
    }
}

fn has_backing_pty(shell_pid: u32, remote: bool, local_fd_valid: bool) -> bool {
    remote || (shell_pid > 0 && local_fd_valid)
}

fn select_backing_route(
    focused: usize,
    focused_is_terminal: bool,
    primary: Option<usize>,
    candidates: impl Iterator<Item = (usize, bool)>,
) -> Option<usize> {
    let routes: Vec<_> = candidates
        .filter_map(|(route, owns_pty)| owns_pty.then_some(route))
        .collect();
    if focused_is_terminal && routes.contains(&focused) {
        Some(focused)
    } else {
        primary
            .filter(|route| routes.contains(route))
            .or_else(|| routes.into_iter().min())
    }
}

fn safe_prompt(
    awaiting: bool,
    running_command: bool,
    alternate_screen: bool,
    passthrough: bool,
    shell_is_foreground: bool,
    unacknowledged_submission: bool,
) -> bool {
    awaiting
        && !running_command
        && !alternate_screen
        && !passthrough
        && shell_is_foreground
        && !unacknowledged_submission
}

fn directory_command(shell: TerminalShellKind, path: &Path) -> Result<String, String> {
    let path = path.to_str().ok_or("Directory is not valid UTF-8")?;
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err("Directory contains a control character".into());
    }
    Ok(match shell {
        TerminalShellKind::Bash | TerminalShellKind::Zsh => {
            format!("builtin cd -- '{}'", path.replace('\'', "'\\''"))
        }
        TerminalShellKind::Fish => {
            // Fish single quotes only escape backslash and single quote.
            format!(
                "builtin cd -- '{}'",
                path.replace('\\', "\\\\").replace('\'', "\\'")
            )
        }
        TerminalShellKind::PowerShell => {
            format!(
                "Microsoft.PowerShell.Management\\Set-Location -LiteralPath '{}'",
                path.replace('\'', "''")
            )
        }
        TerminalShellKind::Cmd => {
            // cmd expands %vars% and (when enabled) !vars! inside quotes.
            if path.contains(['"', '%', '!']) {
                return Err(
                    "cmd cannot safely change to this directory; use PowerShell".into()
                );
            }
            format!("cd /d \"{path}\"")
        }
        TerminalShellKind::Unknown => {
            return Err("Shell identity is unknown; directory change is deferred".into())
        }
    })
}

impl Screen<'_> {
    fn terminal_directory_key(&self) -> (String, String) {
        (
            self.conversation_host_scope(),
            self.context_manager.current_workspace_tree_id(),
        )
    }

    /// Capture the target before changing any root/focus metadata.
    pub(crate) fn queue_explicit_terminal_directory(
        &mut self,
        root: PathBuf,
    ) -> Result<(), String> {
        let grid = self.context_manager.current_grid();
        let focused = self.context_manager.current();
        let owns_pty =
            |ctx: &crate::context::tab::Context<neoism_backend::event::EventProxy>| {
                #[cfg(not(target_os = "windows"))]
                let fd_valid = *ctx.main_fd >= 0;
                #[cfg(target_os = "windows")]
                let fd_valid = true;
                has_backing_pty(ctx.shell_pid, ctx.remote_pty.is_some(), fd_valid)
            };
        // An Agent/editor view may cover the primary Context's live PTY.
        // Only backing identity matters for candidates, never displayed mode.
        let route = select_backing_route(
            focused.route_id,
            !focused.has_non_terminal_surface(),
            grid.workspace_route_id(),
            grid.contexts().values().map(|item| {
                let ctx = item.context();
                (ctx.route_id, owns_pty(ctx))
            }),
        )
        .ok_or("No terminal in this workspace; directory was not changed")?;
        let target = grid
            .contexts()
            .values()
            .map(|item| item.context())
            .find(|ctx| ctx.route_id == route)
            .ok_or("Terminal was closed")?;
        let remote_session = if let Some(binding) = &target.remote_pty {
            let shared = binding
                .shared
                .lock()
                .map_err(|_| "Terminal binding is unavailable")?;
            if shared.failed || shared.awaiting_attach.is_some() {
                return Err("Terminal is reconnecting; directory was not changed".into());
            }
            Some(
                shared
                    .session_id
                    .clone()
                    .ok_or("Terminal session is not ready")?,
            )
        } else {
            if self.context_manager.current_workspace_is_remote_joined() {
                return Err("Joined workspace has no host terminal binding; no local shell input was sent".into());
            }
            None
        };
        // The host owns remote shell identity, quoting and readiness.
        if remote_session.is_none() {
            #[cfg(target_os = "linux")]
            let shell = std::fs::read_link(format!("/proc/{}/exe", target.shell_pid))
                .ok()
                .map(|path| TerminalShellKind::detect(&path.to_string_lossy()))
                .filter(|kind| *kind != TerminalShellKind::Unknown)
                .unwrap_or(target.terminal_shell_kind);
            #[cfg(not(target_os = "linux"))]
            let shell = target.terminal_shell_kind;
            if shell == TerminalShellKind::Unknown {
                return Err("Cannot verify this shell's identity. Wait for shell startup or use a supported integrated shell, then select the directory again; no root or terminal change was made".into());
            }
            directory_command(shell, &root)?;
        }
        let shell_pid = target.shell_pid;
        let rich_text_id = target.rich_text_id;
        let key = self.terminal_directory_key();
        let stale = self
            .conversation_workspaces
            .terminal_directories
            .get(&key)
            .is_some_and(|pending| {
                !self
                    .context_manager
                    .current_grid()
                    .contexts()
                    .values()
                    .any(|item| {
                        let ctx = item.context();
                        let session = ctx.remote_pty.as_ref().and_then(|binding| {
                            binding
                                .shared
                                .lock()
                                .ok()
                                .and_then(|shared| shared.session_id.clone())
                        });
                        pending.matches_identity(
                            ctx.route_id,
                            ctx.shell_pid,
                            ctx.rich_text_id,
                            session.as_deref(),
                        )
                    })
            });
        if stale {
            self.conversation_workspaces
                .terminal_directories
                .remove(&key);
            self.renderer.notifications.push("Canceled a stale terminal directory request; the new selection targets the current terminal.",
                neoism_ui::panels::notifications::NotificationLevel::Warn);
        }
        match self.conversation_workspaces.terminal_directories.get_mut(&key) {
            Some(pending) if pending.route == route && pending.shell_pid == shell_pid
                && pending.rich_text_id == rich_text_id && pending.remote_session == remote_session => {
                pending.desired = root;
                // An explicitly retried failed operation may start over, but an
                // in-flight successful/superseded selection must still settle.
                if pending.failed {
                    pending.flight = None;
                }
                pending.failed = false;
                pending.queued_at = Instant::now();
            }
            Some(_) => return Err("A directory change is pending in another terminal; finish it before changing targets".into()),
            None => {
                self.conversation_workspaces.terminal_directories.insert(key, PendingTerminalDirectory {
                    route, shell_pid, rich_text_id, desired: root, remote_session, flight: None, failed: false, queued_at: Instant::now(),
                });
            }
        }
        Ok(())
    }

    pub(crate) fn terminal_directory_submission_blocked(&self) -> bool {
        self.conversation_workspaces
            .terminal_directories
            .get(&self.terminal_directory_key())
            .is_some_and(|pending| {
                pending.route == self.context_manager.current().route_id
            })
    }

    fn pump_remote_terminal_directory(
        &mut self,
        pending: &mut PendingTerminalDirectory,
    ) -> (bool, Option<String>) {
        let session = pending.remote_session.as_deref().expect("remote request");
        let binding = self
            .context_manager
            .current_grid()
            .contexts()
            .values()
            .map(|item| item.context())
            .find(|ctx| {
                ctx.route_id == pending.route && ctx.rich_text_id == pending.rich_text_id
            })
            .and_then(|ctx| ctx.remote_pty.clone());
        let Some(binding) = binding else {
            return (true, Some("Directory change canceled: captured host terminal was closed or replaced".into()));
        };
        // Check identity even when an old matching reply has arrived.
        let (same_session, ready) = binding
            .shared
            .lock()
            .map(|shared| {
                (
                    shared.session_id.as_deref() == Some(session),
                    !shared.failed && shared.awaiting_attach.is_none(),
                )
            })
            .unwrap_or((false, false));
        if !same_session {
            return (true, Some("Directory change canceled: host session identity changed; select again".into()));
        }
        if !ready {
            let notice = (!pending.failed).then(|| "Host terminal is reconnecting; directory acknowledgment is still required before commands can run".into());
            pending.failed = true;
            return (false, notice);
        }
        if let Some(flight) = &pending.flight {
            let reply = flight
                .request_id
                .as_ref()
                .and_then(|id| self.context_manager.take_terminal_directory_result(id));
            if let Some((reply_session, result)) = reply {
                if reply_session != session {
                    return (false, None);
                }
                match result {
                    Ok(directory) if Path::new(&directory) == flight.root => {
                        if let Some(ctx) = self
                            .context_manager
                            .current_grid()
                            .contexts()
                            .values()
                            .map(|item| item.context())
                            .find(|ctx| {
                                ctx.route_id == pending.route
                                    && ctx.rich_text_id == pending.rich_text_id
                            })
                        {
                            if let Some(mut terminal) = ctx.terminal.try_lock_unfair() {
                                terminal.set_current_directory(PathBuf::from(&directory));
                            }
                        }
                        let done = pending.desired == flight.root;
                        pending.flight = None;
                        if done {
                            return (true, None);
                        }
                    }
                    Ok(_) => {
                        pending.failed = true;
                        return (false, Some("Host acknowledged a different terminal directory; commands remain blocked".into()));
                    }
                    Err(error) => {
                        pending.failed = true;
                        return (false, Some(format!("Host terminal directory change failed: {error}. Commands remain blocked; select again to retry.")));
                    }
                }
            } else if flight.sent_at.elapsed() > Duration::from_secs(30)
                && !pending.failed
            {
                pending.failed = true;
                return (false, Some("Waiting for host terminal directory acknowledgment; commands remain blocked. Select again to retry.".into()));
            }
        }
        if pending.flight.is_none() && !pending.failed {
            let request_id = uuid::Uuid::new_v4().to_string();
            match binding.request_directory_change(
                session,
                pending.desired.to_string_lossy().into_owned(),
                request_id.clone(),
            ) {
                Ok(()) => {
                    pending.flight = Some(DirectoryFlight {
                        root: pending.desired.clone(),
                        finished_generation: 0,
                        sent_at: Instant::now(),
                        request_id: Some(request_id),
                    })
                }
                Err(error) => {
                    pending.failed = true;
                    return (false, Some(error));
                }
            }
        }
        (false, None)
    }

    /// Called before frame snapshots and before composer submission, not just
    /// while the picker is open. Busy/unknown/alternate-screen targets receive
    /// NO bytes, interrupts or line-editor clearing sequences.
    pub(crate) fn pump_explicit_terminal_directory(&mut self) {
        let key = self.terminal_directory_key();
        let Some(mut pending) = self
            .conversation_workspaces
            .terminal_directories
            .remove(&key)
        else {
            return;
        };
        let mut notice = None;
        let mut complete = false;
        if pending.remote_session.is_some() {
            let (done, message) = self.pump_remote_terminal_directory(&mut pending);
            if !done {
                self.conversation_workspaces
                    .terminal_directories
                    .insert(key, pending);
            }
            if let Some(message) = message {
                self.renderer.notifications.push(
                    message,
                    neoism_ui::panels::notifications::NotificationLevel::Warn,
                );
            }
            return;
        }
        let target = self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .values_mut()
            .find(|item| item.context().route_id == pending.route);
        if let Some(item) = target {
            let ctx = item.context_mut();
            if ctx.shell_pid != pending.shell_pid
                || ctx.rich_text_id != pending.rich_text_id
                || ctx.remote_pty.is_some()
            {
                // Identity invalidation is not an ack; never redirect to a new PTY.
                complete = true;
                notice = Some("Directory change canceled: its captured terminal was replaced. Select the directory again to target a live terminal.".into());
            } else if let Some(mut terminal) = ctx.terminal.try_lock_unfair() {
                let state = terminal.shell_prompt_state();
                // /proc/<shell>/cwd observes the shell itself, not an unrelated
                // foreground child or the app's cosmetic current-directory cache.
                #[cfg(target_os = "linux")]
                let actual =
                    std::fs::read_link(format!("/proc/{}/cwd", ctx.shell_pid)).ok();
                #[cfg(not(target_os = "linux"))]
                let actual = terminal.current_directory.clone();
                let actual = actual.map(Self::normalize_workspace_root);
                #[cfg(target_os = "linux")]
                if let Some(observed) = &actual {
                    // Actual kernel observation, never the chosen GUI path.
                    // Keep completion/prefix metadata in step even if OSC 7 is
                    // still queued behind this /proc acknowledgment.
                    terminal.set_current_directory(observed.clone());
                }
                #[cfg(unix)]
                let idle_shell =
                    unsafe { libc::tcgetpgrp(*ctx.main_fd) } == ctx.shell_pid as i32;
                #[cfg(target_os = "windows")]
                let idle_shell =
                    crate::terminal::blocks::detect_foreground_shell(ctx.shell_pid)
                        == Some(ctx.terminal_shell_kind);
                #[cfg(not(any(unix, target_os = "windows")))]
                let idle_shell = false;
                let returned_prompt = state.awaiting_command
                    && !state.running_command
                    && idle_shell
                    && pending.flight.as_ref().is_none_or(|flight| {
                        state.command_finished_generation > flight.finished_generation
                    });
                if pending.observe_local_directory(actual.as_deref(), returned_prompt) {
                    complete = true;
                }
                if let Some(flight) = &pending.flight {
                    if (state.command_finished_generation > flight.finished_generation
                        && actual.as_deref() != Some(flight.root.as_path()))
                        || flight.sent_at.elapsed() > Duration::from_secs(10)
                    {
                        // Leave the gate in place after failure, not an automatic
                        // retry loop and not permission to run in the old cwd.
                        if !pending.failed {
                            notice = Some("Terminal directory change was not acknowledged. Commands remain blocked; select the directory again to retry.".to_string());
                        }
                        pending.failed = true;
                    }
                }
                if pending.flight.is_none()
                    && !complete
                    && !pending.failed
                    && !state.awaiting_command
                    && !state.running_command
                    && !terminal.mode().contains(Mode::ALT_SCREEN)
                    && !ctx.terminal_input.passthrough_session_active()
                    && pending.queued_at.elapsed() > Duration::from_secs(10)
                {
                    pending.failed = true;
                    notice = Some("Shell prompt readiness could not be verified; no input was sent. Enable shell integration and select again, or select the terminal's actual directory to cancel. Commands remain blocked for this target.".into());
                }
                if pending.flight.is_none()
                    && actual.as_deref() == Some(pending.desired.as_path())
                {
                    complete = true;
                } else if pending.flight.is_none()
                    && !pending.failed
                    && state.awaiting_command
                    && !state.running_command
                    && !terminal.mode().contains(Mode::ALT_SCREEN)
                    && !ctx.terminal_input.passthrough_session_active()
                {
                    // A normal command may have just been written while the
                    // parser still holds the old prompt. Its block closes that race.
                    ctx.terminal_input.sync_shell_state(state);
                    let running =
                        ctx.terminal_input.command_block_snapshots().iter().any(
                            |block| {
                                block.status
                                    == crate::terminal::blocks::BlockStatusKind::Running
                            },
                        );
                    if safe_prompt(
                        state.awaiting_command,
                        state.running_command,
                        terminal.mode().contains(Mode::ALT_SCREEN),
                        ctx.terminal_input.passthrough_session_active(),
                        idle_shell,
                        running,
                    ) {
                        #[cfg(target_os = "linux")]
                        if let Some(shell) =
                            crate::terminal::blocks::detect_foreground_shell(*ctx.main_fd)
                        {
                            ctx.terminal_shell_kind = shell;
                        }
                        match directory_command(ctx.terminal_shell_kind, &pending.desired)
                        {
                            Ok(command) => {
                                let output_row = Some(
                                    terminal
                                        .absolute_row_for_line(terminal.cursor().pos.row),
                                );
                                let bracketed =
                                    terminal.mode().contains(Mode::BRACKETED_PASTE);
                                ctx.terminal_input.record_control_command(
                                    &command,
                                    actual.as_deref(),
                                    output_row,
                                );
                                ctx.messenger.send_write(
                                    ctx.terminal_shell_kind
                                        .command_payload(&command, bracketed),
                                );
                                pending.flight = Some(DirectoryFlight {
                                    root: pending.desired.clone(),
                                    finished_generation: state
                                        .command_finished_generation,
                                    sent_at: Instant::now(),
                                    request_id: None,
                                });
                            }
                            Err(error) => {
                                if ctx.terminal_shell_kind != TerminalShellKind::Unknown {
                                    pending.failed = true;
                                    notice = Some(error);
                                }
                            }
                        }
                    }
                }
            }
        } else {
            complete = true;
            notice = Some("Directory change canceled: its captured terminal was closed. Select the directory again to target a live terminal.".into());
        }
        if !complete {
            self.conversation_workspaces
                .terminal_directories
                .insert(key, pending);
        }
        if let Some(notice) = notice {
            self.renderer.notifications.push(
                notice,
                neoism_ui::panels::notifications::NotificationLevel::Warn,
            );
        }
    }
}

#[cfg(test)]
mod pty_tests;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn literal_paths_are_shell_specific() {
        assert_eq!(
            directory_command(TerminalShellKind::Bash, Path::new("/a b/o'k")).unwrap(),
            "builtin cd -- '/a b/o'\\''k'"
        );
        assert_eq!(
            directory_command(TerminalShellKind::Fish, Path::new("/a\\b/o'k")).unwrap(),
            "builtin cd -- '/a\\\\b/o\\'k'"
        );
        assert!(
            directory_command(TerminalShellKind::Cmd, Path::new("C:\\%TEMP%")).is_err()
        );
        assert!(
            directory_command(TerminalShellKind::Unknown, Path::new("/tmp")).is_err()
        );
        assert!(
            directory_command(TerminalShellKind::Bash, Path::new("/tmp\nwhoami"))
                .is_err()
        );
    }
}
