use super::*;
use crate::editor::crdt::CrdtTextBuffer;
use neoism_protocol::crdt::CrdtClientMessage;
use std::path::Path;

const A: &str = "file:///repo/a.rs";
const B: &str = "file:///repo/b.rs";

fn seed(chrome: &mut Chrome<()>, id: &str, text: &str, client: u64) {
    let messages = chrome.pump_code_document(client, Some(id));
    assert!(
        matches!(&messages[..], [CrdtClientMessage::OpenBuffer { buffer_id, .. }] if buffer_id == id)
    );
    let daemon = CrdtTextBuffer::with_text(100, text);
    let (pane, binding) = chrome.code_editor_for_buffer_mut(id).unwrap();
    binding
        .seed_from_snapshot(&daemon.encode_full_update_v1(), &mut pane.buffer)
        .unwrap();
    assert!(!pane.is_dirty());
}

#[test]
fn git_restore_existing_replica_merges_once_and_retains_new_edits_and_undo() {
    for parked in [false, true] {
        let mut chrome = Chrome::<()>::new();
        chrome.open_editor_file(0, "/repo/a.rs", "new\ntail");
        chrome.pump_code_document(11, Some(A));
        let daemon = CrdtTextBuffer::with_text(100, "new\ntail");
        {
            let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
            binding.seed_from_snapshot(&daemon.encode_full_update_v1(), &mut pane.buffer).unwrap();
        }
        assert!(chrome.git_restore_replica_ready(Path::new("/repo/a.rs")));
        if parked { chrome.open_editor_file(1, "/repo/b.rs", "other"); }
        daemon.replace(0, 3, "old").unwrap();
        let snapshot = daemon.encode_full_update_v1();
        let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        pane.buffer.cursor_line = 1;
        pane.buffer.cursor_col = 4;
        pane.buffer.insert_text(" local");
        let result = binding.apply_remote(0, &snapshot, &mut pane.buffer).unwrap();
        assert!(result.flushed_local.is_some());
        assert_eq!(pane.buffer.text_for_disk(), "old\ntail local");
        assert!(binding.can_undo());
        binding.apply_remote(0, &snapshot, &mut pane.buffer).unwrap();
        assert_eq!(pane.buffer.text_for_disk(), "old\ntail local");
        assert_eq!(binding.client_id(), 11);
        binding.undo(&mut pane.buffer);
        assert_eq!(chrome.code_editor_for_buffer_mut(A).unwrap().0.buffer.text_for_disk(), "old\ntail");
        assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
        assert_eq!(chrome.code_pane().unwrap().buffer.text_for_disk(), "old\ntail");
    }
}

#[test]
fn git_restore_replica_keeps_disk_line_endings_and_eof() {
    for (initial, restored) in [("new\r\ntail", "old\r\ntail"), ("new\r\ntail\r\n", "old\r\ntail\r\n"), ("new\ntail", "old\ntail")] {
        let mut chrome = Chrome::<()>::new();
        chrome.open_editor_file(0, "/repo/a.rs", initial);
        chrome.pump_code_document(11, Some(A));
        let daemon = CrdtTextBuffer::with_text(100, initial);
        let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        binding.seed_from_snapshot(&daemon.encode_full_update_v1(), &mut pane.buffer).unwrap();
        daemon.replace(0, 3, "old").unwrap();
        binding.apply_remote(0, &daemon.encode_full_update_v1(), &mut pane.buffer).unwrap();
        pane.buffer.mark_saved();
        assert_eq!(pane.buffer.text_for_disk(), restored);
        assert!(!pane.is_dirty());
    }
}

#[test]
fn git_restore_remote_owner_must_be_ready_even_when_clean() {
    let mut chrome = Chrome::<()>::new();
    chrome.git_diff_panel.open(Some("/repo".into()), None);
    chrome.open_editor_file(0, "/repo/a.rs", "new");
    assert!(!chrome.git_restore_replica_ready(Path::new("/repo/a.rs")));
    chrome.code_pane_mut().unwrap().remote_content_pending = true;
    assert!(!chrome.prepare_git_panel_restore(Path::new("a.rs")));
    chrome.code_pane_mut().unwrap().remote_content_pending = false;
    chrome.code_pane_mut().unwrap().error = Some("load failed".into());
    assert!(!chrome.prepare_git_panel_restore(Path::new("a.rs")));
    let markdown = chrome.buffer_tabs.open_markdown("/repo/parked.md".into());
    chrome.buffer_tabs.set_modified(Path::new("/repo/parked.md"), true);
    assert!(!chrome.prepare_git_panel_restore(Path::new("parked.md")));
    assert!(!chrome.prepare_git_panel_stage(Path::new("parked.md")));
    assert!(chrome.buffer_tabs.tabs()[markdown].modified);
}

