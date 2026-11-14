//! Single-threaded browser shell. GPU initialization yields to the browser; winit owns
//! demand-driven rAF and its device-pixel-content-box ResizeObserver (with DPR fallback).
use std::{cell::RefCell, rc::Rc, sync::{Arc, atomic::Ordering}, time::Duration};
use wasm_bindgen::JsCast;
use winit::{application::ApplicationHandler, event::WindowEvent, event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy},
    platform::web::{EventLoopExtWebSys, WindowAttributesExtWebSys, WindowExtWebSys}, window::{Window, WindowId}};
use crate::{App, Runtime, Env, render::{Gpu, Renderer}, time::Instant};
use super::web_math;

/// The overscan a page's canvas carries above and below the window, in CSS points: the distance
/// the browser's compositor can scroll before Rune has to draw a new band.
const SKIRT: f32 = 256.0;
#[cfg(feature = "embedded-fonts")]
mod full_faces;
mod input;
mod page;
pub(crate) mod platform;

thread_local! {
    static INSETS: std::cell::Cell<[f32; 4]> = const { std::cell::Cell::new([0.0; 4]) };
    /// The largest viewport's height in CSS points (`100lvh`): a page's band is sized from it so
    /// a phone's toolbar coming and going never changes the band.
    static LARGE_VIEWPORT: std::cell::Cell<f32> = const { std::cell::Cell::new(0.0) };
}

pub(crate) fn safe_area() -> [f32; 4] { INSETS.get() }

/// The largest viewport's height in CSS points, or 0 before the browser has reported one.
pub(crate) fn large_viewport() -> f32 { LARGE_VIEWPORT.get() }

/// Open an HTTP(S) or mail link in a new browser tab. Call from a user input handler;
/// browser popup policy applies. The new page receives no opener or referrer.
pub fn open_url(url: &str) -> Result<(), String> {
    let url = crate::open_url::check(url).map_err(|e| e.to_string())?;
    platform::open_link(&url).map_err(|e| format!("{e:?}"))
}

/// Open a link in this tab, as `Tx::open_url_same_tab` does: a link on the page's origin goes
/// to the app's `App::same_tab_router` first, and the browser loads it with `location.assign`
/// unless the router handled it. A link without a scheme (`/notes/42`) is relative to the page.
pub fn open_url_same_tab(url: &str) -> Result<(), String> {
    let url = crate::open_url::check_same_tab(url).map_err(|e| e.to_string())?;
    let resolved = platform::resolve_link(&url).map_err(|e| format!("{e:?}"))?;
    let href = resolved.get(0).as_string().unwrap_or_default();
    if resolved.get(1).is_truthy() && page::route(&href) { return Ok(()); }
    platform::assign_link(&href).map_err(|e| format!("{e:?}"))
}

#[derive(Clone, Copy)]
enum Wake { Ready, App }

pub(crate) fn run<S: 'static>(app: App<S>) -> Result<(), String> {
    if !app.overlays.is_empty() || !app.windows.is_empty() {
        return Err("web supports one canvas; overlays and multiple windows are unsupported".into());
    }
    if !app.open || app.on_window.is_some() || app.on_ready.is_some() {
        return Err("web does not support windowless apps or native window hooks".into());
    }
    console_error_panic_hook::set_once();
    page::start(quitting(&app.shared), Box::new(move || spawn(app)))
}

fn quitting<S: 'static>(shared: &Arc<crate::app::Shared<S>>) -> Box<dyn Fn() -> bool> {
    let shared = shared.clone();
    Box::new(move || shared.quitting.load(Ordering::Acquire))
}

fn spawn<S: 'static>(mut app: App<S>) -> Result<(), String> {
    if app.shared.quitting.load(Ordering::Acquire) { return Ok(()); }
    let event_loop = EventLoop::<Wake>::with_user_event().build().map_err(|e| e.to_string())?;
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let shared = app.shared.clone();
    let wake = proxy.clone();
    shared.connect(Arc::new(move || { let _ = wake.send_event(Wake::App); }));
    page::occupy(quitting(&shared), app.same_tab_router.take());
    event_loop.spawn_app(Browser { app: Some(app), shared, proxy, window: None, embedded: false, document: false, owns_canvas: true,
        borrowed: page::Borrowed::default(), pending: Rc::default(), running: Rc::default() });
    Ok(())
}

