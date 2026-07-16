// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Command palette panel — split into thin topic modules.
//!
//! - [`actions`]  — `PaletteAction` enum + surface-visibility filter.
//! - [`commands`] — static `COMMANDS` catalog + ex-command suggestions.
//! - [`modes`]   — `PaletteMode` and `PaletteRow` enums.
//! - [`state`]   — `CommandPalette` struct, ctor, state mutators.
//! - [`update`]  — input/keystroke handling, selection, hit-testing.
//! - [`fuzzy`]   — fuzzy scoring + text/measurement helpers.
//! - [`render`]  — drawing pipeline (palette + rows + scrollbar).
//!
//! # Native shim relationship
//!
//! A platform-neutral slim palette now lives in
//! `neoism-ui::panels::command_palette::CommandPalette`. The shared
//! version owns the modal UI (text input, filtered list, dispatch via
//! `CommandService`); the catalog (ex commands, fonts, search history,
//! buffer picker, surface filters) stays here because it touches
//! native state — process spawn, font enumeration, workspace
//! introspection — that the shared crate explicitly excludes.
//!
//! Migration is staged: the native `CommandPalette` keeps every
//! existing call site working until the host-side `Chrome` runtime
//! lands. At that point the two halves connect via
//! `palette.set_commands(...)` and the native palette's view layer
//! becomes a thin adapter over the shared one.
//!
//! See `docs/CHROME_LIFT_AUDIT.md` (this panel is the third migrated)
//! and `docs/NEOISM_UI_DESIGN.md` §9 for the migration recipe.

pub mod actions;
pub mod commands;
pub mod fuzzy;
pub mod modes;
pub mod render;
pub mod state;
pub mod update;

#[cfg(test)]
mod tests;

pub use actions::{
    mashup_packs_modal_spec, shaders_modal_spec, theme_picker_modal_spec, HostKind,
    PaletteAction, PaletteBufferEntry, PaletteBufferTarget, PaletteHostEntry,
    PaletteMashupEntry, PaletteServerEntry, PaletteShaderEntry, PaletteSurface,
    PaletteWorkspaceEntry, PaletteWorkspaceTarget, WorkspaceHostKind,
    WorkspaceVisibility, WORKSPACE_ROOT_DETAIL_PREFIX,
};
pub use state::{CommandPalette, WorkspaceMovePhase, WorkspaceMoveStatus};

// Layout — shared across state/update/render so they live at the
// crate-module root rather than being duplicated per file.
pub(crate) const PALETTE_WIDTH: f32 = 480.0;
pub(crate) const PALETTE_CORNER_RADIUS: f32 = 8.0;
pub(crate) const PALETTE_MARGIN_TOP: f32 = 80.0;
pub(crate) const PALETTE_PADDING: f32 = 0.0;

pub(crate) const INPUT_HEIGHT: f32 = 30.0;
pub(crate) const INPUT_FONT_SIZE: f32 = 14.0;
pub(crate) const INPUT_PADDING_X: f32 = 10.0;

pub(crate) const RESULT_ITEM_HEIGHT: f32 = 32.0;
pub(crate) const RESULT_FONT_SIZE: f32 = 13.0;
pub(crate) const SHORTCUT_FONT_SIZE: f32 = 11.0;
pub(crate) const MAX_VISIBLE_RESULTS: usize = 8;
pub(crate) const RESULTS_PADDING_BOTTOM: f32 = 4.0;

// Copy icon (two overlapping page outlines with rounded corners,
// drawn by layering filled + cutout rounded rects). Sized to fit
// comfortably inside RESULT_ITEM_HEIGHT.
pub(crate) const COPY_ICON_PAGE_W: f32 = 10.0;
pub(crate) const COPY_ICON_PAGE_H: f32 = 12.0;
pub(crate) const COPY_ICON_OFFSET: f32 = 3.0;
pub(crate) const COPY_ICON_STROKE: f32 = 1.0;
pub(crate) const COPY_ICON_RADIUS: f32 = 2.0;
pub(crate) const COPY_ICON_W: f32 = COPY_ICON_PAGE_W + COPY_ICON_OFFSET; // 13
pub(crate) const COPY_ICON_H: f32 = COPY_ICON_PAGE_H + COPY_ICON_OFFSET; // 15

pub(crate) const SEPARATOR_HEIGHT: f32 = 1.0;
pub(crate) const RESULTS_MARGIN_TOP: f32 = 0.0;

/// Logical-pixel distance the cursor must travel from the press point
/// before a workspace-row press in the Workspaces modal becomes a drag
/// (5D-drag). Below this, the press resolves as a plain click (select +
/// switch). Mirrors buffer-tabs' `DRAG_ACTIVATION_THRESHOLD_PX`.
pub(crate) const WORKSPACE_DRAG_ACTIVATION_PX: f32 = 4.0;

pub(crate) const CARET_WIDTH: f32 = 1.5;
pub(crate) const CARET_BLINK_MS: u128 = 500;
pub(crate) const LIST_SCROLL_ANIMATION_LENGTH: f32 = 0.30;
pub(crate) const CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub(crate) const OPEN_POP_MS: f32 = 180.0;
pub(crate) const SCROLL_OFF_ROWS: usize = 2;

// Depth / order
#[allow(dead_code)]
pub(crate) const DEPTH_BACKDROP: f32 = 0.0;
pub(crate) const DEPTH_BG: f32 = 0.1;
pub(crate) const DEPTH_ELEMENT: f32 = 0.2;
pub(crate) const ORDER: u8 = 20;

pub(crate) const MAX_RECENT_SEARCHES: usize = 10;
