//! The Neoism agent: conversation state fed by the daemon's `Agent` family, and its surfaces
//! (sidebar, transcript, composer). References: `rune/reference/04..13*.png`.

pub mod mcp;

mod activity;
mod anchor;
pub mod attach;
mod composer;
pub(crate) mod prompts;
#[path = "../../../../neoism-frontend/shared/src/panels/agent_pane/question_policy.rs"]
mod question_policy;
pub(crate) mod family;
mod header;
mod projects;
mod prose;
mod resync;
mod sidebar;
mod table;
mod ticks;
pub mod slash;
mod user;
pub mod timeline;
pub mod tools;

pub use activity::Activity;
pub use composer::overlay;
pub use family::Families;
pub use header::Step as HeaderStep;
pub use projects::{ProjectMenu, view as project_menu};
pub use sidebar::{ConvHeader, catalog_scope, conversations};
pub use timeline::pane;

use crate::app::App;
use crate::daemon::Family;
use crate::tabs::TabKind;
use neoism_protocol::agent::{
    AgentClientMessage, AgentServerMessage, HistoryMessage, HistoryMessageKind, PromptDelivery,
    Role, StreamingState, ThreadSummary,
};
use serde_json::{Value, json};

/// Older pages are sized by bytes, as in the GPUiX fix (`rune/README.md`, data rules).
const OLDER_PAGE_TARGET_BYTES: usize = 160 * 1024;
const HEAD_PAGE: u32 = 80;

/// How often a streaming reply's new text re-runs the views.
///
/// A provider sends text as fast as it writes it, a delta every few milliseconds from a fast
/// model, and each one applied its own commit: a commit a frame for as long as the reply ran.
/// The text is applied as it arrives; the views show it at most `StreamPace::RATE` times a
/// second (and at once with anything else that re-runs them).
#[derive(Default)]
pub struct StreamPace {
    /// Text arrived that no view run has shown yet.
    pub pending: bool,
    shown: Option<std::time::Instant>,
}

impl StreamPace {
    /// Thirty view runs a second: a new batch of words every 33 ms.
    const INTERVAL: std::time::Duration = std::time::Duration::from_millis(33);

    /// Whether pending text is due a view run now, else how long until it is.
    pub fn due(&self, now: std::time::Instant) -> (bool, Option<std::time::Duration>) {
        if !self.pending { return (false, None); }
        match self.shown.map(|shown| now.saturating_duration_since(shown)) {
            Some(since) if since < Self::INTERVAL => (false, Some(Self::INTERVAL - since)),
            _ => (true, None),
        }
    }

    /// The views ran: whatever text was pending is on screen.
    pub fn ran(&mut self, now: std::time::Instant) {
        if std::mem::take(&mut self.pending) { self.shown = Some(now); }
    }
}

#[derive(Default)]
pub struct Session {
    /// Streaming parts share their assistant message's identity even when delivered out of order.
    pub part_parents: std::collections::HashMap<String, String>,
    /// Each message's revision: bumped whenever anything about it changes. Transcript rows
    /// are cached by it (`rune::memo`).
    pub message_versions: std::collections::HashMap<String, u64>,
    /// Bumped whenever the transcript's row list may have changed (a message arrived or
    /// changed; not a reply's text growing). With the message count it keys `rows`.
    pub structure: u64,
    /// The transcript's row list as last computed (`Session::row_items`).
    pub rows: std::cell::RefCell<Option<(timeline::RowsKey, std::rc::Rc<Vec<timeline::RowItem>>)>>,
    pub scroll: std::rc::Rc<std::cell::Cell<(f32, f32)>>,
    /// The wheel or trackpad glide's target and when it was set (`follow::wheel_base`).
    pub wheel_target: Option<(f32, std::time::Instant)>,
    /// Where the transcript was scrolled when it last showed, unless it was at its end. A
    /// conversation that is not showing (a sub-agent is, or another tab) has no scroll view;
    /// the one rebuilt for it starts at the end and is put back here.
    pub kept_scroll: Option<f32>,
    /// The conversation was on screen at the last frame.
    pub shown: bool,
    /// Frames left of putting `kept_scroll` back.
    pub restoring: u8,
    /// When the reader last moved up the transcript: older history loads ahead of someone
    /// heading toward it, never on its own (`anchor.rs`).
    pub scrolled_up_at: Option<std::time::Instant>,
    /// Where each mounted row was last drawn (top below the viewport's top, height), the row
    /// a moving view is held by, and its last placement (`anchor.rs`).
    pub row_tops: std::collections::HashMap<String, (f32, f32)>,
    pub anchor: Option<anchor::Anchor>,
    pub placed: Option<anchor::Placed>,
    /// Questions whose rows are in view: only their `question_tops` are current.
    pub visible_questions: std::collections::HashSet<String>,
    /// Each question's row offset in the transcript, from its last layout (the question rail).
    pub question_tops: std::collections::HashMap<String, f32>,
    /// The question rail's lit tick, and the transcript viewport's height.
    pub rail_active: Option<usize>,
    /// The transcript is scrolled away from its end: the jump-to-latest button shows.
    pub away: bool,
    pub viewport_h: f32,
    pub width: f32,
    pub messages: Vec<HistoryMessage>,
    /// Unconfirmed echoes, retained for queued admission and server-echo reconciliation.
    pub pending_prompts: std::collections::HashMap<String, HistoryMessage>,
    pub older_cursor: Option<String>,
    pub loaded: bool,
    /// What is loaded may be behind the conversation (the window was away from it): its
    /// newest page is on its way, or is fetched when it is next wanted (`resync.rs`).
    pub resync: bool,
    pub loading_older: bool,
    pub older_limit: u32,
    pub streaming: bool,
    /// When the running turn began: the server's time when it reports one (`QueueUpdate`),
    /// else when this client saw it start. It belongs to the conversation, not to the status
    /// line, so the timer counts the whole turn whether or not it is on screen.
    pub turn_started: Option<std::time::Instant>,
    pub queued_prompts: u32,
    pub status: Option<String>,
    pub compacting: bool,
    pub selection: Option<slash::Selection>,
    /// First message of a turn that streamed while open here; its tool and thinking rows stay
    /// visible. Reloaded history shows finished turns as prompt, answer and footer only.
    pub live_from: Option<usize>,
    /// The message `live_from` stood on (the turn's prompt). Rows are added above it when older
    /// history loads and may move when the history's order is applied: the index is found
    /// again from this, so finished turns never slide into the live window
    /// (`timeline_live_trace_anchor`).
    pub live_anchor: Option<String>,
    /// The session's context window from `ProviderState`, when the daemon knows it.
    pub context_limit: Option<u64>,
}

impl Session {
    fn hide_pending_prompts(&mut self) {
        let before = self.messages.len();
        self.messages.retain(|m| !self.pending_prompts.contains_key(&m.id));
        if self.messages.len() != before {
            self.structure += 1;
            self.rebase_live();
        }
    }

    fn order_stream_parts(&mut self, part: &str, parent: Option<String>) {
        if let Some(parent) = parent { self.part_parents.insert(part.to_owned(), parent); }
        let Some(parent) = self.part_parents.get(part) else { return };
        let slots: Vec<usize> = self.messages.iter().enumerate().filter(|(_, m)|
            self.part_parents.get(&m.id) == Some(parent)
                && matches!(m.kind, HistoryMessageKind::Reasoning | HistoryMessageKind::Assistant))
            .map(|(i, _)| i).collect();
        if slots.windows(2).all(|pair| self.messages[pair[0]].kind == HistoryMessageKind::Reasoning
            || self.messages[pair[1]].kind != HistoryMessageKind::Reasoning) { return; }
        let mut rows: Vec<_> = slots.iter().map(|&i| self.messages[i].clone()).collect();
        rows.sort_by_key(|m| m.kind != HistoryMessageKind::Reasoning);
        let mut changed = false;
        for (i, row) in slots.into_iter().zip(rows) {
            changed |= self.messages[i].id != row.id;
            self.messages[i] = row;
        }
        if changed { self.structure += 1; self.rebase_live(); }
    }

    /// Find the live turn again after rows were added or moved. With no anchor yet (the turn
    /// began before its prompt arrived), the first prompt at or after the old index becomes it.
    fn rebase_live(&mut self) {
        let Some(from) = self.live_from else { return };
        let found = match &self.live_anchor {
            Some(id) => self.messages.iter().position(|m| &m.id == id),
            None => self.messages.iter().enumerate().skip(from).find(|(_, m)| m.kind == HistoryMessageKind::User).map(|(i, _)| i),
        };
        if let Some(index) = found {
            if self.live_anchor.is_none() { self.live_anchor = Some(self.messages[index].id.clone()); }
            if index != from { self.live_from = Some(index); self.structure += 1; }
        }
    }

