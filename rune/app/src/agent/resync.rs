//! Coming back to conversations that went on without this window: the lid closed and opened,
//! a network dropped and returned, a server or its agent restarted.
//!
//! Nothing is replayed. An event stream is followed from where the agent is now (a live tail:
//! it opens with whatever reply is being written, whole, and carries on from there), and each
//! transcript takes the newest page of its history, which lays over what is loaded and changes
//! only the rows that differ (`merge_head`). So a conversation still at work is back at its
//! live edge at once, one that finished shows how it ended, and everything in between is
//! there to scroll back through.
//!
//! Three things bring this about:
//! - the daemon socket was lost and dialed again (`Inbound::Connected { again }`): the new
//!   socket follows nothing, and whatever was asked on the old one is never answered;
//! - the machine woke with its socket intact (`Inbound::Woke`): nothing was lost on the way
//!   here, but it is cheap to be sure;
//! - the daemon lost and found its own stream from the agent (`stream.resumed`): the events
//!   in between never reached it.

use super::HEAD_PAGE;
use crate::app::App;
use crate::tabs::TabKind;
use neoism_protocol::agent::AgentClientMessage;

impl App {
    /// The conversations open in this workspace's tabs.
    fn open_conversations(&self) -> Vec<String> {
        let mut open: Vec<String> = self.tabs.iter().filter_map(|t| match &t.kind { TabKind::Agent { session: Some(id) } => Some(id.clone()), _ => None }).collect();
        open.sort();
        open.dedup();
        open
    }

    /// Follow every conversation again and bring its transcript up to date. `reconnected`:
    /// the socket is a new one.
    pub(crate) fn agent_resync(&mut self, reconnected: bool) {
        if reconnected {
            self.composer.prompts.reconnect();
            // Whatever was asked on the old socket and not answered never will be: an older
            // page still marked as on its way would never be asked for again.
            for session in self.sessions.values_mut() { session.loading_older = false; }
            let lost: Vec<String> = self.prefetching.iter().filter(|id| self.sessions.contains_key(*id)).cloned().collect();
            for id in lost { self.prefetching.remove(&id); }
        }
        // The streams this workspace followed, exactly as they were bound: the daemon keeps
        // them per socket. On a socket that still has them this only asks for the state
        // around the transcript again (running or not, the family, questions, permissions).
        let mut bound: Vec<String> = self.stream_owner.values().chain(self.stream_extra.iter())
            .filter(|id| self.sessions.contains_key(*id)).cloned().collect();
        bound.sort();
        bound.dedup();
        for id in bound { self.send_agent(AgentClientMessage::ResumeStream { session_id: id }); }
        let open = self.open_conversations();
        for id in &open {
            let root = self.families.root_of(id).to_owned();
            if !self.stream_owner.contains_key(&root) && !self.stream_extra.contains(id) { self.follow_family(id); }
        }
        // What is on screen, and the conversation a sub-agent on screen belongs to (one step
        // back in its header).
        let mut wanted = open.clone();
        for id in &open {
            let root = self.families.root_of(id);
            if !wanted.iter().any(|w| w == root) && self.sessions.get(root).is_some_and(|s| s.loaded) { wanted.push(root.to_owned()); }
        }
        for id in &wanted { self.refresh_transcript(id); }
        // The rest of what is loaded (sub-agents fetched ahead of a click) is fetched again a
        // few at a time, or at once when one is opened.
        for (id, session) in self.sessions.iter_mut() {
            if session.loaded && !wanted.contains(id) { session.resync = true; }
        }
        self.preload_family_history();
        // Which conversations are at work now. A new socket has just asked (`agent_connected`).
        if !reconnected { self.refresh_threads(); }
    }

    /// The daemon's stream from the agent for `session_id` broke and was opened again: what
    /// its family said in between is in its history only.
    pub(crate) fn agent_stream_resumed(&mut self, session_id: &str) {
        self.composer.prompts.reconnect();
        let root = self.families.root_of(session_id).to_owned();
        let open = self.open_conversations();
        let family: Vec<String> = self.sessions.keys().filter(|id| self.families.root_of(id) == root).cloned().collect();
        for id in family {
            if open.contains(&id) { self.refresh_transcript(&id); }
            else if let Some(session) = self.sessions.get_mut(&id).filter(|s| s.loaded) { session.resync = true; }
        }
        self.preload_family_history();
    }

