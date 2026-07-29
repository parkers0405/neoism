use super::*;

// ---------------------------------------------------------------------------
// Neoism-managed Node.js runtime
//
// npm-based language servers (vue, svelte, typescript, tailwind, eslint, the
// json/css/html trio, …) are the majority of installable servers. If the user
// has no system Node, every one of them fails with "npm not found". To make
// installs "just work" with zero setup — exactly how Zed ships its own private
// Node and uses it to `npm install` servers — we provision a pinned Node
// runtime on demand and drive npm through it, falling back to the system `npm`
// only if provisioning fails (so users who already have Node are never
// regressed).
// ---------------------------------------------------------------------------

/// Pinned Node LTS ("Jod", Node 22). A single source of truth for the version
/// directory, download URL, checksum manifest, and in-archive layout.
pub(crate) const NODE_VERSION: &str = "22.11.0";

/// Resolved Node binaries inside a provisioned version directory.
pub(crate) struct ManagedNode {
    /// The `node` / `node.exe` executable.
    pub node: PathBuf,
    /// `npm-cli.js` — invoked as `node <npm_cli> install …` so we never depend
    /// on the `npm`/`npm.cmd` shim.
    pub npm_cli: PathBuf,
    /// Directory to prepend to `PATH` so any child `node`/`node-gyp` resolves
    /// the managed toolchain first.
    pub bin_dir: PathBuf,
}

/// Node dist naming for the current host: `{os}` (linux/darwin/win), `{arch}`
/// (x64/arm64), and archive `{ext}` (tar.xz on unix, zip on windows).
struct NodeTarget {
    os: &'static str,
    arch: &'static str,
    ext: &'static str,
}

/// Map the running host to Node's dist naming, erroring on any platform Node
/// does not publish a prebuilt runtime for (32-bit x86/arm, exotic OSes).
fn current_node_target() -> Result<NodeTarget, InstallError> {
    node_target_for(std::env::consts::OS, std::env::consts::ARCH)
}

/// Pure mapping from Rust's `OS`/`ARCH` tokens to Node dist naming. Split from
/// `current_node_target` so it can be unit-tested for every platform, not just
/// the host the tests happen to run on.
fn node_target_for(os: &str, arch: &str) -> Result<NodeTarget, InstallError> {
    let os = match os {
        "linux" => "linux",
        "macos" => "darwin",
        "windows" => "win",
        other => return Err(unsupported_platform(other, arch)),
    };
    let arch = match arch {
        "x86_64" => "x64",
        "aarch64" => "arm64",
        other => return Err(unsupported_platform(os, other)),
    };
    let ext = if os == "win" { "zip" } else { "tar.xz" };
    Ok(NodeTarget { os, arch, ext })
}

fn unsupported_platform(os: &str, arch: &str) -> InstallError {
    InstallError::NoAssetForTarget(format!(
        "managed Node runtime for {os}-{arch}: no prebuilt Node for this platform; install Node manually"
    ))
}

/// `node-v22.11.0-linux-x64` — the archive stem and the extracted top-level dir.
fn node_archive_stem(target: &NodeTarget) -> String {
    format!("node-v{NODE_VERSION}-{}-{}", target.os, target.arch)
}

/// `node-v22.11.0-linux-x64.tar.xz`.
fn node_archive_filename(target: &NodeTarget) -> String {
    format!("{}.{}", node_archive_stem(target), target.ext)
}

fn node_dist_url(filename: &str) -> String {
    format!("https://nodejs.org/dist/v{NODE_VERSION}/{filename}")
}

fn node_shasums_url() -> String {
    format!("https://nodejs.org/dist/v{NODE_VERSION}/SHASUMS256.txt")
}

/// Resolve the concrete `node`/`npm-cli.js`/bin-dir paths inside a provisioned
/// version directory. `os` is the Node dist os token (linux/darwin/win). The
/// two layouts differ: unix keeps `node` under `bin/` and npm under
/// `lib/node_modules`, windows puts `node.exe` at the archive root and npm
/// under `node_modules`.
fn resolve_managed_node(version_dir: &Path, os: &str) -> ManagedNode {
    if os == "win" {
        ManagedNode {
            node: version_dir.join("node.exe"),
            npm_cli: version_dir
                .join("node_modules")
                .join("npm")
                .join("bin")
                .join("npm-cli.js"),
            // On Windows the runtime root *is* the bin dir (node.exe lives here).
            bin_dir: version_dir.to_path_buf(),
        }
    } else {
        ManagedNode {
            node: version_dir.join("bin").join("node"),
            npm_cli: version_dir
                .join("lib")
                .join("node_modules")
                .join("npm")
                .join("bin")
                .join("npm-cli.js"),
            bin_dir: version_dir.join("bin"),
        }
    }
}

