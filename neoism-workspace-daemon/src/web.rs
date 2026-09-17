//! Optional static web UI served from the daemon HTTP listener.
//!
//! Production "Start Web Server" should open this same origin as
//! `/session` so the browser never has to run `npm` or guess port 7878.
//! Phone share serves the standalone Agent GUI from `/agent-gui/` so Vite
//! absolute `/assets` paths can be rewritten to that prefix.

use std::path::{Path, PathBuf};

/// Directory that contains a built `index.html`, if one is installed.
pub fn web_root() -> Option<PathBuf> {
    candidate_web_roots()
        .into_iter()
        .find(|path| path.join("index.html").is_file())
}

/// Built Agent GUI dist (`index.html` + hashed assets), never the workspace web UI.
pub fn agent_gui_root() -> Option<PathBuf> {
    candidate_agent_gui_roots()
        .into_iter()
        .find(|path| path.join("index.html").is_file())
}

pub fn candidate_agent_gui_roots() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Ok(value) = std::env::var("NEOISM_AGENT_GUI_ROOT") {
        let trimmed = value.trim();
        if !trimmed.is_empty() {
            out.push(PathBuf::from(trimmed));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            out.extend(neoism_agent_server::gui::GuiRoot::installed_candidates(dir));
        }
    }
    if let Some(data) = dirs::data_local_dir() {
        out.push(data.join("neoism").join("web").join("agent-gui"));
    }
    if let Some(home) = dirs::home_dir() {
        out.push(home.join(".local/share/neoism/web/agent-gui"));
    }
    out.push(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../neoism-agent/sdk/typescript/packages/gui/dist"),
    );
    out
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
                // Standard macOS bundle layout: Contents/MacOS/<binary> and
                // Contents/Resources/web/<assets>.
                out.push(prefix.join("Resources/web"));
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
    out.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../neoism-frontend/web/dist"));
    out
}