    /// Replace the transcript with the conversation's newest page (`resync.rs`): it shows
    /// its end, and older history loads from `older_cursor` as on a first load. Prompts typed
    /// here that the page does not know yet (still on their way) stay last.
    fn restart_at(&mut self, mut page: Vec<HistoryMessage>, older_cursor: Option<String>) {
        let known: std::collections::HashSet<&str> = page.iter().map(|m| m.id.as_str()).collect();
        let unsent = self.messages.iter().rev().take_while(|m| m.id.starts_with("msg_rune_")).filter(|m| !known.contains(m.id.as_str())).count();
        let at = self.messages.len() - unsent;
        page.extend(self.messages.drain(at..));
        self.messages = page;
        self.older_cursor = older_cursor;
        self.older_limit = 24;
        self.loading_older = false;
        self.structure += 1;
        // A turn at work keeps showing its trace, from its prompt.
        self.live_anchor = None;
        self.live_from = self.live_from.map(|_| self.messages.iter().rposition(|m| m.kind == HistoryMessageKind::User).unwrap_or(0));
        self.rebase_live();
        // Nothing measured against the old rows holds; the view goes to the end.
        self.row_tops.clear();
        self.question_tops.clear();
        self.visible_questions.clear();
        self.anchor = None;
        self.placed = None;
        self.wheel_target = None;
        self.kept_scroll = None;
        self.restoring = 3;
    }

    /// Whether the transcript is only now coming on screen: not shown a frame ago, or still
    /// being put where it was left (`App::conversations_keep_scroll`).
    pub(crate) fn arriving(&self) -> bool { !self.shown || self.restoring > 0 }

    fn note_message(&mut self, message: &HistoryMessage) {
        // Any difference counts: a tool row draws its title, detail, todos and children too.
        let changed = self.messages.iter().find(|m| m.id == message.id).is_none_or(|old| old != message);
        if changed {
            *self.message_versions.entry(message.id.clone()).or_default() += 1;
            self.structure += 1;
        }
    }
}

/// A task card that says its sub-agent is at work, when it is not, says how it ended. The
/// server never settles the card of a background sub-agent: it reads `running` in every
/// update and history page from then on. A child nothing has said anything about yet is left
/// alone while its conversation's turn is live (the card arrives before the child is named).
fn settle_task_card(families: &family::Families, message: &mut HistoryMessage, live_turn: bool) -> bool {
    if message.tool != "task" || !matches!(message.status.as_str(), "" | "pending" | "running" | "streaming") { return false; }
    let Some(child) = family::task_id(&message.detail, &message.text) else { return false };
    let failed = match families.stopped(&child) {
        Some(failed) => failed,
        None if live_turn || families.working_now(&child) => return false,
        None => false,
    };
    message.status = if failed { "error" } else { "completed" }.into();
    true
}

impl App {
    fn send_agent(&self, message: AgentClientMessage) {
        #[cfg(test)]
        self.agent_sent.borrow_mut().push(serde_json::to_string(&message).unwrap_or_default());
        if let Some(daemon) = &self.daemon { daemon.agent(&message); }
    }

    /// The conversation catalog for this root: one project's chats, or every project's at
    /// the `~` / `/` dashboard (`workspace_catalog_scope`).
    pub fn refresh_threads(&self) {
        let directory = sidebar::catalog_scope(&self.cwd);
        self.send_agent(AgentClientMessage::ListThreads { directory, limit: Some(50), cursor: None });
    }

    pub(crate) fn agent_connected(&mut self) {
        self.refresh_threads();
        if let Some(daemon) = &self.daemon {
            daemon.send(Family::Agent, json!({"GetConfigDefaults": {"directory": self.cwd}}));
        }
        self.slash_connected();
    }

