use neoism_protocol::pty::{ClientMessage, ServerMessage};
use serde_json::json;

#[test]
fn completion_wire_contract_is_utf8_and_full_line() {
    let request = json!({"CompleteTerminal":{"session_id":"host","line":"cat é","cursor":6}});
    let message: ClientMessage = serde_json::from_value(request.clone()).unwrap();
    assert_eq!(serde_json::to_value(message).unwrap(), request);
    let reply = json!({"TerminalCompletions": {"session_id":"host","line":"cat é","cursor":6,
        "entries":[{"value":"cat 'é space' --suffix","label":"é space","kind":"file"}],"provider":"bash-live-snapshot+native+host"}});
    let message: ServerMessage = serde_json::from_value(reply).unwrap();
    match message {
        ServerMessage::TerminalCompletions { entries, cursor, .. } => {
            assert_eq!(cursor, 6);
            assert_eq!(entries[0].detail, "");
            assert_eq!(entries[0].value, "cat 'é space' --suffix");
        }
        _ => panic!("wrong reply"),
    }
}