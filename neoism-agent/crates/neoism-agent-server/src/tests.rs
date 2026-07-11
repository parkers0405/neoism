use super::*;
use crate::state::{DbBackend, SessionStore};
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use neoism_agent_core::{
    AuthInfo, NeoismConfig, PluginStatusInfo, ProviderListResult, SessionUndoStatus,
    SessionUndoTree,
};
use serde::de::DeserializeOwned;
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
    assert!(tool_allowed_for_model("edit", "gpt-5.5"));
    assert!(!tool_allowed_for_model("write", "gpt-5.5"));

    assert!(!use_apply_patch_for_model("gpt-4.1"));
    assert!(!tool_allowed_for_model("apply_patch", "gpt-4.1"));
    assert!(tool_allowed_for_model("edit", "gpt-4.1"));
    assert!(tool_allowed_for_model("write", "gpt-4.1"));

    let available = HashSet::from(["apply_patch".to_string()]);
    assert_eq!(
        normalize_provider_tool_name("patch", &json!({}), &available).as_deref(),
        Some("apply_patch")
    );
    assert_eq!(
        normalize_provider_tool_name(
            "edit",
            &json!({ "patchText": "*** Begin Patch\n*** End Patch" }),
            &available,
        )
        .as_deref(),
        Some("apply_patch")
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
    let mut info = SessionInfo {
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
            system: None,
            tools: None,
        }),
        parts: vec![Part::Text(TextPart {
            id: Id::ascending(IdKind::Part),
            session_id,
            message_id,
            text: "summarize this context".to_string(),
            synthetic: None,
            time: None,
        })],
    }];
    info.extra.insert(
        "summary".to_string(),
        json!({ "text": build_session_summary(&messages), "messageCount": messages.len() }),
    );

    let provider_messages = provider_messages_for_session(&info, &messages, "stub", None);

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
    let mut info = SessionInfo {
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
    let messages = vec![
        MessageWithParts {
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
            }),
            parts: vec![Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id: session_id.clone(),
                message_id: first_id,
                text: "old compacted request".to_string(),
                synthetic: None,
                time: None,
            })],
        },
        MessageWithParts {
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
            }),
            parts: vec![Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id,
                message_id: second_id,
                text: "new tail request".to_string(),
                synthetic: None,
                time: None,
            })],
        },
    ];
    info.extra.insert(
        "summary".to_string(),
        json!({ "text": "Summary covers old compacted request.", "messageCount": 1 }),
    );

    let provider_messages = provider_messages_for_session(&info, &messages, "stub", None);

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