    pub fn open_conversation(&mut self, session_id: &str) {
        if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Agent { session: Some(session_id.into()) }) {
            self.select_tab(self.tabs[index].id);
            return;
        }
        // A tab already showing this conversation's family (one of its sub-agents, or the
        // conversation while a sub-agent is asked for) switches in place: one tab per family.
        let root = self.families.root_of(session_id).to_owned();
        let family = self.tabs.iter().find(|t| matches!(&t.kind, TabKind::Agent { session: Some(id) } if self.families.root_of(id) == root)).map(|t| t.id);
        if let Some(tab) = family {
            self.select_tab(tab);
            self.view_session(session_id);
            return;
        }
        self.open_tab(TabKind::Agent { session: Some(session_id.into()) });
        self.load_session(session_id);
    }

    /// A restored conversation tab (`session.rs`) loads like an opened one.
    pub(crate) fn load_session_history(&mut self, session_id: &str) { self.load_session(session_id); }

    pub(crate) fn release_closed_agent(&mut self, session_id: &str) {
        let root = self.families.root_of(session_id).to_owned();
        if self.tabs.iter().any(|tab| matches!(&tab.kind, TabKind::Agent { session: Some(id) } if self.families.root_of(id) == root)) { return; }
        if let Some(owner) = self.stream_owner.remove(&root) {
            self.send_agent(AgentClientMessage::StopStream { session_id: owner });
        }
        let extra: Vec<String> = self.stream_extra.iter().filter(|id| self.families.root_of(id) == root).cloned().collect();
        for id in extra {
            self.stream_extra.remove(&id);
            self.send_agent(AgentClientMessage::StopStream { session_id: id });
        }
        self.sessions.retain(|id, _| self.families.root_of(id) != root);
    }

    /// Fetch a session's newest page and follow its stream, once.
    fn load_session(&mut self, session_id: &str) {
        let session = self.sessions.entry(session_id.into()).or_default();
        if session.loaded {
            // Loaded before the window was away and not caught up since: now it is wanted.
            if session.resync && !self.prefetching.contains(session_id) { self.refresh_transcript(session_id); }
            self.preload_family_history();
            return;
        }
        session.older_limit = 24;
        self.follow_family(session_id);
        self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: None, limit: Some(HEAD_PAGE) });
        self.preload_family_history();
    }

    /// Follow the stream that carries `session_id`'s events. One stream carries a whole family
    /// (the agent sends every descendant's events on its conversation's), and the daemon binds
    /// a stream to a session on `SwitchThread` as much as on `ResumeStream`: either one, sent
    /// for a sub-agent whose conversation is already followed, delivers each of the
    /// sub-agent's events twice. Its reply then streamed with every word doubled.
    fn follow_family(&mut self, session_id: &str) {
        let root = self.families.root_of(session_id).to_owned();
        let owner = self.stream_owner.get(&root).cloned();
        match owner {
            // The conversation itself is followed, which covers this session; or this session
            // is the one already followed.
            Some(owner) if owner == root || owner == session_id => {}
            // Only a sub-agent of it is, and this is another one: the first one's stream does
            // not carry it.
            Some(_) if session_id != root => {
                self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.into() });
                self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.into() });
                self.stream_extra.insert(session_id.to_owned());
            }
            // The conversation, or the first of the family to open.
            _ => {
                self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.into() });
                self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.into() });
                if let Some(previous) = self.stream_owner.insert(root.clone(), session_id.to_owned()) { self.stream_extra.insert(previous); }
                if session_id == root { self.drop_child_streams(&root); }
            }
        }
    }

    /// Ask the daemon for a conversation's settings (`SwitchThread`), which also binds its
    /// stream: one already followed is only asked, another is followed properly.
    pub(crate) fn switch_conversation(&mut self, session_id: &str) {
        if self.stream_owner.get(session_id).is_some_and(|owner| owner == session_id) {
            self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.into() });
        } else {
            self.follow_family(session_id);
        }
    }

    /// The conversation `root` is followed: the streams bound to its sub-agents before that
    /// (opened first, or before anything said whose they were) would repeat its events.
    fn drop_child_streams(&mut self, root: &str) {
        let extra: Vec<String> = self.stream_extra.iter().filter(|id| id.as_str() != root && self.families.root_of(id) == root).cloned().collect();
        for id in extra {
            self.stream_extra.remove(&id);
            self.send_agent(AgentClientMessage::StopStream { session_id: id });
        }
    }

    /// The families changed: a session followed as a conversation of its own may have turned
    /// out to be a sub-agent of one that is followed too.
    fn fold_streams(&mut self) {
        let children: Vec<String> = self.stream_owner.keys().filter(|key| self.families.root_of(key) != key.as_str()).cloned().collect();
        for key in children {
            let Some(owner) = self.stream_owner.remove(&key) else { continue };
            let root = self.families.root_of(&key).to_owned();
            match self.stream_owner.get(&root) {
                Some(_) => { self.stream_extra.insert(owner); }
                None => { self.stream_owner.insert(root.clone(), owner); }
            }
        }
        let followed: Vec<String> = self.stream_owner.iter().filter(|(root, owner)| root == owner).map(|(root, _)| root.clone()).collect();
        for root in followed { self.drop_child_streams(&root); }
    }

    /// Fetch the history of every sub-agent in the families that are open, ahead of a click
    /// (`preloadFamilyHistory`): opening one then shows it at once. A few at a time.
    pub(crate) fn preload_family_history(&mut self) {
        const IN_FLIGHT: usize = 4;
        const PAGE: u32 = 200;
        let roots: Vec<String> = self.tabs.iter().filter_map(|t| match &t.kind { TabKind::Agent { session: Some(id) } => Some(self.families.root_of(id).to_owned()), _ => None }).collect();
        let mut wanted = Vec::new();
        for root in roots {
            for member in self.families.roster(&root) {
                let known = self.sessions.get(&member.id).is_some_and(|s| s.loaded && (!s.resync || s.loading_older)) || self.prefetching.contains(&member.id);
                if !known && !wanted.contains(&member.id) { wanted.push(member.id.clone()); }
            }
        }
        for id in wanted {
            if self.prefetching.len() >= IN_FLIGHT { break; }
            self.prefetching.insert(id.clone());
            let session = self.sessions.entry(id.clone()).or_default();
            if !session.loaded { session.older_limit = 24; }
            self.send_agent(AgentClientMessage::GetHistory { session_id: id, cursor: None, limit: Some(PAGE) });
        }
    }

    /// A sub-agent that stopped settles the task card that launched it. The card's own update
    /// arrives with the parent's next step, which can be long after: until then it would keep
    /// turning its loader beside a sub-agent the sidebar already shows as done.
    pub(crate) fn settle_task_cards(&mut self) {
        let families = &self.families;
        for session in self.sessions.values_mut() {
            let streaming = session.streaming;
            let mut changed = Vec::new();
            for message in session.messages.iter_mut() {
                if settle_task_card(families, message, streaming) { changed.push(message.id.clone()); }
            }
            for id in changed {
                *session.message_versions.entry(id).or_default() += 1;
                session.structure += 1;
            }
        }
    }

    fn update_background_jobs(&mut self, root: String, count: usize, epoch: Option<&str>, revision: Option<u64>) {
        if let (Some(epoch), Some(revision)) = (epoch, revision) {
            if self.background_job_versions.get(&root).is_some_and(|(old, rev)| old == epoch && *rev > revision) { return; }
            self.background_job_versions.insert(root.clone(), (epoch.to_owned(), revision));
        }
        self.background_jobs.insert(root, count);
    }

    /// Whether any conversation the sidebar lists is at work (its loader is turning).
    pub fn sidebar_running(&self) -> bool {
        self.threads.iter().any(|t| self.activity_of(t) != "idle") || self.families.sidebar_count() > 0
    }

    /// What a conversation is doing, by the latest catalog event or list snapshot, else the
    /// one its row came with, else the older `busy` flag.
    pub fn activity_of<'a>(&'a self, thread: &'a ThreadSummary) -> &'a str {
        let activity = self.thread_activity.get(&thread.session_id).map(String::as_str)
            .or(thread.catalog_activity.as_deref())
            .unwrap_or(if thread.busy { "running" } else { "idle" });
        if activity != "permission" && self.background_count(&thread.session_id) > 0 { "background" } else { activity }
    }

    /// Per frame: remember where each showing transcript is scrolled and put one that just came
    /// back (a sub-agent or another tab was showing) where it was left. A transcript left at
    /// its end comes back at its end. Returns whether a position is still being put back.
    pub(crate) fn conversations_keep_scroll(&mut self, tx: &mut rune::Tx) -> bool {
        const FRAMES: u8 = 3;
        let showing: Vec<String> = self.layout.panes().iter().filter_map(|pane| pane.active_tab())
            .filter_map(|id| self.tabs.iter().find(|t| t.id == id))
            .filter_map(|tab| match &tab.kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None })
            .collect();
        let mut restoring = false;
        for (id, session) in self.sessions.iter_mut() {
            let shown = session.loaded && showing.contains(id);
            if shown && !session.shown { session.restoring = FRAMES; }
            session.shown = shown;
            if !shown { continue; }
            if session.restoring > 0 {
                session.restoring -= 1;
                session.wheel_target = None;
                // Left at its end, it comes back at its end, wherever that is now.
                match session.kept_scroll {
                    Some(offset) => tx.scroll_to_instant(format!("conversation:{id}"), 0.0, offset),
                    None => tx.scroll_to_bottom_instant(format!("conversation:{id}")),
                }
                restoring = true;
            } else {
                let (offset, max) = session.scroll.get();
                session.kept_scroll = (max - offset > 2.0).then_some(offset);
            }
        }
        restoring
    }

    /// The window is leaving this workspace: none of its conversations are on screen any more.
    pub(crate) fn conversations_hidden(&mut self) {
        for session in self.sessions.values_mut() { session.shown = false; }
    }

    /// Per frame: sub-agents that finished a moment ago leave the sidebar on time. Returns when
    /// to look again and whether the list changed.
    pub(crate) fn family_rows_tick(&mut self) -> (Option<std::time::Duration>, bool) {
        let count = self.families.sidebar_count();
        let changed = std::mem::replace(&mut self.family_rows, count) != count;
        (self.families.next_expiry(), changed)
    }

    /// Show `session_id` in the active agent tab: a sub-agent opened from a task card or the
    /// family header replaces the view in place, as GPUiX's `openThread(thread, false, root)`.
    pub fn view_session(&mut self, session_id: &str) {
        let index = self.active;
        if !matches!(self.tabs[index].kind, TabKind::Agent { .. }) { return; }
        self.tabs[index].kind = TabKind::Agent { session: Some(session_id.to_owned()) };
        self.restore_composer_selection();
        // A sub-agent still at work shows its trace from the start when opened
        // (`retain_inflight_child_trace`): its tools and thinking are the content.
        let working = self.families.working_now(session_id);
        let session = self.sessions.entry(session_id.to_owned()).or_default();
        session.wheel_target = None;
        if working && session.live_from.is_none() {
            session.live_from = Some(0);
            session.live_anchor = session.messages.first().map(|m| m.id.clone());
            session.structure += 1;
        }
        self.load_session(session_id);
    }

    /// A turn starts or ends. The live turn keeps its trace rows (`live_from`); when it ends the
    /// head page is fetched again for the server-composed footer and settled rows.
    fn set_streaming(&mut self, session_id: &str, streaming: bool) {
        // A sub-agent starting before any of its rows arrived is still a turn seen live.
        if streaming && self.families.is_child(session_id) { self.sessions.entry(session_id.to_owned()).or_default(); }
        let Some(session) = self.sessions.get_mut(session_id) else { return };
        if session.streaming == streaming { return; }
        session.streaming = streaming;
        if streaming {
            session.turn_started.get_or_insert_with(std::time::Instant::now);
            if session.live_from.is_none() {
                let from = session.messages.iter().rposition(|m| m.kind == HistoryMessageKind::User).unwrap_or(session.messages.len());
                session.live_from = Some(from);
                session.live_anchor = session.messages.get(from).map(|m| m.id.clone());
            }
        } else {
            session.status = None;
            session.turn_started = None;
            self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.into(), cursor: None, limit: Some(HEAD_PAGE) });
            self.settle_task_cards();
        }
    }

    /// The first prompt of a new conversation waits for `ThreadCreated`, then submits.
    pub fn create_thread(&mut self, index: usize, text: String) {
        self.tabs[index].pending_prompt = Some(text);
        self.send_agent(AgentClientMessage::CreateThread {
            title: None,
            directory: Some(self.cwd.clone()),
            agent: Some(self.composer.selection.agent.clone()).filter(|a| !a.is_empty()),
            model: Some(self.composer.selection.model.clone()).filter(|m| !m.is_empty()),
            connection_id: None,
            external_provider: None,
            thinking: Some(self.composer.selection.thinking.clone()).filter(|t| !t.is_empty()),
        });
    }

    pub(crate) fn submit_prompt(&mut self, session_id: &str, text: String) {
        let (attachments, echo) = self.take_attachments(session_id);
        let message_id = format!("msg_rune_{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
        let session = self.sessions.entry(session_id.into()).or_default();
        let echo = HistoryMessage {
            id: message_id.clone(),
            role: Role::User,
            kind: HistoryMessageKind::User,
            author: Some(self.local_name.clone()),
            title: String::new(),
            text: text.clone(),
            status: String::new(),
            tool: String::new(),
            agent_kind: None,
            tool_batch_id: None,
            mcp: None,
            composition_children: Vec::new(),
            output_kind: None,
            lang: String::new(),
            line_offset: None,
            detail: String::new(),
            todos: Vec::new(),
            usage: None,
            attachments: echo,
            created_at: 0,
        };
        // Idle sends keep their immediate bubble. Busy steers wait for consumption.
        if !session.streaming && session.queued_prompts == 0 { session.messages.push(echo.clone()); }
        session.pending_prompts.insert(message_id.clone(), echo);
        self.send_agent(AgentClientMessage::SubmitPrompt {
            session_id: session_id.into(),
            message_id,
            text,
            author: Some(self.local_name.clone()),
            attachments,
            mode: Some(self.composer.selection.agent.clone()).filter(|a| !a.is_empty()),
            model: Some(self.composer.selection.model.clone()).filter(|m| !m.is_empty()),
            connection_id: None,
            thinking: Some(self.composer.selection.thinking.clone()).filter(|t| !t.is_empty()),
            delivery: PromptDelivery::Steer,
        });
    }

    pub fn load_fixture(&mut self, session_id: &str, title: &str, json: &str) {
        let messages: Vec<HistoryMessage> = serde_json::from_str(json).unwrap_or_default();
        for message in &messages { self.mcp_icons.prepare_message(message); }
        let session = self.sessions.entry(session_id.into()).or_default();
        session.messages = messages;
        session.loaded = true;
        self.threads.push(ThreadSummary {
            session_id: session_id.into(), title: title.into(), directory: Some(self.cwd.clone()), model: None, agent: None,
            external_provider: None, updated_at: 0, message_count: 0, catalog_activity: None, busy: false, pinned: false,
            ..serde_json::from_value(json!({"session_id": session_id, "title": title})).unwrap()
        });
        self.open_tab(TabKind::Agent { session: Some(session_id.into()) });
    }

    pub fn stop(&self, session_id: &str) {
        self.send_agent(AgentClientMessage::CancelInflight { session_id: session_id.into() });
    }

    /// Conservatively request views, except for events explicitly ignored by this dispatch.
    /// Content deltas are paced separately by the inbox drain.
    pub(crate) fn apply_agent(&mut self, message: AgentServerMessage) -> bool {
        if self.apply_prompt_event(&message) { return true; }
        match &message {
            AgentServerMessage::HistoryChunk { messages, .. } => for m in messages { self.mcp_icons.prepare_message(m); },
            AgentServerMessage::MessageUpdated { message, .. } => self.mcp_icons.prepare_message(message),
            _ => {}
        }
        // Live queue events retain their full admission envelope; reconnect hydration uses
        // the typed snapshot. Both describe waiting work, not messages already consumed.
        let queue = match &message {
            AgentServerMessage::QueueUpdate { session_id, count, started_at, .. } => {
                let session = self.sessions.entry(session_id.clone()).or_default();
                match started_at.and_then(activity::instant_from_epoch_millis) {
                    Some(started) => session.turn_started = Some(started),
                    None if !session.streaming => session.turn_started = None,
                    None => {}
                }
                Some((session_id, *count))
            }
            AgentServerMessage::SessionEvent { session_id, kind, properties } if kind == "session.queue.updated" =>
                properties["queue"]["count"].as_u64().and_then(|n| u32::try_from(n).ok()).map(|count| (session_id, count)),
            _ => None,
        };
        if let AgentServerMessage::SessionEvent { session_id, kind, properties } = &message {
            if kind == "session.queue.updated" {
                let session = self.sessions.entry(session_id.clone()).or_default();
                match properties["action"].as_str() {
                    Some("dequeue") => {
                        let id = properties["messageID"].as_str()
                            .or_else(|| properties["messageId"].as_str())
                            .or_else(|| properties["request"]["messageID"].as_str())
                            .or_else(|| properties["request"]["messageId"].as_str());
                        if let Some(prompt) = id.and_then(|id| session.pending_prompts.remove(id)) {
                            session.note_message(&prompt);
                            upsert(&mut session.messages, prompt);
                            session.rebase_live();
                        }
                    }
                    Some("clear") => { session.hide_pending_prompts(); session.pending_prompts.clear(); }
                    _ => {},
                }
            }
        }
        if let Some((session_id, count)) = queue {
            let session = self.sessions.entry(session_id.clone()).or_default();
            // The server can enqueue a send before this client has observed its busy status.
            if count > 0 { session.hide_pending_prompts(); }
            if session.queued_prompts != count {
                session.queued_prompts = count;
                session.structure += 1; // Remeasure the virtual activity row when its child appears/disappears.
            }
            return true;
        }
        // The runtime snapshot also counts the family's background tasks (when it reports them).
        if let AgentServerMessage::RuntimeSnapshot { session_id, snapshot } = &message
            && let Some(jobs) = &snapshot.running_background_tasks
        {
            let root = if snapshot.root_session_id.is_empty() { self.families.root_of(session_id).to_owned() } else { snapshot.root_session_id.clone() };
            if !root.is_empty() { self.update_background_jobs(root, jobs.len(), snapshot.background_jobs_epoch.as_deref(), snapshot.background_jobs_revision); }
        }
        if let AgentServerMessage::BackgroundTasksUpdated { session_id, epoch, revision, tasks } = &message {
            self.update_background_jobs(self.families.root_of(session_id).to_owned(), tasks.len(), Some(epoch), Some(*revision));
        }
        // The daemon found its stream from the agent again: what was said in between is in
        // the history only.
        if let AgentServerMessage::SessionEvent { session_id, kind, .. } = &message && kind == "stream.resumed" {
            let session_id = session_id.clone();
            self.agent_stream_resumed(&session_id);
            return true;
        }
        if self.families.note(&message) {
            // A family gained or changed a member: its chats are fetched ahead of a click, and
            // a sub-agent that stopped settles the card that launched it.
            self.settle_task_cards();
            self.fold_streams();
            self.preload_family_history();
            return true;
        }
        // Only child lifecycle events stop a sub-agent. A task call can complete merely
        // because a launch or follow-up was queued while the child continues running.
        let stopped = match &message {
            AgentServerMessage::SessionIdle { session_id } if self.families.is_child(session_id) => Some((session_id.clone(), false)),
            AgentServerMessage::StreamingState { session_id, state: StreamingState::Idle, .. } if self.families.is_child(session_id) => Some((session_id.clone(), false)),
            _ => None,
        };
        if let Some((child, failed)) = stopped && self.families.finish(&child, failed) { self.settle_task_cards(); }
        if self.apply_slash_event(&message) { return true; }
        match message {
            AgentServerMessage::ThreadList { threads, requested_cursor, .. } => {
                // A refreshed catalog is newer evidence than a cached activity event. In
                // particular, recover from a missed idle event instead of keeping its loader
                // alive forever. Legacy rows without catalog activity do not replace it.
                for thread in &threads {
                    if let Some(activity) = &thread.catalog_activity {
                        self.set_streaming(&thread.session_id, activity == "running");
                        self.thread_activity.insert(thread.session_id.clone(), activity.clone());
                    }
                }
                if requested_cursor.is_none() { self.threads = threads; } else { self.threads.extend(threads); }
            }
            AgentServerMessage::ThreadCreated { session_id, .. } => {
                if let Some(index) = self.tabs.iter().position(|t| t.kind == TabKind::Agent { session: None } && t.pending_prompt.is_some()) {
                    self.tabs[index].kind = TabKind::Agent { session: Some(session_id.clone()) };
                    let prompt = self.tabs[index].pending_prompt.take().unwrap_or_default();
                    let session = self.sessions.entry(session_id.clone()).or_default();
                    session.selection = self.tabs[index].selection.clone().or_else(|| Some(self.composer.selection.clone()));
                    session.loaded = true;
                    // Follow the new conversation's stream before the first prompt goes out, so
                    // its streaming state, deltas and tool rows arrive live.
                    self.send_agent(AgentClientMessage::SwitchThread { session_id: session_id.clone() });
                    self.stream_owner.insert(session_id.clone(), session_id.clone());
                    self.send_agent(AgentClientMessage::ResumeStream { session_id: session_id.clone() });
                    self.submit_prompt(&session_id, prompt);
                }
                self.refresh_threads();
            }
            AgentServerMessage::ThreadUpdated { session_id, title, pinned } => {
                if let Some(thread) = self.threads.iter_mut().find(|t| t.session_id == session_id) {
                    if let Some(title) = title { thread.title = title; }
                    if let Some(pinned) = pinned { thread.pinned = pinned; }
                }
            }
            AgentServerMessage::HistoryChunk { session_id, messages, next_cursor } => {
                // Task cards name their child sessions; seed the family so it can be walked.
                if !self.families.is_child(&session_id) && self.families.seed_from_history(&session_id, &messages) { self.fold_streams(); }
                self.prefetching.remove(&session_id);
                let mut messages = messages;
                identify_anonymous_rows(&mut messages);
                let id = session_id.clone();
                let session = self.sessions.entry(session_id).or_default();
                let older = session.loading_older;
                for message in &mut messages { settle_task_card(&self.families, message, session.streaming); }
                for message in &messages {
                    session.pending_prompts.remove(&message.id);
                    session.note_message(message);
                }
                if session.loading_older {
                    // Size the next page from this one (bytes per server message).
                    let bytes: usize = messages.iter().map(message_bytes).sum();
                    if bytes > 0 {
                        let next = (session.older_limit as usize * OLDER_PAGE_TARGET_BYTES / bytes).clamp(8, 64);
                        session.older_limit = next as u32;
                    }
                    session.loading_older = false;
                    let known: std::collections::HashSet<String> = session.messages.iter().map(|m| m.id.clone()).collect();
                    let mut older: Vec<HistoryMessage> = messages.into_iter().filter(|m| !known.contains(&m.id)).collect();
                    older.append(&mut session.messages);
                    session.messages = older;
                    session.older_cursor = next_cursor;
                    // The page lands above the reader and the view stays on its row: the
                    // scroller holds a row, not an offset (`anchor.rs`), so whatever glide
                    // or coast is running carries straight on.
                    // Row offsets measured before the page are all off by its height.
                    session.question_tops.clear();
                    // And every row's index by its length: the live turn is found again.
                    session.rebase_live();
                } else if std::mem::take(&mut session.resync) && session.loaded && !messages.is_empty() && !overlaps(&session.messages, &messages) {
                    // The window was away for more than a page: laid over what is loaded,
                    // this one would leave a hole between the two. The transcript starts
                    // again at the conversation's end, and everything before it (what was
                    // loaded included) pages in above as it is scrolled to.
                    session.restart_at(messages, next_cursor);
                } else {
                    // The page's order is the conversation's: rows that arrived in another
                    // order (a reply's empty text part ahead of its thinking) take it.
                    if merge_head(&mut session.messages, messages) { session.structure += 1; }
                    session.rebase_live();
                    if !session.loaded { session.older_cursor = next_cursor; self.settle_pending = true; }
                    session.loaded = true;
                }
                // A refresh that waited for this older page (the two answers look alike).
                if older && session.resync { self.refresh_transcript(&id); }
                // The next sub-agent chat waiting to be fetched ahead.
                self.preload_family_history();
            }
            AgentServerMessage::MessageUpdated { session_id, mut message, parent_message_id } => {
                let session = self.sessions.entry(session_id).or_default();
                let part = message.id.clone();
                session.pending_prompts.remove(&message.id);
                settle_task_card(&self.families, &mut message, session.streaming);
                session.note_message(&message);
                upsert(&mut session.messages, message);
                session.order_stream_parts(&part, parent_message_id);
                session.rebase_live();
            }
            AgentServerMessage::ContentDelta { session_id, message_id, text, kind, parent_message_id } => {
                let session = self.sessions.entry(session_id).or_default();
                let part = message_id.clone();
                *session.message_versions.entry(message_id.clone()).or_default() += 1;
                if let Some(message) = session.messages.iter_mut().find(|m| m.id == message_id) {
                    // A reply or a thought growing keeps the row list as it is; anything else
                    // (tool output, or text short enough to still decide its row) may not.
                    let prose = matches!(message.kind, HistoryMessageKind::Assistant | HistoryMessageKind::Reasoning) && message.text.len() >= 32;
                    if !prose { session.structure += 1; }
                    message.text.push_str(&text);
                } else {
                    session.structure += 1;
                    let kind = match serde_json::to_value(&kind).ok().as_ref().and_then(Value::as_str) {
                        Some("Thinking") | Some("Reasoning") => HistoryMessageKind::Reasoning,
                        _ => HistoryMessageKind::Assistant,
                    };
                    insert_new(&mut session.messages, HistoryMessage {
                        id: message_id, role: Role::Assistant, kind, author: None, title: String::new(), text,
                        status: String::new(), tool: String::new(), agent_kind: None, tool_batch_id: None, mcp: None,
                        composition_children: Vec::new(), output_kind: None, lang: String::new(), line_offset: None,
                        detail: String::new(), todos: Vec::new(), usage: None, attachments: Vec::new(), created_at: 0,
                    });
                }
                session.order_stream_parts(&part, parent_message_id);
            }
            AgentServerMessage::Compaction { session_id, phase, .. } => {
                let ended = matches!(phase, neoism_protocol::agent::CompactionPhase::Ended);
                self.sessions.entry(session_id.clone()).or_default().compacting = !ended;
                if ended {
                    self.send_agent(AgentClientMessage::GetHistory { session_id, cursor: None, limit: Some(HEAD_PAGE) });
                }
            }
            AgentServerMessage::StreamingState { session_id, state, label } => {
                // Tool/reasoning parts can emit a derived streaming verb after the turn
                // ended. They describe its timeline, not whether the catalog is at work.
                let streaming = !matches!(state, StreamingState::Idle)
                    && self.thread_activity.get(&session_id).is_none_or(|activity| activity == "running");
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    session.compacting = streaming && matches!(state, StreamingState::Compacting);
                }
                self.set_streaming(&session_id, streaming);
                if let Some(session) = self.sessions.get_mut(&session_id) { session.status = label; }
            }
            // The daemon reports a conversation's turn as catalog activity (`running` / `idle`).
            AgentServerMessage::CatalogActivity { session_id, activity } => {
                if self.sessions.contains_key(&session_id) { self.set_streaming(&session_id, activity == "running"); }
                // Every conversation's, loaded or not: the sidebar shows which are at work.
                self.thread_activity.insert(session_id, activity);
            }
            // A part was withdrawn (a retried step): its row goes.
            AgentServerMessage::PartRemoved { session_id, part_id } => {
                if let Some(session) = self.sessions.get_mut(&session_id) {
                    let before = session.messages.len();
                    session.messages.retain(|m| m.id != part_id);
                    if session.messages.len() != before { session.structure += 1; session.rebase_live(); }
                }
            }
            AgentServerMessage::SessionIdle { session_id, .. } => {
                if let Some(session) = self.sessions.get_mut(&session_id) { session.compacting = false; }
                self.set_streaming(&session_id, false);
                // The session stream and whole-family catalog are independent. Recover a
                // missed catalog idle event without hiding genuinely active background work.
                self.refresh_threads();
            },
            // The agent service (re)started its catalog: on a cold start it comes up after the
            // daemon, and whatever was asked before it did went unanswered. Ask again.
            AgentServerMessage::CatalogReset { .. } => {
                // What the old catalog said was running is only still running if the new one
                // says so again.
                self.thread_activity.clear();
                for thread in &mut self.threads {
                    thread.catalog_activity = None;
                    thread.busy = false;
                }
                self.background_jobs.clear();
                self.background_job_versions.clear();
                self.families.catalog_reset();
                self.settle_task_cards();
                if self.catalog_asked.is_none_or(|at| at.elapsed() > std::time::Duration::from_secs(2)) {
                    self.catalog_asked = Some(std::time::Instant::now());
                    self.agent_connected();
                }
            }
            AgentServerMessage::ProviderState { session_id, context_limit, .. } => {
                if let Some(limit) = context_limit.filter(|l| *l > 0) { self.sessions.entry(session_id).or_default().context_limit = Some(limit); }
            }
            // These carry no state consumed by Rune, including the family and slash
            // handlers above. Keep unclassified events conservative, not ignored.
            AgentServerMessage::SessionEvent { .. }
            | AgentServerMessage::MessageStart { .. }
            | AgentServerMessage::MessageEnd { .. }
            | AgentServerMessage::ThreadSwitched { .. } => return false,
            _ => {}
        }
        true
    }
}

