//! One OS clipboard worker shared by windows and overlays. No OS reads on the event thread.
//!
//! A secret copy hands back a `SecretCopy` the app can ask about or clear. The worker keeps a
//! keyed fingerprint of the secret, never its text, and watches the clipboard's change count
//! while it holds one. Tests can give runtimes a shared `memory()` clipboard instead; without
//! one, headless runtimes never touch the OS clipboard.
//!
//! Copies and pastes can carry HTML and RTF beside the plain text ([`ClipboardContent`]).
//! HTML read from the clipboard passes through [`sanitize_html`] on the worker; on macOS an
//! RTF-only clipboard (TextEdit, Notes, Pages) is converted to HTML there too.
use std::hash::{BuildHasher, RandomState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::Duration;
use crate::time::Instant;
use zeroize::Zeroizing;
use crate::{Runtime, Waker};

mod content;
mod html;
pub(crate) mod tx;
#[cfg(target_os = "macos")]
mod macos;
#[cfg(not(target_arch = "wasm32"))]
mod native;
#[cfg(target_arch = "wasm32")]
mod web;
#[cfg(target_os = "macos")]
pub(crate) use macos::tiff_to_png;
pub use content::{ClipboardContent, ClipboardImage, MAX_IMAGE_BYTES};
#[cfg(target_os = "macos")]
use content::valid_image_size;
pub use html::sanitize_html;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> { m.lock().unwrap_or_else(|e| e.into_inner()) }

/// A pending clipboard write. Debug output never includes its contents.
#[derive(Clone)]
pub struct ClipboardWrite {
    content: ClipboardContent,
    clear_after: Option<Duration>,
    secret: Option<Arc<Unwritten>>,
}
impl ClipboardWrite {
    pub(crate) fn new(content: ClipboardContent) -> Self {
        Self { content, clear_after: None, secret: None }
    }
    /// Secrets are plain text only: no HTML or RTF copy of a password lingers.
    pub(crate) fn secret(text: String, clear_after: Duration) -> (Self, SecretCopy) {
        let slot = Arc::new(Slot::default());
        let write = Self { content: ClipboardContent::new(text), clear_after: Some(clear_after), secret: Some(Arc::new(Unwritten(slot.clone()))) };
        (write, SecretCopy(slot))
    }
    /// Everything this write puts on the clipboard.
    pub fn content(&self) -> &ClipboardContent { &self.content }
    pub(crate) fn is_secret(&self) -> bool { self.clear_after.is_some() }
    fn slot(&self) -> Option<&Arc<Slot>> { self.secret.as_ref().map(|u| &u.0) }
    fn bind(&self, route: Route) {
        if let Some(slot) = self.slot() { let _ = slot.route.set(route); }
    }
}
impl std::ops::Deref for ClipboardWrite {
    type Target = str;
    fn deref(&self) -> &str { self.content.text() }
}
impl std::fmt::Debug for ClipboardWrite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClipboardWrite").field("clear_after", &self.clear_after).finish_non_exhaustive()
    }
}

/// What became of a secret copy.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SecretState {
    /// Not written yet.
    #[default]
    Pending,
    /// On the clipboard: the last copy Rune wrote, and the clipboard hasn't changed since.
    Holding,
    /// Cleared by Rune: its delay ran out, or `clear_now` or `Tx::clear_secret_clipboard` ran.
    Cleared,
    /// Something else took its place: a later copy, a change by another app, or a newer copy
    /// before this one was written. Its countdown should end quietly.
    Replaced,
    /// The write failed; the clipboard didn't change.
    Failed(String),
}

#[derive(Default)]
struct Slot {
    state: Mutex<SecretState>,
    /// `clear_now` ran: skip the write if it hasn't happened yet.
    cancel: AtomicBool,
    route: OnceLock<Route>,
}
impl Slot {
    fn set(&self, state: SecretState) { *lock(&self.state) = state; }
}

/// Settles a secret write that was dropped before reaching a clipboard (replaced in its
/// handler, cancelled, or left in a headless runtime with no clipboard).
struct Unwritten(Arc<Slot>);
impl Drop for Unwritten {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        if *state == SecretState::Pending {
            *state = if self.0.cancel.load(Ordering::SeqCst) { SecretState::Cleared } else { SecretState::Replaced };
        }
    }
}

