use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use axum::http::HeaderMap;
use neoism_agent_core::{Id, IdKind};

pub(crate) fn resolve_directory(
    query_directory: Option<String>,
    headers: &HeaderMap,
) -> String {
    query_directory
        .or_else(|| {
            headers
                .get("x-neoism-directory")
                .and_then(|value| value.to_str().ok())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .display()
                .to_string()
        })
}

pub(crate) fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

pub(crate) fn default_state_dir() -> String {
    if let Ok(dir) = std::env::var("NEOISM_AGENT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return dir;
        }
    }
    #[cfg(windows)]
    {
        return windows_neoism_dir("state", ".neoism/state");
    }
    #[cfg(not(windows))]
    {
        std::env::var("XDG_STATE_HOME")
            .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.local/state")))
            .map(|base| format!("{base}/neoism"))
            .unwrap_or_else(|_| ".neoism/state".to_string())
    }
}

pub(crate) fn default_cache_dir() -> String {
    if let Ok(dir) = std::env::var("NEOISM_AGENT_CACHE_DIR") {
        if !dir.trim().is_empty() {
            return dir;
        }
    }
    #[cfg(windows)]
    {
        return windows_neoism_dir("cache", ".neoism/cache");
    }
    #[cfg(not(windows))]
    {
        std::env::var("XDG_CACHE_HOME")
            .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.cache")))
            .map(|base| format!("{base}/neoism"))
            .unwrap_or_else(|_| ".neoism/cache".to_string())
    }
}

/// Windows state/cache trees use the application-local data root in place of
/// Unix's separate XDG state and cache roots.
#[cfg(windows)]
fn windows_neoism_dir(subdir: &str, fallback: &str) -> String {
    let Some(home) = dirs::home_dir() else {
        return fallback.to_string();
    };
    let mut dir = home.join("AppData").join("Local").join("neoism");
    if !subdir.is_empty() {
        dir = dir.join(subdir);
    }
    dir.display().to_string()
}

pub(crate) fn slug() -> String {
    const ADJECTIVES: &[&str] = &[
        "brave", "calm", "clever", "cosmic", "crisp", "curious", "eager", "gentle",
        "glowing", "happy", "hidden", "jolly", "kind", "lucky", "mighty", "misty",
        "neon", "nimble", "playful", "proud", "quick", "quiet", "shiny", "silent",
        "stellar", "sunny", "swift", "tidy", "witty",
    ];
    const NOUNS: &[&str] = &[
        "cabin", "cactus", "canyon", "circuit", "comet", "eagle", "engine", "falcon",
        "forest", "garden", "harbor", "island", "knight", "lagoon", "meadow", "moon",
        "mountain", "nebula", "orchid", "otter", "panda", "pixel", "planet", "river",
        "rocket", "sailor", "squid", "star", "tiger", "wizard", "wolf",
    ];
    let id = Id::ascending(IdKind::Event).to_string();
    let hash = id.bytes().fold(0usize, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(usize::from(byte))
    });
    format!(
        "{}-{}",
        ADJECTIVES[hash % ADJECTIVES.len()],
        NOUNS[(hash / ADJECTIVES.len()) % NOUNS.len()]
    )
}
