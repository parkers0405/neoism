//! Host-only GitHub backend. No shell, checkout, push, or credential material is exposed.
use neoism_protocol::{
    git::{GitChangeStatus, GitFileChange, GitFileDiff},
    pull_request::*,
};
use serde_json::{json, Value};
#[path = "pull_request_checkout.rs"]
mod checkout;
#[path = "pull_request_identity.rs"]
mod identity;
#[path = "pull_request_inbox.rs"]
mod inbox;
pub(crate) fn push_if_associated(root: &Path) -> Option<std::result::Result<(), String>> {
    identity::push(root)
}
pub(crate) fn is_associated_branch(
    root: &Path,
    branch: &str,
) -> std::result::Result<bool, String> {
    identity::associated(root, branch)
}
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Child, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, (PullRequestErrorKind, String)>;
const LIMIT: usize = 8 * 1024 * 1024;
const FILES: usize = 300;
const THREAD_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewThreads(first:100){pageInfo{hasNextPage} nodes{id path diffSide line startLine isOutdated isResolved comments(first:100){pageInfo{hasNextPage} nodes{id databaseId body createdAt url author{login}}}}}}}}";
const METADATA_QUERY: &str = "query($owner:String!,$repo:String!,$number:Int!){repository(owner:$owner,name:$repo){pullRequest(number:$number){reviewDecision commits(last:1){nodes{commit{statusCheckRollup{state}}}}}}}";
fn error(kind: PullRequestErrorKind, message: &str) -> (PullRequestErrorKind, String) {
    (kind, message.into())
}
fn invalid(message: &str) -> (PullRequestErrorKind, String) {
    error(PullRequestErrorKind::Invalid, message)
}
fn text(v: &Value, key: &str) -> String {
    v[key].as_str().unwrap_or_default().into()
}
fn list(v: &Value) -> &[Value] {
    v.as_array().map(Vec::as_slice).unwrap_or_default()
}
fn sha(s: &str) -> bool {
    s.len() == 40 && s.bytes().all(|c| c.is_ascii_hexdigit())
}
fn branch(s: &str) -> bool {
    !s.is_empty()
        && !s.starts_with('-')
        && !s.contains("..")
        && !s.contains("@{")
        && !s.ends_with('.')
        && s.bytes()
            .all(|c| c.is_ascii_alphanumeric() || b"/_-.".contains(&c))
}
fn body(s: &str) -> Result<()> {
    if s.len() > 65536 || s.contains('\0') {
        Err(invalid("Text exceeds the supported limit or contains NUL"))
    } else {
        Ok(())
    }
}

fn terminate(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let mut kill = std::process::Command::new("taskkill");
        crate::hide_std_command(&mut kill);
        let _ = kill
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.kill();
    let _ = child.wait();
}

