use std::cell::RefCell;
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use web_time::Instant;

thread_local! {
    /// Host-seeded directory listings, keyed by absolute dir path.
    /// On wasm `std::fs::read_dir` always fails, so the web host
    /// pre-lists directories through the daemon and stores `(name,
    /// is_dir)` pairs here for Tab completion to consult.
    static HOST_DIR_CACHE: RefCell<HashMap<PathBuf, Vec<(String, bool)>>> =
        RefCell::new(HashMap::new());
    /// Directories a completion attempt missed in the cache. The host
    /// drains these, fetches the listing from the daemon, seeds the
    /// cache, and the next Tab press completes.
    static HOST_DIR_REQUESTS: RefCell<Vec<PathBuf>> = RefCell::new(Vec::new());
}

/// Store a host-resolved directory listing for Tab completion.
pub fn seed_host_dir_listing(dir: PathBuf, entries: Vec<(String, bool)>) {
    HOST_DIR_CACHE.with(|cache| {
        cache.borrow_mut().insert(dir, entries);
    });
}

/// Drain directories Tab completion wanted but had no listing for.
pub fn drain_host_dir_requests() -> Vec<PathBuf> {
    HOST_DIR_REQUESTS.with(|requests| std::mem::take(&mut *requests.borrow_mut()))
}

pub const COMPLETION_LIMIT: usize = 256;
pub const SUCCESS_FLASH_MS: f32 = 280.0;
pub const NO_MATCH_FLASH_MS: f32 = 420.0;
pub const NO_MATCH_SHAKE_AMP: f32 = 4.5;

/// Snapshot of the latest Tab outcome. The composer reads it each
/// frame to decide what to paint; both variants self-expire after their
/// duration runs out (handled by `flash_state()` returning `None`).
#[derive(Debug, Clone, Copy)]
pub enum CompletionFlash {
    /// A completion or suggestion-accept changed the buffer. `range`
    /// is the byte interval (start, end) of newly-inserted bytes —
    /// the composer paints a fading accent highlight over them.
    Success {
        started: Instant,
        range: (usize, usize),
    },
    /// Tab fired but produced no candidates. Composer shakes the
    /// editable text + tints it red for a moment.
    NoMatch { started: Instant },
}

/// Live animation parameters the composer needs each frame. Computed
/// from `CompletionFlash` + elapsed time. `None` once the flash has
/// expired. Re-exported from `crate::input` so call sites in either
/// location see the same canonical type.
pub use crate::input::CompletionFlashState;

#[derive(Debug, Clone)]
pub struct CompletionCandidate {
    pub replacement: String,
    pub label: String,
    pub kind: CompletionKind,
    pub detail: Option<String>,
    pub sort_group: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionKind {
    App,
    Command,
    Directory,
    File,
    Favorite,
    Git,
    History,
    Option,
    Package,
}

#[derive(Debug, Clone)]
pub struct CompletionCycle {
    pub start: usize,
    pub end: usize,
    pub candidates: Vec<CompletionCandidate>,
    pub selected: usize,
}

pub fn completion_candidates(
    token: &str,
    command_prefix: &str,
    command_position: bool,
    directories_only: bool,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let mut candidates = Vec::new();
    if command_position && !token.contains('/') {
        candidates.extend(command_completion_candidates(token));
    }
    let argument_policy = argument_completion_policy(command_prefix);
    candidates.extend(command_argument_completion_candidates(
        token,
        command_prefix,
        directories_only,
        cwd,
    ));
    let git_branch_context = git_branch_completion_context(command_prefix);
    if git_subcommand_completion_context(command_prefix) {
        candidates.extend(git_subcommand_completion_candidates(token));
    }
    if git_branch_context {
        candidates.extend(git_branch_completion_candidates(token, cwd));
    }
    if !git_branch_context && argument_policy.allows_path_completion(token) {
        candidates.extend(path_completion_candidates(token, cwd, directories_only));
    }

    candidates.sort_by(|a, b| {
        a.sort_group.cmp(&b.sort_group).then_with(|| {
            a.replacement
                .to_lowercase()
                .cmp(&b.replacement.to_lowercase())
        })
    });
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.replacement.clone()));
    candidates.truncate(COMPLETION_LIMIT);
    candidates
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArgumentCompletionPolicy {
    Path,
    PathOnlyWhenExplicit,
}

impl ArgumentCompletionPolicy {
    fn allows_path_completion(self, token: &str) -> bool {
        match self {
            Self::Path => true,
            Self::PathOnlyWhenExplicit => token.contains('/') || token.starts_with('.'),
        }
    }
}

fn argument_completion_policy(command_prefix: &str) -> ArgumentCompletionPolicy {
    let tokens = shell_words(command_prefix);
    let command = tokens
        .iter()
        .find(|token| !token.contains('='))
        .map(|token| command_basename(token));
    match command {
        Some(
            "flatpak" | "snap" | "podman" | "docker" | "systemctl" | "journalctl"
            | "nmcli" | "bluetoothctl" | "gh" | "npm" | "pnpm" | "yarn" | "cargo" | "go"
            | "rustup",
        ) => ArgumentCompletionPolicy::PathOnlyWhenExplicit,
        _ => ArgumentCompletionPolicy::Path,
    }
}

