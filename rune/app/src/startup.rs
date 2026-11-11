//! What the process does before and around its first window: where it opens, the files it was
//! asked to open, and bringing up the local workspace daemon when none is running (GPUiX
//! `startup-directory.ts` and `local-startup.ts`). Also the two facts about the machine the
//! app asks for in several places: the home folder and the host name.

use crate::cli::Launch;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// The user's home folder (`HOME` is not set on Windows).
pub fn home() -> String {
    dirs::home_dir().map(|p| p.display().to_string()).unwrap_or_default()
}

/// This machine's name, for the default display name.
pub fn hostname() -> String {
    let from_env = || std::env::var("COMPUTERNAME").or_else(|_| std::env::var("HOSTNAME")).ok();
    let from_files = || std::fs::read_to_string("/proc/sys/kernel/hostname").or_else(|_| std::fs::read_to_string("/etc/hostname")).ok();
    let from_command = || crate::cli::quiet("hostname").output().ok().filter(|o| o.status.success()).map(|o| String::from_utf8_lossy(&o.stdout).into_owned());
    from_files().or_else(from_env).or_else(from_command).map(|name| name.trim().to_owned()).unwrap_or_default()
}

/// A shortcut as this system's keyboard names it: Alt is Option on a Mac.
pub fn keys_label(keys: &str) -> std::borrow::Cow<'_, str> {
    if cfg!(target_os = "macos") && keys.contains("Alt") { keys.replace("Alt", "Option").into() } else { keys.into() }
}

/// Whether a path on the daemon's machine is absolute: `/home/me`, `C:\Users\me` or a UNC
/// share. The daemon may run on Windows whatever this machine is.
pub fn absolute(path: &str) -> bool {
    let bytes = path.as_bytes();
    path.starts_with('/') || path.starts_with("\\\\") || (bytes.len() >= 3 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' && matches!(bytes[2], b'\\' | b'/'))
}

/// A directory a desktop launcher starts programs in rather than one the user chose: the
/// filesystem root, the inside of a macOS app, or (Windows) the system folders and the
/// program's own folder.
fn launcher_directory(dir: &Path) -> bool {
    if dir.parent().is_none() { return cfg!(any(target_os = "macos", windows)); }
    if cfg!(target_os = "macos") && dir.to_string_lossy().contains(".app/Contents") { return true; }
    if cfg!(windows) {
        let same = |other: &Path| other.to_string_lossy().eq_ignore_ascii_case(&dir.to_string_lossy());
        let system = std::env::var_os("SystemRoot").or_else(|| std::env::var_os("WINDIR")).map(PathBuf::from);
        let exe = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
        return system.iter().flat_map(|root| [root.clone(), root.join("System32"), root.join("SysWOW64")]).chain(exe).any(|p| same(&p));
    }
    false
}

/// The folder the first workspace opens in: the one asked for, else the launch directory,
/// with launcher defaults mapped to the home folder (`startupDirectory`).
fn directory(launch: &Launch) -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok();
    let explicit = launch.working_dir.clone()
        .or_else(|| std::env::var_os("NEOISM_LAUNCH_DIRECTORY").map(PathBuf::from))
        .or_else(|| std::env::var_os("NEOISM_WORKSPACE_ROOT").map(PathBuf::from));
    if let Some(dir) = explicit {
        let dir = if dir.is_absolute() { dir } else { cwd.clone().unwrap_or_default().join(dir) };
        return Some(dir.canonicalize().map(strip_verbatim).unwrap_or(dir));
    }
    match cwd {
        Some(dir) if !launcher_directory(&dir) => None,
        _ => dirs::home_dir(),
    }
}

/// `\\?\C:\dir` (what `canonicalize` returns on Windows) as `C:\dir`.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    match path.to_str().and_then(|s| s.strip_prefix(r"\\?\")).filter(|s| !s.starts_with("UNC\\")) {
        Some(plain) => PathBuf::from(plain),
        None => path,
    }
}

static FILES: OnceLock<Vec<PathBuf>> = OnceLock::new();

/// Take the command line's choices: the daemon to use, the folder to open in, the files.
pub fn apply(launch: &Launch) {
    // SAFETY: the process is still single-threaded here, before any window or thread exists.
    if let Some(url) = &launch.daemon { unsafe { std::env::set_var("NEOISM_GPUIX_DAEMON_URL", url) }; }
    if let Some(dir) = directory(launch) { let _ = std::env::set_current_dir(dir); }
    let _ = FILES.set(launch.files.clone());
}

/// The files named on the command line, to open once the first workspace exists.
pub fn files() -> &'static [PathBuf] { FILES.get().map_or(&[], Vec::as_slice) }

fn exe_name(name: &str) -> String { if cfg!(windows) { format!("{name}.exe") } else { name.to_owned() } }

