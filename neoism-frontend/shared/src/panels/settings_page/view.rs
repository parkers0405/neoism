//! Painter for the Zed-style Settings panel — a full-screen dimmed
//! overlay with a category rail, scrollable setting rows, toggle
//! switches, and dropdown selects.

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::draw_text_with_occlusion;
use crate::primitives::geom::intersect_rect;
use crate::primitives::ide_theme::IdeTheme;

use super::state::{point_in, Category, NeoismSettingsPane, RowControl, KEYBINDS};

const DEPTH: f32 = 0.0;
const ORDER_SCRIM: u8 = 40;
const ORDER_PANEL: u8 = 41;
const ORDER_ROW: u8 = 42;
const ORDER_CTRL: u8 = 43;
const ORDER_TEXT: u8 = 44;
const ORDER_DROPDOWN_BG: u8 = 45;

const SEARCH_GLYPH: &str = "\u{f002}";
const OPEN_FILE_GLYPH: &str = "\u{f15c}"; // file-lines — "open the raw settings.json"
const CHEVRON_GLYPH: &str = "\u{f078}";
const CLOSE_GLYPH: &str = "\u{f00d}";
const CHECK_GLYPH: &str = "\u{f00c}";
const BACK_GLYPH: &str = "\u{f053}";
const FORWARD_GLYPH: &str = "\u{f054}";

const COMPACT_BREAKPOINT: f32 = 720.0;

#[derive(Debug, Clone, Copy, PartialEq)]
struct SettingsGeometry {
    compact: bool,
    panel: [f32; 4],
    header_h: f32,
    sidebar_w: f32,
}

fn settings_geometry(
    win_w: f32,
    win_h: f32,
    scale: f32,
    safe: [f32; 4],
) -> SettingsGeometry {
    let s = scale.clamp(0.75, 2.0);
    let [top, right, bottom, left] = safe;
    let available_w = (win_w - left - right).max(1.0);
    let available_h = (win_h - top - bottom).max(1.0);
    let compact = available_w < COMPACT_BREAKPOINT * s || available_h < 430.0 * s;
    let margin = if compact { 0.0 } else { 40.0 * s };
    SettingsGeometry {
        compact,
        panel: [
            left + margin,
            top + margin,
            (available_w - margin * 2.0).max(1.0),
            (available_h - margin * 2.0).max(1.0),
        ],
        header_h: if compact { 102.0 * s } else { 54.0 * s },
        sidebar_w: if compact { 0.0 } else { 200.0 * s },
    }
}

/// A rounded rect clipped (by intersection) to `clip`, so a partly
/// scrolled-off control is cut at the viewport edge instead of drawing
/// over the header / margins.
fn clip_rrect(
    sugarloaf: &mut Sugarloaf,
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
    radius: f32,
    order: u8,
) {
    if let Some(r) = intersect_rect(rect, clip) {
        let radius = radius.min(r[2] * 0.5).min(r[3] * 0.5).max(0.0);
        sugarloaf.rounded_rect(None, r[0], r[1], r[2], r[3], color, DEPTH, radius, order);
    }
}

