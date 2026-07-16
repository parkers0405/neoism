//! Shell-out operations: Neoism-notes note create, git-backed workspace
//! snapshot export, and the local Docker sandbox helpers. Pure code-move
//! out of the former monolithic `workspace.rs`.

use super::*;

/// Resolve the notes vault a note created from `root` should land in,
/// mirroring the desktop's `notes_workspace_for_root_or_default`: an
/// explicitly linked project vault wins, then a directory-local
/// workspace config, and otherwise the global Default vault
/// (`~/Neoism/Vaults/Default`). Never initializes `root` itself.
fn notes_workspace_for_root(
    root: &Path,
) -> neoism_workspace_index::config::NeoismWorkspace {
    neoism_workspace_index::linked_project_for_code_dir(root)
        .ok()
        .flatten()
        .or_else(|| neoism_workspace_index::load_workspace(root).ok().flatten())
        .filter(|workspace| workspace.config.notes.enabled)
        .unwrap_or_else(neoism_workspace_index::default_notes_workspace)
}

pub(crate) fn create_neoism_note(root: &Path) -> Result<PathBuf, String> {
    let workspace = notes_workspace_for_root(root);
    let notes_dir = workspace.notes_workspace_dir();
    std::fs::create_dir_all(&notes_dir)
        .map_err(|e| format!("failed to create {}: {e}", notes_dir.display()))?;
    let target = unique_note_path(&notes_dir)?;
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&target)
        .map_err(|e| format!("failed to create {}: {e}", target.display()))?;
    Ok(target)
}

pub(crate) fn export_workspace_snapshot(
    manager: &WorkspaceManager,
    workspace_id: &str,
) -> Result<PathBuf, String> {
    let workspace = manager
        .get_host_workspace(workspace_id)
        .ok_or_else(|| format!("no such host workspace: {workspace_id}"))?;
    let root = workspace
        .root_dir
        .clone()
        .ok_or_else(|| format!("workspace {} has no root_dir", workspace.title))?;
    if !root.is_dir() {
        return Err(format!(
            "workspace root is not a directory: {}",
            root.display()
        ));
    }
    let base = std::env::var("NEOISM_SNAPSHOT_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            persistence_path()
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("snapshots")
        });
    let snapshot_id = format!("{}-{}", workspace.id, now_secs());
    let dir = base.join(&snapshot_id);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("failed to create snapshot dir {}: {e}", dir.display()))?;

    let commit = git_output(&root, ["rev-parse", "HEAD"]).unwrap_or_default();
    let branch =
        git_output(&root, ["rev-parse", "--abbrev-ref", "HEAD"]).unwrap_or_default();
    let remote = git_output(&root, ["remote", "get-url", "origin"]).unwrap_or_default();
    let snapshot = crate::workspace_snapshot::capture_uncommitted(&root)
        .map_err(|e| format!("failed to capture workspace snapshot: {e}"))?;
    std::fs::write(
        dir.join("workspace-snapshot.json"),
        serde_json::to_vec_pretty(&snapshot)
            .map_err(|e| format!("failed to encode workspace snapshot: {e}"))?,
    )
    .map_err(|e| format!("failed to write workspace-snapshot.json: {e}"))?;
    if !snapshot.tracked_patch.is_empty() {
        std::fs::write(dir.join("dirty.patch"), snapshot.tracked_patch.as_bytes())
            .map_err(|e| format!("failed to write dirty.patch: {e}"))?;
    }
    let untracked = snapshot
        .untracked
        .iter()
        .map(|(path, _)| path.display().to_string())
        .collect::<Vec<_>>();
    let tabs = manager
        .list_workspace_tabs(workspace_id)
        .into_iter()
        .map(|tab| {
            serde_json::json!({
                "id": tab.id,
                "title": tab.title,
                "kind": tab.kind,
                "surface_id": tab.surface_id,
                "cwd": tab.cwd,
                "active": tab.active,
            })
        })
        .collect::<Vec<_>>();
    let manifest = serde_json::json!({
        "snapshot_id": snapshot_id,
        "workspace_id": workspace.id,
        "title": workspace.title,
        "root_dir": root,
        "base": {
            "type": "git",
            "remote": remote,
            "branch": branch,
            "commit": commit,
        },
        "untracked": untracked,
        "snapshot_file": "workspace-snapshot.json",
        "metadata": {
            "host_kind": workspace.host_kind,
            "visibility": workspace.visibility,
            "main_session_id": workspace.main_session_id,
            "tabs": tabs,
        }
    });
    std::fs::write(
        dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)
            .map_err(|e| format!("failed to encode manifest: {e}"))?,
    )
    .map_err(|e| format!("failed to write manifest: {e}"))?;
    Ok(dir)
}

