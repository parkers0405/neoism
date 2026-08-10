use std::collections::{BTreeMap, BTreeSet};
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex as StdMutex};
use std::time::Duration;

use anyhow::Context;
use neoism_agent_core::{
    event_type, AgentInfo, CommandInfo, CreateSessionRequest, EventPayload, MessageInfo,
    MessageWithParts, ModelInfo, ModelRef, Part, PermissionRequestInfo, PromptRequest,
    ProviderInfo, ProviderListResult, QuestionRequestInfo, SessionInfo, TodoInfo,
    ToolState,
};
use neoism_agent_server::ServerOptions;
use serde::Deserialize;
use serde_json::{json, Value};

#[path = "acp_params.rs"]
mod acp_params;
use acp_params::{
    cwd_from_params, model_ref_from_user_model, parse_model_ref,
    permission_reply_from_acp_result, question_answers_from_acp_result,
    required_string_param, rpc_id_key, string_param,
};
pub(crate) use acp_params::{model_param, prompt_parts_from_acp};

const DEFAULT_VARIANT_VALUE: &str = "default";

#[derive(Debug, Deserialize)]
struct JsonRpcRequest {
    id: Option<Value>,
    method: Option<String>,
    params: Option<Value>,
    result: Option<Value>,
    error: Option<Value>,
}

struct AcpBridge {
    client: reqwest::Client,
    server: String,
    default_cwd: String,
    sessions: BTreeMap<String, String>,
    outbox: Vec<Value>,
    output: Option<mpsc::Sender<Value>>,
    event_tasks: BTreeMap<String, tokio::task::JoinHandle<()>>,
    pending_permissions: Arc<StdMutex<BTreeMap<String, String>>>,
    pending_questions: Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: Arc<AtomicU64>,
}

pub(crate) async fn acp(
    hostname: String,
    port: u16,
    server: Option<String>,
    cwd: Option<String>,
) -> anyhow::Result<()> {
    let client = reqwest::Client::new();
    let server = match server {
        Some(server) => server.trim_end_matches('/').to_string(),
        None => {
            let server = format!("http://{hostname}:{port}");
            ensure_server(&client, &server, hostname, port).await?;
            server
        }
    };
    let default_cwd = match cwd {
        Some(cwd) => cwd,
        None => std::env::current_dir()?.to_string_lossy().to_string(),
    };

    AcpBridge {
        client,
        server,
        default_cwd,
        sessions: BTreeMap::new(),
        outbox: Vec::new(),
        output: None,
        event_tasks: BTreeMap::new(),
        pending_permissions: Arc::new(StdMutex::new(BTreeMap::new())),
        pending_questions: Arc::new(StdMutex::new(BTreeMap::new())),
        request_counter: Arc::new(AtomicU64::new(1)),
    }
    .run_stdio()
    .await
}

async fn ensure_server(
    client: &reqwest::Client,
    server: &str,
    hostname: String,
    port: u16,
) -> anyhow::Result<()> {
    if health(client, server).await {
        return Ok(());
    }
    tokio::spawn(async move {
        let result = neoism_agent_server::listen(ServerOptions {
            hostname,
            port,
            cors: Vec::new(),
        })
        .await;
        if let Err(error) = result {
            eprintln!("neoism-agent ACP server exited: {error:#}");
        }
    });
    for _ in 0..50 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        if health(client, server).await {
            return Ok(());
        }
    }
    anyhow::bail!("Neoism server did not become healthy at {server}");
}

async fn health(client: &reqwest::Client, server: &str) -> bool {
    client
        .get(format!("{server}/global/health"))
        .send()
        .await
        .map(|response| response.status().is_success())
        .unwrap_or(false)
}

