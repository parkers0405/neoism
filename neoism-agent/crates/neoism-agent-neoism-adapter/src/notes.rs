use std::path::{Component, Path, PathBuf};

use neoism_agent_service_api::{
    BuiltinMcpCallResult, BuiltinMcpContent, BuiltinMcpService, BuiltinMcpTool,
    NoteDocument, NoteSearchHit, NoteTask, NotesLocation, NotesRequest, NotesService,
    ScopeChoice, ScopedNotes, ServiceError,
};
use serde_json::{json, Value};

const MCP_ID: &str = "neoism-notes";
const DEFAULT_SCOPE: &str = "auto";

pub(crate) struct NeoismNotesService;

impl NotesService for NeoismNotesService {
    fn scope_choices(&self) -> Vec<ScopeChoice> {
        vec![
            scope("auto", "Linked vault or Default", "Use the vault linked to the working project, otherwise Default."),
            scope("project", "Linked project", "Require the vault linked to the working project."),
            scope("vault", "Default vault", "Use the Default vault."),
            scope("all", "All vaults", "Scan every Neoism vault."),
        ]
    }

    fn default_scope_id(&self) -> &str {
        DEFAULT_SCOPE
    }

    fn tool_description(&self) -> String {
        "Neoism Markdown note-file operations: create, list, read, write, search, tasks, or taskToggle. Scope choices are advertised by the notes service.".to_string()
    }

    fn list(&self, request: &NotesRequest, limit: usize) -> Result<Vec<ScopedNotes<String>>, ServiceError> {
        workspaces(request)?
            .into_iter()
            .map(|workspace| Ok(ScopedNotes {
                location: location(request, &workspace),
                items: note_files(&workspace, limit)?,
            }))
            .collect()
    }

    fn search(&self, request: &NotesRequest, query: &str, limit: usize) -> Result<Vec<ScopedNotes<NoteSearchHit>>, ServiceError> {
        workspaces(request)?
            .into_iter()
            .map(|workspace| Ok(ScopedNotes {
                location: location(request, &workspace),
                items: search_workspace(&workspace, query, limit)?,
            }))
            .collect()
    }

    fn read(&self, request: &NotesRequest, path: &str) -> Result<NoteDocument, ServiceError> {
        let workspace = first_workspace(request)?;
        let absolute_path = safe_note_path(&workspace, path)?;
        let content = std::fs::read_to_string(&absolute_path)?;
        Ok(NoteDocument {
            location: location(request, &workspace),
            path: path.to_string(),
            absolute_path,
            content,
        })
    }

    fn tasks(&self, request: &NotesRequest, limit: usize) -> Result<Vec<ScopedNotes<NoteTask>>, ServiceError> {
        workspaces(request)?
            .into_iter()
            .map(|workspace| Ok(ScopedNotes {
                location: location(request, &workspace),
                items: tasks_workspace(&workspace, limit)?,
            }))
            .collect()
    }

    fn create(&self, request: &NotesRequest, title: &str, content: Option<&str>) -> Result<NoteDocument, ServiceError> {
        let workspace = first_workspace(request)?;
        let root = workspace.notes_workspace_dir();
        std::fs::create_dir_all(&root)?;
        let path = safe_note_file_name(title);
        let absolute_path = safe_note_path(&workspace, &path)?;
        let content = content.map(str::to_string).unwrap_or_else(|| format!("# {}\n", title.trim()));
        let mut file = std::fs::OpenOptions::new().write(true).create_new(true).open(&absolute_path)?;
        std::io::Write::write_all(&mut file, content.as_bytes())?;
        Ok(NoteDocument { location: location(request, &workspace), path, absolute_path, content })
    }

