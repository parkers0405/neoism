//! Pure chrome layout policies shared by native and web hosts.

/// Inputs needed to reserve workspace chrome above and below a
/// terminal/editor grid.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceChromeMetrics {
    pub margin_top: f32,
    pub margin_bottom: f32,
    pub island_top: f32,
    pub buffer_tabs_height: f32,
    pub breadcrumbs_height: f32,
    pub status_line_height: f32,
    pub terminal_top_padding: f32,
    pub has_buffer_tabs: bool,
    pub chrome_safety_pad: f32,
}

/// Logical margin reservations for the active workspace chrome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorkspaceChromeMargins {
    pub editor_top: f32,
    pub terminal_top: f32,
    pub bottom: f32,
}

/// Physical editor-row geometry after all surrounding chrome has been
/// accounted for. `cell_height` is a pane-local pitch: glyph metrics stay
/// unchanged, but the row origins share any otherwise-unused pixels so the
/// last complete row lands exactly on the pane/status boundary.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorRowFit {
    pub rows: u16,
    pub cell_height: f32,
    pub usable_height: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorRowFitInput {
    pub scaled_margin_top: f32,
    pub layout_top: f32,
    pub layout_height: f32,
    pub window_height: f32,
    pub status_line_height: f32,
    pub nominal_cell_height: f32,
}

/// Fit only complete editor rows into the exact physical surface between the
/// pane's top chrome and either its own bottom edge or the global status bar.
///
/// The row count remains conservative (`floor` at the nominal font pitch),
/// while the sub-row remainder is distributed across those rows. This avoids
/// both failure modes of the old policy: an extra clipped nvim row and a solid
/// remainder band below the last row.
pub fn fit_editor_rows(input: EditorRowFitInput) -> EditorRowFit {
    let pane_top = (input.scaled_margin_top + input.layout_top).round();
    let layout_bottom = (pane_top + input.layout_height.max(0.0)).round();
    let status_top = (input.window_height - input.status_line_height.max(0.0)).round();
    let pane_bottom = layout_bottom.min(status_top).max(pane_top);
    let usable_height = (pane_bottom - pane_top).round().max(1.0);
    let nominal_cell_height = input.nominal_cell_height.round().max(1.0);
    let rows = (usable_height / nominal_cell_height)
        .floor()
        .max(1.0)
        .min(u16::MAX as f32) as u16;
    let cell_height = usable_height / f32::from(rows);

    EditorRowFit {
        rows,
        cell_height,
        usable_height,
    }
}

/// Extra logical pixels that `resize_top_or_bottom_line` must add on
/// top of `padding_top_from_config` to make room for the chrome bands
/// above the active context (buffer tabs, breadcrumbs, terminal pad).
///
/// Mirrors the editor/terminal branching inside
/// [`workspace_chrome_margins`] but is exposed separately because
/// `resize_top_or_bottom_line` works in a different coordinate system —
/// it already has `padding_top_from_config` (which folds in
/// margin_top + OS tab bar) and only needs the additive chrome offset.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResizeChromeExtraInput {
    pub current_is_editor: bool,
    pub has_buffer_tabs: bool,
    pub buffer_tabs_height: f32,
    pub breadcrumbs_height: f32,
    pub terminal_top_padding: f32,
    pub chrome_safety_pad: f32,
}

/// Logical px to add to `padding_top_from_config` for the active context.
pub fn resize_chrome_extra(input: ResizeChromeExtraInput) -> f32 {
    if input.current_is_editor {
        input.buffer_tabs_height + input.breadcrumbs_height + input.chrome_safety_pad
    } else if input.has_buffer_tabs {
        input.buffer_tabs_height + input.terminal_top_padding
    } else {
        input.terminal_top_padding
    }
}

/// Compute stable workspace chrome margins.
///
/// Editors always reserve island + buffer tabs + breadcrumbs, even
/// when the tab strip is currently hidden, so switching between
/// terminal/editor focus does not snap the editor grid upward into the
/// chrome band. Terminal panes reserve the island plus the buffer-tab
/// band only when tabs exist, then add the terminal-specific top pad.
pub fn workspace_chrome_margins(
    metrics: WorkspaceChromeMetrics,
) -> WorkspaceChromeMargins {
    let editor_chrome_bottom =
        metrics.island_top + metrics.buffer_tabs_height + metrics.breadcrumbs_height;
    let terminal_tabs_height = if metrics.has_buffer_tabs {
        metrics.buffer_tabs_height
    } else {
        0.0
    };

    WorkspaceChromeMargins {
        editor_top: metrics.margin_top + editor_chrome_bottom + metrics.chrome_safety_pad,
        terminal_top: metrics.margin_top
            + metrics.island_top
            + terminal_tabs_height
            + metrics.terminal_top_padding,
        bottom: metrics.status_line_height,
    }
}

/// Logical editor panel geometry needed to mask grid buffer bleed around chrome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorChromeMaskInput {
    pub panel_left: f32,
    pub panel_width: f32,
    pub pane_top: f32,
    pub chrome_height: f32,
}

/// A logical rect that should be painted with the active background color.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EditorChromeMaskRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

/// Compute the editor's top safety mask. Bottom masking is deliberately not
/// part of this policy: fitted rows now meet the pane/status boundary exactly,
/// and the grid clip owns protection from hidden bottom buffer rows.
pub fn editor_chrome_mask_rects(
    input: EditorChromeMaskInput,
) -> Vec<EditorChromeMaskRect> {
    let mut rects = Vec::with_capacity(1);

    let top_height = input.pane_top.min(input.chrome_height).max(0.0);
    if top_height > 0.0 {
        rects.push(EditorChromeMaskRect {
            x: input.panel_left,
            y: 0.0,
            width: input.panel_width,
            height: top_height,
        });
    }

    rects
}