impl AcpBridge {
    async fn run_stdio(&mut self) -> anyhow::Result<()> {
        let (tx, rx) = mpsc::channel::<Value>();
        self.output = Some(tx.clone());
        let writer = std::thread::spawn(move || {
            let mut stdout = std::io::stdout();
            while let Ok(response) = rx.recv() {
                if writeln!(stdout, "{response}").is_err() {
                    break;
                }
                if stdout.flush().is_err() {
                    break;
                }
            }
        });

        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            for response in self.handle_line(&line).await {
                let _ = tx.send(response);
            }
        }
        self.output = None;
        for task in self.event_tasks.values() {
            task.abort();
        }
        drop(tx);
        let _ = writer.join();
        Ok(())
    }

    async fn handle_line(&mut self, line: &str) -> Vec<Value> {
        let request = match serde_json::from_str::<JsonRpcRequest>(line) {
            Ok(request) => request,
            Err(error) => {
                return vec![jsonrpc_error(
                    Value::Null,
                    -32700,
                    format!("parse error: {error}"),
                )];
            }
        };
        if request.method.is_none()
            && request.id.is_some()
            && (request.result.is_some() || request.error.is_some())
        {
            if let Err(error) = self
                .handle_response(
                    request.id.unwrap(),
                    request.result.unwrap_or(Value::Null),
                    request.error,
                )
                .await
            {
                eprintln!("neoism-agent ACP response handling failed: {error:#}");
            }
            return Vec::new();
        }
        let id = request.id.clone();
        let method = match request.method.as_deref() {
            Some(method) => method,
            None => {
                return id
                    .map(|id| vec![jsonrpc_error(id, -32600, "missing method")])
                    .unwrap_or_default();
            }
        };
        let result = self
            .handle_method(method, request.params.unwrap_or(Value::Null))
            .await;
        let mut responses = std::mem::take(&mut self.outbox);
        match (id, result) {
            (Some(id), Ok(result)) => {
                responses.push(json!({ "jsonrpc": "2.0", "id": id, "result": result }));
                responses
            }
            (Some(id), Err(error)) => {
                responses.push(jsonrpc_error(id, -32603, error.to_string()));
                responses
            }
            (None, Err(error)) => {
                eprintln!("neoism-agent ACP notification failed for {method}: {error:#}");
                responses
            }
            (None, Ok(_)) => responses,
        }
    }

    async fn handle_response(
        &self,
        id: Value,
        result: Value,
        error: Option<Value>,
    ) -> anyhow::Result<()> {
        let Some(id) = rpc_id_key(&id) else {
            return Ok(());
        };
        let permission_id = self
            .pending_permissions
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id));
        if let Some(permission_id) = permission_id {
            let reply = if error.is_some() {
                "reject".to_string()
            } else {
                permission_reply_from_acp_result(&result)
            };
            self.client
                .post(format!("{}/permission/{permission_id}/reply", self.server))
                .json(&json!({ "reply": reply }))
                .send()
                .await?
                .error_for_status()?;
            return Ok(());
        }

        let question_id = self
            .pending_questions
            .lock()
            .ok()
            .and_then(|mut pending| pending.remove(&id));
        if let Some(question_id) = question_id {
            if error.is_some() {
                self.client
                    .post(format!("{}/question/{question_id}/reject", self.server))
                    .send()
                    .await?
                    .error_for_status()?;
                return Ok(());
            }
            if let Some(answers) = question_answers_from_acp_result(&result) {
                self.client
                    .post(format!("{}/question/{question_id}/reply", self.server))
                    .json(&json!({ "answers": answers }))
                    .send()
                    .await?
                    .error_for_status()?;
            } else {
                self.client
                    .post(format!("{}/question/{question_id}/reject", self.server))
                    .send()
                    .await?
                    .error_for_status()?;
            }
        }
        Ok(())
    }

    async fn handle_method(
        &mut self,
        method: &str,
        params: Value,
    ) -> anyhow::Result<Value> {
        match method {
            "initialize" => Ok(self.initialize(params)),
            "authenticate" => Ok(json!({})),
            "session/new" | "newSession" => self.new_session(params).await,
            "session/list" | "listSessions" => self.list_sessions(params).await,
            "session/load" | "loadSession" | "session/resume" | "resumeSession" => {
                self.load_session(params).await
            }
            "session/close" | "closeSession" => self.close_session(params).await,
            "session/fork" | "forkSession" => self.fork_session(params).await,
            "session/cancel" | "cancel" => self.cancel(params).await,
            "session/prompt" | "prompt" => self.prompt(params).await,
            "session/set_config_option" | "setSessionConfigOption" => {
                self.set_config_option(params).await
            }
            "session/set_model" | "setSessionModel" | "unstable_setSessionModel" => {
                self.set_session_model(params).await
            }
            "session/set_mode" | "setSessionMode" => self.set_session_mode(params).await,
            "session/set_variant" | "setSessionVariant" | "session/set_effort" => {
                self.set_session_variant(params).await
            }
            _ => anyhow::bail!("method not found: {method}"),
        }
    }

    fn initialize(&self, params: Value) -> Value {
        initialize_response(params)
    }

    async fn new_session(&mut self, params: Value) -> anyhow::Result<Value> {
        let cwd = cwd_from_params(&params).unwrap_or_else(|| self.default_cwd.clone());
        let mut request = self.client.post(format!("{}/session", self.server));
        request = request.query(&[("directory", cwd.as_str())]);
        let info: SessionInfo = request
            .json(&CreateSessionRequest {
                parent_id: None,
                title: string_param(&params, "title"),
                agent: string_param(&params, "modeId")
                    .or_else(|| string_param(&params, "agent")),
                model: model_param(&params).map(model_ref_from_user_model),
                permission: None,
                workspace_id: None,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.sessions.insert(info.id.to_string(), cwd);
        self.ensure_event_subscription(&info.id.to_string());
        let config = self.config_options_for_session(&info).await;
        self.enqueue_available_commands(&info.id.to_string(), &info.directory)
            .await?;
        Ok(session_payload(&info, config))
    }

    async fn list_sessions(&self, params: Value) -> anyhow::Result<Value> {
        let mut request = self.client.get(format!("{}/session", self.server));
        let cwd = cwd_from_params(&params);
        if let Some(cwd) = cwd.as_deref() {
            request = request.query(&[("directory", cwd), ("roots", "true")]);
        }
        let sessions: Vec<SessionInfo> =
            request.send().await?.error_for_status()?.json().await?;
        let entries = sessions
            .into_iter()
            .map(|session| {
                json!({
                    "sessionId": session.id,
                    "cwd": session.directory,
                    "title": session.title,
                    "updatedAt": session.time.updated.to_string()
                })
            })
            .collect::<Vec<_>>();
        Ok(json!({ "sessions": entries }))
    }

    async fn load_session(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        let info: SessionInfo = self
            .client
            .get(format!("{}/session/{}", self.server, session_id))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        let cwd = cwd_from_params(&params).unwrap_or_else(|| info.directory.clone());
        self.sessions.insert(session_id, cwd);
        self.ensure_event_subscription(&info.id.to_string());
        let config = self.config_options_for_session(&info).await;
        self.enqueue_available_commands(&info.id.to_string(), &info.directory)
            .await?;
        self.enqueue_session_history(&info.id.to_string(), None)
            .await?;
        Ok(session_payload(&info, config))
    }

    async fn close_session(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        self.abort_session(&session_id).await?;
        self.sessions.remove(&session_id);
        if let Some(task) = self.event_tasks.remove(&session_id) {
            task.abort();
        }
        Ok(json!({}))
    }

    async fn fork_session(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        let mut body = json!({});
        if let Some(message_id) = string_param(&params, "messageId")
            .or_else(|| string_param(&params, "messageID"))
        {
            body["messageId"] = Value::String(message_id);
        }
        let info: SessionInfo = self
            .client
            .post(format!("{}/session/{session_id}/fork", self.server))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.sessions
            .insert(info.id.to_string(), info.directory.clone());
        self.ensure_event_subscription(&info.id.to_string());
        let config = self.config_options_for_session(&info).await;
        self.enqueue_available_commands(&info.id.to_string(), &info.directory)
            .await?;
        self.enqueue_session_history(&info.id.to_string(), None)
            .await?;
        Ok(session_payload(&info, config))
    }

    async fn cancel(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        self.abort_session(&session_id).await?;
        Ok(json!({}))
    }

    async fn abort_session(&self, session_id: &str) -> anyhow::Result<()> {
        self.client
            .post(format!("{}/session/{session_id}/abort", self.server))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    async fn set_session_model(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        let current = self.fetch_session(&session_id).await?;
        let providers = self.provider_list().await;
        let model = model_param(&params)
            .map(model_ref_from_user_model)
            .or_else(|| {
                string_param(&params, "modelId")
                    .or_else(|| string_param(&params, "modelID"))
                    .and_then(|value| select_model_ref(&value, providers.as_ref()))
            })
            .with_context(|| "model or modelId is required")?;
        let info = self
            .patch_session(&session_id, json!({ "model": model }))
            .await?;
        self.sessions
            .entry(session_id.clone())
            .or_insert(current.directory);
        let config = self.config_options_for_session(&info).await;
        Ok(session_payload(&info, config))
    }

    async fn set_session_mode(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        let mode = string_param(&params, "modeId")
            .or_else(|| string_param(&params, "modeID"))
            .or_else(|| string_param(&params, "agent"))
            .with_context(|| "modeId is required")?;
        let info = self
            .patch_session(&session_id, json!({ "agent": mode }))
            .await?;
        let config = self.config_options_for_session(&info).await;
        Ok(session_payload(&info, config))
    }

    async fn set_session_variant(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        let variant = string_param(&params, "variant")
            .or_else(|| string_param(&params, "effort"))
            .or_else(|| string_param(&params, "value"))
            .with_context(|| "variant or value is required")?;
        let info = self.update_session_variant(&session_id, &variant).await?;
        let config = self.config_options_for_session(&info).await;
        Ok(session_payload(&info, config))
    }

    async fn set_config_option(&mut self, params: Value) -> anyhow::Result<Value> {
        if let Some(session_id) = string_param(&params, "sessionId") {
            if let Some(config_id) = config_id_param(&params) {
                let value = config_value_param(&params).with_context(|| {
                    format!("value is required for config option {config_id}")
                })?;
                let info = match config_id.as_str() {
                    "model" => {
                        let providers = self.provider_list().await;
                        let model = select_model_ref(&value, providers.as_ref())
                            .with_context(|| format!("invalid model value: {value}"))?;
                        self.patch_session(&session_id, json!({ "model": model }))
                            .await?
                    }
                    "effort" | "variant" => {
                        self.update_session_variant(&session_id, &value).await?
                    }
                    "mode" | "agent" => {
                        self.patch_session(&session_id, json!({ "agent": value }))
                            .await?
                    }
                    other => anyhow::bail!("unknown config option: {other}"),
                };
                let config = self.config_options_for_session(&info).await;
                self.outbox.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": "config_option_update",
                        "configOptions": config.get("configOptions").cloned().unwrap_or(Value::Array(Vec::new()))
                    }),
                ));
                return Ok(config);
            }
            let info: SessionInfo = self
                .client
                .get(format!("{}/session/{}", self.server, session_id))
                .send()
                .await?
                .error_for_status()?
                .json()
                .await?;
            return Ok(self.config_options_for_session(&info).await);
        }
        let cwd = cwd_from_params(&params).unwrap_or_else(|| self.default_cwd.clone());
        Ok(self.config_options_for_cwd(&cwd).await)
    }

    async fn update_session_variant(
        &self,
        session_id: &str,
        variant: &str,
    ) -> anyhow::Result<SessionInfo> {
        let current = self.fetch_session(session_id).await?;
        let mut model = current
            .model
            .clone()
            .with_context(|| "session has no selected model to set a variant on")?;
        let providers = self.provider_list().await;
        let available = model_variants_for_ref(&model, providers.as_ref());
        let normalized = normalize_variant(variant);
        if !available.is_empty() && !available.contains(&normalized) {
            anyhow::bail!("variant not found for selected model: {variant}");
        }
        model.variant = (normalized != DEFAULT_VARIANT_VALUE).then_some(normalized);
        self.patch_session(session_id, json!({ "model": model }))
            .await
    }

    async fn patch_session(
        &self,
        session_id: &str,
        body: Value,
    ) -> anyhow::Result<SessionInfo> {
        Ok(self
            .client
            .patch(format!("{}/session/{session_id}", self.server))
            .json(&body)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn fetch_session(&self, session_id: &str) -> anyhow::Result<SessionInfo> {
        Ok(self
            .client
            .get(format!("{}/session/{session_id}", self.server))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn provider_list(&self) -> Option<ProviderListResult> {
        let response = self
            .client
            .get(format!("{}/provider", self.server))
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?;
        response.json::<ProviderListResult>().await.ok()
    }

    async fn prompt(&mut self, params: Value) -> anyhow::Result<Value> {
        let session_id = required_string_param(&params, "sessionId")?;
        self.ensure_event_subscription(&session_id);
        let parts = prompt_parts_from_acp(&params)?;
        let model = model_param(&params);
        let agent =
            string_param(&params, "modeId").or_else(|| string_param(&params, "agent"));
        let response: MessageWithParts = self
            .client
            .post(format!("{}/session/{session_id}/message", self.server))
            .json(&PromptRequest {
                message_id: None,
                model,
                agent,
                no_reply: false,
                system: None,
                tools: None,
                author: None,
                parts,
            })
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.enqueue_message_updates(&response);
        let usage = usage_from_message(&response);
        self.outbox.push(session_update_notification(
            &session_id,
            json!({
                "sessionUpdate": "usage_update",
                "usage": usage
            }),
        ));
        Ok(json!({
            "stopReason": "end_turn",
            "usage": usage,
            "_meta": {
                "neoism": {
                    "message": response,
                    "cwd": self.sessions.get(&session_id).cloned().unwrap_or_else(|| self.default_cwd.clone())
                }
            }
        }))
    }

    async fn enqueue_session_history(
        &mut self,
        session_id: &str,
        limit: Option<usize>,
    ) -> anyhow::Result<()> {
        let mut request = self
            .client
            .get(format!("{}/session/{session_id}/message", self.server));
        if let Some(limit) = limit {
            request = request.query(&[("limit", limit.to_string())]);
        }
        let messages: Vec<MessageWithParts> =
            request.send().await?.error_for_status()?.json().await?;
        for message in &messages {
            self.enqueue_message_updates(&message);
        }
        if let Some(last) = messages
            .iter()
            .rev()
            .find(|message| matches!(message.info, MessageInfo::Assistant(_)))
        {
            let usage = usage_from_message(last);
            if !usage.is_null() {
                self.outbox.push(session_update_notification(
                    session_id,
                    json!({
                        "sessionUpdate": "usage_update",
                        "usage": usage
                    }),
                ));
            }
        }
        Ok(())
    }

    fn ensure_event_subscription(&mut self, session_id: &str) {
        if self.event_tasks.contains_key(session_id) {
            return;
        }
        let Some(output) = self.output.clone() else {
            return;
        };
        let client = self.client.clone();
        let server = self.server.clone();
        let session_id = session_id.to_string();
        let pending_permissions = self.pending_permissions.clone();
        let pending_questions = self.pending_questions.clone();
        let request_counter = self.request_counter.clone();
        let task_session_id = session_id.clone();
        let task = tokio::spawn(async move {
            run_acp_event_subscription(
                client,
                server,
                task_session_id,
                output,
                pending_permissions,
                pending_questions,
                request_counter,
            )
            .await;
        });
        self.event_tasks.insert(session_id.to_string(), task);
    }

    fn enqueue_message_updates(&mut self, message: &MessageWithParts) {
        self.outbox.extend(acp_updates_for_message(message));
    }

    async fn enqueue_available_commands(
        &mut self,
        session_id: &str,
        cwd: &str,
    ) -> anyhow::Result<()> {
        let commands: Vec<CommandInfo> = self
            .client
            .get(format!("{}/command", self.server))
            .query(&[("directory", cwd)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        self.outbox
            .push(available_commands_notification(session_id, commands));
        Ok(())
    }

    async fn config_options_for_cwd(&self, cwd: &str) -> Value {
        let providers = self.provider_list().await;
        let mut agents_request = self.client.get(format!("{}/agent", self.server));
        agents_request = agents_request.query(&[("directory", cwd)]);
        let agents = agents_request
            .send()
            .await
            .ok()
            .and_then(|response| response.error_for_status().ok());
        let agents = match agents {
            Some(response) => response.json::<Vec<AgentInfo>>().await.ok(),
            None => None,
        };
        let models = providers.map(models_from_providers).unwrap_or_default();
        let modes = agents.map(modes_from_agents).unwrap_or_default();
        build_config_options_payload(models, modes, None, None)
    }

    async fn config_options_for_session(&self, info: &SessionInfo) -> Value {
        let payload = self.config_options_for_cwd(&info.directory).await;
        let models = payload
            .get("_meta")
            .and_then(|meta| meta.get("availableModels"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let modes = payload
            .get("_meta")
            .and_then(|meta| meta.get("availableModes"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let current_model = info
            .model
            .as_ref()
            .map(|model| format!("{}/{}", model.provider_id, model.id));
        build_config_options_payload_with_variant(
            models,
            modes,
            current_model,
            info.model.as_ref().and_then(|model| model.variant.clone()),
            info.agent.clone(),
        )
    }
}

async fn run_acp_event_subscription(
    client: reqwest::Client,
    server: String,
    session_id: String,
    output: mpsc::Sender<Value>,
    pending_permissions: Arc<StdMutex<BTreeMap<String, String>>>,
    pending_questions: Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: Arc<AtomicU64>,
) {
    let mut delay = Duration::from_millis(250);
    loop {
        let mut live = AcpLiveState::default();
        let result = run_acp_event_stream(
            &client,
            &server,
            &session_id,
            &output,
            &mut live,
            &pending_permissions,
            &pending_questions,
            &request_counter,
        )
        .await;
        if let Err(error) = result {
            eprintln!("neoism-agent ACP event stream for {session_id} failed: {error:#}");
        }
        tokio::time::sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(5));
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_acp_event_stream(
    client: &reqwest::Client,
    server: &str,
    session_id: &str,
    output: &mpsc::Sender<Value>,
    live: &mut AcpLiveState,
    pending_permissions: &Arc<StdMutex<BTreeMap<String, String>>>,
    pending_questions: &Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: &Arc<AtomicU64>,
) -> anyhow::Result<()> {
    let mut response = client
        .get(format!("{server}/global/event"))
        .query(&[("sessionID", session_id), ("limit", "0")])
        .send()
        .await?
        .error_for_status()?;
    let mut buffer = String::new();
    while let Some(chunk) = response.chunk().await? {
        buffer.push_str(&String::from_utf8_lossy(&chunk));
        while let Some(frame) = take_sse_frame(&mut buffer) {
            let Some(event) = parse_sse_event(&frame) else {
                continue;
            };
            for response in acp_live_outputs_for_event(
                &event,
                session_id,
                live,
                pending_permissions,
                pending_questions,
                request_counter,
            ) {
                let _ = output.send(response);
            }
        }
    }
    Ok(())
}

fn take_sse_frame(buffer: &mut String) -> Option<String> {
    let marker = buffer
        .find("\n\n")
        .map(|index| (index, 2))
        .or_else(|| buffer.find("\r\n\r\n").map(|index| (index, 4)))?;
    let frame = buffer[..marker.0].to_string();
    buffer.drain(..marker.0 + marker.1);
    Some(frame)
}

fn parse_sse_event(frame: &str) -> Option<EventPayload<Value>> {
    let data = frame
        .lines()
        .filter_map(|line| line.strip_prefix("data:").map(str::trim_start))
        .collect::<Vec<_>>()
        .join("\n");
    if data.trim().is_empty() {
        return None;
    }
    serde_json::from_str(&data).ok()
}

#[derive(Default)]
pub(crate) struct AcpLiveState {
    part_kinds: BTreeMap<String, String>,
    tool_starts: BTreeSet<String>,
}

pub(crate) fn acp_live_outputs_for_event(
    event: &EventPayload<Value>,
    session_id: &str,
    live: &mut AcpLiveState,
    pending_permissions: &Arc<StdMutex<BTreeMap<String, String>>>,
    pending_questions: &Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: &Arc<AtomicU64>,
) -> Vec<Value> {
    if !event_matches_session(&event.properties, session_id) {
        return Vec::new();
    }
    match event.kind.as_str() {
        event_type::MESSAGE_PART_DELTA => acp_live_delta_update(event, live),
        event_type::MESSAGE_PART_UPDATED => acp_live_part_update(event, live),
        event_type::MESSAGE_UPDATED => acp_live_message_update(event),
        event_type::SESSION_STATUS => acp_live_status_update(event),
        event_type::SESSION_QUEUE_UPDATED => acp_live_queue_update(event),
        event_type::SESSION_COMPACTED => acp_live_compacted_update(event),
        event_type::SESSION_ERROR => acp_live_error_update(event),
        event_type::TODO_UPDATED => acp_live_todo_update(event),
        event_type::PERMISSION_ASKED => {
            acp_live_permission_request(event, pending_permissions, request_counter)
        }
        event_type::QUESTION_ASKED => {
            acp_live_question_request(event, pending_questions, request_counter)
        }
        _ => Vec::new(),
    }
}

fn acp_live_status_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "status",
            "status": event.properties.get("status").cloned().unwrap_or(Value::Null),
            "queue": event.properties.get("queue").cloned().unwrap_or(Value::Null),
        }),
    )]
}

fn acp_live_queue_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "queue",
            "action": event.properties.get("action").cloned().unwrap_or(Value::Null),
            "removed": event.properties.get("removed").cloned().unwrap_or(Value::Null),
            "queue": event.properties.get("queue").cloned().unwrap_or(Value::Null),
        }),
    )]
}

