use super::*;
use axum::body::Body;
use axum::http::{Method, Request, StatusCode};
use axum::response::Response;
use neoism_agent_core::{Page, TodoInfo};
use serde::de::DeserializeOwned;
use std::os::unix::fs::PermissionsExt;
use std::time::Duration;
use tower::ServiceExt;

fn http(method: Method, path: &str, body: Option<Value>) -> Request<Body> {
    let mut request = Request::builder().method(method).uri(path);
    if body.is_some() {
        request = request.header("content-type", "application/json");
    }
    request
        .body(body.map_or_else(Body::empty, |body| Body::from(body.to_string())))
        .unwrap()
}

async fn json_response<T: DeserializeOwned>(response: Response) -> T {
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(response.into_body(), 2 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

async fn messages(app: &axum::Router, id: &Id) -> Vec<MessageWithParts> {
    let page: Page<MessageWithParts> = json_response(
        app.clone()
            .oneshot(http(
                Method::GET,
                &format!("/v2/sessions/{id}/messages"),
                None,
            ))
            .await
            .unwrap(),
    )
    .await;
    page.items
}

async fn wait_for_messages(
    app: &axum::Router,
    id: &Id,
    count: usize,
) -> Vec<MessageWithParts> {
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            let list = messages(app, id).await;
            if list.len() >= count
                && matches!(list.last().map(|message| &message.info), Some(MessageInfo::Assistant(assistant)) if assistant.time.completed.is_some())
            {
                return list;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("queued ACP prompt did not finish")
}

fn text_of(message: &MessageWithParts) -> String {
    message
        .parts
        .iter()
        .filter_map(|part| match part {
            Part::Text(part) => Some(part.text.as_str()),
            _ => None,
        })
        .collect::<String>()
}

fn mock_adapter(
    root: &std::path::Path,
    provider: &str,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let script = root.join(format!("adapter-{provider}"));
    let log = root.join(format!("requests-{provider}.log"));
    let init = json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"resume":{},"list":{}}}}}).to_string();
    let model_options = if provider == "codex" {
        vec![]
    } else {
        vec![json!({
            "id":"model", "name":"Model", "category":"model", "type":"select",
            "currentValue":format!("{provider}/default"),
            "options":[{"value":format!("{provider}/default"),"name":"Default"}]
        })]
    };
    let new = json!({"jsonrpc":"2.0","id":2,"result":{"sessionId":format!("native-{provider}"),"configOptions":model_options}}).to_string();
    let load = json!({"jsonrpc":"2.0","id":2,"result":{"configOptions":model_options}})
        .to_string();
    let replay = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"DO NOT REPLAY TO LIVE TURN"}}}}).to_string();
    let plan = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"plan","entries":[{"content":"check history","status":"in_progress","priority":"high"}]}}}).to_string();
    let tool = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call","toolCallId":"native-tool-1","title":"Inspect workspace","kind":"read","status":"in_progress","rawInput":{"path":"README.md"}}}}).to_string();
    let edit = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call","toolCallId":"edit-1","title":"Update README","kind":"edit","status":"in_progress","rawInput":{"path":"README.md"},"content":[{"type":"diff","path":"README.md","oldText":"old\n","newText":"new\n"}]}}}).to_string();
    let edit_done = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"edit-1","status":"completed"}}}).to_string();
    let tool_done = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"native-tool-1","status":"completed","rawOutput":{"text":"inspected"}}}}).to_string();
    let first = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"First "}}}}).to_string();
    let first_prefix = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Fir"}}}}).to_string();
    let first_suffix = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"st "}}}}).to_string();
    let second = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"answer"}}}}).to_string();
    let user_echo = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"user_message_chunk","messageId":"same-user-id","content":{"type":"text","text":"second turn"}}}}).to_string();
    let mislabeled_echo = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","messageId":"same-user-id","content":{"type":"text","text":"second turn"}}}}).to_string();
    let continued = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","messageId":"distinct-assistant-id","content":{"type":"text","text":"Continued answer"}}}}).to_string();
    let permission = json!({"jsonrpc":"2.0","id":91,"method":"session/request_permission","params":{"sessionId":format!("native-{provider}"),"toolCall":{"toolCallId":"native-tool-1","title":"Inspect workspace"},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"reject","kind":"reject_once"}]}}).to_string();
    let done = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","usage":{"totalTokens":12,"inputTokens":5,"outputTokens":7}}}).to_string();
    let commands = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review files","input":null},{"name":"test","description":"Run tests","input":{"hint":"path"}}]}}}).to_string();
    let cleared = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}).to_string();
    let nested_input = if provider == "codex" {
        json!({"message":"Review code"})
    } else {
        json!({"prompt":"Review code","subagent_type":"general"})
    };
    let nested_title = if provider == "codex" {
        "spawn_agent"
    } else {
        "Task"
    };
    let nested = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call","toolCallId":"task-1","title":nested_title,"status":"in_progress"}}}).to_string();
    let nested_input_update = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"task-1","status":"in_progress","rawInput":nested_input}}}).to_string();
    let nested_progress = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"task-1","status":"in_progress","content":[{"type":"content","content":{"type":"text","text":"Review underway"}}]}}}).to_string();
    let nested_done = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"task-1","status":"completed","rawOutput":{"text":"Reviewed"}}}}).to_string();
    let usage = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"usage_update","usage":{"totalTokens":8,"inputTokens":5,"outputTokens":3,"contextWindow":200000}}}}).to_string();
    let source = format!(
        r#"#!/bin/sh
while IFS= read -r line; do
  printf '%s\n' "$line" >> '{log}'
  case "$line" in
    *'"method":"initialize"'*) printf '%s\n' '{init}' ;;
    *'"method":"session/new"'*) printf '%s\n' '{new}' '{commands}' ;;
    *'"method":"session/load"'*) printf '%s\n' '{replay}' '{load}' '{commands}' ;;
    *'"method":"session/prompt"'*)
      case "$line" in
        *'abort me'*)
          printf '%s\n' '{nested}' '{first}'
          # No prompt result: only a real abort can unblock Neoism.
          while IFS= read -r pending; do
            printf '%s\n' "$pending" >> '{log}'
            case "$pending" in *'"method":"session/cancel"'*) break ;; esac
          done
          ;;
        *'first turn'*)
          printf '%s\n' '{tool}' '{edit}' '{nested}' '{nested_input_update}' '{nested_progress}' '{plan}' '{usage}' '{first_prefix}' '{first_suffix}' '{permission}'
          IFS= read -r reply || exit 1
          printf '%s\n' "$reply" >> '{log}'
          case "$reply" in *'"optionId":"allow"'*) ;; *) exit 2 ;; esac
           printf '%s\n' '{tool_done}' '{edit_done}' '{nested_done}'
           sleep 0.2
           printf '%s\n' '{second}' '{done}'
          ;;
        *) printf '%s\n' '{cleared}' '{user_echo}' '{mislabeled_echo}' '{continued}' '{done}' ;;
      esac
      ;;
  esac