/// Bounded stdout/stderr are drained concurrently; every timeout/overflow kills and reaps
/// the process. Raw stderr is deliberately never returned (may contain credentials).
fn run(
    root: &Path,
    program: &str,
    args: &[String],
    input: Option<&Value>,
) -> Result<String> {
    #[cfg(test)]
    if let Some(result) = tests::mock_run(program, args, input) {
        return result;
    }
    let mut cmd = crate::process::background_command(program);
    cmd.current_dir(root)
        .args(args)
        .env("GH_PROMPT_DISABLED", "1")
        .env("GH_PAGER", "cat")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_NO_LAZY_FETCH", "1")
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .env_remove("GIT_INDEX_FILE")
        .env_remove("GIT_COMMON_DIR")
        .env_remove("GIT_OBJECT_DIRECTORY")
        .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
        .env_remove("GIT_NAMESPACE")
        .env_remove("GIT_CONFIG_COUNT")
        .env_remove("GIT_CONFIG_PARAMETERS")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    crate::hide_std_command(&mut cmd);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        error(
            if e.kind() == std::io::ErrorKind::NotFound {
                PullRequestErrorKind::Unavailable
            } else {
                PullRequestErrorKind::Failed
            },
            if program == "gh" {
                "GitHub CLI is unavailable; install gh on the workspace host"
            } else {
                "Cannot start host Git"
            },
        )
    })?;
    let overflow = Arc::new(AtomicBool::new(false));
    fn drain(
        mut pipe: impl Read + Send + 'static,
        overflow: Arc<AtomicBool>,
    ) -> std::thread::JoinHandle<Vec<u8>> {
        std::thread::spawn(move || {
            let mut out = Vec::new();
            let mut buf = [0; 8192];
            loop {
                match pipe.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if out.len() + n <= LIMIT {
                            out.extend_from_slice(&buf[..n]);
                        } else {
                            overflow.store(true, Ordering::Relaxed);
                        }
                    }
                }
            }
            out
        })
    }
    let out = drain(child.stdout.take().unwrap(), overflow.clone());
    let err = drain(child.stderr.take().unwrap(), overflow.clone());
    // Payload is bounded before spawning and written concurrently so pipe pressure
    // cannot prevent the timeout watchdog from running.
    let mut stdin = child.stdin.take().unwrap();
    let payload = input.map(|v| v.to_string()).unwrap_or_default();
    let writer = std::thread::spawn(move || {
        let _ = stdin.write_all(payload.as_bytes());
    });
    let start = Instant::now();
    let status = loop {
        if start.elapsed() > Duration::from_secs(30) || overflow.load(Ordering::Relaxed) {
            terminate(&mut child);
            let _ = writer.join();
            let _ = out.join();
            let _ = err.join();
            return Err(error(PullRequestErrorKind::Unavailable, "Host operation timed out or exceeded the output limit; refresh before retrying a mutation"));
        }
        match child.try_wait() {
            Ok(Some(status))
                if out.is_finished() && err.is_finished() && writer.is_finished() =>
            {
                break status
            }
            Ok(Some(_)) => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            Err(_) => {
                terminate(&mut child);
                return Err(error(PullRequestErrorKind::Failed, "Host process failed"));
            }
        }
    };
    let _ = writer.join();
    let stdout = out.join().unwrap_or_default();
    let stderr = err.join().unwrap_or_default();
    if overflow.load(Ordering::Relaxed) {
        return Err(error(
            PullRequestErrorKind::Unavailable,
            "Host response exceeded output limit",
        ));
    }
    if !status.success() {
        let e = String::from_utf8_lossy(&stderr).to_lowercase();
        let kind = if e.contains("401")
            || e.contains("authentication")
            || e.contains("gh auth login")
            || e.contains("not logged")
        {
            PullRequestErrorKind::Authentication
        } else if e.contains("403") || e.contains("resource not accessible") {
            PullRequestErrorKind::Permission
        } else if e.contains("409") || e.contains("422") || e.contains("405") {
            PullRequestErrorKind::Conflict
        } else {
            PullRequestErrorKind::Failed
        };
        return Err(error(kind, "GitHub host operation failed; check host authentication, repository access, and server policy"));
    }
    String::from_utf8(stdout)
        .map_err(|_| error(PullRequestErrorKind::Failed, "Host returned invalid UTF-8"))
}
fn git(root: &Path, args: &[&str]) -> Result<String> {
    run(
        root,
        "git",
        &args.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
        None,
    )
    .map(|s| s.trim().into())
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Identity {
    owner: String,
    repo: String,
}
impl Identity {
    fn parse(remote: &str) -> Result<Self> {
        let path = if let Some(p) = remote.strip_prefix("git@github.com:") {
            p
        } else if let Some(p) = remote.strip_prefix("https://github.com/") {
            p
        } else if let Some(p) = remote.strip_prefix("ssh://git@github.com/") {
            p
        } else {
            return Err(error(
                PullRequestErrorKind::Unsupported,
                "Only explicit github.com Git remotes are supported",
            ));
        };
        let path = path.strip_suffix(".git").unwrap_or(path);
        let parts: Vec<_> = path.split('/').collect();
        if parts.len() != 2
            || parts.iter().any(|s| {
                s.is_empty()
                    || *s == "."
                    || *s == ".."
                    || !s
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
            })
        {
            return Err(invalid("Invalid GitHub repository identity"));
        }
        Ok(Self {
            owner: parts[0].into(),
            repo: parts[1].into(),
        })
    }
    fn name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }
    fn endpoint(&self, tail: &str) -> String {
        if tail.is_empty() {
            format!("repos/{}/{}", self.owner, self.repo)
        } else {
            format!("repos/{}/{}/{}", self.owner, self.repo, tail)
        }
    }
}
struct Host<'a> {
    root: &'a Path,
    repo: Identity,
    origin: Identity,
}
impl<'a> Host<'a> {
    fn new(root: &'a Path) -> Result<Self> {
        let origin = Identity::parse(&git(root, &["remote", "get-url", "origin"])?)?;
        let repo = match git(root, &["remote", "get-url", "upstream"]) {
            Ok(url) => Identity::parse(&url)?,
            Err(_) => origin.clone(),
        };
        let mut host = Self { root, repo, origin };
        // Fork origin without an upstream remote: target the server-declared parent.
        let info = host.api("GET", &host.repo.endpoint(""), None)?;
        if info["fork"].as_bool() == Some(true) {
            let parent = text(&info["parent"], "full_name");
            host.repo = Identity::parse(&format!("https://github.com/{parent}"))?;
        }
        Ok(host)
    }
    fn api(&self, method: &str, endpoint: &str, payload: Option<Value>) -> Result<Value> {
        let mut args = vec![
            "api".into(),
            "--hostname".into(),
            "github.com".into(),
            "--method".into(),
            method.into(),
            endpoint.into(),
        ];
        if payload.is_some() {
            args.extend(["--input".into(), "-".into()]);
        }
        let raw = run(self.root, "gh", &args, payload.as_ref())?;
        if raw.trim().is_empty() {
            return Ok(Value::Null);
        }
        let v: Value = serde_json::from_str(&raw).map_err(|_| {
            error(PullRequestErrorKind::Failed, "Invalid GitHub response")
        })?;
        if v["errors"].is_array() {
            return Err(error(
                PullRequestErrorKind::Failed,
                "GitHub GraphQL operation rejected",
            ));
        }
        Ok(v)
    }
    fn pr(&self, number: u64) -> Result<Value> {
        if number == 0 {
            return Err(invalid("PR number must be positive"));
        }
        self.api("GET", &self.repo.endpoint(&format!("pulls/{number}")), None)
    }
    fn expected(&self, number: u64, expected: &str) -> Result<Value> {
        if !sha(expected) {
            return Err(invalid("Expected head must be a full commit SHA"));
        }
        let pr = self.pr(number)?;
        if text(&pr["head"], "sha") != expected {
            return Err(error(
                PullRequestErrorKind::Stale,
                "PR head changed; refresh before continuing",
            ));
        }
        if text(&pr, "state") != "open" {
            return Err(error(
                PullRequestErrorKind::Conflict,
                "PR is no longer open",
            ));
        }
        Ok(pr)
    }
    fn graphql(&self, query: &str, variables: Value) -> Result<Value> {
        self.api(
            "POST",
            "graphql",
            Some(json!({"query": query, "variables": variables})),
        )
    }
    fn threads(&self, number: u64) -> Result<(Vec<PullRequestThread>, bool)> {
        let v = self.graphql(
            THREAD_QUERY,
            json!({"owner":self.repo.owner,"repo":self.repo.repo,"number":number}),
        )?;
        let conn = &v["data"]["repository"]["pullRequest"]["reviewThreads"];
        if conn.is_null() {
            return Err(error(
                PullRequestErrorKind::Failed,
                "Cannot read PR review threads",
            ));
        }
        let mut truncated = conn["pageInfo"]["hasNextPage"].as_bool().unwrap_or(false);
        let threads = list(&conn["nodes"])
            .iter()
            .map(|t| {
                truncated |= t["comments"]["pageInfo"]["hasNextPage"]
                    .as_bool()
                    .unwrap_or(false);
                PullRequestThread {
                    id: text(t, "id"),
                    path: text(t, "path"),
                    side: if text(t, "diffSide") == "LEFT" {
                        PullRequestSide::Left
                    } else {
                        PullRequestSide::Right
                    },
                    line: t["line"].as_u64().map(|n| n as u32),
                    start_line: t["startLine"].as_u64().map(|n| n as u32),
                    outdated: t["isOutdated"].as_bool().unwrap_or(false),
                    resolved: t["isResolved"].as_bool().unwrap_or(false),
                    comments: list(&t["comments"]["nodes"])
                        .iter()
                        .map(|c| PullRequestComment {
                            id: text(c, "id"),
                            author: text(&c["author"], "login"),
                            body: text(c, "body"),
                            created_at: text(c, "createdAt"),
                            url: text(c, "url"),
                        })
                        .collect(),
                }
            })
            .collect();
        Ok((threads, truncated))
    }
    fn enrich(&self, summary: &mut PullRequestSummary) -> Result<()> {
        let v = self.graphql(METADATA_QUERY, json!({"owner":self.repo.owner,"repo":self.repo.repo,"number":summary.number}))?;
        let pr = &v["data"]["repository"]["pullRequest"];
        if pr.is_null() {
            return Err(error(
                PullRequestErrorKind::Failed,
                "Cannot read PR metadata",
            ));
        }
        summary.review_decision = text(pr, "reviewDecision");
        summary.checks_state = text(
            &pr["commits"]["nodes"][0]["commit"]["statusCheckRollup"],
            "state",
        );
        Ok(())
    }
    fn files(&self, number: u64) -> Result<Vec<Value>> {
        let mut files = Vec::new();
        for page in 1..=3 {
            let v = self.api(
                "GET",
                &self
                    .repo
                    .endpoint(&format!("pulls/{number}/files?per_page=100&page={page}")),
                None,
            )?;
            let entries = list(&v);
            files.extend_from_slice(entries);
            if entries.len() < 100 {
                break;
            }
        }
        Ok(files)
    }
}
fn summary(pr: &Value) -> PullRequestSummary {
    PullRequestSummary {
        number: pr["number"].as_u64().unwrap_or_default(),
        title: text(pr, "title"),
        url: text(pr, "html_url"),
        author: text(&pr["user"], "login"),
        state: text(pr, "state"),
        draft: pr["draft"].as_bool().unwrap_or(false),
        base_branch: text(&pr["base"], "ref"),
        head_branch: text(&pr["head"], "ref"),
        base_sha: text(&pr["base"], "sha"),
        head_sha: text(&pr["head"], "sha"),
        review_decision: String::new(),
        checks_state: String::new(),
    }
}
fn comment(c: &Value) -> PullRequestComment {
    PullRequestComment {
        id: c["id"].to_string(),
        author: text(&c["user"], "login"),
        body: text(c, "body"),
        created_at: text(c, "created_at"),
        url: text(c, "html_url"),
    }
}

