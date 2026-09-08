//! Shared host-only snapshot cache. One collector per root, at most two Git
//! collectors process-wide. Socket subscribers reuse the existing 2s status
//! budget; simultaneous panels/guests never spawn duplicate status processes.
use neoism_protocol::git::GitRepoStatus;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, OnceLock},
    time::{Duration, Instant},
};
use tokio::sync::{Mutex, Semaphore};

type Entry = Arc<Mutex<Option<(Instant, GitRepoStatus)>>>;
static CACHE: OnceLock<Mutex<HashMap<PathBuf, Entry>>> = OnceLock::new();
static BUDGET: Semaphore = Semaphore::const_new(2);

pub async fn snapshot(root: PathBuf) -> GitRepoStatus {
    let entry = {
        let mut cache = CACHE.get_or_init(Mutex::default).lock().await;
        if cache.len() >= 64 && !cache.contains_key(&root) {
            // Don't evict live collectors (which would defeat single-flight).
            cache.retain(|_, entry| Arc::strong_count(entry) > 1);
        }
        cache.entry(root.clone()).or_default().clone()
    };
    let mut cached = entry.lock().await;
    if let Some((at, value)) = cached.as_ref() {
        if at.elapsed() < Duration::from_secs(1) {
            return value.clone();
        }
    }
    let permit = BUDGET.acquire().await.expect("Git budget never closes");
    let workspace_root = root.to_string_lossy().into_owned();
    let value =
        tokio::task::spawn_blocking(move || super::git::collect_repo_snapshot(&root))
            .await
            .unwrap_or_else(|e| GitRepoStatus {
                workspace_root,
                repo_root: None,
                branch: None,
                files: vec![],
                error: Some(e.to_string()),
            });
    drop(permit);
    *cached = Some((Instant::now(), value.clone()));
    value
}

pub async fn invalidate(root: &std::path::Path) {
    let entry = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .await
        .get(root)
        .cloned();
    if let Some(entry) = entry {
        *entry.lock().await = None;
    }
}