done
"#,
        log = log.display(),
        init = init,
        new = new,
        load = load,
        replay = replay,
        tool = tool,
        edit = edit,
        tool_done = tool_done,
        edit_done = edit_done,
        plan = plan,
        first = first,
        first_prefix = first_prefix,
        first_suffix = first_suffix,
        second = second,
        continued = continued,
        user_echo = user_echo,
        mislabeled_echo = mislabeled_echo,
        permission = permission,
        done = done,
        commands = commands,
        cleared = cleared,
        usage = usage,
        nested = nested,
        nested_input_update = nested_input_update,
        nested_progress = nested_progress,
        nested_done = nested_done
    );
    std::fs::write(&script, source).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    (script, log)
}

async fn orchestrate(
    state: &AppState,
    parent: &SessionInfo,
    provider: &str,
    task_id: Option<&str>,
    prompt: &str,
    background: bool,
    cancel: Option<Arc<AtomicBool>>,
) -> Result<tool::ToolExecutionResult, String> {
    let snapshot = state
        .plugin_snapshot_for_session(&parent.directory, parent.id.as_str())
        .await;
    crate::plugins::subagents::start_task_tool(
        state,
        snapshot,
        &parent.id,
        json!({
            "subagent_type": provider,
            "task_id": task_id,
            "description": "Review code",
            "prompt": prompt,
            "background": background,
        }),
        cancel,
    )
    .await
}

