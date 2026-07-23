use std::borrow::Cow;
use std::collections::HashMap;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::IdeTheme;
pub(super) use crate::primitives::{
    draw_text_with_occlusion, edge_left_row_radii, edge_row_radii, snap_to_device_px,
};

use super::icons::icon_for;
use super::state::{CachedTruncatedLabel, FileTree, TruncatedLabelMetricsKey};
use super::types::{GitStatus, NodeKind};
use super::{
    DEPTH, FONT_SIZE, FRAME_RADIUS, FRAME_STROKE, ICON_FONT_SIZE, ICON_GAP, INDENT_PX,
    LABEL_TRUNCATION_CACHE_MAX, ORDER, REVEAL_FLASH_MS, ROOT_TRANSITION_MS,
    ROOT_TRANSITION_STAGGER_MS, ROW_PADDING_X,
};
use crate::animation::ease_out_cubic;

fn fade_u8(mut color: [u8; 4], alpha: f32) -> [u8; 4] {
    color[3] = (color[3] as f32 * alpha) as u8;
    color
}

fn fade_f32(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] *= alpha;
    color
}

// TODO(wave6-cutover): the native build draws the panel chassis via
// `chrome::widgets::frame::draw_frame(FrameConfig { rounded_corners:
// FrameCorners::Top, .. })`. That widget has not been lifted to
// neoism-ui yet, so the slim port inlines the same outer-fill +
// inner-fill pair below. Behaviour is bit-identical — same radii,
// stroke, ORDER stacking — just without the FrameCorners enum surface.
fn draw_frame_top(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    outer_color: [f32; 4],
    inner_color: [f32; 4],
    radius: f32,
    border_thickness: f32,
    depth: f32,
    order_outer: u8,
    order_inner: u8,
) {
    let [x, y, w, h] = rect;
    // Only the top corners are rounded — matches `FrameCorners::Top`
    // in the native widget.
    let outer_radii = [radius, radius, 0.0, 0.0];
    sugarloaf.quad(
        None,
        x,
        y,
        w,
        h,
        outer_color,
        outer_radii,
        depth,
        order_outer,
    );
    let inner_x = x + border_thickness;
    let inner_y = y + border_thickness;
    let inner_w = (w - border_thickness * 2.0).max(0.0);
    let inner_h = (h - border_thickness).max(0.0);
    let inner_r = (radius - border_thickness).max(0.0);
    let inner_radii = [inner_r, inner_r, 0.0, 0.0];
    sugarloaf.quad(
        None,
        inner_x,
        inner_y,
        inner_w,
        inner_h,
        inner_color,
        inner_radii,
        depth,
        order_inner,
    );
}

