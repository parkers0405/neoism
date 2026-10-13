//! First-run bootstrap for Unix tarball installs.
//!
//! On every launch a background thread checks for and installs anything a
//! `./install.sh` run would have set up that a plain binary drop is missing:
//!   - the Linux desktop launcher + icons (with MIME declarations for
//!     Open With / default-app pickers)
//! Everything is idempotent (cheap stat checks on repeat launches) and
//! best-effort: failures are logged and never block or fail the launch.

#[cfg(target_os = "linux")]
use std::fs;
#[cfg(target_os = "linux")]
use std::path::Path;

#[cfg(target_os = "linux")]
const DESKTOP_ENTRY: &str = include_str!("../../../misc/neoism.desktop");
#[cfg(target_os = "linux")]
const ICON_PNG: &[u8] = include_bytes!("../assets/icons/neoism.png");

#[cfg(target_os = "linux")]
#[path = "bootstrap_omarchy.rs"]
mod omarchy;

pub fn spawn() {
    // Development binaries must not rewrite the installed desktop launcher,
    // icon cache, or PATH integration while being tested alongside release.
    if cfg!(debug_assertions) {
        return;
    }
    #[cfg(windows)]
    std::thread::Builder::new()
        .name("neoism-protocol-bootstrap".into())
        .spawn(|| {
            let _ = register_windows_protocol();
        })
        .ok();
    #[cfg(target_os = "linux")]
    std::thread::Builder::new()
        .name("neoism-bootstrap".into())
        .spawn(install_desktop_entry)
        .ok();
    // Independent of launcher/path/Flatpak early returns above. Never focus or
    // restart the shell; unavailable sessions are retried on the next launch.
    #[cfg(target_os = "linux")]
    std::thread::Builder::new()
        .name("neoism-omarchy-bootstrap".into())
        .spawn(|| {
            if let Err(error) = omarchy::run_host() {
                tracing::debug!(%error, "Omarchy bootstrap deferred");
            }
        })
        .ok();
}

#[cfg(windows)]
fn register_windows_protocol() -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::System::Registry::*;
    fn wide(value: &std::ffi::OsStr) -> Vec<u16> {
        value.encode_wide().chain(Some(0)).collect()
    }
    fn set(
        key: &str,
        name: Option<&str>,
        value: &std::ffi::OsStr,
    ) -> std::io::Result<()> {
        let key = wide(std::ffi::OsStr::new(key));
        let name = name.map(|name| wide(std::ffi::OsStr::new(name)));
        let value = wide(value);
        unsafe {
            let mut handle = std::ptr::null_mut();
            let result = RegCreateKeyExW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                0,
                std::ptr::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_SET_VALUE,
                std::ptr::null(),
                &mut handle,
                std::ptr::null_mut(),
            );
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(result as i32));
            }
            let result = RegSetValueExW(
                handle,
                name.as_ref().map_or(std::ptr::null(), |name| name.as_ptr()),
                0,
                REG_SZ,
                value.as_ptr().cast(),
                (value.len() * 2) as u32,
            );
            RegCloseKey(handle);
            if result != 0 {
                return Err(std::io::Error::from_raw_os_error(result as i32));
            }
        }
        Ok(())
    }
    let exe = std::env::current_exe()?;
    let mut command = std::ffi::OsString::from("\"");
    command.push(exe.as_os_str());
    command.push("\" \"%1\"");
    set(
        r"Software\Classes\neoism",
        None,
        std::ffi::OsStr::new("URL:Neoism Invite"),
    )?;
    set(
        r"Software\Classes\neoism",
        Some("URL Protocol"),
        std::ffi::OsStr::new(""),
    )?;
    set(
        r"Software\Classes\neoism\shell\open\command",
        None,
        &command,
    )
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
        // Use the packaged MIME list and %U for both local files and URLs.
        let quoted_exe = format!(
            "\"{}\"",
            exe.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('`', "\\`")
                .replace('$', "\\$")
                .replace('%', "%%")
        );
        let contents = DESKTOP_ENTRY
            .replace("TryExec=neoism\n", &format!("TryExec={exe}\n"))
            .replace("Exec=neoism %U\n", &format!("Exec={quoted_exe} %U\n"))
            .replace(
                "Exec=neoism --new-window",
                &format!("Exec={quoted_exe} --new-window"),
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
    let png_stale = fs::read(&png_path)
        .map(|existing| existing != ICON_PNG)
        .unwrap_or(true);
    if png_stale && write_if_dir_creatable(&png_path, ICON_PNG) {
        wrote = true;
    }
    // Older builds installed the splash wordmark under the same icon name.
    // Launchers prefer that scalable asset over the canonical square PNG.
    if svg_path.exists() && fs::remove_file(&svg_path).is_ok() {
        wrote = true;
    }

    // Only fill an absent default; never replace another app's association or
    // rewrite mimeapps.list wholesale. xdg-mime owns desktop-specific placement.
    if desktop_path.exists() {
        let current = std::process::Command::new("xdg-mime")
            .args(["query", "default", "x-scheme-handler/neoism"])
            .output();
        if current.is_ok_and(|output| {
            output.status.success() && output.stdout.iter().all(u8::is_ascii_whitespace)
        }) {
            let _ = std::process::Command::new("xdg-mime")
                .args(["default", "neoism.desktop", "x-scheme-handler/neoism"])
                .status();
        }
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

#[cfg(test)]
mod protocol_tests {
    #[test]
    fn packaged_launchers_accept_urls_and_preserve_file_mimes() {
        let desktop = include_str!("../../../misc/neoism.desktop");
        assert!(desktop.contains("Exec=neoism %U\n"));
        assert!(desktop.contains("x-scheme-handler/neoism;"));
        assert!(desktop.contains("text/markdown;"));
        assert!(desktop.contains("text/plain;"));
        assert!(desktop.contains("Exec=neoism --new-window"));
        let msi = include_str!("../../../misc/windows/neoism.wxs");
        assert!(msi.contains(r#"Key="Software\Classes\neoism""#));
        assert!(msi.contains(r#"Name="URL Protocol" Type="string" Value="""#));
        assert!(msi.contains("&quot;[INSTALLFOLDER]neoism.exe&quot; &quot;%1&quot;"));
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
