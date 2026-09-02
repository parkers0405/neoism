//! File tree side panel — Warp-style chrome rendered through sugarloaf,
//! owned by `Renderer` like `island` / `command_palette`.
//!
//! ## Two implementations, side by side
//!
//! - **Native `FileTree`** (this module + `state.rs` / `update.rs` /
//!   `render.rs` / `scan.rs` / `git.rs` / `icons.rs` / `types.rs`) is
//!   the live chrome the native renderer drives today. Heavy state
//!   (animations, git overlays, reveal flash, scroll springs) lives
//!   here and stays native for now.
//! - **Slim `neoism_ui::panels::FileTree`** is the cross-platform
//!   view we lift onto. It carries only the depth-first node list,
//!   selection, scroll, expanded set, and a pending-request map for
//!   web async listings. Re-exported below as
//!   [`UiFileTree`] so callers can adopt it incrementally.
//!
//! ## The `FilesService` shim
//!
//! [`NativeFiles`] adapts native `std::fs` directory reads into the
//! `neoism_ui::services::FilesService` trait. The slim panel consumes
//! that trait synchronously on native and gets
//! `IoError::Pending(req_id)` on web; either path threads through
//! the same shared `FileTree::apply_listing` hook. This is the seam
//! the cross-platform port runs through.

// Chrome colors come from `IdeTheme` so the theme picker repaints the
// tree live alongside tabs, status, and nvim.
//
// Icons + per-extension colors use Nerd Font codepoints. The
// glyphs only render when the active font (or a fallback) carries the
// Nerd Font private-use range — Cascadia Code NF (bundled) and
// "GeistMono Nerd Font" both work; bare "Geist Mono" does not.

mod git;
mod icons;
mod render;
mod scan;
pub(crate) mod state;
mod types;
mod update;
mod virtuals;

#[cfg(test)]
mod tests;

// -- neoism-ui shim ---------------------------------------------------------

/// Cross-platform slim file tree view from `neoism_ui`. Re-exported
/// under a distinct name so callers that already lean on the native
/// `FileTree` keep compiling unchanged while the migration ramps.
#[allow(unused_imports)]
pub use neoism_ui::panels::FileTree as UiFileTree;
#[allow(unused_imports)]
pub use neoism_ui::panels::TreeNode as UiTreeNode;

/// Host-side `FilesService` impl backed by native `std::fs` reads.
/// The slim panel routes every `list_dir` call through this shim on
/// native; web hosts swap in a daemon-relayed
/// implementation that returns `IoError::Pending` and replies via
/// `UiEvent::ServiceReply`.
#[allow(dead_code)]
#[derive(Default)]
pub struct NativeFiles;

impl neoism_ui::services::FilesService for NativeFiles {
    fn list_dir(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<neoism_ui::services::DirEntry>, neoism_ui::services::IoError> {
        // Return a raw, single-level listing. The shared panel applies the
        // same hidden-entry policy for local, daemon, SSH, and web sources.
        // Keeping this non-recursive prevents `.git` or cache traversal.
        let read = std::fs::read_dir(path).map_err(map_io_err)?;
        Ok(read
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_str()?.to_string();
                let is_dir = entry.file_type().ok()?.is_dir();
                Some(neoism_ui::services::DirEntry {
                    name,
                    is_dir,
                    size: None,
                })
            })
            .collect())
    }

    fn read_file(
        &self,
        path: &std::path::Path,
    ) -> Result<Vec<u8>, neoism_ui::services::IoError> {
        std::fs::read(path).map_err(map_io_err)
    }

    fn write_file(
        &self,
        path: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), neoism_ui::services::IoError> {
        std::fs::write(path, bytes).map_err(map_io_err)
    }

    fn stat(
        &self,
        path: &std::path::Path,
    ) -> Result<neoism_ui::services::DirEntry, neoism_ui::services::IoError> {
        let meta = std::fs::metadata(path).map_err(map_io_err)?;
        let name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        Ok(neoism_ui::services::DirEntry {
            name,
            is_dir: meta.is_dir(),
            size: Some(meta.len()),
        })
    }
}

