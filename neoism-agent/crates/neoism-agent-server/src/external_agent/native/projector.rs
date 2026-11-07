//! Projection of the private, provider-neutral native runtime vocabulary.
//! No provider events execute tools here. A warm connection may own this object:
//! consequently the application reference must remain weak.
use super::*;
use neoism_agent_core::ReasoningPart;
use std::sync::Weak;
use tokio::sync::Mutex;

pub(crate) struct NativeProjector {
    pub(super) state: Weak<crate::state::InnerState>,
    pub(super) cancellation: Arc<AtomicBool>,
    pub(super) runtime: ExternalRuntime,
    pub(super) data: Mutex<Projection>,
}

pub(super) struct Projection {
    pub(super) root: Scope,
    pub(super) children: HashMap<String, Scope>,
    model: Option<String>,
    usage: Value,
    finish: Option<String>,
}

pub(super) struct Scope {
    pub(super) session: SessionInfo,
    pub(super) message_id: Id,
    pub(super) live: Arc<Mutex<MessageWithParts>>,
    texts: HashMap<String, NativeText>,
    tools: HashMap<String, NativeTool>,
    closed: bool,
    usage: Value,
}

struct NativeText {
    part_id: Id,
    prior: String,
    completed: bool,
}

struct NativeTool {
    part_id: Id,
    name: String,
    title: String,
    input: Value,
    metadata: Value,
    path: PathBuf,
    terminal: bool,
    unconfirmed: bool,
}

impl Scope {
    pub(super) fn tool_input(&self, call_id: &str) -> Option<Value> {
        self.tools.get(call_id).map(|tool| tool.input.clone())
    }
}

