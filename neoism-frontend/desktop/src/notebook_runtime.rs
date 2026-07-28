use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    fs::File,
    io::ErrorKind,
    net::IpAddr,
    path::{Path, PathBuf},
    process::{Command as StdCommand, ExitStatus, Stdio},
    sync::{mpsc, Arc, Mutex},
    thread::JoinHandle,
    time::{Duration, Instant},
};

use jupyter_protocol::{
    ClearOutput, ConnectionInfo, DisplayData, ErrorOutput, ExecuteReply, ExecuteRequest,
    ExecuteResult, ExecutionState, JupyterMessage, JupyterMessageContent, Media,
    ReplyStatus, ShutdownRequest, Stdio as JupyterStdio, StreamContent, Transport,
    UpdateDisplayData,
};
use jupyter_zmq_client::{
    create_client_control_connection, create_client_iopub_connection,
    create_client_shell_connection_with_identity, find_kernelspec_with_jupyter_paths,
    list_kernelspecs_with_jupyter_paths, peek_ports_with_listeners,
    peer_identity_for_session, read_kernelspec_jsons, KernelspecDir,
};
use neoism_ui::editor::notebook::{
    NotebookDisplayUpdate, NotebookExecutionChunk, NotebookExecutionEvent,
    NotebookExecutionJob, NotebookExecutionResult, NotebookOutputStream,
};
use serde_json::Value;
use tokio::io::{AsyncRead, AsyncReadExt};

pub struct NotebookRuntimeManager {
    tx: Option<mpsc::Sender<RuntimeCommand>>,
    worker: Option<JoinHandle<()>>,
    active_processes: ActiveNotebookProcessRegistry,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AvailableNotebookKernel {
    pub name: String,
    pub display_name: String,
    pub language: String,
}

pub(crate) fn managed_python_kernel_env() -> BTreeMap<String, String> {
    let mut env = BTreeMap::new();
    if let Some(ld_library_path) = managed_python_ld_library_path() {
        env.insert("LD_LIBRARY_PATH".to_string(), ld_library_path);
    }
    env
}

fn managed_jupyter_data_dir() -> PathBuf {
    neoism_extensions::paths::extensions_dir()
        .join("jupyter")
        .join("share")
        .join("jupyter")
}

fn available_kernel_from_spec(spec: KernelspecDir) -> Option<AvailableNotebookKernel> {
    let name = spec.kernel_name.trim();
    let display_name = spec.kernelspec.display_name.trim();
    let language = spec.kernelspec.language.trim();
    if name.is_empty() || display_name.is_empty() || language.is_empty() {
        return None;
    }
    Some(AvailableNotebookKernel {
        name: name.to_string(),
        display_name: display_name.to_string(),
        language: language.to_string(),
    })
}

fn dedupe_and_sort_kernels(kernels: &mut Vec<AvailableNotebookKernel>) {
    let mut seen = BTreeSet::new();
    kernels.retain(|kernel| seen.insert(kernel.name.to_ascii_lowercase()));
    kernels.sort_by(|a, b| {
        kernel_sort_key(a)
            .cmp(&kernel_sort_key(b))
            .then_with(|| a.name.cmp(&b.name))
    });
}

fn kernel_sort_key(kernel: &AvailableNotebookKernel) -> (u8, String) {
    let name = kernel.name.to_ascii_lowercase();
    let language = kernel.language.to_ascii_lowercase();
    let priority = if name == "python3" {
        0
    } else if language == "python" {
        1
    } else {
        2
    };
    (priority, kernel.display_name.to_ascii_lowercase())
}

fn default_notebook_kernels() -> Vec<AvailableNotebookKernel> {
    vec![AvailableNotebookKernel {
        name: "python3".to_string(),
        display_name: "Python 3".to_string(),
        language: "python".to_string(),
    }]
}

pub(crate) fn list_available_kernels() -> Vec<AvailableNotebookKernel> {
    let runtime = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::warn!(error = %err, "Could not create runtime to list notebook kernels");
            return default_notebook_kernels();
        }
    };
    let mut kernels = runtime.block_on(async {
        let mut specs = read_kernelspec_jsons(&managed_jupyter_data_dir()).await;
        specs.extend(list_kernelspecs_with_jupyter_paths().await);
        specs
            .into_iter()
            .filter_map(available_kernel_from_spec)
            .collect::<Vec<_>>()
    });
    dedupe_and_sort_kernels(&mut kernels);
    if kernels.is_empty() {
        return default_notebook_kernels();
    }
    kernels
}

impl NotebookRuntimeManager {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel();
        let active_processes = ActiveNotebookProcessRegistry::default();
        let worker_processes = active_processes.clone();
        let worker = std::thread::spawn(move || runtime_worker(rx, worker_processes));
        Self {
            tx: Some(tx),
            worker: Some(worker),
            active_processes,
        }
    }

    pub fn submit(
        &mut self,
        job: NotebookExecutionJob,
    ) -> mpsc::Receiver<(PathBuf, NotebookExecutionEvent)> {
        let (event_tx, event_rx) = mpsc::channel();
        if let Some(tx) = &self.tx {
            if tx.send(RuntimeCommand::Execute { job, event_tx }).is_ok() {
                return event_rx;
            }
        }
        event_rx
    }

    pub fn restart_kernel(&mut self, path: PathBuf) -> bool {
        self.tx
            .as_ref()
            .is_some_and(|tx| tx.send(RuntimeCommand::RestartKernel { path }).is_ok())
    }

    pub fn shutdown_kernel(&mut self, path: PathBuf) -> bool {
        if let Ok(Some(process)) = self.active_processes.get(&path) {
            let _ = signal_notebook_interrupt(process);
        }
        self.tx
            .as_ref()
            .is_some_and(|tx| tx.send(RuntimeCommand::ShutdownKernel { path }).is_ok())
    }

    pub fn interrupt_kernel(&self, path: &Path) -> Result<(), String> {
        let process = self
            .active_processes
            .get(path)?
            .ok_or_else(|| "No active notebook process for this notebook".to_string())?;
        signal_notebook_interrupt(process)
    }
}

impl Drop for NotebookRuntimeManager {
    fn drop(&mut self) {
        self.tx.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

enum RuntimeCommand {
    Execute {
        job: NotebookExecutionJob,
        event_tx: mpsc::Sender<(PathBuf, NotebookExecutionEvent)>,
    },
    RestartKernel {
        path: PathBuf,
    },
    ShutdownKernel {
        path: PathBuf,
    },
}

struct RuntimeWorker {
    tokio: tokio::runtime::Runtime,
    kernels: HashMap<PathBuf, NativeJupyterSession>,
    active_processes: ActiveNotebookProcessRegistry,
}

fn runtime_worker(
    rx: mpsc::Receiver<RuntimeCommand>,
    active_processes: ActiveNotebookProcessRegistry,
) {
    let tokio = match tokio::runtime::Builder::new_current_thread()
        .enable_io()
        .enable_time()
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            tracing::error!(error = %err, "Could not create notebook runtime");
            return;
        }
    };
    let mut worker = RuntimeWorker {
        tokio,
        kernels: HashMap::new(),
        active_processes,
    };
    for command in rx {
        match command {
            RuntimeCommand::Execute { job, event_tx } => {
                let path = job.path.clone();
                worker.run_streaming(job, |event| {
                    let _ = event_tx.send((path.clone(), event));
                });
            }
            RuntimeCommand::RestartKernel { path } => {
                worker.restart_kernel(&path);
            }
            RuntimeCommand::ShutdownKernel { path } => {
                worker.shutdown_kernel(&path);
            }
        }
    }
}

