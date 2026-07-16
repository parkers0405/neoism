// With the default subsystem, 'console', windows creates an additional console
// window for the program.
// This is silently ignored on non-windows systems.
// See https://msdn.microsoft.com/en-us/library/4cc7ya5b.aspx for more details.
#![windows_subsystem = "windows"]

#[cfg(not(target_arch = "wasm32"))]
mod agent_server;
mod app;
mod bindings;
mod bootstrap;
mod bridges;
mod cli;
mod constants;
mod context;
#[cfg(not(target_arch = "wasm32"))]
mod daemon_client;
#[cfg(not(target_arch = "wasm32"))]
mod discord_presence;
mod editor;
#[cfg(all(unix, not(target_arch = "wasm32")))]
mod embedded_daemon;
mod host;
mod input;
mod ipc;
mod layout;
mod mashup;
mod neoism;
mod notebook_runtime;
#[cfg(windows)]
mod panic;
mod platform;
mod router;
mod screen;
#[cfg(not(target_arch = "wasm32"))]
mod server_registry;
#[cfg(all(unix, not(target_arch = "wasm32")))]
mod ssh_hosts;
#[cfg(not(target_arch = "wasm32"))]
mod tailscale;
mod terminal;
mod workspace;

use base64::{engine::general_purpose, Engine as _};
use clap::Parser;
use neoism_backend::config::config_dir_path;
use neoism_backend::event::EventPayload;
use neoism_backend::{event, performer};
use neoism_terminal_core::ansi;
use std::fs::OpenOptions;
use std::io::{self, Write};
#[cfg(unix)]
use std::os::unix::ffi::OsStrExt;
use std::path::PathBuf;
use std::str::FromStr;
use tracing::level_filters::LevelFilter;
use tracing_subscriber::{
    self, layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer,
};

#[cfg(windows)]
use windows_sys::Win32::System::Console::{
    AttachConsole, FreeConsole, ATTACH_PARENT_PROCESS,
};

const LOG_LEVEL_ENV: &str = "NEOISM_LOG_LEVEL";
const LOG_FILE_ENV: &str = "NEOISM_LOG_FILE";
const SCROLL_LOG_ENV: &str = "NEOISM_SCROLL_LOG";
const RUST_LOG_ENV: &str = "RUST_LOG";

fn emit_neoism_osc(action: &str, payload: Option<&str>) -> io::Result<()> {
    let mut stdout = io::stdout().lock();
    let tmux = std::env::var_os("TMUX").is_some();

    match (tmux, payload) {
        (true, Some(payload)) => write!(
            stdout,
            "\x1bPtmux;\x1b]777;neoism;{action};{payload}\x07\x1b\\"
        ),
        (true, None) => write!(stdout, "\x1bPtmux;\x1b]777;neoism;{action}\x07\x1b\\"),
        (false, Some(payload)) => {
            write!(stdout, "\x1b]777;neoism;{action};{payload}\x07")
        }
        (false, None) => write!(stdout, "\x1b]777;neoism;{action}\x07"),
    }
}

fn run_neoism_terminal_command() -> Result<bool, Box<dyn std::error::Error>> {
    if std::env::var_os("NEOISM").as_deref() != Some(std::ffi::OsStr::new("1")) {
        return Ok(false);
    }

    if let Some(argv0) = std::env::args_os().next() {
        // Inside a Neoism pane, a bare `neoism` acts like a lightweight
        // editor command. Explicit paths (`./target/debug/neoism`,
        // `/usr/bin/neoism`) are treated as real app launches so testing
        // the freshly-built binary from an embedded terminal works.
        if std::path::Path::new(&argv0).components().count() > 1 {
            return Ok(false);
        }
    }

    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.iter().any(|arg| {
        matches!(arg.to_str(), Some("-h" | "--help" | "-V" | "--version"))
            || arg == ipc::NEW_WINDOW_ARG
    }) {
        return Ok(false);
    }

    if args.is_empty() {
        emit_neoism_osc("new", None)?;
        return Ok(true);
    }

    let cwd = std::env::current_dir()?;
    for arg in args {
        let path = PathBuf::from(arg);
        let path = if path.is_absolute() {
            path
        } else {
            cwd.join(path)
        };

        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        if !path.exists() {
            OpenOptions::new()
                .create(true)
                .write(true)
                .truncate(false)
                .open(&path)?;
        }

        let encoded = general_purpose::STANDARD.encode(path.to_string_lossy().as_bytes());
        emit_neoism_osc("open", Some(&encoded))?;
    }

    Ok(true)
}

