use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use neoism_agent_core::{VcsApplyResult, VcsFileDiff, VcsFileStatus, VcsInfo};
use serde_json::{json, Value};

pub fn info(services: &neoism_agent_service_api::AgentServices, directory: &str) -> VcsInfo {
    VcsInfo {
        branch: git_output(services, directory, &["branch", "--show-current"]),
        default_branch: git_output(
            services,
            directory,
            &["symbolic-ref", "refs/remotes/origin/HEAD"],
        )
        .and_then(|value| value.rsplit('/').next().map(ToOwned::to_owned)),
    }
}

pub fn status(services: &neoism_agent_service_api::AgentServices, directory: &str) -> Vec<VcsFileStatus> {
    let Some(output) = git_output_raw(services, directory, &["status", "--porcelain=v1", "-z"])
    else {
        return Vec::new();
    };

    let mut stats = diff_stats(services, directory);
    let mut statuses = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&field[..2]).to_string();
        let path = String::from_utf8_lossy(&field[3..]).to_string();
        if code.starts_with('R') || code.starts_with('C') {
            fields.next();
        }
        let (additions, deletions) = stats.remove(&path).unwrap_or_else(|| {
            if normalize_status(&code) == "added" && is_untracked(services, directory, &path) {
                stat_untracked(directory, &path)
            } else {
                (0, 0)
            }
        });
        statuses.push(VcsFileStatus {
            file: path.clone(),
            path,
            status: normalize_status(&code).to_string(),
            additions,
            deletions,
        });
    }
    statuses.sort_by(|a, b| a.file.cmp(&b.file));
    statuses
}

pub fn diff(services: &neoism_agent_service_api::AgentServices, directory: &str) -> Vec<VcsFileDiff> {
    let statuses = status(services, directory);
    let mut stats = diff_stats(services, directory);
    statuses
        .into_iter()
        .filter_map(|file| {
            let patch = if file.status == "added" && is_untracked(services, directory, &file.path) {
                untracked_patch(directory, &file.path)
            } else {
                git_output(services, directory, &["diff", "HEAD", "--", &file.path])
            };
            let (added, removed) = stats.remove(&file.path).unwrap_or_else(|| {
                patch
                    .as_deref()
                    .map(count_patch_lines)
                    .unwrap_or((0_u64, 0_u64))
            });
            let patch = patch.unwrap_or_default();
            let hunks = (!patch.is_empty())
                .then(|| vec![json!({ "patch": patch.clone() })])
                .unwrap_or_default();
            Some(VcsFileDiff {
                file: file.path.clone(),
                path: file.path,
                status: file.status,
                added,
                removed,
                additions: added,
                deletions: removed,
                patch,
                hunks,
            })
        })
        .collect()
}

pub fn diff_raw(services: &neoism_agent_service_api::AgentServices, directory: &str) -> String {
    git_output(services, directory, &["diff", "HEAD"]).unwrap_or_default()
}

pub fn apply(services: &neoism_agent_service_api::AgentServices, directory: &str, patch: &str) -> VcsApplyResult {
    if patch.trim().is_empty() {
        return VcsApplyResult {
            success: false,
            error: Some("missing patch".to_string()),
        };
    }

    let git = match resolve_git(services) {
        Ok(git) => git,
        Err(error) => return VcsApplyResult { success: false, error: Some(error.to_string()) },
    };
    let mut child = match crate::windows_process::std_command(git)
        .args(["apply", "--whitespace=nowarn", "--"])
        .current_dir(directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            return VcsApplyResult {
                success: false,
                error: Some(format!("failed to start git apply: {error}")),
            }
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(error) = stdin.write_all(patch.as_bytes()) {
            return VcsApplyResult {
                success: false,
                error: Some(format!("failed to write patch to git apply: {error}")),
            };
        }
    }

    match child.wait_with_output() {
        Ok(output) if output.status.success() => VcsApplyResult {
            success: true,
            error: None,
        },
        Ok(output) => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            VcsApplyResult {
                success: false,
                error: Some(if stderr.is_empty() { stdout } else { stderr }),
            }
        }
        Err(error) => VcsApplyResult {
            success: false,
            error: Some(format!("failed to wait for git apply: {error}")),
        },
    }
}

