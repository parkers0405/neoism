//! Smoke test for `PtySession::spawn`.
//!
//! Spawns a trivial `echo hello` behind a fresh PTY (forkpty on unix,
//! ConPTY on Windows) and checks that `b"hello"` shows up in the read
//! stream within the deadline.
//!
//! The tests are `#[ignore]` because:
//!   * They depend on the platform shell (`/bin/sh` + `printf`, or
//!     `cmd.exe`) being present and behaving in the test environment.
//!   * `teletypewriter` writes via spawn, so on some sandboxes the
//!     fork can race with the master fd setup and produce 0 bytes
//!     before the child writes. The tests loop with a wall-clock cap
//!     to stay deterministic-ish, but are still timing-sensitive on
//!     loaded CI runners.
//! Run with `cargo test -p neoism-terminal-pty -- --ignored`.

#![cfg(any(unix, windows))]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::time::{Duration, Instant};

fn read_until_hello(mut session: PtySession, deadline: Duration) -> Vec<u8> {
    let deadline = Instant::now() + deadline;
    let mut got = Vec::<u8>::new();
    let mut buf = [0u8; 256];
    while Instant::now() < deadline {
        match session.read(&mut buf) {
            Ok(0) => {
                if session.exit_code().is_some() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(n) => got.extend_from_slice(&buf[..n]),
            Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(err) => panic!("read failed: {err}"),
        }
        if got.windows(5).any(|w| w == b"hello") {
            break;
        }
    }
    session.close();
    got
}

#[cfg(unix)]
#[test]
#[ignore]
fn spawn_echo_emits_hello() {
    let config = PtySessionConfig {
        shell: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), "printf hello".to_string()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };
    let session = PtySession::spawn(config).expect("spawn PTY");

    let got = read_until_hello(session, Duration::from_secs(1));
    assert!(
        got.windows(5).any(|w| w == b"hello"),
        "expected to see `hello` in PTY output, got: {got:?}"
    );
}

/// Windows leg: same smoke through ConPTY. `cmd.exe` cold-starts much
/// slower than `/bin/sh` (console host + conhost handshake), hence the
/// generous deadline.
#[cfg(windows)]
#[test]
#[ignore]
fn spawn_echo_emits_hello_conpty() {
    let config = PtySessionConfig {
        shell: Some("cmd.exe".to_string()),
        args: vec!["/C".to_string(), "echo hello".to_string()],
        cwd: None,
        env: Vec::new(),
        cols: 80,
        rows: 24,
    };
    let session = PtySession::spawn(config).expect("spawn ConPTY");

    let got = read_until_hello(session, Duration::from_secs(15));
    assert!(
        got.windows(5).any(|w| w == b"hello"),
        "expected to see `hello` in ConPTY output, got: {got:?}"
    );
}
