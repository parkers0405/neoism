use neoism_protocol::pty::{ClientMessage, ServerMessage};
use serde_json::json;

#[test]
fn directory_control_exact_wire_contract() {
    let request = json!({"ChangeTerminalDirectory": {
        "session_id": "host-pty", "directory": "/host/a 'quoted' directory", "request_id": "generation-b"
    }});
    let parsed: ClientMessage = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(parsed).unwrap(), request);
    for reply in [
        json!({"TerminalDirectoryChanged": {"session_id":"host-pty", "request_id":"generation-b", "directory":"/host/actual"}}),
        json!({"TerminalDirectoryChangeFailed": {"session_id":"host-pty", "request_id":"generation-a", "message":"unsafe shell control"}}),
    ] {
        let parsed: ServerMessage = serde_json::from_value(reply.clone()).unwrap();
        assert_eq!(serde_json::to_value(parsed).unwrap(), reply);
    }
    // Durable tab respawn policy remains a different protocol plane.
    assert!(serde_json::from_value::<ClientMessage>(
        json!({"SetCwd":{"session_id":"host-pty", "cwd":"/host/actual"}})
    )
    .is_err());
}
