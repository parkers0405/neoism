//! Building and running the event loop every native window shares.
#[cfg(not(target_arch = "wasm32"))]
use std::rc::Rc;
#[cfg(not(target_arch = "wasm32"))]
use winit::event_loop::{ControlFlow, EventLoop};

use crate::app::App;
use crate::tooling::Args;
use crate::windows::WindowSpec;
#[cfg(not(target_arch = "wasm32"))]
use super::{WakeEvent, Windows};
use super::Factory;

pub fn run<S: 'static>(app: App<S>, args: Args) -> Result<(), String> {
    let accessory = app.accessory;
    run_configured(Box::new(move |_| vec![WindowSpec::from_app("main", app).into()]), args, accessory)
}

pub(crate) fn run_windows(factory: Factory, args: Args, options: crate::RunOptions) -> Result<(), String> {
    run_configured(factory, args, options.accessory)
}

/// Without a display (no `WAYLAND_DISPLAY` or `DISPLAY`, a refused X11 connection) the event
/// loop can't be built.
fn event_loop_error(error: winit::error::EventLoopError) -> String {
    format!("event loop: {error}")
}

/// Whether this process can reach the window server. Over ssh to a Mac no one is logged in to,
/// AppKit starts but never finishes launching, so the app would wait forever.
#[cfg(target_os = "macos")]
fn window_server_session() -> bool {
    use std::ffi::c_void;
    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" { fn CGSessionCopyCurrentDictionary() -> *const c_void; }
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" { fn CFRelease(cf: *const c_void); }
    // SAFETY: a Copy-rule CF function; a non-null result is released exactly once.
    unsafe {
        let session = CGSessionCopyCurrentDictionary();
        if session.is_null() { return false; }
        CFRelease(session);
    }
    true
}

#[cfg(target_arch = "wasm32")]
fn run_configured(_: Factory, _: Args, _: bool) -> Result<(), String> {
    Err("native window factories are unsupported on web; use App::run for one canvas".into())
}

#[cfg(not(target_arch = "wasm32"))]
fn run_configured(factory: Factory, args: Args, accessory: bool) -> Result<(), String> {
    let _ = accessory;
    crate::tooling::perf_warn::launch::run_starts();
    crate::platform::ready_signal::run_starts();
    #[cfg(target_os = "macos")]
    if !window_server_session() {
        crate::render::discard_prewarm();
        return Err("event loop: no window server session (no one is logged in to this Mac)".into());
    }
    // While AppKit starts up.
    crate::render::prewarm_gpu();
    #[allow(unused_mut)]
    let mut builder = EventLoop::<WakeEvent>::with_user_event();
    #[cfg(target_os = "macos")]
    if accessory {
        use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_default_menu(false)
            .with_activate_ignoring_other_apps(false);
    }
    let event_loop = builder.build().inspect_err(|_| crate::render::discard_prewarm()).map_err(event_loop_error)?;
    event_loop.set_control_flow(ControlFlow::Wait);
    #[cfg(target_os = "macos")]
    crate::platform::app_icon::launched();
    let proxy = event_loop.create_proxy();
    let turn = proxy.clone();
    crate::popup_menu::start(std::sync::Arc::new(move || { let _ = turn.send_event(WakeEvent::Turn); }));
    let mut windows = Windows { factory: Some(factory), args, proxy, hosts: Vec::new(), gpu: Rc::default() };
    crate::open_url::incoming::prepare();
    let result = event_loop.run_app(&mut windows);
    // The windows' interaction timing queued its last rollups as they closed.
    crate::tooling::interaction::flush();
    crate::render::discard_prewarm();
    crate::clipboard::shutdown(); // Also cover an event-loop error before `exiting`.
    crate::platform::ready_signal::clear();
    result.map_err(|error| format!("run: {error}"))
}

#[cfg(not(target_arch = "wasm32"))]
impl Windows {
    /// The idle turn winit runs between its message loops, run while a native menu's modal
    /// loop holds winit's (`WakeEvent::Turn`): hosts whose timers are due resume, then
    /// `about_to_wait` draws overlays, answers assistive clients and sets the next wake.
    pub(super) fn turn(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        use winit::application::ApplicationHandler;
        let now = std::time::Instant::now();
        for host in self.hosts.iter_mut().flatten() {
            if let Some(due) = host.wake_deadline().filter(|&due| due <= now) {
                host.new_events(event_loop, winit::event::StartCause::ResumeTimeReached { start: due, requested_resume: due });
            }
        }
        self.about_to_wait(event_loop);
        self.cleanup(event_loop);
    }
}

#[test]
fn an_event_loop_that_cant_start_is_an_error_naming_the_event_loop() {
    let error = event_loop_error(winit::error::EventLoopError::RecreationAttempt);
    assert!(error.starts_with("event loop: ") && error.contains("recreated"), "{error}");
}
