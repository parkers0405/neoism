//! Metadata-only discovery and externally-authorized Agent package activation.

use std::path::{Component, Path, PathBuf};

use neoism_agent_plugin_api::{
    AgentEntrypointRuntime, NeoismPackageManifest, PackageDiagnostic, PackageLifecycleInfo,
    PackageLifecycleState, PackageLocation, PackageTrustRecord, PluginScope,
    PROCESS_PLUGIN_V2_PROTOCOL,
};
use neoism_agent_core::{PluginConfig, PluginManifestInfo};
use serde_json::Value;
use sha2::{Digest, Sha256};

const MAX_PACKAGE_FILES: usize = 4096;
const MAX_PACKAGE_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Clone, Debug)]
pub(crate) struct DiscoveredAgentPackage {
    pub(crate) root: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) manifest: NeoismPackageManifest,
    pub(crate) revision: String,
    pub(crate) location: PackageLocation,
}

pub(crate) fn discover(directory: &str) -> Vec<Result<DiscoveredAgentPackage, String>> {
    let mut roots = Vec::new();
    if let Some(config) = dirs::config_dir() {
        roots.push((config.join("neoism/plugins"), PackageLocation::User));
    }
    roots.push((
        Path::new(directory).join(".neoism/plugins"),
        PackageLocation::Workspace,
    ));
    let mut found = Vec::new();
    for (root, location) in roots {
        let Ok(entries) = std::fs::read_dir(&root) else {
            continue;
        };
        let mut entries = entries.filter_map(Result::ok).collect::<Vec<_>>();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let package_root = entry.path();
            if !package_root.is_dir() || !package_root.join("neoism-plugin.json").is_file() {
                continue;
            }
            found.push(load(&package_root, location));
        }
    }
    found
}

fn load(root: &Path, location: PackageLocation) -> Result<DiscoveredAgentPackage, String> {
    let manifest_path = root.join("neoism-plugin.json");
    let raw = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("failed to read {}: {error}", manifest_path.display()))?;
    let manifest: NeoismPackageManifest = serde_json::from_str(&raw)
        .map_err(|error| format!("invalid {}: {error}", manifest_path.display()))?;
    validate(&manifest, root, location)?;
    let revision = package_revision(root)?;
    Ok(DiscoveredAgentPackage {
        root: root.to_path_buf(),
        manifest_path,
        manifest,
        revision,
        location,
    })
}

fn validate(
    manifest: &NeoismPackageManifest,
    root: &Path,
    location: PackageLocation,
) -> Result<(), String> {
    if !valid_id(&manifest.id) {
        return Err("package id must be a lowercase reverse-DNS name".into());
    }
    let Some(agent) = &manifest.agent else {
        return Ok(());
    };
    if location == PackageLocation::Workspace
        && matches!(agent.scope, PluginScope::Global | PluginScope::User)
    {
        return Err("workspace packages cannot declare global or user Agent scope".into());
    }
    if matches!(agent.runtime, AgentEntrypointRuntime::Lua) {
        validate_entrypoint(root, &agent.entrypoint)?;
    } else if agent.command.is_empty() {
        validate_entrypoint(root, &agent.entrypoint)?;
    }
    Ok(())
}

fn validate_entrypoint(root: &Path, entrypoint: &str) -> Result<PathBuf, String> {
    let path = Path::new(entrypoint);
    if entrypoint.is_empty()
        || path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return Err("agent entrypoint must be a contained relative path".into());
    }
    let root = root
        .canonicalize()
        .map_err(|error| format!("package root is unavailable: {error}"))?;
    let entrypoint = root
        .join(path)
        .canonicalize()
        .map_err(|error| format!("agent entrypoint is unavailable: {error}"))?;
    if !entrypoint.starts_with(&root) || !entrypoint.is_file() {
        return Err("agent entrypoint escapes the package or is not a file".into());
    }
    Ok(entrypoint)
}

fn valid_id(id: &str) -> bool {
    let parts = id.split('.').collect::<Vec<_>>();
    parts.len() >= 2
        && parts.iter().all(|part| {
            !part.is_empty()
                && part.as_bytes()[0].is_ascii_lowercase()
                && part
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
                && !part.ends_with('-')
        })
}

