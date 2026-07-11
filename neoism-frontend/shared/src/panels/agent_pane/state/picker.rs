
use web_time::Instant;

use crate::animation::CriticallyDampedSpring;
use crate::widgets::scroll::Scroll;

const PICKER_VISIBLE_ROWS: usize = 8;
const PICKER_ROW_HEIGHT: f32 = 46.0;
// Snappy critically-damped catch-up matching the side-panel home-list
// scroll rework (`side_panel::SCROLL_ANIMATION_LENGTH`), so trackpad /
// wheel / held-arrow scrolling tracks the gesture tightly.
const LIST_SCROLL_ANIMATION_LENGTH: f32 = 0.12;
const CURSOR_ANIMATION_LENGTH: f32 = 0.10;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NeoismAgentPickerKind {
    Slash,
    Agent,
    Model,
    FileMention,
    SkillMention,
    Thinking,
    Session,
    Subagent,
    Skill,
    /// `/connect` stage 1 — the "Connect a provider" list (Popular +
    /// Providers, checkmark for already-connected).
    Connect,
    /// `/connect` stage 2 — "Select auth method" for the chosen provider
    /// (OAuth variants and "Manually enter API Key").
    ConnectAuth,
    /// `/connect` stage 3 — a single-line secret entry. The picker `query`
    /// row IS the input field (API key, or an OAuth token to paste); Enter
    /// submits it. Carries no selectable rows.
    ConnectSecret,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentPickerOption {
    pub title: String,
    pub description: String,
    pub footer: String,
    pub value: String,
    pub section: String,
    pub is_header: bool,
    /// True for the session the user is currently inside — renders
    /// a colored dot in the picker to distinguish "where I am" from
    /// the keyboard-selected row.
    pub is_current: bool,
    /// True for a pinned session — floats it into the "Pinned" section
    /// at the top of the `/sessions` picker and draws a pin marker.
    pub pinned: bool,
}

impl NeoismAgentPickerOption {
    pub fn new(title: &str, description: &str, footer: &str, value: &str) -> Self {
        Self {
            title: title.to_string(),
            description: description.to_string(),
            footer: footer.to_string(),
            value: value.to_string(),
            section: String::new(),
            is_header: false,
            is_current: false,
            pinned: false,
        }
    }

    pub fn model(title: &str, provider: &str, footer: &str, value: &str) -> Self {
        let mut option = Self::new(title, "", footer, value);
        option.section = provider.to_string();
        option
    }

    pub fn header(title: &str) -> Self {
        Self {
            title: title.to_string(),
            description: String::new(),
            footer: String::new(),
            value: String::new(),
            section: title.to_string(),
            is_header: true,
            is_current: false,
            pinned: false,
        }
    }

    pub fn is_selectable(&self) -> bool {
        !self.is_header
    }
}

#[derive(Clone, Debug)]
pub struct NeoismAgentPicker {
    pub kind: NeoismAgentPickerKind,
    pub title: String,
    pub query: String,
    /// Muted placeholder for the empty search/input row. `None` renders the
    /// default "Search"; the `/connect` secret-entry picker sets e.g.
    /// "API key" so the row reads as a labelled single-field input.
    pub search_placeholder: Option<String>,
    pub selected: usize,
    all_options: Vec<NeoismAgentPickerOption>,
    filtered_options: Vec<NeoismAgentPickerOption>,
    /// Top visible row index. Derived from the animated pixel scroll
    /// position each `tick_list_scroll`, so the render window and click
    /// hit-testing track the pixel-smooth scroll exactly.
    pub scroll_offset: usize,
    pub last_rect: Option<[f32; 4]>,
    /// Device-pixel height of the footer-hint band painted at the bottom
    /// of the card last frame (0 when no footer). Kept so click
    /// hit-testing can exclude the footer from the row grid.
    footer_h_px: f32,
    /// Committed continuous scroll position in logical px (`0` = top).
    /// The `list_scroll` spring chases this; no whole-row quantization,
    /// so trackpad / wheel scroll pixel-precisely. Mirrors the home-list
    /// `scroll_px` + `Scroll` model in `state/side_panel.rs`.
    scroll_px: f32,
    list_scroll: Scroll,
    cursor_spring: CriticallyDampedSpring,
    last_list_scroll_frame: Instant,
    last_cursor_frame: Instant,
    /// True while an async search (semantic session search) is in
    /// flight — the empty state renders a skeleton shimmer instead of
    /// "No results".
    pub loading: bool,
    loading_started: Option<Instant>,
}

impl NeoismAgentPicker {
    pub fn new(
        kind: NeoismAgentPickerKind,
        title: &str,
        options: Vec<NeoismAgentPickerOption>,
        selected: usize,
    ) -> Self {
        let selected = selectable_index_near(&options, selected).unwrap_or(0);
        Self {
            kind,
            title: title.to_string(),
            query: String::new(),
            search_placeholder: None,
            selected,
            filtered_options: options.clone(),
            all_options: options,
            scroll_offset: 0,
            last_rect: None,
            footer_h_px: 0.0,
            scroll_px: 0.0,
            list_scroll: Scroll::new()
                .with_animation_length(LIST_SCROLL_ANIMATION_LENGTH),
            cursor_spring: CriticallyDampedSpring::new(),
            last_list_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            loading: false,
            loading_started: None,
        }
    }

    pub fn set_loading(&mut self, loading: bool) {
        if loading && !self.loading {
            self.loading_started = Some(Instant::now());
        }
        if !loading {
            self.loading_started = None;
        }
        self.loading = loading;
    }

    /// Seconds since loading began — drives the skeleton shimmer phase.
    pub fn loading_elapsed(&self) -> f32 {
        self.loading_started
            .map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            })
            .unwrap_or(0.0)
    }

    pub fn options(&self) -> &[NeoismAgentPickerOption] {
        &self.filtered_options
    }

    /// Number of list rows actually rendered: the option count, capped at
    /// the visible window. Kept in sync with the renderer's row count so
    /// click hit-testing lands on the same rows the renderer draws.
    fn visible_rows(&self) -> usize {
        self.filtered_options.len().min(PICKER_VISIBLE_ROWS).max(1)
    }

    pub fn selected_option(&self) -> Option<&NeoismAgentPickerOption> {
        self.filtered_options
            .get(self.selected)
            .filter(|option| option.is_selectable())
    }

    pub fn move_selection(&mut self, delta: isize) {
        let count = self.filtered_options.len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let Some(next) = selectable_step(&self.filtered_options, self.selected, delta)
        else {
            return;
        };
        self.set_selected(next);
    }

    fn set_selected(&mut self, next: usize) {
        if next == self.selected || next >= self.filtered_options.len() {
            return;
        }
        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected as i32 - next as i32;
        self.cursor_spring.position += rows as f32 * PICKER_ROW_HEIGHT;
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected = next;
        self.clamp_scroll();
    }

    pub fn set_last_rect(&mut self, rect: [f32; 4]) {
        self.last_rect = Some(rect);
    }

    /// Record the device-pixel height of the footer band drawn last
    /// frame so [`activate_row_at`](Self::activate_row_at) can keep the
    /// footer out of the clickable row grid.
    pub fn set_footer_h(&mut self, footer_h_px: f32) {
        self.footer_h_px = footer_h_px.max(0.0);
    }

    /// Translate a click into a row index and select+return true. The
    /// caller is expected to commit the picker; we just move the cursor.
    /// Header / row ratios mirror `renderer::inline_picker` (`TITLE_H = 30`,
    /// `ROW_H = PICKER_ROW_HEIGHT = 34`); we derive the per-row pixel
    /// height from the cached rect so the live scale factor doesn't need
    /// to be plumbed through.
    pub fn activate_row_at(&mut self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.last_rect else {
            return false;
        };
        if x < rx || x > rx + rw || y < ry || y > ry + rh {
            return false;
        }
        const HEADER_BASE: f32 = 30.0;
        let visible_rows = self.visible_rows();
        // The footer band sits below the row grid; exclude it so the
        // per-row height derivation matches the list area only.
        if y > ry + rh - self.footer_h_px {
            return false;
        }
        let total_h = (rh - self.footer_h_px).max(1.0);
        let header_ratio =
            HEADER_BASE / (HEADER_BASE + PICKER_ROW_HEIGHT * visible_rows as f32);
        let header_h_px = total_h * header_ratio;
        let body_top = ry + header_h_px;
        if y < body_top {
            return false;
        }
        let row_h_px = (total_h - header_h_px) / visible_rows.max(1) as f32;
        if row_h_px <= 0.0 {
            return false;
        }
        let row_within = ((y - body_top) / row_h_px).floor() as usize;
        let target = self.scroll_offset + row_within;
        if target >= self.filtered_options.len() {
            return false;
        }
        if !self.filtered_options[target].is_selectable() {
            return false;
        }
        self.set_selected(target);
        true
    }

    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let Some([rx, ry, rw, rh]) = self.last_rect else {
            return false;
        };
        x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32) -> bool {
        let count = self.filtered_options.len();
        if count <= PICKER_VISIBLE_ROWS || delta_pixels == 0.0 {
            return false;
        }
        // Pixel-precise continuous scroll: move the committed position by
        // the exact gesture delta (positive = scroll toward the top,
        // matching the home-list / timeline sign convention) and let the
        // short spring animate to it. No whole-row quantization dead-zone,
        // so trackpad + wheel track the finger tightly.
        let max_px = self.max_scroll_px();
        let next = (self.scroll_px - delta_pixels).clamp(0.0, max_px);
        if next != self.scroll_px {
            self.set_scroll_px(next);
            self.clamp_selected_to_viewport();
        }
        // Consume the gesture whenever the list can scroll, even at a
        // bound, so it doesn't bubble past the picker overlay.
        true
    }

    /// Advance the list-scroll spring and return the sub-row residual (in
    /// logical px, `-PICKER_ROW_HEIGHT..=0`) the renderer adds to each
    /// row's Y so the list scrolls pixel-smoothly. Also refreshes
    /// `scroll_offset` to the animated top row so the render window and
    /// click hit-testing track the animated position.
    pub fn tick_list_scroll(&mut self) -> f32 {
        // A shrinking option list (re-filter / replace) can leave the
        // committed position past the new bound — pull it back in.
        let max_px = self.max_scroll_px();
        if self.scroll_px > max_px {
            self.scroll_px = max_px;
            self.list_scroll.set_target(max_px);
        }
        let anim = if self.list_scroll.is_animating() {
            let now = Instant::now();
            let dt = now
                .saturating_duration_since(self.last_list_scroll_frame)
                .as_secs_f32()
                .min(0.05);
            self.last_list_scroll_frame = now;
            self.list_scroll.tick(dt);
            self.list_scroll.current().max(0.0)
        } else {
            self.last_list_scroll_frame = Instant::now();
            self.list_scroll.current().max(0.0)
        };
        let render_top = (anim / PICKER_ROW_HEIGHT).floor().max(0.0);
        self.scroll_offset = render_top as usize;
        // `-frac`: the renderer positions row `ix` at
        // `list_y + (ix - scroll_offset) * row_h + residual * s`, which
        // reduces to `list_y + ix * row_h - anim * s`.
        render_top * PICKER_ROW_HEIGHT - anim
    }

    pub fn tick_cursor(&mut self) -> f32 {
        if self.cursor_spring.position == 0.0 {
            self.last_cursor_frame = Instant::now();
            return 0.0;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_cursor_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_cursor_frame = now;
        self.cursor_spring.update(dt, CURSOR_ANIMATION_LENGTH);
        self.cursor_spring.position
    }

    pub fn is_animating(&self) -> bool {
        self.list_scroll.is_animating()
                || self.cursor_spring.position != 0.0
                // The loading skeleton shimmers — keep frames coming.
                || self.loading
    }

    /// Largest committed pixel scroll that still leaves the last row flush
    /// at the bottom of the visible window.
    fn max_scroll_px(&self) -> f32 {
        let count = self.filtered_options.len();
        let visible = count.min(PICKER_VISIBLE_ROWS);
        count.saturating_sub(visible) as f32 * PICKER_ROW_HEIGHT
    }

    /// Move the committed continuous scroll position and drive the spring
    /// toward it. `scroll_offset` (the integer top row) is refreshed from
    /// the *animated* position in `tick_list_scroll`.
    fn set_scroll_px(&mut self, next: f32) {
        let next = next.clamp(0.0, self.max_scroll_px());
        if (next - self.scroll_px).abs() < f32::EPSILON {
            return;
        }
        let was_idle = !self.list_scroll.is_animating();
        self.scroll_px = next;
        self.list_scroll.set_target(next);
        if was_idle {
            self.last_list_scroll_frame = Instant::now();
        }
    }

    fn clamp_scroll(&mut self) {
        let count = self.filtered_options.len();
        if count == 0 {
            self.set_scroll_px(0.0);
            return;
        }
        let visible = count.min(PICKER_VISIBLE_ROWS).max(1) as f32;
        // Keep the selected row inside the visible window, springing the
        // list flush to whichever edge the selection ran past. Held-arrow
        // navigation nudges the position one row at a time, so the list
        // follows the cursor smoothly.
        let sel_top = self.selected as f32 * PICKER_ROW_HEIGHT;
        let sel_bottom = sel_top + PICKER_ROW_HEIGHT;
        let view_top = self.scroll_px;
        let view_bottom = view_top + visible * PICKER_ROW_HEIGHT;
        if sel_top < view_top {
            self.set_scroll_px(sel_top);
        } else if sel_bottom > view_bottom {
            self.set_scroll_px(sel_bottom - visible * PICKER_ROW_HEIGHT);
        }
    }

    fn clamp_selected_to_viewport(&mut self) {
        let count = self.filtered_options.len();
        if count == 0 {
            self.selected = 0;
            return;
        }
        let visible = count.min(PICKER_VISIBLE_ROWS).max(1);
        // Window derived from the committed continuous position so a
        // wheel/trackpad scroll keeps the keyboard selection on a visible
        // row (Enter always commits something in view).
        let first =
            ((self.scroll_px / PICKER_ROW_HEIGHT).round() as usize).min(count - 1);
        let last = (first + visible - 1).min(count - 1);
        let old = self.selected;
        self.selected = selectable_index_between(
            &self.filtered_options,
            self.selected.clamp(first, last),
            first,
            last,
        )
        .or_else(|| selectable_index_near(&self.filtered_options, self.selected))
        .unwrap_or(0);
        if self.selected != old {
            // Kick the cursor spring so the highlight animates from
            // the old row to the new clamped row, rather than snapping
            // (which read as a "jump to top-left" when the highlight
            // was at the top/bottom of the viewport).
            let rows = old as i32 - self.selected as i32;
            let was_idle = self.cursor_spring.position == 0.0;
            self.cursor_spring.position += rows as f32 * PICKER_ROW_HEIGHT;
            if was_idle {
                self.last_cursor_frame = Instant::now();
            }
        }
    }

    pub fn set_pre_filtered_options(
        &mut self,
        query: String,
        options: Vec<NeoismAgentPickerOption>,
    ) {
        self.query = query;
        self.filtered_options = options.clone();
        self.all_options = options;
        self.selected = self
            .selected
            .min(self.filtered_options.len().saturating_sub(1));
        self.selected =
            selectable_index_near(&self.filtered_options, self.selected).unwrap_or(0);
        self.scroll_offset = 0;
        self.scroll_px = 0.0;
        self.list_scroll.reset();
        self.cursor_spring.reset();
    }

    pub fn replace_options(&mut self, options: Vec<NeoismAgentPickerOption>) {
        let previous_value = self
            .filtered_options
            .get(self.selected)
            .filter(|option| option.is_selectable())
            .map(|option| option.value.clone());
        self.all_options = options;
        // Re-filter the NEW options against the current query directly.
        // `set_query()` short-circuits when the query is unchanged (a
        // keystroke-perf guard), so the old clear()+set_query() round-trip
        // silently failed to rebuild `filtered_options` whenever the query
        // was empty — leaving the picker showing its stale (e.g. "Loading
        // sessions…") rows after the real catalog arrived.
        self.rebuild_filtered_options();
        if let Some(value) = previous_value {
            if let Some(index) = self
                .filtered_options
                .iter()
                .position(|option| option.value == value)
            {
                self.selected = index;
            }
        }
        self.selected =
            selectable_index_near(&self.filtered_options, self.selected).unwrap_or(0);
    }

    /// Rebuild `filtered_options` from `all_options` for the current
    /// `query`. The single source of truth for the picker filter, called
    /// by both `set_query` (after its idempotency guard) and
    /// `replace_options` (which must rebuild even when the query is
    /// unchanged).
    fn rebuild_filtered_options(&mut self) {
        let needle = self.query.trim().to_lowercase();
        if needle.is_empty() {
            self.filtered_options = self.all_options.clone();
            self.selected =
                selectable_index_near(&self.filtered_options, self.selected).unwrap_or(0);
            return;
        }
        let words = needle.split_whitespace().collect::<Vec<_>>();
        let mut output = Vec::new();
        let mut pending_header: Option<NeoismAgentPickerOption> = None;
        let mut pending_header_matches = false;
        let mut emitted_header = false;
        for option in &self.all_options {
            if option.is_header {
                pending_header_matches = option_matches(option, &words);
                pending_header = Some(option.clone());
                emitted_header = false;
                continue;
            }
            let matches = option_matches(option, &words) || pending_header_matches;
            if !matches {
                continue;
            }
            if let Some(header) = pending_header.as_ref() {
                if !emitted_header {
                    output.push(header.clone());
                    emitted_header = true;
                }
            }
            output.push(option.clone());
        }
        self.filtered_options = output;
        self.selected =
            selectable_index_near(&self.filtered_options, self.selected).unwrap_or(0);
    }

    pub fn set_query(&mut self, query: String) {
        // Idempotent guard — same query firing every keystroke as the
        // user navigates the picker would otherwise reset scroll +
        // cursor spring on every frame, snapping the highlight back to
        // the top-left at boundaries.
        if self.query == query {
            return;
        }
        self.query = query;
        let previous_value = self
            .filtered_options
            .get(self.selected)
            .filter(|option| option.is_selectable())
            .map(|option| option.value.clone());
        self.rebuild_filtered_options();
        // Keep the cursor on the same option across filter changes
        // when it survives the filter, so the trail-cursor doesn't
        // animate back to row 0 every time the user types another
        // letter while their selection is mid-list.
        let new_selected = previous_value
            .and_then(|value| {
                self.filtered_options
                    .iter()
                    .position(|option| option.value == value)
            })
            .and_then(|index| selectable_index_near(&self.filtered_options, index))
            .unwrap_or_else(|| {
                selectable_index_near(&self.filtered_options, 0).unwrap_or(0)
            });
        let selection_changed = new_selected != self.selected;
        self.selected = new_selected.min(self.filtered_options.len().saturating_sub(1));
        self.scroll_offset = 0;
        self.scroll_px = 0.0;
        // Content changed under the list — jump the scroll back to the top
        // (no smooth animation for a fresh filter). The cursor trail only
        // resets when the selection actually moved, so typing another
        // letter that keeps the selection doesn't snap the highlight back.
        self.list_scroll.reset();
        if selection_changed {
            self.cursor_spring.reset();
        }
    }
}

