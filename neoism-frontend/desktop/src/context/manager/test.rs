use super::*;
use crate::daemon_client::{
    DaemonClient, DaemonClientOptions, DaemonEndpoint, ReconnectBackoff,
};
use crate::event::VoidListener;
use neoism_protocol::workspace::{
    PaneFocusDir, PaneLayoutOp, PaneLayoutSnapshotNode, WorkspaceServerMessage,
};
use std::time::Duration;

fn attach_unconnected_daemon(
    context_manager: &mut ContextManager<VoidListener>,
) -> tokio::runtime::Runtime {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let socket_path = format!("/tmp/neoism-l3-test-{}.sock", Uuid::new_v4());
    let endpoint = DaemonEndpoint::Unix {
        path: std::path::PathBuf::from(&socket_path),
    };
    let endpoint_str = format!("unix://{socket_path}");
    let mut options = DaemonClientOptions::new(endpoint);
    options.reconnect = ReconnectBackoff {
        initial: Duration::from_secs(60 * 60),
        max: Duration::from_secs(60 * 60),
    };
    let client = runtime
        .block_on(DaemonClient::connect_with_options(options))
        .unwrap();
    context_manager.attach_daemon_client_with_runtime(
        client.handle(),
        runtime.handle().clone(),
        endpoint_str,
        None,
        true,
    );
    runtime
}

fn session(id: &str) -> SessionSummary {
    SessionSummary {
        id: id.to_string(),
        workspace_id: "workspace-a".to_string(),
        cwd: ".".to_string(),
        label: None,
        last_active: 0,
    }
}

fn snapshot(focused_pane_external_id: u64) -> PaneLayoutSnapshot {
    PaneLayoutSnapshot {
        schema_version: neoism_protocol::workspace::PANE_LAYOUT_SNAPSHOT_SCHEMA_VERSION,
        workspace_id: "workspace-a".to_string(),
        focused_pane_external_id,
        root: PaneLayoutSnapshotNode::Tabs {
            active: 1,
            children: vec![
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 11,
                    surface_id: "11".to_string(),
                    session_id: "session-a".to_string(),
                    path: None,
                    route_id: Some(11),
                },
                PaneLayoutSnapshotNode::Leaf {
                    pane_external_id: 22,
                    surface_id: "22".to_string(),
                    session_id: "session-b".to_string(),
                    path: None,
                    route_id: Some(22),
                },
            ],
        },
    }
}

#[test]
fn apply_full_snapshot_replaces_daemon_cache() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let client_id = Uuid::new_v4();

    assert!(context_manager.apply_full_snapshot(
        client_id,
        vec![session("session-a"), session("session-b")],
        Some(snapshot(22)),
        HashMap::new(),
        HashMap::new(),
    ));

    assert_eq!(context_manager.daemon_cache().client_id, Some(client_id));
    assert_eq!(context_manager.sessions().len(), 2);
    assert_eq!(
        context_manager.cached_active_session_id(),
        Some("session-b")
    );
    assert_eq!(
        context_manager
            .cached_layout()
            .map(|layout| layout.focused_pane_external_id),
        Some(22)
    );
}

#[test]
fn adopted_workspace_identity_survives_active_server_cache_reset() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let stable = context_manager
        .current_grid()
        .workspace_route_id()
        .expect("test grid has a stable root");
    context_manager.adopted_workspaces.insert(
        stable,
        AdoptedWorkspaceBinding {
            workspace_id: "peer-workspace".to_string(),
            endpoint: "ws://100.64.0.8:9877/session".to_string(),
            credential: Some("peer-secret".to_string()),
            is_peer: true,
        },
    );

    // This is what attaching server B used to do to server A's grid.
    context_manager.detach_daemon_client();

    assert_eq!(
        context_manager.current_adopted_workspace_id().as_deref(),
        Some("peer-workspace")
    );
    assert_eq!(
        context_manager.current_adopted_workspace_endpoint(),
        Some("ws://100.64.0.8:9877/session")
    );
    assert!(context_manager.current_workspace_is_remote_joined());
    assert_eq!(
        context_manager
            .agent_server_override_for_current()
            .as_deref(),
        Some("http://100.64.0.8:9877/agent/workspaces/peer-workspace")
    );
    assert_eq!(
        context_manager.workspace_icon_kind_for_index(0).as_deref(),
        Some("joined")
    );
    assert!(!context_manager.current_workspace_is_quick_ssh());
}

