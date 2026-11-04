use super::*;
use neoism_protocol::workspace::{WorkspaceHostKind, WorkspaceVisibility};

fn create(manager: &WorkspaceManager, td: &TempDir) -> String {
    let workspace = manager.create_host_workspace(
        machine_host_id(), None, None, Some(td.path().into()),
    );
    // No custom vault; avoid depending on another test's notes-home override.
    manager.inner.lock().host_workspaces.get_mut(&workspace.id).unwrap().notes_vault_dir = None;
    workspace.id
}

fn replace(manager: &WorkspaceManager, conn: &mut ConnectionWorkspace, ids: &[&str], prune: Option<&str>) -> Vec<String> {
    let replies = handle(manager, conn, WorkspaceClientMessage::SetWindowWorkspaces {
        workspace_ids: ids.iter().map(|id| (*id).to_owned()).collect(),
        prune_empty: prune.map(str::to_owned),
    });
    match replies.as_slice() {
        [WorkspaceServerMessage::HostWorkspaceTree { workspaces, .. }] => {
            workspaces.iter().map(|workspace| workspace.id.clone()).collect()
        }
        other => panic!("expected authoritative tree, got {other:?}"),
    }
}

#[test]
fn empty_lifecycle_same_client_id_and_inactive_tabs_are_independent_claims() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let a = create(&manager, &td);
    let b = create(&manager, &td);
    let stable = Uuid::new_v4();
    let mut first = ConnectionWorkspace { client_id: stable, ..Default::default() };
    let mut second = ConnectionWorkspace { client_id: stable, ..Default::default() };
    replace(&manager, &mut first, &[&a], None);
    replace(&manager, &mut second, &[&a, &b], None);
    handle(&manager, &mut second, WorkspaceClientMessage::SubscribeWorkspace { workspace_id: b.clone() });
    // A is inactive in the second window, but still open there.
    assert!(replace(&manager, &mut first, &[], Some(&a)).contains(&a));
    assert_eq!(manager.inner.lock().workspace_claims.len(), 2);
    drop(first);
    assert_eq!(manager.inner.lock().workspace_claims.len(), 1);
    // Even after the other socket exits, its offline recovery may have drafts.
    assert!(replace(&manager, &mut second, &[&b], Some(&a)).contains(&a));
    assert_eq!(second.active_workspace.as_deref(), Some(b.as_str()));
}

#[test]
fn empty_lifecycle_disconnect_releases_claims_without_pruning_or_broadcast() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let id = create(&manager, &td);
    let mut conn = ConnectionWorkspace::default();
    replace(&manager, &mut conn, &[&id], None);
    let mut events = manager.subscribe_tree_changes();
    drop(conn);
    assert!(manager.inner.lock().workspace_claims.is_empty());
    assert!(manager.inner.lock().host_workspaces.contains_key(&id));
    assert!(events.try_recv().is_err());
    // A different socket cannot prune a workspace it never held.
    let mut other = ConnectionWorkspace::default();
    assert!(replace(&manager, &mut other, &[], Some(&id)).contains(&id));
}

#[test]
fn empty_lifecycle_subscribe_validates_and_preserves_inactive_legacy_claims() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let a = create(&manager, &td);
    let b = create(&manager, &td);
    let mut legacy = ConnectionWorkspace::default();
    for id in [&a, &b] {
        handle(&manager, &mut legacy, WorkspaceClientMessage::SubscribeWorkspace { workspace_id: id.clone() });
    }
    let replies = handle(&manager, &mut legacy, WorkspaceClientMessage::SubscribeWorkspace { workspace_id: "missing".into() });
    assert!(matches!(replies.as_slice(), [WorkspaceServerMessage::Error { .. }]));
    assert_eq!(legacy.active_workspace.as_deref(), Some(b.as_str()));
    let mut closer = ConnectionWorkspace::default();
    replace(&manager, &mut closer, &[&a], None);
    assert!(replace(&manager, &mut closer, &[], Some(&a)).contains(&a));
    handle(&manager, &mut legacy, WorkspaceClientMessage::UnsubscribeWorkspace { workspace_id: a.clone() });
    replace(&manager, &mut closer, &[&a], None);
    assert!(replace(&manager, &mut closer, &[], Some(&a)).contains(&a));
}

