use std::path::{Path, PathBuf};

use neoism_agent_service_api::{
    ConfigDiscoveryRoot, ConfigLayer, ConfigSnapshot, ConfigSnapshotRequest,
    ConfigSourceService, ConfigUpdate, ConfigUpdateRequest, ConfigWritableTarget,
    ServiceError, ServiceFuture,
};
use serde_json::{json, Value};

const GUI_SOURCE: &str = "neoism:user-gui";
const MCP_SOURCE: &str = "neoism:user-mcp";
const PROJECT_SOURCE: &str = "neoism:project";

#[derive(Clone, Debug)]
pub struct NeoismConfigSourceService {
    gui_path: PathBuf,
}

impl NeoismConfigSourceService {
    pub fn new() -> Self {
        Self { gui_path: neoism_extensions::agent_config::agent_config_path() }
    }

    #[cfg(test)]
    fn at(gui_path: PathBuf) -> Self { Self { gui_path } }

    fn user_root(&self) -> PathBuf {
        self.gui_path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf()
    }

    fn project_root(workspace: &Path) -> PathBuf {
        let mut command = std::process::Command::new("git");
        command.args(["rev-parse", "--show-toplevel"]).current_dir(workspace);
        command.output().ok().filter(|output| output.status.success()).and_then(|output| {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            (!path.is_empty()).then(|| PathBuf::from(path))
        }).unwrap_or_else(|| workspace.to_path_buf())
    }

    fn read(path: &Path) -> Result<Value, ServiceError> {
        if !path.is_file() { return Ok(json!({})); }
        let text = std::fs::read_to_string(path)?;
        parse_jsonc(&text).map_err(|error| ServiceError::new(format!("failed to parse {}: {error}", path.display())))
    }

    fn project_gui(value: &Value) -> Value {
        let mut agent = value.get("agent").and_then(Value::as_object).cloned().unwrap_or_default();
        if !agent.contains_key("shell") {
            if let Some(shell) = value.get("terminal").and_then(|terminal| terminal.get("shell")) {
                agent.insert("shell".into(), shell.clone());
            }
        }
        Value::Object(agent)
    }

    fn mcp_document(value: &Value) -> Value {
        let map = value.get("mcp").cloned().unwrap_or_else(|| json!({}));
        json!({ "mcp": map })
    }

    fn update_path(&self, workspace: &Path, source_id: &str) -> Result<(PathBuf, Vec<String>), ServiceError> {
        let project = Self::project_root(workspace);
        match source_id {
            GUI_SOURCE => Ok((self.gui_path.clone(), vec!["agent".into()])),
            // mcp.json remains adapter-owned because the Neoism extensions
            // manager currently persists its canonical MCP catalog there.
            MCP_SOURCE => Ok((self.user_root().join("mcp.json"), Vec::new())),
            // neoism.json is retained only for existing Neoism project config;
            // standalone Agent uses .agent/agent.json and never sees this name.
            PROJECT_SOURCE => Ok((project.join("neoism.json"), Vec::new())),
            _ => Err(ServiceError::new(format!("config source `{source_id}` is not writable"))),
        }
    }
}

impl Default for NeoismConfigSourceService {
    fn default() -> Self { Self::new() }
}

