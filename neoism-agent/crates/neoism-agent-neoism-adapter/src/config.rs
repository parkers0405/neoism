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
const PROJECT_MCP_SOURCE: &str = "neoism:project-mcp";
const LEGACY_NATIVE_MCP_IDS: [&str; 3] = ["neoism-docs", "neoism-memory", "neoism-notes"];

#[derive(Clone, Debug)]
pub struct NeoismConfigSourceService {
    gui_path: PathBuf,
}

impl NeoismConfigSourceService {
    pub fn new() -> Self {
        Self {
            gui_path: neoism_extensions::agent_config::agent_config_path(),
        }
    }

    #[cfg(test)]
    fn at(gui_path: PathBuf) -> Self {
        Self { gui_path }
    }

    fn user_root(&self) -> PathBuf {
        self.gui_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .to_path_buf()
    }

    pub(crate) fn workspace_root(workspace: &Path) -> PathBuf {
        if workspace.is_absolute() {
            workspace.to_path_buf()
        } else {
            std::env::current_dir()
                .unwrap_or_else(|_| PathBuf::from("."))
                .join(workspace)
        }
    }

    fn git_root(workspace: &Path) -> PathBuf {
        let mut command = std::process::Command::new("git");
        command
            .args(["rev-parse", "--show-toplevel"])
            .current_dir(workspace);
        command
            .output()
            .ok()
            .filter(|output| output.status.success())
            .and_then(|output| {
                let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
                (!path.is_empty()).then(|| PathBuf::from(path))
            })
            .unwrap_or_else(|| workspace.to_path_buf())
    }

    fn read(path: &Path) -> Result<Value, ServiceError> {
        if !path.is_file() {
            return Ok(json!({}));
        }
        let text = std::fs::read_to_string(path)?;
        parse_jsonc(&text).map_err(|error| {
            ServiceError::new(format!("failed to parse {}: {error}", path.display()))
        })
    }

    fn write(path: &Path, document: &Value) -> Result<(), ServiceError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("tmp");
        std::fs::write(
            &temp,
            format!(
                "{}\n",
                serde_json::to_string_pretty(document)
                    .map_err(|error| ServiceError::new(error.to_string()))?
            ),
        )?;
        std::fs::rename(temp, path)?;
        Ok(())
    }

    fn migrate_project_config(workspace: &Path) -> Result<(), ServiceError> {
        let project = Self::git_root(workspace);
        if project != workspace {
            return Ok(());
        }
        let source = project.join("neoism.json");
        if !source.is_file() {
            return Ok(());
        }
        let target = workspace.join(".neoism/config.json");
        let migrated = Self::project_document(Self::read(&source)?);
        let mut document = Self::read(&target)?;
        if !document.is_object() {
            document = json!({});
        }
        let agent = document
            .as_object_mut()
            .expect("object initialized")
            .entry("agent")
            .or_insert_with(|| json!({}));
        merge_missing(agent, migrated);
        Self::write(&target, &document)?;
        std::fs::remove_file(source)?;
        Ok(())
    }

    fn project_gui(value: &Value) -> Value {
        let mut agent = value
            .get("agent")
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        canonicalize_gui_agent(&mut agent);
        if !agent.contains_key("shell") {
            if let Some(shell) = value
                .get("terminal")
                .and_then(|terminal| terminal.get("shell"))
                .and_then(project_shell)
            {
                agent.insert("shell".into(), Value::String(shell));
            }
        }
        Value::Object(agent)
    }

    fn project_document(value: Value) -> Value {
        let grouped = value.as_object().is_some_and(|root| {
            [
                "appearance",
                "editor",
                "terminal",
                "ui",
                "presence",
                "keybinds",
                "renderer",
                "developer",
                "platform",
            ]
            .iter()
            .any(|key| root.contains_key(*key))
        });
        if grouped {
            Self::project_gui(&value)
        } else {
            value
        }
    }

    fn mcp_document(value: &Value) -> Value {
        let mut map = value
            .get("mcp")
            .and_then(Value::as_object)
            .or_else(|| value.as_object())
            .cloned()
            .unwrap_or_default();
        // These product-owned services are injected in-process and exposed as
        // native tools. Ignore stale extension-era subprocess entries so an
        // old mcp.json cannot shadow or duplicate the native capability.
        for id in LEGACY_NATIVE_MCP_IDS {
            map.remove(id);
        }
        json!({ "mcp": map })
    }

    fn update_path(
        &self,
        workspace: &Path,
        source_id: &str,
    ) -> Result<(PathBuf, Vec<String>), ServiceError> {
        let workspace = Self::workspace_root(workspace);
        match source_id {
            GUI_SOURCE => Ok((self.gui_path.clone(), vec!["agent".into()])),
            MCP_SOURCE => Ok((self.user_root().join("mcp.json"), Vec::new())),
            PROJECT_SOURCE => {
                Ok((workspace.join(".neoism/config.json"), vec!["agent".into()]))
            }
            PROJECT_MCP_SOURCE => Ok((workspace.join(".neoism/mcp.json"), Vec::new())),
            _ => Err(ServiceError::new(format!(
                "config source `{source_id}` is not writable"
            ))),
        }
    }
}

