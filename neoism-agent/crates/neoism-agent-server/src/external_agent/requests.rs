use super::*;

pub(crate) async fn handle_acp_request(
    ctx: &AcpEventContext,
    method: &str,
    params: Value,
) -> Result<Value, AcpRpcError> {
    match method {
        "fs/read_text_file" => handle_read_text_file(ctx, &params).await,
        "fs/write_text_file" => handle_write_text_file(ctx, &params).await,
        "session/request_permission" => {
            let selected = ask_acp_permission(ctx, &params).await?;
            match selected {
                Some(option_id) => Ok(json!({
                    "outcome": {
                        "outcome": "selected",
                        "optionId": option_id,
                    }
                })),
                None => Ok(json!({
                    "outcome": {
                        "outcome": "cancelled",
                    }
                })),
            }
        }
        "terminal/create" => {
            ensure_terminal_create_permission(ctx, &params).await?;
            blocking_terminal_call(
                ctx.terminal_manager.clone(),
                params,
                |manager, params| manager.create(&params),
            )
            .await
        }
        "terminal/output" => {
            blocking_terminal_call(
                ctx.terminal_manager.clone(),
                params,
                |manager, params| manager.output(&params),
            )
            .await
        }
        "terminal/wait_for_exit" => {
            blocking_terminal_call(
                ctx.terminal_manager.clone(),
                params,
                |manager, params| manager.wait_for_exit(&params),
            )
            .await
        }
        "terminal/kill" => {
            blocking_terminal_call(
                ctx.terminal_manager.clone(),
                params,
                |manager, params| manager.kill(&params),
            )
            .await
        }
        "terminal/release" => {
            blocking_terminal_call(
                ctx.terminal_manager.clone(),
                params,
                |manager, params| manager.release(&params),
            )
            .await
        }
        _ => Err(AcpRpcError {
            code: -32601,
            message: format!("Unsupported ACP client method `{method}`"),
        }),
    }
}

async fn handle_read_text_file(
    ctx: &AcpEventContext,
    params: &Value,
) -> Result<Value, AcpRpcError> {
    let path = workspace_path(&ctx.cwd, params.get("path").and_then(Value::as_str))?;
    ensure_external_permission(
        ctx,
        "read",
        &display_workspace_path(&ctx.cwd, &path),
        "External agent file read",
        json!({
            "method": "fs/read_text_file",
            "path": path.display().to_string(),
            "params": params,
        }),
    )
    .await?;
    let mut content =
        tokio::fs::read_to_string(&path)
            .await
            .map_err(|err| AcpRpcError {
                code: -32000,
                message: format!("Could not read {}: {err}", path.display()),
            })?;
    let start_line = params.get("line").and_then(Value::as_u64).unwrap_or(1);
    let limit = params.get("limit").and_then(Value::as_u64);
    if start_line > 1 || limit.is_some() {
        let start = start_line.saturating_sub(1) as usize;
        let mut lines = content.lines().skip(start);
        content = if let Some(limit) = limit {
            lines
                .by_ref()
                .take(limit as usize)
                .collect::<Vec<_>>()
                .join("\n")
        } else {
            lines.collect::<Vec<_>>().join("\n")
        };
    }
    Ok(json!({ "content": content }))
}

async fn handle_write_text_file(
    ctx: &AcpEventContext,
    params: &Value,
) -> Result<Value, AcpRpcError> {
    let path = workspace_path(&ctx.cwd, params.get("path").and_then(Value::as_str))?;
    ensure_external_permission(
        ctx,
        "edit",
        &display_workspace_path(&ctx.cwd, &path),
        "External agent file write",
        json!({
            "method": "fs/write_text_file",
            "path": path.display().to_string(),
            "params": params,
        }),
    )
    .await?;
    let content = params
        .get("content")
        .and_then(Value::as_str)
        .ok_or_else(|| AcpRpcError {
            code: -32602,
            message: "fs/write_text_file missing content".to_string(),
        })?;
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(|err| AcpRpcError {
                code: -32000,
                message: format!("Could not create {}: {err}", parent.display()),
            })?;
    }
    tokio::fs::write(&path, content)
        .await
        .map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("Could not write {}: {err}", path.display()),
        })?;
    Ok(Value::Null)
}

