use super::*;

fn roundtrip_client(msg: &WorkspaceClientMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: WorkspaceClientMessage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(msg, &back, "roundtrip mismatch: {json}");
}

fn roundtrip_server(msg: &WorkspaceServerMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: WorkspaceServerMessage = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(msg, &back, "roundtrip mismatch: {json}");
}

#[test]
fn client_open_project_root_roundtrip() {
    roundtrip_client(&WorkspaceClientMessage::OpenProjectRoot {
        path: PathBuf::from("/tmp/proj"),
        init_if_missing: true,
    });
    roundtrip_client(&WorkspaceClientMessage::OpenProjectRoot {
        path: PathBuf::from("/tmp/proj"),
        init_if_missing: false,
    });
}

#[test]
fn client_switch_project_root_roundtrip() {
    roundtrip_client(&WorkspaceClientMessage::SwitchProjectRoot {
        id: "root-1".into(),
    });
}

#[test]
fn client_session_lifecycle_roundtrips() {
    roundtrip_client(&WorkspaceClientMessage::NewSession {
        cwd: Some("src".into()),
        label: Some("editor".into()),
    });
    roundtrip_client(&WorkspaceClientMessage::SwitchSession {
        session_id: "s-1".into(),
    });
    roundtrip_client(&WorkspaceClientMessage::SetCwd {
        session_id: "s-1".into(),
        path: "src/foo".into(),
    });
    roundtrip_client(&WorkspaceClientMessage::CloseSession {
        session_id: "s-1".into(),
    });
}

#[test]
fn client_editor_surface_roundtrips() {
    roundtrip_client(&WorkspaceClientMessage::BindEditorSurface {
        surface_id: "pane-1".into(),
        session_id: "s-1".into(),
        path: Some(PathBuf::from("src/main.rs")),
    });
    roundtrip_client(&WorkspaceClientMessage::ListEditorSurfaces);
    roundtrip_client(&WorkspaceClientMessage::CloseEditorSurface {
        surface_id: "pane-1".into(),
    });
}

#[test]
fn client_window_registry_messages_roundtrip() {
    roundtrip_client(&WorkspaceClientMessage::RequestOpenWindow {
        workspace_id: Some("ws-1".into()),
        title: Some("Neoism".into()),
    });
    roundtrip_client(&WorkspaceClientMessage::RequestOpenNativeTab {
        workspace_id: Some("ws-1".into()),
        parent_window_id: Some("win-1".into()),
        title: None,
    });
    roundtrip_client(&WorkspaceClientMessage::RequestOpenConfigEditor {
        workspace_id: None,
    });
    roundtrip_client(&WorkspaceClientMessage::RequestCloseWindow {
        window_id: "win-1".into(),
    });
    roundtrip_client(&WorkspaceClientMessage::ListWindows);
}

#[test]
fn client_workspace_action_and_clipboard_roundtrips() {
    roundtrip_client(&WorkspaceClientMessage::RunWorkspaceAction {
        action: WorkspaceAction::CreateNeoismNote,
    });
    roundtrip_client(&WorkspaceClientMessage::StoreClipboard {
        payload: ClipboardPayload {
            mime_type: "image/png".into(),
            text: None,
            bytes: vec![1, 2, 3],
            filename: Some("shot.png".into()),
        },
    });
    roundtrip_client(&WorkspaceClientMessage::LoadClipboard);
    roundtrip_client(&WorkspaceClientMessage::MaterializeClipboardImage {
        payload: ClipboardPayload {
            mime_type: "image/png".into(),
            text: None,
            bytes: vec![137, 80, 78, 71],
            filename: Some("paste.png".into()),
        },
        request_id: Some("pane-3:42".into()),
    });
    roundtrip_client(&WorkspaceClientMessage::MaterializeClipboardImage {
        payload: ClipboardPayload {
            mime_type: "image/jpeg".into(),
            text: None,
            bytes: vec![255, 216, 255, 224],
            filename: None,
        },
        request_id: None,
    });
}

