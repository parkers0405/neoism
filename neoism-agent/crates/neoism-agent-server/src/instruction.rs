use std::collections::{BTreeMap, BTreeSet};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

use neoism_agent_core::AgentConfigDocument;
use neoism_agent_service_api::{AgentServices, ConfigDiscoveryScope};

use crate::config;

#[derive(Clone, Debug, Eq, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct LoadedInstruction {
    pub(crate) filepath: String,
    pub(crate) content: String,
}

/// Private epoch cache, not an event payload. Configured glob membership and
/// remote responses are frozen until the generation or discovery context changes.
#[derive(Clone, Debug, Default, PartialEq, serde::Deserialize, serde::Serialize)]
pub(crate) struct InstructionState {
    key: Option<CacheKey>,
    configured: Vec<Source>,
    remote: BTreeMap<String, String>,
    loaded: Vec<LoadedInstruction>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
struct CacheKey {
    generation: u64,
    workspace: PathBuf,
    project_root: PathBuf,
    global_dirs: Vec<PathBuf>,
    home: Option<PathBuf>,
    disabled: bool,
    instructions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
enum Source {
    Local(PathBuf),
    Remote(String),
}

impl InstructionState {
    pub(crate) fn contents(&self) -> Vec<String> {
        self.loaded.iter().map(|item| item.content.clone()).collect()
    }

