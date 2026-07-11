
use super::*;
use crate::context::factories::create_dead_context;
use crate::event::VoidListener;
use crate::layout::ContextDimension;
use neoism_protocol::editor::GridCell;
use neoism_terminal_core::crosswords::pos::{Column, Line};

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