/// Avoid unbounded git processes/output on pathological repositories. Pipe
/// readers drain concurrently so the child cannot deadlock on a full pipe.
/// On timeout kill AND reap; reader memory is capped while excess is drained.
pub(super) fn output(
    command: &mut std::process::Command,
) -> std::io::Result<std::process::Output> {
    use std::{
        io::{Error, ErrorKind, Read},
        process::Stdio,
    };
    fn drain(mut input: impl Read) -> std::io::Result<Vec<u8>> {
        let mut kept = Vec::new();
        let mut overflow = false;
        let mut buf = [0u8; 8192];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                return if overflow {
                    Err(Error::other("git snapshot exceeded 16 MiB output limit"))
                } else {
                    Ok(kept)
                };
            }
            let retain = n.min((16 * 1024 * 1024usize).saturating_sub(kept.len()));
            overflow |= retain < n;
            kept.extend_from_slice(&buf[..retain]);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let out = std::thread::spawn(move || drain(stdout));
    let err = std::thread::spawn(move || drain(stderr));
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10))
            }
            result => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                #[cfg(windows)]
                {
                    // Embedded Windows hosts use a console-safe supervisor;
                    // killing only its PID leaves the actual git child alive.
                    let mut kill = std::process::Command::new("taskkill");
                    crate::windows_process::hide_std_command(&mut kill);
                    if let Ok(mut killer) = kill
                        .args(["/PID", &child.id().to_string(), "/T", "/F"])
                        .stdin(Stdio::null())
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                    {
                        let until = Instant::now() + Duration::from_secs(2);
                        while matches!(killer.try_wait(), Ok(None))
                            && Instant::now() < until
                        {
                            std::thread::sleep(Duration::from_millis(10));
                        }
                        let _ = killer.kill();
                        let _ = killer.wait();
                    }
                }
                let _ = child.kill();
                let _ = child.wait();
                break Err(result.err().unwrap_or_else(|| {
                    Error::new(ErrorKind::TimedOut, "git snapshot timed out")
                }));
            }
        }
    };
    let stdout = out
        .join()
        .unwrap_or_else(|_| Err(Error::other("git stdout reader")))?;
    let stderr = err
        .join()
        .unwrap_or_else(|_| Err(Error::other("git stderr reader")))?;
    Ok(std::process::Output {
        status: status?,
        stdout,
        stderr,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use git2::{IndexAddOption, Repository, Signature};
    use neoism_protocol::git::{GitChangeStatus, GitClientMessage, GitServerMessage};

    fn commit(repo: &Repository) {
        let mut index = repo.index().unwrap();
        index.add_all(["*"], IndexAddOption::DEFAULT, None).unwrap();
        index.update_all(["*"], None).unwrap();
        index.write().unwrap();
        let tree = repo.find_tree(index.write_tree().unwrap()).unwrap();
        let sig = Signature::now("Git fixture", "fixture@example.invalid").unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        repo.commit(
            Some("HEAD"),
            &sig,
            &sig,
            "fixture",
            &tree,
            &parent.iter().collect::<Vec<_>>(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn host_snapshot_updates_after_edit_rename_delete_commit_and_branch_switch() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = Repository::init(&root).unwrap();
        std::fs::write(root.join("tracked"), "before\n").unwrap();
        std::fs::write(root.join("rename"), "rename me\n").unwrap();
        std::fs::write(root.join("deleted"), "delete me\n").unwrap();
        commit(&repo);
        let clean = snapshot(root.clone()).await;
        assert!(clean.files.is_empty());
        assert!(clean.branch.is_some());
        std::fs::write(root.join("tracked"), "after\nanother\n").unwrap();
        std::fs::write(root.join("untracked"), "new\n").unwrap();
        std::fs::rename(root.join("rename"), root.join("renamed")).unwrap();
        std::fs::remove_file(root.join("deleted")).unwrap();
        let mut index = repo.index().unwrap();
        index.remove_path(std::path::Path::new("rename")).unwrap();
        index.add_path(std::path::Path::new("renamed")).unwrap();
        index.write().unwrap();
        // External host edits do not call invalidate; the existing poll budget
        // picks them up once the shared cache expires.
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let edited = snapshot(root.clone()).await;
        assert_eq!(edited.workspace_root, root.to_string_lossy());
        assert!(edited
            .files
            .iter()
            .any(|f| f.path == "tracked" && f.additions == 2 && f.deletions == 1));
        assert!(edited
            .files
            .iter()
            .any(|f| f.path == "untracked" && f.status == GitChangeStatus::Untracked));
        assert!(edited
            .files
            .iter()
            .any(|f| f.path == "renamed" && f.status == GitChangeStatus::Renamed));
        assert!(edited
            .files
            .iter()
            .any(|f| f.path == "deleted" && f.status == GitChangeStatus::Deleted));
        let panel =
            crate::git::handle_with_root(root.clone(), GitClientMessage::ChangedFiles)
                .await;
        assert!(
            matches!(&panel[0], GitServerMessage::ChangedFiles { files, .. } if files == &edited.files)
        );
        commit(&repo);
        let head = repo.head().unwrap().peel_to_commit().unwrap();
        repo.branch("host-next", &head, false).unwrap();
        repo.set_head("refs/heads/host-next").unwrap();
        tokio::time::sleep(Duration::from_millis(1100)).await;
        let committed = snapshot(root.clone()).await;
        assert_eq!(committed.branch.as_deref(), Some("host-next"));
        assert!(committed.files.is_empty());
    }

    #[tokio::test]
    async fn repo_subdirectory_identity_and_non_repo_do_not_contaminate_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = std::fs::canonicalize(dir.path()).unwrap();
        let repo = Repository::init(&root).unwrap();
        std::fs::create_dir(root.join("sub")).unwrap();
        std::fs::write(root.join("sub/file"), "one\n").unwrap();
        commit(&repo);
        std::fs::write(root.join("sub/file"), "changed\n").unwrap();
        let sub = root.join("sub");
        let value = snapshot(sub.clone()).await;
        assert_eq!(value.workspace_root, sub.to_string_lossy());
        assert_eq!(value.repo_root.as_deref(), root.to_str());
        assert_eq!(value.files[0].path, "sub/file");
        let other = tempfile::tempdir().unwrap();
        let blank = snapshot(other.path().to_owned()).await;
        assert!(
            blank.repo_root.is_none() && blank.files.is_empty() && blank.branch.is_none()
        );
        assert_eq!(snapshot(sub).await, value);
    }
    #[cfg(unix)]
    #[test]
    fn snapshot_process_deadline_kills_and_reaps_child() {
        let start = Instant::now();
        let error = output(std::process::Command::new("sh").args(["-c", "sleep 30"]))
            .expect_err("sleep exceeded snapshot deadline");
        assert_eq!(error.kind(), std::io::ErrorKind::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(8));
    }
}
