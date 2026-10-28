//! End-to-end smoke test for the real-PTY-backed SessionRegistry.
//!
//! Spawns `sh -c 'printf hello'` through the registry, asserts the
//! resulting PtyOutput contains the expected bytes, then closes.
//!
//! Gated `#[ignore]` because spawning a PTY requires Unix and tends to
//! be flaky in CI; run with `cargo test -- --ignored` to exercise.

use std::time::Duration;

use neoism_protocol::pty::{ClientMessage, ServerMessage};
use neoism_workspace_daemon::sessions::SessionRegistry;

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn create_input_output_close_roundtrip() {
    let (registry, mut output_rx) = SessionRegistry::new();

    // Override the env so PtySession picks `/bin/sh`. The registry
    // reads $SHELL itself; force it to a known one.
    std::env::set_var("SHELL", "/bin/sh");

    let created = registry.handle(ClientMessage::CreatePty {
        presentation: None,
        cwd: None,
        cols: 80,
        rows: 24,
        shell: None,
    });
    let session_id = match created.first().expect("PtyCreated reply") {
        ServerMessage::PtyCreated { session_id, .. } => session_id.clone(),
        other => panic!("expected PtyCreated, got {other:?}"),
    };

    // Drive the shell to print exactly "hello\n" and then exit, so the
    // reader task sees EOF and emits PtyClosed.
    registry.handle(ClientMessage::PtyInput {
        session_id: session_id.clone(),
        bytes: b"printf hello && exit\n".to_vec(),
    });

    let mut accumulated: Vec<u8> = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    let mut saw_close = false;
    while tokio::time::Instant::now() < deadline {
        let next =
            tokio::time::timeout(Duration::from_millis(200), output_rx.recv()).await;
        match next {
            Ok(Ok(ServerMessage::PtyOutput { bytes, .. })) => {
                accumulated.extend_from_slice(&bytes);
                if accumulated.windows(5).any(|w| w == b"hello") && saw_close {
                    break;
                }
            }
            Ok(Ok(ServerMessage::PtyClosed { .. })) => {
                saw_close = true;
                if accumulated.windows(5).any(|w| w == b"hello") {
                    break;
                }
            }
            Ok(Ok(_))
            | Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_)))
            | Err(_) => {}
            Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => break,
        }
    }

    assert!(
        accumulated.windows(5).any(|w| w == b"hello"),
        "expected 'hello' in stdout, got: {:?}",
        String::from_utf8_lossy(&accumulated)
    );
}