fn command_argument_completion_candidates(
    token: &str,
    command_prefix: &str,
    directories_only: bool,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    if directories_only {
        return Vec::new();
    }
    let words = shell_words(command_prefix);
    let Some(command) = words
        .iter()
        .find(|word| !word.contains('='))
        .map(|word| command_basename(word))
    else {
        return Vec::new();
    };
    match command {
        "bun" | "npm" | "pnpm" | "yarn" => {
            package_manager_completion_candidates(command, token, &words, cwd)
        }
        "cargo" => cargo_completion_candidates(token, &words, cwd),
        "docker" | "docker-compose" => {
            docker_completion_candidates(command, token, &words, cwd)
        }
        "flatpak" => flatpak_completion_candidates(token, &words),
        "git" => git_argument_completion_candidates(token, &words, cwd),
        "just" => just_completion_candidates(token, cwd),
        "make" | "gmake" => make_completion_candidates(token, cwd),
        _ => Vec::new(),
    }
}

fn flatpak_completion_candidates(
    token: &str,
    words: &[String],
) -> Vec<CompletionCandidate> {
    let subcommand = words
        .iter()
        .skip(1)
        .find(|word| !word.starts_with('-'))
        .map(String::as_str);
    match subcommand {
        None => {
            completion_words(token, FLATPAK_SUBCOMMANDS, 0, true, CompletionKind::Command)
        }
        Some("run") => {
            let mut candidates = completion_words(
                token,
                FLATPAK_RUN_OPTIONS,
                0,
                false,
                CompletionKind::Option,
            );
            if !token.starts_with('-') {
                candidates.extend(flatpak_app_id_completion_candidates(token));
            }
            candidates
        }
        Some("install") | Some("uninstall") | Some("update") | Some("info")
        | Some("override") => completion_words(
            token,
            FLATPAK_COMMON_OPTIONS,
            0,
            false,
            CompletionKind::Option,
        ),
        _ => completion_words(
            token,
            FLATPAK_COMMON_OPTIONS,
            0,
            false,
            CompletionKind::Option,
        ),
    }
}

fn completion_words(
    token: &str,
    words: &[&str],
    sort_group: u8,
    trailing_space: bool,
    kind: CompletionKind,
) -> Vec<CompletionCandidate> {
    words
        .iter()
        .filter(|word| starts_with_case_insensitive(word, token))
        .map(|word| {
            let replacement = if trailing_space {
                format!("{word} ")
            } else {
                (*word).to_string()
            };
            CompletionCandidate {
                replacement,
                label: (*word).to_string(),
                kind,
                detail: completion_kind_detail(kind, word),
                sort_group,
            }
        })
        .collect()
}

fn flatpak_app_id_completion_candidates(token: &str) -> Vec<CompletionCandidate> {
    flatpak_installed_app_ids()
        .into_iter()
        .filter(|app_id| starts_with_case_insensitive(app_id, token))
        .map(|app_id| CompletionCandidate {
            replacement: app_id.clone(),
            label: app_id.clone(),
            kind: CompletionKind::App,
            detail: Some("Flatpak app id".to_string()),
            sort_group: 1,
        })
        .collect()
}

fn package_manager_completion_candidates(
    command: &str,
    token: &str,
    words: &[String],
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let subcommand = words
        .iter()
        .skip(1)
        .find(|word| !word.starts_with('-'))
        .map(String::as_str);
    match subcommand {
        None => {
            let words = match command {
                "bun" => &["add", "install", "remove", "run", "test", "update"][..],
                "yarn" => &["add", "install", "remove", "run", "test", "upgrade"][..],
                _ => &["ci", "install", "run", "test", "update"][..],
            };
            completion_words(token, words, 0, true, CompletionKind::Command)
        }
        Some("run") | Some("run-script") | Some("exec") if command != "bun" => {
            package_json_script_candidates(token, cwd)
        }
        Some("run") if command == "bun" => package_json_script_candidates(token, cwd),
        _ => Vec::new(),
    }
}

fn package_json_script_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let path = cwd.unwrap_or_else(|| Path::new(".")).join("package.json");
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return Vec::new();
    };
    let Some(scripts) = value.get("scripts").and_then(|value| value.as_object()) else {
        return Vec::new();
    };
    let mut out = scripts
        .iter()
        .filter(|(name, _)| starts_with_case_insensitive(name, token))
        .map(|(name, command)| CompletionCandidate {
            replacement: format!("{name} "),
            label: format!("script {name}"),
            kind: CompletionKind::Package,
            detail: command
                .as_str()
                .map(|command| format!("package.json: {command}")),
            sort_group: 0,
        })
        .collect::<Vec<_>>();
    out.sort_by(|a, b| a.label.to_lowercase().cmp(&b.label.to_lowercase()));
    out
}

