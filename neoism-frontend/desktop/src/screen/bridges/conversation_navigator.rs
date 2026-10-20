//! Native ownership of the shared conversation Search / Projects overlay.
//! Search never mutates the workspace-scoped sidebar catalog.
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use neoism_ui::widgets::conversation_navigator::{
    ConversationNavigatorAction as Action, ConversationNavigatorMode as Mode,
    ConversationNavigatorRow as Row,
};

use crate::neoism::agent::{
    fetch_session_entries_page, fetch_session_search_hits,
    side_panel::NeoismAgentSessionEntry, AgentEventWake,
};
use crate::screen::Screen;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct SearchIdentity {
    generation: u64,
    server: String,
    query: String,
    directory: Option<String>,
}

impl SearchIdentity {
    fn advance(&mut self, server: String, query: String, directory: Option<String>) {
        self.generation = self.generation.wrapping_add(1);
        self.server = server;
        self.query = query;
        self.directory = directory;
    }

    fn accepts(
        &self,
        reply: &Self,
        active_search: bool,
        current_server: &str,
        directory: Option<&str>,
    ) -> bool {
        active_search
            && self == reply
            && self.server == current_server
            && self.directory.as_deref() == directory
    }
}

struct SearchRequest {
    identity: SearchIdentity,
    current: Option<String>,
    transcript_enabled: bool,
    wake: AgentEventWake,
}

const SEARCH_DEBOUNCE: Duration = Duration::from_millis(160);

// One replaceable pending query, shared by exactly two workers. The synchronous
// HTTP API cannot abort an in-flight request: the spare worker bypasses one slow
// obsolete fetch, but if both are blocked the latest query must wait for a slot.
// Never detach additional per-query workers to work around that transport limit.
struct SearchQueue<T> {
    state: Mutex<SearchQueueState<T>>,
    changed: Condvar,
}

struct SearchQueueState<T> {
    generation: u64,
    pending: Option<(T, Instant)>,
    closed: bool,
}

impl<T> Default for SearchQueue<T> {
    fn default() -> Self {
        Self {
            state: Mutex::new(SearchQueueState {
                generation: 0,
                pending: None,
                closed: false,
            }),
            changed: Condvar::new(),
        }
    }
}

impl<T> SearchQueue<T> {
    fn replace(&self, request: Option<T>) {
        let mut state = self.state.lock().unwrap();
        state.generation = state.generation.wrapping_add(1);
        state.pending =
            request.map(|request| (request, Instant::now() + SEARCH_DEBOUNCE));
        self.changed.notify_all();
    }

    fn take(&self) -> Option<(u64, T)> {
        let mut state = self.state.lock().unwrap();
        loop {
            if state.closed {
                return None;
            }
            if let Some((_, ready)) = &state.pending {
                let delay = ready.saturating_duration_since(Instant::now());
                if delay.is_zero() {
                    return Some((state.generation, state.pending.take().unwrap().0));
                }
                state = self.changed.wait_timeout(state, delay).unwrap().0;
            } else {
                state = self.changed.wait(state).unwrap();
            }
        }
    }

    // Publication and retry must be atomic with query replacement. The publish
    // closure only sends to the reply channel; render callbacks run after return.
    fn complete(
        &self,
        generation: u64,
        retry: Option<T>,
        publish: impl FnOnce() -> bool,
    ) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.closed || generation != state.generation || !publish() {
            return false;
        }
        if let Some(request) = retry {
            state.pending = Some((request, Instant::now() + Duration::from_secs(1)));
            self.changed.notify_all();
        }
        true
    }

    fn close(&self) {
        let mut state = self.state.lock().unwrap();
        state.closed = true;
        state.pending = None;
        self.changed.notify_all();
    }
}

struct SearchWorkers(Arc<SearchQueue<SearchRequest>>);

impl Drop for SearchWorkers {
    fn drop(&mut self) {
        // Idle workers exit immediately; blocked workers exit after HTTP returns.
        self.0.close();
    }
}

struct SearchReply {
    identity: SearchIdentity,
    result: Result<(Vec<(NeoismAgentSessionEntry, String)>, bool, bool), String>,
}

