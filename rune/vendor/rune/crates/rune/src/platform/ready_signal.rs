//! A launcher can wait for readiness. With `RUNE_READY_DIR` set, the process writes
//! `$RUNE_READY_DIR/<bundle id>.<pid>` once its first window's `on_ready` has returned (or would
//! have, with none), and removes it when it exits normally. The file is one line of JSON:
//! `{"pid":123,"bundle_id":"com.example.app","ready_ms":412,"rune":"0.1.0"}`, where `ready_ms`
//! counts from the process's start (from `run` where that's unknown). The bundle id is the main
//! bundle's on macOS, else the executable's name. A process killed outright leaves its file
//! behind, so a reader treats one whose PID is no longer running as stale.

use std::ffi::OsStr;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

const VAR: &str = "RUNE_READY_DIR";

static RUN: OnceLock<Instant> = OnceLock::new();
/// `None` until the first `on_ready`; then the file written, if any.
static WRITTEN: Mutex<Option<Option<PathBuf>>> = Mutex::new(None);

/// `run` starts the event loop.
pub(crate) fn run_starts() { RUN.get_or_init(Instant::now); }

/// A window's `on_ready` returned. Only the process's first writes the file.
pub(crate) fn ready() { ready_in(std::env::var_os(VAR).as_deref()) }

fn ready_in(dir: Option<&OsStr>) {
    let mut written = WRITTEN.lock().unwrap_or_else(|e| e.into_inner());
    if written.is_some() { return; }
    let Some(dir) = dir.filter(|dir| !dir.is_empty()) else { *written = Some(None); return };
    let ready_ms = crate::tooling::perf_warn::launch::process_age().or_else(|| RUN.get().map(Instant::elapsed)).map_or(0, |age| age.as_millis() as u64);
    let signal = Signal { pid: std::process::id(), bundle_id: bundle_id(), ready_ms };
    *written = Some(match signal.write_in(Some(dir)) {
        Some(Ok(path)) => {
            on_exit();
            Some(path)
        }
        Some(Err(error)) => {
            #[cfg(debug_assertions)]
            eprintln!("rune: couldn't write the {VAR} readiness file: {error}");
            let _ = error;
            None
        }
        None => None,
    });
}

/// Removes the file, if this process wrote one. Runs as the event loop returns and at exit.
pub(crate) fn clear() {
    // At exit, another thread may hold the lock; leaving the file then is what a kill does.
    let Ok(mut written) = WRITTEN.try_lock() else { return };
    if let Some(Some(path)) = written.as_mut().map(Option::take) { let _ = std::fs::remove_file(path); }
}

/// What the file says.
struct Signal { pid: u32, bundle_id: String, ready_ms: u64 }

impl Signal {
    fn json(&self) -> String {
        let id = serde_json::to_string(&self.bundle_id).unwrap_or_else(|_| "\"\"".into());
        format!(r#"{{"pid":{},"bundle_id":{id},"ready_ms":{},"rune":"{}"}}"#, self.pid, self.ready_ms, env!("CARGO_PKG_VERSION"))
    }

    /// Writes the file into `dir`, made 0700 if it's missing, through a temporary file renamed
    /// over it. Nothing without a `dir`.
    fn write_in(&self, dir: Option<&OsStr>) -> Option<std::io::Result<PathBuf>> {
        let dir = Path::new(dir.filter(|dir| !dir.is_empty())?);
        Some((|| {
            let mut builder = std::fs::DirBuilder::new();
            builder.recursive(true);
            #[cfg(unix)]
            std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
            builder.create(dir)?;
            let name = format!("{}.{}", self.bundle_id.replace(['/', '\\'], "_"), self.pid);
            let path = dir.join(&name);
            let temp = dir.join(format!(".{name}.tmp"));
            let mut file = std::fs::File::create(&temp)?;
            file.write_all(format!("{}\n", self.json()).as_bytes())?;
            drop(file);
            std::fs::rename(&temp, &path).inspect_err(|_| { let _ = std::fs::remove_file(&temp); })?;
            Ok(path)
        })())
    }
}

/// The main bundle's identifier on macOS, else the executable's name.
fn bundle_id() -> String {
    #[cfg(target_os = "macos")]
    if let Some(id) = main_bundle_id() { return id; }
    std::env::current_exe().ok().and_then(|exe| exe.file_name().map(|name| name.to_string_lossy().into_owned()))
        .unwrap_or_else(|| "rune-app".into())
}

#[cfg(target_os = "macos")]
fn main_bundle_id() -> Option<String> {
    use objc2_foundation_video::NSString;
    use objc2_video::{class, msg_send, rc::Retained, runtime::AnyObject};
    // SAFETY: NSBundle reads, which are thread-safe.
    unsafe {
        let bundle: Option<Retained<AnyObject>> = msg_send![class!(NSBundle), mainBundle];
        let id: Option<Retained<NSString>> = msg_send![&*bundle?, bundleIdentifier];
        id.map(|id| id.to_string()).filter(|id| !id.is_empty())
    }
}

/// Removes the file when the process exits through `exit`, as AppKit's Quit does.
fn on_exit() {
    #[cfg(unix)]
    {
        extern "C" fn at_exit() { clear() }
        // SAFETY: registers a plain function with no captured state.
        unsafe { libc::atexit(at_exit) };
    }
}

#[cfg(test)]
#[path = "ready_signal_tests.rs"]
mod tests;
