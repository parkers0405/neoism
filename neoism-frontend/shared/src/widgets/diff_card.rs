// Reusable GitHub-style diff card. Renders a single file's diff as a
// rounded-top card: header (filename + +N/-N stat badges)
// over a body of line-numbered, syntax-highlighted diff text with
// green/red row tints for adds/removes.
//
// Used by `git_diff_panel` for the bottom (selected file's diff) card.
// The same chrome look is reused inline in the panel for the top file
// list card so the two slabs read as one family.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::primitives::truncate_to_fit;
use crate::primitives::IdeTheme;
use crate::syntax::{highlight_line, syn_color, Lang, SynTok};

pub const HEADER_HEIGHT: f32 = 30.0;
pub const LINE_HEIGHT: f32 = 18.0;
pub const FONT_SIZE: f32 = 12.5;
pub const GUTTER_FONT_SIZE: f32 = 12.5;
pub const HEADER_FONT_SIZE: f32 = 12.5;
pub const BADGE_FONT_SIZE: f32 = 10.5;
pub const CARD_RADIUS: f32 = 8.0;
pub const HEADER_PAD_X: f32 = 12.0;
pub const BODY_PAD_X: f32 = 8.0;
pub const GUTTER_WIDTH: f32 = 30.0;
pub const BODY_TOP_PAD: f32 = 4.0;
pub const BODY_BOTTOM_PAD: f32 = 6.0;

const DIFF_WRAP_CACHE_LIMIT: usize = 16384;
const DIFF_HIGHLIGHT_CACHE_LIMIT: usize = 8192;

thread_local! {
    static DIFF_WRAP_CACHE: RefCell<DiffWrapCache> = RefCell::new(DiffWrapCache::new());
    static DIFF_HIGHLIGHT_CACHE: RefCell<DiffHighlightCache> =
        RefCell::new(DiffHighlightCache::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiffWrapKey {
    text_hash: u64,
    text_len: usize,
    max_chars: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct DiffHighlightKey {
    text_hash: u64,
    text_len: usize,
    lang: u8,
}

struct DiffWrapCache {
    values: HashMap<DiffWrapKey, Rc<Vec<String>>>,
    order: VecDeque<DiffWrapKey>,
}

impl DiffWrapCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &DiffWrapKey) -> Option<Rc<Vec<String>>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: DiffWrapKey, value: Rc<Vec<String>>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key);
        self.values.insert(key, value);
        while self.order.len() > DIFF_WRAP_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

struct DiffHighlightCache {
    values: HashMap<DiffHighlightKey, Rc<Vec<(SynTok, String)>>>,
    order: VecDeque<DiffHighlightKey>,
}

impl DiffHighlightCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &DiffHighlightKey) -> Option<Rc<Vec<(SynTok, String)>>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: DiffHighlightKey, value: Rc<Vec<(SynTok, String)>>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key);
        self.values.insert(key, value);
        while self.order.len() > DIFF_HIGHLIGHT_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum DiffLineKind {
    Hunk,
    Add,
    Remove,
    Context,
}

#[derive(Clone, Debug, Hash)]
pub struct DiffLine {
    pub text: String,
    pub kind: DiffLineKind,
    /// Display line number for this diff row. Context rows use the new-file
    /// line, add rows use the new-file line, and remove rows use the old-file
    /// line so row-by-row replacements can show `53 old` followed by `53 new`.
    pub line_number: Option<u32>,
    pub old_line_number: Option<u32>,
    pub new_line_number: Option<u32>,
}

/// Snapshot the panel passes in each frame.
pub struct CardSpec<'a> {
    pub path: &'a str,
    pub link_target: Option<&'a str>,
    pub link_hovered: bool,
    pub additions: u32,
    pub deletions: u32,
    /// Language detected from `path`. Drives syntax highlighting in
    /// the body so the diff reads like a real code block instead of
    /// flat text.
    pub lang: Lang,
    pub diff_lines: &'a [DiffLine],
    pub visual_row_offsets: Option<&'a [usize]>,
    /// Caller-managed body scroll offset (logical pixels). 0 means
    /// the first hunk sits flush below the header.
    pub body_scroll: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct CardLayout {
    pub header_height: f32,
    pub body_height: f32,
    pub total_height: f32,
}

/// Compute the card's full height when nothing is clipped.
pub fn measure(_spec: &CardSpec, scale: f32, available_body_height: f32) -> CardLayout {
    let header_h = HEADER_HEIGHT * scale;
    let body_h = available_body_height.max(0.0);
    CardLayout {
        header_height: header_h,
        body_height: body_h,
        total_height: header_h + body_h,
    }
}