fn diff_stats(services: &neoism_agent_service_api::AgentServices, directory: &str) -> BTreeMap<String, (u64, u64)> {
    let Some(output) = git_output(services, directory, &["diff", "--numstat", "HEAD"]) else {
        return BTreeMap::new();
    };
    output
        .lines()
        .filter_map(|line| {
            let mut parts = line.splitn(3, '\t');
            let added = parts.next()?.parse().ok()?;
            let removed = parts.next()?.parse().ok()?;
            let path = parts.next()?.to_string();
            Some((path, (added, removed)))
        })
        .collect()
}

fn git_output(services: &neoism_agent_service_api::AgentServices, directory: &str, args: &[&str]) -> Option<String> {
    let output = git_output_raw(services, directory, args)?;
    let text = String::from_utf8_lossy(&output).trim().to_string();
    (!text.is_empty()).then_some(text)
}

fn git_output_raw(services: &neoism_agent_service_api::AgentServices, directory: &str, args: &[&str]) -> Option<Vec<u8>> {
    let mut command = Command::new(resolve_git(services).ok()?);
    command.args(args).current_dir(directory);
    crate::tool::process::set_new_process_group_std(&mut command);
    let output = command.output().ok()?;
    output.status.success().then_some(output.stdout)
}

fn resolve_git(services: &neoism_agent_service_api::AgentServices) -> anyhow::Result<PathBuf> {
    crate::executable::resolve(
        services,
        "git",
        neoism_agent_service_api::ExecutablePurpose::VersionControl,
        "Git version-control",
    )
}

fn normalize_status(code: &str) -> &'static str {
    if code == "??" || code.contains('A') {
        "added"
    } else if code.contains('D') {
        "deleted"
    } else {
        "modified"
    }
}

fn is_untracked(services: &neoism_agent_service_api::AgentServices, directory: &str, path: &str) -> bool {
    git_output(
        services,
        directory,
        &["ls-files", "--others", "--exclude-standard", "--", path],
    )
    .map(|output| output.lines().any(|line| line == path))
    .unwrap_or(false)
}

