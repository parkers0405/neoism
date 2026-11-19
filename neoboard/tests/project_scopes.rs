use neochat::{board::now, protocol::*, Board};
use std::collections::BTreeMap;

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ADMIN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn board() -> Board { Board::open(":memory:", TOKEN, ADMIN).unwrap() }
fn invite(board: &Board, actor: &str, channels: &[&str]) -> Invite {
    board.create_invite(ADMIN, InviteRequest {
        actor: actor.into(), channels: channels.iter().map(|s| s.to_string()).collect(), expires_at: None,
    }).unwrap()
}
fn post(channel: &str, task: bool) -> Action {
    Action::Post { channel: channel.into(), text: "message".into(), labels: BTreeMap::new(), task, needs: vec![], assignee: None }
}
fn act(board: &Board, token: &str, action: Action) -> Outcome {
    board.action(token, None, None, action).unwrap()
}
fn event_channel(event: &Event) -> &str {
    match &event.kind {
        EventKind::Message { message } => &message.channel,
        EventKind::ChannelCreated { channel } | EventKind::Reaction { channel, .. }
        | EventKind::Scheduled { channel, .. } | EventKind::ScheduleCanceled { channel, .. }
        | EventKind::SchedulePublished { channel, .. } => channel,
    }
}

#[test]
fn scopes_include_root_and_future_descendants_but_not_similar_prefixes() {
    let board = board();
    for channel in ["neoboard", "neoboard/tasks", "neoboard/tasks/deep", "neoboard2", "neoboard-extra/tasks", "other/neoboard", "NeoBoard/tasks", "general"] {
        act(&board, TOKEN, post(channel, false));
    }
    let scoped = invite(&board, "scoped", &["neoboard/*"]);
    let exact = invite(&board, "exact", &["neoboard"]);
    let child = invite(&board, "child", &["neoboard/tasks"]);
    let mixed = invite(&board, "mixed", &["neoboard/*", "general"]);
    let unrestricted = invite(&board, "unrestricted", &[]);
    let before = board.snapshot(&scoped.token).unwrap();
    assert_eq!(before.channels, ["neoboard", "neoboard/tasks", "neoboard/tasks/deep"]);
    assert_eq!(before.messages.len(), 3);
    // The invite precedes this channel: scopes must not be expanded at creation.
    act(&board, TOKEN, post("neoboard/future/nested", false));
    let snapshot = board.snapshot(&scoped.token).unwrap();
    assert_eq!(snapshot.messages.len(), 4);
    assert!(snapshot.channels.contains(&"neoboard/future/nested".into()));
    let future = board.events(&scoped.token, before.cursor).unwrap();
    assert_eq!(future.len(), 2);
    assert!(future.iter().all(|e| event_channel(e) == "neoboard/future/nested"));
    assert!(board.events(&scoped.token, 0).unwrap().iter().all(|e| snapshot.channels.iter().any(|c| c == event_channel(e))));
    assert_eq!(board.snapshot(&exact.token).unwrap().channels, ["neoboard"]);
    assert_eq!(board.snapshot(&child.token).unwrap().channels, ["neoboard/tasks"]);
    for token in [&exact.token, &child.token] {
        let events = board.events(token, 0).unwrap();
        assert_eq!(events.len(), 2);
        assert!(events.iter().all(|e| event_channel(e) == if token == &exact.token { "neoboard" } else { "neoboard/tasks" }));
        assert_eq!(board.action(token, None, None, post("neoboard/tasks/new", false)).unwrap_err().0, 403);
    }
    assert_eq!(board.snapshot(&mixed.token).unwrap().messages.len(), 5);
    assert_eq!(board.events(&mixed.token, 0).unwrap().len(), board.events(&scoped.token, 0).unwrap().len() + 1);
    assert_eq!(board.snapshot(&unrestricted.token).unwrap(), board.snapshot(TOKEN).unwrap());
    assert_eq!(board.events(&unrestricted.token, 0).unwrap(), board.events(TOKEN, 0).unwrap());
}

