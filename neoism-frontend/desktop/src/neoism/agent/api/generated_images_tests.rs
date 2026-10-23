use super::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};

fn request(socket: &mut TcpStream) -> String {
    let mut bytes = [0; 4096];
    let read = socket.read(&mut bytes).unwrap();
    String::from_utf8_lossy(&bytes[..read]).into_owned()
}
fn respond(socket: &mut TcpStream, body: &str) {
    write!(
        socket,
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .unwrap();
}

#[test]
fn slow_artifact_does_not_delay_history_and_duplicates_share_download() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let (started_tx, started_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let responder = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).starts_with("GET /v2/sessions/history/messages?"));
        respond(&mut socket, &serde_json::json!({"items": [{
            "info": {"id": "msg", "role": "assistant"},
            "parts": [
                {"id": "text", "type": "text", "text": "Visible immediately"},
                {"id": "image", "type": "file", "mime": "image/png", "filename": "image.png", "url": "/v2/artifacts/slow/content"}
            ]
        }]}).to_string());
        let (mut socket, _) = listener.accept().unwrap();
        assert!(request(&mut socket).starts_with("GET /v2/artifacts/slow/content "));
        started_tx.send(()).unwrap();
        // A regression that hydrates during fetch cannot release this gate.
        release_rx.recv_timeout(Duration::from_secs(3)).unwrap();
        respond(&mut socket, "image");
        listener.set_nonblocking(true).unwrap();
        std::thread::sleep(Duration::from_millis(100));
        assert!(listener.accept().is_err(), "duplicate artifact download");
    });
    let started = Instant::now();
    let page =
        super::super::fetch_session_messages_page(&server, "history", None, 32).unwrap();
    assert!(started.elapsed() < Duration::from_secs(2));
    assert!(page
        .blocks
        .iter()
        .any(|message| message.text.contains("Visible immediately")));
    assert!(page
        .blocks
        .iter()
        .flat_map(|message| &message.images)
        .any(|image| image.url.as_str() == "/v2/artifacts/slow/content"));
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    assert!(hydrate_page(
        &server,
        &page.blocks,
        move |patch| {
            tx.send(patch).unwrap();
        },
        || {}
    ));
    started_rx.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(hydrate_page(
        &server,
        &page.blocks,
        move |patch| {
            tx2.send(patch).unwrap();
        },
        || {}
    ));
    assert!(
        rx.try_recv().is_err(),
        "image unexpectedly bypassed the slow gate"
    );
    release_tx.send(()).unwrap();
    let patch = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    let duplicate = rx.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(patch.url, duplicate.url);
    let mut blocks = page.blocks;
    // New text is never overwritten, and a missing image is never resurrected.
    for message in &mut blocks {
        message.text = "new SSE text".into();
    }
    assert!(patch.apply(&mut blocks));
    assert!(blocks.iter().all(|message| message.text == "new SSE text"));
    assert!(
        !patch.apply(&mut blocks),
        "must match the original URL exactly"
    );
    assert!(!patch.apply(&mut []), "must not resurrect deleted messages");
    responder.join().unwrap();
}

#[test]
fn stale_generation_and_changed_image_cannot_be_patched() {
    let mut scope = scope("http://127.0.0.1:1").unwrap();
    scope.generation = scope.generation.wrapping_sub(1);
    let patch = ImagePatch {
        message_id: "image".into(),
        original_url: "/v2/artifacts/a/content".into(),
        mime: "image/png".into(),
        url: "data:image/png;base64,AA==".into(),
        scope,
        _reservation: Arc::new(Reservation(0)),
    };
    let mut message = NeoismAgentMessage::assistant("new text").with_id("image");
    message
        .images
        .push(neoism_ui::panels::agent_pane::state::NeoismAgentImage {
            filename: "a.png".into(),
            mime: "image/png".into(),
            url: "/v2/artifacts/a/content".into(),
        });
    assert!(!patch.apply(std::slice::from_mut(&mut message)));
    assert_eq!(message.images[0].url.as_str(), "/v2/artifacts/a/content");
    assert!(Reservation::acquire(MAX_RETAINED_BYTES + 1).is_none());
    for url in [
        "https://elsewhere/v2/artifacts/a/content",
        "/v2/artifacts/../secret/content",
        "/v2/artifacts/%2e%2e/content",
    ] {
        message.images[0].url = url.into();
        assert!(!artifact(&message.images[0]));
    }
}

#[test]
fn artifact_request_uses_captured_credential_not_live_registry() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let responder = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        let request = request(&mut socket).to_ascii_lowercase();
        assert!(request.contains("authorization: bearer captured-token\r\n"));
        respond(&mut socket, "image");
    });
    // The registry has no credential for this endpoint. The work item must not
    // resolve it afresh when it finally reaches a worker.
    let key = Key {
        scope: Scope {
            server,
            credential: Some("captured-token".into()),
            generation: 0,
        },
        path: "/v2/artifacts/a/content".into(),
        mime: "image/png".into(),
    };
    assert!(download(&key).is_some());
    responder.join().unwrap();
}

#[test]
fn oversized_content_length_is_rejected_without_reading_body() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server = format!("http://{}", listener.local_addr().unwrap());
    let responder = std::thread::spawn(move || {
        let (mut socket, _) = listener.accept().unwrap();
        request(&mut socket);
        // No body follows: the reader must reject the header, not wait for EOF.
        write!(
            socket,
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n",
            33 * 1024 * 1024
        )
        .unwrap();
        std::thread::sleep(Duration::from_millis(400));
    });
    let key = Key {
        scope: scope(&server).unwrap(),
        path: "/v2/artifacts/large/content".into(),
        mime: "image/png".into(),
    };
    let started = Instant::now();
    assert!(download(&key).is_none());
    assert!(started.elapsed() < Duration::from_secs(2));
    responder.join().unwrap();
}
