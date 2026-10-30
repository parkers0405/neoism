mod icons;
pub use icons::InlinePickerIcon;
pub(crate) use icons::crop as clip_icon_image;

use neoism_paint::text::DrawOpts;
use unicode_segmentation::UnicodeSegmentation;

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
pub const DEFAULT_MAX_ROWS: usize = MAX_ROWS;
const RADIUS: f32 = 14.0;
/// Height (pre-scale) of the muted hint band drawn under the list when a
/// picker supplies a `footer_hint` (the `/sessions` picker's key legend).
pub const FOOTER_H: f32 = 26.0;

/// Largest row window that fits entirely between `min_y` and the
/// composer, capped by `preferred_rows`. Always returns at least one
/// row so an empty/no-results picker remains usable on tiny panes.
pub fn row_limit_for_space(
    input_y: f32,
    min_y: f32,
    scale: f32,
    has_footer: bool,
    preferred_rows: usize,
) -> usize {
    let s = scale.clamp(0.5, 3.0);
    let footer_h = if has_footer { FOOTER_H * s } else { 0.0 };
    let fixed_h = TITLE_H * s + footer_h + 6.0 * s;
    let available_rows_h = (input_y - min_y - fixed_h).max(ROW_H * s);
    ((available_rows_h / (ROW_H * s)).floor() as usize)
        .clamp(1, preferred_rows.clamp(1, MAX_ROWS))
}

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
    /// Optional semantic status dot drawn immediately before the footer.
    pub status: Option<InlinePickerStatus>,
}

#[derive(Clone, Copy)]
pub enum InlinePickerStatus {
    Good,
    Warning,
    Error,
    Muted,
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
    /// Multiline title and answer field for model questions. Other pickers
    /// retain their compact, single-line header.
    pub wrap_prompt: bool,
    /// Muted placeholder shown in the empty search/input row. Defaults to
    /// "Search" for filter pickers; the `/connect` secret-entry stage passes
    /// e.g. "API key" so the row reads as a single-field input.
    pub search_placeholder: &'a str,
    /// Draw shimmering skeleton rows in place of "No results" while an async
    /// search (e.g. semantic session search) is still in flight.
    pub loading: bool,
    /// Message rendered in the list area when there are no rows. Input-only
    /// pickers (account label / API key) pass `None` so the card ends after
    /// its input row instead of pretending a search returned no matches.
    pub empty_message: Option<&'a str>,
    /// Seconds since loading began — drives the shimmer phase.
    pub loading_elapsed: f32,
}

/// Progress content inside the same shell. Manual auth may retain its input row.
#[derive(Clone, Copy, Debug)]
pub struct InlinePickerActivity<'a> {
    pub body: &'a str,
    pub status: &'a str,
    pub busy: bool,
    /// Keep the existing query/secret field below the status, not hidden behind it.
    pub show_input: bool,
    pub authorization_url: Option<&'a str>,
}

#[derive(Clone, Copy, Debug)]
pub struct InlinePickerRenderState {
    pub rect: [f32; 4],
    pub selected_cursor_rect: Option<[f32; 4]>,
    /// Device-pixel height of the footer band (0 when absent).
    pub footer_h: f32,
    /// Source-row range actually painted into the card.
    pub first_row: usize,
    pub visible_rows: usize,
    /// Height of the header; question prompts can have wrapped lines.
    pub header_h: f32,
}

fn activity_link_rect(rect: [f32; 4], header_h: f32, scale: f32, show_input: bool) -> [f32; 4] {
    let s = scale.clamp(0.5, 3.0);
    let input_h = if show_input { 22.0 * s } else { 0.0 };
    [rect[0] + 14.0 * s, rect[1] + header_h - input_h - 24.0 * s, (rect[2] - 28.0 * s).max(0.0), 24.0 * s]
}

impl InlinePickerRenderState {
    /// Shared measured hit geometry for a compact authorization link in the header.
    pub fn activity_link_rect(self, scale: f32, show_input: bool) -> [f32; 4] {
        activity_link_rect(self.rect, self.header_h, scale, show_input)
    }

    pub fn row_rect(self, visible_ix: usize, scale: f32) -> [f32; 4] {
        let s = scale.clamp(0.5, 3.0);
        [
            self.rect[0],
            self.rect[1] + self.header_h + visible_ix as f32 * ROW_H * s,
            self.rect[2],
            ROW_H * s,
        ]
    }
}

/// Wrap measured text, breaking oversized words at grapheme boundaries.
fn wrap_prompt_text(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for word in paragraph.split_whitespace() {
            let candidate = if line.is_empty() {
                word.to_owned()
            } else {
                format!("{line} {word}")
            };
            if measure(&candidate) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() {
                lines.push(std::mem::take(&mut line));
            }
            for cluster in word.graphemes(true) {
                if !line.is_empty() && measure(&format!("{line}{cluster}")) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(cluster);
            }
        }
        lines.push(line);
    }
    lines
}