impl RuntimeWorker {
    fn run_streaming(
        &mut self,
        job: NotebookExecutionJob,
        send: impl Fn(NotebookExecutionEvent),
    ) {
        if job
            .kernel_name
            .as_deref()
            .is_some_and(|name| !name.trim().is_empty())
            || use_jupyter_for_language(&job.language)
        {
            match self.run_jupyter(job.clone(), &send) {
                Ok(()) => return,
                Err(err) => {
                    tracing::warn!(notebook = %job.path.display(), error = %err, "Native Jupyter runtime failed");
                    send(NotebookExecutionEvent::Finished(NotebookExecutionResult {
                        cell_index: job.cell_index,
                        cell_id: job.cell_id.clone(),
                        run_id: job.run_id,
                        execution_count: job.execution_count,
                        outputs: vec![jupyter_runtime_error_output(&err)],
                        status: Err(err),
                        elapsed_ms: 0,
                    }));
                    return;
                }
            }
        }

        let result = self.run_local_process(job, &send);
        send(NotebookExecutionEvent::Finished(result));
    }

    fn run_jupyter(
        &mut self,
        job: NotebookExecutionJob,
        send: &impl Fn(NotebookExecutionEvent),
    ) -> Result<(), String> {
        let kernel_name = kernel_name_for_job(&job);
        let key = job.path.clone();
        let cwd = job
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .map(Path::to_path_buf);
        let needs_restart = self
            .kernels
            .get(&key)
            .is_some_and(|session| session.kernel_name != kernel_name);
        if needs_restart {
            if let Some(mut session) = self.kernels.remove(&key) {
                self.active_processes.remove(&key);
                self.tokio.block_on(async { session.shutdown().await });
            }
        }
        if !self.kernels.contains_key(&key) {
            let session = self
                .tokio
                .block_on(NativeJupyterSession::start(&kernel_name, cwd))?;
            self.active_processes.register_session(&key, &session)?;
            self.kernels.insert(key.clone(), session);
        }
        let session = self
            .kernels
            .get_mut(&key)
            .ok_or_else(|| "Jupyter kernel session was not created".to_string())?;
        let result = match self.tokio.block_on(session.execute(&job, send)) {
            Ok(result) => result,
            Err(err) => {
                if let Some(mut session) = self.kernels.remove(&key) {
                    self.active_processes.remove(&key);
                    self.tokio.block_on(async { session.shutdown().await });
                }
                return Err(err);
            }
        };
        send(NotebookExecutionEvent::Finished(result));
        Ok(())
    }

    fn restart_kernel(&mut self, path: &Path) {
        self.shutdown_kernel(path);
    }

    fn shutdown_kernel(&mut self, path: &Path) {
        if let Some(mut session) = self.kernels.remove(path) {
            self.active_processes.remove(path);
            self.tokio.block_on(async { session.shutdown().await });
        }
    }

    fn run_local_process(
        &mut self,
        job: NotebookExecutionJob,
        send: &impl Fn(NotebookExecutionEvent),
    ) -> NotebookExecutionResult {
        let started_at = Instant::now();
        match self.run_local_process_streaming(&job, send) {
            Ok((outputs, status)) => {
                let result_status = if status.success() {
                    Ok(())
                } else {
                    Err(format!(
                        "{} exited with {status}",
                        local_executor_label(&job.language)
                    ))
                };
                NotebookExecutionResult {
                    cell_index: job.cell_index,
                    cell_id: job.cell_id,
                    run_id: job.run_id,
                    execution_count: job.execution_count,
                    outputs,
                    status: result_status,
                    elapsed_ms: started_at.elapsed().as_millis(),
                }
            }
            Err(err) => {
                let result_status = Err(err.clone());
                NotebookExecutionResult {
                    cell_index: job.cell_index,
                    cell_id: job.cell_id,
                    run_id: job.run_id,
                    execution_count: job.execution_count,
                    outputs: vec![serde_json::json!({
                        "output_type": "error",
                        "ename": "ExecutionError",
                        "evalue": err,
                        "traceback": [],
                    })],
                    status: result_status,
                    elapsed_ms: started_at.elapsed().as_millis(),
                }
            }
        }
    }

    fn run_local_process_streaming(
        &mut self,
        job: &NotebookExecutionJob,
        send: &impl Fn(NotebookExecutionEvent),
    ) -> Result<(Vec<Value>, ExitStatus), String> {
        let mut command = local_command_for_language(&job.language)?;
        if let Some(parent) = job
            .path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            command.current_dir(parent);
        }
        configure_local_process_interrupt(&mut command);
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|err| err.to_string())?;
        let pid = child.id();
        // Local processes get their own process group (setpgid on unix,
        // CREATE_NEW_PROCESS_GROUP on windows), so both advertise the
        // group-interrupt capability.
        if let Err(err) =
            self.active_processes
                .register_pid(&job.path, pid, cfg!(any(unix, windows)))
        {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err);
        }

        let result = run_spawned_local_process(&mut child, job, send);
        self.active_processes.remove(&job.path);
        result
    }
}

impl Drop for RuntimeWorker {
    fn drop(&mut self) {
        for (path, mut session) in self.kernels.drain() {
            self.active_processes.remove(&path);
            self.tokio.block_on(async { session.shutdown().await });
        }
    }
}

#[derive(Clone, Default)]
struct ActiveNotebookProcessRegistry {
    processes: Arc<Mutex<HashMap<PathBuf, ActiveNotebookProcess>>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ActiveNotebookProcess {
    pid: u32,
    signal_process_group: bool,
}

impl ActiveNotebookProcessRegistry {
    fn get(&self, path: &Path) -> Result<Option<ActiveNotebookProcess>, String> {
        self.processes
            .lock()
            .map_err(|_| "Notebook process registry is unavailable".to_string())
            .map(|processes| processes.get(path).copied())
    }

    fn register_session(
        &self,
        path: &Path,
        session: &NativeJupyterSession,
    ) -> Result<(), String> {
        let Some(pid) = session.process_id() else {
            return Ok(());
        };
        self.register_pid(path, pid, false)
    }

    fn register_pid(
        &self,
        path: &Path,
        pid: u32,
        signal_process_group: bool,
    ) -> Result<(), String> {
        self.processes
            .lock()
            .map_err(|_| "Notebook process registry is unavailable".to_string())?
            .insert(
                path.to_path_buf(),
                ActiveNotebookProcess {
                    pid,
                    signal_process_group,
                },
            );
        Ok(())
    }

