use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::path::{Component, Path, PathBuf};

use super::checksum::tree_sha256;
use super::error::{io, AcquisitionError, AcquisitionStage};
use super::git::{self, Repository};
use super::lockfile::{LuaPluginLock, LuaPluginLockEntry};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PluginSpec {
    pub plugin_id: String,
    pub repository_url: String,
    pub requested_ref: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ValidatedPluginMetadata {
    pub plugin_version: String,
    pub manifest_checksum: String,
    pub dependencies: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProgressEvent {
    Staging { plugin_id: String },
    Cloning,
    Fetching,
    Resolving,
    CheckingOut { commit: String },
    Checksumming,
    Validating,
    PublishingRevision { path: PathBuf },
    PublishingLockfile,
    Removing { plugin_id: String },
    Pruning { path: PathBuf },
    Done,
}

pub struct ValidationContext<'a> {
    pub spec: &'a PluginSpec,
    pub checkout_path: &'a Path,
    pub resolved_commit: &'a str,
    pub tree_checksum: &'a str,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct PruneReport {
    pub removed_revisions: Vec<PathBuf>,
    pub retained_revisions: usize,
}

#[derive(Clone, Debug)]
pub struct LuaPluginStore {
    root: PathBuf,
    staging_root: PathBuf,
}

impl LuaPluginStore {
    pub fn new(root: impl Into<PathBuf>, staging_root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), staging_root: staging_root.into() }
    }

    pub fn managed() -> Self {
        Self::new(crate::paths::extensions_dir().join("lua-plugins"), crate::paths::staging_dir().join("lua-plugins"))
    }

    pub fn lockfile_path(&self) -> PathBuf { self.root.join("plugins.lock.json") }
    pub fn revisions_root(&self) -> PathBuf { self.root.join("store") }
    pub fn load_lock(&self) -> Result<LuaPluginLock, AcquisitionError> { LuaPluginLock::load(&self.lockfile_path()) }
    pub fn installed_path(&self, entry: &LuaPluginLockEntry) -> Result<PathBuf, AcquisitionError> {
        safe_join(&self.root, &entry.installed_revision_path)
    }

    pub fn install<F, P>(&self, spec: PluginSpec, progress: P, validate: F) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String>, P: FnMut(ProgressEvent) {
        self.acquire(spec, None, progress, validate)
    }

    pub fn update<F, P>(&self, spec: PluginSpec, progress: P, validate: F) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String>, P: FnMut(ProgressEvent) {
        validate_spec(&spec)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let lock = self.load_lock()?;
        if !lock.plugins.contains_key(&spec.plugin_id) {
            return Err(AcquisitionError::NotInstalled(spec.plugin_id));
        }
        self.acquire_locked(spec, None, lock, progress, validate, true)
    }

    pub fn restore_exact<F, P>(&self, plugin_id: &str, mut progress: P, validate: F) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String>, P: FnMut(ProgressEvent) {
        validate_id(plugin_id)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let lock = self.load_lock()?;
        let entry = lock.plugins.get(plugin_id).cloned().ok_or_else(|| AcquisitionError::NotInstalled(plugin_id.to_owned()))?;
        let spec = PluginSpec {
            plugin_id: entry.plugin_id.clone(), repository_url: entry.repository_url.clone(), requested_ref: entry.requested_ref.clone(),
        };
        validate_spec(&spec)?;
        let installed = safe_join(&self.root, &entry.installed_revision_path)?;
        if installed.is_dir() {
            progress(ProgressEvent::Checksumming);
            let actual = tree_sha256(&installed)?;
            if actual != entry.tree_checksum { return Err(AcquisitionError::ChecksumMismatch { expected: entry.tree_checksum, actual }); }
            progress(ProgressEvent::Validating);
            let metadata = validate(&ValidationContext { spec: &spec, checkout_path: &installed, resolved_commit: &entry.resolved_commit, tree_checksum: &actual }).map_err(AcquisitionError::Validation)?;
            validate_restored_metadata(&entry, &metadata)?;
            progress(ProgressEvent::Done);
            return Ok(entry);
        }
        self.acquire_locked(spec, Some((&entry.resolved_commit, &entry.tree_checksum)), lock, progress, validate, false)
    }

    pub fn remove<P: FnMut(ProgressEvent)>(&self, plugin_id: &str, mut progress: P) -> Result<LuaPluginLockEntry, AcquisitionError> {
        validate_id(plugin_id)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let mut lock = self.load_lock()?;
        let entry = lock.plugins.remove(plugin_id).ok_or_else(|| AcquisitionError::NotInstalled(plugin_id.to_owned()))?;
        progress(ProgressEvent::Removing { plugin_id: plugin_id.to_owned() });
        lock.save_atomic(&self.lockfile_path())?;
        // Revisions are immutable and deliberately retained here. Deletion is
        // delegated to `prune`, whose reference walk makes cleanup safe and
        // keeps remove atomic at the lockfile publication boundary.
        progress(ProgressEvent::Done);
        Ok(entry)
    }

    /// Restore one lock entry when the host rejects the full activation
    /// candidate after acquisition. Immutable revisions remain available until
    /// prune, so this returns the store to its exact last-known-good pointer.
    pub fn rollback_lock_entry(
        &self,
        plugin_id: &str,
        previous: Option<LuaPluginLockEntry>,
    ) -> Result<(), AcquisitionError> {
        validate_id(plugin_id)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let mut lock = self.load_lock()?;
        match previous {
            Some(entry) => {
                if entry.plugin_id != plugin_id {
                    return Err(AcquisitionError::Validation(format!(
                        "rollback entry `{}` does not match plugin `{plugin_id}`",
                        entry.plugin_id
                    )));
                }
                lock.plugins.insert(plugin_id.to_string(), entry);
            }
            None => {
                lock.plugins.remove(plugin_id);
            }
        }
        lock.save_atomic(&self.lockfile_path())
    }

    pub fn prune<P: FnMut(ProgressEvent)>(&self, mut progress: P) -> Result<PruneReport, AcquisitionError> {
        let _guard = OperationGuard::acquire(&self.root)?;
        let lock = self.load_lock()?;
        let referenced = lock.plugins.values().map(|entry| entry.installed_revision_path.clone()).collect::<BTreeSet<_>>();
        let mut report = PruneReport::default();
        let store = self.revisions_root();
        if !store.exists() { return Ok(report); }
        for plugin in read_dirs(&store, AcquisitionStage::Prune)? {
            let id = plugin.file_name().to_string_lossy().into_owned();
            if validate_id(&id).is_err() || plugin.file_type().map_err(|e| io(AcquisitionStage::Prune, plugin.path(), e))?.is_symlink() { continue; }
            let revisions = plugin.path().join("revisions");
            if !revisions.is_dir() { continue; }
            for revision in read_dirs(&revisions, AcquisitionStage::Prune)? {
                let path = revision.path();
                let commit = revision.file_name().to_string_lossy().into_owned();
                let relative = PathBuf::from("store").join(&id).join("revisions").join(&commit);
                let ty = revision.file_type().map_err(|e| io(AcquisitionStage::Prune, &path, e))?;
                if !ty.is_dir() || ty.is_symlink() || !valid_commit(&commit) { continue; }
                if referenced.contains(&relative) { report.retained_revisions += 1; continue; }
                progress(ProgressEvent::Pruning { path: path.clone() });
                fs::remove_dir_all(&path).map_err(|e| io(AcquisitionStage::Prune, &path, e))?;
                report.removed_revisions.push(relative);
            }
        }
        progress(ProgressEvent::Done);
        Ok(report)
    }

    fn acquire<F, P>(&self, spec: PluginSpec, exact: Option<(&str, &str)>, progress: P, validate: F) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String>, P: FnMut(ProgressEvent) {
        validate_spec(&spec)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let lock = self.load_lock()?;
        self.acquire_locked(spec, exact, lock, progress, validate, true)
    }

    fn acquire_locked<F, P>(&self, spec: PluginSpec, exact: Option<(&str, &str)>, mut lock: LuaPluginLock, mut progress: P, validate: F, publish_lock: bool) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String>, P: FnMut(ProgressEvent) {
        fs::create_dir_all(&self.staging_root).map_err(|e| io(AcquisitionStage::Clone, &self.staging_root, e))?;
        let staging = self.staging_root.join(format!("{}-{}-{}", spec.plugin_id, std::process::id(), unique()));
        let mut staging_guard = StagingGuard(Some(staging.clone()));
        progress(ProgressEvent::Staging { plugin_id: spec.plugin_id.clone() });
        progress(ProgressEvent::Cloning);
        progress(ProgressEvent::Fetching);
        let requested = exact.map(|v| v.0).unwrap_or(&spec.requested_ref);
        let commit = git::checkout(Repository::Https(&spec.repository_url), requested, &staging)?;
        progress(ProgressEvent::Resolving);
        if let Some((expected, _)) = exact {
            if commit != expected { return Err(AcquisitionError::InvalidResolvedCommit(commit)); }
        }
        progress(ProgressEvent::CheckingOut { commit: commit.clone() });
        progress(ProgressEvent::Checksumming);
        let checksum = tree_sha256(&staging)?;
        if let Some((_, expected)) = exact {
            if checksum != expected { return Err(AcquisitionError::ChecksumMismatch { expected: expected.to_owned(), actual: checksum }); }
        }
        progress(ProgressEvent::Validating);
        let metadata = validate(&ValidationContext { spec: &spec, checkout_path: &staging, resolved_commit: &commit, tree_checksum: &checksum }).map_err(AcquisitionError::Validation)?;
        if let Some(entry) = lock.plugins.get(&spec.plugin_id).filter(|_| exact.is_some()) {
            validate_restored_metadata(entry, &metadata)?;
        }
        let after_validation = tree_sha256(&staging)?;
        if after_validation != checksum { return Err(AcquisitionError::ChecksumMismatch { expected: checksum, actual: after_validation }); }

        let relative = PathBuf::from("store").join(&spec.plugin_id).join("revisions").join(&commit);
        let final_path = safe_join(&self.root, &relative)?;
        progress(ProgressEvent::PublishingRevision { path: final_path.clone() });
        if final_path.exists() {
            let actual = tree_sha256(&final_path)?;
            if actual != after_validation { return Err(AcquisitionError::ImmutableRevisionCollision { path: final_path, expected: after_validation, actual }); }
        } else {
            let parent = final_path.parent().ok_or_else(|| AcquisitionError::UnsafePath(final_path.clone()))?;
            fs::create_dir_all(parent).map_err(|e| io(AcquisitionStage::PublishRevision, parent, e))?;
            fs::rename(&staging, &final_path).map_err(|e| io(AcquisitionStage::PublishRevision, &final_path, e))?;
            staging_guard.0 = None;
        }
        let entry = LuaPluginLockEntry {
            plugin_id: spec.plugin_id.clone(), repository_url: spec.repository_url, requested_ref: spec.requested_ref,
            resolved_commit: commit, plugin_version: metadata.plugin_version, manifest_checksum: metadata.manifest_checksum,
            tree_checksum: after_validation, dependencies: normalized_dependencies(metadata.dependencies), installed_revision_path: relative,
        };
        if publish_lock {
            progress(ProgressEvent::PublishingLockfile);
            lock.plugins.insert(entry.plugin_id.clone(), entry.clone());
            lock.save_atomic(&self.lockfile_path())?;
        }
        progress(ProgressEvent::Done);
        Ok(entry)
    }

    #[cfg(test)]
    fn install_test_local<F>(&self, mut spec: PluginSpec, repository: &Path, validate: F) -> Result<LuaPluginLockEntry, AcquisitionError>
    where F: FnOnce(&ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String> {
        validate_id(&spec.plugin_id)?;
        validate_ref(&spec.requested_ref)?;
        let _guard = OperationGuard::acquire(&self.root)?;
        let mut lock = self.load_lock()?;
        fs::create_dir_all(&self.staging_root).map_err(|e| io(AcquisitionStage::Clone, &self.staging_root, e))?;
        let staging = self.staging_root.join(format!("test-{}", unique()));
        let mut guard = StagingGuard(Some(staging.clone()));
        let commit = git::checkout(Repository::TestLocal(repository), &spec.requested_ref, &staging)?;
        let checksum = tree_sha256(&staging)?;
        let metadata = validate(&ValidationContext { spec: &spec, checkout_path: &staging, resolved_commit: &commit, tree_checksum: &checksum }).map_err(AcquisitionError::Validation)?;
        if tree_sha256(&staging)? != checksum { return Err(AcquisitionError::Validation("validator modified checkout".into())); }
        let relative = PathBuf::from("store").join(&spec.plugin_id).join("revisions").join(&commit);
        let final_path = safe_join(&self.root, &relative)?;
        fs::create_dir_all(final_path.parent().unwrap()).map_err(|e| io(AcquisitionStage::PublishRevision, &final_path, e))?;
        if !final_path.exists() { fs::rename(&staging, &final_path).map_err(|e| io(AcquisitionStage::PublishRevision, &final_path, e))?; guard.0 = None; }
        spec.repository_url = format!("test-local://{}", repository.display());
        let entry = LuaPluginLockEntry { plugin_id: spec.plugin_id.clone(), repository_url: spec.repository_url, requested_ref: spec.requested_ref, resolved_commit: commit, plugin_version: metadata.plugin_version, manifest_checksum: metadata.manifest_checksum, tree_checksum: checksum, dependencies: normalized_dependencies(metadata.dependencies), installed_revision_path: relative };
        lock.plugins.insert(entry.plugin_id.clone(), entry.clone());
        lock.save_atomic(&self.lockfile_path())?;
        Ok(entry)
    }
}

