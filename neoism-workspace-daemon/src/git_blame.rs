//! Read-only, bounded HEAD blame and GitHub avatar service. No Git or HTTP in paint.
use base64::{engine::general_purpose::STANDARD, Engine};
use neoism_protocol::git::{GitBlameCommit, GitBlameSnapshot, GitServerMessage};
use std::{
    collections::{HashMap, VecDeque},
    path::PathBuf,
    sync::OnceLock,
    time::Duration,
};
use tokio::sync::{Mutex, Semaphore};

const MAX_BYTES: usize = 512 * 1024;
type CacheKey = (PathBuf, String, PathBuf);
static CACHE: OnceLock<
    Mutex<VecDeque<(CacheKey, std::time::Instant, GitBlameSnapshot)>>,
> = OnceLock::new();
static GATE: Semaphore = Semaphore::const_new(2);
// Held INSIDE blocking Git work, so dropping an async request cannot free a
// slot while its uncancellable libgit2 operation is still running.
static GIT_GATE: Semaphore = Semaphore::const_new(2);

pub async fn handle(root: PathBuf, path: String) -> Vec<GitServerMessage> {
    match load(root, path).await {
        Ok(snapshot) => vec![GitServerMessage::Blame { snapshot }],
        Err(message) => vec![GitServerMessage::Error { message }],
    }
}

async fn load(root: PathBuf, path: String) -> Result<GitBlameSnapshot, String> {
    if path.len() > 4096 {
        return Err("Blame path exceeds limit".into());
    }
    let _permit = GATE
        .try_acquire()
        .map_err(|_| "Blame busy; retry shortly")?;
    let key = tokio::task::spawn_blocking(move || {
        // Canonical root plus lexical child: HEAD trees never dereference worktree symlinks.
        let original_root = root;
        let file = crate::files::resolve_path(&original_root, &path)?;
        let relative_to_workspace = file
            .strip_prefix(&original_root)
            .map_err(|e| e.to_string())?;
        let root = original_root.canonicalize().map_err(|e| e.to_string())?;
        let file = root.join(relative_to_workspace);
        // Discover from the file's directory, not just the workspace root:
        // nested repositories/submodules have their own HEAD and authors.
        // Resolve only existing parent directories, with workspace confinement;
        // the file itself is read exclusively from the Git tree below.
        let mut probe = file.parent().ok_or("Blame needs a file path")?;
        let parent = loop {
            match probe.canonicalize() {
                Ok(parent) => break parent,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    probe = probe.parent().ok_or("Missing file parent")?;
                }
                Err(error) => return Err(error.to_string()),
            }
        };
        if !parent.starts_with(&root) {
            return Err("File parent escapes workspace".into());
        }
        let file = parent.join(file.strip_prefix(probe).map_err(|e| e.to_string())?);
        let repo = git2::Repository::discover(&parent).map_err(|e| e.to_string())?;
        let wd = repo
            .workdir()
            .ok_or("Bare repository")?
            .canonicalize()
            .map_err(|e| e.to_string())?;
        let relative = file
            .strip_prefix(&wd)
            .map_err(|e| e.to_string())?
            .to_path_buf();
        let head = match repo.head().and_then(|h| h.peel_to_commit()) {
            Ok(commit) => commit.id().to_string(),
            Err(error)
                if matches!(
                    error.code(),
                    git2::ErrorCode::UnbornBranch | git2::ErrorCode::NotFound
                ) =>
            {
                String::new()
            }
            Err(error) => return Err(error.to_string()),
        };
        Ok::<_, String>((wd, head, relative))
    })
    .await
    .map_err(|e| e.to_string())??;
    let cache = CACHE.get_or_init(Default::default);
    {
        let mut cache = cache.lock().await;
        cache.retain(|(_, since, _)| since.elapsed() < Duration::from_secs(600));
        if let Some(i) = cache.iter().position(|(k, _, _)| k == &key) {
            let entry = cache.remove(i).unwrap();
            let result = entry.2.clone();
            cache.push_back(entry);
            return Ok(result);
        }
    }
    let work_key = key.clone();
    let (mut snapshot, github) = tokio::task::spawn_blocking(move || blame(work_key))
        .await
        .map_err(|e| e.to_string())??;
    // At most 32 distinct identities per snapshot; cached negative lookups too.
    // Four concurrent HTTP workers, with a hard per-request timeout/body limit.
    let mut identities = HashMap::new();
    for commit in &snapshot.commits {
        let identity = if commit.email.is_empty() || commit.email.contains("[bot]") {
            commit.sha.clone()
        } else {
            commit.email.clone()
        };
        if identities.len() < 32 {
            identities.entry(identity).or_insert_with(|| commit.clone());
        }
    }
    let mut jobs = tokio::task::JoinSet::new();
    for (identity, commit) in identities {
        let github = github.clone();
        jobs.spawn(async move { (identity, avatar(&commit, github.as_deref()).await) });
    }
    let mut images = HashMap::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(6);
    loop {
        match tokio::time::timeout_at(deadline, jobs.join_next()).await {
            Ok(Some(Ok((id, Some(image))))) => {
                let index = snapshot.avatars.len() as u32;
                snapshot.avatars.push(image);
                images.insert(id, index);
            }
            Ok(Some(_)) => continue,
            _ => break,
        }
    }
    jobs.abort_all();
    for commit in &mut snapshot.commits {
        let id = if commit.email.is_empty() || commit.email.contains("[bot]") {
            &commit.sha
        } else {
            &commit.email
        };
        commit.avatar = images.get(id).copied();
    }
    let mut cache = cache.lock().await;
    if cache.len() >= 16 {
        cache.pop_front();
    }
    cache.push_back((key, std::time::Instant::now(), snapshot.clone()));
    Ok(snapshot)
}

