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
    std::env::var("XDG_STATE_HOME")
        .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.local/state")))
        .map(|base| format!("{base}/neoism"))
        .unwrap_or_else(|_| ".neoism/state".to_string())
}

pub(crate) fn default_cache_dir() -> String {
    std::env::var("XDG_CACHE_HOME")
        .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.cache")))
        .map(|base| format!("{base}/neoism"))
        .unwrap_or_else(|_| ".neoism/cache".to_string())
}

pub(crate) fn default_config_dir() -> String {
    // Shares `~/.config/neoism` with the app config — the agent reads
    // its keys from the same `config.json` the terminal reads (each
    // side ignores the other's keys; `NeoismConfig` has a flatten
    // catch-all and the app's serde skips unknown fields). Skills live
    // at `~/.config/neoism/skills`, markdown agent/mode/command
    // definitions under `~/.config/neoism/{agent,mode,command}/*.md`.
    // `NEOISM_AGENT_CONFIG_DIR` overrides everything — deployments and
    // tests use it to pin (or isolate) the global config root.
    if let Ok(dir) = std::env::var("NEOISM_AGENT_CONFIG_DIR") {
        if !dir.trim().is_empty() {
            return dir;
        }
    }
    std::env::var("XDG_CONFIG_HOME")
        .or_else(|_| std::env::var("HOME").map(|home| format!("{home}/.config")))
        .map(|base| format!("{base}/neoism"))
        .unwrap_or_else(|_| ".neoism/config".to_string())
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