/// A secret copy from `Tx::copy_secret`. It holds no text, and is cheap to clone and send.
/// Its state is updated by the clipboard worker; asking never reads the OS clipboard.
#[derive(Clone)]
pub struct SecretCopy(Arc<Slot>);
impl SecretCopy {
    /// A copy that is never written and stays `Pending`, for scenes and tests that show a copy
    /// in flight without a `Tx`.
    pub fn pending() -> Self { Self(Arc::default()) }
    pub fn state(&self) -> SecretState {
        if let Some(Route::Memory(m)) = self.0.route.get() { m.tick(); }
        lock(&self.0.state).clone()
    }
    /// Still the last copy Rune wrote, and the clipboard hasn't changed since. The OS
    /// clipboard is checked a few times a second while a secret is held.
    pub fn holds(&self) -> bool { self.state() == SecretState::Holding }
    /// Clear the clipboard now if this copy still holds it, on the clipboard worker. A copy not
    /// yet written is never written. Idempotent.
    pub fn clear_now(&self) {
        self.0.cancel.store(true, Ordering::SeqCst);
        let holding = *lock(&self.0.state) == SecretState::Holding;
        if holding && let Some(route) = self.0.route.get() { route.send(Request::Clear(Some(self.0.clone()))); }
    }
}
impl std::fmt::Debug for SecretCopy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("SecretCopy").field(&*lock(&self.0.state)).finish()
    }
}

#[derive(Clone)]
enum Route { System(mpsc::Sender<Request>), Memory(MemoryClipboard) }
impl Route {
    fn send(&self, request: Request) {
        match self {
            Route::System(worker) => { let _ = worker.send(request); }
            Route::Memory(m) => m.handle(request),
        }
    }
}

/// Why the clipboard is read.
#[derive(Clone, Debug, PartialEq)]
enum Purpose {
    /// Cmd/Ctrl-V or `Tx::request_paste`, for this focused field or (`None`) the app, and the
    /// forms that target takes.
    Paste(Option<String>, Forms),
    /// `Tx::read_clipboard`'s callback with this id. It always hears back, empty or not.
    Read(u64),
}

/// The forms a paste target takes, so a read fetches no more than it needs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Forms {
    /// A plain field or a plain `on_paste`.
    Text,
    /// A rich [`TextEdit`](crate::TextEdit) with no hook: text, HTML and RTF.
    Rich,
    /// A hook (`on_input_paste`, `on_paste_content`): every form, images too.
    All,
}
struct Reply { purpose: Purpose, content: ClipboardContent }
#[derive(Clone)]
struct Notify { changed: Arc<AtomicBool>, wake: Waker }
impl Notify {
    fn send(&self) { self.changed.store(true, Ordering::SeqCst); (self.wake)(); }
}
enum Request {
    Shutdown,
    Write(ClipboardWrite, Option<Notify>),
    /// Clear this secret, or whichever secret is held, if the clipboard still holds it.
    Clear(Option<Arc<Slot>>),
    Read { purpose: Purpose, reply: mpsc::Sender<Reply>, wake: Waker },
}
impl Purpose {
    fn paste<S: 'static>(rt: &Runtime<S>, target: Option<String>) -> Self {
        let forms = rt.paste_forms(target.as_deref());
        Purpose::Paste(target, forms)
    }
    fn forms(&self) -> Forms {
        match self { Purpose::Paste(_, forms) => *forms, Purpose::Read(_) => Forms::All }
    }
    /// Reads what this purpose needs from `cb` and what to send back: a paste skips an empty
    /// clipboard.
    fn answer<B: Backend>(self, cb: &mut B) -> Option<Reply> {
        let content = match self.forms() {
            Forms::Text => cb.text().map(ClipboardContent::from),
            forms => cb.content(forms == Forms::All),
        };
        self.reply(content)
    }
    fn reply(self, content: Option<ClipboardContent>) -> Option<Reply> {
        let content = match self {
            Purpose::Paste(..) => content?,
            Purpose::Read(_) => content.unwrap_or_default(),
        };
        Some(Reply { purpose: self, content: content.untrusted() })
    }
}

