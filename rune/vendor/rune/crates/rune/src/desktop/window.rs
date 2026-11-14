//! One headless window in a `--desktop` run and its [`Member`] implementation.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use serde_json::{Value, json};

use crate::app::{Shared, WindowRequest};
use crate::desktop::Slot;
use crate::desktop::displays::VirtualDesktop;
use crate::el::Shell;
use crate::render::{Gpu, OFFSCREEN_FORMAT, Renderer};
use crate::runtime::{Env, Runtime};
use crate::tooling::{Args, Kind, Trace};
use crate::window::WakeEvent;
use crate::{WindowHandle, WindowSpec};

/// Corner radius of a titled window, in points.
const RADIUS: f32 = 10.0;

/// What a window asked of the desktop while it drained its requests.
#[derive(Default)]
pub(crate) struct Changes {
    /// Shown or ordered in front: it goes to the top.
    pub(super) front: bool,
    /// It takes the keyboard.
    pub(super) keys: bool,
}

/// One window of a `--desktop` run, whatever its state type.
pub(crate) trait Member {
    fn key(&self) -> &str;
    /// x, y, width, height in global points.
    fn frame(&self) -> [f32; 4];
    fn visible(&self) -> bool;
    fn click_through(&self) -> bool;
    fn on_top(&self) -> bool;
    /// Corner radius in points.
    fn radius(&self) -> f32;
    fn shadow(&self) -> bool;
    fn features(&self) -> wgpu::Features;
    /// Its first frame: where it opens on `desktop` when it has no position.
    fn start(&mut self, desktop: &VirtualDesktop);
    fn trace_on(&mut self);
    /// Applies what the app asked since the last call (updates, show/hide, place, close).
    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes;
    /// Something asked of it since `drain`, such as by a focus change another window's drain made.
    fn pending(&self) -> bool;
    /// Its last frame no longer shows its state, size or input.
    fn stale(&self) -> bool;
    fn advance(&mut self, dt: f64);
    fn focus(&mut self, focused: bool);
    /// Pointer and keyboard input; pointer kinds at `at`, in window points.
    fn input(&mut self, kind: &Kind, at: (f32, f32));
    fn leave(&mut self);
    fn find(&self, key: &str) -> Option<(f32, f32)>;
    fn pointer(&self) -> Option<(f32, f32)>;
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError>;
    /// How reopening the app sees it.
    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate;
    /// Runs `App::on_reopen`; true when it handled the reopen.
    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool;
    /// Shows it in front with the keyboard, as the default reopen.
    fn reopen(&mut self) -> Changes;
    fn menu(&self, item: &str) -> Result<(), crate::MenuError>;
    fn popup_open(&self) -> bool;
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError>;
    fn file_dialog_open(&self) -> bool;
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError>;
    /// Draws its frame into `slot`, `px` device pixels.
    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64);
    fn backdrop_dark(&self) -> Option<bool>;
    fn tree(&self, t: f64) -> Value;
    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64);
    fn summary(&mut self, device: &wgpu::Device) -> Option<String>;
}

pub(crate) struct Window<S: 'static> {
    key: String,
    pub(super) rt: Runtime<S>,
    shared: Arc<Shared<S>>,
    shortcuts: crate::shortcuts::Guard,
    menus: crate::native_menu::Guard<S>,
    handle: Option<WindowHandle>,
    comp: Option<Box<dyn crate::compose::Compositor<S>>>,
    renderer: Option<Renderer>,
    on_close: Option<fn(&mut S, &mut crate::Tx)>,
    on_reopen: Option<fn(&mut S, bool, &mut crate::Tx) -> bool>,
    keep_running: bool,
    launcher: bool,
    /// A nonactivating panel, launcher or not: reopening never brings it back.
    panel: bool,
    /// The app's own window, not one `App::window` added.
    main: bool,
    /// When it last hid, in `reopen::stamp` order.
    hidden_at: Option<u64>,
    shadow: bool,
    click_through: bool,
    on_top: bool,
    radius: f32,
    position: Option<(i32, i32)>,
    /// `WindowOptions::physical_size`, in pixels of the display the window opens on.
    physical: Option<(u32, u32)>,
    frame: [f32; 4],
    visible: bool,
    /// Closed without `keep_running`: it never shows again.
    finished: bool,
    placed: Option<Value>,
}

