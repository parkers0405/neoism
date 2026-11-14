//! Inject native dialogs in the shell, scripted responses in headless fixtures.
//! Native futures use rfd on macOS and Windows, and the XDG desktop portal's FileChooser
//! over zbus on Linux.
//!
//! A handler can also ask with [`Tx::open_file`], [`Tx::save_file`] or [`Tx::open_folder`]: the window shows the
//! dialog once the handler's frame is done, and the answer runs the callback as an action on
//! a later frame. Headless runs keep the dialog open in `--tree` (`fileDialog`) until a
//! script's `{"type": "file-dialog", "path": …}` or `{"type": "file-dialog-cancel"}` answers it.
use std::cell::RefCell;
use std::collections::VecDeque;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::task::{Context, Poll};

use crate::Tx;
use crate::runtime::Waker;

#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
mod portal;
#[cfg(any(all(feature = "native-file-dialog", target_os = "linux"), all(test, unix)))]
mod portal_format;
#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
pub(crate) use portal::window_focus;
#[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
use portal::show as native;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FileDialogOptions {
    pub title: String,
    pub directory: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<(String, Vec<String>)>,
}
impl FileDialogOptions {
    pub fn filter(mut self, name: impl Into<String>, extensions: &[&str]) -> Self {
        self.filters.push((name.into(), extensions.iter().map(|s| (*s).into()).collect())); self
    }
}

/// Which dialog: Open picks an existing file, Save names a file to write, Folder picks an
/// existing folder (its options' filters and file name don't apply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileDialogKind { Open, Save, Folder }

impl FileDialogKind {
    fn name(self) -> &'static str { match self { Self::Open => "open", Self::Save => "save", Self::Folder => "folder" } }
}

/// Expected request and its response. `None` is cancellation, not a missing fixture.
#[derive(Clone, Debug)]
pub struct ScriptedFileDialog {
    pub kind: FileDialogKind,
    pub path: Option<PathBuf>,
}

#[derive(Clone)]
pub struct FileDialogs(Backend);
#[derive(Clone)]
enum Backend {
    #[cfg(feature = "native-file-dialog")]
    Native,
    Scripted(Arc<Mutex<VecDeque<ScriptedFileDialog>>>),
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileDialogError(pub String);
impl std::fmt::Display for FileDialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str(&self.0) }
}
impl std::error::Error for FileDialogError {}

impl FileDialogs {
    /// Native dialogs are opt-in at the app's capability boundary. Headless apps inject
    /// `scripted` instead, so tests never depend on filesystem UI or a display server.
    /// Poll asynchronously: blocking the window's main thread (for example with
    /// `pollster::block_on`) deadlocks Cocoa's dialog presentation. On Linux, `open` and
    /// `save` return an error when the desktop has no XDG portal FileChooser.
    #[cfg(feature = "native-file-dialog")]
    pub fn native() -> Self { Self(Backend::Native) }
    pub fn scripted(responses: impl IntoIterator<Item = ScriptedFileDialog>) -> Self {
        Self(Backend::Scripted(Arc::new(Mutex::new(responses.into_iter().collect()))))
    }
    pub async fn open(&self, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
        self.show(FileDialogKind::Open, options).await
    }
    pub async fn save(&self, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
        self.show(FileDialogKind::Save, options).await
    }
    pub async fn open_folder(&self, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
        self.show(FileDialogKind::Folder, options).await
    }
    async fn show(&self, kind: FileDialogKind, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
        match &self.0 {
            Backend::Scripted(responses) => {
                let mut responses = responses.lock().unwrap();
                let response = responses.front().ok_or_else(|| FileDialogError("file dialog fixture exhausted".into()))?;
                if response.kind != kind {
                    return Err(FileDialogError(format!("expected {:?} dialog, received {kind:?}", response.kind)));
                }
                let _ = options;
                Ok(responses.pop_front().unwrap().path)
            }
            #[cfg(feature = "native-file-dialog")]
            Backend::Native => native(kind, options).await,
        }
    }
}

