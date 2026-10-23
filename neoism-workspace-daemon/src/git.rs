//! Async handlers for [`GitClientMessage`].
//!
//! Uses libgit2 (via `git2`) on a blocking task to keep the tokio reactor
//! responsive. The repository is opened from the workspace root resolved by
//! [`crate::files::workspace_root`].
//!
//! All operations reject when the workspace root is not a git repository,
//! producing a `GitServerMessage::Error` reply.

#[path = "git_blocks.rs"]
mod git_blocks;
pub use git_blocks::apply_change_block;

use std::path::Path;

use git2::{DiffFormat, DiffOptions, Repository, Status, StatusOptions};
use neoism_protocol::git::{
    CommitSummary, DiffHunk, GitChangeStatus, GitClientMessage, GitFileChange,
    GitFileDiff, GitFileStatus, GitServerMessage, GitStatusEntry,
};
use neoism_protocol::pairing::Permission;

use crate::files::{resolve_path, workspace_root};

/// Permission the socket layer must check before dispatching `msg`.
/// Mutating verbs (stage / unstage / commit / checkout) ride the same
/// write gate as file writes; everything else stays read-only.
pub fn required_permission(msg: &GitClientMessage) -> Permission {
    if matches!(msg, GitClientMessage::PullRequest { request } if request.is_mutation()) {
        return Permission::GitWrite;
    }
    if msg.is_mutation() {
        Permission::WriteFiles
    } else {
        Permission::ReadFiles
    }
}

fn err(msg: impl Into<String>) -> Vec<GitServerMessage> {
    vec![GitServerMessage::Error {
        message: msg.into(),
    }]
}

/// Dispatch a single git message.
pub async fn handle(msg: GitClientMessage) -> Vec<GitServerMessage> {
    handle_with_root(workspace_root(), msg).await
}

/// Late cancelled subscription tasks must not replace a newer scope on the
/// same socket. Equal IDs are allowed for reconnect/open-revision renewal.
pub(crate) fn accept_watch_request(high_water: &mut u64, request_id: u64) -> bool {
    if request_id < *high_water {
        return false;
    }
    *high_water = request_id;
    true
}

/// [`handle`] against an explicit repo root — the `workspace_root`
/// envelope override, so a guest browsing a JOINED workspace gets git
/// status for THAT workspace's repo rather than the daemon's default
/// root. Mirrors the files plane's `handle_with_root`.
pub async fn handle_with_root(
    root: std::path::PathBuf,
    msg: GitClientMessage,
) -> Vec<GitServerMessage> {
    if let GitClientMessage::PullRequest { request } = msg {
        let mutated = request.is_mutation();
        let reply = crate::pull_request::handle(root.clone(), request).await;
        if mutated { crate::git_snapshot::invalidate(&root).await; }
        return vec![GitServerMessage::PullRequest { reply }];
    }
    if let GitClientMessage::Blame { path } = msg {
        return crate::git_blame::handle(root, path).await;
    }
    if matches!(msg, GitClientMessage::ChangedFiles) {
        let snapshot = crate::git_snapshot::snapshot(root).await;
        return vec![GitServerMessage::ChangedFiles {
            files: snapshot.files,
            branch: snapshot.branch,
            upstream: snapshot.upstream,
            error: snapshot.error,
        }];
    }
    let mutated = msg.is_mutation();
    let cache_root = root.clone();
    let result = tokio::task::spawn_blocking(move || handle_blocking(&root, msg)).await;
    if mutated {
        crate::git_snapshot::invalidate(&cache_root).await;
    }
    match result {
        Ok(out) => out,
        Err(e) => err(format!("git task join error: {e}")),
    }
}

/// Socket entry point: restores finish on the existing authoritative hub
/// before Git success. Clients must consume Sync/Saved or reopen a snapshot,
/// not publish a second disk-derived edit into their local Yrs replica.
/// The pure `handle_with_root`/`apply_change_block` API remains for NativeIo.
pub async fn handle_with_root_and_crdt(
    root: std::path::PathBuf,
    msg: GitClientMessage,
    crdt: crate::crdt::sync::CrdtSyncHub,
) -> Vec<GitServerMessage> {
    if let GitClientMessage::PullRequest { request } = msg {
        let mutated = request.is_mutation();
        let reply = crate::pull_request::handle_with_hub(root.clone(), request, crdt).await;
        if mutated { crate::git_snapshot::invalidate(&root).await; }
        return vec![GitServerMessage::PullRequest { reply }];
    }
    if !matches!(
        &msg,
        GitClientMessage::ApplyBlock { restore: true, .. }
            | GitClientMessage::Restore { .. }
    ) {
        return handle_with_root(root, msg).await;
    }
    let cache_root = root.clone();
    let result = tokio::task::spawn_blocking(move || {
        let repo = match Repository::discover(&root) {
            Ok(repo) => repo,
            Err(e) => return err(e.to_string()),
        };
        let Some(wd) = repo.workdir() else {
            return err("repository has no workdir");
        };
        let path = match &msg {
            GitClientMessage::ApplyBlock { path, .. }
            | GitClientMessage::Restore { path } => path,
            _ => unreachable!(),
        };
        // Git paths are repo-relative, but the caller's declared workspace
        // remains the permission boundary (including canonical/symlink checks).
        let target = match resolve_path(wd, path) {
            Ok(path) => path,
            Err(e) => return err(e),
        };
        if !crate::path::canonicalize_lossy(&target)
            .starts_with(crate::path::canonicalize_lossy(&root))
        {
            return err("restore path is outside the declared workspace");
        }
        let result = crdt.guarded_restore(&target, || match &msg {
            GitClientMessage::ApplyBlock {
                path,
                block,
                restore,
            } => apply_change_block(&root, path, block, *restore),
            GitClientMessage::Restore { path } => restore_file(wd, path),
            _ => unreachable!(),
        });
        vec![changed_files_reply(wd, result.err())]
    })
    .await;
    crate::git_snapshot::invalidate(&cache_root).await;
    match result {
        Ok(replies) => replies,
        Err(e) => err(format!("git task join error: {e}")),
    }
}

/// Resolve the current branch of the workspace root repo. Returns a
/// `Branch { name }` reply whose `name` is `None` when the workspace
/// isn't a git repo or HEAD is detached. Used for the unsolicited
/// status snapshot the daemon sends on WebSocket connect.
pub async fn current_branch_snapshot() -> GitServerMessage {
    let root = workspace_root();
    let join = tokio::task::spawn_blocking(move || resolve_branch(&root)).await;
    match join {
        Ok(name) => GitServerMessage::Branch { name },
        Err(_) => GitServerMessage::Branch { name: None },
    }
}

