use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const NEOISM_DIR: &str = ".neoism";
pub const WORKSPACE_FILE: &str = "workspace.json";
/// Pre-JSON workspace file, read once to upgrade in place.
const WORKSPACE_FILE_LEGACY: &str = "workspace.toml";
pub const CURRENT_WORKSPACE_CONFIG_VERSION: u32 = 6;
pub const DEFAULT_NOTES_WORKSPACE: &str = "Default";
pub const DEFAULT_NOTES_VAULTS_DIR: &str = "Neoism/Vaults";
pub const DEFAULT_NOTES_INDEX: &str = "Start Here.md";
pub const WELCOME_DIR: &str = "Welcome";
pub const PROJECT_METADATA_FILE: &str = "project.json";
/// Pre-JSON vault metadata file, read once to upgrade in place.
const PROJECT_METADATA_FILE_LEGACY: &str = "project.toml";
pub(crate) const DEFAULT_NOTES_WORKSPACE_ID: &str = "neoism-notes-default-v1";
const WELCOME_SEEDED_MARKER: &str = ".neoism-welcome-seeded-v6";

const REPLACED_WELCOME_PATHS: &[&str] = &[
    "Getting Started.md",
    "The Terminal.md",
    "The Neoism Agent.md",
    "Notes and Drawings.md",
    "Multiplayer.md",
    "Keybindings.md",
    "Editor",
    "Configuration",
];

#[derive(Debug, Clone)]
pub struct NeoismWorkspace {
    pub root: PathBuf,
    pub config: WorkspaceConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkspaceConfig {
    pub version: u32,
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub notes: NotesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotesConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_notes_workspace_name")]
    pub workspace: String,
    /// Stable vault identity. `workspace` remains the human-readable legacy
    /// name, while this id survives renames.
    #[serde(default)]
    pub vault_id: Option<String>,
    /// Project-relative notes root inside the vault. `.` preserves the
    /// original whole-vault behavior.
    #[serde(default = "default_notes_scope")]
    pub scope: PathBuf,
    #[serde(default = "default_note_ignores")]
    pub ignore: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VaultProjectMetadata {
    #[serde(default = "default_project_metadata_version")]
    pub version: u32,
    pub name: String,
    #[serde(default)]
    pub links: Vec<ProjectLink>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectLink {
    pub kind: String,
    pub path: PathBuf,
    pub label: String,
    /// Folder inside the owning vault used by this code project. Existing
    /// metadata omits it and therefore migrates to the whole vault (`.`).
    #[serde(default = "default_notes_scope")]
    pub notes_path: PathBuf,
}

#[derive(Debug, Clone)]
pub struct ResolvedNotesScope {
    pub workspace: NeoismWorkspace,
    pub vault: crate::vaults::NotesVault,
    pub project_root: PathBuf,
    pub relative_path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedVaultScope {
    pub source_vault_name: String,
    pub destination_vault: crate::vaults::NotesVault,
    pub scope_dir: PathBuf,
    pub links_moved: usize,
}

impl Default for NotesConfig {
    fn default() -> Self {
        let default_vault = crate::vaults::default_notes_vault().ok();
        Self {
            enabled: true,
            workspace: default_vault
                .as_ref()
                .map(|vault| vault.name.clone())
                .unwrap_or_else(|| DEFAULT_NOTES_WORKSPACE.to_string()),
            vault_id: default_vault.map(|vault| vault.id),
            scope: default_notes_scope(),
            ignore: default_note_ignores(),
        }
    }
}

impl WorkspaceConfig {
    pub fn new(root: &Path) -> Self {
        let name = root
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("workspace")
            .to_string();
        Self {
            version: CURRENT_WORKSPACE_CONFIG_VERSION,
            id: Uuid::new_v4().to_string(),
            name,
            notes: NotesConfig::default(),
        }
    }
}

impl NeoismWorkspace {
    pub fn cache_dir(&self) -> PathBuf {
        global_cache_dir().join("vaults").join(cache_key(
            self.config
                .notes
                .vault_id
                .as_deref()
                .unwrap_or(&self.config.notes.workspace),
        ))
    }

    pub fn notes_vault_dir(&self) -> PathBuf {
        let by_id = self
            .config
            .notes
            .vault_id
            .as_deref()
            .and_then(|id| crate::vaults::notes_vault_by_id(id).ok().flatten());
        let by_name = crate::vaults::notes_vault_by_name(&self.config.notes.workspace)
            .ok()
            .flatten();
        // Existing integrations historically selected a vault by replacing
        // only the display name. Honor that when the named vault exists. If
        // the name is stale because that same vault was renamed, its stable
        // id remains authoritative instead.
        match (by_id, by_name) {
            (Some(id_vault), Some(name_vault)) if id_vault.id != name_vault.id => {
                name_vault.path
            }
            (Some(vault), _) | (None, Some(vault)) => vault.path,
            (None, None) => notes_workspace_dir(&self.config.notes.workspace),
        }
    }

    pub fn notes_workspace_dir(&self) -> PathBuf {
        let scope = normalized_notes_scope(&self.config.notes.scope);
        if scope == Path::new(".") {
            self.notes_vault_dir()
        } else {
            self.notes_vault_dir().join(scope)
        }
    }

    pub fn notes_scope_relative(&self) -> PathBuf {
        normalized_notes_scope(&self.config.notes.scope)
    }

    pub fn as_vault_workspace(&self) -> Self {
        let mut workspace = self.clone();
        workspace.config.notes.scope = default_notes_scope();
        workspace
    }

    pub fn note_roots(&self) -> Vec<PathBuf> {
        if !self.config.notes.enabled {
            return Vec::new();
        }
        let mut roots = vec![self.notes_workspace_dir()];
        roots.sort();
        roots.dedup();
        roots
    }

    pub fn note_path_label(&self, path: &Path) -> String {
        let path = if path.is_absolute() {
            path.to_path_buf()
        } else {
            self.root.join(path)
        };
        for root in self.note_roots() {
            if let Ok(relative) = path.strip_prefix(&root) {
                if !relative.as_os_str().is_empty() {
                    return path_components(relative);
                }
            }
        }
        path_components(path.strip_prefix(&self.root).unwrap_or(&path))
    }

    pub fn resolve_note_path(&self, path: &Path) -> PathBuf {
        if path.is_absolute() {
            return path.to_path_buf();
        }
        let roots = self.note_roots();
        for root in &roots {
            let candidate = root.join(path);
            if candidate.exists() {
                return candidate;
            }
        }
        roots
            .first()
            .cloned()
            .unwrap_or_else(|| self.root.clone())
            .join(path)
    }
}

pub fn workspace_config_path(root: &Path) -> PathBuf {
    root.join(NEOISM_DIR).join(WORKSPACE_FILE)
}

fn workspace_config_path_legacy(root: &Path) -> PathBuf {
    root.join(NEOISM_DIR).join(WORKSPACE_FILE_LEGACY)
}

pub fn load_workspace(
    root: impl AsRef<Path>,
) -> std::io::Result<Option<NeoismWorkspace>> {
    let root = normalize_root(root.as_ref())?;
    let path = workspace_config_path(&root);
    let mut config = if path.is_file() {
        let source = fs::read_to_string(&path)?;
        serde_json::from_str::<WorkspaceConfig>(&source).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {err}", path.display()),
            )
        })?
    } else {
        // One-shot upgrade: read a pre-JSON `.neoism/workspace.toml` so the
        // workspace keeps its persistent id, then it is rewritten as JSON
        // the next time it is saved (e.g. on init).
        let legacy = workspace_config_path_legacy(&root);
        if !legacy.is_file() {
            return Ok(None);
        }
        let source = fs::read_to_string(&legacy)?;
        toml::from_str::<WorkspaceConfig>(&source).map_err(|err| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {err}", legacy.display()),
            )
        })?
    };
    migrate_workspace_config(&mut config);
    Ok(Some(NeoismWorkspace { root, config }))
}

