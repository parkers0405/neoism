use super::*;

pub(super) fn declared_executable_names(
    manifest: &ExtensionManifest,
) -> Result<Vec<String>, InstallError> {
    let mut names = Vec::new();
    if let Some(primary) = manifest.run.as_ref().and_then(|run| run.command.first()) {
        names.push(primary.clone());
    }
    for name in &manifest.executables {
        if !names.contains(name) {
            names.push(name.clone());
        }
    }
    for name in &names {
        if !safe_path_component(name) {
            return Err(InstallError::ParseManifest(format!(
                "managed executable name must be one path component, got `{name}`"
            )));
        }
    }
    Ok(names)
}

pub(super) async fn resolve_installed_binaries(
    manifest: &ExtensionManifest,
    install_dir: &Path,
    primary_path: &Path,
    names: &[String],
) -> Result<Vec<(String, PathBuf)>, InstallError> {
    let primary_name = default_bin_name(manifest, primary_path);
    let names = if names.is_empty() {
        vec![primary_name.clone()]
    } else {
        names.to_vec()
    };
    let mut binaries = Vec::with_capacity(names.len());
    for name in names {
        let path = match &manifest.install {
            InstallKind::Npm { .. } => {
                npm_bin_path(&install_dir.join("node_modules").join(".bin"), &name)
            }
            InstallKind::Pip { .. } => venv_bin_path(&install_dir.join("venv"), &name),
            InstallKind::Cargo { .. } => cargo_bin_path(install_dir, &name),
            InstallKind::Go { .. } => go_bin_path(install_dir, &name),
            InstallKind::Gem { .. } => gem_shim_path(install_dir, &name),
            InstallKind::GithubRelease { assets, .. } => {
                let asset = pick_asset(assets, current_target()).ok_or_else(|| {
                    InstallError::NoAssetForTarget(current_target().to_string())
                })?;
                if let Some(relative) = asset.executables.get(&name) {
                    materialize_github_executable(install_dir, &name, relative).await?
                } else if name == primary_name {
                    primary_path.to_path_buf()
                } else {
                    return Err(InstallError::BinaryNotFound(format!(
                        "catalog declares `{name}` but the selected release asset has no executable mapping"
                    )));
                }
            }
        };
        validate_binary(&path).await?;
        binaries.push((name, path));
    }
    Ok(binaries)
}

pub(super) async fn materialize_github_executable(
    install_dir: &Path,
    public_name: &str,
    recipe: &str,
) -> Result<PathBuf, InstallError> {
    if let Some((interpreter, payload)) = interpreter_recipe(recipe) {
        let payload = resolve_release_payload(install_dir, payload).ok_or_else(|| {
            InstallError::BinaryNotFound(format!(
                "release payload `{payload}` for `{public_name}`"
            ))
        })?;
        let launcher = github_launcher_path(install_dir, public_name);
        write_interpreter_launcher(&launcher, interpreter, &payload).await?;
        return Ok(launcher);
    }
    resolve_release_payload(install_dir, recipe).ok_or_else(|| {
        InstallError::BinaryNotFound(format!(
            "release executable `{recipe}` for `{public_name}`"
        ))
    })
}

pub(super) fn resolve_release_payload(
    install_dir: &Path,
    relative: &str,
) -> Option<PathBuf> {
    let relative = relative.strip_prefix("exec:").unwrap_or(relative);
    if !safe_relative_payload(relative) {
        return None;
    }
    let candidate = install_dir.join(relative);
    if candidate.is_file() {
        return Some(candidate);
    }
    let name = Path::new(relative).file_name()?.to_str()?;
    find_binary_in_tree(install_dir, name)
}

pub(super) fn safe_relative_payload(relative: &str) -> bool {
    let path = Path::new(relative);
    !relative.is_empty()
        && !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, std::path::Component::Normal(_)))
}

pub(super) fn interpreter_recipe(recipe: &str) -> Option<(&'static str, &str)> {
    for (prefix, interpreter) in [
        ("dotnet:", "dotnet"),
        ("java-jar:", "java"),
        ("node:", "node"),
        ("ruby:", "ruby"),
        ("php:", "php"),
    ] {
        if let Some(payload) = recipe.strip_prefix(prefix) {
            return Some((interpreter, payload));
        }
    }
    None
}

pub(super) fn github_launcher_path(install_dir: &Path, name: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        install_dir.join("launchers").join(format!("{name}.cmd"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        install_dir.join("launchers").join(name)
    }
}

pub(super) async fn write_interpreter_launcher(
    launcher: &Path,
    interpreter: &str,
    payload: &Path,
) -> Result<(), InstallError> {
    let parent = launcher.parent().ok_or_else(|| {
        InstallError::ParseManifest(format!(
            "release launcher has no parent: {}",
            launcher.display()
        ))
    })?;
    tokio::fs::create_dir_all(parent).await?;

    #[cfg(not(target_os = "windows"))]
    let contents = {
        let interpreter_args = if interpreter == "java" { " -jar" } else { "" };
        let interpreter = shell_single_quote(interpreter);
        let payload = shell_single_quote(path_utf8(payload)?);
        format!("#!/bin/sh\nexec {interpreter}{interpreter_args} {payload} \"$@\"\n")
    };
    #[cfg(target_os = "windows")]
    let contents = {
        let payload = windows_batch_path(payload)?;
        let interpreter_args = if interpreter == "java" { " -jar" } else { "" };
        format!("@echo off\r\n{interpreter}{interpreter_args} \"{payload}\" %*\r\n")
    };

    tokio::fs::write(launcher, contents).await?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(launcher, std::fs::Permissions::from_mode(0o755))
            .await?;
    }
    validate_binary(launcher).await
}
