use super::*;
use neoism_protocol::workspace::{WorkspaceHostKind, WorkspaceVisibility};

fn real_pty(registry: &crate::sessions::SessionRegistry, manager: &WorkspaceManager, workspace: &str) -> String {
    use neoism_protocol::pty::{ClientMessage, ServerMessage};
    let replies = registry.handle(ClientMessage::CreatePty {
        cwd: None, cols: 80, rows: 24, shell: Some("/bin/sh".into()), shell_args: None, presentation: None,
    });
    let Some(ServerMessage::PtyCreated { session_id, .. }) = replies.first() else { panic!("{replies:?}"); };
    let id = session_id.clone();
    manager.insert_session(SessionSummary { id: id.clone(), workspace_id: workspace.into(), cwd: ".".into(), label: None, last_active: 0 });
    assert!(manager.link_pty_session(&id, id.clone()));
    assert!(registry.link_workspace_session(&id, id.clone()));
    id
}

fn explicit_close(manager: &WorkspaceManager, conn: &mut ConnectionWorkspace, registry: &crate::sessions::SessionRegistry, id: &str, global: bool) -> DispatchOutcome {
    let outcome = super::super::dispatch::handle(manager, conn, None, None, if global {
        WorkspaceClientMessage::CloseHostWorkspace { workspace_id: id.into() }
    } else { WorkspaceClientMessage::CloseWindowWorkspace { workspace_id: id.into() } });
    outcome.retirement.close_ptys(registry);
    outcome
}

#[tokio::test]
async fn explicit_close_retires_real_ptys_surfaces_tabs_and_prevents_late_metadata() {
    use neoism_protocol::pty::{ClientMessage, ServerMessage};
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let registry = crate::sessions::SessionRegistry::new_for_test();
    let a = create(&manager, &td);
    let b = create(&manager, &td); // Same root, distinct identity.
    let mut conn = ConnectionWorkspace::default();
    replace(&manager, &mut conn, &[&a, &b], None);
    conn.active_workspace = Some(a.clone());
    let pty = real_pty(&registry, &manager, &a);
    let surviving = real_pty(&registry, &manager, &b);
    let session = manager.get_session(&pty).unwrap();
    let surface = EditorSurfaceSummary { surface_id: "closed-editor".into(), workspace_id: a.clone(), session_id: pty.clone(), path: None, route_id: None, last_active: 0 };
    manager.upsert_editor_surface(surface.clone());
    let tab = WorkspaceTabSummary {
        id: "closed-tab".into(), workspace_id: a.clone(), title: "terminal".into(),
        kind: None, session_id: Some(pty.clone()), surface_id: None, cwd: None, active: true, last_active: 0,
    };
    manager.publish_workspace_tabs(&a, vec![tab.clone()]);
    manager.set_preferences(a.clone(), WorkplacePreferences::default());
    manager.remember_client_state(&conn);
    let outcome = explicit_close(&manager, &mut conn, &registry, &a, false);
    assert!(matches!(outcome.replies.as_slice(), [WorkspaceServerMessage::HostWorkspaceTree { .. }]));
    assert!(outcome.retirement.surface_ids.contains("closed-editor"));
    assert!(matches!(registry.handle(ClientMessage::AttachPty { session_id: pty.clone(), cursor: None }).first(), Some(ServerMessage::Error { .. })));
    assert!(matches!(registry.handle(ClientMessage::AttachPty { session_id: surviving.clone(), cursor: None }).first(), Some(ServerMessage::PtyCreated { .. })));
    manager.insert_session(session);
    manager.upsert_editor_surface(surface);
    manager.publish_workspace_tabs(&a, vec![tab]);
    manager.set_preferences(a.clone(), WorkplacePreferences::default());
    assert!(!manager.preferences_snapshot().contains_key(&a));
    let replacement = manager.create_host_workspace(machine_host_id(), Some(a.clone()), None, Some(td.path().into()));
    assert_ne!(replacement.id, a, "retired identities cannot be recreated by a late create");
    assert!(!manager.link_pty_session(&pty, "late-pty".into()));
    assert!(manager.workspace_sessions(&a).is_empty());
    assert!(manager.editor_surfaces_for_workspace(&a).is_empty());
    assert!(manager.list_workspace_tabs(&a).is_empty());
    assert!(manager.pane_layout_for_workspace(&a).is_none());
    assert!(!manager.inner.lock().session_pty_links.contains_key(&pty));
    assert!(manager.get_host_workspace(&a).is_none());
    assert!(manager.inner.lock().client_states.values().all(|state| state.active_workspace.as_deref() != Some(&a)));
    assert!(conn.active_workspace.is_none());
    assert!(explicit_close(&manager, &mut conn, &registry, &a, false).retirement.session_ids.is_empty());
    assert!(td.path().is_dir());
    registry.handle(ClientMessage::ClosePty { session_id: surviving });
}