#[test]
fn scope_markers_are_validated_not_general_globs_or_channel_names() {
    let board = board();
    let mut invalid = vec!["*".to_string(), "/*".into(), "./*".into(), "../*".into(), "a/b/*".into(),
        "a//*".into(), "a*/*".into(), "a/**".into(), "a/*/b".into(), "a?/*".into(),
        "a%/*".into(), " a/*".into(), "é/*".into(), "a\\b/*".into()];
    invalid.push(format!("{}/*", "a".repeat(81)));
    for scope in invalid {
        let error = board.create_invite(ADMIN, InviteRequest {
            actor: "invalid".into(), channels: vec![scope.clone()], expires_at: None,
        }).unwrap_err();
        assert_eq!(error.0, 400, "{scope:?}");
    }
    for (index, project) in ["neoboard", "A-1_b.c", &"a".repeat(80)].iter().enumerate() {
        let scope = format!("{project}/*");
        let credential = invite(&board, &format!("valid-{index}"), &[&scope]);
        assert_eq!(credential.channels, [scope.clone()]);
        act(&board, &credential.token, post(project, false));
        act(&board, &credential.token, post(&format!("{project}/child"), false));
        assert_eq!(board.action(TOKEN, None, None, Action::CreateChannel { channel: scope }).unwrap_err().0, 400);
    }
    // SQL must use literal, case-sensitive prefix comparisons, not LIKE wildcards.
    let literal = invite(&board, "literal", &["a_b/*"]);
    for channel in ["a_b", "a_b/child", "axb/child", "a_b2/child", "A_b/child"] {
        act(&board, TOKEN, post(channel, false));
    }
    assert_eq!(board.snapshot(&literal.token).unwrap().channels, ["a_b", "a_b/child"]);
    let events = board.events(&literal.token, 0).unwrap();
    assert_eq!(events.len(), 4);
    assert!(events.iter().all(|e| ["a_b", "a_b/child"].contains(&event_channel(e))));
}