pub fn body_text_width(card_width: f32, scale: f32) -> f32 {
    let gutter_w = GUTTER_WIDTH * scale;
    (card_width - gutter_w - BODY_PAD_X * 2.0 * scale).max(0.0)
}

pub fn visual_row_count(diff_lines: &[DiffLine], body_width: f32, scale: f32) -> usize {
    visual_row_offsets(diff_lines, body_width, scale)
        .last()
        .copied()
        .unwrap_or(0)
        .max(1)
}

pub fn visual_row_offsets(
    diff_lines: &[DiffLine],
    body_width: f32,
    scale: f32,
) -> Vec<usize> {
    let max_chars = wrap_chars_for_width(body_width, scale);
    let mut offsets = Vec::with_capacity(diff_lines.len() + 1);
    let mut visual_rows = 0usize;
    offsets.push(0);
    for line in diff_lines {
        visual_rows += wrapped_fragments(&line.text, max_chars).len();
        offsets.push(visual_rows);
    }
    offsets
}

pub fn warm_render_cache(
    diff_lines: &[DiffLine],
    body_width: f32,
    scale: f32,
    lang: Lang,
) -> Vec<usize> {
    let max_chars = wrap_chars_for_width(body_width, scale);
    let mut offsets = Vec::with_capacity(diff_lines.len() + 1);
    let mut visual_rows = 0usize;
    offsets.push(0);
    for line in diff_lines {
        let fragments = wrapped_fragments(&line.text, max_chars);
        visual_rows += fragments.len();
        offsets.push(visual_rows);
        if line.kind == DiffLineKind::Hunk {
            continue;
        }
        for fragment in fragments.iter() {
            highlighted_diff_line(fragment, lang);
        }
    }
    offsets
}

fn first_diff_line_for_visual_row(offsets: &[usize], visual_row: usize) -> usize {
    if offsets.len() < 2 {
        return 0;
    }
    offsets
        .partition_point(|&row| row <= visual_row)
        .saturating_sub(1)
        .min(offsets.len() - 2)
}

fn visual_row_start_for_line(offsets: Option<&[usize]>, line_index: usize) -> usize {
    offsets
        .and_then(|offsets| offsets.get(line_index).copied())
        .unwrap_or(0)
}

fn fallback_visual_row_count(
    diff_lines: &[DiffLine],
    body_width: f32,
    scale: f32,
) -> usize {
    let max_chars = wrap_chars_for_width(body_width, scale);
    diff_lines
        .iter()
        .map(|line| wrapped_fragments(&line.text, max_chars).len())
        .sum::<usize>()
        .max(1)
}

