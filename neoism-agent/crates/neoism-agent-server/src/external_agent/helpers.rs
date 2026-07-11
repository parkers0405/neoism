use super::*;

pub(crate) async fn update_external_session_metadata(
    state: &AppState,
    child_id: &str,
    runtime: ExternalRuntime,
    external_session_id: &str,
    status: &str,
) -> Result<(), ApiError> {
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(());
    };
    child.time.updated = now_millis();
    child.extra.insert(
        "externalAgent".to_string(),
        json!({
            "runtime": "acp",
            "provider": runtime.provider_id(),
            "agent": runtime.agent_name(),
            "externalSessionId": external_session_id,
            "status": status,
        }),
    );
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    Ok(())
}

pub(crate) async fn update_external_session_status(
    state: &AppState,
    child_id: &str,
    runtime: ExternalRuntime,
    status: &str,
) -> Result<(), ApiError> {
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(());
    };
    let mut external = child
        .extra
        .get("externalAgent")
        .cloned()
        .unwrap_or_else(|| json!({}));
    external["runtime"] = json!("acp");
    external["provider"] = json!(runtime.provider_id());
    external["agent"] = json!(runtime.agent_name());
    external["status"] = json!(status);
    external["lastActivityAt"] = json!(now_millis());
    child.extra.insert("externalAgent".to_string(), external);
    child.time.updated = now_millis();
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    Ok(())
}

pub(crate) async fn update_external_activity(
    state: &AppState,
    child_id: &str,
    runtime: ExternalRuntime,
    update: Value,
) -> Result<(), ApiError> {
    let Some(mut child) = state.inner.store.get_session(child_id).await? else {
        return Ok(());
    };
    let mut external = child
        .extra
        .get("externalAgent")
        .cloned()
        .unwrap_or_else(|| json!({}));
    external["runtime"] = json!("acp");
    external["provider"] = json!(runtime.provider_id());
    external["agent"] = json!(runtime.agent_name());
    external["lastUpdate"] = update;
    external["lastActivityAt"] = json!(now_millis());
    child.extra.insert("externalAgent".to_string(), external);
    child.time.updated = now_millis();
    state.inner.store.update_session(&child).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": child.id, "info": child }),
    ));
    Ok(())
}

pub(crate) async fn touch_session(
    state: &AppState,
    session_id: &str,
) -> Result<(), ApiError> {
    let Some(mut session) = state.inner.store.get_session(session_id).await? else {
        return Ok(());
    };
    session.time.updated = now_millis();
    state.inner.store.update_session(&session).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({ "sessionID": session.id, "info": session }),
    ));
    Ok(())
}

pub(crate) fn external_session_id(
    child: &SessionInfo,
    runtime: ExternalRuntime,
) -> Option<String> {
    let external = child.extra.get("externalAgent")?;
    let provider = external.get("provider").and_then(Value::as_str)?;
    if provider != runtime.provider_id() {
        return None;
    }
    external
        .get("externalSessionId")
        .and_then(Value::as_str)
        .map(str::to_string)
}

pub(crate) fn external_model(runtime: ExternalRuntime) -> UserModel {
    UserModel {
        provider_id: "external".to_string(),
        model_id: runtime.provider_id().to_string(),
        variant: None,
    }
}

pub(crate) async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn session_is_running(state: &AppState, session_id: &str) -> bool {
    state.inner.runs.read().await.contains_key(session_id)
}

pub(crate) fn ensure_child_task_belongs_to_parent(
    parent: &SessionInfo,
    child: &SessionInfo,
) -> Result<(), String> {
    if child.parent_id.as_ref().map(|id| id.as_str()) == Some(parent.id.as_str()) {
        return Ok(());
    }
    Err(format!(
        "task_id {} is not a subagent task for session {}",
        child.id, parent.id
    ))
}

pub(crate) fn task_metadata(
    child_session_id: &str,
    runtime: ExternalRuntime,
    status: &str,
    background: bool,
) -> Value {
    json!({
        "sessionId": child_session_id,
        "agent": runtime.agent_name(),
        "runtime": "acp",
        "provider": runtime.provider_id(),
        "status": status,
        "background": background,
    })
}

pub(crate) fn task_started_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id} (use this to check or continue the external subagent task)"),
        "status: running".to_string(),
        String::new(),
        "The external subagent is running in the background. The main session can keep working. Call task_result with this task_id to check the result, or call task with this task_id and a new prompt after it finishes to continue the same external session."
            .to_string(),
    ]
    .join("\n")
}

pub(crate) fn task_running_output(child_session_id: &str) -> String {
    [
        format!("task_id: {child_session_id}"),
        "status: running".to_string(),
        String::new(),
        "The external subagent is still running. Keep working in the main session or call task_result later."
            .to_string(),
    ]
    .join("\n")
}

pub(crate) fn task_result_output(child_session_id: &str, text: String) -> String {
    [
        format!(
            "task_id: {child_session_id} (for resuming to continue this task if needed)"
        ),
        "status: completed".to_string(),
        String::new(),
        "<task_result>".to_string(),
        text,
        "</task_result>".to_string(),
    ]
    .join("\n")
}

pub(crate) fn assistant_text(message: &MessageWithParts) -> Option<String> {
    if !matches!(message.info, MessageInfo::Assistant(_)) {
        return None;
    }
    message.parts.iter().rev().find_map(|part| match part {
        Part::Text(part) => Some(part.text.clone()),
        _ => None,
    })
}
