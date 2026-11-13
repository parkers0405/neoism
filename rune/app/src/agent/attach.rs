//! Image (and file) attachments on the agent composer (`attachment_policy.rs`,
//! `state/input_edit.rs::attach_file_bytes`, GPUiX `App.tsx`'s attachment strip and
//! `ui/AttachmentImage.tsx`):
//!
//! - The paperclip, a pasted image or files dropped on the card attach to the active tab's
//!   draft: each adds an `[imageN]` / `[pdfN]` / `[fileN: name]` token to the text and a chip
//!   in a strip of 64 pt thumbnails at the top of the composer card, removed with its ×.
//! - Submitting sends them with the prompt (`Attachment { kind: mime, path: name, bytes }`),
//!   20 MB each at most, and echoes them into the transcript as `data:` URLs.
//! - Message photos sit outside the text bubble, stacked when there are several. Horizontal
//!   swipes cycle the stack; clicking opens the set in the full-window preview.

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
    inline_pending: RefCell<Vec<rune::Image>>,
    inline_version: std::cell::Cell<usize>,
    /// Paths a picker chose off the main thread, for `poll` to attach.
    pub picked: Arc<Mutex<Vec<String>>>,
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
        let Some(cache) = self.images.as_ref() else { return HashMap::new() };
        let mut result = HashMap::new();
        for event in pulldown_cmark::Parser::new(source) {
            let pulldown_cmark::Event::Start(pulldown_cmark::Tag::Image { dest_url, .. }) = event else { continue };
            let src = dest_url.to_string();
            let Some(path) = inline_path(&src, &self.cwd).or_else(|| src.starts_with("data:image/").then(|| src.clone())) else { continue };
            let key = format!("inline:{path}");
            let cached = self.attach.images.borrow().get(&key).cloned();
            let image = if let Some(image) = cached { image } else {
                let image = if path.starts_with("data:") { history_bytes(&path).map(|bytes| cache.load(bytes)) }
                    else { Some(cache.file(&path, 1024.0, 2.0)) };
                if let Some(image) = &image { self.attach.inline_pending.borrow_mut().push(image.clone()); }
                self.attach.images.borrow_mut().insert(key, image.clone());
                image
            };
            if let Some(image) = image { result.insert(src, image); }
        }
        result
    }

    pub fn preview_inline_image(&mut self, src: &str, name: &str) {
        let Some(path) = inline_path(src, &self.cwd).or_else(|| src.starts_with("data:image/").then(|| src.to_owned())) else { return };
        self.attach.preview_gallery.clear();
        self.attach.preview = Some(Preview { key: format!("inline:{path}"), name: name.to_owned() });
    }

    /// The paperclip: choose images to attach. A worker runs the desktop's picker so the window
    /// keeps drawing; `poll` attaches what it returns.
    /// The paperclip: GPUiX's file browser in its picture mode.
    pub fn request_attach(&mut self) {
        self.open_file_browser(crate::browser::Mode::AttachImage);
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
        let draft = &mut self.tabs[index].draft;
        draft.insert_pill(format!("attachment:{token}"), &token);
        draft.paste(" ");
        true
    }

    /// A picker's paths waiting from its worker. Returns whether any attached.
    pub fn take_picked_attachments(&mut self) -> bool {
        let paths: Vec<String> = std::mem::take(&mut *self.attach.picked.lock().unwrap());
        !paths.is_empty() && self.attach_images(paths)
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
        if let Some(image) = &image { self.attach.inline_pending.borrow_mut().push(image.clone()); }
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
        Some(image) => inner.child(el().w(size).h(size).pixels_fit(image.pixels(), rune::SurfaceFit::Fit)),
        None => inner.center().child(text("Image unavailable").font(app.ty.sized(10.0)).color(rgb(p.muted)).wrap()),
    };
    el().padding(2.0 * s).radius(13.0 * s).fill(rgb(p.border).alpha(0.9)).child(inner)
}

fn inline_path(src: &str, cwd: &str) -> Option<String> {
    let path = if src.starts_with("file:") {
        url::Url::parse(src).ok()?.to_file_path().ok()?
    } else {
        // Web URLs and other schemes must never become local filesystem paths.
        if src.contains(':') && !std::path::Path::new(src).is_absolute() { return None; }
        std::path::PathBuf::from(src)
    };
    Some(if path.is_absolute() { path } else { std::path::Path::new(cwd).join(path) }.to_string_lossy().into_owned())
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
            let (key, name) = (a.key(), a.name.clone());
            thumb(app, app.draft_image(a), COMPOSER_THUMB * s)
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
        .child(text(preview.name.clone()).font(app.ty.sized(14.0)).color(rgb(p.fg)).truncate().max_w((w - 112.0).max(1.0)).absolute(32.0, 20.0))
        .child(el().key("image-preview-close").w(40.0).h(40.0).absolute(w - 56.0, 12.0).radius(6.0).fill(rgb(p.surface)).center()
            .child(text("\u{d7}").font(app.ty.sized(24.0)).color(rgb(p.fg)))
            .on_press(|s: &mut App| s.attach.preview = None));
    if let Some(index) = gallery_index.filter(|_| count > 1) {
        modal = modal.child(text(format!("{} / {}", index + 1, count)).font(app.ty.sized(12.0)).color(rgb(p.fg)).absolute(w / 2.0 - 20.0, h - 26.0));
        for (label, step, x) in [("<", -1, 12.0), (">", 1, w - 52.0)] {
            modal = modal.child(el().w(40.0).h(48.0).absolute(x, h / 2.0 - 24.0).radius(8.0).fill(rgb(p.surface)).center()
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