pub fn init_workspace(root: impl AsRef<Path>) -> std::io::Result<NeoismWorkspace> {
    let root = normalize_root(root.as_ref())?;
    fs::create_dir_all(&root)?;
    let neoism_dir = root.join(NEOISM_DIR);
    fs::create_dir_all(&neoism_dir)?;

    // Load an existing workspace (upgrading a legacy TOML file if that is
    // all that is present) before falling back to a fresh config, so the
    // persistent id survives the JSON switch.
    if let Some(workspace) = load_workspace(&root)? {
        write_workspace_config(&workspace)?;
        ensure_note_root_dirs(&workspace)?;
        ensure_welcome_docs(&workspace)?;
        return Ok(workspace);
    }

    let config = WorkspaceConfig::new(&root);
    let workspace = NeoismWorkspace { root, config };
    write_workspace_config(&workspace)?;
    ensure_note_root_dirs(&workspace)?;
    ensure_welcome_docs(&workspace)?;
    Ok(workspace)
}

/// The global notes workspace used when the user has not explicitly linked
/// the active code directory to a vault. This is intentionally virtual: it
/// never writes a `.neoism/workspace.json` into the process cwd (which may be
/// `/` for a packaged macOS app) and has a stable identity for graph/cache
/// paths across launches.
pub fn default_notes_workspace() -> NeoismWorkspace {
    let vault = crate::vaults::default_notes_vault().unwrap_or_else(|_| {
        crate::vaults::NotesVault {
            id: DEFAULT_NOTES_WORKSPACE_ID.to_string(),
            name: DEFAULT_NOTES_WORKSPACE.to_string(),
            path: notes_vaults_dir().join(DEFAULT_NOTES_WORKSPACE),
        }
    });
    NeoismWorkspace {
        root: vault.path.clone(),
        config: WorkspaceConfig {
            version: CURRENT_WORKSPACE_CONFIG_VERSION,
            id: vault.id.clone(),
            name: vault.name.clone(),
            notes: NotesConfig {
                workspace: vault.name,
                vault_id: Some(vault.id),
                ..NotesConfig::default()
            },
        },
    }
}

/// A virtual workspace for browsing a vault DIRECTLY — sidebar-driven
/// surfaces (the note graph, tasks/tags views) that must follow the
/// vault the user is VIEWING rather than whatever vault the active code
/// workspace links to. Same shape as [`default_notes_workspace`],
/// pointed at `~/Neoism/Vaults/{name}`; the id is stable per vault so
/// reindex rows stay consistent across opens.
pub fn vault_notes_workspace(name: &str) -> NeoismWorkspace {
    let path = notes_workspace_dir(name);
    let vault = crate::vaults::ensure_notes_vault(name, &path).unwrap_or_else(|_| {
        crate::vaults::NotesVault {
            id: format!("vault:{name}"),
            name: name.to_string(),
            path,
        }
    });
    NeoismWorkspace {
        root: vault.path.clone(),
        config: WorkspaceConfig {
            version: CURRENT_WORKSPACE_CONFIG_VERSION,
            id: vault.id.clone(),
            name: vault.name.clone(),
            notes: NotesConfig {
                workspace: vault.name,
                vault_id: Some(vault.id),
                ..NotesConfig::default()
            },
        },
    }
}

pub fn normalize_root(root: &Path) -> std::io::Result<PathBuf> {
    if root.exists() {
        root.canonicalize()
    } else {
        let parent = root.parent().unwrap_or_else(|| Path::new("."));
        let parent = parent.canonicalize()?;
        Ok(parent.join(root.file_name().unwrap_or_default()))
    }
}

fn default_true() -> bool {
    true
}

fn default_notes_workspace_name() -> String {
    crate::vaults::default_notes_vault()
        .map(|vault| vault.name)
        .unwrap_or_else(|_| DEFAULT_NOTES_WORKSPACE.to_string())
}

fn default_notes_scope() -> PathBuf {
    PathBuf::from(".")
}

fn normalized_notes_scope(scope: &Path) -> PathBuf {
    if scope.as_os_str().is_empty() || scope == Path::new(".") {
        return default_notes_scope();
    }
    scope.to_path_buf()
}

fn default_project_metadata_version() -> u32 {
    1
}

