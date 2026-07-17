use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::Context;
use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use neoism_agent_core::{MessageWithParts, Part, ToolState};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const MAX_SCAN_FILES: usize = 1000;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileSnapshot {
    pub(crate) path: String,
    #[serde(default, skip_serializing_if = "is_false")]
    pub(crate) absolute: bool,
    pub(crate) before: FileState,
    pub(crate) after: FileState,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FileState {
    pub(crate) exists: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) content_base64: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) sha256: Option<String>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SnapshotDirection {
    Revert,
    Unrevert,
}

#[derive(Debug)]
pub(crate) enum SnapshotApplyError {
    Conflict(String),
    Io(anyhow::Error),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SnapshotApplyReport {
    pub(crate) files: usize,
}

#[derive(Clone, Debug)]
pub(crate) struct BashSnapshot {
    root: PathBuf,
    before: SnapshotBaseline,
}

#[derive(Clone, Debug)]
enum SnapshotBaseline {
    Git(BTreeMap<String, FileState>),
    Tree(BTreeMap<String, FileState>),
}

impl FileState {
    pub(crate) fn from_path(path: &Path) -> anyhow::Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Ok(Self::from_bytes(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(Self::missing())
            }
            Err(error) => {
                Err(error).with_context(|| format!("failed to read {}", path.display()))
            }
        }
    }

    fn from_bytes(bytes: Vec<u8>) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(&bytes);
        Self {
            exists: true,
            content_base64: Some(STANDARD.encode(bytes)),
            sha256: Some(format!("{:x}", hasher.finalize())),
        }
    }

    fn missing() -> Self {
        Self {
            exists: false,
            content_base64: None,
            sha256: None,
        }
    }

    fn bytes(&self) -> anyhow::Result<Option<Vec<u8>>> {
        if !self.exists {
            return Ok(None);
        }
        let Some(content) = &self.content_base64 else {
            anyhow::bail!("snapshot content is missing");
        };
        Ok(Some(STANDARD.decode(content)?))
    }
}

impl std::fmt::Display for SnapshotApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SnapshotApplyError::Conflict(message) => f.write_str(message),
            SnapshotApplyError::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for SnapshotApplyError {}

pub(crate) fn file_change(
    root: &Path,
    path: &Path,
    before: FileState,
) -> anyhow::Result<Option<FileSnapshot>> {
    let after = FileState::from_path(path)?;
    Ok(snapshot_from_states(root, path, before, after))
}

