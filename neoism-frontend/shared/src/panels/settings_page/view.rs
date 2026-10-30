//! Painter for the Zed-style Settings panel — a full-screen dimmed
//! overlay with a category rail, scrollable setting rows, toggle
//! switches, and dropdown selects.

use neoism_paint::text::DrawOpts;
use crate::primitives::paint::Paint;

use crate::primitives::draw_text_with_occlusion;
use crate::primitives::geom::intersect_rect;
use crate::primitives::ide_theme::IdeTheme;

use super::state::{point_in, NeoismSettingsPane, RowControl, KEYBINDS};

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

#[derive(Debug)]
struct SettingLayout {
    labels: Vec<String>,
    details: Vec<String>,
    detail_y: f32,
    control_y: f32,
    height: f32,
}

/// Measure before painting so wrapping, scrolling, and menu anchors share geometry.
fn setting_layout(
    label: &str,
    detail: &str,
    width: f32,
    scale: f32,
    mut measure: impl FnMut(&str, f32) -> f32,
) -> SettingLayout {
    let mut wrap = |text: &str, size: f32| {
        let mut lines = Vec::new();
        if text.trim().is_empty() {
            return lines;
        }
        for paragraph in text.lines() {
            let mut line = String::new();
            for word in paragraph.split_whitespace() {
                let candidate = if line.is_empty() {
                    word.to_string()
                } else {
                    format!("{line} {word}")
                };
                if measure(&candidate, size) <= width {
                    line = candidate;
                    continue;
                }
                if !line.is_empty() {
                    lines.push(std::mem::take(&mut line));
                }
                // Paths and other unbroken tokens must also fit narrow panes.
                for ch in word.chars() {
                    let mut candidate = line.clone();
                    candidate.push(ch);
                    if !line.is_empty() && measure(&candidate, size) > width {
                        lines.push(std::mem::take(&mut line));
                    }
                    line.push(ch);
                }
            }
            lines.push(line);
        }
        lines
    };
    let labels = wrap(label, 14.0 * scale);
    let details = wrap(detail, 12.0 * scale);
    let detail_y = labels.len() as f32 * 20.0 * scale
        + if details.is_empty() { 0.0 } else { 4.0 * scale };
    let control_y = detail_y + details.len() as f32 * 18.0 * scale + 10.0 * scale;
    SettingLayout {
        labels,
        details,
        detail_y,
        control_y,
        height: control_y + 56.0 * scale,
    }
}

fn dropdown_geometry(
    anchor: [f32; 4],
    bounds: [f32; 4],
    requested_rows: usize,
    search_h: f32,
    scale: f32,
) -> [f32; 4] {
    let gap = 3.0 * scale;
    let below = anchor[1] + anchor[3] + gap;
    let below_h = (bounds[1] + bounds[3] - below).max(0.0);
    let above_h = (anchor[1] - gap - bounds[1]).max(0.0);
    let overhead = 8.0 * scale + search_h;
    let desired_h = requested_rows.max(1) as f32 * 28.0 * scale + overhead;
    let use_below = desired_h <= below_h || below_h >= above_h;
    let available = if use_below { below_h } else { above_h };
    let rows = ((available - overhead) / (28.0 * scale))
        .floor()
        .max(1.0)
        .min(requested_rows.max(1) as f32);
    let height = (rows * 28.0 * scale + overhead).min(bounds[3]);
    let width = anchor[2].max(150.0 * scale).min(bounds[2]);
    let y = if use_below {
        below
    } else {
        anchor[1] - gap - height
    };
    [
        anchor[0].clamp(bounds[0], bounds[0] + bounds[2] - width),
        y.clamp(bounds[1], bounds[1] + bounds[3] - height),
        width,
        height,
    ]
}

