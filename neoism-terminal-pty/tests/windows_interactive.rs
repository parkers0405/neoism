//! Native Windows interactive transport regressions, not one-shot `/C` spawns.
//! Run on Windows with:
//! `cargo test -p neoism-terminal-pty --test windows_interactive -- --ignored`
//! On an isolated CI account, set `NEOISM_TEST_POWERSHELL_DEFAULT_PROFILE=1`
//! to omit `-NoProfile` for integrated tests (loads existing profiles; writes none).
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
    if !powershell {
        // Expand on distinct command lines: cmd expands %vars% before set /p runs.
        let setup = b"set /a _neoism_ready=7100+99\r";
        assert_eq!(session.write(setup).unwrap(), setup.len());
        let read = b"set /p \"_neoism_input=neoism-%_neoism_ready%-ready:\"\r";
        assert_eq!(session.write(read).unwrap(), read.len());
        wait_for(&mut session, b"neoism-7199-ready:");
        let input = b"cmdforeground\r";
        assert_eq!(session.write(input).unwrap(), input.len());
        let reply = b"echo neoism-reply-%_neoism_input%-done\r";
        assert_eq!(session.write(reply).unwrap(), reply.len());
        wait_for(&mut session, b"neoism-reply-cmdforeground-done");
    }
    session.close();
}

struct ListingFixture {
    dir: std::path::PathBuf,
    filename: String,
}

impl ListingFixture {
    fn new() -> Self {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let filename = format!("ls-{unique:x}.txt");
        let dir = std::env::temp_dir()
            .join(format!("neoism-ls-{}-{unique:x}", std::process::id()));
        std::fs::create_dir(&dir).expect("create PowerShell listing fixture");
        std::fs::write(dir.join(&filename), b"actual directory listing probe")
            .expect("write PowerShell listing fixture");
        Self { dir, filename }
    }
}

impl Drop for ListingFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn assert_powershell_ls(session: &mut PtySession, filename: &str) {
    // Exact composer submission: the expected filename is never in the input,
    // so echo cannot satisfy this assertion. Require output before completion.
    assert_eq!(session.write(b"ls\r").unwrap(), 3);
    let done = b"\x1b]133;D;0\x1b\\";
    let output = wait_for(session, done);
    let completed_at = output.windows(done.len()).position(|b| b == done).unwrap();
    assert!(
        output[..completed_at]
            .windows(filename.len())
            .any(|b| b == filename.as_bytes()),
        "ls completed without the fixture filename {filename:?}: {:?}",
        String::from_utf8_lossy(&output)
    );
}

fn assert_powershell_foreground_input(session: &mut PtySession) {
    // Neither the ready prompt nor the computed reply occurs in source/echo.
    // Do not queue the token until Read-Host has actually printed its prompt.
    let command = b"$answer = Read-Host ('neoism-' + (7100 + 99) + '-ready'); Write-Output ('reply-' + $answer.ToUpperInvariant() + '-done')\r";
    assert_eq!(session.write(command).unwrap(), command.len());
    let ready = wait_for(session, b"neoism-7199-ready");
    assert!(
        !ready.windows(7).any(|b| b == b"\x1b]133;D"),
        "Read-Host completed before foreground input: {:?}",
        String::from_utf8_lossy(&ready)
    );
    let token = b"foreground7199\r";
    assert_eq!(session.write(token).unwrap(), token.len());
    let done = b"\x1b]133;D;0\x1b\\";
    let output = wait_for(session, done);
    let completed_at = output.windows(done.len()).position(|b| b == done).unwrap();
    let reply = b"reply-FOREGROUND7199-done";
    assert!(
        output[..completed_at]
            .windows(reply.len())
            .any(|b| b == reply),
        "Read-Host must return computed uppercase input before successful D: {:?}",
        String::from_utf8_lossy(&output)
    );
}

fn integrated_powershell_lifecycle(shell: &str, without_readline: bool) {
    let fixture = ListingFixture::new();
    // Opt in only on an isolated CI account to exercise production profile loading.
    // Never create, replace, or remove the user's real PowerShell profile.
    let mut base_args = vec!["-NoLogo".into()];
    if std::env::var_os("NEOISM_TEST_POWERSHELL_DEFAULT_PROFILE").is_none() {
        base_args.push("-NoProfile".into());
    }
    let mut args =
        neoism_terminal_pty::shell_integration::powershell_args(shell, &base_args)
            .unwrap();
    if without_readline {
        use base64::Engine;
        let encoded = args.last_mut().unwrap();
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&*encoded)
            .unwrap();
        let units: Vec<_> = bytes
            .chunks_exact(2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .collect();
        let script = format!(
            "Import-Module Microsoft.PowerShell.Utility, Microsoft.PowerShell.Management; $PSModuleAutoLoadingPreference = 'None'; Remove-Module PSReadLine -ErrorAction Ignore; Remove-Item Function:PSConsoleHostReadLine -ErrorAction Ignore\n{}\nif ($null -ne (Get-Command PSConsoleHostReadLine -ErrorAction Ignore)) {{ throw 'PSReadLine still loaded' }}; [Console]::Write('READLINE-ABSENT')",
            String::from_utf16(&units).unwrap(),
        );
        *encoded = base64::engine::general_purpose::STANDARD.encode(
            script
                .encode_utf16()
                .flat_map(u16::to_le_bytes)
                .collect::<Vec<_>>(),
        );
    }
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some(shell.into()),
        args,
        cwd: Some(fixture.dir.clone()),
        ..PtySessionConfig::default()
    })
    .expect("spawn integrated PowerShell");
    let startup = wait_for(&mut session, b"\x1b]133;B\x1b\\");
    if without_readline {
        assert!(startup
            .windows(b"READLINE-ABSENT".len())
            .any(|b| b == b"READLINE-ABSENT"));
    }

    assert_powershell_ls(&mut session, &fixture.filename);
    assert_powershell_foreground_input(&mut session);

    for command in ["clear", "cls", "Clear-Host"] {
        let clear = format!("{command}\r");
        assert_eq!(session.write(clear.as_bytes()).unwrap(), clear.len());
        wait_for(&mut session, b"\x1b]133;D;0\x1b\\");
        assert_powershell_ls(&mut session, &fixture.filename);

        let started = Instant::now();
        let command = b"Write-Output ('neoism-' + (7100 + 99) + '-done'); Start-Sleep -Milliseconds 250\r";
        assert_eq!(session.write(command).unwrap(), command.len());
        let output = wait_for(&mut session, b"\x1b]133;D;0\x1b\\");
        for marker in [b"neoism-7199-done".as_slice(), b"\x1b]133;D;0\x1b\\"] {
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
    let output = wait_for(&mut session, b"\x1b]133;D;1\x1b\\");
    let failed = b"\x1b]133;D;1\x1b\\";
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
    integrated_powershell_lifecycle("powershell.exe", false);
}

#[test]
#[ignore = "requires native Windows ConPTY and PowerShell 7 installed"]
fn pwsh_integration_completes_clear_sleep_and_errors() {
    integrated_powershell_lifecycle("pwsh.exe", false);
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

#[test]
#[ignore = "requires native Windows ConPTY and Windows PowerShell"]
fn powershell_integration_without_psreadline_accepts_cr_and_completes() {
    integrated_powershell_lifecycle("powershell.exe", true);
}

#[test]
#[ignore = "requires native Windows ConPTY and PowerShell 7 installed"]
fn pwsh_integration_without_psreadline_accepts_cr_and_completes() {
    integrated_powershell_lifecycle("pwsh.exe", true);
}
