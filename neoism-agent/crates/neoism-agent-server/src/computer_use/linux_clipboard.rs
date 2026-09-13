//! Explicit, replace-only clipboard publication. Never reads the old selection,
//! restores it, invokes an IME, or silently falls back from keyboard input.
//! A successful ACK means publication was dispatched, not that an app pasted it.
use anyhow::{Context, ensure};
use std::{
    ffi::OsString,
    fs::File,
    io::{self, Write},
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::{
        Arc, Mutex, OnceLock,
        atomic::{AtomicBool, AtomicU8, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};
use wayland_client::{
    Connection, Dispatch, EventQueue, QueueHandle,
    protocol::{wl_callback, wl_registry, wl_seat},
};
use wayland_protocols_wlr::data_control::v1::client::{
    zwlr_data_control_device_v1 as device, zwlr_data_control_manager_v1 as manager,
    zwlr_data_control_offer_v1 as offer, zwlr_data_control_source_v1 as source,
};

const LIMIT: Duration = Duration::from_millis(500);
const MIME_TYPES: [&str; 2] = ["text/plain;charset=utf-8", "text/plain"];
const PENDING: u8 = 0;
const CANCELLED: u8 = 1;
const PUBLISHING: u8 = 2;

fn payload(text: &str) -> anyhow::Result<Arc<[u8]>> {
    ensure!(
        text.chars().count() <= 512 && text.len() <= 2048,
        "Clipboard text exceeds 512 Unicode scalars / 2048 bytes"
    );
    ensure!(!text.contains('\0'), "Clipboard text cannot contain NUL");
    Ok(Arc::from(text.as_bytes()))
}

// One counter coalesces request, authorization and cancellation notifications.
// Channel/phase updates always precede notify; only the worker drains this FD.
struct Waker(OwnedFd);
impl Waker {
    fn new() -> io::Result<Self> {
        // SAFETY: eventfd creates a new owned descriptor with no borrowed pointers.
        let fd = unsafe { libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK) };
        if fd < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(unsafe { OwnedFd::from_raw_fd(fd) }))
    }
    fn notify(&self) -> io::Result<()> {
        let value = 1u64;
        loop {
            // SAFETY: live descriptor and an initialized eight-byte counter.
            let n = unsafe {
                libc::write(self.0.as_raw_fd(), (&value as *const u64).cast(), 8)
            };
            if n == 8 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()), // Already readable.
                _ => return Err(error),
            }
        }
    }
    fn drain(&self) -> io::Result<()> {
        let mut value = 0u64;
        loop {
            // SAFETY: live descriptor and writable eight-byte counter storage.
            let n = unsafe {
                libc::read(self.0.as_raw_fd(), (&mut value as *mut u64).cast(), 8)
            };
            if n == 8 {
                return Ok(());
            }
            let error = io::Error::last_os_error();
            match error.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => return Ok(()),
                _ => return Err(error),
            }
        }
    }
}
fn poll_until(fds: &mut [libc::pollfd], deadline: Option<Instant>) -> io::Result<usize> {
    loop {
        // Round UP to avoid a sub-millisecond busy-spin at the deadline.
        let timeout = deadline.map_or(-1, |end| {
            end.saturating_duration_since(Instant::now())
                .as_nanos()
                .div_ceil(1_000_000)
                .min(i32::MAX as u128) as i32
        });
        // SAFETY: initialized pollfd slice, owned/borrowed live FDs at call sites.
        let result =
            unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout) };
        if result >= 0 {
            return Ok(result as usize);
        }
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}

struct Request {
    text: Arc<[u8]>,
    phase: Arc<AtomicU8>,
    permit: mpsc::Receiver<()>,
    response: mpsc::Sender<anyhow::Result<Reply>>,
    deadline: Instant,
}
enum Reply {
    Ready,
    Published,
}
struct Service {
    endpoint: (Option<OsString>, Option<OsString>, Option<OsString>),
    sender: mpsc::SyncSender<Request>,
    waker: Arc<Waker>,
    alive: Arc<AtomicBool>,
}
static SERVICE: OnceLock<Mutex<Option<Service>>> = OnceLock::new();