pub(crate) struct Client {
    sender: mpsc::Sender<Reply>,
    replies: mpsc::Receiver<Reply>,
    wake: Waker,
    /// A secret copy's state changed on the worker: run a frame so the app's poll sees it.
    changed: Arc<AtomicBool>,
}
impl Default for Client {
    fn default() -> Self {
        let (sender, replies) = mpsc::channel();
        Self { sender, replies, wake: std::sync::Arc::new(|| {}), changed: Arc::default() }
    }
}
impl Client {
    pub fn connect(&mut self, wake: Waker) { self.wake = wake; }
    pub fn read<S: 'static>(&self, rt: &Runtime<S>) {
        let purpose = Purpose::paste(rt, rt.paste_target());
        if let Some(m) = &rt.clipboard {
            if let Some(reply) = m.answer(purpose) && self.sender.send(reply).is_ok() { (self.wake)(); }
            return;
        }
        self.read_for(purpose);
    }
    pub fn receive<S: 'static>(&self, rt: &mut Runtime<S>) -> bool {
        let mut received = self.changed.swap(false, Ordering::SeqCst);
        for reply in self.replies.try_iter() { deliver(rt, reply); received = true; }
        received
    }
}

fn deliver<S: 'static>(rt: &mut Runtime<S>, reply: Reply) {
    match reply.purpose {
        Purpose::Paste(target, _) => rt.paste_result(target, reply.content),
        Purpose::Read(id) => rt.read_result(id, reply.content),
    }
}

/// Applies a runtime's clipboard requests to its memory clipboard, if it has one. Runs after
/// every frame, so headless runtimes write too.
pub(crate) fn flush_memory<S: 'static>(rt: &mut Runtime<S>) {
    let Some(m) = rt.clipboard.clone() else { return };
    if std::mem::take(&mut rt.clear_secret_request) { m.handle(Request::Clear(None)); }
    if let Some(copy) = rt.copy_request.take() {
        copy.bind(Route::Memory(m.clone()));
        m.handle(Request::Write(copy, None));
    }
    let paste = rt.paste_request.take().map(|target| Purpose::paste(rt, target));
    let reads = paste.into_iter().chain(std::mem::take(&mut rt.read_requests).into_iter().map(Purpose::Read));
    for purpose in reads.collect::<Vec<_>>() {
        if let Some(reply) = m.answer(purpose) { deliver(rt, reply); }
    }
}

impl<S: 'static> Runtime<S> {
    /// Use a shared in-memory clipboard (`clipboard::memory()`) instead of the OS one: copies,
    /// secret clears and pastes go to it, in headless runs too. For tests.
    pub fn use_clipboard(&mut self, clipboard: &MemoryClipboard) { self.clipboard = Some(clipboard.clone()); }
}

/// An in-memory clipboard for tests, shared by its clones and by every runtime given it with
/// `Runtime::use_clipboard`. It behaves like the OS clipboard, synchronously.
#[derive(Clone)]
pub struct MemoryClipboard(Arc<Mutex<Memory>>);
struct Memory { core: Core, board: Board }
#[derive(Default)]
struct Board { content: Option<ClipboardContent>, revision: u64, fail: Option<String> }