    fn remove(&self, path: &Path) {
        if let Ok(mut processes) = self.processes.lock() {
            processes.remove(path);
        }
    }
}

#[cfg(unix)]
fn signal_notebook_interrupt(process: ActiveNotebookProcess) -> Result<(), String> {
    let target = if process.signal_process_group {
        -(process.pid as libc::pid_t)
    } else {
        process.pid as libc::pid_t
    };
    let rc = unsafe { libc::kill(target, libc::SIGINT) };
    if rc == 0 {
        Ok(())
    } else {
        Err(format!(
            "Could not interrupt notebook process {}: {}",
            process.pid,
            std::io::Error::last_os_error()
        ))
    }
}

#[cfg(windows)]
fn signal_notebook_interrupt(process: ActiveNotebookProcess) -> Result<(), String> {
    use windows_sys::Win32::System::Console::{
        GenerateConsoleCtrlEvent, CTRL_BREAK_EVENT,
    };

    // Best effort: CTRL_BREAK reaches the child's whole process group (local
    // processes spawn with CREATE_NEW_PROCESS_GROUP, so their pid names the
    // group), but only when the child shares a console with us. Piped kernels
    // without a console make the call fail; keep the polite explanation so
    // the UI still tells the user what happened.
    let ok = unsafe { GenerateConsoleCtrlEvent(CTRL_BREAK_EVENT, process.pid) };
    if ok != 0 {
        Ok(())
    } else {
        let err = std::io::Error::last_os_error();
        Err(format!(
            "Notebook kernel interrupt is not supported for this kernel on Windows yet \
             (GenerateConsoleCtrlEvent for process {} failed: {err})",
            process.pid
        ))
    }
}

#[cfg(not(any(unix, windows)))]
fn signal_notebook_interrupt(_process: ActiveNotebookProcess) -> Result<(), String> {
    Err("Notebook kernel interrupt is not supported on this platform yet".to_string())
}

fn local_command_for_language(language: &str) -> Result<StdCommand, String> {
    let normalized = language.trim().to_ascii_lowercase();
    let command = if is_shell_language(&normalized) {
        let shell = match normalized.as_str() {
            "bash" => "bash",
            "zsh" => "zsh",
            "fish" => "fish",
            _ => "sh",
        };
        let mut command = StdCommand::new(shell);
        command.arg("-");
        command
    } else if normalized == "python"
        || normalized == "python3"
        || normalized == "ipython"
        || normalized == "ipython3"
    {
        let mut command = StdCommand::new("python3");
        command.arg("-");
        command
    } else {
        return Err(format!(
            "No notebook executor for `{language}` yet (supported: python, sh, bash, zsh, fish)"
        ));
    };
    Ok(command)
}

fn is_shell_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "sh" | "shell" | "bash" | "zsh" | "fish"
    )
}

fn local_executor_label(language: &str) -> &'static str {
    if is_shell_language(language) {
        "Shell"
    } else {
        "Python"
    }
}

#[cfg(unix)]
fn configure_local_process_interrupt(command: &mut StdCommand) {
    use std::os::unix::process::CommandExt;

    unsafe {
        command.pre_exec(|| {
            if libc::setpgid(0, 0) == 0 {
                Ok(())
            } else {
                Err(std::io::Error::last_os_error())
            }
        });
    }
}

#[cfg(windows)]
fn configure_local_process_interrupt(command: &mut StdCommand) {
    use std::os::windows::process::CommandExt;

    // A fresh process group lets `signal_notebook_interrupt` CTRL_BREAK the
    // child without tearing down our own group. `creation_flags` REPLACES any
    // previously-set flags (Command exposes no getter), so this must stay the
    // only flag-setting site for local notebook commands.
    command
        .creation_flags(windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(any(unix, windows)))]
fn configure_local_process_interrupt(_command: &mut StdCommand) {}

fn run_spawned_local_process(
    child: &mut std::process::Child,
    job: &NotebookExecutionJob,
    send: &impl Fn(NotebookExecutionEvent),
) -> Result<(Vec<Value>, ExitStatus), String> {
    if let Some(mut stdin) = child.stdin.take() {
        use std::io::Write;
        if let Err(err) = stdin.write_all(job.fallback_script.as_bytes()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(err.to_string());
        }
    }

    let (tx, rx) = std::sync::mpsc::channel::<NotebookExecutionChunk>();
    let mut readers = Vec::new();
    if let Some(stdout) = child.stdout.take() {
        let tx = tx.clone();
        let cell_index = job.cell_index;
        let cell_id = job.cell_id.clone();
        let run_id = job.run_id;
        readers.push(std::thread::spawn(move || {
            local_stream_reader(
                stdout,
                cell_index,
                cell_id,
                run_id,
                NotebookOutputStream::Stdout,
                tx,
            );
        }));
    }
    if let Some(stderr) = child.stderr.take() {
        let tx = tx.clone();
        let cell_index = job.cell_index;
        let cell_id = job.cell_id.clone();
        let run_id = job.run_id;
        readers.push(std::thread::spawn(move || {
            local_stream_reader(
                stderr,
                cell_index,
                cell_id,
                run_id,
                NotebookOutputStream::Stderr,
                tx,
            );
        }));
    }
    drop(tx);

    let mut outputs = Vec::new();
    for chunk in rx {
        append_local_stream_output(&mut outputs, chunk.stream, &chunk.text);
        send(NotebookExecutionEvent::Output(chunk));
    }
    for reader in readers {
        let _ = reader.join();
    }
    let status = child.wait().map_err(|err| err.to_string())?;
    if outputs.is_empty() {
        append_local_stream_output(&mut outputs, NotebookOutputStream::Stdout, "");
    }
    Ok((outputs, status))
}

fn local_stream_reader<R: std::io::Read + Send + 'static>(
    mut reader: R,
    cell_index: usize,
    cell_id: String,
    run_id: u64,
    stream: NotebookOutputStream,
    tx: std::sync::mpsc::Sender<NotebookExecutionChunk>,
) {
    let mut buf = [0_u8; 4096];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let text = String::from_utf8_lossy(&buf[..n]).to_string();
                let _ = tx.send(NotebookExecutionChunk {
                    cell_index,
                    cell_id: cell_id.clone(),
                    run_id,
                    stream,
                    text,
                });
            }
            Err(_) => break,
        }
    }
}

fn append_local_stream_output(
    outputs: &mut Vec<Value>,
    stream: NotebookOutputStream,
    text: &str,
) {
    let name = match stream {
        NotebookOutputStream::Stdout => "stdout",
        NotebookOutputStream::Stderr => "stderr",
    };
    if let Some(existing) = outputs.iter_mut().rev().find(|output| {
        output.get("output_type").and_then(Value::as_str) == Some("stream")
            && output.get("name").and_then(Value::as_str) == Some(name)
    }) {
        let current = value_text(existing.get("text")).unwrap_or_default();
        existing["text"] = Value::String(format!("{current}{text}"));
        return;
    }
    outputs.push(serde_json::json!({
        "output_type": "stream",
        "name": name,
        "text": text,
    }));
}

