//! Runs only the isolated Windows updater tests (all installation/process effects mocked).
#![cfg(windows)]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires Windows ConPTY and PowerShell; never installs or launches Neoism"]
fn windows_updater_policy_and_transaction_tests() {
    let shell =
        std::env::var("NEOISM_TEST_PWSH").unwrap_or_else(|_| "powershell.exe".into());
    let script = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../scripts/windows-updater-tests.ps1")
        .canonicalize()
        .expect("locate updater tests");
    let path = script.to_string_lossy();
    let path = path.trim_start_matches(r"\\?\").replace('\'', "''");
    let command = format!(
        "try {{ & '{path}' }} catch {{ Write-Output ($_ | Out-String); Write-Output ('neoism-updater-' + 'test-failed'); exit 1 }}"
    );
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some(shell),
        args: vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-NonInteractive".into(),
            "-ExecutionPolicy".into(),
            "Bypass".into(),
            "-Command".into(),
            command,
        ],
        ..PtySessionConfig::default()
    })
    .expect("spawn PowerShell updater tests");
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut answered_queries = 0;
    let marker = b"neoism-windows-updater-tests-passed";
    while Instant::now() < deadline {
        match session.read(&mut buffer) {
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("updater test output failed: {error}"),
        }
        let queries = output
            .windows(4)
            .filter(|bytes| *bytes == b"\x1b[6n")
            .count();
        while answered_queries < queries {
            session.write(b"\x1b[1;1R").unwrap();
            answered_queries += 1;
        }
        if output.windows(marker.len()).any(|bytes| bytes == marker) {
            session.close();
            return;
        }
        let failed = b"neoism-updater-test-failed";
        if output.windows(failed.len()).any(|bytes| bytes == failed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    session.close();
    panic!(
        "Updater tests did not pass: {}",
        String::from_utf8_lossy(&output)
    );
}
