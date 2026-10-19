//! Conversation catalog and agent detail-rail state.
//!
//! The left catalog flattens project groups, root conversations and their
//! live subagent children into one list shared by painting, keyboard navigation
//! and pointer hits. The right rail retains informational session sections.
//! Branch lifecycle state stays per conversation family; catalog projections
//! preserve its completion grace, permission state and viewed-child retention.

#[cfg(test)]
mod catalog_subagent_tests;
#[cfg(test)]
mod dashboard_directory_tests;

use std::collections::{BTreeSet, HashMap, HashSet};
use web_time::Duration;
use web_time::Instant;

use crate::animation::CriticallyDampedSpring;

use crate::panels::agent_pane::icon::AgentKind;
use crate::widgets::scroll::Scroll;

#[cfg(test)]
mod detail_pointer_tests {
    use super::*;

    #[test]
    fn detail_pointer_hits_real_root_child_rows_not_headings_tasks_or_padding() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_subagents(vec![
            NeoismAgentSessionEntry::new("root", "main session", ""),
            NeoismAgentSessionEntry::new("child", "child task", "")
                .with_runtime_status(Some("running".into())),
        ]);
        panel.set_mode(SidePanelMode::Subagents);
        panel.set_last_panel_rect([500., 0., 240., 500.]);
        // The actual renderer registers a clipped branch-list slice and its full origin.
        panel.set_row_hit_rect_with_origin([510., 100., 220., 80.], 100., 40.);
        let selected = panel.selected_index();
        assert!(panel.detail_clickable_at(600., 110.));
        assert!(panel.detail_clickable_at(600., 150.));
        assert!(!panel.catalog_clickable_at(600., 150.)); // never the left catalog policy
        assert!(!panel.detail_clickable_at(600., 80.)); // heading
        assert!(!panel.detail_clickable_at(600., 200.)); // Tasks/status region
        assert!(!panel.detail_clickable_at(505., 110.)); // panel padding
        assert!(!panel.detail_clickable_at(800., 110.));
        assert_eq!(panel.selected_index(), selected); // hit testing is pure
        panel.set_user_hidden(true);
        assert!(!panel.detail_clickable_at(600., 110.));
        panel.set_user_hidden(false);
        panel.set_mode(SidePanelMode::Sessions);
        assert!(!panel.detail_clickable_at(600., 110.));
        panel.set_mode(SidePanelMode::Subagents);
        panel.clear_last_panel_rect();
        assert!(!panel.detail_clickable_at(600., 110.));
    }

    #[test]
    fn detail_pointer_uses_scrolled_origin_and_rejects_empty_or_non_session_rows() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_mode(SidePanelMode::Subagents);
        panel.set_last_panel_rect([500., 0., 240., 500.]);
        panel.set_row_hit_rect_with_origin([510., 100., 220., 40.], 60., 40.);
        assert!(!panel.detail_clickable_at(600., 110.));
        panel.subagents = vec![
            NeoismAgentSessionEntry::new("root", "main session", ""),
            NeoismAgentSessionEntry::new("child", "child task", "")
                .with_runtime_status(Some("running".into())),
        ];
        assert!(panel.detail_clickable_at(600., 110.)); // clipped child, not row 0
        assert!(!panel.detail_clickable_at(600., 70.)); // root scrolled out
        panel.subagents[1].is_header = true;
        assert!(!panel.detail_clickable_at(600., 110.));
        panel.clear_row_hit_rect();
        assert!(!panel.detail_clickable_at(600., 110.));
    }
}

#[cfg(test)]
mod navigation_header_tests {
    use super::*;

    #[test]
    fn capability_gates_and_resets_typed_hits() {
        let mut panel = NeoismAgentSidePanel::default();
        assert!(!panel.navigation_header_enabled());
        panel.set_header_action_rect(CatalogHeaderAction::Search, [0., 0., 80., 30.]);
        assert_eq!(panel.header_action_at(10., 10.), None);
        panel.set_navigation_header_enabled(true);
        panel.set_header_action_rect(CatalogHeaderAction::Search, [0., 0., 80., 30.]);
        panel.set_header_action_rect(CatalogHeaderAction::Projects, [0., 30., 80., 30.]);
        panel.set_header_action_rect(
            CatalogHeaderAction::AddWorkspace,
            [80., 30., 30., 30.],
        );
        assert_eq!(panel.header_action_at(10., 10.), Some(CatalogHeaderAction::Search));
        assert_eq!(
            panel.header_action_at(10., 40.),
            Some(CatalogHeaderAction::Projects)
        );
        assert_eq!(panel.header_action_at(90., 40.), None);
        assert_eq!(panel.project_button_rect(), Some([0., 30., 80., 30.]));
        panel.clear_new_chat_rect();
        assert_eq!(panel.project_button_rect(), None);
        panel.set_navigation_header_enabled(false);
        assert_eq!(panel.header_action_at(10., 10.), None);
    }

    #[test]
    fn removed_add_workspace_button_never_registers_hit_targets() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_navigation_header_enabled(true);
        panel.set_header_action_rect(CatalogHeaderAction::Search, [0., 0., 80., 30.]);
        panel.set_header_action_rect(CatalogHeaderAction::Projects, [0., 30., 80., 30.]);
        let add_rect = [80., 30., 30., 30.];
        panel.set_header_action_rect(CatalogHeaderAction::AddWorkspace, add_rect);
        assert_eq!(panel.header_action_at(90., 40.), None);
        panel.set_catalog_directory(Some("/work/project".into()));
        assert!(!panel.header_action_enabled(CatalogHeaderAction::AddWorkspace));
        assert_eq!(panel.header_action_at(90., 40.), None);
        panel.set_header_action_rect(CatalogHeaderAction::AddWorkspace, add_rect);
        assert_eq!(panel.header_action_at(90., 40.), None);
        assert_eq!(panel.header_action_at(10., 10.), Some(CatalogHeaderAction::Search));
        assert_eq!(
            panel.header_action_at(10., 40.),
            Some(CatalogHeaderAction::Projects)
        );
        panel.set_catalog_directory(None);
        assert!(panel.header_action_enabled(CatalogHeaderAction::AddWorkspace));
        assert_eq!(panel.header_action_at(90., 40.), None);
        panel.set_header_action_rect(CatalogHeaderAction::AddWorkspace, add_rect);
        assert_eq!(panel.header_action_at(90., 40.), None);
    }

    #[test]
    fn search_and_projects_are_separate_stops_in_root_and_scoped_catalogs() {
        for scope in [None, Some("/work/a".to_owned())] {
            let mut panel = NeoismAgentSidePanel::default();
            panel.set_navigation_header_enabled(true);
            panel.set_catalog_directory(scope);
            let mut chat = NeoismAgentSessionEntry::new("chat", "Chat", "");
            chat.directory = Some("/work/a".into());
            panel.set_sessions(vec![chat]);
            panel.select_new_chat();
            panel.select_next();
            assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
            assert!(!panel.search_focused());
            assert!(panel.selected_session().is_none());
            assert!(!panel.toggle_selected_project());
            assert!(!panel.navigate_project(false));
            panel.set_focused(true);
            assert_eq!(panel.catalog_highlight_index(None), None);
            panel.select_next();
            assert!(panel.projects_selected());
            assert!(!panel.new_chat_selected());
            assert!(panel.selected_session().is_none());
            assert!(panel.selected_catalog_row().is_none());
            assert!(!panel.toggle_selected_project());
            assert!(!panel.navigate_project(false));
            panel.set_focused(true);
            assert_eq!(panel.catalog_highlight_index(None), None);
            panel.select_next();
            assert!(!panel.projects_selected());
            let first = panel.selected_row().unwrap().stable_identity().to_owned();
            assert!(panel.selected_row().unwrap().is_selectable());
            panel.select_prev();
            assert!(panel.projects_selected());
            panel.select_prev();
            assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
            panel.select_prev();
            assert!(panel.new_chat_selected());
            panel.select_prev();
            assert!(panel.new_chat_selected());
            panel.select_next_by(10);
            assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
            panel.select_next_by(10);
            assert!(panel.projects_selected());
            panel.select_next();
            assert_eq!(panel.selected_row().unwrap().stable_identity(), first);
        }
    }

    #[test]
    fn search_header_focus_is_not_inline_input_and_survives_refresh() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_navigation_header_enabled(true);
        panel.set_focused(true);
        panel.select_header_action(CatalogHeaderAction::Search);
        panel.set_session_projects(vec![("/work/a".into(), 1)]);
        panel.set_sessions(vec![NeoismAgentSessionEntry::new("chat", "Chat", "")]);
        assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
        assert!(!panel.search_focused());
        assert!(panel.selected_session().is_none());
        assert_eq!(panel.catalog_highlight_index(None), None);
        panel.select_prev_by(10);
        assert!(panel.new_chat_selected());
        panel.select_header_action(CatalogHeaderAction::Search);
        panel.set_navigation_header_enabled(false);
        assert_eq!(panel.selected_header_action(), None);
    }

    #[test]
    fn projects_focus_survives_empty_refresh_scope_change_and_live_pins() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_navigation_header_enabled(true);
        panel.select_new_chat();
        panel.select_next();
        assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
        panel.select_next();
        assert!(panel.projects_selected());
        panel.select_next(); // Empty catalogs still expose Projects.
        assert!(panel.projects_selected());
        panel.set_session_projects(vec![("/work/a".into(), 1)]);
        assert!(panel.projects_selected());
        let mut chat = NeoismAgentSessionEntry::new("pin", "Pinned", "");
        chat.directory = Some("/work/a".into());
        chat.pinned = true;
        panel.upsert_session(chat);
        assert!(panel.projects_selected());
        panel.set_catalog_directory(Some("/work/a".into()));
        panel.set_sessions(vec![]);
        assert!(panel.projects_selected());
        assert!(!panel.toggle_selected_project());
        panel.select_prev();
        assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
        panel.select_prev();
        assert!(panel.new_chat_selected());
    }

    #[test]
    fn modal_search_hit_is_enabled_without_legacy_inline_focus() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.focus_search();
        panel.set_navigation_header_enabled(true);
        panel.set_header_action_rect(CatalogHeaderAction::Search, [0., 0., 80., 30.]);
        panel.set_session_search_rect([0., 0., 80., 30.]);
        panel.focus_search();
        assert!(!panel.search_focused());
        assert!(!panel.session_search_contains(10., 10.));
        assert_eq!(panel.header_action_at(10., 10.), Some(CatalogHeaderAction::Search));
        panel.set_selected_cursor_rect([0., 0., 1., 20.]);
        panel.select_projects();
        assert_eq!(panel.selected_cursor_rect(), None);
        panel.select_prev();
        assert_eq!(panel.selected_header_action(), Some(CatalogHeaderAction::Search));
        panel.select_prev();
        assert!(panel.new_chat_selected());
    }

    #[test]
    fn pointing_cursor_matches_buttons_and_chat_rows_not_section_captions() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_last_panel_height_rows(8);
        panel.set_catalog_directory(Some("/work/a".into()));
        panel.set_sessions(vec![
            NeoismAgentSessionEntry::new("pin", "Pin", "")
                .with_directory(Some("/work/a".into()))
                .with_pinned(true),
            NeoismAgentSessionEntry::new("chat", "Chat", "")
                .with_directory(Some("/work/a".into())),
        ]);
        panel.set_navigation_header_enabled(true);
        panel.set_new_chat_rect([0., 0., 240., 34.]);
        panel.set_header_action_rect(CatalogHeaderAction::Projects, [0., 42., 240., 34.]);
        let height = panel.row_height();
        let rect = [0., 100., 240., height * 4.];
        panel.set_last_panel_rect([0., 0., 240., 100. + height * 4.]);
        panel.set_row_hit_rect(rect, height);
        assert!(panel.catalog_clickable_at(100., 17.));
        assert!(panel.catalog_clickable_at(100., 59.));
        assert!(!panel.catalog_clickable_at(100., 38.));
        assert!(!panel.catalog_clickable_at(100., 100. + height * 0.5));
        assert!(panel.catalog_clickable_at(100., 100. + height * 1.5));
        assert!(!panel.catalog_clickable_at(100., 100. + height * 2.5));
        assert!(panel.catalog_clickable_at(100., 100. + height * 3.5));
        assert!(!panel.catalog_clickable_at(300., 100. + height * 3.5));
        panel.clear_new_chat_rect();
        assert!(!panel.catalog_clickable_at(100., 17.));
        assert!(!panel.catalog_clickable_at(100., 59.));
    }

    #[test]
    fn project_label_and_tooltip_follow_catalog_scope() {
        let mut panel = NeoismAgentSidePanel::default();
        assert_eq!(panel.project_button_label(), "~");
        panel.set_catalog_directory(Some("/work/neoism".into()));
        assert_eq!(panel.project_button_label(), "neoism");
        assert_eq!(panel.project_button_tooltip(), Some("/work/neoism"));
    }
}

/// Typed navigation intents; hosts own data loading and overlays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CatalogHeaderAction {
    Search,
    Projects,
    AddWorkspace,
}

/// Default width of the workspace Chats panel. The in-chat details
/// rail has its own fixed width and must not inherit this resizable value.
pub const SIDE_PANEL_WIDTH: f32 = 300.0;
pub const SIDE_PANEL_MIN_WIDTH: f32 = 180.0;
pub const SIDE_PANEL_MAX_WIDTH: f32 = 700.0;

/// Minimum agent-pane width below which the panel hides itself —
/// otherwise a narrow split would shove the chat content into nothing.
pub const SIDE_PANEL_MIN_PANE_WIDTH: f32 = 640.0;

/// Session rows carry an identity/time line above the title. Headers and
/// transcript excerpts share this stride so scrolling, hit-testing and search
/// results remain aligned even when the list mixes row kinds.
pub const ROW_HEIGHT: f32 = 42.0;
pub const FONT_SIZE: f32 = 13.0;
pub const ROW_PADDING_X: f32 = 12.0;
pub const FRAME_RADIUS: f32 = 14.0;
pub const FRAME_STROKE: f32 = 2.25;

// Snappy home-list scroll: short critically-damped catch-up so wheel /
// trackpad / arrow scrolling tracks the gesture tightly instead of
// trailing behind (the old 0.30s spring read as "laggy").
pub const SCROLL_ANIMATION_LENGTH: f32 = 0.12;
pub const CURSOR_ANIMATION_LENGTH: f32 = 0.12;
pub const SCROLL_OFF_ROWS: usize = 3;

/// Root conversation source. Child tasks are identified by their own kind,
/// never by a parent's external provider.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ConversationSource {
    #[default]
    Neoism,
    OpenCode,
    ClaudeCode,
    Codex,
}

impl ConversationSource {
    pub const CHOICES: [Self; 4] =
        [Self::Neoism, Self::OpenCode, Self::ClaudeCode, Self::Codex];

    pub fn label(self) -> &'static str {
        match self {
            Self::Neoism => "Neoism Agent",
            Self::OpenCode => "OpenCode",
            Self::ClaudeCode => "Claude Code",
            Self::Codex => "Codex",
        }
    }

    pub fn tab_title(self, title: Option<&str>) -> String {
        title
            .map(str::trim)
            .filter(|title| !title.is_empty())
            .unwrap_or_else(|| {
                if self == Self::Neoism {
                    "Neoism"
                } else {
                    self.label()
                }
            })
            .to_owned()
    }

    pub fn provider(self) -> Option<&'static str> {
        match self {
            Self::Neoism => None,
            Self::OpenCode => Some("opencode"),
            Self::ClaudeCode => Some("claude"),
            Self::Codex => Some("codex"),
        }
    }

    pub fn from_session_json(session: &serde_json::Value) -> Self {
        // Catalogues are root-only. Do not infer a child's identity from a
        // task's externalAgent status or a model provider.
        if session
            .get("parentID")
            .and_then(serde_json::Value::as_str)
            .is_some()
            || session
                .get("parentId")
                .and_then(serde_json::Value::as_str)
                .is_some()
        {
            return Self::Neoism;
        }
        match session
            .pointer("/extra/externalAgent/provider")
            .and_then(serde_json::Value::as_str)
            .or_else(|| {
                session
                    .pointer("/externalAgent/provider")
                    .and_then(serde_json::Value::as_str)
            })
            .or_else(|| {
                session
                    .get("external_provider")
                    .and_then(serde_json::Value::as_str)
            })
            .or_else(|| {
                session
                    .get("externalProvider")
                    .and_then(serde_json::Value::as_str)
            }) {
            Some("opencode") => Self::OpenCode,
            Some("claude") => Self::ClaudeCode,
            Some("codex") => Self::Codex,
            _ => Self::Neoism,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExternalSessionPreview {
    pub source_key: String,
    pub external_session_id: String,
    pub history_state: String,
    pub import_supported: bool,
    pub import_unavailable_reason: Option<String>,
    pub neoism_session_id: Option<String>,
}

mod project_pagination;
use project_pagination::ProjectSessionPage;

/// One entry in the sessions / sub-agents list. `time_label` doubles
/// as the right-aligned footer text — "5 minutes ago" for sessions,
/// agent name ("build" / "plan" / "main session") for sub-agents.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeoismAgentSessionEntry {
    pub id: String,
    pub title: String,
    /// Persisted execution directory; independent of the workspace displaying the catalog.
    pub directory: Option<String>,
    pub time_label: String,
    pub depth: usize,
    /// Owning conversation for a child projected into the left catalog.
    pub branch_root_id: Option<String>,
    pub branch_status: Option<BranchStatus>,
    pub branch_activity_label: Option<String>,
    pub source: ConversationSource,
    pub source_key: Option<String>,
    /// Present only for provider-native previews, never for stored roots.
    pub external_preview: Option<ExternalSessionPreview>,
    pub agent_kind: Option<AgentKind>,
    pub runtime_status: Option<String>,
    /// Server-owned root activity/attention, independent of child foreground work.
    pub catalog_activity: Option<neoism_agent_core::CatalogActivity>,
    /// Raw `time.updated` unix-ms, used to order projects and their chats.
    pub updated_ms: u64,
    /// Whether the session is pinned to the top of the list.
    pub pinned: bool,
    /// True for an injected cyan date-group / "Pinned" header row (not a
    /// selectable session). `title` holds the header label.
    pub is_header: bool,
    /// True for an injected semantic-match excerpt row shown under its
    /// session while a transcript search is active. `id` carries the parent
    /// session id (so activating it resumes that session) and `title` holds
    /// the matched excerpt text.
    pub is_excerpt: bool,
    /// Byte ranges into `title` that matched the active search terms —
    /// rendered highlighted on excerpt rows. Sorted and non-overlapping.
    pub highlights: Vec<(usize, usize)>,
}

/// One semantic transcript-search hit, per session (best chunk kept).
#[derive(Clone, Debug, PartialEq)]
pub struct NeoismAgentSemanticMatch {
    pub session_id: String,
    /// The matched transcript chunk, as returned by the server.
    pub excerpt: String,
    /// Cosine distance — lower is closer.
    pub distance: f64,
}

impl NeoismAgentSessionEntry {
    pub fn stable_identity(&self) -> &str {
        self.source_key.as_deref().unwrap_or(&self.id)
    }

    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        time_label: impl Into<String>,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            directory: None,
            time_label: time_label.into(),
            depth: 0,
            branch_root_id: None,
            branch_status: None,
            branch_activity_label: None,
            source: ConversationSource::Neoism,
            source_key: None,
            external_preview: None,
            agent_kind: None,
            runtime_status: None,
            catalog_activity: None,
            updated_ms: 0,
            pinned: false,
            is_header: false,
            is_excerpt: false,
            highlights: Vec::new(),
        }
    }

    /// A non-selectable date-group / "Pinned" header row.
    pub fn header(label: impl Into<String>) -> Self {
        Self {
            id: String::new(),
            title: label.into(),
            directory: None,
            time_label: String::new(),
            depth: 0,
            branch_root_id: None,
            branch_status: None,
            branch_activity_label: None,
            source: ConversationSource::Neoism,
            source_key: None,
            external_preview: None,
            agent_kind: None,
            runtime_status: None,
            catalog_activity: None,
            updated_ms: 0,
            pinned: false,
            is_header: true,
            is_excerpt: false,
            highlights: Vec::new(),
        }
    }

    /// A semantic-match excerpt row under `session_id`'s entry. Selectable:
    /// activating it resumes the parent session.
    pub fn excerpt(session_id: impl Into<String>, excerpt: impl Into<String>) -> Self {
        Self {
            id: session_id.into(),
            title: excerpt.into(),
            directory: None,
            time_label: String::new(),
            depth: 0,
            branch_root_id: None,
            branch_status: None,
            branch_activity_label: None,
            source: ConversationSource::Neoism,
            source_key: None,
            external_preview: None,
            agent_kind: None,
            runtime_status: None,
            catalog_activity: None,
            updated_ms: 0,
            pinned: false,
            is_header: false,
            is_excerpt: true,
            highlights: Vec::new(),
        }
    }

    pub fn with_directory(mut self, directory: Option<String>) -> Self {
        self.directory = directory.filter(|directory| !directory.trim().is_empty());
        self
    }

    pub fn is_project_page(&self) -> bool {
        self.is_header && self.id.starts_with("project-page:")
    }

    pub fn is_project(&self) -> bool {
        self.is_header && self.directory.is_some() && !self.is_project_page()
    }

    pub fn is_selectable(&self) -> bool {
        !self.is_header || self.is_project() || self.is_project_page()
    }

    pub fn with_source(mut self, source: ConversationSource) -> Self {
        self.source = source;
        self
    }

    pub fn with_source_key(mut self, source_key: Option<String>) -> Self {
        self.source_key = source_key;
        self
    }

    pub fn with_updated_ms(mut self, updated_ms: u64) -> Self {
        self.updated_ms = updated_ms;
        self
    }

    pub fn with_pinned(mut self, pinned: bool) -> Self {
        self.pinned = pinned;
        self
    }

    pub fn with_depth(mut self, depth: usize) -> Self {
        self.depth = depth;
        self
    }

    pub fn with_agent_kind(mut self, agent_kind: Option<AgentKind>) -> Self {
        self.agent_kind = agent_kind;
        self
    }

    pub fn with_catalog_activity(
        mut self,
        activity: Option<neoism_agent_core::CatalogActivity>,
    ) -> Self {
        self.catalog_activity = activity;
        self
    }

    pub fn with_runtime_status(mut self, runtime_status: Option<String>) -> Self {
        self.runtime_status = runtime_status;
        self
    }
}

