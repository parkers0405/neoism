use super::*;

// ---------------------------------------------------------------------------
// github release
// ---------------------------------------------------------------------------

pub(super) async fn install_github_release(
    install_dir: &Path,
    owner: &str,
    repo: &str,
    tag: &str,
    asset: &GithubAsset,
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    tokio::fs::create_dir_all(install_dir).await?;
    let staging = install_dir.join("staging");
    tokio::fs::create_dir_all(&staging).await?;

    let asset_name = safe_asset_name(&asset.file)?;
    let url = github_asset_url(owner, repo, tag, &asset.file);
    let staged_file = staging.join(asset_name);
    let partial_file = staging.join(format!("{asset_name}.part"));
    let _ = tokio::fs::remove_file(&partial_file).await;
    let mut partial_guard = RemovePartialOnDrop::new(partial_file.clone());

    emit(
        progress,
        ProgressEvent::Waiting {
            status: format!("connecting to GitHub for {asset_name}"),
        },
    );
    let client = reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .user_agent(concat!("neoism/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|error| InstallError::Network(error.to_string()))?;
    let response =
        tokio::time::timeout(DOWNLOAD_CONNECT_TIMEOUT, client.get(&url).send())
            .await
            .map_err(|_| InstallError::TimedOut {
                tool: format!("GitHub connection for {asset_name}"),
                seconds: DOWNLOAD_CONNECT_TIMEOUT.as_secs(),
            })?
            .map_err(|error| InstallError::Network(error.to_string()))?
            .error_for_status()
            .map_err(|error| InstallError::Network(error.to_string()))?;
    let total = response.content_length().filter(|total| *total > 0);
    emit(progress, ProgressEvent::Downloading { bytes: 0, total });

    let mut out = tokio::fs::File::create(&partial_file).await?;
    let mut stream = response.bytes_stream();
    let mut downloaded: u64 = 0;
    let mut last_percent = download_percent(downloaded, total);
    let mut last_emit = Instant::now();
    loop {
        let next = tokio::time::timeout(DOWNLOAD_IDLE_TIMEOUT, stream.next())
            .await
            .map_err(|_| InstallError::TimedOut {
                tool: format!("download of {asset_name}"),
                seconds: DOWNLOAD_IDLE_TIMEOUT.as_secs(),
            })?;
        let Some(chunk) = next else {
            break;
        };
        let bytes = chunk.map_err(|error| InstallError::Network(error.to_string()))?;
        out.write_all(&bytes).await?;
        downloaded = downloaded.saturating_add(bytes.len() as u64);
        let percent = download_percent(downloaded, total);
        // At most one event per percentage point (known total) or four per
        // second (unknown total). A 40 MB binary must not enqueue thousands
        // of chunk events faster than the UI can drain them.
        if percent != last_percent || last_emit.elapsed() >= Duration::from_millis(250) {
            last_percent = percent;
            last_emit = Instant::now();
            emit(
                progress,
                ProgressEvent::Downloading {
                    bytes: downloaded,
                    total,
                },
            );
        }
    }
    out.flush().await?;
    drop(out);

    let _ = tokio::fs::remove_file(&staged_file).await;
    tokio::fs::rename(&partial_file, &staged_file).await?;
    partial_guard.disarm();

    emit(progress, ProgressEvent::Extracting);

    let bin_target = install_dir.join(&asset.bin);
    extract_asset(&staged_file, install_dir, &bin_target, &asset.file).await?;

    // Resolve where the binary actually landed. `.gz`/raw assets go straight to
    // `bin_target`, but tar.gz/zip archives often NEST it (e.g.
    // `pkg/bin/server`), so if it's not at the expected path, search the
    // extracted tree for a file named `asset.bin`. Without this the install
    // "succeeds" but leaves a dangling symlink → the pill shows "Binary
    // missing" and LSP silently never works.
    let bin_name = Path::new(&asset.bin)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(asset.bin.as_str())
        .to_string();
    let resolved = if std::fs::metadata(&bin_target).is_ok() {
        bin_target.clone()
    } else {
        find_binary_in_tree(install_dir, &bin_name)
            .ok_or_else(|| InstallError::BinaryNotFound(bin_name.clone()))?
    };

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        // The binary MUST be executable — chmod and treat failure as a real
        // install error (not the old best-effort skip).
        let mut perms = std::fs::metadata(&resolved)?.permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&resolved, perms)?;
    }

    // Final gate: confirm the binary is really there before we register it.
    if std::fs::metadata(&resolved).is_err() {
        return Err(InstallError::BinaryNotFound(resolved.display().to_string()));
    }

    // Drop the staged download once extraction has succeeded.
    let _ = tokio::fs::remove_file(&staged_file).await;

    Ok(resolved)
}

