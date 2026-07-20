use super::*;
use crate::context::factories::create_dead_context;
use crate::event::VoidListener;
use crate::layout::ContextDimension;
use neoism_protocol::editor::GridCell;
use neoism_terminal_core::crosswords::pos::{Column, Line};
use std::path::PathBuf;

#[test]
fn daemon_grid_clear_maps_to_nvim_clear_redraw_event() {
    let events = editor_message_to_redraw_events(EditorServerMessage::GridClear {
        surface_id: Some("pane:root".into()),
        grid_id: 2,
    });

    assert_eq!(events.len(), 1);
    match &events[0] {
        RedrawEvent::Clear { grid } => assert_eq!(*grid, 2),
        other => panic!("unexpected redraw event: {other:?}"),
    }
}

#[test]
fn daemon_grid_update_batches_contiguous_cells_into_grid_lines() {
    let events = editor_message_to_redraw_events(EditorServerMessage::GridUpdate {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        width: 80,
        height: 24,
        cells: vec![
            GridCell {
                row: 3,
                col: 4,
                ch: "a".into(),
                fg: 0x00ff_ffff,
                bg: 0x0000_0000,
                attrs: 0,
            },
            GridCell {
                row: 3,
                col: 5,
                ch: "b".into(),
                fg: 0x00ff_ffff,
                bg: 0x0000_0000,
                attrs: 0,
            },
            GridCell {
                row: 3,
                col: 8,
                ch: "c".into(),
                fg: 0x00ff_ffff,
                bg: 0x0000_0000,
                attrs: 0,
            },
        ],
        cursor: None,
        mode: None,
    });

    let grid_lines: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            RedrawEvent::GridLine {
                row,
                column_start,
                cells,
                ..
            } => Some((*row, *column_start, cells)),
            _ => None,
        })
        .collect();

    assert_eq!(grid_lines.len(), 2);
    assert_eq!(grid_lines[0].0, 3);
    assert_eq!(grid_lines[0].1, 4);
    assert_eq!(grid_lines[0].2.len(), 2);
    assert_eq!(grid_lines[0].2[0].text, "a");
    assert_eq!(grid_lines[0].2[1].text, "b");
    assert_eq!(grid_lines[1].0, 3);
    assert_eq!(grid_lines[1].1, 8);
    assert_eq!(grid_lines[1].2.len(), 1);
    assert_eq!(grid_lines[1].2[0].text, "c");
}

#[test]
fn daemon_editor_messages_drain_without_local_redraw_receiver() {
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        42,
        0,
        ContextDimension::default(),
    );
    assert!(context.editor_redraw_rx.is_none());

    context.enqueue_daemon_editor_message(EditorServerMessage::GridUpdate {
        surface_id: Some("42".into()),
        grid_id: 1,
        width: 80,
        height: 24,
        cells: vec![GridCell {
            row: 0,
            col: 0,
            ch: "Z".into(),
            fg: 0x00ff_ffff,
            bg: 0x0000_0000,
            attrs: 0,
        }],
        cursor: None,
        mode: None,
    });

    let (applied, limited) = context.pump_editor_redraws();
    assert!(applied > 0);
    assert!(!limited);
    assert_eq!(context.terminal.lock().grid[Line(0)][Column(0)].c(), 'Z');
}