/// Immutable merge-base-to-head GitHub patches: never the working tree or index.
fn detail(host: &Host<'_>, number: u64) -> Result<PullRequestDetail> {
    let pr = host.pr(number)?;
    let mut truncated = false;
    let raw = host.files(number)?;
    truncated |= raw.len() >= FILES
        || pr["changed_files"].as_u64().unwrap_or(0) > raw.len() as u64;
    let mut files = Vec::new();
    let mut diffs = Vec::new();
    for f in raw {
        let path = text(&f, "filename");
        let patch = text(&f, "patch");
        // GitHub omits binary/oversized patches. Never pretend these are complete.
        if f["patch"].is_null() || patch.len() >= 65536 {
            truncated = true;
        }
        files.push(GitFileChange {
            path: path.clone(),
            status: match text(&f, "status").as_str() {
                "added" => GitChangeStatus::Added,
                "removed" => GitChangeStatus::Deleted,
                "renamed" => GitChangeStatus::Renamed,
                _ => GitChangeStatus::Modified,
            },
            additions: f["additions"].as_u64().unwrap_or(0) as u32,
            deletions: f["deletions"].as_u64().unwrap_or(0) as u32,
            staged: false,
        });
        diffs.push(GitFileDiff { path, patch });
    }
    let comments = host.api(
        "GET",
        &host
            .repo
            .endpoint(&format!("issues/{number}/comments?per_page=100")),
        None,
    )?;
    truncated |= list(&comments).len() == 100;
    let reviews = host.api(
        "GET",
        &host
            .repo
            .endpoint(&format!("pulls/{number}/reviews?per_page=100")),
        None,
    )?;
    truncated |= list(&reviews).len() == 100;
    let mut conversation: Vec<_> = list(&comments).iter().map(comment).collect();
    for c in &mut conversation {
        c.id = format!("comment:{}", c.id);
    }
    conversation.extend(
        list(&reviews)
            .iter()
            .filter(|r| !text(r, "body").is_empty())
            .map(|r| {
                let mut c = comment(r);
                c.id = format!("review:{}", c.id);
                c.created_at = text(r, "submitted_at");
                c
            }),
    );
    conversation.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    let (threads, cut) = host.threads(number)?;
    truncated |= cut;
    let head = text(&pr["head"], "sha");
    if !sha(&head) {
        return Err(error(PullRequestErrorKind::Failed, "Invalid PR head"));
    }
    let checks = host.api(
        "GET",
        &host
            .repo
            .endpoint(&format!("commits/{head}/check-runs?per_page=100")),
        None,
    )?;
    truncated |= checks["total_count"].as_u64().unwrap_or(0)
        > list(&checks["check_runs"]).len() as u64;
    let statuses = host.api(
        "GET",
        &host
            .repo
            .endpoint(&format!("commits/{head}/status?per_page=100")),
        None,
    )?;
    truncated |= statuses["total_count"].as_u64().unwrap_or(0)
        > list(&statuses["statuses"]).len() as u64;
    let mut check_list: Vec<_> = list(&checks["check_runs"])
        .iter()
        .map(|c| PullRequestCheck {
            name: text(c, "name"),
            status: text(c, "status"),
            conclusion: text(c, "conclusion"),
            url: text(c, "html_url"),
        })
        .collect();
    check_list.extend(
        list(&statuses["statuses"])
            .iter()
            .map(|c| PullRequestCheck {
                name: text(c, "context"),
                status: text(c, "state"),
                conclusion: text(c, "state"),
                url: text(c, "target_url"),
            }),
    );
    // Detect races across paginated PR files and discussions; never return a mixed revision.
    let current = host.pr(number)?;
    if current["head"]["sha"] != pr["head"]["sha"]
        || current["base"]["sha"] != pr["base"]["sha"]
    {
        return Err(error(
            PullRequestErrorKind::Stale,
            "PR comparison changed while loading; refresh",
        ));
    }
    let mut summary = summary(&pr);
    host.enrich(&mut summary)?;
    let final_pr = host.pr(number)?;
    if final_pr["head"]["sha"] != pr["head"]["sha"]
        || final_pr["base"]["sha"] != pr["base"]["sha"]
    {
        return Err(error(
            PullRequestErrorKind::Stale,
            "PR comparison changed while loading metadata; refresh",
        ));
    }
    Ok(PullRequestDetail {
        summary,
        body: text(&pr, "body"),
        files,
        diffs,
        conversation,
        threads,
        checks: check_list,
        truncated,
    })
}