impl<S: 'static> Window<S> {
    pub(crate) fn new(spec: WindowSpec<S>, handle: Option<WindowHandle>, args: &Args, selected: bool, main: bool, scale: f32) -> Self {
        let WindowSpec { key, mut app, options, .. } = spec;
        // --size is the whole output here; each window keeps its own size.
        let mut inherited = args.clone();
        inherited.size = None;
        if !selected { inherited.scene = None; inherited.playground = None; inherited.variant = None; }
        let own = app.resolve_args(inherited);
        let comp = app.compositor.take();
        let (open, keep_running, on_close, on_focus, on_reopen) = (app.open, app.keep_running, app.on_close, app.on_focus, app.on_reopen);
        let on_mods = app.on_mods;
        let on_ime = app.on_ime;
        let native = std::mem::take(&mut app.macos_window);
        let (modifier_keys, tab_visits_controls, srgb_blend) = (app.modifier_keys, app.tab_visits_controls, app.srgb_blend);
        let font_smoothing = app.font_smoothing;
        let (chrome, titlebar) = (app.chrome, app.titlebar);
        let playground = app.playground.take();
        let (spec, state, shared) = app.into_parts(own.scene.as_deref());
        let shortcuts = crate::shortcuts::Guard::attach(&shared, Arc::new(|| {}), true, false);
        let menus = crate::native_menu::Guard::attach(&shared, true);
        let size = own.size.unwrap_or(spec.size);
        let dark = own.theme.as_deref() != Some("light");
        let mut rt = crate::tooling::runtime_for(&spec, state, size, Env::new(dark, scale), Arc::new(|| {}));
        crate::playground::attach(&mut rt, playground, &spec, own.playground.clone(), own.variant.clone(), false);
        rt.on_focus = on_focus;
        rt.on_mods = on_mods;
        rt.app_ime.hook = on_ime;
        rt.set_modifier_keys(modifier_keys);
        rt.set_tab_visits_controls(tab_visits_controls);
        if let Some(bars) = own.scroll_bars.or(args.scroll_bars) { rt.set_system_scroll_bars(bars); }
        rt.set_srgb_blend(srgb_blend);
        rt.set_font_smoothing(font_smoothing);
        rt.headless_chrome(chrome, titlebar, options.borderless);
        rt.transparent = options.transparent;
        rt.track_marks = comp.is_some();
        if selected && args.tree.is_some() { rt.key_warnings = Some(Vec::new()); }
        let visible = options.visible && open;
        rt.set_live_visible(visible);
        shared.visible.store(visible, Ordering::Release);
        let decorated = !options.borderless && !options.transparent;
        Window {
            key, rt, shared, shortcuts, menus, handle, comp, renderer: None, on_close, on_reopen, keep_running,
            launcher: native.launcher,
            panel: native.nonactivating_panel,
            main,
            hidden_at: None,
            shadow: native.shadow != Some(false),
            click_through: options.click_through,
            on_top: options.always_on_top,
            radius: if decorated { RADIUS } else { 0.0 },
            position: options.position,
            physical: options.physical_size,
            frame: [0.0, 0.0, size.0, size.1],
            visible,
            finished: false,
            placed: None,
        }
    }

    fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        self.rt.set_live_visible(visible);
        self.shared.visible.store(visible, Ordering::Release);
    }

    fn request(&mut self, request: WindowRequest, changes: &mut Changes) {
        match request {
            WindowRequest::Open(_) | WindowRequest::Show if self.finished => {}
            WindowRequest::Open(_) => {
                self.show();
                changes.front = true;
                changes.keys = true;
            }
            WindowRequest::Show => {
                self.show();
                changes.front = true;
                // A launcher takes keys when it shows.
                changes.keys |= self.launcher;
            }
            WindowRequest::Hide => self.hide(),
            WindowRequest::Close => self.close(),
        }
    }

    fn show(&mut self) {
        if self.visible { return; }
        // A launcher reveals the frame it kept up to date while hidden; others land their motion.
        if !self.launcher { self.rt.settle_all(); }
        self.set_visible(true);
    }

    fn hide(&mut self) {
        if !self.visible { return; }
        self.hidden_at = Some(crate::platform::reopen::stamp());
        self.rt.cancel_popups();
        self.rt.cancel_file_dialogs();
        self.rt.cancel_pointer();
        self.set_visible(false);
    }

    fn close(&mut self) {
        if !(self.visible || !self.keep_running) { return; }
        if let Some(f) = self.on_close { self.rt.update(f); }
        self.hide();
        if !self.keep_running {
            self.rt.cancel_popups();
            self.rt.cancel_file_dialogs();
            self.finished = true;
        }
    }

    fn place(&mut self, desktop: &VirtualDesktop, request: &crate::windows::PlaceRequest, t: f64) {
        let size = (self.frame[2], self.frame[3]);
        let (display, frame) = desktop.place(request, size);
        let record = json!({ "t": t, "window": self.key, "display": display, "frame": frame });
        println!("rune-placed {record}");
        self.placed = Some(record);
        self.position = None;
        self.resize(frame);
    }

    fn move_to(&mut self, (x, y): (f32, f32)) {
        self.frame[0] = x;
        self.frame[1] = y;
        self.rt.set_window_origin((x, y));
    }

    fn resize(&mut self, frame: [f32; 4]) {
        self.frame = frame;
        let scale = self.rt.env.scale;
        self.rt.set_viewport((frame[2], frame[3]), scale);
        self.rt.set_window_origin((frame[0], frame[1]));
    }
}