/// Editable answers must preserve every space so the painted caret matches
/// the user's buffer, while readonly prompt text can normalize whitespace.
fn wrap_answer_text(
    text: &str,
    width: f32,
    mut measure: impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    for paragraph in text.split('\n') {
        let mut line = String::new();
        for part in paragraph.split_word_bounds() {
            let candidate = format!("{line}{part}");
            if measure(&candidate) <= width {
                line = candidate;
                continue;
            }
            if !line.is_empty() && !part.chars().all(char::is_whitespace) {
                lines.push(std::mem::take(&mut line));
            }
            for cluster in part.graphemes(true) {
                if !line.is_empty() && measure(&format!("{line}{cluster}")) > width {
                    lines.push(std::mem::take(&mut line));
                }
                line.push_str(cluster);
            }
        }
        lines.push(line);
    }
    lines
}

fn answer_caret_rect(mut rect: [f32; 4], clip: [f32; 4]) -> Option<[f32; 4]> {
    rect[0] = rect[0].min((clip[0] + clip[2] - rect[2]).max(clip[0]));
    crate::primitives::geom::intersect_rect(rect, clip)
}

/// Trim `text` with an ellipsis so its measured width is ≤ `max_w`.
fn truncate_to_pixel_width(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    text: &str,
    opts: &DrawOpts,
    max_w: f32,
) -> String {
    if sugarloaf.overlay_text_mut().measure(text, opts) <= max_w {
        return text.to_string();
    }
    let ellipsis = "…";
    let mut buf: Vec<char> = text.chars().collect();
    while !buf.is_empty() {
        buf.pop();
        let candidate: String = buf.iter().collect::<String>() + ellipsis;
        if sugarloaf.overlay_text_mut().measure(&candidate, opts) <= max_w {
            return candidate;
        }
    }
    ellipsis.to_string()
}

/// Rect of visible row `visible_ix` inside a rendered card — for hosts
/// that register their own per-row hit rects (the agent prompt picker's
/// permission choices / question options) instead of routing clicks
/// through a `NeoismAgentPicker`.
pub fn row_rect(card: [f32; 4], visible_ix: usize, scale: f32) -> [f32; 4] {
    let s = scale.clamp(0.5, 3.0);
    [
        card[0],
        card[1] + TITLE_H * s + visible_ix as f32 * ROW_H * s,
        card[2],
        ROW_H * s,
    ]
}

pub fn layout(
    row_count: usize,
    input_rect: [f32; 4],
    scale: f32,
    has_footer: bool,
) -> Option<[f32; 4]> {
    layout_limited(
        row_count,
        input_rect,
        scale,
        has_footer,
        MAX_ROWS,
        8.0 * scale.clamp(0.5, 3.0),
    )
}

pub fn layout_limited(
    row_count: usize,
    input_rect: [f32; 4],
    scale: f32,
    has_footer: bool,
    max_rows: usize,
    min_y: f32,
) -> Option<[f32; 4]> {
    layout_limited_with_empty_row(
        row_count, input_rect, scale, has_footer, max_rows, min_y, true,
    )
}

#[allow(clippy::too_many_arguments)]
fn layout_limited_with_empty_row(
    row_count: usize,
    input_rect: [f32; 4],
    scale: f32,
    has_footer: bool,
    max_rows: usize,
    min_y: f32,
    reserve_empty_row: bool,
) -> Option<[f32; 4]> {
    let s = scale.clamp(0.5, 3.0);
    let row_h = ROW_H * s;
    let title_h = TITLE_H * s;
    let footer_h = if has_footer { FOOTER_H * s } else { 0.0 };
    // An empty picker still shows one row ("No results") and stays open, so
    // a filter that matches nothing doesn't make the modal vanish.
    let visible_rows = if row_count == 0 && !reserve_empty_row {
        0
    } else {
        row_count.min(max_rows.clamp(1, MAX_ROWS)).max(1)
    };
    // Lock to the composer's width and x position so the popover lines up
    // edge-to-edge with the input chrome.
    let width = input_rect[2];
    let height = title_h + visible_rows as f32 * row_h + footer_h;
    let x = input_rect[0];
    let y = (input_rect[1] - height - 6.0 * s).max(min_y);
    Some([x, y, width, height])
}

pub fn render(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
) -> Option<InlinePickerRenderState> {
    render_limited(
        sugarloaf,
        view,
        input_rect,
        theme,
        scale,
        MAX_ROWS,
        8.0 * scale.clamp(0.5, 3.0),
    )
}

