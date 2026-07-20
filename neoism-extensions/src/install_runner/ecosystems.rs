use super::*;

// ---------------------------------------------------------------------------
// npm
// ---------------------------------------------------------------------------

pub(super) async fn install_npm(
    install_dir: &Path,
    pkg: &str,
    version: &str,
    extra_packages: &[String],
    bin_names: &[String],
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    tokio::fs::create_dir_all(install_dir).await?;

    let package_specs = npm_package_specs(pkg, version, extra_packages)?;
    let mut cmd = host_command("npm");
    cmd.arg("install")
        .arg("--prefix")
        .arg(install_dir)
        .arg("--no-audit")
        .arg("--no-fund")
        .arg("--loglevel=info")
        .args(&package_specs)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::MissingTool("npm"))
        }
        Err(e) => return Err(InstallError::Io(e)),
    };

    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, tail) =
        wait_for_command(&mut child, stdout, stderr, progress, "npm").await?;
    if !status.success() {
        return Err(InstallError::Process {
            exit: status,
            tail: tail.join("\n"),
        });
    }

    resolve_npm_bin(install_dir, pkg, bin_names).await
}

pub(super) fn npm_package_specs(
    package: &str,
    version: &str,
    extra_packages: &[String],
) -> Result<Vec<String>, InstallError> {
    validate_package_argument("npm", package)?;
    validate_package_argument("npm version", version)?;
    let mut specs = vec![format!("{package}@{version}")];
    for extra in extra_packages {
        validate_package_argument("npm", extra)?;
        if !specs.iter().any(|existing| existing == extra) {
            specs.push(extra.clone());
        }
    }
    Ok(specs)
}

pub(super) fn validate_package_argument(
    kind: &'static str,
    value: &str,
) -> Result<(), InstallError> {
    if value.is_empty()
        || value.starts_with('-')
        || value.chars().any(char::is_whitespace)
        || value.chars().any(char::is_control)
    {
        return Err(InstallError::InvalidPackageSpec {
            kind,
            value: value.to_string(),
        });
    }
    Ok(())
}

pub(super) async fn resolve_npm_bin(
    install_dir: &Path,
    pkg: &str,
    bin_names: &[String],
) -> Result<PathBuf, InstallError> {
    let bin_dir = install_dir.join("node_modules").join(".bin");
    if let Some(first) = bin_names.first() {
        return Ok(npm_bin_path(&bin_dir, first));
    }

    // Fallback: read package.json's `bin` field.
    let pkg_json = install_dir
        .join("node_modules")
        .join(pkg)
        .join("package.json");
    let bytes = tokio::fs::read(&pkg_json).await?;
    let value: serde_json::Value = serde_json::from_slice(&bytes)
        .map_err(|e| InstallError::ParseManifest(format!("package.json: {e}")))?;
    let bin_field = value.get("bin").ok_or_else(|| {
        InstallError::ParseManifest(format!(
            "package.json missing `bin`: {}",
            pkg_json.display()
        ))
    })?;
    let first_name: String = match bin_field {
        serde_json::Value::String(_) => pkg.rsplit('/').next().unwrap_or(pkg).to_string(),
        serde_json::Value::Object(map) => {
            map.keys().next().cloned().ok_or_else(|| {
                InstallError::ParseManifest("empty `bin` map".to_string())
            })?
        }
        _ => {
            return Err(InstallError::ParseManifest(
                "`bin` must be string or object".to_string(),
            ))
        }
    };
    Ok(npm_bin_path(&bin_dir, &first_name))
}

pub(super) fn npm_bin_path(bin_dir: &Path, name: &str) -> PathBuf {
    #[cfg(windows)]
    {
        bin_dir.join(format!("{name}.cmd"))
    }
    #[cfg(not(windows))]
    {
        bin_dir.join(name)
    }
}

// ---------------------------------------------------------------------------
// pip
// ---------------------------------------------------------------------------

