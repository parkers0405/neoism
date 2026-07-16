//! Per-message handler free functions: open/close/switch/forget
//! workspace, session lifecycle, editor-surface binding, logical-window
//! open, pane-layout op, and the workspace-action runner. Pure
//! code-move out of the former monolithic `workspace.rs`.

use super::shell_ops::create_neoism_note;
use super::*;

pub(crate) fn open_workspace(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    path: PathBuf,
    init_if_missing: bool,
) -> Vec<WorkspaceServerMessage> {
    if init_if_missing {
        if let Err(e) = std::fs::create_dir_all(&path) {
            return vec![err(format!("failed to create {}: {e}", path.display()))];
        }
    }
    if !path.exists() {
        return vec![err(format!("path does not exist: {}", path.display()))];
    }
    // Check for an existing workspace at this path. Otherwise mint a
    // fresh UUID.
    let canonical = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let existing = {
        let inner = manager.inner.lock();
        inner
            .workspaces
            .values()
            .find(|w| {
                std::fs::canonicalize(&w.path)
                    .map(|c| c == canonical)
                    .unwrap_or(false)
            })
            .cloned()
    };

    let workspace = if let Some(mut w) = existing {
        w.last_opened = now_secs();
        manager.upsert_workspace(w.clone());
        w
    } else {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string());
        let w = ProjectRootSummary {
            id: Uuid::new_v4().to_string(),
            name,
            path: canonical.clone(),
            last_opened: now_secs(),
        };
        manager.upsert_workspace(w.clone());
        w
    };

    conn.active_workspace = Some(workspace.id.clone());
    conn.active_session = None;
    vec![
        WorkspaceServerMessage::ProjectRootOpened {
            project_root: workspace.clone(),
        },
        WorkspaceServerMessage::ProjectRootChanged {
            id: Some(workspace.id),
        },
    ]
}

pub(crate) fn close_workspace(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    id: &str,
) -> Vec<WorkspaceServerMessage> {
    // We deliberately do NOT delete from the registry — only drop
    // in-memory state. The protocol contract says `ForgetProjectRoot`
    // is the removal op.
    if manager.get_workspace(id).is_none() {
        return vec![err(format!("no such workspace: {id}"))];
    }
    // Clear any sessions belonging to this workspace from the live map.
    let to_remove: Vec<String> = {
        let inner = manager.inner.lock();
        inner
            .sessions
            .values()
            .filter(|s| s.workspace_id == id)
            .map(|s| s.id.clone())
            .collect()
    };
    for sid in &to_remove {
        manager.remove_session(sid);
    }
    let mut out = vec![WorkspaceServerMessage::ProjectRootClosed { id: id.to_string() }];
    for sid in to_remove {
        out.push(WorkspaceServerMessage::SessionClosed { session_id: sid });
    }
    if conn.active_workspace.as_deref() == Some(id) {
        conn.active_workspace = None;
        conn.active_session = None;
        out.push(WorkspaceServerMessage::ProjectRootChanged { id: None });
    }
    out
}

pub(crate) fn switch_workspace(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    id: String,
) -> Vec<WorkspaceServerMessage> {
    let Some(mut ws) = manager.get_workspace(&id) else {
        return vec![err(format!("no such workspace: {id}"))];
    };
    ws.last_opened = now_secs();
    manager.upsert_workspace(ws);
    conn.active_workspace = Some(id.clone());
    conn.active_session = None;
    vec![WorkspaceServerMessage::ProjectRootChanged { id: Some(id) }]
}

pub(crate) fn get_workspace_info(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
    id: String,
) -> Vec<WorkspaceServerMessage> {
    let Some(ws) = manager.get_workspace(&id) else {
        return vec![err(format!("no such workspace: {id}"))];
    };
    let sessions = manager.sessions_for_workspace(&id);
    let active = conn.active_workspace.as_deref() == Some(id.as_str());
    vec![WorkspaceServerMessage::ProjectRootInfo {
        id: ws.id,
        name: ws.name,
        path: ws.path,
        sessions,
        active,
    }]
}

