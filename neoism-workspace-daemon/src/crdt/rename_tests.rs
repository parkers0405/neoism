use super::*;
use neoism_protocol::files::{FilesClientMessage, FilesServerMessage};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};

fn acquire(hub: &CrdtSyncHub, id: &str) -> CrdtTextBuffer {
    let replies = hub.handle_socket_message(1, CrdtClientMessage::AcquireBuffer {
        buffer_id: id.into(), owner_id: "view".into(), initial_text: "stale".into(),
    });
    let [CrdtServerMessage::Snapshot { update_v1, .. }] = replies.as_slice() else {
        panic!("expected snapshot: {replies:?}");
    };
    let peer = CrdtTextBuffer::new(99);
    peer.apply_update_v1(update_v1).unwrap();
    peer
}

fn edit(hub: &CrdtSyncHub, id: &str, peer: &CrdtTextBuffer, content: &str) {
    let update = peer.apply_local_edit(CrdtTextEdit::Insert {
        index: 5, content: content.into(),
    }).unwrap();
    let replies = hub.handle_socket_message(1, CrdtClientMessage::Owned {
        owner_id: "view".into(), message: Box::new(CrdtClientMessage::ApplyUpdate {
            update: CrdtBufferUpdate { buffer_id: id.into(), origin_client_id: update.origin_client_id,
                update_v1: update.update_v1, state_vector_v1: update.state_vector_v1 },
        }),
    });
    assert!(!replies.iter().any(|r| matches!(r, CrdtServerMessage::Error { .. })), "{replies:?}");
}

async fn rename(hub: &CrdtSyncHub, root: &std::path::Path, from: &str, to: &str) -> Vec<FilesServerMessage> {
    crate::files::handle_with_hub(root, FilesClientMessage::Rename {
        from: from.into(), to: to.into(),
    }, hub).await
}

