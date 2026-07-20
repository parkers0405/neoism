use neoism_window::window::WindowId;
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CHECK_INTERVAL: Duration = Duration::from_secs(1);
const REDRAW_STALL_AFTER: Duration = Duration::from_secs(2);
const RENDER_STALL_AFTER: Duration = Duration::from_secs(2);
const GLOBAL_SPAN_STALL_AFTER: Duration = Duration::from_secs(2);
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);
const WATCHDOG_ENV: &str = "NEOISM_FREEZE_WATCHDOG";
const WATCHDOG_LOG_ENV: &str = "NEOISM_FREEZE_LOG";

static WATCHDOG: OnceLock<Option<Arc<FreezeWatchdog>>> = OnceLock::new();

#[derive(Clone)]
struct WindowState {
    last_event: String,
    last_event_detail: String,
    last_event_at: Instant,
    redraw_requested_at: Option<Instant>,
    redraw_reason: String,
    render_in_progress: bool,
    render_stage: String,
    render_stage_at: Instant,
    render_started_at: Option<Instant>,
    last_render_done_at: Option<Instant>,
    redraw_requests: u64,
    redraw_deliveries: u64,
    render_completions: u64,
}

impl WindowState {
    fn new(now: Instant) -> Self {
        Self {
            last_event: "created".to_string(),
            last_event_detail: String::new(),
            last_event_at: now,
            redraw_requested_at: None,
            redraw_reason: String::new(),
            render_in_progress: false,
            render_stage: "idle".to_string(),
            render_stage_at: now,
            render_started_at: None,
            last_render_done_at: None,
            redraw_requests: 0,
            redraw_deliveries: 0,
            render_completions: 0,
        }
    }
}

struct WatchdogState {
    started_at: Instant,
    last_heartbeat_at: Instant,
    last_global_event: String,
    last_global_event_at: Instant,
    active_global_span: Option<ActiveGlobalSpan>,
    windows: BTreeMap<String, WindowState>,
    sampled_notes: BTreeMap<String, Instant>,
}

struct FreezeWatchdog {
    path: PathBuf,
    state: Mutex<WatchdogState>,
}

struct ActiveGlobalSpan {
    name: String,
    detail: String,
    started_at: Instant,
}

pub struct GlobalSpan {
    name: Option<String>,
}

pub fn init() {
    // Always sweep stale diagnostic logs, even with the watchdog off —
    // past always-on defaults left hundreds of per-pid files behind.
    prune_diag_logs();
    let _ = watchdog();
}

/// Remove old `freeze-watchdog-*` / `editor-grid-*` per-pid logs from
/// the config log dir. Anything older than 3 days goes; when the
/// corresponding diagnostic is disabled there's no reason to keep the
/// family at all, so those are removed regardless of age.
fn prune_diag_logs() {
    let dir = neoism_backend::config::config_dir_path().join("log");
    let Ok(entries) = fs::read_dir(&dir) else {
        return;
    };
    let cutoff = SystemTime::now() - Duration::from_secs(3 * 24 * 60 * 60);
    let watchdog_on = watchdog_enabled();
    let this_pid = format!("-{}.log", std::process::id());
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        let (is_watchdog, is_grid) = (
            name.starts_with("freeze-watchdog-") && name.ends_with(".log"),
            // Legacy nvim editor-grid diagnostics — always stale now.
            name.starts_with("editor-grid-") && name.ends_with(".log"),
        );
        if !is_watchdog && !is_grid {
            continue;
        }
        if name.ends_with(&this_pid) {
            continue;
        }
        let family_disabled = (is_watchdog && !watchdog_on) || is_grid;
        let too_old = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .map(|modified| modified < cutoff)
            .unwrap_or(false);
        if family_disabled || too_old {
            let _ = fs::remove_file(&path);
        }
    }
}

pub fn mark_global(event: &str, detail: impl Into<String>) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    state.last_global_event = format!("{event}: {}", detail.into());
    state.last_global_event_at = Instant::now();
    state.active_global_span = None;
}

pub fn global_span(event: &str, detail: impl Into<String>) -> GlobalSpan {
    let Some(watchdog) = watchdog() else {
        return GlobalSpan { name: None };
    };
    let now = Instant::now();
    let detail = detail.into();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    state.last_global_event = format!("{event}.begin: {detail}");
    state.last_global_event_at = now;
    state.active_global_span = Some(ActiveGlobalSpan {
        name: event.to_string(),
        detail,
        started_at: now,
    });
    GlobalSpan {
        name: Some(event.to_string()),
    }
}

pub fn mark_window_event(window_id: WindowId, event: &str, detail: impl Into<String>) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let window = state
        .windows
        .entry(window_key(window_id))
        .or_insert_with(|| WindowState::new(now));
    window.last_event = event.to_string();
    window.last_event_detail = detail.into();
    window.last_event_at = now;
}