pub(super) async fn install_pip(
    install_dir: &Path,
    pkg: &str,
    version: &str,
    bin_names: &[String],
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    tokio::fs::create_dir_all(install_dir).await?;

    let venv_dir = install_dir.join("venv");
    let mut venv_cmd = host_command("python3");
    venv_cmd
        .arg("-m")
        .arg("venv")
        .arg(&venv_dir)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut venv_child = match venv_cmd.spawn() {
        Ok(c) => c,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::MissingTool("python3"))
        }
        Err(e) => return Err(InstallError::Io(e)),
    };
    let venv_stdout = venv_child.stdout.take();
    let venv_stderr = venv_child.stderr.take();
    let (venv_status, venv_tail) = wait_for_command(
        &mut venv_child,
        venv_stdout,
        venv_stderr,
        progress,
        "python venv",
    )
    .await?;
    if !venv_status.success() {
        return Err(InstallError::Process {
            exit: venv_status,
            tail: venv_tail.join("\n"),
        });
    }

    let pip_bin = venv_pip_path(&venv_dir);
    let pkg_spec = format!("{pkg}=={version}");
    let mut pip_cmd = host_command(&pip_bin);
    pip_cmd
        .arg("install")
        .arg(&pkg_spec)
        .arg("--no-input")
        .arg("--progress-bar")
        .arg("off")
        .arg("--disable-pip-version-check")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut pip_child = pip_cmd.spawn()?;
    let pip_stdout = pip_child.stdout.take();
    let pip_stderr = pip_child.stderr.take();
    let (pip_status, pip_tail) =
        wait_for_command(&mut pip_child, pip_stdout, pip_stderr, progress, "pip").await?;
    if !pip_status.success() {
        return Err(InstallError::Process {
            exit: pip_status,
            tail: pip_tail.join("\n"),
        });
    }

    let bin_name = bin_names
        .first()
        .cloned()
        .unwrap_or_else(|| pkg.to_string());
    Ok(venv_bin_path(&venv_dir, &bin_name))
}

pub(super) fn venv_pip_path(venv: &Path) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv.join("Scripts").join("pip.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv.join("bin").join("pip")
    }
}

pub(super) fn venv_bin_path(venv: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        venv.join("Scripts").join(format!("{name}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        venv.join("bin").join(name)
    }
}

// ---------------------------------------------------------------------------
// cargo
// ---------------------------------------------------------------------------

/// Install a Rust-distributed tool into Neoism's private extension prefix.
/// `--root` is important: extension installation must never modify the user's
/// global Cargo bin directory or overwrite an unrelated executable.
pub(super) async fn install_cargo(
    install_dir: &Path,
    crate_name: &str,
    version: &str,
    features: &[String],
    bin_names: &[String],
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    tokio::fs::create_dir_all(install_dir).await?;

    let mut cmd = host_command("cargo");
    cmd.arg("install")
        .arg("--root")
        .arg(install_dir)
        .arg("--version")
        .arg(version)
        .arg(crate_name);
    if !features.is_empty() {
        cmd.arg("--features").arg(features.join(","));
    }
    cmd.kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::MissingTool("cargo"))
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, tail) =
        wait_for_command(&mut child, stdout, stderr, progress, "cargo").await?;
    if !status.success() {
        return Err(InstallError::Process {
            exit: status,
            tail: tail.join("\n"),
        });
    }

    let bin_name = bin_names
        .first()
        .cloned()
        .unwrap_or_else(|| crate_name.to_string());
    Ok(cargo_bin_path(install_dir, &bin_name))
}

