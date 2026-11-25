//! Image (and file) attachments on the agent composer (`attachment_policy.rs`,
//! `state/input_edit.rs::attach_file_bytes`, GPUiX `App.tsx`'s attachment strip and
//! `ui/AttachmentImage.tsx`):
//!
//! - The paperclip, a pasted image or files dropped on the card attach to the active tab's
//!   draft: each adds an `[imageN]` / `[pdfN]` / `[fileN: name]` token to the text and a chip
//!   in a strip of 64 pt thumbnails at the top of the composer card, removed with its ×.
//! - Submitting sends them with the prompt (`Attachment { kind: mime, path: name, bytes }`),
//!   up to 20 MiB combined (subject to the wire budget), and echoes them as `data:` URLs.
//! - Message photos sit outside the text bubble, stacked when there are several. Horizontal
//!   swipes cycle the stack; clicking opens the set in the full-window preview.

use crate::app::App;
use crate::tabs::TabKind;
use crate::ui::theme::rgb;
use base64::Engine as _;
use neoism_protocol::agent::{Attachment, HistoryAttachment, MOBILE_PHOTO_MAX_BYTES, MOBILE_PHOTO_MAX_COUNT, mobile_photo_filename, mobile_photo_mime};
use rune::{El, KeyEvent, Tx, col, el, row, text};
use std::cell::RefCell;
use std::collections::HashMap;
use std::io::Read as _;
use std::sync::Arc;

/// `MAX_INLINE_ATTACHMENT_BYTES`.
pub const MAX_BYTES: usize = 20 * 1024 * 1024;
// Leave room in the daemon's 64 MiB WebSocket message for text and envelope fields.
const MAX_WIRE_BYTES: usize = 60 * 1024 * 1024;
/// `COMPOSER_THUMB_SIZE` and `MESSAGE_THUMB_SIZE`.
pub const COMPOSER_THUMB: f32 = 64.0;
pub const MESSAGE_THUMB: f32 = 260.0;

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
    fn key(&self, tab: u64) -> String { format!("draft:{tab}:{}", self.token) }
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
    inline_pending: RefCell<Vec<rune::Image>>,
    inline_version: std::cell::Cell<usize>,
    pub preview: Option<Preview>,
    preview_gallery: Vec<Preview>,
    stacks: HashMap<String, usize>,
    swipe: HashMap<String, Swipe>,
}

#[derive(Default)]
struct Swipe {
    distance: f32,
    last: Option<std::time::Instant>,
    stepped: Option<std::time::Instant>,
}

impl Swipe {
    fn step(&mut self, delta: f32, now: std::time::Instant) -> isize {
        if self.last.is_none_or(|last| now.duration_since(last).as_millis() > 160) { self.distance = 0.0; }
        self.last = Some(now);
        if self.stepped.is_some_and(|last| now.duration_since(last).as_millis() < 240) { return 0; }
        self.distance += delta;
        if self.distance.abs() < 48.0 { return 0; }
        let step = if self.distance > 0.0 { 1 } else { -1 };
        self.distance = 0.0;
        self.stepped = Some(now);
        step
    }
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
    pub(crate) fn preview_rendered_diagram(&mut self, image: rune::Image) {
        let key = "mermaid-preview".to_owned();
        self.attach.images.borrow_mut().insert(key.clone(), Some(image));
        self.attach.preview_gallery.clear();
        self.attach.preview = Some(Preview { key, name: "Mermaid diagram".into() });
    }

    /// A decode changes Markdown's placeholder into an image, including inside memoized rows.
    pub fn inline_images_changed(&self) -> bool {
        let mut pending = self.attach.inline_pending.borrow_mut();
        let before = pending.len();
        pending.retain(|image| !image.is_ready());
        let ready = before - pending.len();
        if ready == 0 { return false; }
        self.attach.inline_version.set(self.attach.inline_version.get().wrapping_add(ready));
        true
    }

    pub fn inline_images_version(&self) -> usize {
        self.attach.inline_version.get()
    }