/// Compact settings is a navigation root, not a squeezed desktop rail. It
/// intentionally registers only category rows (plus the close affordance), so
/// stale detail controls cannot remain clickable after a push/pop or resize.
fn render_compact_root(
    pane: &mut NeoismSettingsPane,
    sugarloaf: &mut Sugarloaf,
    theme: &IdeTheme,
    s: f32,
    mouse: Option<(f32, f32)>,
    base: DrawOpts,
) {
    let [px, py, pw, ph] = pane.panel_rect;
    pane.back_rect = [0.0; 4];
    pane.search_rect = [0.0; 4];
    pane.edit_json_rect = [0.0; 4];
    pane.dropdown_search_rect = [0.0; 4];
    pane.control_rects.clear();
    pane.dropdown_rects.clear();
    pane.keybind_rects.clear();
    pane.keybind_reset_rects.clear();
    pane.category_rects.clear();

    let header_h = 64.0 * s;
    let pad = 16.0 * s;
    let title_opts = DrawOpts {
        font_size: 22.0 * s,
        bold: true,
        ..base
    };
    draw_text_with_occlusion(
        sugarloaf,
        px + pad,
        py + 21.0 * s,
        "Settings",
        &title_opts,
        &[],
    );

    let close_sz = 44.0 * s;
    pane.close_rect = [px + pw - pad - close_sz, py + 10.0 * s, close_sz, close_sz];
    let close_hovered = mouse.is_some_and(|(x, y)| point_in(pane.close_rect, x, y));
    let close_opts = DrawOpts {
        font_size: 16.0 * s,
        color: theme.u8(if close_hovered { theme.fg } else { theme.dim }),
        ..base
    };
    draw_text_with_occlusion(
        sugarloaf,
        pane.close_rect[0] + 15.0 * s,
        pane.close_rect[1] + 14.0 * s,
        CLOSE_GLYPH,
        &close_opts,
        &[],
    );

    let row_h = 54.0 * s;
    let list_x = px + pad;
    let list_w = (pw - pad * 2.0).max(1.0);
    let list_top = py + header_h + 8.0 * s;
    let clip = [px, list_top, pw, (py + ph - list_top).max(0.0)];
    let scroll = pane.scroll_offset();
    sugarloaf.rounded_rect(
        None,
        list_x,
        list_top,
        list_w,
        (row_h * Category::ALL.len() as f32).min(clip[3]),
        theme.f32(theme.surface),
        DEPTH,
        12.0 * s,
        ORDER_ROW,
    );

    for (index, category) in Category::ALL.into_iter().enumerate() {
        let y = list_top + index as f32 * row_h - scroll;
        let rect = [list_x, y, list_w, row_h];
        let Some(hit_rect) = intersect_rect(rect, clip) else {
            continue;
        };
        let hovered = mouse.is_some_and(|(x, y)| point_in(hit_rect, x, y));
        if hovered {
            clip_rrect(
                sugarloaf,
                rect,
                clip,
                theme.f32(theme.hover),
                8.0 * s,
                ORDER_CTRL,
            );
        }
        let icon_bg = [list_x + 12.0 * s, y + 11.0 * s, 32.0 * s, 32.0 * s];
        clip_rrect(
            sugarloaf,
            icon_bg,
            clip,
            theme.f32_alpha(theme.accent, 0.18),
            8.0 * s,
            ORDER_CTRL,
        );
        let icon_opts = DrawOpts {
            font_size: 14.0 * s,
            color: theme.u8(theme.accent),
            clip_rect: Some(clip),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            icon_bg[0] + 9.0 * s,
            y + 20.0 * s,
            category.icon(),
            &icon_opts,
            &[],
        );
        let label_opts = DrawOpts {
            font_size: 16.0 * s,
            clip_rect: Some(clip),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            list_x + 56.0 * s,
            y + 19.0 * s,
            category.label(),
            &label_opts,
            &[],
        );
        let chevron_opts = DrawOpts {
            font_size: 11.0 * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(clip),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            list_x + list_w - 24.0 * s,
            y + 21.0 * s,
            FORWARD_GLYPH,
            &chevron_opts,
            &[],
        );
        if index + 1 < Category::ALL.len() {
            if let Some(separator) = intersect_rect(
                [list_x + 56.0 * s, y + row_h - 1.0, list_w - 56.0 * s, 1.0],
                clip,
            ) {
                sugarloaf.rect(
                    None,
                    separator[0],
                    separator[1],
                    separator[2],
                    separator[3],
                    theme.f32_alpha(theme.border, 0.55),
                    DEPTH,
                    ORDER_ROW,
                );
            }
        }
        pane.category_rects.push((hit_rect, category));
    }
    pane.set_content_metrics(row_h * Category::ALL.len() as f32, clip[3]);
}

