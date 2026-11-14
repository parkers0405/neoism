//! A thread that finishes, submits and presents a surface's frames, so the event loop's thread
//! records a frame and moves on: wgpu-core encodes the recorded passes at `finish`, and
//! `finish`, `submit` and `present` are most of a simple frame's CPU time.
//!
//! One job is in flight at most. The owner calls `wait` before it next touches the surface
//! (acquire, configure, drop), because wgpu lends a surface one texture at a time.

use std::sync::mpsc;
use std::thread::JoinHandle;

type Job = Box<dyn FnOnce() + Send>;

pub(crate) struct PresentWorker {
    jobs: Option<mpsc::Sender<Job>>,
    done: mpsc::Receiver<std::thread::Result<()>>,
    in_flight: bool,
    thread: Option<JoinHandle<()>>,
}

impl PresentWorker {
    /// `None` when the thread can't be made; the caller presents on its own thread.
    pub(crate) fn spawn() -> Option<PresentWorker> {
        let (jobs, inbox) = mpsc::channel::<Job>();
        let (finished, done) = mpsc::channel();
        let thread = std::thread::Builder::new().name("rune present".into()).spawn(move || {
            #[cfg(target_os = "macos")]
            interactive();
            while let Ok(job) = inbox.recv() {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
                if finished.send(result).is_err() { break; }
            }
        }).ok()?;
        Some(PresentWorker { jobs: Some(jobs), done, in_flight: false, thread: Some(thread) })
    }

    /// Runs `job` on the thread once the job before it has finished.
    pub(crate) fn run(&mut self, job: impl FnOnce() + Send + 'static) {
        self.wait();
        let Some(jobs) = &self.jobs else { return job() };
        match jobs.send(Box::new(job)) {
            Ok(()) => self.in_flight = true,
            Err(mpsc::SendError(job)) => job(),
        }
    }

    /// Blocks until the job in flight, if any, has finished. A job that panicked panics here,
    /// on the owner's thread, as it would have without the worker.
    pub(crate) fn wait(&mut self) {
        if !std::mem::take(&mut self.in_flight) { return; }
        if let Ok(Err(panic)) = self.done.recv() {
            self.jobs = None;
            std::panic::resume_unwind(panic);
        }
    }
}

impl Drop for PresentWorker {
    fn drop(&mut self) {
        if self.in_flight && !std::thread::panicking() { self.wait(); }
        self.jobs = None;
        if let Some(thread) = self.thread.take() { let _ = thread.join(); }
    }
}

/// Presents keep pace with the display only at the main thread's priority.
#[cfg(target_os = "macos")]
fn interactive() {
    const QOS_CLASS_USER_INTERACTIVE: u32 = 0x21;
    unsafe extern "C" {
        fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
    }
    // SAFETY: sets the calling thread's own QoS class.
    unsafe { pthread_set_qos_class_self_np(QOS_CLASS_USER_INTERACTIVE, 0) };
}

#[cfg(test)]
#[path = "present_worker_tests.rs"]
mod tests;