fn run_workspace_notes_command() -> Result<bool, Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1).collect::<Vec<_>>();
    if args.first().and_then(|arg| arg.to_str()) != Some("notes") {
        return Ok(false);
    }
    args.remove(0);
    if args.is_empty() {
        return Err(notes_usage().into());
    }

    let json = take_flag(&mut args, "--json");
    let workspace = take_option(&mut args, "--workspace")
        .map(PathBuf::from)
        .unwrap_or(std::env::current_dir()?);
    let limit = take_option(&mut args, "--limit")
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(100);
    let command = args
        .first()
        .and_then(|arg| arg.to_str())
        .ok_or_else(notes_usage)?
        .to_string();
    args.remove(0);

    let graph = workspace::NoteGraph::open(&workspace)?;
    let limit = workspace::NoteQueryLimit(limit);

    match command.as_str() {
        "reindex" => {
            graph.reindex()?;
            print_note_result(
                json,
                &serde_json::json!({
                    "workspace": graph.workspace().root,
                    "dbPath": graph.db_path(),
                }),
                format!("Reindexed notes: {}", graph.db_path().display()),
            )?;
        }
        "update" => {
            let path = required_arg(&args, "neoism notes update <path>")?;
            graph.replace_file(path)?;
            print_note_result(
                json,
                &serde_json::json!({ "updated": path }),
                format!("Updated note graph row: {path}"),
            )?;
        }
        "remove" => {
            let path = required_arg(&args, "neoism notes remove <path>")?;
            graph.remove_file(path)?;
            print_note_result(
                json,
                &serde_json::json!({ "removed": path }),
                format!("Removed note graph row: {path}"),
            )?;
        }
        "repair-move" => {
            if args.len() < 2 {
                return Err(
                    "usage: neoism notes repair-move <old-path> <new-path>".into()
                );
            }
            let old_path = args[0].to_string_lossy();
            let new_path = args[1].to_string_lossy();
            let report = graph.repair_moved_note(old_path.as_ref(), new_path.as_ref())?;
            print_note_result(
                json,
                &report,
                format!(
                    "Repaired {} links in {} files",
                    report.links_changed, report.files_changed
                ),
            )?;
        }
        "create" => {
            let title = args
                .iter()
                .filter_map(|arg| arg.to_str())
                .collect::<Vec<_>>()
                .join(" ");
            if title.trim().is_empty() {
                return Err("usage: neoism notes create <title>".into());
            }
            let note = graph.create_note(&title)?;
            print_note_result(json, &note, note.path.clone())?;
        }
        "list" => {
            let notes = graph.notes(limit)?;
            print_note_result(
                json,
                &notes,
                notes
                    .iter()
                    .map(|note| note.path.clone())
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "headings" => {
            let note = args.first().and_then(|arg| arg.to_str());
            let headings = graph.headings(note, limit)?;
            print_note_result(
                json,
                &headings,
                headings
                    .iter()
                    .map(|heading| {
                        format!("{}:{} {}", heading.path, heading.line, heading.text)
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "links" => {
            let links = graph.links(false, limit)?;
            print_note_result(json, &links, render_links(&links))?;
        }
        "unresolved" => {
            let links = graph.links(true, limit)?;
            print_note_result(json, &links, render_links(&links))?;
        }
        "backlinks" => {
            let target = required_arg(&args, "neoism notes backlinks <note>")?;
            let links = graph.backlinks(target, limit)?;
            print_note_result(json, &links, render_links(&links))?;
        }
        "tags" => {
            let tags = graph.tags(limit)?;
            print_note_result(
                json,
                &tags,
                tags.iter()
                    .map(|tag| format!("#{} {}", tag.tag, tag.count))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "tasks" => {
            let checked = match args.first().and_then(|arg| arg.to_str()) {
                Some("open") | None => Some(false),
                Some("done") => Some(true),
                Some("all") => None,
                Some(_) => return Err("usage: neoism notes tasks [open|done|all]".into()),
            };
            let tasks = graph.tasks(checked, limit)?;
            print_note_result(
                json,
                &tasks,
                tasks
                    .iter()
                    .map(|task| {
                        format!(
                            "{}:{} - [{}] {}",
                            task.path,
                            task.line,
                            if task.checked { "x" } else { " " },
                            task.text
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "task-toggle" => {
            if args.len() < 2 {
                return Err(
                    "usage: neoism notes task-toggle <path> <line> [toggle|open|done]"
                        .into(),
                );
            }
            let path = args[0].to_string_lossy();
            let line = args[1]
                .to_string_lossy()
                .parse::<usize>()
                .map_err(|_| "task-toggle line must be a positive integer")?;
            let checked = match args.get(2).and_then(|arg| arg.to_str()) {
                Some("done") => Some(true),
                Some("open") => Some(false),
                Some("toggle") | None => None,
                Some(_) => return Err(
                    "usage: neoism notes task-toggle <path> <line> [toggle|open|done]"
                        .into(),
                ),
            };
            let task = graph.toggle_task(path.as_ref(), line, checked)?;
            print_note_result(
                json,
                &task,
                format!(
                    "{}:{} - [{}] {}",
                    task.path,
                    task.line,
                    if task.checked { "x" } else { " " },
                    task.text
                ),
            )?;
        }
        "properties" => {
            let note = args.first().and_then(|arg| arg.to_str());
            let properties = graph.properties(note, limit)?;
            print_note_result(
                json,
                &properties,
                properties
                    .iter()
                    .map(|property| {
                        format!(
                            "{} {}={} ({})",
                            property.path,
                            property.key,
                            property.value,
                            property.value_type
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "search" => {
            let query = args
                .iter()
                .filter_map(|arg| arg.to_str())
                .collect::<Vec<_>>()
                .join(" ");
            if query.trim().is_empty() {
                return Err("usage: neoism notes search <query>".into());
            }
            let hits = graph.search(&query, limit)?;
            print_note_result(
                json,
                &hits,
                hits.iter()
                    .map(|hit| format!("{}:{} {}", hit.path, hit.start_line, hit.text))
                    .collect::<Vec<_>>()
                    .join("\n"),
            )?;
        }
        "graph" => {
            let graph_summary = graph.graph(limit)?;
            print_note_result(
                json,
                &graph_summary,
                format!(
                    "{} notes, {} links",
                    graph_summary.nodes.len(),
                    graph_summary.edges.len()
                ),
            )?;
        }
        "watch" => {
            let _watcher = graph.watch()?;
            print_note_result(
                json,
                &serde_json::json!({
                    "watching": graph.workspace().root,
                    "dbPath": graph.db_path(),
                }),
                format!("Watching notes at {}", graph.workspace().root.display()),
            )?;
            loop {
                std::thread::park();
            }
        }
        _ => return Err(notes_usage().into()),
    }

    Ok(true)
}

fn notes_usage() -> String {
    "usage: neoism notes <reindex|update|remove|repair-move|create|list|headings|links|unresolved|backlinks|tags|tasks|task-toggle|properties|search|graph|watch> [args] [--workspace PATH] [--limit N] [--json]".to_string()
}

fn take_flag(args: &mut Vec<std::ffi::OsString>, flag: &str) -> bool {
    let Some(index) = args.iter().position(|arg| arg == flag) else {
        return false;
    };
    args.remove(index);
    true
}

fn take_option(args: &mut Vec<std::ffi::OsString>, flag: &str) -> Option<String> {
    let index = args.iter().position(|arg| arg == flag)?;
    args.remove(index);
    if index >= args.len() {
        return None;
    }
    Some(args.remove(index).to_string_lossy().into_owned())
}

fn required_arg<'a>(
    args: &'a [std::ffi::OsString],
    usage: &str,
) -> Result<&'a str, Box<dyn std::error::Error>> {
    args.first()
        .and_then(|arg| arg.to_str())
        .ok_or_else(|| usage.to_string().into())
}

fn print_note_result<T: serde::Serialize>(
    json: bool,
    value: &T,
    plain: String,
) -> Result<(), Box<dyn std::error::Error>> {
    if json {
        println!("{}", serde_json::to_string_pretty(value)?);
    } else if !plain.is_empty() {
        println!("{plain}");
    }
    Ok(())
}

fn render_links(links: &[workspace::query::LinkSummary]) -> String {
    links
        .iter()
        .map(|link| {
            let target = link.target_path.as_deref().unwrap_or(link.target.as_str());
            format!("{}:{} -> {}", link.source_path, link.source_line, target)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn setup_environment_variables(config: &neoism_backend::config::Config) {
    merge_shell_toolchain_environment();

    #[cfg(unix)]
    {
        let terminfo = match (
            teletypewriter::terminfo_exists("xterm-rio"),
            teletypewriter::terminfo_exists("rio"),
        ) {
            // In case `xterm-rio` exists we prioritize it
            (true, _) => "xterm-rio",
            // If is only `rio` installed (which was the default for versions under 0.2.27)
            (false, true) => "rio",
            // If none, then fallback to `xterm-256color`
            (false, false) => "xterm-256color",
        };

        let span = tracing::span!(tracing::Level::INFO, "setup_environment_variables");
        let _guard = span.enter();
        tracing::info!("terminfo: {terminfo}");
        std::env::set_var("TERM", terminfo);
    }

    // https://github.com/raphamorim/rio/issues/200
    std::env::set_var("TERM_PROGRAM", "neoism");
    std::env::set_var("TERM_PROGRAM_VERSION", env!("CARGO_PKG_VERSION"));
    std::env::set_var("NEOISM", "1");

    std::env::set_var("COLORTERM", "truecolor");
    std::env::remove_var("DESKTOP_STARTUP_ID");
    std::env::remove_var("XDG_ACTIVATION_TOKEN");
    #[cfg(target_os = "macos")]
    {
        platform::macos::set_locale_environment();
        std::env::set_current_dir(dirs::home_dir().unwrap()).unwrap();
    }

    // Set env vars from config.
    for env_config in config.env_vars.iter() {
        let env_vec: Vec<&str> = env_config.split('=').collect();

        if env_vec.len() == 2 {
            std::env::set_var(env_vec[0], env_vec[1]);
        }
    }
}

#[cfg(target_os = "linux")]
fn configure_wayland_frame_pacing(config: &neoism_backend::config::Config) {
    use neoism_backend::config::renderer::Backend;

    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    if std::env::var_os("NEOISM_WAYLAND_FRAME_PACING").is_some()
        || std::env::var_os("NEOISM_WAYLAND_NO_FRAME_CALLBACK_THROTTLE").is_some()
    {
        return;
    }
    if config.renderer.use_cpu {
        return;
    }
    if !matches!(
        &config.renderer.backend,
        Backend::Automatic | Backend::Vulkan
    ) {
        return;
    }

    // Automatic and explicit Vulkan both select Sugarloaf's native
    // Vulkan backend on Linux. Keep Wayland frame-callback pacing
    // enabled by default; disabling it globally made AMD/Wayland
    // systems lose normal redraw pacing during sustained UI animation.
    // The explicit env overrides above still allow local NVIDIA
    // experiments without changing runtime behavior for everyone.
    tracing::debug!(
        target: "neoism::wayland",
        "keeping Wayland frame-callback pacing enabled for native Vulkan"
    );
}

#[cfg(not(target_os = "linux"))]
fn configure_wayland_frame_pacing(_config: &neoism_backend::config::Config) {}

#[cfg(unix)]
fn merge_shell_toolchain_environment() {
    if std::env::var_os("NEOISM_SKIP_SHELL_ENV").is_some() {
        return;
    }

    let shell = std::env::var_os("SHELL")
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| "/bin/sh".into());
    let Ok(output) = std::process::Command::new(&shell)
        .args(["-lc", "env -0"])
        .output()
    else {
        tracing::debug!(?shell, "could not read shell environment");
        return;
    };
    if !output.status.success() {
        tracing::debug!(
            ?shell,
            status = ?output.status,
            "shell environment command failed"
        );
        return;
    }

    const KEYS: &[&str] = &[
        "PATH",
        "CARGO_HOME",
        "RUSTUP_HOME",
        "RUSTUP_TOOLCHAIN",
        "MISE_DATA_DIR",
        "ASDF_DATA_DIR",
        "PYENV_ROOT",
        "NVM_DIR",
    ];

    let mut merged = Vec::new();
    for raw in output.stdout.split(|byte| *byte == 0) {
        if raw.is_empty() {
            continue;
        }
        let Some(eq) = raw.iter().position(|byte| *byte == b'=') else {
            continue;
        };
        let key = String::from_utf8_lossy(&raw[..eq]);
        if !KEYS.contains(&key.as_ref()) {
            continue;
        }
        let value = std::ffi::OsStr::from_bytes(&raw[eq + 1..]);
        std::env::set_var(key.as_ref(), value);
        merged.push(key.to_string());
    }

    if !merged.is_empty() {
        tracing::info!(keys = ?merged, "merged shell toolchain environment");
    }
}

#[cfg(not(unix))]
fn merge_shell_toolchain_environment() {}

fn setup_logs_by_filter_level(
    log_level: &str,
    log_file: bool,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut filter_level = LevelFilter::from_str(log_level).unwrap_or(LevelFilter::OFF);

    if let Ok(data) = std::env::var(LOG_LEVEL_ENV) {
        if !data.is_empty() {
            filter_level = LevelFilter::from_str(&data).unwrap_or(filter_level);
        }
    } else if std::env::var_os(SCROLL_LOG_ENV).is_some()
        && filter_level == LevelFilter::OFF
    {
        filter_level = LevelFilter::INFO;
    }
    if log_file && filter_level == LevelFilter::OFF {
        filter_level = LevelFilter::WARN;
    }

    let filter_directives = std::env::var(RUST_LOG_ENV).unwrap_or_default();
    let stdout_subscriber = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stdout)
        .with_ansi(true)
        .with_filter(
            EnvFilter::builder()
                .with_default_directive(filter_level.into())
                .parse(&filter_directives)?,
        );
    let subscriber = tracing_subscriber::registry().with(stdout_subscriber);

    let mut log_file_path = PathBuf::new();
    if log_file {
        let log_dir_path = config_dir_path().join("log");
        log_file_path = log_dir_path.join("neoism.log");
        std::fs::create_dir_all(&log_dir_path)?;
        let log_file = std::fs::File::create(&log_file_path)?;
        let file_subscriber = tracing_subscriber::fmt::layer()
            .with_file(true)
            .with_line_number(true)
            .with_writer(log_file)
            .with_target(false)
            .with_ansi(false)
            .with_filter(
                EnvFilter::builder()
                    .with_default_directive(filter_level.into())
                    .parse(&filter_directives)?,
            );
        subscriber.with(file_subscriber).init();
    } else {
        subscriber.init();
    }

    let span = tracing::span!(tracing::Level::INFO, "logger");
    let _guard = span.enter();
    tracing::info!("logging level: {filter_level}");
    if log_file {
        tracing::info!("logging to a file: {}", log_file_path.display());
    }
    Ok(())
}

/// Quick connect-only probe of an existing daemon's unix socket. We
/// don't speak the protocol; we just check whether `connect(2)`
/// succeeds. If it does, a daemon is listening and we should attach
/// to it instead of spawning an embedded one. If it doesn't
/// (ECONNREFUSED, ENOENT, ETIMEDOUT, etc.), the path is dead
/// and the caller should spawn an embedded daemon.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn probe_daemon_socket(path: &std::path::Path) -> bool {
    use std::os::unix::net::{SocketAddr, UnixStream};

    if !path.exists() {
        return false;
    }
    let Ok(addr) = SocketAddr::from_pathname(path) else {
        return false;
    };
    UnixStream::connect_addr(&addr).is_ok()
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DaemonMode {
    Explicit,
    ExternalDefaultSocket,
    EmbeddedLocal,
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl DaemonMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Explicit => "explicit",
            Self::ExternalDefaultSocket => "external-default-socket",
            Self::EmbeddedLocal => "embedded-local",
        }
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
struct ResolvedDaemon {
    url: String,
    mode: DaemonMode,
    _embedded: Option<embedded_daemon::EmbeddedDaemonHandle>,
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
impl ResolvedDaemon {
    fn external(url: String, mode: DaemonMode) -> Self {
        Self {
            url,
            mode,
            _embedded: None,
        }
    }

    fn embedded(handle: embedded_daemon::EmbeddedDaemonHandle) -> Self {
        let url = unix_socket_url(handle.socket_path());
        Self {
            url,
            mode: DaemonMode::EmbeddedLocal,
            _embedded: Some(handle),
        }
    }
}

#[cfg(all(unix, not(target_arch = "wasm32")))]
fn unix_socket_url(path: &std::path::Path) -> String {
    format!("unix://{}", path.display())
}

/// Daemon resolution: choose between an explicit `--daemon-url`, an
/// already-running daemon on the default socket, or a fresh embedded
/// one we spawn ourselves. Returns the resolved endpoint plus the
/// embedded handle (if any) so the caller keeps it alive for the
/// duration of the process. Drop shuts the daemon down and unlinks
/// the socket.
#[cfg(all(unix, not(target_arch = "wasm32")))]
fn resolve_daemon(daemon_url: Option<&str>) -> Option<ResolvedDaemon> {
    if let Some(url) = daemon_url {
        tracing::info!(daemon = url, "Attached to external daemon at {url}");
        let resolved = ResolvedDaemon::external(url.to_string(), DaemonMode::Explicit);
        return Some(resolved);
    }

    let default_socket = embedded_daemon::default_socket_path();
    embedded_daemon::ensure_embedded_daemon_token();
    if probe_daemon_socket(&default_socket) {
        let url = unix_socket_url(&default_socket);
        tracing::info!(
            socket = %default_socket.display(),
            "Attached to external daemon at {}",
            default_socket.display(),
        );
        let resolved = ResolvedDaemon::external(url, DaemonMode::ExternalDefaultSocket);
        return Some(resolved);
    }

    match embedded_daemon::EmbeddedDaemonHandle::spawn() {
        Ok(handle) => {
            let resolved = ResolvedDaemon::embedded(handle);
            tracing::info!(
                daemon = resolved.url,
                mode = resolved.mode.as_str(),
                "Started embedded daemon at {}",
                resolved.url,
            );
            Some(resolved)
        }
        Err(error) => {
            tracing::error!(
                %error,
                "failed to spawn embedded daemon; desktop will run without daemon-backed features",
            );
            None
        }
    }
}

/// `neoism update` / `neoism upgrade` — self-update from GitHub Releases.
/// Downloads the same `neoism-<os>-<arch>.tar.gz` the installer uses and
/// swaps the three binaries in place. Returns true if it handled the args.
fn run_self_update_command() -> Result<bool, Box<dyn std::error::Error>> {
    let args = std::env::args_os().skip(1).collect::<Vec<_>>();
    match args.first().and_then(|a| a.to_str()) {
        Some("update" | "upgrade" | "self-update") => {}
        _ => return Ok(false),
    }
    let force = args.iter().any(|a| a.to_str() == Some("--force"));
    if let Err(err) = self_update(force) {
        eprintln!("neoism update failed: {err}");
        std::process::exit(1);
    }
    Ok(true)
}

fn self_update(force: bool) -> Result<(), Box<dyn std::error::Error>> {
    // Public binaries repo (source stays private). Override with NEOISM_REPO.
    let repo =
        std::env::var("NEOISM_REPO").unwrap_or_else(|_| "parkers0405/neoism".to_string());
    let repo = repo.as_str();
    const BINS: [&str; 3] = ["neoism", "neoism-workspace-daemon", "neoism-agent"];

    let goos = match std::env::consts::OS {
        "linux" => "linux",
        "macos" => "darwin",
        other => {
            return Err(
                format!("self-update unsupported on {other} — build from source").into(),
            )
        }
    };
    let goarch = match std::env::consts::ARCH {
        "x86_64" => "x86_64",
        "aarch64" => "aarch64",
        other => return Err(format!("self-update unsupported on arch {other}").into()),
    };

    let current = concat!("v", env!("CARGO_PKG_VERSION"));
    println!("neoism {current} ({goos}/{goarch}) — checking for updates…");

    let api = format!("https://api.github.com/repos/{repo}/releases/latest");
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "-A", "neoism-self-update", &api])
        .output()?;
    if !out.status.success() {
        return Err(
            "could not reach GitHub (need `curl`, and a published release)".into(),
        );
    }
    let latest = serde_json::from_slice::<serde_json::Value>(&out.stdout)
        .ok()
        .and_then(|v| {
            v.get("tag_name")
                .and_then(|t| t.as_str())
                .map(str::to_string)
        })
        .ok_or("no release found on GitHub yet")?;

    if !force && latest == current {
        println!("Already up to date ({current}).");
        return Ok(());
    }
    println!("Updating {current} → {latest}…");

    let asset = format!("neoism-{goos}-{goarch}.tar.gz");
    let url = format!("https://github.com/{repo}/releases/download/{latest}/{asset}");
    let tmp = std::env::temp_dir().join(format!("neoism-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let tarball = tmp.join(&asset);
    let dl = std::process::Command::new("curl")
        .args(["-fsSL", "-o"])
        .arg(&tarball)
        .arg(&url)
        .status()?;
    if !dl.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err(format!("download failed: {url}").into());
    }
    let untar = std::process::Command::new("tar")
        .arg("-xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&tmp)
        .status()?;
    if !untar.success() {
        let _ = std::fs::remove_dir_all(&tmp);
        return Err("extract failed".into());
    }

    // Swap the binaries next to the running exe, atomically (stage in the
    // same dir → same filesystem → rename works even over the running bin).
    let exe = std::env::current_exe()?;
    let dir = exe
        .parent()
        .ok_or("cannot resolve install directory")?
        .to_path_buf();
    let extracted = tmp.join(format!("neoism-{goos}-{goarch}"));
    for bin in BINS {
        let src = extracted.join(bin);
        if !src.exists() {
            return Err(format!("`{bin}` missing from {asset}").into());
        }
        let dst = dir.join(bin);
        let staged = dir.join(format!(".{bin}.new"));
        std::fs::copy(&src, &staged).map_err(|e| {
            format!(
                "cannot write to {} ({e}) — try `sudo neoism update`",
                dir.display()
            )
        })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))?;
        }
        std::fs::rename(&staged, &dst)?;
        println!("  ✓ {}", dst.display());
    }

    // Carry the bundled tree-sitter runtime forward too: the first-run
    // bootstrap installs from `runtime/` next to the exe, so an update that
    // only swaps binaries would leave parsers pinned to the old build.
    let runtime_src = extracted.join("runtime");
    if runtime_src.is_dir() {
        let runtime_dst = dir.join("runtime");
        let staged = dir.join(".runtime.new");
        let _ = std::fs::remove_dir_all(&staged);
        let mut copied = 0usize;
        match crate::bootstrap::copy_tree(&runtime_src, &staged, &mut copied) {
            Ok(()) => {
                let _ = std::fs::remove_dir_all(&runtime_dst);
                match std::fs::rename(&staged, &runtime_dst) {
                    Ok(()) => println!("  ✓ {}", runtime_dst.display()),
                    Err(err) => eprintln!("  ! runtime bundle skipped: {err}"),
                }
            }
            Err(err) => {
                let _ = std::fs::remove_dir_all(&staged);
                eprintln!("  ! runtime bundle skipped: {err}");
            }
        }
    }

    let _ = std::fs::remove_dir_all(&tmp);
    println!("Updated to {latest}. Restart neoism to use it.");
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    #[cfg(windows)]
    panic::attach_handler();

    // When linked with the windows subsystem windows won't automatically attach
    // to the console of the parent process, so we do it explicitly. This fails
    // silently if the parent has no console.
    #[cfg(windows)]
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }

    if run_self_update_command()? {
        return Ok(());
    }

    if run_workspace_notes_command()? {
        return Ok(());
    }

    if run_neoism_terminal_command()? {
        return Ok(());
    }

    // Load command line options.
    let args = cli::Cli::parse();
    let terminal_options = &args.window_options.terminal_options;
    let mut initial_open_paths = terminal_options.open_paths.clone();
    initial_open_paths.extend(terminal_options.paths.clone());

    let can_forward_to_running_instance =
        args.daemon_url.is_none() && args.ssh_host.is_none();
    let should_forward_to_running_instance = can_forward_to_running_instance
        && (terminal_options.new_window
            || (terminal_options.command.is_empty()
                && terminal_options.write_config.is_none()
                && !terminal_options.enable_log_file
                && terminal_options.title_placeholder.is_none()
                && terminal_options.app_id.is_none()));
    if should_forward_to_running_instance {
        let launch_cwd = std::env::current_dir().ok();
        let ipc_working_dir = terminal_options
            .working_dir
            .as_ref()
            .and_then(|working_dir| {
                let path = std::path::PathBuf::from(working_dir);
                let path = if path.is_absolute() {
                    path
                } else {
                    launch_cwd.as_ref()?.join(path)
                };
                path.is_dir().then_some(path)
            })
            .or_else(|| launch_cwd.clone());
        let ipc_open_paths = initial_open_paths
            .iter()
            .map(|path| {
                if path.is_absolute() {
                    path.clone()
                } else if let Some(cwd) = launch_cwd.as_ref() {
                    cwd.join(path)
                } else {
                    path.clone()
                }
            })
            .collect();
        if ipc::request_new_window_with_options(ipc_working_dir, ipc_open_paths)? {
            return Ok(());
        }
    }

    let write_config_path = args.window_options.terminal_options.write_config.clone();
    if let Some(config_path) = write_config_path {
        let _ = setup_logs_by_filter_level("TRACE", false);
        neoism_backend::config::create_config_file(config_path);
        return Ok(());
    }

    let (mut config, config_error) = match neoism_backend::config::Config::try_load() {
        Ok(config) => (config, None),
        // First launch: write the default config silently and continue as a
        // normal terminal window. Routing this through the error report used
        // to flip the first window to the legacy Rio welcome screen, and the
        // daemon would then materialize a second window because it never
        // adopts a Welcome-routed one — two windows on a fresh machine.
        Err(neoism_backend::config::ConfigError::PathNotFound) => {
            neoism_backend::config::create_config_file(None);
            // First launch only: drop a zero-byte marker next to the config
            // so the Screen auto-opens the notes sidebar (Welcome/ expanded)
            // exactly once this session. Best-effort — a failed marker just
            // means the first-run reveal is skipped; existing users (config
            // already present) never reach this arm and so never get it.
            let marker =
                neoism_backend::config::config_dir_path().join(".notes-welcome-pending");
            if let Some(parent) = marker.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::File::create(&marker);
            (neoism_backend::config::Config::default(), None)
        }
        Err(err) => (neoism_backend::config::Config::default(), Some(err)),
    };

    // Read platform property and overwrite values per OS
    //
    // [shell]
    // # default (in this case will be used on MacOS/Linux)
    // program = "/bin/fish"
    // args = ["--login"]
    //
    // [platform]
    // # Microsoft Windows overwrite
    // windows.shell.program = "pwsh"
    // windows.shell.args = ["-l"]
    config.overwrite_based_on_platform();

    {
        let log_to_file = args.window_options.terminal_options.enable_log_file
            || std::env::var_os(LOG_FILE_ENV).is_some();
        if let Err(e) = setup_logs_by_filter_level(
            &config.developer.log_level,
            log_to_file || config.developer.enable_log_file,
        ) {
            eprintln!("unable to configure the logger: {e:?}");
        }
        app::freeze_watchdog::init();

        if let Some(command) = args.window_options.terminal_options.command() {
            config.shell = command;
            config.use_fork = false;
        }

        if let Some(working_dir_cli) = args.window_options.terminal_options.working_dir {
            // Use dunce::canonicalize on Windows to avoid UNC paths (\\?\)
            // which break many tools like Neovim and Bun
            #[cfg(target_os = "windows")]
            let canonicalize_fn = dunce::canonicalize;
            #[cfg(not(target_os = "windows"))]
            let canonicalize_fn = std::fs::canonicalize;

            config.working_dir = match canonicalize_fn(&working_dir_cli).and_then(
                |path| {
                    if path.is_dir() {
                        path.into_os_string().into_string().map_err(|_| {
                            std::io::Error::new(
                                std::io::ErrorKind::InvalidData,
                                "Invalid UTF-8 in path",
                            )
                        })
                    } else {
                        Err(std::io::Error::new(
                            std::io::ErrorKind::NotADirectory,
                            "Path is not a directory",
                        ))
                    }
                },
            ) {
                Ok(canonical_path) => Some(canonical_path),
                Err(e) => {
                    tracing::warn!("Failed to set working directory '{}': {}. Using default instead.", working_dir_cli, e);
                    None
                }
            };
        }

        config.title.placeholder = args.window_options.terminal_options.title_placeholder;
    }

    #[cfg(target_os = "linux")]
    {
        // If running inside a flatpak sandbox.
        // Rio will never use use_fork configuration as true
        if std::path::PathBuf::from("/.flatpak-info").exists() {
            config.use_fork = false;
        }
    }

    setup_environment_variables(&config);
    configure_wayland_frame_pacing(&config);
    // Off-thread so first-frame latency never pays for it; everything it
    // installs is only needed once panes/editors are in use.
    bootstrap::spawn();
    #[cfg(not(target_arch = "wasm32"))]
    agent_server::ensure_started();

    // Remote-attach over SSH: if `--ssh-host <alias>` was given, open a
    // loopback `ssh -L` forward to the remote daemon and resolve it to a
    // `ws://127.0.0.1:<port>/session` URL that flows through the normal
    // daemon plumbing below. The `_ssh_attach` guard must outlive
    // `application.run(...)` so the tunnel stays open; dropping it kills
    // the ssh child. On any failure we degrade to the local path.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let mut explicit_daemon_url = args.daemon_url.clone();
    #[cfg(all(windows, not(target_arch = "wasm32")))]
    let explicit_daemon_url = args.daemon_url.clone();
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let _ssh_attach = match args.ssh_host.as_deref() {
        Some(alias) => match ssh_hosts::attach_over_ssh(alias) {
            Ok(attach) => {
                tracing::info!(
                    alias,
                    daemon = %attach.daemon_url,
                    "attached to remote daemon over ssh -L"
                );
                explicit_daemon_url = Some(attach.daemon_url.clone());
                Some(attach)
            }
            Err(err) => {
                tracing::error!(
                    alias,
                    %err,
                    "ssh remote attach failed; falling back to local daemon"
                );
                None
            }
        },
        None => None,
    };

    // Resolve Local Server independently from the initial window's optional
    // explicit endpoint. Fresh windows always use this retained local
    // descriptor, even when the first window was launched with --daemon-url
    // or --ssh-host.
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let local_daemon = resolve_daemon(None);
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let home_daemon_endpoint = local_daemon.as_ref().map(|daemon| daemon.url.clone());
    #[cfg(all(unix, not(target_arch = "wasm32")))]
    let daemon_url = explicit_daemon_url
        .clone()
        .or_else(|| home_daemon_endpoint.clone());
    #[cfg(all(windows, not(target_arch = "wasm32")))]
    let home_daemon_endpoint = Some("ws://127.0.0.1:7878/session".to_string());
    #[cfg(all(windows, not(target_arch = "wasm32")))]
    let daemon_url = explicit_daemon_url
        .clone()
        .or_else(|| home_daemon_endpoint.clone());
    #[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
    let home_daemon_endpoint = None;
    #[cfg(all(not(unix), not(windows), not(target_arch = "wasm32")))]
    let daemon_url = explicit_daemon_url.clone();

    let daemon_token = None;
    let initial_server_id = explicit_daemon_url
        .as_ref()
        .map(|_| "startup-explicit".to_string());

    let window_event_loop =
        neoism_window::event_loop::EventLoop::<EventPayload>::with_user_event()
            .build()?;

    #[cfg(not(target_arch = "wasm32"))]
    discord_presence::start();

    let app_id = args.window_options.terminal_options.app_id;

    let mut application = crate::app::Application::new(
        config,
        config_error,
        &window_event_loop,
        app_id,
        initial_open_paths,
        daemon_url,
        daemon_token,
        initial_server_id,
        home_daemon_endpoint,
    );
    let _ = application.run(window_event_loop);

    #[cfg(windows)]
    unsafe {
        FreeConsole();
    }

    Ok(())
}
