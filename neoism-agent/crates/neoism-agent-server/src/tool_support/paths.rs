use std::path::{Component, Path, PathBuf};

use anyhow::Context;

use super::ToolContext;

pub(super) fn existing_project_path(
    context: &ToolContext,
    raw: &str,
) -> anyhow::Result<PathBuf> {
    let base = context.cwd.canonicalize().with_context(|| {
        format!(
            "failed to resolve project directory {}",
            context.cwd.display()
        )
    })?;
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    let path = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve path {}", candidate.display()))?;
    if !path.starts_with(&base) {
        context.ensure_explicit_allowed(
            "external_directory",
            &external_directory_pattern(&path, path.is_dir()),
        )?;
    }
    Ok(path)
}

pub(super) fn project_path_for_write(
    context: &ToolContext,
    raw: &str,
) -> anyhow::Result<PathBuf> {
    let base = context.cwd.canonicalize().with_context(|| {
        format!(
            "failed to resolve project directory {}",
            context.cwd.display()
        )
    })?;
    let candidate = if Path::new(raw).is_absolute() {
        PathBuf::from(raw)
    } else {
        base.join(raw)
    };
    let candidate = normalize_absolute_path(&candidate)?;
    let mut ancestor = candidate.as_path();
    while !ancestor.exists() {
        ancestor = ancestor.parent().ok_or_else(|| {
            anyhow::anyhow!("path {} has no existing ancestor", candidate.display())
        })?;
    }
    let ancestor = ancestor.canonicalize().with_context(|| {
        format!("failed to resolve existing ancestor {}", ancestor.display())
    })?;
    if !ancestor.is_dir() && ancestor != candidate {
        anyhow::bail!(
            "cannot create {} below non-directory {}",
            candidate.display(),
            ancestor.display()
        );
    }
    let suffix = candidate
        .strip_prefix(
            candidate
                .ancestors()
                .find(|path| path.exists())
                .expect("existing ancestor was found"),
        )
        .with_context(|| format!("failed to resolve path {}", candidate.display()))?;
    let resolved = if suffix.as_os_str().is_empty() {
        ancestor
    } else {
        ancestor.join(suffix)
    };
    if !resolved.starts_with(&base) {
        context.ensure_explicit_allowed(
            "external_directory",
            &external_directory_pattern(&resolved, false),
        )?;
    }
    Ok(resolved)
}

pub(super) fn directory_entries(path: &Path) -> anyhow::Result<Vec<String>> {
    let root = path
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", path.display()))?;
    let mut entries = std::fs::read_dir(&root)
        .with_context(|| format!("failed to list {}", path.display()))?
        .filter_map(|entry| {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => return Some(Err(error)),
            };
            let resolved = match entry.path().canonicalize() {
                Ok(resolved) if resolved.starts_with(&root) => resolved,
                Ok(_) | Err(_) => return None,
            };
            Some((|| {
                let file_type = resolved.metadata()?.file_type();
                let mut name = entry.file_name().to_string_lossy().to_string();
                if file_type.is_dir() {
                    name.push('/');
                }
                Ok((file_type.is_dir(), name))
            })())
        })
        .collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by(|left, right| (!left.0, &left.1).cmp(&(!right.0, &right.1)));
    Ok(entries.into_iter().map(|(_, name)| name).collect())
}

fn normalize_absolute_path(path: &Path) -> anyhow::Result<PathBuf> {
    debug_assert!(path.is_absolute());
    let mut normalized = PathBuf::new();
    let mut normal_components = 0usize;
    for component in path.components() {
        match component {
            Component::Prefix(prefix) => normalized.push(prefix.as_os_str()),
            Component::RootDir => {
                normalized.push(component.as_os_str());
                normal_components = 0;
            }
            Component::CurDir => {}
            Component::Normal(name) => {
                normalized.push(name);
                normal_components += 1;
            }
            Component::ParentDir => {
                if normal_components == 0 {
                    anyhow::bail!("path {} escapes its filesystem root", path.display());
                }
                normalized.pop();
                normal_components -= 1;
            }
        }
    }
    Ok(normalized)
}

pub(super) fn display_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf()))
        .unwrap_or(path)
        .to_string_lossy()
        .to_string()
}

pub(super) fn external_directory_pattern(path: &Path, directory: bool) -> String {
    let dir = if directory {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    format!("{}/*", dir.display())
}

pub(super) fn truncate_line(line: &str) -> String {
    const MAX: usize = 2000;
    if line.chars().count() <= MAX {
        return line.to_string();
    }
    line.chars().take(MAX).collect::<String>() + "..."
}
