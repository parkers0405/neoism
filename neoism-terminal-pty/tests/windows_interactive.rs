//! Native Windows interactive transport regressions, not one-shot `/C` spawns.
//! Run on Windows with:
//! `cargo test -p neoism-terminal-pty --test windows_interactive -- --ignored`
#![cfg(windows)]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::time::{Duration, Instant};

fn wait_for(session: &mut PtySession, expected: &[u8]) -> Vec<u8> {
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut output = Vec::new();
    let mut buf = [0; 8192];
    let mut answered_queries = 0;
    while Instant::now() < deadline {
        match session.read(&mut buf) {
            Ok(n) => output.extend_from_slice(&buf[..n]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!(
                "PTY read failed: {error}; output: {:?}",
                String::from_utf8_lossy(&output)
            ),
        }
        // ConPTY/PSReadLine can query the host cursor before accepting input.
        // Count across chunk boundaries, replying once to each query.
        let queries = output
            .windows(4)
            .filter(|bytes| *bytes == b"\x1b[6n")
            .count();
        while answered_queries < queries {
            assert_eq!(session.write(b"\x1b[1;1R").unwrap(), 6);
            answered_queries += 1;
        }
        if output
            .windows(expected.len())
            .any(|bytes| bytes == expected)
        {
            return output;
        }
        assert!(
            session.exit_code().is_none(),
            "shell exited before expected output: {:?}",
            String::from_utf8_lossy(&output)
        );
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {:?}; output: {:?}",
        String::from_utf8_lossy(expected),
        String::from_utf8_lossy(&output)
    );
}

fn interactive_roundtrip(shell: &str, args: &[&str], powershell: bool) {
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some(shell.into()),
        args: args.iter().map(|arg| (*arg).into()).collect(),
        ..PtySessionConfig::default()
    })
    .expect("spawn interactive ConPTY");

    for index in 0..32 {
        // Let the pipe worker return to its empty-buffer wait between submits.
        std::thread::sleep(Duration::from_millis(30));
        if index == 8 || index == 16 || index == 24 {
            let clear: &[u8] = if powershell {
                b"Clear-Host\r"
            } else {
                b"cls\r"
            };
            assert_eq!(session.write(clear).unwrap(), clear.len());
        }
        // The expected marker is not present in the submitted source, so local
        // echo alone cannot satisfy this assertion.
        let command = if powershell {
            format!("Write-Output ('neoism-' + (7100 + {index}) + '-done')\r")
        } else {
            format!(
                "set /a _neoism_probe=7100+{index} & echo neoism-!_neoism_probe!-done\r"
            )
        };
        assert_eq!(session.write(command.as_bytes()).unwrap(), command.len());
        let expected = format!("neoism-{}-done", 7100 + index);
        wait_for(&mut session, expected.as_bytes());
    }
    session.close();
}

fn integrated_powershell_lifecycle(shell: &str) {
    let args = neoism_terminal_pty::shell_integration::powershell_args(
        shell,
        &["-NoLogo".into(), "-NoProfile".into()],
    )
    .unwrap();
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some(shell.into()),
        args,
        ..PtySessionConfig::default()
    })
    .expect("spawn integrated PowerShell");
    wait_for(&mut session, b"\x1b]133;B\x07");

    for command in ["clear", "cls", "Clear-Host"] {
        let clear = format!("{command}\r");
        assert_eq!(session.write(clear.as_bytes()).unwrap(), clear.len());
        wait_for(&mut session, b"\x1b]133;B\x07");

        let started = Instant::now();
        let command = b"Write-Output ('neoism-' + (7100 + 99) + '-done'); Start-Sleep -Milliseconds 250\r";
        assert_eq!(session.write(command).unwrap(), command.len());
        let output = wait_for(&mut session, b"\x1b]133;B\x07");
        for marker in [b"neoism-7199-done".as_slice(), b"\x1b]133;D;0\x07"] {
            assert!(
                output.windows(marker.len()).any(|bytes| bytes == marker),
                "missing {:?} in {:?}",
                String::from_utf8_lossy(marker),
                String::from_utf8_lossy(&output)
            );
        }
        assert!(
            started.elapsed() >= Duration::from_millis(200),
            "completion preceded command exit"
        );
    }
    let command = b"Write-Error 'intentional lifecycle regression probe'\r";
    assert_eq!(session.write(command).unwrap(), command.len());
    let output = wait_for(&mut session, b"\x1b]133;B\x07");
    let failed = b"\x1b]133;D;1\x07";
    assert!(
        output.windows(failed.len()).any(|bytes| bytes == failed),
        "failed command lost exit status: {:?}",
        String::from_utf8_lossy(&output)
    );
    session.close();
}

#[test]
#[ignore = "requires native Windows ConPTY and Windows PowerShell"]
fn powershell_integration_completes_clear_sleep_and_errors() {
    integrated_powershell_lifecycle("powershell.exe");
}

#[test]
#[ignore = "requires native Windows ConPTY and PowerShell 7 installed"]
fn pwsh_integration_completes_clear_sleep_and_errors() {
    integrated_powershell_lifecycle("pwsh.exe");
}

#[test]
#[ignore = "requires native Windows ConPTY and cmd.exe"]
fn cmd_accepts_idle_separated_commands_and_clear() {
    interactive_roundtrip("cmd.exe", &["/D", "/Q", "/V:ON"], false);
}

#[test]
#[ignore = "requires native Windows ConPTY and Windows PowerShell"]
fn powershell_accepts_idle_separated_commands_and_clear() {
    interactive_roundtrip("powershell.exe", &["-NoLogo", "-NoProfile"], true);
}

#[test]
#[ignore = "requires native Windows ConPTY and PowerShell 7 installed"]
fn pwsh_accepts_idle_separated_commands_and_clear() {
    interactive_roundtrip("pwsh.exe", &["-NoLogo", "-NoProfile"], true);
}