/// Inputs needed to anchor a terminal/editor grid panel inside chrome.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPanelChromeGeometryInput {
    pub is_editor: bool,
    pub scaled_margin_left: f32,
    pub scaled_margin_top: f32,
    pub layout_left: f32,
    pub layout_top: f32,
    pub layout_width: f32,
    pub layout_height: f32,
    pub cell_height: f32,
    pub rows: u32,
    pub visible_row_count: usize,
    pub terminal_reserved_bottom_rows: u32,
    pub editor_buffer_above: u32,
    pub terminal_buffer_above: u32,
    pub terminal_bottom_clip_bleed_px: f32,
}

/// Physical-pixel grid anchor and clip geometry for a rendered panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GridPanelChromeGeometry {
    pub panel_left: f32,
    pub panel_top: f32,
    pub panel_clip_top: f32,
    pub clip_rect: [f32; 4],
}

/// Compute the grid panel origin and terminal clip rectangle.
///
/// The origin is rounded to physical pixels so glyph vertices and background
/// shader cell lookup agree on cell boundaries. Editor panes subtract their
/// hidden top buffer row from the anchor; terminal panes subtract their hidden
/// smooth-scroll buffer row and clip to the visible cell rows instead of the
/// full layout height.
pub fn grid_panel_chrome_geometry(
    input: GridPanelChromeGeometryInput,
) -> GridPanelChromeGeometry {
    let panel_left = (input.scaled_margin_left + input.layout_left).round();
    let panel_clip_top = (input.scaled_margin_top + input.layout_top).round();
    let editor_buffer_offset_phys = if input.is_editor {
        input.editor_buffer_above as f32 * input.cell_height
    } else {
        0.0
    };
    let terminal_buffer_offset_phys = if input.is_editor {
        0.0
    } else {
        input.terminal_buffer_above as f32 * input.cell_height
    };
    let panel_top = if input.is_editor {
        // Keep the visible editor origin exactly on the rounded pane top.
        // A fitted row pitch may be fractional; rounding after subtracting
        // the hidden buffer would re-add a fractional offset when the buffer
        // height is added below, moving both the first and last row off their
        // chrome boundaries.
        panel_clip_top - editor_buffer_offset_phys
    } else {
        (input.scaled_margin_top + input.layout_top - terminal_buffer_offset_phys).round()
    };

    let clip_rect = if input.is_editor {
        let visible_grid_top =
            panel_top + input.editor_buffer_above as f32 * input.cell_height;
        let editor_clip_rows = input.visible_row_count.min(input.rows as usize) as f32;
        let visible_grid_bottom = visible_grid_top + editor_clip_rows * input.cell_height;
        let layout_clip_bottom = panel_clip_top + input.layout_height.round().max(0.0);
        let clip_bottom = visible_grid_bottom
            .min(layout_clip_bottom)
            .max(panel_clip_top);
        let clip_h = (clip_bottom - panel_clip_top).round().max(0.0);
        [
            panel_left,
            panel_clip_top,
            input.layout_width.round().max(0.0),
            clip_h,
        ]
    } else {
        let visible_grid_top =
            panel_top + input.terminal_buffer_above as f32 * input.cell_height;
        let terminal_clip_rows = input.visible_row_count.min(input.rows as usize) as f32;
        let visible_grid_bottom =
            visible_grid_top + terminal_clip_rows * input.cell_height;
        let reserved_bottom_px =
            input.terminal_reserved_bottom_rows as f32 * input.cell_height;
        let layout_clip_bottom =
            panel_clip_top + (input.layout_height - reserved_bottom_px).round().max(0.0);
        let clip_bottom = (visible_grid_bottom + input.terminal_bottom_clip_bleed_px)
            .min(layout_clip_bottom)
            .max(panel_clip_top);
        let clip_h = (clip_bottom - panel_clip_top).round().max(0.0);
        [
            panel_left,
            panel_clip_top,
            input.layout_width.round().max(0.0),
            clip_h,
        ]
    };

    GridPanelChromeGeometry {
        panel_left,
        panel_top,
        panel_clip_top,
        clip_rect,
    }
}

/// Owner selected for the animated trail cursor overlay.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailCursorOverlayTarget {
    Finder,
    CommandPalette,
    ContextMenu,
    FileTree,
    NotesSidebar,
    AgentSidePanel,
    Tabs,
    GitDiffPanel,
    SuppressedByInputOverlay,
    AgentInput,
    Markdown,
    TerminalBlockInput,
    TerminalGrid,
}

/// Rendering family selected after a trail cursor owner is chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrailCursorOverlayDrawKind {
    /// Chrome widgets expose a plain rectangle and use the chrome block cursor.
    ChromeRect,
    /// A higher-priority input overlay owns focus, so lower-priority cursors hide.
    Suppressed,
    /// Content widgets provide their own cursor shape/geometry.
    ContentCursor,
    /// The terminal grid computes cursor geometry from terminal state.
    TerminalGrid,
}

pub fn trail_cursor_overlay_draw_kind(
    target: TrailCursorOverlayTarget,
) -> TrailCursorOverlayDrawKind {
    use TrailCursorOverlayDrawKind as DrawKind;
    use TrailCursorOverlayTarget as Target;

    match target {
        Target::Finder
        | Target::CommandPalette
        | Target::ContextMenu
        | Target::FileTree
        | Target::NotesSidebar
        | Target::AgentSidePanel
        | Target::Tabs
        | Target::GitDiffPanel => DrawKind::ChromeRect,
        Target::SuppressedByInputOverlay => DrawKind::Suppressed,
        Target::AgentInput | Target::Markdown | Target::TerminalBlockInput => {
            DrawKind::ContentCursor
        }
        Target::TerminalGrid => DrawKind::TerminalGrid,
    }
}

