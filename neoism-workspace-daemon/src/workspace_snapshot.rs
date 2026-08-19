//! Portable workspace-snapshot primitive for "work from anywhere".
//!
//! When the daemon "home" moves between hosts (laptop → cloud VM → SSH box)
//! the target host first clones the repo via
//! [`crate::workspace_provision::provision_from_git`] and then re-creates the
//! source's *working state* on top. Committed history travels with git; the
//! uncommitted working tree does not. This module is the missing half: it
//! captures the uncommitted changes on the source and replays them on the
//! target.
//!
//! This mirrors the Warp Oz model (git + uncommitted-diff snapshot, tracked +
//! untracked) — see locked architecture decision #3 in
//! `WORK_FROM_ANYWHERE.md`.
//!
//! ## What "uncommitted" means here
//! * **tracked** modifications — captured as a single unified diff vs `HEAD`
//!   (`git diff HEAD`), which folds staged + unstaged changes plus deletions
//!   into one patch.
//! * **untracked, non-ignored** files — captured as path + raw bytes. We
//!   respect `.gitignore` by enumerating them with
//!   `git ls-files --others --exclude-standard`, so build artifacts and
//!   secrets never ride along.
//!
//! ## git2 vs shell
//! We shell out to `git` (matching [`crate::workspace_provision`], which does
//! the same). Two operations have no clean libgit2 equivalent:
//!   * `git ls-files --others --exclude-standard` applies the *full* exclude
//!     stack (`.gitignore`, `.git/info/exclude`, the global excludesFile) the
//!     way the user expects; reproducing that with `git2::StatusOptions` is
//!     fiddly and easy to get subtly wrong.
//!   * `git apply --reject` gives us Warp-style partial application (apply the
//!     hunks that fit, reject the rest, keep going) for free, with a
//!     machine-parseable progress report on stderr. libgit2's apply API aborts
//!     the whole patch on the first conflicting hunk.
//! Keeping everything on the `git` CLI also means one consistent failure mode.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A serializable bundle of a workspace's uncommitted working state.
///
/// Travels from the source host to the target host (over the daemon's ws
/// transport) and is replayed there with [`apply_snapshot`].
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WorkspaceSnapshot {
    /// Unified diff of tracked modifications vs `HEAD`, with standard
    /// `a/`…`b/` prefixes so it is portable to any checkout at the same base.
    /// Empty when there are no tracked changes.
    pub tracked_patch: String,
    /// Untracked, non-ignored files as `(repo-relative path, raw contents)`.
    /// Stored as bytes so binary files survive the round trip.
    pub untracked: Vec<(PathBuf, Vec<u8>)>,
    /// The `HEAD` commit the patch was taken against, for the target to sanity
    /// check it is replaying onto the same base. `None` for an unborn branch
    /// (a fresh repo with no commits yet).
    pub base_commit: Option<String>,
}

impl WorkspaceSnapshot {
    /// True when there is nothing to apply (no tracked diff, no untracked
    /// files). Lets callers skip the apply step entirely.
    pub fn is_empty(&self) -> bool {
        self.tracked_patch.is_empty() && self.untracked.is_empty()
    }
}

/// Report of an [`apply_snapshot`] run. Mirrors Warp's behavior: a failed hunk
/// does not abort the apply — we record it and keep going so the target lands
/// in the best partial state we can manage rather than nothing.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ApplyReport {
    /// Repo-relative paths whose tracked patch applied (fully or partially).
    pub applied_files: Vec<PathBuf>,
    /// Hunks the patch could not apply, as `(path, human description)`. A
    /// non-empty list means the target's working tree diverged from the
    /// source's base for those regions; the corresponding `.rej` files are
    /// left in the tree exactly like a manual `git apply --reject`.
    pub failed_hunks: Vec<(PathBuf, String)>,
    /// Untracked files written on the target.
    pub wrote_untracked: Vec<PathBuf>,
}

impl ApplyReport {
    /// True when every tracked hunk applied. Untracked writes never "fail"
    /// (we create parent dirs), so they don't affect this.
    pub fn is_clean(&self) -> bool {
        self.failed_hunks.is_empty()
    }
}

/// Errors that prevent a snapshot from being *captured*. Apply, by contrast,
/// is best-effort and surfaces problems through [`ApplyReport`] instead of
/// failing.
#[derive(Debug, thiserror::Error)]
pub enum SnapshotError {
    #[error("repo path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Git(String),
}

type Result<T> = std::result::Result<T, SnapshotError>;

// === Capture ===============================================================