fn default_note_ignores() -> Vec<String> {
    [
        ".git",
        ".hg",
        ".svn",
        ".direnv",
        ".next",
        ".claude",
        ".codex",
        "node_modules",
        "target",
        "dist",
        "build",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

fn migrate_workspace_config(config: &mut WorkspaceConfig) {
    if config.version < CURRENT_WORKSPACE_CONFIG_VERSION {
        if config.notes.workspace.trim().is_empty() {
            config.notes.workspace = default_notes_workspace_name();
        }
        config.version = CURRENT_WORKSPACE_CONFIG_VERSION;
    }
    config.notes.scope = normalized_notes_scope(&config.notes.scope);
    let resolved = config
        .notes
        .vault_id
        .as_deref()
        .and_then(|id| crate::vaults::notes_vault_by_id(id).ok().flatten())
        .or_else(|| {
            crate::vaults::notes_vault_by_name(&config.notes.workspace)
                .ok()
                .flatten()
        })
        .or_else(|| {
            (config.notes.workspace == DEFAULT_NOTES_WORKSPACE)
                .then(|| crate::vaults::default_notes_vault().ok())
                .flatten()
        });
    if let Some(vault) = resolved {
        config.notes.workspace = vault.name;
        config.notes.vault_id = Some(vault.id);
    }
}

fn write_workspace_config(workspace: &NeoismWorkspace) -> std::io::Result<()> {
    let source =
        serde_json::to_string_pretty(&workspace.config).map_err(std::io::Error::other)?;
    fs::write(workspace_config_path(&workspace.root), source)
}

pub fn save_workspace(workspace: &NeoismWorkspace) -> std::io::Result<()> {
    write_workspace_config(workspace)
}

pub fn ensure_notes_workspace(workspace: &NeoismWorkspace) -> std::io::Result<()> {
    ensure_note_root_dirs(workspace)?;
    ensure_welcome_docs(workspace)?;
    Ok(())
}

pub fn link_workspace_to_vault_project(
    workspace: &mut NeoismWorkspace,
    code_root: impl AsRef<Path>,
) -> std::io::Result<PathBuf> {
    link_code_dir_to_workspace_vault(workspace, code_root)
}

/// Read a vault's `project.json`, upgrading a pre-JSON `project.toml`
/// one time if that is all that is present.
pub(crate) fn read_vault_project_metadata(
    vault_dir: &Path,
) -> Option<VaultProjectMetadata> {
    let json_path = vault_dir.join(PROJECT_METADATA_FILE);
    if let Ok(source) = fs::read_to_string(&json_path) {
        return serde_json::from_str::<VaultProjectMetadata>(&source).ok();
    }
    let legacy = vault_dir.join(PROJECT_METADATA_FILE_LEGACY);
    let source = fs::read_to_string(&legacy).ok()?;
    toml::from_str::<VaultProjectMetadata>(&source).ok()
}

fn write_vault_project_metadata(
    vault_dir: &Path,
    metadata: &VaultProjectMetadata,
) -> std::io::Result<()> {
    fs::create_dir_all(vault_dir)?;
    let path = vault_dir.join(PROJECT_METADATA_FILE);
    let temp = path.with_extension("json.tmp");
    let source = serde_json::to_vec_pretty(metadata).map_err(std::io::Error::other)?;
    fs::write(&temp, source)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temp, path)
}

fn validate_notes_scope(
    vault_dir: &Path,
    scope: &Path,
    create: bool,
) -> std::io::Result<PathBuf> {
    let scope = normalized_notes_scope(scope);
    if scope.is_absolute()
        || scope.components().any(|component| {
            matches!(
                component,
                std::path::Component::ParentDir
                    | std::path::Component::RootDir
                    | std::path::Component::Prefix(_)
            )
        })
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "notes scope must be a relative folder inside its vault",
        ));
    }
    let candidate = if scope == Path::new(".") {
        vault_dir.to_path_buf()
    } else {
        vault_dir.join(&scope)
    };
    if create {
        fs::create_dir_all(&candidate)?;
    } else if !candidate.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("linked notes folder is missing: {}", candidate.display()),
        ));
    }
    let canonical_vault = normalize_root(vault_dir)?;
    let canonical_candidate = normalize_root(&candidate)?;
    if canonical_candidate != canonical_vault
        && !canonical_candidate.starts_with(&canonical_vault)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "notes scope escapes its vault",
        ));
    }
    Ok(scope)
}

fn notes_scope_for_dir(vault_dir: &Path, notes_dir: &Path) -> std::io::Result<PathBuf> {
    let vault = normalize_root(vault_dir)?;
    let notes = normalize_root(notes_dir)?;
    let relative = notes.strip_prefix(&vault).map_err(|_| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("{} is outside vault {}", notes.display(), vault.display()),
        )
    })?;
    Ok(if relative.as_os_str().is_empty() {
        default_notes_scope()
    } else {
        relative.to_path_buf()
    })
}

pub fn link_code_dir_to_workspace_vault(
    workspace: &mut NeoismWorkspace,
    code_root: impl AsRef<Path>,
) -> std::io::Result<PathBuf> {
    workspace.config.notes.enabled = true;
    let code_root = normalize_root(code_root.as_ref())?;
    let project_name = code_root
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| workspace.config.name.clone());
    let vault_dir = workspace.notes_vault_dir();
    fs::create_dir_all(&vault_dir)?;
    let vault_name = vault_dir
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&workspace.config.notes.workspace)
        .to_string();
    let vault = crate::vaults::ensure_notes_vault(&vault_name, &vault_dir)?;
    let notes_path =
        validate_notes_scope(&vault_dir, &workspace.config.notes.scope, true)?;
    workspace.config.notes.workspace = vault.name.clone();
    workspace.config.notes.vault_id = Some(vault.id.clone());
    workspace.config.notes.scope = notes_path.clone();

    let label = code_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&workspace.config.name)
        .to_string();
    let mut metadata =
        read_vault_project_metadata(&vault_dir).unwrap_or_else(|| VaultProjectMetadata {
            version: 2,
            name: project_name.clone(),
            links: Vec::new(),
        });
    metadata.version = 2;
    metadata.name = project_name;
    if let Some(linked) = metadata
        .links
        .iter_mut()
        .find(|linked| comparable_code_path(&linked.path) == code_root)
    {
        linked.kind = "dir".to_string();
        linked.label = label;
        linked.notes_path = notes_path;
    } else {
        metadata.links.push(ProjectLink {
            kind: "dir".to_string(),
            path: code_root.clone(),
            label,
            notes_path,
        });
    }
    write_vault_project_metadata(&vault_dir, &metadata)?;
    remove_exact_link_from_other_vaults(&code_root, &vault.id)?;
    save_workspace(workspace)?;
    Ok(workspace.notes_workspace_dir())
}

/// Link a code directory to an arbitrary folder inside a registered vault.
/// The folder becomes the project's default Alt+N / Agent notes scope while
/// the owning vault remains the graph and wiki-link boundary.
pub fn link_code_dir_to_notes_scope(
    workspace: &mut NeoismWorkspace,
    code_root: impl AsRef<Path>,
    notes_dir: impl AsRef<Path>,
) -> std::io::Result<PathBuf> {
    let notes_dir = normalize_root(notes_dir.as_ref())?;
    let vault = crate::vaults::notes_vault_for_path(&notes_dir)?.ok_or_else(|| {
        std::io::Error::other("notes folder is not inside a registered vault")
    })?;
    let scope = notes_scope_for_dir(&vault.path, &notes_dir)?;
    workspace.config.notes.workspace = vault.name;
    workspace.config.notes.vault_id = Some(vault.id);
    workspace.config.notes.scope = scope;
    link_code_dir_to_workspace_vault(workspace, code_root)
}

