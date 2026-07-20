//! Wave 7B: the markdown pane joins the CRDT document plane.
//!
//! These tests drive a REAL `MarkdownPane` + `MarkdownDocBinding` (the
//! exact client stack the desktop ships) against the daemon's
//! `CrdtSyncHub`, proving:
//!
//! - a markdown-pane edit reaches the authoritative doc as ONE minimal
//!   client-origin op and broadcasts exactly once (no daemon echo),
//! - a daemon-origin lines change lands in the pane as an incremental
//!   splice with the caret transformed through it,
//! - a remote-applied change is never re-emitted as a local op.

use std::path::PathBuf;

use neoism_protocol::crdt::{CrdtClientMessage, CrdtServerMessage, CrdtSyncEnvelope};
use neoism_ui::editor::crdt::CrdtTextUpdate;
use neoism_ui::editor::markdown::doc_sync::{lines_to_text, MarkdownDocBinding};
use neoism_ui::editor::markdown::MarkdownPane;
use neoism_workspace_daemon::crdt::sync::CrdtSyncHub;
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

fn apply_sync(buffer_id: &str, update: CrdtTextUpdate) -> CrdtClientMessage {
    CrdtClientMessage::ApplySync {
        envelope: CrdtSyncEnvelope {
            buffer_id: buffer_id.to_string(),
            origin_client_id: update.origin_client_id,
            update_v1: update.update_v1,
            state_vector_v1: update.state_vector_v1,
        },
    }
}

/// Open a markdown-pane client against the hub exactly the way the
/// desktop does: `OpenBuffer` (idempotent), then seed the binding from
/// the snapshot reply.
fn open_markdown_client(
    hub: &CrdtSyncHub,
    buffer_id: &str,
    client_id: u64,
    disk_text: &str,
) -> (MarkdownDocBinding, MarkdownPane) {
    let mut pane =
        MarkdownPane::from_source(PathBuf::from("/notes/shared.md"), disk_text);
    let mut binding = MarkdownDocBinding::new(client_id, buffer_id);
    let replies = hub.handle_client_message(CrdtClientMessage::OpenBuffer {
        buffer_id: buffer_id.to_string(),
        initial_text: disk_text.to_string(),
    });
    let CrdtServerMessage::Snapshot { update_v1, .. } = &replies[0] else {
        panic!("OpenBuffer must reply with a snapshot, got {replies:?}");
    };
    binding
        .seed_from_snapshot(update_v1, &mut pane)
        .expect("snapshot seeds");
    (binding, pane)
}

// ---------------------------------------------------------------------
// Pure hub tests (no nvim binary required).
// ---------------------------------------------------------------------

#[test]
fn markdown_pane_edit_reaches_doc_as_one_client_origin_op() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(910));
    let buffer_id = "file:///w/notes.md";
    let (mut binding, mut pane) =
        open_markdown_client(&hub, buffer_id, 41, "alpha\nbravo\ncharlie");
    let mut rx = hub.subscribe();

    // Edit through a real pane entry point and flush the choke point.
    pane.cursor_line = 1;
    pane.cursor_col = 5;
    pane.insert_text(" edited");
    let update = binding.flush_local(&pane).expect("edit emits an op");
    hub.handle_client_message(apply_sync(buffer_id, update));

    assert_eq!(
        hub.buffers().text(buffer_id).unwrap(),
        "alpha\nbravo edited\ncharlie"
    );
    let syncs = drain_syncs(&mut rx);
    assert_eq!(syncs.len(), 1, "exactly one Sync per pane edit: {syncs:?}");
    assert_eq!(syncs[0].origin_client_id, 41, "op keeps the CLIENT origin");
    assert_ne!(
        syncs[0].origin_client_id,
        hub.daemon_client_id(),
        "a daemon-origin re-emit here would be the echo loop"
    );
}

