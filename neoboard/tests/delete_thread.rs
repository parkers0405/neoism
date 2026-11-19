use axum::http::StatusCode;
use neochat::{board::random_token, protocol::*, Board};
use rusqlite::Connection;
use std::collections::BTreeMap;

struct Fixture { dir: tempfile::TempDir, board: Board, token: String, admin: String }
impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let token = random_token();
        let admin = random_token();
        let board = Board::open(dir.path().join("board.db"), &token, &admin).unwrap();
        Self { dir, board, token, admin }
    }
    fn db(&self) -> Connection { Connection::open(self.dir.path().join("board.db")).unwrap() }
    fn act(&self, action: Action) -> Outcome { self.board.action(&self.token, None, None, action).unwrap() }
    fn post(&self, task: bool, needs: Vec<u64>) -> u64 {
        self.act(Action::Post { channel: "general".into(), text: "private root".into(), labels: BTreeMap::new(), task, needs, assignee: None }).id.unwrap()
    }
    fn invite(&self, actor: &str, channels: Vec<String>) -> String {
        self.board.create_invite(&self.admin, InviteRequest { actor: actor.into(), channels, expires_at: None }).unwrap().token
    }
}

#[test]
fn removes_whole_thread_and_old_replay_payloads_durably() {
    let f = Fixture::new();
    let root = f.post(true, vec![]);
    let peer = f.invite("peer", vec![]);
    let reply = f.board.action(&peer, None, None, Action::Reply { thread: root, text: "private reply".into() }).unwrap().id.unwrap();
    f.act(Action::Edit { id: root, text: "edited private root".into() });
    let unrelated = f.post(false, vec![]);
    let mut attachments = Vec::new();
    for id in [root, reply] {
        attachments.push(f.board.attach(&peer, id, "data.bin", vec![0, 1, 255]).unwrap().id);
        f.act(Action::React { id, reaction: "yes".into(), remove: false });
    }
    f.act(Action::Block { id: root, text: "private block reason".into() });
    let before = f.board.snapshot(&f.token).unwrap().cursor;
    let outcome = f.board.action(&f.token, Some("different-profile"), Some("delete-once"), Action::DeleteThread { id: root }).unwrap();
    assert!(outcome.cursor > before);
    assert_eq!(outcome.id, Some(root));
    assert_eq!(outcome, f.board.action(&f.token, Some("different-profile"), Some("delete-once"), Action::DeleteThread { id: root }).unwrap());
    for attachment in attachments {
        assert_eq!(f.board.download_attachment(&peer, attachment).unwrap_err().0, StatusCode::NOT_FOUND);
    }
    let db = f.db();
    for table in ["attachments", "reactions", "task_blocks"] {
        assert_eq!(db.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get::<_, u64>(0)).unwrap(), 0);
    }
    let reopened = Board::open(f.dir.path().join("board.db"), &f.token, &f.admin).unwrap();
    assert_eq!(reopened.snapshot(&f.token).unwrap().messages.iter().map(|m| m.id).collect::<Vec<_>>(), vec![unrelated]);
    let events = reopened.events(&peer, 0).unwrap();
    assert!(events.iter().all(|e| match &e.kind {
        EventKind::Message { message } => message.id == unrelated,
        EventKind::Reaction { .. } => false,
        _ => true,
    }));
    let tail = reopened.events(&peer, before).unwrap();
    assert_eq!(tail.len(), 1);
    assert_eq!(tail[0].kind, EventKind::ThreadDeleted { id: root, channel: "general".into() });
    assert_eq!(tail[0].cursor, outcome.cursor);
    assert_eq!(f.board.action(&f.token, None, None, Action::DeleteThread { id: root }).unwrap_err().0, StatusCode::NOT_FOUND);
}

#[test]
fn only_root_creator_can_delete_and_scopes_hide_roots() {
    let f = Fixture::new();
    let root = f.post(false, vec![]);
    let peer = f.invite("peer", vec![]);
    let hidden = f.invite("hidden", vec!["other".into()]);
    let reply = f.act(Action::Reply { thread: root, text: "reply".into() }).id.unwrap();
    for (token, id, status) in [(&peer, root, StatusCode::FORBIDDEN), (&hidden, root, StatusCode::NOT_FOUND), (&f.admin, root, StatusCode::UNAUTHORIZED), (&f.token, reply, StatusCode::BAD_REQUEST), (&f.token, u64::MAX, StatusCode::BAD_REQUEST)] {
        let before = f.board.snapshot(&f.token).unwrap();
        assert_eq!(f.board.action(token, None, None, Action::DeleteThread { id }).unwrap_err().0, status);
        assert_eq!(f.board.snapshot(&f.token).unwrap(), before);
    }
}

