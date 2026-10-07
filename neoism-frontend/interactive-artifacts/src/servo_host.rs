//! Nonblocking native facade over the separately packaged real Servo worker.
//! This module has no Servo/native graphics dependency. Pipe I/O never runs on the GUI thread.
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
    ArtifactDocument, ArtifactFrame, ArtifactInput, Error, PumpOutput, SandboxStatus,
    Viewport, MAX_VIEWS, SANDBOX_STATUS,
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
            return Err(Error::Backend("Servo helper is not running".into()));
        }
        Ok(PumpOutput {
            frames: std::mem::take(&mut self.frames).into_values().collect(),
            diagnostics: std::mem::take(&mut self.diagnostics),
            animating: self.animating
                && self.desired.values().any(|(_, _, _, visible)| *visible),
        })
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
/// Caller must gate creation behind explicit experimental opt-in.
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
            .name("servo-artifacts-write".into())
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
                            format!("Servo helper input pipe: {error}"),
                        );
                        terminate(writer_child.clone());
                        break;
                    }
                }
            })
        {
            terminate(child.clone());
            return Err(Error::Backend(format!(
                "Cannot start Servo IPC writer: {error}"
            )));
        }
        let reader_inbound = inbound.clone();
        let reader_alive = alive.clone();
        let reader_child = child.clone();
        let reader_wake = notification.clone();
        if let Err(error) = thread::Builder::new()
            .name("servo-artifacts-read".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let result = read_worker(&mut reader, &reader_inbound, &reader_wake);
                let message = match result {
                    Ok(()) => "Servo helper exited or closed its output pipe".into(),
                    Err(error) => format!("Servo helper protocol failure: {error}"),
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
                "Cannot start Servo IPC reader: {error}"
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
                "Servo helper is not running; inspect pump diagnostics".into(),
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
                    "Servo helper request byte budget full; retry on next tick".into(),
                )
            })?;
        let Some(sender) = &self.sender else {
            self.queued.fetch_sub(size, Ordering::AcqRel);
            return Err(Error::Backend("Servo helper shut down".into()));
        };
        match sender.try_send(bytes) {
            Ok(()) => Ok(()),
            Err(error) => {
                self.queued.fetch_sub(size, Ordering::AcqRel);
                Err(Error::Backend(
                    match error {
                        TrySendError::Full(_) => {
                            "Servo helper request queue full; retry on next tick"
                        }
                        TrySendError::Disconnected(_) => {
                            "Servo helper request pipe disconnected"
                        }
                    }
                    .into(),
                ))
            }
        }
    }
    fn generation(&mut self) -> Result<u64, Error> {
        let generation = self.next_generation;
        self.next_generation = generation
            .checked_add(1)
            .ok_or_else(|| Error::Backend("Servo request generation exhausted".into()))?;
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
                "Pending Servo destroys; retry next tick".into(),
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
            .map_err(|_| Error::Backend("Servo mailbox poisoned".into()))?;
        self.send(&Control::Reconcile {
            document: document.clone(),
            generation,
        })?;
        self.latest.remove(&document.key);
        state.frames.remove(&document.key);
        state.sequences.remove(&document.key);
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
            .map_err(|_| Error::Backend("Servo mailbox poisoned".into()))?;
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
            .map_err(|_| Error::Backend("Servo mailbox poisoned".into()))?;
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
        // Never join blocked pipe threads or wait for Servo on the GUI thread. Shutdown
        // gets 500ms for graceful drain, then the background reaper kills/waits the worker.
        let child = self.child.clone();
        let _ = thread::Builder::new()
            .name("servo-artifacts-reap".into())
            .spawn(move || {
                thread::sleep(std::time::Duration::from_millis(500));
                terminate(child);
            });
    }
}
fn spawn_worker(path: &std::path::Path) -> Result<Child, Error> {
    Command::new(path).args(["--stdio", "--experimental"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
        .spawn().map_err(|e| Error::Backend(format!("Cannot launch Servo helper {}: {e}. Set NEOISM_SERVO_RUNTIME to the separately installed neoism-servo-runtime executable", path.display())))
}
fn runtime_path() -> Result<PathBuf, Error> {
    if let Some(path) = std::env::var_os("NEOISM_SERVO_RUNTIME") {
        if path.is_empty() {
            return Err(Error::Backend("NEOISM_SERVO_RUNTIME is empty".into()));
        }
        return Ok(PathBuf::from(path));
    }
    let executable =
        std::env::current_exe().map_err(|e| Error::Backend(e.to_string()))?;
    let parent = executable.parent().ok_or_else(|| {
        Error::Backend("Cannot locate Neoism executable directory".into())
    })?;
    let name = if cfg!(windows) {
        "neoism-servo-runtime.exe"
    } else {
        "neoism-servo-runtime"
    };
    let bundled = parent.join(name);
    if bundled.is_file() {
        return Ok(bundled);
    }
    // Development builds can use the separately built worker directly, even
    // after cleaning the desktop target directory. Releases require bundling.
    #[cfg(debug_assertions)]
    {
        let development = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../servo-runtime/target/debug")
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
            .join("neoism-servo-runtime");
        let error = spawn_worker(&path).err().map(|e| e.to_string());
        assert!(error.is_some_and(|e| e.contains("NEOISM_SERVO_RUNTIME")));
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
