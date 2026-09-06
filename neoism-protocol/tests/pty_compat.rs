//! Backward-compatibility tests for PTY messages shared by old and new clients.

use neoism_protocol::pty::{ClientMessage, ServerMessage};

#[test]
fn legacy_create_pty_without_shell_still_decodes() {
    let message: ClientMessage =
        serde_json::from_str(r#"{"CreatePty":{"cwd":"/workspace","cols":80,"rows":24}}"#)
            .expect("decode legacy CreatePty");

    match message {
        ClientMessage::CreatePty {
            cwd,
            cols,
            rows,
            shell,
        } => {
            assert_eq!(cwd.as_deref(), Some("/workspace"));
            assert_eq!((cols, rows), (80, 24));
            assert_eq!(shell, None);
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn absent_optional_pty_fields_stay_off_the_wire() {
    let create = serde_json::to_string(&ClientMessage::CreatePty {
        cwd: None,
        cols: 120,
        rows: 40,
        shell: None,
    })
    .expect("serialize CreatePty");
    assert!(
        !create.contains("shell"),
        "None shell leaked onto wire: {create}"
    );

    let created = serde_json::to_string(&ServerMessage::PtyCreated {
        shell: None,
        session_id: "session-1".into(),
        workspace_root: None,
    })
    .expect("serialize PtyCreated");
    assert!(
        !created.contains("workspace_root"),
        "None workspace root leaked onto wire: {created}"
    );
}

#[test]
fn legacy_pty_created_without_workspace_root_still_decodes() {
    let message: ServerMessage =
        serde_json::from_str(r#"{"PtyCreated":{"session_id":"session-1"}}"#)
            .expect("decode legacy PtyCreated");

    match message {
        ServerMessage::PtyCreated {
            shell: None,
            session_id,
            workspace_root,
        } => {
            assert_eq!(session_id, "session-1");
            assert_eq!(workspace_root, None);
        }
        other => panic!("unexpected message: {other:?}"),
    }
}

#[test]
fn attach_and_session_cwd_keep_their_wire_shape() {
    let attach = serde_json::to_string(&ClientMessage::AttachPty {
        session_id: "session-2".into(),
    })
    .expect("serialize AttachPty");
    assert_eq!(attach, r#"{"AttachPty":{"session_id":"session-2"}}"#);

    let cwd = serde_json::to_string(&ServerMessage::SessionCwd {
        session_id: "session-2".into(),
        cwd: "/workspace/neoism".into(),
    })
    .expect("serialize SessionCwd");
    assert_eq!(
        cwd,
        r#"{"SessionCwd":{"session_id":"session-2","cwd":"/workspace/neoism"}}"#
    );
}

#[test]
fn actual_windows_shell_survives_wire_roundtrip() {
    let message: ServerMessage = serde_json::from_str(
        r#"{"PtyCreated":{"session_id":"s","shell":"C:\\Program Files\\PowerShell\\7\\pwsh.exe"}}"#,
    ).unwrap();
    let ServerMessage::PtyCreated {
        shell,
        workspace_root,
        ..
    } = message
    else {
        panic!()
    };
    assert_eq!(
        shell.as_deref(),
        Some(r"C:\Program Files\PowerShell\7\pwsh.exe")
    );
    assert!(workspace_root.is_none());
}
