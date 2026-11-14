//! How long launch took to put the first frame up, split into the app's parts (`main` before
//! `run`, `init` and the first view build) and Rune's (window, GPU and drawing). Warns once
//! when the app's parts pass 100 ms: they are what `on_ready` can take off the first frame.
//! Rune's own time doesn't count toward it, so a debug build's slow GPU setup stays quiet.

#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
use std::{sync::Mutex, time::{Duration, Instant}};

/// The app's share of the first frame, in ms, before it warns.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
const APP_MS: f64 = 100.0;

#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
#[derive(Default)]
struct Launch {
    /// When `run` started, and how long the process had run by then.
    run: Option<(Instant, Option<Duration>)>,
    init: Option<Duration>,
    view: Option<Duration>,
    done: bool,
}

#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
static LAUNCH: Mutex<Launch> = Mutex::new(Launch { run: None, init: None, view: None, done: false });

/// `App::run` starts a window.
#[inline(always)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn run_starts() {
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    {
        let age = process_age();
        LAUNCH.lock().unwrap().run.get_or_insert((Instant::now(), age));
    }
}

/// Runs the app's `init`.
#[inline(always)]
pub(crate) fn init<T>(f: impl FnOnce() -> T) -> T {
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    let start = Instant::now();
    let out = super::timed("init", f);
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    {
        let mut launch = LAUNCH.lock().unwrap();
        if launch.run.is_some() && launch.init.is_none() { launch.init = Some(start.elapsed()); }
    }
    out
}

/// Times a commit while it lives; the first after `run` starts is the first view build.
pub(crate) struct FirstView {
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    start: Option<Instant>,
}

impl FirstView {
    #[inline(always)]
    #[cfg_attr(not(all(debug_assertions, not(target_arch = "wasm32"))), allow(unused_variables))]
    pub(crate) fn start(first: bool) -> Self {
        FirstView {
            #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
            start: first.then(Instant::now),
        }
    }
}

#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
impl Drop for FirstView {
    fn drop(&mut self) {
        let Some(start) = self.start else { return };
        let mut launch = LAUNCH.lock().unwrap();
        if launch.run.is_some() && launch.view.is_none() { launch.view = Some(start.elapsed()); }
    }
}

/// The first window's first frame is ready.
#[inline(always)]
#[cfg_attr(target_arch = "wasm32", allow(dead_code))]
pub(crate) fn first_frame() {
    #[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
    {
        let mut launch = LAUNCH.lock().unwrap();
        let Some((run, age)) = launch.run.filter(|_| !launch.done) else { return };
        launch.done = true;
        let ms = |d: Option<Duration>| d.map_or(0.0, |d| d.as_secs_f64() * 1000.0);
        let (setup, init, view) = (ms(age), ms(launch.init), ms(launch.view));
        let total = setup + ms(Some(run.elapsed()));
        drop(launch);
        if let Some(message) = report(setup, init, view, total) { super::warn_once("first frame".into(), || message); }
    }
}

/// The warning for a first frame `total` ms after the process started, `setup` ms of it in
/// `main` before `run`, `init` in `init` and `view` in the first view build; none while the
/// app's parts stay under `APP_MS`.
#[cfg(all(debug_assertions, not(target_arch = "wasm32")))]
pub(crate) fn report(setup: f64, init: f64, view: f64, total: f64) -> Option<String> {
    if setup + init + view <= APP_MS { return None }
    let rune = (total - setup - init - view).max(0.0);
    let parts = [(setup, "setup in `main` before `run`"), (init, "`init`"), (view, "the first `view`")];
    let slowest = parts.iter().max_by(|a, b| a.0.total_cmp(&b.0)).map_or("", |p| p.1);
    Some(format!(
        "first frame took {total:.0} ms (setup {setup:.0} ms, init {init:.0} ms, first view {view:.0} ms, window and \
         GPU {rune:.0} ms); the slowest is {slowest}: start slow work from `on_ready`"
    ))
}

/// How long this process has run, from the system's record of its start.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn process_age() -> Option<std::time::Duration> {
    #[cfg(any(target_os = "macos", target_os = "linux", windows))]
    use std::time::Duration;
    #[cfg(target_os = "macos")]
    {
        // SAFETY: `info` is a live, writable `proc_bsdinfo` of the size passed, and an all-zero
        // `proc_bsdinfo` is a valid value of that plain C struct.
        let mut info: libc::proc_bsdinfo = unsafe { std::mem::zeroed() };
        let size = std::mem::size_of::<libc::proc_bsdinfo>() as libc::c_int;
        let got = unsafe { libc::proc_pidinfo(libc::getpid(), libc::PROC_PIDTBSDINFO, 0, (&raw mut info).cast(), size) };
        if got != size { return None }
        let start = std::time::UNIX_EPOCH + Duration::new(info.pbi_start_tvsec, info.pbi_start_tvusec as u32 * 1000);
        std::time::SystemTime::now().duration_since(start).ok()
    }
    #[cfg(target_os = "linux")]
    {
        // Field 22 of /proc/self/stat is the start in clock ticks since boot; fields after the
        // command's closing parenthesis start at field 3.
        let stat = std::fs::read_to_string("/proc/self/stat").ok()?;
        let start: f64 = stat[stat.rfind(')')? + 1..].split_whitespace().nth(19)?.parse().ok()?;
        // SAFETY: sysconf reads a configuration value and takes no pointers.
        let ticks = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let up: f64 = std::fs::read_to_string("/proc/uptime").ok()?.split_whitespace().next()?.parse().ok()?;
        (ticks > 0.0).then(|| Duration::from_secs_f64((up - start / ticks).max(0.0)))
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::FILETIME, System::Threading::{GetCurrentProcess, GetProcessTimes}};
        let mut created = FILETIME { dwLowDateTime: 0, dwHighDateTime: 0 };
        let (mut exited, mut kernel, mut user) = (created, created, created);
        // SAFETY: the current-process pseudo-handle is valid and all four outputs are live,
        // writable FILETIMEs. The pseudo-handle is borrowed, not closed.
        if unsafe { GetProcessTimes(GetCurrentProcess(), &mut created, &mut exited, &mut kernel, &mut user) } == 0 { return None }
        // FILETIME counts 100 ns ticks from 1601.
        let ticks = ((u64::from(created.dwHighDateTime) << 32) | u64::from(created.dwLowDateTime)).checked_sub(116_444_736_000_000_000)?;
        let start = std::time::UNIX_EPOCH + Duration::from_nanos(ticks * 100);
        std::time::SystemTime::now().duration_since(start).ok()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
    { None }
}