fn blame(
    (wd, head, path): CacheKey,
) -> Result<(GitBlameSnapshot, Option<String>), String> {
    let _permit = GIT_GATE
        .try_acquire()
        .map_err(|_| "Blame busy; retry shortly")?;
    let repo = git2::Repository::open(&wd).map_err(|e| e.to_string())?;
    let mut out = GitBlameSnapshot {
        repo_root: wd.to_string_lossy().into(),
        path: path.to_string_lossy().into(),
        head: head.clone(),
        baseline: vec![],
        lines: vec![],
        commits: vec![],
        avatars: vec![],
    };
    if head.is_empty() {
        return Ok((out, None));
    }
    let oid = git2::Oid::from_str(&head).map_err(|e| e.to_string())?;
    let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    let entry = match tree.get_path(&path) {
        Ok(entry) => entry,
        Err(e) if e.code() == git2::ErrorCode::NotFound => return Ok((out, None)),
        Err(e) => return Err(e.to_string()),
    };
    if entry.filemode() != 0o100644 && entry.filemode() != 0o100755 {
        return Err("Not a regular Git blob".into());
    }
    let blob = repo.find_blob(entry.id()).map_err(|e| e.to_string())?;
    if blob.size() > MAX_BYTES || blob.is_binary() {
        return Err("Blame unavailable for binary or >512 KiB files".into());
    }
    let text = std::str::from_utf8(blob.content()).map_err(|e| e.to_string())?;
    // Match CodeBuffer::from_text normalization exactly, including no synthetic
    // trailing row. A truly empty buffer still has one editable blank line.
    let cleaned = text.replace('\r', "");
    out.baseline = cleaned.split('\n').map(str::to_owned).collect();
    if cleaned.ends_with('\n') {
        out.baseline.pop();
    }
    if out.baseline.is_empty() {
        out.baseline.push(String::new());
    }
    if out.baseline.len() > 20_000 {
        return Err("Blame limited to 20,000 lines".into());
    }
    out.lines.resize(out.baseline.len(), None);
    let mut opts = git2::BlameOptions::new();
    opts.newest_commit(oid);
    let attribution = repo
        .blame_file(&path, Some(&mut opts))
        .map_err(|e| e.to_string())?;
    let mut indices = HashMap::new();
    let mut metadata_bytes = 0usize;
    for hunk in attribution.iter() {
        let oid = hunk.final_commit_id();
        let index = *indices.entry(oid).or_insert_with(|| {
            let sig = hunk.final_signature();
            let index = out.commits.len() as u32;
            out.commits.push(GitBlameCommit {
                sha: oid.to_string(),
                author: sig
                    .name()
                    .unwrap_or("<no name>")
                    .chars()
                    .take(128)
                    .collect(),
                email: sig
                    .email()
                    .filter(|email| email.len() <= 1024)
                    .unwrap_or("")
                    .into(),
                timestamp: sig.when().seconds(),
                summary: repo
                    .find_commit(oid)
                    .ok()
                    .and_then(|c| c.summary().map(|s| s.chars().take(512).collect()))
                    .unwrap_or_default(),
                avatar: None,
            });
            let commit = out.commits.last().unwrap();
            metadata_bytes += commit.author.len()
                + commit.email.len()
                + commit.summary.len()
                + commit.sha.len();
            index
        });
        if out.commits.len() > 4096 || metadata_bytes > 1024 * 1024 {
            return Err("Blame metadata limit exceeded".into());
        }
        let start = hunk.final_start_line().saturating_sub(1);
        let end = start
            .saturating_add(hunk.lines_in_hunk())
            .min(out.lines.len());
        for line in out.lines.get_mut(start..end).into_iter().flatten() {
            *line = Some(index);
        }
    }
    let github = repo
        .find_remote("origin")
        .ok()
        .and_then(|r| r.url().and_then(github_repo));
    Ok((out, github))
}

