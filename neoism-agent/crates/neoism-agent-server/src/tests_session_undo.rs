use super::*;

async fn submit_prompt_and_wait(
    app: &axum::Router,
    state: &AppState,
    session_id: &str,
    text: &str,
) {
    let previous_count = state
        .inner
        .store
        .list_messages(session_id)
        .await
        .unwrap()
        .len();
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{session_id}/prompt"),
            Some(json!({ "noReply": true, "parts": [{ "type": "text", "text": text }] })),
        ))
        .await
        .unwrap();
    let status = response.status();
    if status != StatusCode::NO_CONTENT {
        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        panic!(
            "prompt returned {status}: {}",
            String::from_utf8_lossy(&body)
        );
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let count = state
                .inner
                .store
                .list_messages(session_id)
                .await
                .unwrap()
                .len();
            if count >= previous_count + 1
                && !state
                    .inner
                    .session_coordinator
                    .active_run(session_id)
                    .await
                    .is_some()
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("prompt should finish");
}

#[tokio::test]
async fn session_revert_and_unrevert_restore_message_tail() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-revert-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    for text in ["first turn", "second turn"] {
        submit_prompt_and_wait(&app, &state, session.id.as_str(), text).await;
    }

    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 2);
    let second_user_id = message_id_of(&messages.items[1]);

    let reverted: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": second_user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(reverted.extra.contains_key("revert"));
    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 1);

    let restored: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/unrevert", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(!restored.extra.contains_key("revert"));
    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 2);
    assert_eq!(message_id_of(&messages.items[1]), second_user_id);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_undo_redo_step_without_request_body() {
    // Regression: the desktop/daemon clients POST `/undo` and `/redo` with no
    // JSON body, which the old `Json`-extractor handlers rejected with
    // `415 Unsupported Media Type`. The handlers must tolerate an empty body and
    // compute the revert target server-side (opencode `/undo` `/redo`
    // semantics).
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-undo-nobody-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    for text in ["first turn", "second turn"] {
        submit_prompt_and_wait(&app, &state, session.id.as_str(), text).await;
    }

    // `/undo` with NO body (the exact shape that used to 415) must succeed and
    // revert the most recent turn.
    let undo_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/undo", session.id),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(undo_response.status(), StatusCode::OK);
    let reverted: SessionInfo = response_json(undo_response).await;
    assert!(reverted.extra.contains_key("revert"));

    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 1);

    // `/redo` with NO body restores the reverted turn.
    let redo_response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/redo", session.id),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(redo_response.status(), StatusCode::OK);
    let restored: SessionInfo = response_json(redo_response).await;
    assert!(!restored.extra.contains_key("revert"));

    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 2);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_undo_tree_reports_applied_and_reverted_nodes() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-undo-tree-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let user_id = Id::ascending(IdKind::Message);
    let assistant_id = Id::ascending(IdKind::Message);
    append_snapshot_test_messages(&state, &session, &user_id, &assistant_id, json!({}))
        .await;

    let before: SessionUndoTree = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/undo-tree", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(before.nodes.len(), 1);
    assert_eq!(before.nodes[0].status, SessionUndoStatus::Applied);
    assert_eq!(before.cursor.unwrap().message_id, user_id.to_string());

    let _reverted: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let after: SessionUndoTree = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/undo-tree", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(after.nodes.len(), 1);
    assert_eq!(after.nodes[0].status, SessionUndoStatus::Reverted);
    assert_eq!(after.revert.unwrap().message_id, user_id.to_string());
    assert!(after.cursor.is_none());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_revert_can_move_marker_backward_and_forward() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-redo-marker-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let first_user_id = Id::ascending(IdKind::Message);
    let first_assistant_id = Id::ascending(IdKind::Message);
    append_snapshot_test_messages(
        &state,
        &session,
        &first_user_id,
        &first_assistant_id,
        json!({}),
    )
    .await;
    let second_user_id = Id::ascending(IdKind::Message);
    let second_assistant_id = Id::ascending(IdKind::Message);
    append_snapshot_test_messages(
        &state,
        &session,
        &second_user_id,
        &second_assistant_id,
        json!({}),
    )
    .await;

    let _: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": second_user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let _: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": first_user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let after_second_undo: SessionUndoTree = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/undo-tree", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        after_second_undo.revert.unwrap().message_id,
        first_user_id.to_string()
    );
    assert!(after_second_undo.cursor.is_none());

    let _: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": second_user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let after_redo_one: SessionUndoTree = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/undo-tree", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        after_redo_one.cursor.unwrap().message_id,
        first_user_id.to_string()
    );
    assert_eq!(
        after_redo_one.revert.unwrap().message_id,
        second_user_id.to_string()
    );

    let messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.items.len(), 2);
    assert_eq!(message_id_of(&messages.items[0]), first_user_id.to_string());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_revert_and_unrevert_restore_file_snapshots() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-snapshot-revert-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("file.txt"), "before").unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let result = execute_tool_call(
        &state,
        root.to_str().unwrap(),
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: neoism_agent_core::PermissionAction::Allow,
        }],
        "write",
        json!({ "filePath": "file.txt", "content": "after" }),
    )
    .await
    .unwrap();
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt")).unwrap(),
        "after"
    );
    let metadata = result.metadata.unwrap();
    assert_eq!(metadata["snapshots"].as_array().unwrap().len(), 1);

    let user_id = Id::ascending(IdKind::Message);
    let assistant_id = Id::ascending(IdKind::Message);
    append_snapshot_test_messages(&state, &session, &user_id, &assistant_id, metadata)
        .await;

    let _: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/revert", session.id),
                Some(json!({ "messageId": user_id })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt")).unwrap(),
        "before"
    );

    let _: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/unrevert", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt")).unwrap(),
        "after"
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn session_revert_snapshot_conflict_keeps_messages_and_files() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-snapshot-conflict-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("file.txt"), "before").unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let result = execute_tool_call(
        &state,
        root.to_str().unwrap(),
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: neoism_agent_core::PermissionAction::Allow,
        }],
        "write",
        json!({ "filePath": "file.txt", "content": "after" }),
    )
    .await
    .unwrap();
    let user_id = Id::ascending(IdKind::Message);
    let assistant_id = Id::ascending(IdKind::Message);
    append_snapshot_test_messages(
        &state,
        &session,
        &user_id,
        &assistant_id,
        result.metadata.unwrap(),
    )
    .await;
    std::fs::write(root.join("file.txt"), "user edit").unwrap();

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/revert", session.id),
            Some(json!({ "messageId": user_id })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CONFLICT);
    assert_eq!(
        std::fs::read_to_string(root.join("file.txt")).unwrap(),
        "user edit"
    );
    let messages = state
        .inner
        .store
        .list_messages(session.id.as_str())
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}