struct Browser<S: 'static> {
    app: Option<App<S>>,
    shared: Arc<crate::app::Shared<S>>,
    proxy: EventLoopProxy<Wake>,
    window: Option<Arc<Window>>,
    /// Inline in the page at its parent's width, rather than covering the viewport.
    embedded: bool,
    /// The app is the page (`App::web_document`): the browser scrolls it.
    document: bool,
    /// Rune made the canvas (no `App::canvas_id`), so it removes it when the app ends.
    owns_canvas: bool,
    borrowed: page::Borrowed,
    /// The GPU, and the bytes of `App::font_url`'s fonts that loaded.
    pending: Rc<RefCell<Option<(Result<(Gpu, wgpu::Surface<'static>), String>, Vec<Vec<u8>>)>>>,
    running: Rc<RefCell<Option<Running<S>>>>,
}

struct Running<S: 'static> {
    gpu: Gpu,
    surface: wgpu::Surface<'static>,
    config: wgpu::SurfaceConfiguration,
    renderer: Renderer,
    rt: Runtime<S>,
    shared: Arc<crate::app::Shared<S>>,
    clock: crate::frame::clock::FrameClock,
    compositor: Option<Box<dyn crate::Compositor<S>>>,
    suspended: bool,
    embedded: bool,
    document: bool,
    /// The first frame is on screen.
    presented: bool,
    /// Where the canvas was last put in the document (a page's band top), in points.
    band: Option<f32>,
    cursor: crate::cursor::os::OsCursor,
    _menus: crate::native_menu::Guard<S>,
    shortcuts: crate::shortcuts::Guard,
    input: input::Bridge,
    clipboard: crate::clipboard::Client,
}

impl<S: 'static> Running<S> {
    /// Ends the app: listeners off the page, then the GPU device, which the WebGPU backend
    /// doesn't destroy on drop.
    fn close(self) {
        self.shared.visible.store(false, Ordering::Release);
        let device = self.gpu.device.clone();
        drop(self);
        device.destroy();
    }

    pub(super) fn resize_surface(&mut self, window: &Window, size: winit::dpi::PhysicalSize<u32>, scale: f32) {
        let max_dim = self.gpu.device.limits().max_texture_dimension_2d.max(1);
        if self.document {
            // The canvas covers a band of the page: size it before the box is measured.
            let (view, band) = band_css(window, scale, max_dim);
            set_band_height(window, band);
            self.rt.set_band(band, view);
        }
        let size = window.canvas().map_or(size, |c| canvas_physical(&c, size, scale));
        self.suspended = size.width == 0 || size.height == 0;
        if self.suspended { return; }
        let next = (size.width.min(max_dim), size.height.min(max_dim));
        let (kept, reconfigure) = web_math::should_reconfigure_surface((self.config.width, self.config.height), next, self.document);
        if reconfigure {
            self.config.width = kept.0;
            self.config.height = kept.1;
            self.surface.configure(&self.gpu.device, &self.config);
        }
        self.rt.set_viewport((next.0 as f32 / scale, next.1 as f32 / scale), scale);
    }

    /// Puts a page's canvas where the band it just drew starts in the document. The browser's
    /// compositor scrolls those pixels itself, at its own rate, until the window nears the band's
    /// edge and the next frame draws a new one.
    fn sync_band(&mut self, window: &Window) {
        if !self.document { return; }
        let top = self.rt.page_offset();
        if self.band == Some(top) { return; }
        self.band = Some(top);
        if let Some(canvas) = window.canvas() {
            let _ = canvas.style().set_property("transform", &format!("translate3d(0,{top}px,0)"));
        }
    }

    pub(super) fn apply_frame_requests(&mut self, window: &Window) {
        if !self.embedded
            && let Some(title) = self.rt.window_title.take_change()
            && let Some(doc) = web_sys::window().and_then(|w| w.document())
        {
            doc.set_title(title);
        }
        // A page's height is the mirror's, which `sync_input` keeps; the canvas stays the band.
        if self.document {
            self.rt.window_frame = None;
        } else if let Some(req) = self.rt.window_frame.take()
            && let Some((_, h)) = req.size
            && let Some(canvas) = window.canvas()
        {
            let _ = canvas.style().set_property("height", &format!("{h}px"));
        }
    }
}

