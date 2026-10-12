//! CEF UI-thread-owned browsers with per-incarnation callback state.
use crate::theme;
use cef::{rc::Rc, *};
use neoism_interactive_artifacts::{
    self as contract, ArtifactDocument, ArtifactInput, ButtonState, Key, KeyState,
    NamedKey, PointerButton, Viewport, WheelUnit,
};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

type Shared = Arc<Mutex<State>>;
struct State {
    viewport: Viewport,
    visible: bool,
    loaded: bool,
    closed: bool,
    epoch: u64,
    ready: bool,
    theme_script: String,
    content_height: Option<f32>,
    last_height: Option<f32>,
    pixels: Option<Vec<u8>>,
    base: Option<Vec<u8>>,
    popup: Popup,
    buttons: u32,
    keymods: u32,
}
#[derive(Default)]
struct Popup {
    shown: bool,
    rect: Rect,
    pixels: Option<Vec<u8>>,
}
impl State {
    fn publish(&mut self) {
        if !self.ready || !self.visible || self.closed {
            return;
        }
        let Some(base) = &self.base else {
            return;
        };
        let mut output = base.clone();
        if self.popup.shown {
            if let Some(pixels) = &self.popup.pixels {
                composite(
                    &mut output,
                    self.viewport.width,
                    self.viewport.height,
                    pixels,
                    &self.popup.rect,
                );
            }
        }
        self.pixels = Some(output);
    }
}
// Both buffers are premultiplied RGBA. Clip the popup to physical VIEW bounds,
// including negative origins; never destructively paint over the retained base.
fn composite(view: &mut [u8], width: u32, height: u32, popup: &[u8], rect: &Rect) {
    if rect.width <= 0 || rect.height <= 0 {
        return;
    }
    let left = (rect.x as i64).max(0);
    let top = (rect.y as i64).max(0);
    let right = (rect.x as i64 + rect.width as i64).min(width as i64);
    let bottom = (rect.y as i64 + rect.height as i64).min(height as i64);
    for y in top..bottom {
        for x in left..right {
            let src = ((y - rect.y as i64) * rect.width as i64 + x - rect.x as i64)
                as usize
                * 4;
            let dst = (y as usize * width as usize + x as usize) * 4;
            if src + 4 > popup.len() || dst + 4 > view.len() {
                continue;
            }
            let inv = 255 - popup[src + 3] as u32;
            for channel in 0..4 {
                view[dst + channel] = (popup[src + channel] as u32
                    + (view[dst + channel] as u32 * inv + 127) / 255)
                    .min(255) as u8;
            }
        }
    }
}
wrap_render_handler! {
    struct Paint { state: Shared }
    impl RenderHandler {
        fn view_rect(&self, _unused1: Option<&mut Browser>, rect: Option<&mut Rect>) {
            let s=self.state.lock().unwrap();
            if let Some(r)=rect {r.x=0;r.y=0;r.width=s.viewport.width as i32;r.height=s.viewport.height as i32;}
        }
        fn screen_info(&self, _unused1: Option<&mut Browser>, info: Option<&mut ScreenInfo>) -> i32 {
            if let Some(i)=info {let v=self.state.lock().unwrap().viewport;i.device_scale_factor=1.;i.depth=32;i.depth_per_component=8;i.rect=Rect{x:0,y:0,width:v.width as i32,height:v.height as i32};i.available_rect=i.rect.clone();return 1;} 0
        }
        fn on_popup_show(&self,browser:Option<&mut Browser>,show:i32) {
            let mut s=self.state.lock().unwrap();
            if !s.ready || !s.visible || s.closed {return;}
            s.popup=Popup{shown:show!=0,..Default::default()};
            s.publish();
            drop(s);
            if show==0 {if let Some(h)=browser.and_then(|b|b.host()){h.invalidate(PaintElementType::VIEW);}}
        }
        fn on_popup_size(&self,_browser:Option<&mut Browser>,rect:Option<&Rect>) {
            let mut s=self.state.lock().unwrap();
            if !s.ready || !s.popup.shown || s.closed {return;}
            if let Some(r)=rect {s.popup.rect=r.clone();s.popup.pixels=None;s.publish();}
        }
        fn on_paint(&self, _unused1: Option<&mut Browser>, kind: PaintElementType, _dirty: Option<&[Rect]>, buffer: *const u8, width:i32,height:i32) {
            let mut s=self.state.lock().unwrap();
            if buffer.is_null() || !s.visible || !s.ready || s.closed || width<=0 || height<=0 {return;}
            if kind==PaintElementType::VIEW {
                if width!=s.viewport.width as i32 || height!=s.viewport.height as i32 {return;}
            } else if kind==PaintElementType::POPUP {
                if !s.popup.shown || width!=s.popup.rect.width || height!=s.popup.rect.height {return;}
            } else {return;}
            let Some(n)=(width as usize).checked_mul(height as usize).and_then(|n|n.checked_mul(4)) else {return;};
            if n>contract::MAX_FRAME_BYTES {return;}
            // CEF's OSR buffer is BGRA premultiplied, top-down. Keep alpha intact.
            let mut bytes=unsafe {std::slice::from_raw_parts(buffer,n)}.to_vec();
            for p in bytes.chunks_exact_mut(4) {p.swap(0,2);}
            if kind==PaintElementType::VIEW {s.base=Some(bytes);} else {s.popup.pixels=Some(bytes);}
            s.publish();
        }
    }
}
wrap_load_handler! {
    struct Load { state: Shared }
    impl LoadHandler {
        fn on_load_end(&self, browser:Option<&mut Browser>, frame:Option<&mut Frame>, _unused1:i32) {
            if frame.is_some_and(|f|f.is_main()==1) {
                self.state.lock().unwrap().loaded=true;
                if let Some(b)=browser {
                    let script=self.state.lock().unwrap().theme_script.clone();
                    if let Some(f)=b.main_frame(){f.execute_java_script(Some(&script.as_str().into()),Some(&"neoism:host".into()),1);}
                    arm(b,&self.state);
                }
            }
        }
    }
}
// Readiness is a renderer round-trip after layout and two compositor turns, not
// a timeout or a heuristic rejecting white content. Each transaction invalidates
// retained pixels before any CEF calls. Old browser callbacks have separate state.
fn arm(browser: &Browser, state: &Shared) {
    let epoch = state.lock().unwrap().epoch;
    if let Some(f) = browser.main_frame() {
        let script =
            include_str!("content_height.js").replace("__EPOCH__", &epoch.to_string());
        f.execute_java_script(
            Some(&script.as_str().into()),
            Some(&"neoism:host".into()),
            1,
        );
    }
}
// Bounded numeric-only readiness channel. Author console text is never forwarded.
fn parse_height(message: &str) -> Option<(bool, u64, f32)> {
    if message.len() > 128 {
        return None;
    }
    let (initial, value) = if let Some(v) = message.strip_prefix("neoism-ready:") {
        (true, v)
    } else {
        (false, message.strip_prefix("neoism-height:")?)
    };
    let (epoch, height) = value.split_once(':')?;
    let epoch = epoch.parse().ok()?;
    let height: f32 = height.parse().ok()?;
    (height.is_finite() && height > 0.0 && height <= 8192.0)
        .then_some((initial, epoch, height))
}
wrap_display_handler! {
    struct Display { state: Shared }
    impl DisplayHandler {
        fn on_console_message(&self,browser:Option<&mut Browser>,_level:LogSeverity,message:Option<&CefString>,source:Option<&CefString>,_unused1:i32)->i32 {
            if source.is_some_and(|s|s.to_string()=="neoism:host") {
                if let Some((initial, epoch, height))=message.and_then(|m|parse_height(&m.to_string())) {
                    let mut s=self.state.lock().unwrap();
                    let accepted=epoch==s.epoch && s.loaded && s.visible && !s.closed && (initial || s.ready);
                    if accepted {
                        if initial {s.ready=true;}
                        if s.last_height!=Some(height) {
                            s.last_height=Some(height);
                            s.content_height=Some(height);
                        }
                    }
                    drop(s);
                    if accepted && initial {
                        if let Some(h)=browser.and_then(|b|b.host()) {h.invalidate(PaintElementType::VIEW);}
                    }
                }
            }
            1
        }
    }
}
wrap_resource_request_handler! {
    struct Resources { initial: String }
    impl ResourceRequestHandler {
        fn on_before_resource_load(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,request:Option<&mut Request>,_unused1:Option<&mut Callback>)->ReturnValue {
            if request.is_some_and(|r|CefString::from(&r.method()).to_string()=="GET" && (CefString::from(&r.url()).to_string()==self.initial || theme::allowed_resource(&CefString::from(&r.url()).to_string()))) {ReturnValue::CONTINUE} else {ReturnValue::CANCEL}
        }
        fn on_resource_redirect(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_unused1:Option<&mut Request>,_response:Option<&mut Response>,url:Option<&mut CefString>) {
            if let Some(u)=url {if !theme::allowed_resource(&u.to_string()) {*u="data:,denied".into();}}
        }
    }
}
wrap_request_handler! {
    struct Requests { initial: String, started: Arc<Mutex<bool>> }
    impl RequestHandler {
        fn on_before_browse(&self,_browser:Option<&mut Browser>,frame:Option<&mut Frame>,request:Option<&mut Request>,_gesture:i32,_redirect:i32)->i32 {
            let mut started=self.started.lock().unwrap();
            if !*started && frame.is_some_and(|f|f.is_main()==1) && request.is_some_and(|r|CefString::from(&r.url()).to_string()==self.initial) {*started=true;0} else {1}
        }
        fn on_open_urlfrom_tab(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_url:Option<&CefString>,_disposition:WindowOpenDisposition,_unused1:i32)->i32 {1}
        fn resource_request_handler(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_unused1:Option<&mut Request>,_gesture:i32,_redirect:i32,_url:Option<&CefString>,_disable:Option<&mut i32>)->Option<ResourceRequestHandler> {Some(Resources::new(self.initial.clone()))}
    }
}
wrap_life_span_handler! {
    struct Life { state: Shared }
    impl LifeSpanHandler {
        fn on_before_popup(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_id:i32,_url:Option<&CefString>,_name:Option<&CefString>,_disposition:WindowOpenDisposition,_gesture:i32,_features:Option<&PopupFeatures>,_window:Option<&mut WindowInfo>,_client:Option<&mut Option<Client>>,_settings:Option<&mut BrowserSettings>,_extra:Option<&mut Option<DictionaryValue>>,_disable:Option<&mut i32>)->i32 {1}
        fn on_before_close(&self,_browser:Option<&mut Browser>) {self.state.lock().unwrap().closed=true;}
    }
}
wrap_permission_handler! {
    struct Permissions {}
    impl PermissionHandler {
        fn on_request_media_access_permission(&self,_browser:Option<&mut Browser>,_frame:Option<&mut Frame>,_url:Option<&CefString>,_unused1:u32,cb:Option<&mut MediaAccessCallback>)->i32 {if let Some(c)=cb {c.cont(0);}1}
        fn on_show_permission_prompt(&self,_browser:Option<&mut Browser>,_prompt_id:u64,_url:Option<&CefString>,_unused1:u32,cb:Option<&mut PermissionPromptCallback>)->i32 {if let Some(c)=cb {c.cont(PermissionRequestResult::DENY);}1}
    }
}
wrap_download_handler! { struct Downloads {} impl DownloadHandler {
    fn can_download(&self,_browser:Option<&mut Browser>,_url:Option<&CefString>,_method:Option<&CefString>)->i32 {0}
    fn on_before_download(&self,_browser:Option<&mut Browser>,_unused1:Option<&mut DownloadItem>,_url:Option<&CefString>,_cb:Option<&mut BeforeDownloadCallback>)->i32 {1}
} }
wrap_dialog_handler! {
    struct FileDialogs {}
    impl DialogHandler {
        fn on_file_dialog(&self,_browser:Option<&mut Browser>,_mode:FileDialogMode,_title:Option<&CefString>,_path:Option<&CefString>,_filters:Option<&mut CefStringList>,_extensions:Option<&mut CefStringList>,_descriptions:Option<&mut CefStringList>,cb:Option<&mut FileDialogCallback>)->i32 {if let Some(c)=cb{c.cancel();}1}
    }
}
wrap_jsdialog_handler! {
    struct JsDialogs {}
    impl JsdialogHandler {
        fn on_jsdialog(&self,_browser:Option<&mut Browser>,_origin:Option<&CefString>,_kind:JsdialogType,_message:Option<&CefString>,_default:Option<&CefString>,_cb:Option<&mut JsdialogCallback>,suppress:Option<&mut i32>)->i32 {if let Some(s)=suppress{*s=1;}0}
        fn on_before_unload_dialog(&self,_browser:Option<&mut Browser>,_message:Option<&CefString>,_reload:i32,cb:Option<&mut JsdialogCallback>)->i32 {if let Some(c)=cb{c.cont(1,None);}1}
    }
}
wrap_client! {
    struct ArtifactClient { render:RenderHandler,load:LoadHandler,display:DisplayHandler,request:RequestHandler,life:LifeSpanHandler,permissions:PermissionHandler,downloads:DownloadHandler,files:DialogHandler,dialogs:JsdialogHandler }
    impl Client {
        fn dialog_handler(&self)->Option<DialogHandler>{Some(self.files.clone())}
        fn jsdialog_handler(&self)->Option<JsdialogHandler>{Some(self.dialogs.clone())}
        fn render_handler(&self)->Option<RenderHandler>{Some(self.render.clone())}
        fn load_handler(&self)->Option<LoadHandler>{Some(self.load.clone())}
        fn display_handler(&self)->Option<DisplayHandler>{Some(self.display.clone())}
        fn request_handler(&self)->Option<RequestHandler>{Some(self.request.clone())}
        fn life_span_handler(&self)->Option<LifeSpanHandler>{Some(self.life.clone())}
        fn permission_handler(&self)->Option<PermissionHandler>{Some(self.permissions.clone())}
        fn download_handler(&self)->Option<DownloadHandler>{Some(self.downloads.clone())}
    }
}
struct View {
    document: ArtifactDocument,
    generation: u64,
    browser: Browser,
    state: Shared,
}
pub struct Host {
    views: HashMap<String, View>,
    closing: Vec<Shared>,
}
impl Host {
    pub fn new() -> Self {
        Self {
            views: HashMap::new(),
            closing: Vec::new(),
        }
    }
    pub fn visible(&self) -> bool {
        self.views.values().any(|v| v.document.visible)
    }
    pub fn reconcile(
        &mut self,
        d: ArtifactDocument,
        generation: u64,
    ) -> Result<(), String> {
        d.validate().map_err(|e| e.to_string())?;
        if let Some(v) = self.views.get(&d.key) {
            if v.document.revision == d.revision && v.document.html != d.html {
                return Err("RevisionConflict".into());
            }
            if v.document.revision != d.revision {
                self.destroy(&d.key);
            }
        }
        if let Some(v) = self.views.get_mut(&d.key) {
            transaction(v, &d, generation)?;
            v.document = d;
            return Ok(());
        }
        if self.views.len() >= contract::MAX_VIEWS {
            return Err("TooManyViews".into());
        }
        let url = theme::document_url(&d)?;
        let state = Arc::new(Mutex::new(State {
            viewport: d.viewport,
            visible: d.visible,
            loaded: false,
            closed: false,
            epoch: 1,
            ready: false,
            theme_script: theme::theme_script(&d)?,
            content_height: None,
            last_height: None,
            pixels: None,
            base: None,
            popup: Popup::default(),
            buttons: 0,
            keymods: 0,
        }));
        let mut client = ArtifactClient::new(
            Paint::new(state.clone()),
            Load::new(state.clone()),
            Display::new(state.clone()),
            Requests::new(url.clone(), Arc::new(Mutex::new(false))),
            Life::new(state.clone()),
            Permissions::new(),
            Downloads::new(),
            FileDialogs::new(),
            JsDialogs::new(),
        );
        let settings = BrowserSettings {
            windowless_frame_rate: 60,
            background_color: 0,
            ..Default::default()
        };
        let window = WindowInfo {
            windowless_rendering_enabled: 1,
            ..Default::default()
        };
        let mut context = request_context_create_context(
            Some(&RequestContextSettings::default()),
            None,
        )
        .ok_or("Request context creation failed")?;
        let browser = browser_host_create_browser_sync(
            Some(&window),
            Some(&mut client),
            Some(&url.as_str().into()),
            Some(&settings),
            None,
            Some(&mut context),
        )
        .ok_or("Browser creation failed")?;
        if let Some(h) = browser.host() {
            h.set_zoom_level((d.viewport.scale as f64).ln() / 1.2f64.ln());
            h.was_hidden((!d.visible) as i32);
        }
        self.views.insert(
            d.key.clone(),
            View {
                document: d,
                generation,
                browser,
                state,
            },
        );
        Ok(())
    }
    pub fn resize(
        &mut self,
        key: &str,
        viewport: Viewport,
        generation: u64,
    ) -> Result<(), String> {
        let v = self.views.get_mut(key).ok_or("UnknownArtifact")?;
        let mut d = v.document.clone();
        d.viewport = viewport;
        transaction(v, &d, generation)?;
        v.document = d;
        Ok(())
    }
    pub fn input(&self, key: &str, input: ArtifactInput) -> Result<(), String> {
        let v = self.views.get(key).ok_or("UnknownArtifact")?;
        if !v.document.visible {
            return Ok(());
        }
        let h = v.browser.host().ok_or("Browser has no host")?;
        let scale = v.document.viewport.scale;
        let mouse = |x: f32, y: f32| MouseEvent {
            x: x as i32,
            y: y as i32,
            modifiers: {
                let s = v.state.lock().unwrap();
                s.buttons | s.keymods
            },
        };
        match input {
            ArtifactInput::PointerMove { x, y } => {
                h.send_mouse_move_event(Some(&mouse(x, y)), 0)
            }
            ArtifactInput::PointerLeave => {
                h.send_mouse_move_event(Some(&mouse(0., 0.)), 1)
            }
            ArtifactInput::PointerButton {
                x,
                y,
                button,
                state,
            } => {
                let (button, flag) = match button {
                    PointerButton::Left => (MouseButtonType::LEFT, 16),
                    PointerButton::Middle => (MouseButtonType::MIDDLE, 32),
                    PointerButton::Right => (MouseButtonType::RIGHT, 64),
                    _ => return Ok(()), // Browser history buttons do not navigate artifacts.
                };
                {
                    let mut s = v.state.lock().unwrap();
                    if state == ButtonState::Down {
                        s.buttons |= flag;
                    } else {
                        s.buttons &= !flag;
                    }
                }
                h.set_focus(1);
                h.send_mouse_click_event(
                    Some(&mouse(x, y)),
                    button,
                    (state == ButtonState::Up) as i32,
                    1,
                );
            }
            ArtifactInput::Wheel {
                x,
                y,
                delta_x,
                delta_y,
                unit,
            } => {
                let factor = match unit {
                    WheelUnit::Pixel => 1.0,
                    WheelUnit::Line => 40. * scale as f64,
                    WheelUnit::Page => v.document.viewport.height as f64,
                };
                h.send_mouse_wheel_event(
                    Some(&mouse(x, y)),
                    (-delta_x * factor) as i32,
                    (-delta_y * factor) as i32,
                );
            }
            ArtifactInput::Focus(f) => {
                if !f {
                    let mut s = v.state.lock().unwrap();
                    s.buttons = 0;
                    s.keymods = 0;
                }
                h.set_focus(f as i32);
            }
            ArtifactInput::Key(k) => {
                let vk = virtual_key(k.code, &k.key);
                let text = match &k.key {
                    Key::Character(t) => t.clone(),
                    Key::Named(NamedKey::Enter) => "\r".into(),
                    _ => String::new(),
                };
                let modifiers = key_modifiers(k.modifiers);
                v.state.lock().unwrap().keymods = modifiers;
                let ev = KeyEvent {
                    type_: if k.state == KeyState::Down {
                        KeyEventType::RAWKEYDOWN
                    } else {
                        KeyEventType::KEYUP
                    },
                    windows_key_code: vk,
                    modifiers,
                    ..Default::default()
                };
                h.send_key_event(Some(&ev));
                if k.state == KeyState::Down
                    && !k.is_composing
                    && !text.is_empty()
                    && !k.modifiers.intersects(
                        contract::Modifiers::CONTROL | contract::Modifiers::META,
                    )
                {
                    for c in text.encode_utf16() {
                        let ev = KeyEvent {
                            type_: KeyEventType::CHAR,
                            windows_key_code: c as i32,
                            character: c,
                            unmodified_character: c,
                            modifiers,
                            ..Default::default()
                        };
                        h.send_key_event(Some(&ev));
                    }
                }
            }
            ArtifactInput::Ime(event) => {
                let range = Range {
                    from: u32::MAX,
                    to: u32::MAX,
                };
                if event.state == contract::CompositionState::End {
                    h.ime_commit_text(Some(&event.data.as_str().into()), Some(&range), 0);
                } else {
                    let selection = Range {
                        from: event.data.encode_utf16().count() as u32,
                        to: event.data.encode_utf16().count() as u32,
                    };
                    h.ime_set_composition(
                        Some(&event.data.as_str().into()),
                        None,
                        Some(&range),
                        Some(&selection),
                    );
                }
            }
            ArtifactInput::ImeDismissed => h.ime_cancel_composition(),
        }
        Ok(())
    }
    pub fn layout_metrics(&mut self) -> Vec<(String, u64, u64, f32)> {
        self.views
            .iter()
            .filter_map(|(key, v)| {
                let mut s = v.state.lock().unwrap();
                if !s.ready || !s.visible || s.closed {
                    return None;
                }
                s.content_height.take().map(|height| {
                    (key.clone(), v.generation, v.document.revision, height)
                })
            })
            .collect()
    }
    pub fn frames(&mut self) -> Vec<(String, u64, u64, Viewport, Vec<u8>)> {
        self.closing.retain(|s| !s.lock().unwrap().closed);
        self.views
            .iter()
            .filter_map(|(key, v)| {
                let mut s = v.state.lock().unwrap();
                s.pixels.take().map(|p| {
                    (
                        key.clone(),
                        v.generation,
                        v.document.revision,
                        v.document.viewport,
                        p,
                    )
                })
            })
            .collect()
    }
    pub fn destroy(&mut self, key: &str) {
        if let Some(v) = self.views.remove(key) {
            let mut s = v.state.lock().unwrap();
            s.visible = false;
            s.ready = false;
            s.content_height = None;
            s.last_height = None;
            s.pixels = None;
            drop(s);
            if let Some(h) = v.browser.host() {
                h.close_browser(1);
            }
            self.closing.push(v.state);
        }
    }
    pub fn shutdown(&mut self) -> Result<(), String> {
        let keys: Vec<_> = self.views.keys().cloned().collect();
        for k in keys {
            self.destroy(&k);
        }
        let start = Instant::now();
        while !self.closing.is_empty() {
            do_message_loop_work();
            self.closing.retain(|s| !s.lock().unwrap().closed);
            if start.elapsed() > Duration::from_secs(10) {
                return Err("CEF browser close timeout".into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Ok(())
    }
}
fn transaction(
    v: &mut View,
    d: &ArtifactDocument,
    generation: u64,
) -> Result<(), String> {
    let script = theme::theme_script(d)?;
    let popup_was_open;
    {
        let mut s = v.state.lock().unwrap();
        popup_was_open = s.popup.shown;
        s.popup = Popup::default();
        s.base = None;
        s.epoch = s.epoch.checked_add(1).ok_or("Epoch exhausted")?;
        s.ready = false;
        s.content_height = None;
        s.last_height = None;
        s.pixels = None;
        s.viewport = d.viewport;
        s.visible = d.visible;
        if !d.visible {
            s.buttons = 0;
            s.keymods = 0;
        }
        s.theme_script = script.clone();
    }
    v.generation = generation;
    if let Some(h) = v.browser.host() {
        // Close a native popup before the new generation's readiness barrier;
        // late POPUP paints cannot repopulate a cleared, not-shown popup.
        if popup_was_open {
            for kind in [KeyEventType::RAWKEYDOWN, KeyEventType::KEYUP] {
                h.send_key_event(Some(&KeyEvent {
                    type_: kind,
                    windows_key_code: 27,
                    ..Default::default()
                }));
            }
        }
        h.set_zoom_level((d.viewport.scale as f64).ln() / 1.2f64.ln());
        h.was_hidden((!d.visible) as i32);
        if d.viewport != v.document.viewport {
            h.notify_screen_info_changed();
            h.was_resized();
        }
    }
    if let Some(f) = v.browser.main_frame() {
        f.execute_java_script(
            Some(&script.as_str().into()),
            Some(&"neoism:host".into()),
            1,
        );
    }
    if v.state.lock().unwrap().loaded {
        arm(&v.browser, &v.state);
    }
    Ok(())
}
// Chromium expects Windows virtual-key codes even on Linux. Physical Code
// supplies stable shortcut/OEM punctuation identity; Key supplies actual text.
fn virtual_key(code: contract::Code, key: &Key) -> i32 {
    let code = format!("{code:?}");
    if let Some(letter) = code.strip_prefix("Key").filter(|v| v.len() == 1) {
        return letter.as_bytes()[0] as i32;
    }
    if let Some(digit) = code.strip_prefix("Digit").filter(|v| v.len() == 1) {
        return digit.as_bytes()[0] as i32;
    }
    if let Some(digit) = code
        .strip_prefix("Numpad")
        .filter(|v| v.len() == 1 && v.as_bytes()[0].is_ascii_digit())
    {
        return 96 + (digit.as_bytes()[0] - b'0') as i32;
    }
    if let Some(f) = code
        .strip_prefix('F')
        .and_then(|v| v.parse::<i32>().ok())
        .filter(|v| (1..=24).contains(v))
    {
        return 111 + f;
    }
    let vk = match code.as_str() {
        "Enter" | "NumpadEnter" => 13,
        "Tab" => 9,
        "Backspace" => 8,
        "Escape" => 27,
        "Delete" => 46,
        "Insert" => 45,
        "ArrowLeft" => 37,
        "ArrowUp" => 38,
        "ArrowRight" => 39,
        "ArrowDown" => 40,
        "Home" => 36,
        "End" => 35,
        "PageUp" => 33,
        "PageDown" => 34,
        "Space" => 32,
        "ShiftLeft" | "ShiftRight" => 16,
        "ControlLeft" | "ControlRight" => 17,
        "AltLeft" | "AltRight" => 18,
        "MetaLeft" => 91,
        "MetaRight" => 92,
        "CapsLock" => 20,
        "NumLock" => 144,
        "ScrollLock" => 145,
        "ContextMenu" => 93,
        "Semicolon" => 186,
        "Equal" => 187,
        "Comma" => 188,
        "Minus" => 189,
        "Period" => 190,
        "Slash" => 191,
        "Backquote" => 192,
        "BracketLeft" => 219,
        "Backslash" | "IntlBackslash" => 220,
        "BracketRight" => 221,
        "Quote" => 222,
        "NumpadMultiply" => 106,
        "NumpadAdd" => 107,
        "NumpadSubtract" => 109,
        "NumpadDecimal" => 110,
        "NumpadDivide" => 111,
        _ => 0,
    };
    if vk != 0 {
        return vk;
    }
    match key {
        Key::Named(n) => match n {
            NamedKey::Enter => 13,
            NamedKey::Tab => 9,
            NamedKey::Backspace => 8,
            NamedKey::Escape => 27,
            NamedKey::Delete => 46,
            NamedKey::Insert => 45,
            NamedKey::ArrowLeft => 37,
            NamedKey::ArrowUp => 38,
            NamedKey::ArrowRight => 39,
            NamedKey::ArrowDown => 40,
            NamedKey::Home => 36,
            NamedKey::End => 35,
            NamedKey::PageUp => 33,
            NamedKey::PageDown => 34,
            NamedKey::Shift => 16,
            NamedKey::Control => 17,
            NamedKey::Alt => 18,
            NamedKey::Meta => 91,
            _ => 0,
        },
        Key::Character(s) => s
            .chars()
            .next()
            .filter(|c| c.is_ascii())
            .map(|c| c.to_ascii_uppercase() as i32)
            .unwrap_or(0),
    }
}
fn key_modifiers(m: contract::Modifiers) -> u32 {
    let mut flags = 0;
    if m.contains(contract::Modifiers::SHIFT) {
        flags |= 2;
    }
    if m.contains(contract::Modifiers::CONTROL) {
        flags |= 4;
    }
    if m.contains(contract::Modifiers::ALT) {
        flags |= 8;
    }
    if m.contains(contract::Modifiers::META) {
        flags |= 128;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn readiness_metrics_are_numeric_only_bounded_and_transaction_scoped() {
        assert_eq!(parse_height("neoism-ready:7:37.5"), Some((true, 7, 37.5)));
        assert_eq!(
            parse_height("neoism-height:8:8192"),
            Some((false, 8, 8192.))
        );
        for invalid in [
            "neoism-ready:7",
            "neoism-height:7:0",
            "neoism-height:7:NaN",
            "neoism-height:7:inf",
            "neoism-height:7:8193",
            "neoism-height:7:-1",
            "neoism-height:7:40:secret",
            "author log",
            "neoism-ready:x:40",
        ] {
            assert!(parse_height(invalid).is_none(), "{invalid}");
        }
    }
    #[test]
    fn popup_premultiplied_blend_and_clipping() {
        let mut base = vec![0, 0, 255, 255, 0, 0, 255, 255];
        let popup = vec![128, 0, 0, 128, 128, 0, 0, 128, 0, 255, 0, 255];
        let rect = Rect {
            x: -1,
            y: 0,
            width: 3,
            height: 1,
        };
        composite(&mut base, 2, 1, &popup, &rect);
        assert_eq!(base, vec![128, 0, 127, 255, 0, 255, 0, 255]);
        let before = base.clone();
        composite(
            &mut base,
            2,
            1,
            &popup,
            &Rect {
                x: 0,
                y: -10,
                width: 3,
                height: 1,
            },
        );
        assert_eq!(base, before);
        composite(
            &mut base,
            2,
            1,
            &popup,
            &Rect {
                x: i32::MAX,
                y: 0,
                width: 3,
                height: 1,
            },
        );
        assert_eq!(base, before);
    }
    #[test]
    fn popup_hide_restores_base_and_gate_blocks_publication() {
        let viewport = Viewport {
            width: 1,
            height: 1,
            scale: 1.0,
        };
        let mut state = State {
            viewport,
            visible: true,
            loaded: true,
            closed: false,
            epoch: 1,
            ready: true,
            theme_script: String::new(),
            content_height: None,
            last_height: None,
            pixels: None,
            base: Some(vec![0, 0, 255, 255]),
            popup: Popup {
                shown: true,
                rect: Rect {
                    x: 0,
                    y: 0,
                    width: 1,
                    height: 1,
                },
                pixels: Some(vec![255, 0, 0, 255]),
            },
            buttons: 0,
            keymods: 0,
        };
        state.publish();
        assert_eq!(state.pixels.take().unwrap(), vec![255, 0, 0, 255]);
        state.popup = Popup::default();
        state.publish();
        assert_eq!(state.pixels.take().unwrap(), vec![0, 0, 255, 255]);
        state.ready = false;
        state.publish();
        assert!(state.pixels.is_none());
        state.ready = true;
        state.visible = false;
        state.publish();
        assert!(state.pixels.is_none());
    }
    #[test]
    fn physical_shortcut_and_punctuation_identity() {
        assert_eq!(
            virtual_key(contract::Code::KeyA, &Key::Character("a".into())),
            65
        );
        assert_eq!(
            virtual_key(contract::Code::Slash, &Key::Character("?".into())),
            191
        );
        assert_eq!(
            virtual_key(contract::Code::Digit1, &Key::Character("!".into())),
            49
        );
        assert_eq!(
            virtual_key(contract::Code::Enter, &Key::Named(NamedKey::Enter)),
            13
        );
        assert_eq!(
            key_modifiers(contract::Modifiers::CONTROL | contract::Modifiers::SHIFT),
            6
        );
    }
}
