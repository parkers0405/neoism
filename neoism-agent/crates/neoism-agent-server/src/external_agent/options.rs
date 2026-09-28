//! ACP-owned session selectors. The provider's complete option response is
//! authoritative; Neoism's model field is never sent as an ACP model choice.
use super::*;
use axum::extract::{Path, Query, State};
use axum::http::HeaderMap;
use axum::Extension;
use axum::Json;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
#[path = "options/cache.rs"]
mod cache;

const SETUP_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Deserialize)]
pub(crate) struct PreviewQuery {
    provider: String,
    directory: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct PreviewRequest {
    selected_options: BTreeMap<String, String>,
}

/// A process-local ACP session, never a Neoism session. Its ID is deliberately
/// not returned: the provider process is terminated when this request ends.
async fn ephemeral(
    state: &AppState,
    runtime: ExternalRuntime,
    directory: &str,
    choices: BTreeMap<String, String>,
) -> Result<ExternalOptionsResponse, ApiError> {
    let (client, mut events) = AcpClient::spawn(
        runtime
            .acp_config(directory, state.services())
            .map_err(ApiError::bad_request)?,
    )
    .map_err(ApiError::bad_request)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let initialize =
        super::acp_run::initialize_acp_client(&client, runtime, cancel.clone())
            .await
            .map_err(ApiError::bad_request)?;
    let setup = super::acp_run::setup_acp_session(
        &client,
        &initialize,
        directory,
        None,
        runtime,
        cancel,
    )
    .await
    .map_err(ApiError::bad_request)?;
    let (mut config_options, mut mode_fallback, selected_options) =
        replay_choices(runtime, &client, &initialize, &setup, choices, None)
            .await
            .map_err(ApiError::bad_request)?;
    let id = setup["sessionId"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("ACP session has no ID"))?;
    // Consume setup notifications without ever routing them into a Neoism root.
    tokio::time::sleep(Duration::from_millis(25)).await;
    let mut available_commands = Vec::new();
    while let Ok(event) = events.try_recv() {
        match event {
            AcpEvent::SessionUpdate {
                session_id, update, ..
            } if session_id == id => match update["sessionUpdate"].as_str() {
                Some("config_option_update") if selected_options.is_empty() => {
                    let (mut updated, new_fallback) = from_setup(&update)?;
                    let preserve_legacy_mode = mode_fallback
                        && !updated.iter().any(|item| {
                            item["id"] == "mode" || item["category"] == "mode"
                        });
                    if preserve_legacy_mode {
                        if let Some(mode) =
                            config_options.iter().find(|item| item["id"] == "mode")
                        {
                            updated.push(mode.clone());
                        }
                    }
                    mode_fallback = new_fallback || preserve_legacy_mode;
                    config_options = updated;
                }
                Some("available_commands_update") => {
                    available_commands = validate_commands(&update)?
                }
                _ => {}
            },
            AcpEvent::Request { id, .. } => {
                let _ = client.respond(
                    id,
                    Err(AcpRpcError {
                        code: -32601,
                        message: "No tools during ACP options preview".into(),
                    }),
                );
                return Err(ApiError::bad_request(
                    "ACP adapter requested a tool during options preview",
                ));
            }
            _ => {}
        }
    }
    Ok(ExternalOptionsResponse {
        provider: runtime.provider_id().into(),
        config_options,
        mode_fallback,
        selected_options,
        external_session_id: None,
        available_commands,
        replay_error: None,
        catalog_stale: None,
    })
}

pub(crate) async fn preview(
    State(state): State<AppState>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    preview_inner(state, query, headers, claims, BTreeMap::new()).await
}

pub(crate) async fn preview_selected(
    State(state): State<AppState>,
    Query(query): Query<PreviewQuery>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(body): Json<PreviewRequest>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    preview_inner(state, query, headers, claims, body.selected_options).await
}

async fn preview_inner(
    state: AppState,
    query: PreviewQuery,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    choices: BTreeMap<String, String>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    let runtime = match query.provider.as_str() {
        "opencode" => ExternalRuntime::OpenCode,
        "claude" => ExternalRuntime::Claude,
        "codex" => ExternalRuntime::Codex,
        _ => {
            return Err(ApiError::bad_request(
                "provider must be opencode, claude or codex",
            ))
        }
    };
    if state.services().hosted
        || claims.as_ref().is_some_and(|Extension(claims)| {
            claims.hosted || claims.tenant_id != "local" || claims.workspace_id.is_some()
        })
    {
        return Err(ApiError::forbidden(
            "Host-native ACP options require the local operator",
        ));
    }
    let directory = crate::resolve_directory(query.directory, &headers);
    let cwd = crate::windows_process::canonicalize_path(std::path::Path::new(&directory))
        .map_err(|_| ApiError::bad_request("Workspace directory does not exist"))?;
    if !cwd.is_dir() {
        return Err(ApiError::bad_request(
            "Workspace directory is not a directory",
        ));
    }
    let cwd = cwd.to_string_lossy().into_owned();
    if claims
        .as_ref()
        .is_some_and(|Extension(claims)| !crate::caller::allows_directory(claims, &cwd))
    {
        return Err(ApiError::forbidden(
            "Workspace directory is outside the caller's scope",
        ));
    }
    if choices.len() > 256 {
        return Err(ApiError::bad_request("Too many ACP options"));
    }
    Ok(Json(cache::preview(&state, runtime, &cwd, choices).await?))
}

pub(crate) async fn validate_draft(
    state: &AppState,
    runtime: ExternalRuntime,
    directory: &str,
    choices: BTreeMap<String, String>,
) -> Result<Value, ApiError> {
    if choices.len() > 256 {
        return Err(ApiError::bad_request("Too many ACP options"));
    }
    let confirmed = ephemeral(state, runtime, directory, choices).await?;
    // Save only confirmed selections, not the ephemeral provider session ID.
    Ok(json!({"selectedOptions": confirmed.selected_options,
        "configOptions": confirmed.config_options, "modeFallback": confirmed.mode_fallback,
        "optionsValid": false}))
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ExternalOptionsResponse {
    provider: String,
    /// The full provider-ordered ACP configOptions array. Unknown types remain
    /// visible, but only advertised select values can currently be changed.
    config_options: Vec<Value>,
    /// True when `configOptions` was absent and legacy ACP modes were used.
    mode_fallback: bool,
    /// Confirmed user choices to reapply to each new ACP process.
    selected_options: BTreeMap<String, String>,
    /// Opaque ACP session identity for this provider snapshot.
    external_session_id: Option<String>,
    available_commands: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    replay_error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    catalog_stale: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SetExternalOptionRequest {
    config_id: String,
    value: String,
}

pub(super) async fn session_lock(
    state: &AppState,
    id: &str,
) -> Arc<tokio::sync::Mutex<()>> {
    let mut locks = state.inner.external_session_locks.lock().await;
    locks
        .entry(id.to_owned())
        .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
        .clone()
}

fn option_values(option: &Value) -> Option<Vec<&str>> {
    let options = option.get("options")?.as_array()?;
    let mut values = Vec::new();
    for entry in options {
        if let Some(group) = entry.get("options") {
            for choice in group.as_array()? {
                values.push(choice.get("value")?.as_str()?);
            }
        } else {
            values.push(entry.get("value")?.as_str()?);
        }
    }
    Some(values)
}

fn validate_options(options: &Value) -> Result<Vec<Value>, ApiError> {
    let options = options.as_array().ok_or_else(|| {
        ApiError::bad_request("ACP configOptions must be a complete array")
    })?;
    if options.len() > 256 {
        return Err(ApiError::bad_request(
            "ACP configOptions exceed 256 entries",
        ));
    }
    let mut ids = BTreeSet::new();
    for option in options {
        let id = option
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .ok_or_else(|| ApiError::bad_request("ACP config option has no id"))?;
        if !ids.insert(id) {
            return Err(ApiError::bad_request(
                "ACP config option IDs are duplicated",
            ));
        }
        if option["type"] == "select" {
            let current = option["currentValue"].as_str().ok_or_else(|| {
                ApiError::bad_request("ACP select option has no currentValue")
            })?;
            if !option_values(option).is_some_and(|choices| choices.contains(&current)) {
                return Err(ApiError::bad_request(
                    "ACP select option currentValue is not advertised",
                ));
            }
        }
    }
    Ok(options.clone())
}

fn from_setup(response: &Value) -> Result<(Vec<Value>, bool), ApiError> {
    let mut options = match response.get("configOptions") {
        Some(options) => validate_options(options)?,
        None => Vec::new(),
    };
    if options
        .iter()
        .any(|option| option["category"] == "mode" || option["id"] == "mode")
    {
        return Ok((options, false));
    }
    let Some(modes) = response.get("modes") else {
        return Ok((options, false));
    };
    let current = modes["currentModeId"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("ACP modes has no currentModeId"))?;
    let available = modes["availableModes"]
        .as_array()
        .ok_or_else(|| ApiError::bad_request("ACP modes has no availableModes"))?;
    let choices = available.iter().map(|mode| {
        Ok(json!({
            "value":mode["id"].as_str().ok_or_else(|| ApiError::bad_request("ACP mode has no id"))?,
            "name":mode["name"].as_str().ok_or_else(|| ApiError::bad_request("ACP mode has no name"))?,
            "description":mode.get("description"),
        }))
    }).collect::<Result<Vec<_>, ApiError>>()?;
    let legacy = json!({"id":"mode","name":"Mode","category":"mode","type":"select","currentValue":current,"options":choices});
    options.push(legacy);
    Ok((validate_options(&json!(options))?, true))
}

fn selected(session: &SessionInfo) -> BTreeMap<String, String> {
    session.extra["externalAgent"]["selectedOptions"]
        .as_object()
        .map(|map| {
            map.iter()
                .filter_map(|(id, value)| Some((id.clone(), value.as_str()?.to_owned())))
                .collect()
        })
        .unwrap_or_default()
}

async fn persist(
    state: &AppState,
    session_id: &str,
    options: &[Value],
    fallback: bool,
    choices: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut info = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("ACP root not found"))?;
    let previous = info.extra["externalAgent"].clone();
    let external = info
        .extra
        .get_mut("externalAgent")
        .ok_or_else(|| ApiError::bad_request("Not an ACP root"))?;
    external["configOptions"] = json!(options);
    external["modeFallback"] = json!(fallback);
    external["selectedOptions"] = json!(choices);
    external["optionsValid"] = json!(true);
    if info.extra["externalAgent"] == previous {
        return Ok(());
    }
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID":info.id,"info":info}),
    ));
    Ok(())
}

