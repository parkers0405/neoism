//! Authoritative restore transactions use the existing hub, including peer edits.
use neoism_protocol::crdt::{CrdtBufferUpdate, CrdtClientMessage, CrdtServerMessage};
use neoism_protocol::git::{GitChangeBlock, GitClientMessage, GitServerMessage};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};
use neoism_workspace_daemon::{
    crdt::{crdt_buffer_id_for_path, sync::CrdtSyncHub},
    git,
};
use std::{path::Path, sync::mpsc, time::Duration};

fn setup() -> (tempfile::TempDir, CrdtSyncHub, String) {
    let dir = tempfile::tempdir().unwrap();
    let repo = git2::Repository::init(dir.path()).unwrap();
    std::fs::create_dir(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/note"), "old\n").unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("nested/note")).unwrap();
    index.write().unwrap();
    let tree_id = index.write_tree().unwrap();
    let tree = repo.find_tree(tree_id).unwrap();
    let sig = git2::Signature::now("Test", "test@example.org").unwrap();
    repo.commit(Some("HEAD"), &sig, &sig, "seed", &tree, &[])
        .unwrap();
    std::fs::write(dir.path().join("nested/note"), "new\n").unwrap();
    let id = crdt_buffer_id_for_path(&dir.path().join("nested/note"));
    let hub = CrdtSyncHub::default();
    hub.open_buffer(&id, "stale");
    (dir, hub, id)
}

fn peer_edit(hub: &CrdtSyncHub, id: &str) -> CrdtClientMessage {
    let peer = CrdtTextBuffer::new(731);
    peer.apply_update_v1(&hub.buffers().snapshot_for(id, &[]).unwrap().update_v1)
        .unwrap();
    let update = peer
        .apply_local_edit(CrdtTextEdit::Insert {
            index: 0,
            content: "peer ".into(),
        })
        .unwrap();
    CrdtClientMessage::ApplySync {
        envelope: CrdtBufferUpdate {
            buffer_id: id.into(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        }
        .into(),
    }
}

fn restore() -> GitClientMessage {
    GitClientMessage::ApplyBlock {
        path: "nested/note".into(),
        restore: true,
        block: GitChangeBlock {
            old_start: 0,
            new_start: 0,
            old_lines: vec!["old".into()],
            new_lines: vec!["new".into()],
        },
    }
}

fn success(replies: &[GitServerMessage]) -> bool {
    matches!(
        replies,
        [GitServerMessage::ChangedFiles { error: None, .. }]
    )
}

#[tokio::test]
async fn dirty_peer_restore_refuses_without_touching_disk_or_index() {
    let (dir, hub, id) = setup();
    hub.handle_client_message(peer_edit(&hub, &id));
    let index_before = std::fs::read(dir.path().join(".git/index")).unwrap();
    let replies =
        git::handle_with_root_and_crdt(dir.path().into(), restore(), hub.clone()).await;
    assert!(!success(&replies), "{replies:?}");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/note")).unwrap(),
        "new\n"
    );
    assert_eq!(hub.buffers().text(&id).unwrap(), "peer new\n");
    assert_eq!(
        std::fs::read(dir.path().join(".git/index")).unwrap(),
        index_before
    );
}

#[tokio::test]
async fn clean_restore_converges_and_broadcasts_before_ack_from_nested_workspace() {
    let (dir, hub, id) = setup();
    let mut rx = hub.subscribe();
    let replies =
        git::handle_with_root_and_crdt(dir.path().join("nested"), restore(), hub.clone())
            .await;
    assert!(success(&replies), "{replies:?}");
    assert_eq!(hub.buffers().text(&id).unwrap(), "old\n");
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/note")).unwrap(),
        "old\n"
    );
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Sync { .. }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Update { .. }
    ));
    assert!(
        matches!(rx.try_recv().unwrap(), CrdtServerMessage::Saved { buffer_id, .. } if buffer_id == id)
    );
    assert!(rx.try_recv().is_err(), "one Saved per restore");
}