fn cargo_completion_candidates(
    token: &str,
    words: &[String],
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let previous = words.last().map(String::as_str);
    match previous {
        Some("--bin") => return cargo_bin_candidates(token, cwd),
        Some("--package") | Some("-p") => return cargo_package_candidates(token, cwd),
        Some("--target") => {
            return completion_words(
                token,
                CARGO_TARGETS,
                0,
                true,
                CompletionKind::Option,
            );
        }
        _ => {}
    }

    let subcommand = words
        .iter()
        .skip(1)
        .find(|word| !word.starts_with('-'))
        .map(String::as_str);
    match subcommand {
        None => {
            completion_words(token, CARGO_SUBCOMMANDS, 0, true, CompletionKind::Command)
        }
        Some("build") | Some("check") | Some("clippy") | Some("run") | Some("test") => {
            if token.starts_with('-') {
                completion_words(
                    token,
                    CARGO_COMMON_OPTIONS,
                    0,
                    false,
                    CompletionKind::Option,
                )
            } else {
                Vec::new()
            }
        }
        _ => Vec::new(),
    }
}

fn cargo_bin_candidates(token: &str, cwd: Option<&Path>) -> Vec<CompletionCandidate> {
    let mut names = BTreeSet::new();
    let root = cwd.unwrap_or_else(|| Path::new("."));
    let manifest = root.join("Cargo.toml");
    if let Ok(text) = std::fs::read_to_string(&manifest) {
        let mut in_bin = false;
        for line in text.lines().map(str::trim) {
            if line == "[[bin]]" {
                in_bin = true;
                continue;
            }
            if line.starts_with('[') && line != "[[bin]]" {
                in_bin = false;
            }
            if in_bin {
                if let Some(name) = quoted_toml_value(line, "name") {
                    names.insert(name);
                }
            }
        }
    }
    for dir in [root.join("src").join("bin"), root.join("bins")] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                if let Some(stem) = path.file_stem().and_then(|stem| stem.to_str()) {
                    names.insert(stem.to_string());
                }
            } else if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                    names.insert(name.to_string());
                }
            }
        }
    }
    names
        .into_iter()
        .filter(|name| starts_with_case_insensitive(name, token))
        .map(|name| CompletionCandidate {
            replacement: format!("{name} "),
            label: format!("bin {name}"),
            kind: CompletionKind::Package,
            detail: Some("Cargo binary target".to_string()),
            sort_group: 0,
        })
        .collect()
}

fn cargo_package_candidates(token: &str, cwd: Option<&Path>) -> Vec<CompletionCandidate> {
    let root = cwd.unwrap_or_else(|| Path::new("."));
    let mut names = BTreeSet::new();
    let mut dir = Some(root);
    while let Some(current) = dir {
        let manifest = current.join("Cargo.toml");
        if let Ok(text) = std::fs::read_to_string(&manifest) {
            if let Some(name) = cargo_package_name(&text) {
                names.insert(name);
            }
        }
        dir = current.parent();
    }
    names
        .into_iter()
        .filter(|name| starts_with_case_insensitive(name, token))
        .map(|name| CompletionCandidate {
            replacement: format!("{name} "),
            label: format!("package {name}"),
            kind: CompletionKind::Package,
            detail: Some("Cargo package".to_string()),
            sort_group: 0,
        })
        .collect()
}

fn cargo_package_name(text: &str) -> Option<String> {
    let mut in_package = false;
    for line in text.lines().map(str::trim) {
        if line == "[package]" {
            in_package = true;
            continue;
        }
        if in_package && line.starts_with('[') {
            return None;
        }
        if in_package {
            if let Some(name) = quoted_toml_value(line, "name") {
                return Some(name);
            }
        }
    }
    None
}

fn quoted_toml_value(line: &str, key: &str) -> Option<String> {
    let line = line.split_once('#').map(|(value, _)| value).unwrap_or(line);
    let (lhs, rhs) = line.split_once('=')?;
    if lhs.trim() != key {
        return None;
    }
    rhs.trim()
        .strip_prefix('"')?
        .split_once('"')
        .map(|(value, _)| value.to_string())
}

fn git_argument_completion_candidates(
    token: &str,
    words: &[String],
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let subcommand = words
        .iter()
        .skip(1)
        .find(|word| !word.starts_with('-'))
        .map(String::as_str);
    match subcommand {
        Some("add") | Some("restore") => git_changed_file_candidates(token, cwd),
        Some("checkout") | Some("switch") | Some("merge") | Some("rebase") => {
            git_branch_completion_candidates(token, cwd)
        }
        _ => Vec::new(),
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn git_changed_file_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let mut command = std::process::Command::new("git");
    command.args(["status", "--porcelain", "--untracked-files=normal"]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let Ok(output) = command.output() else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter_map(|line| line.get(3..))
        .filter_map(|path| {
            path.rsplit_once(" -> ")
                .map(|(_, path)| path)
                .or(Some(path))
        })
        .filter(|path| starts_with_case_insensitive(path, token))
        .map(|path| CompletionCandidate {
            replacement: shell_escape_path(path),
            label: path.to_string(),
            kind: CompletionKind::File,
            detail: Some("Changed git file".to_string()),
            sort_group: 0,
        })
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn git_changed_file_candidates(
    _token: &str,
    _cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    Vec::new()
}

fn make_completion_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let root = cwd.unwrap_or_else(|| Path::new("."));
    let candidates = ["Makefile", "makefile", "GNUmakefile"]
        .iter()
        .map(|file| root.join(file))
        .find(|path| path.is_file());
    let Some(path) = candidates else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    target_candidates_from_lines(token, &text, "Make target")
}

fn just_completion_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let root = cwd.unwrap_or_else(|| Path::new("."));
    let candidates = ["justfile", "Justfile", ".justfile"]
        .iter()
        .map(|file| root.join(file))
        .find(|path| path.is_file());
    let Some(path) = candidates else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    target_candidates_from_lines(token, &text, "Just recipe")
}

fn target_candidates_from_lines(
    token: &str,
    text: &str,
    detail: &str,
) -> Vec<CompletionCandidate> {
    let mut names = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with(|ch: char| ch.is_whitespace())
            || line.starts_with('#')
            || line.starts_with('.')
        {
            continue;
        }
        let Some((name, _)) = line.split_once(':') else {
            continue;
        };
        let name = name.split_whitespace().next().unwrap_or_default();
        if name.is_empty()
            || name.contains('%')
            || name.contains('=')
            || !starts_with_case_insensitive(name, token)
        {
            continue;
        }
        names.insert(name.to_string());
    }
    names
        .into_iter()
        .map(|name| CompletionCandidate {
            replacement: format!("{name} "),
            label: name,
            kind: CompletionKind::Command,
            detail: Some(detail.to_string()),
            sort_group: 0,
        })
        .collect()
}

