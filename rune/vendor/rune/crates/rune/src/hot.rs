//! Desktop development only. Use `scripts/rune-dev`: it validates edits before dx sees them.
//! Long-lived state belongs to Runtime, never to tip-crate statics or cached view trait objects.

use std::path::Path;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, OnceLock, atomic::{AtomicBool, AtomicU64, Ordering}};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::render::{Gpu, OFFSCREEN_FORMAT, Offscreen, Renderer};
use crate::runtime::{Env, Waker};
use crate::tooling::{AppSpec, Args, initial_state, runtime_for, tree_json};
use crate::{Cx, El, KeyEvent, Tx};
use std::any::Any;

// The hot tip crate owns only these thin typed adapters. Runtime<HostState>, layout,
// shell and headless loop are instantiated in the stable rune library, not recompiled per app edit.
// Any's vtable supplies type_id/drop only; no patchable behavior is cached in it.
pub(crate) struct HostState {
    pub(crate) value: Box<dyn Any>,
    spec: Box<dyn Any>,
    view: fn(&HostState, &mut Cx<HostState>) -> El,
    poll: fn(&mut HostState, f64, &mut Tx) -> bool,
    setup: fn(&mut HostState, Waker),
    key: fn(&mut HostState, &KeyEvent, &mut Tx) -> bool,
}

thread_local! {
    static INITIAL: std::cell::RefCell<Option<HostState>> = const { std::cell::RefCell::new(None) };
    static ORIGINAL_ONLY: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Runs `f` with views built by their startup code, skipping the patch table: the fallback
/// after a patched view, or a row it built, panicked.
pub(crate) fn original_only<R>(f: impl FnOnce() -> R) -> R {
    struct Reset(bool);
    impl Drop for Reset {
        fn drop(&mut self) { ORIGINAL_ONLY.set(self.0); }
    }
    let _reset = Reset(ORIGINAL_ONLY.replace(true));
    f()
}

/// Whether views run as built at startup, inside [`original_only`].
pub(crate) fn originals_only() -> bool {
    ORIGINAL_ONLY.get()
}

fn view_adapter<S: 'static>(host: &HostState, cx: &mut Cx<HostState>) -> El {
    let spec = host.spec.downcast_ref::<AppSpec<S>>().unwrap();
    let state = host.value.downcast_ref::<S>().unwrap();
    view_or_original(spec.view, state, cx, |state, typed| subsecond::HotFn::current(spec.view).call((state, typed)))
}

fn view_or_original<S>(original: fn(&S, &mut Cx<S>) -> El, state: &S, cx: &mut Cx<HostState>, patched: impl FnOnce(&S, &mut Cx<S>) -> El) -> El {
    let mut typed = cx.retype();
    if ORIGINAL_ONLY.get() {
        let tree = original(state, &mut typed);
        cx.settings_from(typed);
        return tree;
    }
    let tree = match catch_unwind(AssertUnwindSafe(|| patched(state, &mut typed))) {
        Ok(tree) => tree,
        Err(_) => {
            eprintln!("rune-hot: view panicked; rendering the original view");
            // Discard context writes from the failed view and bypass the patch table.
            typed = cx.retype();
            original(state, &mut typed)
        }
    };
    cx.settings_from(typed);
    tree
}

/// Calls `f` through the patch table; if the patched code panics, calls the startup `f` once
/// instead, as a panicking view renders the original. Side effects of the panicked call stay.
macro_rules! patched_or_original {
    ($what:literal, $f:expr, $($arg:expr),* $(,)?) => {{
        let f = $f;
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| subsecond::HotFn::current(f).call(($($arg,)*)))) {
            Ok(value) => value,
            Err(_) => {
                eprintln!(concat!("rune-hot: patched ", $what, " panicked; calling the original"));
                f($($arg),*)
            }
        }
    }};
}
pub(crate) use patched_or_original;

fn poll_adapter<S: 'static>(host: &mut HostState, t: f64, tx: &mut Tx) -> bool {
    let spec = host.spec.downcast_ref::<AppSpec<S>>().unwrap();
    let Some(f) = spec.poll else { return false };
    patched_or_original!("poll", f, host.value.downcast_mut::<S>().unwrap(), t, &mut *tx)
}

fn setup_adapter<S: 'static>(host: &mut HostState, waker: Waker) {
    let spec = host.spec.downcast_ref::<AppSpec<S>>().unwrap();
    if let Some(f) = spec.setup { f(host.value.downcast_mut::<S>().unwrap(), waker); }
}

