use std::path::PathBuf;

pub(crate) fn managed_lsp_path_entries() -> Vec<PathBuf> {
    let mut entries = Vec::new();
    #[cfg(target_os = "macos")]
    {
        // GUI apps do not inherit the user's interactive-shell PATH on macOS.
        // npm-installed language-server launchers use `/usr/bin/env node`, so
        // include both standard Homebrew prefixes for the child process too.
        entries.push(PathBuf::from("/opt/homebrew/bin"));
        entries.push(PathBuf::from("/usr/local/bin"));
    }
    #[cfg(windows)]
    {
        // Same story on Windows: GUI processes see a minimal PATH, and the
        // usual language-server homes are global npm shims, scoop shims,
        // and per-user program installs.
        if let Some(appdata) = std::env::var_os("APPDATA") {
            entries.push(PathBuf::from(appdata).join("npm"));
        }
        if let Some(profile) = std::env::var_os("USERPROFILE") {
            entries.push(PathBuf::from(profile).join("scoop").join("shims"));
        }
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            entries.push(PathBuf::from(local).join("Programs"));
        }
    }
    if let Some(home) = home_dir() {
        entries.push(
            home.join(".local")
                .join("share")
                .join("neoism")
                .join("extensions")
                .join("bin"),
        );
        let root = home.join(".local").join("share").join("rio").join("lsp");
        entries.push(root.join("bin"));
        entries.push(root.join("node").join("bin"));
        entries.push(root.join("nix-profile").join("bin"));
        entries.push(home.join(".cargo").join("bin"));
    }
    entries
}

fn home_dir() -> Option<PathBuf> {
    // HOME first so unix (and explicit overrides) keep working; Windows
    // normally leaves it unset, so fall back to the platform lookup there.
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home));
    }
    #[cfg(windows)]
    {
        dirs::home_dir()
    }
    #[cfg(not(windows))]
    {
        None
    }
}

pub(crate) fn managed_lsp_path() -> Option<std::ffi::OsString> {
    let mut paths = managed_lsp_path_entries();
    if let Some(existing) = std::env::var_os("PATH") {
        paths.extend(std::env::split_paths(&existing));
    }
    std::env::join_paths(paths).ok()
}
