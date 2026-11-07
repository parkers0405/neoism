//! Rust-owned human interactions. Provider-internal execution is not an OS sandbox.
use super::*;
use super::projector::{scope_key, NativeProjector};
use neoism_agent_core::QuestionRequestInfo;
use crate::state::QuestionPending;

impl NativeProjector {
    pub(crate) async fn interaction(&self, method: &str, params: &Value) -> Result<Value, String> {
        let state = self.app()?;
        if self.cancellation.load(Ordering::Relaxed) { return Err("native interaction cancelled".into()); }
        let (session_id, message_id, directory, tool_input) = {
            let data = self.data.lock().await;
            let key = scope_key(&params["scope"])?;
            let call_id = params["toolCallId"].as_str().unwrap_or("");
            let scope = match key.as_ref() {
                Some(key) => data.children.get(key).ok_or("interaction has unknown native scope")?,
                None => {
                    if data.root.tool_input(call_id).is_some() { &data.root }
                    else {
                        let mut children = data.children.values().filter(|scope| scope.tool_input(call_id).is_some());
                        let child = children.next();
                        if children.next().is_some() { return Err("ambiguous native interaction call ID".into()); }
                        child.unwrap_or(&data.root)
                    }
                }
            };
            (scope.session.id.clone(), scope.message_id.clone(), scope.session.directory.clone(), scope.tool_input(call_id))
        };
        match method {
            "permission.request" => {
                let name = params["toolName"].as_str().ok_or("permission needs toolName")?;
                let mut input = params.get("input").cloned().unwrap_or(json!({}));
                if let Some(cached) = tool_input {
                    if input.is_null() { input = cached; }
                    else if let (Some(current), Some(cached)) = (input.as_object_mut(), cached.as_object()) {
                        for (key, value) in cached { current.entry(key.clone()).or_insert_with(|| value.clone()); }
                    }
                }
                let (permission_name, mut targets) = permission_targets(name, &input);
                if matches!(permission_name.as_str(), "read" | "edit") {
                    let alternate = targets.iter().filter(|path| path.as_str() != "*").map(|path| {
                        let path = Path::new(path);
                        if path.is_absolute() { path.strip_prefix(&directory).unwrap_or(path).to_string_lossy().into_owned() }
                        else { Path::new(&directory).join(path).to_string_lossy().into_owned() }
                    }).collect::<Vec<_>>();
                    targets.extend(alternate);
                    targets.sort(); targets.dedup();
                }
                let rules = current_permissions(&state, session_id.as_str()).await?;
                let actions = targets.iter().map(|target| permission::evaluate(&permission_name, target, &rules).action).collect::<Vec<_>>();
                if actions.contains(&PermissionAction::Deny) { return Ok(json!({"decision":"deny"})); }
                if actions.iter().all(|action| *action == PermissionAction::Allow) {
                    return Ok(json!({"decision":"allow_once"}));
                }
                let call = params["toolCallId"].as_str().or_else(|| params["id"].as_str())
                    .ok_or("permission needs native request id")?;
                let error = crate::permission_runtime::permission_required_many(&permission_name, targets.clone()).to_string();
                let result = crate::permission_runtime::ask_permission_for_tool_cancellable(&state,
                    &session_id, &message_id, call, name, &input, &error, Some(self.cancellation.clone())).await;
                match result {
                    Ok(_) => {
                        // Recheck a deny introduced while the dialog was pending.
                        let current = current_permissions(&state, session_id.as_str()).await?;
                        if targets.iter().any(|target| permission::evaluate(&permission_name, target, &current).action == PermissionAction::Deny) {
                            Ok(json!({"decision":"deny"}))
                        } else { Ok(json!({"decision":"allow_once"})) }
                    }
                    Err(error) if self.cancellation.load(Ordering::Relaxed) => Err(error),
                    Err(_) => Ok(json!({"decision":"deny"})),
                }
            }
            "question.request" => {
                let questions = params["questions"].as_array().filter(|q| !q.is_empty())
                    .ok_or("native question needs nonempty questions")?.clone();
                let mut ids = std::collections::HashSet::new();
                for question in &questions {
                    let id = question["id"].as_str().filter(|id| !id.is_empty()).ok_or("native question needs id")?;
                    if !ids.insert(id.to_string()) { return Err("duplicate native question id".into()); }
                }
                let (sender, receiver) = tokio::sync::oneshot::channel();
                let request = QuestionRequestInfo { id: Id::ascending(IdKind::Question).to_string(),
                    session_id: session_id.to_string(), message_id: message_id.to_string(), questions: questions.clone() };
                state.inner.store.save_question_request(&request).await.map_err(|e| e.to_string())?;
                state.inner.question_waiters.write().await.insert(request.id.clone(), QuestionPending { sender });
                state.inner.questions.write().await.insert(request.id.clone(), request.clone());
                // The handler is also dropped on connection shutdown. Drop cleanup
                // removes live waiters even if the select cancellation never runs.
                let mut cleanup = QuestionCleanup { state: state.clone(), id: request.id.clone(), armed: true };
                state.publish(EventPayload::new(event_type::QUESTION_ASKED, json!(request)));
                let result = tokio::select! {
                    biased;
                    _ = wait_for_cancel(self.cancellation.clone()) => Err("native question cancelled".into()),
                    result = receiver => result.map_err(|_| "native question closed".to_string()).and_then(|r| r),
                };
                state.inner.question_waiters.write().await.remove(&request.id);
                state.inner.questions.write().await.remove(&request.id);
                if result.is_err() {
                    let _ = tokio::time::timeout(Duration::from_millis(100),
                        state.inner.store.resolve_interaction(&request.id, "cancelled", json!({"cancelled":true}))).await;
                    state.publish(EventPayload::new(event_type::QUESTION_REJECTED,
                        json!({"sessionID":session_id,"requestID":request.id})));
                }
                cleanup.armed = false;
                let answers = result?;
                let mut keyed = serde_json::Map::new();
                for (index, question) in questions.iter().enumerate() {
                    keyed.insert(question["id"].as_str().unwrap().into(), json!(answers.get(index).cloned().unwrap_or_default()));
                }
                Ok(json!({"answers":keyed}))
            }
            _ => Err(format!("unsupported native interaction {method}")),
        }
    }
}

