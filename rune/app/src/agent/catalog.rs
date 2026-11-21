//! The all-projects conversation catalog: the daemon lists the projects that have chats
//! (`ListSessionProjects`) and serves each one's chats five at a time
//! (`ListProjectSessions`). The sidebar loads the first page of a dozen projects and the next
//! dozen as the list is scrolled toward its end; "Load more" under a project fetches more of
//! that project's chats. `App::threads` stays the flat
//! list everything else reads: the pins, then every loaded page.

use crate::app::App;
use neoism_protocol::agent::{AgentClientMessage, SessionProjectSummary, ThreadSummary};
use serde_json::Value;
use std::collections::HashMap;

const IDENTITY: &str = "rune-sidebar";
/// Projects whose first page is asked for at once, and per "Load more projects".
const PROJECT_BATCH: usize = 12;

#[derive(Default)]
struct Page {
    threads: Vec<ThreadSummary>,
    next: Option<String>,
    /// A further page was loaded: a refreshed first page must not forget where it got to.
    extended: bool,
    loading: bool,
}

#[derive(Default)]
pub struct Catalog {
    generation: std::cell::Cell<u64>,
    /// Every project with chats, most recent first.
    projects: Vec<String>,
    /// How many of them have been asked for.
    wanted: usize,
    pages: HashMap<String, Page>,
    pinned: Vec<ThreadSummary>,
    /// First pages asked for and not yet answered: the next batch waits for them.
    pending: usize,
}

fn trimmed(directory: &str) -> String { directory.trim_end_matches('/').to_owned() }

/// A pinned session as the agent server lists it (`thread_summary_from_session`).
fn pinned_summary(session: &Value) -> Option<ThreadSummary> {
    Some(ThreadSummary {
        session_id: session["id"].as_str()?.to_owned(),
        title: session["title"].as_str().unwrap_or("Untitled").to_owned(),
        directory: session["directory"].as_str().map(str::to_owned),
        model: session["model"].as_str().map(str::to_owned),
        agent: session["agent"].as_str().map(str::to_owned),
        external_provider: session["parentId"].is_null().then(|| session["externalAgent"]["provider"].as_str().map(str::to_owned)).flatten(),
        updated_at: session["time"]["updated"].as_u64().unwrap_or(0),
        message_count: 0,
        catalog_activity: session["catalogActivity"].as_str().map(str::to_owned),
        busy: false,
        pinned: true,
    })
}

impl Catalog {
    /// Whether `directory` has chats the daemon has not been asked for yet.
    pub fn more_in(&self, directory: &str) -> bool { self.pages.get(directory).is_some_and(|p| p.next.is_some()) }
    pub fn loading(&self, directory: &str) -> bool { self.pages.get(directory).is_some_and(|p| p.loading) }
    /// Every project the daemon listed, most recent first.
    pub fn directories(&self) -> &[String] { &self.projects }
    /// Projects not asked for yet.
    pub fn more_projects(&self) -> usize { self.projects.len().saturating_sub(self.wanted) }

    /// The pins, then each loaded project's chats, each conversation once.
    fn flat(&self) -> Vec<ThreadSummary> {
        let mut seen = std::collections::HashSet::new();
        let mut threads = Vec::new();
        let pages = self.projects.iter().filter_map(|d| self.pages.get(d)).flat_map(|p| p.threads.iter());
        for thread in self.pinned.iter().chain(pages) {
            if seen.insert(thread.session_id.clone()) { threads.push(thread.clone()); }
        }
        threads
    }
}

impl App {
    /// Ask for the project list; its reply asks for the pages.
    pub fn refresh_catalog(&self) {
        self.catalog.generation.set(self.catalog.generation.get() + 1);
        let generation = self.catalog.generation.get();
        self.send_agent(AgentClientMessage::ListSessionProjects { catalog_identity: IDENTITY.into(), generation });
    }

    fn request_project_page(&mut self, directory: &str, cursor: Option<String>) {
        let page = self.catalog.pages.entry(directory.to_owned()).or_default();
        if cursor.is_some() { page.loading = true; } else { self.catalog.pending += 1; }
        let generation = self.catalog.generation.get();
        self.send_agent(AgentClientMessage::ListProjectSessions { catalog_identity: IDENTITY.into(), directory: directory.to_owned(), cursor, generation });
    }

    /// "Load more" under a project: its next five chats.
    pub fn load_more_chats(&mut self, directory: &str) {
        let Some(page) = self.catalog.pages.get(directory).filter(|p| !p.loading) else { return };
        let Some(cursor) = page.next.clone() else { return };
        self.request_project_page(directory, Some(cursor));
    }

    /// The next dozen projects' first pages, as the list is scrolled toward its end
    /// (`conversation_autoload`); one batch at a time.
    pub fn load_more_projects(&mut self) {
        if self.catalog.pending > 0 { return; }
        let from = self.catalog.wanted;
        let to = (from + PROJECT_BATCH).min(self.catalog.projects.len());
        self.catalog.wanted = to;
        for directory in self.catalog.projects[from..to].to_vec() { self.request_project_page(&directory, None); }
    }

    pub(crate) fn apply_session_projects(&mut self, identity: &str, items: Vec<SessionProjectSummary>, pinned: Vec<Value>) {
        if identity != IDENTITY { return; }
        let mut items = items;
        items.sort_by(|a, b| b.updated_ms.cmp(&a.updated_ms).then_with(|| a.directory.cmp(&b.directory)));
        self.catalog.projects = items.into_iter().map(|p| trimmed(&p.directory)).collect();
        self.catalog.pinned = pinned.iter().filter_map(pinned_summary).collect();
        let listed: std::collections::HashSet<&String> = self.catalog.projects.iter().collect();
        self.catalog.pages.retain(|directory, _| listed.contains(directory));
        // Refresh what was already asked for (at least the first batch), newest projects first.
        let wanted = self.catalog.wanted.max(PROJECT_BATCH).min(self.catalog.projects.len());
        self.catalog.wanted = wanted;
        self.catalog.pending = 0;
        for directory in self.catalog.projects[..wanted].to_vec() { self.request_project_page(&directory, None); }
        if self.conversations_dashboard() { self.threads = self.catalog.flat(); }
        self.refresh_sessions_menu();
    }

