//! Desktop goal commands: metadata mutation followed by one durable prompt
//! admission. Saving a goal alone deliberately does not start an agent run.
use super::{http_delete, http_get_json, http_post_json, percent_encode, AgentInner};
use serde_json::{json, Value};

pub(super) fn command_arguments(text: &str) -> Option<&str> {
    let rest = text.trim().strip_prefix("/goal")?;
    (rest.is_empty() || rest.starts_with(char::is_whitespace)).then(|| rest.trim())
}

pub(super) async fn execute(
    inner: &AgentInner,
    session_id: &str,
    arguments: &str,
) -> Result<Value, String> {
    execute_with_prompt(inner, session_id, arguments, None).await
}

pub(super) async fn execute_with_prompt(
    inner: &AgentInner,
    session_id: &str,
    arguments: &str,
    supplied_prompt: Option<Value>,
) -> Result<Value, String> {
    let path = format!("/v2/plugins/dev.neoism.goals/{}", percent_encode(session_id));
    let (verb, tail) = arguments
        .split_once(char::is_whitespace)
        .map(|(verb, tail)| (verb, tail.trim()))
        .unwrap_or((arguments, ""));
    let verb = verb.to_ascii_lowercase();
    if tail.is_empty() && matches!(verb.as_str(), "" | "show" | "read" | "status") {
        return http_get_json(inner, &path).await;
    }
    if tail.is_empty() && matches!(verb.as_str(), "clear" | "off") {
        http_delete(inner, &path).await?;
        return Ok(json!({ "goal": null }));
    }
    let pause = tail.is_empty() && verb == "pause";
    let resume = tail.is_empty() && verb == "resume";
    let text = if pause || resume {
        let current = http_get_json(inner, &path).await?;
        goal_text(&current)?.to_string()
    } else if verb == "set" {
        if tail.is_empty() {
            return Err("usage: /goal set <goal>".into());
        }
        tail.to_string()
    } else {
        arguments.to_string()
    };
    // Snapshot the selected session configuration before saving/admitting work.
    // SlashCommand carries no model override; SetModel/SetAgent persist these
    // selections on the session. Never substitute a hardcoded agent or model.
    let session = if pause {
        Value::Null
    } else {
        http_get_json(inner, &format!("/v2/sessions/{}", percent_encode(session_id))).await?
    };
    let saved = http_post_json(inner, &path, &json!({ "text": text, "paused": pause })).await?;
    if !pause {
        let text = goal_text(&saved)?;
        if saved.pointer("/goal/paused").and_then(Value::as_bool) == Some(true)
            || saved.pointer("/goal/status").and_then(Value::as_str) != Some("active")
        {
            return Err("Goal was saved but is not active; no work was started".into());
        }
        let mut prompt = prompt_request(&session, text)?;
        if let Some(supplied) = supplied_prompt {
            // New-thread pending text may arrive through SubmitPrompt rather
            // than SlashCommand. Preserve its idempotency key and current UI
            // selections, but never send the slash text to the model or make
            // a second admission through the ordinary SubmitPrompt path.
            for key in ["messageId", "agent", "model", "author", "parts"] {
                if let Some(value) = supplied.get(key) {
                    prompt[key] = value.clone();
                }
            }
            prompt["parts"][0]["text"] = json!(text);
        }
        // Async admission acknowledges the durable queue commit, not the full
        // generation. Do not retry here: the stable message ID belongs to this
        // one invocation, and the queue worker owns recovery after admission.
        http_post_json(
            inner,
            &format!("/v2/sessions/{}/prompt-async", percent_encode(session_id)),
            &prompt,
        )
        .await
        .map_err(|error| format!("Goal saved, but work could not be started: {error}"))?;
    }
    Ok(saved)
}