pub(super) async fn invalidate_replay(
    state: &AppState,
    session_id: &str,
) -> Result<(), ApiError> {
    let Some(mut info) = state.inner.store.get_session(session_id).await? else {
        return Ok(());
    };
    let Some(external) = info.extra.get_mut("externalAgent") else {
        return Ok(());
    };
    if external["optionsValid"] == false {
        return Ok(());
    }
    external["optionsValid"] = json!(false);
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID":info.id,"info":info}),
    ));
    Ok(())
}

fn validate_commands(update: &Value) -> Result<Vec<Value>, ApiError> {
    let items = update
        .get("availableCommands")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ApiError::bad_request(
                "ACP available_commands_update requires availableCommands array",
            )
        })?;
    if items.len() > 256 || update["availableCommands"].to_string().len() > 131_072 {
        return Err(ApiError::bad_request(
            "ACP command list exceeds size limits",
        ));
    }
    let mut names = BTreeSet::new();
    for item in items {
        let name = item
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request("ACP command has no name"))?;
        if name.is_empty()
            || name.len() > 128
            || name.starts_with('/')
            || !name.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
            || !names.insert(name)
        {
            return Err(ApiError::bad_request(
                "ACP command name is invalid or duplicated",
            ));
        }
        let description = item
            .get("description")
            .and_then(Value::as_str)
            .ok_or_else(|| ApiError::bad_request("ACP command has no description"))?;
        if description.len() > 2048
            || description
                .chars()
                .any(|ch| ch.is_control() && !matches!(ch, '\n' | '\r' | '\t'))
        {
            return Err(ApiError::bad_request("ACP command description is invalid"));
        }
        if let Some(input) = item.get("input").filter(|input| !input.is_null()) {
            let hint = input.get("hint").and_then(Value::as_str).ok_or_else(|| {
                ApiError::bad_request("ACP command input hint is invalid")
            })?;
            if hint.len() > 512 || hint.chars().any(char::is_control) {
                return Err(ApiError::bad_request("ACP command input hint is invalid"));
            }
        }
    }
    Ok(items.clone())
}

