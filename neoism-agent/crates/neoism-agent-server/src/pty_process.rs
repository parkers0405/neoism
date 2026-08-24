use std::collections::HashMap;
use std::io;
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::Stdio;
#[cfg(unix)]
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use neoism_agent_core::PtyInfo;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
#[cfg(not(unix))]
use tokio::process::ChildStdin;
use tokio::process::{Child, Command};
use tokio::sync::{broadcast, Mutex};
use tracing::warn;

use super::pty_buffer::{PtyOutputBuffer, PtyOutputEvent};
use super::{PtyError, PtySize};

pub(crate) async fn stop_pty_process(pty_id: &str) {
    process_registry().stop(pty_id).await;
}

pub(crate) async fn resize_pty_process(pty_id: &str, size: PtySize) {
    process_registry().resize(pty_id, size).await;
}

pub(crate) async fn stop_all_pty_processes() {
    process_registry().stop_all().await;
}

pub(crate) async fn serve_websocket(
    info: PtyInfo,
    cursor: Option<i64>,
    mut socket: WebSocket,
    on_exit: impl Fn(String, Option<i32>) + Send + Sync + 'static,
) {
    let process = match process_registry()
        .get_or_spawn(info.clone(), Arc::new(on_exit))
        .await
    {
        Ok(process) => process,
        Err(error) => {
            let _ = socket
                .send(Message::Text(format!(
                    "failed to start PTY process: {error:?}"
                )))
                .await;
            let _ = socket.send(Message::Close(None)).await;
            return;
        }
    };

    let mut output_cursor = if cursor.unwrap_or_default() >= 0 {
        cursor.unwrap_or_default() as u64
    } else {
        process.buffer.lock().await.cursor()
    };

    if cursor.unwrap_or_default() >= 0 {
        let replay = process.buffer.lock().await.replay_from(output_cursor);
        for chunk in replay {
            if send_output(&mut socket, &chunk.data, chunk.cursor)
                .await
                .is_err()
            {
                return;
            }
            output_cursor = chunk.cursor;
        }
    }

    if send_cursor(&mut socket, output_cursor).await.is_err() {
        return;
    }

    let mut output = process.output.subscribe();
    if process.exited.load(Ordering::SeqCst) {
        let _ = socket.send(Message::Close(None)).await;
        return;
    }
    loop {
        tokio::select! {
            message = socket.recv() => {
                let Some(message) = message else {
                    break;
                };
                match message {
                    Ok(Message::Text(data)) => {
                        if write_stdin(&process.stdin, data.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Binary(data)) => {
                        if data.first() == Some(&0) {
                            continue;
                        }
                        if write_stdin(&process.stdin, &data).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Ping(data)) => {
                        if socket.send(Message::Pong(data)).await.is_err() {
                            break;
                        }
                    }
                    Ok(Message::Pong(_)) => {}
                    Ok(Message::Close(_)) => break,
                    Err(_) => break,
                }
            }
            event = output.recv() => {
                match event {
                    Ok(PtyOutputEvent::Data(chunk)) => {
                        if chunk.cursor <= output_cursor {
                            continue;
                        }
                        if send_output(&mut socket, &chunk.data, chunk.cursor).await.is_err() {
                            break;
                        }
                        output_cursor = chunk.cursor;
                    }
                    Ok(PtyOutputEvent::Exited) => {
                        let _ = socket.send(Message::Close(None)).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        let replay = process.buffer.lock().await.replay_from(output_cursor);
                        for chunk in replay {
                            if send_output(&mut socket, &chunk.data, chunk.cursor).await.is_err() {
                                return;
                            }
                            output_cursor = chunk.cursor;
                        }
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        }
    }
}

struct PtyProcess {
    stdin: Arc<Mutex<PtyInput>>,
    child: Arc<Mutex<PtyChild>>,
    #[cfg(unix)]
    pgid: Option<libc::pid_t>,
    output: broadcast::Sender<PtyOutputEvent>,
    buffer: Arc<Mutex<PtyOutputBuffer>>,
    exited: AtomicBool,
}

/// The spawned process behind a PTY: a tokio child for the unix-PTY and
/// pipe paths, raw Win32 handles for the ConPTY path (tokio's `Command`
/// cannot carry the pseudoconsole proc-thread attribute).
enum PtyChild {
    Spawned(Child),
    #[cfg(windows)]
    Conpty(conpty::ConptyChild),
}

impl PtyChild {
    /// `Ok(Some(code))` once the process has exited (`code` is `None`
    /// when the platform reports no exit code, e.g. signal death).
    fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
        match self {
            PtyChild::Spawned(child) => Ok(child.try_wait()?.map(|status| status.code())),
            #[cfg(windows)]
            PtyChild::Conpty(child) => child.try_wait(),
        }
    }

    fn start_kill(&mut self) {
        match self {
            PtyChild::Spawned(child) => {
                let _ = child.start_kill();
            }
            #[cfg(windows)]
            PtyChild::Conpty(child) => child.kill(),
        }
    }

    /// Called once the monitor observed the exit. The ConPTY host keeps
    /// the output pipe's write end open until the pseudoconsole is
    /// closed, so without this the reader task never sees EOF.
    fn on_reaped(&mut self) {
        match self {
            PtyChild::Spawned(_) => {}
            #[cfg(windows)]
            PtyChild::Conpty(child) => child.close_console(),
        }
    }
}

enum PtyInput {
    #[cfg(not(unix))]
    Pipe(ChildStdin),
    #[cfg(any(unix, windows))]
    Pty(tokio::fs::File),
}

#[derive(Default)]
struct PtyProcessRegistry {
    processes: Mutex<HashMap<String, Arc<PtyProcess>>>,
}

impl PtyProcessRegistry {
    async fn get_or_spawn(
        &'static self,
        info: PtyInfo,
        on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
    ) -> Result<Arc<PtyProcess>, PtyError> {
        let mut processes = self.processes.lock().await;
        if let Some(process) = processes.get(&info.id) {
            return Ok(process.clone());
        }

        let process = spawn_process(info.clone(), on_exit)?;
        processes.insert(info.id, process.clone());
        Ok(process)
    }

    async fn remove_if_same(&self, pty_id: &str, process: &Arc<PtyProcess>) {
        let mut processes = self.processes.lock().await;
        if processes
            .get(pty_id)
            .is_some_and(|current| Arc::ptr_eq(current, process))
        {
            processes.remove(pty_id);
        }
    }

    async fn stop(&self, pty_id: &str) {
        let process = self.processes.lock().await.remove(pty_id);
        if let Some(process) = process {
            stop_process(process).await;
        }
    }

    async fn resize(&self, pty_id: &str, size: PtySize) {
        let process = self.processes.lock().await.get(pty_id).cloned();
        if let Some(process) = process {
            let _ = resize_process(process, size).await;
        }
    }

    async fn stop_all(&self) {
        let processes = self.processes.lock().await.drain().collect::<Vec<_>>();
        for (_, process) in processes {
            stop_process(process).await;
        }
    }
}

fn process_registry() -> &'static PtyProcessRegistry {
    static REGISTRY: OnceLock<PtyProcessRegistry> = OnceLock::new();
    REGISTRY.get_or_init(PtyProcessRegistry::default)
}

async fn stop_process(process: Arc<PtyProcess>) {
    process.exited.store(true, Ordering::SeqCst);
    #[cfg(unix)]
    {
        signal_process_group(&process, libc::SIGHUP);
        signal_process_group(&process, libc::SIGTERM);
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut child = process.child.lock().await;
    child.start_kill();
    let _ = process.output.send(PtyOutputEvent::Exited);
}

#[cfg(unix)]
fn signal_process_group(process: &PtyProcess, signal: libc::c_int) {
    if let Some(pgid) = process.pgid {
        let rc = unsafe { libc::kill(-pgid, signal) };
        if rc < 0 {
            let error = io::Error::last_os_error();
            if error.raw_os_error() != Some(libc::ESRCH) {
                warn!(pgid, signal, error = %error, "failed to signal PTY process group");
            }
        }
    }
}

#[cfg(unix)]
fn spawn_process(
    info: PtyInfo,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) -> Result<Arc<PtyProcess>, PtyError> {
    spawn_pty_process(info, on_exit)
}

#[cfg(not(unix))]
fn spawn_process(
    info: PtyInfo,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) -> Result<Arc<PtyProcess>, PtyError> {
    // Prefer a real pseudoconsole; the pipe fallback (no TTY, no
    // resize, no ANSI) only remains for pre-1809 conhosts where
    // `CreatePseudoConsole` is unavailable.
    #[cfg(windows)]
    match spawn_conpty_process(info.clone(), on_exit.clone()) {
        Ok(process) => return Ok(process),
        Err(error) => {
            warn!(pty_id = %info.id, ?error, "ConPTY spawn failed; falling back to pipe process");
        }
    }
    spawn_pipe_process(info, on_exit)
}

#[cfg(not(unix))]
fn spawn_pipe_process(
    info: PtyInfo,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) -> Result<Arc<PtyProcess>, PtyError> {
    let command = info.command.first().ok_or_else(|| {
        PtyError::SpawnFailed(
            "PTY command must contain at least one argument".to_string(),
        )
    })?;
    let mut process = Command::new(command);
    process
        .args(info.command.iter().skip(1))
        .current_dir(&info.cwd)
        .env("TERM", "xterm-256color")
        .env("NEOISM_TERMINAL", "1")
        .kill_on_drop(true)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    crate::tool::process::set_new_process_group(&mut process);

    let mut child = process
        .spawn()
        .map_err(|error| PtyError::SpawnFailed(error.to_string()))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| PtyError::Io("failed to open process stdin".to_string()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| PtyError::Io("failed to open process stdout".to_string()))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| PtyError::Io("failed to open process stderr".to_string()))?;

    let (output, _) = broadcast::channel(1024);
    let buffer = Arc::new(Mutex::new(PtyOutputBuffer::default()));
    let child = Arc::new(Mutex::new(PtyChild::Spawned(child)));
    let pty_id = info.id.clone();
    let process = Arc::new(PtyProcess {
        stdin: Arc::new(Mutex::new(PtyInput::Pipe(stdin))),
        child: child.clone(),
        output: output.clone(),
        buffer: buffer.clone(),
        exited: AtomicBool::new(false),
    });

    tokio::spawn(read_process_output(
        stdout,
        output.clone(),
        buffer.clone(),
        pty_id.clone(),
        "stdout",
    ));
    tokio::spawn(read_process_output(
        stderr,
        output.clone(),
        buffer,
        pty_id.clone(),
        "stderr",
    ));

    monitor_process(child, process.clone(), output, pty_id, on_exit);
    Ok(process)
}

/// ConPTY-backed process: full TTY semantics (isatty, ANSI, resize) via
/// `CreatePseudoConsole`, with the conin/conout pipe ends wrapped in
/// `tokio::fs::File` exactly like the unix PTY master.
#[cfg(windows)]
fn spawn_conpty_process(
    info: PtyInfo,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) -> Result<Arc<PtyProcess>, PtyError> {
    if info.command.is_empty() {
        return Err(PtyError::SpawnFailed(
            "PTY command must contain at least one argument".to_string(),
        ));
    }
    let (conpty_child, reader, writer) = conpty::spawn(
        &info.command,
        &info.cwd,
        &[("TERM", "xterm-256color"), ("NEOISM_TERMINAL", "1")],
        PtySize { cols: 80, rows: 24 },
    )?;

    let (output, _) = broadcast::channel(1024);
    let buffer = Arc::new(Mutex::new(PtyOutputBuffer::default()));
    let child = Arc::new(Mutex::new(PtyChild::Conpty(conpty_child)));
    let pty_id = info.id.clone();
    let process = Arc::new(PtyProcess {
        stdin: Arc::new(Mutex::new(PtyInput::Pty(tokio::fs::File::from_std(writer)))),
        child: child.clone(),
        output: output.clone(),
        buffer: buffer.clone(),
        exited: AtomicBool::new(false),
    });

    tokio::spawn(read_process_output(
        tokio::fs::File::from_std(reader),
        output.clone(),
        buffer,
        pty_id.clone(),
        "conpty",
    ));

    monitor_process(child, process.clone(), output, pty_id, on_exit);
    Ok(process)
}

#[cfg(unix)]
fn spawn_pty_process(
    info: PtyInfo,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) -> Result<Arc<PtyProcess>, PtyError> {
    let command = info.command.first().ok_or_else(|| {
        PtyError::SpawnFailed(
            "PTY command must contain at least one argument".to_string(),
        )
    })?;
    let (master_fd, slave_fd) = open_pty(PtySize { cols: 80, rows: 24 })?;
    let writer_fd = unsafe { libc::dup(master_fd) };
    if writer_fd < 0 {
        unsafe {
            libc::close(master_fd);
            libc::close(slave_fd);
        }
        return Err(PtyError::Io(io::Error::last_os_error().to_string()));
    }
    if let Err(error) = set_cloexec(master_fd).and_then(|_| set_cloexec(writer_fd)) {
        unsafe {
            libc::close(master_fd);
            libc::close(writer_fd);
            libc::close(slave_fd);
        }
        return Err(error);
    }

    let mut process = Command::new(command);
    process
        .args(info.command.iter().skip(1))
        .current_dir(&info.cwd)
        .env("TERM", "xterm-256color")
        .env("NEOISM_TERMINAL", "1")
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    unsafe {
        process.as_std_mut().pre_exec(move || {
            if libc::setsid() < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) < 0 {
                return Err(io::Error::last_os_error());
            }
            if libc::tcsetpgrp(slave_fd, libc::getpid()) < 0 {
                return Err(io::Error::last_os_error());
            }
            for fd in [libc::STDIN_FILENO, libc::STDOUT_FILENO, libc::STDERR_FILENO] {
                if libc::dup2(slave_fd, fd) < 0 {
                    return Err(io::Error::last_os_error());
                }
            }
            if slave_fd > libc::STDERR_FILENO {
                libc::close(slave_fd);
            }
            Ok(())
        });
    }

    let child = match process.spawn() {
        Ok(child) => child,
        Err(error) => {
            unsafe {
                libc::close(master_fd);
                libc::close(writer_fd);
                libc::close(slave_fd);
            }
            return Err(PtyError::SpawnFailed(error.to_string()));
        }
    };
    unsafe {
        libc::close(slave_fd);
    }

    let reader = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let writer = unsafe { std::fs::File::from_raw_fd(writer_fd) };
    let (output, _) = broadcast::channel(1024);
    let buffer = Arc::new(Mutex::new(PtyOutputBuffer::default()));
    let pgid = child.id().map(|pid| pid as libc::pid_t);
    let child = Arc::new(Mutex::new(PtyChild::Spawned(child)));
    let pty_id = info.id.clone();
    let process = Arc::new(PtyProcess {
        stdin: Arc::new(Mutex::new(PtyInput::Pty(tokio::fs::File::from_std(writer)))),
        child: child.clone(),
        pgid,
        output: output.clone(),
        buffer: buffer.clone(),
        exited: AtomicBool::new(false),
    });

    tokio::spawn(read_process_output(
        tokio::fs::File::from_std(reader),
        output.clone(),
        buffer,
        pty_id.clone(),
        "pty",
    ));

    monitor_process(child, process.clone(), output, pty_id, on_exit);
    Ok(process)
}

#[cfg(unix)]
fn open_pty(size: PtySize) -> Result<(libc::c_int, libc::c_int), PtyError> {
    let mut master = 0;
    let mut slave = 0;
    let mut winsize = libc::winsize {
        ws_row: size.rows,
        ws_col: size.cols,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let rc = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut winsize,
        )
    };
    if rc < 0 {
        return Err(PtyError::Io(io::Error::last_os_error().to_string()));
    }
    Ok((master, slave))
}