#[tokio::test]
async fn explicit_close_other_connection_detaches_but_global_close_terminates() {
    use neoism_protocol::pty::{ClientMessage, ServerMessage};
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let registry = crate::sessions::SessionRegistry::new_for_test();
    let id = create(&manager, &td);
    let mut first = ConnectionWorkspace::default();
    let mut second = ConnectionWorkspace::default();
    replace(&manager, &mut first, &[&id], None);
    replace(&manager, &mut second, &[&id], None);
    let pty = real_pty(&registry, &manager, &id);
    manager.upsert_editor_surface(EditorSurfaceSummary {
        surface_id: "shared-editor".into(), workspace_id: id.clone(), session_id: pty.clone(),
        path: None, route_id: None, last_active: 0,
    });
    let detached = explicit_close(&manager, &mut first, &registry, &id, false);
    assert!(detached.retirement.session_ids.is_empty());
    assert!(detached.retirement.surface_ids.contains("shared-editor"));
    assert_eq!(manager.editor_surfaces_for_workspace(&id).len(), 1);
    assert!(manager.get_host_workspace(&id).is_some());
    assert!(matches!(registry.handle(ClientMessage::AttachPty { session_id: pty.clone(), cursor: None }).first(), Some(ServerMessage::PtyCreated { .. })));
    explicit_close(&manager, &mut second, &registry, &id, true);
    assert!(matches!(registry.handle(ClientMessage::AttachPty { session_id: pty, cursor: None }).first(), Some(ServerMessage::Error { .. })));
    assert!(manager.inner.lock().workspace_claims.values().all(|claims| !claims.contains(&id)));
}

#[tokio::test]
async fn explicit_close_preserves_shared_foreign_and_recovered_workspaces() {
    use neoism_protocol::pty::{ClientMessage, ServerMessage};
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let registry = crate::sessions::SessionRegistry::new_for_test();
    for reason in 0..4 {
        let id = create(&manager, &td);
        let mut conn = ConnectionWorkspace::default();
        replace(&manager, &mut conn, &[&id], None);
        let pty = real_pty(&registry, &manager, &id);
        {
            let mut inner = manager.inner.lock();
            match reason {
                0 => inner.host_workspaces.get_mut(&id).unwrap().visibility = WorkspaceVisibility::Shared,
                1 => inner.host_workspaces.get_mut(&id).unwrap().host_id = "foreign".into(),
                2 => { inner.workspace_claim_history.remove(&id); }
                3 => inner.host_workspaces.get_mut(&id).unwrap().controlled_by_host_id = Some("foreign".into()),
                _ => unreachable!(),
            }
        }
        let outcome = explicit_close(&manager, &mut conn, &registry, &id, false);
        assert!(outcome.retirement.session_ids.is_empty(), "reason {reason}");
        assert!(manager.get_host_workspace(&id).is_some());
        assert!(manager.inner.lock().workspace_claims.values().all(|claims| !claims.contains(&id)));
        assert!(matches!(registry.handle(ClientMessage::AttachPty { session_id: pty.clone(), cursor: None }).first(), Some(ServerMessage::PtyCreated { .. })));
        registry.handle(ClientMessage::ClosePty { session_id: pty });
    }
}

#[test]
fn explicit_close_unclaimed_is_non_destructive_and_wire_variant_is_distinct() {
    let td = TempDir::new().unwrap();
    let manager = make_manager(&td);
    let id = create(&manager, &td);
    let mut conn = ConnectionWorkspace::default();
    let outcome = super::super::dispatch::handle(&manager, &mut conn, None, None,
        serde_json::from_value(serde_json::json!({"CloseWindowWorkspace": {"workspace_id": id}})).unwrap());
    assert!(outcome.retirement.session_ids.is_empty());
    assert!(manager.get_host_workspace(&id).is_some());
}

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