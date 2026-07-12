use serde::Deserialize;
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::ide_theme::IdeTheme;

const HEIGHT: f32 = 336.0;
const MIN_WIDTH: f32 = 260.0;
const PAD: f32 = 18.0;
const RADIUS: f32 = 18.0;
const CHART_TOP: f32 = 170.0;
const CHART_H: f32 = 84.0;
const CHART_TOP_NO_RANGE: f32 = 142.0;
const CHART_H_NO_RANGE: f32 = 112.0;
const MAX_POINTS: usize = 180;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockCardSpec {
    #[serde(default = "default_version")]
    pub version: u32,
    pub symbol: String,
    #[serde(default)]
    pub name: String,
    pub price: f64,
    #[serde(default = "default_currency")]
    pub currency: String,
    #[serde(default)]
    pub change: Option<f64>,
    #[serde(default)]
    pub change_percent: Option<f64>,
    #[serde(default)]
    pub period: Option<String>,
    #[serde(default)]
    pub range: Option<String>,
    #[serde(default)]
    pub points: Vec<StockPoint>,
    #[serde(default)]
    pub stats: StockStats,
}

#[derive(Clone, Debug, Deserialize)]
pub struct StockPoint {
    #[serde(default)]
    pub t: String,
    pub v: f64,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StockStats {
    pub open: Option<String>,
    pub day_low: Option<String>,
    pub day_high: Option<String>,
    pub volume: Option<String>,
    pub year_low: Option<String>,
    pub year_high: Option<String>,
    pub market_cap: Option<String>,
    pub eps_ttm: Option<String>,
    pub pe_ratio: Option<String>,
}

fn default_version() -> u32 {
    1
}

fn default_currency() -> String {
    "USD".to_string()
}

pub fn parse_stock_card(source: &str) -> Result<StockCardSpec, serde_json::Error> {
    serde_json::from_str(source)
}

pub fn measure_stock_card(_spec: &StockCardSpec, _width: f32, scale: f32) -> f32 {
    HEIGHT * scale
}

#[allow(clippy::too_many_arguments)]
pub fn render_stock_card(
    sugarloaf: &mut Sugarloaf,
    spec: &StockCardSpec,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    theme: &IdeTheme,
    scale: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
    depth: f32,
    order: u8,
) {
    if !rects_intersect([x, y, width, height], viewport_clip) {
        return;
    }

    let card_rect = [x, y, width.max(MIN_WIDTH * scale), height];
    let Some(base_clip) = intersect_rect(card_rect, viewport_clip) else {
        return;
    };
    let mut clips = vec![base_clip];
    for occlusion in occlusion_rects {
        let mut next = Vec::new();
        for clip in clips {
            next.extend(subtract_rect(clip, *occlusion));
        }
        clips = next;
        if clips.is_empty() {
            return;
        }
    }

    for clip in clips {
        render_stock_card_clipped(
            sugarloaf,
            spec,
            x,
            y,
            width,
            height,
            theme,
            scale,
            now_seconds,
            mouse,
            clip,
            depth,
            order,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_stock_card_clipped(
    sugarloaf: &mut Sugarloaf,
    spec: &StockCardSpec,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    theme: &IdeTheme,
    scale: f32,
    now_seconds: f32,
    mouse: Option<(f32, f32)>,
    viewport_clip: [f32; 4],
    depth: f32,
    order: u8,
) {
    let width = width.max(MIN_WIDTH * scale);
    let pad = PAD * scale;
    let radius = RADIUS * scale;
    let green = theme.f32(theme.green);
    let red = theme.f32(theme.red);
    let positive = spec.change.unwrap_or(0.0) >= 0.0;
    let accent = if positive { green } else { red };
    let hovered = mouse.is_some_and(|(mx, my)| {
        mx >= x && mx <= x + width && my >= y && my <= y + height
    });
    let pulse =
        ((now_seconds * 2.2) + symbol_phase(spec.symbol.as_str())).sin() * 0.5 + 0.5;
    let border_alpha = if hovered { 0.95 } else { 0.54 + 0.18 * pulse };
    let glow_alpha = if hovered { 0.18 } else { 0.07 + 0.05 * pulse };

    draw_rounded_rect_clipped(
        sugarloaf,
        viewport_clip,
        None,
        x,
        y,
        width,
        height,
        theme.f32_alpha(theme.bg, if hovered { 1.0 } else { 0.96 }),
        depth,
        radius,
        order,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        viewport_clip,
        None,
        x,
        y,
        width,
        height,
        with_alpha(accent, border_alpha),
        depth,
        radius,
        order + 1,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        viewport_clip,
        None,
        x + 1.0 * scale,
        y + 1.0 * scale,
        width - 2.0 * scale,
        height - 2.0 * scale,
        theme.f32_alpha(theme.bg, 0.98),
        depth,
        (radius - 1.0 * scale).max(0.0),
        order + 2,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        viewport_clip,
        None,
        x + 3.0 * scale,
        y + 3.0 * scale,
        width - 6.0 * scale,
        height - 6.0 * scale,
        with_alpha(accent, glow_alpha),
        depth,
        (radius - 3.0 * scale).max(0.0),
        order + 3,
    );

    let clip =
        intersect_rect([x, y, width, height], viewport_clip).unwrap_or(viewport_clip);
    let title_opts = DrawOpts {
        font_size: 13.0 * scale,
        color: theme.u8_alpha(theme.white, 0.74),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let price_opts = DrawOpts {
        font_size: 28.0 * scale,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let change_opts = DrawOpts {
        font_size: 13.5 * scale,
        color: if positive {
            theme.u8(theme.green)
        } else {
            theme.u8(theme.red)
        },
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let label_opts = DrawOpts {
        font_size: 12.5 * scale,
        color: theme.u8_alpha(theme.white, 0.68),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let value_opts = DrawOpts {
        font_size: 12.5 * scale,
        color: theme.u8(theme.fg),
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };

    let title = if spec.name.trim().is_empty() {
        spec.symbol.as_str().to_string()
    } else {
        format!("{} ({})", spec.name.trim(), spec.symbol.trim())
    };
    sugarloaf
        .text_mut()
        .draw(x + pad, y + 19.0 * scale, title.as_str(), &title_opts);
    sugarloaf.text_mut().draw(
        x + pad,
        y + 47.0 * scale,
        format_price(spec.price, spec.currency.as_str()).as_str(),
        &price_opts,
    );
    sugarloaf.text_mut().draw(
        x + pad,
        y + 88.0 * scale,
        format_change(spec).as_str(),
        &change_opts,
    );

    let divider_y = y + 124.0 * scale;
    draw_line_clipped(
        sugarloaf,
        clip,
        x + pad,
        divider_y,
        x + width - pad,
        divider_y,
        1.0 * scale,
        depth,
        theme.f32_alpha(theme.border, 0.55),
    );
    let has_range = spec
        .range
        .as_deref()
        .is_some_and(|range| !range.trim().is_empty());
    if has_range {
        render_range_tabs(
            sugarloaf,
            spec.range.as_deref().unwrap_or_default(),
            x + pad,
            y + 138.0 * scale,
            width - 2.0 * pad,
            mouse,
            theme,
            scale,
            clip,
            depth,
            order + 3,
        );
    }
    let chart_top = if has_range {
        CHART_TOP
    } else {
        CHART_TOP_NO_RANGE
    };
    let chart_h = if has_range { CHART_H } else { CHART_H_NO_RANGE };
    render_chart(
        sugarloaf,
        spec,
        [
            x + pad,
            y + chart_top * scale,
            width - 2.0 * pad,
            chart_h * scale,
        ],
        accent,
        theme,
        scale,
        now_seconds,
        hovered,
        mouse,
        depth,
    );
    render_stats(
        sugarloaf,
        spec,
        x + pad,
        y + 276.0 * scale,
        width - 2.0 * pad,
        scale,
        &label_opts,
        &value_opts,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_range_tabs(
    sugarloaf: &mut Sugarloaf,
    selected: &str,
    x: f32,
    y: f32,
    width: f32,
    mouse: Option<(f32, f32)>,
    theme: &IdeTheme,
    scale: f32,
    clip: [f32; 4],
    depth: f32,
    order: u8,
) {
    let ranges = [selected.trim()];
    let visible_width = width.min(82.0 * scale).max(44.0 * scale);
    let cell_w = visible_width / ranges.len() as f32;
    let selected_ix = ranges
        .iter()
        .position(|range| range.eq_ignore_ascii_case(selected.trim()))
        .unwrap_or(0);
    let chip_h = 23.0 * scale;
    let chip_w = (cell_w - 7.0 * scale).max(26.0 * scale);
    let hovered_ix = mouse.and_then(|(mx, my)| {
        (my >= y - 7.0 * scale && my <= y + chip_h).then(|| {
            let ix = ((mx - x) / cell_w).floor() as isize;
            (ix >= 0 && ix < ranges.len() as isize).then_some(ix as usize)
        })?
    });
    let chip_x = x + selected_ix as f32 * cell_w + (cell_w - chip_w) * 0.5;
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        None,
        chip_x,
        y - 5.0 * scale,
        chip_w,
        chip_h,
        theme.f32_alpha(theme.surface, 0.72),
        depth,
        11.5 * scale,
        order,
    );
    if let Some(ix) = hovered_ix.filter(|ix| *ix != selected_ix) {
        let hover_x = x + ix as f32 * cell_w + (cell_w - chip_w) * 0.5;
        draw_rounded_rect_clipped(
            sugarloaf,
            clip,
            None,
            hover_x,
            y - 5.0 * scale,
            chip_w,
            chip_h,
            theme.f32_alpha(theme.surface, 0.38),
            depth,
            11.5 * scale,
            order,
        );
    }
    let opts = DrawOpts {
        font_size: 12.0 * scale,
        color: theme.u8_alpha(theme.white, 0.58),
        bold: true,
        clip_rect: Some(clip),
        ..DrawOpts::default()
    };
    let selected_opts = DrawOpts {
        color: theme.u8(theme.fg),
        ..opts
    };
    for (ix, range) in ranges.iter().enumerate() {
        let opts = if ix == selected_ix {
            &selected_opts
        } else {
            &opts
        };
        let text_w = sugarloaf.text_mut().measure(range, opts);
        sugarloaf.text_mut().draw(
            x + ix as f32 * cell_w + (cell_w - text_w) * 0.5,
            y,
            range,
            opts,
        );
    }
}

fn render_chart(
    sugarloaf: &mut Sugarloaf,
    spec: &StockCardSpec,
    rect: [f32; 4],
    accent: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    now_seconds: f32,
    hovered: bool,
    mouse: Option<(f32, f32)>,
    depth: f32,
) {
    let [x, y, w, h] = rect;
    let values = sampled_values(spec.points.as_slice());
    let values = if values.len() >= 2 {
        values
    } else {
        vec![spec.price, spec.price]
    };
    let (mut min_v, mut max_v) = (f64::INFINITY, f64::NEG_INFINITY);
    for value in &values {
        min_v = min_v.min(*value);
        max_v = max_v.max(*value);
    }
    if !min_v.is_finite() || !max_v.is_finite() {
        return;
    }
    if (max_v - min_v).abs() < f64::EPSILON {
        min_v -= 1.0;
        max_v += 1.0;
    }

    for i in 0..4 {
        let gy = y + i as f32 * h / 3.0;
        draw_line_clipped(
            sugarloaf,
            rect,
            x,
            gy,
            x + w,
            gy,
            1.0 * scale,
            depth,
            theme.f32_alpha(theme.border, 0.24),
        );
    }

    let mut points = Vec::with_capacity(values.len());
    for (ix, value) in values.iter().enumerate() {
        let tx = if values.len() <= 1 {
            0.0
        } else {
            ix as f32 / (values.len() - 1) as f32
        };
        let ty = ((*value - min_v) / (max_v - min_v)) as f32;
        points.push((x + tx * w, y + h - ty * h));
    }

    let base_y = y + h;
    let mut fill = accent;
    fill[3] = if hovered { 0.2 } else { 0.12 };
    let line_w = if hovered { 2.6 } else { 2.0 } * scale;
    for pair in points.windows(2) {
        let (x1, y1) = pair[0];
        let (x2, y2) = pair[1];
        let left = x1.min(x2);
        let right = x1.max(x2);
        let top = y1.min(y2);
        let bottom = base_y;
        if right > left && bottom > top {
            draw_rect_clipped(
                sugarloaf,
                rect,
                None,
                left,
                top,
                right - left,
                bottom - top,
                fill,
                depth,
                1,
            );
        }
        draw_line_clipped(sugarloaf, rect, x1, y1, x2, y2, line_w, depth, accent);
    }
    if let Some((mx, my)) = mouse {
        if mx >= x && mx <= x + w && my >= y && my <= y + h {
            render_chart_hover(
                sugarloaf, spec, &values, min_v, max_v, rect, mx, theme, scale, accent,
                depth,
            );
        }
    }
    draw_chart_marker(
        sugarloaf,
        &points,
        rect,
        accent,
        now_seconds,
        hovered,
        scale,
        depth,
    );
}

fn draw_chart_marker(
    sugarloaf: &mut Sugarloaf,
    points: &[(f32, f32)],
    clip: [f32; 4],
    accent: [f32; 4],
    now_seconds: f32,
    hovered: bool,
    scale: f32,
    depth: f32,
) {
    if points.is_empty() {
        return;
    }
    let progress = if hovered {
        1.0
    } else {
        (now_seconds * 0.22).fract()
    };
    let ix = ((points.len() - 1) as f32 * progress).round() as usize;
    let (cx, cy) = points[ix.min(points.len() - 1)];
    let r = if hovered { 4.2 } else { 3.0 } * scale;
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        None,
        cx - r,
        cy - r,
        r * 2.0,
        r * 2.0,
        with_alpha(accent, 0.95),
        depth,
        r,
        6,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        clip,
        None,
        cx - r * 2.0,
        cy - r * 2.0,
        r * 4.0,
        r * 4.0,
        with_alpha(accent, if hovered { 0.2 } else { 0.12 }),
        depth,
        r * 2.0,
        5,
    );
}

#[allow(clippy::too_many_arguments)]
fn render_chart_hover(
    sugarloaf: &mut Sugarloaf,
    spec: &StockCardSpec,
    values: &[f64],
    min_v: f64,
    max_v: f64,
    rect: [f32; 4],
    mouse_x: f32,
    theme: &IdeTheme,
    scale: f32,
    accent: [f32; 4],
    depth: f32,
) {
    let [x, y, w, h] = rect;
    let denom = (values.len().saturating_sub(1)).max(1) as f32;
    let ix = (((mouse_x - x) / w) * denom)
        .round()
        .clamp(0.0, values.len().saturating_sub(1) as f32) as usize;
    let value = values[ix];
    let px = x + ix as f32 / denom * w;
    let py = y + h - ((value - min_v) / (max_v - min_v).max(f64::EPSILON)) as f32 * h;

    draw_line_clipped(
        sugarloaf,
        rect,
        px,
        y,
        px,
        y + h,
        1.0 * scale,
        depth,
        theme.f32_alpha(theme.fg, 0.24),
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        rect,
        None,
        px - 5.0 * scale,
        py - 5.0 * scale,
        10.0 * scale,
        10.0 * scale,
        theme.f32(theme.bg),
        depth,
        5.0 * scale,
        32,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        rect,
        None,
        px - 3.2 * scale,
        py - 3.2 * scale,
        6.4 * scale,
        6.4 * scale,
        accent,
        depth,
        3.2 * scale,
        33,
    );

    let price = format_price(value, spec.currency.as_str());
    let label = point_label(spec, ix, values.len());
    let price_opts = DrawOpts {
        font_size: 12.0 * scale,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(rect),
        ..DrawOpts::default()
    };
    let label_opts = DrawOpts {
        font_size: 10.5 * scale,
        color: theme.u8_alpha(theme.white, 0.64),
        clip_rect: Some(rect),
        ..DrawOpts::default()
    };
    let price_w = sugarloaf.text_mut().measure(&price, &price_opts);
    let label_w = sugarloaf.text_mut().measure(&label, &label_opts);
    let bubble_w = price_w.max(label_w) + 20.0 * scale;
    let bubble_h = 42.0 * scale;
    let mut bx = px - bubble_w * 0.5;
    bx = bx.clamp(x + 4.0 * scale, x + w - bubble_w - 4.0 * scale);
    let mut by = py - bubble_h - 12.0 * scale;
    if by < y + 4.0 * scale {
        by = py + 12.0 * scale;
    }

    draw_rounded_rect_clipped(
        sugarloaf,
        rect,
        None,
        bx - 1.0 * scale,
        by - 1.0 * scale,
        bubble_w + 2.0 * scale,
        bubble_h + 2.0 * scale,
        theme.f32_alpha(theme.fg, 0.18),
        depth,
        11.0 * scale,
        34,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        rect,
        None,
        bx,
        by,
        bubble_w,
        bubble_h,
        theme.f32_alpha(theme.bg, 0.96),
        depth,
        10.0 * scale,
        35,
    );
    sugarloaf
        .text_mut()
        .draw(bx + 10.0 * scale, by + 7.0 * scale, &price, &price_opts);
    sugarloaf
        .text_mut()
        .draw(bx + 10.0 * scale, by + 24.0 * scale, &label, &label_opts);
}

#[allow(clippy::too_many_arguments)]
fn draw_rounded_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    id: Option<usize>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    depth: f32,
    radius: f32,
    order: u8,
) {
    crate::widgets::quad::rounded_rect_clipped(
        sugarloaf,
        clip,
        id,
        [x, y, w, h],
        color,
        depth,
        radius,
        order,
        0.5,
    );
}

#[allow(clippy::too_many_arguments)]
fn draw_rect_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    id: Option<usize>,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    color: [f32; 4],
    depth: f32,
    order: u8,
) {
    let Some([x, y, w, h]) = intersect_rect([x, y, w, h], clip) else {
        return;
    };
    sugarloaf.rect(id, x, y, w, h, color, depth, order);
}

#[allow(clippy::too_many_arguments)]
fn draw_line_clipped(
    sugarloaf: &mut Sugarloaf,
    clip: [f32; 4],
    mut x1: f32,
    mut y1: f32,
    mut x2: f32,
    mut y2: f32,
    width: f32,
    depth: f32,
    color: [f32; 4],
) {
    let [left, top, w, h] = clip;
    let right = left + w;
    let bottom = top + h;
    let dx = x2 - x1;
    let dy = y2 - y1;
    let mut t0 = 0.0;
    let mut t1 = 1.0;
    for (p, q) in [
        (-dx, x1 - left),
        (dx, right - x1),
        (-dy, y1 - top),
        (dy, bottom - y1),
    ] {
        if p == 0.0 {
            if q < 0.0 {
                return;
            }
            continue;
        }
        let r = q / p;
        if p < 0.0 {
            if r > t1 {
                return;
            }
            if r > t0 {
                t0 = r;
            }
        } else {
            if r < t0 {
                return;
            }
            if r < t1 {
                t1 = r;
            }
        }
    }
    if t1 < 1.0 {
        x2 = x1 + t1 * dx;
        y2 = y1 + t1 * dy;
    }
    if t0 > 0.0 {
        x1 += t0 * dx;
        y1 += t0 * dy;
    }
    sugarloaf.line(x1, y1, x2, y2, width, depth, color);
}

fn sampled_values(points: &[StockPoint]) -> Vec<f64> {
    let finite: Vec<f64> = points
        .iter()
        .filter_map(|point| point.v.is_finite().then_some(point.v))
        .collect();
    if finite.len() <= MAX_POINTS {
        return finite;
    }
    let step = (finite.len() - 1) as f32 / (MAX_POINTS - 1) as f32;
    (0..MAX_POINTS)
        .map(|ix| finite[(ix as f32 * step).round() as usize])
        .collect()
}

fn render_stats(
    sugarloaf: &mut Sugarloaf,
    spec: &StockCardSpec,
    x: f32,
    y: f32,
    width: f32,
    scale: f32,
    label_opts: &DrawOpts,
    value_opts: &DrawOpts,
) {
    let columns = [
        [
            ("Open", spec.stats.open.as_deref()),
            ("Day Low", spec.stats.day_low.as_deref()),
            ("Day High", spec.stats.day_high.as_deref()),
        ],
        [
            ("Volume", spec.stats.volume.as_deref()),
            ("Year Low", spec.stats.year_low.as_deref()),
            ("Year High", spec.stats.year_high.as_deref()),
        ],
        [
            ("Market Cap", spec.stats.market_cap.as_deref()),
            ("EPS (TTM)", spec.stats.eps_ttm.as_deref()),
            ("P/E Ratio", spec.stats.pe_ratio.as_deref()),
        ],
    ];
    let col_w = width / columns.len() as f32;
    for (col_ix, rows) in columns.iter().enumerate() {
        let col_x = x + col_ix as f32 * col_w;
        for (row_ix, (label, value)) in rows.iter().enumerate() {
            let Some(value) = value.filter(|value| !value.trim().is_empty()) else {
                continue;
            };
            let row_y = y + row_ix as f32 * 21.0 * scale;
            if let Some(opts) = opts_visible_at(label_opts, col_x, row_y) {
                sugarloaf.text_mut().draw(col_x, row_y, label, &opts);
            }
            let value_w = sugarloaf.text_mut().measure(value, value_opts);
            let value_x = col_x + col_w - value_w - 10.0 * scale;
            if let Some(opts) = opts_visible_at(value_opts, value_x, row_y) {
                sugarloaf.text_mut().draw(value_x, row_y, value, &opts);
            }
        }
    }
}

fn opts_visible_at(opts: &DrawOpts, x: f32, y: f32) -> Option<DrawOpts> {
    if let Some(clip) = opts.clip_rect {
        let h = opts.font_size.max(1.0);
        if y + h < clip[1] || y > clip[1] + clip[3] || x > clip[0] + clip[2] {
            return None;
        }
    }
    Some(opts.clone())
}

fn point_label(spec: &StockCardSpec, ix: usize, len: usize) -> String {
    if spec.points.len() == len {
        if let Some(label) = spec
            .points
            .get(ix)
            .map(|point| point.t.trim())
            .filter(|label| !label.is_empty())
        {
            return label.to_string();
        }
    }
    match spec
        .range
        .as_deref()
        .map(str::trim)
        .filter(|range| !range.is_empty())
    {
        Some(range) => format!("{range} point {}", ix + 1),
        None => format!("Point {}", ix + 1),
    }
}

fn format_price(price: f64, currency: &str) -> String {
    let prefix = match currency.trim().to_ascii_uppercase().as_str() {
        "USD" | "CAD" | "AUD" => "$",
        "GBP" => "£",
        "EUR" => "€",
        "JPY" => "¥",
        _ => "",
    };
    format!("{prefix}{price:.2}")
}

fn format_change(spec: &StockCardSpec) -> String {
    let change = spec.change.unwrap_or(0.0);
    let pct = spec.change_percent.unwrap_or(0.0);
    let sign = if change >= 0.0 { "+" } else { "" };
    let period = spec.period.as_deref().unwrap_or("Today");
    format!("{sign}{change:.2} ({sign}{pct:.2}%) · {period}")
}

fn intersect_rect(a: [f32; 4], b: [f32; 4]) -> Option<[f32; 4]> {
    let x1 = a[0].max(b[0]);
    let y1 = a[1].max(b[1]);
    let x2 = (a[0] + a[2]).min(b[0] + b[2]);
    let y2 = (a[1] + a[3]).min(b[1] + b[3]);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2 - x1, y2 - y1])
}

fn subtract_rect(rect: [f32; 4], cut: [f32; 4]) -> Vec<[f32; 4]> {
    let Some(overlap) = intersect_rect(rect, cut) else {
        return vec![rect];
    };
    let [rx, ry, rw, rh] = rect;
    let [ox, oy, ow, oh] = overlap;
    let r_right = rx + rw;
    let r_bottom = ry + rh;
    let o_right = ox + ow;
    let o_bottom = oy + oh;
    let mut out = Vec::with_capacity(4);
    if oy > ry {
        out.push([rx, ry, rw, oy - ry]);
    }
    if o_bottom < r_bottom {
        out.push([rx, o_bottom, rw, r_bottom - o_bottom]);
    }
    if ox > rx {
        out.push([rx, oy, ox - rx, oh]);
    }
    if o_right < r_right {
        out.push([o_right, oy, r_right - o_right, oh]);
    }
    out.into_iter()
        .filter(|rect| rect[2] > 0.0 && rect[3] > 0.0)
        .collect()
}

fn rects_intersect(a: [f32; 4], b: [f32; 4]) -> bool {
    let (ax1, ay1, ax2, ay2) = (a[0], a[1], a[0] + a[2], a[1] + a[3]);
    let (bx1, by1, bx2, by2) = (b[0], b[1], b[0] + b[2], b[1] + b[3]);
    ax1 < bx2 && ax2 > bx1 && ay1 < by2 && ay2 > by1
}

fn with_alpha(mut color: [f32; 4], alpha: f32) -> [f32; 4] {
    color[3] = alpha.clamp(0.0, 1.0);
    color
}

fn symbol_phase(symbol: &str) -> f32 {
    let hash = symbol.bytes().fold(0u32, |acc, byte| {
        acc.wrapping_mul(31).wrapping_add(byte as u32)
    });
    (hash % 628) as f32 / 100.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_minimal_stock_card() {
        let spec = parse_stock_card(
            r#"{
                "symbol":"AAPL",
                "name":"Apple Inc",
                "price":297.2,
                "points":[{"t":"9:30","v":296.1},{"t":"9:35","v":297.2}]
            }"#,
        )
        .unwrap();
        assert_eq!(spec.version, 1);
        assert_eq!(spec.currency, "USD");
        assert_eq!(spec.symbol, "AAPL");
        assert_eq!(spec.points.len(), 2);
    }
}