fn generic_subagent_metadata(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "" | "agent" | "subagent" | "sub-agent" | "sub agent" | "untitled" | "unknown"
    )
}

/// Collapse a transcript excerpt to one displayable line: whitespace runs
/// (including newlines) become single spaces, and long chunks truncate on a
/// char boundary with an ellipsis.
fn compact_excerpt(excerpt: &str) -> String {
    let mut compact = String::with_capacity(excerpt.len().min(160));
    let mut last_space = true;
    for ch in excerpt.chars() {
        if ch.is_whitespace() {
            if !last_space {
                compact.push(' ');
                last_space = true;
            }
        } else {
            compact.push(ch);
            last_space = false;
        }
    }
    let compact = compact.trim().to_string();
    // Wrapping handles presentation; this is just a sanity ceiling so one
    // enormous chunk can't bloat the display list.
    const MAX: usize = 480;
    if compact.chars().count() <= MAX {
        return compact;
    }
    compact.chars().take(MAX).collect()
}

/// Byte ranges of every ASCII-case-insensitive occurrence of any search
/// term in `line`, merged where they touch or overlap. Terms are expected
/// pre-lowercased; ASCII lowering preserves byte offsets so the ranges
/// index the original string safely.
fn term_highlight_ranges(line: &str, terms: &[String]) -> Vec<(usize, usize)> {
    if terms.is_empty() {
        return Vec::new();
    }
    let haystack = line.to_ascii_lowercase();
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    for term in terms {
        let mut from = 0;
        while let Some(found) = haystack[from..].find(term.as_str()) {
            let start = from + found;
            ranges.push((start, start + term.len()));
            from = start + term.len().max(1);
        }
    }
    ranges.sort_unstable();
    let mut merged: Vec<(usize, usize)> = Vec::new();
    for (start, end) in ranges {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    merged
}

/// Word-wrap an excerpt into at most `max_lines` lines of `columns`
/// monospace characters. Overlong words hard-split; a truncated tail gets an
/// ellipsis on the final line.
fn wrap_excerpt_lines(excerpt: &str, columns: usize, max_lines: usize) -> Vec<String> {
    let columns = columns.max(8);
    let mut lines: Vec<String> = Vec::with_capacity(max_lines);
    let mut current = String::new();
    let mut current_len = 0usize;
    let mut truncated = false;
    'words: for word in excerpt.split_whitespace() {
        let mut word: Vec<char> = word.chars().collect();
        loop {
            let sep = usize::from(current_len > 0);
            if current_len + sep + word.len() <= columns {
                if sep == 1 {
                    current.push(' ');
                }
                current.extend(word.iter());
                current_len += sep + word.len();
                continue 'words;
            }
            // The word doesn't fit on this line.
            if word.len() > columns {
                // Hard-split an overlong token across lines.
                let take = columns - current_len - sep;
                if take > 0 {
                    if sep == 1 {
                        current.push(' ');
                    }
                    current.extend(word.iter().take(take));
                    word.drain(..take);
                }
            }
            if lines.len() + 1 == max_lines {
                truncated = true;
                break 'words;
            }
            lines.push(std::mem::take(&mut current));
            current_len = 0;
        }
    }
    if !current.is_empty() || lines.is_empty() {
        lines.push(current);
    }
    if truncated {
        if let Some(last) = lines.last_mut() {
            while last.chars().count() >= columns {
                last.pop();
            }
            last.push('…');
        }
    }
    lines.retain(|line| !line.is_empty());
    lines
}

fn wrap_excerpt_measured(
    excerpt: &str,
    width: f32,
    max_lines: usize,
    measure: &mut impl FnMut(&str) -> f32,
) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut truncated = false;
    'words: for word in excerpt.split_whitespace() {
        let candidate = if current.is_empty() {
            word.to_string()
        } else {
            format!("{current} {word}")
        };
        if measure(&candidate) <= width {
            current = candidate;
            continue;
        }
        if !current.is_empty() {
            lines.push(std::mem::take(&mut current));
            if lines.len() == max_lines {
                truncated = true;
                break;
            }
        }
        // Long paths/tokens split at UTF-8 character boundaries, using shaped widths.
        for ch in word.chars() {
            let candidate = format!("{current}{ch}");
            if !current.is_empty() && measure(&candidate) > width {
                lines.push(std::mem::take(&mut current));
                if lines.len() == max_lines {
                    truncated = true;
                    break 'words;
                }
            }
            current.push(ch);
        }
    }
    if !current.is_empty() && lines.len() < max_lines {
        lines.push(current);
    }
    if truncated {
        if let Some(last) = lines.last_mut() {
            while !last.is_empty() && measure(&format!("{last}…")) > width {
                last.pop();
            }
            last.push('…');
        }
    }
    lines
}

fn preserve_specific_subagent_metadata(
    incoming: &mut NeoismAgentSessionEntry,
    previous: &NeoismAgentSessionEntry,
) {
    if generic_subagent_metadata(&incoming.title)
        && !generic_subagent_metadata(&previous.title)
    {
        incoming.title = previous.title.clone();
    }
    if generic_subagent_metadata(&incoming.time_label)
        && !generic_subagent_metadata(&previous.time_label)
    {
        incoming.time_label = previous.time_label.clone();
    }
    if incoming.agent_kind.is_none() {
        incoming.agent_kind = previous.agent_kind;
    }
    // `/session/status` is only the live SessionRun set. Omission cannot
    // terminalize the parent-owned subtask lifecycle.
    if incoming.runtime_status.is_none() {
        incoming.runtime_status = previous.runtime_status.clone();
    }
}

/// Which list the side panel is currently steering selection/scroll
/// over. Set every frame by the renderer based on `pane.has_conversation()`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SidePanelMode {
    /// Home view — previous sessions.
    Sessions,
    /// In-session view — sibling / child sub-agent sessions.
    Subagents,
}

/// State of a branch (main session or child) inferred from its
/// latest messages. Drives the colored dot rendered next to each row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchStatus {
    /// Working — tool in flight or waiting for assistant reply.
    /// Rendered as a *blinking* white dot.
    Active,
    /// Blocked on a tool permission prompt — blinking yellow dot.
    WaitingPermission,
    /// Latest message is the assistant's reply — green dot.
    Completed,
    /// Latest signal looks like an error/abort — red dot.
    Stopped,
}

/// How much authority a branch-status transition has over a latched terminal
/// state. Run lifecycle edges may represent a real continuation of the same
/// child session; ancillary events may arrive late while cleaning up a run
/// that has already finished.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BranchStatusTransition {
    /// A real session/subtask lifecycle edge. Active states reopen a finished
    /// child, while terminal states latch it closed.
    AuthoritativeRun,
    /// Permission cleanup or other non-run activity. It may refine a live
    /// branch, but can never reopen a terminal-locked child.
    AncillaryActivity,
}

impl BranchStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "working",
            Self::WaitingPermission => "needs approval",
            Self::Completed => "done",
            Self::Stopped => "stopped",
        }
    }

    pub fn from_runtime_status(status: &str) -> Option<Self> {
        match status.trim().to_ascii_lowercase().as_str() {
            "active" | "busy" | "created" | "running" => Some(Self::Active),
            "blocked" | "retry" | "waiting_permission" | "waiting-permission" => {
                Some(Self::WaitingPermission)
            }
            "completed" | "complete" | "idle" | "done" => Some(Self::Completed),
            "failed" | "error" | "errored" | "stopped" | "aborted" | "interrupted"
            | "cancelled" | "canceled" => Some(Self::Stopped),
            _ => None,
        }
    }

    fn runtime_status(self) -> &'static str {
        match self {
            Self::Active => "running",
            Self::WaitingPermission => "blocked",
            Self::Completed => "completed",
            Self::Stopped => "error",
        }
    }
}

/// Per-branch (per-session) snapshot the side panel renders under each
/// row: the most recent tool the branch was using (truncated to fit)
/// and the derived run status.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BranchActivity {
    pub status: BranchStatus,
    pub current_tool: Option<String>,
    pub started_at: Option<u64>,
    /// When the branch last transitioned into a terminal state
    /// (`Completed` / `Stopped`). Drives the timed auto-hide of
    /// finished sub-agents from the sidebar — see
    /// [`NeoismAgentSidePanel::visible_subagents`]. Reset to `None`
    /// the moment the branch goes back to `Active` /
    /// `WaitingPermission` so a respawned sub-agent reappears.
    pub completed_at: Option<Instant>,
    /// Set when the branch reached a terminal state via an
    /// *authoritative* lifecycle signal — the parent's `task` tool
    /// part finishing, the child session going `idle`, or an explicit
    /// `session.subtask.completed`. Once latched, noisy part-level
    /// activity (a straggler text/reasoning delta from the child that
    /// still claims "responding"/"thinking") can no longer resurrect
    /// the row to "active". This is the fix for sub-agents that stay
    /// stuck on "responding"/"working" after they've actually
    /// finished: those late part deltas are not lifecycle events, so
    /// they must not un-terminate the branch. Only another
    /// authoritative signal (a genuine respawn) clears the latch.
    pub terminal_locked: bool,
}

impl BranchActivity {
    /// Build a fresh activity for `status`. A branch that is *first
    /// observed* already terminal (e.g. a sub-agent that finished before
    /// the panel opened, or one re-listed by a recovery snapshot) gets NO
    /// visibility window — `completed_at` stays `None`, which
    /// [`NeoismAgentSidePanel::subagent_hidden`] treats as "hide
    /// immediately". The 7s show-then-hide window is only started by a
    /// live `active → terminal` transition in [`Self::transition_status`].
    fn new(
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> Self {
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        Self {
            status,
            current_tool: if terminal { None } else { current_tool },
            started_at: if terminal { None } else { started_at },
            completed_at: None,
            terminal_locked: false,
        }
    }

    /// Transition an existing activity to `status`, maintaining the
    /// `completed_at` auto-hide clock: stamp it on the first edge into
    /// a terminal state, clear it the moment the branch becomes active
    /// again (a respawn) so the row reappears. Returns `true` when the
    /// new status is terminal (caller can skip tool/started-at edits).
    fn transition_status(&mut self, status: BranchStatus) -> bool {
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        let was_terminal =
            matches!(self.status, BranchStatus::Completed | BranchStatus::Stopped);
        if terminal {
            // Stamp the show-then-hide window ONLY on the edge into a
            // terminal state (a genuine live completion the user is
            // watching). A terminal→terminal re-apply (e.g. a recovery
            // snapshot re-reporting an already-finished child) must NOT restart the
            // window — otherwise old completions keep flashing back.
            if !was_terminal {
                self.completed_at = Some(Instant::now());
            }
            self.current_tool = None;
            self.started_at = None;
        } else {
            self.completed_at = None;
        }
        self.status = status;
        terminal
    }
}

/// Catalog feed bookkeeping is independent of the selected/open chat pane.
/// Keep terminal activities even after row removal, so recovery cannot restart
/// their completion grace. Retired executions reject delayed rollover traffic.
#[derive(Default)]
struct CatalogFamilyState {
    directory: String,
    execution_id: Option<String>,
    revision: u64,
    retired_executions: HashSet<Option<String>>,
    activities: HashMap<String, BranchActivity>,
    terminal_revisions: HashMap<String, u64>,
    rows: Vec<NeoismAgentSessionEntry>,
}

impl CatalogFamilyState {
    fn visible_rows(&self) -> Vec<NeoismAgentSessionEntry> {
        self.rows
            .iter()
            .filter(|row| {
                self.activities.get(&row.id).is_some_and(|activity| {
                    match activity.status {
                        BranchStatus::Active | BranchStatus::WaitingPermission => true,
                        BranchStatus::Completed | BranchStatus::Stopped => {
                            let grace = if activity.status == BranchStatus::Stopped {
                                FAILED_SUBAGENT_HIDE_AFTER
                            } else {
                                SUBAGENT_HIDE_AFTER
                            };
                            activity.completed_at.is_some_and(|at| at.elapsed() < grace)
                        }
                    }
                })
            })
            .cloned()
            .collect()
    }
}

/// How long a finished (`Completed` / `Stopped`) sub-agent lingers in
/// the sidebar before it's auto-hidden, mirroring Claude's behaviour.
/// A respawn (status back to active) clears the timer and the row
/// returns immediately.
pub const SUBAGENT_HIDE_AFTER: Duration = Duration::from_secs(7);
const FAILED_SUBAGENT_HIDE_AFTER: Duration = Duration::from_secs(60);

/// How long the composer's status label survives a *transient* idle
/// reading. Between events the raw derivation can pass through Idle for
/// a tick (MessageEnd → next MessageStart, tool-phase handoffs, a
/// child's inter-message idle edge zeroing the active-subagent count);
/// blanking the label for that frame also collapsed the status row's
/// reserved height and made the whole composer bounce. The display
/// derivation therefore keeps showing the last non-idle status until
/// idle has persisted for this window; genuine completion still clears
/// once the window elapses.
pub const STATUS_LABEL_GRACE: Duration = Duration::from_millis(400);

/// Memory behind [`STATUS_LABEL_GRACE`]: the status the composer row
/// last displayed, refreshed every frame the raw derivation is
/// non-idle. Stored on the side panel (shared by the desktop and web
/// panes) behind a `RefCell` so the `&self` display getters can record
/// it — same interior-mutability precedent as the pane's timeline
/// layout cache.
#[derive(Clone)]
struct StatusDisplayHold {
    state: crate::panels::agent_pane::state::NeoismAgentStreamingState,
    /// Anchor whose `elapsed()` continues the displayed timer through
    /// the gap, so the label's wave animation / elapsed suffix don't
    /// jump to zero while the hold bridges two events.
    elapsed_anchor: Instant,
    /// Last frame the raw derivation was non-idle. The grace window
    /// counts from here and is deliberately NOT refreshed while the
    /// hold itself is what's showing.
    last_seen: Instant,
    /// Session the label belonged to — a session switch must never show
    /// the previous conversation's held label.
    session: Option<String>,
}

/// Lifecycle of the session's persistent goal, mirroring the backend
/// `goal.status` field. Drives the status shown alongside "Goal" in the
/// side-panel section heading.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum GoalStatus {
    /// The agent is actively pursuing the goal.
    #[default]
    Active,
    /// The goal has been accomplished.
    Complete,
    /// The agent is blocked and can't make progress unaided.
    Blocked,
}

impl GoalStatus {
    pub fn from_str(status: &str) -> Self {
        match status.trim().to_ascii_lowercase().as_str() {
            "complete" | "completed" | "done" => Self::Complete,
            "blocked" | "stuck" => Self::Blocked,
            _ => Self::Active,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Active => "Active",
            Self::Complete => "Complete",
            Self::Blocked => "Blocked",
        }
    }
}

/// The session's persistent goal, surfaced live above the task list.
/// Mirrors the backend goal model: `text` + `status` + an optional
/// `summary` (what the agent accomplished, or why it's blocked) and
/// the `paused` flag. Only rendered when `text` is non-empty.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct SessionGoal {
    pub text: String,
    pub status: GoalStatus,
    pub summary: String,
    pub paused: bool,
    /// Backend `updated` millis — the monotonic version used to drop a
    /// stale `GET /goal` poll that raced (and would clobber) a newer
    /// live `SESSION_UPDATED`. See `SidePanel::set_session_goal`.
    pub updated: u64,
}

impl SessionGoal {
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }

    /// Parse the backend goal object (`{ text, status, summary,
    /// paused, ... }`). Returns `None` for a null/absent goal or one
    /// with empty text so the section stays hidden.
    pub fn from_json(goal: &serde_json::Value) -> Option<Self> {
        if goal.is_null() {
            return None;
        }
        let text = goal
            .get("text")
            .and_then(|text| text.as_str())
            .unwrap_or_default()
            .to_string();
        if text.trim().is_empty() {
            return None;
        }
        Some(Self {
            text,
            status: goal
                .get("status")
                .and_then(|status| status.as_str())
                .map(GoalStatus::from_str)
                .unwrap_or_default(),
            summary: goal
                .get("summary")
                .and_then(|summary| summary.as_str())
                .unwrap_or_default()
                .to_string(),
            paused: goal
                .get("paused")
                .and_then(|paused| paused.as_bool())
                .unwrap_or(false),
            updated: goal
                .get("updated")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0),
        })
    }
}

#[cfg(test)]
mod resilience_tests {
    use super::*;

    fn chat(id: &str) -> NeoismAgentSessionEntry {
        NeoismAgentSessionEntry::new(id, id, "now").with_directory(Some("/work".into()))
    }

    #[test]
    fn ordered_mutations_delete_and_recreate_without_losing_other_rows() {
        use CatalogSessionMutation::{Delete, Upsert};
        let mut panel = NeoismAgentSidePanel::default();
        panel.upsert_sessions([chat("a"), chat("b"), chat("c")]);
        panel.selected = panel.sessions.iter().position(|e| e.id == "b").unwrap();
        let anchor = panel.session_viewport_anchor();
        panel.apply_session_mutations([
            Delete("a".into()),
            Upsert(chat("a")),
            Upsert(chat("c")),
            Delete("c".into()),
        ]);
        assert_eq!(
            panel
                .all_sessions
                .iter()
                .map(|e| e.id.as_str())
                .collect::<Vec<_>>(),
            ["b", "a"]
        );
        assert_eq!(panel.selected_catalog_row().unwrap().id, "b");
        assert_eq!(panel.session_viewport_anchor(), anchor);
        assert!(!panel.deleted_session_ids.contains("a"));
        assert!(panel.deleted_session_ids.contains("c"));
        panel.apply_session_mutations([]);
        assert_eq!(panel.selected_catalog_row().unwrap().id, "b");
    }

    #[test]
    fn mixed_batch_preserves_scrolled_anchor_and_selected_identity() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.upsert_sessions((0..30).map(|n| {
            let mut entry = chat(&format!("chat-{n}"));
            entry.updated_ms = n;
            entry
        }));
        panel.last_panel_height_rows = 3;
        let index = panel.sessions.iter().position(|e| e.id == "chat-15").unwrap();
        panel.selected = index;
        panel.scroll_px = index as f32 * panel.row_height() + 5.0;
        panel.scroll_top = index;
        let anchor = panel.session_viewport_anchor();
        let mut renamed = chat("chat-15");
        renamed.title = "renamed".into();
        renamed.updated_ms = 15;
        panel.apply_session_mutations([
            CatalogSessionMutation::Delete("chat-29".into()),
            CatalogSessionMutation::Upsert(renamed),
        ]);
        assert_eq!(panel.selected_catalog_row().unwrap().id, "chat-15");
        assert_eq!(panel.session_viewport_anchor(), anchor);
        assert_eq!(panel.catalog_session("chat-15").unwrap().title, "renamed");
    }

    #[test]
    fn catalog_retries_are_bounded_and_explicit_reopen_resets_budget() {
        let mut panel = NeoismAgentSidePanel::default();
        for attempt in 1..=3 {
            panel.mark_refresh_kicked();
            panel.set_sessions_error("offline");
            assert!(!panel.should_refresh_sessions());
            panel.last_sessions_refresh =
                Some(Instant::now() - std::time::Duration::from_secs(10));
            assert_eq!(panel.should_refresh_sessions(), attempt < 3);
        }
        panel.set_user_hidden(true);
        panel.toggle_visibility();
        assert!(panel.should_refresh_sessions());
        panel.mark_refresh_kicked();
        panel.set_sessions_error("offline");
        assert!(!panel.should_refresh_sessions());
        assert!(panel.retry_session_catalog());
        assert!(panel.should_refresh_sessions());
        panel.set_sessions(vec![chat("cached")]);
        assert!(!panel.retry_session_catalog());
        panel.set_sessions_error("offline with cached rows");
        assert!(panel.catalog_session("cached").is_some());
        assert!(panel.retry_session_catalog());
    }
}