    pub fn markdown_images(&self, source: &str) -> HashMap<String, rune::Image> {
        let mut result = HashMap::new();
        for event in pulldown_cmark::Parser::new(source) {
            let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { dest_url, .. }) = event else { continue };
            let src = dest_url.to_string();
            if let Some(image) = self.markdown_image_at(&src, &self.cwd) { result.insert(src, image); }
        }
        result
    }

    /// Shared note-body/cover loader. `base_dir` is the note's directory, never the
    /// process cwd. Rune performs file reads and image decoding on its worker;
    /// completion uses the same poll/version lifecycle as agent inline images.
    /// Unsupported network schemes return None (no implicit network requests).
    pub fn markdown_image_at(&self, src: &str, base_dir: &str) -> Option<rune::Image> {
        let cache = self.images.as_ref()?;
        let path = inline_path(src, base_dir).or_else(|| src.starts_with("data:image/").then(|| src.to_owned()))?;
        let key = format!("inline:{path}");
        if let Some(image) = self.attach.images.borrow().get(&key) { return image.clone(); }
        let image = if path.starts_with("data:") { history_bytes(&path).map(|bytes| cache.load(bytes)) }
            else { Some(cache.file(&path, 1024.0, 2.0)) };
        if let Some(image) = &image { self.attach.inline_pending.borrow_mut().push(image.clone()); }
        self.attach.images.borrow_mut().insert(key, image.clone());
        image
    }

    pub(crate) fn viewer_file_image(&self, tab: u64, path: &str) -> Option<rune::Image> {
        let cache = self.images.as_ref()?;
        let key = format!("viewer:{tab}");
        if let Some(image) = self.attach.images.borrow().get(&key) { return image.clone(); }
        let image = cache.file(std::path::Path::new(&self.cwd).join(path), 1024.0, 2.0);
        self.attach.inline_pending.borrow_mut().push(image.clone());
        self.attach.images.borrow_mut().insert(key, Some(image.clone()));
        Some(image)
    }

    pub(crate) fn forget_image_viewer(&mut self, tab: u64) {
        self.attach.images.get_mut().remove(&format!("viewer:{tab}"));
    }

    pub fn preview_inline_image(&mut self, src: &str, name: &str) {
        let Some(path) = inline_path(src, &self.cwd).or_else(|| src.starts_with("data:image/").then(|| src.to_owned())) else { return };
        self.attach.preview_gallery.clear();
        self.attach.preview = Some(Preview { key: format!("inline:{path}"), name: name.to_owned() });
    }

    /// The paperclip opens the file browser in picture mode.
    pub fn request_attach(&mut self) {
        self.open_file_browser(crate::browser::Mode::AttachImage);
    }

    /// Attach files from the browser or a drop, reporting rejected files together.
    pub fn attach_images(&mut self, paths: &[std::path::PathBuf]) {
        let mut errors = Vec::new();
        for path in paths {
            let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy();
            let mime = mime_for(&name);
            let result = (|| {
                if path.extension().and_then(|e| e.to_str()).is_some_and(|ext| {
                    matches!(ext.to_ascii_lowercase().as_str(), "heic" | "heif" | "avif" | "tif" | "tiff" | "svg")
                }) { return Err("Unsupported picture format. Export it as PNG or JPEG."); }
                let metadata = path.metadata().map_err(|_| "Could not find or read the file.")?;
                if !metadata.is_file() { return Err("Not a regular file."); }
                if metadata.len() > MAX_BYTES as u64 { return Err("File exceeds the 20 MiB attachment limit."); }
                let mut bytes = Vec::new();
                std::fs::File::open(path).and_then(|file| file.take(MAX_BYTES as u64 + 1).read_to_end(&mut bytes))
                    .map_err(|_| "Could not read the file.")?;
                self.add_attachment(&name, mime, bytes)
            })();
            if let Err(reason) = result { errors.push(format!("{name}: {reason}")); }
        }
        if !errors.is_empty() {
            self.modal = Some(crate::ui::modal::Modal::notice("Could not attach files", &errors.join("\n")));
        }
    }

    /// `attach_file_bytes`: add one attachment to the active tab's draft.
    pub fn attach_bytes(&mut self, name: &str, mime: &str, bytes: Vec<u8>) -> bool {
        if let Err(reason) = self.add_attachment(name, mime, bytes) {
            self.modal = Some(crate::ui::modal::Modal::notice("Could not attach file", &format!("{name}: {reason}")));
            return false;
        }
        true
    }

    fn add_attachment(&mut self, name: &str, mime: &str, bytes: Vec<u8>) -> Result<(), &'static str> {
        if bytes.is_empty() { return Err("The file is empty."); }
        if bytes.len() > MAX_BYTES { return Err("File exceeds the 20 MiB attachment limit."); }
        let index = self.active;
        if !matches!(self.tabs.get(index).map(|t| &t.kind), Some(TabKind::Agent { .. })) { return Err("Open an agent chat first."); }
        if self.paired_chat_device() {
            // The first prompt still owns this tab's attachment list until ThreadCreated.
            if self.tabs[index].pending_prompt.is_some() { return Err("Wait for the new chat to open before attaching another photo."); }
            Self::mobile_photo_type(name, mime)?;
        }
        self.attachment_budget(index, Some(&bytes))?;
        let tab = self.tabs[index].id;
        let list = self.attach.pending.entry(tab).or_default();
        let mut n = list.len() + 1;
        let token = loop {
            let token = token_for(name, mime, n);
            if !list.iter().any(|a| a.token == token) && !self.tabs[index].draft.text.contains(&token) { break token; }
            n += 1;
        };
        list.push(Attached { token: token.clone(), name: name.to_owned(), mime: mime.to_owned(), bytes: bytes.into() });
        let draft = &mut self.tabs[index].draft;
        draft.insert_pill(format!("attachment:{token}"), &token);
        draft.paste(" ");
        Ok(())
    }

    fn mobile_photo_type(name: &str, mime: &str) -> Result<(), &'static str> {
        if mobile_photo_mime(mime) && mobile_photo_filename(name) { Ok(()) }
        else { Err("Paired mobile chat accepts only photos with a valid filename.") }
    }

    fn attachment_budget(&self, index: usize, additional: Option<&[u8]>) -> Result<(), &'static str> {
        let tab = &self.tabs[index];
        let live = self.attach.pending.get(&tab.id).into_iter().flatten().filter(|a| {
            tab.draft.pills().any(|(_, id)| id.strip_prefix("attachment:") == Some(a.token.as_str()))
        });
        let size = live.clone().map(|a| a.bytes.len()).sum::<usize>() + additional.map_or(0, |bytes| bytes.len());
        if self.paired_chat_device() {
            if size > MOBILE_PHOTO_MAX_BYTES || live.clone().count() + usize::from(additional.is_some()) > MOBILE_PHOTO_MAX_COUNT {
                return Err("Paired mobile chat allows up to four photos and 4 MiB total. Remove or resize a photo.");
            }
            for a in live { Self::mobile_photo_type(&a.name, &a.mime)?; }
            return Ok(());
        }
        if size > MAX_BYTES { return Err("Attachments can total at most 20 MiB per message. Remove or resize a file."); }
        // The current wire protocol encodes each byte as up to three digits plus a comma.
        if size > MAX_WIRE_BYTES / 4 && live.flat_map(|a| a.bytes.iter()).chain(additional.into_iter().flatten())
            .map(|&b| 2 + usize::from(b >= 10) + usize::from(b >= 100)).sum::<usize>() > MAX_WIRE_BYTES
        { return Err("These attachments are too large to send together. Remove or resize a file."); }
        Ok(())
    }

    pub(crate) fn check_attachment_budget(&mut self, index: usize) -> bool {
        if let Err(reason) = self.attachment_budget(index, None) {
            self.modal = Some(crate::ui::modal::Modal::notice("Could not send attachments", reason));
            return false;
        }
        true
    }

    pub(crate) fn clear_draft_attachment_images(&mut self, tab: u64) {
        let prefix = format!("draft:{tab}:");
        self.attach.images.get_mut().retain(|key, _| !key.starts_with(&prefix));
        if self.attach.preview.as_ref().is_some_and(|p| p.key.starts_with(&prefix)) { self.attach.preview = None; }
    }

    /// The × on a chip: drop the attachment and its token.
    pub fn remove_attachment(&mut self, tab: u64, token: &str) {
        if let Some(t) = self.tabs.iter_mut().find(|t| t.id == tab) {
            let range = t.draft.pills().find(|(_, id)| id.strip_prefix("attachment:") == Some(token)).map(|(r, _)| r);
            if let Some(range) = range {
                t.draft.anchor = range.start;
                t.draft.focus = range.end;
                t.draft.paste("");
            }
        }
    }

    /// A pasted image (Ctrl+V or Super+V with an image on the clipboard) attaches as
    /// `clipboard-image-N.png`. Consumes image pastes even when rejected.
    pub fn paste_image(&mut self, content: &rune::ClipboardContent) -> bool {
        let Some(image) = content.image() else { return false };
        let n = self.tabs.get(self.active).and_then(|t| self.attach.pending.get(&t.id)).map_or(0, Vec::len) + 1;
        self.attach_bytes(&format!("clipboard-image-{n}.png"), "image/png", image.png().to_vec());
        true
    }

    /// Take the attachments a tab's draft carries for its prompt: the wire form, and the
    /// transcript's echo.
    pub fn take_attachments(&mut self, session_id: &str) -> (Vec<Attachment>, Vec<HistoryAttachment>) {
        let tab = self.tabs.iter().find(|t| matches!(&t.kind, TabKind::Agent { session: Some(id) } if id == session_id)).map(|t| t.id);
        let list = tab.and_then(|tab| {
            self.clear_draft_attachment_images(tab);
            self.attach.pending.remove(&tab)
        }).unwrap_or_default();
        let wire = list.iter().map(|a| Attachment { kind: a.mime.clone(), path: Some(a.name.clone()), bytes: a.bytes.to_vec() }).collect();
        let echo = list.iter().map(|a| HistoryAttachment {
            filename: a.name.clone(),
            url: format!("data:{};base64,{}", a.mime, base64::engine::general_purpose::STANDARD.encode(&a.bytes)),
            mime: a.mime.clone(),
        }).collect();
        (wire, echo)
    }

    /// The decoded image under `key`, loading it from `bytes` the first time it's asked for.
    pub fn attachment_image(&self, key: &str, bytes: impl FnOnce() -> Option<Vec<u8>>) -> Option<rune::Image> {
        if let Some(image) = self.attach.images.borrow().get(key) { return image.clone(); }
        let images = self.images.as_ref()?;
        let image = bytes().map(|b| images.load(b));
        if let Some(image) = &image { self.attach.inline_pending.borrow_mut().push(image.clone()); }
        self.attach.images.borrow_mut().insert(key.to_owned(), image.clone());
        image
    }

    fn draft_image(&self, tab: u64, a: &Attached) -> Option<rune::Image> {
        let bytes = a.bytes.clone();
        self.attachment_image(&a.key(tab), move || Some(bytes.to_vec()))
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
        if event.key == rune::Key::Right { self.step_image_preview(1); }
        if event.key == rune::Key::Left { self.step_image_preview(-1); }
        // The preview is modal: nothing behind it takes keys.
        true
    }

    fn step_image_preview(&mut self, step: isize) {
        let Some(current) = &self.attach.preview else { return };
        let gallery = &self.attach.preview_gallery;
        let Some(index) = gallery.iter().position(|p| p.key == current.key) else { return };
        self.attach.preview = Some(gallery[(index as isize + step).rem_euclid(gallery.len() as isize) as usize].clone());
    }
}