pub fn memory() -> MemoryClipboard {
    MemoryClipboard(Arc::new(Mutex::new(Memory { core: Core::default(), board: Board::default() })))
}
impl MemoryClipboard {
    pub fn text(&self) -> Option<Zeroizing<String>> { self.content().filter(ClipboardContent::has_plain).map(|c| c.text) }
    /// Every form on the clipboard, as written: pastes from it are sanitised, this isn't.
    pub fn content(&self) -> Option<ClipboardContent> { self.ticked(|_, board| board.content.clone()) }
    /// Changes the clipboard as another app would.
    pub fn set_text(&self, text: &str) { self.set_content(ClipboardContent::new(text)); }
    /// Puts several forms on the clipboard as another app would, e.g. HTML from a browser or
    /// RTF alone from a word processor.
    pub fn set_content(&self, content: ClipboardContent) {
        let mut m = lock(&self.0);
        let Memory { core, board } = &mut *m;
        board.content = Some(content);
        board.revision += 1;
        core.tick(board);
    }
    /// Makes later writes fail with `error`, or succeed again with `None`.
    pub fn fail_writes(&self, error: Option<&str>) { lock(&self.0).board.fail = error.map(Into::into); }
    fn tick(&self) { self.ticked(|_, _| ()) }
    fn answer(&self, purpose: Purpose) -> Option<Reply> { self.ticked(|core, board| core.answer(board, purpose)) }
    fn handle(&self, request: Request) { self.ticked(|core, board| core.handle(board, request)) }
    /// Lets a held secret expire or notice a change first, as the OS worker does per request.
    fn ticked<R>(&self, f: impl FnOnce(&mut Core, &mut Board) -> R) -> R {
        let mut m = lock(&self.0);
        let Memory { core, board } = &mut *m;
        core.tick(board);
        f(core, board)
    }
}
impl std::fmt::Debug for MemoryClipboard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("MemoryClipboard").finish_non_exhaustive() }
}
impl Backend for Board {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
        if let Some(error) = &self.fail { return Err(error.clone()); }
        self.content = Some(copy.content.clone());
        self.revision += 1;
        Ok(())
    }
    fn text(&mut self) -> Option<Zeroizing<String>> {
        self.content.as_ref().filter(|c| c.has_plain()).map(|c| c.text.clone())
    }
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        self.content.clone().and_then(|c| if all { Some(c) } else { c.text_forms() })
    }
    fn revision(&self) -> Option<u64> { Some(self.revision) }
    fn clear(&mut self) { self.content = None; self.revision += 1; }
}

/// Called by the shared event-loop exit path, including overlay-only applications.
pub(crate) fn shutdown() {
    #[cfg(not(target_arch = "wasm32"))]
    native::shutdown();
}

trait Backend {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String>;
    fn text(&mut self) -> Option<Zeroizing<String>>;
    /// Every form a paste can use, unsanitised, and the image and files only when asked (`all`):
    /// decoding an image can take a while.
    fn content(&mut self, _all: bool) -> Option<ClipboardContent> { self.text().map(ClipboardContent::from) }
    fn revision(&self) -> Option<u64>;
    fn clear(&mut self);
}
/// How often the worker checks that a held secret is still on the clipboard. Where there is no
/// change count (Linux) it compares the text's fingerprint, so it checks less often.
const WATCH: Duration = if cfg!(any(target_os = "macos", target_os = "windows")) {
    Duration::from_millis(250)
} else {
    Duration::from_secs(1)
};

/// The secret Rune wrote last, while the clipboard may still hold it.
struct Held {
    slot: Option<Arc<Slot>>,
    notify: Option<Notify>,
    deadline: Option<Instant>,
    revision: Option<u64>,
    /// A keyed hash of the text, so the text needn't stay in memory until the clear.
    print: u64,
}

/// The pill copy Rune wrote last, while the clipboard still holds that exact write.
struct OwnPills {
    revision: Option<u64>,
    text_print: u64,
    pills: Vec<(std::ops::Range<usize>, String)>,
}

/// The clipboard owner's bookkeeping, shared by the OS worker and the memory clipboard.
#[derive(Default)]
struct Core { keys: RandomState, held: Option<Held>, own_pills: Option<OwnPills> }
impl Core {
    fn next_check(&self) -> Option<Instant> {
        let held = self.held.as_ref()?;
        let watch = Instant::now() + WATCH;
        Some(held.deadline.map_or(watch, |d| d.min(watch)))
    }

    fn handle<B: Backend>(&mut self, cb: &mut B, request: Request) {
        match request {
            Request::Write(copy, notify) => self.write(cb, copy, notify),
            Request::Clear(which) => self.clear(cb, which.as_ref()),
            Request::Read { purpose, reply, wake } => {
                if let Some(answer) = self.answer(cb, purpose) && reply.send(answer).is_ok() { wake(); }
            }
            Request::Shutdown => {}
        }
    }

