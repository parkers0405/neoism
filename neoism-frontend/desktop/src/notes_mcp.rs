use std::io::{self, BufRead, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use neoism_agent_service_api::{
    AgentServices, ConfigLayer, ConfigSnapshot, ConfigSnapshotRequest,
    ConfigSourceService, ConfigUpdateRequest, ServiceError, ServiceFuture,
};
use serde_json::{json, Value};

const INTERNAL_ARG: &str = "--neoism-notes-mcp";
const CONFIG_SOURCE: &str = "neoism:desktop-notes-mcp";

#[doc(hidden)]
pub fn maybe_run() -> io::Result<bool> {
    if !std::env::args().any(|argument| argument == INTERNAL_ARG) {
        return Ok(false);
    }
    serve_stdio()?;
    Ok(true)
}

pub fn install(services: AgentServices) -> AgentServices {
    let Ok(executable) = std::env::current_exe() else {
        tracing::warn!("desktop Notes MCP unavailable: current executable path could not be resolved");
        return services;
    };
    install_with_executable(services, executable)
}

#[doc(hidden)]
pub fn install_with_executable(
    mut services: AgentServices,
    executable: impl AsRef<Path>,
) -> AgentServices {
    services.config = Arc::new(DesktopNotesConfig {
        inner: services.config.clone(),
        executable: executable.as_ref().to_string_lossy().into_owned(),
    });
    services
}

struct DesktopNotesConfig {
    inner: Arc<dyn ConfigSourceService>,
    executable: String,
}

impl DesktopNotesConfig {
    fn decorate(&self, mut snapshot: ConfigSnapshot) -> ConfigSnapshot {
        let layer = ConfigLayer {
            source_id: CONFIG_SOURCE.into(),
            document: json!({
                "mcp": {
                    "notes": {
                        "type": "local",
                        "command": [self.executable, INTERNAL_ARG],
                        "enabled": true
                    }
                },
                "agent": {
                    "plan": {
                        "permission": {
                            "mcp": {
                                "mcp__notes__list": "allow",
                                "mcp__notes__search": "allow",
                                "mcp__notes__read": "allow",
                                "mcp__notes__tasks": "allow",
                                "mcp__notes__create": "deny",
                                "mcp__notes__write": "deny",
                                "mcp__notes__taskToggle": "deny"
                            }
                        }
                    }
                }
            }),
            writable: false,
        };
        snapshot.identity.push_str("\0");
        snapshot.identity.push_str(CONFIG_SOURCE);
        snapshot.identity.push_str("\0");
        snapshot.identity.push_str(&layer.document.to_string());
        snapshot.layers.push(layer);
        snapshot
    }
}

impl ConfigSourceService for DesktopNotesConfig {
    fn snapshot(
        &self,
        request: &ConfigSnapshotRequest,
    ) -> Result<ConfigSnapshot, ServiceError> {
        self.inner
            .snapshot(request)
            .map(|snapshot| self.decorate(snapshot))
    }

    fn update<'a>(
        &'a self,
        request: &'a ConfigUpdateRequest,
    ) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async move {
            let snapshot = self.inner.update(request).await?;
            Ok(self.decorate(snapshot))
        })
    }
}

fn serve_stdio() -> io::Result<()> {
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout = stdout.lock();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(request) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = request.get("id").cloned() else {
            continue;
        };
        let response = match handle_request(
            request.get("method").and_then(Value::as_str).unwrap_or(""),
            request.get("params").cloned().unwrap_or_else(|| json!({})),
        ) {
            Ok(result) => json!({"jsonrpc":"2.0","id":id,"result":result}),
            Err(error) => {
                json!({"jsonrpc":"2.0","id":id,"error":{"code":error.code,"message":error.message}})
            }
        };
        serde_json::to_writer(&mut stdout, &response)?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
    }
    Ok(())
}

#[derive(Debug)]
struct RpcError {
    code: i32,
    message: String,
}

