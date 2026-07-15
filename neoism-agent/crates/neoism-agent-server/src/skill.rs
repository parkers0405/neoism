use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};

use anyhow::Context;
use neoism_agent_core::SkillInfo;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::tool::{ToolContext, ToolExecutionResult};
use crate::{config, default_cache_dir, default_config_dir};

#[derive(Clone, Debug)]
pub(crate) struct Skill {
    pub(crate) info: SkillInfo,
    pub(crate) content: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SkillFrontmatter {
    name: Option<String>,
    description: Option<String>,
}

#[derive(Deserialize)]
struct DiscoveryIndex {
    skills: Vec<DiscoveryIndexSkill>,
}

#[derive(Deserialize)]
struct DiscoveryIndexSkill {
    name: String,
    #[serde(default)]
    files: Vec<String>,
}

#[cfg(test)]
pub(crate) fn list(directory: &str) -> anyhow::Result<Vec<SkillInfo>> {
    Ok(load(directory)?
        .into_iter()
        .map(|skill| skill.info)
        .collect())
}

pub(crate) async fn list_async(directory: &str) -> anyhow::Result<Vec<SkillInfo>> {
    Ok(load_async(directory)
        .await?
        .into_iter()
        .map(|skill| skill.info)
        .collect())
}

#[cfg(test)]
pub(crate) fn get(directory: &str, name: &str) -> anyhow::Result<Option<Skill>> {
    let want = name.trim();
    if want.is_empty() {
        return Ok(None);
    }
    Ok(load(directory)?.into_iter().find(|skill| {
        skill.info.name == want
            || skill
                .info
                .path
                .as_deref()
                .map(|path| path == want)
                .unwrap_or(false)
    }))
}

#[cfg(test)]
pub(crate) fn load(directory: &str) -> anyhow::Result<Vec<Skill>> {
    let loaded = config::load(directory)?;
    load_local(directory, &loaded.info.skills.paths)
}

pub(crate) async fn load_async(directory: &str) -> anyhow::Result<Vec<Skill>> {
    let loaded = config::load(directory)?;
    let mut by_name = load_local(directory, &loaded.info.skills.paths)?
        .into_iter()
        .map(|skill| (skill.info.name.clone(), skill))
        .collect::<BTreeMap<_, _>>();
    for skill in load_remote(&loaded.info.skills.urls).await {
        by_name.insert(skill.info.name.clone(), skill);
    }
    Ok(by_name.into_values().collect())
}

pub(crate) fn load_local(
    directory: &str,
    configured_paths: &[String],
) -> anyhow::Result<Vec<Skill>> {
    let mut roots = configured_roots(directory, configured_paths);
    roots.extend(default_roots(directory));

    let mut files = Vec::new();
    let mut seen = BTreeSet::new();
    for root in roots {
        for file in skill_files(&root)? {
            let key = file.canonicalize().unwrap_or_else(|_| file.clone());
            if seen.insert(key) {
                files.push(file);
            }
        }
    }
    let mut by_name = BTreeMap::new();
    for file in files {
        let skill = read_skill(&file)
            .with_context(|| format!("failed to load skill {}", file.display()))?;
        by_name.insert(skill.info.name.clone(), skill);
    }
    Ok(by_name.into_values().collect())
}

pub(crate) fn skill_tool(
    context: ToolContext,
    arguments: Value,
) -> impl std::future::Future<Output = anyhow::Result<ToolExecutionResult>> {
    async move { skill_tool_inner(context, arguments).await }
}

async fn skill_tool_inner(
    context: ToolContext,
    arguments: Value,
) -> anyhow::Result<ToolExecutionResult> {
    let name = string_arg_either_many(&arguments, &["name", "skill", "id"])
        .ok_or_else(|| anyhow::anyhow!("tool argument name is required"))?;
    context.ensure_allowed("skill", &name)?;
    let skills = load_async(&context.cwd.to_string_lossy()).await?;
    let available = skills
        .iter()
        .map(|skill| skill.info.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let skill = skills
        .into_iter()
        .find(|skill| {
            skill.info.name == name
                || skill
                    .info
                    .path
                    .as_deref()
                    .map(|path| path == name)
                    .unwrap_or(false)
        })
        .ok_or_else(|| {
            anyhow::anyhow!(
                "Skill \"{name}\" not found. Available skills: {}",
                if available.is_empty() {
                    "none"
                } else {
                    available.as_str()
                }
            )
        })?;
    let base_dir = skill
        .info
        .path
        .as_deref()
        .and_then(|path| Path::new(path).parent())
        .map(Path::to_path_buf);
    let files = base_dir
        .as_deref()
        .map(sample_skill_files)
        .transpose()?
        .unwrap_or_default()
        .into_iter()
        .map(|path| format!("<file>{}</file>", path.display()))
        .collect::<Vec<_>>()
        .join("\n");
    let base = base_dir
        .as_deref()
        .map(|dir| dir.display().to_string())
        .unwrap_or_else(|| "unknown".to_string());
    Ok(ToolExecutionResult {
        title: format!("Loaded skill {}", skill.info.name),
        output: [
            format!("<skill_content name=\"{}\">", skill.info.name),
            format!("# Skill: {}", skill.info.name),
            String::new(),
            skill.content.trim().to_string(),
            String::new(),
            format!("Base directory for this skill: {base}"),
            "Relative paths in this skill are relative to this base directory."
                .to_string(),
            "Note: file list is sampled.".to_string(),
            String::new(),
            "<skill_files>".to_string(),
            files,
            "</skill_files>".to_string(),
            "</skill_content>".to_string(),
        ]
        .join("\n"),
        metadata: Some(json!({
            "skill": {
                "name": skill.info.name,
                "description": skill.info.description,
                "path": skill.info.path,
                "dir": base,
            }
        })),
    })
}

async fn load_remote(urls: &[String]) -> Vec<Skill> {
    let mut skills = Vec::new();
    for url in urls {
        match pull_remote_skills(url).await {
            Ok(mut pulled) => skills.append(&mut pulled),
            Err(_) => continue,
        }
    }
    skills
}

async fn pull_remote_skills(url: &str) -> anyhow::Result<Vec<Skill>> {
    let base = if url.ends_with('/') {
        url.to_string()
    } else {
        format!("{url}/")
    };
    let base_url = reqwest::Url::parse(&base).context("invalid skill discovery URL")?;
    let index_url = base_url.join("index.json")?;
    let index = reqwest::Client::new()
        .get(index_url)
        .send()
        .await?
        .error_for_status()?
        .json::<DiscoveryIndex>()
        .await?;
    let mut skills = Vec::new();
    for entry in index.skills {
        if !entry.files.iter().any(|file| file == "SKILL.md") {
            continue;
        }
        if let Some(skill) = pull_remote_skill(&base_url, entry).await? {
            skills.push(skill);
        }
    }
    Ok(skills)
}

async fn pull_remote_skill(
    base_url: &reqwest::Url,
    entry: DiscoveryIndexSkill,
) -> anyhow::Result<Option<Skill>> {
    let skill_base = base_url.join(&format!("{}/", entry.name))?;
    let root = PathBuf::from(default_cache_dir())
        .join("skills")
        .join(safe_cache_component(&entry.name));
    let client = reqwest::Client::new();
    for file in entry.files {
        let Some(relative) = safe_relative_path(&file) else {
            continue;
        };
        let dest = root.join(&relative);
        if dest.exists() {
            continue;
        }
        let url = skill_base.join(&file)?;
        let bytes = client
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(dest, bytes.as_ref())?;
    }
    let skill_path = root.join("SKILL.md");
    if skill_path.exists() {
        read_skill(&skill_path).map(Some)
    } else {
        Ok(None)
    }
}

fn configured_roots(directory: &str, paths: &[String]) -> Vec<PathBuf> {
    let base = PathBuf::from(directory);
    paths
        .iter()
        .filter_map(|path| expand_configured_path(&base, path))
        .collect()
}

fn default_roots(directory: &str) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    for dir in config::roots(directory) {
        roots.push(dir.join("skill"));
        roots.push(dir.join("skills"));
    }

    for dir in ancestor_dirs(Path::new(directory)) {
        for config_dir in [".neoism", ".agents", ".claude"] {
            roots.push(dir.join(config_dir).join("skill"));
            roots.push(dir.join(config_dir).join("skills"));
        }
    }

    let global = PathBuf::from(default_config_dir());
    roots.push(global.join("skill"));
    roots.push(global.join("skills"));

    roots
}

fn expand_configured_path(base: &Path, path: &str) -> Option<PathBuf> {
    let trimmed = path.trim();
    if trimmed.is_empty()
        || trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
    {
        return None;
    }
    if let Some(home_path) = trimmed.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(home_path));
    }
    let path = PathBuf::from(trimmed);
    Some(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn safe_cache_component(name: &str) -> String {
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    if out.is_empty() {
        "skill".to_string()
    } else {
        out
    }
}

fn safe_relative_path(file: &str) -> Option<PathBuf> {
    let mut out = PathBuf::new();
    for component in Path::new(file).components() {
        match component {
            Component::Normal(part) => out.push(part),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if out.as_os_str().is_empty() {
        None
    } else {
        Some(out)
    }
}

fn ancestor_dirs(path: &Path) -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    let mut current = if path.is_file() {
        path.parent().unwrap_or(path).to_path_buf()
    } else {
        path.to_path_buf()
    };
    loop {
        dirs.push(current.clone());
        if !current.pop() {
            break;
        }
    }
    dirs
}

fn skill_files(root: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    if root.is_file() {
        if root.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
            files.push(root.to_path_buf());
        }
        return Ok(files);
    }
    if !root.is_dir() {
        return Ok(files);
    }
    collect_skill_files(root, &mut files)?;
    files.sort();
    Ok(files)
}

fn collect_skill_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_skill_files(&path, files)?;
        } else if path.file_name().and_then(|name| name.to_str()) == Some("SKILL.md") {
            files.push(path);
        }
    }
    Ok(())
}