fn key_adapter<S: 'static>(host: &mut HostState, key: &KeyEvent, tx: &mut Tx) -> bool {
    let spec = host.spec.downcast_ref::<AppSpec<S>>().unwrap();
    let Some(f) = spec.on_key else { return false };
    patched_or_original!("on_key", f, host.value.downcast_mut::<S>().unwrap(), key, &mut *tx)
}

pub(crate) fn run<S: 'static>(spec: AppSpec<S>, args: Args) {
    run_app(crate::App::new(spec), args);
}

pub(crate) fn run_app<S: 'static>(app: crate::App<S>, mut args: Args) {
    if args.list_scenes {
        for (name, _) in app.spec.scenes { println!("{name}"); }
        return;
    }
    let (srgb_blend, modifier_keys, font_smoothing) = (app.srgb_blend, app.modifier_keys, app.font_smoothing);
    let frame = (app.chrome, app.titlebar);
    let scene = args.scene.take();
    let (spec, state, _) = app.into_parts(scene.as_deref());
    let value = Box::new(state);
    let shell = AppSpec {
        title: spec.title, size: spec.size, min_size: spec.min_size, fonts: spec.fonts, families: spec.families,
        scenes: &[], init: || INITIAL.with(|s| s.borrow_mut().take().expect("hot initial state")),
        view: |s: &HostState, cx| (s.view)(s, cx),
        poll: Some(|s: &mut HostState, t, tx| (s.poll)(s, t, tx)),
        setup: Some(|s: &mut HostState, wake| (s.setup)(s, wake)),
        on_key: Some(|s: &mut HostState, key, tx| (s.key)(s, key, tx)),
    };
    let state = HostState { value, spec: Box::new(spec), view: view_adapter::<S>, poll: poll_adapter::<S>, setup: setup_adapter::<S>, key: key_adapter::<S> };
    run_host(shell, state, args, (srgb_blend, font_smoothing), modifier_keys, frame);
}

#[inline(never)]
fn run_host(spec: AppSpec<HostState>, state: HostState, args: Args, look: Look, modifier_keys: bool, frame: (crate::Chrome, Option<crate::Titlebar>)) {
    INITIAL.with(|slot| { assert!(slot.borrow().is_none()); *slot.borrow_mut() = Some(state); });
    if args.snap_on_patch.is_some() {
        let state = initial_state(&spec, args.scene.as_deref());
        return headless(&spec, state, args, look);
    }
    let mut app = crate::App::new(spec).font_smoothing(look.1).srgb_blend(look.0);
    app.modifier_keys = modifier_keys;
    // The window's frame is the app's, as without hot reload.
    (app.chrome, app.titlebar) = frame;
    if args.headless() { crate::tooling::headless(app, args); }
    else if let Err(error) = crate::window::run(app, args) { panic!("{error}"); }
}

/// `--snap-on-patch` for a window from `run_windows*`, `WindowSpec::from_app` or `App::window`:
/// the same typed `Runtime<S>` a native window builds, so its view, poll and on_key patch the
/// way they do on screen.
pub(crate) fn snap_window<S: 'static>(app: crate::App<S>, args: Args) {
    let look = (app.srgb_blend, app.font_smoothing);
    let (spec, state, _) = app.into_parts(args.scene.as_deref());
    headless(&spec, state, args, look);
}

static ENABLED: AtomicBool = AtomicBool::new(false);

pub(crate) fn enable() {
    assert!(std::env::var_os("RUNE_DEV_GUARDED").is_some(), "use scripts/rune-dev: unguarded Rust patches can change live state layouts");
    if let Some(path) = std::env::var_os("RUNE_HOT_PARK") {
        // Desk mode: another Mac runs this binary. The build Mac's copy only reports where dx
        // built it, and exits before it can connect and take the patches meant for the desk.
        let exe = std::env::current_exe().expect("rune-hot: current exe");
        atomic_write(Path::new(&path), exe.as_os_str().as_encoded_bytes());
        eprintln!("rune-hot: parked {}; the desk Mac runs it", exe.display());
        std::process::exit(0);
    }
    ENABLED.store(true, Ordering::Release);
}

/// `RUNE_HOT=1` enables live editing like `--hot`, for apps that keep their argv from Rune.
pub(crate) fn enable_from_env() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| if std::env::var_os("RUNE_HOT").is_some_and(|v| v == "1") { enable() });
}

pub fn enabled() -> bool {
    enable_from_env();
    ENABLED.load(Ordering::Acquire)
}

/// One devserver connection per process: dx patches for the last pid and ASLR base that
/// connected, and every connection would apply each patch again. Windows share it.
#[derive(Default)]
pub(crate) struct Hub {
    generation: AtomicU64,
    next: AtomicU64,
    wakers: Mutex<Vec<(u64, Waker)>>,
}

