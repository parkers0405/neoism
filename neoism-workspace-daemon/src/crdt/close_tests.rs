use super::*;
use neoism_protocol::crdt::{CrdtCursorPosition, CrdtPresenceColor};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};

fn fixture() -> (tempfile::TempDir, String, CrdtSyncHub) {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("document.txt");
    std::fs::write(&path, "hello").unwrap();
    let id = crate::crdt::crdt_buffer_id_for_path(&path);
    (root, id, CrdtSyncHub::default())
}

fn acquire(hub: &CrdtSyncHub, connection: u64, id: &str, owner: &str) {
    hub.handle_socket_message(connection, CrdtClientMessage::AcquireBuffer {
        buffer_id: id.into(), owner_id: owner.into(), initial_text: "stale cache".into(),
    });
}

fn close(hub: &CrdtSyncHub, connection: u64, id: &str, owner: &str) -> bool {
    let replies = hub.handle_socket_message(connection, CrdtClientMessage::CloseBuffer {
        buffer_id: id.into(), owner_id: owner.into(),
    });
    matches!(replies.as_slice(), [CrdtServerMessage::BufferClosed { evicted: true, .. }])
}

fn edit(hub: &CrdtSyncHub, connection: u64, id: &str, owner: &str, peer: &CrdtTextBuffer) {
    let update = peer.apply_local_edit(CrdtTextEdit::Insert { index: 5, content: "!".into() }).unwrap();
    hub.handle_socket_message(connection, CrdtClientMessage::Owned { owner_id: owner.into(), message: Box::new(CrdtClientMessage::ApplyUpdate {
        update: CrdtBufferUpdate { buffer_id: id.into(), origin_client_id: update.origin_client_id,
            update_v1: update.update_v1, state_vector_v1: update.state_vector_v1 },
    }) });
}

#[test]
fn explicit_close_waits_for_all_views_and_clients_then_removes_document_state() {
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "view-a");
    acquire(&hub, 1, &id, "view-a"); // Idempotent lease, not a second consumer.
    acquire(&hub, 1, &id, "view-b");
    acquire(&hub, 2, &id, "view-a");
    assert!(!close(&hub, 3, &id, "view-a"), "cannot release another socket's lease");
    assert!(!close(&hub, 1, &id, "view-a"));
    assert!(!close(&hub, 1, &id, "view-b"));
    assert!(hub.buffers.has_buffer(&id));
    hub.handle_socket_message(2, CrdtClientMessage::Owned {
        owner_id: "view-a".into(),
        message: Box::new(CrdtClientMessage::AcknowledgeStateVector {
            buffer_id: id.clone(), peer_id: "peer".into(),
            state_vector_v1: hub.buffers.state_vector_v1(&id).unwrap(),
        }),
    });
    hub.handle_socket_message(2, CrdtClientMessage::PublishPresence { presence: CrdtPeerPresence {
        buffer_id: id.clone(), peer_id: "peer".into(), display_name: "Peer".into(), client_os: None,
        color: CrdtPresenceColor { r: 1, g: 2, b: 3 },
        cursor: CrdtCursorPosition { line: 0, column: 0, offset: Some(0) },
        selection: None, insert: false, rainbow: false, updated_at_ms: 1,
    } });
    assert!(close(&hub, 2, &id, "view-a"));
    assert!(!hub.buffers.has_buffer(&id));
    assert!(!hub.disk_texts.lock().contains_key(&id));
    assert!(!hub.compaction.lock().contains_key(&id));
    assert!(!hub.peer_state_vectors.lock().contains_key(&id));
    assert!(!hub.presence.lock().contains_key(&id));
    assert!(!hub.leases.lock().contains_key(&id));
    std::fs::write(crate::crdt::crdt_path_for_buffer_id(&id).unwrap(), "new disk text").unwrap();
    acquire(&hub, 1, &id, "fresh");
    let peer = CrdtTextBuffer::new(99);
    peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &[]).unwrap().update_v1).unwrap();
    assert_eq!(peer.text(), "new disk text");
}

#[test]
fn explicit_close_preserves_dirty_conflicted_unbacked_and_unknown_documents() {
    for conflict in [false, true] {
        let (_root, id, hub) = fixture();
        acquire(&hub, 1, &id, "owner");
        let peer = CrdtTextBuffer::new(99);
        peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &[]).unwrap().update_v1).unwrap();
        edit(&hub, 1, &id, "owner", &peer);
        if conflict { std::fs::write(crate::crdt::crdt_path_for_buffer_id(&id).unwrap(), "external").unwrap(); }
        assert!(!close(&hub, 1, &id, "owner"));
        assert_eq!(hub.buffers.text(&id).unwrap(), "hello!");
        assert!(hub.disk_texts.lock().contains_key(&id));
    }
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "owner");
    std::fs::write(crate::crdt::crdt_path_for_buffer_id(&id).unwrap(), "external").unwrap();
    assert!(!close(&hub, 1, &id, "owner"), "clean doc with changed disk is not proven clean");
    acquire(&hub, 1, "scratch", "owner");
    assert!(!close(&hub, 1, "scratch", "owner"));
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "owner");
    hub.open_buffer(&id, "legacy model handle");
    assert!(!close(&hub, 1, &id, "owner"));
}

