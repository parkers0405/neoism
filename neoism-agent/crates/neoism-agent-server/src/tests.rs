use super::*;
use crate::state::SessionStore;
use crate::tool_selection::provider_tool_map;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use futures::StreamExt;
use neoism_agent_core::{
    AgentConfigDocument, AuthInfo, ProviderListResult, SessionUndoStatus, SessionUndoTree,
};
use serde::de::DeserializeOwned;
use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tower::ServiceExt;

#[path = "tests_interaction_tools.rs"]
mod interaction_tool_tests;

#[path = "tests_session_queue.rs"]
mod session_queue_tests;
#[path = "tests_session_undo.rs"]
mod session_undo_tests;
#[path = "tests_tool_parts.rs"]
mod tool_part_tests;

#[test]
fn gpt_models_get_opencode_patch_toolset() {
    assert!(use_apply_patch_for_model("gpt-5.5"));
    assert!(use_apply_patch_for_model("openai/gpt-5.4-codex"));
    assert!(use_apply_patch_for_model("codex-mini-latest"));
    assert!(tool_allowed_for_model("apply_patch", "gpt-5.5"));
    assert!(!tool_allowed_for_model("edit", "gpt-5.5"));
    assert!(!tool_allowed_for_model("write", "gpt-5.5"));

    assert!(!use_apply_patch_for_model("gpt-4.1"));
    assert!(!tool_allowed_for_model("apply_patch", "gpt-4.1"));
    assert!(tool_allowed_for_model("edit", "gpt-4.1"));
    assert!(tool_allowed_for_model("write", "gpt-4.1"));

    let available = provider_tool_map(
        &tool::workspace_tool_items()
            .into_iter()
            .filter(|tool| tool.id == "apply_patch")
            .collect::<Vec<_>>(),
    );
    assert_eq!(
        normalize_provider_tool_name("apply_patch", &json!({}), &available).as_deref(),
        Some("apply_patch")
    );
    // Provider calls must use the exact advertised name. Silent aliases make
    // stale prompts and mismatched schemas much harder to detect.
    assert_eq!(
        normalize_provider_tool_name("patch", &json!({}), &available).as_deref(),
        None
    );
    assert_eq!(
        normalize_provider_tool_name(
            "edit",
            &json!({ "patchText": "*** Begin Patch\n*** End Patch" }),
            &available,
        )
        .as_deref(),
        None
    );
    assert!(normalize_provider_tool_name("edit", &json!({}), &available).is_none());
}

#[tokio::test]
async fn diagnostic_tool_results_publish_lsp_updated_event() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-lsp-updated-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let mut events = state.subscribe();

    let result = tool::ToolExecutionResult {
        title: "Edited file".to_string(),
        output: "ok".to_string(),
        metadata: Some(json!({ "diagnostics": [], "diagnosticsCount": 0 })),
    };
    tool_runtime::publish_lsp_updated_if_needed(&state, &result);

    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .expect("lsp.updated should be published")
        .unwrap();
    assert_eq!(event.kind, event_type::LSP_UPDATED);
    assert_eq!(event.properties, json!({}));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn compacted_summary_is_added_to_provider_context() {
    let session_id = neoism_agent_core::new_session_id();
    let message_id = Id::ascending(IdKind::Message);
    let info = SessionInfo {
        id: session_id.clone(),
        slug: "summary-test".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Summary Test".to_string(),
        agent: None,
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    let mut messages = vec![MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: 1 },
            agent: "build".to_string(),
            model: UserModel {
                provider_id: "neoism".to_string(),
                model_id: "stub".to_string(),
                variant: None,
            },
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: session_id.clone(),
            message_id,
            text: "summarize this context".to_string(),
            synthetic: None,
            time: None,
        })],
    }];
    let summary = build_session_summary(&messages);
    messages.extend(test_compaction_pair(&session_id, None, &summary));

    let provider_messages = provider_messages_for_session(&info, &messages, "stub", None, true);

    assert!(matches!(provider_messages[0].role, ProviderRole::System));
    assert!(provider_messages[0]
        .content
        .contains("interactive coding agent running in a real workspace"));
    assert!(provider_messages
        .iter()
        .any(|message| message.content.contains("summarize this context")));
    assert_eq!(provider_messages.len(), 2);
}

#[test]
fn provider_context_includes_active_run_system_once() {
    let session_id = neoism_agent_core::new_session_id();
    let message_id = Id::ascending(IdKind::Message);
    let info = SessionInfo {
        id: session_id.clone(),
        slug: "run-system-test".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Run System Test".to_string(),
        agent: None,
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    let messages = vec![MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: 1 },
            agent: "build".to_string(),
            model: UserModel {
                provider_id: "neoism".to_string(),
                model_id: "stub".to_string(),
                variant: None,
            },
            system: Some("legacy duplicated prompt".to_string()),
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id,
            message_id,
            text: "real user request".to_string(),
            synthetic: None,
            time: None,
        })],
    }];

    let provider_messages = provider_messages_for_session(
        &info,
        &messages,
        "stub",
        Some("active run prompt"),
        true,
    );

    assert_eq!(
        provider_messages
            .iter()
            .filter(|message| message.content.contains("active run prompt"))
            .count(),
        1
    );
    assert!(!provider_messages
        .iter()
        .any(|message| message.content.contains("legacy duplicated prompt")));
    assert!(provider_messages
        .iter()
        .any(|message| message.content.contains("real user request")));
}

#[test]
fn compacted_summary_trims_messages_already_covered_by_summary() {
    let session_id = neoism_agent_core::new_session_id();
    let first_id = Id::ascending(IdKind::Message);
    let second_id = Id::ascending(IdKind::Message);
    let info = SessionInfo {
        id: session_id.clone(),
        slug: "summary-tail-test".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Summary Tail Test".to_string(),
        agent: None,
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    let old_message = MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: first_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: 1 },
            agent: "build".to_string(),
            model: UserModel {
                provider_id: "neoism".to_string(),
                model_id: "stub".to_string(),
                variant: None,
            },
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: session_id.clone(),
            message_id: first_id,
            text: "old compacted request".to_string(),
            synthetic: None,
            time: None,
        })],
    };
    let tail_message = MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: second_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: 2 },
            agent: "build".to_string(),
            model: UserModel {
                provider_id: "neoism".to_string(),
                model_id: "stub".to_string(),
                variant: None,
            },
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: session_id.clone(),
            message_id: second_id.clone(),
            text: "new tail request".to_string(),
            synthetic: None,
            time: None,
        })],
    };
    let mut messages = vec![old_message];
    messages.extend(test_compaction_pair(
        &session_id,
        Some(second_id.clone()),
        "Summary covers old compacted request.",
    ));
    messages.push(tail_message);

    let provider_messages = provider_messages_for_session(&info, &messages, "stub", None, true);

    assert!(provider_messages.iter().any(|message| message
        .content
        .contains("Summary covers old compacted request")));
    assert!(provider_messages
        .iter()
        .any(|message| message.content.contains("new tail request")));
    assert!(!provider_messages
        .iter()
        .skip(2)
        .any(|message| message.content.contains("old compacted request")));
}

fn test_compaction_pair(
    session_id: &neoism_agent_core::SessionId,
    tail_start_message_id: Option<neoism_agent_core::MessageId>,
    summary: &str,
) -> [MessageWithParts; 2] {
    let user_id = Id::ascending(IdKind::Message);
    let assistant_id = Id::ascending(IdKind::Message);
    [
        MessageWithParts {
            info: MessageInfo::User(UserMessage {
                id: user_id.clone(),
                session_id: session_id.clone(),
                time: CreatedTime { created: 10 },
                agent: "neoism".to_string(),
                model: UserModel {
                    provider_id: "neoism".to_string(),
                    model_id: "stub".to_string(),
                    variant: None,
                },
                system: None,
                tools: None,
                author: None,
            }),
            parts: vec![Part::Compaction(CompactionPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: user_id.clone(),
                reason: "test".to_string(),
                summary: false,
                tail_start_message_id,
            })],
        },
        MessageWithParts {
            info: MessageInfo::Assistant(AssistantMessage {
                id: assistant_id.clone(),
                session_id: session_id.clone(),
                time: CompletedTime {
                    created: 11,
                    streamed: Some(12),
                    completed: Some(12),
                },
                parent_id: user_id,
                mode: "compaction".to_string(),
                agent: "neoism".to_string(),
                path: AssistantPath {
                    cwd: "/tmp".to_string(),
                    root: "/tmp".to_string(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: "stub".to_string(),
                provider_id: "neoism".to_string(),
                finish: Some("stop".to_string()),
                error: None,
            }),
            parts: vec![
                Part::Compaction(CompactionPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id: assistant_id.clone(),
                    reason: "summary".to_string(),
                    summary: true,
                    tail_start_message_id: None,
                }),
                Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id: assistant_id,
                    text: summary.to_string(),
                    synthetic: Some(true),
                    time: None,
                }),
            ],
        },
    ]
}

#[tokio::test]
async fn store_persists_sessions_and_messages() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let info = SessionInfo {
        id: session_id.clone(),
        slug: "test-session".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Test Session".to_string(),
        agent: None,
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    store.insert_session(&info).await.unwrap();

    let user_message_id = Id::ascending(IdKind::Message);
    store
        .append_message(
            session_id.as_str(),
            &MessageWithParts {
                info: MessageInfo::User(UserMessage {
                    id: user_message_id.clone(),
                    session_id: session_id.clone(),
                    time: CreatedTime { created: now },
                    agent: "build".to_string(),
                    model: UserModel {
                        provider_id: "neoism".to_string(),
                        model_id: "stub".to_string(),
                        variant: None,
                    },
                    system: None,
                    tools: None,
                    author: None,
                }),
                parts: vec![Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id: user_message_id.clone(),
                    text: "persist me".to_string(),
                    synthetic: None,
                    time: None,
                })],
            },
        )
        .await
        .unwrap();
    store.close().await;

    let store = SessionStore::open(path.clone()).await.unwrap();
    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, session_id);
    let messages = store.list_messages(session_id.as_str()).await.unwrap();
    assert_eq!(messages.len(), 1);
    assert_eq!(message_id_of(&messages[0]), user_message_id.to_string());
    store.close().await;
    cleanup_sqlite_files(&path);
}

fn store_test_session(
    session_id: &neoism_agent_core::SessionId,
    now: u64,
) -> SessionInfo {
    SessionInfo {
        id: session_id.clone(),
        slug: "store-session".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Store Session".to_string(),
        agent: None,
        model: None,
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    }
}

fn store_test_message(
    session_id: &neoism_agent_core::SessionId,
    now: u64,
    text: &str,
) -> MessageWithParts {
    let message_id = Id::ascending(IdKind::Message);
    MessageWithParts {
        info: MessageInfo::User(UserMessage {
            id: message_id.clone(),
            session_id: session_id.clone(),
            time: CreatedTime { created: now },
            agent: "build".to_string(),
            model: UserModel {
                provider_id: "neoism".to_string(),
                model_id: "stub".to_string(),
                variant: None,
            },
            system: None,
            tools: None,
            author: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id: session_id.clone(),
            message_id,
            text: text.to_string(),
            synthetic: None,
            time: None,
        })],
    }
}