/// Capture the uncommitted working state of `repo`.
///
/// Tracked changes become a unified diff vs `HEAD`; untracked, non-ignored
/// files become path + bytes. The result is fully serializable and can be
/// shipped to another host.
pub fn capture_uncommitted(repo: &Path) -> Result<WorkspaceSnapshot> {
    let base_commit = head_commit(repo)?;
    let tracked_patch = capture_tracked_patch(repo, base_commit.is_some())?;
    let untracked = capture_untracked(repo)?;
    Ok(WorkspaceSnapshot {
        tracked_patch,
        untracked,
        base_commit,
    })
}

/// `git diff HEAD` of tracked files, with forced standard prefixes.
///
/// We pin `--src-prefix=a/ --dst-prefix=b/` and `--no-ext-diff` because a
/// user's global git config (e.g. `diff.mnemonicPrefix`, `diff.external`) can
/// otherwise emit `c/`…`w/` prefixes or a custom format that `git apply`
/// chokes on. `--binary` keeps binary tracked changes representable.
fn capture_tracked_patch(repo: &Path, has_head: bool) -> Result<String> {
    if !has_head {
        // No HEAD yet (unborn branch): there is nothing committed to diff
        // against, so every present file is "untracked" and captured below.
        return Ok(String::new());
    }
    let output = git(
        repo,
        [
            "diff",
            "--no-color",
            "--no-ext-diff",
            "--src-prefix=a/",
            "--dst-prefix=b/",
            "--binary",
            "HEAD",
        ],
    )?;
    Ok(String::from_utf8_lossy(&output).into_owned())
}

/// Untracked, non-ignored files via `git ls-files --others --exclude-standard`.
/// `-z` gives NUL-separated paths so names with spaces/newlines survive.
fn capture_untracked(repo: &Path) -> Result<Vec<(PathBuf, Vec<u8>)>> {
    let output = git(repo, ["ls-files", "--others", "--exclude-standard", "-z"])?;
    let mut files = Vec::new();
    for raw in output.split(|&b| b == 0) {
        if raw.is_empty() {
            continue;
        }
        let rel = bytes_to_path(raw);
        let abs = repo.join(&rel);
        // The file can vanish between ls-files and read (racing editor). Skip
        // rather than fail the whole capture.
        match std::fs::read(&abs) {
            Ok(contents) => files.push((rel, contents)),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(SnapshotError::Io(err)),
        }
    }
    Ok(files)
}

/// Resolve `HEAD` to a commit sha. Returns `None` for an unborn branch (fresh
/// repo with no commits) or a non-repo path — both mean "no base to diff
/// against", and capture then treats every file as untracked.
fn head_commit(repo: &Path) -> Result<Option<String>> {
    let output = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(["rev-parse", "HEAD"])
        .output()?;
    if output.status.success() {
        let sha = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok((!sha.is_empty()).then_some(sha))
    } else {
        Ok(None)
    }
}

// === Apply =================================================================

/// Apply a [`WorkspaceSnapshot`] onto `repo` (expected to be a fresh clone at
/// the snapshot's `base_commit`).
///
/// Best-effort and never panics: the tracked patch is applied with
/// `git apply --reject` so hunks that fit land and the rest are recorded in
/// [`ApplyReport::failed_hunks`] (with `.rej` files left in the tree, exactly
/// like a manual reject). Untracked files are then written, creating parent
/// directories as needed.
///
/// ## Where this slots into promote/demote (future, NOT built here)
/// The movable-home flow (Wave 3) will be, on the target host:
/// ```text
///   provision_from_git(repo_url, ref)   // clone history  (workspace_provision)
///   apply_snapshot(&cloned_path, &snap) // replay working state  (this fn)
///   resume_prompt_queues(...)           // ship + resume agents  (Wave 3C)
/// ```
/// The orchestration that decides *when* to capture on the source, ship the
/// `WorkspaceSnapshot`, and call this is a held task pending a product
/// decision on the home-pointer control plane — do not wire it here.
pub fn apply_snapshot(repo: &Path, snapshot: &WorkspaceSnapshot) -> ApplyReport {
    let mut report = ApplyReport::default();
    apply_tracked_patch(repo, &snapshot.tracked_patch, &mut report);
    write_untracked(repo, &snapshot.untracked, &mut report);
    report
}