fn parked_round_trip(restore_by_open: bool, flush_before_park: bool) {
    let mut chrome = Chrome::<()>::new();
    chrome.open_editor_file(0, "/repo/a.rs", "base");
    seed(&mut chrome, A, "base", 11);
    let peer = CrdtTextBuffer::new(22);
    let anchor = {
        let (_, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        peer.apply_update_v1(&binding.encode_full_update_v1().update_v1)
            .unwrap();
        binding.sticky_anchor_at(0, 0, true).unwrap()
    };
    chrome.code_pane_mut().unwrap().buffer.insert_text("local ");
    assert!(chrome.code_pane().unwrap().is_dirty());
    if flush_before_park {
        // Exactly the pump used by WASM when B's click changed the view path
        // but A is still the hosted CodePane. No focus must not drop A.
        let messages = chrome.pump_code_document(11, None);
        assert!(
            matches!(&messages[..], [CrdtClientMessage::ApplySync { envelope }] if envelope.buffer_id == A)
        );
        let (_, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        assert!(binding.is_seeded());
        assert!(binding.can_undo());
        assert_eq!(binding.doc_text(), "local base");
    }
    chrome.open_editor_file(1, "/repo/b.rs", "other");
    seed(&mut chrome, B, "other", 11);
    assert!(chrome.pump_code_document(11, None).is_empty());
    // A receives collaborator edits while parked. apply_remote must flush any
    // still-unsent A edits before merging, never seed/reset the saved baseline.
    let remote = peer
        .apply_local_edit(crate::editor::crdt::CrdtTextEdit::Insert {
            index: 4,
            content: " remote".into(),
        })
        .unwrap();
    let full = {
        let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        let result = binding
            .apply_remote(22, &remote.update_v1, &mut pane.buffer)
            .unwrap();
        assert!(result.changed);
        assert_eq!(result.flushed_local.is_some(), !flush_before_park);
        assert!(pane.is_dirty());
        assert!(binding.can_undo());
        assert_eq!(pane.buffer.lines.join("\n"), "local base remote");
        assert_eq!(binding.resolve_sticky_anchor(&anchor), Some((0, 6)));
        binding.encode_full_update_v1()
    };
    assert_eq!(chrome.code_pane().unwrap().buffer.lines.join("\n"), "other");
    assert!(chrome.code_editor_for_buffer_mut("missing").is_none());
    if restore_by_open {
        // Even a stale host disk load must not overwrite a retained replica.
        chrome.open_editor_file(0, "/repo/a.rs", "stale disk bytes");
    } else {
        assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    }
    assert!(chrome.pump_code_document(11, Some(A)).is_empty());
    let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
    assert_eq!(pane.path, Path::new("/repo/a.rs"));
    assert_eq!(binding.buffer_id(), A);
    assert_eq!(binding.client_id(), 11);
    assert!(binding.is_seeded());
    assert_eq!(binding.encode_full_update_v1(), full);
    assert_eq!(pane.buffer.lines.join("\n"), "local base remote");
    assert!(pane.is_dirty());
    assert!(binding.can_undo());
    assert!(pane.buffer.undo());
    assert!(
        matches!(&chrome.pump_code_document(11, None)[..], [CrdtClientMessage::ApplySync { envelope }] if envelope.buffer_id == A)
    );
    let pane = chrome.code_pane_mut().unwrap();
    assert_eq!(pane.buffer.lines.join("\n"), "base remote");
    assert!(pane.is_dirty()); // remote edits did not move the disk baseline
    assert!(pane.buffer.redo());
    assert!(
        matches!(&chrome.pump_code_document(11, None)[..], [CrdtClientMessage::ApplySync { envelope }] if envelope.buffer_id == A)
    );
    assert_eq!(
        chrome.code_pane().unwrap().buffer.lines.join("\n"),
        "local base remote"
    );
    // Both directions retain the canonical pair, including a same-path reopen.
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/b.rs")));
    assert!(chrome.pump_code_document(11, Some(B)).is_empty());
    assert!(!chrome.code_pane().unwrap().is_dirty());
    chrome.open_editor_file(1, "/repo/b.rs", "stale B disk bytes");
    assert_eq!(chrome.code_pane().unwrap().buffer.lines.join("\n"), "other");
    assert!(chrome.activate_git_diff_editor(Path::new("/repo/a.rs")));
    assert!(chrome.code_pane().unwrap().is_dirty());
    chrome.close_editor_panes();
    assert!(chrome.code_editor_for_buffer_mut(A).is_none());
    assert!(chrome.code_editor_for_buffer_mut(B).is_none());
    assert!(chrome.pump_code_document(11, None).is_empty());
}

#[test]
fn source_navigation_opens_special_extensions_as_code_and_never_reseeds_retained_text() {
    for path in ["/repo/note.md", "/repo/book.ipynb", "/repo/sketch.neodraw"] {
        let mut chrome = Chrome::<()>::new();
        let tab = chrome.buffer_tabs.open_path(path.into());
        chrome.set_active_tab_index(tab);
        assert_eq!(chrome.source_editor_policy(Path::new(path)), "load");
        assert!(chrome.open_source_file(tab, path, "first\nsecond\nthird"));
        assert_eq!(chrome.active_editor_pane_kind(), Some(EditorPaneKind::Code));
        chrome.code_pane_mut().unwrap().buffer.insert_text("dirty ");
        assert!(chrome.focus_source_editor(tab, Path::new(path), 2, 1));
        assert_eq!(chrome.code_pane().unwrap().buffer.cursor().line, 2);
        assert!(chrome.open_source_file(tab, path, "stale bytes"));
        let pane = chrome.code_pane_mut().unwrap();
        assert_eq!(pane.buffer.text_for_disk(), "dirty first\nsecond\nthird");
        assert_eq!(pane.buffer.cursor().line, 2);
        assert!(pane.is_dirty());
        assert!(pane.buffer.undo());
        assert_eq!(pane.buffer.text_for_disk(), "first\nsecond\nthird");
    }
}

#[test]
fn source_navigation_wrong_tab_metadata_never_moves_current_file() {
    let mut chrome = Chrome::<()>::new();
    let tab = chrome.buffer_tabs.open_path("/repo/current.rs".into());
    chrome.set_active_tab_index(tab);
    chrome.open_editor_file(tab, "/repo/current.rs", "current\nfile");
    chrome
        .code_pane_mut()
        .unwrap()
        .buffer
        .set_cursor_position(1, 2, false);
    let before = chrome.code_pane().unwrap().buffer.cursor();
    assert!(!chrome.focus_source_editor(tab, Path::new("/repo/target.md"), 0, 0));
    assert_eq!(
        chrome.code_pane().unwrap().path,
        Path::new("/repo/current.rs")
    );
    assert_eq!(chrome.code_pane().unwrap().buffer.cursor(), before);
}

#[test]
fn source_navigation_rejects_active_and_parked_structured_owners() {
    for path in ["/repo/book.ipynb", "/repo/sketch.neodraw"] {
        let mut chrome = Chrome::<()>::new();
        chrome.open_editor_file(0, path, "{}");
        assert_eq!(chrome.source_editor_policy(Path::new(path)), "unsupported");
        assert!(!chrome.open_source_file(0, path, "raw bytes"));
        chrome.open_editor_file(1, "/repo/other.rs", "other");
        assert_eq!(chrome.source_editor_policy(Path::new(path)), "unsupported");
        assert!(!chrome.open_source_file(0, path, "raw bytes"));
        assert_eq!(
            chrome.code_pane().unwrap().path,
            Path::new("/repo/other.rs")
        );
    }
}

#[test]
fn source_navigation_rejects_dirty_markdown_owner_without_duplicate_or_reseed() {
    let mut chrome = Chrome::<()>::new();
    let path = "/repo/note.md";
    chrome.set_markdown_content(Some("# Draft\nbody".into()), Some(path));
    chrome.markdown_pane_mut().unwrap().insert_text("dirty ");
    let before = chrome.markdown_pane_mut().unwrap().lines.clone();
    let tab = chrome.buffer_tabs.open_path(path.into());
    chrome.set_active_tab_index(tab);
    assert_eq!(chrome.source_editor_policy(Path::new(path)), "unsupported");
    assert!(!chrome.open_source_file(tab, path, "stale disk"));
    assert!(!chrome.focus_source_editor(tab, Path::new(path), 1, 0));
    assert!(chrome.code_pane().is_none());
    let pane = chrome.markdown_pane_mut().unwrap();
    assert_eq!(pane.lines, before);
    assert!(pane.is_dirty());
    assert!(pane.undo());
    assert_eq!(pane.lines, ["# Draft", "body"]);
}

#[test]
fn source_navigation_reuses_active_and_parked_replica_only_after_source_tab_exists() {
    for parked in [false, true] {
        let mut chrome = Chrome::<()>::new();
        chrome.open_editor_file(0, "/repo/a.rs", "first\nsecond\nthird");
        seed(&mut chrome, A, "first\nsecond\nthird", 11);
        chrome.code_pane_mut().unwrap().buffer.insert_text("local ");
        chrome.pump_code_document(11, Some(A));
        let full = chrome
            .code_editor_for_buffer_mut(A)
            .unwrap()
            .1
            .encode_full_update_v1();
        if parked {
            chrome.open_editor_file(1, "/repo/b.rs", "other");
            seed(&mut chrome, B, "other", 11);
        }
        let review = chrome.buffer_tabs.open_git_diff(7, Path::new("/repo/a.rs"));
        chrome.set_active_tab_index(review);
        // A retained pane alone is not a source tab: never steal review focus.
        assert!(!chrome.focus_source_editor(review, Path::new("/repo/a.rs"), 1, 2));
        let source = chrome.buffer_tabs.open_path("/repo/a.rs".into());
        chrome.set_active_tab_index(source);
        assert!(chrome.focus_source_editor(source, Path::new("/repo/a.rs"), 1, 2));
        assert!(chrome.active_chrome_page().is_none());
        assert_eq!(chrome.active_editor_pane_kind(), Some(EditorPaneKind::Code));
        let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
        assert_eq!(pane.buffer.cursor().line, 1);
        assert_eq!(pane.buffer.cursor().col, 2);
        assert!(pane.buffer.follow_cursor);
        assert!(pane.is_dirty());
        assert_eq!(pane.buffer.text_for_disk(), "local first\nsecond\nthird");
        assert_eq!(binding.encode_full_update_v1(), full);
        assert!(binding.can_undo());
        assert!(pane.buffer.undo());
        chrome.pump_code_document(11, Some(A));
        let pane = chrome.code_pane_mut().unwrap();
        assert_eq!(pane.buffer.text_for_disk(), "first\nsecond\nthird");
        assert!(pane.buffer.redo());
        chrome.pump_code_document(11, Some(A));
        assert_eq!(
            chrome.code_pane().unwrap().buffer.text_for_disk(),
            "local first\nsecond\nthird"
        );
        assert!(chrome.pump_code_document(11, Some(A)).is_empty());
    }
}

#[test]
fn diff_restore_retains_seeded_replica_after_null_focus_flush() {
    parked_round_trip(false, true);
}

#[test]
fn file_restore_retains_seeded_replica_after_null_focus_flush() {
    parked_round_trip(true, true);
}

#[test]
fn diff_restore_merges_remote_into_unsent_parked_document() {
    parked_round_trip(false, false);
}

#[test]
fn file_restore_merges_remote_into_unsent_parked_document() {
    parked_round_trip(true, false);
}

#[test]
fn parked_replica_survives_non_code_focus_and_accepts_save_acknowledgement() {
    let mut chrome = Chrome::<()>::new();
    chrome.open_editor_file(0, "/repo/a.rs", "base");
    seed(&mut chrome, A, "base", 11);
    chrome.code_pane_mut().unwrap().buffer.insert_text("local ");
    chrome.pump_code_document(11, None);
    chrome.open_editor_file(1, "/repo/drawing.neodraw", "{}");
    assert!(chrome.code_pane().is_none());
    assert!(chrome.pump_code_document(11, None).is_empty());
    let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
    assert!(binding.can_undo());
    assert!(pane.is_dirty());
    // The inbound Saved route uses this same buffer-id lookup, not focus.
    pane.buffer.mark_saved();
    chrome.open_editor_file(0, "/repo/a.rs", "stale disk");
    assert!(chrome.pump_code_document(11, Some(A)).is_empty());
    let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
    assert!(!pane.is_dirty());
    assert_eq!(pane.buffer.lines.join("\n"), "local base");
    assert!(binding.can_undo());
}

#[test]
fn clean_bound_restore_does_not_reseed_from_stale_disk() {
    let mut chrome = Chrome::<()>::new();
    chrome.open_editor_file(0, "/repo/a.rs", "base");
    seed(&mut chrome, A, "base", 11);
    let full = chrome
        .code_editor_for_buffer_mut(A)
        .unwrap()
        .1
        .encode_full_update_v1();
    chrome.open_editor_file(1, "/repo/b.rs", "other");
    chrome.open_editor_file(0, "/repo/a.rs", "stale");
    let (pane, binding) = chrome.code_editor_for_buffer_mut(A).unwrap();
    assert!(!pane.is_dirty());
    assert_eq!(pane.buffer.lines.join("\n"), "base");
    assert_eq!(binding.encode_full_update_v1(), full);
}