fn sample_skill_files(dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    collect_sample_files(dir, &mut files)?;
    files.sort();
    files.truncate(10);
    Ok(files)
}

fn collect_sample_files(dir: &Path, files: &mut Vec<PathBuf>) -> anyhow::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_sample_files(&path, files)?;
        } else if path.file_name().and_then(|name| name.to_str()) != Some("SKILL.md") {
            files.push(path);
            if files.len() >= 10 {
                break;
            }
        }
    }
    Ok(())
}

fn read_skill(file: &Path) -> anyhow::Result<Skill> {
    let raw = std::fs::read_to_string(file)
        .with_context(|| format!("failed to read skill file {}", file.display()))?;
    let (frontmatter, content) = split_frontmatter(&raw)?;
    let fallback_name = file
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .unwrap_or("skill")
        .to_string();
    let name = frontmatter
        .name
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback_name);
    Ok(Skill {
        info: SkillInfo {
            name,
            description: frontmatter
                .description
                .map(|description| description.trim().to_string())
                .filter(|description| !description.is_empty()),
            path: Some(file.display().to_string()),
        },
        content: content.trim().to_string(),
    })
}

fn split_frontmatter(raw: &str) -> anyhow::Result<(SkillFrontmatter, String)> {
    let normalized = raw.replace("\r\n", "\n");
    if !normalized.starts_with("---\n") {
        return Ok((SkillFrontmatter::default(), raw.to_string()));
    }
    let Some(end) = normalized[4..].find("\n---\n") else {
        return Ok((SkillFrontmatter::default(), raw.to_string()));
    };
    let yaml = &normalized[4..4 + end];
    let frontmatter: SkillFrontmatter =
        serde_yaml::from_str(yaml).context("failed to parse skill frontmatter")?;
    let content_start = 4 + end + "\n---\n".len();
    Ok((frontmatter, normalized[content_start..].to_string()))
}

