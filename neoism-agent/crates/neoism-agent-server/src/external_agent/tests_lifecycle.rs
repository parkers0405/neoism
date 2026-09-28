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
    let model_options = if provider == "codex" { vec![] } else { vec![json!({
        "id":"model", "name":"Model", "category":"model", "type":"select",
        "currentValue":format!("{provider}/default"),
        "options":[{"value":format!("{provider}/default"),"name":"Default"}]
    })] };
    let new = json!({"jsonrpc":"2.0","id":2,"result":{"sessionId":format!("native-{provider}"),"configOptions":model_options}}).to_string();
    let load = json!({"jsonrpc":"2.0","id":2,"result":{"configOptions":model_options}}).to_string();
    let replay = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"DO NOT REPLAY TO LIVE TURN"}}}}).to_string();
    let plan = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"plan","entries":[{"content":"check history","status":"in_progress","priority":"high"}]}}}).to_string();
    let tool = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call","toolCallId":"native-tool-1","title":"Inspect workspace","kind":"read","status":"in_progress","rawInput":{"path":"README.md"}}}}).to_string();
    let tool_done = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"native-tool-1","status":"completed","rawOutput":{"text":"inspected"}}}}).to_string();
    let first = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"First "}}}}).to_string();
    let second = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"answer"}}}}).to_string();
    let user_echo = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"user_message_chunk","messageId":"same-user-id","content":{"type":"text","text":"second turn"}}}}).to_string();
    let mislabeled_echo = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","messageId":"same-user-id","content":{"type":"text","text":"second turn"}}}}).to_string();
    let continued = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"agent_message_chunk","messageId":"distinct-assistant-id","content":{"type":"text","text":"Continued answer"}}}}).to_string();
    let permission = json!({"jsonrpc":"2.0","id":91,"method":"session/request_permission","params":{"sessionId":format!("native-{provider}"),"toolCall":{"toolCallId":"native-tool-1","title":"Inspect workspace"},"options":[{"optionId":"allow","kind":"allow_once"},{"optionId":"reject","kind":"reject_once"}]}}).to_string();
    let done = json!({"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn","usage":{"totalTokens":12,"inputTokens":5,"outputTokens":7}}}).to_string();
    let commands = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"review","description":"Review files","input":null},{"name":"test","description":"Run tests","input":{"hint":"path"}}]}}}).to_string();
    let cleared = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"available_commands_update","availableCommands":[]}}}).to_string();
    let nested = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call","toolCallId":"task-1","title":"Task","status":"in_progress","rawInput":{"prompt":"Review code","subagent_type":"general"}}}}).to_string();
    let nested_done = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"tool_call_update","toolCallId":"task-1","status":"completed","rawOutput":{"text":"Reviewed"}}}}).to_string();
    let usage = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":format!("native-{provider}"),"update":{"sessionUpdate":"usage_update","usage":{"totalTokens":8,"inputTokens":5,"outputTokens":3}}}}).to_string();
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
          printf '%s\n' '{tool}' '{nested}' '{plan}' '{usage}' '{first}' '{permission}'
          IFS= read -r reply || exit 1
          printf '%s\n' "$reply" >> '{log}'
          case "$reply" in *'"optionId":"allow"'*) ;; *) exit 2 ;; esac
          printf '%s\n' '{tool_done}' '{nested_done}' '{second}' '{done}'
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
        tool_done = tool_done,
        plan = plan,
        first = first,
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
        nested_done = nested_done
    );
    std::fs::write(&script, source).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    (script, log)
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
        let session: SessionInfo = json_response(
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
        tokio::time::timeout(Duration::from_secs(8), async {
            while !(saw_delta && saw_user && saw_todo && saw_tool && saw_permission && saw_live_usage) {
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
                        }
                    }
                    event_type::MESSAGE_UPDATED if event.properties["info"]["tokens"]["total"] == 8 => {
                        assert!(event.properties["info"]["time"]["completed"].is_null());
                        saw_live_usage = true;
                    }
                    event_type::PERMISSION_ASKED => {
                        saw_permission = true;
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
                    }
                    _ => {}
                }
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{provider}: missing GUI events: delta={saw_delta} todo={saw_todo} tool={saw_tool} permission={saw_permission}"));
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
        assert_eq!(first_assistant.model_id, if provider == "codex" { String::new() } else { format!("{provider}/default") });
        assert!(first[1].parts.iter().any(|part| matches!(part, Part::Tool(tool)
            if tool.call_id == "native-tool-1" && matches!(tool.state, neoism_agent_core::ToolState::Completed { .. }))));
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
        assert!(super::options::apply_commands(
            &state,
            session.id.as_str(),
            &format!("native-{provider}"),
            &json!({"availableCommands":[{"name":"invalid name","description":"bad"}]})
        )
        .await
        .is_err());
        assert!(super::options::apply_commands(&state, session.id.as_str(), &format!("native-{provider}"),
            &json!({"availableCommands":[{"name":"same","description":"one"},{"name":"same","description":"two"}]})).await.is_err());
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