pub(crate) fn add_metadata_snapshots(metadata: &mut Value, snapshots: Vec<FileSnapshot>) {
    if snapshots.is_empty() {
        return;
    }
    let diffs = snapshots
        .iter()
        .filter_map(file_diff_summary)
        .collect::<Vec<_>>();
    let files = snapshots
        .iter()
        .filter_map(file_patch_metadata)
        .collect::<Vec<_>>();
    let aggregate_diff = files
        .iter()
        .filter_map(|file| file.get("patch").and_then(Value::as_str))
        .filter(|patch| !patch.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    if let Some(object) = metadata.as_object_mut() {
        object.insert("snapshots".to_string(), json!(snapshots));
        if !diffs.is_empty() {
            object.insert("diffs".to_string(), Value::Array(diffs));
        }
        if !aggregate_diff.is_empty() {
            object.insert("diff".to_string(), Value::String(aggregate_diff));
        }
        if !files.is_empty() {
            object.insert("files".to_string(), Value::Array(files));
        }
    }
}

pub(crate) fn bash_before(root: &Path) -> BashSnapshot {
    let root = canonical_or_self(root);
    let before = git_status_states(&root)
        .map(SnapshotBaseline::Git)
        .unwrap_or_else(|| SnapshotBaseline::Tree(tree_states(&root)));
    BashSnapshot { root, before }
}

pub(crate) fn bash_after(before: BashSnapshot) -> Vec<FileSnapshot> {
    match before.before {
        SnapshotBaseline::Git(before_states) => {
            let mut paths = before_states.keys().cloned().collect::<BTreeSet<_>>();
            if let Some(after_states) = git_status_states(&before.root) {
                paths.extend(after_states.keys().cloned());
            }
            paths
                .into_iter()
                .filter(|path| !ignored_snapshot_path(Path::new(path)))
                .filter_map(|path| {
                    let full_path = before.root.join(&path);
                    let before_state = before_states
                        .get(&path)
                        .cloned()
                        .or_else(|| git_head_state(&before.root, &path))
                        .unwrap_or_else(FileState::missing);
                    let after_state = FileState::from_path(&full_path).ok()?;
                    snapshot_from_states(
                        &before.root,
                        &full_path,
                        before_state,
                        after_state,
                    )
                })
                .collect()
        }
        SnapshotBaseline::Tree(before_states) => {
            let after_states = tree_states(&before.root);
            let mut paths = before_states.keys().cloned().collect::<BTreeSet<_>>();
            paths.extend(after_states.keys().cloned());
            paths
                .into_iter()
                .filter_map(|path| {
                    let full_path = before.root.join(&path);
                    let before_state = before_states
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(FileState::missing);
                    let after_state = after_states
                        .get(&path)
                        .cloned()
                        .unwrap_or_else(FileState::missing);
                    snapshot_from_states(
                        &before.root,
                        &full_path,
                        before_state,
                        after_state,
                    )
                })
                .collect()
        }
    }
}

pub(crate) fn collect_from_revert_items(
    removed_messages: &[MessageWithParts],
    removed_parts: &[Part],
) -> Vec<FileSnapshot> {
    let mut snapshots = Vec::new();
    snapshots.extend(collect_from_parts(removed_parts));
    for message in removed_messages {
        snapshots.extend(collect_from_parts(&message.parts));
    }
    snapshots
}

pub(crate) fn collect_from_parts(parts: &[Part]) -> Vec<FileSnapshot> {
    let mut snapshots = Vec::new();
    for part in parts {
        collect_from_part(part, &mut snapshots);
    }
    snapshots
}

pub(crate) fn apply(
    root: &str,
    snapshots: &[FileSnapshot],
    direction: SnapshotDirection,
) -> Result<SnapshotApplyReport, SnapshotApplyError> {
    if snapshots.is_empty() {
        return Ok(SnapshotApplyReport { files: 0 });
    }
    let root = canonical_or_self(Path::new(root));
    let ordered: Vec<&FileSnapshot> = match direction {
        SnapshotDirection::Revert => snapshots.iter().rev().collect(),
        SnapshotDirection::Unrevert => snapshots.iter().collect(),
    };
    let mut planned = BTreeMap::<PathBuf, FileState>::new();

    for snapshot in ordered {
        let path = snapshot_path(&root, snapshot);
        let current = match planned.get(&path) {
            Some(state) => state.clone(),
            None => FileState::from_path(&path).map_err(SnapshotApplyError::Io)?,
        };
        let (expected, replacement) = match direction {
            SnapshotDirection::Revert => (&snapshot.after, &snapshot.before),
            SnapshotDirection::Unrevert => (&snapshot.before, &snapshot.after),
        };
        if &current != expected {
            return Err(SnapshotApplyError::Conflict(format!(
                "snapshot conflict for {}: current file content differs from expected {}-change content",
                snapshot.path,
                match direction {
                    SnapshotDirection::Revert => "post",
                    SnapshotDirection::Unrevert => "pre",
                }
            )));
        }
        planned.insert(path, replacement.clone());
    }

    for (path, state) in &planned {
        write_state(path, state).map_err(SnapshotApplyError::Io)?;
    }
    Ok(SnapshotApplyReport {
        files: planned.len(),
    })
}

fn collect_from_part(part: &Part, snapshots: &mut Vec<FileSnapshot>) {
    let Part::Tool(tool) = part else {
        return;
    };
    let ToolState::Completed { metadata, .. } = &tool.state else {
        return;
    };
    let Some(value) = metadata.get("snapshots").cloned() else {
        return;
    };
    if let Ok(mut decoded) = serde_json::from_value::<Vec<FileSnapshot>>(value) {
        snapshots.append(&mut decoded);
    }
}

fn snapshot_from_states(
    root: &Path,
    path: &Path,
    before: FileState,
    after: FileState,
) -> Option<FileSnapshot> {
    if before == after {
        return None;
    }
    let root = canonical_or_self(root);
    let path = canonical_for_snapshot(path);
    let (path, absolute) = match path.strip_prefix(&root) {
        Ok(relative) => (slash_path(relative), false),
        Err(_) => (path.to_string_lossy().to_string(), true),
    };
    Some(FileSnapshot {
        path,
        absolute,
        before,
        after,
    })
}

fn file_diff_summary(snapshot: &FileSnapshot) -> Option<Value> {
    let before_text = state_text(&snapshot.before)?;
    let after_text = state_text(&snapshot.after)?;
    let before = before_text.lines().collect::<Vec<_>>();
    let after = after_text.lines().collect::<Vec<_>>();
    let (additions, deletions, old_start, new_start) = diff_counts(&before, &after);
    Some(json!({
        "path": snapshot.path,
        "kind": if !snapshot.before.exists {
            "added"
        } else if !snapshot.after.exists {
            "deleted"
        } else {
            "modified"
        },
        "additions": additions,
        "deletions": deletions,
        "oldStart": old_start,
        "newStart": new_start,
        "beforeSha256": snapshot.before.sha256,
        "afterSha256": snapshot.after.sha256,
    }))
}

fn file_patch_metadata(snapshot: &FileSnapshot) -> Option<Value> {
    let before_text = state_text(&snapshot.before)?;
    let after_text = state_text(&snapshot.after)?;
    let before = before_text.lines().collect::<Vec<_>>();
    let after = after_text.lines().collect::<Vec<_>>();
    let (additions, deletions, _, _) = diff_counts(&before, &after);
    let patch = unified_patch(snapshot, &before, &after)?;
    Some(json!({
        "filePath": snapshot.path,
        "relativePath": snapshot.path,
        "type": if !snapshot.before.exists {
            "add"
        } else if !snapshot.after.exists {
            "delete"
        } else {
            "update"
        },
        "patch": patch,
        "additions": additions,
        "deletions": deletions,
    }))
}

fn unified_patch(
    snapshot: &FileSnapshot,
    before: &[&str],
    after: &[&str],
) -> Option<String> {
    if before == after {
        return None;
    }
    let mut prefix = 0usize;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < before.len()
        && suffix + prefix < after.len()
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let before_change_end = before.len().saturating_sub(suffix);
    let after_change_end = after.len().saturating_sub(suffix);
    let context = 3usize;
    let before_start = prefix.saturating_sub(context);
    let after_start = prefix.saturating_sub(context);
    let before_end = (before_change_end + context).min(before.len());
    let after_end = (after_change_end + context).min(after.len());
    let before_count = before_end.saturating_sub(before_start);
    let after_count = after_end.saturating_sub(after_start);
    let mut patch = String::new();
    let path = snapshot.path.as_str();
    patch.push_str(&format!("diff --git a/{path} b/{path}\n"));
    if snapshot.before.exists {
        patch.push_str(&format!("--- a/{path}\n"));
    } else {
        patch.push_str("--- /dev/null\n");
    }
    if snapshot.after.exists {
        patch.push_str(&format!("+++ b/{path}\n"));
    } else {
        patch.push_str("+++ /dev/null\n");
    }
    patch.push_str(&format!(
        "@@ -{},{} +{},{} @@\n",
        hunk_start(before_start, before_count),
        before_count,
        hunk_start(after_start, after_count),
        after_count,
    ));
    for line in &before[before_start..prefix] {
        patch.push(' ');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in &before[prefix..before_change_end] {
        patch.push('-');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in &after[prefix..after_change_end] {
        patch.push('+');
        patch.push_str(line);
        patch.push('\n');
    }
    for line in &before[before_change_end..before_end] {
        patch.push(' ');
        patch.push_str(line);
        patch.push('\n');
    }
    Some(patch)
}

fn hunk_start(start: usize, count: usize) -> usize {
    if count == 0 {
        0
    } else {
        start + 1
    }
}

fn state_text(state: &FileState) -> Option<String> {
    match state.bytes() {
        Ok(Some(bytes)) => String::from_utf8(bytes).ok(),
        Ok(None) => Some(String::new()),
        Err(_) => None,
    }
}

fn diff_counts(
    before: &[&str],
    after: &[&str],
) -> (usize, usize, Option<usize>, Option<usize>) {
    let mut prefix = 0usize;
    while prefix < before.len() && prefix < after.len() && before[prefix] == after[prefix]
    {
        prefix += 1;
    }
    let mut suffix = 0usize;
    while suffix + prefix < before.len()
        && suffix + prefix < after.len()
        && before[before.len() - 1 - suffix] == after[after.len() - 1 - suffix]
    {
        suffix += 1;
    }
    let before_end = before.len().saturating_sub(suffix);
    let after_end = after.len().saturating_sub(suffix);
    let deletions = before_end.saturating_sub(prefix);
    let additions = after_end.saturating_sub(prefix);
    let changed = additions > 0 || deletions > 0;
    (
        additions,
        deletions,
        changed.then_some(prefix + 1),
        changed.then_some(prefix + 1),
    )
}

fn snapshot_path(root: &Path, snapshot: &FileSnapshot) -> PathBuf {
    if snapshot.absolute {
        PathBuf::from(&snapshot.path)
    } else {
        root.join(&snapshot.path)
    }
}

fn write_state(path: &Path, state: &FileState) -> anyhow::Result<()> {
    match state.bytes()? {
        Some(bytes) => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .with_context(|| format!("failed to create {}", parent.display()))?;
            }
            std::fs::write(path, bytes)
                .with_context(|| format!("failed to write {}", path.display()))?;
        }
        None => match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to remove {}", path.display()));
            }
        },
    }
    Ok(())
}