    /// Fetch `session_id`'s newest page over what is loaded.
    pub(super) fn refresh_transcript(&mut self, session_id: &str) {
        let session = self.sessions.entry(session_id.to_owned()).or_default();
        if session.older_limit == 0 { session.older_limit = 24; }
        session.resync = true;
        // An older page is on its way and its answer looks the same as this one's would: ask
        // once it has landed (`HistoryChunk`).
        if session.loading_older { return; }
        self.prefetching.insert(session_id.to_owned());
        self.send_agent(AgentClientMessage::GetHistory { session_id: session_id.to_owned(), cursor: None, limit: Some(HEAD_PAGE) });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_protocol::agent::{AgentServerMessage as M, HistoryMessage};
    use serde_json::json;

    fn sent(app: &App, kind: &str, session: &str) -> usize {
        app.agent_sent.borrow().iter().filter(|m| m.contains(kind) && m.contains(&format!("\"{session}\""))).count()
    }

    fn row(id: &str, kind: &str, role: &str, text: &str) -> HistoryMessage {
        serde_json::from_value(json!({"id": id, "role": role, "kind": kind, "text": text})).unwrap()
    }

    fn page(session: &str, messages: Vec<HistoryMessage>, next_cursor: Option<&str>) -> M {
        M::HistoryChunk { session_id: session.into(), messages, next_cursor: next_cursor.map(str::to_owned) }
    }

    fn ids(app: &App, session: &str) -> Vec<String> { app.sessions[session].messages.iter().map(|m| m.id.clone()).collect() }

    fn family(app: &mut App) {
        let branch = |id: &str| json!({"sessionId": id, "parentSessionId": "root", "status": "outstanding", "title": id, "agent": "explore"});
        app.apply_agent(M::CatalogFamily { session_id: "root".into(), snapshot: json!({"rootSessionId": "root", "familyRevision": 1, "branches": [branch("a"), branch("b")]}) });
    }

    /// The daemon keeps streams per socket: a new socket follows nothing until asked, and
    /// the reply that was streaming carried on without this window.
    #[test]
    fn a_new_socket_follows_each_open_conversation_again_and_takes_its_newest_page() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("p", "User", "User", "go"), row("r", "Assistant", "Assistant", "Hello wor")], None));
        app.agent_sent.borrow_mut().clear();
        app.agent_resync(true);
        assert_eq!(sent(&app, "ResumeStream", "root"), 1);
        assert_eq!(sent(&app, "SwitchThread", "root"), 0, "following it again changes none of its settings");
        assert_eq!(sent(&app, "GetHistory", "root"), 1);
        // The agent's live tail opens with the reply as it stands, then carries on.
        app.apply_agent(M::MessageUpdated { session_id: "root".into(), message: row("r", "Assistant", "Assistant", "Hello world, this is"), parent_message_id: None });
        app.apply_agent(M::ContentDelta { session_id: "root".into(), message_id: "r".into(), parent_message_id: None, kind: neoism_protocol::agent::ContentKind::Text, text: " live".into() });
        // The history page was read a moment earlier than that: it never takes text back.
        app.apply_agent(page("root", vec![row("p", "User", "User", "go"), row("r", "Assistant", "Assistant", "Hello world")], None));
        assert_eq!(app.sessions["root"].messages[1].text, "Hello world, this is live");
        assert_eq!(ids(&app, "root"), ["p", "r"], "nothing is shown twice");
    }

    #[test]
    fn a_sub_agent_being_viewed_comes_back_on_its_conversations_stream() {
        let mut app = App::new();
        app.open_conversation("root");
        family(&mut app);
        app.apply_agent(page("root", vec![row("p", "User", "User", "go")], None));
        app.apply_agent(page("a", vec![row("a1", "Assistant", "Assistant", "child a")], None));
        app.apply_agent(page("b", vec![row("b1", "Assistant", "Assistant", "child b")], None));
        app.open_child("root", "a");
        app.agent_sent.borrow_mut().clear();
        app.agent_resync(true);
        // One stream carries the family: the conversation's, as before.
        assert_eq!((sent(&app, "ResumeStream", "root"), sent(&app, "ResumeStream", "a"), sent(&app, "ResumeStream", "b")), (1, 0, 0));
        // The sub-agent on screen is brought up to date at once; the others ahead of a click.
        assert_eq!(sent(&app, "GetHistory", "a"), 1);
        assert_eq!(sent(&app, "GetHistory", "root"), 1, "and the conversation it belongs to");
        assert!(app.sessions["b"].resync);
        assert_eq!(sent(&app, "GetHistory", "b"), 1, "fetched ahead, a few at a time");
        app.apply_agent(page("a", vec![row("a1", "Assistant", "Assistant", "child a"), row("a2", "Assistant", "Assistant", "and what it did meanwhile")], None));
        assert_eq!(ids(&app, "a"), ["a1", "a2"]);
        assert!(!app.sessions["a"].resync);
    }

    /// Hours away from an agent on another machine: more happened than one page holds.
    #[test]
    fn a_page_that_does_not_reach_what_was_loaded_restarts_the_transcript_at_the_live_end() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("old-prompt", "User", "User", "go"), row("old-reply", "Assistant", "Assistant", "done")], Some("older")));
        app.sessions.get_mut("root").unwrap().kept_scroll = Some(40.0);
        app.agent_resync(true);
        app.apply_agent(page("root", vec![row("new-prompt", "User", "User", "again"), row("new-reply", "Assistant", "Assistant", "still going")], Some("between")));
        // No hole between the two: the transcript is the newest page, and the rest (what was
        // loaded before included) pages in above it as it is scrolled to.
        assert_eq!(ids(&app, "root"), ["new-prompt", "new-reply"]);
        let session = &app.sessions["root"];
        assert_eq!(session.older_cursor.as_deref(), Some("between"));
        assert_eq!(session.kept_scroll, None, "and it shows its end");
        assert!(session.restoring > 0);
        // An ordinary refresh that shares nothing with a transcript (a turn's end in a new
        // chat) still merges.
        app.apply_agent(page("root", vec![row("later", "Assistant", "Assistant", "more")], None));
        assert_eq!(ids(&app, "root"), ["new-prompt", "new-reply", "later"]);
    }

    #[test]
    fn a_prompt_not_yet_answered_survives_the_restart() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("old", "Assistant", "Assistant", "done")], None));
        app.submit_prompt("root", "typed while offline".into());
        app.agent_resync(true);
        app.apply_agent(page("root", vec![row("new", "Assistant", "Assistant", "meanwhile")], None));
        let rows = ids(&app, "root");
        assert_eq!(rows[0], "new");
        assert!(rows[1].starts_with("msg_rune_"), "{rows:?}");
    }

    #[test]
    fn an_unconsumed_queued_prompt_survives_restart_without_becoming_a_transcript_bubble() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("old", "Assistant", "Assistant", "done")], None));
        app.sessions.get_mut("root").unwrap().streaming = true;
        app.submit_prompt("root", "typed while offline".into());
        let id = app.sessions["root"].pending_prompts.keys().next().unwrap().clone();
        app.agent_resync(true);
        app.apply_agent(page("root", vec![row("new", "Assistant", "Assistant", "meanwhile")], None));
        assert_eq!(ids(&app, "root"), ["new"]);
        app.apply_agent(M::SessionEvent {
            session_id: "root".into(), kind: "session.queue.updated".into(),
            properties: serde_json::json!({"action":"dequeue", "messageID":id, "queue":{"count":0}}),
        });
        assert_eq!(ids(&app, "root"), ["new".to_owned(), id]);
    }

    #[test]
    fn an_older_page_lost_with_the_socket_can_be_asked_for_again() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("p", "User", "User", "go")], Some("older")));
        app.sessions.get_mut("root").unwrap().loading_older = true;
        app.agent_resync(true);
        assert!(!app.sessions["root"].loading_older);
        // A socket that survived keeps its requests.
        app.sessions.get_mut("root").unwrap().loading_older = true;
        app.agent_resync(false);
        assert!(app.sessions["root"].loading_older);
    }

    #[test]
    fn the_daemons_own_stream_coming_back_refreshes_that_family_only() {
        let mut app = App::new();
        app.open_conversation("root");
        app.apply_agent(page("root", vec![row("p", "User", "User", "go")], None));
        app.open_tab(TabKind::Agent { session: Some("other".into()) });
        app.apply_agent(page("other", vec![row("o", "User", "User", "hi")], None));
        app.agent_sent.borrow_mut().clear();
        app.apply_agent(M::SessionEvent { session_id: "root".into(), kind: "stream.resumed".into(), properties: json!({}) });
        assert_eq!((sent(&app, "GetHistory", "root"), sent(&app, "GetHistory", "other")), (1, 0));
        assert_eq!(sent(&app, "ResumeStream", "root"), 0, "the daemon already follows it");
    }
}