/// Explicit consent to REPLACE the clipboard is a caller responsibility. `check`
/// must revalidate that consent and foreground target, including while waiting.
/// No automatic retry is performed, even when publication cannot be acknowledged.
/// The isolated worker retains the source after this call returns.
pub(super) fn publish(
    text: &str,
    mut check: impl FnMut() -> anyhow::Result<()>,
) -> anyhow::Result<()> {
    let text = payload(text)?;
    check()?;
    let endpoint = (
        std::env::var_os("XDG_RUNTIME_DIR"),
        std::env::var_os("WAYLAND_DISPLAY"),
        std::env::var_os("WAYLAND_SOCKET"),
    );
    let (sender, waker) = {
        let mut slot = SERVICE
            .get_or_init(|| Mutex::new(None))
            .lock()
            .map_err(|_| anyhow::anyhow!("Clipboard service lock poisoned"))?;
        if slot
            .as_ref()
            .is_some_and(|service| !service.alive.load(Ordering::Acquire))
        {
            *slot = None;
        }
        if slot.is_none() {
            // One queued request at most. An uncertain publication is NEVER
            // retried against a replacement worker.
            let (sender, receiver) = mpsc::sync_channel(1);
            let waker =
                Arc::new(Waker::new().context("Cannot create clipboard worker waker")?);
            let worker_waker = waker.clone();
            let alive = Arc::new(AtomicBool::new(true));
            let running = alive.clone();
            std::thread::Builder::new()
                .name("neoism-clipboard".into())
                .spawn(move || {
                    struct Running(Arc<AtomicBool>);
                    impl Drop for Running {
                        fn drop(&mut self) {
                            self.0.store(false, Ordering::Release);
                        }
                    }
                    let _running = Running(running);
                    worker(receiver, &worker_waker);
                })
                .context("Cannot start clipboard service")?;
            *slot = Some(Service {
                endpoint: endpoint.clone(),
                sender,
                waker,
                alive,
            });
        }
        let service = slot.as_ref().unwrap();
        ensure!(
            service.endpoint == endpoint,
            "Clipboard service belongs to a different Wayland connection; no publication sent"
        );
        (service.sender.clone(), service.waker.clone())
    }; // Never hold the service lock while calling a guard or waiting.
    check()?;
    let phase = Arc::new(AtomicU8::new(PENDING));
    // Also revoke an unconsumed authorization if the caller's guard panics.
    struct RevokeOnDrop(Arc<AtomicU8>, Arc<Waker>);
    impl Drop for RevokeOnDrop {
        fn drop(&mut self) {
            cancel(&self.0);
            let _ = self.1.notify();
        }
    }
    let _revoke = RevokeOnDrop(phase.clone(), waker.clone());
    let (permit, permitted) = mpsc::channel();
    let (response, replies) = mpsc::channel();
    let deadline = Instant::now() + LIMIT;
    sender
        .try_send(Request {
            text,
            phase: phase.clone(),
            permit: permitted,
            response,
            deadline,
        })
        .map_err(|error| match error {
            mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("Clipboard service busy; no publication queued")
            }
            mpsc::TrySendError::Disconnected(_) => anyhow::anyhow!(
                "Clipboard service disconnected; no publication queued (not retried)"
            ),
        })?;
    let result = (|| {
        waker.notify().context("Cannot wake clipboard worker")?;
        let mut authorized = false;
        loop {
            check()?;
            ensure!(
                Instant::now() < deadline,
                "Clipboard publication acknowledgement timed out"
            );
            match replies.recv_timeout(Duration::from_millis(5)) {
                Ok(Ok(Reply::Ready)) => {
                    ensure!(!authorized, "Duplicate clipboard readiness response");
                    // Protocol and seat were discovered before this final guard.
                    check()?;
                    permit
                        .send(())
                        .context("Clipboard worker disconnected before authorization")?;
                    waker
                        .notify()
                        .context("Cannot wake clipboard authorization")?;
                    authorized = true;
                }
                Ok(Ok(Reply::Published)) => {
                    check()?;
                    return Ok(());
                }
                Ok(Err(error)) => return Err(error),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    anyhow::bail!("Clipboard service disconnected before acknowledgement")
                }
            }
        }
    })();
    if let Err(error) = result {
        // Atomic arbitration closes the race between cancellation and set_selection.
        // Once PUBLISHING wins, callers must assume clipboard disclosure occurred.
        let changed = cancel(&phase);
        return Err(error.context(if changed {
            "Clipboard may already have changed; do not retry automatically or claim paste acceptance"
        } else {
            "Clipboard publication cancelled before selection change"
        }));
    }
    Ok(())
}
fn cancel(phase: &AtomicU8) -> bool {
    phase
        .compare_exchange(PENDING, CANCELLED, Ordering::AcqRel, Ordering::Acquire)
        .is_err_and(|value| value == PUBLISHING)
}
fn authorize(phase: &AtomicU8) -> bool {
    phase
        .compare_exchange(PENDING, PUBLISHING, Ordering::AcqRel, Ordering::Acquire)
        .is_ok()
}