#[test]
fn quick_ssh_workspace_is_distinct_from_shared_remote_join() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let stable = context_manager
        .current_grid()
        .workspace_route_id()
        .expect("test grid has a stable root");
    context_manager.adopted_workspaces.insert(
        stable,
        AdoptedWorkspaceBinding {
            workspace_id: format!(
                "{}-0123456789abcdef",
                crate::ssh_hosts::QUICK_SSH_WORKSPACE_ID
            ),
            endpoint: "ws://127.0.0.1:43210/session".to_string(),
            credential: Some("ssh-secret".to_string()),
            is_peer: true,
        },
    );

    assert!(context_manager.current_workspace_is_remote_joined());
    assert!(context_manager.current_workspace_is_quick_ssh());
}

#[test]
fn adopted_workspace_rebind_updates_the_workspace_scoped_agent_endpoint() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let stable = context_manager
        .current_grid()
        .workspace_route_id()
        .expect("test grid has a stable root");
    context_manager.adopted_workspaces.insert(
        stable,
        AdoptedWorkspaceBinding {
            workspace_id: "quick-ssh".to_string(),
            endpoint: "ws://127.0.0.1:1/session".to_string(),
            credential: Some("stale".to_string()),
            is_peer: true,
        },
    );
    let _runtime = attach_unconnected_daemon(&mut context_manager);
    let fresh_endpoint = "ws://127.0.0.1:43210/session";
    if let Some(link) = context_manager.daemon.link.as_mut() {
        link.endpoint = fresh_endpoint.to_string();
        link.credential = Some("fresh".to_string());
    }
    context_manager.daemon.link_is_peer = true;

    context_manager.rebind_adopted_workspace_at(0, "quick-ssh");

    let rebound = context_manager
        .adopted_workspaces
        .get(&stable)
        .expect("adopted binding remains present");
    assert_eq!(rebound.endpoint, fresh_endpoint);
    assert_eq!(rebound.credential.as_deref(), Some("fresh"));
    assert!(rebound.is_peer);
    assert_eq!(
        context_manager
            .agent_server_override_for_current()
            .as_deref(),
        Some("http://127.0.0.1:43210/agent/workspaces/quick-ssh")
    );
}

#[test]
fn joined_workspace_icon_survives_missing_terminal_title() {
    // Validate the same fallback fields used by the Island bridge.

    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let stable = context_manager
        .current_grid()
        .workspace_route_id()
        .expect("test grid has a stable root");
    context_manager.adopted_workspaces.insert(
        stable,
        AdoptedWorkspaceBinding {
            workspace_id: "peer-workspace".to_string(),
            endpoint: "ws://peer.example/session".to_string(),
            credential: None,
            is_peer: true,
        },
    );
    context_manager.titles.titles.remove(&0);

    let content = context_manager
        .titles
        .titles
        .get(&0)
        .map(|title| title.content.clone())
        .unwrap_or_else(|| "~".to_string());
    assert_eq!(content, "~");
    assert_eq!(
        context_manager.workspace_icon_kind_for_index(0).as_deref(),
        Some("joined")
    );
}