pub(crate) struct ConversationSearchState {
    identity: SearchIdentity,
    requests: Option<SearchWorkers>,
    replies: (Sender<SearchReply>, Receiver<SearchReply>),
    project_rows: Vec<Row>,
    captured_buttons: Vec<neoism_window::event::MouseButton>,
    entries: HashMap<String, NeoismAgentSessionEntry>,
    transcript_unavailable: HashSet<String>,
}

impl Default for ConversationSearchState {
    fn default() -> Self {
        Self {
            identity: SearchIdentity::default(),
            requests: None,
            replies: mpsc::channel(),
            project_rows: Vec::new(),
            captured_buttons: Vec::new(),
            entries: HashMap::new(),
            transcript_unavailable: HashSet::new(),
        }
    }
}

fn fetch(request: &SearchRequest) -> SearchReply {
    let identity = request.identity.clone();
    let result = if identity.query.trim().is_empty() {
        // Initial view uses the same sidebar scope as transcript search.
        // No transcript request is necessary.
        let mut cursor = None;
        let mut entries = Vec::new();
        let mut error = None;
        for _ in 0..2 {
            match fetch_session_entries_page(
                &identity.server,
                request.current.as_deref(),
                identity.directory.as_deref(),
                cursor.as_deref(),
            ) {
                Ok((page, next)) => {
                    entries.extend(page.into_iter().map(|entry| (entry, String::new())));
                    if next.is_none() || next == cursor {
                        break;
                    }
                    cursor = next;
                }
                Err(message) => {
                    error = Some(message);
                    break;
                }
            }
        }
        if entries.is_empty() && error.is_some() {
            Err(error.unwrap())
        } else {
            Ok((entries, false, false))
        }
    } else {
        fetch_session_search_hits(
            &identity.server,
            &identity.query,
            request.current.as_deref(),
            identity.directory.as_deref(),
            request.transcript_enabled,
        )
        .map(|(hits, unavailable, indexing)| {
            (
                hits.into_iter()
                    .map(|hit| (hit.entry, hit.excerpt))
                    .collect(),
                unavailable,
                indexing,
            )
        })
    };
    SearchReply { identity, result }
}

impl ConversationSearchState {
    fn submit(&mut self, request: SearchRequest) -> Result<(), String> {
        if self.requests.is_none() {
            let workers =
                SearchWorkers(Arc::new(SearchQueue::<SearchRequest>::default()));
            for index in 0..2 {
                let queue = Arc::clone(&workers.0);
                let replies = self.replies.0.clone();
                std::thread::Builder::new()
                    .name(format!("neoism-conversation-search-{index}"))
                    .spawn(move || {
                        while let Some((generation, request)) = queue.take() {
                            let reply = fetch(&request);
                            let retry = matches!(&reply.result, Ok((_, false, true)));
                            let wake = request.wake.clone();
                            // Only the nonblocking channel send runs under the queue lock.
                            // The UI identity guard covers already-published replies.
                            if queue.complete(
                                generation,
                                retry.then_some(request),
                                || replies.send(reply).is_ok(),
                            ) {
                                // Each completion needs an event, including indexing
                                // retries sharing this wake's coalescing flag.
                                wake.begin_drain();
                                wake.wake();
                            }
                        }
                    })
                    .map_err(|error| error.to_string())?;
            }
            self.requests = Some(workers);
        }
        self.requests.as_ref().unwrap().0.replace(Some(request));
        Ok(())
    }
}

fn project_label(directory: Option<&str>) -> String {
    directory
        .filter(|path| !path.is_empty())
        .map(|path| {
            path.trim_end_matches(['/', '\\'])
                .rsplit(['/', '\\'])
                .next()
                .filter(|name| !name.is_empty())
                .unwrap_or(path)
                .to_owned()
        })
        .unwrap_or_else(|| "Unknown project".into())
}