pub(super) fn safe_asset_name(file: &str) -> Result<&str, InstallError> {
    if file.is_empty()
        || file == "."
        || file == ".."
        || file.contains('/')
        || file.contains('\\')
    {
        return Err(InstallError::ParseManifest(format!(
            "release asset must be a file name, got `{file}`"
        )));
    }
    Ok(file)
}

pub(super) fn download_percent(downloaded: u64, total: Option<u64>) -> Option<u8> {
    let total = total.filter(|total| *total > 0)?;
    Some(
        downloaded
            .saturating_mul(100)
            .checked_div(total)
            .unwrap_or(0)
            .min(100) as u8,
    )
}

/// Cancellation-safe cleanup for an incomplete HTTP body. The guard is
/// declared before the open file, so Rust drops the file handle first and the
/// removal also works on Windows.
pub(super) struct RemovePartialOnDrop {
    path: PathBuf,
    armed: bool,
}

impl RemovePartialOnDrop {
    pub(super) fn new(path: PathBuf) -> Self {
        Self { path, armed: true }
    }

    pub(super) fn disarm(&mut self) {
        self.armed = false;
    }
}

impl Drop for RemovePartialOnDrop {
    fn drop(&mut self) {
        if self.armed {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// Recursively search `dir` for a file named `name` (the extracted binary may
/// be nested inside an archive's directory layout). Returns the first match.
pub(super) fn find_binary_in_tree(dir: &Path, name: &str) -> Option<PathBuf> {
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = std::fs::read_dir(&current).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                // Skip the leftover download staging dir.
                if path.file_name().and_then(|n| n.to_str()) != Some("staging") {
                    stack.push(path);
                }
            } else if path.file_name().and_then(|n| n.to_str()) == Some(name) {
                return Some(path);
            }
        }
    }
    None
}

pub(super) fn github_asset_url(owner: &str, repo: &str, tag: &str, file: &str) -> String {
    format!("https://github.com/{owner}/{repo}/releases/download/{tag}/{file}")
}

pub(super) async fn extract_asset(
    staged: &Path,
    install_dir: &Path,
    bin_target: &Path,
    file_name: &str,
) -> Result<(), InstallError> {
    let lower = file_name.to_ascii_lowercase();
    let staged = staged.to_path_buf();
    let install_dir = install_dir.to_path_buf();
    let bin_target = bin_target.to_path_buf();

    // Archive crates are sync; hop to a blocking task so we don't stall the
    // tokio reactor on large extractions.
    tokio::task::spawn_blocking(move || -> Result<(), InstallError> {
        if lower.ends_with(".tar.gz") || lower.ends_with(".tgz") {
            extract_tar_gz(&staged, &install_dir)
        } else if lower.ends_with(".tar.xz") || lower.ends_with(".txz") {
            extract_tar_xz(&staged, &install_dir)
        } else if lower.ends_with(".tar.zst") || lower.ends_with(".tzst") {
            extract_tar_zstd(&staged, &install_dir)
        } else if lower.ends_with(".tar.bz2") || lower.ends_with(".tbz2") {
            extract_tar_bzip2(&staged, &install_dir)
        } else if lower.ends_with(".tar") {
            extract_tar(&staged, &install_dir)
        } else if lower.ends_with(".zip") || lower.ends_with(".vsix") {
            extract_zip(&staged, &install_dir)
        } else if lower.ends_with(".gz") {
            extract_gz_to(&staged, &bin_target)
        } else {
            // Treat as a raw binary; rename into the final location.
            if let Some(parent) = bin_target.parent() {
                std::fs::create_dir_all(parent)?;
            }
            std::fs::rename(&staged, &bin_target)?;
            Ok(())
        }
    })
    .await
    .map_err(|e| InstallError::ParseManifest(format!("extract join: {e}")))?
}

pub(super) fn extract_tar_gz(staged: &Path, out_dir: &Path) -> Result<(), InstallError> {
    let f = std::fs::File::open(staged)?;
    let gz = flate2::read::GzDecoder::new(f);
    let mut archive = tar::Archive::new(gz);
    archive.unpack(out_dir)?;
    Ok(())
}

pub(super) fn extract_tar_xz(staged: &Path, out_dir: &Path) -> Result<(), InstallError> {
    let file = std::fs::File::open(staged)?;
    let decoder = xz2::read::XzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(out_dir)?;
    Ok(())
}

pub(super) fn extract_tar_zstd(
    staged: &Path,
    out_dir: &Path,
) -> Result<(), InstallError> {
    let file = std::fs::File::open(staged)?;
    let decoder = zstd::stream::read::Decoder::new(file)?;
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(out_dir)?;
    Ok(())
}