#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
async fn native(kind: FileDialogKind, options: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
    rfd_show(rfd_dialog(options), kind).await
}
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
fn rfd_dialog(options: FileDialogOptions) -> rfd::AsyncFileDialog {
    let mut dialog = rfd::AsyncFileDialog::new().set_title(options.title);
    if let Some(directory) = options.directory { dialog = dialog.set_directory(directory); }
    if let Some(file_name) = options.file_name { dialog = dialog.set_file_name(file_name); }
    for (name, extensions) in options.filters { dialog = dialog.add_filter(name, &extensions); }
    dialog
}
/// Calling this starts the dialog (on macOS, a sheet over the parent); awaiting reads it.
#[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
fn rfd_show(dialog: rfd::AsyncFileDialog, kind: FileDialogKind) -> impl Future<Output = Result<Option<PathBuf>, FileDialogError>> {
    let file: Pin<Box<dyn Future<Output = Option<rfd::FileHandle>> + Send>> = match kind {
        FileDialogKind::Open => Box::pin(dialog.pick_file()),
        FileDialogKind::Save => Box::pin(dialog.save_file()),
        FileDialogKind::Folder => Box::pin(dialog.pick_folder()),
    };
    async move { Ok(file.await.map(|file| file.path().to_path_buf())) }
}
#[cfg(all(feature = "native-file-dialog", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
async fn native(_: FileDialogKind, _: FileDialogOptions) -> Result<Option<PathBuf>, FileDialogError> {
    Err(FileDialogError("native file dialogs aren't supported on this platform".into()))
}

/// What a [`Tx::open_file`], [`Tx::save_file`] or [`Tx::open_folder`] callback gets: the chosen path, `Ok(None)`
/// when the user cancels, or why no dialog could show.
pub type FileDialogResult = Result<Option<PathBuf>, FileDialogError>;

pub(crate) type AnswerFn = Box<dyn FnOnce(crate::states::States<'_>, FileDialogResult, &mut Tx)>;

pub(crate) struct Request {
    pub kind: FileDialogKind,
    pub options: FileDialogOptions,
    pub on_answer: AnswerFn,
}

/// A `Tx`'s dialog request. `Tx` is `Clone`; clones share the request and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct Call(Rc<RefCell<Option<Request>>>);

impl Call {
    pub(crate) fn take(&self) -> Option<Request> { self.0.borrow_mut().take() }
}

impl std::fmt::Debug for Call {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let request = self.0.borrow();
        f.debug_tuple("FileDialogCall").field(&request.as_ref().map(|r| (r.kind, &r.options))).finish()
    }
}

/// The runtime's dialog, from the request until its answer is queued.
pub(crate) struct Pending {
    pub id: u64,
    pub kind: FileDialogKind,
    pub options: FileDialogOptions,
    /// A host has it on screen.
    pub shown: bool,
}

impl Pending {
    pub(crate) fn to_json(&self) -> serde_json::Value {
        let o = &self.options;
        let filters: Vec<_> = o.filters.iter().map(|(name, ext)| serde_json::json!({ "name": name, "extensions": ext })).collect();
        let mut v = serde_json::json!({ "kind": self.kind.name(), "title": o.title, "filters": filters });
        if let Some(dir) = &o.directory { v["directory"] = dir.display().to_string().into(); }
        if let Some(name) = &o.file_name { v["fileName"] = name.clone().into(); }
        v
    }
}

impl Tx {
    /// Show an Open dialog once this handler's frame is done. `on_answer` runs, as an action on
    /// a later frame, with the chosen file, `Ok(None)` when the user cancels (or the window
    /// hides or closes), or an error when no dialog could show: a second dialog asked for while
    /// one is open, a window that stays hidden, an overlay, the web, or a build without the
    /// `native-file-dialog` feature. Asked for in the same turn that shows a hidden window, it
    /// shows with the window. The window keeps drawing and handling events while the dialog is
    /// up: it's a sheet on macOS, owned by the window on Windows, and the desktop portal's
    /// chooser on Linux.
    ///
    /// ```ignore
    /// .on_press_tx(|_: &mut App, tx: &mut Tx| {
    ///     let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("Export", &["json"]);
    ///     tx.open_file(options, |s: &mut App, result, _| match result {
    ///         Ok(Some(path)) => s.import(path),
    ///         Ok(None) => {}
    ///         Err(e) => s.error = Some(e.to_string()),
    ///     });
    /// })
    /// ```
    pub fn open_file<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Open, options, on_answer);
    }

    /// [`Tx::open_file`] for a Save dialog: the path to write, which may not exist yet.
    pub fn save_file<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Save, options, on_answer);
    }

    /// [`Tx::open_file`] for a folder: an Open dialog that picks an existing folder, as an app
    /// opening a project or choosing where exports go. `options`' title and directory apply;
    /// its filters and file name don't. A script answers it as it does a file dialog.
    pub fn open_folder<S: 'static>(&mut self, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        self.file_dialog(FileDialogKind::Folder, options, on_answer);
    }

    fn file_dialog<S: 'static>(&mut self, kind: FileDialogKind, options: FileDialogOptions, on_answer: impl FnOnce(&mut S, FileDialogResult, &mut Tx) + 'static) {
        let on_answer: AnswerFn = Box::new(move |s, result, tx| {
            // None once the playground has dropped the stage that asked.
            if let Some(s) = s.cast::<S>() { on_answer(s, result, tx); }
        });
        self.file_dialogs.push(Call(Rc::new(RefCell::new(Some(Request { kind, options, on_answer })))));
    }
}