fn git_status_states(root: &Path) -> Option<BTreeMap<String, FileState>> {
    // This runs twice per bash command (before + after) to diff what the
    // command touched, so keep it cheap: `--no-optional-locks` skips the
    // on-disk index refresh/lock, and `--untracked-files=normal` avoids
    // recursively walking whole untracked directories (a big cost on repos
    // with large untracked trees). New files under a fresh untracked dir
    // surface as the dir and are gracefully skipped by the snapshot rather
    // than dragging every bash call.
    let output = Command::new("git")
        .args([
            "--no-optional-locks",
            "status",
            "--porcelain=v1",
            "-z",
            "--untracked-files=normal",
        ])
        .current_dir(root)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let mut states = BTreeMap::new();
    for path in git_status_paths(&output.stdout) {
        if ignored_snapshot_path(Path::new(&path)) {
            continue;
        }
        let state = FileState::from_path(&root.join(&path)).ok()?;
        states.insert(path, state);
    }
    Some(states)
}

fn git_status_paths(output: &[u8]) -> Vec<String> {
    let mut paths = Vec::new();
    let mut fields = output
        .split(|byte| *byte == 0)
        .filter(|field| !field.is_empty());
    while let Some(field) = fields.next() {
        if field.len() < 4 {
            continue;
        }
        let code = String::from_utf8_lossy(&field[..2]);
        let path = String::from_utf8_lossy(&field[3..]).to_string();
        paths.push(path);
        if code.starts_with('R') || code.starts_with('C') {
            fields.next();
        }
    }
    paths
}