/// Ordered live catalog mutations. Later mutations for the same ID win.
pub enum CatalogSessionMutation {
    Upsert(NeoismAgentSessionEntry),
    Delete(String),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SessionCatalogState {
    Initial,
    Loading,
    Ready,
    Error(String),
}

pub struct NeoismAgentSidePanel {
    width: f32,
    focused: bool,
    /// User-driven open/close — set by the bottom-right toggle button.
    /// When `true`, the renderer skips drawing the side panel column
    /// entirely (the carve helper returns `None`) so the chat area
    /// reclaims the full pane width.
    user_hidden: bool,
    /// Bottom-right toggle button rect, cached every frame so the
    /// click handler can hit-test it without recomputing the layout.
    toggle_button_rect: Option<[f32; 4]>,
    mode: SidePanelMode,
    scroll: Scroll,
    cursor_spring: CriticallyDampedSpring,
    scroll_top: usize,
    /// Continuous pixel scroll position for the home (sessions) list —
    /// the target `self.scroll` animates toward. Drives pixel-precise
    /// wheel / trackpad scrolling (no whole-row quantization dead zone);
    /// `scroll_top` is derived as `floor(scroll_px / row_h)`. Subagent
    /// mode uses `content_scroll_px` instead and leaves this at 0.
    scroll_px: f32,
    selected: usize,
    last_scroll_frame: Instant,
    last_cursor_frame: Instant,
    last_panel_height_rows: usize,
    /// Last screen rect the panel was rendered into. Click / wheel
    /// handlers in `screen::bridges::agent` read this to decide whether
    /// pointer events belong to the panel without having to recompute
    /// the carve.
    last_panel_rect: Option<[f32; 4]>,
    /// Screen rect for the actual selectable row list. In chat mode the
    /// branch rows sit below session/usage headers, so hit-testing the
    /// whole panel would map clicks to the wrong row. The rect is
    /// clamped to the visible viewport (so a click in a lower section
    /// is rejected), while `last_row_origin_y` keeps the *full* list's
    /// top — which can sit above the viewport once scrolled — so the
    /// row math stays anchored correctly.
    last_row_hit_rect: Option<[f32; 4]>,
    last_row_hit_height: f32,
    last_row_origin_y: f32,
    /// Where the selected row's cursor caret should sit. The screen
    /// render polls this and routes the global trail cursor to it when
    /// the panel owns focus — same pattern file_tree uses so Alt+Right
    /// animates the actual block cursor into the panel.
    selected_cursor_rect: Option<[f32; 4]>,
    /// Flat, unfiltered source list of home-mode sessions (no header
    /// rows). `sessions` is derived from this by
    /// [`rebuild_session_display`](Self::rebuild_session_display) — sorted
    /// pinned-first / newest-day-first, filtered by `session_query`, with
    /// cyan date-group header rows injected.
    all_sessions: Vec<NeoismAgentSessionEntry>,
    /// Live catalog edges take precedence over in-flight list snapshots.
    session_activity_overrides: HashMap<String, neoism_agent_core::CatalogActivity>,
    external_sessions: Vec<NeoismAgentSessionEntry>,
    external_importing_key: Option<String>,
    external_errors: Vec<(ConversationSource, String)>,
    external_scanning: bool,
    /// Live search filter for the home-mode session list. Typed while the
    /// panel owns focus; matches session titles case-insensitively.
    session_query: String,
    /// Query the current `semantic_results` were fetched for (trimmed).
    /// Results only apply while it still matches the live filter, so a
    /// stale fetch that raced further typing never mislabels the list.
    semantic_query: String,
    /// Sessions matched by server-side semantic transcript search,
    /// best-first. Unioned into the display list with title matches.
    semantic_results: Vec<NeoismAgentSemanticMatch>,
    /// True while a semantic transcript search is in flight — the empty
    /// list state renders a skeleton shimmer instead of "No results".
    semantic_searching: bool,
    semantic_search_started: Option<Instant>,
    /// Monospace column budget for wrapping semantic excerpt chunks into
    /// list rows, fed back from the renderer's measured panel width.
    result_wrap_columns: usize,
    measured_excerpt_lines: HashMap<String, Vec<String>>,
    result_wrap_metrics: Option<(u32, u32, Option<usize>, u32)>,
    /// True when the selection cursor sits on the search row (reached by
    /// arrow-up past the first session, or by typing). While set, the
    /// trail cursor renders on the search field and no session row is
    /// highlighted.
    search_focused: bool,
    /// Exact painted hit rectangle for the home-mode session search field.
    /// Separate from the row list so mobile never summons the keyboard for a
    /// session row or a vertical list drag.
    session_search_rect: Option<[f32; 4]>,
    sessions: Vec<NeoismAgentSessionEntry>,
    /// Workspace-owned catalog scope, never the selected chat's execution directory.
    catalog_directory: Option<String>,
    /// Authorized dashboard identity, independent of the current catalog scope.
    dashboard_directory: Option<String>,
    collapsed_projects: HashSet<String>,
    project_pages: HashMap<String, ProjectSessionPage>,
    project_pagination: bool,
    project_generation: u64,
    retired_project_requests: HashSet<u64>,
    search_only_session_ids: HashSet<String>,
    discovered_pinned_versions: HashMap<String, u64>,
    deleted_session_ids: HashSet<String>,
    session_catalog_state: SessionCatalogState,
    visible_running_indicator: bool,
    /// Opaque keyset cursor returned by the session catalogue. `None` means
    /// the successful page was final.
    session_next_cursor: Option<String>,
    /// Prevents repeated wheel/touch events at the bottom from launching the
    /// same continuation request more than once.
    session_page_loading: bool,
    session_requested_cursor: Option<String>,
    session_request_generation: u64,
    session_refresh_attempts: u8,
    /// Phase origin for the first-load session skeleton. The panel starts
    /// unloaded, so this clock can begin with the panel state and stops
    /// participating in redraws as soon as `set_sessions` lands.
    sessions_loading_started: Instant,
    /// Last time we kicked off a refresh of the sessions list.
    /// Used to debounce refreshes when the home view re-renders every
    /// frame.
    last_sessions_refresh: Option<Instant>,
    /// Sub-agent / sibling session list shown in chat mode. First entry
    /// is always "main session" (the parent), followed by children — the
    /// shape `fetch_subagent_options` returns. We render the section
    /// only when this list has at least one *non-main* entry.
    subagents: Vec<NeoismAgentSessionEntry>,
    /// Session whose transcript is currently open in the pane.
    viewed_session_id: Option<String>,
    /// None means legacy/unbound; Some(None) is authoritative no-chat context.
    catalog_viewed_root: Option<Option<String>>,
    catalog_viewed_session: Option<Option<String>>,
    catalog_subagents: HashMap<String, Vec<NeoismAgentSessionEntry>>,
    catalog_families: HashMap<String, CatalogFamilyState>,
    detail_family_root: Option<String>,
    /// Keep the viewed child as a return-navigation affordance, even when
    /// opening a historical completion. Cleared when navigating away.
    retained_viewed_subagent_id: Option<String>,
    subagents_loaded: bool,
    /// Only one branch-tree snapshot may be in flight at a time. The
    /// generation invalidates an older worker when the viewed session or
    /// live branch tree changes, preventing late snapshots from replacing
    /// newer event-driven state.
    subagent_refresh_in_flight: bool,
    subagent_refresh_generation: u64,
    /// Per-session activity snapshot used to paint the indented
    /// connector + tool + status dot under each branch row. Keyed by
    /// session id so it survives sub-agent list reorderings.
    branch_activities: HashMap<String, BranchActivity>,
    /// The session's persistent goal, refreshed from
    /// `GET /session/:id/goal` on session change / `SESSION_UPDATED`.
    /// Rendered above the task list when its `text` is non-empty.
    session_goal: Option<SessionGoal>,
    /// Last time a goal refetch was kicked. `None` forces the next
    /// `should_refresh_goal` to fire (used on session change /
    /// `SESSION_UPDATED`).
    last_goal_refresh: Option<Instant>,
    /// Set true once the goal has been fetched at least once for the
    /// current session, so we don't refetch every frame.
    goal_loaded: bool,
    /// Monotonic version (backend `updated` millis) of the most recent
    /// goal state we applied. A slow `GET /goal` poll that returns an
    /// OLDER snapshot than a live event already applied is dropped, so
    /// the Goal section never flickers active → stale → active.
    goal_version: u64,
    /// Pixel scroll offset for the *whole* chat-mode content column
    /// (Directory → Session → Usage → Goal → Branches → Tasks). Unlike
    /// `scroll_top` (the home-view row cursor), this lets every section
    /// scroll as one viewport so nothing falls off the bottom out of
    /// reach. Clamped against `content_scroll_max` each frame.
    content_scroll_px: f32,
    content_scroll_max: f32,
    reveal_selected_branch: bool,
    navigation_header_enabled: bool,
    header_rects: Vec<(CatalogHeaderAction, [f32; 4])>,
    new_chat_rect: Option<[f32; 4]>,
    new_chat_selected: bool,
    selected_header: Option<CatalogHeaderAction>,
    provider_menu_open: bool,
    provider_selection: usize,
    hovered_session: Option<usize>,
    hovered_session_identity: Option<String>,
    session_hover_target: bool,
    session_hover_scale: f32,
    session_title_hover_started: Option<Instant>,
    session_title_hover_overflow: bool,
    last_hover_frame: Instant,
    /// Hit target spanning the compact Usage meter and numeric label.
    usage_rect: Option<[f32; 4]>,
    /// Last values actually presented by the compact Usage surface.
    usage_snapshot: Option<(u64, Option<u64>)>,
    usage_scramble_started: Option<Instant>,
    usage_scramble_revision: u64,
    /// See [`StatusDisplayHold`] / [`STATUS_LABEL_GRACE`].
    status_hold: std::cell::RefCell<Option<StatusDisplayHold>>,
}

impl Default for NeoismAgentSidePanel {
    fn default() -> Self {
        Self {
            width: SIDE_PANEL_WIDTH,
            focused: false,
            // Default visible; the renderer still suppresses the panel
            // on narrow panes, and the user can toggle it with Alt+H or
            // the top-bar action.
            user_hidden: false,
            toggle_button_rect: None,
            mode: SidePanelMode::Sessions,
            scroll: Scroll::new().with_animation_length(SCROLL_ANIMATION_LENGTH),
            cursor_spring: CriticallyDampedSpring::new(),
            scroll_top: 0,
            scroll_px: 0.0,
            selected: 0,
            last_scroll_frame: Instant::now(),
            last_cursor_frame: Instant::now(),
            last_panel_height_rows: 1,
            last_panel_rect: None,
            last_row_hit_rect: None,
            last_row_hit_height: ROW_HEIGHT,
            last_row_origin_y: 0.0,
            selected_cursor_rect: None,
            all_sessions: Vec::new(),
            session_activity_overrides: HashMap::new(),
            external_sessions: Vec::new(),
            external_importing_key: None,
            external_errors: Vec::new(),
            external_scanning: false,
            session_query: String::new(),
            semantic_query: String::new(),
            semantic_results: Vec::new(),
            semantic_searching: false,
            semantic_search_started: None,
            result_wrap_columns: 42,
            measured_excerpt_lines: HashMap::new(),
            result_wrap_metrics: None,
            search_focused: false,
            session_search_rect: None,
            session_next_cursor: None,
            session_page_loading: false,
            session_requested_cursor: None,
            session_request_generation: 0,
            session_refresh_attempts: 0,
            sessions: Vec::new(),
            catalog_directory: None,
            dashboard_directory: None,
            collapsed_projects: HashSet::new(),
            project_pages: HashMap::new(),
            project_pagination: false,
            project_generation: 0,
            retired_project_requests: HashSet::new(),
            search_only_session_ids: HashSet::new(),
            discovered_pinned_versions: HashMap::new(),
            deleted_session_ids: HashSet::new(),
            session_catalog_state: SessionCatalogState::Initial,
            visible_running_indicator: false,
            sessions_loading_started: Instant::now(),
            last_sessions_refresh: None,
            subagents: Vec::new(),
            viewed_session_id: None,
            catalog_viewed_root: None,
            catalog_viewed_session: None,
            catalog_subagents: HashMap::new(),
            catalog_families: HashMap::new(),
            detail_family_root: None,
            retained_viewed_subagent_id: None,
            subagents_loaded: false,
            subagent_refresh_in_flight: false,
            subagent_refresh_generation: 0,
            branch_activities: HashMap::new(),
            session_goal: None,
            last_goal_refresh: None,
            goal_loaded: false,
            goal_version: 0,
            content_scroll_px: 0.0,
            content_scroll_max: 0.0,
            reveal_selected_branch: false,
            navigation_header_enabled: false,
            header_rects: Vec::new(),
            new_chat_rect: None,
            new_chat_selected: false,
            selected_header: None,
            provider_menu_open: false,
            provider_selection: 0,
            hovered_session: None,
            hovered_session_identity: None,
            session_hover_target: false,
            session_hover_scale: 0.0,
            session_title_hover_started: None,
            session_title_hover_overflow: false,
            last_hover_frame: Instant::now(),
            usage_rect: None,
            usage_snapshot: None,
            usage_scramble_started: None,
            usage_scramble_revision: 0,
            status_hold: std::cell::RefCell::new(None),
        }
    }
}

impl NeoismAgentSidePanel {
    pub fn navigation_header_enabled(&self) -> bool {
        self.navigation_header_enabled
    }

    pub fn set_navigation_header_enabled(&mut self, enabled: bool) {
        if self.navigation_header_enabled == enabled {
            return;
        }
        self.navigation_header_enabled = enabled;
        self.selected_header = None;
        self.selected_cursor_rect = None;
        self.clear_header_rects();
        if enabled {
            self.session_search_rect = None;
            self.search_focused = false;
        }
    }

    pub fn clear_header_rects(&mut self) {
        self.header_rects.clear();
    }

    pub fn header_action_enabled(&self, action: CatalogHeaderAction) -> bool {
        self.navigation_header_enabled
            && (action != CatalogHeaderAction::AddWorkspace
                || self.catalog_directory.is_none())
    }

    pub fn set_header_action_rect(
        &mut self,
        action: CatalogHeaderAction,
        rect: [f32; 4],
    ) {
        self.header_rects.retain(|(a, _)| *a != action);
        if action != CatalogHeaderAction::AddWorkspace
            && self.header_action_enabled(action)
        {
            self.header_rects.push((action, rect));
        }
    }

    pub fn header_action_at(&self, x: f32, y: f32) -> Option<CatalogHeaderAction> {
        if !self.navigation_header_enabled {
            return None;
        }
        self.header_rects
            .iter()
            .find_map(|(action, [rx, ry, w, h])| {
                (self.header_action_enabled(*action)
                    && x >= *rx
                    && x < rx + w
                    && y >= *ry
                    && y < ry + h)
                    .then_some(*action)
            })
    }

    pub fn project_button_rect(&self) -> Option<[f32; 4]> {
        self.header_rects
            .iter()
            .find_map(|(a, r)| (*a == CatalogHeaderAction::Projects).then_some(*r))
    }

    /// Full selected path, suitable for a host tooltip. Scope stays catalog-owned.
    pub fn project_button_tooltip(&self) -> Option<&str> {
        self.catalog_directory()
    }

    pub fn project_button_label(&self) -> &str {
        self.catalog_directory()
            .map(|path| {
                path.trim_end_matches(['/', '\\'])
                    .rsplit(['/', '\\'])
                    .next()
                    .filter(|name| !name.is_empty())
                    .unwrap_or(path)
            })
            .unwrap_or("~")
    }

    /// Catalog-only presentation: disable controls owned by other surfaces
    /// even when an old render frame left hit rectangles or focus behind.
    pub fn hide_catalog_controls(&mut self) {
        self.clear_header_rects();
        self.new_chat_rect = None;
        self.selected_header = None;
        self.new_chat_selected = false;
        self.session_search_rect = None;
        self.provider_menu_open = false;
        self.search_focused = false;
    }

    pub fn provider_menu_open(&self) -> bool {
        self.provider_menu_open
    }

    pub fn toggle_provider_menu(&mut self) {
        self.provider_menu_open = !self.provider_menu_open;
        self.provider_selection = 0;
        self.search_focused = false;
    }

    pub fn close_provider_menu(&mut self) {
        self.provider_menu_open = false;
    }

    pub fn provider_selection(&self) -> usize {
        self.provider_selection
    }

    pub fn move_provider_selection(&mut self, down: bool) {
        self.provider_selection = if down {
            (self.provider_selection + 1) % ConversationSource::CHOICES.len()
        } else {
            (self.provider_selection + ConversationSource::CHOICES.len() - 1)
                % ConversationSource::CHOICES.len()
        };
    }

    pub fn choose_provider(&mut self, index: usize) -> Option<ConversationSource> {
        let source = ConversationSource::CHOICES.get(index).copied()?;
        self.provider_menu_open = false;
        Some(source)
    }

    pub fn set_new_chat_rect(&mut self, rect: [f32; 4]) {
        self.new_chat_rect = Some(rect);
    }

    pub fn selected_header_action(&self) -> Option<CatalogHeaderAction> {
        (self.navigation_header_enabled && matches!(self.mode, SidePanelMode::Sessions))
            .then_some(self.selected_header)
            .flatten()
    }

    pub fn projects_selected(&self) -> bool {
        self.selected_header_action() == Some(CatalogHeaderAction::Projects)
    }

    pub fn select_header_action(&mut self, action: CatalogHeaderAction) {
        if self.navigation_header_enabled && matches!(self.mode, SidePanelMode::Sessions)
            && matches!(action, CatalogHeaderAction::Search | CatalogHeaderAction::Projects)
        {
            self.new_chat_selected = false;
            self.search_focused = false;
            self.selected_header = Some(action);
            self.cursor_spring.reset();
            self.selected_cursor_rect = None;
        }
    }

    pub fn select_projects(&mut self) {
        self.select_header_action(CatalogHeaderAction::Projects);
    }

    pub fn new_chat_selected(&self) -> bool {
        self.new_chat_selected && matches!(self.mode, SidePanelMode::Sessions)
    }

    pub fn select_new_chat(&mut self) {
        if matches!(self.mode, SidePanelMode::Sessions) {
            self.search_focused = false;
            self.new_chat_selected = true;
            self.selected_header = None;
            self.selected_cursor_rect = None;
            self.cursor_spring.reset();
        }
    }

    pub fn clear_new_chat_rect(&mut self) {
        self.clear_header_rects();
        self.new_chat_rect = None;
    }

    /// Pure cursor hit policy for the right-hand conversation branch list.
    /// Its renderer-registered viewport excludes headings, Tasks and status padding.
    pub fn detail_clickable_at(&self, x: f32, y: f32) -> bool {
        if self.user_hidden || !matches!(self.mode, SidePanelMode::Subagents) {
            return false;
        }
        self.last_panel_rect()
            .filter(|_| self.contains_point(x, y))
            .and_then(|rect| self.hit_test_row(x, y, rect))
            .and_then(|row| self.subagents.get(row))
            .is_some_and(|entry| !entry.is_header && !entry.id.is_empty())
    }

    pub fn catalog_clickable_at(&self, x: f32, y: f32) -> bool {
        if !matches!(self.mode, SidePanelMode::Sessions) {
            return false;
        }
        if self.new_chat_hit(x, y).is_some() || self.header_action_at(x, y).is_some() {
            return true;
        }
        self.last_panel_rect()
            .and_then(|rect| self.hit_test_row(x, y, rect))
            .and_then(|row| self.sessions.get(row))
            .is_some_and(|entry| {
                if entry.is_project_page() {
                    self.project_page_action_at(x, y)
                } else {
                    entry.is_selectable()
                }
            })
    }

    pub fn new_chat_hit(&self, x: f32, y: f32) -> Option<Option<usize>> {
        let [rx, ry, rw, rh] = self.new_chat_rect?;
        if x < rx || x > rx + rw || y < ry {
            return None;
        }
        if y <= ry + rh {
            return Some(None);
        }
        if self.provider_menu_open && y <= ry + rh * 5.0 {
            return Some(Some(((y - ry - rh) / rh) as usize));
        }
        None
    }

    pub fn sync_conversation_details_from(&mut self, catalog: &Self) {
        let root = catalog.subagents.first().map(|row| row.id.as_str());
        self.sync_conversation_details_from_root(catalog, root);
    }

    pub fn sync_conversation_details_from_root(
        &mut self,
        catalog: &Self,
        root: Option<&str>,
    ) {
        let previous_root = self
            .detail_family_root
            .as_deref()
            .or_else(|| self.subagents.first().map(|row| row.id.as_str()));
        let conversation_changed = previous_root != root;
        if conversation_changed {
            self.subagents.clear();
            self.branch_activities.clear();
            self.retained_viewed_subagent_id = None;
            self.content_scroll_px = 0.0;
            self.reveal_selected_branch = false;
            self.selected = 0;
        }
        self.detail_family_root = root.map(str::to_owned);
        self.viewed_session_id
            .clone_from(&catalog.viewed_session_id);
        // Pin the new exact view before merging/pruning the family roster.
        self.retained_viewed_subagent_id
            .clone_from(&catalog.retained_viewed_subagent_id);
        if conversation_changed || self.subagents != catalog.subagents {
            self.set_subagents(catalog.subagents.clone());
        }
        if self.branch_activities != catalog.branch_activities {
            self.branch_activities
                .clone_from(&catalog.branch_activities);
        }
        if self.session_goal != catalog.session_goal {
            self.session_goal.clone_from(&catalog.session_goal);
        }
        if self.goal_loaded != catalog.goal_loaded {
            self.goal_loaded = catalog.goal_loaded;
        }
        if self.goal_version != catalog.goal_version {
            self.goal_version = catalog.goal_version;
        }
    }