pub(super) fn extract_tar_bzip2(
    staged: &Path,
    out_dir: &Path,
) -> Result<(), InstallError> {
    let file = std::fs::File::open(staged)?;
    let decoder = bzip2::read::BzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    archive.unpack(out_dir)?;
    Ok(())
}

pub(super) fn extract_tar(staged: &Path, out_dir: &Path) -> Result<(), InstallError> {
    let file = std::fs::File::open(staged)?;
    let mut archive = tar::Archive::new(file);
    archive.unpack(out_dir)?;
    Ok(())
}

pub(super) fn extract_zip(staged: &Path, out_dir: &Path) -> Result<(), InstallError> {
    let f = std::fs::File::open(staged)?;
    let mut zip =
        zip::ZipArchive::new(f).map_err(|e| InstallError::Zip(e.to_string()))?;
    for i in 0..zip.len() {
        let mut entry = zip
            .by_index(i)
            .map_err(|e| InstallError::Zip(e.to_string()))?;
        let Some(rel) = entry.enclosed_name() else {
            continue;
        };
        let target = out_dir.join(rel);
        if entry.is_dir() {
            std::fs::create_dir_all(&target)?;
            continue;
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut out = std::fs::File::create(&target)?;
        std::io::copy(&mut entry, &mut out)?;
    }
    Ok(())
}

pub(super) fn extract_gz_to(
    staged: &Path,
    bin_target: &Path,
) -> Result<(), InstallError> {
    use std::io::Read;
    let f = std::fs::File::open(staged)?;
    let mut gz = flate2::read::GzDecoder::new(f);
    if let Some(parent) = bin_target.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = std::fs::File::create(bin_target)?;
    let mut buf = [0u8; 64 * 1024];
    loop {
        let n = gz.read(&mut buf)?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut out, &buf[..n])?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// platform target resolution + asset matching
// ---------------------------------------------------------------------------

/// Return the Mason-style target string for the current host. We mirror
/// Mason's naming so a Mason-translated manifest matches without remapping.
pub fn current_target() -> &'static str {
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        return "linux_x64_gnu";
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        return "linux_arm64_gnu";
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        return "darwin_x64";
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        return "darwin_arm64";
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        return "win_x64";
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        return "win_arm64";
    }
    #[allow(unreachable_code)]
    {
        "unknown"
    }
}

pub fn pick_asset<'a>(
    assets: &'a [GithubAsset],
    target: &str,
) -> Option<&'a GithubAsset> {
    assets
        .iter()
        .enumerate()
        .filter_map(|(index, asset)| {
            target_compatibility_rank(target, &asset.target)
                .map(|rank| (rank, index, asset))
        })
        .min_by_key(|(rank, index, _)| (*rank, *index))
        .map(|(_, _, asset)| asset)
}

/// Lower is better. Never treat another ABI-specific build as compatible:
/// `linux_x64_musl` must not win on a GNU host merely because its architecture
/// matches. Mason's ABI-neutral (`linux_x64`) and family (`linux`, `unix`)
/// targets are safe fallbacks after an exact target.
pub(super) fn target_compatibility_rank(host: &str, candidate: &str) -> Option<u8> {
    if candidate == host {
        return Some(0);
    }

    let (platform, arch, abi) = split_target(host);
    let (candidate_platform, candidate_arch, candidate_abi) = split_target(candidate);
    if candidate_platform == platform
        && candidate_arch == arch
        && arch.is_some()
        && abi.is_some()
        && candidate_abi.is_none()
    {
        return Some(1);
    }
    if candidate == platform {
        return Some(2);
    }
    if candidate == "unix" && matches!(platform, "linux" | "darwin") {
        return Some(3);
    }
    if candidate.is_empty() {
        return Some(4);
    }
    None
}

pub(super) fn split_target(target: &str) -> (&str, Option<&str>, Option<&str>) {
    let mut parts = target.split('_');
    let platform = parts.next().unwrap_or(target);
    let arch = parts.next();
    let abi = parts.next();
    (platform, arch, abi)
}

/// Whether the manifest has an install strategy usable on this host.
pub fn supported_on_current_host(manifest: &ExtensionManifest) -> bool {
    match &manifest.install {
        InstallKind::GithubRelease { assets, .. } => pick_asset(assets, current_target())
            .is_some_and(|asset| {
                manifest.executables.is_empty()
                    || manifest
                        .run
                        .as_ref()
                        .and_then(|run| run.command.first())
                        .is_some_and(|primary| asset.executables.contains_key(primary))
            }),
        InstallKind::Npm { .. }
        | InstallKind::Pip { .. }
        | InstallKind::Cargo { .. }
        | InstallKind::Go { .. }
        | InstallKind::Gem { .. } => true,
    }
}