    fn write(&self, request: &NotesRequest, path: &str, content: &str) -> Result<NoteDocument, ServiceError> {
        let workspace = first_workspace(request)?;
        let absolute_path = safe_note_path(&workspace, path)?;
        if let Some(parent) = absolute_path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&absolute_path, content.as_bytes())?;
        Ok(NoteDocument {
            location: location(request, &workspace),
            path: path.to_string(),
            absolute_path,
            content: content.to_string(),
        })
    }

    fn task_toggle(&self, request: &NotesRequest, path: &str, line: usize, checked: Option<bool>) -> Result<NoteTask, ServiceError> {
        let workspace = first_workspace(request)?;
        let absolute_path = safe_note_path(&workspace, path)?;
        let original = std::fs::read_to_string(&absolute_path)?;
        let trailing_newline = original.ends_with('\n');
        let mut lines = original.lines().map(str::to_string).collect::<Vec<_>>();
        let target = lines.get_mut(line.saturating_sub(1)).ok_or_else(|| ServiceError::new("task line is outside the note"))?;
        let next_checked = checked.unwrap_or(!target.contains("[x]") && !target.contains("[X]"));
        let marker = if target.contains("[ ]") { "[ ]" } else if target.contains("[x]") { "[x]" } else if target.contains("[X]") { "[X]" } else {
            return Err(ServiceError::new("line is not a Markdown task"));
        };
        *target = target.replacen(marker, if next_checked { "[x]" } else { "[ ]" }, 1);
        let text = target.trim_start().get(5..).unwrap_or("").trim().to_string();
        let mut next = lines.join("\n");
        if trailing_newline { next.push('\n'); }
        std::fs::write(absolute_path, next)?;
        Ok(NoteTask { path: path.to_string(), line, checked: next_checked, text })
    }
}

impl BuiltinMcpService for NeoismNotesService {
    fn id(&self) -> &str { MCP_ID }

    fn tools(&self) -> Vec<BuiltinMcpTool> {
        let choices = self.scope_choices();
        let scope = json!({
            "type": "string",
            "enum": choices.iter().map(|choice| choice.id.as_str()).collect::<Vec<_>>(),
            "oneOf": choices.iter().map(|choice| json!({
                "const": choice.id,
                "title": choice.label,
                "description": choice.description,
            })).collect::<Vec<_>>(),
        });
        vec![
            mcp_tool("notes.list", "List note files", json!({"type":"object","properties":{"scope":scope.clone(),"limit":{"type":"integer"}}})),
            mcp_tool("notes.search", "Search note files", json!({"type":"object","properties":{"query":{"type":"string"},"scope":scope.clone(),"limit":{"type":"integer"}},"required":["query"]})),
            mcp_tool("notes.read", "Read a note by scope-relative path", json!({"type":"object","properties":{"path":{"type":"string"},"scope":scope},"required":["path"]})),
            mcp_tool("notes.tasks", "List Markdown tasks directly from note files", json!({"type":"object","properties":{"scope":{"type":"string","enum":["auto","project","vault","all"]},"limit":{"type":"integer"}}})),
        ]
    }

    fn call_tool(&self, working_directory: &Path, tool: &str, arguments: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
        let request = request(working_directory, &arguments);
        let limit = arguments.get("limit").and_then(Value::as_u64).unwrap_or(100).max(1) as usize;
        let output = match tool {
            "notes.list" => json!({"notes": self.list(&request, limit)?.into_iter().map(scoped_strings_json).collect::<Vec<_>>() }),
            "notes.search" => {
                let query = required_string(&arguments, "query")?;
                json!({"query":query,"hits":self.search(&request, &query, limit)?.into_iter().map(scoped_hits_json).collect::<Vec<_>>()})
            }
            "notes.read" => {
                let path = required_string(&arguments, "path")?;
                let doc = self.read(&request, &path)?;
                json!({"path":doc.path,"absolutePath":doc.absolute_path,"text":doc.content})
            }
            "notes.tasks" => json!({"tasks":self.tasks(&request, limit)?.into_iter().map(scoped_tasks_json).collect::<Vec<_>>() }),
            other => return Err(ServiceError::new(format!("unknown notes MCP tool {other}"))),
        };
        text_result(output)
    }
}

fn request(cwd: &Path, arguments: &Value) -> NotesRequest {
    let mut request = NotesRequest::new(cwd);
    if let Some(scope) = arguments.get("scope").and_then(Value::as_str) {
        request.scope_id = Some(scope.to_string());
    }
    request
}

fn scope(id: &str, label: &str, description: &str) -> ScopeChoice {
    ScopeChoice { id: id.to_string(), label: label.to_string(), description: Some(description.to_string()) }
}

fn selected_scope(request: &NotesRequest) -> &str {
    request.scope_id.as_deref().unwrap_or(DEFAULT_SCOPE)
}