fn git_head_state(root: &Path, path: &str) -> Option<FileState> {
    let output = Command::new("git")
        .args(["show", &format!("HEAD:{path}")])
        .current_dir(root)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| FileState::from_bytes(output.stdout))
}

fn tree_states(root: &Path) -> BTreeMap<String, FileState> {
    let mut states = BTreeMap::new();
    collect_tree_states(root, root, &mut states);
    states
}

fn collect_tree_states(
    root: &Path,
    path: &Path,
    states: &mut BTreeMap<String, FileState>,
) {
    if states.len() >= MAX_SCAN_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return;
    };
    for entry in entries.flatten() {
        if states.len() >= MAX_SCAN_FILES {
            break;
        }
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if ignored_snapshot_path(path.strip_prefix(root).unwrap_or(&path)) {
            continue;
        }
        if file_type.is_dir() {
            collect_tree_states(root, &path, states);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES {
            continue;
        }
        if let (Ok(relative), Ok(state)) =
            (path.strip_prefix(root), FileState::from_path(&path))
        {
            states.insert(slash_path(relative), state);
        }
    }
}

fn ignored_snapshot_path(path: &Path) -> bool {
    let mut components = path.components().filter_map(|component| {
        let text = component.as_os_str().to_string_lossy();
        (!text.is_empty()).then(|| text.to_string())
    });
    if let Some(first) = components.next() {
        if matches!(
            first.as_str(),
            ".git" | "target" | "node_modules" | ".next" | "dist"
        ) {
            return true;
        }
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    name.ends_with(".sqlite3")
        || name.ends_with(".sqlite3-wal")
        || name.ends_with(".sqlite3-shm")
}

fn canonical_or_self(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn canonical_for_snapshot(path: &Path) -> PathBuf {
    if path.exists() {
        return canonical_or_self(path);
    }
    if let Some(parent) = path.parent() {
        if let Ok(parent) = parent.canonicalize() {
            if let Some(name) = path.file_name() {
                return parent.join(name);
            }
        }
    }
    path.to_path_buf()
}

fn slash_path(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_agent_core::{Id, IdKind, PartTime, ToolPart};

    #[test]
    fn apply_revert_and_unrevert_detects_conflicts() {
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-snapshot-{}",
            Id::ascending(IdKind::Event)
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("file.txt");
        std::fs::write(&path, "before").unwrap();
        let before = FileState::from_path(&path).unwrap();
        std::fs::write(&path, "after").unwrap();
        let snapshot = file_change(&root, &path, before).unwrap().unwrap();

        apply(
            root.to_str().unwrap(),
            &[snapshot.clone()],
            SnapshotDirection::Revert,
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "before");

        std::fs::write(&path, "user edit").unwrap();
        let error = apply(
            root.to_str().unwrap(),
            &[snapshot],
            SnapshotDirection::Unrevert,
        )
        .unwrap_err();
        assert!(matches!(error, SnapshotApplyError::Conflict(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "user edit");

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn collect_from_completed_tool_metadata() {
        let snapshot = FileSnapshot {
            path: "a.txt".to_string(),
            absolute: false,
            before: FileState::missing(),
            after: FileState::from_bytes(b"after".to_vec()),
        };
        let part = Part::Tool(ToolPart {
            id: Id::ascending(IdKind::Part),
            session_id: Id::ascending(IdKind::Session),
            message_id: Id::ascending(IdKind::Message),
            tool: "write".to_string(),
            call_id: "call".to_string(),
            state: ToolState::Completed {
                input: json!({}),
                output: "ok".to_string(),
                metadata: json!({ "snapshots": [snapshot.clone()] }),
                title: "Write".to_string(),
                time: PartTime {
                    start: 1,
                    end: Some(2),
                },
            },
            metadata: None,
        });

        assert_eq!(collect_from_revert_items(&[], &[part]), vec![snapshot]);
    }

    #[test]
    fn metadata_snapshots_include_stable_diff_summary() {
        let snapshot = FileSnapshot {
            path: "TASK.md".to_string(),
            absolute: false,
            before: FileState::from_bytes(b"one\nbefore\nthree\n".to_vec()),
            after: FileState::from_bytes(b"one\nafter\nagain\nthree\n".to_vec()),
        };
        let mut metadata = json!({});

        add_metadata_snapshots(&mut metadata, vec![snapshot]);

        assert_eq!(metadata["snapshots"].as_array().unwrap().len(), 1);
        let diff = &metadata["diffs"][0];
        assert_eq!(diff["path"], "TASK.md");
        assert_eq!(diff["kind"], "modified");
        assert_eq!(diff["additions"], 2);
        assert_eq!(diff["deletions"], 1);
        assert_eq!(diff["oldStart"], 2);
        assert_eq!(diff["newStart"], 2);
        assert!(diff["beforeSha256"].is_string());
        assert!(diff["afterSha256"].is_string());
    }
}
