//! First-party Agent plugins.
//!
//! These built-ins use the same public registration/runtime contracts as
//! third-party plugins. The crate deliberately does not depend on the server;
//! server-owned persistence or tool-kernel behavior is supplied through the
//! narrow host traits exposed by individual plugins.

use std::path::PathBuf;
use std::sync::Arc;

use tokio::sync::{oneshot, Mutex};

pub mod auth_store;
pub mod plugin;
pub mod provider;
mod provider_auth;
mod provider_auth_browser;
mod provider_catalog;
pub mod provider_error;
mod provider_responses;
mod provider_service;
mod provider_transform;
#[cfg(windows)]
pub mod windows_acl;

pub use provider_service::ProviderPlatform;

pub enum ProviderOAuthPending {
    OpenAiBrowser { issuer: String, redirect_uri: String, code_verifier: String, state: String, receiver: Arc<Mutex<Option<oneshot::Receiver<Result<String, String>>>>> },
    OpenAiHeadless { issuer: String, device_auth_id: String, user_code: String, interval_ms: u64 },
    GithubCopilot { access_token_url: String, device_code: String, interval_ms: u64, enterprise_url: Option<String> },
    XaiLoopback { redirect_uri: String, code_verifier: String },
    XaiDevice { device_code: String, interval_ms: u64 },
}

fn now_millis() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default().as_millis().try_into().unwrap_or(u64::MAX)
}

fn default_state_dir() -> PathBuf {
    default_data_dir("NEOISM_AGENT_STATE_DIR", "XDG_STATE_HOME", ".local/state", ".neoism/state")
}

fn default_cache_dir() -> PathBuf {
    default_data_dir("NEOISM_AGENT_CACHE_DIR", "XDG_CACHE_HOME", ".cache", ".neoism/cache")
}

fn default_data_dir(override_key: &str, xdg_key: &str, home_suffix: &str, fallback: &str) -> PathBuf {
    if let Ok(dir) = std::env::var(override_key) {
        if !dir.trim().is_empty() { return PathBuf::from(dir); }
    }
    #[cfg(windows)]
    if let Some(home) = dirs::home_dir() {
        let leaf = if override_key.contains("CACHE") { "cache" } else { "state" };
        return home.join("AppData").join("Local").join("neoism").join(leaf);
    }
    std::env::var(xdg_key).map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|home| PathBuf::from(home).join(home_suffix)))
        .map(|base| base.join("neoism")).unwrap_or_else(|_| PathBuf::from(fallback))
}