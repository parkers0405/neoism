use std::cell::RefCell;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::ops::Range;
use std::rc::Rc;

use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::NeoismAgentMessage;
use crate::syntax::{Lang, SynTok};

use super::draw::{
    draw_rect_clipped, draw_rounded_rect_clipped, draw_text_clipped,
    draw_top_rounded_rect_clipped, measure_text_cached, opts_with_clip,
};
use super::tool_message::AgentToolPane;
use super::ORDER_PANEL;
use crate::primitives::ide_theme::IdeTheme;

const CODE_LINE_HIGHLIGHT_CACHE_LIMIT: usize = 8192;
const CODE_LINE_RANGE_CACHE_LIMIT: usize = 128;

thread_local! {
    static CODE_LINE_HIGHLIGHT_CACHE: RefCell<CodeLineHighlightCache> =
        RefCell::new(CodeLineHighlightCache::new());
    static CODE_LINE_RANGE_CACHE: RefCell<CodeLineRangeCache> =
        RefCell::new(CodeLineRangeCache::new());
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CodeLineHighlightKey {
    line: String,
    lang: u8,
}

struct CodeLineHighlightCache {
    values: HashMap<CodeLineHighlightKey, Rc<Vec<(SynTok, String)>>>,
    order: VecDeque<CodeLineHighlightKey>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct CodeLineRangeKey {
    message_id: String,
    text_len: usize,
    text_fingerprint: u64,
}

struct CodeLineRangeCache {
    values: HashMap<CodeLineRangeKey, Rc<Vec<Range<usize>>>>,
    order: VecDeque<CodeLineRangeKey>,
}

impl CodeLineHighlightCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &CodeLineHighlightKey) -> Option<Rc<Vec<(SynTok, String)>>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: CodeLineHighlightKey, value: Rc<Vec<(SynTok, String)>>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key.clone());
        self.values.insert(key, value);
        while self.order.len() > CODE_LINE_HIGHLIGHT_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

impl CodeLineRangeCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &CodeLineRangeKey) -> Option<Rc<Vec<Range<usize>>>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: CodeLineRangeKey, value: Rc<Vec<Range<usize>>>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key.clone());
        self.values.insert(key, value);
        while self.order.len() > CODE_LINE_RANGE_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

pub trait AgentCodeMessage {
    fn id(&self) -> &str;
    fn text(&self) -> &str;
    fn lang(&self) -> &str;
    fn line_offset(&self) -> Option<usize>;
}

pub trait AgentCodePane: AgentToolPane {}

impl<T> AgentCodePane for T where T: AgentToolPane {}

pub fn warm_code_block_render_cache(message: &impl AgentCodeMessage) {
    warm_code_text_render_cache(message.id(), message.text(), message.lang());
}

pub fn warm_code_text_render_cache(cache_id: &str, text: &str, lang: &str) {
    let ranges = cached_code_line_ranges(cache_id, text);
    let lang = syntax_lang(lang);
    for range in ranges.iter() {
        highlighted_code_line(&text[range.clone()], lang);
    }
}

pub fn warm_code_lines_render_cache(lines: &[String], lang: &str) {
    let lang = syntax_lang(lang);
    for line in lines {
        highlighted_code_line(line, lang);
    }
}

