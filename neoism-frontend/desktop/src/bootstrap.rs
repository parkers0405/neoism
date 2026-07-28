//! First-run bootstrap: make a bare tarball install self-sufficient.
//!
//! On every launch a background thread checks for and installs anything a
//! `./install.sh` run would have set up that a plain binary drop is missing:
//!   - the `xterm-rio`/`rio` terminfo entry (compiled into `~/.terminfo`)
//!   - the Linux desktop launcher + icons (with MIME declarations for
//!     Open With / default-app pickers)
//!   - the Windows Start Menu shortcut, `App Paths` registration, and
//!     Default Apps / Open With file associations
//!
//! Everything is idempotent (cheap stat checks on repeat launches) and
//! best-effort: failures are logged and never block or fail the launch.

use std::fs;
use std::path::{Path, PathBuf};

#[cfg(unix)]
const TERMINFO_SOURCE: &str = include_str!("../../../misc/rio.terminfo");
#[cfg(target_os = "linux")]
const DESKTOP_ENTRY: &str = include_str!("../../../misc/neoism.desktop");
#[cfg(target_os = "linux")]
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/neoism.png");
#[cfg(target_os = "linux")]
const ICON_SVG: &str = include_str!("../assets/splash/neoism-wordmark.svg");

/// MIME types the desktop entry declares so file managers offer Neoism in
/// Open With / default-app pickers for code and text files.
#[cfg(target_os = "linux")]
const DESKTOP_MIME_TYPES: &str = "text/markdown;application/json;\
    application/x-yaml;text/x-rust;text/x-python;application/javascript;\
    text/x-go;text/x-c;text/x-c++;application/x-shellscript;application/toml;\
    text/html;text/css;text/plain;application/x-ipynb+json;";

pub fn spawn() {
    std::thread::Builder::new()
        .name("neoism-bootstrap".into())
        .spawn(|| {
            #[cfg(unix)]
            install_terminfo();
            #[cfg(target_os = "linux")]
            install_desktop_entry();
            #[cfg(windows)]
            install_start_menu_shortcut();
        })
        .ok();
}

//  Terminfo

#[cfg(unix)]
fn terminfo_installed() -> bool {
    let mut candidates = vec![
        PathBuf::from("/usr/share/terminfo/x/xterm-rio"),
        PathBuf::from("/usr/lib/terminfo/x/xterm-rio"),
        PathBuf::from("/etc/terminfo/x/xterm-rio"),
    ];
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".terminfo/x/xterm-rio"));
        // Darwin ncurses hashes the leading char as hex ('x' == 0x78).
        candidates.push(home.join(".terminfo/78/xterm-rio"));
    }
    candidates.iter().any(|path| path.is_file())
}

#[cfg(unix)]
fn install_terminfo() {
    if terminfo_installed() {
        return;
    }
    let Some(home) = dirs::home_dir() else {
        return;
    };

    let source_path = std::env::temp_dir().join("neoism-rio.terminfo");
    if fs::write(&source_path, TERMINFO_SOURCE).is_err() {
        return;
    }
    let status = std::process::Command::new("tic")
        .arg("-xe")
        .arg("xterm-rio,rio")
        .arg("-o")
        .arg(home.join(".terminfo"))
        .arg(&source_path)
        .status();
    let _ = fs::remove_file(&source_path);
    match status {
        Ok(status) if status.success() => {
            tracing::info!("bootstrap: installed rio terminfo into ~/.terminfo");
        }
        Ok(status) => {
            tracing::warn!(%status, "bootstrap: tic failed to compile terminfo");
        }
        Err(err) => {
            tracing::warn!(%err, "bootstrap: tic unavailable; skipped terminfo install");
        }
    }
}

//  Desktop launcher + icons (Linux)