#[tokio::test]
async fn files_rename_preserves_dirty_replica_and_rejects_stale_saves() {
    // Exercise both a file and a directory through the production Files handler.
    for directory in [false, true] {
        let root = tempfile::tempdir().unwrap();
        let hub = CrdtSyncHub::default();
        let old_rel = if directory { "old/nested/doc.txt" } else { "old.txt" };
        let new_rel = if directory { "new/nested/doc.txt" } else { "new.txt" };
        let old = root.path().join(old_rel);
        let new = root.path().join(new_rel);
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, "hello").unwrap();
        let old_id = crate::crdt::crdt_buffer_id_for_path(&old);
        let new_id = crate::crdt::crdt_buffer_id_for_path(&new);
        let peer = acquire(&hub, &old_id);
        edit(&hub, &old_id, &peer, "!");
        let vector = hub.buffers.state_vector_v1(&old_id).unwrap();
        hub.handle_client_message(CrdtClientMessage::AcknowledgeStateVector {
            buffer_id: old_id.clone(), peer_id: "peer".into(), state_vector_v1: vector.clone(),
        }); // Also establishes an unknown (non-socket) owner.
        hub.handle_client_message(CrdtClientMessage::PublishPresence {
            presence: CrdtPeerPresence {
                buffer_id: old_id.clone(), peer_id: "peer".into(), display_name: "Peer".into(),
                client_os: None, color: neoism_protocol::crdt::CrdtPresenceColor { r: 1, g: 2, b: 3 },
                cursor: neoism_protocol::crdt::CrdtCursorPosition { line: 0, column: 0, offset: Some(0) },
                selection: None, insert: false, rainbow: false, updated_at_ms: 1,
            },
        });
        let mut extra = None;
        if directory {
            let second = root.path().join("old/second.txt");
            std::fs::write(&second, "hello").unwrap();
            let second_id = crate::crdt::crdt_buffer_id_for_path(&second);
            acquire(&hub, &second_id);
            // Prefix matching must be component-based, not string-based.
            let sibling = root.path().join("old-other.txt");
            std::fs::write(&sibling, "hello").unwrap();
            let sibling_id = crate::crdt::crdt_buffer_id_for_path(&sibling);
            acquire(&hub, &sibling_id);
            extra = Some((second_id, sibling_id));
        }
        let from = if directory { "old" } else { old_rel };
        // Absolute destination is tolerated and echoed unchanged.
        let to = root.path().join(if directory { "new" } else { new_rel }).to_string_lossy().into_owned();
        let replies = rename(&hub, root.path(), from, &to).await;
        assert!(matches!(replies.as_slice(), [FilesServerMessage::Renamed { from: f, to: t }] if f == from && t == &to));
        assert!(!old.exists());
        assert_eq!(std::fs::read_to_string(&new).unwrap(), "hello");
        assert_eq!(hub.buffers.text(&new_id).unwrap(), "hello!");
        assert_eq!(hub.buffers.state_vector_v1(&new_id).unwrap(), vector);
        assert_eq!(hub.disk_texts.lock().get(&new_id).unwrap(), "hello");
        assert!(!hub.disk_texts.lock().contains_key(&old_id));
        assert!(!hub.buffers.has_buffer(&old_id));
        assert!(hub.leases.lock().get(&new_id).unwrap().contains(&(1, "view".into())));
        assert!(hub.compaction.lock().contains_key(&new_id));
        assert!(!hub.compaction.lock().contains_key(&old_id));
        assert_eq!(hub.peer_state_vectors.lock().get(&new_id).unwrap().get("peer").unwrap(), &vector);
        assert!(!hub.peer_state_vectors.lock().contains_key(&old_id));
        assert!(hub.unknown_owners.lock().contains(&new_id));
        assert!(!hub.unknown_owners.lock().contains(&old_id));
        assert_eq!(hub.presence.lock().get(&new_id).unwrap().get("peer").unwrap().buffer_id, new_id);
        assert!(!hub.presence.lock().contains_key(&old_id));
        if let Some((second_id, sibling_id)) = extra {
            let relocated = crate::crdt::crdt_buffer_id_for_path(&root.path().join("new/second.txt"));
            assert!(!hub.buffers.has_buffer(&second_id));
            assert_eq!(hub.buffers.text(&relocated).unwrap(), "hello");
            assert!(matches!(hub.save_buffer(&second_id), CrdtServerMessage::Error { .. }));
            assert!(matches!(hub.save_buffer(&relocated), CrdtServerMessage::Saved { .. }));
            assert!(hub.buffers.has_buffer(&sibling_id));
        }
        // The requesting client's rebind must see the dirty authoritative text,
        // not reseed from the older bytes on disk at the new path.
        assert_eq!(acquire(&hub, &new_id).text(), peer.text());
        // Same peer clocks and owner still work after relocation (no reseeding).
        edit(&hub, &new_id, &peer, "?");
        assert!(matches!(hub.save_buffer(&old_id), CrdtServerMessage::Error { .. }));
        assert!(matches!(hub.save_buffer(&new_id), CrdtServerMessage::Saved { .. }));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), peer.text());
        assert!(!old.exists());
        // Unknown-buffer recovery must not silently Acquire/Open the retired ID,
        // nor return the no-baseline error eligible for a Files save fallback.
        let mut receiver = hub.subscribe();
        let (stale, _) = hub.handle_socket_message_draining(2, CrdtClientMessage::AcquireBuffer {
            buffer_id: old_id.clone(), owner_id: "stale-view".into(), initial_text: "stale cache".into(),
        }, &mut receiver);
        let mut rejected = stale;
        rejected.push(hub.open_buffer(&old_id, "stale cache"));
        rejected.push(hub.save_buffer(&old_id));
        for reply in rejected {
            let CrdtServerMessage::Error { message, .. } = reply else { panic!("retired ID accepted: {reply:?}") };
            assert!(message.contains("renamed"));
            assert!(!message.contains("unknown CRDT buffer"));
            assert!(!message.contains("no successfully read disk baseline"));
        }
        assert!(!hub.buffers.has_buffer(&old_id));
        assert!(!hub.leases.lock().contains_key(&old_id));
        assert!(!old.exists());
        // Explicit Files creation at the retired name is still legitimate.
        let replies = crate::files::handle_with_hub(root.path(), FilesClientMessage::CreateFile {
            dir: old.parent().unwrap().to_string_lossy().into_owned(), name: old.file_name().unwrap().to_string_lossy().into_owned(),
        }, &hub).await;
        assert!(matches!(replies.as_slice(), [FilesServerMessage::FileCreated { .. }]));
        assert_eq!(acquire(&hub, &old_id).text(), "");
        assert!(matches!(hub.save_buffer(&old_id), CrdtServerMessage::Saved { .. }));
        assert_eq!(std::fs::read_to_string(&new).unwrap(), peer.text());
    }
}

