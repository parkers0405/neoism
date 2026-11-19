use axum::http::StatusCode;
use neochat::{board::{now, random_token}, protocol::*, Board};
use std::collections::BTreeMap;

struct Fixture { dir: tempfile::TempDir, board: Board, token: String, admin: String }
impl Fixture {
    fn new() -> Self {
        let dir=tempfile::tempdir().unwrap(); let token=random_token(); let admin=random_token();
        let board=Board::open(dir.path().join("board.db"),&token,&admin).unwrap();
        Self { dir,board,token,admin }
    }
    fn act(&self, token: &str, action: Action) -> Outcome { self.board.action(token,None,None,action).unwrap() }
    fn post(&self, channel: &str, needs: Vec<u64>) -> u64 {
        self.act(&self.token,Action::Post { channel:channel.into(),text:"task".into(),labels:BTreeMap::new(),task:true,needs,assignee:None }).id.unwrap()
    }
    fn invite(&self, actor: &str, scopes: &[&str]) -> Invite {
        self.board.create_invite(&self.admin,InviteRequest { actor:actor.into(),channels:scopes.iter().map(|s|s.to_string()).collect(),expires_at:None }).unwrap()
    }
    fn task(&self, id: u64) -> Task {
        self.board.snapshot(&self.token).unwrap().messages.into_iter().find(|m|m.id==id).unwrap().task.unwrap()
    }
    fn denied(&self, token: &str, action: Action, status: StatusCode) {
        let before=self.board.snapshot(&self.token).unwrap();
        assert_eq!(self.board.action(token,None,None,action).unwrap_err().0,status);
        assert_eq!(self.board.snapshot(&self.token).unwrap(),before);
    }
}

#[test]
fn assignment_validates_live_scoped_identity_and_prevents_stealing() {
    let f=Fixture::new(); let id=f.post("project/tasks",vec![]);
    let worker=f.invite("worker",&["project/*"]); let peer=f.invite("peer",&["project/tasks"]);
    let outside=f.invite("outside",&["other/*"]); let revoked=f.invite("revoked",&[]);
    f.board.revoke_invite(&f.admin,revoked.id).unwrap();
    let expired=f.invite("expired",&[]);
    let db=rusqlite::Connection::open(f.dir.path().join("board.db")).unwrap();
    db.execute("UPDATE credentials SET expires=?1 WHERE id=?2",rusqlite::params![now()-1,expired.id]).unwrap();
    for actor in ["unknown","outside","revoked","expired","worker/profile",""] {
        f.denied(&f.token,Action::Assign { id,assignee:Some(actor.into()) },StatusCode::BAD_REQUEST);
        f.denied(&f.token,Action::Post { channel:"project/tasks".into(),text:"bad assignment".into(),labels:BTreeMap::new(),task:true,needs:vec![],assignee:Some(actor.into()) },StatusCode::BAD_REQUEST);
    }
    f.denied(&outside.token,Action::Assign { id,assignee:None },StatusCode::NOT_FOUND);
    f.denied(&peer.token,Action::Assign { id,assignee:Some("peer".into()) },StatusCode::FORBIDDEN);
    f.act(&f.token,Action::Assign { id,assignee:Some("worker".into()) });
    f.denied(&peer.token,Action::Claim { id,lease_secs:60 },StatusCode::FORBIDDEN);
    f.act(&worker.token,Action::Claim { id,lease_secs:60 });
    for action in [Action::Assign { id,assignee:Some("peer".into()) },Action::Block { id,text:"stop".into() }] {
        f.denied(&f.token,action,StatusCode::CONFLICT);
    }
    f.act(&worker.token,Action::Assign { id,assignee:Some("peer".into()) });
    let t=f.task(id); assert_eq!(t.state,TaskState::Ready); assert_eq!(t.owner,None); assert_eq!(t.expires_at,None);
    f.denied(&worker.token,Action::Renew { id,lease_secs:60 },StatusCode::CONFLICT);
    f.act(&peer.token,Action::Claim { id,lease_secs:60 });
    f.act(&peer.token,Action::Assign { id,assignee:None });
    assert_eq!(f.task(id).assignee,None);
    f.board.revoke_invite(&f.admin,worker.id).unwrap();
    f.denied(&worker.token,Action::Assign { id,assignee:None },StatusCode::UNAUTHORIZED);
}