async fn ensure_terminal_create_permission(
    ctx: &AcpEventContext,
    params: &Value,
) -> Result<(), AcpRpcError> {
    let command = params
        .get("command")
        .and_then(Value::as_str)
        .ok_or_else(|| AcpRpcError {
            code: -32602,
            message: "terminal/create missing command".to_string(),
        })?;
    let mut command_line = command.to_string();
    if let Some(args) = params.get("args").and_then(Value::as_array) {
        for arg in args.iter().filter_map(Value::as_str) {
            command_line.push(' ');
            command_line.push_str(arg);
        }
    }
    ensure_external_permission(
        ctx,
        "bash",
        command_line.trim(),
        "External agent terminal command",
        json!({
            "method": "terminal/create",
            "command": command,
            "params": params,
        }),
    )
    .await
}

async fn ensure_external_permission(
    ctx: &AcpEventContext,
    permission_name: &str,
    target: &str,
    title: &str,
    input: Value,
) -> Result<(), AcpRpcError> {
    let rules = external_effective_permissions(ctx).await?;
    match permission::evaluate(permission_name, target, &rules).action {
        PermissionAction::Allow => Ok(()),
        PermissionAction::Deny => Err(AcpRpcError {
            code: -32000,
            message: format!("tool permission {permission_name} for {target} is denied"),
        }),
        PermissionAction::Ask => {
            let session_id =
                Id::parse(IdKind::Session, ctx.child_id.clone()).map_err(|err| {
                    AcpRpcError {
                        code: -32000,
                        message: format!("Invalid external session id: {err}"),
                    }
                })?;
            let error = format!(
                "tool permission {permission_name} for {target} requires approval"
            );
            ask_permission_for_tool(
                &ctx.state,
                &session_id,
                &ctx.assistant_id,
                &format!("external-{}-{}", ctx.runtime.agent_name(), permission_name),
                title,
                &input,
                &error,
            )
            .await
            .map(|_| ())
            .map_err(|err| AcpRpcError {
                code: -32000,
                message: err,
            })
        }
    }
}

async fn external_effective_permissions(
    ctx: &AcpEventContext,
) -> Result<Vec<PermissionRule>, AcpRpcError> {
    let Some(session) = ctx
        .state
        .inner
        .store
        .get_session(&ctx.child_id)
        .await
        .map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("Could not load external session permissions: {err}"),
        })?
    else {
        return Ok(Vec::new());
    };
    let approval_scope = (
        crate::caller::session_tenant(&session).to_string(),
        session.project_id.clone(),
    );
    let mut rules = session.permission.unwrap_or_default();
    if let Some(extra) = ctx
        .state
        .inner
        .permission_approvals
        .read()
        .await
        .get(&approval_scope)
        .cloned()
    {
        rules.extend(extra);
    }
    Ok(rules)
}

fn display_workspace_path(cwd: &Path, path: &Path) -> String {
    path.strip_prefix(cwd)
        .ok()
        .and_then(|path| path.to_str())
        .filter(|path| !path.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| path.display().to_string())
}

async fn blocking_terminal_call(
    manager: AcpTerminalManager,
    params: Value,
    f: fn(AcpTerminalManager, Value) -> Result<Value, AcpRpcError>,
) -> Result<Value, AcpRpcError> {
    tokio::task::spawn_blocking(move || f(manager, params))
        .await
        .map_err(|err| AcpRpcError {
            code: -32000,
            message: format!("ACP terminal task failed: {err}"),
        })?
}

