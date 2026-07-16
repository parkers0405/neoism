//! File tree side panel — Warp-style chrome rendered through sugarloaf.
//!
//! Lifted verbatim from `frontends/neoism/src/editor/file_tree/` so the
//! native (winit) and web (wasm) frontends share the same depth-first
//! flat list, alphabetical-with-dirs-first sort, git-status overlays,
//! reveal flash, scroll/cursor springs, label truncation cache, and
//! Nerd-Font icon palette. Heavy state stays here; OS I/O is routed
//! through `services::FilesService` / `services::GitService` so the
//! web host can marshal calls over the daemon WebSocket.
//!
//! ## Two layers, one panel
//!
//! - **State + render** (this module + `state.rs` / `update.rs` /
//!   `render.rs` / `scan.rs` / `git.rs` / `icons.rs` / `types.rs`) is
//!   the cross-platform port. Same animations, same tuning constants
//!   as the original native module.
//! - **`Panel` trait shim** at the bottom adapts the wider
//!   `FileTree::render(&mut self, sugarloaf, x_left, y_top, width, height,
//!   &IdeTheme, &[occlusion_rects])` entry into the slim chrome.rs
//!   `Panel::draw(&self, sugarloaf, &PanelLayout, &PanelContext)`
//!   surface. The active process-wide `IdeTheme` is resolved here so this
//!   shim stays visually in sync with the rest of chrome.
//!
//! ## Icons + theming
//!
//! Icons use Nerd Font codepoints; glyphs render only when the active
//! font (or a fallback) carries the Nerd Font private-use range —
//! Cascadia Code NF (bundled) and "GeistMono Nerd Font" both work;
//! bare "Geist Mono" does not. Per-row colors come from `IdeTheme`
//! so the theme picker repaints the tree live.

mod git;
pub mod icons;
mod policy;
mod render;
mod scan;
mod state;
mod types;
mod update;
mod virtuals;

#[cfg(test)]
mod tests;

/// Default width when the user hasn't resized. Real width lives on
/// the `FileTree` instance so `Alt+Left` / `Alt+Right` can grow or
/// shrink the panel without rebuilding the renderer.
pub const FILE_TREE_WIDTH: f32 = 280.0;
pub const FILE_TREE_MIN_WIDTH: f32 = 140.0;
pub const FILE_TREE_MAX_WIDTH: f32 = 700.0;
pub const FILE_TREE_RESIZE_STEP: f32 = 24.0;
pub const ROW_HEIGHT: f32 = 26.0;

pub const FONT_SIZE: f32 = 13.0;
pub const ICON_FONT_SIZE: f32 = 13.0;
pub const ROW_PADDING_X: f32 = 12.0;
pub const INDENT_PX: f32 = 14.0;
pub const ICON_GAP: f32 = 8.0;
pub const FRAME_RADIUS: f32 = 14.0;
pub const FRAME_STROKE: f32 = 2.25;
// Folder/file icon fallback colors. File-type icon colors intentionally
// stay semantic, but folder color is overridden by `IdeTheme::folder`
// in the render path.
pub const FOLDER_ICON_COLOR: [u8; 4] = [126, 186, 228, 255];
pub(crate) const FILE_ICON_DEFAULT: [u8; 4] = [200, 200, 200, 255];
pub(crate) const DEPTH: f32 = 0.0;
pub(crate) const ORDER: u8 = 6;
pub(crate) const SCROLL_ANIMATION_LENGTH: f32 = 0.30;
pub(crate) const CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub(crate) const REVEAL_FLASH_MS: f32 = 900.0;
/// Row-reveal sweep when the tree re-roots (workspace switch, server
/// join, explicit re-root). Mirrors the status line's mode-swap
/// transition — same duration, same ease-out-cubic — with each row
/// starting a beat after the one above it.
pub(crate) const ROOT_TRANSITION_MS: f32 = 320.0;
pub(crate) const ROOT_TRANSITION_STAGGER_MS: f32 = 12.0;
pub(crate) const SCROLL_OFF_ROWS: usize = 4;
pub(crate) const LABEL_TRUNCATION_CACHE_MAX: usize = 2048;

pub use git::{git_statuses_for, git_watch_paths_for, parse_git_status};
pub use icons::{icon_for_file, workspace_root_icon};
pub use policy::{
    activation_for_selection, close_policy, directory_link_policy,
    file_tree_context_menu_items, file_tree_context_menu_should_open,
    file_tree_git_refresh_action, open_command_policy, rename_target_for_input,
    selected_path_for_entry, target_dir_for_selection, toggle_visibility_policy,
    DirectoryLinkDecision, FileTreeBridgeState, FileTreeContextItem,
    FileTreeContextMenuInputs, FileTreeGitRefreshAction, FileTreeGitRefreshState,
    FileTreeVisibilityDecision, RenameTarget, SelectionActivation,
};
pub(crate) use render::truncate_label;
pub use scan::{
    apply_git_statuses, entries_from_dir_listing, normalize_path, same_entry_layout,
    scan_dir, scan_dir_result, scan_dir_with_open,
};
pub use state::FileTree;
pub use types::{
    FileTreeGitRefreshRequest, FileTreeGitRefreshResult, GitStatus, GitWatchPaths,
    NodeKind, TreeEntry, VirtualEntryKind,
};
pub use virtuals::{
    is_workspace_note_path, scan_root_with_workspace, virtual_workspace_path,
    workspace_virtual_children, NEOISM_FOLDER_ICON_COLOR,
};

/// Compatibility alias for the lookalike panel's `TreeNode` type. The
/// slim shape (`path` / `name` / `depth` / `is_dir`) maps onto the
/// richer `TreeEntry` — kept so existing `panels::mod` re-exports
/// (`pub use file_tree::{FileTree, TreeNode}`) keep compiling. New
/// callers should prefer [`TreeEntry`] which carries the kind enum and
/// the per-row git status.
pub type TreeNode = TreeEntry;

// ---- Panel-trait shim --------------------------------------------------------

use sugarloaf::Sugarloaf;

use crate::event::UiEvent;
use crate::layout::PanelLayout;
use crate::panels::PanelContext;

impl crate::panels::Panel for FileTree {
    fn handle_event(&mut self, event: &UiEvent, ctx: &mut PanelContext) {
        let _ = self.handle_ui_event(event, ctx, None);
    }

    fn draw(&self, sugarloaf: &mut Sugarloaf, layout: &PanelLayout, _ctx: &PanelContext) {
        // Slim Panel::draw is `&self`; the native `FileTree::render`
        // is `&mut self` because it ticks the scroll/cursor springs
        // and writes back `last_panel_height_rows` / the truncation
        // cache. Until the Panel trait can carry `&mut self` (TODO
        // wave6-cutover), step around it with a UnsafeCell-free
        // pointer cast — safe here because the chrome only calls
        // `draw` once per frame and panels are not shared across
        // threads.
        let bounds = layout.bounds;
        let theme = crate::chrome::active_ide_theme();
        let occlusion: [[f32; 4]; 0] = [];
        // SAFETY: chrome dispatch is single-threaded and never invokes
        // `draw` concurrently with itself or with the `handle_event`
        // mutators above. The pointer cast is only here because
        // `Panel::draw` is `&self`; the underlying render path needs
        // mutable access to advance the per-frame spring state.
        unsafe {
            let this = self as *const FileTree as *mut FileTree;
            (*this).render(
                sugarloaf, bounds.x, bounds.y, bounds.w, bounds.h, &theme, &occlusion,
            );
        }
    }

    fn wants_focus(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "file_tree"
    }
}