fn acp_live_compacted_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "context_compacted",
            "summary": event.properties.get("summary").cloned().unwrap_or(Value::Null),
        }),
    )]
}

fn acp_live_error_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "error",
            "error": event.properties.get("error").cloned().unwrap_or(Value::Null),
        }),
    )]
}

fn acp_live_delta_update(
    event: &EventPayload<Value>,
    live: &mut AcpLiveState,
) -> Vec<Value> {
    let props = &event.properties;
    if props.get("field").and_then(Value::as_str) != Some("text") {
        return Vec::new();
    }
    let delta = props
        .get("delta")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if delta.is_empty() {
        return Vec::new();
    }
    let session_id = event_session_id(props).unwrap_or_default();
    let message_id = props
        .get("messageID")
        .or_else(|| props.get("messageId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let part_id = props
        .get("partID")
        .or_else(|| props.get("partId"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let session_update = match live.part_kinds.get(part_id).map(String::as_str) {
        Some("reasoning") => "agent_thought_chunk",
        _ => "agent_message_chunk",
    };
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": session_update,
            "messageId": message_id,
            "content": { "type": "text", "text": delta }
        }),
    )]
}

fn acp_live_part_update(
    event: &EventPayload<Value>,
    live: &mut AcpLiveState,
) -> Vec<Value> {
    let Some(part_value) = event.properties.get("part") else {
        return Vec::new();
    };
    let Some(part_id) = part_value
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
    else {
        return Vec::new();
    };
    if let Some(kind) = part_value.get("type").and_then(Value::as_str) {
        live.part_kinds.insert(part_id, kind.to_string());
    }
    let Ok(part) = serde_json::from_value::<Part>(part_value.clone()) else {
        return Vec::new();
    };
    let Part::Tool(part) = part else {
        return Vec::new();
    };
    let session_id = part.session_id.to_string();
    let mut updates = Vec::new();
    if live.tool_starts.insert(part.call_id.clone()) {
        updates.push(session_update_notification(
            &session_id,
            json!({
                "sessionUpdate": "tool_call",
                "toolCallId": part.call_id,
                "title": part.tool,
                "kind": acp_tool_kind(&part.tool),
                "status": "pending",
                "locations": [],
                "rawInput": {}
            }),
        ));
    }
    if matches!(
        part.state,
        ToolState::Completed { .. } | ToolState::Error { .. }
    ) {
        live.tool_starts.remove(&part.call_id);
    }
    updates.push(session_update_notification(
        &session_id,
        acp_tool_update(&part.tool, &part.call_id, &part.state),
    ));
    updates
}