fn selectable_index_near(
    options: &[NeoismAgentPickerOption],
    index: usize,
) -> Option<usize> {
    if options.is_empty() {
        return None;
    }
    let index = index.min(options.len().saturating_sub(1));
    if options
        .get(index)
        .is_some_and(NeoismAgentPickerOption::is_selectable)
    {
        return Some(index);
    }
    let last = options.len().saturating_sub(1);
    for offset in 0..=last {
        let forward = index.saturating_add(offset);
        if forward <= last && options[forward].is_selectable() {
            return Some(forward);
        }
        if let Some(backward) = index.checked_sub(offset) {
            if options[backward].is_selectable() {
                return Some(backward);
            }
        }
    }
    None
}

fn selectable_index_between(
    options: &[NeoismAgentPickerOption],
    index: usize,
    first: usize,
    last: usize,
) -> Option<usize> {
    if options.is_empty() || first > last {
        return None;
    }
    let index = index.min(options.len().saturating_sub(1));
    if options
        .get(index)
        .is_some_and(NeoismAgentPickerOption::is_selectable)
    {
        return Some(index);
    }
    for offset in 0..=last.saturating_sub(first) {
        let forward = index.saturating_add(offset);
        if forward <= last
            && options
                .get(forward)
                .is_some_and(NeoismAgentPickerOption::is_selectable)
        {
            return Some(forward);
        }
        if let Some(backward) = index.checked_sub(offset) {
            if backward >= first
                && options
                    .get(backward)
                    .is_some_and(NeoismAgentPickerOption::is_selectable)
            {
                return Some(backward);
            }
        }
    }
    None
}