#[tokio::test]
#[ignore = "requires authenticated local Codex and Node/npx; makes a real provider request"]
async fn live_codex_new_task_streams_tools_and_retains_stdout() {
    let root = std::env::temp_dir().join(format!(
        "neoism-live-codex-{}", Id::ascending(IdKind::Event)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let state = AppState::open_database_with_services(root.join("agent.sqlite3"), crate::standard_services())
        .await.unwrap();
    if std::env::var_os("NEOISM_ACP_TEST_TOOL_TRACE").is_some() {
        let config = ExternalRuntime::Codex.acp_config(root.to_str().unwrap(), state.services()).unwrap();
        eprintln!("ACP_TEST_CODEX_PATH {:?}", config.env.iter().find(|(name, _)| name == "CODEX_PATH"));
    }
    let app = crate::app(state.clone());
    let parent: SessionInfo = json_response(app.clone().oneshot(http(
        Method::POST,
        &format!("/v2/sessions?directory={}", root.display()),
        Some(json!({
            "model":{"providerId":"orchestration-test","id":"none"},
            "permission":[{"permission":"bash","pattern":"*","action":"allow"}]
        })),
    )).await.unwrap()).await;
    let mut events = state.subscribe();
    let task_state = state.clone();
    let task_parent = parent.clone();
    let mut running = tokio::spawn(async move {
        orchestrate(
            &task_state, &task_parent, "codex", Some(""),
            "This is an integration smoke test in an empty temporary workspace. Do not read or modify files or spawn agents. First send ACP_LIVE_TEXT_BEGIN. Use your command tool exactly once to execute `printf 'ACP_LIVE_TOOL_BEGIN\\n'; sleep 2; printf 'ACP_LIVE_TOOL_END\\n'`. Then reply ACP_LIVE_TEXT_END. Actually execute the command; report any actual error instead of pretending to succeed.",
            false, None,
        ).await
    });
    let mut streamed = String::new();
    let mut assistant_ids = std::collections::HashSet::new();
    let mut saw_running_tool = false;
    let outcome = tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            tokio::select! {
                result = &mut running => break result.map_err(|error| error.to_string()).and_then(|result| result),
                event = events.recv() => {
                    let Ok(event) = event else { continue; };
                    if event.kind == event_type::MESSAGE_UPDATED && event.properties["info"]["role"] == "assistant" {
                        if let Some(id) = event.properties["info"]["id"].as_str() {
                            assistant_ids.insert(id.to_owned());
                        }
                    } else if event.kind == event_type::MESSAGE_PART_UPDATED {
                        let part = &event.properties["part"];
                        if !part.get("messageId").or_else(|| part.get("messageID")).and_then(Value::as_str)
                            .is_some_and(|id| assistant_ids.contains(id)) { continue; }
                        if part["type"] == "tool" && part["state"]["status"] == "running" {
                            saw_running_tool = true;
                        }
                        if part["type"] == "text" {
                            streamed.push_str(part["text"].as_str().unwrap_or_default());
                        }
                    } else if event.kind == event_type::MESSAGE_PART_DELTA {
                        if event.properties.get("messageID").or_else(|| event.properties.get("messageId"))
                            .and_then(Value::as_str).is_some_and(|id| assistant_ids.contains(id)) {
                            streamed.push_str(event.properties["delta"].as_str().unwrap_or_default());
                        }
                    }
                }
            }
        }
    }).await;
    if outcome.is_err() {
        for child in state.inner.store.list_child_sessions(parent.id.as_str()).await.unwrap() {
            crate::session_actions::abort_session_run(&state, child.id.as_str()).await;
        }
        if tokio::time::timeout(Duration::from_secs(5), &mut running).await.is_err() {
            running.abort();
            let _ = running.await;
        }
    }
    let result = outcome.unwrap_or_else(|_| Err("live Codex task timed out".into()));
    let mut saved_output = String::new();
    if let Ok(result) = &result {
        let child = result.metadata.as_ref().unwrap()["sessionId"].as_str().unwrap();
        for message in state.inner.store.list_messages(child).await.unwrap() {
            for part in message.parts {
                let part = serde_json::to_value(part).unwrap();
                if part["type"] == "tool" {
                    saved_output.push_str(part["state"]["output"].as_str().unwrap_or_default());
                }
            }
        }
    }
    drop(events);
    drop(app);
    state.shutdown().await.unwrap();
    let _ = std::fs::remove_dir_all(root);
    let result = result.expect("real Codex task failed");
    assert!(streamed.contains("ACP_LIVE_TEXT_BEGIN"), "no live assistant text: {streamed}");
    assert!(saw_running_tool, "no running tool event before task completion");
    assert!(result.output.contains("ACP_LIVE_TEXT_END"), "{}", result.output);
    assert!(saved_output.contains("ACP_LIVE_TOOL_BEGIN"), "stdout missing: {saved_output}");
    assert!(saved_output.contains("ACP_LIVE_TOOL_END"), "stdout missing: {saved_output}");
}