fn handle_request(method: &str, params: Value) -> Result<Value, RpcError> {
    match method {
        "initialize" => Ok(json!({
            "protocolVersion": "2024-11-05",
            "capabilities": {"tools": {}},
            "serverInfo": {"name":"neoism-notes","version":env!("CARGO_PKG_VERSION")}
        })),
        "tools/list" => Ok(json!({"tools": tool_definitions()})),
        "tools/call" => {
            let name = required_string(&params, "name").map_err(invalid_params)?;
            let result = call_tool(
                &name,
                params
                    .get("arguments")
                    .cloned()
                    .unwrap_or_else(|| json!({})),
            );
            let (text, is_error) = match result {
                Ok(text) => (text, false),
                Err(error) => (error, true),
            };
            Ok(json!({"content":[{"type":"text","text":text}],"isError":is_error}))
        }
        "resources/list" => Ok(json!({"resources":[]})),
        "prompts/list" => Ok(json!({"prompts":[]})),
        _ => Err(RpcError {
            code: -32601,
            message: format!("unknown MCP method {method}"),
        }),
    }
}

fn invalid_params(message: String) -> RpcError {
    RpcError {
        code: -32602,
        message,
    }
}

fn tool_definitions() -> Vec<Value> {
    vec![
        tool(
            "list",
            "List Markdown files in the workspace's linked Neoism Notes vault",
            json!({"type":"object","properties":{"limit":{"type":"integer","minimum":1}}}),
        ),
        tool(
            "search",
            "Search the workspace's linked Neoism Notes vault",
            json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1}},"required":["query"]}),
        ),
        tool(
            "read",
            "Read a note by vault-relative path",
            json!({"type":"object","properties":{"path":{"type":"string"}},"required":["path"]}),
        ),
        tool(
            "create",
            "Create a Markdown note in the linked vault",
            json!({"type":"object","properties":{"title":{"type":"string"},"content":{"type":"string"}},"required":["title"]}),
        ),
        tool(
            "write",
            "Write or replace a note by vault-relative path",
            json!({"type":"object","properties":{"path":{"type":"string"},"content":{"type":"string"}},"required":["path","content"]}),
        ),
        tool(
            "tasks",
            "List Markdown tasks in the linked vault",
            json!({"type":"object","properties":{"limit":{"type":"integer","minimum":1}}}),
        ),
        tool(
            "taskToggle",
            "Set or toggle a Markdown task by path and one-based line",
            json!({"type":"object","properties":{"path":{"type":"string"},"line":{"type":"integer","minimum":1},"checked":{"type":"boolean"}},"required":["path","line"]}),
        ),
    ]
}

fn tool(name: &str, description: &str, input_schema: Value) -> Value {
    json!({"name":name,"description":description,"inputSchema":input_schema})
}

fn call_tool(name: &str, arguments: Value) -> Result<String, String> {
    let workspace = std::env::current_dir().map_err(|error| error.to_string())?;
    let notes = Notes::for_workspace(&workspace)?;
    call_notes_tool(&notes, name, arguments)
}

