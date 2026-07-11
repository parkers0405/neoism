use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::IdeTheme;

const DEPTH: f32 = 0.0;
const ORDER: u8 = 180;
// Two-line rows: title on the first line, description underneath. Mirrored by
// PICKER_ROW_HEIGHT in panels/agent_pane/state/picker.rs — keep in sync.
const ROW_H: f32 = 46.0;
/// Shimmer placeholder rows drawn while an async search is in flight.
const SKELETON_ROWS: usize = 3;
const TITLE_H: f32 = 54.0;
const MAX_ROWS: usize = 8;
const RADIUS: f32 = 14.0;
/// Height (pre-scale) of the muted hint band drawn under the list when a
/// picker supplies a `footer_hint` (the `/sessions` picker's key legend).
pub const FOOTER_H: f32 = 26.0;

#[derive(Clone, Copy)]
pub struct InlinePickerRow<'a> {
    pub title: &'a str,
    pub description: &'a str,
    pub footer: &'a str,
    pub is_header: bool,
    /// Draw a filled accent-colored dot on the left to mark the currently
    /// active item (e.g. the session the user is presently inside).
    pub is_current: bool,
    /// Draw a small pin glyph on the right of the row to mark a pinned
    /// session.
    pub is_pinned: bool,
}

#[derive(Clone, Copy)]
pub struct InlinePickerView<'a> {
    pub title: &'a str,
    pub query: &'a str,
    pub selected: usize,
    pub scroll_offset: usize,
    pub list_scroll_offset: f32,
    pub cursor_offset: f32,
    pub rows: &'a [InlinePickerRow<'a>],
    /// Muted key-legend band drawn under the list (e.g. the `/sessions`
    /// picker's `pin/unpin ctrl+f …`). `None` leaves the card list-only.
    pub footer_hint: Option<&'a str>,
    /// In-progress rename buffer. When `Some`, the search row is replaced
    /// with an inline `Rename › <buffer>` editor instead of the filter text.
    pub rename: Option<&'a str>,
    /// Whether to draw the blinking search caret. Off for pickers whose live
    /// input is the composer (slash / @file / skill mentions) — the composer
    /// already shows a caret there, so a second one in the search row reads
    /// as a doubled/misplaced cursor.
    pub show_search_caret: bool,
    /// Muted placeholder shown in the empty search/input row. Defaults to
    /// "Search" for filter pickers; the `/connect` secret-entry stage passes
    /// e.g. "API key" so the row reads as a single-field input.
    pub search_placeholder: &'a str,
    /// Draw shimmering skeleton rows in place of "No results" while an async
    /// search (e.g. semantic session search) is still in flight.
    pub loading: bool,
    /// Seconds since loading began — drives the shimmer phase.
    pub loading_elapsed: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct InlinePickerRenderState {
    pub rect: [f32; 4],
    pub selected_cursor_rect: Option<[f32; 4]>,
    /// Device-pixel height of the footer band (0 when absent).
    pub footer_h: f32,
}

/// Trim `text` with an ellipsis so its measured width is ≤ `max_w`.
fn truncate_to_pixel_width(
    sugarloaf: &mut Sugarloaf,
    text: &str,
    opts: &DrawOpts,
    max_w: f32,
) -> String {
    if sugarloaf.text_mut().measure(text, opts) <= max_w {
        return text.to_string();
    }
    let ellipsis = "…";
    let mut buf: Vec<char> = text.chars().collect();
    while !buf.is_empty() {
        buf.pop();
        let candidate: String = buf.iter().collect::<String>() + ellipsis;
        if sugarloaf.text_mut().measure(&candidate, opts) <= max_w {
            return candidate;
        }
    }
    ellipsis.to_string()
}