fn workspaces(request: &NotesRequest) -> Result<Vec<neoism_workspace_index::config::NeoismWorkspace>, ServiceError> {
    match selected_scope(request) {
        "project" => Ok(neoism_workspace_index::linked_project_for_code_dir(&request.working_directory).map_err(service_error)?.into_iter().collect()),
        "vault" => Ok(vec![neoism_workspace_index::default_notes_workspace()]),
        "all" => {
            let mut workspaces = neoism_workspace_index::existing_notes_vaults().map_err(service_error)?
                .into_iter().map(|vault| neoism_workspace_index::config::vault_notes_workspace(&vault.name)).collect::<Vec<_>>();
            if workspaces.is_empty() { workspaces.push(neoism_workspace_index::default_notes_workspace()); }
            Ok(workspaces)
        }
        "auto" => Ok(vec![neoism_workspace_index::linked_project_for_code_dir(&request.working_directory).map_err(service_error)?
            .unwrap_or_else(neoism_workspace_index::default_notes_workspace)]),
        other => Err(ServiceError::new(format!("unknown notes scope {other}"))),
    }
}

fn first_workspace(request: &NotesRequest) -> Result<neoism_workspace_index::config::NeoismWorkspace, ServiceError> {
    workspaces(request)?.into_iter().next().ok_or_else(|| ServiceError::new("no notes scope is available"))
}

fn location(request: &NotesRequest, workspace: &neoism_workspace_index::config::NeoismWorkspace) -> NotesLocation {
    NotesLocation {
        scope_id: selected_scope(request).to_string(),
        scope_label: workspace.config.notes.workspace.clone(),
        root: workspace.notes_workspace_dir(),
    }
}

fn safe_note_path(workspace: &neoism_workspace_index::config::NeoismWorkspace, raw: &str) -> Result<PathBuf, ServiceError> {
    let relative = Path::new(raw);
    if relative.is_absolute() || relative.components().any(|component| matches!(component, Component::ParentDir | Component::RootDir | Component::Prefix(_))) {
        return Err(ServiceError::new("note path must stay inside the selected scope"));
    }
    Ok(workspace.notes_workspace_dir().join(relative))
}

fn note_files(workspace: &neoism_workspace_index::config::NeoismWorkspace, limit: usize) -> Result<Vec<String>, ServiceError> {
    let root = workspace.notes_workspace_dir();
    let mut files = Vec::new();
    collect_markdown_files(&root, &root, &mut files, limit.max(1))?;
    Ok(files)
}

fn collect_markdown_files(root: &Path, directory: &Path, out: &mut Vec<String>, limit: usize) -> Result<(), ServiceError> {
    if out.len() >= limit { return Ok(()); }
    let Ok(entries) = std::fs::read_dir(directory) else { return Ok(()); };
    let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
    entries.sort_by_key(|entry| entry.path());
    for entry in entries {
        if out.len() >= limit { break; }
        let path = entry.path();
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            collect_markdown_files(root, &path, out, limit)?;
        } else if file_type.is_file() && path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("md")) {
            out.push(path.strip_prefix(root).unwrap_or(&path).to_string_lossy().replace('\\', "/"));
        }
    }
    Ok(())
}

fn search_workspace(workspace: &neoism_workspace_index::config::NeoismWorkspace, query: &str, limit: usize) -> Result<Vec<NoteSearchHit>, ServiceError> {
    let query = query.to_lowercase();
    let mut hits = Vec::new();
    for relative in note_files(workspace, 10_000)? {
        let Ok(text) = std::fs::read_to_string(safe_note_path(workspace, &relative)?) else { continue; };
        for (line, content) in text.lines().enumerate() {
            if content.to_lowercase().contains(&query) {
                hits.push(NoteSearchHit { path: relative.clone(), line: line + 1, text: content.trim().to_string() });
                if hits.len() >= limit { return Ok(hits); }
            }
        }
    }
    Ok(hits)
}

fn tasks_workspace(workspace: &neoism_workspace_index::config::NeoismWorkspace, limit: usize) -> Result<Vec<NoteTask>, ServiceError> {
    let mut tasks = Vec::new();
    for relative in note_files(workspace, 10_000)? {
        let Ok(text) = std::fs::read_to_string(safe_note_path(workspace, &relative)?) else { continue; };
        for (line, content) in text.lines().enumerate() {
            let trimmed = content.trim_start();
            let checked = if trimmed.starts_with("- [ ]") { Some(false) } else if trimmed.starts_with("- [x]") || trimmed.starts_with("- [X]") { Some(true) } else { None };
            if let Some(checked) = checked {
                tasks.push(NoteTask { path: relative.clone(), line: line + 1, checked, text: trimmed.get(5..).unwrap_or("").trim().to_string() });
                if tasks.len() >= limit { return Ok(tasks); }
            }
        }
    }
    Ok(tasks)
}