#[test]
fn server_clipboard_image_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::ClipboardImageMaterialized {
        path: PathBuf::from("/tmp/neoism/clipboard/paste-123.png"),
        mime_type: "image/png".into(),
        filename: Some("paste.png".into()),
        request_id: Some("pane-3:42".into()),
    });
    roundtrip_server(&WorkspaceServerMessage::ClipboardImageMaterialized {
        path: PathBuf::from("/tmp/neoism/clipboard/paste-456.bin"),
        mime_type: "application/octet-stream".into(),
        filename: None,
        request_id: None,
    });
}

#[test]
fn server_workspace_list_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::ProjectRootList {
        project_roots: vec![ProjectRootSummary {
            id: "ws-1".into(),
            name: "neoism".into(),
            path: PathBuf::from("/tmp/proj"),
            last_opened: 1_700_000_000,
        }],
    });
}

#[test]
fn server_workspace_info_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::ProjectRootInfo {
        id: "ws-1".into(),
        name: "neoism".into(),
        path: PathBuf::from("/tmp/proj"),
        sessions: vec![SessionSummary {
            id: "s-1".into(),
            workspace_id: "ws-1".into(),
            cwd: ".".into(),
            label: None,
            last_active: 0,
        }],
        active: true,
    });
}

#[test]
fn server_session_state_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::SessionState {
        id: "s-1".into(),
        workspace_id: "ws-1".into(),
        cwd: "src".into(),
        label: Some("editor".into()),
        last_active: 1_700_000_000,
    });
}

#[test]
fn server_editor_surface_roundtrips() {
    let surface = EditorSurfaceSummary {
        surface_id: "pane-1".into(),
        workspace_id: "ws-1".into(),
        session_id: "s-1".into(),
        path: Some(PathBuf::from("src/main.rs")),
        route_id: Some(7),
        last_active: 1_700_000_000,
    };
    roundtrip_server(&WorkspaceServerMessage::EditorSurfaceList {
        surfaces: vec![surface.clone()],
    });
    roundtrip_server(&WorkspaceServerMessage::EditorSurfaceChanged { surface });
    roundtrip_server(&WorkspaceServerMessage::EditorSurfaceClosed {
        surface_id: "pane-1".into(),
    });
}

#[test]
fn server_window_registry_messages_roundtrip() {
    let window = WorkspaceWindowSummary {
        id: "win-1".into(),
        kind: WorkspaceWindowKind::Terminal,
        workspace_id: Some("ws-1".into()),
        parent_window_id: None,
        title: "Neoism".into(),
        route_id: Some(9),
        created_at: 1_700_000_000,
        last_active: 1_700_000_001,
    };
    roundtrip_server(&WorkspaceServerMessage::WindowList {
        windows: vec![window.clone()],
    });
    roundtrip_server(&WorkspaceServerMessage::WindowOpened {
        window: window.clone(),
    });
    roundtrip_server(&WorkspaceServerMessage::WindowChanged { window });
    roundtrip_server(&WorkspaceServerMessage::WindowClosed {
        window_id: "win-1".into(),
    });
}

#[test]
fn editor_surface_route_id_defaults_to_none_for_old_daemons() {
    // Older daemons serialize EditorSurfaceSummary without
    // `route_id`. The chrome's `coerceServerMessage` switch sees
    // the same shape; deserialization must succeed and yield
    // `None` so the connection-level fallback (the legacy
    // `ACTIVE_EDITOR_ROUTE_ID = 1`) can kick in.
    let json = r#"{"surface_id":"pane-1","workspace_id":"ws-1","session_id":"s-1","path":null,"last_active":0}"#;
    let surface: EditorSurfaceSummary =
        serde_json::from_str(json).expect("backcompat deserialize");
    assert_eq!(surface.route_id, None);
}