fn canonicalize_gui_agent(agent: &mut serde_json::Map<String, Value>) {
    for (legacy, canonical) in [
        ("disabled-providers", "disabledProviders"),
        ("enabled-providers", "enabledProviders"),
        ("reasoning-effort", "variant"),
        ("text-verbosity", "textVerbosity"),
        ("small-model", "smallModel"),
        ("default-agent", "defaultAgent"),
        ("dangerously-skip-permissions", "dangerouslySkipPermissions"),
    ] {
        move_legacy_key(agent, legacy, canonical);
    }

    if let Some(experimental) =
        agent.get_mut("experimental").and_then(Value::as_object_mut)
    {
        for (legacy, canonical) in [
            ("disable-paste-summary", "disablePasteSummary"),
            ("batch-tool", "batchTool"),
            ("open-telemetry", "openTelemetry"),
            ("primary-tools", "primaryTools"),
        ] {
            move_legacy_key(experimental, legacy, canonical);
        }
    }

    for group in ["agent", "mode"] {
        let Some(profiles) = agent.get_mut(group).and_then(Value::as_object_mut) else {
            continue;
        };
        for profile in profiles.values_mut().filter_map(Value::as_object_mut) {
            move_legacy_key(profile, "top-p", "topP");
            move_legacy_key(profile, "max-steps", "maxSteps");
        }
    }
}

fn move_legacy_key(
    map: &mut serde_json::Map<String, Value>,
    legacy: &str,
    canonical: &str,
) {
    let legacy_value = map.remove(legacy);
    if !map.contains_key(canonical) {
        if let Some(value) = legacy_value {
            map.insert(canonical.to_string(), value);
        }
    }
}

impl Default for NeoismConfigSourceService {
    fn default() -> Self {
        Self::new()
    }
}

impl ConfigSourceService for NeoismConfigSourceService {
    fn snapshot(
        &self,
        request: &ConfigSnapshotRequest,
    ) -> Result<ConfigSnapshot, ServiceError> {
        let workspace = Self::workspace_root(&request.workspace);
        Self::migrate_project_config(&workspace)?;
        let gui = Self::read(&self.gui_path)?;
        let mcp = Self::read(&self.user_root().join("mcp.json"))?;
        let project_config = Self::read(&workspace.join(".neoism/config.json"))?;
        let project_mcp = Self::read(&workspace.join(".neoism/mcp.json"))?;
        let layers = vec![
            ConfigLayer {
                source_id: GUI_SOURCE.into(),
                document: Self::project_gui(&gui),
                writable: true,
            },
            ConfigLayer {
                source_id: MCP_SOURCE.into(),
                document: Self::mcp_document(&mcp),
                writable: true,
            },
            ConfigLayer {
                source_id: PROJECT_SOURCE.into(),
                document: Self::project_gui(&project_config),
                writable: true,
            },
            ConfigLayer {
                source_id: PROJECT_MCP_SOURCE.into(),
                document: Self::mcp_document(&project_mcp),
                writable: true,
            },
        ];
        let identity = layers
            .iter()
            .map(|layer| format!("{}\0{}", layer.source_id, layer.document))
            .collect::<Vec<_>>()
            .join("\0");
        Ok(ConfigSnapshot {
            identity,
            workspace: workspace.clone(),
            layers,
            discovery_roots: vec![
                ConfigDiscoveryRoot {
                    source_id: "neoism:user-root".into(),
                    path: self.user_root(),
                },
                ConfigDiscoveryRoot {
                    source_id: "neoism:project-root".into(),
                    path: workspace.join(".neoism"),
                },
            ],
            writable_target: ConfigWritableTarget {
                source_id: PROJECT_SOURCE.into(),
                label: "Neoism workspace config".into(),
            },
        })
    }

    fn update<'a>(
        &'a self,
        request: &'a ConfigUpdateRequest,
    ) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
        Box::pin(async move {
            let (path, prefix) =
                self.update_path(&request.workspace, &request.source_id)?;
            let mut document = Self::read(&path)?;
            match &request.update {
                ConfigUpdate::ReplaceDocument {
                    document: replacement,
                } if prefix.is_empty() => document = replacement.clone(),
                ConfigUpdate::ReplaceDocument {
                    document: replacement,
                } => set_value(&mut document, &prefix, replacement.clone()),
                ConfigUpdate::SetValue { path, value } => {
                    let mut full = prefix;
                    full.extend(path.iter().cloned());
                    set_value(&mut document, &full, value.clone());
                }
            }
            Self::write(&path, &document)?;
            self.snapshot(&ConfigSnapshotRequest::new(&request.workspace))
        })
    }
}