#[test]
fn scoped_mutations_attachments_reactions_and_schedules_share_authorization() {
    let board = board();
    let scoped = invite(&board, "worker", &["neoboard/*"]);
    let other = invite(&board, "other", &["neoboard2/*"]);
    let id = act(&board, &scoped.token, post("neoboard/future/tasks", true)).id.unwrap();
    act(&board, &scoped.token, post("neoboard", false));
    let hidden = act(&board, &other.token, post("neoboard2/tasks", true)).id.unwrap();
    for action in [
        Action::Reply { thread: id, text: "reply".into() },
        Action::Edit { id, text: "edited".into() },
        Action::SetLabels { id, labels: BTreeMap::from([("priority".into(), "high".into())]) },
        Action::React { id, reaction: "+1".into(), remove: false },
        Action::Claim { id, lease_secs: 60 },
        Action::Renew { id, lease_secs: 120 },
        Action::Complete { id, text: "done".into() },
    ] { act(&board, &scoped.token, action); }
    let canceled = act(&board, &scoped.token, post("neoboard/tasks", true)).id.unwrap();
    act(&board, &scoped.token, Action::Cancel { id: canceled });
    let failed = act(&board, &scoped.token, post("neoboard/tasks", true)).id.unwrap();
    act(&board, &scoped.token, Action::Claim { id: failed, lease_secs: 60 });
    act(&board, &scoped.token, Action::Fail { id: failed, text: "failed".into() });
    act(&board, &other.token, Action::React { id: hidden, reaction: "secret".into(), remove: false });
    assert_eq!(board.reactions(&scoped.token).unwrap().len(), 1);
    assert_eq!(board.reactions(&scoped.token).unwrap()[0].id, id);
    let attachment = board.attach(&scoped.token, id, "data", vec![1, 2]).unwrap();
    assert_eq!(board.attachments(&scoped.token, id).unwrap(), [attachment.clone()]);
    assert_eq!(board.download_attachment(&scoped.token, attachment.id).unwrap(), [1, 2]);
    let secret = board.attach(&other.token, hidden, "secret", vec![3]).unwrap();
    assert_eq!(board.attach(&scoped.token, hidden, "x", vec![]).unwrap_err().0, 404);
    assert_eq!(board.attachments(&scoped.token, hidden).unwrap_err().0, 404);
    assert_eq!(board.download_attachment(&scoped.token, secret.id).unwrap_err().0, 404);
    // Every message-ID mutation must hide out-of-scope IDs, including task paths.
    let before = board.snapshot(TOKEN).unwrap();
    for action in [
        Action::Reply { thread: hidden, text: "no".into() },
        Action::Edit { id: hidden, text: "no".into() },
        Action::SetLabels { id: hidden, labels: BTreeMap::new() },
        Action::React { id: hidden, reaction: "+1".into(), remove: false },
        Action::React { id: hidden, reaction: "secret".into(), remove: true },
        Action::Claim { id: hidden, lease_secs: 60 },
        Action::Renew { id: hidden, lease_secs: 60 },
        Action::Complete { id: hidden, text: String::new() },
        Action::Fail { id: hidden, text: String::new() },
        Action::Cancel { id: hidden },
    ] { assert_eq!(board.action(&scoped.token, None, None, action).unwrap_err().0, 404); }
    let mut dependency = post("neoboard/tasks", true);
    if let Action::Post { needs, .. } = &mut dependency { needs.push(hidden); }
    assert_eq!(board.action(&scoped.token, None, None, dependency).unwrap_err().0, 404);
    for channel in ["general", "neoboard2", "neoboard-extra/tasks"] {
        for action in [post(channel, false), Action::CreateChannel { channel: channel.into() },
            Action::Schedule { channel: channel.into(), text: "no".into(), publish_at: now()+3600, labels: BTreeMap::new() }] {
            assert_eq!(board.action(&scoped.token, None, None, action).unwrap_err().0, 403);
        }
    }
    assert_eq!(board.snapshot(TOKEN).unwrap(), before);
    let schedule = |channel: &str| Action::Schedule { channel: channel.into(), text: "later".into(), publish_at: now()+3600, labels: BTreeMap::new() };
    let pending = act(&board, &scoped.token, schedule("neoboard/future/scheduled")).id.unwrap();
    let other_pending = act(&board, &other.token, schedule("neoboard2/scheduled")).id.unwrap();
    assert_eq!(board.schedules(&scoped.token).unwrap().len(), 1);
    assert_eq!(board.schedules(&scoped.token).unwrap()[0].id, pending);
    assert_eq!(board.action(&scoped.token, None, None, Action::CancelSchedule { id: other_pending }).unwrap_err().0, 403);
    act(&board, &scoped.token, Action::CancelSchedule { id: pending });
    // A visible message still retains creator-only editing rules.
    let foreign = act(&board, TOKEN, post("neoboard/tasks", false)).id.unwrap();
    assert_eq!(board.action(&scoped.token, None, None, Action::Edit { id: foreign, text: "no".into() }).unwrap_err().0, 403);
    let events = board.events(&scoped.token, 0).unwrap();
    assert!(events.iter().all(|e| event_channel(e) == "neoboard" || event_channel(e).starts_with("neoboard/")));
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::Scheduled { id, .. } if id == pending)));
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::ScheduleCanceled { id, .. } if id == pending)));
    // Applying scoped message upserts must yield exactly the scoped snapshot.
    let mut replay = BTreeMap::new();
    for event in events {
        if let EventKind::Message { message } = event.kind { replay.insert(message.id, message); }
    }
    assert_eq!(replay.into_values().collect::<Vec<_>>(), board.snapshot(&scoped.token).unwrap().messages);
}