#[test]
fn manual_blocks_survive_restart_and_recompute_descendants_without_churn() {
    let f=Fixture::new(); let root=f.post("general",vec![]); let child=f.post("general",vec![root]); let leaf=f.post("general",vec![child]);
    let action=Action::Block { id:root,text:"waiting for approval".into() };
    let first=f.board.action(&f.token,Some("planner"),Some("block-once"),action.clone()).unwrap();
    assert_eq!(first,f.board.action(&f.token,Some("planner"),Some("block-once"),action).unwrap());
    for id in [root,child,leaf] { assert_eq!(f.task(id).state,TaskState::Blocked); }
    let before=f.board.snapshot(&f.token).unwrap();
    assert_eq!(before.messages.iter().filter(|m|m.parent==Some(root)).count(),1);
    assert!(before.messages.iter().any(|m|m.parent==Some(root) && m.text=="waiting for approval" && m.author=="local/planner"));
    let reopened=Board::open(f.dir.path().join("board.db"),&f.token,&f.admin).unwrap();
    for _ in 0..3 { reopened.tick().unwrap(); assert_eq!(reopened.snapshot(&f.token).unwrap(),before); }
    reopened.action(&f.token,None,None,Action::Unblock { id:root }).unwrap();
    assert_eq!(f.task(root).state,TaskState::Ready);
    for id in [child,leaf] { assert_eq!(f.task(id).state,TaskState::Waiting); }
    let stable=f.board.snapshot(&f.token).unwrap(); f.board.tick().unwrap(); assert_eq!(f.board.snapshot(&f.token).unwrap(),stable);
    f.act(&f.token,Action::Claim { id:root,lease_secs:60 }); f.act(&f.token,Action::Complete { id:root,text:String::new() });
    assert_eq!(f.task(child).state,TaskState::Ready);
}

#[test]
fn clearing_manual_block_does_not_override_dependencies_or_other_manual_blocks() {
    let f=Fixture::new(); let root=f.post("general",vec![]); let child=f.post("general",vec![root]); let leaf=f.post("general",vec![child]);
    f.act(&f.token,Action::Block { id:root,text:"root reason".into() });
    f.act(&f.token,Action::Block { id:child,text:"child reason".into() });
    f.act(&f.token,Action::Unblock { id:root });
    assert_eq!(f.task(child).state,TaskState::Blocked); assert_eq!(f.task(leaf).state,TaskState::Blocked);
    f.act(&f.token,Action::Claim { id:root,lease_secs:60 }); f.act(&f.token,Action::Fail { id:root,text:"failed".into() });
    f.act(&f.token,Action::Unblock { id:child });
    assert_eq!(f.task(child).state,TaskState::Blocked); assert_eq!(f.task(leaf).state,TaskState::Blocked);
    f.denied(&f.token,Action::Unblock { id:child },StatusCode::CONFLICT);
}