fn set_value(document: &mut Value, path: &[String], value: Value) {
    if path.is_empty() {
        *document = value;
        return;
    }
    let mut current = document;
    for component in &path[..path.len() - 1] {
        if !current.is_object() {
            *current = json!({});
        }
        current = current
            .as_object_mut()
            .expect("object initialized")
            .entry(component)
            .or_insert_with(|| json!({}));
    }
    if !current.is_object() {
        *current = json!({});
    }
    current
        .as_object_mut()
        .expect("object initialized")
        .insert(path.last().unwrap().clone(), value);
}

fn merge_missing(target: &mut Value, source: Value) {
    let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object())
    else {
        return;
    };
    for (key, value) in source {
        match target.get_mut(key) {
            Some(existing) if existing.is_object() && value.is_object() => {
                merge_missing(existing, value.clone())
            }
            Some(_) => {}
            None => {
                target.insert(key.clone(), value.clone());
            }
        }
    }
}

fn project_shell(value: &Value) -> Option<String> {
    match value {
        Value::String(shell) => {
            let shell = shell.trim();
            (!shell.is_empty()).then(|| shell.to_string())
        }
        Value::Object(shell) => {
            let program = shell.get("program")?.as_str()?.trim();
            if program.is_empty() {
                return None;
            }
            let args = shell
                .get("args")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(" ");
            Some(if args.is_empty() {
                program.to_string()
            } else {
                format!("{program} {args}")
            })
        }
        _ => None,
    }
}