pub(super) async fn apply_commands(
    state: &AppState,
    session_id: &str,
    acp_id: &str,
    update: &Value,
) -> Result<(), ApiError> {
    let Some(mut session) = state.inner.store.get_session(session_id).await? else {
        return Ok(());
    };
    let Some(external) = session.extra.get_mut("externalAgent") else {
        return Ok(());
    };
    if external["externalSessionId"].as_str() != Some(acp_id) {
        return Ok(());
    }
    let commands = validate_commands(update)?;
    if external["availableCommands"] == json!(commands) {
        return Ok(());
    }
    external["availableCommands"] = json!(commands);
    state.inner.store.update_session(&session).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID": session.id, "info": session}),
    ));
    Ok(())
}

pub(super) async fn clear_commands(
    state: &AppState,
    session_id: &str,
    acp_id: &str,
) -> Result<(), ApiError> {
    apply_commands(state, session_id, acp_id, &json!({"availableCommands": []})).await
}

fn response(session: &SessionInfo, runtime: ExternalRuntime) -> ExternalOptionsResponse {
    ExternalOptionsResponse {
        provider: runtime.provider_id().into(),
        config_options: session.extra["externalAgent"]["configOptions"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        mode_fallback: session.extra["externalAgent"]["modeFallback"] == true,
        selected_options: selected(session),
        external_session_id: session.extra["externalAgent"]["externalSessionId"]
            .as_str()
            .map(str::to_owned),
        available_commands: session.extra["externalAgent"]["availableCommands"]
            .as_array()
            .cloned()
            .unwrap_or_default(),
        replay_error: None,
        catalog_stale: None,
    }
}

fn option<'a>(
    options: &'a [Value],
    id: &str,
    value: &str,
) -> Result<&'a Value, ApiError> {
    let option = options
        .iter()
        .find(|option| option["id"] == id)
        .ok_or_else(|| ApiError::bad_request("ACP configId is not advertised"))?;
    if option["type"] != "select" {
        return Err(ApiError::bad_request(
            "Only advertised select ACP options are supported",
        ));
    }
    if !option_values(option).is_some_and(|choices| choices.contains(&value)) {
        return Err(ApiError::bad_request("ACP option value is not advertised"));
    }
    Ok(option)
}

