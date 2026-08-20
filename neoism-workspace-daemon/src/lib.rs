//! Library surface for the neoism workspace daemon.
//!
//! The binary in `main.rs` is a thin wrapper around this crate; the
//! integration tests live alongside it and depend on these modules.

pub mod agent;
pub mod audit;
pub mod auth;
pub mod cloud_auth;
/// Config get/set + read-only extensions inventory for the websocket
/// `Config` envelope (web settings + extensions pages).
pub mod config_surface;
pub mod crdt;
/// Standalone-daemon `NEOISM_DAEMON_TOKEN` bootstrap.
pub mod daemon_token;
pub mod files;
pub mod fs_watch;
pub mod git;
pub mod handshake;
pub mod hosts;
pub mod language_server;
pub mod pairing;
mod path;
pub mod permissions;
pub mod persistence;
mod process;
pub mod search;
pub mod server;
pub mod sessions;
pub mod tailnet;
/// Built web UI served from the daemon HTTP listener.
pub mod web;
/// Windows-only ACL hardening for secret files (unix uses mode bits).
#[cfg(windows)]
pub mod windows_acl;
#[cfg(windows)]
pub(crate) mod windows_process;

#[cfg(windows)]
pub fn hide_std_command(command: &mut std::process::Command) {
    windows_process::hide_std_command(command);
}

#[cfg(windows)]
pub fn hide_tokio_command(command: &mut tokio::process::Command) {
    windows_process::hide_tokio_command(command);
}

#[cfg(windows)]
pub fn detach_std_command(command: &mut std::process::Command) {
    windows_process::detach_std_command(command);
}

pub fn hidden_std_command(program: impl AsRef<std::ffi::OsStr>) -> std::process::Command {
    let mut command = std::process::Command::new(program);
    #[cfg(windows)]
    hide_std_command(&mut command);
    command
}

pub fn hidden_tokio_command(
    program: impl AsRef<std::ffi::OsStr>,
) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(program);
    #[cfg(windows)]
    hide_tokio_command(&mut command);
    command
}

pub mod workspace;
pub mod workspace_promote;
pub mod workspace_provision;
pub mod workspace_snapshot;