/// Ensure a managed Node is present and return its resolved binaries.
///
/// Idempotent: if the pinned version dir already holds a working `node` +
/// `npm-cli.js` it returns immediately (no re-download). Otherwise it downloads
/// the dist archive, verifies its SHA-256 against `SHASUMS256.txt`, extracts to
/// a temp dir, smoke-tests `node --version`, then **atomically renames** the
/// extracted tree onto the final version dir. Concurrent provisions can't leave
/// a half-baked dir, and a lost rename race adopts the winner's install.
pub(crate) async fn ensure_managed_node(
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<ManagedNode, InstallError> {
    let target = current_node_target()?;
    let version_dir = crate::paths::node_dir().join(format!("v{NODE_VERSION}"));

    // Fast path: already provisioned and working.
    if let Some(managed) = existing_managed_node(&version_dir, target.os).await {
        return Ok(managed);
    }

    emit(
        progress,
        ProgressEvent::Waiting {
            status: format!("downloading Node {NODE_VERSION} runtime…"),
        },
    );

    let filename = node_archive_filename(&target);
    let url = node_dist_url(&filename);

    let node_root = crate::paths::node_dir();
    tokio::fs::create_dir_all(&node_root).await?;

    // Unique staging dir (pid + timestamp) so two concurrent provisions never
    // clobber each other. The guard removes it on any early return.
    let staging = node_root.join(format!(".staging-{}-{}", std::process::id(), now_ms()));
    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging).await?;
    let _staging_guard = RemoveDirOnDrop::new(staging.clone());

    // Download the dist archive (bounded timeouts + `.part` guard, shared with
    // the GitHub-release path).
    let archive = staging.join(&filename);
    download_to_file(&url, &archive, &format!("Node {NODE_VERSION}"), progress).await?;

    // Integrity: verify SHA-256 against the published SHASUMS256.txt. A network
    // hiccup fetching the manifest (or a filename that's somehow absent) is not
    // fatal — the post-extract `node --version` smoke test still rejects a
    // corrupt binary. TODO(node-checksum): make offline verification failures
    // hard errors when we want to guarantee no unverified binary is ever run.
    match fetch_shasums(&node_shasums_url()).await {
        Ok(shasums) => match find_sha_for(&shasums, &filename) {
            Some(expected) => verify_sha256(&archive, &expected, &filename).await?,
            None => {}
        },
        Err(_) => {}
    }

    // Extract into a temp dir inside staging, then locate the top-level dir.
    let extract_dir = staging.join("extract");
    tokio::fs::create_dir_all(&extract_dir).await?;
    extract_node_archive(&archive, &extract_dir, target.ext).await?;

    let extracted_top = extract_dir.join(node_archive_stem(&target));
    if tokio::fs::metadata(&extracted_top).await.is_err() {
        return Err(InstallError::BinaryNotFound(format!(
            "extracted Node directory missing: {}",
            extracted_top.display()
        )));
    }

    // Smoke-test the staged binary BEFORE publishing so a broken extract never
    // becomes the accepted version dir.
    let staged = resolve_managed_node(&extracted_top, target.os);
    verify_node_runs(&staged.node).await?;

    // Atomic publish: rename the extracted tree onto the version dir. Both live
    // under node_dir(), so this is a same-filesystem rename.
    match tokio::fs::rename(&extracted_top, &version_dir).await {
        Ok(()) => {}
        Err(err) => {
            // A concurrent provision may have published first (rename onto an
            // existing dir fails). If theirs is present and working, adopt it;
            // otherwise surface the real error.
            if existing_managed_node(&version_dir, target.os).await.is_none() {
                return Err(InstallError::Io(err));
            }
        }
    }

    Ok(resolve_managed_node(&version_dir, target.os))
}

