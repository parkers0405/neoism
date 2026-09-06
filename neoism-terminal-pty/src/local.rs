//! Native PTY worker.
//!
//! `LocalPty` is the only place in the codebase (post Phase 4) that
//! opens, drives, or tears down a `teletypewriter::Pty`. It owns the
//! corcovado/mio event loop that used to live inside
//! `neoism_backend::performer::Machine::spawn`:
//!
//! * Reads bytes off the PTY master and pushes them to a
//!   [`corcovado::channel::Sender<Vec<u8>>`] for the parser driver.
//! * Watches for `ChildEvent::Exited`, records the raw waitpid status
//!   (Unix) or process exit code (Windows) in a shared atomic, and notifies
//!   via a [`corcovado::channel::Sender<i32>`].
//! * Receives [`Command`]s (write / resize / shutdown) from the public
//!   `PtySession` handle and applies them to the PTY.
//!
//! The native frontend takes the byte / child-event receivers out of
//! the session at construction time and registers them with its own
//! `corcovado::Poll` — that way the parser-driver loop in
//! `neoism-backend::performer` can multiplex PTY bytes alongside the
//! frontend's `Msg` channel without ever touching the PTY fd.
//!
//! For non-native callers (workspace daemon, integration tests) the
//! receivers stay inside `LocalPty` and the synchronous
//! [`PtySession::read`] / [`PtySession::write`] API drains them.

use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read as _};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::{Builder, JoinHandle};

use corcovado::channel::{self, Receiver, Sender};
#[cfg(unix)]
use corcovado::unix::UnixReady;
use corcovado::{Events, PollOpt, Ready};
#[cfg(test)]
use teletypewriter::ProcessReadWrite;
use teletypewriter::{EventedPty, WinsizeBuilder};
use tracing::{error, trace};

use crate::session::{PtySessionConfig, PtySessionError};

/// Sentinel that tells `exit_code()` "child is still running."
const EXIT_RUNNING: i32 = i32::MIN;

const READ_BUFFER_SIZE: usize = 0x10_0000;

/// Control messages from the public [`PtySession`](crate::PtySession)
/// handle to the background reader thread.
enum Command {
    Write(Vec<u8>),
    WriteReply {
        bytes: Vec<u8>,
        completion: SyncSender<std::io::Result<usize>>,
    },
    Resize(WinsizeBuilder),
    Shutdown,
}

struct PendingWrite {
    bytes: Vec<u8>,
    offset: usize,
    completion: Option<SyncSender<std::io::Result<usize>>>,
}

/// Push as much queued input into the PTY writer as it will accept now.
///
/// This must run immediately after draining `Command::Write`, not only after
/// a later writable event. The Windows ConPTY writer is backed by an in-memory
/// pipe whose readiness can already be `writable` when poll interest is
/// enabled; with edge-triggered polling there may be no new edge, leaving the
/// command (and synchronous terminal-protocol replies) queued forever.
fn flush_pending_writes<W: io::Write>(
    writer: &mut W,
    pending_writes: &mut VecDeque<PendingWrite>,
    current_write: &mut Option<PendingWrite>,
) -> io::Result<()> {
    loop {
        if current_write.is_none() {
            *current_write = pending_writes.pop_front();
        }
        let Some(write) = current_write.as_mut() else {
            return Ok(());
        };
        if write.bytes.is_empty() {
            if let Some(completion) = write.completion.take() {
                let _ = completion.send(Ok(0));
            }
            *current_write = None;
            continue;
        }
        match writer.write(&write.bytes[write.offset..]) {
            // ConPTY's ring-buffer adapter returns zero while full. Keep the
            // pending bytes and retry on the next writable wakeup.
            Ok(0) => return Ok(()),
            Ok(n) => {
                write.offset += n;
                if write.offset >= write.bytes.len() {
                    tracing::debug!(target: "neoism_terminal_pty", byte_len = write.bytes.len(),
                        "PTY writer accepted queued input (not evidence of shell execution)");
                    if let Some(completion) = write.completion.take() {
                        let _ = completion.send(Ok(write.bytes.len()));
                    }
                    *current_write = None;
                }
            }
            Err(err) if err.kind() == ErrorKind::Interrupted => continue,
            Err(err) if err.kind() == ErrorKind::WouldBlock => return Ok(()),
            Err(err) => {
                if let Some(completion) = write.completion.take() {
                    let _ = completion.send(Err(copy_io_error(&err)));
                }
                return Err(err);
            }
        }
    }
}

