//! Headless Windows composer -> ConPTY -> production parser -> command-block test.
//! `cargo test -p neoism-ui --test windows_composer -- --ignored --nocapture`
//! Requires pwsh.exe on PATH (or NEOISM_TEST_PWSH). This deliberately does not
//! instantiate a window, keyboard dispatcher, backend Machine/messenger, or GPU
//! block overlay. It exercises their shared input/parser/state boundary, not
//! raw OSC byte matching. Parser replies take the production direct-reply path.
//! On an isolated CI account, set `NEOISM_TEST_POWERSHELL_DEFAULT_PROFILE=1`
//! to omit `-NoProfile` (loads existing profiles; never modifies them).
#![cfg(windows)]

use neoism_terminal_core::{
    ansi::CursorShape, crosswords::Mode, handler::Processor, Crosswords, TerminalEffect,
    TerminalId,
};
use neoism_terminal_pty::{PtySession, PtySessionConfig};
use neoism_ui::{
    input::TerminalShellKind,
    terminal_blocks::{BlockStatusKind, TerminalInputBuffer},
};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};

struct Harness {
    pty: Option<PtySession>,
    parser: Processor,
    term: Crosswords,
    input: TerminalInputBuffer,
    cwd: PathBuf,
    replies: usize,
    defer_render: bool,
    output: Vec<u8>,
}

impl Drop for Harness {
    fn drop(&mut self) {
        if let Some(pty) = self.pty.take() {
            pty.close();
        }
    }
}

impl Harness {
    fn new(cwd: PathBuf) -> Self {
        let shell =
            std::env::var("NEOISM_TEST_PWSH").unwrap_or_else(|_| "pwsh.exe".into());
        Self::with_shell(cwd, &shell, false)
    }

    fn with_shell(cwd: PathBuf, shell: &str, without_readline: bool) -> Self {
        // Profile loading is opt-in for an isolated CI account; no profile writes.
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
        let pty = PtySession::spawn(PtySessionConfig {
            shell: Some(shell.into()),
            args,
            cwd: Some(cwd.clone()),
            cols: 120,
            rows: 40,
            ..Default::default()
        })
        .expect("spawn production PowerShell hook in ConPTY");
        let mut input = TerminalInputBuffer::default();
        input.set_shell_kind(TerminalShellKind::PowerShell);
        Self {
            pty: Some(pty),
            parser: Processor::new(),
            term: Crosswords::new(
                (40usize, 120usize),
                CursorShape::Block,
                TerminalId::new(0),
                1000,
            ),
            input,
            cwd,
            replies: 0,
            defer_render: false,
            output: Vec::new(),
        }
    }

    fn parse(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
        self.parser.advance(&mut self.term, bytes);
        // Same ordering as performer: parse, dispatch direct protocol writes,
        // then let the render-side composer sample the terminal's real state.
        for effect in self.term.drain_effects() {
            if let TerminalEffect::PtyWrite(bytes) = effect {
                assert_eq!(
                    self.pty.as_mut().unwrap().write_reply(&bytes).unwrap(),
                    bytes.len()
                );
                self.replies += 1;
            }
        }
        self.sync_render_state();
    }

    fn sync_render_state(&mut self) {
        if self.defer_render {
            return;
        }
        let state = self.term.shell_prompt_state();
        let line = self.term.cursor().pos.row;
        let absolute_row = self.term.absolute_row_for_line(line);
        let prompt_row: String = self.term.grid[line]
            .inner
            .iter()
            .map(|cell| cell.c())
            .collect();
        self.input.sync_shell_state(state);
        // terminal_compose.rs runs this after sync for a local Windows PTY.
        self.input
            .finish_unintegrated_local_command_at_prompt(&prompt_row, Some(absolute_row));
    }