#[cfg(target_os = "linux")]
fn install_desktop_entry() {
    // Sandboxed installs manage their own launchers.
    if Path::new("/.flatpak-info").exists() {
        return;
    }
    let Some(data) = dirs::data_local_dir() else {
        return;
    };

    let desktop_path = data.join("applications/neoism.desktop");
    let icons = data.join("icons/hicolor");
    let png_path = icons.join("512x512/apps/neoism.png");
    let svg_path = icons.join("scalable/apps/neoism.svg");
    let mut wrote = false;

    if let Ok(exe) = std::env::current_exe() {
        let exe = exe.display().to_string();
        // `%F` lets file managers hand picked files to the Exec line;
        // `MimeType=` is what surfaces Neoism in Open With / default-app
        // pickers. Both are injected here so the bundled template stays a
        // plain `neoism` launcher.
        let contents = DESKTOP_ENTRY
            .replace("TryExec=neoism\n", &format!("TryExec={exe}\n"))
            .replace(
                "Exec=neoism\n",
                &format!("Exec={exe} %F\nMimeType={DESKTOP_MIME_TYPES}\n"),
            )
            .replace(
                "Exec=neoism --new-window",
                &format!("Exec={exe} --new-window"),
            );
        // Refresh on any drift (app relocation, new MIME declarations); an
        // already-current entry is left untouched.
        let stale = fs::read_to_string(&desktop_path)
            .map(|existing| existing != contents)
            .unwrap_or(true);
        if stale && write_if_dir_creatable(&desktop_path, contents.as_bytes()) {
            wrote = true;
        }
    }
    if !png_path.exists() && write_if_dir_creatable(&png_path, ICON_PNG) {
        wrote = true;
    }
    if !svg_path.exists() && write_if_dir_creatable(&svg_path, ICON_SVG.as_bytes()) {
        wrote = true;
    }

    if wrote {
        tracing::info!("bootstrap: installed desktop launcher and icons");
        let _ = std::process::Command::new("update-desktop-database")
            .arg(data.join("applications"))
            .status();
        let _ = std::process::Command::new("gtk-update-icon-cache")
            .args(["-f", "-t", "--ignore-theme-index"])
            .arg(&icons)
            .status();
    }
}

#[cfg(target_os = "linux")]
fn write_if_dir_creatable(path: &Path, contents: &[u8]) -> bool {
    let Some(parent) = path.parent() else {
        return false;
    };
    if fs::create_dir_all(parent).is_err() {
        return false;
    }
    match fs::write(path, contents) {
        Ok(()) => true,
        Err(err) => {
            tracing::warn!(%err, path = %path.display(), "bootstrap: write failed");
            false
        }
    }
}

//  Start Menu shortcut + App Paths + file associations (Windows)

/// `dwCreationFlags` bit that keeps the helper `powershell` invocation from
/// flashing a console window.
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Extensions mapped to the `Neoism.Document` ProgID via the Default Apps
/// `Capabilities\FileAssociations` registration. Keep in sync with the same
/// list in `install.ps1`.
#[cfg(windows)]
const ASSOC_EXTENSIONS: &[&str] = &[
    ".md",
    ".markdown",
    ".json",
    ".jsonc",
    ".toml",
    ".yaml",
    ".yml",
    ".rs",
    ".py",
    ".js",
    ".jsx",
    ".ts",
    ".tsx",
    ".go",
    ".c",
    ".h",
    ".cpp",
    ".hpp",
    ".cs",
    ".java",
    ".rb",
    ".php",
    ".sh",
    ".ps1",
    ".lua",
    ".html",
    ".css",
    ".scss",
    ".sql",
    ".txt",
    ".log",
    ".ipynb",
    ".neodraw",
];

/// PowerShell run by [`install_start_menu_shortcut`]. `@EXE@` / `@LNK@` /
/// `@HOME@` / `@EXTS@` are substituted with single-quoted values before
/// launch, and newlines are collapsed to spaces. Every registry write is
/// compare-before-set, so repeat runs are no-ops and an exe relocation
/// refreshes every path-bearing value. No literal double quotes anywhere:
/// powershell.exe re-parses its command line and would eat them, so the
/// shell\open\command value builds its quotes from `[char]34`.
///
/// UserChoice per-extension defaults are deliberately NOT written — Windows
/// protects them with a hash; registering Capabilities is what makes Neoism
/// appear in Settings -> Default Apps and Open With.
#[cfg(windows)]
const WINDOWS_SETUP_SCRIPT: &str = r"$ErrorActionPreference = 'Stop';
$exe = @EXE@;
$q = [string][char]34;
function Set-RegValue([string]$Path, [string]$Name, [string]$Value) {
  if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null };
  $query = $Name;
  if ($Name -eq '(default)') { $query = '' };
  if (((Get-Item $Path).GetValue($query)) -ne $Value) { Set-ItemProperty -Path $Path -Name $Name -Value $Value }
};
$shell = New-Object -ComObject WScript.Shell;
$shortcut = $shell.CreateShortcut(@LNK@);
if ($shortcut.TargetPath -ne $exe) { $shortcut.TargetPath = $exe; $shortcut.WorkingDirectory = @HOME@; $shortcut.Save() };
Set-RegValue 'HKCU:\Software\Microsoft\Windows\CurrentVersion\App Paths\neoism.exe' '(default)' $exe;
$doc = 'HKCU:\Software\Classes\Neoism.Document';
Set-RegValue $doc '(default)' 'Neoism Document';
Set-RegValue ($doc + '\DefaultIcon') '(default)' ($exe + ',0');
Set-RegValue ($doc + '\shell\open\command') '(default)' ($q + $exe + $q + ' ' + $q + '%1' + $q);
$caps = 'HKCU:\Software\Neoism\Capabilities';
Set-RegValue $caps 'ApplicationName' 'Neoism';
Set-RegValue $caps 'ApplicationDescription' 'Terminal, code editor, and notes workspace';
foreach ($ext in @(@EXTS@)) { Set-RegValue ($caps + '\FileAssociations') $ext 'Neoism.Document' };
Set-RegValue 'HKCU:\Software\RegisteredApplications' 'Neoism' 'Software\Neoism\Capabilities'";