fn copy_io_error(err: &io::Error) -> io::Error {
    match err.raw_os_error() {
        Some(code) => io::Error::from_raw_os_error(code),
        None => io::Error::new(err.kind(), err.to_string()),
    }
}

/// Native PTY worker. See module docs.
pub struct LocalPty {
    /// Sends control commands into the reader thread.
    cmd_tx: Sender<Command>,
    /// Bytes pulled off the PTY master, in arrival order. `None` once
    /// the native frontend has taken ownership for its own poll.
    byte_rx: Option<Receiver<Vec<u8>>>,
    /// Notifies a single `i32` (raw waitpid status on Unix, exit code
    /// on Windows) when the child process exits. `None` once taken.
    child_event_rx: Option<Receiver<i32>>,
    /// Shared with the reader thread — `EXIT_RUNNING` while the
    /// child is alive, otherwise the raw waitpid status / exit code.
    exit_status: Arc<AtomicI32>,
    worker_error: Arc<Mutex<Option<io::Error>>>,
    /// Child PID (best-effort copy for diagnostics).
    pub(crate) shell_pid: u32,
    /// PTY master fd — exposed as `Arc<i32>` so the frontend can
    /// pass it to `teletypewriter::foreground_process_*` without
    /// breaking the "process introspection" path.
    #[cfg(unix)]
    pub(crate) main_fd: Arc<libc::c_int>,
    /// Spillover for synchronous reads.
    spill: Vec<u8>,
    spill_pos: usize,
    /// Worker thread join handle, kept so we can wait on shutdown.
    worker: Option<JoinHandle<()>>,
}

impl LocalPty {
    /// Spawn the configured shell behind a fresh PTY and start the
    /// background reader thread.
    pub(crate) fn spawn(config: PtySessionConfig) -> Result<Self, PtySessionError> {
        use std::borrow::Cow;

        #[cfg(unix)]
        let shell = config.shell.clone().unwrap_or_else(|| {
            std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
        });
        #[cfg(windows)]
        let shell = config.shell.clone().unwrap_or_else(default_windows_shell);
        #[cfg(windows)]
        let config = {
            let mut config = config;
            // Last native spawn boundary: includes daemon prepared PTYs,
            // desktop sessions and platform-default cmd fallback alike.
            crate::shell_integration::apply_cmd_prompt_env(
                &shell,
                &config.args,
                &mut config.env,
            );
            config
        };
        let working_dir = config
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());

        let pty = teletypewriter::create_pty_with_spawn_env(
            &Cow::Borrowed(shell.as_str()),
            config.args.clone(),
            &working_dir,
            config.cols,
            config.rows,
            &config.env,
        )
        .map_err(|e| PtySessionError::Spawn(format!("{e:?}")))?;