fn goal_text(value: &Value) -> Result<&str, String> {
    value
        .pointer("/goal/text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| "No saved goal; use /goal <goal> first".to_string())
}

fn prompt_request(session: &Value, text: &str) -> Result<Value, String> {
    let mut prompt = json!({
        "messageId": format!("msg_goal_{}", uuid::Uuid::new_v4()),
        "delivery": "queue",
        "parts": [{ "type": "text", "text": text }],
    });
    if let Some(agent) = session.get("agent").and_then(Value::as_str) {
        prompt["agent"] = json!(agent);
    }
    if let Some(model) = session.get("model").filter(|model| !model.is_null()) {
        let mut model = model.clone();
        let fields = model.as_object_mut().ok_or("Invalid selected session model")?;
        if let Some(id) = fields.remove("id") {
            fields.insert("modelId".to_string(), id);
        }
        if !fields.get("providerId").is_some_and(Value::is_string)
            || !fields.get("modelId").is_some_and(Value::is_string)
        {
            return Err("Invalid selected session model".into());
        }
        prompt["model"] = model;
    }
    Ok(prompt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::{dispatcher, reply_queue, AgentSession};
    use axum::{body::Bytes, extract::{OriginalUri, State}, http::{Method, StatusCode}, response::{IntoResponse, Response}};
    use neoism_protocol::agent::{AgentClientMessage, AgentServerMessage, HistoryMessageKind as MessageKind};
    use parking_lot::Mutex;
    use std::{sync::Arc, time::Duration};

    #[derive(Clone)]
    struct Fake {
        calls: Arc<Mutex<Vec<(Method, String, Value)>>>,
        goal: Arc<Mutex<Value>>,
        fail_save: bool,
        fail_prompt: bool,
    }

    async fn respond(
        State(fake): State<Fake>,
        OriginalUri(uri): OriginalUri,
        method: Method,
        bytes: Bytes,
    ) -> Response {
        let path = uri.path();
        if path == "/v2/health" {
            return axum::Json(json!({"healthy": true, "version": "test", "providerCredentialStore": "test"})).into_response();
        }
        let body = if bytes.is_empty() { Value::Null } else { serde_json::from_slice(&bytes).unwrap() };
        fake.calls.lock().push((method.clone(), path.to_string(), body.clone()));
        if path == "/v2/plugins/dev.neoism.goals/root" {
            if method == Method::GET {
                return axum::Json(json!({"goal": *fake.goal.lock()})).into_response();
            }
            if method == Method::DELETE {
                *fake.goal.lock() = Value::Null;
                return StatusCode::NO_CONTENT.into_response();
            }
            if fake.fail_save {
                return (StatusCode::BAD_REQUEST, "goal save failed").into_response();
            }
            let goal = json!({"text": body["text"], "paused": body["paused"], "status": "active"});
            *fake.goal.lock() = goal.clone();
            return axum::Json(json!({"goal": goal})).into_response();
        }
        if path == "/v2/sessions/root" {
            return axum::Json(json!({"id": "root", "agent": "selected-agent", "model": {
                "providerId": "selected-provider", "id": "selected-model",
                "connectionId": "selected-connection", "variant": "high"
            }})).into_response();
        }
        if path == "/v2/sessions/root/prompt-async" {
            return if fake.fail_prompt {
                (StatusCode::BAD_REQUEST, "prompt admission failed").into_response()
            } else {
                StatusCode::NO_CONTENT.into_response()
            };
        }
        (StatusCode::NOT_FOUND, "unexpected endpoint").into_response()
    }

    async fn fixture(fail_save: bool, fail_prompt: bool) -> (
        AgentSession, reply_queue::AgentReplyReceiver, Fake, tokio::task::JoinHandle<()>,
    ) {
        let fake = Fake {
            calls: Arc::new(Mutex::new(Vec::new())),
            goal: Arc::new(Mutex::new(json!({"text": "saved goal", "paused": true, "status": "active"}))),
            fail_save,
            fail_prompt,
        };
        let app = axum::Router::new().fallback(respond).with_state(fake.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tx, rx) = reply_queue::channel();
        let mut session = AgentSession::spawn(Some("test".into()), String::new(), tx);
        let inner = Arc::get_mut(&mut session.inner).unwrap();
        inner.agent_server = format!("http://{address}");
        inner.agent_http = reqwest::Client::builder().no_proxy().build().unwrap();
        (session, rx, fake, server)
    }

    async fn command(session: &AgentSession, rx: &mut reply_queue::AgentReplyReceiver, text: &str) -> AgentServerMessage {
        dispatcher::dispatch(session, AgentClientMessage::SlashCommand { session_id: "root".into(), text: text.into() });
        tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap()
    }

    #[test]
    fn goal_command_boundary_and_selected_configuration() {
        assert_eq!(command_arguments(" /goal set ship it "), Some("set ship it"));
        assert_eq!(command_arguments("/goal\tresume"), Some("resume"));
        assert_eq!(command_arguments("/goal"), Some(""));
        assert_eq!(command_arguments("/goals ship it"), None);
        assert!(prompt_request(&json!({"model": {"id": "bad"}}), "ship").is_err());
    }

    #[tokio::test]
    async fn desktop_goal_pending_first_prompt_keeps_message_id_and_current_selections() {
        let (session, mut rx, fake, server) = fixture(false, false).await;
        dispatcher::dispatch(&session, AgentClientMessage::SubmitPrompt {
            session_id: "root".into(), message_id: "msg_new_goal".into(),
            text: "/goal set ship new".into(), author: Some("peer".into()), attachments: Vec::new(),
            mode: Some("draft-agent".into()), model: Some("draft-provider/draft-model".into()),
            connection_id: Some("draft-connection".into()), thinking: Some("low".into()),
            delivery: neoism_protocol::agent::PromptDelivery::Steer,
        });
        assert!(matches!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap(), AgentServerMessage::CommandOutput { .. }));
        let calls = fake.calls.lock();
        let prompts = calls.iter().filter(|(_, path, _)| path.contains("/prompt")).collect::<Vec<_>>();
        assert_eq!(prompts.len(), 1);
        let (_, path, body) = prompts[0];
        assert!(path.ends_with("/prompt-async"));
        assert_eq!(body["messageId"], "msg_new_goal");
        assert_eq!(body["agent"], "draft-agent");
        assert_eq!(body["model"], json!({"providerId": "draft-provider", "modelId": "draft-model", "connectionId": "draft-connection", "variant": "low"}));
        assert_eq!(body["author"], "peer");
        assert_eq!(body["delivery"], "queue");
        assert_eq!(body["parts"][0]["text"], "ship new");
        server.abort();
    }

    #[tokio::test]
    async fn desktop_goal_set_and_resume_save_then_admit_once_with_selected_model_agent() {
        let (session, mut rx, fake, server) = fixture(false, false).await;
        let mut ids = Vec::new();
        for (text, expected) in [
            ("/goal set ship it", "ship it"),
            ("/goal resume", "ship it"),
            ("/goal finish it", "finish it"),
            ("/goal read README", "read README"),
        ] {
            fake.calls.lock().clear();
            let reply = command(&session, &mut rx, text).await;
            assert!(matches!(reply, AgentServerMessage::CommandOutput { title, .. } if title == "Goal"));
            let calls = fake.calls.lock();
            assert!(!calls.iter().any(|(_, path, _)| path.ends_with("/commands")));
            let save = calls.iter().position(|(method, path, _)| *method == Method::POST && path == "/v2/plugins/dev.neoism.goals/root").unwrap();
            let prompts = calls.iter().enumerate().filter(|(_, (_, path, _))| path.ends_with("/prompt-async")).collect::<Vec<_>>();
            assert_eq!(prompts.len(), 1);
            let (admit, (_, _, prompt)) = prompts[0];
            assert!(save < admit);
            assert_eq!(prompt["agent"], "selected-agent");
            assert_eq!(prompt["model"]["providerId"], "selected-provider");
            assert_eq!(prompt["model"]["modelId"], "selected-model");
            assert_eq!(prompt["model"]["connectionId"], "selected-connection");
            assert_eq!(prompt["model"]["variant"], "high");
            assert_eq!(prompt["delivery"], "queue");
            assert_eq!(prompt["parts"][0]["text"], expected);
            ids.push(prompt["messageId"].as_str().unwrap().to_string());
        }
        ids.sort();
        ids.dedup();
        assert_eq!(ids.len(), 4);
        server.abort();
    }

    #[tokio::test]
    async fn desktop_goal_read_pause_clear_never_admit_work() {
        let (session, mut rx, fake, server) = fixture(false, false).await;
        for text in ["/goal", "/goal show", "/goal pause", "/goal clear", "/goal read"] {
            assert!(matches!(command(&session, &mut rx, text).await, AgentServerMessage::CommandOutput { .. }));
        }
        assert!(!fake.calls.lock().iter().any(|(_, path, _)| path.contains("/prompt") || path.ends_with("/commands")));
        // Resuming a cleared/missing goal must fail without saving/admitting.
        assert!(!matches!(command(&session, &mut rx, "/goal resume").await, AgentServerMessage::CommandOutput { .. }));
        server.abort();
    }

    #[tokio::test]
    async fn desktop_goal_failed_save_never_admits_and_failed_admission_is_not_retried() {
        let (session, mut rx, fake, server) = fixture(true, false).await;
        for text in ["/goal ship it", "/goal resume"] {
            assert!(!matches!(command(&session, &mut rx, text).await, AgentServerMessage::CommandOutput { .. }));
        }
        assert!(!fake.calls.lock().iter().any(|(_, path, _)| path.contains("/prompt")));
        server.abort();
        let (session, mut rx, fake, server) = fixture(false, true).await;
        assert!(!matches!(command(&session, &mut rx, "/goal ship it").await, AgentServerMessage::CommandOutput { .. }));
        assert_eq!(fake.calls.lock().iter().filter(|(_, path, _)| path.ends_with("/prompt-async")).count(), 1);
        server.abort();
    }

    #[test]
    fn goal_notice_protocol_projection_matches_live_and_user_history() {
        let part = json!({"id": "prt_goal_started_msg_goal", "messageId": "msg_goal", "type": "tool", "tool": "goal_started",
            "metadata": {"transcriptOnly": true}, "state": {"status": "completed", "title": "Goal Started",
                "input": {}, "output": "ship it", "time": {"start": 1200, "end": 1200}}});
        let live = super::super::history_from_part(&part).unwrap();
        let history = super::super::history_messages_from_response(&[json!({
            "info": {"id": "msg_goal", "role": "user", "sessionId": "ses_goal", "time": {"created": 1000},
                "agent": "build", "model": {"providerId": "neoism", "modelId": "stub"}},
            "parts": [{"id": "text", "type": "text", "text": "ship it"}, part]
        })]);
        assert_eq!(history.len(), 2);
        assert_eq!(history[0].kind, MessageKind::User);
        for notice in [&live, &history[1]] {
            assert_eq!(notice.id, "prt_goal_started_msg_goal");
            assert_eq!(notice.kind, MessageKind::System);
            assert_eq!(notice.title, "Goal Started");
            assert_eq!(notice.tool, "goal_started");
            assert_eq!(notice.status, "completed");
            assert_eq!(notice.created_at, 1200);
        }
    }

    #[tokio::test]
    async fn desktop_goal_native_set_resume_generate_and_project_confirmed_markers() {
        // A one-step selected agent keeps this a finite real-provider test even
        // though the goal remains active. No completion tool or network model
        // credentials are involved.
        let root = tempfile::tempdir().unwrap();
        let config = root.path().join("config");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::write(config.join("agent.json"), json!({
            "agent": {"goal-test": {"steps": 1, "mode": "primary"}}
        }).to_string()).unwrap();
        let services = neoism_agent_server::standard_services().with_config(Arc::new(
            neoism_agent_service_api::StandardConfigSourceService::new(&config),
        ));
        let state = neoism_agent_server::AppState::open_database_with_services(
            root.path().join("agent.db"), services,
        ).await.unwrap();
        let app = neoism_agent_server::app(state.clone());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let (tx, mut rx) = reply_queue::channel();
        let mut desktop = AgentSession::spawn(Some("test".into()), String::new(), tx);
        let inner = Arc::get_mut(&mut desktop.inner).unwrap();
        inner.agent_server = format!("http://{address}");
        inner.agent_http = reqwest::Client::builder().no_proxy().build().unwrap();
        let created = http_post_json(&desktop.inner, "/v2/sessions", &json!({
            "directory": workspace, "agent": "goal-test",
            "model": {"providerId": "neoism", "id": "stub"}
        })).await.unwrap();
        let session_id = created["id"].as_str().unwrap();
        let mut events = state.subscribe();
        for (index, text) in ["/goal ship it", "/goal resume"].into_iter().enumerate() {
            dispatcher::dispatch(&desktop, AgentClientMessage::SlashCommand {
                session_id: session_id.to_string(), text: text.to_string(),
            });
            let reply = tokio::time::timeout(Duration::from_secs(10), rx.recv()).await.unwrap().unwrap();
            assert!(matches!(reply, AgentServerMessage::CommandOutput { .. }), "{reply:?}");
            let live_notice = tokio::time::timeout(Duration::from_secs(10), async {
                let mut notice = None;
                let mut streamed = false;
                loop {
                    let event = events.recv().await.unwrap();
                    if event.properties["sessionID"] != session_id { continue; }
                    if event.kind == "message.part.updated"
                        && event.properties["part"]["tool"] == "goal_started"
                    {
                        assert!(notice.is_none(), "only one start marker per admitted prompt");
                        notice = super::super::history_from_part(&event.properties["part"]);
                    }
                    if event.kind == "message.part.delta" && event.properties["field"] == "text" {
                        streamed = true;
                    }
                    if event.kind == "session.status" && event.properties["status"]["type"] == "idle" {
                        assert!(streamed, "the selected stub model must actually stream");
                        break notice.expect("generation must confirm goal start");
                    }
                }
            }).await.expect("goal model run must settle");
            let (history, _) = super::super::handlers::fetch_history(&desktop.inner, session_id, None, Some(80)).await.unwrap();
            let markers = history.iter().filter(|message| message.title == "Goal Started").collect::<Vec<_>>();
            assert_eq!(markers.len(), index + 1);
            let hydrated = markers.iter().find(|message| message.id == live_notice.id).unwrap();
            assert_eq!(hydrated.kind, MessageKind::System);
            assert_eq!(hydrated.created_at, live_notice.created_at);
            assert_eq!(history.iter().filter(|message| message.kind == MessageKind::User).count(), index + 1);
            let raw = http_get_json(&desktop.inner, &format!("/v2/sessions/{session_id}/messages")).await.unwrap();
            let messages = raw.as_array().or_else(|| raw.get("items").and_then(Value::as_array)).unwrap();
            let replies = messages.iter().filter(|message| message["info"]["role"] == "assistant").collect::<Vec<_>>();
            assert_eq!(replies.len(), index + 1, "exactly one generated turn per command");
            for reply in replies {
                assert_eq!(reply["info"]["agent"], "goal-test");
                assert_eq!(reply["info"]["modelId"], "stub");
                assert_eq!(reply["info"]["providerId"], "neoism");
            }
            // Pause changes only metadata; the next resume must still run.
            dispatcher::dispatch(&desktop, AgentClientMessage::SlashCommand {
                session_id: session_id.to_string(), text: "/goal pause".into(),
            });
            assert!(matches!(tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap().unwrap(), AgentServerMessage::CommandOutput { .. }));
        }
        server.abort();
        state.shutdown().await.unwrap();
    }
}