//! Wave 6C: bidirectional CRDT cutover for editor buffers.
//!
//! Covers both sync directions plus the echo-loop guards:
//!
//! - nvim→CRDT: an `on_lines` change folds into the authoritative
//!   replica as a MINIMAL daemon-origin update (no full re-seed) that a
//!   subscribed peer can apply incrementally.
//! - No echo loop: a CRDT-applied remote update must NOT re-emit itself
//!   as a fresh daemon-origin update.

use neoism_protocol::crdt::{
    CrdtBufferUpdate, CrdtClientMessage, CrdtServerMessage, CrdtSyncEnvelope,
};
use neoism_ui::editor::crdt::{CrdtTextBuffer, CrdtTextEdit};
use neoism_workspace_daemon::crdt::sync::{min_utf16_replace, CrdtSyncHub};
use neoism_workspace_daemon::crdt::CrdtBufferRegistry;

fn drain_syncs(
    rx: &mut tokio::sync::broadcast::Receiver<CrdtServerMessage>,
) -> Vec<CrdtSyncEnvelope> {
    let mut out = Vec::new();
    while let Ok(message) = rx.try_recv() {
        if let CrdtServerMessage::Sync { envelope } = message {
            out.push(envelope);
        }
    }
    out
}

fn seed_peer_from_hub(
    hub: &CrdtSyncHub,
    buffer_id: &str,
    client_id: u64,
) -> CrdtTextBuffer {
    let peer = CrdtTextBuffer::new(client_id);
    let snapshot = hub
        .buffers()
        .snapshot_for(buffer_id, &[])
        .expect("seeded buffer snapshots");
    peer.apply_update_v1(&snapshot.update_v1)
        .expect("snapshot applies");
    peer
}

// ---------------------------------------------------------------------
// nvim→CRDT: incremental line changes (no nvim binary required — the
// on_lines payload is simulated exactly as the lua bridge reports it).
// ---------------------------------------------------------------------

#[test]
fn nvim_lines_change_applies_minimal_daemon_origin_update() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(900));
    hub.open_buffer("file:///w/a.rs", "alpha\nbravo\ncharlie");
    let peer = seed_peer_from_hub(&hub, "file:///w/a.rs", 11);
    let mut rx = hub.subscribe();

    // nvim reports: line 1 replaced ("bravo" → "bravo edited").
    let reply = hub
        .apply_nvim_lines_change(
            "file:///w/a.rs",
            1,
            2,
            1,
            "bravo edited",
            hub.daemon_client_id(),
        )
        .expect("tracked buffer accepts the change");

    assert_eq!(
        hub.buffers().text("file:///w/a.rs").unwrap(),
        "alpha\nbravo edited\ncharlie"
    );
    let CrdtServerMessage::Sync { envelope } = reply else {
        panic!("expected Sync reply, got {reply:?}");
    };
    assert_eq!(envelope.origin_client_id, hub.daemon_client_id());

    // The broadcast carries the SAME daemon-origin update, and it is
    // incremental: a peer that only had the seed converges by applying
    // just these update bytes (not a re-snapshot).
    let syncs = drain_syncs(&mut rx);
    assert_eq!(syncs.len(), 1, "exactly one Sync broadcast, got {syncs:?}");
    assert_eq!(syncs[0].origin_client_id, hub.daemon_client_id());
    peer.apply_update_v1(&syncs[0].update_v1).unwrap();
    assert_eq!(peer.text(), "alpha\nbravo edited\ncharlie");
}

