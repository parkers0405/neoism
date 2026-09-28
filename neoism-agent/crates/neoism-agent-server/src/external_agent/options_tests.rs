use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use std::os::unix::fs::PermissionsExt;
use tower::ServiceExt;

#[tokio::test]
async fn preview_cold_concurrent_same_key_single_flight() {
    let root = std::env::temp_dir().join(format!(
        "acp-preview-flight-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (script, log) = fake(&root);
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with("opencode", script),
    );
    let state =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let app = crate::app(state.clone());
    let uri = format!(
        "/v2/external/options/preview?provider=opencode&directory={}",
        root.display()
    );
    let requests = (0..8).map(|_| app.clone().oneshot(request(Method::GET, &uri, None)));
    for result in futures::future::join_all(requests).await {
        assert_eq!(result.unwrap().status(), StatusCode::OK);
    }
    assert_eq!(
        std::fs::read_to_string(&log)
            .unwrap()
            .matches("\"method\":\"initialize\"")
            .count(),
        1
    );
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn preview_stale_last_good_refresh_failure_backs_off() {
    let root = std::env::temp_dir().join(format!(
        "acp-preview-stale-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (script, log) = fake(&root);
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with("opencode", script),
    );
    let state =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let app = crate::app(state.clone());
    let uri = format!(
        "/v2/external/options/preview?provider=opencode&directory={}",
        root.display()
    );
    let body = Some(json!({"selectedOptions":{"model":"fast"}}));
    let good = body_json(
        app.clone()
            .oneshot(request(Method::POST, &uri, body.clone()))
            .await
            .unwrap(),
    )
    .await;
    let cwd = root.canonicalize().unwrap().to_string_lossy().into_owned();
    let choices = BTreeMap::from([("model".into(), "fast".into())]);
    let key = cache::key(ExternalRuntime::OpenCode, &cwd, &choices, &state).unwrap();
    let file = cache::path(&key);
    let mut snapshot: Value =
        serde_json::from_slice(&std::fs::read(&file).unwrap()).unwrap();
    snapshot["saved_at"] = json!(cache::now() - 3600);
    std::fs::write(&file, snapshot.to_string()).unwrap();
    // A broken provider must not overwrite the last-good snapshot.
    std::fs::write(root.join("fail"), "1").unwrap();
    let stale = body_json(
        app.clone()
            .oneshot(request(Method::POST, &uri, body.clone()))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(stale["catalogStale"], true);
    assert_eq!(stale["configOptions"], good["configOptions"]);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = std::fs::read_to_string(&log).unwrap();
    let again = body_json(
        app.clone()
            .oneshot(request(Method::POST, &uri, body))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(again["catalogStale"], true);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    assert_eq!(
        serde_json::from_slice::<Value>(&std::fs::read(&file).unwrap()).unwrap()
            ["saved_at"],
        snapshot["saved_at"]
    );
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn preview_cache_rejects_tampered_snapshots() {
    let response = ExternalOptionsResponse {
        provider: "opencode".into(),
        config_options: vec![
            json!({"id":"model","type":"select","currentValue":"base","options":[{"value":"base"}]}),
        ],
        mode_fallback: false,
        selected_options: BTreeMap::from([("model".into(), "invented".into())]),
        external_session_id: None,
        available_commands: vec![],
        replay_error: None,
        catalog_stale: None,
    };
    assert!(cache::sanitized(response).is_none());
}

#[tokio::test]
async fn preview_post_reveals_conditional_effort_without_creating_root() {
    let root = std::env::temp_dir().join(format!(
        "acp-preview-effort-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (script, log) = fake(&root);
    // The adapter advertises effort only after selecting the non-default model.
    let source = std::fs::read_to_string(&script)
        .unwrap()
        .replace("thought", "effort")
        .replace("Thought", "Effort");
    std::fs::write(&script, source).unwrap();
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with(
            "opencode",
            script.clone(),
        ),
    );
    let state =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let app = crate::app(state.clone());
    let uri = format!(
        "/v2/external/options/preview?provider=opencode&directory={}",
        root.display()
    );
    let initial = body_json(
        app.clone()
            .oneshot(request(Method::GET, &uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(initial["configOptions"][0]["currentValue"], "base");
    assert!(initial["configOptions"]
        .as_array()
        .unwrap()
        .iter()
        .all(|item| item["id"] != "effort"));
    assert_eq!(initial["selectedOptions"], json!({}));
    let first_wire = std::fs::read_to_string(&log).unwrap();
    let hit = body_json(
        app.clone()
            .oneshot(request(Method::GET, &uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(hit, initial);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), first_wire);
    let selected = body_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &uri,
                Some(json!({"selectedOptions":{"model":"fast"}})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(selected["configOptions"][0]["currentValue"], "fast");
    assert_eq!(selected["configOptions"][1]["id"], "effort");
    assert_eq!(selected["configOptions"][1]["currentValue"], "deep");
    assert_eq!(selected["selectedOptions"], json!({"model":"fast"}));
    assert!(selected["externalSessionId"].is_null());
    let effort = body_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &uri,
                Some(json!({"selectedOptions":{"model":"fast","effort":"shallow"}})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(effort["configOptions"][1]["currentValue"], "shallow");
    assert_eq!(
        effort["selectedOptions"],
        json!({"model":"fast","effort":"shallow"})
    );
    let unchanged = body_json(
        app.clone()
            .oneshot(request(Method::GET, &uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(unchanged["configOptions"], initial["configOptions"]);
    assert_eq!(unchanged["selectedOptions"], json!({}));
    for choices in [
        json!({"model":"invented"}),
        json!({"effort":"shallow"}),
        json!({"model":"fast","effort":"invented"}),
    ] {
        let rejected = app
            .clone()
            .oneshot(request(
                Method::POST,
                &uri,
                Some(json!({"selectedOptions":choices})),
            ))
            .await
            .unwrap();
        assert_eq!(rejected.status(), StatusCode::BAD_REQUEST);
    }
    std::fs::write(root.join("fail"), "1").unwrap();
    // A confirmed POST snapshot is reused without consulting the adapter.
    let cached_selected = body_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &uri,
                Some(json!({"selectedOptions":{"model":"fast"}})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(cached_selected["selectedOptions"], json!({"model":"fast"}));
    assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
    let wire = std::fs::read_to_string(&log).unwrap();
    assert!(wire.contains("session/set_config_option"));
    assert!(!wire.contains("session/prompt"));
    state.shutdown().await.unwrap();
    drop(app);
    drop(state);
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with("opencode", script),
    );
    let restarted =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let cached = body_json(
        crate::app(restarted.clone())
            .oneshot(request(Method::GET, &uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(cached, initial);
    assert_eq!(std::fs::read_to_string(&log).unwrap(), wire);
    restarted.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[tokio::test]
async fn preview_and_draft_are_ephemeral_and_replay_on_first_prompt() {
    for provider in ["opencode", "claude", "codex"] {
        let root = std::env::temp_dir().join(format!(
            "acp-preview-{provider}-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (script, log) = fake(&root);
        let mut services = crate::standard_services();
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                if provider == "opencode" {
                    "opencode"
                } else {
                    "npx"
                },
                script,
            ),
        );
        let state =
            AppState::open_database_with_services(root.join("agent.sqlite3"), services)
                .await
                .unwrap();
        let app = crate::app(state.clone());
        let preview = body_json(
            app.clone()
                .oneshot(request(
                    Method::GET,
                    &format!(
                        "/v2/external/options/preview?provider={provider}&directory={}",
                        root.display()
                    ),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(preview["provider"], provider);
        assert_eq!(preview["configOptions"][0]["currentValue"], "base");
        assert_eq!(preview["selectedOptions"], json!({}));
        assert!(preview["externalSessionId"].is_null());
        assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
        let invalid = app.clone().oneshot(request(Method::POST, &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({"externalProvider":provider,"externalOptions":{"model":"invented"}})))).await.unwrap();
        assert_eq!(invalid.status(), StatusCode::BAD_REQUEST);
        assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
        let invalid_dependency = app.clone().oneshot(request(Method::POST, &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({"externalProvider":provider,"externalOptions":{"thought":"shallow"}})))).await.unwrap();
        assert_eq!(invalid_dependency.status(), StatusCode::BAD_REQUEST);
        let non_acp = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({"externalOptions":{"model":"fast"}})),
            ))
            .await
            .unwrap();
        assert_eq!(non_acp.status(), StatusCode::BAD_REQUEST);
        assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
        let created = body_json(app.clone().oneshot(request(Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({"externalProvider":provider,"externalOptions":{"model":"fast","thought":"shallow","mode":"code"}})))).await.unwrap()).await;
        let id = created["id"].as_str().unwrap();
        assert_eq!(
            created["externalAgent"]["selectedOptions"],
            json!({"model":"fast","thought":"shallow","mode":"code"})
        );
        assert_eq!(created["externalAgent"]["optionsValid"], false);
        assert!(created["externalAgent"]["externalSessionId"].is_null());
        let before = std::fs::read_to_string(&log).unwrap();
        assert!(!before.contains("session/prompt"));
        let sent = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{id}/prompt"),
                Some(
                    json!({"delivery":"queue","parts":[{"type":"text","text":"hello"}]}),
                ),
            ))
            .await
            .unwrap();
        assert_eq!(sent.status(), StatusCode::NO_CONTENT);
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if state.inner.store.list_messages(id).await.unwrap().iter().any(|m| matches!(&m.info, MessageInfo::Assistant(a) if a.time.completed.is_some())) { break; }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.unwrap();
        let after = std::fs::read_to_string(&log).unwrap();
        assert!(after.contains("session/prompt"));
        assert!(after.matches("session/set_config_option").count() >= 6);
        let saved = state.inner.store.get_session(id).await.unwrap().unwrap();
        assert_eq!(
            saved.extra["externalAgent"]["selectedOptions"],
            json!({"model":"fast","thought":"shallow","mode":"code"})
        );
        state.shutdown().await.unwrap();
        drop(app);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn preview_rejects_scoped_claims_before_launch() {
    let root = std::env::temp_dir().join(format!(
        "acp-preview-scope-{}",
        Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let (script, log) = fake(&root);
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with("opencode", script),
    );
    let state =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let app = crate::app(state.clone());
    for provider in ["unknown", "codex"] {
        let response = app
            .clone()
            .oneshot(request(
                Method::GET,
                &format!(
                    "/v2/external/options/preview?provider={provider}&directory={}",
                    root.display()
                ),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }
    let scoped = crate::caller::CallerClaims {
        subject: "actor".into(),
        workspace_id: Some("joined".into()),
        tenant_id: "local".into(),
        directory_prefixes: vec![root.to_string_lossy().into_owned()],
        hosted: false,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
        resolved: None,
    };
    let denied = preview(
        State(state.clone()),
        Query(PreviewQuery {
            provider: "opencode".into(),
            directory: Some(root.to_string_lossy().into_owned()),
        }),
        HeaderMap::new(),
        Some(Extension(scoped.clone())),
    )
    .await;
    assert!(denied.is_err());
    let denied_post = preview_selected(
        State(state.clone()),
        Query(PreviewQuery {
            provider: "opencode".into(),
            directory: Some(root.to_string_lossy().into_owned()),
        }),
        HeaderMap::new(),
        Some(Extension(scoped)),
        Json(PreviewRequest {
            selected_options: BTreeMap::from([("model".into(), "fast".into())]),
        }),
    )
    .await;
    assert!(denied_post.is_err());
    let outside = crate::caller::CallerClaims {
        subject: "actor".into(),
        workspace_id: None,
        tenant_id: "local".into(),
        directory_prefixes: vec![root.join("isolated").to_string_lossy().into_owned()],
        hosted: false,
        max_sessions: None,
        max_artifacts: None,
        max_artifact_bytes: None,
        artifact_retention_days: None,
        requests_per_minute: None,
        max_in_flight: None,
        resolved: None,
    };
    std::fs::create_dir_all(root.join("isolated")).unwrap();
    let denied = preview(
        State(state.clone()),
        Query(PreviewQuery {
            provider: "opencode".into(),
            directory: Some(root.to_string_lossy().into_owned()),
        }),
        HeaderMap::new(),
        Some(Extension(outside.clone())),
    )
    .await;
    assert!(denied.is_err());
    let denied_post = preview_selected(
        State(state.clone()),
        Query(PreviewQuery {
            provider: "opencode".into(),
            directory: Some(root.to_string_lossy().into_owned()),
        }),
        HeaderMap::new(),
        Some(Extension(outside)),
        Json(PreviewRequest {
            selected_options: BTreeMap::from([("model".into(), "fast".into())]),
        }),
    )
    .await;
    assert!(denied_post.is_err());
    assert!(!log.exists());
    // Even a warm disk entry cannot bypass local caller/directory authorization.
    let warm_uri = format!(
        "/v2/external/options/preview?provider=opencode&directory={}",
        root.display()
    );
    body_json(
        app.clone()
            .oneshot(request(Method::GET, &warm_uri, None))
            .await
            .unwrap(),
    )
    .await;
    let before = std::fs::read_to_string(&log).unwrap();
    let denied_warm = preview(
        State(state.clone()),
        Query(PreviewQuery {
            provider: "opencode".into(),
            directory: Some(root.to_string_lossy().into_owned()),
        }),
        HeaderMap::new(),
        Some(Extension(crate::caller::CallerClaims {
            subject: "outside".into(),
            tenant_id: "local".into(),
            workspace_id: None,
            directory_prefixes: vec![root
                .join("isolated")
                .to_string_lossy()
                .into_owned()],
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
            resolved: None,
        })),
    )
    .await;
    assert!(denied_warm.is_err());
    assert_eq!(std::fs::read_to_string(&log).unwrap(), before);
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn claude_commands_without_arguments_accept_null_input() {
    let commands = validate_commands(&json!({"availableCommands": [
        {"name": "compact", "description": "Compact context\nPreserve key decisions:\tfiles and links", "input": null},
        {"name": "review", "description": "Review changes", "input": {"hint": "optional focus"}}
    ]}))
    .unwrap();
    assert_eq!(commands.len(), 2);
    assert!(validate_commands(&json!({"availableCommands": [
        {"name": "broken", "description": "No hint", "input": {}}
    ]}))
    .is_err());
    assert!(validate_commands(&json!({"availableCommands": [
        {"name": "broken", "description": "bad\u{0000}control"}
    ]}))
    .is_err());
}

fn request(method: Method, uri: &str, body: Option<Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if body.is_some() {
        builder = builder.header("content-type", "application/json");
    }
    builder
        .body(body.map_or_else(Body::empty, |v| Body::from(v.to_string())))
        .unwrap()
}

async fn body_json(response: axum::response::Response) -> Value {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 2_000_000)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn fake(root: &std::path::Path) -> (std::path::PathBuf, std::path::PathBuf) {
    let script = root.join("adapter");
    let log = root.join("rpc.log");
    let basic = json!([
        {"id":"model","name":"Model","category":"model","type":"select","currentValue":"base","options":[{"value":"base","name":"Base"},{"value":"fast","name":"Fast"}]},
        {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"ask","options":[{"value":"ask","name":"Ask"},{"value":"code","name":"Code"}]},
        {"id":"cache","name":"Cache","category":"model_config","type":"select","currentValue":"normal","options":[{"value":"normal","name":"Normal"},{"value":"large","name":"Large"}]}
    ]).to_string();
    let fast = json!([
        {"id":"model","name":"Model","category":"model","type":"select","currentValue":"fast","options":[{"value":"base","name":"Base"},{"value":"fast","name":"Fast"}]},
        {"id":"thought","name":"Thought","category":"thought_level","type":"select","currentValue":"deep","options":[{"value":"deep","name":"Deep"},{"value":"shallow","name":"Shallow"}]},
        {"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":"ask","options":[{"value":"ask","name":"Ask"},{"value":"code","name":"Code"}]},
        {"id":"cache","name":"Cache","category":"model_config","type":"select","currentValue":"normal","options":[{"value":"normal","name":"Normal"},{"value":"large","name":"Large"}]}
    ]).to_string();
    let mut thought: Value = serde_json::from_str(&fast).unwrap();
    thought[1]["currentValue"] = json!("shallow");
    let mut mode = thought.clone();
    mode[2]["currentValue"] = json!("code");
    let thought = thought.to_string();
    let mode = mode.to_string();
    let mut notified: Value = serde_json::from_str(&fast).unwrap();
    notified[2]["currentValue"] = json!("code");
    let update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"provider-session","update":{"sessionUpdate":"config_option_update","configOptions":notified}}}).to_string();
    let commands = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"provider-session","update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review changes\nInclude context:\tfiles and links","input":null}]}}}).to_string();
    let source = format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=${{line#*\"id\":}}; id=${{id%%,*}}
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}}}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"provider-session","configOptions":{basic}}}}}\n' "$id"; printf '%s\n' '{commands}' ;;
    *'"method":"session/load"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":{basic}}}}}\n' "$id"; printf '%s\n' '{commands}' ;;
    *'"method":"session/set_config_option"'*)
      case "$line" in *'"configId":"model","sessionId":"provider-session","value":"fast"'*|*'"configId":"model"'*'"value":"fast"'*)
        if [ -f '{root}/fail' ]; then printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id";
        else printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":{fast}}}}}\n' "$id"; fi ;;
       *'"configId":"thought"'*'"value":"shallow"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":{thought}}}}}\n' "$id" ;;
       *'"configId":"mode"'*'"value":"code"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"configOptions":{mode}}}}}\n' "$id" ;;
       *) printf '{{"jsonrpc":"2.0","id":%s,"error":{{"code":-32602,"message":"unsupported"}}}}\n' "$id" ;; esac ;;
    *'"method":"session/prompt"'*)
      case "$line" in *'"text":"hold"'*)
        while IFS= read -r pending; do
          printf '%s\n' "$pending" >> '{log}'
          case "$pending" in *'"method":"session/cancel"'*) break ;; esac
        done
        continue ;; esac
      printf '%s\n' '{update}'
      printf '%s\n' '{{"jsonrpc":"2.0","method":"session/update","params":{{"sessionId":"provider-session","update":{{"sessionUpdate":"agent_message_chunk","content":{{"type":"text","text":"selected answer"}}}}}}}}'
      printf '{{"jsonrpc":"2.0","id":%s,"result":{{"stopReason":"end_turn"}}}}\n' "$id" ;;
  esac
done
"#,
        log = log.display(),
        root = root.display(),
        basic = basic,
        fast = fast,
        thought = thought,
        mode = mode,
        update = update,
        commands = commands
    );
    std::fs::write(&script, source).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    (script, log)
}