        Self::from_pty(pty)
    }

    #[cfg(unix)]
    fn from_pty(pty: teletypewriter::Pty) -> Result<Self, PtySessionError> {
        let main_fd = pty.child.id.clone();
        let shell_pid = *pty.child.pid as u32;

        Self::from_pty_parts(pty, shell_pid, Some(main_fd))
    }

    #[cfg(windows)]
    fn from_pty(pty: teletypewriter::Pty) -> Result<Self, PtySessionError> {
        let shell_pid = pty.child_pid().map(|pid| pid.get()).unwrap_or(0);

        Self::from_pty_parts(pty, shell_pid)
    }

    fn from_pty_parts(
        pty: teletypewriter::Pty,
        shell_pid: u32,
        #[cfg(unix)] main_fd: Option<Arc<libc::c_int>>,
    ) -> Result<Self, PtySessionError> {
        let (cmd_tx, cmd_rx) = channel::channel::<Command>();
        let (byte_tx, byte_rx) = channel::channel::<Vec<u8>>();
        let (child_event_tx, child_event_rx) = channel::channel::<i32>();
        let exit_status = Arc::new(AtomicI32::new(EXIT_RUNNING));

        let worker_exit = exit_status.clone();
        let worker_error = Arc::new(Mutex::new(None));
        let failure = worker_error.clone();
        let worker = Builder::new()
            .name("neoism-pty-io".to_string())
            .spawn(move || {
                let span = tracing::debug_span!(target: "neoism_terminal_pty", "local_pty_worker", shell_pid);
                let _entered = span.enter();
                run_worker(
                    pty,
                    cmd_rx,
                    byte_tx,
                    child_event_tx,
                    worker_exit,
                    failure,
                    child_exit_status,
                );
            })
            .map_err(|e| PtySessionError::Spawn(format!("worker spawn failed: {e}")))?;

        Ok(Self {
            cmd_tx,
            byte_rx: Some(byte_rx),
            child_event_rx: Some(child_event_rx),
            exit_status,
            worker_error,
            shell_pid,
            #[cfg(unix)]
            main_fd: main_fd.expect("unix PTY main fd is required"),
            spill: Vec::new(),
            spill_pos: 0,
            worker: Some(worker),
        })
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.cmd_tx
            .send(Command::Write(bytes.to_vec()))
            .map_err(|_| self.disconnected_error())?;
        tracing::debug!(target: "neoism_terminal_pty", shell_pid = self.shell_pid, byte_len = bytes.len(),
            "input queued for PTY worker (not yet accepted by PTY writer)");
        Ok(bytes.len())
    }

    pub(crate) fn write_reply(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        let len = bytes.len();
        let (completion, completed) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::WriteReply {
                bytes: bytes.to_vec(),
                completion,
            })
            .map_err(|_| self.disconnected_error())?;

        completed
            .recv_timeout(std::time::Duration::from_secs(5))
            .map_err(|error| match error {
                RecvTimeoutError::Timeout => std::io::Error::new(
                    ErrorKind::TimedOut,
                    "timed out waiting for PTY reply write",
                ),
                RecvTimeoutError::Disconnected => self.disconnected_error(),
            })??;
        Ok(len)
    }

    pub(crate) fn resize(&mut self, cols: u16, rows: u16) -> std::io::Result<()> {
        let ws = WinsizeBuilder {
            rows,
            cols,
            width: 0,
            height: 0,
        };
        self.cmd_tx
            .send(Command::Resize(ws))
            .map_err(|_| self.disconnected_error())?;
        Ok(())
    }

    pub(crate) fn read(&mut self, dst: &mut [u8]) -> std::io::Result<usize> {
        if dst.is_empty() {
            return Ok(0);
        }
        // Drain whatever's already in the spill buffer first.
        if self.spill_pos < self.spill.len() {
            let n = (self.spill.len() - self.spill_pos).min(dst.len());
            dst[..n].copy_from_slice(&self.spill[self.spill_pos..self.spill_pos + n]);
            self.spill_pos += n;
            if self.spill_pos >= self.spill.len() {
                self.spill.clear();
                self.spill_pos = 0;
            }
            return Ok(n);
        }

        let Some(rx) = self.byte_rx.as_ref() else {
            return Err(std::io::Error::new(
                ErrorKind::Other,
                "PtySession::read called after the byte receiver was taken",
            ));
        };

        match rx.try_recv() {
            Ok(chunk) => {
                let n = chunk.len().min(dst.len());
                dst[..n].copy_from_slice(&chunk[..n]);
                if n < chunk.len() {
                    self.spill = chunk;
                    self.spill_pos = n;
                }
                Ok(n)
            }
            Err(TryRecvError::Empty) => Err(std::io::Error::new(
                ErrorKind::WouldBlock,
                "no PTY bytes buffered",
            )),
            Err(TryRecvError::Disconnected) => {
                if self.exit_code().is_some() {
                    Ok(0)
                } else {
                    Err(self.disconnected_error())
                }
            }
        }
    }

    pub(crate) fn close(mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(handle) = self.worker.take() {
            // Best effort — don't hang on a stuck worker thread.
            let _ = handle.join();
        }
    }

    pub(crate) fn worker_error(&self) -> Option<io::Error> {
        self.worker_error
            .lock()
            .unwrap()
            .as_ref()
            .map(copy_io_error)
    }

    fn disconnected_error(&self) -> io::Error {
        self.worker_error().unwrap_or_else(|| {
            io::Error::new(
                ErrorKind::BrokenPipe,
                "PTY worker disconnected without child exit",
            )
        })
    }

    pub(crate) fn exit_code(&self) -> Option<i32> {
        let raw = self.exit_status.load(Ordering::SeqCst);
        if raw == EXIT_RUNNING {
            None
        } else {
            Some(raw)
        }
    }

    /// Native-frontend hook: take the byte receiver so the existing
    /// performer-thread `corcovado::Poll` can register it directly.
    /// After this call, [`Self::read`] will refuse to operate.
    pub fn take_byte_receiver(&mut self) -> Option<Receiver<Vec<u8>>> {
        self.byte_rx.take()
    }

    /// Native-frontend hook: take the child-event receiver so the
    /// performer thread can poll on it.
    pub fn take_child_event_receiver(&mut self) -> Option<Receiver<i32>> {
        self.child_event_rx.take()
    }

    /// PTY master fd (Unix only). Used by `foreground_process_*`.
    #[cfg(unix)]
    pub fn main_fd(&self) -> Arc<libc::c_int> {
        self.main_fd.clone()
    }

    /// Child PID. On Windows this is the ConPTY child process id (0 if
    /// it could not be captured at spawn time).
    pub fn shell_pid(&self) -> u32 {
        self.shell_pid
    }
}

