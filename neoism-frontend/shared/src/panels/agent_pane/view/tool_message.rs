use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde_json::Value;
use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use sugarloaf::text::DrawOpts;
use sugarloaf::Sugarloaf;

use crate::panels::agent_pane::state::{
    NeoismAgentMessage, NeoismAgentPane, NeoismAgentTodo,
};
use crate::syntax::Lang;
use crate::widgets::diff_card::{self, CardSpec, DiffLine, DiffLineKind};
use crate::widgets::scrollbar;

use super::code_block::truncate_chars;
use super::draw::{
    draw_rect_clipped, draw_rounded_rect_clipped, draw_text_clipped, intersect_rect,
    opts_with_clip, wrap_text,
};
use super::{ORDER_PANEL, ORDER_TEXT};
use crate::primitives::ide_theme::IdeTheme;

const COLLAPSED_DIFF_ROWS: usize = 24;
const EXPANDED_DIFF_ROWS: usize = 120;
const MAX_STORED_DIFF_ROWS: usize = 160;
const TOOL_GROUP_PREVIEW_LINES: usize = 5;
const TOOL_WRAP_CACHE_LIMIT: usize = 2048;
const TOOL_DIFF_CARD_VIEW_CACHE_LIMIT: usize = 2048;

pub const TODO_ROW_HEIGHT: f32 = 28.0;

thread_local! {
    static TOOL_WRAP_CACHE: RefCell<ToolWrapCache> = RefCell::new(ToolWrapCache::new());
    static TOOL_DIFF_CARD_VIEW_CACHE: RefCell<ToolDiffCardViewCache> =
        RefCell::new(ToolDiffCardViewCache::new());
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ToolWrapCacheKey {
    body_hash: u64,
    body_len: usize,
    width_bits: u32,
    font_size_bits: u32,
    bold: bool,
    italic: bool,
    limit: usize,
}

struct ToolWrapCache {
    values: HashMap<ToolWrapCacheKey, Rc<Vec<ToolWrappedRow>>>,
    order: VecDeque<ToolWrapCacheKey>,
}

impl ToolWrapCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &ToolWrapCacheKey) -> Option<Rc<Vec<ToolWrappedRow>>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: ToolWrapCacheKey, value: Rc<Vec<ToolWrappedRow>>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key);
        self.values.insert(key, value);
        while self.order.len() > TOOL_WRAP_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
struct ToolDiffCardViewKey {
    section_fingerprint: u64,
    body_width_bits: u32,
    scale_bits: u32,
    expanded: bool,
}

pub(crate) struct ToolDiffCardView {
    rows: Rc<Vec<DiffLine>>,
    visual_row_offsets: Rc<Vec<usize>>,
    visual_rows: usize,
    preview_visual_rows: usize,
}

struct ToolDiffCardViewCache {
    values: HashMap<ToolDiffCardViewKey, Rc<ToolDiffCardView>>,
    order: VecDeque<ToolDiffCardViewKey>,
}