#[cfg(unix)]
fn set_cloexec(fd: libc::c_int) -> Result<(), PtyError> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 {
        return Err(PtyError::Io(io::Error::last_os_error().to_string()));
    }
    let rc = unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) };
    if rc < 0 {
        return Err(PtyError::Io(io::Error::last_os_error().to_string()));
    }
    Ok(())
}

fn monitor_process(
    child: Arc<Mutex<PtyChild>>,
    process: Arc<PtyProcess>,
    output: broadcast::Sender<PtyOutputEvent>,
    pty_id: String,
    on_exit: Arc<dyn Fn(String, Option<i32>) + Send + Sync>,
) {
    tokio::spawn(async move {
        let mut code = None;
        loop {
            tokio::time::sleep(Duration::from_millis(250)).await;
            let status = {
                let mut child = child.lock().await;
                child.try_wait()
            };
            match status {
                Ok(Some(exit_code)) => {
                    code = exit_code;
                    break;
                }
                Ok(None) => continue,
                Err(error) => {
                    warn!(pty_id = %pty_id, error = %error, "failed to poll PTY process");
                    break;
                }
            }
        }
        child.lock().await.on_reaped();
        let already_exited = process.exited.swap(true, Ordering::SeqCst);
        let _ = output.send(PtyOutputEvent::Exited);
        process_registry().remove_if_same(&pty_id, &process).await;
        if !already_exited {
            on_exit(pty_id, code);
        }
    });
}