fn docker_completion_candidates(
    command: &str,
    token: &str,
    words: &[String],
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    if command == "docker" {
        let first = words.iter().skip(1).find(|word| !word.starts_with('-'));
        if first.is_none() {
            return completion_words(
                token,
                &[
                    "build", "compose", "exec", "images", "logs", "ps", "run", "stop",
                ],
                0,
                true,
                CompletionKind::Command,
            );
        }
        if first.map(String::as_str) != Some("compose") {
            return Vec::new();
        }
    }
    let compose_subcommand = words
        .iter()
        .skip(if command == "docker" { 2 } else { 1 })
        .find(|word| !word.starts_with('-'))
        .map(String::as_str);
    match compose_subcommand {
        None => completion_words(
            token,
            &[
                "build", "exec", "logs", "ps", "restart", "run", "stop", "up",
            ],
            0,
            true,
            CompletionKind::Command,
        ),
        Some("exec") | Some("logs") | Some("restart") | Some("run") | Some("stop")
        | Some("up") => docker_compose_service_candidates(token, cwd),
        _ => Vec::new(),
    }
}

fn docker_compose_service_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let root = cwd.unwrap_or_else(|| Path::new("."));
    let path = [
        "compose.yaml",
        "compose.yml",
        "docker-compose.yaml",
        "docker-compose.yml",
    ]
    .iter()
    .map(|file| root.join(file))
    .find(|path| path.is_file());
    let Some(path) = path else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut in_services = false;
    let mut services_indent = 0usize;
    let mut names = BTreeSet::new();
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let indent = line.len() - line.trim_start().len();
        if trimmed == "services:" {
            in_services = true;
            services_indent = indent;
            continue;
        }
        if in_services && indent <= services_indent && trimmed.ends_with(':') {
            in_services = false;
        }
        if in_services
            && indent > services_indent
            && trimmed.ends_with(':')
            && !trimmed.starts_with('-')
        {
            let name = trimmed.trim_end_matches(':');
            if starts_with_case_insensitive(name, token) {
                names.insert(name.to_string());
            }
        }
    }
    names
        .into_iter()
        .map(|name| CompletionCandidate {
            replacement: format!("{name} "),
            label: format!("service {name}"),
            kind: CompletionKind::Package,
            detail: Some("Docker Compose service".to_string()),
            sort_group: 0,
        })
        .collect()
}

#[cfg(not(target_arch = "wasm32"))]
fn shell_escape_path(path: &str) -> String {
    path.split('/')
        .map(shell_escape_path_segment)
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(not(target_arch = "wasm32"))]
fn flatpak_installed_app_ids() -> Vec<String> {
    let Ok(output) = std::process::Command::new("flatpak")
        .args(["list", "--app", "--columns=application"])
        .output()
    else {
        return Vec::new();
    };
    if !output.status.success() {
        return Vec::new();
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(target_arch = "wasm32")]
fn flatpak_installed_app_ids() -> Vec<String> {
    Vec::new()
}

const FLATPAK_SUBCOMMANDS: &[&str] = &[
    "build",
    "build-bundle",
    "build-commit-from",
    "build-export",
    "build-finish",
    "build-import-bundle",
    "build-init",
    "build-sign",
    "build-update-repo",
    "config",
    "create-usb",
    "document-export",
    "document-info",
    "document-unexport",
    "documents",
    "enter",
    "history",
    "info",
    "install",
    "kill",
    "list",
    "make-current",
    "mask",
    "override",
    "permission-remove",
    "permission-reset",
    "permission-set",
    "permissions",
    "pin",
    "ps",
    "remote-add",
    "remote-delete",
    "remote-info",
    "remote-ls",
    "remote-modify",
    "remotes",
    "repair",
    "repo",
    "run",
    "search",
    "spawn",
    "uninstall",
    "update",
];

const FLATPAK_RUN_OPTIONS: &[&str] = &[
    "--arch=",
    "--branch=",
    "--command=",
    "--cwd=",
    "--devel",
    "--die-with-parent",
    "--env=",
    "--env-fd=",
    "--file-forwarding",
    "--help",
    "--log-a11y-bus",
    "--log-session-bus",
    "--no-a11y-bus",
    "--no-documents-portal",
    "--no-session-bus",
    "--runtime=",
    "--share=",
    "--socket=",
    "--system",
    "--talk-name=",
    "--user",
    "--verbose",
    "--version",
];

const FLATPAK_COMMON_OPTIONS: &[&str] = &[
    "--arch=",
    "--assumeyes",
    "--help",
    "--installation=",
    "--noninteractive",
    "--ostree-verbose",
    "--system",
    "--user",
    "--verbose",
    "--version",
];

const CARGO_SUBCOMMANDS: &[&str] = &[
    "bench", "build", "check", "clean", "clippy", "doc", "fmt", "metadata", "new", "run",
    "test", "update",
];

const CARGO_COMMON_OPTIONS: &[&str] = &[
    "--all",
    "--all-features",
    "--bin",
    "--features",
    "--lib",
    "--locked",
    "--no-default-features",
    "--package",
    "--release",
    "--target",
    "--tests",
    "--workspace",
    "-p",
];

const CARGO_TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "aarch64-unknown-linux-gnu",
    "wasm32-unknown-unknown",
    "x86_64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
];

