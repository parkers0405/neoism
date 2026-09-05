use super::*;
use neoism_protocol::workspace::{
    PaneLayoutSnapshotNode, PaneSplitAxis, PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
};
use tempfile::TempDir;

/// Back-compat shim for tests written before the `Hello` arm
/// gained the `pairing_tokens` + `peer_ip` parameters. The
/// pre-existing arms never touched either, so we pass `None` and
/// flatten the `DispatchOutcome` back to the legacy
/// `Vec<WorkspaceServerMessage>` return shape every assertion
/// below already speaks. New tests that exercise the `Hello`
/// flow live in `crate::handshake::tests` and call the real
/// `super::handle` directly.
fn handle(
    manager: &WorkspaceManager,
    conn: &mut ConnectionWorkspace,
    msg: WorkspaceClientMessage,
) -> Vec<WorkspaceServerMessage> {
    super::handle(manager, conn, None, None, msg).replies
}

fn make_manager(td: &TempDir) -> WorkspaceManager {
    let path = td.path().join("workspaces.json");
    let (preferences_tx, _) =
        tokio::sync::broadcast::channel(PREFERENCES_BROADCAST_CAPACITY);
    let (pane_layout_tx, _) =
        tokio::sync::broadcast::channel(PANE_LAYOUT_BROADCAST_CAPACITY);
    let (tree_tx, _) = tokio::sync::broadcast::channel(TREE_BROADCAST_CAPACITY);
    WorkspaceManager {
        inner: Arc::new(Mutex::new(ManagerInner {
            hosts: HashMap::new(),
            host_workspaces: HashMap::new(),
            previous_workspace_roots: HashMap::new(),
            workspace_tabs: HashMap::new(),
            active_workspace_by_host: HashMap::new(),
            workspaces: HashMap::new(),
            sessions: HashMap::new(),
            editor_surfaces: HashMap::new(),
            pane_layouts: HashMap::new(),
            windows: HashMap::new(),
            preferences: HashMap::new(),
            client_states: HashMap::new(),
            session_pty_links: HashMap::new(),
            next_route_id: 1,
        })),
        persistence_path: path,
        preferences_tx: Arc::new(preferences_tx),
        pane_layout_tx: Arc::new(pane_layout_tx),
        tree_tx: Arc::new(tree_tx),
        snapshot_writer: SnapshotWriter::ephemeral(),
    }
}

#[test]
fn first_and_later_terminal_cwd_never_reroot_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let declared = td.path().join("declared");
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: declared.clone(),
            init_if_missing: true,
        },
    );
    let mut sessions = Vec::new();
    for _ in 0..2 {
        let created = handle(
            &mgr,
            &mut conn,
            WorkspaceClientMessage::NewSession {
                cwd: None,
                label: None,
            },
        );
        sessions.push(
            created
                .into_iter()
                .find_map(|message| match message {
                    WorkspaceServerMessage::SessionCreated { session } => {
                        Some(session.id)
                    }
                    _ => None,
                })
                .expect("session"),
        );
    }
    let hosted = mgr.create_host_workspace(
        "host-a".into(),
        Some("cwd-stable-workspace".into()),
        None,
        Some(declared.clone()),
    );
    {
        let mut inner = mgr.inner.lock();
        for session_id in &sessions {
            inner.sessions.get_mut(session_id).unwrap().workspace_id = hosted.id.clone();
        }
        inner
            .host_workspaces
            .get_mut(&hosted.id)
            .unwrap()
            .main_session_id = Some(sessions[0].clone());
        inner
            .session_pty_links
            .insert(sessions[0].clone(), "pty-first".into());
        inner
            .session_pty_links
            .insert(sessions[1].clone(), "pty-later".into());
    }
    let roots_before: Vec<_> = mgr
        .inner
        .lock()
        .host_workspaces
        .values()
        .map(|workspace| workspace.root_dir.clone())
        .collect();
    assert!(mgr.track_pty_cwd("pty-first", "/tmp/first".into()));
    let roots_after_first: Vec<_> = mgr
        .inner
        .lock()
        .host_workspaces
        .values()
        .map(|workspace| workspace.root_dir.clone())
        .collect();
    assert_eq!(roots_after_first, roots_before);
    assert!(mgr.track_pty_cwd("pty-later", "/tmp/later".into()));
    let roots_after: Vec<_> = mgr
        .inner
        .lock()
        .host_workspaces
        .values()
        .map(|workspace| workspace.root_dir.clone())
        .collect();
    assert_eq!(roots_after, roots_before);
    assert!(roots_after
        .iter()
        .any(|root| root.as_deref() == Some(declared.as_path())));
}