#[test]
fn instruction_files_are_added_to_provider_context() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-provider-instructions-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("AGENTS.md"),
        "Always respect project instructions.\n",
    )
    .unwrap();

    let session_id = Id::ascending(IdKind::Session);
    let info = SessionInfo {
        id: session_id,
        slug: "instruction-test".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: root.to_string_lossy().to_string(),
        path: None,
        parent_id: None,
        title: "Instruction test".to_string(),
        agent: None,
        model: None,
        version: "0.0.0".to_string(),
        time: TimeInfo {
            created: 1,
            updated: 1,
            compacting: None,
            archived: None,
        },
        permission: None,
        extra: BTreeMap::new(),
    };

    let provider_messages = provider_messages_for_session(&info, &[], "stub", None);

    assert!(provider_messages[0].content.contains("Instructions from:"));
    assert!(provider_messages[0]
        .content
        .contains("Always respect project instructions."));
    assert_eq!(provider_messages.len(), 1);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn sqlite_store_persists_sessions_and_messages() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-{}.sqlite3",
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
async fn turso_store_persists_sessions_and_search_falls_back_to_like() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open_with_backend(path.clone(), DbBackend::Turso)
        .await
        .unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    store
        .insert_session(&store_test_session(&session_id, now))
        .await
        .unwrap();
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
    let store = SessionStore::open_with_backend(path.clone(), DbBackend::Turso)
        .await
        .unwrap();
    assert_eq!(store.list_sessions().await.unwrap().len(), 1);
    assert_eq!(
        store
            .list_messages(session_id.as_str())
            .await
            .unwrap()
            .len(),
        2
    );

    // No FTS5 on turso: search takes the LIKE fallback, AND-ing all terms.
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

    // delete_session removes children explicitly (no FK cascade on turso).
    assert!(store.delete_session(session_id.as_str()).await.unwrap());
    assert!(store
        .list_messages(session_id.as_str())
        .await
        .unwrap()
        .is_empty());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn semantic_store_ranks_by_vector_distance_on_turso() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-sem-{}.turso.db",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open_with_backend(path.clone(), DbBackend::Turso)
        .await
        .unwrap();
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
        .messages_missing_embeddings("test-model", 10)
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
        .messages_missing_embeddings("test-model", 10)
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
async fn search_messages_uses_fts_on_sqlite() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-fts-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);

    let store = SessionStore::open_with_backend(path.clone(), DbBackend::Sqlite)
        .await
        .unwrap();
    let session_id = neoism_agent_core::new_session_id();
    let now = now_millis();
    store
        .insert_session(&store_test_session(&session_id, now))
        .await
        .unwrap();
    store
        .append_message(
            session_id.as_str(),
            &store_test_message(&session_id, now, "the quick brown fox jumps"),
        )
        .await
        .unwrap();

    let hits = store.search_messages("quick", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(
        hits[0].excerpt.contains(">>quick<<"),
        "excerpt: {}",
        hits[0].excerpt
    );
    store.close().await;
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
async fn sync_history_replays_persisted_events() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-events-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session_id = neoism_agent_core::new_session_id();
    state.publish(EventPayload::new(
        event_type::SESSION_STATUS,
        json!({ "sessionID": session_id, "status": { "type": "idle" } }),
    ));

    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let events = state
                .inner
                .store
                .list_events_after(0, 10, Some(session_id.as_str()))
                .await
                .unwrap();
            if !events.is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .unwrap();

    let events: Vec<Value> = response_json(
        app.oneshot(request(
            Method::POST,
            "/sync/history",
            Some(json!({ "since": 0, "sessionID": session_id })),
        ))
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], event_type::SESSION_STATUS);
    assert_eq!(events[0]["properties"]["sessionID"], session_id.as_str());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn sync_history_accepts_opencode_aggregate_sequence_map() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-sync-map-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session_id = neoism_agent_core::new_session_id();
    state
        .publish_persisted(EventPayload::new(
            event_type::SESSION_STATUS,
            json!({ "sessionID": session_id, "status": { "type": "busy" } }),
        ))
        .await
        .unwrap();
    state
        .publish_persisted(EventPayload::new(
            event_type::SESSION_STATUS,
            json!({ "sessionID": session_id, "status": { "type": "idle" } }),
        ))
        .await
        .unwrap();

    let events: Vec<Value> = response_json(
        app.oneshot(request(
            Method::POST,
            "/sync/history",
            Some(json!({ session_id.to_string(): 0 })),
        ))
        .await
        .unwrap(),
    )
    .await;

    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["aggregate_id"], session_id.as_str());
    assert_eq!(events[0]["aggregateID"], session_id.as_str());
    assert_eq!(events[0]["seq"], 1);
    assert_eq!(events[0]["data"]["status"]["type"], "idle");
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn sync_replay_persists_opencode_events() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-sync-replay-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session_id = neoism_agent_core::new_session_id();
    let event_id = Id::ascending(IdKind::Event).to_string();

    let replayed: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/sync/replay",
                Some(json!({
                    "directory": "/tmp/neoism-test",
                    "events": [{
                        "id": event_id.clone(),
                        "aggregateID": session_id.to_string(),
                        "seq": 0,
                        "type": event_type::SESSION_STATUS,
                        "data": { "status": { "type": "idle" } }
                    }]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replayed["sessionID"], session_id.as_str());

    let events: Vec<Value> = response_json(
        app.oneshot(request(Method::POST, "/sync/history", Some(json!({}))))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["id"], event_id);
    assert_eq!(events[0]["aggregate_id"], session_id.as_str());
    assert_eq!(events[0]["seq"], 0);
    assert_eq!(events[0]["data"]["sessionID"], session_id.as_str());
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn sync_replay_projects_session_message_and_parts() {
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-sync-project-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    let session_id = neoism_agent_core::new_session_id();
    let message_id = Id::ascending(IdKind::Message);
    let part_id = Id::ascending(IdKind::Part);
    let session = SessionInfo {
        id: session_id.clone(),
        slug: "synced-session".to_string(),
        project_id: "global".to_string(),
        workspace_id: None,
        directory: "/tmp".to_string(),
        path: None,
        parent_id: None,
        title: "Synced Session".to_string(),
        agent: Some("build".to_string()),
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
    let user_info = MessageInfo::User(UserMessage {
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
    });
    let part = Part::Text(TextPart {
        id: part_id,
        session_id: session_id.clone(),
        message_id: message_id.clone(),
        text: "projected from sync replay".to_string(),
        synthetic: None,
        time: None,
    });

    let replayed: Value = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                "/sync/replay",
                Some(json!({
                    "ownerID": "owner-a",
                    "events": [
                        {
                            "id": Id::ascending(IdKind::Event).to_string(),
                            "aggregateID": session_id.to_string(),
                            "seq": 0,
                            "type": event_type::SESSION_CREATED,
                            "data": { "sessionID": session_id.to_string(), "info": session }
                        },
                        {
                            "id": Id::ascending(IdKind::Event).to_string(),
                            "aggregateID": session_id.to_string(),
                            "seq": 1,
                            "type": event_type::MESSAGE_UPDATED,
                            "data": { "sessionID": session_id.to_string(), "info": user_info }
                        },
                        {
                            "id": Id::ascending(IdKind::Event).to_string(),
                            "aggregateID": session_id.to_string(),
                            "seq": 2,
                            "type": event_type::MESSAGE_PART_UPDATED,
                            "data": { "sessionID": session_id.to_string(), "part": part }
                        }
                    ]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replayed["sessionID"], session_id.as_str());

    let projected_session = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected_session.title, "Synced Session");
    let messages = state
        .inner
        .store
        .list_messages(session_id.as_str())
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(matches!(
        messages[0].parts.first(),
        Some(Part::Text(TextPart { text, .. })) if text == "projected from sync replay"
    ));

    let mut ignored = projected_session.clone();
    ignored.title = "Wrong Owner Update".to_string();
    let _: Value = response_json(
        app.oneshot(request(
            Method::POST,
            "/sync/replay",
            Some(json!({
                "ownerID": "owner-b",
                "events": [{
                    "id": Id::ascending(IdKind::Event).to_string(),
                    "aggregateID": session_id.to_string(),
                    "seq": 3,
                    "type": event_type::SESSION_UPDATED,
                    "data": { "sessionID": session_id.to_string(), "info": ignored }
                }]
            })),
        ))
        .await
        .unwrap(),
    )
    .await;
    let projected_session = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(projected_session.title, "Synced Session");
    cleanup_sqlite_files(&path);
}

#[tokio::test]
async fn neoism_headless_routes_are_registered() {
    std::env::set_var("NEOISM_AGENT_DISABLE_MODELS_FETCH", "true");
    let path = std::env::temp_dir().join(format!(
        "neoism-agent-routes-{}.sqlite3",
        Id::ascending(IdKind::Event)
    ));
    cleanup_sqlite_files(&path);
    let state = AppState::open_database(path.clone()).await.unwrap();
    let app = app(state.clone());
    for (method, path) in [
        (Method::GET, "/global/health"),
        (Method::GET, "/global/config"),
        (Method::GET, "/path"),
        (Method::GET, "/config/providers"),
        (Method::GET, "/provider"),
        (Method::GET, "/provider/auth"),
        (Method::GET, "/auth/test-provider"),
        (Method::GET, "/project/current"),
        (Method::GET, "/vcs"),
        (Method::GET, "/command"),
        (Method::GET, "/agent"),
        (Method::GET, "/agent/build"),
        (Method::GET, "/skill"),
        (Method::GET, "/plugin"),
        (Method::GET, "/lsp"),
        (Method::GET, "/lsp/hover?file=src/lib.rs&line=1&character=1"),
        (
            Method::GET,
            "/lsp/definition?file=src/lib.rs&line=1&character=1",
        ),
        (Method::GET, "/lsp/document-symbols?file=src/lib.rs"),
        (Method::POST, "/lsp/shutdown"),
        (Method::GET, "/permission"),
        (Method::GET, "/question"),
        (Method::GET, "/pty/shells"),
        (Method::GET, "/mcp"),
        (Method::GET, "/experimental/tool/ids"),
        (Method::POST, "/experimental/tool/read/execute"),
        (Method::GET, "/experimental/resource"),
        (Method::GET, "/api/session"),
        (Method::GET, "/session"),
        (Method::GET, "/session/status"),
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
                .oneshot(request(Method::GET, "/provider/auth", None))
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
                "/provider/test-provider/oauth/authorize",
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
            .oneshot(request(Method::GET, "/auth/test-provider", None))
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
            .oneshot(request(Method::GET, "/provider", None))
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
            .oneshot(request(Method::DELETE, "/auth/test-provider", None))
            .await
            .unwrap(),
    )
    .await;
    assert!(removed);

    let stored: Option<AuthInfo> = response_json(
        app.clone()
            .oneshot(request(Method::GET, "/auth/test-provider", None))
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
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let assistant: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({
                    "parts": [{ "type": "text", "text": "stream this" }]
                })),
            ))
            .await
            .unwrap(),
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

    let messages: Vec<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/message", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(messages.len(), 2);
    assert_eq!(
        serde_json::to_value(&messages[1].parts).unwrap(),
        serde_json::to_value(&assistant.parts).unwrap()
    );

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

struct TestNativePlugin;

impl plugin::NativePlugin for TestNativePlugin {
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
            *args = json!({ "path": "input.txt" });
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
async fn native_plugin_hooks_can_shape_tools_and_chat_context() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-native-plugin-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("input.txt"), "plugin selected this file").unwrap();
    std::fs::write(
        root.join("neoism.json"),
        r#"{ "permission": { "read": "allow" } }"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    state.inner.plugins.register(TestNativePlugin);
    let app = app(state);

    let tools: Vec<ToolListItem> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/experimental/tool?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(tools
        .iter()
        .any(|tool| tool.id == "read" && tool.description.contains("[plugin]")));

    let result: tool::ToolExecutionResult = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!(
                    "/experimental/tool/read/execute?directory={}",
                    root.to_string_lossy()
                ),
                Some(json!({ "path": "missing.txt" })),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(result.output.contains("plugin selected this file"));
    assert!(result.title.contains("[plugin]"));
    assert_eq!(result.metadata.unwrap()["plugin"], "test-native");

    let session: SessionInfo = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let assistant: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({ "parts": [{ "type": "text", "text": "hello plugin" }] })),
            ))
            .await
            .unwrap(),
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
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let assistant: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({
                    "model": { "providerId": "neoism", "modelId": "stub" },
                    "parts": [{ "type": "text", "text": "read-tool: input.txt" }]
                })),
            ))
            .await
            .unwrap(),
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

    let messages: Vec<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/message", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
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
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let assistant: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({
                    "model": { "providerId": "neoism", "modelId": "stub" },
                    "parts": [{ "type": "text", "text": "read-tool-chain: first.txt, second.txt" }]
                })),
            ))
            .await
            .unwrap(),
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

    let messages: Vec<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/message", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
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
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let _assistant: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({
                    "model": { "providerId": "neoism", "modelId": "stub" },
                    "parts": [{ "type": "text", "text": "parallel-read-tools: first.txt, second.txt" }]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;

    let messages: Vec<MessageWithParts> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/session/{}/message", session.id),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
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
                &format!("/session?directory={}", root.to_string_lossy()),
                Some(json!({})),
            ))
            .await
            .unwrap(),
    )
    .await;

    let _: MessageWithParts = response_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/session/{}/message", session.id),
                Some(json!({
                    "model": { "providerId": "neoism", "modelId": "gpt-5.5" },
                    "parts": [{ "type": "text", "text": "duplicate-patch-tool: file.md" }]
                })),
            ))
            .await
            .unwrap(),
    )
    .await;

    let contents = std::fs::read_to_string(root.join("file.md")).unwrap();
    assert_eq!(contents.matches("duplicate patch guard line").count(), 1);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn plugin_status_loads_configured_rust_native_plugins() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-plugin-status-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(
        root.join("neoism.json"),
        r#"{
              "plugins": {
                "neoism.internal.noop": { "scope": "project" },
                "unknown.rust.plugin": { "enabled": true, "custom": true }
              }
            }"#,
    )
    .unwrap();
    let db_path = root.join("agent.sqlite3");
    cleanup_sqlite_files(&db_path);

    let state = AppState::open_database(db_path.clone()).await.unwrap();
    let app = app(state.clone());
    let statuses: Vec<PluginStatusInfo> = response_json(
        app.oneshot(request(
            Method::GET,
            &format!("/plugin?directory={}", root.to_string_lossy()),
            None,
        ))
        .await
        .unwrap(),
    )
    .await;

    let noop = statuses
        .iter()
        .find(|status| status.id == "neoism.internal.noop")
        .expect("internal plugin status");
    assert!(noop.active);
    assert_eq!(noop.source, neoism_agent_core::PluginSource::Internal);
    let unknown = statuses
        .iter()
        .find(|status| status.id == "unknown.rust.plugin")
        .expect("unknown plugin status");
    assert!(!unknown.active);
    assert!(unknown
        .reason
        .as_deref()
        .unwrap_or_default()
        .contains("unsupported plugin id"));
    assert_eq!(unknown.options["custom"], true);

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
    std::fs::create_dir_all(root.join(".neoism/plugins")).unwrap();
    std::fs::create_dir_all(root.join(".neoism/tools")).unwrap();
    std::fs::write(
        root.join("neoism.json"),
        r#"{ "permission": { "bash": "allow" } }"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".neoism/plugins/test-plugin.json"),
        r#"{
          "id": "test-plugin",
          "chatHeaders": { "X-Test-Plugin": "yes" },
          "chatOptions": { "metadata": { "plugin": true } },
          "shellEnv": { "PLUGIN_ENV": "loaded" }
        }"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".neoism/tools/custom_echo.json"),
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
    let app = app(state.clone());
    let statuses: Vec<PluginStatusInfo> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/plugin?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(statuses
        .iter()
        .any(|status| status.id == "test-plugin" && status.active));
    let hook_ctx = plugin::ChatHookContext {
        session_id: "ses_test".to_string(),
        agent: "build".to_string(),
        provider_id: "openai".to_string(),
        model_id: "gpt-test".to_string(),
    };
    let mut headers = std::collections::BTreeMap::new();
    state
        .inner
        .plugins
        .chat_headers(&hook_ctx, &mut headers)
        .unwrap();
    assert_eq!(headers["X-Test-Plugin"], "yes");
    let mut options = std::collections::BTreeMap::new();
    state
        .inner
        .plugins
        .chat_options(&hook_ctx, &mut options)
        .unwrap();
    assert_eq!(options["metadata"]["plugin"], true);

    let tools: Vec<ToolListItem> = response_json(
        app.clone()
            .oneshot(request(
                Method::GET,
                &format!("/experimental/tool?directory={}", root.to_string_lossy()),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    assert!(tools.iter().any(|tool| tool.id == "custom_echo"));

    let result: crate::tool::ToolExecutionResult = response_json(
        app.oneshot(request(
            Method::POST,
            &format!(
                "/experimental/tool/custom_echo/execute?directory={}",
                root.to_string_lossy()
            ),
            Some(json!({ "text": "hello" })),
        ))
        .await
        .unwrap(),
    )
    .await;
    assert_eq!(result.output, "loaded:hello");
    assert_eq!(result.metadata.unwrap()["customTool"], true);

    cleanup_sqlite_files(&db_path);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn config_loads_project_agents_commands_and_permissions() {
    let root = std::env::temp_dir().join(format!(
        "neoism-agent-config-{}",
        Id::ascending(IdKind::Event)
    ));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(root.join(".neoism/agents")).unwrap();
    std::fs::create_dir_all(root.join(".neoism/modes")).unwrap();
    std::fs::create_dir_all(root.join(".neoism/commands")).unwrap();
    std::fs::write(
        root.join("neoism.json"),
        r#"{
              "default_agent": "plan",
              "permission": { "external_directory": { "*": "ask" } },
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
        root.join(".neoism/agents/reviewer.md"),
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
        root.join(".neoism/modes/architect.md"),
        r#"---
description: Designs implementation plans
---
Design first, then hand off implementation.
"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".neoism/commands/audit.md"),
        r#"---
description: Audit the current worktree
agent: reviewer
---
Audit the current worktree for correctness.
"#,
    )
    .unwrap();

    let loaded = config::load(root.to_str().unwrap()).unwrap();
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

    let catalog = AgentCatalog::from_config(&loaded.info);
    let agents = catalog.list();
    assert_eq!(
        agents.first().map(|agent| agent.name.as_str()),
        Some("plan")
    );
    assert_eq!(catalog.get("reviewer").unwrap().mode, "subagent");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn config_validation_reports_real_setup_problems() {
    let mut config = NeoismConfig {
        default_agent: Some("missing".to_string()),
        enabled_providers: Some(vec!["openai".to_string()]),
        disabled_providers: vec!["openai".to_string()],
        model: Some("gpt-5.5".to_string()),
        ..NeoismConfig::default()
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
        .enqueue_prompt(
            session_id.as_str(),
            &PromptRequest {
                message_id: None,
                model: None,
                agent: None,
                no_reply: false,
                system: None,
                tools: None,
                parts: vec![PromptPart::Text {
                    text: "continue please".to_string(),
                }],
            },
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

    // Drive POST /sessions/import through the router; assert 2xx + echoed id.
    let response: Value = response_json(
        app.oneshot(request(
            Method::POST,
            "/sessions/import",
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
        .list_queued_prompts(session_id.as_str())
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
    // ids living there, so POST /sessions/export takes a workspaceRoot and must
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

    // Drive POST /sessions/export through the real router.
    let app = app(state.clone());
    let response: Value = response_json(
        app.oneshot(request(
            Method::POST,
            "/sessions/export",
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
                        input: json!({ "path": "file.txt", "content": "after" }),
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

async fn response_json<T: DeserializeOwned>(response: Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
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
