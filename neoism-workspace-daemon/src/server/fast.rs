//! Fail-closed WebSocket policy for temporary Fast Servers.
//! This scopes file/control APIs, NOT shell/agent execution. Fast collaborators
//! are trusted to run code with the host user's privileges.
use super::socket::ServiceClientMessage;
use neoism_protocol::{
    editor::EditorClientMessage, files::FilesClientMessage,
    workspace::WorkspaceClientMessage,
};
use serde_json::Value;
use std::path::{Component, Path};

/// Validate and reserialize a service envelope. Unknown/administrative planes
/// and raw legacy PTY frames are rejected. Hello credentials are replaced only
/// AFTER the ingress authenticated the upgrade with its separate guest secret.
pub fn filter_frame(
    text: &str,
    root: &Path,
    daemon_token: &str,
) -> Result<String, String> {
    if text.len() > 4 * 1024 * 1024 {
        return Err("Fast Server frame too large".into());
    }
    let mut envelope: ServiceClientMessage =
        serde_json::from_str(text).map_err(|_| "Invalid Fast Server service frame")?;
    match &mut envelope {
        ServiceClientMessage::Workspace { message, .. } => {
            use WorkspaceClientMessage::*;
            match message {
                Hello { token, .. } => *token = Some(daemon_token.to_owned()),
                ListProjectRoots
                | SwitchProjectRoot { .. }
                | GetProjectRootInfo { .. }
                | ListSessions
                | SwitchSession { .. }
                | NewSession { .. }
                | CloseSession { .. }
                | GetSessionState { .. }
                | SetCwd { .. }
                | RenameSession { .. }
                | BindEditorSurface { .. }
                | ListEditorSurfaces
                | CloseEditorSurface { .. }
                | ListWindows
                | PaneLayoutOp { .. }
                | PublishPaneLayout { .. }
                | Ping { .. }
                | RequestFullSnapshot { .. }
                | GetWorkplacePreferences { .. }
                | SetWorkplacePreferences { .. }
                | ListHosts
                | ListHostWorkspaces { .. }
                | ListWorkspaceTabs { .. }
                | RequestHostWorkspaceTree
                | ResolveInitialWorkspace { .. }
                | SwitchHostWorkspace { .. }
                | SubscribeWorkspace { .. }
                | UnsubscribeWorkspace { .. }
                | PublishWorkspaceTabs { .. } => {}
                _ => {
                    return Err(
                        "Workspace administration is not available on Fast Servers"
                            .into(),
                    )
                }
            }
        }
        ServiceClientMessage::Files {
            message,
            workspace_root,
            ..
        } => {
            if matches!(
                message,
                FilesClientMessage::ListBrowserLocations
                    | FilesClientMessage::BrowserListDir { .. }
                    | FilesClientMessage::BrowserStat { .. }
                    | FilesClientMessage::BrowserReadFile { .. }
                    | FilesClientMessage::ReadShellHistory { .. }
            ) {
                return Err(
                    "Host browser/history is not available on Fast Servers".into()
                );
            }
            scope_root(workspace_root, root)?;
            if let FilesClientMessage::CreateFile { dir, name }
            | FilesClientMessage::CreateDir { dir, name } = message
            {
                // Wire names are basenames, never paths (reject both platform
                // separators even on Unix). Check the actual destination too:
                // create/write may follow an already-existing leaf symlink.
                let mut components = Path::new(name).components();
                if name.contains(['/', '\\'])
                    || !matches!(components.next(), Some(Component::Normal(_)))
                    || components.next().is_some()
                {
                    return Err(
                        "Fast Server creation requires one normal filename component"
                            .into(),
                    );
                }
                let destination = Path::new(dir).join(name);
                check_path(
                    destination.to_str().ok_or("Invalid Fast Server filename")?,
                    root,
                )?;
            }
        }
        ServiceClientMessage::Git { workspace_root, .. }
        | ServiceClientMessage::Search { workspace_root, .. } => {
            scope_root(workspace_root, root)?
        }
        ServiceClientMessage::Editor {
            message,
            workspace_root,
            ..
        } => {
            if matches!(message, EditorClientMessage::PluginResource { .. }) {
                return Err("Plugin resources are not available on Fast Servers".into());
            }
            scope_root(workspace_root, root)?;
        }
        ServiceClientMessage::Pty { message, .. } => {
            if let neoism_protocol::pty::ClientMessage::CreatePty { cwd, .. } = message {
                if cwd.is_none() {
                    *cwd = Some(root.to_string_lossy().into_owned());
                }
            }
        }
        ServiceClientMessage::Crdt { .. }
        | ServiceClientMessage::CursorOverlay { .. }
        | ServiceClientMessage::Diagnostics { .. } => {}
        _ => return Err("This service plane is not available on Fast Servers".into()),
    }
    let value = serde_json::to_value(envelope).map_err(|e| e.to_string())?;
    check_paths(&value, root)?;
    serde_json::to_string(&value).map_err(|e| e.to_string())
}