impl Drop for LocalPty {
    fn drop(&mut self) {
        // Cmd channel may already be dropped via close(); ignore send
        // errors. The worker will fall through its loop once the cmd
        // channel hangs up.
        let _ = self.cmd_tx.send(Command::Shutdown);
    }
}

fn run_worker<P: EventedPty>(
    mut pty: P,
    cmd_rx: Receiver<Command>,
    byte_tx: Sender<Vec<u8>>,
    child_event_tx: Sender<i32>,
    exit_status: Arc<AtomicI32>,
    failure: Arc<Mutex<Option<io::Error>>>,
    child_status: impl Fn(&P) -> Option<i32>,
) {
    // Publish before dropping either sender: disconnect wakes the consumer.
    if let Err(err) = reader_loop_impl(
        &mut pty,
        &cmd_rx,
        &byte_tx,
        &child_event_tx,
        exit_status,
        child_status,
    ) {
        error!(target: "neoism_terminal_pty", "PTY worker terminated: {err}");
        *failure.lock().unwrap() = Some(err);
    }
}

fn reader_loop_impl<P: EventedPty>(
    pty: &mut P,
    cmd_rx: &Receiver<Command>,
    byte_tx: &Sender<Vec<u8>>,
    child_event_tx: &Sender<i32>,
    exit_status: Arc<AtomicI32>,
    child_status: impl Fn(&P) -> Option<i32>,
) -> std::io::Result<()> {
    let poll = corcovado::Poll::new()?;
    let mut tokens = (0..).map(Into::into);
    let poll_opts = PollOpt::edge() | PollOpt::oneshot();

    let cmd_token = tokens.next().unwrap();
    poll.register(cmd_rx, cmd_token, Ready::readable(), poll_opts)?;

    pty.register(&poll, &mut tokens, Ready::readable(), poll_opts)?;

    let mut events = Events::with_capacity(1024);
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    let mut shutting_down = false;
    let mut pending_writes = VecDeque::<PendingWrite>::new();
    let mut current_write: Option<PendingWrite> = None;

    let result = 'event_loop: loop {
        events.clear();
        if let Err(err) = poll.poll(&mut events, None) {
            match err.kind() {
                ErrorKind::Interrupted => continue,
                _ => {
                    break 'event_loop Err(err);
                }
            }
        }

        // Drain command channel first so writes / resizes affect the
        // current poll iteration.
        loop {
            match cmd_rx.try_recv() {
                Ok(Command::Write(bytes)) => pending_writes.push_back(PendingWrite {
                    bytes,
                    offset: 0,
                    completion: None,
                }),
                Ok(Command::WriteReply { bytes, completion }) => {
                    // Terminal protocol replies must not sit behind queued
                    // keyboard or paste input.
                    pending_writes.push_front(PendingWrite {
                        bytes,
                        offset: 0,
                        completion: Some(completion),
                    });
                }
                Ok(Command::Resize(ws)) => {
                    if let Err(err) = pty.set_winsize(ws) {
                        // A rejected size is not evidence that transport died.
                        tracing::warn!(target: "neoism_terminal_pty", error = %err,
                            "PTY resize rejected; keeping session alive");
                    }
                }
                Ok(Command::Shutdown) => {
                    shutting_down = true;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    shutting_down = true;
                    break;
                }
            }
        }

        if shutting_down {
            break 'event_loop Ok(());
        }

        // A command-channel wake-up proves there is new work, so attempt it
        // now. Waiting exclusively for a separate writable edge loses input
        // on Windows when ConPTY's write pipe was already writable before we
        // enabled that interest.
        if let Err(err) =
            flush_pending_writes(pty.writer(), &mut pending_writes, &mut current_write)
        {
            break 'event_loop Err(err);
        }

        for event in events.iter() {
            let token = event.token();
            if token == cmd_token {
                if let Err(err) =
                    poll.reregister(cmd_rx, cmd_token, Ready::readable(), poll_opts)
                {
                    break 'event_loop Err(err);
                }
                continue;
            }

            if token == pty.child_event_token() {
                if let Some(teletypewriter::ChildEvent::Exited) = pty.next_child_event() {
                    let Some(status) = child_status(pty) else {
                        break 'event_loop Err(io::Error::new(
                            ErrorKind::Other,
                            "child exited but its exit status is unavailable",
                        ));
                    };
                    exit_status.store(status, Ordering::SeqCst);
                    let _ = child_event_tx.send(status);
                    break 'event_loop Ok(());
                }
            }

            if token == pty.read_token() || token == pty.write_token() {
                #[cfg(unix)]
                if UnixReady::from(event.readiness()).is_hup() {
                    continue;
                }

                if event.readiness().is_readable() {
                    'read_loop: loop {
                        match pty.reader().read(&mut buf) {
                            Ok(0) => break 'read_loop,
                            Ok(n) => {
                                trace!(
                                    target: "neoism_terminal_pty",
                                    read_len = n,
                                    "PTY reader chunk"
                                );
                                if byte_tx.send(buf[..n].to_vec()).is_err() {
                                    // Receiver dropped — nobody is
                                    // listening; stop the loop.
                                    break 'event_loop Ok(());
                                }
                                if n < buf.len() {
                                    break 'read_loop;
                                }
                            }
                            Err(err) => match err.kind() {
                                ErrorKind::Interrupted => continue,
                                ErrorKind::WouldBlock => break 'read_loop,
                                _ => {
                                    #[cfg(target_os = "linux")]
                                    if err.raw_os_error() == Some(libc::EIO) {
                                        // Client side hung up; wait
                                        // for the inevitable Exited
                                        // event.
                                        break 'read_loop;
                                    }
                                    break 'event_loop Err(err);
                                }
                            },
                        }
                    }
                }

                if event.readiness().is_writable() {
                    if let Err(err) = flush_pending_writes(
                        pty.writer(),
                        &mut pending_writes,
                        &mut current_write,
                    ) {
                        break 'event_loop Err(err);
                    }
                }
            }
        }

        // Reregister with appropriate interest.
        let mut interest = Ready::readable();
        if current_write.is_some() || !pending_writes.is_empty() {
            interest.insert(Ready::writable());
        }
        if let Err(err) = pty.reregister(&poll, interest, poll_opts) {
            break 'event_loop Err(err);
        }
    };

    for mut write in current_write.into_iter().chain(pending_writes) {
        if let Some(completion) = write.completion.take() {
            let error = result.as_ref().err().map(copy_io_error).unwrap_or_else(|| {
                io::Error::new(
                    ErrorKind::BrokenPipe,
                    "PTY closed before reply was written",
                )
            });
            let _ = completion.send(Err(error));
        }
    }

    let _ = poll.deregister(cmd_rx);
    let _ = pty.deregister(&poll);
    result
}