/// Focus/visibility facts needed to pick the trail cursor overlay owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TrailCursorOverlayState {
    pub finder_enabled: bool,
    pub command_palette_enabled: bool,
    pub context_menu_visible: bool,
    pub file_tree_focused: bool,
    pub notes_sidebar_focused: bool,
    pub agent_side_panel_focused: bool,
    pub tab_cursor_available: bool,
    pub git_diff_panel_focused: bool,
    pub search_active: bool,
    pub modal_owns_editor_focus: bool,
    pub agent_input_cursor_available: bool,
    pub markdown_cursor_available: bool,
    pub terminal_block_input_active: bool,
    pub trail_cursor_enabled: bool,
}

/// Pick the single UI owner allowed to draw the animated trail cursor.
///
/// Popovers and focused side panels claim priority even when their row rect is
/// unavailable for this frame; that mirrors the native renderer's occlusion
/// behavior and prevents lower-priority carets from bleeding through overlays.
pub fn trail_cursor_overlay_target(
    state: TrailCursorOverlayState,
) -> Option<TrailCursorOverlayTarget> {
    use TrailCursorOverlayTarget as Target;

    if state.finder_enabled {
        Some(Target::Finder)
    } else if state.command_palette_enabled {
        Some(Target::CommandPalette)
    } else if state.context_menu_visible {
        Some(Target::ContextMenu)
    } else if state.file_tree_focused {
        Some(Target::FileTree)
    } else if state.notes_sidebar_focused {
        Some(Target::NotesSidebar)
    } else if state.agent_side_panel_focused {
        Some(Target::AgentSidePanel)
    } else if state.tab_cursor_available {
        Some(Target::Tabs)
    } else if state.git_diff_panel_focused {
        Some(Target::GitDiffPanel)
    } else if state.search_active || state.modal_owns_editor_focus {
        Some(Target::SuppressedByInputOverlay)
    } else if state.agent_input_cursor_available {
        Some(Target::AgentInput)
    } else if state.markdown_cursor_available {
        Some(Target::Markdown)
    } else if state.trail_cursor_enabled && state.terminal_block_input_active {
        Some(Target::TerminalBlockInput)
    } else if state.trail_cursor_enabled {
        Some(Target::TerminalGrid)
    } else {
        None
    }
}

/// Inputs for [`island_drag_tab_geometry`].
///
/// The host owns scale factor, window size, and tab count; the policy
/// converts those into the per-tab logical width that the island uses
/// for hit testing during a drag-move gesture.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandDragTabGeometryInput {
    pub window_width_px: f32,
    pub scale_factor: f32,
    pub num_tabs: usize,
    pub left_margin: f32,
    pub margin_right: f32,
}

/// Per-tab logical geometry returned by [`island_drag_tab_geometry`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IslandDragTabGeometry {
    pub available_width: f32,
    pub tab_width: f32,
}

/// Compute logical tab width during an island workspace-strip drag.
///
/// Mirrors the host-side math in `handle_island_drag_move`: shave the
/// fixed right gutter and the OS-specific left margin (e.g. the macOS
/// traffic-light reservation) off the window width, then divide by the
/// number of workspace tabs. When no tabs are present we return zero
/// width instead of dividing by zero — the host is expected to bail
/// out before reaching the per-tab geometry in that case.
pub fn island_drag_tab_geometry(
    input: IslandDragTabGeometryInput,
) -> IslandDragTabGeometry {
    let logical_width = if input.scale_factor > 0.0 {
        input.window_width_px / input.scale_factor
    } else {
        0.0
    };
    let available_width =
        (logical_width - input.margin_right - input.left_margin).max(0.0);
    let tab_width = if input.num_tabs > 0 {
        available_width / input.num_tabs as f32
    } else {
        0.0
    };
    IslandDragTabGeometry {
        available_width,
        tab_width,
    }
}

/// Inputs derived from an `LspStatusNotification` for the missing-LSP
/// modal dedupe + display policy.
#[derive(Debug, Clone, PartialEq)]
pub struct LspMissingNotificationInput {
    pub name: Option<String>,
    pub binary: Option<String>,
    pub filetype: Option<String>,
}

/// Resolved display strings + dedupe key for the missing-LSP modal.
#[derive(Debug, Clone, PartialEq)]
pub struct LspMissingModalDescriptor {
    /// Server identifier to feed back into the installer lookup.
    pub server: String,
    /// Binary name shown in the modal body.
    pub binary: String,
    /// Human-readable filetype label (e.g. `"rust"` or `"this filetype"`).
    pub filetype_label: String,
    /// Dedupe key — `"<server>:<filetype-or-empty>"`. The host pushes this
    /// into its `BTreeSet<String>` and bails on `insert` returning `false`.
    pub dedupe_key: String,
}

/// Pure resolver: collapse `Option<String>` notification fields into the
/// display strings + dedupe key consumed by `maybe_open_lsp_missing_modal`.
///
/// The host owns the actual `BTreeSet` of already-shown prompts and the
/// `ModalSpec` construction; the policy just settles fallback strings
/// and the canonical dedupe key so both desktop and a future web host
/// agree on which (server, filetype) pairs are "the same prompt".
pub fn lsp_missing_modal_descriptor(
    input: LspMissingNotificationInput,
) -> LspMissingModalDescriptor {
    let server = input
        .name
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
        .or_else(|| input.binary.as_ref().filter(|s| !s.is_empty()).cloned())
        .unwrap_or_else(|| "language server".to_string());
    let binary = input
        .binary
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| server.clone());
    let filetype_label = input
        .filetype
        .as_ref()
        .filter(|s| !s.is_empty())
        .cloned()
        .unwrap_or_else(|| "this filetype".to_string());
    let dedupe_key = format!("{}:{}", server, input.filetype.as_deref().unwrap_or(""));
    LspMissingModalDescriptor {
        server,
        binary,
        filetype_label,
        dedupe_key,
    }
}