#[test]
fn client_pane_layout_op_roundtrips() {
    roundtrip_client(&WorkspaceClientMessage::PaneLayoutOp {
        pane_external_id: 7,
        op: PaneLayoutOp::Split {
            axis: PaneSplitAxis::Vertical,
            placement: PaneSplitPlacement::After,
        },
    });
    roundtrip_client(&WorkspaceClientMessage::PaneLayoutOp {
        pane_external_id: 7,
        op: PaneLayoutOp::Focus {
            dir: PaneFocusDir::Right,
        },
    });
    roundtrip_client(&WorkspaceClientMessage::PaneLayoutOp {
        pane_external_id: 7,
        op: PaneLayoutOp::Close,
    });
    roundtrip_client(&WorkspaceClientMessage::PaneLayoutOp {
        pane_external_id: 7,
        op: PaneLayoutOp::ResizeRatio { delta: 0.125 },
    });
    roundtrip_client(&WorkspaceClientMessage::PaneLayoutOp {
        pane_external_id: 7,
        op: PaneLayoutOp::MoveTab { from: 0, to: 2 },
    });
}

#[test]
fn server_pane_layout_changed_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::PaneLayoutChanged {
        pane_external_id: 11,
        op: PaneLayoutOp::Split {
            axis: PaneSplitAxis::Horizontal,
            placement: PaneSplitPlacement::Before,
        },
        new_layout_snapshot: None,
    });
    roundtrip_server(&WorkspaceServerMessage::PaneLayoutChanged {
        pane_external_id: 11,
        op: PaneLayoutOp::Close,
        new_layout_snapshot: Some("{\"root\":\"...\"}".into()),
    });
}

#[test]
fn pane_layout_snapshot_shape_roundtrips() {
    let snapshot = PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws-1".into(),
        focused_pane_external_id: 7,
        root: PaneLayoutSnapshotNode::Tabs {
            active: 1,
            children: vec![
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 5,
                    surface_id: "5".into(),
                    session_id: "s-1".into(),
                    path: Some(PathBuf::from("src/lib.rs")),
                    route_id: Some(1),
                },
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 7,
                    surface_id: "7".into(),
                    session_id: "s-2".into(),
                    path: None,
                    route_id: Some(2),
                },
            ],
        },
    };

    let json = serde_json::to_string(&snapshot).expect("serialize");
    let back: PaneLayoutSnapshot = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(snapshot, back);
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["root"]["kind"], "tabs");
    assert_eq!(value["root"]["children"][1]["kind"], "leaf");
}

#[test]
fn pane_layout_snapshot_builds_from_surfaces_and_applies_ops() {
    let mut snapshot = PaneLayoutSnapshot::from_editor_surfaces(
        "ws-1",
        7,
        vec![
            EditorSurfaceSummary {
                surface_id: "7".into(),
                workspace_id: "ws-1".into(),
                session_id: "s-1".into(),
                route_id: Some(1),
                path: Some(PathBuf::from("src/lib.rs")),
                last_active: 1,
            },
            EditorSurfaceSummary {
                surface_id: "9".into(),
                workspace_id: "ws-1".into(),
                session_id: "s-2".into(),
                route_id: Some(2),
                path: None,
                last_active: 2,
            },
        ],
    )
    .expect("snapshot");

    assert_eq!(snapshot.focused_pane_external_id, 7);
    assert_eq!(snapshot.external_ids_in_order(), vec![7, 9]);
    assert!(matches!(
        &snapshot.root,
        PaneLayoutSnapshotNode::Tabs { active: 0, children } if children.len() == 2
    ));

    assert!(snapshot.apply_op(
        7,
        PaneLayoutOp::Focus {
            dir: PaneFocusDir::Right,
        },
    ));
    assert_eq!(snapshot.focused_pane_external_id, 9);
    assert!(matches!(
        &snapshot.root,
        PaneLayoutSnapshotNode::Tabs { active: 1, .. }
    ));

    assert!(snapshot.apply_op(
        7,
        PaneLayoutOp::Split {
            axis: PaneSplitAxis::Vertical,
            placement: PaneSplitPlacement::After,
        },
    ));
    assert_eq!(snapshot.focused_pane_external_id, 10);
    assert_eq!(snapshot.external_ids_in_order(), vec![7, 10, 9]);

    assert!(snapshot.apply_op(7, PaneLayoutOp::ResizeRatio { delta: -0.1 }));
    match &snapshot.root {
        PaneLayoutSnapshotNode::Tabs { children, .. } => match &children[0] {
            PaneLayoutSnapshotNode::Split { ratios, .. } => {
                assert_eq!(ratios, &vec![0.4]);
            }
            other => panic!("unexpected split child {other:?}"),
        },
        other => panic!("unexpected root {other:?}"),
    }

    assert!(snapshot.apply_op(7, PaneLayoutOp::Close));
    assert_eq!(snapshot.focused_pane_external_id, 10);
    assert!(!snapshot.contains_external_id(7));
    assert!(snapshot.contains_external_id(10));
}

