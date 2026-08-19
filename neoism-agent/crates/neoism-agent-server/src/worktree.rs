use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use serde_json::Value;

fn git_command() -> Command {
    crate::windows_process::std_command("git")
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeCreateRequest {
    #[serde(default, alias = "path", alias = "worktree")]
    pub directory: Option<String>,
    #[serde(default)]
    pub branch: Option<String>,
    #[serde(default, alias = "from", alias = "startPoint")]
    pub base: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreePathRequest {
    #[serde(default, alias = "path", alias = "worktree")]
    pub directory: Option<String>,
    #[serde(default)]
    pub force: bool,
    #[serde(default, alias = "removeUntracked")]
    pub clean: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WorktreeCreateResult {
    pub directory: String,
    pub branch: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct WorktreeEntry {
    path: PathBuf,
    branch: Option<String>,
    bare: bool,
    detached: bool,
}

pub(crate) fn list(directory: &str) -> Result<Vec<String>, String> {
    let root = git_root(directory)?;
    let entries = worktree_entries(directory)?;
    Ok(entries
        .into_iter()
        .filter(|entry| normalize_path(&entry.path) != normalize_path(&root))
        .map(|entry| path_text(&entry.path))
        .collect())
}

pub(crate) fn create(
    directory: &str,
    request: Option<WorktreeCreateRequest>,
) -> Result<WorktreeCreateResult, String> {
    let root = git_root(directory)?;
    let request = request.unwrap_or(WorktreeCreateRequest {
        directory: None,
        branch: None,
        base: None,
    });
    let branch = request.branch.unwrap_or_else(|| generated_branch(&root));
    let target = request
        .directory
        .map(|path| resolve_new_path(&root, &path))
        .unwrap_or_else(|| generated_path(&root, &branch));
    if target.exists() {
        return Err(format!(
            "worktree path already exists: {}",
            target.display()
        ));
    }
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create {}: {error}", parent.display()))?;
    }

    let base = request.base.unwrap_or_else(|| "HEAD".to_string());
    let branch_exists = branch_exists(&root, &branch);
    let mut args = vec!["worktree".to_string(), "add".to_string()];
    if !branch_exists {
        args.push("-b".to_string());
        args.push(branch.clone());
    }
    args.push(path_text(&target));
    args.push(if branch_exists { branch.clone() } else { base });
    run_git(&root, args.iter().map(String::as_str))?;

    Ok(WorktreeCreateResult {
        directory: path_text(&target),
        branch,
    })
}

pub(crate) fn remove(
    directory: &str,
    request: WorktreePathRequest,
) -> Result<bool, String> {
    let root = git_root(directory)?;
    let target = request
        .directory
        .as_deref()
        .map(|path| resolve_existingish_path(&root, path))
        .ok_or_else(|| "missing worktree directory".to_string())?;
    ensure_known_non_primary_worktree(&root, &target)?;

    let mut args = vec!["worktree", "remove"];
    if request.force {
        args.push("--force");
    }
    let target_text = path_text(&target);
    args.push(&target_text);
    run_git(&root, args)?;
    Ok(true)
}

pub(crate) fn reset(
    directory: &str,
    request: Option<WorktreePathRequest>,
) -> Result<bool, String> {
    let root = git_root(directory)?;
    let request = request.unwrap_or(WorktreePathRequest {
        directory: None,
        force: false,
        clean: false,
    });
    let target = request
        .directory
        .as_deref()
        .map(|path| resolve_existingish_path(&root, path))
        .unwrap_or(root);
    ensure_git_worktree(&target)?;

    if has_unmerged_paths(&target)? && !request.force {
        return Err(
            "worktree has unresolved conflicts; pass force to reset anyway".to_string(),
        );
    }

    run_git(&target, ["reset", "--hard", "HEAD"])?;
    if request.clean {
        run_git(&target, ["clean", "-fd"])?;
    }
    Ok(true)
}

pub(crate) fn create_request_from_value(
    value: Option<Value>,
) -> Option<WorktreeCreateRequest> {
    value.and_then(|value| serde_json::from_value(value).ok())
}

pub(crate) fn path_request_from_value(
    value: Option<Value>,
) -> Option<WorktreePathRequest> {
    value.and_then(|value| serde_json::from_value(value).ok())
}

fn worktree_entries(directory: impl AsRef<Path>) -> Result<Vec<WorktreeEntry>, String> {
    let output = run_git_output(directory, ["worktree", "list", "--porcelain", "-z"])?;
    let mut entries = Vec::new();
    let mut current: Option<WorktreeEntry> = None;
    for field in output.split('\0') {
        if field.is_empty() {
            if let Some(entry) = current.take() {
                entries.push(entry);
            }
            continue;
        }
        if let Some(path) = field.strip_prefix("worktree ") {
            if let Some(entry) = current.replace(WorktreeEntry {
                path: PathBuf::from(path),
                branch: None,
                bare: false,
                detached: false,
            }) {
                entries.push(entry);
            }
            continue;
        }
        let Some(entry) = current.as_mut() else {
            continue;
        };
        if let Some(branch) = field.strip_prefix("branch ") {
            entry.branch = branch
                .strip_prefix("refs/heads/")
                .or(Some(branch))
                .map(ToOwned::to_owned);
        } else if field == "bare" {
            entry.bare = true;
        } else if field == "detached" {
            entry.detached = true;
        }
    }
    if let Some(entry) = current {
        entries.push(entry);
    }
    Ok(entries)
}

fn ensure_known_non_primary_worktree(root: &Path, target: &Path) -> Result<(), String> {
    let entries = worktree_entries(root)?;
    let root = normalize_path(root);
    let target = normalize_path(target);
    if target == root {
        return Err("refusing to remove the primary worktree".to_string());
    }
    if entries
        .iter()
        .any(|entry| normalize_path(&entry.path) == target)
    {
        return Ok(());
    }
    Err(format!("unknown git worktree: {}", target.display()))
}

fn ensure_git_worktree(target: &Path) -> Result<(), String> {
    git_root(target).map(|_| ())
}

fn git_root(directory: impl AsRef<Path>) -> Result<PathBuf, String> {
    let output = run_git_output(directory, ["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(output.trim()))
}

fn branch_exists(root: &Path, branch: &str) -> bool {
    crate::windows_process::std_command("git")
        .args(["show-ref", "--verify", "--quiet"])
        .arg(format!("refs/heads/{branch}"))
        .current_dir(root)
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn has_unmerged_paths(directory: &Path) -> Result<bool, String> {
    let output = run_git_output(directory, ["status", "--porcelain=v1", "-z"])?;
    for field in output.split('\0').filter(|field| !field.is_empty()) {
        let code = field.as_bytes().get(..2).unwrap_or_default();
        if matches!(code, b"DD" | b"AU" | b"UD" | b"UA" | b"DU" | b"AA" | b"UU") {
            return Ok(true);
        }
    }
    Ok(false)
}

fn generated_branch(root: &Path) -> String {
    let base = run_git_output(root, ["branch", "--show-current"])
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "worktree".to_string());
    format!("neoism/{}-{}", slug(&base), unique_suffix())
}

fn generated_path(root: &Path, branch: &str) -> PathBuf {
    let parent = root.parent().unwrap_or_else(|| Path::new("."));
    let name = root
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("worktree");
    parent.join(format!("{name}-{}", slug(branch)))
}

fn resolve_new_path(root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        root.join(path)
    }
}

fn resolve_existingish_path(root: &Path, path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    let path = if path.is_absolute() {
        path
    } else {
        root.join(path)
    };
    normalize_path(&path)
}

fn normalize_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn path_text(path: &Path) -> String {
    path.display().to_string()
}

fn slug(value: &str) -> String {
    let slug = value
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '-' || character == '_' {
                character
            } else {
                '-'
            }
        })
        .collect::<String>()
        .trim_matches('-')
        .to_string();
    if slug.is_empty() {
        "worktree".to_string()
    } else {
        slug
    }
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
}

fn run_git<'a>(
    directory: impl AsRef<Path>,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<(), String> {
    let output = git_command()
        .args(args)
        .current_dir(directory.as_ref())
        .output()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if output.status.success() {
        return Ok(());
    }
    Err(git_error(output))
}

fn run_git_output<'a>(
    directory: impl AsRef<Path>,
    args: impl IntoIterator<Item = &'a str>,
) -> Result<String, String> {
    let output = git_command()
        .args(args)
        .current_dir(directory.as_ref())
        .output()
        .map_err(|error| format!("failed to start git: {error}"))?;
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).trim().to_string());
    }
    Err(git_error(output))
}