/// Return the resolved runtime iff the version dir holds a working `node` +
/// `npm-cli.js` (existence checks plus a `node --version` smoke test).
async fn existing_managed_node(version_dir: &Path, os: &str) -> Option<ManagedNode> {
    let managed = resolve_managed_node(version_dir, os);
    if tokio::fs::metadata(&managed.node).await.is_err() {
        return None;
    }
    if tokio::fs::metadata(&managed.npm_cli).await.is_err() {
        return None;
    }
    if verify_node_runs(&managed.node).await.is_err() {
        return None;
    }
    Some(managed)
}

/// Run `<node> --version`, erroring unless it exits successfully. Cheap gate
/// against a truncated download or a mismatched-platform binary.
async fn verify_node_runs(node: &Path) -> Result<(), InstallError> {
    let output = Command::new(node)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .output()
        .await?;
    if !output.status.success() {
        return Err(InstallError::BinaryNotFound(format!(
            "managed node failed `--version`: {}",
            node.display()
        )));
    }
    Ok(())
}

/// Build the `node <npm-cli.js> …` base command with the managed bin dir
/// prepended to PATH. Callers append `install --prefix … <specs>`.
pub(crate) fn managed_npm_command(managed: &ManagedNode) -> Command {
    let mut cmd = Command::new(&managed.node);
    cmd.arg(&managed.npm_cli);

    let mut paths = vec![managed.bin_dir.clone()];
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    let mut unique = Vec::new();
    for path in paths {
        if !unique.contains(&path) {
            unique.push(path);
        }
    }
    if let Ok(path) = std::env::join_paths(unique) {
        cmd.env("PATH", path);
    }
    cmd
}

/// Unpack the Node dist archive (tar.xz on unix, zip on windows) into `out_dir`.
/// Reuses release.rs's extractors; runs on a blocking task since the archive
/// crates are sync.
async fn extract_node_archive(
    archive: &Path,
    out_dir: &Path,
    ext: &str,
) -> Result<(), InstallError> {
    let archive = archive.to_path_buf();
    let out_dir = out_dir.to_path_buf();
    let is_zip = ext == "zip";
    tokio::task::spawn_blocking(move || -> Result<(), InstallError> {
        std::fs::create_dir_all(&out_dir)?;
        if is_zip {
            extract_zip(&archive, &out_dir)
        } else {
            extract_tar_xz(&archive, &out_dir)
        }
    })
    .await
    .map_err(|e| InstallError::ParseManifest(format!("node extract join: {e}")))?
}

/// Fetch the plaintext `SHASUMS256.txt` for the pinned Node version.
async fn fetch_shasums(url: &str) -> Result<String, InstallError> {
    let client = reqwest::Client::builder()
        .connect_timeout(DOWNLOAD_CONNECT_TIMEOUT)
        .timeout(DOWNLOAD_REQUEST_TIMEOUT)
        .user_agent(concat!("neoism/", env!("CARGO_PKG_VERSION")))
        .build()
        .map_err(|e| InstallError::Network(e.to_string()))?;
    let text = tokio::time::timeout(DOWNLOAD_CONNECT_TIMEOUT, client.get(url).send())
        .await
        .map_err(|_| InstallError::TimedOut {
            tool: "Node SHASUMS256.txt".to_string(),
            seconds: DOWNLOAD_CONNECT_TIMEOUT.as_secs(),
        })?
        .map_err(|e| InstallError::Network(e.to_string()))?
        .error_for_status()
        .map_err(|e| InstallError::Network(e.to_string()))?
        .text()
        .await
        .map_err(|e| InstallError::Network(e.to_string()))?;
    Ok(text)
}

/// Find the hex SHA-256 for `filename` in a `SHASUMS256.txt` body. Each line is
/// `<hex>  <filename>`.
fn find_sha_for(shasums: &str, filename: &str) -> Option<String> {
    shasums.lines().find_map(|line| {
        let mut it = line.split_whitespace();
        let hash = it.next()?;
        let name = it.next()?;
        (name == filename).then(|| hash.to_string())
    })
}

/// Verify the SHA-256 of `file` equals `expected_hex` (case-insensitive).
async fn verify_sha256(
    file: &Path,
    expected_hex: &str,
    label: &str,
) -> Result<(), InstallError> {
    let path = file.to_path_buf();
    let expected = expected_hex.to_ascii_lowercase();
    let label = label.to_string();
    tokio::task::spawn_blocking(move || -> Result<(), InstallError> {
        use sha2::{Digest, Sha256};
        use std::io::Read;
        let mut f = std::fs::File::open(&path)?;
        let mut hasher = Sha256::new();
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = f.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
        }
        let actual = hex_encode(&hasher.finalize());
        if actual != expected {
            return Err(InstallError::Network(format!(
                "checksum mismatch for {label}: expected {expected}, got {actual}"
            )));
        }
        Ok(())
    })
    .await
    .map_err(|e| InstallError::ParseManifest(format!("node hash join: {e}")))?
}