pub fn mark_redraw_requested(window_id: WindowId, reason: &str) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let window = state
        .windows
        .entry(window_key(window_id))
        .or_insert_with(|| WindowState::new(now));
    window.redraw_requested_at = Some(now);
    window.redraw_reason = reason.to_string();
    window.redraw_requests = window.redraw_requests.saturating_add(1);
}

pub fn mark_redraw_delivered(window_id: WindowId) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let window = state
        .windows
        .entry(window_key(window_id))
        .or_insert_with(|| WindowState::new(now));
    window.redraw_requested_at = None;
    window.redraw_deliveries = window.redraw_deliveries.saturating_add(1);
    window.last_event = "WindowEvent::RedrawRequested".to_string();
    window.last_event_detail.clear();
    window.last_event_at = now;
}

pub fn mark_render_stage(window_id: WindowId, stage: &str) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let window = state
        .windows
        .entry(window_key(window_id))
        .or_insert_with(|| WindowState::new(now));
    if !window.render_in_progress {
        window.render_started_at = Some(now);
    }
    window.render_in_progress = true;
    window.render_stage = stage.to_string();
    window.render_stage_at = now;
}

pub fn mark_render_done(window_id: WindowId) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let window = state
        .windows
        .entry(window_key(window_id))
        .or_insert_with(|| WindowState::new(now));
    window.render_in_progress = false;
    window.render_stage = "idle".to_string();
    window.render_stage_at = now;
    window.render_started_at = None;
    window.last_render_done_at = Some(now);
    window.render_completions = window.render_completions.saturating_add(1);
}

pub fn unregister_window(window_id: WindowId) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    state.windows.remove(&window_key(window_id));
}

pub fn note(detail: impl Into<String>) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let detail = detail.into();
    watchdog.write_line(format_args!("NOTE {detail}"));
}

pub fn note_sampled(
    key: impl Into<String>,
    interval: Duration,
    detail: impl Into<String>,
) {
    let Some(watchdog) = watchdog() else {
        return;
    };
    let key = key.into();
    let now = Instant::now();
    let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
    let should_write = state
        .sampled_notes
        .get(&key)
        .is_none_or(|last| now.duration_since(*last) >= interval);
    if !should_write {
        return;
    }
    state.sampled_notes.insert(key, now);
    drop(state);

    let detail = detail.into();
    watchdog.write_line(format_args!("NOTE {detail}"));
}

impl Drop for GlobalSpan {
    fn drop(&mut self) {
        let Some(name) = self.name.as_deref() else {
            return;
        };
        let Some(watchdog) = watchdog() else {
            return;
        };
        let mut state = watchdog.state.lock().expect("freeze watchdog mutex");
        let matches_active = state
            .active_global_span
            .as_ref()
            .map(|span| span.name == name)
            .unwrap_or(false);
        if matches_active {
            state.active_global_span = None;
        }
        state.last_global_event = format!("{name}.end");
        state.last_global_event_at = Instant::now();
    }
}

fn watchdog() -> Option<Arc<FreezeWatchdog>> {
    WATCHDOG
        .get_or_init(|| {
            if !watchdog_enabled() {
                return None;
            }

            let now = Instant::now();
            let watchdog = Arc::new(FreezeWatchdog {
                path: log_path(),
                state: Mutex::new(WatchdogState {
                    started_at: now,
                    last_heartbeat_at: now,
                    last_global_event: "init".to_string(),
                    last_global_event_at: now,
                    active_global_span: None,
                    windows: BTreeMap::new(),
                    sampled_notes: BTreeMap::new(),
                }),
            });
            watchdog.write_line(format_args!(
                "started pid={} path={}",
                std::process::id(),
                watchdog.path.display()
            ));
            let thread_watchdog = Arc::clone(&watchdog);
            let _ = thread::Builder::new()
                .name("neoism-freeze-watchdog".to_string())
                .spawn(move || loop {
                    thread::sleep(CHECK_INTERVAL);
                    thread_watchdog.check();
                });
            Some(watchdog)
        })
        .clone()
}

fn watchdog_enabled() -> bool {
    // OFF BY DEFAULT: it earned its keep diagnosing the (since fixed)
    // nvim RPC freezes, but always-on it filled `log/` with a
    // per-launch file of per-frame NOTE lines — tens of MB per long
    // session. Opt back in with NEOISM_FREEZE_WATCHDOG=1 when chasing
    // a hang; `init()` still prunes old diagnostic logs either way.
    std::env::var_os(WATCHDOG_ENV)
        .map(|raw| neoism_ui::lifecycle_policy::env_flag_truthy(&raw.to_string_lossy()))
        .unwrap_or(false)
}