#[test]
fn unfocused_hosted_workspace_keeps_its_network_icon_without_active_server_cache() {
    use neoism_protocol::workspace::{
        WorkspaceHostKind, WorkspaceSummary, WorkspaceVisibility,
    };

    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    assert!(context_manager.add_context_with_working_dir(false, 1, None));
    assert_eq!(context_manager.current_index(), 0);

    let hosted_id = context_manager
        .workspace_tree_id_for_index(1)
        .expect("second workspace has a stable id");
    context_manager.upsert_daemon_host_workspace(WorkspaceSummary {
        id: hosted_id,
        host_id: context_manager.local_host_id(),
        title: "Hosted in background".to_string(),
        host_kind: WorkspaceHostKind::Local,
        visibility: WorkspaceVisibility::Shared,
        main_session_id: None,
        root_dir: None,
        linked_vault_dir: None,
        notes_vault_dir: None,
        active_tab_id: None,
        running_on_host_id: None,
        controlled_by_host_id: None,
        layout_snapshot: None,
        last_active: 0,
    });
    assert_eq!(
        context_manager.workspace_icon_kind_for_index(1).as_deref(),
        Some("shared")
    );

    // Switching/detaching daemon connections replaces the active cache. The
    // workspace strip must still show the hosted badge on the unfocused tab.
    context_manager.detach_daemon_client();
    assert_eq!(context_manager.current_index(), 0);
    assert_eq!(
        context_manager.workspace_icon_kind_for_index(1).as_deref(),
        Some("shared")
    );
}

#[test]
fn self_hosted_adopted_workspace_is_collaborative_on_peer_link() {
    use neoism_protocol::workspace::{
        WorkspaceHostKind, WorkspaceSummary, WorkspaceVisibility,
    };

    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let stable = context_manager
        .current_grid()
        .workspace_route_id()
        .expect("test grid has a stable root");
    context_manager.adopted_workspaces.insert(
        stable,
        AdoptedWorkspaceBinding {
            workspace_id: "host-workspace".to_string(),
            endpoint: "ws://127.0.0.1:9877/session".to_string(),
            credential: Some("host-secret".to_string()),
            is_peer: false,
        },
    );
    context_manager.daemon.link_is_peer = true;
    context_manager
        .daemon
        .cache
        .daemon_host_workspaces
        .push(WorkspaceSummary {
            id: "host-workspace".to_string(),
            host_id: "local-host".to_string(),
            title: "Shared".to_string(),
            host_kind: WorkspaceHostKind::Local,
            visibility: WorkspaceVisibility::Shared,
            main_session_id: None,
            root_dir: None,
            linked_vault_dir: None,
            notes_vault_dir: None,
            active_tab_id: None,
            running_on_host_id: None,
            controlled_by_host_id: None,
            layout_snapshot: None,
            last_active: 0,
        });

    assert!(!context_manager.current_workspace_is_remote_joined());
    assert!(context_manager.current_workspace_is_collaborative());
    assert_eq!(
        context_manager
            .agent_server_override_for_current()
            .as_deref(),
        Some("http://127.0.0.1:9877/agent/workspaces/host-workspace")
    );
}

#[test]
fn apply_pane_layout_changed_accepts_snapshot_json() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    context_manager.apply_session_list(vec![session("session-a"), session("session-b")]);
    let snapshot_json = serde_json::to_string(&snapshot(11)).unwrap();

    assert!(context_manager.apply_pane_layout_changed(
        11,
        PaneLayoutOp::Focus {
            dir: PaneFocusDir::Left,
        },
        Some(snapshot_json),
    ));

    assert_eq!(
        context_manager.cached_active_session_id(),
        Some("session-a")
    );
    assert!(context_manager.daemon_cache().layout_json.is_some());
    assert!(context_manager
        .daemon_cache()
        .last_layout_update_at
        .is_some());
}

#[test]
fn daemon_attached_focus_split_is_send_only_until_snapshot() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let _runtime = attach_unconnected_daemon(&mut context_manager);
    let route_before = context_manager.current_route();
    let index_before = context_manager.current_index();

    context_manager.select_next_split();

    assert_eq!(context_manager.current_route(), route_before);
    assert_eq!(context_manager.current_index(), index_before);
    assert_eq!(context_manager.daemon_cache().pending_request_count, 1);
    assert!(context_manager.cached_layout().is_none());
}