fn command_basename(command: &str) -> &str {
    command
        .rsplit_once('/')
        .map(|(_, name)| name)
        .unwrap_or(command)
}

fn shell_words(text: &str) -> Vec<String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = text.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                current.push(next);
            }
            continue;
        }
        if let Some(active_quote) = quote {
            if ch == active_quote {
                quote = None;
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ' ' | '\t' | '\n' | '\r' => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            '|' | ';' | '&' => {
                current.clear();
                words.clear();
                if ch == '&' && chars.peek() == Some(&'&') {
                    let _ = chars.next();
                }
            }
            _ => current.push(ch),
        }
    }
    if !current.is_empty() {
        words.push(current);
    }
    words
}

pub fn completion_labels(
    candidates: &[CompletionCandidate],
    selected: Option<usize>,
) -> Vec<String> {
    let mixed_sections = candidates
        .windows(2)
        .any(|pair| completion_section(&pair[0]) != completion_section(&pair[1]));
    let mut last_section: Option<&'static str> = None;
    let mut labels = Vec::with_capacity(candidates.len() + 6);
    for (idx, candidate) in candidates.iter().enumerate() {
        let section = completion_section(candidate);
        if mixed_sections && last_section != Some(section) {
            labels.push(format!("§{section}"));
            last_section = Some(section);
        }
        let icon = completion_icon(candidate);
        if Some(idx) == selected {
            labels.push(format!(">{icon} {}", candidate.label));
        } else {
            labels.push(format!(" {icon} {}", candidate.label));
        }
    }
    labels
}

pub fn completion_detail(
    candidates: &[CompletionCandidate],
    selected: Option<usize>,
) -> Option<String> {
    let candidate = selected.and_then(|idx| candidates.get(idx))?;
    candidate.detail.clone().or_else(|| {
        Some(match candidate.kind {
            CompletionKind::App => "Application id".to_string(),
            CompletionKind::Command => "Command".to_string(),
            CompletionKind::Directory => "Directory".to_string(),
            CompletionKind::File => "File".to_string(),
            CompletionKind::Favorite => "Favorite command".to_string(),
            CompletionKind::Git => "Git ref".to_string(),
            CompletionKind::History => "History".to_string(),
            CompletionKind::Option => "Option".to_string(),
            CompletionKind::Package => "Project item".to_string(),
        })
    })
}

fn completion_section(candidate: &CompletionCandidate) -> &'static str {
    match candidate.kind {
        CompletionKind::App => "Apps",
        CompletionKind::Command => "Commands",
        CompletionKind::Directory => "Directories",
        CompletionKind::File => "Files",
        CompletionKind::Favorite => "Favorites",
        CompletionKind::Git => "Git",
        CompletionKind::History => "History",
        CompletionKind::Option => "Options",
        CompletionKind::Package => "Project",
    }
}

fn completion_kind_detail(kind: CompletionKind, word: &str) -> Option<String> {
    Some(match kind {
        CompletionKind::App => "Application id".to_string(),
        CompletionKind::Command => format!("Command `{word}`"),
        CompletionKind::Directory => "Directory".to_string(),
        CompletionKind::File => "File".to_string(),
        CompletionKind::Favorite => "Favorite command".to_string(),
        CompletionKind::Git => "Git item".to_string(),
        CompletionKind::History => "History".to_string(),
        CompletionKind::Option => "Command option".to_string(),
        CompletionKind::Package => "Project item".to_string(),
    })
}

// Completion icons are deliberately category chips (one glyph for all
// code files, media, archives …), NOT the per-language file-tree
// table — merging them would redesign the composer menu. Only the
// semantics the two tables share (folder / default file) consume the
// Mash Up Pack override keys.
fn completion_icon(candidate: &CompletionCandidate) -> &'static str {
    match candidate.kind {
        CompletionKind::App => "\u{f1b2}",
        CompletionKind::Command => "\u{e795}",
        CompletionKind::Directory => {
            crate::primitives::look::themed_glyph("folder", "\u{f07b}")
        }
        CompletionKind::File => file_completion_icon(&candidate.label),
        CompletionKind::Favorite => "\u{f005}",
        CompletionKind::Git => "\u{e725}",
        CompletionKind::History => "\u{f1da}",
        CompletionKind::Option => "\u{f013}",
        CompletionKind::Package => "\u{f487}",
    }
}