const MAX_KERNEL_PROCESS_OUTPUT_BYTES: usize = 16 * 1024;

#[derive(Default)]
struct KernelProcessOutput {
    stdout: String,
    stderr: String,
}

struct KernelOutputCapture {
    stdout: Option<tokio::task::JoinHandle<String>>,
    stderr: Option<tokio::task::JoinHandle<String>>,
}

impl KernelOutputCapture {
    fn start(child: &mut tokio::process::Child) -> Self {
        Self {
            stdout: child.stdout.take().map(spawn_kernel_output_reader),
            stderr: child.stderr.take().map(spawn_kernel_output_reader),
        }
    }

    async fn collect(self) -> KernelProcessOutput {
        KernelProcessOutput {
            stdout: join_kernel_output(self.stdout).await,
            stderr: join_kernel_output(self.stderr).await,
        }
    }

    fn abort(&mut self) {
        if let Some(stdout) = self.stdout.take() {
            stdout.abort();
        }
        if let Some(stderr) = self.stderr.take() {
            stderr.abort();
        }
    }
}

fn spawn_kernel_output_reader<R>(reader: R) -> tokio::task::JoinHandle<String>
where
    R: AsyncRead + Send + Unpin + 'static,
{
    tokio::spawn(read_limited_kernel_output(reader))
}