/// One window's native file dialog: shows what its runtime asks for, and queues the answer
/// after `wake` brings the host back to poll it. The dialog's future owns copies of the
/// options and the window's handle, never the app's state or the runtime.
pub(crate) struct Presenter {
    showing: Option<(u64, Pin<Box<dyn Future<Output = FileDialogResult>>>)>,
    #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
    sheet_parent: Option<objc2::rc::Retained<objc2_app_kit::NSWindow>>,
    wake: std::task::Waker,
}

struct Wake(Waker);
impl std::task::Wake for Wake {
    fn wake(self: Arc<Self>) { (self.0)() }
    fn wake_by_ref(self: &Arc<Self>) { (self.0)() }
}

impl Presenter {
    pub(crate) fn new(wake: Waker) -> Self {
        Self {
            showing: None,
            #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
            sheet_parent: None,
            wake: Arc::new(Wake(wake)).into(),
        }
    }

    /// Queues the answer of a dialog that closed. True when one did.
    pub(crate) fn answer<S: 'static>(&mut self, rt: &mut crate::Runtime<S>) -> bool {
        let Some((id, future)) = &mut self.showing else { return false };
        let Poll::Ready(result) = future.as_mut().poll(&mut Context::from_waker(&self.wake)) else { return false };
        let id = *id;
        self.showing = None;
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        {
            self.sheet_parent = None;
        }
        rt.answer_file_dialog(id, result);
        true
    }

    /// Shows the runtime's dialog over `window`, starting it before this returns, so nothing
    /// blocks the event loop. A dialog asked for while the window is hidden waits for it to
    /// show. True when an answer was queued.
    pub(crate) fn open<S: 'static>(&mut self, rt: &mut crate::Runtime<S>, window: Option<&winit::window::Window>) -> bool {
        let Some(window) = window.filter(|_| self.showing.is_none()) else { return false };
        let Some((id, kind, options)) = rt.take_file_dialog() else { return false };
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        {
            self.sheet_parent = crate::platform::macos::ns_window(window);
        }
        self.showing = Some((id, show_over(kind, options, window)));
        self.answer(rt)
    }

    /// The window is going away (hidden, closed or quitting): any attached sheet is ended and
    /// the open or waiting dialog is answered `Ok(None)` while its state is alive.
    pub(crate) fn cancel<S: 'static>(&mut self, rt: &mut crate::Runtime<S>) {
        self.stop();
        rt.cancel_file_dialogs();
    }

    fn stop(&mut self) {
        self.showing = None;
        #[cfg(all(feature = "native-file-dialog", target_os = "macos"))]
        if let Some(window) = self.sheet_parent.take() {
            // SAFETY: the host's (main) thread; `window` is a live `Retained<NSWindow>`.
            unsafe {
                if let Some(sheet) = window.attachedSheet() {
                    window.endSheet(&sheet);
                }
            }
        }
    }
}

impl Drop for Presenter {
    fn drop(&mut self) { self.stop(); }
}