fn apply_tracked_patch(repo: &Path, patch: &str, report: &mut ApplyReport) {
    if patch.trim().is_empty() {
        return;
    }

    let child = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(["apply", "--reject", "--whitespace=nowarn"])
        // Read the patch from stdin so we never touch a temp file.
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let mut child = match child {
        Ok(child) => child,
        Err(err) => {
            report
                .failed_hunks
                .push((PathBuf::new(), format!("failed to spawn git apply: {err}")));
            return;
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        let _ = stdin.write_all(patch.as_bytes());
        // Drop closes stdin so git sees EOF and proceeds.
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => {
            report
                .failed_hunks
                .push((PathBuf::new(), format!("git apply did not complete: {err}")));
            return;
        }
    };

    // `git apply --reject` reports progress on stderr, one stanza per file:
    //   "Applied patch <file> cleanly."
    //   "Applying patch <file> with N reject..."  +  "Rejected hunk #K."
    // Exit status is non-zero when any hunk was rejected, but partial work is
    // already on disk — so we parse the report rather than trusting the code.
    parse_apply_report(&output.stderr, report);
}

/// Parse the stderr of `git apply --reject` into applied files + failed hunks.
fn parse_apply_report(stderr: &[u8], report: &mut ApplyReport) {
    let text = String::from_utf8_lossy(stderr);
    let mut current_reject_file: Option<PathBuf> = None;

    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("Applied patch ") {
            // "Applied patch <file> cleanly."
            if let Some(file) = rest.strip_suffix(" cleanly.") {
                push_unique(&mut report.applied_files, PathBuf::from(file));
            }
            current_reject_file = None;
        } else if let Some(rest) = line.strip_prefix("Applying patch ") {
            // "Applying patch <file> with N reject..." — partial apply; the
            // file still received the hunks that fit, so it counts as applied.
            let file = rest
                .split(" with ")
                .next()
                .unwrap_or("")
                .trim()
                .trim_end_matches("...");
            let path = PathBuf::from(file);
            push_unique(&mut report.applied_files, path.clone());
            current_reject_file = Some(path);
        } else if let Some(rest) = line.strip_prefix("Rejected hunk #") {
            let hunk = rest.trim_end_matches('.');
            let path = current_reject_file.clone().unwrap_or_default();
            report
                .failed_hunks
                .push((path, format!("rejected hunk #{hunk}")));
        } else if let Some(rest) = line.strip_prefix("error: patch failed: ") {
            // "error: patch failed: <file>:<line>" — a hunk could not be
            // located. The follow-up "Rejected hunk" lines carry the detail;
            // this branch is a fallback when --reject can't even stage the
            // file (e.g. the target file is missing entirely) and so no
            // "Rejected hunk" line will follow.
            let file = rest.split(':').next().unwrap_or(rest).trim();
            let path = PathBuf::from(file);
            if current_reject_file.as_deref() != Some(path.as_path()) {
                report.failed_hunks.push((path, line.to_string()));
            }
        }
    }
}

fn write_untracked(
    repo: &Path,
    untracked: &[(PathBuf, Vec<u8>)],
    report: &mut ApplyReport,
) {
    for (rel, contents) in untracked {
        let abs = repo.join(rel);
        if let Some(parent) = abs.parent() {
            if let Err(err) = std::fs::create_dir_all(parent) {
                report
                    .failed_hunks
                    .push((rel.clone(), format!("failed to create parent dir: {err}")));
                continue;
            }
        }
        match std::fs::write(&abs, contents) {
            Ok(()) => report.wrote_untracked.push(rel.clone()),
            Err(err) => report.failed_hunks.push((
                rel.clone(),
                format!("failed to write untracked file: {err}"),
            )),
        }
    }
}

// === helpers ===============================================================

/// Run `git -C <repo> <args...>`, returning stdout bytes on success.
fn git<const N: usize>(repo: &Path, args: [&str; N]) -> Result<Vec<u8>> {
    repo.to_str()
        .ok_or_else(|| SnapshotError::NonUtf8Path(repo.to_path_buf()))?;
    let output = crate::hidden_std_command("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        Err(SnapshotError::Git(format!(
            "git {} failed: {}",
            args.first().copied().unwrap_or(""),
            stderr.trim()
        )))
    }
}

#[cfg(unix)]
fn bytes_to_path(raw: &[u8]) -> PathBuf {
    use std::os::unix::ffi::OsStrExt;
    PathBuf::from(std::ffi::OsStr::from_bytes(raw))
}

#[cfg(not(unix))]
fn bytes_to_path(raw: &[u8]) -> PathBuf {
    PathBuf::from(String::from_utf8_lossy(raw).into_owned())
}

fn push_unique(vec: &mut Vec<PathBuf>, path: PathBuf) {
    if !vec.contains(&path) {
        vec.push(path);
    }
}
