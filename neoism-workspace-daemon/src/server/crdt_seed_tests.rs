
use super::*;
use crate::nvim::BufferText;
use neoism_protocol::crdt::CrdtClientMessage;
use neoism_ui::editor::crdt::CrdtTextBuffer;
use std::path::PathBuf;

#[test]
fn buffer_id_scheme_is_stable_file_uri() {
    let id = crdt_buffer_id_for_path(&PathBuf::from("/work/src/main.rs"));
    assert_eq!(id, "file:///work/src/main.rs");
}

// The seed function itself needs a live nvim; here we exercise the
// exact hub behaviour it relies on: an OpenBuffer becomes a shareable,
// file-URI-keyed authoritative document that a second client can
// subscribe to over `/crdt` and observe identical text.
#[test]
fn seeding_makes_buffer_shareable_to_a_second_client() {
    let hub = CrdtSyncHub::default();
    let buffer = BufferText {
        path: PathBuf::from("/work/src/lib.rs"),
        text: "fn main() {}\n".into(),
        cursor_line: 0,
        cursor_col: 0,
    };
    let buffer_id = crdt_buffer_id_for_path(&buffer.path);

    // Daemon seeds from nvim's view (what seed_crdt_from_open_buffer does).
    hub.open_buffer(buffer_id.clone(), &buffer.text);
    assert!(hub.buffers().has_buffer(&buffer_id));
    assert_eq!(hub.buffers().text(&buffer_id).unwrap(), buffer.text);

    // A future second client requests a catch-up snapshot and converges.
    let peer = CrdtTextBuffer::new(42);
    let reply = hub.handle_client_message(CrdtClientMessage::RequestSnapshot {
        buffer_id: buffer_id.clone(),
        state_vector_v1: Vec::new(),
    });
    let CrdtServerMessage::Snapshot { update_v1, .. } = &reply[0] else {
        panic!("expected snapshot, got {:?}", reply[0]);
    };
    peer.apply_update_v1(update_v1).unwrap();
    assert_eq!(peer.text(), buffer.text);
}

// Tab switches re-OpenBuffer the same file; seeding must be idempotent
// and never clobber CRDT history accumulated by connected clients.
#[test]
fn reseeding_same_file_is_idempotent_and_preserves_client_edits() {
    let hub = CrdtSyncHub::default();
    let path = PathBuf::from("/work/notes.md");
    let buffer_id = crdt_buffer_id_for_path(&path);
    let snapshot = hub.open_buffer(buffer_id.clone(), "original");

    // A client joins and edits the shared document.
    let peer = CrdtTextBuffer::new(7);
    let CrdtServerMessage::Snapshot { update_v1, .. } = &snapshot else {
        panic!("expected snapshot");
    };
    peer.apply_update_v1(update_v1).unwrap();
    let edit = peer
        .apply_local_edit(neoism_ui::editor::crdt::CrdtTextEdit::Insert {
            index: 8,
            content: " edited".into(),
        })
        .unwrap();
    hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
        update: neoism_protocol::crdt::CrdtBufferUpdate {
            buffer_id: buffer_id.clone(),
            origin_client_id: edit.origin_client_id,
            update_v1: edit.update_v1,
            state_vector_v1: edit.state_vector_v1,
        },
    });
    assert_eq!(hub.buffers().text(&buffer_id).unwrap(), "original edited");

    // Re-opening the same file (tab switch) must NOT reset the document
    // back to nvim's on-disk text — the live CRDT state wins.
    hub.open_buffer(buffer_id.clone(), "original");
    assert_eq!(hub.buffers().text(&buffer_id).unwrap(), "original edited");
}