#[test]
fn pane_layout_snapshot_upsert_preserves_geometry() {
    let mut snapshot = PaneLayoutSnapshot::from_editor_surfaces(
        "ws-1",
        1,
        vec![EditorSurfaceSummary {
            surface_id: "1".into(),
            workspace_id: "ws-1".into(),
            session_id: "s-1".into(),
            route_id: Some(1),
            path: None,
            last_active: 1,
        }],
    )
    .expect("snapshot");

    assert!(snapshot.apply_op(
        1,
        PaneLayoutOp::Split {
            axis: PaneSplitAxis::Horizontal,
            placement: PaneSplitPlacement::After,
        },
    ));
    assert!(snapshot.upsert_surface(EditorSurfaceSummary {
        surface_id: "2".into(),
        workspace_id: "ws-1".into(),
        session_id: "s-2".into(),
        route_id: Some(99),
        path: Some(PathBuf::from("src/main.rs")),
        last_active: 2,
    }));

    match &snapshot.root {
        PaneLayoutSnapshotNode::Split {
            axis,
            ratios,
            children,
        } => {
            assert_eq!(*axis, PaneSplitAxis::Horizontal);
            assert_eq!(ratios, &vec![0.5]);
            assert!(matches!(
                &children[1],
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 2,
                    session_id,
                    route_id: Some(99),
                    path: Some(path),
                    ..
                } if session_id == "s-2" && path == &PathBuf::from("src/main.rs")
            ));
        }
        other => panic!("unexpected root {other:?}"),
    }
}

#[test]
fn editor_surface_pane_id_helpers_keep_route_id_separate() {
    let surface = EditorSurfaceSummary {
        surface_id: "42".into(),
        workspace_id: "ws-1".into(),
        session_id: "s-1".into(),
        route_id: Some(7),
        path: None,
        last_active: 1,
    };

    assert_eq!(pane_external_id_from_surface_id("42"), Some(42));
    assert_eq!(pane_external_id_from_surface_id("pane-42"), None);
    assert_eq!(surface_id_for_pane_external_id(42), "42");
    assert_eq!(editor_surface_pane_external_id(&surface), Some(42));

    let snapshot = PaneLayoutSnapshot::from_editor_surfaces("ws-1", 42, vec![surface])
        .expect("snapshot");
    match snapshot.root {
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id,
            route_id,
            ..
        } => {
            assert_eq!(pane_external_id, 42);
            assert_eq!(route_id, Some(7));
        }
        other => panic!("unexpected root {other:?}"),
    }
}

#[test]
fn pane_layout_snapshot_normalize_collapses_and_refocuses() {
    let mut snapshot = PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws-1".into(),
        focused_pane_external_id: 99,
        root: PaneLayoutSnapshotNode::Tabs {
            active: 8,
            children: vec![PaneLayoutSnapshotNode::Split {
                axis: PaneSplitAxis::Vertical,
                ratios: vec![0.25, 0.75],
                children: vec![PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 3,
                    surface_id: "3".into(),
                    session_id: "s-1".into(),
                    path: None,
                    route_id: Some(11),
                }],
            }],
        },
    };

    snapshot.normalize();

    assert_eq!(snapshot.focused_pane_external_id, 3);
    assert!(matches!(
        snapshot.root,
        PaneLayoutSnapshotNode::Leaf {
            pane_external_id: 3,
            route_id: Some(11),
            ..
        }
    ));
}