fn untracked_patch(directory: &str, path: &str) -> Option<String> {
    let full_path = PathBuf::from(directory).join(path);
    let content = std::fs::read_to_string(&full_path).ok()?;
    let lines: Vec<&str> = content.lines().collect();
    let line_count = lines.len();
    let mut patch = format!(
        "diff --git a/{0} b/{0}\nnew file mode 100644\nindex 0000000..0000000\n--- /dev/null\n+++ b/{0}\n@@ -0,0 +1,{1} @@\n",
        patch_path(path),
        line_count
    );
    for line in lines {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    if content.ends_with('\n') {
        Some(patch)
    } else {
        patch.push_str("\\ No newline at end of file\n");
        Some(patch)
    }
}

fn stat_untracked(directory: &str, path: &str) -> (u64, u64) {
    let full_path = PathBuf::from(directory).join(path);
    let Ok(bytes) = std::fs::read(&full_path) else {
        return (0, 0);
    };
    if bytes.contains(&0) {
        return (0, 0);
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines = if text.is_empty() {
        0
    } else {
        text.lines().count() as u64
    };
    (lines, 0)
}

fn patch_path(path: &str) -> String {
    Path::new(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn count_patch_lines(patch: &str) -> (u64, u64) {
    let mut added = 0;
    let mut removed = 0;
    for line in patch.lines() {
        if line.starts_with("+++") || line.starts_with("---") {
            continue;
        }
        if line.starts_with('+') {
            added += 1;
        } else if line.starts_with('-') {
            removed += 1;
        }
    }
    (added, removed)
}

pub fn patch_from_body(body: &Value) -> Option<&str> {
    body.get("patch")
        .or_else(|| body.get("diff"))
        .or_else(|| body.get("content"))
        .and_then(Value::as_str)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    fn services() -> neoism_agent_service_api::AgentServices {
        crate::standard_services()
    }

    #[test]
    fn git_resolution_honors_injected_path_and_reports_missing_git() {
        use crate::executable::test_support::FakeExecutableService;
        use std::sync::Arc;

        let injected = PathBuf::from("/injected/tools/git");
        let mut services = services();
        services.executables = Arc::new(FakeExecutableService::with("git", &injected));
        assert_eq!(resolve_git(&services).unwrap(), injected);

        services.executables = Arc::new(FakeExecutableService::default());
        let error = resolve_git(&services).unwrap_err().to_string();
        assert!(error.contains("Git version-control executable `git` is unavailable"));
        assert!(error.contains("install it"));
    }

    struct TempRepo {
        path: PathBuf,
    }

    impl TempRepo {
        fn new() -> Self {
            let suffix = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_nanos();
            let path = std::env::temp_dir().join(format!(
                "neoism-agent-vcs-test-{}-{suffix}",
                std::process::id()
            ));
            fs::create_dir_all(&path).unwrap();
            run_git(&path, &["init"]);
            run_git(&path, &["config", "user.email", "test@example.com"]);
            run_git(&path, &["config", "user.name", "Test User"]);
            Self { path }
        }

        fn dir(&self) -> &str {
            self.path.to_str().unwrap()
        }
    }

    impl Drop for TempRepo {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.path);
        }
    }

    #[test]
    fn diff_includes_modified_file_patch_and_counts() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("file.txt"), "one\ntwo\n").unwrap();
        run_git(&repo.path, &["add", "file.txt"]);
        run_git(&repo.path, &["commit", "-m", "initial"]);

        fs::write(repo.path.join("file.txt"), "one\nthree\nfour\n").unwrap();

        let diffs = diff(&services(), repo.dir());
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "file.txt");
        assert_eq!(diffs[0].file, "file.txt");
        assert_eq!(diffs[0].status, "modified");
        assert_eq!(diffs[0].added, 2);
        assert_eq!(diffs[0].removed, 1);
        assert_eq!(diffs[0].additions, 2);
        assert_eq!(diffs[0].deletions, 1);
        assert!(diffs[0].patch.contains("+three"));
        assert!(diffs[0].hunks[0]["patch"]
            .as_str()
            .unwrap()
            .contains("+three"));
    }

    #[test]
    fn diff_includes_untracked_file_patch() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("new.txt"), "alpha\nbeta\n").unwrap();

        let diffs = diff(&services(), repo.dir());
        assert_eq!(diffs.len(), 1);
        assert_eq!(diffs[0].path, "new.txt");
        assert_eq!(diffs[0].file, "new.txt");
        assert_eq!(diffs[0].status, "added");
        assert_eq!(diffs[0].added, 2);
        assert_eq!(diffs[0].removed, 0);
        assert_eq!(diffs[0].additions, 2);
        assert_eq!(diffs[0].deletions, 0);
        let patch = &diffs[0].patch;
        assert!(patch.contains("new file mode"));
        assert!(patch.contains("+alpha"));
        assert_eq!(diffs[0].hunks[0]["patch"], diffs[0].patch);
    }

    #[test]
    fn status_includes_opencode_file_and_counts() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("file.txt"), "one\ntwo\n").unwrap();
        run_git(&repo.path, &["add", "file.txt"]);
        run_git(&repo.path, &["commit", "-m", "initial"]);

        fs::write(repo.path.join("file.txt"), "one\nthree\n").unwrap();
        fs::write(repo.path.join("new.txt"), "alpha\nbeta\n").unwrap();

        let statuses = status(&services(), repo.dir());
        let modified = statuses
            .iter()
            .find(|status| status.file == "file.txt")
            .unwrap();
        assert_eq!(modified.path, "file.txt");
        assert_eq!(modified.status, "modified");
        assert_eq!(modified.additions, 1);
        assert_eq!(modified.deletions, 1);

        let added = statuses
            .iter()
            .find(|status| status.file == "new.txt")
            .unwrap();
        assert_eq!(added.path, "new.txt");
        assert_eq!(added.status, "added");
        assert_eq!(added.additions, 2);
        assert_eq!(added.deletions, 0);
    }

    #[test]
    fn apply_reports_success_and_failure() {
        let repo = TempRepo::new();
        fs::write(repo.path.join("file.txt"), "one\n").unwrap();
        run_git(&repo.path, &["add", "file.txt"]);
        run_git(&repo.path, &["commit", "-m", "initial"]);

        let patch = "\
diff --git a/file.txt b/file.txt
index 5626abf..814f4a4 100644
--- a/file.txt
+++ b/file.txt
@@ -1 +1 @@
-one
+two
";
        let result = apply(&services(), repo.dir(), patch);
        assert!(result.success, "{:?}", result.error);
        assert_eq!(
            fs::read_to_string(repo.path.join("file.txt")).unwrap(),
            "two\n"
        );

        let result = apply(&services(), repo.dir(), patch);
        assert!(!result.success);
        assert!(result
            .error
            .as_deref()
            .unwrap_or_default()
            .contains("patch"));
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