impl AgentCodeMessage for NeoismAgentMessage {
    fn id(&self) -> &str {
        &self.id
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn lang(&self) -> &str {
        &self.lang
    }

    fn line_offset(&self) -> Option<usize> {
        self.line_offset
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_code_block(
    sugarloaf: &mut Sugarloaf,
    pane: &mut impl AgentCodePane,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    message: &impl AgentCodeMessage,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
    occlusion_rects: &[[f32; 4]],
) {
    if h <= 0.0 || message.text().trim().is_empty() {
        return;
    }
    // Only show line numbers when the server actually parsed real source
    // positions out of the tool output (e.g. `Line 42: …`). Synthesizing
    // "1, 2, 3" for output that has no underlying line numbers is more
    // misleading than helpful.
    let real_line_offset = message.line_offset();
    let body_pad = 14.0 * s;
    let radius = 10.0 * s;
    let border_w = 1.0_f32.max(s);
    let header_h = 30.0 * s;
    let body_y = y + header_h;
    let body_h = (h - header_h).max(0.0);
    let line_ranges = cached_code_line_ranges(message.id(), message.text());
    let line_count = line_ranges.len().max(1);
    let num_text_w = if real_line_offset.is_some() {
        let last_line = real_line_offset.unwrap_or(0) + line_count;
        let digits = digit_count(last_line.max(1));
        // ~7.8px per monospace digit at 12.5pt; rounded up for the trailing
        // space before the code column.
        (digits as f32) * 7.8 * s + 8.0 * s
    } else {
        0.0
    };
    let code_left_pad = body_pad + num_text_w;
    let Some(opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.5 * s,
            color: theme.u8(theme.fg),
            bold: true,
            clip_rect: Some([
                x + code_left_pad,
                body_y,
                (w - code_left_pad - body_pad).max(0.0),
                body_h,
            ]),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    let num_opts = real_line_offset.and_then(|_| {
        opts_with_clip(
            DrawOpts {
                font_size: 12.5 * s,
                color: theme.u8(theme.muted),
                clip_rect: Some([x + body_pad, body_y, num_text_w, body_h]),
                ..DrawOpts::default()
            },
            viewport_clip,
        )
    });
    let lang = syntax_lang(message.lang());
    let first_line_y = body_y + 8.0 * s;
    let code_x = x + code_left_pad;
    let line_h = 18.0 * s;
    let visible_lines =
        (((body_h - 16.0 * s) / line_h).floor().max(1.0) as usize).min(line_count);
    let full_body_h = line_count as f32 * line_h + 16.0 * s;
    let max_scroll = (full_body_h - body_h).max(0.0);
    let scroll_key = format!("code:{}", message.id());
    let internal_scroll = pane.diff_scroll_offset(&scroll_key, max_scroll);
    let suppress_interactions = pane.suppress_tool_interactions();
    if max_scroll > 1.0 && !suppress_interactions {
        pane.register_diff_scroll_rect(scroll_key, [x, body_y, w, body_h], max_scroll);
    }
    let line_offset = (internal_scroll / line_h).floor().max(0.0) as usize;
    let intra_line_offset = internal_scroll - line_offset as f32 * line_h;
    let clip_top = viewport_clip[1];
    let clip_bottom = viewport_clip[1] + viewport_clip[3];
    let start_ix = ((clip_top - first_line_y - line_h) / line_h)
        .floor()
        .max(0.0) as usize;
    let end_ix = ((clip_bottom - first_line_y + line_h) / line_h)
        .ceil()
        .max(0.0) as usize;
    let start_ix = (start_ix + line_offset).min(line_count);
    let end_ix =
        (end_ix + line_offset).min((line_offset + visible_lines + 2).min(line_count));
    if start_ix >= end_ix {
        return;
    }
    // Diff-card style chrome: rounded outer border, subtle header slab,
    // and body surface. The border ring is drawn first, then inset panels.
    draw_rounded_rect_clipped(
        sugarloaf,
        [x, y - 4.0 * s, w, h + 4.0 * s],
        theme.f32(theme.border),
        radius,
        ORDER_PANEL,
        viewport_clip,
    );
    draw_rounded_rect_clipped(
        sugarloaf,
        [
            x + border_w,
            y - 4.0 * s + border_w,
            (w - 2.0 * border_w).max(0.0),
            (h + 4.0 * s - 2.0 * border_w).max(0.0),
        ],
        theme.f32(theme.panel_bg()),
        (radius - border_w).max(0.0),
        ORDER_PANEL + 1,
        viewport_clip,
    );
    draw_top_rounded_rect_clipped(
        sugarloaf,
        [
            x + border_w,
            y - 4.0 * s + border_w,
            (w - 2.0 * border_w).max(0.0),
            header_h,
        ],
        theme.f32(theme.surface),
        (radius - border_w).max(0.0),
        ORDER_PANEL + 2,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [
            x + border_w,
            y + header_h - border_w,
            (w - 2.0 * border_w).max(0.0),
            border_w,
        ],
        theme.f32(theme.border),
        ORDER_PANEL + 4,
        viewport_clip,
    );

    let lang_label = if message.lang().trim().is_empty() {
        "code"
    } else {
        message.lang().trim()
    };
    let Some(header_opts) = opts_with_clip(
        DrawOpts {
            font_size: 12.0 * s,
            color: theme.u8(theme.syn_type),
            bold: true,
            clip_rect: Some([x + body_pad, y, (w - body_pad * 2.0).max(0.0), header_h]),
            ..DrawOpts::default()
        },
        viewport_clip,
    ) else {
        return;
    };
    draw_text_clipped(
        sugarloaf,
        x + body_pad,
        y + 7.0 * s,
        lang_label,
        &header_opts,
        occlusion_rects,
    );
    let copy_label = "copy";
    let mut copy_opts = header_opts;
    copy_opts.color = theme.u8(theme.muted);
    copy_opts.bold = false;
    let copy_w = measure_text_cached(sugarloaf, copy_label, &copy_opts);
    draw_text_clipped(
        sugarloaf,
        x + w - body_pad - copy_w,
        y + 7.0 * s,
        copy_label,
        &copy_opts,
        occlusion_rects,
    );

    let text = message.text();
    for ix in start_ix..end_ix {
        let line = line_ranges
            .get(ix)
            .map(|range| &text[range.clone()])
            .unwrap_or("");
        let visible_ix = ix.saturating_sub(line_offset);
        let line_y = first_line_y + visible_ix as f32 * line_h - intra_line_offset;
        let diff = diff_line_kind(line);
        render_code_line_background(
            sugarloaf,
            x,
            line_y,
            w,
            code_left_pad,
            diff,
            theme,
            s,
            viewport_clip,
        );
        if let (Some(offset), Some(base_num_opts)) = (real_line_offset, num_opts) {
            let mut line_num_opts = base_num_opts;
            if let Some(color) = diff.map(|kind| diff_color(kind, theme)) {
                line_num_opts.color = theme.u8(color);
            }
            let line_no = offset + ix + 1;
            draw_text_clipped(
                sugarloaf,
                x + body_pad,
                line_y,
                &format!("{line_no}"),
                &line_num_opts,
                occlusion_rects,
            );
        }
        render_code_line_text(
            sugarloaf,
            code_x,
            line_y,
            line,
            lang,
            diff,
            &opts,
            theme,
            occlusion_rects,
        );
    }
}

pub fn digit_count(value: usize) -> usize {
    let mut n = value.max(1);
    let mut count = 0;
    while n > 0 {
        n /= 10;
        count += 1;
    }
    count
}

#[derive(Clone, Copy)]
pub enum DiffLineKind {
    Add,
    Remove,
}

pub fn diff_line_kind(line: &str) -> Option<DiffLineKind> {
    let trimmed = line.trim_start();
    if trimmed.starts_with("+++") || trimmed.starts_with("---") {
        return None;
    }
    if trimmed.starts_with('+') {
        Some(DiffLineKind::Add)
    } else if trimmed.starts_with('-') {
        Some(DiffLineKind::Remove)
    } else {
        None
    }
}

pub fn diff_color(kind: DiffLineKind, theme: &IdeTheme) -> u32 {
    match kind {
        DiffLineKind::Add => theme.green,
        DiffLineKind::Remove => theme.red,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn render_code_line_background(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    w: f32,
    gutter_w: f32,
    diff: Option<DiffLineKind>,
    theme: &IdeTheme,
    s: f32,
    viewport_clip: [f32; 4],
) {
    let Some(kind) = diff else {
        return;
    };
    let color = diff_color(kind, theme);
    draw_rect_clipped(
        sugarloaf,
        [x + gutter_w, y - 2.0 * s, (w - gutter_w).max(0.0), 18.0 * s],
        theme.f32_alpha(color, 0.13),
        ORDER_PANEL + 3,
        viewport_clip,
    );
    draw_rect_clipped(
        sugarloaf,
        [x + gutter_w, y - 2.0 * s, 3.0 * s, 18.0 * s],
        theme.f32(color),
        ORDER_PANEL + 4,
        viewport_clip,
    );
}

#[allow(clippy::too_many_arguments)]
pub fn render_code_line_text(
    sugarloaf: &mut Sugarloaf,
    x: f32,
    y: f32,
    line: &str,
    lang: Lang,
    diff: Option<DiffLineKind>,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusion_rects: &[[f32; 4]],
) {
    if let Some(kind) = diff {
        let mut diff_opts = *opts;
        diff_opts.color = theme.u8(diff_color(kind, theme));
        draw_text_clipped(sugarloaf, x, y, line, &diff_opts, occlusion_rects);
    } else {
        draw_syntax_line(sugarloaf, x, y, line, lang, opts, theme, occlusion_rects);
    }
}

pub fn syntax_lang(lang: &str) -> crate::syntax::Lang {
    match lang {
        "rust" | "rs" => crate::syntax::Lang::Rust,
        "javascript" | "js" | "mjs" | "cjs" => crate::syntax::Lang::Javascript,
        "jsx" => crate::syntax::Lang::Jsx,
        "typescript" | "ts" => crate::syntax::Lang::Typescript,
        "tsx" => crate::syntax::Lang::Tsx,
        "python" | "py" => crate::syntax::Lang::Python,
        "go" => crate::syntax::Lang::Go,
        "lua" => crate::syntax::Lang::Lua,
        "toml" => crate::syntax::Lang::Toml,
        "json" | "jsonc" => crate::syntax::Lang::Json,
        "nix" => crate::syntax::Lang::Nix,
        "bash" | "sh" | "shell" | "zsh" => crate::syntax::Lang::Bash,
        "c" | "h" => crate::syntax::Lang::C,
        "cpp" | "c++" | "cc" | "cxx" | "hpp" => crate::syntax::Lang::Cpp,
        "yaml" | "yml" => crate::syntax::Lang::Yaml,
        "css" => crate::syntax::Lang::Css,
        "html" | "htm" => crate::syntax::Lang::Html,
        _ => crate::syntax::Lang::Other,
    }
}

pub fn draw_syntax_line(
    sugarloaf: &mut Sugarloaf,
    mut x: f32,
    y: f32,
    line: &str,
    lang: Lang,
    opts: &DrawOpts,
    theme: &IdeTheme,
    occlusion_rects: &[[f32; 4]],
) {
    let spans = highlighted_code_line(line, lang);
    for (tok, slice) in spans.iter() {
        let mut span_opts = *opts;
        span_opts.color = crate::syntax::syn_color(*tok, theme, false);
        draw_text_clipped(sugarloaf, x, y, slice, &span_opts, occlusion_rects);
        x += measure_text_cached(sugarloaf, slice, &span_opts);
    }
}

fn highlighted_code_line(line: &str, lang: Lang) -> Rc<Vec<(SynTok, String)>> {
    let key = CodeLineHighlightKey {
        line: line.to_string(),
        lang: lang_cache_tag(lang),
    };
    if let Some(hit) = CODE_LINE_HIGHLIGHT_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    super::derivations::bump_code_highlight();
    let spans = Rc::new(
        crate::syntax::highlight_line(line, lang)
            .into_iter()
            .map(|(tok, slice)| (tok, slice.to_string()))
            .collect::<Vec<_>>(),
    );
    CODE_LINE_HIGHLIGHT_CACHE.with(|cache| cache.borrow_mut().insert(key, spans.clone()));
    spans
}

fn cached_code_line_ranges(message_id: &str, text: &str) -> Rc<Vec<Range<usize>>> {
    let key = CodeLineRangeKey {
        message_id: message_id.to_string(),
        text_len: text.len(),
        text_fingerprint: sampled_text_fingerprint(text),
    };
    if let Some(hit) = CODE_LINE_RANGE_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    super::derivations::bump_code_line_range();
    let ranges = Rc::new(code_line_ranges(text));
    CODE_LINE_RANGE_CACHE.with(|cache| cache.borrow_mut().insert(key, ranges.clone()));
    ranges
}

fn code_line_ranges(text: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0usize;
    for segment in text.split_inclusive('\n') {
        let end = start + segment.len();
        let mut line_end = if segment.ends_with('\n') {
            end - 1
        } else {
            end
        };
        if line_end > start && text.as_bytes()[line_end - 1] == b'\r' {
            line_end -= 1;
        }
        ranges.push(start..line_end);
        start = end;
    }
    if ranges.is_empty() && !text.is_empty() {
        ranges.push(0..text.len());
    }
    ranges
}

fn sampled_text_fingerprint(text: &str) -> u64 {
    const SAMPLE_BYTES: usize = 64;

    let bytes = text.as_bytes();
    let mut hasher = DefaultHasher::new();
    bytes.len().hash(&mut hasher);
    if bytes.is_empty() {
        return hasher.finish();
    }

    let sample = SAMPLE_BYTES.min(bytes.len());
    bytes[..sample].hash(&mut hasher);
    if bytes.len() > sample * 2 {
        let mid_start = bytes.len() / 2 - sample / 2;
        bytes[mid_start..mid_start + sample].hash(&mut hasher);
    }
    if bytes.len() > sample {
        bytes[bytes.len() - sample..].hash(&mut hasher);
    }
    hasher.finish()
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

pub fn truncate_chars(value: &str, max: usize) -> String {
    let mut out = value.chars().take(max).collect::<String>();
    if value.chars().count() > max {
        out.push_str("...");
    }
    out
}
