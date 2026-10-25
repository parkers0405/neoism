use super::sections::render_section_header;
use super::*;
use crate::panels::agent_pane::view::draw::measure_text_cached;

#[derive(Clone, Copy)]
enum CatalogIcon {
    Compose,
    Search,
    Folder,
    Chevron,
}

/// Use the file tree's bundled icon font, sizing, and ink-centered alignment.
#[allow(clippy::too_many_arguments)]
fn draw_catalog_icon(
    sugarloaf: &mut Sugarloaf,
    icon: CatalogIcon,
    x: f32,
    y: f32,
    s: f32,
    theme: &IdeTheme,
    clip: [f32; 4],
    cuts: &[[f32; 4]],
) {
    let (glyph, color) = match icon {
        CatalogIcon::Compose => ("\u{f044}", theme.fg),
        CatalogIcon::Search => ("\u{f002}", theme.fg),
        CatalogIcon::Folder => (
            crate::panels::file_tree::icons::folder_icon(false).0,
            theme.folder,
        ),
        CatalogIcon::Chevron => ("\u{f078}", theme.muted),
    };
    let opts = DrawOpts {
        font_size: crate::panels::file_tree::ICON_FONT_SIZE * s,
        color: theme.u8(color),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    crate::primitives::draw_icon_centered_with_occlusion(
        sugarloaf,
        x,
        [x, y, 20.0 * s, 20.0 * s],
        glyph,
        &opts,
        cuts,
        true,
    );
}

pub(super) fn catalog_action_layout(
    rect: [f32; 4],
    scale: f32,
    font_size: f32,
) -> ([f32; 4], [f32; 2]) {
    let icon_side = 20.0 * scale;
    let icon_x = rect[0] + 7.0 * scale;
    (
        [
            icon_x,
            rect[1] + (rect[3] - icon_side) / 2.0,
            icon_side,
            icon_side,
        ],
        [icon_x + 29.0 * scale, rect[1] + (rect[3] - font_size) / 2.0],
    )
}

fn project_label_opts(theme: &IdeTheme, scale: f32, clip: [f32; 4]) -> DrawOpts {
    DrawOpts {
        font_size: FONT_SIZE * scale,
        color: theme.u8(theme.readable_accent(theme.blue)),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    }
}

fn conversation_row_columns(left: f32, width: f32, scale: f32) -> (f32, f32) {
    let width = width.max(0.0);
    let status_width = (18.0 * scale).min(width);
    (width - status_width, left + width - status_width * 0.5)
}

/// Project nesting is not a branch relationship. Only explicit branch rows
/// receive a connector; flattened depth is relative to the owning root.
pub(super) fn branch_row_indent(
    entry: &NeoismAgentSessionEntry,
    root_depth: usize,
    scale: f32,
) -> f32 {
    if entry.branch_root_id.is_some() {
        entry.depth.saturating_sub(root_depth).max(1) as f32 * 24.0 * scale
    } else {
        0.0
    }
}

pub(super) fn branch_connector_top(
    sessions: &[NeoismAgentSessionEntry],
    index: usize,
    row_y: f32,
    row_h: f32,
    scale: f32,
) -> Option<f32> {
    let entry = sessions.get(index)?;
    let root = entry.branch_root_id.as_deref()?;
    let (previous_index, previous) =
        sessions[..index]
            .iter()
            .enumerate()
            .rev()
            .find(|(_, row)| {
                row.id == root
                    || (row.branch_root_id.as_deref() == Some(root)
                        && row.depth <= entry.depth)
            })?;
    let previous_y = row_y - (index - previous_index) as f32 * row_h;
    let sibling =
        previous.branch_root_id.as_deref() == Some(root) && previous.depth == entry.depth;
    let sibling_offset = 23.0 + FONT_SIZE * 0.5;
    Some(
        previous_y
            + if sibling {
                sibling_offset
            } else {
                // Keep a small gap below the parent title's ink.
                23.0 + FONT_SIZE + 4.0
            } * scale,
    )
}

pub(super) struct BranchConnectorGeometry {
    pub straight: [[f32; 4]; 2],
    pub center: [f32; 2],
    pub radius: f32,
    pub stroke: f32,
    pub curve_bounds: [f32; 4],
}

/// Shared straight-stroke tangents and an analytic bottom-left quarter-ring.
pub(super) fn branch_connector_geometry(
    title_x: f32,
    row_y: f32,
    top: f32,
    continues: bool,
    scale: f32,
    device_scale: f32,
) -> BranchConnectorGeometry {
    let device_scale = device_scale.max(f32::EPSILON);
    let stroke = (2.0 * scale * device_scale).round().max(2.0) / device_scale;
    let radius = snap_to_device_px(5.0 * scale, device_scale).max(stroke);
    let left = snap_to_device_px(title_x - 19.0 * scale - stroke * 0.5, device_scale);
    let elbow_y =
        snap_to_device_px(row_y + (23.0 + FONT_SIZE * 0.5) * scale, device_scale);
    let center_x = left + stroke * 0.5 + radius;
    let center_y = elbow_y - radius;
    let top = snap_to_device_px(top, device_scale);
    let bottom = if continues {
        elbow_y + stroke * 0.5
    } else {
        center_y
    };
    let straight = [
        [left, top, stroke, (bottom - top).max(0.0)],
        [
            center_x,
            elbow_y - stroke * 0.5,
            (title_x - 6.0 * scale - center_x).max(0.0),
            stroke,
        ],
    ];
    // Keep the AA fringe while clipping at the two tangent axes. The full
    // circle's center/radius stay fixed under scrolling and overlay cutouts.
    let padded_outer = radius + stroke * 0.5 + 0.5 / device_scale;
    BranchConnectorGeometry {
        straight,
        center: [center_x, center_y],
        radius,
        stroke,
        curve_bounds: [
            center_x - padded_outer,
            center_y,
            padded_outer,
            padded_outer,
        ],
    }
}

pub(super) fn catalog_branch_status(entry: &NeoismAgentSessionEntry) -> BranchStatus {
    entry.branch_status.unwrap_or_else(|| {
        entry
            .runtime_status
            .as_deref()
            .and_then(BranchStatus::from_runtime_status)
            .unwrap_or(BranchStatus::Completed)
    })
}

pub(super) fn catalog_branch_activity_label(entry: &NeoismAgentSessionEntry) -> &str {
    entry
        .branch_activity_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| catalog_branch_status(entry).label())
}

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