#[derive(Default)]
struct State {
    seats: Vec<(u32, wl_seat::WlSeat)>,
    manager: Option<(u32, manager::ZwlrDataControlManagerV1)>,
    current: Option<(source::ZwlrDataControlSourceV1, Arc<[u8]>)>,
    stopped: bool,
}
impl Dispatch<wl_registry::WlRegistry, ()> for State {
    fn event(
        s: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        q: &QueueHandle<Self>,
    ) {
        match event {
            wl_registry::Event::Global {
                name, interface, ..
            } => match interface.as_str() {
                "wl_seat" => s.seats.push((name, registry.bind(name, 1, q, ()))),
                "zwlr_data_control_manager_v1" => {
                    if s.manager.is_some() {
                        s.stopped = true; // Duplicate managers are ambiguous, never last-wins.
                    } else {
                        s.manager = Some((name, registry.bind(name, 1, q, ())));
                    }
                }
                _ => {}
            },
            wl_registry::Event::GlobalRemove { name } => {
                if s.seats.iter().any(|(id, _)| *id == name)
                    || s.manager.as_ref().is_some_and(|(id, _)| *id == name)
                {
                    s.stopped = true;
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<device::ZwlrDataControlDeviceV1, ()> for State {
    fn event(
        s: &mut Self,
        _: &device::ZwlrDataControlDeviceV1,
        event: device::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            // We neither request nor inspect existing clipboard contents.
            device::Event::DataOffer { id } => id.destroy(),
            device::Event::Finished => s.stopped = true,
            _ => {}
        }
    }
    wayland_client::event_created_child!(State, device::ZwlrDataControlDeviceV1, [
        0 => (offer::ZwlrDataControlOfferV1, ())
    ]);
}
impl Dispatch<source::ZwlrDataControlSourceV1, ()> for State {
    fn event(
        s: &mut Self,
        object: &source::ZwlrDataControlSourceV1,
        event: source::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            source::Event::Send { mime_type, fd } => {
                if let Some((current, bytes)) = &s.current {
                    if current == object {
                        // The receiver gets no bytes for MIME types we did not offer.
                        // A broken/stalled individual paste is not a service failure.
                        let _ = serve(fd, &mime_type, bytes);
                    }
                }
            }
            source::Event::Cancelled => {
                if let Some(object) = release_source(&mut s.current, object) {
                    object.destroy();
                }
            }
            _ => {}
        }
    }
}
impl Dispatch<wl_callback::WlCallback, Arc<AtomicBool>> for State {
    fn event(
        _: &mut Self,
        _: &wl_callback::WlCallback,
        _: wl_callback::Event,
        done: &Arc<AtomicBool>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        done.store(true, Ordering::Release);
    }
}
wayland_client::delegate_noop!(State: ignore wl_seat::WlSeat);
wayland_client::delegate_noop!(State: ignore manager::ZwlrDataControlManagerV1);
wayland_client::delegate_noop!(State: ignore offer::ZwlrDataControlOfferV1);

// A delayed cancellation of an old source must not discard its replacement.
fn release_source<T: PartialEq>(
    current: &mut Option<(T, Arc<[u8]>)>,
    cancelled: &T,
) -> Option<T> {
    if current
        .as_ref()
        .is_some_and(|(object, _)| object == cancelled)
    {
        current.take().map(|(object, _)| object) // Drop payload; never clear selection.
    } else {
        None
    }
}

fn serve(fd: OwnedFd, mime: &str, bytes: &[u8]) -> io::Result<()> {
    if !MIME_TYPES.contains(&mime) {
        return Ok(());
    }
    if bytes.len() > 2048 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Clipboard payload exceeds byte cap",
        ));
    }
    let mut file = File::from(fd);
    let raw = file.as_raw_fd();
    // O_NONBLOCK does not bound regular-file/device I/O. Only stream IPC FDs
    // can be served under this deadline; never write an arbitrary received file.
    let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
    // SAFETY: valid owned FD and correctly sized output storage.
    if unsafe { libc::fstat(raw, stat.as_mut_ptr()) } < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful fstat initialized the structure.
    let kind = unsafe { stat.assume_init() }.st_mode & libc::S_IFMT;
    if kind != libc::S_IFIFO && kind != libc::S_IFSOCK {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "Clipboard receiver must supply a pipe or socket",
        ));
    }
    // SAFETY: fcntl operates on the live, owned descriptor only.
    let flags = unsafe { libc::fcntl(raw, libc::F_GETFL) };
    if flags < 0
        || unsafe { libc::fcntl(raw, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0
    {
        return Err(io::Error::last_os_error());
    }
    let deadline = Instant::now() + Duration::from_millis(50);
    let mut remaining = bytes;
    while !remaining.is_empty() {
        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Clipboard receiver stalled",
            ));
        }
        match file.write(remaining) {
            Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
            Ok(n) => remaining = &remaining[n..],
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                let mut poll = libc::pollfd {
                    fd: raw,
                    events: libc::POLLOUT,
                    revents: 0,
                };
                poll_until(std::slice::from_mut(&mut poll), Some(deadline))?;
                if poll.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0 {
                    return Err(io::ErrorKind::BrokenPipe.into());
                }
            }
            Err(error) => return Err(error),
        }
    }
    Ok(()) // Owned FD closes on every path, including unsupported MIME and errors.
}

