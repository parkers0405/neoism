//! Async handlers for [`FilesClientMessage`].
//!
//! Workspace root is resolved from `NEOISM_WORKSPACE_ROOT`, falling back to
//! the daemon's current working directory. Every incoming path is treated as
//! workspace-relative and validated against traversal:
//!
//! * absolute paths are rejected,
//! * any `..` component is rejected,
//! * Windows-style prefixes / root-dir components (e.g. `\`) are rejected.
//!
//! Rejections produce a `FilesServerMessage::Error` instead of touching the
//! filesystem.

use std::path::{Component, Path, PathBuf};

use futures::{stream, StreamExt};
use neoism_protocol::files::{
    DirEntry, FilesClientMessage, FilesServerMessage, TreeEntry,
};
use tokio::fs;
use walkdir::WalkDir;

/// Resolve the workspace root from `NEOISM_WORKSPACE_ROOT` or CWD.
pub fn workspace_root() -> PathBuf {
    if let Ok(root) = std::env::var("NEOISM_WORKSPACE_ROOT") {
        if !root.is_empty() {
            return PathBuf::from(root);
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
}

/// Validate `path` is a safe workspace-relative path and return its
/// fully-resolved absolute form rooted under `root`.
///
/// Returns `Err(reason)` if the path is absolute, contains `..`, or contains
/// any other non-normal component (root dir, prefix). The empty string maps
/// to `root` itself (useful for listing the workspace root).
pub fn resolve_path(root: &Path, path: &str) -> Result<PathBuf, String> {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        // Tolerate an absolute path that already sits INSIDE `root`. Some
        // clients (the web file tree) hand back absolute tree paths, and a
        // root/subdir mismatch otherwise surfaced as "absolute paths not
        // allowed" file-open failures. This stays confined to `root` — a
        // path with any `..` component, or one outside the root, is still
        // rejected, so it grants no traversal the relative form wouldn't.
        let has_parent = candidate
            .components()
            .any(|c| matches!(c, Component::ParentDir));
        if !has_parent && candidate.starts_with(root) {
            return Ok(candidate.to_path_buf());
        }
        return Err(format!("absolute paths are not allowed: {path}"));
    }
    for component in candidate.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                return Err(format!("path traversal (`..`) is not allowed: {path}"));
            }
            Component::Prefix(_) | Component::RootDir => {
                return Err(format!("invalid path component in: {path}"));
            }
        }
    }
    Ok(root.join(candidate))
}

fn err(msg: impl Into<String>) -> Vec<FilesServerMessage> {
    vec![FilesServerMessage::Error {
        message: msg.into(),
    }]
}

/// Dispatch a single client files message.
pub async fn handle(msg: FilesClientMessage) -> Vec<FilesServerMessage> {
    let root = workspace_root();
    handle_with_root(&root, msg).await
}

/// Dispatch a single client files message against an explicit workspace root.
pub async fn handle_with_root(
    root: &Path,
    msg: FilesClientMessage,
) -> Vec<FilesServerMessage> {
    match msg {
        FilesClientMessage::ListDir { path } => list_dir(root, path).await,
        FilesClientMessage::Stat { path } => stat(root, path).await,
        FilesClientMessage::ReadFile { path } => read_file(root, path).await,
        FilesClientMessage::WriteFile { path, bytes } => {
            write_file(root, path, bytes).await
        }
        FilesClientMessage::WalkTree { path, max_depth } => {
            walk_tree(root, path, max_depth).await
        }
        FilesClientMessage::CreateFile { dir, name } => {
            create_file(root, dir, name).await
        }
        FilesClientMessage::CreateDir { dir, name } => create_dir(root, dir, name).await,
        FilesClientMessage::Rename { from, to } => rename(root, from, to).await,
        FilesClientMessage::Delete { path } => delete(root, path).await,
        FilesClientMessage::ReadShellHistory { max_entries } => {
            read_shell_history(max_entries.unwrap_or(500) as usize).await
        }
    }
}

