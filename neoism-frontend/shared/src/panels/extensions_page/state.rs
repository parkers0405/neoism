use sugarloaf::Sugarloaf;

use crate::event::{
    KeyDescriptor, KeyState, LogicalKey, Modifiers, NamedKey, PointerButton,
};
use crate::primitives::ide_theme::IdeTheme;

/// Outcome of feeding a key to the panel. `consumed` tells the host
/// bridge whether to stop the key here (so it doesn't leak to global
/// shortcuts / the terminal); `action` carries any side-effecting
/// intent (install/uninstall) the host must run.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct KeyResponse {
    pub consumed: bool,
    pub action: Option<PaneAction>,
}

impl KeyResponse {
    fn handled() -> Self {
        Self {
            consumed: true,
            action: None,
        }
    }

    fn with_action(action: PaneAction) -> Self {
        Self {
            consumed: true,
            action: Some(action),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionFilter {
    All,
    Installed,
    NotInstalled,
}

impl Default for ExtensionFilter {
    fn default() -> Self {
        Self::All
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExtensionStatus {
    NotInstalled,
    /// The integration ships with Neoism and has no package lifecycle. It is
    /// shown in the catalog, but clicking/pressing Enter must never dispatch
    /// an install or uninstall job.
    BuiltIn,
    /// The binary exists on this machine but is NOT managed by Neoism's
    /// installer (found on `$PATH` or via explicit config). Uninstalling it
    /// is not ours to offer, so the row is informational only.
    Detected,
    /// No binary anywhere and no managed installer can supply one. The row
    /// stays visible (honesty about the gap) but has no lifecycle actions.
    Unavailable,
    /// `None` means the current phase has no trustworthy denominator (DNS,
    /// package-manager work, extraction, linking). The view renders an
    /// animated indeterminate bar instead of lying with a frozen `0%`.
    Installing {
        percent: Option<u8>,
        status_text: String,
    },
    Installed {
        version: String,
    },
    Uninstalling,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtensionTab {
    McpServers,
    LanguageServers,
    TreeSitterParsers,
    Kernels,
}

impl Default for ExtensionTab {
    fn default() -> Self {
        Self::McpServers
    }
}

#[derive(Debug, Clone)]
pub struct ExtensionEntry {
    pub id: String,
    pub name: String,
    pub version: String,
    pub description: String,
    pub author: String,
    pub downloads: Option<u64>,
    pub categories: Vec<String>,
    /// Languages this extension targets (language-server rows carry the
    /// languages their engine adapter routes, e.g. `["Rust"]`). Used by
    /// the card chips and search index. Empty for entries without a
    /// language association (e.g. most MCP servers).
    pub languages: Vec<String>,
    pub status: ExtensionStatus,
    pub repository_url: Option<String>,
    /// For language-server rows: the adapter's live runtime state as the
    /// Neoism LSP engine reports it — `"connected"` when a client is
    /// attached, otherwise where the binary/endpoint resolves:
    /// `"built-in/socket"`, `"extension"` (managed install), `"path"`
    /// (found on `$PATH`), `"config"`, or `"missing"`.
    /// `None` for non-LSP entries.
    /// Drives the source badge so the page reflects what the engine will
    /// actually run.
    pub lsp_source: Option<String>,
}

#[derive(Debug, Clone)]
pub(crate) enum RowAction {
    ToggleInstall(String),
    Focus(usize),
}

/// Action surfaced by the panel back to the host bridge after an
/// interaction has been handled. Bridges decide what to do with
/// it (spawn an install task, route a "Reveal" command, etc.). The
/// panel itself only mutates its own state and returns the intent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PaneAction {
    InstallToggleRequested {
        id: String,
        currently_installed: bool,
    },
    OpenRepository(String),
}

#[derive(Debug, Clone)]
pub(crate) struct RowHit {
    pub rect: [f32; 4],
    pub action: RowAction,
}

#[derive(Debug, Clone)]
pub struct NeoismExtensionsPane {
    pub(crate) entries: Vec<ExtensionEntry>,
    pub(crate) search_query: String,
    pub(crate) filter: ExtensionFilter,
    pub(crate) active_tab: ExtensionTab,
    /// `None` = "All languages" (the default). `Some(lang)` filters
    /// `visible_entries` to entries whose `languages` field contains
    /// the chosen language (case-insensitive). Only meaningful on the
    /// LSP / Formatter / Linter tabs — the MCP tab ignores it.
    pub(crate) selected_language: Option<String>,
    pub(crate) selected_index: usize,
    /// Currently-rendered scroll position. Lerps toward
    /// `target_scroll_top` each frame for the same smooth feel the
    /// markdown pane has.
    pub(crate) scroll_top: f32,
    /// Where scrolling wants to end up. Wheel / keyboard nudges this;
    /// `tick_scroll` decays the gap toward zero.
    pub(crate) target_scroll_top: f32,
    /// Inertial velocity for accelerated wheel scroll — exponentially
    /// decayed each tick. Mirror of the markdown pane's approach.
    pub(crate) scroll_velocity_px_s: f32,
    pub(crate) scroll_last_tick_at: Option<web_time::Instant>,
    pub(crate) content_height: f32,
    pub(crate) row_hits: Vec<RowHit>,
    pub(crate) search_input_rect: [f32; 4],
    pub(crate) filter_pill_rects: [[f32; 4]; 3],
    pub(crate) tab_pill_rects: Vec<([f32; 4], ExtensionTab)>,
    /// Hit-rect for the language selector trigger (the small pill that
    /// opens the language dropdown). Recomputed each render.
    pub(crate) language_trigger_rect: [f32; 4],
    /// Hit-rects for each row in the language dropdown when open. Each
    /// entry pairs a rect with the language string (or empty string for
    /// the "All languages" reset row).
    pub(crate) language_option_rects: Vec<([f32; 4], String)>,
    pub(crate) language_picker_open: bool,
    /// Live filter text typed into the dropdown's search input. Empty
    /// string means "show everything".
    pub(crate) language_search_query: String,
    /// Whether the dropdown's search input owns text focus. Auto-true
    /// on open so the user can immediately type.
    pub(crate) language_search_focused: bool,
    /// Hit-rect for the dropdown's search input (so click-to-focus +
    /// click-out-to-blur both work).
    pub(crate) language_search_rect: [f32; 4],
    /// Hit-rect for the whole dropdown panel (used to suppress
    /// outside-click dismissal when the user clicks inside the panel
    /// but not on any specific option).
    pub(crate) language_panel_rect: [f32; 4],
    /// Vertical scroll offset (logical px) for the dropdown's option
    /// list — moves the long list under the fixed search/header area
    /// when there are more languages than fit in the viewport.
    pub(crate) language_scroll_top: f32,
    pub(crate) focused_search: bool,
    pub(crate) viewport_height: f32,
    /// Per-row vertical advance (card height + gap) in the view's
    /// *scaled* drawing space. The view recomputes cards at
    /// `(CARD_HEIGHT + CARD_GAP) * scale`, and `scroll_top` lives in the
    /// same space, so keyboard scroll-follow must use this scaled value
    /// — not the bare constants — to place the selected row correctly.
    /// Refreshed every render by `draw_card_list`.
    pub(crate) list_row_advance: f32,
}

impl NeoismExtensionsPane {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            search_query: String::new(),
            filter: ExtensionFilter::default(),
            active_tab: ExtensionTab::default(),
            selected_language: None,
            selected_index: 0,
            scroll_top: 0.0,
            target_scroll_top: 0.0,
            scroll_velocity_px_s: 0.0,
            scroll_last_tick_at: None,
            content_height: 0.0,
            row_hits: Vec::new(),
            search_input_rect: [0.0; 4],
            filter_pill_rects: [[0.0; 4]; 3],
            tab_pill_rects: Vec::new(),
            language_trigger_rect: [0.0; 4],
            language_option_rects: Vec::new(),
            language_picker_open: false,
            language_search_query: String::new(),
            language_search_focused: false,
            language_search_rect: [0.0; 4],
            language_panel_rect: [0.0; 4],
            language_scroll_top: 0.0,
            focused_search: false,
            viewport_height: 0.0,
            list_row_advance: view::CARD_HEIGHT + view::CARD_GAP,
        }
    }

    /// Move keyboard focus to the page's main search box. Called by the
    /// host when the page opens (auto-focus, Cmd+P style) and when the
    /// user presses `/` or Cmd/Ctrl+F to jump back to it.
    pub fn focus_search(&mut self) {
        self.focused_search = true;
        // The language dropdown's own search is a separate target; if it
        // was open, retract it so focus is unambiguous.
        self.close_language_picker();
    }

    /// Record the view's scaled per-row advance so keyboard scroll-
    /// follow lands the selected card in the same coordinate space the
    /// renderer draws in. Called once per render from `draw_card_list`.
    pub fn set_list_row_advance(&mut self, row_advance: f32) {
        if row_advance.is_finite() && row_advance > 0.0 {
            self.list_row_advance = row_advance;
        }
    }

    /// Cycle the active category tab (MCP -> Language -> Syntax ->
    /// Kernels, wrapping). `forward` advances; otherwise steps back.
    /// Resets selection + scroll so the new tab starts at the top.
    pub fn cycle_tab(&mut self, forward: bool) {
        const ORDER: [ExtensionTab; 4] = [
            ExtensionTab::McpServers,
            ExtensionTab::LanguageServers,
            ExtensionTab::TreeSitterParsers,
            ExtensionTab::Kernels,
        ];
        let cur = ORDER
            .iter()
            .position(|tab| *tab == self.active_tab)
            .unwrap_or(0);
        let n = ORDER.len();
        let next = if forward {
            (cur + 1) % n
        } else {
            (cur + n - 1) % n
        };
        self.set_active_tab(ORDER[next]);
        self.selected_index = 0;
        self.scroll_top = 0.0;
        self.target_scroll_top = 0.0;
        self.scroll_velocity_px_s = 0.0;
    }

    /// Sorted, deduplicated list of language tags across all entries.
    /// Hosts pass this into the dropdown render. Cheap to recompute
    /// (entry count caps at ~1500) so we don't memoise it.
    pub fn known_languages(&self) -> Vec<String> {
        let mut seen: std::collections::BTreeSet<String> =
            std::collections::BTreeSet::new();
        for entry in &self.entries {
            for lang in &entry.languages {
                let trimmed = lang.trim();
                if !trimmed.is_empty() {
                    seen.insert(trimmed.to_string());
                }
            }
        }
        seen.into_iter().collect()
    }

    pub fn selected_language(&self) -> Option<&str> {
        self.selected_language.as_deref()
    }

    pub fn language_picker_open(&self) -> bool {
        self.language_picker_open
    }

    /// Whether the active tab cares about the language filter. MCP
    /// servers don't carry language tags, so the dropdown is hidden
    /// on that tab.
    pub fn tab_supports_language_filter(&self) -> bool {
        !matches!(
            self.active_tab,
            ExtensionTab::McpServers | ExtensionTab::Kernels
        )
    }

    pub fn set_entries(&mut self, entries: Vec<ExtensionEntry>) {
        self.entries = entries;
        if self.selected_index >= self.entries.len() {
            self.selected_index = 0;
        }
    }

    /// Wheel input. `delta_pixels` is positive when scrolling DOWN
    /// (content moves up). When the language dropdown is open the
    /// wheel scrolls the dropdown's option list (capped between 0 and
    /// `max`, which the host computes from the rendered viewport);
    /// otherwise it scrolls the card list. Mirrors
    /// `MarkdownPane::scroll_pixels` → `scroll_by_content_pixels`:
    /// bumps the target, zeroes velocity, and lets `tick_scroll` lerp
    /// `scroll_top` toward it each frame.
    pub fn scroll_pixels(&mut self, delta_pixels: f32) {
        if self.language_picker_open {
            // Caller hasn't passed the max yet — clamp lazily inside the
            // dropdown render once it knows row count + viewport. Here
            // we just bump the target; renderer will re-clamp.
            self.language_scroll_top = (self.language_scroll_top + delta_pixels).max(0.0);
            return;
        }
        self.scroll_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
        let max_scroll = self.max_scroll();
        self.target_scroll_top =
            (self.target_scroll_top + delta_pixels).clamp(0.0, max_scroll);
    }

    /// Re-clamp `language_scroll_top` after the dropdown viewport size
    /// is known. View calls this once per render after measuring the
    /// option-list region.
    pub fn clamp_language_scroll(&mut self, max_scroll: f32) {
        self.language_scroll_top =
            self.language_scroll_top.clamp(0.0, max_scroll.max(0.0));
    }

    /// Update the viewport height (drives `max_scroll`) and re-clamp
    /// current positions. View code calls this each layout pass.
    pub fn set_viewport_height(&mut self, viewport_height: f32) {
        self.viewport_height = viewport_height.max(0.0);
        let max_scroll = self.max_scroll();
        self.scroll_top = self.scroll_top.clamp(0.0, max_scroll);
        self.target_scroll_top = self.target_scroll_top.clamp(0.0, max_scroll);
    }

    /// Update the rendered content height (drives `max_scroll`) and
    /// re-clamp positions. Should be called once per layout pass after
    /// the card list has been measured.
    pub fn set_content_height(&mut self, content_height: f32) {
        self.content_height = content_height.max(0.0);
        let max_scroll = self.max_scroll();
        self.scroll_top = self.scroll_top.clamp(0.0, max_scroll);
        self.target_scroll_top = self.target_scroll_top.clamp(0.0, max_scroll);
    }

    /// Advance one frame of smooth scroll. Returns true when the pane
    /// still has motion left so the host can keep redrawing. Mirrors
    /// `MarkdownPane::tick_scroll` exactly — same lerp factor, same
    /// epsilon, same inertial decay path.
    pub fn tick_scroll(&mut self) -> bool {
        // Match markdown's constants (see editor/markdown/types.rs).
        const SCROLL_SETTLE_FACTOR: f32 = 0.24;
        const SCROLL_EPSILON: f32 = 0.35;

        let inertial = self.tick_inertial_scroll();
        let delta = self.target_scroll_top - self.scroll_top;
        if delta.abs() <= SCROLL_EPSILON {
            if self.scroll_top != self.target_scroll_top {
                self.scroll_top = self.target_scroll_top;
                return true;
            }
            return inertial;
        }
        self.scroll_top += delta * SCROLL_SETTLE_FACTOR;
        true
    }

    fn tick_inertial_scroll(&mut self) -> bool {
        if self.scroll_velocity_px_s.abs() < 4.0 {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_last_tick_at = None;
            return false;
        }
        let max_scroll = self.max_scroll();
        if max_scroll <= 0.0 {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_last_tick_at = None;
            return false;
        }
        let now = web_time::Instant::now();
        let dt = self
            .scroll_last_tick_at
            .map(|last| now.saturating_duration_since(last).as_secs_f32().min(0.05))
            .unwrap_or(0.016);
        self.scroll_last_tick_at = Some(now);
        // 0.28s time constant — same as markdown pane.
        self.scroll_velocity_px_s *= (-dt / 0.28).exp();
        let step = self.scroll_velocity_px_s * dt;
        let before = self.target_scroll_top;
        self.target_scroll_top = (self.target_scroll_top + step).clamp(0.0, max_scroll);
        let applied = self.target_scroll_top - before;
        if applied.abs() < f32::EPSILON {
            self.scroll_velocity_px_s = 0.0;
            self.scroll_last_tick_at = None;
            return false;
        }
        true
    }

    fn max_scroll(&self) -> f32 {
        (self.content_height - self.viewport_height).max(0.0)
    }

    pub fn scroll_top(&self) -> f32 {
        self.scroll_top
    }

    pub fn entries(&self) -> &[ExtensionEntry] {
        &self.entries
    }

    pub fn entries_mut(&mut self) -> &mut [ExtensionEntry] {
        &mut self.entries
    }

    pub fn set_search_query(&mut self, query: String) {
        self.search_query = query;
    }

    pub fn set_filter(&mut self, filter: ExtensionFilter) {
        self.filter = filter;
    }

    pub fn set_active_tab(&mut self, tab: ExtensionTab) {
        self.active_tab = tab;
        // Tabs that don't carry language tags ignore the filter;
        // close the picker so it doesn't paint over the wrong tab.
        if matches!(tab, ExtensionTab::McpServers | ExtensionTab::Kernels) {
            self.language_picker_open = false;
        }
    }

    pub fn set_selected_language(&mut self, language: Option<String>) {
        self.selected_language = language;
        self.language_picker_open = false;
        self.selected_index = 0;
    }

    pub fn toggle_language_picker(&mut self) {
        if self.language_picker_open {
            self.close_language_picker();
        } else {
            self.open_language_picker();
        }
    }

    pub fn open_language_picker(&mut self) {
        self.language_picker_open = true;
        // Auto-focus the search box on open so the user can immediately
        // type to narrow the list. Reset scroll + query so reopening
        // doesn't show last-session state.
        self.language_search_focused = true;
        self.language_search_query.clear();
        self.language_scroll_top = 0.0;
    }

    pub fn close_language_picker(&mut self) {
        self.language_picker_open = false;
        self.language_search_focused = false;
    }

    /// Languages that pass the dropdown's search filter. Substring
    /// match (case-insensitive); empty query returns everything.
    /// Always preceded by the `""` "All languages" sentinel.
    pub fn filtered_language_options(&self) -> Vec<String> {
        let query = self.language_search_query.trim().to_lowercase();
        let mut out: Vec<String> = Vec::new();
        out.push(String::new());
        for lang in self.known_languages() {
            if query.is_empty() || lang.to_lowercase().contains(&query) {
                out.push(lang);
            }
        }
        out
    }

    /// Whether the language dropdown owns text focus (input box +
    /// keyboard wheel for scrolling). The bridge consults this before
    /// dispatching key events to the rest of the panel.
    pub fn language_search_focused(&self) -> bool {
        self.language_search_focused
    }

    pub fn set_status(&mut self, id: &str, status: ExtensionStatus) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.id == id) {
            entry.status = status;
        }
    }

    /// Indices of entries that pass the current status filter, tab
    /// filter, and fuzzy search query. Order is preserved from
    /// `entries` for filter/tab passes; the search pass re-orders by
    /// match quality (name-prefix > id-prefix > body-substring).
    pub(super) fn visible_entries(&self) -> Vec<usize> {
        let query = self.search_query.trim().to_lowercase();
        let has_query = !query.is_empty();

        // First pass: status + tab filter + language filter.
        let mut base: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.passes_status(entry))
            .filter(|(_, entry)| self.passes_tab(entry))
            .filter(|(_, entry)| self.passes_language(entry))
            .map(|(i, _)| i)
            .collect();

        if !has_query {
            return base;
        }

        // Second pass: fuzzy (substring) search with a tiered score.
        // 0 = name prefix, 1 = id prefix, 2 = body substring, 3 = no match.
        let mut scored: Vec<(usize, u8)> = Vec::new();
        for idx in base.drain(..) {
            let entry = &self.entries[idx];
            let name_l = entry.name.to_lowercase();
            let id_l = entry.id.to_lowercase();
            // Include language tags in the searchable body so e.g.
            // typing "python" surfaces every LSP/grammar/snippet tagged
            // with Python even when the entry name doesn't mention it.
            let langs = entry
                .languages
                .iter()
                .map(|s| s.to_lowercase())
                .collect::<Vec<_>>()
                .join(" ");
            let body = format!(
                "{} {} {} {}",
                id_l,
                name_l,
                entry.description.to_lowercase(),
                langs,
            );
            let tier = if name_l.starts_with(&query) {
                0
            } else if id_l.starts_with(&query) {
                1
            } else if body.contains(&query) {
                2
            } else {
                3
            };
            if tier < 3 {
                scored.push((idx, tier));
            }
        }
        scored.sort_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)));
        scored.into_iter().map(|(idx, _)| idx).collect()
    }

    fn passes_status(&self, entry: &ExtensionEntry) -> bool {
        match self.filter {
            ExtensionFilter::All => true,
            ExtensionFilter::Installed => matches!(
                entry.status,
                ExtensionStatus::BuiltIn
                    | ExtensionStatus::Detected
                    | ExtensionStatus::Installed { .. }
                    | ExtensionStatus::Uninstalling
            ),
            ExtensionFilter::NotInstalled => matches!(
                entry.status,
                ExtensionStatus::NotInstalled
                    | ExtensionStatus::Unavailable
                    | ExtensionStatus::Installing { .. }
            ),
        }
    }

    fn passes_language(&self, entry: &ExtensionEntry) -> bool {
        let Some(target) = self.selected_language.as_deref() else {
            return true;
        };
        if !self.tab_supports_language_filter() {
            return true;
        }
        entry
            .languages
            .iter()
            .any(|lang| lang.eq_ignore_ascii_case(target))
    }

    fn passes_tab(&self, entry: &ExtensionEntry) -> bool {
        entry
            .categories
            .iter()
            .any(|cat| matches_tab(cat, self.active_tab))
    }

    /// Whether the search input is the active text target. Useful
    /// for hosts that want to suppress chrome-level shortcuts while
    /// the panel is consuming printable input.
    pub fn search_focused(&self) -> bool {
        self.focused_search
    }

    /// Dispatch a pointer-down event against the cached hit rects.
    /// Only left-button clicks fire interaction; everything else is
    /// a no-op.
    pub fn on_pointer_down(
        &mut self,
        x: f32,
        y: f32,
        button: PointerButton,
    ) -> Option<PaneAction> {
        if !matches!(button, PointerButton::Left) {
            return None;
        }

        // Search input focus. Use the same path as the keyboard `/` /
        // Cmd+F shortcut so a click moves the cursor to the search box
        // (showing the caret) and dismisses any open language dropdown,
        // keeping focus unambiguous.
        if point_in_rect(x, y, self.search_input_rect) {
            self.focus_search();
            return None;
        }

        // Filter pills (All / Installed / Not Installed).
        for (i, rect) in self.filter_pill_rects.iter().enumerate() {
            if point_in_rect(x, y, *rect) {
                self.filter = match i {
                    0 => ExtensionFilter::All,
                    1 => ExtensionFilter::Installed,
                    _ => ExtensionFilter::NotInstalled,
                };
                self.focused_search = false;
                self.selected_index = 0;
                return None;
            }
        }

        // Language trigger pill — clicking it always toggles. Checked
        // BEFORE the panel-options block so a click on the trigger
        // while the panel is open retracts cleanly (the previous order
        // closed-then-reopened in the same dispatch).
        if self.tab_supports_language_filter()
            && point_in_rect(x, y, self.language_trigger_rect)
        {
            self.toggle_language_picker();
            self.focused_search = false;
            return None;
        }

        // Language dropdown contents — handled only when the picker is
        // actually open. Any click inside the panel rect is consumed
        // (focus search, pick an option, or just absorb the click);
        // clicks outside the panel close the picker AND fall through
        // so the underlying chrome still reacts.
        if self.language_picker_open {
            // Search input: focus it; consume the click.
            if point_in_rect(x, y, self.language_search_rect) {
                self.language_search_focused = true;
                self.focused_search = false;
                return None;
            }
            // Option rows: select + close.
            for (rect, lang) in &self.language_option_rects {
                if point_in_rect(x, y, *rect) {
                    self.selected_language = if lang.is_empty() {
                        None
                    } else {
                        Some(lang.clone())
                    };
                    self.close_language_picker();
                    self.focused_search = false;
                    self.selected_index = 0;
                    return None;
                }
            }
            // Click inside the panel rect but not on a row/input — just
            // absorb (don't dismiss; user might be aiming for a tight
            // option below the visible viewport).
            if point_in_rect(x, y, self.language_panel_rect) {
                return None;
            }
            // Click outside the panel: dismiss + let dispatch continue.
            self.close_language_picker();
        }

        // Tab pills.
        for (rect, tab) in &self.tab_pill_rects {
            if point_in_rect(x, y, *rect) {
                let was = self.active_tab;
                self.active_tab = *tab;
                if was != *tab
                    && matches!(*tab, ExtensionTab::McpServers | ExtensionTab::Kernels)
                {
                    // These tabs ignore language filter — reset so the
                    // user doesn't see "no results" because of a stale
                    // pick that doesn't apply here.
                    self.language_picker_open = false;
                }
                self.focused_search = false;
                self.selected_index = 0;
                return None;
            }
        }

        // Row hits. Iterate in REVERSE so the install button rect
        // (pushed after the row focus rect by view::draw_card_body)
        // gets matched first.
        let mut focus_target: Option<usize> = None;
        let mut toggle_target: Option<String> = None;
        for hit in self.row_hits.iter().rev() {
            if !point_in_rect(x, y, hit.rect) {
                continue;
            }
            match &hit.action {
                RowAction::ToggleInstall(id) => {
                    toggle_target = Some(id.clone());
                    break;
                }
                RowAction::Focus(idx) => {
                    focus_target = Some(*idx);
                    break;
                }
            }
        }
        if let Some(id) = toggle_target {
            self.focused_search = false;
            let currently_installed = self
                .entries
                .iter()
                .find(|e| e.id == id)
                .map(|e| matches!(e.status, ExtensionStatus::Installed { .. }))
                .unwrap_or(false);
            return Some(PaneAction::InstallToggleRequested {
                id,
                currently_installed,
            });
        }
        if let Some(idx) = focus_target {
            self.focused_search = false;
            self.selected_index = idx;
            return None;
        }

        None
    }

    /// Keyboard handler. Returns a [`KeyResponse`] describing whether
    /// the key was consumed and any side-effecting intent (Enter on the
    /// selected card) the host must run.
    pub fn on_key(&mut self, key: &KeyDescriptor) -> KeyResponse {
        if key.state != KeyState::Pressed {
            return KeyResponse::default();
        }

        let alt = key.modifiers.contains(Modifiers::ALT);
        let ctrl = key.modifiers.contains(Modifiers::CTRL);
        let meta = key.modifiers.contains(Modifiers::META);

        // Alt+Left / Alt+Right cycle the category tabs, mirroring the
        // Alt+arrow focus movement on the chrome tab strip. Handled
        // before every other branch so it works whether the search box
        // or the list currently holds focus.
        if alt && !ctrl && !meta {
            if let LogicalKey::Named(named) = &key.logical {
                match named {
                    NamedKey::ArrowLeft => {
                        self.cycle_tab(false);
                        return KeyResponse::handled();
                    }
                    NamedKey::ArrowRight => {
                        self.cycle_tab(true);
                        return KeyResponse::handled();
                    }
                    _ => {}
                }
            }
        }

        // Cmd/Ctrl+F jumps the cursor to the search box from anywhere.
        if (ctrl || meta) && !alt {
            if let LogicalKey::Character(ch) = &key.logical {
                if ch.eq_ignore_ascii_case("f") {
                    self.focus_search();
                    return KeyResponse::handled();
                }
            }
        }

        // `/` focuses the search box while navigating the list. When a
        // search field already owns focus `/` is a literal character
        // (handled by `on_text`), so only intercept it otherwise.
        let typing_in_search = self.focused_search
            || (self.language_picker_open && self.language_search_focused);
        if !typing_in_search {
            if let LogicalKey::Character(ch) = &key.logical {
                if ch.as_str() == "/" {
                    self.focus_search();
                    return KeyResponse::handled();
                }
            }
        }

        let LogicalKey::Named(named) = &key.logical else {
            return KeyResponse::default();
        };

        // Language dropdown owns the keyboard while open. We hijack
        // Backspace / Escape / Enter before the panel's own search
        // handler so the user can edit / dismiss the dropdown without
        // affecting the extension search underneath.
        if self.language_picker_open {
            match named {
                NamedKey::Backspace if self.language_search_focused => {
                    self.language_search_query.pop();
                    self.language_scroll_top = 0.0;
                    return KeyResponse::handled();
                }
                NamedKey::Escape => {
                    self.close_language_picker();
                    return KeyResponse::handled();
                }
                NamedKey::Enter => {
                    // Commit the first match; if none, just close.
                    let mut options = self.filtered_language_options();
                    options.remove(0); // drop "All" sentinel
                    self.selected_language = options.into_iter().next();
                    self.close_language_picker();
                    self.selected_index = 0;
                    return KeyResponse::handled();
                }
                _ => {}
            }
        }

        // Search-focused branch first; Enter falls through to the
        // shared selection-trigger path below.
        if self.focused_search {
            match named {
                NamedKey::Backspace => {
                    self.search_query.pop();
                    return KeyResponse::handled();
                }
                NamedKey::Escape => {
                    self.focused_search = false;
                    self.search_query.clear();
                    return KeyResponse::handled();
                }
                NamedKey::ArrowDown | NamedKey::ArrowUp => {
                    // Drop focus into the list and land on the top
                    // (best) match — Cmd+P feel: type, then arrow down.
                    self.focused_search = false;
                    if !self.visible_entries().is_empty() {
                        self.selected_index = 0;
                        self.ensure_selected_visible(self.viewport_height);
                    }
                    return KeyResponse::handled();
                }
                NamedKey::Enter => {
                    self.focused_search = false;
                    // fall through to shared Enter handler
                }
                _ => return KeyResponse::default(),
            }
        }

        let visible = self.visible_entries();
        let last = visible.len().saturating_sub(1);
        match named {
            NamedKey::ArrowUp => {
                if !visible.is_empty() {
                    self.selected_index = self.selected_index.saturating_sub(1).min(last);
                    self.ensure_selected_visible(self.viewport_height);
                }
                KeyResponse::handled()
            }
            NamedKey::ArrowDown => {
                if !visible.is_empty() {
                    self.selected_index = self.selected_index.saturating_add(1).min(last);
                    self.ensure_selected_visible(self.viewport_height);
                }
                KeyResponse::handled()
            }
            NamedKey::Enter => {
                if visible.is_empty() {
                    return KeyResponse::handled();
                }
                let clamped = self.selected_index.min(last);
                let entry_idx = visible[clamped];
                let entry = &self.entries[entry_idx];
                if matches!(
                    entry.status,
                    ExtensionStatus::BuiltIn
                        | ExtensionStatus::Detected
                        | ExtensionStatus::Unavailable
                ) {
                    return KeyResponse::handled();
                }
                KeyResponse::with_action(PaneAction::InstallToggleRequested {
                    id: entry.id.clone(),
                    currently_installed: matches!(
                        entry.status,
                        ExtensionStatus::Installed { .. }
                    ),
                })
            }
            _ => KeyResponse::default(),
        }
    }

    /// Printable-text input. Returns true when the character was
    /// consumed. Two consumers: the dropdown's language-search input
    /// (when open + focused) takes precedence over the panel's main
    /// search box so typing inside the dropdown doesn't leak into the
    /// extension search underneath.
    pub fn on_text(&mut self, text: &str) -> bool {
        if text.is_empty() || !text.chars().all(|c| !c.is_control()) {
            return false;
        }
        if self.language_picker_open && self.language_search_focused {
            self.language_search_query.push_str(text);
            self.language_scroll_top = 0.0;
            return true;
        }
        if self.focused_search {
            self.search_query.push_str(text);
            return true;
        }
        false
    }

    /// Adjust `scroll_top` so the row at `selected_index` (looked up
    /// through `visible_entries`) is on-screen for the given list
    /// viewport height. Caller usually passes the cached
    /// `viewport_height` from the most recent render.
    pub fn ensure_selected_visible(&mut self, list_height: f32) {
        let visible = self.visible_entries();
        if visible.is_empty() || list_height <= 0.0 {
            return;
        }
        let last = visible.len() - 1;
        if self.selected_index > last {
            self.selected_index = last;
        }
        // visible_entries is ordered by score on a query, not by
        // entry order — but scroll position is keyed off the on-
        // screen rendering order, which mirrors `visible_entries`,
        // so the position-in-visible is what we use here.
        //
        // Nudge `target_scroll_top` (NOT `scroll_top`) so the move
        // animates through the same lerp the wheel uses, matching the
        // command-palette scroll-follow feel. Writing `scroll_top`
        // directly would snap, then `tick_scroll` would lerp it back
        // toward the unchanged target.
        let row_advance = self.list_row_advance;
        let row_y = (self.selected_index as f32) * row_advance;
        let row_bottom = row_y + row_advance;
        let mut target = self.target_scroll_top;
        if row_y < target {
            target = row_y;
        }
        if row_bottom > target + list_height {
            target = row_bottom - list_height;
        }
        let max_scroll = self.max_scroll();
        self.target_scroll_top = target.clamp(0.0, max_scroll);
        // Keyboard nav is a deliberate jump, not a flick — kill any
        // in-flight inertial wheel velocity so it can't fight the move.
        self.scroll_velocity_px_s = 0.0;
        self.scroll_last_tick_at = None;
    }

    pub fn render(
        &mut self,
        sugarloaf: &mut Sugarloaf,
        rect: [f32; 4],
        theme: &IdeTheme,
        scale: f32,
        mouse: Option<(f32, f32)>,
        occlusion_rects: &[[f32; 4]],
    ) {
        view::render(self, sugarloaf, rect, theme, scale, mouse, occlusion_rects);
    }
}

