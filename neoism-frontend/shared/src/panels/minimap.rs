use rustc_hash::{FxHashMap, FxHashSet};
use sugarloaf::Sugarloaf;

use crate::editor_snapshot::{MinimapData, MinimapGitChange};
use crate::primitives::{IdeTheme, IdeThemeName};

const BASE_WIDTH: f32 = 92.0;
const MIN_WIDTH: f32 = 58.0;
const MAX_WIDTH: f32 = 122.0;
const MIN_PANE_WIDTH: f32 = 300.0;
const OUTER_MARGIN: f32 = 6.0;
const INNER_PAD_X: f32 = 7.0;
const INNER_PAD_Y: f32 = 7.0;
const MIN_COMPACT_HEIGHT: f32 = 48.0;
const MIN_VIEWPORT_H: f32 = 14.0;
const MAX_DRAWN_LINES_PER_PIXEL: f32 = 0.75;
const MAX_SEGMENTS: usize = 1800;
const MAX_GIT_MARKERS: usize = 1200;

const DEPTH_BG: f32 = 0.08;
const DEPTH_LINE: f32 = 0.10;
const DEPTH_VIEWPORT: f32 = 0.12;
const ORDER: u8 = 14;

#[derive(Clone, Debug, Default)]
struct MinimapSnapshot {
    path: Option<std::path::PathBuf>,
    changedtick: u64,
    line_revision: u64,
    total_lines: u64,
    top_line: u64,
    bottom_line: u64,
    cursor_line: u64,
    sample_stride: u64,
    line_shapes: Vec<MinimapLineShape>,
    git_changes: Vec<MinimapGitChangeMarker>,
    line_cache: Option<MinimapLineCache>,
}