async fn set_confirmed(
    client: &AcpClient,
    initialize: &Value,
    runtime: ExternalRuntime,
    session_id: &str,
    config_id: &str,
    value: &str,
    options: &[Value],
    fallback: bool,
) -> Result<(Vec<Value>, bool), ApiError> {
    option(options, config_id, value)?;
    if fallback && config_id == "mode" {
        client
            .request(
                "session/set_mode",
                json!({"sessionId":session_id,"modeId":value}),
                SETUP_TIMEOUT,
            )
            .await
            .map_err(|error| {
                ApiError::bad_request(super::acp_run::session_setup_error(
                    runtime,
                    "session/set_mode",
                    &error,
                    initialize,
                ))
            })?;
        let mut confirmed = options.to_vec();
        let mode = confirmed
            .iter_mut()
            .find(|option| option["id"] == "mode")
            .ok_or_else(|| ApiError::bad_request("Legacy ACP mode disappeared"))?;
        mode["currentValue"] = json!(value);
        return Ok((confirmed, true));
    }
    let result = client
        .request(
            "session/set_config_option",
            json!({"sessionId":session_id,"configId":config_id,"value":value}),
            SETUP_TIMEOUT,
        )
        .await
        .map_err(|error| {
            ApiError::bad_request(super::acp_run::session_setup_error(
                runtime,
                "session/set_config_option",
                &error,
                initialize,
            ))
        })?;
    let mut confirmed =
        validate_options(result.get("configOptions").ok_or_else(|| {
            ApiError::bad_request(
                "ACP set_config_option omitted the complete configOptions response",
            )
        })?)?;
    if fallback
        && !confirmed
            .iter()
            .any(|item| item["category"] == "mode" || item["id"] == "mode")
    {
        let legacy = options
            .iter()
            .find(|item| item["id"] == "mode")
            .ok_or_else(|| ApiError::bad_request("Legacy ACP mode disappeared"))?;
        confirmed.push(legacy.clone());
    }
    let fallback = fallback
        && confirmed
            .iter()
            .any(|item| item["id"] == "mode" && item["category"] == "mode")
        && !result["configOptions"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|item| item["id"] == "mode" || item["category"] == "mode")
        });
    if confirmed
        .iter()
        .find(|item| item["id"] == config_id)
        .is_none_or(|item| item["currentValue"] != value)
    {
        return Err(ApiError::conflict(
            "ACP did not confirm the requested option value",
        ));
    }
    Ok((confirmed, fallback))
}

