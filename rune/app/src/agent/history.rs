//! GPUiX's history-edit projection: paint now, mutate serially, rebase on acknowledgement.
use super::*;
use std::collections::VecDeque;

#[derive(Clone, Default)]
struct State {
    messages: Vec<HistoryMessage>,
    redo: Vec<Vec<HistoryMessage>>,
}

impl State {
    fn step(&mut self, redo: bool) {
        if redo {
            if let Some(turn) = self.redo.pop() { self.messages.extend(turn); }
        } else if let Some(at) = self.messages.iter().rposition(|m| m.kind == HistoryMessageKind::User) {
            self.redo.push(self.messages.split_off(at));
        }
    }
}

#[derive(Default)]
pub(super) struct Edits {
    base: State,
    pub pending: VecDeque<bool>,
}

impl Session {
    pub(super) fn reset_history_edits(&mut self) {
        // A lost acknowledgement is ambiguous: never replay a possibly applied undo.
        if !self.history_edits.pending.is_empty() {
            self.restart_at(Vec::new(), None);
            self.loaded = false;
        }
        self.history_edits = Edits::default();
    }

    pub(super) fn clear_history_redo(&mut self) { self.history_edits.base.redo.clear(); }

    fn project_history(&mut self) {
        let mut projected = self.history_edits.base.clone();
        for &redo in &self.history_edits.pending { projected.step(redo); }
        self.restart_at(projected.messages, self.older_cursor.clone());
    }
}

impl App {
    pub(crate) fn edit_agent_history(&mut self, id: &str, redo: bool) {
        let session = self.sessions.entry(id.to_owned()).or_default();
        let send = session.history_edits.pending.is_empty();
        if send { session.history_edits.base.messages = session.messages.clone(); }
        session.history_edits.pending.push_back(redo);
        session.project_history();
        if send { self.send_agent(AgentClientMessage::EditSessionHistory { session_id: id.to_owned(), redo }); }
    }

    pub(super) fn agent_history_edited(&mut self, id: &str, error: Option<String>, messages: Option<Vec<HistoryMessage>>, cursor: Option<String>) {
        let Some(session) = self.sessions.get_mut(id) else { return };
        let Some(redo) = session.history_edits.pending.pop_front() else { return };
        if error.is_none() {
            session.history_edits.base.step(redo);
            if let Some(mut messages) = messages {
                identify_anonymous_rows(&mut messages);
                for message in &messages { session.note_message(message); }
                session.history_edits.base.messages = messages;
                session.older_cursor = cursor;
                session.loaded = true;
            }
        }
        session.project_history();
        let next = session.history_edits.pending.front().copied();
        if let Some(redo) = next { self.send_agent(AgentClientMessage::EditSessionHistory { session_id: id.to_owned(), redo }); }
        if let Some(message) = error {
            self.apply_agent(AgentServerMessage::Notice {
                session_id: id.to_owned(), title: "History edit failed".into(), body: message,
                level: neoism_protocol::agent::NoticeLevel::Warn,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows() -> Vec<HistoryMessage> {
        [("u1", "User"), ("a1", "Assistant"), ("u2", "User"), ("a2", "Assistant")].into_iter()
            .map(|(id, kind)| serde_json::from_value(json!({"id":id,"role":kind,"kind":kind,"text":id})).unwrap()).collect()
    }

    fn ids(app: &App) -> Vec<&str> { app.sessions["chat"].messages.iter().map(|m| m.id.as_str()).collect() }

    fn ack(app: &mut App, error: Option<&str>, messages: Option<Vec<HistoryMessage>>) {
        app.apply_agent(AgentServerMessage::SessionHistoryEdited {
            session_id: "chat".into(), error: error.map(str::to_owned), messages, next_cursor: None,
        });
    }

    #[test]
    fn instant_edits_are_serial_and_ignore_inflight_history() {
        let mut app = App::new();
        app.sessions.entry("chat".into()).or_default().messages = rows();
        app.edit_agent_history("chat", false);
        assert_eq!(ids(&app), ["u1", "a1"]);
        app.edit_agent_history("chat", false);
        assert!(ids(&app).is_empty());
        app.edit_agent_history("chat", true);
        assert_eq!(ids(&app), ["u1", "a1"]);
        assert_eq!(app.agent_sent.borrow().len(), 1);
        app.apply_agent(AgentServerMessage::HistoryChunk { session_id: "chat".into(), messages: rows(), next_cursor: None });
        assert_eq!(ids(&app), ["u1", "a1"]);
        ack(&mut app, None, Some(rows()[..2].to_vec()));
        assert_eq!(ids(&app), ["u1", "a1"]);
        assert_eq!(app.agent_sent.borrow().len(), 2);
        ack(&mut app, None, Some(vec![]));
        assert_eq!(app.agent_sent.borrow().len(), 3);
        ack(&mut app, None, Some(rows()[..2].to_vec()));
        app.edit_agent_history("chat", true);
        assert_eq!(ids(&app), ["u1", "a1", "u2", "a2"]);
    }

    #[test]
    fn rejected_edits_roll_back_and_rebase_the_queue() {
        let mut app = App::new();
        app.sessions.entry("chat".into()).or_default().messages = rows();
        app.edit_agent_history("chat", false);
        app.edit_agent_history("chat", false);
        ack(&mut app, Some("denied"), None);
        assert_eq!(ids(&app), ["u1", "a1"]);
        ack(&mut app, Some("denied"), None);
        assert_eq!(ids(&app), ["u1", "a1", "u2", "a2"]);
    }

    #[test]
    fn reconnect_discards_unacknowledged_edits_without_replaying_them() {
        let mut app = App::new();
        app.sessions.entry("chat".into()).or_default().messages = rows();
        app.edit_agent_history("chat", false);
        app.edit_agent_history("chat", false);
        app.agent_resync(true);
        assert!(app.sessions["chat"].history_edits.pending.is_empty());
        app.apply_agent(AgentServerMessage::HistoryChunk { session_id: "chat".into(), messages: rows()[..2].to_vec(), next_cursor: None });
        assert_eq!(ids(&app), ["u1", "a1"]);
        ack(&mut app, None, Some(rows())); // A late reply from the old connection is inert.
        assert_eq!(ids(&app), ["u1", "a1"]);
    }
}