async fn read_limited_kernel_output<R>(mut reader: R) -> String
where
    R: AsyncRead + Unpin,
{
    let mut out = Vec::new();
    let mut buf = [0_u8; 1024];
    loop {
        match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => {
                let remaining = MAX_KERNEL_PROCESS_OUTPUT_BYTES.saturating_sub(out.len());
                if remaining > 0 {
                    out.extend_from_slice(&buf[..n.min(remaining)]);
                }
            }
            Err(_) => break,
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

async fn join_kernel_output(handle: Option<tokio::task::JoinHandle<String>>) -> String {
    match handle {
        Some(handle) => handle.await.unwrap_or_default(),
        None => String::new(),
    }
}

fn with_kernel_process_output(err: String, output: &KernelProcessOutput) -> String {
    let stderr = output.stderr.trim();
    let stdout = output.stdout.trim();
    if stderr.is_empty() && stdout.is_empty() {
        return err;
    }

    let mut parts = vec![err];
    if !stderr.is_empty() {
        parts.push(format!("Kernel stderr:\n{stderr}"));
    }
    if !stdout.is_empty() {
        parts.push(format!("Kernel stdout:\n{stdout}"));
    }
    parts.join("\n\n")
}

struct NativeJupyterSession {
    kernel_name: String,
    child: tokio::process::Child,
    output_capture: KernelOutputCapture,
    connection_file: PathBuf,
    shell: jupyter_zmq_client::ClientShellConnection,
    iopub: jupyter_zmq_client::ClientIoPubConnection,
    control: jupyter_zmq_client::ClientControlConnection,
}

impl NativeJupyterSession {
    async fn start(kernel_name: &str, cwd: Option<PathBuf>) -> Result<Self, String> {
        let kernelspec = find_neoism_or_system_kernelspec(kernel_name).await?;
        let (connection_info, listeners) = connection_info(kernel_name).await?;
        let connection_file = connection_file_path(kernel_name);
        write_connection_file(&connection_file, &connection_info)?;
        let mut command = kernelspec
            .command(&connection_file, Some(Stdio::piped()), Some(Stdio::piped()))
            .map_err(|err| format!("Could not build kernel command: {err}"))?;
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        drop(listeners);
        let mut child = match command.spawn() {
            Ok(child) => child,
            Err(err) => {
                let _ = std::fs::remove_file(&connection_file);
                return Err(format!(
                    "Could not start Jupyter kernel `{kernel_name}`: {err}"
                ));
            }
        };
        let output_capture = KernelOutputCapture::start(&mut child);

        let connected = async {
            tokio::time::sleep(Duration::from_millis(150)).await;

            let session_id = uuid::Uuid::new_v4().to_string();
            let identity = peer_identity_for_session(&session_id).map_err(|err| {
                format!("Could not create Jupyter peer identity: {err}")
            })?;
            let shell = create_client_shell_connection_with_identity(
                &connection_info,
                &session_id,
                identity,
            )
            .await
            .map_err(|err| {
                format!("Could not connect to Jupyter shell channel: {err}")
            })?;
            let iopub = create_client_iopub_connection(&connection_info, "", &session_id)
                .await
                .map_err(|err| {
                    format!("Could not connect to Jupyter IOPub channel: {err}")
                })?;
            let control = create_client_control_connection(&connection_info, &session_id)
                .await
                .map_err(|err| {
                    format!("Could not connect to Jupyter control channel: {err}")
                })?;
            tokio::time::sleep(Duration::from_millis(100)).await;
            Ok::<_, String>((shell, iopub, control))
        }
        .await;

        let (shell, iopub, control) = match connected {
            Ok(channels) => channels,
            Err(err) => {
                let _ = child.kill().await;
                let output = output_capture.collect().await;
                let _ = std::fs::remove_file(&connection_file);
                return Err(with_kernel_process_output(err, &output));
            }
        };

        Ok(Self {
            kernel_name: kernel_name.to_string(),
            child,
            output_capture,
            connection_file,
            shell,
            iopub,
            control,
        })
    }

    fn process_id(&self) -> Option<u32> {
        self.child.id()
    }

    async fn execute(
        &mut self,
        job: &NotebookExecutionJob,
        send: &impl Fn(NotebookExecutionEvent),
    ) -> Result<NotebookExecutionResult, String> {
        let started_at = Instant::now();
        let request: JupyterMessage = ExecuteRequest::new(job.source.clone()).into();
        let request_id = request.header.msg_id.clone();
        self.shell
            .send(request)
            .await
            .map_err(|err| format!("Could not send Jupyter execute_request: {err}"))?;

        let mut outputs = Vec::new();
        let mut execution_count = job.execution_count;
        let mut status = Ok(());
        let mut clear_waiting = false;
        loop {
            let msg = tokio::time::timeout(Duration::from_secs(120), self.iopub.read())
                .await
                .map_err(|_| "Timed out waiting for Jupyter IOPub output".to_string())?
                .map_err(|err| format!("Could not read Jupyter IOPub output: {err}"))?;
            if !message_is_child_of(&msg, &request_id) {
                continue;
            }
            match msg.content {
                JupyterMessageContent::ExecuteInput(input) => {
                    execution_count = input.execution_count.value() as u32;
                }
                JupyterMessageContent::StreamContent(stream) => {
                    apply_pending_clear_output(&mut outputs, &mut clear_waiting);
                    let output = stream_output_json(&stream);
                    outputs.push(output.clone());
                    if let Some(chunk) = execution_chunk_from_output(
                        job.cell_index,
                        &job.cell_id,
                        job.run_id,
                        &output,
                    ) {
                        send(NotebookExecutionEvent::Output(chunk));
                    }
                }
                JupyterMessageContent::DisplayData(display) => {
                    apply_pending_clear_output(&mut outputs, &mut clear_waiting);
                    outputs.push(display_data_json(&display));
                }
                JupyterMessageContent::ExecuteResult(result) => {
                    apply_pending_clear_output(&mut outputs, &mut clear_waiting);
                    execution_count = result.execution_count.value() as u32;
                    outputs.push(execute_result_json(&result));
                }
                JupyterMessageContent::UpdateDisplayData(update) => {
                    apply_pending_clear_output(&mut outputs, &mut clear_waiting);
                    apply_update_display_data(&mut outputs, &update);
                    if let Some(event) = display_update_event_from_update(&job, &update) {
                        send(event);
                    }
                }
                JupyterMessageContent::ClearOutput(clear) => {
                    apply_clear_output(&mut outputs, &mut clear_waiting, &clear);
                }
                JupyterMessageContent::ErrorOutput(error) => {
                    apply_pending_clear_output(&mut outputs, &mut clear_waiting);
                    status = Err(format!("{}: {}", error.ename, error.evalue));
                    outputs.push(error_output_json(&error));
                }
                JupyterMessageContent::Status(status_msg)
                    if status_msg.execution_state == ExecutionState::Idle =>
                {
                    break;
                }
                _ => {}
            }
        }

        if let Some(reply) = self.read_execute_reply(&request_id).await? {
            execution_count = reply.execution_count.value() as u32;
            if reply.status != ReplyStatus::Ok {
                let err = reply
                    .error
                    .map(|err| format!("{}: {}", err.ename, err.evalue))
                    .unwrap_or_else(|| {
                        format!("Jupyter execution ended with {:?}", reply.status)
                    });
                status = Err(err);
            }
        }

        Ok(NotebookExecutionResult {
            cell_index: job.cell_index,
            cell_id: job.cell_id.clone(),
            run_id: job.run_id,
            execution_count,
            outputs,
            status,
            elapsed_ms: started_at.elapsed().as_millis(),
        })
    }

    async fn read_execute_reply(
        &mut self,
        request_id: &str,
    ) -> Result<Option<ExecuteReply>, String> {
        for _ in 0..8 {
            let msg = tokio::time::timeout(Duration::from_secs(5), self.shell.read())
                .await
                .map_err(|_| "Timed out waiting for Jupyter execute_reply".to_string())?
                .map_err(|err| format!("Could not read Jupyter execute_reply: {err}"))?;
            if !message_is_child_of(&msg, request_id) {
                continue;
            }
            if let JupyterMessageContent::ExecuteReply(reply) = msg.content {
                return Ok(Some(reply));
            }
        }
        Ok(None)
    }

    async fn shutdown(&mut self) {
        let _ = self
            .control
            .send(JupyterMessage::from(ShutdownRequest { restart: false }))
            .await;
        let _ = tokio::time::timeout(Duration::from_secs(2), self.child.wait()).await;
        let _ = self.child.kill().await;
        self.output_capture.abort();
        let _ = std::fs::remove_file(&self.connection_file);
    }
}

async fn find_neoism_or_system_kernelspec(
    kernel_name: &str,
) -> Result<KernelspecDir, String> {
    let managed = managed_jupyter_data_dir();
    let mut managed_error = None;
    for spec in read_kernelspec_jsons(&managed).await {
        if spec.kernel_name == kernel_name {
            match validate_managed_python_kernelspec(&spec) {
                Ok(()) => return Ok(spec),
                Err(err) => {
                    tracing::warn!(
                        kernel = %kernel_name,
                        path = %spec.path.display(),
                        error = %err,
                        "Ignoring unusable managed Jupyter kernelspec"
                    );
                    managed_error = Some(err);
                }
            }
        }
    }
    match find_kernelspec_with_jupyter_paths(kernel_name).await {
        Ok(spec) => Ok(spec),
        Err(err) => {
            if let Some(managed_error) = managed_error {
                Err(format!(
                    "Could not find usable Jupyter kernelspec `{kernel_name}`. \
The managed kernelspec is present but unusable: {managed_error}. \
Reinstall Extensions -> Kernels -> Neoism Python Kernel, then run the cell again."
                ))
            } else {
                Err(format!(
                    "Could not find Jupyter kernelspec `{kernel_name}`: {err}"
                ))
            }
        }
    }
}

pub(crate) fn has_managed_python_kernel() -> bool {
    managed_python_kernel_value()
        .and_then(|value| {
            let program = kernel_program_from_value(&value)?;
            let env = kernel_env_from_value(&value);
            validate_python_kernel_program(&program, &env)
        })
        .is_ok()
}

fn managed_python_kernel_json_path() -> PathBuf {
    neoism_extensions::paths::extensions_dir()
        .join("jupyter")
        .join("share")
        .join("jupyter")
        .join("kernels")
        .join("python3")
        .join("kernel.json")
}

fn managed_python_kernel_value() -> Result<Value, String> {
    let kernel_json = managed_python_kernel_json_path();
    let source = std::fs::read_to_string(&kernel_json).map_err(|err| {
        format!(
            "Could not read managed Python kernelspec {}: {err}",
            kernel_json.display()
        )
    })?;
    let value = serde_json::from_str::<Value>(&source).map_err(|err| {
        format!(
            "Could not parse managed Python kernelspec {}: {err}",
            kernel_json.display()
        )
    })?;
    Ok(value)
}

fn kernel_program_from_value(value: &Value) -> Result<String, String> {
    value
        .get("argv")
        .and_then(Value::as_array)
        .and_then(|argv| argv.first())
        .and_then(Value::as_str)
        .filter(|program| !program.trim().is_empty())
        .map(ToString::to_string)
        .ok_or_else(|| "managed Python kernelspec has no argv[0]".to_string())
}

fn kernel_env_from_value(value: &Value) -> HashMap<String, String> {
    value
        .get("env")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value
                        .as_str()
                        .map(|value| (key.to_string(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

fn validate_managed_python_kernelspec(spec: &KernelspecDir) -> Result<(), String> {
    let Some(program) = spec.kernelspec.argv.first() else {
        return Err(format!(
            "managed Python kernelspec {} has no argv[0]",
            spec.path.display()
        ));
    };
    let empty_env = HashMap::new();
    let env = spec.kernelspec.env.as_ref().unwrap_or(&empty_env);
    validate_python_kernel_program(program, env)
}

fn validate_python_kernel_program(
    program: &str,
    env: &HashMap<String, String>,
) -> Result<(), String> {
    if program.trim().is_empty() {
        return Err("kernel Python executable is empty".to_string());
    }
    let mut command = StdCommand::new(program);
    command
        .arg("-c")
        .arg("import ipykernel")
        .envs(env)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    match command.output() {
        Ok(output) if output.status.success() => Ok(()),
        Ok(output) => {
            let detail = process_output_detail(&output.stdout, &output.stderr);
            let status = output.status;
            if detail.trim().is_empty() {
                Err(format!(
                    "kernel Python `{program}` cannot import ipykernel (exited with {status})"
                ))
            } else {
                Err(format!(
                    "kernel Python `{program}` cannot import ipykernel (exited with {status}):\n{detail}"
                ))
            }
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            Err(format!("kernel Python `{program}` does not exist"))
        }
        Err(err) => Err(format!("could not run kernel Python `{program}`: {err}")),
    }
}

fn managed_python_ld_library_path() -> Option<String> {
    if !cfg!(target_os = "linux") {
        return None;
    }
    managed_python_ld_library_path_from(managed_python_library_dir_candidates())
}

fn managed_python_library_dir_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
        candidates.extend(std::env::split_paths(&existing));
    }
    candidates.extend(
        [
            "/usr/lib",
            "/usr/lib64",
            "/usr/local/lib",
            "/usr/local/lib64",
            "/lib",
            "/lib64",
            "/usr/lib/x86_64-linux-gnu",
            "/lib/x86_64-linux-gnu",
            "/run/current-system/sw/lib",
        ]
        .into_iter()
        .map(PathBuf::from),
    );
    if let Some(home) = std::env::var_os("HOME").map(PathBuf::from) {
        candidates.push(home.join(".nix-profile").join("lib"));
    }
    if let Some(user) = std::env::var_os("USER") {
        candidates.push(
            PathBuf::from("/etc/profiles/per-user")
                .join(user)
                .join("lib"),
        );
    }
    candidates
}

fn managed_python_ld_library_path_from(candidates: Vec<PathBuf>) -> Option<String> {
    let mut dirs = Vec::new();
    for dir in candidates {
        if !dir.join("libstdc++.so.6").exists() {
            continue;
        }
        if dirs.iter().any(|existing| existing == &dir) {
            continue;
        }
        dirs.push(dir);
    }
    if dirs.is_empty() {
        return None;
    }
    std::env::join_paths(dirs)
        .ok()
        .map(|paths| paths.to_string_lossy().to_string())
}

fn process_output_detail(stdout: &[u8], stderr: &[u8]) -> String {
    let stderr = String::from_utf8_lossy(stderr);
    let stdout = String::from_utf8_lossy(stdout);
    if stderr.trim().is_empty() {
        stdout.to_string()
    } else if stdout.trim().is_empty() {
        stderr.to_string()
    } else {
        format!("stderr:\n{stderr}\nstdout:\n{stdout}")
    }
}

fn is_missing_kernel_error(err: &str) -> bool {
    err.contains("Could not find Jupyter kernelspec")
        || err.contains("Could not find usable Jupyter kernelspec")
        || err.contains("KernelNotFound")
}

fn jupyter_runtime_error_output(err: &str) -> Value {
    let missing_kernel = is_missing_kernel_error(err);
    serde_json::json!({
        "output_type": "error",
        "ename": if missing_kernel {
            "JupyterKernelUnavailable"
        } else {
            "JupyterExecutionError"
        },
        "evalue": err,
        "traceback": jupyter_runtime_traceback(err, missing_kernel),
    })
}

fn jupyter_runtime_traceback(err: &str, missing_kernel: bool) -> Vec<String> {
    if missing_kernel {
        vec![
            "Neoism could not find a usable Python Jupyter kernel.".to_string(),
            err.to_string(),
            "Install or reinstall Extensions -> Kernels -> Neoism Python Kernel, then run the cell again.".to_string(),
        ]
    } else {
        vec![
            "Neoism found a Python Jupyter kernel, but the kernel did not start or respond.".to_string(),
            err.to_string(),
            "Reinstall Extensions -> Kernels -> Neoism Python Kernel, or verify the selected kernelspec can run ipykernel_launcher.".to_string(),
        ]
    }
}

async fn connection_info(
    kernel_name: &str,
) -> Result<(ConnectionInfo, Vec<tokio::net::TcpListener>), String> {
    let ip: IpAddr = "127.0.0.1"
        .parse()
        .map_err(|err| format!("Invalid Jupyter kernel bind address: {err}"))?;
    let (ports, listeners) = peek_ports_with_listeners(ip, 5)
        .await
        .map_err(|err| format!("Could not reserve Jupyter kernel ports: {err}"))?;
    Ok((
        ConnectionInfo {
            transport: Transport::TCP,
            ip: ip.to_string(),
            shell_port: ports[0],
            iopub_port: ports[1],
            stdin_port: ports[2],
            control_port: ports[3],
            hb_port: ports[4],
            signature_scheme: "hmac-sha256".to_string(),
            key: uuid::Uuid::new_v4().to_string(),
            kernel_name: Some(kernel_name.to_string()),
        },
        listeners,
    ))
}

fn connection_file_path(kernel_name: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "neoism-kernel-{kernel_name}-{}.json",
        uuid::Uuid::new_v4()
    ))
}

fn write_connection_file(
    path: &Path,
    connection_info: &ConnectionInfo,
) -> Result<(), String> {
    let file = File::create(path).map_err(|err| {
        format!(
            "Could not create Jupyter connection file {}: {err}",
            path.display()
        )
    })?;
    serde_json::to_writer(file, connection_info).map_err(|err| {
        format!(
            "Could not write Jupyter connection file {}: {err}",
            path.display()
        )
    })
}

fn message_is_child_of(msg: &JupyterMessage, request_id: &str) -> bool {
    msg.parent_header
        .as_ref()
        .is_some_and(|header| header.msg_id == request_id)
}

fn stream_output_json(stream: &StreamContent) -> Value {
    serde_json::json!({
        "output_type": "stream",
        "name": match stream.name {
            JupyterStdio::Stderr => "stderr",
            JupyterStdio::Stdout => "stdout",
        },
        "text": stream.text,
    })
}

fn apply_clear_output(
    outputs: &mut Vec<Value>,
    clear_waiting: &mut bool,
    clear: &ClearOutput,
) {
    if clear.wait {
        *clear_waiting = true;
    } else {
        outputs.clear();
        *clear_waiting = false;
    }
}

fn apply_pending_clear_output(outputs: &mut Vec<Value>, clear_waiting: &mut bool) {
    if *clear_waiting {
        outputs.clear();
        *clear_waiting = false;
    }
}

fn apply_update_display_data(outputs: &mut Vec<Value>, update: &UpdateDisplayData) {
    let replacement = update_display_data_json(update);
    let Some(display_id) = update.transient.display_id.as_deref() else {
        outputs.push(replacement);
        return;
    };

    for output in outputs.iter_mut() {
        if output_display_id(output) == Some(display_id) {
            *output = replacement.clone();
        }
    }
}

fn display_update_event_from_update(
    job: &NotebookExecutionJob,
    update: &UpdateDisplayData,
) -> Option<NotebookExecutionEvent> {
    let display_id = update
        .transient
        .display_id
        .as_deref()
        .filter(|id| !id.is_empty())?
        .to_string();
    Some(NotebookExecutionEvent::DisplayUpdate(
        NotebookDisplayUpdate {
            cell_index: job.cell_index,
            cell_id: job.cell_id.clone(),
            run_id: job.run_id,
            display_id,
            output: update_display_data_json(update),
        },
    ))
}

fn display_data_json(display: &DisplayData) -> Value {
    let mut output = serde_json::json!({
        "output_type": "display_data",
        "data": media_json(&display.data),
        "metadata": display.metadata,
    });
    if let Some(transient) = display.transient.as_ref() {
        insert_transient_json(&mut output, transient.display_id.as_deref());
    }
    output
}

fn execute_result_json(result: &ExecuteResult) -> Value {
    let mut output = serde_json::json!({
        "output_type": "execute_result",
        "execution_count": result.execution_count.value(),
        "data": media_json(&result.data),
        "metadata": result.metadata,
    });
    if let Some(transient) = result.transient.as_ref() {
        insert_transient_json(&mut output, transient.display_id.as_deref());
    }
    output
}

fn update_display_data_json(update: &UpdateDisplayData) -> Value {
    let mut output = serde_json::json!({
        "output_type": "display_data",
        "data": media_json(&update.data),
        "metadata": update.metadata,
    });
    insert_transient_json(&mut output, update.transient.display_id.as_deref());
    output
}

fn insert_transient_json(output: &mut Value, display_id: Option<&str>) {
    let Some(display_id) = display_id.filter(|id| !id.is_empty()) else {
        return;
    };
    if let Some(object) = output.as_object_mut() {
        object.insert(
            "transient".to_string(),
            serde_json::json!({ "display_id": display_id }),
        );
    }
}

fn output_display_id(output: &Value) -> Option<&str> {
    output
        .get("transient")
        .and_then(|transient| transient.get("display_id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
}

fn error_output_json(error: &ErrorOutput) -> Value {
    serde_json::json!({
        "output_type": "error",
        "ename": error.ename,
        "evalue": error.evalue,
        "traceback": error.traceback,
    })
}

fn media_json(media: &Media) -> Value {
    serde_json::to_value(media).unwrap_or_else(|_| Value::Object(Default::default()))
}

fn execution_chunk_from_output(
    cell_index: usize,
    cell_id: &str,
    run_id: u64,
    output: &Value,
) -> Option<NotebookExecutionChunk> {
    let output_type = output.get("output_type")?.as_str()?;
    if output_type != "stream" {
        return None;
    }
    let stream = match output.get("name").and_then(Value::as_str) {
        Some("stderr") => NotebookOutputStream::Stderr,
        _ => NotebookOutputStream::Stdout,
    };
    let text = value_text(output.get("text"))?;
    Some(NotebookExecutionChunk {
        cell_index,
        cell_id: cell_id.to_string(),
        run_id,
        stream,
        text,
    })
}

fn value_text(value: Option<&Value>) -> Option<String> {
    match value? {
        Value::String(text) => Some(text.clone()),
        Value::Array(parts) => Some(parts.iter().filter_map(Value::as_str).collect()),
        other => Some(other.to_string()),
    }
}

fn use_jupyter_for_language(language: &str) -> bool {
    matches!(
        language.trim().to_ascii_lowercase().as_str(),
        "python" | "python3" | "ipython" | "ipython3" | "rust"
    )
}

fn kernel_name_for_language(language: &str) -> &str {
    match language.trim().to_ascii_lowercase().as_str() {
        "python" | "python3" | "ipython" | "ipython3" => "python3",
        "rust" => "rust",
        _ => "python3",
    }
}

fn kernel_name_for_job(job: &NotebookExecutionJob) -> String {
    job.kernel_name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| kernel_name_for_language(&job.language).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_program_from_value_reads_argv0() {
        let value = serde_json::json!({
            "argv": ["/tmp/neoism-python", "-m", "ipykernel_launcher"],
        });

        assert_eq!(
            kernel_program_from_value(&value).unwrap(),
            "/tmp/neoism-python"
        );
    }

    #[test]
    fn kernel_program_from_value_rejects_missing_or_empty_argv0() {
        assert!(kernel_program_from_value(&serde_json::json!({})).is_err());
        assert!(kernel_program_from_value(&serde_json::json!({"argv": []})).is_err());
        assert!(kernel_program_from_value(&serde_json::json!({"argv": [""]})).is_err());
    }

    #[test]
    fn kernel_env_from_value_reads_string_env_entries() {
        let env = kernel_env_from_value(&serde_json::json!({
            "env": {
                "LD_LIBRARY_PATH": "/usr/lib",
                "NUMBER": 123,
                "EMPTY": ""
            }
        }));

        assert_eq!(
            env.get("LD_LIBRARY_PATH").map(String::as_str),
            Some("/usr/lib")
        );
        assert_eq!(env.get("EMPTY").map(String::as_str), Some(""));
        assert!(!env.contains_key("NUMBER"));
    }

    #[test]
    fn managed_python_ld_library_path_from_keeps_libstdcxx_dirs_once() {
        let tmp = tempfile::tempdir().unwrap();
        let lib_dir = tmp.path().join("lib");
        let missing_dir = tmp.path().join("missing");
        std::fs::create_dir_all(&lib_dir).unwrap();
        std::fs::write(lib_dir.join("libstdc++.so.6"), "").unwrap();

        let path = managed_python_ld_library_path_from(vec![
            missing_dir,
            lib_dir.clone(),
            lib_dir.clone(),
        ])
        .unwrap();
        let paths = std::env::split_paths(&path).collect::<Vec<_>>();

        assert_eq!(paths, vec![lib_dir]);
    }

    #[test]
    fn missing_kernel_classifier_treats_unusable_managed_spec_as_missing() {
        assert!(is_missing_kernel_error(
            "Could not find usable Jupyter kernelspec `python3`. The managed kernelspec is present but unusable"
        ));
    }

    #[test]
    fn kernel_name_for_job_prefers_notebook_kernelspec_name() {
        let job = NotebookExecutionJob {
            path: PathBuf::from("custom.ipynb"),
            cell_index: 0,
            cell_id: "cell-0".to_string(),
            run_id: 1,
            execution_count: 1,
            language: "python".to_string(),
            kernel_name: Some("conda-env-analysis-py".to_string()),
            source: String::new(),
            fallback_script: String::new(),
        };

        assert_eq!(kernel_name_for_job(&job), "conda-env-analysis-py");
    }

    #[test]
    fn kernel_name_for_job_falls_back_to_language_default() {
        let job = NotebookExecutionJob {
            path: PathBuf::from("default.ipynb"),
            cell_index: 0,
            cell_id: "cell-0".to_string(),
            run_id: 1,
            execution_count: 1,
            language: "python".to_string(),
            kernel_name: None,
            source: String::new(),
            fallback_script: String::new(),
        };

        assert_eq!(kernel_name_for_job(&job), "python3");
    }

    #[test]
    fn rust_language_uses_jupyter_kernel() {
        assert!(use_jupyter_for_language("rust"));
        assert_eq!(kernel_name_for_language("rust"), "rust");
    }

    #[test]
    fn kernel_name_for_job_uses_rust_kernelspec_name() {
        let job = NotebookExecutionJob {
            path: PathBuf::from("rust.ipynb"),
            cell_index: 0,
            cell_id: "cell-0".to_string(),
            run_id: 1,
            execution_count: 1,
            language: "rust".to_string(),
            kernel_name: Some("rust".to_string()),
            source: String::new(),
            fallback_script: String::new(),
        };

        assert_eq!(kernel_name_for_job(&job), "rust");
    }

    #[test]
    fn jupyter_runtime_error_output_for_missing_kernel_is_actionable() {
        let output = jupyter_runtime_error_output(
            "Could not find Jupyter kernelspec `python3`: KernelNotFound",
        );
        let traceback = output["traceback"].as_array().unwrap();

        assert_eq!(output["ename"], "JupyterKernelUnavailable");
        assert!(traceback
            .iter()
            .any(|line| line.as_str().unwrap().contains("Could not find")));
        assert!(traceback
            .iter()
            .any(|line| line.as_str().unwrap().contains("Install or reinstall")));
    }

    #[test]
    fn jupyter_runtime_error_output_for_startup_failure_keeps_details() {
        let output = jupyter_runtime_error_output(
            "Could not connect to Jupyter shell channel: connection refused",
        );
        let traceback = output["traceback"].as_array().unwrap();

        assert_eq!(output["ename"], "JupyterExecutionError");
        assert!(traceback
            .iter()
            .any(|line| line.as_str().unwrap().contains("connection refused")));
        assert!(traceback
            .iter()
            .any(|line| line.as_str().unwrap().contains("did not start or respond")));
    }

    #[test]
    fn kernel_startup_error_includes_captured_stderr_and_stdout() {
        let output = KernelProcessOutput {
            stderr: "Traceback: missing module\n".to_string(),
            stdout: "boot banner\n".to_string(),
        };
        let err = with_kernel_process_output(
            "Could not connect to Jupyter shell channel: connection refused".to_string(),
            &output,
        );

        assert!(err.contains("Could not connect"));
        assert!(err.contains("Kernel stderr:\nTraceback: missing module"));
        assert!(err.contains("Kernel stdout:\nboot banner"));
    }

    #[test]
    fn kernel_startup_error_omits_empty_process_output() {
        let err = with_kernel_process_output(
            "Could not connect to Jupyter shell channel: connection refused".to_string(),
            &KernelProcessOutput::default(),
        );

        assert_eq!(
            err,
            "Could not connect to Jupyter shell channel: connection refused"
        );
    }

    #[test]
    fn display_data_json_preserves_transient_display_id() {
        let display = serde_json::from_value::<DisplayData>(serde_json::json!({
            "data": {"text/plain": "old"},
            "metadata": {},
            "transient": {"display_id": "plot-1"}
        }))
        .unwrap();

        let output = display_data_json(&display);

        assert_eq!(output["output_type"], "display_data");
        assert_eq!(output["data"]["text/plain"], "old");
        assert_eq!(output["transient"]["display_id"], "plot-1");
    }

    #[test]
    fn update_display_data_replaces_matching_display_outputs() {
        let mut outputs = vec![
            serde_json::json!({
                "output_type": "display_data",
                "data": {"text/plain": "old"},
                "metadata": {},
                "transient": {"display_id": "plot-1"}
            }),
            serde_json::json!({
                "output_type": "stream",
                "name": "stdout",
                "text": "keep me\n"
            }),
        ];
        let update = serde_json::from_value::<UpdateDisplayData>(serde_json::json!({
            "data": {"text/plain": "new"},
            "metadata": {"text/plain": {}},
            "transient": {"display_id": "plot-1"}
        }))
        .unwrap();

        apply_update_display_data(&mut outputs, &update);

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[0]["output_type"], "display_data");
        assert_eq!(outputs[0]["data"]["text/plain"], "new");
        assert_eq!(outputs[0]["transient"]["display_id"], "plot-1");
        assert_eq!(outputs[1]["text"], "keep me\n");
    }

    #[test]
    fn update_display_data_with_unknown_display_id_is_ignored() {
        let mut outputs = vec![serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": "current cell\n"
        })];
        let update = serde_json::from_value::<UpdateDisplayData>(serde_json::json!({
            "data": {"text/plain": "previous cell update"},
            "metadata": {},
            "transient": {"display_id": "other-cell"}
        }))
        .unwrap();

        apply_update_display_data(&mut outputs, &update);

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0]["text"], "current cell\n");
    }

    #[test]
    fn update_display_data_builds_cross_cell_update_event() {
        let job = NotebookExecutionJob {
            path: PathBuf::from("display.ipynb"),
            cell_index: 2,
            cell_id: "cell-2".to_string(),
            run_id: 99,
            execution_count: 4,
            language: "python".to_string(),
            kernel_name: None,
            source: "handle.update('new')".to_string(),
            fallback_script: String::new(),
        };
        let update = serde_json::from_value::<UpdateDisplayData>(serde_json::json!({
            "data": {"text/plain": "new"},
            "metadata": {},
            "transient": {"display_id": "plot-1"}
        }))
        .unwrap();

        let Some(NotebookExecutionEvent::DisplayUpdate(event)) =
            display_update_event_from_update(&job, &update)
        else {
            panic!("expected display update event");
        };

        assert_eq!(event.cell_index, 2);
        assert_eq!(event.cell_id, "cell-2");
        assert_eq!(event.run_id, 99);
        assert_eq!(event.display_id, "plot-1");
        assert_eq!(event.output["data"]["text/plain"], "new");
        assert_eq!(event.output["transient"]["display_id"], "plot-1");
    }

    #[test]
    fn clear_output_honors_immediate_and_waiting_modes() {
        let mut outputs = vec![serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": "old\n"
        })];
        let mut clear_waiting = false;

        apply_clear_output(
            &mut outputs,
            &mut clear_waiting,
            &ClearOutput { wait: true },
        );
        assert_eq!(outputs.len(), 1);
        assert!(clear_waiting);

        apply_pending_clear_output(&mut outputs, &mut clear_waiting);
        assert!(outputs.is_empty());
        assert!(!clear_waiting);

        outputs.push(serde_json::json!({
            "output_type": "stream",
            "name": "stdout",
            "text": "again\n"
        }));
        apply_clear_output(
            &mut outputs,
            &mut clear_waiting,
            &ClearOutput { wait: false },
        );
        assert!(outputs.is_empty());
        assert!(!clear_waiting);
    }

    #[test]
    fn interrupt_kernel_reports_missing_active_kernel() {
        let manager = NotebookRuntimeManager::new();
        let err = manager
            .interrupt_kernel(Path::new("missing.ipynb"))
            .unwrap_err();

        assert_eq!(err, "No active notebook process for this notebook");
    }

    #[test]
    fn active_process_registry_tracks_local_process_group_flag() {
        let registry = ActiveNotebookProcessRegistry::default();
        let path = Path::new("local.ipynb");

        registry.register_pid(path, 42, true).unwrap();

        assert_eq!(
            registry.get(path).unwrap(),
            Some(ActiveNotebookProcess {
                pid: 42,
                signal_process_group: true
            })
        );
        registry.remove(path);
        assert_eq!(registry.get(path).unwrap(), None);
    }
}