fn search_rows(
    entries: Vec<(NeoismAgentSessionEntry, String)>,
    current: Option<&str>,
) -> (Vec<Row>, HashMap<String, NeoismAgentSessionEntry>) {
    let mut projects: HashMap<String, HashSet<String>> = HashMap::new();
    for (entry, _) in &entries {
        projects
            .entry(project_label(entry.directory.as_deref()))
            .or_default()
            .insert(entry.directory.clone().unwrap_or_default());
    }
    let mut cache = HashMap::new();
    let mut rows = Vec::new();
    for (entry, excerpt) in entries {
        let value = entry.stable_identity().to_owned();
        if cache.contains_key(&value) {
            continue;
        }
        let project = project_label(entry.directory.as_deref());
        let path = entry.directory.as_deref().unwrap_or("");
        let duplicate = projects.get(&project).is_some_and(|paths| paths.len() > 1);
        let description = if duplicate && !path.is_empty() {
            if excerpt.is_empty() {
                path.to_owned()
            } else {
                format!("{excerpt} · {path}")
            }
        } else {
            excerpt
        };
        rows.push(Row {
            value: value.clone(),
            title: if entry.title.trim().is_empty() {
                "Untitled conversation".into()
            } else {
                entry.title.clone()
            },
            description,
            footer: project,
            is_current: current == Some(entry.id.as_str()),
            is_disabled: false,
        });
        cache.insert(value, entry);
    }
    (rows, cache)
}

fn can_add_workspace(catalog_directory: Option<&str>) -> bool {
    catalog_directory.is_none()
}