impl Default for NeoismExtensionsPane {
    fn default() -> Self {
        Self::new()
    }
}

use super::view;

/// Liberal substring match from an entry category string to a tab.
/// Category strings come from several sources (engine adapters, MCP
/// registry, kernel manifests) with various spellings (`mcp`, `MCP`,
/// `LSP`, `language server`, ...) — we lowercase + substring-match so
/// all spellings collapse to one tab.
fn matches_tab(category: &str, tab: ExtensionTab) -> bool {
    let c = category.to_lowercase();
    match tab {
        ExtensionTab::McpServers => c.contains("mcp"),
        ExtensionTab::LanguageServers => {
            c.contains("lsp") || c.contains("language server")
        }
        ExtensionTab::TreeSitterParsers => {
            c.contains("tree-sitter")
                || c.contains("treesitter")
                || c.contains("syntax parser")
                || c == "syntax"
        }
        ExtensionTab::Kernels => c.contains("kernel"),
    }
}

fn point_in_rect(x: f32, y: f32, rect: [f32; 4]) -> bool {
    x >= rect[0] && y >= rect[1] && x <= rect[0] + rect[2] && y <= rect[1] + rect[3]
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::event::{KeyState, LogicalKey, Modifiers, NamedKey, PhysicalKey};
    use smol_str::SmolStr;

    fn entry(id: &str, status: ExtensionStatus) -> ExtensionEntry {
        ExtensionEntry {
            id: id.to_string(),
            name: format!("Entry {id}"),
            version: "1.0.0".into(),
            description: format!("Description for {id}"),
            author: "author".into(),
            downloads: Some(1),
            categories: vec!["mcp".into()],
            languages: Vec::new(),
            status,
            repository_url: None,
            lsp_source: None,
        }
    }

    fn entry_with(
        id: &str,
        name: &str,
        description: &str,
        categories: Vec<String>,
        status: ExtensionStatus,
    ) -> ExtensionEntry {
        ExtensionEntry {
            id: id.to_string(),
            name: name.to_string(),
            version: "1.0.0".into(),
            description: description.to_string(),
            author: "author".into(),
            downloads: Some(1),
            categories,
            languages: Vec::new(),
            status,
            repository_url: None,
            lsp_source: None,
        }
    }

    fn key_press(named: NamedKey) -> KeyDescriptor {
        KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(named),
            state: KeyState::Pressed,
            modifiers: Modifiers::empty(),
            repeat: false,
        }
    }

    fn key_press_char(c: &str) -> KeyDescriptor {
        KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Character(SmolStr::new(c)),
            state: KeyState::Pressed,
            modifiers: Modifiers::empty(),
            repeat: false,
        }
    }

    #[test]
    fn visible_entries_passes_all_with_empty_filter() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry("a", ExtensionStatus::NotInstalled),
            entry(
                "b",
                ExtensionStatus::Installed {
                    version: "1.0.0".into(),
                },
            ),
            entry("c", ExtensionStatus::NotInstalled),
        ]);
        pane.set_filter(ExtensionFilter::All);
        pane.set_active_tab(ExtensionTab::McpServers);
        assert_eq!(pane.visible_entries(), vec![0, 1, 2]);
    }

    #[test]
    fn visible_entries_filters_by_installed() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry("a", ExtensionStatus::NotInstalled),
            entry(
                "b",
                ExtensionStatus::Installed {
                    version: "1.0.0".into(),
                },
            ),
            entry("c", ExtensionStatus::BuiltIn),
        ]);
        pane.set_active_tab(ExtensionTab::McpServers);
        pane.set_filter(ExtensionFilter::Installed);
        assert_eq!(pane.visible_entries(), vec![1, 2]);
    }

    #[test]
    fn visible_entries_fuzzy_matches_description() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry_with(
                "fs",
                "Filesystem",
                "Files and folders",
                vec!["mcp".into()],
                ExtensionStatus::NotInstalled,
            ),
            entry_with(
                "gh",
                "Tooling",
                "GitHub MCP integration",
                vec!["mcp".into()],
                ExtensionStatus::NotInstalled,
            ),
            entry_with(
                "pg",
                "Postgres",
                "SQL database",
                vec!["mcp".into()],
                ExtensionStatus::NotInstalled,
            ),
        ]);
        pane.set_active_tab(ExtensionTab::McpServers);
        pane.set_filter(ExtensionFilter::All);
        pane.set_search_query("github".into());
        assert_eq!(pane.visible_entries(), vec![1]);
    }

    #[test]
    fn visible_entries_filters_by_tab() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry_with(
                "rust",
                "rust-analyzer",
                "Rust language server",
                vec!["LSP".into()],
                ExtensionStatus::NotInstalled,
            ),
            entry_with(
                "fs",
                "filesystem",
                "MCP file server",
                vec!["mcp".into()],
                ExtensionStatus::NotInstalled,
            ),
            entry_with(
                "py",
                "pyright",
                "Python lsp",
                vec!["language server".into()],
                ExtensionStatus::NotInstalled,
            ),
        ]);
        pane.set_filter(ExtensionFilter::All);
        pane.set_active_tab(ExtensionTab::McpServers);
        assert_eq!(pane.visible_entries(), vec![1]);
    }

    #[test]
    fn arrow_keys_navigate_visible_indices() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(
            (0..5)
                .map(|i| entry(&format!("e{i}"), ExtensionStatus::NotInstalled))
                .collect(),
        );
        pane.set_filter(ExtensionFilter::All);
        pane.set_active_tab(ExtensionTab::McpServers);

        assert!(pane
            .on_key(&key_press(NamedKey::ArrowDown))
            .action
            .is_none());
        assert!(pane
            .on_key(&key_press(NamedKey::ArrowDown))
            .action
            .is_none());
        assert!(pane
            .on_key(&key_press(NamedKey::ArrowDown))
            .action
            .is_none());
        assert_eq!(pane.selected_index, 3);

        // Push past the end — should stay at last visible index (4).
        assert!(pane
            .on_key(&key_press(NamedKey::ArrowDown))
            .action
            .is_none());
        assert!(pane
            .on_key(&key_press(NamedKey::ArrowDown))
            .action
            .is_none());
        assert_eq!(pane.selected_index, 4);
    }

    #[test]
    fn enter_returns_install_toggle_for_selected() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![
            entry("a", ExtensionStatus::NotInstalled),
            entry("b", ExtensionStatus::NotInstalled),
        ]);
        pane.set_filter(ExtensionFilter::All);
        pane.set_active_tab(ExtensionTab::McpServers);
        pane.selected_index = 1;

        let action = pane.on_key(&key_press(NamedKey::Enter)).action;
        assert_eq!(
            action,
            Some(PaneAction::InstallToggleRequested {
                id: "b".into(),
                currently_installed: false,
            })
        );
    }

    #[test]
    fn enter_on_built_in_entry_never_requests_package_lifecycle() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("godot-gdscript", ExtensionStatus::BuiltIn)]);
        pane.set_filter(ExtensionFilter::All);
        pane.set_active_tab(ExtensionTab::McpServers);

        let response = pane.on_key(&key_press(NamedKey::Enter));
        assert!(response.consumed);
        assert_eq!(response.action, None);
    }

    fn key_press_mods(named: NamedKey, modifiers: Modifiers) -> KeyDescriptor {
        KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Named(named),
            state: KeyState::Pressed,
            modifiers,
            repeat: false,
        }
    }

    fn key_press_char_mods(c: &str, modifiers: Modifiers) -> KeyDescriptor {
        KeyDescriptor {
            physical: PhysicalKey(0),
            logical: LogicalKey::Character(SmolStr::new(c)),
            state: KeyState::Pressed,
            modifiers,
            repeat: false,
        }
    }

    #[test]
    fn alt_arrow_cycles_category_tabs() {
        let mut pane = NeoismExtensionsPane::new();
        assert_eq!(pane.active_tab, ExtensionTab::McpServers);
        assert!(
            pane.on_key(&key_press_mods(NamedKey::ArrowRight, Modifiers::ALT))
                .consumed
        );
        assert_eq!(pane.active_tab, ExtensionTab::LanguageServers);
        pane.on_key(&key_press_mods(NamedKey::ArrowLeft, Modifiers::ALT));
        assert_eq!(pane.active_tab, ExtensionTab::McpServers);
        // Wrap backwards from the first tab to the last.
        pane.on_key(&key_press_mods(NamedKey::ArrowLeft, Modifiers::ALT));
        assert_eq!(pane.active_tab, ExtensionTab::Kernels);
    }

    #[test]
    fn slash_focuses_search_from_list() {
        let mut pane = NeoismExtensionsPane::new();
        assert!(!pane.search_focused());
        let resp = pane.on_key(&key_press_char("/"));
        assert!(resp.consumed);
        assert!(pane.search_focused());
    }

    #[test]
    fn slash_is_literal_while_typing_in_search() {
        let mut pane = NeoismExtensionsPane::new();
        pane.focus_search();
        // `/` while the search box owns focus must NOT be intercepted —
        // it's a real character the host routes through `on_text`.
        let resp = pane.on_key(&key_press_char("/"));
        assert!(!resp.consumed);
        assert!(pane.on_text("/"));
        assert_eq!(pane.search_query, "/");
    }

    #[test]
    fn ctrl_or_cmd_f_focuses_search() {
        let mut pane = NeoismExtensionsPane::new();
        assert!(
            pane.on_key(&key_press_char_mods("f", Modifiers::CTRL))
                .consumed
        );
        assert!(pane.search_focused());

        let mut pane = NeoismExtensionsPane::new();
        assert!(
            pane.on_key(&key_press_char_mods("f", Modifiers::META))
                .consumed
        );
        assert!(pane.search_focused());
    }

    #[test]
    fn arrow_down_in_search_drops_into_list() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(
            (0..3)
                .map(|i| entry(&format!("e{i}"), ExtensionStatus::NotInstalled))
                .collect(),
        );
        pane.set_active_tab(ExtensionTab::McpServers);
        pane.focus_search();
        assert!(pane.search_focused());
        let resp = pane.on_key(&key_press(NamedKey::ArrowDown));
        assert!(resp.consumed);
        assert!(!pane.search_focused());
        assert_eq!(pane.selected_index, 0);
    }

    #[test]
    fn pointer_click_search_focuses_and_closes_language_picker() {
        let mut pane = NeoismExtensionsPane::new();
        pane.search_input_rect = [0.0, 0.0, 200.0, 36.0];
        pane.set_active_tab(ExtensionTab::LanguageServers);
        pane.open_language_picker();
        assert!(pane.language_picker_open());
        // Clicking the main search box moves focus there (caret) and
        // dismisses the language dropdown.
        pane.on_pointer_down(10.0, 10.0, PointerButton::Left);
        assert!(pane.search_focused());
        assert!(!pane.language_picker_open());
    }

    #[test]
    fn pointer_click_language_search_focuses_dropdown() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_active_tab(ExtensionTab::LanguageServers);
        pane.open_language_picker();
        // Simulate the user having clicked away, then clicking back into
        // the dropdown's mini-search.
        pane.language_search_focused = false;
        pane.language_search_rect = [0.0, 0.0, 150.0, 30.0];
        pane.on_pointer_down(10.0, 10.0, PointerButton::Left);
        assert!(pane.language_search_focused());
        assert!(!pane.search_focused());
    }

    #[test]
    fn pointer_down_on_filter_pill_changes_filter() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);
        // Fake pill rects so pointer dispatch has something to hit.
        pane.filter_pill_rects = [
            [0.0, 0.0, 40.0, 20.0],
            [50.0, 0.0, 60.0, 20.0],
            [120.0, 0.0, 80.0, 20.0],
        ];
        let action = pane.on_pointer_down(80.0, 10.0, PointerButton::Left);
        assert!(action.is_none());
        assert_eq!(pane.filter, ExtensionFilter::Installed);
    }

    #[test]
    fn pointer_down_on_install_button_returns_action() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);
        // Simulate what view::draw_card_body pushes: a Focus rect
        // followed by a ToggleInstall rect (the button) on top.
        let focus_rect = [0.0, 0.0, 400.0, 80.0];
        let button_rect = [300.0, 24.0, 80.0, 32.0];
        pane.row_hits = vec![
            RowHit {
                rect: focus_rect,
                action: RowAction::Focus(0),
            },
            RowHit {
                rect: button_rect,
                action: RowAction::ToggleInstall("a".into()),
            },
        ];
        let action = pane.on_pointer_down(340.0, 40.0, PointerButton::Left);
        assert_eq!(
            action,
            Some(PaneAction::InstallToggleRequested {
                id: "a".into(),
                currently_installed: false,
            })
        );
    }

    #[test]
    fn on_text_appends_only_when_search_focused() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);

        // Not focused: ignored.
        assert!(!pane.on_text("g"));
        assert_eq!(pane.search_query, "");

        // Focused via pointer click on the search rect.
        pane.search_input_rect = [0.0, 0.0, 200.0, 36.0];
        pane.on_pointer_down(20.0, 20.0, PointerButton::Left);
        assert!(pane.search_focused());

        assert!(pane.on_text("git"));
        assert!(pane.on_text("hub"));
        assert_eq!(pane.search_query, "github");
    }

    #[test]
    fn escape_clears_search_when_focused() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);
        pane.search_input_rect = [0.0, 0.0, 200.0, 36.0];
        pane.on_pointer_down(10.0, 10.0, PointerButton::Left);
        pane.on_text("query");
        assert_eq!(pane.search_query, "query");

        pane.on_key(&key_press(NamedKey::Escape));
        assert!(!pane.search_focused());
        assert_eq!(pane.search_query, "");
    }

    #[test]
    fn backspace_trims_last_char_when_search_focused() {
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);
        pane.search_input_rect = [0.0, 0.0, 200.0, 36.0];
        pane.on_pointer_down(10.0, 10.0, PointerButton::Left);
        pane.on_text("ab");
        pane.on_key(&key_press(NamedKey::Backspace));
        assert_eq!(pane.search_query, "a");
    }

    #[test]
    fn character_key_press_is_routed_via_on_text_only() {
        // KeyDescriptor::Character should NOT mutate search_query
        // on its own — hosts emit a separate UiEvent::Text for IME
        // commit. Verify on_key ignores it.
        let mut pane = NeoismExtensionsPane::new();
        pane.set_entries(vec![entry("a", ExtensionStatus::NotInstalled)]);
        pane.focused_search = true;
        let _ = pane.on_key(&key_press_char("z"));
        assert_eq!(pane.search_query, "");
    }

    #[test]
    fn matches_tab_is_liberal_about_spellings() {
        assert!(matches_tab("MCP", ExtensionTab::McpServers));
        assert!(matches_tab("mcp-server", ExtensionTab::McpServers));
        assert!(matches_tab("LSP", ExtensionTab::LanguageServers));
        assert!(matches_tab(
            "Language Server",
            ExtensionTab::LanguageServers
        ));
        assert!(matches_tab(
            "Tree-sitter Parser",
            ExtensionTab::TreeSitterParsers
        ));
        assert!(matches_tab(
            "syntax parser",
            ExtensionTab::TreeSitterParsers
        ));
    }
}
