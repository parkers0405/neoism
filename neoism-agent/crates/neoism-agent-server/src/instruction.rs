use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use crate::config;

const INSTRUCTION_FILES: &[&str] = &["AGENTS.md", "CLAUDE.md", "CONTEXT.md"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LoadedInstruction {
    pub(crate) filepath: String,
    pub(crate) content: String,
}

#[cfg(test)]
fn system(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
) -> Vec<String> {
    let config = neoism_agent_builtins::plugin::config::load(services, directory)
        .ok()
        .map(|(config, _)| config)
        .unwrap_or_default();
    system_with_config(services, directory, &config)
}

pub(crate) fn system_with_config(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
    config: &neoism_agent_core::AgentConfigDocument,
) -> Vec<String> {
    let mut paths = BTreeSet::new();
    let mut ordered = Vec::new();

    for path in global_instruction_candidates(services, directory) {
        if push_existing(&mut paths, &mut ordered, path) {
            break;
        }
    }

    for path in project_instruction_files(directory) {
        push_existing(&mut paths, &mut ordered, path);
    }

    for raw in &config.instructions {
        if let Some(path) = configured_instruction_path(directory, raw) {
            push_existing(&mut paths, &mut ordered, path);
        }
    }

    ordered
        .into_iter()
        .filter_map(|path| {
            std::fs::read_to_string(&path).ok().and_then(|content| {
                let content = content.trim();
                (!content.is_empty()).then(|| {
                    format!("Instructions from: {}\n{}", path.display(), content)
                })
            })
        })
        .collect()
}

pub(crate) fn nearby(directory: &Path, filepath: &Path) -> Vec<LoadedInstruction> {
    let root = directory
        .canonicalize()
        .unwrap_or_else(|_| directory.to_path_buf());
    let target = filepath
        .canonicalize()
        .unwrap_or_else(|_| filepath.to_path_buf());
    let mut current = target
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| root.clone());
    let mut loaded = Vec::new();
    let mut seen = BTreeSet::new();

    while current.starts_with(&root) && current != root {
        if let Some(path) = instruction_in_dir(&current) {
            let key = path.canonicalize().unwrap_or_else(|_| path.clone());
            if key != target && seen.insert(key.clone()) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let content = content.trim();
                    if !content.is_empty() {
                        loaded.push(LoadedInstruction {
                            filepath: path.display().to_string(),
                            content: format!(
                                "Instructions from: {}\n{}",
                                path.display(),
                                content
                            ),
                        });
                    }
                }
            }
        }
        if !current.pop() {
            break;
        }
    }

    loaded
}

fn instruction_in_dir(dir: &Path) -> Option<PathBuf> {
    INSTRUCTION_FILES
        .iter()
        .map(|file| dir.join(file))
        .find(|path| path.is_file())
}

fn global_instruction_candidates(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
) -> Vec<PathBuf> {
    config::roots(services, directory)
        .into_iter()
        .flat_map(|root| INSTRUCTION_FILES.iter().map(move |file| root.join(file)))
        .collect()
}

fn project_instruction_files(directory: &str) -> Vec<PathBuf> {
    let ancestors = ancestor_dirs(Path::new(directory));
    for file in INSTRUCTION_FILES {
        let matches = ancestors
            .iter()
            .map(|dir| dir.join(file))
            .filter(|path| path.is_file())
            .collect::<Vec<_>>();
        if !matches.is_empty() {
            return matches;
        }
    }
    Vec::new()
}

fn configured_instruction_path(directory: &str, raw: &str) -> Option<PathBuf> {
    let raw = raw.trim();
    if raw.is_empty() || raw.starts_with("http://") || raw.starts_with("https://") {
        return None;
    }
    if let Some(rest) = raw.strip_prefix("~/") {
        return std::env::var_os("HOME").map(|home| PathBuf::from(home).join(rest));
    }
    let path = PathBuf::from(raw);
    Some(if path.is_absolute() {
        path
    } else {
        find_up(Path::new(directory), &path)
            .unwrap_or_else(|| Path::new(directory).join(path))
    })
}

fn find_up(directory: &Path, relative: &Path) -> Option<PathBuf> {
    ancestor_dirs(directory)
        .into_iter()
        .map(|dir| dir.join(relative))
        .find(|path| path.is_file())
}

fn push_existing(
    seen: &mut BTreeSet<PathBuf>,
    ordered: &mut Vec<PathBuf>,
    path: PathBuf,
) -> bool {
    if !path.is_file() {
        return false;
    }
    let key = path.canonicalize().unwrap_or_else(|_| path.clone());
    if seen.insert(key) {
        ordered.push(path);
        return true;
    }
    false
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
    dirs.reverse();
    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "neoism-agent-instruction-{name}-{}",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ))
    }

    #[test]
    fn loads_agents_md_and_configured_instruction_files() {
        let root = temp_root("system");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join(".agent")).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Project instructions.\n").unwrap();
        std::fs::write(root.join("EXTRA.md"), "Extra configured instructions.\n")
            .unwrap();
        std::fs::write(
            root.join(".agent/agent.json"),
            r#"{ "instructions": ["EXTRA.md", "https://example.com/remote.md"] }"#,
        )
        .unwrap();

        let instructions = system(&crate::standard_services(), root.to_str().unwrap());
        assert_eq!(instructions.len(), 2);
        assert!(instructions[0].contains("Project instructions."));
        assert!(instructions[1].contains("Extra configured instructions."));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn prefers_agents_md_over_claude_md_for_project_defaults() {
        let root = temp_root("preference");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Agent instructions.\n").unwrap();
        std::fs::write(root.join("CLAUDE.md"), "Claude instructions.\n").unwrap();

        let instructions = system(&crate::standard_services(), root.to_str().unwrap());
        assert_eq!(instructions.len(), 1);
        assert!(instructions[0].contains("Agent instructions."));
        assert!(!instructions[0].contains("Claude instructions."));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nearby_loads_nested_instruction_files_but_not_root_default() {
        let root = temp_root("nearby");
        let _ = std::fs::remove_dir_all(&root);
        let feature = root.join("src/feature");
        std::fs::create_dir_all(&feature).unwrap();
        std::fs::write(root.join("AGENTS.md"), "Root instructions.\n").unwrap();
        std::fs::write(feature.join("AGENTS.md"), "Feature instructions.\n").unwrap();
        std::fs::write(feature.join("lib.rs"), "pub fn feature() {}\n").unwrap();

        let loaded = nearby(&root, &feature.join("lib.rs"));
        assert_eq!(loaded.len(), 1);
        assert!(loaded[0].content.contains("Feature instructions."));
        assert!(!loaded[0].content.contains("Root instructions."));
        let _ = std::fs::remove_dir_all(root);
    }
}
