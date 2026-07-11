//! Roundtrip every editor-protocol variant through serde_json.
//!
//! Mirrors `tests/files.rs` / `tests/git.rs` — every variant gets a
//! ser → deser → re-ser pass to lock the wire shape against drift.

use std::path::PathBuf;

use neoism_protocol::editor::{
    DiagnosticItem, DiagnosticSeverity, EditorClientMessage, EditorServerMessage,
    GridCell, GridPos, HighlightAttrs, PopupMenuItem,
};

fn roundtrip_client(msg: &EditorClientMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: EditorClientMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

fn roundtrip_server(msg: &EditorServerMessage) {
    let json = serde_json::to_string(msg).expect("serialize");
    let back: EditorServerMessage = serde_json::from_str(&json).expect("deserialize");
    let json_back = serde_json::to_string(&back).expect("re-serialize");
    assert_eq!(json, json_back, "roundtrip mismatch: {json}");
}

#[test]
fn editor_surface_id_is_backward_compatible_for_old_client_json() {
    let msg: EditorClientMessage =
        serde_json::from_str(r#"{"OpenBuffer":{"path":"src/lib.rs"}}"#)
            .expect("old OpenBuffer without surface_id still decodes");
    match msg {
        EditorClientMessage::OpenBuffer {
            path, surface_id, ..
        } => {
            assert_eq!(path, PathBuf::from("src/lib.rs"));
            assert_eq!(surface_id, None);
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn editor_surface_id_is_backward_compatible_for_old_server_json() {
    let msg: EditorServerMessage =
        serde_json::from_str(r#"{"GridResize":{"grid_id":1,"width":80,"height":24}}"#)
            .expect("old GridResize without surface_id still decodes");
    match msg {
        EditorServerMessage::GridResize {
            surface_id,
            grid_id,
            width,
            height,
        } => {
            assert_eq!(surface_id, None);
            assert_eq!(grid_id, 1);
            assert_eq!((width, height), (80, 24));
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn editor_surface_id_serializes_only_when_present() {
    let legacy = serde_json::to_string(&EditorServerMessage::GridResize {
        surface_id: None,
        grid_id: 1,
        width: 80,
        height: 24,
    })
    .expect("serialize");
    assert!(
        !legacy.contains("surface_id"),
        "None surface id should stay off the legacy wire: {legacy}"
    );

    let routed = serde_json::to_string(&EditorServerMessage::GridResize {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        width: 80,
        height: 24,
    })
    .expect("serialize");
    assert!(
        routed.contains(r#""surface_id":"pane:root""#),
        "Some surface id should be present: {routed}"
    );
}

#[test]
fn editor_client_surface_id_accessor_tracks_targeted_commands() {
    let msg = EditorClientMessage::SendKeys {
        bytes: b"i".to_vec(),
        surface_id: Some("pane:2".into()),
    };
    assert_eq!(msg.surface_id(), Some("pane:2"));
    assert_eq!(EditorClientMessage::Close.surface_id(), None);
}

#[test]
fn client_open_buffer_roundtrip() {
    roundtrip_client(&EditorClientMessage::OpenBuffer {
        path: PathBuf::from("src/lib.rs"),
        line: None,
        character: None,
        surface_id: None,
    });
    roundtrip_client(&EditorClientMessage::OpenBuffer {
        path: PathBuf::from(""),
        line: Some(2),
        character: Some(4),
        surface_id: Some("pane:root".into()),
    });
}

#[test]
fn client_send_keys_roundtrip() {
    roundtrip_client(&EditorClientMessage::SendKeys {
        bytes: b"i".to_vec(),
        surface_id: None,
    });
    roundtrip_client(&EditorClientMessage::SendKeys {
        bytes: b":wq\r".to_vec(),
        surface_id: Some("pane:1".into()),
    });
    roundtrip_client(&EditorClientMessage::SendKeys {
        bytes: Vec::new(),
        surface_id: None,
    });
}

#[test]
fn editor_client_accepts_spec_style_nvim_input_alias() {
    let msg: EditorClientMessage =
        serde_json::from_str(r#"{"NvimInput":{"bytes":[105],"surface_id":"pane:1"}}"#)
            .expect("NvimInput alias decodes");
    match msg {
        EditorClientMessage::SendKeys { bytes, surface_id } => {
            assert_eq!(bytes, b"i");
            assert_eq!(surface_id.as_deref(), Some("pane:1"));
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn client_mouse_input_roundtrip() {
    roundtrip_client(&EditorClientMessage::MouseInput {
        button: "wheel".into(),
        action: "down".into(),
        modifier: "S-".into(),
        grid: 0,
        row: 4,
        col: 12,
        count: 3,
        surface_id: Some("pane:mouse".into()),
    });
}

#[test]
fn client_resize_roundtrip() {
    roundtrip_client(&EditorClientMessage::Resize {
        width: 120,
        height: 40,
        surface_id: Some("pane:resize".into()),
    });
}

#[test]
fn client_close_roundtrip() {
    roundtrip_client(&EditorClientMessage::Close);
}

#[test]
fn server_grid_update_roundtrip() {
    roundtrip_server(&EditorServerMessage::GridUpdate {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        width: 80,
        height: 24,
        cells: vec![
            GridCell {
                row: 0,
                col: 0,
                ch: "h".into(),
                fg: 0x00FFFFFF,
                bg: 0x00000000,
                attrs: 0,
            },
            GridCell {
                row: 0,
                col: 1,
                ch: "i".into(),
                fg: 0x00FFFFFF,
                bg: 0x00000000,
                attrs: 0b0000_0001, // bold
            },
        ],
        cursor: Some(GridPos { row: 0, col: 2 }),
        mode: Some("normal".into()),
    });
    roundtrip_server(&EditorServerMessage::GridUpdate {
        surface_id: None,
        grid_id: 1,
        width: 0,
        height: 0,
        cells: Vec::new(),
        cursor: None,
        mode: None,
    });
}

#[test]
fn server_grid_resize_roundtrip() {
    roundtrip_server(&EditorServerMessage::GridResize {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        width: 100,
        height: 30,
    });
}

#[test]
fn server_grid_clear_roundtrip() {
    roundtrip_server(&EditorServerMessage::GridClear {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
    });
    roundtrip_server(&EditorServerMessage::GridClear {
        surface_id: None,
        grid_id: 3,
    });
}

#[test]
fn server_grid_scroll_roundtrip() {
    roundtrip_server(&EditorServerMessage::GridScroll {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        top: 2,
        bot: 22,
        left: 0,
        right: 80,
        rows: 1,
        cols: 0,
    });
    roundtrip_server(&EditorServerMessage::GridScroll {
        surface_id: None,
        grid_id: 3,
        top: 0,
        bot: 10,
        left: 4,
        right: 40,
        rows: -2,
        cols: 0,
    });
}

#[test]
fn server_win_viewport_roundtrip() {
    roundtrip_server(&EditorServerMessage::WinViewport {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        topline: 64,
        botline: 108,
        line_count: 220,
        scroll_delta: 12.0,
        curline: 80,
        curcol: 12,
        textoff: 6,
    });
}

#[test]
fn editor_server_cursor_highlight_mouse_and_message_roundtrip() {
    roundtrip_server(&EditorServerMessage::CursorGoto {
        surface_id: Some("pane:root".into()),
        grid_id: 1,
        row: 3,
        col: 9,
    });
    roundtrip_server(&EditorServerMessage::HighlightDefined {
        surface_id: Some("pane:root".into()),
        hl_id: 42,
        attrs: HighlightAttrs {
            fg: Some(0x00AA_BBCC),
            bg: Some(0x0001_0203),
            sp: None,
            bold: true,
            italic: false,
            underline: true,
            undercurl: false,
            strikethrough: false,
            reverse: false,
        },
    });
    roundtrip_server(&EditorServerMessage::MouseMode {
        surface_id: Some("pane:root".into()),
        enabled: true,
    });
    roundtrip_server(&EditorServerMessage::Message {
        surface_id: Some("pane:root".into()),
        kind: "lua_print".into(),
        content: "hi".into(),
        replace_last: false,
    });
}

#[test]
fn editor_server_accepts_spec_style_nvim_aliases() {
    let grid: EditorServerMessage = serde_json::from_str(
        r#"{"NvimGridLine":{"surface_id":"pane:root","grid_id":1,"width":80,"height":24,"cells":[],"cursor":null,"mode":null}}"#,
    )
    .expect("NvimGridLine alias decodes to canonical aggregate GridUpdate");
    assert!(matches!(grid, EditorServerMessage::GridUpdate { .. }));

    let clear: EditorServerMessage =
        serde_json::from_str(r#"{"Clear":{"surface_id":"pane:root","grid_id":1}}"#)
            .expect("Clear alias decodes to canonical GridClear");
    assert!(matches!(clear, EditorServerMessage::GridClear { .. }));

    let message: EditorServerMessage = serde_json::from_str(
        r#"{"NvimMessage":{"surface_id":"pane:root","kind":"lua_print","content":"hi","replace_last":false}}"#,
    )
    .expect("NvimMessage alias decodes");
    match message {
        EditorServerMessage::Message { kind, content, .. } => {
            assert_eq!(kind, "lua_print");
            assert_eq!(content, "hi");
        }
        other => panic!("unexpected variant: {other:?}"),
    }
}

#[test]
fn server_default_colors_roundtrip() {
    roundtrip_server(&EditorServerMessage::DefaultColors {
        surface_id: Some("pane:root".into()),
        rgb_fg: 0x00ABCDEF,
        rgb_bg: 0x00123456,
        rgb_sp: 0x00FFFFFF,
    });
}

#[test]
fn server_popup_menu_roundtrip() {
    roundtrip_server(&EditorServerMessage::PopupMenu {
        surface_id: Some("pane:popup".into()),
        items: vec![
            PopupMenuItem {
                word: "fn".into(),
                kind: "k".into(),
                menu: "Keyword".into(),
                info: "".into(),
            },
            PopupMenuItem {
                word: "format".into(),
                kind: "f".into(),
                menu: "Function".into(),
                info: "format the string".into(),
            },
        ],
        selected: Some(0),
        anchor: GridPos { row: 5, col: 12 },
        grid_id: 1,
    });
    roundtrip_server(&EditorServerMessage::PopupHide {
        surface_id: Some("pane:popup".into()),
    });
}

#[test]
fn server_diagnostics_roundtrip() {
    roundtrip_server(&EditorServerMessage::Diagnostics {
        surface_id: Some("pane:diag".into()),
        error: 1,
        warn: 1,
        info: 0,
        hint: 0,
        file_path: Some("src/main.rs".into()),
        items: vec![
            DiagnosticItem {
                severity: DiagnosticSeverity::Error,
                message: "unresolved identifier".into(),
                source: Some("rust-analyzer".into()),
                line: 12,
                col: 4,
                lnum: 13,
            },
            DiagnosticItem {
                severity: DiagnosticSeverity::Warn,
                message: "unused variable".into(),
                source: None,
                line: 0,
                col: 0,
                lnum: 1,
            },
        ],
    });
}

#[test]
fn server_mode_change_roundtrip() {
    roundtrip_server(&EditorServerMessage::ModeChange {
        surface_id: Some("pane:root".into()),
        mode: "insert".into(),
        mode_idx: 1,
    });
}

#[test]
fn server_buffer_opened_roundtrip() {
    roundtrip_server(&EditorServerMessage::BufferOpened {
        surface_id: Some("pane:root".into()),
        path: PathBuf::from("src/main.rs"),
        line_count: 1234,
    });
}

#[test]
fn server_buffer_modified_roundtrip() {
    roundtrip_server(&EditorServerMessage::BufferModified {
        surface_id: Some("pane:root".into()),
        path: PathBuf::from("src/main.rs"),
        modified: true,
    });
}

#[test]
fn server_closed_roundtrip() {
    roundtrip_server(&EditorServerMessage::Closed {
        surface_id: Some("pane:root".into()),
        reason: None,
    });
    roundtrip_server(&EditorServerMessage::Closed {
        surface_id: None,
        reason: Some("nvim exited".into()),
    });
}

#[test]
fn server_error_roundtrip() {
    roundtrip_server(&EditorServerMessage::Error {
        surface_id: Some("pane:root".into()),
        message: "spawn failed".into(),
    });
}

#[test]
fn severity_from_u8_codes() {
    assert_eq!(DiagnosticSeverity::from_u8(1), DiagnosticSeverity::Error);
    assert_eq!(DiagnosticSeverity::from_u8(2), DiagnosticSeverity::Warn);
    assert_eq!(DiagnosticSeverity::from_u8(3), DiagnosticSeverity::Info);
    assert_eq!(DiagnosticSeverity::from_u8(4), DiagnosticSeverity::Hint);
    // Fallback for unknown codes.
    assert_eq!(DiagnosticSeverity::from_u8(0), DiagnosticSeverity::Hint);
    assert_eq!(DiagnosticSeverity::from_u8(99), DiagnosticSeverity::Hint);
}
