use std::collections::BTreeMap;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;

use super::web::render_web_body;
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
        json!({ "filePath": "src/lib.rs", "limit": 1 }),
    )
    .await
    .unwrap();
    assert!(read.output.contains("<type>file</type>"));
    assert!(read.output.contains("1: fn main() {}"));

    std::fs::write(root.join("src/other.rs"), "fn other() {}\n").unwrap();
    let listed = execute("read", context.clone(), json!({ "filePath": "." }))
        .await
        .unwrap();
    assert!(listed.output.contains("<type>directory</type>"));
    assert!(listed.output.contains("src/"));

    let grep = execute("grep", context.clone(), json!({ "pattern": "needle" }))
        .await
        .unwrap();
    assert!(grep.output.contains("src/lib.rs:"));
    assert!(grep.output.contains("Line 2"));

    let scoped = execute(
        "grep",
        context.clone(),
        json!({ "pattern": "needle", "path": "src/lib.rs" }),
    )
    .await
    .unwrap();
    assert!(scoped.output.contains("src/lib.rs:"));

    let glob = execute("glob", context, json!({ "pattern": "*.rs" }))
        .await
        .unwrap();
    assert!(glob.output.contains("src/lib.rs"));
    assert!(glob.output.contains("src/other.rs"));

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn grep_and_glob_return_bounded_fff_metadata() {
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
    assert!(grep.output.contains("Grep: Found"));
    assert!(grep.output.contains("src/a.rs:"));
    assert!(!grep.output.contains("src/b.txt"));
    let metadata = grep.metadata.unwrap();
    assert_eq!(metadata["engine"], "fff");
    assert_eq!(metadata["truncated"], true);
    assert_eq!(metadata["items"].as_array().unwrap().len(), 1);
    assert_eq!(metadata["items"][0]["line"], 1);

    let glob = execute(
        "glob",
        context.clone(),
        json!({ "pattern": "a.rs", "path": "src", "limit": 1 }),
    )
    .await
    .unwrap();
    let metadata = glob.metadata.unwrap();
    assert_eq!(metadata["count"], 1);
    assert_eq!(metadata["engine"], "fff");
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
async fn canonical_search_tools_expose_fff_modes_and_multi_pattern_search() {
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
        "glob",
        context.clone(),
        json!({ "pattern": "upload", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(find.output.contains("src/upload.rs"));
    assert_eq!(find.metadata.as_ref().unwrap()["engine"], "fff");

    let grep = execute(
        "grep",
        context.clone(),
        json!({ "pattern": "PrepareUpload", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(grep.output.contains("src/upload.rs:"));
    assert!(grep.output.contains("Line 1"));
    assert_eq!(grep.metadata.as_ref().unwrap()["engine"], "fff");

    let scoped_grep = execute(
        "grep",
        context.clone(),
        json!({ "pattern": "PrepareUpload", "path": "src", "include": "*.rs", "limit": 5 }),
    )
    .await
    .unwrap();
    assert!(scoped_grep.output.contains("upload.rs:"));
    assert!(scoped_grep.output.contains("Line 1"));

    let multi = execute(
        "grep",
        context,
        json!({ "pattern": ["PrepareUpload", "prepare_upload"], "limit": 10 }),
    )
    .await
    .unwrap();
    assert!(multi.output.contains("Line 1"));
    assert!(multi.output.contains("Line 2"));
    assert_eq!(multi.metadata.as_ref().unwrap()["engine"], "fff");

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn notes_tool_delegates_to_the_injected_service_and_rejects_removed_operations() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-notes-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let notes = Arc::new(FakeNotesService { root: root.join("notes") });
    let services = crate::standard_services().with_notes(notes);
    let state = crate::state::AppState::open_database_with_services(root.join("agent.db"), services).await.unwrap();
    let context = allow_context(&root).with_state(Some(state.clone()));

    let list = execute("notes", context.clone(), json!({ "operation": "list" }))
        .await
        .unwrap();
    assert_eq!(list.output, "Roadmap.md");

    let search = execute(
        "notes",
        context.clone(),
        json!({ "operation": "search", "query": "ship notes" }),
    )
    .await
    .unwrap();
    assert!(search.output.contains("Roadmap.md:7"));

    let tasks = execute("notes", context.clone(), json!({ "operation": "tasks" }))
        .await
        .unwrap();
    assert!(tasks.output.contains("Roadmap.md:7 - [ ] ship notes"));

    let graph = execute("notes", context, json!({ "operation": "graph" }))
        .await
        .unwrap_err();
    assert!(graph.to_string().contains("unknown notes operation graph"));

    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{"plugins":{"dev.neoism.tools.notes":{"enabled":false}}}"#,
    )
    .unwrap();
    let disabled = execute(
        "notes",
        allow_context(&root).with_state(Some(state)),
        json!({ "operation": "list" }),
    )
    .await
    .unwrap_err();
    assert!(disabled.to_string().contains("disabled"));

    let _ = std::fs::remove_dir_all(root);
}

struct FakeNotesService {
    root: std::path::PathBuf,
}

impl neoism_agent_service_api::NotesService for FakeNotesService {
    fn scope_choices(&self) -> Vec<neoism_agent_service_api::ScopeChoice> {
        vec![neoism_agent_service_api::ScopeChoice { id: "workspace".to_string(), label: "Workspace notes".to_string(), description: None }]
    }

    fn default_scope_id(&self) -> &str { "workspace" }
    fn tool_description(&self) -> String { "Fake injected notes".to_string() }

    fn list(&self, _request: &neoism_agent_service_api::NotesRequest, _limit: usize) -> Result<Vec<neoism_agent_service_api::ScopedNotes<String>>, neoism_agent_service_api::ServiceError> {
        Ok(vec![neoism_agent_service_api::ScopedNotes { location: self.location(), items: vec!["Roadmap.md".to_string()] }])
    }

    fn search(&self, _request: &neoism_agent_service_api::NotesRequest, _query: &str, _limit: usize) -> Result<Vec<neoism_agent_service_api::ScopedNotes<neoism_agent_service_api::NoteSearchHit>>, neoism_agent_service_api::ServiceError> {
        Ok(vec![neoism_agent_service_api::ScopedNotes { location: self.location(), items: vec![neoism_agent_service_api::NoteSearchHit { path: "Roadmap.md".to_string(), line: 7, text: "ship notes".to_string() }] }])
    }

    fn read(&self, _request: &neoism_agent_service_api::NotesRequest, path: &str) -> Result<neoism_agent_service_api::NoteDocument, neoism_agent_service_api::ServiceError> {
        Ok(self.document(path, "contents"))
    }

    fn tasks(&self, _request: &neoism_agent_service_api::NotesRequest, _limit: usize) -> Result<Vec<neoism_agent_service_api::ScopedNotes<neoism_agent_service_api::NoteTask>>, neoism_agent_service_api::ServiceError> {
        Ok(vec![neoism_agent_service_api::ScopedNotes { location: self.location(), items: vec![neoism_agent_service_api::NoteTask { path: "Roadmap.md".to_string(), line: 7, checked: false, text: "ship notes".to_string() }] }])
    }

    fn create(&self, _request: &neoism_agent_service_api::NotesRequest, title: &str, content: Option<&str>) -> Result<neoism_agent_service_api::NoteDocument, neoism_agent_service_api::ServiceError> {
        Ok(self.document(&format!("{title}.md"), content.unwrap_or_default()))
    }

    fn write(&self, _request: &neoism_agent_service_api::NotesRequest, path: &str, content: &str) -> Result<neoism_agent_service_api::NoteDocument, neoism_agent_service_api::ServiceError> {
        Ok(self.document(path, content))
    }

    fn task_toggle(&self, _request: &neoism_agent_service_api::NotesRequest, path: &str, line: usize, checked: Option<bool>) -> Result<neoism_agent_service_api::NoteTask, neoism_agent_service_api::ServiceError> {
        Ok(neoism_agent_service_api::NoteTask { path: path.to_string(), line, checked: checked.unwrap_or(true), text: "ship notes".to_string() })
    }
}

impl FakeNotesService {
    fn location(&self) -> neoism_agent_service_api::NotesLocation {
        neoism_agent_service_api::NotesLocation { scope_id: "workspace".to_string(), scope_label: "Workspace notes".to_string(), root: self.root.clone() }
    }

    fn document(&self, path: &str, content: &str) -> neoism_agent_service_api::NoteDocument {
        neoism_agent_service_api::NoteDocument { location: self.location(), path: path.to_string(), absolute_path: self.root.join(path), content: content.to_string() }
    }
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
        json!({ "filePath": external.to_string_lossy() }),
    )
    .await
    .unwrap_err();
    assert!(error.to_string().contains("external_directory"));

    let _ = std::fs::remove_file(external);
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
        json!({ "filePath": external.to_string_lossy() }),
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
        json!({ "filePath": "dir", "offset": 2, "limit": 2 }),
    )
    .await
    .unwrap();
    assert!(result.output.contains("<type>directory</type>"));
    assert!(result.output.contains("a.txt\nb.txt"));
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["type"], "directory");
    assert_eq!(metadata["truncated"], false);

    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_tool_uses_the_canonical_file_path_argument() {
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
async fn write_tool_creates_nested_missing_directories() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-write-nested-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    execute(
        "write",
        allow_context(&root),
        json!({ "filePath": "new/deep/file.txt", "content": "nested\n" }),
    )
    .await
    .unwrap();

    assert_eq!(
        std::fs::read_to_string(root.join("new/deep/file.txt")).unwrap(),
        "nested\n"
    );
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn read_tool_rejects_invalid_utf8_text() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-read-invalid-utf8-{}",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("invalid.txt"), [b'a', 0xff, b'\n']).unwrap();

    let error = execute(
        "read",
        allow_context(&root),
        json!({ "filePath": "invalid.txt" }),
    )
    .await
    .unwrap_err();

    assert!(format!("{error:#}").contains("not valid UTF-8"));
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
    let error = error.to_string();
    assert!(error.to_ascii_lowercase().contains("command aborted"));
    assert!(
        error.contains("started") || error.contains("(no output)"),
        "{error}"
    );
    assert!(!error.contains("finished"), "{error}");

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

    let error = execute("read", context, json!({ "filePath": "file.txt" }))
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
        json!({ "filePath": "notes.txt", "content": "hello world" }),
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
        json!({ "filePath": "notes.txt", "oldString": "world", "newString": "neoism" }),
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
*** Begin Patch
*** Update File: notes.txt
@@
-hello world
+hello neoism
*** End Patch";
    let result = execute(
        "apply_patch",
        allow_context(&root),
        json!({ "patchText": patch }),
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
fn render_web_body_strips_tags_and_caps_output() {
    let (body, truncated) =
        render_web_body(b"<html><body>Hello <b>Neoism</b></body></html>");
    assert_eq!(body, "Hello Neoism");
    assert!(!truncated);
}

#[tokio::test]
async fn file_tools_accept_canonical_arguments() {
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
        .contains("invalid tool input: $input.filePath is required"));
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
    assert!(tools.iter().all(|tool| tool.output_schema.is_some()));
    let output_schema = tools[0].output_schema.as_ref().unwrap();
    assert_eq!(output_schema["required"], json!(["title", "metadata"]));
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
    let ids = tools
        .iter()
        .map(|tool| tool.id.as_str())
        .collect::<Vec<_>>();
    for removed in [
        "read_many",
        "read_around",
        "list",
        "ffgrep",
        "fffind",
        "fff_multi_grep",
        "webfetch_batch",
        "websearch_batch",
    ] {
        assert!(
            !ids.contains(&removed),
            "removed duplicate tool {removed} was advertised"
        );
    }
    let read = tools.iter().find(|tool| tool.id == "read").unwrap();
    assert_eq!(read.parameters["required"], json!(["filePath"]));
    assert!(read.parameters["properties"].get("path").is_none());
    let grep = tools.iter().find(|tool| tool.id == "grep").unwrap();
    assert_eq!(grep.parameters["required"], json!(["pattern"]));
    for tool in tools {
        assert_eq!(tool.parameters["additionalProperties"], false);
    }
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
        json!({ "filePath": "notes.txt", "content": "blocked" }),
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
        json!({ "operation": "documentSymbol", "filePath": external.to_string_lossy() }),
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
        json!({ "operation": "documentSymbol", "filePath": external.to_string_lossy() }),
    )
    .await
    .unwrap();
    assert_eq!(result.title, "LSP document symbols");

    let _ = std::fs::remove_dir_all(root);
    let _ = std::fs::remove_dir_all(external_dir);
}