fn acp_live_message_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(info) = event.properties.get("info") else {
        return Vec::new();
    };
    if info.get("role").and_then(Value::as_str) != Some("assistant") {
        return Vec::new();
    }
    if info
        .get("time")
        .and_then(|time| time.get("completed"))
        .is_none_or(Value::is_null)
    {
        return Vec::new();
    }
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    let usage = usage_from_assistant_info(info);
    if usage.is_null() {
        return Vec::new();
    }
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "usage_update",
            "usage": usage
        }),
    )]
}

fn acp_live_todo_update(event: &EventPayload<Value>) -> Vec<Value> {
    let Some(session_id) = event_session_id(&event.properties) else {
        return Vec::new();
    };
    let todos = event
        .properties
        .get("todos")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let entries = todos
        .into_iter()
        .filter_map(|todo| serde_json::from_value::<TodoInfo>(todo).ok())
        .map(|todo| {
            json!({
                "priority": todo.priority,
                "status": if todo.status == "cancelled" { "completed" } else { todo.status.as_str() },
                "content": todo.content
            })
        })
        .collect::<Vec<_>>();
    vec![session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "plan",
            "entries": entries
        }),
    )]
}

fn acp_live_permission_request(
    event: &EventPayload<Value>,
    pending_permissions: &Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: &Arc<AtomicU64>,
) -> Vec<Value> {
    let Ok(permission) =
        serde_json::from_value::<PermissionRequestInfo>(event.properties.clone())
    else {
        return Vec::new();
    };
    let request_rpc_id = next_acp_request_id("permission", request_counter);
    if let Ok(mut pending) = pending_permissions.lock() {
        pending.insert(request_rpc_id.clone(), permission.id.clone());
    }
    vec![permission_request_jsonrpc(&request_rpc_id, &permission)]
}