impl FileTree {
    /// Draw the panel inside the caller-assigned rect.
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        x_left: f32,
        y_top: f32,
        panel_width: f32,
        panel_height: f32,
        theme: &IdeTheme,
        text_occlusion_rects: &[[f32; 4]],
    ) {
        if !self.visible || panel_width <= 0.0 || panel_height <= 0.0 {
            return;
        }
        let perf_started = std::env::var_os("NEOISM_FILE_TREE_PERF")
            .is_some()
            .then(web_time::Instant::now);

        let row_h = self.row_height();
        let font_size = FONT_SIZE * self.scale;
        let icon_size = ICON_FONT_SIZE * self.scale;
        let row_pad_x = ROW_PADDING_X * self.scale;
        let indent_px = INDENT_PX * self.scale;
        let icon_gap = ICON_GAP * self.scale;
        let frame_stroke = (FRAME_STROKE * self.scale).max(2.0);
        let frame_radius = FRAME_RADIUS * self.scale;
        let content_x = x_left + frame_stroke;
        let content_y = y_top + frame_stroke;
        let content_w = (panel_width - frame_stroke * 2.0).max(0.0);
        let content_h = (panel_height - frame_stroke).max(0.0);
        // Re-clamp before painting — terminal resize can shrink the
        // panel between input and frame, and we want the selection
        // visible.
        let rows_visible = self.rows_per_panel(content_h).max(1);
        self.last_panel_height_rows = rows_visible;
        self.clamp_scroll_bounds(rows_visible);
        let scroll_offset =
            snap_to_device_px(self.tick_scroll(), sugarloaf.scale_factor());
        let cursor_offset = self.tick_cursor();
        let reveal_flash = self.reveal_flash.as_ref().and_then(|flash| {
            let elapsed_ms = web_time::Instant::now()
                .saturating_duration_since(flash.started)
                .as_secs_f32()
                * 1000.0;
            (elapsed_ms < REVEAL_FLASH_MS).then(|| {
                let t = elapsed_ms / REVEAL_FLASH_MS;
                let alpha = 0.10 + 0.28 * (1.0 - t).max(0.0);
                (flash.index, alpha)
            })
        });
        if reveal_flash.is_none()
            && self.reveal_flash.as_ref().is_some_and(|flash| {
                web_time::Instant::now()
                    .saturating_duration_since(flash.started)
                    .as_secs_f32()
                    * 1000.0
                    >= REVEAL_FLASH_MS
            })
        {
            self.reveal_flash = None;
        }
        let panel_bottom = content_y + content_h;
        let panel_clip = [content_x, content_y, content_w, content_h];
        let content_radius = (frame_radius - frame_stroke).max(0.0);
        self.selected_cursor_rect = None;

        draw_frame_top(
            sugarloaf,
            [x_left, y_top, panel_width, panel_height],
            theme.f32(theme.surface),
            theme.f32(theme.bg),
            frame_radius,
            frame_stroke,
            DEPTH,
            ORDER,
            ORDER + 1,
        );

        // Loading skeleton: a root listing is in flight and there is
        // nothing to draw yet — the window a remote/tailnet join sits
        // in while the host daemon streams the first DirListing.
        // Shimmering tree-shaped placeholder rows instead of a blank
        // frame; `is_animating()` keeps frames coming on both hosts,
        // and the rows vanish the moment entries land (they are not
        // hit-testable — purely paint).
        if self.is_loading() {
            let started = *self
                .skeleton_started
                .get_or_insert_with(web_time::Instant::now);
            let elapsed = web_time::Instant::now()
                .saturating_duration_since(started)
                .as_secs_f32();
            // Quick fade-in so a fast local reply never flashes a
            // skeleton frame.
            let fade_in = (elapsed / 0.18).min(1.0);
            const SKELETON_ROWS: usize = 12;
            const SKELETON_DEPTHS: [f32; SKELETON_ROWS] =
                [0.0, 0.0, 1.0, 1.0, 2.0, 1.0, 0.0, 1.0, 2.0, 2.0, 0.0, 1.0];
            const SKELETON_WIDTHS: [f32; SKELETON_ROWS] = [
                0.58, 0.72, 0.46, 0.64, 0.38, 0.55, 0.68, 0.44, 0.52, 0.36, 0.62, 0.48,
            ];
            let bar_h = (font_size * 0.72).max(4.0);
            let stub_h = icon_size.min(row_h).max(4.0);
            for i in 0..rows_visible.min(SKELETON_ROWS) {
                let row_y = content_y + i as f32 * row_h;
                if row_y + row_h > panel_bottom {
                    break;
                }
                // Slow sine wave marching down the rows — same clock
                // family as the splash shimmer, phase-staggered so the
                // pulse travels instead of blinking in unison.
                let wave =
                    (elapsed / 1.3 * std::f32::consts::TAU - i as f32 * 0.55).sin();
                let alpha = (0.16 + 0.08 * wave).max(0.04) * fade_in;
                let indent = SKELETON_DEPTHS[i] * indent_px;
                let stub_x = content_x + row_pad_x + indent + indent_px;
                let stub_y = row_y + (row_h - stub_h) / 2.0;
                sugarloaf.quad(
                    None,
                    stub_x,
                    stub_y,
                    stub_h,
                    stub_h,
                    theme.f32_alpha(theme.muted, alpha),
                    [3.0 * self.scale; 4],
                    DEPTH,
                    ORDER + 2,
                );
                let bar_x = stub_x + stub_h + icon_gap;
                let bar_y = row_y + (row_h - bar_h) / 2.0;
                let bar_budget = (content_x + content_w - row_pad_x - bar_x).max(0.0);
                let bar_w = bar_budget * SKELETON_WIDTHS[i];
                if bar_w > 1.0 {
                    sugarloaf.quad(
                        None,
                        bar_x,
                        bar_y,
                        bar_w,
                        bar_h,
                        theme.f32_alpha(theme.muted, alpha),
                        [bar_h / 2.0; 4],
                        DEPTH,
                        ORDER + 2,
                    );
                }
            }
        } else {
            self.skeleton_started = None;
        }

        if !self.entries.is_empty() && self.selected < self.entries.len() {
            let row_ix = self.selected as isize - self.scroll_top as isize;
            let row_y = content_y + row_ix as f32 * row_h + scroll_offset + cursor_offset;
            let row_bottom = row_y + row_h;
            let visible_row_y = row_y.max(content_y);
            let visible_row_h = row_bottom.min(panel_bottom) - visible_row_y;
            if visible_row_h > 0.0 {
                sugarloaf.quad(
                    None,
                    content_x,
                    visible_row_y,
                    content_w,
                    visible_row_h,
                    theme.f32(theme.surface),
                    edge_row_radii(
                        visible_row_y,
                        visible_row_h,
                        content_y,
                        panel_bottom,
                        content_radius,
                    ),
                    DEPTH,
                    ORDER + 2,
                );
                if self.focused {
                    let cursor_w = (font_size * 0.6).max(2.0);
                    let cursor_x = content_x + (row_pad_x - cursor_w).max(0.0);
                    let cursor_h = (row_h - 6.0 * self.scale)
                        .max(font_size)
                        .min(row_h)
                        .min(content_h.max(2.0));
                    let cursor_y = (row_y + (row_h - cursor_h) / 2.0)
                        .clamp(content_y, (panel_bottom - cursor_h).max(content_y));
                    self.selected_cursor_rect =
                        Some([cursor_x, cursor_y, cursor_w, cursor_h]);
                }
            }
        }

        let overscan = ((scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
        let start = self.scroll_top.saturating_sub(overscan);
        let end = (self.scroll_top + rows_visible + overscan).min(self.entries.len());
        // Root-swap reveal: same duration/easing as the status-line mode
        // transition, cascading a beat per row from the top. Expire the
        // sweep once the LAST visible row has finished so `is_animating`
        // stops holding the redraw loop open.
        let root_reveal_elapsed_ms = self
            .root_transition_started
            .map(|started| started.elapsed().as_secs_f32() * 1000.0);
        if let Some(elapsed) = root_reveal_elapsed_ms {
            let sweep_ms =
                ROOT_TRANSITION_MS + rows_visible as f32 * ROOT_TRANSITION_STAGGER_MS;
            if elapsed >= sweep_ms {
                self.root_transition_started = None;
            }
        }
        // The dragged row is LIFTED out of the list: its real icon +
        // label (the actual entry, not a synthetic chip) follow the
        // cursor on a softly raised sheet, while its slot in the list
        // dims to a placeholder. Laid out here (up front) so the sheet's
        // rect can join the text-occlusion set — row labels paint in a
        // layer above plain quads and would otherwise bleed through it.
        // The sheet is painted after the rows below.
        let drag_source_index = self.drag_source_index();
        struct LiftedRow {
            rect: [f32; 4],
            chevron: Option<&'static str>,
            icon_glyph: &'static str,
            icon_color: [u8; 4],
            label: String,
            label_color: [u8; 4],
            radius: f32,
        }
        let lifted = if let Some(drag) = self.file_drag.as_ref().filter(|drag| drag.live) {
            // Pull the real source entry so the lifted row is a faithful
            // copy of the one in the list (same glyph, same folder blue).
            let entry = drag_source_index.and_then(|ix| self.entries.get(ix));
            let (icon_glyph, file_icon_color) = entry
                .map(icon_for)
                .unwrap_or_else(|| super::icons::icon_for_file(&drag.source_label));
            let is_dir = entry.is_some_and(|e| matches!(e.kind, NodeKind::Dir { .. }));
            let is_open = entry.is_some_and(|e| matches!(e.kind, NodeKind::Dir { open: true }));
            let chevron = is_dir.then(|| if is_open { "\u{f078}" } else { "\u{f054}" });
            let icon_color = if is_dir {
                theme.u8(theme.folder)
            } else {
                file_icon_color
            };
            let pad = 10.0 * self.scale;
            let text_opts = DrawOpts {
                font_size,
                ..DrawOpts::default()
            };
            let glyph_opts = DrawOpts {
                font_size: icon_size,
                ..DrawOpts::default()
            };
            let label =
                truncate_label(&drag.source_label, 240.0 * self.scale, sugarloaf, &text_opts);
            let label_w = sugarloaf.text_mut().measure(&label, &text_opts);
            let icon_w = sugarloaf.text_mut().measure(icon_glyph, &glyph_opts);
            let chevron_w = chevron
                .map(|chev| sugarloaf.text_mut().measure(chev, &glyph_opts) + icon_gap)
                .unwrap_or(0.0);
            let sheet_w = pad + chevron_w + icon_w + icon_gap + label_w + pad;
            let sheet_h = row_h;
            // Sit the row just off the cursor's lower-right, like Finder.
            let sheet_x = drag.current_x + 12.0 * self.scale;
            let sheet_y = drag.current_y - sheet_h * 0.5;
            Some(LiftedRow {
                rect: [sheet_x, sheet_y, sheet_w, sheet_h],
                chevron,
                icon_glyph,
                icon_color,
                label,
                label_color: theme.u8(theme.fg),
                radius: 6.0 * self.scale,
            })
        } else {
            None
        };
        // Fold the lifted sheet rect into the occlusion set the row text
        // honors, so the rows underneath don't bleed through it.
        let mut occlusion_owned: Vec<[f32; 4]>;
        let text_occlusion_rects: &[[f32; 4]] = match lifted.as_ref() {
            Some(l) => {
                occlusion_owned = text_occlusion_rects.to_vec();
                occlusion_owned.push(l.rect);
                &occlusion_owned
            }
            None => text_occlusion_rects,
        };

        // Spring-loaded drag: the folder under the cursor is the drop
        // target — it gets a highlight band and a gentle horizontal
        // wiggle. Resolve it by PATH (via `drag_hovered_index`) so a
        // spring-open re-indexing the rows mid-drag doesn't lose it.
        let drag_hover_index = self.drag_hovered_index();
        let drag_wiggle_dx = self
            .file_drag
            .as_ref()
            .filter(|drag| drag.live)
            .map(|drag| {
                let t = drag
                    .hovered_since
                    .map(|since| since.elapsed().as_secs_f32())
                    .unwrap_or(0.0);
                // Amplitude ramps in over ~120ms so the folder shivers to
                // life instead of snapping; ~4Hz feels alive, not jittery.
                let ramp = (t / 0.12).clamp(0.0, 1.0);
                (t * 26.0).sin() * 2.4 * self.scale * ramp
            })
            .unwrap_or(0.0);
        let mut rendered_rows = 0usize;
        for absolute_ix in start..end {
            let entry = &self.entries[absolute_ix];
            let row_ix = absolute_ix as isize - self.scroll_top as isize;
            let row_y = content_y + row_ix as f32 * row_h + scroll_offset;
            let row_bottom = row_y + row_h;
            let visible_row_y = row_y.max(content_y);
            let visible_row_h = row_bottom.min(panel_bottom) - visible_row_y;
            if visible_row_h <= 0.0 {
                continue;
            }
            rendered_rows += 1;
            let reveal = match root_reveal_elapsed_ms {
                Some(elapsed) => {
                    let delay = (rendered_rows - 1) as f32 * ROOT_TRANSITION_STAGGER_MS;
                    ease_out_cubic(
                        ((elapsed - delay) / ROOT_TRANSITION_MS).clamp(0.0, 1.0),
                    )
                }
                None => 1.0,
            };
            let is_selected = absolute_ix == self.selected;
            let is_active_buffer = self
                .active_path
                .as_deref()
                .and_then(|p| entry.path.as_deref().map(|q| p == q))
                .unwrap_or(false);
            // The row being dragged is lifted out — leave a dimmed
            // placeholder where it sat so the list doesn't collapse.
            let is_drag_source = drag_source_index == Some(absolute_ix);
            let row_reveal = if is_drag_source { reveal * 0.32 } else { reveal };
            // Active buffer (the file nvim is currently showing) gets
            // a thin white accent stripe on the left edge — visible
            // even when the user's keyboard selection is on a
            // different row. Selection still wins the full row bg.
            if is_active_buffer && !is_selected {
                let stripe_w = (3.0 * self.scale).max(2.0);
                sugarloaf.quad(
                    None,
                    content_x,
                    visible_row_y,
                    stripe_w,
                    visible_row_h,
                    fade_f32(theme.f32(theme.accent), reveal),
                    edge_left_row_radii(
                        visible_row_y,
                        visible_row_h,
                        content_y,
                        panel_bottom,
                        content_radius,
                    ),
                    DEPTH,
                    ORDER + 2,
                );
            }
            if let Some((flash_ix, alpha)) = reveal_flash {
                if flash_ix == absolute_ix {
                    sugarloaf.quad(
                        None,
                        content_x,
                        visible_row_y,
                        content_w,
                        visible_row_h,
                        theme.f32_alpha(theme.yellow, alpha * reveal),
                        edge_row_radii(
                            visible_row_y,
                            visible_row_h,
                            content_y,
                            panel_bottom,
                            content_radius,
                        ),
                        DEPTH,
                        ORDER + 3,
                    );
                }
            }

            // Spring-loaded drop target: accent-tinted band + a bright
            // outline so it reads as "release here".
            let is_drop_target = drag_hover_index == Some(absolute_ix);
            if is_drop_target {
                sugarloaf.quad(
                    None,
                    content_x,
                    visible_row_y,
                    content_w,
                    visible_row_h,
                    theme.f32_alpha(theme.accent, 0.22),
                    edge_row_radii(
                        visible_row_y,
                        visible_row_h,
                        content_y,
                        panel_bottom,
                        content_radius,
                    ),
                    DEPTH,
                    ORDER + 4,
                );
            }

            // Chevron is its own glyph in dim grey so the label color
            // can stay folder-white / file-grey without being polluted.
            let chevron = match entry.kind {
                // Use the same FontAwesome/Nerd Font chevrons as the
                // tags panel. The plain Unicode triangles rendered on
                // native but disappeared in the web font stack.
                NodeKind::Dir { open: true } => Some("\u{f078}"),
                NodeKind::Dir { open: false } => Some("\u{f054}"),
                NodeKind::File => None,
            };

            let label_color = match entry.kind {
                NodeKind::Dir { .. } if entry.git_status != GitStatus::None => {
                    entry.git_status.color(theme)
                }
                NodeKind::Dir { .. } => theme.u8(theme.fg),
                NodeKind::File if entry.git_status != GitStatus::None => {
                    entry.git_status.color(theme)
                }
                NodeKind::File if is_selected || is_active_buffer => theme.u8(theme.fg),
                NodeKind::File => theme.u8(theme.dim),
            };
            let (icon_glyph, icon_color) = icon_for(entry);
            let icon_color = match entry.kind {
                NodeKind::Dir { .. } if entry.git_status != GitStatus::None => {
                    entry.git_status.color(theme)
                }
                NodeKind::Dir { .. } => theme.u8(theme.folder),
                NodeKind::File => icon_color,
            };

            let label_opts = DrawOpts {
                font_size,
                color: fade_u8(label_color, row_reveal),
                clip_rect: Some(panel_clip),
                ..DrawOpts::default()
            };
            let git_opts = DrawOpts {
                font_size: font_size * 0.9,
                color: fade_u8(entry.git_status.color(theme), row_reveal),
                bold: true,
                clip_rect: Some(panel_clip),
                ..DrawOpts::default()
            };
            let chevron_opts = DrawOpts {
                font_size,
                color: fade_u8(theme.u8(theme.muted), row_reveal),
                clip_rect: Some(panel_clip),
                ..DrawOpts::default()
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: fade_u8(icon_color, row_reveal),
                clip_rect: Some(panel_clip),
                ..DrawOpts::default()
            };

            // Mid-sweep rows slide in from the left as they fade; the
            // drop-target folder additionally wiggles under the drag.
            let base_x = content_x
                + row_pad_x
                + entry.depth as f32 * indent_px
                + (1.0 - reveal) * 12.0 * self.scale
                + if is_drop_target { drag_wiggle_dx } else { 0.0 };
            let text_y = row_y + (row_h - font_size) / 2.0;
            let icon_y = row_y + (row_h - icon_size) / 2.0;

            // Layout per row:
            //   [chevron-or-pad] [icon] [icon_gap] [label]
            let mut cursor_x = base_x;
            if let Some(chev) = chevron {
                draw_text_with_occlusion(
                    sugarloaf,
                    cursor_x,
                    text_y,
                    chev,
                    &chevron_opts,
                    text_occlusion_rects,
                );
            }
            cursor_x += indent_px;

            draw_text_with_occlusion(
                sugarloaf,
                cursor_x,
                icon_y,
                icon_glyph,
                &icon_opts,
                text_occlusion_rects,
            );
            cursor_x += icon_size + icon_gap;

            // Truncate the label so the row never overflows the panel
            // — long file names like
            // "ProductRecommendationServiceImpl.java" used to spill
            // past the right edge into the editor pane.
            let git_marker = entry.git_status.marker();
            let (git_width, git_gap) = git_marker
                .map(|marker| {
                    (
                        sugarloaf.text_mut().measure(marker, &git_opts),
                        8.0 * self.scale,
                    )
                })
                .unwrap_or((0.0, 0.0));
            let label_budget_px =
                (content_x + content_w - cursor_x - row_pad_x - git_width - git_gap)
                    .max(0.0);
            let label = truncate_label_cached(
                &mut self.label_truncation_cache,
                &mut self.label_truncation_cache_items,
                &entry.label,
                label_budget_px,
                sugarloaf,
                &label_opts,
            );
            draw_text_with_occlusion(
                sugarloaf,
                cursor_x,
                text_y,
                label.as_ref(),
                &label_opts,
                text_occlusion_rects,
            );

            if let Some(marker) = git_marker {
                let git_x = content_x + content_w - row_pad_x - git_width;
                draw_text_with_occlusion(
                    sugarloaf,
                    git_x,
                    text_y,
                    marker,
                    &git_opts,
                    text_occlusion_rects,
                );
            }
        }
        // Paint the lifted row above the list (its rect was already cut
        // out of the row text above). A soft shadow + faintly raised sheet
        // reads as the real row peeled up off the list — no synthetic
        // chrome. No clip: it follows the cursor past the panel edge.
        if let Some(l) = lifted {
            let s = self.scale;
            let [lx, ly, lw, lh] = l.rect;
            let pad = 10.0 * s;
            sugarloaf.quad(
                None,
                lx + 1.5 * s,
                ly + 3.0 * s,
                lw,
                lh,
                theme.f32_alpha(theme.bg, 0.40),
                [l.radius; 4],
                DEPTH,
                ORDER + 6,
            );
            sugarloaf.quad(
                None,
                lx,
                ly,
                lw,
                lh,
                theme.f32_alpha(theme.surface, 0.94),
                [l.radius; 4],
                DEPTH,
                ORDER + 7,
            );
            let chevron_opts = DrawOpts {
                font_size: icon_size,
                color: theme.u8(theme.muted),
                ..DrawOpts::default()
            };
            let icon_opts = DrawOpts {
                font_size: icon_size,
                color: l.icon_color,
                ..DrawOpts::default()
            };
            let label_opts = DrawOpts {
                font_size,
                color: l.label_color,
                ..DrawOpts::default()
            };
            let mut cx = lx + pad;
            let icon_y = ly + (lh - icon_size) / 2.0;
            let text_y = ly + (lh - font_size) / 2.0;
            if let Some(chev) = l.chevron {
                sugarloaf.text_mut().draw(cx, icon_y, chev, &chevron_opts);
                cx += sugarloaf.text_mut().measure(chev, &chevron_opts) + icon_gap;
            }
            sugarloaf.text_mut().draw(cx, icon_y, l.icon_glyph, &icon_opts);
            cx += sugarloaf.text_mut().measure(l.icon_glyph, &icon_opts) + icon_gap;
            sugarloaf.text_mut().draw(cx, text_y, &l.label, &label_opts);
        }

        if let Some(started) = perf_started {
            tracing::info!(
                target: "neoism::file_tree_perf",
                elapsed_us = started.elapsed().as_micros(),
                entries = self.entries.len(),
                rendered_rows,
                rows_visible,
                scroll_top = self.scroll_top,
                scroll_offset,
                scroll_animating = self.scroll.position != 0.0,
                cursor_animating = self.cursor_spring.position != 0.0,
                label_cache_items = self.label_truncation_cache_items,
                "file tree render"
            );
        }
    }
}

/// Truncate `label` so its rendered width fits inside `budget_px`,
/// adding an ellipsis when we cut. Uses Sugarloaf's actual shaping
/// width so long single words and fallback-font glyphs don't spill
/// past the tree's right edge.
pub(crate) fn truncate_label(
    label: &str,
    budget_px: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> String {
    if budget_px <= 0.0 || label.is_empty() {
        return String::new();
    }
    if sugarloaf.text_mut().measure(label, opts) <= budget_px {
        return label.to_string();
    }
    if sugarloaf.text_mut().measure("…", opts) >= budget_px {
        return "…".to_string();
    }

    let chars: Vec<char> = label.chars().collect();
    let mut lo = 0usize;
    let mut hi = chars.len();
    while lo < hi {
        let mid = (lo + hi).div_ceil(2);
        let mut candidate: String = chars[..mid].iter().collect();
        candidate.push('…');
        if sugarloaf.text_mut().measure(&candidate, opts) <= budget_px {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }

    let mut out: String = chars[..lo].iter().collect();
    out.push('…');
    out
}

pub(super) fn truncate_label_cached<'a>(
    cache: &mut HashMap<TruncatedLabelMetricsKey, HashMap<String, CachedTruncatedLabel>>,
    cache_items: &mut usize,
    label: &'a str,
    budget_px: f32,
    sugarloaf: &mut Sugarloaf,
    opts: &DrawOpts,
) -> Cow<'a, str> {
    let metrics_key = TruncatedLabelMetricsKey {
        budget_bits: budget_px.to_bits(),
        font_size_bits: opts.font_size.to_bits(),
        scale_factor_bits: sugarloaf.scale_factor().to_bits(),
    };
    if let Some(cached) = cache.get(&metrics_key).and_then(|labels| labels.get(label)) {
        return match cached {
            CachedTruncatedLabel::Original => Cow::Borrowed(label),
            CachedTruncatedLabel::Truncated(text) => Cow::Owned(text.clone()),
        };
    }

    let truncated = truncate_label(label, budget_px, sugarloaf, opts);
    let cached = if truncated == label {
        CachedTruncatedLabel::Original
    } else {
        CachedTruncatedLabel::Truncated(truncated)
    };

    let out = match &cached {
        CachedTruncatedLabel::Original => Cow::Borrowed(label),
        CachedTruncatedLabel::Truncated(text) => Cow::Owned(text.clone()),
    };

    if *cache_items >= LABEL_TRUNCATION_CACHE_MAX {
        cache.clear();
        *cache_items = 0;
    }
    let labels = cache.entry(metrics_key).or_default();
    if labels.insert(label.to_string(), cached).is_none() {
        *cache_items += 1;
    }

    out
}
