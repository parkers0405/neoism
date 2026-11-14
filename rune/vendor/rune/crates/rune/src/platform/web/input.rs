use super::*;
use wasm_bindgen::{JsCast, JsValue, prelude::*};
use winit::event::{ElementState, MouseButton, TouchPhase};
use winit::keyboard::ModifiersState;

#[wasm_bindgen(module = "/src/platform/web/bridge.js")]
extern "C" {
    fn mount(canvas: &web_sys::HtmlCanvasElement, callback: &js_sys::Function, embedded: bool, fit_height: bool, document: bool) -> JsValue;
    fn sync(handle: &JsValue, key: &str, text: &str, base: usize, anchor: u32, focus: u32, secure: bool);
    #[wasm_bindgen(js_name = syncCopy)]
    fn sync_copy(handle: &JsValue, text: &str);
    #[wasm_bindgen(js_name = syncA11y)]
    fn sync_a11y(handle: &JsValue, json: &str);
    #[wasm_bindgen(js_name = syncScrollX)]
    fn sync_scroll_x(handle: &JsValue, path: &str, x: f32);
    fn activate(handle: &JsValue);
    #[wasm_bindgen(js_name = syncPage)]
    fn sync_page(handle: &JsValue, height: f32, focused: bool);
    #[wasm_bindgen(js_name = presented)]
    fn presented_js(handle: &JsValue);
    fn destroy(handle: &JsValue);
    #[wasm_bindgen(js_name = pastedImage)]
    fn pasted_image(handle: &JsValue, max: u32) -> js_sys::Promise;
}

/// How the canvas sits in the page.
#[derive(Clone, Copy)]
pub(super) struct Mode {
    /// Inline at its parent's width.
    pub embedded: bool,
    /// As tall as its content: vertical wheel and pans belong to the page.
    pub fit_height: bool,
    /// The app is the page (`App::web_document`).
    pub document: bool,
}

pub(super) struct Bridge {
    handle: JsValue,
    _callback: Closure<dyn FnMut(String)>,
    dispose_viewport: js_sys::Function,
    mods: ModifiersState,
    mirror: crate::runtime::web_runtime::Mirror,
    a11y: crate::runtime::web_a11y::A11yMirror,
    copy_cache: String,
    fit_height: bool,
    document: bool,
    /// The page height and control focus the page last heard.
    page: Option<(f32, bool)>,
    /// The horizontal offsets the mirror's containers were last set to.
    scroll_x: Vec<(String, f32)>,
}

impl Bridge {
    pub(super) fn new<S: 'static>(canvas: &web_sys::HtmlCanvasElement,
        running: std::rc::Weak<RefCell<Option<Running<S>>>>, window: Arc<Window>,
        mode: Mode) -> Self {
        let Mode { embedded, fit_height, document } = mode;
        let callback = Closure::new(move |event: String| {
            let Some(running) = running.upgrade() else { return };
            let event = zeroize::Zeroizing::new(event);
            if let Ok(mut event) = serde_json::from_str(&event) {
                // A DOM event fired synchronously by page JavaScript Rune called mid-frame (a
                // same-tab router tearing the route down) is dropped rather than re-entering.
                let Ok(mut running) = running.try_borrow_mut() else { return };
                if let Some(r) = &mut *running && r.input_event(&mut event, &window) { window.request_redraw(); }
            }
        });
        // A page's canvas is sized by its style and reports no safe area: the page lays out
        // around the notch itself.
        let dispose_viewport = platform::viewport(canvas, callback.as_ref().unchecked_ref(), embedded || document);
        let handle = mount(canvas, callback.as_ref().unchecked_ref(), embedded, fit_height, document);
        Self { handle, _callback: callback, dispose_viewport, mods: ModifiersState::empty(),
            mirror: Default::default(), a11y: Default::default(),
            copy_cache: String::new(), fit_height, document, page: None, scroll_x: Vec::new() }
    }

    /// The first frame is on screen: a page swaps its static export for the live mirror.
    pub(super) fn presented(&self) { presented_js(&self.handle); }
}
impl Drop for Bridge {
    fn drop(&mut self) {
        let _ = self.dispose_viewport.call0(&JsValue::NULL);
        destroy(&self.handle);
    }
}