/// Read the daemon user's shell history for the web composer's
/// ArrowUp recall — desktop parity, where the composer loads
/// `~/.zsh_history` directly. Zsh extended-format lines
/// (`: <ts>:<dur>;cmd`) are stripped to the bare command. On Windows
/// the PowerShell PSReadLine history (plain lines) is read instead.
async fn read_shell_history(max_entries: usize) -> Vec<FilesServerMessage> {
    let result = tokio::task::spawn_blocking(move || {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .or_else(dirs::home_dir)?;
        #[cfg_attr(not(windows), allow(unused_mut))]
        let mut candidates = vec![
            std::env::var_os("HISTFILE").map(std::path::PathBuf::from),
            Some(home.join(".zsh_history")),
            Some(home.join(".bash_history")),
        ];
        #[cfg(windows)]
        candidates.push(dirs::data_dir().map(|appdata| {
            appdata
                .join("Microsoft")
                .join("Windows")
                .join("PowerShell")
                .join("PSReadLine")
                .join("ConsoleHost_history.txt")
        }));
        for path in candidates.into_iter().flatten() {
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            let text = String::from_utf8_lossy(&bytes);
            let mut entries: Vec<String> = Vec::new();
            for line in text.lines() {
                // Zsh extended history: `: 1700000000:0;the command`.
                let command = if let Some(rest) = line.strip_prefix(": ") {
                    match rest.split_once(';') {
                        Some((_meta, cmd)) => cmd,
                        None => continue,
                    }
                } else {
                    line
                };
                // Bash stores continuations with a trailing `\`; on Windows
                // a trailing backslash is a real path char (PSReadLine
                // continues with a backtick), so don't strip it there.
                #[cfg(not(windows))]
                let command = command.trim_end_matches('\\');
                let command = command.trim();
                if command.is_empty() {
                    continue;
                }
                // Collapse consecutive duplicates like shells do.
                if entries.last().map(String::as_str) != Some(command) {
                    entries.push(command.to_string());
                }
            }
            let start = entries.len().saturating_sub(max_entries.max(1));
            return Some(entries.split_off(start));
        }
        None
    })
    .await;
    match result {
        Ok(Some(entries)) => vec![FilesServerMessage::ShellHistory { entries }],
        Ok(None) => vec![FilesServerMessage::ShellHistory {
            entries: Vec::new(),
        }],
        Err(e) => err(format!("shell history read failed: {e}")),
    }
}

fn join_rel(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        name.to_string()
    } else if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

fn validate_name(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("name cannot be empty".into());
    }
    if name.contains('\0') {
        return Err("name contains nul byte".into());
    }
    Ok(())
}

async fn create_file(root: &Path, dir: String, name: String) -> Vec<FilesServerMessage> {
    if let Err(e) = validate_name(&name) {
        return err(e);
    }
    let rel = join_rel(&dir, &name);
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    if let Some(parent) = resolved.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return err(format!("create_dir_all {rel}: {e}"));
            }
        }
    }
    match fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&resolved)
        .await
    {
        Ok(_) => vec![FilesServerMessage::FileCreated {
            path: rel,
            is_dir: false,
        }],
        Err(e) => err(format!("create_file {rel}: {e}")),
    }
}

async fn create_dir(root: &Path, dir: String, name: String) -> Vec<FilesServerMessage> {
    if let Err(e) = validate_name(&name) {
        return err(e);
    }
    let rel = join_rel(&dir, &name);
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    match fs::create_dir_all(&resolved).await {
        Ok(()) => vec![FilesServerMessage::FileCreated {
            path: rel,
            is_dir: true,
        }],
        Err(e) => err(format!("create_dir {rel}: {e}")),
    }
}

async fn rename(root: &Path, from: String, to: String) -> Vec<FilesServerMessage> {
    let src = match resolve_path(root, &from) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let dst = match resolve_path(root, &to) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    if let Some(parent) = dst.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return err(format!("create_dir_all {to}: {e}"));
            }
        }
    }
    match fs::rename(&src, &dst).await {
        Ok(()) => vec![FilesServerMessage::Renamed { from, to }],
        Err(e) => err(format!("rename {from} -> {to}: {e}")),
    }
}

async fn delete(root: &Path, rel: String) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    let was_dir = match fs::metadata(&resolved).await {
        Ok(md) => md.is_dir(),
        Err(e) => return err(format!("delete {rel}: {e}")),
    };
    let result = if was_dir {
        fs::remove_dir_all(&resolved).await
    } else {
        fs::remove_file(&resolved).await
    };
    match result {
        Ok(()) => vec![FilesServerMessage::Deleted { path: rel, was_dir }],
        Err(e) => err(format!("delete {rel}: {e}")),
    }
}

