//! The browser's clipboard: reads and writes go through the Clipboard API and DOM paste events.

use super::*;

impl Client {
    pub(super) fn read_for(&self, purpose: Purpose) {
        // Start the permission-sensitive operation before yielding the input handler.
        let forms = purpose.forms();
        let promise = crate::platform::web::platform::read_clipboard(forms != Forms::Text, forms == Forms::All, MAX_IMAGE_BYTES as u32);
        let (sender, wake) = (self.sender.clone(), self.wake.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let content = match wasm_bindgen_futures::JsFuture::from(promise).await {
                Ok(value) => {
                    let field = |name: &str| js_sys::Reflect::get(&value, &name.into()).ok().and_then(|v| v.as_string());
                    let html = field("html").filter(|h| !h.is_empty());
                    let text = field("text");
                    let image = web_png(js_sys::Reflect::get(&value, &"png".into()).ok());
                    (text.is_some() || html.is_some() || image.is_some()).then(|| {
                        let mut content = ClipboardContent::new(text.unwrap_or_default());
                        if let Some(html) = html { content = content.with_html(html); }
                        if let Some(image) = image { content = content.with_image(image); }
                        content
                    })
                }
                Err(_) => {
                    web_sys::console::warn_1(&"Clipboard read denied; use the browser's Paste command.".into());
                    None
                }
            };
            if let Some(reply) = purpose.reply(content) && sender.send(reply).is_ok() { wake(); }
        });
    }
    /// A DOM paste into the focused field: the text and HTML come with the event, and when it
    /// carried an image, `png` reads its bytes, only if the target takes images.
    pub fn paste_event<S: 'static>(&self, rt: &mut Runtime<S>, content: ClipboardContent, png: Option<impl FnOnce() -> js_sys::Promise>) {
        let purpose = Purpose::paste(rt, rt.paste_target());
        let content = if purpose.forms() == Forms::Text { ClipboardContent::new(content.text()) } else { content };
        let Some(png) = png.filter(|_| purpose.forms() == Forms::All) else { return rt.paste_content(content) };
        let promise = png();
        let (sender, wake) = (self.sender.clone(), self.wake.clone());
        wasm_bindgen_futures::spawn_local(async move {
            let image = web_png(wasm_bindgen_futures::JsFuture::from(promise).await.ok());
            let content = match image { Some(image) => content.with_image(image), None => content };
            if let Some(reply) = purpose.reply(Some(content)) && sender.send(reply).is_ok() { wake(); }
        });
    }
    pub fn flush<S: 'static>(&self, rt: &mut Runtime<S>) {
        if rt.clipboard.is_some() { return flush_memory(rt); }
        rt.clear_secret_request = false;
        if let Some(copy) = rt.copy_request.take() {
            if copy.is_secret() {
                if let Some(slot) = copy.slot() {
                    slot.set(SecretState::Failed("secret clipboard writes are unsupported on web".into()));
                    self.changed.store(true, Ordering::SeqCst);
                    (self.wake)();
                }
            } else {
                // The browser's Clipboard API takes plain text, HTML and PNG; RTF and files stay behind.
                let content = copy.content();
                let png = content.image().map_or(&[][..], ClipboardImage::png);
                let promise = crate::platform::web::platform::write_clipboard(content.text(), content.html().unwrap_or(""), png);
                wasm_bindgen_futures::spawn_local(async move {
                    if wasm_bindgen_futures::JsFuture::from(promise).await.is_err() {
                        web_sys::console::warn_1(&"Clipboard write denied by the browser.".into());
                    }
                });
            }
        }
        if let Some(target) = rt.paste_request.take() { self.read_for(Purpose::paste(rt, target)); }
        for id in std::mem::take(&mut rt.read_requests) { self.read_for(Purpose::Read(id)); }
    }
}

/// A `Uint8Array` of PNG bytes from the browser.
fn web_png(value: Option<wasm_bindgen::JsValue>) -> Option<ClipboardImage> {
    use wasm_bindgen::JsCast;
    let bytes = value?.dyn_into::<js_sys::Uint8Array>().ok()?;
    if bytes.length() as usize > MAX_IMAGE_BYTES { return None; }
    ClipboardImage::from_png(bytes.to_vec())
}