fn path_completion_detail(path: &Path, is_dir: bool) -> Option<String> {
    if is_dir {
        let count = std::fs::read_dir(path)
            .ok()
            .map(|entries| entries.count())?;
        return Some(format!(
            "Directory · {}",
            if count == 1 {
                "1 item".to_string()
            } else {
                format!("{count} items")
            }
        ));
    }
    let metadata = std::fs::metadata(path).ok()?;
    Some(format!("File · {}", format_file_size(metadata.len())))
}

fn format_file_size(bytes: u64) -> String {
    const UNITS: &[&str] = &["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0usize;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} {}", UNITS[unit])
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn file_completion_icon(label: &str) -> &'static str {
    let name = label.trim_end_matches('/').to_ascii_lowercase();
    if is_package_file(&name) {
        return "\u{f487}";
    }
    match name.as_str() {
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "\u{e702}",
        ".env" | ".env.local" | ".env.development" | ".env.production" => {
            return "\u{f023}"
        }
        _ => {}
    }

    let Some(extension) = name.rsplit_once('.').map(|(_, extension)| extension) else {
        return crate::primitives::look::themed_glyph("file", "\u{f15b}");
    };
    match extension {
        "7z" | "br" | "bz2" | "gz" | "rar" | "tar" | "tgz" | "xz" | "zip" | "zst" => {
            "\u{f1c6}"
        }
        "bmp" | "gif" | "heic" | "ico" | "jpeg" | "jpg" | "png" | "svg" | "webp" => {
            "\u{f1c5}"
        }
        "aac" | "flac" | "m4a" | "mp3" | "ogg" | "wav" => "\u{f1c7}",
        "avi" | "mkv" | "mov" | "mp4" | "webm" => "\u{f1c8}",
        "db" | "sqlite" | "sqlite3" | "sql" => "\u{f1c0}",
        "lock" => "\u{f023}",
        "md" | "markdown" | "rst" | "txt" => "\u{f15c}",
        "pdf" => "\u{f1c1}",
        "toml" | "json" | "jsonc" | "yaml" | "yml" => "\u{f013}",
        _ if is_code_extension(extension) => "\u{f1c9}",
        _ => crate::primitives::look::themed_glyph("file", "\u{f15b}"),
    }
}

fn is_package_file(name: &str) -> bool {
    matches!(
        name.to_ascii_lowercase().as_str(),
        "cargo.toml"
            | "cargo.lock"
            | "package.json"
            | "package-lock.json"
            | "pnpm-lock.yaml"
            | "yarn.lock"
            | "bun.lock"
            | "bun.lockb"
            | "composer.json"
            | "go.mod"
            | "go.sum"
            | "pyproject.toml"
            | "requirements.txt"
            | "gemfile"
            | "gemfile.lock"
    )
}

fn is_code_extension(extension: &str) -> bool {
    matches!(
        extension,
        "bash"
            | "c"
            | "cc"
            | "clj"
            | "cpp"
            | "cs"
            | "css"
            | "dart"
            | "ex"
            | "exs"
            | "fish"
            | "go"
            | "h"
            | "hpp"
            | "html"
            | "java"
            | "js"
            | "jsx"
            | "kt"
            | "lua"
            | "m"
            | "mm"
            | "php"
            | "pl"
            | "py"
            | "rb"
            | "rs"
            | "scala"
            | "scss"
            | "sh"
            | "swift"
            | "ts"
            | "tsx"
            | "vim"
            | "vue"
            | "zig"
            | "zsh"
    )
}

fn command_completion_candidates(prefix: &str) -> Vec<CompletionCandidate> {
    let Some(path_var) = std::env::var_os("PATH") else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for dir in std::env::split_paths(&path_var) {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if !starts_with_case_insensitive(&name, prefix) {
                continue;
            }
            let Ok(metadata) = entry.metadata() else {
                continue;
            };
            if !is_executable_file(&metadata) {
                continue;
            }
            out.push(CompletionCandidate {
                replacement: name.clone(),
                label: name,
                kind: CompletionKind::Command,
                detail: Some("Command on PATH".to_string()),
                sort_group: 3,
            });
        }
    }
    out
}

fn shell_escape_path_segment(segment: &str) -> String {
    let mut escaped = String::with_capacity(segment.len());
    for ch in segment.chars() {
        if is_shell_path_safe_char(ch) {
            escaped.push(ch);
        } else {
            escaped.push('\\');
            escaped.push(ch);
        }
    }
    escaped
}

fn is_shell_path_safe_char(ch: char) -> bool {
    ch.is_ascii_alphanumeric()
        || matches!(ch, '_' | '-' | '.' | '+' | ',' | ':' | '=' | '@')
}

fn shell_unescape_token(token: &str) -> String {
    let mut unescaped = String::with_capacity(token.len());
    let mut chars = token.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            if let Some(next) = chars.next() {
                unescaped.push(next);
            }
        } else {
            unescaped.push(ch);
        }
    }
    unescaped
}

