//! A real controlling PTY, not `sh -c` in an unrelated child cwd.
use super::*;
#[cfg(target_os = "linux")]
use std::{
    fs::File,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    },
    process::{Child, Command, Stdio},
};

#[test]
fn agent_view_selects_its_primary_backing_shell_without_focus_change() {
    // The focused Agent view covers the only live primary Context.
    assert_eq!(
        select_backing_route(
            7,
            false,
            Some(7),
            [(7, has_backing_pty(123, false, true))].into_iter()
        ),
        Some(7)
    );
    // A virtual editor has pid 0 / fd -1. Use the workspace primary, not it.
    assert_eq!(
        select_backing_route(
            9,
            false,
            Some(7),
            [(9, has_backing_pty(0, false, false)), (7, true)].into_iter()
        ),
        Some(7)
    );
    assert_eq!(
        select_backing_route(8, true, Some(7), [(7, true), (8, true)].into_iter()),
        Some(8)
    );
    assert_eq!(
        select_backing_route(9, false, Some(9), [(9, false)].into_iter()),
        None
    );
}

#[test]
fn stale_selections_and_replaced_terminal_identities_do_not_ack_new_intent() {
    let mut pending = PendingTerminalDirectory {
        route: 7,
        shell_pid: 123,
        rich_text_id: 55,
        desired: PathBuf::from("/B"),
        remote_session: None,
        flight: Some(DirectoryFlight {
            root: PathBuf::from("/A"),
            finished_generation: 3,
            sent_at: Instant::now(),
            request_id: None,
        }),
        failed: false,
        queued_at: Instant::now(),
    };
    // A was written, then B was selected before its actual-cwd report arrived.
    assert!(!pending.observe_local_directory(Some(Path::new("/home")), true));
    assert!(!pending.observe_local_directory(None, true));
    assert!(!pending.observe_local_directory(Some(Path::new("/A")), true));
    assert!(pending.flight.is_none()); // B may now be queued, but its gate stays.
    assert!(pending.observe_local_directory(Some(Path::new("/B")), true));
    assert!(pending.matches_identity(7, 123, 55, None));
    assert!(!pending.matches_identity(7, 124, 55, None));
    assert!(!pending.matches_identity(7, 123, 56, None));
    assert!(!pending.matches_identity(8, 123, 55, None));
    assert!(!pending.matches_identity(7, 123, 55, Some("another-user-session")));
    pending.remote_session = Some("own-session".into());
    pending.flight = Some(DirectoryFlight {
        root: PathBuf::from("/B"),
        finished_generation: 0,
        sent_at: Instant::now(),
        request_id: Some("request-B".into()),
    });
    assert!(
        !pending.observe_local_directory(Some(Path::new("/B")), true),
        "remote results require correlated host ack, not local observation"
    );
}

#[test]
fn unsafe_windows_and_unknown_prompts_never_write() {
    assert!(safe_prompt(true, false, false, false, true, false));
    for flags in [
        (false, false, false, false, true, false),
        (true, true, false, false, true, false),
        (true, false, true, false, true, false),
        (true, false, false, true, true, false),
        (true, false, false, false, false, false),
        (true, false, false, false, true, true),
    ] {
        assert!(!safe_prompt(
            flags.0, flags.1, flags.2, flags.3, flags.4, flags.5
        ));
    }
}

