use super::*;
use std::time::Duration;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};

fn database() -> PathBuf {
    std::env::temp_dir().join(format!(
        "event-writer-lifecycle-{}.db",
        Id::ascending(IdKind::Event)
    ))
}

fn session() -> SessionInfo {
    serde_json::from_value(json!({
        "id": "session-lifecycle", "slug": "lifecycle", "projectId": "global",
        "directory": "/tmp", "title": "lifecycle", "version": "test",
        "time": {"created": 1, "updated": 1}
    })).unwrap()
}

#[tokio::test]
async fn dropping_state_releases_writer_and_state() {
    let state = AppState::open_database(database()).await.unwrap();
    let weak = Arc::downgrade(&state.inner);
    let writer = state.inner.event_writer_handle.lock().await.as_ref().unwrap().abort_handle();
    // Let the writer reach recv(), which formerly retained the state forever.
    tokio::task::yield_now().await;
    drop(state);
    tokio::time::timeout(Duration::from_secs(5), async {
        while weak.upgrade().is_some() || !writer.is_finished() {
            tokio::task::yield_now().await;
        }
    }).await.expect("dropped state must abort its writer without a self-cycle");
}

#[tokio::test]
async fn shutdown_drains_retires_and_reopens_without_sequence_conflicts() {
    let path = database();
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut receiver = state.subscribe();
    let run = SessionRun {
        id: "local-run".into(),
        started_at: 1,
        cancel: Arc::new(AtomicBool::new(false)),
    };
    state.inner.session_coordinator.try_start_run("local-session", run.clone()).await.unwrap();
    // Force a real backlog: shutdown must wait, not merely detach the writer.
    let blocked = state.inner.store.lock_writer_for_test().await;
    for n in 0..16 {
        state.publish(EventPayload::new("fixture.queued", json!({"n": n})));
    }
    let shutdown_state = state.clone();
    let shutdown = tokio::spawn(async move { shutdown_state.shutdown().await });
    tokio::time::timeout(Duration::from_secs(5), async {
        while !state.inner.retired.load(Ordering::SeqCst) {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let second_state = state.clone();
    let second = tokio::spawn(async move { second_state.shutdown().await });
    assert!(!shutdown.is_finished());
    assert!(!second.is_finished());
    state.publish(EventPayload::new("fixture.rejected", json!({})));
    assert!(state.publish_persisted(EventPayload::new("fixture.rejected", json!({}))).await.is_err());
    assert!(state.update_session_with_event(&session(), EventPayload::new("fixture.rejected", json!({}))).await.is_err());
    drop(blocked);
    tokio::time::timeout(Duration::from_secs(10), async {
        shutdown.await.unwrap().unwrap();
        second.await.unwrap().unwrap();
    }).await.unwrap();
    assert!(run.cancel.load(Ordering::SeqCst), "local runs must be cancelled too");
    assert!(state.inner.event_writer_handle.lock().await.is_none());
    for _ in 0..16 {
        assert_eq!(receiver.recv().await.unwrap().kind, "fixture.queued");
    }
    assert!(matches!(receiver.recv().await, Err(RecvError::Closed)));
    assert!(matches!(state.subscribe().try_recv(), Err(TryRecvError::Closed)));
    let before = state.inner.store.latest_event_sequence().await.unwrap();
    assert_eq!(before, 16);
    let reopened = AppState::open_database(path).await.unwrap();
    for _ in 0..16 {
        state.publish(EventPayload::new("fixture.stale", json!({})));
        reopened.publish(EventPayload::new("fixture.new", json!({})));
    }
    reopened.shutdown().await.unwrap();
    let rows = reopened.inner.store.list_events_after(TenantQueryScope::LocalAll, 0, 128, None).await.unwrap();
    assert_eq!(rows.len(), 32, "every accepted queued event must survive reopen");
    assert_eq!(reopened.inner.store.latest_event_sequence().await.unwrap(), before + 16);
    state.shutdown().await.unwrap();
}

#[tokio::test]
async fn shutdown_waits_for_admitted_event_commits() {
    let state = AppState::open_database(database()).await.unwrap();
    let info = session();
    state.inner.store.insert_session(&info).await.unwrap();
    let blocked = state.inner.store.lock_writer_for_test().await;
    let starting_sequence = state.inner.event_sequence.load(Ordering::Relaxed);
    let committing_state = state.clone();
    let commit = tokio::spawn(async move {
        committing_state.update_session_with_event(
            &info, EventPayload::new("fixture.transaction", json!({}))
        ).await
    });
    tokio::time::timeout(Duration::from_secs(5), async {
        while state.inner.event_sequence.load(Ordering::Relaxed) == starting_sequence {
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    let other = state.clone();
    let shutdown = tokio::spawn(async move { other.shutdown().await });
    while !state.inner.retired.load(Ordering::SeqCst) {
        tokio::task::yield_now().await;
    }
    assert!(!shutdown.is_finished());
    assert!(state.event_commit_guard().await.is_err());
    assert!(!commit.is_finished());
    drop(blocked);
    commit.await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(10), shutdown).await.unwrap().unwrap().unwrap();
    assert_eq!(state.inner.store.latest_event_sequence().await.unwrap(), starting_sequence + 1);
}

#[tokio::test]
async fn event_bus_close_drains_and_wakes_all_receivers() {
    let bus = EventBus::new(16, 1024 * 1024);
    let clone = bus.clone();
    let mut draining = bus.subscribe();
    bus.send(EventPayload::new("fixture.queued", json!({}))).unwrap();
    let mut waiting = bus.subscribe();
    let waiter = tokio::spawn(async move { waiting.recv().await });
    tokio::task::yield_now().await;
    bus.close();
    bus.close();
    assert!(clone.send(EventPayload::new("fixture.rejected", json!({}))).is_err());
    assert_eq!(draining.recv().await.unwrap().kind, "fixture.queued");
    assert!(matches!(draining.recv().await, Err(RecvError::Closed)));
    assert!(matches!(tokio::time::timeout(Duration::from_secs(5), waiter).await.unwrap().unwrap(), Err(RecvError::Closed)));
}

#[tokio::test]
async fn writer_sequence_conflict_is_not_hidden_by_shutdown() {
    let state = AppState::open_database(database()).await.unwrap();
    let mut first = EventPayload::new("fixture.first", json!({}));
    first.sequence = Some(1);
    let mut duplicate = EventPayload::new("fixture.duplicate", json!({}));
    duplicate.sequence = Some(1);
    state.publish(first);
    state.publish(duplicate);
    assert!(state.shutdown().await.is_err());
    // A second caller must observe the failure too, without polling an already
    // completed JoinHandle or mistaking a missing handle for successful drain.
    assert!(state.shutdown().await.is_err());
    let rows = state.inner.store.list_events_after(TenantQueryScope::LocalAll, 0, 128, None).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(state.inner.store.latest_event_sequence().await.unwrap(), 1);
}