fn message_bytes(message: &HistoryMessage) -> usize {
    message.text.len() + message.detail.len() + message.title.len()
        + message.composition_children.iter().map(message_bytes).sum::<usize>()
}

/// A reply's text part before any of it has arrived. The daemon creates it when the turn
/// starts, ahead of the thinking and tool rows it then lists before it.
fn placeholder(message: &HistoryMessage) -> bool {
    matches!(message.role, Role::Assistant) && matches!(message.kind, HistoryMessageKind::Assistant) && message.text.is_empty() && message.tool.is_empty()
}

/// A row seen for the first time goes last, but ahead of a reply that has not started: the
/// thinking that arrives after the empty reply part is shown before it, as the history has it.
fn insert_new(messages: &mut Vec<HistoryMessage>, message: HistoryMessage) {
    let mut at = messages.len();
    if !placeholder(&message) && message.kind != HistoryMessageKind::User {
        while at > 0 && placeholder(&messages[at - 1]) { at -= 1; }
    }
    messages.insert(at, message);
}

fn upsert(messages: &mut Vec<HistoryMessage>, message: HistoryMessage) {
    if let Some(existing) = messages.iter_mut().find(|m| m.id == message.id) {
        *existing = message;
    } else {
        insert_new(messages, message);
    }
}