#[test]
fn nvim_lines_deletion_and_insertion_edge_cases() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(901));
    hub.open_buffer("file:///w/b.rs", "one\ntwo\nthree");

    // Pure deletion: on_lines reports new_line_count == 0.
    hub.apply_nvim_lines_change("file:///w/b.rs", 1, 2, 0, "", hub.daemon_client_id())
        .expect("deletion applies");
    assert_eq!(hub.buffers().text("file:///w/b.rs").unwrap(), "one\nthree");

    // Insertion at top: lines [0, 0) replaced with two new lines.
    hub.apply_nvim_lines_change(
        "file:///w/b.rs",
        0,
        0,
        2,
        "zero\nhalf",
        hub.daemon_client_id(),
    )
    .expect("insertion applies");
    assert_eq!(
        hub.buffers().text("file:///w/b.rs").unwrap(),
        "zero\nhalf\none\nthree"
    );

    // Replacing with an explicit single EMPTY line is distinct from a
    // deletion (new_line_count disambiguates the empty joined text).
    hub.apply_nvim_lines_change("file:///w/b.rs", 0, 1, 1, "", hub.daemon_client_id())
        .expect("blanking a line applies");
    assert_eq!(
        hub.buffers().text("file:///w/b.rs").unwrap(),
        "\nhalf\none\nthree"
    );

    // Untracked buffers are ignored (no panic, no broadcast).
    assert!(hub
        .apply_nvim_lines_change(
            "file:///w/missing.rs",
            0,
            1,
            1,
            "x",
            hub.daemon_client_id()
        )
        .is_none());
}

#[test]
fn nvim_lines_change_is_a_noop_when_text_already_matches() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(902));
    hub.open_buffer("file:///w/c.rs", "same\ntext");
    let mut rx = hub.subscribe();

    // nvim re-reports a line that already matches the replica (e.g. a
    // formatting pass that changed nothing): no update, no broadcast.
    assert!(hub
        .apply_nvim_lines_change(
            "file:///w/c.rs",
            0,
            1,
            1,
            "same",
            hub.daemon_client_id()
        )
        .is_none());
    assert!(drain_syncs(&mut rx).is_empty());
}

#[test]
fn min_utf16_replace_trims_to_the_changed_span() {
    // Plain ASCII: only the changed middle is replaced.
    assert_eq!(
        min_utf16_replace("alpha\nbravo\ncharlie", "alpha\nbravo edited\ncharlie"),
        Some((11, 0, " edited".to_string()))
    );
    // Identical texts produce no edit.
    assert_eq!(min_utf16_replace("same", "same"), None);
    // Multibyte scalars never split, and offsets are UTF-16 units:
    // "🦀" is 4 UTF-8 bytes but 2 UTF-16 code units.
    let (index, len, content) = min_utf16_replace("a🦀b", "a🦀🦀b").expect("differs");
    assert_eq!((index, len), (3, 0));
    assert_eq!(content, "🦀");
    // Replacement across similar multibyte chars stays on char bounds.
    let (index, len, content) = min_utf16_replace("aéz", "aèz").expect("differs");
    assert_eq!((index, len), (1, 1));
    assert_eq!(content, "è");
}

// ---------------------------------------------------------------------
// Remote client update: applied once, broadcast once, no daemon echo.
// ---------------------------------------------------------------------

#[test]
fn remote_update_broadcasts_once_and_does_not_reemit_itself() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(903));
    hub.open_buffer("file:///w/d.rs", "hello");
    let peer = seed_peer_from_hub(&hub, "file:///w/d.rs", 7);
    let mut rx = hub.subscribe();

    let edit = peer
        .apply_local_edit(CrdtTextEdit::Insert {
            index: 5,
            content: " world".into(),
        })
        .unwrap();
    hub.handle_client_message(CrdtClientMessage::ApplyUpdate {
        update: CrdtBufferUpdate {
            buffer_id: "file:///w/d.rs".into(),
            origin_client_id: edit.origin_client_id,
            update_v1: edit.update_v1,
            state_vector_v1: edit.state_vector_v1,
        },
    });

    assert_eq!(hub.buffers().text("file:///w/d.rs").unwrap(), "hello world");
    let syncs = drain_syncs(&mut rx);
    assert_eq!(syncs.len(), 1, "exactly one Sync for one client update");
    assert_eq!(
        syncs[0].origin_client_id, 7,
        "the broadcast keeps the CLIENT origin; a daemon-origin re-emit \
         here would be the echo loop"
    );
}
