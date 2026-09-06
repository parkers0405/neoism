#[cfg(test)]
#[path = "pipes_tests.rs"]
mod tests;

use crate::windows::spsc::*;
use corcovado::{
    event::Evented, Poll, PollOpt, Ready, Registration, SetReadiness, Token,
};
use miow::pipe::{AnonRead, AnonWrite};
use parking_lot::{Condvar, Mutex};
use windows_sys::Win32::System::IO::CancelSynchronousIo;

use std::io;
use std::os::windows::io::AsRawHandle;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::{channel, Receiver, TryRecvError},
    Arc,
};
use std::thread::{spawn, JoinHandle};
use std::time::{Duration, Instant};

#[derive(Default)]
struct WaitTag {
    exited: bool,
}

// Cancellation is not sticky: a worker can pass its done check just before
// CancelSynchronousIo runs, then enter ReadFile/WriteFile afterwards. Retry until
// it exits. Never hold the queue mutex here (or across synchronous pipe I/O).
// A broken driver must not hang the UI indefinitely: after one second hand off
// to a reaper that keeps cancelling, including I/O entered after that deadline.
// The worker owns its pipe, Arc state and Arc-backed buffer until it exits.
fn stop_worker(thread: &mut Option<JoinHandle<()>>) {
    let Some(thread) = thread.take() else { return };
    let deadline = Instant::now() + Duration::from_secs(1);
    while !thread.is_finished() {
        unsafe {
            CancelSynchronousIo(thread.as_raw_handle());
        }
        if Instant::now() >= deadline {
            tracing::warn!(
                "Pipe cancellation exceeded one second; reaping in background"
            );
            let result = std::thread::Builder::new()
                .name("pipe-cancellation".into())
                .spawn(move || {
                    while !thread.is_finished() {
                        unsafe {
                            CancelSynchronousIo(thread.as_raw_handle());
                        }
                        std::thread::sleep(Duration::from_millis(10));
                    }
                    let _ = thread.join();
                });
            if let Err(err) = result {
                // Resource exhaustion: detaching is still memory-safe and must
                // not turn a best-effort shutdown into a panic in Drop.
                tracing::warn!(%err, "Could not spawn pipe cancellation reaper");
            }
            return;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    // Drop must also be safe after a worker panic.
    let _ = thread.join();
}

// Wake the poller on EOF/error as well as data. Constructed before the worker's
// queue guards so those guards are released before publishing terminal readiness.
struct WorkerExit<'a> {
    wait_tag: &'a Mutex<WaitTag>,
    readiness: SetReadiness,
    ready: Ready,
    sender: Option<std::sync::mpsc::Sender<String>>,
}
impl Drop for WorkerExit<'_> {
    fn drop(&mut self) {
        let mut guard = self.wait_tag.lock();
        guard.exited = true;
        // Disconnect first, so a woken poller always sees the terminal state.
        self.sender.take();
        let _ = self.readiness.set_readiness(self.ready);
    }
}

struct EventedAnonReadInner {
    registration: Registration,
    readiness: SetReadiness,
    done: AtomicBool,
    sig_buffer_not_full: Condvar,
    wait_tag: Mutex<WaitTag>,
}

/// Wraps an AnonRead pipe so that it can be read asynchronously using mio.
///
/// This is achieved by spawning a worker thread which continuously attempts
/// to read from the pipe into a buffer, which reads from the EventedAnonRead
/// object will be directed to.
///
/// This should only be considered if your application architecture requires
/// a synchronous anonymous pipe; an asynchronous NamedPipe will likely be
/// more performant.
pub struct EventedAnonRead {
    // Is an Option so it can be moved out and joined in the Drop impl.
    thread: Option<JoinHandle<()>>,
    consumer: SpscBufferReader,
    inner: Arc<EventedAnonReadInner>,
    error_receiver: Receiver<String>,
}

// Helper to send an error string from the worker threads
macro_rules! try_or_send {
    ($e:expr, $worker:ident) => {
        match $e {
            Ok(value) => value,
            Err(e) => {
                let _ = $worker.sender.as_ref().unwrap().send(e.to_string());
                return;
            }
        }
    };
}