#[test]
fn open_workspace_creates_directory_when_requested() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let path = td.path().join("new-ws");
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: path.clone(),
            init_if_missing: true,
        },
    );
    assert!(
        out.iter()
            .any(|m| matches!(m, WorkspaceServerMessage::ProjectRootOpened { .. })),
        "expected ProjectRootOpened, got {out:?}"
    );
    assert!(path.is_dir(), "init_if_missing should create the directory");
    assert!(conn.active_workspace.is_some());
}

#[test]
fn list_then_forget_round_trip() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws-a"),
            init_if_missing: true,
        },
    );
    let listed = handle(&mgr, &mut conn, WorkspaceClientMessage::ListProjectRoots);
    match listed.into_iter().next().unwrap() {
        WorkspaceServerMessage::ProjectRootList {
            project_roots: workspaces,
        } => {
            assert_eq!(workspaces.len(), 1);
            let id = workspaces[0].id.clone();
            let out = handle(
                &mgr,
                &mut conn,
                WorkspaceClientMessage::ForgetProjectRoot { id },
            );
            assert!(out.iter().any(|m| matches!(
                m,
                WorkspaceServerMessage::ProjectRootChanged { id: None }
            )));
        }
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn session_lifecycle_requires_active_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: None,
            label: None,
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::Error { .. })
    ));
}

#[test]
fn new_session_after_open_works() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: Some("src".into()),
            label: Some("editor".into()),
        },
    );
    let created = out
        .iter()
        .find_map(|m| match m {
            WorkspaceServerMessage::SessionCreated { session } => Some(session.clone()),
            _ => None,
        })
        .expect("session created");
    assert_eq!(created.cwd, "src");
    assert_eq!(created.label.as_deref(), Some("editor"));
    // SetCwd should mutate the in-memory copy.
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::SetCwd {
            session_id: created.id.clone(),
            path: "src/foo".into(),
        },
    );
    assert!(out.is_empty(), "SetCwd succeeded silently: {out:?}");
    let state = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::GetSessionState {
            session_id: created.id,
        },
    );
    match state.into_iter().next().unwrap() {
        WorkspaceServerMessage::SessionState { cwd, .. } => {
            assert_eq!(cwd, "src/foo");
        }
        other => panic!("unexpected reply {other:?}"),
    }
}

#[test]
fn pty_link_records_and_resolves() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    // No link until a tab exists + is bridged.
    assert!(
        !mgr.link_pty_session("ghost-tab", "pty-1".into()),
        "linking an unknown tab must fail"
    );
    let session = SessionSummary {
        id: "tab-1".into(),
        workspace_id: "ws".into(),
        cwd: ".".into(),
        label: None,
        last_active: now_secs(),
    };
    mgr.insert_session(session);
    assert!(mgr.link_pty_session("tab-1", "pty-1".into()));
    assert_eq!(mgr.pty_session_for("tab-1").as_deref(), Some("pty-1"));
    // Last-write-wins (e.g. after a respawn-in-cwd).
    assert!(mgr.link_pty_session("tab-1", "pty-2".into()));
    assert_eq!(mgr.pty_session_for("tab-1").as_deref(), Some("pty-2"));
    // Unlink leaves the tab but forgets the shell.
    assert_eq!(mgr.unlink_pty_session("tab-1").as_deref(), Some("pty-2"));
    assert!(mgr.pty_session_for("tab-1").is_none());
    assert!(mgr.get_session("tab-1").is_some());
}