/// Some server-generated system rows have no part ID (notably compaction notices).
/// Give them an identity relative to the preceding real row, rather than conflating
/// every anonymous notice or making them compete for the same held slot.
fn identify_anonymous_rows(rows: &mut [HistoryMessage]) {
    use std::hash::{Hash, Hasher};
    let mut anchor = String::new();
    let mut occurrence = 0;
    for row in rows {
        if !row.id.is_empty() {
            if !row.id.starts_with("history-anonymous:") { anchor = row.id.clone(); occurrence = 0; }
            continue;
        }
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        row.text.hash(&mut hash);
        row.title.hash(&mut hash);
        row.id = format!("history-anonymous:{anchor}:{}:{occurrence}", hash.finish());
        occurrence += 1;
    }
}

/// Whether any row of `page` is one the transcript already holds.
fn overlaps(messages: &[HistoryMessage], page: &[HistoryMessage]) -> bool {
    let held: std::collections::HashSet<&str> = messages.iter().map(|m| m.id.as_str()).collect();
    page.iter().any(|m| held.contains(m.id.as_str()))
}

/// A history page is read from the store a moment before or after the stream's events for
/// the same reply: its copy of a reply still being written can be shorter than what has
/// streamed. The text never goes back to it (the words after would be appended to less than
/// what came before them).
fn keep_streamed_text(old: &HistoryMessage, new: &mut HistoryMessage) {
    if matches!(new.kind, HistoryMessageKind::Assistant | HistoryMessageKind::Reasoning) && new.tool.is_empty()
        && old.text.len() > new.text.len() && old.text.starts_with(new.text.as_str()) {
        new.text = old.text.clone();
    }
}