fn git_output<const N: usize>(root: &Path, args: [&str; N]) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .map_err(|e| format!("failed to run git in {}: {e}", root.display()))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[derive(Debug, Clone)]
pub(crate) struct LocalDockerSandbox {
    pub url: String,
    pub container: String,
    pub volume: String,
}

pub(crate) fn start_local_docker_sandbox(
    workspace_id: &str,
) -> Result<LocalDockerSandbox, String> {
    let image = std::env::var("NEOISM_DOCKER_SANDBOX_IMAGE")
        .unwrap_or_else(|_| "neoism-workspace-daemon:latest".to_string());
    let name = format!(
        "neoism-sandbox-{}-{}",
        workspace_id
            .chars()
            .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
            .take(24)
            .collect::<String>(),
        now_secs()
    );
    let volume = format!("{name}-data");
    run_docker(["volume", "create", volume.as_str()])?;
    let volume_mount = format!("{volume}:/var/lib/neoism");
    let output = std::process::Command::new("docker")
        .args([
            "run",
            "-d",
            "--name",
            name.as_str(),
            "-p",
            "127.0.0.1::9876",
            "-v",
            volume_mount.as_str(),
            image.as_str(),
        ])
        .output()
        .map_err(|e| format!("failed to run docker: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let port = docker_mapped_port(&name)?;
    let url = format!("http://127.0.0.1:{port}");
    wait_for_docker_daemon_health(&url)?;
    Ok(LocalDockerSandbox {
        url,
        container: name,
        volume,
    })
}

pub(crate) fn cleanup_local_docker_sandbox(sandbox: &LocalDockerSandbox) {
    let _ = std::process::Command::new("docker")
        .args(["rm", "-f", sandbox.container.as_str()])
        .output();
    let _ = std::process::Command::new("docker")
        .args(["volume", "rm", "-f", sandbox.volume.as_str()])
        .output();
}

fn run_docker<const N: usize>(args: [&str; N]) -> Result<(), String> {
    let output = std::process::Command::new("docker")
        .args(args)
        .output()
        .map_err(|e| format!("failed to run docker: {e}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

fn docker_mapped_port(container: &str) -> Result<String, String> {
    let output = std::process::Command::new("docker")
        .args(["port", container, "9876/tcp"])
        .output()
        .map_err(|e| format!("failed to inspect docker port: {e}"))?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    text.trim()
        .rsplit_once(':')
        .map(|(_, port)| port.to_string())
        .filter(|port| !port.is_empty())
        .ok_or_else(|| format!("docker did not report a mapped port for {container}"))
}

fn wait_for_docker_daemon_health(base_url: &str) -> Result<(), String> {
    let health_url = format!("{}/health", base_url.trim_end_matches('/'));
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
    loop {
        let ok = std::process::Command::new("curl")
            .args(["-fsS", "--max-time", "2", health_url.as_str()])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
        if ok {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(format!(
                "docker sandbox daemon did not become healthy at {health_url}"
            ));
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn unique_note_path(dir: &Path) -> Result<PathBuf, String> {
    for index in 1..=999 {
        let name = if index == 1 {
            "Note.md".to_string()
        } else {
            format!("Note {index}.md")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err(format!("No available note filename in {}", dir.display()))
}