#[tokio::test]
async fn transcript_search_route_serves_keyword_hits_without_embeddings() {
    // Force the no-embeddings path so this is deterministic regardless of
    // any provider keys in the environment.
    std::env::set_var("NEOISM_AGENT_DISABLE_EMBEDDINGS", "1");
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-keyword-search-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let session = store_test_session(&session_id, now);
    state.inner.store.insert_session(&session).await.unwrap();
    state
        .inner
        .store
        .append_message(
            session_id.as_str(),
            &store_test_message(&session_id, now, "this may sound weird but go with it"),
        )
        .await
        .unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v2/plugins/dev.neoism.semantic/search?q=sound%20weird&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(
        parsed["available"], true,
        "content search must stay available without an embeddings provider"
    );
    let hits = parsed["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1, "{parsed}");
    assert_eq!(hits[0]["sessionId"], session_id.to_string());
    let excerpt = hits[0]["excerpt"].as_str().unwrap();
    assert!(
        excerpt.contains("sound weird but go with it"),
        "excerpt carries the matched chunk: {excerpt}"
    );
    assert!(
        !excerpt.contains(">>") && !excerpt.contains("<<"),
        "tool-facing match markers are stripped for the UI: {excerpt}"
    );
    assert_eq!(hits[0]["distance"], 0.0, "exact matches outrank semantic ones");

    std::env::remove_var("NEOISM_AGENT_DISABLE_EMBEDDINGS");
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn transcript_search_falls_back_to_per_term_matches_for_multi_word_queries() {
    std::env::set_var("NEOISM_AGENT_DISABLE_EMBEDDINGS", "1");
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-or-search-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let now = now_millis();
    // Two sessions, each mentioning only ONE of the query's words — the
    // AND search finds neither, the per-term fallback finds both.
    let tokenizer_session = neoism_agent_core::new_session_id();
    let unicode_session = neoism_agent_core::new_session_id();
    for (session_id, text) in [
        (&tokenizer_session, "rewrote the tokenizer end to end"),
        (&unicode_session, "unicode boundaries were the culprit"),
    ] {
        let session = store_test_session(session_id, now);
        state.inner.store.insert_session(&session).await.unwrap();
        state
            .inner
            .store
            .append_message(
                session_id.as_str(),
                &store_test_message(session_id, now, text),
            )
            .await
            .unwrap();
    }

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri("/v2/plugins/dev.neoism.semantic/search?q=tokenizer%20unicode&limit=10")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(status, StatusCode::OK, "{}", String::from_utf8_lossy(&body));
    let parsed: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let hits = parsed["hits"].as_array().unwrap();
    let sessions: Vec<&str> = hits
        .iter()
        .filter_map(|hit| hit["sessionId"].as_str())
        .collect();
    assert!(
        sessions.contains(&tokenizer_session.as_str())
            && sessions.contains(&unicode_session.as_str()),
        "per-term fallback surfaces both single-word matches: {parsed}"
    );

    std::env::remove_var("NEOISM_AGENT_DISABLE_EMBEDDINGS");
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn store_persists_sessions_and_searches_with_like() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let mut session = store_test_session(&session_id, now);
    session.set_goal(&neoism_agent_core::SessionGoal {
        text: "survive a Turso reopen".to_string(),
        created: now,
        updated: now + 1,
        status: neoism_agent_core::GoalStatus::Complete,
        summary: "goal state, lifecycle, and summary are durable".to_string(),
        ..Default::default()
    });
    store.insert_session(&session).await.unwrap();
    for text in ["the quick brown fox jumps", "unrelated transcript entry"] {
        store
            .append_message(
                session_id.as_str(),
                &store_test_message(&session_id, now, text),
            )
            .await
            .unwrap();
    }

    // Reopen the same file to prove persistence across handles.
    drop(store);
    let store = SessionStore::open(path.clone()).await.unwrap();
    let sessions = store.list_sessions().await.unwrap();
    assert_eq!(sessions.len(), 1);
    let persisted_goal = sessions[0].goal().expect("goal persisted through Turso");
    assert_eq!(persisted_goal.text, "survive a Turso reopen");
    assert_eq!(
        persisted_goal.status,
        neoism_agent_core::GoalStatus::Complete
    );
    assert_eq!(
        persisted_goal.summary,
        "goal state, lifecycle, and summary are durable"
    );
    assert_eq!(
        store
            .list_messages(session_id.as_str())
            .await
            .unwrap()
            .len(),
        2
    );

    // Transcript search ANDs terms in the bounded LIKE scan.
    let hits = store.search_messages("quick fox", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].session_id, session_id.to_string());
    assert!(
        hits[0].excerpt.contains(">>quick<<"),
        "excerpt: {}",
        hits[0].excerpt
    );
    assert!(store
        .search_messages("quick zebra", None, 10)
        .await
        .unwrap()
        .is_empty());

    // delete_session removes child rows explicitly.
    store
        .admit_execution_activity(
            session_id.as_str(),
            "execution-delete",
            "message-delete",
            "",
        )
        .await
        .unwrap()
        .unwrap();
    store
        .insert_execution_segment(
            session_id.as_str(),
            "execution-delete",
            "segment-delete",
            "owner-delete",
            session_id.as_str(),
            now,
        )
        .await
        .unwrap();
    assert!(store.delete_session(session_id.as_str()).await.unwrap());
    assert!(store
        .list_messages(session_id.as_str())
        .await
        .unwrap()
        .is_empty());
    assert!(store
        .get_session_runtime_snapshot(session_id.as_str())
        .await
        .unwrap()
        .execution
        .is_none());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn cancelled_subtask_completion_keeps_admission_armed_for_retry() {
    let path = std::env::temp_dir().join(format!(
        "neoism-subtask-admission-drop-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let parent_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let mut parent = store_test_session(&parent_id, now_millis());
    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(parent_id.clone());
    state.inner.store.insert_session(&parent).await.unwrap();
    state.inner.store.insert_session(&child).await.unwrap();
    let execution = state.inner.store
        .admit_execution_activity(parent_id.as_str(), "execution-guard", "message-guard", "")
        .await.unwrap().unwrap();
    parent.extra.insert(crate::execution_activity::EXECUTION_ID_KEY.into(), json!(execution.execution_id));
    parent.extra.insert(crate::execution_activity::EXECUTION_ROOT_KEY.into(), json!(parent_id.to_string()));
    state.inner.store.update_session(&parent).await.unwrap();
    let guard = crate::execution_activity::SubtaskAdmissionGuard::admit(
        &state, &parent, child_id.as_str(),
    ).await.unwrap();
    let child_two_id = neoism_agent_core::new_session_id();
    let mut child_two = store_test_session(&child_two_id, now_millis());
    child_two.parent_id = Some(parent_id.clone());
    state.inner.store.insert_session(&child_two).await.unwrap();
    crate::execution_activity::register_subtask(&state, &parent, child_two_id.as_str())
        .await
        .unwrap();
    let writer = state.inner.store.lock_writer_for_test().await;
    let finish = tokio::spawn(guard.complete("completed"));
    tokio::task::yield_now().await;
    finish.abort();
    let _ = finish.await;
    drop(writer);
    tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            if state.inner.store
                .execution_subtask_status("execution-guard", child_id.as_str())
                .await.unwrap().as_deref() == Some("completed")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    }).await.unwrap();
    assert!(crate::execution_activity::SubtaskAdmissionGuard::admit(
        &state, &parent, child_id.as_str(),
    ).await.is_err(), "terminal duplicate must not resurrect");
    crate::session_actions::publish_background_subtask_finished(
        &state,
        child_two_id.as_str(),
        &Id::ascending(IdKind::Message),
        "completed",
        "done",
    ).await;
    assert_eq!(
        state.inner.store
            .execution_subtask_status("execution-guard", child_two_id.as_str())
            .await.unwrap().as_deref(),
        Some("completed"),
        "execution terminalization must not depend on notification tracking",
    );
    let child_three_id = neoism_agent_core::new_session_id();
    let mut child_three = store_test_session(&child_three_id, now_millis());
    child_three.parent_id = Some(parent_id.clone());
    state.inner.store.insert_session(&child_three).await.unwrap();
    assert!(crate::execution_activity::SubtaskAdmissionGuard::admit(
        &state,
        &parent,
        child_three_id.as_str(),
    )
    .await
    .is_err(), "finished execution must reject and prevent child launch");
    assert!(state.inner.store
        .execution_subtask_status("execution-guard", child_three_id.as_str())
        .await.unwrap().is_none());
    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn deleting_child_cleans_runtime_without_deleting_root_execution() {
    let path = std::env::temp_dir().join(format!(
        "neoism-delete-child-execution-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let store = SessionStore::open(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let root = store_test_session(&root_id, now_millis());
    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(root_id.clone());
    store.insert_session(&root).await.unwrap();
    store.insert_session(&child).await.unwrap();
    store.admit_execution_activity(root_id.as_str(), "execution-delete-child", "message", "")
        .await.unwrap().unwrap();
    store.register_execution_subtask(
        "execution-delete-child", root_id.as_str(), root_id.as_str(), child_id.as_str(), 10,
    ).await.unwrap();
    store.insert_execution_segment(
        root_id.as_str(), "execution-delete-child", "child-segment", "owner", child_id.as_str(), 10,
    ).await.unwrap();
    assert!(store.delete_session(child_id.as_str()).await.unwrap());
    let runtime = store.get_session_runtime_snapshot(root_id.as_str()).await.unwrap();
    assert!(runtime.execution.is_some());
    assert!(runtime.branches.is_empty());
    assert!(runtime.execution.unwrap().active_segments.is_empty());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn setting_a_new_goal_reopens_a_completed_goal() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-goal-replacement-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let mut session = store_test_session(&session_id, now);
    session.set_goal(&neoism_agent_core::SessionGoal {
        text: "finished goal".to_string(),
        created: now,
        updated: now + 10,
        status: neoism_agent_core::GoalStatus::Complete,
        summary: "finished summary".to_string(),
        ..Default::default()
    });
    state.inner.store.insert_session(&session).await.unwrap();

    let response: Value = response_json(
        app(state.clone())
            .oneshot(request(
                Method::POST,
                &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
                Some(json!({ "text": "new active goal" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(response["goal"]["text"], "new active goal");
    assert_eq!(response["goal"]["status"], "active");
    assert!(
        response["goal"].get("summary").is_none(),
        "reopening a goal clears the completed summary"
    );
    assert!(
        response["goal"]["updated"].as_u64().unwrap() > now + 10,
        "replacement must advance past the completed goal's version"
    );

    let stored = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .expect("session remains stored");
    let goal = stored.goal().expect("replacement goal is durable");
    assert_eq!(goal.status, neoism_agent_core::GoalStatus::Active);
    assert_eq!(goal.text, "new active goal");
    assert!(goal.summary.is_empty());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn session_directory_patch_moves_and_persists_the_session() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-session-cd-route-{}",
        Id::ascending(IdKind::Event)
    ));
    let current = root.join("current");
    let target = root.join("target");
    std::fs::create_dir_all(&current).unwrap();
    std::fs::create_dir_all(&target).unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let mut session = store_test_session(&session_id, now_millis());
    session.directory = current.to_string_lossy().to_string();
    state.inner.store.insert_session(&session).await.unwrap();

    let response: SessionInfo = response_json(
        app(state.clone())
            .oneshot(request(
                Method::PATCH,
                &format!("/v2/sessions/{session_id}"),
                Some(json!({ "directory": "../target" })),
            ))
            .await
            .unwrap(),
    )
    .await;

    let expected = target.canonicalize().unwrap().to_string_lossy().to_string();
    assert_eq!(response.directory, expected);
    let stored = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .expect("moved session remains stored");
    assert_eq!(stored.directory, expected);
    assert!(stored.extra.contains_key("contextEpoch"));

    state.inner.store.close().await;
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn semantic_store_ranks_by_vector_distance_on_turso() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-sem-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open(path.clone()).await.unwrap();
    assert!(store.semantic_search_supported());
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    store
        .insert_session(&store_test_session(&session_id, now))
        .await
        .unwrap();
    for text in ["rust borrow checker", "cooking pasta recipe"] {
        store
            .append_message(
                session_id.as_str(),
                &store_test_message(&session_id, now, text),
            )
            .await
            .unwrap();
    }
    let messages = store.list_messages(session_id.as_str()).await.unwrap();
    let (first_id, second_id) =
        (message_id_of(&messages[0]), message_id_of(&messages[1]));

    let pending = store
        .messages_missing_embeddings("test-model", &[session_id.to_string()], 10)
        .await
        .unwrap();
    assert_eq!(pending.len(), 2);

    store
        .upsert_message_embedding(
            &first_id,
            session_id.as_str(),
            1,
            "test-model",
            "[1,0,0]",
        )
        .await
        .unwrap();
    store
        .upsert_message_embedding(
            &second_id,
            session_id.as_str(),
            1,
            "test-model",
            "[0,1,0]",
        )
        .await
        .unwrap();
    assert!(store
        .messages_missing_embeddings("test-model", &[session_id.to_string()], 10)
        .await
        .unwrap()
        .is_empty());

    // Query vector close to the first embedding: it must rank first.
    let hits = store
        .semantic_search("[0.9,0.1,0]", "test-model", None, 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 2);
    assert_eq!(hits[0].message_id, first_id);
    assert!(hits[0].distance < hits[1].distance);
    assert!(hits[0].excerpt.contains("rust borrow checker"));

    // A different model's vectors are invisible, and tombstones drop rows
    // out of the missing set without becoming searchable.
    assert!(store
        .semantic_search("[0.9,0.1,0]", "other-model", None, 10)
        .await
        .unwrap()
        .is_empty());
    store
        .tombstone_message_embedding(&first_id, session_id.as_str(), 1)
        .await
        .unwrap();
    let hits = store
        .semantic_search("[0.9,0.1,0]", "test-model", None, 10)
        .await
        .unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0].message_id, second_id);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn list_messages_page_pages_by_cursor_in_sql() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-page-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    store
        .insert_session(&SessionInfo {
            id: session_id.clone(),
            slug: "page-session".to_string(),
            project_id: "global".to_string(),
            workspace_id: None,
            directory: "/tmp".to_string(),
            path: None,
            parent_id: None,
            title: "Page Session".to_string(),
            agent: None,
            model: None,
            version: env!("CARGO_PKG_VERSION").to_string(),
            time: TimeInfo {
                created: now,
                updated: now,
                compacting: None,
                archived: None,
            },
            permission: None,
            extra: BTreeMap::new(),
        })
        .await
        .unwrap();

    // Append 5 user messages in order; remember each message id and part id.
    let mut message_ids = Vec::new();
    let mut part_ids = Vec::new();
    for index in 0..5 {
        let message_id = Id::ascending(IdKind::Message);
        let part_id = Id::ascending(IdKind::Part);
        store
            .append_message(
                session_id.as_str(),
                &MessageWithParts {
                    info: MessageInfo::User(UserMessage {
                        id: message_id.clone(),
                        session_id: session_id.clone(),
                        time: CreatedTime {
                            created: now + index,
                        },
                        agent: "build".to_string(),
                        model: UserModel {
                            provider_id: "neoism".to_string(),
                            model_id: "stub".to_string(),
                            variant: None,
                        },
                        system: None,
                        tools: None,
                        author: None,
                    }),
                    parts: vec![Part::Text(TextPart {
                        id: part_id.clone(),
                        session_id: session_id.clone(),
                        message_id: message_id.clone(),
                        text: format!("message {index}"),
                        synthetic: None,
                        time: None,
                    })],
                },
            )
            .await
            .unwrap();
        message_ids.push(message_id.to_string());
        part_ids.push(part_id.to_string());
    }

    let text_of = |message: &MessageWithParts| match &message.parts[0] {
        Part::Text(part) => part.text.clone(),
        _ => unreachable!(),
    };

    // desc + limit → newest first.
    let newest = store
        .list_messages_page(session_id.as_str(), None, Some(2), true)
        .await
        .unwrap();
    assert_eq!(
        newest.iter().map(&text_of).collect::<Vec<_>>(),
        vec!["message 4", "message 3"]
    );

    // desc + message-id cursor → the page immediately older than the cursor.
    let older = store
        .list_messages_page(session_id.as_str(), Some(&message_ids[4]), Some(2), true)
        .await
        .unwrap();
    assert_eq!(
        older.iter().map(&text_of).collect::<Vec<_>>(),
        vec!["message 3", "message 2"]
    );

    // A part id resolves to the same boundary as its message id.
    let older_by_part = store
        .list_messages_page(session_id.as_str(), Some(&part_ids[4]), Some(2), true)
        .await
        .unwrap();
    assert_eq!(
        older_by_part.iter().map(&text_of).collect::<Vec<_>>(),
        vec!["message 3", "message 2"]
    );

    // asc + limit → oldest first.
    let oldest = store
        .list_messages_page(session_id.as_str(), None, Some(2), false)
        .await
        .unwrap();
    assert_eq!(
        oldest.iter().map(&text_of).collect::<Vec<_>>(),
        vec!["message 0", "message 1"]
    );

    // An unresolved cursor behaves as no cursor (newest page).
    let unresolved = store
        .list_messages_page(
            session_id.as_str(),
            Some("prt_does_not_exist"),
            Some(1),
            true,
        )
        .await
        .unwrap();
    assert_eq!(
        unresolved.iter().map(&text_of).collect::<Vec<_>>(),
        vec!["message 4"]
    );

    store.close().await;
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn compact_session_publishes_streaming_compaction_events() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-compaction-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut events = state.subscribe();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    let info = SessionInfo {
        id: session_id.clone(),
        slug: "compaction-events".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Compaction Events".to_string(),
        agent: None,
        model: Some(neoism_agent_core::ModelRef {
            id: "stub".to_string(),
            provider_id: "neoism".to_string(),
            variant: None,
        }),
        version: env!("CARGO_PKG_VERSION").to_string(),
        time: TimeInfo {
            created: now,
            updated: now,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    state.inner.store.insert_session(&info).await.unwrap();

    let message_id = Id::ascending(IdKind::Message);
    state
        .inner
        .store
        .append_message(
            session_id.as_str(),
            &MessageWithParts {
                info: MessageInfo::User(UserMessage {
                    id: message_id.clone(),
                    session_id: session_id.clone(),
                    time: CreatedTime { created: now },
                    agent: "build".to_string(),
                    model: UserModel {
                        provider_id: "neoism".to_string(),
                        model_id: "stub".to_string(),
                        variant: None,
                    },
                    system: None,
                    tools: None,
                    author: None,
                }),
                parts: vec![Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id,
                    text: "remember this compactable context".to_string(),
                    synthetic: None,
                    time: None,
                })],
            },
        )
        .await
        .unwrap();

    let compacted = compact_session_context(&state, session_id.as_str())
        .await
        .unwrap();
    assert!(compacted.time.compacting.is_none());
    assert!(compacted
        .extra
        .get("summary")
        .and_then(|summary| summary.get("text"))
        .and_then(Value::as_str)
        .is_some_and(|text| !text.trim().is_empty()));

    let mut kinds = Vec::new();
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.unwrap();
            kinds.push(event.kind.clone());
            if event.kind == event_type::SESSION_COMPACTED {
                break;
            }
        }
    })
    .await
    .unwrap();

    assert!(kinds.contains(&event_type::SESSION_COMPACTION_STARTED.to_string()));
    assert!(kinds.contains(&event_type::SESSION_COMPACTION_DELTA.to_string()));
    assert!(kinds.contains(&event_type::SESSION_COMPACTION_ENDED.to_string()));
    assert!(kinds.contains(&event_type::SESSION_COMPACTED.to_string()));
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn live_stream_events_broadcast_without_persistence() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-live-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let mut events = state.subscribe();
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": session_id,
            "messageID": "message-live",
            "partID": "part-live",
            "field": "text",
            "delta": "token"
        }),
    ));

    let event = tokio::time::timeout(Duration::from_secs(1), events.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(event.kind, event_type::MESSAGE_PART_DELTA);
    assert_eq!(event.properties["delta"], "token");
    assert!(state
        .inner
        .store
        .list_events_after(0, 10, Some(session_id.as_str()))
        .await
        .unwrap()
        .is_empty());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_root_event_stream_forwards_live_delta_from_child_created_after_connect() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-family-live-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let unrelated_id = neoism_agent_core::new_session_id();
    let root = store_test_session(&root_id, now_millis());
    let unrelated = store_test_session(&unrelated_id, now_millis());
    state.inner.store.insert_session(&root).await.unwrap();
    state.inner.store.insert_session(&unrelated).await.unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v2/events?sessionId={}&tail=true&limit=1",
                    root_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(root_id.clone());
    state.inner.store.insert_session(&child).await.unwrap();
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": unrelated_id,
            "messageID": "message-unrelated",
            "partID": "part-unrelated",
            "field": "text",
            "delta": "must-not-leak"
        }),
    ));
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": child_id,
            "messageID": "message-child",
            "partID": "part-child",
            "field": "text",
            "delta": "live-child-token"
        }),
    ));

    let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .expect("child delta should reach root SSE stream")
        .expect("SSE body should remain open")
        .expect("SSE body chunk should be readable");
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("message.part.delta"), "{text}");
    assert!(text.contains("live-child-token"), "{text}");
    assert!(text.contains(child_id.as_str()), "{text}");
    assert!(!text.contains("must-not-leak"), "{text}");

    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn foreign_session_verdicts_are_cached_per_connection() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-foreign-cache-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let foreign_id = neoism_agent_core::new_session_id();
    for id in [&root_id, &foreign_id] {
        let session = store_test_session(id, now_millis());
        state.inner.store.insert_session(&session).await.unwrap();
    }

    let mut family = Some(std::collections::HashSet::from([root_id.to_string()]));
    let mut foreign = std::collections::HashSet::new();
    let event = EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({ "sessionID": foreign_id, "messageID": "m", "partID": "p", "field": "text", "delta": "x" }),
    );
    // First foreign event pays the store walk and records the verdict...
    assert!(!crate::v2_routes::admit_live_event(
        &state, &event, Some(root_id.as_str()), &mut family, &mut foreign
    ).await);
    assert!(foreign.contains(foreign_id.as_str()), "negative verdict cached");
    // ...later ones skip the store entirely (delete the session row: a
    // cached verdict needs no lookup, an uncached one would now admit the
    // event through the missing-parent fallback path differently).
    state.inner.store.delete_session(root_id.as_str()).await.ok();
    assert!(!crate::v2_routes::admit_live_event(
        &state, &event, Some(root_id.as_str()), &mut family, &mut foreign
    ).await);

    // Family members always admit without store traffic.
    let own = EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({ "sessionID": root_id, "messageID": "m", "partID": "p", "field": "text", "delta": "y" }),
    );
    assert!(crate::v2_routes::admit_live_event(
        &state, &own, Some(root_id.as_str()), &mut family, &mut foreign
    ).await);

    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_live_events_carry_monotone_wire_sequences() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-wire-seq-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let session = store_test_session(&session_id, now_millis());
    state.inner.store.insert_session(&session).await.unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v2/events?sessionId={}&tail=true", session_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    // Mix live-only deltas and durable events: every broadcast frame must
    // carry a strictly increasing wire sequence — the SDK's reconnect
    // cursor depends on it (sequence: 0 on live events made clients drop
    // every event after the first as a replay).
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({ "sessionID": session_id, "messageID": "m", "partID": "p", "field": "text", "delta": "a" }),
    ));
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({ "sessionID": session_id, "part": { "id": "p", "type": "text", "messageID": "m", "sessionId": session_id, "text": "ab" } }),
    ));
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({ "sessionID": session_id, "messageID": "m", "partID": "p", "field": "text", "delta": "b" }),
    ));

    let mut received = String::new();
    let mut sequences: Vec<u64> = Vec::new();
    while sequences.len() < 3 {
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("stamped events should arrive")
            .expect("SSE stream open")
            .expect("readable chunk");
        received.push_str(&String::from_utf8_lossy(&chunk));
        sequences = received
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
            .filter_map(|value| value["sequence"].as_u64())
            .collect();
    }
    assert!(
        sequences.iter().all(|sequence| *sequence > 0),
        "live events must be stamped, got {sequences:?}"
    );
    let mut sorted = sequences.clone();
    sorted.sort_unstable();
    sorted.dedup();
    assert_eq!(
        sorted.len(),
        sequences.len(),
        "wire sequences must be distinct: {sequences:?}"
    );
    assert_eq!(
        sorted, sequences,
        "wire sequences must arrive in increasing order: {sequences:?}"
    );

    // The durable row persists its stamped value, so a client cursor from a
    // live event resumes the durable log coherently.
    tokio::time::sleep(Duration::from_millis(200)).await;
    let durable_max = state.inner.store.latest_event_sequence().await.unwrap();
    assert!(
        sequences.contains(&durable_max),
        "durable seq {durable_max} should be one of the broadcast sequences {sequences:?}"
    );

    state.shutdown().await.unwrap();
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_event_stream_flushes_live_deltas_before_durable_replay() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-live-before-durable-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let session = store_test_session(&session_id, now_millis());
    state.inner.store.insert_session(&session).await.unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v2/events?sessionId={}&tail=true&limit=10",
                    session_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    // A step's token deltas are published live before the full-part snapshot
    // containing them is committed. Queue the deltas, then land the durable
    // snapshot BEFORE the stream is first polled: the snapshot must not be
    // replayed ahead of the queued deltas, or clients append the tail twice.
    for delta in ["tail-delta-one", "tail-delta-two"] {
        state.publish_live(EventPayload::new(
            event_type::MESSAGE_PART_DELTA,
            json!({
                "sessionID": session_id,
                "messageID": "message-live",
                "partID": "part-live",
                "field": "text",
                "delta": delta
            }),
        ));
    }
    let sequence_before = state
        .inner
        .store
        .latest_event_sequence()
        .await
        .unwrap_or_default();
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({
            "sessionID": session_id,
            "part": {
                "id": "part-live",
                "type": "text",
                "messageID": "message-live",
                "text": "full-snapshot-text"
            }
        }),
    ));
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let sequence = state
                .inner
                .store
                .latest_event_sequence()
                .await
                .unwrap_or_default();
            if sequence > sequence_before {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("durable snapshot should commit");

    let mut received = String::new();
    while !(received.contains("full-snapshot-text")
        && received.contains("tail-delta-two"))
    {
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("stream should deliver deltas and snapshot")
            .expect("SSE body should remain open")
            .expect("SSE body chunk should be readable");
        received.push_str(&String::from_utf8_lossy(&chunk));
    }
    let first_delta = received.find("tail-delta-one").unwrap();
    let second_delta = received.find("tail-delta-two").unwrap();
    let snapshot = received.find("full-snapshot-text").unwrap();
    assert!(
        first_delta < snapshot && second_delta < snapshot,
        "live deltas must precede the durable snapshot that contains them: {received}"
    );

    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_event_stream_keeps_publish_order_for_committed_parts_before_deltas() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-publish-order-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let session = store_test_session(&session_id, now_millis());
    state.inner.store.insert_session(&session).await.unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v2/events?sessionId={}&tail=true&limit=10",
                    session_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let mut body = response.into_body().into_data_stream();

    // A reasoning part opens (committed snapshot) BEFORE the answer's token
    // deltas. The subscriber must see them in publish order — the snapshot
    // lagging behind later deltas rendered thinking cards BELOW the streaming
    // answer until the next full refresh reordered them.
    state.publish(EventPayload::new(
        event_type::MESSAGE_PART_UPDATED,
        json!({
            "sessionID": session_id,
            "part": {
                "id": "part-reasoning",
                "type": "reasoning",
                "messageID": "message-live",
                "text": "reasoning-opens-first"
            }
        }),
    ));
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": session_id,
            "messageID": "message-live",
            "partID": "part-text",
            "field": "text",
            "delta": "answer-token-after"
        }),
    ));

    let mut received = String::new();
    while !(received.contains("reasoning-opens-first")
        && received.contains("answer-token-after"))
    {
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("stream should deliver snapshot and delta")
            .expect("SSE body should remain open")
            .expect("SSE body chunk should be readable");
        received.push_str(&String::from_utf8_lossy(&chunk));
    }
    let snapshot = received.find("reasoning-opens-first").unwrap();
    let delta = received.find("answer-token-after").unwrap();
    assert!(
        snapshot < delta,
        "committed part snapshot must precede deltas published after it: {received}"
    );

    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_root_stream_delivers_child_deletion_without_unrelated_leak() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-family-delete-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let unrelated_id = neoism_agent_core::new_session_id();
    let root = store_test_session(&root_id, now_millis());
    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(root_id.clone());
    let unrelated = store_test_session(&unrelated_id, now_millis());
    state.inner.store.insert_session(&root).await.unwrap();
    state.inner.store.insert_session(&child).await.unwrap();
    state.inner.store.insert_session(&unrelated).await.unwrap();
    state.inner.store
        .admit_execution_activity(root_id.as_str(), "execution-delete-sse", "message", "")
        .await.unwrap().unwrap();
    state.inner.store.register_execution_subtask(
        "execution-delete-sse", root_id.as_str(), root_id.as_str(), child_id.as_str(), 10,
    ).await.unwrap();

    let response = app(state.clone()).oneshot(
        Request::builder()
            .method(Method::GET)
            .uri(format!("/v2/events?sessionId={}&tail=true&limit=10", root_id))
            .body(Body::empty())
            .unwrap(),
    ).await.unwrap();
    let mut body = response.into_body().into_data_stream();

    for deleted in [&unrelated_id, &child_id] {
        let response = app(state.clone()).oneshot(
            Request::builder()
                .method(Method::DELETE)
                .uri(format!("/v2/sessions/{deleted}"))
                .body(Body::empty())
                .unwrap(),
        ).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    let mut received = String::new();
    while !received.contains("session.deleted") {
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("child deletion should reach root SSE stream")
            .expect("SSE body should remain open")
            .expect("SSE body chunk should be readable");
        received.push_str(&String::from_utf8_lossy(&chunk));
    }
    let text = received;
    assert!(text.contains("session.deleted"), "{text}");
    assert!(text.contains(child_id.as_str()), "{text}");
    assert!(!text.contains(unrelated_id.as_str()), "{text}");

    let runtime = state.inner.store
        .get_session_runtime_snapshot(root_id.as_str())
        .await.unwrap();
    assert!(runtime.branches.is_empty(), "reconnect hydration must omit deleted child");
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_child_event_stream_receives_root_and_sibling_execution_family_events() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-child-family-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let sibling_id = neoism_agent_core::new_session_id();
    let root = store_test_session(&root_id, now_millis());
    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(root_id.clone());
    let mut sibling = store_test_session(&sibling_id, now_millis());
    sibling.parent_id = Some(root_id.clone());
    state.inner.store.insert_session(&root).await.unwrap();
    state.inner.store.insert_session(&child).await.unwrap();
    state.inner.store.insert_session(&sibling).await.unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!(
                    "/v2/events?sessionId={}&tail=true&limit=1",
                    child_id
                ))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    state.publish_live(EventPayload::new(
        event_type::SESSION_EXECUTION_UPDATED,
        json!({ "sessionID": root_id, "snapshot": { "executionId": "execution-a" } }),
    ));
    state.publish_live(EventPayload::new(
        event_type::MESSAGE_PART_DELTA,
        json!({
            "sessionID": sibling_id,
            "messageID": "message-sibling",
            "partID": "part-sibling",
            "field": "text",
            "delta": "sibling-token"
        }),
    ));

    let mut received = String::new();
    while !(received.contains("session.execution.updated")
        && received.contains("sibling-token"))
    {
        let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
            .await
            .expect("root and sibling events should reach child SSE stream")
            .expect("SSE body should remain open")
            .expect("SSE body chunk should be readable");
        received.push_str(&String::from_utf8_lossy(&chunk));
    }
    assert!(received.contains(root_id.as_str()), "{received}");
    assert!(received.contains(sibling_id.as_str()), "{received}");
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn v2_root_event_stream_replays_durable_child_events_only() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-family-replay-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let root_id = neoism_agent_core::new_session_id();
    let child_id = neoism_agent_core::new_session_id();
    let unrelated_id = neoism_agent_core::new_session_id();
    let root = store_test_session(&root_id, now_millis());
    let mut child = store_test_session(&child_id, now_millis());
    child.parent_id = Some(root_id.clone());
    let unrelated = store_test_session(&unrelated_id, now_millis());
    state.inner.store.insert_session(&root).await.unwrap();
    state.inner.store.insert_session(&child).await.unwrap();
    state.inner.store.insert_session(&unrelated).await.unwrap();
    state
        .publish_persisted(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({
                "sessionID": unrelated_id,
                "part": { "id": "unrelated-part", "text": "must-not-replay" }
            }),
        ))
        .await
        .unwrap();
    state
        .publish_persisted(EventPayload::new(
            event_type::MESSAGE_PART_UPDATED,
            json!({
                "sessionID": child_id,
                "part": { "id": "child-part", "text": "durable-child-update" }
            }),
        ))
        .await
        .unwrap();

    let response = app(state.clone())
        .oneshot(
            Request::builder()
                .method(Method::GET)
                .uri(format!("/v2/events?sessionId={}&since=0&limit=1", root_id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body().into_data_stream();
    let chunk = tokio::time::timeout(Duration::from_secs(2), body.next())
        .await
        .expect("durable child event should replay")
        .expect("SSE body should remain open")
        .expect("SSE body chunk should be readable");
    let text = String::from_utf8_lossy(&chunk);
    assert!(text.contains("durable-child-update"), "{text}");
    assert!(text.contains(child_id.as_str()), "{text}");
    assert!(!text.contains("must-not-replay"), "{text}");

    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn concurrent_event_commits_keep_one_gapless_aggregate_sequence() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-atomic-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id().to_string();
    let commits = (0..24).map(|index| {
        let state = state.clone();
        let session_id = session_id.clone();
        tokio::spawn(async move {
            state
                .publish_persisted(EventPayload::new(
                    event_type::SESSION_STATUS,
                    json!({ "sessionID": session_id, "index": index }),
                ))
                .await
        })
    });
    for result in futures::future::join_all(commits).await {
        result.unwrap().unwrap();
    }
    let events = state
        .inner
        .store
        .list_events_after(0, 100, Some(&session_id))
        .await
        .unwrap();
    assert_eq!(events.len(), 24);
    assert!(events.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn turso_transactions_retry_while_another_store_is_writing() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-busy-retry-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    // Two separately opened stores deliberately do not share the in-process
    // write gate. This reproduces a debug/production or multi-process writer
    // holding the database while the durable event transaction begins.
    let event_store = SessionStore::open(path.clone()).await.unwrap();
    let projection_store = SessionStore::open(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let session = store_test_session(&session_id, now_millis());
    projection_store.insert_session(&session).await.unwrap();

    let writes = 64;
    let mut tasks = Vec::with_capacity(writes * 2);
    for index in 0..writes {
        let store = event_store.clone();
        let session_id = session_id.clone();
        tasks.push(tokio::spawn(async move {
            store
                .append_event(&EventPayload::new(
                    event_type::SESSION_STATUS,
                    json!({ "sessionID": session_id, "index": index }),
                ))
                .await
        }));

        let store = projection_store.clone();
        let mut session = session.clone();
        tasks.push(tokio::spawn(async move {
            session.time.updated += index as u64 + 1;
            store.update_session(&session).await
        }));
    }
    for result in futures::future::join_all(tasks).await {
        result.unwrap().unwrap();
    }

    let events = event_store
        .list_events_after(0, writes, Some(session_id.as_str()))
        .await
        .unwrap();
    assert_eq!(events.len(), writes);
    assert!(events.windows(2).all(|pair| pair[1].seq == pair[0].seq + 1));

    drop(projection_store);
    drop(event_store);
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn context_epochs_survive_restart_and_advance_on_instruction_change() {
    let root = std::env::temp_dir().join(format!(
        "neoism-context-epoch-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("AGENTS.md"), "first instructions\n").unwrap();
    let path = root.join("agent.sqlite3");
    let state = AppState::open_database(path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let mut session = store_test_session(&session_id, now_millis());
    session.directory = root.to_string_lossy().to_string();
    state.inner.store.insert_session(&session).await.unwrap();
    let first = context_epoch::reconcile(&state, &mut session)
        .await
        .unwrap();
    assert_eq!(first.generation, 1);
    drop(state);

    std::fs::write(root.join("AGENTS.md"), "second instructions\n").unwrap();
    let state = AppState::open_database(path.clone()).await.unwrap();
    let mut session = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .unwrap();
    let second = context_epoch::reconcile(&state, &mut session)
        .await
        .unwrap();
    assert_eq!(second.generation, 2);
    assert_eq!(second.baseline, first.baseline);
    assert_ne!(second.snapshot, first.snapshot);
    drop(state);
    cleanup_sqlite_files(&path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn public_api_is_v2_only() {
    std::env::set_var("NEOISM_AGENT_DISABLE_MODELS_FETCH", "true");
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-routes-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    for (method, path) in [
        (Method::GET, "/v2/health"),
        (Method::GET, "/v2/config"),
        (Method::GET, "/v2/providers/configured"),
        (Method::GET, "/v2/providers"),
        (Method::GET, "/v2/providers/auth-methods"),
        (Method::GET, "/v2/agents"),
        (Method::GET, "/v2/agents/build"),
        (Method::GET, "/v2/skills"),
        (Method::GET, "/v2/plugins/dev.neoism.workflows"),
        (Method::GET, "/v2/plugins/dev.neoism.lsp"),
        (Method::GET, "/v2/interactions/permissions"),
        (Method::GET, "/v2/interactions/questions"),
        (Method::GET, "/v2/plugins/dev.neoism.pty/shells"),
        (Method::GET, "/v2/plugins/dev.neoism.mcp"),
        (Method::GET, "/v2/sessions"),
        (Method::GET, "/v2/sessions/status"),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method(method)
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{path}");
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{path}");
    }
    let manifest: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                "/v2/plugins/dev.neoism.mcp/manifest",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(manifest["id"], "dev.neoism.mcp");
    let mcp_root: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                "/v2/plugins/dev.neoism.mcp",
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(mcp_root.get("id"), Some(&json!("dev.neoism.mcp")));
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            "/v2/plugins/dev.neoism.mcp",
            Some(json!({})),
        ))
        .await
        .unwrap();
    assert_ne!(response.status(), StatusCode::NOT_FOUND);
    assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED);
    for path in [
        "/global/health",
        "/session",
        "/api/session",
        "/provider",
        "/permission",
        "/mcp",
        "/lsp",
    ] {
        let response = app
            .clone()
            .oneshot(request(Method::GET, path, None))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "{path}");
    }
    let response = app
        .oneshot(request(
            Method::GET,
            "/v2/sessions/ses_missing",
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let error: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        error.as_object().unwrap().keys().cloned().collect::<BTreeSet<_>>(),
        BTreeSet::from([
            "code".to_string(),
            "details".to_string(),
            "message".to_string(),
            "retryable".to_string(),
        ])
    );
    std::env::remove_var("NEOISM_AGENT_DISABLE_MODELS_FETCH");
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn provider_auth_routes_persist_api_credentials() {
    let _guard = env_lock();
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-auth-route-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    let auth_path = root.join("auth.json");
    let models_path = root.join("models.json");
    std::fs::write(&models_path, test_models_catalog()).unwrap();

    std::env::set_var("NEOISM_AGENT_MODELS_PATH", &models_path);
    std::env::set_var("NEOISM_AGENT_AUTH_PATH", &auth_path);
    std::env::set_var("NEOISM_AGENT_DISABLE_MODELS_FETCH", "true");
    std::env::remove_var("NEOISM_AGENT_AUTH_CONTENT");
    std::env::remove_var("NEOISM_TEST_PROVIDER_KEY_DO_NOT_SET");

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());

    let methods: BTreeMap<String, Vec<neoism_agent_core::ProviderAuthMethod>> =
        response_json(
            app.clone()
                .oneshot(request(Method::GET, "/v2/providers/auth-methods", None))
                .await
                .unwrap(),
        )
        .await;
    let test_methods = methods.get("test-provider").unwrap();
    assert!(matches!(
        test_methods[0].kind,
        neoism_agent_core::ProviderAuthMethodKind::Api
    ));

    let authorization: Option<Value> = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/v2/providers/test-provider/oauth/authorize",
                Some(json!({
                    "method": 0,
                    "inputs": {
                        "key": "stored-key",
                        "accountId": "acct"
                    }
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(authorization.is_none());

    let stored: Option<AuthInfo> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/v2/providers/test-provider/auth", None))
            .await
            .unwrap(),
    )
    .await;
    match stored.unwrap() {
        AuthInfo::Api { key, metadata } => {
            assert_eq!(key, "stored-key");
            assert_eq!(metadata, Some(json!({ "accountId": "acct" })));
        }
        _ => panic!("expected stored API credentials"),
    }

    let providers: ProviderListResult = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/v2/providers", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(providers
        .connected
        .iter()
        .any(|provider| provider == "test-provider"));

    let removed: bool = response_json(
        app.clone()
            .oneshot(request(Method::DELETE, "/v2/providers/test-provider/auth", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(removed);

    let stored: Option<AuthInfo> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/v2/providers/test-provider/auth", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(stored.is_none());

    std::env::remove_var("NEOISM_AGENT_MODELS_PATH");
    std::env::remove_var("NEOISM_AGENT_AUTH_PATH");
    std::env::remove_var("NEOISM_AGENT_DISABLE_MODELS_FETCH");
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn prompt_persists_streamed_assistant_message() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-stream-route-{}",
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

    let assistant = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({ "parts": [{ "type": "text", "text": "stream this" }] }),
    )
    .await;

    let MessageInfo::Assistant(info) = &assistant.info else {
        panic!("expected assistant message")
    };
    assert_eq!(info.provider_id, "neoism");
    assert_eq!(info.finish.as_deref(), Some("stop"));
    assert!(info.time.completed.is_some());
    assert!(info.tokens.output > 0);
    assert_eq!(assistant.parts.len(), 3);
    assert!(matches!(assistant.parts[0], Part::StepStart(_)));
    assert!(matches!(assistant.parts[2], Part::StepFinish(_)));
    let Part::Text(text) = &assistant.parts[1] else {
        panic!("expected text part")
    };
    assert!(text.text.contains("stream this"));
    assert!(text.time.as_ref().and_then(|time| time.end).is_some());

    let message_page: neoism_agent_core::Page<MessageWithParts> = response_json(
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
    let messages = message_page.items;
    assert_eq!(messages.len(), 2);
    assert_eq!(
        serde_json::to_value(&messages[1].parts).unwrap(),
        serde_json::to_value(&assistant.parts).unwrap()
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[cfg(any())]
struct TestRuntimeHook;

#[cfg(any())]
impl plugin::RuntimeHook for TestRuntimeHook {
    fn name(&self) -> &str {
        "test-native"
    }

    fn chat_messages_transform(
        &self,
        ctx: &plugin::ChatHookContext,
        messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        if ctx.provider_id == "neoism" && ctx.model_id == "stub" && ctx.agent == "build" {
            if let Some(message) = messages
                .iter_mut()
                .rev()
                .find(|message| matches!(message.role, ProviderRole::User))
            {
                message.content.push_str(" transformed-by-plugin");
            }
        }
        Ok(())
    }

    fn tool_definition(
        &self,
        ctx: &plugin::ToolDefinitionContext,
        tool: &mut ToolListItem,
    ) -> anyhow::Result<()> {
        if ctx.tool_id == "read" {
            tool.description.push_str(" [plugin]");
        }
        Ok(())
    }

    fn tool_execute_before(
        &self,
        ctx: &plugin::ToolExecutionContext,
        args: &mut Value,
    ) -> anyhow::Result<()> {
        if ctx.tool_id == "read" {
            *args = json!({ "filePath": "input.txt" });
        }
        Ok(())
    }

    fn tool_execute_after(
        &self,
        ctx: &plugin::ToolExecutionContext,
        result: &mut tool::ToolExecutionResult,
    ) -> anyhow::Result<()> {
        if ctx.tool_id == "read" {
            result.title.push_str(" [plugin]");
            result.metadata = Some(json!({ "plugin": "test-native" }));
        }
        Ok(())
    }
}

#[tokio::test]
#[cfg(any())]
async fn native_plugin_hooks_can_shape_tools_and_chat_context() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-native-plugin-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(root.join("input.txt"), "plugin selected this file").unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{ "permission": { "read": "allow" } }"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let snapshot = state.plugin_snapshot(root.to_string_lossy().as_ref()).await;
    let app = app(state.clone());

    let tools: Vec<ToolListItem> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/tools?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(tools
        .iter()
        .any(|tool| tool.id == "read" && tool.description.contains("[plugin]")));

    let tool_context = plugin::ToolExecutionContext {
        tool_id: "read".to_string(),
        directory: root.to_string_lossy().to_string(),
        session_id: Some("ses_test".to_string()),
        message_id: Some("msg_test".to_string()),
        call_id: Some("call_test".to_string()),
    };
    let mut arguments = json!({ "filePath": "missing.txt" });
    plugin::tool_execute_before(&snapshot, &tool_context, &mut arguments).unwrap();
    let mut result = tool::execute(
        "read",
        tool::ToolContext::new(&root)
            .with_state(Some(state.clone()))
            .with_permission_rules(vec![neoism_agent_core::PermissionRule {
                permission: "read".into(), pattern: "*".into(), action: neoism_agent_core::PermissionAction::Allow,
            }]),
        arguments,
    )
    .await
    .unwrap();
    plugin::tool_execute_after(&snapshot, &tool_context, &mut result).unwrap();
    assert!(result.output.contains("plugin selected this file"));
    assert!(result.title.contains("[plugin]"));
    assert_eq!(result.metadata.unwrap()["plugin"], "test-native");

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
    let assistant = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({ "parts": [{ "type": "text", "text": "hello plugin" }] }),
    )
    .await;
    let Part::Text(text) = &assistant.parts[1] else {
        panic!("expected text part")
    };
    assert!(text.text.contains("hello plugin transformed-by-plugin"));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn loop_sends_tool_result_back_to_provider() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tool-loop-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("input.txt"), "tool loop content").unwrap();
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

    let assistant = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({
            "model": { "providerId": "neoism", "modelId": "stub" },
            "parts": [{ "type": "text", "text": "read-tool: input.txt" }]
        }),
    )
    .await;
    let MessageInfo::Assistant(info) = &assistant.info else {
        panic!("expected final assistant message")
    };
    assert_eq!(info.finish.as_deref(), Some("stop"));
    let Part::Text(text) = &assistant.parts[1] else {
        panic!("expected final text part")
    };
    assert!(text.text.contains("Tool result received"));
    assert!(text.text.contains("1: tool loop content"));

    let message_page: neoism_agent_core::Page<MessageWithParts> = response_json(
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
    let messages = message_page.items;
    assert_eq!(messages.len(), 3);
    assert!(messages[1].parts.iter().any(|part| matches!(
        part,
        Part::Tool(ToolPart {
            state: ToolState::Completed { output, .. },
            ..
        }) if output.contains("1: tool loop content")
    )));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn loop_continues_until_tool_calls_stop() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-tool-loop-chain-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("first.txt"), "first result").unwrap();
    std::fs::write(root.join("second.txt"), "second result").unwrap();
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

    let assistant = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({
            "model": { "providerId": "neoism", "modelId": "stub" },
            "parts": [{ "type": "text", "text": "read-tool-chain: first.txt, second.txt" }]
        }),
    )
    .await;
    let MessageInfo::Assistant(info) = &assistant.info else {
        panic!("expected final assistant message")
    };
    assert_eq!(info.finish.as_deref(), Some("stop"));
    let Part::Text(text) = &assistant.parts[1] else {
        panic!("expected final text part")
    };
    assert!(text.text.contains("second result"));

    let message_page: neoism_agent_core::Page<MessageWithParts> = response_json(
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
    let messages = message_page.items;
    assert_eq!(messages.len(), 4);
    let completed_read_tools = messages
        .iter()
        .flat_map(|message| &message.parts)
        .filter(|part| {
            matches!(
                part,
                Part::Tool(ToolPart {
                    tool,
                    state: ToolState::Completed { .. },
                    ..
                }) if tool == "read"
            )
        })
        .count();
    assert_eq!(completed_read_tools, 2);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn loop_executes_same_step_parallel_tool_calls() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-parallel-tool-loop-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("first.txt"), "first parallel result").unwrap();
    std::fs::write(root.join("second.txt"), "second parallel result").unwrap();
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

    let _assistant = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({
            "model": { "providerId": "neoism", "modelId": "stub" },
            "parts": [{ "type": "text", "text": "parallel-read-tools: first.txt, second.txt" }]
        }),
    )
    .await;

    let message_page: neoism_agent_core::Page<MessageWithParts> = response_json(
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
    let messages = message_page.items;
    let tool_outputs = messages[1]
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Tool(ToolPart {
                state: ToolState::Completed { output, .. },
                ..
            }) => Some(output.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(tool_outputs.len(), 2);
    assert!(tool_outputs
        .iter()
        .any(|output| output.contains("first parallel result")));
    assert!(tool_outputs
        .iter()
        .any(|output| output.contains("second parallel result")));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn duplicate_tool_call_event_executes_once() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-duplicate-tool-call-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("file.md"), "# Smoke\n\nInitial line.\n").unwrap();
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

    let _ = v2_prompt_and_wait(
        &app,
        &session.id,
        json!({
            "model": { "providerId": "neoism", "modelId": "gpt-5.5" },
            "parts": [{ "type": "text", "text": "duplicate-patch-tool: file.md" }]
        }),
    )
    .await;

    let contents = std::fs::read_to_string(root.join("file.md")).unwrap();
    assert_eq!(contents.matches("duplicate patch guard line").count(), 1);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn runtime_source_plugins_are_workspace_disableable() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-skills-disabled-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{ "plugins": {
            "dev.neoism.skills": { "enabled": false },
            "dev.neoism.commands": { "enabled": false },
            "dev.neoism.websearch": { "enabled": false },
            "dev.neoism.agents": { "enabled": false },
            "dev.neoism.mcp": { "enabled": false },
            "dev.neoism.lsp": { "enabled": false },
            "dev.neoism.workflows": { "enabled": false },
            "dev.neoism.tools.notes": { "enabled": false },
            "dev.neoism.tools.workspace": { "enabled": false },
            "dev.neoism.semantic": { "enabled": false },
            "dev.neoism.goals": { "enabled": false },
            "dev.neoism.artifacts": { "enabled": false },
            "dev.neoism.interactions": { "enabled": false },
            "dev.neoism.subagents": { "enabled": false },
            "dev.neoism.vcs": { "enabled": false },
            "dev.neoism.pty": { "enabled": false }
        } }"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let directory = root.to_string_lossy();

    let capabilities: Vec<neoism_agent_core::CapabilityInfo> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/capabilities?directory={directory}"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    let snapshot = state.plugin_snapshot(&directory).await;
    let disabled_ids = [
        "dev.neoism.skills", "dev.neoism.commands", "dev.neoism.websearch",
        "dev.neoism.agents", "dev.neoism.mcp", "dev.neoism.lsp",
        "dev.neoism.workflows", "dev.neoism.tools.notes",
        "dev.neoism.tools.workspace", "dev.neoism.semantic", "dev.neoism.goals",
        "dev.neoism.subagents", "dev.neoism.vcs", "dev.neoism.pty",
        "dev.neoism.artifacts", "dev.neoism.interactions",
    ];
    assert!(snapshot.manifests.iter().all(|manifest| !disabled_ids.contains(&manifest.id.as_str())));
    assert!(snapshot.capabilities.iter().all(|capability| capability.plugin_id.as_deref().is_none_or(|id| !disabled_ids.contains(&id))));
    assert!(snapshot.contributions.values().all(|contribution| !disabled_ids.contains(&contribution.plugin_id.as_str())));
    assert!(snapshot.runtime_tools.is_empty());

    let response = app.clone().oneshot(request(
            Method::GET,
            &format!("/v2/skills?directory={directory}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    let tools: Vec<neoism_agent_core::ToolListItem> = response_json(
        app.clone().oneshot(request(
            Method::GET,
            &format!("/v2/tools?directory={directory}"),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;
    assert!(!tools.iter().any(|tool| tool.id == "websearch"));
    assert!(!tools.iter().any(|tool| tool.id == "lsp"));
    assert!(!tools.iter().any(|tool| tool.id == "notes"));
    assert!(!capabilities.iter().any(|capability| capability.id == "neoism.tools.notes"));
    assert!(!tools.iter().any(|tool| tool.id == "skill"));
    assert!(!tools.iter().any(|tool| tool.id == "read"));
    assert!(!tools.iter().any(|tool| tool.id == "complete_goal"));
    let response = app
        .clone()
        .oneshot(request(
            Method::GET,
            &format!("/v2/agents?directory={directory}"),
            None,
        ))
        .await
        .unwrap();
    assert!(!response.status().is_success());
    for (plugin_id, path) in [
        (
            "dev.neoism.goals",
            "/v2/plugins/dev.neoism.goals/session-test",
        ),
        (
            "dev.neoism.subagents",
            "/v2/plugins/dev.neoism.subagents/sessions/session-test/tasks",
        ),
        ("dev.neoism.semantic", "/v2/plugins/dev.neoism.semantic/search"),
        ("dev.neoism.workflows", "/v2/plugins/dev.neoism.workflows"),
        ("dev.neoism.lsp", "/v2/plugins/dev.neoism.lsp"),
        ("dev.neoism.mcp", "/v2/plugins/dev.neoism.mcp"),
        ("dev.neoism.vcs", "/v2/plugins/dev.neoism.vcs"),
        ("dev.neoism.pty", "/v2/plugins/dev.neoism.pty/shells"),
    ] {
        assert!(!snapshot.manifests.iter().any(|manifest| manifest.id == plugin_id));
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!("{path}?directory={directory}"),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
    let response = app.oneshot(request(
            Method::GET,
            &format!("/v2/commands?directory={directory}"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    state.inner.store.close().await;
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn disabled_execution_contributions_have_no_side_effects() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-disabled-execution-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(root.join(".agent/tools")).unwrap();
    let marker = root.join("spawned");
    std::fs::write(
        root.join(".agent/agent.json"),
        format!(
            r#"{{
                "plugins": {{
                    "dev.neoism.tools.workspace": {{ "enabled": false }},
                    "dev.neoism.subagents": {{ "enabled": false }},
                    "dev.neoism.mcp": {{ "enabled": false }},
                    "dev.neoism.goals": {{ "enabled": false }}
                }},
                "mcp": {{ "disabled": {{ "type": "local", "command": ["sh", "-c", "touch '{}'"] }} }}
            }}"#,
            marker.display()
        ),
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/tools/custom_spawn.json"),
        format!(
            r#"{{ "command": ["sh", "-c", "touch '{}'"], "parameters": {{ "type": "object" }} }}"#,
            marker.display()
        ),
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let mut session = store_test_session(&session_id, now_millis());
    session.directory = root.to_string_lossy().into_owned();
    state.inner.store.insert_session(&session).await.unwrap();
    let message_id = Id::ascending(IdKind::Message);
    let permissions = vec![neoism_agent_core::PermissionRule {
        permission: "*".to_string(),
        pattern: "*".to_string(),
        action: neoism_agent_core::PermissionAction::Allow,
    }];

    for (name, input) in [
        ("bash", json!({ "command": format!("touch {}", marker.display()) })),
        ("background_task", json!({ "command": format!("touch {}", marker.display()), "description": "disabled" })),
        ("session_search", json!({ "query": "anything" })),
        ("task", json!({ "description": "disabled", "prompt": "disabled", "subagent_type": "general" })),
        ("execute", json!({ "action": "search" })),
        ("custom_spawn", json!({})),
    ] {
        let result = execute_tool_call_with_permission_wait(
            &state,
            &session_id,
            &message_id,
            root.to_string_lossy().as_ref(),
            permissions.clone(),
            "call-disabled",
            name,
            input,
        )
        .await;
        assert!(result.is_err(), "disabled {name} unexpectedly executed");
    }

    let workspace = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
    assert!(!workspace.mcp_is_allocated(), "disabled MCP allocated a runtime");
    assert!(workspace.background_if_allocated().is_none(), "disabled background tool allocated state");
    assert!(!marker.exists(), "a disabled shell, MCP, background, or custom tool spawned");
    assert_eq!(state.inner.store.list_sessions().await.unwrap().len(), 1, "disabled task created a child session");

    let subtask = PromptRequest {
        message_id: None,
        model: None,
        agent: None,
        no_reply: false,
        system: None,
        tools: None,
        author: None,
        parts: vec![PromptPart::Subtask {
            prompt: "disabled".to_string(),
            description: "disabled".to_string(),
            agent: "general".to_string(),
            model: None,
            command: None,
        }],
    };
    assert!(append_prompt(&state, session_id.as_str(), subtask, true).await.is_err());
    assert_eq!(state.inner.store.list_sessions().await.unwrap().len(), 1);

    state.inner.store.close().await;
    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn declarative_plugins_and_custom_tools_load_from_config_dirs() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-dynamic-plugin-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent/plugins")).unwrap();
    std::fs::create_dir_all(root.join(".agent/tools")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{ "permission": { "bash": "allow" } }"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/plugins/test-plugin.json"),
        r#"{
          "id": "dev.example.test-plugin",
          "chatHeaders": { "X-Test-Plugin": "yes" },
          "chatOptions": { "metadata": { "plugin": true } },
          "shellEnv": { "PLUGIN_ENV": "loaded" }
        }"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/tools/custom_echo.json"),
        r#"{
          "description": "Echoes a custom tool argument",
          "command": ["bash", "-lc", "printf '%s:%s' \"$PLUGIN_ENV\" \"$NEOISM_ARG_TEXT\""],
          "parameters": {
            "type": "object",
            "properties": { "text": { "type": "string" } },
            "required": ["text"]
          }
        }"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let runtime = state.workspace_runtime(root.to_string_lossy().as_ref()).await.unwrap();
    let app = app(state.clone());
    let hook_ctx = plugin::ChatHookContext {
        session_id: "ses_test".to_string(),
        agent: "build".to_string(),
        provider_id: "openai".to_string(),
        model_id: "gpt-test".to_string(),
    };
    let mut headers = std::collections::BTreeMap::new();
    plugin::chat_headers(&runtime.snapshot(), &hook_ctx, &mut headers).unwrap();
    assert_eq!(headers["X-Test-Plugin"], "yes");
    let mut options = std::collections::BTreeMap::new();
    plugin::chat_options(&runtime.snapshot(), &hook_ctx, &mut options).unwrap();
    assert_eq!(options["metadata"]["plugin"], true);

    let tools: Vec<ToolListItem> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/v2/tools?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(tools.iter().any(|tool| tool.id == "custom_echo"));

    let mut custom_env = std::collections::BTreeMap::new();
    plugin::shell_env(&runtime.snapshot(), &plugin::ShellEnvContext {
        cwd: root.to_string_lossy().into_owned(), session_id: None, call_id: None,
    }, &mut custom_env).unwrap();
    let result = custom_tool::execute(
        state.services(), root.to_string_lossy().as_ref(), "custom_echo",
        json!({ "text": "hello" }), &[neoism_agent_core::PermissionRule {
            permission: "bash".into(), pattern: "*".into(), action: neoism_agent_core::PermissionAction::Allow,
        }], custom_env, None,
    ).await.unwrap().unwrap();
    assert_eq!(result.output, "loaded:hello");
    assert_eq!(result.metadata.unwrap()["customTool"], true);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn generic_config_reads_only_canonical_agent_json() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-mcp-file-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    // Product-specific/compatibility names are deliberately invisible to
    // standalone Agent.
    std::fs::write(
        root.join("config.json"),
        r#"{ "mcp": { "alpha": { "type": "local", "command": ["alpha-old"] } } }"#,
    )
    .unwrap();
    std::fs::write(
        root.join("ignored-product-config.json"),
        r#"{"mcp":{"beta":{"type":"local","command":["ignored"]}}}"#,
    )
    .unwrap();

    let user_root = root.join("user");
    let services = neoism_agent_service_api::AgentServices::new(std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService), crate::standard_workspace_search())
        .with_config(std::sync::Arc::new(neoism_agent_service_api::StandardConfigSourceService::new(&user_root)));
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{"mcp":{"gamma":{"type":"local","command":["gamma-agent"]}}}"#,
    )
    .unwrap();
    let loaded = config::load(&services, root.to_str().unwrap()).unwrap();
    assert!(loaded.info.mcp.contains_key("gamma"));
    assert!(!loaded.info.mcp.contains_key("alpha"));
    assert!(!loaded.info.mcp.contains_key("beta"));
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn config_loads_project_agents_commands_and_permissions() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-config-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".agent/agents")).unwrap();
    std::fs::create_dir_all(root.join(".agent/modes")).unwrap();
    std::fs::create_dir_all(root.join(".agent/commands")).unwrap();
    std::fs::write(
        root.join(".agent/agent.json"),
        r#"{
              "defaultAgent": "plan",
              "permission": { "externalDirectory": { "*": "ask" } },
              "agent": {
                "build": {
                  "temperature": 0.2,
                  "permission": { "bash": "ask" }
                }
              }
            }"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/agents/reviewer.md"),
        r#"---
description: Reviews code changes
mode: subagent
tools:
  read: true
  write: false
permission:
  bash: deny
---
Review the change and report risks.
"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/modes/architect.md"),
        r#"---
description: Designs implementation plans
---
Design first, then hand off implementation.
"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".agent/commands/audit.md"),
        r#"---
description: Audit the current worktree
agent: reviewer
---
Audit the current worktree for correctness.
"#,
    )
    .unwrap();

    let services = neoism_agent_service_api::AgentServices::new(std::sync::Arc::new(neoism_agent_service_api::StandardExecutableService), crate::standard_workspace_search())
        .with_config(std::sync::Arc::new(neoism_agent_service_api::StandardConfigSourceService::new(root.join("user"))));
    let loaded = config::load(&services, root.to_str().unwrap()).unwrap();
    assert_eq!(loaded.info.default_agent.as_deref(), Some("plan"));
    assert!(loaded.info.agent.contains_key("reviewer"));
    assert_eq!(
        loaded.info.agent["reviewer"].permission["edit"],
        json!("deny")
    );
    assert_eq!(
        loaded.info.agent["reviewer"].permission["read"],
        json!("allow")
    );
    assert_eq!(
        loaded.info.agent["architect"].mode.as_deref(),
        Some("primary")
    );
    assert_eq!(
        loaded.info.command["audit"].agent.as_deref(),
        Some("reviewer")
    );

    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn config_validation_reports_real_setup_problems() {
    let mut config = AgentConfigDocument {
        default_agent: Some("missing".to_string()),
        enabled_providers: Some(vec!["openai".to_string()]),
        disabled_providers: vec!["openai".to_string()],
        model: Some("gpt-5.5".to_string()),
        ..AgentConfigDocument::default()
    };
    config.command.insert(
        "audit".to_string(),
        neoism_agent_core::CommandInfo {
            name: "audit".to_string(),
            description: None,
            template: None,
            agent: Some("missing".to_string()),
            model: None,
            subtask: None,
        },
    );

    let validation = config::validate_loaded(&config);

    assert!(!validation.ok);
    let messages = validation
        .diagnostics
        .iter()
        .map(|item| item.message.as_str())
        .collect::<Vec<_>>();
    assert!(messages
        .iter()
        .any(|message| message.contains("both enabled and disabled")));
    assert!(messages
        .iter()
        .any(|message| message.contains("default agent `missing`")));
    assert!(messages
        .iter()
        .any(|message| message.contains("has no provider prefix")));
    assert!(messages
        .iter()
        .any(|message| message.contains("command `audit` has no template")));
}

#[tokio::test]
async fn sessions_import_route_round_trips_a_transferred_session() {
    // Source host: persist a session, its transcript and one queued prompt,
    // then export it to a portable bundle (mirrors session_transfer's tests).
    let source_db = std::env::temp_dir().join(format!(
        "neoism-agent-import-src-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&source_db);
    let source = AppState::open_database(source_db.clone()).await.unwrap();

    let session_id = neoism_agent_core::new_session_id();
    let session = SessionInfo {
        id: session_id.clone(),
        slug: "import-route-test".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/home/alice/proj/nested".to_string(),
        path: Some("nested".to_string()),
        parent_id: None,
        title: "Portable session".to_string(),
        agent: Some("build".to_string()),
        model: None,
        version: "0.1".to_string(),
        time: TimeInfo {
            created: 10,
            updated: 20,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };
    source.inner.store.insert_session(&session).await.unwrap();

    let user_message_id = Id::ascending(IdKind::Message);
    source
        .inner
        .store
        .append_message(
            session_id.as_str(),
            &MessageWithParts {
                info: MessageInfo::User(UserMessage {
                    id: user_message_id.clone(),
                    session_id: session_id.clone(),
                    time: CreatedTime { created: 1 },
                    agent: "build".to_string(),
                    model: UserModel {
                        provider_id: "neoism".to_string(),
                        model_id: "stub".to_string(),
                        variant: None,
                    },
                    system: None,
                    tools: None,
                    author: None,
                }),
                parts: vec![Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session_id.clone(),
                    message_id: user_message_id,
                    text: "hello from host A".to_string(),
                    synthetic: None,
                    time: None,
                })],
            },
        )
        .await
        .unwrap();
    source
        .inner
        .store
        .append_message(
            session_id.as_str(),
            &MessageWithParts {
                info: MessageInfo::Assistant(AssistantMessage {
                    id: Id::ascending(IdKind::Message),
                    session_id: session_id.clone(),
                    time: CompletedTime {
                        created: 2,
                        streamed: Some(3),
                        completed: Some(3),
                    },
                    parent_id: Id::ascending(IdKind::Message),
                    mode: "build".to_string(),
                    agent: "build".to_string(),
                    path: AssistantPath {
                        cwd: "/home/alice/proj/nested".to_string(),
                        root: "/home/alice/proj".to_string(),
                    },
                    cost: 0.0,
                    tokens: TokenUsage::default(),
                    model_id: "stub".to_string(),
                    provider_id: "neoism".to_string(),
                    finish: None,
                    error: None,
                }),
                parts: Vec::new(),
            },
        )
        .await
        .unwrap();
    source
        .inner
        .store
        .enqueue_prompt_with_delivery(
            session_id.as_str(),
            &PromptRequest {
                message_id: None,
                model: None,
                agent: None,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts: vec![PromptPart::Text {
                    text: "continue please".to_string(),
                }],
            },
            "queue",
        )
        .await
        .unwrap();

    let bundle = export_session(&source, session_id.as_str()).await.unwrap();
    assert_eq!(bundle.workspace_root.as_deref(), Some("/home/alice/proj"));

    // Target host: a fresh, independent store fronted by the real router.
    let target_db = std::env::temp_dir().join(format!(
        "neoism-agent-import-dst-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&target_db);
    let target = AppState::open_database(target_db.clone()).await.unwrap();
    let app = app(target.clone());

    // Sanity: the session is absent before the import call.
    assert!(target
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .is_none());

    // Drive POST /v2/sessions/import through the router; assert 2xx + echoed id.
    let response: Value = response_json(
        app.oneshot(request(
            Method::POST,
            "/v2/sessions/import",
            Some(json!({
                "bundle": bundle,
                "targetWorkspaceRoot": "/srv/work/proj",
            })),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(response["sessionId"], session_id.as_str());

    // The session is present and rebased onto the importing host's root.
    let restored = target
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .expect("imported session present");
    assert_eq!(restored.directory, "/srv/work/proj/nested");
    assert_eq!(restored.path.as_deref(), Some("nested"));
    assert_eq!(restored.title, "Portable session");

    // Transcript preserved; assistant paths rebased onto the new root.
    let messages = target
        .inner
        .store
        .list_messages(session_id.as_str())
        .await
        .unwrap();
    assert_eq!(messages.len(), 2);
    match &messages[1].info {
        MessageInfo::Assistant(assistant) => {
            assert_eq!(assistant.path.cwd, "/srv/work/proj/nested");
            assert_eq!(assistant.path.root, "/srv/work/proj");
        }
        other => panic!("expected assistant message, got {other:?}"),
    }

    // The queued prompt survived, so the session resumes on the new host.
    let queued = target
        .inner
        .store
        .list_queued_prompt_entries(session_id.as_str())
        .await
        .unwrap();
    assert_eq!(queued.len(), 1);
    assert!(target
        .inner
        .store
        .queued_session_ids()
        .await
        .unwrap()
        .contains(&session_id.to_string()));

    source.inner.store.close().await;
    target.inner.store.close().await;
    cleanup_sqlite_files(&source_db);
    cleanup_sqlite_files(&target_db);
}

#[tokio::test]
async fn sessions_export_route_returns_only_sessions_under_requested_root() {
    // A workspace promote knows the checkout path it is moving, not the session
    // ids living there, so POST /v2/sessions/export takes a workspaceRoot and must
    // return a bundle for every session under it — and nothing else.
    let db = std::env::temp_dir().join(format!(
        "neoism-agent-export-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&db);
    let state = AppState::open_database(db.clone()).await.unwrap();

    // Two sessions under the workspace we are exporting: one at the worktree
    // root itself (no subpath) and one in a nested subdirectory.
    let root_session = export_route_test_session(
        "export-root",
        "/home/alice/proj",
        None,
        "Root session",
    );
    let nested_session = export_route_test_session(
        "export-nested",
        "/home/alice/proj/nested",
        Some("nested"),
        "Nested session",
    );
    // A session under a *different* workspace root — must be excluded.
    let other_session = export_route_test_session(
        "export-other",
        "/home/alice/other/sub",
        Some("sub"),
        "Other session",
    );
    state
        .inner
        .store
        .insert_session(&root_session)
        .await
        .unwrap();
    state
        .inner
        .store
        .insert_session(&nested_session)
        .await
        .unwrap();
    state
        .inner
        .store
        .insert_session(&other_session)
        .await
        .unwrap();

    // Drive the canonical export endpoint through the real router.
    let app = app(state.clone());
    let response: Value = response_json(
        app.oneshot(request(
            Method::POST,
            "/v2/sessions/export",
            Some(json!({ "workspaceRoot": "/home/alice/proj" })),
        ))
        .await
        .unwrap(),
    )
    .await;

    // Exactly the two matching sessions come back; the other-root one is gone.
    let bundles = response["bundles"].as_array().expect("bundles array");
    assert_eq!(bundles.len(), 2);
    let returned_ids: HashSet<String> = bundles
        .iter()
        .map(|bundle| bundle["session"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        returned_ids,
        HashSet::from([root_session.id.to_string(), nested_session.id.to_string(),])
    );
    assert!(!returned_ids.contains(&other_session.id.to_string()));

    // Each returned bundle must equal exactly what export_session yields for
    // that session (same shape the import route consumes).
    for session_id in [root_session.id.as_str(), nested_session.id.as_str()] {
        let direct = export_session(&state, session_id).await.unwrap();
        let direct_value = serde_json::to_value(&direct).unwrap();
        let from_route = bundles
            .iter()
            .find(|bundle| bundle["session"]["id"] == session_id)
            .expect("matching bundle in response");
        assert_eq!(from_route, &direct_value);
    }
    // The derived workspace root the import side rebases off of is the one we
    // asked for, for both matching sessions.
    for bundle in bundles {
        assert_eq!(bundle["workspaceRoot"], "/home/alice/proj");
    }

    state.inner.store.close().await;
    cleanup_sqlite_files(&db);
}

/// Build a minimal [`SessionInfo`] for the export-route test with a given
/// workspace `directory` and worktree-relative `path`.
fn export_route_test_session(
    slug: &str,
    directory: &str,
    path: Option<&str>,
    title: &str,
) -> SessionInfo {
    SessionInfo {
        id: neoism_agent_core::new_session_id(),
        slug: slug.to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: directory.to_string(),
        path: path.map(ToString::to_string),
        parent_id: None,
        title: title.to_string(),
        agent: Some("build".to_string()),
        model: None,
        version: "0.1".to_string(),
        time: TimeInfo {
            created: 10,
            updated: 20,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    }
}

fn cleanup_sqlite_files(path: &std::path::Path) {
    let base = path.to_string_lossy();
    let _ = std::fs::remove_file(path);
    let _ = std::fs::remove_file(format!("{base}-wal"));
    let _ = std::fs::remove_file(format!("{base}-shm"));
}

fn request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(
            body.map(|body| Body::from(body.to_string()))
                .unwrap_or_else(Body::empty),
        )
        .unwrap()
}

async fn append_snapshot_test_messages(
    state: &AppState,
    session: &SessionInfo,
    user_id: &Id,
    assistant_id: &Id,
    metadata: Value,
) {
    let now = now_millis();
    state
        .inner
        .store
        .append_message(
            session.id.as_str(),
            &MessageWithParts {
                info: MessageInfo::User(UserMessage {
                    id: user_id.clone(),
                    session_id: session.id.clone(),
                    time: CreatedTime { created: now },
                    agent: "build".to_string(),
                    model: UserModel {
                        provider_id: "neoism".to_string(),
                        model_id: "stub".to_string(),
                        variant: None,
                    },
                    system: None,
                    tools: None,
                    author: None,
                }),
                parts: vec![Part::Text(TextPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session.id.clone(),
                    message_id: user_id.clone(),
                    text: "write file".to_string(),
                    synthetic: None,
                    time: None,
                })],
            },
        )
        .await
        .unwrap();
    state
        .inner
        .store
        .append_message(
            session.id.as_str(),
            &MessageWithParts {
                info: MessageInfo::Assistant(AssistantMessage {
                    id: assistant_id.clone(),
                    session_id: session.id.clone(),
                    time: CompletedTime {
                        created: now,
                        streamed: Some(now),
                        completed: Some(now),
                    },
                    parent_id: user_id.clone(),
                    mode: "build".to_string(),
                    agent: "build".to_string(),
                    path: AssistantPath {
                        cwd: session.directory.clone(),
                        root: session.directory.clone(),
                    },
                    cost: 0.0,
                    tokens: TokenUsage::default(),
                    model_id: "stub".to_string(),
                    provider_id: "neoism".to_string(),
                    finish: Some("stop".to_string()),
                    error: None,
                }),
                parts: vec![Part::Tool(ToolPart {
                    id: Id::ascending(IdKind::Part),
                    session_id: session.id.clone(),
                    message_id: assistant_id.clone(),
                    tool: "write".to_string(),
                    call_id: "call_write_1".to_string(),
                    state: ToolState::Completed {
                        input: json!({ "filePath": "file.txt", "content": "after" }),
                        output: "wrote file".to_string(),
                        metadata,
                        title: "Write file.txt".to_string(),
                        time: PartTime {
                            start: now,
                            end: Some(now),
                        },
                    },
                    metadata: None,
                })],
            },
        )
        .await
        .unwrap();
}

#[tokio::test]
async fn v2_discovers_protocol_capabilities_and_internal_plugins() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v2-discovery-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let router = app(state.clone());

    let meta: neoism_agent_core::ApiMeta = response_json(
        router
            .clone()
            .oneshot(Request::get("/v2/meta").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(meta.api_version, neoism_agent_core::API_VERSION);

    let capabilities: Vec<neoism_agent_core::CapabilityInfo> = response_json(
        router
            .clone()
            .oneshot(
                Request::get("/v2/capabilities")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(capabilities
        .iter()
        .any(|capability| capability.id == "neoism.sessions"));
    assert!(capabilities.iter().any(|capability| {
        capability.id == "neoism.subagents"
            && capability.plugin_id.as_deref() == Some("dev.neoism.subagents")
    }));

    let plugins: Vec<neoism_agent_core::PluginManifestInfo> = response_json(
        router
            .oneshot(Request::get("/v2/plugins").body(Body::empty()).unwrap())
            .await
            .unwrap(),
    )
    .await;
    assert!(plugins
        .iter()
        .any(|plugin| plugin.id == "dev.neoism.subagents"));

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v2_openapi_describes_the_sdk_discovery_surface() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v2-openapi-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let document: serde_json::Value = response_json(
        app(state)
            .oneshot(
                Request::get("/v2/openapi.json")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap(),
    )
    .await;
    assert!(document["paths"]["/v2/events"].is_object());
    assert!(document["components"]["schemas"]["EventEnvelope"].is_object());

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn v2_artifacts_round_trip_binary_content() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-v2-artifact-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let router = app(state.clone());
    let payload = b"neoism artifact".to_vec();

    let response = router
        .clone()
        .oneshot(
            Request::post("/v2/artifacts")
                .header("content-type", "text/plain")
                .header("x-neoism-filename", "note.txt")
                .body(Body::from(payload.clone()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    let artifact: neoism_agent_core::ArtifactInfo = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(artifact.filename, "note.txt");
    assert_eq!(artifact.size, payload.len() as u64);
    assert_eq!(artifact.sha256.len(), 64);
    assert_eq!(
        state
            .inner
            .store
            .artifact_tenant(&artifact.id)
            .await
            .unwrap()
            .as_deref(),
        Some("local")
    );
    let audit = neoism_agent_core::AuditEntry {
        id: Id::ascending(IdKind::Audit).to_string(),
        tenant_id: "local".to_string(),
        subject: Some("local-operator".to_string()),
        method: "POST".to_string(),
        path: "/v2/artifacts".to_string(),
        status: StatusCode::CREATED.as_u16(),
        created: crate::now_millis(),
    };
    state.inner.store.append_audit(&audit).await.unwrap();
    let entries = state.inner.store.list_audit("local", 10).await.unwrap();
    assert_eq!(entries.first().map(|entry| entry.id.as_str()), Some(audit.id.as_str()));

    let response = router
        .clone()
        .oneshot(
            Request::get(format!("/v2/artifacts/{}/content", artifact.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let downloaded = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    assert_eq!(downloaded.as_ref(), payload.as_slice());

    let response = router
        .oneshot(
            Request::delete(format!("/v2/artifacts/{}", artifact.id))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn pending_interactions_restore_after_restart() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-interaction-restore-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let db_path = root.join("agent.sqlite3");
    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let permission = PermissionRequestInfo {
        id: Id::ascending(IdKind::Permission).to_string(),
        session_id: Id::ascending(IdKind::Session).to_string(),
        message_id: Id::ascending(IdKind::Message).to_string(),
        title: "Allow read".to_string(),
        permission: "read".to_string(),
        patterns: vec!["file.txt".to_string()],
        always: vec!["file.txt".to_string()],
        tool: None,
        metadata: None,
    };
    let question = QuestionRequestInfo {
        id: Id::ascending(IdKind::Question).to_string(),
        session_id: permission.session_id.clone(),
        message_id: permission.message_id.clone(),
        questions: vec![json!({ "question": "Continue?" })],
    };
    state
        .inner
        .store
        .save_permission_request(&permission)
        .await
        .unwrap();
    state
        .inner
        .store
        .save_question_request(&question)
        .await
        .unwrap();
    state.inner.store.close().await;
    drop(state);

    let restored = AppState::open_database(db_path.clone()).await.unwrap();
    assert!(restored
        .inner
        .permissions
        .read()
        .await
        .contains_key(&permission.id));
    assert!(restored
        .inner
        .questions
        .read()
        .await
        .contains_key(&question.id));
    assert!(restored
        .inner
        .store
        .resolve_interaction(&permission.id, "once", json!({ "reply": "once" }))
        .await
        .unwrap());
    assert!(!restored
        .inner
        .store
        .resolve_interaction(&permission.id, "once", json!({ "reply": "once" }))
        .await
        .unwrap());
    crate::interaction::cancel_session_interactions(&restored, &permission.session_id).await;
    assert!(restored.inner.permissions.read().await.is_empty());
    assert!(restored.inner.questions.read().await.is_empty());
    assert!(!restored
        .inner
        .store
        .resolve_interaction(&question.id, "answered", json!({ "answers": [] }))
        .await
        .unwrap());
    restored.inner.store.close().await;

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

async fn response_json<T: DeserializeOwned>(response: Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn v2_prompt_and_wait(
    app: &axum::Router,
    session_id: &Id,
    body: Value,
) -> MessageWithParts {
    let response = app
        .clone()
        .oneshot(request(
            Method::POST,
            &format!("/v2/sessions/{session_id}/prompt"),
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NO_CONTENT);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let page: neoism_agent_core::Page<MessageWithParts> = response_json(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    &format!("/v2/sessions/{session_id}/messages"),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        if let Some(message) = page.items.into_iter().rev().find(|message| {
            matches!(&message.info, MessageInfo::Assistant(info) if info.time.completed.is_some())
        }) {
            return message;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for V2 prompt completion"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
}

fn test_models_catalog() -> &'static str {
    r#"{
          "test-provider": {
            "id": "test-provider",
            "name": "Test Provider",
            "env": ["NEOISM_TEST_PROVIDER_KEY_DO_NOT_SET"],
            "models": {
              "test-model": {
                "id": "test-model",
                "name": "Test Model",
                "release_date": "2026-01-01",
                "limit": { "context": 128000, "output": 4096 }
              }
            }
          }
        }"#
}

#[derive(Default)]
struct RecordingWorkspaceSearch {
    label: String,
    warms: AtomicUsize,
}

struct RecordingRootPin(PathBuf);

impl neoism_agent_service_api::WorkspaceSearchRootPin for RecordingRootPin {
    fn root(&self) -> &std::path::Path { &self.0 }
}

impl neoism_agent_service_api::WorkspaceSearchService for RecordingWorkspaceSearch {
    fn warm(&self, _root: &std::path::Path) -> Result<(), neoism_agent_service_api::ServiceError> {
        self.warms.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }

    fn pin_root(&self, root: &std::path::Path) -> Result<Arc<dyn neoism_agent_service_api::WorkspaceSearchRootPin>, neoism_agent_service_api::ServiceError> {
        Ok(Arc::new(RecordingRootPin(root.to_path_buf())))
    }

    fn find_files(&self, _request: &neoism_agent_service_api::FindFilesRequest) -> Result<neoism_agent_service_api::FindFilesResult, neoism_agent_service_api::ServiceError> {
        Ok(neoism_agent_service_api::FindFilesResult {
            items: vec![neoism_agent_service_api::WorkspaceFileMatch {
                path: format!("{}.rs", self.label), score: 0, git_status: None, size: 0, modified: 0,
            }],
            bounds: Default::default(), engine: Some("fake".to_string()), fallback_reason: None,
        })
    }

    fn grep(&self, _request: &neoism_agent_service_api::GrepWorkspaceRequest) -> Result<neoism_agent_service_api::GrepWorkspaceResult, neoism_agent_service_api::ServiceError> {
        Ok(neoism_agent_service_api::GrepWorkspaceResult {
            items: Vec::new(), files_with_matches: 0, total_files_searched: 0,
            bounds: Default::default(), mode: "plain".to_string(), engine: Some("fake".to_string()), fallback_reason: None,
        })
    }

    fn search_directories(&self, _request: &neoism_agent_service_api::DirectorySearchRequest) -> Result<neoism_agent_service_api::DirectorySearchResult, neoism_agent_service_api::ServiceError> {
        Ok(neoism_agent_service_api::DirectorySearchResult {
            paths: vec![self.label.clone()], bounds: Default::default(), engine: Some("fake".to_string()),
        })
    }
}

#[tokio::test]
async fn app_states_keep_workspace_search_services_isolated() {
    let root = std::env::temp_dir().join(format!("neoism-search-isolation-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let first_search = Arc::new(RecordingWorkspaceSearch { label: "first".into(), ..Default::default() });
    let second_search = Arc::new(RecordingWorkspaceSearch { label: "second".into(), ..Default::default() });
    let first_services = neoism_agent_service_api::AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), first_search.clone());
    let second_services = neoism_agent_service_api::AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), second_search.clone());
    let first = AppState::open_database_with_services(root.join("first.db"), first_services).await.unwrap();
    let second = AppState::open_database_with_services(root.join("second.db"), second_services).await.unwrap();
    first.services().workspace_search.warm(&root).unwrap();
    let search_request = neoism_agent_service_api::FindFilesRequest {
        root: root.clone(), query: "rs".into(), offset: 0, limit: 10, control: Default::default(),
    };
    assert_eq!(first.services().workspace_search.find_files(&search_request).unwrap().items[0].path, "first.rs");
    assert_eq!(second.services().workspace_search.find_files(&search_request).unwrap().items[0].path, "second.rs");
    for (state, expected) in [(first.clone(), "first"), (second.clone(), "second")] {
        let router = app(state);
        let session: SessionInfo = response_json(router.clone().oneshot(request(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.to_string_lossy()),
            Some(json!({})),
        )).await.unwrap()).await;
        let options: Value = response_json(router.oneshot(request(
            Method::GET,
            &format!("/v2/sessions/{}/directory-options?query=x", session.id),
            None,
        )).await.unwrap()).await;
        assert!(options.as_array().unwrap().iter().any(|option| {
            option.as_str().is_some_and(|path| path.ends_with(expected))
        }));
    }
    assert_eq!(first_search.warms.load(Ordering::SeqCst), 1);
    assert_eq!(second_search.warms.load(Ordering::SeqCst), 0);
    cleanup_sqlite_files(&root.join("first.db"));
    cleanup_sqlite_files(&root.join("second.db"));
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn disabled_workspace_tools_do_not_warm_search() {
    let root = std::env::temp_dir().join(format!("neoism-search-disabled-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(root.join(".agent")).unwrap();
    std::fs::write(root.join(".agent/agent.json"), r#"{"plugins":{"dev.neoism.tools.workspace":{"enabled":false}}}"#).unwrap();
    let search = Arc::new(RecordingWorkspaceSearch::default());
    let services = neoism_agent_service_api::AgentServices::new(Arc::new(neoism_agent_service_api::StandardExecutableService), search.clone());
    let db = root.join("agent.db");
    let state = AppState::open_database_with_services(&db, services).await.unwrap();
    let snapshot = state.plugin_snapshot(root.to_str().unwrap()).await;
    let tools = provider_tools_for_agent(
        &state,
        root.to_str().unwrap(),
        &snapshot,
        &[],
        "gpt-5.5",
    )
    .await
    .unwrap();
    assert!(!tools.iter().any(|tool| matches!(tool.id.as_str(), "grep" | "glob" | "read")));
    let router = app(state);
    let session: SessionInfo = response_json(router.clone().oneshot(request(
        Method::POST,
        &format!("/v2/sessions?directory={}", root.to_string_lossy()),
        Some(json!({})),
    )).await.unwrap()).await;
    let response = router.oneshot(request(
        Method::GET,
        &format!("/v2/sessions/{}/directory-options", session.id),
        None,
    )).await.unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
    assert_eq!(search.warms.load(Ordering::SeqCst), 0);
    cleanup_sqlite_files(&db);
    let _ = std::fs::remove_dir_all(root);
}