fn git_subcommand_completion_context(command_prefix: &str) -> bool {
    command_prefix
        .split_whitespace()
        .next()
        .is_some_and(|command| command.eq_ignore_ascii_case("git"))
        && command_prefix.split_whitespace().count() == 1
}

fn git_subcommand_completion_candidates(prefix: &str) -> Vec<CompletionCandidate> {
    const SUBCOMMANDS: &[&str] = &[
        "add", "branch", "checkout", "clone", "commit", "diff", "fetch", "log", "merge",
        "pull", "push", "rebase", "restore", "show", "status", "switch",
    ];
    SUBCOMMANDS
        .iter()
        .filter(|subcommand| starts_with_case_insensitive(subcommand, prefix))
        .map(|subcommand| CompletionCandidate {
            replacement: format!("{subcommand} "),
            label: format!("git {subcommand}"),
            kind: CompletionKind::Git,
            detail: Some("Git subcommand".to_string()),
            sort_group: 0,
        })
        .collect()
}

fn path_completion_candidates(
    token: &str,
    cwd: Option<&Path>,
    directories_only: bool,
) -> Vec<CompletionCandidate> {
    let (dir_token, name_prefix) = split_completion_path(token);
    let dir_lookup = shell_unescape_token(dir_token);
    let name_lookup = shell_unescape_token(name_prefix);
    let base_dir = completion_base_dir(&dir_lookup, cwd);
    let push_candidate =
        |out: &mut Vec<CompletionCandidate>, name: &str, is_dir: bool| {
            if name.starts_with('.') && !name_lookup.starts_with('.') {
                return;
            }
            let Some(rank) = smart_path_match_rank(name, &name_lookup) else {
                return;
            };
            if directories_only && !is_dir {
                return;
            }
            let label = if is_dir {
                format!("{name}/")
            } else {
                name.to_string()
            };
            let replacement = if is_dir {
                format!("{}{}{}", dir_token, shell_escape_path_segment(name), "/")
            } else {
                format!("{}{}", dir_token, shell_escape_path_segment(name))
            };
            let detail = path_completion_detail(&base_dir.join(name), is_dir);
            out.push(CompletionCandidate {
                replacement,
                label,
                kind: if is_dir {
                    CompletionKind::Directory
                } else if is_package_file(name) {
                    CompletionKind::Package
                } else {
                    CompletionKind::File
                },
                detail,
                sort_group: if is_dir { 1 + rank } else { 2 + rank },
            });
        };

    let mut out = Vec::new();
    match std::fs::read_dir(&base_dir) {
        Ok(entries) => {
            for entry in entries.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false);
                push_candidate(&mut out, &name, is_dir);
            }
        }
        Err(_) => {
            // No local fs (wasm) — consult the host-seeded cache and
            // queue a fetch when the listing isn't there yet.
            let cached =
                HOST_DIR_CACHE.with(|cache| cache.borrow().get(&base_dir).cloned());
            match cached {
                Some(entries) => {
                    for (name, is_dir) in &entries {
                        push_candidate(&mut out, name, *is_dir);
                    }
                }
                None => {
                    HOST_DIR_REQUESTS.with(|requests| {
                        let mut requests = requests.borrow_mut();
                        if !requests.contains(&base_dir) {
                            requests.push(base_dir.clone());
                        }
                    });
                }
            }
        }
    }
    out
}

fn smart_path_match_rank(name: &str, query: &str) -> Option<u8> {
    let query = query.trim();
    if query.is_empty() {
        return Some(0);
    }
    if starts_with_case_insensitive(name, query) {
        return Some(0);
    }
    if path_segment_prefix_match(name, query) {
        return Some(1);
    }
    let name_lower = name.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if name_lower.contains(&query_lower) {
        return Some(2);
    }
    fuzzy_subsequence_match(&name_lower, &query_lower).then_some(3)
}

fn path_segment_prefix_match(name: &str, query: &str) -> bool {
    name.split(|ch: char| matches!(ch, '-' | '_' | '.' | ' '))
        .skip(1)
        .any(|segment| starts_with_case_insensitive(segment, query))
}

fn git_branch_completion_context(command_prefix: &str) -> bool {
    let mut parts = command_prefix.split_whitespace();
    if !parts
        .next()
        .is_some_and(|command| command.eq_ignore_ascii_case("git"))
    {
        return false;
    }
    let Some(subcommand) = parts.find(|part| !part.starts_with('-')) else {
        return false;
    };
    matches!(
        subcommand,
        "checkout" | "switch" | "merge" | "rebase" | "branch" | "show" | "log"
    )
}

