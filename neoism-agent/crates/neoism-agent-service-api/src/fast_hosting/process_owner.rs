//! Identity-checked ownership of ordinary Unix daemon descendants. No global
//! subreaper/cgroup changes. Intentionally detached processes are not contained.
use std::collections::HashMap;
#[cfg(any(not(target_os = "linux"), test))]
use std::process::Command;
#[cfg(not(target_os = "linux"))]
use std::process::Stdio;
#[derive(Clone)]
struct Process {
    pid: u32,
    parent: u32,
    group: u32,
    birth: String,
}
struct OwnedProcess {
    process: Process,
    #[cfg(target_os = "linux")]
    pidfd: Option<std::os::fd::OwnedFd>,
}
pub(super) struct ProcessOwner {
    root: u32,
    owned: HashMap<u32, OwnedProcess>,
}
impl ProcessOwner {
    pub(super) fn new(root: u32) -> Self {
        let mut owner = Self {
            root,
            owned: HashMap::new(),
        };
        // Root ownership is established ONCE, immediately after spawn. An
        // empty/failed initial snapshot must never claim a later recycled PID.
        if let Some(process) = snapshot().into_iter().find(|p| p.pid == root) {
            owner.insert(process);
        }
        owner.refresh();
        owner
    }
    pub(super) fn refresh(&mut self) {
        let processes = snapshot();
        // Release dead descendant pidfds; retain the root birth permanently so
        // neither an empty map nor a reused PGID can manufacture ownership.
        self.owned.retain(|pid, owned| {
            *pid == self.root
                || processes
                    .iter()
                    .any(|current| same_identity(current, &owned.process))
        });
        loop {
            let additions: Vec<_> = processes
                .iter()
                .filter(|p| {
                    !self.owned.contains_key(&p.pid)
                        && self.owned.get(&p.parent).is_some_and(|parent| {
                            processes
                                .iter()
                                .any(|current| same_identity(current, &parent.process))
                        })
                })
                .cloned()
                .collect();
            if additions.is_empty() {
                break;
            }
            for process in additions {
                self.insert(process);
            }
        }
    }
    fn insert(&mut self, process: Process) {
        #[cfg(target_os = "linux")]
        let pidfd = {
            use std::os::fd::FromRawFd;
            let fd = unsafe { libc::syscall(libc::SYS_pidfd_open, process.pid, 0) };
            if fd < 0 {
                None
            } else {
                let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd as i32) };
                // Snapshot/open race: keep a pidfd only if it is the same birth.
                if read_process(process.pid).is_some_and(|p| same_identity(&p, &process))
                {
                    Some(fd)
                } else {
                    return;
                }
            }
        };
        self.owned.insert(
            process.pid,
            OwnedProcess {
                process,
                #[cfg(target_os = "linux")]
                pidfd,
            },
        );
    }
    pub(super) fn kill(&mut self) {
        self.refresh();
        // Separate-session PTYs survive daemon crashes; pidfds retain ownership
        // even after reparenting, and cannot signal a reused PID.
        for (pid, process) in &self.owned {
            if *pid != self.root {
                signal(process);
            }
        }
        let current = snapshot();
        // The original group remains owned after its leader exits, but must
        // not be signalled if the PGID has since been reused by a new leader.
        let leader_reused = current.iter().any(|p| {
            p.pid == self.root
                && self
                    .owned
                    .get(&self.root)
                    .is_none_or(|original| !same_identity(p, &original.process))
        });
        let original_group_exists = current.iter().any(|p| {
            p.group == self.root
                && self
                    .owned
                    .get(&p.pid)
                    .is_some_and(|owned| same_identity(p, &owned.process))
        });
        if !leader_reused && original_group_exists {
            unsafe {
                libc::kill(-(self.root as i32), libc::SIGKILL);
            }
        }
        if let Some(root) = self.owned.get(&self.root) {
            signal(root);
        }
    }
}
fn same_identity(a: &Process, b: &Process) -> bool {
    a.pid == b.pid && a.birth == b.birth
}
fn signal(owned: &OwnedProcess) {
    #[cfg(target_os = "linux")]
    if let Some(fd) = &owned.pidfd {
        use std::os::fd::AsRawFd;
        unsafe {
            libc::syscall(
                libc::SYS_pidfd_send_signal,
                fd.as_raw_fd(),
                libc::SIGKILL,
                std::ptr::null::<libc::siginfo_t>(),
                0,
            );
        }
        return;
    }
    // macOS/older Linux fallback: check the current birth, never bare retained
    // PID. Unlike Linux pidfds this check/signal has a small OS-level race.
    if snapshot().iter().any(|p| same_identity(p, &owned.process)) {
        unsafe {
            libc::kill(owned.process.pid as i32, libc::SIGKILL);
        }
    }
}
#[cfg(target_os = "linux")]
fn read_process(pid: u32) -> Option<Process> {
    let text = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // comm may contain whitespace and ')'; remaining fields begin after its
    // LAST closing parenthesis. starttime is stat field 22 (remaining index 19).
    let fields: Vec<_> = text.rsplit_once(')')?.1.split_whitespace().collect();
    Some(Process {
        pid,
        parent: fields.get(1)?.parse().ok()?,
        group: fields.get(2)?.parse().ok()?,
        birth: fields.get(19)?.to_string(),
    })
}
#[cfg(target_os = "linux")]
fn snapshot() -> Vec<Process> {
    std::fs::read_dir("/proc")
        .into_iter()
        .flatten()
        .filter_map(|entry| {
            let pid = entry.ok()?.file_name().to_str()?.parse().ok()?;
            read_process(pid)
        })
        .collect()
}
#[cfg(not(target_os = "linux"))]
fn snapshot() -> Vec<Process> {
    let output = ["/bin/ps", "/usr/bin/ps"].into_iter().find_map(|binary| {
        Command::new(binary)
            .args(["-axo", "pid=,ppid=,pgid=,lstart="])
            .stdin(Stdio::null())
            .output()
            .ok()
    });
    output
        .into_iter()
        .flat_map(|output| {
            String::from_utf8_lossy(&output.stdout)
                .lines()
                .filter_map(|line| {
                    let mut fields = line.split_whitespace();
                    Some(Process {
                        pid: fields.next()?.parse().ok()?,
                        parent: fields.next()?.parse().ok()?,
                        group: fields.next()?.parse().ok()?,
                        birth: fields.collect::<Vec<_>>().join(" "),
                    })
                })
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn recycled_pid_is_not_owned() {
        let original = Process {
            pid: 100,
            parent: 1,
            group: 100,
            birth: "old".into(),
        };
        let recycled = Process {
            birth: "new".into(),
            ..original.clone()
        };
        assert!(!same_identity(&original, &recycled));
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn stale_retained_identity_cannot_kill_unrelated_live_process() {
        let mut unrelated = Command::new("/bin/sleep").arg("30").spawn().unwrap();
        let process = read_process(unrelated.id()).unwrap();
        let stale = Process {
            birth: "not-the-current-birth".into(),
            ..process
        };
        let mut owner = ProcessOwner {
            root: u32::MAX,
            owned: HashMap::from([(
                stale.pid,
                OwnedProcess {
                    process: stale,
                    pidfd: None,
                },
            )]),
        };
        owner.kill();
        assert!(unrelated.try_wait().unwrap().is_none());
        let mut unknown_root = ProcessOwner {
            root: unrelated.id(),
            owned: HashMap::new(),
        };
        unknown_root.kill(); // An empty tracker must never claim this root later.
        assert!(unrelated.try_wait().unwrap().is_none());
        unrelated.kill().unwrap();
        unrelated.wait().unwrap();
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn captured_separate_session_survives_root_crash_but_not_stop() {
        use std::{os::unix::process::CommandExt, time::Duration};
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("spawn.sh");
        std::fs::write(&script, "#!/bin/sh\n/usr/bin/setsid /bin/sleep 30 &\necho $! > child.pid\n/bin/sleep 30\n").unwrap();
        let mut command = Command::new("/bin/sh");
        command.arg(script).current_dir(dir.path());
        unsafe {
            command.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        let mut root = command.spawn().unwrap();
        let mut owner = ProcessOwner::new(root.id());
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while !dir.path().join("child.pid").exists()
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
        owner.refresh();
        let child: u32 = std::fs::read_to_string(dir.path().join("child.pid"))
            .unwrap()
            .trim()
            .parse()
            .unwrap();
        assert!(owner.owned.contains_key(&child));
        root.kill().unwrap();
        root.wait().unwrap();
        assert!(read_process(child).is_some());
        owner.kill();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        loop {
            let stat = std::fs::read_to_string(format!("/proc/{child}/stat"));
            if stat.as_ref().is_err()
                || stat
                    .unwrap()
                    .rsplit_once(')')
                    .unwrap()
                    .1
                    .split_whitespace()
                    .next()
                    == Some("Z")
            {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "owned PTY-like process survived"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}