#[test]
fn dependent_tasks_even_terminal_prevent_deletion() {
    let f = Fixture::new();
    let root = f.post(true, vec![]);
    let dependent = f.post(true, vec![root]);
    for terminal in [false, true] {
        if terminal { f.act(Action::Cancel { id: dependent }); }
        let before = f.board.snapshot(&f.token).unwrap();
        assert_eq!(f.board.action(&f.token, None, None, Action::DeleteThread { id: root }).unwrap_err().0, StatusCode::CONFLICT);
        assert_eq!(f.board.snapshot(&f.token).unwrap(), before);
    }
    f.act(Action::DeleteThread { id: dependent });
    f.act(Action::DeleteThread { id: root });
    assert!(f.board.snapshot(&f.token).unwrap().messages.is_empty());
}

#[test]
fn failure_to_emit_rolls_back_all_removals_and_replay_pruning() {
    let f = Fixture::new();
    let root = f.post(false, vec![]);
    let reply = f.act(Action::Reply { thread: root, text: "reply".into() }).id.unwrap();
    let attachment = f.board.attach(&f.token, reply, "data", vec![42]).unwrap();
    f.act(Action::React { id: reply, reaction: "yes".into(), remove: false });
    let snapshot = f.board.snapshot(&f.token).unwrap();
    let events = f.board.events(&f.token, 0).unwrap();
    let db = f.db();
    db.execute_batch("CREATE TRIGGER reject_delete_event BEFORE INSERT ON events WHEN json_extract(NEW.body,'$.type')='thread_deleted' BEGIN SELECT RAISE(ABORT,'injected'); END;").unwrap();
    assert_eq!(f.board.action(&f.token, None, Some("retry"), Action::DeleteThread { id: root }).unwrap_err().0, StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(f.board.snapshot(&f.token).unwrap(), snapshot);
    assert_eq!(f.board.events(&f.token, 0).unwrap(), events);
    assert_eq!(f.board.download_attachment(&f.token, attachment.id).unwrap(), vec![42]);
    assert_eq!(f.board.reactions(&f.token).unwrap().len(), 1);
    db.execute_batch("DROP TRIGGER reject_delete_event").unwrap();
    f.board.action(&f.token, None, Some("retry"), Action::DeleteThread { id: root }).unwrap();
}

#[test]
fn wire_tombstone_scope_and_cached_post_retry_do_not_resurrect() {
    let f = Fixture::new();
    let post = Action::Post { channel: "general".into(), text: "cached text".into(), labels: BTreeMap::new(), task: false, needs: vec![], assignee: None };
    let posted = f.board.action(&f.token, Some("creator-profile"), Some("post-once"), post.clone()).unwrap();
    let id = posted.id.unwrap();
    let action: Action = serde_json::from_value(serde_json::json!({"type":"delete_thread","id":id})).unwrap();
    assert_eq!(action, Action::DeleteThread { id });
    let deleted = f.act(action);
    assert_eq!(posted, f.board.action(&f.token, Some("creator-profile"), Some("post-once"), post).unwrap());
    assert!(f.board.snapshot(&f.token).unwrap().messages.is_empty());
    let visible = f.invite("visible", vec!["general".into()]);
    let hidden = f.invite("hidden", vec!["other".into()]);
    assert!(f.board.events(&hidden, posted.cursor).unwrap().is_empty());
    let events = f.board.events(&visible, posted.cursor).unwrap();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].cursor, deleted.cursor);
    let wire = serde_json::to_value(&events[0]).unwrap();
    assert_eq!(wire["type"], "thread_deleted");
    assert_eq!(wire["id"], id);
    assert_eq!(wire["channel"], "general");
    assert_eq!(serde_json::from_value::<Event>(wire).unwrap(), events[0]);
}