/// A page's band in CSS points: `view`, how much of the document the window shows at once, and
/// `band`, the canvas that covers it. The view height is the *largest* viewport (`100lvh`), so a
/// phone's toolbar coming and going never changes the band. The skirt is clamped so the texture
/// stays inside the device's limit; a viewport already at the limit has no skirt.
fn band_css(window: &Window, scale: f32, max_dim: u32) -> (f32, f32) {
    let scale = scale.max(1e-3);
    let reported = large_viewport();
    let view = if reported > 0.0 { reported } else { window.inner_size().height as f32 / scale };
    let max = max_dim as f32 / scale;
    let band = (view + 2.0 * SKIRT).min(max.max(view));
    (view, band)
}

/// The canvas's CSS height is its band, in points.
fn set_band_height(window: &Window, band: f32) {
    if let Some(canvas) = window.canvas() {
        let height = format!("{band}px");
        if canvas.style().get_property_value("height").unwrap_or_default() != height {
            let _ = canvas.style().set_property("height", &height);
        }
    }
}

/// The canvas's size in device pixels: its CSS box times `scale` when the browser's own
/// `devicePixelContentBoxSize` (what winit reports) disagrees, as Chromium's emulated device
/// scale does by reporting CSS pixels. Trusting it would lay out at a fraction of the box.
fn canvas_physical(canvas: &web_sys::HtmlCanvasElement, reported: winit::dpi::PhysicalSize<u32>, scale: f32) -> winit::dpi::PhysicalSize<u32> {
    let rect = canvas.get_bounding_client_rect();
    let (w, h) = (rect.width() as f32, rect.height() as f32);
    if w <= 0.0 || h <= 0.0 { return reported; }
    let fix = |got: u32, css: f32| { let want = (css * scale).round().max(1.0); if (got as f32 - want).abs() > scale.ceil() + 1.0 { want as u32 } else { got } };
    winit::dpi::PhysicalSize::new(fix(reported.width, w), fix(reported.height, h))
}

/// Reports `message` to the console, and on screen unless the app is the page: a document's
/// static HTML is a working page, and an opaque overlay would cover the page a reader can still
/// read and scroll. Ending the app leaves that page as the browser found it.
fn error(message: &str, target: Option<&web_sys::HtmlCanvasElement>, embedded: bool, document: bool) {
    web_sys::console::error_1(&message.into());
    let Some(doc) = web_sys::window().and_then(|w| w.document()) else { return };
    if document && doc.query_selector("[data-rune-static]").ok().flatten().is_some() { return; }
    if let Ok(label) = doc.create_element("pre") {
        label.set_text_content(Some(message));
        if embedded && let Some(canvas) = target && let Some(parent) = canvas.parent_element() {
            let _ = label.set_attribute("style", "margin:0;padding:16px;white-space:pre-wrap;background:Canvas;color:CanvasText;border:1px solid #444;border-radius:8px");
            let _ = parent.insert_before(&label, Some(canvas));
            return;
        }
        if let Some(body) = doc.body() {
            let _ = label.set_attribute("style", "position:fixed;inset:0;z-index:1;padding:24px;white-space:pre-wrap;background:Canvas;color:CanvasText");
            let _ = body.append_child(&label);
        }
    }
}

async fn try_backend(canvas: &web_sys::HtmlCanvasElement, backends: wgpu::Backends, features: wgpu::Features, webgl: bool)
    -> Result<(Gpu, wgpu::Surface<'static>), String> {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = backends;
    let instance = wgpu::Instance::new(descriptor);
    let surface = instance.create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone())).map_err(|e| e.to_string())?;
    let adapter = instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower, compatible_surface: Some(&surface),
        force_fallback_adapter: false, apply_limit_buckets: false,
    }).await.map_err(|e| e.to_string())?;
    let required_limits = if webgl {
        wgpu::Limits::downlevel_webgl2_defaults().using_resolution(adapter.limits())
    } else {
        wgpu::Limits::default().using_resolution(adapter.limits())
    };
    let (device, queue) = adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("rune web"), required_features: adapter.features() & features,
        required_limits, experimental_features: Default::default(),
        memory_hints: Default::default(), trace: Default::default(),
    }).await.map_err(|e| format!("GPU device: {e}"))?;
    Ok((Gpu { instance, adapter, device, queue }, surface))
}