#[cfg(target_os = "linux")]
struct ShellPty {
    master: File,
    child: Child,
}
#[cfg(target_os = "linux")]
impl ShellPty {
    fn start(cwd: &Path) -> Self {
        let (mut master, mut slave) = (-1, -1);
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null(),
                    std::ptr::null(),
                )
            },
            0
        );
        let master = unsafe { File::from_raw_fd(master) };
        let slave = unsafe { File::from_raw_fd(slave) };
        let mut command = Command::new("/bin/bash");
        command
            .args(["--noprofile", "--norc", "-i"])
            .current_dir(cwd)
            .env("PS1", "\x1b]133;B\x07")
            .env(
                "PROMPT_COMMAND",
                r#"printf '\033]7;file://localhost%s\007' "$PWD""#,
            )
            .env("TERM", "xterm-256color")
            .stdin(Stdio::from(slave.try_clone().unwrap()))
            .stdout(Stdio::from(slave.try_clone().unwrap()))
            .stderr(Stdio::from(slave));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command.spawn().unwrap();
        let mut pty = Self { master, child };
        pty.read_until("\x1b]133;B\x07");
        pty
    }
    fn read_until(&mut self, needle: &str) -> String {
        let until = Instant::now() + Duration::from_secs(8);
        let mut output = Vec::new();
        while Instant::now() < until {
            let mut poll = libc::pollfd {
                fd: self.master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            if unsafe { libc::poll(&mut poll, 1, 100) } > 0 {
                let mut chunk = [0; 8192];
                let n = self.master.read(&mut chunk).unwrap();
                assert!(n > 0);
                output.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&output);
                if text.contains(needle) {
                    return text.into_owned();
                }
            }
        }
        panic!(
            "PTY did not emit {needle:?}: {}",
            String::from_utf8_lossy(&output)
        );
    }
    fn run(&mut self, command: &str) -> String {
        self.master
            .write_all(&TerminalShellKind::Bash.command_payload(command, false))
            .unwrap();
        self.read_until("\x1b]133;B\x07")
    }
    fn cwd(&self) -> PathBuf {
        std::fs::read_link(format!("/proc/{}/cwd", self.child.id())).unwrap()
    }
    fn foreground_shell(&self) -> bool {
        (unsafe { libc::tcgetpgrp(self.master.as_raw_fd()) }) == self.child.id() as i32
    }
}
#[cfg(target_os = "linux")]
impl Drop for ShellPty {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn real_pty_home_to_repo_cd_preserves_shell_jobs_environment_and_draft() {
    let root = std::env::temp_dir()
        .join(format!("neoism-native-rebase-{}", uuid::Uuid::new_v4()));
    let home = root.join("home");
    let repo = root.join("builder path's repository");
    std::fs::create_dir_all(&home).unwrap();
    std::fs::create_dir_all(&repo).unwrap();
    std::fs::write(
        repo.join("Cargo.toml"),
        "[package]\nname='rebase-contract'\nversion='0.0.0'\nedition='2021'\n",
    )
    .unwrap();
    let mut pty = ShellPty::start(&home);
    let pid = pty.child.id();
    assert_eq!(pty.cwd(), home);
    let before = pty.run("cargo locate-project --message-format plain");
    assert!(
        before.contains("could not find"),
        "home unexpectedly resolved a manifest: {before}"
    );
    let setup = pty.run(
        "export KEEP=preserved; keep() { printf 'function-preserved'; }; sleep 20 &",
    );
    assert!(!setup.contains("command not found"), "{setup}");
    assert!(pty.foreground_shell());
    let mut input = neoism_ui::terminal_blocks::TerminalInputBuffer::default();
    input.insert_str("cargo check --not-submitted");
    input.move_home();
    input.move_right();
    let draft = input.text().to_owned();
    let command = directory_command(TerminalShellKind::Bash, &repo).unwrap();
    input.record_control_command(&command, Some(&home), Some(0));
    assert_eq!(input.text(), draft);
    assert_eq!(input.cursor_byte(), 1);
    assert!(safe_prompt(
        true,
        false,
        false,
        false,
        pty.foreground_shell(),
        false
    ));
    let mut pending = PendingTerminalDirectory {
        route: 7,
        shell_pid: pid,
        rich_text_id: 55,
        desired: repo.clone(),
        remote_session: None,
        flight: Some(DirectoryFlight {
            root: repo.clone(),
            finished_generation: 0,
            sent_at: Instant::now(),
            request_id: None,
        }),
        failed: false,
        queued_at: Instant::now(),
    };
    assert!(
        !pending.observe_local_directory(Some(&pty.cwd()), true),
        "writing or selecting is not a cwd acknowledgment"
    );
    pty.run(&command);
    // This is the very same kernel-observed ack the native queue requires.
    assert!(pending.observe_local_directory(Some(&pty.cwd()), true));
    assert_eq!(pty.cwd(), repo);
    assert_eq!(pty.child.id(), pid);
    let output = pty.run("printf '\\nDIR=%s\\nENV=%s\\n' \"$PWD\" \"$KEEP\"; keep; printf '\\n'; cargo locate-project --message-format plain; jobs -p");
    assert!(
        output.contains(&format!("DIR={}", repo.display())),
        "{output}"
    );
    assert!(output.contains("ENV=preserved"), "{output}");
    assert!(output.contains("function-preserved"), "{output}");
    assert!(
        output.contains(&repo.join("Cargo.toml").to_string_lossy().to_string()),
        "{output}"
    );
    let jobs = pty.run("printf '\\nJOB=%s\\n' \"$(jobs -pr)\"");
    assert!(
        jobs.lines()
            .filter_map(|line| line.strip_prefix("JOB="))
            .any(|pid| pid.trim().parse::<u32>().is_ok()),
        "background job lost: {jobs}"
    );
    pty.run("kill $(jobs -pr) 2>/dev/null; wait 2>/dev/null");
    drop(pty);
    std::fs::remove_dir_all(root).unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn real_busy_and_alternate_screen_defer_without_sending_control_bytes() {
    let mut pty = ShellPty::start(&std::env::temp_dir());
    pty.master.write_all(b"sleep 0.3\n").unwrap();
    let until = Instant::now() + Duration::from_secs(2);
    while pty.foreground_shell() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(!pty.foreground_shell());
    assert!(!safe_prompt(
        false,
        true,
        false,
        false,
        pty.foreground_shell(),
        false
    ));
    // Deliberately send NOTHING while the existing process owns the terminal.
    pty.read_until("\x1b]133;B\x07");
    assert!(pty.foreground_shell());
    pty.master
        .write_all(b"printf '\\033[?1049h'; sleep 0.3; printf '\\033[?1049l'\n")
        .unwrap();
    pty.read_until("\x1b[?1049h");
    assert!(!safe_prompt(true, false, true, false, true, false));
    pty.read_until("\x1b]133;B\x07");
    assert!(pty.foreground_shell());
}