    fn answer<B: Backend>(&mut self, cb: &mut B, purpose: Purpose) -> Option<Reply> {
        let mut reply = purpose.answer(cb)?;
        if let Some(own) = &self.own_pills {
            let rev_ok = match (own.revision, cb.revision()) {
                (Some(a), Some(b)) => a == b,
                (None, None) => true,
                _ => false,
            };
            let html_ok = reply.content.html().is_some_and(|h| {
                let frag = crate::TextEdit::from_html(h);
                frag.pills().count() == own.pills.len()
                    && frag.pills().zip(&own.pills).all(|((r1, id1), (r2, id2))| r1 == *r2 && id1 == id2)
            });
            if rev_ok && self.keys.hash_one(reply.content.text()) == own.text_print && html_ok {
                reply.content.own_pills = true;
            }
        }
        Some(reply)
    }

    fn write<B: Backend>(&mut self, cb: &mut B, copy: ClipboardWrite, notify: Option<Notify>) {
        let slot = copy.slot().cloned();
        let notify = notify.filter(|_| slot.is_some());
        let cancelled = |slot: &Option<Arc<Slot>>| slot.as_ref().is_some_and(|s| s.cancel.load(Ordering::SeqCst));
        if cancelled(&slot) { return; } // `Unwritten` settles it as cleared.
        if let Err(error) = cb.write(&copy) {
            if let Some(s) = &slot { s.set(SecretState::Failed(error)); }
            if let Some(n) = &notify { n.send(); }
            return;
        }
        self.release(SecretState::Replaced);
        let rev = cb.revision();
        self.own_pills = copy
            .clear_after
            .is_none()
            .then(|| copy.content.html())
            .flatten()
            .filter(|h| h.contains("<span data-pill="))
            .map(|h| {
                let frag = crate::TextEdit::from_html(h);
                OwnPills {
                    revision: rev,
                    text_print: self.keys.hash_one(copy.content.text()),
                    pills: frag.pills().map(|(r, id)| (r, id.to_owned())).collect(),
                }
            })
            .filter(|o| !o.pills.is_empty());
        if let Some(delay) = copy.clear_after {
            if let Some(s) = &slot { s.set(SecretState::Holding); }
            self.held = Some(Held {
                slot: slot.clone(),
                notify: notify.clone(),
                deadline: Instant::now().checked_add(delay),
                revision: rev,
                print: self.keys.hash_one(copy.content.text()),
            });
            if let Some(n) = &notify { n.send(); }
            if cancelled(&slot) { self.clear(cb, slot.as_ref()); }
        }
    }

    fn release(&mut self, state: SecretState) {
        let Some(held) = self.held.take() else { return };
        if let Some(s) = &held.slot { s.set(state); }
        if let Some(n) = &held.notify { n.send(); }
    }

    fn unchanged<B: Backend>(&self, cb: &mut B) -> bool {
        let Some(held) = &self.held else { return false };
        held.revision == cb.revision() && cb.text().is_some_and(|t| self.keys.hash_one(t.as_str()) == held.print)
    }

    fn clear<B: Backend>(&mut self, cb: &mut B, which: Option<&Arc<Slot>>) {
        let Some(held) = &self.held else { return };
        if which.is_some_and(|w| !held.slot.as_ref().is_some_and(|s| Arc::ptr_eq(s, w))) { return; }
        if self.unchanged(cb) {
            cb.clear();
            self.own_pills = None;
            self.release(SecretState::Cleared);
        } else {
            self.release(SecretState::Replaced);
        }
    }

