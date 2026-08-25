use std::path::{Path, PathBuf};

use neoism_agent_core::AgentConfigDocument;
use neoism_agent_service_api::{AgentServices, ConfigSnapshotRequest};
use serde_json::Value;

pub(super) fn load(services: &AgentServices, directory: &str) -> anyhow::Result<(AgentConfigDocument, Vec<PathBuf>)> {
    let snapshot = services
        .config
        .snapshot(&ConfigSnapshotRequest::new(directory))?;
    let mut value = serde_json::json!({});
    for layer in snapshot.layers {
        merge(&mut value, layer.document);
    }
    let document = serde_json::from_value(value)?;
    let roots = snapshot.discovery_roots.into_iter().map(|root| root.path).collect();
    Ok((document, roots))
}

pub(super) fn markdown_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                collect(&path, files)?;
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("md") {
                files.push(path);
            }
        }
        Ok(())
    }
    let mut files = Vec::new();
    if root.is_dir() {
        collect(root, &mut files)?;
    }
    files.sort();
    Ok(files)
}

pub(super) fn parse_markdown(path: &Path) -> anyhow::Result<(serde_json::Map<String, Value>, String)> {
    let text = std::fs::read_to_string(path)?;
    if !text.starts_with("---") {
        return Ok((serde_json::Map::new(), text));
    }
    let rest = text.strip_prefix("---").unwrap_or(&text);
    let rest = rest.strip_prefix('\n').unwrap_or(rest);
    let Some(index) = rest.find("\n---") else {
        return Ok((serde_json::Map::new(), text));
    };
    let content = rest[index + "\n---".len()..]
        .strip_prefix('\n')
        .unwrap_or_default()
        .to_string();
    let yaml = serde_yaml::from_str::<serde_yaml::Value>(&rest[..index])?;
    let data = serde_json::to_value(yaml)?;
    Ok((data.as_object().cloned().unwrap_or_default(), content))
}

fn merge(target: &mut Value, source: Value) {
    match (target, source) {
        (Value::Object(target), Value::Object(source)) => {
            for (key, value) in source {
                merge(target.entry(key).or_insert(Value::Null), value);
            }
        }
        (target, source) => *target = source,
    }
}