fn parse_jsonc(text: &str) -> Result<Value, serde_json::Error> {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut string = false;
    let mut escaped = false;
    while let Some(ch) = chars.next() {
        if string {
            out.push(ch);
            if ch == '"' && !escaped {
                string = false;
            }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' {
                escaped = false;
            }
        } else if ch == '"' {
            string = true;
            out.push(ch);
        } else if ch == '/' && chars.peek() == Some(&'/') {
            chars.next();
            for next in chars.by_ref() {
                if next == '\n' {
                    out.push('\n');
                    break;
                }
            }
        } else if ch == '/' && chars.peek() == Some(&'*') {
            chars.next();
            let mut prior = '\0';
            for next in chars.by_ref() {
                if prior == '*' && next == '/' {
                    break;
                }
                prior = next;
            }
        } else {
            out.push(ch);
        }
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
            if ch == '"' && !escaped {
                string = false;
            }
            escaped = ch == '\\' && !escaped;
            if ch != '\\' {
                escaped = false;
            }
        } else if ch == '"' {
            string = true;
            out.push(ch);
        } else if ch == ',' {
            let mut rest = chars.clone();
            while rest.peek().is_some_and(|next| next.is_whitespace()) {
                rest.next();
            }
            if !matches!(rest.peek(), Some('}' | ']')) {
                out.push(ch);
            }
        } else {
            out.push(ch);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_product_mcp_entries_cannot_shadow_native_tools() {
        let document = NeoismConfigSourceService::mcp_document(&json!({
            "mcp": {
                "neoism-docs": { "type": "local", "command": ["broken", "docs"] },
                "neoism-memory": { "type": "local", "command": ["broken", "memory"] },
                "neoism-notes": { "type": "local", "command": ["broken", "notes"] },
                "external-search": { "type": "remote", "url": "https://example.test/mcp" }
            }
        }));

        assert_eq!(
            document.pointer("/mcp/external-search/type"),
            Some(&json!("remote")),
        );
        for id in LEGACY_NATIVE_MCP_IDS {
            assert!(document.pointer(&format!("/mcp/{id}")).is_none());
        }
    }

    #[test]
    fn projects_grouped_gui_config_to_canonical_agent_json() {
        let root = std::env::temp_dir()
            .join(format!("neoism-config-adapter-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let gui = root.join("config.json");
        std::fs::write(&gui, r#"{
            // Product JSONC stays adapter-owned.
            "appearance":{"theme":"x"},
            "terminal":{"shell":{"program":"/bin/zsh","args":["--login"]}},
            "agent":{"model":"p/m","smallModel":"p/s","variant":"high","textVerbosity":"low"},
        }"#).unwrap();
        let service = NeoismConfigSourceService::at(gui);
        let snapshot = service
            .snapshot(&ConfigSnapshotRequest::new(&root))
            .unwrap();
        assert_eq!(snapshot.layers[0].document["model"], "p/m");
        assert_eq!(snapshot.layers[0].document["shell"], "/bin/zsh --login");
        assert_eq!(snapshot.layers[0].document["smallModel"], "p/s");
        assert_eq!(snapshot.layers[0].document["variant"], "high");
        assert_eq!(snapshot.layers[0].document["textVerbosity"], "low");
        assert!(snapshot.layers[0].document.get("terminal").is_none());
        assert!(snapshot.layers[0].document.get("appearance").is_none());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn projects_persisted_neoism_agent_keys_without_weakening_canonical_json() {
        let projected = NeoismConfigSourceService::project_gui(&json!({
            "agent": {
                "reasoning-effort": "medium",
                "variant": "high",
                "small-model": "p/s",
                "dangerously-skip-permissions": true,
                "experimental": { "batch-tool": true },
                "agent": {
                    "review": { "top-p": 0.8, "max-steps": 12 }
                }
            }
        }));

        assert_eq!(projected["variant"], "high");
        assert_eq!(projected["smallModel"], "p/s");
        assert_eq!(projected["dangerouslySkipPermissions"], true);
        assert_eq!(projected["experimental"]["batchTool"], true);
        assert_eq!(projected["agent"]["review"]["topP"], 0.8);
        assert_eq!(projected["agent"]["review"]["maxSteps"], 12);
        assert!(projected.get("reasoning-effort").is_none());
    }

    #[test]
    fn project_file_is_projected_but_canonical_agent_file_is_not_unwrapped() {
        let product = json!({
            "terminal": { "shell": "fish" },
            "agent": { "defaultAgent": "build" }
        });
        assert_eq!(
            NeoismConfigSourceService::project_document(product),
            json!({
                "shell": "fish", "defaultAgent": "build"
            })
        );

        let canonical = json!({
            "defaultAgent": "build",
            "agent": { "build": { "topP": 0.8 } }
        });
        assert_eq!(
            NeoismConfigSourceService::project_document(canonical.clone()),
            canonical
        );
    }

    #[test]
    fn root_project_config_is_moved_into_workspace_config() {
        let root = std::env::temp_dir()
            .join(format!("neoism-workspace-config-{}", std::process::id()));
        std::fs::create_dir_all(root.join(".neoism")).unwrap();
        let gui = root.join("user/config.json");
        std::fs::create_dir_all(gui.parent().unwrap()).unwrap();
        std::fs::write(&gui, r#"{"agent":{"model":"global/model"}}"#).unwrap();
        std::fs::write(
            root.join("neoism.json"),
            r#"{"model":"old/model","smallModel":"old/small"}"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".neoism/config.json"),
            r#"{
            "appearance":{"theme":"ignored-by-agent"},
            "agent":{"model":"workspace/model","defaultAgent":"build"}
        }"#,
        )
        .unwrap();
        std::fs::write(
            root.join(".neoism/mcp.json"),
            r#"{"search":{"type":"remote","url":"https://example.test/mcp"}}"#,
        )
        .unwrap();

        let snapshot = NeoismConfigSourceService::at(gui)
            .snapshot(&ConfigSnapshotRequest::new(&root))
            .unwrap();
        assert_eq!(snapshot.layers[2].source_id, PROJECT_SOURCE);
        assert_eq!(snapshot.layers[2].document["model"], "workspace/model");
        assert_eq!(snapshot.layers[2].document["smallModel"], "old/small");
        assert_eq!(snapshot.layers[2].document["defaultAgent"], "build");
        assert_eq!(
            snapshot.layers[3].document["mcp"]["search"]["type"],
            "remote"
        );
        assert_eq!(snapshot.writable_target.source_id, PROJECT_SOURCE);
        assert!(!root.join("neoism.json").exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn projected_updates_preserve_gui_groups() {
        let root = std::env::temp_dir().join(format!(
            "neoism-config-adapter-write-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let gui = root.join("config.json");
        std::fs::write(
            &gui,
            r#"{"appearance":{"theme":"x"},"agent":{"model":"old/model"}}"#,
        )
        .unwrap();
        let service = NeoismConfigSourceService::at(gui.clone());
        service
            .update(&ConfigUpdateRequest {
                workspace: root.clone(),
                source_id: GUI_SOURCE.into(),
                update: ConfigUpdate::SetValue {
                    path: vec!["model".into()],
                    value: json!("new/model"),
                },
            })
            .await
            .unwrap();
        let written: Value =
            serde_json::from_str(&std::fs::read_to_string(gui).unwrap()).unwrap();
        assert_eq!(written["appearance"]["theme"], "x");
        assert_eq!(written["agent"]["model"], "new/model");
        let _ = std::fs::remove_dir_all(root);
    }
}