#[cfg(windows)]
fn install_start_menu_shortcut() {
    use std::os::windows::process::CommandExt;

    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let exe = exe.display().to_string();
    // dirs::config_dir() is %APPDATA% (FOLDERID_RoamingAppData), which hosts
    // the per-user Start Menu.
    let Some(appdata) = dirs::config_dir() else {
        return;
    };
    let lnk: PathBuf =
        appdata.join("Microsoft\\Windows\\Start Menu\\Programs\\Neoism.lnk");
    if shortcut_points_at(&lnk, &exe) {
        // Cheap repeat-launch path: the shortcut targeting the current exe is
        // the proxy for the whole registration (App Paths + associations)
        // being current — everything is written together below, keyed on the
        // same exe path, so staleness of one implies staleness of all.
        return;
    }
    let home = dirs::home_dir()
        .map(|home| home.display().to_string())
        .unwrap_or_default();

    // WScript.Shell's CreateShortcut loads an existing .lnk, so this re-points
    // a stale shortcut after an app relocation and no-ops when the target
    // already matches (never clobbering other user-tuned properties). The
    // App Paths entry makes Win+R / shell `neoism` resolve without PATH
    // edits, and the Neoism.Document class + Capabilities registration puts
    // Neoism into Open With / Default Apps for code and text files.
    let extensions = ASSOC_EXTENSIONS
        .iter()
        .map(|ext| ps_single_quote(ext))
        .collect::<Vec<_>>()
        .join(",");
    let script = WINDOWS_SETUP_SCRIPT
        .replace("@EXE@", &ps_single_quote(&exe))
        .replace("@LNK@", &ps_single_quote(&lnk.display().to_string()))
        .replace("@HOME@", &ps_single_quote(&home))
        .replace("@EXTS@", &extensions)
        .replace("\r\n", " ")
        .replace('\n', " ");

    let status = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command"])
        .arg(&script)
        // `creation_flags` REPLACES any previously-set flags (Command exposes
        // no getter), so this must stay the only flag-setting site for this
        // command.
        .creation_flags(CREATE_NO_WINDOW)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    match status {
        Ok(status) if status.success() => {
            tracing::info!(
                "bootstrap: installed Start Menu shortcut, App Paths, and file associations"
            );
        }
        Ok(status) => {
            tracing::warn!(
                %status,
                "bootstrap: PowerShell failed to install Start Menu shortcut + associations"
            );
        }
        Err(err) => {
            tracing::warn!(
                %err,
                "bootstrap: powershell unavailable; skipped Start Menu shortcut + associations"
            );
        }
    }
}

/// Cheap repeat-launch check: does the existing `.lnk` reference `exe`?
///
/// Shell Link files store the target as an ANSI local base path and/or
/// UTF-16LE string data, so scanning the raw bytes for either encoding of the
/// path avoids shelling out to PowerShell on every launch. A false "stale"
/// verdict only costs one idempotent PowerShell run.
#[cfg(windows)]
fn shortcut_points_at(lnk: &Path, exe: &str) -> bool {
    let Ok(bytes) = fs::read(lnk) else {
        return false;
    };
    let ansi = exe.as_bytes();
    if !ansi.is_empty()
        && bytes
            .windows(ansi.len())
            .any(|window| window.eq_ignore_ascii_case(ansi))
    {
        return true;
    }
    let wide: Vec<u8> = exe.encode_utf16().flat_map(u16::to_le_bytes).collect();
    !wide.is_empty()
        && bytes
            .windows(wide.len())
            .any(|window| window.eq_ignore_ascii_case(&wide))
}

/// Quote `value` as a PowerShell single-quoted string literal.
#[cfg(windows)]
fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
