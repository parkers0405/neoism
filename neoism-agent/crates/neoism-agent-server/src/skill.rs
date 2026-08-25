//! Server adapter for built-in skill discovery plus the kernel tool result.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use crate::tool::{ToolContext, ToolExecutionResult};

pub(crate) use neoism_agent_builtins::plugin::skills::Skill;

pub(crate) async fn load_async(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
) -> anyhow::Result<Vec<Skill>> {
    neoism_agent_builtins::plugin::skills::load(services, directory).await
}

pub(crate) fn skill_tool(
    context: ToolContext,
    arguments: Value,
) -> impl std::future::Future<Output = anyhow::Result<ToolExecutionResult>> {
    async move {
        let name = arguments.get("name").and_then(Value::as_str).map(str::trim)
            .filter(|value| !value.is_empty()).ok_or_else(|| anyhow::anyhow!("tool argument name is required"))?;
        context.ensure_allowed("skill", name)?;
        let services = context.state().map(|state| state.services().clone()).unwrap_or_else(crate::standard_services);
        let skills = load_async(&services, &context.cwd.to_string_lossy()).await?;
        let available = skills.iter().map(|skill| skill.info.name.as_str()).collect::<Vec<_>>().join(", ");
        let skill = skills.into_iter().find(|skill| skill.info.name == name || skill.info.path.as_deref() == Some(name))
            .ok_or_else(|| anyhow::anyhow!("Skill \"{name}\" not found. Available skills: {}", if available.is_empty() { "none" } else { &available }))?;
        render_skill(skill)
    }
}

fn render_skill(skill: Skill) -> anyhow::Result<ToolExecutionResult> {
    let base_dir = skill.info.path.as_deref().and_then(|path| Path::new(path).parent()).map(Path::to_path_buf);
    let files = base_dir.as_deref().map(sample_skill_files).transpose()?.unwrap_or_default()
        .into_iter().map(|path| format!("<file>{}</file>", path.display())).collect::<Vec<_>>().join("\n");
    let base = base_dir.as_deref().map(|dir| dir.display().to_string()).unwrap_or_else(|| "unknown".into());
    Ok(ToolExecutionResult {
        title: format!("Loaded skill {}", skill.info.name),
        output: [
            format!("<skill_content name=\"{}\">", skill.info.name),
            format!("# Skill: {}", skill.info.name), String::new(), skill.content.trim().to_string(), String::new(),
            format!("Base directory for this skill: {base}"),
            "Relative paths in this skill are relative to this base directory.".into(),
            "Note: file list is sampled.".into(), String::new(), "<skill_files>".into(), files,
            "</skill_files>".into(), "</skill_content>".into(),
        ].join("\n"),
        metadata: Some(json!({"skill":{"name":skill.info.name,"description":skill.info.description,"path":skill.info.path,"dir":base}})),
    })
}

fn sample_skill_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    fn collect(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() { collect(&path, files)?; }
            else if path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") { files.push(path); if files.len() >= 10 { break; } }
        }
        Ok(())
    }
    let mut files = Vec::new();
    collect(dir, &mut files)?;
    files.sort();
    files.truncate(10);
    Ok(files)
}