fn string_arg_either_many(arguments: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| arguments.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        // Pin the GLOBAL config root to an empty temp dir so the real
        // user's `~/.config/neoism/skills` can't leak into assertions
        // (skill discovery always unions the global root in).
        static ISOLATE: std::sync::Once = std::sync::Once::new();
        ISOLATE.call_once(|| {
            let global = std::env::temp_dir().join("neoism-agent-skill-tests-global");
            let _ = std::fs::create_dir_all(&global);
            std::env::set_var("NEOISM_AGENT_CONFIG_DIR", &global);
        });
        std::env::temp_dir().join(format!(
            "neoism-agent-skill-{name}-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ))
    }

    #[test]
    fn loads_project_skills_from_neoism_directory() {
        let root = temp_root("project");
        let _ = std::fs::remove_dir_all(&root);
        let skill_dir = root.join(".neoism/skills/refactor");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: refactor\ndescription: Refactor Rust code\n---\nUse small focused patches.\n",
        )
        .unwrap();

        let skills = list(root.to_str().unwrap()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "refactor");
        assert_eq!(skills[0].description.as_deref(), Some("Refactor Rust code"));

        let skill = get(root.to_str().unwrap(), "refactor").unwrap().unwrap();
        assert_eq!(skill.content, "Use small focused patches.");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn configured_skill_paths_can_point_at_a_skill_file() {
        let root = temp_root("configured");
        let _ = std::fs::remove_dir_all(&root);
        let skill_dir = root.join("custom/portable");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\ndescription: Portable defaults\n---\nKeep behavior cross-platform.\n",
        )
        .unwrap();
        std::fs::write(
            root.join("neoism.json"),
            r#"{ "skills": { "paths": ["custom/portable/SKILL.md"] } }"#,
        )
        .unwrap();

        let skills = list(root.to_str().unwrap()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "portable");
        assert_eq!(skills[0].description.as_deref(), Some("Portable defaults"));
        let _ = std::fs::remove_dir_all(root);
    }
}
