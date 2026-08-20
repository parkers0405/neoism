use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config::{
    notes_vaults_dir, DEFAULT_NOTES_WORKSPACE, DEFAULT_NOTES_WORKSPACE_ID,
};

pub const VAULT_REGISTRY_FILE: &str = ".neoism-vaults.json";
const CURRENT_VAULT_REGISTRY_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct NotesVault {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
struct VaultRegistry {
    version: u32,
    default_vault_id: String,
    #[serde(default)]
    vaults: Vec<NotesVault>,
}

impl Default for VaultRegistry {
    fn default() -> Self {
        Self {
            version: CURRENT_VAULT_REGISTRY_VERSION,
            default_vault_id: String::new(),
            vaults: Vec::new(),
        }
    }
}

pub fn vault_registry_path() -> PathBuf {
    notes_vaults_dir().join(VAULT_REGISTRY_FILE)
}

/// Return all registered vaults. Existing top-level directories from the
/// pre-registry layout are registered on first read, so upgrading does not
/// require an explicit migration command.
pub fn notes_vaults() -> std::io::Result<Vec<NotesVault>> {
    Ok(load_registry()?.vaults)
}

pub fn existing_notes_vaults() -> std::io::Result<Vec<NotesVault>> {
    let mut vaults = notes_vaults()?
        .into_iter()
        .filter(|vault| vault.path.is_dir())
        .collect::<Vec<_>>();
    vaults
        .sort_by(|left, right| left.name.to_lowercase().cmp(&right.name.to_lowercase()));
    Ok(vaults)
}

pub fn default_notes_vault() -> std::io::Result<NotesVault> {
    let registry = load_registry()?;
    registry
        .vaults
        .iter()
        .find(|vault| vault.id == registry.default_vault_id)
        .cloned()
        .or_else(|| registry.vaults.first().cloned())
        .ok_or_else(|| std::io::Error::other("notes vault registry is empty"))
}

pub fn notes_vault_by_id(id: &str) -> std::io::Result<Option<NotesVault>> {
    Ok(load_registry()?
        .vaults
        .into_iter()
        .find(|vault| vault.id == id))
}

pub fn notes_vault_by_name(name: &str) -> std::io::Result<Option<NotesVault>> {
    let name = name.trim();
    Ok(load_registry()?
        .vaults
        .into_iter()
        .find(|vault| vault.name == name))
}

pub fn notes_vault_for_path(
    path: impl AsRef<Path>,
) -> std::io::Result<Option<NotesVault>> {
    let path = comparable_path(path.as_ref());
    let mut matches = load_registry()?
        .vaults
        .into_iter()
        .filter(|vault| {
            let root = comparable_path(&vault.path);
            path == root || path.starts_with(&root)
        })
        .collect::<Vec<_>>();
    // If custom vault roots ever overlap, the innermost registered vault owns
    // the path. This also makes the result deterministic.
    matches.sort_by_key(|vault| comparable_path(&vault.path).components().count());
    Ok(matches.pop())
}

pub fn ensure_notes_vault(
    name: &str,
    path: impl AsRef<Path>,
) -> std::io::Result<NotesVault> {
    let name = normalized_name(name, path.as_ref());
    let path = absolute_path(path.as_ref());
    let mut registry = load_registry()?;
    if let Some(index) = registry
        .vaults
        .iter()
        .position(|vault| same_path(&vault.path, &path))
    {
        if registry.vaults[index].name != name {
            registry.vaults[index].name = name;
            save_registry(&registry)?;
        }
        return Ok(registry.vaults[index].clone());
    }
    if let Some(existing) = registry.vaults.iter().find(|vault| vault.name == name) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!(
                "vault name `{name}` is already registered at {}",
                existing.path.display()
            ),
        ));
    }
    let id = if name == DEFAULT_NOTES_WORKSPACE
        && !registry
            .vaults
            .iter()
            .any(|vault| vault.id == DEFAULT_NOTES_WORKSPACE_ID)
    {
        DEFAULT_NOTES_WORKSPACE_ID.to_string()
    } else {
        Uuid::new_v4().to_string()
    };
    let vault = NotesVault { id, name, path };
    if registry.default_vault_id.is_empty() {
        registry.default_vault_id = vault.id.clone();
    }
    registry.vaults.push(vault.clone());
    save_registry(&registry)?;
    Ok(vault)
}

/// Rename a local vault directory while keeping its stable id (and therefore
/// default-vault identity, cache key, and project links) unchanged.
pub fn rename_notes_vault(
    old_path: impl AsRef<Path>,
    new_name: &str,
) -> std::io::Result<NotesVault> {
    let old_path = absolute_path(old_path.as_ref());
    let new_name = new_name.trim();
    let mut components = Path::new(new_name).components();
    let valid_name = matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none();
    if new_name.is_empty() || !valid_name {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "vault name must be one folder name",
        ));
    }
    let vaults_root = absolute_path(&notes_vaults_dir());
    if old_path.parent() != Some(vaults_root.as_path()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "only top-level local vaults can be renamed",
        ));
    }
    let new_path = vaults_root.join(new_name);
    if !same_path(&old_path, &new_path) && new_path.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::AlreadyExists,
            format!("vault already exists: {}", new_path.display()),
        ));
    }

    let mut registry = load_registry()?;
    let index = registry
        .vaults
        .iter()
        .position(|vault| same_path(&vault.path, &old_path))
        .ok_or_else(|| std::io::Error::other("vault is not registered"))?;

    if !same_path(&old_path, &new_path) {
        fs::rename(&old_path, &new_path)?;
    }
    let old_record = registry.vaults[index].clone();
    registry.vaults[index].name = new_name.to_string();
    registry.vaults[index].path = new_path.clone();
    if let Err(error) = save_registry(&registry) {
        if !same_path(&old_path, &new_path) {
            let _ = fs::rename(&new_path, &old_path);
        }
        return Err(error);
    }
    let renamed = registry.vaults[index].clone();
    // The registry and vault-owned metadata are authoritative; updating the
    // optional per-code-root cache is best effort and cannot roll back a
    // successful directory rename.
    let _ = update_linked_workspace_names(&old_record, &renamed);
    Ok(renamed)
}