pub(crate) fn forget_workspace(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    id: String,
) -> Vec<WorkspaceServerMessage> {
    if !manager.forget_workspace(&id) {
        return vec![err(format!("no such workspace: {id}"))];
    }
    let mut out = Vec::new();
    if conn.active_workspace.as_deref() == Some(id.as_str()) {
        conn.active_workspace = None;
        conn.active_session = None;
        out.push(WorkspaceServerMessage::ProjectRootChanged { id: None });
    }
    out
}

pub(crate) fn list_sessions(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
) -> Vec<WorkspaceServerMessage> {
    let Some(ws_id) = conn.active_workspace.clone() else {
        return vec![err("no active workspace".into())];
    };
    let sessions = manager.sessions_for_workspace(&ws_id);
    vec![WorkspaceServerMessage::SessionList { sessions }]
}

pub(crate) fn switch_session(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    session_id: String,
) -> Vec<WorkspaceServerMessage> {
    let Some(s) = manager.get_session(&session_id) else {
        return vec![err(format!("no such session: {session_id}"))];
    };
    if conn.active_workspace.as_deref() != Some(s.workspace_id.as_str()) {
        return vec![err("session does not belong to the active workspace".into())];
    }
    conn.active_session = Some(session_id.clone());
    vec![WorkspaceServerMessage::SessionChanged {
        session_id: Some(session_id),
    }]
}

pub(crate) fn new_session(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    cwd: Option<String>,
    label: Option<String>,
) -> Vec<WorkspaceServerMessage> {
    let Some(ws_id) = conn.active_workspace.clone() else {
        return vec![err("no active workspace".into())];
    };
    let session = SessionSummary {
        id: Uuid::new_v4().to_string(),
        workspace_id: ws_id,
        cwd: cwd.unwrap_or_else(|| ".".to_string()),
        label,
        last_active: now_secs(),
    };
    manager.insert_session(session.clone());
    conn.active_session = Some(session.id.clone());
    vec![
        WorkspaceServerMessage::SessionCreated {
            session: session.clone(),
        },
        WorkspaceServerMessage::SessionChanged {
            session_id: Some(session.id),
        },
    ]
}

pub(crate) fn close_session(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    session_id: String,
) -> Vec<WorkspaceServerMessage> {
    if !manager.remove_session(&session_id) {
        return vec![err(format!("no such session: {session_id}"))];
    }
    let mut out = vec![WorkspaceServerMessage::SessionClosed {
        session_id: session_id.clone(),
    }];
    if conn.active_session.as_deref() == Some(session_id.as_str()) {
        conn.active_session = None;
        out.push(WorkspaceServerMessage::SessionChanged { session_id: None });
    }
    out
}

pub(crate) fn bind_editor_surface(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
    surface_id: String,
    session_id: String,
    path: Option<PathBuf>,
) -> Vec<WorkspaceServerMessage> {
    let session = if let Some(session) = manager.get_session(&session_id) {
        session
    } else {
        let Some(ws_id) = conn.active_workspace.clone() else {
            return vec![err("no active workspace".into())];
        };
        // Web can bind editor surfaces before the workspace session
        // inventory has arrived, while it already has a PTY session id
        // from the transport layer. Treat that id as a valid workspace
        // session instead of poisoning the tab with "no such session".
        let session = SessionSummary {
            id: session_id.clone(),
            workspace_id: ws_id,
            cwd: ".".to_string(),
            label: None,
            last_active: now_secs(),
        };
        manager.insert_session(session.clone());
        // In this synthesised case the workspace tab id *is* the live
        // PTY session id (it came straight off the transport layer), so
        // record the bridge explicitly. This is the one place we know
        // both ids and that they coincide; recording it lets
        // `pty_session_for` resolve the tab to its shell without the
        // caller having to special-case "id == pty id".
        manager.link_pty_session(&session_id, session_id.clone());
        session
    };
    if conn.active_workspace.as_deref() != Some(session.workspace_id.as_str()) {
        return vec![err("session does not belong to the active workspace".into())];
    }
    // Reuse an existing route_id for this surface so re-binds (e.g.
    // retargeting the active path) don't churn the chrome's
    // `SubscribeDiagnostics` subscription. New surfaces get a fresh
    // monotonic id from the per-daemon counter.
    let route_id = manager.route_id_for_surface(&surface_id);
    let surface = EditorSurfaceSummary {
        surface_id,
        workspace_id: session.workspace_id,
        session_id,
        path,
        last_active: now_secs(),
        route_id: Some(route_id),
    };
    manager.upsert_editor_surface(surface.clone());
    vec![WorkspaceServerMessage::EditorSurfaceChanged { surface }]
}