fn hex_encode(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Cancellation-safe cleanup for the Node staging directory. Mirrors
/// `RemovePartialOnDrop`, but removes a whole tree.
struct RemoveDirOnDrop {
    path: PathBuf,
}

impl RemoveDirOnDrop {
    fn new(path: PathBuf) -> Self {
        Self { path }
    }
}

impl Drop for RemoveDirOnDrop {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_platform_to_node_dist_naming() {
        let t = node_target_for("linux", "x86_64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("linux", "x64", "tar.xz"));
        let filename = node_archive_filename(&t);
        assert_eq!(filename, format!("node-v{NODE_VERSION}-linux-x64.tar.xz"));
        assert_eq!(
            node_dist_url(&filename),
            format!(
                "https://nodejs.org/dist/v{NODE_VERSION}/node-v{NODE_VERSION}-linux-x64.tar.xz"
            )
        );

        let t = node_target_for("linux", "aarch64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("linux", "arm64", "tar.xz"));

        let t = node_target_for("macos", "x86_64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("darwin", "x64", "tar.xz"));
        assert_eq!(
            node_archive_filename(&t),
            format!("node-v{NODE_VERSION}-darwin-x64.tar.xz")
        );

        let t = node_target_for("macos", "aarch64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("darwin", "arm64", "tar.xz"));

        let t = node_target_for("windows", "x86_64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("win", "x64", "zip"));
        assert_eq!(
            node_archive_filename(&t),
            format!("node-v{NODE_VERSION}-win-x64.zip")
        );

        let t = node_target_for("windows", "aarch64").unwrap();
        assert_eq!((t.os, t.arch, t.ext), ("win", "arm64", "zip"));
    }

    #[test]
    fn rejects_unsupported_platforms() {
        // 32-bit and exotic hosts have no prebuilt Node dist.
        assert!(node_target_for("linux", "x86").is_err());
        assert!(node_target_for("linux", "arm").is_err());
        assert!(node_target_for("freebsd", "x86_64").is_err());
        assert!(node_target_for("windows", "x86").is_err());
    }

    #[test]
    fn resolves_in_archive_binary_paths() {
        let root = Path::new("/managed/node/v22.11.0");

        // Unix: node under bin/, npm under lib/node_modules, bin dir = bin/.
        for os in ["linux", "darwin"] {
            let m = resolve_managed_node(root, os);
            assert_eq!(m.node, root.join("bin").join("node"));
            assert_eq!(
                m.npm_cli,
                root.join("lib")
                    .join("node_modules")
                    .join("npm")
                    .join("bin")
                    .join("npm-cli.js")
            );
            assert_eq!(m.bin_dir, root.join("bin"));
        }

        // Windows: node.exe at root, npm under node_modules, bin dir = root.
        let win = resolve_managed_node(root, "win");
        assert_eq!(win.node, root.join("node.exe"));
        assert_eq!(
            win.npm_cli,
            root.join("node_modules")
                .join("npm")
                .join("bin")
                .join("npm-cli.js")
        );
        assert_eq!(win.bin_dir, root.to_path_buf());
    }

    #[test]
    fn parses_shasums_manifest() {
        let shasums = "\
aaaaaaaa11111111  node-v22.11.0-linux-x64.tar.gz
bbbbbbbb22222222  node-v22.11.0-linux-x64.tar.xz
cccccccc33333333  node-v22.11.0-win-x64.zip
";
        assert_eq!(
            find_sha_for(shasums, "node-v22.11.0-linux-x64.tar.xz").as_deref(),
            Some("bbbbbbbb22222222")
        );
        assert_eq!(
            find_sha_for(shasums, "node-v22.11.0-win-x64.zip").as_deref(),
            Some("cccccccc33333333")
        );
        assert_eq!(find_sha_for(shasums, "node-v22.11.0-darwin-arm64.tar.xz"), None);
    }

    #[test]
    fn hex_encode_is_lowercase_padded() {
        assert_eq!(hex_encode(&[0x00, 0x0f, 0xab, 0xff]), "000fabff");
    }
}
