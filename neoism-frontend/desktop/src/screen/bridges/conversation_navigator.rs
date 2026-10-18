//! Native ownership of the shared conversation Search / Projects overlay.
//! Search never mutates the workspace-scoped sidebar catalog.
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

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
}

impl SearchIdentity {
    fn advance(&mut self, server: String, query: String) {
        self.generation = self.generation.wrapping_add(1);
        self.server = server;
        self.query = query;
    }

    fn accepts(&self, reply: &Self, active_search: bool, current_server: &str) -> bool {
        active_search && self == reply && self.server == current_server
    }
}

struct SearchRequest {
    identity: SearchIdentity,
    current: Option<String>,
    transcript_enabled: bool,
    wake: AgentEventWake,
}

enum SearchCommand {
    Query(SearchRequest),
    Cancel,
}

struct SearchReply {
    identity: SearchIdentity,
    result: Result<(Vec<(NeoismAgentSessionEntry, String)>, bool, bool), String>,
}

pub(crate) struct ConversationSearchState {
    identity: SearchIdentity,
    requests: Option<Sender<SearchCommand>>,
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
        // Initial view is a bounded, newest-first GLOBAL authorized root list.
        // No directory is sent, and no transcript request is necessary.
        let mut cursor = None;
        let mut entries = Vec::new();
        let mut error = None;
        for _ in 0..2 {
            match fetch_session_entries_page(
                &identity.server,
                request.current.as_deref(),
                None,
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
            None,
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
            let (tx, rx) = mpsc::channel::<SearchCommand>();
            let replies = self.replies.0.clone();
            std::thread::Builder::new()
                .name("neoism-conversation-search".into())
                .spawn(move || {
                    let mut pending: Option<SearchRequest> = None;
                    let mut delay = Duration::from_millis(160);
                    loop {
                        let command = if pending.is_some() {
                            rx.recv_timeout(delay)
                        } else {
                            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                        };
                        match command {
                            Ok(SearchCommand::Query(request)) => {
                                pending = Some(request);
                                delay = Duration::from_millis(160);
                            }
                            Ok(SearchCommand::Cancel) => pending = None,
                            Err(mpsc::RecvTimeoutError::Disconnected) => return,
                            Err(mpsc::RecvTimeoutError::Timeout) => {
                                let request = pending.take().unwrap();
                                let reply = fetch(&request);
                                let retry = matches!(&reply.result, Ok((_, false, true)));
                                if replies.send(reply).is_err() {
                                    return;
                                }
                                request.wake.wake();
                                if retry {
                                    // Same identity, one existing worker, interruptible wait.
                                    // Cancel/query commands queued during HTTP win before retry.
                                    pending = Some(request);
                                    delay = Duration::from_secs(1);
                                }
                            }
                        }
                    }
                })
                .map_err(|error| error.to_string())?;
            self.requests = Some(tx);
        }
        self.requests
            .as_ref()
            .unwrap()
            .send(SearchCommand::Query(request))
            .map_err(|error| error.to_string())
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
            let _ = requests.send(SearchCommand::Cancel);
        }
        self.conversation_search
            .identity
            .advance(String::new(), String::new());
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
        self.conversation_search
            .identity
            .advance(server.clone(), query);
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
        if search && self.conversation_search.identity.server != server {
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
                if server != self.conversation_search_endpoint() {
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
        identity.advance("server-a".into(), "hello".into());
        let old = identity.clone();
        identity.advance("server-a".into(), "hello".into());
        assert!(!identity.accepts(&old, true, "server-a"));
        assert!(identity.accepts(&identity, true, "server-a"));
        assert!(!identity.accepts(&identity, false, "server-a"));
        assert!(!identity.accepts(&identity, true, "server-b"));
    }

    #[test]
    fn dismiss_and_project_picker_invalidate_inflight_results() {
        let mut identity = SearchIdentity::default();
        identity.advance("a".into(), "".into());
        let inflight = identity.clone();
        identity.advance(String::new(), String::new());
        assert!(!identity.accepts(&inflight, false, "a"));
        identity.advance("a".into(), "".into());
        assert!(!identity.accepts(&inflight, true, "a"));
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