async fn read_process_output<R>(
    mut reader: R,
    output: broadcast::Sender<PtyOutputEvent>,
    buffer: Arc<Mutex<PtyOutputBuffer>>,
    pty_id: String,
    stream: &'static str,
) where
    R: AsyncRead + Unpin + Send + 'static,
{
    let mut bytes = [0; 8192];
    loop {
        match reader.read(&mut bytes).await {
            Ok(0) => break,
            Ok(n) => {
                let data = String::from_utf8_lossy(&bytes[..n]).to_string();
                let chunk = buffer.lock().await.push(data);
                let _ = output.send(PtyOutputEvent::Data(chunk));
            }
            // ConPTY teardown surfaces as ERROR_BROKEN_PIPE rather than
            // a clean zero-read; that's an EOF, not a failure.
            Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
            Err(error) => {
                warn!(pty_id = %pty_id, stream, error = %error, "failed to read PTY process output");
                break;
            }
        }
    }
}

async fn resize_process(process: Arc<PtyProcess>, size: PtySize) -> Result<(), PtyError> {
    #[cfg(unix)]
    {
        let input = process.stdin.lock().await;
        let PtyInput::Pty(file) = &*input;
        let mut winsize = libc::winsize {
            ws_row: size.rows,
            ws_col: size.cols,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        let rc = unsafe {
            libc::ioctl(
                file.as_raw_fd(),
                libc::TIOCSWINSZ as libc::c_ulong,
                &mut winsize,
            )
        };
        if rc < 0 {
            return Err(PtyError::Io(io::Error::last_os_error().to_string()));
        }
        signal_process_group(&process, libc::SIGWINCH);
    }
    #[cfg(windows)]
    {
        let child = process.child.lock().await;
        if let PtyChild::Conpty(conpty) = &*child {
            conpty.resize(size)?;
        }
        // Pipe-fallback processes have no console to resize.
    }
    #[cfg(all(not(unix), not(windows)))]
    {
        let _ = (process, size);
    }
    Ok(())
}

/// Raw ConPTY plumbing: `CreatePseudoConsole` + `CreateProcessW` with the
/// pseudoconsole proc-thread attribute, synchronous anonymous pipes for
/// conin/conout. Kept self-contained so the rest of the module only deals
/// in `std::fs::File` + `ConptyChild`.
#[cfg(windows)]
mod conpty {
    use std::io;
    use std::iter::once;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::FromRawHandle;
    use std::ptr;
    use std::sync::atomic::{AtomicBool, Ordering};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, STILL_ACTIVE, S_OK};
    use windows_sys::Win32::System::Console::{
        ClosePseudoConsole, CreatePseudoConsole, ResizePseudoConsole, COORD, HPCON,
    };
    use windows_sys::Win32::System::Pipes::CreatePipe;
    use windows_sys::Win32::System::Threading::{
        CreateProcessW, DeleteProcThreadAttributeList, GetExitCodeProcess,
        InitializeProcThreadAttributeList, TerminateProcess, UpdateProcThreadAttribute,
        CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, EXTENDED_STARTUPINFO_PRESENT,
        LPPROC_THREAD_ATTRIBUTE_LIST, PROCESS_INFORMATION,
        PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE, STARTF_USESTDHANDLES, STARTUPINFOEXW,
    };

    use super::{PtyError, PtySize};

    pub(super) struct ConptyChild {
        hpc: HPCON,
        process: HANDLE,
        pid: u32,
        console_closed: AtomicBool,
    }

    // Raw handles are plain kernel object references; every operation we
    // perform on them is documented thread-safe.
    unsafe impl Send for ConptyChild {}
    unsafe impl Sync for ConptyChild {}

    impl ConptyChild {
        pub(super) fn try_wait(&mut self) -> io::Result<Option<Option<i32>>> {
            let mut code: u32 = 0;
            let ok = unsafe { GetExitCodeProcess(self.process, &mut code) };
            if ok == 0 {
                return Err(io::Error::last_os_error());
            }
            if code == STILL_ACTIVE as u32 {
                Ok(None)
            } else {
                Ok(Some(Some(code as i32)))
            }
        }

        pub(super) fn kill(&mut self) {
            // Children first (the shell's own children don't die with
            // it on Windows), then the root, then the console host.
            crate::tool::process::kill_process_group(Some(self.pid));
            unsafe {
                let _ = TerminateProcess(self.process, 1);
            }
            self.close_console();
        }

        pub(super) fn close_console(&mut self) {
            if !self.console_closed.swap(true, Ordering::SeqCst) {
                unsafe { ClosePseudoConsole(self.hpc) };
            }
        }

        pub(super) fn resize(&self, size: PtySize) -> Result<(), PtyError> {
            let coord = COORD {
                X: size.cols.max(1) as i16,
                Y: size.rows.max(1) as i16,
            };
            let hr = unsafe { ResizePseudoConsole(self.hpc, coord) };
            if hr != S_OK {
                return Err(PtyError::Io(format!(
                    "ResizePseudoConsole failed: HRESULT {hr:#x}"
                )));
            }
            Ok(())
        }
    }

    impl Drop for ConptyChild {
        fn drop(&mut self) {
            self.close_console();
            unsafe {
                let _ = CloseHandle(self.process);
            }
        }
    }

    /// Close-on-drop guard for handles on the error paths.
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = CloseHandle(self.0);
                }
            }
        }
    }
    impl Handle {
        fn take(mut self) -> HANDLE {
            std::mem::replace(&mut self.0, ptr::null_mut())
        }
    }

    fn last_error(what: &str) -> PtyError {
        PtyError::SpawnFailed(format!("{what}: {}", io::Error::last_os_error()))
    }

    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(once(0)).collect()
    }

    /// Quote one argument per the MSVCRT/CommandLineToArgvW rules.
    fn quote_arg(arg: &str) -> String {
        if !arg.is_empty() && !arg.contains([' ', '\t', '\n', '\x0b', '"']) {
            return arg.to_string();
        }
        let mut quoted = String::with_capacity(arg.len() + 2);
        quoted.push('"');
        let mut backslashes = 0usize;
        for ch in arg.chars() {
            match ch {
                '\\' => backslashes += 1,
                '"' => {
                    quoted.extend(std::iter::repeat('\\').take(backslashes * 2 + 1));
                    backslashes = 0;
                    quoted.push('"');
                    continue;
                }
                _ => {
                    quoted.extend(std::iter::repeat('\\').take(backslashes));
                    backslashes = 0;
                }
            }
            if ch != '"' {
                if ch == '\\' {
                    continue;
                }
                quoted.push(ch);
            }
        }
        quoted.extend(std::iter::repeat('\\').take(backslashes * 2));
        quoted.push('"');
        quoted
    }

    /// Inherited environment plus overrides, as a sorted UTF-16
    /// double-NUL-terminated block (`CREATE_UNICODE_ENVIRONMENT`).
    fn environment_block(extra: &[(&str, &str)]) -> Vec<u16> {
        let mut vars: Vec<(std::ffi::OsString, std::ffi::OsString)> = std::env::vars_os()
            .filter(|(key, _)| {
                !extra.iter().any(|(extra_key, _)| {
                    key.to_string_lossy().eq_ignore_ascii_case(extra_key)
                })
            })
            .collect();
        for (key, value) in extra {
            vars.push((key.into(), value.into()));
        }
        vars.sort_by(|(a, _), (b, _)| {
            a.to_string_lossy()
                .to_uppercase()
                .cmp(&b.to_string_lossy().to_uppercase())
        });

        let mut block = Vec::new();
        for (key, value) in vars {
            block.extend(key.encode_wide());
            block.push('=' as u16);
            block.extend(value.encode_wide());
            block.push(0);
        }
        block.push(0);
        block
    }

    pub(super) fn spawn(
        command: &[String],
        cwd: &str,
        extra_env: &[(&str, &str)],
        size: PtySize,
    ) -> Result<(ConptyChild, std::fs::File, std::fs::File), PtyError> {
        unsafe {
            // conin: we write, the console reads. conout: the console
            // writes, we read. Plain synchronous pipes — tokio::fs::File
            // reads them on the blocking pool.
            let (mut conin_read, mut conin_write): (HANDLE, HANDLE) =
                (ptr::null_mut(), ptr::null_mut());
            if CreatePipe(&mut conin_read, &mut conin_write, ptr::null(), 0) == 0 {
                return Err(last_error("CreatePipe (conin)"));
            }
            let conin_read = Handle(conin_read);
            let conin_write = Handle(conin_write);
            let (mut conout_read, mut conout_write): (HANDLE, HANDLE) =
                (ptr::null_mut(), ptr::null_mut());
            if CreatePipe(&mut conout_read, &mut conout_write, ptr::null(), 0) == 0 {
                return Err(last_error("CreatePipe (conout)"));
            }
            let conout_read = Handle(conout_read);
            let conout_write = Handle(conout_write);

            let coord = COORD {
                X: size.cols.max(1) as i16,
                Y: size.rows.max(1) as i16,
            };
            let mut hpc: HPCON = zeroed();
            let hr =
                CreatePseudoConsole(coord, conin_read.0, conout_write.0, 0, &mut hpc);
            if hr != S_OK {
                return Err(PtyError::SpawnFailed(format!(
                    "CreatePseudoConsole failed: HRESULT {hr:#x}"
                )));
            }
            // NOTE: the console-side pipe ends (conin_read/conout_write)
            // must stay open until AFTER CreateProcessW — the EchoCon
            // sample's ordering. Real Windows tolerates closing them
            // right here (the console host duplicates them), but Wine's
            // ConPTY does not, and closing early kills the console
            // before the child attaches.

            let close_hpc = |hpc: HPCON| ClosePseudoConsole(hpc);

            let mut attr_size: usize = 0;
            // First call intentionally fails, reporting the needed size.
            let _ =
                InitializeProcThreadAttributeList(ptr::null_mut(), 1, 0, &mut attr_size);
            let mut attr_buf = vec![0u8; attr_size];
            let attr_list = attr_buf.as_mut_ptr() as LPPROC_THREAD_ATTRIBUTE_LIST;
            if InitializeProcThreadAttributeList(attr_list, 1, 0, &mut attr_size) == 0 {
                let error = last_error("InitializeProcThreadAttributeList");
                close_hpc(hpc);
                return Err(error);
            }
            if UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_PSEUDOCONSOLE as usize,
                hpc as *mut core::ffi::c_void,
                size_of::<HPCON>(),
                ptr::null_mut(),
                ptr::null_mut(),
            ) == 0
            {
                let error = last_error("UpdateProcThreadAttribute");
                DeleteProcThreadAttributeList(attr_list);
                close_hpc(hpc);
                return Err(error);
            }

            let mut startup: STARTUPINFOEXW = zeroed();
            startup.StartupInfo.cb = size_of::<STARTUPINFOEXW>() as u32;
            // USESTDHANDLES with null handles: stops the child from
            // binding the PARENT's console for stdio instead of the
            // pseudoconsole (same trick as teletypewriter's spawn).
            startup.StartupInfo.dwFlags |= STARTF_USESTDHANDLES;
            startup.lpAttributeList = attr_list;

            let cmdline = command
                .iter()
                .map(|arg| quote_arg(arg))
                .collect::<Vec<_>>()
                .join(" ");
            let mut cmdline_wide = wide(std::ffi::OsStr::new(&cmdline));
            let cwd_wide = if cwd.is_empty() {
                None
            } else {
                Some(wide(std::ffi::OsStr::new(cwd)))
            };
            let mut env_block = environment_block(extra_env);

            let mut process_info: PROCESS_INFORMATION = zeroed();
            let created = CreateProcessW(
                ptr::null(),
                cmdline_wide.as_mut_ptr(),
                ptr::null(),
                ptr::null(),
                0,
                EXTENDED_STARTUPINFO_PRESENT
                    | CREATE_UNICODE_ENVIRONMENT
                    | CREATE_NO_WINDOW,
                env_block.as_mut_ptr() as *mut core::ffi::c_void,
                cwd_wide
                    .as_ref()
                    .map(|wide| wide.as_ptr())
                    .unwrap_or(ptr::null()),
                &startup.StartupInfo,
                &mut process_info,
            );
            DeleteProcThreadAttributeList(attr_list);
            if created == 0 {
                let error = last_error("CreateProcessW");
                close_hpc(hpc);
                return Err(error);
            }
            let _ = CloseHandle(process_info.hThread);
            drop(conin_read);
            drop(conout_write);

            let child = ConptyChild {
                hpc,
                process: process_info.hProcess,
                pid: process_info.dwProcessId,
                console_closed: AtomicBool::new(false),
            };
            let reader = std::fs::File::from_raw_handle(conout_read.take() as _);
            let writer = std::fs::File::from_raw_handle(conin_write.take() as _);
            Ok((child, reader, writer))
        }
    }
}