#[test]
fn set_cwd_drops_stale_pty_link() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let created = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: Some("src".into()),
            label: None,
        },
    )
    .into_iter()
    .find_map(|m| match m {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .expect("session created");
    assert!(mgr.link_pty_session(&created.id, "pty-live".into()));
    // SetCwd applies the respawn-in-cwd policy: it updates the cwd
    // and drops the now-stale PTY link so the next attach respawns.
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::SetCwd {
            session_id: created.id.clone(),
            path: "src/foo".into(),
        },
    );
    assert!(out.is_empty(), "SetCwd should succeed silently: {out:?}");
    assert!(
        mgr.pty_session_for(&created.id).is_none(),
        "SetCwd must drop the stale PTY link"
    );
    assert_eq!(mgr.get_session(&created.id).unwrap().cwd, "src/foo");
}

#[test]
fn closing_session_clears_pty_link() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let session = SessionSummary {
        id: "tab-x".into(),
        workspace_id: "ws".into(),
        cwd: ".".into(),
        label: None,
        last_active: now_secs(),
    };
    mgr.insert_session(session);
    mgr.publish_workspace_tabs(
        "ws",
        vec![WorkspaceTabSummary {
            id: "tab-x".into(),
            workspace_id: "ws".into(),
            title: "Terminal".into(),
            kind: Some("terminal".into()),
            session_id: Some("tab-x".into()),
            surface_id: None,
            cwd: Some(".".into()),
            active: true,
            last_active: now_secs(),
        }],
    );
    mgr.upsert_editor_surface(EditorSurfaceSummary {
        surface_id: "pane-x".into(),
        workspace_id: "ws".into(),
        session_id: "tab-x".into(),
        path: Some(PathBuf::from("src/main.rs")),
        last_active: now_secs(),
        route_id: Some(7),
    });
    assert!(mgr.link_pty_session("tab-x", "pty-x".into()));
    assert!(mgr.remove_session("tab-x"));
    assert!(mgr.pty_session_for("tab-x").is_none());
    assert!(mgr.list_workspace_tabs("ws").is_empty());
    assert!(mgr.editor_surfaces_for_workspace("ws").is_empty());
}

#[test]
fn editor_surfaces_are_keyed_by_active_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let created = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: None,
            label: Some("editor".into()),
        },
    )
    .into_iter()
    .find_map(|m| match m {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .expect("session created");

    let changed = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::BindEditorSurface {
            surface_id: "pane-a".into(),
            session_id: created.id.clone(),
            path: Some(PathBuf::from("src/main.rs")),
        },
    );
    match changed.into_iter().next().unwrap() {
        WorkspaceServerMessage::EditorSurfaceChanged { surface } => {
            assert_eq!(surface.surface_id, "pane-a");
            assert_eq!(surface.session_id, created.id);
            assert_eq!(surface.path, Some(PathBuf::from("src/main.rs")));
        }
        other => panic!("unexpected reply {other:?}"),
    }

    let listed = handle(&mgr, &mut conn, WorkspaceClientMessage::ListEditorSurfaces);
    match listed.into_iter().next().unwrap() {
        WorkspaceServerMessage::EditorSurfaceList { surfaces } => {
            assert_eq!(surfaces.len(), 1);
            assert_eq!(surfaces[0].surface_id, "pane-a");
        }
        other => panic!("unexpected reply {other:?}"),
    }

    let closed = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::CloseEditorSurface {
            surface_id: "pane-a".into(),
        },
    );
    assert_eq!(
        closed,
        vec![WorkspaceServerMessage::EditorSurfaceClosed {
            surface_id: "pane-a".into(),
        }]
    );
    let duplicate = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::CloseEditorSurface {
            surface_id: "pane-a".into(),
        },
    );
    assert_eq!(duplicate, closed, "duplicate close remains acknowledged");
}

