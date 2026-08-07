//! First-run bootstrap: make a bare tarball install self-sufficient.
//!
//! On every launch a background thread checks for and installs anything a
//! `./install.sh` run would have set up that a plain binary drop is missing:
//!   - the `xterm-rio`/`rio` terminfo entry (compiled into `~/.terminfo`)
//!   - the Linux desktop launcher + icons (with MIME declarations for
//!     Open With / default-app pickers)
//!   - the Windows Start Menu shortcut, user `PATH`, `App Paths`
//!     registration, and Default Apps / Open With file associations
//!
//! Everything is idempotent (cheap stat checks on repeat launches) and
//! best-effort: failures are logged and never block or fail the launch.

use std::fs;
#[cfg(unix)]
use std::path::Path;
use std::path::PathBuf;

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
    // Development binaries must not rewrite the installed desktop launcher,
    // icon cache, or PATH integration while being tested alongside release.
    if cfg!(debug_assertions) {
        return;
    }
    #[cfg(windows)]
    let windows_exe = install_windows_stack();
    std::thread::Builder::new()
        .name("neoism-bootstrap".into())
        .spawn(move || {
            #[cfg(unix)]
            install_terminfo();
            #[cfg(target_os = "linux")]
            install_desktop_entry();
            #[cfg(windows)]
            install_start_menu_shortcut(windows_exe);
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
const WINDOWS_SETUP_SCRIPT: &str = r##"$ErrorActionPreference = 'Stop';
$exe = @EXE@;
$bin = Split-Path -Parent $exe;
$q = [string][char]34;
function Set-RegValue([string]$Path, [string]$Name, [string]$Value) {
  if (-not (Test-Path $Path)) { New-Item -Path $Path -Force | Out-Null };
  $query = $Name;
  if ($Name -eq '(default)') { $query = '' };
  if (((Get-Item $Path).GetValue($query)) -ne $Value) { Set-ItemProperty -Path $Path -Name $Name -Value $Value }
};
function Normalize-PathEntry([string]$Value) {
  return [Environment]::ExpandEnvironmentVariables($Value).Trim().Trim([char]34).TrimEnd([char]92)
};
$envKey = [Microsoft.Win32.Registry]::CurrentUser.CreateSubKey('Environment');
try {
  $rawPath = [string]$envKey.GetValue('Path', '', [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames);
  $target = Normalize-PathEntry $bin;
  $present = $false;
  foreach ($entry in ($rawPath -split ';')) { if ((Normalize-PathEntry $entry) -ieq $target) { $present = $true; break } };
  if (-not $present) {
    $nextPath = if ([string]::IsNullOrWhiteSpace($rawPath)) { $bin } else { $rawPath.TrimEnd(';') + ';' + $bin };
    $envKey.SetValue('Path', $nextPath, [Microsoft.Win32.RegistryValueKind]::ExpandString);
    $signature = '[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)] public static extern IntPtr SendMessageTimeout(IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam, uint flags, uint timeout, out UIntPtr result);';
    $native = Add-Type -MemberDefinition $signature -Name 'EnvBroadcast' -Namespace 'NeoismBootstrap' -PassThru;
    $result = [UIntPtr]::Zero;
    [void]$native::SendMessageTimeout([IntPtr]0xFFFF, 0x001A, [UIntPtr]::Zero, 'Environment', 2, 5000, [ref]$result)
  }
} finally { $envKey.Close() };
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
Set-RegValue 'HKCU:\Software\RegisteredApplications' 'Neoism' 'Software\Neoism\Capabilities'"##;

#[cfg(windows)]
fn install_windows_stack() -> Option<PathBuf> {
    let source_exe = std::env::current_exe().ok()?;
    let source_dir = source_exe.parent()?;
    let install_dir = dirs::data_local_dir()?.join("Programs").join("Neoism");
    let install_exe = install_dir.join("neoism.exe");

    let mut installed = source_dir == install_dir;
    if source_dir != install_dir && fs::create_dir_all(&install_dir).is_ok() {
        installed = true;
        for name in [
            "neoism.exe",
            "neoism-workspace-daemon.exe",
            "neoism-agent.exe",
        ] {
            let source = source_dir.join(name);
            let destination = install_dir.join(name);
            if !source.is_file() {
                installed = false;
                continue;
            }
            if let Err(err) = fs::copy(&source, &destination) {
                installed = false;
                tracing::warn!(
                    %err,
                    source = %source.display(),
                    destination = %destination.display(),
                    "bootstrap: failed to seed Windows installation"
                );
            }
        }
    }

    let target = if installed && install_exe.exists() {
        install_exe
    } else {
        source_exe
    };
    if let Some(bin) = target.parent() {
        let current = std::env::var_os("PATH").unwrap_or_default();
        let bin = bin.to_string_lossy();
        let present = std::env::split_paths(&current)
            .any(|entry| entry.to_string_lossy().eq_ignore_ascii_case(&bin));
        if !present {
            let mut paths = vec![PathBuf::from(bin.as_ref())];
            paths.extend(std::env::split_paths(&current));
            if let Ok(path) = std::env::join_paths(paths) {
                std::env::set_var("PATH", path);
            }
        }
    }
    Some(target)
}

#[cfg(windows)]
fn install_start_menu_shortcut(exe: Option<PathBuf>) {
    use std::os::windows::process::CommandExt;

    let Some(exe) = exe else {
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
    let home = dirs::home_dir()
        .map(|home| home.display().to_string())
        .unwrap_or_default();

    // WScript.Shell's CreateShortcut loads an existing .lnk, so this re-points
    // a stale shortcut after an app relocation and no-ops when the target
    // already matches (never clobbering other user-tuned properties). The
    // App Paths handles Win+R while the user PATH makes `neoism` available in
    // new PowerShell/cmd sessions. The document registration puts Neoism into
    // Open With / Default Apps for code and text files.
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
                "bootstrap: installed Start Menu shortcut, user PATH, App Paths, and file associations"
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

/// Quote `value` as a PowerShell single-quoted string literal.
#[cfg(windows)]
fn ps_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
