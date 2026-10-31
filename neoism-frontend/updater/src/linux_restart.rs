//! Gracefully stop only the invoking Linux installation, then launch its new GUI.
use crate::Error;
use std::{
    fs, io,
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::MetadataExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn target_role(path: &Path, executable: &Path) -> Option<u8> {
    let directory = executable.parent()?;
    let relative = path.strip_prefix(directory).ok()?;
    let parts = relative
        .components()
        .map(|part| part.as_os_str())
        .collect::<Vec<_>>();
    // Earlier updates can leave a still-running executable mapped from an
    // unlinked backup. Accept only this installation's known transaction paths.
    let name = match parts.as_slice() {
        [name] => *name,
        [transaction, backup, name]
            if (transaction.to_str()?.starts_with(".neoism-update-")
                && *backup == "backup")
                || (transaction.to_str()?.starts_with(".neoism-install.")
                    && *backup == "old") =>
        {
            *name
        }
        _ => return None,
    };
    if Some(name) == executable.file_name() {
        Some(0)
    } else if [
        "neoism-workspace-daemon",
        "neoism-agent",
        "neoism-agent-lua-runner",
    ]
    .iter()
    .any(|bin| name == *bin)
    {
        Some(1)
    } else {
        None
    }
}

fn process_path(pid: u32) -> Option<PathBuf> {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let path = fs::read_link(format!("/proc/{pid}/exe")).ok()?;
    let bytes = path.as_os_str().as_bytes();
    Some(PathBuf::from(std::ffi::OsString::from_vec(
        bytes.strip_suffix(b" (deleted)").unwrap_or(bytes).to_vec(),
    )))
}

struct Process {
    pid: u32,
    handle: OwnedFd,
}
impl Process {
    fn open(pid: u32) -> io::Result<Self> {
        let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, pid, 0) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self {
            pid,
            handle: unsafe { OwnedFd::from_raw_fd(fd as i32) },
        })
    }
    fn exited(&self) -> io::Result<bool> {
        let mut poll = libc::pollfd {
            fd: self.handle.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        if unsafe { libc::poll(&mut poll, 1, 0) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(poll.revents & libc::POLLIN != 0)
    }
    fn terminate(&self) -> io::Result<()> {
        // The handle prevents PID reuse from directing a signal at another app.
        let result = unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                self.handle.as_raw_fd(),
                libc::SIGTERM,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            )
        };
        if result < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                return Err(error);
            }
        }
        Ok(())
    }
}

pub fn stop_stack(
    executable: &Path,
    parent: Option<u32>,
    timeout: Duration,
) -> Result<(), Error> {
    let deadline = Instant::now() + timeout;
    // Finish GUI recovery checkpoints before taking its daemon away.
    for role in [0, 1] {
        loop {
            let mut processes = Vec::new();
            for entry in fs::read_dir("/proc")?.flatten() {
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<u32>().ok())
                else {
                    continue;
                };
                if pid == std::process::id() || Some(pid) == parent {
                    continue;
                }
                if !entry
                    .metadata()
                    .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
                {
                    continue;
                }
                if process_path(pid)
                    .as_deref()
                    .and_then(|path| target_role(path, executable))
                    != Some(role)
                {
                    continue;
                }
                // Leave another update CLI alone; the installation lock serializes it.
                let command = fs::read(entry.path().join("cmdline")).unwrap_or_default();
                if command.split(|b| *b == 0).nth(1).is_some_and(|arg| {
                    [
                        b"update".as_slice(),
                        b"upgrade",
                        b"self-update",
                        b"--update",
                    ]
                    .contains(&arg)
                }) {
                    continue;
                }
                let process = match Process::open(pid) {
                    Ok(process) => process,
                    Err(error) if error.raw_os_error() == Some(libc::ESRCH) => continue,
                    Err(error) => return Err(error.into()),
                };
                // Recheck identity after opening the handle, before signaling it.
                if process.exited()?
                    || process_path(pid)
                        .as_deref()
                        .and_then(|path| target_role(path, executable))
                        != Some(role)
                {
                    continue;
                }
                processes.push(process);
            }
            if processes.is_empty() {
                break;
            }
            if Instant::now() >= deadline {
                return Err("Timed out waiting for this Neoism installation to close; update not installed".into());
            }
            for process in &processes {
                process.terminate()?;
            }
            for process in processes {
                while !process.exited()? {
                    if Instant::now() >= deadline {
                        return Err(format!("Neoism process {} did not finish saving and closing; update not installed", process.pid).into());
                    }
                    std::thread::sleep(Duration::from_millis(25));
                }
            }
        }
    }
    Ok(())
}

