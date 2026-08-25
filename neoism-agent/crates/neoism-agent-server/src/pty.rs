#[cfg(any(not(windows), test))]
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

use neoism_agent_core::{Id, IdKind, PtyInfo, ShellItem};
use serde::{Deserialize, Serialize};

pub(crate) const CONNECT_TOKEN_TTL_MS: u64 = 30_000;

#[path = "pty_buffer.rs"]
mod pty_buffer;

#[cfg(test)]
use pty_buffer::PtyOutputBuffer;

#[path = "pty_process.rs"]
mod pty_process;

pub(crate) use pty_process::{serve_websocket, PtyProcessRegistry};

/// Lazily allocated PTY state owned by a single workspace generation.
#[derive(Default)]
pub(crate) struct PtyWorkspaceRuntime {
    pub(crate) infos: RwLock<HashMap<String, PtyInfo>>,
    pub(crate) tokens: RwLock<ConnectTokens>,
    pub(crate) processes: Arc<PtyProcessRegistry>,
}

impl PtyWorkspaceRuntime {
    pub(crate) async fn shutdown(&self) {
        self.processes.shutdown().await;
        self.infos.write().await.clear();
        self.tokens.write().await.tokens.clear();
    }
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyCreateRequest {
    pub(crate) command: Option<Vec<String>>,
    pub(crate) cwd: Option<String>,
    pub(crate) title: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtyUpdateRequest {
    pub(crate) title: Option<String>,
    pub(crate) cwd: Option<String>,
    pub(crate) size: Option<PtySize>,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PtySize {
    pub(crate) cols: u16,
    pub(crate) rows: u16,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize)]
pub(crate) struct PtyConnectToken {
    pub(crate) ticket: String,
    pub(crate) expires_in: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct PtyConnectTicket {
    pty_id: String,
    ticket: String,
    expires_at: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PtyError {
    NotFound,
    ExpiredTicket,
    InvalidTicket,
    TicketPtyMismatch,
    SpawnFailed(String),
    Io(String),
}

#[derive(Clone, Debug, Default)]
pub(crate) struct ConnectTokens {
    tokens: HashMap<String, PtyConnectTicket>,
}

impl ConnectTokens {
    pub(crate) fn issue(
        &mut self,
        pty_id: impl Into<String>,
        now_ms: u64,
    ) -> PtyConnectToken {
        self.issue_with_ticket(
            pty_id,
            Id::ascending(IdKind::Entry).to_string(),
            now_ms,
            CONNECT_TOKEN_TTL_MS / 1000,
        )
    }

    pub(crate) fn issue_with_ticket(
        &mut self,
        pty_id: impl Into<String>,
        ticket: impl Into<String>,
        now_ms: u64,
        expires_in: u64,
    ) -> PtyConnectToken {
        let pty_id = pty_id.into();
        let ticket = ticket.into();
        let token = PtyConnectToken {
            ticket: ticket.clone(),
            expires_in,
        };
        self.tokens.insert(
            ticket.clone(),
            PtyConnectTicket {
                pty_id,
                ticket,
                expires_at: now_ms + expires_in.saturating_mul(1000),
            },
        );
        token
    }

    pub(crate) fn validate(
        &mut self,
        pty_id: &str,
        ticket: &str,
        now_ms: u64,
    ) -> Result<PtyConnectToken, PtyError> {
        let record = self.tokens.remove(ticket).ok_or(PtyError::InvalidTicket)?;
        if record.pty_id != pty_id {
            return Err(PtyError::TicketPtyMismatch);
        }
        if record.expires_at <= now_ms {
            return Err(PtyError::ExpiredTicket);
        }
        Ok(PtyConnectToken {
            ticket: record.ticket,
            expires_in: record
                .expires_at
                .saturating_sub(now_ms)
                .saturating_add(999)
                .max(1)
                / 1000,
        })
    }

    pub(crate) fn prune_expired(&mut self, now_ms: u64) {
        self.tokens.retain(|_, token| token.expires_at > now_ms);
    }
}

pub(crate) fn discover_shells() -> Vec<ShellItem> {
    #[cfg(windows)]
    {
        let mut shells = ["pwsh.exe", "powershell.exe", "cmd.exe"]
            .into_iter()
            .filter_map(crate::platform_shell::resolve_command)
            .map(|path| {
                let path = path.to_string_lossy().into_owned();
                ShellItem {
                    name: shell_name(&path),
                    path,
                    acceptable: true,
                }
            })
            .collect::<Vec<_>>();
        shells.sort_by(|left, right| left.name.cmp(&right.name));
        return shells;
    }

    #[cfg(not(windows))]
    let shell = std::env::var("SHELL").ok();
    #[cfg(not(windows))]
    discover_shells_with(shell.as_deref(), default_shell_candidates(), |path| {
        Path::new(path).exists()
    })
}

#[cfg(any(not(windows), test))]
pub(crate) fn discover_shells_with<I>(
    env_shell: Option<&str>,
    candidates: I,
    acceptable: impl Fn(&str) -> bool,
) -> Vec<ShellItem>
where
    I: IntoIterator,
    I::Item: AsRef<str>,
{
    let mut shells = BTreeMap::<String, ShellItem>::new();

    if let Some(path) = env_shell {
        insert_shell(&mut shells, path, &acceptable);
    }
    for path in candidates {
        insert_shell(&mut shells, path.as_ref(), &acceptable);
    }

    if shells.is_empty() {
        #[cfg(windows)]
        let fallback = crate::platform_shell::program();
        #[cfg(not(windows))]
        let fallback = "/bin/sh".to_string();
        shells.insert(
            fallback.clone(),
            ShellItem {
                name: shell_name(&fallback),
                path: fallback,
                acceptable: true,
            },
        );
    }

    shells.into_values().collect()
}

#[cfg(any(not(windows), test))]
fn insert_shell(
    shells: &mut BTreeMap<String, ShellItem>,
    path: &str,
    acceptable: &impl Fn(&str) -> bool,
) {
    let path = path.trim();
    if path.is_empty() {
        return;
    }
    shells.entry(path.to_string()).or_insert_with(|| ShellItem {
        name: shell_name(path),
        path: path.to_string(),
        acceptable: acceptable(path),
    });
}

pub(crate) fn create_pty_info(
    request: PtyCreateRequest,
    default_cwd: impl Into<String>,
    default_shell: impl Into<String>,
    now_ms: u64,
) -> PtyInfo {
    let default_shell = default_shell.into();
    let command = request
        .command
        .map(|command| sanitize_command(command))
        .filter(|command| !command.is_empty())
        .unwrap_or_else(|| login_shell_command(&default_shell));

    PtyInfo {
        id: Id::ascending(IdKind::Pty).to_string(),
        command,
        cwd: clean_text(request.cwd).unwrap_or_else(|| default_cwd.into()),
        title: clean_text(request.title).unwrap_or_else(|| "shell".to_string()),
        time: now_ms,
    }
}

pub(crate) fn insert_pty(ptys: &mut HashMap<String, PtyInfo>, info: PtyInfo) -> PtyInfo {
    ptys.insert(info.id.clone(), info.clone());
    info
}

pub(crate) fn list_ptys(ptys: &HashMap<String, PtyInfo>) -> Vec<PtyInfo> {
    let mut values = ptys.values().cloned().collect::<Vec<_>>();
    values.sort_by(|left, right| {
        left.time
            .cmp(&right.time)
            .then_with(|| left.id.cmp(&right.id))
    });
    values
}

pub(crate) fn get_pty(
    ptys: &HashMap<String, PtyInfo>,
    pty_id: &str,
) -> Result<PtyInfo, PtyError> {
    ptys.get(pty_id).cloned().ok_or(PtyError::NotFound)
}

pub(crate) fn update_pty(
    ptys: &mut HashMap<String, PtyInfo>,
    pty_id: &str,
    request: PtyUpdateRequest,
) -> Result<PtyInfo, PtyError> {
    let info = ptys.get_mut(pty_id).ok_or(PtyError::NotFound)?;
    apply_pty_update(info, request);
    Ok(info.clone())
}

pub(crate) fn remove_pty(
    ptys: &mut HashMap<String, PtyInfo>,
    pty_id: &str,
) -> Result<PtyInfo, PtyError> {
    ptys.remove(pty_id).ok_or(PtyError::NotFound)
}

pub(crate) fn apply_pty_update(info: &mut PtyInfo, request: PtyUpdateRequest) {
    if let Some(title) = clean_text(request.title) {
        info.title = title;
    }
    if let Some(cwd) = clean_text(request.cwd) {
        info.cwd = cwd;
    }
    let _ = request.size;
}
#[cfg(not(windows))]
fn default_shell_candidates() -> impl IntoIterator<Item = &'static str> {
    ["/bin/zsh", "/bin/bash", "/bin/sh", "/usr/bin/fish"]
}

pub(crate) fn fallback_shell() -> String {
    crate::platform_shell::program()
}

fn login_shell_command(shell: &str) -> Vec<String> {
    let name = shell_name(shell);
    match name.as_str() {
        "bash" | "zsh" | "fish" => vec![shell.to_string(), "-l".to_string()],
        _ => vec![shell.to_string()],
    }
}

fn sanitize_command(command: Vec<String>) -> Vec<String> {
    command
        .into_iter()
        .filter_map(|value| clean_text(Some(value)))
        .collect()
}

fn clean_text(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn shell_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| path.to_string())
}

#[cfg(test)]
#[path = "pty_tests.rs"]
mod tests;