/// Host-side `NotificationService` impl backed by the `neoism-notifier`
/// crate (macOS `UNUserNotificationCenter` / Linux D-Bus / Windows
/// toasts). Web hosts swap in a JS-bridged implementation that calls
/// `navigator.permissions` + the browser's `Notification` API and
/// falls back to the in-app toast stack when permission is denied.
///
/// Lives alongside `NativeFiles` so the native-only services share a
/// home; the trait surface lives in `neoism_ui::services` so panels
/// don't need to know which backend they're talking to.
#[allow(dead_code)]
#[derive(Default)]
pub struct NativeNotifications;

impl neoism_ui::services::NotificationService for NativeNotifications {
    fn notify(
        &self,
        title: &str,
        body: &str,
        _level: neoism_ui::services::NotificationLevel,
    ) {
        // `neoism_notifier::send_notification` already spawns a
        // background thread per call, so we can fire and forget. The
        // `level` hint is ignored today — neither D-Bus's `Notify`
        // nor macOS's default `UNNotificationContent` carry the
        // matching urgency parameter — but the trait surface keeps
        // the field so a future revision can map it onto
        // `interruptionLevel` / `ToastImportance` / D-Bus `urgency`
        // hints without touching panel call sites.
        neoism_notifier::send_notification(title, body);
    }
}

struct NativeClipboard;

impl neoism_ui::services::ClipboardService for NativeClipboard {
    fn read(&self) -> Option<String> {
        None
    }

    fn write(&self, _text: &str) {}
}

struct NativeCommands;

impl neoism_ui::services::CommandService for NativeCommands {
    fn run(&self, _command: &str) -> Result<(), neoism_ui::services::CommandError> {
        Ok(())
    }
}

struct NativeClock;

impl neoism_ui::services::ClockService for NativeClock {
    fn now_monotonic(&self) -> std::time::Duration {
        std::time::Instant::now().elapsed()
    }
}

pub(super) fn with_native_panel_context<R>(
    f: impl FnOnce(&neoism_ui::panels::PanelContext<'_>) -> R,
) -> R {
    with_panel_context_files(None, f)
}

/// Same bundle as [`with_native_panel_context`] but with an optional
/// files-service override — the JOINED-workspace tree swaps in the
/// daemon-backed [`remote::RemoteFiles`] so listings come from the
/// host machine.
pub(super) fn with_panel_context_files<R>(
    files_override: Option<&dyn neoism_ui::services::FilesService>,
    f: impl FnOnce(&neoism_ui::panels::PanelContext<'_>) -> R,
) -> R {
    let files = NativeFiles;
    let clipboard = NativeClipboard;
    let commands = NativeCommands;
    let git = git::NativeGit;
    let clock = NativeClock;
    let search = neoism_ui::services::NullSearchService;
    let notifications = NativeNotifications;
    let theme = neoism_ui::theme::ChromeTheme::default();
    let services = neoism_ui::services::Services {
        files: files_override.unwrap_or(&files),
        clipboard: &clipboard,
        commands: &commands,
        git: &git,
        clock: &clock,
        search: &search,
        notifications: &notifications,
    };
    let ctx = neoism_ui::panels::PanelContext {
        services,
        theme: &theme,
        time: neoism_ui::services::ClockService::now_monotonic(&clock),
    };
    f(&ctx)
}

#[allow(dead_code)]
fn map_io_err(err: std::io::Error) -> neoism_ui::services::IoError {
    match err.kind() {
        std::io::ErrorKind::NotFound => {
            neoism_ui::services::IoError::NotFound(err.to_string())
        }
        std::io::ErrorKind::PermissionDenied => {
            neoism_ui::services::IoError::PermissionDenied(err.to_string())
        }
        _ => neoism_ui::services::IoError::Other(err.to_string()),
    }
}

pub use neoism_ui::panels::file_tree::FILE_TREE_RESIZE_STEP;
#[cfg(test)]
pub use neoism_ui::panels::file_tree::{FILE_TREE_WIDTH, ROW_HEIGHT};

#[cfg(test)]
pub(crate) const FRAME_STROKE: f32 = 2.25;
#[cfg(test)]
pub(crate) const FOLDER_ICON_COLOR: [u8; 4] = [126, 186, 228, 255];

#[allow(unused_imports)]
pub use icons::icon_for_file;
pub use state::FileTree;
pub use types::{FileTreeGitRefreshResult, VirtualEntryKind};

pub use git::git_watch_paths_for;
