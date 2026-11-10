//! Image (and file) attachments on the agent composer (`attachment_policy.rs`,
//! `state/input_edit.rs::attach_file_bytes`, GPUiX `App.tsx`'s attachment strip and
//! `ui/AttachmentImage.tsx`):
//!
//! - The paperclip, a pasted image or files dropped on the card attach to the active tab's
//!   draft: each adds an `[imageN]` / `[pdfN]` / `[fileN: name]` token to the text and a chip
//!   in a strip of 64 pt thumbnails at the top of the composer card, removed with its ×.
//! - Submitting sends them with the prompt (`Attachment { kind: mime, path: name, bytes }`),
//!   20 MB each at most, and echoes them into the transcript as `data:` URLs.
//! - User messages show their images as 150 pt thumbnails at the top of the bubble (their
//!   tokens dropped from the text); clicking one opens the full-window preview, which Esc, the
//!   × or a click on the backdrop closes.

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::theme::rgb;
use base64::Engine as _;
use neoism_protocol::agent::{Attachment, HistoryAttachment};
use rune::{El, KeyEvent, Tx, col, el, row, text};
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// `MAX_INLINE_ATTACHMENT_BYTES`.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
/// `COMPOSER_THUMB_SIZE` and `MESSAGE_THUMB_SIZE`.
pub const COMPOSER_THUMB: f32 = 64.0;
pub const MESSAGE_THUMB: f32 = 150.0;
const GAP: f32 = 10.0;

/// One attachment waiting on a draft.
#[derive(Clone)]
pub struct Attached {
    pub token: String,
    pub name: String,
    pub mime: String,
    pub bytes: Arc<[u8]>,
}

impl Attached {
    pub fn is_image(&self) -> bool { self.mime.starts_with("image/") }
    /// The cache key its image decodes under.
    pub fn key(&self) -> String { format!("draft:{}:{}", self.token, self.bytes.len()) }
}

/// The open full-window preview: the image's cache key and name.
#[derive(Clone)]
pub struct Preview {
    pub key: String,
    pub name: String,
}

#[derive(Default)]
pub struct Attachments {
    /// Each tab's attachments, until its draft is sent.
    pub pending: HashMap<u64, Vec<Attached>>,
    /// Decoded images by key (`None`: unreadable). Rune decodes on its own worker.
    images: RefCell<HashMap<String, Option<rune::Image>>>,
    /// Paths a picker chose off the main thread, for `poll` to attach.
    pub picked: Arc<Mutex<Vec<String>>>,
    pub preview: Option<Preview>,
}

/// The type of a file from its name (`mime_for_path`).
pub fn mime_for(name: &str) -> &'static str {
    match name.rsplit('.').next().map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("png") => "image/png",
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("pdf") => "application/pdf",
        Some("txt" | "md" | "rs" | "ts" | "tsx" | "js" | "json" | "toml" | "py") => "text/plain",
        _ => "application/octet-stream",
    }
}

/// `file_attachment_token`.
pub fn token_for(name: &str, mime: &str, index: usize) -> String {
    if mime.starts_with("image/") { format!("[image{index}]") }
    else if mime == "application/pdf" { format!("[pdf{index}]") }
    else { format!("[file{index}: {name}]") }
}

/// A user message's text without its `[image…]` anchors (`user_message_display_text`).
pub fn display_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find('[') {
        out.push_str(&rest[..start]);
        let after = &rest[start..];
        let Some(end) = after.find(']') else { out.push_str(after); return tidy(&out) };
        let label = after[1..end].trim().to_ascii_lowercase();
        let anchor = label.strip_prefix("image").is_some_and(|s| s.is_empty() || s.starts_with(':') || s.starts_with(|c: char| c.is_ascii_digit()));
        if !anchor { out.push_str(&after[..=end]); }
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    tidy(&out)
}

fn tidy(text: &str) -> String {
    text.split('\n').map(|line| line.split(' ').filter(|w| !w.is_empty()).collect::<Vec<_>>().join(" ")).collect::<Vec<_>>().join("\n").trim().to_owned()
}