    pub(crate) fn source_names(&self) -> Vec<String> {
        self.loaded.iter().map(|item| item.filepath.clone()).collect()
    }
}

pub(crate) async fn load(
    services: &AgentServices,
    directory: &str,
    project_root: &Path,
    config: &AgentConfigDocument,
    generation: u64,
    previous: &InstructionState,
) -> InstructionState {
    let global_dirs = config::snapshot(services, directory)
        .map(|snapshot| {
            snapshot.discovery_roots.into_iter()
                .filter(|root| root.scope == ConfigDiscoveryScope::Installation)
                .take(1)
                .map(|root| normalize(&root.path))
                .collect()
        })
        .unwrap_or_default();
    reconcile(CacheKey {
        generation,
        workspace: normalize(Path::new(directory)),
        project_root: normalize(project_root),
        global_dirs,
        home: dirs::home_dir().map(|path| normalize(&path)),
        disabled: project_disabled(),
        instructions: config.instructions.clone(),
    }, previous).await
}

fn project_disabled() -> bool {
    std::env::var("NEOISM_DISABLE_PROJECT_CONFIG").as_deref() == Ok("1")
}

fn normalize(path: &Path) -> PathBuf {
    if let Ok(path) = crate::windows_process::canonicalize_path(path) {
        return path;
    }
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir().unwrap_or_default().join(path)
    };
    let mut result = PathBuf::new();
    for component in absolute.components() {
        match component {
            Component::CurDir => {},
            Component::ParentDir => { result.pop(); },
            _ => result.push(component.as_os_str()),
        }
    }
    crate::windows_process::strip_verbatim_prefix(&result)
}

/// Inclusive, nearest first. Never walk above the explicit boundary.
fn ancestors(directory: &Path, boundary: &Path) -> Vec<PathBuf> {
    if !directory.starts_with(boundary) {
        return Vec::new();
    }
    directory.ancestors().take_while(|dir| dir.starts_with(boundary))
        .map(Path::to_path_buf).collect()
}

fn default_paths(key: &CacheKey) -> Vec<PathBuf> {
    let mut paths: Vec<_> = key.global_dirs.iter().take(1).map(|dir| dir.join("AGENTS.md")).collect();
    if !key.disabled && key.workspace.starts_with(&key.project_root) {
        let boundary = key.home.as_ref().filter(|home| key.workspace.starts_with(home))
            .unwrap_or(&key.project_root);
        paths.extend(ancestors(&key.workspace, boundary).into_iter().map(|dir| dir.join("AGENTS.md")));
    }
    paths
}

fn expand_configured(key: &CacheKey) -> Vec<Source> {
    let relative_dirs = if key.disabled || !key.workspace.starts_with(&key.project_root) {
        key.global_dirs.iter().take(1).cloned().collect()
    } else {
        ancestors(&key.workspace, &key.project_root)
    };
    let mut sources = Vec::new();
    let mut seen = BTreeSet::new();
    for raw in &key.instructions {
        let raw = raw.trim();
        if raw.is_empty() { continue; }
        if raw.starts_with("https://") || raw.starts_with("http://") {
            if seen.insert(raw.to_owned()) { sources.push(Source::Remote(raw.to_owned())); }
            continue;
        }
        // Explicit files are retried even when initially absent. Only glob
        // membership is frozen until the next configuration generation.
        if !raw.contains(['*', '?', '[']) {
            let paths = if let Some(rest) = raw.strip_prefix("~/") {
                key.home.iter().map(|home| home.join(rest)).collect::<Vec<_>>()
            } else if Path::new(raw).is_absolute() {
                vec![PathBuf::from(raw)]
            } else {
                relative_dirs.iter().map(|dir| dir.join(raw)).collect()
            };
            for path in paths {
                let path = normalize(&path);
                if seen.insert(path.to_string_lossy().into_owned()) {
                    sources.push(Source::Local(path));
                }
            }
            continue;
        }
        let patterns = if let Some(rest) = raw.strip_prefix("~/") {
            key.home.iter().map(|home| relative_pattern(home, rest)).collect::<Vec<_>>()
        } else if Path::new(raw).is_absolute() {
            vec![raw.to_owned()]
        } else {
            relative_dirs.iter().map(|dir| relative_pattern(dir, raw)).collect()
        };
        for pattern in patterns {
            // Escape only directory components supplied by discovery, not the
            // user pattern. glob returns sorted results within each pattern.
            if let Ok(matches) = glob::glob(&pattern) {
                for path in matches.flatten() {
                    if path.is_file() {
                        let path = normalize(&path);
                        if seen.insert(path.to_string_lossy().into_owned()) {
                            sources.push(Source::Local(path));
                        }
                    }
                }
            }
        }
    }
    sources
}

fn relative_pattern(base: &Path, raw: &str) -> String {
    format!("{}{separator}{raw}", glob::Pattern::escape(&base.to_string_lossy()), separator = std::path::MAIN_SEPARATOR)
}

async fn fetch_remote(url: &str) -> Option<String> {
    // Bound headers AND body consumption, including slow streaming responses.
    tokio::time::timeout(Duration::from_secs(5), async {
        reqwest::Client::new().get(url).send().await.ok()?
            .error_for_status().ok()?.text().await.ok()
    }).await.ok().flatten()
}

async fn reconcile(key: CacheKey, previous: &InstructionState) -> InstructionState {
    let changed = previous.key.as_ref() != Some(&key);
    let configured = if changed { expand_configured(&key) } else { previous.configured.clone() };
    let mut remote = BTreeMap::new();
    for source in &configured {
        if let Source::Remote(url) = source {
            let value = if changed { fetch_remote(url).await } else { None }
                .or_else(|| previous.remote.get(url).cloned());
            if let Some(value) = value { remote.insert(url.clone(), value); }
        }
    }
    let mut loaded = Vec::new();
    let mut seen = BTreeSet::new();
    for source in default_paths(&key).into_iter().map(Source::Local).chain(configured.clone()) {
        let (name, content) = match source {
            Source::Remote(url) => {
                let content = remote.get(&url).cloned();
                (url, content)
            },
            Source::Local(path) => {
                let name = normalize(&path).to_string_lossy().into_owned();
                let content = match std::fs::read_to_string(&path) {
                    Ok(content) => Some(content),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
                    Err(_) => {
                        // Prior loaded values are formatted already; preserve
                        // them verbatim on permission/encoding/transient errors.
                        if seen.insert(name.clone()) {
                            if let Some(old) = previous.loaded.iter().find(|item| item.filepath == name) {
                                loaded.push(old.clone());
                            }
                        }
                        continue;
                    },
                };
                (name, content)
            },
        };
        if !seen.insert(name.clone()) { continue; }
        if let Some(content) = content.filter(|content| !content.trim().is_empty()) {
            loaded.push(LoadedInstruction {
                content: format!("Instructions from: {}\n{}", name, content.trim()),
                filepath: name,
            });
        }
    }
    InstructionState { key: Some(key), configured, remote, loaded }
}

pub(crate) fn nearby(directory: &Path, filepath: &Path) -> Vec<LoadedInstruction> {
    if project_disabled() { return Vec::new(); }
    let root = normalize(directory);
    let target = normalize(filepath);
    let start = if target.is_dir() { target.as_path() } else { target.parent().unwrap_or(&target) };
    let mut seen = BTreeSet::new();
    ancestors(start, &root).into_iter().take_while(|dir| dir != &root)
        .filter_map(|dir| {
            let path = normalize(&dir.join("AGENTS.md"));
            if path == target || !seen.insert(path.clone()) { return None; }
            let content = std::fs::read_to_string(&path).ok()?;
            if content.trim().is_empty() { return None; }
            Some(LoadedInstruction {
                filepath: path.display().to_string(),
                content: format!("Instructions from: {}\n{}", path.display(), content.trim()),
            })
        }).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture(PathBuf);

    impl Fixture {
        fn new() -> Self {
            let root = std::env::temp_dir().join(format!("neoism-instructions-{}",
                neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)));
            std::fs::create_dir_all(root.join("home/project/sub")).unwrap();
            std::fs::create_dir_all(root.join("global")).unwrap();
            Self(normalize(&root))
        }

        fn write(&self, path: &str, text: &str) {
            let path = self.0.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }

        fn key(&self, instructions: &[&str]) -> CacheKey {
            CacheKey {
                generation: 1,
                workspace: self.0.join("home/project/sub"),
                project_root: self.0.join("home/project"),
                global_dirs: vec![self.0.join("global")],
                home: Some(self.0.join("home")),
                disabled: false,
                instructions: instructions.iter().map(|value| value.to_string()).collect(),
            }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    #[tokio::test]
    async fn snapshot_discovery_uses_first_installation_root_not_workspace_config() {
        use neoism_agent_service_api::{ConfigDiscoveryRoot, ConfigSnapshot,
            ConfigSnapshotRequest, ConfigSourceService, ConfigUpdateRequest,
            ConfigWritableTarget, ServiceError, ServiceFuture};
        use std::sync::Arc;

        struct ConfigSource(Vec<ConfigDiscoveryRoot>);
        impl ConfigSourceService for ConfigSource {
            fn snapshot(&self, request: &ConfigSnapshotRequest) -> Result<ConfigSnapshot, ServiceError> {
                Ok(ConfigSnapshot {
                    identity: "instructions-test".into(),
                    workspace: request.workspace.clone(),
                    layers: Vec::new(),
                    discovery_roots: self.0.clone(),
                    writable_target: ConfigWritableTarget { source_id: "test".into(), label: "test".into() },
                })
            }
            fn update<'a>(&'a self, _: &'a ConfigUpdateRequest) -> ServiceFuture<'a, Result<ConfigSnapshot, ServiceError>> {
                Box::pin(async { Err(ServiceError::new("read only")) })
            }
        }

        let f = Fixture::new();
        f.write("global/AGENTS.md", "installation");
        f.write("second-global/AGENTS.md", "not the selected installation");
        f.write("home/project/sub/.neoism/AGENTS.md", "not global");
        let roots = [
            (ConfigDiscoveryScope::Workspace, "home/project/sub/.neoism"),
            (ConfigDiscoveryScope::Installation, "global"),
            (ConfigDiscoveryScope::Installation, "second-global"),
        ].into_iter().map(|(scope, path)| ConfigDiscoveryRoot {
            scope, source_id: path.into(), path: f.0.join(path),
        }).collect();
        let services = crate::standard_services().with_config(Arc::new(ConfigSource(roots)));
        let key = f.key(&[]);
        let state = load(&services, key.workspace.to_str().unwrap(), &key.project_root,
            &AgentConfigDocument::default(), 1, &InstructionState::default()).await;
        assert_eq!(state.source_names(), vec![f.0.join("global/AGENTS.md").display().to_string()]);
        assert!(state.contents()[0].ends_with("installation"));
    }

    #[tokio::test]
    async fn outside_project_keeps_configured_sources_and_resolves_relative_to_global() {
        let f = Fixture::new();
        f.write("global/rules/a.md", "relative global");
        f.write("home/rules/a.md", "not relative workspace");
        f.write("home/extra.md", "absolute or home");
        let absolute = f.0.join("home/extra.md").display().to_string();
        let mut key = f.key(&["rules/*.md", &absolute, "~/extra.md", "https://example.com/rules"]);
        key.workspace = f.0.join("home");
        let sources = expand_configured(&key);
        assert_eq!(sources, vec![
            Source::Local(f.0.join("global/rules/a.md")),
            Source::Local(f.0.join("home/extra.md")),
            Source::Remote("https://example.com/rules".into()),
        ]);
        key.instructions.pop();
        let state = reconcile(key, &InstructionState::default()).await;
        assert_eq!(state.contents().len(), 2);
        assert!(state.contents()[0].ends_with("relative global"));
    }

    #[tokio::test]
    async fn global_then_nearest_ancestors_agents_only() {
        let f = Fixture::new();
        for (path, text) in [
            ("global/AGENTS.md", "global"),
            ("home/AGENTS.md", "home"),
            ("home/project/AGENTS.md", "project"),
            ("home/project/sub/AGENTS.md", "sub"),
            ("home/project/sub/CLAUDE.md", "ignored"),
            ("home/project/sub/CONTEXT.md", "ignored"),
            ("home/project/sub/.neoism/AGENTS.md", "ignored"),
            ("AGENTS.md", "above home"),
        ] { f.write(path, text); }
        let state = reconcile(f.key(&[]), &InstructionState::default()).await;
        assert_eq!(state.contents().iter().map(|s| s.lines().last().unwrap()).collect::<Vec<_>>(),
            ["global", "sub", "project", "home"]);
    }

    #[tokio::test]
    async fn explicit_boundary_outside_home_and_outside_project() {
        let f = Fixture::new();
        f.write("global/AGENTS.md", "global");
        f.write("home/AGENTS.md", "above");
        f.write("home/project/AGENTS.md", "project");
        let mut key = f.key(&[]);
        key.home = Some(f.0.join("elsewhere"));
        let state = reconcile(key.clone(), &InstructionState::default()).await;
        assert_eq!(state.contents().len(), 2);
        key.workspace = f.0.join("home");
        key.instructions = vec![f.0.join("home/AGENTS.md").display().to_string()];
        let state = reconcile(key, &state).await;
        assert_eq!(state.contents().len(), 2);
        assert!(state.contents()[0].ends_with("global"));
    }

    #[tokio::test]
    async fn disabled_uses_global_relative_paths_only() {
        let f = Fixture::new();
        f.write("home/project/AGENTS.md", "project");
        f.write("home/project/sub/extra.md", "local extra");
        f.write("global/extra.md", "global extra");
        let mut key = f.key(&["extra.md"]);
        key.disabled = true;
        let state = reconcile(key, &InstructionState::default()).await;
        assert_eq!(state.contents().len(), 1);
        assert!(state.contents()[0].ends_with("global extra"));
    }

    #[tokio::test]
    async fn configured_patterns_are_ordered_bounded_and_deduplicated() {
        let f = Fixture::new();
        f.write("home/project/sub/rules/b.md", "b");
        f.write("home/project/sub/rules/a.md", "a");
        f.write("home/project/rules/c.md", "c");
        f.write("home/rules/no.md", "outside boundary");
        f.write("home/extra.md", "home extra");
        let absolute = f.0.join("home/project/rules/*.md").display().to_string();
        let key = f.key(&["rules/*.md", &absolute, "~/extra.md", "[invalid"]);
        let state = reconcile(key, &InstructionState::default()).await;
        assert_eq!(state.contents().iter().map(|s| s.lines().last().unwrap()).collect::<Vec<_>>(),
            ["a", "b", "c", "home extra"]);
    }

    #[tokio::test]
    async fn local_refresh_errors_deletions_and_generation_cached_membership() {
        let f = Fixture::new();
        f.write("home/project/sub/a.md", "one");
        let mut key = f.key(&["*.md"]);
        let first = reconcile(key.clone(), &InstructionState::default()).await;
        f.write("home/project/sub/a.md", "two");
        f.write("home/project/sub/b.md", "new match");
        let second = reconcile(key.clone(), &first).await;
        assert_eq!(second.contents().len(), 1);
        assert!(second.contents()[0].ends_with("two"));
        // Invalid UTF-8 is a read error, not a deletion.
        std::fs::write(f.0.join("home/project/sub/a.md"), [0xff]).unwrap();
        assert_eq!(reconcile(key.clone(), &second).await.contents(), second.contents());
        f.write("home/project/sub/a.md", " \n");
        assert!(reconcile(key.clone(), &second).await.contents().is_empty());
        std::fs::remove_file(f.0.join("home/project/sub/a.md")).unwrap();
        assert!(reconcile(key.clone(), &second).await.contents().is_empty());
        key.generation += 1;
        let third = reconcile(key, &second).await;
        assert_eq!(third.contents().len(), 1);
        assert!(third.contents()[0].ends_with("new match"));
        let encoded = serde_json::to_string(&third).unwrap();
        assert_eq!(third, serde_json::from_str(&encoded).unwrap());
    }

    #[tokio::test]
    async fn default_files_are_discovered_each_time_and_workspace_invalidates_globs() {
        let f = Fixture::new();
        let mut key = f.key(&["*.txt"]);
        let first = reconcile(key.clone(), &InstructionState::default()).await;
        f.write("home/project/sub/AGENTS.md", "new default");
        f.write("home/project/sub/new.txt", "new configured");
        let second = reconcile(key.clone(), &first).await;
        assert_eq!(second.contents().len(), 1);
        key.workspace = f.0.join("home/project");
        let third = reconcile(key.clone(), &second).await;
        key.workspace = f.0.join("home/project/sub");
        assert_eq!(reconcile(key, &third).await.contents().len(), 2);
    }

    #[tokio::test]
    async fn missing_literal_sources_are_retried_without_generation_change() {
        let f = Fixture::new();
        let key = f.key(&["later.txt"]);
        let first = reconcile(key.clone(), &InstructionState::default()).await;
        assert!(first.contents().is_empty());
        f.write("home/project/sub/later.txt", "created");
        let second = reconcile(key, &first).await;
        assert_eq!(second.contents().len(), 1);
        assert!(second.contents()[0].ends_with("created"));
    }

    #[tokio::test]
    async fn remote_cached_until_generation_change_and_failed_refresh_keeps_last_good() {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/instructions", listener.local_addr().unwrap());
        let server = std::thread::spawn(move || {
            for response in [
                "HTTP/1.1 200 OK\r\nContent-Length: 4\r\nConnection: close\r\n\r\ngood",
                "HTTP/1.1 500 Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            ] {
                let (mut stream, _) = listener.accept().unwrap();
                stream.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
                let mut request = [0; 4096];
                stream.read(&mut request).unwrap();
                stream.write_all(response.as_bytes()).unwrap();
            }
        });
        let f = Fixture::new();
        let mut key = f.key(&[&url, &url]);
        let first = reconcile(key.clone(), &InstructionState::default()).await;
        assert_eq!(first.source_names(), vec![url]);
        assert!(first.contents()[0].ends_with("good"));
        let cached = reconcile(key.clone(), &first).await;
        assert_eq!(first, cached);
        key.generation += 1;
        let failed = reconcile(key.clone(), &cached).await;
        assert_eq!(first.contents(), failed.contents());
        key.generation += 1;
        assert!(reconcile(key, &failed).await.contents().is_empty());
        server.join().unwrap();
    }

    #[test]
    fn nearby_is_nearest_first_agents_only_and_excludes_root_and_target() {
        if project_disabled() { return; }
        let f = Fixture::new();
        f.write("home/project/AGENTS.md", "root");
        f.write("home/project/sub/AGENTS.md", "sub");
        f.write("home/project/sub/deep/AGENTS.md", "deep");
        f.write("home/project/sub/deep/CLAUDE.md", "ignored");
        let root = f.0.join("home/project");
        let deep = root.join("sub/deep");
        let loaded = nearby(&root, &deep.join("file.rs"));
        assert_eq!(loaded.len(), 2);
        assert!(loaded[0].content.ends_with("deep"));
        assert!(loaded[1].content.ends_with("sub"));
        assert_eq!(nearby(&root, &deep.join("AGENTS.md")).len(), 1);
        assert!(nearby(&root, &f.0.join("outside/file.rs")).is_empty());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn canonical_aliases_deduplicate_defaults_and_configured_sources() {
        let f = Fixture::new();
        f.write("home/project/sub/AGENTS.md", "one");
        std::os::unix::fs::symlink("AGENTS.md", f.0.join("home/project/sub/alias.md")).unwrap();
        let state = reconcile(f.key(&["*.md"]), &InstructionState::default()).await;
        assert_eq!(state.contents().len(), 1);
    }
}