async fn ask_acp_permission(
    ctx: &AcpEventContext,
    params: &Value,
) -> Result<Option<String>, AcpRpcError> {
    let tool_call = params.get("toolCall").cloned().unwrap_or(Value::Null);
    let options = params.get("options").cloned().unwrap_or(Value::Null);
    let option_ids = options
        .as_array()
        .map(|options| {
            options
                .iter()
                .filter_map(|option| option.get("optionId").and_then(Value::as_str))
                .map(str::to_string)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let request_id = Id::ascending(IdKind::Permission).to_string();
    let title = tool_call
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("External agent permission")
        .to_string();
    let tool_call_id = tool_call
        .get("toolCallId")
        .or_else(|| tool_call.get("toolCallID"))
        .and_then(Value::as_str)
        .unwrap_or(&request_id)
        .to_string();
    let request = PermissionRequestInfo {
        id: request_id.clone(),
        session_id: ctx.child_id.clone(),
        message_id: ctx.assistant_id.to_string(),
        title: format!("{} needs approval: {title}", ctx.runtime.display_name()),
        permission: "external_agent".to_string(),
        patterns: vec![title.clone()],
        always: vec![title],
        tool: Some(json!({ "messageID": ctx.assistant_id, "callID": tool_call_id })),
        metadata: Some(json!({
            "runtime": "acp",
            "provider": ctx.runtime.provider_id(),
            "toolCall": tool_call,
            "options": options,
        })),
    };
    let mut events = ctx.state.subscribe();
    ctx.state
        .inner
        .permissions
        .write()
        .await
        .insert(request.id.clone(), request.clone());
    ctx.state.publish(EventPayload::new(
        event_type::PERMISSION_ASKED,
        crate::permission_runtime::permission_request_payload(&ctx.state, &request).await,
    ));

    loop {
        tokio::select! {
            event = events.recv() => {
                let Ok(event) = event else {
                    return Ok(None);
                };
                if event.kind != event_type::PERMISSION_REPLIED {
                    continue;
                }
                let request_matches = event.properties
                    .get("requestID")
                    .or_else(|| event.properties.get("requestId"))
                    .and_then(Value::as_str)
                    == Some(request_id.as_str());
                if !request_matches {
                    continue;
                }
                let reply = permission_reply_kind(event.properties.get("reply"))
                    .unwrap_or_else(|| "once".to_string());
                return Ok(select_acp_permission_option(&options, &option_ids, &reply));
            }
            _ = wait_for_cancel(ctx.cancellation.clone()) => {
                ctx.state.inner.permissions.write().await.remove(&request_id);
                return Ok(None);
            }
        }
    }
}

pub(crate) fn select_acp_permission_option(
    options: &Value,
    option_ids: &[String],
    reply: &str,
) -> Option<String> {
    if option_ids.is_empty() {
        return Some(reply.to_string());
    }
    if option_ids.iter().any(|option| option == reply) {
        return Some(reply.to_string());
    }
    let normalized = reply.to_ascii_lowercase();
    let preferred = match normalized.as_str() {
        "always" | "allow_always" => &[
            "allow_always",
            "always",
            "acceptEdits",
            "auto",
            "bypassPermissions",
            "allow",
        ][..],
        "reject" | "deny" | "no" => &["reject", "deny", "plan", "cancel"][..],
        _ => &["allow", "once", "default"][..],
    };
    for candidate in preferred {
        if option_ids.iter().any(|option| option == candidate) {
            return Some((*candidate).to_string());
        }
    }
    let desired_kind = match normalized.as_str() {
        "always" | "allow_always" => Some("allow_always"),
        "reject" | "deny" | "no" => Some("reject_once"),
        _ => Some("allow_once"),
    };
    options
        .as_array()
        .and_then(|items| {
            items.iter().find_map(|item| {
                let kind = item.get("kind").and_then(Value::as_str)?;
                if Some(kind) == desired_kind {
                    item.get("optionId").and_then(Value::as_str)
                } else {
                    None
                }
            })
        })
        .map(str::to_string)
        .or_else(|| option_ids.first().cloned())
}

fn permission_reply_kind(value: Option<&Value>) -> Option<String> {
    match value {
        Some(Value::String(value)) => Some(value.clone()),
        Some(Value::Object(map)) => map
            .get("reply")
            .or_else(|| map.get("response"))
            .and_then(Value::as_str)
            .map(str::to_string),
        _ => None,
    }
}