async fn list_dir(root: &Path, rel: String) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    let mut read_dir = match fs::read_dir(&resolved).await {
        Ok(rd) => rd,
        Err(e) => return err(format!("read_dir {rel}: {e}")),
    };

    let mut raw_entries = Vec::new();
    loop {
        match read_dir.next_entry().await {
            Ok(Some(entry)) => raw_entries.push(entry),
            Ok(None) => break,
            Err(e) => return err(format!("read_dir {rel}: {e}")),
        }
    }

    // Metadata used to be awaited serially for every child. A directory with
    // hundreds of entries therefore multiplied filesystem latency before a
    // joined client could paint even one row. Keep the wire contract (file
    // sizes are still populated), but resolve a bounded batch concurrently.
    const METADATA_CONCURRENCY: usize = 32;
    let mut entries = stream::iter(raw_entries)
        .map(|entry| async move {
            let name = entry.file_name().to_string_lossy().into_owned();
            let (is_dir, size) = match entry.metadata().await {
                Ok(md) => {
                    let is_dir = md.is_dir();
                    let size = md.is_file().then(|| md.len());
                    (is_dir, size)
                }
                Err(_) => (false, None),
            };
            // Markdown page icon, read HERE because the browser client
            // has no filesystem to read frontmatter from.
            let icon = if is_dir {
                None
            } else {
                markdown_frontmatter_icon(&entry.path()).await
            };
            DirEntry {
                name,
                is_dir,
                size,
                icon,
            }
        })
        .buffer_unordered(METADATA_CONCURRENCY)
        .collect::<Vec<_>>()
        .await;

    entries.sort_by(|a, b| a.name.cmp(&b.name));
    vec![FilesServerMessage::DirListing { path: rel, entries }]
}

async fn stat(root: &Path, rel: String) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    match fs::metadata(&resolved).await {
        Ok(md) => {
            let name = Path::new(&rel)
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| ".".into());
            let is_dir = md.is_dir();
            let size = if md.is_file() { Some(md.len()) } else { None };
            vec![FilesServerMessage::Stat {
                path: rel,
                entry: DirEntry {
                    name,
                    is_dir,
                    size,
                    icon: None,
                },
            }]
        }
        Err(e) => err(format!("stat {rel}: {e}")),
    }
}

async fn read_file(root: &Path, rel: String) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    match fs::read(&resolved).await {
        Ok(bytes) => vec![FilesServerMessage::FileContent { path: rel, bytes }],
        Err(e) => err(format!("read_file {rel}: {e}")),
    }
}

async fn write_file(root: &Path, rel: String, bytes: Vec<u8>) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };
    // Ensure the parent directory exists so we can create new files under
    // sub-paths without the caller doing it manually.
    if let Some(parent) = resolved.parent() {
        if !parent.as_os_str().is_empty() {
            if let Err(e) = fs::create_dir_all(parent).await {
                return err(format!("create_dir_all {rel}: {e}"));
            }
        }
    }
    let len = bytes.len();
    match fs::write(&resolved, &bytes).await {
        Ok(()) => vec![FilesServerMessage::FileWritten {
            path: rel,
            bytes_written: len,
        }],
        Err(e) => err(format!("write_file {rel}: {e}")),
    }
}