fn show_over(kind: FileDialogKind, options: FileDialogOptions, window: &winit::window::Window) -> Pin<Box<dyn Future<Output = FileDialogResult>>> {
    #[cfg(all(feature = "native-file-dialog", any(target_os = "macos", target_os = "windows")))]
    return Box::pin(rfd_show(rfd_dialog(options).set_parent(window), kind));
    // The portal's parent is the focused window (`window_focus`).
    #[cfg(all(feature = "native-file-dialog", target_os = "linux"))]
    return { let _ = window; Box::pin(native(kind, options)) };
    #[cfg(all(feature = "native-file-dialog", not(any(target_os = "linux", target_os = "macos", target_os = "windows"))))]
    return { let _ = window; Box::pin(native(kind, options)) };
    #[cfg(not(feature = "native-file-dialog"))]
    {
        let _ = (kind, options, window);
        Box::pin(std::future::ready(Err(FileDialogError("file dialogs need Rune's native-file-dialog feature".into()))))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scripted_dialogs_preserve_order_unicode_paths_and_cancellation() {
        let path = PathBuf::from("/fixture/录音.wav");
        let dialogs = FileDialogs::scripted([
            ScriptedFileDialog { kind: FileDialogKind::Open, path: Some(path.clone()) },
            ScriptedFileDialog { kind: FileDialogKind::Save, path: None },
            ScriptedFileDialog { kind: FileDialogKind::Folder, path: Some("/fixture/Projects".into()) },
        ]);
        let options = FileDialogOptions::default().filter("Audio", &["wav", "flac"]);
        assert!(pollster::block_on(dialogs.save(options.clone())).is_err());
        assert_eq!(pollster::block_on(dialogs.open(options.clone())).unwrap(), Some(path));
        assert_eq!(pollster::block_on(dialogs.clone().save(options.clone())).unwrap(), None);
        assert!(pollster::block_on(dialogs.open(options.clone())).is_err(), "a folder fixture doesn't answer an Open");
        assert_eq!(pollster::block_on(dialogs.open_folder(options.clone())).unwrap(), Some("/fixture/Projects".into()));
        assert!(pollster::block_on(dialogs.open(options)).is_err());
    }

    use crate::{Cx, El, Env, Runtime, el};

    #[derive(Default)]
    struct Import { answers: Vec<Result<Option<PathBuf>, String>> }

    fn import(_: &Import, _: &mut Cx<Import>) -> El {
        el().key("import").size(80.0, 20.0).on_press_tx(|_: &mut Import, tx: &mut Tx| {
            let options = FileDialogOptions { title: "Import".into(), ..Default::default() }.filter("Export", &["json"]);
            tx.open_file(options, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0)));
        })
    }

    fn press(rt: &mut Runtime<Import>) {
        rt.pointer_down(10.0, 10.0);
        rt.pointer_up(10.0, 10.0);
        rt.frame(1.0 / 60.0);
    }

    #[test]
    fn a_handlers_dialog_waits_open_and_its_answer_runs_on_a_later_frame() {
        let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        press(&mut rt);
        let (kind, options) = rt.file_dialog().expect("the press asked for a dialog");
        assert_eq!((kind, options.title.as_str(), options.filters.as_slice()), (FileDialogKind::Open, "Import", &[("Export".to_owned(), vec!["json".to_owned()])][..]));
        assert_eq!(rt.file_dialog_json().unwrap(), serde_json::json!({ "kind": "open", "title": "Import", "filters": [{ "name": "Export", "extensions": ["json"] }] }));
        rt.frame(2.0 / 60.0);
        assert!(rt.state.answers.is_empty() && rt.file_dialog().is_some(), "it stays open across frames");

        rt.resolve_file_dialog(Some("/exports/录音.json".into())).unwrap();
        assert!(rt.file_dialog().is_none() && rt.state.answers.is_empty(), "closed; the answer waits for a frame");
        assert!(rt.has_pending_input(), "so the host draws one");
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.state.answers, [Ok(Some("/exports/录音.json".into()))]);

        press(&mut rt);
        rt.resolve_file_dialog(None).unwrap();
        rt.frame(5.0 / 60.0);
        assert_eq!(rt.state.answers[1], Ok(None), "cancelling answers None");
        assert!(rt.resolve_file_dialog(None).is_err(), "nothing is open");
    }

    #[test]
    fn a_folder_dialog_waits_open_as_a_folder_and_answers_with_the_folder() {
        fn view(_: &Import, _: &mut Cx<Import>) -> El {
            el().key("open").size(80.0, 20.0).on_press_tx(|_: &mut Import, tx: &mut Tx| {
                let options = FileDialogOptions { title: "Open Project".into(), directory: Some("/work".into()), ..Default::default() };
                tx.open_folder(options, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0)));
            })
        }
        let mut rt = Runtime::new(Import::default(), view, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        press(&mut rt);
        assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.title.clone())), Some((FileDialogKind::Folder, "Open Project".into())));
        assert_eq!(rt.file_dialog_json().unwrap(), serde_json::json!({ "kind": "folder", "title": "Open Project", "filters": [], "directory": "/work" }));
        rt.resolve_file_dialog(Some("/work/notes".into())).unwrap();
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.answers, [Ok(Some("/work/notes".into()))]);
    }

    #[test]
    fn a_second_dialog_while_one_is_open_is_refused_and_the_first_stays() {
        let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        press(&mut rt);
        press(&mut rt);
        rt.frame(3.0 / 60.0);
        assert_eq!(rt.state.answers, [Err("a file dialog is already open".to_owned())]);
        rt.update(|_, tx| tx.save_file(FileDialogOptions::default(), |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0))));
        rt.frame(4.0 / 60.0);
        assert_eq!(rt.state.answers.len(), 2, "an update's dialog too");
        assert_eq!(rt.file_dialog().map(|(kind, _)| kind), Some(FileDialogKind::Open));
        rt.resolve_file_dialog(Some("a.json".into())).unwrap();
        rt.frame(5.0 / 60.0);
        assert_eq!(rt.state.answers[2], Ok(Some("a.json".into())));

        rt.update(|_, tx| tx.save_file(FileDialogOptions { file_name: Some("out.json".into()), ..Default::default() }, |s: &mut Import, result, _| s.answers.push(result.map_err(|e| e.0))));
        assert_eq!(rt.file_dialog().map(|(kind, o)| (kind, o.file_name.clone())), Some((FileDialogKind::Save, Some("out.json".into()))), "once it closed the next opens");
    }

    #[test]
    fn a_host_shows_the_dialog_once_and_answers_it_by_id() {
        let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        press(&mut rt);
        let (id, kind, _) = rt.take_file_dialog().expect("the host takes it");
        assert_eq!(kind, FileDialogKind::Open);
        assert!(rt.take_file_dialog().is_none(), "showing, so not taken again");
        rt.answer_file_dialog(id, Err(FileDialogError("no portal".into())));
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.answers, [Err("no portal".to_owned())]);

        press(&mut rt);
        rt.refuse_file_dialog("unsupported");
        rt.frame(4.0 / 60.0);
        assert_eq!(rt.state.answers[1], Err("unsupported".to_owned()), "a host without dialogs refuses it");
    }

    #[test]
    fn an_overlay_refuses_file_dialogs() {
        let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
        rt.native_overlay = true;
        rt.frame(0.0);
        press(&mut rt);
        assert!(rt.file_dialog().is_none(), "overlays never leave a dialog pending");
        assert!(rt.keys_queued(), "hidden_input delivers the refusal without waiting for a show");
        rt.frame(2.0 / 60.0);
        assert_eq!(rt.state.answers, [Err("overlays cannot open file dialogs".to_owned())]);
    }

    #[test]
    fn closing_or_hiding_cancels_open_and_queued_file_dialogs() {
        let mut rt = Runtime::new(Import::default(), import, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        rt.update(|_, tx| {
            tx.open_file(FileDialogOptions::default(), |s: &mut Import, result, tx| {
                s.answers.push(result.map_err(|e| e.0));
                tx.save_file(FileDialogOptions::default(), |s: &mut Import, follow, _| {
                    s.answers.push(follow.map_err(|e| e.0));
                });
            });
            tx.save_file(FileDialogOptions::default(), |s: &mut Import, result, _| {
                s.answers.push(result.map_err(|e| e.0));
            });
        });
        assert!(rt.file_dialog().is_some());
        rt.cancel_file_dialogs();
        assert!(rt.file_dialog().is_none());
        assert_eq!(
            rt.state.answers,
            [
                Err("a file dialog is already open".to_owned()),
                Ok(None),
                Ok(None),
            ],
            "queued answers, the open dialog, and a dialog asked for in its callback all settle before the host drops"
        );
    }
}
