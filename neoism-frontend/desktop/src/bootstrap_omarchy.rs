//! Unattended native Omarchy plugin installation. No shell restart or config writes.
//! Also compiled directly by tools/omarchy-installer for small, opt-in host tests.
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::{self, Read, Seek, SeekFrom},
    os::unix::{ffi::OsStrExt, fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

pub const ID: &str = "dev.neoism.agent-status";
const OWNER: &str = ".neoism-owned.json";
const ASSETS: &[(&str, &[u8])] = &[
    (
        "manifest.json",
        include_bytes!("../../../assets/omarchy/dev.neoism.agent-status/manifest.json"),
    ),
    (
        "BarWidget.qml",
        include_bytes!("../../../assets/omarchy/dev.neoism.agent-status/BarWidget.qml"),
    ),
];
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
fn fail(message: &str) -> Box<dyn std::error::Error> {
    io::Error::other(message).into()
}
fn hash(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Reject symlinked parents too: plugin discovery uses HOME, never XDG_CONFIG_HOME.
fn safe_path(path: &Path) -> Result<()> {
    for ancestor in path.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) if meta.file_type().is_symlink() => {
                return Err(fail("symlinked install path"))
            }
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}
fn marker() -> Value {
    let hashes: serde_json::Map<String, Value> = ASSETS
        .iter()
        .map(|(n, b)| (n.to_string(), json!(hash(b))))
        .collect();
    json!({"owner": ID, "format": 1, "hashes": hashes})
}
fn verify_owned(dir: &Path) -> Result<Value> {
    safe_path(dir)?;
    let names = fs::read_dir(dir)?
        .map(|e| e.map(|e| e.file_name()))
        .collect::<io::Result<Vec<_>>>()?;
    // Extra files (including .git), missing files, or symlinks mean user ownership.
    if names.len() != ASSETS.len() + 1 {
        return Err(fail("custom plugin contents; left untouched"));
    }
    for name in &names {
        let path = dir.join(name);
        if !fs::symlink_metadata(&path)?.file_type().is_file() {
            return Err(fail("non-regular plugin asset"));
        }
    }
    let saved: Value = serde_json::from_slice(&fs::read(dir.join(OWNER))?)?;
    if saved["owner"] != ID || saved["format"] != 1 {
        return Err(fail("unowned plugin; left untouched"));
    }
    for (name, _) in ASSETS {
        if saved["hashes"][name] != hash(&fs::read(dir.join(name))?) {
            return Err(fail("locally modified plugin; left untouched"));
        }
    }
    Ok(saved)
}

// Atomic directory publication: NOREPLACE on first install, EXCHANGE on a
// verified owned upgrade. Never remove a user's directory to make rename work.
fn publish(from: &Path, to: &Path, replace: bool) -> Result<()> {
    let from = std::ffi::CString::new(from.as_os_str().as_bytes())?;
    let to = std::ffi::CString::new(to.as_os_str().as_bytes())?;
    let flags = if replace {
        libc::RENAME_EXCHANGE
    } else {
        libc::RENAME_NOREPLACE
    };
    let rc = unsafe {
        libc::renameat2(
            libc::AT_FDCWD,
            from.as_ptr(),
            libc::AT_FDCWD,
            to.as_ptr(),
            flags,
        )
    };
    if rc != 0 {
        return Err(io::Error::last_os_error().into());
    }
    Ok(())
}
fn install_assets(dir: &Path) -> Result<()> {
    safe_path(dir)?;
    let exists = dir.try_exists()?;
    let old = if exists {
        Some(verify_owned(dir)?)
    } else {
        None
    };
    if old.as_ref() == Some(&marker()) {
        return Ok(());
    }
    let parent = dir.parent().ok_or_else(|| fail("missing plugin parent"))?;
    fs::create_dir_all(parent)?;
    let stage = tempfile::Builder::new()
        .prefix(".neoism-stage-")
        .tempdir_in(parent)?;
    for (name, bytes) in ASSETS {
        fs::write(stage.path().join(name), bytes)?;
    }
    fs::write(
        stage.path().join(OWNER),
        serde_json::to_vec_pretty(&marker())?,
    )?;
    if exists && Some(verify_owned(dir)?) != old {
        return Err(fail("plugin changed during installation"));
    }
    publish(stage.path(), dir, exists)?;
    // After EXCHANGE, stage contains only the previously verified owned files.
    Ok(())
}

pub trait Shell {
    fn call(&mut self, args: &[&str]) -> Result<String>;
    // Mock shells may omit disk persistence; LiveShell always checks it.
    fn stored_config(&mut self) -> Result<Option<Value>> {
        Ok(None)
    }
    fn wait(&mut self) {
        thread::sleep(Duration::from_millis(150));
    }
}
struct LiveShell {
    root: PathBuf,
    config_path: PathBuf,
}
impl Shell for LiveShell {
    fn stored_config(&mut self) -> Result<Option<Value>> {
        Ok(Some(serde_json::from_slice(&fs::read(&self.config_path)?)?))
    }
    fn call(&mut self, args: &[&str]) -> Result<String> {
        // A regular temporary file avoids pipe-buffer deadlocks. Also bound the
        // whole wrapper, not just qs, in case a distro script stalls.
        let mut output = tempfile::tempfile()?;
        let executable = self.root.join("bin/omarchy-shell");
        let mut child = Command::new(executable)
            .arg("shell")
            .args(args)
            .env("OMARCHY_PATH", &self.root)
            .env("OMARCHY_SHELL_IPC_TIMEOUT", "2s")
            .stdin(Stdio::null())
            .stdout(output.try_clone()?)
            .stderr(Stdio::null())
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(4);
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(fail("Omarchy IPC timed out"));
            }
            thread::sleep(Duration::from_millis(25));
        };
        if !status.success() {
            return Err(fail("Omarchy shell unavailable"));
        }
        if output.metadata()?.len() > 4 * 1024 * 1024 {
            return Err(fail("oversized shell IPC response"));
        }
        output.seek(SeekFrom::Start(0))?;
        let mut text = String::new();
        output.read_to_string(&mut text)?;
        Ok(text.trim().to_owned())
    }
}
fn ok(shell: &mut impl Shell, args: &[&str]) -> Result<()> {
    let response = shell.call(args)?;
    if response != "ok" {
        return Err(fail(&format!("Omarchy {}: {response}", args[0])));
    }
    Ok(())
}

