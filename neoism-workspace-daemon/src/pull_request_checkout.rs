//! Explicit PR checkout. Fetch is outside the authoritative document gate;
//! only the short local switch and document reconciliation run under it.
use super::*;
use git2::{Repository, RepositoryState, StatusOptions};
use std::fs;

static CHECKOUT_GATE: parking_lot::Mutex<()> = parking_lot::Mutex::new(());
#[path = "pull_request_worktree_ignore.rs"]
mod ignore;

struct Prepared {
    root: PathBuf,
    workdir: PathBuf,
    head: String,
    number: u64,
    reference: String,
    branch: String,
    association: identity::Association,
}
impl Drop for Prepared {
    fn drop(&mut self) {
        // Never delete a ref another actor changed. This ref is a private,
        // per-attempt pin, not a branch or a user's FETCH_HEAD.
        if let Ok(repo) = Repository::discover(&self.root) {
            if let Ok(mut reference) = repo.find_reference(&self.reference) {
                if reference
                    .target()
                    .is_some_and(|oid| oid.to_string() == self.head)
                {
                    let _ = reference.delete();
                }
            }
        }
    }
}
fn local_error(message: &str) -> (PullRequestErrorKind, String) {
    error(PullRequestErrorKind::Conflict, message)
}
fn canonical_root(root: &Path) -> Result<PathBuf> {
    let root = crate::path::canonicalize(root)
        .map_err(|_| invalid("Declared workspace root is unavailable"))?;
    if !root.is_dir() {
        return Err(invalid("Declared workspace root must be a directory"));
    }
    Ok(root)
}
fn repository(root: &Path) -> Result<(Repository, PathBuf)> {
    let repo = Repository::discover(root)
        .map_err(|_| invalid("Declared root is not in a Git worktree"))?;
    let workdir = repo
        .workdir()
        .ok_or_else(|| invalid("Bare repositories cannot be checked out"))?;
    let workdir = crate::path::canonicalize(workdir)
        .map_err(|_| invalid("Cannot establish repository scope"))?;
    if !root.starts_with(&workdir) {
        return Err(invalid("Repository is outside declared workspace scope"));
    }
    Ok((repo, workdir))
}
pub(super) fn authorized_root(root: &Path) -> Result<PathBuf> {
    let root = canonical_root(root)?;
    let (_, workdir) = repository(&root)?;
    if root != workdir {
        return Err(error(PullRequestErrorKind::Unsupported, "Hosted PR operations require a repository-root workspace; open the repository root explicitly. A nested workspace cannot authorize repository-wide reads or mutations"));
    }
    Ok(root)
}
fn hooks_disabled() -> String {
    // No checkout hooks, reference hooks, or submodule recursion. The path is
    // generated host-side and intentionally nonexistent, on every platform.
    format!(
        "core.hooksPath={}",
        std::env::temp_dir()
            .join(format!("neoism-no-hooks-{}", uuid::Uuid::new_v4()))
            .display()
    )
}
fn prepare(root: &Path, number: u64, expected: &str) -> Result<Prepared> {
    if number == 0 || !sha(expected) {
        return Err(invalid("A PR number and full expected head are required"));
    }
    let root = authorized_root(root)?;
    let (_, workdir) = repository(&root)?;
    let host = Host::new(&root)?;
    let initial = host.expected(number, expected)?;
    let nonce = uuid::Uuid::new_v4().simple().to_string();
    let reference = format!("refs/neoism/pr-checkout/{number}/{nonce}");
    let branch = format!("neoism/pr-{number}-{}-{nonce}", &expected[..12]);
    let association = identity::Association::from_pr(
        &host.repo,
        &initial,
        format!("neoism-pr-{number}-{nonce}"),
    )?;
    if association.number != number {
        return Err(invalid("PR identity did not match requested number"));
    }
    let prepared = Prepared {
        root: root.clone(),
        workdir,
        head: expected.into(),
        number,
        reference,
        branch,
        association,
    };
    let url = format!(
        "https://github.com/{}/{}.git",
        host.repo.owner, host.repo.repo
    );
    let refspec = format!("refs/pull/{number}/head:{}", prepared.reference);
    git(
        &prepared.root,
        &[
            "-c",
            &hooks_disabled(),
            "-c",
            "credential.interactive=false",
            "fetch",
            "--no-tags",
            "--no-recurse-submodules",
            "--no-write-fetch-head",
            "--",
            &url,
            &refspec,
        ],
    )?;
    verify_fetched(&prepared)?;
    let current = host.expected(number, expected)?;
    prepared.association.verify_pr(&host.repo, &current)?;
    Ok(prepared)
}