#[test]
fn block_authority_lease_release_terminal_and_validation() {
    let f=Fixture::new(); let worker=f.invite("worker",&["general"]); let peer=f.invite("peer",&["general"]); let hidden=f.invite("hidden",&["private"]);
    let id=f.post("general",vec![]);
    f.denied(&peer.token,Action::Block { id,text:"no".into() },StatusCode::FORBIDDEN);
    f.act(&worker.token,Action::Claim { id,lease_secs:60 });
    f.act(&worker.token,Action::Block { id,text:"need help".into() });
    let t=f.task(id); assert_eq!(t.state,TaskState::Blocked); assert_eq!(t.owner,None); assert_eq!(t.expires_at,None);
    f.denied(&peer.token,Action::Unblock { id },StatusCode::FORBIDDEN);
    f.denied(&hidden.token,Action::Unblock { id },StatusCode::NOT_FOUND);
    f.act(&worker.token,Action::Unblock { id });
    f.denied(&f.token,Action::Block { id,text:" ".into() },StatusCode::BAD_REQUEST);
    f.act(&worker.token,Action::Claim { id,lease_secs:60 });
    f.act(&worker.token,Action::Block { id,text:"again".into() });
    f.act(&f.token,Action::Unblock { id });
    // Deterministic expiry: maintenance must remove lease authority before assignment.
    f.act(&worker.token,Action::Claim { id,lease_secs:60 });
    let db=rusqlite::Connection::open(f.dir.path().join("board.db")).unwrap();
    db.execute("UPDATE messages SET body=json_set(body,'$.task.expires_at',0) WHERE id=?1",[id]).unwrap();
    f.denied(&worker.token,Action::Assign { id,assignee:None },StatusCode::FORBIDDEN);
    f.act(&f.token,Action::Assign { id,assignee:Some("peer".into()) });
    f.act(&peer.token,Action::Claim { id,lease_secs:60 }); f.act(&peer.token,Action::Complete { id,text:String::new() });
    let failed=f.post("general",vec![]); f.act(&f.token,Action::Claim { id:failed,lease_secs:60 }); f.act(&f.token,Action::Fail { id:failed,text:String::new() });
    let canceled=f.post("general",vec![]); f.act(&f.token,Action::Cancel { id:canceled });
    for id in [id,failed,canceled] {
        for action in [Action::Assign { id,assignee:None },Action::Block { id,text:"no".into() },Action::Unblock { id }] { f.denied(&f.token,action,StatusCode::CONFLICT); }
    }
    let plain=f.act(&f.token,Action::Post { channel:"general".into(),text:"plain".into(),labels:BTreeMap::new(),task:false,needs:vec![],assignee:None }).id.unwrap();
    for action in [Action::Assign { id:plain,assignee:None },Action::Block { id:plain,text:"no".into() },Action::Unblock { id:plain }] { f.denied(&f.token,action,StatusCode::BAD_REQUEST); }
}

#[test]
fn assignment_replay_idempotency_and_profile_cannot_impersonate_creator() {
    let f=Fixture::new(); let worker=f.invite("worker",&["general"]); let peer=f.invite("peer",&["general"]);
    let id=f.board.action(&worker.token,Some("planner"),None,Action::Post { channel:"general".into(),text:"task".into(),labels:BTreeMap::new(),task:true,needs:vec![],assignee:None }).unwrap().id.unwrap();
    let before=f.board.snapshot(&f.token).unwrap();
    assert_eq!(f.board.action(&peer.token,Some("worker"),None,Action::Assign { id,assignee:Some("peer".into()) }).unwrap_err().0,StatusCode::FORBIDDEN);
    assert_eq!(f.board.snapshot(&f.token).unwrap(),before);
    let action=Action::Assign { id,assignee:Some("peer".into()) };
    let result=f.board.action(&worker.token,Some("executor"),Some("assign-once"),action.clone()).unwrap();
    assert_eq!(result,f.board.action(&worker.token,Some("executor"),Some("assign-once"),action).unwrap());
    let events=f.board.events(&worker.token,before.cursor).unwrap(); assert_eq!(events.len(),1);
    assert_eq!(events[0].actor,"worker");
    let EventKind::Message { message }=&events[0].kind else { panic!("expected message upsert") };
    assert_eq!(message.id,id); assert_eq!(message.task.as_ref().unwrap().assignee.as_deref(),Some("peer"));
    assert_eq!(message.author,"worker/planner");
    f.act(&worker.token,Action::Assign { id,assignee:None });
    assert_eq!(f.task(id).assignee,None);
}

#[test]
fn task_management_wire_roundtrips_without_task_schema_changes() {
    for (json,action) in [
        (r#"{"type":"assign","id":1,"assignee":"worker"}"#,Action::Assign { id:1,assignee:Some("worker".into()) }),
        (r#"{"type":"assign","id":1,"assignee":null}"#,Action::Assign { id:1,assignee:None }),
        (r#"{"type":"block","id":1,"text":"reason"}"#,Action::Block { id:1,text:"reason".into() }),
        (r#"{"type":"unblock","id":1}"#,Action::Unblock { id:1 }),
    ] {
        assert_eq!(serde_json::from_str::<Action>(json).unwrap(),action);
        assert_eq!(serde_json::from_str::<Action>(&serde_json::to_string(&action).unwrap()).unwrap(),action);
    }
    let legacy=r#"{"state":"blocked","needs":[],"assignee":null,"owner":null,"expires_at":null}"#;
    let t: Task=serde_json::from_str(legacy).unwrap();
    assert_eq!(serde_json::to_value(&t).unwrap(),serde_json::from_str::<serde_json::Value>(legacy).unwrap());
}