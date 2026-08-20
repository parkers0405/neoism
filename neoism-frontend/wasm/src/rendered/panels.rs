use super::*;
use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use neoism_protocol::git::GitClientMessage;
use neoism_ui::panels::git_diff::{FileChange, FileStatus, GitDiffIo};

/// JS-backed `GitDiffIo` provider — the web twin of the desktop's
/// `NativeGitDiffIo`. Where the desktop shells out to `git`, this
/// serializes the matching `GitClientMessage` envelope and hands it to
/// the JS host (installed via [`ChromeBridge::set_git_panel_ops`]),
/// which forwards it to the daemon over the WebSocket. Every method is
/// fire-and-forget: results land back through the
/// `git_panel_apply_changed_files` / `git_panel_set_branches` /
/// `git_panel_set_error` push entry points below, mirroring how the
/// other JS-backed services resume via `service_reply` after
/// `IoError::Pending`.
struct DaemonGitDiffIo {
    cb: js_sys::Function,
    /// Request ids handed to JS for symmetry with the other service
    /// callbacks. Allocated from a high base so they can't collide
    /// with `SharedState::alloc_request_id` ids if a future host
    /// routes them through the shared correlation table.
    next_id: Cell<u64>,
}

// SAFETY: wasm32-unknown-unknown is single-threaded; `GitDiffIo`'s
// `Send + Sync` bounds exist for the desktop's background threads,
// which never run here. Same justification as `SharedState`'s impls
// in `mod.rs`.
unsafe impl Send for DaemonGitDiffIo {}
unsafe impl Sync for DaemonGitDiffIo {}

impl DaemonGitDiffIo {
    fn fire(&self, msg: &GitClientMessage) {
        let Ok(json) = serde_json::to_string(msg) else {
            return;
        };
        let id = self.next_id.get();
        self.next_id.set(id.wrapping_add(1));
        let _ = self.cb.call2(
            &JsValue::NULL,
            &JsValue::from_f64(id as f64),
            &JsValue::from_str(&json),
        );
    }
}

impl GitDiffIo for DaemonGitDiffIo {
    fn collect_files(&self, _repo_root: &Path) -> Vec<FileChange> {
        // Fire-and-forget refresh; the daemon reply lands through
        // `git_panel_apply_changed_files`. The empty return is never
        // stored (the wasm panel paths ignore it).
        self.fire(&GitClientMessage::ChangedFiles);
        Vec::new()
    }

    fn stage(&self, _repo_root: &Path, path: &str) -> Result<(), String> {
        self.fire(&GitClientMessage::Stage {
            path: path.to_string(),
        });
        Ok(())
    }

    fn unstage(&self, _repo_root: &Path, path: &str) -> Result<(), String> {
        self.fire(&GitClientMessage::Unstage {
            path: path.to_string(),
        });
        Ok(())
    }

    fn commit(&self, _repo_root: &Path, message: &str) -> Result<(), String> {
        self.fire(&GitClientMessage::Commit {
            message: message.to_string(),
        });
        Ok(())
    }

    fn list_branches(&self, _repo_root: &Path) -> Vec<String> {
        self.fire(&GitClientMessage::Branches);
        Vec::new()
    }

    fn checkout(&self, _repo_root: &Path, branch: &str) -> Result<(), String> {
        self.fire(&GitClientMessage::Checkout {
            branch: branch.to_string(),
        });
        Ok(())
    }
}