#[test]
fn pane_layout_snapshot_normalize_rebalances_split_ratios() {
    let mut snapshot = PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws-1".into(),
        focused_pane_external_id: 5,
        root: PaneLayoutSnapshotNode::Split {
            axis: PaneSplitAxis::Horizontal,
            ratios: vec![0.9],
            children: vec![
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 5,
                    surface_id: "5".into(),
                    session_id: "s-1".into(),
                    path: None,
                    route_id: None,
                },
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 7,
                    surface_id: "7".into(),
                    session_id: "s-2".into(),
                    path: None,
                    route_id: None,
                },
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 9,
                    surface_id: "9".into(),
                    session_id: "s-3".into(),
                    path: None,
                    route_id: None,
                },
            ],
        },
    };

    snapshot.normalize();

    match snapshot.root {
        PaneLayoutSnapshotNode::Split { ratios, .. } => {
            assert_eq!(ratios, vec![1.0 / 3.0, 2.0 / 3.0]);
        }
        other => panic!("unexpected root {other:?}"),
    }
}

#[test]
fn server_workspace_action_and_clipboard_roundtrips() {
    roundtrip_server(&WorkspaceServerMessage::WorkspaceActionCompleted {
        action: WorkspaceAction::CreateNeoismNote,
        path: Some(PathBuf::from("/tmp/proj")),
        message: "created".into(),
    });
    roundtrip_server(&WorkspaceServerMessage::ClipboardPayload {
        payload: Some(ClipboardPayload {
            mime_type: "text/plain".into(),
            text: Some("hello".into()),
            bytes: Vec::new(),
            filename: None,
        }),
    });
}

#[test]
fn server_workspace_changed_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::ProjectRootChanged {
        id: Some("ws-1".into()),
    });
    roundtrip_server(&WorkspaceServerMessage::ProjectRootChanged { id: None });
}

#[test]
fn server_error_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::Error {
        message: "no active workspace".into(),
    });
}

/// Sanity-check that the external-tagging shape matches the rest
/// of the protocol — the web `ProtocolClient.coerceServerMessage`
/// switch relies on the top-level JSON being a single-key object
/// whose key is the variant name.
#[test]
fn server_messages_are_externally_tagged() {
    let json = serde_json::to_string(&WorkspaceServerMessage::ProjectRootClosed {
        id: "ws-1".into(),
    })
    .unwrap();
    assert!(
        json.starts_with("{\"ProjectRootClosed\""),
        "expected externally-tagged JSON, got: {json}"
    );
}

#[test]
fn generated_task_update_uses_source_file_link() {
    let update = parse_generated_task_update(
        "- [x] Ship it [[Other]] [[/tmp/neoism-note.md-42|note.md:42]]",
    )
    .unwrap();

    assert_eq!(update.path, PathBuf::from("/tmp/neoism-note.md"));
    assert_eq!(update.line, 42);
    assert!(update.checked);
}

#[test]
fn generated_task_update_keeps_source_before_trailing_broken_link() {
    let update = parse_generated_task_update(
        "- [x] Ship it [[/tmp/neoism-note.md-42|note.md:42]] [[unfinished",
    )
    .unwrap();

    assert_eq!(update.path, PathBuf::from("/tmp/neoism-note.md"));
    assert_eq!(update.line, 42);
    assert!(update.checked);
}

#[test]
fn generated_task_update_uses_hidden_source_marker() {
    let marker = generated_task_source_marker(Path::new("/tmp/neoism note.md"), 42);
    let update = parse_generated_task_update(&format!("- [ ] Ship it {marker}")).unwrap();

    assert_eq!(update.path, PathBuf::from("/tmp/neoism note.md"));
    assert_eq!(update.line, 42);
    assert!(!update.checked);
}

