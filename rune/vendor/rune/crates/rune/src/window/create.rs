//! Creating the native window: its attributes, frame, surface and GPU, accessibility adapter
//! and platform pieces, and the first frame it comes on screen with.

use super::*;

impl<S: 'static> Host<S> {
    #[cfg_attr(not(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android"))), allow(unused_variables))]
    pub(super) fn try_create(&mut self, event_loop: &ActiveEventLoop, token: Option<String>, focus: bool) -> Result<(), String> {
        // A released window reopens at the height its content reached while it was gone.
        let built = self.shown && self.build_for_show();
        let size = self.args.size.unwrap_or(self.spec.size);
        let launcher = self.launcher();
        let visible = !launcher && (self.started || self.options.visible);
        #[allow(unused_mut)]
        let mut attrs = self.options.attributes(self.rt.window_title.current().unwrap_or(self.spec.title), size, self.spec.min_size, visible).with_active(focus);
        #[cfg(target_os = "macos")]
        if self.macos_window.nonactivating_panel { attrs = attrs.with_visible(false).with_active(false); }
        #[cfg(all(unix, not(target_os = "macos"), not(target_os = "ios"), not(target_os = "android")))]
        {
            if let Some(id) = crate::tooling::app_id() {
                attrs = winit::platform::wayland::WindowAttributesExtWayland::with_name(attrs, id, id);
                attrs = winit::platform::x11::WindowAttributesExtX11::with_name(attrs, id, id);
            }
            if self.float_on_open { attrs = float_on_open::fixed(attrs, &self.options, size); }
            if let Some(token) = token {
                let token = winit::window::ActivationToken::from_raw(token);
                attrs = winit::platform::startup_notify::WindowAttributesExtStartupNotify::with_activation_token(attrs, token);
            }
        }
        let maximize = self.open_maximized && !self.shown && self.options.resizable;
        // Windows maximizes it once cloaked: maximizing shows a window.
        #[cfg(not(target_os = "windows"))]
        { attrs = attrs.with_maximized(maximize); }
        let chrome = frame_chrome(self.chrome, self.titlebar, self.rt.titlebar_set());
        #[cfg(target_os = "macos")]
        if !self.options.borderless && chrome.full_size_content {
            use winit::platform::macos::WindowAttributesExtMacOS;
            attrs = attrs.with_titlebar_transparent(true).with_title_hidden(true).with_fullsize_content_view(true);
        }
        // Windows opens it hidden, then shows it cloaked until its first frame lands
        // (`Cloak`), so it never comes on screen empty.
        #[cfg(target_os = "windows")]
        let attrs = crate::platform::app_icon::with_icon(attrs.with_visible(false));
        let window = Arc::new(event_loop.create_window(attrs).map_err(|e| e.to_string())?);
        #[cfg(target_os = "windows")]
        crate::platform::win32::follow_suggested_rect(&window);
        // Before it shows, so the title bar never appears.
        #[cfg(target_os = "windows")]
        let frame = self.titlebar.and_then(|t| crate::platform::win32::CustomFrame::install(&window, t.height()));
        #[cfg(target_os = "windows")]
        let cloak = crate::platform::win32::Cloak::default();
        #[cfg(target_os = "windows")]
        if visible {
            cloak.hold(&window, Instant::now() + win::READY_FALLBACK);
            // Unfocused (`App::open_inactive`), the first show doesn't activate it.
            window.set_visible(true);
            if maximize { window.set_maximized(true); }
        }
        // First, before anything observes the view (macOS) or an assistive client asks.
        let a11y = {
            let handle = self.native_handle.clone();
            crate::a11y::Adapter::new(&window, Arc::new(move || handle.send(WakeEvent::A11y)))
        };
        #[cfg(target_os = "macos")]
        crate::native_menu::adopt_standard_view();
        #[cfg(target_os = "macos")]
        let panel = if self.macos_window.nonactivating_panel {
            Some(crate::platform::macos::Panel::adopt_window(&window, size, &self.macos_window).ok_or("could not create nonactivating panel")?)
        } else { None };
        #[cfg(target_os = "macos")]
        crate::platform::macos::configure_window(&window, &self.macos_window);
        #[cfg(target_os = "macos")]
        crate::platform::macos::drop::install(&window);
        if self.options.click_through && let Err(error) = window.set_cursor_hittest(false) {
            eprintln!("click-through unavailable for {}: {error}", self.native_handle.key());
        }
        window.set_ime_allowed(false);
        self.rt.ime_reset = true;
        #[cfg(target_os = "macos")]
        if uses_drag_regions(chrome, self.options.borderless, false) {
            mac::own_window_dragging(&window);
        }
        self.rt.set_titlebar_inset(place_chrome(&window, chrome));
        #[cfg(target_os = "macos")]
        {
            self.rt.double_click_interval = mac::double_click_interval();
            self.rt.set_native_help(true);
        }
        #[cfg(windows)]
        {
            let ms = unsafe { windows_sys::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime() };
            self.rt.double_click_interval = f64::from(ms) / 1000.0;
            // A tooltip control's initial delay is the double-click time.
            self.rt.help_delay = self.rt.double_click_interval;
        }
        let mut prewarmed = None;
        let (gpu, surface) = match self.gpu.take() {
            Some((gpu, renderer)) => {
                let surface = gpu.instance.create_surface(window.clone());
                self.gpu = Some((gpu, renderer));
                (None, surface.map_err(|e| e.to_string())?)
            }
            None => {
                let features = self.comp.as_ref().map_or(wgpu::Features::empty(), |c| c.features());
                let transparent = self.options.transparent;
                // A shared device that can't composite this window's translucent pixels gets its own.
                let cached = self.gpu_pool.borrow().upgrade().and_then(|gpu| {
                    let surface = gpu.instance.create_surface(window.clone()).ok()?;
                    crate::render::translucency::serves(&gpu, &surface, features, transparent).then_some((gpu, surface))
                });
                let (gpu, surface) = if let Some(cached) = cached {
                    cached
                } else {
                    let (gpu, surface, renderer) = Gpu::for_surface(features, transparent, |instance| instance.create_surface(window.clone()).map_err(|e| e.to_string()))?;
                    prewarmed = renderer;
                    let gpu = Arc::new(gpu);
                    *self.gpu_pool.borrow_mut() = Arc::downgrade(&gpu);
                    crate::render::surface_gpu(&gpu);
                    (gpu, surface)
                };
                (Some(gpu), surface)
            }
        };
        let adapter = &gpu.as_ref().or(self.gpu.as_ref().map(|g| &g.0)).unwrap().adapter;
        let phys = window.inner_size();
        let (mut config, translucent) = surface_config(&surface, adapter, (phys.width, phys.height), self.rt.srgb_blend())
            .ok_or("the GPU adapter can't present to this window")?;
        #[cfg(target_os = "macos")]
        let launch_drawables = LaunchDrawables::cap(&mut config);
        if self.options.transparent {
            config.alpha_mode = translucent.unwrap_or_else(|| {
                eprintln!("transparency unavailable for {}; using an opaque window", self.native_handle.key());
                wgpu::CompositeAlphaMode::Opaque
            });
            self.rt.transparent = config.alpha_mode != wgpu::CompositeAlphaMode::Opaque;
        }
        let target_format = if self.rt.srgb_blend() { config.format.remove_srgb_suffix() } else { config.format };
        if let Some(gpu) = gpu {
            let mut renderer = prewarmed.unwrap_or_else(|| Renderer::new(&gpu.device, target_format));
            renderer.set_srgb_blend(self.rt.srgb_blend());
            self.gpu = Some((gpu, renderer));
        }
        let (gpu, renderer) = self.gpu.as_mut().unwrap();
        rebuild_renderer(&gpu.device, renderer, target_format, &mut self.rt);
        surface.configure(&gpu.device, &config);
        #[cfg(target_os = "macos")]
        crate::platform::macos::pin_contents_top_left(&surface);
        #[cfg(target_os = "macos")]
        crate::platform::macos::window_shape::apply_corners(&window, &self.macos_window, self.options.transparent, self.options.borderless);
        // Metal's PostMultiplied mode only makes CAMetalLayer non-opaque; Core Animation
        // still composites premultiplied pixels. Other backends follow wgpu's straight-alpha contract.
        let alpha = (self.options.transparent && config.alpha_mode == wgpu::CompositeAlphaMode::PostMultiplied
            && gpu.adapter.get_info().backend != wgpu::Backend::Metal)
            .then(|| crate::render::surface_alpha::AlphaSurface::new(&gpu.device, target_format, (config.width, config.height)));
        let scale = window.scale_factor() as f32;
        self.rt.set_system_dark(crate::platform::preferences::window_dark(&window, self.args.theme.as_deref()));
        self.rt.set_viewport((phys.width as f32 / scale, phys.height as f32 / scale), scale);
        if (self.shown || launcher) && !built {
            // Reopening paints the settled state in the first frame (guarantee 8).
            self.rt.settle_all();
        }
        self.shown = true;
        self.idle = true;
        self.rt.set_live_visible(visible);
        *self.native_handle.native.window.lock().unwrap() = Arc::downgrade(&window);
        self.native_handle.native.number.store(native_window_number(&window).unwrap_or(0), Ordering::Release);
        #[cfg(target_os = "macos")]
        let resamples = crate::platform::macos::window_shape::resamples(&window, self.options.transparent);
        #[cfg(target_os = "macos")]
        let hook_log = self.present_log.clone()
            .or_else(|| resamples.then(|| Arc::new(crate::frame::present_time::metal::Log::untraced())));
        #[cfg(target_os = "macos")]
        let present = hook_log.clone().and_then(|log| crate::frame::present_time::metal::LayerHook::attach(&surface, log));
        #[cfg(target_os = "macos")]
        let shadow = hook_log.filter(|_| resamples && present.is_some())
            .and_then(|log| crate::platform::macos::window_shape::Shadow::new(&window, log, visible || launcher));
        #[cfg(target_os = "macos")]
        if self.present_log.is_some() && present.is_none() { eprintln!("rune: trace: this surface reports no present times"); }
        #[cfg(target_os = "macos")]
        let fn_key = if self.rt.modifier_keys() { crate::input::fn_key::FnKey::new(&window) } else { None };
        #[cfg(target_os = "macos")]
        let full_screen = mac::FullScreenWatch::new(&window);
        #[cfg(target_os = "macos")]
        let prime = if visible && panel.is_none() { 2 } else { 0 };
        #[cfg(target_os = "macos")]
        let native = Native {
            pacing: crate::platform::display_link::DisplayLink::new(&window, self.frame_wake.clone()),
            present,
            shadow,
            panel,
            materials: crate::system_material::macos::Materials::new(),
            help: Default::default(),
            full_screen,
            fn_key,
            prime,
            launch_drawables,
            _spot: crate::platform::macos::traffic_lights::Spot::new(&window),
        };
        // Making the GPU and surface took part of the cloak's wait; the frame gets all of it.
        #[cfg(target_os = "windows")]
        cloak.extend(Instant::now() + win::READY_FALLBACK);
        #[cfg(target_os = "windows")]
        let native = Native { cloak, frame };
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let native = Native::default();
        self.win = Some(Win {
            a11y,
            native,
            first_frame: FirstFrame::opened(Instant::now()),
            surface,
            config,
            window,
            visible,
            occluded: false,
            retry: None,
            occluded_wait: OCCLUDED_RETRY,
            hidden_wait: TIMEOUT_RETRY,
            hidden_pace: HiddenPace::default(),
            cursor: crate::cursor::os::OsCursor::new(event_loop),
            alpha,
            quiet: false,
            timer_frame: TimerFrame::default(),
            presented: None,
            #[cfg(target_os = "windows")]
            steady: Steady::Rest,
            redraw: std::cell::Cell::new(Redraw::Idle),
        });
        if launcher { self.win.as_mut().unwrap().first_frame.prepare(); }
        self.apply_placement(event_loop);
        #[cfg(target_os = "macos")]
        if visible && let Some(w) = &self.win && w.native.panel.is_some() { w.show(focus); }
        if let Some(created) = self.on_created.take() { created(self.native_handle.clone()); }
        if let Some(on_window) = self.on_window.as_mut() { on_window(&self.native_handle); }
        if let (Some(w), Some((gpu, ..))) = (self.win.as_mut(), self.gpu.as_ref()) {
            #[cfg(target_os = "macos")]
            if let Some(pacing) = &mut w.native.pacing { pacing.update_display(&w.window); }
            let size = w.window.inner_size();
            let scale = w.window.scale_factor() as f32;
            if size.width > 0 && size.height > 0 {
                w.resize_surface(&gpu.device, size);
                self.rt.set_viewport((size.width as f32 / scale, size.height as f32 / scale), scale);
            }
            sync_chrome(&mut self.rt, w, self.chrome, self.titlebar);
            sync_screen_origin(&mut self.rt, &w.window);
            if w.window.has_focus() {
                self.rt.window_focus(true);
            }
        }
        // The first frame, now, so the window comes on screen with it instead of empty for a
        // frame or two. AppKit has ordered the window in, but it reaches the screen with this
        // turn's Core Animation commit, so nothing reports it visible yet.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut drew = false;
        #[cfg(target_os = "macos")]
        if visible && !self.rt.minimize_request && !self.finished && !event_loop.exiting()
            && self.win.as_ref().is_some_and(|w| w.native.panel.is_none() && w.window.is_minimized() != Some(true))
        {
            let presents = self.meter.totals().presents;
            self.redraw();
            drew = self.meter.totals().presents > presents;
            self.shell(event_loop);
        }
        if !drew && (visible || launcher) && let Some(w) = &self.win {
            w.request_redraw();
        }
        Ok(())
    }
}