fn validate_spec(spec: &PluginSpec) -> Result<(), AcquisitionError> {
    validate_id(&spec.plugin_id)?;
    validate_ref(&spec.requested_ref)?;
    let url = reqwest::Url::parse(&spec.repository_url).map_err(|e| AcquisitionError::InvalidRepositoryUrl { url: spec.repository_url.clone(), reason: e.to_string() })?;
    let lower = spec.repository_url.to_ascii_lowercase();
    let traverses = url.path_segments().into_iter().flatten().any(|part| matches!(part, "." | "..")) || lower.contains("%2e") || spec.repository_url.contains('\\');
    if url.scheme() != "https" || url.host_str().is_none() || !url.username().is_empty() || url.password().is_some() || url.query().is_some() || url.fragment().is_some() || traverses {
        return Err(AcquisitionError::InvalidRepositoryUrl { url: spec.repository_url.clone(), reason: "only credential-free HTTPS repository URLs without traversal, query, or fragment are allowed".into() });
    }
    Ok(())
}

fn validate_restored_metadata(entry: &LuaPluginLockEntry, metadata: &ValidatedPluginMetadata) -> Result<(), AcquisitionError> {
    let dependencies = normalized_dependencies(metadata.dependencies.clone());
    if metadata.plugin_version != entry.plugin_version
        || metadata.manifest_checksum != entry.manifest_checksum
        || dependencies != entry.dependencies
    {
        return Err(AcquisitionError::Validation(format!(
            "restored package metadata does not match lock entry for `{}`",
            entry.plugin_id
        )));
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), AcquisitionError> {
    if id.is_empty() || id.len() > 128 || matches!(id, "." | "..") || !id.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')) {
        Err(AcquisitionError::InvalidPluginId(id.to_owned()))
    } else { Ok(()) }
}