fn call_notes_tool(
    notes: &Notes,
    name: &str,
    arguments: Value,
) -> Result<String, String> {
    let limit = arguments
        .get("limit")
        .and_then(Value::as_u64)
        .unwrap_or(100)
        .max(1) as usize;
    match name {
        "list" => Ok(notes.files(limit)?.join("\n")),
        "search" => {
            let query = required_string(&arguments, "query")?.to_lowercase();
            let mut hits = Vec::new();
            for path in notes.files(10_000)? {
                let text = std::fs::read_to_string(notes.path(&path)?)
                    .map_err(|error| error.to_string())?;
                for (line, content) in text.lines().enumerate() {
                    if content.to_lowercase().contains(&query) {
                        hits.push(format!("{path}:{} {}", line + 1, content.trim()));
                        if hits.len() >= limit {
                            return Ok(hits.join("\n"));
                        }
                    }
                }
            }
            Ok(hits.join("\n"))
        }
        "read" => {
            let path = required_string_alias(&arguments, "path", "note")?;
            std::fs::read_to_string(notes.path(&path)?).map_err(|error| error.to_string())
        }
        "create" => {
            let title = required_string_alias(&arguments, "title", "query")?;
            let path = notes.root.join(safe_note_file_name(&title));
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            let content = arguments
                .get("content")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| format!("# {}\n", title.trim()));
            std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
                .and_then(|mut file| file.write_all(content.as_bytes()))
                .map_err(|error| error.to_string())?;
            Ok(path
                .strip_prefix(&notes.root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/"))
        }
        "write" => {
            let path = required_string_alias(&arguments, "path", "note")?;
            let absolute = notes.path(&path)?;
            if let Some(parent) = absolute.parent() {
                std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            }
            std::fs::write(&absolute, required_string(&arguments, "content")?)
                .map_err(|error| error.to_string())?;
            Ok(format!("Wrote {path}"))
        }
        "tasks" => {
            let mut tasks = Vec::new();
            for path in notes.files(10_000)? {
                let text = std::fs::read_to_string(notes.path(&path)?)
                    .map_err(|error| error.to_string())?;
                for (line, content) in text.lines().enumerate() {
                    let trimmed = content.trim_start();
                    if trimmed.starts_with("- [ ]")
                        || trimmed.starts_with("- [x]")
                        || trimmed.starts_with("- [X]")
                    {
                        tasks.push(format!("{path}:{} {}", line + 1, trimmed));
                        if tasks.len() >= limit {
                            return Ok(tasks.join("\n"));
                        }
                    }
                }
            }
            Ok(tasks.join("\n"))
        }
        "taskToggle" => {
            let path = required_string_alias(&arguments, "path", "note")?;
            let line = arguments
                .get("line")
                .and_then(Value::as_u64)
                .filter(|line| *line > 0)
                .ok_or_else(|| "line is required".to_string())?
                as usize;
            let absolute = notes.path(&path)?;
            let original =
                std::fs::read_to_string(&absolute).map_err(|error| error.to_string())?;
            let trailing_newline = original.ends_with('\n');
            let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
            let target = lines
                .get_mut(line - 1)
                .ok_or_else(|| "task line is outside the note".to_string())?;
            let checked = arguments
                .get("checked")
                .and_then(Value::as_bool)
                .unwrap_or(!target.contains("[x]") && !target.contains("[X]"));
            let marker = if target.contains("[ ]") {
                "[ ]"
            } else if target.contains("[x]") {
                "[x]"
            } else if target.contains("[X]") {
                "[X]"
            } else {
                return Err("line is not a Markdown task".into());
            };
            *target = target.replacen(marker, if checked { "[x]" } else { "[ ]" }, 1);
            let mut content = lines.join("\n");
            if trailing_newline {
                content.push('\n');
            }
            std::fs::write(absolute, content).map_err(|error| error.to_string())?;
            Ok(format!("{path}:{line}"))
        }
        _ => Err(format!("unknown Notes tool {name}")),
    }
}

struct Notes {
    root: PathBuf,
}

impl Notes {
    fn for_workspace(workspace: &Path) -> Result<Self, String> {
        let workspace = neoism_workspace_index::linked_project_for_code_dir(workspace)
            .map_err(|error| error.to_string())?
            .unwrap_or_else(neoism_workspace_index::default_notes_workspace);
        Ok(Self {
            root: workspace.notes_workspace_dir(),
        })
    }

    fn path(&self, raw: &str) -> Result<PathBuf, String> {
        let relative = Path::new(raw);
        if relative.is_absolute()
            || relative.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err("note path must stay inside the selected vault".into());
        }
        Ok(self.root.join(relative))
    }

    fn files(&self, limit: usize) -> Result<Vec<String>, String> {
        let mut files = Vec::new();
        collect_markdown_files(&self.root, &self.root, &mut files, limit.max(1))?;
        Ok(files)
    }
}

fn collect_markdown_files(
    root: &Path,
    directory: &Path,
    out: &mut Vec<String>,
    limit: usize,
) -> Result<(), String> {
    if out.len() >= limit {
        return Ok(());
    }
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Ok(());
    };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit {
            break;
        }
        let path = entry.path();
        let file_type = entry.file_type().map_err(|error| error.to_string())?;
        if file_type.is_dir() {
            collect_markdown_files(root, &path, out, limit)?;
        } else if file_type.is_file()
            && path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| ext.eq_ignore_ascii_case("md"))
        {
            out.push(
                path.strip_prefix(root)
                    .unwrap_or(&path)
                    .to_string_lossy()
                    .replace('\\', "/"),
            );
        }
    }
    Ok(())
}

fn required_string(arguments: &Value, key: &str) -> Result<String, String> {
    arguments
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{key} is required"))
}

fn required_string_alias(
    arguments: &Value,
    key: &str,
    alias: &str,
) -> Result<String, String> {
    required_string(arguments, key).or_else(|_| {
        required_string(arguments, alias).map_err(|_| format!("{key} is required"))
    })
}