fn git_error(output: std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if stderr.is_empty() {
        stdout
    } else {
        stderr
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;

    struct TempRepo {
        base: PathBuf,
        root: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            let suffix = unique_suffix();
            let base = std::env::temp_dir().join(format!(
                "neoism-agent-worktree-test-{}-{suffix}",
                std::process::id()
            ));
            let root = base.join("repo");
            fs::create_dir_all(&root).unwrap();
            run_git(&root, ["init"]).unwrap();
            run_git(&root, ["config", "user.email", "test@example.com"]).unwrap();
            run_git(&root, ["config", "user.name", "Test User"]).unwrap();
            fs::write(root.join("file.txt"), "one\n").unwrap();
            run_git(&root, ["add", "file.txt"]).unwrap();
            run_git(&root, ["commit", "-m", "initial"]).unwrap();
            Self { base, root }
        }

        fn dir(&self) -> &str {
            self.root.to_str().unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.base);
        }
    }

    #[test]
    fn create_lists_and_removes_worktree() {
        let repo = TempRepo::new();
        let target = repo.base.join("feature-worktree");
        let created = create(
            repo.dir(),
            Some(WorktreeCreateRequest {
                directory: Some(path_text(&target)),
                branch: Some("feature/test".to_string()),
                base: None,
            }),
        )
        .unwrap();

        assert_eq!(created.directory, path_text(&target));
        assert_eq!(created.branch, "feature/test");
        assert!(target.join("file.txt").exists());
        assert_eq!(list(repo.dir()).unwrap(), vec![path_text(&target)]);

        assert!(remove(
            repo.dir(),
            WorktreePathRequest {
                directory: Some(path_text(&target)),
                force: false,
                clean: false,
            },
        )
        .unwrap());
        assert!(list(repo.dir()).unwrap().is_empty());
    }

    #[test]
    fn remove_refuses_primary_worktree() {
        let repo = TempRepo::new();
        let error = remove(
            repo.dir(),
            WorktreePathRequest {
                directory: Some(repo.dir().to_string()),
                force: false,
                clean: false,
            },
        )
        .unwrap_err();
        assert!(error.contains("primary worktree"));
    }

    #[test]
    fn reset_discards_tracked_changes_and_keeps_untracked_by_default() {
        let repo = TempRepo::new();
        fs::write(repo.root.join("file.txt"), "changed\n").unwrap();
        fs::write(repo.root.join("scratch.txt"), "keep\n").unwrap();

        assert!(reset(repo.dir(), None).unwrap());

        assert_eq!(
            fs::read_to_string(repo.root.join("file.txt")).unwrap(),
            "one\n"
        );
        assert!(repo.root.join("scratch.txt").exists());
    }

    #[test]
    fn reset_cleans_untracked_when_requested() {
        let repo = TempRepo::new();
        fs::write(repo.root.join("scratch.txt"), "remove\n").unwrap();

        assert!(reset(
            repo.dir(),
            Some(WorktreePathRequest {
                directory: None,
                force: false,
                clean: true,
            }),
        )
        .unwrap());

        assert!(!repo.root.join("scratch.txt").exists());
    }

    #[test]
    fn reset_refuses_unmerged_paths_without_force() {
        let repo = TempRepo::new();
        let default_branch =
            run_git_output(&repo.root, ["branch", "--show-current"]).unwrap();
        run_git(&repo.root, ["checkout", "-b", "conflict-side"]).unwrap();
        fs::write(repo.root.join("file.txt"), "feature\n").unwrap();
        run_git(&repo.root, ["add", "file.txt"]).unwrap();
        run_git(&repo.root, ["commit", "-m", "feature"]).unwrap();
        run_git(&repo.root, ["checkout", &default_branch]).unwrap();
        fs::write(repo.root.join("file.txt"), "main\n").unwrap();
        run_git(&repo.root, ["add", "file.txt"]).unwrap();
        run_git(&repo.root, ["commit", "-m", "main"]).unwrap();

        let merge = git_command()
            .args(["merge", "conflict-side"])
            .current_dir(&repo.root)
            .output()
            .unwrap();
        assert!(!merge.status.success());

        let error = reset(repo.dir(), None).unwrap_err();
        assert!(error.contains("unresolved conflicts"));

        assert!(reset(
            repo.dir(),
            Some(WorktreePathRequest {
                directory: None,
                force: true,
                clean: false,
            }),
        )
        .unwrap());
    }
}
