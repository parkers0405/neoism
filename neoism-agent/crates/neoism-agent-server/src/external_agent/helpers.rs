use super::*;

/// Append-only ACP output lives in the same managed directory accepted by
/// artifact_read. The deterministic, session-scoped name also permits recovery
/// after the event consumer (or server) exits, without retaining output in RAM.
pub(super) fn acp_output_path(session: &str, key: &str) -> PathBuf {
    use sha2::{Digest, Sha256};
    let mut digest = Sha256::new();
    digest.update(session.as_bytes());
    digest.update([0]);
    digest.update(key.as_bytes());
    std::env::temp_dir()
        .join("neoism-agent-state")
        .join("tool-output")
        .join(format!("acp-{:x}.txt", digest.finalize()))
}

/// Return the previous byte length (zero for a replacement) so callers can
/// publish exactly one cap-crossing boundary without rereading a growing file.
pub(super) async fn write_acp_output(
    path: PathBuf,
    text: String,
    append: bool,
) -> Result<u64, ApiError> {
    tokio::task::spawn_blocking(move || {
        use std::io::Write;
        std::fs::create_dir_all(path.parent().expect("managed output directory"))?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(append)
            .truncate(!append)
            .open(path)?;
        let previous_bytes = if append { file.metadata()?.len() } else { 0 };
        // Closing after write_all makes bytes available across reconnects and
        // process restarts without an fsync/DB transaction for every fragment.
        file.write_all(text.as_bytes())?;
        Ok::<_, std::io::Error>(previous_bytes)
    })
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?
    .map_err(|error| ApiError::internal(format!("saving ACP output: {error}")))
}

/// Read at most the standard output cap, never the entire spool. A managed
/// path is deliberately disclosed even without tool metadata (nested text and
/// failed tools), which artifact_read can resolve after reconnect/restart.
pub(super) async fn acp_output_preview(
    path: PathBuf,
) -> Result<Option<String>, ApiError> {
    tokio::task::spawn_blocking(move || -> std::io::Result<Option<String>> {
        use std::io::Read;
        use crate::tool::truncate::{MAX_BYTES, MAX_LINES};
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let mut bytes = Vec::new();
        file.take((MAX_BYTES + 1) as u64).read_to_end(&mut bytes)?;
        let overflow = bytes.len() > MAX_BYTES;
        bytes.truncate(MAX_BYTES);
        // Chunks are UTF-8; a byte-limited preview can end inside a codepoint.
        if let Err(error) = std::str::from_utf8(&bytes) {
            if error.error_len().is_some() {
                return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, error));
            }
            bytes.truncate(error.valid_up_to());
        }
        let text = String::from_utf8(bytes).expect("UTF-8 boundary checked");
        let end = text.match_indices('\n').nth(MAX_LINES - 1)
            .map(|(i, _)| i + 1).filter(|end| *end < text.len());
        let truncated = overflow || end.is_some();
        let preview = &text[..end.unwrap_or(text.len())];
        Ok(Some(if truncated {
            format!("{preview}\n\n[ACP output preview truncated. Exact full output saved to: {}\nUse artifact_read or artifact_search with this managed path.]", path.display())
        } else {
            text
        }))
    })
    .await
    .map_err(|error| ApiError::internal(error.to_string()))?
    .map_err(|error| ApiError::internal(format!("reading ACP output: {error}")))
}

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
    let mut external = child
        .extra
        .get("externalAgent")
        .cloned()
        .unwrap_or_else(|| json!({}));
    external["runtime"] = json!("acp");
    external["provider"] = json!(runtime.provider_id());
    external["agent"] = json!(runtime.agent_name());
    external["externalSessionId"] = json!(external_session_id);
    external["status"] = json!(status);
    if child.parent_id.is_none()
        && external["sourceHost"].is_null()
        && external["sourceKey"].is_null()
    {
        let cwd = crate::windows_process::canonicalize_path(std::path::Path::new(
            &child.directory,
        ))
        .map_err(|error| {
            ApiError::bad_request(format!("ACP workspace path is unavailable: {error}"))
        })?;
        let key = super::catalog::source_key_for(
            runtime,
            crate::caller::session_tenant(&child),
            &cwd,
            external_session_id,
        )
        .ok_or_else(|| ApiError::bad_request("ACP workspace path is not UTF-8"))?;
        external["sourceHost"] = json!(super::catalog::native_host_id());
        external["sourceKey"] = json!(key);
    }
    if child.extra.get("externalAgent") == Some(&external) {
        return Ok(());
    }
    child.extra.insert("externalAgent".to_string(), external);
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
        connection_id: None,
        variant: None,
    }
}

pub(crate) async fn wait_for_cancel(cancel: Arc<AtomicBool>) {
    while !cancel.load(Ordering::SeqCst) {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn session_is_running(state: &AppState, session_id: &str) -> bool {
    state
        .inner
        .session_coordinator
        .active_run(session_id)
        .await
        .is_some()
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