/// A thumbnail: a bordered rounded square holding the whole image, fitted (`render_image_strip`).
fn thumb(app: &App, image: Option<rune::Image>, size: f32) -> El {
    let p = app.pal;
    let s = app.ty.scale();
    let inner = el().w(size).h(size).radius(11.0 * s).fill(rgb(p.bg)).clip_rounded(11.0 * s);
    let inner = match image {
        Some(image) if image.error().is_none() => inner.child(el().w(size).h(size).pixels_fit(image.pixels(), rune::SurfaceFit::Fit)),
        _ => inner.center().child(text("Image unavailable").font(app.ty.sized(10.0)).color(rgb(p.muted)).wrap()),
    };
    el().padding(2.0 * s).radius(13.0 * s).fill(rgb(p.border).alpha(0.9)).child(inner)
}

fn inline_path(src: &str, cwd: &str) -> Option<String> {
    // Normalize the base before URL joining: otherwise a trailing `..` is
    // treated as a directory name when a relative image also climbs a level.
    use std::path::{Component, Path, PathBuf};
    let mut base = PathBuf::new();
    for part in Path::new(cwd).components() {
        match part {
            Component::CurDir => {},
            Component::ParentDir if base.file_name().is_some_and(|n| n != "..") => { base.pop(); },
            Component::ParentDir if !base.has_root() => base.push(".."),
            Component::ParentDir => {},
            _ => base.push(part.as_os_str()),
        }
    }
    let path = if src.starts_with("file:") {
        url::Url::parse(src).ok()?.to_file_path().ok()?
    } else {
        // Web URLs and other schemes must never become local filesystem paths.
        if src.contains(':') && !std::path::Path::new(src).is_absolute() { return None; }
        // Markdown destinations may percent-encode spaces/non-ASCII filenames.
        // URL joining is lexical and performs no filesystem or network work.
        if let Ok(url) = url::Url::from_directory_path(&base) {
            url.join(src).ok()?.to_file_path().ok()?
        } else { std::path::PathBuf::from(src) }
    };
    Some(if path.is_absolute() { path } else { base.join(path) }.to_string_lossy().into_owned())
}