/// Head refresh (the GPUiX `mergeHistoryHead` contract): rows the page knows take its copies
/// and its order, in the places those rows held; rows it doesn't know stay where they were;
/// rows new with the page land after the row the page puts before them. Returns whether any
/// row moved or arrived.
fn merge_head(messages: &mut Vec<HistoryMessage>, mut page: Vec<HistoryMessage>) -> bool {
    identify_anonymous_rows(messages);
    identify_anonymous_rows(&mut page);
    // Compaction refreshes can repeat a part ID. Keep its first position and latest
    // snapshot, as a normal MessageUpdated would, before assigning held-row slots.
    let mut unique = Vec::<HistoryMessage>::with_capacity(page.len());
    let mut positions = std::collections::HashMap::<String, usize>::new();
    for message in page {
        if let Some(&at) = positions.get(&message.id) { unique[at] = message; }
        else { positions.insert(message.id.clone(), unique.len()); unique.push(message); }
    }
    let page = unique;
    let mut seen = std::collections::HashSet::new();
    let before = messages.len();
    messages.retain(|m| seen.insert(m.id.clone()));
    let deduplicated = messages.len() != before;
    if messages.is_empty() {
        *messages = page;
        return false;
    }
    let paged: std::collections::HashSet<&str> = page.iter().map(|m| m.id.as_str()).collect();
    let slots: Vec<usize> = messages.iter().enumerate().filter(|(_, m)| paged.contains(m.id.as_str())).map(|(i, _)| i).collect();
    let held: std::collections::HashSet<String> = slots.iter().map(|i| messages[*i].id.clone()).collect();
    let mut moved = deduplicated;
    let mut slot = slots.iter();
    let mut arrivals: Vec<(Option<String>, HistoryMessage)> = Vec::new();
    let mut previous: Option<String> = None;
    let streamed: std::collections::HashMap<String, HistoryMessage> = messages.iter().filter(|m| held.contains(&m.id) && !m.text.is_empty()
        && matches!(m.kind, HistoryMessageKind::Assistant | HistoryMessageKind::Reasoning)).map(|m| (m.id.clone(), m.clone())).collect();
    for mut message in page {
        let id = message.id.clone();
        if held.contains(&id) {
            let at = *slot.next().expect("one slot per held row");
            moved |= messages[at].id != id;
            if let Some(old) = streamed.get(&id) { keep_streamed_text(old, &mut message); }
            messages[at] = message;
        } else {
            arrivals.push((previous.clone(), message));
        }
        previous = Some(id);
    }
    for (after, message) in arrivals {
        moved = true;
        let at = match after.and_then(|id| messages.iter().position(|m| m.id == id)) {
            Some(index) => index + 1,
            // The page starts with it: ahead of the page's first held row, else last.
            None => slots.first().copied().unwrap_or(messages.len()),
        };
        messages.insert(at.min(messages.len()), message);
    }
    moved
}

pub(crate) fn title_case(model: &str) -> String {
    model.split('/').map(|part| part.split('-').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join("-")).collect::<Vec<_>>().join("/")
}

#[cfg(test)]
mod prompt_queue_tests {
    use super::*;

    #[test]
    fn idle_sends_echo_immediately_but_server_queued_sends_wait_for_consumption() {
        let mut app = App::new();
        app.load_fixture("queue", "Admission race", "[]");
        app.submit_prompt("queue", "An ordinary idle send".into());
        assert_eq!(app.sessions["queue"].messages.len(), 1);
        let confirmed = app.sessions["queue"].messages[0].clone();
        app.apply_agent(AgentServerMessage::MessageUpdated {
            session_id: "queue".into(), message: confirmed.clone(), parent_message_id: None,
        });
        assert_eq!(app.sessions["queue"].messages.len(), 1);

        app.submit_prompt("queue", "The server is busy before this client knows".into());
        assert_eq!(app.sessions["queue"].messages.len(), 2);
        let raced = app.sessions["queue"].messages[1].clone();
        app.apply_agent(AgentServerMessage::SessionEvent {
            session_id: "queue".into(), kind: "session.queue.updated".into(),
            properties: json!({"action":"enqueue", "request":{"messageId":raced.id}, "queue":{"count":1}}),
        });
        assert_eq!(app.sessions["queue"].messages.len(), 1, "an authoritative queue snapshot must retract the unconsumed echo");
        let mut reply = confirmed.clone();
        reply.id = "current-reply".into();
        reply.role = Role::Assistant;
        reply.kind = HistoryMessageKind::Assistant;
        reply.text = "Finish the active turn first".into();
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "queue".into(), message: reply, parent_message_id: None });
        app.apply_agent(AgentServerMessage::SessionEvent {
            session_id: "queue".into(), kind: "session.queue.updated".into(),
            properties: json!({"action":"dequeue", "messageId":raced.id, "queue":{"count":0}}),
        });
        let ids: Vec<_> = app.sessions["queue"].messages.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, [confirmed.id.as_str(), "current-reply", raced.id.as_str()]);
    }

    #[test]
    fn queued_prompts_enter_the_transcript_only_when_consumed_and_in_consumption_order() {
        let mut app = App::new();
        app.load_fixture("queue", "Queued turns", r#"[{"id":"old-reply","role":"Assistant","kind":"Assistant","text":""}]"#);
        app.sessions.get_mut("queue").unwrap().streaming = true;
        app.submit_prompt("queue", "First steer".into());
        app.submit_prompt("queue", "Second steer".into());
        let first = app.sessions["queue"].pending_prompts.values().find(|p| p.text == "First steer").unwrap().clone();
        let second = app.sessions["queue"].pending_prompts.values().find(|p| p.text == "Second steer").unwrap().clone();
        let event = |action: &str, id: &str, count| AgentServerMessage::SessionEvent {
            session_id: "queue".into(), kind: "session.queue.updated".into(),
            properties: json!({"action": action, "request": {"messageId": id}, "queue": {"count": count}}),
        };
        app.apply_agent(event("enqueue", &first.id, 2));
        app.apply_agent(AgentServerMessage::SessionEvent {
            session_id: "queue".into(), kind: "session.prompt.admitted".into(),
            properties: json!({"request": {"messageId": first.id}}),
        });
        assert_eq!(app.sessions["queue"].messages.len(), 1, "admission and queue snapshots must not create a bubble");

        // Consumption, not submission order, decides where the bubble belongs. Keep the
        // old reply's placeholder ahead of it so a late final chunk cannot cross turns.
        app.apply_agent(event("dequeue", &second.id, 1));
        let ids = |app: &App| app.sessions["queue"].messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        assert_eq!(ids(&app), vec!["old-reply".to_owned(), second.id.clone()]);
        let update = |message| AgentServerMessage::MessageUpdated { session_id: "queue".into(), message, parent_message_id: None };
        app.apply_agent(update(second.clone()));
        assert_eq!(app.sessions["queue"].messages.len(), 2, "the server echo must merge with the dequeued row");
        let mut old_reply = app.sessions["queue"].messages[0].clone();
        old_reply.text = "The old turn's final output".into();
        app.apply_agent(update(old_reply));
        let mut next_reply = app.sessions["queue"].messages[0].clone();
        next_reply.id = "next-reply".into();
        next_reply.text = "Answer to the consumed steer".into();
        app.apply_agent(update(next_reply));
        // A real user part may beat the dequeue event. Neither event may duplicate it.
        app.apply_agent(update(first.clone()));
        app.apply_agent(event("dequeue", &first.id, 0));
        assert_eq!(ids(&app), vec!["old-reply".to_owned(), second.id, "next-reply".to_owned(), first.id]);
        assert!(app.sessions["queue"].pending_prompts.is_empty());

        app.submit_prompt("queue", "Cleared steer".into());
        let cleared = app.sessions["queue"].pending_prompts.keys().next().unwrap().clone();
        app.apply_agent(event("clear", "", 0));
        app.apply_agent(event("dequeue", &cleared, 0));
        assert_eq!(app.sessions["queue"].messages.len(), 4, "a cleared steer must not become a transcript message");
    }
}