fn acp_live_question_request(
    event: &EventPayload<Value>,
    pending_questions: &Arc<StdMutex<BTreeMap<String, String>>>,
    request_counter: &Arc<AtomicU64>,
) -> Vec<Value> {
    let Ok(question) =
        serde_json::from_value::<QuestionRequestInfo>(event.properties.clone())
    else {
        return Vec::new();
    };
    let request_rpc_id = next_acp_request_id("question", request_counter);
    if let Ok(mut pending) = pending_questions.lock() {
        pending.insert(request_rpc_id.clone(), question.id.clone());
    }
    vec![json!({
        "jsonrpc": "2.0",
        "id": request_rpc_id,
        "method": "session/request_question",
        "params": {
            "sessionId": question.session_id,
            "questionId": question.id,
            "messageId": question.message_id,
            "questions": question.questions
        }
    })]
}

fn permission_request_jsonrpc(id: &str, permission: &PermissionRequestInfo) -> Value {
    let metadata = permission.metadata.clone().unwrap_or_else(|| json!({}));
    let tool_name = metadata
        .get("tool")
        .and_then(Value::as_str)
        .unwrap_or(&permission.permission);
    let input = metadata.get("input").unwrap_or(&metadata);
    let tool_call_id = permission
        .tool
        .as_ref()
        .and_then(|tool| tool.get("callID").or_else(|| tool.get("callId")))
        .and_then(Value::as_str)
        .unwrap_or(&permission.id);
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/request_permission",
        "params": {
            "sessionId": permission.session_id,
            "toolCall": {
                "toolCallId": tool_call_id,
                "status": "pending",
                "title": permission.title,
                "kind": acp_tool_kind(tool_name),
                "locations": acp_tool_locations(tool_name, input),
                "rawInput": metadata
            },
            "options": [
                { "optionId": "once", "kind": "allow_once", "name": "Allow once" },
                { "optionId": "always", "kind": "allow_always", "name": "Always allow" },
                { "optionId": "reject", "kind": "reject_once", "name": "Reject" }
            ]
        }
    })
}