/// The composer's attachment strip: a 64 pt thumbnail (or the file's name) and a × per
/// attachment, scrolling sideways when they overflow.
pub fn composer_strip(app: &App, tab: u64) -> Option<El> {
    let list = app.attach.pending.get(&tab).filter(|l| !l.is_empty())?;
    let draft = &app.tabs.iter().find(|t| t.id == tab)?.draft;
    let list: Vec<_> = list.iter().filter(|a| draft.pills().any(|(_, id)| id.strip_prefix("attachment:") == Some(a.token.as_str()))).collect();
    if list.is_empty() { return None; }
    let p = app.pal;
    let s = app.ty.scale();
    let mut strip = row().key("attachment-strip").gap(6.0 * s).center_y();
    for a in list {
        let token = a.token.clone();
        let body = if a.is_image() {
            let (key, name) = (a.key(tab), a.name.clone());
            thumb(app, app.draft_image(tab, a), COMPOSER_THUMB * s)
                .on_press(move |s: &mut App| {
                    s.attach.preview_gallery.clear();
                    s.attach.preview = Some(Preview { key: key.clone(), name: name.clone() });
                })
        } else {
            text(a.name.clone()).font(app.ty.sized(11.0)).color(rgb(p.fg)).truncate().max_w(160.0 * s)
        };
        strip = strip.child(row().key(format!("attachment:{}", a.token)).gap(6.0 * s).padding(5.0 * s).radius(5.0 * s).fill(rgb(p.blue).alpha(0.2)).border(s, rgb(p.blue).alpha(0.4)).center_y()
            .child(body)
            .child(el().key("remove").w(18.0 * s).h(18.0 * s).center()
                .child(text("\u{d7}").font(app.ty.sized(13.0)).color(rgb(p.muted)))
                .on_press(move |s: &mut App| s.remove_attachment(tab, &token))));
    }
    // Hugging its chips at the card's left, never stretched across it.
    Some(row().w_full().justify_start().items_start().child(strip))
}

