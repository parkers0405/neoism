//! Read-only, session-independent server directory chooser.
use crate::{caller::CallerClaims, error::ApiError};
use axum::{extract::Query, Extension, Json};
use serde::{Deserialize, Serialize};

#[derive(Default, Deserialize)]
pub(crate) struct DirectoryQuery {
    pub path: Option<String>,
}
#[derive(Serialize)]
pub(crate) struct DirectoryEntry {
    name: String,
    path: String,
}
#[derive(Serialize)]
pub(crate) struct DirectoryListing {
    path: String,
    parent: Option<String>,
    entries: Vec<DirectoryEntry>,
}

pub(crate) async fn list(
    claims: Option<Extension<CallerClaims>>,
    Query(query): Query<DirectoryQuery>,
) -> Result<Json<DirectoryListing>, ApiError> {
    // The router's authentication middleware has already approved this request.
    // Absent claims mean the configured local no-auth policy, not an auth override.
    let claims = claims.map(|claims| claims.0);
    tokio::task::spawn_blocking(move || browse(claims.as_ref(), query.path.as_deref()))
        .await
        .map_err(|e| ApiError::internal(e.to_string()))?
        .map(Json)
}

fn browse(
    claims: Option<&CallerClaims>,
    requested: Option<&str>,
) -> Result<DirectoryListing, ApiError> {
    if claims.is_some_and(|claims| claims.hosted && claims.directory_prefixes.is_empty())
    {
        return Err(ApiError::forbidden(
            "Hosted directory browsing requires an explicit directory scope",
        ));
    }
    let allows = |path: &std::path::Path| {
        claims.is_none_or(|claims| {
            crate::caller::allows_directory(claims, &path.to_string_lossy())
        })
    };
    let default = claims
        .and_then(|claims| claims.directory_prefixes.first())
        .cloned()
        .unwrap_or_else(|| crate::resolve_directory(None, &Default::default()));
    let resolve_default = || {
        crate::windows_process::canonicalize_path(
            &crate::session_routes::expand_home_path(&default)?,
        )
        .map_err(|_| ApiError::bad_request("Folder is unavailable"))
    };
    let path = match requested.filter(|p| !p.trim().is_empty()) {
        None => resolve_default()?,
        Some(raw) => {
            let candidate = crate::session_routes::expand_home_path(raw)?;
            let candidate = if candidate.is_absolute() {
                candidate
            } else {
                resolve_default()?.join(candidate)
            };
            crate::windows_process::canonicalize_path(&candidate)
                .map_err(|_| ApiError::bad_request("Folder is unavailable"))?
        }
    };
    if !allows(&path) {
        return Err(ApiError::forbidden(
            "The caller is not authorized for this directory",
        ));
    }
    let reader = std::fs::read_dir(&path)
        .map_err(|_| ApiError::bad_request("Folder cannot be read"))?;
    let mut entries = Vec::new();
    for entry in reader {
        let entry = entry.map_err(|_| ApiError::bad_request("Folder cannot be read"))?;
        let Ok(resolved) = crate::windows_process::canonicalize_path(&entry.path())
        else {
            continue;
        };
        if resolved.is_dir() && allows(&resolved) {
            entries.push(DirectoryEntry {
                name: entry.file_name().to_string_lossy().into_owned(),
                path: resolved.to_string_lossy().into_owned(),
            });
        }
    }
    entries.sort_by_cached_key(|entry| entry.name.to_lowercase());
    let mut seen = std::collections::HashSet::new();
    entries.retain(|entry| seen.insert(entry.path.clone()));
    let parent = path
        .parent()
        .filter(|p| allows(p))
        .map(|p| p.to_string_lossy().into_owned());
    Ok(DirectoryListing {
        path: path.to_string_lossy().into_owned(),
        parent,
        entries,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Temp(std::path::PathBuf);
    impl Temp {
        fn new() -> Self {
            Self::under(&std::env::temp_dir())
        }
        fn under(base: &std::path::Path) -> Self {
            let path = base.join(format!(
                "neoism-directory-picker-{}",
                neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Session)
            ));
            std::fs::create_dir_all(&path).unwrap();
            Self(path)
        }
        fn path(&self) -> &std::path::Path {
            &self.0
        }
    }
    impl Drop for Temp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    fn claims(root: &std::path::Path) -> CallerClaims {
        CallerClaims {
            subject: "test".into(),
            workspace_id: None,
            tenant_id: "test".into(),
            directory_prefixes: vec![root.to_string_lossy().into_owned()],
            hosted: true,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
        }
    }
    #[test]
    fn canonical_children_only_and_scoped_parent() {
        let temp = Temp::new();
        let root = temp.path().join("allowed");
        std::fs::create_dir_all(root.join("child")).unwrap();
        std::fs::write(root.join("file.txt"), "not a directory").unwrap();
        let claims = claims(&root);
        let result = browse(Some(&claims), None).unwrap();
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].name, "child");
        assert!(result.parent.is_none());
        let nested = browse(Some(&claims), Some("child/..")).unwrap();
        assert_eq!(nested.path, result.path);
        assert!(browse(Some(&claims), Some("..")).is_err());
        assert!(browse(Some(&claims), Some("file.txt")).is_err());
        assert!(browse(Some(&claims), Some("missing")).is_err());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_cannot_escape_caller_scope() {
        let temp = Temp::new();
        let root = temp.path().join("allowed");
        let outside = temp.path().join("outside");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("escape")).unwrap();
        let claims = claims(&root);
        assert!(browse(Some(&claims), None).unwrap().entries.is_empty());
        assert!(browse(Some(&claims), Some("escape")).is_err());
    }
    #[tokio::test]
    async fn middleware_approved_local_no_auth_can_browse() {
        let result = list(None, Query(DirectoryQuery::default())).await.unwrap();
        assert_eq!(
            result.path,
            crate::windows_process::canonicalize_path(&std::env::current_dir().unwrap())
                .unwrap()
                .to_string_lossy()
        );
    }
    #[test]
    fn relative_default_root_is_resolved_once() {
        let cwd = std::env::current_dir().unwrap();
        let temp = Temp::under(&cwd);
        std::fs::create_dir_all(temp.path().join("child")).unwrap();
        let claims = claims(temp.path().strip_prefix(&cwd).unwrap());
        let expected = crate::windows_process::canonicalize_path(temp.path()).unwrap();
        for input in [None, Some(""), Some("  "), Some("."), Some("child/..")] {
            let result = browse(Some(&claims), input).unwrap();
            assert_eq!(result.path, expected.to_string_lossy());
            assert!(result.parent.is_none());
        }
        let child = browse(Some(&claims), Some("child")).unwrap();
        assert_eq!(child.path, expected.join("child").to_string_lossy());
        assert_eq!(child.parent.as_deref(), Some(expected.to_str().unwrap()));
        assert!(browse(Some(&claims), Some("..")).is_err());
    }
    #[test]
    fn explicit_absolute_path_does_not_require_available_default_root() {
        let temp = Temp::new();
        let mut claims = claims(&temp.path().join("missing"));
        claims
            .directory_prefixes
            .push(temp.path().to_string_lossy().into_owned());
        assert!(browse(Some(&claims), None).is_err());
        let result = browse(Some(&claims), Some(temp.path().to_str().unwrap())).unwrap();
        assert_eq!(
            result.path,
            crate::windows_process::canonicalize_path(temp.path())
                .unwrap()
                .to_string_lossy()
        );
    }
    #[test]
    fn home_paths_use_server_home_and_preserve_scope() {
        let home = crate::session_routes::expand_home_path("~").unwrap();
        let claims = claims(&home);
        let expected = crate::windows_process::canonicalize_path(&home).unwrap();
        for input in ["~", "~/.", "~\\."] {
            assert_eq!(
                browse(Some(&claims), Some(input)).unwrap().path,
                expected.to_string_lossy()
            );
        }
        assert!(browse(Some(&claims), Some("~someone")).is_err());
        let temp = Temp::new();
        assert!(browse(Some(&self::claims(temp.path())), Some("~")).is_err());
    }
    #[test]
    fn hosted_browsing_requires_explicit_roots() {
        let mut claims = claims(std::path::Path::new("/"));
        claims.directory_prefixes.clear();
        assert!(browse(Some(&claims), None).is_err());
    }
}