#[test]
fn logical_window_registry_lifecycle() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );

    let opened = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RequestOpenWindow {
            workspace_id: None,
            title: Some("Neoism".into()),
        },
    );
    let window = match opened.into_iter().next().expect("window reply") {
        WorkspaceServerMessage::WindowOpened { window } => window,
        other => panic!("unexpected reply {other:?}"),
    };
    assert_eq!(window.kind, WorkspaceWindowKind::Terminal);
    assert_eq!(window.workspace_id, conn.active_workspace);
    assert_eq!(window.route_id, Some(1));

    let listed = handle(&mgr, &mut conn, WorkspaceClientMessage::ListWindows);
    match listed.into_iter().next().expect("list reply") {
        WorkspaceServerMessage::WindowList { windows } => {
            assert_eq!(windows, vec![window.clone()]);
        }
        other => panic!("unexpected reply {other:?}"),
    }

    let closed = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RequestCloseWindow {
            window_id: window.id.clone(),
        },
    );
    assert_eq!(
        closed,
        vec![WorkspaceServerMessage::WindowClosed {
            window_id: window.id
        }]
    );
    assert!(mgr.list_windows().is_empty());
}

#[test]
fn logical_window_rejects_unknown_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RequestOpenNativeTab {
            workspace_id: Some("missing".into()),
            parent_window_id: None,
            title: None,
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::Error { .. })
    ));
}

#[test]
fn bind_editor_surface_assigns_monotonic_route_ids() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let created = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: None,
            label: None,
        },
    )
    .into_iter()
    .find_map(|m| match m {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .expect("session created");

    let mut bind = |surface_id: &str| -> Option<u64> {
        let out = handle(
            &mgr,
            &mut conn,
            WorkspaceClientMessage::BindEditorSurface {
                surface_id: surface_id.into(),
                session_id: created.id.clone(),
                path: None,
            },
        );
        out.into_iter().find_map(|m| match m {
            WorkspaceServerMessage::EditorSurfaceChanged { surface } => surface.route_id,
            _ => None,
        })
    };

    let a1 = bind("pane-a").expect("route_id for pane-a");
    let b1 = bind("pane-b").expect("route_id for pane-b");
    let a2 = bind("pane-a").expect("rebind keeps route_id");

    assert_ne!(a1, b1, "distinct surfaces must get distinct route_ids");
    assert_eq!(a1, a2, "re-bind must reuse the same route_id");
    // Daemon hands the first allocation to `pane-a`, so the
    // chrome's legacy `ACTIVE_EDITOR_ROUTE_ID = 1` slot stays valid
    // for single-surface clients.
    assert_eq!(a1, 1);
    assert_eq!(b1, 2);
}

#[test]
fn editor_surface_requires_session_in_active_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::BindEditorSurface {
            surface_id: "pane-a".into(),
            session_id: "missing".into(),
            path: None,
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::Error { .. })
    ));
}

#[test]
fn workspace_root_change_resolves_relative_path_and_rejects_missing_directory() {
    let td = TempDir::new().unwrap();
    let original = td.path().join("projects").join("one");
    let target = td.path().join("projects").join("two with spaces");
    std::fs::create_dir_all(&original).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let mgr = make_manager(&td);
    mgr.create_host_workspace(
        "local".into(),
        Some("workspace-cd".into()),
        None,
        Some(original.clone()),
    );

    let updated = mgr
        .set_host_workspace_root("workspace-cd", PathBuf::from("'../two with spaces'"))
        .expect("existing relative directory accepted");
    assert_eq!(updated.root_dir.as_deref(), Some(target.as_path()));

    let previous = mgr
        .set_host_workspace_root("workspace-cd", PathBuf::from("-"))
        .expect("cd - returns to the previous workspace root");
    assert_eq!(previous.root_dir.as_deref(), Some(original.as_path()));

    let missing = td.path().join("projects").join("missing");
    assert!(mgr
        .set_host_workspace_root("workspace-cd", PathBuf::from("../missing"))
        .is_none());
    assert!(
        !missing.exists(),
        "a mistyped cd must not create a directory"
    );
}