#[test]
fn explicit_close_preserves_legacy_subscriber_and_plugin_handles() {
    for plugin in [false, true] {
        let (_root, id, hub) = fixture();
        acquire(&hub, 1, &id, "owner");
        let message = CrdtClientMessage::RequestSnapshot { buffer_id: id.clone(), state_vector_v1: vec![] };
        if plugin { hub.handle_client_message(message); }
        else { hub.handle_socket_message(1, message); } // Legacy handle on the SAME multiplexed socket.
        assert!(!close(&hub, 1, &id, "owner"));
    }
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "owner");
    let registry_handle = hub.buffers().clone();
    registry_handle.snapshot_for(&id, &[]).unwrap();
    assert!(!close(&hub, 1, &id, "owner"), "untracked registry snapshot consumers retain their clocks");
}

#[test]
fn explicit_close_reopen_drops_queued_old_incarnation_updates_only_for_that_document() {
    let (_root, id, hub) = fixture();
    let mut receiver = hub.subscribe();
    acquire(&hub, 1, &id, "owner");
    let peer = CrdtTextBuffer::new(99);
    peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &[]).unwrap().update_v1).unwrap();
    edit(&hub, 1, &id, "owner", &peer);
    hub.save_buffer(&id);
    assert!(close(&hub, 1, &id, "owner"));
    let other = CrdtServerMessage::Saved { buffer_id: "another workspace".into(), bytes_written: 7 };
    hub.tx.send(other.clone()).unwrap();
    std::fs::write(crate::crdt::crdt_path_for_buffer_id(&id).unwrap(), "fresh disk").unwrap();
    let (replies, pending) = hub.handle_socket_message_draining(1, CrdtClientMessage::AcquireBuffer {
        buffer_id: id.clone(), owner_id: "fresh".into(), initial_text: "stale".into(),
    }, &mut receiver);
    assert_eq!(pending, vec![other]);
    let fresh = CrdtTextBuffer::new(100);
    match &replies[0] {
        CrdtServerMessage::Snapshot { update_v1, .. } => fresh.apply_update_v1(update_v1).unwrap(),
        other => panic!("expected fresh snapshot, got {other:?}"),
    };
    assert_eq!(fresh.text(), "fresh disk");
    assert!(receiver.try_recv().is_err());
}

#[test]
fn explicit_close_retains_disconnected_replica_clocks_and_offline_updates() {
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "offline");
    let peer = CrdtTextBuffer::new(99);
    peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &[]).unwrap().update_v1).unwrap();
    // Socket 1 disappears unexpectedly. There is deliberately no Drop release.
    acquire(&hub, 2, &id, "other");
    assert!(!close(&hub, 2, &id, "other"));
    acquire(&hub, 3, &id, "offline");
    edit(&hub, 3, &id, "offline", &peer);
    peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &peer.state_vector_v1()).unwrap().update_v1).unwrap();
    assert_eq!(peer.text(), "hello!");
    assert_eq!(hub.buffers.text(&id).unwrap(), "hello!");
    hub.save_buffer(&id);
    assert!(!close(&hub, 3, &id, "offline"), "old socket lease still protects old clocks");
}

#[test]
fn explicit_close_racing_agent_mutation_never_discards_accepted_work() {
    let mut accepted_count = 0;
    for iteration in 0..20 {
        let (_root, id, hub) = fixture();
        acquire(&hub, 1, &id, "owner");
        if iteration == 0 {
            // A long-lived agent handle must win even when its next edit has
            // not started yet. Remaining iterations race first-use mutations.
            hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
                buffer_id: id.clone(), state_vector_v1: vec![],
            });
        }
        let agent_hub = hub.clone();
        let agent_id = id.clone();
        let agent = std::thread::spawn(move || agent_hub.handle_client_message(CrdtClientMessage::ApplyEdits {
            transaction: CrdtEditTransaction { buffer_id: agent_id, plugin_id: "agent".into(), invocation_id: "job".into(),
                idempotency_key: "edit".into(), expected_state_vector_v1: vec![],
                edits: vec![CrdtBufferEdit::Insert { index: 5, content: " agent".into() }] },
        }));
        let evicted = close(&hub, 1, &id, "owner");
        let replies = agent.join().unwrap();
        if replies.iter().any(|r| matches!(r, CrdtServerMessage::Sync { .. })) {
            accepted_count += 1;
            assert!(!evicted);
            assert_eq!(hub.buffers.text(&id).unwrap(), "hello agent");
        } else {
            assert!(matches!(replies.as_slice(), [CrdtServerMessage::Error { .. }]));
        }
    }
    assert!(accepted_count > 0);
}

#[test]
fn explicit_close_rejects_stale_owner_updates_after_fresh_reopen() {
    let (_root, id, hub) = fixture();
    acquire(&hub, 1, &id, "old");
    let peer = CrdtTextBuffer::new(99);
    peer.apply_update_v1(&hub.buffers.snapshot_for_managed(&id, &[]).unwrap().update_v1).unwrap();
    let update = peer.apply_local_edit(CrdtTextEdit::Insert { index: 5, content: " stale".into() }).unwrap();
    assert!(close(&hub, 1, &id, "old"));
    std::fs::write(crate::crdt::crdt_path_for_buffer_id(&id).unwrap(), "new bytes").unwrap();
    acquire(&hub, 1, &id, "new");
    let replies = hub.handle_socket_message(1, CrdtClientMessage::Owned {
        owner_id: "old".into(),
        message: Box::new(CrdtClientMessage::ApplyUpdate { update: CrdtBufferUpdate {
            buffer_id: id.clone(), origin_client_id: update.origin_client_id,
            update_v1: update.update_v1, state_vector_v1: update.state_vector_v1,
        } }),
    });
    assert!(matches!(replies.as_slice(), [CrdtServerMessage::Error { .. }]));
    assert_eq!(hub.buffers.text(&id).unwrap(), "new bytes");
    assert!(close(&hub, 1, &id, "new"));
}