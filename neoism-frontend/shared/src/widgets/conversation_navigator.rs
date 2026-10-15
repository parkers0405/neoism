//! Host-neutral conversation search modal / project selector. No I/O or input ownership.
use super::inline_picker::{self, InlinePickerRow, InlinePickerView};
use crate::primitives::IdeTheme;
use sugarloaf::Sugarloaf;

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

impl ConversationNavigatorRow {
    fn picker_row(&self) -> InlinePickerRow<'_> {
        InlinePickerRow {
            title: &self.title,
            description: &self.description,
            footer: &self.footer,
            is_header: false,
            is_current: self.is_current,
            is_pinned: false,
            status: self
                .is_disabled
                .then_some(inline_picker::InlinePickerStatus::Muted),
        }
    }
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
    offset: usize,
    loading: bool,
    error: Option<String>,
    anchor: Option<[f32; 4]>,
    bounds: Option<[f32; 4]>,
    row_hits: Vec<(usize, [f32; 4])>,
    visible: usize,
    wheel: f32,
}

impl ConversationNavigator {
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
    }
    pub fn close(&mut self) {
        *self = Self::default();
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
            self.selection_touched = false;
            self.offset = 0;
            self.refilter();
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
    pub fn set_rows(&mut self, rows: Vec<ConversationNavigatorRow>) {
        let previous = (self.mode == Some(ConversationNavigatorMode::Search)
            || self.selection_touched)
            .then(|| self.selected_value().map(str::to_owned))
            .flatten();
        let top = self
            .filtered
            .get(self.offset)
            .map(|i| self.rows[*i].value.clone());
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
        if self.projects() {
            if let Some(i) = top.and_then(|value| {
                self.filtered
                    .iter()
                    .position(|i| self.rows[*i].value == value)
            }) {
                self.offset =
                    i.min(self.filtered.len().saturating_sub(self.visible.max(1)));
            }
            self.invalidate();
        } else {
            self.reveal();
        }
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
        self.reveal();
    }
    fn reveal(&mut self) {
        let visible = self.visible.max(1);
        self.offset = self.offset.min(self.filtered.len().saturating_sub(visible));
        if let Some(i) = self.selected {
            if i < self.offset {
                self.offset = i;
            }
            if i >= self.offset + visible {
                self.offset = i + 1 - visible;
            }
        }
        self.invalidate();
    }
    fn projects(&self) -> bool {
        self.mode == Some(ConversationNavigatorMode::Projects)
    }
    fn footer_height(&self) -> f32 {
        if self.projects() {
            inline_picker::FOOTER_H
        } else {
            0.0
        }
    }
    fn row_capacity(&self, viewport: [f32; 4], scale: f32) -> usize {
        (((viewport[3] - (16.0 + 54.0 + self.footer_height()) * scale) / (46.0 * scale))
            .floor() as usize)
            .clamp(1, if self.projects() { 6 } else { 8 })
    }
    fn layout_rows(&self) -> usize {
        if self.error.is_some() {
            1
        } else if self.filtered.is_empty() && self.loading {
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
            620.0 * s
        })
        .min(aw);
        let h = ((54.0
            + self.footer_height()
            + 46.0 * self.layout_rows().min(self.row_capacity(viewport, s)) as f32)
            * s)
            .min(ah);
        let (x, y) = if projects {
            self.anchor
                .map_or((vx + (vw - w) / 2.0, vy + (vh - h) / 2.0), |r| {
                    (r[0], r[1] + r[3] + 6.0 * s)
                })
        } else {
            (vx + (vw - w) / 2.0, vy + (vh - h) / 2.0)
        };
        [
            x.clamp(vx + margin, vx + vw - margin - w),
            y.clamp(vy + margin, vy + vh - margin - h),
            w,
            h,
        ]
    }
    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
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
        self.visible = self.row_capacity(viewport, s);
        self.offset = self
            .offset
            .min(self.filtered.len().saturating_sub(self.visible));
        let rect = self.bounds_for_viewport(viewport, s);
        self.bounds = Some(rect);
        // The primitive requires a header and one full row. Never spill on tiny hosts.
        if rect[3] < (100.0 + self.footer_height()) * s || rect[2] < 60.0 * s {
            return;
        }
        let mut rows: Vec<_> = self
            .filtered
            .iter()
            .map(|i| self.rows[*i].picker_row())
            .collect();
        if self.error.is_some() {
            rows.clear();
        }
        let project_footer = self.projects().then(|| {
            if self.error.is_some() || self.filtered.is_empty() {
                "Esc close".to_owned()
            } else {
                format!(
                    "{}-{} of {} · Scroll / ↑↓ · Enter open",
                    self.offset + 1,
                    (self.offset + self.visible).min(self.filtered.len()),
                    self.filtered.len()
                )
            }
        });
        let view = InlinePickerView {
            title: if self.mode == Some(ConversationNavigatorMode::Projects) {
                "Switch project"
            } else {
                "Search conversations"
            },
            query: &self.query,
            selected: self.selected.unwrap_or(usize::MAX),
            scroll_offset: self.offset,
            list_scroll_offset: 0.0,
            cursor_offset: 0.0,
            rows: &rows,
            footer_hint: project_footer.as_deref(),
            rename: None,
            show_search_caret: true,
            wrap_prompt: false,
            search_placeholder: if self.projects() {
                "Find a project..."
            } else {
                "Search"
            },
            loading: self.loading && self.error.is_none(),
            empty_message: Some(self.error.as_deref().unwrap_or("No results")),
            loading_elapsed: now,
        };
        let disabled: Vec<_> = self
            .filtered
            .iter()
            .enumerate()
            .filter_map(|(index, row)| self.rows[*row].is_disabled.then_some(index))
            .collect();
        if let Some(state) = inline_picker::render_limited_with_disabled(
            sugarloaf,
            view,
            [rect[0], rect[1] + rect[3] + 6.0 * s, rect[2], 0.0],
            theme,
            s,
            self.visible,
            rect[1],
            &disabled,
        ) {
            self.bounds = Some(state.rect);
            for i in 0..state.visible_rows {
                let index = state.first_row + i;
                if self.error.is_none() && index < self.filtered.len() {
                    self.row_hits.push((index, state.row_rect(i, s)));
                }
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
    pub fn scroll(&mut self, delta: f32) -> bool {
        if !self.is_active() {
            return false;
        }
        if !delta.is_finite() {
            return true;
        }
        self.wheel += delta;
        let steps = (self.wheel / 46.0).trunc() as isize;
        self.wheel -= steps as f32 * 46.0;
        self.offset = self
            .offset
            .saturating_add_signed(steps)
            .min(self.filtered.len().saturating_sub(self.visible.max(1)));
        self.invalidate();
        true
    }
}
fn contains([x, y, w, h]: [f32; 4], px: f32, py: f32) -> bool {
    px >= x && px < x + w && py >= y && py < y + h
}

#[cfg(test)]
mod tests {
    use super::*;
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
            (0, [0., 54., 300., 46.]),
            (1, [0., 100., 300., 46.]),
            (2, [0., 146., 300., 46.]),
        ];
        assert!(n.clickable_at(10., 60.));
        assert!(!n.clickable_at(10., 110.));
        assert!(n.clickable_at(10., 150.));
        assert!(!n.clickable_at(400., 150.));
        assert!(!n.pointer_move(10., 110.));
        assert_eq!(n.selected_value(), Some("first"));
        assert!(n.pointer_move(10., 150.));
        assert_eq!(n.selected_value(), Some("other"));
        assert!(!n.pointer_move(10., 150.));
        assert!(!n.pointer_move(400., 150.));
        n.set_rows(vec![row("first", false), row("other", false)]);
        assert_eq!(n.selected_value(), Some("other"));
        assert!(!n.pointer_move(10., 60.));
        assert!(!n.clickable_at(10., 60.));
    }

    #[test]
    fn disabled_items_keep_path_footer_and_current_marker() {
        let mut disabled = row("Projects", true);
        disabled.is_current = true;
        let view = disabled.picker_row();
        assert!(!view.is_header);
        assert_eq!(view.description, "/work/repo");
        assert_eq!(view.footer, "Rust");
        assert!(view.is_current);
        assert!(matches!(
            view.status,
            Some(inline_picker::InlinePickerStatus::Muted)
        ));
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
        n.scroll(46.0 * 5.0);
        assert_eq!(n.offset, 5);
        let mut rows = vec![row("inserted", false)];
        rows.extend((0..12).map(|i| row(&i.to_string(), false)));
        n.set_rows(rows);
        assert_eq!(n.selected_value(), Some("2"));
        assert_eq!(n.rows[n.filtered[n.offset]].value, "5");
        assert!(n.row_hits.is_empty());
    }

    #[test]
    fn projects_are_bounded_and_search_dimensions_stay_unchanged() {
        let viewport = [0., 0., 1000., 800.];
        let mut n = ConversationNavigator::default();
        n.open(
            ConversationNavigatorMode::Projects,
            vec![row("one", false); 20],
        );
        n.set_anchor(Some([20., 20., 900., 34.]));
        let bounds = n.bounds_for_viewport(viewport, 1.0);
        assert_eq!(bounds[2], 420.);
        assert_eq!(bounds[3], 54. + 6. * 46. + inline_picker::FOOTER_H);
        n.open(
            ConversationNavigatorMode::Search,
            vec![row("one", false); 20],
        );
        assert_eq!(
            n.bounds_for_viewport(viewport, 1.0)[2..],
            [620., 54. + 8. * 46.]
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
        assert!(!n.scroll(46.0));
        n.open(
            ConversationNavigatorMode::Search,
            vec![row("disabled", true), row("chat", false)],
        );
        n.bounds = Some([0., 0., 200., 200.]);
        n.row_hits = vec![(0, [0., 54., 200., 46.]), (1, [0., 100., 200., 46.])];
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
        assert!(n.scroll(92.));
        assert_eq!(n.offset, 2);
        n.set_query("missing".into());
        assert_eq!(n.offset, 0);
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
