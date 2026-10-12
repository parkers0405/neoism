//! Nonblocking native facade over the separately packaged Chromium worker.
//! This module has no browser/native graphics dependency. Pipe I/O never runs on the GUI thread.
use std::collections::HashMap;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc::{self, SyncSender, TrySendError},
    Arc, Mutex,
};
use std::thread;

use crate::{
    ipc::{self, Control, Output},
    ArtifactDocument, ArtifactFrame, ArtifactInput, ArtifactLayoutMetrics, Error,
    PumpOutput, SandboxStatus, Viewport, MAX_VIEWS, SANDBOX_STATUS,
};
const QUEUE_BYTES: usize = 64 * 1024 * 1024;
const QUEUE_COMMANDS: usize = 32;
type Wake = Arc<dyn Fn() + Send + Sync>;

struct Notification {
    pending: AtomicBool,
    wake: Wake,
}
impl Notification {
    fn new(wake: Wake) -> Self {
        Self {
            pending: AtomicBool::new(false),
            wake,
        }
    }
    // Publish and reset only while holding Inbound's mutex. A producer after
    // drain always observes false; a producer before drain is included in it.
    fn publish(&self) -> bool {
        !self.pending.swap(true, Ordering::AcqRel)
    }
    fn reset(&self) {
        self.pending.store(false, Ordering::Release);
    }
    fn wake_if(&self, notify: bool) {
        if notify {
            (self.wake)();
        }
    }
}

#[derive(Default)]
struct Inbound {
    desired: HashMap<String, (u64, u64, Viewport, bool)>,
    frames: HashMap<String, ArtifactFrame>,
    sequences: HashMap<String, u64>,
    metrics: HashMap<String, ArtifactLayoutMetrics>,
    metric_heights: HashMap<String, f32>,
    diagnostics: Vec<String>,
    animating: bool,
    error: Option<String>,
}
impl Inbound {
    fn note(&mut self, message: String) {
        if self.diagnostics.len() < 64 {
            self.diagnostics.push(message.chars().take(2048).collect());
        }
    }
    fn drain(&mut self, alive: bool) -> Result<PumpOutput, Error> {
        if let Some(error) = &self.error {
            return Err(Error::Backend(error.clone()));
        }
        if !alive {
            return Err(Error::Backend("Chromium helper is not running".into()));
        }
        Ok(PumpOutput {
            frames: std::mem::take(&mut self.frames).into_values().collect(),
            layout_metrics: std::mem::take(&mut self.metrics).into_values().collect(),
            diagnostics: std::mem::take(&mut self.diagnostics),
            animating: self.animating
                && self.desired.values().any(|(_, _, _, visible)| *visible),
        })
    }
    fn accept_metric(
        &mut self,
        key: String,
        generation: u64,
        revision: u64,
        height: f32,
    ) -> bool {
        if key.is_empty()
            || key.len() > 1024
            || !height.is_finite()
            || height <= 0.0
            || height > 8192.0
        {
            return false;
        }
        if !self.desired.get(&key).is_some_and(|(g, r, _, visible)| {
            *g == generation && *r == revision && *visible
        }) {
            return false;
        }
        if self.metric_heights.get(&key) == Some(&height) {
            return false;
        }
        self.metric_heights.insert(key.clone(), height);
        self.metrics.insert(
            key.clone(),
            ArtifactLayoutMetrics {
                key,
                revision,
                height,
            },
        );
        true
    }
    fn accept(&mut self, frame: ArtifactFrame, generation: u64) -> bool {
        if let Some((expected_generation, revision, viewport, visible)) =
            self.desired.get(&frame.key)
        {
            if *expected_generation == generation
                && *revision == frame.revision
                && *visible
                && viewport.width == frame.width
                && viewport.height == frame.height
            {
                if self
                    .sequences
                    .get(&frame.key)
                    .is_some_and(|old| *old >= frame.sequence)
                {
                    return false;
                }
                self.sequences.insert(frame.key.clone(), frame.sequence);
                self.frames.insert(frame.key.clone(), frame);
                return true;
            }
        }
        false
    }
}