#[cfg(all(test, windows))]
mod conpty_tests {
    use super::*;
    use std::io::Read;

    /// Round-trips a real pseudoconsole: spawn `cmd /C echo` and read
    /// the marker back off the conout pipe. The pipes are synchronous,
    /// so — exactly like the production monitor task — a SEPARATE
    /// thread must close the pseudoconsole once the child exits, or a
    /// blocked `read` would never see EOF (the ConPTY host holds the
    /// write end open until `ClosePseudoConsole`).
    #[test]
    fn conpty_spawn_echo_roundtrip() {
        let (child, mut reader, _writer) = conpty::spawn(
            &[
                "cmd.exe".to_string(),
                "/C".to_string(),
                "echo hello-conpty".to_string(),
            ],
            "C:\\",
            &[("NEOISM_TERMINAL", "1")],
            PtySize { cols: 80, rows: 24 },
        )
        .expect("conpty spawn");
        let child = std::sync::Arc::new(std::sync::Mutex::new(child));

        let watchdog = std::sync::Arc::clone(&child);
        let watchdog_handle = std::thread::spawn(move || {
            let deadline = std::time::Instant::now() + Duration::from_secs(15);
            loop {
                {
                    let mut child = watchdog.lock().unwrap();
                    let exited = matches!(child.try_wait(), Ok(Some(_)));
                    if exited || std::time::Instant::now() >= deadline {
                        child.close_console();
                        return;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });

        let marker = b"hello-conpty";
        let mut got = Vec::new();
        let mut buf = [0u8; 512];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    got.extend_from_slice(&buf[..n]);
                    if got.windows(marker.len()).any(|w| w == marker) {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::BrokenPipe => break,
                Err(error) => panic!("conout read failed: {error}"),
            }
        }
        let _ = watchdog_handle.join();
        child.lock().unwrap().kill();
        assert!(
            got.windows(marker.len()).any(|w| w == marker),
            "marker missing from ConPTY output: {:?}",
            String::from_utf8_lossy(&got)
        );
    }
}

async fn write_stdin(stdin: &Arc<Mutex<PtyInput>>, data: &[u8]) -> Result<(), PtyError> {
    let mut input = stdin.lock().await;
    match &mut *input {
        #[cfg(not(unix))]
        PtyInput::Pipe(stdin) => {
            stdin
                .write_all(data)
                .await
                .map_err(|error| PtyError::Io(error.to_string()))?;
            stdin
                .flush()
                .await
                .map_err(|error| PtyError::Io(error.to_string()))
        }
        #[cfg(any(unix, windows))]
        PtyInput::Pty(file) => {
            file.write_all(data)
                .await
                .map_err(|error| PtyError::Io(error.to_string()))?;
            file.flush()
                .await
                .map_err(|error| PtyError::Io(error.to_string()))
        }
    }
}

async fn send_output(
    socket: &mut WebSocket,
    data: &str,
    cursor: u64,
) -> Result<(), axum::Error> {
    socket.send(Message::Text(data.to_string())).await?;
    send_cursor(socket, cursor).await
}

async fn send_cursor(socket: &mut WebSocket, cursor: u64) -> Result<(), axum::Error> {
    let mut payload = Vec::with_capacity(32);
    payload.push(0);
    payload.extend_from_slice(format!(r#"{{"cursor":{cursor}}}"#).as_bytes());
    socket.send(Message::Binary(payload)).await
}