// Source-line membership in a unified patch, not rendered/visible-row coordinates.
fn contains_line(patch: &str, side: PullRequestSide, wanted: u32) -> bool {
    let (mut old, mut new) = (0u32, 0u32);
    let mut active = false;
    for line in patch.lines() {
        if line.starts_with("@@ ") {
            let fields: Vec<_> = line.split_whitespace().collect();
            let parse = |s: &str| {
                s.get(1..)
                    .and_then(|s| s.split(',').next())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0)
            };
            if fields.len() < 3 {
                return false;
            }
            old = parse(fields[1]);
            new = parse(fields[2]);
            active = true;
            continue;
        }
        if !active {
            continue;
        }
        let (left, right) = match line.as_bytes().first() {
            Some(b' ') => (true, true),
            Some(b'-') => (true, false),
            Some(b'+') => (false, true),
            _ => (false, false),
        };
        if (side == PullRequestSide::Left && left && old == wanted)
            || (side == PullRequestSide::Right && right && new == wanted)
        {
            return true;
        }
        if left {
            old += 1;
        }
        if right {
            new += 1;
        }
    }
    false
}

pub async fn handle(root: PathBuf, request: PullRequestRequest) -> PullRequestReply {
    match tokio::task::spawn_blocking(move || handle_at(&root, request)).await {
        Ok(reply) => reply,
        Err(_) => PullRequestReply::Error {
            kind: PullRequestErrorKind::Failed,
            message: "PR worker failed".into(),
        },
    }
}
/// Attached-host entry point: current-workspace checkout must use this hub.
pub async fn handle_with_hub(
    root: PathBuf,
    request: PullRequestRequest,
    hub: crate::crdt::sync::CrdtSyncHub,
) -> PullRequestReply {
    match tokio::task::spawn_blocking(move || {
        match dispatch(&root, request, Some(&hub)) {
            Ok(reply) => reply,
            Err((kind, message)) => PullRequestReply::Error { kind, message },
        }
    })
    .await
    {
        Ok(reply) => reply,
        Err(_) => PullRequestReply::Error {
            kind: PullRequestErrorKind::Failed,
            message: "PR worker failed".into(),
        },
    }
}
pub fn handle_at(root: &Path, request: PullRequestRequest) -> PullRequestReply {
    match dispatch(root, request, None) {
        Ok(reply) => reply,
        Err((kind, message)) => PullRequestReply::Error { kind, message },
    }
}
fn dispatch(
    root: &Path,
    request: PullRequestRequest,
    hub: Option<&crate::crdt::sync::CrdtSyncHub>,
) -> Result<PullRequestReply> {
    if let PullRequestRequest::Checkout {
        number,
        expected_head,
        new_worktree,
    } = request
    {
        return checkout::checkout(root, number, &expected_head, new_worktree, hub);
    }
    let authorized_root = checkout::authorized_root(root)?;
    let root = authorized_root.as_path();
    let host = Host::new(root)?;
    match request {
        PullRequestRequest::List {
            query,
            sort,
            filter,
            cursor,
            generation,
        } => inbox::page(&host, &query, sort, filter, cursor, generation),
        PullRequestRequest::ReadInbox { number } => inbox::read(&host, number),
        PullRequestRequest::Discover { branch: requested } => {
            let current = git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
            let branch = requested.unwrap_or(current);
            if !self::branch(&branch) {
                return Err(invalid("Invalid branch"));
            }
            let head = git(
                root,
                &["rev-parse", "--verify", &format!("refs/heads/{branch}")],
            )?;
            let info = host.api("GET", &host.repo.endpoint(""), None)?;
            if let Some(pull_request) = identity::discover(&host, &branch)? {
                return Ok(PullRequestReply::Discovered {
                    branch,
                    head,
                    default_branch: text(&info, "default_branch"),
                    pull_request: Some(pull_request),
                });
            }
            let query = url::form_urlencoded::Serializer::new(String::new())
                .append_pair("head", &format!("{}:{branch}", host.origin.owner))
                .finish();
            let prs = host.api(
                "GET",
                &host
                    .repo
                    .endpoint(&format!("pulls?state=open&per_page=100&{query}")),
                None,
            )?;
            let mut pull_request = list(&prs).first().map(summary);
            if let Some(pr) = &mut pull_request {
                host.enrich(pr)?;
            }
            Ok(PullRequestReply::Discovered {
                branch,
                head,
                default_branch: text(&info, "default_branch"),
                pull_request,
            })
        }
        PullRequestRequest::Read { number } => Ok(PullRequestReply::Detail {
            detail: detail(&host, number)?,
        }),
        PullRequestRequest::Create {
            title,
            body: content,
            base,
            draft,
            expected_branch,
            expected_head,
        } => {
            if title.trim().is_empty()
                || title.len() > 256
                || title.contains(['\r', '\n', '\0'])
                || !branch(&base)
                || !branch(&expected_branch)
                || !sha(&expected_head)
            {
                return Err(invalid(
                    "Valid title, base, branch, and full expected head are required",
                ));
            }
            body(&content)?;
            if git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
                != expected_branch
                || git(root, &["rev-parse", "HEAD"])? != expected_head
            {
                return Err(error(
                    PullRequestErrorKind::Stale,
                    "Current branch or head changed",
                ));
            }
            let upstream = git(
                root,
                &[
                    "rev-parse",
                    "--abbrev-ref",
                    "--symbolic-full-name",
                    "@{upstream}",
                ],
            )?;
            let remote = git(
                root,
                &[
                    "config",
                    "--get",
                    &format!("branch.{expected_branch}.remote"),
                ],
            )?;
            if remote != "origin"
                || upstream != format!("origin/{expected_branch}")
                || git(root, &["rev-parse", "@{upstream}"])? != expected_head
            {
                return Err(error(PullRequestErrorKind::Conflict,"Publish this exact branch/head to origin first; PR creation never pushes"));
            }
            // Verify the remote server, not just potentially stale tracking refs.
            let remote_branch = host.api(
                "GET",
                &host
                    .origin
                    .endpoint(&format!("git/ref/heads/{expected_branch}")),
                None,
            )?;
            if text(&remote_branch["object"], "sha") != expected_head {
                return Err(error(
                    PullRequestErrorKind::Stale,
                    "Published branch does not match expected head",
                ));
            }
            let base_ref = host.api(
                "GET",
                &host.repo.endpoint(&format!("git/ref/heads/{base}")),
                None,
            )?;
            if !sha(&text(&base_ref["object"], "sha")) {
                return Err(invalid("Base branch does not exist"));
            }
            // Network preflight may take time; recheck the local proof before mutation.
            if git(root, &["symbolic-ref", "--quiet", "--short", "HEAD"])?
                != expected_branch
                || git(root, &["rev-parse", "HEAD"])? != expected_head
                || git(root, &["rev-parse", "@{upstream}"])? != expected_head
            {
                return Err(error(
                    PullRequestErrorKind::Stale,
                    "Local branch/head changed during PR preflight",
                ));
            }
            let pr = host.api("POST",&host.repo.endpoint("pulls"),Some(json!({"title":title,"body":content,"base":base,"head":format!("{}:{expected_branch}",host.origin.owner),"draft":draft})))?;
            Ok(PullRequestReply::Created {
                pull_request: summary(&pr),
            })
        }
        PullRequestRequest::SubmitReview {
            number,
            expected_head,
            body: content,
            event,
            comments,
        } => {
            body(&content)?;
            if comments.len() > 100 {
                return Err(invalid("At most 100 inline comments are supported"));
            }
            host.expected(number, &expected_head)?;
            let files = host.files(number)?;
            let mut inline = Vec::new();
            for c in comments {
                body(&c.body)?;
                if c.body.trim().is_empty()
                    || c.line == 0
                    || c.start_line
                        .is_some_and(|s| s == 0 || s > c.line || c.line - s > 1000)
                {
                    return Err(invalid("Invalid inline comment or source range"));
                }
                let file = files
                    .iter()
                    .find(|f| text(f, "filename") == c.path)
                    .ok_or_else(|| invalid("Inline path is not in this PR comparison"))?;
                let patch = text(file, "patch");
                for line in c.start_line.unwrap_or(c.line)..=c.line {
                    if !contains_line(&patch, c.side, line) {
                        return Err(invalid("Inline source line is not present on the selected side of the PR patch"));
                    }
                }
                let side = if c.side == PullRequestSide::Left {
                    "LEFT"
                } else {
                    "RIGHT"
                };
                let mut v =
                    json!({"path":c.path,"side":side,"line":c.line,"body":c.body});
                if let Some(start) = c.start_line {
                    if start != c.line {
                        v["start_line"] = json!(start);
                        v["start_side"] = json!(side);
                    }
                }
                inline.push(v);
            }
            if event == PullRequestReviewEvent::Comment
                && content.trim().is_empty()
                && inline.is_empty()
            {
                return Err(invalid("An empty comment review cannot be submitted"));
            }
            host.expected(number, &expected_head)?;
            let event = match event {
                PullRequestReviewEvent::Comment => "COMMENT",
                PullRequestReviewEvent::Approve => "APPROVE",
                PullRequestReviewEvent::RequestChanges => "REQUEST_CHANGES",
            };
            host.api("POST", &host.repo.endpoint(&format!("pulls/{number}/reviews")), Some(json!({"commit_id":expected_head,"body":content,"event":event,"comments":inline})))?;
            Ok(PullRequestReply::ReviewSubmitted { number })
        }
        PullRequestRequest::ReplyThread {
            number,
            thread_id,
            body: content,
        } => {
            body(&content)?;
            if content.trim().is_empty() {
                return Err(invalid("Reply body is empty"));
            }
            let (threads, _) = host.threads(number)?;
            if !threads.iter().any(|t| t.id == thread_id) {
                return Err(invalid("Thread does not belong to this repository and PR, or is outside the supported window"));
            }
            host.graphql("mutation($id:ID!,$body:String!){addPullRequestReviewThreadReply(input:{pullRequestReviewThreadId:$id,body:$body}){comment{id}}}",json!({"id":thread_id,"body":content}))?;
            Ok(PullRequestReply::ThreadUpdated { number })
        }
        PullRequestRequest::ResolveThread {
            number,
            thread_id,
            resolved,
        } => {
            let (threads, _) = host.threads(number)?;
            if !threads.iter().any(|t| t.id == thread_id) {
                return Err(invalid("Thread does not belong to this repository and PR, or is outside the supported window"));
            }
            let query = if resolved {
                "mutation($id:ID!){resolveReviewThread(input:{threadId:$id}){thread{id}}}"
            } else {
                "mutation($id:ID!){unresolveReviewThread(input:{threadId:$id}){thread{id}}}"
            };
            host.graphql(query, json!({"id":thread_id}))?;
            Ok(PullRequestReply::ThreadUpdated { number })
        }
        PullRequestRequest::Merge {
            number,
            expected_head,
            method,
        } => {
            host.expected(number, &expected_head)?;
            let method = match method {
                PullRequestMergeMethod::Merge => "merge",
                PullRequestMergeMethod::Squash => "squash",
                PullRequestMergeMethod::Rebase => "rebase",
            };
            let v = host.api(
                "PUT",
                &host.repo.endpoint(&format!("pulls/{number}/merge")),
                Some(json!({"sha":expected_head,"merge_method":method})),
            )?;
            if v["merged"].as_bool() != Some(true) {
                return Err(error(PullRequestErrorKind::Conflict,"GitHub refused the merge; branch protection and server policies were not bypassed"));
            }
            Ok(PullRequestReply::Merged { number })
        }
        PullRequestRequest::Checkout { .. } => unreachable!(),
    }
}