/// Working-tree change totals in LINES `(added, deleted)` — the same
/// semantics as the desktop status pill so web and desktop bottom bars
/// agree: `git diff HEAD --numstat` totals plus every untracked file's
/// line count folded into `added`.
///
/// Returns `(0, 0)` if the path isn't a git repo or `git` isn't on
/// `PATH` — the caller treats that as a no-op (status pill stays at the
/// last known counts).
pub fn git_changes_snapshot(repo: &Path) -> (u64, u64) {
    // LINE totals, not file counts — mirrors the desktop status pill
    // (`neoism-ui::panels::git_branch::read_change_summary`): tracked
    // changes via `git diff HEAD --numstat`, plus every untracked
    // file's line count folded into `added`, so the pill agrees with
    // the side diff panel's `+N -M` header.
    let numstat = crate::process::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(["diff", "HEAD", "--numstat", "--no-color"])
        .output();
    let (mut added, deleted) = match numstat {
        Ok(output) if output.status.success() => {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let mut added: u64 = 0;
            let mut deleted: u64 = 0;
            for line in stdout.lines() {
                // numstat: "<added>\t<deleted>\t<path>" — binary files
                // report "-" for both counts; skip them.
                let mut cols = line.split('\t');
                let (Some(a), Some(d)) = (cols.next(), cols.next()) else {
                    continue;
                };
                added += a.parse::<u64>().unwrap_or(0);
                deleted += d.parse::<u64>().unwrap_or(0);
            }
            (added, deleted)
        }
        _ => return (0, 0),
    };

    // Untracked files don't show up in `diff HEAD`; count their lines
    // as additions.
    if let Ok(output) = crate::process::background_command("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .arg("-C")
        .arg(repo)
        .args(["status", "--porcelain=v1", "--untracked-files=all"])
        .output()
    {
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for line in stdout.lines() {
                let Some(path) = line.strip_prefix("?? ") else {
                    continue;
                };
                if let Ok(contents) = std::fs::read(repo.join(path)) {
                    added = added.saturating_add(bytecount_lines(&contents) as u64);
                }
            }
        }
    }
    (added, deleted)
}

/// Count newline-terminated lines, treating a trailing partial line as
/// one more (matches `wc -l` + 1-for-no-trailing-newline semantics the
/// desktop pill uses).
fn bytecount_lines(bytes: &[u8]) -> usize {
    if bytes.is_empty() {
        return 0;
    }
    let newlines = bytes.iter().filter(|b| **b == b'\n').count();
    if bytes.ends_with(b"\n") {
        newlines
    } else {
        newlines + 1
    }
}

fn resolve_branch(root: &Path) -> Option<String> {
    let repo = Repository::discover(root).ok()?;
    let head = match repo.head() {
        Ok(head) => head,
        Err(_) => {
            return repo
                .find_reference("HEAD")
                .ok()?
                .symbolic_target()
                .and_then(|name| name.strip_prefix("refs/heads/"))
                .map(str::to_owned)
        }
    };
    if head.is_branch() {
        head.shorthand().map(str::to_owned)
    } else {
        // Detached HEAD — fall back to short SHA so the status line shows
        // *something* useful instead of nothing.
        head.target().map(|oid| {
            let s = oid.to_string();
            s.chars().take(7).collect::<String>()
        })
    }
}

