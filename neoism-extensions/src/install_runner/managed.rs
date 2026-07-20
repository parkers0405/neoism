use super::*;

pub(super) async fn validate_binary(path: &Path) -> Result<(), InstallError> {
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| InstallError::BinaryNotFound(path.display().to_string()))?;
    if !metadata.is_file() {
        return Err(InstallError::BinaryNotFound(path.display().to_string()));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err(InstallError::BinaryNotFound(format!(
                "{} is not executable",
                path.display()
            )));
        }
    }
    Ok(())
}

/// Commit a completed install to the process-wide installed index. Exposed for
/// host-owned installers (Tree-sitter and notebook kernels) so they obey the
/// same "record before Done" contract as registry-backed packages.
pub async fn record_installed(entry: InstalledEntry) -> Result<(), InstallError> {
    let path = paths::installed_record_path();
    tokio::task::spawn_blocking(move || persist_installed_entry_to(&path, entry))
        .await
        .map_err(|error| {
            InstallError::ParseManifest(format!("installed index task failed: {error}"))
        })?
}

pub(super) fn persist_installed_entry_to(
    path: &Path,
    entry: InstalledEntry,
) -> Result<(), InstallError> {
    let lock = INSTALLED_INDEX_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = InstalledIndex::load_from(path)?;
    index.install_record(entry);
    index.save_to(path)
}

#[derive(Clone, Debug)]
pub struct ReconcileReport {
    pub index: InstalledIndex,
    pub recovered: Vec<String>,
}

/// Recover completed installs left behind by older Neoism versions that
/// linked the binary before the UI wrote `installed.json`.
///
/// This deliberately never searches `$PATH`. A package is recoverable only
/// when its managed launcher exists in Neoism's own `bin/` directory and can
/// be proven to originate from that manifest's `installed/<id>/` directory.
/// All recovered entries are committed in one atomic index write.
pub fn reconcile_managed_installs(
    manifests: &[ExtensionManifest],
) -> Result<ReconcileReport, InstallError> {
    let extensions_dir = paths::extensions_dir();
    let index_path = paths::installed_record_path();
    reconcile_managed_installs_from(manifests, &extensions_dir, &index_path)
}

pub(super) fn reconcile_managed_installs_from(
    manifests: &[ExtensionManifest],
    extensions_dir: &Path,
    index_path: &Path,
) -> Result<ReconcileReport, InstallError> {
    let lock = INSTALLED_INDEX_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut index = InstalledIndex::load_from(index_path)?;
    let mut recovered = Vec::new();

    for manifest in manifests {
        if index.is_installed(&manifest.id) || index.is_builtin_disabled(&manifest.id) {
            continue;
        }
        let Some(bin_path) = recoverable_managed_binary(manifest, extensions_dir) else {
            continue;
        };
        let installed_at = std::fs::metadata(&bin_path)
            .and_then(|metadata| metadata.modified())
            .ok()
            .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as i64)
            .unwrap_or_else(now_ms);
        index.install_record(InstalledEntry {
            id: manifest.id.clone(),
            version: manifest.version.clone(),
            install_kind: install_kind_tag(&manifest.install).to_string(),
            bin_path: Some(bin_path),
            installed_at,
        });
        recovered.push(manifest.id.clone());
    }

    if !recovered.is_empty() {
        index.save_to(index_path)?;
    }
    Ok(ReconcileReport { index, recovered })
}

pub(super) fn recoverable_managed_binary(
    manifest: &ExtensionManifest,
    extensions_dir: &Path,
) -> Option<PathBuf> {
    if !safe_path_component(&manifest.id) {
        return None;
    }
    let command = manifest.run.as_ref()?.command.first()?;
    if !safe_path_component(command) {
        return None;
    }

    let install_dir = extensions_dir.join("installed").join(&manifest.id);
    let canonical_install_dir = std::fs::canonicalize(&install_dir).ok()?;
    let managed_bin = extensions_dir.join("bin").join(command);
    if !valid_executable_file(&managed_bin) {
        return None;
    }

    let canonical_managed = std::fs::canonicalize(&managed_bin).ok()?;
    let managed_is_symlink = std::fs::symlink_metadata(&managed_bin)
        .ok()?
        .file_type()
        .is_symlink();
    let expected = expected_installed_binary(manifest, &install_dir, command)?;
    let canonical_expected = std::fs::canonicalize(&expected).ok()?;
    if !canonical_expected.starts_with(&canonical_install_dir)
        || !valid_executable_file(&expected)
    {
        return None;
    }

    // Unix installs use a symlink: both canonical paths must identify the
    // exact manifest-derived executable. Windows uses a managed copy because
    // symlinks commonly require elevation; accept that form only when it
    // byte-matches the expected binary inside the install directory.
    if canonical_managed != canonical_expected {
        if managed_is_symlink || !files_equal(&managed_bin, &expected) {
            return None;
        }
    }
    Some(managed_bin)
}