fn safe_note_file_name(title: &str) -> String {
    let stem = title
        .trim()
        .chars()
        .map(|ch| {
            if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '-'
            } else {
                ch
            }
        })
        .collect::<String>()
        .trim_matches(|ch| ch == '-' || ch == ' ')
        .to_string();
    format!("{}.md", if stem.is_empty() { "Note" } else { &stem })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advertises_the_complete_notes_surface() {
        assert_eq!(
            tool_definitions()
                .iter()
                .filter_map(|tool| tool["name"].as_str())
                .collect::<Vec<_>>(),
            [
                "list",
                "search",
                "read",
                "create",
                "write",
                "tasks",
                "taskToggle"
            ]
        );
    }

    #[test]
    fn rejects_paths_outside_the_vault() {
        let notes = Notes {
            root: PathBuf::from("/tmp/notes"),
        };
        assert!(notes.path("../secret.md").is_err());
        assert!(notes.path("nested/note.md").is_ok());
    }

    #[test]
    fn model_can_create_read_write_search_and_toggle_notes() {
        let root =
            std::env::temp_dir().join(format!("neoism-notes-mcp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let notes = Notes { root: root.clone() };

        assert_eq!(
            call_notes_tool(
                &notes,
                "create",
                json!({"title":"Roadmap","content":"# Roadmap\n- [ ] Ship MCP\n"})
            )
            .unwrap(),
            "Roadmap.md"
        );
        assert!(call_notes_tool(&notes, "list", json!({}))
            .unwrap()
            .contains("Roadmap.md"));
        assert!(
            call_notes_tool(&notes, "search", json!({"query":"Ship MCP"}))
                .unwrap()
                .contains("Roadmap.md:2")
        );
        assert!(call_notes_tool(&notes, "tasks", json!({}))
            .unwrap()
            .contains("- [ ] Ship MCP"));
        assert_eq!(
            call_notes_tool(&notes, "taskToggle", json!({"path":"Roadmap.md","line":2}))
                .unwrap(),
            "Roadmap.md:2"
        );
        assert!(
            call_notes_tool(&notes, "read", json!({"path":"Roadmap.md"}))
                .unwrap()
                .contains("- [x] Ship MCP")
        );
        assert_eq!(
            call_notes_tool(
                &notes,
                "write",
                json!({"path":"Roadmap.md","content":"# Updated\n"})
            )
            .unwrap(),
            "Wrote Roadmap.md"
        );
        assert_eq!(
            std::fs::read_to_string(root.join("Roadmap.md")).unwrap(),
            "# Updated\n"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_legacy_note_and_query_argument_aliases() {
        let root = std::env::temp_dir()
            .join(format!("neoism-notes-mcp-aliases-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let notes = Notes { root: root.clone() };

        assert_eq!(
            call_notes_tool(&notes, "create", json!({"query":"Legacy"})).unwrap(),
            "Legacy.md"
        );
        assert!(call_notes_tool(&notes, "read", json!({"note":"Legacy.md"}))
            .unwrap()
            .contains("# Legacy"));
        assert_eq!(
            call_notes_tool(
                &notes,
                "write",
                json!({"note":"Legacy.md","content":"- [ ] Done\n"})
            )
            .unwrap(),
            "Wrote Legacy.md"
        );
        assert_eq!(
            call_notes_tool(&notes, "taskToggle", json!({"note":"Legacy.md","line":1}))
                .unwrap(),
            "Legacy.md:1"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn uses_json_rpc_method_not_found_for_unknown_methods() {
        let error = handle_request("missing/method", json!({})).unwrap_err();
        assert_eq!(error.code, -32601);
    }

    #[test]
    fn desktop_config_injects_notes_as_a_read_only_mcp() {
        let services = install(neoism_agent_neoism_adapter::neoism_services());
        let root = std::env::temp_dir()
            .join(format!("neoism-notes-config-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let snapshot = services
            .config
            .snapshot(&ConfigSnapshotRequest::new(&root))
            .unwrap();
        let layer = snapshot.layers.last().unwrap();
        assert_eq!(layer.source_id, CONFIG_SOURCE);
        assert!(!layer.writable);
        assert_eq!(layer.document["mcp"]["notes"]["type"], "local");
        assert_eq!(layer.document["mcp"]["notes"]["command"][1], INTERNAL_ARG);
        let _ = std::fs::remove_dir_all(root);
    }
}