/// Thread-affine facade with bounded asynchronous request and latest-per-key frame mailboxes.
/// `new` launches the helper, but never blocks awaiting its engine initialization or frames.
/// The caller can disable page execution; the worker requests sandboxing by default.
pub struct Host {
    sender: Option<SyncSender<Vec<u8>>>,
    queued: Arc<AtomicUsize>,
    alive: Arc<AtomicBool>,
    inbound: Arc<Mutex<Inbound>>,
    notification: Arc<Notification>,
    child: Arc<Mutex<Child>>,
    documents: HashMap<String, ArtifactDocument>,
    pending_destroy: Vec<String>,
    next_generation: u64,
    latest: HashMap<String, ArtifactFrame>,
    // Enforce event-thread usage independently of the thread-safe mailbox internals.
    _thread_affine: std::marker::PhantomData<std::rc::Rc<()>>,
}
impl Host {
    pub fn new(waker: Wake) -> Result<Self, Error> {
        let notification = Arc::new(Notification::new(waker));
        let path = runtime_path()?;
        let mut child = spawn_worker(&path)?;
        let Some(stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Backend("Helper stdin unavailable".into()));
        };
        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(Error::Backend("Helper stdout unavailable".into()));
        };
        let child = Arc::new(Mutex::new(child));
        let inbound = Arc::new(Mutex::new(Inbound::default()));
        let queued = Arc::new(AtomicUsize::new(0));
        let alive = Arc::new(AtomicBool::new(true));
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(QUEUE_COMMANDS);
        let writer_queued = queued.clone();
        let writer_alive = alive.clone();
        let writer_inbound = inbound.clone();
        let writer_child = child.clone();
        let writer_wake = notification.clone();
        if let Err(error) = thread::Builder::new()
            .name("cef-artifacts-write".into())
            .spawn(move || {
                let mut writer = BufWriter::new(stdin);
                while let Ok(bytes) = receiver.recv() {
                    writer_queued.fetch_sub(bytes.len(), Ordering::AcqRel);
                    if let Err(error) = ipc::write_encoded_json(&mut writer, &bytes)
                        .and_then(|_| writer.flush())
                    {
                        fail(
                            &writer_inbound,
                            &writer_alive,
                            &writer_wake,
                            format!("Chromium helper input pipe: {error}"),
                        );
                        terminate(writer_child.clone());
                        break;
                    }
                }
            })
        {
            terminate(child.clone());
            return Err(Error::Backend(format!(
                "Cannot start Chromium IPC writer: {error}"
            )));
        }
        let reader_inbound = inbound.clone();
        let reader_alive = alive.clone();
        let reader_child = child.clone();
        let reader_wake = notification.clone();
        if let Err(error) = thread::Builder::new()
            .name("cef-artifacts-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let result = read_worker(&mut reader, &reader_inbound, &reader_wake);
                let message = match result {
                    Ok(()) => "Chromium helper exited or closed its output pipe".into(),
                    Err(error) => format!("Chromium helper protocol failure: {error}"),
                };
                if reader_alive.load(Ordering::Acquire) {
                    fail(&reader_inbound, &reader_alive, &reader_wake, message);
                }
                // Close/reap failed workers too; never leave zombie child processes behind.
                terminate(reader_child);
            })
        {
            alive.store(false, Ordering::Release);
            drop(sender);
            terminate(child.clone());
            return Err(Error::Backend(format!(
                "Cannot start Chromium IPC reader: {error}"
            )));
        }
        Ok(Self {
            sender: Some(sender),
            queued,
            alive,
            inbound,
            notification,
            child,
            documents: HashMap::new(),
            pending_destroy: Vec::new(),
            next_generation: 1,
            latest: HashMap::new(),
            _thread_affine: std::marker::PhantomData,
        })
    }
    pub fn sandbox_status(&self) -> SandboxStatus {
        SANDBOX_STATUS
    }
    fn send(&self, control: &Control) -> Result<(), Error> {
        if !self.alive.load(Ordering::Acquire) {
            return Err(Error::Backend(
                "Chromium helper is not running; inspect pump diagnostics".into(),
            ));
        }
        let bytes =
            ipc::encode_json(control).map_err(|e| Error::Backend(e.to_string()))?;
        let size = bytes.len();
        self.queued
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |used| {
                used.checked_add(size).filter(|sum| *sum <= QUEUE_BYTES)
            })
            .map_err(|_| {
                Error::Backend(
                    "Chromium helper request byte budget full; retry on next tick".into(),
                )
            })?;
        let Some(sender) = &self.sender else {
            self.queued.fetch_sub(size, Ordering::AcqRel);
            return Err(Error::Backend("Chromium helper shut down".into()));
        };
        match sender.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.queued.fetch_sub(size, Ordering::AcqRel);
                Err(Error::Backend(
                    match error {
                        TrySendError::Full(_) => {
                            "Chromium helper request queue full; retry on next tick"
                        }
                        TrySendError::Disconnected(_) => {
                            "Chromium helper request pipe disconnected"
                        }
                    }
                    .into(),
                ))
            }
        }
    }
    fn generation(&mut self) -> Result<u64, Error> {
        let generation = self.next_generation;
        self.next_generation = generation.checked_add(1).ok_or_else(|| {
            Error::Backend("Chromium request generation exhausted".into())
        })?;
        Ok(generation)
    }
    fn flush_destroy(&mut self) {
        while let Some(key) = self.pending_destroy.last() {
            if self.send(&Control::Destroy { key: key.clone() }).is_err() {
                break;
            }
            self.pending_destroy.pop();
        }
    }
    pub fn reconcile(&mut self, document: ArtifactDocument) -> Result<(), Error> {
        document.validate()?;
        self.flush_destroy();
        if !self.pending_destroy.is_empty() {
            return Err(Error::Backend(
                "Pending Chromium destroys; retry next tick".into(),
            ));
        }
        if let Some(previous) = self.documents.get(&document.key) {
            if previous.revision == document.revision && previous.html != document.html {
                return Err(Error::RevisionConflict);
            }
            if previous.revision == document.revision
                && previous.viewport == document.viewport
                && previous.visible == document.visible
                && previous.theme == document.theme
                && previous.styles == document.styles
            {
                return Ok(());
            }
        } else if self.documents.len() >= MAX_VIEWS {
            return Err(Error::TooManyViews);
        }
        // Lock only short mailbox updates, never OS pipe reads/writes. Register before send
        // so a very fast worker cannot publish a frame before the desired revision exists.
        let generation = self.generation()?;
        let mut state = self
            .inbound
            .lock()
            .map_err(|_| Error::Backend("Chromium mailbox poisoned".into()))?;
        self.send(&Control::Reconcile {
            document: document.clone(),
            generation,
        })?;
        self.latest.remove(&document.key);
        state.frames.remove(&document.key);
        state.sequences.remove(&document.key);
        state.metrics.remove(&document.key);
        state.metric_heights.remove(&document.key);
        state.desired.insert(
            document.key.clone(),
            (
                generation,
                document.revision,
                document.viewport,
                document.visible,
            ),
        );
        self.documents.insert(document.key.clone(), document);
        Ok(())
    }
    pub fn resize(&mut self, key: &str, viewport: Viewport) -> Result<(), Error> {
        viewport.validate()?;
        if self
            .documents
            .get(key)
            .is_some_and(|document| document.viewport == viewport)
        {
            return Ok(());
        }
        let generation = self.generation()?;
        if !self.documents.contains_key(key) {
            return Err(Error::UnknownArtifact);
        }
        let mut state = self
            .inbound
            .lock()
            .map_err(|_| Error::Backend("Chromium mailbox poisoned".into()))?;
        self.send(&Control::Resize {
            key: key.into(),
            viewport,
            generation,
        })?;
        if let Some(document) = self.documents.get_mut(key) {
            document.viewport = viewport;
            state.desired.insert(
                key.into(),
                (generation, document.revision, viewport, document.visible),
            );
        }
        state.frames.remove(key);
        state.sequences.remove(key);
        state.metrics.remove(key);
        state.metric_heights.remove(key);
        self.latest.remove(key);
        Ok(())
    }
    pub fn input(&mut self, key: &str, input: ArtifactInput) -> Result<(), Error> {
        input.validate()?;
        if !self.documents.contains_key(key) {
            return Err(Error::UnknownArtifact);
        }
        self.send(&Control::Input {
            key: key.into(),
            input,
        })
    }
    /// Drains already received frames only; never waits on the helper or its pipes.
    /// The helper pumps independently on wakes/display ticks, so this sends no poll command.
    pub fn pump(&mut self) -> Result<PumpOutput, Error> {
        self.flush_destroy();
        let mut state = self
            .inbound
            .lock()
            .map_err(|_| Error::Backend("Chromium mailbox poisoned".into()))?;
        self.notification.reset();
        let output = state.drain(self.alive.load(Ordering::Acquire))?;
        for frame in &output.frames {
            self.latest.insert(frame.key.clone(), frame.clone());
        }
        Ok(output)
    }
    pub fn snapshot(&self, key: &str) -> Option<ArtifactFrame> {
        self.latest.get(key).cloned()
    }
    pub fn destroy(&mut self, key: &str) -> bool {
        if !self.documents.contains_key(key) {
            return false;
        }
        // API compatibility: destroy cannot return a queue error. Report it via diagnostics;
        // clear the local key immediately so late worker frames can never resurrect it.
        let result = self.send(&Control::Destroy { key: key.into() });
        if result.is_err() {
            self.pending_destroy.push(key.into());
        }
        self.documents.remove(key);
        self.latest.remove(key);
        let notify = if let Ok(mut state) = self.inbound.lock() {
            state.desired.remove(key);
            state.frames.remove(key);
            state.sequences.remove(key);
            state.metrics.remove(key);
            state.metric_heights.remove(key);
            if let Err(error) = result {
                let before = state.diagnostics.len();
                state.note(format!("Destroy: {error}"));
                state.diagnostics.len() != before && self.notification.publish()
            } else {
                false
            }
        } else {
            false
        };
        self.notification.wake_if(notify);
        true
    }
    pub fn shutdown(self) {
        drop(self);
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        let _ = self.send(&Control::Shutdown);
        self.alive.store(false, Ordering::Release);
        self.sender.take();
        // Never join blocked pipe threads or wait for Chromium on the GUI thread. Shutdown
        // gets 500ms for graceful drain, then the background reaper kills/waits the worker.
        let child = self.child.clone();
        let _ = thread::Builder::new()
            .name("cef-artifacts-reap".into())
            .spawn(move || {
                thread::sleep(std::time::Duration::from_millis(500));
                terminate(child);
            });
    }
}
fn spawn_worker(path: &std::path::Path) -> Result<Child, Error> {
    let mut command = Command::new(path);
    command
        .args(["--stdio", "--experimental"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    // Development CEF binaries live outside the executable directory. Set the
    // loader path for this child only; never change the desktop's library path.
    if let Some(directory) = cef_directory() {
        // Keep Chromium libraries and resource lookup in the worker process.
        command.env("NEOISM_CEF_DIR", &directory);
        #[cfg(target_os = "linux")]
        let variable = "LD_LIBRARY_PATH";
        #[cfg(target_os = "macos")]
        let variable = "DYLD_FALLBACK_LIBRARY_PATH";
        #[cfg(target_os = "windows")]
        let variable = "PATH";
        #[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
        {
            let mut paths = vec![PathBuf::from(directory)];
            if let Some(existing) = std::env::var_os(variable) {
                paths.extend(std::env::split_paths(&existing));
            }
            let joined = std::env::join_paths(paths).map_err(|error| {
                Error::Backend(format!("Invalid CEF library path: {error}"))
            })?;
            command.env(variable, joined);
        }
    }
    command.spawn().map_err(|e| Error::Backend(format!("Cannot launch Chromium helper {}: {e}. Set NEOISM_CEF_RUNTIME to the separately installed neoism-cef-runtime executable", path.display())))
}
fn cef_directory() -> Option<PathBuf> {
    if let Some(path) =
        std::env::var_os("NEOISM_CEF_DIR").or_else(|| std::env::var_os("CEF_PATH"))
    {
        return Some(PathBuf::from(path));
    }
    #[cfg(debug_assertions)]
    if let Some(home) = std::env::var_os("HOME") {
        let cached = PathBuf::from(home).join(".local/share/cef");
        if cached.is_dir() {
            return Some(cached);
        }
    }
    None
}
/// Desktop auto-activation only checks worker presence, not launchability or
/// sandbox readiness. Cache the probe so rendering never stats the filesystem.
/// Explicit `=1` bypasses it: Host::new re-resolves the path on every attempt,
/// preserving unavailable-worker diagnostics and the existing edit-to-retry path.
/// Installations made after the first automatic probe require an app restart.
pub fn html_artifacts_enabled() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let flag = std::env::var("NEOISM_HTML_ARTIFACTS").ok();
    html_artifacts_enabled_with(flag.as_deref(), || {
        *AVAILABLE.get_or_init(|| worker_present(runtime_path().ok().as_deref()))
    })
}

fn worker_present(path: Option<&std::path::Path>) -> bool {
    path.is_some_and(std::path::Path::is_file)
}

fn html_artifacts_enabled_with(
    flag: Option<&str>,
    available: impl FnOnce() -> bool,
) -> bool {
    match flag {
        Some("0") => false,
        Some("1") => true,
        _ => available(),
    }
}

fn runtime_path() -> Result<PathBuf, Error> {
    if let Some(path) = std::env::var_os("NEOISM_CEF_RUNTIME") {
        if path.is_empty() {
            return Err(Error::Backend("NEOISM_CEF_RUNTIME is empty".into()));
        }
        return Ok(PathBuf::from(path));
    }
    let executable =
        std::env::current_exe().map_err(|e| Error::Backend(e.to_string()))?;
    let parent = executable.parent().ok_or_else(|| {
        Error::Backend("Cannot locate Neoism executable directory".into())
    })?;
    let name = if cfg!(windows) {
        "neoism-cef-runtime.exe"
    } else {
        "neoism-cef-runtime"
    };
    let bundled = parent.join(name);
    // A native debug build prepares this worker automatically. Prefer it over
    // an older packaged helper left beside the desktop executable.
    #[cfg(debug_assertions)]
    {
        let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../cef-runtime/target/debug")
            .join(name);
        if development.is_file() {
            return Ok(development);
        }
    }
    Ok(bundled)
}
fn terminate(child: Arc<Mutex<Child>>) {
    if let Ok(mut child) = child.lock() {
        if !matches!(child.try_wait(), Ok(Some(_))) {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}
fn fail(
    state: &Mutex<Inbound>,
    alive: &AtomicBool,
    wake: &Notification,
    message: String,
) {
    alive.store(false, Ordering::Release);
    let notify = if let Ok(mut state) = state.lock() {
        state.error = Some(message);
        state.animating = false;
        wake.publish()
    } else {
        true
    };
    wake.wake_if(notify);
}
fn read_worker(
    reader: &mut impl std::io::Read,
    inbound: &Mutex<Inbound>,
    wake: &Notification,
) -> std::io::Result<()> {
    match ipc::read_json::<Output>(reader)? {
        Some(Output::Hello { version }) if version == ipc::PROTOCOL_VERSION => (),
        _ => {
            return Err(ipc::invalid(
                "Missing/incompatible worker protocol handshake",
            ))
        }
    }
    loop {
        let Some(message) = ipc::read_json::<Output>(reader)? else {
            return Ok(());
        };
        match message {
            Output::Hello { .. } => {
                return Err(ipc::invalid("Repeated worker handshake"))
            }
            Output::Fatal(message) => {
                return Err(ipc::invalid(
                    &message.chars().take(2048).collect::<String>(),
                ))
            }
            Output::Frame(meta) => {
                let bytes = ipc::read_frame(reader, &meta)?;
                let frame = ArtifactFrame {
                    key: meta.key,
                    revision: meta.revision,
                    sequence: meta.sequence,
                    width: meta.width,
                    height: meta.height,
                    stride: meta.stride,
                    rgba: bytes.into(),
                };
                let notify = if let Ok(mut state) = inbound.lock() {
                    state.accept(frame, meta.generation) && wake.publish()
                } else {
                    false
                };
                wake.wake_if(notify);
            }
            Output::ContentHeight {
                key,
                generation,
                revision,
                height,
            } => {
                let notify = if let Ok(mut state) = inbound.lock() {
                    state.accept_metric(key, generation, revision, height)
                        && wake.publish()
                } else {
                    false
                };
                wake.wake_if(notify);
            }
            Output::Status {
                animating,
                diagnostics,
            } => {
                let notify = if let Ok(mut state) = inbound.lock() {
                    state.animating = animating;
                    let before = state.diagnostics.len();
                    for diagnostic in diagnostics.into_iter().take(64) {
                        state.note(diagnostic);
                    }
                    state.diagnostics.len() != before && wake.publish()
                } else {
                    false
                };
                wake.wake_if(notify);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_artifact_activation_respects_override_and_worker_presence() {
        // Inject paths and flags: never mutate process-global environment in tests.
        let executable = std::env::current_exe().unwrap();
        let missing = executable.join("not-a-worker");
        assert!(worker_present(Some(&executable)));
        assert!(!worker_present(Some(&missing)));
        assert!(!worker_present(executable.parent()));
        assert!(!worker_present(None));
        for flag in [None, Some(""), Some("true")] {
            assert!(html_artifacts_enabled_with(flag, || worker_present(Some(
                &executable
            ))));
            assert!(!html_artifacts_enabled_with(flag, || worker_present(Some(
                &missing
            ))));
        }
        assert!(html_artifacts_enabled_with(Some("1"), || panic!(
            "opt-in must skip probe"
        )));
        assert!(!html_artifacts_enabled_with(Some("0"), || panic!(
            "disable must skip probe"
        )));
    }

    fn frame(revision: u64) -> ArtifactFrame {
        ArtifactFrame {
            key: "a".into(),
            revision,
            sequence: 1,
            width: 1,
            height: 1,
            stride: 4,
            rgba: vec![0; 4].into(),
        }
    }
    #[test]
    fn v2_metric_frame_packet_order_is_independent_and_v1_is_rejected() {
        let wake = Notification::new(Arc::new(|| {}));
        let old = Mutex::new(Inbound::default());
        let mut bytes = Vec::new();
        ipc::write_json(&mut bytes, &Output::Hello { version: 1 }).unwrap();
        assert!(read_worker(&mut std::io::Cursor::new(bytes), &old, &wake).is_err());
        for metric_first in [true, false] {
            let inbound = Mutex::new(Inbound::default());
            inbound.lock().unwrap().desired.insert(
                "a".into(),
                (
                    1,
                    1,
                    Viewport {
                        width: 1,
                        height: 1,
                        scale: 1.,
                    },
                    true,
                ),
            );
            let mut bytes = Vec::new();
            ipc::write_json(
                &mut bytes,
                &Output::Hello {
                    version: ipc::PROTOCOL_VERSION,
                },
            )
            .unwrap();
            let meta = ipc::FrameMetadata {
                key: "a".into(),
                generation: 1,
                revision: 1,
                sequence: 1,
                width: 1,
                height: 1,
                stride: 4,
                bytes: 4,
            };
            let metric = Output::ContentHeight {
                key: "a".into(),
                generation: 1,
                revision: 1,
                height: 37.5,
            };
            if metric_first {
                ipc::write_json(&mut bytes, &metric).unwrap();
            }
            ipc::write_frame(&mut bytes, &meta, &[0; 4]).unwrap();
            if !metric_first {
                ipc::write_json(&mut bytes, &metric).unwrap();
            }
            read_worker(&mut std::io::Cursor::new(bytes), &inbound, &wake).unwrap();
            let output = inbound.lock().unwrap().drain(true).unwrap();
            assert_eq!(output.frames.len(), 1);
            assert_eq!(output.layout_metrics[0].height, 37.5);
        }
    }
    #[test]
    fn metric_wire_wakes_once_and_static_repeats_do_not_wake_after_drain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let wake = Notification::new(Arc::new(move || {
            count.fetch_add(1, Ordering::Relaxed);
        }));
        let inbound = Mutex::new(Inbound::default());
        inbound.lock().unwrap().desired.insert(
            "a".into(),
            (
                1,
                1,
                Viewport {
                    width: 1,
                    height: 1,
                    scale: 1.,
                },
                true,
            ),
        );
        let run = |heights: &[f32]| {
            let mut bytes = Vec::new();
            ipc::write_json(
                &mut bytes,
                &Output::Hello {
                    version: ipc::PROTOCOL_VERSION,
                },
            )
            .unwrap();
            for height in heights {
                ipc::write_json(
                    &mut bytes,
                    &Output::ContentHeight {
                        key: "a".into(),
                        generation: 1,
                        revision: 1,
                        height: *height,
                    },
                )
                .unwrap();
            }
            read_worker(&mut std::io::Cursor::new(bytes), &inbound, &wake).unwrap();
        };
        run(&[0., 8193., 40., 50., 50.]);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        {
            let mut s = inbound.lock().unwrap();
            wake.reset();
            assert_eq!(s.drain(true).unwrap().layout_metrics[0].height, 50.);
        }
        run(&[50., 50.]);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        run(&[60.]);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }
    #[test]
    fn metrics_are_bounded_transaction_scoped_coalesced_and_independent_of_frames() {
        let mut s = Inbound::default();
        let vp = Viewport {
            width: 1,
            height: 1,
            scale: 1.25,
        };
        s.desired.insert("a".into(), (7, 9, vp, true));
        for h in [f32::NAN, f32::INFINITY, -1., 0., 8192.5] {
            assert!(!s.accept_metric("a".into(), 7, 9, h));
        }
        assert!(!s.accept_metric("a".into(), 6, 9, 40.));
        assert!(!s.accept_metric("a".into(), 7, 8, 40.));
        assert!(!s.accept_metric("gone".into(), 7, 9, 40.));
        assert!(s.accept_metric("a".into(), 7, 9, 40.));
        assert!(!s.accept_metric("a".into(), 7, 9, 40.));
        assert!(s.accept_metric("a".into(), 7, 9, 50.));
        assert!(s.accept(frame(9), 7));
        let output = s.drain(true).unwrap();
        assert_eq!(output.frames.len(), 1);
        assert_eq!(output.layout_metrics.len(), 1);
        assert_eq!(output.layout_metrics[0].height, 50.);
        assert!(!s.accept_metric("a".into(), 7, 9, 50.));
        s.desired.insert("a".into(), (8, 9, vp, false));
        assert!(!s.accept_metric("a".into(), 8, 9, 70.));
        s.desired.clear();
        assert!(!s.accept_metric("a".into(), 8, 9, 70.));
        // A new geometry/revision resets deduplication, even if height repeats.
        s.metric_heights.clear();
        s.desired.insert("a".into(), (9, 10, vp, true));
        assert!(s.accept_metric("a".into(), 9, 10, 50.));
        let mut f = frame(10);
        f.sequence = 2;
        assert!(s.accept(f, 9));
        assert!(s.accept_metric("a".into(), 9, 10, 60.));
        assert_eq!(s.drain(true).unwrap().layout_metrics[0].height, 60.);
    }
    #[test]
    fn mailbox_burst_coalesces_and_drain_rearms_without_lost_damage() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let notification = Arc::new(Notification::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        })));
        let state = Arc::new(Mutex::new(Inbound::default()));
        for _ in 0..100 {
            let mut state = state.lock().unwrap();
            state.note("damage".into());
            notification.wake_if(notification.publish());
        }
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        // Force a producer to contend with a GUI drain. Reset under the same
        // mailbox lock ensures the blocked producer re-arms after the drain.
        let mut guard = state.lock().unwrap();
        let producer_state = state.clone();
        let producer_notification = notification.clone();
        let barrier = Arc::new(std::sync::Barrier::new(2));
        let producer_barrier = barrier.clone();
        let producer = thread::spawn(move || {
            producer_barrier.wait();
            let mut state = producer_state.lock().unwrap();
            state.note("after drain".into());
            let notify = producer_notification.publish();
            drop(state);
            producer_notification.wake_if(notify);
        });
        barrier.wait();
        notification.reset();
        assert_eq!(guard.drain(true).unwrap().diagnostics.len(), 64);
        drop(guard);
        producer.join().unwrap();
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        let mut state = state.lock().unwrap();
        notification.reset();
        assert_eq!(state.drain(true).unwrap().diagnostics, ["after drain"]);
    }

    #[test]
    fn only_new_frames_and_diagnostics_wake_not_status_or_rejected_frames() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let notification = Notification::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let mut state = Inbound::default();
        state.desired.insert(
            "a".into(),
            (
                1,
                1,
                Viewport {
                    width: 1,
                    height: 1,
                    scale: 1.0,
                },
                true,
            ),
        );
        let inbound = Mutex::new(state);
        let run = |messages: Vec<Output>| {
            let mut bytes = Vec::new();
            ipc::write_json(
                &mut bytes,
                &Output::Hello {
                    version: ipc::PROTOCOL_VERSION,
                },
            )
            .unwrap();
            for message in messages {
                if let Output::Frame(meta) = message {
                    ipc::write_frame(&mut bytes, &meta, &[0; 4]).unwrap();
                } else {
                    ipc::write_json(&mut bytes, &message).unwrap();
                }
            }
            read_worker(&mut std::io::Cursor::new(bytes), &inbound, &notification)
                .unwrap();
        };
        let meta = ipc::FrameMetadata {
            key: "a".into(),
            generation: 1,
            revision: 1,
            sequence: 1,
            width: 1,
            height: 1,
            stride: 4,
            bytes: 4,
        };
        run(vec![
            Output::Status {
                animating: true,
                diagnostics: vec![],
            },
            Output::Frame(meta.clone()),
            Output::Status {
                animating: true,
                diagnostics: vec![],
            },
        ]);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        {
            let mut state = inbound.lock().unwrap();
            notification.reset();
            assert_eq!(state.drain(true).unwrap().frames.len(), 1);
        }
        run(vec![
            Output::Frame(meta.clone()),
            Output::Status {
                animating: false,
                diagnostics: vec![],
            },
        ]);
        assert_eq!(calls.load(Ordering::Relaxed), 1); // Duplicate even after drain.
        let mut stale = meta.clone();
        stale.generation = 2;
        run(vec![Output::Frame(stale)]);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        run(vec![Output::Status {
            animating: false,
            diagnostics: vec!["warning".into()],
        }]);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn worker_failures_coalesce_and_remain_observable_after_drain() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let notification = Notification::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let inbound = Mutex::new(Inbound::default());
        let alive = AtomicBool::new(true);
        fail(&inbound, &alive, &notification, "input pipe".into());
        fail(&inbound, &alive, &notification, "output pipe".into());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let mut state = inbound.lock().unwrap();
        notification.reset();
        assert!(!alive.load(Ordering::Acquire));
        assert!(matches!(state.drain(false), Err(Error::Backend(_))));
        assert!(matches!(state.drain(true), Err(Error::Backend(_))));
    }

    #[test]
    fn latest_frame_burst_is_one_notification_and_survives_delayed_wake() {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let notification = Notification::new(Arc::new(move || {
            counter.fetch_add(1, Ordering::Relaxed);
        }));
        let inbound = Mutex::new(Inbound::default());
        let mut state = inbound.lock().unwrap();
        state.desired.insert(
            "a".into(),
            (
                1,
                1,
                Viewport {
                    width: 1,
                    height: 1,
                    scale: 1.0,
                },
                true,
            ),
        );
        let mut notify = false;
        for sequence in 1..=100 {
            let mut frame = frame(1);
            frame.sequence = sequence;
            assert!(state.accept(frame, 1));
            notify |= notification.publish();
        }
        notification.reset();
        assert_eq!(state.drain(true).unwrap().frames[0].sequence, 100);
        // GUI can drain between publication and callback. Its late event is
        // harmless, and the next producer still owns a fresh notification.
        drop(state);
        notification.wake_if(notify);
        let mut state = inbound.lock().unwrap();
        let mut next = frame(1);
        next.sequence = 101;
        assert!(state.accept(next, 1));
        let notify = notification.publish();
        drop(state);
        notification.wake_if(notify);
        assert_eq!(calls.load(Ordering::Relaxed), 2);
        let mut state = inbound.lock().unwrap();
        notification.reset();
        assert_eq!(state.drain(true).unwrap().frames[0].sequence, 101);
    }

    #[test]
    fn request_epoch_rejects_same_revision_destroy_recreate_and_theme_resize() {
        let mut state = Inbound::default();
        let viewport = Viewport {
            width: 1,
            height: 1,
            scale: 1.0,
        };
        state.desired.insert("a".into(), (10, 1, viewport, true));
        state.accept(frame(1), 10);
        state.desired.remove("a");
        state.frames.clear();
        state.sequences.clear();
        state.desired.insert("a".into(), (11, 1, viewport, true));
        state.accept(frame(1), 10); // same key/revision/dimensions, old incarnation
        assert!(state.frames.is_empty());
        state.accept(frame(1), 11);
        assert_eq!(state.frames.len(), 1);
        state.frames.clear();
        state.sequences.clear();
        state.desired.insert("a".into(), (12, 1, viewport, true));
        state.accept(frame(1), 11); // theme-only change or resize away and back
        assert!(state.frames.is_empty());
    }
    #[test]
    fn all_hidden_never_owns_animation_tick() -> Result<(), Error> {
        let mut state = Inbound::default();
        state.animating = true;
        state.desired.insert(
            "a".into(),
            (
                1,
                1,
                Viewport {
                    width: 1,
                    height: 1,
                    scale: 1.0,
                },
                false,
            ),
        );
        assert!(!state.drain(true)?.animating);
        Ok(())
    }
    #[test]
    fn missing_helper_is_actionable_error() {
        let path = std::env::temp_dir()
            .join(format!("neoism-no-worker-{}-missing", std::process::id()))
            .join("neoism-cef-runtime");
        let error = spawn_worker(&path).err().map(|e| e.to_string());
        assert!(error.is_some_and(|e| e.contains("NEOISM_CEF_RUNTIME")));
    }
    #[test]
    fn fatal_state_returns_persistent_backend_error() {
        let mut state = Inbound::default();
        state.error = Some("worker crash".into());
        assert!(matches!(state.drain(false), Err(Error::Backend(_))));
        assert!(matches!(state.drain(true), Err(Error::Backend(_))));
        state.error = None;
        assert!(matches!(state.drain(false), Err(Error::Backend(_))));
    }
    #[test]
    fn fatal_and_malformed_frame_are_protocol_errors() -> std::io::Result<()> {
        let state = Mutex::new(Inbound::default());
        let wake = Notification::new(Arc::new(|| {}));
        let mut bytes = Vec::new();
        ipc::write_json(
            &mut bytes,
            &Output::Hello {
                version: ipc::PROTOCOL_VERSION,
            },
        )?;
        ipc::write_json(&mut bytes, &Output::Fatal("bad engine".into()))?;
        assert!(read_worker(&mut std::io::Cursor::new(bytes), &state, &wake).is_err());
        let mut bytes = Vec::new();
        ipc::write_json(
            &mut bytes,
            &Output::Hello {
                version: ipc::PROTOCOL_VERSION,
            },
        )?;
        ipc::write_json(
            &mut bytes,
            &Output::Frame(ipc::FrameMetadata {
                key: "a".into(),
                generation: 1,
                revision: 1,
                sequence: 1,
                width: 1,
                height: 1,
                stride: 4,
                bytes: 4,
            }),
        )?;
        // A metadata packet without its advertised binary body is fatal, not a blank frame.
        assert!(read_worker(&mut std::io::Cursor::new(bytes), &state, &wake).is_err());
        Ok(())
    }
    #[test]
    fn stale_destroyed_and_hidden_frames_are_rejected() {
        let mut inbound = Inbound::default();
        inbound.accept(frame(1), 1);
        assert!(inbound.frames.is_empty());
        let viewport = Viewport {
            width: 1,
            height: 1,
            scale: 1.0,
        };
        inbound.desired.insert("a".into(), (1, 2, viewport, true));
        inbound.accept(frame(1), 1);
        assert!(inbound.frames.is_empty());
        inbound.accept(frame(2), 1);
        assert_eq!(inbound.frames.len(), 1);
        inbound.frames.clear();
        inbound.desired.insert("a".into(), (1, 2, viewport, false));
        inbound.accept(frame(2), 1);
        assert!(inbound.frames.is_empty());
    }
}