pub(super) fn cargo_bin_path(install_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        install_dir.join("bin").join(format!("{name}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_dir.join("bin").join(name)
    }
}

// ---------------------------------------------------------------------------
// go
// ---------------------------------------------------------------------------

/// Install a Go-distributed tool into Neoism's private bin directory. Go's
/// module/build caches may be shared with the host, but `GOBIN` is always
/// isolated so this never replaces a user's globally installed executable.
pub(super) async fn install_go(
    install_dir: &Path,
    package: &str,
    version: &str,
    bin_names: &[String],
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    validate_package_argument("Go", package)?;
    validate_package_argument("Go version", version)?;
    let bin_dir = install_dir.join("bin");
    tokio::fs::create_dir_all(&bin_dir).await?;

    let package_spec = format!("{package}@{version}");
    let mut cmd = host_command("go");
    cmd.arg("install")
        .arg(package_spec)
        .env("GOBIN", &bin_dir)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::MissingTool("go"))
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, tail) =
        wait_for_command(&mut child, stdout, stderr, progress, "go install").await?;
    if !status.success() {
        return Err(InstallError::Process {
            exit: status,
            tail: tail.join("\n"),
        });
    }

    let bin_name = bin_names
        .first()
        .cloned()
        .or_else(|| package.rsplit('/').next().map(str::to_string))
        .ok_or_else(|| InstallError::InvalidPackageSpec {
            kind: "Go",
            value: package.to_string(),
        })?;
    Ok(go_bin_path(install_dir, &bin_name))
}

pub(super) fn go_bin_path(install_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        install_dir.join("bin").join(format!("{name}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_dir.join("bin").join(name)
    }
}

// ---------------------------------------------------------------------------
// Ruby gems
// ---------------------------------------------------------------------------

/// Install gems into a private GEM_HOME and expose a launcher that restores
/// that GEM_HOME every time the tool starts. Linking RubyGems' generated
/// wrapper directly is insufficient: after Neoism restarts, the process would
/// otherwise search the user's global gem paths and fail to load the package.
pub(super) async fn install_gem(
    install_dir: &Path,
    package: &str,
    version: &str,
    extra_packages: &[String],
    bin_names: &[String],
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<PathBuf, InstallError> {
    validate_package_argument("Ruby gem", package)?;
    validate_package_argument("Ruby gem version", version)?;
    let gem_home = install_dir.join("gem-home");
    let generated_bin_dir = install_dir.join("gem-wrappers");
    tokio::fs::create_dir_all(&gem_home).await?;
    tokio::fs::create_dir_all(&generated_bin_dir).await?;

    run_gem_install(
        &gem_home,
        &generated_bin_dir,
        package,
        Some(version),
        progress,
    )
    .await?;
    for extra in extra_packages {
        let (name, extra_version) = split_gem_package_spec(extra)?;
        run_gem_install(&gem_home, &generated_bin_dir, name, extra_version, progress)
            .await?;
    }

    let bin_names = if bin_names.is_empty() {
        vec![package.to_string()]
    } else {
        bin_names.to_vec()
    };
    let mut primary_shim = None;
    for bin_name in bin_names {
        if !safe_path_component(&bin_name) {
            return Err(InstallError::InvalidPackageSpec {
                kind: "Ruby executable",
                value: bin_name,
            });
        }
        let generated_wrapper = gem_generated_wrapper_path(&generated_bin_dir, &bin_name);
        validate_binary(&generated_wrapper).await?;
        let shim = gem_shim_path(install_dir, &bin_name);
        write_gem_shim(&shim, &gem_home, &generated_wrapper).await?;
        if primary_shim.is_none() {
            primary_shim = Some(shim);
        }
    }
    primary_shim.ok_or_else(|| InstallError::BinaryNotFound(package.to_string()))
}

pub(super) async fn run_gem_install(
    gem_home: &Path,
    generated_bin_dir: &Path,
    package: &str,
    version: Option<&str>,
    progress: &UnboundedSender<ProgressEvent>,
) -> Result<(), InstallError> {
    validate_package_argument("Ruby gem", package)?;
    if let Some(version) = version {
        validate_package_argument("Ruby gem version", version)?;
    }

    let mut cmd = host_command("gem");
    cmd.arg("install")
        .arg("--install-dir")
        .arg(gem_home)
        .arg("--bindir")
        .arg(generated_bin_dir)
        .arg("--no-document");
    if let Some(version) = version {
        cmd.arg("--version").arg(version);
    }
    cmd.arg(package)
        .env("GEM_HOME", gem_home)
        .env("GEM_PATH", gem_home)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(InstallError::MissingTool("gem"))
        }
        Err(error) => return Err(InstallError::Io(error)),
    };
    let stdout = child.stdout.take();
    let stderr = child.stderr.take();
    let (status, tail) =
        wait_for_command(&mut child, stdout, stderr, progress, "gem install").await?;
    if !status.success() {
        return Err(InstallError::Process {
            exit: status,
            tail: tail.join("\n"),
        });
    }
    Ok(())
}

