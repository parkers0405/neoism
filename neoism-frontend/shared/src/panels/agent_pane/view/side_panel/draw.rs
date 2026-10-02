use super::sections::{render_directory_section, render_section_header, SECTION_GAP};
use super::*;
use crate::panels::agent_pane::view::draw::measure_text_cached;

const SCRAMBLE_MS: f32 = 320.0;
const SCRAMBLE_GLYPHS: &[char] = &['#', '@', '%', '&', '*', '+', '?', '/', '~', '='];

pub(crate) fn render_scramble_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    elapsed_ms: f32,
) {
    render_scramble_text_inner(sugarloaf, x, y, text, opts, elapsed_ms, true);
}

pub(crate) fn render_loading_scramble_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    elapsed_ms: f32,
) {
    render_scramble_text_inner(sugarloaf, x, y, text, opts, elapsed_ms, false);
}

fn render_scramble_text_inner(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    text: &str,
    opts: &DrawOpts,
    elapsed_ms: f32,
    settle: bool,
) {
    let colors = [
        [255, 82, 82, 255],
        [255, 184, 77, 255],
        [255, 235, 59, 255],
        [105, 240, 174, 255],
        [77, 208, 225, 255],
        [100, 149, 237, 255],
        [186, 104, 200, 255],
    ];
    let progress = (elapsed_ms / SCRAMBLE_MS).clamp(0.0, 1.0);
    let chars = text.chars().collect::<Vec<_>>();
    let mut draw_x = x;
    for (index, target) in chars.iter().copied().enumerate() {
        let settle_at = 0.35 + 0.65 * (index as f32 + 1.0) / chars.len().max(1) as f32;
        let settled = (settle && progress >= settle_at) || target.is_whitespace();
        let tick = (elapsed_ms / 34.0).floor() as usize;
        let glyph = if settled {
            target
        } else {
            SCRAMBLE_GLYPHS[(tick + index * 3) % SCRAMBLE_GLYPHS.len()]
        };
        let mut glyph_opts = *opts;
        if !settled {
            glyph_opts.color = colors[(tick + index) % colors.len()];
        }
        let glyph = glyph.to_string();
        sugarloaf.text_mut().draw(draw_x, y, &glyph, &glyph_opts);
        draw_x += sugarloaf.text_mut().measure(&target.to_string(), opts);
    }
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
    glow_order: u8,
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
                glow_order,
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
            glow_order + 1,
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
fn draw_session_loading_skeleton(
    sugarloaf: &mut Sugarloaf,
    list_rect: [f32; 4],
    text_x: f32,
    row_h: f32,
    elapsed: f32,
    theme: &IdeTheme,
    s: f32,
) {
    // Match the file-tree loader: a short fade-in, square icon stubs,
    // varied rounded text bars, and a sine wave travelling down the rows.
    let fade_in = (elapsed / 0.18).min(1.0);
    const SKELETON_WIDTHS: [f32; 12] = [
        0.58, 0.72, 0.46, 0.64, 0.38, 0.55, 0.68, 0.44, 0.52, 0.36, 0.62, 0.48,
    ];
    let [_, list_top, list_w, list_h] = list_rect;
    let list_bottom = list_top + list_h;
    let rows_visible = (list_h / row_h).ceil().max(0.0) as usize;
    let stub_size = (10.0 * s).min(row_h).max(4.0);
    let bar_h = (FONT_SIZE * 0.72 * s).max(4.0);
    let gap = 7.0 * s;

    for (i, width) in SKELETON_WIDTHS.iter().enumerate().take(rows_visible) {
        let row_y = list_top + i as f32 * row_h;
        if row_y + row_h > list_bottom + 0.5 {
            break;
        }
        let wave = (elapsed / 1.3 * std::f32::consts::TAU - i as f32 * 0.55).sin();
        let alpha = (0.16 + 0.08 * wave).max(0.04) * fade_in;
        let stub_y = row_y + (row_h - stub_size) / 2.0;
        sugarloaf.quad(
            None,
            text_x,
            stub_y,
            stub_size,
            stub_size,
            theme.f32_alpha(theme.muted, alpha),
            [3.0 * s; 4],
            DEPTH,
            ORDER_PANEL + 2,
        );

        let bar_x = text_x + stub_size + gap;
        let bar_y = row_y + (row_h - bar_h) / 2.0;
        let bar_budget = (list_rect[0] + list_w - ROW_PADDING_X * s - bar_x).max(0.0);
        let bar_w = bar_budget * width;
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
                ORDER_PANEL + 2,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn render_sessions_list(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentSidePanelPane,
    content_rect: [f32; 4],
    theme: &IdeTheme,
    s: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
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
    pane.side_panel_mut().clear_selected_cursor_rect();

    // Catalog is always the left rail, whether a chat is open or not.
    let mut y = cy + 14.0 * s;
    pane.side_panel_mut().close_provider_menu();
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
    y += SECTION_GAP * s;
    y = render_section_header(
        sugarloaf,
        "Conversations",
        text_x,
        y,
        theme,
        s,
        clip,
        occlusion_rects,
    );

    let button_h = 34.0 * s;
    let button_rect = [text_x, y + 5.0 * s, text_w, button_h];
    pane.side_panel_mut().set_new_chat_rect(button_rect);
    let button_hovered = mouse.is_some_and(|(mx, my)| {
        mx >= button_rect[0]
            && mx <= button_rect[0] + button_rect[2]
            && my >= button_rect[1]
            && my <= button_rect[1] + button_rect[3]
    });
    let button_selected = pane.side_panel().new_chat_selected()
        && pane.side_panel().is_focused();
    let radius = 7.0 * s;
    if button_hovered || button_selected {
        sugarloaf.rounded_rect(
            None,
            button_rect[0],
            button_rect[1],
            button_rect[2],
            button_rect[3],
            theme.f32_alpha(theme.surface, if button_hovered { 0.78 } else { 0.55 }),
            DEPTH,
            radius,
            ORDER_PANEL + 2,
        );
    }
    let icon_side = 20.0 * s;
    let icon_x = button_rect[0] + 7.0 * s;
    let icon_y = button_rect[1] + (button_h - icon_side) / 2.0;
    if button_hovered || button_selected {
        sugarloaf.rounded_rect(
            None,
            icon_x,
            icon_y,
            icon_side,
            icon_side,
            theme.f32_alpha(theme.accent, if button_hovered { 0.24 } else { 0.16 }),
            DEPTH,
            5.0 * s,
            ORDER_PANEL + 3,
        );
    }
    let plus_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.readable_accent(theme.accent)),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(
        sugarloaf,
        icon_x + 5.0 * s,
        icon_y + 4.0 * s,
        "+",
        &plus_opts,
        occlusion_rects,
    );
    let label_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.95,
        color: theme.u8(theme.fg),
        bold: button_hovered || button_selected,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(
        sugarloaf,
        icon_x + icon_side + 9.0 * s,
        button_rect[1] + (button_h - FONT_SIZE * s * 0.95) / 2.0,
        "New chat",
        &label_opts,
        occlusion_rects,
    );
    if button_selected {
        let cursor_w = (FONT_SIZE * s * 0.6).max(2.0);
        let cursor_h = (button_h - 8.0 * s).max(FONT_SIZE * s).min(button_h);
        pane.side_panel_mut().set_selected_cursor_rect([
            button_rect[0] + 1.0 * s,
            button_rect[1] + (button_h - cursor_h) / 2.0,
            cursor_w,
            cursor_h,
        ]);
    }

    y = button_rect[1] + button_rect[3] + 8.0 * s;
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

    if matches!(
        pane.side_panel().session_catalog_state(),
        crate::panels::agent_pane::state::side_panel::SessionCatalogState::Initial
            | crate::panels::agent_pane::state::side_panel::SessionCatalogState::Loading
    ) && pane.side_panel().sessions().is_empty()
    {
        draw_session_loading_skeleton(
            sugarloaf,
            list_rect,
            text_x,
            row_h,
            pane.side_panel().sessions_loading_elapsed(),
            theme,
            s,
        );
        return;
    }

    if let crate::panels::agent_pane::state::side_panel::SessionCatalogState::Error(
        message,
    ) = pane.side_panel().session_catalog_state()
    {
        if pane.side_panel().sessions().is_empty() {
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
                message,
                &opts,
                occlusion_rects,
            );
            return;
        }
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

    let sessions_len = pane.side_panel().sessions().len();
    if selected < sessions_len && !pane.side_panel().new_chat_selected() {
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
    // Date groups share the model chip's subdued blue accent across themes.
    let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
    let header_opts = DrawOpts {
        font_size: FONT_SIZE * s * 0.92,
        color: theme.u8(theme.readable_accent(theme.blue)),
        bold: true,
        extrude: true,
        font_id: pixel_font,
        clip_rect: Some(list_rect),
        ..DrawOpts::default()
    };

    // Reserve only the activity mark plus a normal gap. The old 34px gutter
    // left a large empty column on scaled displays.
    let dot_gutter = 18.0 * s;
    let dot_diameter = 7.0 * s;
    let title_x = text_x + dot_gutter;
    let current_id = pane
        .session_id_str()
        .or_else(|| pane.side_panel().viewed_session_id())
        .map(str::to_string);

    // Feed the measured monospace column budget back so excerpt chunks wrap
    // to this exact panel width; a change (resize) rebuilds the display list
    // before we snapshot it below.
    {
        let excerpt_font = FONT_SIZE * s * 0.9;
        let probe = DrawOpts {
            font_size: excerpt_font,
            ..DrawOpts::default()
        };
        let char_w = sugarloaf.text_mut().measure("M", &probe).max(1.0);
        let excerpt_w = (text_w - dot_gutter - 10.0 * s).max(char_w);
        pane.side_panel_mut()
            .set_result_wrap_columns((excerpt_w / char_w).floor() as usize);
    }

    let rename_buffer = pane.session_rename_buffer();
    let now_ms = web_time::SystemTime::now()
        .duration_since(web_time::UNIX_EPOCH)
        .map(|time| time.as_millis() as u64)
        .unwrap_or(0);
    let hovered_session = pane.side_panel().hovered_session();
    let session_hover_scale = pane.side_panel().session_hover_scale();
    let title_hover_elapsed = pane.side_panel().session_title_hover_elapsed();
    let mut title_hover_overflow = false;
    let sessions = pane.side_panel().sessions();
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
            let label =
                truncate_sidebar_text(&entry.title, text_w, sugarloaf, &header_opts);
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

        // Semantic-match excerpt row: the matched transcript chunk, dim and
        // indented under its session, with a small accent tick. Selectable —
        // activating it resumes the parent session — so the shared hover
        // treatment below still applies.
        if entry.is_excerpt {
            let first_of_run = absolute_ix == 0 || !sessions[absolute_ix - 1].is_excerpt;
            let hover = if hovered_session == Some(absolute_ix) {
                session_hover_scale
            } else {
                0.0
            };
            if hover > 0.002 {
                sugarloaf.quad(
                    None,
                    list_rect[0],
                    visible_y,
                    list_rect[2],
                    visible_h,
                    theme.f32_alpha(theme.surface, 0.28 * hover),
                    [5.0 * s; 4],
                    DEPTH,
                    ORDER_PANEL + 1,
                );
            }
            let tick_w = 2.0 * s;
            // One continuous tick per excerpt run: full-height on every row,
            // so wrapped lines read as a single quoted chunk.
            let tick_y = if first_of_run { row_y + 4.0 * s } else { row_y };
            let tick_y = tick_y.max(list_rect[1]);
            let tick_h = (row_bottom.min(list_bottom) - tick_y).max(0.0);
            sugarloaf.rounded_rect(
                None,
                title_x + 2.0 * s,
                tick_y,
                tick_w,
                tick_h,
                theme.f32_alpha(theme.cyan, 0.55),
                DEPTH,
                tick_w / 2.0,
                ORDER_PANEL + 2,
            );
            let excerpt_opts = DrawOpts {
                font_size: FONT_SIZE * s * 0.9,
                color: theme.u8_alpha(theme.fg, 0.62),
                clip_rect: Some(list_rect),
                ..DrawOpts::default()
            };
            let excerpt_x = title_x + 10.0 * s;
            let excerpt_w = text_w - dot_gutter - 10.0 * s;
            let label =
                truncate_sidebar_text(&entry.title, excerpt_w, sugarloaf, &excerpt_opts);
            // Matched search terms render as bright segments over a soft
            // accent wash — the excerpt reads like a real search result.
            // Lines are pre-wrapped to the measured budget, so truncation
            // is the rare case; when it fires, highlight offsets no longer
            // line up with the label and we fall back to the flat draw.
            if entry.highlights.is_empty() || label != entry.title {
                draw_text_with_occlusion(
                    sugarloaf,
                    excerpt_x,
                    text_y,
                    &label,
                    &excerpt_opts,
                    occlusion_rects,
                );
                continue;
            }
            let bright_opts = DrawOpts {
                color: theme.u8_alpha(theme.fg, 0.95),
                ..excerpt_opts.clone()
            };
            let mut cursor_x = excerpt_x;
            let mut byte = 0;
            let mut spans = entry.highlights.iter().copied().peekable();
            while byte < label.len() {
                let (bright, end) = match spans.peek().copied() {
                    Some((start, end)) if start <= byte => {
                        spans.next();
                        (true, end.min(label.len()))
                    }
                    Some((start, _)) => (false, start.min(label.len())),
                    None => (false, label.len()),
                };
                let segment = &label[byte..end];
                byte = end;
                if segment.is_empty() {
                    continue;
                }
                let opts = if bright { &bright_opts } else { &excerpt_opts };
                let advance = measure_text_cached(sugarloaf, segment, opts);
                if bright {
                    sugarloaf.rounded_rect(
                        None,
                        cursor_x - 1.0 * s,
                        row_y + 2.0 * s,
                        advance + 2.0 * s,
                        row_h - 4.0 * s,
                        theme.f32_alpha(theme.cyan, 0.16),
                        DEPTH,
                        3.0 * s,
                        ORDER_PANEL + 2,
                    );
                }
                draw_text_with_occlusion(
                    sugarloaf,
                    cursor_x,
                    text_y,
                    segment,
                    opts,
                    occlusion_rects,
                );
                cursor_x += advance;
            }
            continue;
        }

        let is_current = current_id.as_deref() == Some(entry.id.as_str());
        let running = session_entry_is_running(entry);
        let hover = if hovered_session == Some(absolute_ix) {
            session_hover_scale
        } else {
            0.0
        };
        let hover_scale = 1.0 + 0.045 * hover;
        if hover > 0.002 {
            sugarloaf.quad(
                None,
                list_rect[0],
                visible_y,
                list_rect[2],
                visible_h,
                theme.f32_alpha(theme.surface, 0.28 * hover),
                [5.0 * s; 4],
                DEPTH,
                ORDER_PANEL + 1,
            );
        }

        // Running conversations share the same pastel orbit as active agents
        // in the details rail. The current-but-idle conversation keeps the
        // quieter green dot so "open" never reads as "working".
        if running {
            let spinner = 10.0 * s * hover_scale;
            draw_subagent_spinner(
                sugarloaf,
                text_x - (spinner - dot_diameter) * 0.5,
                row_y + (row_h - spinner) * 0.5,
                spinner,
                now_seconds,
                list_rect,
                s,
                ORDER_PANEL + 2,
            );
        } else if is_current {
            let scaled_dot = dot_diameter * hover_scale;
            let dot_y = row_y + (row_h - scaled_dot) / 2.0;
            draw_status_dot_text(
                sugarloaf,
                text_x - (scaled_dot - dot_diameter) * 0.5,
                dot_y,
                scaled_dot,
                theme.u8(theme.green),
                Some((theme.u8(theme.green), 0.35)),
                list_rect,
                occlusion_rects,
                s,
            );
        }

        // Conversation cards lead with the product identity, matching the
        // compact app/title/time hierarchy used by the rest of the sidebar.
        let source_icon = agent_icon::AgentKind::Neoism;
        let icon_size = 13.0 * s;
        push_provider_icon_clipped(
            sugarloaf,
            source_icon,
            [title_x, row_y + 4.0 * s, icon_size, icon_size],
            list_rect,
            occlusion_rects,
        );

        let title_budget = (text_w - dot_gutter).max(0.0);
        let context = if entry.time_label.trim().is_empty() {
            relative_session_time(entry.updated_ms, now_ms)
        } else {
            entry.time_label.clone()
        };
        let identity_opts = DrawOpts {
            font_size: FONT_SIZE * s * 0.82,
            color: theme.u8_alpha(theme.fg, 0.72),
            clip_rect: Some(list_rect),
            ..DrawOpts::default()
        };
        let identity_x = title_x + icon_size + 6.0 * s;
        draw_text_with_occlusion(
            sugarloaf,
            identity_x,
            row_y + 4.0 * s,
            source_icon.display_name(),
            &identity_opts,
            occlusion_rects,
        );
        if !context.is_empty() {
            let context_opts = DrawOpts {
                color: theme.u8(theme.muted),
                ..identity_opts.clone()
            };
            let identity_end = identity_x
                + measure_text_cached(
                    sugarloaf,
                    source_icon.display_name(),
                    &identity_opts,
                );
            let context_right = text_x + text_w;
            let context_budget = (context_right - identity_end - 10.0 * s).max(0.0);
            let label =
                truncate_sidebar_text(&context, context_budget, sugarloaf, &context_opts);
            let context_w = measure_text_cached(sugarloaf, &label, &context_opts);
            draw_text_with_occlusion(
                sugarloaf,
                context_right - context_w,
                row_y + 4.0 * s,
                &label,
                &context_opts,
                occlusion_rects,
            );
        }
        let mut hovered_title_opts = title_opts;
        hovered_title_opts.font_size *= hover_scale;
        hovered_title_opts.clip_rect =
            Some([title_x, visible_y, title_budget, visible_h]);
        let display_title = if focused && absolute_ix == selected {
            rename_buffer.as_deref().unwrap_or(&entry.title)
        } else {
            &entry.title
        };
        let title_hovered = hovered_session == Some(absolute_ix);
        let full_title_width =
            measure_text_cached(sugarloaf, display_title, &hovered_title_opts);
        let overflow_distance = (full_title_width - title_budget).max(0.0);
        if title_hovered {
            title_hover_overflow = overflow_distance > 0.5;
        }
        let title_offset = title_hovered
            .then_some((title_hover_elapsed, overflow_distance))
            .and_then(|(elapsed, distance)| elapsed.map(|elapsed| (elapsed, distance)))
            .and_then(|(elapsed, distance)| {
                crate::primitives::hover_title_offset(elapsed, distance, s)
            });
        let title_text = if title_offset.is_some() {
            display_title.to_owned()
        } else {
            truncate_sidebar_text(
                display_title,
                title_budget,
                sugarloaf,
                &hovered_title_opts,
            )
        };
        let scaled_text_y = row_y + 23.0 * s;
        draw_text_with_occlusion(
            sugarloaf,
            title_x - title_offset.unwrap_or(0.0),
            scaled_text_y,
            &title_text,
            &hovered_title_opts,
            occlusion_rects,
        );
    }
    pane.side_panel_mut()
        .set_session_title_hover_overflow(title_hover_overflow);

    if pane.side_panel().session_page_loading() {
        let badge = 24.0 * s;
        let badge_x = list_rect[0] + (list_rect[2] - badge) * 0.5;
        let badge_y = list_bottom - badge - 4.0 * s;
        sugarloaf.quad(
            None,
            badge_x,
            badge_y,
            badge,
            badge,
            theme.f32_alpha(theme.surface, 0.92),
            [badge * 0.5; 4],
            DEPTH,
            ORDER_PANEL + 5,
        );
        let spinner = 10.0 * s;
        draw_subagent_spinner(
            sugarloaf,
            badge_x + (badge - spinner) * 0.5,
            badge_y + (badge - spinner) * 0.5,
            spinner,
            now_seconds,
            list_rect,
            s,
            ORDER_PANEL + 6,
        );
    }
}

fn relative_session_time(updated_ms: u64, now_ms: u64) -> String {
    if updated_ms == 0 || now_ms == 0 {
        return String::new();
    }
    let minutes = now_ms.saturating_sub(updated_ms) / 60_000;
    if minutes < 1 {
        "just now".into()
    } else if minutes < 60 {
        format!("{minutes}m ago")
    } else if minutes < 1_440 {
        format!("{}h ago", minutes / 60)
    } else if minutes < 10_080 {
        format!("{}d ago", minutes / 1_440)
    } else {
        format!("{}w ago", minutes / 10_080)
    }
}

#[cfg(test)]
mod row_time_tests {
    use super::relative_session_time;

    #[test]
    fn relative_time_uses_real_timestamp_only() {
        let now = 1_800_000_000_000;
        assert_eq!(relative_session_time(0, now), "");
        assert_eq!(relative_session_time(now - 15_000, now), "just now");
        assert_eq!(relative_session_time(now - 3_600_000, now), "1h ago");
        assert_eq!(relative_session_time(now - 172_800_000, now), "2d ago");
    }
}