fn pump(
    conn: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    waker: &Waker,
    deadline: Option<Instant>,
) -> anyhow::Result<()> {
    let dispatched = queue.dispatch_pending(state)?;
    ensure!(
        !state.stopped && state.seats.len() <= 1,
        "Clipboard seat/device/manager became unavailable or ambiguous"
    );
    let blocked = match conn.flush() {
        Ok(()) => false,
        Err(wayland_client::backend::WaylandError::Io(e))
            if e.kind() == io::ErrorKind::WouldBlock =>
        {
            true
        }
        Err(error) => return Err(error.into()),
    };
    // Pending callbacks may have satisfied sync(). Do not sleep before its
    // caller can recheck completion, cancellation and queued channel messages.
    if dispatched != 0 {
        return Ok(());
    }
    if let Some(read) = queue.prepare_read() {
        let mut fds = [
            libc::pollfd {
                fd: read.connection_fd().as_raw_fd(),
                events: libc::POLLIN | if blocked { libc::POLLOUT } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: waker.0.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        poll_until(&mut fds, deadline)?; // None means genuinely idle: no timer.
        ensure!(
            fds.iter().all(|fd| fd.revents
                & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL)
                == 0),
            "Clipboard Wayland connection or waker closed"
        );
        if fds[1].revents & libc::POLLIN != 0 {
            waker.drain()?;
        }
        if fds[0].revents & libc::POLLIN != 0 {
            read.read()?;
        }
    }
    queue.dispatch_pending(state)?;
    ensure!(
        !state.stopped && state.seats.len() <= 1,
        "Clipboard seat/device/manager became unavailable or ambiguous"
    );
    Ok(())
}
fn sync(
    conn: &Connection,
    queue: &mut EventQueue<State>,
    state: &mut State,
    waker: &Waker,
    deadline: Instant,
    phase: &AtomicU8,
) -> anyhow::Result<()> {
    let done = Arc::new(AtomicBool::new(false));
    conn.display().sync(&queue.handle(), done.clone());
    while !done.load(Ordering::Acquire) {
        ensure!(
            phase.load(Ordering::Acquire) != CANCELLED,
            "Clipboard request cancelled before publication"
        );
        ensure!(
            Instant::now() < deadline,
            "Clipboard Wayland acknowledgement timed out"
        );
        pump(conn, queue, state, waker, Some(deadline))?;
    }
    Ok(())
}

fn worker(receiver: mpsc::Receiver<Request>, waker: &Waker) {
    let Ok(first) = receiver.recv() else {
        return;
    };
    let setup = (|| -> anyhow::Result<_> {
        let conn = Connection::connect_to_env()?;
        let mut queue = conn.new_event_queue::<State>();
        let mut state = State::default();
        conn.display().get_registry(&queue.handle(), ());
        sync(
            &conn,
            &mut queue,
            &mut state,
            waker,
            first.deadline,
            &first.phase,
        )?;
        ensure!(
            state.seats.len() == 1,
            "Clipboard publication requires one unambiguous Wayland seat"
        );
        let manager = &state
            .manager
            .as_ref()
            .context(
                "Explicit clipboard paste unavailable: compositor lacks wlr-data-control",
            )?
            .1;
        let device = manager.get_data_device(&state.seats[0].1, &queue.handle(), ());
        sync(
            &conn,
            &mut queue,
            &mut state,
            waker,
            first.deadline,
            &first.phase,
        )?;
        Ok((conn, queue, state, device))
    })();
    let (conn, mut queue, mut state, device) = match setup {
        Ok(native) => native,
        Err(error) => {
            let _ = first.response.send(Err(error));
            return;
        }
    };
    let mut next = Some(first);
    loop {
        if let Some(request) = next.take() {
            let result = (|| -> anyhow::Result<bool> {
                if request.phase.load(Ordering::Acquire) != PENDING
                    || Instant::now() >= request.deadline
                {
                    return Ok(false);
                }
                if request.response.send(Ok(Reply::Ready)).is_err() {
                    return Ok(false);
                }
                loop {
                    if request.phase.load(Ordering::Acquire) != PENDING
                        || Instant::now() >= request.deadline
                    {
                        return Ok(false);
                    }
                    match request.permit.try_recv() {
                        Ok(()) => break,
                        Err(mpsc::TryRecvError::Disconnected) => return Ok(false),
                        Err(mpsc::TryRecvError::Empty) => pump(
                            &conn,
                            &mut queue,
                            &mut state,
                            waker,
                            Some(request.deadline),
                        )?,
                    }
                }
                ensure!(
                    !state.stopped && state.seats.len() == 1,
                    "Clipboard seat unavailable"
                );
                if Instant::now() >= request.deadline || !authorize(&request.phase) {
                    return Ok(false);
                }
                let object = state
                    .manager
                    .as_ref()
                    .unwrap()
                    .1
                    .create_data_source(&queue.handle(), ());
                for mime in MIME_TYPES {
                    object.offer(mime.to_owned());
                }
                device.set_selection(Some(&object));
                if let Some((old, _)) =
                    state.current.replace((object, request.text.clone()))
                {
                    old.destroy();
                }
                sync(
                    &conn,
                    &mut queue,
                    &mut state,
                    waker,
                    request.deadline,
                    &request.phase,
                )?;
                Ok(true)
            })();
            match result {
                Ok(true) => {
                    let _ = request.response.send(Ok(Reply::Published));
                }
                Ok(false) => {
                    let _ = request.response.send(Err(anyhow::anyhow!(
                        "Clipboard request cancelled or expired before publication"
                    )));
                }
                Err(error) => {
                    let _ = request.response.send(Err(error));
                    break;
                }
            }
        }
        // Check the queue BEFORE sleeping: sync may have consumed its wake while
        // finishing a previous request. Producers enqueue before notifying, so a
        // request racing this check leaves eventfd readable for the following poll.
        match receiver.try_recv() {
            Ok(request) => next = Some(request),
            Err(mpsc::TryRecvError::Empty) => {
                // Source ownership needs no periodic maintenance. Sleep indefinitely
                // until a Wayland event or request/authorization/cancellation wake.
                if pump(&conn, &mut queue, &mut state, waker, None).is_err() {
                    break;
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => break,
        }
    }
    if let Some((object, _)) = state.current.take() {
        object.destroy();
    }
    device.destroy();
    let _ = conn.flush();
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{io::Read, os::unix::net::UnixStream};

    #[test]
    fn waker_coalesces_notifications_and_drain_rearms() {
        let waker = Waker::new().unwrap();
        let mut fds = [libc::pollfd {
            fd: waker.0.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        assert_eq!(poll_until(&mut fds, Some(Instant::now())).unwrap(), 0);
        waker.notify().unwrap();
        waker.notify().unwrap();
        assert_eq!(poll_until(&mut fds, Some(Instant::now())).unwrap(), 1);
        waker.drain().unwrap();
        assert_eq!(poll_until(&mut fds, Some(Instant::now())).unwrap(), 0);
        waker.notify().unwrap();
        assert_eq!(poll_until(&mut fds, Some(Instant::now())).unwrap(), 1);
    }
    #[test]
    fn idle_pump_has_no_timer_and_cancellation_wakes_it() {
        let (client, _server) = UnixStream::pair().unwrap();
        let conn = Connection::from_socket(client).unwrap();
        let waker = Arc::new(Waker::new().unwrap());
        let phase = Arc::new(AtomicU8::new(PENDING));
        let (started, start) = mpsc::channel();
        let (finished, finish) = mpsc::channel();
        let worker_waker = waker.clone();
        let worker_phase = phase.clone();
        let thread = std::thread::spawn(move || {
            let mut queue = conn.new_event_queue::<State>();
            started.send(()).unwrap();
            let result = pump(
                &conn,
                &mut queue,
                &mut State::default(),
                &worker_waker,
                None,
            );
            finished
                .send((result.is_ok(), authorize(&worker_phase)))
                .unwrap();
        });
        start.recv_timeout(LIMIT).unwrap();
        let idle = finish.recv_timeout(Duration::from_millis(40));
        // Always wake/join before assertions so a regression cannot leak a thread.
        assert!(!cancel(&phase));
        waker.notify().unwrap();
        let result = match idle {
            Ok(result) => Some(result),
            Err(_) => finish.recv_timeout(LIMIT).ok(),
        };
        thread.join().unwrap();
        assert!(
            matches!(idle, Err(mpsc::RecvTimeoutError::Timeout)),
            "idle pump returned without an event"
        );
        assert_eq!(
            result,
            Some((true, false)),
            "cancelled authorization must not publish"
        );
    }
    #[test]
    fn active_ack_obeys_deadline_without_periodic_polling() {
        let (client, _server) = UnixStream::pair().unwrap();
        let conn = Connection::from_socket(client).unwrap();
        let mut queue = conn.new_event_queue::<State>();
        let start = Instant::now();
        let error = sync(
            &conn,
            &mut queue,
            &mut State::default(),
            &Waker::new().unwrap(),
            start + Duration::from_millis(30),
            &AtomicU8::new(PENDING),
        )
        .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(start.elapsed() >= Duration::from_millis(30));
        assert!(start.elapsed() < LIMIT);
    }
    #[test]
    fn duplicate_data_control_managers_fail_closed() {
        let (client, _server) = UnixStream::pair().unwrap();
        let conn = Connection::from_socket(client).unwrap();
        let mut queue = conn.new_event_queue::<State>();
        let registry = conn.display().get_registry(&queue.handle(), ());
        let mut state = State::default();
        for name in [1, 2] {
            <State as Dispatch<wl_registry::WlRegistry, ()>>::event(
                &mut state,
                &registry,
                wl_registry::Event::Global {
                    name,
                    interface: "zwlr_data_control_manager_v1".into(),
                    version: 1,
                },
                &(),
                &conn,
                &queue.handle(),
            );
        }
        assert_eq!(
            state.manager.as_ref().unwrap().0,
            1,
            "must not silently select last manager"
        );
        let error = pump(&conn, &mut queue, &mut state, &Waker::new().unwrap(), None)
            .unwrap_err();
        assert!(error.to_string().contains("ambiguous"));
    }
    #[test]
    fn payload_is_exact_unicode_and_accepts_literal_paste_controls() {
        let text = "🦀👩\u{200d}💻e\u{301}\n\t\r\u{85}";
        assert_eq!(&*payload(text).unwrap(), text.as_bytes());
        assert!(payload("").unwrap().is_empty());
        assert!(payload("a\0b").is_err());
        assert_eq!(payload(&"🦀".repeat(512)).unwrap().len(), 2048);
        assert!(payload(&"a".repeat(513)).is_err());
        assert!(payload(&"🦀".repeat(513)).is_err());
    }
    #[test]
    fn cancellation_arbitrates_publication_without_retry() {
        let phase = AtomicU8::new(PENDING);
        assert!(!cancel(&phase));
        assert!(!authorize(&phase));
        let phase = AtomicU8::new(PENDING);
        assert!(authorize(&phase));
        assert!(cancel(&phase)); // Must report possible clipboard disclosure.
        assert!(!authorize(&phase));
    }
    #[test]
    fn guard_rejection_never_starts_service() {
        let error = publish("🦀", || anyhow::bail!("consent revoked")).unwrap_err();
        assert_eq!(error.to_string(), "consent revoked");
    }
    #[test]
    fn offered_mime_only_and_owned_fd_closes() {
        for mime in [MIME_TYPES[0], MIME_TYPES[1], "text/html", "UTF8_STRING"] {
            let (writer, mut reader) = UnixStream::pair().unwrap();
            reader.set_read_timeout(Some(LIMIT)).unwrap();
            serve(writer.into(), mime, "🦀\n\t".as_bytes()).unwrap();
            let mut output = Vec::new();
            reader.read_to_end(&mut output).unwrap();
            assert_eq!(
                output,
                if MIME_TYPES.contains(&mime) {
                    "🦀\n\t".as_bytes()
                } else {
                    b""
                }
            );
        }
    }
    #[test]
    fn oversize_fd_payload_is_rejected_and_closed() {
        let (writer, mut reader) = UnixStream::pair().unwrap();
        reader.set_read_timeout(Some(LIMIT)).unwrap();
        assert_eq!(
            serve(writer.into(), MIME_TYPES[0], &[b'a'; 2049])
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidInput
        );
        assert_eq!(reader.read(&mut [0; 1]).unwrap(), 0);
    }
    #[test]
    fn cancelled_source_releases_payload_but_stale_cancel_preserves_replacement() {
        let bytes = payload("private 🦀").unwrap();
        let weak = Arc::downgrade(&bytes);
        let mut current = Some((2, bytes));
        assert_eq!(release_source(&mut current, &1), None);
        assert!(weak.upgrade().is_some());
        assert_eq!(release_source(&mut current, &2), Some(2));
        assert!(current.is_none());
        assert!(weak.upgrade().is_none());
    }
    #[test]
    fn non_ipc_receiver_is_rejected_without_writing() {
        use std::os::fd::FromRawFd;
        // Anonymous memory file: no filesystem or host clipboard mutation.
        let fd =
            unsafe { libc::memfd_create(c"clipboard-test".as_ptr(), libc::MFD_CLOEXEC) };
        assert!(fd >= 0);
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        assert_eq!(
            serve(fd, MIME_TYPES[0], b"data").unwrap_err().kind(),
            io::ErrorKind::InvalidInput
        );
    }
    #[test]
    fn closed_and_stalled_receivers_are_bounded() {
        let (writer, reader) = UnixStream::pair().unwrap();
        drop(reader);
        assert!(serve(writer.into(), MIME_TYPES[0], b"data").is_err());
        let (mut writer, _reader) = UnixStream::pair().unwrap();
        writer.set_nonblocking(true).unwrap();
        while writer.write(&[0; 4096]).is_ok() {}
        let start = Instant::now();
        assert_eq!(
            serve(writer.into(), MIME_TYPES[0], b"data")
                .unwrap_err()
                .kind(),
            io::ErrorKind::TimedOut
        );
        assert!(start.elapsed() < LIMIT);
    }
}