/// The workspace daemon this installation ships: the one named by the environment, else the
/// one beside the executable, else a development build in this checkout.
pub fn daemon_executable() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("NEOISM_GPUIX_WORKSPACE_DAEMON") { return Some(PathBuf::from(path)); }
    let name = exe_name("neoism-workspace-daemon");
    let beside = std::env::current_exe().ok().and_then(|p| p.canonicalize().ok()).and_then(|p| p.parent().map(|d| d.join(&name)));
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let built = ["target/activation", "target/release", "target/debug", "neoism-frontend/gpuix/artifacts"].map(|dir| repo.join(dir).join(&name));
    beside.into_iter().chain(built).find(|p| p.exists())
}

/// Where the local daemon's output goes (`$XDG_STATE_HOME/neoism/daemon.log`).
fn daemon_log() -> Option<std::fs::File> {
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from).or_else(|| dirs::home_dir().map(|h| h.join(".local/state")))?.join("neoism");
    std::fs::create_dir_all(&state).ok()?;
    std::fs::OpenOptions::new().create(true).append(true).open(state.join("daemon.log")).ok()
}

/// The arguments a shared local daemon starts with (`localDaemonArgs`): it picks free ports
/// unless the environment pins them.
fn daemon_args(directory: &str, env: impl Fn(&str) -> bool) -> Vec<String> {
    let mut args = vec!["--workspace".to_owned(), directory.to_owned()];
    if !env("NEOISM_DAEMON_ADDR") && !env("NEOISM_DAEMON_SOCKET") { args.push("--auto-port".into()); }
    if !env("NEOISM_AGENT_SERVER") && !env("NEOISM_SERVER") { args.push("--auto-agent-port".into()); }
    args
}

/// When this process last started a daemon. One window's reconnect loop per workspace would
/// otherwise start one each; the daemon also holds its own ownership lock.
static STARTED: Mutex<Option<Instant>> = Mutex::new(None);
/// A daemon gets this long to publish its endpoint before another start is tried.
const START_GRACE: Duration = Duration::from_secs(if cfg!(windows) { 240 } else { 120 });

/// No local daemon answered: start the bundled one, detached, so it outlives this window and
/// serves the next. A daemon chosen explicitly (`--daemon`, `NEOISM_GPUIX_DAEMON_URL`) is
/// never replaced. Returns what went wrong, if the daemon could not be started.
pub fn ensure_local_daemon() -> Result<(), String> {
    if std::env::var_os("NEOISM_GPUIX_DAEMON_URL").is_some() || std::env::var_os("NEOISM_RUNE_NO_DAEMON").is_some() { return Ok(()); }
    let mut started = STARTED.lock().unwrap();
    if started.is_some_and(|at| at.elapsed() < START_GRACE) { return Ok(()); }
    let binary = daemon_executable().ok_or("The workspace daemon is missing from this installation. Reinstall the complete Neoism bundle.")?;
    let directory = std::env::current_dir().ok().filter(|d| !launcher_directory(d)).or_else(dirs::home_dir).unwrap_or_default();
    let directory = directory.display().to_string();
    let mut command = crate::cli::quiet(&binary);
    command.args(daemon_args(&directory, |key| std::env::var_os(key).is_some()))
        .env("NEOISM_WORKSPACE_ROOT", &directory).current_dir(&directory).stdin(std::process::Stdio::null());
    match (daemon_log(), daemon_log()) {
        (Some(out), Some(err)) => { command.stdout(out).stderr(err); }
        _ => { command.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()); }
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Its own process group: closing the terminal the app was started from, or the app
        // itself, leaves the daemon (and the shells and agents it runs) alone.
        command.process_group(0);
    }
    command.spawn().map_err(|e| format!("Could not start the workspace daemon ({}): {e}", binary.display()))?;
    *started = Some(Instant::now());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shared_daemon_picks_its_ports_unless_the_environment_pins_them() {
        assert_eq!(daemon_args("/w", |_| false), ["--workspace", "/w", "--auto-port", "--auto-agent-port"]);
        assert_eq!(daemon_args("/w", |key| key == "NEOISM_DAEMON_ADDR"), ["--workspace", "/w", "--auto-agent-port"]);
        assert_eq!(daemon_args("/w", |key| key == "NEOISM_SERVER" || key == "NEOISM_DAEMON_SOCKET"), ["--workspace", "/w"]);
    }

    #[test]
    fn a_chosen_folder_wins_and_a_project_folder_is_kept() {
        let here = std::env::temp_dir().canonicalize().unwrap();
        let launch = Launch { working_dir: Some(here.clone()), ..Launch::default() };
        assert_eq!(directory(&launch), Some(strip_verbatim(here)));
        // A project folder is not a launcher default on any system.
        assert!(!launcher_directory(Path::new(env!("CARGO_MANIFEST_DIR"))));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\C:\Users\me")), PathBuf::from(r"C:\Users\me"));
        assert!(absolute("/home/me") && absolute(r"C:\Users\me") && absolute("c:/Users/me") && absolute(r"\\server\share"));
        assert!(!absolute("src/main.rs") && !absolute("C:relative") && !absolute(""));
        assert_eq!(strip_verbatim(PathBuf::from(r"\\?\UNC\server\share")), PathBuf::from(r"\\?\UNC\server\share"));
    }
}
