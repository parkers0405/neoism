//! The OS clipboard: one worker thread over arboard, and the per-OS reads and writes.

use super::*;

impl Client {
    pub(super) fn read_for(&self, purpose: Purpose) {
        let _ = worker().send(Request::Read { purpose, reply: self.sender.clone(), wake: self.wake.clone() });
    }
    pub fn flush<S: 'static>(&self, rt: &mut Runtime<S>) {
        if rt.clipboard.is_some() { return flush_memory(rt); }
        // Nothing to clear before the first write.
        if std::mem::take(&mut rt.clear_secret_request) && let Some(w) = WORKER.get() { let _ = w.sender.send(Request::Clear(None)); }
        if let Some(copy) = rt.copy_request.take() {
            let worker = worker();
            copy.bind(Route::System(worker.clone()));
            let _ = worker.send(Request::Write(copy, Some(Notify { changed: self.changed.clone(), wake: self.wake.clone() })));
        }
        if let Some(target) = rt.paste_request.take() { self.read_for(Purpose::paste(rt, target)); }
        for id in std::mem::take(&mut rt.read_requests) { self.read_for(Purpose::Read(id)); }
    }
}

static WORKER: OnceLock<Worker> = OnceLock::new();
pub(super) struct Worker {
    pub(super) sender: mpsc::Sender<Request>,
    join: Mutex<Option<(std::thread::JoinHandle<()>, mpsc::Receiver<()>)>>,
}
impl Worker {
    pub(super) fn spawn<B: Backend + 'static>(open: impl FnMut() -> Option<B> + Send + 'static) -> Self {
        let (sender, recv) = mpsc::channel();
        let (done, completed) = mpsc::channel();
        let handle = std::thread::Builder::new().name("rune-clipboard".into()).spawn(move || {
            run_worker(recv, open);
            // Backend dropped (including Linux handoff) before acknowledging completion.
            let _ = done.send(());
        }).expect("clipboard worker");
        Self { sender, join: Mutex::new(Some((handle, completed))) }
    }
    pub(super) fn stop(&self) {
        let Some((handle, completed)) = self.join.lock().unwrap().take() else { return };
        let _ = self.sender.send(Request::Shutdown);
        // A hung clipboard owner must not hang application exit. Join only after completion.
        if completed.recv_timeout(Duration::from_millis(250)).is_ok() { let _ = handle.join(); }
    }
}
#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn worker() -> &'static mpsc::Sender<Request> {
    &WORKER.get_or_init(|| Worker::spawn(|| arboard::Clipboard::new().ok())).sender
}

#[cfg(any(target_os = "ios", target_os = "android"))]
fn worker() -> &'static mpsc::Sender<Request> {
    &WORKER.get_or_init(|| Worker::spawn(|| None::<Board>)).sender
}

/// Called by the shared event-loop exit path, including overlay-only applications.
pub(super) fn shutdown() {
    if let Some(worker) = WORKER.get() { worker.stop(); }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
impl Backend for arboard::Clipboard {
    fn write(&mut self, copy: &ClipboardWrite) -> Result<(), String> { write(self, copy) }
    fn text(&mut self) -> Option<Zeroizing<String>> { self.get_text().ok().map(Zeroizing::new) }
    #[cfg(target_os = "macos")]
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        macos::read(&*unsafe { objc2_app_kit::NSPasteboard::generalPasteboard() }, all)
    }
    /// Text, HTML, and with `all` an image and files; RTF isn't read here. Windows and Linux
    /// images come through arboard as pixels (Windows `PNG` or `CF_DIBV5`, Linux `image/png`),
    /// encoded as PNG again.
    #[cfg(not(target_os = "macos"))]
    fn content(&mut self, all: bool) -> Option<ClipboardContent> {
        let text = self.text();
        #[cfg(target_os = "windows")]
        let html = windows_read_html().or_else(|| self.get().html().ok()).filter(|h| !h.is_empty() && h.len() <= html::MAX_RICH_BYTES);
        #[cfg(not(target_os = "windows"))]
        let html = self.get().html().ok().filter(|h| !h.is_empty() && h.len() <= html::MAX_RICH_BYTES);
        #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android")))))]
        let image = all.then(|| self.get_image().ok()).flatten()
            .and_then(|i| u32::try_from(i.width).ok().zip(u32::try_from(i.height).ok()).and_then(|(w, h)| ClipboardImage::from_rgba(w, h, &i.bytes)));
        #[cfg(not(any(target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android"))))))]
        let image: Option<ClipboardImage> = None;
        let files = if all { self.get().file_list().unwrap_or_default() } else { Vec::new() };
        if text.is_none() && html.is_none() && image.is_none() && files.is_empty() { return None; }
        let mut content = ClipboardContent::from(text.unwrap_or_default()).with_files(files);
        if let Some(html) = html { content = content.with_html(html); }
        if let Some(image) = image { content = content.with_image(image); }
        Some(content)
    }
    fn revision(&self) -> Option<u64> { revision() }
    fn clear(&mut self) { let _ = arboard::Clipboard::clear(self); }
}