pub(super) fn expected_installed_binary(
    manifest: &ExtensionManifest,
    install_dir: &Path,
    command: &str,
) -> Option<PathBuf> {
    match &manifest.install {
        InstallKind::Npm { .. } => {
            let bin_dir = install_dir.join("node_modules").join(".bin");
            Some(npm_bin_path(&bin_dir, command))
        }
        InstallKind::Pip { .. } => {
            Some(venv_bin_path(&install_dir.join("venv"), command))
        }
        InstallKind::GithubRelease { assets, .. } => {
            let asset = pick_asset(assets, current_target())?;
            if asset
                .executables
                .get(command)
                .and_then(|recipe| interpreter_recipe(recipe))
                .is_some()
            {
                return Some(github_launcher_path(install_dir, command));
            }
            let candidate = install_dir.join(&asset.bin);
            if candidate.is_file() {
                return Some(candidate);
            }
            let name = Path::new(&asset.bin).file_name()?.to_str()?;
            find_binary_in_tree(install_dir, name)
        }
        InstallKind::Cargo { .. } => Some(cargo_bin_path(install_dir, command)),
        InstallKind::Go { .. } => Some(go_bin_path(install_dir, command)),
        InstallKind::Gem { .. } => Some(gem_shim_path(install_dir, command)),
    }
}

pub(super) fn safe_path_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !value.contains('/')
        && !value.contains('\\')
}

pub(super) fn valid_executable_file(path: &Path) -> bool {
    let Ok(metadata) = std::fs::metadata(path) else {
        return false;
    };
    if !metadata.is_file() || metadata.len() == 0 {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if metadata.permissions().mode() & 0o111 == 0 {
            return false;
        }
    }
    true
}

pub(super) fn files_equal(left: &Path, right: &Path) -> bool {
    let Ok(left_metadata) = std::fs::metadata(left) else {
        return false;
    };
    let Ok(right_metadata) = std::fs::metadata(right) else {
        return false;
    };
    if left_metadata.len() != right_metadata.len() {
        return false;
    }
    let (Ok(left), Ok(right)) = (std::fs::File::open(left), std::fs::File::open(right))
    else {
        return false;
    };
    use std::io::Read;
    let mut left = std::io::BufReader::new(left);
    let mut right = std::io::BufReader::new(right);
    let mut left_buf = [0u8; 64 * 1024];
    let mut right_buf = [0u8; 64 * 1024];
    loop {
        let (Ok(left_n), Ok(right_n)) =
            (left.read(&mut left_buf), right.read(&mut right_buf))
        else {
            return false;
        };
        if left_n != right_n || left_buf[..left_n] != right_buf[..right_n] {
            return false;
        }
        if left_n == 0 {
            return true;
        }
    }
}

pub(super) fn install_kind_tag(kind: &InstallKind) -> &'static str {
    match kind {
        InstallKind::Npm { .. } => "npm",
        InstallKind::Pip { .. } => "pip",
        InstallKind::GithubRelease { .. } => "github_release",
        InstallKind::Cargo { .. } => "cargo",
        InstallKind::Go { .. } => "go",
        InstallKind::Gem { .. } => "gem",
    }
}

// ---------------------------------------------------------------------------
// symlink wiring
// ---------------------------------------------------------------------------

/// Compute the bin name the user invokes — first fall back to file stem of
/// the actual binary so we always have something to symlink under.
pub(super) fn default_bin_name(manifest: &ExtensionManifest, bin_path: &Path) -> String {
    if let Some(name) = manifest.run.as_ref().and_then(|r| r.command.first()) {
        return name.clone();
    }
    bin_path
        .file_name()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| manifest.id.clone())
}

pub(super) async fn link_bin(
    bin_path: &Path,
    link_name: String,
) -> Result<PathBuf, InstallError> {
    if !safe_path_component(&link_name) {
        return Err(InstallError::ParseManifest(format!(
            "managed executable name must be one path component, got `{link_name}`"
        )));
    }
    let bin_dir = paths::bin_dir();
    tokio::fs::create_dir_all(&bin_dir).await?;
    let link_path = bin_dir.join(&link_name);
    let temp_name = format!(
        ".{link_name}.neoism-tmp-{}-{}",
        std::process::id(),
        now_ms()
    );
    let temp_path = bin_dir.join(temp_name);
    let _ = tokio::fs::remove_file(&temp_path).await;

    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(bin_path, &temp_path)?;
        if let Err(error) = tokio::fs::rename(&temp_path, &link_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(InstallError::Io(error));
        }
    }
    #[cfg(windows)]
    {
        // Symlinks on Windows require elevated rights; a plain copy is
        // simpler and avoids that requirement for the common case. Prepare
        // the copy first so cancellation never exposes a partial executable.
        std::fs::copy(bin_path, &temp_path)?;
        let _ = tokio::fs::remove_file(&link_path).await;
        if let Err(error) = tokio::fs::rename(&temp_path, &link_path).await {
            let _ = tokio::fs::remove_file(&temp_path).await;
            return Err(InstallError::Io(error));
        }
    }
    validate_binary(&link_path).await?;
    Ok(link_path)
}