#[test]
fn external_disk_write_preserves_dirty_document_even_on_reopen() {
    let (dir, hub, id) = setup();
    hub.handle_client_message(peer_edit(&hub, &id));
    let mut rx = hub.subscribe();
    let path = dir.path().join("nested/note");
    std::fs::write(&path, "external\n").unwrap();
    assert!(!hub.reconcile_disk_path(&path));
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Error { .. }
    ));
    hub.open_buffer(&id, "stale");
    assert_eq!(hub.buffers().text(&id).unwrap(), "peer new\n");
    assert!(hub
        .guarded_restore(&path, || -> Result<(), String> {
            panic!("must reject before write")
        })
        .is_err());
}

#[test]
fn restore_serializes_save_and_apply_sync_until_reconciliation() {
    let (dir, hub, id) = setup();
    let update = peer_edit(&hub, &id);
    let path = dir.path().join("nested/note");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let h = hub.clone();
    let restore_thread = std::thread::spawn(move || {
        h.guarded_restore(&path, || {
            entered_tx.send(()).unwrap();
            release_rx.recv_timeout(Duration::from_secs(5)).unwrap();
            std::fs::write(&path, "old\n").map_err(|e| e.to_string())
        })
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let (done_tx, done_rx) = mpsc::channel();
    let h = hub.clone();
    let apply_done = done_tx.clone();
    let apply = std::thread::spawn(move || {
        h.handle_client_message(update);
        apply_done.send(()).unwrap();
    });
    let h = hub.clone();
    let save_id = id.clone();
    let save = std::thread::spawn(move || {
        h.save_buffer(&save_id);
        done_tx.send(()).unwrap();
    });
    assert!(
        done_rx.recv_timeout(Duration::from_millis(100)).is_err(),
        "save/apply cannot enter guarded restore"
    );
    release_tx.send(()).unwrap();
    restore_thread.join().unwrap().unwrap();
    apply.join().unwrap();
    save.join().unwrap();
    // Save may precede the queued edit or follow it, but never writes the
    // pre-restore document over the restored disk.
    let disk = std::fs::read_to_string(dir.path().join("nested/note")).unwrap();
    assert!(disk == "old\n" || disk == hub.buffers().text(&id).unwrap());
    assert_ne!(disk, "new\n");
}

#[cfg(unix)]
#[tokio::test]
async fn dirty_symlink_alias_blocks_restore_of_canonical_target() {
    let (dir, hub, _) = setup();
    let alias = dir.path().join("alias");
    std::os::unix::fs::symlink(dir.path().join("nested/note"), &alias).unwrap();
    let id = crdt_buffer_id_for_path(&alias);
    hub.open_buffer(&id, "stale");
    hub.handle_client_message(peer_edit(&hub, &id));
    let replies = git::handle_with_root_and_crdt(dir.path().into(), restore(), hub).await;
    assert!(!success(&replies));
    assert_eq!(std::fs::read_to_string(alias).unwrap(), "new\n");
}

#[tokio::test]
async fn restore_added_file_deletion_converges_to_empty_document() {
    let (dir, hub, _) = setup();
    let path = dir.path().join("nested/added");
    std::fs::write(&path, "added\n").unwrap();
    let id = crdt_buffer_id_for_path(&path);
    hub.open_buffer(&id, "stale");
    let mut rx = hub.subscribe();
    hub.guarded_restore(&path, || std::fs::remove_file(&path).map_err(|e| e.to_string()))
        .unwrap();
    assert!(!path.exists());
    assert_eq!(hub.buffers().text(&id).unwrap(), "");
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Sync { .. }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Update { .. }
    ));
    assert!(matches!(
        rx.try_recv().unwrap(),
        CrdtServerMessage::Saved {
            bytes_written: 0,
            ..
        }
    ));
}

#[tokio::test]
async fn repo_relative_restore_cannot_escape_declared_workspace() {
    let (dir, hub, _) = setup();
    std::fs::create_dir(dir.path().join("other")).unwrap();
    let replies =
        git::handle_with_root_and_crdt(dir.path().join("other"), restore(), hub).await;
    assert!(!success(&replies));
    assert_eq!(
        std::fs::read_to_string(dir.path().join("nested/note")).unwrap(),
        "new\n"
    );
}
