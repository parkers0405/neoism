// Copyright (c) 2023-present, Raphael Amorim.
//
// This source code is licensed under the MIT license found in the
// LICENSE file in the root directory of this source tree.

//! Palette drawing pipeline: rect primitives, copy icon, and the main
//! `render` method that walks the filtered row list each frame.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;
use web_time::Instant;

use crate::panels::file_tree;
use crate::primitives::IdeTheme;
use crate::widgets::scrollbar;

use super::commands::CommandService;
use super::fuzzy::{ease_out_back, ease_out_cubic, snap_to_device_px, truncate_to_fit};
use super::modes::{PaletteMode, PaletteRow};
use super::state::CommandPalette;
use super::WORKSPACE_ROOT_DETAIL_PREFIX;
use super::{
    CARET_BLINK_MS, CARET_WIDTH, COPY_ICON_H, COPY_ICON_OFFSET, COPY_ICON_PAGE_H,
    COPY_ICON_PAGE_W, COPY_ICON_RADIUS, COPY_ICON_STROKE, COPY_ICON_W,
    CURSOR_ANIMATION_LENGTH, DEPTH_BG, DEPTH_ELEMENT, INPUT_FONT_SIZE, INPUT_HEIGHT,
    INPUT_PADDING_X, LIST_SCROLL_ANIMATION_LENGTH, MAX_VISIBLE_RESULTS, OPEN_POP_MS,
    ORDER, PALETTE_CORNER_RADIUS, PALETTE_PADDING, RESULTS_MARGIN_TOP,
    RESULTS_PADDING_BOTTOM, RESULT_FONT_SIZE, RESULT_ITEM_HEIGHT, SEPARATOR_HEIGHT,
    SHORTCUT_FONT_SIZE,
};

/// Paint a rounded-rect outline by layering two filled rounded rects:
/// the outer one in `stroke_color`, then a smaller one in `fill_color`
/// inset by `stroke` on all sides to carve out the interior. Sugarloaf
/// has no stroked-rect primitive, so this is how we get a 1px border
/// effect. Nine params is the irreducible minimum here — grouping them
/// into a struct would just shuffle the same fields.
#[allow(clippy::too_many_arguments)]
pub(crate) fn stroke_rounded_rect(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke: f32,
    radius: f32,
    stroke_color: [f32; 4],
    fill_color: [f32; 4],
    depth: f32,
    order: u8,
) {
    sugarloaf.overlay_rounded_rect(
        x,
        y,
        width,
        height,
        stroke_color,
        depth,
        radius,
        order,
    );
    let inner_radius = (radius - stroke).max(0.0);
    // Inset fill carves out the interior. Painted slightly deeper so
    // it lands on top of the outer rect.
    sugarloaf.overlay_rounded_rect(
        x + stroke,
        y + stroke,
        (width - stroke * 2.0).max(0.0),
        (height - stroke * 2.0).max(0.0),
        fill_color,
        depth + 0.001,
        inner_radius,
        order,
    );
}

/// Draw a modal card with only its top corners rounded (bottom flush),
/// layering a `stroke_color` outer quad and an inset `fill_color` inner
/// quad.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_modal_frame_top(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    stroke: f32,
    radius: f32,
    stroke_color: [f32; 4],
    fill_color: [f32; 4],
    depth: f32,
    order: u8,
) {
    let outer_radii = [radius, radius, 0.0, 0.0];
    sugarloaf.overlay_quad(x, y, width, height, stroke_color, outer_radii, depth, order);
    let inner_radius = (radius - stroke).max(0.0);
    let inner_radii = [inner_radius, inner_radius, 0.0, 0.0];
    sugarloaf.overlay_quad(
        x + stroke,
        y + stroke,
        (width - stroke * 2.0).max(0.0),
        (height - stroke * 2.0).max(0.0),
        fill_color,
        inner_radii,
        depth + 0.001,
        order,
    );
}

/// Paint a "copy" icon (two overlapping rounded page outlines)
/// anchored at `(x, y)`. Drawn from rects only — no font glyph
/// dependency — so it renders consistently regardless of what the
/// user's font stack can produce for ⎘ / 📋 / similar.
///
/// `row_fill_color` is the background behind the icon (palette BG when
/// the row is idle, selection highlight when hovered/selected); it's
/// used to cut out the page interiors so the outlines read as a
/// proper border rather than two solid blobs. Back page painted
/// slightly below the front via depth so the front's cutout
/// correctly hides the overlapping portion of the back's stroke.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_copy_icon(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    stroke_color: [f32; 4],
    row_fill_color: [f32; 4],
    depth: f32,
    order: u8,
) {
    // Back page (upper-left).
    stroke_rounded_rect(
        sugarloaf,
        x,
        y,
        COPY_ICON_PAGE_W,
        COPY_ICON_PAGE_H,
        COPY_ICON_STROKE,
        COPY_ICON_RADIUS,
        stroke_color,
        row_fill_color,
        depth,
        order,
    );
    // Front page (offset down-right), painted above the back so its
    // cutout hides the back's overlapping interior.
    stroke_rounded_rect(
        sugarloaf,
        x + COPY_ICON_OFFSET,
        y + COPY_ICON_OFFSET,
        COPY_ICON_PAGE_W,
        COPY_ICON_PAGE_H,
        COPY_ICON_STROKE,
        COPY_ICON_RADIUS,
        stroke_color,
        row_fill_color,
        depth + 0.01,
        order,
    );
}