impl ToolDiffCardViewCache {
    fn new() -> Self {
        Self {
            values: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    fn get(&self, key: &ToolDiffCardViewKey) -> Option<Rc<ToolDiffCardView>> {
        self.values.get(key).cloned()
    }

    fn insert(&mut self, key: ToolDiffCardViewKey, value: Rc<ToolDiffCardView>) {
        if self.values.contains_key(&key) {
            self.values.insert(key, value);
            return;
        }
        self.order.push_back(key);
        self.values.insert(key, value);
        while self.order.len() > TOOL_DIFF_CARD_VIEW_CACHE_LIMIT {
            if let Some(old) = self.order.pop_front() {
                self.values.remove(&old);
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TodoVisualState {
    Completed,
    InProgress,
    Pending,
}

impl TodoVisualState {
    pub fn from_status(status: &str) -> Self {
        match status
            .trim()
            .replace('-', "_")
            .to_ascii_lowercase()
            .as_str()
        {
            "completed" | "complete" | "done" | "success" => Self::Completed,
            "in_progress" | "active" | "running" | "current" => Self::InProgress,
            _ => Self::Pending,
        }
    }

    pub fn text_color(self, theme: &IdeTheme) -> [u8; 4] {
        theme.u8(match self {
            Self::Completed => theme.muted,
            Self::InProgress => theme.yellow,
            Self::Pending => theme.fg,
        })
    }

    pub fn text_bold(self) -> bool {
        matches!(self, Self::InProgress)
    }
}

pub trait AgentToolTodo {
    fn status(&self) -> &str;
    fn content(&self) -> &str;
}

struct EmptyToolTodo;

impl AgentToolTodo for EmptyToolTodo {
    fn status(&self) -> &str {
        ""
    }

    fn content(&self) -> &str {
        ""
    }
}

impl AgentToolTodo for NeoismAgentTodo {
    fn status(&self) -> &str {
        &self.status
    }

    fn content(&self) -> &str {
        &self.content
    }
}

pub trait AgentToolMessage {
    type Todo: AgentToolTodo;

    fn id(&self) -> &str;
    fn title(&self) -> &str;
    fn text(&self) -> &str;
    fn status(&self) -> &str;
    fn tool(&self) -> &str;
    fn detail(&self) -> &str;
    fn is_todos_output(&self) -> bool;
    fn todos(&self) -> &[Self::Todo];

    fn title_text(&self) -> String {
        if !self.title().is_empty() {
            if !self.status().is_empty() {
                return format!("{}  {}", self.title(), self.status());
            }
            return self.title().to_string();
        }
        "Tool".to_string()
    }
}

pub type CachedToolDiffSections = Rc<Vec<ToolDiffSection>>;

struct ToolMessageParts<'a> {
    id: &'a str,
    title: &'a str,
    text: &'a str,
    status: &'a str,
    tool: &'a str,
    detail: &'a str,
}

impl AgentToolMessage for ToolMessageParts<'_> {
    type Todo = EmptyToolTodo;

    fn id(&self) -> &str {
        self.id
    }

    fn title(&self) -> &str {
        self.title
    }

    fn text(&self) -> &str {
        self.text
    }

    fn status(&self) -> &str {
        self.status
    }

    fn tool(&self) -> &str {
        self.tool
    }

    fn detail(&self) -> &str {
        self.detail
    }

    fn is_todos_output(&self) -> bool {
        false
    }

    fn todos(&self) -> &[Self::Todo] {
        &[]
    }
}

impl AgentToolMessage for NeoismAgentMessage {
    type Todo = NeoismAgentTodo;

    fn id(&self) -> &str {
        &self.id
    }

    fn title(&self) -> &str {
        &self.title
    }

    fn text(&self) -> &str {
        &self.text
    }

    fn status(&self) -> &str {
        &self.status
    }

    fn tool(&self) -> &str {
        &self.tool
    }

    fn detail(&self) -> &str {
        &self.detail
    }

    fn is_todos_output(&self) -> bool {
        matches!(
            self.output_kind,
            crate::panels::agent_pane::state::NeoismAgentOutputKind::Todos
        )
    }

    fn todos(&self) -> &[Self::Todo] {
        &self.todos
    }
}

pub trait AgentToolPane {
    fn register_tool_hit_rect(&mut self, id: String, rect: [f32; 4]);
    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str>;
    fn tool_expanded(&self, id: &str) -> bool;
    fn tool_expand_progress(&self, id: &str) -> f32;
    /// True when this tool row belongs to a turn that settled before the
    /// current visit's live trace window. Archived tool cards render as a
    /// single header line (no output preview, no diff bodies) until clicked —
    /// finished conversations show what happened, not every byte of it.
    fn tool_archived(&self, _id: &str) -> bool {
        false
    }
    fn diff_scroll_offset(&mut self, key: &str, max_scroll: f32) -> f32;
    fn register_diff_scroll_rect(&mut self, key: String, rect: [f32; 4], max_scroll: f32);
    fn register_link_hit_rect(&mut self, target: String, rect: [f32; 4]);
    fn link_hovered(&self, target: &str) -> bool;
    fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize;
    fn selectable_line_highlight(&self, index: usize) -> Option<(f32, f32)>;
    fn suppress_tool_interactions(&self) -> bool {
        false
    }
}

#[macro_export]
macro_rules! neoism_ui_impl_agent_tool_message {
    ($todo:ty, $message:ty, $pane:ty, $output_kind:ident) => {
        impl $crate::panels::agent_pane::view::tool_message::AgentToolTodo for $todo {
            fn status(&self) -> &str {
                &self.status
            }

            fn content(&self) -> &str {
                &self.content
            }
        }

        impl $crate::panels::agent_pane::view::tool_message::AgentToolMessage
            for $message
        {
            type Todo = $todo;

            fn id(&self) -> &str {
                &self.id
            }

            fn title(&self) -> &str {
                &self.title
            }

            fn text(&self) -> &str {
                &self.text
            }

            fn status(&self) -> &str {
                &self.status
            }

            fn tool(&self) -> &str {
                &self.tool
            }

            fn detail(&self) -> &str {
                &self.detail
            }

            fn is_todos_output(&self) -> bool {
                matches!(self.output_kind, $output_kind::Todos)
            }

            fn todos(&self) -> &[Self::Todo] {
                &self.todos
            }
        }

        impl $crate::panels::agent_pane::view::tool_message::AgentToolPane for $pane {
            fn register_tool_hit_rect(&mut self, id: String, rect: [f32; 4]) {
                <$pane>::register_tool_hit_rect(self, id, rect);
            }

            fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
                <$pane>::selected_tool_group_child(self, group_id)
            }

            fn tool_expanded(&self, id: &str) -> bool {
                <$pane>::tool_expanded(self, id)
            }

            fn tool_expand_progress(&self, id: &str) -> f32 {
                <$pane>::tool_expand_progress(self, id)
            }

            fn tool_archived(&self, id: &str) -> bool {
                <$pane>::tool_archived(self, id)
            }

            fn diff_scroll_offset(&mut self, key: &str, max_scroll: f32) -> f32 {
                <$pane>::diff_scroll_offset(self, key, max_scroll)
            }

            fn register_diff_scroll_rect(
                &mut self,
                key: String,
                rect: [f32; 4],
                max_scroll: f32,
            ) {
                <$pane>::register_diff_scroll_rect(self, key, rect, max_scroll);
            }

            fn register_link_hit_rect(&mut self, target: String, rect: [f32; 4]) {
                <$pane>::register_link_hit_rect(self, target, rect);
            }

            fn link_hovered(&self, target: &str) -> bool {
                <$pane>::link_hovered(self, target)
            }

            fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize {
                <$pane>::register_selectable_line(self, text, rect)
            }

            fn selectable_line_highlight(&self, index: usize) -> Option<(f32, f32)> {
                <$pane>::selectable_line_highlight(self, index)
            }

            fn suppress_tool_interactions(&self) -> bool {
                <$pane>::suppress_markdown_interactions(self)
            }
        }
    };
}

impl AgentToolPane for NeoismAgentPane {
    fn register_tool_hit_rect(&mut self, id: String, rect: [f32; 4]) {
        NeoismAgentPane::register_tool_hit_rect(self, id, rect);
    }

    fn selected_tool_group_child(&self, group_id: &str) -> Option<&str> {
        NeoismAgentPane::selected_tool_group_child(self, group_id)
    }

    fn tool_expanded(&self, id: &str) -> bool {
        NeoismAgentPane::tool_expanded(self, id)
    }

    fn tool_expand_progress(&self, id: &str) -> f32 {
        NeoismAgentPane::tool_expand_progress(self, id)
    }

    fn tool_archived(&self, id: &str) -> bool {
        NeoismAgentPane::tool_archived(self, id)
    }

    fn diff_scroll_offset(&mut self, key: &str, max_scroll: f32) -> f32 {
        NeoismAgentPane::diff_scroll_offset(self, key, max_scroll)
    }

    fn register_diff_scroll_rect(
        &mut self,
        key: String,
        rect: [f32; 4],
        max_scroll: f32,
    ) {
        NeoismAgentPane::register_diff_scroll_rect(self, key, rect, max_scroll);
    }

    fn register_link_hit_rect(&mut self, target: String, rect: [f32; 4]) {
        NeoismAgentPane::register_link_hit_rect(self, target, rect);
    }

    fn link_hovered(&self, target: &str) -> bool {
        NeoismAgentPane::link_hovered(self, target)
    }

    fn register_selectable_line(&mut self, text: &str, rect: [f32; 4]) -> usize {
        NeoismAgentPane::register_selectable_line(self, text, rect)
    }

    fn selectable_line_highlight(&self, index: usize) -> Option<(f32, f32)> {
        NeoismAgentPane::selectable_line_highlight(self, index)
    }

    fn suppress_tool_interactions(&self) -> bool {
        NeoismAgentPane::suppress_markdown_interactions(self)
    }
}

// ---- god-file split: sibling modules; each child is `use super::*;` ----
mod diff;
mod render;
mod widgets;

pub use diff::*;
pub use render::*;
pub use widgets::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug)]
pub struct ToolDiffSection {
    path: String,
    link_target: String,
    additions: u32,
    deletions: u32,
    lines: Vec<DiffLine>,
    omitted: usize,
    fingerprint: u64,
    /// LSP diagnostics for this file, parsed from the tool's `metadata.diagnostics`
    /// (opencode parity). Rendered as a footer beneath the diff card.
    diagnostics: Vec<DiagLine>,
}

/// A single LSP diagnostic line surfaced under a diff card, e.g.
/// `ERROR [12:5] cannot find value `foo``.
#[derive(Clone, Debug)]
struct DiagLine {
    is_error: bool,
    text: String,
}

/// Cap on diagnostics lines shown beneath one diff card before collapsing to a
/// "+N more" hint, so a badly-broken file can't blow up the card height.
const MAX_DIAG_LINES_PER_CARD: usize = 6;
/// Row height for a diagnostics footer line.
const DIAG_LINE_HEIGHT: f32 = 18.0;

#[derive(Clone)]
struct ToolWrappedRow {
    text: String,
    nested: bool,
}

fn tool_body_wrap_width(width: f32, expanded: bool, s: f32) -> f32 {
    let left = if expanded { 76.0 } else { 58.0 } * s;
    (width - left - 24.0 * s).max(80.0 * s)
}

fn tool_wrapped_rows(
    sugarloaf: &mut Sugarloaf,
    body: &str,
    width: f32,
    opts: &DrawOpts,
    limit: usize,
) -> Rc<Vec<ToolWrappedRow>> {
    let key = ToolWrapCacheKey {
        body_hash: hash_value(&body),
        body_len: body.len(),
        width_bits: width.to_bits(),
        font_size_bits: opts.font_size.to_bits(),
        bold: opts.bold,
        italic: opts.italic,
        limit,
    };
    if let Some(hit) = TOOL_WRAP_CACHE.with(|cache| cache.borrow().get(&key)) {
        return hit;
    }
    super::derivations::bump_tool_wrap();

    let mut rows = Vec::new();
    for line in body.lines() {
        if rows.len() >= limit {
            break;
        }
        let nested = line.starts_with("  ");
        let text = if nested { line.trim() } else { line.trim_end() };
        for wrapped in wrap_text(sugarloaf, text, width, opts, limit - rows.len()) {
            if rows.len() >= limit {
                break;
            }
            rows.push(ToolWrappedRow {
                text: wrapped,
                nested,
            });
        }
    }
    if rows.is_empty() {
        rows.push(ToolWrappedRow {
            text: String::new(),
            nested: false,
        });
    }
    let rows = Rc::new(rows);
    TOOL_WRAP_CACHE.with(|cache| cache.borrow_mut().insert(key, rows.clone()));
    rows
}

fn line_count_until(text: &str, limit: usize) -> usize {
    text.lines().take(limit).count()
}