fn event_matches_session(properties: &Value, expected: &str) -> bool {
    event_session_id(properties)
        .map(|session_id| session_id == expected)
        .unwrap_or(false)
}

fn event_session_id(properties: &Value) -> Option<&str> {
    properties
        .get("sessionID")
        .or_else(|| properties.get("sessionId"))
        .and_then(Value::as_str)
        .or_else(|| {
            properties
                .get("part")
                .and_then(|part| part.get("sessionID").or_else(|| part.get("sessionId")))
                .and_then(Value::as_str)
        })
        .or_else(|| {
            properties
                .get("info")
                .and_then(|info| info.get("sessionID").or_else(|| info.get("sessionId")))
                .and_then(Value::as_str)
        })
}

fn next_acp_request_id(prefix: &str, counter: &AtomicU64) -> String {
    let id = counter.fetch_add(1, Ordering::SeqCst);
    format!("neoism_{prefix}_{id}")
}

pub(crate) fn initialize_response(params: Value) -> Value {
    let mut auth_method = json!({
        "id": "neoism-login",
        "name": "Neoism Login",
        "description": "Use Neoism provider auth routes or `neoism-agent run` with configured provider credentials."
    });
    if params
        .pointer("/clientCapabilities/_meta/terminal-auth")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        auth_method["_meta"] = json!({
            "terminal-auth": {
                "command": "neoism-agent",
                "args": ["auth", "login-codex"],
                "label": "Neoism Login"
            }
        });
    }
    json!({
        "protocolVersion": 1,
        "agentCapabilities": {
            "loadSession": true,
            "mcpCapabilities": { "http": true, "sse": true },
            "promptCapabilities": { "embeddedContext": true, "image": true },
            "sessionCapabilities": {
                "close": {},
                "cancel": {},
                "fork": {},
                "list": {},
                "resume": {},
                "setModel": {},
                "setMode": {},
                "queue": {},
                "status": {}
            }
        },
        "authMethods": [auth_method],
        "agentInfo": {
            "name": "Neoism",
            "version": env!("CARGO_PKG_VERSION")
        }
    })
}

pub(crate) fn available_commands_notification(
    session_id: &str,
    mut commands: Vec<CommandInfo>,
) -> Value {
    if !commands.iter().any(|command| command.name == "compact") {
        commands.push(CommandInfo {
            name: "compact".to_string(),
            description: Some("Compact the session".to_string()),
            template: None,
            agent: None,
            model: None,
            subtask: None,
        });
    }
    commands.sort_by(|left, right| left.name.cmp(&right.name));
    session_update_notification(
        session_id,
        json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": commands.into_iter().map(|command| {
                json!({
                    "name": command.name,
                    "description": command.description.unwrap_or_default()
                })
            }).collect::<Vec<_>>()
        }),
    )
}

fn session_update_notification(session_id: &str, update: Value) -> Value {
    json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {
            "sessionId": session_id,
            "update": update
        }
    })
}

pub(crate) fn acp_updates_for_message(message: &MessageWithParts) -> Vec<Value> {
    let (session_id, message_id, role) = match &message.info {
        MessageInfo::User(info) => (
            info.session_id.to_string(),
            info.id.to_string(),
            "user_message_chunk",
        ),
        MessageInfo::Assistant(info) => (
            info.session_id.to_string(),
            info.id.to_string(),
            "agent_message_chunk",
        ),
    };
    let mut updates = Vec::new();
    for part in &message.parts {
        match part {
            Part::Text(part) if !part.text.is_empty() => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": role,
                        "messageId": message_id,
                        "content": {
                            "type": "text",
                            "text": part.text
                        }
                    }),
                ));
            }
            Part::Reasoning(part) if !part.text.is_empty() => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": "agent_thought_chunk",
                        "messageId": message_id,
                        "content": {
                            "type": "text",
                            "text": part.text
                        }
                    }),
                ));
            }
            Part::Subtask(part) => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": role,
                        "messageId": message_id,
                        "content": {
                            "type": "text",
                            "text": format!("[subtask:{}] {}", part.agent, part.prompt)
                        }
                    }),
                ));
            }
            Part::Agent(part) => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": role,
                        "messageId": message_id,
                        "content": {
                            "type": "text",
                            "text": format!("[agent:{}]", part.name)
                        }
                    }),
                ));
            }
            Part::Tool(part) => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": "tool_call",
                        "toolCallId": part.call_id,
                        "title": part.tool,
                        "kind": acp_tool_kind(&part.tool),
                        "status": "pending",
                        "locations": [],
                        "rawInput": {}
                    }),
                ));
                updates.push(session_update_notification(
                    &session_id,
                    acp_tool_update(&part.tool, &part.call_id, &part.state),
                ));
            }
            Part::File(part) => {
                updates.push(session_update_notification(
                    &session_id,
                    json!({
                        "sessionUpdate": role,
                        "messageId": message_id,
                        "content": {
                            "type": "resource_link",
                            "uri": part.url,
                            "name": part.filename.clone().unwrap_or_else(|| "file".to_string()),
                            "mimeType": part.mime
                        }
                    }),
                ));
            }
            _ => {}
        }
    }
    updates
}

