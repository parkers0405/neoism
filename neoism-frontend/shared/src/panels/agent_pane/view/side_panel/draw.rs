use super::sections::{render_directory_section, render_section_header};
use super::*;

#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_status_dot_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    diameter: f32,
    color: [u8; 4],
    halo: Option<([u8; 4], f32)>,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    s: f32,
) {
    let dot = "●";
    let font_size = (diameter * 1.55).max(10.0 * s);
    if let Some((mut halo_color, halo_alpha)) = halo {
        halo_color[3] = ((halo_color[3] as f32) * halo_alpha.clamp(0.0, 1.0)) as u8;
        let halo_size = font_size * 1.65;
        let halo_opts = DrawOpts {
            font_size: halo_size,
            color: halo_color,
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let halo_w = sugarloaf.text_mut().measure(dot, &halo_opts);
        draw_text_with_occlusion(
            sugarloaf,
            x + (diameter - halo_w) * 0.5,
            y + (diameter - halo_size) * 0.5 - 0.5 * s,
            dot,
            &halo_opts,
            occlusion_rects,
        );
    }

    let dot_opts = DrawOpts {
        font_size,
        color,
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let dot_w = sugarloaf.text_mut().measure(dot, &dot_opts);
    draw_text_with_occlusion(
        sugarloaf,
        x + (diameter - dot_w) * 0.5,
        y + (diameter - font_size) * 0.5 - 0.5 * s,
        dot,
        &dot_opts,
        occlusion_rects,
    );
}

/// Paint the running-sub-agent spinner: a square orbit of pastel dots
/// with a fading trail, occupying the same gutter slot a status dot
/// would. It reuses the terminal running-block loader's pure helpers
/// (`loader_*` in `render_policy`) so the side-panel spinner matches the
/// terminal one's look and cadence (1.35x phase, 12 Hz palette tick).
/// `now_seconds` is the panel's animation clock; the panel keeps
/// redraw-ticking while any sub-agent is active (see
/// `SidePanel::is_animating`), so the orbit stays in motion.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_subagent_spinner(
    sugarloaf: &mut Sugarloaf,
    dot_x: f32,
    dot_y: f32,
    diameter: f32,
    now_seconds: f32,
    clip: [f32; 4],
    s: f32,
) {
    let center_x = dot_x + diameter * 0.5;
    let center_y = dot_y + diameter * 0.5;
    let side = (diameter * 1.05).max(8.0 * s);
    let half = side * 0.5;
    let dot = (side * 0.4).clamp(2.4 * s, 4.8 * s);
    let loader_frame = loader_animation_frame(now_seconds);
    let phase = loader_frame.phase;
    let tick = loader_frame.tick;

    for (trail, alpha) in [1.0f32, 0.58, 0.32, 0.16].into_iter().enumerate() {
        let (dx, dy) = loader_orbit_position(phase - trail as f32 * 0.075, half);
        let x = center_x + dx - dot * 0.5;
        let y = center_y + dy - dot * 0.5;
        if intersect_rect([x, y, dot, dot], clip).is_none() {
            continue;
        }
        // Soft halo under the leading dots, same as the terminal loader.
        if trail <= 1 {
            let glow = dot * 1.75;
            sugarloaf.quad(
                None,
                center_x + dx - glow * 0.5,
                center_y + dy - glow * 0.5,
                glow,
                glow,
                loader_pastel_color(tick, trail, alpha * 0.24),
                [glow * 0.5; 4],
                DEPTH,
                ORDER_PANEL + 3,
            );
        }
        sugarloaf.quad(
            None,
            x,
            y,
            dot,
            dot,
            loader_pastel_color(tick, trail, alpha),
            [dot * 0.5; 4],
            DEPTH,
            ORDER_PANEL + 4,
        );
    }
}

pub(crate) fn push_provider_icon_clipped(
    sugarloaf: &mut Sugarloaf,
    kind: agent_icon::AgentKind,
    rect: [f32; 4],
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let Some((rect, source_rect)) = clip_image_rect(rect, clip) else {
        return;
    };
    push_image_overlay_clipped(
        sugarloaf,
        SIDE_PANEL_ICON_PANEL_ID,
        kind.image_id(),
        rect,
        source_rect,
        1,
        sugarloaf.scale_factor(),
        occlusion_rects,
    );
}

fn clip_image_rect(rect: [f32; 4], clip: [f32; 4]) -> Option<([f32; 4], [f32; 4])> {
    let [x, y, w, h] = rect;
    if w <= 0.0 || h <= 0.0 {
        return None;
    }
    let x1 = x.max(clip[0]);
    let y1 = y.max(clip[1]);
    let x2 = (x + w).min(clip[0] + clip[2]);
    let y2 = (y + h).min(clip[1] + clip[3]);
    if x2 <= x1 || y2 <= y1 {
        return None;
    }
    let source_rect = [(x1 - x) / w, (y1 - y) / h, (x2 - x) / w, (y2 - y) / h];
    Some(([x1, y1, x2 - x1, y2 - y1], source_rect))
}

pub(crate) fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_sessions_list(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    content_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    occlusion_rects: &[[f32; 4]],
    inner_radius: f32,
) {
    // Kick a background refresh on first show + every few seconds while
    // the home view is up. Cheap because the helper itself debounces.
    pane.maybe_refresh_side_panel_sessions();

    let [cx, cy, cw, ch] = content_rect;
    let pad_x = ROW_PADDING_X * s;
    let text_x = cx + pad_x;
    let text_w = (cw - pad_x * 2.0).max(0.0);
    let clip = [cx, cy, cw, ch];

    let mut y = cy + 14.0 * s;
    y = render_directory_section(
        sugarloaf,
        pane,
        text_x,
        y,
        text_w,
        theme,
        s,
        clip,
        occlusion_rects,
    );
    y += 8.0 * s;
    y = render_section_header(
        sugarloaf,
        "Previous Sessions",
        text_x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );

    // Home-mode search input — mirrors the /sessions modal's search bar.
    // When focused (arrow-up past the first session, or while typing) the row
    // gets the same highlight a selected session would, plus a caret that
    // sits right after the query text and advances as the user types.
    let search_row_h = (FONT_SIZE + 10.0) * s;
    let search_top = y;
    let search_focused = pane.side_panel().search_focused();
    let search_hl_y = (search_top - 2.0 * s).max(cy);
    let search_hl_h = search_row_h + 2.0 * s;
    {
        let query = pane.side_panel().session_query().to_string();
        if search_focused {
            sugarloaf.quad(
                None,
                cx,
                search_hl_y,
                cw,
                search_hl_h,
                theme.f32_alpha(theme.surface, 0.55),
                [inner_radius, inner_radius, inner_radius, inner_radius],
                DEPTH,
                ORDER_PANEL + 1,
            );
        }
        let sy = search_top + 1.0 * s;
        let text_opts = DrawOpts {
            font_size: FONT_SIZE * s,
            color: theme.u8(theme.fg),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        // Caret Y aligns with the text; X tracks the measured query width so
        // it moves right on each keystroke and left on backspace.
        let caret_x = if query.is_empty() {
            let placeholder = DrawOpts {
                color: theme.u8(theme.muted),
                ..text_opts
            };
            draw_text_with_occlusion(
                sugarloaf,
                text_x + 8.0 * s,
                sy,
                "Search sessions",
                &placeholder,
                occlusion_rects,
            );
            text_x + 2.0 * s
        } else {
            draw_text_with_occlusion(
                sugarloaf,
                text_x + 2.0 * s,
                sy,
                &query,
                &text_opts,
                occlusion_rects,
            );
            text_x + 2.0 * s + sugarloaf.text_mut().measure(&query, &text_opts)
        };
        if search_focused {
            sugarloaf.rounded_rect(
                None,
                caret_x + 1.0 * s,
                sy,
                (1.5 * s).max(1.0),
                FONT_SIZE * s,
                theme.f32(theme.accent),
                DEPTH,
                0.0,
                ORDER_PANEL + 3,
            );
        }
        sugarloaf.rect(
            None,
            text_x,
            search_top + search_row_h,
            text_w,
            (1.0 * s).max(1.0),
            theme.f32_alpha(theme.border, 0.6),
            DEPTH,
            ORDER_PANEL,
        );
    }
    y += search_row_h + 6.0 * s;

    let list_top = y;
    let list_h = (cy + ch - list_top).max(0.0);
    let list_rect = [cx, list_top, cw, list_h];

    // Cache the row capacity so update.clamp_scroll / scrolloff can use
    // it on the next interaction.
    let row_h = ROW_HEIGHT * s;
    pane.side_panel_mut().set_row_hit_rect(list_rect, row_h);
    let rows_visible = (list_h / row_h).floor().max(0.0) as usize;
    pane.side_panel_mut()
        .set_last_panel_height_rows(rows_visible.max(1));
    pane.side_panel_mut()
        .clamp_scroll_bounds(rows_visible.max(1));

    if !pane.side_panel().sessions_loaded() {
        let opts = DrawOpts {
            font_size: FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            list_top + 12.0 * s,
            "loading…",
            &opts,
            occlusion_rects,
        );
        return;
    }

    if pane.side_panel().sessions().is_empty() {
        // A semantic search may still surface matches for this query — show
        // a shimmering skeleton instead of prematurely declaring "No results".
        if !pane.side_panel().session_query().trim().is_empty()
            && pane.side_panel().semantic_searching()
        {
            let elapsed = pane.side_panel().semantic_search_elapsed();
            let fade_in = (elapsed / 0.18).min(1.0);
            const SKELETON_WIDTHS: [f32; 3] = [0.62, 0.44, 0.55];
            let bar_h = 10.0 * s;
            for (i, frac) in SKELETON_WIDTHS.iter().enumerate() {
                let bar_y = list_top + i as f32 * row_h + (row_h - bar_h) / 2.0;
                if bar_y + bar_h > cy + ch {
                    break;
                }
                let wave =
                    (elapsed / 1.3 * std::f32::consts::TAU - i as f32 * 0.55).sin();
                let alpha = (0.16 + 0.08 * wave).max(0.04) * fade_in;
                sugarloaf.rounded_rect(
                    None,
                    text_x,
                    bar_y,
                    (cw - 32.0 * s).max(0.0) * frac,
                    bar_h,
                    theme.f32_alpha(theme.muted, alpha),
                    DEPTH,
                    bar_h / 2.0,
                    ORDER_PANEL + 2,
                );
            }
            return;
        }
        let opts = DrawOpts {
            font_size: FONT_SIZE * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        let empty_label = if pane.side_panel().session_query().trim().is_empty() {
            "no previous sessions"
        } else {
            "No results"
        };
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            list_top + 12.0 * s,
            empty_label,
            &opts,
            occlusion_rects,
        );
        return;
    }

    // Continuous pixel scroll: `tick_scroll` returns the absolute animated
    // scroll position in rows; scale by this panel's row height and derive
    // the top row + sub-row remainder so every row lands at
    // `list_top + row*row_h - scroll_now_px` (pixel-smooth).
    let scroll_now_px = snap_to_device_px(
        pane.side_panel_mut().tick_scroll().max(0.0) * row_h,
        sugarloaf.scale_factor(),
    );
    let cursor_offset = pane.side_panel_mut().tick_cursor();
    let render_top = (scroll_now_px / row_h).floor().max(0.0) as usize;
    let frac = scroll_now_px - render_top as f32 * row_h;
    let selected = pane.side_panel().selected_index();
    let focused = pane.side_panel().is_focused();
    let list_bottom = list_rect[1] + list_rect[3];

    // Selected-row background (the trail cursor handles the focus
    // signal — same model as file_tree, see screen/render).
    pane.side_panel_mut().clear_selected_cursor_rect();
    let sessions_len = pane.side_panel().sessions().len();
    if search_focused {
        // Cursor is on the search row — the moving text caret drawn in the
        // search block above is the cursor, so skip the session-row
        // highlight + left-gutter block cursor entirely.
    } else if selected < sessions_len {
        let row_ix = selected as isize - render_top as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h - frac + cursor_offset;
        let row_bottom = row_y + row_h;
        let visible_y = row_y.max(list_rect[1]);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h > 0.0 {
            let bg_color = theme.f32_alpha(theme.surface, 0.55);
            sugarloaf.quad(
                None,
                list_rect[0],
                visible_y,
                list_rect[2],
                visible_h,
                bg_color,
                edge_row_radii(
                    visible_y,
                    visible_h,
                    list_rect[1],
                    list_bottom,
                    inner_radius,
                ),
                DEPTH,
                ORDER_PANEL + 2,
            );
            if focused {
                let font_size = FONT_SIZE * s;
                let cursor_w = (font_size * 0.6).max(2.0);
                let cursor_x = list_rect[0] + (ROW_PADDING_X * s - cursor_w).max(0.0);
                let cursor_h = (row_h - 6.0 * s).max(font_size).min(row_h);
                let cursor_y = (row_y + (row_h - cursor_h) / 2.0)
                    .clamp(list_rect[1], (list_bottom - cursor_h).max(list_rect[1]));
                pane.side_panel_mut()
                    .set_selected_cursor_rect([cursor_x, cursor_y, cursor_w, cursor_h]);
            }
        }
    }

    // `frac` is always < row_h, so a small fixed overscan covers the
    // partially-visible top/bottom rows during animation.
    let overscan = 2usize;
    let start = render_top.saturating_sub(overscan);
    let end = (render_top + rows_visible.max(1) + overscan).min(sessions_len);

    // Row text clips to `list_rect`, not the panel content rect, so
    // rows scrolling up off the top can't paint over the "PREVIOUS
    // SESSIONS" header. Same pattern the file tree uses to keep label
    // text inside the panel frame.
    let title_opts = DrawOpts {
        font_size: FONT_SIZE * s,
        color: theme.u8(theme.fg),
        clip_rect: Some(list_rect),
        ..DrawOpts::default()
    };
    // Cyan date-group / "Pinned" header rows, matching the /sessions modal.
    let header_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.92,
        color: theme.u8(theme.cyan),
        bold: true,
        clip_rect: Some(list_rect),
        ..DrawOpts::default()
    };

    // A small left gutter so the active session can show a colored status
    // dot (mirroring the branch rows). Titles start past the gutter on every
    // row so the list stays aligned whether or not a row carries a dot.
    let dot_gutter = 16.0 * s;
    let dot_diameter = 7.0 * s;
    let pin_d = 6.0 * s;
    let title_x = text_x + dot_gutter;
    let current_id = pane.session_id_str().map(str::to_string);

    let sessions = pane.side_panel().sessions().to_vec();
    for absolute_ix in start..end {
        let entry = &sessions[absolute_ix];
        let row_ix = absolute_ix as isize - render_top as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h - frac;
        let row_bottom = row_y + row_h;
        let visible_y = row_y.max(list_rect[1]);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h <= 0.0 {
            continue;
        }

        let text_y = row_y + (row_h - FONT_SIZE * s) / 2.0;

        // Date-group / "Pinned" header row.
        if entry.is_header {
            let label = truncate_to_fit(&entry.title, text_w, sugarloaf, &header_opts);
            draw_text_with_occlusion(
                sugarloaf,
                text_x,
                text_y,
                &label,
                &header_opts,
                occlusion_rects,
            );
            continue;
        }

        let is_current = current_id.as_deref() == Some(entry.id.as_str());

        // Colored dot for the active session — a clear live-status signal.
        if is_current {
            let dot_y = row_y + (row_h - dot_diameter) / 2.0;
            draw_status_dot_text(
                sugarloaf,
                text_x,
                dot_y,
                dot_diameter,
                theme.u8(theme.green),
                Some((theme.u8(theme.green), 0.35)),
                list_rect,
                occlusion_rects,
                s,
            );
        }

        // Pinned marker — a small cyan dot in the row's right padding.
        let pin_reserve = if entry.pinned { pin_d + 8.0 * s } else { 0.0 };
        if entry.pinned {
            let pin_x = text_x + text_w - pin_d;
            let pin_y = row_y + (row_h - pin_d) / 2.0;
            sugarloaf.rounded_rect(
                None,
                pin_x,
                pin_y,
                pin_d,
                pin_d,
                theme.f32(theme.cyan),
                DEPTH,
                pin_d / 2.0,
                ORDER_PANEL + 3,
            );
        }

        let title_budget = (text_w - dot_gutter - pin_reserve).max(0.0);
        let title_text =
            truncate_to_fit(&entry.title, title_budget, sugarloaf, &title_opts);
        draw_text_with_occlusion(
            sugarloaf,
            title_x,
            text_y,
            &title_text,
            &title_opts,
            occlusion_rects,
        );
    }
}
