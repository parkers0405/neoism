use std::path::PathBuf;

/// Root directory for all Neoism-managed extension state.
///
/// Resolution order: `dirs::data_dir()` (honors `$XDG_DATA_HOME`), then an
/// explicit `$HOME/.local/share` fallback, then a cwd-relative `.neoism`
/// directory as the last resort. No filesystem creation is performed here.
pub fn extensions_dir() -> PathBuf {
    if let Some(data) = dirs::data_dir() {
        return data.join("neoism").join("extensions");
    }
    if let Ok(home) = std::env::var("HOME") {
        return PathBuf::from(home)
            .join(".local")
            .join("share")
            .join("neoism")
            .join("extensions");
    }
    PathBuf::from(".neoism").join("extensions")
}

pub fn installed_root() -> PathBuf {
    extensions_dir().join("installed")
}

pub fn bin_dir() -> PathBuf {
    extensions_dir().join("bin")
}

pub fn staging_dir() -> PathBuf {
    extensions_dir().join("staging")
}

/// Root for Neoism-managed Node.js runtimes. Each pinned version is cached
/// under `node_dir().join(format!("v{VERSION}"))` so npm-based language servers
/// can be installed with zero user setup, Zed-style.
pub fn node_dir() -> PathBuf {
    extensions_dir().join("node")
}

pub fn installed_record_path() -> PathBuf {
    extensions_dir().join("installed.json")
}

pub fn install_dir_for(id: &str) -> PathBuf {
    installed_root().join(id)
}
