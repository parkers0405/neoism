//! Real Servo 0.5 embedding. Native, thread-affine, intentionally not OS-sandboxed.
#[cfg(test)]
#[path = "engine_tests.rs"]
mod tests;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::{Duration, Instant};

use crate::{
    ArtifactDocument, ArtifactFrame, ArtifactInput, ButtonState, Error, PointerButton,
    PumpOutput, SandboxStatus, Theme, Viewport, WheelUnit, MAX_VIEWS, SANDBOX_STATUS,
};
use dpi::PhysicalSize;
use euclid::Scale;
use http::{header, HeaderValue};
use servo::{
    EventLoopWaker, RenderingContext, Servo, ServoBuilder, SoftwareRenderingContext,
    WebView, WebViewBuilder,
};
use url::Url;

// Servo's process-global options and SpiderMonkey initialization prohibit recreation.
const PRESENTATION_INTERVAL: Duration = Duration::from_nanos(33_333_334);
static INITIALIZED: AtomicBool = AtomicBool::new(false);
static NEXT_DOCUMENT: AtomicU64 = AtomicU64::new(1);
const CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src 'none'; font-src 'none'; media-src 'none'; connect-src 'none'; frame-src 'none'; child-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'";

#[derive(Clone)]
struct Waker(Arc<dyn Fn() + Send + Sync>);
impl EventLoopWaker for Waker {
    fn wake(&self) {
        (self.0)();
    }
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }
}

struct DocumentDelegate {
    url: Url,
    bytes: RefCell<Vec<u8>>,
    style_epoch: Cell<u64>,
    applying_style: Cell<bool>,
    evaluating_style: Cell<bool>,
    served: Cell<bool>,
    visible: Cell<bool>,
    dirty: Cell<bool>,
    crashed: Cell<bool>,
    diagnostics: Rc<RefCell<Vec<String>>>,
}
impl DocumentDelegate {
    fn note(&self, message: String) {
        // Bound diagnostic accumulation even when the parent doesn't pump regularly.
        if let Ok(mut diagnostics) = self.diagnostics.try_borrow_mut() {
            if diagnostics.len() < 64 {
                diagnostics.push(message);
            }
        }
    }
    fn serve(&self, load: servo::WebResourceLoad) {
        let request = load.request();
        let allowed = request.method == http::Method::GET
            && request.url == self.url
            && request.is_for_main_frame
            && !request.is_redirect
            && !self.served.get();
        let mut response = servo::WebResourceResponse::new(request.url.clone());
        if !allowed {
            // Interception precedes native protocol handlers too (including file/data/blob).
            // Never drop a load unhandled: that would permit the native request to proceed.
            load.intercept(response).cancel();
            return;
        }
        self.served.set(true);
        response.headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_static("text/html; charset=utf-8"),
        );
        response.headers.insert(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(CSP),
        );
        response
            .headers
            .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response.headers.insert(
            header::REFERRER_POLICY,
            HeaderValue::from_static("no-referrer"),
        );
        response.headers.insert(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        );
        let mut intercepted = load.intercept(response);
        if let Ok(bytes) = self.bytes.try_borrow() {
            intercepted.send_body_data(bytes.clone());
            intercepted.finish();
        } else {
            intercepted.cancel();
            self.crashed.set(true);
            self.note("Synthetic document bytes unavailable".into());
        }
    }
}
impl servo::WebViewDelegate for DocumentDelegate {
    fn load_web_resource(&self, _: WebView, load: servo::WebResourceLoad) {
        self.serve(load);
    }
    fn request_navigation(&self, _: WebView, request: servo::NavigationRequest) {
        // Initial synthetic navigation only; subsequent same-origin navigations are denied too.
        if request.url == self.url && !self.served.get() {
            request.allow();
        } else {
            request.deny();
        }
    }
    fn request_permission(&self, _: WebView, request: servo::PermissionRequest) {
        request.deny();
    }
    fn request_create_new(&self, _: WebView, request: servo::CreateNewWebViewRequest) {
        drop(request);
    }
    fn notify_load_status_changed(&self, webview: WebView, status: servo::LoadStatus) {
        // set_throttled targets the current pipeline. The construction-time
        // call can precede the asynchronous synthetic navigation's commit.
        // Reapply once the real document is current, using the latest desire.
        if style_target_ready(
            self.served.get(),
            webview.url().as_ref(),
            &self.url,
            status,
        ) {
            webview.set_throttled(!self.visible.get());
        }
    }
    fn notify_new_frame_ready(&self, _: WebView) {
        if !self.applying_style.get() {
            self.dirty.set(true);
        }
    }
    fn notify_crashed(&self, _: WebView, reason: String, _: Option<String>) {
        self.crashed.set(true);
        self.note(format!(
            "Servo artifact crashed: {}",
            reason.chars().take(2048).collect::<String>()
        ));
    }
    // Default controls drop deny/cancel responders, including native file picker/dialogs.
    // No console-to-host messages, custom protocols, native window moves/resizes or bridge.
}
impl servo::ClipboardDelegate for DocumentDelegate {
    fn get_text(&self, _: WebView, request: servo::StringRequest) {
        request.failure("Clipboard access is disabled for artifacts".into());
    }
}

