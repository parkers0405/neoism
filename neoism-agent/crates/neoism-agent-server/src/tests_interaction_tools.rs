use super::*;

async fn wait_for_session_message_count(
    state: &AppState,
    session_id: &str,
    count: usize,
) -> Vec<MessageWithParts> {
    let mut messages = Vec::new();
    for _ in 0..500 {
        messages = state.inner.store.list_messages(session_id).await.unwrap();
        let running = state.inner.session_coordinator.active_run(session_id).await.is_some();
        if messages.len() >= count && !running {
            return messages;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!(
        "session {session_id} did not reach {count} messages; saw {}",
        messages.len()
    );
}

async fn wait_for_queued_prompt(
    state: &AppState,
    session_id: &str,
) -> Vec<(PromptRequest, String)> {
    for _ in 0..500 {
        let queued = state
            .inner
            .store
            .list_queued_prompt_entries(session_id)
            .await
            .unwrap();
        if !queued.is_empty() {
            return queued;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("session {session_id} did not receive a queued prompt");
}

#[tokio::test]
async fn subtask_command_creates_linked_child_session() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-subtask-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent/agents")).unwrap();
    std::fs::create_dir_all(root.join(".agent/commands")).unwrap();
    std::fs::write(
        root.join(".agent/agents/reviewer.md"),
        r#"---
mode: subagent
---
Review carefully.
"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/commands/review.md"),
        r#"---
agent: reviewer
subtask: true
---
Review $1
"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let parent: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.to_string_lossy()),
                Some(json!({
                    "model": {
                        "providerId": "neoism",
                        "id": "stub",
                        "variant": "xhigh"
                    }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let response: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{}/commands", parent.id),
                Some(json!({ "command": "review", "arguments": "src/lib.rs" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let response_id = match &response.info {
        MessageInfo::Assistant(assistant) => assistant.id.clone(),
        MessageInfo::User(_) => {
            panic!("expected subtask assistant response")
        }
    };
    assert!(
        response
            .parts
            .iter()
            .any(|part| matches!(part, Part::Text(_))),
        "parent agent should continue after subtask completion"
    );
    let parent_messages: Page<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/messages", parent.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let last_parent_id = match &parent_messages.items.last().expect("parent messages").info {
        MessageInfo::Assistant(assistant) => assistant.id.clone(),
        MessageInfo::User(_) => panic!("expected last parent message to be assistant"),
    };
    assert_eq!(last_parent_id, response_id);
    let Some((metadata, output)) = parent_messages.items.iter().find_map(|message| {
        message.parts.iter().find_map(|part| {
            let Part::Tool(tool) = part else {
                return None;
            };
            if tool.tool != "task" {
                return None;
            }
            let ToolState::Completed {
                metadata, output, ..
            } = &tool.state
            else {
                return None;
            };
            Some((metadata, output))
        })
    }) else {
        panic!("expected subtask assistant response")
    };
    assert!(output.contains("status: running"));
    let child_id_from_tool = metadata
        .get("sessionId")
        .and_then(Value::as_str)
        .expect("task metadata should include child session id")
        .to_string();

    let sessions: Page<SessionInfo> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/v2/sessions", None))
            .await
            .unwrap(),
    )
    .await;
    let child = sessions.items
        .iter()
        .find(|session| session.parent_id.as_ref() == Some(&parent.id))
        .expect("child session should be linked to parent");
    assert_eq!(child.id.as_str(), child_id_from_tool.as_str());
    assert_eq!(child.agent.as_deref(), Some("reviewer"));
    let Part::Subtask(subtask) = &parent_messages.items[0].parts[0] else {
        panic!("expected parent user subtask part")
    };
    assert_eq!(subtask.agent, "reviewer");
    assert_eq!(subtask.prompt, "Review src/lib.rs");
    let child_messages =
        wait_for_session_message_count(&state, child.id.as_str(), 2).await;
    assert!(child_messages.len() >= 2);
    let MessageInfo::User(user) = &child_messages[0].info else {
        panic!("expected subtask user prompt")
    };
    assert_eq!(user.agent, "reviewer");
    let Part::Text(text) = &child_messages[0].parts[0] else {
        panic!("expected text prompt")
    };
    assert_eq!(text.text, "Review src/lib.rs");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn todowrite_tool_updates_session_todos_and_event() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-todo-{}",
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
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({
                    "model": {
                        "providerId": "neoism",
                        "id": "stub",
                        "variant": "xhigh"
                    }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    let mut events = state.subscribe();

    let result = execute_tool_call_with_permission_wait(
        &state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Allow,
        }],
        "call-todo",
        "todowrite",
        json!({
            "todos": [
                { "content": "ship runtime", "status": "in_progress", "priority": "high" },
                { "content": "write tests", "status": "pending", "priority": "medium" }
            ]
        }),
    )
    .await
    .unwrap();
    assert_eq!(result.title, "2 todos");
    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, event_type::TODO_UPDATED);
    assert_eq!(event.properties["sessionID"], session.id.to_string());

    let todos: Vec<TodoInfo> = response_json(
        app.oneshot(request(
            Method::GET,
            &format!("/v2/sessions/{}/todos", session.id),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(todos.len(), 2);
    assert_eq!(todos[0].content, "ship runtime");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn workflow_permission_asks_are_denied_without_waiting() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-workflow-permission-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let mut session: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    session
        .extra
        .insert("workflowRunID".to_string(), json!("run-test"));
    state.inner.store.update_session(&session).await.unwrap();

    for permissions in [
        Vec::new(),
        vec![PermissionRule {
            permission: "bash".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        }],
    ] {
        let error = tokio::time::timeout(
            Duration::from_secs(1),
            execute_tool_call_with_permission_wait(
                &state,
                &session.id,
                &Id::ascending(IdKind::Message),
                &session.directory,
                permissions,
                "call-workflow-permission",
                "bash",
                json!({ "command": "printf blocked", "description": "Blocked" }),
            ),
        )
        .await
        .expect("workflow permission should not wait")
        .unwrap_err();
        assert!(error.contains("tool permission bash"));
        assert!(error.contains("is denied"));
    }
    assert!(state.inner.permission_waiters.read().await.is_empty());
    assert!(state.inner.permissions.read().await.is_empty());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn multi_file_apply_patch_uses_one_permission_request_for_every_path() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-multi-patch-permission-{}",
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
                &format!("/v2/sessions?directory={}", root.display()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let expected = (0..6)
        .map(|index| format!("file-{index}.txt"))
        .collect::<Vec<_>>();
    let mut patch_text = "*** Begin Patch\n".to_string();
    for (index, path) in expected.iter().enumerate() {
        patch_text.push_str(&format!("*** Add File: {path}\n+value {index}\n"));
    }
    patch_text.push_str("*** End Patch");

    let tool_state = state.clone();
    let session_id = session.id.clone();
    let message_id = Id::ascending(IdKind::Message);
    let directory = session.directory.clone();
    let handle = tokio::spawn(async move {
        execute_tool_call_with_permission_wait(
            &tool_state,
            &session_id,
            &message_id,
            &directory,
            vec![PermissionRule {
                permission: "edit".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Ask,
            }],
            "call-multi-patch",
            "apply_patch",
            json!({ "patchText": patch_text }),
        )
        .await
    });

    let permission = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            if let Some(request) = state
                .inner
                .permissions
                .read()
                .await
                .values()
                .next()
                .cloned()
            {
                break request;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("multi-file patch should ask for permission");
    assert_eq!(permission.permission, "edit");
    assert_eq!(permission.patterns, expected);

    let allowed: bool = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/interactions/permissions/{}/reply", permission.id),
            Some(json!({ "reply": "once" })),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert!(allowed);
    let result = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("approved patch should finish")
        .unwrap()
        .unwrap();
    assert!(result.output.contains("Applied patch to"));
    for path in &expected {
        assert!(root.join(path).is_file(), "{path} was not created");
    }
    assert!(state.inner.permissions.read().await.is_empty());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn skip_permissions_applies_multi_file_patch_without_a_permission_request() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-multi-patch-skip-permissions-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{"dangerouslySkipPermissions":true}"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    let expected = (0..6)
        .map(|index| format!("file-{index}.txt"))
        .collect::<Vec<_>>();
    let mut patch_text = "*** Begin Patch\n".to_string();
    for (index, path) in expected.iter().enumerate() {
        patch_text.push_str(&format!("*** Add File: {path}\n+value {index}\n"));
    }
    patch_text.push_str("*** End Patch");

    let result = execute_tool_call_with_permission_wait(
        &state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        vec![PermissionRule {
            permission: "edit".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Ask,
        }],
        "call-multi-patch-skip-permissions",
        "apply_patch",
        json!({ "patchText": patch_text }),
    )
    .await
    .unwrap();

    assert!(result.output.contains("Applied patch to"));
    assert!(state.inner.permissions.read().await.is_empty());
    for path in &expected {
        assert!(root.join(path).is_file(), "{path} was not created");
    }

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn skip_permissions_does_not_override_an_explicit_edit_deny() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-plan-edit-deny-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{"dangerouslySkipPermissions":true}"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;

    let result = execute_tool_call_with_permission_wait(
        &state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        vec![PermissionRule {
            permission: "edit".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Deny,
        }],
        "call-plan-edit-deny",
        "apply_patch",
        json!({
            "patchText": "*** Begin Patch\n*** Add File: denied.txt\n+blocked\n*** End Patch"
        }),
    )
    .await
    .unwrap_err();

    assert!(result.contains("tool permission edit for denied.txt is denied"));
    assert!(state.inner.permissions.read().await.is_empty());
    assert!(!root.join("denied.txt").exists());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn question_tool_waits_for_route_reply() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-question-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let mut events = state.subscribe();
    let session_id = Id::ascending(IdKind::Session);
    let message_id = Id::ascending(IdKind::Message);
    let tool_state = state.clone();
    let directory = root.to_string_lossy().to_string();
    let handle = tokio::spawn(async move {
        execute_tool_call_with_permission_wait(
            &tool_state,
            &session_id,
            &message_id,
            &directory,
            vec![PermissionRule {
                permission: "*".to_string(),
                pattern: "*".to_string(),
                action: PermissionAction::Allow,
            }],
            "call-question",
            "question",
            json!({ "questions": [{ "question": "Proceed?" }] }),
        )
        .await
    });

    let event = events.recv().await.unwrap();
    assert_eq!(event.kind, event_type::QUESTION_ASKED);
    let request_id = event.properties["id"].as_str().unwrap().to_string();
    let pending: Vec<QuestionRequestInfo> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/v2/interactions/questions", None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(pending.len(), 1);

    let ok: bool = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/interactions/questions/{request_id}/reply"),
            Some(json!({ "answers": [["yes"]] })),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert!(ok);
    let result = handle.await.unwrap().unwrap();
    assert!(result.output.contains("\"Proceed?\"=\"yes\""));
    assert!(state.inner.questions.read().await.is_empty());
    assert!(state.inner.question_waiters.read().await.is_empty());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn background_task_runs_shell_command_and_result_can_be_collected() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-background-task-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let session: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({
                "model": {
                    "providerId": "neoism",
                    "id": "stub",
                    "variant": "xhigh"
                }
            })),
        ))
        .await
        .unwrap(),
    )
    .await;
    let mut events = state.subscribe();

    let result = execute_tool_call_with_permission_wait(
        &state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Allow,
        }],
        "call-background-task",
        "background_task",
        json!({
            "description": "Echo background result",
            "command": "printf background-ok",
            "timeout": 5000
        }),
    )
    .await
    .unwrap();

    assert_eq!(result.title, "Echo background result");
    assert!(result.output.contains("status: running"));
    let job_id = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("jobId"))
        .and_then(Value::as_str)
        .expect("job id")
        .to_string();

    let completion = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let event = events.recv().await.unwrap();
            if event.kind == event_type::SESSION_BACKGROUND_TASK_COMPLETED
                && event.properties["jobID"] == job_id
            {
                break event;
            }
        }
    })
    .await
    .expect("background task should complete");
    assert_eq!(completion.properties["status"], "completed");
    assert_eq!(completion.properties["result"], "background-ok");

    let collected = execute_tool_call_with_permission_wait(
        &state,
        &session.id,
        &Id::ascending(IdKind::Message),
        &session.directory,
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Allow,
        }],
        "call-background-task-result",
        "background_task_result",
        json!({ "job_id": job_id }),
    )
    .await
    .unwrap();
    assert!(collected.output.contains("status: completed"));
    assert!(collected.output.contains("<background_task_result>"));
    assert!(collected.output.contains("background-ok"));

    for _ in 0..50 {
        if !state.inner.session_coordinator.active_run(session.id.as_str()).await.is_some()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn task_tool_creates_background_child_session_and_result_can_be_collected() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-task-tool-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let mut parent: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({
                "model": {
                    "providerId": "neoism",
                    "id": "stub",
                    "variant": "xhigh"
                }
            })),
        ))
        .await
        .unwrap(),
    )
    .await;
    crate::execution_activity::ensure_for_prompt(
        &state,
        &mut parent,
        "message-parent-background-task",
        true,
    )
    .await
    .unwrap()
    .expect("parent execution");
    state.inner.store.update_session(&parent).await.unwrap();

    let result = execute_tool_call_with_permission_wait(
        &state,
        &parent.id,
        &Id::ascending(IdKind::Message),
        &parent.directory,
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Allow,
        }],
        "call-task",
        "task",
        json!({
            "description": "Inspect runtime",
            "prompt": "Say hello from the subtask",
            "subagent_type": "general"
        }),
    )
    .await
    .unwrap();

    assert_eq!(result.title, "Inspect runtime");
    assert!(result.output.contains("task_id:"));
    assert!(result.output.contains("status: running"));
    assert!(!result.output.contains("<task_result>"));
    let child_id = result
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("sessionId"))
        .and_then(Value::as_str)
        .expect("child session id");
    let child = state
        .inner
        .store
        .get_session(child_id)
        .await
        .unwrap()
        .expect("child session");
    assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
    assert_eq!(child.agent.as_deref(), Some("general"));
    let child_model = child
        .model
        .as_ref()
        .expect("child should inherit parent model");
    assert_eq!(child_model.provider_id, "neoism");
    assert_eq!(child_model.id, "stub");
    assert_eq!(child_model.variant.as_deref(), Some("xhigh"));
    let child_messages = wait_for_session_message_count(&state, child_id, 2).await;
    assert_eq!(
        child_messages
            .iter()
            .filter(|message| matches!(message.info, MessageInfo::Assistant(_)))
            .count(),
        1
    );

    let collected = execute_tool_call_with_permission_wait(
        &state,
        &parent.id,
        &Id::ascending(IdKind::Message),
        &parent.directory,
        vec![PermissionRule {
            permission: "*".to_string(),
            pattern: "*".to_string(),
            action: PermissionAction::Allow,
        }],
        "call-task-result",
        "task_result",
        json!({ "task_id": child_id }),
    )
    .await
    .unwrap();
    assert!(collected.output.contains("status: completed"));
    assert!(collected.output.contains("<task_result>"));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn task_tool_resumes_existing_child_session() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-task-resume-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let mut parent: SessionInfo = response_json(
        app.oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    let allow_all = vec![PermissionRule {
        permission: "*".to_string(),
        pattern: "*".to_string(),
        action: PermissionAction::Allow,
    }];
    let first_execution = crate::execution_activity::ensure_for_prompt(
        &state,
        &mut parent,
        "message-parent-first",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    state.inner.store.update_session(&parent).await.unwrap();
    let first_parent_run = crate::state::SessionRun {
        id: "parent-task-resume-e1".to_string(),
        started_at: 1,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    state
        .inner
        .session_coordinator
        .try_start_run(parent.id.as_str(), first_parent_run.clone())
        .await
        .unwrap();

    let first = execute_tool_call_with_permission_wait(
        &state,
        &parent.id,
        &Id::ascending(IdKind::Message),
        &parent.directory,
        allow_all.clone(),
        "call-task-1",
        "task",
        json!({
            "description": "Inspect runtime",
            "prompt": "First child turn",
            "subagent_type": "general",
            "background": true
        }),
    )
    .await
    .unwrap();
    let child_id = first
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("sessionId"))
        .and_then(Value::as_str)
        .expect("child id")
        .to_string();
    let first_notifications = wait_for_queued_prompt(&state, parent.id.as_str()).await;
    assert_eq!(first_notifications.len(), 1);
    assert!(matches!(
        first_notifications[0].0.parts.first(),
        Some(PromptPart::Text { text }) if text.contains(child_id.as_str())
    ));
    let (first_notification, _) = state
        .inner
        .store
        .pop_queued_prompt_with_delivery(parent.id.as_str(), None)
        .await
        .unwrap()
        .expect("first completion notification");
    crate::session_actions::acknowledge_parent_subtask_completion_delivery(
        &state,
        parent.id.as_str(),
        &first_notification,
    )
    .await
    .unwrap();
    assert!(
        state
            .inner
            .session_coordinator
            .finish_run(parent.id.as_str(), &first_parent_run.id)
            .await
    );
    tokio::time::timeout(
        Duration::from_secs(2),
        state
            .inner
            .session_coordinator
            .wait_until_idle(parent.id.as_str()),
    )
    .await
    .expect("parent queue worker should settle after E1 notification is consumed");
    let mut settled_first = state
        .inner
        .store
        .get_execution_activity(parent.id.as_str())
        .await
        .unwrap()
        .expect("first execution");
    for _ in 0..100 {
        crate::execution_activity::finish_if_quiescent(&state, parent.id.as_str()).await;
        settled_first = state
            .inner
            .store
            .get_execution_activity(parent.id.as_str())
            .await
            .unwrap()
            .expect("first execution");
        if settled_first.finished {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let first_branches = state
        .inner
        .store
        .list_execution_subtasks(&first_execution.execution_id)
        .await
        .unwrap();
    let parent_store_runs = state
        .inner
        .store
        .session_run_statuses(parent.id.as_str())
        .await
        .unwrap();
    let child_store_runs = state
        .inner
        .store
        .session_run_statuses(&child_id)
        .await
        .unwrap();
    assert_eq!(settled_first.execution_id, first_execution.execution_id);
    assert!(
        settled_first.finished,
        "E1 remained active after its parent run, worker, queue, and child branch settled: snapshot={settled_first:?} branches={first_branches:?} parent_runs={parent_store_runs:?} child_runs={child_store_runs:?}"
    );
    let second_execution = crate::execution_activity::ensure_for_prompt(
        &state,
        &mut parent,
        "message-parent-second",
        true,
    )
    .await
    .unwrap()
    .unwrap();
    assert_ne!(second_execution.execution_id, first_execution.execution_id);
    state.inner.store.update_session(&parent).await.unwrap();
    let second_parent_run = crate::state::SessionRun {
        id: "parent-task-resume-e2".to_string(),
        started_at: 2,
        cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };
    state
        .inner
        .session_coordinator
        .try_start_run(parent.id.as_str(), second_parent_run)
        .await
        .unwrap();

    let second = execute_tool_call_with_permission_wait(
        &state,
        &parent.id,
        &Id::ascending(IdKind::Message),
        &parent.directory,
        allow_all,
        "call-task-2",
        "task",
        json!({
            "description": "Inspect runtime",
            "prompt": "Second child turn",
            "subagent_type": "general",
            "task_id": &child_id,
            "background": true
        }),
    )
    .await
    .unwrap();
    let resumed_id = second
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("sessionId"))
        .and_then(Value::as_str)
        .expect("resumed child id");
    assert_eq!(resumed_id, child_id);

    let second_notifications = wait_for_queued_prompt(&state, parent.id.as_str()).await;
    assert_eq!(second_notifications.len(), 1);
    let second_notification = &second_notifications[0].0;
    assert_ne!(
        second_notification.message_id,
        first_notification.message_id
    );
    assert!(matches!(
        second_notification.parts.first(),
        Some(PromptPart::Text { text }) if text.contains(child_id.as_str())
    ));
    let current_runtime = state
        .inner
        .store
        .get_session_runtime_snapshot(parent.id.as_str())
        .await
        .unwrap();
    assert_eq!(
        current_runtime
            .execution
            .as_ref()
            .map(|execution| execution.execution_id.as_str()),
        Some(second_execution.execution_id.as_str())
    );
    assert!(current_runtime
        .branches
        .iter()
        .any(|branch| { branch.session_id == child_id && branch.status == "completed" }));

    let child_messages = state.inner.store.list_messages(&child_id).await.unwrap();
    let user_prompts = child_messages
        .iter()
        .filter(|message| matches!(message.info, MessageInfo::User(_)))
        .flat_map(|message| message.parts.iter())
        .filter_map(|part| match part {
            Part::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(user_prompts, vec!["First child turn", "Second child turn"]);
    let stored_child = state
        .inner
        .store
        .get_session(&child_id)
        .await
        .unwrap()
        .unwrap();
    let completions = stored_child.extra["subtaskCompletions"].as_array().unwrap();
    assert_eq!(completions.len(), 2);
    assert_ne!(completions[0]["generation"], completions[1]["generation"]);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v2_prompt_accepts_subtask_parts_and_children_page() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v2-subtask-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let parent: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({
                    "model": {
                        "providerId": "neoism",
                        "id": "stub",
                        "variant": "xhigh"
                    }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(
        parent
            .model
            .as_ref()
            .and_then(|model| model.variant.as_deref()),
        Some("xhigh")
    );

    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{}/prompt", parent.id),
            Some(json!({
                "delivery": "steer",
                "parts": [{
                    "type": "subtask",
                    "prompt": "Inspect the v2 subtask path",
                    "description": "Inspect v2",
                    "agent": "general"
                }]
            })),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);
    let parent_messages = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let messages = state
                .inner
                .store
                .list_messages(parent.id.as_str())
                .await
                .unwrap();
            let finished = messages.iter().any(|message| {
                message.parts.iter().any(|part| {
                    matches!(
                        part,
                        Part::Tool(tool)
                            if tool.tool == "task"
                                && matches!(&tool.state, ToolState::Completed { .. })
                    )
                })
            });
            if finished {
                break messages;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("v2 prompt should complete");
    assert!(
        parent_messages.iter().any(|message| {
            message.parts.iter().any(|part| {
                matches!(
                    part,
                    Part::Tool(tool)
                        if tool.tool == "task"
                            && matches!(&tool.state, ToolState::Completed { .. })
                )
            })
        }),
        "expected v2 subtask to produce parent task tool part"
    );

    let children: Page<SessionInfo> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/sessions/{}/children", parent.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(children.items.len(), 1);
    assert_eq!(children.items[0].parent_id.as_ref(), Some(&parent.id));
    assert_eq!(children.items[0].agent.as_deref(), Some("general"));
    let child_model = children.items[0]
        .model
        .as_ref()
        .expect("v2 subtask should inherit parent model");
    assert_eq!(child_model.provider_id, "neoism");
    assert_eq!(child_model.id, "stub");
    assert_eq!(child_model.variant.as_deref(), Some("xhigh"));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if state
                .inner
                .store
                .list_messages(children.items[0].id.as_str())
                .await
                .unwrap()
                .len()
                >= 2
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("v2 child session should produce a reply");

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn permission_and_question_replies_publish_events() {
    async fn next_matching_event(
        events: &mut tokio::sync::broadcast::Receiver<EventPayload>,
        expected: &str,
    ) {
        tokio::time::timeout(Duration::from_secs(2), async {
            loop {
                if events.recv().await.unwrap().kind == expected {
                    return;
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("event {expected} was not published"));
    }
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let permission_id = Id::ascending(IdKind::Permission).to_string();
    let question_id = Id::ascending(IdKind::Question).to_string();
    let rejected_question_id = Id::ascending(IdKind::Question).to_string();
    state.inner.permissions.write().await.insert(
        permission_id.clone(),
        PermissionRequestInfo {
            id: permission_id.clone(),
            session_id: Id::ascending(IdKind::Session).to_string(),
            message_id: Id::ascending(IdKind::Message).to_string(),
            title: "Allow read".to_string(),
            permission: "read".to_string(),
            patterns: vec!["file.txt".to_string()],
            always: vec!["file.txt".to_string()],
            tool: None,
            metadata: None,
        },
    );
    for id in [&question_id, &rejected_question_id] {
        state.inner.questions.write().await.insert(
            (*id).clone(),
            QuestionRequestInfo {
                id: (*id).clone(),
                session_id: Id::ascending(IdKind::Session).to_string(),
                message_id: Id::ascending(IdKind::Message).to_string(),
                questions: vec![json!({ "label": "Proceed?" })],
            },
        );
    }
    let mut events = state.subscribe();
    let app = app(state.clone());

    let ok: bool = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/interactions/permissions/{permission_id}/reply"),
                Some(json!({ "reply": "once" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(ok);
    next_matching_event(&mut events, event_type::PERMISSION_REPLIED).await;

    let ok: bool = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/interactions/questions/{question_id}/reply"),
                Some(json!({ "answers": [["yes"]] })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(ok);
    next_matching_event(&mut events, event_type::QUESTION_REPLIED).await;

    let ok: bool = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/interactions/questions/{rejected_question_id}/reject"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(ok);
    next_matching_event(&mut events, event_type::QUESTION_REJECTED).await;

    cleanup_sqlite_files(&path);
}