impl Screen<'_> {
    fn conversation_search_endpoint(&self) -> String {
        self.context_manager
            .agent_server_override_for_current()
            .unwrap_or_else(crate::neoism::agent::neoism_agent_server)
    }

    pub fn open_conversation_search(&mut self) {
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_navigation_header_enabled(true);
        self.renderer
            .conversation_navigator
            .open(Mode::Search, Vec::new());
        self.renderer.conversation_navigator.set_anchor(None);
        self.queue_conversation_search();
        self.mark_dirty();
    }

    pub fn open_conversation_project_picker(&mut self) {
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_navigation_header_enabled(true);
        self.invalidate_conversation_search();
        let rows = self.native_conversation_project_rows();
        self.conversation_search.project_rows = rows.clone();
        let anchor = self
            .renderer
            .conversations_pane
            .side_panel()
            .project_button_rect();
        self.renderer
            .conversation_navigator
            .open(Mode::Projects, rows);
        self.renderer.conversation_navigator.set_anchor(anchor);
        self.mark_dirty();
    }

    fn native_conversation_project_rows(&mut self) -> Vec<Row> {
        let mut rows = self.conversation_project_rows();
        if can_add_workspace(
            self.renderer
                .conversations_pane
                .side_panel()
                .catalog_directory(),
        ) {
            rows.push(Row {
                value: "native:add-conversation-workspace".into(),
                title: "Add workspace…".into(),
                description: "Choose a project folder".into(),
                footer: String::new(),
                is_current: false,
                is_disabled: false,
            });
        }
        rows
    }

    fn invalidate_conversation_search(&mut self) {
        if let Some(requests) = &self.conversation_search.requests {
            requests.0.replace(None);
        }
        self.conversation_search
            .identity
            .advance(String::new(), String::new(), None);
        self.conversation_search.entries.clear();
    }

    pub(crate) fn close_conversation_navigator(&mut self) {
        self.renderer.conversation_navigator.close();
        self.invalidate_conversation_search();
        self.mark_dirty();
    }

    fn queue_conversation_search(&mut self) {
        let server = self.conversation_search_endpoint();
        let query = self.renderer.conversation_navigator.query().to_owned();
        let directory = self
            .renderer
            .conversations_pane
            .side_panel()
            .catalog_directory()
            .map(str::to_owned);
        self.conversation_search
            .identity
            .advance(server.clone(), query, directory);
        self.conversation_search.entries.clear();
        self.renderer.conversation_navigator.set_rows(Vec::new());
        self.renderer.conversation_navigator.set_error(None);
        self.renderer.conversation_navigator.set_loading(true);
        let current = self
            .context_manager
            .current()
            .neoism_agent
            .as_ref()
            .and_then(|agent| agent.session_id_str())
            .map(str::to_owned);
        let request = SearchRequest {
            identity: self.conversation_search.identity.clone(),
            current,
            transcript_enabled: !self
                .conversation_search
                .transcript_unavailable
                .contains(&server),
            wake: AgentEventWake::new(
                self.context_manager.event_proxy(),
                self.context_manager.window_id(),
            ),
        };
        if let Err(error) = self.conversation_search.submit(request) {
            self.renderer.conversation_navigator.set_loading(false);
            self.renderer.conversation_navigator.set_error(Some(error));
        }
    }

    pub(crate) fn conversation_navigator_query_changed(&mut self) {
        if matches!(
            self.renderer.conversation_navigator.mode(),
            Some(Mode::Search)
        ) {
            self.queue_conversation_search();
        }
        self.mark_dirty();
    }

    pub(crate) fn pump_conversation_navigator(&mut self) {
        self.pump_conversation_workspaces();
        if matches!(
            self.renderer.conversation_navigator.mode(),
            Some(Mode::Projects)
        ) {
            let rows = self.native_conversation_project_rows();
            if rows != self.conversation_search.project_rows {
                self.conversation_search.project_rows = rows.clone();
                self.renderer.conversation_navigator.set_rows(rows);
            }
        }
        // Enable the header even before the first open, including after renderer reloads.
        self.renderer
            .conversations_pane
            .side_panel_mut()
            .set_navigation_header_enabled(true);
        let server = self.conversation_search_endpoint();
        let search = matches!(
            self.renderer.conversation_navigator.mode(),
            Some(Mode::Search)
        );
        let directory = self
            .renderer
            .conversations_pane
            .side_panel()
            .catalog_directory()
            .map(str::to_owned);
        if search
            && (self.conversation_search.identity.server != server
                || self.conversation_search.identity.directory != directory)
        {
            self.close_conversation_navigator();
        }
        while let Ok(reply) = self.conversation_search.replies.1.try_recv() {
            let active = matches!(
                self.renderer.conversation_navigator.mode(),
                Some(Mode::Search)
            );
            if !self.conversation_search.identity.accepts(
                &reply.identity,
                active,
                &server,
                directory.as_deref(),
            ) {
                continue;
            }
            self.renderer.conversation_navigator.set_loading(false);
            match reply.result {
                Ok((entries, unavailable, indexing)) => {
                    self.renderer.conversation_navigator.set_indexing(indexing);
                    if unavailable {
                        self.conversation_search
                            .transcript_unavailable
                            .insert(server.clone());
                    }
                    let current = self
                        .context_manager
                        .current()
                        .neoism_agent
                        .as_ref()
                        .and_then(|agent| agent.session_id_str());
                    let (rows, cache) = search_rows(entries, current);
                    self.conversation_search.entries = cache;
                    self.renderer.conversation_navigator.set_rows(rows);
                    self.renderer.conversation_navigator.set_error(None);
                }
                Err(error) => self.renderer.conversation_navigator.set_error(Some(error)),
            }
            self.mark_dirty();
        }
    }

    pub(crate) fn activate_conversation_navigator_value(&mut self, value: String) {
        match self.renderer.conversation_navigator.mode() {
            Some(Mode::Search) => {
                let server = self.conversation_search.identity.server.clone();
                if server != self.conversation_search_endpoint()
                    || self.conversation_search.identity.directory.as_deref()
                        != self
                            .renderer
                            .conversations_pane
                            .side_panel()
                            .catalog_directory()
                {
                    self.close_conversation_navigator();
                    return;
                }
                if let Some(entry) = self.conversation_search.entries.remove(&value) {
                    self.close_conversation_navigator();
                    self.open_conversation_catalog_entry(entry, &server);
                }
            }
            Some(Mode::Projects) => {
                if value == "native:add-conversation-workspace" {
                    let allowed = can_add_workspace(
                        self.renderer
                            .conversations_pane
                            .side_panel()
                            .catalog_directory(),
                    );
                    self.close_conversation_navigator();
                    if allowed {
                        self.open_add_conversation_workspace_browser();
                    }
                    return;
                }
                match self.activate_conversation_project(&value) {
                    Ok(()) => self.close_conversation_navigator(),
                    Err(error) => {
                        self.renderer.notifications.push(
                            error,
                            neoism_ui::panels::notifications::NotificationLevel::Warn,
                        );
                    }
                }
            }
            None => {}
        }
        self.mark_dirty();
    }

    pub(crate) fn handle_conversation_navigator_mouse(
        &mut self,
        button: neoism_window::event::MouseButton,
        state: neoism_window::event::ElementState,
    ) -> bool {
        use neoism_window::event::{ElementState, MouseButton};
        let active = self.renderer.conversation_navigator.is_active();
        if state == ElementState::Released {
            let captured = self.conversation_search.captured_buttons.contains(&button);
            self.conversation_search
                .captured_buttons
                .retain(|held| *held != button);
            return active || captured;
        }
        self.conversation_search
            .captured_buttons
            .retain(|held| *held != button);
        if !active {
            return false;
        }
        self.conversation_search.captured_buttons.push(button);
        if button == MouseButton::Left {
            self.handle_conversation_navigator_click();
        }
        true
    }

    pub(crate) fn handle_conversation_navigator_click(&mut self) {
        let (x, y) = self.mouse_logical_for_hit_test();
        match self.renderer.conversation_navigator.pointer_down(x, y) {
            Some(Action::Activate(value)) => {
                self.activate_conversation_navigator_value(value)
            }
            Some(Action::Dismiss) => self.close_conversation_navigator(),
            None => {}
        }
        self.mark_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready<T>(queue: &SearchQueue<T>) {
        queue.state.lock().unwrap().pending.as_mut().unwrap().1 = Instant::now();
        queue.changed.notify_all();
    }

    #[test]
    fn pending_queries_coalesce_and_keep_the_debounce() {
        let queue = SearchQueue::default();
        for query in 0..100 {
            let before = Instant::now();
            queue.replace(Some(query));
            let state = queue.state.lock().unwrap();
            let (pending, deadline) = state.pending.as_ref().unwrap();
            assert_eq!(*pending, query);
            assert!(*deadline >= before + Duration::from_millis(160));
        }
        ready(&queue);
        assert_eq!(queue.take(), Some((100, 99)));
        assert!(queue.state.lock().unwrap().pending.is_none());
    }

    #[test]
    fn spare_worker_bypasses_blocked_fetch_and_saturation_keeps_only_latest() {
        let queue = Arc::new(SearchQueue::default());
        let (started, starts) = mpsc::channel();
        let mut workers = Vec::new();
        let mut releases = Vec::new();
        for _ in 0..2 {
            let queue = Arc::clone(&queue);
            let started = started.clone();
            let (release, blocked) = mpsc::channel();
            releases.push(release);
            workers.push(std::thread::spawn(move || {
                let request = queue.take().unwrap();
                started.send(request).unwrap();
                // Fake synchronous HTTP: no completion until explicitly released.
                blocked.recv().unwrap();
                assert!(queue.take().is_none());
            }));
        }
        queue.replace(Some("slow"));
        ready(&queue);
        let (old, query) = starts.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(query, "slow");
        queue.replace(Some("new"));
        ready(&queue);
        let (new, query) = starts.recv_timeout(Duration::from_secs(2)).unwrap();
        assert_eq!(query, "new");
        assert_ne!(old, new);
        // Both slots are occupied. No additional worker is created; only the
        // newest debounced request is retained, not a FIFO of obsolete queries.
        for query in ["third", "fourth", "latest"] {
            queue.replace(Some(query));
        }
        assert_eq!(
            queue.state.lock().unwrap().pending.as_ref().unwrap().0,
            "latest"
        );
        queue.close();
        for release in releases {
            release.send(()).unwrap();
        }
        for worker in workers {
            worker.join().unwrap();
        }
    }

    #[test]
    fn cancel_invalidates_inflight_and_clears_pending_without_waiting() {
        let queue = SearchQueue::default();
        queue.replace(Some("inflight"));
        ready(&queue);
        let (generation, _) = queue.take().unwrap();
        queue.replace(Some("pending"));
        queue.replace(None);
        let state = queue.state.lock().unwrap();
        assert_ne!(generation, state.generation);
        assert!(state.pending.is_none());
        drop(state);
        queue.close();
        assert!(queue.take().is_none());
    }

    #[test]
    fn stale_completion_cannot_publish_or_replace_newer_pending_query() {
        let queue = SearchQueue::default();
        queue.replace(Some("old"));
        ready(&queue);
        let (generation, _) = queue.take().unwrap();
        queue.replace(Some("latest"));
        assert!(!queue.complete(generation, Some("old retry"), || {
            panic!("obsolete completion must not publish")
        }));
        ready(&queue);
        assert_eq!(queue.take().unwrap().1, "latest");
        queue.replace(None);
        assert!(!queue.complete(generation, None, || {
            panic!("cancelled completion must not publish")
        }));
    }

    #[test]
    fn completion_retry_is_interruptible_and_wake_can_reenter_queue() {
        let queue = Arc::new(SearchQueue::default());
        queue.replace(Some("indexing"));
        ready(&queue);
        let (generation, request) = queue.take().unwrap();
        let reentrant_queue = Arc::clone(&queue);
        let wake = AgentEventWake::for_test(move || {
            // A render callback must not run with the queue lock held.
            reentrant_queue.replace(Some("new query"));
        });
        assert!(queue.complete(generation, Some(request), || true));
        {
            let state = queue.state.lock().unwrap();
            assert_eq!(state.pending.as_ref().unwrap().0, "indexing");
        }
        wake.begin_drain();
        wake.wake();
        ready(&queue);
        let (next_generation, next_request) = queue.take().unwrap();
        assert_eq!(next_request, "new query");
        // Reuse the same coalescing wake as an indexing retry would. It must
        // emit again, rather than strand the second completion without a frame.
        assert!(queue.complete(next_generation, None, || true));
        wake.begin_drain();
        wake.wake();
        ready(&queue);
        assert_eq!(queue.take().unwrap().1, "new query");
        queue.close();
        assert!(!queue.complete(generation, Some(request), || {
            panic!("closed queue must not publish or retry")
        }));
    }

    #[test]
    fn dropping_worker_owner_releases_idle_workers() {
        let owner = SearchWorkers(Arc::new(SearchQueue::default()));
        let queue = Arc::clone(&owner.0);
        let workers: Vec<_> = (0..2)
            .map(|_| {
                let queue = Arc::clone(&queue);
                std::thread::spawn(move || assert!(queue.take().is_none()))
            })
            .collect();
        drop(owner);
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(Arc::strong_count(&queue), 1);
    }

    #[test]
    fn add_workspace_is_root_dashboard_only_including_stale_activation() {
        assert!(can_add_workspace(None));
        assert!(!can_add_workspace(Some("/project")));
        assert!(!can_add_workspace(Some("/")));
        assert!(!can_add_workspace(Some("")));
    }

    #[test]
    fn generation_rejects_stale_same_query_and_endpoint() {
        let mut identity = SearchIdentity::default();
        identity.advance("server-a".into(), "hello".into(), None);
        let old = identity.clone();
        identity.advance("server-a".into(), "hello".into(), None);
        assert!(!identity.accepts(&old, true, "server-a", None));
        assert!(identity.accepts(&identity, true, "server-a", None));
        assert!(!identity.accepts(&identity, false, "server-a", None));
        assert!(!identity.accepts(&identity, true, "server-b", None));
    }

    #[test]
    fn dismiss_and_project_picker_invalidate_inflight_results() {
        let mut identity = SearchIdentity::default();
        identity.advance("a".into(), "".into(), None);
        let inflight = identity.clone();
        identity.advance(String::new(), String::new(), None);
        assert!(!identity.accepts(&inflight, false, "a", None));
        identity.advance("a".into(), "".into(), None);
        assert!(!identity.accepts(&inflight, true, "a", None));
    }

    #[test]
    fn same_server_scope_switch_rejects_replies_before_and_after_invalidation() {
        let mut identity = SearchIdentity::default();
        identity.advance("a".into(), "needle".into(), None);
        let global = identity.clone();
        assert!(!identity.accepts(&global, true, "a", Some("/project")));
        identity.advance("a".into(), "needle".into(), Some("/project".into()));
        assert!(!identity.accepts(&global, true, "a", Some("/project")));
        assert!(identity.accepts(&identity, true, "a", Some("/project")));
        assert!(!identity.accepts(&identity, true, "a", None));
        assert!(!identity.accepts(&identity, true, "a", Some("/other")));
    }

    #[test]
    fn rows_keep_full_entries_and_disambiguate_projects() {
        let mut first = NeoismAgentSessionEntry::new("one", "First", "");
        first.directory = Some("/work/one/app".into());
        let mut second = NeoismAgentSessionEntry::new("two", "Second", "");
        second.directory = Some("/work/two/app".into());
        let (rows, cache) = search_rows(
            vec![(first.clone(), "excerpt".into()), (second, String::new())],
            Some("one"),
        );
        assert_eq!(rows[0].footer, "app");
        assert!(rows[0].description.contains("/work/one/app"));
        assert!(rows[0].description.contains("excerpt"));
        assert!(rows[0].is_current);
        assert_eq!(cache.get(&rows[0].value), Some(&first));
    }
}