impl<S: 'static> Member for Window<S> {
    fn key(&self) -> &str { &self.key }
    fn frame(&self) -> [f32; 4] { self.frame }
    fn visible(&self) -> bool { self.visible }
    fn click_through(&self) -> bool { self.click_through }
    fn on_top(&self) -> bool { self.on_top }
    fn radius(&self) -> f32 { self.radius }
    fn shadow(&self) -> bool { self.shadow }
    fn features(&self) -> wgpu::Features { self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features()) }

    fn start(&mut self, desktop: &VirtualDesktop) {
        let display = desktop.display_at(self.position);
        let (w, h) = match self.physical {
            Some((w, h)) => ((f64::from(w) / display.scale) as f32, (f64::from(h) / display.scale) as f32),
            None => (self.frame[2], self.frame[3]),
        };
        self.resize(desktop.open_at(self.position, (w, h)));
    }

    fn trace_on(&mut self) {
        self.rt.trace_channels = Some(Default::default());
        self.rt.scene.store_mut().audit_retargets(true);
    }

    fn drain(&mut self, desktop: &VirtualDesktop, t: f64) -> Changes {
        let mut changes = Changes::default();
        self.shortcuts.drain();
        self.menus.drain();
        let requests = self.shared.drain(|f| {
            self.rt.update_if(f);
            self.menus.drain();
        });
        if let Some(p) = requests.position { self.move_to(desktop.to_points(p)); }
        if let Some(place) = &requests.place { self.place(desktop, place, t); }
        if let Some(request) = requests.window { self.request(request, &mut changes); }
        if requests.focus && self.visible { changes.front = true; changes.keys = true; }
        // `quit` ends a native app; a recording runs to the end of its timeline.
        let queued = self.handle.as_ref().map(WindowHandle::take_headless).unwrap_or_default();
        for event in queued {
            match event {
                WakeEvent::Window(_, request) => self.request(request, &mut changes),
                WakeEvent::Place(_, place) => self.place(desktop, &place, t),
                WakeEvent::Position(_, p) => self.move_to(desktop.to_points(p)),
                WakeEvent::Focus(_) if self.visible => { changes.front = true; changes.keys = true; }
                _ => {}
            }
        }
        for _ in 0..8 {
            // A headless desktop draws no Dock to minimize into (`requests.minimize` too).
            self.rt.minimize_request = false;
            match self.rt.shell_request.take() {
                Some(Shell::Close) => self.close(),
                Some(Shell::Quit) | None => {}
            }
            if self.visible {
                self.rt.open_popup_headless();
            } else {
                self.rt.cancel_popups();
                self.rt.refuse_file_dialog("cannot open a file dialog while the window is hidden");
            }
            if self.rt.shell_request.is_none() { break; }
        }
        changes
    }

    fn pending(&self) -> bool {
        self.shared.window_pending() || self.rt.shell_request.is_some() || self.handle.as_ref().is_some_and(WindowHandle::headless_pending)
    }

    fn stale(&self) -> bool { self.rt.stale() || self.rt.text.families_behind() }
    fn advance(&mut self, dt: f64) { self.rt.frame(dt); }
    fn focus(&mut self, focused: bool) { self.rt.window_focus(focused); }

    fn input(&mut self, kind: &Kind, (x, y): (f32, f32)) {
        let rt = &mut self.rt;
        match kind {
            Kind::Focus(focused) => rt.window_focus(*focused),
            Kind::Move => rt.pointer_move(x, y),
            Kind::Down => rt.pointer_down(x, y),
            Kind::Up => rt.pointer_up(x, y),
            Kind::SecondaryDown => rt.secondary_down(x, y),
            Kind::SecondaryUp => rt.secondary_up(x, y),
            Kind::Key(k, false) => rt.key(k.clone()),
            Kind::Key(k, true) => rt.repeat_key(k.clone()),
            Kind::Physical(event) => rt.physical_input(*event),
            Kind::Paste(content) => rt.paste_content(content.clone()),
            Kind::Wheel(dx, dy) => crate::tooling::wheel(rt, (x, y), *dx, *dy),
            Kind::Fling(vx, vy) => crate::tooling::fling(rt, (x, y), *vx, *vy),
            Kind::Files(step) => crate::tooling::file_input(rt, step, (x, y)),
            Kind::Mark(_) | Kind::Reopen | Kind::OpenUrl(_) | Kind::Menu(_) | Kind::PopupMenu(_) | Kind::FileDialog(_) | Kind::Shortcut(_) => {}
            kind => { crate::tooling::canvas_input(rt, kind, (x, y)); }
        }
    }

    fn leave(&mut self) { self.rt.pointer_leave(); }
    fn find(&self, key: &str) -> Option<(f32, f32)> { self.rt.find(key) }
    fn pointer(&self) -> Option<(f32, f32)> { self.rt.pointer() }
    fn shortcut(&self, shortcut: &crate::Shortcut) -> Result<(), crate::ShortcutError> { self.shared.shortcuts.fire(shortcut) }

    fn reopen_candidate(&self) -> crate::platform::reopen::Candidate {
        crate::platform::reopen::Candidate {
            reopenable: !self.finished && !self.panel,
            visible: self.visible,
            // A headless desktop has no Dock to minimize into.
            minimized: false,
            hidden_at: self.hidden_at,
            main: self.main,
        }
    }

    fn reopen_hook(&mut self, has_visible_windows: bool) -> bool {
        let Some(f) = self.on_reopen.filter(|_| !self.finished) else { return false };
        let mut handled = false;
        self.rt.update(|s, tx| handled = f(s, has_visible_windows, tx));
        handled
    }

    fn reopen(&mut self) -> Changes {
        let mut changes = Changes::default();
        self.request(WindowRequest::Open(None), &mut changes);
        changes
    }
    fn menu(&self, item: &str) -> Result<(), crate::MenuError> { crate::native_menu::fire(&self.shared, item) }
    fn popup_open(&self) -> bool { self.rt.popup_menu().is_some() }
    fn resolve_popup(&mut self, picked: Option<&str>) -> Result<(), crate::PopupMenuError> { self.rt.resolve_popup(picked) }
    fn file_dialog_open(&self) -> bool { self.rt.file_dialog().is_some() }
    fn resolve_file_dialog(&mut self, path: Option<std::path::PathBuf>) -> Result<(), crate::FileDialogError> { self.rt.resolve_file_dialog(path) }

    fn render(&mut self, gpu: &Gpu, slot: &Slot, px: (u32, u32), dt: f64, update_ms: f64) {
        let format = if self.rt.srgb_blend() { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
        let view = if self.rt.srgb_blend() { &slot.srgb_blend_view } else { &slot.view };
        let renderer = match &mut self.renderer {
            Some(renderer) => {
                crate::window::rebuild_renderer(&gpu.device, renderer, format, &mut self.rt);
                renderer
            }
            None => {
                let mut renderer = Renderer::new(&gpu.device, format);
                renderer.set_srgb_blend(self.rt.srgb_blend());
                self.renderer.insert(renderer)
            }
        };
        renderer.upload_images(&gpu.device, &gpu.queue, &mut self.rt.images);
        renderer.prepare_surfaces(&gpu.device, &gpu.queue, &self.rt.surfaces, px);
        let mut enc = gpu.device.create_command_encoder(&Default::default());
        if let Some(c) = self.comp.as_mut() {
            let mut f = crate::compose::Frame {
                device: &gpu.device, queue: &gpu.queue, encoder: &mut enc, target: view, format,
                size: px, dt, update_ms, rt: &mut self.rt, ui: renderer,
            };
            c.render(&mut f);
        } else {
            renderer.prepare(&gpu.device, &gpu.queue, &mut self.rt.text.atlas, &self.rt.instances, px);
            renderer.draw(&mut enc, view, self.rt.clear_color());
        }
        renderer.submit(&gpu.queue, enc);
    }

    fn backdrop_dark(&self) -> Option<bool> { self.rt.backdrop_dark() }

    fn tree(&self, t: f64) -> Value {
        let mut tree = crate::tooling::tree_json(&self.rt, t);
        if let Some(placed) = &self.placed { tree["placed"] = placed.clone(); }
        tree
    }

    fn trace(&mut self, trace: &mut Trace, t_ms: f64, cpu_ms: f64) { trace.frame(&mut self.rt, t_ms, cpu_ms); }
    fn summary(&mut self, device: &wgpu::Device) -> Option<String> { self.comp.as_mut().and_then(|c| c.summary(device)) }
}

