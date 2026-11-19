use neochat::{board::now, client::Client, protocol::*, Board};

const TOKEN: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const ADMIN: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn invite(board: &Board, actor: &str, channels: &[&str]) -> Invite {
    board.create_invite(ADMIN, InviteRequest {
        actor: actor.into(), channels: channels.iter().map(|c| c.to_string()).collect(), expires_at: None,
    }).unwrap()
}
fn actors(board: &Board, token: &str, channel: &str) -> Vec<String> {
    board.members(token, channel).unwrap().into_iter().map(|m| m.actor).collect()
}

#[test]
fn directory_uses_active_credentials_and_literal_channel_scopes_without_posts() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.db");
    let board = Board::open(&path, TOKEN, ADMIN).unwrap();
    let exact = invite(&board, "exact", &["a_b/tasks"]);
    let scoped = invite(&board, "scoped", &["a_b/*"]);
    invite(&board, "root-only", &["a_b"]);
    invite(&board, "unrestricted", &[]);
    for (actor, channel) in [("other", "private/*"), ("prefix", "a_b2/*"), ("wildcard", "axb/*"), ("case", "A_b/*")] {
        invite(&board, actor, &[channel]);
    }
    let revoked = invite(&board, "revoked", &["a_b/*"]);
    board.revoke_invite(ADMIN, revoked.id).unwrap();
    let expired = invite(&board, "expired", &["a_b/*"]);
    // Deterministic expiry at the inclusive boundary; no clock-dependent sleep.
    rusqlite::Connection::open(&path).unwrap().execute("UPDATE credentials SET expires=?1 WHERE id=?2", rusqlite::params![now(), expired.id]).unwrap();

    assert!(board.snapshot(TOKEN).unwrap().messages.is_empty());
    assert_eq!(actors(&board, &exact.token, "a_b/tasks"), ["exact", "local", "scoped", "unrestricted"]);
    assert_eq!(actors(&board, &scoped.token, "a_b"), ["local", "root-only", "scoped", "unrestricted"]);
    assert_eq!(actors(&board, &scoped.token, "a_b/future/deep"), ["local", "scoped", "unrestricted"]);
    for channel in ["private/tasks", "a_b2/tasks", "axb/tasks", "A_b/tasks"] {
        assert_eq!(board.members(&scoped.token, channel).unwrap_err().0, 403);
    }
    assert_eq!(board.members(&exact.token, "a_b/tasks/child").unwrap_err().0, 403);
    for token in [ADMIN, "invalid", &expired.token, &revoked.token] {
        assert_eq!(board.members(token, "a_b/tasks").unwrap_err().0, 401);
        assert_eq!(board.set_display_name(token, "Not authorized").unwrap_err().0, 401);
    }
    assert_eq!(board.members(TOKEN, "a_b/*").unwrap_err().0, 400);
}

#[test]
fn display_names_persist_reset_and_do_not_change_identity_or_authorization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("board.db");
    let board = Board::open(&path, TOKEN, ADMIN).unwrap();
    let alice = invite(&board, "alice", &["general"]);
    let bob = invite(&board, "bob", &["private"]);
    board.set_display_name(&alice.token, "  Zoë 王 Smith  ").unwrap();
    board.set_display_name(&bob.token, "Hidden Name").unwrap();
    board.set_display_name(TOKEN, "Host Person").unwrap();
    let expected = vec![Member { actor: "alice".into(), display_name: Some("Zoë 王 Smith".into()) }, Member { actor: "local".into(), display_name: Some("Host Person".into()) }];
    assert_eq!(board.members(&alice.token, "general").unwrap(), expected);
    for name in ["bad\nname", "bad\0name", "bad\u{202e}name", &"é".repeat(129)] {
        assert_eq!(board.set_display_name(&alice.token, name).unwrap_err().0, 400);
        assert_eq!(board.members(&alice.token, "general").unwrap(), expected);
    }
    // Duplicate names and even another actor's slug are presentation only.
    board.set_display_name(&alice.token, "local").unwrap();
    assert_eq!(board.members(&alice.token, "private").unwrap_err().0, 403);
    let id = board.action(&alice.token, Some("agent"), None, Action::Post {
        channel: "general".into(), text: "hello".into(), labels: Default::default(), task: false, needs: vec![], assignee: None,
    }).unwrap().id.unwrap();
    let snapshot = board.snapshot(TOKEN).unwrap();
    assert_eq!(snapshot.messages.iter().find(|m| m.id == id).unwrap().author, "alice/agent");
    drop(board);
    let board = Board::open(&path, TOKEN, ADMIN).unwrap();
    assert_eq!(board.members(&alice.token, "general").unwrap()[0].display_name.as_deref(), Some("local"));
    board.set_display_name(&alice.token, "").unwrap();
    assert_eq!(board.members(&alice.token, "general").unwrap()[0].display_name, None);
    board.set_display_name(&alice.token, "New Name").unwrap();
    board.set_display_name(&alice.token, "   ").unwrap();
    assert_eq!(board.members(&alice.token, "general").unwrap()[0].display_name, None);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_client_round_trip_and_profile_is_not_a_display_name_target() {
    let board = Board::open(":memory:", TOKEN, ADMIN).unwrap();
    let guest = invite(&board, "guest", &["project/*"]);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move { axum::serve(listener, neochat::board::router(board)).await.unwrap(); });
    tokio::task::spawn_blocking(move || {
        let host = Client::new(&endpoint, TOKEN, None).unwrap();
        let client = Client::new(&endpoint, &guest.token, Some("agent")).unwrap();
        client.set_display_name("Guest Person 🦀").unwrap();
        assert_eq!(host.members("project/future").unwrap(), vec![
            Member { actor: "guest".into(), display_name: Some("Guest Person 🦀".into()) },
            Member { actor: "local".into(), display_name: None },
        ]);
        assert!(client.members("general").unwrap_err().to_string().contains("403"));
        assert!(Client::new(&endpoint, ADMIN, None).unwrap().members("general").unwrap_err().to_string().contains("401"));
        client.set_display_name("").unwrap();
        assert_eq!(host.members("project/future").unwrap()[0].display_name, None);
        assert!(client.set_display_name("bad\nname").is_err());
    }).await.unwrap();
    server.abort();
}