/// Project links recorded in a vault's `project.json` — the code dirs the
/// vault's "Page Link" (`[[@`) completion should search.
pub fn vault_project_links(vault_dir: impl AsRef<Path>) -> Vec<ProjectLink> {
    read_vault_project_metadata(vault_dir.as_ref())
        .map(|metadata| metadata.links)
        .unwrap_or_default()
}

/// Project links whose notes scope owns `note_path`. Nested scopes outrank a
/// whole-vault link, while multiple code roots attached to the same scope are
/// all retained for `[[@` completion.
pub fn vault_project_links_for_note(
    vault_dir: impl AsRef<Path>,
    note_path: impl AsRef<Path>,
) -> Vec<ProjectLink> {
    let vault_dir = vault_dir.as_ref();
    let note_path = note_path.as_ref();
    let mut matches = vault_project_links(vault_dir)
        .into_iter()
        .filter_map(|link| {
            let scope = normalized_notes_scope(&link.notes_path);
            let scope_root = if scope == Path::new(".") {
                vault_dir.to_path_buf()
            } else {
                vault_dir.join(&scope)
            };
            (note_path == scope_root || note_path.starts_with(&scope_root))
                .then_some((scope.components().count(), link))
        })
        .collect::<Vec<_>>();
    let specificity = matches.iter().map(|(depth, _)| *depth).max();
    matches
        .drain(..)
        .filter(|(depth, _)| Some(*depth) == specificity)
        .map(|(_, link)| link)
        .collect()
}

fn comparable_code_path(path: &Path) -> PathBuf {
    path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
}

fn remove_exact_link_from_other_vaults(
    code_root: &Path,
    target_vault_id: &str,
) -> std::io::Result<()> {
    for vault in crate::vaults::notes_vaults()? {
        if vault.id == target_vault_id || !vault.path.is_dir() {
            continue;
        }
        let Some(mut metadata) = read_vault_project_metadata(&vault.path) else {
            continue;
        };
        let before = metadata.links.len();
        metadata
            .links
            .retain(|link| comparable_code_path(&link.path) != code_root);
        if metadata.links.len() != before {
            metadata.version = 2;
            write_vault_project_metadata(&vault.path, &metadata)?;
        }
    }
    Ok(())
}

pub fn linked_notes_scope_for_code_dir(
    code_root: impl AsRef<Path>,
) -> std::io::Result<Option<ResolvedNotesScope>> {
    let code_root = normalize_root(code_root.as_ref())?;
    let mut best: Option<(usize, ResolvedNotesScope)> = None;
    for vault in crate::vaults::notes_vaults()? {
        if !vault.path.is_dir() {
            continue;
        }
        let Some(metadata) = read_vault_project_metadata(&vault.path) else {
            continue;
        };
        for link in metadata.links {
            let linked_path = comparable_code_path(&link.path);
            if linked_path != code_root && !code_root.starts_with(&linked_path) {
                continue;
            }
            let Ok(relative_path) =
                validate_notes_scope(&vault.path, &link.notes_path, false)
            else {
                continue;
            };
            let mut config = WorkspaceConfig::new(&code_root);
            config.notes.workspace = vault.name.clone();
            config.notes.vault_id = Some(vault.id.clone());
            config.notes.scope = relative_path.clone();
            let specificity = linked_path.components().count();
            let resolved = ResolvedNotesScope {
                workspace: NeoismWorkspace {
                    root: code_root.clone(),
                    config,
                },
                vault: vault.clone(),
                project_root: linked_path,
                relative_path,
            };
            if best
                .as_ref()
                .is_none_or(|(current_specificity, _)| specificity > *current_specificity)
            {
                best = Some((specificity, resolved));
            }
        }
    }
    Ok(best.map(|(_, resolved)| resolved))
}

pub fn linked_project_for_code_dir(
    code_root: impl AsRef<Path>,
) -> std::io::Result<Option<NeoismWorkspace>> {
    Ok(linked_notes_scope_for_code_dir(code_root)?.map(|scope| scope.workspace))
}

/// Convert a legacy standalone project vault into a folder inside another
/// vault. All code links are rewritten to the new nested scope before the old
/// vault registration is removed. Callers should confirm this user-visible
/// move before invoking it.
pub fn convert_vault_to_nested_scope(
    source_vault: impl AsRef<Path>,
    destination_dir: impl AsRef<Path>,
) -> std::io::Result<ConvertedVaultScope> {
    let source_vault = normalize_root(source_vault.as_ref())?;
    let destination_dir = normalize_root(destination_dir.as_ref())?;
    let vaults_root = normalize_root(&notes_vaults_dir())?;
    if source_vault.parent() != Some(vaults_root.as_path()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "only a top-level standalone vault can be converted",
        ));
    }
    let source = crate::vaults::notes_vault_for_path(&source_vault)?
        .filter(|vault| comparable_code_path(&vault.path) == source_vault)
        .ok_or_else(|| std::io::Error::other("source vault is not registered"))?;
    let destination = crate::vaults::notes_vault_for_path(&destination_dir)?
        .ok_or_else(|| std::io::Error::other("destination is not inside a vault"))?;
    if source.id == destination.id {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "source and destination belong to the same vault",
        ));
    }
    let folder_name = source_vault
        .file_name()
        .ok_or_else(|| std::io::Error::other("source vault has no folder name"))?;
    let target = destination_dir.join(folder_name);
    if target.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("destination already exists: {}", target.display()),
        ));
    }
    let target_scope = notes_scope_for_dir(&destination.path, &target)?;
    let source_metadata =
        read_vault_project_metadata(&source.path).unwrap_or(VaultProjectMetadata {
            version: 2,
            name: source.name.clone(),
            links: Vec::new(),
        });
    let source_original = source_metadata.clone();
    let destination_original = read_vault_project_metadata(&destination.path);
    let mut destination_metadata =
        destination_original
            .clone()
            .unwrap_or(VaultProjectMetadata {
                version: 2,
                name: destination.name.clone(),
                links: Vec::new(),
            });
    destination_metadata.version = 2;
    let mut moved_links = Vec::with_capacity(source_metadata.links.len());
    for mut link in source_metadata.links {
        let old_scope = normalized_notes_scope(&link.notes_path);
        link.notes_path = if old_scope == Path::new(".") {
            target_scope.clone()
        } else {
            target_scope.join(old_scope)
        };
        if let Some(existing) = destination_metadata.links.iter_mut().find(|existing| {
            comparable_code_path(&existing.path) == comparable_code_path(&link.path)
        }) {
            *existing = link.clone();
        } else {
            destination_metadata.links.push(link.clone());
        }
        moved_links.push(link);
    }

    fs::rename(&source_vault, &target)?;
    if let Err(error) =
        write_vault_project_metadata(&destination.path, &destination_metadata)
    {
        let _ = fs::rename(&target, &source_vault);
        return Err(error);
    }
    // The destination vault root now owns these links. Leaving a second
    // metadata file inside the nested folder would be misleading and could be
    // re-imported if the folder were later moved back to the Vaults root.
    let _ = fs::remove_file(target.join(PROJECT_METADATA_FILE));
    let _ = fs::remove_file(target.join(PROJECT_METADATA_FILE_LEGACY));
    if let Err(error) =
        crate::vaults::remove_notes_vault_registration(&source.id, &destination.id)
    {
        restore_vault_project_metadata(&destination.path, destination_original.as_ref());
        let _ = fs::rename(&target, &source_vault);
        let _ = write_vault_project_metadata(&source_vault, &source_original);
        return Err(error);
    }

    for link in &moved_links {
        let _ = remove_exact_link_from_other_vaults(
            &comparable_code_path(&link.path),
            &destination.id,
        );
        let Ok(Some(mut workspace)) = load_workspace(&link.path) else {
            continue;
        };
        workspace.config.notes.enabled = true;
        workspace.config.notes.workspace = destination.name.clone();
        workspace.config.notes.vault_id = Some(destination.id.clone());
        workspace.config.notes.scope = link.notes_path.clone();
        let _ = save_workspace(&workspace);
    }

    Ok(ConvertedVaultScope {
        source_vault_name: source.name,
        destination_vault: destination,
        scope_dir: target,
        links_moved: moved_links.len(),
    })
}

