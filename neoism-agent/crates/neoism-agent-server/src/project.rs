use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use neoism_agent_core::ProjectInfo;

#[derive(Clone)]
pub(crate) struct ProjectContext {
    pub(crate) info: ProjectInfo,
    pub(crate) directory: String,
    pub(crate) path: Option<String>,
}

// Owned by one AppState's registry, never shared across injected services or
// admission domains. Short TTL also notices git init/repository replacement.
type DiscoveryCell = Arc<tokio::sync::Mutex<Option<(Instant, ProjectContext)>>>;
#[derive(Clone, Default)]
pub(crate) struct DiscoveryCache {
    entries: Arc<Mutex<HashMap<PathBuf, DiscoveryCell>>>,
}

impl DiscoveryCache {
    pub(crate) async fn discover(
        &self,
        services: &neoism_agent_service_api::AgentServices,
        directory: PathBuf,
    ) -> Result<ProjectContext, tokio::task::JoinError> {
        let directory =
            tokio::task::spawn_blocking(move || canonicalize_lossy(&directory)).await?;
        let cell = {
            let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
            if entries.len() >= 128 {
                entries.retain(|_, cell| Arc::strong_count(cell) > 1);
            }
            entries.entry(directory.clone()).or_default().clone()
        };
        // Waiters yield instead of consuming blocking-pool threads. The worker
        // owns the guard: cancelling its HTTP caller cannot abandon publication
        // or let another request start duplicate Git work.
        let mut cached = cell.lock_owned().await;
        if let Some((at, context)) = &*cached {
            if at.elapsed() < Duration::from_secs(5) {
                return Ok(context.clone());
            }
        }
        let services = services.clone();
        tokio::task::spawn_blocking(move || {
            let context = discover(&services, directory);
            *cached = Some((Instant::now(), context.clone()));
            context
        })
        .await
    }
}

pub(crate) fn discover(
    services: &neoism_agent_service_api::AgentServices,
    directory: impl AsRef<Path>,
) -> ProjectContext {
    let directory = canonicalize_lossy(directory.as_ref());
    let directory_text = path_text(&directory);
    let Some(worktree) =
        git_output(services, &directory, &["rev-parse", "--show-toplevel"])
            .map(PathBuf::from)
            .map(|path| canonicalize_lossy(&path))
    else {
        return ProjectContext {
            info: fallback_project(directory_text.clone()),
            directory: directory_text,
            path: None,
        };
    };

    let id = project_id(services, &directory).unwrap_or_else(|| "global".to_string());
    let worktree_text = path_text(&worktree);
    let path = relative_path(&worktree, &directory);
    ProjectContext {
        info: ProjectInfo {
            id,
            name: project_name(&worktree),
            directory: directory_text.clone(),
            vcs: Some("git".to_string()),
            worktree: Some(worktree_text),
        },
        directory: directory_text,
        path,
    }
}

fn project_id(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
) -> Option<String> {
    let cache = cache_path(services, directory);
    if let Some(cached) = cache.as_deref().and_then(read_cached_id) {
        return Some(cached);
    }
    let mut roots = git_output(
        services,
        directory,
        &["rev-list", "--max-parents=0", "HEAD"],
    )?
    .lines()
    .map(str::trim)
    .filter(|line| !line.is_empty())
    .map(ToString::to_string)
    .collect::<Vec<_>>();
    roots.sort();
    let id = roots.into_iter().next()?;
    if let Some(path) = cache {
        let _ = std::fs::write(path, &id);
    }
    Some(id)
}

fn read_cached_id(path: &Path) -> Option<String> {
    let mut value = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(129)
        .read_to_string(&mut value)
        .ok()?;
    if value.len() > 128 {
        return None;
    }
    let value = value.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn cache_path(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
) -> Option<PathBuf> {
    let common = git_output(services, directory, &["rev-parse", "--git-common-dir"])?;
    let common = PathBuf::from(common);
    let path = if common.is_absolute() {
        common
    } else {
        directory.join(common)
    };
    Some(path.join("neoism"))
}

fn fallback_project(directory: String) -> ProjectInfo {
    ProjectInfo {
        id: "global".to_string(),
        name: project_name(Path::new(&directory)),
        directory,
        vcs: None,
        worktree: None,
    }
}

fn relative_path(root: &Path, directory: &Path) -> Option<String> {
    let relative = directory.strip_prefix(root).ok()?;
    if relative.as_os_str().is_empty() {
        return None;
    }
    Some(relative.to_string_lossy().replace('\\', "/"))
}

fn project_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "project".to_string())
}