#[derive(Clone, Copy, Debug)]
struct MinimapGitChangeMarker {
    line: u64,
    kind: MinimapGitChangeKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapGitChangeKind {
    Add,
    Change,
    Delete,
}

#[derive(Clone, Copy, Debug)]
struct MinimapLineShape {
    source_line: u64,
    indent_chars: u16,
    char_count: u16,
    class: MinimapLineClass,
}

#[derive(Clone, Debug)]
struct MinimapLineCache {
    key: MinimapLineCacheKey,
    rects: Vec<MinimapLineRect>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MinimapLineCacheKey {
    line_revision: u64,
    total_lines: u64,
    sample_stride: u64,
    width_bits: u32,
    height_bits: u32,
    theme: IdeThemeName,
}

#[derive(Clone, Copy, Debug)]
struct MinimapLineRect {
    x_offset: f32,
    y_offset: f32,
    width: f32,
    height: f32,
    color: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MinimapLineClass {
    Comment,
    Function,
    Type,
    Keyword,
    String,
    Number,
    Text,
}

#[derive(Clone, Copy, Debug)]
struct MinimapRect {
    route_id: usize,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    content_y: f32,
    content_h: f32,
    total_lines: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MinimapHit {
    pub route_id: usize,
    pub line: u64,
}

pub struct Minimap {
    enabled: bool,
    scale: f32,
    snapshots: FxHashMap<usize, MinimapSnapshot>,
    subscribed_routes: FxHashSet<usize>,
    rects: Vec<MinimapRect>,
    hovered_route: Option<usize>,
    drag_route: Option<usize>,
    last_drag_line: Option<u64>,
}

impl Minimap {
    pub fn new() -> Self {
        Self {
            enabled: false,
            scale: 1.0,
            snapshots: FxHashMap::default(),
            subscribed_routes: FxHashSet::default(),
            rects: Vec::new(),
            hovered_route: None,
            drag_route: None,
            last_drag_line: None,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn set_enabled(&mut self, enabled: bool) {
        if self.enabled == enabled {
            return;
        }

        self.enabled = enabled;
        self.hovered_route = None;
        self.drag_route = None;
        self.last_drag_line = None;
        self.rects.clear();
        self.subscribed_routes.clear();
        if !enabled {
            self.snapshots.clear();
        }
    }

    pub fn set_scale(&mut self, scale: f32) {
        self.scale = scale.clamp(0.5, 3.0);
    }

    pub fn sync_visible_routes(&mut self, routes: &[usize]) -> (Vec<usize>, Vec<usize>) {
        if !self.enabled {
            return (Vec::new(), Vec::new());
        }

        let visible: FxHashSet<usize> = routes.iter().copied().collect();
        let enable_routes = visible
            .difference(&self.subscribed_routes)
            .copied()
            .collect::<Vec<_>>();
        let disable_routes = self
            .subscribed_routes
            .difference(&visible)
            .copied()
            .collect::<Vec<_>>();

        self.subscribed_routes = visible;
        let subscribed_routes = self.subscribed_routes.clone();
        self.snapshots
            .retain(|route_id, _| subscribed_routes.contains(route_id));

        (enable_routes, disable_routes)
    }

    pub fn is_subscribed(&self, route_id: usize) -> bool {
        self.enabled && self.subscribed_routes.contains(&route_id)
    }

    /// Drop the cached snapshot for `route_id` so the next render
    /// paints nothing for that route until a fresh `apply_update` lands.
    /// Returns `true` when a snapshot was actually removed.
    pub fn clear_route(&mut self, route_id: usize) -> bool {
        self.subscribed_routes.remove(&route_id);
        self.snapshots.remove(&route_id).is_some()
    }

    /// Apply a per-route minimap data push. The host packs its editor
    /// state into `MinimapData` at the boundary so this panel stays
    /// backend-free.
    pub fn apply_update(&mut self, route_id: usize, update: MinimapData) -> bool {
        if !self.enabled {
            return false;
        }

        if update.total_lines == 0 {
            return self.snapshots.remove(&route_id).is_some();
        }

        let snapshot = self.snapshots.entry(route_id).or_default();
        let total_lines = update.total_lines.max(1);
        let top_line = update.top_line.clamp(1, total_lines);
        let bottom_line = update.bottom_line.clamp(top_line, total_lines);
        let cursor_line = update.cursor_line.clamp(1, total_lines);
        let sample_stride = update.sample_stride.max(1);

        let mut changed = snapshot.path != update.path
            || snapshot.changedtick != update.changedtick
            || snapshot.total_lines != total_lines
            || snapshot.top_line != top_line
            || snapshot.bottom_line != bottom_line
            || snapshot.cursor_line != cursor_line
            || snapshot.sample_stride != sample_stride;

        snapshot.path = update.path;
        snapshot.changedtick = update.changedtick;
        snapshot.total_lines = total_lines;
        snapshot.top_line = top_line;
        snapshot.bottom_line = bottom_line;
        snapshot.cursor_line = cursor_line;
        snapshot.sample_stride = sample_stride;

        let git_changes = update
            .git_changes
            .iter()
            .filter_map(|change| {
                MinimapGitChangeMarker::from_notification(change, total_lines)
            })
            .take(MAX_GIT_MARKERS)
            .collect::<Vec<_>>();
        if snapshot.git_changes.len() != git_changes.len()
            || snapshot
                .git_changes
                .iter()
                .zip(git_changes.iter())
                .any(|(a, b)| a.line != b.line || a.kind != b.kind)
        {
            snapshot.git_changes = git_changes;
            changed = true;
        }

        if let Some(lines) = update.lines {
            snapshot.line_shapes = build_line_shapes(&lines, sample_stride, total_lines);
            snapshot.line_revision = snapshot.line_revision.wrapping_add(1);
            snapshot.line_cache = None;
            changed = true;
        }

        changed
    }

    pub fn begin_frame(&mut self) {
        self.rects.clear();
        if !self.enabled {
            self.hovered_route = None;
            self.drag_route = None;
            self.last_drag_line = None;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render_pane(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        route_id: usize,
        pane_x: f32,
        pane_y: f32,
        pane_w: f32,
        pane_h: f32,
        viewport_rows: u32,
        scroll_offset_lines: f32,
        theme: IdeTheme,
    ) {
        if !self.enabled || pane_w < MIN_PANE_WIDTH || pane_h <= INNER_PAD_Y * 2.0 {
            return;
        }
        let Some(snapshot) = self.snapshots.get_mut(&route_id) else {
            return;
        };
        if snapshot.total_lines <= 1 || snapshot.line_shapes.is_empty() {
            return;
        }

        let width = (BASE_WIDTH * self.scale).clamp(MIN_WIDTH, MAX_WIDTH);
        let height =
            minimap_height(snapshot.total_lines, viewport_rows, pane_h, self.scale);
        if height <= INNER_PAD_Y * 2.0 {
            return;
        }
        let x = pane_x + pane_w - width - OUTER_MARGIN;
        let y = pane_y + OUTER_MARGIN;
        let content_y = y + INNER_PAD_Y;
        let content_h = height - INNER_PAD_Y * 2.0;
        let content_x = x + INNER_PAD_X;
        let content_w = (width - INNER_PAD_X * 2.0).max(1.0);

        self.rects.push(MinimapRect {
            route_id,
            x,
            y,
            width,
            height,
            content_y,
            content_h,
            total_lines: snapshot.total_lines,
        });

        let hovered =
            self.hovered_route == Some(route_id) || self.drag_route == Some(route_id);
        let bg_alpha = if hovered { 0.72 } else { 0.58 };
        sugarloaf.rounded_rect(
            None,
            x,
            y,
            width,
            height,
            theme.f32_alpha(theme.surface, bg_alpha),
            DEPTH_BG,
            7.0,
            ORDER,
        );
        sugarloaf.rounded_rect(
            None,
            x,
            y,
            width,
            height,
            theme.f32_alpha(theme.border, if hovered { 0.36 } else { 0.22 }),
            DEPTH_BG + 0.001,
            7.0,
            ORDER,
        );
        sugarloaf.rounded_rect(
            None,
            x + 1.0,
            y + 1.0,
            (width - 2.0).max(0.0),
            (height - 2.0).max(0.0),
            theme.f32_alpha(theme.surface, bg_alpha),
            DEPTH_BG + 0.002,
            6.0,
            ORDER,
        );

        render_lines(
            sugarloaf, snapshot, content_x, content_y, content_w, content_h, theme,
        );
        render_git_changes(
            sugarloaf, snapshot, content_x, content_y, content_w, content_h, theme,
        );
        render_viewport(
            sugarloaf,
            snapshot,
            x,
            width,
            content_y,
            content_h,
            scroll_offset_lines,
            theme,
        );
    }

    pub fn hover(&mut self, x: f32, y: f32) -> bool {
        let next = self.hit_rect(x, y).map(|rect| rect.route_id);
        if self.hovered_route == next {
            return false;
        }
        self.hovered_route = next;
        true
    }

    pub fn is_hovered(&self) -> bool {
        self.hovered_route.is_some()
    }

    pub fn begin_drag(&mut self, x: f32, y: f32) -> Option<MinimapHit> {
        let hit = self.hit_test(x, y)?;
        self.drag_route = Some(hit.route_id);
        self.last_drag_line = Some(hit.line);
        Some(hit)
    }

    pub fn drag_to(&mut self, x: f32, y: f32) -> Option<MinimapHit> {
        let route_id = self.drag_route?;
        let rect = self.rects.iter().find(|rect| rect.route_id == route_id)?;
        let line = rect.line_at(y);
        if self.last_drag_line == Some(line) {
            return None;
        }
        self.last_drag_line = Some(line);
        let _ = x;
        Some(MinimapHit { route_id, line })
    }

    pub fn end_drag(&mut self) -> bool {
        let was_dragging = self.drag_route.take().is_some();
        self.last_drag_line = None;
        was_dragging
    }

    pub fn is_dragging(&self) -> bool {
        self.drag_route.is_some()
    }

    pub fn hit_test(&self, x: f32, y: f32) -> Option<MinimapHit> {
        let rect = self.hit_rect(x, y)?;
        Some(MinimapHit {
            route_id: rect.route_id,
            line: rect.line_at(y),
        })
    }

    fn hit_rect(&self, x: f32, y: f32) -> Option<&MinimapRect> {
        if !self.enabled {
            return None;
        }
        self.rects.iter().rev().find(|rect| {
            x >= rect.x
                && x <= rect.x + rect.width
                && y >= rect.y
                && y <= rect.y + rect.height
        })
    }
}

#[allow(clippy::too_many_arguments)]
fn render_lines(
    sugarloaf: &mut Sugarloaf,
    snapshot: &mut MinimapSnapshot,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    theme: IdeTheme,
) {
    let rects = cached_line_rects(snapshot, width, height, theme);
    for rect in rects {
        sugarloaf.rect(
            None,
            x + rect.x_offset,
            y + rect.y_offset,
            rect.width,
            rect.height,
            rect.color,
            DEPTH_LINE,
            ORDER,
        );
    }
}

fn cached_line_rects(
    snapshot: &mut MinimapSnapshot,
    width: f32,
    height: f32,
    theme: IdeTheme,
) -> &[MinimapLineRect] {
    let key = MinimapLineCacheKey {
        line_revision: snapshot.line_revision,
        total_lines: snapshot.total_lines,
        sample_stride: snapshot.sample_stride,
        width_bits: width.to_bits(),
        height_bits: height.to_bits(),
        theme: theme.name,
    };

    if snapshot
        .line_cache
        .as_ref()
        .is_none_or(|cache| cache.key != key)
    {
        let rects = build_line_rects(snapshot, width, height, theme);
        snapshot.line_cache = Some(MinimapLineCache { key, rects });
    }

    snapshot
        .line_cache
        .as_ref()
        .map(|cache| cache.rects.as_slice())
        .unwrap_or(&[])
}

fn build_line_rects(
    snapshot: &MinimapSnapshot,
    width: f32,
    height: f32,
    theme: IdeTheme,
) -> Vec<MinimapLineRect> {
    let sample_count = snapshot.line_shapes.len();
    if sample_count == 0 || height <= 0.0 || width <= 0.0 {
        return Vec::new();
    }

    let max_rows = (height * MAX_DRAWN_LINES_PER_PIXEL).ceil().max(1.0) as usize;
    let step = sample_count.div_ceil(max_rows).max(1);
    let mut rects = Vec::with_capacity((sample_count / step).min(MAX_SEGMENTS));

    for shape in snapshot.line_shapes.iter().step_by(step) {
        if rects.len() >= MAX_SEGMENTS {
            break;
        }
        let denom = snapshot.total_lines.saturating_sub(1).max(1) as f32;
        let progress = shape.source_line.saturating_sub(1) as f32 / denom;
        let line_y = progress * height;
        let line_h = ((height / snapshot.total_lines.max(1) as f32)
            * snapshot.sample_stride.max(1) as f32)
            .clamp(1.0, 2.0);
        rects.push(minimap_line_rect(shape, line_y, width, line_h, theme));
    }

    rects
}

fn render_git_changes(
    sugarloaf: &mut Sugarloaf,
    snapshot: &MinimapSnapshot,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    theme: IdeTheme,
) {
    if snapshot.git_changes.is_empty() || snapshot.total_lines <= 1 || height <= 0.0 {
        return;
    }

    let rail_w = (width * 0.10).clamp(3.0, 6.0);
    let marker_x = x + width - rail_w;
    let marker_h = ((height / snapshot.total_lines.max(1) as f32) * 2.0).clamp(2.0, 5.0);
    let denom = snapshot.total_lines.saturating_sub(1).max(1) as f32;
    for marker in &snapshot.git_changes {
        let progress = marker.line.saturating_sub(1) as f32 / denom;
        let marker_y = y + progress * height;
        sugarloaf.rect(
            None,
            marker_x,
            marker_y,
            rail_w,
            marker_h,
            marker.kind.color(theme),
            DEPTH_VIEWPORT + 0.004,
            ORDER,
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn render_viewport(
    sugarloaf: &mut Sugarloaf,
    snapshot: &MinimapSnapshot,
    x: f32,
    width: f32,
    content_y: f32,
    content_h: f32,
    scroll_offset_lines: f32,
    theme: IdeTheme,
) {
    let denom = snapshot.total_lines.saturating_sub(1).max(1) as f32;
    let visual_top =
        visual_line(snapshot.top_line, scroll_offset_lines, snapshot.total_lines);
    let visual_bottom = visual_line(
        snapshot.bottom_line,
        scroll_offset_lines,
        snapshot.total_lines,
    );
    let top_progress = (visual_top - 1.0) / denom;
    let bottom_progress = (visual_bottom - 1.0) / denom;
    let viewport_y = content_y + top_progress * content_h;
    let viewport_h = ((bottom_progress - top_progress).max(0.0) * content_h)
        .max(MIN_VIEWPORT_H)
        .min(content_h);

    sugarloaf.rounded_rect(
        None,
        x + 2.0,
        viewport_y,
        (width - 4.0).max(0.0),
        viewport_h,
        theme.f32_alpha(theme.hover, 0.42),
        DEPTH_VIEWPORT,
        4.0,
        ORDER,
    );
    sugarloaf.rounded_rect(
        None,
        x + 2.0,
        viewport_y,
        (width - 4.0).max(0.0),
        viewport_h,
        theme.f32_alpha(theme.accent, 0.22),
        DEPTH_VIEWPORT + 0.001,
        4.0,
        ORDER,
    );

    let visual_cursor = visual_line(
        snapshot.cursor_line,
        scroll_offset_lines,
        snapshot.total_lines,
    );
    let cursor_progress = (visual_cursor - 1.0) / denom;
    let cursor_y = content_y + cursor_progress * content_h;
    sugarloaf.rect(
        None,
        x + 4.0,
        cursor_y,
        (width - 8.0).max(0.0),
        1.5,
        theme.f32_alpha(theme.accent, 0.86),
        DEPTH_VIEWPORT + 0.002,
        ORDER,
    );
}

fn minimap_height(total_lines: u64, viewport_rows: u32, pane_h: f32, scale: f32) -> f32 {
    let full_height = (pane_h - OUTER_MARGIN * 2.0).max(0.0);
    if full_height <= 0.0 {
        return 0.0;
    }

    let viewport_rows = viewport_rows.max(1) as f32;
    if total_lines as f32 >= viewport_rows {
        return full_height;
    }

    let min_height = (MIN_COMPACT_HEIGHT * scale).clamp(MIN_COMPACT_HEIGHT, full_height);
    (full_height * (total_lines as f32 / viewport_rows)).clamp(min_height, full_height)
}

fn visual_line(line: u64, scroll_offset_lines: f32, total_lines: u64) -> f32 {
    (line as f32 - scroll_offset_lines).clamp(1.0, total_lines.max(1) as f32)
}

impl MinimapRect {
    fn line_at(&self, y: f32) -> u64 {
        if self.total_lines <= 1 || self.content_h <= 0.0 {
            return 1;
        }
        let progress = ((y - self.content_y) / self.content_h).clamp(0.0, 1.0);
        (1.0 + progress * (self.total_lines - 1) as f32).round() as u64
    }
}

fn minimap_line_rect(
    shape: &MinimapLineShape,
    y_offset: f32,
    width: f32,
    line_h: f32,
    theme: IdeTheme,
) -> MinimapLineRect {
    let indent_px = (shape.indent_chars as f32 * 1.0).min(width * 0.30);
    let available_w = (width - indent_px).max(2.0);
    let total_w = (shape.char_count.max(1) as f32 * 1.45).clamp(3.0, available_w);
    MinimapLineRect {
        x_offset: indent_px,
        y_offset,
        width: total_w,
        height: line_h,
        color: shape.class.color(theme),
    }
}

fn build_line_shapes(
    lines: &[String],
    sample_stride: u64,
    total_lines: u64,
) -> Vec<MinimapLineShape> {
    lines
        .iter()
        .enumerate()
        .filter_map(|(sample_index, line)| {
            let trimmed = line.trim_start();
            if trimmed.is_empty() {
                return None;
            }
            let source_line = (sample_index as u64)
                .saturating_mul(sample_stride.max(1))
                .saturating_add(1)
                .min(total_lines.max(1));
            let indent_chars =
                line.chars().count().saturating_sub(trimmed.chars().count());
            Some(MinimapLineShape {
                source_line,
                indent_chars: indent_chars.min(u16::MAX as usize) as u16,
                char_count: trimmed.chars().take(180).count().max(1) as u16,
                class: classify_line(trimmed),
            })
        })
        .collect()
}

fn classify_line(line: &str) -> MinimapLineClass {
    let lower = line.to_ascii_lowercase();
    if lower.starts_with("//")
        || lower.starts_with('#')
        || lower.starts_with("--")
        || lower.starts_with('*')
    {
        MinimapLineClass::Comment
    } else if lower.starts_with("fn ")
        || lower.starts_with("def ")
        || lower.starts_with("function ")
        || lower.contains(" function ")
        || lower.contains(" => ")
    {
        MinimapLineClass::Function
    } else if lower.starts_with("struct ")
        || lower.starts_with("class ")
        || lower.starts_with("interface ")
        || lower.starts_with("enum ")
        || lower.starts_with("type ")
    {
        MinimapLineClass::Type
    } else if lower.starts_with("use ")
        || lower.starts_with("import ")
        || lower.starts_with("from ")
        || lower.starts_with("pub ")
        || lower.starts_with("let ")
        || lower.starts_with("const ")
        || lower.starts_with("return ")
        || lower.starts_with("if ")
        || lower.starts_with("for ")
        || lower.starts_with("while ")
        || lower.starts_with("match ")
    {
        MinimapLineClass::Keyword
    } else if line.contains('"') || line.contains('\'') || line.contains('`') {
        MinimapLineClass::String
    } else if line.chars().any(|ch| ch.is_ascii_digit()) {
        MinimapLineClass::Number
    } else {
        MinimapLineClass::Text
    }
}

impl MinimapLineClass {
    fn color(self, theme: IdeTheme) -> [f32; 4] {
        let color = match self {
            MinimapLineClass::Comment => theme.syn_comment,
            MinimapLineClass::Function => theme.syn_func,
            MinimapLineClass::Type => theme.syn_type,
            MinimapLineClass::Keyword => theme.syn_keyword,
            MinimapLineClass::String => theme.syn_string,
            MinimapLineClass::Number => theme.syn_number,
            MinimapLineClass::Text => theme.fg,
        };
        theme.f32_alpha(color, 0.58)
    }
}

impl MinimapGitChangeMarker {
    /// Build a marker from the shared POD git change record. Native
    /// `MinimapGitChange` flows through `MinimapData` so the panel sees
    /// only this neutral type.
    fn from_notification(change: &MinimapGitChange, total_lines: u64) -> Option<Self> {
        let kind = match change.kind.as_str() {
            "add" => MinimapGitChangeKind::Add,
            "change" => MinimapGitChangeKind::Change,
            "delete" => MinimapGitChangeKind::Delete,
            _ => return None,
        };
        Some(Self {
            line: change.line.clamp(1, total_lines.max(1)),
            kind,
        })
    }
}

impl MinimapGitChangeKind {
    fn color(self, theme: IdeTheme) -> [f32; 4] {
        let color = match self {
            MinimapGitChangeKind::Add => theme.green,
            MinimapGitChangeKind::Change => theme.yellow,
            MinimapGitChangeKind::Delete => theme.red,
        };
        theme.f32_alpha(color, 0.92)
    }
}

impl Default for Minimap {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_files_use_compact_minimap_height() {
        let full = minimap_height(200, 40, 800.0, 1.0);
        let compact = minimap_height(10, 40, 800.0, 1.0);

        assert!(compact < full);
        assert!(compact >= MIN_COMPACT_HEIGHT);
    }

    #[test]
    fn visual_line_tracks_scroll_offset() {
        assert_eq!(visual_line(20, 1.5, 100), 18.5);
        assert_eq!(visual_line(1, 4.0, 100), 1.0);
        assert_eq!(visual_line(100, -4.0, 100), 100.0);
    }

    #[test]
    fn line_shapes_skip_blank_lines_and_keep_source_lines() {
        let lines = vec![
            "fn main() {}".to_string(),
            "   ".to_string(),
            "let x = 1;".to_string(),
        ];
        let shapes = build_line_shapes(&lines, 3, 100);

        assert_eq!(shapes.len(), 2);
        assert_eq!(shapes[0].source_line, 1);
        assert_eq!(shapes[0].class, MinimapLineClass::Function);
        assert_eq!(shapes[1].source_line, 7);
        assert_eq!(shapes[1].class, MinimapLineClass::Keyword);
    }
}