fn validate_ref(reference: &str) -> Result<(), AcquisitionError> {
    if reference.is_empty() || reference.len() > 512 || reference.starts_with('-') || reference.bytes().any(|b| b.is_ascii_control()) || reference.contains("..") || reference.contains("@{") || reference.ends_with('.') || reference.ends_with('/') || reference.contains('\\') {
        Err(AcquisitionError::InvalidRef(reference.to_owned()))
    } else { Ok(()) }
}

fn normalized_dependencies(mut dependencies: Vec<String>) -> Vec<String> { dependencies.sort(); dependencies.dedup(); dependencies }
fn valid_commit(value: &str) -> bool { matches!(value.len(), 40 | 64) && value.bytes().all(|b| b.is_ascii_hexdigit()) }

fn safe_join(root: &Path, relative: &Path) -> Result<PathBuf, AcquisitionError> {
    if relative.is_absolute() || relative.components().any(|part| !matches!(part, Component::Normal(_))) { return Err(AcquisitionError::UnsafePath(relative.to_path_buf())); }
    Ok(root.join(relative))
}

fn read_dirs(path: &Path, stage: AcquisitionStage) -> Result<Vec<fs::DirEntry>, AcquisitionError> {
    fs::read_dir(path).map_err(|e| io(stage, path, e))?.collect::<Result<Vec<_>, _>>().map_err(|e| io(stage, path, e))
}