#[cfg(test)]
mod cache_tests {
    use super::*;
    #[test]
    fn compaction_notices_without_ids_keep_their_separate_positions() {
        let row = |id: &str, text: &str| serde_json::from_value::<HistoryMessage>(json!({"id":id,"role":"System","kind":"System","text":text})).unwrap();
        let page = vec![row("reply", "reply"), row("", "Compacting"), row("compact1", "summary"), row("", "Compacting"), row("compact2", "summary")];
        let mut rows = Vec::new();
        merge_head(&mut rows, page.clone());
        merge_head(&mut rows, page);
        assert_eq!(rows.len(), 5);
        assert_ne!(rows[1].id, rows[3].id);
        assert_eq!(rows[2].id, "compact1");
        assert_eq!(rows[4].id, "compact2");
    }

    #[test]
    fn repeated_compaction_rows_merge_once_without_losing_unpaged_history() {
        let row = |id: &str, text: &str| serde_json::from_value::<HistoryMessage>(json!({"id":id,"role":"Assistant","kind":"Assistant","text":text})).unwrap();
        let mut rows = vec![row("older", "old"), row("summary", "before"), row("tail", "tail")];
        merge_head(&mut rows, vec![row("summary", "partial"), row("summary", "finished"), row("new", "first"), row("new", "last")]);
        assert_eq!(rows.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["older", "summary", "new", "tail"]);
        assert_eq!(rows[1].text, "finished");
        assert_eq!(rows[2].text, "last");
    }

    #[test]
    fn compaction_activity_ends_without_stopping_a_continuing_turn() {
        use neoism_protocol::agent::CompactionPhase;
        let mut app = App::new();
        app.sessions.entry("s".into()).or_default().streaming = true;
        for phase in [CompactionPhase::Started, CompactionPhase::Delta] {
            app.apply_agent(AgentServerMessage::Compaction { session_id: "s".into(), phase, run_continues: None, text: None, reason: None });
            assert_eq!(app.activity_state("s"), Some(activity::State::Compacting));
        }
        app.apply_agent(AgentServerMessage::Compaction { session_id: "s".into(), phase: CompactionPhase::Ended, run_continues: Some(true), text: None, reason: None });
        assert_ne!(app.activity_state("s"), Some(activity::State::Compacting));
        assert!(app.sessions["s"].streaming);
    }