#[tokio::test]
async fn options_http_confirms_complete_state_and_reapplies_after_reload() {
    for provider in ["opencode", "codex", "claude"] {
        let root = std::env::temp_dir().join(format!(
            "acp-options-{provider}-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("agent.sqlite3");
        let (script, log) = fake(&root);
        let make_services = || {
            let mut services = crate::standard_services();
            services.executables = Arc::new(
                crate::executable::test_support::FakeExecutableService::with(
                    if provider == "opencode" {
                        "opencode"
                    } else {
                        "npx"
                    },
                    script.clone(),
                ),
            );
            services
        };
        let state = AppState::open_database_with_services(&db, make_services())
            .await
            .unwrap();
        let app = crate::app(state.clone());
        let new = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &format!("/v2/sessions?directory={}", root.display()),
                    Some(json!({"externalProvider":provider})),
                ))
                .await
                .unwrap(),
        )
        .await;
        let id = new["id"].as_str().unwrap();
        let info = state.inner.store.get_session(id).await.unwrap().unwrap();
        let created_at = info.time.updated;
        let scoped = crate::caller::CallerClaims {
            subject: "other-actor".into(),
            workspace_id: Some("foreign".into()),
            tenant_id: "local".into(),
            directory_prefixes: vec![root.to_string_lossy().into_owned()],
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
            resolved: None,
        };
        assert!(scope(&state, Some(&scoped), &info).is_err());
        let hosted = crate::caller::CallerClaims {
            hosted: true,
            workspace_id: None,
            ..scoped
        };
        assert!(scope(&state, Some(&hosted), &info).is_err());
        let mut foreign = info.clone();
        foreign.extra.get_mut("externalAgent").unwrap()["sourceHost"] =
            json!("another-host");
        assert!(scope(&state, None, &foreign).is_err());
        let path = format!("/v2/sessions/{id}/external/options");
        let first = body_json(
            app.clone()
                .oneshot(request(Method::GET, &path, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["provider"], provider);
        assert_eq!(first["availableCommands"][0]["name"], "review");
        assert_eq!(
            first["availableCommands"][0]["description"],
            "Review changes\nInclude context:\tfiles and links"
        );
        assert_eq!(first["configOptions"][0]["currentValue"], "base");
        assert_eq!(first["configOptions"][1]["category"], "mode");
        assert_eq!(first["configOptions"][2]["category"], "model_config");
        assert!(first["configOptions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item["id"] != "thought"));
        assert_eq!(first["modeFallback"], false);
        let info = body_json(
            app.clone()
                .oneshot(request(Method::GET, &format!("/v2/sessions/{id}"), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            info["externalAgent"]["externalSessionId"],
            "provider-session"
        );
        assert_eq!(info["time"]["updated"], created_at);
        let launched = std::fs::read_to_string(&log).unwrap();
        let cached = body_json(
            app.clone()
                .oneshot(request(Method::GET, &path, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(cached, first);
        assert_eq!(std::fs::read_to_string(&log).unwrap(), launched);
        let bad = app
            .clone()
            .oneshot(request(
                Method::POST,
                &path,
                Some(json!({"configId":"model","value":"invented"})),
            ))
            .await
            .unwrap();
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
        std::fs::write(root.join("fail"), "1").unwrap();
        let malformed = app
            .clone()
            .oneshot(request(
                Method::POST,
                &path,
                Some(json!({"configId":"model","value":"fast"})),
            ))
            .await
            .unwrap();
        assert_eq!(malformed.status(), StatusCode::BAD_REQUEST);
        let info = body_json(
            app.clone()
                .oneshot(request(Method::GET, &format!("/v2/sessions/{id}"), None))
                .await
                .unwrap(),
        )
        .await;
        assert!(info["externalAgent"]["selectedOptions"]
            .as_object()
            .unwrap()
            .is_empty());
        std::fs::remove_file(root.join("fail")).unwrap();
        let chosen = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &path,
                    Some(json!({"configId":"model","value":"fast"})),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(chosen["configOptions"][0]["currentValue"], "fast");
        assert_eq!(chosen["configOptions"][1]["currentValue"], "deep");
        assert_eq!(chosen["selectedOptions"]["model"], "fast");
        assert_eq!(chosen["configOptions"][3]["id"], "cache");
        let thinking = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &path,
                    Some(json!({"configId":"thought","value":"shallow"})),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(thinking["configOptions"][1]["currentValue"], "shallow");
        assert_eq!(thinking["selectedOptions"]["thought"], "shallow");
        let configured = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &path,
                    Some(json!({"configId":"mode","value":"code"})),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(configured["selectedOptions"]["mode"], "code");
        assert_eq!(configured["configOptions"][2]["currentValue"], "code");
        apply_notification(
            &state,
            id,
            &json!({"configOptions": first["configOptions"]}),
        )
        .await
        .unwrap();
        let preserved = state.inner.store.get_session(id).await.unwrap().unwrap();
        assert_eq!(
            preserved.extra["externalAgent"]["selectedOptions"]["model"],
            "fast"
        );
        assert_eq!(
            preserved.extra["externalAgent"]["selectedOptions"]["thought"],
            "shallow"
        );
        assert_eq!(
            preserved.extra["externalAgent"]["selectedOptions"]["mode"],
            "code"
        );
        assert_eq!(
            preserved.extra["externalAgent"]["configOptions"][1]["currentValue"],
            "shallow"
        );
        assert_eq!(preserved.time.updated, created_at);
        invalidate_replay(&state, id).await.unwrap();
        std::fs::write(root.join("fail"), "1").unwrap();
        let replay_failure = body_json(
            app.clone()
                .oneshot(request(Method::GET, &path, None))
                .await
                .unwrap(),
        )
        .await;
        assert!(replay_failure["replayError"]
            .as_str()
            .unwrap()
            .contains("complete configOptions"));
        assert_eq!(replay_failure["selectedOptions"]["model"], "fast");
        let failed = state.inner.store.get_session(id).await.unwrap().unwrap();
        assert_eq!(failed.extra["externalAgent"]["optionsValid"], false);
        assert_eq!(
            failed.extra["externalAgent"]["selectedOptions"]["model"],
            "fast"
        );
        assert_eq!(
            failed.extra["externalAgent"]["selectedOptions"]["thought"],
            "shallow"
        );
        assert_eq!(
            failed.extra["externalAgent"]["configOptions"][1]["currentValue"],
            "shallow"
        );
        std::fs::remove_file(root.join("fail")).unwrap();
        let recovered = body_json(
            app.clone()
                .oneshot(request(Method::GET, &path, None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(recovered["configOptions"][0]["currentValue"], "fast");
        assert_eq!(recovered["configOptions"][1]["currentValue"], "shallow");
        assert_eq!(recovered["configOptions"][2]["currentValue"], "code");
        assert_eq!(
            state
                .inner
                .store
                .get_session(id)
                .await
                .unwrap()
                .unwrap()
                .extra["externalAgent"]["optionsValid"],
            true
        );
        let mut broken = state.inner.store.get_session(id).await.unwrap().unwrap();
        let external = broken.extra.get_mut("externalAgent").unwrap();
        external["selectedOptions"]["model"] = json!("retired-model");
        external["configOptions"] = first["configOptions"].clone();
        external["optionsValid"] = json!(false);
        state.inner.store.update_session(&broken).await.unwrap();
        let stuck = body_json(
            app.clone()
                .oneshot(request(Method::GET, &path, None))
                .await
                .unwrap(),
        )
        .await;
        assert!(stuck["replayError"]
            .as_str()
            .unwrap()
            .contains("selected config option"));
        assert_eq!(stuck["selectedOptions"]["model"], "retired-model");
        assert_eq!(stuck["configOptions"][0]["currentValue"], "base");
        let repaired = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &path,
                    Some(json!({"configId":"model","value":"fast"})),
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(repaired["selectedOptions"]["model"], "fast");
        assert_eq!(repaired["selectedOptions"]["thought"], "shallow");
        assert_eq!(repaired["configOptions"][1]["currentValue"], "shallow");
        assert_eq!(
            state
                .inner
                .store
                .get_session(id)
                .await
                .unwrap()
                .unwrap()
                .extra["externalAgent"]["optionsValid"],
            true
        );
        drop(app);
        state.shutdown().await.unwrap();
        drop(state);
        let state = AppState::open_database_with_services(&db, make_services())
            .await
            .unwrap();
        let app = crate::app(state.clone());
        let sent = app.clone().oneshot(request(Method::POST,&format!("/v2/sessions/{id}/prompt"),Some(json!({"delivery":"queue","model":{"providerId":"neoism","modelId":"DO-NOT-PASS"},"parts":[{"type":"text","text":"hello"}]})))).await.unwrap();
        assert_eq!(sent.status(), StatusCode::NO_CONTENT);
        tokio::time::timeout(Duration::from_secs(8),async { loop { let messages = state.inner.store.list_messages(id).await.unwrap(); if messages.iter().any(|message| matches!(&message.info,MessageInfo::Assistant(assistant) if assistant.time.completed.is_some())) { break; } tokio::time::sleep(Duration::from_millis(20)).await; } }).await.unwrap();
        let answers = state.inner.store.list_messages(id).await.unwrap();
        let answer = answers
            .iter()
            .find_map(|message| match &message.info {
                MessageInfo::Assistant(assistant) => Some(assistant),
                _ => None,
            })
            .unwrap();
        assert_eq!(answer.model_id, "fast");
        let log_text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(log_text.matches("\"method\":\"session/new\"").count(), 1);
        assert!(log_text.matches("\"method\":\"session/load\"").count() >= 3);
        assert!(
            log_text
                .matches("\"method\":\"session/set_config_option\"")
                .count()
                >= 2
        );
        assert!(!log_text.contains("DO-NOT-PASS"));
        let info = body_json(
            app.clone()
                .oneshot(request(Method::GET, &format!("/v2/sessions/{id}"), None))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(info["externalAgent"]["selectedOptions"]["model"], "fast");
        assert_eq!(
            info["externalAgent"]["selectedOptions"]["thought"],
            "shallow"
        );
        assert_eq!(info["externalAgent"]["selectedOptions"]["mode"], "code");
        assert_eq!(
            info["externalAgent"]["configOptions"][1]["currentValue"],
            "shallow"
        );
        assert_eq!(
            info["externalAgent"]["configOptions"][2]["currentValue"],
            "code"
        );
        let hold = app
            .clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions/{id}/prompt"),
                Some(json!({"delivery":"queue","parts":[{"type":"text","text":"hold"}]})),
            ))
            .await
            .unwrap();
        assert_eq!(hold.status(), StatusCode::NO_CONTENT);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read_to_string(&log)
                .unwrap_or_default()
                .contains("\"text\":\"hold\"")
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("queued hold prompt did not reach ACP adapter");
        let busy = app
            .clone()
            .oneshot(request(
                Method::POST,
                &path,
                Some(json!({"configId":"model","value":"base"})),
            ))
            .await
            .unwrap();
        assert_eq!(busy.status(), StatusCode::CONFLICT);
        let stopped = body_json(
            app.clone()
                .oneshot(request(
                    Method::POST,
                    &format!("/v2/sessions/{id}/abort"),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(stopped, true);
        state.shutdown().await.unwrap();
        drop(app);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn legacy_mode_uses_set_mode_only_when_no_config_mode_exists() {
    let root = std::env::temp_dir()
        .join(format!("acp-legacy-mode-{}", Id::ascending(IdKind::Event)));
    std::fs::create_dir_all(&root).unwrap();
    let script = root.join("adapter");
    let log = root.join("calls.log");
    let source = format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  id=${{line#*\"id\":}}; id=${{id%%,*}}
  case "$line" in
    *'"method":"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":1,"agentCapabilities":{{"loadSession":true}}}}}}\n' "$id" ;;
    *'"method":"session/new"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"sessionId":"legacy-id","modes":{{"currentModeId":"ask","availableModes":[{{"id":"ask","name":"Ask"}},{{"id":"code","name":"Code"}}]}}}}}}\n' "$id" ;;
    *'"method":"session/load"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"modes":{{"currentModeId":"ask","availableModes":[{{"id":"ask","name":"Ask"}},{{"id":"code","name":"Code"}}]}}}}}}\n' "$id" ;;
    *'"method":"session/set_mode"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{}}}}\n' "$id" ;;
  esac
done
"#,
        log = log.display()
    );
    std::fs::write(&script, source).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let mut services = crate::standard_services();
    services.executables = Arc::new(
        crate::executable::test_support::FakeExecutableService::with("opencode", script),
    );
    let state =
        AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
    let app = crate::app(state.clone());
    let new = body_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &format!("/v2/sessions?directory={}", root.display()),
                Some(json!({"externalProvider":"opencode"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    let uri = format!(
        "/v2/sessions/{}/external/options",
        new["id"].as_str().unwrap()
    );
    let options = body_json(
        app.clone()
            .oneshot(request(Method::GET, &uri, None))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(options["modeFallback"], true);
    assert_eq!(options["configOptions"][0]["currentValue"], "ask");
    let chosen = body_json(
        app.clone()
            .oneshot(request(
                Method::POST,
                &uri,
                Some(json!({"configId":"mode","value":"code"})),
            ))
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(chosen["configOptions"][0]["currentValue"], "code");
    assert_eq!(chosen["selectedOptions"]["mode"], "code");
    let calls = std::fs::read_to_string(log).unwrap();
    assert_eq!(calls.matches("\"method\":\"session/new\"").count(), 1);
    assert_eq!(calls.matches("\"method\":\"session/set_mode\"").count(), 1);
    assert!(!calls.contains("session/set_config_option"));
    state.shutdown().await.unwrap();
    drop(app);
    drop(state);
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn grouped_values_and_legacy_mode_only_when_config_mode_absent() {
    let response = json!({"configOptions":[{"id":"model","name":"Model","type":"select","category":"model","currentValue":"a","options":[{"group":"available","name":"Available","options":[{"value":"a","name":"A"}]}]}],"modes":{"currentModeId":"ask","availableModes":[{"id":"ask","name":"Ask"},{"id":"code","name":"Code"}]}});
    let (options, fallback) = from_setup(&response).unwrap();
    assert!(fallback);
    assert_eq!(options.len(), 2);
    assert!(option(&options, "model", "a").is_ok());
    assert!(option(&options, "mode", "code").is_ok());
    assert!(option(&options, "model", "not-real").is_err());
    let with_mode = json!({"configOptions":[{"id":"native-mode","name":"Native Mode","type":"select","category":"mode","currentValue":"ask","options":[{"value":"ask","name":"Ask"}]}],"modes":response["modes"]});
    let (options, fallback) = from_setup(&with_mode).unwrap();
    assert!(!fallback);
    assert_eq!(options.len(), 1);
}
