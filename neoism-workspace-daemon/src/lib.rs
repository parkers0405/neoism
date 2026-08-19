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
pub mod permissions;
pub mod persistence;
mod process;
pub mod search;
pub mod server;
pub mod sessions;
pub mod tailnet;
/// Windows-only ACL hardening for secret files (unix uses mode bits).
#[cfg(windows)]
pub mod windows_acl;
pub mod workspace;
pub mod workspace_promote;
pub mod workspace_provision;
pub mod workspace_snapshot;