/// Move a note/folder on disk and rewrite every project scope contained by
/// that path. This keeps nested links stable when users reorganize folders,
/// including moves between two registered vaults.
pub fn move_notes_path_preserving_scopes(
    source: impl AsRef<Path>,
    target: impl AsRef<Path>,
) -> std::io::Result<usize> {
    let source = normalize_root(source.as_ref())?;
    let target = normalize_root(target.as_ref())?;
    let source_vault = crate::vaults::notes_vault_for_path(&source)?;
    let destination_vault = crate::vaults::notes_vault_for_path(&target)?;

    if source_vault
        .as_ref()
        .is_some_and(|vault| comparable_code_path(&vault.path) == source)
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "use Rename Vault or the vault-conversion drop action for a standalone vault",
        ));
    }

    let mut affected = Vec::new();
    let source_original = source_vault
        .as_ref()
        .and_then(|vault| read_vault_project_metadata(&vault.path));
    if let (Some(vault), Some(metadata)) = (&source_vault, &source_original) {
        for link in &metadata.links {
            let scope = normalized_notes_scope(&link.notes_path);
            if scope == Path::new(".") {
                continue;
            }
            let scope_root = vault.path.join(&scope);
            if scope_root == source || scope_root.starts_with(&source) {
                affected.push(link.clone());
            }
        }
    }
    if !affected.is_empty() && destination_vault.is_none() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "a folder containing linked project scopes must stay inside a vault",
        ));
    }

    if let Some(parent) = target.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::rename(&source, &target)?;
    if affected.is_empty() {
        return Ok(0);
    }

    let source_vault = source_vault.expect("affected scopes require a source vault");
    let destination_vault = destination_vault.expect("checked above");
    let destination_original = read_vault_project_metadata(&destination_vault.path);
    let source_prefix = notes_scope_for_dir(&source_vault.path, &source)?;
    let destination_prefix = notes_scope_for_dir(&destination_vault.path, &target)?;

    let commit = (|| -> std::io::Result<Vec<ProjectLink>> {
        if source_vault.id == destination_vault.id {
            let mut metadata = source_original.clone().unwrap_or(VaultProjectMetadata {
                version: 2,
                name: source_vault.name.clone(),
                links: Vec::new(),
            });
            let mut rewritten = Vec::new();
            for link in &mut metadata.links {
                if affected.iter().any(|affected| {
                    comparable_code_path(&affected.path)
                        == comparable_code_path(&link.path)
                }) {
                    let suffix = normalized_notes_scope(&link.notes_path)
                        .strip_prefix(&source_prefix)
                        .unwrap_or(Path::new(""))
                        .to_path_buf();
                    link.notes_path = destination_prefix.join(suffix);
                    rewritten.push(link.clone());
                }
            }
            metadata.version = 2;
            write_vault_project_metadata(&source_vault.path, &metadata)?;
            return Ok(rewritten);
        }

        let mut from = source_original.clone().unwrap_or(VaultProjectMetadata {
            version: 2,
            name: source_vault.name.clone(),
            links: Vec::new(),
        });
        let mut into = destination_original
            .clone()
            .unwrap_or(VaultProjectMetadata {
                version: 2,
                name: destination_vault.name.clone(),
                links: Vec::new(),
            });
        let mut rewritten = Vec::new();
        from.links.retain(|link| {
            if !affected.iter().any(|affected| {
                comparable_code_path(&affected.path) == comparable_code_path(&link.path)
            }) {
                return true;
            }
            let mut moved = link.clone();
            let suffix = normalized_notes_scope(&moved.notes_path)
                .strip_prefix(&source_prefix)
                .unwrap_or(Path::new(""))
                .to_path_buf();
            moved.notes_path = destination_prefix.join(suffix);
            if let Some(existing) = into.links.iter_mut().find(|existing| {
                comparable_code_path(&existing.path) == comparable_code_path(&moved.path)
            }) {
                *existing = moved.clone();
            } else {
                into.links.push(moved.clone());
            }
            rewritten.push(moved);
            false
        });
        from.version = 2;
        into.version = 2;
        write_vault_project_metadata(&destination_vault.path, &into)?;
        if let Err(error) = write_vault_project_metadata(&source_vault.path, &from) {
            restore_vault_project_metadata(
                &destination_vault.path,
                destination_original.as_ref(),
            );
            return Err(error);
        }
        Ok(rewritten)
    })();

    let rewritten = match commit {
        Ok(rewritten) => rewritten,
        Err(error) => {
            restore_vault_project_metadata(&source_vault.path, source_original.as_ref());
            let _ = fs::rename(&target, &source);
            return Err(error);
        }
    };
    for link in &rewritten {
        let Ok(Some(mut workspace)) = load_workspace(&link.path) else {
            continue;
        };
        workspace.config.notes.workspace = destination_vault.name.clone();
        workspace.config.notes.vault_id = Some(destination_vault.id.clone());
        workspace.config.notes.scope = link.notes_path.clone();
        let _ = save_workspace(&workspace);
    }
    Ok(rewritten.len())
}