async fn walk_tree(
    root: &Path,
    rel: String,
    max_depth: Option<u32>,
) -> Vec<FilesServerMessage> {
    let resolved = match resolve_path(root, &rel) {
        Ok(p) => p,
        Err(e) => return err(e),
    };

    let resolved_for_blocking = resolved.clone();
    let depth_cap = max_depth;
    let rel_for_blocking = rel.clone();

    // walkdir is sync; run it on the blocking pool so we don't stall the
    // tokio reactor on large trees.
    let result = tokio::task::spawn_blocking(move || {
        let mut walker = WalkDir::new(&resolved_for_blocking).min_depth(1);
        if let Some(d) = depth_cap {
            walker = walker.max_depth(d as usize);
        }
        let mut entries = Vec::new();
        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(e) => return Err(format!("walk_tree {rel_for_blocking}: {e}")),
            };
            let rel_path = match entry.path().strip_prefix(&resolved_for_blocking) {
                Ok(p) => p.to_string_lossy().into_owned(),
                Err(_) => continue,
            };
            entries.push(TreeEntry {
                path: rel_path,
                is_dir: entry.file_type().is_dir(),
                depth: entry.depth() as u32,
            });
        }
        Ok(entries)
    })
    .await;

    match result {
        Ok(Ok(entries)) => vec![FilesServerMessage::TreeListing { path: rel, entries }],
        Ok(Err(e)) => err(e),
        Err(e) => err(format!("walk_tree {rel}: join error: {e}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_absolute_paths() {
        let root = PathBuf::from("/tmp/root");
        assert!(resolve_path(&root, "/etc/passwd").is_err());
    }

    #[test]
    fn rejects_parent_dir() {
        let root = PathBuf::from("/tmp/root");
        assert!(resolve_path(&root, "../etc/passwd").is_err());
        assert!(resolve_path(&root, "foo/../../bar").is_err());
    }

    #[test]
    fn accepts_normal_paths() {
        let root = PathBuf::from("/tmp/root");
        assert_eq!(
            resolve_path(&root, "src/lib.rs").unwrap(),
            PathBuf::from("/tmp/root/src/lib.rs")
        );
        // Empty path => root.
        assert_eq!(resolve_path(&root, "").unwrap(), PathBuf::from("/tmp/root"));
        // CurDir components are permitted.
        assert_eq!(
            resolve_path(&root, "./src").unwrap(),
            PathBuf::from("/tmp/root/./src")
        );
    }

    #[tokio::test]
    async fn stat_reports_real_file_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes").join("today.md");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"hello").unwrap();

        let out = stat(dir.path(), "notes/today.md".into()).await;
        match &out[..] {
            [FilesServerMessage::Stat { path, entry }] => {
                assert_eq!(path, "notes/today.md");
                assert_eq!(entry.name, "today.md");
                assert!(!entry.is_dir);
                assert_eq!(entry.size, Some(5));
            }
            other => panic!("unexpected stat response: {other:?}"),
        }
    }

    #[tokio::test]
    async fn stat_reports_directories_without_size() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();

        let out = stat(dir.path(), "src".into()).await;
        match &out[..] {
            [FilesServerMessage::Stat { path, entry }] => {
                assert_eq!(path, "src");
                assert_eq!(entry.name, "src");
                assert!(entry.is_dir);
                assert_eq!(entry.size, None);
            }
            other => panic!("unexpected stat response: {other:?}"),
        }
    }
}

/// The `icon:` value from a markdown file's YAML frontmatter, or `None`.
///
/// Mirrors `neoism_ui::panels::notes_sidebar::note_frontmatter_icon`,
/// which reads the file directly — fine on the desktop, a silent `None`
/// in a wasm client with no filesystem. Serving it from here is what lets
/// web notes rows and tabs show the same emoji the desktop does.
///
/// Only the first KB is read, and only the leading frontmatter block is
/// scanned, so this stays cheap enough to run per directory entry.
async fn markdown_frontmatter_icon(path: &Path) -> Option<String> {
    if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
        return None;
    }
    let mut file = fs::File::open(path).await.ok()?;
    let mut buffer = vec![0u8; 1024];
    let read = {
        use tokio::io::AsyncReadExt;
        file.read(&mut buffer).await.ok()?
    };
    let head = String::from_utf8_lossy(&buffer[..read]).into_owned();
    let mut lines = head.lines();
    if lines.next().map(str::trim) != Some("---") {
        return None;
    }
    for line in lines.take(32) {
        if line.trim() == "---" {
            return None;
        }
        let Some((key, value)) = line.split_once(':') else {
            continue;
        };
        if key.trim() != "icon" {
            continue;
        }
        let icon = value.trim().trim_matches(&['"', '\''][..]).trim();
        return (!icon.is_empty()).then(|| icon.to_string());
    }
    None
}

#[cfg(test)]
mod frontmatter_icon_tests {
    use super::*;

    #[tokio::test]
    async fn reads_icon_from_frontmatter() {
        let dir = tempfile::tempdir().unwrap();
        let note = dir.path().join("TASKS.md");
        tokio::fs::write(&note, "---\nicon: \u{1f525}\ncover: creation\n---\n# T\n")
            .await
            .unwrap();
        assert_eq!(
            markdown_frontmatter_icon(&note).await.as_deref(),
            Some("\u{1f525}")
        );
    }

    #[tokio::test]
    async fn non_markdown_and_iconless_files_are_none() {
        let dir = tempfile::tempdir().unwrap();
        let txt = dir.path().join("a.txt");
        tokio::fs::write(&txt, "---\nicon: x\n---\n").await.unwrap();
        assert!(markdown_frontmatter_icon(&txt).await.is_none());

        let plain = dir.path().join("b.md");
        tokio::fs::write(&plain, "# just a heading\n").await.unwrap();
        assert!(markdown_frontmatter_icon(&plain).await.is_none());
    }
}
