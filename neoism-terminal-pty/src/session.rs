//! Public PtySession API.
//!
//! Configuration + handle for a single child process running behind a
//! PTY. The frontend (and, later, the workspace daemon) only ever
//! touches `PtySession` — never `teletypewriter::Pty` directly.

use std::path::PathBuf;

/// Description of the shell + winsize to launch.
#[derive(Debug, Clone)]
pub struct PtySessionConfig {
    /// Path / name of the shell binary. `None` falls back to `$SHELL`
    /// (or the platform default) inside the spawn implementation.
    pub shell: Option<String>,
    /// Argv (excluding argv[0]).
    pub args: Vec<String>,
    /// Working directory for the child. `None` keeps the parent's cwd.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables to set in the child. Pairs of
    /// `(KEY, VALUE)`. Reserved for future use — the current native
    /// spawn path inherits env from the parent process.
    pub env: Vec<(String, String)>,
    /// Initial column count.
    pub cols: u16,
    /// Initial row count.
    pub rows: u16,
}

impl Default for PtySessionConfig {
    fn default() -> Self {
        Self {
            shell: None,
            args: Vec::new(),
            cwd: None,
            env: Vec::new(),
            cols: 80,
            rows: 24,
        }
    }
}

/// Errors surfaced by [`PtySession`].
#[derive(Debug, thiserror::Error)]
pub enum PtySessionError {
    #[error("failed to spawn PTY: {0}")]
    Spawn(String),
    #[error("PTY I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("PTY closed unexpectedly")]
    Closed,
}

/// Handle for a PTY-backed shell.
///
/// Two backings share this one surface:
/// * [`Self::spawn`] — a local child process behind a real PTY, with a
///   background reader thread (the historical path).
/// * [`Self::remote`] — a shell hosted by the workspace daemon; bytes
///   are bridged through a [`crate::RemotePtyFeed`] and control ops
///   flow out through a caller sink. No local process exists.
pub struct PtySession {
    pub(crate) inner: PtyInner,
}

pub(crate) enum PtyInner {
    Local(crate::local::LocalPty),
    Remote(crate::remote::RemotePty),
}

impl PtySession {
    /// Spawn the configured shell behind a fresh PTY.
    pub fn spawn(config: PtySessionConfig) -> Result<Self, PtySessionError> {
        Ok(Self {
            inner: PtyInner::Local(crate::local::LocalPty::spawn(config)?),
        })
    }

    /// Wrap a daemon-hosted shell. `sink` receives every
    /// [`crate::RemotePtyOp`] (input / resize / close) and is expected
    /// to translate them into daemon messages; the returned
    /// [`crate::RemotePtyFeed`] is how the host delivers daemon output
    /// and exit events back in.
    pub fn remote(
        sink: Box<dyn FnMut(crate::RemotePtyOp) + Send>,
    ) -> (Self, crate::RemotePtyFeed) {
        let (remote, feed) = crate::remote::RemotePty::new(sink);
        (
            Self {
                inner: PtyInner::Remote(remote),
            },
            feed,
        )
    }

    /// Write `bytes` to the PTY master (or forward them to the daemon).
    ///
    /// Returns the number of bytes accepted by the underlying writer
    /// (which may be less than `bytes.len()` for non-blocking modes).
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.write(bytes),
            PtyInner::Remote(pty) => pty.write(bytes),
        }
    }

    /// Write a terminal protocol reply before returning. Local sessions
    /// acknowledge only after the PTY master accepted every byte.
    pub fn write_reply(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.write_reply(bytes),
            PtyInner::Remote(pty) => pty.write(bytes),
        }
    }

    /// Set the PTY's window size (cols × rows).
    pub fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.resize(cols, rows),
            PtyInner::Remote(pty) => pty.resize(cols, rows),
        }
    }

    /// Pull bytes that the background reader thread has buffered.
    ///
    /// Returns `Ok(0)` together with [`std::io::ErrorKind::WouldBlock`]
    /// semantics when no bytes are currently available — except that
    /// the `Ok(0)` case here means "no bytes, try again," not "EOF".
    /// A clean EOF on the PTY surfaces via [`PtySession::exit_code`].
    /// Remote sessions never support synchronous reads — their byte
    /// stream only exists on the channel taken by
    /// [`Self::take_byte_receiver`].
    pub fn read(&mut self, dst: &mut [u8]) -> std::io::Result<usize> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.read(dst),
            PtyInner::Remote(_) => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "remote PTY sessions are channel-only; use take_byte_receiver",
            )),
        }
    }

    /// Shut down the backing (reader thread + PTY, or notify the
    /// daemon the client is done with the session).
    pub fn close(self) {
        match self.inner {
            PtyInner::Local(pty) => pty.close(),
            PtyInner::Remote(pty) => pty.close(),
        }
    }

    /// `Some(status)` if the child has exited, `None` while it is
    /// still running.
    pub fn exit_code(&self) -> Option<i32> {
        match &self.inner {
            PtyInner::Local(pty) => pty.exit_code(),
            PtyInner::Remote(pty) => pty.exit_code(),
        }
    }

    /// Native-frontend hook: take ownership of the byte channel so the
    /// parser-driver thread can register it on its own corcovado
    /// `Poll`. After this call, [`PtySession::read`] will refuse to
    /// operate.
    pub fn take_byte_receiver(
        &mut self,
    ) -> Option<corcovado::channel::Receiver<Vec<u8>>> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.take_byte_receiver(),
            PtyInner::Remote(pty) => pty.take_byte_receiver(),
        }
    }

    /// Native-frontend hook: take ownership of the child-event
    /// channel.
    pub fn take_child_event_receiver(
        &mut self,
    ) -> Option<corcovado::channel::Receiver<i32>> {
        match &mut self.inner {
            PtyInner::Local(pty) => pty.take_child_event_receiver(),
            PtyInner::Remote(pty) => pty.take_child_event_receiver(),
        }
    }

    /// PTY master fd (Unix only). Exposed so callers can keep using
    /// `teletypewriter::foreground_process_*` for process
    /// introspection without holding the raw `teletypewriter::Pty`.
    /// Remote sessions report `-1` — the introspection helpers treat
    /// an invalid fd as "no info available".
    #[cfg(unix)]
    pub fn main_fd(&self) -> std::sync::Arc<libc::c_int> {
        match &self.inner {
            PtyInner::Local(pty) => pty.main_fd(),
            PtyInner::Remote(pty) => pty.main_fd.clone(),
        }
    }

    /// PID of the child shell. Remote sessions report `0` (the process
    /// lives in the daemon, possibly on another machine).
    pub fn shell_pid(&self) -> u32 {
        match &self.inner {
            PtyInner::Local(pty) => pty.shell_pid(),
            PtyInner::Remote(_) => 0,
        }
    }
}
