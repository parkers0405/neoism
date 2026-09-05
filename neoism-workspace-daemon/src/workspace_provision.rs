//! Provision workspaces from git repositories for cloud deployments.
//!
//! The HTTP route owns auth and workspace registration; this module owns
//! deterministic path selection and the blocking `git` operations.

use std::path::{Path, PathBuf};
use std::process::Command;

use neoism_protocol::workspace::ProjectRootSummary;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Deserialize)]
pub struct GitWorkspaceRequest {
    pub git_url: String,
    #[serde(default, rename = "ref")]
    pub git_ref: Option<String>,
    /// Run `git fetch`/`git pull` when the workspace already exists.
    /// Defaults to true so reconnecting a cloud workspace keeps it fresh.
    #[serde(default = "default_pull")]
    pub pull: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct GitWorkspaceResponse {
    pub workspace: ProjectRootSummary,
    pub git_url: String,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub cloned: bool,
    pub reused: bool,
    pub updated: bool,
    pub path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ProvisionedPath {
    pub git_url: String,
    pub git_ref: Option<String>,
    pub path: PathBuf,
    pub cloned: bool,
    pub reused: bool,
    pub updated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ProvisionError {
    #[error("git_url is required")]
    MissingGitUrl,
    #[error("git_url contains unsupported characters")]
    InvalidGitUrl,
    #[error("workspace directory error: {0}")]
    Io(#[from] std::io::Error),
    #[error("git command failed: {0}")]
    Git(String),
}

const MARKER_DIR: &str = ".neoism";
const MARKER_FILE: &str = "provision.json";

fn default_pull() -> bool {
    true
}

pub fn workspaces_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("NEOISM_WORKSPACES_DIR") {
        return PathBuf::from(dir);
    }
    if let Ok(dir) = std::env::var("WORKSPACES_DIR") {
        return PathBuf::from(dir);
    }
    crate::auth::data_dir().join("workspaces")
}

pub fn provision_from_git(
    request: GitWorkspaceRequest,
    root: &Path,
) -> Result<ProvisionedPath, ProvisionError> {
    provision_from_git_with_depth(request, root, None)
}

/// Management-plane variant of the existing provisioner. Depth is bounded by
/// the public service contract and applies only to a fresh clone.
pub fn provision_from_git_with_depth(
    request: GitWorkspaceRequest,
    root: &Path,
    depth: Option<u32>,
) -> Result<ProvisionedPath, ProvisionError> {
    let git_url = request.git_url.trim().to_string();
    if git_url.is_empty() {
        return Err(ProvisionError::MissingGitUrl);
    }
    validate_git_url(&git_url)?;

    std::fs::create_dir_all(root)?;
    let slug = slug_for_git_url(&git_url);
    let path = root.join(slug);
    let exists = path.exists();
    let mut cloned = false;
    let mut updated = false;

    if exists {
        if request.pull {
            update_existing_repo(&path, request.git_ref.as_deref())?;
            updated = true;
        }
    } else {
        clone_repo(&git_url, &path, depth)?;
        cloned = true;
        if let Some(git_ref) = request.git_ref.as_deref() {
            checkout_ref(&path, git_ref)?;
        }
    }

    write_marker(&path, &git_url, request.git_ref.as_deref())?;

    Ok(ProvisionedPath {
        git_url,
        git_ref: request.git_ref,
        path,
        cloned,
        reused: exists,
        updated,
    })
}

pub fn slug_for_git_url(git_url: &str) -> String {
    let trimmed = git_url.trim().trim_end_matches('/');
    let last = trimmed
        .rsplit(['/', ':'])
        .next()
        .unwrap_or("workspace")
        .trim_end_matches(".git");
    let mut name = String::new();
    let mut previous_dash = false;
    for ch in last.chars() {
        let ch = ch.to_ascii_lowercase();
        if ch.is_ascii_alphanumeric() {
            name.push(ch);
            previous_dash = false;
        } else if !previous_dash {
            name.push('-');
            previous_dash = true;
        }
    }
    let name = name.trim_matches('-');
    let name = if name.is_empty() { "workspace" } else { name };
    format!("{name}-{}", short_hash(git_url))
}

fn validate_git_url(git_url: &str) -> Result<(), ProvisionError> {
    if git_url.contains('\0') || git_url.contains('\n') || git_url.contains('\r') {
        return Err(ProvisionError::InvalidGitUrl);
    }
    Ok(())
}

fn short_hash(value: &str) -> String {
    let digest = Sha256::digest(value.as_bytes());
    let mut out = String::with_capacity(10);
    for byte in &digest[..5] {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn clone_repo(
    git_url: &str,
    path: &Path,
    depth: Option<u32>,
) -> Result<(), ProvisionError> {
    let mut command = Command::new("git");
    #[cfg(windows)]
    crate::hide_std_command(&mut command);
    command.arg("clone");
    let depth_value = depth.map(|value| value.to_string());
    if let Some(value) = depth_value.as_deref() {
        command.args(["--depth", value]);
    }
    command.arg("--").arg(git_url).arg(path);
    let output = command.output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(ProvisionError::Git(command_error(&output)))
}

fn update_existing_repo(
    path: &Path,
    git_ref: Option<&str>,
) -> Result<(), ProvisionError> {
    run_git(["fetch", "--all", "--prune"], Some(path))?;
    if let Some(git_ref) = git_ref {
        checkout_ref(path, git_ref)?;
    }
    if git_ref.is_none() || has_upstream(path) {
        run_git(["pull", "--ff-only"], Some(path))?;
    }
    Ok(())
}

fn checkout_ref(path: &Path, git_ref: &str) -> Result<(), ProvisionError> {
    if git_ref.trim().is_empty() {
        return Ok(());
    }
    if git_ref.contains('\0') || git_ref.contains('\n') || git_ref.contains('\r') {
        return Err(ProvisionError::InvalidGitUrl);
    }
    run_git(["checkout", git_ref], Some(path))
}

fn run_git<const N: usize>(
    args: [&str; N],
    cwd: Option<&Path>,
) -> Result<(), ProvisionError> {
    let mut cmd = Command::new("git");
    #[cfg(windows)]
    crate::hide_std_command(&mut cmd);
    cmd.args(args);
    if let Some(cwd) = cwd {
        cmd.current_dir(cwd);
    }
    let output = cmd.output()?;
    if output.status.success() {
        return Ok(());
    }
    Err(ProvisionError::Git(command_error(&output)))
}

fn command_error(output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    format!(
        "{}{}",
        stderr.trim(),
        if stderr.trim().is_empty() {
            stdout.trim().to_string()
        } else if stdout.trim().is_empty() {
            String::new()
        } else {
            format!("; {}", stdout.trim())
        }
    )
}

fn has_upstream(path: &Path) -> bool {
    let mut command = Command::new("git");
    #[cfg(windows)]
    crate::hide_std_command(&mut command);
    command
        .args(["rev-parse", "--abbrev-ref", "--symbolic-full-name", "@{u}"])
        .current_dir(path)
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn write_marker(
    path: &Path,
    git_url: &str,
    git_ref: Option<&str>,
) -> Result<(), ProvisionError> {
    let marker_dir = path.join(MARKER_DIR);
    std::fs::create_dir_all(&marker_dir)?;
    let body = serde_json::json!({
        "kind": "git",
        "git_url": git_url,
        "ref": git_ref,
    });
    std::fs::write(
        marker_dir.join(MARKER_FILE),
        serde_json::to_vec_pretty(&body)
            .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err))?,
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn slug_is_stable_and_hides_url_punctuation() {
        let slug = slug_for_git_url("https://github.com/example/Neoism.App.git");
        assert!(slug.starts_with("neoism-app-"), "{slug}");
        assert!(slug
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-'));
        assert_eq!(
            slug,
            slug_for_git_url("https://github.com/example/Neoism.App.git")
        );
    }

    #[test]
    fn provision_reuses_local_repo_clone() {
        if Command::new("git").arg("--version").output().is_err() {
            return;
        }

        let source_dir = TempDir::new().unwrap();
        run_git(["init"], Some(source_dir.path())).unwrap();
        run_git(
            ["config", "user.email", "test@example.com"],
            Some(source_dir.path()),
        )
        .unwrap();
        run_git(
            ["config", "user.name", "Neoism Test"],
            Some(source_dir.path()),
        )
        .unwrap();
        std::fs::write(source_dir.path().join("README.md"), "hello\n").unwrap();
        run_git(["add", "README.md"], Some(source_dir.path())).unwrap();
        run_git(["commit", "-m", "initial"], Some(source_dir.path())).unwrap();

        let root = TempDir::new().unwrap();
        let git_url = source_dir.path().to_string_lossy().to_string();
        let first = provision_from_git(
            GitWorkspaceRequest {
                git_url: git_url.clone(),
                git_ref: None,
                pull: true,
            },
            root.path(),
        )
        .unwrap();
        assert!(first.cloned);
        assert!(!first.reused);
        assert!(first.path.join("README.md").exists());

        let second = provision_from_git(
            GitWorkspaceRequest {
                git_url,
                git_ref: None,
                pull: false,
            },
            root.path(),
        )
        .unwrap();
        assert!(!second.cloned);
        assert!(second.reused);
        assert!(!second.updated);
        assert_eq!(first.path, second.path);
    }
}