impl NativeProjector {
    pub(crate) fn new(
        state: &AppState, session: &SessionInfo, runtime: ExternalRuntime,
        step: &StartedAssistantStep, cancellation: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            state: Arc::downgrade(&state.inner), cancellation, runtime,
            data: Mutex::new(Projection {
                root: Scope { session: session.clone(), message_id: step.assistant_id.clone(),
                    live: step.live_message.clone(), texts: HashMap::new(), tools: HashMap::new(), closed: false, usage: json!({}) },
                children: HashMap::new(), model: None, usage: json!({}), finish: None,
            }),
        })
    }

    pub(super) fn app(&self) -> Result<AppState, String> {
        self.state.upgrade().map(|inner| AppState { inner })
            .ok_or_else(|| "native runtime application has closed".into())
    }

    pub(crate) async fn apply(&self, event: &Value) -> Result<(), String> {
        let state = self.app()?;
        // Serializes turn callbacks and finish; interaction never holds this lock
        // while waiting for permission or a human answer.
        let mut data = self.data.lock().await;
        let kind = required(event, "type")?;
        let key = scope_key(&event["scope"])?;
        if key.is_none() {
            match kind {
                "conversation.updated" => {
                    let native_id = required(event, "sessionId")?;
                    let mut session = state.inner.store.get_session(data.root.session.id.as_str()).await
                        .map_err(|e| e.to_string())?.ok_or("native session no longer exists")?;
                    if session.extra["externalAgent"]["runtime"] != self.runtime.native_driver_id()
                        || !self.runtime.owns_session(&session) {
                        return Err("native conversation driver mismatch".into());
                    }
                    let extra = session.extra.get_mut("externalAgent").ok_or("missing native metadata")?;
                    if extra["nativeSessionId"].as_str().filter(|id| !id.is_empty()).is_some_and(|id| id != native_id) {
                        return Err("native continuation changed its confirmed conversation ID".into());
                    }
                    extra["nativeSessionId"] = json!(native_id);
                    // Keep the shared catalog identity in sync with the confirmed ID.
                    extra["externalSessionId"] = json!(native_id);
                    state.inner.store.update_session(&session).await.map_err(|e| e.to_string())?;
                    state.publish(EventPayload::new(event_type::SESSION_UPDATED,
                        json!({"sessionID":session.id,"info":session})));
                    data.root.session = session;
                    if let Some(model) = event["model"].as_str() { data.model = Some(model.into()); }
                    return Ok(());
                }
                "model.updated" => {
                    data.model = Some(required(event, "model")?.into());
                    if let Some(limit) = event.get("contextWindow") { data.usage["contextWindow"] = limit.clone(); }
                }
                "usage.updated" => {
                    merge(&mut data.usage, &event["usage"]);
                }
                _ => {}
            }
            if matches!(kind, "usage.updated" | "model.updated") {
                let mut message = data.root.live.lock().await;
                if let MessageInfo::Assistant(a) = &mut message.info {
                    a.tokens.context_limit = data.usage["contextWindow"].as_u64();
                    if let Some(model) = &data.model { a.model_id = model.clone(); }
                }
            }
        }
        if let Some(key) = key.as_ref() {
            if !data.children.contains_key(key) {
                // Scoped background callbacks remain attributable to this turn
                // even when the root has already returned its final response.
                let scope = self.create_child(&state, &data, &event["scope"], key).await?;
                data.children.insert(key.clone(), scope);
            }
        }
        let scope = match key.as_ref() {
            Some(key) => data.children.get_mut(key).expect("inserted scope"),
            None => &mut data.root,
        };
        if key.is_some() {
            if let Some(native_id) = event["scope"]["sessionId"].as_str().filter(|id| !id.is_empty()) {
                let mut session = state.inner.store.get_session(scope.session.id.as_str()).await.map_err(|e| e.to_string())?
                    .ok_or("native projection session disappeared")?;
                if session.extra["externalAgent"]["nativeSessionId"] != native_id {
                    session.extra.get_mut("externalAgent").ok_or("missing projection metadata")?["nativeSessionId"] = json!(native_id);
                    state.inner.store.update_session(&session).await.map_err(|e| e.to_string())?;
                    state.publish(EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID":session.id,"info":session})));
                    scope.session = session;
                }
            }
        }
        match kind {
            "usage.updated" | "model.updated" if key.is_some() => {
                if kind == "usage.updated" { merge(&mut scope.usage, &event["usage"]); }
                if let Some(limit) = event.get("contextWindow") { scope.usage["contextWindow"] = limit.clone(); }
                {
                    let mut message = scope.live.lock().await;
                    if let MessageInfo::Assistant(a) = &mut message.info {
                        if kind == "model.updated" { a.model_id = required(event, "model")?.into(); }
                        a.tokens = tokens(&scope.usage);
                    }
                }
                persist(&state, scope, vec![]).await?;
            }
            "text.delta" | "text.completed" | "reasoning.delta" | "reasoning.completed" => {
                if !scope.closed { project_text(&state, scope, event, kind).await?; }
            }
            "tool.started" | "tool.updated" | "tool.output" | "tool.completed" | "tool.failed" => {
                if scope.closed && !scope.tools.contains_key(required(event, "id")?) { return Ok(()); }
                seal_text(&state, scope).await?;
                project_tool(&state, scope, event, kind).await?;
            }
            "child.updated" if key.is_some() => {
                let status = required(event, "status")?;
                if scope.closed && status == "running" { return Ok(()); }
                if status != "running" {
                    if let Some(summary) = event["summary"].as_str() {
                        project_text(&state, scope, &json!({"id":"native-summary","text":summary}), "text.completed").await?;
                    }
                    close_scope(&state, scope, status).await?;
                }
                update_external_session_status(&state, scope.session.id.as_str(), self.runtime, status)
                    .await.map_err(|e| e.to_string())?;
            }
            "plan.updated" if !scope.closed => {
                let todos = event["entries"].as_array().ok_or("plan entries must be an array")?
                    .iter().map(|entry| json!({"content":entry["content"],"status":entry["status"],
                        "priority":entry.get("priority").cloned().unwrap_or(json!("medium"))}))
                    .map(serde_json::from_value::<neoism_agent_core::TodoInfo>)
                    .collect::<Result<Vec<_>, _>>().map_err(|e| e.to_string())?;
                state.inner.todos.write().await.insert(scope.session.id.to_string(), todos.clone());
                let mut session = state.inner.store.get_session(scope.session.id.as_str()).await.map_err(|e| e.to_string())?
                    .ok_or("native plan session disappeared")?;
                session.extra.get_mut("externalAgent").ok_or("missing native metadata")?["planTodos"] = json!(todos);
                state.inner.store.update_session(&session).await.map_err(|e| e.to_string())?;
                state.publish(EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID":session.id,"info":session})));
                state.publish(EventPayload::new(event_type::TODO_UPDATED,
                    json!({"sessionID":scope.session.id,"todos":todos})));
            }
            "runtime.status" if !scope.closed => {
                update_external_activity(&state, scope.session.id.as_str(), self.runtime, event.clone())
                    .await.map_err(|e| e.to_string())?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn create_child(&self, state: &AppState, data: &Projection, native: &Value, key: &str) -> Result<Scope, String> {
        let root = state.inner.store.get_session(data.root.session.id.as_str()).await.map_err(|e| e.to_string())?
            .ok_or("native root session disappeared")?;
        let parent = if let Some(id) = native["parentId"].as_str() {
            if root.extra["externalAgent"]["nativeSessionId"] == id || root.extra["externalAgent"]["externalSessionId"] == id {
                &root
            } else { &data.children.get(id).ok_or("native child has unknown parent scope")?.session }
        } else { &root };
        // Load current inherited rules, not the policy captured at turn start.
        let parent = state.inner.store.get_session(parent.id.as_str()).await.map_err(|e| e.to_string())?
            .ok_or("native parent session disappeared")?;
        let mut extra = parent.extra.clone();
        extra.remove("externalAgent");
        extra.insert("externalAgent".into(), json!({
            "runtime":self.runtime.native_driver_id(),"provider":self.runtime.provider_id(),
            "agent":self.runtime.agent_name(),"nested":true,"nativeProjection":true,
            "nativeScopeId":native.get("id"),"parentToolCallId":native.get("parentToolCallId"),
            "status":"running"
        }));
        // Only an explicitly confirmed real conversation ID is resumable upstream;
        // even then this Neoism child remains a non-controllable projection.
        if let Some(id) = native["sessionId"].as_str().filter(|id| !id.is_empty()) {
            extra.get_mut("externalAgent").unwrap()["nativeSessionId"] = json!(id);
        }
        let title = native["title"].as_str().unwrap_or("Native subagent");
        let request = neoism_agent_core::CreateSessionRequest {
            parent_id: Some(parent.id.clone()), title: Some(title.into()),
            agent: Some(native["agent"].as_str().unwrap_or(self.runtime.agent_name()).into()),
            model: parent.model.clone(), permission: parent.permission.clone(), workspace_id: parent.workspace_id.clone(),
            external_provider: None, external_options: None,
        };
        let existing = state.inner.store.list_sessions().await.map_err(|e| e.to_string())?
            .into_iter().find(|session| {
                let meta = &session.extra["externalAgent"];
                session.parent_id.as_ref() == Some(&parent.id)
                    && meta["runtime"] == self.runtime.native_driver_id()
                    && meta["nativeProjection"] == true
                    && ((native["id"].is_string() && meta["nativeScopeId"] == native["id"])
                        || (native["sessionId"].is_string() && meta["nativeSessionId"] == native["sessionId"])
                        || ((native["id"].is_null() || meta["nativeScopeId"].is_null())
                            && native["parentToolCallId"].is_string()
                            && meta["parentToolCallId"] == native["parentToolCallId"]))
            });
        let session = if let Some(session) = existing { session } else {
            crate::session_routes::create_session_in_directory(state, &parent.directory, request, extra)
                .await.map_err(|e| e.to_string())?
        };
        update_external_session_status(state, session.id.as_str(), self.runtime, "running").await.map_err(|e| e.to_string())?;
        let step = start_assistant_step(state, &session.id, session.id.as_str(), &data.root.message_id,
            &session.directory, now_millis(), "native".into(), session.agent.clone().unwrap_or_default(),
            data.model.clone().unwrap_or_default(), self.runtime.provider_id().into(), false)
            .await.map_err(|e| format!("starting native child {key}: {e}"))?;
        Ok(Scope { session, message_id: step.assistant_id, live: step.live_message,
            texts: HashMap::new(), tools: HashMap::new(), closed: false, usage: json!({}) })
    }

    pub(crate) async fn finish(&self, status: &str) -> Result<(), String> {
        let state = self.app()?;
        let mut data = self.data.lock().await;
        data.finish = Some(status.into());
        seal_text(&state, &mut data.root).await?;
        data.root.closed = true;
        persist(&state, &data.root, vec![]).await?;
        if status != "completed" && status != "stop" {
            settle_tools(&state, &mut data.root, status).await?;
            for scope in data.children.values_mut() {
                if !scope.closed {
                    close_scope(&state, scope, status).await?;
                    update_external_session_status(&state, scope.session.id.as_str(), self.runtime, status)
                        .await.map_err(|e| e.to_string())?;
                }
            }
        }
        Ok(())
    }

    pub(crate) async fn has_background_work(&self) -> bool {
        let data = self.data.lock().await;
        data.root.tools.values().any(|t| !t.terminal) || data.children.values()
            .any(|s| !s.closed || s.tools.values().any(|t| !t.terminal))
    }

    pub(crate) async fn response(&self, _model: &UserModel, native_result: &Value) -> ProviderGenerationResponse {
        let data = self.data.lock().await;
        let mut usage = data.usage.clone();
        merge(&mut usage, &native_result["usage"]);
        let message = data.root.live.lock().await;
        let text = message.parts.iter().filter_map(|part| match part { Part::Text(p) => Some(p.text.as_str()), _ => None }).collect::<Vec<_>>().join("");
        ProviderGenerationResponse {
            provider_id: self.runtime.provider_id().into(),
            model_id: native_result["model"].as_str().map(str::to_owned).or_else(|| data.model.clone())
                // Requested selections (including the historical "codex" or
                // "claude" placeholders) are not proof of the served model.
                .unwrap_or_default(),
            text, finish: native_result["finish"].as_str().map(str::to_owned).or_else(|| data.finish.clone()),
            total_tokens: usage["total"].as_u64(), input_tokens: number(&usage, "input"),
            output_tokens: number(&usage, "output"), reasoning_tokens: number(&usage, "reasoning"),
            cache_read_tokens: number(&usage, "cacheRead"), cache_write_tokens: number(&usage, "cacheWrite"),
        }
    }
}

pub(super) fn scope_key(scope: &Value) -> Result<Option<String>, String> {
    if scope.is_null() { return Ok(None); }
    if let Some(id) = scope["id"].as_str().filter(|id| !id.is_empty()) { return Ok(Some(id.into())); }
    scope["parentToolCallId"].as_str().filter(|id| !id.is_empty()).map(|id| Some(format!("tool:{id}")))
        .ok_or_else(|| "native scope needs an observed id or parentToolCallId".into())
}

fn required<'a>(value: &'a Value, field: &str) -> Result<&'a str, String> {
    value[field].as_str().filter(|s| !s.is_empty()).ok_or_else(|| format!("native event missing {field}"))
}
fn number(value: &Value, key: &str) -> u64 { value[key].as_u64().unwrap_or(0) }
fn tokens(usage: &Value) -> TokenUsage {
    TokenUsage {
        total: usage["total"].as_u64(), context_limit: usage["contextWindow"].as_u64(),
        input: number(usage, "input").saturating_sub(number(usage, "cacheRead").saturating_add(number(usage, "cacheWrite"))),
        output: number(usage, "output").saturating_sub(number(usage, "reasoning")),
        reasoning: number(usage, "reasoning"),
        cache: neoism_agent_core::CacheUsage { read: number(usage, "cacheRead"), write: number(usage, "cacheWrite") },
    }
}
fn merge(target: &mut Value, source: &Value) {
    if !target.is_object() { *target = json!({}); }
    if let Some(map) = source.as_object() { for (key, value) in map { target[key] = value.clone(); } }
}

async fn persist(state: &AppState, scope: &Scope, parts: Vec<Part>) -> Result<(), String> {
    let message = scope.live.lock().await.clone();
    state.inner.store.update_message(scope.session.id.as_str(), &message).await.map_err(|e| e.to_string())?;
    for part in parts { state.publish(EventPayload::new(event_type::MESSAGE_PART_UPDATED,
        json!({"sessionID":scope.session.id,"part":part,"time":now_millis()}))); }
    Ok(())
}

async fn seal_text(state: &AppState, scope: &mut Scope) -> Result<(), String> {
    let parts = {
        let mut message = scope.live.lock().await;
        let ids = message.parts.iter().filter_map(|p| match p {
            Part::Text(t) if t.time.as_ref().is_some_and(|t| t.end.is_none()) => Some(t.id.clone()),
            Part::Reasoning(t) if t.time.end.is_none() => Some(t.id.clone()), _ => None,
        }).collect::<Vec<_>>();
        ids.into_iter().filter_map(|id| crate::message_part_mutation::finish_text_part(&mut message.parts, id.as_str(), None)).collect::<Vec<_>>()
    };
    if !parts.is_empty() { persist(state, scope, parts).await?; }
    Ok(())
}

async fn project_text(state: &AppState, scope: &mut Scope, event: &Value, kind: &str) -> Result<(), String> {
    let native_id = required(event, "id")?;
    let reasoning = kind.starts_with("reasoning.");
    let completed = kind.ends_with("completed");
    let key = format!("{}:{native_id}", if reasoning { "r" } else { "t" });
    if scope.texts.get(&key).is_some_and(|t| t.completed) { return Ok(()); }
    let mut is_new = !scope.texts.contains_key(&key);
    // Semantic boundaries seal display segments, not native IDs. A provider can
    // continue the same ID after reasoning or a tool; do not drop that text.
    if let Some(stream) = scope.texts.get_mut(&key) {
        let message = scope.live.lock().await;
        let previous = message.parts.iter().find_map(|p| match p {
            Part::Text(t) if t.id == stream.part_id && t.time.as_ref().is_some_and(|t| t.end.is_some()) => Some(&t.text),
            Part::Reasoning(t) if t.id == stream.part_id && t.time.end.is_some() => Some(&t.text), _ => None,
        });
        if let Some(previous) = previous {
            if !completed {
                stream.prior.push_str(previous);
                stream.part_id = Id::ascending(IdKind::Part);
                is_new = true;
            }
        }
    }
    if is_new { seal_text(state, scope).await?; }
    let stream = scope.texts.entry(key).or_insert_with(|| NativeText {
        part_id: Id::ascending(IdKind::Part), prior: String::new(), completed: false,
    });
    let id = stream.part_id.clone();
    let prior = stream.prior.clone();
    let aggregate = event["text"].as_str().unwrap_or("");
    let text = if completed { aggregate.strip_prefix(&prior).unwrap_or(aggregate) } else { aggregate };
    stream.completed = completed;
    let part = {
        let mut message = scope.live.lock().await;
        let existing = message.parts.iter().position(|p| match p { Part::Text(t) => t.id == id, Part::Reasoning(t) => t.id == id, _ => false });
        if let Some(index) = existing {
            match &mut message.parts[index] {
                Part::Text(t) => {
                    if completed { if event["text"].is_string() { t.text = text.into(); } }
                    else { t.text.push_str(text); }
                    if completed { t.time.get_or_insert(PartTime { start: now_millis(), end: None }).end = Some(now_millis()); }
                }
                Part::Reasoning(t) => {
                    if completed { if event["text"].is_string() { t.text = text.into(); } }
                    else { t.text.push_str(text); }
                    if completed { t.time.end = Some(now_millis()); }
                }
                _ => unreachable!(),
            }
            message.parts[index].clone()
        } else {
            let time = PartTime { start: now_millis(), end: completed.then(now_millis) };
            let part = if reasoning { Part::Reasoning(ReasoningPart { id: id.clone(), session_id: scope.session.id.clone(),
                message_id: scope.message_id.clone(), text: text.into(), time, metadata: None }) }
            else { Part::Text(TextPart { id: id.clone(), session_id: scope.session.id.clone(),
                message_id: scope.message_id.clone(), text: text.into(), time: Some(time), synthetic: None }) };
            message.parts.push(part.clone()); part
        }
    };
    if is_new || completed { persist(state, scope, vec![part]).await?; }
    else { state.publish_live(EventPayload::new(event_type::MESSAGE_PART_DELTA,
        json!({"sessionID":scope.session.id,"messageID":scope.message_id,"partID":id,
            "partType":if reasoning {"reasoning"} else {"text"},"field":"text","delta":text}))); }
    Ok(())
}

async fn project_tool(state: &AppState, scope: &mut Scope, event: &Value, kind: &str) -> Result<(), String> {
    let call_id = required(event, "id")?;
    let is_new = !scope.tools.contains_key(call_id);
    let tool = scope.tools.entry(call_id.into()).or_insert_with(|| {
        let part_id = Id::ascending(IdKind::Part);
        NativeTool { path: output_path(scope.session.id.as_str(), part_id.as_str()), part_id,
            name: "native_tool".into(), title: "Native tool".into(), input: json!({}), metadata: json!({}), terminal: false, unconfirmed: false }
    });
    if tool.terminal && !(tool.unconfirmed && matches!(kind, "tool.completed" | "tool.failed")) { return Ok(()); }
    if let Some(name) = event["name"].as_str() { tool.name = name.into(); }
    if let Some(title) = event["title"].as_str() { tool.title = title.into(); }
    if let Some(input) = event.get("input") { tool.input = input.clone(); }
    merge(&mut tool.metadata, &event["metadata"]);
    if let Some(kind) = event.get("kind") { tool.metadata["nativeKind"] = kind.clone(); }
    tool.metadata["outputPath"] = json!(tool.path);
    if let Some(exit) = event.get("exitCode") { tool.metadata["exitCode"] = exit.clone(); }
    let output = if kind == "tool.output" { event["text"].as_str() } else { event["output"].as_str() };
    if let Some(output) = output {
        let append = if kind == "tool.output" { event["mode"] == "append" } else { event["outputMode"] == "append" };
        spool(tool.path.clone(), output.into(), append).await?;
    }
    let preview = preview(tool.path.clone()).await?;
    tool.metadata["outputPreview"] = json!(preview);
    tool.metadata["terminalOutput"] = json!(preview);
    let part = {
        let mut message = scope.live.lock().await;
        set_tool_running(&mut message.parts, tool.part_id.clone(), &scope.session.id, &scope.message_id,
            call_id.into(), tool.name.clone(), tool.input.clone());
        if kind == "tool.completed" {
            tool.terminal = true;
            tool.unconfirmed = false;
            set_tool_completed(&mut message.parts, tool.part_id.as_str(), preview.clone(), tool.title.clone(), tool.metadata.clone());
        } else if kind == "tool.failed" {
            tool.terminal = true;
            tool.unconfirmed = false;
            let error = event["error"].as_str().map(str::to_owned).unwrap_or_else(|| event["error"].to_string());
            set_tool_error(&mut message.parts, tool.part_id.as_str(), error);
        }
        let part = message.parts.iter_mut().find(|p| matches!(p, Part::Tool(t) if t.id == tool.part_id)).expect("tool exists");
        if let Part::Tool(t) = part {
            let mut metadata = tool.metadata.clone();
            metadata["title"] = json!(tool.title);
            metadata["outputPreview"] = json!(preview);
            metadata["terminalOutput"] = json!(preview);
            t.metadata = Some(metadata);
        }
        part.clone()
    };
    if kind == "tool.output" && !is_new {
        state.publish_live(EventPayload::new(event_type::MESSAGE_PART_UPDATED,
            json!({"sessionID":scope.session.id,"part":part,"time":now_millis()})));
        Ok(())
    } else { persist(state, scope, vec![part]).await }
}

async fn settle_tools(state: &AppState, scope: &mut Scope, status: &str) -> Result<(), String> {
    let mut parts = Vec::new();
    {
        let mut message = scope.live.lock().await;
        for tool in scope.tools.values_mut().filter(|t| !t.terminal) {
            // Losing the turn is not evidence that a provider process was killed.
            if let Some(mut part) = set_tool_error(&mut message.parts, tool.part_id.as_str(),
                format!("Native turn {status}; tool outcome was not confirmed")) {
                if let Part::Tool(t) = &mut part {
                    let mut meta = tool.metadata.clone(); meta["outcomeUnconfirmed"] = json!(true);
                    t.metadata = Some(meta);
                }
                crate::message_part_mutation::upsert_part(&mut message.parts, part.clone());
                parts.push(part);
            }
            tool.terminal = true;
            tool.unconfirmed = true;
        }
    }
    persist(state, scope, parts).await
}

async fn close_scope(state: &AppState, scope: &mut Scope, status: &str) -> Result<(), String> {
    seal_text(state, scope).await?;
    if status != "completed" { settle_tools(state, scope, status).await?; }
    let was_closed = scope.closed;
    scope.closed = true;
    let (info, step_finish) = {
        let mut message = scope.live.lock().await;
        if let MessageInfo::Assistant(a) = &mut message.info {
            a.time.completed = Some(now_millis()); a.finish = Some(status.into());
            a.error = if status != "completed" { Some(json!({"message":format!("Native child {status}")})) } else { None };
        }
        let step_finish = if !was_closed {
            let part = Part::StepFinish(neoism_agent_core::StepFinishPart {
                id: Id::ascending(IdKind::Part), session_id: scope.session.id.clone(), message_id: scope.message_id.clone(),
                reason: status.into(), tokens: tokens(&scope.usage), cost: 0.0, snapshot: None,
            });
            message.parts.push(part.clone()); Some(part)
        } else { None };
        (message.info.clone(), step_finish)
    };
    persist(state, scope, step_finish.into_iter().collect()).await?;
    state.publish(EventPayload::new(event_type::MESSAGE_UPDATED, json!({"sessionID":scope.session.id,"info":info})));
    Ok(())
}

fn output_path(session: &str, part: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new(); hash.update(session); hash.update([0]); hash.update(part);
    std::env::temp_dir().join("neoism-agent-state").join("tool-output").join(format!("native-{:x}.txt", hash.finalize()))
}

// Snapshot reconciliation compares bounded disk chunks. A final aggregate must
// neither duplicate the initial snapshot plus deltas nor erase a longer stream.
async fn spool(path: PathBuf, text: String, append: bool) -> Result<(), String> {
    tokio::task::spawn_blocking(move || -> Result<(), String> {
        use std::io::{Read, Write};
        std::fs::create_dir_all(path.parent().ok_or("missing spool parent")?).map_err(|e| e.to_string())?;
        let mut offset = 0;
        let mut replace = false;
        if !append {
            match std::fs::File::open(&path) {
                Ok(mut file) => {
                    let mut buffer = [0u8; 8192];
                    loop {
                        let n = file.read(&mut buffer).map_err(|e| e.to_string())?;
                        if n == 0 { break; }
                        let remaining = text.len().saturating_sub(offset);
                        let compare = n.min(remaining);
                        if buffer[..compare] != text.as_bytes()[offset..offset + compare] { offset = 0; replace = true; break; }
                        offset += compare;
                        if offset == text.len() { return Ok(()); }
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.to_string()),
            }
        }
        let mut file = std::fs::OpenOptions::new().create(true).write(true).append(!replace).truncate(replace).open(path).map_err(|e| e.to_string())?;
        file.write_all(&text.as_bytes()[offset..]).map_err(|e| e.to_string())
    }).await.map_err(|e| e.to_string())?
}

async fn preview(path: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || -> Result<String, String> {
        use std::io::Read;
        use crate::tool::truncate::{MAX_BYTES, MAX_LINES};
        let file = match std::fs::File::open(&path) {
            Ok(file) => file, Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
            Err(e) => return Err(e.to_string()),
        };
        let mut bytes = Vec::new(); file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
        let mut truncated = bytes.len() > MAX_BYTES; bytes.truncate(MAX_BYTES);
        if let Err(e) = std::str::from_utf8(&bytes) { bytes.truncate(e.valid_up_to()); }
        let mut text = String::from_utf8(bytes).map_err(|e| e.to_string())?;
        if let Some((end, _)) = text.match_indices('\n').nth(MAX_LINES - 1) {
            let end = end + 1;
            if end < text.len() { text.truncate(end); truncated = true; }
        }
        Ok(if truncated { format!("{text}\n[Native output preview truncated; full output: {}]", path.display()) } else { text })
    }).await.map_err(|e| e.to_string())?
}

#[cfg(test)]
#[path = "projector_tests.rs"]
mod tests;