impl Hub {
    pub(crate) fn patched(&self) {
        self.generation.fetch_add(1, Ordering::Release);
        let wakers: Vec<Waker> = self.wakers.lock().unwrap().iter().map(|(_, w)| w.clone()).collect();
        for wake in wakers { wake(); }
    }
}

fn hub() -> &'static Hub {
    static HUB: OnceLock<Hub> = OnceLock::new();
    let mut fresh = false;
    let hub = HUB.get_or_init(|| { fresh = true; Hub::default() });
    if fresh {
        subsecond::register_handler(Arc::new(|| hub.patched()));
        connect();
    }
    hub
}

fn connect() {
    // dioxus-devtools reads the same variables and connects silently, or not at all.
    let var = |name| std::env::var(name).ok();
    let Some(addr) = var("DIOXUS_DEVSERVER_IP").zip(var("DIOXUS_DEVSERVER_PORT"))
        .and_then(|(ip, port)| Some(std::net::SocketAddr::new(ip.parse().ok()?, port.parse().ok()?)))
    else {
        eprintln!("rune-hot: DIOXUS_DEVSERVER_IP/PORT are unset; edits will not patch (use scripts/rune-dev)");
        return;
    };
    if let Err(error) = std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
        eprintln!("rune-hot: no devserver at {addr} ({error}); edits will not patch");
        return;
    }
    let pid = std::process::id();
    dioxus_devtools::connect(move |msg| {
        if let dioxus_devtools::DevserverMsg::HotReload(msg) = msg
            && let Some(table) = msg.jump_table
            && msg.for_pid == Some(pid)
        {
            let lib = table.lib.clone();
            // SAFETY: rune-dev's guard admits only edits that keep every type, static and
            // signature; the patch was linked against this binary's ASLR reference.
            if let Err(error) = unsafe { subsecond::apply_patch(table) } {
                eprintln!("rune-hot: patch {} did not load: {error}", lib.display());
            }
        }
    });
}

pub(crate) struct Session {
    hub: &'static Hub,
    id: u64,
    seen: u64,
}

impl Session {
    pub(crate) fn connect(wake: Waker) -> Self { Self::join(hub(), wake) }

    fn join(hub: &'static Hub, wake: Waker) -> Self {
        let id = hub.next.fetch_add(1, Ordering::Relaxed);
        hub.wakers.lock().unwrap().push((id, wake));
        Self { hub, id, seen: hub.generation.load(Ordering::Acquire) }
    }

    pub(crate) fn changed(&mut self) -> bool {
        let next = self.hub.generation.load(Ordering::Acquire);
        let changed = next != self.seen;
        self.seen = next;
        changed
    }

    pub(crate) fn pending(&self) -> bool { self.hub.generation.load(Ordering::Acquire) != self.seen }

    /// Moves this session, waker and all, to a test's own hub, so a test patch reaches only it.
    #[cfg(test)]
    pub(crate) fn move_to(&mut self, hub: &'static Hub) {
        let wake = {
            let mut wakers = self.hub.wakers.lock().unwrap();
            let at = wakers.iter().position(|(id, _)| *id == self.id).unwrap();
            wakers.remove(at).1
        };
        *self = Self::join(hub, wake);
    }
}

#[cfg(test)]
pub(crate) fn enable_for_tests() { ENABLED.store(true, Ordering::Release); }

impl Drop for Session {
    fn drop(&mut self) { self.hub.wakers.lock().unwrap().retain(|(id, _)| *id != self.id); }
}

fn atomic_write(path: &Path, bytes: &[u8]) {
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent).unwrap(); }
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, bytes).unwrap();
    std::fs::rename(temp, path).unwrap();
}

fn control_input<S: 'static>(rt: &mut crate::Runtime<S>, command: &Value) {
    if let Some(tap) = command.get("tap") {
        if let Some((x, y)) = tap.as_str().and_then(|key| rt.find(key)) {
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
        } else {
            eprintln!("rune-hot: skipping tap: expected an existing element key");
        }
    }
    if let Some(scroll) = command.get("scroll") {
        match serde_json::from_value::<[f32; 3]>(scroll.clone()) {
            Ok([x, y, dy]) if [x, y, dy].iter().all(|v| v.is_finite()) => rt.wheel(x, y, dy),
            _ => eprintln!("rune-hot: skipping scroll: expected three finite numbers [x,y,dy]"),
        }
    }
}

/// Persistent offscreen GPU session. A control file is an atomically replaced JSON request:
/// `{"id":1,"tap":"login"}`, `{"id":2,"scroll":[800,400,100]}`,
/// `{"id":3,"pause":true}`, `{"id":4,"step":0.05}`, or `{"id":5,"quit":true}`.
/// Every request and patch publishes a PNG + JSON tree, then a flushed completion marker.
/// A window's blending (sRGB or not) and font smoothing.
type Look = (bool, crate::FontSmoothing);