#[cfg(windows)]
fn child_exit_status(pty: &teletypewriter::Pty) -> Option<i32> {
    pty.child_exit_code()
        .ok()
        .flatten()
        .map(|code| code.min(i32::MAX as u32) as i32)
}

#[cfg(unix)]
fn child_exit_status(pty: &teletypewriter::Pty) -> Option<i32> {
    // next_child_event already performed waitpid; never reap a second time.
    pty.child_exit_status()
}

#[cfg(windows)]
fn default_windows_shell() -> String {
    if command_on_path("pwsh.exe") {
        return "pwsh.exe".to_string();
    }

    if command_on_path("powershell.exe") {
        return "powershell.exe".to_string();
    }

    std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string())
}

#[cfg(windows)]
fn command_on_path(command: &str) -> bool {
    let Some(paths) = std::env::var_os("PATH") else {
        return false;
    };

    std::env::split_paths(&paths).any(|dir| dir.join(command).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    // Exercises the real worker event loop with readiness but no child event.
    // The injected reader/writer returns an OS error, not a synthetic exit.
    struct FailedIo;
    impl io::Read for FailedIo {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(9))
        }
    }
    impl io::Write for FailedIo {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            Err(io::Error::from_raw_os_error(9))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    struct FailedPty {
        io: FailedIo,
        ready: Receiver<()>,
        token: corcovado::Token,
    }
    impl ProcessReadWrite for FailedPty {
        type Reader = FailedIo;
        type Writer = FailedIo;
        fn reader(&mut self) -> &mut FailedIo {
            &mut self.io
        }
        fn writer(&mut self) -> &mut FailedIo {
            &mut self.io
        }
        fn read_token(&self) -> corcovado::Token {
            self.token
        }
        fn write_token(&self) -> corcovado::Token {
            self.token
        }
        fn set_winsize(&mut self, _: WinsizeBuilder) -> io::Result<()> {
            Err(io::Error::new(
                ErrorKind::InvalidInput,
                "test rejected size",
            ))
        }
        fn register(
            &mut self,
            poll: &corcovado::Poll,
            tokens: &mut dyn Iterator<Item = corcovado::Token>,
            _: Ready,
            opts: PollOpt,
        ) -> io::Result<()> {
            self.token = tokens.next().unwrap();
            poll.register(&self.ready, self.token, Ready::readable(), opts)
        }
        fn reregister(
            &mut self,
            poll: &corcovado::Poll,
            _: Ready,
            opts: PollOpt,
        ) -> io::Result<()> {
            poll.reregister(&self.ready, self.token, Ready::readable(), opts)
        }
        fn deregister(&mut self, poll: &corcovado::Poll) -> io::Result<()> {
            poll.deregister(&self.ready)
        }
    }
    impl EventedPty for FailedPty {
        fn child_event_token(&self) -> corcovado::Token {
            corcovado::Token(999)
        }
        fn next_child_event(&mut self) -> Option<teletypewriter::ChildEvent> {
            None
        }
    }

    #[test]
    fn rejected_resize_does_not_terminate_worker() {
        let (cmd_tx, cmd_rx) = channel::channel();
        let (byte_tx, _byte_rx) = channel::channel();
        let (child_tx, _child_rx) = channel::channel();
        let (_ready_tx, ready) = channel::channel();
        cmd_tx
            .send(Command::Resize(WinsizeBuilder {
                cols: 0,
                rows: 0,
                width: 0,
                height: 0,
            }))
            .unwrap();
        cmd_tx.send(Command::Shutdown).unwrap();
        let mut pty = FailedPty {
            io: FailedIo,
            ready,
            token: corcovado::Token(0),
        };
        reader_loop_impl(
            &mut pty,
            &cmd_rx,
            &byte_tx,
            &child_tx,
            Arc::new(AtomicI32::new(EXIT_RUNNING)),
            |_| None,
        )
        .unwrap();
        assert!(
            matches!(cmd_rx.try_recv(), Err(TryRecvError::Empty)),
            "worker must process shutdown after rejected resize"
        );
    }

    #[cfg(unix)]
    #[test]
    fn real_unix_shell_exit_preserves_reaped_status_without_worker_failure() {
        for code in [0, 7] {
            let session = crate::PtySession::spawn(PtySessionConfig {
                shell: Some("/bin/sh".into()),
                args: vec!["-c".into(), format!("exit {code}")],
                ..PtySessionConfig::default()
            })
            .unwrap();
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
            loop {
                assert!(
                    session.worker_error().is_none(),
                    "normal shell exit was reported as worker failure"
                );
                if let Some(status) = session.exit_code() {
                    assert!(libc::WIFEXITED(status));
                    assert_eq!(libc::WEXITSTATUS(status), code);
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "shell exit was not observed"
                );
                std::thread::sleep(std::time::Duration::from_millis(5));
            }
            session.close();
        }
    }

    fn assert_worker_io_failure(write: bool) {
        let (cmd_tx, cmd_rx) = channel::channel();
        let (byte_tx, byte_rx) = channel::channel();
        let (child_tx, child_rx) = channel::channel();
        let consumer_poll = corcovado::Poll::new().unwrap();
        consumer_poll
            .register(
                &child_rx,
                corcovado::Token(0),
                Ready::readable(),
                PollOpt::edge(),
            )
            .unwrap();
        let (ready_tx, ready) = channel::channel();
        let status = Arc::new(AtomicI32::new(EXIT_RUNNING));
        let failure = Arc::new(Mutex::new(None));
        if write {
            cmd_tx
                .send(Command::Write(b"test input\r".to_vec()))
                .unwrap();
        } else {
            ready_tx.send(()).unwrap();
        }
        let worker_status = status.clone();
        let worker_failure = failure.clone();
        let (done_tx, done_rx) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            run_worker(
                FailedPty {
                    io: FailedIo,
                    ready,
                    token: corcovado::Token(0),
                },
                cmd_rx,
                byte_tx,
                child_tx,
                worker_status,
                worker_failure,
                |_| panic!("no child exit should be queried"),
            );
            done_tx.send(()).unwrap();
        });
        done_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("worker did not stop on I/O failure");
        worker.join().unwrap();
        let mut events = Events::with_capacity(8);
        consumer_poll
            .poll(&mut events, Some(std::time::Duration::from_secs(1)))
            .unwrap();
        assert!(
            events
                .iter()
                .any(|event| event.token() == corcovado::Token(0)),
            "worker disconnect must wake the performer"
        );
        consumer_poll.deregister(&child_rx).unwrap();
        assert!(matches!(
            child_rx.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert!(matches!(
            byte_rx.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
        assert_eq!(status.load(Ordering::SeqCst), EXIT_RUNNING);
        let mut session = crate::PtySession {
            inner: crate::session::PtyInner::Local(LocalPty {
                cmd_tx,
                byte_rx: Some(byte_rx),
                child_event_rx: Some(child_rx),
                exit_status: status,
                worker_error: failure.clone(),
                shell_pid: 0,
                #[cfg(unix)]
                main_fd: Arc::new(-1),
                spill: Vec::new(),
                spill_pos: 0,
                worker: None,
            }),
        };
        assert!(session.exit_code().is_none());
        assert_eq!(session.worker_error().unwrap().raw_os_error(), Some(9));
        assert_eq!(
            session.read(&mut [0; 8]).unwrap_err().raw_os_error(),
            Some(9)
        );
        assert_eq!(session.write(b"more").unwrap_err().raw_os_error(), Some(9));
        assert_eq!(
            failure.lock().unwrap().as_ref().unwrap().raw_os_error(),
            Some(9)
        );
    }

    #[test]
    fn zero_write_retains_pending_input_and_empty_input_is_skipped() {
        let mut writer: &mut [u8] = &mut [];
        let mut pending = VecDeque::from([PendingWrite {
            bytes: vec![],
            offset: 0,
            completion: None,
        }]);
        let mut current = None;
        flush_pending_writes(&mut writer, &mut pending, &mut current).unwrap();
        pending.push_back(PendingWrite {
            bytes: vec![1],
            offset: 0,
            completion: None,
        });
        flush_pending_writes(&mut writer, &mut pending, &mut current).unwrap();
        assert_eq!(current.as_ref().unwrap().offset, 0);
        let mut resumed = Vec::new();
        flush_pending_writes(&mut resumed, &mut pending, &mut current).unwrap();
        assert_eq!(resumed, vec![1]);
        assert!(current.is_none());
        assert!(pending.is_empty());
    }

    #[test]
    fn partial_write_then_full_buffer_resumes_without_early_ack() {
        let (completion, completed) = mpsc::sync_channel(1);
        let mut pending = VecDeque::from([
            PendingWrite {
                bytes: b"first\n".to_vec(),
                offset: 0,
                completion: Some(completion),
            },
            PendingWrite {
                bytes: b"second\n".to_vec(),
                offset: 0,
                completion: None,
            },
        ]);
        let mut current = None;
        let mut accepted = [0u8; 3];
        let mut full_writer = &mut accepted[..];
        flush_pending_writes(&mut full_writer, &mut pending, &mut current).unwrap();
        assert_eq!(current.as_ref().unwrap().offset, 3);
        assert!(matches!(completed.try_recv(), Err(TryRecvError::Empty)));
        flush_pending_writes(&mut full_writer, &mut pending, &mut current).unwrap();
        assert_eq!(current.as_ref().unwrap().offset, 3);
        let mut resumed = Vec::new();
        flush_pending_writes(&mut resumed, &mut pending, &mut current).unwrap();
        assert_eq!(
            [accepted.as_slice(), resumed.as_slice()].concat(),
            b"first\nsecond\n"
        );
        assert_eq!(completed.try_recv().unwrap().unwrap(), 6);
        assert!(current.is_none());
        assert!(pending.is_empty());
    }

    #[test]
    fn worker_read_failure_without_child_exit_is_preserved() {
        assert_worker_io_failure(false);
    }

    #[test]
    fn worker_async_write_failure_without_child_exit_is_preserved() {
        assert_worker_io_failure(true);
    }

    #[test]
    fn queued_input_is_written_without_waiting_for_a_writable_event() {
        let mut writer = Vec::new();
        let mut pending = VecDeque::from([PendingWrite {
            bytes: b"neoism update\r".to_vec(),
            offset: 0,
            completion: None,
        }]);
        let mut current = None;

        flush_pending_writes(&mut writer, &mut pending, &mut current).unwrap();

        assert_eq!(writer, b"neoism update\r");
        assert!(pending.is_empty());
        assert!(current.is_none());
    }

    #[test]
    fn immediate_write_completes_synchronous_protocol_reply() {
        let (completion, completed) = mpsc::sync_channel(1);
        let mut writer = Vec::new();
        let mut pending = VecDeque::from([PendingWrite {
            bytes: b"reply".to_vec(),
            offset: 0,
            completion: Some(completion),
        }]);
        let mut current = None;

        flush_pending_writes(&mut writer, &mut pending, &mut current).unwrap();

        assert_eq!(completed.recv().unwrap().unwrap(), 5);
        assert_eq!(writer, b"reply");
    }
}
