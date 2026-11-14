//! `Tx`'s clipboard requests, carried out by the shell's clipboard worker after the handler.
use std::rc::Rc;
use crate::Tx;

pub(crate) type ReadFn = Box<dyn FnOnce(crate::states::States<'_>, &crate::ClipboardContent, &mut Tx)>;

/// A `Tx::read_clipboard` callback. `Tx` is `Clone`; clones share it and the runtime takes it once.
#[derive(Clone)]
pub(crate) struct ClipboardRead(Rc<std::cell::Cell<Option<ReadFn>>>);
impl ClipboardRead {
    pub(crate) fn take(&self) -> Option<ReadFn> { self.0.take() }
}
impl std::fmt::Debug for ClipboardRead {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.write_str("ClipboardRead") }
}

impl Tx {
    /// Request a clipboard write from the shell after this handler finishes.
    pub fn copy(&mut self, text: impl Into<String>) {
        self.copy = Some(crate::ClipboardWrite::new(crate::ClipboardContent::new(text)));
    }

    /// Copy several forms at once: plain text for every app, HTML and RTF for apps that paste
    /// formatting (Mail, Notes, Pages, Google Docs), an image, and files for a file manager.
    /// macOS writes every form. Windows and Linux write one: the files if there are any, else
    /// the image, else the HTML beside the text, else the text. The web writes text, HTML and
    /// the image. Forms a platform can't hold are skipped silently.
    ///
    /// ```ignore
    /// tx.copy_rich(ClipboardContent::new("Ship it").with_html("<b>Ship</b> it"));
    /// tx.copy_rich(ClipboardContent::default().with_image(image));
    /// tx.copy_rich(ClipboardContent::default().with_files([path]));
    /// ```
    pub fn copy_rich(&mut self, content: crate::ClipboardContent) {
        self.copy = Some(crate::ClipboardWrite::new(content));
    }

    /// Exclude this text from clipboard histories and clear it after `clear_after` if unchanged.
    /// The handle reports whether the copy still holds the clipboard, and can clear it early.
    pub fn copy_secret(&mut self, text: impl Into<String>, clear_after: std::time::Duration) -> crate::SecretCopy {
        let (write, copy) = crate::ClipboardWrite::secret(text.into(), clear_after);
        self.copy = Some(write);
        copy
    }

    /// Clear the secret Rune copied last, from any window, if the clipboard still holds it
    /// (on lock, say). A secret copy asked for earlier in this handler is dropped.
    pub fn clear_secret_clipboard(&mut self) {
        if self.copy.as_ref().is_some_and(crate::ClipboardWrite::is_secret) { self.copy = None; }
        self.clear_secret = true;
    }

    /// Read the OS clipboard asynchronously and paste into the focused field or root `on_paste`.
    pub fn request_paste(&mut self) { self.paste = true; }

    /// Read the clipboard off the event thread and hand what it holds to `f`, as an update,
    /// once it arrives: an empty [`ClipboardContent`](crate::ClipboardContent) when it holds
    /// nothing readable. HTML is sanitised, and on macOS made from RTF when that's all there
    /// is. Nothing is pasted anywhere. The web asks the browser, which may prompt or refuse.
    ///
    /// ```ignore
    /// tx.read_clipboard(|s: &mut Composer, content, _| s.preview = content.html().map(Into::into));
    /// ```
    pub fn read_clipboard<S: 'static>(&mut self, f: impl FnOnce(&mut S, &crate::ClipboardContent, &mut Tx) + 'static) {
        let f: ReadFn = Box::new(move |s, content, tx| {
            if let Some(s) = s.cast::<S>() { f(s, content, tx); }
        });
        self.clipboard_reads.push(ClipboardRead(Rc::new(std::cell::Cell::new(Some(f)))));
    }
}
