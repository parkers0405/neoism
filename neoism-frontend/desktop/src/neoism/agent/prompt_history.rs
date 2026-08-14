//! Global, persistent agent prompt history (zsh / codex / opencode style).
//!
//! Every prompt sent from the agent composer is appended here, and every
//! freshly-opened agent pane seeds its Up-arrow recall from the same file.
//! History is therefore GLOBAL across sessions and survives app restarts,
//! exactly like a shell's history file — unlike the old per-session vec,
//! which died with the pane.
//!
//! This lives in the DESKTOP crate on purpose. The shared `neoism-ui`
//! [`AgentInputBuffer`](neoism_ui::panels::agent_pane::input_controller::AgentInputBuffer)
//! keeps its plain `Vec<String>` (it also compiles to wasm, which has no
//! `std::fs`); the desktop host owns loading that vec on pane creation and
//! persisting it on send. On wasm no host wiring calls in here, so history
//! stays session-local there without breaking the build.

use std::fs;
use std::path::{Path, PathBuf};
#[cfg(not(test))]
use std::sync::{mpsc, OnceLock};
#[cfg(not(test))]
use std::thread;

/// zsh's default `SAVEHIST`. Bounds both the file and a new pane's recall
/// depth to the most recent 1000 prompts so a long-lived history can't grow
/// without limit.
pub const MAX_PROMPT_HISTORY: usize = 1000;

const HISTORY_FILE: &str = "agent_prompt_history";
#[cfg(not(test))]
static HISTORY_WRITER: OnceLock<Option<mpsc::Sender<String>>> = OnceLock::new();

/// Resolve the history file path. `NEOISM_AGENT_PROMPT_HISTORY_FILE`
/// overrides it outright (handy for isolation / relocation); otherwise it
/// sits beside the terminal history under the neoism data dir
/// (`~/.local/share/neoism/agent_prompt_history` on Linux,
/// `%APPDATA%\neoism\…` on Windows), mirroring
/// `terminal::blocks::shell::default_terminal_history_path`.
fn path() -> Option<PathBuf> {
    if let Some(explicit) = std::env::var_os("NEOISM_AGENT_PROMPT_HISTORY_FILE") {
        return Some(PathBuf::from(explicit));
    }
    dirs::data_local_dir()
        .or_else(dirs::config_dir)
        .map(|base| base.join("neoism").join(HISTORY_FILE))
}

/// Load the global prompt history, oldest-first / most-recent LAST — the
/// order [`AgentInputBuffer`] walks backwards from with Up. A missing or
/// unreadable file yields an empty history (first run).
///
/// [`AgentInputBuffer`]: neoism_ui::panels::agent_pane::input_controller::AgentInputBuffer
pub fn load() -> Vec<String> {
    path().map(|path| load_from(&path)).unwrap_or_default()
}

/// Append a just-sent prompt to the global history: empties are skipped, a
/// prompt identical to the previous entry is deduped (zsh
/// `HIST_IGNORE_DUPS`), and the file is capped to the most recent
/// [`MAX_PROMPT_HISTORY`]. Best-effort — a write failure is swallowed so a
/// read-only home directory never blocks sending a prompt.
pub fn append(text: &str) {
    if let Some(path) = path() {
        append_to(&path, text);
    }
}

/// Queue a history write without performing filesystem I/O on the caller.
/// A single process-wide writer preserves prompt ordering and avoids the
/// lost-update race that one background thread per prompt would introduce.
#[cfg(not(test))]
pub fn append_async(text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let writer = HISTORY_WRITER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<String>();
        match thread::Builder::new()
            .name("neoism-agent-history".into())
            .spawn(move || {
                while let Ok(text) = rx.recv() {
                    append(&text);
                }
            }) {
            Ok(_) => Some(tx),
            Err(error) => {
                tracing::warn!(
                    target: "neoism::agent_history",
                    %error,
                    "failed to start prompt-history writer"
                );
                None
            }
        }
    });
    if let Some(writer) = writer {
        let _ = writer.send(text.to_string());
    }
}