#[test]
fn viewport_round_trip_preserves_daemon_diagnostic_cache() {
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        46,
        0,
        ContextDimension::default(),
    );
    context.editor_path = Some(PathBuf::from("/repo/main.gd"));
    context.apply_daemon_editor_sideband(&EditorServerMessage::Diagnostics {
        surface_id: Some("46".into()),
        error: 1,
        warn: 0,
        info: 0,
        hint: 0,
        file_path: Some(PathBuf::from("/repo/main.gd")),
        items: vec![neoism_protocol::editor::DiagnosticItem {
            severity: neoism_protocol::editor::DiagnosticSeverity::Error,
            message: "broken expression".into(),
            source: Some("godot".into()),
            line: 122,
            col: 4,
            end_line: 122,
            end_col: 10,
            lnum: 123,
            code: Some("parse-error".into()),
            code_description: None,
            tags: Vec::new(),
            related_information: Vec::new(),
        }],
    });

    let update_viewport = |context: &mut Context<VoidListener>, topline| {
        context.enqueue_daemon_editor_message(EditorServerMessage::WinViewport {
            surface_id: Some("46".into()),
            grid_id: 1,
            topline,
            botline: topline + 10,
            line_count: 200,
            scroll_delta: 0.0,
            curline: topline,
            curcol: 0,
            textoff: 0,
        });
        let _ = context.pump_editor_redraws();
    };

    update_viewport(&mut context, 120);
    assert_eq!(context.editor_diagnostics.as_ref().unwrap().items.len(), 1);
    assert_eq!(
        context.editor_diagnostics.as_ref().unwrap().items[0].lnum,
        123
    );

    update_viewport(&mut context, 150);
    assert_eq!(
        context.editor_diagnostics.as_ref().unwrap().items[0].lnum,
        123,
        "virtualizing the diagnostic offscreen must not evict it",
    );

    update_viewport(&mut context, 120);
    assert_eq!(
        context.editor_diagnostics.as_ref().unwrap().items[0].lnum,
        123,
        "returning to the original viewport must reuse the cached item",
    );
}

#[test]
fn opening_a_different_buffer_clears_stale_lsp_and_diagnostics_immediately() {
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        47,
        0,
        ContextDimension::default(),
    );
    context.editor_path = Some(PathBuf::from("/repo/src/main.rs"));
    context.editor_lsp_status = Some("active".into());
    context.lsp_snapshot = Some(LspSnapshotNotification {
        filetype: "rust".into(),
        servers: vec![LspSnapshotServer {
            name: "Rust".into(),
            binary: "rust-analyzer".into(),
            filetype: "rust".into(),
            state: "attached".into(),
            source: Some("extension".into()),
            message: None,
            level: None,
        }],
    });
    context.editor_diagnostics = Some(DiagnosticsNotification {
        error: 1,
        ..DiagnosticsNotification::default()
    });

    context.apply_daemon_editor_sideband(&EditorServerMessage::BufferOpened {
        surface_id: Some("47".into()),
        path: PathBuf::from("/repo/Dockerfile"),
        line_count: 216,
    });

    assert_eq!(context.editor_path, Some(PathBuf::from("/repo/Dockerfile")));
    assert_eq!(context.editor_lsp_status.as_deref(), Some("none"));
    assert!(context.attached_lsps.is_empty());
    assert!(context.lsp_snapshot.is_none());
    assert!(context.lsp_messages.is_empty());
    assert!(context.editor_diagnostics.is_none());

    context.apply_daemon_editor_sideband(&EditorServerMessage::Diagnostics {
        surface_id: Some("47".into()),
        error: 99,
        warn: 0,
        info: 0,
        hint: 0,
        file_path: Some(PathBuf::from("/repo/src/main.rs")),
        items: Vec::new(),
    });
    context.apply_daemon_editor_sideband(&EditorServerMessage::LspStatus {
        surface_id: Some("47".into()),
        state: "active".into(),
        name: Some("Rust".into()),
        binary: Some("rust-analyzer".into()),
        filetype: Some("rust".into()),
    });
    context.apply_daemon_editor_sideband(&EditorServerMessage::LspMessage {
        surface_id: Some("47".into()),
        server: "Rust".into(),
        text: "late rust-analyzer stderr".into(),
        level: "error".into(),
    });
    assert!(context.editor_diagnostics.is_none());
    assert_eq!(context.editor_lsp_status.as_deref(), Some("none"));
    assert!(context.attached_lsps.is_empty());
    assert!(context.lsp_messages.is_empty());

    context.apply_daemon_editor_sideband(&EditorServerMessage::LspSnapshot {
        surface_id: Some("47".into()),
        file_path: Some(PathBuf::from("/repo/src/main.rs")),
        filetype: "rust".into(),
        servers: Vec::new(),
    });
    assert!(
        context.lsp_snapshot.is_none(),
        "a late snapshot for the prior buffer must be rejected",
    );

    context.apply_daemon_editor_sideband(&EditorServerMessage::LspSnapshot {
        surface_id: Some("47".into()),
        file_path: Some(PathBuf::from("/repo/Dockerfile")),
        filetype: "dockerfile".into(),
        servers: vec![neoism_protocol::editor::LspSnapshotServer {
            name: "Docker".into(),
            binary: "docker-language-server".into(),
            filetype: "dockerfile".into(),
            state: "attached".into(),
            source: Some("extension".into()),
            message: None,
            level: None,
        }],
    });
    assert_eq!(
        context
            .lsp_snapshot
            .as_ref()
            .map(|snapshot| snapshot.filetype.as_str()),
        Some("dockerfile"),
    );

    context.apply_daemon_editor_sideband(&EditorServerMessage::LspStatus {
        surface_id: Some("47".into()),
        state: "active".into(),
        name: Some("Docker".into()),
        binary: Some("docker-language-server".into()),
        filetype: Some("dockerfile".into()),
    });
    context.apply_daemon_editor_sideband(&EditorServerMessage::LspMessage {
        surface_id: Some("47".into()),
        server: "Docker".into(),
        text: "ready".into(),
        level: "info".into(),
    });
    assert_eq!(context.attached_lsps.len(), 1);
    assert!(context.lsp_messages.contains_key("Docker"));

    context.apply_daemon_editor_sideband(&EditorServerMessage::LspMessage {
        surface_id: Some("47".into()),
        server: "Rust".into(),
        text: "even later rust-analyzer stderr".into(),
        level: "error".into(),
    });
    assert!(!context.lsp_messages.contains_key("Rust"));
}