#[test]
fn workspace_action_create_note_falls_back_to_default_vault() {
    let td = TempDir::new().unwrap();
    // `create_neoism_note` resolves the global Default vault when the
    // workspace root has no linked/local notes workspace; point the
    // vaults home at the tempdir so the test never touches the real
    // `~/Neoism/Vaults`.
    std::env::set_var("NEOISM_NOTES_HOME", td.path().join("vaults"));
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );

    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RunWorkspaceAction {
            action: WorkspaceAction::CreateNeoismNote,
        },
    );
    let note = match out.first().unwrap() {
        WorkspaceServerMessage::WorkspaceActionCompleted { path, .. } => {
            path.clone().unwrap()
        }
        other => panic!("unexpected reply {other:?}"),
    };
    assert!(note.is_file());
    assert!(note.starts_with(td.path().join("vaults").join("Default")));
    // The legacy per-project init surface is gone: creating a note must
    // never write a `.neoism/workspace.toml` into the code root.
    let root = mgr
        .get_workspace(conn.active_workspace.as_deref().unwrap())
        .unwrap()
        .path;
    assert!(!root.join(".neoism/workspace.toml").exists());
}

#[test]
fn clipboard_payload_is_connection_scoped() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let payload = ClipboardPayload {
        mime_type: "image/png".into(),
        text: None,
        bytes: vec![1, 2, 3],
        filename: Some("shot.png".into()),
    };
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::StoreClipboard {
            payload: payload.clone(),
        },
    );
    assert_eq!(
        out,
        vec![WorkspaceServerMessage::ClipboardPayload {
            payload: Some(payload.clone())
        }]
    );
    let out = handle(&mgr, &mut conn, WorkspaceClientMessage::LoadClipboard);
    assert_eq!(
        out,
        vec![WorkspaceServerMessage::ClipboardPayload {
            payload: Some(payload)
        }]
    );
}

#[test]
fn registry_persists_across_managers() {
    let td = TempDir::new().unwrap();
    let path = td.path().join("workspaces.json");
    let (preferences_tx, _) =
        tokio::sync::broadcast::channel(PREFERENCES_BROADCAST_CAPACITY);
    let (pane_layout_tx, _) =
        tokio::sync::broadcast::channel(PANE_LAYOUT_BROADCAST_CAPACITY);
    let (tree_tx, _) = tokio::sync::broadcast::channel(TREE_BROADCAST_CAPACITY);
    let mgr = WorkspaceManager {
        inner: Arc::new(Mutex::new(ManagerInner {
            hosts: HashMap::new(),
            host_workspaces: HashMap::new(),
            previous_workspace_roots: HashMap::new(),
            workspace_tabs: HashMap::new(),
            active_workspace_by_host: HashMap::new(),
            workspaces: HashMap::new(),
            sessions: HashMap::new(),
            editor_surfaces: HashMap::new(),
            pane_layouts: HashMap::new(),
            windows: HashMap::new(),
            preferences: HashMap::new(),
            client_states: HashMap::new(),
            session_pty_links: HashMap::new(),
            next_route_id: 1,
        })),
        persistence_path: path.clone(),
        preferences_tx: Arc::new(preferences_tx),
        pane_layout_tx: Arc::new(pane_layout_tx),
        tree_tx: Arc::new(tree_tx),
        snapshot_writer: SnapshotWriter::ephemeral(),
    };
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("persisted"),
            init_if_missing: true,
        },
    );
    // Reload from disk.
    let restored = load_registry(&path);
    assert_eq!(restored.len(), 1, "registry should have one entry on disk");
}

#[test]
fn pane_layout_op_requires_active_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id: 1,
            op: PaneLayoutOp::Close,
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::Error { .. })
    ));
}

#[test]
fn pane_layout_op_requires_known_surface() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id: 42,
            op: PaneLayoutOp::Focus {
                dir: neoism_protocol::workspace::PaneFocusDir::Right,
            },
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::Error { .. })
    ));
}