/// A rounded rect clipped (by intersection) to `clip`, so a partly
/// scrolled-off control is cut at the viewport edge instead of drawing
/// over the header / margins.
fn clip_rrect(
    sugarloaf: &mut dyn Paint,
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
    sugarloaf: &mut dyn Paint,
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

    let categories = pane.navigation_categories();
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
        (row_h * categories.len() as f32).min(clip[3]),
        theme.f32(theme.surface),
        DEPTH,
        12.0 * s,
        ORDER_ROW,
    );

    for (index, category) in categories.iter().copied().enumerate() {
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
        if index + 1 < categories.len() {
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
    pane.set_content_metrics(row_h * categories.len() as f32, clip[3]);
}

pub(crate) fn render(
    pane: &mut NeoismSettingsPane,
    sugarloaf: &mut dyn Paint,
    win_w: f32,
    win_h: f32,
    theme: &IdeTheme,
    scale: f32,
    mouse: Option<(f32, f32)>,
) {
    if !pane.is_active() || win_w <= 0.0 || win_h <= 0.0 {
        pane.row_ui.invalidate_presentation();
        pane.row_geometry_valid = false;
        return;
    }
    pane.row_geometry_valid = false;
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
        for cat in pane.navigation_categories() {
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
    let pill_w = (360.0 * s).min(cw);
    let pill_h = 28.0 * s;
    let visible = pane.visible_settings();
    let label_opts = DrawOpts {
        font_size: 14.0 * s,
        clip_rect: Some([cx, clip_top, cw, content_clip[3]]),
        ..base
    };
    let desc_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        ..label_opts
    };
    let details: Vec<_> = visible
        .iter()
        .map(|&idx| {
            let def = pane.row(idx);
            let mut detail = def.description.clone();
            if def.constraints.min.is_some() || def.constraints.max.is_some() {
                let range = match (def.constraints.min, def.constraints.max) {
                    (Some(min), Some(max)) => format!("{min}-{max}"),
                    (Some(min), None) => format!(">= {min}"),
                    (None, Some(max)) => format!("<= {max}"),
                    (None, None) => String::new(),
                };
                if !detail.is_empty() {
                    detail.push_str("  |  ");
                }
                detail.push_str(&range);
                if let Some(unit) = def.constraints.unit.as_deref() {
                    detail.push(' ');
                    detail.push_str(unit);
                }
            }
            detail
        })
        .collect();
    let mut layouts: Vec<_> = visible
        .iter()
        .zip(&details)
        .map(|(&idx, detail)| {
            let def = pane.row(idx);
            if def.control == RowControl::Toggle {
                // The declarative tree, not a parallel hand-written measure,
                // supplies these row heights below.
                SettingLayout {
                    labels: Vec::new(),
                    details: Vec::new(),
                    detail_y: 0.0,
                    control_y: 0.0,
                    height: 0.0,
                }
            } else {
                setting_layout(&def.label, detail, cw, s, |text, font_size| {
                    sugarloaf
                        .text_mut()
                        .measure(text, &DrawOpts { font_size, ..base })
                })
            }
        })
        .collect();
    let row_props = super::neoui::SettingsRowsProps {
        rows: visible
            .iter()
            .zip(&details)
            .zip(&layouts)
            .map(|((&idx, detail), layout)| {
                let row = pane.row(idx);
                super::neoui::RowSnapshot {
                    path: row.path.clone(),
                    label: row.label.clone(),
                    detail: detail.clone(),
                    checked: (row.control == RowControl::Toggle)
                        .then(|| pane.bool_at(idx)),
                    legacy_height: layout.height,
                }
            })
            .collect(),
        width: cw,
        scale: s,
        palette: super::neoui::RowPalette::new(theme),
        focused_key: pane.row_ui.runtime().focused_key().map(str::to_owned),
    };
    match pane
        .row_ui
        .set_root::<super::neoui::SettingsRows>("settings/rows", row_props)
    {
        Ok(handle) => pane.row_component = Some(handle),
        Err(error) => {
            tracing::error!(%error, "NeoUI settings owner reconciliation failed");
            pane.control_rects.clear();
            return;
        }
    }
    let old_scroll = pane.scroll_offset();
    let row_origin = [cx, content_top + pad - old_scroll];
    let row_clip = crate::neoui_renderer::rect(content_clip);
    pane.row_viewport = content_clip;
    let mut metrics = crate::neoui_renderer::TextMetrics { sugarloaf, base };
    if let Err(error) = pane.row_ui.present(row_origin, cw, row_clip, &mut metrics) {
        tracing::error!(%error, "NeoUI settings layout failed");
        pane.control_rects.clear();
        return;
    }
    for (&idx, layout) in visible.iter().zip(&mut layouts) {
        if pane.row(idx).control == RowControl::Toggle {
            layout.height = pane
                .row_ui
                .runtime()
                .frame()
                .bounds_for(&super::neoui::row_key(&pane.row(idx).path))
                .map_or(0.0, |bounds| bounds.height);
        }
    }
    let descriptor_h: f32 = layouts.iter().map(|layout| layout.height).sum();
    let kb_header_h = 52.0 * s;
    let keybind_layouts: Vec<_> = if pane.is_keybinds() {
        KEYBINDS
            .iter()
            .enumerate()
            .map(|(idx, def)| {
                setting_layout(
                    def.label,
                    if pane.keybind_is_rebindable(idx) {
                        def.action
                    } else {
                        ""
                    },
                    cw,
                    s,
                    |text, font_size| {
                        sugarloaf
                            .text_mut()
                            .measure(text, &DrawOpts { font_size, ..base })
                    },
                )
            })
            .collect()
    } else {
        Vec::new()
    };
    let mut content_h = descriptor_h;
    let mut last_group = None;
    for (idx, layout) in keybind_layouts.iter().enumerate() {
        let group = pane.keybind_group(idx);
        if last_group != Some(group) {
            content_h += kb_header_h;
            last_group = Some(group);
        }
        content_h += layout.height;
    }
    pane.set_content_metrics(content_h + pad * 0.5, viewport_h);
    let scroll = pane.scroll_offset();
    // Clamping after filtering/resizing may move the content. Present the same
    // retained tree at the final origin; do not render/clone the tree or rerun
    // layout just to translate and clip its paint and input geometry.
    if scroll != old_scroll {
        let mut metrics = crate::neoui_renderer::TextMetrics { sugarloaf, base };
        if let Err(error) = pane.row_ui.present(
            [cx, content_top + pad - scroll],
            cw,
            row_clip,
            &mut metrics,
        ) {
            tracing::error!(%error, "NeoUI settings layout failed after scroll clamp");
            return;
        }
    }

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
            let row_offset: f32 = layouts[..pos].iter().map(|layout| layout.height).sum();
            let anchor = [
                cx,
                content_top + pad - scroll + row_offset + layouts[pos].control_y,
                pill_w,
                pill_h,
            ];
            let search_h = if pane.open_dropdown_is_searchable() {
                30.0 * s
            } else {
                0.0
            };
            if intersect_rect(anchor, content_clip).is_some() {
                let bounds = [
                    cx,
                    clip_top + 4.0 * s,
                    cw,
                    (clip_bottom - clip_top - 8.0 * s).max(1.0),
                ];
                dropdown = Some((
                    dropdown_geometry(anchor, bounds, rows, search_h, s),
                    open_idx,
                ));
            }
        }
    }
    let row_occ: &[[f32; 4]] = match &dropdown {
        Some((rect, _)) => std::slice::from_ref(rect),
        None => &[],
    };

    pane.row_geometry_valid = true;
    crate::neoui_renderer::paint(
        sugarloaf,
        pane.row_ui.runtime().frame(),
        base,
        ORDER_CTRL,
        row_occ,
    );
    let mut ry = content_top + pad - scroll;
    for (&idx, layout) in visible.iter().zip(&layouts) {
        let setting_row_h = layout.height;
        if pane.row(idx).control == RowControl::Toggle {
            ry += setting_row_h;
            continue;
        }
        if ry + setting_row_h < clip_top || ry > clip_bottom {
            ry += setting_row_h;
            continue;
        }
        let def = pane.row(idx).clone();
        for (line, text) in layout.labels.iter().enumerate() {
            draw_text_with_occlusion(
                sugarloaf,
                cx,
                ry + line as f32 * 20.0 * s,
                text,
                &label_opts,
                row_occ,
            );
        }
        for (line, text) in layout.details.iter().enumerate() {
            draw_text_with_occlusion(
                sugarloaf,
                cx,
                ry + layout.detail_y + line as f32 * 18.0 * s,
                text,
                &desc_opts,
                row_occ,
            );
        }
        let control_y = ry + layout.control_y;

        match def.control {
            RowControl::Toggle => unreachable!("boolean rows are painted by NeoUI"),
            RowControl::Select | RowControl::FontFamily => {
                let raw = pane.string_at(idx);
                let value = if raw.is_empty() {
                    "Default".to_string()
                } else {
                    raw
                };
                let pxr = cx;
                let pyr = control_y;
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
                        .unwrap_or([0.0; 4]),
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
                if let Some(hit) =
                    intersect_rect([pxr, pyr, pill_w, pill_h], content_clip)
                {
                    pane.push_control_rect(hit, idx);
                }
            }
            RowControl::Text => {
                let pxr = cx;
                let pyr = control_y;
                let editing = pane.is_editing(idx);
                let edit = editing.then(|| pane.edit_presentation());
                let value = if let Some((text, _, _, _)) = &edit {
                    text.clone()
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
                        intersect_rect(
                            [pxr + 10.0 * s, pyr, (pill_w - 20.0 * s).max(0.0), pill_h],
                            content_clip,
                        )
                        .unwrap_or([0.0; 4]),
                    ),
                    ..base
                };
                let caret_width = edit.as_ref().map_or(0.0, |(_, _, caret, _)| {
                    sugarloaf
                        .text_mut()
                        .measure(value.get(..*caret).unwrap_or(&value), &value_opts)
                });
                let text_x = pxr + 10.0 * s
                    - if editing {
                        (caret_width - (pill_w - 24.0 * s).max(0.0)).max(0.0)
                    } else {
                        0.0
                    };
                let text_clip = value_opts.clip_rect.unwrap_or([0.0; 4]);
                if let Some((_, selection, _, _)) = &edit {
                    if !selection.is_empty() {
                        let left = sugarloaf.text_mut().measure(
                            value.get(..selection.start).unwrap_or(""),
                            &value_opts,
                        );
                        let right = sugarloaf.text_mut().measure(
                            value.get(..selection.end).unwrap_or(&value),
                            &value_opts,
                        );
                        clip_rrect(
                            sugarloaf,
                            [
                                text_x + left,
                                pyr + 4.0 * s,
                                (right - left).max(0.0),
                                pill_h - 8.0 * s,
                            ],
                            text_clip,
                            theme.f32_alpha(theme.accent, 0.28),
                            0.0,
                            ORDER_CTRL,
                        );
                    }
                }
                draw_text_with_occlusion(
                    sugarloaf,
                    text_x,
                    pyr + (pill_h - 12.0 * s) * 0.5,
                    &value,
                    &value_opts,
                    row_occ,
                );
                if let Some((_, _, _, Some(preedit))) = &edit {
                    let left = sugarloaf
                        .text_mut()
                        .measure(value.get(..preedit.start).unwrap_or(""), &value_opts);
                    let right = sugarloaf
                        .text_mut()
                        .measure(value.get(..preedit.end).unwrap_or(&value), &value_opts);
                    clip_rrect(
                        sugarloaf,
                        [
                            text_x + left,
                            pyr + pill_h - 5.0 * s,
                            (right - left).max(0.0),
                            s,
                        ],
                        text_clip,
                        theme.f32(theme.accent),
                        0.0,
                        ORDER_TEXT,
                    );
                }
                if editing {
                    clip_rrect(
                        sugarloaf,
                        [
                            text_x + caret_width,
                            pyr + 5.0 * s,
                            1.5 * s,
                            pill_h - 10.0 * s,
                        ],
                        text_clip,
                        theme.f32(theme.accent),
                        0.0,
                        ORDER_TEXT,
                    );
                }
                if let Some(hit) =
                    intersect_rect([pxr, pyr, pill_w, pill_h], content_clip)
                {
                    pane.push_control_rect(hit, idx);
                }
            }
            RowControl::Keybinding => {
                let value_opts = DrawOpts {
                    font_size: 12.0 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some([cx, clip_top, cw, content_clip[3]]),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    cx,
                    control_y + 6.0 * s,
                    "Shortcuts below",
                    &value_opts,
                    row_occ,
                );
            }
            RowControl::Action { button, .. } => {
                let bw = (130.0 * s).min(cw);
                let bh = 28.0 * s;
                let bx = cx;
                let by = control_y;
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
                if let Some(hit) = intersect_rect([bx, by, bw, bh], content_clip) {
                    pane.push_control_rect(hit, idx);
                }
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

    // ── Keybinds list (below its canonical descriptor row). Click a
    //    shortcut pill to capture a new chord; the reset glyph clears an
    //    override. ──
    if pane.is_keybinds() {
        let header_h = kb_header_h;
        let capturing = pane.capturing();
        // Section titles render in the bundled Press Start 2P pixel face
        // (same treatment as the notes / agent-panel section headers).
        let pixel_font = crate::primitives::pixel_font_id(sugarloaf);
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
            let layout = &keybind_layouts[idx];
            let kb_row_h = layout.height;
            if ky + kb_row_h < clip_top || ky > clip_bottom {
                ky += kb_row_h;
                continue;
            }
            let rebindable = pane.keybind_is_rebindable(idx);
            for (line, text) in layout.labels.iter().enumerate() {
                draw_text_with_occlusion(
                    sugarloaf,
                    cx,
                    ky + line as f32 * 20.0 * s,
                    text,
                    &label_opts,
                    occ,
                );
            }
            for (line, text) in layout.details.iter().enumerate() {
                draw_text_with_occlusion(
                    sugarloaf,
                    cx,
                    ky + layout.detail_y + line as f32 * 18.0 * s,
                    text,
                    &desc_opts,
                    occ,
                );
            }
            let control_y = ky + layout.control_y;

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
                let pw = pw.min((cw - reset_sz - reset_gap).max(1.0));
                let pill_x = cx;
                let pill_y = control_y;
                let combo_opts = DrawOpts {
                    clip_rect: Some(
                        intersect_rect([pill_x, pill_y, pw, pill_h], content_clip)
                            .unwrap_or([0.0; 4]),
                    ),
                    ..combo_opts
                };
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
                    pill_x + ((pw - combo_w) * 0.5).max(8.0 * s),
                    pill_y + (pill_h - 12.5 * s) * 0.5,
                    &combo,
                    &combo_opts,
                    occ,
                );
                if let Some(hit) =
                    intersect_rect([pill_x, pill_y, pw, pill_h], content_clip)
                {
                    pane.push_keybind_rect(hit, idx);
                }
                if has_ov {
                    // Small rounded "reset to default" button, only shown
                    // when this shortcut has been changed from its default.
                    let rx = pill_x + pw + reset_gap;
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
                    if let Some(hit) =
                        intersect_rect([rx, rry, reset_sz, reset_sz], content_clip)
                    {
                        pane.push_keybind_reset_rect(hit, idx);
                    }
                }
            } else {
                // Reference row: a plain dim combo, no button / capture.
                let combo_opts = DrawOpts {
                    font_size: 12.5 * s,
                    color: theme.u8(theme.dim),
                    clip_rect: Some([cx, clip_top, cw, content_clip[3]]),
                    ..base
                };
                draw_text_with_occlusion(
                    sugarloaf,
                    cx,
                    control_y + 6.0 * s,
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
                    clip_rect: Some([qx, sb[1], (sb[2] - 30.0 * s).max(0.0), sb[3]]),
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
                    clip_rect: Some([qx, sb[1], (sb[2] - 30.0 * s).max(0.0), sb[3]]),
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
        let rows = (((dd_h - dd_pad * 2.0 - search_h) / opt_h).round().max(1.0) as usize)
            .min(options.len().max(1));
        let max_dscroll = options.len().saturating_sub(rows);
        pane.clamp_dropdown_scroll(max_dscroll);
        let dscroll = pane.dropdown_scroll();
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

    fn measure(text: &str, size: f32) -> f32 {
        text.chars().count() as f32 * size * 0.6
    }

    fn boolean_row(description: &str) -> super::super::state::SettingRow {
        super::super::state::SettingRow {
            category: super::super::state::Category::General,
            path: "general.toggle".into(),
            label: "Automatically animate the splash screen".into(),
            description: description.into(),
            value_kind: neoism_protocol::config::ConfigValueKind::Boolean,
            default: serde_json::Value::Bool(false),
            suggestions: Vec::new(),
            options: Vec::new(),
            constraints: Default::default(),
            extensible: false,
            control: RowControl::Toggle,
        }
    }

    #[test]
    fn neoui_boolean_rows_match_existing_wrapped_geometry_at_all_widths_and_scales() {
        let theme = IdeTheme::default();
        for scale in [0.75, 1.0, 2.0] {
            for width in [180.0, 340.0, 900.0] {
                for description in ["", "Description.",
                    "Automatically play the splash animation when the terminal is idle. This should wrap at compact widths.",
                    "\n\nFirst paragraph.\n\nSecond paragraph.\n\n",
                    "/home/somebody/a-very-long-directory-name/background.png\nSecond line."]
                {
                    let row = boolean_row(description);
                    let width = width * scale;
                    let old = setting_layout(&row.label, description, width, scale, measure);
                    for checked in [false, true] {
                        let root = super::super::neoui::toggle_row(
                            &row, description, checked, false, width, scale, &theme,
                        );
                        let mut runtime = neoui::Runtime::new();
                        let frame = runtime.layout(root, [5.0, 10.0], width,
                            neoui::Rect::new(0.0, 0.0, 4000.0, 4000.0),
                            &mut |text: &str, style: &neoui::TextStyle| measure(text, style.size),
                        ).unwrap();
                        assert!((frame.bounds.height - old.height).abs() < 0.01,
                            "width={width}, scale={scale}, description={description:?}: {} != {}", frame.bounds.height, old.height);
                        let toggle = frame.bounds_for(&super::super::neoui::toggle_key(&row.path)).unwrap();
                        assert!((toggle.y - (10.0 + old.control_y + 3.0 * scale)).abs() < 0.01);
                        let knob = frame.bounds_for("setting:general.toggle/toggle/knob").unwrap();
                        let expected_x = if checked { toggle.right() - 19.0 * scale } else { toggle.x + 3.0 * scale };
                        assert!((knob.x - expected_x).abs() < 0.01);
                        assert!((knob.y - (toggle.y + 3.0 * scale)).abs() < 0.01);
                        assert_eq!(frame.semantics.iter().find(|node| node.key == super::super::neoui::toggle_key(&row.path))
                            .unwrap().semantics.checked, Some(checked));
                    }
                }
            }
        }
    }

    #[test]
    fn neoui_mixed_rows_share_offsets_and_content_extent() {
        let row = boolean_row("A wrapped description for the declarative control.");
        let theme = IdeTheme::default();
        let tree = neoui::Element::column(
            "settings/root",
            [
                neoui::Element::spacer("legacy-before").height(neoui::Size::Px(123.0)),
                super::super::neoui::toggle_row(
                    &row,
                    &row.description,
                    false,
                    false,
                    180.0,
                    1.0,
                    &theme,
                ),
                neoui::Element::spacer("legacy-after").height(neoui::Size::Px(97.0)),
            ],
        );
        let mut runtime = neoui::Runtime::new();
        let frame = runtime
            .layout(
                tree,
                [20.0, -40.0],
                180.0,
                neoui::Rect::new(20.0, 0.0, 180.0, 600.0),
                &mut |text: &str, style: &neoui::TextStyle| measure(text, style.size),
            )
            .unwrap();
        let boolean = frame
            .bounds_for(&super::super::neoui::row_key(&row.path))
            .unwrap();
        let after = frame.bounds_for("legacy-after").unwrap();
        assert_eq!(boolean.y, -40.0 + 123.0);
        assert_eq!(after.y, boolean.bottom());
        assert!((frame.bounds.height - (123.0 + boolean.height + 97.0)).abs() < 0.01);
    }

    #[test]
    fn controls_follow_wrapped_descriptions_at_every_width_and_scale() {
        for scale in [0.75, 1.0, 2.0] {
            for width in [180.0, 340.0, 900.0] {
                let width = width * scale;
                let layout = setting_layout(
                    "Background image opacity",
                    "Image alpha multiplier from 0 (transparent) to 1 (opaque). Independent of window and panel opacity. | 0-1",
                    width, scale, measure,
                );
                for line in &layout.labels {
                    assert!(measure(line, 14.0 * scale) <= width);
                }
                for line in &layout.details {
                    assert!(measure(line, 12.0 * scale) <= width);
                }
                let detail_bottom =
                    layout.detail_y + layout.details.len() as f32 * 18.0 * scale;
                assert!(layout.control_y >= detail_bottom + 10.0 * scale - 0.001);
                assert!(layout.height >= layout.control_y + 28.0 * scale + 24.0 * scale);
            }
        }
    }

    #[test]
    fn unbroken_tokens_and_explicit_lines_stay_inside_the_row() {
        let detail = "/home/somebody/a-very-long-directory-name/background.png\n\u{65e5}\u{672c}\u{8a9e}\u{306e}\u{8aac}\u{660e}";
        let layout = setting_layout("Path", detail, 120.0, 1.0, measure);
        assert!(layout.details.len() > 2);
        assert!(layout
            .details
            .iter()
            .all(|line| measure(line, 12.0) <= 120.0));
        assert_eq!(layout.details.concat(), detail.replace('\n', ""));
    }

    #[test]
    fn empty_descriptions_do_not_reserve_a_phantom_line() {
        let empty = setting_layout("Setting", "", 300.0, 1.0, measure);
        let described = setting_layout("Setting", "Description.", 300.0, 1.0, measure);
        assert!(empty.details.is_empty());
        assert_eq!(empty.control_y, 30.0);
        assert_eq!(described.control_y - empty.control_y, 22.0);
    }

    #[test]
    fn wrapped_rows_expand_scroll_extent_and_menu_anchor() {
        let wide = setting_layout(
            "Setting",
            "A long description that wraps onto several lines.",
            900.0,
            1.0,
            measure,
        );
        let narrow = setting_layout(
            "Setting",
            "A long description that wraps onto several lines.",
            140.0,
            1.0,
            measure,
        );
        assert!(narrow.height > wide.height);
        let anchor_y = wide.height + narrow.control_y;
        let menu = dropdown_geometry(
            [20.0, anchor_y, 140.0, 28.0],
            [20.0, 0.0, 300.0, 700.0],
            3,
            0.0,
            1.0,
        );
        assert_eq!(menu[1], anchor_y + 28.0 + 3.0);
    }

    #[test]
    fn dropdowns_flip_shrink_and_stay_within_the_content_bounds() {
        let bounds = [20.0, 100.0, 260.0, 300.0];
        for search_h in [0.0, 30.0] {
            for anchor_y in [100.0, 240.0, 365.0] {
                let menu = dropdown_geometry(
                    [200.0, anchor_y, 180.0, 28.0],
                    bounds,
                    10,
                    search_h,
                    1.0,
                );
                assert!(menu[0] >= bounds[0]);
                assert!(menu[1] >= bounds[1]);
                assert!(menu[0] + menu[2] <= bounds[0] + bounds[2]);
                assert!(menu[1] + menu[3] <= bounds[1] + bounds[3]);
                assert!(menu[3] < 10.0 * 28.0 + 8.0 + search_h);
                if anchor_y == 365.0 {
                    assert!(menu[1] + menu[3] < anchor_y);
                }
            }
        }
    }

    #[test]
    fn variable_height_content_keeps_last_row_reachable_and_clamps_after_resize() {
        let short = setting_layout("Setting", "Short.", 600.0, 1.0, measure);
        let wrapped = setting_layout(
            "Setting",
            "A much longer description that needs multiple lines.",
            140.0,
            1.0,
            measure,
        );
        let content_h = short.height + wrapped.height;
        let viewport_h = 100.0;
        let mut pane = NeoismSettingsPane::new();
        pane.open();
        pane.set_content_metrics(content_h, viewport_h);
        pane.scroll_by(-10000.0);
        assert_eq!(pane.scroll_offset(), content_h - viewport_h);
        pane.set_content_metrics(short.height * 2.0, viewport_h);
        assert_eq!(pane.scroll_offset(), short.height * 2.0 - viewport_h);
        pane.set_content_metrics(short.height, 900.0);
        assert_eq!(pane.scroll_offset(), 0.0);
    }

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