#[test]
fn unscoped_lsp_filetypes_use_runtime_routes_not_a_rust_special_case() {
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        48,
        0,
        ContextDimension::default(),
    );
    context.editor_path = Some(PathBuf::from("/repo/web/view.tsx"));

    assert!(context.unscoped_lsp_filetype_targets_active_file(Some("typescript")));
    assert!(context.unscoped_lsp_filetype_targets_active_file(Some("typescriptreact")));
    assert!(!context.unscoped_lsp_filetype_targets_active_file(Some("rust")));
    assert!(!context.unscoped_lsp_filetype_targets_active_file(None));
}

#[test]
fn typing_prediction_paints_confirms_and_expires() {
    use neoism_backend::performer::nvim_events::EditorMode;
    let mut dimension = ContextDimension::default();
    dimension.columns = 20;
    dimension.lines = 5;
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        44,
        0,
        dimension,
    );
    context.editor_mode = EditorMode::Insert;
    context.editor_grid_id = Some(1);

    // Paint: predicted char lands at the cursor, cursor advances.
    assert!(context.predict_editor_insert_char('x'));
    {
        let terminal = context.terminal.lock();
        assert_eq!(terminal.grid[Line(0)][Column(0)].c(), 'x');
        assert_eq!(terminal.grid.cursor.pos.col.0, 1);
    }
    assert_eq!(context.editor_predicted_cells.len(), 1);

    // Confirm: an authoritative repaint of the row supersedes it.
    context.enqueue_daemon_editor_message(EditorServerMessage::GridUpdate {
        surface_id: Some("44".into()),
        grid_id: 1,
        width: 20,
        height: 5,
        cells: vec![GridCell {
            row: 0,
            col: 0,
            ch: "x".into(),
            fg: 0x00ff_ffff,
            bg: 0x0000_0000,
            attrs: 0,
        }],
        cursor: None,
        mode: None,
    });
    let (applied, _) = context.pump_editor_redraws();
    assert!(applied > 0);
    assert!(context.editor_predicted_cells.is_empty());
    assert_eq!(context.terminal.lock().grid[Line(0)][Column(0)].c(), 'x');

    // Expire: an unconfirmed prediction reverts to blank after TTL.
    assert!(context.predict_editor_insert_char('y'));
    assert_eq!(context.terminal.lock().grid[Line(0)][Column(1)].c(), 'y');
    context.editor_predicted_cells[0].at =
        std::time::Instant::now() - std::time::Duration::from_secs(2);
    assert!(context.expire_editor_predictions());
    assert!(context.editor_predicted_cells.is_empty());
    assert_eq!(context.terminal.lock().grid[Line(0)][Column(1)].c(), ' ');
}