#[tokio::test]
async fn external_orchestration_continues_same_provider_and_delivers_background_results() {
    for provider in ["opencode", "claude", "codex"] {
        let root = std::env::temp_dir().join(format!(
            "neoism-orchestration-{provider}-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (script, log) = mock_adapter(&root, provider);
        let mut services = crate::standard_services();
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                if provider == "opencode" { "opencode" } else { "npx" },
                script,
            ),
        );
        let state = AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
        let app = crate::app(state.clone());
        // The native parent must never use a real API while completion delivery
        // is exercised. Its deliberately unconfigured model fails locally.
        let parent: SessionInfo = json_response(app.clone().oneshot(http(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({"model":{"providerId":"orchestration-test","id":"none"}})),
        )).await.unwrap()).await;
        let mut legacy = create_external_subtask_session(&state, &parent, "review", "Review", ExternalRuntime::resolve(provider).unwrap()).await.unwrap();
        legacy.extra.get_mut("externalAgent").unwrap()["runtime"] = json!("acp");
        state.inner.store.update_session(&legacy).await.unwrap();
        let result = orchestrate(&state, &parent, provider, Some(legacy.id.as_str()), "Review workspace", false, None)
            .await
            .unwrap();
        let metadata = result.metadata.unwrap();
        assert_eq!(metadata["provider"], provider);
        assert_eq!(metadata["status"], "completed");
        assert!(result.output.contains("Continued answer"));
        let child_id = metadata["sessionId"].as_str().unwrap();
        let child = state.inner.store.get_session(child_id).await.unwrap().unwrap();
        assert_eq!(child.parent_id.as_ref(), Some(&parent.id));
        assert_eq!(child.directory, parent.directory);
        assert_eq!(child.extra["externalAgent"]["externalSessionId"], format!("native-{provider}"));

        orchestrate(&state, &parent, provider, Some(child_id), "Continue review", false, None)
            .await
            .unwrap();
        assert!(std::fs::read_to_string(&log).unwrap().contains("session/load"));
        let other = if provider == "codex" { "claude" } else { "codex" };
        let error = orchestrate(&state, &parent, other, Some(child_id), "Wrong provider", true, None)
            .await
            .unwrap_err();
        assert!(error.contains("original subagent_type"));
        let error = orchestrate(&state, &parent, "general", Some(child_id), "Wrong runtime", false, None)
            .await
            .unwrap_err();
        assert!(error.contains("original external subagent_type"));
        let error = orchestrate(&state, &parent, provider, Some("missing-task"), "Missing child", false, None)
            .await
            .unwrap_err();
        assert!(error.contains("not found"));
        assert_eq!(state.inner.store.list_child_sessions(parent.id.as_str()).await.unwrap().len(), 1);
        assert_eq!(state.inner.store.get_session(child_id).await.unwrap().unwrap().extra["externalAgent"]["provider"], provider);

        let mut events = state.subscribe();
        orchestrate(&state, &parent, provider, Some(child_id), "Background review", true, None)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                let event = events.recv().await.unwrap();
                if event.kind == event_type::SESSION_SUBTASK_COMPLETED
                    && event.properties["childSessionID"] == child_id
                {
                    assert_eq!(event.properties["status"], "completed");
                    assert!(event.properties["result"].as_str().unwrap().contains("Continued answer"));
                    break;
                }
            }
        }).await.expect("external child completion was not delivered to its parent");
        let snapshot = state.plugin_snapshot_for_session(&parent.directory, parent.id.as_str()).await;
        let result = crate::plugins::subagents::task_result_tool(
            &state, &snapshot, &parent.id, json!({"task_id":child_id}),
        ).await.unwrap();
        assert_eq!(result.metadata.unwrap()["status"], "completed");
        drop(snapshot);

        let mut nested = state.inner.store.get_session(child_id).await.unwrap().unwrap();
        nested.extra.get_mut("externalAgent").unwrap()["nested"] = json!(true);
        state.inner.store.update_session(&nested).await.unwrap();
        let error = orchestrate(&state, &parent, provider, Some(child_id), "Cannot control native child", false, None)
            .await
            .unwrap_err();
        assert!(error.contains("Provider-native nested tasks"));
        drop(events);
        drop(app);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn external_orchestration_stop_cancels_foreground_child_without_cancelling_parent() {
    for provider in ["opencode", "claude", "codex"] {
        let root = std::env::temp_dir().join(format!(
            "neoism-orchestration-stop-{provider}-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let (script, log) = mock_adapter(&root, provider);
        let mut services = crate::standard_services();
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                if provider == "opencode" { "opencode" } else { "npx" },
                script,
            ),
        );
        let state = AppState::open_database_with_services(root.join("agent.sqlite3"), services)
            .await
            .unwrap();
        let app = crate::app(state.clone());
        let parent: SessionInfo = json_response(app.clone().oneshot(http(
            Method::POST,
            &format!("/v2/sessions?directory={}", root.display()),
            Some(json!({"model":{"providerId":"orchestration-test","id":"none"}})),
        )).await.unwrap()).await;
        let parent_cancel = Arc::new(AtomicBool::new(false));
        let mut legacy = create_external_subtask_session(&state, &parent, "review", "Review", ExternalRuntime::resolve(provider).unwrap()).await.unwrap();
        legacy.extra.get_mut("externalAgent").unwrap()["runtime"] = json!("acp");
        state.inner.store.update_session(&legacy).await.unwrap();
        let task_state = state.clone();
        let task_parent = parent.clone();
        let task_cancel = parent_cancel.clone();
        let running = tokio::spawn(async move {
            orchestrate(&task_state, &task_parent, provider, Some(legacy.id.as_str()), "abort me", false, Some(task_cancel)).await
        });
        let child = tokio::time::timeout(Duration::from_secs(8), async {
            loop {
                if std::fs::read_to_string(&log).unwrap_or_default().contains("session/prompt") {
                    if let Some(child) = state.inner.store.list_child_sessions(parent.id.as_str()).await.unwrap().into_iter().next() {
                        if session_is_running(&state, child.id.as_str()).await {
                            break child;
                        }
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        }).await.expect("external child never started");
        let error = orchestrate(&state, &parent, provider, Some(child.id.as_str()), "Do not drop this", true, None)
            .await
            .unwrap_err();
        assert!(error.contains("follow-up prompt was not submitted"));
        let snapshot = state.plugin_snapshot_for_session(&parent.directory, parent.id.as_str()).await;
        crate::plugins::subagents::stop_task_tool(&state, &snapshot, &parent.id, json!({"task_id":child.id}))
            .await
            .unwrap();
        drop(snapshot);
        let error = tokio::time::timeout(Duration::from_secs(4), running)
            .await
            .expect("stopping foreground child did not cancel its ACP turn")
            .unwrap()
            .unwrap_err();
        assert!(error.contains("Session aborted"));
        assert!(!parent_cancel.load(Ordering::SeqCst));
        let requests = std::fs::read_to_string(&log).unwrap();
        assert!(requests.contains("session/cancel"));
        assert!(!requests.contains("Do not drop this"));
        assert!(!session_is_running(&state, child.id.as_str()).await);
        drop(app);
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[tokio::test]
async fn three_provider_roots_run_via_real_http_queue_and_reload() {
    for provider in ["opencode", "codex", "claude"] {
        let root = std::env::temp_dir().join(format!(
            "neoism-acp-http-{provider}-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let db = root.join("agent.sqlite3");
        let (script, log) = mock_adapter(&root, provider);
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
        state.start_session_list_backfill();
        let app = crate::app(state.clone());
        let mut events = state.subscribe();
        let mut session: SessionInfo = json_response(
            app.clone()
                .oneshot(http(
                    Method::POST,
                    &format!("/v2/sessions?directory={}", root.display()),
                    Some(json!({"externalProvider":provider})),
                ))
                .await
                .unwrap(),
        )
        .await;
        session.extra.get_mut("externalAgent").unwrap()["runtime"] = json!("acp");
        state.inner.store.update_session(&session).await.unwrap();
        assert_eq!(
            session.extra["externalAgent"]["historyState"],
            "neoism_only"
        );
        assert_eq!(
            session.extra["externalAgent"]["externalSessionId"],
            Value::Null
        );
        let result = app.clone().oneshot(http(Method::POST, &format!("/v2/sessions/{}/prompt", session.id), Some(json!({"delivery":"queue","parts":[{"type":"text","text":"first turn"}]})))).await.unwrap();
        assert_eq!(result.status(), StatusCode::NO_CONTENT);
        let mut saw_delta = false;
        let mut saw_user = false;
        let mut saw_todo = false;
        let mut saw_tool = false;
        let mut saw_permission = false;
        let mut saw_live_usage = false;
        let mut saw_terminal_task_still_running = false;
        let mut saw_nested_completed = false;
        tokio::time::timeout(Duration::from_secs(8), async {
            while !(saw_delta && saw_user && saw_todo && saw_tool && saw_permission && saw_live_usage && saw_terminal_task_still_running) {
                let event = events.recv().await.unwrap();
                match event.kind.as_str() {
                    event_type::MESSAGE_PART_DELTA => saw_delta = true,
                    event_type::TODO_UPDATED => saw_todo = true,
                     event_type::MESSAGE_PART_UPDATED => {
                         if event.properties["part"]["type"] == "text" && event.properties["part"]["text"] == "first turn" {
                             assert_eq!(event.properties["part"]["role"], "user");
                             saw_user = true;
                         }
                          if event.properties["part"]["type"] == "tool" {
                             saw_tool = true;
                              if event.properties["part"]["callId"] == "task-1"
                                  && event.properties["part"]["state"]["status"] == "completed"
                              {
                                  assert!(!saw_nested_completed);
                                  saw_terminal_task_still_running = true;
                              }
                          }
                     }
                    event_type::SESSION_UPDATED
                        if event.properties["info"]["externalAgent"]["parentToolCallId"] == "task-1"
                            && event.properties["info"]["externalAgent"]["status"] == "completed" =>
                    {
                        saw_nested_completed = true;
                    }
                    event_type::MESSAGE_UPDATED if event.properties["info"]["tokens"]["total"] == 8 => {
                        assert!(event.properties["info"]["time"]["completed"].is_null());
                        assert_eq!(event.properties["info"]["tokens"]["contextLimit"], 200_000);
                        saw_live_usage = true;
                    }
                    event_type::PERMISSION_ASKED => {
                        saw_permission = true;
                        let children: Vec<_> = state.inner.store.list_sessions().await.unwrap().into_iter()
                            .filter(|info| info.parent_id.as_ref() == Some(&session.id)
                                && info.extra["externalAgent"]["parentToolCallId"] == "task-1").collect();
                        assert_eq!(children.len(), 1, "task must exist before completion");
                        assert_eq!(children[0].extra["externalAgent"]["status"], "running");
                        let child_messages = state.inner.store.list_messages(children[0].id.as_str()).await.unwrap();
                        assert_eq!(text_of(&child_messages[0]), "Review code");
                        // Running text is transient; only the initial boundary
                        // is durable until completion. Reconnect uses the bounded
                        // live baseline, and the spool retains exact output.
                        assert_eq!(text_of(&child_messages[1]), "");
                        let (_, live) = state.subscribe_with_messages();
                        assert!(live.iter().any(|event|
                            event.kind == event_type::MESSAGE_PART_UPDATED
                                && event.properties["sessionID"] == children[0].id.as_str()
                                && event.properties["part"]["text"] == "Review underway"));
                        let spool = children[0].extra["externalAgent"]["outputPath"].as_str().unwrap();
                        assert_eq!(std::fs::read_to_string(spool).unwrap(), "Review underway");
                        let MessageInfo::Assistant(live_child) = &child_messages[1].info else { panic!("child assistant expected") };
                        assert!(live_child.time.completed.is_none());
                        let live_id = live_child.id.clone();
                        let live_part_id = match &child_messages[1].parts[0] { Part::Text(part) => part.id.clone(), _ => panic!("text expected") };
                        let id = event.properties["id"]
                            .as_str()
                            .or_else(|| event.properties["requestID"].as_str())
                            .expect("permission request ID");
                        let reply: bool = json_response(
                            app.clone()
                                .oneshot(http(
                                    Method::POST,
                                    &format!("/v2/interactions/permissions/{id}/reply"),
                                    Some(json!({"reply":"once"})),
                                ))
                                .await
                                .unwrap(),
                        )
                        .await;
                        assert!(reply);
                        let finished_child = wait_for_messages(&app, &children[0].id, 2).await;
                        assert_eq!(finished_child.len(), 2);
                        assert_eq!(text_of(&finished_child[1]), "{\"text\":\"Reviewed\"}");
                        let MessageInfo::Assistant(done_child) = &finished_child[1].info else { panic!("child assistant expected") };
                        assert_eq!(done_child.id, live_id);
                        assert_eq!(match &finished_child[1].parts[0] { Part::Text(part) => &part.id, _ => panic!("text expected") }, &live_part_id);
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{provider}: missing GUI events: delta={saw_delta} todo={saw_todo} tool={saw_tool} permission={saw_permission} task_running={saw_terminal_task_still_running}"));
        let first = wait_for_messages(&app, &session.id, 2).await;
        assert_eq!(first.len(), 2);
        let nested_child = state
            .inner
            .store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .find(|info| {
                info.parent_id.as_ref() == Some(&session.id)
                    && info.extra["externalAgent"]["parentToolCallId"] == "task-1"
            })
            .expect("explicit nested task was not linked");
        assert_eq!(nested_child.extra["externalAgent"]["status"], "completed");
        assert_eq!(
            state
                .inner
                .store
                .list_messages(nested_child.id.as_str())
                .await
                .unwrap()
                .len(),
            2
        );
        assert!(matches!(first[0].info, MessageInfo::User(_)));
        assert_eq!(text_of(&first[0]), "first turn");
        assert_eq!(text_of(&first[1]), "First answer");
        let MessageInfo::Assistant(first_assistant) = &first[1].info else {
            panic!("assistant expected")
        };
        assert_eq!(first_assistant.tokens.total, Some(12));
        assert_eq!(first_assistant.tokens.context_limit, Some(200_000));
        assert_eq!(
            first_assistant.model_id,
            if provider == "codex" {
                String::new()
            } else {
                format!("{provider}/default")
            }
        );
        assert!(first[1].parts.iter().any(|part| matches!(part, Part::Tool(tool)
            if tool.call_id == "native-tool-1" && tool.tool == "read"
                && matches!(&tool.state, neoism_agent_core::ToolState::Completed { input, title, .. }
                    if input["path"] == "README.md" && title == "Inspect workspace"))));
        assert!(first[1].parts.iter().any(|part| matches!(part, Part::Tool(tool)
            if tool.call_id == "edit-1" && tool.tool == "edit"
                && matches!(&tool.state, neoism_agent_core::ToolState::Completed { metadata, .. }
                    if metadata["acpDiffs"][0]["newText"] == "new\n"))));
        let task_index = first[1]
            .parts
            .iter()
            .position(|part| matches!(part, Part::Tool(tool) if tool.call_id == "task-1"))
            .expect("task tool part");
        let answer_index = first[1]
            .parts
            .iter()
            .position(
                |part| matches!(part, Part::Text(text) if text.text == "answer"),
            )
            .expect("assistant answer part");
        assert!(
            task_index < answer_index,
            "ACP tool must retain stream order before the final answer"
        );
        let mut old_root = state
            .inner
            .store
            .get_session(session.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(old_root.title, "first turn");
        old_root.title = "New session - 1790000000".into();
        state.inner.store.update_session(&old_root).await.unwrap();
        let listing: Value = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions?roots=true&directory={}", root.display()),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(listing["items"][0]["title"], "first turn");
        let persisted: SessionInfo = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions/{}", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(persisted.title, "first turn");
        assert_eq!(
            persisted.extra["externalAgent"]["externalSessionId"],
            format!("native-{provider}")
        );
        assert_eq!(
            persisted.extra["externalAgent"]["availableCommands"][0]["name"],
            "review"
        );
        let options: Value = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions/{}/external/options", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(options["provider"], provider);
        assert_eq!(options["externalSessionId"], format!("native-{provider}"));
        assert!(options["availableCommands"][0]["input"].is_null());
        assert_eq!(options["availableCommands"][1]["input"]["hint"], "path");
        let snapshot = state
            .inner
            .store
            .get_session(session.id.as_str())
            .await
            .unwrap()
            .unwrap();
        let bogus =
            json!({"availableCommands":[{"name":"wrong","description":"Wrong session"}]});
        super::options::apply_commands(&state, session.id.as_str(), "foreign-id", &bogus)
            .await
            .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .get_session(session.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .extra["externalAgent"]["availableCommands"],
            snapshot.extra["externalAgent"]["availableCommands"]
        );
        super::options::apply_commands(
            &state, session.id.as_str(), &format!("native-{provider}"),
            &json!({"availableCommands":[{"name":"invalid name","description":"bad"},{"name":"/same","description":"one"},{"name":"same","description":"two"}]})
        ).await.unwrap();
        assert_eq!(
            state
                .inner
                .store
                .get_session(session.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .extra["externalAgent"]["availableCommands"],
            json!([{"name":"same","description":"one"}])
        );
        super::options::clear_commands(
            &state,
            session.id.as_str(),
            &format!("native-{provider}"),
        )
        .await
        .unwrap();
        assert_eq!(
            state
                .inner
                .store
                .get_session(session.id.as_str())
                .await
                .unwrap()
                .unwrap()
                .extra["externalAgent"]["availableCommands"],
            json!([])
        );
        super::options::apply_commands(&state, session.id.as_str(), &format!("native-{provider}"),
            &json!({"availableCommands": [{"name":"review","description":"Review files","input":{"hint":"path"}}]})).await.unwrap();
        let todos: Vec<TodoInfo> = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions/{}/todos", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(todos[0].content, "check history");
        tokio::time::timeout(Duration::from_secs(3), async {
            while !state.inner.store.session_list_index_ready().await.unwrap() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        let page: Page<SessionInfo> = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions?roots=true&directory={}", root.display()),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].extra["externalAgent"]["provider"], provider);
        assert_eq!(
            page.items[0].extra["externalAgent"]["historyState"],
            "neoism_only"
        );
        drop(events);
        // Simulate a crash after the nested assistant was committed but before
        // its status update; restart must settle status without duplicating text.
        let mut stranded = state
            .inner
            .store
            .get_session(nested_child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        stranded.extra.get_mut("externalAgent").unwrap()["status"] = json!("running");
        state.inner.store.update_session(&stranded).await.unwrap();
        let mut unfinished = stranded.clone();
        unfinished.id = neoism_agent_core::new_session_id();
        unfinished.extra.get_mut("externalAgent").unwrap()["parentToolCallId"] =
            json!("crashed-task");
        state.inner.store.insert_session(&unfinished).await.unwrap();
        append_external_user_message(
            &state,
            &unfinished,
            Id::ascending(IdKind::Message),
            "unfinished",
            ExternalRuntime::resolve(provider).unwrap(),
            &external_model(ExternalRuntime::resolve(provider).unwrap()),
            None,
        )
        .await
        .unwrap();
        drop(app);
        state.shutdown().await.unwrap();
        drop(state);

        let state = AppState::open_database_with_services(&db, make_services())
            .await
            .unwrap();
        let app = crate::app(state.clone());
        let recovered = state
            .inner
            .store
            .get_session(nested_child.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(recovered.extra["externalAgent"]["status"], "completed");
        assert_eq!(
            state
                .inner
                .store
                .list_messages(nested_child.id.as_str())
                .await
                .unwrap()
                .len(),
            2
        );
        let recovered_unfinished = state
            .inner
            .store
            .get_session(unfinished.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            recovered_unfinished.extra["externalAgent"]["status"],
            "interrupted"
        );
        assert_eq!(
            state
                .inner
                .store
                .list_messages(unfinished.id.as_str())
                .await
                .unwrap()
                .len(),
            2
        );
        let response = app.clone().oneshot(http(Method::POST, &format!("/v2/sessions/{}/prompt", session.id), Some(json!({"delivery":"queue","parts":[{"type":"text","text":"second turn"}]})))).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let second = wait_for_messages(&app, &session.id, 4).await;
        assert_eq!(second.len(), 4);
        assert_eq!(text_of(&second[3]), "Continued answer");
        let reloaded: SessionInfo = json_response(
            app.clone()
                .oneshot(http(
                    Method::GET,
                    &format!("/v2/sessions/{}", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(
            reloaded.extra["externalAgent"]["availableCommands"],
            json!([])
        );
        assert!(!text_of(&second[3]).contains("DO NOT REPLAY"));
        assert_eq!(text_of(&second[1]), "First answer");
        let response = app.clone().oneshot(http(Method::POST, &format!("/v2/sessions/{}/prompt", session.id), Some(json!({"delivery":"queue","parts":[{"type":"text","text":"abort me"}]})))).await.unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if std::fs::read_to_string(&log)
                    .unwrap_or_default()
                    .matches("abort me")
                    .count()
                    >= 1
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("abort prompt never reached adapter");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .inner
                    .store
                    .list_sessions()
                    .await
                    .unwrap()
                    .iter()
                    .any(|info| {
                        info.parent_id.as_ref() == Some(&session.id)
                            && info.extra["externalAgent"]["status"] == "running"
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("abort task never started");
        let aborted: bool = json_response(
            app.clone()
                .oneshot(http(
                    Method::POST,
                    &format!("/v2/sessions/{}/abort", session.id),
                    None,
                ))
                .await
                .unwrap(),
        )
        .await;
        assert!(aborted);
        tokio::time::timeout(Duration::from_secs(5), async {
            while state
                .inner
                .session_coordinator
                .active_run(session.id.as_str())
                .await
                .is_some()
            {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("aborted ACP run stayed active");
        let after_abort = messages(&app, &session.id).await;
        let abort_user = after_abort
            .iter()
            .find(|message| {
                matches!(message.info, MessageInfo::User(_))
                    && text_of(message) == "abort me"
            })
            .expect("aborted turn should retain its user message");
        let MessageInfo::User(abort_user_info) = &abort_user.info else {
            unreachable!()
        };
        assert!(!after_abort.iter().any(|message| matches!(&message.info,
            MessageInfo::Assistant(assistant) if assistant.parent_id == abort_user_info.id
                && assistant.error.is_none() && assistant.time.completed.is_some()
        )), "abort must not turn a partial ACP response into a successful reply");
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state
                    .inner
                    .store
                    .list_sessions()
                    .await
                    .unwrap()
                    .iter()
                    .any(|info| {
                        info.parent_id.as_ref() == Some(&session.id)
                            && info.extra["externalAgent"]["status"] == "interrupted"
                    })
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("nested task not settled on abort");
        let children = state
            .inner
            .store
            .list_sessions()
            .await
            .unwrap()
            .into_iter()
            .filter(|info| info.parent_id.as_ref() == Some(&session.id))
            .collect::<Vec<_>>();
        assert_eq!(children.len(), 3);
        assert!(
            children
                .iter()
                .any(|info| info.extra["externalAgent"]["status"] == "interrupted"),
            "{:?}",
            children
                .iter()
                .map(|info| &info.extra["externalAgent"])
                .collect::<Vec<_>>()
        );
        let log_text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(log_text.matches("\"method\":\"session/new\"").count(), 1);
        assert!(log_text.matches("\"method\":\"session/load\"").count() >= 2);
        assert!(log_text.contains("\"optionId\":\"allow\""));
        state.shutdown().await.unwrap();
        drop(app);
        drop(state);
        let _ = std::fs::remove_dir_all(root);
    }
}