/// Shared two-bar loading rows, bounded by the picker body.
pub(crate) fn render_loading_skeleton(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    body: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    elapsed: f32,
) {
    let s = scale.clamp(0.5, 3.0);
    let [x, y, width, height] = body;
    let row_h = ROW_H * s;
    let fade_in = (elapsed / 0.18).min(1.0);
    const SKELETON_WIDTHS: [(f32, f32); SKELETON_ROWS] =
        [(0.42, 0.68), (0.58, 0.46), (0.36, 0.60)];
    for (i, (title_frac, desc_frac)) in SKELETON_WIDTHS.iter().enumerate() {
        let row_y = y + i as f32 * row_h;
        if row_y + row_h > y + height + 0.5 {
            break;
        }
        let wave = (elapsed / 1.3 * std::f32::consts::TAU - i as f32 * 0.55).sin();
        let alpha = (0.16 + 0.08 * wave).max(0.04) * fade_in;
        let avail = (width - 44.0 * s).max(0.0);
        sugarloaf.overlay_rounded_rect(
            x + 22.0 * s,
            row_y + 8.0 * s,
            avail * title_frac,
            12.0 * s,
            theme.f32_alpha(theme.muted, alpha),
            DEPTH,
            6.0 * s,
            ORDER + 2,
        );
        sugarloaf.overlay_rounded_rect(
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
}

#[allow(clippy::too_many_arguments)]
pub fn render_limited(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
) -> Option<InlinePickerRenderState> {
    render_limited_with_disabled(
        sugarloaf,
        view,
        input_rect,
        theme,
        scale,
        max_rows,
        min_y,
        &[],
    )
}

/// Render disabled choices as ordinary two-line items with muted titles, not
/// section headers. Indices refer to `view.rows`; hosts still own activation.
/// The separate slice avoids changing every existing picker row initializer.
#[allow(clippy::too_many_arguments)]
pub fn render_limited_with_disabled(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
    disabled_rows: &[usize],
) -> Option<InlinePickerRenderState> {
    render_limited_content(sugarloaf, view, input_rect, theme, scale, max_rows, min_y, disabled_rows, None, &[], None)
}

/// Activity detail is optional so ordinary picker initializers remain unchanged.
#[allow(clippy::too_many_arguments)]
pub fn render_limited_with_activity(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
    activity: Option<InlinePickerActivity<'_>>,
) -> Option<InlinePickerRenderState> {
    render_limited_content(sugarloaf, view, input_rect, theme, scale, max_rows, min_y, &[], activity, &[], None)
}

/// Optional presentation slots; absent icons preserve existing picker geometry.
#[allow(clippy::too_many_arguments)]
pub fn render_limited_with_icons(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
    activity: Option<InlinePickerActivity<'_>>,
    row_icons: &[Option<InlinePickerIcon>],
    header_icon: Option<InlinePickerIcon>,
) -> Option<InlinePickerRenderState> {
    render_limited_content(sugarloaf, view, input_rect, theme, scale, max_rows, min_y, &[], activity, row_icons, header_icon)
}

#[allow(clippy::too_many_arguments)]
fn render_limited_content(
    sugarloaf: &mut dyn crate::primitives::paint::Paint,
    view: InlinePickerView<'_>,
    input_rect: [f32; 4],
    theme: &IdeTheme,
    scale: f32,
    max_rows: usize,
    min_y: f32,
    disabled_rows: &[usize],
    activity: Option<InlinePickerActivity<'_>>,
    row_icons: &[Option<InlinePickerIcon>],
    header_icon: Option<InlinePickerIcon>,
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
    let reserve_empty_row = view.loading || (activity.is_none() && view.empty_message.is_some());
    let title_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.fg),
        bold: true,
        ..DrawOpts::default()
    };
    let search_opts = DrawOpts {
        font_size: 13.0 * s,
        color: theme.u8(theme.fg),
        ..DrawOpts::default()
    };
    let mut title_lines = Vec::new();
    let mut query_lines = Vec::new();
    let mut header_h = title_h;
    let mut available_rows = max_rows.clamp(1, MAX_ROWS);
    let mut activity_lines = Vec::new();
    if let Some(activity) = activity {
        let text = format!("{}\n{}", activity.body, activity.status);
        activity_lines = wrap_prompt_text(&text, (input_rect[2] - 48.0 * s).max(1.0), |part| {
            sugarloaf.overlay_text_mut().measure(part, &search_opts)
        });
        // Keep every action visible when possible. Bound verbose failures to
        // the latest wrapped lines (where connection errors usually end), not
        // a single ellipsized search row. Mark any omitted earlier context.
        let actions_h = layout_rows.min(available_rows).max(1) as f32 * row_h;
        let input_h = if activity.show_input { 22.0 * s } else { 0.0 };
        let link_h = if activity.authorization_url.is_some() { 24.0 * s } else { 0.0 };
        let room = (input_rect[1] - min_y - 40.0 * s - actions_h - footer_h - 6.0 * s - input_h - link_h).max(18.0 * s);
        let line_cap = ((room / (18.0 * s)).floor() as usize).clamp(1, 12);
        if activity_lines.len() > line_cap {
            activity_lines = activity_lines.split_off(activity_lines.len() - line_cap);
            if line_cap > 1 {
                activity_lines[0] = "Earlier detail omitted; latest status below".into();
            }
        }
        header_h = (36.0 + activity_lines.len() as f32 * 18.0) * s + input_h + link_h;
        available_rows = row_limit_for_space(
            input_rect[1] - (header_h - title_h), min_y, s, has_footer, available_rows,
        );
    } else if view.wrap_prompt {
        let title_w = (input_rect[2] - 62.0 * s).max(1.0);
        let query_w = (input_rect[2] - 32.0 * s).max(1.0);
        title_lines = wrap_prompt_text(view.title, title_w, |part| {
            sugarloaf.overlay_text_mut().measure(part, &title_opts)
        });
        query_lines = wrap_answer_text(view.query, query_w, |part| {
            sugarloaf.overlay_text_mut().measure(part, &search_opts)
        });
        // Keep a list row and the footer visible on short panes. The answer
        // field shows its latest lines so the caret never disappears.
        let extra_space =
            (input_rect[1] - min_y - title_h - ROW_H * s - footer_h - 6.0 * s).max(0.0);
        let extra_lines = (extra_space / (18.0 * s)).floor() as usize;
        let title_count = title_lines.len().min(extra_lines.saturating_add(1).min(6));
        let query_count = query_lines.len().min(
            extra_lines
                .saturating_sub(title_count - 1)
                .saturating_add(1)
                .min(5),
        );
        let title_overflows = title_count < title_lines.len();
        title_lines.truncate(title_count);
        if title_overflows {
            let last = title_lines.last_mut().expect("question has a title line");
            *last = truncate_to_pixel_width(
                sugarloaf,
                &format!("{last}…"),
                &title_opts,
                title_w,
            );
        }
        query_lines = query_lines.split_off(query_lines.len() - query_count);
        header_h += (title_count + query_count - 2) as f32 * 18.0 * s;
        available_rows = row_limit_for_space(
            input_rect[1] - (header_h - title_h),
            min_y,
            s,
            has_footer,
            available_rows,
        );
    }
    let [x, y, width, height] = if view.wrap_prompt || activity.is_some() {
        let visible = if layout_rows == 0 && !reserve_empty_row {
            0
        } else {
            layout_rows.min(available_rows).max(1)
        };
        let height = header_h + visible as f32 * row_h + footer_h;
        [
            input_rect[0],
            (input_rect[1] - height - 6.0 * s).max(min_y),
            input_rect[2],
            height,
        ]
    } else {
        layout_limited_with_empty_row(
            layout_rows,
            input_rect,
            scale,
            has_footer,
            max_rows,
            min_y,
            reserve_empty_row,
        )?
    };
    let visible_rows = if layout_rows == 0 && !reserve_empty_row {
        0
    } else {
        layout_rows.min(available_rows).max(1)
    };
    let selected = view.selected.min(view.rows.len().saturating_sub(1));
    let first = view
        .scroll_offset
        .min(view.rows.len().saturating_sub(visible_rows));
    let header_clip = [x, y, width, header_h];

    // NB: no square backing rect here — a full-bounds `rect` would fill
    // the four corner triangles the rounded rects leave empty, showing as
    // black square corners poking past the rounded ones on themes where
    // `black != bg`. The rounded fills below are the whole container.
    sugarloaf.overlay_rounded_rect(
        x,
        y,
        width,
        height,
        theme.f32(theme.bg),
        DEPTH,
        RADIUS * s,
        ORDER,
    );
    sugarloaf.overlay_rounded_rect(
        x,
        y,
        width,
        height,
        theme.f32(theme.border),
        DEPTH,
        RADIUS * s,
        ORDER + 1,
    );
    sugarloaf.overlay_rounded_rect(
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
    if view.wrap_prompt {
        for (index, line) in title_lines.iter().enumerate() {
            sugarloaf.overlay_text_mut().draw(
                x + 14.0 * s,
                y + (9.0 + index as f32 * 18.0) * s,
                line,
                &DrawOpts {
                    clip_rect: Some(header_clip),
                    ..title_opts
                },
            );
        }
    } else {
        let title_x = x + if header_icon.is_some() { 38.0 } else { 14.0 } * s;
        if let Some(icon) = header_icon {
            icon.draw(sugarloaf, [x + 14.0 * s, y + 8.0 * s, 16.0 * s, 16.0 * s], header_clip, theme.u8(theme.fg));
        }
        let title = header_icon.map(|_| truncate_to_pixel_width(sugarloaf, view.title, &title_opts, (width - (title_x - x) - 48.0 * s).max(1.0)));
        sugarloaf.overlay_text_mut().draw(
            title_x,
            y + 9.0 * s,
            title.as_deref().unwrap_or(view.title),
            &DrawOpts {
                clip_rect: Some(header_clip),
                ..title_opts
            },
        );
    }
    let esc_opts = DrawOpts {
        font_size: 12.0 * s,
        color: theme.u8(theme.muted),
        clip_rect: Some(header_clip),
        ..DrawOpts::default()
    };
    let esc_w = sugarloaf.overlay_text_mut().measure("esc", &esc_opts);
    sugarloaf.overlay_text_mut().draw(
        x + width - 14.0 * s - esc_w,
        y + 9.0 * s,
        "esc",
        &esc_opts,
    );
    if let Some(activity) = activity {
        let text_x = x + if activity.busy { 32.0 } else { 14.0 } * s;
        for (index, line) in activity_lines.iter().enumerate() {
            sugarloaf.overlay_text_mut().draw(
                text_x, y + (31.0 + index as f32 * 18.0) * s, line,
                &DrawOpts { clip_rect: Some(header_clip), ..search_opts },
            );
        }
        if activity.busy {
            let now = web_time::SystemTime::now().duration_since(web_time::UNIX_EPOCH).unwrap_or_default();
            let phase = crate::render_policy::animation_phase_from_unix_secs(now.as_secs(), now.subsec_nanos());
            for (trail, alpha) in crate::render_policy::RUNNING_SQUARE_ALPHAS.into_iter().enumerate() {
                let (dx, dy) = crate::render_policy::running_square_position(phase, trail, s);
                sugarloaf.overlay_rounded_rect(
                    x + 19.0 * s + dx - s, y + 37.5 * s + dy - s,
                    2.0 * s, 2.0 * s, theme.f32_alpha(theme.fg, alpha), DEPTH, 0.0, ORDER + 3,
                );
            }
        }
    }
    if let Some(detail) = activity.filter(|detail| detail.authorization_url.is_some()) {
        let link = activity_link_rect([x, y, width, height], header_h, s, detail.show_input);
        sugarloaf.overlay_text_mut().draw(link[0], link[1] + 3.0 * s, "Open sign-in page", &DrawOpts {
            font_size: 12.0 * s, color: theme.u8(theme.accent), clip_rect: Some(link), ..DrawOpts::default()
        });
    }
    if activity.is_none() && view.rename.is_some() {
        let rename = view.rename.unwrap();
        // Inline rename editor takes over the search row: an accent label
        // plus the live buffer with a trailing caret block.
        let label_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.cyan),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        sugarloaf.overlay_text_mut().draw(
            x + 14.0 * s,
            y + 31.0 * s,
            "Rename ›",
            &label_opts,
        );
        let label_w = sugarloaf
            .overlay_text_mut()
            .measure("Rename › ", &label_opts);
        let buffer = format!("{rename}▏");
        sugarloaf.overlay_text_mut().draw(
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
    } else if activity.is_none_or(|detail| detail.show_input) {
        let query_y = if activity.is_some() { y + header_h - 22.0 * s } else { y + (31.0 + title_lines.len().saturating_sub(1) as f32 * 18.0) * s };
        let answer_clip = if view.wrap_prompt || activity.is_some() {
            [x + 14.0 * s, query_y, (width - 28.0 * s).max(0.0), (y + header_h - query_y).max(0.0)]
        } else {
            header_clip
        };
        let search_opts = DrawOpts {
            font_size: 13.0 * s,
            color: theme.u8(theme.fg),
            clip_rect: Some(answer_clip),
            ..DrawOpts::default()
        };
        // Caret follows the last visible answer line, even if earlier lines
        // were dropped to fit above the composer.
        let caret_x = if view.wrap_prompt {
            for (index, line) in query_lines.iter().enumerate() {
                sugarloaf.overlay_text_mut().draw(
                    x + 14.0 * s,
                    query_y + index as f32 * 18.0 * s,
                    line,
                    &search_opts,
                );
            }
            if view.query.is_empty() {
                sugarloaf.overlay_text_mut().draw(
                    x + 19.0 * s,
                    query_y,
                    view.search_placeholder,
                    &DrawOpts {
                        color: theme.u8(theme.muted),
                        ..search_opts
                    },
                );
            }
            x + 14.0 * s
                + sugarloaf.overlay_text_mut().measure(
                    query_lines.last().map(String::as_str).unwrap_or(""),
                    &search_opts,
                )
        } else if view.query.is_empty() {
            sugarloaf.overlay_text_mut().draw(
                x + 14.0 * s + 5.0 * s,
                query_y,
                view.search_placeholder,
                &DrawOpts {
                    color: theme.u8(theme.muted),
                    ..search_opts
                },
            );
            x + 14.0 * s
        } else {
            sugarloaf.overlay_text_mut().draw(
                x + 14.0 * s,
                query_y,
                view.query,
                &search_opts,
            );
            x + 14.0 * s
                + sugarloaf
                    .overlay_text_mut()
                    .measure(view.query, &search_opts)
        };
        if view.show_search_caret {
            // Caret sits ON the search line (same Y as the query/placeholder
            // text at `y + 31*s`), a short bar ~ the search font size — NOT a
            // tall bar floating up into the title row. `caret_x` already
            // tracks the measured query width, so it advances as you type.
            let caret_w = (1.5 * s).max(1.0);
            let rect = [
                caret_x + 1.0 * s,
                if view.wrap_prompt {
                    query_y + query_lines.len().saturating_sub(1) as f32 * 18.0 * s
                } else {
                    query_y
                },
                caret_w,
                13.0 * s,
            ];
            let rect = if view.wrap_prompt { answer_caret_rect(rect, answer_clip) } else { Some(rect) };
            if let Some([cx, cy, cw, ch]) = rect {
                sugarloaf.overlay_rounded_rect(
                    cx, cy, cw, ch, theme.f32(theme.accent), DEPTH, 0.0, ORDER + 3,
                );
            }
        }
    }

    let list_y = y + header_h;
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
        let disabled = disabled_rows.contains(&absolute_ix);
        let selected_row = absolute_ix == selected && !disabled;
        let title_opts = DrawOpts {
            color: theme.u8(if disabled { theme.muted } else { theme.fg }),
            ..title_opts.clone()
        };
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
            sugarloaf.overlay_text_mut().draw(
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
            sugarloaf.overlay_rounded_rect(
                x + 6.0 * s,
                visible_y + 3.0 * s,
                width - 12.0 * s,
                (visible_h - 6.0 * s).max(0.0),
                theme.f32(theme.hover),
                DEPTH,
                9.0 * s,
                ORDER + 3,
            );
            sugarloaf.overlay_rounded_rect(
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
                sugarloaf.overlay_rounded_rect(
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
        let icon = row_icons.get(absolute_ix).copied().flatten();
        let title_x = x + if icon.is_some() { 48.0 } else { 22.0 } * s;
        if let Some(icon) = icon {
            icon.draw(sugarloaf, [x + 22.0 * s, row_y + (ROW_H - 18.0) * 0.5 * s, 18.0 * s, 18.0 * s], list_clip, theme.u8(theme.fg));
        }
        let footer_w = if row.footer.is_empty() {
            0.0
        } else {
            sugarloaf
                .overlay_text_mut()
                .measure(row.footer, &footer_opts)
                + 22.0 * s
                + if row.status.is_some() { 12.0 * s } else { 0.0 }
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
            .overlay_text_mut()
            .draw(title_x, title_y, &title_text, &title_opts);
        if !row.description.is_empty() {
            let desc_max_w = (footer_x - title_x - 12.0 * s).max(48.0);
            let desc_text = truncate_to_pixel_width(
                sugarloaf,
                row.description,
                &desc_opts,
                desc_max_w,
            );
            sugarloaf.overlay_text_mut().draw(
                title_x,
                row_y + 25.0 * s,
                &desc_text,
                &desc_opts,
            );
        }
        if !row.footer.is_empty() {
            if let Some(status) = row.status {
                let color = match status {
                    InlinePickerStatus::Good => theme.green,
                    InlinePickerStatus::Warning => theme.yellow,
                    InlinePickerStatus::Error => theme.red,
                    InlinePickerStatus::Muted => theme.muted,
                };
                let dot_d = 6.0 * s;
                let dot_x = footer_x - dot_d - 6.0 * s;
                let dot_y = row_y + (row_h - dot_d) / 2.0;
                if dot_y >= list_y && dot_y + dot_d <= list_bottom {
                    sugarloaf.overlay_rounded_rect(
                        dot_x,
                        dot_y,
                        dot_d,
                        dot_d,
                        theme.f32(color),
                        DEPTH,
                        dot_d / 2.0,
                        ORDER + 5,
                    );
                }
            }
            sugarloaf.overlay_text_mut().draw(
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
                sugarloaf.overlay_rounded_rect(
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
    if view.rows.is_empty() && activity.is_none() {
        if view.loading {
            render_loading_skeleton(
                sugarloaf,
                [x, list_y, width, list_bottom - list_y],
                theme,
                s,
                view.loading_elapsed,
            );
        } else if let Some(empty_message) = view.empty_message {
            sugarloaf.overlay_text_mut().draw(
                x + 22.0 * s,
                list_y + (row_h - 14.0 * s) / 2.0 + 7.0 * s,
                empty_message,
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
        sugarloaf.overlay_quad(
            x + s,
            band_y,
            (width - 2.0 * s).max(0.0),
            (footer_h - s).max(0.0),
            theme.f32(theme.bg),
            [0.0, 0.0, (RADIUS - 1.0) * s, (RADIUS - 1.0) * s],
            DEPTH,
            ORDER + 6,
        );
        sugarloaf.overlay_rect(
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
        sugarloaf.overlay_text_mut().draw(
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
        first_row: first,
        visible_rows,
        header_h,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activity_link_geometry_stays_above_manual_input_and_uses_exact_hits_at_all_scales() {
        use crate::panels::agent_pane::state::picker::{NeoismAgentPicker, NeoismAgentPickerKind};
        for scale in [0.5, 1.0, 1.25, 2.0, 3.0] {
            for manual in [false, true] {
                let mut picker = NeoismAgentPicker::new(if manual { NeoismAgentPickerKind::ConnectSecret } else { NeoismAgentPickerKind::Connect }, "Sign in", Vec::new(), 0);
                picker.set_connect_activity("Enter code ABCD", "Waiting for authorization", !manual);
                picker.set_connect_authorization_url("https://example.invalid/oauth?state=opaque");
                let header_h = (36.0 + 18.0 + 24.0 + if manual { 22.0 } else { 0.0 }) * scale;
                let state = InlinePickerRenderState {
                    rect: [10.0, 20.0, 300.0 * scale, header_h], header_h,
                    footer_h: 0.0, first_row: 0, visible_rows: 0, selected_cursor_rect: None,
                };
                let link = state.activity_link_rect(scale, manual);
                let input_top = state.rect[1] + header_h - if manual { 22.0 * scale } else { 0.0 };
                assert!((link[1] + link[3] - input_top).abs() < 0.001);
                picker.set_render_geometry(state, scale, 0.0);
                assert_eq!(picker.authorization_url_at(link[0] + 1.0, link[1] + 1.0).as_deref(), Some("https://example.invalid/oauth?state=opaque"));
                assert!(picker.authorization_url_at(link[0] + 1.0, input_top + 1.0).is_none());
                assert!(picker.empty_message().is_none(), "activity is not an empty results list");
                picker.set_connect_authorization_url("javascript:alert(1)");
                assert!(picker.activity().unwrap().authorization_url.is_none());
                assert!(picker.authorization_url_at(link[0] + 1.0, link[1] + 1.0).is_none());
            }
        }
    }

    #[test]
    fn activity_errors_wrap_without_ellipsis_including_oversized_tokens() {
        let text = "Connection failed\nserver_returned_a_very_long_error_token\nAuthorized, not connected";
        let lines = wrap_prompt_text(text, 12.0, |line| line.graphemes(true).count() as f32);
        assert!(lines.len() > 3);
        assert!(lines.iter().all(|line| line.graphemes(true).count() <= 12));
        assert!(!lines.iter().any(|line| line.contains('…')));
        assert_eq!(lines.concat().replace(' ', ""), text.replace([' ', '\n'], ""));
    }

    #[test]
    fn activity_rows_start_after_measured_header_at_all_scales() {
        for scale in [0.5, 1.0, 1.25, 2.0, 3.0] {
            for line_count in [1, 3, 12] {
                let header_h = (36.0 + line_count as f32 * 18.0) * scale;
                let state = InlinePickerRenderState {
                    rect: [10.0, 20.0, 300.0 * scale, header_h + 92.0 * scale],
                    header_h, footer_h: 0.0, first_row: 0, visible_rows: 2,
                    selected_cursor_rect: None,
                };
                let first = state.row_rect(0, scale);
                let second = state.row_rect(1, scale);
                assert_eq!(first[1], 20.0 + header_h);
                assert_eq!(second[1], first[1] + 46.0 * scale);
                assert_eq!(second[1] + second[3], state.rect[1] + state.rect[3]);
            }
        }
    }

    #[test]
    fn editable_answer_preserves_spaces_and_empty_lines() {
        let measure = |text: &str| text.graphemes(true).count() as f32;
        for text in ["answer", "answer ", "answer  ", "  answer  here ", "   ", "answer \n\n"] {
            let lines = wrap_answer_text(text, 100.0, measure);
            assert_eq!(lines.join("\n"), text);
        }
        assert_eq!(wrap_answer_text("answer ", 100.0, measure)[0].len(), 7);
        assert_eq!(wrap_answer_text("answer  ", 100.0, measure)[0].len(), 8);
        assert_eq!(wrap_answer_text("hello  world", 6.0, measure), ["hello ", " world"]);
    }

    #[test]
    fn editable_answer_wraps_by_measured_width_without_losing_graphemes() {
        let measure = |text: &str| text.graphemes(true).map(|cluster| match cluster {
            "W" => 9.0, "i" => 2.0, " " => 3.0, _ => 6.0,
        }).sum::<f32>();
        let text = "  iii WWW 👩‍💻👩‍💻👩‍💻 a_very_long_unbroken_answer_token  ".repeat(8);
        for scale in [0.5, 1.0, 1.25, 2.0, 3.0] {
            let width = 64.0 * scale;
            let lines = wrap_answer_text(&text, width, |line| measure(line) * scale);
            assert!(lines.len() > 5);
            assert_eq!(lines.concat(), text);
            assert!(lines.iter().all(|line| measure(line) * scale <= width));
            assert!(lines.last().unwrap().ends_with(' '));
            assert!(lines.iter().all(|line| !line.ends_with('\u{200d}') && !line.starts_with('\u{200d}')));
        }
    }

    #[test]
    fn editable_answer_caret_stays_inside_its_field_at_all_scales() {
        for scale in [0.5, 1.0, 1.25, 2.0, 3.0] {
            let clip = [20.0 * scale, 40.0 * scale, 200.0 * scale, 54.0 * scale];
            let rect = answer_caret_rect(
                [1000.0 * scale, 58.0 * scale, (1.5 * scale).max(1.0), 13.0 * scale], clip,
            ).unwrap();
            assert!(rect[0] >= clip[0]);
            assert!(rect[0] + rect[2] <= clip[0] + clip[2]);
            assert!(rect[1] + rect[3] <= clip[1] + clip[3]);
        }
    }

    #[test]
    fn question_header_wraps_words_long_tokens_and_hard_breaks() {
        let text = "The subagent accidentally formatted unrelated files. May I what?\nYour answer";
        let lines = wrap_prompt_text(text, 20.0, |s| s.graphemes(true).count() as f32);
        assert_eq!(
            lines,
            [
                "The subagent",
                "accidentally",
                "formatted unrelated",
                "files. May I what?",
                "Your answer"
            ]
        );
        assert!(lines.iter().all(|line| line.graphemes(true).count() <= 20));
        let token =
            wrap_prompt_text("👩‍💻👩‍💻👩‍💻abcdef", 2.0, |s| {
                s.graphemes(true).count() as f32
            });
        assert_eq!(token, ["👩‍💻👩‍💻", "👩‍💻a", "bc", "de", "f"]);
    }

    #[test]
    fn question_row_hitbox_starts_below_wrapped_header() {
        let state = InlinePickerRenderState {
            rect: [20.0, 40.0, 300.0, 200.0],
            selected_cursor_rect: None,
            footer_h: 26.0,
            first_row: 0,
            visible_rows: 2,
            header_h: 90.0,
        };
        assert_eq!(state.row_rect(1, 1.0), [20.0, 176.0, 300.0, 46.0]);
    }

    #[test]
    fn limited_picker_stays_below_pane_chrome_and_above_composer() {
        let input = [100.0, 640.0, 700.0, 100.0];
        let pane_content_top = 96.0;
        let rows =
            row_limit_for_space(input[1], pane_content_top, 1.0, false, DEFAULT_MAX_ROWS);
        let card =
            layout_limited(DEFAULT_MAX_ROWS, input, 1.0, false, rows, pane_content_top)
                .expect("picker layout");

        assert!(card[1] >= pane_content_top);
        assert!(card[1] + card[3] + 6.0 <= input[1]);
    }

    #[test]
    fn limited_picker_shrinks_instead_of_crossing_pane_tab_strip() {
        let input = [20.0, 360.0, 420.0, 80.0];
        let pane_content_top = 180.0;
        let rows =
            row_limit_for_space(input[1], pane_content_top, 1.0, true, DEFAULT_MAX_ROWS);
        let card =
            layout_limited(DEFAULT_MAX_ROWS, input, 1.0, true, rows, pane_content_top)
                .expect("picker layout");

        assert!(rows < DEFAULT_MAX_ROWS);
        assert!(card[1] >= pane_content_top);
        assert!(card[1] + card[3] + 6.0 <= input[1]);
    }

    #[test]
    fn input_only_picker_does_not_reserve_a_fake_result_row() {
        let input = [20.0, 360.0, 420.0, 80.0];
        let card = layout_limited_with_empty_row(0, input, 1.0, false, 8, 8.0, false)
            .expect("input-only picker layout");

        assert_eq!(card[3], TITLE_H);
    }
}