impl ConfigSourceService for NeoismConfigSourceService {
    fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
        let workspace = if request.workspace.is_absolute() { request.workspace.clone() } else {
            std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")).join(&request.workspace)
        };
        let project = Self::project_root(&workspace);
        let gui = Self::read(&self.gui_path)?;
        let mcp = Self::read(&self.user_root().join("mcp.json"))?;
        let project_document = Self::read(&project.join("neoism.json"))?;
        let layers = vec![
            ConfigLayer { source_id: GUI_SOURCE.into(), document: Self::project_gui(&gui), writable: true },
            ConfigLayer { source_id: MCP_SOURCE.into(), document: Self::mcp_document(&mcp), writable: true },
            ConfigLayer { source_id: PROJECT_SOURCE.into(), document: project_document, writable: true },
        ];
        let identity = layers.iter().map(|layer| format!("{}\0{}", layer.source_id, layer.document)).collect::<Vec<_>>().join("\0");
        Ok(ConfigSnapshot {
            identity,
            workspace,
            layers,
            discovery_roots: vec![
                ConfigDiscoveryRoot { source_id: "neoism:user-root".into(), path: self.user_root() },
                ConfigDiscoveryRoot { source_id: "neoism:project-root".into(), path: project.join(".neoism") },
            ],
            writable_target: ConfigWritableTarget { source_id: PROJECT_SOURCE.into(), label: "Neoism project Agent config".into() },
        })
    }

    fn update<'a>(&'a self, request: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async move {
            let (path, prefix) = self.update_path(&request.workspace, &request.source_id)?;
            let mut document = Self::read(&path)?;
            match &request.update {
                ConfigUpdate::ReplaceDocument { document: replacement } if prefix.is_empty() => document = replacement.clone(),
                ConfigUpdate::ReplaceDocument { document: replacement } => set_value(&mut document, &prefix, replacement.clone()),
                ConfigUpdate::SetValue { path, value } => {
                    let mut full = prefix;
                    full.extend(path.iter().cloned());
                    set_value(&mut document, &full, value.clone());
                }
            }
            if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
            let temp = path.with_extension("tmp");
            std::fs::write(&temp, format!("{}\n", serde_json::to_string_pretty(&document).map_err(|error| ServiceError::new(error.to_string()))?))?;
            std::fs::rename(temp, path)?;
            self.snapshot(&ConfigSnapshotRequest::new(&request.workspace))
        })
    }
}

fn set_value(document: &mut Value, path: &[String], value: Value) {
    if path.is_empty() { *document = value; return; }
    let mut current = document;
    for component in &path[..path.len() - 1] {
        if !current.is_object() { *current = json!({}); }
        current = current.as_object_mut().expect("object initialized").entry(component).or_insert_with(|| json!({}));
    }
    if !current.is_object() { *current = json!({}); }
    current.as_object_mut().expect("object initialized").insert(path.last().unwrap().clone(), value);
}

fn parse_jsonc(text: &str) -> Result<Value, serde_json::Error> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if string {
            out.push(ch);
            if ch == '"' && !escaped { string = false; }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' { escaped = false; }
        } else if ch == '"' { string = true; out.push(ch); }
        else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next(); for next in chars.by_ref() { if next == '\n' { out.push('\n'); break; } }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next(); let mut prior = '\0'; for next in chars.by_ref() { if prior == '*' && next == '/' { break; } prior = next; }
        } else { out.push(ch); }
    }
    serde_json::from_str(&strip_trailing_commas(&out))
}

fn strip_trailing_commas(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if string {
            out.push(ch);
            if ch == '"' && !escaped { string = false; }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' { escaped = false; }
        } else if ch == '"' { string = true; out.push(ch); }
        else if ch == ',' {
            let mut rest = chars.clone();
            while rest.peek().is_some_and(|next| next.is_whitespace()) { rest.next(); }
            if !matches!(rest.peek(), Some('}' | ']')) { out.push(ch); }
        } else { out.push(ch); }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_only_agent_and_terminal_shell() {
        let root = std::env::temp_dir().join(format!("neoism-config-adapter-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let gui = root.join("config.json");
        std::fs::write(&gui, r#"{"appearance":{"theme":"x"},"terminal":{"shell":"fish"},"agent":{"model":"p/m"}}"#).unwrap();
        let service = NeoismConfigSourceService::at(gui);
        let snapshot = service.snapshot(&ConfigSnapshotRequest::new(&root)).unwrap();
        assert_eq!(snapshot.layers[0].document["model"], "p/m");
        assert_eq!(snapshot.layers[0].document["shell"], "fish");
        assert!(snapshot.layers[0].document.get("terminal").is_none());
        assert!(snapshot.layers[0].document.get("appearance").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn projected_updates_preserve_gui_groups() {
        let root = std::env::temp_dir().join(format!("neoism-config-adapter-write-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let gui = root.join("config.json");
        std::fs::write(&gui, r#"{"appearance":{"theme":"x"},"agent":{"model":"old/model"}}"#).unwrap();
        let service = NeoismConfigSourceService::at(gui.clone());
        service.update(&ConfigUpdateRequest {
            workspace: root.clone(), source_id: GUI_SOURCE.into(),
            update: ConfigUpdate::SetValue { path: vec!["model".into()], value: json!("new/model") },
        }).await.unwrap();
        let written: Value = serde_json::from_str(&std::fs::read_to_string(gui).unwrap()).unwrap();
        assert_eq!(written["appearance"]["theme"], "x");
        assert_eq!(written["agent"]["model"], "new/model");
        let _ = std::fs::remove_dir_all(root);
    }
}