/// A standalone photo or a fanned, swipeable stack. Every image remains reachable.
pub fn message_strip(app: &App, message_id: &str, attachments: &[HistoryAttachment], max_w: f32) -> Option<El> {
    let s = app.ty.scale();
    let images: Vec<(usize, &HistoryAttachment)> = attachments.iter().enumerate().filter(|(_, a)| a.mime.starts_with("image/")).collect();
    if images.is_empty() { return None; }
    let count = images.len();
    let selected = app.attach.stacks.get(message_id).copied().unwrap_or(0) % count;
    let size = (MESSAGE_THUMB * s).min((max_w - 28.0 * s).max(1.0));
    let gallery: Vec<_> = images.iter().map(|(index, a)| Preview {
        key: format!("history:{message_id}:{index}"), name: a.filename.clone(),
    }).collect();
    // Decode all members once so cycling and opening the gallery never produces missing keys.
    let decoded: Vec<_> = images.iter().map(|(index, a)| app.history_image(message_id, *index, a)).collect();
    let height = if count == 1 {
        decoded[0].as_ref().and_then(|image| image.size()).map(|[w, h]| (size * h as f32 / w.max(1) as f32).clamp(size * 0.5, size * 1.3)).unwrap_or(size)
    } else { size };
    let mut stack = el().key(format!("photo-stack:{message_id}")).w(size + 28.0 * s).h(height + 28.0 * s);
    for depth in (0..count.min(3)).rev() {
        let index = (selected + depth) % count;
        let image = el().w(size).h(height).radius(14.0 * s).clip_rounded(14.0 * s).fill(rgb(app.pal.surface));
        let image = match &decoded[index] {
            Some(handle) => image.child(el().w(size).h(height).pixels_fit(handle.pixels(), rune::SurfaceFit::Fill)),
            None => image.center().child(text("Image unavailable").font(app.ty.sized(11.0)).color(rgb(app.pal.muted))),
        };
        stack = stack.child(image.rotate(if depth == 0 { 0.0 } else if depth == 1 { -0.075 } else { 0.09 })
            .absolute((14.0 + depth as f32 * 2.0) * s, (14.0 - depth as f32 * 3.0) * s));
    }
    let id = message_id.to_owned();
    let stack = stack.on_press(move |a: &mut App| {
        a.attach.preview = Some(gallery[selected].clone());
        a.attach.preview_gallery = gallery.clone();
    }).on_wheel(move |a: &mut App, e: rune::WheelEvent, _: &mut Tx| {
        if count < 2 || !(e.dx.abs() > e.dy.abs() || e.mods.shift) { return false; }
        if e.momentum { return true; }
        let delta = -if e.dx.abs() > e.dy.abs() { e.dx } else { e.dy };
        let step = a.attach.swipe.entry(id.clone()).or_default().step(if e.precise { delta } else { delta * 48.0 }, std::time::Instant::now());
        if step != 0 {
            let index = a.attach.stacks.entry(id.clone()).or_default();
            *index = (*index as isize + step).rem_euclid(count as isize) as usize;
        }
        true
    });
    let mut photos = col().items_end().child(stack);
    if count > 1 {
        photos = photos.child(text(format!("{} / {}", selected + 1, count)).font(app.ty.sized(10.0)).color(rgb(app.pal.muted)));
    }
    Some(photos)
}