#[test]
fn persisted_scopes_filter_maintenance_publications_and_revocation_events() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.sqlite3");
    let board = Board::open(&path, TOKEN, ADMIN).unwrap();
    let scoped = invite(&board, "scoped", &["neoboard/*"]);
    let other = invite(&board, "other", &["neoboard2/*"]);
    let schedule = |channel: &str| Action::Schedule {
        channel: channel.into(), text: "published".into(), publish_at: now()+3600, labels: BTreeMap::new(),
    };
    let visible = act(&board, &scoped.token, schedule("neoboard/future")).id.unwrap();
    act(&board, &other.token, schedule("neoboard2/future"));
    let before = board.snapshot(&scoped.token).unwrap().cursor;
    drop(board);
    // Deterministic maintenance on reopening, without waiting for wall-clock expiry.
    rusqlite::Connection::open(&path).unwrap().execute("UPDATE schedules SET publish_at=0", []).unwrap();
    let board = Board::open(&path, TOKEN, ADMIN).unwrap();
    let events = board.events(&scoped.token, before).unwrap();
    assert_eq!(events.len(), 2);
    assert!(events.iter().all(|e| event_channel(e) == "neoboard/future"));
    assert!(events.iter().any(|e| matches!(e.kind, EventKind::SchedulePublished { id, .. } if id == visible)));
    assert_eq!(board.snapshot(&scoped.token).unwrap().messages.len(), 1);
    assert!(board.schedules(&scoped.token).unwrap().is_empty());
    // Revocation emits cancellation only in its own project and invalidates all reads/writes.
    act(&board, &scoped.token, schedule("neoboard/canceled"));
    let before = board.snapshot(&other.token).unwrap().cursor;
    board.revoke_invite(ADMIN, scoped.id).unwrap();
    assert!(board.events(&other.token, before).unwrap().is_empty());
    assert_eq!(board.snapshot(&scoped.token).unwrap_err().0, 401);
    assert_eq!(board.events(&scoped.token, 0).unwrap_err().0, 401);
    assert_eq!(board.action(&scoped.token, None, None, post("neoboard/new", false)).unwrap_err().0, 401);
}

#[test]
fn replay_filters_before_limit_and_pages_without_gaps_or_duplicates() {
    let board = board();
    let scoped = invite(&board, "scoped", &["neoboard/*", "general"]);
    // More than a full page of hidden events before the first visible event.
    for index in 0..1005 {
        act(&board, TOKEN, Action::CreateChannel { channel: format!("neoboard2/hidden-{index}") });
    }
    act(&board, &scoped.token, Action::CreateChannel { channel: "neoboard".into() });
    for index in 0..1002 {
        act(&board, &scoped.token, Action::CreateChannel { channel: format!("neoboard/future-{index}") });
        act(&board, TOKEN, Action::CreateChannel { channel: format!("neoboard-extra/hidden-{index}") });
    }
    act(&board, TOKEN, post("general", false));
    let first = board.events(&scoped.token, 0).unwrap();
    assert_eq!(first.len(), 1000);
    let second = board.events(&scoped.token, first.last().unwrap().cursor).unwrap();
    assert_eq!(second.len(), 4);
    let all: Vec<_> = first.into_iter().chain(second).collect();
    assert!(all.windows(2).all(|pair| pair[0].cursor < pair[1].cursor));
    assert!(all.iter().all(|e| event_channel(e) == "neoboard" || event_channel(e) == "general" || event_channel(e).starts_with("neoboard/")));
    let mut expected = Vec::new();
    let mut after = 0;
    loop {
        let page = board.events(TOKEN, after).unwrap();
        if page.is_empty() { break; }
        after = page.last().unwrap().cursor;
        expected.extend(page.into_iter().filter(|e| event_channel(e) == "neoboard" || event_channel(e) == "general" || event_channel(e).starts_with("neoboard/")));
    }
    assert_eq!(all, expected);
    let last = all.last().unwrap().cursor;
    assert!(board.events(&scoped.token, last).unwrap().is_empty());
    act(&board, TOKEN, post("neoboard/new-after-replay", false));
    assert_eq!(board.events(&scoped.token, last).unwrap().len(), 2);
}