/// Categorizes a [`neoism_ui::widgets::modal::ModalAction`] dispatch
/// into a coarse outcome class. The actual side effects (talking to
/// nvim, opening installers, mutating file trees) stay on the host —
/// the policy just answers "should the modal stay open, close before
/// the action runs, or close on completion?" so multiple frontends can
/// share the same dispatch contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalActionDispatch {
    /// Pure "Esc" / X-button close.
    Close,
    /// Long-running installer or apply step that leaves the modal open
    /// (e.g. an `InstallLsp` that flips the modal into a busy state).
    KeepOpenForBusyAction,
    /// Action that closes the modal as its first side effect (e.g.
    /// `RunEditorCommand` clearing the prompt before dispatching).
    CloseBeforeAction,
    /// Inline input commit: validate non-empty before closing.
    CloseAfterValidatedInput,
    /// Prompt-style action that opens a follow-up modal instead of
    /// dispatching directly (e.g. delete/new-file/rename prompts).
    OpenFollowupPrompt,
}

/// Tag for [`modal_action_dispatch`] — matches the variants on
/// `neoism_ui::widgets::modal::ModalAction` but without dragging the
/// (host-only) modal crate into shared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModalActionTag {
    Close,
    InstallLsp,
    InstallPythonKernel,
    InstallTreesitter,
    ApplyTheme,
    ApplyShaderOverlay,
    ApplyMashupPack,
    RunEditorCommand,
    RunEditorCommandWithInput,
    OpenLspLocation,
    InstallAgent,
    RunAgent,
    AcpPermission,
    FileTreeEdit,
    FileTreeCopy,
    FileTreePaste,
    FileTreePromptDelete,
    FileTreeDelete,
    FileTreePromptNewFile,
    NotesPromptNewFile,
    FileTreePromptNewFolder,
    FileTreePromptRename,
    FileTreeNewFile,
    NotesNewFile,
    NotesNewDrawing,
    NotesPromptIcon,
    NotesSetIcon,
    FileTreeNewFolder,
    FileTreeRename,
    RenameTab,
    NotesVaultPromptAdd,
    NotesVaultAdd,
    NotesVaultPromptRename,
    NotesVaultRename,
    NotesVaultSwitch,
    NotesVaultOpenVaultsRoot,
    NotesVaultLinkCurrentWorkspace,
    NotesVaultPromptLinkProject,
    NotesVaultLinkProject,
    NotesVaultShareWithRemarkable,
}

/// Pure policy mirror of `Screen::execute_modal_action`'s dispatch table.
///
/// The host still owns the actual `mark_dirty`, modal closing, and the
/// side-effecting installer / editor / file-tree calls — but every arm
/// of the dispatch table reduces to one of [`ModalActionDispatch`]'s
/// outcomes. Pulling that shape out lets a non-desktop host (web,
/// future TUI) reuse the same contract without hardcoding the match.
pub fn modal_action_dispatch(tag: ModalActionTag) -> ModalActionDispatch {
    use ModalActionDispatch as D;
    use ModalActionTag as T;
    match tag {
        T::Close => D::Close,
        T::InstallLsp
        | T::InstallPythonKernel
        | T::InstallTreesitter
        | T::ApplyTheme
        | T::ApplyShaderOverlay
        | T::ApplyMashupPack
        | T::InstallAgent => D::KeepOpenForBusyAction,
        T::RunEditorCommand
        | T::OpenLspLocation
        | T::RunAgent
        | T::AcpPermission
        | T::FileTreeEdit
        | T::FileTreeCopy
        | T::FileTreePaste
        | T::FileTreeDelete
        | T::FileTreeNewFile
        | T::NotesNewFile
        | T::NotesNewDrawing
        | T::NotesSetIcon
        | T::FileTreeNewFolder
        | T::FileTreeRename
        | T::NotesVaultAdd
        | T::NotesVaultRename
        | T::NotesVaultSwitch
        | T::NotesVaultOpenVaultsRoot
        | T::NotesVaultLinkCurrentWorkspace
        | T::NotesVaultLinkProject
        | T::NotesVaultShareWithRemarkable => D::CloseBeforeAction,
        T::RunEditorCommandWithInput | T::RenameTab => D::CloseAfterValidatedInput,
        T::FileTreePromptDelete
        | T::FileTreePromptNewFile
        | T::NotesPromptNewFile
        | T::NotesPromptIcon
        | T::FileTreePromptNewFolder
        | T::FileTreePromptRename
        | T::NotesVaultPromptAdd
        | T::NotesVaultPromptRename
        | T::NotesVaultPromptLinkProject => D::OpenFollowupPrompt,
    }
}

/// Inputs for [`island_drag_move_outcome`] — purely the drag state the
/// host already pulls off the live `Island` widget. The policy collapses
/// these into the small dispatch decision the host needs to apply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IslandDragMoveInput {
    /// `Some((from, to))` when the per-tab geometry recognised a swap.
    pub swap: Option<(usize, usize)>,
    /// `Island::is_dragging()` — true once the cursor crossed the
    /// activation threshold.
    pub is_dragging: bool,
    /// `Island::is_detach_armed()` — true while the cursor sits past
    /// the detach threshold so the host can paint a ghost preview.
    pub is_detach_armed: bool,
}

/// Outcome of [`island_drag_move_outcome`].
///
/// The host owns the actual `move_workspace` + `swap_tab_state` +
/// `reapply_chrome_layout` calls because they touch live state; the
/// policy just answers "what should the host do after a drag-move?".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IslandDragMoveOutcome {
    /// `Some((from, to))` when the host should reorder workspaces and
    /// rebase island per-tab state. Mirrors the input's `swap`, but
    /// surfaced separately so the host can match on the outcome alone.
    pub perform_swap: Option<(usize, usize)>,
    /// Whether the host should call `mark_dirty()` to repaint —
    /// always true while a drag is live (so the floating tab + any
    /// detach-ghost track the cursor).
    pub mark_dirty: bool,
    /// Return value the host should give to its caller — `true` when
    /// a drag is live so the caller can short-circuit other hover
    /// paths (tab focus, cursor icon changes, etc.).
    pub drag_was_live: bool,
}