pub(crate) fn list_editor_surfaces(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
) -> Vec<WorkspaceServerMessage> {
    let Some(ws_id) = conn.active_workspace.clone() else {
        return vec![err("no active workspace".into())];
    };
    vec![WorkspaceServerMessage::EditorSurfaceList {
        surfaces: manager.editor_surfaces_for_workspace(&ws_id),
    }]
}

pub(crate) fn open_logical_window(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
    kind: WorkspaceWindowKind,
    requested_workspace_id: Option<String>,
    parent_window_id: Option<String>,
    title: Option<String>,
) -> Vec<WorkspaceServerMessage> {
    let workspace_id = requested_workspace_id.or_else(|| conn.active_workspace.clone());
    if let Some(id) = workspace_id.as_deref() {
        if manager.get_workspace(id).is_none() {
            return vec![err(format!("no such workspace: {id}"))];
        }
    }

    let title = title.unwrap_or_else(|| match kind {
        WorkspaceWindowKind::Terminal | WorkspaceWindowKind::NativeTab => "Neoism".into(),
        WorkspaceWindowKind::ConfigEditor => "Rio Settings".into(),
    });
    let now = now_secs();
    let window = WorkspaceWindowSummary {
        id: Uuid::new_v4().to_string(),
        kind,
        workspace_id,
        parent_window_id,
        title,
        route_id: Some(manager.allocate_route_id()),
        created_at: now,
        last_active: now,
    };
    manager.insert_window(window.clone());
    vec![WorkspaceServerMessage::WindowOpened { window }]
}

/// Apply a pane-layout op to the daemon-owned canonical layout tree
/// and echo/broadcast the resulting snapshot.
pub(crate) fn handle_pane_layout_op(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
    pane_external_id: u64,
    op: PaneLayoutOp,
) -> Vec<WorkspaceServerMessage> {
    let Some(ws_id) = conn.active_workspace.clone() else {
        return vec![err("no active workspace".into())];
    };
    let layout = match manager.apply_pane_layout_op(&ws_id, pane_external_id, op) {
        Ok(layout) => layout,
        Err(message) => return vec![err(message)],
    };
    let new_layout_snapshot = match serde_json::to_string(&layout) {
        Ok(json) => json,
        Err(e) => return vec![err(format!("failed to serialize pane layout: {e}"))],
    };
    // Fan the accepted op out to every other connected websocket via
    // the manager's broadcast bus so paired surfaces converge on the
    // same pane tree without polling. The submitter still gets the
    // echo via the synchronous reply below — both paths are wire-equal
    // so the chrome handles them identically.
    manager.broadcast_pane_layout(
        pane_external_id,
        op,
        Some(new_layout_snapshot.clone()),
    );
    vec![WorkspaceServerMessage::PaneLayoutChanged {
        pane_external_id,
        op,
        new_layout_snapshot: Some(new_layout_snapshot),
    }]
}

pub(crate) fn run_workspace_action(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    action: WorkspaceAction,
) -> Vec<WorkspaceServerMessage> {
    let Some(root) = active_workspace_root(manager, conn) else {
        return vec![err("no active workspace".into())];
    };
    match action {
        WorkspaceAction::CreateNeoismNote => create_neoism_note(&root)
            .map(|path| {
                vec![WorkspaceServerMessage::WorkspaceActionCompleted {
                    action,
                    path: Some(path.clone()),
                    message: format!("created {}", path.display()),
                }]
            })
            .unwrap_or_else(|e| vec![err(e)]),
    }
}

pub(crate) fn active_workspace_root(
    manager: &WorkspaceManager,
    conn: &ConnectionWorkspace,
) -> Option<PathBuf> {
    let id = conn.active_workspace.as_deref()?;
    manager
        .get_host_workspace(id)
        .and_then(|ws| ws.root_dir)
        .or_else(|| manager.get_workspace(id).map(|ws| ws.path))
}