struct QuestionCleanup { state: AppState, id: String, armed: bool }
impl Drop for QuestionCleanup {
    fn drop(&mut self) {
        if let Ok(mut map) = self.state.inner.question_waiters.try_write() { map.remove(&self.id); }
        if let Ok(mut map) = self.state.inner.questions.try_write() { map.remove(&self.id); }
        if self.armed {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let weak = Arc::downgrade(&self.state.inner);
                let id = self.id.clone();
                runtime.spawn(async move {
                    if let Some(inner) = weak.upgrade() {
                        inner.question_waiters.write().await.remove(&id);
                        inner.questions.write().await.remove(&id);
                        let _ = tokio::time::timeout(Duration::from_millis(100),
                            inner.store.resolve_interaction(&id, "cancelled", json!({"cancelled":true}))).await;
                    }
                });
            }
        }
    }
}

fn permission_targets(name: &str, input: &Value) -> (String, Vec<String>) {
    let lower = name.to_ascii_lowercase();
    let permission = match lower.as_str() {
        "read" | "read_file" | "readfile" => "read",
        "edit" | "write" | "write_file" | "writefile" | "apply_patch" | "multiedit" | "filechange" => "edit",
        "bash" | "shell" | "execute" | "exec_command" | "commandexecution" => "bash",
        _ => name,
    };
    let keys: &[&str] = if permission == "bash" { &["command", "cmd", "rawCommand"] }
        else { &["filePath", "path", "file_path"] };
    let target = keys.iter().find_map(|key| input[*key].as_str()).map(str::to_owned)
        .or_else(|| if permission == "bash" { input["command"].as_array().map(|args| args.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" ")) } else { None })
        .or_else(|| input.as_str().map(str::to_owned));
    let mut targets = target.into_iter().collect::<Vec<_>>();
    if permission == "edit" {
        for field in ["changes", "edits", "files"] {
            if let Some(changes) = input[field].as_array() {
                for change in changes {
                    if let Some(path) = ["path", "filePath", "file_path"].iter().find_map(|key| change[*key].as_str()) {
                        targets.push(path.into());
                    }
                }
            }
            if let Some(changes) = input[field].as_object() { targets.extend(changes.keys().cloned()); }
        }
    }
    if targets.is_empty() { targets.push("*".into()); }
    (permission.into(), targets)
}