/// Resolve server authority before the legacy runtime-status fallback.
pub(super) fn catalog_activity_color(
    entry: &NeoismAgentSessionEntry,
    red: [f32; 3],
    yellow: [f32; 3],
) -> Option<[f32; 3]> {
    use neoism_agent_core::CatalogActivity;
    match entry.catalog_activity {
        Some(CatalogActivity::Idle) => None,
        Some(CatalogActivity::Running) => Some([1.0; 3]),
        Some(CatalogActivity::Background) => Some(red),
        Some(CatalogActivity::Permission) => Some(yellow),
        None => session_entry_is_running(entry).then_some([1.0; 3]),
    }
}

fn running_square_visible(square: [f32; 4], clip: [f32; 4], cuts: &[[f32; 4]]) -> bool {
    intersect_rect(square, clip).is_some()
        && !cuts
            .iter()
            .any(|cut| intersect_rect(square, *cut).is_some())
}

/// Literal timeline running indicator, centered in the sidebar status slot.
/// Returns true only when a nonoccluded square is painted.
#[allow(clippy::too_many_arguments)]
pub(crate) fn draw_subagent_spinner(
    sugarloaf: &mut Sugarloaf,
    dot_x: f32,
    dot_y: f32,
    diameter: f32,
    rgb: [f32; 3],
    now_seconds: f32,
    clip: [f32; 4],
    s: f32,
    order: u8,
    occlusion_rects: &[[f32; 4]],
) -> bool {
    let mut visible = false;
    for (trail, alpha) in super::super::draw::RUNNING_SQUARE_ALPHAS
        .into_iter()
        .enumerate()
    {
        let (dx, dy) = super::super::draw::running_square_position(now_seconds, trail, s);
        let square = [
            dot_x + diameter * 0.5 + dx - s,
            dot_y + diameter * 0.5 + dy - s,
            2.0 * s,
            2.0 * s,
        ];
        if running_square_visible(square, clip, occlusion_rects) {
            super::super::draw::draw_rect_clipped(
                sugarloaf,
                square,
                [rgb[0], rgb[1], rgb[2], alpha],
                order,
                clip,
            );
            visible = true;
        }
    }
    visible
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
    let navigation = pane.side_panel().navigation_header_enabled();
    pane.side_panel_mut().clear_header_rects();
    if navigation {
        pane.side_panel_mut().clear_session_search_rect();
    }
    let pad_x = if navigation {
        18.0 * s
    } else {
        ROW_PADDING_X * s
    };
    let text_x = cx + pad_x;
    let text_w = (cw - pad_x * 2.0).max(0.0);
    let clip = [cx, cy, cw, ch];
    pane.side_panel_mut().clear_selected_cursor_rect();

    // Catalog is always the left rail, whether a chat is open or not.
    let mut y = cy + 14.0 * s;
    pane.side_panel_mut().close_provider_menu();
    y = render_section_header(
        sugarloaf,
        "Chats",
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
    let button_selected =
        pane.side_panel().new_chat_selected() && pane.side_panel().is_focused();
    let radius = 7.0 * s;
    if button_hovered || button_selected {
        sugarloaf.rounded_rect(
            None,
            button_rect[0],
            button_rect[1],
            button_rect[2],
            button_rect[3],
            sidebar_background_opacity(
                theme.f32_alpha(theme.surface, if button_hovered { 0.78 } else { 0.55 }),
            ),
            DEPTH,
            radius,
            ORDER_PANEL + 2,
        );
    }
    let label_size = FONT_SIZE * s * if navigation { 1.0 } else { 0.95 };
    let ([icon_x, icon_y, icon_side, _], [label_x, label_y]) =
        catalog_action_layout(button_rect, s, label_size);
    if !navigation && (button_hovered || button_selected) {
        sugarloaf.rounded_rect(
            None,
            icon_x,
            icon_y,
            icon_side,
            icon_side,
            sidebar_background_opacity(
                theme.f32_alpha(theme.accent, if button_hovered { 0.24 } else { 0.16 }),
            ),
            DEPTH,
            5.0 * s,
            ORDER_PANEL + 3,
        );
    }
    draw_catalog_icon(
        sugarloaf,
        CatalogIcon::Compose,
        icon_x,
        icon_y,
        s,
        theme,
        clip,
        occlusion_rects,
    );
    let label_opts = DrawOpts {
        font_size: label_size,
        color: theme.u8(theme.fg),
        bold: !navigation && (button_hovered || button_selected),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    draw_text_with_occlusion(
        sugarloaf,
        label_x,
        label_y,
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

    y = button_rect[1] + button_rect[3] + if navigation { 0.0 } else { 8.0 * s };
    if navigation {
        use crate::panels::agent_pane::state::side_panel::CatalogHeaderAction;
        let label = pane.side_panel().project_button_label().to_owned();
        for (action, title, icon) in [
            (CatalogHeaderAction::Search, "Search", CatalogIcon::Search),
            (CatalogHeaderAction::Projects, label.as_str(), CatalogIcon::Folder),
        ] {
            let rect = [text_x, y, text_w.max(0.0), button_h];
            let ([icon_x, icon_y, _, _], [label_x, label_y]) =
                catalog_action_layout(rect, s, label_size);
            if let Some(hit) = intersect_rect(rect, clip) {
                pane.side_panel_mut().set_header_action_rect(action, hit);
            }
            let hovered = mouse.is_some_and(|(mx, my)| {
                mx >= rect[0] && mx < rect[0] + rect[2]
                    && my >= rect[1] && my < rect[1] + rect[3]
            });
            let selected = pane.side_panel().selected_header_action() == Some(action)
                && pane.side_panel().is_focused();
            if hovered || selected {
                sugarloaf.rounded_rect(
                    None, rect[0], rect[1], rect[2], rect[3],
                    sidebar_background_opacity(theme.f32_alpha(
                        theme.surface, if hovered { 0.78 } else { 0.55 },
                    )),
                    DEPTH, radius, ORDER_PANEL + 2,
                );
            }
            draw_catalog_icon(
                sugarloaf,
                icon,
                icon_x,
                icon_y,
                s,
                theme,
                clip,
                occlusion_rects,
            );
            let opts = DrawOpts {
                font_size: label_size,
                color: theme.u8(theme.fg),
                clip_rect: intersect_rect(
                    [label_x, y, (rect[2] - 54.0 * s).max(0.0), rect[3]],
                    clip,
                ),
                ..DrawOpts::default()
            };
            if opts.clip_rect.is_some() {
                draw_text_with_occlusion(
                    sugarloaf,
                    label_x,
                    label_y,
                    title,
                    &opts,
                    occlusion_rects,
                );
            }
            if selected {
                let cursor_w = (FONT_SIZE * s * 0.6).max(2.0);
                let cursor_h = (rect[3] - 8.0 * s).max(FONT_SIZE * s).min(rect[3]);
                pane.side_panel_mut().set_selected_cursor_rect([
                    rect[0] + s,
                    rect[1] + (rect[3] - cursor_h) / 2.0,
                    cursor_w,
                    cursor_h,
                ]);
            }
            if action == CatalogHeaderAction::Projects {
                draw_catalog_icon(
                    sugarloaf,
                    CatalogIcon::Chevron,
                    rect[0] + rect[2] - 22.0 * s,
                    icon_y,
                    s,
                    theme,
                    clip,
                    occlusion_rects,
                );
            }
            y += button_h;
        }
        super::super::draw::draw_rect_clipped(
            sugarloaf,
            [text_x, y + 4.0 * s, text_w, s],
            theme.f32_alpha(theme.muted, 0.22),
            ORDER_PANEL + 2,
            clip,
        );
        y += 12.0 * s;
    } else {
        // Legacy hosts still route text input to the inline session query.
        let rect = [text_x, y, text_w, 32.0 * s];
        if let Some(hit) = intersect_rect(rect, clip) {
            pane.side_panel_mut().set_session_search_rect(hit);
        }
        let query = pane.side_panel().session_query();
        let label = if query.is_empty() { "Search" } else { query };
        let opts = DrawOpts {
            font_size: FONT_SIZE * s,
            color: theme.u8(if query.is_empty() {
                theme.muted
            } else {
                theme.fg
            }),
            clip_rect: intersect_rect(rect, clip),
            ..DrawOpts::default()
        };
        if opts.clip_rect.is_some() {
            draw_text_with_occlusion(
                sugarloaf,
                text_x + 7.0 * s,
                y + 9.0 * s,
                label,
                &opts,
                occlusion_rects,
            );
        }
        y += 40.0 * s;
    }
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
    let current_id = pane
        .side_panel()
        .catalog_viewed_session(pane.session_id_str())
        .map(str::to_owned);
    let highlighted = pane
        .side_panel()
        .catalog_highlight_index(pane.session_id_str());
    let list_bottom = list_rect[1] + list_rect[3];

    let sessions_len = pane.side_panel().sessions().len();
    if let Some(selected) = highlighted {
        let row_ix = selected as isize - render_top as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h - frac
            + if focused { cursor_offset } else { 0.0 };
        let row_bottom = row_y + row_h;
        let visible_y = row_y.max(list_rect[1]);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h > 0.0 {
            let bg_color =
                sidebar_background_opacity(theme.f32_alpha(theme.surface, 0.55));
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
    // Date groups retain the original extruded P2 date-heading style.
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

    // Keep activity in a trailing slot; titles start at the normal left inset.
    let dot_diameter = 7.0 * s;
    let title_x = text_x;
    let (title_budget, status_center_x) = conversation_row_columns(text_x, text_w, s);

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
        let excerpt_w = (title_budget - 10.0 * s).max(char_w);
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
    let mut running_indicator_visible = false;
    let sessions = pane.side_panel().sessions();
    for absolute_ix in start..end {
        let entry = &sessions[absolute_ix];
        let row_ix = absolute_ix as isize - render_top as isize;
        let row_y = list_rect[1] + row_ix as f32 * row_h - frac;
        let row_bottom = row_y + row_h;
        // Draw before row culling: a below-viewport child can still have a
        // visible stem reaching up through its parent or preceding sibling.
        if let Some(top) = branch_connector_top(sessions, absolute_ix, row_y, row_h, s) {
            let root_depth = sessions
                .iter()
                .find(|row| Some(row.id.as_str()) == entry.branch_root_id.as_deref())
                .map_or(0, |root| root.depth);
            let child_x =
                title_x + branch_row_indent(entry, root_depth, s).min(title_budget);
            let continues = sessions[absolute_ix + 1..]
                .iter()
                .take_while(|row| {
                    row.branch_root_id == entry.branch_root_id && row.depth >= entry.depth
                })
                .any(|row| row.depth == entry.depth);
            let geometry = branch_connector_geometry(
                child_x,
                row_y,
                top,
                continues,
                s,
                sugarloaf.scale_factor(),
            );
            let color = theme.f32_alpha(theme.fg, 1.0);
            for clip in crate::primitives::visible_rect_fragments(
                geometry.curve_bounds,
                list_rect,
                occlusion_rects,
            ) {
                sugarloaf.ring_clipped(
                    geometry.center,
                    geometry.radius,
                    geometry.stroke,
                    color,
                    DEPTH,
                    ORDER_PANEL + 2,
                    clip,
                );
            }
            for rect in geometry.straight {
                for [x, y, w, h] in crate::primitives::visible_rect_fragments(
                    rect,
                    list_rect,
                    occlusion_rects,
                ) {
                    sugarloaf.quad(
                        None,
                        x,
                        y,
                        w,
                        h,
                        color,
                        [0.0; 4],
                        DEPTH,
                        ORDER_PANEL + 2,
                    );
                }
            }
        }
        let visible_y = row_y.max(list_rect[1]);
        let visible_h = row_bottom.min(list_bottom) - visible_y;
        if visible_h <= 0.0 {
            continue;
        }

        let text_y = row_y + (row_h - FONT_SIZE * s) / 2.0;

        // Paging controls are selectable catalog actions, never sessions.
        if entry.is_project_page() {
            let directory = entry.directory.as_deref().unwrap_or_default();
            if pane.side_panel().project_page_loading(directory) {
                // Static, directory-stable widths: no per-row animation owner or
                // clock, and no additional display-list entries/hit targets.
                let seed = directory.bytes().fold(0u32, |hash, byte| {
                    hash.wrapping_mul(31).wrapping_add(u32::from(byte))
                });
                let width_fraction = 0.46 + (seed % 5) as f32 * 0.05;
                let bar_h = (FONT_SIZE * 0.72 * s).min(row_h);
                let budget = title_budget;
                let bar = [
                    title_x,
                    row_y + (row_h - bar_h) * 0.5,
                    budget * width_fraction,
                    bar_h,
                ];
                if let Some(bar) = intersect_rect(bar, list_rect) {
                    // Match the existing status primitive's conservative
                    // occlusion: omit a covered bar rather than bleed through.
                    if running_square_visible(bar, list_rect, occlusion_rects) {
                        sugarloaf.quad(
                            None,
                            bar[0],
                            bar[1],
                            bar[2],
                            bar[3],
                            theme.f32_alpha(theme.muted, 0.18),
                            [bar[3] * 0.5; 4],
                            DEPTH,
                            ORDER_PANEL + 2,
                        );
                    }
                }
                continue;
            }
            let selected = pane.side_panel().selected_index() == absolute_ix
                && !pane.side_panel().search_focused()
                && !pane.side_panel().new_chat_selected()
                && pane.side_panel().selected_header_action().is_none();
            if selected || hovered_session == Some(absolute_ix) {
                sugarloaf.quad(
                    None,
                    list_rect[0],
                    visible_y,
                    list_rect[2],
                    visible_h,
                    sidebar_background_opacity(theme.f32_alpha(theme.surface, 0.28)),
                    [5.0 * s; 4],
                    DEPTH,
                    ORDER_PANEL + 1,
                );
            }
            let page_opts = project_label_opts(theme, s, list_rect);
            let label =
                truncate_sidebar_text(&entry.title, text_w, sugarloaf, &page_opts);
            draw_text_with_occlusion(
                sugarloaf,
                text_x,
                text_y,
                &label,
                &page_opts,
                occlusion_rects,
            );
            continue;
        }
        // Project folders are selectable; unknown/date captions remain labels.
        if entry.is_project() {
            let directory = entry.directory.as_deref().unwrap_or_default();
            render_project_row(
                sugarloaf,
                entry,
                pane.side_panel().project_expanded(directory),
                pane.side_panel().is_remote_project(directory),
                text_x,
                text_y,
                text_w,
                theme,
                s,
                list_rect,
                occlusion_rects,
            );
            continue;
        }
        // Pinned/Recent use monochrome P2; project rows keep their body typography.
        if entry.is_header {
            let section_opts = DrawOpts {
                color: theme.u8(theme.fg),
                font_id: pixel_font,
                ..project_label_opts(theme, s, list_rect)
            };
            let opts = if entry.id.starts_with("section:") {
                &section_opts
            } else {
                &header_opts
            };
            let indent = entry.depth as f32 * 18.0 * s;
            let label = truncate_sidebar_text(
                &entry.title,
                (text_w - indent).max(0.0),
                sugarloaf,
                opts,
            );
            draw_text_with_occlusion(
                sugarloaf,
                text_x + indent,
                text_y,
                &label,
                opts,
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
                    sidebar_background_opacity(
                        theme.f32_alpha(theme.surface, 0.28 * hover),
                    ),
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
            let excerpt_w = title_budget - 10.0 * s;
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
                        sidebar_background_opacity(theme.f32_alpha(theme.cyan, 0.16)),
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

        let is_child = entry.branch_root_id.is_some();
        let root_depth = entry
            .branch_root_id
            .as_deref()
            .and_then(|root| sessions.iter().find(|row| row.id == root))
            .map_or(0, |root| root.depth);
        let indent = branch_row_indent(entry, root_depth, s).min(title_budget);
        let title_x = title_x + indent;
        let title_budget = (title_budget - indent).max(0.0);
        let text_clip = [title_x, visible_y, title_budget, visible_h];

        let is_current = current_id.as_deref() == Some(entry.id.as_str());
        let activity_color = if is_child {
            matches!(catalog_branch_status(entry), BranchStatus::Active)
                .then_some([1.0; 3])
        } else {
            catalog_activity_color(
                entry,
                theme.f32_alpha(theme.red, 1.0)[..3].try_into().unwrap(),
                theme.f32_alpha(theme.yellow, 1.0)[..3].try_into().unwrap(),
            )
        };
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
                sidebar_background_opacity(theme.f32_alpha(theme.surface, 0.28 * hover)),
                [5.0 * s; 4],
                DEPTH,
                ORDER_PANEL + 1,
            );
        }

        if let Some(rgb) = activity_color {
            let spinner = 10.0 * s * hover_scale;
            let visible = draw_subagent_spinner(
                sugarloaf,
                status_center_x - spinner * 0.5,
                row_y + (row_h - spinner) * 0.5,
                spinner,
                rgb,
                now_seconds,
                list_rect,
                s,
                ORDER_PANEL + 2,
                occlusion_rects,
            );
            running_indicator_visible |= visible;
        } else if is_child {
            let scaled_dot = dot_diameter * hover_scale;
            let dot_y = row_y + (row_h - scaled_dot) / 2.0;
            let (color, halo) = match catalog_branch_status(entry) {
                BranchStatus::WaitingPermission => {
                    let blink = 0.55 + 0.45 * (now_seconds * 4.0).sin().abs();
                    (
                        theme.u8_alpha(theme.yellow, blink),
                        Some((theme.u8(theme.yellow), (1.0 - blink) * 0.25)),
                    )
                }
                BranchStatus::Stopped => (theme.u8(theme.red), None),
                _ => (theme.u8(theme.green), None),
            };
            draw_status_dot_text(
                sugarloaf,
                status_center_x - scaled_dot * 0.5,
                dot_y,
                scaled_dot,
                color,
                halo,
                list_rect,
                occlusion_rects,
                s,
            );
            if is_child && catalog_branch_status(entry) == BranchStatus::WaitingPermission
            {
                running_indicator_visible |= running_square_visible(
                    [
                        status_center_x - scaled_dot * 0.5,
                        dot_y,
                        scaled_dot,
                        scaled_dot,
                    ],
                    list_rect,
                    occlusion_rects,
                );
            }
        }

        // Conversation cards lead with the product identity, matching the
        // compact app/title/time hierarchy used by the rest of the sidebar.
        let source_icon = if is_child {
            entry.agent_kind.unwrap_or(agent_icon::AgentKind::Neoism)
        } else {
            agent_icon::AgentKind::Neoism
        };
        let icon_size = 13.0 * s;
        push_provider_icon_clipped(
            sugarloaf,
            source_icon,
            [title_x, row_y + 4.0 * s, icon_size, icon_size],
            text_clip,
            occlusion_rects,
        );

        let context = if is_child {
            catalog_branch_activity_label(entry).to_owned()
        } else if entry.time_label.trim().is_empty() {
            relative_session_time(entry.updated_ms, now_ms)
        } else {
            entry.time_label.clone()
        };
        let identity_opts = DrawOpts {
            font_size: FONT_SIZE * s * 0.82,
            color: theme.u8_alpha(theme.fg, 0.72),
            clip_rect: Some(text_clip),
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
            let context_right = title_x + title_budget;
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
        if is_child && is_current {
            hovered_title_opts.color = theme.u8(theme.accent);
        }
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
        .note_visible_running_indicator(running_indicator_visible);
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
            sidebar_background_opacity(theme.f32_alpha(theme.surface, 0.92)),
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
            [1.0; 3],
            now_seconds,
            list_rect,
            s,
            ORDER_PANEL + 6,
            occlusion_rects,
        );
    }
}

fn project_icon(remote: bool, open: bool) -> &'static str {
    if remote {
        "\u{f0c2}"
    } else {
        crate::panels::file_tree::icons::folder_icon(open).0
    }
}

/// Project glyphs share the file tree's icon fallback and folder color;
/// project labels use the same regular body font as chat titles.
#[allow(clippy::too_many_arguments)]
fn render_project_row(
    sugarloaf: &mut Sugarloaf,
    entry: &NeoismAgentSessionEntry,
    open: bool,
    remote: bool,
    x: f32,
    y: f32,
    width: f32,
    theme: &IdeTheme,
    s: f32,
    clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    let indent = crate::panels::file_tree::INDENT_PX * s;
    let icon_size = crate::panels::file_tree::ICON_FONT_SIZE * s;
    let icon_opts = DrawOpts {
        font_size: (indent - 6.0 * s).max(6.0 * s),
        color: theme.u8(theme.muted),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    crate::primitives::draw_icon_centered_with_occlusion(
        sugarloaf,
        x,
        [x, y, (indent - 4.0 * s).max(1.0), FONT_SIZE * s],
        if open { "\u{f078}" } else { "\u{f054}" },
        &icon_opts,
        occlusion_rects,
        true,
    );
    let project_opts = DrawOpts {
        font_size: icon_size,
        color: theme.u8(theme.folder),
        ..icon_opts
    };
    crate::primitives::draw_icon_centered_with_occlusion(
        sugarloaf,
        x + indent,
        [x + indent, y, icon_size, FONT_SIZE * s],
        project_icon(remote, open),
        &project_opts,
        occlusion_rects,
        true,
    );
    let label_opts = project_label_opts(theme, s, clip);
    let label_x = x + indent + icon_size + crate::panels::file_tree::ICON_GAP * s;
    let right = x + width;
    // Project folders deliberately have no numeric/partial-page count badge.
    let label_budget = (right - label_x).max(0.0);
    let label = truncate_sidebar_text(&entry.title, label_budget, sugarloaf, &label_opts);
    draw_text_with_occlusion(sugarloaf, label_x, y, &label, &label_opts, occlusion_rects);
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
    use super::{
        conversation_row_columns, project_icon, project_label_opts,
        relative_session_time, running_square_visible, IdeTheme, FONT_SIZE,
    };

    #[test]
    fn remote_projects_use_cloud_icons_without_changing_local_folders() {
        for open in [false, true] {
            assert_eq!(project_icon(true, open), "\u{f0c2}");
            assert_eq!(
                project_icon(false, open),
                crate::panels::file_tree::icons::folder_icon(open).0
            );
        }
    }

    #[test]
    fn activity_slot_trails_the_text_column_at_each_scale() {
        for scale in [1.0, 1.5, 2.0] {
            let left = 16.0 * scale;
            let width = 250.0 * scale;
            let (text_width, center) = conversation_row_columns(left, width, scale);
            assert_eq!(text_width, width - 18.0 * scale);
            for diameter in [7.0 * scale, 10.0 * scale * 1.045] {
                assert!(center - diameter * 0.5 > left + text_width);
                assert!(center + diameter * 0.5 < left + width);
            }
        }
        for width in [0.0, 4.0, 12.0] {
            let (text_width, center) = conversation_row_columns(16.0, width, 2.0);
            assert_eq!(text_width, 0.0);
            assert!(center >= 16.0 && center <= 16.0 + width);
        }
    }

    #[test]
    fn project_and_page_labels_share_regular_body_typography() {
        let theme = IdeTheme::default();
        let clip = [0.0, 0.0, 300.0, 500.0];
        for scale in [1.0, 1.5, 2.0] {
            let opts = project_label_opts(&theme, scale, clip);
            assert_eq!(opts.font_size, FONT_SIZE * scale);
            assert!(opts.font_id.is_none());
            assert!(!opts.bold);
            assert!(!opts.extrude);
            assert_eq!(opts.clip_rect, Some(clip));
            assert_eq!(opts.color, theme.u8(theme.readable_accent(theme.blue)));
        }
    }

    #[test]
    fn running_squares_own_only_visible_nonoccluded_ink() {
        let square = [4.0, 4.0, 2.0, 2.0];
        let clip = [0.0, 0.0, 10.0, 10.0];
        assert!(running_square_visible(square, clip, &[]));
        assert!(!running_square_visible([20.0, 4.0, 2.0, 2.0], clip, &[]));
        assert!(!running_square_visible(square, clip, &[clip]));
        assert!(!running_square_visible(
            square,
            clip,
            &[[5.0, 5.0, 1.0, 1.0]]
        ));
    }

    #[test]
    fn relative_time_uses_real_timestamp_only() {
        let now = 1_800_000_000_000;
        assert_eq!(relative_session_time(0, now), "");
        assert_eq!(relative_session_time(now - 15_000, now), "just now");
        assert_eq!(relative_session_time(now - 3_600_000, now), "1h ago");
        assert_eq!(relative_session_time(now - 172_800_000, now), "2d ago");
    }
}
