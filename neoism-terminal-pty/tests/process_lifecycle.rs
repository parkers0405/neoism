#![cfg(unix)]

use neoism_terminal_pty::{PtySession, PtySessionConfig};
use std::time::{Duration, Instant};

fn spawn(script: &str) -> PtySession {
    spawn_with_env(script, vec![])
}

fn spawn_with_env(script: &str, env: Vec<(String, String)>) -> PtySession {
    PtySession::spawn(PtySessionConfig {
        shell: Some("/bin/sh".into()),
        args: vec!["-c".into(), script.into()],
        cwd: None,
        env,
        cols: 80,
        rows: 24,
    }).expect("spawn owned shell")
}

#[test]
fn ordinary_close_allows_hup_cleanup_to_finish() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let cleanup = std::env::temp_dir().join(format!(
        "neoism-pty-hup-cleanup-{}-{unique}", std::process::id()
    ));
    let mut session = spawn_with_env(
        "trap 'sleep 0.05; printf cleanup-complete > \"$CLEANUP_FILE\"; exit 0' HUP; printf ready; while :; do :; done",
        vec![("CLEANUP_FILE".into(), cleanup.to_string_lossy().into_owned())],
    );
    let pid = session.shell_pid();
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut output = Vec::new();
    let mut buf = [0; 256];
    while !output.windows(5).any(|bytes| bytes == b"ready") {
        if let Ok(n) = session.read(&mut buf) { output.extend_from_slice(&buf[..n]); }
        assert!(Instant::now() < deadline, "HUP trap was not installed");
        std::thread::sleep(Duration::from_millis(5));
    }
    let start = Instant::now();
    session.close();
    assert!(start.elapsed() < Duration::from_millis(200), "close waited for grace period");
    assert_reaped(pid);
    // The PTY is closed, so use an independent sink to prove the shell trap
    // ran to completion rather than racing a disconnected output receiver.
    let emitted = std::fs::read(&cleanup).expect("HUP cleanup did not emit its marker");
    std::fs::remove_file(cleanup).unwrap();
    assert_eq!(emitted, b"cleanup-complete");
}

fn assert_reaped(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        // Unlike waitpid, this probe cannot accidentally reap the child and
        // conceal the bug. Zombies still exist and answer kill(pid, 0).
        if unsafe { libc::kill(pid as i32, 0) } == -1
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH)
        {
            break;
        }
        assert!(Instant::now() < deadline, "owned child {pid} alive or zombie");
        std::thread::sleep(Duration::from_millis(10));
    }
    let mut status = 0;
    assert_eq!(unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) }, -1);
    assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ECHILD));
}

#[test]
fn explicit_close_and_drop_reap_owned_leaders() {
    // An unrelated sibling must survive; no process-group signalling allowed.
    let sibling = spawn("read line");
    let sibling_pid = sibling.shell_pid();
    for explicit in [true, false, true, false] {
        let mut session = spawn("trap '' HUP; printf ready; while :; do :; done");
        let pid = session.shell_pid();
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let mut buf = [0; 256];
        while !output.windows(5).any(|bytes| bytes == b"ready") {
            if let Ok(n) = session.read(&mut buf) { output.extend_from_slice(&buf[..n]); }
            assert!(Instant::now() < deadline, "shell did not become ready");
            std::thread::sleep(Duration::from_millis(5));
        }
        let start = Instant::now();
        if explicit { session.close(); } else { drop(session); }
        assert!(start.elapsed() < Duration::from_secs(1), "close blocked caller");
        assert_reaped(pid);
        assert_eq!(unsafe { libc::kill(sibling_pid as i32, 0) }, 0);
    }
    sibling.close();
    assert_reaped(sibling_pid);
}

#[test]
fn natural_exit_reaps_and_preserves_final_output_and_notification() {
    // Repeat immediate exits to cover exit-before-signal-registration races.
    for _ in 0..16 {
        let mut session = spawn("printf final-output; exit 7");
        let pid = session.shell_pid();
        let events = session.take_child_event_receiver().expect("child events");
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut output = Vec::new();
        let mut buf = [0; 256];
        loop {
            match session.read(&mut buf) {
                Ok(0) if session.exit_code().is_some() => break,
                Ok(n) => output.extend_from_slice(&buf[..n]),
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {},
                Err(error) => panic!("read: {error}"),
            }
            assert!(Instant::now() < deadline, "natural exit was lost");
            std::thread::sleep(Duration::from_millis(5));
        }
        let status = session.exit_code().unwrap();
        assert!(libc::WIFEXITED(status));
        assert_eq!(libc::WEXITSTATUS(status), 7);
        // macOS's login intermediary may emit a last-login banner first.
        assert!(output.ends_with(b"final-output"), "final output lost: {output:?}");
        assert_eq!(events.try_recv().unwrap(), status);
        assert!(events.try_recv().is_err(), "duplicate exit event");
        assert_reaped(pid);
        session.close();
    }
}

#[cfg(target_os = "linux")]
#[test]
fn exit_before_poll_registration_remains_signalled_for_both_spawn_paths() {
    use teletypewriter::{EventedPty, ProcessReadWrite};

    for use_fork in [false, true] {
        let mut pty = if use_fork {
            teletypewriter::create_pty_with_fork("/bin/true", 80, 24).unwrap()
        } else {
            teletypewriter::create_pty_with_spawn(
                "/bin/sh", vec!["-c".into(), "exit 0".into()], &None, 80, 24,
            ).unwrap()
        };
        let pid = *pty.child.pid as u32;
        // Observe exit without reaping it, so registration definitely happens
        // after SIGCHLD rather than relying on a scheduler timing assumption.
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            let mut info = std::mem::MaybeUninit::<libc::siginfo_t>::zeroed();
            let result = unsafe {
                libc::waitid(libc::P_PID, pid, info.as_mut_ptr(), libc::WEXITED | libc::WNOWAIT | libc::WNOHANG)
            };
            assert_eq!(result, 0);
            if unsafe { info.assume_init().si_pid() } == pid as i32 { break; }
            assert!(Instant::now() < deadline, "child did not exit");
            std::thread::sleep(Duration::from_millis(5));
        }
        let poll = corcovado::Poll::new().unwrap();
        let mut tokens = (0..).map(Into::into);
        pty.register(&poll, &mut tokens, corcovado::Ready::readable(),
            corcovado::PollOpt::edge() | corcovado::PollOpt::oneshot()).unwrap();
        let mut events = corcovado::Events::with_capacity(8);
        poll.poll(&mut events, Some(Duration::from_secs(2))).unwrap();
        assert!(events.iter().any(|event| event.token() == pty.child_event_token()),
            "exit notification lost before poll registration (fork={use_fork})");
        assert!(matches!(pty.next_child_event(), Some(teletypewriter::ChildEvent::Exited)));
        assert_eq!(libc::WEXITSTATUS(pty.child_exit_status().unwrap()), 0);
        assert!(pty.next_child_event().is_none());
        assert_reaped(pid);
    }
}