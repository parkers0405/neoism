use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::Context;
use neoism_agent_core::SkillInfo;
use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginFuture, PluginHostError, PluginManifest,
    ContributionMetadata, PluginRuntimeError, PluginScope, RouteContribution, RouteDescriptor, RouteHandler,
    RouteMethod, RouteRequest, RouteResponse, RouteScope, SkillSource,
};
use neoism_agent_service_api::AgentServices;
use serde::Deserialize;

use super::config;

pub const ID: &str = "dev.neoism.skills";

/// The only server-owned part of the skills plugin is registration of the
/// kernel-backed `skill` tool. Discovery and source execution remain here.
pub trait SkillsHost: Send + Sync + 'static {
    fn register_tools(&self, registrar: &mut PluginContributions);
}

pub struct SkillsPlugin {
    config: Arc<neoism_agent_core::AgentConfigDocument>,
    discovery_roots: Arc<Vec<PathBuf>>,
    host: Arc<dyn SkillsHost>,
}

impl SkillsPlugin {
    pub fn new(
        config: neoism_agent_core::AgentConfigDocument,
        discovery_roots: Vec<PathBuf>,
        host: Arc<dyn SkillsHost>,
    ) -> Self {
        Self {
            config: Arc::new(config),
            discovery_roots: Arc::new(discovery_roots),
            host,
        }
    }
}

impl PluginDefinition for SkillsPlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: ID.into(), name: "Skills".into(), version: env!("CARGO_PKG_VERSION").into(),
            internal: true, disableable: true, capabilities: vec!["neoism.skills".into()],
            requires: Vec::new(), event_namespaces: vec!["skill".into()],
            api_prefix: Some("/v2/skills".into()), config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> { vec![neoism_agent_plugin_api::HostCapability::WorkspaceRead] }
    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        registrar.runtime_route(RouteContribution {
            descriptor: RouteDescriptor {
                id: "v2.skills.list".into(),
                method: RouteMethod::Get,
                path: "/v2/skills".into(),
                scope: RouteScope::Workspace,
                request_schema: None,
                response_schema: None,
            },
            metadata: ContributionMetadata::new("v2.skills.list", ID, PluginScope::Workspace),
            handler: Arc::new(SkillsRoute(self.config.clone(), self.discovery_roots.clone())),
        });
        registrar.skill_source_runtime(
            "workspace-skills",
            Arc::new(WorkspaceSkills(self.config.clone(), self.discovery_roots.clone())),
        );
        self.host.register_tools(registrar);
        Ok(())
    }
}

struct SkillsRoute(
    Arc<neoism_agent_core::AgentConfigDocument>,
    Arc<Vec<PathBuf>>,
);

impl RouteHandler for SkillsRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        Box::pin(async move {
            let directory = request.workspace.unwrap_or_default();
            let directory = directory.to_string_lossy();
            let skills = load_from_config(&directory, &self.0, &self.1)
                .await
                .map(|skills| skills.into_iter().map(|skill| skill.info).collect::<Vec<_>>())
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let body = serde_json::to_value(skills)
                .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            Ok(RouteResponse::json(200, body))
        })
    }
}

struct WorkspaceSkills(
    Arc<neoism_agent_core::AgentConfigDocument>,
    Arc<Vec<PathBuf>>,
);

impl SkillSource for WorkspaceSkills {
    fn list<'a>(&'a self, directory: &'a str) -> PluginFuture<'a, Vec<SkillInfo>> {
        Box::pin(async move {
            load_from_config(directory, &self.0, &self.1)
                .await
                .map(|skills| skills.into_iter().map(|skill| skill.info).collect())
                .map_err(|error| PluginRuntimeError::new(error.to_string()))
        })
    }