fn git_branch_completion_candidates(
    token: &str,
    cwd: Option<&Path>,
) -> Vec<CompletionCandidate> {
    let Some(git_dir) = find_git_dir(cwd.unwrap_or_else(|| Path::new("."))) else {
        return Vec::new();
    };
    let mut branches = Vec::new();
    collect_git_branch_refs(&git_dir.join("refs").join("heads"), "", &mut branches);
    collect_git_branch_refs(&git_dir.join("refs").join("remotes"), "", &mut branches);
    collect_packed_git_branch_refs(&git_dir.join("packed-refs"), &mut branches);

    branches.sort();
    branches.dedup();
    branches
        .into_iter()
        .filter(|branch| starts_with_case_insensitive(branch, token))
        .map(|branch| {
            let is_remote = is_remote_git_branch(&git_dir, &branch);
            CompletionCandidate {
                replacement: branch.clone(),
                label: if is_remote {
                    format!("remote {branch}")
                } else {
                    format!("branch {branch}")
                },
                kind: CompletionKind::Git,
                detail: Some(if is_remote {
                    "Remote git branch".to_string()
                } else {
                    "Local git branch".to_string()
                }),
                sort_group: if is_remote { 1 } else { 0 },
            }
        })
        .collect()
}

fn is_remote_git_branch(git_dir: &Path, branch: &str) -> bool {
    git_dir.join("refs").join("remotes").join(branch).is_file()
        || branch
            .split_once('/')
            .is_some_and(|(remote, _)| matches!(remote, "origin" | "upstream"))
}

fn find_git_dir(start: &Path) -> Option<PathBuf> {
    let mut dir = if start.is_dir() {
        start
    } else {
        start.parent()?
    };
    loop {
        let dot_git = dir.join(".git");
        if dot_git.is_dir() {
            return Some(dot_git);
        }
        if dot_git.is_file() {
            if let Ok(text) = std::fs::read_to_string(&dot_git) {
                if let Some(path) = text.trim().strip_prefix("gitdir:") {
                    let path = Path::new(path.trim());
                    return Some(if path.is_absolute() {
                        path.to_path_buf()
                    } else {
                        dir.join(path)
                    });
                }
            }
        }
        dir = dir.parent()?;
    }
}

fn collect_git_branch_refs(dir: &Path, prefix: &str, out: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name == "HEAD" {
            continue;
        }
        let branch = if prefix.is_empty() {
            name.clone()
        } else {
            format!("{prefix}/{name}")
        };
        if entry.file_type().map(|ty| ty.is_dir()).unwrap_or(false) {
            collect_git_branch_refs(&entry.path(), &branch, out);
        } else {
            out.push(branch);
        }
    }
}

fn collect_packed_git_branch_refs(path: &Path, out: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    for line in text.lines() {
        if line.starts_with('#') || line.starts_with('^') {
            continue;
        }
        let Some(reference) = line.split_whitespace().nth(1) else {
            continue;
        };
        if let Some(branch) = reference.strip_prefix("refs/heads/") {
            out.push(branch.to_string());
        } else if let Some(branch) = reference.strip_prefix("refs/remotes/") {
            if !branch.ends_with("/HEAD") {
                out.push(branch.to_string());
            }
        }
    }
}

fn split_completion_path(token: &str) -> (&str, &str) {
    token
        .rfind('/')
        .map(|idx| token.split_at(idx + 1))
        .unwrap_or(("", token))
}

fn completion_base_dir(dir_token: &str, cwd: Option<&Path>) -> PathBuf {
    let dir = if dir_token.is_empty() { "." } else { dir_token };
    if dir == "~/" || dir == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    let path = Path::new(dir);
    if path.is_absolute() {
        path.to_path_buf()
    } else {
        cwd.unwrap_or_else(|| Path::new(".")).join(path)
    }
}

pub fn starts_with_case_insensitive(value: &str, prefix: &str) -> bool {
    value
        .get(..prefix.len())
        .is_some_and(|start| start.eq_ignore_ascii_case(prefix))
        || value.to_lowercase().starts_with(&prefix.to_lowercase())
}

pub fn history_prefix_match(candidate: &str, query: &str) -> bool {
    query.is_empty() || starts_with_case_insensitive(candidate, query)
}

pub fn history_fuzzy_match(candidate: &str, query: &str) -> bool {
    let query = query.trim();
    if query.is_empty() {
        return true;
    }
    let candidate_lower = candidate.to_ascii_lowercase();
    let query_lower = query.to_ascii_lowercase();
    if candidate_lower.contains(&query_lower) {
        return true;
    }
    fuzzy_subsequence_match(&candidate_lower, &query_lower)
}

fn fuzzy_subsequence_match(candidate_lower: &str, query_lower: &str) -> bool {
    let mut chars = candidate_lower.chars();
    query_lower
        .chars()
        .all(|needle| chars.by_ref().any(|ch| ch == needle))
}

pub fn byte_at_char_column(text: &str, start: usize, end: usize, column: usize) -> usize {
    for (seen, (offset, _)) in text[start..end].char_indices().enumerate() {
        if seen == column {
            return start + offset;
        }
    }
    end
}

pub fn common_prefix_case_insensitive(values: &[&str]) -> String {
    let Some(first) = values.first() else {
        return String::new();
    };
    let mut prefix = first.to_string();
    for value in values.iter().skip(1) {
        while !starts_with_case_insensitive(value, &prefix) {
            let Some((idx, _)) = prefix.char_indices().next_back() else {
                return String::new();
            };
            prefix.truncate(idx);
        }
    }
    prefix
}

#[cfg(unix)]
fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.is_file() && metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(unix))]
fn is_executable_file(metadata: &std::fs::Metadata) -> bool {
    metadata.is_file()
}