fn canonicalize_lossy(path: &Path) -> PathBuf {
    crate::windows_process::canonicalize_path_lossy(path)
}

fn path_text(path: &Path) -> String {
    path.to_string_lossy().to_string()
}

fn git_output(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
    args: &[&str],
) -> Option<String> {
    let command = git_command(services, directory, args).ok()?;
    bounded_output(command, Duration::from_secs(2))
}

// No interactive input, discarded stderr and a strict stdout cap. In particular,
// rev-list must not consume arbitrary memory or occupy a worker indefinitely.
fn bounded_output(
    mut command: std::process::Command,
    timeout: Duration,
) -> Option<String> {
    const MAX_OUTPUT: u64 = 64 * 1024;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::tool::process::set_new_process_group_std(&mut command);
    let mut child = command.spawn().ok()?;
    let stdout = child.stdout.take()?;
    let (send, receive) = std::sync::mpsc::sync_channel(1);
    let reader = std::thread::Builder::new()
        .name("project-git-output".into())
        .spawn(move || {
            let mut bytes = Vec::new();
            let result = stdout.take(MAX_OUTPUT + 1).read_to_end(&mut bytes);
            let _ = send.send(result.ok().map(|_| bytes));
        });
    let reader = match reader {
        Ok(reader) => reader,
        Err(_) => {
            crate::tool::process::kill_process_group(Some(child.id()));
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
    };
    let deadline = Instant::now() + timeout;
    let output = loop {
        match receive.try_recv() {
            Ok(Some(bytes)) if bytes.len() as u64 <= MAX_OUTPUT => break Some(bytes),
            Ok(_) | Err(std::sync::mpsc::TryRecvError::Disconnected) => break None,
            Err(std::sync::mpsc::TryRecvError::Empty) => {}
        }
        if Instant::now() >= deadline {
            break None;
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    let status = if output.is_some() {
        loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                _ => break None,
            }
        }
    } else {
        None
    };
    if status.is_none() {
        crate::tool::process::kill_process_group(Some(child.id()));
        let _ = child.kill();
        let _ = child.wait();
    }
    // Never wait indefinitely on an escaped descendant holding the stdout pipe.
    if reader.is_finished() {
        let _ = reader.join();
    }
    if !status?.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output?).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_command(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
    args: &[&str],
) -> anyhow::Result<std::process::Command> {
    let git = crate::executable::resolve(
        services,
        "git",
        neoism_agent_service_api::ExecutablePurpose::VersionControl,
        "Git project discovery",
    )?;
    let mut command = std::process::Command::new(git);
    command.args(args).current_dir(directory);
    Ok(command)
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{Id, IdKind};

    fn services() -> neoism_agent_service_api::AgentServices {
        crate::standard_services()
    }

    #[test]
    fn project_discovery_honors_injected_git_and_reports_missing_git() {
        use crate::executable::test_support::FakeExecutableService;
        use std::sync::Arc;

        let injected = PathBuf::from("/injected/project-git");
        let mut services = services();
        services.executables = Arc::new(FakeExecutableService::with("git", &injected));
        let command = git_command(&services, Path::new("."), &["status"]).unwrap();
        assert_eq!(command.get_program(), injected.as_os_str());

        services.executables = Arc::new(FakeExecutableService::default());
        let error = git_command(&services, Path::new("."), &["status"])
            .unwrap_err()
            .to_string();
        assert!(error.contains("Git project discovery executable `git` is unavailable"));
        assert!(error.contains("install it"));
    }

    #[test]
    fn non_git_directory_uses_global_project() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-project-{}",
            Id::ascending(IdKind::Event)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let context = discover(&services(), &root);
        assert_eq!(context.info.id, "global");
        assert_eq!(context.path, None);

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn git_directory_uses_root_commit_as_project_id() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-project-git-{}",
            Id::ascending(IdKind::Event)
        ));
        let child = root.join("nested");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&child).unwrap();
        run_git(&root, &["init"]);
        std::fs::write(root.join("README.md"), "test").unwrap();
        run_git(&root, &["add", "README.md"]);
        run_git(
            &root,
            &[
                "-c",
                "user.name=Neoism Test",
                "-c",
                "user.email=neoism@example.invalid",
                "commit",
                "-m",
                "init",
            ],
        );
        let root_commit =
            git_output(&services(), &root, &["rev-list", "--max-parents=0", "HEAD"])
                .unwrap();

        let context = discover(&services(), &child);
        assert_eq!(context.info.id, root_commit);
        assert_eq!(context.path.as_deref(), Some("nested"));
        assert_eq!(
            context.info.worktree.as_deref(),
            Some(path_text(&root).as_str())
        );

        let _ = std::fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn slow_git_and_oversized_output_are_bounded() {
        let mut slow = std::process::Command::new("sh");
        slow.args(["-c", "sleep 30 & wait"]);
        let started = Instant::now();
        assert!(bounded_output(slow, Duration::from_millis(100)).is_none());
        assert!(started.elapsed() < Duration::from_secs(2));

        let pid_file = std::env::temp_dir()
            .join(format!("neoism-git-reap-{}", Id::ascending(IdKind::Event)));
        let mut direct = std::process::Command::new("sh");
        direct.args(["-c", "echo $$ > \"$1\"; exec sleep 30", "sh"]);
        direct.arg(&pid_file);
        assert!(bounded_output(direct, Duration::from_millis(200)).is_none());
        let pid: libc::pid_t = std::fs::read_to_string(&pid_file)
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        // A live child returns 0 and an unreaped zombie returns its pid.
        // ECHILD proves the timeout path has waited for this direct child.
        assert_eq!(
            unsafe { libc::waitpid(pid, std::ptr::null_mut(), libc::WNOHANG) },
            -1
        );
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ECHILD)
        );
        let _ = std::fs::remove_file(pid_file);

        let mut large = std::process::Command::new("sh");
        large.args(["-c", "yes x"]);
        assert!(bounded_output(large, Duration::from_secs(1)).is_none());
    }

    #[tokio::test]
    async fn unavailable_git_discovery_falls_back() {
        use crate::executable::test_support::FakeExecutableService;
        let mut services = services();
        services.executables = Arc::new(FakeExecutableService::default());
        let context = DiscoveryCache::default()
            .discover(&services, std::env::temp_dir())
            .await
            .unwrap();
        assert_eq!(context.info.id, "global");
        assert!(context.info.vcs.is_none());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn concurrent_discovery_runs_git_once_without_blocking_executor() {
        use crate::executable::test_support::FakeExecutableService;
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!(
            "neoism-discovery-flight-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let git = root.join("git");
        std::fs::write(&git, "#!/bin/sh\nprintf x >> calls\nsleep 0.2\nexit 1\n")
            .unwrap();
        std::fs::set_permissions(&git, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut services = services();
        services.executables = Arc::new(FakeExecutableService::with("git", &git));
        let cache = DiscoveryCache::default();
        let requests = (0..8).map(|_| cache.discover(&services, root.clone()));
        let all = futures::future::join_all(requests);
        tokio::pin!(all);
        // This test uses a single-thread executor: the timer must beat slow Git.
        tokio::select! {
            _ = &mut all => panic!("Git did not wait"),
            _ = tokio::time::sleep(Duration::from_millis(30)) => {}
        }
        for result in all.await {
            assert_eq!(result.unwrap().info.id, "global");
        }
        assert_eq!(std::fs::read_to_string(root.join("calls")).unwrap(), "x");
        {
            let cell = cache
                .entries
                .lock()
                .unwrap()
                .get(&canonicalize_lossy(&root))
                .unwrap()
                .clone();
            let mut cached = cell.lock().await;
            cached.as_mut().unwrap().0 = Instant::now() - Duration::from_secs(6);
        }
        cache.discover(&services, root.clone()).await.unwrap();
        assert_eq!(std::fs::read_to_string(root.join("calls")).unwrap(), "xx");
        let _ = std::fs::remove_dir_all(root);
    }

    fn run_git(directory: &Path, args: &[&str]) {
        let output = crate::windows_process::std_command("git")
            .args(args)
            .current_dir(directory)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