/// On each fresh process, replay choices in provider order, rechecking each
/// dependency against the *latest complete* response. A removed/invalid value
/// aborts the prompt rather than silently switching provider model or mode.
async fn replay_choices(
    runtime: ExternalRuntime,
    client: &AcpClient,
    initialize: &Value,
    setup: &Value,
    mut choices: BTreeMap<String, String>,
    requested: Option<&SetExternalOptionRequest>,
) -> Result<(Vec<Value>, bool, BTreeMap<String, String>), String> {
    let id = setup["sessionId"].as_str().ok_or("ACP session has no ID")?;
    let (mut options, mut fallback) =
        from_setup(setup).map_err(|error| error.to_string())?;
    if let Some(requested) = requested {
        choices.insert(requested.config_id.clone(), requested.value.clone());
    }
    let mut pending = choices.keys().cloned().collect::<BTreeSet<_>>();
    while !pending.is_empty() {
        // Selectors can be conditional: OpenCode, for example, only advertises
        // effort after switching from its default model to a reasoning model.
        let next = options
            .iter()
            .filter(|item| {
                item["id"].as_str().is_some_and(|id| {
                    pending.contains(id) && option(&options, id, &choices[id]).is_ok()
                })
            })
            .min_by_key(|item| if item["category"] == "model" { 0 } else { 1 })
            .and_then(|item| item["id"].as_str())
            .map(str::to_owned);
        let Some(next) = next else {
            if requested.is_some_and(|choice| !pending.contains(&choice.config_id)) {
                // An explicit change may remove dependent selectors. Keep only
                // values the provider still confirms after that change.
                break;
            }
            return Err(
                "ACP session no longer advertises a selected config option or value"
                    .into(),
            );
        };
        let value = &choices[&next];
        if options
            .iter()
            .all(|item| item["id"] != next || item["currentValue"] != value.as_str())
        {
            (options, fallback) = set_confirmed(
                client, initialize, runtime, id, &next, value, &options, fallback,
            )
            .await
            .map_err(|error| error.to_string())?;
        }
        pending.remove(&next);
    }
    if let Some(requested) = requested {
        if options
            .iter()
            .find(|item| item["id"] == requested.config_id)
            .is_none_or(|item| item["currentValue"] != requested.value)
        {
            return Err("ACP did not confirm the requested option value".into());
        }
        choices.retain(|id, value| {
            if id == &requested.config_id {
                return true;
            }
            let Some(current) = options
                .iter()
                .find(|item| item["id"].as_str() == Some(id.as_str()))
                .and_then(|item| item["currentValue"].as_str())
            else {
                return false;
            };
            *value = current.to_owned();
            true
        });
    } else if choices.iter().any(|(id, value)| {
        options
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .is_none_or(|item| item["currentValue"].as_str() != Some(value.as_str()))
    }) {
        return Err("ACP did not preserve all selected config option values".into());
    }
    Ok((options, fallback, choices))
}