fn acp_tool_update(tool: &str, call_id: &str, state: &ToolState) -> Value {
    match state {
        ToolState::Pending { input, .. } => json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": call_id,
            "status": "pending",
            "kind": acp_tool_kind(tool),
            "title": tool,
            "locations": acp_tool_locations(tool, input),
            "rawInput": input
        }),
        ToolState::Running { input, .. } => json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": call_id,
            "status": "in_progress",
            "kind": acp_tool_kind(tool),
            "title": tool,
            "locations": acp_tool_locations(tool, input),
            "rawInput": input
        }),
        ToolState::Completed {
            input,
            output,
            metadata,
            title,
            ..
        } => {
            let mut content = vec![json!({
                "type": "content",
                "content": {
                    "type": "text",
                    "text": output
                }
            })];
            if acp_tool_kind(tool) == "edit" {
                if let Some(diff) = acp_diff_content(input) {
                    content.push(diff);
                }
            }
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": call_id,
                "status": "completed",
                "kind": acp_tool_kind(tool),
                "title": title,
                "locations": acp_tool_locations(tool, input),
                "rawInput": input,
                "rawOutput": {
                    "output": output,
                    "metadata": metadata
                },
                "content": content
            })
        }
        ToolState::Error { input, error, .. } => json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": call_id,
            "status": "failed",
            "kind": acp_tool_kind(tool),
            "title": tool,
            "locations": acp_tool_locations(tool, input),
            "rawInput": input,
            "rawOutput": {
                "error": error
            },
            "content": [{
                "type": "content",
                "content": {
                    "type": "text",
                    "text": error
                }
            }]
        }),
    }
}

fn acp_tool_kind(tool: &str) -> &'static str {
    match tool.to_ascii_lowercase().as_str() {
        "bash" => "execute",
        "webfetch" | "websearch" => "fetch",
        "edit" | "patch" | "write" | "apply_patch" => "edit",
        "grep" | "glob" => "search",
        "read" => "read",
        _ => "other",
    }
}

fn acp_tool_locations(tool: &str, input: &Value) -> Vec<Value> {
    let key = match tool.to_ascii_lowercase().as_str() {
        "read" | "edit" | "write" => Some("filePath"),
        "apply_patch" | "patch" => None,
        "glob" | "grep" => Some("path"),
        _ => None,
    };
    key.and_then(|key| input.get(key).or_else(|| input.get("path")))
        .and_then(Value::as_str)
        .map(|path| vec![json!({ "path": path })])
        .unwrap_or_default()
}