fn restore_vault_project_metadata(
    vault_dir: &Path,
    metadata: Option<&VaultProjectMetadata>,
) {
    if let Some(metadata) = metadata {
        let _ = write_vault_project_metadata(vault_dir, metadata);
    } else {
        let _ = fs::remove_file(vault_dir.join(PROJECT_METADATA_FILE));
    }
}

fn ensure_note_root_dirs(workspace: &NeoismWorkspace) -> std::io::Result<()> {
    if !workspace.config.notes.enabled {
        return Ok(());
    }
    fs::create_dir_all(workspace.notes_workspace_dir())?;
    Ok(())
}

/// Bundled Zed-style "Welcome" getting-started docs, seeded into the DEFAULT
/// vault so users have a built-in guide (the start-screen "Notes" button and
/// Alt+N open onto this folder). Pages are real Markdown files under
/// `src/welcome/`.
/// Seed the `Welcome/` getting-started folder into the vault once. A marker
/// records that the initial bundle was installed so later user edits and
/// deletions remain authoritative.
fn ensure_welcome_docs(workspace: &NeoismWorkspace) -> std::io::Result<()> {
    if !workspace.config.notes.enabled {
        return Ok(());
    }
    // The bundled getting-started docs always live in the DEFAULT vault,
    // not per-project/linked vaults, so there is one canonical home for
    // them regardless of which workspace is open.
    seed_welcome_docs(&default_notes_workspace().notes_vault_dir())
}

fn seed_welcome_docs(default_vault: &Path) -> std::io::Result<()> {
    let marker = default_vault.join(WELCOME_SEEDED_MARKER);
    if marker.is_file() {
        return Ok(());
    }

    let welcome = default_vault.join(WELCOME_DIR);
    // v4 replaces the old flat handbook. Remove only paths Neoism shipped;
    // unrelated notes in Welcome remain user-owned.
    for replaced in REPLACED_WELCOME_PATHS {
        let path = welcome.join(replaced);
        if path.is_dir() {
            let _ = fs::remove_dir_all(path);
        } else {
            let _ = fs::remove_file(path);
        }
    }
    // A marker bump is a one-time managed-doc migration: replaced paths are
    // removed, every current bundled page is installed/refreshed, and unrelated
    // user notes remain untouched. Once the v4 marker exists this function
    // returns early, so later user edits and deletions are honored.
    for doc in crate::docs::BUNDLED_DOCS {
        let page = welcome.join(doc.path);
        if let Some(parent) = page.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(page, doc.body)?;
    }

    fs::create_dir_all(default_vault)?;
    // Drop stale `.neoism-welcome-seeded-v*` markers from earlier versions
    // so the vault root isn't littered with one per release.
    if let Ok(entries) = default_vault.read_dir() {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.starts_with(".neoism-welcome-seeded-")
                && name != WELCOME_SEEDED_MARKER
            {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
    fs::write(marker, b"seeded\n")?;
    Ok(())
}

pub fn notes_workspace_dir(name: &str) -> PathBuf {
    let name = name.trim();
    if name.is_empty() {
        return crate::vaults::default_notes_vault()
            .map(|vault| vault.path)
            .unwrap_or_else(|_| notes_vaults_dir().join(DEFAULT_NOTES_WORKSPACE));
    }
    let name_path = Path::new(name);
    if name_path.is_absolute() {
        return name_path.to_path_buf();
    }
    if let Ok(Some(vault)) = crate::vaults::notes_vault_by_name(name) {
        return vault.path;
    }
    let base = notes_vaults_dir();
    base.join(name)
}

pub fn notes_vaults_dir() -> PathBuf {
    std::env::var_os("NEOISM_NOTES_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(DEFAULT_NOTES_VAULTS_DIR))
        })
        .or_else(|| dirs::home_dir().map(|home| home.join(DEFAULT_NOTES_VAULTS_DIR)))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_NOTES_VAULTS_DIR))
}

pub fn global_cache_dir() -> PathBuf {
    std::env::var_os("NEOISM_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("XDG_CACHE_HOME")
                .map(PathBuf::from)
                .map(|cache| cache.join("neoism"))
        })
        .or_else(|| {
            std::env::var_os("HOME")
                .map(PathBuf::from)
                .map(|home| home.join(".cache").join("neoism"))
        })
        .or_else(|| dirs::cache_dir().map(|cache| cache.join("neoism")))
        .unwrap_or_else(|| PathBuf::from(".neoism-cache"))
}