pub(super) async fn setup_and_reapply(
    state: &AppState,
    session: &SessionInfo,
    runtime: ExternalRuntime,
    client: &AcpClient,
    initialize: &Value,
    setup: &Value,
    requested: Option<&SetExternalOptionRequest>,
) -> Result<(), String> {
    let (options, fallback, choices) = replay_choices(
        runtime,
        client,
        initialize,
        setup,
        selected(session),
        requested,
    )
    .await?;
    let id = setup["sessionId"].as_str().ok_or("ACP session has no ID")?;
    persist(state, session.id.as_str(), &options, fallback, &choices)
        .await
        .map_err(|error| error.to_string())?;
    clear_commands(state, session.id.as_str(), id)
        .await
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub(super) async fn apply_notification(
    state: &AppState,
    session_id: &str,
    update: &Value,
) -> Result<(), ApiError> {
    let mut options =
        validate_options(update.get("configOptions").ok_or_else(|| {
            ApiError::bad_request(
                "ACP config_option_update has no complete configOptions",
            )
        })?)?;
    let session = state
        .inner
        .store
        .get_session(session_id)
        .await?
        .ok_or_else(|| ApiError::not_found("ACP root not found"))?;
    let fallback = session.extra["externalAgent"]["modeFallback"] == true
        && !options
            .iter()
            .any(|option| option["category"] == "mode" || option["id"] == "mode");
    if fallback {
        if let Some(mode) = session.extra["externalAgent"]["configOptions"]
            .as_array()
            .and_then(|items| items.iter().find(|item| item["id"] == "mode"))
        {
            options.push(mode.clone());
        }
    }
    let choices = selected(&session);
    // A session/load notification may arrive after the confirmed replay. It
    // describes that process's transient defaults, not a new user selection.
    if choices.iter().any(|(id, value)| {
        options
            .iter()
            .find(|item| item["id"].as_str() == Some(id.as_str()))
            .is_none_or(|item| item["currentValue"].as_str() != Some(value.as_str()))
    }) {
        return Ok(());
    }
    persist(state, session_id, &options, fallback, &choices).await
}

fn scope(
    state: &AppState,
    claims: Option<&crate::caller::CallerClaims>,
    session: &SessionInfo,
) -> Result<ExternalRuntime, ApiError> {
    if state.services().hosted
        || claims.is_some_and(|claims| {
            claims.hosted
                || claims.tenant_id != "local"
                || claims.workspace_id.is_some()
                || !crate::caller::allows_session(claims, session)
        })
    {
        return Err(ApiError::forbidden(
            "Host-native ACP options require the local operator and authorized session",
        ));
    }
    let runtime = root_runtime(session)
        .ok_or_else(|| ApiError::bad_request("Session is not an external ACP root"))?;
    if matches!(
        session.extra["externalAgent"]["historyState"].as_str(),
        Some("importing" | "not_loaded")
    ) {
        return Err(ApiError::conflict(
            "Native chat history has not been imported",
        ));
    }
    if session.extra["externalAgent"]
        .get("sourceHost")
        .is_some_and(|host| {
            host.as_str() != Some(super::catalog::native_host_id().as_str())
        })
    {
        return Err(ApiError::conflict(
            "Native ACP session belongs to another host",
        ));
    }
    Ok(runtime)
}

async fn idle(state: &AppState, id: &str) -> Result<(), ApiError> {
    if state
        .inner
        .session_coordinator
        .active_run(id)
        .await
        .is_some()
        || state.inner.session_coordinator.worker_active(id).await
        || state.inner.store.queued_prompt_count(id).await? > 0
    {
        return Err(ApiError::conflict(
            "ACP options cannot change while a prompt is queued or running",
        ));
    }
    Ok(())
}

async fn drain_control_events(
    state: &AppState,
    session_id: &str,
    acp_id: &str,
    client: &AcpClient,
    events: &mut tokio::sync::mpsc::UnboundedReceiver<AcpEvent>,
) -> Result<(), ApiError> {
    // ACP request responses are read after preceding updates on the same
    // stream. The options route owns this receiver, so it must NOT call
    // client.drain_events() here (that barrier waits for a separate consumer).
    // Give queued post-setup notifications a bounded quiet window; loading a
    // provider session can replay its command snapshot just after the RPC reply.
    tokio::time::sleep(Duration::from_millis(25)).await;
    while let Ok(event) = events.try_recv() {
        match event {
            AcpEvent::SessionUpdate {
                session_id: from,
                update,
                ..
            } if from == acp_id && update["sessionUpdate"] == "config_option_update" => {
                apply_notification(state, session_id, &update).await?
            }
            AcpEvent::SessionUpdate {
                session_id: from,
                update,
                ..
            } if from == acp_id
                && update["sessionUpdate"] == "available_commands_update" =>
            {
                apply_commands(state, session_id, acp_id, &update).await?
            }
            AcpEvent::Request { id, .. } => {
                let _ = client.respond(
                    id,
                    Err(AcpRpcError {
                        code: -32601,
                        message: "No tools during ACP options discovery".into(),
                    }),
                );
                return Err(ApiError::bad_request(
                    "ACP adapter requested a tool during options discovery",
                ));
            }
            _ => {}
        }
    }
    Ok(())
}

async fn control(
    state: &AppState,
    session: &SessionInfo,
    runtime: ExternalRuntime,
    choice: Option<&SetExternalOptionRequest>,
) -> Result<ExternalOptionsResponse, ApiError> {
    let (client, mut events) = AcpClient::spawn(
        runtime
            .acp_config(&session.directory, state.services())
            .map_err(ApiError::bad_request)?,
    )
    .map_err(ApiError::bad_request)?;
    let cancel = Arc::new(AtomicBool::new(false));
    let initialize =
        super::acp_run::initialize_acp_client(&client, runtime, cancel.clone())
            .await
            .map_err(ApiError::bad_request)?;
    let setup = super::acp_run::setup_acp_session(
        &client,
        &initialize,
        &session.directory,
        external_session_id(session, runtime),
        runtime,
        cancel,
    )
    .await
    .map_err(ApiError::bad_request)?;
    let id = setup["sessionId"]
        .as_str()
        .ok_or_else(|| ApiError::bad_request("ACP session has no ID"))?;
    update_external_session_metadata(state, session.id.as_str(), runtime, id, "ready")
        .await?;
    if let Err(error) = setup_and_reapply(
        state,
        session,
        runtime,
        &client,
        &initialize,
        &setup,
        choice,
    )
    .await
    {
        invalidate_replay(state, session.id.as_str()).await?;
        if choice.is_none() {
            let saved = state
                .inner
                .store
                .get_session(session.id.as_str())
                .await?
                .ok_or_else(|| ApiError::not_found("ACP root not found"))?;
            if saved.extra["externalAgent"]["configOptions"]
                .as_array()
                .is_some_and(|items| !items.is_empty())
            {
                let mut snapshot = response(&saved, runtime);
                snapshot.replay_error = Some(error);
                return Ok(snapshot);
            }
        }
        return Err(ApiError::bad_request(error));
    }
    // load may replay historical updates. Only full config_option_update
    // snapshots affect options; never project replay as a new transcript.
    drain_control_events(state, session.id.as_str(), id, &client, &mut events).await?;
    let latest = state
        .inner
        .store
        .get_session(session.id.as_str())
        .await?
        .ok_or_else(|| ApiError::not_found("ACP root not found"))?;
    Ok(response(&latest, runtime))
}

async fn handle(
    state: AppState,
    id: String,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    choice: Option<SetExternalOptionRequest>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    let session = state
        .inner
        .store
        .get_session(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    let runtime = scope(
        &state,
        claims.as_ref().map(|Extension(claims)| claims),
        &session,
    )?;
    if choice.is_none()
        && session.extra["externalAgent"]["configOptions"].is_array()
        && session.extra["externalAgent"]["optionsValid"] != false
        && session.extra["externalAgent"]["externalSessionId"]
            .as_str()
            .is_some()
    {
        return Ok(Json(response(&session, runtime)));
    }
    idle(&state, &id).await?;
    let lock = session_lock(&state, &id).await;
    let _guard = lock.try_lock().map_err(|_| {
        ApiError::conflict("ACP prompt or options request is already running")
    })?;
    idle(&state, &id).await?;
    let session = state
        .inner
        .store
        .get_session(&id)
        .await?
        .ok_or_else(|| ApiError::not_found("Session not found"))?;
    scope(
        &state,
        claims.as_ref().map(|Extension(claims)| claims),
        &session,
    )?;
    if let Some(choice) = &choice {
        if let Some(options) = session.extra["externalAgent"]["configOptions"].as_array()
        {
            option(options, &choice.config_id, &choice.value)?;
        }
    }
    Ok(Json(
        control(&state, &session, runtime, choice.as_ref()).await?,
    ))
}

pub(crate) async fn external_options_get(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    handle(state, id, claims, None).await
}

pub(crate) async fn external_options_set(
    State(state): State<AppState>,
    Path(id): Path<String>,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(choice): Json<SetExternalOptionRequest>,
) -> Result<Json<ExternalOptionsResponse>, ApiError> {
    handle(state, id, claims, Some(choice)).await
}

#[cfg(all(test, unix))]
#[path = "options_tests.rs"]
mod tests;