fn package_revision(root: &Path) -> Result<String, String> {
    fn visit(
        root: &Path,
        directory: &Path,
        files: &mut Vec<PathBuf>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(directory)
            .map_err(|error| format!("failed to inspect package: {error}"))?
        {
            let entry = entry.map_err(|error| format!("failed to inspect package: {error}"))?;
            let file_type = entry
                .file_type()
                .map_err(|error| format!("failed to inspect package: {error}"))?;
            if file_type.is_symlink() {
                return Err("package revisions do not permit symlinks".into());
            }
            if file_type.is_dir() {
                visit(root, &entry.path(), files)?;
            } else if file_type.is_file() {
                files.push(entry.path());
                if files.len() > MAX_PACKAGE_FILES {
                    return Err("package has too many files".into());
                }
            }
        }
        let _ = root;
        Ok(())
    }
    let mut files = Vec::new();
    visit(root, root, &mut files)?;
    files.sort();
    let mut total = 0_u64;
    let mut digest = Sha256::new();
    for file in files {
        let relative = file
            .strip_prefix(root)
            .map_err(|_| "package file escaped root".to_string())?;
        let bytes = std::fs::read(&file)
            .map_err(|error| format!("failed to hash {}: {error}", file.display()))?;
        total = total.saturating_add(bytes.len() as u64);
        if total > MAX_PACKAGE_BYTES {
            return Err("package exceeds metadata hashing budget".into());
        }
        digest.update(relative.to_string_lossy().as_bytes());
        digest.update([0]);
        digest.update(&bytes);
        digest.update([0xff]);
    }
    Ok(format!("sha256:{:x}", digest.finalize()))
}

pub(crate) fn trust_record(options: &std::collections::BTreeMap<String, Value>) -> Option<PackageTrustRecord> {
    options
        .get("trust")
        .cloned()
        .and_then(|value| serde_json::from_value(value).ok())
}

pub(crate) fn authorized(
    package: &DiscoveredAgentPackage,
    workspace_id: &str,
    options: &std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    authorized_for(
        package,
        PluginScope::Workspace,
        Some(workspace_id),
        None,
        options,
    )
}

pub(crate) fn authorized_for(
    package: &DiscoveredAgentPackage,
    runtime_scope: PluginScope,
    workspace_id: Option<&str>,
    scope_id: Option<&str>,
    options: &std::collections::BTreeMap<String, Value>,
) -> Result<(), String> {
    let trust = trust_record(options).ok_or_else(|| "external trust record is required".to_string())?;
    let agent = package
        .manifest
        .agent
        .as_ref()
        .ok_or_else(|| "package has no Agent entrypoint".to_string())?;
    if agent.scope != runtime_scope
        || trust.scope.is_some_and(|scope| scope != runtime_scope)
        || trust.package_id != package.manifest.id
        || trust.revision != package.revision
    {
        return Err("trust record does not match package revision and runtime scope".into());
    }
    match runtime_scope {
        PluginScope::Workspace | PluginScope::Session => {
            if workspace_id != Some(trust.workspace_id.as_str()) {
                return Err("trust record does not match workspace identity".into());
            }
        }
        PluginScope::Global | PluginScope::User => {
            if package.location != PackageLocation::User {
                return Err("only installation packages may activate outside workspace scope".into());
            }
        }
    }
    if matches!(runtime_scope, PluginScope::User | PluginScope::Session)
        && trust.scope_id.as_deref() != scope_id
    {
        return Err("trust record does not match exact user/session pin".into());
    }
    for capability in &agent.capabilities {
        if !trust.capabilities.contains(capability) {
            return Err(format!("capability {capability:?} requires approval"));
        }
    }
    match agent.runtime {
        AgentEntrypointRuntime::Lua if !trust.lua_approved => {
            Err("Lua execution requires explicit approval".into())
        }
        AgentEntrypointRuntime::Process if !trust.executable_approved => {
            Err("executable entrypoint requires explicit approval".into())
        }
        _ => Ok(()),
    }
}