fn scope_root(supplied: &mut Option<String>, root: &Path) -> Result<(), String> {
    if let Some(path) = supplied.as_deref().filter(|s| !s.is_empty()) {
        if Path::new(path) != root {
            return Err("Fast Server workspace root cannot be changed".into());
        }
    }
    *supplied = Some(root.to_string_lossy().into_owned());
    Ok(())
}

fn check_paths(value: &Value, root: &Path) -> Result<(), String> {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(
                    key.as_str(),
                    "path"
                        | "paths"
                        | "cwd"
                        | "directory"
                        | "dir"
                        | "from"
                        | "to"
                        | "root_dir"
                        | "workspace_root"
                ) {
                    check_path_values(value, root)?;
                } else if key == "buffer_id" {
                    if let Some(path) =
                        value.as_str().and_then(|s| s.strip_prefix("file://"))
                    {
                        check_path(path, root)?;
                    }
                } else {
                    check_paths(value, root)?;
                }
            }
        }
        Value::Array(values) => {
            for value in values {
                check_paths(value, root)?;
            }
        }
        _ => {}
    }
    Ok(())
}
fn check_path_values(value: &Value, root: &Path) -> Result<(), String> {
    match value {
        Value::String(path) => check_path(path, root),
        Value::Array(paths) => {
            for path in paths {
                check_path_values(path, root)?;
            }
            Ok(())
        }
        Value::Null => Ok(()),
        _ => Err("Invalid Fast Server path".into()),
    }
}
fn check_path(path: &str, root: &Path) -> Result<(), String> {
    let path = Path::new(path);
    if path.components().any(|c| {
        matches!(c, Component::ParentDir)
            || matches!(c, Component::Prefix(_)) && !path.is_absolute()
    }) {
        return Err("Fast Server paths may not traverse outside the shared root".into());
    }
    let absolute = if path.is_absolute() {
        path.to_owned()
    } else {
        root.join(path)
    };
    if !absolute.starts_with(root) {
        return Err("Path is outside the Fast Server root".into());
    }
    // Resolve existing ancestors too, so new files cannot be created through
    // a symlink to an unshared directory. Not a sandbox or a defense against
    // hostile collaborators racing symlinks using their trusted shell access.
    let mut ancestor = absolute.as_path();
    loop {
        match std::fs::symlink_metadata(ancestor) {
            Ok(_) => {
                let canonical = crate::path::canonicalize(ancestor)
                    .map_err(|_| "Unresolvable Fast Server path")?;
                if !canonical.starts_with(root) {
                    return Err("Symlink escapes the Fast Server root".into());
                }
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                ancestor = ancestor.parent().ok_or("Invalid Fast Server path")?;
            }
            Err(_) => return Err("Unresolvable Fast Server path".into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn policy_is_fail_closed_and_rewrites_hello() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        for frame in [
            r#"{"Config":{"message":"GetConfig","request_id":1}}"#,
            r#"{"Workspace":{"message":{"SetWorkspaceRoot":{"workspace_id":"x","root_dir":"/"}}}}"#,
            r#"{"Files":{"request_id":1,"message":{"ReadShellHistory":{"max_entries":null}}}}"#,
            r#"{"CreatePty":{"cwd":null,"cols":80,"rows":24}}"#,
        ] {
            assert!(filter_frame(frame, &root, "master").is_err(), "{frame}");
        }
        let hello = r#"{"Workspace":{"message":{"Hello":{"token":"guest"}}}}"#;
        let result = filter_frame(hello, &root, "master").unwrap();
        assert!(result.contains("master"));
        assert!(!result.contains("guest"));
    }
    #[test]
    fn creation_checks_basename_and_final_destination() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        for operation in ["CreateFile", "CreateDir"] {
            for name in [
                "",
                ".",
                "..",
                "../escape",
                "nested/file",
                "nested\\file",
                "/absolute",
            ] {
                let frame = serde_json::json!({"Files":{"request_id":1,"message":{operation:{"dir":"","name":name}}}});
                assert!(
                    filter_frame(&frame.to_string(), &root, "master").is_err(),
                    "{operation}: {name}"
                );
            }
            let frame = serde_json::json!({"Files":{"request_id":1,"message":{operation:{"dir":"","name":"ordinary"}}}});
            assert!(filter_frame(&frame.to_string(), &root, "master").is_ok());
        }
    }
    #[cfg(unix)]
    #[test]
    fn creation_rejects_preexisting_leaf_symlinks() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        let secret = outside.path().join("secret");
        std::fs::write(&secret, "untouched").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("file-link")).unwrap();
        std::os::unix::fs::symlink(outside.path(), root.join("dir-link")).unwrap();
        for (operation, name) in [("CreateFile", "file-link"), ("CreateDir", "dir-link")]
        {
            let frame = serde_json::json!({"Files":{"request_id":1,"message":{operation:{"dir":"","name":name}}}});
            assert!(filter_frame(&frame.to_string(), &root, "master").is_err());
        }
        assert_eq!(std::fs::read_to_string(secret).unwrap(), "untouched");
    }

    #[test]
    fn terminal_directory_control_checks_host_scope_before_dispatch() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = project.path().canonicalize().unwrap();
        for directory in [outside.path().to_string_lossy().into_owned(), "../escape".into()] {
            let frame = serde_json::json!({"Pty":{"request_id":9,"message":{"ChangeTerminalDirectory":{
                "session_id":"host-pty", "request_id":"generation-b", "directory":directory
            }}}});
            assert!(filter_frame(&frame.to_string(), &root, "master").is_err());
        }
        let frame = serde_json::json!({"Pty":{"request_id":9,"message":{"ChangeTerminalDirectory":{
            "session_id":"host-pty", "request_id":"generation-b", "directory":root
        }}}});
        assert!(filter_frame(&frame.to_string(), &root, "master").is_ok());
    }

    #[test]
    fn traversal_and_absolute_escape_are_denied() {
        let root = std::env::temp_dir().canonicalize().unwrap();
        for path in ["../secret", "/etc/passwd", "a/../../secret"] {
            let frame = serde_json::json!({"Files":{"request_id":1,"message":{"ReadFile":{"path":path}}}});
            assert!(filter_frame(&frame.to_string(), &root, "secret").is_err());
        }
        assert!(check_path("new-file", &root).is_ok());
    }
    #[cfg(unix)]
    #[test]
    fn symlink_escape_is_denied_for_existing_and_new_files() {
        let root = std::env::temp_dir()
            .join(format!("neoism-fast-policy-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&root).unwrap();
        std::os::unix::fs::symlink("/etc", root.join("escape")).unwrap();
        assert!(check_path("escape/passwd", &root).is_err());
        assert!(check_path("escape/new-file", &root).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