async fn gpu(canvas: web_sys::HtmlCanvasElement, features: wgpu::Features) -> Result<(Gpu, wgpu::Surface<'static>), String> {
    match try_backend(&canvas, wgpu::Backends::BROWSER_WEBGPU, features, false).await {
        Ok(pair) => Ok(pair),
        #[cfg(feature = "webgl")]
        Err(webgpu_err) => try_backend(&canvas, wgpu::Backends::GL, features, true)
            .await
            .map_err(|gl_err| format!("WebGPU ({webgpu_err}) and WebGL2 ({gl_err}) unavailable")),
        #[cfg(not(feature = "webgl"))]
        Err(e) => Err(format!("WebGPU adapter unavailable (HTTPS and WebGPU are required): {e}")),
    }
}

impl<S: 'static> Browser<S> {
    fn end(&self, event_loop: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        event_loop.exit();
    }
}

impl<S: 'static> ApplicationHandler<Wake> for Browser<S> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() { return; }
        let app = self.app.as_ref().expect("app before canvas");
        self.document = app.web_document;
        self.embedded = app.web_inline() && !self.document;
        let existing = match &app.canvas_id {
            Some(id) => {
                let Some(el) = web_sys::window().and_then(|w| w.document()).and_then(|d| d.get_element_by_id(id)) else {
                    error(&format!("canvas #{id} not found in document"), None, true, self.document);
                    self.end(event_loop);
                    return;
                };
                match el.dyn_into::<web_sys::HtmlCanvasElement>() {
                    Ok(canvas) => Some(canvas),
                    Err(_) => {
                        error(&format!("element #{id} is not a <canvas>"), None, true, self.document);
                        self.end(event_loop);
                        return;
                    }
                }
            }
            None => None,
        };
        self.owns_canvas = existing.is_none();
        if let Some(canvas) = &existing { self.borrowed.canvas(canvas); }
        // Not `active`: winit would focus the canvas before it's styled, and a browser scrolls a
        // focused element into view, so a page landed at its foot. The bridge focuses a
        // full-window canvas itself, without scrolling.
        let mut attrs = Window::default_attributes().with_title(app.spec.title).with_prevent_default(false).with_active(false);
        attrs = match &existing {
            Some(c) => attrs.with_canvas(Some(c.clone())).with_append(false),
            None => attrs.with_append(true),
        };
        let window = match event_loop.create_window(attrs) {
            Ok(window) => Arc::new(window),
            Err(e) => { error(&e.to_string(), existing.as_ref(), self.embedded, self.document); self.end(event_loop); return; }
        };
        let canvas = window.canvas().expect("winit web canvas");
        if !canvas.has_attribute("tabindex") {
            let _ = canvas.set_attribute("tabindex", "0");
        }
        if self.document {
            // The browser scrolls the page over a canvas that is part of the document, not fixed
            // to the screen: the compositor moves the pixels Rune drew at the display's own rate,
            // and a frame is only drawn when the window nears the band's edge. Rune puts it in a
            // clipper as tall as the page (the bridge) and translates it to `page.top`.
            let _ = canvas.set_attribute("tabindex", "-1");
            canvas.style().set_css_text("position:absolute;left:0;top:0;width:100%;display:block;transform:translate3d(0,0,0);will-change:transform;pointer-events:auto;touch-action:pan-y pinch-zoom;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.spec.title);
                if let Some(body) = document.body() { let _ = body.style().set_property("margin", "0"); }
            }
        } else if self.embedded {
            let touch = if app.fit_content_height { "pan-y" } else { "none" };
            canvas.style().set_css_text(&format!(
                "display:block;width:100%;touch-action:{touch};outline:none;user-select:none;-webkit-user-select:none"
            ));
        } else {
            // No border/padding: winit's physical input and resize coordinates must match pixels.
            canvas.style().set_css_text("position:fixed;inset:0;width:100%;height:100%;display:block;touch-action:none;outline:none;user-select:none;-webkit-user-select:none");
            if let Some(document) = web_sys::window().and_then(|w| w.document()) {
                self.borrowed.viewport(&document);
                document.set_title(app.spec.title);
                if let Some(body) = document.body() {
                    let style = body.style();
                    for (name, value) in [("margin", "0"), ("overflow", "hidden"), ("overscroll-behavior", "none")] {
                        let _ = style.set_property(name, value);
                    }
                }
            }
        }
        let features = app.compositor.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
        let pending = self.pending.clone();
        let proxy = self.proxy.clone();
        // The fonts download while the GPU starts.
        let fonts: Vec<_> = app.font_urls.iter().map(|url| (url.clone(), platform::fetch_bytes(url))).collect();
        self.window = Some(window);
        wasm_bindgen_futures::spawn_local(async move {
            let result = gpu(canvas, features).await;
            let mut loaded = Vec::new();
            for (url, font) in fonts {
                match wasm_bindgen_futures::JsFuture::from(font).await {
                    Ok(bytes) => loaded.push(js_sys::Uint8Array::new(&bytes).to_vec()),
                    Err(e) => web_sys::console::warn_2(&format!("rune: font {url} did not load:").into(), &e),
                }
            }
            // A loop that has closed was quit while the GPU started.
            if proxy.send_event(Wake::Ready).is_err() {
                if let Ok((gpu, _)) = result { gpu.device.destroy(); }
                return;
            }
            *pending.borrow_mut() = Some((result, loaded));
        });
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: Wake) {
        if self.shared.quitting.load(Ordering::Acquire) { self.end(event_loop); return; }
        let Some(window) = self.window.as_ref() else { return };
        if matches!(event, Wake::Ready) {
            let Some((result, fonts)) = self.pending.borrow_mut().take() else { return };
            let (gpu, surface) = match result {
                Ok(gpu) => gpu,
                Err(e) => {
                    if let Some(app) = &self.app { *app.shared.window_error.lock().unwrap() = Some(e.clone()); }
                    error(&e, window.canvas().as_ref(), self.embedded, self.document); self.end(event_loop); return;
                }
            };
            let mut app = self.app.take().expect("one canvas initialization");
            for bytes in fonts { app.spec.fonts = crate::app::with_font(app.spec.fonts, bytes); }
            let compositor = app.compositor.take();
            let (on_focus, modifier_keys, srgb_blend) = (app.on_focus, app.modifier_keys, app.srgb_blend);
            let on_mods = app.on_mods;
            let (font_smoothing, fit_height) = (app.font_smoothing, app.fit_content_height);
            let tab_visits_controls = app.tab_visits_controls;
            let playground = app.playground.take();
            let (spec, state, shared) = app.into_parts(None);
            let scale = window.scale_factor() as f32;
            let document = self.document;
            // A page's canvas is its band: the visible height plus a skirt of overscan, sized
            // before the box is measured so the first frame is that tall.
            let band = if document {
                let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
                let (view, band) = band_css(window, scale, max_dim);
                set_band_height(window, band);
                Some((view, band))
            } else {
                None
            };
            // Both sides from the canvas's own box: a browser that reports the box in CSS pixels
            // (Chromium's emulated device scale) would otherwise leave the height at the CSS
            // height, which its style stretches back over the screen: a 2× vertical stretch.
            let inner = window.inner_size();
            let css = window.canvas().map_or((0.0, 0.0), |c| (c.client_width().max(0) as f32, c.client_height().max(0) as f32));
            let fixed = web_math::fix_canvas_size(css, (inner.width, inner.height), spec.size, scale, fit_height);
            let mut size = winit::dpi::PhysicalSize::new(fixed.0, fixed.1);
            let dark = web_sys::window().and_then(|w| w.match_media("(prefers-color-scheme: dark)").ok().flatten()).is_some_and(|q| q.matches());
            let proxy = self.proxy.clone();
            let wake: crate::Waker = Arc::new(move || { let _ = proxy.send_event(Wake::App); });
            let mut rt = crate::tooling::runtime_for(&spec, state, (size.width as f32 / scale, size.height as f32 / scale), Env::new(dark, scale), wake.clone());
            crate::playground::attach(&mut rt, playground, &spec, None, None, true);
            if let Some((view, band)) = band {
                rt.page_mode();
                rt.set_band(band, view);
                rt.set_viewport((size.width as f32 / scale, band), scale);
            } else if fit_height {
                rt.fit_height = true;
                let w = (size.width as f32 / scale).max(1.0);
                rt.set_viewport((w, spec.size.1), scale);
                let h = rt.content_height();
                rt.set_viewport((w, h), scale);
                size.height = (h * scale).round().max(1.0) as u32;
                if let Some(canvas) = window.canvas() {
                    let _ = canvas.style().set_property("height", &format!("{h}px"));
                }
            }
            rt.on_focus = on_focus;
            rt.on_mods = on_mods;
            rt.set_modifier_keys(modifier_keys);
            rt.set_tab_visits_controls(tab_visits_controls);
            rt.set_srgb_blend(srgb_blend);
            rt.set_font_smoothing(font_smoothing);
            rt.set_os_drag(false);
            let now = Instant::now();
            rt.wall_start = Some(now);
            rt.open_urls_natively = true;
            rt.record_events = false;
            let menus = crate::native_menu::Guard::attach(&shared, false);
            let shortcuts = crate::shortcuts::Guard::attach(&shared, wake.clone(), false, false);
            let mut clipboard = crate::clipboard::Client::default();
            clipboard.connect(wake);
            shared.visible.store(true, Ordering::Release);
            let max_dim = gpu.device.limits().max_texture_dimension_2d.max(1);
            let Some(mut config) = surface.get_default_config(&gpu.adapter, size.width.clamp(1, max_dim), size.height.clamp(1, max_dim)) else {
                error("WebGPU canvas has no supported configuration", window.canvas().as_ref(), self.embedded, self.document); self.end(event_loop); return;
            };
            config.present_mode = wgpu::PresentMode::Fifo;
            let caps = surface.get_capabilities(&gpu.adapter);
            config.usage |= caps.usages & wgpu::TextureUsages::COPY_SRC;
            let wanted = if srgb_blend { config.format.remove_srgb_suffix() } else { config.format.add_srgb_suffix() };
            let is_gl = gpu.adapter.get_info().backend == wgpu::Backend::Gl;
            let format = if wanted != config.format && !is_gl {
                config.view_formats.push(wanted);
                wanted
            } else {
                config.format
            };
            surface.configure(&gpu.device, &config);
            let mut renderer = Renderer::new(&gpu.device, format);
            renderer.set_srgb_blend(!format.is_srgb());
            renderer.attach_text(&mut rt.text.atlas);
            let cursor = crate::cursor::os::OsCursor::new(event_loop);
            let input = input::Bridge::new(&window.canvas().unwrap(), Rc::downgrade(&self.running), window.clone(),
                input::Mode { embedded: self.embedded, fit_height, document });
            *self.running.borrow_mut() = Some(Running { gpu, surface, config, renderer, rt, shared, input, clipboard,
                clock: crate::frame::clock::FrameClock::new(now), compositor, suspended: false, embedded: self.embedded,
                document, presented: false, band: None,
                cursor, _menus: menus, shortcuts });
        }
        if let Some(r) = &mut *self.running.borrow_mut() {
            r.shortcuts.drain();
            r.clipboard.receive(&mut r.rt);
            let requests = r.shared.take();
            if requests.quit { self.end(event_loop); return; }
            if requests.window.as_ref().is_some_and(|r| !matches!(r, crate::app::WindowRequest::Open(_) | crate::app::WindowRequest::Show))
                || requests.position.is_some() || requests.place.is_some() || requests.minimize {
                web_sys::console::warn_1(&"Native window management is ignored on web.".into());
            }
            let had_updates = !requests.updates.is_empty();
            for update in requests.updates { r.rt.update_if(update); }
            if !had_updates { r.rt.invalidate(); }
            window.request_redraw();
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        if matches!(event, WindowEvent::Focused(_)) { return; }
        let mut running = self.running.borrow_mut();
        let (Some(window), Some(r)) = (&self.window, &mut *running) else { return };
        let _input = crate::input::time::arrived(&event);
        if r.window_input(&event, window) { window.request_redraw(); return; }
        match event {
            WindowEvent::Resized(size) => {
                r.resize_surface(window, size, window.scale_factor() as f32);
                window.request_redraw();
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                r.resize_surface(window, window.inner_size(), scale_factor as f32);
                window.request_redraw();
            }
            WindowEvent::ThemeChanged(theme) => { r.rt.set_system_dark(theme == winit::window::Theme::Dark); window.request_redraw(); }
            WindowEvent::RedrawRequested if !r.suspended => {
                let now = Instant::now();
                let (animating, dt) = r.clock.advance(&mut r.rt, now, r.compositor.as_ref().is_some_and(|c| c.animating()));
                r.apply_frame_requests(window);
                r.clipboard.flush(&mut r.rt);
                if r.rt.popup_menu().is_some() {
                    web_sys::console::warn_1(&"Native popup menus are unsupported on web.".into());
                    r.rt.cancel_popups();
                }
                r.rt.refuse_file_dialog("native file dialogs are unsupported on web");
                r.sync_input(false);
                r.cursor.apply(window, r.rt.cursor(), r.rt.pointer().is_some());
                if !r.rt.surfaces.is_empty() {
                    error("native video surfaces are unsupported on web", window.canvas().as_ref(), r.embedded, r.document); self.end(event_loop); return;
                }
                if r.gpu.adapter.get_info().backend != wgpu::Backend::Gl && r.renderer.srgb_blend() != r.rt.srgb_blend() {
                    let format = if r.rt.srgb_blend() { r.config.format.remove_srgb_suffix() } else { r.config.format.add_srgb_suffix() };
                    if format != r.config.format && !r.config.view_formats.contains(&format) {
                        r.config.view_formats.push(format);
                        r.surface.configure(&r.gpu.device, &r.config);
                    }
                    crate::window::rebuild_renderer(&r.gpu.device, &mut r.renderer, format, &mut r.rt);
                }
                let frame = match r.surface.get_current_texture() {
                    wgpu::CurrentSurfaceTexture::Success(frame) | wgpu::CurrentSurfaceTexture::Suboptimal(frame) => frame,
                    _ => { error("WebGPU canvas acquisition failed", window.canvas().as_ref(), r.embedded, r.document); self.end(event_loop); return; }
                };
                let target = frame.texture.create_view(&wgpu::TextureViewDescriptor {
                    format: Some(r.renderer.format()), ..Default::default()
                });
                r.renderer.upload_images(&r.gpu.device, &r.gpu.queue, &mut r.rt.images);
                let mut encoder = r.gpu.device.create_command_encoder(&Default::default());
                if let Some(compositor) = &mut r.compositor {
                    compositor.render(&mut crate::Frame { device: &r.gpu.device, queue: &r.gpu.queue, encoder: &mut encoder,
                        target: &target, format: r.renderer.format(), size: (r.config.width, r.config.height), dt,
                        update_ms: now.elapsed().as_secs_f64() * 1000.0, rt: &mut r.rt, ui: &mut r.renderer });
                } else {
                    r.renderer.prepare(&r.gpu.device, &r.gpu.queue, &mut r.rt.text.atlas, &r.rt.instances, (r.config.width, r.config.height));
                    r.renderer.draw(&mut encoder, &target, r.rt.clear_color());
                }
                r.renderer.submit(&r.gpu.queue, encoder);
                window.pre_present_notify();
                // In the same commit as the frame: the canvas moves to where this frame drew it.
                r.sync_band(window);
                r.gpu.queue.present(frame);
                if !r.presented {
                    r.presented = true;
                    r.input.presented();
                    #[cfg(feature = "embedded-fonts")]
                    {
                        let proxy = self.proxy.clone();
                        full_faces::fetch(move || { let _ = proxy.send_event(Wake::App); });
                    }
                }
                if r.rt.shell_request.take().is_some() { self.end(event_loop); return; }
                if animating || r.rt.has_pending_input() || r.rt.live_pending() || r.compositor.as_ref().is_some_and(|c| c.animating()) {
                    window.request_redraw();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        crate::cursor::os::make_pending(event_loop);
        let Ok(running) = self.running.try_borrow() else { return };
        let Some(r) = &*running else { return };
        event_loop.set_control_flow(match r.rt.wake_deadline() {
            Some(deadline) if deadline <= r.rt.wall_start.unwrap().elapsed().as_secs_f64() => {
                if let Some(window) = &self.window { window.request_redraw(); }
                ControlFlow::Wait
            }
            Some(deadline) => ControlFlow::WaitUntil(crate::time::for_winit(r.rt.wall_start.unwrap() + Duration::from_secs_f64(deadline.max(0.0)))),
            None => ControlFlow::Wait,
        });
    }

    fn new_events(&mut self, _: &ActiveEventLoop, cause: winit::event::StartCause) {
        if matches!(cause, winit::event::StartCause::ResumeTimeReached { .. }) {
            if let Some(window) = &self.window { window.request_redraw(); }
        }
    }

    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.shared.quitting.store(true, Ordering::Release);
        let running = self.running.borrow_mut().take();
        if let Some(r) = running { r.close(); }
        if let Some((Ok((gpu, _)), _)) = self.pending.borrow_mut().take() { gpu.device.destroy(); }
        if let Some(window) = self.window.take() && self.owns_canvas && let Some(canvas) = window.canvas() { canvas.remove(); }
        std::mem::take(&mut self.borrowed).give_back();
        INSETS.set([0.0; 4]);
        LARGE_VIEWPORT.set(0.0);
        page::vacate();
    }
}
