//! Right-click context menu. Thin shell around `Popover<Menu<...>>` —
//! the popover owns visibility + dismissal, the menu owns selection and
//! scrolling. This file is only responsible for visuals (rounded body,
//! accent strip, row layout) and for translating call-sites' API
//! (`open(title, items, x, y, ...)`, `hover()`, `hit_test()`, etc.).
//!
//! Lifted verbatim from
//! `frontends/neoism/src/chrome/panels/context_menu.rs`.
//!
//! TODO(wave6-cutover): this panel sits on top of four pieces that
//! haven't been lifted yet:
//!
//! * `Popover<T>` + `PopoverAnchor` — overlay/anchor logic.
//! * `Menu<T>` + `MenuItem` — list / shortcut / scrolling state.
//! * `scrollbar` — thumb rendering helper.
//! * `ModalAction` (from `widgets::modal`) — the modal-action variant
//!   of `ContextMenuAction`.
//! * `PaletteAction` — currently lives in `chrome/panels/command_palette`.
//!   The shared crate has a slim `CommandPalette` but no `PaletteAction`
//!   enum yet; the lifted variant points at the eventual home.
//!
//! Imports below resolve to `crate::widgets::*` and to neighbour panel
//! modules that don't exist yet in the shared crate. The host wires
//! `lib.rs`/`panels/mod.rs` after the widget lift commits.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::editor::markdown::MarkdownBlockTemplate;

use crate::primitives::IdeTheme;
// TODO(wave6-cutover): pending widget lift — see module docstring.
use super::command_palette::PaletteAction;
use crate::widgets::menu::{Menu, MenuItem};
use crate::widgets::modal::ModalAction;
use crate::widgets::popover::{Popover, PopoverAnchor};
use crate::widgets::scrollbar;

/// Publish state of a workspace on its daemon — still used by the
/// palette + web strip badges. The old chrome menu that acted on it
/// (Share / Stop Sharing / Send to Docker Sandbox / Send to Cloud) was
/// from the pre-server workspace iteration and is gone; joining a
/// server IS the sharing model now.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceChromeVisibility {
    Private,
    Shared,
    Team,
}