#[test]
fn daemon_attached_tab_reorder_does_not_swap_local_contexts_before_snapshot() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    context_manager.current_mut().rich_text_id = 9001;
    context_manager.add_context(false, 0);
    context_manager.add_context(false, 0);
    let _runtime = attach_unconnected_daemon(&mut context_manager);

    context_manager.move_current_to_next();

    assert_eq!(context_manager.current_index(), 0);
    assert_eq!(context_manager.current().rich_text_id, 9001);
    assert_eq!(context_manager.daemon_cache().pending_request_count, 1);
    assert!(context_manager.cached_layout().is_none());
}

#[test]
fn stale_workspace_switch_ack_does_not_change_the_selected_grid() {
    let window_id: WindowId = WindowId::from(0);
    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    context_manager.add_context(false, 0);
    let workspace_a =
        context_manager.workspace_id_for_grid(&context_manager.contexts[0], 0);
    let workspace_b =
        context_manager.workspace_id_for_grid(&context_manager.contexts[1], 1);

    // The UI has completed A -> B -> A, but the independently queued B
    // acknowledgement arrives last.
    context_manager.set_current(1);
    context_manager.set_current(0);
    assert!(context_manager.apply_workspace_server_message(
        WorkspaceServerMessage::HostWorkspaceChanged {
            host_id: "desktop-window".to_string(),
            workspace_id: Some(workspace_a),
        },
    ));
    assert!(context_manager.apply_workspace_server_message(
        WorkspaceServerMessage::HostWorkspaceChanged {
            host_id: "desktop-window".to_string(),
            workspace_id: Some(workspace_b),
        },
    ));

    assert_eq!(context_manager.current_index(), 0);
}

#[test]
fn test_capacity() {
    let window_id: WindowId = WindowId::from(0);

    let context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    assert_eq!(context_manager.capacity, 5);

    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    context_manager.increase_capacity(3);
    assert_eq!(context_manager.capacity, 8);
}

#[test]
fn test_add_context() {
    let window_id: WindowId = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    assert_eq!(context_manager.capacity, 5);
    assert_eq!(context_manager.current_index, 0);

    let should_redirect = false;
    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.capacity, 5);
    assert_eq!(context_manager.current_index, 0);

    let should_redirect = true;
    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.capacity, 5);
    assert_eq!(context_manager.current_index, 2);
}

#[test]
fn test_add_context_start_with_capacity_limit() {
    let window_id: WindowId = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(3, VoidListener {}, window_id).unwrap();
    assert_eq!(context_manager.capacity, 3);
    assert_eq!(context_manager.current_index, 0);
    let should_redirect = false;
    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.len(), 2);
    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.len(), 3);

    for _ in 0..20 {
        context_manager.add_context(should_redirect, 0);
    }

    assert_eq!(context_manager.len(), 3);
    assert_eq!(context_manager.capacity, 3);
}

#[test]
fn test_set_current() {
    let window_id: WindowId = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(8, VoidListener {}, window_id).unwrap();
    let should_redirect = true;

    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.current_index, 1);
    context_manager.set_current(0);
    assert_eq!(context_manager.current_index, 0);
    assert_eq!(context_manager.len(), 2);
    assert_eq!(context_manager.capacity, 8);

    let should_redirect = false;
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.set_current(3);
    assert_eq!(context_manager.current_index, 3);

    context_manager.set_current(8);
    assert_eq!(context_manager.current_index, 3);
}

#[test]
fn test_switch_to_next() {
    let window_id: WindowId = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let should_redirect = false;

    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    assert_eq!(context_manager.len(), 5);
    assert_eq!(context_manager.current_index, 0);

    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 1);
    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 2);
    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 3);
    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 4);
    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 0);
    context_manager.switch_to_next();
    assert_eq!(context_manager.current_index, 1);
}

