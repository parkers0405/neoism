use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

pub const NEOISM_DIR: &str = ".neoism";
pub const WORKSPACE_FILE: &str = "workspace.json";
/// Pre-JSON workspace file, read once to upgrade in place.
const WORKSPACE_FILE_LEGACY: &str = "workspace.toml";
pub const CURRENT_WORKSPACE_CONFIG_VERSION: u32 = 5;
pub const DEFAULT_NOTES_WORKSPACE: &str = "Default";
pub const DEFAULT_NOTES_VAULTS_DIR: &str = "Neoism/Vaults";
pub const DEFAULT_NOTES_INDEX: &str = "Getting Started.md";
pub const WELCOME_DIR: &str = "Welcome";
pub const PROJECT_METADATA_FILE: &str = "project.json";
/// Pre-JSON vault metadata file, read once to upgrade in place.
const PROJECT_METADATA_FILE_LEGACY: &str = "project.toml";
const DEFAULT_NOTES_WORKSPACE_ID: &str = "neoism-notes-default-v1";
const WELCOME_SEEDED_MARKER: &str = ".neoism-welcome-seeded-v3";

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
}

impl Default for NotesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            workspace: default_notes_workspace_name(),
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
        global_cache_dir()
            .join("vaults")
            .join(cache_key(&self.config.notes.workspace))
    }

    pub fn notes_workspace_dir(&self) -> PathBuf {
        notes_workspace_dir(&self.config.notes.workspace)
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
    NeoismWorkspace {
        root: notes_workspace_dir(DEFAULT_NOTES_WORKSPACE),
        config: WorkspaceConfig {
            version: CURRENT_WORKSPACE_CONFIG_VERSION,
            id: DEFAULT_NOTES_WORKSPACE_ID.to_string(),
            name: DEFAULT_NOTES_WORKSPACE.to_string(),
            notes: NotesConfig::default(),
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
    NeoismWorkspace {
        root: notes_workspace_dir(name),
        config: WorkspaceConfig {
            version: CURRENT_WORKSPACE_CONFIG_VERSION,
            id: format!("vault:{name}"),
            name: name.to_string(),
            notes: NotesConfig {
                workspace: name.to_string(),
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
    DEFAULT_NOTES_WORKSPACE.to_string()
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
fn read_vault_project_metadata(vault_dir: &Path) -> Option<VaultProjectMetadata> {
    let json_path = vault_dir.join(PROJECT_METADATA_FILE);
    if let Ok(source) = fs::read_to_string(&json_path) {
        return serde_json::from_str::<VaultProjectMetadata>(&source).ok();
    }
    let legacy = vault_dir.join(PROJECT_METADATA_FILE_LEGACY);
    let source = fs::read_to_string(&legacy).ok()?;
    toml::from_str::<VaultProjectMetadata>(&source).ok()
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
    let vault_dir = workspace.notes_workspace_dir();
    fs::create_dir_all(&vault_dir)?;

    let metadata_path = vault_dir.join(PROJECT_METADATA_FILE);
    let label = code_root
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&workspace.config.name)
        .to_string();
    let mut metadata =
        read_vault_project_metadata(&vault_dir).unwrap_or_else(|| VaultProjectMetadata {
            version: 1,
            name: project_name.clone(),
            links: Vec::new(),
        });
    metadata.name = project_name;
    if !metadata.links.iter().any(|linked| {
        linked
            .path
            .canonicalize()
            .unwrap_or_else(|_| linked.path.clone())
            == code_root
    }) {
        metadata.links.push(ProjectLink {
            kind: "dir".to_string(),
            path: code_root,
            label,
        });
    }
    let source =
        serde_json::to_string_pretty(&metadata).map_err(std::io::Error::other)?;
    fs::write(&metadata_path, source)?;
    save_workspace(workspace)?;
    Ok(vault_dir)
}

/// Project links recorded in a vault's `project.json` — the code dirs the
/// vault's "Page Link" (`[[@`) completion should search.
pub fn vault_project_links(vault_dir: impl AsRef<Path>) -> Vec<ProjectLink> {
    read_vault_project_metadata(vault_dir.as_ref())
        .map(|metadata| metadata.links)
        .unwrap_or_default()
}

pub fn linked_project_for_code_dir(
    code_root: impl AsRef<Path>,
) -> std::io::Result<Option<NeoismWorkspace>> {
    let code_root = normalize_root(code_root.as_ref())?;
    let vaults_dir = notes_vaults_dir();
    let Ok(vaults) = fs::read_dir(vaults_dir) else {
        return Ok(None);
    };
    for vault in vaults.filter_map(Result::ok) {
        let vault_path = vault.path();
        if !vault_path.is_dir() {
            continue;
        }
        let Some(metadata) = read_vault_project_metadata(&vault_path) else {
            continue;
        };
        if metadata.links.iter().any(|link| {
            let linked_path = link
                .path
                .canonicalize()
                .unwrap_or_else(|_| link.path.clone());
            linked_path == code_root || code_root.starts_with(&linked_path)
        }) {
            let vault_name = vault_path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(DEFAULT_NOTES_WORKSPACE)
                .to_string();
            let mut config = WorkspaceConfig::new(&code_root);
            config.notes.workspace = vault_name;
            return Ok(Some(NeoismWorkspace {
                root: code_root,
                config,
            }));
        }
    }
    Ok(None)
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
const WELCOME_PAGES: &[(&str, &str)] = &[
    (
        "Getting Started.md",
        include_str!("welcome/Getting Started.md"),
    ),
    ("The Terminal.md", include_str!("welcome/The Terminal.md")),
    (
        "The Neoism Agent.md",
        include_str!("welcome/The Neoism Agent.md"),
    ),
    (
        "Notes and Drawings.md",
        include_str!("welcome/Notes and Drawings.md"),
    ),
    ("Multiplayer.md", include_str!("welcome/Multiplayer.md")),
    ("Keybindings.md", include_str!("welcome/Keybindings.md")),
    (
        "Editor/The Editor.md",
        include_str!("welcome/Editor/The Editor.md"),
    ),
    (
        "Editor/Languages and LSP.md",
        include_str!("welcome/Editor/Languages and LSP.md"),
    ),
    (
        "Configuration/Configuration.md",
        include_str!("welcome/Configuration/Configuration.md"),
    ),
    (
        "Configuration/Themes, Cursor and Fonts.md",
        include_str!("welcome/Configuration/Themes, Cursor and Fonts.md"),
    ),
    (
        "Configuration/Shaders.md",
        include_str!("welcome/Configuration/Shaders.md"),
    ),
    (
        "Configuration/Mash Up Packs.md",
        include_str!("welcome/Configuration/Mash Up Packs.md"),
    ),
];

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
    seed_welcome_docs(&notes_workspace_dir(DEFAULT_NOTES_WORKSPACE))
}

fn seed_welcome_docs(default_vault: &Path) -> std::io::Result<()> {
    let marker = default_vault.join(WELCOME_SEEDED_MARKER);
    if marker.is_file() {
        return Ok(());
    }

    let welcome = default_vault.join(WELCOME_DIR);
    // A Welcome directory that already has content is an established vault:
    // it was seeded by an earlier marker version. Bumping the marker means
    // the shipped docs changed, so REFRESH each canonical page that is still
    // present — but never resurrect one the user deleted (deletions stay
    // deliberate), and never touch notes the user created themselves.
    // A fresh (empty) vault gets the full set seeded.
    let established = welcome
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false);
    for (name, body) in WELCOME_PAGES {
        let page = welcome.join(name);
        if let Some(parent) = page.parent() {
            fs::create_dir_all(parent)?;
        }
        if established {
            // Refresh canonical pages in place; honor deletions.
            if page.exists() {
                fs::write(page, body)?;
            }
        } else if !page.exists() {
            fs::write(page, body)?;
        }
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
    let name = if name.is_empty() {
        DEFAULT_NOTES_WORKSPACE
    } else {
        name
    };
    let name_path = Path::new(name);
    if name_path.is_absolute() {
        return name_path.to_path_buf();
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
                .join(DEFAULT_NOTES_WORKSPACE)
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
        // Established vault seeded by an OLDER marker version: a shipped page
        // holds stale content, and a user note the app never shipped.
        let shipped = welcome.join("Configuration").join("Configuration.md");
        fs::create_dir_all(shipped.parent().unwrap()).unwrap();
        fs::write(&shipped, "STALE OLD DOC").unwrap();
        fs::write(root.join(".neoism-welcome-seeded-v2"), b"seeded\n").unwrap();
        let user_note = welcome.join("My Note.md");
        fs::write(&user_note, "keep me").unwrap();

        seed_welcome_docs(&root).unwrap();

        // Shipped page refreshed to current content; stale marker cleaned up;
        // the current marker written; the user's own note left untouched.
        let refreshed = fs::read_to_string(&shipped).unwrap();
        assert!(refreshed.contains("Settings are grouped by domain"));
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
}
