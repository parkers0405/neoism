// Cheap git-branch lookup for the status line: walks up looking for
// `.git/HEAD` and parses it directly (avoids forking `git`). Cached
// per-directory with a 2s TTL so per-frame `set_info` calls don't hit
// the filesystem on every paint.
//
// TODO(wave6-cutover): this module is process-spawn + filesystem
// heavy and won't compile to wasm. The shape stays here so the
// native host can keep calling these free functions verbatim; on
// web the host will route through `crate::services::GitService`
// instead. Once that bridge lands, gate the `std::process::Command`
// + `std::thread::spawn` bodies behind `#[cfg(not(target_arch =
// "wasm32"))]` (or move the cache behind a `GitService` impl).

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use web_time::Duration;
use web_time::Instant;

const BRANCH_TTL: Duration = Duration::from_secs(2);
const ROOT_TTL: Duration = Duration::from_secs(10);
const CHANGE_TTL: Duration = Duration::from_secs(10);

struct Entry {
    value: Option<String>,
    fetched_at: Instant,
}

struct RootEntry {
    value: Option<PathBuf>,
    fetched_at: Instant,
}

struct ChangeEntry {
    value: Option<GitChangeSummary>,
    fetched_at: Instant,
    in_flight: bool,
}

/// Line-level change totals for the active repo. `added` counts line
/// additions across all tracked files (via `git diff HEAD --numstat`)
/// plus the line counts of every untracked file; `deleted` is the
/// numstat deletion total. Mirrors the totals shown in the side diff
/// panel so the status pill stays consistent with it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GitChangeSummary {
    pub added: u64,
    pub deleted: u64,
}

impl GitChangeSummary {
    pub fn is_empty(self) -> bool {
        self.added == 0 && self.deleted == 0
    }
}

thread_local! {
    static CACHE: RefCell<HashMap<PathBuf, Entry>> = RefCell::new(HashMap::new());
    static ROOT_CACHE: RefCell<HashMap<PathBuf, RootEntry>> = RefCell::new(HashMap::new());
}

static CHANGE_CACHE: OnceLock<Mutex<HashMap<PathBuf, ChangeEntry>>> = OnceLock::new();

fn change_cache() -> &'static Mutex<HashMap<PathBuf, ChangeEntry>> {
    CHANGE_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn branch_for(start: &Path) -> Option<String> {
    let dir: PathBuf = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    let now = Instant::now();
    if let Some(hit) = CACHE.with(|c| {
        let map = c.borrow();
        map.get(&dir)
            .filter(|e| now.saturating_duration_since(e.fetched_at) < BRANCH_TTL)
            .map(|e| e.value.clone())
    }) {
        return hit;
    }

    let value = read_head(&dir);
    CACHE.with(|c| {
        c.borrow_mut().insert(
            dir,
            Entry {
                value: value.clone(),
                fetched_at: now,
            },
        );
    });
    value
}

pub fn change_summary_for(start: &Path) -> Option<GitChangeSummary> {
    let repo_root = repo_root_for(start)?;
    let now = Instant::now();
    let (cached, should_refresh) = {
        let Ok(mut cache) = change_cache().lock() else {
            return None;
        };
        match cache.get_mut(&repo_root) {
            Some(entry) => {
                let stale = now.saturating_duration_since(entry.fetched_at) >= CHANGE_TTL;
                let should_refresh = stale && !entry.in_flight;
                if should_refresh {
                    entry.in_flight = true;
                }
                (entry.value, should_refresh)
            }
            None => {
                cache.insert(
                    repo_root.clone(),
                    ChangeEntry {
                        value: None,
                        fetched_at: now,
                        in_flight: true,
                    },
                );
                (None, true)
            }
        }
    };

    if should_refresh {
        refresh_change_summary(repo_root);
    }

    cached
}

fn read_head(dir: &Path) -> Option<String> {
    let mut cur = Some(dir);
    while let Some(d) = cur {
        let head = d.join(".git").join("HEAD");
        if head.is_file() {
            let raw = std::fs::read_to_string(&head).ok()?;
            let s = raw.trim();
            if let Some(rest) = s.strip_prefix("ref: refs/heads/") {
                return Some(rest.to_string());
            }
            return Some(s.chars().take(7).collect());
        }
        cur = d.parent();
    }
    None
}

pub fn repo_root_for(start: &Path) -> Option<PathBuf> {
    let dir = if start.is_file() {
        start.parent()?.to_path_buf()
    } else {
        start.to_path_buf()
    };

    let now = Instant::now();
    if let Some(hit) = ROOT_CACHE.with(|c| {
        let map = c.borrow();
        map.get(&dir)
            .filter(|e| now.saturating_duration_since(e.fetched_at) < ROOT_TTL)
            .map(|e| e.value.clone())
    }) {
        return hit;
    }

    let value = find_repo_root(&dir);
    ROOT_CACHE.with(|c| {
        c.borrow_mut().insert(
            dir,
            RootEntry {
                value: value.clone(),
                fetched_at: now,
            },
        );
    });
    value
}