#[test]
fn generated_task_update_clamps_hidden_source_line() {
    let marker = generated_task_source_marker(Path::new("/tmp/neoism note.md"), 0);
    let update = parse_generated_task_update(&format!("- [X] Ship it {marker}")).unwrap();

    assert_eq!(update.line, 1);
    assert!(update.checked);
}

#[test]
fn client_hello_roundtrips_and_tolerates_missing_fields() {
    // Fully populated handshake (post-G2: includes a real
    // `client_id` for a returning client).
    roundtrip_client(&WorkspaceClientMessage::Hello {
        token: Some("pair-abc123".into()),
        client_name: Some("neoism-desktop".into()),
        client_id: Uuid::from_u128(0x1234_5678_9abc_def0_1122_3344_5566_7788),
    });
    // No client name — legitimate when the client doesn't want to
    // self-identify in audit logs.
    roundtrip_client(&WorkspaceClientMessage::Hello {
        token: Some("pair-abc123".into()),
        client_name: None,
        client_id: Uuid::nil(),
    });
    // No token — legitimate when the daemon allows trust-local
    // connections (NEOISM_REQUIRE_AUTH unset).
    roundtrip_client(&WorkspaceClientMessage::Hello {
        token: None,
        client_name: Some("neoism-desktop".into()),
        client_id: Uuid::nil(),
    });
    // Empty handshake — should still roundtrip; this is what an
    // older client that just learned the variant exists might emit
    // before it knows how to read its local token file or its
    // persisted client_id.
    roundtrip_client(&WorkspaceClientMessage::Hello {
        token: None,
        client_name: None,
        client_id: Uuid::nil(),
    });
}

#[test]
fn server_hello_ack_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::HelloAck {
        accepted: true,
        reason: None,
        peer_identity: Some("you@tailnet".into()),
    });
    roundtrip_server(&WorkspaceServerMessage::HelloAck {
        accepted: false,
        reason: Some("invalid pairing token".into()),
        peer_identity: None,
    });
    // Bare-minimum positive ack — both optional fields elided.
    roundtrip_server(&WorkspaceServerMessage::HelloAck {
        accepted: true,
        reason: None,
        peer_identity: None,
    });
}

#[test]
fn hello_missing_fields_deserialize_from_old_clients() {
    // A pre-handshake client never sent `Hello`, but a transitional
    // client might emit just `{"Hello": {}}` if it knows the wire
    // shape but hasn't yet plumbed token/client_name/client_id. The
    // `#[serde(default)]` annotations must make this parse — and
    // the missing `client_id` field must default to the all-zero
    // UUID so the daemon treats the connection as a fresh client
    // and mints a new id.
    let parsed: WorkspaceClientMessage =
        serde_json::from_str(r#"{"Hello":{}}"#).expect("default fields");
    match parsed {
        WorkspaceClientMessage::Hello {
            token,
            client_name,
            client_id,
        } => {
            assert!(token.is_none());
            assert!(client_name.is_none());
            assert_eq!(client_id, Uuid::nil());
        }
        other => panic!("expected Hello, got {other:?}"),
    }
}

#[test]
fn hello_ack_missing_fields_deserialize_from_old_daemons() {
    let parsed: WorkspaceServerMessage =
        serde_json::from_str(r#"{"HelloAck":{"accepted":true}}"#)
            .expect("default fields");
    match parsed {
        WorkspaceServerMessage::HelloAck {
            accepted,
            reason,
            peer_identity,
        } => {
            assert!(accepted);
            assert!(reason.is_none());
            assert!(peer_identity.is_none());
        }
        other => panic!("expected HelloAck, got {other:?}"),
    }
}

#[test]
fn generated_task_save_toggles_source_checkbox_only() {
    let mut line = "  - [ ] Ship it #neoism".to_string();
    assert!(set_task_line_checked(&mut line, true));
    assert_eq!(line, "  - [x] Ship it #neoism");
    assert!(!set_task_line_checked(&mut line, true));
    assert!(set_task_line_checked(&mut line, false));
    assert_eq!(line, "  - [ ] Ship it #neoism");
}

#[test]
fn client_pairing_messages_roundtrip() {
    roundtrip_client(&WorkspaceClientMessage::ListPairings);
    roundtrip_client(&WorkspaceClientMessage::RevokePairing {
        fingerprint_prefix: "abc123def456".into(),
    });
}

#[test]
fn server_pairing_messages_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::PairingList {
        pairings: vec![
            PairingSummary {
                device_label: Some("laptop-a".into()),
                last_seen: Some(1_700_000_000),
                fingerprint_prefix: "abc123def456".into(),
                created_at: 1_699_000_000,
            },
            PairingSummary {
                device_label: None,
                last_seen: None,
                fingerprint_prefix: "0123deadbeef".into(),
                created_at: 0,
            },
        ],
    });
    roundtrip_server(&WorkspaceServerMessage::PairingRevoked {
        fingerprint_prefix: "abc123def456".into(),
        removed: true,
    });
    roundtrip_server(&WorkspaceServerMessage::PairingRevoked {
        fingerprint_prefix: "ffff00001111".into(),
        removed: false,
    });
}

