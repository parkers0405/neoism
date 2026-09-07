//! Validate release-smoke PowerShell syntax through a real Windows console.
#![cfg(windows)]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::time::{Duration, Instant};

fn validate_smoke_script(shell: &str) {
    let script = std::env::current_dir()
        .unwrap()
        .join("../scripts/windows-installed-gui-smoke.ps1")
        .canonicalize()
        .expect("locate installed-GUI smoke script");
    let path = script.to_string_lossy().replace('\'', "''");
    let command = format!(
        "$ErrorActionPreference='Stop'; $tokens=$null; $errors=$null; \
         $ast=[System.Management.Automation.Language.Parser]::ParseFile(\
         '{path}',[ref]$tokens,[ref]$errors); \
         if($errors.Count){{throw ($errors | Out-String)}}; \
         $add=$ast.Find({{param($a) \
         $a -is [System.Management.Automation.Language.CommandAst] -and \
         $a.GetCommandName() -eq 'Add-Type'}},$true); \
         Add-Type -TypeDefinition $add.CommandElements[1].Value; \
         if(-not [NeoismWindowProbe]::IsInternalEventWindow('Winit Thread Event Target',0x080800A0)){{throw 'event target not classified'}}; \
         if([NeoismWindowProbe]::IsInternalEventWindow('Winit Thread Event Target',0)){{throw 'ordinary target incorrectly excluded'}}; \
         foreach($class in @('Neoism','ConsoleWindowClass','CASCADIA_HOSTING_WINDOW_CLASS')){{ \
           if([NeoismWindowProbe]::IsInternalEventWindow($class,0x080800A0)){{throw 'real window incorrectly excluded'}} \
         }}; \
         Write-Output ('neoism-' + 'script-validated')"
    );
    let mut session = PtySession::spawn(PtySessionConfig {
        shell: Some(shell.into()),
        args: vec![
            "-NoLogo".into(),
            "-NoProfile".into(),
            "-Command".into(),
            command,
        ],
        ..PtySessionConfig::default()
    })
    .expect("spawn PowerShell script parser");
    let deadline = Instant::now() + Duration::from_secs(60);
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut answered_queries = 0;
    let marker = b"neoism-script-validated";
    while Instant::now() < deadline {
        match session.read(&mut buffer) {
            Ok(count) => output.extend_from_slice(&buffer[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("script parser output failed: {error}"),
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
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!(
        "PowerShell script validation did not succeed: {}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
#[ignore = "requires Windows ConPTY and PowerShell 7"]
fn pwsh_parses_smoke_script_and_compiles_window_probe() {
    validate_smoke_script("pwsh.exe");
}

#[test]
#[ignore = "requires Windows ConPTY and Windows PowerShell"]
fn windows_powershell_parses_smoke_script_and_compiles_window_probe() {
    validate_smoke_script("powershell.exe");
}
