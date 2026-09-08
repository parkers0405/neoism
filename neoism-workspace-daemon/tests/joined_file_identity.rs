//! Cross-platform regression: host listing -> shared tree -> relative ReadFile
//! -> exactly the host's document/presence identity. No guest path joining.
use neoism_protocol::{
    files::{FilesClientMessage, FilesServerMessage},
    host_path::HostPath,
};
use neoism_ui::editor::crdt::presence_buffer_id_for_path;
use neoism_workspace_daemon::{
    crdt::{crdt_buffer_id_for_path, sync::CrdtSyncHub},
    files,
};
use std::{collections::HashMap, path::Path};

async fn listed_child(root: &Path, relative: &str, name: &str) -> std::path::PathBuf {
    let messages = files::handle_with_root(
        root,
        FilesClientMessage::ListDir {
            path: relative.into(),
        },
    )
    .await;
    let FilesServerMessage::DirListing { entries, .. } = &messages[0] else {
        panic!("{messages:?}")
    };
    let entries: Vec<neoism_ui::services::DirEntry> =
        serde_json::from_value(serde_json::to_value(entries).unwrap()).unwrap();
    let directory = HostPath::new(root.to_str().unwrap()).join(relative);
    let rows = neoism_ui::panels::file_tree::entries_from_dir_listing(
        Path::new(directory.as_str()),
        0,
        &HashMap::new(),
        entries,
        true,
    );
    rows.into_iter()
        .find(|row| row.label == name)
        .unwrap()
        .path
        .unwrap()
}

#[tokio::test]
async fn host_tree_read_crdt_and_presence_share_one_identity() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path();
    std::fs::create_dir_all(root.join("src 子 dir").join("deep dir")).unwrap();
    for relative in [
        "README.md",
        "src 子 dir/main 文 file.rs",
        "src 子 dir/deep dir/nested.rs",
    ] {
        let host_path = relative
            .split('/')
            .fold(root.to_path_buf(), |path, segment| path.join(segment));
        let content = format!("host bytes for {relative}\n");
        std::fs::write(&host_path, &content).unwrap();
        let (dir, name) = relative.rsplit_once('/').unwrap_or(("", relative));
        if !dir.is_empty() {
            let mut parent_relative = String::new();
            let mut native_parent = root.to_path_buf();
            for segment in dir.split('/') {
                let folder = listed_child(root, &parent_relative, segment).await;
                native_parent.push(segment);
                assert_eq!(folder.as_os_str(), native_parent.as_os_str());
                if !parent_relative.is_empty() {
                    parent_relative.push('/');
                }
                parent_relative.push_str(segment);
            }
        }
        let tree_path = listed_child(root, dir, name).await;
        assert_eq!(tree_path.as_os_str(), host_path.as_os_str());
        let wire = HostPath::new(root.to_str().unwrap())
            .relative(tree_path.to_str().unwrap())
            .unwrap();
        assert_eq!(wire, relative);
        let stats = files::handle_with_root(
            root,
            FilesClientMessage::Stat { path: wire.clone() },
        )
        .await;
        let FilesServerMessage::Stat { entry, .. } = &stats[0] else {
            panic!("{stats:?}")
        };
        assert_eq!(entry.host_path.as_deref(), host_path.to_str());
        let replies =
            files::handle_with_root(root, FilesClientMessage::ReadFile { path: wire })
                .await;
        let FilesServerMessage::FileContent { bytes, .. } = &replies[0] else {
            panic!("{replies:?}")
        };
        assert_eq!(bytes, content.as_bytes());
        let host_id = crdt_buffer_id_for_path(&host_path);
        let guest_id = HostPath::new(tree_path.to_str().unwrap()).buffer_id();
        assert_eq!(guest_id, host_id);
        assert_eq!(presence_buffer_id_for_path(&tree_path), host_id);
        let hub = CrdtSyncHub::default();
        hub.open_buffer(&host_id, "stale host cache");
        hub.open_buffer(&guest_id, String::from_utf8_lossy(bytes));
        assert_eq!(hub.buffers().text(&guest_id).unwrap(), content);
        assert!(matches!(
            hub.save_buffer(&guest_id),
            neoism_protocol::crdt::CrdtServerMessage::Saved { .. }
        ));
    }
}

#[cfg(unix)]
#[tokio::test]
async fn literal_unix_backslash_is_a_real_distinct_file() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join(r"literal\文 件.md"), "literal").unwrap();
    let tree_path = listed_child(temp.path(), "", r"literal\文 件.md").await;
    let root = HostPath::new(temp.path().to_str().unwrap());
    let relative = root.relative(tree_path.to_str().unwrap()).unwrap();
    assert_eq!(relative, r"literal\文 件.md");
    let replies = files::handle_with_root(
        temp.path(),
        FilesClientMessage::ReadFile { path: relative },
    )
    .await;
    assert!(
        matches!(&replies[0], FilesServerMessage::FileContent { bytes, .. } if bytes == b"literal")
    );
    let hub = CrdtSyncHub::default();
    let id = crdt_buffer_id_for_path(&tree_path);
    hub.open_buffer(&id, "ignored");
    assert!(matches!(
        hub.save_buffer(&id),
        neoism_protocol::crdt::CrdtServerMessage::Saved { .. }
    ));
}