#[test]
fn pairing_summary_serialization_omits_raw_token_field() {
    // Belt-and-suspenders: serializing a `PairingSummary` must not
    // include anything that looks like a raw `pair-...` token. The
    // shape itself doesn't carry the token, but this test wires that
    // promise into CI so a future refactor can't accidentally add
    // one (e.g. through an over-broad `#[serde(flatten)]` of an
    // internal entry struct).
    let summary = PairingSummary {
        device_label: Some("laptop-a".into()),
        last_seen: Some(1),
        fingerprint_prefix: "abc123def456".into(),
        created_at: 1,
    };
    let json = serde_json::to_string(&summary).unwrap();
    assert!(!json.contains("\"token\""), "leaked token field: {json}");
    assert!(!json.contains("pair-"), "leaked raw pair- value: {json}");
}

#[test]
fn client_workplace_preferences_messages_roundtrip() {
    roundtrip_client(&WorkspaceClientMessage::GetWorkplacePreferences {
        workspace_id: "ws-1".into(),
    });
    let mut sidebar = HashMap::new();
    sidebar.insert("file_tree".into(), 280.0);
    roundtrip_client(&WorkspaceClientMessage::SetWorkplacePreferences {
        workspace_id: "ws-1".into(),
        prefs: WorkplacePreferences {
            theme: Some("solarized-dark".into()),
            font_size: Some(14.5),
            sidebar_widths: sidebar,
            session_tree: Some("{\"v\":1}".into()),
        },
    });
}

#[test]
fn server_workplace_preferences_messages_roundtrip() {
    roundtrip_server(&WorkspaceServerMessage::WorkplacePreferences {
        workspace_id: "ws-1".into(),
        prefs: WorkplacePreferences::default(),
    });
    roundtrip_server(&WorkspaceServerMessage::WorkplacePreferencesChanged {
        workspace_id: "ws-1".into(),
        prefs: WorkplacePreferences {
            theme: Some("nord".into()),
            ..WorkplacePreferences::default()
        },
    });
}

// ----------------------------------------------------------------
// G2 — snapshot/resume additions:
//   * `WorkspaceClientMessage::RequestFullSnapshot`
//   * `WorkspaceServerMessage::FullSnapshot`
//   * `WorkspaceServerMessage::PtyBacklog`
//   * `Hello.client_id` round-trip + default
// ----------------------------------------------------------------

#[test]
fn client_request_full_snapshot_roundtrips() {
    roundtrip_client(&WorkspaceClientMessage::RequestFullSnapshot { since_offset: None });
    roundtrip_client(&WorkspaceClientMessage::RequestFullSnapshot {
        since_offset: Some(0),
    });
    roundtrip_client(&WorkspaceClientMessage::RequestFullSnapshot {
        since_offset: Some(4096),
    });
}