/// Paint the card. `clip_top` / `clip_bottom` is the panel viewport in
/// window-logical coords; rows entirely outside that range are skipped.
#[allow(clippy::too_many_arguments)]
pub fn render(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    width: f32,
    body_height: f32,
    spec: &CardSpec,
    scale: f32,
    theme: &IdeTheme,
    depth: f32,
    base_order: u8,
    clip_top: f32,
    clip_bottom: f32,
) -> CardLayout {
    let layout = measure(spec, scale, body_height);
    let header_h = layout.header_height;
    let body_h = layout.body_height;
    let total_h = layout.total_height;

    if y + total_h < clip_top || y > clip_bottom {
        return layout;
    }

    let radius = CARD_RADIUS * scale;
    let top_clipped = y < clip_top - 0.5;
    let bottom_clipped = y + total_h > clip_bottom + 0.5;
    let stroke = (2.0 * scale).max(2.0);

    // ── Border ring ──────────────────────────────────────────────────
    // Drawn first as a slightly larger rounded backing in `theme.border`
    // so the header + body fills above it leave a clean stroke
    // around the whole card. Mirrors the way `file_tree::render` builds
    // its frame (surface outer + bg inner) — same trick at card scale.
    let border_x = x - stroke;
    let border_y = y - stroke;
    let border_w = width + stroke * 2.0;
    let border_h = total_h + stroke * 2.0;
    let border_radius = radius + stroke;
    let border_radii = match (top_clipped, bottom_clipped) {
        (true, true) => [0.0, 0.0, 0.0, 0.0],
        (true, false) => [0.0, 0.0, border_radius, border_radius],
        (false, true) => [border_radius, border_radius, 0.0, 0.0],
        (false, false) => [border_radius, border_radius, border_radius, border_radius],
    };
    let border_visible_y = border_y.max(clip_top);
    let border_visible_bot = (border_y + border_h).min(clip_bottom);
    let border_visible_h = (border_visible_bot - border_visible_y).max(0.0);
    if border_visible_h > 0.0 {
        sugarloaf.quad(
            None,
            border_x,
            border_visible_y,
            border_w,
            border_visible_h,
            theme.f32(theme.border),
            border_radii,
            depth,
            base_order,
        );
    }

    let header_radii = if top_clipped {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [radius, radius, 0.0, 0.0]
    };

    let header_visible_y = y.max(clip_top);
    let header_visible_bot = (y + header_h).min(clip_bottom);
    let header_visible_h = (header_visible_bot - header_visible_y).max(0.0);
    if header_visible_h > 0.0 {
        sugarloaf.quad(
            None,
            x,
            header_visible_y,
            width,
            header_visible_h,
            theme.f32(theme.surface),
            header_radii,
            depth,
            base_order + 1,
        );
    }

    // ── Header text ──────────────────────────────────────────────────
    // Text clipping must follow the same visibility test as the header
    // fill. A zero-height text clip can still submit glyphs on some
    // backends, which lets scrolled-off card titles bleed into top chrome.
    if header_visible_h > 0.0 {
        let header_clip = clip_to_viewport(x, y, width, header_h, clip_top, clip_bottom);
        let path_color = if spec.link_target.is_some() {
            theme.blue
        } else {
            theme.fg
        };
        let path_opts = DrawOpts {
            font_size: HEADER_FONT_SIZE * scale,
            color: theme.u8(path_color),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let add_badge_opts = DrawOpts {
            font_size: BADGE_FONT_SIZE * scale,
            color: theme.u8(theme.green),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };
        let del_badge_opts = DrawOpts {
            font_size: BADGE_FONT_SIZE * scale,
            color: theme.u8(theme.red),
            bold: true,
            clip_rect: Some(header_clip),
            ..DrawOpts::default()
        };

        let header_text_y =
            snap_text_y(y + (header_h - HEADER_FONT_SIZE * scale) / 2.0 - 1.0 * scale);
        let hx = x + HEADER_PAD_X * scale;

        let add_text = if spec.additions > 0 {
            format!("+{}", spec.additions)
        } else {
            String::new()
        };
        let del_text = if spec.deletions > 0 {
            format!("-{}", spec.deletions)
        } else {
            String::new()
        };
        let add_w = if add_text.is_empty() {
            0.0
        } else {
            sugarloaf
                .text_mut()
                .measure(add_text.as_str(), &add_badge_opts)
        };
        let del_w = if del_text.is_empty() {
            0.0
        } else {
            sugarloaf
                .text_mut()
                .measure(del_text.as_str(), &del_badge_opts)
        };
        let badges_total = add_w
            + del_w
            + if !add_text.is_empty() && !del_text.is_empty() {
                8.0 * scale
            } else {
                0.0
            };
        let path_budget =
            (x + width - hx - HEADER_PAD_X * scale - badges_total - 12.0 * scale)
                .max(0.0);
        let path_fit = truncate_to_fit(spec.path, path_budget, sugarloaf, &path_opts);
        let path_w = sugarloaf.text_mut().measure(path_fit.as_str(), &path_opts);
        sugarloaf
            .text_mut()
            .draw(hx, header_text_y, path_fit.as_str(), &path_opts);
        if spec.link_target.is_some() && spec.link_hovered && path_w > 0.0 {
            sugarloaf.quad(
                None,
                hx,
                header_text_y + HEADER_FONT_SIZE * scale + 2.0 * scale,
                path_w,
                (1.0 * scale).max(1.0),
                theme.f32(theme.blue),
                [0.0, 0.0, 0.0, 0.0],
                depth,
                base_order + 3,
            );
        }

        let badge_y =
            snap_text_y(y + (header_h - BADGE_FONT_SIZE * scale) / 2.0 - 1.0 * scale);
        let mut bx = x + width - HEADER_PAD_X * scale;
        if !del_text.is_empty() {
            bx -= del_w;
            sugarloaf
                .text_mut()
                .draw(bx, badge_y, del_text.as_str(), &del_badge_opts);
        }
        if !add_text.is_empty() {
            if !del_text.is_empty() {
                bx -= 8.0 * scale;
            }
            bx -= add_w;
            sugarloaf
                .text_mut()
                .draw(bx, badge_y, add_text.as_str(), &add_badge_opts);
        }
    }

    // ── Body ─────────────────────────────────────────────────────────
    if spec.diff_lines.is_empty() {
        return layout;
    }

    let body_top = y + header_h;
    let body_visible_y = body_top.max(clip_top);
    let body_visible_bot = (body_top + body_h).min(clip_bottom);
    let body_visible_h = (body_visible_bot - body_visible_y).max(0.0);
    let body_radii = if bottom_clipped {
        [0.0, 0.0, 0.0, 0.0]
    } else {
        [0.0, 0.0, radius, radius]
    };
    if body_visible_h > 0.0 {
        sugarloaf.quad(
            None,
            x,
            body_visible_y,
            width,
            body_visible_h,
            theme.f32(theme.bg),
            body_radii,
            depth,
            base_order + 1,
        );
    }

    let line_h = LINE_HEIGHT * scale;
    let body_clip = clip_to_viewport(x, body_top, width, body_h, clip_top, clip_bottom);
    let gutter_w = GUTTER_WIDTH * scale;
    let body_inner_x = x + gutter_w + BODY_PAD_X * scale;
    let body_inner_right = x + width - BODY_PAD_X * scale;

    let body_text_w = body_text_width(width, scale);
    let max_chars = wrap_chars_for_width(body_text_w, scale);
    let scroll = spec.body_scroll.max(0.0);
    let row_top_bound = body_top.max(clip_top);
    let row_bot_bound = (body_top + body_h).min(clip_bottom);
    let scroll_first_visual = (scroll / line_h) as usize;
    let clip_first_visual = ((row_top_bound - body_top - BODY_TOP_PAD * scale + scroll)
        / line_h)
        .floor()
        .max(0.0) as usize;
    let first_visible_visual =
        scroll_first_visual.max(clip_first_visual.saturating_sub(1));
    let visible_rows =
        (((row_bot_bound - row_top_bound).max(0.0) / line_h).ceil() as usize).max(1);
    let last_visible_visual = first_visible_visual + visible_rows + 4;
    let total_visual_rows = spec
        .visual_row_offsets
        .and_then(|offsets| offsets.last().copied())
        .unwrap_or_else(|| fallback_visual_row_count(spec.diff_lines, body_text_w, scale))
        .max(1);
    let logical_start = spec
        .visual_row_offsets
        .map(|offsets| first_diff_line_for_visual_row(offsets, first_visible_visual))
        .unwrap_or(0);

    let mut visual_row_ix =
        visual_row_start_for_line(spec.visual_row_offsets, logical_start);
    for line in &spec.diff_lines[logical_start..] {
        let fragments = wrapped_fragments(&line.text, max_chars);
        if visual_row_ix + fragments.len() <= first_visible_visual {
            visual_row_ix += fragments.len();
            continue;
        }
        if visual_row_ix > last_visible_visual || visual_row_ix > total_visual_rows {
            break;
        }

        for (fragment_ix, fragment) in fragments.iter().enumerate() {
            let row_y = body_top
                + BODY_TOP_PAD * scale
                + (visual_row_ix + fragment_ix) as f32 * line_h
                - scroll;
            let row_bot = row_y + line_h;
            if row_bot < row_top_bound || row_y > row_bot_bound {
                continue;
            }
            let visible_y = row_y.max(row_top_bound);
            let visible_h = row_bot.min(row_bot_bound) - visible_y;
            if visible_h <= 0.0 {
                continue;
            }

            // Row background tint.
            let bg_color = match line.kind {
                DiffLineKind::Add => Some(theme.f32_alpha(theme.green, 0.18)),
                DiffLineKind::Remove => Some(theme.f32_alpha(theme.red, 0.20)),
                DiffLineKind::Hunk => Some(theme.f32_alpha(theme.surface, 0.45)),
                DiffLineKind::Context => None,
            };
            if let Some(bg) = bg_color {
                sugarloaf.rect(
                    None,
                    x,
                    visible_y,
                    width,
                    visible_h,
                    bg,
                    depth,
                    base_order + 2,
                );
            }

            // Gutter line number. Continuation rows intentionally leave the
            // gutter blank so wrapped code still reads as one logical line.
            let gutter_color = match line.kind {
                DiffLineKind::Add => theme.u8_alpha(theme.green, 0.85),
                DiffLineKind::Remove => theme.u8_alpha(theme.red, 0.85),
                DiffLineKind::Hunk => theme.u8(theme.muted),
                DiffLineKind::Context => theme.u8(theme.muted),
            };
            let gutter_opts = DrawOpts {
                font_size: GUTTER_FONT_SIZE * scale,
                color: gutter_color,
                bold: true,
                clip_rect: Some(body_clip),
                ..DrawOpts::default()
            };
            if fragment_ix == 0 {
                let line_number = match line.kind {
                    DiffLineKind::Remove => line.old_line_number.or(line.line_number),
                    DiffLineKind::Add => line.new_line_number.or(line.line_number),
                    DiffLineKind::Context => line.new_line_number.or(line.line_number),
                    DiffLineKind::Hunk => line.line_number,
                };
                if let Some(n) = line_number {
                    let s_str = n.to_string();
                    let w = sugarloaf.text_mut().measure(s_str.as_str(), &gutter_opts);
                    let gutter_text_y =
                        snap_text_y(row_y + (line_h - GUTTER_FONT_SIZE * scale) / 2.0);
                    sugarloaf.text_mut().draw(
                        x + gutter_w - w - 6.0 * scale,
                        gutter_text_y,
                        s_str.as_str(),
                        &gutter_opts,
                    );
                }
            }

            // Body text. Hunk + remove rows render flat (single colour);
            // add + context rows get syntax highlighting so the code reads
            // like a buffer rather than a wall of monochrome diff text.
            let text_y = snap_text_y(row_y + (line_h - FONT_SIZE * scale) / 2.0);
            match line.kind {
                DiffLineKind::Hunk => {
                    let opts = DrawOpts {
                        font_size: FONT_SIZE * scale,
                        color: theme.u8(theme.muted),
                        italic: true,
                        clip_rect: Some(body_clip),
                        ..DrawOpts::default()
                    };
                    let body_budget = (body_inner_right - body_inner_x).max(0.0);
                    let fit = truncate_to_fit(fragment, body_budget, sugarloaf, &opts);
                    let _ = sugarloaf.text_mut().draw(
                        body_inner_x,
                        text_y,
                        fit.as_str(),
                        &opts,
                    );
                }
                DiffLineKind::Remove => {
                    draw_syntax_line(
                        sugarloaf,
                        body_inner_x,
                        text_y,
                        fragment,
                        spec.lang,
                        theme,
                        body_clip,
                        body_inner_right,
                        scale,
                        Some(theme.u8(theme.red)),
                    );
                }
                DiffLineKind::Add | DiffLineKind::Context => {
                    draw_syntax_line(
                        sugarloaf,
                        body_inner_x,
                        text_y,
                        fragment,
                        spec.lang,
                        theme,
                        body_clip,
                        body_inner_right,
                        scale,
                        None,
                    );
                }
            }
        }
        visual_row_ix += fragments.len();
    }

    layout
}

/// Render a single source line with the shared syntax highlighter.
/// Tokens get themed colours; if `tint` is `Some`, every token is
/// drawn in that tint instead (used for remove lines so red is
/// preserved end-to-end).
#[allow(clippy::too_many_arguments)]
fn draw_syntax_line(
    sugarloaf: &mut Sugarloaf,
    start_x: f32,
    text_y: f32,
    text: &str,
    lang: Lang,
    theme: &IdeTheme,
    clip: [f32; 4],
    right_edge: f32,
    scale: f32,
    tint: Option<[u8; 4]>,
) {
    let mut cursor_x = start_x;
    let spans = highlighted_diff_line(text, lang);
    for (tok, slice) in spans.iter() {
        let color = tint.unwrap_or_else(|| syn_color(*tok, theme, false));
        let opts = DrawOpts {
            font_size: FONT_SIZE * scale,
            color,
            bold: true,
            clip_rect: Some(clip),
            ..DrawOpts::default()
        };
        if cursor_x >= right_edge {
            return;
        }
        // Truncate any single token that would overflow the row.
        let budget = (right_edge - cursor_x).max(0.0);
        let measured = sugarloaf.text_mut().measure(slice.as_str(), &opts);
        if measured <= budget {
            cursor_x +=
                sugarloaf
                    .text_mut()
                    .draw(cursor_x, text_y, slice.as_str(), &opts);
        } else {
            let fit = truncate_to_fit(slice.as_str(), budget, sugarloaf, &opts);
            let _ = sugarloaf
                .text_mut()
                .draw(cursor_x, text_y, fit.as_str(), &opts);
            return;
        }
    }
}

fn wrap_chars_for_width(width: f32, scale: f32) -> usize {
    let approx_char_w = (FONT_SIZE * scale * 0.58).max(1.0);
    (width / approx_char_w).floor().max(1.0) as usize
}

fn wrapped_fragments(text: &str, max_chars: usize) -> Rc<Vec<String>> {
    let max_chars = max_chars.max(1);
    let key = DiffWrapKey {
        text_hash: hash_text(text),
        text_len: text.len(),
        max_chars,
    };
    if let Some(hit) = DIFF_WRAP_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    crate::panels::agent_pane::view::derivations::bump_diff_wrap();

    let fragments = Rc::new(wrap_fragments_uncached(text, max_chars));
    DIFF_WRAP_CACHE.with(|cache| cache.borrow_mut().insert(key, fragments.clone()));
    fragments
}

fn wrap_fragments_uncached(text: &str, max_chars: usize) -> Vec<String> {
    let max_chars = max_chars.max(1);
    if text.is_empty() {
        return vec![String::new()];
    }

    let mut out = Vec::new();
    let mut start = 0usize;
    let mut count = 0usize;
    for (ix, _) in text.char_indices() {
        if count == max_chars {
            out.push(text[start..ix].to_string());
            start = ix;
            count = 0;
        }
        count += 1;
    }
    out.push(text[start..].to_string());
    out
}

fn highlighted_diff_line(text: &str, lang: Lang) -> Rc<Vec<(SynTok, String)>> {
    let key = DiffHighlightKey {
        text_hash: hash_text(text),
        text_len: text.len(),
        lang: lang_cache_tag(lang),
    };
    if let Some(hit) = DIFF_HIGHLIGHT_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    crate::panels::agent_pane::view::derivations::bump_diff_highlight();
    let spans = Rc::new(
        highlight_line(text, lang)
            .into_iter()
            .map(|(tok, slice)| (tok, slice.to_string()))
            .collect::<Vec<_>>(),
    );
    DIFF_HIGHLIGHT_CACHE.with(|cache| cache.borrow_mut().insert(key, spans.clone()));
    spans
}

fn lang_cache_tag(lang: Lang) -> u8 {
    match lang {
        Lang::Rust => 1,
        Lang::Javascript => 2,
        Lang::Jsx => 3,
        Lang::Typescript => 4,
        Lang::Tsx => 5,
        Lang::Python => 6,
        Lang::Go => 7,
        Lang::Lua => 8,
        Lang::Toml => 9,
        Lang::Json => 10,
        Lang::Markdown => 11,
        Lang::Other => 12,
        Lang::Nix => 13,
        Lang::Make => 20,
        Lang::Bash => 14,
        Lang::C => 15,
        Lang::Cpp => 16,
        Lang::Yaml => 17,
        Lang::Css => 18,
        Lang::Html => 19,
    }
}

fn hash_text(text: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut hasher);
    hasher.finish()
}

