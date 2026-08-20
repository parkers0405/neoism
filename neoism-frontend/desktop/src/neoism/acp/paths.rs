use std::path::{Component, Path, PathBuf};

use super::client::AcpRpcError;

pub(super) fn normalize_existing_dir(path: &Path) -> std::io::Result<PathBuf> {
    let path = canonicalize_path(path)?;
    if path.is_dir() {
        Ok(path)
    } else {
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "path is not a directory",
        ))
    }
}

pub(super) fn workspace_path(
    cwd: &Path,
    raw: Option<&str>,
) -> Result<PathBuf, AcpRpcError> {
    let raw = raw.ok_or_else(|| AcpRpcError {
        code: -32602,
        message: "ACP request missing path".to_string(),
    })?;
    let path = normalize_absolute_path(Path::new(raw)).ok_or_else(|| AcpRpcError {
        code: -32602,
        message: format!("ACP path must be absolute: {raw}"),
    })?;
    if !path.is_absolute() {
        return Err(AcpRpcError {
            code: -32602,
            message: format!("ACP path must be absolute: {raw}"),
        });
    }

    let root = canonicalize_path(cwd).map_err(|err| AcpRpcError {
        code: -32000,
        message: format!("Could not resolve workspace {}: {err}", cwd.display()),
    })?;
    let existing_ancestor = nearest_existing_ancestor(
        path.parent().unwrap_or(path.as_path()),
    )
    .ok_or_else(|| AcpRpcError {
        code: -32000,
        message: format!("Could not resolve parent for {}", path.display()),
    })?;
    let existing_ancestor =
        canonicalize_path(&existing_ancestor).map_err(|err| AcpRpcError {
            code: -32000,
            message: format!(
                "Could not resolve ancestor {}: {err}",
                existing_ancestor.display()
            ),
        })?;
    if !existing_ancestor.starts_with(&root) {
        return Err(AcpRpcError {
            code: -32000,
            message: format!(
                "ACP file access outside workspace is blocked: {}",
                path.display()
            ),
        });
    }
    Ok(path)
}

pub(super) fn canonicalize_path(path: &Path) -> std::io::Result<PathBuf> {
    #[cfg(windows)]
    {
        dunce::canonicalize(path)
    }
    #[cfg(not(windows))]
    {
        path.canonicalize()
    }
}

pub(super) fn normalize_absolute_path(path: &Path) -> Option<PathBuf> {
    if !path.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => out.push(prefix.as_os_str()),
            Component::RootDir => out.push(component.as_os_str()),
            Component::CurDir => {}
            Component::ParentDir => {
                out.pop();
            }
            Component::Normal(part) => out.push(part),
        }
    }
    Some(out)
}

pub(super) fn nearest_existing_ancestor(path: &Path) -> Option<PathBuf> {
    let mut current = path;
    loop {
        if current.exists() {
            return Some(current.to_path_buf());
        }
        current = current.parent()?;
    }
}
