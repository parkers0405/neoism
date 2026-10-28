//! Host-neutral conversation search modal / project selector. No I/O or input ownership.
mod render;
use crate::primitives::IdeTheme;
use crate::widgets::scroll::Scroll;
use render::{frame_inset, header_height, NavigatorRenderGeometry};
use web_time::Instant;

const PROJECT_ROW_HEIGHT: f32 = 32.0;
const SEARCH_ROW_HEIGHT: f32 = 48.0;
const SEARCH_VISIBLE_ROWS: usize = 8;
const LIST_SCROLL_ANIMATION_SECONDS: f32 = 0.12;

struct NavigatorScroll {
    motion: Scroll,
    last_frame: Option<Instant>,
}

impl Default for NavigatorScroll {
    fn default() -> Self {
        Self {
            motion: Scroll::new().with_animation_length(LIST_SCROLL_ANIMATION_SECONDS),
            last_frame: None,
        }
    }
}

impl NavigatorScroll {
    fn set_target(&mut self, target: f32) {
        if target == self.motion.target() {
            return;
        }
        if !self.motion.is_animating() {
            self.last_frame = Some(Instant::now());
        }
        self.motion.set_target(target);
    }
    fn tick(&mut self, dt: f32) {
        if self.motion.is_animating() {
            let dt = if dt.is_finite() {
                dt.clamp(0.0, 0.05)
            } else {
                0.0
            };
            self.motion.tick(dt);
            // Stop only once the remaining move is visually negligible. A
            // fixed deadline can snap several pixels on a larger wheel step.
            if (self.motion.current() - self.motion.target()).abs() <= 0.25 {
                self.motion.set_target_immediate(self.motion.target());
            }
        }
        if !self.motion.is_animating() {
            self.last_frame = None;
        }
    }
    fn frame_dt(&self, now: Instant) -> f32 {
        self.last_frame.map_or(0.0, |last| {
            now.saturating_duration_since(last).as_secs_f32()
        })
    }
    fn finish_frame(&mut self, now: Instant) {
        self.last_frame = self.motion.is_animating().then_some(now);
    }
    fn snap(&mut self, position: f32) {
        self.motion.set_target_immediate(position);
        self.last_frame = None;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConversationNavigatorMode {
    Search,
    Projects,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ConversationNavigatorRow {
    pub value: String,
    pub title: String,
    pub description: String,
    pub footer: String,
    pub is_current: bool,
    pub is_disabled: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversationNavigatorAction {
    Activate(String),
    Dismiss,
}

#[derive(Default)]
pub struct ConversationNavigator {
    mode: Option<ConversationNavigatorMode>,
    query: String,
    rows: Vec<ConversationNavigatorRow>,
    filtered: Vec<usize>,
    selected: Option<usize>,
    selection_touched: bool,
    scroll: NavigatorScroll,
    cursor: NavigatorScroll,
    loading: bool,
    indexing: bool,
    error: Option<String>,
    anchor: Option<[f32; 4]>,
    bounds: Option<[f32; 4]>,
    row_hits: Vec<(usize, [f32; 4])>,
    visible: usize,
}

impl ConversationNavigator {
    fn row_height(&self) -> f32 {
        if self.projects() {
            PROJECT_ROW_HEIGHT
        } else {
            SEARCH_ROW_HEIGHT
        }
    }
    fn show_skeletons(&self) -> bool {
        self.loading && self.filtered.is_empty() && self.error.is_none()
    }
    pub fn open(
        &mut self,
        mode: ConversationNavigatorMode,
        rows: Vec<ConversationNavigatorRow>,
    ) {
        *self = Self {
            mode: Some(mode),
            rows,
            ..Self::default()
        };
        self.refilter();
        self.reveal();
        self.scroll.snap(self.scroll.motion.target());
        self.snap_cursor();
    }
    pub fn close(&mut self) {
        *self = Self::default();
    }
    /// Scroll or selection-cursor motion still settling (hosts keep painting).
    pub fn is_animating(&self) -> bool {
        self.is_active() && (self.scroll.motion.is_animating() || self.cursor.motion.is_animating() || self.show_skeletons())
    }
    pub fn is_active(&self) -> bool {
        self.mode.is_some()
    }
    pub fn mode(&self) -> Option<ConversationNavigatorMode> {
        self.mode
    }
    pub fn query(&self) -> &str {
        &self.query
    }
    pub fn set_query(&mut self, query: String) {
        if self.query != query {
            self.query = query;
            self.indexing = false;
            self.selection_touched = false;
            self.refilter();
            self.scroll = NavigatorScroll::default();
            self.reveal();
            self.scroll.snap(self.scroll.motion.target());
            self.snap_cursor();
        }
    }
    pub fn push_text(&mut self, text: &str) {
        self.set_query(format!("{}{text}", self.query));
    }
    pub fn pop_char(&mut self) {
        let mut q = self.query.clone();
        q.pop();
        self.set_query(q);
    }
    pub fn set_rows(&mut self, mut rows: Vec<ConversationNavigatorRow>) {
        if !self.projects() {
            for row in &mut rows {
                row.description = matching_excerpt(&row.description, &self.query);
            }
        }
        let cursor_residual = self.cursor.motion.current()
            - self.selected.unwrap_or(0) as f32 * self.row_height();
        let previous = (self.mode == Some(ConversationNavigatorMode::Search)
            || self.selection_touched)
            .then(|| self.selected_value().map(str::to_owned))
            .flatten();
        // Anchor current AND destination independently: a refresh during a wheel
        // or keyboard animation must not discard either fractional displacement.
        let anchor = |px: f32| {
            let first = (px / self.row_height()).floor() as usize;
            self.filtered.get(first).map(|i| {
                (
                    self.rows[*i].value.clone(),
                    px - first as f32 * self.row_height(),
                )
            })
        };
        let top = anchor(self.scroll.motion.current());
        let destination = anchor(self.scroll.motion.target());
        self.rows = rows;
        self.refilter();
        if let Some(value) = previous {
            if let Some(i) = self
                .filtered
                .iter()
                .position(|i| !self.rows[*i].is_disabled && self.rows[*i].value == value)
            {
                self.selected = Some(i);
            }
        }
        let restore = |anchor: Option<(String, f32)>| {
            anchor.and_then(|(value, fraction)| {
                self.filtered
                    .iter()
                    .position(|i| self.rows[*i].value == value)
                    .map(|i| i as f32 * self.row_height() + fraction)
            })
        };
        let current = restore(top);
        let target = restore(destination);
        if let Some(current) = current {
            // Translate without clearing the spring's in-flight velocity.
            self.scroll
                .motion
                .scroll_by(current - self.scroll.motion.current());
        }
        if let Some(target) = target {
            self.scroll.motion.set_target(target);
        }
        let cursor_target = self.selected.unwrap_or(0) as f32 * self.row_height();
        self.cursor
            .motion
            .scroll_by(cursor_target + cursor_residual - self.cursor.motion.current());
        self.cursor.motion.set_target(cursor_target);
        self.clamp_scroll();
        self.invalidate();
    }
    pub fn set_indexing(&mut self, indexing: bool) {
        self.indexing = indexing;
    }
    fn indexing_status(&self) -> Option<&'static str> {
        (self.mode == Some(ConversationNavigatorMode::Search)
            && self.indexing
            && self.error.is_none())
        .then_some("Indexing conversation history...")
    }
    pub fn set_loading(&mut self, loading: bool) {
        self.loading = loading;
    }
    pub fn set_error(&mut self, error: Option<String>) {
        self.error = error;
    }
    pub fn set_anchor(&mut self, anchor: Option<[f32; 4]>) {
        self.anchor = anchor;
        self.invalidate();
    }
    pub fn selected_value(&self) -> Option<&str> {
        if self.error.is_some() {
            return None;
        }
        let row = self.rows.get(*self.filtered.get(self.selected?)?)?;
        (!row.is_disabled).then_some(row.value.as_str())
    }
    pub fn select_next(&mut self) {
        self.step(true);
    }
    pub fn select_previous(&mut self) {
        self.step(false);
    }
    fn step(&mut self, forward: bool) {
        let n = self.filtered.len();
        if n == 0 {
            return;
        }
        self.selection_touched = true;
        let start = self.selected.unwrap_or(if forward { n - 1 } else { 0 });
        for delta in 1..=n {
            let i = if forward {
                (start + delta) % n
            } else {
                (start + n - delta) % n
            };
            if !self.rows[self.filtered[i]].is_disabled {
                self.selected = Some(i);
                break;
            }
        }
        self.reveal();
    }
    fn invalidate(&mut self) {
        self.bounds = None;
        self.row_hits.clear();
    }
    fn refilter(&mut self) {
        let query = self.query.trim().to_lowercase();
        self.filtered = self
            .rows
            .iter()
            .enumerate()
            .filter_map(|(i, row)| {
                (self.mode != Some(ConversationNavigatorMode::Projects)
                    || query.is_empty()
                    || [&row.title, &row.description, &row.footer]
                        .iter()
                        .any(|s| s.to_lowercase().contains(&query)))
                .then_some(i)
            })
            .collect();
        self.selected = (self.mode == Some(ConversationNavigatorMode::Projects))
            .then(|| {
                self.filtered
                    .iter()
                    .position(|i| self.rows[*i].is_current && !self.rows[*i].is_disabled)
            })
            .flatten()
            .or_else(|| {
                self.filtered
                    .iter()
                    .position(|i| !self.rows[*i].is_disabled)
            });
        self.invalidate();
    }
    fn snap_cursor(&mut self) {
        self.cursor
            .snap(self.selected.unwrap_or(0) as f32 * self.row_height());
    }
    fn animate_cursor(&mut self) {
        self.cursor
            .set_target(self.selected.unwrap_or(0) as f32 * self.row_height());
    }
    fn reveal(&mut self) {
        self.animate_cursor();
        let visible = self.visible.max(1) as f32 * self.row_height();
        self.clamp_scroll();
        if let Some(i) = self.selected {
            let top = i as f32 * self.row_height();
            let target = self.scroll.motion.target();
            if top < target {
                self.set_scroll_target(top);
            } else if top + self.row_height() > target + visible {
                self.set_scroll_target(top + self.row_height() - visible);
            }
        }
        self.invalidate();
    }
    fn set_visible_rows(&mut self, visible: usize) {
        if self.visible != visible {
            self.visible = visible;
            self.reveal();
        }
    }
    fn max_scroll(&self) -> f32 {
        self.filtered.len().saturating_sub(self.visible.max(1)) as f32 * self.row_height()
    }
    fn clamp_scroll(&mut self) {
        self.scroll.motion.clamp(0.0, self.max_scroll());
    }
    fn set_scroll_target(&mut self, target: f32) {
        self.scroll.set_target(target.clamp(0.0, self.max_scroll()));
    }
    fn tick_scroll(&mut self, dt: f32) -> (usize, f32) {
        self.clamp_scroll();
        self.scroll.tick(dt);
        let current = self.scroll.motion.current();
        let first = (current / self.row_height()).floor() as usize;
        (first, first as f32 * self.row_height() - current)
    }
    fn projects(&self) -> bool {
        self.mode == Some(ConversationNavigatorMode::Projects)
    }
    fn row_capacity(&self, viewport: [f32; 4], scale: f32) -> usize {
        let s = scale.clamp(0.5, 3.0);
        (((viewport[3] - 16.0 * s - header_height(s) - frame_inset(s))
            / (self.row_height() * s))
            .floor() as usize)
            .clamp(
                1,
                if self.projects() {
                    6
                } else {
                    SEARCH_VISIBLE_ROWS
                },
            )
    }
    fn layout_rows(&self) -> usize {
        if !self.projects() {
            SEARCH_VISIBLE_ROWS
        } else if self.error.is_some() {
            1
        } else if self.show_skeletons() {
            3
        } else {
            self.filtered.len().max(1)
        }
    }
    /// Exact occlusion/blocking region, in the viewport's coordinate space.
    pub fn bounds_for_viewport(&self, viewport: [f32; 4], scale: f32) -> [f32; 4] {
        let [vx, vy, vw, vh] = viewport;
        let vw = vw.max(0.0);
        let vh = vh.max(0.0);
        let s = scale.clamp(0.5, 3.0);
        let margin = (8.0 * s).min(vw / 2.0).min(vh / 2.0);
        let aw = (vw - 2.0 * margin).max(0.0);
        let ah = (vh - 2.0 * margin).max(0.0);
        let projects = self.mode == Some(ConversationNavigatorMode::Projects);
        let w = (if projects {
            self.anchor
                .map_or(360.0 * s, |r| r[2].clamp(320.0 * s, 420.0 * s))
        } else {
            480.0 * s
        })
        .min(aw);
        let h = (header_height(s)
            + frame_inset(s)
            + self.row_height()
                * s
                * self.layout_rows().min(self.row_capacity(viewport, s)) as f32)
            .min(ah);
        let (x, y) = if projects {
            self.anchor
                .map_or((vx + (vw - w) / 2.0, vy + (vh - h) / 2.0), |r| {
                    (r[0], r[1] + r[3] + 6.0 * s)
                })
        } else {
            (vx + (vw - w) / 2.0, vy + (vh - h) / 2.0)
        };
        render::snap_bounds(
            [
                x.clamp(vx + margin, vx + vw - margin - w),
                y.clamp(vy + margin, vy + vh - margin - h),
                w,
                h,
            ],
            s,
            viewport,
        )
    }
    pub fn render(
        &mut self,
        sugarloaf: &mut (impl crate::primitives::paint::Paint + ?Sized),
        theme: &IdeTheme,
        viewport: [f32; 4],
        scale: f32,
        now: f32,
    ) {
        self.invalidate();
        if !self.is_active() {
            return;
        }
        let s = scale.clamp(0.5, 3.0);
        self.set_visible_rows(self.row_capacity(viewport, s));
        let frame = Instant::now();
        let (first_row, residual) = self.tick_scroll(self.scroll.frame_dt(frame));
        self.scroll.finish_frame(frame);
        self.cursor.tick(self.cursor.frame_dt(frame));
        self.cursor.finish_frame(frame);
        let cursor_offset = self.cursor.motion.current()
            - self.selected.unwrap_or(0) as f32 * self.row_height();
        let rect = self.bounds_for_viewport(viewport, s);
        self.bounds = Some(rect);
        let state = NavigatorRenderGeometry {
            rect,
            first_row,
            row_height: self.row_height(),
            visible_rows: self.visible.min(self.layout_rows()),
        };
        self.set_render_geometry(state, s, residual);
        self.paint(sugarloaf, theme, state, s, residual, cursor_offset, now);
    }
    fn set_render_geometry(
        &mut self,
        state: NavigatorRenderGeometry,
        scale: f32,
        residual: f32,
    ) {
        self.bounds = Some(state.rect);
        self.row_hits.clear();
        if self.error.is_some() {
            return;
        }
        for index in state.first_row
            ..(state.first_row + state.visible_rows + 1).min(self.filtered.len())
        {
            if let Some(hit) = state.row_hit(index, scale, residual) {
                self.row_hits.push((index, hit));
            }
        }
    }
    pub fn clickable_at(&self, x: f32, y: f32) -> bool {
        self.is_active()
            && self.error.is_none()
            && self.row_hits.iter().any(|(index, rect)| {
                contains(*rect, x, y) && !self.rows[self.filtered[*index]].is_disabled
            })
    }

    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        if !self.is_active() || self.error.is_some() {
            return false;
        }
        let Some(index) = self.row_hits.iter().find_map(|(index, rect)| {
            (contains(*rect, x, y) && !self.rows[self.filtered[*index]].is_disabled)
                .then_some(*index)
        }) else {
            return false;
        };
        if self.selected == Some(index) {
            return false;
        }
        self.selected = Some(index);
        self.animate_cursor();
        self.selection_touched = true;
        true
    }

    pub fn pointer_down(
        &mut self,
        x: f32,
        y: f32,
    ) -> Option<ConversationNavigatorAction> {
        if !self.is_active() {
            return None;
        }
        let bounds = self.bounds?;
        if !contains(bounds, x, y) {
            self.close();
            return Some(ConversationNavigatorAction::Dismiss);
        }
        for (i, r) in &self.row_hits {
            if contains(*r, x, y) {
                let row = &self.rows[self.filtered[*i]];
                if row.is_disabled {
                    return None;
                }
                self.selected = Some(*i);
                self.selection_touched = true;
                return Some(ConversationNavigatorAction::Activate(row.value.clone()));
            }
        }
        None
    }
    /// Wheel scrolling in logical pixels (positive = down). Consumes input
    /// while active, including at bounds; the short follower owns no momentum.
    pub fn scroll(&mut self, delta: f32) -> bool {
        if !self.is_active() {
            return false;
        }
        if !delta.is_finite() {
            return true;
        }
        self.set_scroll_target(self.scroll.motion.target() + delta);
        self.invalidate();
        true
    }
    /// Direct touch: apply logical pixels to the painted position, discarding
    /// any previous wheel/keyboard destination and both animation tails.
    pub fn scroll_immediate(&mut self, delta: f32) -> bool {
        if !self.is_active() {
            return false;
        }
        if !delta.is_finite() {
            return true;
        }
        self.clamp_scroll();
        self.scroll
            .snap((self.scroll.motion.current() + delta).clamp(0.0, self.max_scroll()));
        self.snap_cursor();
        self.invalidate();
        true
    }
}
/// Plain-text, single-line preview starting near the earliest query term. Character
/// indexing keeps excerpts safe for Unicode messages and never renders HTML.
fn query_match(chars: &[char], query: &str) -> Option<(usize, usize)> {
    query
        .split_whitespace()
        .filter_map(|term| {
            let term: Vec<char> = term
                .trim_matches(|c: char| !c.is_alphanumeric())
                .chars()
                .collect();
            if term.is_empty() || term.len() > chars.len() {
                return None;
            }
            chars
                .windows(term.len())
                .position(|window| {
                    window
                        .iter()
                        .zip(&term)
                        .all(|(a, b)| a.to_lowercase().eq(b.to_lowercase()))
                })
                .map(|start| (start, term.len()))
        })
        .min_by_key(|(start, _)| *start)
}

fn matching_excerpt(text: &str, query: &str) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let chars: Vec<char> = normalized.chars().collect();
    let matched = query_match(&chars, query).map(|(start, _)| start);
    // Keep sentence context; the renderer can trim its lead if measured space
    // is too narrow to reach the match, without sacrificing the second line.
    let start = matched.unwrap_or(0).saturating_sub(12);
    let end = (start + 160).min(chars.len());
    format!(
        "{}{}{}",
        if start > 0 { "…" } else { "" },
        chars[start..end].iter().collect::<String>(),
        if end < chars.len() { "…" } else { "" }
    )
}

fn contains([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settle(n: &mut ConversationNavigator) {
        for _ in 0..64 {
            n.tick_scroll(0.01);
            n.cursor.tick(0.01);
            if !n.scroll.motion.is_animating() && !n.cursor.motion.is_animating() {
                return;
            }
        }
        assert!(!n.scroll.motion.is_animating());
        assert!(!n.cursor.motion.is_animating());
    }

    #[test]
    fn larger_wheel_steps_do_not_snap_a_visible_remainder_at_a_deadline() {
        let mut n = list(ConversationNavigatorMode::Search);
        n.scroll(500.0);
        for _ in 0..11 {
            n.tick_scroll(0.01);
        }
        let before = n.scroll.motion.current();
        n.tick_scroll(0.01);
        assert!(n.scroll.motion.current() < 500.0);
        assert!(n.scroll.motion.current() - before < 30.0);
        settle(&mut n);
        assert_eq!(n.scroll.motion.current(), 500.0);
        assert!(!n.scroll.motion.is_animating());
    }

    fn list(mode: ConversationNavigatorMode) -> ConversationNavigator {
        let mut n = ConversationNavigator::default();
        n.open(mode, (0..20).map(|i| row(&i.to_string(), false)).collect());
        n.visible = 4;
        n
    }

    #[test]
    fn sidebar_anchored_projects_use_smooth_list_cursor_and_clipped_hits() {
        let mut n = list(ConversationNavigatorMode::Projects);
        let viewport = [0.0, 0.0, 1200.0, 900.0];
        n.set_visible_rows(n.row_capacity(viewport, 1.0));
        n.set_anchor(Some([16.0, 108.0, 250.0, 34.0]));
        let rect = n.bounds_for_viewport(viewport, 1.0);
        assert_eq!(rect[0], 16.0);
        assert_eq!(rect[1], 148.0);

        n.scroll(13.5);
        let (first, residual) = n.tick_scroll(0.016);
        assert_eq!(first, 0);
        assert!(residual < 0.0 && residual > -13.5);
        n.select_next();
        n.cursor.tick(0.016);
        assert!(n.cursor.motion.current() > 0.0);
        assert!(n.cursor.motion.current() < n.row_height());
        assert_eq!(n.scroll.motion.target(), 13.5);
        settle(&mut n);

        for _ in 0..5 {
            n.select_next();
        }
        assert_eq!(n.selected_value(), Some("6"));
        assert_eq!(n.scroll.motion.target(), n.row_height());
        let (first, residual) = n.tick_scroll(0.016);
        assert!(residual < -13.5 && residual > -n.row_height());
        assert_eq!(n.bounds_for_viewport(viewport, 1.0), rect);
        n.set_render_geometry(
            NavigatorRenderGeometry {
                row_height: n.row_height(),
                rect,
                first_row: first,
                visible_rows: n.visible,
            },
            1.0,
            residual,
        );
        assert!(!n.clickable_at(rect[0] + 10.0, rect[1] + 10.0));
        assert!(!n.clickable_at(rect[0] + 10.0, rect[1] + rect[3] - 1.0));
        let (index, hit) = *n.row_hits.last().unwrap();
        assert_eq!(index, 6);
        assert!(hit[3] > 0.0 && hit[3] < n.row_height());
        assert_eq!(
            n.pointer_down(hit[0] + 10.0, hit[1] + hit[3] / 2.0),
            Some(ConversationNavigatorAction::Activate("6".into()))
        );

        let current = n.scroll.motion.current();
        n.scroll_immediate(4.25);
        assert_eq!(n.scroll.motion.current(), current + 4.25);
        assert_eq!(n.scroll.motion.target(), current + 4.25);
        assert!(!n.scroll.motion.is_animating());
        assert!(!n.cursor.motion.is_animating());
    }

    #[test]
    fn subrow_wheel_tracks_pixels_and_settles_without_glide() {
        let mut n = list(ConversationNavigatorMode::Search);
        assert!(n.scroll(7.25));
        assert!(n.scroll(3.5));
        assert_eq!(n.scroll.motion.target(), 10.75);
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
        let (first, residual) = n.tick_scroll(0.01);
        assert_eq!(first, 0);
        assert!(residual < 0.0 && residual > -10.75);
        settle(&mut n);
        assert_eq!(n.tick_scroll(0.0), (0, -10.75));
        for _ in 0..100 {
            assert_eq!(n.tick_scroll(0.05), (0, -10.75));
        }
        n.scroll(-2.25);
        settle(&mut n);
        assert_eq!(n.scroll.motion.current(), 8.5);
        n.scroll(f32::NAN);
        assert_eq!(n.scroll.motion.target(), 8.5);
        n.scroll(-1000.0);
        settle(&mut n);
        assert_eq!(n.scroll.motion.current(), 0.0);
    }

    #[test]
    fn arrows_animate_highlight_inside_window_and_reveal_at_edge() {
        let mut n = list(ConversationNavigatorMode::Projects);
        n.select_next();
        assert_eq!(n.selected_value(), Some("1"));
        assert_eq!(n.scroll.motion.target(), 0.0);
        assert_eq!(n.cursor.motion.current(), 0.0);
        n.cursor.tick(0.016);
        assert!(
            n.cursor.motion.current() > 0.0 && n.cursor.motion.current() < n.row_height()
        );
        settle(&mut n);
        assert_eq!(n.cursor.motion.current(), n.row_height());
        for _ in 0..3 {
            n.select_next();
        }
        assert_eq!(n.selected_value(), Some("4"));
        assert_eq!(n.scroll.motion.target(), n.row_height());
        let (_, residual) = n.tick_scroll(0.016);
        assert!(residual < 0.0 && residual > -n.row_height());
        settle(&mut n);
        assert_eq!(n.tick_scroll(0.0), (1, 0.0));
        assert_eq!(n.cursor.motion.current(), 4.0 * n.row_height());
        n.select_previous();
        settle(&mut n);
        assert_eq!(n.scroll.motion.current(), n.row_height());
        assert_eq!(n.cursor.motion.current(), 3.0 * n.row_height());
    }

    #[test]
    fn refresh_preserves_both_fractional_anchors_and_pending_selection() {
        for mode in [
            ConversationNavigatorMode::Search,
            ConversationNavigatorMode::Projects,
        ] {
            let mut n = list(mode);
            n.select_next();
            n.scroll_immediate(2.0 * n.row_height() + 11.25);
            n.scroll(2.0 * n.row_height() + 7.5);
            n.tick_scroll(0.016);
            let current = n.scroll.motion.current();
            let target = n.scroll.motion.target();
            let mut rows = vec![row("new", false)];
            rows.extend((0..20).map(|i| row(&i.to_string(), false)));
            n.set_rows(rows);
            assert_eq!(n.selected_value(), Some("1"));
            assert!((n.scroll.motion.current() - current - n.row_height()).abs() < 0.001);
            assert!((n.scroll.motion.target() - target - n.row_height()).abs() < 0.001);
            assert_eq!(n.cursor.motion.current(), 2.0 * n.row_height());
            settle(&mut n);
            assert!((n.scroll.motion.current() - target - n.row_height()).abs() < 0.001);
        }
    }

    #[test]
    fn refresh_translates_in_flight_cursor_and_hover_animates_without_scrolling() {
        let mut n = list(ConversationNavigatorMode::Search);
        n.select_next();
        n.cursor.tick(0.016);
        let before = n.cursor.motion.current();
        let mut rows = vec![row("inserted", false)];
        rows.extend((0..20).map(|i| row(&i.to_string(), false)));
        n.set_rows(rows);
        assert_eq!(n.selected_value(), Some("1"));
        assert!((n.cursor.motion.current() - before - n.row_height()).abs() < 0.001);
        assert_eq!(n.cursor.motion.target(), 2.0 * n.row_height());
        settle(&mut n);
        let list_position = n.scroll.motion.current();
        n.row_hits = vec![(3, [0.0, 52.0, 300.0, n.row_height()])];
        assert!(n.pointer_move(10.0, 60.0));
        assert_eq!(n.selected_value(), Some("2"));
        n.cursor.tick(0.016);
        assert!(n.cursor.motion.current() > 2.0 * n.row_height());
        assert!(n.cursor.motion.current() < 3.0 * n.row_height());
        assert_eq!(n.scroll.motion.current(), list_position);
        settle(&mut n);
        for _ in 0..100 {
            n.cursor.tick(0.05);
            assert_eq!(n.cursor.motion.current(), 3.0 * n.row_height());
        }
    }

    #[test]
    fn resize_shrink_and_immediate_touch_clamp_and_cancel_tails() {
        let mut n = list(ConversationNavigatorMode::Search);
        n.scroll(10000.0);
        n.tick_scroll(0.016);
        let current = n.scroll.motion.current();
        n.select_next();
        n.scroll_immediate(3.25);
        assert_eq!(n.scroll.motion.current(), current + 3.25);
        assert_eq!(n.scroll.motion.target(), current + 3.25);
        assert!(!n.cursor.motion.is_animating());
        assert!(n.scroll.last_frame.is_none());
        settle(&mut n);
        n.scroll_immediate(10000.0);
        n.visible = 8;
        assert_eq!(n.tick_scroll(0.0), (12, 0.0));
        n.set_rows((0..3).map(|i| row(&i.to_string(), false)).collect());
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
        assert_eq!(n.scroll.motion.target(), 0.0);
        n.set_rows(vec![]);
        settle(&mut n);
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
    }

    #[test]
    fn query_close_open_reset_motion_and_idle_clock_starts_at_input() {
        let mut n = list(ConversationNavigatorMode::Search);
        n.scroll(50.0);
        n.select_next();
        n.tick_scroll(0.02);
        n.set_query("new".into());
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
        assert!(!n.cursor.motion.is_animating());
        assert!(n.scroll.last_frame.is_none());
        let stale = Instant::now() - std::time::Duration::from_secs(3600);
        n.scroll.last_frame = Some(stale);
        n.scroll(12.5);
        assert!(n.scroll.last_frame.unwrap() > stale);
        let dt = n.scroll.frame_dt(n.scroll.last_frame.unwrap());
        assert_eq!(dt, 0.0);
        assert_eq!(n.tick_scroll(dt), (0, 0.0));
        n.close();
        assert!(!n.scroll_immediate(1.0));
        assert_eq!(n.scroll.motion.current(), 0.0);
        assert!(!n.cursor.motion.is_animating());
        n.open(ConversationNavigatorMode::Search, vec![row("new", false)]);
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
        assert_eq!(n.cursor.motion.current(), 0.0);
    }

    #[test]
    fn resizing_reveals_selection_but_unchanged_layout_preserves_wheel_scroll() {
        let mut n = list(ConversationNavigatorMode::Search);
        for _ in 0..3 {
            n.select_next();
        }
        settle(&mut n);
        assert_eq!(n.scroll.motion.current(), 0.0);
        n.set_visible_rows(2);
        assert_eq!(n.scroll.motion.target(), 2.0 * n.row_height());
        settle(&mut n);
        n.scroll_immediate(5.0 * n.row_height());
        let scrolled = n.scroll.motion.current();
        n.set_visible_rows(2);
        assert_eq!(n.scroll.motion.current(), scrolled);
        assert_eq!(n.scroll.motion.target(), scrolled);
    }

    #[test]
    fn project_query_reset_keeps_current_selection_visible_without_old_motion() {
        let mut rows: Vec<_> = (0..20).map(|i| row(&i.to_string(), false)).collect();
        rows[15].is_current = true;
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Projects, rows);
        n.set_visible_rows(4);
        n.set_query("Rust".into());
        assert_eq!(n.selected_value(), Some("15"));
        assert_eq!(n.scroll.motion.current(), 12.0 * n.row_height());
        assert!(!n.scroll.motion.is_animating());
        assert!(!n.cursor.motion.is_animating());
    }

    #[test]
    fn fractional_hit_rows_match_snapped_render_clip_at_every_scale() {
        for mode in [
            ConversationNavigatorMode::Search,
            ConversationNavigatorMode::Projects,
        ] {
            for s in [0.5, 1.0, 1.25, 2.0, 3.0] {
                let mut n = list(mode);
                n.rows[2].is_disabled = true;
                n.scroll_immediate(n.row_height() + 10.25);
                let (first, residual) = n.tick_scroll(0.0);
                let state = NavigatorRenderGeometry {
                    row_height: n.row_height(),
                    rect: [
                        20.0,
                        30.0,
                        300.0 * s,
                        header_height(s) + frame_inset(s) + 4.0 * n.row_height() * s,
                    ],
                    first_row: first,
                    visible_rows: 4,
                };
                n.set_render_geometry(state, s, residual);
                let top = state.body(s)[1];
                let bottom = state.body(s)[1] + state.body(s)[3];
                let snapped = crate::primitives::snap_to_device_px(residual * s, s);
                assert_eq!(n.row_hits.len(), 5);
                assert_eq!(n.row_hits[0].0, 1);
                assert_eq!(n.row_hits[0].1[1], top);
                assert!(
                    (n.row_hits[0].1[3] - (n.row_height() * s + snapped)).abs() < 0.001
                );
                assert_eq!(n.row_hits[4].0, 5);
                assert!((n.row_hits[4].1[3] + snapped).abs() < 0.001);
                let hit_x = state.body(s)[0] + 1.0;
                assert!(!n.clickable_at(hit_x, top - 0.01));
                assert!(!n.clickable_at(hit_x, bottom));
                assert!(n.clickable_at(hit_x, top));
                assert!(n.pointer_move(hit_x, bottom - 0.01));
                assert_eq!(n.selected_value(), Some("5"));
                let disabled_y = n.row_hits[1].1[1] + 1.0;
                assert!(!n.clickable_at(hit_x, disabled_y));
                assert_eq!(n.pointer_down(hit_x, disabled_y), None);
                assert_eq!(
                    n.pointer_down(hit_x, bottom - 0.01),
                    Some(ConversationNavigatorAction::Activate("5".into()))
                );
                assert_eq!(n.pointer_down(hit_x, top - 1.0), None);
                assert_eq!(n.bounds, Some(state.rect));
            }
        }
    }

    #[test]
    fn finder_shell_has_one_input_separator_mode_rows_and_no_footer() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![row("one", false)]);
        assert_eq!(n.row_height(), 48.0);
        let viewport = [0.0, 0.0, 1000.0, 800.0];
        let rect = n.bounds_for_viewport(viewport, 1.0);
        let geometry = NavigatorRenderGeometry {
            row_height: n.row_height(),
            rect,
            first_row: 0,
            visible_rows: SEARCH_VISIBLE_ROWS,
        };
        assert_eq!(rect[2], 480.0);
        assert_eq!(
            rect[3],
            (header_height(1.0)
                + SEARCH_VISIBLE_ROWS as f32 * n.row_height()
                + frame_inset(1.0))
            .floor()
        );
        assert_eq!(geometry.input(1.0)[3], 40.0);
        assert_eq!(header_height(1.0), frame_inset(1.0) + 40.0 + 1.0 + 4.0);
        let body = geometry.body(1.0);
        assert_eq!(body[3], SEARCH_VISIBLE_ROWS as f32 * n.row_height());
        n.set_render_geometry(geometry, 1.0, 0.0);
        let x = body[0] + 1.0;
        assert_eq!(n.row_hits.len(), 1);
        assert!(!n.clickable_at(x, body[1] - 1.0));
        assert!(!n.clickable_at(rect[0] + 1.0, body[1] + 1.0));
        assert!(!n.clickable_at(x, body[1] + body[3]));
        assert!(!n.clickable_at(x, body[1] + n.row_height() + 1.0));
        assert!(n.clickable_at(x, body[1]));
        n.set_rows(vec![row("one", false), row("two", false)]);
        assert_eq!(n.bounds_for_viewport(viewport, 1.0), rect);
        n.set_loading(true);
        n.set_rows(vec![]);
        assert_eq!(n.layout_rows(), SEARCH_VISIBLE_ROWS);
        assert_eq!(n.bounds_for_viewport(viewport, 1.0), rect);
        n.set_error(Some("Offline".into()));
        assert_eq!(n.layout_rows(), SEARCH_VISIBLE_ROWS);
        assert_eq!(n.bounds_for_viewport(viewport, 1.0), rect);
    }

    #[test]
    fn search_bounds_stay_fixed_during_loading_results_empty_and_error() {
        for scale in [0.75, 1.0, 1.25, 2.0] {
            for viewport in [[0.0, 0.0, 1200.0, 900.0], [20.0, 30.0, 360.0, 260.0]] {
                let mut n = ConversationNavigator::default();
                n.open(ConversationNavigatorMode::Search, vec![]);
                let rect = n.bounds_for_viewport(viewport, scale);
                n.set_loading(true);
                assert_eq!(n.bounds_for_viewport(viewport, scale), rect);
                for count in [1, 3, 8, 16] {
                    n.set_rows((0..count).map(|i| row(&i.to_string(), false)).collect());
                    n.set_loading(false);
                    assert_eq!(n.bounds_for_viewport(viewport, scale), rect);
                }
                n.set_error(Some("Offline".into()));
                assert_eq!(n.bounds_for_viewport(viewport, scale), rect);
                n.set_error(None);
                n.set_rows(vec![]);
                assert_eq!(n.bounds_for_viewport(viewport, scale), rect);
            }
        }
    }

    #[test]
    fn snapped_paint_and_hits_stay_inside_tiny_and_fractional_hosts() {
        let mut n = list(ConversationNavigatorMode::Projects);
        n.set_anchor(Some([18.3, 42.7, 340.0, 34.0]));
        for s in [0.5, 1.0, 1.25, 2.0, 3.0] {
            for viewport in [
                [10.3, 20.1, 4.0, 3.0],
                [10.3, 20.1, 160.0, 120.0],
                [0.0, 0.0, 1000.0, 800.0],
            ] {
                let rect = n.bounds_for_viewport(viewport, s);
                assert!(rect[0] >= viewport[0] && rect[1] >= viewport[1]);
                assert!(rect[0] + rect[2] <= viewport[0] + viewport[2]);
                assert!(rect[1] + rect[3] <= viewport[1] + viewport[3]);
                let geometry = NavigatorRenderGeometry {
                    row_height: n.row_height(),
                    rect,
                    first_row: 0,
                    visible_rows: n.row_capacity(viewport, s),
                };
                n.set_render_geometry(geometry, s, -10.25);
                for (index, hit) in &n.row_hits {
                    assert_eq!(Some(*hit), geometry.row_hit(*index, s, -10.25));
                    assert_eq!(
                        Some(*hit),
                        crate::primitives::geom::intersect_rect(*hit, rect)
                    );
                }
            }
        }
    }

    #[test]
    fn live_status_only_occupies_input_with_real_hits() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![]);
        n.set_indexing(true);
        assert_eq!(n.input_status(), None); // body status, no badge or fake hits
        n.set_rows(vec![row("hit", false)]);
        assert_eq!(n.input_status(), Some("Indexing…"));
        assert_eq!(n.selected_value(), Some("hit"));
        n.set_indexing(false);
        n.set_loading(true);
        assert_eq!(n.input_status(), Some("Loading…"));
        n.set_error(Some("Offline".into()));
        assert_eq!(n.input_status(), None);
        assert_eq!(n.selected_value(), None);
        n.set_error(None);
        n.set_loading(false);
        assert_eq!(n.input_status(), None);
    }

    #[test]
    fn search_excerpt_keeps_source_and_duplicate_project_context() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![]);
        n.set_query("needle".into());
        let mut hit = row("same title", false);
        hit.description = format!("{} needle context", "界".repeat(200));
        hit.footer = "Project · /work/duplicate · Messages".into();
        n.set_rows(vec![hit]);
        assert!(n.rows[0].description.contains("needle context"));
        assert_eq!(n.rows[0].footer, "Project · /work/duplicate · Messages");
        assert_eq!(n.selected_value(), Some("same title"));
    }

    #[test]
    fn loading_skeletons_fill_mode_viewport_until_real_hits() {
        for mode in [
            ConversationNavigatorMode::Search,
            ConversationNavigatorMode::Projects,
        ] {
            let mut n = ConversationNavigator::default();
            n.open(mode, vec![]);
            let search = mode == ConversationNavigatorMode::Search;
            let empty_rows = if search { SEARCH_VISIBLE_ROWS } else { 1 };
            let loading_rows = if search { SEARCH_VISIBLE_ROWS } else { 3 };
            assert_eq!(n.layout_rows(), empty_rows);
            n.set_loading(true);
            assert!(n.show_skeletons());
            assert_eq!(n.layout_rows(), loading_rows);
            assert_eq!(n.row_height(), if search { 48.0 } else { 32.0 });
            for s in [0.5, 1.0, 1.25, 2.0, 3.0] {
                let viewport = [0.0, 0.0, 1800.0, 1800.0];
                let rect = n.bounds_for_viewport(viewport, s);
                let geometry = NavigatorRenderGeometry {
                    rect,
                    first_row: 0,
                    visible_rows: loading_rows,
                    row_height: n.row_height(),
                };
                assert!(
                    (geometry.body(s)[3] - loading_rows as f32 * n.row_height() * s)
                        .abs()
                        < 0.001
                );
                n.set_render_geometry(geometry, s, 0.0);
                assert!(n.row_hits.is_empty());
                for index in 0..loading_rows {
                    let bars = geometry.skeleton_bars(index, s);
                    assert_eq!(bars.len(), 2);
                    assert_eq!(bars[1][1] > bars[0][1], search);
                    for bar in bars {
                        let body = geometry.body(s);
                        assert!(bar[0] >= body[0] && bar[1] >= body[1]);
                        assert!(bar[0] + bar[2] <= body[0] + body[2] + 0.001);
                        assert!(bar[1] + bar[3] <= body[1] + body[3] + 0.001);
                    }
                }
            }
            assert_ne!(
                render::skeleton_alpha(0.0, 0),
                render::skeleton_alpha(0.25, 0)
            );
            assert!(render::skeleton_alpha(f32::NAN, 0).is_finite());
            n.set_rows(vec![row("real hit", false)]);
            assert!(!n.show_skeletons());
            assert_eq!(n.selected_value(), Some("real hit"));
            assert_eq!(n.input_status(), Some("Loading…"));
            n.set_rows(vec![]);
            n.set_error(Some("Offline".into()));
            assert!(!n.show_skeletons());
            assert_eq!(n.layout_rows(), empty_rows);
            n.set_error(None);
            n.set_loading(false);
            assert!(!n.show_skeletons());
            assert_eq!(n.layout_rows(), empty_rows);
        }
    }

    #[test]
    fn search_sentence_has_full_second_line_independent_of_source() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![row("title", false)]);
        let rect = n.bounds_for_viewport([0.0, 0.0, 1000.0, 800.0], 1.0);
        let geometry = NavigatorRenderGeometry {
            rect,
            first_row: 0,
            visible_rows: 1,
            row_height: n.row_height(),
        };
        let (first, sentence) = geometry.text_lines(0, 1.0, 0.0);
        let sentence = sentence.unwrap();
        assert_eq!(first[2], sentence[2]);
        assert_eq!(sentence[2], geometry.body(1.0)[2] - 28.0);
        assert!(sentence[1] >= first[1] + first[3]);
        assert_eq!(sentence[1] + sentence[3], geometry.body(1.0)[1] + 42.0);
        n.rows[0].footer = "Neoism · /full/project/path · Messages".into();
        assert_eq!(geometry.text_lines(0, 1.0, 0.0).1, Some(sentence));
        // A deterministic variable-width measurer makes leading CJK context
        // expensive: the query must still reach a narrow real width budget.
        let excerpt = matching_excerpt(
            &format!("{} needle explains this hit", "界".repeat(30)),
            "needle",
        );
        let fitted = render::fit_excerpt(&excerpt, "NEEDLE", 70.0, |text| {
            text.chars()
                .map(|c| if c == '界' { 12.0 } else { 6.0 })
                .sum()
        });
        assert!(fitted.contains("needle explains this hit"));
        let prefix = fitted.split("needle").next().unwrap();
        let prefix_width: f32 = prefix
            .chars()
            .map(|c| if c == '界' { 12.0 } else { 6.0 })
            .sum();
        assert!(prefix_width + 6.0 * 6.0 <= 70.0);
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("project", false)],
        );
        let project = NavigatorRenderGeometry {
            row_height: n.row_height(),
            ..geometry
        };
        assert!(project.text_lines(0, 1.0, 0.0).1.is_none());
    }

    fn row(value: &str, disabled: bool) -> ConversationNavigatorRow {
        ConversationNavigatorRow {
            value: value.into(),
            title: value.into(),
            description: "/work/repo".into(),
            footer: "Rust".into(),
            is_disabled: disabled,
            ..Default::default()
        }
    }
    #[test]
    fn indexing_keeps_real_hits_selectable_and_resets_with_query_or_close() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![]);
        n.set_query("needle".into());
        n.set_indexing(true);
        assert_eq!(
            n.indexing_status(),
            Some("Indexing conversation history...")
        );
        assert_eq!(n.selected_value(), None);
        n.set_rows(vec![row("real hit", false)]);
        assert_eq!(n.selected_value(), Some("real hit"));
        assert!(n.indexing_status().is_some());
        n.set_indexing(false);
        assert!(n.indexing_status().is_none());
        assert_eq!(n.selected_value(), Some("real hit"));
        n.set_indexing(true);
        n.set_error(Some("Network error".into()));
        assert!(n.indexing_status().is_none());
        n.set_error(None);
        n.set_query("another".into());
        assert!(n.indexing_status().is_none());
        n.set_indexing(true);
        n.close();
        assert!(n.indexing_status().is_none());
        n.open(ConversationNavigatorMode::Projects, vec![]);
        n.set_indexing(true);
        assert!(n.indexing_status().is_none());
    }

    #[test]
    fn search_preserves_server_rank_and_keeps_unicode_match_visible() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Search, vec![]);
        n.set_query("NEEDLE".into());
        let mut first = row("unrelated title", false);
        first.description =
            format!("{}\nneedle matched here\tand context", "界".repeat(200));
        n.set_rows(vec![first, row("needle title", false)]);
        assert_eq!(n.selected_value(), Some("unrelated title"));
        assert_eq!(n.filtered, vec![0, 1]);
        assert!(n.rows[0].description.starts_with('…'));
        assert!(n.rows[0]
            .description
            .chars()
            .take(19)
            .collect::<String>()
            .contains("needle"));
        assert!(n.rows[0]
            .description
            .contains("needle matched here and context"));
        assert!(!n.rows[0].description.contains('\n'));
        assert!(n.rows[0].description.chars().count() <= 162);
        n.select_next();
        assert_eq!(n.selected_value(), Some("needle title"));
        n.select_previous();
        assert_eq!(n.selected_value(), Some("unrelated title"));
        assert_eq!(matching_excerpt("", "test"), "");
    }

    #[test]
    fn pointer_motion_selects_enabled_rows_without_activation_or_stale_hits() {
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            vec![
                row("first", false),
                row("disabled", true),
                row("other", false),
            ],
        );
        n.row_hits = vec![
            (0, [0., 52., 300., n.row_height()]),
            (1, [0., 84., 300., n.row_height()]),
            (2, [0., 116., 300., n.row_height()]),
        ];
        assert!(n.clickable_at(10., 60.));
        assert!(!n.clickable_at(10., 90.));
        assert!(n.clickable_at(10., 120.));
        assert!(!n.clickable_at(400., 120.));
        assert!(!n.pointer_move(10., 90.));
        assert_eq!(n.selected_value(), Some("first"));
        assert!(n.pointer_move(10., 120.));
        assert_eq!(n.selected_value(), Some("other"));
        assert!(!n.pointer_move(10., 120.));
        assert!(!n.pointer_move(400., 120.));
        n.set_rows(vec![row("first", false), row("other", false)]);
        assert_eq!(n.selected_value(), Some("other"));
        assert!(!n.pointer_move(10., 60.));
        assert!(!n.clickable_at(10., 60.));
    }

    #[test]
    fn disabled_items_keep_path_footer_and_current_marker() {
        let mut disabled = row("Projects", true);
        disabled.is_current = true;
        assert_eq!(disabled.description, "/work/repo");
        assert_eq!(disabled.footer, "Rust");
        assert!(disabled.is_current);
        assert!(disabled.is_disabled);
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Projects, vec![disabled]);
        n.select_next();
        assert_eq!(n.selected_value(), None);
    }

    #[test]
    fn refresh_preserves_explicit_selection_and_top_visible_identity() {
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            (0..12).map(|i| row(&i.to_string(), false)).collect(),
        );
        n.visible = 4;
        n.select_next();
        n.select_next();
        n.scroll(n.row_height() * 5.0);
        assert_eq!(n.scroll.motion.current(), 0.0);
        assert_eq!(n.scroll.motion.target(), 5.0 * n.row_height());
        settle(&mut n);
        assert_eq!(n.tick_scroll(0.0), (5, 0.0));
        let mut rows = vec![row("inserted", false)];
        rows.extend((0..12).map(|i| row(&i.to_string(), false)));
        n.set_rows(rows);
        assert_eq!(n.selected_value(), Some("2"));
        let (first, residual) = n.tick_scroll(0.0);
        assert_eq!(n.rows[n.filtered[first]].value, "5");
        assert_eq!(residual, 0.0);
        assert!(n.row_hits.is_empty());
    }

    #[test]
    fn projects_keep_dropdown_width_and_search_uses_finder_width() {
        let viewport = [0., 0., 1000., 800.];
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("one", false); 20],
        );
        n.set_anchor(Some([20., 20., 900., 34.]));
        let bounds = n.bounds_for_viewport(viewport, 1.0);
        assert_eq!(bounds[2], 420.);
        assert_eq!(
            bounds[3],
            (header_height(1.0) + frame_inset(1.0) + 6.0 * n.row_height()).floor()
        );
        n.open(
            ConversationNavigatorMode::Search,
            vec![row("one", false); 20],
        );
        assert_eq!(
            n.bounds_for_viewport(viewport, 1.0)[2..],
            [
                480.,
                (header_height(1.0) + frame_inset(1.0) + 8.0 * n.row_height()).floor()
            ]
        );
    }

    #[test]
    fn projects_select_current_root_but_keep_explicit_keyboard_choices() {
        let mut n = ConversationNavigator::default();
        n.open(ConversationNavigatorMode::Projects, vec![]);
        let home = row("home", false);
        n.set_rows(vec![home.clone()]);
        let mut current = row("current", false);
        current.is_current = true;
        n.set_rows(vec![home.clone(), current.clone()]);
        assert_eq!(n.selected_value(), Some("current"));
        n.select_previous();
        assert_eq!(n.selected_value(), Some("home"));
        n.set_rows(vec![home, current, row("later", false)]);
        assert_eq!(n.selected_value(), Some("home"));
    }

    #[test]
    fn filtering_and_disabled_selection() {
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("disabled", true), row("one", false), row("two", false)],
        );
        assert_eq!(n.selected_value(), Some("one"));
        n.select_previous();
        assert_eq!(n.selected_value(), Some("two"));
        n.set_query("RUST".into());
        assert_eq!(n.filtered.len(), 3);
        n.set_query("/work".into());
        assert_eq!(n.filtered.len(), 3);
        n.set_query("absent".into());
        assert_eq!(n.selected_value(), None);
        n.open(ConversationNavigatorMode::Search, vec![row("one", false)]);
        n.set_query("server query".into());
        assert_eq!(n.selected_value(), Some("one"));
        n.close();
        assert!(!n.is_active());
    }
    #[test]
    fn bounds_clamp_and_anchor() {
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("one", false); 20],
        );
        n.set_anchor(Some([990., 790., 200., 34.]));
        for viewport in [
            [0., 0., 1000., 800.],
            [10., 20., 160., 120.],
            [5., 5., 4., 3.],
        ] {
            let r = n.bounds_for_viewport(viewport, 2.0);
            assert!(r[0] >= viewport[0] && r[1] >= viewport[1]);
            assert!(r[0] + r[2] <= viewport[0] + viewport[2] + 0.01);
            assert!(r[1] + r[3] <= viewport[1] + viewport[3] + 0.01);
        }
        n.set_anchor(Some([40., 40., 300., 34.]));
        assert_eq!(n.bounds_for_viewport([0., 0., 1000., 800.], 1.0)[1], 80.);
    }
    #[test]
    fn pointer_activation_dismissal_and_wheel_consumption() {
        let mut n = ConversationNavigator::default();
        assert!(!n.scroll(n.row_height()));
        n.open(
            ConversationNavigatorMode::Search,
            vec![row("disabled", true), row("chat", false)],
        );
        n.bounds = Some([0., 0., 200., 200.]);
        n.row_hits = vec![
            (0, [0., 52., 200., n.row_height()]),
            (1, [0., 100., 200., n.row_height()]),
        ];
        assert_eq!(n.pointer_down(20., 60.), None);
        assert_eq!(
            n.pointer_down(20., 110.),
            Some(ConversationNavigatorAction::Activate("chat".into()))
        );
        assert_eq!(
            n.pointer_down(220., 110.),
            Some(ConversationNavigatorAction::Dismiss)
        );
        assert!(!n.is_active());
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("project", false); 20],
        );
        n.visible = 4;
        assert!(n.scroll(n.row_height() * 2.0));
        assert_eq!(n.scroll.motion.current(), 0.0);
        assert_eq!(n.scroll.motion.target(), n.row_height() * 2.0);
        settle(&mut n);
        assert_eq!(n.tick_scroll(0.0), (2, 0.0));
        n.set_query("missing".into());
        assert_eq!(n.tick_scroll(0.0), (0, 0.0));
        assert!(!n.scroll.motion.is_animating());
    }

    #[test]
    fn rows_preserve_identity_and_clear_stale_hits() {
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Search,
            vec![row("one", false), row("two", false)],
        );
        n.select_next();
        n.set_rows(vec![row("two", false), row("one", false)]);
        assert_eq!(n.selected_value(), Some("two"));
        n.set_rows(vec![row("two", true)]);
        assert_eq!(n.selected_value(), None);
        assert!(n.row_hits.is_empty());
    }
}