impl EventedAnonRead {
    pub fn new(mut pipe: AnonRead) -> Self {
        let (registration, readiness) = Registration::new2();

        let (mut producer, consumer) = spsc_buffer(65536);

        let done = AtomicBool::new(false);

        let sig_buffer_not_full = Condvar::new();
        let wait_tag = Mutex::new(WaitTag::default());

        let (error_sender, error_receiver) = channel();

        let inner = Arc::new(EventedAnonReadInner {
            registration,
            readiness,
            done,
            sig_buffer_not_full,
            wait_tag,
        });

        let thread = {
            let inner = inner.clone();
            spawn(move || {
                use std::io::Read;
                let worker = WorkerExit {
                    wait_tag: &inner.wait_tag,
                    readiness: inner.readiness.clone(),
                    ready: Ready::readable(),
                    sender: Some(error_sender),
                };

                let mut tmp_buf = [0u8; 65535];

                loop {
                    if inner.done.load(Ordering::SeqCst) {
                        return;
                    }

                    // Read into temp buffer
                    let nbytes = try_or_send!(pipe.read(&mut tmp_buf[..]), worker);
                    if nbytes == 0 {
                        return; // EOF: do not spin on a zero-length read.
                    }

                    // Write from the temp buffer into the producer
                    let mut written = 0usize;
                    while written < nbytes {
                        // Predicate and notification share the same mutex.
                        let mut wait_tag = inner.wait_tag.lock();
                        while producer.is_full() && !inner.done.load(Ordering::SeqCst) {
                            inner.sig_buffer_not_full.wait(&mut wait_tag);
                        }
                        if inner.done.load(Ordering::SeqCst) {
                            return;
                        }

                        written += producer.write_from_slice(&tmp_buf[written..nbytes]);

                        if !inner.readiness.readiness().is_readable() {
                            try_or_send!(
                                inner.readiness.set_readiness(Ready::readable()),
                                worker
                            );
                        }
                    }
                }
            })
        };

        Self {
            thread: Some(thread),
            consumer,
            inner,
            error_receiver,
        }
    }
}

impl io::Read for EventedAnonRead {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let _guard = self.inner.wait_tag.lock();
        // Drain queued bytes before reporting worker EOF/error.
        let nbytes = self.consumer.read_to_slice(buf);
        self.inner.sig_buffer_not_full.notify_one();
        if nbytes > 0 {
            // Rearm idle-separated edge notifications, but never clear terminal
            // readiness after the worker has published EOF/error under this lock.
            if self.consumer.is_empty() && !_guard.exited {
                self.inner.readiness.set_readiness(Ready::empty())?;
            }
            return Ok(nbytes);
        }
        self.inner.readiness.set_readiness(Ready::empty())?;
        match self.error_receiver.try_recv() {
            Ok(err) => {
                self.inner.readiness.set_readiness(Ready::readable())?;
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, err));
            }
            Err(TryRecvError::Disconnected) => {
                self.inner.readiness.set_readiness(Ready::readable())?;
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "pipe reader closed",
                ));
            }
            Err(TryRecvError::Empty) => {}
        }
        Ok(nbytes)
    }
}

