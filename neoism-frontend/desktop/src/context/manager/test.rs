
use super::*;
use crate::daemon_client::{
    DaemonClient, DaemonClientOptions, DaemonEndpoint, ReconnectBackoff,
};
use crate::event::VoidListener;
use neoism_protocol::workspace::PaneLayoutSnapshotNode;
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