#[test]
fn pane_layout_op_echoes_when_surface_bound() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let session = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: None,
            label: None,
        },
    )
    .into_iter()
    .find_map(|m| match m {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .expect("session created");
    // Phone-control sugar uses the integer external id stringified.
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::BindEditorSurface {
            surface_id: "7".into(),
            session_id: session.id,
            path: None,
        },
    );

    let ops = [
        PaneLayoutOp::Split {
            axis: neoism_protocol::workspace::PaneSplitAxis::Vertical,
            placement: neoism_protocol::workspace::PaneSplitPlacement::After,
        },
        PaneLayoutOp::Focus {
            dir: neoism_protocol::workspace::PaneFocusDir::Left,
        },
        PaneLayoutOp::ResizeRatio { delta: -0.1 },
        PaneLayoutOp::MoveTab { from: 1, to: 0 },
        PaneLayoutOp::Close,
    ];
    for op in ops {
        let out = handle(
            &mgr,
            &mut conn,
            WorkspaceClientMessage::PaneLayoutOp {
                pane_external_id: 7,
                op,
            },
        );
        match out.into_iter().next().expect("reply") {
            WorkspaceServerMessage::PaneLayoutChanged {
                pane_external_id,
                op: echoed,
                new_layout_snapshot,
            } => {
                assert_eq!(pane_external_id, 7);
                assert_eq!(echoed, op);
                let json = new_layout_snapshot.expect("authoritative snapshot");
                let snapshot: PaneLayoutSnapshot =
                    serde_json::from_str(&json).expect("snapshot json");
                assert_eq!(snapshot.workspace_id, session.workspace_id);
            }
            other => panic!("unexpected reply {other:?}"),
        }
    }
}

#[test]
fn published_pane_layout_becomes_canonical_and_echoes_sync() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let workspace_id = "workspace-with-nested-layout".to_string();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::CreateHostWorkspace {
            host_id: "host-a".into(),
            workspace_id: Some(workspace_id.clone()),
            title: None,
            root_dir: None,
        },
    );

    let leaf = |id| PaneLayoutSnapshotNode::Leaf {
        pane_external_id: id,
        surface_id: format!("surface-{id}"),
        session_id: format!("session-{id}"),
        path: None,
        route_id: Some(id),
    };
    let layout = PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: workspace_id.clone(),
        focused_pane_external_id: 3,
        root: PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Horizontal,
            ratios: vec![0.5],
            children: vec![
                PaneLayoutSnapshotNode::Tabs {
                    active: 1,
                    children: vec![leaf(1), leaf(2)],
                },
                PaneLayoutSnapshotNode::Split {
                    axis: PaneSplitAxis::Vertical,
                    ratios: vec![0.5],
                    children: vec![leaf(3), leaf(4)],
                },
            ],
        },
    };

    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::PublishPaneLayout {
            layout: layout.clone(),
        },
    );
    assert!(matches!(
        out.first(),
        Some(WorkspaceServerMessage::PaneLayoutChanged {
            pane_external_id: 3,
            op: PaneLayoutOp::Sync,
            new_layout_snapshot: Some(_),
        })
    ));
    assert_eq!(mgr.pane_layout_for_workspace(&workspace_id), Some(layout));
}

#[test]
fn pane_layout_op_updates_canonical_full_snapshot() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::OpenProjectRoot {
            path: td.path().join("ws"),
            init_if_missing: true,
        },
    );
    let session = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::NewSession {
            cwd: None,
            label: None,
        },
    )
    .into_iter()
    .find_map(|m| match m {
        WorkspaceServerMessage::SessionCreated { session } => Some(session),
        _ => None,
    })
    .expect("session created");
    handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::BindEditorSurface {
            surface_id: "7".into(),
            session_id: session.id.clone(),
            path: Some(PathBuf::from("src/lib.rs")),
        },
    );

    let op = PaneLayoutOp::Split {
        axis: neoism_protocol::workspace::PaneSplitAxis::Vertical,
        placement: neoism_protocol::workspace::PaneSplitPlacement::After,
    };
    let changed = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::PaneLayoutOp {
            pane_external_id: 7,
            op,
        },
    );
    let changed_layout = match changed.into_iter().next().expect("reply") {
        WorkspaceServerMessage::PaneLayoutChanged {
            new_layout_snapshot: Some(json),
            ..
        } => serde_json::from_str::<PaneLayoutSnapshot>(&json).expect("snapshot json"),
        other => panic!("unexpected reply {other:?}"),
    };
    assert_eq!(changed_layout.focused_pane_external_id, 8);
    assert_eq!(changed_layout.external_ids_in_order(), vec![7, 8]);

    let full = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RequestFullSnapshot { since_offset: None },
    );
    match full.into_iter().next().expect("snapshot") {
        WorkspaceServerMessage::FullSnapshot {
            layout: Some(layout),
            ..
        } => assert_eq!(layout, changed_layout),
        other => panic!("unexpected full snapshot {other:?}"),
    }
}