/// The bytes a history attachment carries: a `data:` URL's payload, or a local file's.
fn history_bytes(url: &str) -> Option<Vec<u8>> {
    if let Some(rest) = url.strip_prefix("data:") {
        let (_, data) = rest.split_once(";base64,")?;
        return base64::engine::general_purpose::STANDARD.decode(data.trim()).ok();
    }
    let path = url.strip_prefix("file://").unwrap_or(url);
    if path.contains("://") { return None; }
    let bytes = std::fs::read(path.replace("%20", " ")).ok()?;
    (bytes.len() <= MAX_BYTES).then_some(bytes)
}

impl App {
    /// The paperclip: choose images to attach. A worker runs the desktop's picker so the window
    /// keeps drawing; `poll` attaches what it returns.
    pub fn request_attach(&mut self) {
        let (picked, wake, dir) = (self.attach.picked.clone(), self.waker.clone(), self.cwd.clone());
        std::thread::spawn(move || {
            let output = std::process::Command::new("zenity")
                .args(["--file-selection", "--multiple", "--separator=\n", "--title=Attach files"])
                .arg(format!("--filename={}/", dir.trim_end_matches('/')))
                .arg("--file-filter=Images | *.png *.jpg *.jpeg *.gif *.webp *.bmp *.PNG *.JPG *.JPEG")
                .arg("--file-filter=All files | *")
                .output();
            let Ok(output) = output else { return };
            if !output.status.success() { return; }
            let paths: Vec<String> = String::from_utf8_lossy(&output.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
            if paths.is_empty() { return; }
            picked.lock().unwrap().extend(paths);
            if let Some(wake) = wake { wake(); }
        });
    }

    /// Attach files to the active composer (the picker's, a browser's or a drop's choice).
    /// Unreadable, empty or oversized files are skipped. Returns whether any attached.
    pub fn attach_images(&mut self, paths: Vec<String>) -> bool {
        let mut any = false;
        for path in paths {
            let Ok(bytes) = std::fs::read(&path) else { continue };
            let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| "file".into());
            let mime = mime_for(&name);
            any |= self.attach_bytes(&name, mime, bytes);
        }
        any
    }

    /// `attach_file_bytes`: add one attachment to the active tab's draft.
    pub fn attach_bytes(&mut self, name: &str, mime: &str, bytes: Vec<u8>) -> bool {
        if bytes.is_empty() || bytes.len() > MAX_BYTES { return false; }
        let index = self.active;
        if !matches!(self.tabs.get(index).map(|t| &t.kind), Some(TabKind::Agent { .. })) { return false; }
        let tab = self.tabs[index].id;
        let list = self.attach.pending.entry(tab).or_default();
        let mut n = list.len() + 1;
        let token = loop {
            let token = token_for(name, mime, n);
            if !list.iter().any(|a| a.token == token) && !self.tabs[index].draft.text.contains(&token) { break token; }
            n += 1;
        };
        list.push(Attached { token: token.clone(), name: name.to_owned(), mime: mime.to_owned(), bytes: bytes.into() });
        // The token joins the draft (`[current.trimEnd(), token].join(" ") + " "`).
        let draft = &mut self.tabs[index].draft;
        let text = [draft.text.trim_end(), token.as_str()].iter().filter(|s| !s.is_empty()).copied().collect::<Vec<_>>().join(" ") + " ";
        draft.replace_text(text);
        true
    }

    /// A picker's paths waiting from its worker. Returns whether any attached.
    pub fn take_picked_attachments(&mut self) -> bool {
        let paths: Vec<String> = std::mem::take(&mut *self.attach.picked.lock().unwrap());
        !paths.is_empty() && self.attach_images(paths)
    }