#[test]
fn request_full_snapshot_tolerates_missing_since_offset() {
    // A client implementation that omits the optional field still
    // parses cleanly thanks to `#[serde(default)]`.
    let parsed: WorkspaceClientMessage =
        serde_json::from_str(r#"{"RequestFullSnapshot":{}}"#)
            .expect("default since_offset");
    match parsed {
        WorkspaceClientMessage::RequestFullSnapshot { since_offset } => {
            assert!(since_offset.is_none());
        }
        other => panic!("expected RequestFullSnapshot, got {other:?}"),
    }
}

#[test]
fn server_full_snapshot_roundtrips() {
    let mut prefs_map = HashMap::new();
    prefs_map.insert(
        "ws-1".to_string(),
        WorkplacePreferences {
            theme: Some("nord".into()),
            ..WorkplacePreferences::default()
        },
    );
    let mut offsets = HashMap::new();
    offsets.insert(1u64, 4096u64);
    offsets.insert(2u64, 0u64);

    let layout = PaneLayoutSnapshot {
        schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "ws-1".into(),
        focused_pane_external_id: 7,
        root: PaneLayoutSnapshotNode::Leaf {
            pane_external_id: 7,
            surface_id: "7".into(),
            session_id: "s-1".into(),
            path: Some(PathBuf::from("src/main.rs")),
            route_id: Some(1),
        },
    };

    roundtrip_server(&WorkspaceServerMessage::FullSnapshot {
        client_id: Uuid::from_u128(0xdead_beef_cafe_babe_1122_3344_5566_7788),
        sessions: vec![SessionSummary {
            id: "s-1".into(),
            workspace_id: "ws-1".into(),
            cwd: "src".into(),
            label: Some("editor".into()),
            last_active: 1_700_000_000,
        }],
        layout: Some(layout),
        prefs: prefs_map.clone(),
        pty_offsets: offsets.clone(),
    });

    // Empty / cold-start snapshot — no workspace active, no
    // sessions, no layout, no prefs, no PTYs yet.
    roundtrip_server(&WorkspaceServerMessage::FullSnapshot {
        client_id: Uuid::nil(),
        sessions: Vec::new(),
        layout: None,
        prefs: HashMap::new(),
        pty_offsets: HashMap::new(),
    });
}

#[test]
fn server_pty_backlog_roundtrips() {
    roundtrip_server(&WorkspaceServerMessage::PtyBacklog {
        route_id: 7,
        bytes: b"hello\r\n$ ".to_vec(),
        from_offset: 1024,
    });
    // Empty backlog is a legal "no replay needed" reply — the
    // daemon may emit one to confirm the client is caught up.
    roundtrip_server(&WorkspaceServerMessage::PtyBacklog {
        route_id: 1,
        bytes: Vec::new(),
        from_offset: 0,
    });
}

#[test]
fn full_snapshot_externally_tagged_shape() {
    // Cross-check the JSON shape stays externally tagged so the
    // chrome's single-key dispatch in
    // `ProtocolClient.coerceServerMessage` continues to recognise
    // the new variants.
    let json = serde_json::to_string(&WorkspaceServerMessage::FullSnapshot {
        client_id: Uuid::nil(),
        sessions: Vec::new(),
        layout: None,
        prefs: HashMap::new(),
        pty_offsets: HashMap::new(),
    })
    .unwrap();
    assert!(
        json.starts_with("{\"FullSnapshot\""),
        "expected externally-tagged JSON, got: {json}"
    );

    let json = serde_json::to_string(&WorkspaceServerMessage::PtyBacklog {
        route_id: 3,
        bytes: vec![1, 2, 3],
        from_offset: 99,
    })
    .unwrap();
    assert!(
        json.starts_with("{\"PtyBacklog\""),
        "expected externally-tagged JSON, got: {json}"
    );
}