pub fn relaunch(executable: &Path) -> Result<(), Error> {
    let mut command = Command::new(executable);
    command
        .arg("--new-window")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() == -1 {
                Err(io::Error::last_os_error())
            } else {
                Ok(())
            }
        });
    }
    command.spawn()?;
    Ok(())
}

pub fn protect_from_terminal_close() -> Result<(), Error> {
    // The update may have been invoked inside Neoism's own terminal. Closing
    // that window must not hang up the installer halfway through replacement.
    if unsafe { libc::signal(libc::SIGHUP, libc::SIG_IGN) } == libc::SIG_ERR {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    static TERMINATED: std::sync::atomic::AtomicBool =
        std::sync::atomic::AtomicBool::new(false);
    extern "C" fn terminate(_signal: libc::c_int) {
        TERMINATED.store(true, std::sync::atomic::Ordering::Relaxed);
    }
    #[test]
    fn graceful_child() {
        let Some(marker) = std::env::var_os("NEOISM_TEST_GRACEFUL_RESTART") else {
            return;
        };
        let marker = PathBuf::from(marker);
        unsafe {
            libc::signal(libc::SIGTERM, terminate as *const () as libc::sighandler_t);
        }
        fs::write(marker.with_extension("ready"), "ready").unwrap();
        while !TERMINATED.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(100));
        fs::write(marker, "saved before exit").unwrap();
    }
    #[test]
    fn shutdown_waits_for_checkpoint_and_handles_an_unlinked_previous_update() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("neoism");
        fs::copy(std::env::current_exe().unwrap(), &exe).unwrap();
        let marker = root.path().join("checkpoint");
        let mut child = Command::new(&exe)
            .args(["--exact", "linux_restart::tests::graceful_child"])
            .env("NEOISM_TEST_GRACEFUL_RESTART", &marker)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !marker.with_extension("ready").exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        if !marker.with_extension("ready").exists() {
            let _ = child.kill();
            let _ = child.wait();
            panic!("child did not become ready");
        }
        // The updater owner is exempt, even if it runs the same executable.
        stop_stack(&exe, Some(child.id()), Duration::from_secs(1)).unwrap();
        assert!(child.try_wait().unwrap().is_none());
        let backup = root.path().join(".neoism-update-test/backup");
        fs::create_dir_all(&backup).unwrap();
        fs::rename(&exe, backup.join("neoism")).unwrap();
        fs::remove_file(backup.join("neoism")).unwrap();
        let result = stop_stack(&exe, None, Duration::from_secs(3));
        let _ = child.kill();
        let _ = child.wait();
        result.unwrap();
        assert_eq!(fs::read_to_string(marker).unwrap(), "saved before exit");
    }
    #[test]
    fn only_the_invoking_installation_and_its_known_backups_match() {
        let exe = Path::new("/home/example/bin/neoism");
        for path in [
            "neoism",
            "neoism-workspace-daemon",
            ".neoism-update-abc/backup/neoism",
            ".neoism-install.abc/old/neoism-agent",
        ] {
            assert!(
                target_role(&exe.parent().unwrap().join(path), exe).is_some(),
                "{path}"
            );
        }
        for path in [
            "other/neoism",
            ".neoism-update-abc/staged/neoism",
            "web/native/neoism-update-helper",
            "neoism-lsp",
            "../other/neoism",
        ] {
            assert_eq!(
                target_role(&exe.parent().unwrap().join(path), exe),
                None,
                "{path}"
            );
        }
        assert_eq!(target_role(Path::new("/another/install/neoism"), exe), None);
    }
    #[test]
    fn shutdown_is_scoped_and_waits_for_graceful_exit_then_relaunches() {
        let root = tempfile::tempdir().unwrap();
        let exe = root.path().join("neoism");
        // A separate copy gives /proc/exe the same identity as a real installed app.
        fs::copy("/bin/sleep", &exe).unwrap();
        let mut gui = Command::new(&exe).arg("60").spawn().unwrap();
        let mut unrelated = Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let result = stop_stack(&exe, None, Duration::from_secs(3));
        let gui_exited = gui.try_wait().unwrap().is_some();
        let unrelated_running = unrelated.try_wait().unwrap().is_none();
        let _ = gui.kill();
        let _ = gui.wait();
        let _ = unrelated.kill();
        let _ = unrelated.wait();
        result.unwrap();
        assert!(gui_exited);
        assert!(unrelated_running);
        use std::os::unix::fs::PermissionsExt;
        let marker = root.path().join("launched");
        fs::write(
            &exe,
            format!("#!/bin/sh\nprintf '%s' \"$1\" > '{}'\n", marker.display()),
        )
        .unwrap();
        fs::set_permissions(&exe, fs::Permissions::from_mode(0o700)).unwrap();
        relaunch(&exe).unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !marker.exists() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(fs::read_to_string(marker).unwrap(), "--new-window");
    }
}
