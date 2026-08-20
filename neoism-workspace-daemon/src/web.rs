//! Optional static web UI served from the daemon HTTP listener.
//!
//! Production "Start Web Server" should open this same origin as
//! `/session` so the browser never has to run `npm` or guess port 7878.

use std::path::{Path, PathBuf};

/// Directory that contains a built `index.html`, if one is installed.
pub fn web_root() -> Option<PathBuf> {
    candidate_web_roots()
        .into_iter()
        .find(|path| path.join("index.html").is_file())
}

pub fn candidate_web_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(value) = std::env::var("NEOISM_WEB_ROOT") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            out.push(PathBuf::from(trimmed));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.push(dir.join("web"));
            if let Some(prefix) = dir.parent() {
                out.push(prefix.join("share/neoism/web"));
            }
        }
    }
    if let Some(data) = dirs::data_local_dir() {
        out.push(data.join("neoism").join("web"));
    }
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".local/share/neoism/web"));
    }
    out.push(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../neoism-frontend/web/dist"),
    );
    out
}
