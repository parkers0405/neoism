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
    #[cfg(target_os = "linux")]
    std::thread::Builder::new()
        .name("neoism-bootstrap".into())
        .spawn(install_desktop_entry)
        .ok();
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