    fn get<'a>(
        &'a self,
        directory: &'a str,
        id: &'a str,
    ) -> PluginFuture<'a, Option<neoism_agent_plugin_api::SkillDocument>> {
        Box::pin(async move {
            load_from_config(directory, &self.0, &self.1)
                .await
                .map(|skills| {
                    skills
                        .into_iter()
                        .find(|skill| skill.info.id == id)
                        .map(|skill| neoism_agent_plugin_api::SkillDocument {
                            info: skill.info,
                            content: skill.content,
                        })
                })
                .map_err(|error| PluginRuntimeError::new(error.to_string()))
        })
    }
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub info: SkillInfo,
    pub content: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
    #[allow(dead_code)]
    version: Option<String>,
    #[allow(dead_code)]
    license: Option<String>,
    #[allow(dead_code)]
    compatibility: Option<serde_json::Value>,
    #[allow(dead_code)]
    metadata: Option<BTreeMap<String, serde_json::Value>>,
}

#[derive(Deserialize)]
struct DiscoveryIndex { skills: Vec<DiscoveryIndexSkill> }

#[derive(Deserialize)]
struct DiscoveryIndexSkill { name: String, #[serde(default)] files: Vec<String> }

pub async fn list(services: &AgentServices, directory: &str) -> anyhow::Result<Vec<SkillInfo>> {
    Ok(load(services, directory).await?.into_iter().map(|skill| skill.info).collect())
}

pub async fn load(services: &AgentServices, directory: &str) -> anyhow::Result<Vec<Skill>> {
    let (document, roots) = config::load(services, directory)?;
    load_from_config(directory, &document, &roots).await
}

async fn load_from_config(
    directory: &str,
    document: &neoism_agent_core::AgentConfigDocument,
    roots: &[PathBuf],
) -> anyhow::Result<Vec<Skill>> {
    let mut by_name = load_local(directory, &document.skills.paths, &roots)?
        .into_iter().map(|skill| (skill.info.id.clone(), skill)).collect::<BTreeMap<_, _>>();
    for skill in load_remote(&document.skills.urls).await { by_name.insert(skill.info.id.clone(), skill); }
    Ok(by_name.into_values().collect())
}

fn load_local(directory: &str, configured_paths: &[String], discovery_roots: &[PathBuf]) -> anyhow::Result<Vec<Skill>> {
    let mut roots = configured_roots(directory, configured_paths);
    for root in discovery_roots {
        roots.push(root.join("skill"));
        roots.push(root.join("skills"));
    }
    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        for file in skill_files(&root)? {
            let key = file.canonicalize().unwrap_or_else(|_| file.clone());
            if seen.insert(key) { files.push(file); }
        }
    }
    let mut by_name = BTreeMap::new();
    for file in files {
        let skill = read_skill(&file).with_context(|| format!("failed to load skill {}", file.display()))?;
        by_name.insert(skill.info.id.clone(), skill);
    }
    Ok(by_name.into_values().collect())
}

async fn load_remote(urls: &[String]) -> Vec<Skill> {
    let mut skills = Vec::new();
    for url in urls {
        if let Ok(mut pulled) = pull_remote_skills(url).await { skills.append(&mut pulled); }
    }
    skills
}

async fn pull_remote_skills(url: &str) -> anyhow::Result<Vec<Skill>> {
    let base = if url.ends_with('/') { url.to_string() } else { format!("{url}/") };
    let base_url = reqwest::Url::parse(&base).context("invalid skill discovery URL")?;
    let index = reqwest::Client::new().get(base_url.join("index.json")?).send().await?.error_for_status()?.json::<DiscoveryIndex>().await?;
    let mut skills = Vec::new();
    for entry in index.skills {
        if entry.files.iter().any(|file| file == "SKILL.md") {
            if let Some(skill) = pull_remote_skill(&base_url, entry).await? { skills.push(skill); }
        }
    }
    Ok(skills)
}

async fn pull_remote_skill(base_url: &reqwest::Url, entry: DiscoveryIndexSkill) -> anyhow::Result<Option<Skill>> {
    let skill_base = base_url.join(&format!("{}/", entry.name))?;
    let root = default_cache_dir().join("skills").join(safe_cache_component(&entry.name));
    let client = reqwest::Client::new();
    for file in entry.files {
        let Some(relative) = safe_relative_path(&file) else { continue; };
        let destination = root.join(&relative);
        if destination.exists() { continue; }
        let bytes = client.get(skill_base.join(&file)?).send().await?.error_for_status()?.bytes().await?;
        if let Some(parent) = destination.parent() { std::fs::create_dir_all(parent)?; }
        std::fs::write(destination, bytes.as_ref())?;
    }
    let path = root.join("SKILL.md");
    if path.exists() { read_skill(&path).map(Some) } else { Ok(None) }
}