    pub fn width(&self) -> f32 {
        self.width
    }

    pub fn set_width(&mut self, width: f32) {
        self.width = width.clamp(SIDE_PANEL_MIN_WIDTH, SIDE_PANEL_MAX_WIDTH);
    }

    pub fn resize(&mut self, delta: f32) {
        self.set_width(self.width + delta);
    }

    pub fn is_focused(&self) -> bool {
        self.focused
    }

    pub fn set_focused(&mut self, focused: bool) {
        self.focused = focused;
        if !focused {
            self.search_focused = false;
        }
    }

    pub fn tick_pointer_animations(
        &mut self,
        hovered_session: Option<usize>,
        hovered_identity: Option<&str>,
    ) {
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_hover_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_hover_frame = now;

        if let Some(hovered) = hovered_session {
            let identity_changed =
                self.hovered_session_identity.as_deref() != hovered_identity;
            if Some(hovered) != self.hovered_session || identity_changed {
                self.hovered_session = Some(hovered);
                self.hovered_session_identity = hovered_identity.map(str::to_owned);
                self.session_hover_scale = 0.0;
                self.session_title_hover_started = Some(now);
                self.session_title_hover_overflow = false;
            }
        } else {
            self.hovered_session = None;
            self.hovered_session_identity = None;
            self.session_hover_scale = 0.0;
            self.session_title_hover_started = None;
            self.session_title_hover_overflow = false;
        }
        self.session_hover_target = hovered_session.is_some();
        let target = if self.session_hover_target { 1.0 } else { 0.0 };
        let blend = 1.0 - (-dt * 18.0).exp();
        self.session_hover_scale += (target - self.session_hover_scale) * blend;
        if (self.session_hover_scale - target).abs() < 0.002 {
            self.session_hover_scale = target;
            if target == 0.0 {
                self.hovered_session = None;
            }
        }
    }

    pub fn hovered_session(&self) -> Option<usize> {
        self.hovered_session
    }

    pub fn session_hover_scale(&self) -> f32 {
        self.session_hover_scale
    }

    pub fn session_title_hover_elapsed(&self) -> Option<f32> {
        self.session_title_hover_started
            .map(|started| started.elapsed().as_secs_f32())
    }

    pub fn set_session_title_hover_overflow(&mut self, overflow: bool) {
        self.session_title_hover_overflow = overflow;
    }

    pub fn update_usage_meter(&mut self, used: u64, context_limit: Option<u64>) {
        let snapshot = (used, context_limit);
        if self.usage_snapshot == Some(snapshot) {
            return;
        }
        self.usage_snapshot = Some(snapshot);
        self.usage_scramble_started = Some(Instant::now());
        self.usage_scramble_revision = self.usage_scramble_revision.wrapping_add(1);
    }

    pub fn usage_scramble_elapsed_ms(&self) -> Option<f32> {
        let elapsed = self.usage_scramble_started?.elapsed().as_secs_f32() * 1000.0;
        (elapsed < 320.0).then_some(elapsed)
    }

    pub fn usage_scramble_revision(&self) -> u64 {
        self.usage_scramble_revision
    }

    pub fn set_usage_rect(&mut self, rect: [f32; 4]) {
        self.usage_rect = Some(rect);
    }

    pub fn clear_usage_rect(&mut self) {
        self.usage_rect = None;
    }

    pub fn usage_contains(&self, x: f32, y: f32) -> bool {
        self.usage_rect.is_some_and(|[rx, ry, rw, rh]| {
            x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
        })
    }

    pub fn user_hidden(&self) -> bool {
        self.user_hidden
    }

    pub fn set_user_hidden(&mut self, hidden: bool) {
        if self.user_hidden && !hidden {
            self.retry_session_catalog();
        }
        self.user_hidden = hidden;
        if hidden {
            self.reset_visible_running_indicator();
            self.focused = false;
            self.selected_cursor_rect = None;
        }
    }

    /// Toggle the panel's visibility. When hiding, also drops keyboard
    /// focus so the cursor returns to the agent input.
    pub fn toggle_visibility(&mut self) {
        self.set_user_hidden(!self.user_hidden);
    }

    #[allow(dead_code)]
    pub fn toggle_button_rect(&self) -> Option<[f32; 4]> {
        self.toggle_button_rect
    }

    pub fn set_toggle_button_rect(&mut self, rect: [f32; 4]) {
        self.toggle_button_rect = Some(rect);
    }

    pub fn clear_toggle_button_rect(&mut self) {
        self.toggle_button_rect = None;
    }

    pub fn toggle_button_contains(&self, x: f32, y: f32) -> bool {
        let Some([bx, by, bw, bh]) = self.toggle_button_rect else {
            return false;
        };
        x >= bx && x <= bx + bw && y >= by && y <= by + bh
    }

    #[allow(dead_code)]
    pub fn mode(&self) -> SidePanelMode {
        self.mode
    }

    /// Set by the renderer each frame so navigation / scroll / click
    /// operate on the correct list. Resets selection cursor if the
    /// active list shrinks below the previous index.
    pub fn set_mode(&mut self, mode: SidePanelMode) {
        if self.mode == mode {
            return;
        }
        self.mode = mode;
        self.selected_header = None;
        self.new_chat_selected = false;
        self.selected = 0;
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
        self.cursor_spring.reset();
        self.content_scroll_px = 0.0;
        self.reveal_selected_branch = false;
    }

    /// The session's persistent goal, if any has been fetched.
    pub fn session_goal(&self) -> Option<&SessionGoal> {
        self.session_goal.as_ref()
    }

    /// Apply a goal update from a live event or a `GET /goal` poll,
    /// guarded by a monotonic `version` (backend `updated` millis).
    ///
    /// Two things this gets right that a bare assignment did not:
    ///  - **No flicker.** A slow poll can land *after* a newer live
    ///    event already advanced the goal; applying its stale snapshot
    ///    made the section blink active → old → active. We drop any
    ///    update that is not strictly newer than what we've shown.
    ///  - **Completed goals retire cleanly.** A `Complete` update hides the
    ///    finished goal while still advancing `goal_version`, so a stale
    ///    active snapshot cannot resurrect it and a genuinely newer goal can
    ///    replace it normally.
    ///
    /// `version == 0` means "unversioned" — a poll that found no goal.
    /// It must never clear a goal a live event just set (authoritative
    /// clears arrive as a versioned event), so a version-0 `None` is
    /// ignored.
    pub fn set_session_goal(&mut self, goal: Option<SessionGoal>, version: u64) {
        self.goal_loaded = true;
        if version != 0 && version <= self.goal_version {
            return;
        }
        if version == 0 && goal.is_none() {
            return;
        }
        if version != 0 {
            self.goal_version = version;
        }
        self.session_goal =
            goal.filter(|goal| !goal.is_empty() && goal.status != GoalStatus::Complete);
    }

    /// Reset the goal section for a session switch: drop the cached goal
    /// AND its version so the next session's goal (whatever its version)
    /// applies cleanly. Pair with `invalidate_goal_refresh` to force the
    /// refetch.
    pub fn reset_session_goal(&mut self) {
        self.session_goal = None;
        self.goal_version = 0;
        self.goal_loaded = false;
    }

    /// Optimistically clear the displayed goal after a user-driven
    /// `/goal clear` (or an empty `/goal`). Unlike [`reset_session_goal`]
    /// this keeps `goal_version`, so a slow in-flight `GET /goal` poll that
    /// still carries the just-removed goal (same version) is dropped by the
    /// monotonic guard instead of resurrecting the section — while the
    /// authoritative clear (a versioned `SESSION_UPDATED`) still applies.
    pub fn clear_session_goal_local(&mut self) {
        self.session_goal = None;
        self.goal_loaded = true;
    }

    /// True when the goal is stale enough to justify a refetch. Loaded
    /// goals refresh slowly (every few seconds) since live edits also
    /// arrive via `SESSION_UPDATED` → `invalidate_goal_refresh`.
    pub fn should_refresh_goal(&self) -> bool {
        if !self.goal_loaded {
            return self
                .last_goal_refresh
                .map(|last| {
                    Instant::now().saturating_duration_since(last).as_millis() >= 400
                })
                .unwrap_or(true);
        }
        self.last_goal_refresh
            .map(|last| {
                Instant::now().saturating_duration_since(last).as_secs_f32() >= 6.0
            })
            .unwrap_or(true)
    }

    pub fn mark_goal_refresh_kicked(&mut self) {
        self.last_goal_refresh = Some(Instant::now());
    }

    /// Force the next `should_refresh_goal` to fire — used on session
    /// change and when a `SESSION_UPDATED` event reports the goal may
    /// have changed, so the side panel reflects it promptly.
    pub fn invalidate_goal_refresh(&mut self) {
        self.last_goal_refresh = None;
        self.goal_loaded = false;
    }

    /// Current pixel scroll offset of the whole chat-mode content
    /// column. The renderer subtracts this from every section's `y`.
    pub fn content_scroll_px(&self) -> f32 {
        self.content_scroll_px
    }

    pub fn take_reveal_selected_branch(&mut self) -> bool {
        std::mem::take(&mut self.reveal_selected_branch)
    }

    /// Record the total scrollable overflow for the chat-mode content
    /// column (content height minus viewport height, clamped at 0) and
    /// re-clamp the current offset against it. Called by the renderer
    /// once it knows the laid-out content height.
    pub fn set_content_scroll_max(&mut self, max: f32) {
        self.content_scroll_max = max.max(0.0);
        self.content_scroll_px =
            self.content_scroll_px.clamp(0.0, self.content_scroll_max);
    }

    /// Scroll the whole chat-mode content column by `delta_pixels`
    /// (positive scrolls the content up / reveals lower sections),
    /// clamped to the known overflow. Returns whether the offset moved.
    pub fn scroll_content_pixels(&mut self, delta_pixels: f32) -> bool {
        let before = self.content_scroll_px;
        self.content_scroll_px =
            (self.content_scroll_px + delta_pixels).clamp(0.0, self.content_scroll_max);
        self.content_scroll_px != before
    }

    pub fn sessions(&self) -> &[NeoismAgentSessionEntry] {
        &self.sessions
    }

    /// Hosts supply only the observed, authorized dashboard path. Use the same
    /// exact saved-directory identity as grouping; never infer it from a basename,
    /// execution cwd, filesystem root, or the client's home.
    pub fn set_dashboard_directory(&mut self, directory: Option<String>) {
        let directory = directory.filter(|directory| !directory.trim().is_empty());
        if self.dashboard_directory == directory {
            return;
        }
        self.dashboard_directory = directory;
        if self.catalog_directory.is_some() {
            return;
        }
        if !matches!(self.mode, SidePanelMode::Sessions) {
            self.rebuild_session_display();
            return;
        }
        let selected = self
            .selected_catalog_row()
            .map(|entry| entry.stable_identity().to_owned());
        let at_top = self.scroll_px <= f32::EPSILON;
        let anchor = self.session_viewport_anchor();
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        if at_top {
            self.scroll_px = 0.0;
            self.scroll_top = 0;
            self.scroll.set_target_immediate(0.0);
        } else {
            self.restore_session_viewport_anchor(anchor);
        }
    }

    pub fn catalog_directory(&self) -> Option<&str> {
        self.catalog_directory.as_deref()
    }

    pub fn set_catalog_directory(&mut self, directory: Option<String>) -> bool {
        let directory = directory.filter(|directory| !directory.trim().is_empty());
        if self.catalog_directory == directory {
            return false;
        }
        self.catalog_directory = directory;
        self.header_rects
            .retain(|(action, _)| *action != CatalogHeaderAction::AddWorkspace);
        self.invalidate_session_catalog();
        true
    }

    fn in_catalog_scope(&self, entry: &NeoismAgentSessionEntry) -> bool {
        self.catalog_directory
            .as_deref()
            .is_none_or(|directory| entry.directory.as_deref() == Some(directory))
    }

    pub fn invalidate_session_catalog(&mut self) {
        self.selected = 0;
        self.selected_cursor_rect = None;
        self.cursor_spring.reset();
        self.all_sessions.clear();
        self.catalog_subagents.clear();
        self.catalog_families.clear();
        self.external_sessions.clear();
        self.external_errors.clear();
        self.external_importing_key = None;
        self.external_scanning = false;
        self.semantic_results.clear();
        self.semantic_query.clear();
        self.semantic_searching = false;
        self.semantic_search_started = None;
        self.measured_excerpt_lines.clear();
        self.result_wrap_metrics = None;
        self.project_pages.clear();
        self.retired_project_requests.clear();
        self.search_only_session_ids.clear();
        self.discovered_pinned_versions.clear();
        self.project_pagination = false;
        self.collapsed_projects.clear();
        self.deleted_session_ids.clear();
        self.catalog_viewed_root = None;
        self.session_activity_overrides.clear();
        self.sessions.clear();
        self.session_next_cursor = None;
        self.session_requested_cursor = None;
        self.session_page_loading = false;
        self.session_request_generation =
            self.session_request_generation.saturating_add(1);
        self.session_refresh_attempts = 0;
        self.session_catalog_state = SessionCatalogState::Initial;
        self.sessions_loading_started = Instant::now();
        self.last_sessions_refresh = None;
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
    }

    pub fn sessions_loaded(&self) -> bool {
        matches!(
            self.session_catalog_state,
            SessionCatalogState::Ready | SessionCatalogState::Error(_)
        )
    }

    pub fn session_catalog_state(&self) -> &SessionCatalogState {
        &self.session_catalog_state
    }

    /// Seconds since the initial session-list skeleton began.
    pub fn sessions_loading_elapsed(&self) -> f32 {
        Instant::now()
            .saturating_duration_since(self.sessions_loading_started)
            .as_secs_f32()
    }

    pub fn subagents(&self) -> &[NeoismAgentSessionEntry] {
        &self.subagents
    }

    pub fn visible_subagents(&self) -> impl Iterator<Item = &NeoismAgentSessionEntry> {
        self.subagents
            .iter()
            .enumerate()
            .filter_map(move |(index, entry)| {
                (index == 0 || !self.subagent_hidden(entry)).then_some(entry)
            })
    }

    /// Apply a complete replacement for one root from the tab-independent feed.
    /// Returns true when accepted (including duplicates and empty replacements),
    /// false for lower revisions, retired executions, deleted roots, or out-of-scope
    /// data. Equal revisions still refresh canonical metadata and permissions.
    pub fn apply_catalog_family_snapshot(
        &mut self,
        root: &str,
        snapshot: neoism_agent_core::CatalogFamilySnapshot,
    ) -> bool {
        if root.is_empty()
            || self.deleted_session_ids.contains(root)
            || self
                .catalog_directory
                .as_deref()
                .is_some_and(|directory| directory != snapshot.directory)
        {
            return false;
        }
        if let Some(family) = self.catalog_families.get(root) {
            if family.execution_id == snapshot.execution_id {
                if snapshot.family_revision < family.revision {
                    return false;
                }
            } else if family.retired_executions.contains(&snapshot.execution_id) {
                return false;
            }
        }
        let family = self
            .catalog_families
            .entry(root.to_owned())
            .or_insert_with(|| CatalogFamilyState {
                execution_id: snapshot.execution_id.clone(),
                ..CatalogFamilyState::default()
            });
        if family.execution_id != snapshot.execution_id {
            family.retired_executions.insert(family.execution_id.take());
            family.execution_id = snapshot.execution_id;
            family.activities.clear();
            family.terminal_revisions.clear();
        }
        family.directory = snapshot.directory.clone();
        family.revision = snapshot.family_revision;
        family.rows.clear();
        for branch in snapshot.branches {
            if branch.session_id == root
                || self.deleted_session_ids.contains(&branch.session_id)
            {
                continue;
            }
            let status = match branch.status.as_str() {
                "outstanding" if branch.permission_pending => {
                    BranchStatus::WaitingPermission
                }
                "outstanding" => BranchStatus::Active,
                "completed" => BranchStatus::Completed,
                "failed" => BranchStatus::Stopped,
                _ => continue,
            };
            let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
            let can_reopen = family.terminal_revisions.get(&branch.session_id)
                .is_none_or(|revision| snapshot.family_revision > *revision);
            if terminal {
                family.terminal_revisions.insert(branch.session_id.clone(), snapshot.family_revision);
            } else if can_reopen {
                family.terminal_revisions.remove(&branch.session_id);
            }
            let activity = family.activities.entry(branch.session_id.clone())
                .and_modify(|activity| {
                    // Equal revisions can update canonical metadata/permissions,
                    // but only a newer lifecycle revision can reopen a terminal
                    // child (a Task follow-up within the same execution).
                    if !activity.terminal_locked || terminal || can_reopen {
                        activity.transition_status(status);
                        activity.terminal_locked = matches!(
                            status, BranchStatus::Completed | BranchStatus::Stopped
                        );
                        if !activity.terminal_locked {
                            activity.started_at = branch.started_at;
                        }
                    }
                })
                .or_insert_with(|| {
                    let mut activity = BranchActivity::new(status, None, branch.started_at);
                    activity.terminal_locked = matches!(
                        status, BranchStatus::Completed | BranchStatus::Stopped
                    );
                    activity
                });
            let mut row = NeoismAgentSessionEntry::new(
                branch.session_id,
                branch.title,
                branch.agent.unwrap_or_default(),
            )
            .with_directory(Some(snapshot.directory.clone()))
            .with_depth(branch.depth)
            .with_runtime_status(Some(activity.status.runtime_status().to_owned()));
            row.agent_kind = AgentKind::from_label(&row.time_label);
            row.branch_root_id = Some(root.to_owned());
            row.branch_status = Some(activity.status);
            family.rows.push(row);
        }
        let rows = family.visible_rows();
        self.set_catalog_subagent_rows(root, rows);
        true
    }

    /// Reconnect/scope reset: discard both projections and lifecycle watermarks.
    pub fn clear_catalog_families(&mut self) -> bool {
        let changed = !self.catalog_families.is_empty() || !self.catalog_subagents.is_empty();
        self.catalog_families.clear();
        let roots: Vec<_> = self.catalog_subagents.keys().cloned().collect();
        for root in roots {
            self.set_catalog_subagent_rows(&root, Vec::new());
        }
        changed
    }

    /// Scoped catalog streams can reconnect independently in the browser.
    pub fn clear_catalog_families_in_scope(&mut self, directory: Option<&str>) -> bool {
        let Some(directory) = directory else {
            return self.clear_catalog_families();
        };
        let roots: Vec<_> = self.catalog_families.iter()
            .filter(|(_, family)| family.directory == directory)
            .map(|(root, _)| root.clone())
            .collect();
        let changed = !roots.is_empty();
        for root in roots {
            self.catalog_families.remove(&root);
            self.set_catalog_subagent_rows(&root, Vec::new());
        }
        changed
    }

    fn catalog_completion_pending(&self) -> bool {
        self.catalog_subagents.values().flatten().any(|row| {
            matches!(row.branch_status, Some(BranchStatus::Completed | BranchStatus::Stopped))
        })
    }

    fn tick_catalog_families(&mut self) -> bool {
        if !self.catalog_completion_pending() {
            return false;
        }
        let updates: Vec<_> = self.catalog_families.iter()
            .filter_map(|(root, family)| {
                let rows = family.visible_rows();
                (self.catalog_subagents.get(root).map(Vec::as_slice).unwrap_or_default()
                    != rows.as_slice()).then(|| (root.clone(), rows))
            })
            .collect();
        let changed = !updates.is_empty();
        for (root, rows) in updates {
            self.set_catalog_subagent_rows(&root, rows);
        }
        changed
    }

    fn set_catalog_subagent_rows(
        &mut self,
        root: &str,
        rows: Vec<NeoismAgentSessionEntry>,
    ) {
        if self.catalog_subagents.get(root) == Some(&rows)
            || (rows.is_empty() && !self.catalog_subagents.contains_key(root))
        {
            return;
        }
        let selected = self
            .selected_catalog_row()
            .map(|row| row.stable_identity().to_owned());
        let anchor = self.session_viewport_anchor();
        if rows.is_empty() {
            self.catalog_subagents.remove(root);
        } else {
            self.catalog_subagents.insert(root.to_owned(), rows);
        }
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    pub fn set_catalog_viewed_session(&mut self, session: Option<&str>) {
        self.catalog_viewed_session =
            Some(session.filter(|id| !id.is_empty()).map(str::to_owned));
    }

    /// Bind the catalog to the focused buffer's owning root. Explicit None
    /// clears passive paint; it never falls back to cached pane/session data.
    pub fn set_catalog_viewed_root(&mut self, root: Option<&str>) {
        self.catalog_viewed_root =
            Some(root.filter(|id| !id.is_empty()).map(str::to_owned));
    }

    pub fn catalog_viewed_root<'a>(
        &'a self,
        legacy_root: Option<&'a str>,
    ) -> Option<&'a str> {
        match &self.catalog_viewed_root {
            Some(root) => root.as_deref(),
            None => legacy_root,
        }
    }