    #[test]
    fn late_thinking_precedes_its_streaming_reply_without_crossing_steps() {
        let mut app = App::new();
        let delta = |app: &mut App, id: &str, parent: &str, kind, text: &str| app.apply_agent(AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: id.into(), parent_message_id: Some(parent.into()), kind, text: text.into(),
        });
        use neoism_protocol::agent::ContentKind;
        delta(&mut app, "earlier", "step1", ContentKind::Text, "Previous reply");
        delta(&mut app, "reply", "step2", ContentKind::Text, "Already streaming");
        delta(&mut app, "thinking", "step2", ContentKind::Reasoning, "Planning");
        let order = |app: &App| app.sessions["s"].messages.iter().map(|m| m.id.as_str()).collect::<Vec<_>>().join(",");
        assert_eq!(order(&app), "earlier,thinking,reply");
        let structure = app.sessions["s"].structure;
        delta(&mut app, "reply", "step2", ContentKind::Text, " more text");
        assert_eq!(order(&app), "earlier,thinking,reply");
        assert!(app.sessions["s"].structure >= structure);
    }

    #[test]
    fn unchanged_history_keeps_other_message_versions() {
        let message: HistoryMessage = serde_json::from_value(json!({"id":"a", "role":"Assistant", "kind":"Assistant", "text":"first"})).unwrap();
        let mut session = Session::default();
        session.note_message(&message);
        session.messages.push(message.clone());
        let version = session.message_versions["a"];
        session.note_message(&message);
        assert_eq!(session.message_versions["a"], version);
        let mut changed = message.clone();
        changed.status = "Build · model".into();
        session.note_message(&changed);
        assert_eq!(session.message_versions["a"], version + 1);
        let mut other = message; other.id = "b".into();
        session.note_message(&other);
        assert_eq!(session.message_versions["a"], version + 1);
    }

    fn tool(id: &str, status: &str, detail: &str) -> HistoryMessage {
        serde_json::from_value(json!({"id": id, "role": "Assistant", "kind": "Tool", "tool": "bash", "title": "Bash", "status": status, "detail": detail})).unwrap()
    }

    #[test]
    fn any_change_to_a_message_bumps_its_revision() {
        let mut app = App::new();
        let update = |app: &mut App, message: HistoryMessage| app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message, parent_message_id: None });
        update(&mut app, tool("t", "running", ""));
        let first = app.sessions["s"].message_versions["t"];
        // A tool row draws its detail too: a change there alone is a new revision.
        update(&mut app, tool("t", "running", "line one"));
        assert_eq!(app.sessions["s"].message_versions["t"], first + 1);
        update(&mut app, tool("t", "running", "line one"));
        assert_eq!(app.sessions["s"].message_versions["t"], first + 1);
    }

    #[test]
    fn a_growing_reply_keeps_the_row_list_and_a_new_row_recomputes_it() {
        let mut app = App::new();
        let delta = |app: &mut App, text: &str| app.apply_agent(AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: "a".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: text.into(),
        });
        delta(&mut app, "The scheduler presents on the display clock. ");
        let key = |app: &App| { let s = &app.sessions["s"]; (s.structure, s.messages.len(), usize::MAX, false) };
        let rows = app.sessions["s"].row_items(key(&app));
        assert_eq!(*rows, vec![timeline::RowItem::Message(0)]);
        // More text for the same reply: same key, the very same list.
        delta(&mut app, "It never busy-waits.");
        assert!(std::rc::Rc::ptr_eq(&rows, &app.sessions["s"].row_items(key(&app))));
        // A tool call arrives: a new key and a new list.
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: tool("t", "running", ""), parent_message_id: None });
        let next = app.sessions["s"].row_items((key(&app).0, 2, 0, false));
        assert_eq!(*next, vec![timeline::RowItem::Message(0), timeline::RowItem::Message(1)]);
        // Its status changing may fold or unfold rows: the key moves again.
        let structure = app.sessions["s"].structure;
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: tool("t", "completed", ""), parent_message_id: None });
        assert!(app.sessions["s"].structure > structure);
    }

    #[test]
    fn older_history_loading_above_never_puts_finished_turns_in_the_live_window() {
        let mut app = App::new();
        let message = |id: &str, kind: &str, role: &str| -> HistoryMessage { serde_json::from_value(json!({"id": id, "role": role, "kind": kind, "text": id})).unwrap() };
        let session = app.sessions.entry("s".into()).or_default();
        session.loaded = true;
        session.messages = vec![message("old-prompt", "User", "User"), message("old-tool", "Tool", "Assistant"), message("old-answer", "Assistant", "Assistant"), message("prompt", "User", "User")];
        // A turn starts: its trace is live from its prompt.
        app.apply_agent(AgentServerMessage::StreamingState { session_id: "s".into(), state: neoism_protocol::agent::StreamingState::Working, label: None });
        app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: message("tool", "Tool", "Assistant"), parent_message_id: None });
        let shown = |app: &App| { let s = &app.sessions["s"]; s.messages.iter().enumerate().filter(|(i, m)| tools::visible(m, *i, s.live_from.unwrap_or(usize::MAX))).map(|(_, m)| m.id.clone()).collect::<Vec<_>>() };
        assert_eq!(shown(&app), ["old-prompt", "old-answer", "prompt", "tool"]);
        // An older page lands above: every index moves, the live turn does not.
        let session = app.sessions.get_mut("s").unwrap();
        session.loading_older = true;
        app.apply_agent(AgentServerMessage::HistoryChunk { session_id: "s".into(), next_cursor: None, messages: vec![
            message("ancient-prompt", "User", "User"), message("ancient-tool", "Tool", "Assistant"), message("ancient-think", "Reasoning", "Assistant"), message("ancient-answer", "Assistant", "Assistant"),
        ] });
        assert_eq!(shown(&app), ["ancient-prompt", "ancient-answer", "old-prompt", "old-answer", "prompt", "tool"]);
        assert_eq!(app.sessions["s"].live_from, Some(7));
    }

    #[test]
    fn thinking_is_shown_before_the_reply_it_precedes_while_streaming_and_once_settled() {
        let mut app = App::new();
        let message = |id: &str, kind: &str, role: &str, text: &str| -> HistoryMessage { serde_json::from_value(json!({"id": id, "role": role, "kind": kind, "text": text})).unwrap() };
        let update = |app: &mut App, m: HistoryMessage| app.apply_agent(AgentServerMessage::MessageUpdated { session_id: "s".into(), message: m, parent_message_id: None });
        let order = |app: &App| app.sessions["s"].messages.iter().map(|m| m.id.clone()).collect::<Vec<_>>();
        // As the daemon sends a turn: the prompt, the reply's empty text part, then the thinking.
        update(&mut app, message("user", "User", "User", "oq"));
        update(&mut app, message("reply", "Assistant", "Assistant", ""));
        update(&mut app, message("think", "Reasoning", "Assistant", ""));
        assert_eq!(order(&app), ["user", "think", "reply"]);
        app.apply_agent(AgentServerMessage::ContentDelta { session_id: "s".into(), message_id: "reply".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: "What would you like me to do?".into() });
        // Once the reply has text, what follows it follows it.
        update(&mut app, message("usage", "System", "System", "8145 tokens"));
        assert_eq!(order(&app), ["user", "think", "reply", "usage"]);
        // Rows that did arrive out of order take the settled history's order.
        let session = app.sessions.get_mut("s").unwrap();
        session.loaded = true;
        session.messages.swap(1, 2);
        session.messages.push(message("local", "System", "System", "only here"));
        let structure = session.structure;
        app.apply_agent(AgentServerMessage::HistoryChunk { session_id: "s".into(), next_cursor: None, messages: vec![
            message("user", "User", "User", "oq"), message("think", "Reasoning", "Assistant", "**Seeking clarification**"), message("reply", "Assistant", "Assistant", "What would you like me to do?"),
            message("usage", "System", "System", "8145 tokens"), message("late", "System", "System", "new with the page"),
        ] });
        assert_eq!(order(&app), ["user", "think", "reply", "usage", "late", "local"]);
        assert!(app.sessions["s"].structure > structure, "the row list is rebuilt");
    }

    #[test]
    fn streamed_text_re_runs_the_views_at_most_thirty_times_a_second() {
        let mut pace = StreamPace::default();
        let start = std::time::Instant::now();
        assert_eq!(pace.due(start), (false, None), "nothing arrived");
        pace.pending = true;
        assert_eq!(pace.due(start), (true, None), "the first text shows at once");
        pace.ran(start);
        // More text right behind it waits out the interval, then shows.
        pace.pending = true;
        let soon = start + std::time::Duration::from_millis(10);
        assert_eq!(pace.due(soon), (false, Some(std::time::Duration::from_millis(23))));
        assert_eq!(pace.due(start + StreamPace::INTERVAL), (true, None));
        // Anything else that runs the views shows the text with it.
        pace.ran(soon);
        assert_eq!(pace.due(soon), (false, None));
    }

    #[test]
    fn ignored_agent_events_do_not_invalidate_views() {
        let mut app = App::new();
        let inbox: crate::daemon::Inbox = Default::default();
        app.inbox = Some(inbox.clone());
        let ignored = || [
            AgentServerMessage::SessionEvent { session_id: "s".into(), kind: "message.updated".into(), properties: json!({}) },
            AgentServerMessage::MessageStart { session_id: "s".into(), role: Role::Assistant, message_id: "a".into() },
            AgentServerMessage::MessageEnd { session_id: "s".into(), message_id: "a".into(), stop_reason: "end_turn".into() },
            AgentServerMessage::ThreadSwitched { session_id: "s".into() },
        ];
        let inbound = |message| crate::daemon::Inbound::Agent(Box::new(message));
        for message in ignored() {
            inbox.lock().unwrap().push_back(inbound(message));
            assert!(!app.drain_current(), "an ignored event must not re-run the views");
            assert!(!app.stream_pace.pending, "ignored events must not schedule a text commit either");
        }
        assert!(app.sessions.is_empty(), "ignored lifecycle events create no transcript state");
        assert!(inbox.lock().unwrap().is_empty());

        // Ignored events on either side must not mask a legitimate visible change.
        let mut queue = inbox.lock().unwrap();
        queue.extend(ignored().into_iter().map(inbound));
        queue.push_back(inbound(AgentServerMessage::CatalogActivity { session_id: "s".into(), activity: "running".into() }));
        queue.extend(ignored().into_iter().map(inbound));
        drop(queue);
        assert!(app.drain_current());
        assert_eq!(app.thread_activity["s"], "running");
        assert!(!app.drain_current(), "the inbox has been drained");
    }

    #[test]
    fn queue_resume_and_prompt_events_still_invalidate_views() {
        let mut app = App::new();
        let inbox: crate::daemon::Inbox = Default::default();
        app.inbox = Some(inbox.clone());
        for message in [
            AgentServerMessage::SessionEvent {
                session_id: "s".into(), kind: "session.queue.updated".into(),
                properties: json!({"queue":{"count":1}}),
            },
            AgentServerMessage::SessionEvent {
                session_id: "s".into(), kind: "stream.resumed".into(), properties: json!({}),
            },
            AgentServerMessage::QuestionAsked {
                session_id: "s".into(), request: json!({"id":"q","questions":[{"question":"Continue?"}]}),
            },
        ] {
            inbox.lock().unwrap().push_back(crate::daemon::Inbound::Agent(Box::new(message)));
            assert!(app.drain_current(), "handled state events must still update the UI");
            assert!(!app.stream_pace.pending);
        }
        assert_eq!(app.sessions["s"].queued_prompts, 1);
        assert!(!app.drain_current());
    }

    #[test]
    fn deltas_wait_for_the_stream_pace_and_other_events_do_not() {
        let mut app = App::new();
        let inbox: crate::daemon::Inbox = Default::default();
        app.inbox = Some(inbox.clone());
        let delta = || crate::daemon::Inbound::Agent(Box::new(AgentServerMessage::ContentDelta {
            session_id: "s".into(), message_id: "a".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: "word ".into(),
        }));
        inbox.lock().unwrap().push_back(delta());
        assert!(!app.drain_current(), "text alone does not re-run the views by itself");
        assert!(app.stream_pace.pending);
        assert_eq!(app.sessions["s"].messages[0].text, "word ", "but it is applied at once");
        inbox.lock().unwrap().extend([delta(), crate::daemon::Inbound::Agent(Box::new(AgentServerMessage::SessionIdle { session_id: "s".into() }))]);
        assert!(app.drain_current(), "a visible state event re-runs them now");
    }
}