impl<S: 'static> Running<S> {
    pub(super) fn sync_input(&mut self, focus: bool) {
        if self.rt.web_context(&mut self.input.mirror) {
            let m = &self.input.mirror;
            sync(&self.input.handle, &m.key, &m.text, m.base, m.anchor, m.focus, m.secure);
        }
        let next_copy = self.rt.copy_text().unwrap_or_default();
        if self.input.copy_cache != next_copy {
            self.input.copy_cache = next_copy;
            sync_copy(&self.input.handle, &self.input.copy_cache);
        }
        if let Some(json) = self.rt.web_a11y(&mut self.input.a11y) {
            sync_a11y(&self.input.handle, json);
        }
        if self.input.document {
            let page = (self.rt.page_height().ceil(), self.rt.has_focus());
            if self.input.page != Some(page) {
                self.input.page = Some(page);
                sync_page(&self.input.handle, page.0, page.1);
            }
            // A horizontal viewport Rune scrolled (a wheel or a key on desktop) moves the mirror's
            // container to match; one the browser scrolled already says where it is.
            let handle = &self.input.handle;
            let next = self.rt.page_scroll_x();
            for (path, x) in &next {
                let known = self.input.scroll_x.iter().find(|(p, _)| p == path).map(|(_, x)| *x);
                if known.is_none_or(|known| (known - x).abs() > 0.5) { sync_scroll_x(handle, path, *x); }
            }
            self.input.scroll_x = next;
        }
        if focus { activate(&self.input.handle); }
    }

    /// Handles one DOM event. Returns whether a frame must be drawn: a page's scroll only draws
    /// when it moves the band (the browser's compositor scrolls the pixels Rune already drew).
    pub(super) fn input_event(&mut self, event: &mut serde_json::Value, window: &Arc<Window>) -> bool {
        let mut activate = false;
        match event["kind"].as_str() {
            Some("viewport") => {
                let insets = std::array::from_fn(|i| event["insets"][i].as_f64().unwrap_or(0.0).max(0.0) as f32);
                let h = event["h"].as_f64().unwrap_or(0.0).max(0.0) as f32;
                // A phone reports the visual viewport on every scroll frame as its toolbar moves;
                // when neither the safe area nor the largest viewport moved, there's nothing to do.
                let moved = LARGE_VIEWPORT.replace(h) != h;
                if INSETS.replace(insets) == insets && !moved { return false; }
                if moved { self.resize_surface(window, window.inner_size(), window.scale_factor() as f32); }
                self.rt.invalidate();
            }
            Some("dpr") => {
                let size = window.inner_size();
                let scale = event["scale"].as_f64().unwrap_or_else(|| window.scale_factor()).max(0.5) as f32;
                self.resize_surface(window, size, scale);
            }
            Some("motion") => self.rt.set_system_reduced_motion(event["reduced"].as_bool().unwrap_or(false)),
            Some("focus") => self.rt.window_focus(event["focused"].as_bool().unwrap_or(false)),
            Some("visibility") => {
                self.shared.visible.store(event["visible"].as_bool().unwrap_or(false), Ordering::Release);
                return true;
            }
            Some("edit" | "selection" | "preedit") => self.rt.web_edit(event),
            Some("sync") => self.input.mirror.key.clear(), // Composition's final sync was deferred by the DOM.
            Some("paste") => {
                let mut content = crate::ClipboardContent::new(event["text"].as_str().unwrap_or_default());
                if let Some(html) = event["html"].as_str().filter(|html| !html.is_empty()) { content = content.with_html(html); }
                let handle = &self.input.handle;
                let image = (event["image"].as_bool() == Some(true)).then_some(|| pasted_image(handle, crate::clipboard::MAX_IMAGE_BYTES as u32));
                self.clipboard.paste_event(&mut self.rt, content, image);
            }
            Some("secure") => {
                if let serde_json::Value::String(text) = event["text"].take() {
                    self.rt.web_secure(event["key"].as_str().unwrap_or_default(), text);
                }
            }
            Some("key") => activate = self.rt.web_key(event),
            Some("a11y_press" | "a11y_link") => { let _ = self.rt.web_a11y_action(event); }
            // The page scrolled: draw the band in view, and hover what is now under the pointer.
            // The redraw that follows the event draws it; nothing commits.
            Some("scroll") => {
                let before = self.rt.page_scroll_y();
                let moved = self.rt.set_page_scroll(event["y"].as_f64().unwrap_or(0.0) as f32);
                // The pointer keeps its place on screen while the band moves under it.
                let turned = self.rt.page_scroll_y() - before;
                if turned != 0.0 && let Some((x, y)) = self.rt.pointer().filter(|_| !self.rt.touching()) {
                    self.rt.pointer_move(x, y + turned);
                }
                return moved;
            }
            // A page's horizontal viewport scrolled natively, by the browser's own compositor.
            Some("scroll_x") => {
                let path = event["path"].as_str().unwrap_or_default();
                let x = event["x"].as_f64().unwrap_or(0.0) as f32;
                return self.rt.set_page_scroll_x(path, x);
            }
            // The pointer is over mirrored text or a link, which takes it from the canvas.
            Some("hover") => {
                if self.rt.touching() { return false; }
                // The mirror's pointer is in window points; the canvas draws the band.
                let (x, y) = (event["x"].as_f64().unwrap_or(0.0) as f32, event["y"].as_f64().unwrap_or(0.0) as f32);
                self.rt.pointer_move(x, y + self.rt.page_window().0);
                return true;
            }
            _ => {}
        }
        // Commit within the DOM event: clipboard, link and soft-keyboard APIs need activation.
        self.rt.frame(0.0);
        self.apply_frame_requests(window);
        self.clipboard.flush(&mut self.rt);
        self.sync_input(activate);
        true
    }

    pub(super) fn window_input(&mut self, event: &WindowEvent, window: &Arc<Window>) -> bool {
        let scale = window.scale_factor() as f32;
        let mut activate = false;
        let mut immediate = false;
        match event {
            WindowEvent::ModifiersChanged(mods) => { self.input.mods = mods.state(); return false; }
            WindowEvent::CursorMoved { position, .. } if !self.rt.touching() => {
                self.rt.pointer_move(position.x as f32 / scale, position.y as f32 / scale);
                self.cursor.apply(window, self.rt.cursor(), true);
            }
            WindowEvent::CursorLeft { .. } if !self.rt.touching() => {
                self.rt.pointer_leave();
                self.cursor.apply(window, crate::el::Cursor::Default, false);
            }
            WindowEvent::MouseInput { button, state, .. } if !self.rt.touching() => {
                immediate = true;
                let Some((x, y)) = self.rt.pointer() else { return false };
                match (button, state) {
                    (MouseButton::Left, ElementState::Pressed) => { self.rt.pointer_down(x, y); activate = true; }
                    (MouseButton::Left, ElementState::Released) => self.rt.pointer_up(x, y),
                    (MouseButton::Right, ElementState::Pressed) => self.rt.secondary_down(x, y),
                    (MouseButton::Right, ElementState::Released) => self.rt.secondary_up(x, y),
                    (&button, ElementState::Pressed) => self.rt.button_down(x, y, crate::window::pointer_button(button)),
                    (&button, ElementState::Released) => self.rt.button_up(x, y, crate::window::pointer_button(button)),
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let Some((x, y)) = self.rt.pointer() else { return false };
                let (dx, dy) = crate::runtime::wheel_delta(*delta, scale, self.input.mods.shift_key());
                // A page's pinch is the browser's zoom.
                if self.input.mods.control_key() { return !self.input.document && self.rt.pinch_wheel(x, y, dy); }
                // On a content-sized embedded canvas, vertical wheel belongs to the page unless
                // a horizontal strip is being scrolled.
                if self.input.fit_height && dx.abs() <= dy.abs() { return false; }
                let precise = matches!(delta, winit::event::MouseScrollDelta::PixelDelta(_));
                self.rt.wheel_event(crate::WheelEvent::new(dx, dy).at(x, y).mods(modifiers(self.input.mods)).precise(precise));
            }
            // A page's Tab is the browser's, through the mirror's links and buttons.
            WindowEvent::KeyboardInput { event, .. }
                if self.input.document && event.logical_key == winit::keyboard::Key::Named(winit::keyboard::NamedKey::Tab) => return false,
            WindowEvent::KeyboardInput { event, is_synthetic: false, .. } => {
                immediate = true;
                let pressed = event.state == ElementState::Pressed;
                let mut logical = (pressed || self.rt.wants_key_up()).then(|| crate::window::map_key_event(&event, self.input.mods)).flatten();
                if let Some(key) = &mut logical {
                    key.mods = modifiers(self.input.mods);
                    if key.mods.cmd && let crate::Key::Text(text) = &key.key && text != " " && text.chars().count() == 1 {
                        key.key = crate::Key::Command(text.chars().next().unwrap().to_ascii_lowercase());
                    }
                }
                activate = logical.as_ref().is_some_and(|k| k.key == crate::Key::Tab);
                if let winit::keyboard::PhysicalKey::Code(key) = event.physical_key {
                    self.rt.physical_with_fallback(crate::PhysicalInput::Key { key, mods: modifiers(self.input.mods),
                        pressed: event.state == ElementState::Pressed, repeat: event.repeat }, logical);
                } else if let Some(key) = logical {
                    if pressed { self.rt.key(key); } else { self.rt.key_up(key); }
                }
            }
            // On a canvas as tall as its content the browser takes vertical pans and cancels the
            // touch; taps and horizontal drags stay Rune's.
            WindowEvent::Touch(touch) => {
                crate::window::touch(&mut self.rt, touch, scale);
                // A tap is a user activation: links, the clipboard and focus act before the handler
                // returns. In a page only a tap that left something focused needs it; anything else
                // would focus the canvas and drop the reader's selection.
                if matches!(touch.phase, TouchPhase::Ended) {
                    activate = !self.input.document || self.rt.has_focus();
                    immediate = true;
                }
            }
            _ => return false,
        }
        // Moves and wheels are coalesced by rAF. Only activation-sensitive input must
        // commit before the DOM handler returns (clipboard, links and keyboard focus).
        if immediate {
            self.rt.frame(0.0);
            self.apply_frame_requests(window);
            self.clipboard.flush(&mut self.rt);
            self.sync_input(activate);
            self.cursor.apply(window, self.rt.cursor(), self.rt.pointer().is_some());
        }
        true
    }
}

fn modifiers(mods: ModifiersState) -> crate::Mods {
    crate::Mods { shift: mods.shift_key(), alt: mods.alt_key(), ctrl: mods.control_key(),
        logo: mods.super_key(), cmd: mods.super_key() || mods.control_key() }
}