const MENU_WIDTH: f32 = 270.0;
const PREVIEW_MENU_WIDTH: f32 = 360.0;
const PREVIEW_COLUMN_WIDTH: f32 = 82.0;
const MENU_PADDING: f32 = 6.0;
const MENU_MARGIN: f32 = 8.0;
const MENU_RADIUS: f32 = 8.0;
const TITLE_FONT_SIZE: f32 = 11.0;
const ITEM_FONT_SIZE: f32 = 13.0;
const HINT_FONT_SIZE: f32 = 11.0;
const TITLE_HEIGHT: f32 = 22.0;
const ITEM_HEIGHT: f32 = 30.0;
const MAX_VISIBLE_ITEMS: usize = 10;
const SEPARATOR_HEIGHT: f32 = 1.0;
const DEPTH_BG: f32 = 0.1;
const DEPTH_ELEMENT: f32 = 0.2;
const ORDER: u8 = 26;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ContextMenuAction {
    Palette(PaletteAction),
    Modal(ModalAction),
    Lsp(LspContextAction),
    Workspace(WorkspaceContextAction),
    Notebook(NotebookContextAction),
    MarkdownBlock(MarkdownBlockTemplate),
    MarkdownLinkCompletion(String),
    MarkdownSpellingReplace {
        line: usize,
        start: usize,
        end: usize,
        replacement: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NotebookContextAction {
    SelectKernel {
        name: String,
        display_name: String,
        language: String,
    },
}

/// Right-click actions on a top-level workspace ("Island") tab. `index`
/// is the workspace tab the menu was opened on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkspaceContextAction {
    /// Tear the workspace out into its own OS window, keeping its live
    /// session (same hand-off as the drag-detach gesture).
    Detach { index: usize },
    /// Close the workspace and everything in it.
    Close { index: usize },
    /// Move a buffer tab from the current workspace into another
    /// top-level workspace **in the same window**. `tab_index` is the
    /// buffer-tab strip index; `target_workspace` is the destination
    /// Island tab index.
    MoveBufferTab {
        tab_index: usize,
        target_workspace: usize,
    },
    /// Move a buffer tab into a workspace that lives in a **different OS
    /// window** (e.g. a detached workspace). `target_window` is the
    /// destination native window id (`u64::from(WindowId)`);
    /// `target_workspace` is the Island tab index within that window.
    MoveBufferTabToWindow {
        tab_index: usize,
        target_window: u64,
        target_workspace: usize,
    },
    /// Open the rename prompt for the buffer tab at `tab_index`
    /// (right-click → Rename). The host fills the modal input with the
    /// current title; on submit it relabels the tab locally and, for
    /// agent tabs, publishes the new title at the daemon level.
    RenameBufferTab { tab_index: usize },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LspContextAction {
    Hover,
    Definition,
    References,
    CodeAction,
    Rename,
    Format,
    DocumentSymbols,
    WorkspaceSymbols,
    ToggleInlayHints,
    Info,
}

impl LspContextAction {
    pub fn command_key(self) -> Option<&'static str> {
        Some(match self {
            Self::Hover => "hover",
            Self::Definition => "definition",
            Self::References => "references",
            Self::CodeAction => "code_action",
            Self::Format => "format",
            Self::DocumentSymbols => "document_symbols",
            Self::ToggleInlayHints => "toggle_inlay_hints",
            Self::Info => "info",
            Self::Rename | Self::WorkspaceSymbols => return None,
        })
    }
}

#[derive(Clone, Debug)]
pub struct ContextMenuItem {
    pub label: String,
    pub hint: String,
    pub preview: String,
    pub action: ContextMenuAction,
    pub enabled: bool,
}

impl ContextMenuItem {
    pub fn new(
        label: impl Into<String>,
        hint: impl Into<String>,
        action: ContextMenuAction,
    ) -> Self {
        Self {
            label: label.into(),
            hint: hint.into(),
            preview: String::new(),
            action,
            enabled: true,
        }
    }

    pub fn with_preview(mut self, preview: impl Into<String>) -> Self {
        self.preview = preview.into();
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ContextMenuMode {
    Generic,
    NotebookKernel,
    MarkdownBlock,
}

pub struct ContextMenu {
    popover: Popover<Menu<ContextMenuAction>>,
    title: String,
    /// We preserve the raw `hint` strings separately because Menu's
    /// `MenuItem.shortcut` is `Option<String>` (None when blank), but
    /// we still want to draw an empty hint column slot for visual
    /// consistency. Indexed parallel to popover.content().items().
    hints: Vec<String>,
    previews: Vec<String>,
    source_items: Vec<ContextMenuItem>,
    query: String,
    mode: ContextMenuMode,
    wheel_accumulator: f32,
    x: f32,
    y: f32,
    scale: f32,
    last_rect: [f32; 4],
    selected_cursor_rect: Option<[f32; 4]>,
    /// Text row this menu is anchored to (top, bottom, flipped-above).
    /// While set, repositioning after filter-driven size changes keeps the
    /// menu glued to the row — bottom-anchored when flipped — instead of
    /// drifting away as it shrinks.
    row_anchor: Option<(f32, f32, bool)>,
}

impl ContextMenu {
    pub fn new() -> Self {
        Self {
            popover: Popover::new(Menu::new().with_max_visible(MAX_VISIBLE_ITEMS)),
            title: String::new(),
            hints: Vec::new(),
            previews: Vec::new(),
            source_items: Vec::new(),
            query: String::new(),
            mode: ContextMenuMode::Generic,
            wheel_accumulator: 0.0,
            x: 0.0,
            y: 0.0,
            scale: 1.0,
            last_rect: [0.0; 4],
            selected_cursor_rect: None,
            row_anchor: None,
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    pub fn is_visible(&self) -> bool {
        self.popover.is_visible()
    }

    pub fn rect(&self) -> Option<[f32; 4]> {
        self.is_visible().then_some(self.last_rect)
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn is_markdown_link_completion(&self) -> bool {
        self.is_visible()
            && self.popover.content().items().iter().any(|item| {
                matches!(item.action, ContextMenuAction::MarkdownLinkCompletion(_))
            })
    }

    pub fn is_markdown_block_completion(&self) -> bool {
        self.is_visible() && self.mode == ContextMenuMode::MarkdownBlock
    }

    pub fn open(
        &mut self,
        title: impl Into<String>,
        items: Vec<ContextMenuItem>,
        x: f32,
        y: f32,
        window_width: f32,
        window_height: f32,
    ) {
        self.title = title.into();
        self.mode = ContextMenuMode::Generic;
        self.row_anchor = None;
        self.query.clear();
        self.source_items = items;
        let empty = self.rebuild_menu_items(false);
        self.update_visible_limit(window_height);
        self.fit_visible_limit(window_height);
        self.wheel_accumulator = 0.0;
        self.selected_cursor_rect = None;

        let (w, h) = self.dimensions();
        self.x = x.clamp(
            MENU_MARGIN,
            (window_width - w - MENU_MARGIN).max(MENU_MARGIN),
        );
        self.y = y.clamp(
            MENU_MARGIN,
            (window_height - h - MENU_MARGIN).max(MENU_MARGIN),
        );
        self.last_rect = [self.x, self.y, w, h];
        self.popover
            .set_anchor(PopoverAnchor::Point([self.x, self.y]));

        if empty {
            self.popover.close_instant();
        } else {
            // Context menus snap open — no fade — matching the original
            // behavior. Users perceive any animation on a right-click
            // menu as latency.
            self.popover.open_instant();
        }
    }

    pub fn open_notebook_kernel(
        &mut self,
        title: impl Into<String>,
        items: Vec<ContextMenuItem>,
        x: f32,
        y: f32,
        window_width: f32,
        window_height: f32,
    ) {
        self.open(title, items, x, y, window_width, window_height);
        self.mode = ContextMenuMode::NotebookKernel;
    }

    pub fn is_notebook_kernel(&self) -> bool {
        self.is_visible() && self.mode == ContextMenuMode::NotebookKernel
    }

    /// Open anchored to a text row: below `row_bottom` when the menu fits,
    /// otherwise flipped ABOVE `row_top` — never clamped onto the row
    /// itself (the plain `open` clamp shoved the link-completion menu up
    /// over the exact line being typed near the window bottom).
    #[allow(clippy::too_many_arguments)]
    pub fn open_avoiding_row(
        &mut self,
        title: impl Into<String>,
        items: Vec<ContextMenuItem>,
        x: f32,
        row_top: f32,
        row_bottom: f32,
        window_width: f32,
        window_height: f32,
    ) {
        self.open(
            title,
            items,
            x,
            row_bottom + 6.0,
            window_width,
            window_height,
        );
        self.avoid_row(row_top, row_bottom);
    }

    /// Post-open adjustment: when the bottom clamp pulled the menu up onto
    /// the given text row, flip it to sit above the row instead. The chosen
    /// side sticks for the menu's lifetime (filter updates re-anchor to the
    /// same side via `reapply_row_anchor`).
    pub fn avoid_row(&mut self, row_top: f32, row_bottom: f32) {
        let above = self.y < row_bottom + 6.0;
        self.row_anchor = Some((row_top, row_bottom, above));
        self.reapply_row_anchor();
    }

    /// Re-glue the menu to its text row after a size change: below-anchored
    /// menus keep their top at the row; flipped menus keep their BOTTOM at
    /// the row (a shrinking top-anchored flipped menu drifted upward, away
    /// from the line being typed).
    fn reapply_row_anchor(&mut self) {
        let Some((row_top, row_bottom, above)) = self.row_anchor else {
            return;
        };
        let (w, h) = self.dimensions();
        let y = if above {
            (row_top - 6.0 - h).max(MENU_MARGIN)
        } else {
            row_bottom + 6.0
        };
        self.y = y;
        self.last_rect = [self.x, self.y, w, h];
        self.popover
            .set_anchor(PopoverAnchor::Point([self.x, self.y]));
    }

    pub fn open_markdown_block(
        &mut self,
        title: impl Into<String>,
        items: Vec<ContextMenuItem>,
        query: impl Into<String>,
        x: f32,
        y: f32,
        window_width: f32,
        window_height: f32,
    ) {
        self.title = title.into();
        self.mode = ContextMenuMode::MarkdownBlock;
        self.row_anchor = None;
        self.query = query.into();
        self.source_items = items;
        let source_empty = self.source_items.is_empty();
        self.rebuild_menu_items(true);
        self.update_visible_limit(window_height);
        self.fit_visible_limit(window_height);
        self.wheel_accumulator = 0.0;
        self.selected_cursor_rect = None;

        let (w, h) = self.dimensions();
        self.x = x.clamp(
            MENU_MARGIN,
            (window_width - w - MENU_MARGIN).max(MENU_MARGIN),
        );
        self.y = y.clamp(
            MENU_MARGIN,
            (window_height - h - MENU_MARGIN).max(MENU_MARGIN),
        );
        self.last_rect = [self.x, self.y, w, h];
        self.popover
            .set_anchor(PopoverAnchor::Point([self.x, self.y]));

        if source_empty {
            self.popover.close_instant();
        } else {
            self.popover.open_instant();
        }
    }

    pub fn set_markdown_block_query(&mut self, query: impl Into<String>) -> bool {
        if self.mode != ContextMenuMode::MarkdownBlock {
            return false;
        }
        let query = query.into();
        if self.query == query {
            return false;
        }
        self.query = query;
        self.rebuild_menu_items(true);
        self.reapply_row_anchor();
        true
    }

    pub fn close(&mut self) {
        self.popover.close_instant();
        self.popover.content_mut().clear();
        self.title.clear();
        self.hints.clear();
        self.previews.clear();
        self.source_items.clear();
        self.query.clear();
        self.mode = ContextMenuMode::Generic;
        self.wheel_accumulator = 0.0;
        self.last_rect = [0.0; 4];
        self.selected_cursor_rect = None;
        self.row_anchor = None;
        self.popover
            .content_mut()
            .set_max_visible(MAX_VISIBLE_ITEMS);
    }

    pub fn selected_action(&self) -> Option<ContextMenuAction> {
        self.popover.content().selected_action().cloned()
    }

    pub fn move_selection(&mut self, delta: i32) {
        self.popover.content_mut().move_selection(delta);
    }

    pub fn select_shortcut(&mut self, ch: char) -> Option<ContextMenuAction> {
        if self.mode == ContextMenuMode::MarkdownBlock {
            return None;
        }
        self.popover.content_mut().match_shortcut(ch).cloned()
    }

    pub fn hover(&mut self, mouse_x: f32, mouse_y: f32) -> bool {
        if !self.is_visible() {
            return false;
        }
        match self.hit_test(mouse_x, mouse_y) {
            Ok(Some(index)) => {
                let menu = self.popover.content();
                let already_selected = index == menu.selected_index();
                let enabled = menu.items().get(index).is_some_and(|it| it.enabled);
                if enabled && !already_selected {
                    self.popover.content_mut().set_selected_index(index);
                    return true;
                }
                false
            }
            _ => false,
        }
    }

    pub fn hit_test(&self, mouse_x: f32, mouse_y: f32) -> Result<Option<usize>, ()> {
        if !self.is_visible() {
            return Err(());
        }
        let [x, y, w, h] = self.last_rect;
        if mouse_x < x || mouse_x > x + w || mouse_y < y || mouse_y > y + h {
            return Err(());
        }
        let s = self.scale;
        let rows_y = y + MENU_PADDING * s + self.header_height();
        let row_h = ITEM_HEIGHT * s;
        if mouse_y < rows_y {
            return Ok(None);
        }
        let row = ((mouse_y - rows_y) / row_h).floor() as usize;
        let menu = self.popover.content();
        let index = menu.scroll_offset() + row;
        if row < menu.visible_count() && index < menu.len() {
            Ok(Some(index))
        } else {
            Ok(None)
        }
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32) {
        let row_h = (ITEM_HEIGHT * self.scale).max(1.0);
        let mut acc = self.wheel_accumulator;
        self.popover
            .content_mut()
            .scroll_pixels(delta_pixels, row_h, &mut acc);
        self.wheel_accumulator = acc;
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        dimensions: (f32, f32, f32),
        theme: &IdeTheme,
    ) {
        if !self.is_visible() {
            return;
        }
        let (_, _, scale_factor) = dimensions;
        let s = self.scale;
        let window_w = dimensions.0 / scale_factor;
        let window_h = dimensions.1 / scale_factor;
        self.update_visible_limit(window_h);
        self.fit_visible_limit(window_h);
        let (w, h) = self.dimensions();
        self.x = self
            .x
            .clamp(MENU_MARGIN, (window_w - w - MENU_MARGIN).max(MENU_MARGIN));
        self.y = self
            .y
            .clamp(MENU_MARGIN, (window_h - h - MENU_MARGIN).max(MENU_MARGIN));
        self.last_rect = [self.x, self.y, w, h];

        sugarloaf.rounded_rect(
            None,
            self.x,
            self.y,
            w,
            h,
            theme.f32(theme.panel_bg()),
            DEPTH_BG,
            MENU_RADIUS * s,
            ORDER,
        );
        sugarloaf.rect(
            None,
            self.x,
            self.y,
            3.0 * s,
            h,
            theme.f32(theme.accent),
            DEPTH_ELEMENT,
            ORDER + 1,
        );

        let pad = MENU_PADDING * s;
        let inner_x = self.x + pad;
        let inner_w = w - pad * 2.0;
        let mut y = self.y + pad;
        if !self.title.is_empty() {
            let title_opts = DrawOpts {
                font_size: TITLE_FONT_SIZE * s,
                color: theme.u8(theme.muted),
                bold: true,
                clip_rect: Some([inner_x, y, inner_w, TITLE_HEIGHT * s]),
                ..DrawOpts::default()
            };
            let title =
                truncate_to_fit(&self.title, inner_w - 12.0 * s, sugarloaf, &title_opts);
            sugarloaf.text_mut().draw(
                inner_x + 8.0 * s,
                y + (TITLE_HEIGHT * s - TITLE_FONT_SIZE * s) / 2.0,
                &title,
                &title_opts,
            );
            y += TITLE_HEIGHT * s;
            sugarloaf.rect(
                None,
                inner_x,
                y,
                inner_w,
                SEPARATOR_HEIGHT,
                theme.f32(theme.border),
                DEPTH_ELEMENT,
                ORDER + 1,
            );
            y += SEPARATOR_HEIGHT + 4.0 * s;
        }

        let row_h = ITEM_HEIGHT * s;
        let menu = self.popover.content();
        let visible_count = menu.visible_count();
        let selected_index = menu.selected_index();
        let scroll_offset = menu.scroll_offset();
        let items_len = menu.len();
        let list_clip = [inner_x, y, inner_w, row_h * visible_count as f32];
        let mut next_selected_cursor_rect = None;
        let has_previews = self.has_previews();
        let preview_w = if has_previews {
            PREVIEW_COLUMN_WIDTH * s
        } else {
            0.0
        };
        let hint_opts = DrawOpts {
            font_size: HINT_FONT_SIZE * s,
            color: theme.u8(theme.muted),
            clip_rect: Some(list_clip),
            ..DrawOpts::default()
        };
        let preview_opts = DrawOpts {
            font_size: HINT_FONT_SIZE * s,
            color: theme.u8(theme.accent),
            bold: true,
            clip_rect: Some(list_clip),
            ..DrawOpts::default()
        };

        for (display_index, (index, item)) in menu.visible_items().enumerate() {
            let row_y = y + display_index as f32 * row_h;
            let selected = index == selected_index && item.enabled;
            let hint = self.hints.get(index).map(String::as_str).unwrap_or("");
            let preview = self.previews.get(index).map(String::as_str).unwrap_or("");
            let preview_x = inner_x + 12.0 * s;
            let label_x = inner_x + 12.0 * s + preview_w;
            if selected {
                sugarloaf.rounded_rect(
                    None,
                    inner_x,
                    row_y,
                    inner_w,
                    row_h,
                    theme.f32(theme.hover),
                    DEPTH_ELEMENT,
                    4.0 * s,
                    ORDER + 1,
                );
                let cursor_w = (ITEM_FONT_SIZE * s * 0.55).max(2.0);
                let cursor_h = (row_h - 8.0 * s).max(ITEM_FONT_SIZE * s).min(row_h);
                let cursor_x = (label_x - cursor_w - 4.0 * s).max(inner_x);
                let cursor_y = row_y + (row_h - cursor_h) * 0.5;
                next_selected_cursor_rect =
                    Some([cursor_x, cursor_y, cursor_w, cursor_h]);
            }

            let hint_w = if hint.is_empty() {
                0.0
            } else {
                sugarloaf.text_mut().measure(hint, &hint_opts)
            };
            let label_y = row_y + (row_h - ITEM_FONT_SIZE * s) / 2.0;
            if has_previews && !preview.is_empty() {
                let preview = truncate_to_fit(
                    preview,
                    (preview_w - 12.0 * s).max(0.0),
                    sugarloaf,
                    &preview_opts,
                );
                sugarloaf.text_mut().draw(
                    preview_x,
                    row_y + (row_h - HINT_FONT_SIZE * s) / 2.0,
                    &preview,
                    &preview_opts,
                );
            }
            let label_opts = DrawOpts {
                font_size: ITEM_FONT_SIZE * s,
                color: if !item.enabled {
                    theme.u8(theme.muted)
                } else if selected {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                },
                bold: selected,
                clip_rect: Some(list_clip),
                ..DrawOpts::default()
            };
            let label_budget =
                (inner_w - preview_w - 24.0 * s - hint_w - 14.0 * s).max(0.0);
            let label =
                truncate_to_fit(&item.label, label_budget, sugarloaf, &label_opts);
            sugarloaf
                .text_mut()
                .draw(label_x, label_y, &label, &label_opts);
            if !hint.is_empty() {
                sugarloaf.text_mut().draw(
                    inner_x + inner_w - 12.0 * s - hint_w,
                    row_y + (row_h - HINT_FONT_SIZE * s) / 2.0,
                    hint,
                    &hint_opts,
                );
            }
        }

        self.selected_cursor_rect = next_selected_cursor_rect;

        if items_len > visible_count {
            let max_offset = items_len.saturating_sub(visible_count);
            let normalized = if max_offset == 0 {
                0.0
            } else {
                scroll_offset as f32 / max_offset as f32
            };
            if let Some((thumb_y, thumb_h)) = scrollbar::compute_thumb(
                visible_count,
                items_len,
                list_clip[1],
                list_clip[3],
                normalized,
            ) {
                let bar_x = inner_x + inner_w - scrollbar::width();
                scrollbar::draw_track(
                    sugarloaf,
                    bar_x,
                    list_clip[1],
                    list_clip[3],
                    0.95,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
                scrollbar::draw_thumb(
                    sugarloaf,
                    bar_x,
                    thumb_y,
                    thumb_h,
                    0.95,
                    false,
                    DEPTH_ELEMENT + 0.05,
                    ORDER + 2,
                );
            }
        }
    }

    fn dimensions(&self) -> (f32, f32) {
        let s = self.scale;
        let visible_count = self.popover.content().visible_count();
        let h = MENU_PADDING * 2.0 * s
            + self.header_height()
            + visible_count as f32 * ITEM_HEIGHT * s;
        (self.menu_width() * s, h.max(MENU_PADDING * 2.0 * s))
    }

    fn menu_width(&self) -> f32 {
        if self.has_previews() {
            PREVIEW_MENU_WIDTH
        } else {
            MENU_WIDTH
        }
    }

    fn has_previews(&self) -> bool {
        self.previews.iter().any(|preview| !preview.is_empty())
    }

    fn rebuild_menu_items(&mut self, keep_empty_markdown_block_open: bool) -> bool {
        let mut items = match self.mode {
            ContextMenuMode::Generic | ContextMenuMode::NotebookKernel => {
                self.source_items.clone()
            }
            ContextMenuMode::MarkdownBlock => {
                filtered_markdown_block_items(&self.source_items, &self.query)
            }
        };
        let empty = items.is_empty();
        if empty && keep_empty_markdown_block_open {
            if let Some(fallback) = self.source_items.first().cloned() {
                let label = if self.query.trim().is_empty() {
                    "No blocks".to_string()
                } else {
                    format!("No matches for /{}", self.query.trim())
                };
                items.push(ContextMenuItem {
                    label,
                    hint: String::new(),
                    preview: String::new(),
                    action: fallback.action,
                    enabled: false,
                });
            }
        }

        self.hints = items.iter().map(|i| i.hint.clone()).collect();
        self.previews = items.iter().map(|i| i.preview.clone()).collect();
        let menu_items = items
            .into_iter()
            .map(|item| {
                let mut mi = MenuItem::new(item.label, item.action);
                mi.enabled = item.enabled;
                if !item.hint.is_empty() {
                    mi = mi.with_shortcut(item.hint);
                }
                mi
            })
            .collect();
        self.popover.content_mut().set_items(menu_items);
        empty
    }

    fn header_height(&self) -> f32 {
        if self.title.is_empty() {
            0.0
        } else {
            (TITLE_HEIGHT + 4.0) * self.scale + SEPARATOR_HEIGHT
        }
    }

    fn update_visible_limit(&mut self, window_height: f32) {
        let s = self.scale;
        let row_h = (ITEM_HEIGHT * s).max(1.0);
        let vertical_shell =
            MENU_MARGIN * 2.0 + MENU_PADDING * 2.0 * s + self.header_height();
        let available = (window_height - vertical_shell).max(row_h);
        let rows_that_fit = (available / row_h).floor().max(1.0) as usize;
        let max_visible = rows_that_fit.min(MAX_VISIBLE_ITEMS).max(1);
        self.popover.content_mut().set_max_visible(max_visible);
    }

    fn fit_visible_limit(&mut self, window_height: f32) {
        let max_h =
            (window_height - MENU_MARGIN * 2.0).max(MENU_PADDING * 2.0 * self.scale);
        while self.popover.content().max_visible() > 1 && self.dimensions().1 > max_h {
            let next = self.popover.content().max_visible() - 1;
            self.popover.content_mut().set_max_visible(next);
        }
    }
}

impl Default for ContextMenu {
    fn default() -> Self {
        Self::new()
    }
}

fn filtered_markdown_block_items(
    items: &[ContextMenuItem],
    query: &str,
) -> Vec<ContextMenuItem> {
    let query = query.trim().trim_start_matches('/').to_ascii_lowercase();
    if query.is_empty() {
        return items.to_vec();
    }

    let mut scored = items
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let label = item.label.to_ascii_lowercase();
            let hint = item.hint.to_ascii_lowercase();
            let preview = item.preview.to_ascii_lowercase();
            let score = if label == query {
                0
            } else if label.starts_with(&query) {
                1
            } else if label
                .split_whitespace()
                .any(|word| word.starts_with(&query))
            {
                2
            } else if label.contains(&query) {
                3
            } else if preview.contains(&query) {
                4
            } else if hint.contains(&query) {
                5
            } else {
                return None;
            };
            Some((score, index, item.clone()))
        })
        .collect::<Vec<_>>();
    scored.sort_by(|left, right| left.0.cmp(&right.0).then(left.1.cmp(&right.1)));
    scored.into_iter().map(|(_, _, item)| item).collect()
}

fn truncate_to_fit(
    text: &str,
    available_w: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    if available_w <= 0.0 || text.is_empty() {
        return String::new();
    }
    if sugarloaf.text_mut().measure(text, opts) <= available_w {
        return text.to_string();
    }
    if sugarloaf.text_mut().measure("…", opts) >= available_w {
        return "…".to_string();
    }

    let chars: Vec<char> = text.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if sugarloaf.text_mut().measure(&candidate, opts) <= available_w {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut out: String = chars[..lo].iter().collect();
    out.push('…');
    out
}
