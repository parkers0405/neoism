use neochat::{board::{now, random_token}, protocol::*, Board};
use std::{collections::BTreeMap, sync::{Arc, Barrier}, time::Duration};

struct Fixture { dir: tempfile::TempDir, board: Board, token: String, admin: String }
impl Fixture {
    fn new() -> Self {
        let dir=tempfile::tempdir().unwrap(); let token=random_token(); let admin=random_token();
        let board=Board::open(dir.path().join("board.sqlite3"),&token,&admin).unwrap();
        Self { dir,board,token,admin }
    }
    fn action(&self,action:Action)->Outcome { self.board.action(&self.token,None,None,action).unwrap() }
    fn post(&self,text:&str,task:bool,needs:Vec<u64>)->u64 {
        self.action(post("general",text,task,needs)).id.unwrap()
    }
    fn invite(&self,actor:&str,channels:Vec<String>)->Invite {
        self.board.create_invite(&self.admin,InviteRequest { actor:actor.into(),channels,expires_at:None }).unwrap()
    }
    fn state(&self,id:u64)->Task {
        self.board.snapshot(&self.token).unwrap().messages.into_iter().find(|m|m.id==id).unwrap().task.unwrap()
    }
}
fn post(channel:&str,text:&str,task:bool,needs:Vec<u64>)->Action {
    Action::Post { channel:channel.into(),text:text.into(),labels:BTreeMap::new(),task,needs,assignee:None }
}