fn headless<S: 'static>(spec: &AppSpec<S>, state: S, args: Args, (srgb_blend, font_smoothing): Look) {
    assert!(enabled(), "--snap-on-patch requires --hot or RUNE_HOT=1 (use scripts/rune-dev)");
    let size = args.size.unwrap_or(spec.size);
    let scale = args.scale.unwrap_or(2.0);
    let (wake, sleep) = std::sync::mpsc::channel();
    let mut rt = runtime_for(spec, state, size, Env { system_dark: args.theme.as_deref() != Some("light"), scale }, Arc::new(move || { let _ = wake.send(()); }));
    rt.set_srgb_blend(srgb_blend);
    rt.set_font_smoothing(font_smoothing);
    if let Some(bars) = args.scroll_bars { rt.set_system_scroll_bars(bars); }
    let gpu = Gpu::new(Gpu::instance(), None);
    let target_format = if srgb_blend { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
    let mut renderer = Renderer::new(&gpu.device, target_format);
    renderer.set_srgb_blend(srgb_blend);
    let (w, h) = ((size.0 * scale).round() as u32, (size.1 * scale).round() as u32);
    let mut target = Offscreen::with_format(&gpu.device, w, h, target_format);
    let png = args.snap_on_patch.as_ref().unwrap();
    let mut first = true;
    let mut paused = false;
    let mut command = Value::Null;
    let mut previous = Instant::now();
    let mut woken = true;
    loop {
        let now = Instant::now();
        let elapsed = (now - previous).as_secs_f64();
        // `None`: a frame `elapsed` after the last, under the shared frame rule.
        let mut fixed = (first || paused).then_some(0.0);
        previous = now;
        let mut requested = false;
        if !first && let Some(path) = &args.hot_control
            && let Ok(bytes) = std::fs::read(path)
            && let Ok(next) = serde_json::from_slice::<Value>(&bytes)
            && next != command
        {
            command = next;
            if command["quit"] == true { break; }
            if let Some(pause) = command["pause"].as_bool() { paused = pause; fixed = Some(0.0); }
            if let Some(step) = command["step"].as_f64() { fixed = Some(step.clamp(0.0, 10.0)); }
            control_input(&mut rt, &command);
            requested = true;
        }
        let timer_due = rt.wake_deadline().is_some_and(|deadline| deadline <= rt.clock + elapsed);
        if !first && !requested && !rt.hot.as_ref().unwrap().pending() && (paused || (!woken && !rt.animating && !timer_due)) {
            // Paused/idle sessions must not compete with their own patch compiler.
            if !paused { rt.clock += elapsed; }
            woken = sleep.recv_timeout(Duration::from_millis(16)).is_ok();
            continue;
        }
        let before = rt.hot.as_ref().unwrap().seen;
        match fixed {
            Some(dt) => { rt.frame(dt); }
            None => { crate::frame::clock::advance_by(&mut rt, elapsed); }
        }
        let generation = rt.hot.as_ref().unwrap().seen;
        if first || requested || before != generation {
            if renderer.srgb_blend() != rt.srgb_blend() {
                let format = if rt.srgb_blend() { OFFSCREEN_FORMAT.remove_srgb_suffix() } else { OFFSCREEN_FORMAT };
                crate::window::rebuild_renderer(&gpu.device, &mut renderer, format, &mut rt);
                target = Offscreen::with_format(&gpu.device, w, h, format);
            }
            renderer.upload_images(&gpu.device, &gpu.queue, &mut rt.images);
            renderer.prepare_surfaces(&gpu.device, &gpu.queue, &rt.surfaces, (w, h));
            renderer.prepare(&gpu.device, &gpu.queue, &mut rt.text.atlas, &rt.instances, (w, h));
            let mut enc = gpu.device.create_command_encoder(&Default::default());
            renderer.draw(&mut enc, &target.view, rt.clear_color());
            target.copy(&mut enc);
            gpu.queue.submit([enc.finish()]);
            let pixels = target.read(&gpu.device);
            let pixels_ns = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
            let temp = png.with_extension("tmp.png");
            if let Some(parent) = temp.parent() { std::fs::create_dir_all(parent).unwrap(); }
            let mut encoder = png::Encoder::new(std::io::BufWriter::new(std::fs::File::create(&temp).unwrap()), w, h);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::Fastest);
            encoder.write_header().unwrap().write_image_data(&pixels).expect("hot snapshot");
            std::fs::rename(temp, png).expect("publish hot snapshot");
            let mut tree = tree_json(&rt, rt.clock);
            tree["hot"] = json!({"generation": generation, "pid": std::process::id(), "state": format!("{:p}", crate::runtime::handler_state_ref(&rt.state) as *const dyn Any as *const ()), "request": command["id"], "pixels_ns": pixels_ns, "scrolls": rt.hot_scrolls(), "motion": rt.hot_motion()});
            atomic_write(&png.with_extension("json"), &serde_json::to_vec_pretty(&tree).unwrap());
            println!("rune-hot-pixels {}", tree["hot"]);
            use std::io::Write;
            std::io::stdout().flush().unwrap();
            first = false;
        }
        // The patch callback wakes immediately; control files are checked at a bounded cadence.
        woken = sleep.recv_timeout(Duration::from_millis(16)).is_ok();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Key, TextEdit, col, el, text_input};

    struct State { pressed: bool, hovered: bool, secondary: bool, changed: bool, keyed: bool, edit: TextEdit }

    fn runtime<S: Default + 'static>(view: fn(&S, &mut Cx<S>) -> El) -> crate::Runtime<HostState> {
        let spec = AppSpec::new("test", (300.0, 200.0), S::default, view);
        let host = HostState { value: Box::new((spec.init)()), spec: Box::new(spec), view: view_adapter::<S>, poll: poll_adapter::<S>, setup: setup_adapter::<S>, key: key_adapter::<S> };
        crate::Runtime::new(host, |s, cx| (s.view)(s, cx), (300.0, 200.0), Env::new(true, 1.0))
    }

    #[test]
    fn one_patch_reaches_every_window_once_and_closed_windows_leave() {
        let hub: &'static Hub = Box::leak(Box::default());
        let woken = Arc::new(AtomicU64::new(0));
        let waker = |n: u64| -> Waker { let woken = woken.clone(); Arc::new(move || { woken.fetch_add(n, Ordering::Relaxed); }) };
        let mut main = Session::join(hub, waker(1));
        let mut settings = Session::join(hub, waker(10));
        assert!(!main.changed() && !settings.changed());
        hub.patched();
        assert_eq!(woken.load(Ordering::Relaxed), 11);
        assert!(main.pending() && main.changed() && !main.changed());
        assert!(settings.changed());
        drop(settings);
        hub.patched();
        assert_eq!(woken.load(Ordering::Relaxed), 12, "a closed window's waker is not called");
        // A window opened after a patch starts from the patched code; it has nothing to recommit.
        let late = Session::join(hub, waker(100));
        assert!(!late.pending());
        assert!(main.changed());
    }

    thread_local! {
        static CALLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    /// The first call stands in for a panicking patch: with no patch loaded, the patch table
    /// and the startup pointer are the same function.
    fn panics_once() {
        CALLS.set(CALLS.get() + 1);
        if CALLS.get() == 1 { panic!("broken patched body"); }
    }

    #[test]
    fn a_typed_window_survives_a_panicking_view_and_its_memos_rebuild() {
        CALLS.set(0);
        fn view(s: &u32, cx: &mut Cx<u32>) -> El {
            cx.time_scale(0.25);
            col().child(crate::memo("card", *s, || {
                panics_once();
                el().key("card").size(40.0, 40.0).on_press(|s: &mut u32| *s += 1)
            }))
        }
        let mut rt = crate::Runtime::new(5u32, view, (200.0, 100.0), Env::new(true, 1.0));
        rt.frame(0.0);
        assert_eq!(CALLS.get(), 2, "the startup view runs once after the patched one panics");
        assert!(rt.find("card").is_some());
        assert_eq!(rt.time_scale, 0.25, "context writes come from the view that built the tree");
        let (x, y) = rt.find("card").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(0.0);
        assert_eq!(rt.state, 6);
        assert_eq!(CALLS.get(), 3, "the memo cache works again: a new version builds once");
        rt.update(|_, _| {});
        rt.frame(0.0);
        assert_eq!(CALLS.get(), 3, "and an unchanged version is reused");
    }

    /// Index-built rows run the app's closures after the view returns; a patched one that
    /// panics falls back like the view, and what the fallback memoized outlives its commit.
    #[test]
    fn a_panicking_index_built_row_falls_back_and_keeps_its_memos() {
        CALLS.set(0);
        fn view(s: &(u32, bool), _: &mut Cx<(u32, bool)>) -> El {
            col().key("scroll").size(200.0, 200.0).scroll(crate::ScrollSpec { stick_bottom: false }).child(
                col().key("rows").w_full().growth(crate::GrowthSpec::default()).virtual_rows(s.0 as usize, 40.0,
                    |_: &(u32, bool), i| format!("r{i}").into(),
                    |s: &(u32, bool), i| crate::memo(format!("r{i}"), s.1, || {
                        if s.1 && i == 0 { panics_once(); }
                        el().w_full().h(40.0)
                    })))
        }
        let mut rt = crate::Runtime::new((20u32, false), view, (200.0, 200.0), Env::new(true, 1.0));
        rt.frame(0.0);
        for _ in 0..3 {
            rt.update(|_, _| {});
            rt.frame(0.0);
        }
        rt.update(|s, _| s.1 = true);
        rt.frame(0.0);
        assert_eq!(CALLS.get(), 2, "the startup view builds the row once after the patched one panics");
        assert!(rt.find("r0").is_some());
        rt.update(|_, _| {});
        rt.frame(0.0);
        assert_eq!(CALLS.get(), 2, "the fallback's memo is reused");
    }

    #[test]
    fn panicking_poll_and_on_key_patches_fall_back_to_the_startup_callbacks() {
        CALLS.set(0);
        let mut rt = crate::Runtime::new(0u32, |_: &u32, _: &mut Cx<u32>| el().size(10.0, 10.0), (100.0, 100.0), Env::new(true, 1.0));
        rt.on_key = Some(|s, _, _| { panics_once(); *s += 1; true });
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Function(5), mods: Default::default() });
        rt.frame(0.0);
        assert_eq!((rt.state, CALLS.get()), (1, 2));
        CALLS.set(0);
        rt.poll = Some(|s, _, _| { panics_once(); *s += 10; true });
        rt.frame(0.0);
        assert_eq!((rt.state, CALLS.get()), (11, 2));
        // Through the type-erased host as well: its adapters dispatch through the table too.
        CALLS.set(0);
        fn view(_: &usize, _: &mut Cx<usize>) -> El { el().size(10.0, 10.0) }
        let mut host = runtime::<usize>(view);
        let spec = host.state.spec.downcast_mut::<AppSpec<usize>>().unwrap();
        spec.on_key = Some(|s, _, _| { panics_once(); *s += 1; true });
        host.on_key = Some(|s: &mut HostState, key, tx| (s.key)(s, key, tx));
        host.frame(0.0);
        host.key(KeyEvent { key: Key::Function(5), mods: Default::default() });
        host.frame(0.0);
        assert_eq!((*host.state.value.downcast_ref::<usize>().unwrap(), CALLS.get()), (1, 2));
    }

    #[test]
    fn app_try_run_snaps_the_selected_secondary_window_at_its_own_size() {
        ENABLED.store(true, Ordering::Release);
        let dir = std::env::temp_dir().join(format!("rune-hot-snap-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (png, control) = (dir.join("settings.png"), dir.join("control.json"));
        // Read after the first snapshot, so the loop ends once it is published.
        std::fs::write(&control, r#"{"quit":true}"#).unwrap();
        let mut app = crate::App::new(AppSpec::new("main", (120.0, 90.0), || 0u8, |_, _| el().key("main-root").size(10.0, 10.0)));
        app.window(crate::WindowSpec::from_app("settings",
            crate::App::new(AppSpec::new("settings", (60.0, 40.0), || 0u16, |_, _| el().key("settings-root").size(10.0, 10.0)))));
        let args = ["--snap-on-patch", png.to_str().unwrap(), "--hot-control", control.to_str().unwrap(), "--window", "settings", "--scale", "1"];
        app.args(args.map(String::from)).try_run().unwrap();
        let tree: Value = serde_json::from_slice(&std::fs::read(png.with_extension("json")).unwrap()).unwrap();
        let png_size = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&png).unwrap())).read_info().unwrap().info().size();
        std::fs::remove_dir_all(&dir).unwrap();
        let keys: Vec<&str> = tree["elements"].as_array().unwrap().iter().filter_map(|e| e["key"].as_str()).collect();
        assert_eq!(keys, ["settings-root"], "--window picks App::window's window, not main");
        assert_eq!(png_size, (60, 40));
    }

    #[test]
    fn a_pending_patch_keeps_a_window_stale_until_a_frame_rebuilds_it() {
        let hub: &'static Hub = Box::leak(Box::default());
        let mut rt = crate::Runtime::new(0u8, |_: &u8, _: &mut Cx<u8>| el().size(10.0, 10.0), (100.0, 100.0), Env::new(true, 1.0));
        rt.hot = Some(Session::join(hub, Arc::new(|| {})));
        rt.frame(0.0);
        assert!(!rt.needs_commit() && !rt.stale());
        hub.patched();
        // A hidden launcher reveals its cached frame unless this says it is out of date, and
        // refreshes that frame when its hidden poll (the patch's wake) reports it, poll or not.
        assert!(rt.needs_commit() && rt.stale());
        assert!(rt.poll_hidden());
        rt.poll = Some(|_, _, _| false);
        assert!(rt.poll_hidden());
        rt.frame(0.0);
        assert!(!rt.needs_commit() && !rt.stale() && !rt.poll_hidden());
    }

    #[test]
    fn snap_on_patch_is_a_headless_window_tool() {
        let args = Args::parse_from(vec!["--snap-on-patch".into(), "out.png".into(), "--window".into(), "settings".into()]);
        assert!(args.headless(), "run_windows must render the selected window headless, not open one");
    }

    #[test]
    fn panicking_patch_falls_back_without_losing_state_or_context() {
        fn original(s: &usize, _: &mut Cx<usize>) -> El {
            el().key(format!("count-{s}")).on_press(|s: &mut usize| *s += 1)
        }
        let mut rt = runtime(original);
        *rt.state.value.downcast_mut::<usize>().unwrap() = 7;
        let address = rt.state.value.as_ref() as *const dyn Any as *const ();
        rt.state.view = |host, cx| {
            let state = host.value.downcast_ref::<usize>().unwrap();
            let original = host.spec.downcast_ref::<AppSpec<usize>>().unwrap().view;
            view_or_original(original, state, cx, |_, typed| {
                typed.time_scale(0.125);
                panic!("broken patched view")
            })
        };
        rt.frame(0.0);
        assert!(rt.find("count-7").is_some());
        assert_eq!(rt.time_scale, 1.0, "failed view's context writes must be discarded");
        control_input(&mut rt, &json!({"tap": "count-7"}));
        rt.frame(0.0);
        assert!(rt.find("count-8").is_some(), "original handlers still operate on retained state");
        assert_eq!(rt.state.value.as_ref() as *const dyn Any as *const (), address);
        // A later healthy patch is still attempted rather than latching the failure.
        rt.state.view = |host, cx| {
            let state = host.value.downcast_ref::<usize>().unwrap();
            view_or_original(original, state, cx, |s, _| el().key(format!("recovered-{s}")))
        };
        rt.update(|_, _| {});
        rt.frame(0.0);
        assert!(rt.find("recovered-8").is_some());
    }

    #[test]
    fn invalid_control_input_is_skipped_and_later_requests_still_work() {
        let mut rt = runtime(|_: &usize, _: &mut Cx<usize>| el().key("button").on_press(|s: &mut usize| *s += 1));
        rt.frame(0.0);
        for command in [
            json!({"tap": "missing"}), json!({"tap": 1}),
            json!({"scroll": null}), json!({"scroll": "down"}),
            json!({"scroll": [1, 2]}), json!({"scroll": [1, 2, 3, 4]}),
            json!({"scroll": [1, "bad", 3]}), json!({"scroll": [1, 2, 1e100]}),
        ] {
            control_input(&mut rt, &command);
            rt.frame(0.0);
        }
        assert_eq!(*rt.state.value.downcast_ref::<usize>().unwrap(), 0);
        control_input(&mut rt, &json!({"tap": "button", "scroll": [10, 10, 20]}));
        rt.frame(0.0);
        assert_eq!(*rt.state.value.downcast_ref::<usize>().unwrap(), 1);
    }

    #[test]
    fn erased_host_routes_focus_physical_links_and_window_callbacks() {
        #[derive(Default)]
        struct Events { presses: usize, physical: usize, dismisses: usize, links: Vec<String>, paste: String, focused: bool, moved: bool, layout: Option<[f32; 4]> }
        fn view(_: &Events, _: &mut Cx<Events>) -> El {
            col().size(300.0, 200.0)
                .on_paste(|s: &mut Events, text, _| s.paste = text.into())
                .on_window_focus(|s: &mut Events, focused, _| s.focused = focused)
                .modal(false, |s: &mut Events| s.dismisses += 1)
                .child(el().key("button").size(100.0, 30.0).focusable(true)
                    .on_press(|s: &mut Events| s.presses += 1)
                    .on_pointer_move(|s: &mut Events, _| s.moved = true)
                    .on_layout(|s: &mut Events, rect| s.layout = Some(rect))
                    .on_physical_input(|s: &mut Events, ev, _| {
                        if matches!(ev, crate::PhysicalInput::Modifiers(_)) { s.physical += 1; }
                        false
                    }))
                .child(crate::text("docs").link(crate::Link::new(0..4, "docs"))
                    .on_link(|s: &mut Events, id, _| s.links.push(id.into())))
        }
        let mut rt = runtime(view);
        rt.frame(0.0);
        rt.frame(0.0); // Deliver the first layout report.
        rt.window_focus(true);
        rt.paste("window paste");
        // The modal focuses its first control; Space presses it (Return would go to the default button).
        rt.key(KeyEvent { key: Key::Text(" ".into()), mods: Default::default() });
        rt.physical_input(crate::PhysicalInput::Modifiers(Default::default()));
        rt.frame(0.0);
        let (x, y) = rt.find("button").unwrap();
        rt.pointer_move(x, y);
        rt.key(KeyEvent { key: Key::Tab, mods: Default::default() });
        rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
        rt.key(KeyEvent { key: Key::Escape, mods: Default::default() });
        rt.frame(0.0);
        let s = rt.state.value.downcast_ref::<Events>().unwrap();
        assert_eq!((s.presses, s.physical, s.dismisses), (1, 1, 1));
        assert_eq!(s.links, ["docs"]);
        assert_eq!(s.paste, "window paste");
        assert!(s.focused && s.moved);
        assert_eq!(s.layout.unwrap()[2..], [100.0, 30.0]);
    }

    #[test]
    fn erased_host_preserves_context_reads_and_host_motion_preference() {
        #[derive(Default)]
        struct Reads { edit: TextEdit, seen: std::cell::RefCell<(bool, Option<[f32; 4]>, Option<[f32; 4]>)> }
        fn view(s: &Reads, cx: &mut Cx<Reads>) -> El {
            *s.seen.borrow_mut() = (cx.system_reduced_motion(), cx.layout("input"), cx.caret("input"));
            col().child(text_input(&s.edit, crate::TextStyle::new(14.0, 400.0)).key("input").size(180.0, 30.0)
                .on_edit(|s: &mut Reads| &mut s.edit))
        }
        let mut rt = runtime(view);
        rt.set_system_reduced_motion(true);
        rt.frame(0.0);
        let (x, y) = rt.find("input").unwrap();
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.paste("caret");
        rt.frame(0.0);
        rt.frame(0.0);
        let s = rt.state.value.downcast_ref::<Reads>().unwrap();
        let (reduced, layout, caret) = *s.seen.borrow();
        assert!(reduced);
        assert_eq!(layout.unwrap()[2..], [180.0, 30.0]);
        assert!(caret.unwrap()[0] > 0.0);
        assert_eq!(s.edit.text, "caret");
    }

    #[test]
    fn erased_host_routes_handlers_and_context_to_the_typed_state() {
        fn view(s: &State, cx: &mut Cx<State>) -> El {
            cx.time_scale(0.75);
            col().child(el().key("button").size(80.0, 30.0).on_press(|s: &mut State| s.pressed = true)
                    .on_hover(|s: &mut State| s.hovered = true)
                    .on_secondary_press(|s: &mut State, _, _| s.secondary = true))
                .child(text_input(&s.edit, crate::tok::TextStyle::new(14.0, 400.0)).key("input").size(180.0, 30.0)
                    .on_input_change(|s: &mut State, _| s.changed = true)
                    .on_input_key(|s: &mut State, _, _| { s.keyed = true; false })
                    .on_edit(|s: &mut State| &mut s.edit).on_submit(|s: &mut State, tx| { tx.copy(s.edit.text.clone()); }))
        }
        let spec = AppSpec::new("test", (200.0, 150.0), || State { pressed: false, hovered: false, secondary: false, changed: false, keyed: false, edit: TextEdit::new("") }, view);
        let host = HostState { value: Box::new((spec.init)()), spec: Box::new(spec), view: view_adapter::<State>, poll: poll_adapter::<State>, setup: setup_adapter::<State>, key: key_adapter::<State> };
        let mut rt = crate::Runtime::new(host, |s, cx| (s.view)(s, cx), (200.0, 150.0), Env { system_dark: true, scale: 1.0 });
        rt.frame(0.0);
        assert_eq!(rt.time_scale, 0.75);
        let (x, y) = rt.find("button").unwrap();
        rt.secondary_down(x, y);
        rt.secondary_up(x, y);
        rt.frame(0.0);
        for key in ["button", "input"] {
            let (x, y) = rt.find(key).unwrap();
            rt.pointer_move(x, y);
            rt.pointer_down(x, y);
            rt.pointer_up(x, y);
            rt.frame(0.0);
        }
        rt.paste("state kept");
        rt.frame(0.0);
        rt.key(KeyEvent { key: Key::Enter, mods: Default::default() });
        rt.frame(0.0);
        let s = rt.state.value.downcast_ref::<State>().unwrap();
        assert!(s.pressed);
        assert!(s.hovered);
        assert!(s.secondary && s.changed && s.keyed);
        assert_eq!(s.edit.text, "state kept");
        assert_eq!(rt.copy_request.as_deref(), Some("state kept"));
    }
}