impl Evented for EventedAnonRead {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        poll.register(&self.inner.registration, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        poll.reregister(&self.inner.registration, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        poll.deregister(&self.inner.registration)
    }
}

impl Drop for EventedAnonRead {
    fn drop(&mut self) {
        {
            let _guard = self.inner.wait_tag.lock();
            self.inner.done.store(true, Ordering::SeqCst);
            self.inner.sig_buffer_not_full.notify_one();
        }
        stop_worker(&mut self.thread);
    }
}

struct EventedAnonWriteInner {
    registration: Registration,
    readiness: SetReadiness,
    done: AtomicBool,
    sig_buffer_not_empty: Condvar,
    wait_tag: Mutex<WaitTag>,
}

/// Wraps an AnonWrite pipe so that it can be written asynchronously using mio.
///
/// This is achieved by spawning a worker thread which continuously attempts
/// to write to the pipe from a buffer, which writes to the EventedAnonWrite
/// object will be directed to.
///
/// This should only be considered if your application architecture requires
/// a synchronous anonymous pipe; an asynchronous NamedPipe will likely be
/// more performant.
pub struct EventedAnonWrite {
    // Is an Option so it can be moved out and joined in the Drop impl
    thread: Option<JoinHandle<()>>,
    producer: SpscBufferWriter,
    inner: Arc<EventedAnonWriteInner>,
    error_receiver: Receiver<String>,
}

impl EventedAnonWrite {
    pub fn new(mut pipe: AnonWrite) -> Self {
        let (registration, readiness) = Registration::new2();

        let (producer, mut consumer) = spsc_buffer(65536);

        let done = AtomicBool::new(false);

        let sig_buffer_not_empty = Condvar::new();
        let wait_tag = Mutex::new(WaitTag::default());

        let inner = Arc::new(EventedAnonWriteInner {
            registration,
            readiness,
            done,
            sig_buffer_not_empty,
            wait_tag,
        });

        let (error_sender, error_receiver) = channel();

        let thread = {
            let inner = inner.clone();
            spawn(move || {
                use std::io::Write;
                let worker = WorkerExit {
                    wait_tag: &inner.wait_tag,
                    readiness: inner.readiness.clone(),
                    ready: Ready::writable(),
                    sender: Some(error_sender),
                };
                let mut tmp_buf = [0u8; 65535];

                try_or_send!(inner.readiness.set_readiness(Ready::writable()), worker);

                loop {
                    if inner.done.load(Ordering::SeqCst) {
                        return;
                    }

                    // Read into temp buffer while holding the lock
                    let nbytes = {
                        // Queue access is locked; the pipe write below is not.
                        let mut wait_tag = inner.wait_tag.lock();
                        while consumer.is_empty() && !inner.done.load(Ordering::SeqCst) {
                            inner.sig_buffer_not_empty.wait(&mut wait_tag);
                        }
                        if inner.done.load(Ordering::SeqCst) {
                            return;
                        }

                        let nbytes = consumer.read_to_slice(&mut tmp_buf);

                        if !inner.readiness.readiness().is_writable() {
                            try_or_send!(
                                inner.readiness.set_readiness(Ready::writable()),
                                worker
                            );
                        }

                        nbytes
                    };

                    let mut written = 0usize;
                    while written < nbytes {
                        if inner.done.load(Ordering::SeqCst) {
                            return;
                        }
                        let count =
                            try_or_send!(pipe.write(&tmp_buf[written..nbytes]), worker);
                        if count == 0 {
                            try_or_send!(
                                Err::<(), _>(io::Error::from(io::ErrorKind::WriteZero)),
                                worker
                            );
                        }
                        written += count;
                    }
                }
            })
        };

        Self {
            thread: Some(thread),
            producer,
            inner,
            error_receiver,
        }
    }
}

impl io::Write for EventedAnonWrite {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        if self.thread.is_none() {
            return Err(io::Error::new(io::ErrorKind::BrokenPipe, ""));
        }

        let _guard = self.inner.wait_tag.lock();
        match self.error_receiver.try_recv() {
            Ok(err) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, err));
            }
            Err(TryRecvError::Disconnected) => {
                return Err(io::Error::new(io::ErrorKind::BrokenPipe, ""))
            }
            Err(TryRecvError::Empty) => {}
        }

        let nbytes = self.producer.write_from_slice(buf);
        self.inner.sig_buffer_not_empty.notify_one();
        if self.producer.is_full() {
            self.inner.readiness.set_readiness(Ready::empty())?;
        }

        Ok(nbytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Evented for EventedAnonWrite {
    fn register(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        poll.register(&self.inner.registration, token, interest, opts)
    }

    fn reregister(
        &self,
        poll: &Poll,
        token: Token,
        interest: Ready,
        opts: PollOpt,
    ) -> io::Result<()> {
        poll.reregister(&self.inner.registration, token, interest, opts)
    }

    fn deregister(&self, poll: &Poll) -> io::Result<()> {
        poll.deregister(&self.inner.registration)
    }
}

impl Drop for EventedAnonWrite {
    fn drop(&mut self) {
        {
            let _guard = self.inner.wait_tag.lock();
            self.inner.done.store(true, Ordering::SeqCst);
            self.inner.sig_buffer_not_empty.notify_one();
        }
        stop_worker(&mut self.thread);
    }
}