    /// The × on a chip: drop the attachment and its token.
    pub fn remove_attachment(&mut self, tab: u64, token: &str) {
        if let Some(list) = self.attach.pending.get_mut(&tab) { list.retain(|a| a.token != token); }
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
            let text = t.draft.text.replace(&format!("{token} "), "").replace(token, "");
            t.draft.replace_text(text);
        }
    }

    /// A pasted image (Ctrl+V or Super+V with an image on the clipboard) attaches as
    /// `clipboard-image-N.png`. Returns whether it did.
    pub fn paste_image(&mut self, content: &rune::ClipboardContent) -> bool {
        let Some(image) = content.image() else { return false };
        let n = self.tabs.get(self.active).and_then(|t| self.attach.pending.get(&t.id)).map_or(0, Vec::len) + 1;
        self.attach_bytes(&format!("clipboard-image-{n}.png"), "image/png", image.png().to_vec())
    }

    /// Take the attachments a tab's draft carries for its prompt: the wire form, and the
    /// transcript's echo.
    pub fn take_attachments(&mut self, session_id: &str) -> (Vec<Attachment>, Vec<HistoryAttachment>) {
        let tab = self.tabs.iter().find(|t| matches!(&t.kind, TabKind::Agent { session: Some(id) } if id == session_id)).map(|t| t.id);
        let list = tab.and_then(|tab| self.attach.pending.remove(&tab)).unwrap_or_default();
        let wire = list.iter().map(|a| Attachment { kind: a.mime.clone(), path: Some(a.name.clone()), bytes: a.bytes.to_vec() }).collect();
        let echo = list.iter().map(|a| HistoryAttachment {
            filename: a.name.clone(),
            url: format!("data:{};base64,{}", a.mime, base64::engine::general_purpose::STANDARD.encode(&a.bytes)),
            mime: a.mime.clone(),
        }).collect();
        // The echo's images are already decoded under the draft's keys.
        (wire, echo)
    }

    /// The decoded image under `key`, loading it from `bytes` the first time it's asked for.
    pub fn attachment_image(&self, key: &str, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<rune::Image> {
        if let Some(image) = self.attach.images.borrow().get(key) { return image.clone(); }
        let images = self.images.as_ref()?;
        let image = bytes().map(|b| images.load(b));
        self.attach.images.borrow_mut().insert(key.to_owned(), image.clone());
        image
    }

    fn draft_image(&self, a: &Attached) -> Option<rune::Image> {
        let bytes = a.bytes.clone();
        self.attachment_image(&a.key(), move || Some(bytes.to_vec()))
    }

    /// A history attachment's image, keyed by its message and place.
    pub fn history_image(&self, message_id: &str, index: usize, a: &HistoryAttachment) -> Option<rune::Image> {
        let url = a.url.clone();
        self.attachment_image(&format!("history:{message_id}:{index}"), move || history_bytes(&url))
    }

    /// Esc closes the preview. Returns whether the key was taken.
    pub fn image_preview_key(&mut self, event: &KeyEvent) -> bool {
        if self.attach.preview.is_none() { return false; }
        if event.key == rune::Key::Escape { self.attach.preview = None; }
        // The preview is modal: nothing behind it takes keys.
        true
    }
}

/// A thumbnail: a bordered rounded square holding the whole image, fitted (`render_image_strip`).
fn thumb(app: &App, image: Option<rune::Image>, size: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let inner = el().w(size).h(size).radius(11.0 * s).fill(rgb(p.bg)).clip_rounded(11.0 * s);
    let inner = match image {
        Some(image) => inner.child(el().w(size).h(size).pixels_fit(image.pixels(), rune::SurfaceFit::Fit)),
        None => inner.center().child(text("Image unavailable").font(app.ty.sized(10.0)).color(rgb(p.muted)).wrap()),
    };
    el().padding(2.0 * s).radius(13.0 * s).fill(rgb(p.border).alpha(0.9)).child(inner)
}

/// The composer's attachment strip: a 64 pt thumbnail (or the file's name) and a × per
/// attachment, scrolling sideways when they overflow.
pub fn composer_strip(app: &App, tab: u64) -> Option<El> {
    let list = app.attach.pending.get(&tab).filter(|l| !l.is_empty())?;
    let p = app.pal;
    let s = app.ty.scale();
    let mut strip = row().key("attachment-strip").gap(6.0 * s).center_y();
    for a in list {
        let token = a.token.clone();
        let body = if a.is_image() {
            let (key, name) = (a.key(), a.name.clone());
            thumb(app, app.draft_image(a), COMPOSER_THUMB * s)
                .on_press(move |s: &mut App| s.attach.preview = Some(Preview { key: key.clone(), name: name.clone() }))
        } else {
            text(a.name.clone()).font(app.ty.sized(11.0)).color(rgb(p.fg)).truncate().max_w(160.0 * s)
        };
        strip = strip.child(row().key(format!("attachment:{}", a.token)).gap(6.0 * s).padding(5.0 * s).radius(5.0 * s).fill(rgb(p.surface)).center_y()
            .child(body)
            .child(el().key("remove").w(18.0 * s).h(18.0 * s).center()
                .child(text("\u{d7}").font(app.ty.sized(13.0)).color(rgb(p.muted)))
                .on_press(move |s: &mut App| s.remove_attachment(tab, &token))));
    }
    // Hugging its chips at the card's left, never stretched across it.
    Some(row().w_full().justify_start().items_start().child(strip))
}