fn log_path() -> PathBuf {
    if let Some(path) = std::env::var_os(WATCHDOG_LOG_ENV) {
        return PathBuf::from(path);
    }
    neoism_backend::config::config_dir_path()
        .join("log")
        .join(format!("freeze-watchdog-{}.log", std::process::id()))
}

fn window_key(window_id: WindowId) -> String {
    format!("{window_id:?}")
}

impl FreezeWatchdog {
    fn check(&self) {
        let now = Instant::now();
        let mut lines = Vec::new();
        let mut should_dump_threads = false;
        {
            let mut state = self.state.lock().expect("freeze watchdog mutex");
            if now.duration_since(state.last_heartbeat_at) >= HEARTBEAT_INTERVAL {
                state.last_heartbeat_at = now;
                lines.push(format!(
                    "heartbeat uptime_ms={} last_global={} age_ms={}",
                    now.duration_since(state.started_at).as_millis(),
                    state.last_global_event,
                    now.duration_since(state.last_global_event_at).as_millis()
                ));
                for (window_id, window) in &state.windows {
                    lines.push(format!(
                        "window heartbeat window={} last_event={} detail={} requests={} deliveries={} render_completions={} redraw_reason={} render_stage={} render_in_progress={}",
                        window_id,
                        window.last_event,
                        window.last_event_detail,
                        window.redraw_requests,
                        window.redraw_deliveries,
                        window.render_completions,
                        window.redraw_reason,
                        window.render_stage,
                        window.render_in_progress
                    ));
                }
            }

            if let Some(span) = &state.active_global_span {
                let age = now.duration_since(span.started_at);
                if age >= GLOBAL_SPAN_STALL_AFTER {
                    should_dump_threads = true;
                    lines.push(format!(
                        "STALL global_span_in_progress name={} age_ms={} detail={} last_global={} global_age_ms={}",
                        span.name,
                        age.as_millis(),
                        span.detail,
                        state.last_global_event,
                        now.duration_since(state.last_global_event_at).as_millis()
                    ));
                }
            }

            for (window_id, window) in &state.windows {
                if let Some(requested_at) = window.redraw_requested_at {
                    let age = now.duration_since(requested_at);
                    if age >= REDRAW_STALL_AFTER {
                        should_dump_threads = true;
                        lines.push(format!(
                            "STALL redraw_not_delivered window={} age_ms={} reason={} last_event={} detail={} event_age_ms={} requests={} deliveries={} render_stage={} render_in_progress={}",
                            window_id,
                            age.as_millis(),
                            window.redraw_reason,
                            window.last_event,
                            window.last_event_detail,
                            now.duration_since(window.last_event_at).as_millis(),
                            window.redraw_requests,
                            window.redraw_deliveries,
                            window.render_stage,
                            window.render_in_progress
                        ));
                    }
                }

                if window.render_in_progress {
                    if let Some(started_at) = window.render_started_at {
                        let render_age = now.duration_since(started_at);
                        let stage_age = now.duration_since(window.render_stage_at);
                        if stage_age >= RENDER_STALL_AFTER {
                            should_dump_threads = true;
                            lines.push(format!(
                                "STALL render_in_progress window={} render_age_ms={} stage_age_ms={} stage={} last_event={} detail={} completions={}",
                                window_id,
                                render_age.as_millis(),
                                stage_age.as_millis(),
                                window.render_stage,
                                window.last_event,
                                window.last_event_detail,
                                window.render_completions
                            ));
                        }
                    }
                }
            }
        }

        if should_dump_threads {
            lines.push(format!("threads {}", thread_snapshot()));
        }

        for line in lines {
            self.write_line(format_args!("{line}"));
        }
    }

    fn write_line(&self, args: std::fmt::Arguments<'_>) {
        if let Some(parent) = self.path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let Ok(mut file) = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)
        else {
            return;
        };
        let _ = writeln!(
            file,
            "{} pid={} {}",
            wall_time_ms(),
            std::process::id(),
            args
        );
        let _ = file.flush();
    }
}

fn wall_time_ms() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

fn thread_snapshot() -> String {
    let Ok(entries) = fs::read_dir("/proc/self/task") else {
        return "task_read_failed".to_string();
    };
    let mut rows = Vec::new();
    for entry in entries.flatten() {
        let tid = entry.file_name().to_string_lossy().to_string();
        let path = entry.path();
        let comm = fs::read_to_string(path.join("comm"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "?".to_string());
        let wchan = fs::read_to_string(path.join("wchan"))
            .map(|s| s.trim().to_string())
            .unwrap_or_else(|_| "?".to_string());
        rows.push(format!("{tid}:{comm}:{wchan}"));
    }
    rows.sort();
    rows.join(",")
}