struct EngineDelegate(Rc<RefCell<Vec<String>>>, Rc<Cell<bool>>);
impl servo::ServoDelegate for EngineDelegate {
    fn notify_error(&self, error: servo::ServoError) {
        self.1.set(true);
        if let Ok(mut diagnostics) = self.0.try_borrow_mut() {
            if diagnostics.len() < 64 {
                diagnostics.push(format!("Servo engine error: {error:?}"));
            }
        }
    }
    fn load_web_resource(&self, load: servo::WebResourceLoad) {
        // Worker/unassociated loads must not escape the view-level deny policy.
        let response = servo::WebResourceResponse::new(load.request().url.clone());
        load.intercept(response).cancel();
    }
    fn request_devtools_connection(&self, request: servo::AllowOrDenyRequest) {
        request.deny();
    }
}

struct View {
    // Drop the WebView (and its renderer) before dropping the context.
    webview: WebView,
    context: Rc<SoftwareRenderingContext>,
    delegate: Rc<DocumentDelegate>,
    document: ArtifactDocument,
    pending_styles: Option<String>,
    latest: Option<ArtifactFrame>,
    sequence: u64,
    next_presentation: Option<Instant>,
}

/// A single process-lifetime Servo engine hosting multiple independent synthetic documents.
/// Create only after explicit experimental runtime opt-in. !Send / !Sync by construction.
/// Use one Host for the whole desktop process, not one per artifact or per window.
pub struct Host {
    views: HashMap<String, View>,
    engine: Servo,
    failed: Rc<Cell<bool>>,
    diagnostics: Rc<RefCell<Vec<String>>>,
    waker: Waker,
}
impl Host {
    /// Wake can run on Servo worker threads: schedule main-thread pump; do not reenter Host.
    /// Upstream ServoBuilder can panic on native engine initialization failures; this wrapper
    /// does not pretend those failures or engine exploits can be contained in-process.
    pub fn new(waker: Arc<dyn Fn() + Send + Sync>) -> Result<Self, Error> {
        if INITIALIZED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(Error::EngineAlreadyInitialized);
        }
        // TLS should never be used by this host, but Servo's network machinery requires a
        // provider. Respect an existing host-installed provider rather than replacing it.
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let waker = Waker(waker);
        let preferences = servo::Preferences {
            devtools_server_enabled: false,
            dom_async_clipboard_enabled: false,
            dom_composition_event_enabled: true,
            dom_serviceworker_enabled: false,
            dom_sharedworker_enabled: false,
            dom_worklet_enabled: false,
            dom_navigator_protocol_handlers_enabled: false,
            dom_servo_helpers_enabled: false,
            dom_bluetooth_enabled: false,
            dom_webrtc_enabled: false,
            dom_webgpu_enabled: false,
            dom_webxr_enabled: false,
            dom_gamepad_enabled: false,
            dom_geolocation_enabled: false,
            dom_notification_enabled: false,
            dom_testutils_enabled: false,
            dom_testing_html_input_element_select_files_enabled: false,
            network_local_directory_listing_enabled: false,
            network_http_cache_disabled: true,
            ..servo::Preferences::default()
        };
        let opts = servo::Opts {
            hard_fail: false,
            multiprocess: false,
            sandbox: false,
            temporary_storage: true,
            ..servo::Opts::default()
        };
        let engine = ServoBuilder::default()
            .opts(opts)
            .preferences(preferences)
            .event_loop_waker(Box::new(waker.clone()))
            .build();
        let diagnostics = Rc::new(RefCell::new(Vec::new()));
        let failed = Rc::new(Cell::new(false));
        engine.set_delegate(Rc::new(EngineDelegate(diagnostics.clone(), failed.clone())));
        Ok(Self {
            views: HashMap::new(),
            engine,
            failed,
            diagnostics,
            waker,
        })
    }
    pub fn sandbox_status(&self) -> SandboxStatus {
        SANDBOX_STATUS
    }

    /// Creates or updates a document. New revisions replace the WebView to avoid stale
    /// frames, history and asynchronous callbacks being mislabeled with a new revision.
    pub fn reconcile(&mut self, document: ArtifactDocument) -> Result<(), Error> {
        document.validate()?;
        if let Some(view) = self.views.get_mut(&document.key) {
            if same_source(&view.document, &document)? {
                // All metadata updates preserve the live DOM/JS state. Only a new
                // source revision replaces the WebView. Style evaluation is guarded
                // against superseded callbacks and blocks readback until completion.
                if view.document.viewport != document.viewport {
                    view.webview
                        .set_hidpi_scale_factor(Scale::new(document.viewport.scale));
                    view.webview.resize(PhysicalSize::new(
                        document.viewport.width,
                        document.viewport.height,
                    ));
                    view.latest = None;
                    view.delegate.dirty.set(false);
                }
                if view.document.theme != document.theme
                    || view.document.styles != document.styles
                {
                    view.webview
                        .notify_theme_change(servo_theme(document.theme));
                    if !view.delegate.served.get() {
                        let bytes = crate::theme_script::inline_html(&document)?;
                        *view.delegate.bytes.try_borrow_mut().map_err(|_| {
                            Error::Backend("Document theme bytes busy".into())
                        })? = bytes;
                    } else {
                        view.delegate.style_epoch.set(
                            view.delegate.style_epoch.get().checked_add(1).ok_or_else(
                                || Error::Backend("Theme update epoch exhausted".into()),
                            )?,
                        );
                        view.delegate.applying_style.set(true);
                        view.delegate.evaluating_style.set(false);
                        view.delegate.dirty.set(false);
                        view.pending_styles = Some(crate::theme_script::live_script(
                            &document.styles,
                            document.theme,
                            &view.delegate.url.origin().ascii_serialization(),
                        )?);
                    }
                }
                if view.document.visible != document.visible {
                    set_visible(view, document.visible);
                }
                view.document = document;
                self.waker.wake();
                return Ok(());
            }
        } else if self.views.len() >= MAX_VIEWS {
            return Err(Error::TooManyViews);
        }
        let size = PhysicalSize::new(document.viewport.width, document.viewport.height);
        let context = Rc::new(SoftwareRenderingContext::new(size).map_err(|error| {
            Error::Backend(format!("Software rendering context: {error:?}"))
        })?);
        let serial = NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed);
        let url = Url::parse(&format!(
            "https://artifact-{serial}.neoism.invalid/document"
        ))
        .map_err(|error| Error::Backend(error.to_string()))?;
        let delegate = Rc::new(DocumentDelegate {
            url: url.clone(),
            bytes: RefCell::new(crate::theme_script::inline_html(&document)?),
            style_epoch: Cell::new(0),
            applying_style: Cell::new(false),
            evaluating_style: Cell::new(false),
            served: Cell::new(false),
            visible: Cell::new(document.visible),
            dirty: Cell::new(false),
            crashed: Cell::new(false),
            diagnostics: self.diagnostics.clone(),
        });
        let webview = WebViewBuilder::new(&self.engine, context.clone())
            .url(url)
            .hidpi_scale_factor(Scale::new(document.viewport.scale))
            .delegate(delegate.clone())
            .clipboard_delegate(delegate.clone())
            .build();
        webview.notify_theme_change(servo_theme(document.theme));
        // Every view has its own offscreen context. Host visibility gates
        // readback/input scheduling, not membership in Servo's root scene.
        // Removing/reinserting that iframe asynchronously creates empty scenes
        // whose generic frame-ready notifications cannot be tied to activation.
        webview.show();
        // Throttle script timers and Servo's internal animation refresh driver,
        // without removing the retained iframe from its offscreen scene.
        webview.set_throttled(!document.visible);
        self.views.insert(
            document.key.clone(),
            View {
                webview,
                context,
                delegate,
                document,
                pending_styles: None,
                latest: None,
                sequence: 0,
                next_presentation: None,
            },
        );
        self.waker.wake();
        Ok(())
    }
    pub fn resize(&mut self, key: &str, viewport: Viewport) -> Result<(), Error> {
        viewport.validate()?;
        let mut document = self
            .views
            .get(key)
            .ok_or(Error::UnknownArtifact)?
            .document
            .clone();
        document.viewport = viewport;
        self.reconcile(document)
    }
    pub fn input(&mut self, key: &str, input: ArtifactInput) -> Result<(), Error> {
        input.validate()?;
        let view = self.views.get(key).ok_or(Error::UnknownArtifact)?;
        if view.delegate.crashed.get() {
            return Err(Error::Backend("Artifact has crashed".into()));
        }
        let event = match input {
            ArtifactInput::Focus(focused) => {
                if focused {
                    view.webview.focus();
                } else {
                    view.webview.blur();
                }
                self.waker.wake();
                return Ok(());
            }
            ArtifactInput::PointerMove { x, y } => {
                servo::InputEvent::MouseMove(servo::MouseMoveEvent::new(point(x, y)))
            }
            ArtifactInput::PointerButton {
                x,
                y,
                button,
                state,
            } => {
                if state == ButtonState::Down {
                    view.webview.focus();
                }
                let action = match state {
                    ButtonState::Down => servo::MouseButtonAction::Down,
                    ButtonState::Up => servo::MouseButtonAction::Up,
                };
                let button = match button {
                    PointerButton::Left => servo::MouseButton::Left,
                    PointerButton::Middle => servo::MouseButton::Middle,
                    PointerButton::Right => servo::MouseButton::Right,
                    PointerButton::Back => servo::MouseButton::Back,
                    PointerButton::Forward => servo::MouseButton::Forward,
                };
                servo::InputEvent::MouseButton(servo::MouseButtonEvent::new(
                    action,
                    button,
                    point(x, y),
                ))
            }
            ArtifactInput::PointerLeave => {
                servo::InputEvent::MouseLeftViewport(Default::default())
            }
            ArtifactInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                unit,
            } => {
                // Servo's embedder deltas have the opposite sign to DOM/winit deltas.
                let mode = match unit {
                    WheelUnit::Pixel => servo::WheelMode::DeltaPixel,
                    WheelUnit::Line => servo::WheelMode::DeltaLine,
                    WheelUnit::Page => servo::WheelMode::DeltaPage,
                };
                servo::InputEvent::Wheel(servo::WheelEvent::new(
                    servo::WheelDelta {
                        x: -delta_x,
                        y: -delta_y,
                        z: 0.0,
                        mode,
                    },
                    point(x, y),
                ))
            }
            ArtifactInput::Key(event) => {
                servo::InputEvent::Keyboard(servo::KeyboardEvent::new(event))
            }
            ArtifactInput::Ime(event) => {
                servo::InputEvent::Ime(servo::ImeEvent::Composition(event))
            }
            ArtifactInput::ImeDismissed => {
                servo::InputEvent::Ime(servo::ImeEvent::Dismissed)
            }
        };
        view.webview.notify_input_event(event);
        self.waker.wake();
        Ok(())
    }
    /// One nonblocking event-loop turn plus CPU readback of dirty visible frames.
    /// Painting/readback itself is synchronous and may be expensive. No implicit timer.
    pub fn pump(&mut self) -> Result<PumpOutput, Error> {
        self.engine.spin_event_loop();
        if self.failed.get()
            || self.views.values().any(|view| view.delegate.crashed.get())
        {
            return Err(Error::Backend("Servo artifact engine crashed".into()));
        }
        let mut output = PumpOutput::default();
        for (key, view) in &mut self.views {
            let current_url = view.webview.url();
            if style_target_ready(
                view.delegate.served.get(),
                current_url.as_ref(),
                &view.delegate.url,
                view.webview.load_status(),
            ) {
                if view.delegate.applying_style.get()
                    && !view.delegate.evaluating_style.get()
                    && view.pending_styles.is_none()
                {
                    view.pending_styles = Some(crate::theme_script::live_script(
                        &view.document.styles,
                        view.document.theme,
                        &view.delegate.url.origin().ascii_serialization(),
                    )?);
                }
                if let Some(script) = view.pending_styles.take() {
                    let delegate = view.delegate.clone();
                    delegate.evaluating_style.set(true);
                    let epoch = delegate.style_epoch.get();
                    view.webview.evaluate_javascript(script, move |result| {
                        if delegate.style_epoch.get() != epoch {
                            return;
                        }
                        delegate.evaluating_style.set(false);
                        match result {
                            Ok(servo::JSValue::Boolean(true)) => {
                                delegate.applying_style.set(false);
                                delegate.dirty.set(true);
                            }
                            Ok(_) => (), // Not the committed root yet: retain desire and retry.
                            Err(error) => {
                                delegate.note(format!(
                                    "Live artifact theme failed: {error:?}"
                                ));
                                delegate.crashed.set(true);
                            }
                        }
                    });
                }
            }
            if !view.document.visible || view.delegate.crashed.get() {
                continue;
            }
            let animating = view.webview.animating();
            output.animating |= animating;
            if !animating && !view.delegate.dirty.get() {
                // Fully idle: there is no retained animation frame to pace.
                view.next_presentation = None;
            }
            if view.delegate.applying_style.get()
                || !view.delegate.dirty.get()
                || !style_target_ready(
                    view.delegate.served.get(),
                    current_url.as_ref(),
                    &view.delegate.url,
                    view.webview.load_status(),
                )
                || view
                    .next_presentation
                    .is_some_and(|deadline| Instant::now() < deadline)
            {
                // Retain dirty until its presentation deadline, including the
                // last animation frame and one-shot static input changes.
                continue;
            }
            view.context
                .make_current()
                .map_err(|error| Error::Backend(format!("Make current: {error:?}")))?;
            view.webview.paint();
            let viewport = view.document.viewport;
            let rect = servo::DeviceIntRect::from_origin_and_size(
                servo::DeviceIntPoint::new(0, 0),
                servo::DeviceIntSize::new(viewport.width as i32, viewport.height as i32),
            );
            let image = view
                .context
                .read_to_image(rect)
                .ok_or_else(|| Error::Backend("Servo RGBA readback failed".into()))?;
            if image.width() != viewport.width || image.height() != viewport.height {
                return Err(Error::Backend("Servo readback size mismatch".into()));
            }
            view.context.present();
            view.delegate.dirty.set(false);
            // Continuous animations are capped; once their final retained
            // frame is presented, static input/load work need not pay a timer.
            view.next_presentation =
                animating.then(|| Instant::now() + PRESENTATION_INTERVAL);
            view.sequence = view.sequence.saturating_add(1);
            let frame = ArtifactFrame {
                key: key.clone(),
                revision: view.document.revision,
                sequence: view.sequence,
                width: viewport.width,
                height: viewport.height,
                stride: viewport.width as usize * 4,
                rgba: image.into_raw().into(),
            };
            view.latest = Some(frame.clone());
            output.frames.push(frame);
        }
        if let Ok(mut diagnostics) = self.diagnostics.try_borrow_mut() {
            output.diagnostics = std::mem::take(&mut *diagnostics);
        }
        Ok(output)
    }
    /// Next retained dirty presentation, even if Servo stops animating and never
    /// wakes again. The worker must include this deadline in its idle receive.
    /// Style/load-blocked views rely on engine wakes, not a zero-delay busy loop.
    pub fn pending_presentation_delay(&self) -> Option<Duration> {
        let now = Instant::now();
        self.views
            .values()
            .filter_map(|view| {
                if !view.document.visible
                    || !view.delegate.dirty.get()
                    || view.delegate.applying_style.get()
                    || view.delegate.crashed.get()
                    || !style_target_ready(
                        view.delegate.served.get(),
                        view.webview.url().as_ref(),
                        &view.delegate.url,
                        view.webview.load_status(),
                    )
                {
                    return None;
                }
                Some(view.next_presentation.map_or(Duration::ZERO, |deadline| {
                    deadline.saturating_duration_since(now)
                }))
            })
            .min()
    }
    pub fn snapshot(&self, key: &str) -> Option<ArtifactFrame> {
        self.views.get(key).and_then(|view| view.latest.clone())
    }
    pub fn destroy(&mut self, key: &str) -> bool {
        let removed = self.views.remove(key).is_some();
        if removed {
            self.waker.wake();
        }
        removed
    }
    /// Drops all WebViews, then Servo. Upstream teardown drains its event loop and may
    /// block. A new Host cannot be created later in the same process.
    pub fn shutdown(mut self) {
        self.views.clear();
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.views.clear();
    }
}
fn style_target_ready(
    served: bool,
    current: Option<&Url>,
    expected: &Url,
    status: servo::LoadStatus,
) -> bool {
    served
        && status == servo::LoadStatus::Complete
        && current.is_some_and(|url| url.origin() == expected.origin())
}
fn same_source(
    previous: &ArtifactDocument,
    next: &ArtifactDocument,
) -> Result<bool, Error> {
    if previous.revision == next.revision && previous.html != next.html {
        return Err(Error::RevisionConflict);
    }
    Ok(previous.revision == next.revision)
}
fn point(x: f32, y: f32) -> servo::WebViewPoint {
    servo::DevicePoint::new(x, y).into()
}
fn servo_theme(theme: Theme) -> servo::Theme {
    match theme {
        Theme::Light => servo::Theme::Light,
        Theme::Dark => servo::Theme::Dark,
    }
}
fn set_visible(view: &mut View, visible: bool) {
    // Keep the iframe resident: hide/show can asynchronously publish an empty
    // scene on resume. Throttling instead stops Servo's internal refresh driver
    // and limits JS timers, independently of our CPU presentation gate.
    view.delegate.visible.set(visible);
    view.webview.set_throttled(!visible);
    if visible {
        // Repaint the retained scene, including any changes made while hidden.
        // Style/resize readiness remains guarded independently by pump().
        view.delegate.dirty.set(true);
    } else if view.webview.focused() {
        view.webview.blur();
    }
}