/// A user message's images: 150 pt thumbnails, right-aligned in the bubble, each opening the
/// preview.
pub fn message_strip(app: &App, message_id: &str, attachments: &[HistoryAttachment], max_w: f32) -> Option<El> {
    let s = app.ty.scale();
    let images: Vec<(usize, &HistoryAttachment)> = attachments.iter().enumerate().filter(|(_, a)| a.mime.starts_with("image/")).collect();
    if images.is_empty() { return None; }
    let size = (MESSAGE_THUMB * s).min(max_w);
    let shown = images.len().min((((max_w + GAP * s) / (size + 4.0 * s + GAP * s)).floor() as usize).max(1));
    let mut strip = row().gap(GAP * s).justify_end();
    for (index, a) in images.into_iter().take(shown) {
        let key = format!("history:{message_id}:{index}");
        let name = a.filename.clone();
        strip = strip.child(thumb(app, app.history_image(message_id, index, a), size)
            .on_press(move |s: &mut App| s.attach.preview = Some(Preview { key: key.clone(), name: name.clone() })));
    }
    Some(strip)
}

/// The full-window preview (`ImagePreviewModal`): a dark backdrop, the image fitted 32 pt in
/// from the sides, 64 pt from the top and 32 from the bottom, its name, and a ×.
pub fn preview(app: &App) -> El {
    let Some(preview) = &app.attach.preview else { return el() };
    let p = app.pal;
    let [w, h] = app.window;
    let image = app.attach.images.borrow().get(&preview.key).cloned().flatten();
    let body = match image {
        Some(image) => el().w((w - 64.0).max(1.0)).h((h - 96.0).max(1.0)).absolute(32.0, 64.0)
            .pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        None => text("Image unavailable").font(app.ty.sized(14.0)).color(rgb(p.fg)).absolute(32.0, 64.0),
    };
    col().key("image-preview").w(w).h(h).absolute(0.0, 0.0).fill(rgb(0x000000).alpha(0xdd as f32 / 255.0))
        .on_press(|s: &mut App| s.attach.preview = None)
        .on_wheel(|_: &mut App, _: rune::WheelEvent, _: &mut Tx| true)
        .child(body)
        .child(text(preview.name.clone()).font(app.ty.sized(14.0)).color(rgb(p.fg)).truncate().max_w((w - 112.0).max(1.0)).absolute(32.0, 20.0))
        .child(el().key("image-preview-close").w(40.0).h(40.0).absolute(w - 56.0, 12.0).radius(6.0).fill(rgb(p.surface)).center()
            .child(text("\u{d7}").font(app.ty.sized(24.0)).color(rgb(p.fg)))
            .on_press(|s: &mut App| s.attach.preview = None))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokens_follow_the_policy() {
        assert_eq!(token_for("a.png", "image/png", 1), "[image1]");
        assert_eq!(token_for("a.pdf", "application/pdf", 2), "[pdf2]");
        assert_eq!(token_for("notes.txt", "text/plain", 3), "[file3: notes.txt]");
        assert_eq!(mime_for("Shot.JPG"), "image/jpeg");
    }

    #[test]
    fn display_text_drops_image_anchors_only() {
        assert_eq!(display_text("look [image1] at this"), "look at this");
        assert_eq!(display_text("[image1] [image2]"), "");
        assert_eq!(display_text("keep [pdf1] and [link]"), "keep [pdf1] and [link]");
        assert_eq!(display_text("a [image: shot.png] b"), "a b");
    }

    #[test]
    fn data_urls_decode() {
        assert_eq!(history_bytes("data:image/png;base64,AQID"), Some(vec![1, 2, 3]));
        assert_eq!(history_bytes("https://example.com/x.png"), None);
    }
}
