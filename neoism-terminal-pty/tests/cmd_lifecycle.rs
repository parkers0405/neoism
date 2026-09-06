//! Real interactive cmd PROMPT lifecycle regression. Run via cargo xwin + Wine
//! or on Windows: `cargo test -p neoism-terminal-pty --test cmd_lifecycle -- --ignored`.
#![cfg(windows)]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::io::Write;
use std::time::{Duration, Instant};

const D: &[u8] = b"\x1b]133;D\x1b\\";
const A: &[u8] = b"\x1b]133;A\x1b\\";
const B: &[u8] = b"\x1b]133;B\x1b\\";

fn contains(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|window| window == needle)
}

fn prompt(
    session: &mut PtySession,
    computed: Option<&[u8]>,
    minimum: Duration,
) -> Vec<u8> {
    let started = Instant::now();
    let mut output = Vec::new();
    let mut queries_answered = 0;
    let mut buf = [0; 8192];
    while started.elapsed() < Duration::from_secs(20) {
        match session.read(&mut buf) {
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("read: {error}; {:?}", String::from_utf8_lossy(&output)),
        }
        assert!(output.len() < 1024 * 1024, "runaway shell output");
        let queries = output.windows(4).filter(|w| *w == b"\x1b[6n").count();
        while queries_answered < queries {
            session.write_reply(b"\x1b[1;1R").unwrap();
            queries_answered += 1;
        }
        if contains(&output, D) {
            assert!(
                started.elapsed() >= minimum,
                "premature D while child is sleeping: {:?}",
                String::from_utf8_lossy(&output)
            );
            if let Some(expected) = computed {
                assert!(
                    contains(&output, expected),
                    "D before computed child result: {:?}",
                    String::from_utf8_lossy(&output)
                );
            }
        }
        // cmd's line editor can redraw the old prompt tail (B) while echoing
        // a wrapped command. Only a new D can complete this submission.
        let complete_prompt = output.windows(D.len()).rposition(|bytes| bytes == D)
            .is_some_and(|start| contains(&output[start..], A) && contains(&output[start..], B));
        if complete_prompt {
            assert!(
                contains(&output, D),
                "missing D in {:?}",
                String::from_utf8_lossy(&output)
            );
            assert!(
                contains(&output, A),
                "missing A in {:?}",
                String::from_utf8_lossy(&output)
            );
            assert!(
                !contains(&output, b"\x1b]133;D;"),
                "cmd fabricated an exit status"
            );
            return output;
        }
        assert!(
            session.exit_code().is_none(),
            "cmd exited early: {:?}",
            String::from_utf8_lossy(&output)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("prompt timeout: {:?}", String::from_utf8_lossy(&output));
}

// Spawned by cmd as a foreground process, independent of ping/timeout behavior
// under Wine. Its computed markers never occur in the command submitted to cmd.
#[test]
#[ignore]
fn cmd_delay_child() {
    for row in 0..300 {
        println!("cmd-stream-{}", 8100 + row);
    }
    std::io::stdout().flush().unwrap();
    std::thread::sleep(Duration::from_millis(800));
    println!("cmd-complete-{}", 9100 + 55);
    std::io::stdout().flush().unwrap();
}

#[test]
#[ignore]
fn interactive_cmd_prompt_lifecycle_survives_output_delay_and_cls() {
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some("cmd.exe".into()),
        // /D prevents machine-specific AutoRun from replacing the prompt.
        // Production doesn't add /D, preserving the user's startup behavior.
        args: vec!["/D".into()],
        env: vec![("Prompt".into(), "neoism-custom $P$G".into())],
        cwd: Some(std::path::PathBuf::from(r"C:\")),
        ..PtySessionConfig::default()
    })
    .expect("spawn integrated cmd");
    let initial = prompt(&mut session, None, Duration::ZERO);
    assert!(
        contains(&initial, b"neoism-custom "),
        "original prompt lost"
    );

    // A short command must not complete from a cached prompt repaint either.
    session.write(b"set /a 1200+37\r").unwrap();
    prompt(&mut session, Some(b"1237"), Duration::ZERO);

    let executable = std::env::current_exe().unwrap();
    let command = format!(
        "\"{}\" --ignored --exact cmd_delay_child --nocapture\r",
        executable.display()
    );
    assert!(!command.contains("cmd-complete-9155"));
    for _ in 0..3 {
        assert_eq!(session.write(command.as_bytes()).unwrap(), command.len());
        let output = prompt(
            &mut session,
            Some(b"cmd-complete-9155"),
            Duration::from_millis(700),
        );
        assert!(
            contains(&output, b"cmd-stream-8399"),
            "long output was truncated"
        );
        assert!(contains(&output, b"neoism-custom "));
        assert_eq!(session.write(b"cls\r").unwrap(), 4);
        let cleared = prompt(&mut session, None, Duration::ZERO);
        assert!(contains(&cleared, b"neoism-custom "));
    }
    session.close();
}