fn insert_peer_text(hub: &CrdtSyncHub, id: &str, text: &str) {
    use neoism_protocol::crdt::{CrdtBufferUpdate, CrdtClientMessage};
    use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};
    let peer = CrdtTextBuffer::new(817);
    let snapshot = hub.buffers().snapshot_for(id, &[]).unwrap();
    peer.apply_update_v1(&snapshot.update_v1).unwrap();
    let update = peer
        .apply_local_edit(CrdtTextEdit::Insert {
            index: 0,
            content: text.into(),
        })
        .unwrap();
    hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
        update: CrdtBufferUpdate {
            buffer_id: id.into(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        },
    });
}

#[tokio::test]
async fn new_file_create_read_open_edit_save_workflow() {
    use neoism_protocol::crdt::CrdtServerMessage;
    let temp = tempfile::tempdir().unwrap();
    for relative in ["new.rs", "notes/新 note.md"] {
        let messages = files::handle_with_root(
            temp.path(),
            FilesClientMessage::CreateFile {
                dir: String::new(),
                name: relative.into(),
            },
        )
        .await;
        assert!(matches!(
            &messages[0],
            FilesServerMessage::FileCreated { is_dir: false, .. }
        ));
        let messages = files::handle_with_root(
            temp.path(),
            FilesClientMessage::ReadFile {
                path: relative.into(),
            },
        )
        .await;
        let FilesServerMessage::FileContent { bytes, .. } = &messages[0] else {
            panic!("{messages:?}")
        };
        assert!(bytes.is_empty());
        // Mirrors both shipped entry points: local create_new, or host
        // CreateFile reply -> ReadFile -> OpenBuffer -> edit -> SaveBuffer.
        let path = relative
            .split('/')
            .fold(temp.path().to_path_buf(), |path, name| path.join(name));
        let id = crdt_buffer_id_for_path(&path);
        let hub = CrdtSyncHub::default();
        hub.open_buffer(&id, String::from_utf8_lossy(bytes));
        insert_peer_text(&hub, &id, "new document text");
        assert!(matches!(
            hub.save_buffer(&id),
            CrdtServerMessage::Saved { .. }
        ));
        assert_eq!(std::fs::read_to_string(path).unwrap(), "new document text");
    }
}

#[test]
fn previously_read_file_can_be_recreated_on_save_without_rewriting_its_identity() {
    use neoism_protocol::crdt::CrdtServerMessage;
    let temp = tempfile::tempdir().unwrap();
    std::fs::create_dir(temp.path().join("dir")).unwrap();
    // A legitimate lexical alias must not be broken by a blanket `..` ban.
    let path = temp.path().join("dir").join("..").join("note.md");
    std::fs::write(&path, "original").unwrap();
    let id = crdt_buffer_id_for_path(&path);
    let hub = CrdtSyncHub::default();
    hub.open_buffer(&id, "ignored cache");
    insert_peer_text(&hub, &id, "edited ");
    std::fs::remove_file(&path).unwrap();
    assert!(matches!(
        hub.save_buffer(&id),
        CrdtServerMessage::Saved { .. }
    ));
    assert_eq!(std::fs::read_to_string(path).unwrap(), "edited original");
}

#[cfg(unix)]
#[test]
fn known_literal_backslash_file_can_be_recreated_but_unknown_phantom_cannot() {
    use neoism_protocol::crdt::CrdtServerMessage;
    let temp = tempfile::tempdir().unwrap();
    let real = temp.path().join(r"real\name.md");
    std::fs::write(&real, "real file").unwrap();
    let hub = CrdtSyncHub::default();
    let real_id = crdt_buffer_id_for_path(&real);
    hub.open_buffer(&real_id, "ignored");
    std::fs::remove_file(&real).unwrap();
    assert!(matches!(
        hub.save_buffer(&real_id),
        CrdtServerMessage::Saved { .. }
    ));
    assert_eq!(std::fs::read_to_string(real).unwrap(), "real file");
    let phantom = temp.path().join(r"project\README.md");
    let phantom_id = crdt_buffer_id_for_path(&phantom);
    hub.open_buffer(&phantom_id, "unsynced edits");
    assert!(matches!(
        hub.save_buffer(&phantom_id),
        CrdtServerMessage::Error { .. }
    ));
    assert!(!phantom.exists());
    assert_eq!(hub.buffers().text(&phantom_id).unwrap(), "unsynced edits");
}

#[test]
fn malformed_or_missing_identity_cannot_create_file_and_keeps_unsaved_doc() {
    let temp = tempfile::tempdir().unwrap();
    let malformed = temp.path().join(r"project\README.md");
    let id = crdt_buffer_id_for_path(&malformed);
    let hub = CrdtSyncHub::default();
    hub.open_buffer(&id, "precious unsynced edits");
    assert!(matches!(
        hub.save_buffer(&id),
        neoism_protocol::crdt::CrdtServerMessage::Error { .. }
    ));
    assert!(!malformed.exists());
    assert_eq!(hub.buffers().text(&id).unwrap(), "precious unsynced edits");
}