#[test]
fn typing_prediction_refuses_non_blank_tail_and_wrong_mode() {
    use neoism_backend::performer::nvim_events::EditorMode;
    let mut dimension = ContextDimension::default();
    dimension.columns = 20;
    dimension.lines = 5;
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        45,
        0,
        dimension,
    );
    context.editor_grid_id = Some(1);

    // Normal mode: no prediction.
    assert!(!context.predict_editor_insert_char('x'));

    // Insert mode but text under the tail: no prediction (a real
    // mid-line insert shifts the tail; leave it to the round trip).
    context.editor_mode = EditorMode::Insert;
    context.enqueue_daemon_editor_message(EditorServerMessage::GridUpdate {
        surface_id: Some("45".into()),
        grid_id: 1,
        width: 20,
        height: 5,
        cells: vec![GridCell {
            row: 0,
            col: 3,
            ch: "z".into(),
            fg: 0x00ff_ffff,
            bg: 0x0000_0000,
            attrs: 0,
        }],
        cursor: None,
        mode: None,
    });
    let _ = context.pump_editor_redraws();
    assert!(!context.predict_editor_insert_char('x'));
}

#[test]
fn editor_grid_selection_recovers_when_viewport_points_at_sparse_grid() {
    let mut dimension = ContextDimension::default();
    dimension.columns = 10;
    dimension.lines = 5;
    let mut context = create_dead_context(
        VoidListener,
        unsafe { neoism_backend::event::WindowId::dummy() },
        43,
        0,
        dimension,
    );
    context.editor_grid_id = Some(2);

    let mut real_editor_cells = Vec::new();
    for row in 0..3 {
        for col in 0..10 {
            real_editor_cells.push(GridCell {
                row,
                col,
                ch: if row == 0 && col == 0 { "A" } else { "." }.into(),
                fg: 0x00ff_ffff,
                bg: 0x0000_0000,
                attrs: 0,
            });
        }
    }

    context.enqueue_daemon_editor_message(EditorServerMessage::Batch {
        surface_id: Some("43".into()),
        messages: vec![
            EditorServerMessage::WinViewport {
                surface_id: Some("43".into()),
                grid_id: 2,
                topline: 0,
                botline: 5,
                line_count: 100,
                scroll_delta: 0.0,
                curline: 0,
                curcol: 0,
                textoff: 0,
            },
            EditorServerMessage::GridUpdate {
                surface_id: Some("43".into()),
                grid_id: 2,
                width: 10,
                height: 5,
                cells: vec![GridCell {
                    row: 0,
                    col: 9,
                    ch: "x".into(),
                    fg: 0x00ff_ffff,
                    bg: 0x0000_0000,
                    attrs: 0,
                }],
                cursor: None,
                mode: None,
            },
            EditorServerMessage::GridUpdate {
                surface_id: Some("43".into()),
                grid_id: 1,
                width: 10,
                height: 5,
                cells: real_editor_cells,
                cursor: None,
                mode: None,
            },
        ],
    });

    let (applied, limited) = context.pump_editor_redraws();
    assert!(applied > 0);
    assert!(!limited);
    assert_eq!(context.editor_grid_id, Some(1));
    let terminal = context.terminal.lock();
    assert_eq!(terminal.grid[Line(0)][Column(0)].c(), 'A');
    assert_eq!(terminal.grid[Line(0)][Column(9)].c(), '.');
}