fn acp_diff_content(input: &Value) -> Option<Value> {
    let path = input
        .get("filePath")
        .or_else(|| input.get("path"))
        .and_then(Value::as_str)?;
    let old_text = input
        .get("oldString")
        .or_else(|| input.get("old"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    let new_text = input
        .get("newString")
        .or_else(|| input.get("new"))
        .or_else(|| input.get("content"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Some(json!({
        "type": "diff",
        "path": path,
        "oldText": old_text,
        "newText": new_text
    }))
}

fn config_id_param(params: &Value) -> Option<String> {
    string_param(params, "configId")
        .or_else(|| string_param(params, "configID"))
        .or_else(|| string_param(params, "config_id"))
        .or_else(|| string_param(params, "id"))
        .or_else(|| string_param(params, "optionId"))
}

fn config_value_param(params: &Value) -> Option<String> {
    params
        .get("value")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| string_param(params, "modelId"))
        .or_else(|| string_param(params, "modelID"))
        .or_else(|| string_param(params, "modeId"))
        .or_else(|| string_param(params, "modeID"))
        .or_else(|| string_param(params, "variant"))
        .or_else(|| string_param(params, "effort"))
}

fn select_model_ref(
    value: &str,
    providers: Option<&ProviderListResult>,
) -> Option<ModelRef> {
    let model = parse_model_ref(value)?;
    let mut model_ref = model_ref_from_user_model(model);
    let Some(providers) = providers else {
        return Some(model_ref);
    };
    let Some(provider) = providers
        .all
        .iter()
        .find(|provider| provider.id == model_ref.provider_id)
    else {
        return Some(model_ref);
    };
    if provider.models.contains_key(&model_ref.id) {
        return Some(model_ref);
    }
    if let Some((base, variant)) = model_ref
        .id
        .rsplit_once('/')
        .map(|(base, variant)| (base.to_string(), variant.to_string()))
    {
        if provider
            .models
            .get(&base)
            .and_then(|model| model.variants.as_ref())
            .is_some_and(|variants| variants.contains_key(&variant))
        {
            model_ref.id = base;
            model_ref.variant = (variant != DEFAULT_VARIANT_VALUE).then_some(variant);
        }
    }
    Some(model_ref)
}

fn model_variants_for_ref(
    model: &ModelRef,
    providers: Option<&ProviderListResult>,
) -> Vec<String> {
    providers
        .and_then(|providers| {
            providers
                .all
                .iter()
                .find(|provider| provider.id == model.provider_id)
        })
        .and_then(|provider| provider.models.get(&model.id))
        .and_then(|model| model.variants.as_ref())
        .map(|variants| variants.keys().cloned().collect())
        .unwrap_or_default()
}

fn normalize_variant(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case(DEFAULT_VARIANT_VALUE) {
        DEFAULT_VARIANT_VALUE.to_string()
    } else {
        value.to_string()
    }
}

fn models_from_providers(providers: ProviderListResult) -> Vec<Value> {
    let mut models = Vec::new();
    for provider in providers.all {
        for model in provider.models.values() {
            models.push(model_option(&provider, model));
        }
    }
    models.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    models
}

fn model_option(provider: &ProviderInfo, model: &ModelInfo) -> Value {
    let variants = model
        .variants
        .as_ref()
        .map(|variants| variants.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    json!({
        "modelId": format!("{}/{}", provider.id, model.id),
        "name": format!("{} / {}", provider.name, model.name),
        "_meta": {
            "providerId": provider.id,
            "modelId": model.id,
            "context": model.limit.context,
            "variants": variants
        }
    })
}

fn modes_from_agents(agents: Vec<AgentInfo>) -> Vec<Value> {
    agents
        .into_iter()
        .filter(|agent| !agent.hidden)
        .map(|agent| {
            json!({
                "id": agent.name,
                "name": agent.name,
                "description": agent.description.unwrap_or_default(),
                "_meta": {
                    "mode": agent.mode,
                    "native": agent.native
                }
            })
        })
        .collect()
}

fn session_payload(info: &SessionInfo, config: Value) -> Value {
    json!({
        "sessionId": info.id,
        "cwd": info.directory,
        "title": info.title,
        "configOptions": config.get("configOptions").cloned().unwrap_or(Value::Array(Vec::new())),
        "models": config.get("models").cloned().unwrap_or(Value::Array(Vec::new())),
        "modes": config.get("modes").cloned().unwrap_or(Value::Array(Vec::new())),
        "_meta": {
            "neoism": {
                "session": info
            }
        }
    })
}

pub(crate) fn build_config_options_payload(
    models: Vec<Value>,
    modes: Vec<Value>,
    current_model: Option<String>,
    current_mode: Option<String>,
) -> Value {
    build_config_options_payload_with_variant(
        models,
        modes,
        current_model,
        None,
        current_mode,
    )
}

pub(crate) fn build_config_options_payload_with_variant(
    models: Vec<Value>,
    modes: Vec<Value>,
    current_model: Option<String>,
    current_variant: Option<String>,
    current_mode: Option<String>,
) -> Value {
    let current_model = current_model.or_else(|| {
        models
            .first()
            .and_then(|model| model.get("modelId"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });
    let current_mode = current_mode.or_else(|| {
        modes
            .first()
            .and_then(|mode| mode.get("id"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
    });
    let mut config_options = Vec::new();
    if let Some(current_model) = current_model.as_deref() {
        config_options.push(json!({
            "id": "model",
            "name": "Model",
            "category": "model",
            "type": "select",
            "currentValue": current_model,
            "options": models
                .iter()
                .filter_map(|model| {
                    Some(json!({
                        "value": model.get("modelId")?.as_str()?,
                        "name": model.get("name").and_then(Value::as_str).unwrap_or_else(|| model.get("modelId").and_then(Value::as_str).unwrap_or("Model"))
                    }))
                })
                .collect::<Vec<_>>()
        }));
    }
    let available_variants = current_model
        .as_deref()
        .and_then(|current_model| {
            models.iter().find(|model| {
                model.get("modelId").and_then(Value::as_str) == Some(current_model)
            })
        })
        .and_then(|model| model.pointer("/_meta/variants"))
        .and_then(Value::as_array)
        .map(|variants| {
            variants
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if !available_variants.is_empty() {
        let selected_variant = current_variant
            .filter(|variant| available_variants.contains(variant))
            .or_else(|| {
                available_variants
                    .iter()
                    .any(|variant| variant == DEFAULT_VARIANT_VALUE)
                    .then(|| DEFAULT_VARIANT_VALUE.to_string())
            })
            .unwrap_or_else(|| available_variants[0].clone());
        config_options.push(json!({
            "id": "effort",
            "name": "Effort",
            "description": "Available effort levels for this model",
            "category": "thought_level",
            "type": "select",
            "currentValue": selected_variant,
            "options": available_variants
                .iter()
                .map(|variant| json!({ "value": variant, "name": format_variant_name(variant) }))
                .collect::<Vec<_>>()
        }));
    }
    if let Some(current_mode) = current_mode.as_deref() {
        config_options.push(json!({
            "id": "mode",
            "name": "Session Mode",
            "category": "mode",
            "type": "select",
            "currentValue": current_mode,
            "options": modes
                .iter()
                .filter_map(|mode| {
                    Some(json!({
                        "value": mode.get("id")?.as_str()?,
                        "name": mode.get("name").and_then(Value::as_str).unwrap_or_else(|| mode.get("id").and_then(Value::as_str).unwrap_or("Mode")),
                        "description": mode.get("description").and_then(Value::as_str).unwrap_or_default()
                    }))
                })
                .collect::<Vec<_>>()
        }));
    }
    json!({
        "configOptions": config_options,
        "models": {
            "currentModelId": current_model,
            "availableModels": models
        },
        "modes": current_mode.as_ref().map(|current_mode| {
            json!({
                "currentModeId": current_mode,
                "availableModes": modes
            })
        }),
        "_meta": {
            "availableModels": models,
            "availableModes": modes,
            "availableVariants": available_variants
        }
    })
}

fn format_variant_name(variant: &str) -> String {
    variant
        .split(['_', '-'])
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn jsonrpc_error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message.into()
        }
    })
}

fn usage_from_message(message: &MessageWithParts) -> Value {
    let MessageInfo::Assistant(info) = &message.info else {
        return Value::Null;
    };
    let total = info.tokens.input
        + info.tokens.output
        + info.tokens.reasoning
        + info.tokens.cache.read
        + info.tokens.cache.write;
    json!({
        "totalTokens": total,
        "inputTokens": info.tokens.input,
        "outputTokens": info.tokens.output,
        "thoughtTokens": info.tokens.reasoning,
        "cachedReadTokens": info.tokens.cache.read,
        "cachedWriteTokens": info.tokens.cache.write
    })
}

fn usage_from_assistant_info(info: &Value) -> Value {
    let Some(tokens) = info.get("tokens") else {
        return Value::Null;
    };
    let input = tokens.get("input").and_then(Value::as_u64).unwrap_or(0);
    let output = tokens.get("output").and_then(Value::as_u64).unwrap_or(0);
    let reasoning = tokens.get("reasoning").and_then(Value::as_u64).unwrap_or(0);
    let cache_read = tokens
        .get("cache")
        .and_then(|cache| cache.get("read"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let cache_write = tokens
        .get("cache")
        .and_then(|cache| cache.get("write"))
        .and_then(Value::as_u64)
        .unwrap_or(0);
    let total = input + output + reasoning + cache_read + cache_write;
    if total == 0 {
        return Value::Null;
    }
    json!({
        "totalTokens": total,
        "inputTokens": input,
        "outputTokens": output,
        "thoughtTokens": reasoning,
        "cachedReadTokens": cache_read,
        "cachedWriteTokens": cache_write
    })
}