    pub fn catalog_viewed_session<'a>(
        &'a self,
        legacy_root: Option<&'a str>,
    ) -> Option<&'a str> {
        match &self.catalog_viewed_session {
            Some(viewed) => viewed.as_deref(),
            None => self.catalog_viewed_root(legacy_root),
        }
    }

    /// Paint policy only: never changes the keyboard candidate or viewport.
    pub fn catalog_highlight_index(&self, legacy_root: Option<&str>) -> Option<usize> {
        if self.focused {
            return (!self.new_chat_selected()
                && self.selected_header_action().is_none()
                && !self.search_focused()
                && self.selected < self.sessions.len())
            .then_some(self.selected);
        }
        let viewed = self.catalog_viewed_session(legacy_root)?;
        self.sessions
            .iter()
            .position(|row| !row.is_header && !row.is_excerpt && row.id == viewed)
    }

    pub fn detail_highlight_index(&self, viewed: Option<&str>) -> Option<usize> {
        if self.focused {
            return (self.selected < self.subagents.len()).then_some(self.selected);
        }
        let viewed = viewed?;
        self.subagents.iter().position(|row| row.id == viewed)
    }

    pub fn viewed_session_id(&self) -> Option<&str> {
        self.viewed_session_id.as_deref()
    }

    pub fn set_viewed_session_id(&mut self, session_id: Option<String>) {
        let session_id = session_id.filter(|id| !id.is_empty());
        self.retained_viewed_subagent_id = session_id
            .as_ref()
            .filter(|id| {
                !self
                    .subagents
                    .first()
                    .is_some_and(|root| root.id == id.as_str())
            })
            .cloned();
        self.viewed_session_id = session_id;
        self.prune_expired_completed_subagents();
    }

    /// Record the non-idle status the composer row is displaying this
    /// frame. `elapsed_seconds` is the timer currently shown with it,
    /// carried into the hold so the animation phase stays continuous if
    /// the raw derivation dips through Idle next frame. Idle is never
    /// recorded — the hold only ever bridges *away* from activity.
    pub fn note_status_display(
        &self,
        state: crate::panels::agent_pane::state::NeoismAgentStreamingState,
        elapsed_seconds: Option<f32>,
    ) {
        if state == crate::panels::agent_pane::state::NeoismAgentStreamingState::Idle {
            return;
        }
        let now = Instant::now();
        let elapsed_anchor = elapsed_seconds
            .and_then(|seconds| {
                now.checked_sub(Duration::from_secs_f32(seconds.max(0.0)))
            })
            .unwrap_or(now);
        *self.status_hold.borrow_mut() = Some(StatusDisplayHold {
            state,
            elapsed_anchor,
            last_seen: now,
            session: self.viewed_session_id.clone(),
        });
    }

    fn active_status_hold(&self) -> Option<StatusDisplayHold> {
        let hold = self.status_hold.borrow();
        let hold = hold.as_ref()?;
        if hold.session != self.viewed_session_id {
            return None;
        }
        (Instant::now().saturating_duration_since(hold.last_seen) < STATUS_LABEL_GRACE)
            .then(|| hold.clone())
    }

    /// The recently-displayed status, if the raw derivation went idle
    /// less than [`STATUS_LABEL_GRACE`] ago and the viewed session
    /// hasn't changed since it was recorded. `None` once idle has
    /// persisted for the whole window — the label may then clear (and
    /// the status row collapse) as a genuine completion.
    pub fn held_status_display(
        &self,
    ) -> Option<crate::panels::agent_pane::state::NeoismAgentStreamingState> {
        self.active_status_hold().map(|hold| hold.state)
    }

    /// Continuation of the held status's elapsed timer, so the display
    /// clock doesn't jump to zero while the hold bridges two events.
    pub fn held_status_elapsed_seconds(&self) -> Option<f32> {
        self.active_status_hold().map(|hold| {
            Instant::now()
                .saturating_duration_since(hold.elapsed_anchor)
                .as_secs_f32()
        })
    }

    /// Drop the hold immediately — used by hard stops (user abort, new
    /// conversation) where a lingering label would read as lag.
    pub fn clear_status_display_hold(&self) {
        *self.status_hold.borrow_mut() = None;
    }

    /// Test support: age the hold past [`STATUS_LABEL_GRACE`] without
    /// sleeping. Falls back to dropping the hold when the platform
    /// clock can't be rewound that far.
    pub fn rewind_status_display_hold(&self, by: Duration) {
        let mut hold = self.status_hold.borrow_mut();
        if let Some(inner) = hold.as_mut() {
            match inner.last_seen.checked_sub(by) {
                Some(last_seen) => inner.last_seen = last_seen,
                None => *hold = None,
            }
        }
    }

    /// Whether `entry` (a non-main branch) should be hidden from the
    /// sidebar because it finished more than the completion grace ago
    /// (failed children linger longer for inspection) and isn't running again.
    /// The main session (index 0) is never hidden.
    /// A respawned sub-agent reports `Active`/`WaitingPermission`
    /// (which clears `completed_at`) so it stays visible.
    fn subagent_hidden(&self, entry: &NeoismAgentSessionEntry) -> bool {
        // Visibility-only exception: opening an old child must leave a way
        // back to main without reviving that child as an active branch.
        if self.retained_viewed_subagent_id.as_deref() == Some(entry.id.as_str()) {
            return false;
        }
        // Runtime status reported straight on the entry takes
        // precedence — a live respawn keeps the row regardless of any
        // stale activity snapshot.
        if let Some(status) = entry
            .runtime_status
            .as_deref()
            .and_then(BranchStatus::from_runtime_status)
        {
            if matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            ) {
                return false;
            }
        }
        let Some(activity) = self.branch_activities.get(&entry.id) else {
            return false;
        };
        if matches!(
            activity.status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            return false;
        }
        match activity.completed_at {
            // A live completion stamped the window — show it until it
            // elapses, then hide.
            Some(at) => {
                let grace = if activity.status == BranchStatus::Stopped {
                    FAILED_SUBAGENT_HIDE_AFTER
                } else {
                    SUBAGENT_HIDE_AFTER
                };
                Instant::now().saturating_duration_since(at) >= grace
            }
            // Terminal but no window: finished before we were watching (or
            // a re-listed old completion). Hide it immediately.
            None => true,
        }
    }

    /// Drop finished sub-agents whose auto-hide window has elapsed.
    /// Called by the renderer each frame (cheap — usually a no-op) so
    /// the hide fires ~[`SUBAGENT_HIDE_AFTER`] after completion even
    /// without a server refresh, and keeps `self.subagents` aligned
    /// with what's drawn so selection/scroll indices stay valid. The
    /// main session at index 0 is always retained. Returns `true` if
    /// anything was removed (so the caller can mark the frame dirty).
    pub fn prune_expired_completed_subagents(&mut self) -> bool {
        let catalog_changed = self.tick_catalog_families();
        if self.subagents.len() <= 1 {
            return catalog_changed;
        }
        let selected_id = self
            .subagents
            .get(self.selected)
            .map(|entry| entry.id.clone());
        let hidden: Vec<bool> = self
            .subagents
            .iter()
            .enumerate()
            .map(|(ix, entry)| ix != 0 && self.subagent_hidden(entry))
            .collect();
        if !hidden.iter().any(|&h| h) {
            return catalog_changed;
        }
        let mut ix = 0;
        self.subagents.retain(|_| {
            let keep = !hidden[ix];
            ix += 1;
            keep
        });
        // In catalog mode this cursor belongs to the flattened conversation
        // tree, not the lifecycle roster being pruned.
        if matches!(self.mode, SidePanelMode::Subagents) {
            if let Some(index) = selected_id
                .as_deref()
                .and_then(|id| self.subagents.iter().position(|entry| entry.id == id))
            {
                self.selected = index;
            } else if self.selected >= self.subagents.len() {
                self.selected = self.subagents.len().saturating_sub(1);
            }
            if self.scroll_top >= self.subagents.len() {
                self.scroll_top = self.subagents.len().saturating_sub(1);
            }
        }
        true
    }

    #[allow(dead_code)]
    pub fn subagents_loaded(&self) -> bool {
        self.subagents_loaded
    }

    /// Current list — sessions or subagents — keyed off `mode`.
    pub fn active_rows(&self) -> &[NeoismAgentSessionEntry] {
        match self.mode {
            SidePanelMode::Sessions => &self.sessions,
            SidePanelMode::Subagents => &self.subagents,
        }
    }

    /// Only the left catalog owns selectable conversation rows. The detail
    /// rail contains informational sections and no longer takes row focus.
    pub fn focusable(&self) -> bool {
        // A hidden panel is never focusable, whatever rows it last held.
        if self.user_hidden {
            return false;
        }
        match self.mode {
            SidePanelMode::Sessions => {
                self.new_chat_rect.is_some() || !self.sessions.is_empty()
            }
            SidePanelMode::Subagents => false,
        }
    }

    pub fn selected_row(&self) -> Option<&NeoismAgentSessionEntry> {
        self.active_rows().get(self.selected)
    }

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn scroll_top(&self) -> usize {
        self.scroll_top
    }

    pub fn last_panel_height_rows(&self) -> usize {
        self.last_panel_height_rows
    }

    pub fn set_last_panel_height_rows(&mut self, rows: usize) {
        self.last_panel_height_rows = rows.max(1);
    }

    pub fn last_panel_rect(&self) -> Option<[f32; 4]> {
        self.last_panel_rect
    }

    pub fn set_last_panel_rect(&mut self, rect: [f32; 4]) {
        self.last_panel_rect = Some(rect);
    }

    pub fn set_session_search_rect(&mut self, rect: [f32; 4]) {
        self.session_search_rect = (!self.navigation_header_enabled).then_some(rect);
    }

    pub fn clear_session_search_rect(&mut self) {
        self.session_search_rect = None;
    }

    pub fn session_search_contains(&self, x: f32, y: f32) -> bool {
        self.session_search_rect.is_some_and(|[rx, ry, rw, rh]| {
            x >= rx && x <= rx + rw && y >= ry && y <= ry + rh
        })
    }

    pub fn set_row_hit_rect(&mut self, rect: [f32; 4], row_height: f32) {
        self.last_row_hit_rect = Some(rect);
        self.last_row_hit_height = row_height.max(1.0);
        self.last_row_origin_y = rect[1];
    }

    /// Like [`set_row_hit_rect`] but for the chat-mode branch list,
    /// where the visible `rect` is clamped to the viewport while
    /// `origin_y` is the (possibly off-screen) top of the full list —
    /// the anchor the row math counts from.
    pub fn set_row_hit_rect_with_origin(
        &mut self,
        rect: [f32; 4],
        origin_y: f32,
        row_height: f32,
    ) {
        self.last_row_hit_rect = Some(rect);
        self.last_row_hit_height = row_height.max(1.0);
        self.last_row_origin_y = origin_y;
    }

    pub fn clear_row_hit_rect(&mut self) {
        self.last_row_hit_rect = None;
        self.session_search_rect = None;
    }

    /// Clear the cached rect when the pane has gone too narrow to host
    /// the panel. Also drops focus — a panel that isn't on screen
    /// shouldn't keep keyboard focus.
    pub fn clear_last_panel_rect(&mut self) {
        self.reset_visible_running_indicator();
        self.last_panel_rect = None;
        self.last_row_hit_rect = None;
        self.focused = false;
        self.selected_cursor_rect = None;
        self.usage_rect = None;
    }

    pub fn selected_cursor_rect(&self) -> Option<[f32; 4]> {
        self.selected_cursor_rect
    }

    pub fn set_selected_cursor_rect(&mut self, rect: [f32; 4]) {
        self.selected_cursor_rect = Some(rect);
    }

    pub fn clear_selected_cursor_rect(&mut self) {
        self.selected_cursor_rect = None;
    }

    pub fn contains_point(&self, x: f32, y: f32) -> bool {
        let Some([px, py, pw, ph]) = self.last_panel_rect else {
            return false;
        };
        x >= px && x <= px + pw && y >= py && y <= py + ph
    }

    /// Kept for the home-mode click path that always wants a session
    /// (not a subagent). Subagent click uses `selected_row()` instead.
    pub fn selected_session(&self) -> Option<&NeoismAgentSessionEntry> {
        if self.search_focused() || self.new_chat_selected() || self.selected_header_action().is_some() {
            return None;
        }
        self.sessions.get(self.selected).filter(|e| !e.is_header)
    }

    fn selected_catalog_row(&self) -> Option<&NeoismAgentSessionEntry> {
        if !matches!(self.mode, SidePanelMode::Sessions)
            || self.search_focused()
            || self.new_chat_selected()
            || self.selected_header_action().is_some()
        {
            return None;
        }
        self.sessions
            .get(self.selected)
            .filter(|entry| entry.is_selectable() && !entry.is_project_page())
    }

    /// Replace the flat catalog; display rows are derived by project, with pins
    /// first within each project. Only home-mode selection/scroll is affected.
    pub fn set_sessions(&mut self, sessions: Vec<NeoismAgentSessionEntry>) {
        self.set_session_page(sessions, None, None);
    }

    /// Update the left catalog in place, independently of the chat/detail pane.
    pub fn set_session_activity(
        &mut self,
        session_id: &str,
        activity: neoism_agent_core::CatalogActivity,
    ) -> bool {
        let mut changed = self
            .session_activity_overrides
            .insert(session_id.to_owned(), activity)
            != Some(activity);
        for entry in self.all_sessions.iter_mut().chain(self.sessions.iter_mut()) {
            if entry.id == session_id && entry.catalog_activity != Some(activity) {
                entry.catalog_activity = Some(activity);
                changed = true;
            }
        }
        changed
    }

    pub fn clear_session_activity_overrides(&mut self) {
        self.session_activity_overrides.clear();
    }

    fn apply_session_activity_override(&self, entry: &mut NeoismAgentSessionEntry) {
        if let Some(activity) = self.session_activity_overrides.get(&entry.id) {
            entry.catalog_activity = Some(*activity);
        }
    }

    /// Apply one live catalogue mutation without discarding continuation pages
    /// or moving the user's selection to a different session after re-sorting.
    pub fn upsert_session(&mut self, session: NeoismAgentSessionEntry) {
        self.apply_session_mutations(std::iter::once(CatalogSessionMutation::Upsert(
            session,
        )));
    }

    pub fn catalog_session(&self, id: &str) -> Option<&NeoismAgentSessionEntry> {
        self.all_sessions
            .iter()
            .find(|entry| entry.id == id && self.in_catalog_scope(entry))
    }

    /// Search snapshots may discover chats, but never replace newer live metadata.
    pub fn admit_search_sessions(
        &mut self,
        sessions: impl IntoIterator<Item = NeoismAgentSessionEntry>,
    ) {
        let sessions = sessions
            .into_iter()
            .filter(|session| {
                self.in_catalog_scope(session)
                    && !self.deleted_session_ids.contains(&session.id)
                    && !self
                        .all_sessions
                        .iter()
                        .any(|existing| existing.id == session.id)
            })
            .collect::<Vec<_>>();
        let search_ids: Vec<_> = sessions.iter().map(|entry| entry.id.clone()).collect();
        let discovered: HashSet<_> = self.project_pages.keys().cloned().collect();
        self.upsert_sessions(sessions);
        self.project_pages
            .retain(|directory, _| discovered.contains(directory));
        self.search_only_session_ids.extend(search_ids);
        if self.project_pagination_enabled() && self.session_query.trim().is_empty() {
            self.rebuild_session_display();
        }
    }

    /// Admit search/catalog batches with one grouping rebuild, not one per hit.
    pub fn upsert_sessions(
        &mut self,
        sessions: impl IntoIterator<Item = NeoismAgentSessionEntry>,
    ) {
        self.apply_session_mutations(
            sessions.into_iter().map(CatalogSessionMutation::Upsert),
        );
    }

    /// Apply a drained event batch in arrival order, with indexed lookups and one
    /// display rebuild. Deletes also fence stale snapshots; a later live upsert
    /// clears that fence. Selection and the viewport follow stable identities.
    pub fn apply_session_mutations(
        &mut self,
        mutations: impl IntoIterator<Item = CatalogSessionMutation>,
    ) {
        let mut mutations = mutations.into_iter().peekable();
        if mutations.peek().is_none() {
            return;
        }
        let selected_id = self
            .selected_catalog_row()
            .map(|entry| entry.stable_identity().to_owned());
        let anchor = self.session_viewport_anchor();
        let mut rows: Vec<_> = std::mem::take(&mut self.all_sessions)
            .into_iter()
            .map(Some)
            .collect();
        let mut indices: HashMap<_, _> = rows
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.as_ref().unwrap().id.clone(), index))
            .collect();
        let mut admitted = false;
        for mutation in mutations {
            let mut session = match mutation {
                CatalogSessionMutation::Upsert(session)
                    if self.in_catalog_scope(&session) =>
                {
                    session
                }
                CatalogSessionMutation::Upsert(session) => {
                    if let Some(index) = indices.remove(&session.id) {
                        rows[index] = None;
                        self.remove_session_metadata(&session.id);
                    }
                    continue;
                }
                CatalogSessionMutation::Delete(id) => {
                    if let Some(index) = indices.remove(&id) {
                        rows[index] = None;
                    }
                    self.remove_session_metadata(&id);
                    continue;
                }
            };
            admitted = true;
            self.deleted_session_ids.remove(&session.id);
            self.search_only_session_ids.remove(&session.id);
            if self.project_pagination_enabled() {
                let directory = session.directory.clone().unwrap_or_default();
                let page = self.project_pages.entry(directory).or_default();
                page.updated_ms = page.updated_ms.max(session.updated_ms);
            }
            self.apply_session_activity_override(&mut session);
            if let Some(index) = indices.get(&session.id).copied() {
                let existing = rows[index].as_mut().unwrap();
                if session.catalog_activity.is_none() {
                    session.catalog_activity = existing.catalog_activity;
                }
                // Metadata-only rename/title events are not an idle edge.
                if session.runtime_status.is_none()
                    && !self.session_activity_overrides.contains_key(&session.id)
                {
                    session.runtime_status.clone_from(&existing.runtime_status);
                }
                *existing = session;
            } else {
                indices.insert(session.id.clone(), rows.len());
                rows.push(Some(session));
            }
        }
        self.all_sessions = rows.into_iter().flatten().collect();
        if admitted {
            self.session_catalog_state = SessionCatalogState::Ready;
        }
        self.rebuild_session_display();
        self.restore_selected_session(selected_id.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    /// Merge an authoritative newest page after the catalogue stream has
    /// subscribed. A live create can race the HTTP response; merging prevents
    /// that newer row from being erased by a snapshot taken just before it.
    pub fn reconcile_session_head(
        &mut self,
        sessions: Vec<NeoismAgentSessionEntry>,
        next_cursor: Option<String>,
    ) {
        let selected_id = self
            .selected_catalog_row()
            .map(|entry| entry.stable_identity().to_owned());
        let viewport_anchor = self.session_viewport_anchor();
        for mut session in sessions {
            if !self.in_catalog_scope(&session) {
                continue;
            }
            self.apply_session_activity_override(&mut session);
            if let Some(existing) = self
                .all_sessions
                .iter_mut()
                .find(|existing| existing.id == session.id)
            {
                *existing = session;
            } else {
                self.all_sessions.push(session);
            }
        }
        self.session_next_cursor =
            next_cursor.or_else(|| self.session_next_cursor.take());
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        self.session_refresh_attempts = 0;
        self.session_catalog_state = SessionCatalogState::Ready;
        self.rebuild_session_display();
        self.restore_selected_session(selected_id.as_deref());
        self.restore_session_viewport_anchor(viewport_anchor);
    }

    pub fn remove_session(&mut self, session_id: &str) {
        self.apply_session_mutations(std::iter::once(CatalogSessionMutation::Delete(
            session_id.to_owned(),
        )));
    }

    fn remove_session_metadata(&mut self, session_id: &str) {
        self.search_only_session_ids.remove(session_id);
        self.deleted_session_ids.insert(session_id.to_owned());
        self.session_activity_overrides.remove(session_id);
        self.catalog_subagents.remove(session_id);
        self.catalog_families.remove(session_id);
        for family in self.catalog_families.values_mut() {
            family.rows.retain(|entry| entry.id != session_id);
            family.activities.remove(session_id);
            family.terminal_revisions.remove(session_id);
        }
        for children in self.catalog_subagents.values_mut() {
            children.retain(|entry| entry.id != session_id);
        }
        self.semantic_results
            .retain(|entry| entry.session_id != session_id);
    }

    fn restore_selected_session(&mut self, session_identity: Option<&str>) {
        let Some(session_identity) = session_identity else {
            return;
        };
        if let Some(index) = self.sessions.iter().position(|entry| {
            entry.is_selectable()
                && (entry.id == session_identity
                    || entry.stable_identity() == session_identity)
        }) {
            self.selected = index;
        }
    }

    fn session_viewport_anchor(&self) -> Option<(String, f32)> {
        let row_height = self.row_height().max(1.0);
        let top = (self.scroll_px / row_height).floor() as usize;
        self.sessions
            .iter()
            .enumerate()
            .skip(top)
            .find(|(_, entry)| entry.is_selectable() && !entry.is_excerpt)
            .map(|(index, entry)| {
                (
                    entry.stable_identity().to_owned(),
                    self.scroll_px - index as f32 * row_height,
                )
            })
    }

    fn restore_session_viewport_anchor(&mut self, anchor: Option<(String, f32)>) {
        // Initial/global-pin hydration must not scroll past the new top section.
        if self.catalog_directory.is_none()
            && self.scroll_px <= f32::EPSILON
            && self
                .sessions
                .first()
                .is_some_and(|row| row.id == "section:global:Pinned")
        {
            self.scroll_top = 0;
            self.scroll.set_target_immediate(0.0);
            return;
        }
        let Some((identity, offset)) = anchor else {
            return;
        };
        let Some(index) = self.sessions.iter().position(|entry| {
            entry.is_selectable()
                && !entry.is_excerpt
                && entry.stable_identity() == identity
        }) else {
            return;
        };
        let row_height = self.row_height().max(1.0);
        let position = (index as f32 * row_height + offset)
            .clamp(0.0, self.max_scroll_px(self.last_panel_height_rows));
        self.scroll_px = position;
        self.scroll_top = (position / row_height).floor() as usize;
        self.scroll.set_target_immediate(position);
    }

    pub fn external_errors(&self) -> &[(ConversationSource, String)] {
        &self.external_errors
    }
    pub fn external_scanning(&self) -> bool {
        self.external_scanning
    }
    pub fn set_external_scanning(&mut self, scanning: bool) {
        self.external_scanning = scanning;
    }

    pub fn set_external_importing(&mut self, key: Option<String>) {
        self.external_importing_key = key;
        self.rebuild_session_display();
    }

    pub fn set_external_notice(&mut self, source: ConversationSource, message: String) {
        self.external_errors
            .retain(|(provider, _)| *provider != source);
        self.external_errors.push((source, message));
    }

    pub fn set_external_provider_error(
        &mut self,
        source: ConversationSource,
        error: String,
    ) {
        self.external_errors
            .retain(|(provider, _)| *provider != source);
        self.external_errors.push((source, error));
        // Retain the last good previews when a refresh or adapter fails.
        // Server/directory switches create a fresh pane rather than reusing these rows.
    }

    /// Provider previews are independent of the paged Neoism catalog. A
    /// first-page refresh must never discard them or reset the scroll.
    pub fn set_external_provider_rows(
        &mut self,
        source: ConversationSource,
        rows: Vec<NeoismAgentSessionEntry>,
    ) {
        let selected_id = self
            .selected_catalog_row()
            .map(|entry| entry.stable_identity().to_owned());
        let viewport_anchor = self.session_viewport_anchor();
        self.external_errors
            .retain(|(provider, _)| *provider != source);
        self.external_sessions
            .retain(|entry| entry.source != source);
        self.external_sessions.extend(
            rows.into_iter()
                .filter(|entry| entry.external_preview.is_some()),
        );
        self.rebuild_session_display();
        self.restore_selected_session(selected_id.as_deref());
        self.restore_session_viewport_anchor(viewport_anchor);
    }

    pub fn mark_external_imported(&mut self, source_key: &str, id: &str) {
        let viewport_anchor = self.session_viewport_anchor();
        if let Some(entry) = self
            .external_sessions
            .iter_mut()
            .find(|entry| entry.source_key.as_deref() == Some(source_key))
        {
            entry.id = id.to_string();
            if let Some(preview) = entry.external_preview.as_mut() {
                preview.neoism_session_id = Some(id.to_string());
                preview.history_state = "text_only".into();
            }
        }
        self.rebuild_session_display();
        self.restore_selected_session(Some(source_key));
        self.restore_session_viewport_anchor(viewport_anchor);
    }

    /// Apply a catalogue page. First pages replace and reset the home list;
    /// continuation pages append by id while preserving the viewport.
    pub fn set_session_page(
        &mut self,
        mut sessions: Vec<NeoismAgentSessionEntry>,
        requested_cursor: Option<&str>,
        next_cursor: Option<String>,
    ) {
        sessions.retain(|session| self.in_catalog_scope(session));
        for session in &mut sessions {
            self.apply_session_activity_override(session);
        }
        let was_home = matches!(self.mode, SidePanelMode::Sessions);
        let catalog_was_empty =
            self.all_sessions.is_empty() && self.external_sessions.is_empty();
        let selected_id = self
            .selected_catalog_row()
            .map(|entry| entry.stable_identity().to_owned());
        let viewport_anchor = self.session_viewport_anchor();
        if requested_cursor.is_some() {
            let mut known = self
                .all_sessions
                .iter()
                .map(|session| session.id.clone())
                .collect::<HashSet<_>>();
            self.all_sessions.extend(
                sessions
                    .into_iter()
                    .filter(|session| known.insert(session.id.clone())),
            );
        } else {
            self.all_sessions = sessions;
        }
        self.session_next_cursor = next_cursor;
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        self.session_refresh_attempts = 0;
        self.session_catalog_state = SessionCatalogState::Ready;
        self.rebuild_session_display();
        self.restore_selected_session(selected_id.as_deref());
        if was_home && requested_cursor.is_none() && catalog_was_empty {
            self.scroll_px = 0.0;
            self.scroll.reset();
            self.cursor_spring.reset();
        } else {
            self.restore_session_viewport_anchor(viewport_anchor);
        }
    }

    pub fn session_next_cursor(&self) -> Option<&str> {
        self.session_next_cursor.as_deref()
    }

    pub fn session_requested_cursor(&self) -> Option<&str> {
        self.session_requested_cursor.as_deref()
    }

    pub fn session_page_loading(&self) -> bool {
        self.session_page_loading
    }

    pub fn next_session_request_generation(&mut self) -> u64 {
        self.session_request_generation =
            self.session_request_generation.saturating_add(1);
        self.session_request_generation
    }

    pub fn session_request_is_current(&self, generation: u64) -> bool {
        self.session_request_generation == generation
    }

    /// Claim the continuation cursor once the viewport is within four rows
    /// of the loaded end. The claim is single-flight until a page or failure
    /// settles it.
    pub fn begin_session_page_near_end(&mut self) -> Option<String> {
        if self.project_pagination_enabled()
            || self.session_page_loading
            || !matches!(self.mode, SidePanelMode::Sessions)
            || !self.session_query.trim().is_empty()
            || self
                .scroll_top
                .saturating_add(self.last_panel_height_rows + 4)
                < self.active_len()
        {
            return None;
        }
        let cursor = self.session_next_cursor.clone()?;
        self.session_page_loading = true;
        self.session_requested_cursor = Some(cursor.clone());
        self.last_sessions_refresh = Some(Instant::now());
        self.sessions_loading_started = Instant::now();
        Some(cursor)
    }

    pub fn settle_session_page_error(&mut self, message: impl Into<String>) {
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        if self.all_sessions.is_empty() {
            self.last_sessions_refresh = Some(Instant::now());
            self.session_refresh_attempts = self.session_refresh_attempts.max(1);
            self.session_catalog_state = SessionCatalogState::Error(message.into());
        }
    }

    /// The live home-mode search filter.
    pub fn session_query(&self) -> &str {
        &self.session_query
    }

    /// Replace the home-mode search filter and rebuild the display list.
    pub fn set_session_query(&mut self, query: String) {
        if self.session_query == query {
            return;
        }
        self.session_query = query;
        self.rebuild_session_display();
        self.selected = self.nearest_selectable(0).unwrap_or(0);
        self.scroll_top = 0;
        self.scroll_px = 0.0;
        self.scroll.reset();
        self.cursor_spring.reset();
    }

    /// Append typed text to the home-mode search filter. Typing moves the
    /// cursor onto the search row.
    pub fn push_session_query(&mut self, text: &str) {
        self.search_focused = true;
        let mut query = self.session_query.clone();
        query.push_str(text);
        self.set_session_query(query);
    }

    /// Delete the last character of the home-mode search filter.
    pub fn backspace_session_query(&mut self) {
        let mut query = self.session_query.clone();
        if query.pop().is_some() {
            self.set_session_query(query);
        }
    }

    /// Clear the home-mode search filter.
    pub fn clear_session_query(&mut self) {
        if !self.session_query.is_empty() {
            self.set_session_query(String::new());
        }
    }

    /// Install semantic transcript-search results (best chunk per session,
    /// closest-first) fetched for `query`, and fold them into the display
    /// list: matched sessions join the title filter and each shows its
    /// matched excerpt chunk beneath the row. A result set for a query the
    /// user has already typed past is stored but simply won't apply until
    /// queries line up again.
    pub fn set_semantic_results(
        &mut self,
        query: String,
        results: Vec<NeoismAgentSemanticMatch>,
    ) {
        let selected = self
            .selected_catalog_row()
            .map(|row| row.stable_identity().to_owned());
        let anchor = self.session_viewport_anchor();
        self.semantic_query = query.trim().to_string();
        self.semantic_results = results;
        self.result_wrap_metrics = None;
        self.measured_excerpt_lines.clear();
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    /// Mark a semantic transcript search as started/finished so the
    /// empty state can shimmer instead of declaring "No results" while
    /// hits may still arrive.
    pub fn set_semantic_searching(&mut self, searching: bool) {
        if searching && !self.semantic_searching {
            self.semantic_search_started = Some(Instant::now());
        }
        if !searching {
            self.semantic_search_started = None;
        }
        self.semantic_searching = searching;
    }

    pub fn semantic_searching(&self) -> bool {
        self.semantic_searching
    }

    /// Feed the renderer's measured monospace column budget back so excerpt
    /// chunks wrap to the actual panel width. Rebuilds the display list only
    /// when the budget actually changes (a resize), so calling this every
    /// frame is free.
    pub fn set_result_wrap_columns(&mut self, columns: usize) {
        let columns = columns.clamp(16, 160);
        if columns != self.result_wrap_columns {
            self.result_wrap_columns = columns;
            self.rebuild_session_display();
        }
    }

    /// Reflow proportional-font excerpts only when their content or typography changes.
    pub fn set_result_wrap_metrics(
        &mut self,
        width: f32,
        font_size: f32,
        font_id: Option<usize>,
        scale: f32,
        mut measure: impl FnMut(&str) -> f32,
    ) {
        let width = width.max(1.0);
        let key = (
            width.to_bits(),
            font_size.to_bits(),
            font_id,
            scale.to_bits(),
        );
        if self.result_wrap_metrics == Some(key) {
            return;
        }
        let selected = self
            .selected_catalog_row()
            .map(|row| row.stable_identity().to_owned());
        let anchor = self.session_viewport_anchor();
        self.result_wrap_metrics = Some(key);
        self.measured_excerpt_lines = self
            .semantic_results
            .iter()
            .map(|hit| {
                (
                    hit.session_id.clone(),
                    wrap_excerpt_measured(
                        &compact_excerpt(&hit.excerpt),
                        width,
                        3,
                        &mut measure,
                    ),
                )
            })
            .collect();
        self.rebuild_session_display();
        self.restore_selected_session(selected.as_deref());
        self.restore_session_viewport_anchor(anchor);
    }

    /// Seconds since the in-flight semantic search began.
    pub fn semantic_search_elapsed(&self) -> f32 {
        self.semantic_search_started
            .map(|started| {
                Instant::now()
                    .saturating_duration_since(started)
                    .as_secs_f32()
            })
            .unwrap_or(0.0)
    }

    pub fn project_expanded(&self, directory: &str) -> bool {
        !self.session_query.trim().is_empty()
            || !self.collapsed_projects.contains(directory)
    }

    /// Folder rows are keyboard-selectable, but never session activation targets.
    pub fn toggle_selected_project(&mut self) -> bool {
        if !matches!(self.mode, SidePanelMode::Sessions)
            || self.search_focused()
            || self.new_chat_selected()
            || self.selected_header_action().is_some()
        {
            return false;
        }
        if self.activate_selected_project_page() {
            return true;
        }
        let Some(directory) = self
            .sessions
            .get(self.selected)
            .filter(|entry| entry.is_project())
            .and_then(|entry| entry.directory.clone())
        else {
            return false;
        };
        if !self.collapsed_projects.remove(&directory) {
            self.collapsed_projects.insert(directory.clone());
        }
        self.rebuild_session_display();
        if let Some(index) = self.sessions.iter().position(|entry| {
            entry.is_project() && entry.directory.as_deref() == Some(directory.as_str())
        }) {
            self.move_selection_to(index);
        }
        true
    }

    pub fn navigate_project(&mut self, expand: bool) -> bool {
        if !matches!(self.mode, SidePanelMode::Sessions)
            || self.search_focused()
            || self.new_chat_selected()
            || self.selected_header_action().is_some()
        {
            return false;
        }
        let Some(entry) = self.sessions.get(self.selected) else {
            return false;
        };
        let Some(directory) = entry.directory.clone().or_else(|| {
            (self.project_pagination_enabled() && self.project_pages.contains_key(""))
                .then(String::new)
        }) else {
            return false;
        };
        if entry.is_project() {
            if self.project_expanded(&directory) != expand {
                self.toggle_selected_project();
            } else if expand {
                self.select_next();
            }
        } else if !expand {
            if let Some(index) = self.sessions.iter().position(|entry| {
                entry.is_project()
                    && entry.directory.as_deref() == Some(directory.as_str())
            }) {
                self.move_selection_to(index);
            }
        }
        true
    }

    /// Filter the catalog and group exact saved directories by recent activity.
    /// Pins remain within their project; search temporarily reveals collapsed groups.
    fn rebuild_session_display(&mut self) {
        let needle = self.session_query.trim().to_lowercase();
        // Search terms for excerpt highlighting: whitespace-split words of
        // two characters or more, already lowercased with the needle.
        let terms: Vec<String> = needle
            .split_whitespace()
            .filter(|word| word.len() >= 2)
            .map(str::to_string)
            .collect();
        let semantic_active =
            !needle.is_empty() && self.semantic_query == self.session_query.trim();
        let semantic: std::collections::HashMap<&str, &NeoismAgentSemanticMatch> =
            if semantic_active {
                self.semantic_results
                    .iter()
                    .map(|hit| (hit.session_id.as_str(), hit))
                    .collect()
            } else {
                std::collections::HashMap::new()
            };
        let mut visible: Vec<NeoismAgentSessionEntry> = self
            .all_sessions
            .iter()
            .chain(self.external_sessions.iter().filter(|entry| {
                if entry.source_key == self.external_importing_key
                    && self.external_importing_key.is_some()
                {
                    return false;
                }
                !self.all_sessions.iter().any(|stored| {
                    entry
                        .external_preview
                        .as_ref()
                        .and_then(|preview| preview.neoism_session_id.as_deref())
                        == Some(stored.id.as_str())
                        || (entry.source_key.is_some()
                            && entry.source_key == stored.source_key)
                })
            }))
            .filter(|entry| self.in_catalog_scope(entry))
            .filter(|entry| {
                !self.project_pagination_enabled()
                    || !needle.is_empty()
                    || !self.search_only_session_ids.contains(&entry.id)
            })
            .filter(|entry| {
                needle.is_empty()
                    || entry.title.to_lowercase().contains(&needle)
                    || (self.catalog_directory.is_none()
                        && entry.directory.as_ref().is_some_and(|directory| {
                            directory.to_lowercase().contains(&needle)
                        }))
                    || (entry.external_preview.is_some()
                        && entry.source.label().to_lowercase().contains(&needle))
                    || semantic.contains_key(entry.id.as_str())
            })
            .cloned()
            .collect();
        let mut groups: HashMap<Option<String>, Vec<NeoismAgentSessionEntry>> =
            HashMap::new();
        for entry in visible.drain(..) {
            let directory = if entry.directory.is_none()
                && self.project_pagination_enabled()
                && self.project_pages.contains_key("")
            {
                Some(String::new())
            } else {
                entry.directory.clone()
            };
            groups.entry(directory).or_default().push(entry);
        }
        let grouped_paging = self.project_pagination_enabled() && needle.is_empty();
        if grouped_paging {
            groups.retain(|directory, entries| {
                directory.is_none()
                    || directory
                        .as_ref()
                        .is_some_and(|d| self.project_pages.contains_key(d))
                    || entries.iter().any(|entry| entry.external_preview.is_some())
            });
            for directory in self.project_pages.keys() {
                groups.entry(Some(directory.clone())).or_default();
            }
        }
        let project_name = |directory: &str| {
            if directory.is_empty() {
                return "Other chats".to_owned();
            }
            let trimmed = directory.trim_end_matches(['/', '\\']);
            trimmed
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(directory)
                .to_string()
        };
        let mut name_counts: HashMap<String, usize> = HashMap::new();
        for directory in groups.keys().flatten() {
            *name_counts.entry(project_name(directory)).or_default() += 1;
        }
        let dashboard = self
            .catalog_directory
            .is_none()
            .then_some(self.dashboard_directory.as_deref())
            .flatten();
        let mut groups: Vec<_> = groups.into_iter().collect();
        groups.sort_by(|(a_dir, a), (b_dir, b)| {
            let is_dashboard = |directory: &Option<String>| {
                dashboard.is_some() && directory.as_deref() == dashboard
            };
            let latest = |entries: &[NeoismAgentSessionEntry]| {
                entries
                    .iter()
                    .map(|entry| entry.updated_ms)
                    .max()
                    .unwrap_or(0)
            };
            let project_latest =
                |directory: &Option<String>, entries: &[NeoismAgentSessionEntry]| {
                    latest(entries).max(if grouped_paging {
                        directory
                            .as_ref()
                            .and_then(|d| self.project_pages.get(d))
                            .map_or(0, |p| p.updated_ms)
                    } else {
                        0
                    })
                };
            is_dashboard(b_dir)
                .cmp(&is_dashboard(a_dir))
                .then_with(|| project_latest(b_dir, b).cmp(&project_latest(a_dir, a)))
                .then(a_dir.cmp(b_dir))
        });
        let scoped = self.catalog_directory.is_some();
        let mut out = Vec::new();
        if !scoped && needle.is_empty() {
            let mut pinned: Vec<_> = groups
                .iter()
                .flat_map(|(_, entries)| entries.iter())
                .filter(|entry| entry.pinned)
                .cloned()
                .collect();
            pinned.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then(a.id.cmp(&b.id)));
            if !pinned.is_empty() {
                let mut header = NeoismAgentSessionEntry::header("Pinned");
                header.id = "section:global:Pinned".into();
                out.push(header);
                for mut entry in pinned {
                    entry.depth = 0;
                    out.push(entry);
                }
            }
        }
        for (directory, mut entries) in groups {
            if !scoped {
                let label = directory
                    .as_deref()
                    .map(|directory| {
                        if dashboard == Some(directory) {
                            return "~".to_owned();
                        }
                        let name = project_name(directory);
                        if name_counts.get(&name).copied().unwrap_or(0) > 1 {
                            directory.to_string()
                        } else {
                            name
                        }
                    })
                    .unwrap_or_else(|| "Other chats".to_string());
                let mut header = NeoismAgentSessionEntry::header(label)
                    .with_directory(directory.clone());
                if grouped_paging {
                    header.directory = directory.clone();
                }
                header.time_label = format!(
                    "{}{}",
                    entries.len(),
                    if self.session_next_cursor.is_some() {
                        "+"
                    } else {
                        ""
                    }
                );
                if grouped_paging {
                    // Discovery supplies no total count; do not invent one from loaded pages.
                    header.time_label.clear();
                }
                if let Some(directory) = &directory {
                    header.id = format!("project:{directory}");
                }
                out.push(header);
                if directory
                    .as_deref()
                    .is_some_and(|directory| !self.project_expanded(directory))
                {
                    continue;
                }
            }
            entries.sort_by(|a, b| {
                b.pinned
                    .cmp(&a.pinned)
                    .then(b.updated_ms.cmp(&a.updated_ms))
                    .then(a.id.cmp(&b.id))
            });
            if !scoped && needle.is_empty() {
                entries.retain(|entry| !entry.pinned);
            }
            let mut previous_section = None;
            for mut entry in entries {
                let depth = usize::from(!scoped);
                if scoped && needle.is_empty() && previous_section != Some(entry.pinned) {
                    let title = if entry.pinned { "Pinned" } else { "Recent" };
                    let mut section = NeoismAgentSessionEntry::header(title);
                    section.id = format!(
                        "section:{}:{title}",
                        directory.as_deref().unwrap_or_default()
                    );
                    out.push(section);
                    previous_section = Some(entry.pinned);
                }
                entry.depth = depth;
                let hit = semantic.get(entry.id.as_str()).copied();
                let id = entry.id.clone();
                out.push(entry);
                if let Some(hit) = hit {
                    let excerpt = compact_excerpt(&hit.excerpt);
                    let lines = self
                        .measured_excerpt_lines
                        .get(&id)
                        .cloned()
                        .unwrap_or_else(|| {
                            wrap_excerpt_lines(&excerpt, self.result_wrap_columns, 3)
                        });
                    for line in lines {
                        let mut row = NeoismAgentSessionEntry::excerpt(id.clone(), line)
                            .with_directory(directory.clone())
                            .with_depth(depth);
                        row.highlights = term_highlight_ranges(&row.title, &terms);
                        out.push(row);
                    }
                }
            }
            if grouped_paging {
                if let Some(row) =
                    directory.as_deref().and_then(|d| self.project_page_row(d))
                {
                    out.push(row);
                }
            }
        }
        // The same flattened rows drive painting, pointer hits and keyboard stops.
        let mut tree = Vec::with_capacity(out.len());
        for entry in out {
            let children = (!entry.is_header && !entry.is_excerpt)
                .then(|| self.catalog_subagents.get(&entry.id))
                .flatten();
            let directory = entry.directory.clone();
            let depth = entry.depth;
            tree.push(entry);
            if let Some(children) = children {
                for child in children {
                    let mut child = child.clone();
                    if child.directory.is_none() {
                        child.directory.clone_from(&directory);
                    }
                    child.depth = depth + child.depth.max(1);
                    tree.push(child);
                }
            }
        }
        self.sessions = tree;

        // Selection / scroll indices only refer to the sessions list in
        // home mode; in chat (subagent) mode they belong to `subagents`,
        // so leave them untouched there.
        if matches!(self.mode, SidePanelMode::Sessions) {
            if self.selected >= self.sessions.len() {
                self.selected = self.sessions.len().saturating_sub(1);
            }
            self.snap_selection_to_selectable();
            if self.scroll_top >= self.sessions.len() {
                self.scroll_top = self.sessions.len().saturating_sub(1);
            }
        }
    }

    pub fn set_subagents(&mut self, mut subagents: Vec<NeoismAgentSessionEntry>) {
        // The roster can arrive after navigation. Once its root is known, do
        // not retain root lifecycle data under the viewed-child exception.
        self.retained_viewed_subagent_id = self
            .viewed_session_id
            .as_ref()
            .filter(|id| !subagents.first().is_some_and(|root| root.id == id.as_str()))
            .cloned();
        let was_subagents = matches!(self.mode, SidePanelMode::Subagents);
        // Recovery snapshots can briefly know only that a child is a generic
        // "subagent", while live task/status events already supplied its real
        // title and agent. Refresh runtime/depth from the snapshot, but never let
        // placeholder metadata downgrade a row the user has already seen.
        for incoming in &mut subagents {
            if let Some(previous) = self
                .subagents
                .iter()
                .find(|previous| previous.id == incoming.id)
            {
                preserve_specific_subagent_metadata(incoming, previous);
            } else if incoming.runtime_status.is_none() {
                // On first bootstrap, children absent from the active-run map
                // are historical/idle. Subsequent recovery snapshots retain
                // the event-driven state above instead of repeating this
                // inference and flickering an active child off.
                incoming.runtime_status = Some("completed".to_string());
            }
        }
        // A branch discovered by the live event stream is newer than a
        // recovery snapshot that started before that event. If such a
        // snapshot omits an agent that is still authoritatively active, retain
        // its last known row. This
        // keeps both the sidebar and the footer's active count stable until a
        // real terminal lifecycle event arrives.
        let incoming_ids = subagents
            .iter()
            .map(|entry| entry.id.clone())
            .collect::<HashSet<_>>();
        let missing_active =
            self.subagents
                .iter()
                .enumerate()
                .filter(|(index, entry)| {
                    *index > 0
                        && !incoming_ids.contains(&entry.id)
                        && (self.branch_activities.get(&entry.id).is_some_and(
                            |activity| {
                                matches!(
                                    activity.status,
                                    BranchStatus::Active
                                        | BranchStatus::WaitingPermission
                                )
                            },
                        ) || self.retained_viewed_subagent_id.as_deref()
                            == Some(entry.id.as_str()))
                })
                .map(|(_, entry)| entry.clone())
                .collect::<Vec<_>>();
        subagents.extend(missing_active);
        // Capture the id of the row the cursor was on so it survives a
        // re-fetch. The subagent list is rebuilt wholesale on every
        // refresh (and after a session switch invalidates it), so a raw
        // index would silently re-point the highlight at a *different*
        // branch whenever the list reorders or grows — the "switching
        // between sub agents seems odd" report. Preserve by id instead,
        // mirroring how the picker keeps its selection across filters.
        let previously_selected_id = self
            .subagents
            .get(self.selected)
            .map(|entry| entry.id.clone());
        // Whether the set of rows actually changed. A status-only recovery
        // snapshot (same ids) must not reset scroll/selection springs.
        let ids_changed = self.subagents.len() != subagents.len()
            || self
                .subagents
                .iter()
                .zip(subagents.iter())
                .any(|(old, new)| old.id != new.id);
        self.subagents = subagents;
        self.subagents_loaded = true;
        // Reconcile explicit statuses from a bootstrap/reconnect snapshot.
        // Collect first to avoid borrowing `self.subagents` while mutating
        // `self.branch_activities`.
        let reconciled = self
            .subagents
            .iter()
            .filter_map(|entry| {
                entry
                    .runtime_status
                    .as_deref()
                    .and_then(BranchStatus::from_runtime_status)
                    .map(|status| (entry.id.clone(), status))
            })
            .collect::<Vec<_>>();
        for (id, status) in reconciled {
            self.reconcile_snapshot_branch_activity_status(id, status);
        }
        if was_subagents {
            // Restore the cursor onto the same logical branch it was on
            // before the refresh, falling back to a clamped index when
            // that branch is gone (e.g. a completed subagent dropped out
            // of the list).
            if let Some(index) = previously_selected_id
                .as_deref()
                .and_then(|id| self.subagents.iter().position(|entry| entry.id == id))
            {
                self.selected = index;
            } else if self.selected >= self.subagents.len() {
                self.selected = self.subagents.len().saturating_sub(1);
            }
            if self.scroll_top >= self.subagents.len() {
                self.scroll_top = self.subagents.len().saturating_sub(1);
            }
            if ids_changed {
                self.scroll.reset();
                self.cursor_spring.reset();
            }
        }
        // Drop finished sub-agents the backend re-listed (the /children
        // endpoint always returns completed children) before they can paint
        // for even one frame — this is what stopped already-hidden rows from
        // flashing back when a new sub-agent completes.
        self.prune_expired_completed_subagents();
    }

    pub fn ensure_subagent_main_entry(&mut self, id: impl Into<String>) {
        let id = id.into();
        if id.is_empty() || self.subagents.first().is_some_and(|entry| entry.id == id) {
            return;
        }
        self.subagents.insert(
            0,
            NeoismAgentSessionEntry::new(id, "main session", "return"),
        );
        self.subagents_loaded = true;
    }

    pub fn upsert_subagent(
        &mut self,
        id: impl Into<String>,
        title: impl Into<String>,
        time_label: impl Into<String>,
    ) -> bool {
        let id = id.into();
        if id.is_empty() {
            return false;
        }
        let title = title.into();
        let time_label = time_label.into();
        if let Some(entry) = self.subagents.iter_mut().find(|entry| entry.id == id) {
            if !title.trim().is_empty()
                && (!generic_subagent_metadata(&title)
                    || generic_subagent_metadata(&entry.title))
            {
                entry.title = title;
            }
            if !time_label.trim().is_empty()
                && (!generic_subagent_metadata(&time_label)
                    || generic_subagent_metadata(&entry.time_label))
            {
                entry.time_label = time_label;
                entry.agent_kind =
                    AgentKind::from_label(&entry.time_label).or(entry.agent_kind);
            }
            return false;
        }
        let time_label = if time_label.trim().is_empty() {
            "subagent".to_string()
        } else {
            time_label
        };
        let entry = NeoismAgentSessionEntry::new(
            id,
            if title.trim().is_empty() {
                "subagent".to_string()
            } else {
                title
            },
            time_label.clone(),
        )
        .with_depth(1)
        .with_agent_kind(AgentKind::from_label(&time_label));
        self.subagents.push(entry);
        self.subagents_loaded = true;
        true
    }

    /// Start a fresh retry budget after an explicit retry or sidebar reopen.
    /// Keeps cached rows and errors visible until a worker actually claims work.
    /// Returns false unless the catalog is failed; never duplicates live work.
    pub fn retry_session_catalog(&mut self) -> bool {
        if !matches!(self.session_catalog_state, SessionCatalogState::Error(_)) {
            return false;
        }
        self.session_refresh_attempts = 0;
        self.last_sessions_refresh = None;
        true
    }

    /// True when the sessions list is stale enough to justify a refetch.
    /// Used by the pane to debounce the refresh worker.
    pub fn should_refresh_sessions(&self) -> bool {
        if matches!(self.session_catalog_state, SessionCatalogState::Ready) {
            // Periodic first-page replacement races continuation pages and
            // resets a scrolled catalogue. Ready lists advance only through
            // their cursor; endpoint changes explicitly invalidate them.
            return false;
        }
        let Some(last) = self.last_sessions_refresh else {
            return true;
        };
        let retry_after =
            if matches!(self.session_catalog_state, SessionCatalogState::Error(_)) {
                if self.session_refresh_attempts >= 3 {
                    return false;
                }
                0.75 * 2.0_f32
                    .powi(i32::from(self.session_refresh_attempts.saturating_sub(1)))
            } else {
                8.0
            };
        Instant::now().saturating_duration_since(last).as_secs_f32() >= retry_after
    }

    pub fn mark_refresh_kicked(&mut self) {
        self.session_refresh_attempts = self.session_refresh_attempts.saturating_add(1);
        self.last_sessions_refresh = Some(Instant::now());
        self.session_catalog_state = SessionCatalogState::Loading;
        if self.all_sessions.is_empty() {
            self.sessions_loading_started = Instant::now();
        }
    }

    pub fn set_sessions_error(&mut self, message: impl Into<String>) {
        self.session_page_loading = false;
        self.session_requested_cursor = None;
        self.last_sessions_refresh = Some(Instant::now());
        self.session_refresh_attempts = self.session_refresh_attempts.max(1);
        self.session_catalog_state = SessionCatalogState::Error(message.into());
    }

    pub fn should_refresh_subagents(&self) -> bool {
        !self.subagent_refresh_in_flight && !self.subagents_loaded
    }

    /// Preserve the existing waiting-permission blink ownership.
    fn has_waiting_subagents(&self) -> bool {
        self.branch_activities
            .values()
            .any(|activity| activity.status == BranchStatus::WaitingPermission)
            || self.subagents.iter().any(|entry| {
                entry
                    .runtime_status
                    .as_deref()
                    .and_then(BranchStatus::from_runtime_status)
                    == Some(BranchStatus::WaitingPermission)
            })
    }

    /// Claim the next branch-tree refresh. Returns a generation token that
    /// must accompany the result; `None` means another worker already owns
    /// the refresh or the debounce has not elapsed.
    pub fn begin_subagent_refresh(&mut self) -> Option<u64> {
        if !self.should_refresh_subagents() {
            return None;
        }
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = true;
        Some(self.subagent_refresh_generation)
    }

    /// Release a refresh only when it is still the current generation.
    /// Stale workers must not clear the in-flight flag of a newer request.
    pub fn complete_subagent_refresh(&mut self, generation: u64) -> bool {
        if generation != self.subagent_refresh_generation {
            return false;
        }
        self.subagent_refresh_in_flight = false;
        true
    }

    /// Stop retrying a failed event-triggered snapshot every render frame.
    /// The last good rows remain authoritative; a later lifecycle event,
    /// session switch, or event-stream reconnect will invalidate the tree and
    /// request one new recovery snapshot.
    pub fn settle_failed_subagent_refresh(&mut self) {
        self.subagents_loaded = true;
    }

    /// Discard an in-flight snapshot after a newer live lifecycle/activity
    /// event arrives. The refresh worker cannot be cancelled, but its
    /// generation can be invalidated so an older snapshot never overwrites
    /// the event-driven branch truth and flickers the footer/sidebar.
    pub fn invalidate_inflight_subagent_refresh(&mut self) {
        if !self.subagent_refresh_in_flight {
            return;
        }
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
    }

    /// Force the next `should_refresh_*` to fire — used when the
    /// session changes (e.g. user picks a different sub-agent) so the
    /// list reflects the new parent immediately rather than waiting
    /// for the debounce to tick down.
    pub fn invalidate_subagent_refresh(&mut self) {
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
        self.subagents_loaded = false;
        self.subagents.clear();
    }

    pub fn mark_subagent_tree_dirty(&mut self) {
        self.subagent_refresh_generation =
            self.subagent_refresh_generation.wrapping_add(1);
        self.subagent_refresh_in_flight = false;
        self.subagents_loaded = false;
    }

    pub fn branch_activity(&self, session_id: &str) -> Option<&BranchActivity> {
        self.branch_activities.get(session_id)
    }

    /// Apply an *authoritative* lifecycle status to a branch (parent
    /// `task` tool part, child `session.status idle`,
    /// `session.subtask.completed`). A terminal status here latches
    /// `terminal_locked` so subsequent straggler part-level activity
    /// can't flip the row back to "active"; an active/respawn status
    /// clears the latch so a genuinely re-spawned child reappears.
    pub fn set_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        self.transition_branch_activity_status(
            session_id,
            status,
            BranchStatusTransition::AuthoritativeRun,
        );
    }

    /// Apply a reconnect/history lifecycle snapshot. Terminal branches are
    /// hidden immediately instead of receiving the live completion linger
    /// window; active branches may still reopen on a newer family revision.
    pub fn set_branch_activity_status_from_recovery(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        let session_id = session_id.into();
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) {
            self.set_branch_activity_status(session_id, status);
            return;
        }
        let mut activity = BranchActivity::new(status, None, None);
        activity.terminal_locked = true;
        self.branch_activities.insert(session_id.clone(), activity);
        if let Some(entry) = self
            .subagents
            .iter_mut()
            .find(|entry| entry.id == session_id)
        {
            entry.runtime_status = Some(status.runtime_status().to_string());
        }
    }

    /// Apply a branch status with explicit lifecycle semantics. Returns
    /// `false` only when ancillary activity was rejected by a terminal lock.
    pub fn transition_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
        transition: BranchStatusTransition,
    ) -> bool {
        let session_id = session_id.into();
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) && self.viewed_session_id.as_deref() == Some(session_id.as_str())
        {
            self.retained_viewed_subagent_id = Some(session_id.clone());
        }
        if transition == BranchStatusTransition::AncillaryActivity
            && matches!(
                status,
                BranchStatus::Active | BranchStatus::WaitingPermission
            )
            && self.branch_terminal_locked(&session_id)
        {
            return false;
        }
        let terminal = matches!(status, BranchStatus::Completed | BranchStatus::Stopped);
        self.branch_activities
            .entry(session_id.clone())
            .and_modify(|activity| {
                activity.transition_status(status);
                activity.terminal_locked = terminal;
            })
            .or_insert_with(|| {
                let mut activity = BranchActivity::new(status, None, None);
                activity.terminal_locked = terminal;
                activity
            });

        // The sidebar row and its BranchActivity used to be two independent
        // lifecycle stores. A live completion updated BranchActivity (and the
        // parent Task card), while the already-rendered row could retain its
        // snapshot-era `runtime_status: running` until opening the child
        // forced another snapshot. Keep the visible row on the same
        // authoritative edge so completion paints immediately in the parent.
        if let Some(entry) = self
            .subagents
            .iter_mut()
            .find(|entry| entry.id == session_id)
        {
            entry.runtime_status = Some(status.runtime_status().to_string());
        }
        true
    }

    /// Reconcile a bootstrap/reconnect status snapshot without allowing an
    /// older snapshot to undo a terminal lifecycle event that arrived live. A genuinely
    /// restarted child emits a fresh live active event, which uses
    /// `set_branch_activity_status` and intentionally clears the latch.
    fn reconcile_snapshot_branch_activity_status(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) {
        let session_id = session_id.into();
        if matches!(
            status,
            BranchStatus::Active | BranchStatus::WaitingPermission
        ) && self.branch_terminal_locked(&session_id)
        {
            // `self.subagents` was rebuilt from this snapshot before status
            // reconciliation. Restore the live terminal value onto the row as
            // well as retaining it in BranchActivity; otherwise rendering
            // still sees the stale `running` snapshot even though the active
            // count correctly says the child is finished.
            if let Some(locked_status) = self
                .branch_activities
                .get(&session_id)
                .map(|activity| activity.status)
            {
                if let Some(entry) = self
                    .subagents
                    .iter_mut()
                    .find(|entry| entry.id == session_id)
                {
                    entry.runtime_status =
                        Some(locked_status.runtime_status().to_string());
                }
            }
            return;
        }
        self.set_branch_activity_status(session_id, status);
    }

    pub fn reconcile_branch_lifecycle_snapshot(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
    ) -> BranchStatus {
        let session_id = session_id.into();
        self.reconcile_snapshot_branch_activity_status(session_id.clone(), status);
        self.branch_activities
            .get(&session_id)
            .map(|activity| activity.status)
            .unwrap_or(status)
    }

    pub fn reset_branch_lifecycle(&mut self) {
        self.branch_activities.clear();
        for entry in self.subagents.iter_mut().skip(1) {
            entry.runtime_status = None;
        }
    }

    pub fn retain_authoritative_branches(
        &mut self,
        branch_ids: &std::collections::HashSet<String>,
    ) {
        let retained = self.retained_viewed_subagent_id.as_deref();
        self.branch_activities.retain(|session_id, _| {
            branch_ids.contains(session_id) || retained == Some(session_id.as_str())
        });
        let root_id = self.subagents.first().map(|entry| entry.id.clone());
        self.subagents.retain(|entry| {
            root_id.as_deref() == Some(entry.id.as_str())
                || branch_ids.contains(&entry.id)
                || retained == Some(entry.id.as_str())
        });
    }

    pub fn set_branch_activity_started_at(
        &mut self,
        session_id: impl Into<String>,
        started_at: Option<u64>,
    ) {
        self.branch_activities
            .entry(session_id.into())
            .and_modify(|activity| activity.started_at = started_at)
            .or_insert_with(|| {
                BranchActivity::new(BranchStatus::Active, None, started_at)
            });
    }

    pub fn set_branch_activity_tool(
        &mut self,
        session_id: impl Into<String>,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) {
        self.branch_activities
            .entry(session_id.into())
            .and_modify(|activity| {
                let was_terminal = activity.transition_status(status);
                if was_terminal {
                    return;
                }
                match current_tool.as_ref().map(|tool| tool.trim()) {
                    Some(tool) if !tool.is_empty() => {
                        activity.current_tool = Some(tool.to_string());
                    }
                    Some(_) => {
                        activity.current_tool = None;
                    }
                    None => {}
                }
                if started_at.is_some() {
                    activity.started_at = started_at;
                }
            })
            .or_insert_with(|| BranchActivity::new(status, current_tool, started_at));
    }

    /// Apply *part-level* activity (a raw text/reasoning/tool delta
    /// from the child) to a branch. Unlike an authoritative lifecycle
    /// status, this is noisy: the child keeps emitting "active"
    /// part-updates even as it winds down, and a straggler can arrive
    /// after the run has already finished. Once a branch is
    /// `terminal_locked` (it finished via an authoritative signal),
    /// these updates are dropped entirely so a finished sub-agent
    /// never resurrects to "responding"/"thinking". Returns `true`
    /// when the update was applied (so the caller can mirror the
    /// `active_subagent_ids` set only for genuinely-live branches).
    pub fn note_subagent_part_activity(
        &mut self,
        session_id: &str,
        status: BranchStatus,
        current_tool: Option<String>,
        started_at: Option<u64>,
    ) -> bool {
        if let Some(activity) = self.branch_activities.get(session_id) {
            if activity.terminal_locked {
                // The branch already finished for real — ignore the
                // late part delta that still claims it's working.
                return false;
            }
        }
        self.set_branch_activity_tool(
            session_id.to_string(),
            status,
            current_tool,
            started_at,
        );
        true
    }

    /// Whether `session_id` has latched an authoritative terminal
    /// state. Callers use this to keep their own "live" bookkeeping
    /// (`active_subagent_ids`) from re-adding a finished branch on a
    /// straggler part delta.
    pub fn branch_terminal_locked(&self, session_id: &str) -> bool {
        self.branch_activities
            .get(session_id)
            .is_some_and(|activity| activity.terminal_locked)
    }

    pub fn active_child_count(&self, current_session_id: Option<&str>) -> usize {
        self.active_child_ids(current_session_id).len()
    }

    pub fn active_child_ids(&self, current_session_id: Option<&str>) -> BTreeSet<String> {
        self.branch_activities
            .iter()
            .filter(|(session_id, activity)| {
                Some(session_id.as_str()) != current_session_id
                    && self
                        .subagents
                        .iter()
                        .any(|entry| entry.id.as_str() == session_id.as_str())
                    && matches!(
                        activity.status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    )
            })
            .map(|(session_id, _)| session_id.clone())
            .collect()
    }

    pub fn active_child_started_at(
        &self,
        current_session_id: Option<&str>,
    ) -> Option<u64> {
        self.branch_activities
            .iter()
            .filter(|(session_id, activity)| {
                Some(session_id.as_str()) != current_session_id
                    && self
                        .subagents
                        .iter()
                        .any(|entry| entry.id.as_str() == session_id.as_str())
                    && matches!(
                        activity.status,
                        BranchStatus::Active | BranchStatus::WaitingPermission
                    )
            })
            .filter_map(|(_, activity)| activity.started_at)
            .min()
    }

    /// Effective per-row height. Chat mode doubles up so each branch
    /// has space for its indented activity sub-row (connector + tool +
    /// status dot).
    pub fn row_height(&self) -> f32 {
        match self.mode {
            SidePanelMode::Sessions => ROW_HEIGHT,
            SidePanelMode::Subagents => 52.0,
        }
    }

    /// Number of rows that fit in `panel_height` logical pixels.
    /// Kept available for the keyboard-nav path queued behind the
    /// initial wire-up.
    #[allow(dead_code)]
    pub fn rows_per_panel(&self, panel_height: f32) -> usize {
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return 0;
        }
        (panel_height / row_h).floor().max(0.0) as usize
    }

    fn active_len(&self) -> usize {
        self.active_rows().len()
    }

    /// Next selectable (non-header) row in `active_rows` from `from`,
    /// scanning forward or backward. `None` when there is none in that
    /// direction — the caller then keeps its current selection.
    fn step_selectable(&self, from: usize, forward: bool) -> Option<usize> {
        let rows = self.active_rows();
        if forward {
            ((from + 1)..rows.len()).find(|&i| rows[i].is_selectable())
        } else {
            (0..from).rev().find(|&i| rows[i].is_selectable())
        }
    }

    /// Nearest selectable (non-header) row at or around `index`.
    fn nearest_selectable(&self, index: usize) -> Option<usize> {
        let rows = self.active_rows();
        if rows.is_empty() {
            return None;
        }
        let index = index.min(rows.len() - 1);
        if rows[index].is_selectable() {
            return Some(index);
        }
        self.step_selectable(index, true)
            .or_else(|| self.step_selectable(index, false))
    }

    /// Move selection off a header row (used after the display list is
    /// rebuilt so the cursor never lands on a group caption).
    fn snap_selection_to_selectable(&mut self) {
        if let Some(index) = self.nearest_selectable(self.selected) {
            self.selected = index;
        }
    }

    /// Whether the selection cursor is currently on the home-mode search
    /// row (only meaningful in [`SidePanelMode::Sessions`]).
    pub fn search_focused(&self) -> bool {
        self.search_focused && matches!(self.mode, SidePanelMode::Sessions)
    }

    /// Move the selection cursor onto the search row.
    pub fn focus_search(&mut self) {
        if self.navigation_header_enabled {
            return;
        }
        self.selected_header = None;
        self.new_chat_selected = false;
        self.search_focused = true;
        self.cursor_spring.reset();
    }

    fn clear_search_focus(&mut self) {
        self.search_focused = false;
    }

    pub fn select_next(&mut self) {
        let len = self.active_len();
        if self.new_chat_selected() && self.navigation_header_enabled {
            self.select_header_action(CatalogHeaderAction::Search);
            return;
        }
        if self.selected_header_action() == Some(CatalogHeaderAction::Search) {
            self.select_projects();
            return;
        }
        if self.projects_selected() {
            if let Some(first) = self.nearest_selectable(0) {
                self.move_selection_to(first);
            }
            return;
        }
        if self.new_chat_selected() {
            self.new_chat_selected = false;
            if let Some(first) = self.nearest_selectable(0) {
                self.move_selection_to(first);
            }
            return;
        }
        if len == 0 {
            return;
        }
        if self.search_focused() {
            self.clear_search_focus();
            if let Some(first) = self.nearest_selectable(0) {
                self.move_selection_to(first);
            }
            return;
        }
        if let Some(next) = self.step_selectable(self.selected, true) {
            self.move_selection_to(next);
        }
    }

    pub fn select_prev(&mut self) {
        if self.projects_selected() {
            self.select_header_action(CatalogHeaderAction::Search);
            return;
        }
        if self.selected_header_action() == Some(CatalogHeaderAction::Search) {
            self.select_new_chat();
            return;
        }
        if self.new_chat_selected() || self.search_focused() {
            return;
        }
        if self.navigation_header_enabled
            && matches!(self.mode, SidePanelMode::Sessions)
            && self.step_selectable(self.selected, false).is_none()
        {
            self.select_projects();
            return;
        }
        if self.active_len() == 0 {
            if matches!(self.mode, SidePanelMode::Sessions)
                && self.new_chat_rect.is_some()
            {
                self.select_new_chat();
            }
            return;
        }
        match self.step_selectable(self.selected, false) {
            Some(prev) => self.move_selection_to(prev),
            None if matches!(self.mode, SidePanelMode::Sessions)
                && self.new_chat_rect.is_some() =>
            {
                self.select_new_chat()
            }
            None if matches!(self.mode, SidePanelMode::Sessions)
                && self.session_search_rect.is_some() =>
            {
                self.focus_search()
            }
            None => {}
        }
    }

    pub fn select_next_by(&mut self, rows: usize) {
        if self.new_chat_selected() || self.selected_header_action().is_some() {
            self.select_next();
            return;
        }
        let len = self.active_len();
        if len == 0 {
            return;
        }
        if self.search_focused() {
            self.clear_search_focus();
        }
        let target = self.selected.saturating_add(rows.max(1)).min(len - 1);
        let target = if !self.active_rows()[target].is_selectable() {
            self.step_selectable(target, true)
                .or_else(|| self.step_selectable(target, false))
        } else {
            Some(target)
        };
        if let Some(target) = target {
            self.move_selection_to(target);
        }
    }

    pub fn select_prev_by(&mut self, rows: usize) {
        if self.new_chat_selected() || self.selected_header_action().is_some() {
            self.select_prev();
            return;
        }
        if self.search_focused() || self.active_len() == 0 {
            return;
        }
        let target = self.selected.saturating_sub(rows.max(1));
        let target = if !self.active_rows()[target].is_selectable() {
            self.step_selectable(target, false)
                .or_else(|| self.step_selectable(target, true))
        } else {
            Some(target)
        };
        if let Some(target) = target {
            self.move_selection_to(target);
        }
    }

    pub fn set_selected(&mut self, row: usize) {
        let len = self.active_len();
        if len == 0 {
            return;
        }
        // A click selects a real row, so it also leaves the search field and
        // the Back affordance.
        self.search_focused = false;
        self.new_chat_selected = false;
        // A click may land on a header row; snap to the nearest session.
        let row = self.nearest_selectable(row.min(len - 1)).unwrap_or(0);
        self.move_selection_to(row);
    }

    fn move_selection_to(&mut self, new_selected: usize) {
        self.selected_header = None;
        self.new_chat_selected = false;
        self.selected_cursor_rect = None;
        let len = self.active_len();
        if len == 0 {
            return;
        }
        let new_selected = new_selected.min(len - 1);
        if new_selected == self.selected {
            return;
        }
        let was_idle = self.cursor_spring.position == 0.0;
        let rows = self.selected as i32 - new_selected as i32;
        self.cursor_spring.position += rows as f32 * self.row_height();
        if was_idle {
            self.last_cursor_frame = Instant::now();
        }
        self.selected = new_selected;
        if matches!(self.mode, SidePanelMode::Subagents) {
            self.reveal_selected_branch = true;
        } else {
            self.clamp_scroll(self.last_panel_height_rows);
        }
    }

    fn scrolloff_for(panel_height_rows: usize) -> usize {
        if panel_height_rows <= 2 {
            return 0;
        }
        SCROLL_OFF_ROWS.min(panel_height_rows.saturating_sub(1) / 2)
    }

    pub fn clamp_scroll(&mut self, panel_height_rows: usize) {
        if self.active_len() == 0 {
            self.scroll_top = 0;
            return;
        }
        if panel_height_rows == 0 {
            return;
        }
        let scrolloff = Self::scrolloff_for(panel_height_rows);
        if self.selected < self.scroll_top.saturating_add(scrolloff) {
            self.set_scroll_top(self.selected.saturating_sub(scrolloff));
        } else if self.selected.saturating_add(scrolloff)
            >= self.scroll_top.saturating_add(panel_height_rows)
        {
            self.set_scroll_top(self.selected + scrolloff + 1 - panel_height_rows);
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub fn clamp_scroll_bounds(&mut self, panel_height_rows: usize) {
        if self.active_len() == 0 {
            self.scroll_top = 0;
            return;
        }
        let max_top = self.max_scroll_top(panel_height_rows);
        if self.scroll_top > max_top {
            self.set_scroll_top(max_top);
        }
    }

    pub fn max_scroll_top(&self, panel_height_rows: usize) -> usize {
        let visible = panel_height_rows.max(1);
        self.active_len().saturating_sub(visible)
    }

    /// Set the committed top row and drive the pixel spring toward its
    /// row boundary (used by arrow-key navigation / clamping — rows align
    /// flush, so no sub-row remainder).
    fn set_scroll_top(&mut self, new_top: usize) {
        self.scroll_top = new_top;
        self.scroll_px = new_top as f32 * self.row_height();
        self.scroll.set_target(self.scroll_px);
    }

    /// Maximum continuous pixel scroll for the home list at the given
    /// viewport height (rows), leaving the last row flush at the bottom.
    fn max_scroll_px(&self, panel_height_rows: usize) -> f32 {
        let row_h = self.row_height();
        let content = self.active_len() as f32 * row_h;
        let viewport = panel_height_rows.max(1) as f32 * row_h;
        (content - viewport).max(0.0)
    }

    pub fn scroll_by(&mut self, delta: i32, panel_height_rows: usize) {
        let max_top = self.max_scroll_top(panel_height_rows);
        let new_top = if delta < 0 {
            self.scroll_top
                .saturating_sub(delta.unsigned_abs() as usize)
        } else {
            self.scroll_top.saturating_add(delta as usize).min(max_top)
        };
        if new_top != self.scroll_top {
            self.set_scroll_top(new_top);
        }
    }

    pub fn scroll_pixels(&mut self, delta_pixels: f32, panel_height_rows: usize) {
        if delta_pixels == 0.0 {
            return;
        }
        // Chat mode scrolls the *whole* content column as one pixel
        // viewport so every section (goal, branches, tasks) is
        // reachable — not just the branch sub-list. A positive
        // `delta_pixels` is a scroll-up gesture, which should reveal
        // earlier (top) content, i.e. reduce the offset.
        if matches!(self.mode, SidePanelMode::Subagents) {
            self.scroll_content_pixels(-delta_pixels);
            return;
        }
        let row_h = self.row_height();
        if row_h <= 0.0 {
            return;
        }
        // Pixel-precise home scroll: move the continuous position by the
        // exact gesture delta (positive = scroll toward the top) and let
        // the short spring animate to it. No whole-row quantization, so
        // trackpad / wheel track the finger tightly. `scroll_top` is the
        // derived integer top row for the render window + selection math.
        let max_px = self.max_scroll_px(panel_height_rows);
        let next = (self.scroll_px - delta_pixels).clamp(0.0, max_px);
        if next == self.scroll_px {
            return;
        }
        self.scroll_px = next;
        self.scroll.set_target(next);
        self.scroll_top = (next / row_h).floor() as usize;
    }

    /// Direct touch counterpart. Home/session lists snap to the exact bounded
    /// target; chat content is already a continuous direct viewport.
    pub fn scroll_touch_pixels(&mut self, delta_pixels: f32, panel_height_rows: usize) {
        self.scroll_pixels(delta_pixels, panel_height_rows);
        if !matches!(self.mode, SidePanelMode::Subagents) {
            self.scroll.set_target_immediate(self.scroll_px);
        }
    }

    /// Advance the home-list scroll spring and return the *absolute*
    /// animated scroll position in **rows** (0 = top). The renderer
    /// multiplies by its own (chrome-scaled) row height and derives the
    /// top row + sub-row offset from it, so scrolling is pixel-smooth and
    /// scale-independent.
    pub fn tick_scroll(&mut self) -> f32 {
        self.tick_catalog_families();
        if matches!(self.mode, SidePanelMode::Subagents) {
            self.scroll.reset();
            self.last_scroll_frame = Instant::now();
            return 0.0;
        }
        let row_h = self.row_height().max(1.0);
        if !self.scroll.is_animating() {
            self.last_scroll_frame = Instant::now();
            return self.scroll.current().max(0.0) / row_h;
        }
        let now = Instant::now();
        let dt = now
            .saturating_duration_since(self.last_scroll_frame)
            .as_secs_f32()
            .min(0.05);
        self.last_scroll_frame = now;
        self.scroll.tick(dt);
        self.scroll.current().max(0.0) / row_h
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

    /// Each sidebar paint starts a fresh, independent redraw claim.
    pub fn reset_visible_running_indicator(&mut self) {
        self.visible_running_indicator = false;
    }

    pub fn note_visible_running_indicator(&mut self, visible: bool) {
        self.visible_running_indicator |= visible;
    }

    pub fn is_animating(&self) -> bool {
        let sessions_loading = matches!(self.mode, SidePanelMode::Sessions)
            && !self.user_hidden
            && self.sessions_loading_elapsed() < 1.5
            && (matches!(self.session_catalog_state, SessionCatalogState::Loading)
                || self.session_page_loading);
        self.scroll.is_animating()
            || self.catalog_completion_pending()
            || self.cursor_spring.position != 0.0
            || sessions_loading
            || (self.semantic_searching && self.semantic_search_elapsed() < 1.5)
            // Waiting retains its existing blink owner; active indicators own
            // frames only after painting visible, nonoccluded ink.
            || self.has_waiting_subagents()
            || (!self.user_hidden && self.visible_running_indicator)
            || (self.session_hover_target && self.session_title_hover_overflow)
            || (self.session_hover_target && self.session_hover_scale < 0.998)
            || (!self.session_hover_target && self.session_hover_scale > 0.002)
            || self.usage_scramble_elapsed_ms().is_some()
    }

    pub fn catalog_is_animating(&self) -> bool {
        let sessions_loading = !self.user_hidden
            && self.sessions_loading_elapsed() < 1.5
            && (matches!(self.session_catalog_state, SessionCatalogState::Loading)
                || self.session_page_loading);
        matches!(self.mode, SidePanelMode::Sessions)
            && (self.scroll.is_animating()
                || self.catalog_completion_pending()
                || self.cursor_spring.position != 0.0
                || sessions_loading
                || (!self.user_hidden && self.visible_running_indicator)
                || (self.session_hover_target && self.session_hover_scale < 0.998)
                || (!self.session_hover_target && self.session_hover_scale > 0.002)
                || (self.session_hover_target && self.session_title_hover_overflow))
    }

    /// Map a window-space click to a row index. Returns `None` when
    /// outside the panel content area or past the last visible row.
    pub fn hit_test_row(
        &self,
        mouse_x: f32,
        mouse_y: f32,
        _panel_rect: [f32; 4],
    ) -> Option<usize> {
        let [content_x, content_y, content_w, content_h] = self.last_row_hit_rect?;
        if content_w <= 0.0 || content_h <= 0.0 {
            return None;
        }
        let row_h = self.last_row_hit_height;
        if mouse_x < content_x || mouse_x > content_x + content_w {
            return None;
        }
        // Bounds-check against the visible (clamped) rect, but anchor
        // the row math to the full list's origin so a scrolled list
        // still maps clicks to the right index.
        if mouse_y < content_y || mouse_y >= content_y + content_h {
            return None;
        }
        // Home mode uses the row spring; chat mode has already shifted the
        // full list origin by its content scroll in the renderer.
        let row = if matches!(self.mode, SidePanelMode::Sessions) {
            let scroll_rows = self.scroll.current().max(0.0) / self.row_height();
            ((mouse_y - self.last_row_origin_y) / row_h + scroll_rows).floor() as isize
        } else {
            ((mouse_y - self.last_row_origin_y) / row_h).floor() as isize
        };
        if row < 0 {
            return None;
        }
        let row = row as usize;
        if row >= self.active_len() {
            return None;
        }
        Some(row)
    }
}

#[cfg(test)]
#[path = "project_catalog_tests.rs"]
mod project_catalog_tests;

#[cfg(test)]
mod catalog_activity_tests {
    use super::*;

    #[test]
    fn left_catalog_activity_all_variants_pages_and_invalidation() {
        use neoism_agent_core::CatalogActivity;
        for activity in [
            CatalogActivity::Running,
            CatalogActivity::Background,
            CatalogActivity::Permission,
            CatalogActivity::Idle,
        ] {
            let mut panel = NeoismAgentSidePanel::default();
            panel.set_sessions(vec![root()]);
            panel.set_session_activity("main", activity);
            panel.set_session_activity("paged", activity);
            panel.set_session_page(
                vec![NeoismAgentSessionEntry::new("paged", "Paged", "")],
                Some("cursor"),
                None,
            );
            assert_eq!(
                panel
                    .all_sessions
                    .iter()
                    .find(|row| row.id == "paged")
                    .unwrap()
                    .catalog_activity,
                Some(activity)
            );
            panel.upsert_session(root());
            assert_eq!(
                panel
                    .all_sessions
                    .iter()
                    .find(|row| row.id == "main")
                    .unwrap()
                    .catalog_activity,
                Some(activity)
            );
            panel.invalidate_session_catalog();
            panel.set_sessions(vec![root()]);
            assert_eq!(panel.all_sessions[0].catalog_activity, None);
        }
    }

    fn root() -> NeoismAgentSessionEntry {
        NeoismAgentSessionEntry::new("main", "Main conversation", "")
    }

    #[test]
    fn left_catalog_activity_updates_without_detail_branches_or_reflow() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_sessions(vec![
            root(),
            NeoismAgentSessionEntry::new("other", "Other", ""),
        ]);
        panel.selected = panel
            .sessions
            .iter()
            .position(|row| row.id == "main")
            .unwrap();
        panel.scroll_px = 42.0;
        let selected = panel.selected;
        let identities: Vec<_> =
            panel.sessions.iter().map(|row| row.id.clone()).collect();
        assert!(panel.subagents.is_empty());
        assert!(panel
            .set_session_activity("main", neoism_agent_core::CatalogActivity::Running));
        assert_eq!(
            panel.selected_session().unwrap().catalog_activity,
            Some(neoism_agent_core::CatalogActivity::Running)
        );
        assert_eq!(panel.selected, selected);
        assert_eq!(panel.scroll_px, 42.0);
        assert_eq!(
            panel
                .sessions
                .iter()
                .map(|row| row.id.clone())
                .collect::<Vec<_>>(),
            identities
        );
        assert!(!panel
            .set_session_activity("main", neoism_agent_core::CatalogActivity::Running));
        assert!(
            panel.set_session_activity("main", neoism_agent_core::CatalogActivity::Idle)
        );
        assert_eq!(
            panel.selected_session().unwrap().catalog_activity,
            Some(neoism_agent_core::CatalogActivity::Idle)
        );
    }

    #[test]
    fn left_catalog_activity_survives_metadata_and_late_snapshots() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_sessions(vec![root()]);
        panel.set_session_activity("main", neoism_agent_core::CatalogActivity::Running);
        panel.upsert_session(NeoismAgentSessionEntry::new("main", "Renamed", ""));
        panel.reconcile_session_head(vec![root()], None);
        panel.set_session_page(vec![root()], None, None);
        assert_eq!(
            panel.all_sessions[0].catalog_activity,
            Some(neoism_agent_core::CatalogActivity::Running)
        );
        panel.set_session_activity("main", neoism_agent_core::CatalogActivity::Idle);
        let stale = root()
            .with_runtime_status(Some("running".into()))
            .with_catalog_activity(Some(neoism_agent_core::CatalogActivity::Running));
        panel.upsert_session(stale.clone());
        panel.reconcile_session_head(vec![stale.clone()], None);
        panel.set_session_page(vec![stale], None, None);
        assert_eq!(
            panel.all_sessions[0].catalog_activity,
            Some(neoism_agent_core::CatalogActivity::Idle)
        );
    }

    #[test]
    fn left_catalog_activity_before_row_and_reconnect_recovery() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_session_activity("main", neoism_agent_core::CatalogActivity::Running);
        panel.upsert_session(root());
        assert_eq!(
            panel.all_sessions[0].catalog_activity,
            Some(neoism_agent_core::CatalogActivity::Running)
        );
        panel.clear_session_activity_overrides();
        panel.reconcile_session_head(vec![root()], None);
        assert!(panel.all_sessions[0].runtime_status.is_none());
        panel.set_session_activity("main", neoism_agent_core::CatalogActivity::Running);
        panel.remove_session("main");
        panel.set_sessions(vec![root()]);
        assert!(panel.all_sessions[0].runtime_status.is_none());
    }
}

#[cfg(test)]
mod failed_child_visibility_tests {
    use super::*;

    #[test]
    fn failed_child_remains_inspectable_after_completed_child_grace() {
        let mut panel = NeoismAgentSidePanel::default();
        panel.set_subagents(vec![
            NeoismAgentSessionEntry::new("main", "main", ""),
            NeoismAgentSessionEntry::new("child", "failed task", "claude")
                .with_runtime_status(Some("running".into())),
        ]);
        panel.set_branch_activity_status("child", BranchStatus::Stopped);
        panel
            .branch_activities
            .get_mut("child")
            .unwrap()
            .completed_at =
            Some(Instant::now() - SUBAGENT_HIDE_AFTER - Duration::from_secs(1));
        assert!(!panel.prune_expired_completed_subagents());
        assert_eq!(panel.subagents().len(), 2);

        panel
            .branch_activities
            .get_mut("child")
            .unwrap()
            .completed_at =
            Some(Instant::now() - FAILED_SUBAGENT_HIDE_AFTER - Duration::from_secs(1));
        assert!(panel.prune_expired_completed_subagents());
        assert_eq!(panel.subagents().len(), 1);
    }
}