fn find_repo_root(start: &Path) -> Option<PathBuf> {
    let mut dir = start;
    loop {
        let git_marker = dir.join(".git");
        if git_marker.is_dir() || git_marker.is_file() {
            return Some(dir.to_path_buf());
        }
        dir = dir.parent()?;
    }
}

fn refresh_change_summary(repo_root: PathBuf) {
    #[cfg(not(target_arch = "wasm32"))]
    std::thread::spawn(move || {
        let value = read_change_summary(&repo_root);
        if let Ok(mut cache) = change_cache().lock() {
            cache.insert(
                repo_root,
                ChangeEntry {
                    value,
                    fetched_at: Instant::now(),
                    in_flight: false,
                },
            );
        }
    });
    // wasm has no subprocess/thread — the git change summary is sourced from
    // the daemon over the wire on the web frontend, so this producer is a
    // no-op here (the reader `change_summary_for` stays cross-platform).
    #[cfg(target_arch = "wasm32")]
    let _ = repo_root;
}

#[cfg(not(target_arch = "wasm32"))]
fn read_change_summary(repo_root: &Path) -> Option<GitChangeSummary> {
    // Tracked file line changes via numstat — same source the diff
    // panel uses, so the status pill agrees with what the panel shows.
    let (mut added, deleted) = crate::utils::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
        .args(["diff", "HEAD", "--numstat", "-z", "--no-color"])
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| sum_numstat(&o.stdout))
        .unwrap_or((0, 0));

    // Untracked files don't show up in `diff HEAD`. Count their
    // contents as additions so the totals match the side panel.
    if let Ok(output) = crate::utils::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo_root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
    {
        if output.status.success() {
            for path in untracked_paths(&output.stdout) {
                added = added.saturating_add(count_lines(&repo_root.join(path)) as u64);
            }
        }
    }

    let summary = GitChangeSummary { added, deleted };
    if summary.is_empty() {
        None
    } else {
        Some(summary)
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn sum_numstat(bytes: &[u8]) -> (u64, u64) {
    let mut added = 0u64;
    let mut deleted = 0u64;
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let record = &bytes[start..i];
        i = i.saturating_add(1);
        if record.is_empty() {
            continue;
        }
        let Ok(s) = std::str::from_utf8(record) else {
            continue;
        };
        let mut parts = s.splitn(3, '\t');
        let add = parts.next().unwrap_or("0");
        let del = parts.next().unwrap_or("0");
        let path = parts.next().unwrap_or("");
        // Renamed entries with `-z` emit "<add>\t<del>\t" then two
        // separate NUL records (old path, new path). Skip both so the
        // outer loop doesn't try to parse them as numstat lines.
        if path.is_empty() {
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }
        added = added.saturating_add(add.parse::<u64>().unwrap_or(0));
        deleted = deleted.saturating_add(del.parse::<u64>().unwrap_or(0));
    }
    (added, deleted)
}

#[cfg(not(target_arch = "wasm32"))]
fn untracked_paths(bytes: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut i = 0usize;
    while i < bytes.len() {
        let start = i;
        while i < bytes.len() && bytes[i] != 0 {
            i += 1;
        }
        let record = &bytes[start..i];
        i = i.saturating_add(1);
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        if x == '?' && y == '?' {
            if let Ok(s) = std::str::from_utf8(&record[3..]) {
                paths.push(s.to_string());
            }
        } else if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            while i < bytes.len() && bytes[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }
    }
    paths
}

#[cfg(not(target_arch = "wasm32"))]
fn count_lines(path: &Path) -> u32 {
    let bytes = match std::fs::read(path) {
        Ok(b) => b,
        Err(_) => return 0,
    };
    if bytes.is_empty() {
        return 0;
    }
    let mut count = bytes.iter().filter(|b| **b == b'\n').count();
    if !bytes.ends_with(b"\n") {
        count += 1;
    }
    count.min(u32::MAX as usize) as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sum_numstat_adds_and_deletes() {
        let bytes = b"3\t1\tsrc/main.rs\x002\t0\tsrc/lib.rs\x00";
        assert_eq!(sum_numstat(bytes), (5, 1));
    }

    #[test]
    fn sum_numstat_handles_renames() {
        // `-z` emits "<add>\t<del>\t" then old/new path as two NULs.
        let bytes = b"4\t2\t\x00old.rs\x00new.rs\x001\t1\tother.rs\x00";
        assert_eq!(sum_numstat(bytes), (5, 3));
    }
}
