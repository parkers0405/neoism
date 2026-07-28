use crate::event::EventProxy;
use crate::router::frame_stats::FrameCadenceStats;
use crate::router::router_impl::{centered_position, compute_window_size_from_grid};
use crate::screen::{Screen, ScreenWindowProperties};
use neoism_backend::config::window::{Decorations, WindowMode};
use neoism_backend::config::Config;
use neoism_backend::config::Config as RioConfig;
use neoism_window::dpi::PhysicalSize;
use neoism_window::event_loop::ActiveEventLoop;
#[cfg(not(any(target_os = "macos", windows)))]
use neoism_window::platform::startup_notify::{
    self, EventLoopExtStartupNotify, WindowAttributesExtStartupNotify,
};
use neoism_window::window::{
    CursorIcon, Fullscreen, Icon, ImePurpose, Window, WindowAttributes,
};
use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use std::time::{Duration, Instant};

pub const LOGO_ICON: &[u8] = include_bytes!("../../assets/icons/neoism.png");
// Terminal W/H constraints
pub const DEFAULT_MINIMUM_WINDOW_HEIGHT: i32 = 200;
pub const DEFAULT_MINIMUM_WINDOW_WIDTH: i32 = 300;

const MAX_ANIMATION_FRAME_DELTA: Duration = Duration::from_millis(50);

#[cfg(all(
    any(feature = "wayland", feature = "x11"),
    not(any(target_os = "macos", windows))
))]
pub const APPLICATION_ID: &str = "neoism";