#[test]
fn empty_lifecycle_retain_used_shared_foreign_and_custom_workspaces() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    // Each independent sign of use must be sufficient to retain a workspace.
    for reason in 0..16 {
        let id = create(&manager, &td);
        let mut conn = ConnectionWorkspace::default();
        replace(&manager, &mut conn, &[&id], None);
        {
            let mut inner = manager.inner.lock();
            let workspace = inner.host_workspaces.get_mut(&id).unwrap();
            match reason {
                0 => workspace.visibility = WorkspaceVisibility::Shared,
                1 => workspace.host_id = "foreign".into(),
                2 => workspace.host_kind = WorkspaceHostKind::CloudSandbox,
                3 => workspace.running_on_host_id = Some("foreign".into()),
                4 => workspace.main_session_id = Some("main".into()),
                5 => workspace.controlled_by_host_id = Some(machine_host_id()),
                6 => workspace.active_tab_id = Some("tab".into()),
                7 => workspace.layout_snapshot = Some("{}".into()),
                8 => workspace.linked_vault_dir = Some(td.path().join("custom")),
                9 => workspace.notes_vault_dir = Some(td.path().join("custom")),
                10 => { inner.sessions.insert(id.clone(), SessionSummary {
                    id: id.clone(), workspace_id: id.clone(), cwd: ".".into(), label: None, last_active: 0,
                }); }
                11 => { inner.editor_surfaces.insert(id.clone(), EditorSurfaceSummary {
                    surface_id: id.clone(), workspace_id: id.clone(), session_id: "session".into(), path: None, route_id: None, last_active: 0,
                }); }
                12 => { inner.workspace_tabs.insert(id.clone(), WorkspaceTabSummary {
                    id: id.clone(), workspace_id: id.clone(), title: "tab".into(), kind: None, session_id: None, surface_id: None, cwd: None, active: false, last_active: 0,
                }); }
                13 => { inner.pane_layouts.insert(id.clone(), PaneLayoutSnapshot {
                    schema_version: PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION, workspace_id: id.clone(), focused_pane_external_id: 1,
                    root: PaneLayoutSnapshotNode::Leaf { pane_external_id: 1, surface_id: "surface".into(), session_id: "session".into(), path: None, route_id: None },
                }); }
                14 => { inner.preferences.insert(id.clone(), WorkplacePreferences { session_tree: Some("{}".into()), ..Default::default() }); }
                15 => workspace.visibility = WorkspaceVisibility::Team,
                _ => unreachable!(),
            }
        }
        assert!(replace(&manager, &mut conn, &[], Some(&id)).contains(&id), "reason {reason}");
    }
}

#[test]
fn empty_lifecycle_repeated_create_close_is_bounded_and_clears_active_refs() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let mut conn = ConnectionWorkspace { client_id: Uuid::new_v4(), ..Default::default() };
    for _ in 0..32 {
        let id = create(&manager, &td);
        handle(&manager, &mut conn, WorkspaceClientMessage::SubscribeWorkspace { workspace_id: id.clone() });
        manager.inner.lock().active_workspace_by_host.insert(machine_host_id(), id.clone());
        let mut events = manager.subscribe_tree_changes();
        // Cannot prune a workspace retained in the replacement membership.
        assert!(replace(&manager, &mut conn, &[&id], Some(&id)).contains(&id));
        assert!(events.try_recv().is_err());
        assert!(replace(&manager, &mut conn, &[], Some(&id)).is_empty());
        assert!(events.try_recv().is_ok());
        assert!(conn.active_workspace.is_none());
        let inner = manager.inner.lock();
        assert!(inner.host_workspaces.is_empty());
        assert!(inner.previous_workspace_roots.is_empty());
        assert!(inner.workspace_claim_history.is_empty());
        assert!(inner.active_workspace_by_host.is_empty());
        assert!(inner.client_states.values().all(|state| state.active_workspace.is_none()));
    }
    assert!(td.path().is_dir()); // Metadata only, never delete the root directory.
}

#[test]
fn empty_lifecycle_startup_and_resume_claim_and_wire_default() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let id = create(&manager, &td);
    let mut startup = ConnectionWorkspace { client_id: Uuid::new_v4(), ..Default::default() };
    assert_eq!(manager.resolve_initial_workspace(&mut startup, None).0.id, id);
    manager.remember_client_state(&startup);
    let mut resumed = ConnectionWorkspace { client_id: startup.client_id, ..Default::default() };
    assert_eq!(manager.resume_client_state(&mut resumed).unwrap().active_workspace.as_deref(), Some(id.as_str()));
    assert!(replace(&manager, &mut startup, &[], Some(&id)).contains(&id));
    let message: WorkspaceClientMessage = serde_json::from_str(r#"{"SetWindowWorkspaces":{"workspace_ids":[]}}"#).unwrap();
    assert!(matches!(message, WorkspaceClientMessage::SetWindowWorkspaces { prune_empty: None, .. }));
}

#[test]
fn empty_lifecycle_other_transport_history_survives_unsubscribe_and_disconnect() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    for unsubscribe in [false, true] {
        let id = create(&manager, &td);
        let mut creator = ConnectionWorkspace::default();
        let mut other = ConnectionWorkspace::default();
        replace(&manager, &mut creator, &[&id], None);
        handle(&manager, &mut other, WorkspaceClientMessage::SubscribeWorkspace { workspace_id: id.clone() });
        if unsubscribe {
            handle(&manager, &mut other, WorkspaceClientMessage::UnsubscribeWorkspace { workspace_id: id.clone() });
        }
        drop(other);
        assert!(replace(&manager, &mut creator, &[], Some(&id)).contains(&id));
        assert!(manager.inner.lock().workspace_claim_history[&id].multiple_connections);
    }
}

#[test]
fn empty_lifecycle_recovered_rows_without_creation_provenance_are_retained() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let id = create(&manager, &td);
    // Snapshot hydration deliberately does not recreate disposable provenance.
    manager.inner.lock().workspace_claim_history.clear();
    let mut conn = ConnectionWorkspace::default();
    replace(&manager, &mut conn, &[&id], None);
    assert!(replace(&manager, &mut conn, &[], Some(&id)).contains(&id));
}