use super::*;
use std::path::PathBuf;

#[wasm_bindgen]
impl ChromeBridge {
    // -------- rich side panels (git diff / notes) ----------------
    //
    // Desktop's Alt+G right-side git panel and Alt+N notes
    // sidebar, hosted by the shared `Chrome`. The panels have no
    // IO on wasm — when they open, chrome queues a refresh flag
    // (`take_git_panel_refresh` / `take_notes_refresh`) that the
    // JS host answers by fetching from the daemon and pushing the
    // results back through the `git_panel_set_*` / `notes_set_*`
    // entry points below.

    /// Toggle the rich right-side git diff panel. Returns the new
    /// visibility so JS can kick the daemon fetch.
    pub fn toggle_git_diff_panel(&mut self) -> bool {
        self.chrome.toggle_git_diff_panel()
    }

    /// Toggle the notes sidebar. Returns the new visibility.
    pub fn toggle_notes_sidebar(&mut self) -> bool {
        self.chrome.toggle_notes_sidebar()
    }

    /// One-shot: true when the git side panel wants fresh data.
    pub fn take_git_panel_refresh(&mut self) -> bool {
        self.chrome.take_git_panel_refresh()
    }

    /// One-shot: true when the notes sidebar wants a listing.
    pub fn take_notes_refresh(&mut self) -> bool {
        self.chrome.take_notes_refresh()
    }

    /// Push the changed-file list into the git side panel.
    /// `files_json` is `[{path, status, additions, deletions}]`
    /// with `status` one of the porcelain-ish tags the daemon's
    /// `GitFileStatus` serializes to.
    pub fn git_panel_set_files(&mut self, files_json: String) {
        use neoism_ui::panels::git_diff::{FileChange, FileStatus};
        #[derive(serde::Deserialize)]
        struct WireFile {
            path: String,
            status: String,
            additions: u32,
            deletions: u32,
        }
        let Ok(files) = serde_json::from_str::<Vec<WireFile>>(&files_json) else {
            return;
        };
        let files = files
            .into_iter()
            .map(|f| FileChange {
                path: f.path,
                status: match f.status.as_str() {
                    "Added" => FileStatus::Added,
                    "Deleted" => FileStatus::Deleted,
                    "Renamed" => FileStatus::Renamed,
                    "Untracked" => FileStatus::Untracked,
                    "Conflicted" => FileStatus::Conflict,
                    _ => FileStatus::Modified,
                },
                additions: f.additions,
                deletions: f.deletions,
                // Web staging flows through the daemon (Pass 2); the
                // wire payload carries no index/worktree split yet.
                staged: false,
            })
            .collect();
        self.chrome.git_diff_panel.host_set_files(files);
    }

    /// Push one file's raw `git diff` patch text (hunk headers
    /// included) into the git side panel.
    pub fn git_panel_set_diff(&mut self, path: String, patch: String) {
        self.chrome.git_diff_panel.host_set_diff_text(&path, &patch);
    }

    /// Surface a daemon-side git failure in the panel body.
    pub fn git_panel_set_error(&mut self, message: String) {
        self.chrome.git_diff_panel.host_set_error(message);
    }

    /// Push the notes tree listing. `entries_json` is
    /// `[{path, is_dir}]` with daemon-absolute paths.
    pub fn notes_set_entries(&mut self, entries_json: String) {
        #[derive(serde::Deserialize)]
        struct WireEntry {
            path: String,
            is_dir: bool,
        }
        let Ok(entries) = serde_json::from_str::<Vec<WireEntry>>(&entries_json) else {
            return;
        };
        let entries = entries
            .into_iter()
            .map(|e| (PathBuf::from(e.path), e.is_dir))
            .collect();
        self.chrome.notes_sidebar.set_entries_from_host(entries);
    }

    /// Drain note / git-panel rows the user activated; JS opens
    /// each path through the same pipeline as file-tree opens.
    pub fn drain_panel_open_paths(&mut self) -> JsValue {
        let paths = self.chrome.drain_panel_open_paths();
        serde_wasm_bindgen::to_value(&paths).unwrap_or(JsValue::NULL)
    }
}
