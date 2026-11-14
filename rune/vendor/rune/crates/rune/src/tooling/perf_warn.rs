//! Debug-only warnings for app code that works but runs slow: a `virtualize` list that builds
//! every row to mount a few, a callback that blocks the main thread, a `poll` that says it
//! changed what shows when nothing did, a window that keeps drawing while idle, a slow first
//! frame, something that commits the view on most frames of a scroll, an app stepping its own
//! scroll glide, and (a mistake rather than slowness) a `memo` whose root already had another
//! key. Each warns once per site, on stderr; `RUNE_PERF_WARN=0` silences them. Release builds
//! contain none of it. `budget` is the exception: an opt-in line per slow commit, in any build.

pub(crate) mod budget;
mod commits;
mod idle;
pub(crate) mod launch;
pub(crate) mod runq;

#[cfg(debug_assertions)]
pub(crate) use idle::Drawn;
pub(crate) use idle::IdleMeter;
pub(crate) use commits::CommitMeter;

/// A window's commit watch: what the commit underway is for (`RUNE_COMMIT_BUDGET_MS` prints
/// it), and the commit warnings' meters.
#[derive(Default)]
pub(crate) struct Watch {
    /// `CommitWhy` bits for the commit underway; 0 for one outside a frame's checks.
    pub cause: u16,
    pub meter: CommitMeter,
    /// Interaction timing (`tooling::interaction`), when it's on.
    pub interaction: crate::tooling::interaction::Tracker,
}

#[cfg(debug_assertions)]
use std::{collections::HashSet, sync::{Mutex, OnceLock}, time::Instant};

/// Blocked wall time (ms; time waiting for a core doesn't count) a callback may take before it warns, and how many times its CPU time
/// that wall time must also be. A callback that computes for 50 ms is slow but not blocked.
#[cfg(debug_assertions)]
const STALL_MS: f64 = 8.0;
#[cfg(debug_assertions)]
const STALL_CPU_RATIO: f64 = 4.0;

/// Rows a `virtualize` list must build, and how many times what it mounts, before it warns.
#[cfg(debug_assertions)]
const OVERBUILD_ROWS: usize = 200;
#[cfg(debug_assertions)]
const OVERBUILD_RATIO: usize = 4;

#[cfg(debug_assertions)]
fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("RUNE_PERF_WARN").is_none_or(|v| v != "0"))
}

/// Prints `message` the first time `site` warns.
#[cfg(debug_assertions)]
fn warn_once(site: String, message: impl FnOnce() -> String) {
    static WARNED: Mutex<Option<HashSet<String>>> = Mutex::new(None);
    #[cfg(test)]
    let site = format!("{:?} {site}", std::thread::current().id());
    if !enabled() || !WARNED.lock().unwrap().get_or_insert_default().insert(site) { return }
    let message = format!("rune: {}. (debug builds only; RUNE_PERF_WARN=0 silences)", message());
    #[cfg(test)]
    LOG.with_borrow_mut(|log| log.push(message.clone()));
    eprintln!("{message}");
}

#[cfg(all(test, debug_assertions))]
thread_local! {
    static LOG: std::cell::RefCell<Vec<String>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// The warnings this thread printed since the last call.
#[cfg(all(test, debug_assertions))]
pub(crate) fn take_log() -> Vec<String> { LOG.take() }

/// A `virtualize` list at `path`, set up at `site`, was handed `built` rows and mounts
/// `mounted` of them.
#[cfg(debug_assertions)]
pub(crate) fn overbuilt(path: &str, site: Option<&std::panic::Location>, built: usize, mounted: usize) {
    if built <= OVERBUILD_ROWS || built <= OVERBUILD_RATIO * mounted { return }
    let site = site.map_or_else(String::new, |s| format!(" (.virtualize at {s})"));
    warn_once(format!("overbuild {path}"), || format!(
        "virtualized list {path:?}{site} built {built} rows to mount {mounted}: build only the \
         mounted rows with `.virtual_rows(count, estimated_row_height, key, build)`"
    ));
}

/// `memo(id, ..)`, called at `site`, built a root already keyed `existing` and renamed it `id`.
#[cfg(debug_assertions)]
pub(crate) fn memo_key_replaced(id: &str, existing: &str, site: &std::panic::Location) {
    if existing == id { return }
    warn_once(format!("memo key {site}"), || format!(
        "`memo({id:?}, ..)` at {site} replaced its root's key {existing:?} with {id:?}: look it up \
         by {id:?}, or key the memo with the id you want"
    ));
}

/// Times one app callback on the main thread: [`Stall::end`] warns when it spent most of its
/// time blocked rather than computing.
pub(crate) struct Stall {
    #[cfg(debug_assertions)]
    start: Option<(Instant, f64, Option<f64>)>,
}

impl Stall {
    #[inline(always)]
    pub(crate) fn start() -> Self {
        Stall {
            #[cfg(debug_assertions)]
            start: if enabled() { crate::frame::profile::cpu_ms().map(|cpu| (Instant::now(), cpu, runq::wait_ms())) } else { None },
        }
    }

    /// `what` names the callback in the message, as "`poll`" or "`on_press` on \"#0/save\"";
    /// it warns once per `site`.
    #[inline(always)]
    #[cfg_attr(not(debug_assertions), allow(unused_variables))]
    pub(crate) fn end(self, what: impl FnOnce() -> (String, String)) {
        #[cfg(debug_assertions)]
        if let Some((wall, cpu, wait)) = self.start {
            let wall = wall.elapsed().as_secs_f64() * 1000.0;
            if wall <= STALL_MS { return }
            let cpu = crate::frame::profile::cpu_ms().map_or(0.0, |now| now - cpu);
            let waited = wait.and_then(|start| runq::wait_ms().map(|now| now - start));
            let Some(blocked) = runq::blocked_ms(wall, cpu, waited, waited.is_none() && runq::overloaded()) else { return };
            let (site, name) = what();
            warn_once(format!("stall {site}"), || {
                let queued = waited.filter(|w| *w >= 1.0).map_or_else(String::new, |w| format!(", waited {w:.0} ms for a core"));
                format!(
                    "{name} blocked {blocked:.0} ms (CPU {cpu:.0} ms{queued}) on the main thread: move IO off it \
                     with `rune::Latest` or a thread"
                )
            });
        }
    }
}

/// Runs `f`, an app callback named `name` (a call site of its own), under a [`Stall`].
#[inline(always)]
pub(crate) fn timed<T>(name: &'static str, f: impl FnOnce() -> T) -> T {
    let stall = Stall::start();
    let out = f();
    stall.end(|| (name.to_owned(), if name.contains(' ') { name.to_owned() } else { format!("`{name}`") }));
    out
}

/// A handler's site: its name and the element's path with the digits taken out, so the rows of
/// one list share it.
#[cfg(debug_assertions)]
pub(crate) fn handler_site(name: &str, path: &str) -> String {
    format!("{name} {}", path.chars().filter(|c| !c.is_ascii_digit()).collect::<String>())
}

#[cfg(all(test, debug_assertions))]
mod commits_tests;
#[cfg(all(test, debug_assertions))]
mod tests;