    /// Expire the held secret, or let it go if the clipboard changed under it.
    fn tick<B: Backend>(&mut self, cb: &mut B) {
        if let Some(own) = &self.own_pills
            && let Some(rev) = own.revision
            && cb.revision() != Some(rev)
        {
            self.own_pills = None;
        }
        let Some(held) = &self.held else { return };
        if held.deadline.is_some_and(|d| Instant::now() >= d) { return self.clear(cb, None); }
        let changed = match held.revision {
            Some(revision) => cb.revision() != Some(revision),
            None => !self.unchanged(cb),
        };
        if changed { self.release(SecretState::Replaced); }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::native::Worker;

    #[derive(Default)]
    struct State { text: String, revision: u64, clears: usize, drops: usize }
    struct Fake { state: Arc<Mutex<State>>, dropped: Option<mpsc::Sender<()>> }
    impl Backend for Fake {
        fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> {
            let mut s = self.state.lock().unwrap();
            s.text = copy.to_string(); s.revision += 1;
            Ok(())
        }
        fn text(&mut self) -> Option<Zeroizing<String>> { Some(Zeroizing::new(self.state.lock().unwrap().text.clone())) }
        fn revision(&self) -> Option<u64> { Some(self.state.lock().unwrap().revision) }
        fn clear(&mut self) { let mut s = self.state.lock().unwrap(); s.text.clear(); s.clears += 1; }
    }
    impl Drop for Fake {
        fn drop(&mut self) {
            self.state.lock().unwrap().drops += 1;
            if let Some(dropped) = &self.dropped { let _ = dropped.send(()); }
        }
    }

    const LONG: Duration = Duration::from_secs(3600);

    fn copy(m: &MemoryClipboard, text: &str, clear_after: Duration) -> SecretCopy {
        let (write, handle) = ClipboardWrite::secret(text.into(), clear_after);
        write.bind(Route::Memory(m.clone()));
        m.handle(Request::Write(write, None));
        handle
    }
    fn text(m: &MemoryClipboard) -> Option<String> { m.text().map(|t| t.to_string()) }

    #[test]
    fn shutdown_expires_only_our_pending_secret_before_dropping_backend() {
        for replacement in [None, Some("new value"), Some("fixture")] {
            let state = Arc::new(Mutex::new(State::default()));
            let shared = state.clone();
            let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
            worker.sender.send(Request::Write(ClipboardWrite::secret("fixture".into(), LONG).0, None)).unwrap();
            let (reply, received) = mpsc::channel();
            worker.sender.send(Request::Read { purpose: Purpose::Paste(None, Forms::Text), reply, wake: Arc::new(|| {}) }).unwrap();
            received.recv_timeout(Duration::from_secs(1)).unwrap(); // Write and revision recorded.
            if let Some(text) = replacement {
                let mut s = state.lock().unwrap(); s.text = text.into(); s.revision += 1;
            }
            worker.stop(); worker.stop();
            let s = state.lock().unwrap();
            assert_eq!(s.text, replacement.unwrap_or(""));
            assert_eq!(s.clears, usize::from(replacement.is_none()));
            assert_eq!(s.drops, 1, "normal shutdown joins after clipboard handoff/drop");
        }
    }

    #[test]
    fn shutdown_does_not_wait_indefinitely_for_a_blocked_backend() {
        let (entered, waiting) = mpsc::channel();
        let (release, released) = mpsc::channel();
        let (dropped, finished) = mpsc::channel();
        let worker = Worker::spawn(move || {
            entered.send(()).unwrap();
            released.recv().unwrap();
            Some(Fake { state: Arc::default(), dropped: Some(dropped.clone()) })
        });
        worker.sender.send(Request::Write(ClipboardWrite::secret("fixture".into(), LONG).0, None)).unwrap();
        waiting.recv_timeout(Duration::from_secs(1)).unwrap();
        let start = Instant::now();
        worker.stop();
        assert!(start.elapsed() < Duration::from_secs(1));
        release.send(()).unwrap();
        finished.recv_timeout(Duration::from_secs(1)).unwrap();
    }

    /// The OS worker reports the write, then notices another app's change on its own.
    #[test]
    fn worker_reports_the_write_and_watches_for_outside_changes() {
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
        let (woke, wakes) = mpsc::channel();
        let notify = Notify { changed: Arc::default(), wake: Arc::new(move || { let _ = woke.send(()); }) };
        let (write, handle) = ClipboardWrite::secret("fixture".into(), LONG);
        write.bind(Route::System(worker.sender.clone()));
        worker.sender.send(Request::Write(write, Some(notify.clone()))).unwrap();
        wakes.recv_timeout(Duration::from_secs(1)).unwrap();
        assert!(handle.holds() && notify.changed.load(Ordering::SeqCst));
        { let mut s = state.lock().unwrap(); s.text = "theirs".into(); s.revision += 1; }
        wakes.recv_timeout(WATCH * 8).unwrap();
        assert_eq!(handle.state(), SecretState::Replaced);
        handle.clear_now();
        worker.stop();
        assert_eq!(state.lock().unwrap().text, "theirs");
    }

    #[test]
    fn holds_until_a_later_copy_or_an_outside_change() {
        let m = memory();
        let first = copy(&m, "fixture", LONG);
        assert!(first.holds());
        let second = copy(&m, "fixture", LONG);
        assert_eq!(first.state(), SecretState::Replaced, "identical text from a later copy still supersedes");
        assert!(second.holds());
        m.set_text("theirs");
        assert_eq!(second.state(), SecretState::Replaced);
        let expired = copy(&m, "brief", Duration::ZERO);
        assert_eq!(expired.state(), SecretState::Cleared);
        assert_eq!(text(&m), None);
        assert!(!format!("{expired:?} {m:?}").contains("brief"));
    }

    #[test]
    fn clear_now_clears_only_while_it_holds() {
        let m = memory();
        let replaced = copy(&m, "secret", LONG);
        m.set_text("theirs");
        replaced.clear_now();
        assert_eq!(text(&m).as_deref(), Some("theirs"));
        let held = copy(&m, "secret", LONG);
        held.clear_now();
        held.clear_now();
        assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));
        let (write, unwritten) = ClipboardWrite::secret("never".into(), LONG);
        write.bind(Route::Memory(m.clone()));
        unwritten.clear_now();
        m.handle(Request::Write(write, None));
        assert_eq!((unwritten.state(), text(&m)), (SecretState::Cleared, None));
        let (write, dropped) = ClipboardWrite::secret("dropped".into(), LONG);
        drop(write);
        assert_eq!(dropped.state(), SecretState::Replaced);
    }

    #[test]
    fn a_failed_write_reports_and_keeps_the_earlier_copy() {
        let m = memory();
        let earlier = copy(&m, "earlier", LONG);
        m.fail_writes(Some("pasteboard denied"));
        let failed = copy(&m, "later", LONG);
        assert_eq!(failed.state(), SecretState::Failed("pasteboard denied".into()));
        assert!(earlier.holds());
        assert_eq!(text(&m).as_deref(), Some("earlier"));
    }

    #[test]
    fn two_runtimes_share_a_memory_clipboard() {
        use crate::{col, Cx, El, Env};
        fn view(_: &Option<SecretCopy>, _: &mut Cx<Option<SecretCopy>>) -> El { col() }
        let m = memory();
        let [mut a, mut b] = [0, 1].map(|_| {
            let mut rt = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
            rt.use_clipboard(&m);
            rt.frame(0.0);
            rt
        });
        a.update(|s, tx| *s = Some(tx.copy_secret("from a", LONG))); a.frame(0.0);
        assert!(a.state.as_ref().unwrap().holds());
        b.update(|s, tx| *s = Some(tx.copy_secret("from b", LONG))); b.frame(0.0);
        assert_eq!(text(&m).as_deref(), Some("from b"));
        assert_eq!(a.state.as_ref().unwrap().state(), SecretState::Replaced);
        a.update(|_, tx| tx.clear_secret_clipboard()); a.frame(0.0);
        assert_eq!(text(&m), None, "either window clears the latest secret");
        assert_eq!(b.state.as_ref().unwrap().state(), SecretState::Cleared);

        a.update(|s, tx| *s = Some(tx.copy_secret("no frame", LONG)));
        assert_eq!(text(&m).as_deref(), Some("no frame"), "update writes without waiting for a frame");

        let mut headless = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
        headless.update(|s, tx| *s = Some(tx.copy_secret("nowhere", LONG))); headless.frame(0.0);
        assert_eq!(headless.state.as_ref().unwrap().state(), SecretState::Pending, "no clipboard, no write");
    }

    #[test]
    fn rich_copies_carry_every_form_and_secrets_stay_plain() {
        use crate::{col, Cx, El, Env};
        fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }
        let m = memory();
        let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.use_clipboard(&m);
        rt.frame(0.0);
        let rich = ClipboardContent::new("Ship it").with_html("<b>Ship</b> it").with_rtf(r"{\rtf1 {\b Ship} it}");
        rt.update(|_, tx| tx.copy_rich(rich.clone()));
        assert_eq!(m.content().as_ref(), Some(&rich), "HTML is written as given; only reads are sanitised");
        assert_eq!(text(&m).as_deref(), Some("Ship it"));
        assert!(!format!("{rich:?} {:?}", ClipboardWrite::new(rich.clone())).contains("Ship"));

        rt.update(|_, tx| { tx.copy_secret("hunter2", LONG); });
        let secret = m.content().unwrap();
        assert!(!secret.is_rich() && secret.text() == "hunter2", "no HTML or RTF copy of a secret");
        rt.update(|_, tx| tx.clear_secret_clipboard());
        assert_eq!(m.content(), None);
    }

    /// A plain paste leaves an empty clipboard alone; a read always answers.
    #[test]
    fn the_worker_answers_reads_with_sanitised_content() {
        let state = Arc::new(Mutex::new(State::default()));
        let shared = state.clone();
        let worker = Worker::spawn(move || Some(Fake { state: shared.clone(), dropped: None }));
        state.lock().unwrap().text = "<script>".into();
        let (reply, replies) = mpsc::channel();
        worker.sender.send(Request::Read { purpose: Purpose::Read(7), reply, wake: Arc::new(|| {}) }).unwrap();
        let answer = replies.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!((answer.purpose, answer.content.text()), (Purpose::Read(7), "<script>"), "plain text is never rewritten");
        worker.stop();
        assert!(Purpose::Paste(None, Forms::Rich).reply(None).is_none());
        let mut board = Board { content: Some(ClipboardContent::new("t").with_html("<b>t</b>")), ..Board::default() };
        let plain = Purpose::Paste(None, Forms::Text).answer(&mut board).unwrap().content;
        assert!(!plain.is_rich() && plain.text() == "t", "a plain target reads only the text, as before");
        let read = Purpose::Read(1).reply(Some(ClipboardContent::new("").with_html("<p onclick=x>t</p>"))).unwrap();
        assert_eq!(read.content.html(), Some("<p>t</p>"));
        assert_eq!(Purpose::Read(2).reply(None).unwrap().content, ClipboardContent::default());
    }

    /// `on_close` runs through `update`, and a hidden window only polls: both must reach the
    /// clipboard before any frame.
    #[test]
    fn clears_from_on_close_and_a_hidden_poll_need_no_frame() {
        use crate::{col, Cx, El, Env};
        fn view(_: &u8, _: &mut Cx<u8>) -> El { col() }
        let m = memory();
        let mut rt = Runtime::new(0, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.use_clipboard(&m);
        rt.frame(0.0);
        let on_close: fn(&mut u8, &mut crate::Tx) = |_, tx| tx.clear_secret_clipboard();
        let held = copy(&m, "secret", LONG);
        rt.update(on_close);
        assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));

        rt.poll = Some(|_, _, tx| { tx.clear_secret_clipboard(); false });
        let held = copy(&m, "secret", LONG);
        rt.poll_hidden();
        assert_eq!((held.state(), text(&m)), (SecretState::Cleared, None));
    }

    #[test]
    fn a_clear_after_an_unwritten_copy_drops_the_stale_secret() {
        use crate::{col, Cx, El, Env};
        fn view(_: &Option<SecretCopy>, _: &mut Cx<Option<SecretCopy>>) -> El { col() }
        // No clipboard: the copy waits for the shell, as it does between window flushes.
        let mut rt = Runtime::new(None, view, (100.0, 100.0), Env::new(true, 1.0));
        rt.update(|s, tx| *s = Some(tx.copy_secret("stale", LONG)));
        rt.update(|_, tx| tx.clear_secret_clipboard());
        assert!(rt.copy_request.is_none() && rt.clear_secret_request);
        assert_eq!(rt.state.as_ref().unwrap().state(), SecretState::Replaced);

        // The same in one handler, and with a memory clipboard on the next flush.
        let m = memory();
        rt.use_clipboard(&m);
        rt.update(|s, tx| { *s = Some(tx.copy_secret("stale", LONG)); tx.clear_secret_clipboard(); });
        rt.frame(0.0);
        assert_eq!(text(&m), None);
        assert_eq!(rt.state.as_ref().unwrap().state(), SecretState::Replaced);
    }

    #[test]
    fn a_staged_pending_copy_stays_pending_and_clear_now_is_a_no_op() {
        let copy = SecretCopy::pending();
        let clone = copy.clone();
        drop(copy);
        assert_eq!(clone.state(), SecretState::Pending);
        clone.clear_now();
        assert_eq!(clone.state(), SecretState::Pending);
        assert!(!clone.holds());
    }
}