/// Pure policy mirror of `handle_island_drag_move`'s post-geometry
/// dispatch. The host still owns the actual workspace-reorder and
/// repaint calls — this collapses the three Island flags + swap tuple
/// into the single decision the host needs to act on.
pub fn island_drag_move_outcome(input: IslandDragMoveInput) -> IslandDragMoveOutcome {
    let _ = input.is_detach_armed;
    IslandDragMoveOutcome {
        perform_swap: input.swap,
        mark_dirty: input.is_dragging,
        drag_was_live: input.is_dragging,
    }
}

/// Pure formatter for `ModalAction::RunEditorCommandWithInput`'s
/// embedded-nvim command string.
///
/// `command` is the modal's command label (`"Rename"`,
/// `"WorkspaceSymbols"`, ...) and `value` is the user's already-trimmed
/// input. The host calls `lua_string_literal` on its end of the wire to
/// quote-escape `value`; the policy supplies the lua wrapper for known
/// labels and the bare `"<cmd> <value>"` fallback for the rest. Returns
/// either a [`ModalEditorCommand::LuaCall`] (which the host renders with
/// its lua-string-literal helper) or a [`ModalEditorCommand::Raw`]
/// passthrough.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModalEditorCommand {
    /// `lua require('rio.lsp').<method>(<lua_string_literal(value)>)`.
    /// The host concatenates `lua_call_prefix` + literal + `")"`.
    LuaCall {
        lua_call_prefix: String,
        value: String,
    },
    /// `"<command> <value>"` — direct passthrough for unknown labels.
    Raw(String),
}

/// Decide which embedded-nvim command an input-modal commit should
/// dispatch. Pure projection — host owns `send_editor_command`.
pub fn modal_input_editor_command(command: &str, value: &str) -> ModalEditorCommand {
    match command {
        "Rename" => ModalEditorCommand::LuaCall {
            lua_call_prefix: "lua require('rio.lsp').rename_apply(".to_string(),
            value: value.to_string(),
        },
        "WorkspaceSymbols" => ModalEditorCommand::LuaCall {
            lua_call_prefix: "lua require('rio.lsp').workspace_symbols(".to_string(),
            value: value.to_string(),
        },
        other => ModalEditorCommand::Raw(format!("{other} {value}")),
    }
}

/// Inputs for [`lsp_missing_modal_body`].
#[derive(Debug, Clone, Copy)]
pub struct LspMissingModalBodyInput<'a> {
    pub binary: &'a str,
    pub filetype_label: &'a str,
    /// Whether the host could resolve a Mason manifest for either the
    /// server name or the binary. Drives the body copy — "Neoism can
    /// install" vs. "install it manually".
    pub has_installer_spec: bool,
}

