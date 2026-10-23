//! Undo updates use the accepted revert boundary, not a second history/image fetch.
use super::*;
use std::io::{Read, Write};
use std::net::TcpListener;

#[test]
fn undo_loaded_turn_completes_with_only_the_mutation_request() {
    for marker in [Some("user-b"), None] {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let server = format!("http://{}", listener.local_addr().unwrap());
        super::super::api::register_agent_server_credential(&server, Some("undo-test"));
        let responder = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket.set_read_timeout(Some(Duration::from_secs(2))).unwrap();
            let mut request = Vec::new();
            let mut chunk = [0; 1024];
            while !request.windows(4).any(|bytes| bytes == b"\r\n\r\n") {
                let n = socket.read(&mut chunk).unwrap();
                assert!(n > 0);
                request.extend_from_slice(&chunk[..n]);
            }
            assert!(String::from_utf8_lossy(&request)
                .starts_with("POST /v2/sessions/root/undo "));
            let body = match marker {
                Some(id) => json!({"id":"root", "revert":{"messageID":id}}),
                None => json!({"id":"root"}),
            }.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            // No server remains to answer a history or image fetch. Waiting for
            // either would turn this successful undo into a failed update.
        });
        let mut pane = NeoismAgentPane::default();
        pane.server = server.clone();
        pane.session_id = Some("root".into());
        pane.messages = vec![
            NeoismAgentMessage::user("first").with_id("user-a"),
            NeoismAgentMessage::assistant("first reply").with_id("part-a"),
            NeoismAgentMessage::user("second").with_id("user-b"),
            NeoismAgentMessage::assistant("second reply").with_id("part-b"),
        ];
        pane.execute_undo_session_command();
        assert!(pane.session_history_in_flight.contains("root"));
        let update = pane.background_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        match update {
            NeoismAgentBackgroundUpdate::SessionHistoryApplied {
                messages,
                revert_message_id,
                ..
            } => {
                assert!(messages.is_none());
                assert_eq!(revert_message_id.as_deref(), marker);
            }
            _ => panic!("undo must complete without a history re-fetch"),
        }
        responder.join().unwrap();
        super::super::api::register_agent_server_credential(&server, None);
    }
}

#[test]
fn completed_undo_updates_cached_conversation_after_switching_away() {
    let mut pane = NeoismAgentPane::default();
    let mut cached = CachedAgentSession::live_only();
    cached.messages = vec![
        NeoismAgentMessage::user("first").with_id("user-a"),
        NeoismAgentMessage::assistant("first reply").with_id("part-a"),
        NeoismAgentMessage::user("second").with_id("user-b"),
        NeoismAgentMessage::assistant("second reply").with_id("part-b"),
    ];
    pane.session_cache.insert("root".into(), cached);
    pane.session_history_in_flight.insert("root".into());
    pane.input = "draft in another conversation".into();
    pane.background_sender().send(NeoismAgentBackgroundUpdate::SessionHistoryApplied {
        session_id: "root".into(),
        title: "Undo".into(),
        messages: None,
        revert_message_id: Some("user-b".into()),
    }).unwrap();
    pane.drain_background_updates();
    assert_eq!(pane.session_cache["root"].messages.len(), 2);
    assert_eq!(pane.input, "draft in another conversation");
    assert!(!pane.session_history_in_flight.contains("root"));
}

#[test]
fn history_update_preserves_unsent_draft_and_prevents_overlapping_mutations() {
    let mut pane = NeoismAgentPane::default();
    pane.session_id = Some("root".into());
    pane.session_history_in_flight.insert("root".into());
    pane.input = "do not send while files are reverting".into();
    pane.cursor_byte = pane.input.len();
    assert!(pane.submit());
    assert_eq!(pane.input, "do not send while files are reverting");
    assert!(pane.pending_prompt_dispatches.is_empty());
    pane.execute_redo_session_command();
    assert!(pane.pending_outbound.is_empty());
    pane.background_sender().send(NeoismAgentBackgroundUpdate::SessionHistoryFailed {
        session_id: "root".into(),
        title: "Undo".into(),
        error: "Session is already running".into(),
    }).unwrap();
    pane.drain_background_updates();
    assert!(!pane.session_history_in_flight.contains("root"));
    assert_eq!(pane.input, "do not send while files are reverting");
}