fn handle_blocking(root: &Path, msg: GitClientMessage) -> Vec<GitServerMessage> {
    let repo = match Repository::discover(root) {
        Ok(r) => r,
        Err(e) => return err(format!("not a git repository at {}: {e}", root.display())),
    };
    match msg {
        GitClientMessage::WatchStatus { .. } | GitClientMessage::UnwatchStatus { .. } => {
            vec![]
        }
        GitClientMessage::PullRequest { request } => vec![GitServerMessage::PullRequest { reply: crate::pull_request::handle_at(root, request) }],
        GitClientMessage::Blame { .. } => err("Blame requires async dispatch"),
        GitClientMessage::Status => status(&repo),
        GitClientMessage::Diff { path } => diff(&repo, path.as_deref(), root),
        GitClientMessage::Log { max_count } => log(&repo, max_count),
        GitClientMessage::ReadCommit { revision } => {
            match read_commit(&repo, &revision) {
                Ok(detail) => vec![GitServerMessage::CommitDetail { detail }],
                Err(e) => err(e),
            }
        }
        // ── Web git-panel write parity (Pass 2) ──────────────────────
        // These shell out to `git` with the exact flags the desktop
        // panel's `NativeGitDiffIo` uses (see
        // `neoism-frontend/desktop/src/editor/git_diff_panel/io.rs`) so
        // web and desktop observe identical semantics. All of them run
        // against the repo's TOPLEVEL workdir — the panel's paths are
        // repo-relative, and the workspace root may sit below it.
        GitClientMessage::ChangedFiles => {
            with_workdir(&repo, |wd| vec![changed_files_reply(wd, None)])
        }
        GitClientMessage::Stage { path } => with_workdir(&repo, |wd| {
            if let Err(e) = validate_git_path(wd, &path) {
                return vec![changed_files_reply(wd, Some(e))];
            }
            let result = run_git(wd, &["add", "--", &path]);
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Unstage { path } => with_workdir(&repo, |wd| {
            if let Err(e) = validate_git_path(wd, &path) {
                return vec![changed_files_reply(wd, Some(e))];
            }
            // Desktop parity: `git restore --staged`, falling back to
            // the older `git reset` for git builds without `restore`.
            let result = run_git(wd, &["restore", "--staged", "--", &path])
                .or_else(|_| run_git(wd, &["reset", "-q", "HEAD", "--", &path]));
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::ReadBase { path } => {
            with_workdir(&repo, |wd| match load_base(wd, &path) {
                Ok(text) => vec![GitServerMessage::BaseFile { path, text }],
                Err(e) => err(e),
            })
        }
        GitClientMessage::Restore { path } => with_workdir(&repo, |wd| {
            vec![changed_files_reply(wd, restore_file(wd, &path).err())]
        }),
        GitClientMessage::ApplyBlock {
            path,
            block,
            restore,
        } => with_workdir(&repo, |wd| {
            vec![changed_files_reply(
                wd,
                apply_change_block(root, &path, &block, restore).err(),
            )]
        }),
        GitClientMessage::Commit { message } => with_workdir(&repo, |wd| {
            let result = if message.trim().is_empty() {
                Err("Commit message is empty".to_string())
            } else {
                run_git(wd, &["commit", "-m", &message])
            };
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Branches => with_workdir(&repo, |wd| {
            vec![GitServerMessage::Branches {
                branches: list_branches(wd),
            }]
        }),
        GitClientMessage::Checkout { branch } => with_workdir(&repo, |wd| {
            // `git switch`, falling back to `git checkout` — same
            // two-step the desktop panel runs.
            let result = run_git(wd, &["switch", &branch])
                .or_else(|_| run_git(wd, &["checkout", &branch]));
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::Fetch => with_workdir(&repo, |wd| {
            vec![changed_files_reply(wd, run_git(wd, &["fetch"]).err())]
        }),
        GitClientMessage::Pull => with_workdir(&repo, |wd| {
            vec![changed_files_reply(
                wd,
                run_git(wd, &["pull", "--ff-only"]).err(),
            )]
        }),
        GitClientMessage::Publish => {
            with_workdir(&repo, |wd| vec![changed_files_reply(wd, publish(wd).err())])
        }
        GitClientMessage::Push => with_workdir(&repo, |wd| {
            let result = crate::pull_request::push_if_associated(root).unwrap_or_else(|| run_git(wd, &["push"]));
            vec![changed_files_reply(wd, result.err())]
        }),
        GitClientMessage::DiffFiles { paths } => with_workdir(&repo, |wd| {
            let mut diffs = Vec::with_capacity(paths.len());
            for path in paths {
                if resolve_path(wd, &path).is_err() {
                    continue;
                }
                diffs.push(GitFileDiff {
                    patch: load_file_diff(wd, &path),
                    path,
                });
            }
            vec![GitServerMessage::FileDiffs { diffs }]
        }),
    }
}

/// Run `f` against the repository's toplevel working directory, or
/// reply with an error for bare repositories.
fn with_workdir<F>(repo: &Repository, f: F) -> Vec<GitServerMessage>
where
    F: FnOnce(&Path) -> Vec<GitServerMessage>,
{
    match repo.workdir() {
        Some(wd) => f(wd),
        None => err("bare repository has no working tree"),
    }
}

/// A single literal directory pathspec stages/unstages all descendants in one
/// git invocation (including deletions), rather than issuing per-file writes.
/// Reject whole-repo and escaping paths before handing them to git. Deleted
/// paths need not exist, so check the nearest existing ancestor for symlinks.
fn validate_git_path(workdir: &Path, path: &str) -> Result<(), String> {
    let relative = Path::new(path);
    if relative.is_absolute()
        || relative
            .components()
            .all(|c| !matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "expected a repo-relative file or folder path: {path}"
        ));
    }
    let resolved = resolve_path(workdir, path)?;
    let canonical_root = workdir.canonicalize().map_err(|e| e.to_string())?;
    let mut ancestor = resolved.as_path();
    while !ancestor.exists() {
        ancestor = ancestor
            .parent()
            .ok_or_else(|| format!("invalid git path: {path}"))?;
    }
    if !ancestor
        .canonicalize()
        .map_err(|e| e.to_string())?
        .starts_with(&canonical_root)
    {
        return Err(format!(
            "git path escapes repository through a symlink: {path}"
        ));
    }
    Ok(())
}

/// Full HEAD baseline; never use index/worktree contents or lossy UTF-8 conversion.
fn load_base(workdir: &Path, path: &str) -> Result<String, String> {
    validate_git_path(workdir, path)?;
    if path.contains('\0')
        || Path::new(path)
            .components()
            .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path
            .split(['/', '\\'])
            .any(|c| c.eq_ignore_ascii_case(".git"))
    {
        return Err("ReadBase requires a literal repo-relative file path".into());
    }
    let repo = Repository::open(workdir).map_err(|e| e.to_string())?;
    let head = match repo.head() {
        Ok(head) => head,
        Err(e)
            if matches!(
                e.code(),
                git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
            ) =>
        {
            return Ok(String::new())
        }
        Err(e) => return Err(e.to_string()),
    };
    let tree = head.peel_to_tree().map_err(|e| e.to_string())?;
    let entry = match tree.get_path(Path::new(path)) {
        Ok(entry) => entry,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok(String::new()),
        Err(e) => return Err(e.to_string()),
    };
    let odb = repo.odb().map_err(|e| e.to_string())?;
    let (size, kind) = odb.read_header(entry.id()).map_err(|e| e.to_string())?;
    if kind != git2::ObjectType::Blob {
        return Err("HEAD baseline is not a file".into());
    }
    if size > 2 * 1024 * 1024 {
        return Err("HEAD baseline exceeds 2 MiB".into());
    }
    let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
    if blob.is_binary() || blob.content().contains(&0) {
        return Err("HEAD baseline is binary".into());
    }
    String::from_utf8(blob.content().to_vec())
        .map_err(|_| "HEAD baseline is not UTF-8".into())
}

/// Restore only an exact index entry, never a directory/pathspec or untracked file.
fn restore_file(workdir: &Path, path: &str) -> Result<(), String> {
    validate_git_path(workdir, path)?;
    if Path::new(path)
        .components()
        .any(|c| !matches!(c, std::path::Component::Normal(_)))
        || path
            .split(['/', '\\'])
            .any(|part| part.eq_ignore_ascii_case(".git"))
        || path.contains('\0')
    {
        return Err("Restore requires a literal repo-relative file path".into());
    }
    let output = git_command(workdir)
        .args(["ls-files", "--cached", "-z", "--", path])
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success()
        || !output
            .stdout
            .split(|b| *b == 0)
            .any(|entry| entry == path.as_bytes())
    {
        return Err(
            "Cannot restore an untracked file; remove it explicitly instead".into(),
        );
    }
    run_git(workdir, &["restore", "--worktree", "--", path])
}

/// Resolve configured tracking metadata without requiring the remote ref to exist.
fn resolve_upstream(workdir: &Path) -> Option<String> {
    let repo = Repository::discover(workdir).ok()?;
    let head = repo.find_reference("HEAD").ok()?;
    let branch = head.symbolic_target()?;
    let output = crate::git_snapshot::output(git_command(workdir).args([
        "for-each-ref",
        "--format=%(upstream:short)",
        branch,
    ]))
    .ok()?;
    if !output.status.success() {
        return None;
    }
    let upstream = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!upstream.is_empty()).then_some(upstream)
}

fn git_text(workdir: &Path, args: &[&str]) -> Result<String, String> {
    let output = git_command(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_owned());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// Never infer a destination from remote ordering or overwrite a remote branch.
fn publish(workdir: &Path) -> Result<(), String> {
    let branch =
        git_text(workdir, &["symbolic-ref", "--quiet", "HEAD"]).map_err(|_| {
            "Cannot publish a detached HEAD; switch to a branch first".to_owned()
        })?;
    let branch = branch.strip_prefix("refs/heads/").ok_or_else(|| {
        "Cannot publish: HEAD does not refer to a local branch".to_owned()
    })?;
    if crate::pull_request::is_associated_branch(workdir, branch)? {
        return Err("This checkout already tracks its original PR source branch; use Push, not Publish".into());
    }
    git_text(workdir, &["rev-parse", "--verify", "HEAD^{commit}"]).map_err(|_| {
        "Cannot publish a branch with no commits; create a commit first".to_owned()
    })?;
    let remotes = git_text(workdir, &["remote"])?;
    let remotes: Vec<_> = remotes.lines().filter(|s| !s.is_empty()).collect();
    let configured = git_text(
        workdir,
        &["config", "--get", &format!("branch.{branch}.pushRemote")],
    )
    .ok()
    .filter(|s| !s.is_empty())
    .or_else(|| {
        git_text(workdir, &["config", "--get", "remote.pushDefault"])
            .ok()
            .filter(|s| !s.is_empty())
    });
    let remote = if let Some(remote) = configured {
        if !remotes.contains(&remote.as_str()) {
            return Err(format!(
                "Publish remote '{remote}' is not a configured remote"
            ));
        }
        remote
    } else if remotes.contains(&"origin") {
        "origin".to_owned()
    } else {
        match remotes.as_slice() {
            [] => return Err("Cannot publish: no Git remote is configured".to_owned()),
            [remote] => (*remote).to_owned(),
            _ => return Err("Cannot publish: multiple remotes; configure branch.pushRemote or remote.pushDefault".to_owned()),
        }
    };
    // Explicit same-name refspec ignores push.default and remote.<name>.push.
    run_git(
        workdir,
        &[
            "push",
            "--set-upstream",
            "--",
            &remote,
            &format!("refs/heads/{branch}:refs/heads/{branch}"),
        ],
    )
}

fn changed_files_reply(workdir: &Path, error: Option<String>) -> GitServerMessage {
    GitServerMessage::ChangedFiles {
        files: collect_changed_files(workdir),
        branch: resolve_branch(workdir),
        upstream: resolve_upstream(workdir),
        error,
    }
}

/// Port of the desktop panel's `collect_files`
/// (`desktop/src/editor/git_diff_panel/io.rs`): `git status
/// --porcelain=v1 -z --untracked-files=all` for the entry list +
/// staged bit, `git diff HEAD --numstat -z` for per-file line counts,
/// and a raw line count for untracked files.
fn collect_changed_files(workdir: &Path) -> Vec<GitFileChange> {
    collect_changed_files_checked(workdir).unwrap_or_default()
}

fn collect_changed_files_checked(workdir: &Path) -> Result<Vec<GitFileChange>, String> {
    let status = match crate::git_snapshot::output(git_command(workdir).args([
        "status",
        "--porcelain=v1",
        "-z",
        "--untracked-files=all",
    ])) {
        Ok(o) if o.status.success() => o.stdout,
        Ok(o) => return Err(String::from_utf8_lossy(&o.stderr).into_owned()),
        Err(e) => return Err(e.to_string()),
    };

    let numstat = crate::git_snapshot::output(git_command(workdir).args([
        "diff",
        "HEAD",
        "--numstat",
        "-z",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
    ]))
    .map_err(|e| e.to_string())?;
    let numstat = Some(numstat)
        .filter(|o| o.status.success())
        .map(|o| neoism_ui::panels::git_diff::parse_numstat(&o.stdout))
        .unwrap_or_default();

    let mut files = Vec::new();
    let mut i = 0usize;
    while i < status.len() {
        let start = i;
        while i < status.len() && status[i] != 0 {
            i += 1;
        }
        let record = &status[start..i];
        i = i.saturating_add(1);
        if record.len() < 4 {
            continue;
        }
        let x = record[0] as char;
        let y = record[1] as char;
        let path = String::from_utf8_lossy(&record[3..]).into_owned();

        let status_kind = if x == '?' || y == '?' {
            GitChangeStatus::Untracked
        } else if matches!((x, y), ('A', 'A') | ('D', 'D')) || x == 'U' || y == 'U' {
            GitChangeStatus::Conflict
        } else if !matches!(x, ' ' | '?') && !matches!(y, ' ' | '?') {
            GitChangeStatus::Mixed
        } else if x == 'D' || y == 'D' {
            GitChangeStatus::Deleted
        } else if x == 'A' || y == 'A' {
            GitChangeStatus::Added
        } else if x == 'R' || y == 'R' {
            GitChangeStatus::Renamed
        } else if matches!(x, 'M' | 'T') {
            GitChangeStatus::Staged
        } else {
            GitChangeStatus::Modified
        };

        // Rename/copy records carry the ORIGINAL path in a second
        // NUL-separated field — skip it, same as the desktop parser.
        if matches!(x, 'R' | 'C') || matches!(y, 'R' | 'C') {
            while i < status.len() && status[i] != 0 {
                i += 1;
            }
            i = i.saturating_add(1);
        }

        let (additions, deletions) = if matches!(status_kind, GitChangeStatus::Untracked)
        {
            let line_count = std::fs::File::open(workdir.join(&path))
                .and_then(|file| {
                    use std::io::Read;
                    let mut file = file;
                    let mut buf = [0u8; 8192];
                    let mut lines = 0u32;
                    let mut last = None;
                    loop {
                        let n = file.read(&mut buf)?;
                        if n == 0 {
                            break;
                        }
                        if buf[..n].contains(&0) {
                            return Ok(0);
                        }
                        lines = lines.saturating_add(
                            buf[..n].iter().filter(|&&b| b == b'\n').count() as u32,
                        );
                        last = Some(buf[n - 1]);
                    }
                    Ok(lines.saturating_add(u32::from(last.is_some_and(|b| b != b'\n'))))
                })
                .unwrap_or(0);
            (line_count, 0)
        } else {
            numstat.get(&path).copied().unwrap_or((0, 0))
        };
        // Index column (`x`) non-empty ⇒ the file has staged content.
        // Untracked (`?`) and worktree-only (` `) read as unstaged. A
        // partially-staged file (both columns dirty) still reads staged.
        let staged = !matches!(x, ' ' | '?');
        files.push(GitFileChange {
            path,
            status: status_kind,
            additions,
            deletions,
            staged,
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// List every local branch, newest committer date first — desktop
/// `list_branches` parity. Empty on git failure.
fn list_branches(workdir: &Path) -> Vec<String> {
    let output = git_command(workdir)
        .args([
            "for-each-ref",
            "--format=%(refname:short)",
            "--sort=-committerdate",
            "refs/heads",
        ])
        .output();
    match output {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .lines()
            .map(|l| l.trim().to_string())
            .filter(|l| !l.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

/// Desktop `load_diff` parity: raw patch text for one file. Tracked
/// files diff against HEAD (so fully-staged changes still show);
/// untracked files diff against `/dev/null` via `--no-index`, which
/// exits non-zero by design when a diff exists.
fn load_file_diff(workdir: &Path, path: &str) -> String {
    let tracked = crate::git_snapshot::output(git_command(workdir).args([
        "diff",
        "HEAD",
        "--no-color",
        "--no-ext-diff",
        "--no-textconv",
        "--",
        path,
    ]));
    if let Ok(o) = &tracked {
        if o.status.success() && !o.stdout.is_empty() {
            return String::from_utf8_lossy(&o.stdout).into_owned();
        }
    }
    // Empty tracked diff — the file may be untracked (or new+staged
    // with HEAD unborn). `--no-index` renders the whole file as
    // additions, matching the desktop's untracked branch.
    let abs = workdir.join(path);
    if !abs.is_file() {
        return String::new();
    }
    let null = if cfg!(windows) { "NUL" } else { "/dev/null" };
    let untracked = crate::git_snapshot::output(
        git_command(workdir)
            .args([
                "diff",
                "--no-index",
                "--no-color",
                "--no-ext-diff",
                "--no-textconv",
                "--",
                null,
            ])
            .arg(&abs),
    );
    match untracked {
        // `--no-index` exits 1 when the files differ; take stdout
        // whenever git produced any.
        Ok(o) if !o.stdout.is_empty() => String::from_utf8_lossy(&o.stdout).into_owned(),
        _ => String::new(),
    }
}

/// Run a git subcommand in `workdir`, mapping a non-zero exit to the
/// trimmed stderr (or stdout) so the panel can surface it — a direct
/// port of the desktop panel's `run_git`.
fn run_git(workdir: &Path, args: &[&str]) -> Result<(), String> {
    let output = git_command(workdir)
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if output.status.success() {
        return Ok(());
    }
    let mut msg = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if msg.is_empty() {
        msg = String::from_utf8_lossy(&output.stdout).trim().to_string();
    }
    if msg.is_empty() {
        msg = "git command failed".to_string();
    }
    Err(msg)
}

/// Base `git` invocation rooted at `workdir` with the same
/// lock-avoidance env the rest of the daemon (and the desktop panel)
/// uses.
fn git_command(workdir: &Path) -> std::process::Command {
    let mut cmd = crate::process::background_command("git");
    cmd.env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_LITERAL_PATHSPECS", "1")
        .args(["-c", "core.fsmonitor=false"])
        .arg("-C")
        .arg(workdir);
    cmd
}

fn map_status(s: Status) -> Option<GitFileStatus> {
    if s.contains(Status::CONFLICTED) {
        return Some(GitFileStatus::Conflicted);
    }
    if s.intersects(Status::INDEX_RENAMED | Status::WT_RENAMED) {
        return Some(GitFileStatus::Renamed);
    }
    if s.intersects(Status::INDEX_DELETED | Status::WT_DELETED) {
        return Some(GitFileStatus::Deleted);
    }
    if s.intersects(Status::INDEX_NEW) {
        return Some(GitFileStatus::Added);
    }
    if s.contains(Status::WT_NEW) {
        return Some(GitFileStatus::Untracked);
    }
    if s.intersects(
        Status::INDEX_MODIFIED
            | Status::WT_MODIFIED
            | Status::INDEX_TYPECHANGE
            | Status::WT_TYPECHANGE,
    ) {
        return Some(GitFileStatus::Modified);
    }
    None
}

fn status(repo: &Repository) -> Vec<GitServerMessage> {
    let mut opts = StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let statuses = match repo.statuses(Some(&mut opts)) {
        Ok(s) => s,
        Err(e) => return err(format!("git status: {e}")),
    };

    let mut entries = Vec::new();
    for entry in statuses.iter() {
        let s = entry.status();
        let Some(mapped) = map_status(s) else {
            continue;
        };
        let path = entry.path().unwrap_or("").to_string();
        entries.push(GitStatusEntry {
            path,
            status: mapped,
        });
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    vec![GitServerMessage::Status { entries }]
}

fn diff(
    repo: &Repository,
    path_filter: Option<&str>,
    root: &Path,
) -> Vec<GitServerMessage> {
    // If a path was supplied, validate it (the same traversal protection we
    // use for file ops) before handing it to libgit2 as a pathspec.
    if let Some(p) = path_filter {
        if let Err(e) = resolve_path(root, p) {
            return err(e);
        }
    }

    let mut opts = DiffOptions::new();
    opts.context_lines(3);
    if let Some(p) = path_filter {
        opts.pathspec(p);
    }

    // Diff index vs workdir (uncommitted changes). For now this matches the
    // common "what's changed in my checkout?" question; if we need staged or
    // commit-to-commit diffs later we extend the message.
    let diff = match repo.diff_index_to_workdir(None, Some(&mut opts)) {
        Ok(d) => d,
        Err(e) => return err(format!("git diff: {e}")),
    };

    let mut hunks: Vec<DiffHunk> = Vec::new();
    let foreach_res = diff.print(DiffFormat::Patch, |delta, hunk, line| {
        let Some(hunk) = hunk else {
            return true;
        };
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default();

        let origin = line.origin();
        let prefix = match origin {
            '+' | '-' | ' ' => Some(origin),
            _ => None,
        };
        let content = std::str::from_utf8(line.content()).unwrap_or("");

        let key = (
            path.clone(),
            hunk.old_start(),
            hunk.old_lines(),
            hunk.new_start(),
            hunk.new_lines(),
        );

        let existing = hunks.iter_mut().rev().find(|h| {
            h.path == key.0
                && h.old_start == key.1
                && h.old_lines == key.2
                && h.new_start == key.3
                && h.new_lines == key.4
        });

        let target = match existing {
            Some(h) => h,
            None => {
                hunks.push(DiffHunk {
                    path,
                    old_start: hunk.old_start(),
                    old_lines: hunk.old_lines(),
                    new_start: hunk.new_start(),
                    new_lines: hunk.new_lines(),
                    patch: format!(
                        "@@ -{},{} +{},{} @@\n",
                        hunk.old_start(),
                        hunk.old_lines(),
                        hunk.new_start(),
                        hunk.new_lines()
                    ),
                });
                hunks.last_mut().expect("just pushed")
            }
        };

        if let Some(p) = prefix {
            target.patch.push(p);
        }
        target.patch.push_str(content);
        true
    });

    if let Err(e) = foreach_res {
        return err(format!("git diff print: {e}"));
    }

    vec![GitServerMessage::Diff { hunks }]
}

/// Native provider shares the daemon's immutable, bounded commit reader.
pub fn read_commit_at(
    root: &Path,
    revision: &str,
) -> Result<neoism_protocol::git::GitCommitDetail, String> {
    let repo = Repository::discover(root).map_err(|e| e.to_string())?;
    read_commit(&repo, revision)
}

pub fn latest_commit_at(root: &Path) -> Option<CommitSummary> {
    let repo = Repository::discover(root).ok()?;
    match log(&repo, Some(1)).pop()? {
        GitServerMessage::Log { mut commits } => commits.pop(),
        _ => None,
    }
}

/// Resolve to an actual commit before reading trees. No revision ever reaches a
/// subprocess or an option parser. Reject option-shaped input even in libgit2.
fn read_commit(
    repo: &Repository,
    revision: &str,
) -> Result<neoism_protocol::git::GitCommitDetail, String> {
    if revision.is_empty()
        || revision.starts_with('-')
        || revision.contains('\0')
        || revision.len() > 4096
    {
        return Err("Invalid commit revision".into());
    }
    let commit = repo
        .revparse_single(revision)
        .and_then(|object| object.peel_to_commit())
        .map_err(|e| format!("Resolve commit: {e}"))?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let parent = if commit.parent_count() > 0 {
        Some(
            commit
                .parent(0)
                .and_then(|p| p.tree())
                .map_err(|e| e.to_string())?,
        )
    } else {
        None
    };
    let mut options = git2::DiffOptions::new();
    options.context_lines(3);
    let diff = repo
        .diff_tree_to_tree(parent.as_ref(), Some(&tree), Some(&mut options))
        .map_err(|e| e.to_string())?;
    let mut files = Vec::new();
    let mut diffs = Vec::new();
    let mut total = 0usize;
    for (index, delta) in diff.deltas().enumerate() {
        if index >= 2000 {
            return Err("Commit exceeds 2000 changed files".into());
        }
        let path = delta
            .new_file()
            .path()
            .or_else(|| delta.old_file().path())
            .ok_or("Commit file has no path")?
            .to_string_lossy()
            .into_owned();
        let mut patch =
            git2::Patch::from_diff(&diff, index).map_err(|e| e.to_string())?;
        let (additions, deletions, text) = if let Some(ref mut patch) = patch {
            let (_, adds, dels) = patch.line_stats().map_err(|e| e.to_string())?;
            let bytes = patch.to_buf().map_err(|e| e.to_string())?;
            (
                adds as u32,
                dels as u32,
                String::from_utf8_lossy(&bytes).into_owned(),
            )
        } else {
            (0, 0, "Binary file changed".into())
        };
        total += text.len();
        if total > 16 * 1024 * 1024 {
            return Err("Commit patches exceed 16 MiB".into());
        }
        files.push(GitFileChange {
            path: path.clone(),
            additions,
            deletions,
            staged: false,
            status: match delta.status() {
                git2::Delta::Added => GitChangeStatus::Added,
                git2::Delta::Deleted => GitChangeStatus::Deleted,
                git2::Delta::Renamed => GitChangeStatus::Renamed,
                _ => GitChangeStatus::Modified,
            },
        });
        diffs.push(neoism_protocol::git::GitFileDiff { path, patch: text });
    }
    let author = commit.author();
    Ok(neoism_protocol::git::GitCommitDetail {
        sha: commit.id().to_string(),
        author: author.name().unwrap_or("").into(),
        email: author.email().unwrap_or("").into(),
        timestamp: author.when().seconds(),
        offset_minutes: author.when().offset_minutes(),
        message: commit.message().unwrap_or("").into(),
        files,
        diffs,
    })
}

fn log(repo: &Repository, max_count: Option<u32>) -> Vec<GitServerMessage> {
    let mut revwalk = match repo.revwalk() {
        Ok(r) => r,
        Err(e) => return err(format!("git revwalk: {e}")),
    };
    if let Err(e) = revwalk.push_head() {
        // An empty repository has no HEAD; report an empty log rather than an error.
        if e.code() == git2::ErrorCode::UnbornBranch
            || e.code() == git2::ErrorCode::NotFound
        {
            return vec![GitServerMessage::Log {
                commits: Vec::new(),
            }];
        }
        return err(format!("git log push_head: {e}"));
    }

    let cap = max_count.unwrap_or(u32::MAX) as usize;
    let mut commits = Vec::new();
    for oid in revwalk.take(cap) {
        let oid = match oid {
            Ok(o) => o,
            Err(e) => return err(format!("git revwalk iter: {e}")),
        };
        let commit = match repo.find_commit(oid) {
            Ok(c) => c,
            Err(e) => return err(format!("git find_commit {oid}: {e}")),
        };
        let sha = oid.to_string();
        let short_sha: String = sha.chars().take(7).collect();
        let author = {
            let a = commit.author();
            match (a.name(), a.email()) {
                (Some(n), Some(em)) => format!("{n} <{em}>"),
                (Some(n), None) => n.to_string(),
                (None, Some(em)) => format!("<{em}>"),
                (None, None) => String::new(),
            }
        };
        commits.push(CommitSummary {
            sha,
            short_sha,
            author,
            message: commit.message().unwrap_or("").to_string(),
            timestamp: commit.time().seconds(),
        });
    }
    vec![GitServerMessage::Log { commits }]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_base_is_exact_bounded_read_only_head_text() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = Repository::init(root).unwrap();
        assert_eq!(load_base(root, "file").unwrap(), "");
        for (path, bytes) in [
            ("file", b"HEAD\r\n\n".to_vec()),
            ("binary", vec![0, 1, 2]),
            ("invalid", vec![255, 254]),
            ("huge", vec![b'x'; 2 * 1024 * 1024 + 1]),
        ] {
            std::fs::write(root.join(path), bytes).unwrap();
            run_git(root, &["add", "--", path]).unwrap();
        }
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        std::fs::write(root.join("file"), "index").unwrap();
        run_git(root, &["add", "--", "file"]).unwrap();
        std::fs::write(root.join("file"), "working").unwrap();
        assert_eq!(load_base(root, "file").unwrap(), "HEAD\r\n\n");
        assert_eq!(load_base(root, "untracked").unwrap(), "");
        for path in ["binary", "invalid", "huge", "../file", ".", ".git/config"] {
            assert!(load_base(root, path).is_err(), "accepted {path:?}");
        }
        let message = GitClientMessage::ReadBase {
            path: "file".into(),
        };
        assert!(!message.is_mutation());
        assert!(matches!(
            required_permission(&message),
            Permission::ReadFiles
        ));
        assert!(
            matches!(&handle_blocking(root, message)[0], GitServerMessage::BaseFile { path, text } if path == "file" && text == "HEAD\r\n\n")
        );
    }

    #[test]
    fn restore_preserves_index_and_refuses_unsafe_or_untracked_paths() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        let repo = Repository::init(&root).unwrap();
        std::fs::write(root.join("file"), "base\n").unwrap();
        run_git(&root, &["add", "--", "file"]).unwrap();
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("Test", "test@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        std::fs::write(root.join("file"), "staged\n").unwrap();
        run_git(&root, &["add", "--", "file"]).unwrap();
        let index_before = git_text(&root, &["ls-files", "--stage"]).unwrap();
        std::fs::write(root.join("file"), "unstaged\n").unwrap();
        let replies = handle_blocking(
            &root,
            GitClientMessage::Restore {
                path: "file".into(),
            },
        );
        assert!(matches!(
            &replies[0],
            GitServerMessage::ChangedFiles { error: None, .. }
        ));
        assert_eq!(
            std::fs::read_to_string(root.join("file")).unwrap(),
            "staged\n"
        );
        assert_eq!(
            git_text(&root, &["ls-files", "--stage"]).unwrap(),
            index_before
        );
        assert_eq!(git_text(&root, &["show", ":file"]).unwrap(), "staged");
        assert_eq!(git_text(&root, &["show", "HEAD:file"]).unwrap(), "base");
        std::fs::remove_file(root.join("file")).unwrap();
        restore_file(&root, "file").unwrap();
        assert_eq!(
            std::fs::read_to_string(root.join("file")).unwrap(),
            "staged\n"
        );
        std::fs::write(root.join("untracked"), "keep").unwrap();
        assert!(restore_file(&root, "untracked")
            .unwrap_err()
            .contains("untracked"));
        assert_eq!(
            std::fs::read_to_string(root.join("untracked")).unwrap(),
            "keep"
        );
        for path in [
            "",
            ".",
            "..",
            "../file",
            "nested/../file",
            ".git/index",
            "*",
            ":(top)file",
        ] {
            assert!(restore_file(&root, path).is_err(), "accepted {path:?}");
        }
        assert!(restore_file(&root, root.join("file").to_str().unwrap()).is_err());
        assert!(matches!(
            required_permission(&GitClientMessage::Restore {
                path: "file".into()
            }),
            Permission::WriteFiles
        ));
    }

    #[cfg(unix)]
    #[test]
    fn restore_rejects_symlink_escape_and_directory_scope() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("repo");
        Repository::init(&root).unwrap();
        let outside = dir.path().join("outside");
        std::fs::write(&outside, "keep").unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        run_git(&root, &["add", "--", "link"]).unwrap();
        assert!(restore_file(&root, "link").is_err());
        assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
        std::fs::create_dir(root.join("folder")).unwrap();
        std::fs::write(root.join("folder/file"), "index").unwrap();
        run_git(&root, &["add", "--", "folder/file"]).unwrap();
        std::fs::write(root.join("folder/file"), "working").unwrap();
        assert!(restore_file(&root, "folder").is_err());
        assert_eq!(
            std::fs::read_to_string(root.join("folder/file")).unwrap(),
            "working"
        );
    }

    #[test]
    fn restore_uses_index_on_unborn_branch_and_literal_filename() {
        let dir = tempfile::tempdir().unwrap();
        Repository::init(dir.path()).unwrap();
        // A wildcard in the actual filename must never match the other file.
        for path in ["file*", "file-other"] {
            std::fs::write(dir.path().join(path), "index\n").unwrap();
            run_git(dir.path(), &["add", "--", path]).unwrap();
            std::fs::write(dir.path().join(path), "working\n").unwrap();
        }
        restore_file(dir.path(), "file*").unwrap();
        assert_eq!(
            std::fs::read_to_string(dir.path().join("file*")).unwrap(),
            "index\n"
        );
        assert_eq!(
            std::fs::read_to_string(dir.path().join("file-other")).unwrap(),
            "working\n"
        );
    }

    #[test]
    fn publish_sets_tracking_and_rejects_unsafe_destinations() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("work");
        let repo = Repository::init(&root).unwrap();
        repo.set_head("refs/heads/topic").unwrap();
        assert!(publish(&root).unwrap_err().contains("no commits"));
        let tree_id = repo.index().unwrap().write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig =
            git2::Signature::now("Publish test", "publish@example.invalid").unwrap();
        let head = repo
            .commit(Some("HEAD"), &sig, &sig, "initial", &tree, &[])
            .unwrap();
        assert!(publish(&root).unwrap_err().contains("no Git remote"));
        let a = dir.path().join("a.git");
        let b = dir.path().join("b.git");
        Repository::init_bare(&a).unwrap();
        Repository::init_bare(&b).unwrap();
        repo.remote("alpha", a.to_str().unwrap()).unwrap();
        repo.remote("beta", b.to_str().unwrap()).unwrap();
        assert!(publish(&root).unwrap_err().contains("multiple remotes"));
        repo.config()
            .unwrap()
            .set_str("remote.pushDefault", "beta")
            .unwrap();
        repo.config()
            .unwrap()
            .set_str("branch.topic.pushRemote", "alpha")
            .unwrap();
        let reply = handle_blocking(&root, GitClientMessage::Publish);
        assert!(
            matches!(&reply[0], GitServerMessage::ChangedFiles { upstream: Some(upstream), error: None, .. } if upstream == "alpha/topic")
        );
        assert_eq!(
            collect_repo_snapshot(&root).upstream.as_deref(),
            Some("alpha/topic")
        );
        assert_eq!(
            Repository::open_bare(&a)
                .unwrap()
                .refname_to_id("refs/heads/topic")
                .unwrap(),
            head
        );
        assert!(Repository::open_bare(&b)
            .unwrap()
            .find_reference("refs/heads/topic")
            .is_err());
        // Configured tracking remains meaningful even after a remote ref disappears.
        repo.find_reference("refs/remotes/alpha/topic")
            .unwrap()
            .delete()
            .unwrap();
        assert_eq!(resolve_upstream(&root).as_deref(), Some("alpha/topic"));
        repo.config()
            .unwrap()
            .set_str("branch.topic.pushRemote", "missing")
            .unwrap();
        assert!(publish(&root)
            .unwrap_err()
            .contains("not a configured remote"));
        repo.config()
            .unwrap()
            .remove("branch.topic.pushRemote")
            .unwrap();
        publish(&root).unwrap();
        assert_eq!(resolve_upstream(&root).as_deref(), Some("beta/topic"));
        repo.config().unwrap().remove("remote.pushDefault").unwrap();
        repo.remote_delete("beta").unwrap();
        publish(&root).unwrap();
        assert_eq!(resolve_upstream(&root).as_deref(), Some("alpha/topic"));
        repo.remote_rename("alpha", "origin").unwrap();
        repo.remote("beta", b.to_str().unwrap()).unwrap();
        publish(&root).unwrap();
        assert_eq!(resolve_upstream(&root).as_deref(), Some("origin/topic"));
        repo.set_head_detached(head).unwrap();
        assert!(publish(&root).unwrap_err().contains("detached HEAD"));
        assert!(GitClientMessage::Publish.is_mutation());
    }

    #[test]
    fn apply_block_dispatch_refreshes_changed_files_on_success_and_error() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let repo = Repository::init(root).unwrap();
        run_git(root, &["config", "core.autocrlf", "false"]).unwrap();
        std::fs::write(root.join("file"), "old\n").unwrap();
        run_git(root, &["add", "file"]).unwrap();
        let tree = repo
            .find_tree(repo.index().unwrap().write_tree().unwrap())
            .unwrap();
        let sig = git2::Signature::now("Test", "test@example.invalid").unwrap();
        repo.commit(Some("HEAD"), &sig, &sig, "base", &tree, &[])
            .unwrap();
        std::fs::write(root.join("file"), "new\n").unwrap();
        let block = neoism_protocol::git::GitChangeBlock {
            old_start: 0,
            new_start: 0,
            old_lines: vec!["old".into()],
            new_lines: vec!["new".into()],
        };
        for restore in [false, true] {
            let message = GitClientMessage::ApplyBlock {
                path: "file".into(),
                block: block.clone(),
                restore,
            };
            assert!(matches!(
                required_permission(&message),
                Permission::WriteFiles
            ));
            let result = handle_blocking(root, message);
            assert!(
                matches!(&result[0], GitServerMessage::ChangedFiles { error, .. } if error.is_some() == restore)
            );
        }
        assert_eq!(std::fs::read(root.join("file")).unwrap(), b"new\n");
        let scope = root.join("sub");
        std::fs::create_dir(&scope).unwrap();
        let reply = handle_blocking(
            &scope,
            GitClientMessage::ApplyBlock {
                path: "file".into(),
                block,
                restore: false,
            },
        );
        assert!(
            matches!(&reply[0], GitServerMessage::ChangedFiles { error: Some(error), .. } if error.contains("outside the declared workspace"))
        );
    }

    #[test]
    fn workspace_without_repo_returns_error() {
        // A path that almost certainly is not a git repo.
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-not-a-repo-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let out = handle_blocking(&tmp, GitClientMessage::Status);
        std::fs::remove_dir_all(&tmp).ok();
        assert!(
            matches!(out.first(), Some(GitServerMessage::Error { .. })),
            "expected Error, got {out:?}"
        );
    }

    #[test]
    fn git_changes_snapshot_non_repo_is_zero() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-changes-empty-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let counts = git_changes_snapshot(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(counts, (0, 0));
    }

    #[test]
    fn git_changes_snapshot_counts_untracked_and_modified() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-changes-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let repo = Repository::init(&tmp).expect("init repo");
        // Commit one file so we have an HEAD; then mutate the worktree
        // to produce a deletion + an untracked add.
        std::fs::write(tmp.join("kept.txt"), b"hello").expect("write kept");
        std::fs::write(tmp.join("doomed.txt"), b"bye").expect("write doomed");
        {
            let mut index = repo.index().expect("index");
            index.add_path(std::path::Path::new("kept.txt")).unwrap();
            index.add_path(std::path::Path::new("doomed.txt")).unwrap();
            index.write().unwrap();
            let oid = index.write_tree().unwrap();
            let tree = repo.find_tree(oid).unwrap();
            let sig = git2::Signature::now("t", "t@e").unwrap();
            repo.commit(Some("HEAD"), &sig, &sig, "init", &tree, &[])
                .unwrap();
        }
        std::fs::remove_file(tmp.join("doomed.txt")).unwrap();
        std::fs::write(tmp.join("brand_new.txt"), b"new").unwrap();
        let (added, deleted) = git_changes_snapshot(&tmp);
        std::fs::remove_dir_all(&tmp).ok();
        assert_eq!(deleted, 1, "doomed.txt should count as one deletion");
        assert_eq!(added, 1, "brand_new.txt should count as one addition");
    }

    #[test]
    fn stage_commit_branch_roundtrip_via_wire_verbs() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-panel-verbs-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        let _repo = Repository::init(&tmp).expect("init repo");
        run_git(&tmp, &["config", "user.name", "t"]).expect("config name");
        run_git(&tmp, &["config", "user.email", "t@e"]).expect("config email");
        std::fs::write(tmp.join("a.txt"), b"one\n").expect("write a.txt");

        // Untracked + unstaged before any op.
        let out = handle_blocking(&tmp, GitClientMessage::ChangedFiles);
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "unexpected error: {error:?}");
                assert_eq!(files.len(), 1, "expected one entry, got {files:?}");
                assert_eq!(files[0].path, "a.txt");
                assert_eq!(files[0].status, GitChangeStatus::Untracked);
                assert!(!files[0].staged);
                assert_eq!(files[0].additions, 1, "untracked line count");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Stage flips the staged bit (index column non-empty).
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Stage {
                path: "a.txt".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "stage error: {error:?}");
                assert_eq!(files.len(), 1);
                assert_eq!(files[0].status, GitChangeStatus::Added);
                assert!(files[0].staged, "staged bit after git add");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Unstage puts it back.
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Unstage {
                path: "a.txt".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { files, error, .. }) => {
                assert!(error.is_none(), "unstage error: {error:?}");
                assert!(!files[0].staged, "unstaged after restore --staged");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Re-stage + commit empties the list and reports a branch.
        handle_blocking(
            &tmp,
            GitClientMessage::Stage {
                path: "a.txt".into(),
            },
        );
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Commit {
                message: "init".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles {
                files,
                branch,
                error,
                ..
            }) => {
                assert!(error.is_none(), "commit error: {error:?}");
                assert!(files.is_empty(), "clean tree after commit: {files:?}");
                assert!(branch.is_some(), "branch after first commit");
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Branch list + checkout of a fresh branch.
        run_git(&tmp, &["branch", "dev"]).expect("create dev branch");
        let out = handle_blocking(&tmp, GitClientMessage::Branches);
        match out.first() {
            Some(GitServerMessage::Branches { branches }) => {
                assert!(
                    branches.iter().any(|b| b == "dev"),
                    "dev missing from {branches:?}"
                );
            }
            other => panic!("expected Branches, got {other:?}"),
        }
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Checkout {
                branch: "dev".into(),
            },
        );
        match out.first() {
            Some(GitServerMessage::ChangedFiles { branch, error, .. }) => {
                assert!(error.is_none(), "checkout error: {error:?}");
                assert_eq!(branch.as_deref(), Some("dev"));
            }
            other => panic!("expected ChangedFiles, got {other:?}"),
        }

        // Per-file diff carries HEAD-relative patch text.
        std::fs::write(tmp.join("a.txt"), b"one\ntwo\n").expect("modify a.txt");
        let out = handle_blocking(
            &tmp,
            GitClientMessage::DiffFiles {
                paths: vec!["a.txt".into()],
            },
        );
        match out.first() {
            Some(GitServerMessage::FileDiffs { diffs }) => {
                assert_eq!(diffs.len(), 1);
                assert_eq!(diffs[0].path, "a.txt");
                assert!(
                    diffs[0].patch.contains("+two"),
                    "patch missing addition: {}",
                    diffs[0].patch
                );
            }
            other => panic!("expected FileDiffs, got {other:?}"),
        }

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn directory_pathspec_stages_and_unstages_as_one_operation() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-folder-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("nested")).unwrap();
        let _repo = Repository::init(&tmp).unwrap();
        std::fs::write(tmp.join("nested/a.txt"), "a\n").unwrap();
        std::fs::write(tmp.join("nested/b.txt"), "b\n").unwrap();
        std::fs::write(tmp.join("other.txt"), "other\n").unwrap();
        for (request, staged) in [
            (
                GitClientMessage::Stage {
                    path: "nested".into(),
                },
                true,
            ),
            (
                GitClientMessage::Unstage {
                    path: "nested".into(),
                },
                false,
            ),
        ] {
            let result = handle_blocking(&tmp, request);
            let Some(GitServerMessage::ChangedFiles { files, error, .. }) =
                result.first()
            else {
                panic!("expected ChangedFiles: {result:?}");
            };
            assert!(error.is_none(), "{error:?}");
            assert_eq!(
                files.iter().filter(|f| f.staged).count(),
                if staged { 2 } else { 0 }
            );
            assert!(!files.iter().find(|f| f.path == "other.txt").unwrap().staged);
        }
        for path in [".", "", "../outside"] {
            let result =
                handle_blocking(&tmp, GitClientMessage::Stage { path: path.into() });
            assert!(matches!(
                result.first(),
                Some(GitServerMessage::ChangedFiles { error: Some(_), .. })
            ));
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn network_verbs_report_errors_with_refreshed_files() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-network-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        Repository::init(&tmp).unwrap();
        std::fs::write(tmp.join("pending.txt"), "pending\n").unwrap();
        for request in [
            GitClientMessage::Fetch,
            GitClientMessage::Pull,
            GitClientMessage::Push,
            GitClientMessage::Publish,
        ] {
            let must_fail = !matches!(request, GitClientMessage::Fetch);
            let result = handle_blocking(&tmp, request);
            let Some(GitServerMessage::ChangedFiles { files, error, .. }) =
                result.first()
            else {
                panic!("expected ChangedFiles: {result:?}");
            };
            // Fetch without a configured remote may succeed; pull and push cannot.
            assert_eq!(files[0].path, "pending.txt");
            if must_fail {
                assert!(error.as_ref().is_some_and(|e| !e.is_empty()));
            }
        }
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn late_old_subscription_cannot_override_new_workspace_watch() {
        let mut high_water = 0;
        assert!(super::accept_watch_request(&mut high_water, 101));
        assert!(super::accept_watch_request(&mut high_water, 102));
        assert!(!super::accept_watch_request(&mut high_water, 101));
        assert!(super::accept_watch_request(&mut high_water, 102));
        assert_eq!(high_water, 102);
    }

    #[test]
    fn mutation_verbs_require_write_permission() {
        assert!(matches!(
            required_permission(&GitClientMessage::Stage { path: "x".into() }),
            Permission::WriteFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Commit {
                message: "m".into()
            }),
            Permission::WriteFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Checkout { branch: "b".into() }),
            Permission::WriteFiles
        ));
        for request in [
            GitClientMessage::Fetch,
            GitClientMessage::Pull,
            GitClientMessage::Push,
            GitClientMessage::Publish,
        ] {
            assert!(matches!(
                required_permission(&request),
                Permission::WriteFiles
            ));
        }
        assert!(matches!(
            required_permission(&GitClientMessage::ChangedFiles),
            Permission::ReadFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::Branches),
            Permission::ReadFiles
        ));
        assert!(matches!(
            required_permission(&GitClientMessage::DiffFiles { paths: vec![] }),
            Permission::ReadFiles
        ));
    }

    #[test]
    fn diff_rejects_traversal_path() {
        let tmp = std::env::temp_dir()
            .join(format!("neoism-git-traversal-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).expect("mkdir tmp");
        // Initialise a real repo so the early "not a repo" branch doesn't fire.
        let _repo = Repository::init(&tmp).expect("init repo");
        let out = handle_blocking(
            &tmp,
            GitClientMessage::Diff {
                path: Some("../etc/passwd".into()),
            },
        );
        std::fs::remove_dir_all(&tmp).ok();
        match out.first() {
            Some(GitServerMessage::Error { message }) => {
                assert!(message.contains(".."), "unexpected error: {message}");
            }
            other => panic!("expected traversal error, got {other:?}"),
        }
    }
}

/// Combined snapshot: discovery, branch and file list all refer to the host's
/// repository workdir, never to the requesting machine's filesystem.
pub(crate) fn collect_repo_snapshot(root: &Path) -> neoism_protocol::git::GitRepoStatus {
    use neoism_protocol::git::GitRepoStatus;
    let mut snapshot = GitRepoStatus {
        workspace_root: root.to_string_lossy().into_owned(),
        repo_root: None,
        branch: None,
        upstream: None,
        files: Vec::new(),
        error: None,
    };
    let repo = match Repository::discover(root) {
        Ok(repo) => repo,
        Err(_) => return snapshot,
    };
    let Some(workdir) = repo.workdir() else {
        return snapshot;
    };
    // Match daemon-produced directory identities (Windows canonical roots may
    // carry a verbatim prefix). Canonicalization only happens on the HOST.
    let workdir =
        std::fs::canonicalize(workdir).unwrap_or_else(|_| workdir.to_path_buf());
    snapshot.repo_root = Some(workdir.to_string_lossy().into_owned());
    snapshot.branch = resolve_branch(&workdir);
    snapshot.upstream = resolve_upstream(&workdir);
    match collect_changed_files_checked(&workdir) {
        Ok(files) => snapshot.files = files,
        Err(error) => snapshot.error = Some(error),
    }
    snapshot
}

#[cfg(test)]
#[path = "git_commit_read_tests.rs"]
mod commit_read_tests;