pub(crate) fn lifecycle_manifests(
    directory: &str,
    configured: &std::collections::BTreeMap<String, PluginConfig>,
) -> Vec<PluginManifestInfo> {
    discover(directory)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|package| package.manifest.agent.is_some())
        .map(|package| {
            let config = configured.get(&package.manifest.id);
            let (state, enabled, reason) = match config {
                None => (PackageLifecycleState::PermissionRequired, false, Some("external trust is required".to_string())),
                Some(config) if !config.enabled => (PackageLifecycleState::Disabled, false, Some("disabled by installation configuration".to_string())),
                Some(config) => {
                    let agent = package.manifest.agent.as_ref().expect("filtered Agent package");
                    let trust = trust_record(&config.options);
                    match authorized_for(
                        &package,
                        agent.scope,
                        matches!(agent.scope, PluginScope::Workspace | PluginScope::Session)
                            .then_some(directory),
                        trust.as_ref().and_then(|trust| trust.scope_id.as_deref()),
                        &config.options,
                    ) {
                    Ok(()) => (PackageLifecycleState::Trusted, true, None),
                    Err(reason) => (PackageLifecycleState::PermissionRequired, true, Some(reason)),
                    }
                }
            };
            let capabilities = package
                .manifest
                .agent
                .as_ref()
                .map(|agent| {
                    agent.capabilities
                        .iter()
                        .filter_map(|capability| serde_json::to_value(capability).ok()?.as_str().map(str::to_string))
                        .collect()
                })
                .unwrap_or_default();
            let agent = package.manifest.agent.as_ref().expect("filtered Agent package");
            let requested_capabilities = agent.capabilities.clone();
            let granted_capabilities = config
                .and_then(|config| trust_record(&config.options))
                .map(|trust| {
                    requested_capabilities
                        .iter()
                        .copied()
                        .filter(|capability| trust.capabilities.contains(capability))
                        .collect()
                })
                .unwrap_or_default();
            let lifecycle = PackageLifecycleInfo {
                package_id: package.manifest.id.clone(),
                state,
                source: package.location,
                revision: package.revision.clone(),
                scope: agent.scope,
                scope_id: config
                    .and_then(|config| trust_record(&config.options))
                    .and_then(|trust| trust.scope_id),
                requested_capabilities,
                granted_capabilities,
                retained_revision: None,
                lease_active: false,
                diagnostics: reason
                    .as_ref()
                    .map(|message| vec![PackageDiagnostic {
                        code: if state == PackageLifecycleState::PermissionRequired {
                            "permission-required".into()
                        } else {
                            "inactive".into()
                        },
                        message: message.clone(),
                        action: (state == PackageLifecycleState::PermissionRequired)
                            .then(|| "Approve this exact package revision in installation configuration".into()),
                    }])
                    .unwrap_or_default(),
            };
            PluginManifestInfo {
                id: package.manifest.id,
                name: package.manifest.name,
                version: package.manifest.version,
                plugin_api: PROCESS_PLUGIN_V2_PROTOCOL.into(),
                internal: false,
                enabled,
                active: false,
                disableable: true,
                capabilities,
                requires: Vec::new(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                reason,
                config: std::collections::BTreeMap::from([
                    ("agentLifecycle".into(), serde_json::to_value(state).unwrap_or(Value::Null)),
                    ("packageRevision".into(), Value::String(package.revision)),
                    ("packageLocation".into(), serde_json::to_value(package.location).unwrap_or(Value::Null)),
                    ("agentLifecycleInfo".into(), serde_json::to_value(lifecycle).unwrap_or(Value::Null)),
                ]),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_package_is_metadata_only_until_exact_revision_is_trusted() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let base = std::env::temp_dir().join(format!("neoism-agent-package-{unique}"));
        let root = base.join(".neoism/plugins/dev.example.fixture");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent.lua"), "return {}").unwrap();
        std::fs::write(
            root.join("neoism-plugin.json"),
            r#"{"id":"dev.example.fixture","name":"Fixture","version":"1","agent":{"runtime":"lua","entrypoint":"agent.lua","capabilities":["workspace-read"]}}"#,
        )
        .unwrap();
        let package = discover(base.to_str().unwrap())
            .into_iter()
            .filter_map(Result::ok)
            .find(|package| package.location == PackageLocation::Workspace && package.manifest.id == "dev.example.fixture")
            .unwrap();
        assert_eq!(package.location, PackageLocation::Workspace);
        assert!(authorized(&package, base.to_str().unwrap(), &Default::default()).is_err());
        let discovered = lifecycle_manifests(base.to_str().unwrap(), &Default::default())
            .into_iter()
            .find(|manifest| manifest.id == "dev.example.fixture")
            .unwrap();
        assert!(!discovered.active);
        assert_eq!(discovered.config["agentLifecycle"], "permission-required");
        let trust = PackageTrustRecord {
            workspace_id: base.to_string_lossy().into_owned(),
            package_id: package.manifest.id.clone(),
            revision: package.revision.clone(),
            scope: Some(PluginScope::Workspace),
            scope_id: None,
            capabilities: vec![neoism_agent_plugin_api::HostCapability::WorkspaceRead],
            executable_approved: false,
            native_approved: false,
            lua_approved: true,
        };
        let options = std::collections::BTreeMap::from([(
            "trust".into(),
            serde_json::to_value(trust).unwrap(),
        )]);
        assert!(authorized(&package, base.to_str().unwrap(), &options).is_ok());
        let configured = std::collections::BTreeMap::from([(
            package.manifest.id.clone(),
            PluginConfig { enabled: true, options, ..PluginConfig::default() },
        )]);
        let trusted = lifecycle_manifests(base.to_str().unwrap(), &configured)
            .into_iter()
            .find(|manifest| manifest.id == "dev.example.fixture")
            .unwrap();
        assert_eq!(trusted.config["agentLifecycle"], "trusted");
        std::fs::write(root.join("agent.lua"), "return { version = 2 }").unwrap();
        let changed = load(&root, PackageLocation::Workspace).unwrap();
        assert_ne!(changed.revision, package.revision);
        assert!(authorized(&changed, base.to_str().unwrap(), &configured["dev.example.fixture"].options).is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn session_scope_requires_installation_package_and_exact_session_pin() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neoism-agent-session-package-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent.lua"), "return {}").unwrap();
        std::fs::write(
            root.join("neoism-plugin.json"),
            r#"{"id":"dev.example.session","name":"Session","version":"1","agent":{"runtime":"lua","entrypoint":"agent.lua","scope":"session","capabilities":["prompt-read"]}}"#,
        )
        .unwrap();
        let package = load(&root, PackageLocation::User).unwrap();
        let trust = PackageTrustRecord {
            workspace_id: "workspace-1".into(),
            package_id: package.manifest.id.clone(),
            revision: package.revision.clone(),
            scope: Some(PluginScope::Session),
            scope_id: Some("session-1".into()),
            capabilities: vec![neoism_agent_plugin_api::HostCapability::PromptRead],
            executable_approved: false,
            native_approved: false,
            lua_approved: true,
        };
        let options = std::collections::BTreeMap::from([(
            "trust".into(),
            serde_json::to_value(trust).unwrap(),
        )]);
        assert!(authorized_for(
            &package,
            PluginScope::Session,
            Some("workspace-1"),
            Some("session-1"),
            &options,
        )
        .is_ok());
        assert!(authorized_for(
            &package,
            PluginScope::Session,
            Some("workspace-1"),
            Some("session-2"),
            &options,
        )
        .unwrap_err()
        .contains("session pin"));
        assert!(authorized_for(
            &package,
            PluginScope::Workspace,
            Some("workspace-1"),
            None,
            &options,
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn workspace_package_cannot_claim_installation_scope() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neoism-agent-global-package-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent.lua"), "return {}").unwrap();
        std::fs::write(
            root.join("neoism-plugin.json"),
            r#"{"id":"dev.example.global","name":"Global","version":"1","agent":{"runtime":"lua","entrypoint":"agent.lua","scope":"global"}}"#,
        )
        .unwrap();
        assert!(load(&root, PackageLocation::Workspace)
            .unwrap_err()
            .contains("cannot declare global"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn editor_only_manifest_is_parsed_without_evaluating_lua() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neoism-editor-only-package-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("editor.lua"), "this is deliberately not valid Lua !!!").unwrap();
        std::fs::write(
            root.join("neoism-plugin.json"),
            r#"{"id":"dev.example.editor","name":"Editor only","version":"1","entrypoint":"editor.lua"}"#,
        )
        .unwrap();
        let package = load(&root, PackageLocation::User).unwrap();
        assert!(package.manifest.agent.is_none());
        assert_eq!(package.manifest.entrypoint.as_deref(), Some("editor.lua"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn installation_package_global_scope_uses_exact_revision_trust() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root = std::env::temp_dir().join(format!("neoism-installation-package-{unique}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("agent.lua"), "return {}").unwrap();
        std::fs::write(
            root.join("neoism-plugin.json"),
            r#"{"id":"dev.example.installation","name":"Installation","version":"1","agent":{"runtime":"lua","entrypoint":"agent.lua","scope":"global"}}"#,
        )
        .unwrap();
        let package = load(&root, PackageLocation::User).unwrap();
        let trust = PackageTrustRecord {
            workspace_id: "installation".into(),
            package_id: package.manifest.id.clone(),
            revision: package.revision.clone(),
            scope: Some(PluginScope::Global),
            scope_id: None,
            capabilities: Vec::new(),
            executable_approved: false,
            native_approved: false,
            lua_approved: true,
        };
        let options = std::collections::BTreeMap::from([("trust".into(), serde_json::to_value(trust).unwrap())]);
        assert!(authorized_for(&package, PluginScope::Global, None, None, &options).is_ok());
        let _ = std::fs::remove_dir_all(root);
    }
}