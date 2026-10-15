//! Serialized live cwd control for daemon-spawned, source-integrated Linux shells.
//! Caller authentication/capabilities are checked by the socket just like PTY writes.
//! Lock order is control -> PTY for BOTH output observations and input writes.
use neoism_protocol::pty::ServerMessage;
use neoism_terminal_pty::PtySession;
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
use tokio::sync::mpsc;

struct Request {
    session_id: String,
    request_id: String,
    route: u64,
    tx: mpsc::Sender<ServerMessage>,
    target: PathBuf,
    since: Instant,
    reported_timeout: bool,
}
impl Request {
    fn fail(&self, message: &str) {
        let _ = self
            .tx
            .try_send(ServerMessage::TerminalDirectoryChangeFailed {
                session_id: self.session_id.clone(),
                request_id: self.request_id.clone(),
                message: message.into(),
            });
    }
}

pub(super) struct DirectoryControl {
    token: Option<String>,
    executable: Option<PathBuf>,
    output: Vec<u8>,
    prompt: bool,
    running: bool,
    submitted: usize,
    partial: bool,
    alternate: bool,
    queued: Option<Request>,
    active: Option<Request>,
}
impl DirectoryControl {
    pub(super) fn new(token: Option<String>, executable: Option<PathBuf>) -> Self {
        Self {
            token,
            executable,
            output: Vec::new(),
            prompt: false,
            running: false,
            submitted: 0,
            partial: false,
            alternate: false,
            queued: None,
            active: None,
        }
    }
    pub(super) fn blocks_input(&self) -> bool {
        self.queued.is_some() || self.active.is_some()
    }

    /// Called under the same lock as the actual input write. A previously observed
    /// prompt is unusable as soon as ANY input is submitted, even before preexec.
    pub(super) fn input(&mut self, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        self.prompt = false;
        let mut previous_cr = false;
        for byte in bytes {
            match *byte {
                b'\r' | b'\n' => {
                    if *byte != b'\n' || !previous_cr {
                        self.submitted += 1;
                    }
                    self.partial = false;
                }
                3 => {
                    self.submitted = 0;
                    self.partial = false;
                }
                _ => self.partial = true,
            }
            previous_cr = *byte == b'\r';
        }
    }

    /// Bounded streaming OSC/CSI scanner; shell-control marks carry a per-spawn
    /// nonce emitted by OUR rc hooks next to OSC 133. Ordinary program OSC 133,
    /// retained backlog, and split/incomplete escape sequences cannot grant readiness.
    pub(super) fn output(&mut self, bytes: &[u8]) {
        self.output.extend_from_slice(bytes);
        let mut pos = 0;
        while pos < self.output.len() {
            if self.output[pos] != 27 {
                pos += 1;
                continue;
            }
            if pos + 1 >= self.output.len() {
                break;
            }
            if self.output[pos + 1] == b']' {
                let start = pos + 2;
                let mut end = start;
                while end < self.output.len()
                    && self.output[end] != 7
                    && !(self.output[end] == 27
                        && self.output.get(end + 1) == Some(&b'\\'))
                {
                    end += 1;
                }
                if end == self.output.len() {
                    break;
                }
                let payload = String::from_utf8_lossy(&self.output[start..end]);
                if let Some(token) = &self.token {
                    let prefix = format!("777;neoism-control;{token};");
                    if let Some(mark) = payload.strip_prefix(&prefix) {
                        match mark {
                            "C" => {
                                self.running = true;
                                self.prompt = false;
                            }
                            "D" => {
                                if self.running {
                                    self.submitted = self.submitted.saturating_sub(1);
                                }
                                self.running = false;
                                self.prompt = false;
                            }
                            "B" => {
                                self.prompt =
                                    !self.running && self.submitted == 0 && !self.partial
                            }
                            _ => {}
                        }
                    }
                }
                pos = end + if self.output[end] == 7 { 1 } else { 2 };
            } else if self.output[pos + 1] == b'[' {
                let start = pos + 2;
                let mut end = start;
                while end < self.output.len()
                    && !(0x40..=0x7e).contains(&self.output[end])
                {
                    end += 1;
                }
                if end == self.output.len() {
                    break;
                }
                let params = &self.output[start..end];
                if [b"?1049".as_slice(), b"?1047", b"?47"].contains(&params) {
                    if self.output[end] == b'h' {
                        self.alternate = true;
                        self.prompt = false;
                    }
                    if self.output[end] == b'l' {
                        self.alternate = false;
                    }
                }
                pos = end + 1;
            } else {
                pos += 2;
            }
        }
        self.output.drain(..pos);
        if self.output.len() > 8192 {
            self.output.clear();
            self.prompt = false;
        }
    }