#[test]
fn nvim_change_lands_in_markdown_pane_with_caret_transform_and_no_echo() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(911));
    let buffer_id = "file:///w/notes.md";
    let (mut binding, mut pane) =
        open_markdown_client(&hub, buffer_id, 42, "alpha\nbravo\ncharlie");
    let mut rx = hub.subscribe();

    // The pane's caret sits on "charlie".
    pane.cursor_line = 2;
    pane.cursor_col = 4;

    // nvim inserts two lines at the top (daemon-origin minimal update).
    hub.apply_nvim_lines_change(buffer_id, 0, 0, 2, "zero\nhalf", hub.daemon_client_id())
        .expect("tracked buffer accepts the change");

    // The pane applies the broadcast incrementally.
    for envelope in drain_syncs(&mut rx) {
        let result = binding
            .apply_remote(envelope.origin_client_id, &envelope.update_v1, &mut pane)
            .expect("remote applies");
        assert!(result.flushed_local.is_none(), "no pending local edits");
    }

    assert_eq!(
        lines_to_text(&pane.lines),
        "zero\nhalf\nalpha\nbravo\ncharlie"
    );
    // Caret followed its line down through the remote insertion.
    assert_eq!((pane.cursor_line, pane.cursor_col), (4, 4));

    // Echo guard: applying the remote change must NOT re-emit it as a
    // local op on the next flush.
    assert!(
        binding.flush_local(&pane).is_none(),
        "remote-applied change re-emitted as a local op (echo loop)"
    );
    assert!(drain_syncs(&mut rx).is_empty());
}

#[test]
fn two_markdown_panes_and_nvim_style_edits_converge_without_echo() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(912));
    let buffer_id = "file:///w/notes.md";
    let (mut binding_a, mut pane_a) =
        open_markdown_client(&hub, buffer_id, 51, "one\ntwo\nthree");
    let (mut binding_b, mut pane_b) =
        open_markdown_client(&hub, buffer_id, 52, "one\ntwo\nthree");
    let mut rx = hub.subscribe();

    // Concurrent-ish edits: A edits line 0, B edits line 2, nvim edits
    // line 1 — all land on the hub, broadcasts fan back out.
    pane_a.cursor_line = 0;
    pane_a.cursor_col = 3;
    pane_a.insert_text(" A");
    hub.handle_client_message(apply_sync(
        buffer_id,
        binding_a.flush_local(&pane_a).unwrap(),
    ));

    pane_b.cursor_line = 2;
    pane_b.cursor_col = 5;
    pane_b.insert_text(" B");
    hub.handle_client_message(apply_sync(
        buffer_id,
        binding_b.flush_local(&pane_b).unwrap(),
    ));

    hub.apply_nvim_lines_change(buffer_id, 1, 2, 1, "two NVIM", hub.daemon_client_id())
        .expect("nvim change applies");

    // Both panes replay the full broadcast stream (including their own
    // updates — the echo guard must skip those).
    let syncs = drain_syncs(&mut rx);
    assert_eq!(syncs.len(), 3);
    let mut emitted = Vec::new();
    for envelope in &syncs {
        for (binding, pane) in
            [(&mut binding_a, &mut pane_a), (&mut binding_b, &mut pane_b)]
        {
            let result = binding
                .apply_remote(envelope.origin_client_id, &envelope.update_v1, pane)
                .expect("remote applies");
            if let Some(update) = result.flushed_local {
                emitted.push(update);
            }
        }
    }
    assert!(
        emitted.is_empty(),
        "replaying broadcasts emitted new local ops (echo loop)"
    );

    let doc = hub.buffers().text(buffer_id).unwrap();
    assert_eq!(doc, "one A\ntwo NVIM\nthree B");
    assert_eq!(lines_to_text(&pane_a.lines), doc);
    assert_eq!(lines_to_text(&pane_b.lines), doc);
    assert!(binding_a.flush_local(&pane_a).is_none());
    assert!(binding_b.flush_local(&pane_b).is_none());
}