#[cfg(test)]
#[path = "pull_request_tests.rs"]
mod fixture_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use std::{cell::RefCell, collections::VecDeque};
    pub(super) struct Call {
        pub program: &'static str,
        pub args: Vec<String>,
        pub input: Option<Value>,
        pub result: Result<String>,
    }
    thread_local! { static MOCK: RefCell<Option<VecDeque<Call>>> = const { RefCell::new(None) }; }
    pub(super) fn fixture(calls: Vec<Call>, f: impl FnOnce()) {
        MOCK.with(|m| *m.borrow_mut() = Some(calls.into()));
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                MOCK.with(|m| *m.borrow_mut() = None);
            }
        }
        let _reset = Reset;
        f();
        MOCK.with(|m| {
            assert!(
                m.borrow().as_ref().unwrap().is_empty(),
                "Unconsumed fixture calls"
            )
        });
    }
    pub(super) fn mock_run(
        program: &str,
        args: &[String],
        input: Option<&Value>,
    ) -> Option<Result<String>> {
        MOCK.with(|m| {
            let mut mock = m.borrow_mut();
            let calls = mock.as_mut()?;
            let call = calls
                .pop_front()
                .expect("Unexpected host command (network is forbidden in fixtures)");
            assert_eq!(program, call.program);
            assert_eq!(args, call.args);
            assert_eq!(input, call.input.as_ref());
            Some(call.result)
        })
    }
    #[test]
    fn identity_is_not_an_arbitrary_url() {
        for url in [
            "git@github.com:owner/repo.git",
            "https://github.com/owner/repo.git",
            "ssh://git@github.com/owner/repo",
        ] {
            assert_eq!(Identity::parse(url).unwrap().name(), "owner/repo");
        }
        for url in [
            "https://token@github.com/a/b",
            "https://evil.test/a/b",
            "https://github.com/a/b?token=x",
            "git@github.com:a/../b",
            "https://github.com/a/b/extra",
            "-x",
        ] {
            assert!(Identity::parse(url).is_err(), "{url}");
        }
    }
    #[test]
    fn inline_coordinates_are_source_lines_and_sided() {
        let patch="@@ -10,3 +20,3 @@\n context\n-old\n+new\n tail\n\\ No newline at end of file\n";
        assert!(contains_line(patch, PullRequestSide::Left, 11));
        assert!(!contains_line(patch, PullRequestSide::Right, 11));
        assert!(contains_line(patch, PullRequestSide::Right, 21));
        assert!(!contains_line(patch, PullRequestSide::Left, 21));
        assert!(!contains_line(patch, PullRequestSide::Right, 23));
        assert!(!contains_line(patch, PullRequestSide::Left, 0));
    }
    #[test]
    fn checkout_fails_before_any_process_or_network() {
        assert!(matches!(
            handle_at(
                Path::new("/nonexistent"),
                PullRequestRequest::Checkout {
                    number: 1,
                    expected_head: "x".into(),
                    new_worktree: false
                }
            ),
            PullRequestReply::Error {
                kind: PullRequestErrorKind::Unsupported,
                ..
            }
        ));
    }
    #[test]
    fn refs_and_heads_are_strict() {
        for s in ["-evil", "a..b", "a@{b}", "a\nb", ""] {
            assert!(!branch(s));
        }
        assert!(branch("feature/pr-1"));
        assert!(!sha("HEAD"));
        assert!(sha(&"a".repeat(40)));
    }
}