impl CommandPalette {
    pub(super) fn tick_list_scroll(&mut self) -> f32 {
        if self.list_scroll_spring.position == 0.0 {
            self.last_list_scroll_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_list_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_list_scroll_frame = now;
        self.list_scroll_spring
            .update(dt, LIST_SCROLL_ANIMATION_LENGTH);
        self.list_scroll_spring.position
    }

    pub(super) fn tick_cursor(&mut self) -> f32 {
        if self.cursor_spring.position == 0.0 {
            self.last_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_cursor_frame = now;
        self.cursor_spring.update(dt, CURSOR_ANIMATION_LENGTH);
        self.cursor_spring.position
    }

    pub(super) fn open_pop_transform(&self, scale: f32) -> (f32, f32) {
        if !self.pop_on_open {
            return (1.0, 0.0);
        }

        let t = (Instant::now()
            .saturating_duration_since(self.open_pop_started)
            .as_secs_f32()
            * 1000.0
            / OPEN_POP_MS)
            .clamp(0.0, 1.0);
        let eased = ease_out_back(t).min(1.04);
        let pop_scale = 0.94 + (eased * 0.06);
        let pop_offset_y = (1.0 - ease_out_cubic(t)) * 14.0 * scale;
        (pop_scale, pop_offset_y)
    }

    pub fn is_animating(&self) -> bool {
        self.enabled
            && (self.list_scroll_spring.position != 0.0
                || self.cursor_spring.position != 0.0
                || (self.pop_on_open
                    && Instant::now()
                        .saturating_duration_since(self.open_pop_started)
                        .as_secs_f32()
                        * 1000.0
                        < OPEN_POP_MS))
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        dimensions: (f32, f32, f32),
        theme: &IdeTheme,
    ) {
        if !self.enabled {
            // Immediate mode: not drawing == not visible.
            self.selected_cursor_rect = None;
            return;
        }
        self.server_edit_hit = None;
        self.server_remove_hit = None;

        let (window_width, window_height, scale_factor) = dimensions;

        // Scaled aliases — every length flows through `s` so Ctrl+/Ctrl-
        // resizes the whole palette uniformly. Reading these once at the
        // top keeps the render path readable; no `* self.scale` clutter
        // sprinkled through every draw call.
        let s = self.scale;
        let pad = PALETTE_PADDING * s;
        let input_h = INPUT_HEIGHT * s;
        let input_pad_x = INPUT_PADDING_X * s;
        let input_font = INPUT_FONT_SIZE * s;
        let row_h = RESULT_ITEM_HEIGHT * s;
        let result_font = RESULT_FONT_SIZE * s;
        let shortcut_font = SHORTCUT_FONT_SIZE * s;
        let margin_top = RESULTS_MARGIN_TOP * s;
        let caret_w = CARET_WIDTH * s;
        let radius = PALETTE_CORNER_RADIUS * s;
        let icon_w = COPY_ICON_W * s;
        let icon_h = COPY_ICON_H * s;
        let frame_stroke = (file_tree::FRAME_STROKE * s).max(2.0);
        let list_scroll_offset = snap_to_device_px(self.tick_list_scroll(), scale_factor);
        let cursor_offset = self.tick_cursor();

        let (base_palette_x, base_palette_y, base_palette_width, base_palette_height) =
            self.palette_rect(window_width, scale_factor);
        let (pop_scale, pop_offset_y) = self.open_pop_transform(s);
        let palette_width = base_palette_width * pop_scale;
        let palette_height = base_palette_height * pop_scale;
        let palette_x = base_palette_x + (base_palette_width - palette_width) / 2.0;
        let palette_y = base_palette_y + pop_offset_y;

        // Keep the body fully opaque so document/editor text behind it
        // never bleeds through the command line or result rows.
        let _ = (window_width, window_height, scale_factor);

        draw_modal_frame_top(
            sugarloaf,
            palette_x,
            palette_y,
            palette_width,
            palette_height,
            frame_stroke,
            radius,
            theme.f32(theme.surface),
            theme.f32(theme.panel_bg()),
            DEPTH_BG,
            ORDER,
        );

        let content_x = palette_x + frame_stroke;
        let content_y = palette_y + frame_stroke;
        let content_w = (palette_width - frame_stroke * 2.0).max(0.0);
        let input_x = content_x + pad;
        let input_y = content_y + pad;
        let input_width = (content_w - pad * 2.0).max(0.0);
        let input_clip = [input_x, input_y, input_width, input_h];

        // No separate input background — blends with palette bg for minimalism

        let placeholder = match self.mode {
            PaletteMode::Commands => "Type a command...",
            PaletteMode::Fonts(_) => "Type a font name...",
            PaletteMode::Themes(_) => "Type a theme name...",
            PaletteMode::Shaders(_) => "Type a shader name...",
            PaletteMode::Buffers(_) => "Search buffers...",
            PaletteMode::Workspaces(_) => "Search workspaces...",
            PaletteMode::Servers(_) => "Search servers...",
            // Ex/Search modes should open with a readable prompt, not
            // a lone ':' or '/' glyph sitting in the input.
            PaletteMode::Ex => "Type a command...",
            PaletteMode::Search if self.search_backward => "Search buffer backward...",
            PaletteMode::Search => "Search buffer...",
        };
        // Ex mode prepends its command glyph only once the user types,
        // so the initial empty state stays clean. Search mode shows
        // only the user's pattern; `/` is just the key used to open it.
        let display_owned;
        let display_text = if self.query.is_empty() {
            placeholder
        } else {
            match self.mode {
                PaletteMode::Ex => {
                    display_owned = format!(":{}", self.query);
                    display_owned.as_str()
                }
                PaletteMode::Search => self.query.as_str(),
                _ => self.query.as_str(),
            }
        };
        let text_color = if self.query.is_empty() {
            theme.u8(theme.muted)
        } else {
            theme.u8(theme.fg)
        };

        let text_x = input_x + input_pad_x;
        let text_y = input_y + (input_h - input_font) / 2.0;
        let input_opts = DrawOpts {
            font_size: input_font,
            color: text_color,
            clip_rect: Some(input_clip),
            ..DrawOpts::default()
        };
        let input_rendered_width =
            sugarloaf
                .overlay_text_mut()
                .draw(text_x, text_y, display_text, &input_opts);

        // `/`-search: vim-style `[cur/total]` match count, right-aligned
        // + muted in the input row. `cur` = highlighted match position
        // (1-based), `total` = live buffer-match count; both update as
        // matches stream in. Zero matches renders `[0/0]`. Only shown
        // once the user has typed a pattern (empty query lists recents).
        if matches!(self.mode, PaletteMode::Search) && !self.query.is_empty() {
            let total = self.buffer_matches.len();
            let cur = if total == 0 {
                0
            } else {
                self.selected_index.min(total - 1) + 1
            };
            let count_label = format!("[{cur}/{total}]");
            let count_opts = DrawOpts {
                font_size: input_font,
                color: theme.u8(theme.muted),
                clip_rect: Some(input_clip),
                ..DrawOpts::default()
            };
            let count_w = sugarloaf
                .overlay_text_mut()
                .measure(&count_label, &count_opts);
            // Right-align inside the input, but never overlap the typed
            // query (clamp the start x to the query's right edge).
            let count_x = (input_x + input_width - input_pad_x - count_w)
                .max(text_x + input_rendered_width + input_pad_x);
            sugarloaf
                .overlay_text_mut()
                .draw(count_x, text_y, &count_label, &count_opts);
        }

        let elapsed_ms = Instant::now()
            .saturating_duration_since(self.caret_blink_start)
            .as_millis();
        let caret_visible = (elapsed_ms / CARET_BLINK_MS).is_multiple_of(2);

        if caret_visible {
            let text_width = if self.query.is_empty() {
                0.0
            } else {
                input_rendered_width
            };

            let max_caret_x = input_x + input_width - input_pad_x - caret_w;
            let caret_x = (text_x + text_width).min(max_caret_x.max(text_x));
            let caret_height = input_font + 4.0 * s;
            let caret_y = input_y + (input_h - caret_height) / 2.0 + 2.0 * s;

            sugarloaf.overlay_rect(
                caret_x,
                caret_y,
                caret_w,
                caret_height,
                theme.f32(theme.fg),
                DEPTH_ELEMENT,
                ORDER,
            );
        }

        let filtered = self.filtered_rows();
        let sep_y = input_y + input_h;
        sugarloaf.overlay_rect(
            input_x,
            sep_y,
            input_width,
            SEPARATOR_HEIGHT,
            theme.f32(theme.border),
            DEPTH_ELEMENT,
            ORDER,
        );

        let results_y = sep_y + SEPARATOR_HEIGHT + margin_top;
        let visible_rows = filtered
            .len()
            .saturating_sub(self.scroll_offset)
            .min(MAX_VISIBLE_RESULTS);
        let list_h = visible_rows as f32 * row_h;
        let list_clip_h = list_h + RESULTS_PADDING_BOTTOM * s;
        let list_bottom = results_y + list_clip_h;
        let list_clip = [input_x, results_y, input_width, list_clip_h];
        let mut next_selected_cursor_rect = None;
        let mut next_server_edit_hit = None;
        let mut next_server_remove_hit = None;

        // 5D-drag: the host header currently under the cursor during an
        // active workspace drag is the drop target — highlight it so the
        // user sees where the workspace will land. Owned compare below
        // against each `WorkspaceHost` row's `host_id`.
        let drag_drop_host_id = self.workspace_drag_drop_host_id().map(str::to_string);

        let shortcut_opts = DrawOpts {
            font_size: shortcut_font,
            color: theme.u8(theme.muted),
            clip_rect: Some(list_clip),
            ..DrawOpts::default()
        };
        // Command rows swap the muted raw-shortcut hint for a green
        // bracketed keybind chip, matching the Alt+K command sheet /
        // splash menu (`[165, 230, 170]`, bold). Every other row kind
        // keeps `shortcut_opts` untouched.
        let command_chip_opts = DrawOpts {
            font_size: shortcut_font,
            color: [165, 230, 170, 255],
            bold: true,
            clip_rect: Some(list_clip),
            ..DrawOpts::default()
        };

        let overscan =
            ((list_scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let start = self.scroll_offset.saturating_sub(overscan);
        let end = (self.scroll_offset + visible_rows + overscan).min(filtered.len());
        for (i, (_, row)) in filtered[start..end].iter().enumerate() {
            let actual_index = start + i;
            let display_i = actual_index as isize - self.scroll_offset as isize;
            let item_y = results_y + row_h * display_i as f32 + list_scroll_offset;
            if item_y + row_h <= results_y || item_y >= list_bottom {
                continue;
            }
            let is_selected = actual_index == self.selected_index;
            let is_hovered = self.hovered_index == Some(actual_index);
            let server_active =
                matches!(row, PaletteRow::Server { entry } if entry.active);

            // 5D-drag drop-target highlight: paint the hovered host
            // header with the selection-hover fill plus a folder-blue
            // outline so it reads as "drop here". Mirrors how file_tree /
            // cross_window_drag surface a drop target. Kept simple — the
            // user tunes the exact look in their dev loop.
            let is_drop_target = drag_drop_host_id.as_deref().is_some_and(|drop| {
                matches!(row, PaletteRow::WorkspaceHost { host_id, .. } if *host_id == drop)
            });
            if is_drop_target {
                let visible_y = item_y.max(results_y);
                let visible_h = (item_y + row_h).min(list_bottom) - visible_y;
                if visible_h > 0.0 {
                    stroke_rounded_rect(
                        sugarloaf,
                        input_x,
                        visible_y,
                        input_width,
                        visible_h,
                        (1.5 * s).max(1.0),
                        4.0 * s,
                        theme.f32(theme.folder),
                        theme.f32(theme.hover),
                        DEPTH_ELEMENT,
                        ORDER,
                    );
                }
            }

            // Selection highlight
            if is_selected || is_hovered {
                let selected_y = item_y + cursor_offset;
                let visible_y = selected_y.max(results_y);
                let visible_h = (selected_y + row_h).min(list_bottom) - visible_y;
                if visible_h > 0.0 {
                    sugarloaf.overlay_rounded_rect(
                        input_x,
                        visible_y,
                        input_width,
                        visible_h,
                        if is_selected {
                            theme.f32_alpha(theme.accent, 0.34)
                        } else {
                            theme.f32_alpha(theme.hover, 0.72)
                        },
                        DEPTH_ELEMENT,
                        4.0 * s,
                        ORDER,
                    );
                    let cursor_w = (result_font * 0.6).max(2.0);
                    let cursor_x =
                        (input_x + input_pad_x - cursor_w - 2.0 * s).max(input_x);
                    // Match the file-tree jump cursor, not the taller
                    // command-row highlight. The palette rows have more
                    // vertical padding, but the trail cursor should stay
                    // text/cell-sized so it reads like the real cursor.
                    let cursor_h = (file_tree::ROW_HEIGHT * s - 6.0 * s)
                        .max(result_font)
                        .min(row_h);
                    let cursor_y = (selected_y + (row_h - cursor_h) / 2.0)
                        .clamp(results_y, (list_bottom - cursor_h).max(results_y));
                    next_selected_cursor_rect =
                        Some([cursor_x, cursor_y, cursor_w, cursor_h]);
                    if is_selected && matches!(row, PaletteRow::Server { .. }) {
                        sugarloaf.overlay_rect(
                            input_x,
                            visible_y + 4.0 * s,
                            3.0 * s,
                            (visible_h - 8.0 * s).max(2.0 * s),
                            theme.f32(theme.accent),
                            DEPTH_ELEMENT + 0.02,
                            ORDER + 1,
                        );
                    }
                }
            }

            // Current-place marker: the connected server / the workspace
            // this window is viewing gets a LEFT accent stripe (like the
            // file tree's active-buffer stripe) — never a full-row wash,
            // and independent of hover/selection.
            let row_is_current = server_active
                || matches!(row, PaletteRow::Workspace { entry } if entry.current);
            if row_is_current {
                let visible_y = item_y.max(results_y);
                let visible_h = (item_y + row_h).min(list_bottom) - visible_y;
                if visible_h > 6.0 * s {
                    sugarloaf.overlay_rect(
                        input_x,
                        visible_y + 3.0 * s,
                        3.0 * s,
                        visible_h - 6.0 * s,
                        theme.f32(theme.green),
                        DEPTH_ELEMENT + 0.03,
                        ORDER + 2,
                    );
                }
            }

            let result_opts = DrawOpts {
                font_size: result_font,
                color: if is_selected || is_hovered {
                    theme.u8(theme.fg)
                } else {
                    theme.u8(theme.dim)
                },
                clip_rect: Some(list_clip),
                ..DrawOpts::default()
            };
            // Host headers read as group labels (kind glyph + label);
            // their workspace children are indented one tree level under
            // them, mirroring file_tree's folder→file nesting.
            let is_host_header = matches!(row, PaletteRow::WorkspaceHost { .. });
            // Command rows get the Alt+K command-sheet treatment: a
            // per-service icon, a bold label, and a green keybind chip.
            let is_command = matches!(row, PaletteRow::Command { .. });
            let row_indent = match row {
                PaletteRow::Workspace { .. } => file_tree::INDENT_PX * s,
                _ => 0.0,
            };
            let (row_icon, row_icon_color) = match row {
                // Per-service nerd-font glyph, muted ink brightened on the
                // active row. Prefix→service reverse-lookup keeps
                // `CommandService::icon()` the single icon source (and
                // `icon_themed` lets a Mash Up Pack re-glyph it).
                PaletteRow::Command { service, .. } => {
                    let icon = CommandService::from_prefix(service)
                        .map(CommandService::icon_themed);
                    let color = if is_selected {
                        theme.u8(theme.fg)
                    } else {
                        theme.u8(theme.dim)
                    };
                    (icon, color)
                }
                PaletteRow::Buffer { entry } => {
                    if entry.detail.starts_with(WORKSPACE_ROOT_DETAIL_PREFIX) {
                        let (icon, color) = file_tree::workspace_root_icon();
                        (Some(icon), color)
                    } else {
                        (None, theme.u8(theme.dim))
                    }
                }
                // Host header: kind glyph, drawn in the folder blue so the
                // group reads as chrome rather than content.
                PaletteRow::WorkspaceHost { kind, .. } => {
                    (Some(kind.icon()), theme.u8(theme.folder))
                }
                // Workspace child: the folder glyph in the lighter folder
                // blue, same as an Island/workspace tab.
                PaletteRow::Workspace { entry } => {
                    let (icon, color) = file_tree::workspace_root_icon();
                    (
                        Some(
                            entry
                                .workspace_host_kind
                                .icon_override(entry.workspace_visibility)
                                .unwrap_or(icon),
                        ),
                        color,
                    )
                }
                _ => (None, theme.u8(theme.dim)),
            };
            let row_icon_width = row_icon
                .map(|icon| {
                    sugarloaf.overlay_text_mut().measure(
                        icon,
                        &DrawOpts {
                            font_size: result_font,
                            color: row_icon_color,
                            clip_rect: Some(list_clip),
                            ..DrawOpts::default()
                        },
                    )
                })
                .unwrap_or(0.0);
            let row_icon_gap = if row_icon_width > 0.0 { 8.0 * s } else { 0.0 };
            let icon_x = input_x + input_pad_x + row_indent;
            // Host headers paint an online dot (`●`/`○`) between the kind
            // glyph and the label; reserve its width so the label clears
            // it. Non-header rows reserve nothing.
            let (online_dot, online_dot_color) = match row {
                PaletteRow::WorkspaceHost { online, .. } => {
                    let dot = if *online { "\u{25cf}" } else { "\u{25cb}" }; // ● / ○
                    let color = if *online {
                        theme.u8(theme.green)
                    } else {
                        theme.u8(theme.muted)
                    };
                    (Some(dot), color)
                }
                PaletteRow::Server { entry } => {
                    let (dot, color) = match entry.status {
                        crate::panels::ServerIndicatorStatus::Online => {
                            ("\u{25cf}", theme.u8(theme.green))
                        }
                        crate::panels::ServerIndicatorStatus::Connecting => {
                            ("\u{25cf}", theme.u8(theme.yellow))
                        }
                        crate::panels::ServerIndicatorStatus::Offline => {
                            ("\u{25cf}", theme.u8(theme.red))
                        }
                        crate::panels::ServerIndicatorStatus::Unknown => {
                            ("\u{25cb}", theme.u8(theme.muted))
                        }
                    };
                    (Some(dot), color)
                }
                _ => (None, theme.u8(theme.muted)),
            };
            let online_dot_width = online_dot
                .map(|dot| {
                    sugarloaf.overlay_text_mut().measure(
                        dot,
                        &DrawOpts {
                            font_size: result_font,
                            color: online_dot_color,
                            clip_rect: Some(list_clip),
                            ..DrawOpts::default()
                        },
                    )
                })
                .unwrap_or(0.0);
            let online_dot_gap = if online_dot_width > 0.0 { 6.0 * s } else { 0.0 };
            let row_text_x = icon_x
                + row_icon_width
                + row_icon_gap
                + online_dot_width
                + online_dot_gap;
            let row_text_y = item_y + (row_h - result_font) / 2.0;

            // Right-side hint: shortcut for commands, copy icon for
            // font rows (signals "Enter copies this to clipboard").
            // Command rows render only the real keybind (the first
            // whitespace-delimited token of the raw shortcut, dropping
            // trailing alias words) wrapped in a green `[…]` chip; an
            // empty keybind yields no chip. Other rows keep the raw
            // shortcut string in the muted style.
            let is_font_row = matches!(row, PaletteRow::Font { .. });
            let command_chip = if is_command {
                let key = row.shortcut().split_whitespace().next().unwrap_or("");
                (!key.is_empty()).then(|| format!("[{key}]"))
            } else {
                None
            };
            let is_server_row = matches!(row, PaletteRow::Server { .. });
            let shortcut_text: &str = if is_server_row {
                ""
            } else if is_command {
                command_chip.as_deref().unwrap_or("")
            } else {
                row.shortcut()
            };
            let shortcut_draw_opts = if is_command {
                &command_chip_opts
            } else {
                &shortcut_opts
            };
            let shortcut_width = if !shortcut_text.is_empty() {
                sugarloaf
                    .overlay_text_mut()
                    .measure(shortcut_text, shortcut_draw_opts)
            } else {
                0.0
            };
            // Workspace-move feedback on the target host header:
            // "moving…" while the daemon copies the workspace over,
            // then ✓/✗ + message for a few seconds.
            let move_hint: Option<(String, [u8; 4])> = match row {
                PaletteRow::WorkspaceHost { host_id, .. } => self
                    .workspace_move
                    .as_ref()
                    .filter(|status| status.target_host_id == *host_id)
                    .map(|status| match &status.phase {
                        super::state::WorkspaceMovePhase::InFlight => {
                            let dots =
                                ((status.since.elapsed().as_millis() / 300) % 3) + 1;
                            (
                                format!("moving{}", ".".repeat(dots as usize)),
                                theme.u8(theme.yellow),
                            )
                        }
                        super::state::WorkspaceMovePhase::Done { ok: true, .. } => {
                            ("\u{2713} moved".to_string(), theme.u8(theme.green))
                        }
                        super::state::WorkspaceMovePhase::Done { ok: false, message } => {
                            let mut message = message.replace(['\r', '\n'], " ");
                            if message.chars().count() > 60 {
                                message = message.chars().take(59).collect::<String>()
                                    + "\u{2026}";
                            }
                            (format!("\u{2717} {message}"), theme.u8(theme.red))
                        }
                    }),
                _ => None,
            };
            let move_hint_opts = move_hint.as_ref().map(|(_, color)| DrawOpts {
                font_size: shortcut_font,
                color: *color,
                clip_rect: Some(list_clip),
                ..DrawOpts::default()
            });
            let move_hint_width = move_hint
                .as_ref()
                .zip(move_hint_opts.as_ref())
                .map(|((text, _), opts)| sugarloaf.overlay_text_mut().measure(text, opts))
                .unwrap_or(0.0);
            let right_reserve = if move_hint_width > 0.0 {
                move_hint_width + 12.0 * s
            } else if !shortcut_text.is_empty() {
                shortcut_width + 12.0 * s
            } else if is_font_row {
                icon_w + 12.0 * s
            } else {
                0.0
            };
            let server_actions = match row {
                PaletteRow::Server { entry } if !entry.local => Some(entry.id.clone()),
                _ => None,
            };
            let server_action_reserve = if server_actions.is_some() {
                132.0 * s
            } else {
                0.0
            };
            let title_budget = (input_width
                - input_pad_x * 2.0
                - row_indent
                - row_icon_width
                - row_icon_gap
                - online_dot_width
                - online_dot_gap
                - right_reserve
                - server_action_reserve)
                .max(0.0);
            // Host header label reads as a muted group separator (never
            // the bright selected-row fg), since a header can't be
            // selected. Everything else keeps the selected/dim coloring.
            let title_opts = if is_host_header {
                DrawOpts {
                    color: theme.u8(theme.dim),
                    ..result_opts
                }
            } else if is_command {
                // Bold `{service}: {title}` to match the sheet / splash
                // weight; keeps the selected/dim coloring from result_opts.
                DrawOpts {
                    bold: true,
                    ..result_opts
                }
            } else {
                result_opts
            };
            // Zed-style namespaced display: command rows read
            // `{service}: {command}` (e.g. `code: Write File`).
            let display_title = row.display_title();
            let title =
                truncate_to_fit(&display_title, title_budget, sugarloaf, &title_opts);
            if matches!(
                row,
                PaletteRow::WorkspaceCreate
                    | PaletteRow::ServerAdd
                    | PaletteRow::ServerCreate
            ) {
                let plus_w = sugarloaf.overlay_text_mut().measure(&title, &title_opts);
                sugarloaf.overlay_text_mut().draw(
                    input_x + (input_width - plus_w) / 2.0,
                    row_text_y,
                    &title,
                    &title_opts,
                );
                continue;
            }
            if let Some(icon) = row_icon {
                let icon_opts = DrawOpts {
                    font_size: result_font,
                    color: row_icon_color,
                    // Command-service glyphs render bold to match the
                    // sheet's icon weight; other row icons stay regular.
                    bold: is_command,
                    clip_rect: Some(list_clip),
                    ..DrawOpts::default()
                };
                sugarloaf
                    .overlay_text_mut()
                    .draw(icon_x, row_text_y, icon, &icon_opts);
            }
            if let Some(dot) = online_dot {
                let dot_opts = DrawOpts {
                    font_size: result_font,
                    color: online_dot_color,
                    clip_rect: Some(list_clip),
                    ..DrawOpts::default()
                };
                sugarloaf.overlay_text_mut().draw(
                    icon_x + row_icon_width + row_icon_gap,
                    row_text_y,
                    dot,
                    &dot_opts,
                );
            }
            sugarloaf.overlay_text_mut().draw(
                row_text_x,
                row_text_y,
                &title,
                &title_opts,
            );
            if let PaletteRow::Server { entry } = row {
                let endpoint_x = row_text_x
                    + sugarloaf.overlay_text_mut().measure(&title, &title_opts)
                    + 16.0 * s;
                let endpoint_right = if server_actions.is_some() {
                    input_x + input_width - input_pad_x - server_action_reserve
                } else {
                    input_x + input_width - input_pad_x - 24.0 * s
                };
                let endpoint_budget = (endpoint_right - endpoint_x).max(0.0);
                if endpoint_budget > 18.0 * s {
                    let endpoint_opts = DrawOpts {
                        font_size: shortcut_font,
                        color: theme.u8(theme.dim),
                        clip_rect: Some([
                            endpoint_x,
                            list_clip[1],
                            endpoint_budget,
                            list_clip[3],
                        ]),
                        ..DrawOpts::default()
                    };
                    let endpoint = truncate_to_fit(
                        &entry.address,
                        endpoint_budget,
                        sugarloaf,
                        &endpoint_opts,
                    );
                    sugarloaf.overlay_text_mut().draw(
                        endpoint_x,
                        item_y + (row_h - shortcut_font) / 2.0,
                        &endpoint,
                        &endpoint_opts,
                    );
                }
            }
            if let Some(server_id) = server_actions {
                let chip_h = 24.0 * s;
                let remove_w = 66.0 * s;
                let edit_w = 52.0 * s;
                let gap = 6.0 * s;
                let remove_x = input_x + input_width - input_pad_x - remove_w;
                let edit_x = remove_x - gap - edit_w;
                let chip_y = item_y + (row_h - chip_h) * 0.5;
                for (x, w, label) in
                    [(edit_x, edit_w, "Edit"), (remove_x, remove_w, "Remove")]
                {
                    sugarloaf.rounded_rect(
                        None,
                        x,
                        chip_y,
                        w,
                        chip_h,
                        theme.f32_alpha(theme.surface, 0.94),
                        DEPTH_ELEMENT + 0.03,
                        5.0 * s,
                        ORDER + 2,
                    );
                    let opts = DrawOpts {
                        font_size: 11.0 * s,
                        color: theme.u8(theme.fg),
                        clip_rect: Some(list_clip),
                        ..DrawOpts::default()
                    };
                    let text_w = sugarloaf.overlay_text_mut().measure(label, &opts);
                    sugarloaf.overlay_text_mut().draw(
                        x + (w - text_w) * 0.5,
                        chip_y + (chip_h - 11.0 * s) * 0.5,
                        label,
                        &opts,
                    );
                }
                next_server_edit_hit =
                    Some(([edit_x, chip_y, edit_w, chip_h], server_id.clone()));
                next_server_remove_hit =
                    Some(([remove_x, chip_y, remove_w, chip_h], server_id));
            }
            if let Some(((text, _), opts)) =
                move_hint.as_ref().zip(move_hint_opts.as_ref())
            {
                let hint_x = input_x + input_width - input_pad_x - move_hint_width;
                let hint_y = item_y + (row_h - shortcut_font) / 2.0;
                sugarloaf
                    .overlay_text_mut()
                    .draw(hint_x, hint_y, text, opts);
            } else if !shortcut_text.is_empty() {
                let shortcut_x = input_x + input_width - input_pad_x - shortcut_width;
                let shortcut_y = item_y + (row_h - shortcut_font) / 2.0;
                sugarloaf.overlay_text_mut().draw(
                    shortcut_x,
                    shortcut_y,
                    shortcut_text,
                    shortcut_draw_opts,
                );
            }

            if is_font_row {
                let stroke_color = if is_selected {
                    theme.f32(theme.fg)
                } else {
                    theme.f32(theme.muted)
                };
                // Cutout inside each page uses the row's own background
                // so the border reads as a clean outline on either
                // palette-bg (idle) or selection-highlight-bg (hovered).
                let row_fill_color = if is_selected {
                    theme.f32(theme.hover)
                } else {
                    theme.f32_alpha(theme.surface, 0.98)
                };
                let icon_x = input_x + input_width - input_pad_x - icon_w;
                let icon_y = item_y + (row_h - icon_h) / 2.0;
                if icon_y >= results_y && icon_y + icon_h <= list_bottom {
                    draw_copy_icon(
                        sugarloaf,
                        icon_x,
                        icon_y,
                        stroke_color,
                        row_fill_color,
                        DEPTH_ELEMENT,
                        ORDER,
                    );
                }
            }
        }

        // Scrollbar: shares the terminal scrollbar's visual language
        // (6 px wide, gray semi-transparent, 2 s visibility + 300 ms
        // fade after the last scroll event) via `renderer::scrollbar`.
        // Drawn only when the palette has actually been scrolled —
        // hidden on first open, faded out 2.3 s after the last scroll.
        let total = filtered.len();
        drop(filtered);
        self.server_edit_hit = next_server_edit_hit;
        self.server_remove_hit = next_server_remove_hit;
        let track_height = MAX_VISIBLE_RESULTS as f32 * row_h;
        let normalized = if total > MAX_VISIBLE_RESULTS {
            self.scroll_offset as f32 / (total - MAX_VISIBLE_RESULTS) as f32
        } else {
            0.0
        };
        if let Some((thumb_y, thumb_height)) = scrollbar::compute_thumb(
            MAX_VISIBLE_RESULTS,
            total,
            results_y,
            track_height,
            normalized,
        ) {
            let opacity = scrollbar::opacity_from_last_scroll(
                self.last_scroll_time,
                false, // palette has no drag interaction
            );
            let bar_x =
                input_x + input_width - scrollbar::width() - scrollbar::SCROLLBAR_MARGIN;
            // Palette backdrop + bg rects use ORDER=20; the terminal
            // scrollbar's default ORDER=5 would land *under* them and
            // be invisible. Piggy-back on the palette's own order, at
            // a depth slightly above the selection highlight so a
            // hovered row doesn't mask the thumb.
            scrollbar::draw_track_overlay(
                sugarloaf,
                bar_x,
                results_y,
                track_height,
                opacity,
                DEPTH_ELEMENT + 0.05,
                ORDER,
            );
            scrollbar::draw_thumb_overlay(
                sugarloaf,
                bar_x,
                thumb_y,
                thumb_height,
                opacity,
                false,
                DEPTH_ELEMENT + 0.05,
                ORDER,
            );
        }
        self.selected_cursor_rect = next_selected_cursor_rect;
    }
}