    pub(crate) fn apply_project_page(&mut self, identity: &str, directory: &str, requested: Option<String>, threads: Vec<ThreadSummary>, next: Option<String>) {
        if identity != IDENTITY { return; }
        for thread in &threads {
            if let Some(activity) = &thread.catalog_activity {
                self.set_streaming(&thread.session_id, activity == "running");
                self.note_activity(&thread.session_id, activity);
            }
        }
        if requested.is_none() { self.catalog.pending = self.catalog.pending.saturating_sub(1); }
        let page = self.catalog.pages.entry(trimmed(directory)).or_default();
        if requested.is_some() {
            // A further page: after what is loaded.
            page.loading = false;
            page.extended = true;
            page.threads.retain(|t| !threads.iter().any(|n| n.session_id == t.session_id));
            page.threads.extend(threads);
            page.next = next;
        } else if page.extended {
            // A refreshed first page keeps the chats loaded beyond it, and their cursor.
            let older: Vec<ThreadSummary> = page.threads.drain(..).filter(|t| !threads.iter().any(|n| n.session_id == t.session_id)).collect();
            page.threads = threads;
            page.threads.extend(older);
        } else {
            page.threads = threads;
            page.next = next;
        }
        if self.conversations_dashboard() { self.threads = self.catalog.flat(); }
        self.refresh_sessions_menu();
    }

    pub(crate) fn project_page_failed(&mut self, identity: &str, directory: &str, requested: Option<String>) {
        if identity != IDENTITY { return; }
        if requested.is_none() { self.catalog.pending = self.catalog.pending.saturating_sub(1); }
        if let Some(page) = self.catalog.pages.get_mut(&trimmed(directory)) { page.loading = false; }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_conversation_that_finishes_off_screen_is_unread_until_opened() {
        let mut app = App::new();
        // Nothing is marked from history: idle with no work seen before it.
        app.note_activity("old", "idle");
        assert!(app.finished.is_empty());
        app.note_activity("a", "running");
        app.note_activity("a", "permission");
        app.note_activity("a", "idle");
        assert!(app.finished.is_empty(), "an approval answered is not a finish");
        app.note_activity("a", "running");
        app.note_activity("a", "idle");
        assert!(app.finished.contains("a"));
        app.note_activity("a", "running");
        assert!(app.finished.is_empty(), "working again clears it");
        app.note_activity("a", "background");
        app.note_activity("a", "idle");
        app.open_conversation("a");
        assert!(app.finished.is_empty(), "opening it reads it");
    }

    fn thread(id: &str, directory: &str, at: u64) -> ThreadSummary {
        serde_json::from_value(serde_json::json!({"session_id": id, "title": id, "directory": directory, "updated_at": at})).unwrap()
    }

    #[test]
    fn projects_load_five_at_a_time_and_a_refresh_keeps_what_was_loaded() {
        let mut app = App::new();
        app.conv_all = true;
        let projects = (0..15).map(|i| SessionProjectSummary { directory: format!("/p{i:02}"), updated_ms: 100 - i }).collect();
        app.apply_session_projects(IDENTITY, projects, vec![serde_json::json!({"id": "pin", "title": "Pinned", "directory": "/p09", "time": {"updated": 3}})]);
        assert_eq!(app.catalog.wanted, PROJECT_BATCH);
        assert_eq!(app.catalog.more_projects(), 3);
        assert_eq!(app.threads.len(), 1, "the pins show before any page lands");
        assert!(app.threads[0].pinned);

        let first: Vec<ThreadSummary> = (0..5).map(|i| thread(&format!("a{i}"), "/p00", 50 - i)).collect();
        app.apply_project_page(IDENTITY, "/p00/", None, first.clone(), Some("c1".into()));
        assert!(app.catalog.more_in("/p00"));
        app.load_more_chats("/p00");
        assert!(app.catalog.loading("/p00"));
        app.apply_project_page(IDENTITY, "/p00", Some("c1".into()), (5..8).map(|i| thread(&format!("a{i}"), "/p00", 50 - i)).collect(), None);
        assert!(!app.catalog.more_in("/p00") && !app.catalog.loading("/p00"));
        assert_eq!(app.threads.len(), 9);

        // A refresh of the first page (a new chat on top) keeps the three loaded beyond it.
        let mut refreshed = vec![thread("new", "/p00", 99)];
        refreshed.extend(first[..4].iter().cloned());
        app.apply_project_page(IDENTITY, "/p00", None, refreshed, Some("c1b".into()));
        assert_eq!(app.threads.len(), 10);
        assert_eq!(app.threads[1].session_id, "new");
        assert!(!app.catalog.more_in("/p00"), "the loaded tail's own end stands");

        // The next batch waits for the first pages still out, then loads by itself on scroll.
        app.load_more_projects();
        assert_eq!(app.catalog.more_projects(), 3);
        for i in 1..PROJECT_BATCH { app.apply_project_page(IDENTITY, &format!("/p{i:02}"), None, Vec::new(), None); }
        app.load_more_projects();
        assert_eq!(app.catalog.more_projects(), 0);
        app.apply_project_page("someone-else", "/p01", None, vec![thread("x", "/p01", 1)], None);
        assert_eq!(app.threads.len(), 10);
    }
}