fn github_repo(remote: &str) -> Option<String> {
    let path = remote
        .strip_prefix("https://github.com/")
        .or_else(|| remote.strip_prefix("git@github.com:"))
        .or_else(|| remote.strip_prefix("ssh://git@github.com/"))?
        .trim_end_matches(".git");
    let parts: Vec<_> = path.split('/').collect();
    (parts.len() == 2
        && parts.iter().all(|s| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
        }))
    .then(|| path.into())
}

static AVATAR_GATE: Semaphore = Semaphore::const_new(4);
type AvatarCache = Mutex<VecDeque<(String, std::time::Instant, Option<String>)>>;
// Negative results expire, so offline startup does not permanently suppress identities.
static AVATARS: OnceLock<AvatarCache> = OnceLock::new();
async fn avatar(commit: &GitBlameCommit, github: Option<&str>) -> Option<String> {
    let key = if !commit.email.is_empty() && !commit.email.contains("[bot]") {
        format!("email:{}", commit.email)
    } else {
        format!("{}:{}", github?, commit.sha)
    };
    let cache = AVATARS.get_or_init(Default::default);
    if let Some((_, _, result)) = cache
        .lock()
        .await
        .iter()
        .find(|(k, time, _)| k == &key && time.elapsed() < Duration::from_secs(600))
    {
        return result.clone();
    }
    let _permit = AVATAR_GATE.acquire().await.ok()?;
    let result = fetch_avatar(commit, github).await;
    let mut cache = cache.lock().await;
    if cache.len() >= 256 {
        cache.pop_front();
    }
    cache.push_back((key, std::time::Instant::now(), result.clone()));
    result
}
async fn bounded_body(mut response: reqwest::Response) -> Option<Vec<u8>> {
    if !response.status().is_success()
        || response
            .content_length()
            .is_some_and(|n| n > MAX_BYTES as u64)
    {
        return None;
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        if bytes.len() + chunk.len() > MAX_BYTES {
            return None;
        }
        bytes.extend_from_slice(&chunk);
    }
    Some(bytes)
}
async fn fetch_avatar(commit: &GitBlameCommit, github: Option<&str>) -> Option<String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(4))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent("Neoism-git-blame")
        .build()
        .ok()?;
    let url = if !commit.email.is_empty() && !commit.email.contains("[bot]") {
        let mut url =
            url::Url::parse("https://avatars.githubusercontent.com/u/e").ok()?;
        url.query_pairs_mut()
            .append_pair("email", &commit.email)
            .append_pair("s", "128");
        url.to_string()
    } else {
        let mut request = client.get(format!(
            "https://api.github.com/repos/{}/commits/{}",
            github?, commit.sha
        ));
        if let Ok(token) = std::env::var("GITHUB_TOKEN") {
            request = request.bearer_auth(token);
        }
        let body = bounded_body(request.send().await.ok()?).await?;
        let value: serde_json::Value = serde_json::from_slice(&body).ok()?;
        value.get("author")?.get("avatar_url")?.as_str()?.to_owned()
    };
    let url = url::Url::parse(&url).ok()?;
    if url.scheme() != "https"
        || url.host_str() != Some("avatars.githubusercontent.com")
        || url.port().is_some()
        || !url.username().is_empty()
    {
        return None;
    }
    let bytes = bounded_body(client.get(url).send().await.ok()?).await?;
    tokio::task::spawn_blocking(move || {
        let mut reader = image_rs::ImageReader::new(std::io::Cursor::new(bytes))
            .with_guessed_format()
            .ok()?;
        let mut limits = image_rs::Limits::default();
        limits.max_image_width = Some(1024);
        limits.max_image_height = Some(1024);
        limits.max_alloc = Some(8 * 1024 * 1024);
        reader.limits(limits);
        let mut image = reader
            .decode()
            .ok()?
            .resize_exact(32, 32, image_rs::imageops::FilterType::Triangle)
            .to_rgba8();
        for (x, y, pixel) in image.enumerate_pixels_mut() {
            let distance = ((x as f32 - 15.5).powi(2) + (y as f32 - 15.5).powi(2)).sqrt();
            pixel[3] = (pixel[3] as f32 * (16.0 - distance).clamp(0.0, 1.0)) as u8;
        }
        Some(STANDARD.encode(image.into_raw()))
    })
    .await
    .ok()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commit_file(repo: &git2::Repository, text: &str, author: &str) -> git2::Oid {
        let root = repo.workdir().unwrap();
        std::fs::write(root.join("file.rs"), text).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new("file.rs")).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let signature = git2::Signature::new(
            author,
            "test@example.invalid",
            &git2::Time::new(1_700_000_000, 0),
        )
        .unwrap();
        let parent = repo.head().ok().and_then(|h| h.peel_to_commit().ok());
        let parents: Vec<_> = parent.iter().collect();
        repo.commit(
            Some("HEAD"),
            &signature,
            &signature,
            "fixture commit",
            &tree,
            &parents,
        )
        .unwrap()
    }

    #[test]
    fn head_snapshot_is_read_only_and_tracks_commit_identity() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let repo = git2::Repository::init(&root).unwrap();
        let path = PathBuf::from("file.rs");
        let unborn = blame((root.clone(), String::new(), path.clone()))
            .unwrap()
            .0;
        assert!(unborn.baseline.is_empty());
        let first = commit_file(&repo, "one\ntwo\nthree\n", "Alice");
        let second = commit_file(&repo, "one\nTWO\nthree\n", "Bob");
        // Even an unsaved/staged worktree must not contaminate HEAD attribution.
        std::fs::write(root.join(&path), "dirty\n").unwrap();
        let current = blame((root.clone(), second.to_string(), path.clone()))
            .unwrap()
            .0;
        assert_eq!(current.baseline, vec!["one", "TWO", "three"]);
        let authors: Vec<_> = current
            .lines
            .iter()
            .map(|line| current.commits[line.unwrap() as usize].author.as_str())
            .collect();
        assert_eq!(authors, vec!["Alice", "Bob", "Alice"]);
        assert_eq!(
            std::fs::read_to_string(root.join(&path)).unwrap(),
            "dirty\n"
        );
        assert_eq!(repo.head().unwrap().target(), Some(second));
        let original = blame((root.clone(), first.to_string(), path)).unwrap().0;
        assert_ne!(original.head, current.head);
        assert_eq!(original.baseline[1], "two");
        let untracked =
            blame((root.clone(), second.to_string(), PathBuf::from("new.rs")))
                .unwrap()
                .0;
        assert!(untracked.baseline.is_empty());
        let binary = commit_file(&repo, "binary\0data", "Bob");
        assert!(blame((root, binary.to_string(), PathBuf::from("file.rs")),).is_err());
    }

    #[tokio::test]
    async fn nested_repository_and_workspace_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        git2::Repository::init(&root).unwrap();
        let nested = root.join("nested");
        git2::Repository::init(&nested).unwrap();
        // Unborn HEAD does no HTTP, but still resolves the owning repository.
        let snapshot = load(root.clone(), "nested/new.rs".into()).await.unwrap();
        assert_eq!(snapshot.repo_root, nested.to_string_lossy());
        assert!(snapshot.head.is_empty());
        assert!(load(root.clone(), "../outside.rs".into()).await.is_err());
        #[cfg(unix)]
        {
            let outside = tempfile::tempdir().unwrap();
            std::os::unix::fs::symlink(outside.path(), root.join("escape")).unwrap();
            assert!(load(root, "escape/file.rs".into()).await.is_err());
        }
    }

    #[test]
    fn github_remote_boundary() {
        assert_eq!(
            github_repo("git@github.com:zed-industries/zed.git"),
            Some("zed-industries/zed".into())
        );
        assert!(github_repo("https://github.com.evil/a/b").is_none());
        assert!(github_repo("https://github.com/a/b/../../x").is_none());
    }
}