#[test]
fn tagged_reply_is_atomic_scoped_and_keeps_legacy_reply() {
    let f=Fixture::new();
    let root=f.post("root",false,vec![]);
    let root_labels=BTreeMap::from([("root".into(),"unchanged".into())]);
    f.action(Action::SetLabels { id:root,labels:root_labels.clone() });
    let before=f.board.snapshot(&f.token).unwrap();
    let labels=BTreeMap::from([("area".into(),"desktop UI".into())]);
    let action=Action::ReplyWithLabels { thread:root,text:"tagged".into(),labels:labels.clone() };
    assert_eq!(serde_json::from_value::<Action>(serde_json::to_value(&action).unwrap()).unwrap(),action);
    let outcome=f.board.action(&f.token,None,Some("tagged-reply"),action.clone()).unwrap();
    assert_eq!(outcome,f.board.action(&f.token,None,Some("tagged-reply"),action).unwrap());
    let events=f.board.events(&f.token,before.cursor).unwrap();
    assert_eq!(events.len(),1);
    let EventKind::Message { message }=&events[0].kind else {panic!("message event expected")};
    assert_eq!(message.labels,labels); assert_eq!(message.parent,Some(root));
    let tagged=message.id;
    let snapshot=f.board.snapshot(&f.token).unwrap();
    assert_eq!(snapshot.messages[0].labels,root_labels);
    for action in [
        Action::ReplyWithLabels { thread:root,text:"bad labels".into(),labels:BTreeMap::from([("invalid key".into(),"x".into())]) },
        Action::ReplyWithLabels { thread:tagged,text:"nested".into(),labels:labels.clone() },
        Action::ReplyWithLabels { thread:root,text:" ".into(),labels:labels.clone() },
    ] { assert!(f.board.action(&f.token,None,None,action).is_err()); }
    let guest=f.invite("guest",vec!["other".into()]);
    assert!(f.board.action(&guest.token,None,None,Action::ReplyWithLabels { thread:root,text:"hidden".into(),labels }).is_err());
    assert_eq!(f.board.snapshot(&f.token).unwrap(),snapshot);
    let legacy:Action=serde_json::from_str(&format!(r#"{{"type":"reply","thread":{root},"text":"legacy"}}"#)).unwrap();
    let id=f.action(legacy).id.unwrap();
    assert!(f.board.snapshot(&f.token).unwrap().messages.iter().find(|m|m.id==id).unwrap().labels.is_empty());
}

#[test]
fn durable_snapshot_replay_and_idempotency() {
    let f=Fixture::new();
    let empty=f.board.snapshot(&f.token).unwrap(); assert_eq!(empty.cursor,0);
    let action=post("general","hello",false,vec![]);
    let outcome=f.board.action(&f.token,Some("planner"),Some("retry-1"),action.clone()).unwrap();
    assert_eq!(outcome,f.board.action(&f.token,Some("planner"),Some("retry-1"),action).unwrap());
    assert!(f.board.action(&f.token,None,Some("retry-1"),post("general","other",false,vec![])).is_err());
    let id=outcome.id.unwrap();
    f.action(Action::Reply { thread:id,text:"reply".into() });
    let snapshot=f.board.snapshot(&f.token).unwrap();
    assert_eq!(snapshot.messages[0].author,"local/planner");
    assert_eq!(snapshot.messages[1].parent,Some(id));
    let events=f.board.events(&f.token,empty.cursor).unwrap();
    assert!(events.windows(2).all(|w|w[0].cursor<w[1].cursor));
    assert_eq!(events.last().unwrap().cursor,snapshot.cursor);
    assert!(f.board.events(&f.token,snapshot.cursor).unwrap().is_empty());
    drop(f.board);
    let reopened=Board::open(f.dir.path().join("board.sqlite3"),&f.token,&f.admin).unwrap();
    assert_eq!(reopened.snapshot(&f.token).unwrap(),snapshot);
    assert_eq!(reopened.events(&f.token,0).unwrap(),events);
}

#[test]
fn dependency_completion_and_failure_propagate_atomically() {
    let f=Fixture::new(); let first=f.post("first",true,vec![]); let next=f.post("next",true,vec![first]);
    assert_eq!(f.state(first).state,TaskState::Ready); assert_eq!(f.state(next).state,TaskState::Waiting);
    assert!(f.board.action(&f.token,None,None,Action::Claim { id:next,lease_secs:10 }).is_err());
    f.action(Action::Claim { id:first,lease_secs:60 });
    f.action(Action::Complete { id:first,text:"done".into() });
    assert_eq!(f.state(next).state,TaskState::Ready);
    let dependent=f.post("blocked",true,vec![next]); let transitive=f.post("also blocked",true,vec![dependent]);
    f.action(Action::Claim { id:next,lease_secs:60 }); f.action(Action::Fail { id:next,text:"error".into() });
    assert_eq!(f.state(dependent).state,TaskState::Blocked); assert_eq!(f.state(transitive).state,TaskState::Blocked);
    let snapshot=f.board.snapshot(&f.token).unwrap();
    assert!(snapshot.messages.iter().any(|m|m.parent==Some(first) && m.text=="done"));
}

#[test]
fn exactly_one_concurrent_claim_and_owner_only_renew_finish() {
    let f=Fixture::new(); let id=f.post("race",true,vec![]);
    let one=f.invite("one",vec![]); let two=f.invite("two",vec![]);
    let barrier=Arc::new(Barrier::new(3));
    let mut threads=Vec::new();
    for token in [one.token.clone(),two.token.clone()] {
        let board=f.board.clone(); let barrier=barrier.clone();
        threads.push(std::thread::spawn(move||{ barrier.wait(); board.action(&token,None,None,Action::Claim { id,lease_secs:60 }).is_ok() }));
    }
    barrier.wait(); assert_eq!(threads.into_iter().map(|t|usize::from(t.join().unwrap())).sum::<usize>(),1);
    let owner=f.state(id).owner.unwrap();
    let (winner,loser)=if owner=="one" { (&one.token,&two.token) } else { (&two.token,&one.token) };
    assert!(f.board.action(loser,None,None,Action::Renew { id,lease_secs:60 }).is_err());
    assert!(f.board.action(loser,None,None,Action::Complete { id,text:String::new() }).is_err());
    f.board.action(winner,None,None,Action::Renew { id,lease_secs:120 }).unwrap();
    f.board.action(winner,None,None,Action::Complete { id,text:String::new() }).unwrap();
    assert_eq!(f.state(id).state,TaskState::Completed);
}

#[test]
fn lease_expiration_recovers_durably() {
    let f=Fixture::new(); let id=f.post("lease",true,vec![]);
    f.action(Action::Claim { id,lease_secs:1 });
    std::thread::sleep(Duration::from_millis(1100));
    f.board.tick().unwrap(); let task=f.state(id);
    assert_eq!(task.state,TaskState::Ready); assert_eq!(task.owner,None); assert_eq!(task.expires_at,None);
    assert!(f.board.action(&f.token,None,None,Action::Complete { id,text:String::new() }).is_err());
}

#[test]
fn channel_scope_admin_separation_revocation_and_authorization() {
    let f=Fixture::new(); let public=f.post("public",false,vec![]);
    let hidden=f.action(post("private/team","secret",true,vec![])).id.unwrap();
    let guest=f.invite("guest",vec!["general".into()]);
    let snapshot=f.board.snapshot(&guest.token).unwrap();
    assert_eq!(snapshot.channels,vec!["general"]); assert_eq!(snapshot.messages.len(),1);
    let serialized=serde_json::to_string(&f.board.events(&guest.token,0).unwrap()).unwrap(); assert!(!serialized.contains("secret"));
    assert!(f.board.snapshot(&f.admin).is_err());
    assert!(f.board.create_invite(&guest.token,InviteRequest { actor:"evil".into(),channels:vec![],expires_at:None }).is_err());
    assert!(f.board.action(&guest.token,None,None,Action::Edit { id:public,text:"forged".into() }).is_err());
    assert!(f.board.action(&guest.token,None,None,Action::Reply { thread:hidden,text:"leak".into() }).is_err());
    assert!(f.board.action(&guest.token,None,None,post("private/team","injected",false,vec![])).is_err());
    assert!(f.board.action(&guest.token,Some("local/admin"),None,post("general","fake",false,vec![])).is_err());
    f.board.revoke_invite(&f.admin,guest.id).unwrap(); assert!(f.board.snapshot(&guest.token).is_err());
}

#[test]
fn validation_rolls_back_without_partial_events() {
    let f=Fixture::new(); let before=f.board.snapshot(&f.token).unwrap();
    let actions=vec![post("new/channel"," ",false,vec![]),post("../bad","text",false,vec![]),post("general","x",true,vec![999]),Action::Claim { id:1,lease_secs:0 }];
    for action in actions { assert!(f.board.action(&f.token,None,None,action).is_err()); }
    assert_eq!(f.board.snapshot(&f.token).unwrap(),before);
    let msg=f.post("message",false,vec![]);
    assert!(f.board.action(&f.token,None,None,post("general","task",true,vec![msg])).is_err());
    assert!(f.board.events(&f.token,u64::MAX).is_err());
}

#[test]
fn edits_labels_reactions_and_cancel() {
    let f=Fixture::new(); let id=f.post("original",true,vec![]);
    f.action(Action::Edit { id,text:"edited".into() });
    f.action(Action::SetLabels { id,labels:BTreeMap::from([("priority".into(),"high".into())]) });
    let reaction=Action::React { id,reaction:"👍".into(),remove:false };
    let a=f.action(reaction.clone()); let b=f.action(reaction); assert_eq!(a.cursor,b.cursor);
    assert_eq!(f.board.reactions(&f.token).unwrap(),vec![Reaction { id,channel:"general".into(),actor:"local".into(),reaction:"👍".into() }]);
    f.action(Action::React { id,reaction:"👍".into(),remove:true }); f.action(Action::Cancel { id });
    assert!(f.board.reactions(&f.token).unwrap().is_empty());
    let m=f.board.snapshot(&f.token).unwrap().messages.remove(0);
    assert_eq!(m.text,"edited"); assert_eq!(m.labels["priority"],"high"); assert_eq!(m.task.unwrap().state,TaskState::Canceled);
}

#[test]
fn scheduled_posts_survive_restart_and_publish_once() {
    let f=Fixture::new();
    f.action(Action::Schedule { channel:"general".into(),text:"later".into(),publish_at:now()+1,labels:BTreeMap::new() });
    assert_eq!(f.board.schedules(&f.token).unwrap().len(),1);
    assert!(f.board.snapshot(&f.token).unwrap().messages.is_empty());
    drop(f.board);
    std::thread::sleep(Duration::from_millis(1100));
    let reopened=Board::open(f.dir.path().join("board.sqlite3"),&f.token,&f.admin).unwrap();
    reopened.tick().unwrap(); reopened.tick().unwrap();
    let s=reopened.snapshot(&f.token).unwrap(); assert_eq!(s.messages.len(),1); assert_eq!(s.messages[0].text,"later");
    assert!(reopened.schedules(&f.token).unwrap().is_empty());
    assert!(reopened.events(&f.token,0).unwrap().iter().any(|e|matches!(e.kind,EventKind::SchedulePublished { message_id,.. } if message_id==s.messages[0].id)));
}

#[test]
fn cancel_schedules_and_revoke_prevent_publication() {
    let f=Fixture::new(); let invite=f.invite("scheduler",vec![]);
    let schedule=||Action::Schedule { channel:"general".into(),text:"never".into(),publish_at:now()+10,labels:BTreeMap::new() };
    let id=f.action(schedule()).id.unwrap(); f.action(Action::CancelSchedule { id });
    f.board.action(&invite.token,None,None,schedule()).unwrap();
    assert!(f.board.schedules(&f.token).unwrap().is_empty());
    assert_eq!(f.board.schedules(&invite.token).unwrap().len(),1);
    f.board.revoke_invite(&f.admin,invite.id).unwrap();
    let canceled=f.board.events(&f.token,0).unwrap().iter().filter(|e|matches!(e.kind,EventKind::ScheduleCanceled { .. })).count();
    assert_eq!(canceled,2); assert!(f.board.snapshot(&f.token).unwrap().messages.is_empty());
}

#[test]
fn assignment_limits_claims_and_cross_channel_dependencies_rejected() {
    let f=Fixture::new(); let mut action=post("general","assigned",true,vec![]);
    if let Action::Post { assignee,.. }=&mut action { *assignee=Some("worker".into()); }
    let id=f.action(action).id.unwrap();
    assert!(f.board.action(&f.token,None,None,Action::Claim { id,lease_secs:10 }).is_err());
    let worker=f.invite("worker",vec![]); f.board.action(&worker.token,None,None,Action::Claim { id,lease_secs:10 }).unwrap();
    assert!(f.board.action(&f.token,None,None,post("other","cross",true,vec![id])).is_err());
}

#[test]
fn bounded_replay_pages_have_no_gaps_and_expiring_credentials_fail_closed() {
    let f=Fixture::new();
    for index in 0..1003 { f.action(Action::CreateChannel { channel:format!("channel-{index}") }); }
    let first=f.board.events(&f.token,0).unwrap(); assert_eq!(first.len(),1000);
    let next=f.board.events(&f.token,first.last().unwrap().cursor).unwrap(); assert_eq!(next.len(),3);
    assert_eq!(next.last().unwrap().cursor,f.board.snapshot(&f.token).unwrap().cursor);
    let invite=f.board.create_invite(&f.admin,InviteRequest { actor:"temporary".into(),channels:vec!["general".into()],expires_at:Some(now()+1) }).unwrap();
    std::thread::sleep(Duration::from_millis(1100));
    assert!(f.board.snapshot(&invite.token).is_err());
    assert!(f.board.events(&invite.token,0).is_err());
    assert!(f.board.action(&invite.token,None,None,post("general","expired",false,vec![])).is_err());
}

#[tokio::test]
async fn http_contract_auth_and_long_poll() {
    let f=Fixture::new();
    let listener=tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url=format!("http://{}",listener.local_addr().unwrap());
    let app=neochat::router(f.board.clone());
    let server=tokio::spawn(async move { axum::serve(listener,app).await.unwrap(); });
    let client=reqwest::Client::new();
    assert_eq!(client.get(format!("{url}/v1/snapshot")).send().await.unwrap().status(),401);
    assert_eq!(client.get(format!("{url}/v1/snapshot")).bearer_auth(&f.admin).send().await.unwrap().status(),401);
    let json=serde_json::json!({"type":"post","channel":"general","text":"HTTP"});
    let response=client.post(format!("{url}/v1/actions")).bearer_auth(&f.token).header("x-neochat-profile","cli").header("idempotency-key","http-1").json(&json).send().await.unwrap();
    assert_eq!(response.status(),200); let outcome:Outcome=response.json().await.unwrap();
    let response=client.get(format!("{url}/v1/events?after=0&wait_ms=100")).bearer_auth(&f.token).send().await.unwrap();
    let events:Vec<Event>=response.json().await.unwrap(); assert_eq!(events.last().unwrap().cursor,outcome.cursor);
    let poll_client=client.clone(); let poll_url=url.clone(); let token=f.token.clone();
    let poll=tokio::spawn(async move { poll_client.get(format!("{poll_url}/v1/events?after={}&wait_ms=3000",outcome.cursor)).bearer_auth(token).send().await.unwrap().json::<Vec<Event>>().await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    client.post(format!("{url}/v1/actions")).bearer_auth(&f.token).json(&serde_json::json!({"type":"reply","thread":outcome.id.unwrap(),"text":"wake"})).send().await.unwrap().error_for_status().unwrap();
    assert!(!poll.await.unwrap().is_empty());
    let invite:Invite=client.post(format!("{url}/v1/admin/invites")).bearer_auth(&f.admin).json(&InviteRequest { actor:"http-user".into(),channels:vec![],expires_at:None }).send().await.unwrap().error_for_status().unwrap().json().await.unwrap();
    assert_eq!(client.delete(format!("{url}/v1/admin/invites/{}",invite.id)).bearer_auth(&f.admin).send().await.unwrap().status(),204);
    assert_eq!(client.get(format!("{url}/v1/snapshot")).bearer_auth(&invite.token).send().await.unwrap().status(),401);
    assert_eq!(client.post(format!("{url}/v1/actions")).bearer_auth(&f.token).json(&serde_json::json!({"type":"post","channel":"general","text":"bad","command":"oops"})).send().await.unwrap().status(),422);
    server.abort();
}