pub fn create_window_builder(
    title: &str,
    config: &Config,
    #[allow(unused_variables)] tab_id: Option<&str>,
    #[allow(unused_variables)] app_id: Option<&str>,
) -> WindowAttributes {
    // On Windows, prefer the multi-size icon resource winres embeds in
    // the exe (ordinal 1): Windows then picks the hand-tuned 16/24/32px
    // frames for the title bar and taskbar instead of downscaling the
    // 512px PNG. Dev/cross builds without the resource fall back to it.
    #[cfg(windows)]
    let resource_icon = {
        use neoism_window::platform::windows::IconExtWindows;
        Icon::from_resource(1, None).ok()
    };
    #[cfg(not(windows))]
    let resource_icon: Option<Icon> = None;
    let icon = resource_icon.unwrap_or_else(|| {
        let image_icon = image_rs::load_from_memory(LOGO_ICON).unwrap();
        Icon::from_rgba(
            image_icon.to_rgba8().into_raw(),
            image_icon.width(),
            image_icon.height(),
        )
        .unwrap()
    });

    let mut window_builder = WindowAttributes::default()
        .with_title(title)
        .with_min_inner_size(neoism_window::dpi::LogicalSize {
            width: DEFAULT_MINIMUM_WINDOW_WIDTH,
            height: DEFAULT_MINIMUM_WINDOW_HEIGHT,
        })
        .with_resizable(true)
        .with_decorations(true)
        .with_transparent(config.window.opacity < 1.)
        .with_blur(config.window.blur)
        .with_window_icon(Some(icon));

    match config.window.decorations {
        Decorations::Disabled => {
            window_builder = window_builder.with_decorations(false);
        }
        Decorations::Transparent => {
            #[cfg(target_os = "macos")]
            {
                use neoism_window::platform::macos::WindowAttributesExtMacOS;
                window_builder = window_builder.with_titlebar_transparent(true)
            }
        }
        Decorations::Buttonless => {
            #[cfg(target_os = "macos")]
            {
                use neoism_window::platform::macos::WindowAttributesExtMacOS;
                window_builder = window_builder.with_titlebar_buttons_hidden(true)
            }
        }
        _ => {}
    };

    #[cfg(all(feature = "x11", not(any(target_os = "macos", windows))))]
    {
        use neoism_window::platform::x11::WindowAttributesExtX11;
        let app_name = app_id.unwrap_or(APPLICATION_ID);
        window_builder = window_builder.with_name(app_name.to_lowercase(), app_name);
    }

    #[cfg(all(feature = "wayland", not(any(target_os = "macos", windows))))]
    {
        use neoism_window::platform::wayland::WindowAttributesExtWayland;
        let app_name = app_id.unwrap_or(APPLICATION_ID);
        window_builder = window_builder.with_name(app_name.to_lowercase(), app_name);
    }

    #[cfg(target_os = "windows")]
    {
        use neoism_window::platform::windows::WindowAttributesExtWindows;
        if let Some(use_undecorated_shadow) = config.window.windows_use_undecorated_shadow
        {
            window_builder =
                window_builder.with_undecorated_shadow(use_undecorated_shadow);
        }

        if let Some(use_no_redirection_bitmap) =
            config.window.windows_use_no_redirection_bitmap
        {
            // This sets WS_EX_NOREDIRECTIONBITMAP.
            window_builder =
                window_builder.with_no_redirection_bitmap(use_no_redirection_bitmap);
        }
    }

    #[cfg(target_os = "macos")]
    {
        use neoism_window::platform::macos::WindowAttributesExtMacOS;
        // MacOS is always transparent
        window_builder = window_builder.with_transparent(true);

        // Configure colorspace
        window_builder = window_builder
            .with_colorspace(config.window.colorspace.to_neoism_window_colorspace());

        if let (Some(x), Some(y)) = (
            config.window.macos_traffic_light_position_x,
            config.window.macos_traffic_light_position_y,
        ) {
            window_builder = window_builder.with_traffic_light_position(x, y);
        }

        if config.navigation.is_native() {
            if let Some(identifier) = tab_id {
                window_builder = window_builder
                    .with_tabbing_identifier(identifier)
                    .with_unified_titlebar(config.window.macos_use_unified_titlebar);
            }
        } else {
            use crate::constants::TRAFFIC_LIGHT_PADDING;
            window_builder = window_builder
                .with_title_hidden(true)
                .with_titlebar_transparent(true)
                .with_fullsize_content_view(true);

            if config.navigation.is_enabled() {
                window_builder = window_builder.with_traffic_light_position(
                    TRAFFIC_LIGHT_PADDING,
                    TRAFFIC_LIGHT_PADDING,
                );
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        use neoism_window::platform::windows::WindowAttributesExtWindows;
        // On windows cloak (hide) the window initially, we later reveal it after the first draw.
        // This is a workaround to hide the "white flash" that occurs during application startup.
        window_builder = window_builder.with_cloaked(true);
    }

    match config.window.mode {
        WindowMode::Fullscreen => {
            window_builder =
                window_builder.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }
        WindowMode::Maximized => {
            window_builder = window_builder.with_maximized(true);
        }
        _ => {
            window_builder =
                window_builder.with_inner_size(neoism_window::dpi::LogicalSize {
                    width: config.window.width,
                    height: config.window.height,
                })
        }
    };

    window_builder
}

pub fn configure_window(winit_window: &Window, config: &Config) {
    if config.effects.custom_mouse_cursor {
        winit_window.set_cursor_visible(false);
    } else {
        winit_window.set_cursor_visible(true);
        let current_mouse_cursor = CursorIcon::Text;
        winit_window.set_cursor(current_mouse_cursor);
    }

    // https://docs.rs/winit/latest/winit;/window/enum.ImePurpose.html#variant.Terminal
    winit_window.set_ime_purpose(ImePurpose::Terminal);
    winit_window.set_ime_allowed(true);

    // TODO: Update ime position based on cursor
    // winit_window.set_ime_cursor_area(neoism_window::dpi::PhysicalPosition::new(500.0, 500.0), neoism_window::dpi::LogicalSize::new(400, 400));

    // This will ignore diacritical marks and accent characters from
    // being processed as received characters. Instead, the input
    // device's raw character will be placed in event queues with the
    // Alt modifier set.
    #[cfg(target_os = "macos")]
    {
        // OnlyLeft - The left `Option` key is treated as `Alt`.
        // OnlyRight - The right `Option` key is treated as `Alt`.
        // Both - Both `Option` keys are treated as `Alt`.
        // None - No special handling is applied for `Option` key.
        use neoism_window::platform::macos::{OptionAsAlt, WindowExtMacOS};

        match config.option_as_alt.to_lowercase().as_str() {
            "both" => winit_window.set_option_as_alt(OptionAsAlt::Both),
            "left" => winit_window.set_option_as_alt(OptionAsAlt::OnlyLeft),
            "right" => winit_window.set_option_as_alt(OptionAsAlt::OnlyRight),
            _ => {}
        }
    }

    let is_transparent = config.window.opacity < 1.;
    winit_window.set_transparent(is_transparent);

    #[cfg(target_os = "macos")]
    {
        use neoism_window::platform::macos::WindowExtMacOS;
        let bg_color = config.colors.background.1;
        winit_window.set_background_color(
            bg_color.r,
            bg_color.g,
            bg_color.b,
            config.window.opacity as f64,
        );

        if !config.window.macos_use_shadow {
            winit_window.set_has_shadow(false);
        }
    }

    #[cfg(target_os = "windows")]
    {
        use neoism_backend::config::window::WindowsCornerPreference;
        use neoism_window::platform::windows::WindowExtWindows;

        if let Some(with_corner_preference) = &config.window.windows_corner_preference {
            let preference = match with_corner_preference {
                WindowsCornerPreference::Default => {
                    neoism_window::platform::windows::CornerPreference::Default
                }
                WindowsCornerPreference::DoNotRound => {
                    neoism_window::platform::windows::CornerPreference::DoNotRound
                }
                WindowsCornerPreference::Round => {
                    neoism_window::platform::windows::CornerPreference::Round
                }
                WindowsCornerPreference::RoundSmall => {
                    neoism_window::platform::windows::CornerPreference::RoundSmall
                }
            };

            winit_window.set_corner_preference(preference);
        }
    }
    if let Some(title) = &config.title.placeholder {
        winit_window.set_title(title);
    }

    winit_window.set_blur(config.window.blur);
}

pub struct RouteWindow<'a> {
    pub is_focused: bool,
    pub is_occluded: bool,
    pub needs_render_after_occlusion: bool,
    pub render_timestamp: Instant,
    last_animation_frame_at: Instant,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    pub vblank_interval: Duration,
    vblank_refresh_rate_millihertz: Option<u32>,
    frame_cadence: FrameCadenceStats,
    pub winit_window: Window,
    current_mouse_cursor: CursorIcon,
    cursor_visible: bool,
    pub screen: Screen<'a>,
}

impl<'a> RouteWindow<'a> {
    fn vblank_interval_from_refresh_rate(
        refresh_rate_millihertz: u32,
    ) -> (Duration, f64) {
        neoism_ui::lifecycle_policy::vblank_interval_from_refresh_rate(
            refresh_rate_millihertz,
        )
    }

    fn monitor_refresh_rate_millihertz(window: &Window) -> Option<u32> {
        window
            .current_monitor()
            .and_then(|monitor| monitor.refresh_rate_millihertz())
    }

    fn initial_monitor_vblank_interval(
        event_loop: &ActiveEventLoop,
        window: &Window,
    ) -> (Duration, Option<u32>, f64, &'static str) {
        let current_monitor_refresh_rate = Self::monitor_refresh_rate_millihertz(window);
        let primary_monitor_refresh_rate = current_monitor_refresh_rate.or_else(|| {
            event_loop
                .primary_monitor()
                .and_then(|monitor| monitor.refresh_rate_millihertz())
        });
        let refresh_rate_millihertz = primary_monitor_refresh_rate.unwrap_or(60_000);
        let refresh_source = if current_monitor_refresh_rate.is_some() {
            "current_monitor"
        } else if primary_monitor_refresh_rate.is_some() {
            "primary_monitor"
        } else {
            "fallback_60hz"
        };
        let (interval, refresh_rate_hz) =
            Self::vblank_interval_from_refresh_rate(refresh_rate_millihertz);

        (
            interval,
            primary_monitor_refresh_rate,
            refresh_rate_hz,
            refresh_source,
        )
    }

    pub fn animation_frame_interval(&mut self) -> Duration {
        let sampled_refresh_rate_millihertz =
            Self::monitor_refresh_rate_millihertz(&self.winit_window);
        let refresh_rate_millihertz = sampled_refresh_rate_millihertz
            .or(self.vblank_refresh_rate_millihertz)
            .unwrap_or(60_000);
        let (interval, refresh_rate_hz) =
            Self::vblank_interval_from_refresh_rate(refresh_rate_millihertz);

        if sampled_refresh_rate_millihertz.is_some()
            && (interval != self.vblank_interval
                || sampled_refresh_rate_millihertz != self.vblank_refresh_rate_millihertz)
        {
            tracing::info!(
                target: "neoism::frame_pacing",
                window_id = ?self.winit_window.id(),
                refresh_rate_millihertz = ?sampled_refresh_rate_millihertz,
                refresh_rate_hz,
                frame_interval_ms = interval.as_secs_f64() * 1000.0,
                "updated animation frame interval"
            );
            self.vblank_interval = interval;
            self.vblank_refresh_rate_millihertz = sampled_refresh_rate_millihertz;
        } else if sampled_refresh_rate_millihertz.is_none()
            && self.vblank_refresh_rate_millihertz.is_none()
        {
            self.vblank_interval = interval;
        }
        self.vblank_interval
    }

    pub fn record_frame_cadence(&mut self, now: Instant) {
        self.frame_cadence
            .record_frame_start(now, self.vblank_interval);
    }

    pub fn animation_frame_delta(&mut self) -> Duration {
        let now = self.render_timestamp;
        let delta = now.saturating_duration_since(self.last_animation_frame_at);
        self.last_animation_frame_at = now;
        if delta.is_zero() {
            self.vblank_interval
        } else {
            delta.min(MAX_ANIMATION_FRAME_DELTA)
        }
    }

    pub fn finish_frame_cadence(&mut self, now: Instant) {
        self.frame_cadence
            .record_render_duration(now.saturating_duration_since(self.render_timestamp));
        self.frame_cadence.maybe_log(
            now,
            self.winit_window.id(),
            self.vblank_interval,
            self.vblank_refresh_rate_millihertz,
        );
    }

    pub fn configure_window(&mut self, config: &neoism_backend::config::Config) {
        configure_window(&self.winit_window, config);
    }

    pub fn set_cursor(&mut self, cursor: CursorIcon) {
        if self.current_mouse_cursor == cursor {
            return;
        }
        let _span = crate::app::freeze_watchdog::global_span(
            "route_window.set_cursor",
            format!("{cursor:?}"),
        );
        self.winit_window.set_cursor(cursor);
        self.current_mouse_cursor = cursor;
    }

    pub fn set_cursor_visible(&mut self, visible: bool) {
        if self.cursor_visible == visible {
            return;
        }
        let _span = crate::app::freeze_watchdog::global_span(
            "route_window.set_cursor_visible",
            format!("{visible}"),
        );
        self.winit_window.set_cursor_visible(visible);
        self.cursor_visible = visible;
    }

    pub fn wait_until(&self) -> Option<Duration> {
        // If we need to render after occlusion, render immediately
        if self.needs_render_after_occlusion {
            return None;
        }

        // On macOS, CVDisplayLink handles VSync synchronization automatically,
        // so we don't need software-based frame timing calculations
        #[cfg(target_os = "macos")]
        {
            None
        }

        #[cfg(not(target_os = "macos"))]
        {
            let now = Instant::now();
            let elapsed = now.duration_since(self.render_timestamp);
            let vblank = self.vblank_interval;

            // Calculate how many complete frames have elapsed
            let frames_elapsed = elapsed.as_nanos() / vblank.as_nanos();

            // Calculate when the next frame should occur
            let next_frame_time = self.render_timestamp
                + Duration::from_nanos(
                    (frames_elapsed + 1) as u64 * vblank.as_nanos() as u64,
                );

            if next_frame_time > now {
                // Return the time to wait until the next ideal frame time
                Some(next_frame_time.duration_since(now))
            } else {
                // We've missed the target frame time, render immediately
                None
            }
        }
    }

    // TODO: Use it whenever animated cursor is done
    // pub fn request_animation_frame(&mut self) {
    //     if self.config.renderer.strategy.is_event_based() {
    //         // Schedule a render for the next frame time
    //         let route_id = self.window.screen.ctx().current_route();
    //         let timer_id = TimerId::new(Topic::RenderRoute, route_id);
    //         let event = EventPayload::new(
    //             RioEventType::Rio(RioEvent::RenderRoute(route_id)),
    //             self.window.winit_window.id(),
    //         );

    //         // Always schedule at the next vblank interval
    //         self.scheduler.schedule(event, self.window.vblank_interval, false, timer_id);
    //     } else {
    //         // For game loop rendering, the standard redraw is fine
    //         self.request_redraw();
    //     }
    // }

    #[inline]
    pub fn update_vblank_interval(&mut self) {
        let _ = self.animation_frame_interval();
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_target<'b>(
        event_loop: &'b ActiveEventLoop,
        event_proxy: EventProxy,
        config: &'b RioConfig,
        font_library: &neoism_backend::sugarloaf::font::FontLibrary,
        window_name: &str,
        tab_id: Option<&str>,
        open_url: Option<String>,
        app_id: Option<&str>,
    ) -> RouteWindow<'a> {
        #[allow(unused_mut)]
        let mut window_builder =
            create_window_builder(window_name, config, tab_id, app_id);

        #[cfg(not(any(target_os = "macos", windows)))]
        if let Some(token) = event_loop.read_token_from_env() {
            tracing::debug!("Activating window with token: {token:?}");
            window_builder = window_builder.with_activation_token(token);

            // Remove the token from the env.
            startup_notify::reset_activation_token_env();
        }

        let winit_window = event_loop.create_window(window_builder).unwrap();
        configure_window(&winit_window, config);

        let properties = ScreenWindowProperties {
            size: winit_window.inner_size(),
            scale: winit_window.scale_factor(),
            raw_window_handle: winit_window.window_handle().unwrap().into(),
            raw_display_handle: winit_window.display_handle().unwrap().into(),
            window_id: winit_window.id(),
        };

        let screen = Screen::new(properties, config, event_proxy, font_library, open_url)
            .expect("Screen not created");

        if config.window.columns.is_some() || config.window.rows.is_some() {
            let (physical_width, physical_height) = compute_window_size_from_grid(
                config.window.columns,
                config.window.rows,
                &config.panel,
                &screen.ctx().current().dimension,
                winit_window.inner_size(),
            );
            let _ = winit_window.request_inner_size(PhysicalSize {
                width: physical_width,
                height: physical_height,
            });
            if let Some(pos) =
                centered_position(event_loop, physical_width, physical_height)
            {
                winit_window.set_outer_position(pos);
            }
        }

        #[cfg(target_os = "windows")]
        {
            // On windows cloak (hide) the window initially, we later reveal it after the first draw.
            // This is a workaround to hide the "white flash" that occurs during application startup.
            use neoism_window::platform::windows::WindowExtWindows;
            winit_window.set_cloaked(false);
        }

        let (
            monitor_vblank_interval,
            refresh_rate_millihertz,
            refresh_rate_hz,
            refresh_source,
        ) = Self::initial_monitor_vblank_interval(event_loop, &winit_window);
        tracing::info!(
            target: "neoism::frame_pacing",
            window_id = ?winit_window.id(),
            refresh_rate_millihertz = ?refresh_rate_millihertz,
            refresh_rate_hz,
            frame_interval_ms = monitor_vblank_interval.as_secs_f64() * 1000.0,
            refresh_source,
            xdg_session_type = ?std::env::var("XDG_SESSION_TYPE").ok(),
            wayland_display = std::env::var_os("WAYLAND_DISPLAY").is_some(),
            "initialized animation frame interval"
        );

        let now = Instant::now();
        Self {
            vblank_interval: monitor_vblank_interval,
            vblank_refresh_rate_millihertz: refresh_rate_millihertz,
            frame_cadence: FrameCadenceStats::new(now),
            render_timestamp: now,
            last_animation_frame_at: now,
            is_focused: true,
            is_occluded: false,
            needs_render_after_occlusion: false,
            current_mouse_cursor: CursorIcon::Text,
            cursor_visible: true,
            winit_window,
            screen,
        }
    }
}
