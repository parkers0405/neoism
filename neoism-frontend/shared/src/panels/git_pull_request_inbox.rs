//! Persistent PR inbox. All network operations share one flight because errors have no identity.
use super::{GitDiffView, GitDiffViewAction};
use crate::{
    layout::Rect,
    panels::git_pull_request::{Intent, Tab},
};
use neoism_protocol::pull_request::*;
use std::{
    collections::{BTreeMap, VecDeque},
    path::PathBuf,
};
#[path = "git_pull_request_inbox_format.rs"]
mod format;
#[path = "git_pull_request_inbox_list.rs"]
mod list_render;
#[path = "git_pull_request_inbox_render.rs"]
mod render;
use unicode_segmentation::UnicodeSegmentation;

// Static placeholders deliberately do not claim animation/redraw ownership or create hit targets.
pub(super) fn skeleton(
    sl: &mut sugarloaf::Sugarloaf,
    rect: Rect,
    clip: [f32; 4],
    occlusions: &[[f32; 4]],
    theme: &crate::primitives::IdeTheme,
    scale: f32,
    rows: usize,
) {
    for i in 0..rows {
        let y = rect.y + i as f32 * 26.0 * scale;
        let width = rect.w * [0.78, 0.94, 0.58][i % 3];
        for v in crate::primitives::visible_rect_fragments(
            [rect.x, y, width, 10.0 * scale],
            clip,
            occlusions,
        ) {
            sl.quad(
                None,
                v[0],
                v[1],
                v[2],
                v[3],
                theme.f32_alpha(theme.muted, 0.18),
                [0.0; 4],
                0.0,
                20,
            );
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Menu {
    Sort,
    Filter,
}
#[cfg(test)]
#[path = "git_pull_request_inbox_tests.rs"]
mod tests;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DetailTab {
    Summary,
    Timeline,
    Code,
}
#[derive(Clone, Debug)]
enum Pending {
    List { generation: u64, append: bool },
    Child { number: u64, mutation: bool },
}
#[derive(Clone)]
enum Hit {
    Select(u64),
    Search,
    Sort,
    Filter,
    More,
    Back,
    Tab(DetailTab),
    Browser(String),
    Checkout,
    Review,
    Refresh,
    RefreshList,
}
pub struct PullRequestInbox {
    root: PathBuf,
    pub query: String,
    pub sort: PullRequestSort,
    pub filter: PullRequestFilter,
    pub generation: u64,
    pub items: Vec<PullRequestInboxItem>,
    pub selected: Option<u64>,
    pub children: BTreeMap<u64, Box<GitDiffView>>,
    metadata: BTreeMap<u64, PullRequestInboxItem>,
    tabs: BTreeMap<u64, DetailTab>,
    pub next_cursor: Option<String>,
    pub total_count: u32,
    pub repository: String,
    pub viewer: String,
    pub list_error: Option<String>,
    detail_errors: BTreeMap<u64, String>,
    search_cursor: usize,
    search_offset: f32,
    menu: Option<Menu>,
    menu_index: usize,
    menu_anchor: Rect,
    menu_hits: Vec<(Rect, usize)>,
    pending: Option<Pending>,
    list: Option<Option<String>>,
    read: Option<u64>,
    queue: VecDeque<(u64, PullRequestRequest)>,
    out: Vec<GitDiffViewAction>,
    pub error: Option<String>,
    search_focus: bool,
    list_scroll: f32,
    list_max: f32,
    detail_scroll: f32,
    detail_max: f32,
    prelude_height: f32,
    child_rect: Rect,
    pointer: Option<(f32, f32)>,
    list_rect: Rect,
    detail_rect: Rect,
    hits: Vec<(Rect, Hit)>,
    occlusions: Vec<[f32; 4]>,
    narrow_detail: bool,
}
impl PullRequestInbox {
    pub fn new(root: PathBuf) -> Self {
        let mut s = Self {
            root,
            query: String::new(),
            sort: PullRequestSort::Updated,
            filter: PullRequestFilter::Open,
            generation: 1,
            items: vec![],
            selected: None,
            children: BTreeMap::new(),
            metadata: BTreeMap::new(),
            tabs: BTreeMap::new(),
            next_cursor: None,
            total_count: 0,
            repository: String::new(),
            viewer: String::new(),
            list_error: None,
            detail_errors: BTreeMap::new(),
            search_cursor: 0,
            search_offset: 0.0,
            menu: None,
            menu_index: 0,
            menu_anchor: Rect::new(0.0, 0.0, 0.0, 0.0),
            menu_hits: vec![],
            pending: None,
            list: Some(None),
            read: None,
            queue: VecDeque::new(),
            out: vec![],
            error: None,
            search_focus: false,
            list_scroll: 0.0,
            list_max: 0.0,
            detail_scroll: 0.0,
            detail_max: 0.0,
            prelude_height: 0.0,
            child_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            pointer: None,
            list_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            detail_rect: Rect::new(0.0, 0.0, 0.0, 0.0),
            hits: vec![],
            occlusions: vec![],
            narrow_detail: false,
        };
        s.pump();
        s
    }
    pub fn has_drafts(&self) -> bool {
        self.queue.iter().any(|(_,r)| r.is_mutation())
            || matches!(self.pending, Some(Pending::Child { mutation: true, .. }))
            || self.children.values().any(|c| {
                c.pull_request.as_ref().is_some_and(|p| p.has_drafts())
                    || c.pending_actions.iter().any(|a| matches!(a, GitDiffViewAction::PullRequest {request,..} if request.is_mutation()))
            })
    }
    pub fn refresh_list(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.list = Some(None);
        self.items.clear();
        self.next_cursor = None;
        self.list_error = None;
        self.list_scroll = 0.0;
        self.pump();
    }
    pub fn select(&mut self, number: u64) {
        self.clear_child_pointer();
        self.prelude_height = 0.0;
        self.detail_max = 0.0;
        self.selected = Some(number);
        self.narrow_detail = true;
        self.search_focus = false;
        self.detail_scroll = 0.0;
        if self.children.contains_key(&number)
            && self.detail_errors.remove(&number).is_some()
        {
            let c = self.children.get_mut(&number).unwrap();
            c.pull_request.as_mut().unwrap().message = None;
            c.pr_intent(Intent::Refresh);
            self.error = None;
        }
        if !self.children.contains_key(&number)
            && !matches!(self.pending, Some(Pending::Child { number: n, .. }) if n == number)
        {
            self.detail_errors.remove(&number);
            self.read = Some(number);
        }
        self.pump();
    }
    fn child(&mut self) -> Option<&mut GitDiffView> {
        self.children.get_mut(&self.selected?).map(Box::as_mut)
    }
    fn tab(&self) -> DetailTab {
        self.selected
            .and_then(|n| self.tabs.get(&n).copied())
            .unwrap_or(DetailTab::Summary)
    }
    fn collect(&mut self) {
        let drafts = self
            .children
            .values()
            .any(|c| c.pull_request.as_ref().is_some_and(|p| p.has_drafts()));
        for (&number, child) in &mut self.children {
            for action in child.take_actions() {
                match action {
                    GitDiffViewAction::PullRequest {
                        request: PullRequestRequest::Checkout { .. },
                        ..
                    } if drafts => {
                        let p = child.pull_request.as_mut().unwrap();
                        p.pending = false;
                        p.message = Some(
                            "Finish or discard all review drafts before checkout".into(),
                        );
                        self.error = p.message.clone();
                    }
                    GitDiffViewAction::PullRequest { request, .. } => {
                        self.queue.push_back((number, request))
                    }
                    GitDiffViewAction::OpenUrl { .. } => self.out.push(action),
                    _ => {}
                }
            }
        }
    }
    fn pump(&mut self) {
        self.collect();
        if self.pending.is_some() {
            return;
        }
        // Revalidate deferred checkout as drafts can be created while a list/read is in flight.
        if self
            .children
            .values()
            .any(|c| c.pull_request.as_ref().is_some_and(|p| p.has_drafts()))
        {
            let mut keep = VecDeque::new();
            for (number, request) in std::mem::take(&mut self.queue) {
                if matches!(request, PullRequestRequest::Checkout { .. }) {
                    if let Some(c) = self.children.get_mut(&number) {
                        let p = c.pull_request.as_mut().unwrap();
                        p.pending = false;
                        p.message = Some(
                            "Finish or discard all review drafts before checkout".into(),
                        );
                        self.error = p.message.clone();
                    }
                } else {
                    keep.push_back((number, request));
                }
            }
            self.queue = keep;
        }
        let request = if let Some((n, request)) = self.queue.pop_front() {
            self.pending = Some(Pending::Child {
                number: n,
                mutation: request.is_mutation(),
            });
            request
        } else if let Some(cursor) = self.list.take() {
            self.pending = Some(Pending::List {
                generation: self.generation,
                append: cursor.is_some(),
            });
            PullRequestRequest::List {
                query: self.query.clone(),
                sort: self.sort,
                filter: self.filter,
                cursor,
                generation: self.generation,
            }
        } else if let Some(number) = self.read.take() {
            if self.children.contains_key(&number) {
                return;
            }
            self.pending = Some(Pending::Child {
                number,
                mutation: false,
            });
            PullRequestRequest::ReadInbox { number }
        } else {
            return;
        };
        self.out.push(GitDiffViewAction::PullRequest {
            root: self.root.clone(),
            request,
        });
    }
    pub fn take_actions(&mut self) -> Vec<GitDiffViewAction> {
        self.pump();
        std::mem::take(&mut self.out)
    }
    pub fn apply(&mut self, reply: &PullRequestReply) {
        // A late/duplicate list page must not release a newer pending flight.
        if let PullRequestReply::Listed { page } = reply {
            if !matches!(self.pending, Some(Pending::List { generation, .. }) if generation == page.generation)
            {
                return;
            }
        }
        if let Some(Pending::Child {
            number: expected, ..
        }) = self.pending.as_ref()
        {
            let actual = match reply {
                PullRequestReply::InboxDetail { detail, .. }
                | PullRequestReply::Detail { detail } => Some(detail.summary.number),
                PullRequestReply::ReviewSubmitted { number }
                | PullRequestReply::ThreadUpdated { number }
                | PullRequestReply::CheckedOut { number, .. }
                | PullRequestReply::Merged { number } => Some(*number),
                PullRequestReply::Error { .. } => None,
                _ => return,
            };
            if actual.is_some_and(|n| n != *expected) {
                return;
            }
        }
        let Some(pending) = self.pending.take() else {
            return;
        };
        match pending {
            Pending::List { generation, append } => {
                if generation == self.generation {
                    match reply {
                        PullRequestReply::Listed { page } => {
                            self.repository = page.repository.clone();
                            self.viewer = page.viewer.clone();
                            if !append {
                                self.items.clear();
                            }
                            for item in &page.items {
                                let n = item.summary.number;
                                self.metadata.insert(n, item.clone());
                                if let Some(old) =
                                    self.items.iter_mut().find(|i| i.summary.number == n)
                                {
                                    *old = item.clone();
                                } else {
                                    self.items.push(item.clone());
                                }
                            }
                            self.next_cursor = page.next_cursor.clone();
                            self.total_count = page.total_count;
                            self.list_error = None;
                        }
                        PullRequestReply::Error { message, .. } => {
                            self.list_error = Some(message.clone())
                        }
                        _ => {}
                    }
                }
            }
            Pending::Child { number, .. } => match reply {
                PullRequestReply::InboxDetail { detail, item }
                    if detail.summary.number == number =>
                {
                    self.metadata.insert(number, item.clone());
                    if let Some(c) = self.children.get_mut(&number) {
                        c.apply_pull_request_reply(&PullRequestReply::Detail {
                            detail: detail.clone(),
                        });
                    } else {
                        let mut c = GitDiffView::from_pull_request(
                            self.root.clone(),
                            detail.clone(),
                        );
                        c.pull_request.as_mut().unwrap().embedded = true;
                        self.children.insert(number, Box::new(c));
                    }
                    self.detail_errors.remove(&number);
                    self.error = None;
                }
                _ => {
                    if let PullRequestReply::Detail { detail } = reply {
                        if let Some(item) = self.metadata.get_mut(&number) {
                            item.summary = detail.summary.clone();
                        }
                    }
                    if let Some(c) = self.children.get_mut(&number) {
                        c.apply_pull_request_reply(reply);
                    }
                    if let PullRequestReply::Error { message, .. } = reply {
                        self.detail_errors.insert(number, message.clone());
                    } else {
                        self.detail_errors.remove(&number);
                        self.error = None;
                    }
                }
            },
        }
        self.pump();
    }
    pub fn text_input(&mut self, text: &str) -> bool {
        if self.search_focus {
            self.search_cursor = self.search_cursor.min(self.query.len());
            while !self.query.is_char_boundary(self.search_cursor) {
                self.search_cursor -= 1;
            }
            let text = text.replace(['\n', '\r'], " ");
            self.query.insert_str(self.search_cursor, &text);
            self.search_cursor += text.len();
            self.refresh_list();
            true
        } else {
            self.child()
                .is_some_and(|c| c.pull_request_text_input(text))
        }
    }
    pub fn key(&mut self, key: &str, shift: bool, primary: bool) -> bool {
        if let Some(menu) = self.menu {
            let len = if menu == Menu::Sort { 3 } else { 4 };
            match key {
                "Escape" => self.menu = None,
                "ArrowDown" | "Down" => self.menu_index = (self.menu_index + 1) % len,
                "ArrowUp" | "Up" => self.menu_index = (self.menu_index + len - 1) % len,
                "Enter" => self.choose_menu(self.menu_index),
                _ => {}
            }
            return true;
        }
        if self.search_focus {
            let mut cursor = self.search_cursor.min(self.query.len());
            while !self.query.is_char_boundary(cursor) {
                cursor -= 1;
            }
            self.search_cursor = cursor;
            let prev = self
                .query
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .take_while(|i| *i < cursor)
                .last()
                .unwrap_or(0);
            let next = self
                .query
                .grapheme_indices(true)
                .map(|(i, _)| i)
                .find(|i| *i > cursor)
                .unwrap_or(self.query.len());
            match key {
                "Backspace" if cursor > 0 => {
                    self.query.replace_range(prev..cursor, "");
                    self.search_cursor = prev;
                    self.refresh_list();
                }
                "Delete" if cursor < self.query.len() => {
                    self.query.replace_range(cursor..next, "");
                    self.refresh_list();
                }
                "ArrowLeft" | "Left" => self.search_cursor = prev,
                "ArrowRight" | "Right" => self.search_cursor = next,
                "Home" => self.search_cursor = 0,
                "End" => self.search_cursor = self.query.len(),
                "Escape" | "Enter" => self.search_focus = false,
                _ => {}
            }
            return true;
        }
        if key == "Escape" {
            if let Some(c) = self.child() {
                if c.pull_request.as_ref().is_some_and(|p| p.sheet) {
                    return c.pull_request_key(key, shift, primary);
                }
            }
            self.narrow_detail = false;
            return true;
        }
        if key == "Tab"
            && !self
                .selected
                .and_then(|n| self.children.get(&n))
                .is_some_and(|c| c.pull_request.as_ref().is_some_and(|p| p.sheet))
        {
            let tabs = [DetailTab::Summary, DetailTab::Timeline, DetailTab::Code];
            let i = tabs.iter().position(|t| *t == self.tab()).unwrap_or(0);
            let t = tabs[(i + if shift { 2 } else { 1 }) % 3];
            self.detail_scroll = 0.0;
            self.clear_child_pointer();
            if let Some(n) = self.selected {
                self.tabs.insert(n, t);
            }
            if let Some(c) = self.child() {
                c.pull_request.as_mut().unwrap().focus = None;
                c.pull_request.as_mut().unwrap().tab = if t == DetailTab::Timeline {
                    Tab::Conversation
                } else {
                    Tab::Changes
                };
            }
            return true;
        }
        let child_editing = self
            .selected
            .and_then(|n| self.children.get(&n))
            .is_some_and(|c| c.pull_request.as_ref().is_some_and(|p| p.focus.is_some()));
        if !primary
            && !shift
            && !self.sheet_open()
            && !child_editing
            && self.selected.is_some()
        {
            let delta = match key {
                "ArrowDown" | "Down" | "j" => Some(60.0),
                "ArrowUp" | "Up" | "k" => Some(-60.0),
                "PageDown" | " " => Some(self.detail_rect.h * 0.9),
                "PageUp" => Some(-self.detail_rect.h * 0.9),
                "Home" | "g" => Some(-f32::MAX),
                "End" | "G" => Some(f32::MAX),
                _ => None,
            };
            if let Some(delta) = delta {
                self.scroll_detail(delta);
                return true;
            }
        }
        if self.tab() != DetailTab::Summary {
            return self.child().is_some_and(|c| {
                if primary || shift {
                    c.pull_request_key(key, shift, primary)
                } else {
                    c.key(key)
                }
            });
        }
        false
    }
    fn list_loading(&self) -> bool {
        self.list_error.is_none()
            && (self.list.is_some() || matches!(self.pending, Some(Pending::List { .. })))
    }
    fn detail_loading(&self, number: u64) -> bool {
        !self.detail_errors.contains_key(&number)
            && !self.children.get(&number).is_some_and(|c| {
                c.pull_request.as_ref().is_some_and(|p| p.detail.is_some())
            })
    }
    fn sheet_open(&self) -> bool {
        self.selected
            .and_then(|n| self.children.get(&n))
            .is_some_and(|c| c.pull_request.as_ref().is_some_and(|p| p.sheet))
    }
    fn child_visible_at(&self, x: f32, y: f32) -> bool {
        self.tab() != DetailTab::Summary
            && self.child_rect.w > 0.0
            && self.child_rect.h > 0.0
            && self.child_rect.contains(x, y)
    }
    fn clear_child_pointer(&mut self) {
        let detail = self.detail_rect;
        self.hits.retain(|(r, _)| !detail.contains(r.x, r.y));
        self.child_rect = Rect::new(0.0, 0.0, 0.0, 0.0);
        if let Some(c) = self.child() {
            c.pointer_leave();
        }
    }
    fn child_scroll_position(c: &GitDiffView) -> f32 {
        if let Some(p) = &c.pull_request {
            if p.sheet || p.detail.is_none() || p.tab != Tab::Changes {
                return p.scroll;
            }
        }
        c.scroll_top
    }
    fn scroll_detail(&mut self, mut delta: f32) {
        if self.sheet_open() {
            if let Some(c) = self.child() {
                c.scroll(delta);
            }
            return;
        }
        if self.tab() == DetailTab::Summary {
            let before = self.detail_scroll;
            self.detail_scroll = (before + delta).clamp(0.0, self.detail_max);
            if self.detail_scroll != before {
                self.clear_child_pointer();
            }
            return;
        }
        // A resize or shorter header can shrink the prelude while it is hidden.
        self.detail_scroll = self.detail_scroll.clamp(0.0, self.prelude_height);
        // Reverse the chain: exhaust the child before revealing the document prelude.
        if delta < 0.0 {
            if let Some(c) = self.child() {
                let before = Self::child_scroll_position(c);
                c.scroll(delta);
                delta -= Self::child_scroll_position(c) - before;
            }
        }
        let before = self.detail_scroll;
        self.detail_scroll = (before + delta).clamp(0.0, self.prelude_height);
        delta -= self.detail_scroll - before;
        let old_height =
            (self.detail_rect.h - (self.prelude_height - before).max(0.0)).max(0.0);
        let new_height = (self.detail_rect.h
            - (self.prelude_height - self.detail_scroll).max(0.0))
        .max(0.0);
        let growth = new_height - old_height;
        if let Some(c) = self.child() {
            // Account for the actual visible viewport change before forwarding residual input.
            if let Some(p) = &mut c.pull_request {
                if p.tab != Tab::Changes || p.detail.is_none() {
                    p.max_scroll = (p.max_scroll - growth).max(0.0);
                    p.scroll = p.scroll.min(p.max_scroll);
                } else {
                    c.max_scroll = (c.max_scroll - growth).max(0.0);
                    c.scroll_top = c.scroll_top.min(c.max_scroll);
                }
            }
            if delta > 0.0 {
                c.scroll(delta);
            }
        }
        if self.detail_scroll != before {
            self.clear_child_pointer();
        }
    }
    pub fn scroll(&mut self, delta: f32) {
        if self.menu.is_some() {
            return;
        }
        if !delta.is_finite() {
            return;
        }
        if self
            .pointer
            .is_some_and(|(x, y)| self.detail_rect.contains(x, y))
        {
            self.scroll_detail(delta);
        } else {
            self.list_scroll = (self.list_scroll + delta).clamp(0.0, self.list_max);
        }
    }
    pub fn pointer_move(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        if self.menu.is_some() {
            if let Some((_, i)) = self.menu_hits.iter().find(|(r, _)| r.contains(x, y)) {
                let changed = self.menu_index != *i;
                self.menu_index = *i;
                return changed;
            }
            return false;
        }
        if self.child_visible_at(x, y) || self.is_dragging() {
            if let Some(c) = self.child() {
                let before = Self::child_scroll_position(c);
                let changed = c.pointer_move(x, y);
                let advanced = Self::child_scroll_position(c) > before;
                if advanced && !self.sheet_open() {
                    self.scroll_detail(
                        (self.prelude_height - self.detail_scroll).max(0.0),
                    );
                }
                return changed;
            }
        }
        self.child().is_some_and(|c| c.pointer_leave())
    }
    fn open_menu(&mut self, menu: Menu) {
        if self.menu == Some(menu) {
            self.menu = None;
            return;
        }
        self.menu = Some(menu);
        self.menu_index = match menu {
            Menu::Sort => format::SORTS
                .iter()
                .position(|v| *v == self.sort)
                .unwrap_or(0),
            Menu::Filter => format::FILTERS
                .iter()
                .position(|v| *v == self.filter)
                .unwrap_or(0),
        };
    }
    fn choose_menu(&mut self, index: usize) {
        let changed = match self.menu.take() {
            Some(Menu::Sort) => format::SORTS.get(index).is_some_and(|v| {
                let changed = self.sort != *v;
                self.sort = *v;
                changed
            }),
            Some(Menu::Filter) => format::FILTERS.get(index).is_some_and(|v| {
                let changed = self.filter != *v;
                self.filter = *v;
                changed
            }),
            None => false,
        };
        self.menu_hits.clear();
        if changed {
            self.refresh_list();
        }
    }
    fn detail_ready(&self) -> bool {
        self.selected
            .and_then(|n| self.children.get(&n))
            .is_some_and(|c| {
                c.pull_request
                    .as_ref()
                    .is_some_and(|p| p.detail.is_some() && !p.pending)
            })
    }
    pub fn pointer_down(&mut self, x: f32, y: f32) -> bool {
        self.pointer = Some((x, y));
        if self
            .occlusions
            .iter()
            .any(|r| Rect::new(r[0], r[1], r[2], r[3]).contains(x, y))
        {
            return false;
        }
        if self.menu.is_some() {
            if let Some((_, i)) = self.menu_hits.iter().find(|(r, _)| r.contains(x, y)) {
                self.choose_menu(*i);
            } else {
                self.menu = None;
                self.menu_hits.clear();
            }
            return true;
        }
        if self.sheet_open() && self.detail_rect.contains(x, y) {
            return self.child().is_some_and(|c| c.pointer_down(x, y));
        }
        if let Some((_, hit)) = self.hits.iter().find(|(r, _)| r.contains(x, y)) {
            let hit = hit.clone();
            self.search_focus = false;
            match hit {
                Hit::Select(n) => self.select(n),
                Hit::Search => {
                    self.search_focus = true;
                    self.search_cursor = self.query.len();
                }
                Hit::Sort => self.open_menu(Menu::Sort),
                Hit::Filter => self.open_menu(Menu::Filter),
                Hit::RefreshList => {
                    self.menu = None;
                    self.refresh_list();
                }
                Hit::More => {
                    if self.list.is_none() {
                        self.list = self.next_cursor.clone().map(Some);
                        self.pump();
                    }
                }
                Hit::Back => self.narrow_detail = false,
                Hit::Tab(t) => {
                    self.clear_child_pointer();
                    if let Some(n) = self.selected {
                        self.tabs.insert(n, t);
                    }
                    self.detail_scroll = 0.0;
                    if let Some(c) = self.child() {
                        let p = c.pull_request.as_mut().unwrap();
                        p.tab = if t == DetailTab::Timeline {
                            Tab::Conversation
                        } else {
                            Tab::Changes
                        };
                        p.focus = None;
                    }
                }
                Hit::Browser(url) => self.out.push(GitDiffViewAction::OpenUrl { url }),
                Hit::Checkout => {
                    if !self.detail_ready() {
                        return true;
                    }
                    if self.has_drafts() {
                        self.error = Some(
                            "Finish or discard all review drafts before checkout".into(),
                        );
                    } else if let Some(c) = self.child() {
                        c.pr_intent(Intent::Checkout(false));
                    }
                }
                Hit::Review => {
                    if !self.detail_ready() {
                        return true;
                    }
                    if let Some(n) = self.selected {
                        self.tabs.insert(n, DetailTab::Code);
                    }
                    if let Some(c) = self.child() {
                        c.pull_request.as_mut().unwrap().tab = Tab::Changes;
                        c.pr_intent(Intent::Review);
                    }
                }
                Hit::Refresh => {
                    if let Some(n) = self.selected {
                        self.detail_errors.remove(&n);
                    }
                    self.error = None;
                    if let Some(c) = self.child() {
                        if !c.pull_request.as_ref().is_some_and(|p| p.pending) {
                            c.pull_request.as_mut().unwrap().message = None;
                        }
                        c.pr_intent(Intent::Refresh);
                    } else if let Some(n) = self.selected {
                        self.select(n);
                    }
                    self.pump();
                }
            }
            return true;
        }
        self.search_focus = false;
        if self.child_visible_at(x, y) {
            return self.child().is_some_and(|c| c.pointer_down(x, y));
        }
        false
    }
    pub fn text_cursor(&self, x: f32, y: f32) -> bool {
        if self.menu.is_some() {
            return false;
        }
        self.hits
            .iter()
            .any(|(r, h)| matches!(h, Hit::Search) && r.contains(x, y))
    }
    pub fn horizontal_delta(&mut self, delta: f32) {
        if let Some((x, y)) = self.pointer {
            self.horizontal(x, y, delta);
        }
    }
    pub fn pointer_cursor(&self, x: f32, y: f32) -> bool {
        if self.menu.is_some() {
            return self.menu_hits.iter().any(|(r, _)| r.contains(x, y));
        }
        self.hits.iter().any(|(r, _)| r.contains(x, y))
            || (self.child_visible_at(x, y)
                && self
                    .selected
                    .and_then(|n| self.children.get(&n))
                    .is_some_and(|c| c.pointer_cursor(x, y)))
    }
    pub fn set_occlusions(&mut self, rects: &[[f32; 4]]) {
        self.occlusions = rects.to_vec();
        for c in self.children.values_mut() {
            c.set_text_occlusions(rects);
        }
    }
    pub fn horizontal(&mut self, x: f32, y: f32, delta: f32) {
        if self.menu.is_none() && self.child_visible_at(x, y) {
            if let Some(c) = self.child() {
                c.scroll_horizontal_at(x, y, delta);
            }
        }
    }
    pub fn is_dragging(&self) -> bool {
        self.selected
            .and_then(|n| self.children.get(&n))
            .is_some_and(|c| c.is_dragging())
    }
    pub fn needs_redraw(&self) -> bool {
        self.tab() != DetailTab::Summary
            && self
                .selected
                .and_then(|n| self.children.get(&n))
                .is_some_and(|c| c.needs_redraw())
    }
    pub fn pointer_up(&mut self) -> bool {
        self.child().is_some_and(|c| c.pointer_up())
    }
    pub fn pointer_leave(&mut self) -> bool {
        self.pointer = None;
        self.child().is_some_and(|c| c.pointer_leave())
    }
}