pub fn layout(
    row_count: usize,
    input_rect: [f32; 4],
    scale: f32,
    has_footer: bool,
) -> Option<[f32; 4]> {
    let s = scale.clamp(0.5, 3.0);
    let row_h = ROW_H * s;
    let title_h = TITLE_H * s;
    let footer_h = if has_footer { FOOTER_H * s } else { 0.0 };
    // An empty picker still shows one row ("No results") and stays open, so
    // a filter that matches nothing doesn't make the modal vanish.
    let visible_rows = row_count.min(MAX_ROWS).max(1);
    // Lock to the composer's width and x position so the popover lines up
    // edge-to-edge with the input chrome.
    let width = input_rect[2];
    let height = title_h + visible_rows as f32 * row_h + footer_h;
    let x = input_rect[0];
    let y = (input_rect[1] - height - 6.0 * s).max(8.0 * s);
    Some([x, y, width, height])
}

pub fn render(
    sugarloaf: &mut Sugarloaf,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
) -> Option<InlinePickerRenderState> {
    let s = scale.clamp(0.5, 3.0);
    let row_h = ROW_H * s;
    let title_h = TITLE_H * s;
    let has_footer = view.footer_hint.is_some();
    let footer_h = if has_footer { FOOTER_H * s } else { 0.0 };
    // While loading with no rows yet, size the card for the skeleton rows.
    let layout_rows = if view.rows.is_empty() && view.loading {
        SKELETON_ROWS
    } else {
        view.rows.len()
    };
    let [x, y, width, height] = layout(layout_rows, input_rect, scale, has_footer)?;
    let visible_rows = layout_rows.min(MAX_ROWS).max(1);
    let selected = view.selected.min(view.rows.len().saturating_sub(1));
    let first = view
        .scroll_offset
        .min(view.rows.len().saturating_sub(visible_rows));
    let header_clip = [x, y, width, title_h];

    // NB: no square backing rect here — a full-bounds `rect` would fill
    // the four corner triangles the rounded rects leave empty, showing as
    // black square corners poking past the rounded ones on themes where
    // `black != bg`. The rounded fills below are the whole container.
    sugarloaf.rounded_rect(
        None,
        x,
        y,
        width,
        height,
        theme.f32(theme.black),
        DEPTH,
        RADIUS * s,
        ORDER,
    );
    sugarloaf.rounded_rect(
        None,
        x,
        y,
        width,
        height,
        theme.f32(theme.border),
        DEPTH,
        RADIUS * s,
        ORDER + 1,
    );
    sugarloaf.rounded_rect(
        None,
        x + s,
        y + s,
        (width - 2.0 * s).max(0.0),
        (height - 2.0 * s).max(0.0),
        theme.f32(theme.bg),
        DEPTH,
        (RADIUS - 1.0) * s,
        ORDER + 2,
    );

    // Header band: modal title (left) + `esc` hint (right), then a search
    // row below showing the live query or a muted "Search" placeholder.
    // Type-to-filter is always on; this just gives it a visible input.
    sugarloaf.text_mut().draw(
        x + 14.0 * s,
        y + 9.0 * s,
        view.title,
        &DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        },
    );
    let esc_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(header_clip),
        ..DrawOpts::default()
    };
    let esc_w = sugarloaf.text_mut().measure("esc", &esc_opts);
    sugarloaf.text_mut().draw(
        x + width - 14.0 * s - esc_w,
        y + 9.0 * s,
        "esc",
        &esc_opts,
    );
    if let Some(rename) = view.rename {
        // Inline rename editor takes over the search row: an accent label
        // plus the live buffer with a trailing caret block.
        let label_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.cyan),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        sugarloaf
            .text_mut()
            .draw(x + 14.0 * s, y + 31.0 * s, "Rename ›", &label_opts);
        let label_w = sugarloaf.text_mut().measure("Rename › ", &label_opts);
        let buffer = format!("{rename}▏");
        sugarloaf.text_mut().draw(
            x + 14.0 * s + label_w,
            y + 31.0 * s,
            &buffer,
            &DrawOpts {
                font_size: 13.0 * s,
                color: theme.u8(theme.fg),
                clip_rect: Some(header_clip),
                ..DrawOpts::default()
            },
        );
    } else {
        let search_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.fg),
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        // Caret sits after the typed query, or at the field start (with the
        // muted placeholder pushed right) when empty — reads as a live input.
        let caret_x = if view.query.is_empty() {
            sugarloaf.text_mut().draw(
                x + 14.0 * s + 5.0 * s,
                y + 31.0 * s,
                view.search_placeholder,
                &DrawOpts {
                    color: theme.u8(theme.muted),
                    ..search_opts
                },
            );
            x + 14.0 * s
        } else {
            sugarloaf.text_mut().draw(
                x + 14.0 * s,
                y + 31.0 * s,
                view.query,
                &search_opts,
            );
            x + 14.0 * s + sugarloaf.text_mut().measure(view.query, &search_opts)
        };
        if view.show_search_caret {
            // Caret sits ON the search line (same Y as the query/placeholder
            // text at `y + 31*s`), a short bar ~ the search font size — NOT a
            // tall bar floating up into the title row. `caret_x` already
            // tracks the measured query width, so it advances as you type.
            let caret_w = (1.5 * s).max(1.0);
            sugarloaf.rounded_rect(
                None,
                caret_x + 1.0 * s,
                y + 31.0 * s,
                caret_w,
                13.0 * s,
                theme.f32(theme.accent),
                DEPTH,
                0.0,
                ORDER + 3,
            );
        }
    }

    let list_y = y + title_h;
    let list_clip = [x, list_y, width, visible_rows as f32 * row_h];
    let title_opts = DrawOpts {
        font_size: 14.0 * s,
        color: theme.u8(theme.fg),
        bold: true,
        clip_rect: Some(list_clip),
        ..DrawOpts::default()
    };
    let desc_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.dim),
        clip_rect: Some(list_clip),
        ..DrawOpts::default()
    };
    let footer_opts = DrawOpts {
        font_size: 11.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(list_clip),
        ..DrawOpts::default()
    };
    let header_opts = DrawOpts {
        font_size: 13.5 * s,
        color: theme.u8(theme.cyan),
        bold: true,
        clip_rect: Some(list_clip),
        ..DrawOpts::default()
    };

    let list_bottom = list_y + visible_rows as f32 * row_h;
    let mut selected_cursor_rect = None;
    let overscan =
        ((view.list_scroll_offset.abs() / row_h).ceil() as usize).saturating_add(1);
    let start = first.saturating_sub(overscan);
    let end = (first + visible_rows + overscan).min(view.rows.len());
    // Snap the spring offsets to device-pixel boundaries before they
    // land in any row Y. Matches the same fix applied to the editor
    // `pixel_offset_y` and Finder's `list_scroll_offset` — sub-pixel
    // float positions push every glyph onto a slightly different
    // sub-pixel anchor per frame during continuous scroll, which the
    // eye integrates as smeared text. Snap once here so every row's
    // y inherits an integer pixel position.
    let list_scroll_snapped =
        crate::primitives::snap_to_device_px(view.list_scroll_offset * s, s);
    let cursor_offset_snapped =
        crate::primitives::snap_to_device_px(view.cursor_offset * s, s);
    for absolute_ix in start..end {
        let row = &view.rows[absolute_ix];
        let visible_ix = absolute_ix as isize - first as isize;
        let row_y = list_y + visible_ix as f32 * row_h + list_scroll_snapped;
        if row_y + row_h <= list_y || row_y >= list_bottom {
            continue;
        }
        let selected_row = absolute_ix == selected;
        if row.is_header {
            if row.title.is_empty() {
                continue;
            }
            let title_x = x + 22.0 * s;
            let title_text = truncate_to_pixel_width(
                sugarloaf,
                row.title,
                &header_opts,
                width - 44.0 * s,
            );
            sugarloaf.text_mut().draw(
                title_x,
                row_y + 16.0 * s,
                &title_text,
                &header_opts,
            );
            continue;
        }
        if selected_row {
            let selected_y = row_y + cursor_offset_snapped;
            let visible_y = selected_y.max(list_y);
            let visible_h = (selected_y + row_h).min(list_bottom) - visible_y;
            sugarloaf.rounded_rect(
                None,
                x + 6.0 * s,
                visible_y + 3.0 * s,
                width - 12.0 * s,
                (visible_h - 6.0 * s).max(0.0),
                theme.f32(theme.hover),
                DEPTH,
                9.0 * s,
                ORDER + 3,
            );
            sugarloaf.rounded_rect(
                None,
                x + 10.0 * s,
                visible_y + 9.0 * s,
                3.0 * s,
                (visible_h - 18.0 * s).max(0.0),
                theme.f32(theme.accent),
                DEPTH,
                2.0 * s,
                ORDER + 4,
            );
            let cursor_w = (14.0 * s * 0.6).max(2.0);
            let cursor_h = (row_h - 8.0 * s).max(14.0 * s).min(row_h);
            let cursor_x = (x + 18.0 * s - cursor_w - 2.0 * s).max(x + 6.0 * s);
            let cursor_y = (selected_y + (row_h - cursor_h) / 2.0)
                .clamp(list_y, (list_bottom - cursor_h).max(list_y));
            selected_cursor_rect = Some([cursor_x, cursor_y, cursor_w, cursor_h]);
        }
        // Current-session dot — small filled circle in accent color,
        // left-aligned in the 22 px gutter, independent of selection.
        // Rects/quads aren't bounded by the text `clip_rect`, so cull the
        // dot when its row is sliced at the list edges — otherwise it bleeds
        // past the modal's top/bottom rounded corners as the row scrolls out.
        if row.is_current {
            let dot_d = 6.0 * s;
            let dot_x = x + 7.0 * s;
            let dot_y = row_y + (row_h - dot_d) / 2.0;
            if dot_y >= list_y && dot_y + dot_d <= list_bottom {
                sugarloaf.rounded_rect(
                    None,
                    dot_x,
                    dot_y,
                    dot_d,
                    dot_d,
                    theme.f32(theme.accent),
                    DEPTH,
                    dot_d / 2.0,
                    ORDER + 5,
                );
            }
        }
        let title_x = x + 22.0 * s;
        let footer_w = if row.footer.is_empty() {
            0.0
        } else {
            sugarloaf.text_mut().measure(row.footer, &footer_opts) + 22.0 * s
        };
        let footer_x = x + width - footer_w;
        // Reserve a gap before the footer so long titles don't smear
        // through the time column; trim with an ellipsis when needed.
        let title_max_w = (footer_x - title_x - 14.0 * s).max(48.0);
        let title_text =
            truncate_to_pixel_width(sugarloaf, row.title, &title_opts, title_max_w);
        // Two-line rows: title on the first line, description on the line
        // below. Rows without a description keep the title centered.
        let title_y = if row.description.is_empty() {
            row_y + 13.0 * s
        } else {
            row_y + 5.0 * s
        };
        sugarloaf
            .text_mut()
            .draw(title_x, title_y, &title_text, &title_opts);
        if !row.description.is_empty() {
            let desc_max_w = (footer_x - title_x - 12.0 * s).max(48.0);
            let desc_text = truncate_to_pixel_width(
                sugarloaf,
                row.description,
                &desc_opts,
                desc_max_w,
            );
            sugarloaf
                .text_mut()
                .draw(title_x, row_y + 25.0 * s, &desc_text, &desc_opts);
        }
        if !row.footer.is_empty() {
            sugarloaf.text_mut().draw(
                footer_x,
                row_y + 15.0 * s,
                row.footer,
                &footer_opts,
            );
        }
        // Pinned marker — a small cyan dot in the row's right padding,
        // clear of the right-aligned time text. Culled at the list edges
        // for the same reason as the current-session dot above.
        if row.is_pinned {
            let dot_d = 6.0 * s;
            let dot_x = x + width - dot_d - 8.0 * s;
            let dot_y = row_y + (row_h - dot_d) / 2.0;
            if dot_y >= list_y && dot_y + dot_d <= list_bottom {
                sugarloaf.rounded_rect(
                    None,
                    dot_x,
                    dot_y,
                    dot_d,
                    dot_d,
                    theme.f32(theme.cyan),
                    DEPTH,
                    dot_d / 2.0,
                    ORDER + 5,
                );
            }
        }
    }
    // Empty state — keep the modal open and legible instead of collapsing
    // to nothing when a filter (or an empty catalog) yields no rows.
    if view.rows.is_empty() {
        if view.loading {
            // Shimmering two-bar skeleton rows while the async search runs —
            // same wave family as the file-tree loading skeleton.
            let fade_in = (view.loading_elapsed / 0.18).min(1.0);
            const SKELETON_WIDTHS: [(f32, f32); SKELETON_ROWS] =
                [(0.42, 0.68), (0.58, 0.46), (0.36, 0.60)];
            for i in 0..SKELETON_ROWS {
                let row_y = list_y + i as f32 * row_h;
                if row_y + row_h > list_bottom + 0.5 {
                    break;
                }
                let wave = (view.loading_elapsed / 1.3 * std::f32::consts::TAU
                    - i as f32 * 0.55)
                    .sin();
                let alpha = (0.16 + 0.08 * wave).max(0.04) * fade_in;
                let (title_frac, desc_frac) = SKELETON_WIDTHS[i];
                let avail = (width - 44.0 * s).max(0.0);
                sugarloaf.rounded_rect(
                    None,
                    x + 22.0 * s,
                    row_y + 8.0 * s,
                    avail * title_frac,
                    12.0 * s,
                    theme.f32_alpha(theme.muted, alpha),
                    DEPTH,
                    6.0 * s,
                    ORDER + 2,
                );
                sugarloaf.rounded_rect(
                    None,
                    x + 22.0 * s,
                    row_y + 26.0 * s,
                    avail * desc_frac,
                    9.0 * s,
                    theme.f32_alpha(theme.muted, alpha * 0.7),
                    DEPTH,
                    4.5 * s,
                    ORDER + 2,
                );
            }
        } else {
            sugarloaf.text_mut().draw(
                x + 22.0 * s,
                list_y + (row_h - 14.0 * s) / 2.0 + 7.0 * s,
                "No results",
                &DrawOpts {
                    font_size: 13.0 * s,
                    color: theme.u8(theme.muted),
                    clip_rect: Some(list_clip),
                    ..DrawOpts::default()
                },
            );
        }
    }
    // Footer hint band: a muted key legend under the list, split off by a
    // thin separator. Only the `/sessions` picker supplies one.
    if let Some(hint) = view.footer_hint {
        let band_y = list_bottom;
        // Opaque band with rounded bottom corners matching the card, drawn
        // ABOVE the row highlights (ORDER+3..5) so the selected-row accent
        // can't phase through the legend. Reuses the per-corner-radii quad.
        sugarloaf.quad(
            None,
            x + s,
            band_y,
            (width - 2.0 * s).max(0.0),
            (footer_h - s).max(0.0),
            theme.f32(theme.bg),
            [0.0, 0.0, (RADIUS - 1.0) * s, (RADIUS - 1.0) * s],
            DEPTH,
            ORDER + 6,
        );
        sugarloaf.rect(
            None,
            x + s,
            band_y,
            (width - 2.0 * s).max(0.0),
            (1.0 * s).max(1.0),
            theme.f32(theme.border),
            DEPTH,
            ORDER + 7,
        );
        let footer_hint_opts = DrawOpts {
            font_size: 11.0 * s,
            color: theme.u8(theme.muted),
            clip_rect: Some([x, band_y, width, footer_h]),
            ..DrawOpts::default()
        };
        sugarloaf.text_mut().draw(
            x + 14.0 * s,
            band_y + (footer_h - 11.0 * s) / 2.0,
            hint,
            &footer_hint_opts,
        );
    }
    Some(InlinePickerRenderState {
        rect: [x, y, width, height],
        selected_cursor_rect,
        footer_h,
    })
}
