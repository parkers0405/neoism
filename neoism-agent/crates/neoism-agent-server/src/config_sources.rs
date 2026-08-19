use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

pub(super) fn global_config_files() -> Vec<PathBuf> {
    let dir = PathBuf::from(crate::default_config_dir());
    ["config.json", "config.jsonc", "neoism.json", "neoism.jsonc"]
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
}

pub(super) fn project_config_files(
    directory: &Path,
    worktree: Option<&Path>,
) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for dir in ancestor_dirs(directory, worktree) {
        files.push(dir.join("neoism.json"));
        files.push(dir.join("neoism.jsonc"));
    }
    files
}

pub(super) fn config_files_in_dir(dir: &Path) -> Vec<PathBuf> {
    ["config.json", "config.jsonc", "neoism.json", "neoism.jsonc"]
        .into_iter()
        .map(|name| dir.join(name))
        .collect()
}

pub(super) fn config_directories(
    directory: &Path,
    worktree: Option<&Path>,
) -> Vec<PathBuf> {
    let mut seen = BTreeSet::new();
    let mut dirs = Vec::new();
    push_unique(
        &mut dirs,
        &mut seen,
        PathBuf::from(crate::default_config_dir()),
    );

    if !env_truthy("NEOISM_AGENT_DISABLE_PROJECT_CONFIG") {
        for dir in ancestor_dirs(directory, worktree) {
            let candidate = dir.join(".neoism");
            if candidate.is_dir() {
                push_unique(&mut dirs, &mut seen, candidate);
            }
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let candidate = PathBuf::from(home).join(".neoism");
        if candidate.is_dir() {
            push_unique(&mut dirs, &mut seen, candidate);
        }
    }

    if let Ok(dir) = std::env::var("NEOISM_AGENT_CONFIG_DIR") {
        push_unique(&mut dirs, &mut seen, PathBuf::from(dir));
    }
    dirs
}

fn push_unique(dirs: &mut Vec<PathBuf>, seen: &mut BTreeSet<PathBuf>, dir: PathBuf) {
    let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
    if seen.insert(key) {
        dirs.push(dir);
    }
}

fn ancestor_dirs(directory: &Path, stop: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = if directory.is_file() {
        directory.parent().unwrap_or(directory).to_path_buf()
    } else {
        directory.to_path_buf()
    };
    loop {
        dirs.push(current.clone());
        if stop.map(|stop| same_path(&current, stop)).unwrap_or(false) {
            break;
        }
        if !current.pop() {
            break;
        }
    }
    dirs.reverse();
    dirs
}

fn same_path(left: &Path, right: &Path) -> bool {
    left.canonicalize().unwrap_or_else(|_| left.to_path_buf())
        == right.canonicalize().unwrap_or_else(|_| right.to_path_buf())
}

pub(super) fn markdown_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if !root.is_dir() {
        return Ok(files);
    }
    collect_markdown(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_markdown(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_markdown(&path, files)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
            files.push(path);
        }
    }
    Ok(())
}

pub(super) fn entry_name(root: &Path, file: &Path) -> String {
    file.strip_prefix(root)
        .unwrap_or(file)
        .with_extension("")
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

pub(super) fn worktree_root(directory: &Path) -> Option<PathBuf> {
    let mut command = std::process::Command::new("git");
    command
        .args(["rev-parse", "--show-toplevel"])
        .current_dir(directory);
    crate::tool::process::set_new_process_group_std(&mut command);
    let output = command.output().ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then(|| PathBuf::from(text))
}

pub(super) fn absolute_path(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

pub(super) fn env_truthy(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "yes" | "YES"))
        .unwrap_or(false)
}
