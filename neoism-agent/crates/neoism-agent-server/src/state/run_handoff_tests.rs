use super::*;

async fn store(path: &std::path::Path) -> SessionStore {
    SessionStore::open(path.to_path_buf()).await.unwrap()
}

async fn admit(store: &SessionStore, id: &str) {
    let session: SessionInfo = serde_json::from_value(json!({
        "id": "ses_handoff", "slug": "handoff", "projectId": "global",
        "directory": "/tmp", "title": "handoff", "version": "test",
        "time": {"created": 1, "updated": 1}
    })).unwrap();
    store.insert_session(&session).await.unwrap();
    store.db.execute(
        "INSERT INTO prompt_queue (id, session_id, position, request_json, created, kind, delivery) VALUES (?, 'ses_handoff', 0, ?, 1, 'prompt', 'queue')",
        vec![text(id), text(json!({"parts": [{"type": "text", "text": "original input"}], "messageID": "msg_handoff"}).to_string())],
    ).await.unwrap();
}

#[tokio::test]
async fn run_handoff_retains_input_across_reopen_without_requeueing() {
    let path = std::env::temp_dir().join(format!("run-handoff-{}.db", neoism_agent_core::new_session_id()));
    let first = store(&path).await;
    admit(&first, "admission").await;
    let original = first.db.fetch_optional("SELECT request_json FROM prompt_queue WHERE id = 'admission'", vec![]).await.unwrap().unwrap().get_str("request_json").unwrap();
    first.handoff_prompt_admission("admission", "ses_handoff", "run").await.unwrap();
    drop(first);

    let reopened = store(&path).await;
    reopened.interrupt_stale_runs().await.unwrap();
    assert_eq!(reopened.queued_prompt_count("ses_handoff").await.unwrap(), 0);
    let input = reopened.db.fetch_optional("SELECT request_json, delivery FROM session_run_inputs WHERE run_id = 'run'", vec![]).await.unwrap().unwrap();
    assert_eq!(input.get_str("request_json").unwrap(), original);
    assert_eq!(input.get_str("delivery").unwrap(), "queue");
    assert!(reopened.handoff_prompt_admission("admission", "ses_handoff", "run").await.is_err());
    assert_eq!(reopened.session_run_statuses("ses_handoff").await.unwrap(), vec![("run".into(), "interrupted".into())]);
}

#[tokio::test]
async fn run_handoff_failure_keeps_admission_and_rolls_back_run() {
    let path = std::env::temp_dir().join(format!("run-handoff-failure-{}.db", neoism_agent_core::new_session_id()));
    let store = store(&path).await;
    admit(&store, "admission").await;
    store.db.execute("ALTER TABLE session_run_inputs RENAME TO unavailable_inputs", vec![]).await.unwrap();
    assert!(store.handoff_prompt_admission("admission", "ses_handoff", "run").await.is_err());
    assert_eq!(store.queued_prompt_count("ses_handoff").await.unwrap(), 1);
    assert!(store.session_run_statuses("ses_handoff").await.unwrap().is_empty());
    store.db.execute("ALTER TABLE unavailable_inputs RENAME TO session_run_inputs", vec![]).await.unwrap();
    assert!(store.handoff_prompt_admission("admission", "other_session", "wrong_run").await.is_err());
    assert_eq!(store.queued_prompt_count("ses_handoff").await.unwrap(), 1);
    store.handoff_prompt_admission("admission", "ses_handoff", "run").await.unwrap();
    assert_eq!(store.queued_prompt_count("ses_handoff").await.unwrap(), 0);
}