pub(crate) fn remove_notes_vault_registration(
    vault_id: &str,
    replacement_default_id: &str,
) -> std::io::Result<()> {
    let mut registry = load_registry()?;
    registry.vaults.retain(|vault| vault.id != vault_id);
    if registry.default_vault_id == vault_id {
        registry.default_vault_id = replacement_default_id.to_string();
    }
    save_registry(&registry)
}

fn load_registry() -> std::io::Result<VaultRegistry> {
    let root = notes_vaults_dir();
    fs::create_dir_all(&root)?;
    let path = vault_registry_path();
    let mut registry = if path.is_file() {
        let source = fs::read_to_string(&path)?;
        serde_json::from_str::<VaultRegistry>(&source).map_err(|error| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("failed to parse {}: {error}", path.display()),
            )
        })?
    } else {
        VaultRegistry::default()
    };
    let mut changed = registry.version != CURRENT_VAULT_REGISTRY_VERSION;
    registry.version = CURRENT_VAULT_REGISTRY_VERSION;

    // Import the original layout: every direct child directory was a vault.
    if let Ok(entries) = fs::read_dir(&root) {
        for entry in entries.filter_map(Result::ok) {
            let dir = entry.path();
            if !dir.is_dir()
                || dir
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with('.'))
                || registry
                    .vaults
                    .iter()
                    .any(|vault| same_path(&vault.path, &dir))
            {
                continue;
            }
            let name = normalized_name("", &dir);
            let id = if name == DEFAULT_NOTES_WORKSPACE
                && !registry
                    .vaults
                    .iter()
                    .any(|vault| vault.id == DEFAULT_NOTES_WORKSPACE_ID)
            {
                DEFAULT_NOTES_WORKSPACE_ID.to_string()
            } else {
                Uuid::new_v4().to_string()
            };
            registry.vaults.push(NotesVault {
                id,
                name,
                path: absolute_path(&dir),
            });
            changed = true;
        }
    }

    if registry.vaults.is_empty() {
        registry.vaults.push(NotesVault {
            id: DEFAULT_NOTES_WORKSPACE_ID.to_string(),
            name: DEFAULT_NOTES_WORKSPACE.to_string(),
            path: absolute_path(&root.join(DEFAULT_NOTES_WORKSPACE)),
        });
        changed = true;
    }
    if !registry
        .vaults
        .iter()
        .any(|vault| vault.id == registry.default_vault_id)
    {
        registry.default_vault_id = registry
            .vaults
            .iter()
            .find(|vault| vault.name == DEFAULT_NOTES_WORKSPACE)
            .or_else(|| registry.vaults.first())
            .map(|vault| vault.id.clone())
            .unwrap_or_default();
        changed = true;
    }
    if changed || !path.is_file() {
        save_registry(&registry)?;
    }
    Ok(registry)
}

fn save_registry(registry: &VaultRegistry) -> std::io::Result<()> {
    let path = vault_registry_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let source = serde_json::to_vec_pretty(registry).map_err(std::io::Error::other)?;
    let temp = path.with_extension("json.tmp");
    fs::write(&temp, source)?;
    #[cfg(windows)]
    if path.exists() {
        fs::remove_file(&path)?;
    }
    fs::rename(temp, path)
}

fn normalized_name(name: &str, path: &Path) -> String {
    let name = name.trim();
    if !name.is_empty() {
        return name.to_string();
    }
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or(DEFAULT_NOTES_WORKSPACE)
        .to_string()
}

fn absolute_path(path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    }
}

fn comparable_path(path: &Path) -> PathBuf {
    crate::path::canonicalize(path).unwrap_or_else(|_| absolute_path(path))
}

fn same_path(left: &Path, right: &Path) -> bool {
    comparable_path(left) == comparable_path(right)
}

fn update_linked_workspace_names(
    old_vault: &NotesVault,
    new_vault: &NotesVault,
) -> std::io::Result<()> {
    let Some(metadata) = crate::config::read_vault_project_metadata(&new_vault.path)
    else {
        return Ok(());
    };
    let mut first_error = None;
    for link in metadata.links {
        let Ok(Some(mut workspace)) = crate::config::load_workspace(&link.path) else {
            continue;
        };
        if workspace.config.notes.vault_id.as_deref() == Some(old_vault.id.as_str())
            || workspace.config.notes.workspace == old_vault.name
        {
            workspace.config.notes.vault_id = Some(new_vault.id.clone());
            workspace.config.notes.workspace = new_vault.name.clone();
            if let Err(error) = crate::config::save_workspace(&workspace) {
                first_error.get_or_insert(error);
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}