pub(crate) fn render(
    pane: &mut NeoismSettingsPane,
    sugarloaf: &mut Sugarloaf,
    win_w: f32,
    win_h: f32,
    theme: &IdeTheme,
    scale: f32,
    mouse: Option<(f32, f32)>,
) {
    if !pane.is_active() || win_w <= 0.0 || win_h <= 0.0 {
        return;
    }
    let s = scale.clamp(0.75, 2.0);
    let occ: &[[f32; 4]] = &[];

    sugarloaf.rect(
        None,
        0.0,
        0.0,
        win_w,
        win_h,
        theme.f32_alpha(theme.black, 0.45),
        DEPTH,
        ORDER_SCRIM,
    );

    let geometry = settings_geometry(win_w, win_h, s, pane.safe_area_insets());
    let compact = geometry.compact;
    pane.set_compact_layout(compact);
    let [px, py, pw, ph] = geometry.panel;
    pane.panel_rect = [px, py, pw, ph];
    sugarloaf.rounded_rect(
        None,
        px,
        py,
        pw,
        ph,
        theme.f32(theme.panel_bg()),
        DEPTH,
        if compact { 0.0 } else { 12.0 * s },
        ORDER_PANEL,
    );

    let base = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.fg),
        bold: false,
        italic: false,
        extrude: false,
        font_id: None,
        clip_rect: Some(pane.panel_rect),
    };

    let pad = if compact { 16.0 * s } else { 20.0 * s };
    let header_h = geometry.header_h;
    let content_top = py + header_h;
    let sidebar_w = geometry.sidebar_w;

    if pane.compact_root() {
        render_compact_root(pane, sugarloaf, theme, s, mouse, base);
        return;
    }

    // ── Header ──
    let title_opts = DrawOpts {
        font_size: 18.0 * s,
        bold: true,
        ..base
    };
    let title = if compact {
        pane.current_category().label()
    } else {
        "Settings"
    };
    let title_w = sugarloaf.text_mut().measure(title, &title_opts);
    draw_text_with_occlusion(
        sugarloaf,
        if compact {
            px + (pw - title_w) * 0.5
        } else {
            px + pad
        },
        py + if compact { 18.0 * s } else { pad },
        title,
        &title_opts,
        occ,
    );

    if compact {
        pane.back_rect = [px, py, (112.0 * s).min(pw * 0.42), 54.0 * s];
        let back_opts = DrawOpts {
            font_size: 13.5 * s,
            color: theme.u8(theme.accent),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            px + 16.0 * s,
            py + 20.0 * s,
            BACK_GLYPH,
            &back_opts,
            occ,
        );
        draw_text_with_occlusion(
            sugarloaf,
            px + 34.0 * s,
            py + 19.0 * s,
            "Settings",
            &back_opts,
            occ,
        );
    } else {
        pane.back_rect = [0.0; 4];
    }

    let ctrl_h = 30.0 * s;
    let ctrl_y = if compact {
        py + 60.0 * s
    } else {
        py + (header_h - ctrl_h) * 0.5
    };
    let close_sz = 26.0 * s;
    let close_x = px + pw - pad - close_sz;
    pane.close_rect = if compact {
        [0.0; 4]
    } else {
        [
            close_x,
            py + (header_h - close_sz) * 0.5,
            close_sz,
            close_sz,
        ]
    };

    let json_sz = ctrl_h;
    let search_w = if compact {
        (pw - pad * 2.0 - json_sz - 8.0 * s).max(80.0 * s)
    } else {
        260.0 * s
    };
    let search_x = if compact {
        px + pad
    } else {
        close_x - 12.0 * s - json_sz - 12.0 * s - search_w
    };
    let json_x = if compact {
        search_x + search_w + 8.0 * s
    } else {
        close_x - 12.0 * s - json_sz
    };
    pane.edit_json_rect = [json_x, ctrl_y, json_sz, json_sz];
    let json_hovered =
        mouse.is_some_and(|(mx, my)| point_in(pane.edit_json_rect, mx, my));
    sugarloaf.rounded_rect(
        None,
        json_x,
        ctrl_y,
        json_sz,
        json_sz,
        theme.f32(if json_hovered {
            theme.hover
        } else {
            theme.surface
        }),
        DEPTH,
        6.0 * s,
        ORDER_ROW,
    );
    let json_opts = DrawOpts {
        font_size: 14.0 * s,
        color: theme.u8(if json_hovered { theme.fg } else { theme.dim }),
        ..base
    };
    draw_text_with_occlusion(
        sugarloaf,
        json_x + json_sz * 0.5 - 6.0 * s,
        ctrl_y + (json_sz - 14.0 * s) * 0.5,
        OPEN_FILE_GLYPH,
        &json_opts,
        occ,
    );

    pane.search_rect = [search_x, ctrl_y, search_w, ctrl_h];
    let sb_focused = pane.is_search_focused();
    let sb_hovered = mouse.is_some_and(|(mx, my)| point_in(pane.search_rect, mx, my));
    sugarloaf.rounded_rect(
        None,
        search_x,
        ctrl_y,
        search_w,
        ctrl_h,
        theme.f32(if sb_focused || sb_hovered {
            theme.hover
        } else {
            theme.surface
        }),
        DEPTH,
        6.0 * s,
        ORDER_ROW,
    );
    let glyph_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        ..base
    };
    draw_text_with_occlusion(
        sugarloaf,
        search_x + 10.0 * s,
        ctrl_y + (ctrl_h - 12.0 * s) * 0.5,
        SEARCH_GLYPH,
        &glyph_opts,
        occ,
    );
    let text_x = search_x + 30.0 * s;
    let text_y = ctrl_y + (ctrl_h - 12.0 * s) * 0.5;
    if pane.search_query().is_empty() {
        let ph_opts = DrawOpts {
            font_size: 12.5 * s,
            color: theme.u8(theme.dim),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            text_x,
            text_y,
            "Search settings\u{2026}",
            &ph_opts,
            occ,
        );
    } else {
        let q_opts = DrawOpts {
            font_size: 12.5 * s,
            ..base
        };
        let query = pane.search_query().to_string();
        draw_text_with_occlusion(sugarloaf, text_x, text_y, &query, &q_opts, occ);
        if sb_focused {
            let caret_w = (1.5 * s).max(1.0);
            let query_w = sugarloaf.text_mut().measure(&query, &q_opts);
            sugarloaf.rect(
                None,
                (text_x + query_w).min(search_x + search_w - caret_w - 4.0 * s),
                ctrl_y + 6.0 * s,
                caret_w,
                ctrl_h - 12.0 * s,
                theme.f32(theme.accent),
                DEPTH,
                ORDER_TEXT,
            );
        }
    }

    if !compact {
        let close_hovered =
            mouse.is_some_and(|(mx, my)| point_in(pane.close_rect, mx, my));
        let close_opts = DrawOpts {
            font_size: 15.0 * s,
            color: theme.u8(if close_hovered { theme.fg } else { theme.dim }),
            ..base
        };
        draw_text_with_occlusion(
            sugarloaf,
            close_x + close_sz * 0.5 - 5.0 * s,
            pane.close_rect[1] + (close_sz - 15.0 * s) * 0.5,
            CLOSE_GLYPH,
            &close_opts,
            occ,
        );
    }

    sugarloaf.rect(
        None,
        px,
        content_top,
        pw,
        1.0 * s,
        theme.f32(theme.border),
        DEPTH,
        ORDER_PANEL,
    );

    // ── Category rail ──
    pane.category_rects.clear();
    let cat_row_h = 34.0 * s;
    let mut cy = content_top + pad;
    let searching = !pane.search_is_empty();
    if !compact {
        for cat in Category::ALL {
            let rect = [px + 10.0 * s, cy, sidebar_w - 20.0 * s, cat_row_h];
            let selected = !searching && cat == pane.current_category();
            let hovered = mouse.is_some_and(|(mx, my)| point_in(rect, mx, my));
            if selected {
                sugarloaf.rounded_rect(
                    None,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    theme.f32_alpha(theme.accent, 0.18),
                    DEPTH,
                    6.0 * s,
                    ORDER_ROW,
                );
            } else if hovered {
                sugarloaf.rounded_rect(
                    None,
                    rect[0],
                    rect[1],
                    rect[2],
                    rect[3],
                    theme.f32(theme.hover),
                    DEPTH,
                    6.0 * s,
                    ORDER_ROW,
                );
            }
            let icon_opts = DrawOpts {
                font_size: 13.0 * s,
                color: theme.u8(if selected { theme.accent } else { theme.dim }),
                ..base
            };
            draw_text_with_occlusion(
                sugarloaf,
                rect[0] + 12.0 * s,
                rect[1] + (cat_row_h - 13.0 * s) * 0.5,
                cat.icon(),
                &icon_opts,
                occ,
            );
            let label_opts = DrawOpts {
                font_size: 13.5 * s,
                color: theme.u8(if selected { theme.fg } else { theme.dim }),
                bold: selected,
                ..base
            };
            draw_text_with_occlusion(
                sugarloaf,
                rect[0] + 36.0 * s,
                rect[1] + (cat_row_h - 13.0 * s) * 0.5,
                cat.label(),
                &label_opts,
                occ,
            );
            pane.category_rects.push((rect, cat));
            cy += cat_row_h + 2.0 * s;
        }
    }

    let div_x = px + sidebar_w;
    if !compact {
        sugarloaf.rect(
            None,
            div_x,
            content_top,
            1.0 * s,
            ph - header_h,
            theme.f32(theme.border),
            DEPTH,
            ORDER_PANEL,
        );
    }

    // ── Setting rows (scrollable, partial-clipped to the viewport) ──
    pane.control_rects.clear();
    pane.dropdown_rects.clear();
    pane.keybind_rects.clear();
    pane.keybind_reset_rects.clear();
    let cx = if compact { px + pad } else { div_x + pad };
    let cw = (px + pw - pad - cx).max(1.0);
    // Content viewport — text AND rects are clipped to this so a partly
    // scrolled row is cut at the edge (matching the rest of the app),
    // never drawn over the header or bottom margin.
    let clip_top = content_top;
    let clip_bottom = py + ph - 8.0 * s;
    let content_clip = [
        div_x,
        clip_top,
        pw - sidebar_w,
        (clip_bottom - clip_top).max(0.0),
    ];
    let viewport_h = (clip_bottom - (content_top + pad * 0.5)).max(0.0);
    let setting_row_h = 66.0 * s;
    let pill_w = if compact {
        (cw * 0.46).clamp(92.0 * s, 150.0 * s)
    } else {
        180.0 * s
    };
    let pill_h = 28.0 * s;
    let visible = pane.visible_settings();
    let scroll = pane.scroll_offset();

    // Pre-compute the open dropdown's popover rect so the row text under
    // it can be occluded (an opaque popover can't hide text on its own —
    // text renders in a later pass than rects).
    let mut dropdown: Option<([f32; 4], usize)> = None;
    if let Some(open_idx) = pane.open_dropdown() {
        let is_menu = matches!(
            pane.row(open_idx).control,
            RowControl::Select | RowControl::FontFamily
        );
        if let Some(pos) = visible
            .iter()
            .position(|&i| i == open_idx)
            .filter(|_| is_menu)
        {
            let opt_count = pane.dropdown_options(open_idx).len();
            let rows = opt_count.min(pane.dropdown_visible_rows()).max(1);
            let row_ry = content_top + pad - scroll + pos as f32 * setting_row_h;
            let pxr = cx + cw - pill_w;
            let pyr = row_ry - 2.0 * s;
            let opt_h = 28.0 * s;
            let dd_pad = 4.0 * s;
            // Long dynamic catalogs reserve an auto-focused search box.
            let search_h = if pane.open_dropdown_is_searchable() {
                30.0 * s
            } else {
                0.0
            };
            let dd_w = pill_w.max(150.0 * s);
            let dd_h = rows as f32 * opt_h + dd_pad * 2.0 + search_h;
            let below = pyr + pill_h + 3.0 * s;
            let dd_y = if below + dd_h > py + ph {
                (pyr - dd_h - 3.0 * s).max(py + 4.0 * s)
            } else {
                below
            };
            dropdown = Some(([pxr, dd_y, dd_w, dd_h], open_idx));
        }
    }
    let row_occ: &[[f32; 4]] = match &dropdown {
        Some((rect, _)) => std::slice::from_ref(rect),
        None => &[],
    };

    let mut ry = content_top + pad - scroll;
    for &idx in &visible {
        // Cheap cull only for rows far outside; near-edge rows are drawn
        // and clipped so they scroll off smoothly.
        if ry + setting_row_h < clip_top - setting_row_h
            || ry > clip_bottom + setting_row_h
        {
            ry += setting_row_h;
            continue;
        }
        let def = pane.row(idx).clone();
        let label_opts = DrawOpts {
            font_size: 14.0 * s,
            clip_rect: Some(content_clip),
            ..base
        };
        draw_text_with_occlusion(sugarloaf, cx, ry, &def.label, &label_opts, row_occ);
        let desc_opts = DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.dim),
            clip_rect: Some(content_clip),
            ..base
        };
        let mut detail = def.description.clone();
        if def.constraints.min.is_some() || def.constraints.max.is_some() {
            let range = match (def.constraints.min, def.constraints.max) {
                (Some(min), Some(max)) => format!("{min}-{max}"),
                (Some(min), None) => format!(">= {min}"),
                (None, Some(max)) => format!("<= {max}"),
                (None, None) => String::new(),
            };
            detail.push_str("  ·  ");
            detail.push_str(&range);
            if let Some(unit) = def.constraints.unit.as_deref() {
                detail.push(' ');
                detail.push_str(unit);
            }
        }
        draw_text_with_occlusion(
            sugarloaf,
            cx,
            ry + 20.0 * s,
            &detail,
            &desc_opts,
            row_occ,
        );

        match def.control {
            RowControl::Toggle => {
                let on = pane.bool_at(idx);
                let tw = 40.0 * s;
                let th = 22.0 * s;
                let tx = cx + cw - tw;
                let ty = ry + 2.0 * s;
                clip_rrect(
                    sugarloaf,
                    [tx, ty, tw, th],
                    content_clip,
                    theme.f32(if on { theme.accent } else { theme.border }),
                    th * 0.5,
                    ORDER_CTRL,
                );
                let knob = th - 6.0 * s;
                let kx = if on {
                    tx + tw - knob - 3.0 * s
                } else {
                    tx + 3.0 * s
                };
                clip_rrect(
                    sugarloaf,
                    [kx, ty + 3.0 * s, knob, knob],
                    content_clip,
                    theme.f32(theme.bg),
                    knob * 0.5,
                    ORDER_TEXT,
                );
                pane.push_control_rect([tx, ty, tw, th], idx);
            }
            RowControl::Select | RowControl::FontFamily => {
                let raw = pane.string_at(idx);
                let value = if raw.is_empty() {
                    "Default".to_string()
                } else {
                    raw
                };
                let pxr = cx + cw - pill_w;
                let pyr = ry - 2.0 * s;
                let hovered =
                    pane.hover_control == Some(idx) || pane.open_dropdown() == Some(idx);
                clip_rrect(
                    sugarloaf,
                    [pxr, pyr, pill_w, pill_h],
                    content_clip,
                    theme.f32(if hovered { theme.hover } else { theme.surface }),
                    6.0 * s,
                    ORDER_CTRL,
                );
                let val_opts = DrawOpts {
                    font_size: 12.5 * s,
                    clip_rect: Some(
                        intersect_rect(
                            [pxr, pyr, pill_w - 22.0 * s, pill_h],
                            content_clip,
                        )
                        .unwrap_or(content_clip),
                    ),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    pxr + 12.0 * s,
                    pyr + (pill_h - 12.0 * s) * 0.5,
                    &value,
                    &val_opts,
                    row_occ,
                );
                let chev_opts = DrawOpts {
                    font_size: 9.0 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(content_clip),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    pxr + pill_w - 18.0 * s,
                    pyr + (pill_h - 9.0 * s) * 0.5,
                    CHEVRON_GLYPH,
                    &chev_opts,
                    row_occ,
                );
                pane.push_control_rect([pxr, pyr, pill_w, pill_h], idx);
            }
            RowControl::Text => {
                let pxr = cx + cw - pill_w;
                let pyr = ry - 2.0 * s;
                let editing = pane.is_editing(idx);
                let value = if editing {
                    pane.edit_buffer().to_string()
                } else {
                    let current = pane.string_at(idx);
                    if current.is_empty() {
                        "Default".to_string()
                    } else {
                        current
                    }
                };
                clip_rrect(
                    sugarloaf,
                    [pxr, pyr, pill_w, pill_h],
                    content_clip,
                    theme.f32(if editing || pane.hover_control == Some(idx) {
                        theme.hover
                    } else {
                        theme.surface
                    }),
                    6.0 * s,
                    ORDER_CTRL,
                );
                let value_opts = DrawOpts {
                    font_size: 12.5 * s,
                    clip_rect: Some(
                        intersect_rect([pxr, pyr, pill_w, pill_h], content_clip)
                            .unwrap_or(content_clip),
                    ),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    pxr + 10.0 * s,
                    pyr + (pill_h - 12.0 * s) * 0.5,
                    &value,
                    &value_opts,
                    row_occ,
                );
                if editing {
                    let width = sugarloaf.text_mut().measure(&value, &value_opts);
                    sugarloaf.rect(
                        None,
                        (pxr + 10.0 * s + width).min(pxr + pill_w - 5.0 * s),
                        pyr + 5.0 * s,
                        1.5 * s,
                        pill_h - 10.0 * s,
                        theme.f32(theme.accent),
                        DEPTH,
                        ORDER_TEXT,
                    );
                }
                pane.push_control_rect([pxr, pyr, pill_w, pill_h], idx);
            }
            RowControl::Keybinding => {
                let value_opts = DrawOpts {
                    font_size: 12.0 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(content_clip),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    cx + cw - 140.0 * s,
                    ry + 4.0 * s,
                    "Shortcuts below",
                    &value_opts,
                    row_occ,
                );
            }
            RowControl::Action { button, .. } => {
                let bw = 130.0 * s;
                let bh = 28.0 * s;
                let bx = cx + cw - bw;
                let by = ry - 2.0 * s;
                let hovered = pane.hover_control == Some(idx);
                clip_rrect(
                    sugarloaf,
                    [bx, by, bw, bh],
                    content_clip,
                    theme.f32_alpha(theme.accent, if hovered { 0.30 } else { 0.16 }),
                    6.0 * s,
                    ORDER_CTRL,
                );
                let b_opts = DrawOpts {
                    font_size: 12.5 * s,
                    color: theme.u8(theme.accent),
                    clip_rect: Some(content_clip),
                    ..base
                };
                let bwm = sugarloaf.text_mut().measure(button, &b_opts);
                draw_text_with_occlusion(
                    sugarloaf,
                    bx + (bw - bwm) * 0.5,
                    by + (bh - 12.0 * s) * 0.5,
                    button,
                    &b_opts,
                    row_occ,
                );
                pane.push_control_rect([bx, by, bw, bh], idx);
            }
        }

        // Row divider.
        if let Some(sep) = intersect_rect(
            [cx, ry + setting_row_h - 12.0 * s, cw, 1.0 * s],
            content_clip,
        ) {
            sugarloaf.rect(
                None,
                sep[0],
                sep[1],
                sep[2],
                sep[3],
                theme.f32_alpha(theme.border, 0.5),
                DEPTH,
                ORDER_ROW,
            );
        }
        ry += setting_row_h;
    }
    // The Keybinds list owns its own content metrics below; setting this
    // here (with an empty `visible`) would clamp scroll to 0 every frame.
    if !pane.is_keybinds() {
        pane.set_content_metrics(visible.len() as f32 * setting_row_h, viewport_h);
    }

    // ── Keybinds list (below its canonical descriptor row). Click a
    //    shortcut pill to capture a new chord; the reset glyph clears an
    //    override. ──
    if pane.is_keybinds() {
        let kb_row_h = 46.0 * s;
        let header_h = 52.0 * s;
        let capturing = pane.capturing();
        // Section titles render in the bundled Press Start 2P pixel face
        // (same treatment as the notes / agent-panel section headers).
        let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
        let descriptor_h = visible.len() as f32 * setting_row_h;
        let start_y = content_top + pad - scroll + descriptor_h;
        let mut ky = start_y;
        let mut last_group: Option<super::state::KeyGroup> = None;
        for idx in 0..KEYBINDS.len() {
            // Group header whenever the surface changes.
            let group = pane.keybind_group(idx);
            if last_group != Some(group) {
                last_group = Some(group);
                if ky + header_h >= clip_top && ky <= clip_bottom + header_h {
                    // Real header size. The pixel face draws wider per
                    // point, so it runs a touch smaller than the UI-font
                    // fallback but still reads as a bold section header.
                    let h_size = if pixel_font.is_some() {
                        15.0 * s
                    } else {
                        18.0 * s
                    };
                    let h_opts = DrawOpts {
                        font_size: h_size,
                        bold: true,
                        color: theme.u8(theme.accent),
                        font_id: pixel_font,
                        clip_rect: Some(content_clip),
                        ..base
                    };
                    draw_text_with_occlusion(
                        sugarloaf,
                        cx,
                        ky + header_h - 26.0 * s,
                        group.title(),
                        &h_opts,
                        occ,
                    );
                    if let Some(sep) = intersect_rect(
                        [cx, ky + header_h - 6.0 * s, cw, 1.0 * s],
                        content_clip,
                    ) {
                        sugarloaf.rect(
                            None,
                            sep[0],
                            sep[1],
                            sep[2],
                            sep[3],
                            theme.f32_alpha(theme.border, 0.6),
                            DEPTH,
                            ORDER_ROW,
                        );
                    }
                }
                ky += header_h;
            }
            if ky + kb_row_h < clip_top - kb_row_h || ky > clip_bottom + kb_row_h {
                ky += kb_row_h;
                continue;
            }
            let def = KEYBINDS[idx];
            let rebindable = pane.keybind_is_rebindable(idx);
            let label_opts = DrawOpts {
                font_size: 13.5 * s,
                clip_rect: Some(content_clip),
                ..base
            };
            let label_y = if rebindable {
                ky + 2.0 * s
            } else {
                ky + kb_row_h * 0.5 - 7.0 * s
            };
            draw_text_with_occlusion(sugarloaf, cx, label_y, def.label, &label_opts, occ);
            if rebindable {
                let sub_opts = DrawOpts {
                    font_size: 10.5 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(content_clip),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    cx,
                    ky + 21.0 * s,
                    def.action,
                    &sub_opts,
                    occ,
                );
            }

            let is_capturing = capturing == Some(idx);
            let combo = if is_capturing {
                "Press keys\u{2026}".to_string()
            } else {
                pane.keybind_display(idx)
            };

            if rebindable {
                let combo_opts = DrawOpts {
                    font_size: 12.5 * s,
                    color: theme.u8(if is_capturing { theme.accent } else { theme.fg }),
                    clip_rect: Some(content_clip),
                    ..base
                };
                let combo_w = sugarloaf.text_mut().measure(&combo, &combo_opts);
                let pw = (combo_w + 26.0 * s).max(72.0 * s);
                let has_ov = pane.keybind_has_override(idx);
                let reset_sz = if has_ov { 22.0 * s } else { 0.0 };
                let reset_gap = if has_ov { 8.0 * s } else { 0.0 };
                let pill_x = cx + cw - pw - reset_sz - reset_gap;
                let pill_y = ky - 1.0 * s;
                let hovered = mouse.is_some_and(|(mx, my)| {
                    point_in([pill_x, pill_y, pw, pill_h], mx, my)
                });
                let pill_bg = if is_capturing {
                    theme.f32_alpha(theme.accent, 0.18)
                } else if hovered {
                    theme.f32(theme.hover)
                } else {
                    theme.f32(theme.surface)
                };
                clip_rrect(
                    sugarloaf,
                    [pill_x, pill_y, pw, pill_h],
                    content_clip,
                    pill_bg,
                    6.0 * s,
                    ORDER_CTRL,
                );
                draw_text_with_occlusion(
                    sugarloaf,
                    pill_x + (pw - combo_w) * 0.5,
                    pill_y + (pill_h - 12.5 * s) * 0.5,
                    &combo,
                    &combo_opts,
                    occ,
                );
                pane.push_keybind_rect([pill_x, pill_y, pw, pill_h], idx);
                if has_ov {
                    // Small rounded "reset to default" button, only shown
                    // when this shortcut has been changed from its default.
                    let rx = cx + cw - reset_sz;
                    let rry = pill_y + (pill_h - reset_sz) * 0.5;
                    let rhov = mouse.is_some_and(|(mx, my)| {
                        point_in([rx, rry, reset_sz, reset_sz], mx, my)
                    });
                    let rbg = if rhov {
                        theme.f32_alpha(theme.accent, 0.22)
                    } else {
                        theme.f32(theme.surface)
                    };
                    clip_rrect(
                        sugarloaf,
                        [rx, rry, reset_sz, reset_sz],
                        content_clip,
                        rbg,
                        reset_sz * 0.5,
                        ORDER_CTRL,
                    );
                    let ro = DrawOpts {
                        font_size: 10.0 * s,
                        color: theme.u8(if rhov { theme.accent } else { theme.dim }),
                        clip_rect: Some(content_clip),
                        ..base
                    };
                    draw_text_with_occlusion(
                        sugarloaf,
                        rx + (reset_sz - 9.0 * s) * 0.5,
                        rry + (reset_sz - 10.0 * s) * 0.5,
                        "\u{f0e2}",
                        &ro,
                        occ,
                    );
                    pane.push_keybind_reset_rect([rx, rry, reset_sz, reset_sz], idx);
                }
            } else {
                // Reference row: a plain dim combo, no button / capture.
                let combo_opts = DrawOpts {
                    font_size: 12.5 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some(content_clip),
                    ..base
                };
                let combo_w = sugarloaf.text_mut().measure(&combo, &combo_opts);
                draw_text_with_occlusion(
                    sugarloaf,
                    cx + cw - combo_w - 6.0 * s,
                    ky + kb_row_h * 0.5 - 6.0 * s,
                    &combo,
                    &combo_opts,
                    occ,
                );
            }

            if let Some(sep) =
                intersect_rect([cx, ky + kb_row_h - 10.0 * s, cw, 1.0 * s], content_clip)
            {
                sugarloaf.rect(
                    None,
                    sep[0],
                    sep[1],
                    sep[2],
                    sep[3],
                    theme.f32_alpha(theme.border, 0.35),
                    DEPTH,
                    ORDER_ROW,
                );
            }
            ky += kb_row_h;
        }
        pane.set_content_metrics(descriptor_h + ky - start_y, viewport_h);
    }

    // ── Dropdown popover (drawn last, on top; row text under it is
    //    already occluded via `row_occ`). ──
    if let Some((dd_rect, idx)) = dropdown {
        let [dd_x, dd_y, dd_w, dd_h] = dd_rect;
        let opt_h = 28.0 * s;
        let dd_pad = 4.0 * s;
        sugarloaf.rounded_rect(
            None,
            dd_x,
            dd_y,
            dd_w,
            dd_h,
            theme.f32(theme.surface),
            DEPTH,
            6.0 * s,
            ORDER_DROPDOWN_BG,
        );

        // Searchable catalog: typing immediately narrows fonts/themes/etc.
        let is_searchable = pane.open_dropdown_is_searchable();
        let search_h = if is_searchable { 30.0 * s } else { 0.0 };
        if is_searchable {
            let sb = [dd_x + dd_pad, dd_y + dd_pad, dd_w - dd_pad * 2.0, 26.0 * s];
            pane.dropdown_search_rect = sb;
            sugarloaf.rounded_rect(
                None,
                sb[0],
                sb[1],
                sb[2],
                sb[3],
                theme.f32(theme.hover),
                DEPTH,
                5.0 * s,
                ORDER_DROPDOWN_BG,
            );
            let sg = DrawOpts {
                font_size: 11.0 * s,
                color: theme.u8(theme.dim),
                ..base
            };
            draw_text_with_occlusion(
                sugarloaf,
                sb[0] + 8.0 * s,
                sb[1] + (sb[3] - 11.0 * s) * 0.5,
                SEARCH_GLYPH,
                &sg,
                occ,
            );
            let q = pane.dropdown_search_query().to_string();
            let qx = sb[0] + 26.0 * s;
            let qy = sb[1] + (sb[3] - 11.5 * s) * 0.5;
            if q.is_empty() {
                let ph = DrawOpts {
                    font_size: 11.5 * s,
                    color: theme.u8(theme.dim),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    qx,
                    qy,
                    if pane.row(idx).control == RowControl::FontFamily {
                        "Search fonts\u{2026}"
                    } else {
                        "Filter options\u{2026}"
                    },
                    &ph,
                    occ,
                );
            } else {
                let qo = DrawOpts {
                    font_size: 11.5 * s,
                    color: theme.u8(theme.fg),
                    ..base
                };
                draw_text_with_occlusion(sugarloaf, qx, qy, &q, &qo, occ);
                let caret_w = (1.5 * s).max(1.0);
                let qw = sugarloaf.text_mut().measure(&q, &qo);
                sugarloaf.rect(
                    None,
                    (qx + qw).min(sb[0] + sb[2] - caret_w - 4.0 * s),
                    sb[1] + 5.0 * s,
                    caret_w,
                    sb[3] - 10.0 * s,
                    theme.f32(theme.accent),
                    DEPTH,
                    ORDER_TEXT,
                );
            }
        } else {
            pane.dropdown_search_rect = [0.0; 4];
        }

        let options = pane.dropdown_options(idx);
        if is_searchable && options.is_empty() {
            let nm = DrawOpts {
                font_size: 12.0 * s,
                color: theme.u8(theme.dim),
                ..base
            };
            draw_text_with_occlusion(
                sugarloaf,
                dd_x + 14.0 * s,
                dd_y + dd_pad + search_h + (opt_h - 12.0 * s) * 0.5,
                "No matching options",
                &nm,
                occ,
            );
        }
        let rows = options.len().min(pane.dropdown_visible_rows()).max(1);
        let max_dscroll = options.len().saturating_sub(rows);
        let dscroll = pane.dropdown_scroll().min(max_dscroll);
        let current = pane.string_at(idx);
        let mut oy = dd_y + dd_pad + search_h;
        for opt in options.iter().skip(dscroll).take(rows) {
            let orect = [dd_x, oy, dd_w, opt_h];
            let hovered = mouse.is_some_and(|(mx, my)| point_in(orect, mx, my));
            if hovered {
                sugarloaf.rounded_rect(
                    None,
                    dd_x + 3.0 * s,
                    oy,
                    dd_w - 6.0 * s,
                    opt_h,
                    theme.f32(theme.hover),
                    DEPTH,
                    4.0 * s,
                    ORDER_DROPDOWN_BG,
                );
            }
            let is_current = *opt == current;
            if is_current {
                let check_opts = DrawOpts {
                    font_size: 10.0 * s,
                    color: theme.u8(theme.accent),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    dd_x + 10.0 * s,
                    oy + (opt_h - 10.0 * s) * 0.5,
                    CHECK_GLYPH,
                    &check_opts,
                    occ,
                );
            }
            let opt_opts = DrawOpts {
                font_size: 12.5 * s,
                color: theme.u8(if is_current { theme.fg } else { theme.dim }),
                clip_rect: Some([dd_x, oy, dd_w - 6.0 * s, opt_h]),
                ..base
            };
            draw_text_with_occlusion(
                sugarloaf,
                dd_x + 26.0 * s,
                oy + (opt_h - 12.0 * s) * 0.5,
                opt,
                &opt_opts,
                occ,
            );
            pane.push_dropdown_rect(orect, idx, opt.clone());
            oy += opt_h;
        }
        // Scroll affordance when the list is longer than the popover.
        if max_dscroll > 0 {
            let hint = DrawOpts {
                font_size: 9.0 * s,
                color: theme.u8(theme.dim),
                ..base
            };
            if dscroll < max_dscroll {
                draw_text_with_occlusion(
                    sugarloaf,
                    dd_x + dd_w - 15.0 * s,
                    dd_y + dd_h - 13.0 * s,
                    "\u{f078}",
                    &hint,
                    occ,
                );
            }
            if dscroll > 0 {
                draw_text_with_occlusion(
                    sugarloaf,
                    dd_x + dd_w - 15.0 * s,
                    dd_y + 4.0 * s,
                    "\u{f077}",
                    &hint,
                    occ,
                );
            }
        }
    }
}

#[cfg(test)]
mod geometry_tests {
    use super::*;

    #[test]
    fn iphone_geometry_is_full_width_and_uses_pushed_navigation() {
        let geometry = settings_geometry(390.0, 844.0, 1.0, [0.0; 4]);
        assert!(geometry.compact);
        assert_eq!(geometry.panel, [0.0, 0.0, 390.0, 844.0]);
        assert_eq!(geometry.sidebar_w, 0.0);
        assert_eq!(geometry.header_h, 102.0);
    }

    #[test]
    fn desktop_geometry_preserves_the_rail_card() {
        let geometry = settings_geometry(1440.0, 900.0, 1.0, [0.0; 4]);
        assert!(!geometry.compact);
        assert_eq!(geometry.panel, [40.0, 40.0, 1360.0, 820.0]);
        assert_eq!(geometry.sidebar_w, 200.0);
        assert_eq!(geometry.header_h, 54.0);
    }
}