/// Map a wire status tag (either the legacy `GitFileStatus` spelling
/// or the richer `GitChangeStatus` one) onto the shared panel's
/// `FileStatus`.
fn map_wire_status(tag: &str) -> FileStatus {
    match tag {
        "Added" => FileStatus::Added,
        "Deleted" => FileStatus::Deleted,
        "Renamed" => FileStatus::Renamed,
        "Untracked" => FileStatus::Untracked,
        "Conflicted" | "Conflict" => FileStatus::Conflict,
        "Staged" => FileStatus::Staged,
        "Mixed" => FileStatus::Mixed,
        _ => FileStatus::Modified,
    }
}

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

    /// Show the "Share with phone" QR sheet for `url`. The JS host asks
    /// the daemon for a phone-reachable address first (`RequestShareTarget`)
    /// — the browser only knows the origin it was served from, which for a
    /// local session is loopback and useless to a phone.
    pub fn share_sheet_show(&mut self, url: String, hint: Option<String>) {
        if url.is_empty() {
            self.chrome.share_sheet.show_message(
                hint.unwrap_or_else(|| "No shareable address.".to_string()),
            );
        } else {
            self.chrome.share_sheet.show(url, hint);
        }
    }

    /// Dismiss the sheet (click / Escape). True when it was open and
    /// therefore consumed the input.
    pub fn share_sheet_dismiss(&mut self) -> bool {
        self.chrome.dismiss_share_sheet_if_open()
    }

    pub fn share_sheet_visible(&self) -> bool {
        self.chrome.share_sheet.is_visible()
    }

    /// Point the notes sidebar at the host's linked vault. JS passes the
    /// daemon-absolute `WorkspaceSummary::linked_vault_dir`, or `None`
    /// when the workspace links no vault (drives the "no linked vault"
    /// empty state). Notes never live under `<workspace_root>/notes`, so
    /// this cannot be derived from `set_workspace_root`.
    pub fn set_notes_vault_root(&mut self, vault: Option<String>) {
        let vault = vault
            .filter(|v| !v.is_empty())
            .map(PathBuf::from);
        self.chrome.set_notes_vault_root(vault);
    }

    /// One-shot: true when the git side panel wants fresh data.
    pub fn take_git_panel_refresh(&mut self) -> bool {
        self.chrome.take_git_panel_refresh()
    }

    /// One-shot: true when the notes sidebar wants a listing.
    pub fn take_notes_refresh(&mut self) -> bool {
        self.chrome.take_notes_refresh()
    }

    /// Install the JS callback that ships serialized
    /// `GitClientMessage` envelopes to the daemon: `(reqId,
    /// envelopeJson) => void`. Installing it also plugs a
    /// [`DaemonGitDiffIo`] provider into the shared git panel, which
    /// flips the panel's stage / commit / branch flows from native
    /// no-ops to daemon round trips — the web twin of the desktop's
    /// `install_io(&mut panel)`.
    pub fn set_git_panel_ops(&mut self, cb: js_sys::Function) {
        self.chrome
            .git_diff_panel
            .set_io_provider(Arc::new(DaemonGitDiffIo {
                cb,
                next_id: Cell::new(0x5000_0000),
            }));
    }

    /// Push the changed-file list into the git side panel.
    /// `files_json` is `[{path, status, additions, deletions}]`
    /// with `status` one of the porcelain-ish tags the daemon's
    /// `GitFileStatus` serializes to.
    ///
    /// Legacy staged-less path: once a `GitDiffIo` provider is
    /// installed (`set_git_panel_ops`), the provider-driven
    /// `ChangedFiles` flow owns the file list — with real staged
    /// bits — so this push is ignored to keep the two sources from
    /// clobbering each other.
    pub fn git_panel_set_files(&mut self, files_json: String) {
        if self.chrome.git_diff_panel.has_io_provider() {
            return;
        }
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
                status: map_wire_status(&f.status),
                additions: f.additions,
                deletions: f.deletions,
                // This legacy wire payload carries no index/worktree
                // split; the provider path above carries the real bit.
                staged: false,
            })
            .collect();
        self.chrome.git_diff_panel.host_set_files(files);
    }

    /// Provider-path push: apply a daemon `ChangedFiles` reply —
    /// `{files: [{path, status, additions, deletions, staged}],
    /// branch, error}` — refreshing the file list (real staged
    /// state), the branch label and any mutation error, mirroring
    /// the desktop mutation thread's store-back.
    pub fn git_panel_apply_changed_files(&mut self, reply_json: String) {
        #[derive(serde::Deserialize)]
        struct WireChangedFile {
            path: String,
            status: String,
            additions: u32,
            deletions: u32,
            staged: bool,
        }
        #[derive(serde::Deserialize)]
        struct WireChangedFiles {
            files: Vec<WireChangedFile>,
            #[serde(default)]
            branch: Option<String>,
            #[serde(default)]
            error: Option<String>,
        }
        let Ok(reply) = serde_json::from_str::<WireChangedFiles>(&reply_json) else {
            return;
        };
        let files = reply
            .files
            .into_iter()
            .map(|f| FileChange {
                path: f.path,
                status: map_wire_status(&f.status),
                additions: f.additions,
                deletions: f.deletions,
                staged: f.staged,
            })
            .collect();
        let panel = &mut self.chrome.git_diff_panel;
        panel.host_set_files(files);
        if reply.branch.is_some() {
            panel.host_set_branch(reply.branch);
        }
        if let Some(error) = reply.error {
            panel.host_set_error(error);
        }
    }

    /// Provider-path push: the local branch list for the panel's
    /// branch dropdown (daemon `Branches` reply).
    pub fn git_panel_set_branches(&mut self, branches_json: String) {
        let Ok(branches) = serde_json::from_str::<Vec<String>>(&branches_json) else {
            return;
        };
        self.chrome.git_diff_panel.host_set_branches(branches);
    }

    /// Legacy push: one file's raw index→workdir patch text from the
    /// host's whole-repo diff fetch. Ignored once a provider is
    /// installed — the provider path feeds `git diff HEAD` patches
    /// through [`Self::git_panel_apply_file_diffs`] instead (desktop
    /// `load_diff` parity: staged changes stay visible), and letting
    /// both write would leave whichever landed last.
    pub fn git_panel_set_diff(&mut self, path: String, patch: String) {
        if self.chrome.git_diff_panel.has_io_provider() {
            return;
        }
        self.chrome.git_diff_panel.host_set_diff_text(&path, &patch);
    }

    /// Provider-path push: apply a daemon `FileDiffs` reply —
    /// `[{path, patch}]` with desktop-parity `git diff HEAD` patch
    /// text per changed file.
    pub fn git_panel_apply_file_diffs(&mut self, diffs_json: String) {
        #[derive(serde::Deserialize)]
        struct WireFileDiff {
            path: String,
            patch: String,
        }
        let Ok(diffs) = serde_json::from_str::<Vec<WireFileDiff>>(&diffs_json) else {
            return;
        };
        for d in diffs {
            self.chrome
                .git_diff_panel
                .host_set_diff_text(&d.path, &d.patch);
        }
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
            /// Daemon-resolved markdown page icon; the browser cannot read
            /// frontmatter itself.
            #[serde(default)]
            icon: Option<String>,
        }
        let Ok(entries) = serde_json::from_str::<Vec<WireEntry>>(&entries_json) else {
            return;
        };
        let entries = entries
            .into_iter()
            .map(|e| (PathBuf::from(e.path), e.is_dir, e.icon))
            .collect();
        self.chrome
            .notes_sidebar
            .set_entries_from_host_with_icons(entries);
    }

    /// Drain note / git-panel rows the user activated; JS opens
    /// each path through the same pipeline as file-tree opens.
    pub fn drain_panel_open_paths(&mut self) -> JsValue {
        let paths = self.chrome.drain_panel_open_paths();
        serde_wasm_bindgen::to_value(&paths).unwrap_or(JsValue::NULL)
    }
}