fn safe_note_file_name(title: &str) -> String {
    let stem = title.trim().chars().map(|ch| if matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { ch })
        .collect::<String>().trim_matches(|ch| ch == '-' || ch == ' ').to_string();
    format!("{}.md", if stem.is_empty() { "Note" } else { &stem })
}

fn mcp_tool(name: &str, description: &str, input_schema: Value) -> BuiltinMcpTool {
    BuiltinMcpTool { name: name.to_string(), description: Some(description.to_string()), input_schema, annotations: None }
}

fn required_string(arguments: &Value, key: &str) -> Result<String, ServiceError> {
    arguments.get(key).and_then(Value::as_str).map(str::to_string).ok_or_else(|| ServiceError::new(format!("{key} is required")))
}

fn scoped_base<T>(scoped: &ScopedNotes<T>, result: Value) -> Value {
    json!({"vault":scoped.location.scope_label,"notesRoot":scoped.location.root,"result":result})
}

fn scoped_strings_json(scoped: ScopedNotes<String>) -> Value { let result = json!(scoped.items); scoped_base(&scoped, result) }
fn scoped_hits_json(scoped: ScopedNotes<NoteSearchHit>) -> Value {
    let result = json!(scoped.items.iter().map(|hit| json!({"path":hit.path,"line":hit.line,"text":hit.text})).collect::<Vec<_>>()); scoped_base(&scoped, result)
}
fn scoped_tasks_json(scoped: ScopedNotes<NoteTask>) -> Value {
    let result = json!(scoped.items.iter().map(|task| json!({"path":task.path,"line":task.line,"checked":task.checked,"text":task.text})).collect::<Vec<_>>()); scoped_base(&scoped, result)
}

fn text_result(value: Value) -> Result<BuiltinMcpCallResult, ServiceError> {
    Ok(BuiltinMcpCallResult { content: vec![BuiltinMcpContent::Text { text: serde_json::to_string_pretty(&value).map_err(service_error)?, annotations: None }], is_error: None })
}

fn service_error(error: impl std::fmt::Display) -> ServiceError { ServiceError::new(error.to_string()) }

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_choices_are_stable_service_ids() {
        let choices = NeoismNotesService.scope_choices();
        assert_eq!(choices.iter().map(|choice| choice.id.as_str()).collect::<Vec<_>>(), ["auto", "project", "vault", "all"]);
        assert!(choices.iter().all(|choice| !choice.label.is_empty()));
    }

    #[test]
    fn neoism_default_scope_preserves_file_search_task_and_write_behavior() {
        let _lock = crate::test_env_lock().lock().unwrap_or_else(|error| error.into_inner());
        let root = std::env::temp_dir().join(format!(
            "neoism-agent-notes-adapter-{}-{}",
            std::process::id(),
            std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos(),
        ));
        let previous = std::env::var_os("NEOISM_NOTES_HOME");
        unsafe { std::env::set_var("NEOISM_NOTES_HOME", &root); }
        std::fs::create_dir_all(root.join("code")).unwrap();
        let service = NeoismNotesService;
        let request = NotesRequest::new(root.join("code"));

        service.write(&request, "Roadmap.md", "# Roadmap\n\n- [ ] ship notes\n").unwrap();
        assert_eq!(service.list(&request, 10).unwrap()[0].items, ["Roadmap.md"]);
        assert_eq!(service.search(&request, "ship notes", 10).unwrap()[0].items[0].line, 3);
        assert!(!service.tasks(&request, 10).unwrap()[0].items[0].checked);
        service.task_toggle(&request, "Roadmap.md", 3, Some(true)).unwrap();
        assert!(service.read(&request, "Roadmap.md").unwrap().content.contains("- [x] ship notes"));
        assert_eq!(service.create(&request, "Plan", None).unwrap().path, "Plan.md");

        match previous {
            Some(value) => unsafe { std::env::set_var("NEOISM_NOTES_HOME", value) },
            None => unsafe { std::env::remove_var("NEOISM_NOTES_HOME") },
        }
        let _ = std::fs::remove_dir_all(root);
    }
}