fn load_from(path: &Path) -> Vec<String> {
    let Ok(contents) = fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut out: Vec<String> = Vec::new();
    for entry in contents.lines().filter_map(decode_line) {
        if out.last().is_some_and(|last| *last == entry) {
            continue; // collapse consecutive duplicates on read
        }
        out.push(entry);
    }
    trim_to_cap(&mut out);
    out
}

fn append_to(path: &Path, text: &str) {
    if text.trim().is_empty() {
        return;
    }
    let mut entries = load_from(path);
    if entries.last().is_some_and(|last| last == text) {
        return; // consecutive duplicate — nothing to record
    }
    entries.push(text.to_string());
    trim_to_cap(&mut entries);

    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let mut body = String::with_capacity(entries.len() * 24);
    for entry in &entries {
        body.push_str(&encode_line(entry));
        body.push('\n');
    }
    let _ = fs::write(path, body);
}

/// One prompt per line, JSON-encoded so multi-line prompts round-trip
/// without colliding with the line delimiter. A legacy/plain line that is
/// not valid JSON is taken verbatim.
fn decode_line(line: &str) -> Option<String> {
    let line = line.trim_end_matches(['\r', '\n']);
    if line.is_empty() {
        return None;
    }
    let value = serde_json::from_str::<String>(line).unwrap_or_else(|_| line.to_string());
    (!value.trim().is_empty()).then_some(value)
}

fn encode_line(text: &str) -> String {
    serde_json::to_string(text).unwrap_or_default()
}

fn trim_to_cap(entries: &mut Vec<String>) {
    if entries.len() > MAX_PROMPT_HISTORY {
        let extra = entries.len() - MAX_PROMPT_HISTORY;
        entries.drain(0..extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_history_path(tag: &str) -> PathBuf {
        let unique = format!(
            "neoism-prompt-history-{}-{}-{}",
            tag,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn append_then_load_round_trips_most_recent_last() {
        let path = temp_history_path("round-trip");
        append_to(&path, "first");
        append_to(&path, "second");
        append_to(&path, "third");

        assert_eq!(load_from(&path), vec!["first", "second", "third"]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn append_skips_empties_and_consecutive_duplicates() {
        let path = temp_history_path("dedupe");
        append_to(&path, "keep");
        append_to(&path, "   "); // empty after trim — skipped
        append_to(&path, "keep"); // consecutive dup — skipped
        append_to(&path, "next");
        append_to(&path, "keep"); // non-consecutive dup — kept

        assert_eq!(load_from(&path), vec!["keep", "next", "keep"]);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn append_preserves_multiline_prompts() {
        let path = temp_history_path("multiline");
        let prompt = "line one\nline two\n  indented three";
        append_to(&path, prompt);
        append_to(&path, "after");

        assert_eq!(
            load_from(&path),
            vec![prompt.to_string(), "after".to_string()]
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn history_is_capped_to_the_most_recent_entries() {
        let path = temp_history_path("cap");
        for ix in 0..(MAX_PROMPT_HISTORY + 25) {
            append_to(&path, &format!("prompt {ix}"));
        }
        let loaded = load_from(&path);

        assert_eq!(loaded.len(), MAX_PROMPT_HISTORY);
        assert_eq!(loaded.first().unwrap(), "prompt 25");
        assert_eq!(
            loaded.last().unwrap(),
            &format!("prompt {}", MAX_PROMPT_HISTORY + 24)
        );
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn public_api_resolves_env_override() {
        // Exercises the public `load`/`append`/`path` surface (the desktop
        // host's real entry points) through the env override so it stays
        // wired even though the production callers are `cfg(not(test))`.
        let path = temp_history_path("env-override");
        std::env::set_var("NEOISM_AGENT_PROMPT_HISTORY_FILE", &path);

        append("via-public-api");
        assert_eq!(load(), vec!["via-public-api"]);

        std::env::remove_var("NEOISM_AGENT_PROMPT_HISTORY_FILE");
        let _ = fs::remove_file(&path);
    }
}