fn selectable_step(
    options: &[NeoismAgentPickerOption],
    selected: usize,
    delta: isize,
) -> Option<usize> {
    if options.is_empty() || delta == 0 {
        return selectable_index_near(options, selected);
    }
    let mut index = selected.min(options.len().saturating_sub(1));
    let mut remaining = delta.unsigned_abs().max(1);
    while remaining > 0 {
        let mut next = None;
        if delta > 0 {
            for candidate in index.saturating_add(1)..options.len() {
                if options[candidate].is_selectable() {
                    next = Some(candidate);
                    break;
                }
            }
        } else {
            for candidate in (0..index).rev() {
                if options[candidate].is_selectable() {
                    next = Some(candidate);
                    break;
                }
            }
        }
        index = next?;
        remaining -= 1;
    }
    Some(index)
}

fn option_matches(option: &NeoismAgentPickerOption, words: &[&str]) -> bool {
    let mut haystack = String::with_capacity(
        option.title.len()
            + option.description.len()
            + option.footer.len()
            + option.value.len()
            + option.section.len()
            + 4,
    );
    haystack.push_str(&option.title);
    haystack.push(' ');
    haystack.push_str(&option.description);
    haystack.push(' ');
    haystack.push_str(&option.footer);
    haystack.push(' ');
    haystack.push_str(&option.value);
    haystack.push(' ');
    haystack.push_str(&option.section);
    haystack.make_ascii_lowercase();
    words.iter().all(|word| haystack.contains(word))
}
