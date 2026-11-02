//! Explicitly configured static web UI served from the daemon HTTP listener.
//!
//! GPUIX is the workspace client. Only explicit NEOISM_WEB_ROOT and
//! NEOISM_AGENT_GUI_ROOT overrides enable browser asset hosting. Never discover
//! installed resources (web/app and web/native are native runtime files).
//! Phone share serves the standalone Agent GUI from `/agent-gui/` so Vite
//! absolute `/assets` paths can be rewritten to that prefix.

use std::path::PathBuf;

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
    out
}