#[test]
fn test_move_current_to_next() {
    let window_id = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let should_redirect = false;

    context_manager.current_mut().rich_text_id = 1;
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);

    assert_eq!(context_manager.len(), 5);
    assert_eq!(context_manager.current_index, 0);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 1);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 2);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 3);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 4);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 0);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_next();
    assert_eq!(context_manager.current_index, 1);
    assert_eq!(context_manager.current().rich_text_id, 1);
}

#[test]
fn test_move_current_to_prev() {
    let window_id = WindowId::from(0);

    let mut context_manager =
        ContextManager::start_with_capacity(5, VoidListener {}, window_id).unwrap();
    let should_redirect = false;

    context_manager.current_mut().rich_text_id = 1;
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);
    context_manager.add_context(should_redirect, 0);

    assert_eq!(context_manager.len(), 5);
    assert_eq!(context_manager.current_index, 0);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 4);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 3);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 2);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 1);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 0);
    assert_eq!(context_manager.current().rich_text_id, 1);

    context_manager.move_current_to_prev();
    assert_eq!(context_manager.current_index, 4);
    assert_eq!(context_manager.current().rich_text_id, 1);
}

#[test]
fn remote_attach_error_invalidates_binding_and_late_success_cannot_revive_it() {
    use crate::context::remote_pty;
    use neoism_protocol::pty::ServerMessage;
    let mut manager =
        ContextManager::start_with_capacity(5, VoidListener {}, WindowId::from(0))
            .unwrap();
    let runtime = attach_unconnected_daemon(&mut manager);
    let (handle, _) = manager
        .daemon
        .link
        .as_ref()
        .unwrap()
        .handle_and_runtime()
        .unwrap();
    let prepared = remote_pty::prepare(handle, runtime.handle().clone());
    let (mut pty, feed) = neoism_terminal_pty::PtySession::remote(prepared.sink);
    let binding = remote_pty::RemotePtyBinding {
        feed,
        shared: prepared.shared,
    };
    let route = 987;
    manager
        .daemon
        .cache
        .remote_routes
        .insert(route, binding.clone());
    manager
        .daemon
        .cache
        .route_sessions
        .insert(route, "stale".into());
    manager
        .daemon
        .cache
        .session_routes
        .insert("stale".into(), route);
    manager
        .daemon
        .cache
        .pending_pty_attaches
        .insert(123, (route, "stale".into()));
    pty.write(b"ls\n").unwrap();
    assert_eq!(binding.shared.lock().unwrap().queued.len(), 1);
    assert!(manager.apply_pty_server_message(
        123,
        ServerMessage::Error {
            message: "unknown session stale".into()
        }
    ));
    assert!(binding.shared.lock().unwrap().queued.is_empty());
    assert!(binding.shared.lock().unwrap().session_id.is_none());
    assert!(!manager.daemon.cache.route_sessions.contains_key(&route));
    assert!(!manager.daemon.cache.session_routes.contains_key("stale"));
    assert!(pty.exit_code().is_some());
    assert!(!manager.apply_pty_server_message(
        123,
        ServerMessage::PtyCreated {
            session_id: "stale".into(),
            shell: None,
            workspace_root: None
        }
    ));
    pty.write(b"never replay\n").unwrap();
    assert!(binding.shared.lock().unwrap().queued.is_empty());
    // Close after invalidation must not send ClosePty to a possibly live shell.
    pty.close();
}

#[test]
fn stale_attach_reply_cannot_bind_a_reassigned_route() {
    let mut manager =
        ContextManager::start_with_capacity(5, VoidListener {}, WindowId::from(0))
            .unwrap();
    manager
        .daemon
        .cache
        .pending_pty_attaches
        .insert(123, (987, "old".into()));
    manager
        .daemon
        .cache
        .route_sessions
        .insert(987, "new".into());
    assert!(!manager.apply_pty_server_message(
        123,
        neoism_protocol::pty::ServerMessage::PtyCreated {
            session_id: "old".into(),
            shell: None,
            workspace_root: None
        }
    ));
    assert_eq!(
        manager
            .daemon
            .cache
            .route_sessions
            .get(&987)
            .map(String::as_str),
        Some("new")
    );
}