#[test]
fn pane_layout_snapshot_rehydrates_from_g1_state() {
    let td = TempDir::new().unwrap();
    let mut mgr = make_manager(&td);
    let layout = PaneLayoutSnapshot {
        schema_version: neoism_protocol::workspace::PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws-1".into(),
        focused_pane_external_id: 7,
        root: neoism_protocol::workspace::PaneLayoutSnapshotNode::Leaf {
            pane_external_id: 7,
            surface_id: "7".into(),
            session_id: "sess-1".into(),
            path: Some(PathBuf::from("src/main.rs")),
            route_id: Some(1),
        },
    };
    mgr.rehydrate_from_snapshot(Snapshot {
        version: 1,
        saved_at_unix_secs: 0,
        hosts: Default::default(),
        host_workspaces: Default::default(),
        workspace_tabs: Default::default(),
        active_workspace_by_host: Default::default(),
        workspaces: vec![ProjectRootSummary {
            id: "ws-1".into(),
            name: "Workspace".into(),
            path: td.path().join("ws"),
            last_opened: 0,
        }],
        sessions: vec![SessionSummary {
            id: "sess-1".into(),
            workspace_id: "ws-1".into(),
            cwd: ".".into(),
            label: None,
            last_active: 0,
        }],
        editor_surfaces: Vec::new(),
        pane_layouts: vec![layout.clone()],
        windows: Vec::new(),
        preferences: HashMap::new(),
        next_route_id: 2,
    });
    let mut conn = ConnectionWorkspace {
        active_workspace: Some("ws-1".into()),
        ..Default::default()
    };

    let full = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::RequestFullSnapshot { since_offset: None },
    );
    match full.into_iter().next().expect("snapshot") {
        WorkspaceServerMessage::FullSnapshot {
            layout: Some(restored),
            ..
        } => assert_eq!(restored, layout),
        other => panic!("unexpected full snapshot {other:?}"),
    }
}

// ----------------------------------------------------------------
// F3 — per-workplace persistence coverage.
//   * round-trip: SetWorkplacePreferences -> reload from disk ->
//     GetWorkplacePreferences returns the same payload.
//   * broadcast: a SetWorkplacePreferences fan-outs a
//     WorkplacePreferencesChanged on every subscriber.
// ----------------------------------------------------------------

fn sample_prefs() -> WorkplacePreferences {
    let mut sidebar = HashMap::new();
    sidebar.insert("file_tree".to_string(), 280.0);
    sidebar.insert("diagnostics".to_string(), 200.0);
    WorkplacePreferences {
        theme: Some("solarized-dark".into()),
        font_size: Some(14.5),
        sidebar_widths: sidebar,
        session_tree: Some("{\"v\":1}".into()),
    }
}