pub(super) fn split_gem_package_spec(
    spec: &str,
) -> Result<(&str, Option<&str>), InstallError> {
    validate_package_argument("Ruby gem", spec)?;
    let Some((name, version)) = spec.rsplit_once('@') else {
        return Ok((spec, None));
    };
    if name.is_empty() || version.is_empty() {
        return Err(InstallError::InvalidPackageSpec {
            kind: "Ruby gem",
            value: spec.to_string(),
        });
    }
    validate_package_argument("Ruby gem", name)?;
    validate_package_argument("Ruby gem version", version)?;
    Ok((name, Some(version)))
}

pub(super) fn gem_generated_wrapper_path(bin_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        bin_dir.join(format!("{name}.bat"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        bin_dir.join(name)
    }
}

pub(super) fn gem_shim_path(install_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        install_dir.join("bin").join(format!("{name}.cmd"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_dir.join("bin").join(name)
    }
}

pub(super) async fn write_gem_shim(
    shim: &Path,
    gem_home: &Path,
    generated_wrapper: &Path,
) -> Result<(), InstallError> {
    let parent = shim.parent().ok_or_else(|| {
        InstallError::ParseManifest(format!("gem shim has no parent: {}", shim.display()))
    })?;
    tokio::fs::create_dir_all(parent).await?;

    #[cfg(not(target_os = "windows"))]
    let contents = {
        let gem_home = shell_single_quote(path_utf8(gem_home)?);
        let wrapper = shell_single_quote(path_utf8(generated_wrapper)?);
        format!(
            "#!/bin/sh\nGEM_HOME={gem_home}\nGEM_PATH={gem_home}\nexport GEM_HOME GEM_PATH\nexec ruby {wrapper} \"$@\"\n"
        )
    };
    #[cfg(target_os = "windows")]
    let contents = {
        let gem_home = windows_batch_path(gem_home)?;
        let wrapper = windows_batch_path(generated_wrapper)?;
        format!(
            "@echo off\r\nset \"GEM_HOME={gem_home}\"\r\nset \"GEM_PATH={gem_home}\"\r\nruby \"{wrapper}\" %*\r\n"
        )
    };

    tokio::fs::write(shim, contents).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(shim, std::fs::Permissions::from_mode(0o755)).await?;
    }
    validate_binary(shim).await
}

pub(super) fn path_utf8(path: &Path) -> Result<&str, InstallError> {
    path.to_str().ok_or_else(|| {
        InstallError::ParseManifest(format!(
            "path is not valid UTF-8: {}",
            path.display()
        ))
    })
}

#[cfg(not(target_os = "windows"))]
pub(super) fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(target_os = "windows")]
pub(super) fn windows_batch_path(path: &Path) -> Result<String, InstallError> {
    let value = path_utf8(path)?;
    if value.contains(['\r', '\n', '"', '%']) {
        return Err(InstallError::ParseManifest(format!(
            "path cannot be represented safely in a batch launcher: {}",
            path.display()
        )));
    }
    Ok(value.to_string())
}