fn configured_roots(directory: &str, paths: &[String]) -> Vec<PathBuf> {
    let base = PathBuf::from(directory);
    paths.iter().filter_map(|path| {
        let path = path.trim();
        if path.is_empty() || path.starts_with("http://") || path.starts_with("https://") { return None; }
        if let Some(rest) = path.strip_prefix("~/") { return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest)); }
        let path = PathBuf::from(path);
        Some(if path.is_absolute() { path } else { base.join(path) })
    }).collect()
}

fn safe_cache_component(name: &str) -> String {
    let value = name.chars().map(|ch| if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') { ch } else { '_' }).collect::<String>();
    if value.is_empty() { "skill".into() } else { value }
}

fn safe_relative_path(file: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(file).components() {
        match component { Component::Normal(part) => out.push(part), Component::CurDir => {}, _ => return None }
    }
    (!out.as_os_str().is_empty()).then_some(out)
}

fn skill_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    if root.is_file() { return Ok((root.file_name().and_then(|name| name.to_str()) == Some("SKILL.md")).then(|| root.to_path_buf()).into_iter().collect()); }
    let mut files = Vec::new();
    if root.is_dir() { collect_skill_files(root, &mut files)?; }
    files.sort();
    Ok(files)
}

fn collect_skill_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() { collect_skill_files(&path, files)?; }
        else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") { files.push(path); }
    }
    Ok(())
}

fn read_skill(file: &Path) -> anyhow::Result<Skill> {
    let raw = std::fs::read_to_string(file)?;
    let (frontmatter, content) = split_frontmatter(&raw)?;
    let id = if file.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
        file.parent().and_then(Path::file_name)
    } else {
        file.file_stem()
    }
    .and_then(|name| name.to_str())
    .unwrap_or("skill")
    .to_string();
    Ok(Skill {
        info: SkillInfo {
            id: id.clone(),
            name: frontmatter.name.map(|name| name.trim().to_string()).filter(|name| !name.is_empty()).unwrap_or(id),
            description: frontmatter.description.map(|value| value.trim().to_string()).filter(|value| !value.is_empty()),
            path: Some(file.display().to_string()),
        },
        content: content.trim().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let normalized = raw.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") { return Ok((SkillFrontmatter::default(), raw.to_string())); }
    let Some(end) = normalized[4..].find("\n---\n") else { return Ok((SkillFrontmatter::default(), raw.to_string())); };
    let frontmatter = serde_yaml::from_str(&normalized[4..4 + end])?;
    Ok((frontmatter, normalized[4 + end + "\n---\n".len()..].to_string()))
}

fn default_cache_dir() -> PathBuf {
    if let Some(path) = std::env::var_os("NEOISM_AGENT_CACHE_DIR").filter(|path| !path.is_empty()) { return PathBuf::from(path); }
    #[cfg(windows)]
    {
        return std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("AppData/Local")))
            .unwrap_or_else(|| PathBuf::from(".neoism"))
            .join("neoism/cache");
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("XDG_CACHE_HOME").map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))
            .unwrap_or_else(|| PathBuf::from(".neoism/cache")).join("neoism")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Host;
    impl SkillsHost for Host { fn register_tools(&self, _: &mut PluginContributions) {} }

    #[test]
    fn plugin_owns_manifest_and_runtime_source() {
        let plugin = SkillsPlugin::new(
            neoism_agent_core::AgentConfigDocument::default(),
            Vec::new(),
            Arc::new(Host),
        );
        assert_eq!(plugin.manifest().id, ID);
        let mut registrar = PluginContributions::default();
        plugin.contributions(&mut registrar).unwrap();
    }

    #[test]
    fn rejects_remote_parent_traversal() { assert!(safe_relative_path("../secret").is_none()); }
}