#[test]
fn preferences_round_trip_through_disk() {
    let td = TempDir::new().unwrap();
    let path = td.path().join("workspaces.json");
    let (preferences_tx, _) =
        tokio::sync::broadcast::channel(PREFERENCES_BROADCAST_CAPACITY);
    let (pane_layout_tx, _) =
        tokio::sync::broadcast::channel(PANE_LAYOUT_BROADCAST_CAPACITY);
    let (tree_tx, _) = tokio::sync::broadcast::channel(TREE_BROADCAST_CAPACITY);
    let mgr = WorkspaceManager {
        inner: Arc::new(Mutex::new(ManagerInner {
            hosts: HashMap::new(),
            host_workspaces: HashMap::new(),
            previous_workspace_roots: HashMap::new(),
            workspace_tabs: HashMap::new(),
            active_workspace_by_host: HashMap::new(),
            workspaces: HashMap::new(),
            sessions: HashMap::new(),
            editor_surfaces: HashMap::new(),
            pane_layouts: HashMap::new(),
            windows: HashMap::new(),
            preferences: HashMap::new(),
            client_states: HashMap::new(),
            session_pty_links: HashMap::new(),
            next_route_id: 1,
        })),
        persistence_path: path.clone(),
        preferences_tx: Arc::new(preferences_tx),
        pane_layout_tx: Arc::new(pane_layout_tx),
        tree_tx: Arc::new(tree_tx),
        snapshot_writer: SnapshotWriter::ephemeral(),
    };
    let prefs = sample_prefs();
    mgr.set_preferences("ws-1".into(), prefs.clone());

    // Reload from disk into a fresh manager — proves the prefs map
    // is round-tripping through `PersistedRegistry`.
    let restored = load_registry_full(&path);
    assert_eq!(restored.workspaces.len(), 0, "no workspaces written");
    assert_eq!(restored.preferences.len(), 1);
    let restored_prefs = restored.preferences.get("ws-1").expect("prefs round-trip");
    assert_eq!(restored_prefs, &prefs);
}

#[test]
fn get_preferences_returns_default_for_unknown_workspace() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let prefs = mgr.get_preferences("never-seen");
    assert_eq!(prefs, WorkplacePreferences::default());
}

#[test]
fn dispatcher_round_trips_preferences() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut conn = ConnectionWorkspace::default();
    let prefs = sample_prefs();
    // SetWorkplacePreferences returns no synchronous reply — the
    // submitter sees the new state through the broadcast subscription
    // wired on the websocket layer.
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::SetWorkplacePreferences {
            workspace_id: "ws-1".into(),
            prefs: prefs.clone(),
        },
    );
    assert!(out.is_empty(), "Set should not emit a direct reply");
    // GetWorkplacePreferences echoes whatever was last stored.
    let out = handle(
        &mgr,
        &mut conn,
        WorkspaceClientMessage::GetWorkplacePreferences {
            workspace_id: "ws-1".into(),
        },
    );
    match out.into_iter().next().expect("reply") {
        WorkspaceServerMessage::WorkplacePreferences {
            workspace_id,
            prefs: got,
        } => {
            assert_eq!(workspace_id, "ws-1");
            assert_eq!(got, prefs);
        }
        other => panic!("unexpected reply {other:?}"),
    }
}

#[tokio::test]
async fn set_preferences_broadcasts_to_subscribers() {
    let td = TempDir::new().unwrap();
    let mgr = make_manager(&td);
    let mut rx_a = mgr.subscribe_preferences();
    let mut rx_b = mgr.subscribe_preferences();
    let prefs = sample_prefs();
    mgr.set_preferences("ws-broadcast".into(), prefs.clone());
    let a = rx_a.recv().await.expect("subscriber a got broadcast");
    let b = rx_b.recv().await.expect("subscriber b got broadcast");
    assert_eq!(a.workspace_id, "ws-broadcast");
    assert_eq!(a.prefs, prefs);
    assert_eq!(b.workspace_id, "ws-broadcast");
    assert_eq!(b.prefs, prefs);
}

#[test]
fn quick_ssh_home_sentinel_resolves_on_the_daemon_host() {
    let Some(home) = dirs::home_dir() else {
        return;
    };
    assert_eq!(expand_workspace_home(PathBuf::from("~")), home);
    assert_eq!(
        expand_workspace_home(PathBuf::from("relative-project")),
        PathBuf::from("relative-project")
    );
}