fn path_components(path: &Path) -> String {
    path.components()
        .filter_map(|component| match component {
            std::path::Component::Normal(part) => {
                Some(part.to_string_lossy().into_owned())
            }
            std::path::Component::ParentDir => Some("..".to_string()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("/")
}

fn cache_key(value: &str) -> String {
    let mut out = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
            out.push(ch);
        } else if ch == '/' || ch == std::path::MAIN_SEPARATOR {
            out.push_str("__");
        } else if ch.is_whitespace() {
            out.push('-');
        }
    }
    let out = out.trim_matches(['-', '.', '_']).to_string();
    if out.is_empty() {
        "default".to_string()
    } else {
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "neoism-workspace-config-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        root
    }

    fn temp_notes_home(name: &str) -> PathBuf {
        let root = std::env::temp_dir()
            .join(format!("neoism-notes-home-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        unsafe {
            std::env::set_var("NEOISM_NOTES_HOME", &root);
        }
        root
    }

    #[test]
    fn init_creates_marker_cache_and_stable_config() {
        let root = temp_root("init");
        let notes_home = temp_notes_home("init");
        let workspace = init_workspace(&root).unwrap();

        assert!(workspace_config_path(&root).is_file());
        assert_eq!(
            workspace.cache_dir(),
            global_cache_dir()
                .join("vaults")
                .join(DEFAULT_NOTES_WORKSPACE_ID)
        );
        assert!(!workspace
            .notes_workspace_dir()
            .join(PROJECT_METADATA_FILE)
            .exists());
        assert!(!root.join(NEOISM_DIR).join("cache").exists());
        assert_eq!(workspace.config.version, CURRENT_WORKSPACE_CONFIG_VERSION);

        let reloaded = load_workspace(&root).unwrap().unwrap();
        assert_eq!(reloaded.config.id, workspace.config.id);

        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn link_workspace_creates_project_metadata_in_vault() {
        let root = temp_root("link-workspace-root");
        let notes_home = std::env::temp_dir().join(format!(
            "neoism-notes-home-link-workspace-root-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&notes_home);
        fs::create_dir_all(&root).unwrap();
        fs::create_dir_all(root.join(NEOISM_DIR)).unwrap();
        let mut workspace = NeoismWorkspace {
            root: root.clone(),
            config: WorkspaceConfig::new(&root),
        };
        workspace.config.notes.workspace = notes_home.display().to_string();
        workspace.config.notes.vault_id = None;

        let project_dir = link_workspace_to_vault_project(&mut workspace, &root).unwrap();

        let project_name = root.file_name().unwrap().to_string_lossy().into_owned();
        assert_eq!(project_dir, workspace.notes_workspace_dir());
        let metadata = fs::read_to_string(
            workspace.notes_workspace_dir().join(PROJECT_METADATA_FILE),
        )
        .unwrap();
        assert!(metadata.contains(&format!("\"name\": \"{project_name}\"")));
        assert!(metadata.contains("\"kind\": \"dir\""));
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn welcome_seed_respects_deleted_pages() {
        let root = temp_root("welcome-delete");
        seed_welcome_docs(&root).unwrap();
        let getting_started = root.join(WELCOME_DIR).join(DEFAULT_NOTES_INDEX);
        assert!(getting_started.is_file());

        fs::remove_file(&getting_started).unwrap();
        seed_welcome_docs(&root).unwrap();
        assert!(!getting_started.exists());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn welcome_seed_refreshes_shipped_pages_on_version_bump() {
        let root = temp_root("welcome-refresh");
        let welcome = root.join(WELCOME_DIR);
        fs::create_dir_all(&welcome).unwrap();
        // A previous handbook marker exists: one current managed page holds
        // stale content, an old shipped path remains, and a user note exists.
        let shipped = welcome.join(DEFAULT_NOTES_INDEX);
        fs::create_dir_all(shipped.parent().unwrap()).unwrap();
        fs::write(&shipped, "STALE DOC").unwrap();
        let replaced = welcome.join("Getting Started.md");
        fs::write(&replaced, "REPLACED DOC").unwrap();
        fs::write(root.join(".neoism-welcome-seeded-v2"), b"seeded\n").unwrap();
        let user_note = welcome.join("My Note.md");
        fs::write(&user_note, "keep me").unwrap();

        seed_welcome_docs(&root).unwrap();

        // Managed page refreshed; replaced path and previous marker removed;
        // the current marker written; the user's own note left untouched.
        let refreshed = fs::read_to_string(&shipped).unwrap();
        assert!(refreshed.contains("# Welcome to Neoism"));
        assert!(!replaced.exists());
        assert!(!root.join(".neoism-welcome-seeded-v2").exists());
        assert!(root.join(WELCOME_SEEDED_MARKER).is_file());
        assert_eq!(fs::read_to_string(&user_note).unwrap(), "keep me");

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn linked_vault_resolves_to_one_scoped_vault_dir() {
        // Mirrors how the daemon advertises `linked_vault_dir` to guests:
        // resolve the code dir's linked project, then take its
        // `notes_workspace_dir()`. It must be exactly the ONE vault dir
        // under `~/Neoism/Vaults`, never the `Vaults` parent (which would
        // leak every vault to a joined guest).
        let notes_home = temp_notes_home("linked-vault-scope");
        let code_root = temp_root("linked-vault-scope-code");
        fs::create_dir_all(&code_root).unwrap();
        fs::create_dir_all(code_root.join(NEOISM_DIR)).unwrap();

        let mut workspace = NeoismWorkspace {
            root: code_root.clone(),
            config: WorkspaceConfig::new(&code_root),
        };
        workspace.config.notes.workspace = "ProjectVault".to_string();
        workspace.config.notes.vault_id = None;
        let vault_dir =
            link_code_dir_to_workspace_vault(&mut workspace, &code_root).unwrap();

        let resolved = linked_project_for_code_dir(&code_root).unwrap().unwrap();
        let advertised = resolved.notes_workspace_dir();

        assert_eq!(advertised, vault_dir);
        assert_eq!(advertised, notes_vaults_dir().join("ProjectVault"));
        // Scoped: the advertised path is a single vault, and its parent is
        // the Vaults root — the guest can never walk up to sibling vaults.
        assert_eq!(advertised.parent().unwrap(), notes_vaults_dir());
        assert_ne!(advertised, notes_vaults_dir());

        let _ = fs::remove_dir_all(code_root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn default_notes_workspace_has_stable_global_identity() {
        let first = default_notes_workspace();
        let second = default_notes_workspace();
        assert_eq!(first.config.id, DEFAULT_NOTES_WORKSPACE_ID);
        assert_eq!(first.config.id, second.config.id);
        assert_eq!(first.config.notes.workspace, DEFAULT_NOTES_WORKSPACE);
        assert_eq!(first.notes_workspace_dir(), second.notes_workspace_dir());
    }

    #[test]
    fn legacy_project_links_migrate_to_the_whole_vault_scope() {
        let metadata: VaultProjectMetadata = serde_json::from_str(
            r#"{
                "version": 1,
                "name": "Legacy",
                "links": [{
                    "kind": "dir",
                    "path": "/tmp/legacy-project",
                    "label": "legacy-project"
                }]
            }"#,
        )
        .unwrap();
        assert_eq!(metadata.links[0].notes_path, PathBuf::from("."));
    }

    #[test]
    fn nested_project_scope_resolves_for_project_children_but_indexes_whole_vault() {
        let notes_home = temp_notes_home("nested-project-scope");
        let vault = vault_notes_workspace("Personal");
        ensure_notes_workspace(&vault).unwrap();
        let scope = vault.notes_vault_dir().join("Projects/Thing");
        fs::create_dir_all(&scope).unwrap();
        fs::write(scope.join("Scoped.md"), "# Scoped\n").unwrap();
        fs::write(vault.notes_vault_dir().join("Global.md"), "# Global\n").unwrap();

        let code_root = temp_root("nested-project-code");
        fs::create_dir_all(code_root.join("src")).unwrap();
        let mut workspace = init_workspace(&code_root).unwrap();
        let linked =
            link_code_dir_to_notes_scope(&mut workspace, &code_root, &scope).unwrap();
        assert_eq!(linked, scope);

        let resolved = linked_notes_scope_for_code_dir(code_root.join("src"))
            .unwrap()
            .unwrap();
        assert_eq!(resolved.project_root, code_root);
        assert_eq!(resolved.relative_path, PathBuf::from("Projects/Thing"));
        assert_eq!(resolved.workspace.notes_workspace_dir(), scope);
        assert_eq!(
            resolved.workspace.notes_vault_dir(),
            vault.notes_vault_dir()
        );

        let graph = crate::query::NoteGraph::open(&resolved.project_root).unwrap();
        assert_eq!(
            graph.workspace().notes_workspace_dir(),
            vault.notes_vault_dir()
        );

        let metadata = read_vault_project_metadata(&vault.notes_vault_dir()).unwrap();
        assert_eq!(metadata.version, 2);
        assert_eq!(
            metadata.links[0].notes_path,
            PathBuf::from("Projects/Thing")
        );

        let moved_scope = vault.notes_vault_dir().join("Archive/Thing");
        fs::create_dir_all(moved_scope.parent().unwrap()).unwrap();
        assert_eq!(
            move_notes_path_preserving_scopes(&scope, &moved_scope).unwrap(),
            1
        );
        let moved = linked_notes_scope_for_code_dir(code_root.join("src"))
            .unwrap()
            .unwrap();
        assert_eq!(moved.relative_path, PathBuf::from("Archive/Thing"));
        assert_eq!(moved.workspace.notes_workspace_dir(), moved_scope);

        let _ = fs::remove_dir_all(code_root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn linked_scope_can_move_between_vaults_without_losing_its_project() {
        let notes_home = temp_notes_home("move-scope-between-vaults");
        let source_vault = vault_notes_workspace("Work");
        let destination_vault = vault_notes_workspace("Archive");
        ensure_notes_workspace(&source_vault).unwrap();
        ensure_notes_workspace(&destination_vault).unwrap();

        let source_scope = source_vault.notes_vault_dir().join("Projects/Thing");
        fs::create_dir_all(&source_scope).unwrap();
        fs::write(source_scope.join("Plan.md"), "# Plan\n").unwrap();
        let code_root = temp_root("move-scope-between-vaults-code");
        fs::create_dir_all(&code_root).unwrap();
        let mut workspace = init_workspace(&code_root).unwrap();
        link_code_dir_to_notes_scope(&mut workspace, &code_root, &source_scope).unwrap();

        let destination_scope = destination_vault
            .notes_vault_dir()
            .join("Archived Projects/Thing");
        fs::create_dir_all(destination_scope.parent().unwrap()).unwrap();
        assert_eq!(
            move_notes_path_preserving_scopes(&source_scope, &destination_scope).unwrap(),
            1
        );
        assert!(destination_scope.join("Plan.md").is_file());
        assert!(!source_scope.exists());

        let resolved = linked_notes_scope_for_code_dir(&code_root)
            .unwrap()
            .unwrap();
        assert_eq!(resolved.vault.id, destination_vault.config.id);
        assert_eq!(resolved.workspace.notes_workspace_dir(), destination_scope);
        let reloaded = load_workspace(&code_root).unwrap().unwrap();
        assert_eq!(
            reloaded.config.notes.vault_id.as_deref(),
            Some(destination_vault.config.id.as_str())
        );

        let _ = fs::remove_dir_all(code_root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn renaming_default_vault_preserves_identity_and_fallback() {
        let notes_home = temp_notes_home("rename-default-vault");
        let original = default_notes_workspace();
        ensure_notes_workspace(&original).unwrap();
        let original_id = original.config.notes.vault_id.clone().unwrap();
        fs::write(original.notes_vault_dir().join("Keep.md"), "keep").unwrap();

        let renamed = crate::vaults::rename_notes_vault(
            original.notes_vault_dir(),
            "Personal Notes",
        )
        .unwrap();
        assert_eq!(renamed.id, original_id);
        assert!(renamed.path.join("Keep.md").is_file());
        // Already-open workspaces may still carry the old display name for a
        // moment; their stable vault identity must resolve the renamed path.
        assert_eq!(original.notes_vault_dir(), renamed.path);

        let fallback = default_notes_workspace();
        assert_eq!(fallback.config.notes.workspace, "Personal Notes");
        assert_eq!(
            fallback.config.notes.vault_id.as_deref(),
            Some(original_id.as_str())
        );
        assert_eq!(fallback.notes_vault_dir(), renamed.path);

        let code_root = temp_root("rename-default-code");
        let workspace = init_workspace(&code_root).unwrap();
        assert_eq!(workspace.config.notes.workspace, "Personal Notes");
        assert!(!notes_home.join(DEFAULT_NOTES_WORKSPACE).exists());

        let _ = fs::remove_dir_all(code_root);
        let _ = fs::remove_dir_all(notes_home);
    }

    #[test]
    fn standalone_vault_conversion_moves_notes_and_preserves_project_link() {
        let notes_home = temp_notes_home("convert-vault");
        let source = vault_notes_workspace("Thing");
        ensure_notes_workspace(&source).unwrap();
        fs::write(source.notes_vault_dir().join("Plan.md"), "# Plan\n").unwrap();
        let code_root = temp_root("convert-vault-code");
        fs::create_dir_all(&code_root).unwrap();
        let mut code_workspace = init_workspace(&code_root).unwrap();
        code_workspace.config.notes.workspace = "Thing".to_string();
        code_workspace.config.notes.vault_id = source.config.notes.vault_id.clone();
        link_workspace_to_vault_project(&mut code_workspace, &code_root).unwrap();

        let destination = vault_notes_workspace("Personal");
        ensure_notes_workspace(&destination).unwrap();
        let projects = destination.notes_vault_dir().join("Projects");
        fs::create_dir_all(&projects).unwrap();
        let converted =
            convert_vault_to_nested_scope(source.notes_vault_dir(), &projects).unwrap();

        assert!(!source.notes_vault_dir().exists());
        assert!(converted.scope_dir.join("Plan.md").is_file());
        assert!(!converted.scope_dir.join(PROJECT_METADATA_FILE).exists());
        let resolved = linked_notes_scope_for_code_dir(&code_root)
            .unwrap()
            .unwrap();
        assert_eq!(
            resolved.workspace.notes_workspace_dir(),
            converted.scope_dir
        );
        assert_eq!(resolved.vault.name, "Personal");
        assert!(crate::vaults::notes_vault_by_name("Thing")
            .unwrap()
            .is_none());

        let _ = fs::remove_dir_all(code_root);
        let _ = fs::remove_dir_all(notes_home);
    }
}
