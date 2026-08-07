use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::web::{bounded_arg, limit_text, render_web_body, string_array_or_single_arg};
use super::*;

fn allow_context(root: &Path) -> ToolContext {
    ToolContext::new(root)
        .with_permissions(BTreeMap::from([("*".to_string(), json!("allow"))]))
}

#[tokio::test]
async fn safe_filesystem_tools_execute_inside_project() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "fn main() {}\nlet needle = true;\n",
    )
    .unwrap();
    let context = allow_context(&root);

    let read = execute(
        "read",
        context.clone(),
        json!({ "path": "src/lib.rs", "limit": 1 }),
    )
    .await
    .unwrap();
    assert!(read.output.contains("<type>file</type>"));
    assert!(read.output.contains("1: fn main() {}"));

    std::fs::write(root.join("src/other.rs"), "fn other() {}\n").unwrap();
    let batch_read = execute(
        "read",
        context.clone(),
        json!({ "paths": ["src/lib.rs", "src/other.rs"], "limit": 1 }),
    )
    .await
    .unwrap();
    assert!(batch_read.output.contains("src/lib.rs"));
    assert!(batch_read.output.contains("src/other.rs"));
    assert_eq!(batch_read.metadata.as_ref().unwrap()["type"], "batch");
    assert_eq!(batch_read.metadata.as_ref().unwrap()["count"], 2);

    let file_paths_batch_read = execute(
        "read",
        context.clone(),
        json!({ "paths": [], "filePaths": ["src/lib.rs", "src/other.rs"], "limit": 1 }),
    )
    .await
    .unwrap();
    assert!(file_paths_batch_read.output.contains("src/lib.rs"));
    assert!(file_paths_batch_read.output.contains("src/other.rs"));
    assert_eq!(
        file_paths_batch_read.metadata.as_ref().unwrap()["type"],
        "batch"
    );
    assert_eq!(file_paths_batch_read.metadata.as_ref().unwrap()["count"], 2);

    let listed = execute("list", context.clone(), json!({ "path": "." }))
        .await
        .unwrap();
    assert_eq!(listed.output, "src/");

    let grep = execute("grep", context.clone(), json!({ "pattern": "needle" }))
        .await
        .unwrap();
    assert!(grep.output.contains("src/lib.rs:"));
    assert!(grep.output.contains("Line 2:"));

    let glob = execute("glob", context, json!({ "pattern": "*.rs" }))
        .await
        .unwrap();
    assert!(glob.output.contains("src/lib.rs"));
    assert!(glob.output.contains("src/other.rs"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn grep_and_glob_return_opencode_style_metadata() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-search-metadata-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "needle one\nneedle two\n").unwrap();
    std::fs::write(root.join("src/b.txt"), "needle text\n").unwrap();
    let context = allow_context(&root);

    let grep = execute(
        "grep",
        context.clone(),
        json!({ "pattern": "needle", "include": "*.{rs,txt}", "exclude": "b.txt", "limit": 1 }),
    )
    .await
    .unwrap();
    assert!(grep.output.contains("Found 2 matches (showing first 1)"));
    assert!(grep.output.contains("src/a.rs:"));
    assert!(!grep.output.contains("src/b.txt"));
    let metadata = grep.metadata.unwrap();
    assert_eq!(metadata["matches"], 2);
    assert_eq!(metadata["truncated"], true);
    assert_eq!(metadata["items"].as_array().unwrap().len(), 1);
    assert_eq!(metadata["items"][0]["line"], 1);

    let glob = execute(
        "glob",
        context.clone(),
        json!({ "pattern": "*.{rs,txt}", "path": "src", "exclude": "b.txt", "limit": 1 }),
    )
    .await
    .unwrap();
    let metadata = glob.metadata.unwrap();
    assert_eq!(metadata["count"], 1);
    assert_eq!(metadata["total"], 1);
    assert_eq!(metadata["truncated"], false);
    assert_eq!(metadata["items"].as_array().unwrap().len(), 1);

    let error = execute(
        "glob",
        context,
        json!({ "pattern": "*.rs", "path": "src/a.rs" }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("glob path must be a directory"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn fff_tools_search_files_contents_and_variants() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-fff-search-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/upload.rs"),
        "pub struct PrepareUpload;\nfn prepare_upload() {}\n",
    )
    .unwrap();
    std::fs::write(root.join("src/other.rs"), "fn unrelated() {}\n").unwrap();
    let context = allow_context(&root);

    let find = execute(
        "fffind",
        context.clone(),
        json!({ "query": "upload", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(find.output.contains("src/upload.rs"));
    assert_eq!(find.metadata.as_ref().unwrap()["engine"], "fff");

    let listed = execute(
        "fffind",
        context.clone(),
        json!({ "path": "src", "limit": 10 }),
    )
    .await
    .unwrap();
    assert!(listed.output.contains("other.rs"));
    assert!(listed.output.contains("upload.rs"));
    assert_eq!(listed.metadata.as_ref().unwrap()["engine"], "directory");

    let grep = execute(
        "ffgrep",
        context.clone(),
        json!({ "pattern": "PrepareUpload", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(grep.output.contains("src/upload.rs:"));
    assert!(grep.output.contains("Line 1"));
    assert_eq!(grep.metadata.as_ref().unwrap()["engine"], "fff");

    let scoped_grep = execute(
        "ffgrep",
        context.clone(),
        json!({ "pattern": "PrepareUpload", "path": "src", "include": "*.rs", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(scoped_grep.output.contains("upload.rs:"));
    assert!(scoped_grep.output.contains("Line 1"));

    let multi = execute(
        "fff_multi_grep",
        context,
        json!({ "patterns": ["PrepareUpload", "prepare_upload"], "limit": 10 }),
    )
    .await
    .unwrap();
    assert!(multi.output.contains("Line 1"));
    assert!(multi.output.contains("Line 2"));
    assert_eq!(multi.metadata.as_ref().unwrap()["engine"], "fff");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn notes_tool_indexes_and_queries_workspace_notes() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-notes-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let notes_home = root.join("vaults");
    unsafe {
        std::env::set_var("NEOISM_NOTES_HOME", &notes_home);
    }
    let notes_root =
        notes_home.join(neoism_workspace_index::config::DEFAULT_NOTES_WORKSPACE);
    let roadmap = "Roadmap.md".to_string();
    let plan = "Plan.md".to_string();
    let planning_roadmap = "Planning/Roadmap.md".to_string();
    std::fs::create_dir_all(&notes_root).unwrap();
    std::fs::write(
        notes_root.join("Roadmap.md"),
        "---\nowner: Parker\npriority: 2\n---\n# Roadmap\n\n- [ ] ship notes #neoism\n\nSee [[Plan]].\n",
    )
    .unwrap();
    std::fs::write(
        notes_root.join("Plan.md"),
        "# Plan\n\nBack to [[Roadmap]].\n",
    )
    .unwrap();
    let context = allow_context(&root);

    let init = execute("notes", context.clone(), json!({ "operation": "init" }))
        .await
        .unwrap();
    assert!(init.output.contains("Initialized note graph"));
    assert_eq!(init.metadata.as_ref().unwrap()["operation"], "init");

    let backlinks = execute(
        "notes",
        context.clone(),
        json!({ "operation": "backlinks", "note": "Roadmap" }),
    )
    .await
    .unwrap();
    assert!(backlinks.output.contains(&format!("{plan}:3 -> {roadmap}")));
    assert_eq!(
        backlinks.metadata.as_ref().unwrap()["links"][0]["sourcePath"],
        plan
    );

    let search = execute(
        "notes",
        context.clone(),
        json!({ "operation": "search", "query": "ship notes" }),
    )
    .await
    .unwrap();
    assert!(search.output.contains(&format!("{roadmap}:7")));

    let properties = execute(
        "notes",
        context.clone(),
        json!({ "operation": "properties", "note": "Roadmap" }),
    )
    .await
    .unwrap();
    assert!(properties
        .output
        .contains(&format!("{roadmap} owner=\"Parker\"")));
    assert_eq!(
        properties.metadata.as_ref().unwrap()["properties"][0]["path"],
        roadmap
    );

    let toggled = execute(
        "notes",
        context.clone(),
        json!({ "operation": "taskToggle", "path": roadmap.clone(), "line": 7, "checked": true }),
    )
    .await
    .unwrap();
    assert!(toggled
        .output
        .contains(&format!("{roadmap}:7 - [x] ship notes")));
    assert_eq!(toggled.metadata.as_ref().unwrap()["task"]["checked"], true);

    std::fs::create_dir_all(notes_root.join("Planning")).unwrap();
    std::fs::rename(
        notes_root.join("Roadmap.md"),
        notes_root.join("Planning/Roadmap.md"),
    )
    .unwrap();
    let repaired = execute(
        "notes",
        context,
        json!({ "operation": "repairMove", "oldPath": roadmap, "newPath": planning_roadmap }),
    )
    .await
    .unwrap();
    assert!(repaired.output.contains("Repaired 1 links in 1 files"));
    assert_eq!(
        std::fs::read_to_string(notes_root.join("Plan.md")).unwrap(),
        "# Plan\n\nBack to [[Planning/Roadmap]].\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn safe_tools_reject_external_paths() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let external = std::env::temp_dir().join(format!(
        "neoism-agent-tools-external-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(&external, "secret").unwrap();

    let error = execute(
        "read",
        ToolContext::new(&root)
            .with_permissions(BTreeMap::from([("read".to_string(), json!("allow"))])),
        json!({ "path": external.to_string_lossy() }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("external_directory"));

    let _ = std::fs::remove_file(external);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_many_reads_per_file_ranges() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-many-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src/a.rs"), "a1\na2\na3\na4\n").unwrap();
    std::fs::write(root.join("src/b.rs"), "b1\nb2\nb3\n").unwrap();

    let result = execute(
        "read_many",
        allow_context(&root),
        json!({
            "files": [
                { "path": "src/a.rs", "ranges": [{ "offset": 2, "limit": 2 }] },
                { "path": "src/b.rs", "offset": 3, "limit": 1 }
            ]
        }),
    )
    .await
    .unwrap();

    assert!(result.output.contains("2: a2"));
    assert!(result.output.contains("3: a3"));
    assert!(result.output.contains("3: b3"));
    assert_eq!(result.metadata.unwrap()["count"], 2);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_around_reads_pattern_window() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-around-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("src/lib.rs"),
        "one\ntwo\nfn target() {}\nfour\nfive\n",
    )
    .unwrap();

    let result = execute(
        "read_around",
        allow_context(&root),
        json!({
            "path": "src/lib.rs",
            "line": 0,
            "pattern": "target",
            "before": 1,
            "after": 1
        }),
    )
    .await
    .unwrap();

    assert!(result.output.contains("2: two"));
    assert!(result.output.contains("3: fn target() {}"));
    assert!(result.output.contains("4: four"));
    assert!(!result.output.contains("1: one"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn external_directory_permission_allows_whitelisted_paths() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let external_dir = std::env::temp_dir().join(format!(
        "neoism-agent-tools-external-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let external = external_dir.join("file.txt");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&external_dir);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external_dir).unwrap();
    std::fs::write(&external, "secret").unwrap();
    let pattern = format!("{}/*", external_dir.display());
    let mut external_rules = serde_json::Map::new();
    external_rules.insert(pattern, json!("allow"));

    let result = execute(
        "read",
        ToolContext::new(&root).with_permissions(BTreeMap::from([
            ("read".to_string(), json!("allow")),
            (
                "external_directory".to_string(),
                Value::Object(external_rules),
            ),
        ])),
        json!({ "path": external.to_string_lossy() }),
    )
    .await
    .unwrap();
    assert!(result.output.contains("<type>file</type>"));
    assert!(result.output.contains("1: secret"));

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_dir);
}

#[tokio::test]
async fn read_tool_lists_directories_with_offsets() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-dir-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("dir/sub")).unwrap();
    std::fs::write(root.join("dir/a.txt"), "a").unwrap();
    std::fs::write(root.join("dir/b.txt"), "b").unwrap();

    let result = execute(
        "read",
        allow_context(&root),
        json!({ "path": "dir", "offset": 2, "limit": 2 }),
    )
    .await
    .unwrap();
    assert!(result.output.contains("<type>directory</type>"));
    assert!(result.output.contains("b.txt\nsub/"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["type"], "directory");
    assert_eq!(metadata["truncated"], false);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_tool_prefers_file_path_over_path_context() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-filepath-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("TASK.md"), "target file\n").unwrap();

    let result = execute(
        "read",
        allow_context(&root),
        json!({
            "path": root.to_string_lossy(),
            "filePath": "TASK.md",
            "offset": 1,
            "limit": 5,
        }),
    )
    .await
    .unwrap();
    assert!(result.output.contains("<type>file</type>"));
    assert!(result.output.contains("1: target file"));
    assert_eq!(result.metadata.unwrap()["type"], "file");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_tool_loads_nearby_instruction_files() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-instructions-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let feature = root.join("src/feature");
    std::fs::create_dir_all(&feature).unwrap();
    std::fs::write(root.join("AGENTS.md"), "Root project instructions.\n").unwrap();
    std::fs::write(feature.join("AGENTS.md"), "Feature-local instructions.\n").unwrap();
    std::fs::write(feature.join("lib.rs"), "pub fn feature() {}\n").unwrap();

    let result = execute(
        "read",
        allow_context(&root),
        json!({ "filePath": "src/feature/lib.rs" }),
    )
    .await
    .unwrap();

    assert!(result.output.contains("1: pub fn feature() {}"));
    assert!(result.output.contains("<system-reminder>"));
    assert!(result.output.contains("Feature-local instructions."));
    assert!(!result.output.contains("Root project instructions."));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["loaded"].as_array().unwrap().len(), 1);
    assert!(metadata["loaded"][0]
        .as_str()
        .unwrap()
        .ends_with("src/feature/AGENTS.md"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_tool_returns_media_attachment_metadata() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-media-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("shot.png"), b"\x89PNG\r\n\x1a\nbytes").unwrap();

    let result = execute(
        "read",
        allow_context(&root),
        json!({ "filePath": "shot.png" }),
    )
    .await
    .unwrap();

    assert_eq!(result.output, "Image read successfully");
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["mime"], "image/png");
    assert_eq!(metadata["attachments"][0]["type"], "file");
    assert_eq!(metadata["attachments"][0]["mime"], "image/png");
    assert!(metadata["attachments"][0]["url"]
        .as_str()
        .unwrap()
        .starts_with("data:image/png;base64,"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn bash_tool_runs_in_project_and_obeys_permission() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-bash-tool-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("subdir")).unwrap();
    let context = ToolContext::new(&root)
        .with_permissions(BTreeMap::from([("bash".to_string(), json!("allow"))]));

    let result = execute(
        "bash",
        context,
        json!({
            "command": "printf neoism-bash",
            "description": "Print bash marker",
            "workdir": "subdir",
            "timeout": 120_000,
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.title, "Print bash marker");
    assert_eq!(result.output, "neoism-bash");
    assert_eq!(result.metadata.unwrap()["workdir"], "subdir");

    let denied = execute(
        "bash",
        ToolContext::new(&root)
            .with_permissions(BTreeMap::from([("bash".to_string(), json!("deny"))])),
        json!({ "command": "printf blocked", "description": "Print blocked marker" }),
    )
    .await
    .unwrap_err();
    assert!(denied.to_string().contains("permission bash"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn bash_tool_stops_when_cancelled() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-bash-cancel-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let cancel = Arc::new(AtomicBool::new(false));
    let context = ToolContext::new(&root)
        .with_permissions(BTreeMap::from([("bash".to_string(), json!("allow"))]))
        .with_cancel(Some(cancel.clone()));

    let task = tokio::spawn(async move {
        execute(
            "bash",
            context,
            json!({
                "command": "printf started; sleep 30; printf finished",
                "description": "Cancelable sleep",
                "timeout": 60_000,
            }),
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    cancel.store(true, Ordering::SeqCst);
    let error = tokio::time::timeout(Duration::from_secs(3), task)
        .await
        .expect("bash tool should return quickly after cancellation")
        .unwrap()
        .unwrap_err();
    assert!(error.to_string().contains("bash command aborted"));
    assert!(error.to_string().contains("started"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn safe_tools_apply_permission_rules() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("file.txt"), "content").unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("read".to_string(), json!("deny")),
    ]));

    let error = execute("read", context, json!({ "path": "file.txt" }))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("permission read"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn write_and_edit_tools_modify_project_files() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("edit".to_string(), json!("allow")),
    ]));

    let written = execute(
        "write",
        context.clone(),
        json!({ "path": "notes.txt", "content": "hello world" }),
    )
    .await
    .unwrap();
    assert!(written.output.contains("Wrote"));
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "hello world"
    );

    let edited = execute(
        "edit",
        context,
        json!({ "path": "notes.txt", "old": "world", "new": "neoism" }),
    )
    .await
    .unwrap();
    assert!(edited.output.contains("Replaced"));
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "hello neoism"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn write_tools_serialize_same_file_changes() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-file-lock-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "start\n").unwrap();

    let first_context = allow_context(&root);
    let second_context = allow_context(&root);
    let first = tokio::spawn(async move {
        execute(
            "write",
            first_context,
            json!({ "filePath": "notes.txt", "content": "first\n" }),
        )
        .await
    });
    let second = tokio::spawn(async move {
        execute(
            "write",
            second_context,
            json!({ "filePath": "notes.txt", "content": "second\n" }),
        )
        .await
    });

    first.await.unwrap().unwrap();
    second.await.unwrap().unwrap();
    let final_content = std::fs::read_to_string(root.join("notes.txt")).unwrap();
    assert!(matches!(final_content.as_str(), "first\n" | "second\n"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_locks_use_canonical_paths() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-canonical-file-lock-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let file = root.join("sub").join("notes.txt");
    std::fs::write(&file, "start\n").unwrap();

    let guard = super::locks::lock_file(&file).await;
    let alternate_path = root.join("sub").join("..").join("sub").join("notes.txt");
    let started = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let finished = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_clone = started.clone();
    let finished_clone = finished.clone();
    let task = tokio::spawn(async move {
        started_clone.store(true, std::sync::atomic::Ordering::SeqCst);
        let _guard = super::locks::lock_file(&alternate_path).await;
        finished_clone.store(true, std::sync::atomic::Ordering::SeqCst);
    });

    while !started.load(std::sync::atomic::Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    assert!(!finished.load(std::sync::atomic::Ordering::SeqCst));
    drop(guard);
    task.await.unwrap();
    assert!(finished.load(std::sync::atomic::Ordering::SeqCst));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn apply_patch_tool_modifies_project_files() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-apply-patch-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("notes.txt"), "hello world\n").unwrap();
    std::process::Command::new("git")
        .arg("init")
        .current_dir(&root)
        .output()
        .unwrap();

    let patch = "\
diff --git a/notes.txt b/notes.txt
--- a/notes.txt
+++ b/notes.txt
@@ -1 +1 @@
-hello world
+hello neoism
";
    let result = execute(
        "apply_patch",
        allow_context(&root),
        json!({ "patch": patch }),
    )
    .await
    .unwrap();

    assert!(result.output.contains("notes.txt"));
    assert_eq!(
        std::fs::read_to_string(root.join("notes.txt")).unwrap(),
        "hello neoism\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn write_tool_runs_configured_formatter() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-format-write-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = allow_context(&root).with_formatter(Some(json!({
        "testfmt": {
            "extensions": ["txt"],
            "command": ["sh", "-c", "printf formatted > \"$1\"", "neoism-testfmt", "$FILE"]
        }
    })));

    let result = execute(
        "write",
        context,
        json!({ "filePath": "note.txt", "content": "raw" }),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("note.txt")).unwrap(),
        "formatted"
    );
    assert_eq!(result.metadata.unwrap()["formatted"], json!(["note.txt"]));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn write_tool_reports_bounded_diagnostics_metadata() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-write-diagnostics-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let result = execute(
        "write",
        allow_context(&root),
        json!({ "filePath": "note.rs", "content": "fn main() {}\n" }),
    )
    .await
    .unwrap();

    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["diagnosticsProjectFileLimit"], json!(8));
    assert_eq!(metadata["diagnosticsProjectScanLimit"], json!(200));
    assert_eq!(metadata["diagnostics"][0]["path"], json!("note.rs"));
    assert_eq!(metadata["diagnostics"][0]["source"], json!("touched"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v4a_apply_patch_reports_added_file_diagnostics() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v4a-diagnostics-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let result = execute(
        "apply_patch",
        allow_context(&root),
        json!({
            "patchText": "*** Begin Patch\n*** Add File: added.rs\n+fn added() {}\n*** End Patch"
        }),
    )
    .await
    .unwrap();

    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["diagnostics"][0]["path"], json!("added.rs"));
    assert_eq!(metadata["diagnostics"][0]["source"], json!("touched"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v4a_apply_patch_rejects_existing_add() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v4a-existing-add-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("added.rs"), "old\n").unwrap();

    let error = execute(
        "apply_patch",
        allow_context(&root),
        json!({
            "patchText": "*** Begin Patch\n*** Add File: added.rs\n+new\n*** End Patch"
        }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("file already exists"));
    assert_eq!(
        std::fs::read_to_string(root.join("added.rs")).unwrap(),
        "old\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v4a_apply_patch_rejects_missing_delete() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v4a-missing-delete-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let error = execute(
        "apply_patch",
        allow_context(&root),
        json!({ "patchText": "*** Begin Patch\n*** Delete File: missing.rs\n*** End Patch" }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("file does not exist"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v4a_apply_patch_rejects_move_to_existing_target() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v4a-move-existing-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("old.rs"), "old\n").unwrap();
    std::fs::write(root.join("new.rs"), "new\n").unwrap();

    let error = execute(
        "apply_patch",
        allow_context(&root),
        json!({
            "patchText": "*** Begin Patch\n*** Update File: old.rs\n*** Move to: new.rs\n@@\n-old\n+old2\n*** End Patch"
        }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("target already exists"));
    assert_eq!(
        std::fs::read_to_string(root.join("old.rs")).unwrap(),
        "old\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("new.rs")).unwrap(),
        "new\n"
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn patch_paths_collects_new_and_modified_paths() {
    let paths = patch::paths(
        "\
diff --git a/src/lib.rs b/src/lib.rs
--- a/src/lib.rs
+++ b/src/lib.rs
diff --git a/new.txt b/new.txt
--- /dev/null
+++ b/new.txt
",
    );
    assert_eq!(paths, vec!["src/lib.rs", "new.txt"]);
}

#[test]
fn render_web_body_strips_tags_and_caps_output() {
    let (body, truncated) =
        render_web_body(b"<html><body>Hello <b>Neoism</b></body></html>");
    assert_eq!(body, "Hello Neoism");
    assert!(!truncated);
}

#[test]
fn web_batch_args_accept_arrays_and_single_fallbacks() {
    let urls = string_array_or_single_arg(
        &json!({ "urls": ["https://example.com", "https://neoism.dev"] }),
        "urls",
        "url",
    )
    .unwrap();
    assert_eq!(urls, vec!["https://example.com", "https://neoism.dev"]);

    let single = string_array_or_single_arg(
        &json!({ "url": "https://example.com" }),
        "urls",
        "url",
    )
    .unwrap();
    assert_eq!(single, vec!["https://example.com"]);

    let error =
        string_array_or_single_arg(&json!({ "urls": [] }), "urls", "url").unwrap_err();
    assert!(error.to_string().contains("must not be empty"));
}

#[test]
fn web_batch_limits_are_bounded() {
    assert_eq!(bounded_arg(&json!({}), "concurrency", 4, 8), 4);
    assert_eq!(
        bounded_arg(&json!({ "concurrency": 0 }), "concurrency", 4, 8),
        1
    );
    assert_eq!(
        bounded_arg(&json!({ "concurrency": 99 }), "concurrency", 4, 8),
        8
    );

    let (limited, truncated) = limit_text("abcdef", 3);
    assert!(truncated);
    assert!(limited.starts_with("abc"));
    assert!(limited.contains("Item output truncated"));
}

#[tokio::test]
async fn file_tools_accept_opencode_argument_aliases() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tool-aliases-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("edit".to_string(), json!("allow")),
    ]));

    execute(
        "write",
        context.clone(),
        json!({ "filePath": "notes.txt", "content": "alpha alpha" }),
    )
    .await
    .unwrap();
    let edited = execute(
        "edit",
        context.clone(),
        json!({
            "filePath": "notes.txt",
            "oldString": "alpha",
            "newString": "beta",
            "replaceAll": true,
        }),
    )
    .await
    .unwrap();
    assert_eq!(edited.metadata.unwrap()["replaced"], 2);
    let patched = execute(
        "apply_patch",
        context.clone(),
        json!({
            "patchText": "\
*** Begin Patch
*** Update File: notes.txt
@@
-beta beta
+gamma gamma
*** End Patch"
        }),
    )
    .await
    .unwrap();
    assert!(patched.output.contains("notes.txt"));
    let read = execute("read", context, json!({ "filePath": "notes.txt" }))
        .await
        .unwrap();
    assert!(read.output.contains("<type>file</type>"));
    assert!(read.output.contains("1: gamma gamma"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn edit_tool_rejects_patch_text_payloads() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-edit-patch-text-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("TASK.md"), "before\n").unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("edit".to_string(), json!("allow")),
    ]));

    let error = execute(
        "edit",
        context,
        json!({
            "patchText": "\
*** Begin Patch
*** Update File: TASK.md
@@
-before
+after
*** End Patch"
        }),
    )
    .await
    .unwrap_err();

    assert!(error
        .to_string()
        .contains("tool argument filePath is required"));
    assert_eq!(
        std::fs::read_to_string(root.join("TASK.md")).unwrap(),
        "before\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v4a_patch_does_not_partially_apply_when_later_file_fails() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-atomic-v4a-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("first.txt"), "before\n").unwrap();
    std::fs::write(root.join("second.txt"), "current\n").unwrap();

    let error = execute(
        "apply_patch",
        allow_context(&root),
        json!({
            "patchText": "*** Begin Patch\n*** Update File: first.txt\n@@\n-before\n+after\n*** Update File: second.txt\n@@\n-stale\n+changed\n*** End Patch"
        }),
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("patch context not found:\nstale"));
    assert_eq!(
        std::fs::read_to_string(root.join("first.txt")).unwrap(),
        "before\n"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("second.txt")).unwrap(),
        "current\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn advertised_tools_use_opencode_patch_contract() {
    let tools = list();
    assert!(tools.iter().any(|tool| tool.id == "apply_patch"));
    assert!(!tools.iter().any(|tool| tool.id == "patch"));
    let edit = tools.iter().find(|tool| tool.id == "edit").unwrap();
    assert_eq!(
        edit.parameters["required"],
        json!(["filePath", "oldString", "newString"])
    );
    let apply_patch = tools.iter().find(|tool| tool.id == "apply_patch").unwrap();
    assert_eq!(apply_patch.parameters["required"], json!(["patchText"]));
    assert!(apply_patch.parameters["properties"].get("patch").is_none());
}

#[tokio::test]
async fn write_tool_obeys_edit_permission() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tools-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("edit".to_string(), json!("deny")),
    ]));

    let error = execute(
        "write",
        context,
        json!({ "path": "notes.txt", "content": "blocked" }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("permission edit"));
    assert!(!root.join("notes.txt").exists());

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn skill_tool_loads_project_skill_content() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-skill-tool-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let skill_dir = root.join(".neoism/skills/review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: review\ndescription: Review code changes\n---\nFocus on bugs and tests.\n",
    )
    .unwrap();
    std::fs::write(skill_dir.join("checklist.md"), "Look for regressions.\n").unwrap();

    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("skill".to_string(), json!("allow")),
    ]));
    let result = execute("skill", context, json!({ "name": "review" }))
        .await
        .unwrap();

    assert_eq!(result.title, "Loaded skill review");
    assert!(result.output.contains("<skill_content name=\"review\">"));
    assert!(result.output.contains("Focus on bugs and tests."));
    assert!(result.output.contains("<skill_files>"));
    assert!(result.output.contains("checklist.md"));
    assert_eq!(result.metadata.unwrap()["skill"]["name"], "review");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn skill_tool_obeys_skill_permission() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-skill-tool-deny-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    let skill_dir = root.join(".neoism/skills/review");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "Review carefully.\n").unwrap();

    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("skill".to_string(), json!("deny")),
    ]));
    let error = execute("skill", context, json!({ "name": "review" }))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("tool permission skill"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn lsp_tool_reports_workspace_status() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-lsp-tool-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(root.join("src/lib.rs"), "pub fn demo() {}\n").unwrap();

    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("lsp".to_string(), json!("allow")),
    ]));
    let result = execute("lsp", context, json!({ "operation": "status" }))
        .await
        .unwrap();

    assert_eq!(result.title, "LSP status");
    assert!(result.output.contains("rust"));
    assert_eq!(result.metadata.unwrap()["lsp"]["operation"], "status");
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn lsp_tool_obeys_lsp_permission() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-lsp-tool-deny-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let context = ToolContext::new(&root).with_permissions(BTreeMap::from([
        ("*".to_string(), json!("allow")),
        ("lsp".to_string(), json!("deny")),
    ]));

    let error = execute("lsp", context, json!({ "operation": "status" }))
        .await
        .unwrap_err();

    assert!(error.to_string().contains("tool permission lsp"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn lsp_tool_checks_external_directory_permission_for_file_operations() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-lsp-external-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let external_dir = std::env::temp_dir().join(format!(
        "neoism-agent-lsp-external-file-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let external = external_dir.join("lib.rs");
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(&external_dir);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::create_dir_all(&external_dir).unwrap();
    std::fs::write(&external, "pub fn outside() {}\n").unwrap();

    let error = execute(
        "lsp",
        ToolContext::new(&root)
            .with_permissions(BTreeMap::from([("lsp".to_string(), json!("allow"))])),
        json!({ "operation": "documentSymbol", "file": external.to_string_lossy() }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("external_directory"));

    let pattern = format!("{}/*", external_dir.display());
    let mut external_rules = serde_json::Map::new();
    external_rules.insert(pattern, json!("allow"));
    let result = execute(
        "lsp",
        ToolContext::new(&root).with_permissions(BTreeMap::from([
            ("lsp".to_string(), json!("allow")),
            (
                "external_directory".to_string(),
                serde_json::Value::Object(external_rules),
            ),
        ])),
        json!({ "operation": "documentSymbol", "file": external.to_string_lossy() }),
    )
    .await
    .unwrap();
    assert_eq!(result.title, "LSP document symbols");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_dir);
}