/// The full-window preview (`ImagePreviewModal`): a dark backdrop, the image fitted 32 pt in
/// from the sides, 64 pt from the top and 32 from the bottom, its name, and a ×.
pub fn preview(app: &App, [top, right, bottom, left]: [f32; 4]) -> El {
    let Some(preview) = &app.attach.preview else { return el() };
    let p = app.pal;
    let [w, h] = app.window;
    let safe_w = (w - left - right).max(0.0);
    let safe_h = (h - top - bottom).max(0.0);
    let image = app.attach.images.borrow().get(&preview.key).cloned().flatten();
    let body = match image {
        Some(image) => el().w((safe_w - 64.0).max(1.0)).h((safe_h - 96.0).max(1.0)).absolute(left + 32.0, top + 64.0)
            .pixels_fit(image.pixels(), rune::SurfaceFit::Fit),
        None => text("Image unavailable").font(app.ty.sized(14.0)).color(rgb(p.fg)).absolute(left + 32.0, top + 64.0),
    };
    let gallery_index = app.attach.preview_gallery.iter().position(|p| p.key == preview.key);
    let count = app.attach.preview_gallery.len();
    let mut modal = col().key("image-preview").w(w).h(h).absolute(0.0, 0.0).fill(rgb(0x000000).alpha(0xdd as f32 / 255.0))
        .on_press(|s: &mut App| s.attach.preview = None)
        .on_wheel(|a: &mut App, e: rune::WheelEvent, _: &mut Tx| {
            if e.dx.abs() > e.dy.abs() || e.mods.shift {
                if e.momentum { return true; }
                let delta = -if e.dx.abs() > e.dy.abs() { e.dx } else { e.dy };
                let step = a.attach.swipe.entry("image-preview".into()).or_default().step(if e.precise { delta } else { delta * 48.0 }, std::time::Instant::now());
                if step != 0 { a.step_image_preview(step); }
            }
            true
        })
        .child(body.on_press(|_: &mut App| {}))
        .child(text(preview.name.clone()).font(app.ty.sized(14.0)).color(rgb(p.fg)).truncate().max_w((safe_w - 112.0).max(1.0)).absolute(left + 32.0, top + 20.0))
        .child(el().key("image-preview-close").label("Close image preview").w(44.0).h(44.0).absolute(w - right - 56.0, top + 12.0).radius(6.0).fill(rgb(p.surface)).center()
            .child(text("\u{d7}").font(app.ty.sized(24.0)).color(rgb(p.fg)))
            .on_press(|s: &mut App| s.attach.preview = None));
    if let Some(index) = gallery_index.filter(|_| count > 1) {
        modal = modal.child(text(format!("{} / {}", index + 1, count)).font(app.ty.sized(12.0)).color(rgb(p.fg)).absolute(left + safe_w / 2.0 - 20.0, h - bottom - 26.0));
        for (label, step, x) in [("<", -1, left + 12.0), (">", 1, w - right - 52.0)] {
            modal = modal.child(el().w(40.0).h(48.0).absolute(x, top + safe_h / 2.0 - 24.0).radius(8.0).fill(rgb(p.surface)).center()
                .child(text(label).font(app.ty.sized(24.0)).color(rgb(p.fg)))
                .on_press(move |a: &mut App| a.step_image_preview(step)));
        }
    }
    modal
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paired_new_chat_keeps_photo_until_thread_created() {
        use neoism_protocol::agent::{AgentClientMessage, AgentServerMessage};

        let mut app = App::new();
        app.mobile = true;
        app.server.link.current = Some(crate::servers::Selection {
            endpoint: "wss://desk.example.ts.net/mobile-device/session".into(),
            ..Default::default()
        });
        app.connection = crate::app::Connection::Connected;
        app.workspace_ready = true;
        app.open_tab(TabKind::Agent { session: None });
        app.composer.selection.model = "mock/image-model".into();
        let tab = app.active_tab().id;
        app.cwd.clear();
        assert!(!app.attach_bytes("oversize.png", "image/png", vec![0; MOBILE_PHOTO_MAX_BYTES + 1]));
        assert!(!app.attach_bytes("notes.txt", "text/plain", vec![1]));
        assert!(app.attach_bytes("picked.png", "image/png", vec![1, 2, 3]));
        app.tabs[app.active].draft.paste("Describe this");
        let draft = app.active_tab().draft.text.clone();
        app.agent_sent.borrow_mut().clear();
        app.submit();
        assert_eq!(app.active_tab().draft.text, draft);
        assert!(app.active_tab().draft.pills().any(|(_, id)| id == "attachment:[image1]"));
        assert!(app.active_tab().pending_prompt.is_none());

        app.cwd = "/existing-project".into();
        app.submit();
        assert_eq!(app.active_tab().pending_prompt.as_deref(), Some(draft.trim()));
        assert!(app.attach.pending.contains_key(&tab));
        assert!(app.agent_sent.borrow().iter().any(|wire| wire.contains("CreateThread")));
        assert!(!app.agent_sent.borrow().iter().any(|wire| wire.contains("SubmitPrompt")));
        assert!(!app.attach_bytes("another.png", "image/png", vec![4, 5, 6]));
        app.apply_agent(serde_json::from_value::<AgentServerMessage>(serde_json::json!({
            "ThreadCreated": {"session_id": "test-session"}
        })).unwrap());
        let sent = app.agent_sent.borrow();
        let prompts: Vec<_> = sent.iter().filter_map(|wire| match serde_json::from_str(wire).unwrap() {
            AgentClientMessage::SubmitPrompt { session_id, text, attachments, .. } => Some((session_id, text, attachments)),
            _ => None,
        }).collect();
        assert_eq!(prompts, vec![("test-session".into(), draft.trim().into(), vec![Attachment {
            kind: "image/png".into(), path: Some("picked.png".into()), bytes: vec![1, 2, 3],
        }])]);
        assert!(!app.attach.pending.contains_key(&tab));
    }

    #[test]
    fn attachment_limits_report_errors_without_changing_the_draft() {
        let mut app = App::new();
        app.load_fixture("attachment-limits", "Attachments", "[]");
        assert!(app.attach_bytes("valid.png", "image/png", vec![0; MAX_BYTES]));
        let draft = app.active_tab().draft.text.clone();
        assert!(!app.attach_bytes("extra.png", "image/png", vec![0]));
        assert!(app.modal.as_ref().unwrap().body.contains("20 MiB per message"));
        assert_eq!(app.active_tab().draft.text, draft);
        let tab = app.active_tab().id;
        app.remove_attachment(tab, "[image1]");
        assert!(!app.attach_bytes("large.png", "image/png", vec![255; MAX_WIRE_BYTES / 4 + 1]));
        assert!(app.modal.as_ref().unwrap().body.contains("too large to send"));
        assert!(app.attach_bytes("small.png", "image/png", vec![0]));
        assert!(app.check_attachment_budget(app.active));
    }

    #[test]
    fn swipe_accumulates_and_does_not_race_through_the_stack() {
        let now = std::time::Instant::now();
        let mut swipe = Swipe::default();
        assert_eq!(swipe.step(20.0, now), 0);
        assert_eq!(swipe.step(30.0, now), 1);
        assert_eq!(swipe.step(100.0, now + std::time::Duration::from_millis(100)), 0);
        assert_eq!(swipe.step(-50.0, now + std::time::Duration::from_millis(400)), -1);
    }

    #[test]
    fn gallery_wraps_and_single_image_previews_do_not_reuse_the_gallery() {
        let mut app = App::new();
        app.attach.preview_gallery = (0..4).map(|i| Preview { key: i.to_string(), name: format!("{i}.png") }).collect();
        app.attach.preview = Some(app.attach.preview_gallery[0].clone());
        app.step_image_preview(-1);
        assert_eq!(app.attach.preview.as_ref().unwrap().key, "3");
        app.step_image_preview(1);
        assert_eq!(app.attach.preview.as_ref().unwrap().key, "0");
        app.preview_inline_image("photo.png", "Photo");
        app.step_image_preview(1);
        assert_eq!(app.attach.preview.as_ref().unwrap().name, "Photo");
        assert!(app.attach.preview_gallery.is_empty());
    }

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

    #[test]
    fn inline_sources_resolve_local_paths_without_treating_web_urls_as_files() {
        let root = std::env::temp_dir();
        let cwd = root.to_str().unwrap();
        let absolute = root.join("preview.png").to_string_lossy().into_owned();
        assert_eq!(inline_path(&absolute, cwd), Some(absolute.clone()));
        assert_eq!(inline_path("images/preview.png", cwd), Some(root.join("images/preview.png").to_string_lossy().into_owned()));
        let dotted = root.join("notes").join("..");
        assert_eq!(inline_path("images/preview.png", dotted.to_str().unwrap()),
            Some(root.join("images/preview.png").to_string_lossy().into_owned()));
        assert_eq!(inline_path("../preview.png", dotted.to_str().unwrap()),
            Some(root.parent().unwrap().join("preview.png").to_string_lossy().into_owned()));
        let spaced = root.join("a b.png");
        let url = url::Url::from_file_path(&spaced).unwrap();
        assert_eq!(inline_path(url.as_str(), cwd), Some(spaced.to_string_lossy().into_owned()));
        for src in ["https://example.com/a.png", "javascript:alert(1)", "data:image/png;base64,AQID"] {
            assert!(inline_path(src, cwd).is_none());
        }
    }

    #[test]
    fn inline_local_image_decodes_refreshes_and_opens_in_the_existing_viewer() {
        let mut app = App::new();
        app.images = Some(rune::ImageCache::new(Arc::new(|| {})));
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/splash/neoism-wordmark.png");
        let source = format!("![Splash preview]({path})");
        let images = app.markdown_images(&source);
        let image = &images[path];
        assert!(image.wait().is_ok());
        assert!(app.inline_images_changed());
        assert!(!app.inline_images_changed());
        app.preview_inline_image(path, "Splash preview");
        let preview = app.attach.preview.as_ref().unwrap();
        assert_eq!(preview.name, "Splash preview");
        assert!(app.attach.images.borrow()[&preview.key].as_ref().unwrap().error().is_none());
        // Code examples must not trigger file reads or image placeholders.
        assert!(app.markdown_images(&format!("`![not an image]({path})`")).is_empty());
    }
}