/// Keep qs/CLI11 from interpreting a bare `[a,b]` argument as an argv list.
/// Without the leading JSON whitespace, ["neoism"] reaches JSON.parse as
/// "neoism" (a string); multiple names become extra IPC arguments.
fn ipc_json(value: &Value) -> String {
    format!(" {value}")
}

fn hidden_matches(config: &Value, section: &str, index: usize, hidden: &Value) -> bool {
    let entry = &config["bar"]["layout"][section][index];
    entry["id"] == "omarchy.tray" && entry["hidden"] == *hidden
}

fn set_hidden(
    shell: &mut impl Shell,
    section: &str,
    index: usize,
    hidden: &Value,
) -> Result<()> {
    let selector = json!({"section": section, "index": index}).to_string();
    ok(
        shell,
        &[
            "setBarWidget",
            "omarchy.tray",
            "hidden",
            &ipc_json(hidden),
            &selector,
        ],
    )?;
    verify_hidden(shell, section, index, hidden)
}

fn verify_hidden(
    shell: &mut impl Shell,
    section: &str,
    index: usize,
    hidden: &Value,
) -> Result<()> {
    // An "ok" reply alone did not catch qs stripping singleton array brackets.
    // Require exact array values in live state and (on the host) persisted JSON.
    for _ in 0..10 {
        let effective: Value = serde_json::from_str(&shell.call(&["listShellConfig"])?)?;
        let stored = shell.stored_config()?;
        if hidden_matches(&effective, section, index, hidden)
            && stored
                .as_ref()
                .is_none_or(|c| hidden_matches(c, section, index, hidden))
        {
            return Ok(());
        }
        shell.wait();
    }
    Err(fail(
        "tray hidden array did not round-trip through IPC/persistence",
    ))
}

/// Uses live config and normalizes CSV legacy values to actual arrays, the only
/// representation honored by Tray.qml. Never writes pinned or unrelated keys.
/// Migration only repairs the exact singleton string our first installer made.
fn hide_generic_duplicate(shell: &mut impl Shell, legacy_repair: bool) -> Result<()> {
    let config: Value = serde_json::from_str(&shell.call(&["listShellConfig"])?)?;
    let layout = config
        .pointer("/bar/layout")
        .and_then(Value::as_object)
        .ok_or_else(|| fail("missing shell bar layout"))?;
    if legacy_repair
        && !layout
            .values()
            .filter_map(Value::as_array)
            .flatten()
            .any(|entry| entry.as_str() == Some(ID) || entry["id"] == ID)
    {
        return Ok(()); // User removed the plugin's bar entry; do not re-hide it.
    }
    for (section, entries) in layout {
        for (index, entry) in entries
            .as_array()
            .ok_or_else(|| fail("invalid bar section"))?
            .iter()
            .enumerate()
        {
            if entry.as_str() != Some("omarchy.tray") && entry["id"] != "omarchy.tray" {
                continue;
            }
            if legacy_repair && entry["hidden"] != json!("neoism") {
                if entry["hidden"]
                    .as_array()
                    .is_some_and(|names| names.contains(&json!("neoism")))
                {
                    verify_hidden(shell, section, index, &entry["hidden"])?;
                }
                continue;
            }
            let mut names = match entry.get("hidden") {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::Array(names)) if names.iter().all(Value::is_string) => {
                    names.clone()
                }
                Some(Value::String(csv)) => csv
                    .split(',')
                    .map(str::trim)
                    .filter(|name| !name.is_empty())
                    .map(|name| json!(name))
                    .collect(),
                _ => return Err(fail("invalid tray hidden names; left untouched")),
            };
            if !names.contains(&json!("neoism")) {
                names.push(json!("neoism"));
            }
            let hidden = Value::Array(names);
            if entry["hidden"] != hidden {
                set_hidden(shell, section, index, &hidden)?;
            } else {
                verify_hidden(shell, section, index, &hidden)?;
            }
        }
    }
    Ok(())
}

