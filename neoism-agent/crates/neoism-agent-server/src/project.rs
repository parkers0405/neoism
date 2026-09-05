use std::path::{Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::ProjectInfo;

pub(crate) struct ProjectContext {
    pub(crate) info: ProjectInfo,
    pub(crate) directory: String,
    pub(crate) path: Option<String>,
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
    if let Some(cached) = read_cached_id(services, directory) {
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
    let _ = write_cached_id(services, directory, &id);
    Some(id)
}

fn read_cached_id(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
) -> Option<String> {
    let path = cache_path(services, directory)?;
    std::fs::read_to_string(path)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn write_cached_id(
    services: &neoism_agent_service_api::AgentServices,
    directory: &Path,
    id: &str,
) -> anyhow::Result<()> {
    let Some(path) = cache_path(services, directory) else {
        return Ok(());
    };
    std::fs::write(path, id).context("failed to cache project id")
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
    let mut command = git_command(services, directory, args).ok()?;
    crate::tool::process::set_new_process_group_std(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
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