/// Pure body-copy resolver for the missing-LSP modal. The host owns
/// the `ModalSpec` construction (buttons, title, meta) and the
/// `BTreeSet` dedupe; the policy just settles which body string runs
/// so multiple frontends agree on the copy.
pub fn lsp_missing_modal_body(input: LspMissingModalBodyInput<'_>) -> String {
    if input.has_installer_spec {
        format!(
            "`{}` is missing for {}. Neoism can install and manage this LSP without Mason or user nvim plugins.",
            input.binary, input.filetype_label
        )
    } else {
        format!(
            "`{}` is missing for {}. Neoism does not have an installer for this server yet, so install it manually and reopen the buffer.",
            input.binary, input.filetype_label
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metrics(has_buffer_tabs: bool) -> WorkspaceChromeMetrics {
        WorkspaceChromeMetrics {
            margin_top: 4.0,
            margin_bottom: 6.0,
            island_top: 20.0,
            buffer_tabs_height: 30.0,
            breadcrumbs_height: 12.0,
            status_line_height: 18.0,
            terminal_top_padding: 7.0,
            has_buffer_tabs,
            chrome_safety_pad: 2.0,
        }
    }

    #[test]
    fn editor_reserves_full_workspace_chrome() {
        assert_eq!(workspace_chrome_margins(metrics(false)).editor_top, 68.0);
        assert_eq!(workspace_chrome_margins(metrics(true)).editor_top, 68.0);
    }

    #[test]
    fn terminal_only_reserves_tab_band_when_tabs_exist() {
        assert_eq!(workspace_chrome_margins(metrics(false)).terminal_top, 31.0);
        assert_eq!(workspace_chrome_margins(metrics(true)).terminal_top, 61.0);
    }

    #[test]
    fn bottom_reserves_status_line() {
        assert_eq!(workspace_chrome_margins(metrics(true)).bottom, 18.0);
    }

    #[test]
    fn editor_chrome_masks_only_hidden_top_buffer() {
        let masks = editor_chrome_mask_rects(EditorChromeMaskInput {
            panel_left: 10.0,
            panel_width: 80.0,
            pane_top: 24.0,
            chrome_height: 40.0,
        });

        assert_eq!(
            masks,
            vec![EditorChromeMaskRect {
                x: 10.0,
                y: 0.0,
                width: 80.0,
                height: 24.0,
            }]
        );
    }

    #[test]
    fn editor_chrome_top_mask_stops_at_chrome_height() {
        let masks = editor_chrome_mask_rects(EditorChromeMaskInput {
            panel_left: 0.0,
            panel_width: 100.0,
            pane_top: 60.0,
            chrome_height: 30.0,
        });

        assert_eq!(
            masks,
            vec![EditorChromeMaskRect {
                x: 0.0,
                y: 0.0,
                width: 100.0,
                height: 30.0,
            }]
        );
    }

    #[test]
    fn editor_chrome_top_mask_skips_pane_at_window_top() {
        let masks = editor_chrome_mask_rects(EditorChromeMaskInput {
            panel_left: 0.0,
            panel_width: 100.0,
            pane_top: 0.0,
            chrome_height: 30.0,
        });

        assert!(masks.is_empty());
    }

    #[test]
    fn editor_rows_fit_exactly_between_breadcrumbs_and_status() {
        let fit = fit_editor_rows(EditorRowFitInput {
            scaled_margin_top: 110.0,
            layout_top: 0.0,
            layout_height: 613.7143,
            window_height: 752.0,
            status_line_height: 47.142857,
            nominal_cell_height: 35.0,
        });

        assert_eq!(fit.rows, 17);
        assert_eq!(fit.usable_height, 595.0);
        assert_eq!(fit.cell_height, 35.0);
    }

    #[test]
    fn editor_rows_distribute_fractional_remainder_instead_of_leaving_band() {
        let fit = fit_editor_rows(EditorRowFitInput {
            scaled_margin_top: 140.0,
            layout_top: 0.0,
            layout_height: 564.0,
            window_height: 752.0,
            status_line_height: 47.0,
            nominal_cell_height: 35.0,
        });

        assert_eq!(fit.rows, 16);
        assert_eq!(fit.usable_height, 564.0);
        assert_eq!(fit.cell_height, 35.25);
        assert_eq!(f32::from(fit.rows) * fit.cell_height, fit.usable_height);
    }

    #[test]
    fn grid_panel_geometry_offsets_editor_hidden_top_buffer() {
        let geometry = grid_panel_chrome_geometry(GridPanelChromeGeometryInput {
            is_editor: true,
            scaled_margin_left: 10.5,
            scaled_margin_top: 5.5,
            layout_left: 2.0,
            layout_top: 20.0,
            layout_width: 300.0,
            layout_height: 160.0,
            cell_height: 18.0,
            rows: 8,
            visible_row_count: 8,
            terminal_reserved_bottom_rows: 0,
            editor_buffer_above: 1,
            terminal_buffer_above: 1,
            terminal_bottom_clip_bleed_px: 2.0,
        });

        assert_eq!(
            geometry,
            GridPanelChromeGeometry {
                panel_left: 13.0,
                panel_top: 8.0,
                panel_clip_top: 26.0,
                clip_rect: [13.0, 26.0, 300.0, 144.0],
            }
        );
    }

    #[test]
    fn grid_panel_geometry_clips_terminal_to_visible_rows() {
        let geometry = grid_panel_chrome_geometry(GridPanelChromeGeometryInput {
            is_editor: false,
            scaled_margin_left: 10.5,
            scaled_margin_top: 5.5,
            layout_left: 2.0,
            layout_top: 20.0,
            layout_width: 303.4,
            layout_height: 200.0,
            cell_height: 18.0,
            rows: 20,
            visible_row_count: 4,
            terminal_reserved_bottom_rows: 2,
            editor_buffer_above: 1,
            terminal_buffer_above: 1,
            terminal_bottom_clip_bleed_px: 2.0,
        });

        assert_eq!(
            geometry,
            GridPanelChromeGeometry {
                panel_left: 13.0,
                panel_top: 8.0,
                panel_clip_top: 26.0,
                clip_rect: [13.0, 26.0, 303.0, 74.0],
            }
        );
    }

    #[test]
    fn trail_cursor_popovers_claim_priority_even_without_rects() {
        let state = TrailCursorOverlayState {
            finder_enabled: true,
            command_palette_enabled: true,
            tab_cursor_available: true,
            trail_cursor_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            trail_cursor_overlay_target(state),
            Some(TrailCursorOverlayTarget::Finder)
        );

        let state = TrailCursorOverlayState {
            command_palette_enabled: true,
            tab_cursor_available: true,
            trail_cursor_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            trail_cursor_overlay_target(state),
            Some(TrailCursorOverlayTarget::CommandPalette)
        );
    }

    #[test]
    fn trail_cursor_input_overlay_suppresses_lower_priority_cursors() {
        let state = TrailCursorOverlayState {
            search_active: true,
            agent_input_cursor_available: true,
            markdown_cursor_available: true,
            terminal_block_input_active: true,
            trail_cursor_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            trail_cursor_overlay_target(state),
            Some(TrailCursorOverlayTarget::SuppressedByInputOverlay)
        );
    }

    #[test]
    fn trail_cursor_falls_back_to_terminal_only_when_enabled() {
        let state = TrailCursorOverlayState {
            terminal_block_input_active: true,
            trail_cursor_enabled: true,
            ..Default::default()
        };

        assert_eq!(
            trail_cursor_overlay_target(state),
            Some(TrailCursorOverlayTarget::TerminalBlockInput)
        );

        let state = TrailCursorOverlayState {
            terminal_block_input_active: true,
            trail_cursor_enabled: false,
            ..Default::default()
        };

        assert_eq!(trail_cursor_overlay_target(state), None);
    }

    #[test]
    fn trail_cursor_draw_kind_classifies_native_render_paths() {
        use TrailCursorOverlayDrawKind as DrawKind;
        use TrailCursorOverlayTarget as Target;

        for target in [
            Target::Finder,
            Target::CommandPalette,
            Target::ContextMenu,
            Target::FileTree,
            Target::AgentSidePanel,
            Target::Tabs,
            Target::GitDiffPanel,
        ] {
            assert_eq!(trail_cursor_overlay_draw_kind(target), DrawKind::ChromeRect);
        }

        assert_eq!(
            trail_cursor_overlay_draw_kind(Target::SuppressedByInputOverlay),
            DrawKind::Suppressed
        );
        assert_eq!(
            trail_cursor_overlay_draw_kind(Target::AgentInput),
            DrawKind::ContentCursor
        );
        assert_eq!(
            trail_cursor_overlay_draw_kind(Target::Markdown),
            DrawKind::ContentCursor
        );
        assert_eq!(
            trail_cursor_overlay_draw_kind(Target::TerminalBlockInput),
            DrawKind::ContentCursor
        );
        assert_eq!(
            trail_cursor_overlay_draw_kind(Target::TerminalGrid),
            DrawKind::TerminalGrid
        );
    }

    #[test]
    fn resize_chrome_extra_editor_includes_breadcrumbs_and_pad() {
        let extra = resize_chrome_extra(ResizeChromeExtraInput {
            current_is_editor: true,
            has_buffer_tabs: false,
            buffer_tabs_height: 28.0,
            breadcrumbs_height: 22.0,
            terminal_top_padding: 7.0,
            chrome_safety_pad: 3.0,
        });
        assert_eq!(extra, 53.0);
    }

    #[test]
    fn resize_chrome_extra_terminal_with_tabs_adds_strip_and_top_pad() {
        let extra = resize_chrome_extra(ResizeChromeExtraInput {
            current_is_editor: false,
            has_buffer_tabs: true,
            buffer_tabs_height: 28.0,
            breadcrumbs_height: 22.0,
            terminal_top_padding: 7.0,
            chrome_safety_pad: 0.0,
        });
        assert_eq!(extra, 35.0);
    }

    #[test]
    fn resize_chrome_extra_terminal_without_tabs_is_just_top_pad() {
        let extra = resize_chrome_extra(ResizeChromeExtraInput {
            current_is_editor: false,
            has_buffer_tabs: false,
            buffer_tabs_height: 28.0,
            breadcrumbs_height: 22.0,
            terminal_top_padding: 7.0,
            chrome_safety_pad: 0.0,
        });
        assert_eq!(extra, 7.0);
    }

    #[test]
    fn resize_chrome_extra_editor_ignores_has_buffer_tabs() {
        let with_tabs = resize_chrome_extra(ResizeChromeExtraInput {
            current_is_editor: true,
            has_buffer_tabs: true,
            buffer_tabs_height: 28.0,
            breadcrumbs_height: 22.0,
            terminal_top_padding: 7.0,
            chrome_safety_pad: 3.0,
        });
        let without_tabs = resize_chrome_extra(ResizeChromeExtraInput {
            current_is_editor: true,
            has_buffer_tabs: false,
            buffer_tabs_height: 28.0,
            breadcrumbs_height: 22.0,
            terminal_top_padding: 7.0,
            chrome_safety_pad: 3.0,
        });
        assert_eq!(with_tabs, without_tabs);
    }

    #[test]
    fn island_drag_tab_geometry_splits_evenly_after_margins() {
        let geom = island_drag_tab_geometry(IslandDragTabGeometryInput {
            window_width_px: 800.0,
            scale_factor: 2.0,
            num_tabs: 4,
            left_margin: 76.0,
            margin_right: 8.0,
        });
        // logical_width = 400, available = 400 - 8 - 76 = 316, tab = 79
        assert_eq!(geom.available_width, 316.0);
        assert_eq!(geom.tab_width, 79.0);
    }

    #[test]
    fn island_drag_tab_geometry_zero_tabs_does_not_divide_by_zero() {
        let geom = island_drag_tab_geometry(IslandDragTabGeometryInput {
            window_width_px: 400.0,
            scale_factor: 1.0,
            num_tabs: 0,
            left_margin: 0.0,
            margin_right: 8.0,
        });
        assert_eq!(geom.tab_width, 0.0);
        assert_eq!(geom.available_width, 392.0);
    }

    #[test]
    fn island_drag_tab_geometry_clamps_negative_available_width() {
        // Margins exceed logical window — available width should clamp to 0.
        let geom = island_drag_tab_geometry(IslandDragTabGeometryInput {
            window_width_px: 100.0,
            scale_factor: 1.0,
            num_tabs: 3,
            left_margin: 80.0,
            margin_right: 40.0,
        });
        assert_eq!(geom.available_width, 0.0);
        assert_eq!(geom.tab_width, 0.0);
    }

    #[test]
    fn lsp_missing_modal_descriptor_prefers_name_then_binary() {
        let desc = lsp_missing_modal_descriptor(LspMissingNotificationInput {
            name: Some("rust-analyzer".to_string()),
            binary: Some("ra".to_string()),
            filetype: Some("rust".to_string()),
        });
        assert_eq!(desc.server, "rust-analyzer");
        assert_eq!(desc.binary, "ra");
        assert_eq!(desc.filetype_label, "rust");
        assert_eq!(desc.dedupe_key, "rust-analyzer:rust");
    }

    #[test]
    fn lsp_missing_modal_descriptor_falls_back_to_binary_when_name_blank() {
        let desc = lsp_missing_modal_descriptor(LspMissingNotificationInput {
            name: Some(String::new()),
            binary: Some("pyright".to_string()),
            filetype: Some("python".to_string()),
        });
        assert_eq!(desc.server, "pyright");
        assert_eq!(desc.binary, "pyright");
        assert_eq!(desc.dedupe_key, "pyright:python");
    }

    #[test]
    fn lsp_missing_modal_descriptor_defaults_when_all_blank() {
        let desc = lsp_missing_modal_descriptor(LspMissingNotificationInput {
            name: None,
            binary: None,
            filetype: None,
        });
        assert_eq!(desc.server, "language server");
        assert_eq!(desc.binary, "language server");
        assert_eq!(desc.filetype_label, "this filetype");
        assert_eq!(desc.dedupe_key, "language server:");
    }

    #[test]
    fn lsp_missing_modal_dedupe_key_is_stable_across_calls() {
        let a = lsp_missing_modal_descriptor(LspMissingNotificationInput {
            name: Some("gopls".to_string()),
            binary: Some("gopls".to_string()),
            filetype: Some("go".to_string()),
        });
        let b = lsp_missing_modal_descriptor(LspMissingNotificationInput {
            name: Some("gopls".to_string()),
            binary: None,
            filetype: Some("go".to_string()),
        });
        assert_eq!(a.dedupe_key, b.dedupe_key);
    }

    #[test]
    fn modal_action_dispatch_close_is_pure_close() {
        assert_eq!(
            modal_action_dispatch(ModalActionTag::Close),
            ModalActionDispatch::Close
        );
    }

    #[test]
    fn modal_action_dispatch_install_paths_keep_modal_open() {
        for tag in [
            ModalActionTag::InstallLsp,
            ModalActionTag::InstallTreesitter,
            ModalActionTag::ApplyTheme,
            ModalActionTag::ApplyShaderOverlay,
            ModalActionTag::InstallAgent,
        ] {
            assert_eq!(
                modal_action_dispatch(tag),
                ModalActionDispatch::KeepOpenForBusyAction,
                "tag {:?} should keep modal open",
                tag
            );
        }
    }

    #[test]
    fn modal_action_dispatch_run_paths_close_before_action() {
        for tag in [
            ModalActionTag::RunEditorCommand,
            ModalActionTag::RunAgent,
            ModalActionTag::AcpPermission,
            ModalActionTag::FileTreeEdit,
            ModalActionTag::FileTreeCopy,
            ModalActionTag::FileTreePaste,
            ModalActionTag::FileTreeDelete,
            ModalActionTag::FileTreeNewFile,
            ModalActionTag::FileTreeNewFolder,
            ModalActionTag::FileTreeRename,
        ] {
            assert_eq!(
                modal_action_dispatch(tag),
                ModalActionDispatch::CloseBeforeAction,
                "tag {:?} should close before action",
                tag
            );
        }
    }

    #[test]
    fn modal_action_dispatch_validated_input_distinguishes_from_close_before() {
        assert_eq!(
            modal_action_dispatch(ModalActionTag::RunEditorCommandWithInput),
            ModalActionDispatch::CloseAfterValidatedInput
        );
    }

    #[test]
    fn modal_action_dispatch_prompt_paths_open_followup() {
        for tag in [
            ModalActionTag::FileTreePromptDelete,
            ModalActionTag::FileTreePromptNewFile,
            ModalActionTag::FileTreePromptNewFolder,
            ModalActionTag::FileTreePromptRename,
        ] {
            assert_eq!(
                modal_action_dispatch(tag),
                ModalActionDispatch::OpenFollowupPrompt,
                "tag {:?} should open followup prompt",
                tag
            );
        }
    }

    #[test]
    fn island_drag_move_outcome_idle_returns_inert_decision() {
        let outcome = island_drag_move_outcome(IslandDragMoveInput {
            swap: None,
            is_dragging: false,
            is_detach_armed: false,
        });
        assert_eq!(outcome.perform_swap, None);
        assert!(!outcome.mark_dirty);
        assert!(!outcome.drag_was_live);
    }

    #[test]
    fn island_drag_move_outcome_live_drag_marks_dirty() {
        let outcome = island_drag_move_outcome(IslandDragMoveInput {
            swap: None,
            is_dragging: true,
            is_detach_armed: false,
        });
        assert_eq!(outcome.perform_swap, None);
        assert!(outcome.mark_dirty);
        assert!(outcome.drag_was_live);
    }

    #[test]
    fn island_drag_move_outcome_swap_surfaces_indices() {
        let outcome = island_drag_move_outcome(IslandDragMoveInput {
            swap: Some((2, 5)),
            is_dragging: true,
            is_detach_armed: false,
        });
        assert_eq!(outcome.perform_swap, Some((2, 5)));
        assert!(outcome.mark_dirty);
        assert!(outcome.drag_was_live);
    }

    #[test]
    fn island_drag_move_outcome_detach_armed_still_marks_dirty() {
        // Detach-armed paints a ghost preview; the host still needs a
        // repaint to keep the floating tab tracking the cursor.
        let outcome = island_drag_move_outcome(IslandDragMoveInput {
            swap: None,
            is_dragging: true,
            is_detach_armed: true,
        });
        assert!(outcome.mark_dirty);
        assert!(outcome.drag_was_live);
    }

    #[test]
    fn modal_input_editor_command_rename_wraps_in_lua_call() {
        let cmd = modal_input_editor_command("Rename", "new_name");
        match cmd {
            ModalEditorCommand::LuaCall {
                lua_call_prefix,
                value,
            } => {
                assert_eq!(lua_call_prefix, "lua require('rio.lsp').rename_apply(");
                assert_eq!(value, "new_name");
            }
            other => panic!("expected LuaCall, got {other:?}"),
        }
    }

    #[test]
    fn modal_input_editor_command_workspace_symbols_wraps_in_lua_call() {
        let cmd = modal_input_editor_command("WorkspaceSymbols", "Foo");
        match cmd {
            ModalEditorCommand::LuaCall {
                lua_call_prefix,
                value,
            } => {
                assert_eq!(lua_call_prefix, "lua require('rio.lsp').workspace_symbols(");
                assert_eq!(value, "Foo");
            }
            other => panic!("expected LuaCall, got {other:?}"),
        }
    }

    #[test]
    fn modal_input_editor_command_unknown_falls_through_to_raw() {
        let cmd = modal_input_editor_command("Edit", "src/lib.rs");
        assert_eq!(cmd, ModalEditorCommand::Raw("Edit src/lib.rs".to_string()));
    }

    #[test]
    fn lsp_missing_modal_body_with_installer_promises_managed_install() {
        let body = lsp_missing_modal_body(LspMissingModalBodyInput {
            binary: "rust-analyzer",
            filetype_label: "rust",
            has_installer_spec: true,
        });
        assert!(body.contains("rust-analyzer"));
        assert!(body.contains("rust"));
        assert!(body.contains("Neoism can install and manage"));
    }

    #[test]
    fn lsp_missing_modal_body_without_installer_tells_user_to_install_manually() {
        let body = lsp_missing_modal_body(LspMissingModalBodyInput {
            binary: "exotic-ls",
            filetype_label: "exotic",
            has_installer_spec: false,
        });
        assert!(body.contains("exotic-ls"));
        assert!(body.contains("exotic"));
        assert!(body.contains("install it manually"));
    }
}