fn snap_text_y(y: f32) -> f32 {
    if y.is_finite() {
        y.round()
    } else {
        y
    }
}

fn clip_to_viewport(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    clip_top: f32,
    clip_bottom: f32,
) -> [f32; 4] {
    let top = y.max(clip_top);
    let bot = (y + h).min(clip_bottom);
    [x, top, w, (bot - top).max(0.0)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn warm_render_cache_offsets_match_visual_row_count() {
        let lines = vec![
            DiffLine {
                text: "fn main() { println!(\"hello\"); }".to_string(),
                kind: DiffLineKind::Context,
                line_number: Some(1),
                old_line_number: Some(1),
                new_line_number: Some(1),
            },
            DiffLine {
                text: "+let very_long_identifier = compute_value_from_many_inputs();"
                    .to_string(),
                kind: DiffLineKind::Add,
                line_number: Some(2),
                old_line_number: None,
                new_line_number: Some(2),
            },
        ];
        let body_w = 90.0;
        let scale = 1.0;

        crate::panels::agent_pane::view::derivations::reset();
        let offsets = warm_render_cache(&lines, body_w, scale, Lang::Rust);
        let counts = crate::panels::agent_pane::view::derivations::take();

        assert_eq!(
            offsets.last().copied().unwrap(),
            visual_row_count(&lines, body_w, scale)
        );
        assert_eq!(offsets.len(), lines.len() + 1);
        assert!(counts.diff_wraps > 0);
        assert!(counts.diff_highlights > 0);
    }
}