#[test]
fn markdown_pane_undo_through_hub_reverts_only_own_edit() {
    // Wave 7D: per-user undo across a REAL daemon hub. A and B both
    // edit; A's undo must revert only A's edit, broadcast as a normal
    // A-origin op, and converge everywhere — B's text untouched.
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(914));
    let buffer_id = "file:///w/notes.md";
    let (mut binding_a, mut pane_a) =
        open_markdown_client(&hub, buffer_id, 71, "alpha\nbravo");
    let (mut binding_b, mut pane_b) =
        open_markdown_client(&hub, buffer_id, 72, "alpha\nbravo");
    let mut rx = hub.subscribe();

    // A edits line 0, B edits line 1; both fan through the hub.
    pane_a.cursor_line = 0;
    pane_a.cursor_col = 5;
    pane_a.insert_text(" A");
    hub.handle_client_message(apply_sync(
        buffer_id,
        binding_a.flush_local(&pane_a).unwrap(),
    ));
    pane_b.cursor_line = 1;
    pane_b.cursor_col = 5;
    pane_b.insert_text(" B");
    hub.handle_client_message(apply_sync(
        buffer_id,
        binding_b.flush_local(&pane_b).unwrap(),
    ));
    for envelope in drain_syncs(&mut rx) {
        for (binding, pane) in
            [(&mut binding_a, &mut pane_a), (&mut binding_b, &mut pane_b)]
        {
            binding
                .apply_remote(envelope.origin_client_id, &envelope.update_v1, pane)
                .expect("remote applies");
        }
    }
    assert_eq!(lines_to_text(&pane_a.lines), "alpha A\nbravo B");

    // A undoes: only A's edit reverts locally...
    let result = binding_a.undo(&mut pane_a);
    assert!(result.changed);
    assert_eq!(lines_to_text(&pane_a.lines), "alpha\nbravo B");
    let history = result.history_update.expect("undo emits an op");
    assert_eq!(history.origin_client_id, 71, "undo keeps A's origin");

    // ...and the revert is a normal sync op through the hub.
    hub.handle_client_message(apply_sync(buffer_id, history));
    assert_eq!(hub.buffers().text(buffer_id).unwrap(), "alpha\nbravo B");
    for envelope in drain_syncs(&mut rx) {
        for (binding, pane) in
            [(&mut binding_a, &mut pane_a), (&mut binding_b, &mut pane_b)]
        {
            let result = binding
                .apply_remote(envelope.origin_client_id, &envelope.update_v1, pane)
                .expect("remote applies");
            assert!(result.flushed_local.is_none(), "undo echo re-emitted");
        }
    }
    assert_eq!(lines_to_text(&pane_a.lines), "alpha\nbravo B");
    assert_eq!(lines_to_text(&pane_b.lines), "alpha\nbravo B");

    // Redo round-trip converges both panes back.
    let result = binding_a.redo(&mut pane_a);
    assert!(result.changed);
    hub.handle_client_message(apply_sync(
        buffer_id,
        result.history_update.expect("redo emits an op"),
    ));
    for envelope in drain_syncs(&mut rx) {
        for (binding, pane) in
            [(&mut binding_a, &mut pane_a), (&mut binding_b, &mut pane_b)]
        {
            binding
                .apply_remote(envelope.origin_client_id, &envelope.update_v1, pane)
                .expect("remote applies");
        }
    }
    let doc = hub.buffers().text(buffer_id).unwrap();
    assert_eq!(doc, "alpha A\nbravo B");
    assert_eq!(lines_to_text(&pane_a.lines), doc);
    assert_eq!(lines_to_text(&pane_b.lines), doc);
    assert!(binding_a.flush_local(&pane_a).is_none());
    assert!(binding_b.flush_local(&pane_b).is_none());
}

#[test]
fn open_buffer_is_idempotent_and_existing_doc_wins_over_disk_text() {
    let hub = CrdtSyncHub::new(CrdtBufferRegistry::with_daemon_client_id(913));
    let buffer_id = "file:///w/notes.md";

    // First client opens and edits.
    let (mut binding_a, mut pane_a) = open_markdown_client(&hub, buffer_id, 61, "base");
    pane_a.cursor_col = 4;
    pane_a.insert_text(" plus edits");
    hub.handle_client_message(apply_sync(
        buffer_id,
        binding_a.flush_local(&pane_a).unwrap(),
    ));

    // Second client joins later with the STALE on-disk text: the doc
    // wins and the late pane is reconciled to the live state.
    let (binding_b, pane_b) = open_markdown_client(&hub, buffer_id, 62, "base");
    assert_eq!(lines_to_text(&pane_b.lines), "base plus edits");
    assert_eq!(binding_b.doc_text(), "base plus edits");
    assert_eq!(hub.buffers().text(buffer_id).unwrap(), "base plus edits");
}
