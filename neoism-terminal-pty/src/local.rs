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

use std::io::{ErrorKind, Read as _, Write as _};
use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::sync::Arc;
use std::thread::{Builder, JoinHandle};

use corcovado::channel::{self, Receiver, Sender};
#[cfg(unix)]
use corcovado::unix::UnixReady;
use corcovado::{Events, PollOpt, Ready};
use teletypewriter::{EventedPty, ProcessReadWrite, WinsizeBuilder};
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
        let working_dir = config
            .cwd
            .as_ref()
            .map(|p| p.to_string_lossy().into_owned());

        let pty = teletypewriter::create_pty_with_spawn(
            &Cow::Borrowed(shell.as_str()),
            config.args.clone(),
            &working_dir,
            config.cols,
            config.rows,
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
        let worker = Builder::new()
            .name("neoism-pty-io".to_string())
            .spawn(move || {
                if let Err(err) =
                    reader_loop(pty, cmd_rx, byte_tx, child_event_tx, worker_exit)
                {
                    error!(target: "neoism_terminal_pty", "PTY reader loop terminated: {err}");
                }
            })
            .map_err(|e| PtySessionError::Spawn(format!("worker spawn failed: {e}")))?;

        Ok(Self {
            cmd_tx,
            byte_rx: Some(byte_rx),
            child_event_rx: Some(child_event_rx),
            exit_status,
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
            .map_err(|_| {
                std::io::Error::new(ErrorKind::BrokenPipe, "PTY worker is gone")
            })?;
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
            .map_err(|_| {
                std::io::Error::new(ErrorKind::BrokenPipe, "PTY worker is gone")
            })?;

        completed.recv().map_err(|_| {
            std::io::Error::new(ErrorKind::BrokenPipe, "PTY worker dropped reply write")
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
        self.cmd_tx.send(Command::Resize(ws)).map_err(|_| {
            std::io::Error::new(ErrorKind::BrokenPipe, "PTY worker is gone")
        })?;
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
                    Err(std::io::Error::new(
                        ErrorKind::UnexpectedEof,
                        "PTY worker disconnected",
                    ))
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

fn reader_loop(
    mut pty: teletypewriter::Pty,
    cmd_rx: Receiver<Command>,
    byte_tx: Sender<Vec<u8>>,
    child_event_tx: Sender<i32>,
    exit_status: Arc<AtomicI32>,
) -> std::io::Result<()> {
    reader_loop_impl(&mut pty, cmd_rx, byte_tx, child_event_tx, exit_status)
}

fn reader_loop_impl(
    pty: &mut teletypewriter::Pty,
    cmd_rx: Receiver<Command>,
    byte_tx: Sender<Vec<u8>>,
    child_event_tx: Sender<i32>,
    exit_status: Arc<AtomicI32>,
) -> std::io::Result<()> {
    let poll = corcovado::Poll::new()?;
    let mut tokens = (0..).map(Into::into);
    let poll_opts = PollOpt::edge() | PollOpt::oneshot();

    let cmd_token = tokens.next().unwrap();
    poll.register(&cmd_rx, cmd_token, Ready::readable(), poll_opts)?;

    pty.register(&poll, &mut tokens, Ready::readable(), poll_opts)?;

    let mut events = Events::with_capacity(1024);
    let mut buf = vec![0u8; READ_BUFFER_SIZE];
    let mut shutting_down = false;
    struct PendingWrite {
        bytes: Vec<u8>,
        offset: usize,
        completion: Option<SyncSender<std::io::Result<usize>>>,
    }

    let mut pending_writes = std::collections::VecDeque::<PendingWrite>::new();
    let mut current_write: Option<PendingWrite> = None;

    'event_loop: loop {
        events.clear();
        if let Err(err) = poll.poll(&mut events, None) {
            match err.kind() {
                ErrorKind::Interrupted => continue,
                _ => {
                    error!(target: "neoism_terminal_pty", "poll error: {err}");
                    break 'event_loop;
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
                    let _ = pty.set_winsize(ws);
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
            break 'event_loop;
        }

        for event in events.iter() {
            let token = event.token();
            if token == cmd_token {
                let _ = poll.reregister(&cmd_rx, cmd_token, Ready::readable(), poll_opts);
                continue;
            }

            if token == pty.child_event_token() {
                if let Some(teletypewriter::ChildEvent::Exited) = pty.next_child_event() {
                    let status = child_exit_status(pty).unwrap_or(0);
                    exit_status.store(status, Ordering::SeqCst);
                    let _ = child_event_tx.send(status);
                    break 'event_loop;
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
                                    break 'event_loop;
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
                                    error!(
                                        target: "neoism_terminal_pty",
                                        "PTY read error: {err}"
                                    );
                                    break 'event_loop;
                                }
                            },
                        }
                    }
                }

                if event.readiness().is_writable() {
                    'write_loop: loop {
                        if current_write.is_none() {
                            current_write = pending_writes.pop_front();
                        }
                        let Some(write) = current_write.as_mut() else {
                            break 'write_loop;
                        };
                        match pty.writer().write(&write.bytes[write.offset..]) {
                            Ok(0) => break 'write_loop,
                            Ok(n) => {
                                write.offset += n;
                                if write.offset >= write.bytes.len() {
                                    if let Some(completion) = write.completion.take() {
                                        let _ = completion.send(Ok(write.bytes.len()));
                                    }
                                    current_write = None;
                                }
                            }
                            Err(err) => match err.kind() {
                                ErrorKind::Interrupted => continue,
                                ErrorKind::WouldBlock => break 'write_loop,
                                _ => {
                                    if let Some(completion) = write.completion.take() {
                                        let _ =
                                            completion.send(Err(std::io::Error::new(
                                                err.kind(),
                                                err.to_string(),
                                            )));
                                    }
                                    error!(
                                        target: "neoism_terminal_pty",
                                        "PTY write error: {err}"
                                    );
                                    break 'event_loop;
                                }
                            },
                        }
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
            error!(target: "neoism_terminal_pty", "PTY reregister failed: {err}");
            break 'event_loop;
        }
    }

    if let Some(mut write) = current_write {
        if let Some(completion) = write.completion.take() {
            let _ = completion.send(Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "PTY closed before reply was written",
            )));
        }
    }
    for mut write in pending_writes {
        if let Some(completion) = write.completion.take() {
            let _ = completion.send(Err(std::io::Error::new(
                ErrorKind::BrokenPipe,
                "PTY closed before reply was written",
            )));
        }
    }

    let _ = poll.deregister(&cmd_rx);
    let _ = pty.deregister(&poll);
    Ok(())
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
    pty.child.waitpid().ok().flatten()
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