fn stamp(path: &Path, bytes: &[u8]) -> Result<()> {
    safe_path(path)?;
    if path.try_exists()? {
        return Ok(());
    }
    let mut marker = tempfile::NamedTempFile::new_in(
        path.parent().ok_or_else(|| fail("missing state parent"))?,
    )?;
    use std::io::Write;
    marker.write_all(bytes)?;
    marker.as_file().sync_all()?;
    marker.persist_noclobber(path)?;
    Ok(())
}

/// Actual installer shared by release bootstrap, mock tests, and opt-in harness.
/// `home` must be the actual user's HOME, not an XDG config directory.
pub fn install(home: &Path, shell: &mut impl Shell) -> Result<&'static str> {
    let dir = home.join(".config/omarchy/plugins").join(ID);
    let state = home.join(".config/neoism/bootstrap/omarchy");
    safe_path(&dir)?;
    safe_path(&state)?;
    fs::create_dir_all(&state)?;
    let lock_path = state.join("install.lock");
    safe_path(&lock_path)?;
    let lock = fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path)?;
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Ok("another installer is running");
    }
    let activated = state.join("activated-v1.json");
    safe_path(&activated)?;
    let done = activated.try_exists()?;
    if done && !dir.try_exists()? {
        return Ok("respecting removed plugin");
    }
    // Require a live plugin-capable shell before touching assets or settings.
    ok(shell, &["ping"])?;
    let _: Vec<Value> = serde_json::from_str(&shell.call(&["listPlugins"])?)?;
    install_assets(&dir)?;
    let array_fix = state.join("hidden-array-v1.json");
    safe_path(&array_fix)?;
    if done {
        if !array_fix.try_exists()? {
            // Only a verified owned plugin + our recognized activation marker
            // authorizes migration. No marker reset, placement, or blanket union.
            let activation: Value = serde_json::from_slice(&fs::read(&activated)?)?;
            if activation == json!({"activation": 1}) {
                hide_generic_duplicate(shell, true)?;
                stamp(&array_fix, b"{\"hidden-array\":1}\n")?;
            }
        }
        return Ok("already activated; preserving user placement/settings");
    }
    shell.call(&["rescanPlugins"])?;
    let mut discovered = false;
    for _ in 0..30 {
        let plugins: Vec<Value> = serde_json::from_str(&shell.call(&["listPlugins"])?)?;
        if plugins.iter().any(|p| p["id"] == ID) {
            discovered = true;
            break;
        }
        shell.wait();
    }
    if !discovered {
        return Err(fail("plugin scan pending; will retry next launch"));
    }
    ok(shell, &["putBarWidget", ID, r#"{"section":"right"}"#])?;
    hide_generic_duplicate(shell, false)?;
    stamp(&array_fix, b"{\"hidden-array\":1}\n")?;
    // Distinct from the asset manifest version. Once activated, a user removing
    // the entry or unhiding the tray icon must never be countermanded on launch.
    stamp(&activated, b"{\"activation\":1}\n")?;
    Ok("installed native Omarchy Neoism widget")
}

pub fn run_host() -> Result<&'static str> {
    if Path::new("/.flatpak-info").exists() || std::env::var_os("FLATPAK_ID").is_some() {
        return Ok("Flatpak: skipped");
    }
    let root = std::env::var_os("OMARCHY_PATH")
        .map(PathBuf::from)
        .filter(|p| p.join("shell/shell.qml").is_file())
        .or_else(|| {
            Path::new("/usr/share/omarchy/shell/shell.qml")
                .is_file()
                .then(|| PathBuf::from("/usr/share/omarchy"))
        });
    let Some(root) = root else {
        return Ok("not an Omarchy shell installation");
    };
    let source = fs::read_to_string(root.join("shell/shell.qml"))?;
    if ![
        "function listPlugins(",
        "function putBarWidget(",
        "function setBarWidget(",
        "function listShellConfig(",
    ]
    .iter()
    .all(|s| source.contains(s))
    {
        return Ok("Omarchy shell lacks plugin installation IPC; skipped");
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| fail("HOME unavailable"))?;
    install(
        &home,
        &mut LiveShell {
            root,
            config_path: home.join(".config/omarchy/shell.json"),
        },
    )
}

#[cfg(test)]
#[path = "bootstrap_omarchy_tests.rs"]
mod tests;