fn run_worker<B: Backend>(recv: mpsc::Receiver<Request>, mut open: impl FnMut() -> Option<B>) {
    let mut clipboard: Option<B> = None;
    let mut core = Core::default();
    loop {
        let request = match core.next_check() {
            Some(at) => recv.recv_timeout(at.saturating_duration_since(Instant::now())),
            None => recv.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected),
        };
        if let Some(cb) = &mut clipboard { core.tick(cb); }
        let request = match request {
            Ok(Request::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(request) => request,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
        };
        if matches!(request, Request::Clear(_)) && core.held.is_none() { continue; }
        if clipboard.is_none() { clipboard = open(); }
        let Some(cb) = &mut clipboard else {
            if let Request::Write(copy, notify) = request && let Some(slot) = copy.slot() {
                slot.set(SecretState::Failed("no clipboard".into()));
                if let Some(n) = notify { n.send(); }
            }
            continue;
        };
        core.handle(cb, request);
    }
    if let Some(cb) = clipboard.as_mut() { core.clear(cb, None); }
    drop(clipboard);
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn write(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), String> {
    let content = &copy.content;
    if copy.clear_after.is_none() && content.is_rich() {
        #[cfg(target_os = "macos")]
        { return macos::write(&*unsafe { objc2_app_kit::NSPasteboard::generalPasteboard() }, content); }
        #[cfg(not(target_os = "macos"))]
        return write_one_form(cb, content);
    }
    write_text(cb, copy).map_err(|e| e.to_string())
}

/// Windows and Linux hold one of Rune's forms per write: the files, else the image, else HTML
/// beside the text (CF_HTML, `text/html`), else the text. RTF isn't written.
#[cfg(all(not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
fn write_one_form(cb: &mut arboard::Clipboard, content: &ClipboardContent) -> Result<(), String> {
    let error = |e: arboard::Error| e.to_string();
    if !content.files().is_empty() { return cb.set().file_list(content.files()).map_err(error); }
    #[cfg(any(target_os = "windows", all(unix, not(any(target_os = "ios", target_os = "android")))))]
    if let Some(rgba) = content.image().and_then(ClipboardImage::to_rgba) {
        let (width, height) = (rgba.width() as usize, rgba.height() as usize);
        return cb.set_image(arboard::ImageData { width, height, bytes: rgba.into_raw().into() }).map_err(error);
    }
    match content.html() {
        Some(html) => cb.set().html(html, Some(content.text())).map_err(error),
        None => cb.set_text(content.text()).map_err(error),
    }
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn write_text(cb: &mut arboard::Clipboard, copy: &ClipboardWrite) -> Result<(), arboard::Error> {
    let text = copy.content.text();
    if copy.clear_after.is_none() { return cb.set_text(text); }
    #[cfg(target_os = "macos")]
    { use arboard::SetExtApple; return cb.set().exclude_from_history().text(text); }
    #[cfg(target_os = "windows")]
    { use arboard::SetExtWindows; return cb.set().exclude_from_monitoring().text(text); }
    #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
    { use arboard::SetExtLinux; return cb.set().exclude_from_history().text(text); }
    #[allow(unreachable_code)]
    cb.set_text(text)
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
fn revision() -> Option<u64> {
    #[cfg(target_os = "macos")]
    { return Some(unsafe { objc2_app_kit::NSPasteboard::generalPasteboard().changeCount() } as u64); }
    #[cfg(target_os = "windows")]
    { return Some(unsafe { windows_sys::Win32::System::DataExchange::GetClipboardSequenceNumber() } as u64); }
    // Linux's portable clipboard API has no generation counter; compare the current text.
    #[allow(unreachable_code)]
    None
}

/// Reads Windows `CF_HTML` (`HTML Format`) directly so `-1` offsets and `<!--StartFragment-->`
/// markers from other apps work instead of failing `clipboard-win`'s unsigned offset parser.
#[cfg(target_os = "windows")]
fn windows_read_html() -> Option<String> {
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, GetClipboardData, IsClipboardFormatAvailable, OpenClipboard, RegisterClipboardFormatW,
    };
    use windows_sys::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    const NAME: &[u16] = &[b'H' as u16, b'T' as u16, b'M' as u16, b'L' as u16, b' ' as u16, b'F' as u16, b'o' as u16, b'r' as u16, b'm' as u16, b'a' as u16, b't' as u16, 0];
    unsafe {
        let format = RegisterClipboardFormatW(NAME.as_ptr());
        if format == 0 || IsClipboardFormatAvailable(format) == 0 { return None; }
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(core::ptr::null_mut()) != 0 { opened = true; break; }
            std::thread::sleep(Duration::from_millis(5));
        }
        if !opened { return None; }
        let handle = GetClipboardData(format);
        let out = if !handle.is_null() {
            let size = GlobalSize(handle);
            let ptr = GlobalLock(handle) as *const u8;
            let res = (!ptr.is_null() && size > 0 && size <= html::MAX_RICH_BYTES * 2)
                .then(|| html::unpack_cf_html(core::slice::from_raw_parts(ptr, size)))
                .flatten();
            if !ptr.is_null() { let _ = GlobalUnlock(handle); }
            res
        } else {
            None
        };
        let _ = CloseClipboard();
        out
    }
}