#[tokio::test]
async fn files_rename_failure_and_live_destination_leave_document_untouched() {
    let root = tempfile::tempdir().unwrap();
    let hub = CrdtSyncHub::default();
    let old = root.path().join("old");
    std::fs::write(&old, "hello").unwrap();
    let old_id = crate::crdt::crdt_buffer_id_for_path(&old);
    let peer = acquire(&hub, &old_id);
    edit(&hub, &old_id, &peer, "!");
    let vector = hub.buffers.state_vector_v1(&old_id).unwrap();
    std::fs::write(root.path().join("not-a-dir"), "blocker").unwrap();
    assert!(matches!(rename(&hub, root.path(), "old", "not-a-dir/new").await.as_slice(), [FilesServerMessage::Error { .. }]));
    // Parent creation succeeds, but the actual filesystem rename must fail.
    std::fs::create_dir(root.path().join("nonempty-dir")).unwrap();
    std::fs::write(root.path().join("nonempty-dir/child"), "blocker").unwrap();
    assert!(matches!(rename(&hub, root.path(), "old", "nonempty-dir").await.as_slice(), [FilesServerMessage::Error { .. }]));
    // A live destination remains a collision even after its disk file disappears.
    let target = root.path().join("target");
    std::fs::write(&target, "other").unwrap();
    let target_id = crate::crdt::crdt_buffer_id_for_path(&target);
    acquire(&hub, &target_id);
    std::fs::remove_file(&target).unwrap();
    assert!(matches!(rename(&hub, root.path(), "old", "target").await.as_slice(), [FilesServerMessage::Error { .. }]));
    assert_eq!(hub.buffers.text(&old_id).unwrap(), "hello!");
    assert_eq!(hub.buffers.state_vector_v1(&old_id).unwrap(), vector);
    assert_eq!(hub.buffers.text(&target_id).unwrap(), "other");
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "hello");
    assert!(!target.exists());
    assert!(matches!(hub.save_buffer(&old_id), CrdtServerMessage::Saved { .. }));
    assert_eq!(std::fs::read_to_string(&old).unwrap(), "hello!");

    std::fs::create_dir(root.path().join("source-dir")).unwrap();
    std::fs::write(root.path().join("source-dir/file"), "source").unwrap();
    std::fs::create_dir(root.path().join("target-dir")).unwrap();
    let descendant = root.path().join("target-dir/child");
    std::fs::write(&descendant, "hello").unwrap();
    let descendant_id = crate::crdt::crdt_buffer_id_for_path(&descendant);
    acquire(&hub, &descendant_id);
    std::fs::remove_dir_all(root.path().join("target-dir")).unwrap();
    assert!(matches!(rename(&hub, root.path(), "source-dir", "target-dir").await.as_slice(), [FilesServerMessage::Error { .. }]));
    assert!(root.path().join("source-dir/file").exists());
    assert!(!root.path().join("target-dir").exists());
    assert_eq!(hub.buffers.text(&descendant_id).unwrap(), "hello");
}