fn verify_fetched(prepared: &Prepared) -> Result<()> {
    let fetched = git(
        &prepared.root,
        &["rev-parse", "--verify", &prepared.reference],
    )?;
    if fetched != prepared.head {
        return Err(error(
            PullRequestErrorKind::Stale,
            "Fetched PR ref differs from expected head; no checkout was performed",
        ));
    }
    let repo = Repository::discover(&prepared.root)
        .map_err(|_| invalid("Repository disappeared"))?;
    repo.find_commit(
        git2::Oid::from_str(&prepared.head)
            .map_err(|_| invalid("Invalid expected head"))?,
    )
    .map_err(|_| invalid("Fetched head is not a commit"))?;
    Ok(())
}

fn plain_managed_directory(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|_| local_error("Cannot inspect managed worktree directory"))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(local_error(
            "Managed worktree directory must be a real directory, not a symlink",
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } || metadata.mode() & 0o022 != 0 {
            return Err(local_error("Managed worktree directory must be owned by the host user and not writable by other users"));
        }
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(local_error(
                "Reparse points are forbidden for managed worktrees",
            ));
        }
    }
    Ok(())
}
fn destination(prepared: &Prepared) -> Result<PathBuf> {
    // Never accept a path from the client, never choose an unregistered sibling,
    // and never broaden a nested workspace declaration to the enclosing repo.
    let container = ignore::managed_container(prepared)?;
    let name = prepared.branch.replace('/', "-");
    let path = container.join(name);
    if fs::symlink_metadata(&path).is_ok() {
        return Err(local_error(
            "Managed worktree destination already exists; nothing was overwritten",
        ));
    }
    Ok(path)
}
fn add_worktree(prepared: &Prepared) -> Result<PullRequestReply> {
    let path = destination(prepared)?;
    let path_text = path
        .to_str()
        .ok_or_else(|| invalid("Worktree path is not UTF-8"))?;
    git(&prepared.root, &["-c", &hooks_disabled(), "-c", "submodule.recurse=false", "worktree", "add", "--no-track", "-b", &prepared.branch, "--", path_text, &prepared.head])
        .map_err(|(kind, _)| error(kind,"Worktree creation did not complete; no existing destination was overwritten. Inspect the managed worktree directory before retrying"))?;
    if crate::path::canonicalize(&path).ok().as_ref() != Some(&path) {
        return Err(local_error("Created worktree failed scope verification"));
    }
    let repo = Repository::open(&path)
        .map_err(|_| local_error("Created worktree cannot be opened"))?;
    if repo
        .head()
        .ok()
        .and_then(|r| r.target())
        .map(|o| o.to_string())
        .as_deref()
        != Some(prepared.head.as_str())
        || repo
            .head()
            .ok()
            .and_then(|r| r.shorthand().map(str::to_owned))
            .as_deref()
            != Some(prepared.branch.as_str())
    {
        return Err(local_error(
            "Created worktree head verification failed; inspect it before retrying",
        ));
    }
    identity::configure(
        &repo,
        &prepared.branch,
        &prepared.head,
        &prepared.association,
    )?;
    Ok(PullRequestReply::CheckedOut {
        number: prepared.number,
        path: path_text.into(),
        worktree: true,
    })
}
fn switch(prepared: &Prepared, paths: &[PathBuf]) -> std::result::Result<(), String> {
    let (repo, workdir) = repository(&prepared.root).map_err(|(_, e)| e)?;
    if workdir != prepared.workdir || workdir != prepared.root {
        return Err("Repository scope changed before checkout".into());
    }
    if repo.state() != RepositoryState::Clean {
        return Err("Finish the in-progress Git operation before PR checkout".into());
    }
    let mut options = StatusOptions::new();
    options
        .include_untracked(true)
        .recurse_untracked_dirs(false)
        .include_ignored(false);
    if !repo
        .statuses(Some(&mut options))
        .map_err(|e| e.to_string())?
        .is_empty()
    {
        return Err("PR checkout refused: index/worktree is dirty (including untracked files); commit or move changes explicitly".into());
    }
    let target = repo
        .find_commit(git2::Oid::from_str(&prepared.head).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?
        .tree()
        .map_err(|e| e.to_string())?;
    // Prove reconciliation is representable before touching the tree. A target
    // symlink, submodule, or binary replacing an open text buffer is refused.
    for path in paths {
        let relative = path
            .strip_prefix(&workdir)
            .map_err(|_| "Open document escaped repository scope")?;
        let mut prefix = PathBuf::new();
        for part in relative.components() {
            prefix.push(part);
            match target.get_path(&prefix) {
                Ok(entry) if entry.filemode() == 0o120000 || entry.filemode() == 0o160000 => return Err("Target PR replaces an open document or its parent with a symlink/submodule".into()),
                _ => {},
            }
        }
        if let Ok(entry) = target.get_path(relative) {
            let blob = entry.to_object(&repo).and_then(|o|o.peel_to_blob()).map_err(|_|"Target PR replaces an open document with a directory or non-text entry")?;
            if blob.size() > 16 * 1024 * 1024 {
                return Err("Target PR content for an open document exceeds the reconciliation limit".into());
            }
            std::str::from_utf8(blob.content()).map_err(|_| {
                "Target PR contains non-UTF-8 content for an open document"
            })?;
        }
    }
    // Local libgit2 checkout only: no hooks, shell filters, lazy fetches, or
    // network work while the hub gate is held. Cap the changed tree workload.
    let current = repo
        .head()
        .and_then(|r| r.peel_to_tree())
        .map_err(|e| e.to_string())?;
    let diff = repo
        .diff_tree_to_tree(Some(&current), Some(&target), None)
        .map_err(|e| e.to_string())?;
    if diff.deltas().len() > 2000 {
        return Err(
            "PR checkout exceeds the guarded local file-count limit; use a new worktree"
                .into(),
        );
    }
    let mut bytes = 0usize;
    for delta in diff.deltas() {
        let file = delta.new_file();
        if file.id().is_zero() || file.mode() == git2::FileMode::Commit {
            continue;
        }
        let blob = repo.find_blob(file.id()).map_err(|_| {
            "PR checkout requires locally available blobs; fetch all objects first"
        })?;
        bytes = bytes.saturating_add(blob.size());
        if bytes > 64 * 1024 * 1024 {
            return Err(
                "PR checkout exceeds the guarded local byte limit; use a new worktree"
                    .into(),
            );
        }
    }
    let commit = repo
        .find_commit(git2::Oid::from_str(&prepared.head).map_err(|e| e.to_string())?)
        .map_err(|e| e.to_string())?;
    let branch = repo
        .branch(&prepared.branch, &commit, false)
        .map_err(|_| "Dedicated PR branch already exists or cannot be created")?;
    let branch_ref = branch
        .get()
        .name()
        .ok_or("Invalid dedicated branch identity")?
        .to_string();
    // Establish the complete fail-closed PR identity before changing any files
    // or HEAD. A config failure leaves only a fresh, unselected local branch.
    identity::configure(
        &repo,
        &prepared.branch,
        &prepared.head,
        &prepared.association,
    )
    .map_err(|(_, message)| message)?;
    let mut options = git2::build::CheckoutBuilder::new();
    options
        .safe()
        .disable_filters(true)
        .remove_untracked(false)
        .overwrite_ignored(false);
    repo.checkout_tree(commit.as_object(), Some(&mut options))
        .map_err(|_| "Local PR checkout failed; inspect Git status before retrying")?;
    repo.set_head(&branch_ref).map_err(|_| {
        "PR tree changed but HEAD update failed; inspect Git status before retrying"
    })?;
    // Configuration was verified before the local tree mutation.
    if repo
        .head()
        .ok()
        .and_then(|r| r.target())
        .map(|o| o.to_string())
        .as_deref()
        != Some(prepared.head.as_str())
    {
        return Err("Checked-out head did not match expected PR head".into());
    }
    Ok(())
}

pub(super) fn checkout(
    root: &Path,
    number: u64,
    expected: &str,
    new_worktree: bool,
    hub: Option<&crate::crdt::sync::CrdtSyncHub>,
) -> Result<PullRequestReply> {
    if !new_worktree && hub.is_none() {
        return Err(error(PullRequestErrorKind::Unsupported,"Current-workspace checkout requires the attached daemon's authoritative document hub; use daemon RPC or create a managed worktree"));
    }
    let _operation = CHECKOUT_GATE.lock();
    let prepared = prepare(root, number, expected)?;
    if new_worktree {
        return add_worktree(&prepared);
    }
    let hub = hub.unwrap();
    hub.guarded_repository_checkout(&prepared.root, |paths| switch(&prepared, paths))
        .map_err(|message| error(PullRequestErrorKind::Conflict, &message))?;
    Ok(PullRequestReply::CheckedOut {
        number,
        path: prepared.root.to_string_lossy().into_owned(),
        worktree: false,
    })
}

#[cfg(test)]
#[path = "pull_request_checkout_tests.rs"]
mod tests;
#[cfg(test)]
#[path = "pull_request_workflow_tests.rs"]
mod workflow_tests;