    pub(super) fn request(
        &mut self,
        session_id: String,
        request_id: String,
        directory: String,
        route: u64,
        tx: mpsc::Sender<ServerMessage>,
    ) -> Vec<ServerMessage> {
        let fail = |message: &str| {
            vec![ServerMessage::TerminalDirectoryChangeFailed {
                session_id: session_id.clone(),
                request_id: request_id.clone(),
                message: message.into(),
            }]
        };
        // Empty directory is a cancellation of this exact request on this exact
        // connection (not a request to cd HOME). In-flight writes cannot be undone;
        // their real cwd result still completes before the client may ungate.
        if directory.is_empty() {
            if self
                .active
                .as_ref()
                .is_some_and(|r| r.route == route && r.request_id == request_id)
            {
                return vec![];
            }
            if self
                .queued
                .as_ref()
                .is_some_and(|r| r.route == route && r.request_id == request_id)
            {
                self.queued.take();
            }
            return fail("Directory request cancelled; no queued shell input was sent");
        }
        if self.token.is_none() || !cfg!(target_os = "linux") {
            return fail("Live directory control requires a daemon-managed integrated bash/zsh shell on Linux");
        }
        if !directory.starts_with('/') || directory.chars().any(char::is_control) {
            return fail(
                "Directory must be an absolute host path without control characters",
            );
        }
        let target = match std::fs::canonicalize(&directory) {
            Ok(path) if path.is_dir() && std::fs::read_dir(&path).is_ok() => path,
            _ => return fail("Host directory does not exist or is inaccessible"),
        };
        if target.to_str().is_none() {
            return fail("Host directory is not UTF-8");
        }
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStrExt;
            let path = std::ffi::CString::new(target.as_os_str().as_bytes()).unwrap();
            if unsafe { libc::access(path.as_ptr(), libc::X_OK) } != 0 {
                return fail("Host directory is not searchable by the shell user");
            }
        }
        if self.partial {
            return fail("Shell has unsubmitted input; finish or cancel that input before changing directory");
        }
        if let Some(old) = self.queued.take() {
            old.fail("Directory request superseded by a newer selection");
        }
        self.queued = Some(Request {
            session_id,
            request_id,
            route,
            tx,
            target,
            since: Instant::now(),
            reported_timeout: false,
        });
        vec![]
    }

    /// Only the reader thread dispatches controls. Foreground identity and cwd
    /// are obtained from the real host PTY/process, never GUI metadata or OSC 7.
    pub(super) fn tick(&mut self, pty: &mut PtySession) {
        if self.queued.is_none() && self.active.is_none() {
            return;
        }
        if self.queued.as_ref().is_some_and(|r| r.tx.is_closed()) {
            self.queued.take();
        }
        // Dropping a reply route can cancel a queue, but cannot retract bytes
        // already written. Keep the host input gate until that shell is safe.
        if let Some(active) = &mut self.active {
            if !active.reported_timeout
                && active.since.elapsed() > Duration::from_secs(15)
            {
                active.reported_timeout = true;
                tracing::warn!(session_id = %active.session_id, request_id = %active.request_id,
                    "still waiting for shell-confirmed cwd; host input remains paused");
            }
        }
        if self
            .queued
            .as_ref()
            .is_some_and(|r| r.since.elapsed() > Duration::from_secs(300))
        {
            self.queued
                .take()
                .unwrap()
                .fail("Directory request expired while waiting for an idle shell");
        }
        if !self.prompt
            || self.alternate
            || self.partial
            || self.submitted != 0
            || !self.foreground_shell(pty)
        {
            return;
        }
        if let Some(active) = self.active.take() {
            #[cfg(target_os = "linux")]
            let cwd = std::fs::read_link(format!("/proc/{}/cwd", pty.shell_pid()));
            #[cfg(not(target_os = "linux"))]
            let cwd: std::io::Result<PathBuf> =
                Err(std::io::ErrorKind::Unsupported.into());
            match cwd {
                Ok(cwd) if cwd == active.target => {
                    let _ = active.tx.try_send(ServerMessage::TerminalDirectoryChanged {
                        session_id: active.session_id, request_id: active.request_id,
                        directory: cwd.to_string_lossy().into_owned(),
                    });
                }
                _ => active.fail("Shell returned to a prompt but its actual cwd does not match the requested directory"),
            }
        }
        if let Some(mut queued) = self.queued.take() {
            // Revalidate at dispatch after arbitrary time spent waiting for jobs.
            if std::fs::canonicalize(&queued.target).ok().as_ref() != Some(&queued.target)
            {
                queued.fail("Host directory changed while waiting for the shell");
                return;
            }
            let literal = queued.target.to_str().unwrap().replace('\'', "'\\''");
            let command = format!(" builtin cd -- '{literal}'\r");
            self.input(command.as_bytes());
            match pty.write(command.as_bytes()) {
                Ok(_) => {
                    queued.since = Instant::now();
                    self.active = Some(queued);
                }
                Err(_) => {
                    tracing::warn!(session_id = %queued.session_id, "directory control write failed; awaiting terminal closure or verified prompt");
                    queued.reported_timeout = true;
                    self.active = Some(queued);
                }
            }
        }
    }

    fn foreground_shell(&self, pty: &PtySession) -> bool {
        #[cfg(target_os = "linux")]
        {
            let pid = pty.shell_pid() as libc::pid_t;
            let fd = *pty.main_fd();
            if pid <= 0
                || fd < 0
                || unsafe { libc::tcgetpgrp(fd) } != pid
                || unsafe { libc::getpgid(pid) } != pid
            {
                return false;
            }
            return std::fs::read_link(format!("/proc/{pid}/exe")).ok().as_ref()
                == self.executable.as_ref();
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = pty;
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn mark(kind: &str) -> Vec<u8> {
        format!("\x1b]777;neoism-control;nonce;{kind}\x07").into_bytes()
    }
    fn state() -> DirectoryControl {
        DirectoryControl::new(Some("nonce".into()), None)
    }

    #[test]
    fn every_submission_closes_stale_prompt_until_its_own_command_roundtrip() {
        let mut s = state();
        s.output(&mark("D"));
        s.output(&mark("B"));
        assert!(s.prompt);
        s.input(b"sleep 1\r");
        s.output(&mark("B"));
        assert!(!s.prompt, "old queued prompt must not revive readiness");
        s.output(&mark("C"));
        s.input(b"pwd\r");
        s.output(&mark("D"));
        s.output(&mark("B"));
        assert!(
            !s.prompt,
            "first completion must not acknowledge second submission"
        );
        s.output(&mark("C"));
        s.output(&mark("D"));
        s.output(&mark("B"));
        assert!(s.prompt);
        s.input(b"unsent draft");
        s.output(&mark("B"));
        assert!(!s.prompt);
    }

    #[test]
    fn untrusted_marks_split_sequences_and_alternate_screen_fail_closed() {
        let mut s = state();
        s.output(b"\x1b]133;B\x07\x1b]777;neoism-control;wrong;B\x07");
        assert!(!s.prompt);
        let b = mark("B");
        for byte in b {
            s.output(&[byte]);
        }
        assert!(s.prompt);
        s.output(b"\x1b[?1049h");
        assert!(s.alternate);
        assert!(!s.prompt);
        s.output(b"\x1b[?1049l");
        assert!(!s.prompt);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn queued_cancellation_and_supersession_are_request_and_route_scoped() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().to_str().unwrap().to_owned();
        let (tx, mut rx) = mpsc::channel(16);
        let mut s = state();
        assert!(s
            .request("pty".into(), "A".into(), target.clone(), 1, tx.clone())
            .is_empty());
        assert!(s
            .request("pty".into(), "B".into(), target.clone(), 1, tx.clone())
            .is_empty());
        assert!(
            matches!(rx.try_recv().unwrap(), ServerMessage::TerminalDirectoryChangeFailed { request_id, .. } if request_id == "A")
        );
        s.request("pty".into(), "A".into(), String::new(), 1, tx.clone());
        assert_eq!(s.queued.as_ref().unwrap().request_id, "B");
        s.request("pty".into(), "B".into(), String::new(), 2, tx.clone());
        assert!(
            s.blocks_input(),
            "peer cancellation is not an owner-route cancellation"
        );
        s.request("pty".into(), "B".into(), String::new(), 1, tx);
        assert!(!s.blocks_input());
    }
}

impl Drop for DirectoryControl {
    fn drop(&mut self) {
        if let Some(r) = self.queued.take() {
            r.fail("Terminal closed before the directory change");
        }
        if let Some(r) = self.active.take() {
            r.fail("Terminal closed before cwd confirmation");
        }
    }
}