    fn pump(&mut self) {
        let mut bytes = [0; 65536];
        match self.pty.as_mut().unwrap().read(&mut bytes) {
            Ok(n) => self.parse(&bytes[..n]),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                self.sync_render_state();
            }
            Err(e) => panic!("PTY read: {e}"),
        }
        assert!(
            self.pty.as_ref().unwrap().exit_code().is_none(),
            "shell exited"
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    fn grid(&self) -> String {
        // Include scrollback, just as block output can extend above viewport.
        (-(self.term.history_size() as i32)..40)
            .map(|row| {
                self.term.grid[neoism_terminal_core::crosswords::pos::Line(row)]
                    .inner
                    .iter()
                    .map(|cell| cell.c())
                    .collect::<String>()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wait(&mut self, label: &str, predicate: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !predicate(self) {
            assert!(
                Instant::now() < deadline,
                "timeout {label}; state={:?}; blocks={:?}; grid:\n{}",
                self.term.shell_prompt_state(),
                self.input.command_block_snapshots(),
                self.grid()
            );
            self.pump();
        }
    }

    fn status(&self) -> Option<BlockStatusKind> {
        self.input
            .command_block_snapshots()
            .last()
            .map(|b| b.status)
    }

    fn submit(&mut self, text: &str) {
        let row = self.term.absolute_row_for_line(self.term.cursor().pos.row);
        self.input.insert_str(text);
        let command = self.input.submit_with_context(Some(&self.cwd), Some(row));
        assert_eq!(command, text);
        assert!(self.input.text().is_empty(), "Enter consumes composer text");
        let kind = self.input.shell_kind();
        if kind.is_clear_command(&command) {
            self.input.clear_previous_blocks_for_active_command();
        }
        assert_eq!(self.status(), Some(BlockStatusKind::Running));
        assert_eq!(
            self.input
                .command_block_snapshots()
                .last()
                .unwrap()
                .output_start_row,
            Some(row)
        );
        // A render before transport delivery must not finish against the old B.
        self.sync_render_state();
        assert_eq!(self.status(), Some(BlockStatusKind::Running));
        let bytes = kind
            .command_payload(&command, self.term.mode().contains(Mode::BRACKETED_PASTE));
        assert_eq!(bytes.last(), Some(&b'\r'));
        assert_eq!(
            self.pty.as_mut().unwrap().write(&bytes).unwrap(),
            bytes.len()
        );
    }

    fn foreground_input(&mut self) {
        self.wait("editable prompt before Read-Host", |s| {
            s.term.shell_prompt_state().awaiting_command
        });
        let generation = self.term.shell_prompt_state().command_finished_generation;
        self.output.clear();
        // Computed prompt cannot be satisfied by source echo. The reply token is
        // sent separately to the foreground reader, not submitted as a new block.
        self.submit("$answer = Read-Host ('composer-' + (7100 + 99) + '-ready'); Write-Output ('reply-' + $answer.ToUpperInvariant() + '-done')");
        self.wait("Read-Host ready prompt", |s| {
            assert_eq!(s.status(), Some(BlockStatusKind::Running));
            assert_eq!(
                s.term.shell_prompt_state().command_finished_generation,
                generation
            );
            s.grid().contains("composer-7199-ready")
        });
        let start = Instant::now();
        while start.elapsed() < Duration::from_millis(250) {
            // Keep answering real cursor queries while the foreground read waits.
            self.pump();
            assert_eq!(self.status(), Some(BlockStatusKind::Running));
            assert_eq!(
                self.term.shell_prompt_state().command_finished_generation,
                generation
            );
        }
        let blocks = self.input.command_block_count();
        let token = b"foreground7199\r";
        assert_eq!(
            self.pty.as_mut().unwrap().write(token).unwrap(),
            token.len()
        );
        self.wait("Read-Host Finished(0)", |s| {
            s.status() == Some(BlockStatusKind::Ok)
        });
        assert_eq!(self.input.command_block_count(), blocks);
        assert_eq!(self.term.shell_prompt_state().last_exit_code, Some(0));
        assert!(self.grid().contains("reply-FOREGROUND7199-done"));
        let done = b"\x1b]133;D;0\x1b\\";
        let completed_at = self
            .output
            .windows(done.len())
            .position(|b| b == done)
            .expect("successful D after foreground reply");
        let reply = b"reply-FOREGROUND7199-done";
        assert!(
            self.output[..completed_at]
                .windows(reply.len())
                .any(|b| b == reply),
            "computed uppercase reply must precede successful D: {:?}",
            String::from_utf8_lossy(&self.output)
        );
        println!("PASS Read-Host: Running while awaiting separate CR input, uppercase reply before Finished(0)");
    }

    fn ls(&mut self, filename: &str) {
        self.submit("ls");
        self.wait("ls Finished(0)", |s| {
            s.status() == Some(BlockStatusKind::Ok)
        });
        assert_eq!(self.term.shell_prompt_state().last_exit_code, Some(0));
        assert!(
            self.grid().contains(filename),
            "listing missing from parsed grid: {}",
            self.grid()
        );
        println!("PASS ls: Running -> Finished(0), fixture retained in output grid");
    }
}

fn foreground_input(shell: &str, without_readline: bool) {
    let fixture = tempfile::tempdir().unwrap();
    let mut h = Harness::with_shell(fixture.path().to_owned(), shell, without_readline);
    h.wait("initial editable prompt", |s| {
        s.term.shell_prompt_state().awaiting_command
    });
    if without_readline {
        assert!(h.grid().contains("READLINE-ABSENT"));
    }
    h.foreground_input();
}

#[test]
#[ignore = "requires native Windows ConPTY and Windows PowerShell"]
fn powershell51_composer_foreground_input() {
    foreground_input("powershell.exe", false);
}

#[test]
#[ignore = "requires native Windows ConPTY and Windows PowerShell"]
fn powershell51_composer_foreground_input_without_psreadline() {
    foreground_input("powershell.exe", true);
}

#[test]
#[ignore = "requires native Windows ConPTY and PowerShell 7 installed"]
fn pwsh7_composer_foreground_input_without_psreadline() {
    let shell = std::env::var("NEOISM_TEST_PWSH").unwrap_or_else(|_| "pwsh.exe".into());
    foreground_input(&shell, true);
}

#[test]
#[ignore = "requires native Windows/Wine ConPTY and installed PowerShell"]
fn powershell_composer_parser_blocks() {
    let fixture = tempfile::tempdir().unwrap();
    let filename = "composer-real-listing-fixture.txt";
    std::fs::write(fixture.path().join(filename), "not command echo").unwrap();
    let mut h = Harness::new(fixture.path().to_owned());
    h.wait("initial editable prompt", |s| {
        s.term.shell_prompt_state().awaiting_command
    });
    h.ls(filename);
    h.foreground_input();

    // Fast commands can complete between desktop render frames. Parse all real
    // output first, then sample only the final D/A/B state once.
    h.wait("editable prompt after ls", |s| {
        s.term.shell_prompt_state().awaiting_command
    });
    let generation = h.term.shell_prompt_state().command_finished_generation;
    h.submit("ls");
    h.defer_render = true;
    h.wait("batched ls D/A/B", |s| {
        let state = s.term.shell_prompt_state();
        state.command_finished_generation > generation && state.awaiting_command
    });
    assert_eq!(h.status(), Some(BlockStatusKind::Running));
    h.defer_render = false;
    h.sync_render_state();
    assert_eq!(h.status(), Some(BlockStatusKind::Ok));
    assert_eq!(h.term.shell_prompt_state().last_exit_code, Some(0));
    assert!(h.grid().contains(filename));
    println!(
        "PASS batched ls: final parser generation finishes block in one render sample"
    );

    h.submit("Start-Sleep -Seconds 2; Write-Output ('sleep-' + 'completed')");
    // Exercise an old B/redraw through the production parser, not fake fields.
    h.parse(b"\x1b]133;B\x1b\\");
    assert_eq!(h.status(), Some(BlockStatusKind::Running));
    h.wait("shell C before reply probe", |s| {
        s.term.shell_prompt_state().running_command
    });
    // Wine/ConPTY did not forward any startup query in the observed run.
    // Inject DSR at the parser boundary to exercise an actual synchronous
    // PtySession::write_reply, acknowledged after every byte reaches the PTY.
    // CPR is a console-host protocol reply, not ordinary ReadKey shell input.
    let replies_before = h.replies;
    let grid_before = h.grid();
    h.parse(b"\x1b[6n");
    assert_eq!(h.replies, replies_before + 1);
    assert_eq!(h.grid(), grid_before, "query/reply must not paint glyphs");
    assert_eq!(h.status(), Some(BlockStatusKind::Running));
    println!("PASS injected DSR: production parser CPR acknowledged by real PTY, grid unchanged");
    let start = Instant::now();
    while start.elapsed() < Duration::from_millis(1200) {
        h.pump();
        assert_eq!(
            h.status(),
            Some(BlockStatusKind::Running),
            "premature sleep completion"
        );
    }
    h.wait("sleep completion", |s| {
        s.status() == Some(BlockStatusKind::Ok)
    });
    assert!(h.grid().contains("sleep-completed"));
    assert!(h.grid().contains(filename), "earlier block output lost");
    println!("PASS sleep/old B: remains Running, then Finished(0), output retained");

    h.submit("neoism_command_that_does_not_exist_918273");
    h.wait(
        "shell error completion",
        |s| matches!(s.status(), Some(BlockStatusKind::Error(code)) if code != 0),
    );
    assert!(
        h.grid().contains("not recognized"),
        "shell diagnostic missing: {}",
        h.grid()
    );
    assert_ne!(h.term.shell_prompt_state().last_exit_code, Some(0));
    println!("PASS nonexistent command: diagnostic in grid, Finished(nonzero)");

    for clear in ["clear", "cls", "Clear-Host"] {
        h.submit(clear);
        h.wait("clear removes completed block chrome", |s| {
            s.input.command_block_count() == 0
        });
        assert!(
            !h.grid().contains(filename),
            "clear did not clear output grid"
        );
        h.ls(filename);
        println!("PASS {clear}: clears blocks/grid; subsequent ls finishes");
    }
    println!(
        "Production parser replies routed directly to PTY: {}",
        h.replies
    );
}