struct StagingGuard(Option<PathBuf>);
impl Drop for StagingGuard { fn drop(&mut self) { if let Some(path) = self.0.take() { let _ = fs::remove_dir_all(path); } } }

struct OperationGuard { path: PathBuf }
impl OperationGuard {
    fn acquire(root: &Path) -> Result<Self, AcquisitionError> {
        fs::create_dir_all(root).map_err(|e| io(AcquisitionStage::Lock, root, e))?;
        let path = root.join(".plugins.lock");
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => Ok(Self { path }),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Err(AcquisitionError::StoreBusy(path)),
            Err(e) => Err(io(AcquisitionStage::Lock, path, e)),
        }
    }
}
impl Drop for OperationGuard { fn drop(&mut self) { let _ = fs::remove_file(&self.path); } }

fn unique() -> u128 { std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos() }

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn rejects_non_https_and_traversal() {
        for url in ["file:///tmp/plugin", "../plugin", "https://example.com/a/%2e%2e/b", "https://user@example.com/p"] {
            assert!(validate_spec(&spec("p", url)).is_err(), "accepted {url}");
        }
        assert!(validate_spec(&spec("p", "https://example.com/owner/plugin.git")).is_ok());
        assert!(validate_id("../escape").is_err());
    }

    #[test]
    fn failed_validation_preserves_old_lock_and_revision() {
        let Some((temp, repo)) = repository() else { return; };
        let store = LuaPluginStore::new(temp.join("managed"), temp.join("staging"));
        let old = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(metadata())).unwrap();
        commit(&repo, "plugin.lua", "return 2");
        let before = fs::read(store.lockfile_path()).unwrap();
        let result = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Err("bad plugin".into()));
        assert!(matches!(result, Err(AcquisitionError::Validation(_))));
        assert_eq!(before, fs::read(store.lockfile_path()).unwrap());
        assert!(store.root.join(old.installed_revision_path).is_dir());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn revisions_are_immutable_and_prune_keeps_referenced_commit() {
        let Some((temp, repo)) = repository() else { return; };
        let store = LuaPluginStore::new(temp.join("managed"), temp.join("staging"));
        let first = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(metadata())).unwrap();
        commit(&repo, "plugin.lua", "return 2");
        let second = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(metadata())).unwrap();
        assert_ne!(first.installed_revision_path, second.installed_revision_path);
        assert!(store.root.join(&first.installed_revision_path).is_dir());
        let report = store.prune(|_| {}).unwrap();
        assert_eq!(report.removed_revisions, vec![first.installed_revision_path]);
        assert!(store.root.join(second.installed_revision_path).is_dir());
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn validator_metadata_is_published_and_checked_on_restore() {
        let Some((temp, repo)) = repository() else { return; };
        let store = LuaPluginStore::new(temp.join("managed"), temp.join("staging"));
        let mut validated = metadata();
        validated.plugin_version = "2.4.0".into();
        validated.manifest_checksum = "validated-manifest".into();
        let installed = store
            .install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(validated.clone()))
            .unwrap();
        assert_eq!(installed.plugin_version, "2.4.0");
        assert_eq!(installed.manifest_checksum, "validated-manifest");

        let mut mismatched = validated;
        mismatched.plugin_version = "9.9.9".into();
        let error = validate_restored_metadata(&installed, &mismatched).unwrap_err();
        assert!(matches!(&error, AcquisitionError::Validation(message) if message.contains("does not match lock entry")), "{error:?}");
        let _ = fs::remove_dir_all(temp);
    }

    #[test]
    fn rollback_restores_exact_prior_lock_entry() {
        let Some((temp, repo)) = repository() else { return; };
        let store = LuaPluginStore::new(temp.join("managed"), temp.join("staging"));
        let first = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(metadata())).unwrap();
        commit(&repo, "plugin.lua", "return 3");
        let second = store.install_test_local(spec("plugin", "https://example.invalid/plugin.git"), &repo, |_| Ok(metadata())).unwrap();
        assert_ne!(first.resolved_commit, second.resolved_commit);
        store.rollback_lock_entry("plugin", Some(first.clone())).unwrap();
        assert_eq!(store.load_lock().unwrap().plugins.get("plugin"), Some(&first));
        let _ = fs::remove_dir_all(temp);
    }

    fn spec(id: &str, url: &str) -> PluginSpec {
        PluginSpec { plugin_id: id.into(), repository_url: url.into(), requested_ref: "main".into() }
    }

    fn metadata() -> ValidatedPluginMetadata {
        ValidatedPluginMetadata { plugin_version: "1.0.0".into(), manifest_checksum: "manifest-sha".into(), dependencies: vec!["z".into(), "a".into(), "a".into()] }
    }

    fn repository() -> Option<(PathBuf, PathBuf)> {
        if Command::new("git").arg("--version").output().ok()?.status.success() == false { return None; }
        let temp = std::env::temp_dir().join(format!("neoism-lua-store-{}-{}", std::process::id(), unique()));
        let repo = temp.join("repo");
        fs::create_dir_all(&repo).unwrap();
        git_cmd(&repo, ["init", "-b", "main"]);
        git_cmd(&repo, ["config", "user.email", "test@neoism.invalid"]);
        git_cmd(&repo, ["config", "user.name", "Neoism Test"]);
        commit(&repo, "plugin.lua", "return 1");
        Some((temp, repo))
    }

    fn commit(repo: &Path, file: &str, body: &str) {
        fs::write(repo.join(file), body).unwrap();
        git_cmd(repo, ["add", "--", file]);
        git_cmd(repo, ["commit", "-m", body]);
    }

    fn git_cmd<const N: usize>(repo: &Path, args: [&str; N]) {
        let output = Command::new("git").current_dir(repo).args(args).output().unwrap();
        assert!(output.status.success(), "{}", String::from_utf8_lossy(&output.stderr));
    }
}