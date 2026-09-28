//! Capability-gated, host-local ACP history discovery. Listing never imports a
//! transcript or changes the Neoism session store; callers must not treat a
//! catalog preview as a loaded conversation.
use super::*;
use axum::extract::{Query, State};
use axum::http::HeaderMap;
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::Connection;
use std::collections::HashSet;
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalCatalogQuery {
    provider: String,
    directory: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalCatalogEntry {
    provider: String,
    /// Stable for this tenant, provider, canonical cwd and opaque provider ID.
    source_key: String,
    external_session_id: String,
    cwd: String,
    title: Option<String>,
    updated_at: Option<String>,
    /// An ACP list result is only a preview; it is not a transcript.
    history_state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    neoism_session_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalCatalogResponse {
    provider: String,
    cwd: String,
    sessions: Vec<ExternalCatalogEntry>,
    /// Whether this adapter advertises a history replay that can be imported.
    import_supported: bool,
    /// Present when replay is unavailable; a catalog preview is never a transcript.
    #[serde(skip_serializing_if = "Option::is_none")]
    import_unavailable_reason: Option<String>,
}

pub(crate) fn is_importing(session: &SessionInfo) -> bool {
    session
        .extra
        .get("externalAgent")
        .is_some_and(|external| external["historyState"] == "importing")
}

pub(super) fn native_host_id() -> String {
    std::env::var("NEOISM_HOST_ID")
        .ok()
        .filter(|id| !id.trim().is_empty())
        .unwrap_or_else(|| gethostname::gethostname().to_string_lossy().into_owned())
}

pub(super) fn source_key_for(
    runtime: ExternalRuntime,
    tenant: &str,
    cwd: &Path,
    external_session_id: &str,
) -> Option<String> {
    let host_id = native_host_id();
    let mut hasher = Sha256::new();
    for segment in [
        host_id.as_str(),
        tenant,
        runtime.provider_id(),
        cwd.to_str()?,
        external_session_id,
    ] {
        hasher.update((segment.len() as u64).to_be_bytes());
        hasher.update(segment.as_bytes());
    }
    Some(format!("acp:{}:{:x}", runtime.provider_id(), hasher.finalize()))
}

fn verified_catalog_entry(
    runtime: ExternalRuntime,
    tenant: &str,
    cwd: &Path,
    candidate: &Value,
) -> Option<ExternalCatalogEntry> {
    let external_session_id = candidate.get("sessionId")?.as_str()?.trim();
    if external_session_id.is_empty() || external_session_id.len() > 4096 {
        return None;
    }
    let candidate_path = candidate.get("cwd")?.as_str()?;
    let canonical =
        crate::windows_process::canonicalize_path(Path::new(candidate_path)).ok()?;
    // ACP's cwd filter is advisory: reject anything outside this exact root,
    // including a sibling, nested directory, missing directory or bad symlink.
    if canonical != cwd {
        return None;
    }
    let source_key = source_key_for(runtime, tenant, cwd, external_session_id)?;
    Some(ExternalCatalogEntry {
        provider: runtime.provider_id().into(),
        source_key,
        external_session_id: external_session_id.into(),
        cwd: cwd.to_string_lossy().into_owned(),
        title: candidate
            .get("title")
            .and_then(Value::as_str)
            .map(str::to_owned),
        updated_at: candidate
            .get("updatedAt")
            .and_then(Value::as_str)
            .map(str::to_owned),
        history_state: "not_loaded",
        neoism_session_id: None,
    })
}

async fn fetch_catalog_entries(
    client: &AcpClient,
    initialize: &Value,
    runtime: ExternalRuntime,
    tenant: &str,
    cwd: &Path,
) -> Result<Vec<ExternalCatalogEntry>, ApiError> {
    let mut seen_cursors = HashSet::new();
    let mut seen_ids = HashSet::new();
    let mut cursor: Option<String> = None;
    let mut sessions = Vec::new();
    for _ in 0..20 {
        let mut params = json!({ "cwd": cwd.to_string_lossy() });
        if let Some(value) = &cursor {
            params["cursor"] = json!(value);
        }
        let page = client
            .request("session/list", params, Duration::from_secs(30))
            .await
            .map_err(|err| {
                ApiError::bad_request(super::acp_run::session_setup_error(
                    runtime,
                    "session/list",
                    &err,
                    &initialize,
                ))
            })?;
        let rows = page
            .get("sessions")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                ApiError::bad_request("ACP session/list response has no sessions array")
            })?;
        for candidate in rows {
            if let Some(entry) = verified_catalog_entry(runtime, tenant, &cwd, candidate)
            {
                if seen_ids.insert(entry.external_session_id.clone()) {
                    sessions.push(entry);
                }
            }
            if sessions.len() > 1000 {
                return Err(ApiError::bad_request("ACP history exceeds 1000 sessions"));
            }
        }
        cursor = page
            .get("nextCursor")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .map(str::to_owned);
        match &cursor {
            None => break,
            Some(value) if !seen_cursors.insert(value.clone()) => {
                return Err(ApiError::bad_request("ACP session/list repeated a cursor"))
            }
            _ => {}
        }
    }
    if cursor.is_some() {
        return Err(ApiError::bad_request("ACP history exceeds 20 pages"));
    }
    Ok(sessions)
}

// ACP session/list includes sessions created by options previews. OpenCode's
// local message store is the only positive evidence that a listed row is a
// conversation; an unreadable or missing row is unknown, not empty.
async fn hide_empty_opencode_sessions(sessions: &mut Vec<ExternalCatalogEntry>, cwd: &str, db_path: &Path) {
    let options = sqlx::sqlite::SqliteConnectOptions::new()
        .filename(db_path)
        .read_only(true);
    let Ok(mut db) = sqlx::SqliteConnection::connect_with(&options).await else { return; };
    let mut visible = Vec::with_capacity(sessions.len());
    for entry in sessions.drain(..) {
        if entry.neoism_session_id.is_some() {
            visible.push(entry);
            continue;
        }
        let user_turn = sqlx::query_scalar::<_, i64>(
            "SELECT EXISTS (SELECT 1 FROM message WHERE session_id = session.id AND json_extract(data, '$.role') = 'user') FROM session WHERE id = ? AND directory = ?",
        )
        .bind(&entry.external_session_id)
        .bind(cwd)
        .fetch_optional(&mut db)
        .await;
        if !matches!(user_turn, Ok(Some(0))) {
            visible.push(entry);
        }
    }
    *sessions = visible;
}

pub(crate) async fn external_catalog(
    State(state): State<AppState>,
    Query(query): Query<ExternalCatalogQuery>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
) -> Result<Json<ExternalCatalogResponse>, ApiError> {
    // A process-wide CLI credential store cannot safely enumerate a hosted
    // tenant's native history, regardless of the ACP adapter's cwd filtering.
    if state.services().hosted {
        return Err(ApiError::forbidden(
            "Native ACP history is available only on the local host",
        ));
    }
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
    let directory = crate::resolve_directory(query.directory, &headers);
    let cwd = crate::windows_process::canonicalize_path(Path::new(&directory))
        .map_err(|_| ApiError::bad_request("Workspace directory does not exist"))?;
    if !cwd.is_dir() {
        return Err(ApiError::bad_request(
            "Workspace directory is not a directory",
        ));
    }
    let cwd_text = cwd.to_string_lossy().into_owned();
    if claims.as_ref().is_some_and(|Extension(claims)| {
        !crate::caller::allows_directory(claims, &cwd_text)
    }) {
        return Err(ApiError::forbidden(
            "Workspace directory is outside the caller's scope",
        ));
    }
    let tenant = claims
        .as_ref()
        .map(|Extension(claims)| claims.tenant_id.as_str())
        .unwrap_or("local");
    if claims.as_ref().is_some_and(|Extension(claims)| {
        claims.hosted || claims.tenant_id != "local" || claims.workspace_id.is_some()
    }) {
        return Err(ApiError::forbidden("Host-native ACP history is not isolated by workspace identity; use the local operator"));
    }
    let (client, _events) = AcpClient::spawn(
        runtime
            .acp_config(&cwd_text, state.services())
            .map_err(ApiError::bad_request)?,
    )
    .map_err(|err| {
        ApiError::bad_request(format!(
            "{} ACP launch failed: {err}",
            runtime.display_name()
        ))
    })?;
    let initialize = super::acp_run::initialize_acp_client(
        &client,
        runtime,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .map_err(ApiError::bad_request)?;
    if !initialize["agentCapabilities"]["sessionCapabilities"]["list"].is_object() {
        return Err(ApiError::bad_request(format!(
            "{} ACP adapter does not advertise sessionCapabilities.list",
            runtime.display_name()
        )));
    }
    let mut sessions =
        fetch_catalog_entries(&client, &initialize, runtime, tenant, &cwd).await?;
    let existing = state.inner.store.list_sessions().await?;
    for entry in &mut sessions {
        entry.neoism_session_id = existing
            .iter()
            .find(|session| {
                session.parent_id.is_none()
                    && session.directory == cwd_text
                    && crate::caller::session_tenant(session) == tenant
                    && claims.as_ref().is_none_or(|Extension(claims)| {
                        crate::caller::allows_session(claims, session)
                    })
                    && session.extra.get("externalAgent").is_some_and(|external| {
                        external["provider"] == runtime.provider_id()
                            && external["sourceHost"] == native_host_id()
                            && external["externalSessionId"] == entry.external_session_id
                    })
            })
            .and_then(|session| {
                entry.history_state =
                    match session.extra["externalAgent"]["historyState"].as_str() {
                        Some("text_only") => "text_only",
                        Some("importing") => "importing",
                        _ => "not_loaded",
                    };
                (!is_importing(session)).then(|| session.id.to_string())
            });
    }
    if runtime == ExternalRuntime::OpenCode {
        if let Some(path) = dirs::data_dir().map(|dir| dir.join("opencode/opencode.db")) {
            hide_empty_opencode_sessions(&mut sessions, &cwd_text, &path).await;
        }
    }
    Ok(Json(ExternalCatalogResponse {
        provider: runtime.provider_id().into(),
        cwd: cwd_text,
        sessions,
        import_supported: initialize["agentCapabilities"]["loadSession"] == true,
        import_unavailable_reason: (initialize["agentCapabilities"]["loadSession"]
            != true)
            .then(|| {
                format!(
                    "{} ACP adapter does not advertise session/load replay",
                    runtime.display_name()
                )
            }),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn opencode_catalog_hides_only_known_empty_unimported_sessions() {
        let root = std::env::temp_dir().join(format!("opencode-catalog-{}", Id::ascending(IdKind::Event)));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = std::fs::canonicalize(&root).unwrap();
        let db_path = root.join("opencode.db");
        let mut db = sqlx::SqliteConnection::connect_with(
            &sqlx::sqlite::SqliteConnectOptions::new().filename(&db_path).create_if_missing(true)
        ).await.unwrap();
        sqlx::query("CREATE TABLE session (id TEXT PRIMARY KEY, directory TEXT NOT NULL)").execute(&mut db).await.unwrap();
        sqlx::query("CREATE TABLE message (id TEXT PRIMARY KEY, session_id TEXT NOT NULL, data TEXT NOT NULL)").execute(&mut db).await.unwrap();
        let directory = cwd.to_str().unwrap();
        for id in ["preview", "real-new-title", "imported"] {
            sqlx::query("INSERT INTO session (id, directory) VALUES (?, ?)").bind(id).bind(directory).execute(&mut db).await.unwrap();
        }
        sqlx::query("INSERT INTO message (id, session_id, data) VALUES ('msg', 'real-new-title', '{\"role\":\"user\"}')").execute(&mut db).await.unwrap();
        drop(db);
        let mut entries = ["preview", "real-new-title", "imported", "unknown"].into_iter().map(|id| {
            verified_catalog_entry(ExternalRuntime::OpenCode, "local", &cwd,
                &json!({"sessionId":id,"cwd":directory,"title":"New session"})).unwrap()
        }).collect::<Vec<_>>();
        entries[2].neoism_session_id = Some("root".into());
        hide_empty_opencode_sessions(&mut entries, directory, &db_path).await;
        assert_eq!(entries.iter().map(|entry| entry.external_session_id.as_str()).collect::<Vec<_>>(),
            vec!["real-new-title", "imported", "unknown"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn rejects_provider_rows_outside_exact_workspace_and_stabilizes_source_key() {
        let root = std::env::temp_dir()
            .join(format!("acp-catalog-{}", Id::ascending(IdKind::Event)));
        std::fs::create_dir_all(root.join("other")).unwrap();
        let cwd = std::fs::canonicalize(&root).unwrap();
        let row = json!({ "sessionId": "opaque", "cwd": cwd, "title": "Real title" });
        let first =
            verified_catalog_entry(ExternalRuntime::Claude, "tenant-a", &cwd, &row)
                .unwrap();
        assert_eq!(first.history_state, "not_loaded");
        assert_eq!(
            first.source_key,
            verified_catalog_entry(ExternalRuntime::Claude, "tenant-a", &cwd, &row)
                .unwrap()
                .source_key
        );
        assert_ne!(
            first.source_key,
            verified_catalog_entry(ExternalRuntime::Claude, "tenant-b", &cwd, &row)
                .unwrap()
                .source_key
        );
        assert_ne!(
            first.source_key,
            verified_catalog_entry(ExternalRuntime::Codex, "tenant-a", &cwd, &row)
                .unwrap()
                .source_key
        );
        let other = json!({ "sessionId": "opaque", "cwd": root.join("other") });
        assert!(verified_catalog_entry(
            ExternalRuntime::Claude,
            "tenant-a",
            &cwd,
            &other
        )
        .is_none());
        assert!(verified_catalog_entry(
            ExternalRuntime::Claude,
            "tenant-a",
            &cwd,
            &json!({ "sessionId":"opaque" })
        )
        .is_none());
        let _ = std::fs::remove_dir_all(root);
    }
}

#[cfg(all(test, unix))]
mod catalog_rpc_tests {
    use super::*;

    #[tokio::test]
    async fn paged_list_deduplicates_and_filters_adversarial_cwd() {
        let root = std::env::temp_dir().join(format!(
            "acp-catalog-pages-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(root.join("sibling")).unwrap();
        let cwd = std::fs::canonicalize(&root).unwrap();
        let page1 = json!({ "jsonrpc":"2.0", "id":1, "result":{
            "sessions":[
                {"sessionId":"good", "cwd":cwd, "title":"original"},
                {"sessionId":"foreign", "cwd":root.join("sibling"), "title":"private"}
            ], "nextCursor":"opaque"
        }})
        .to_string();
        let page2 = json!({ "jsonrpc":"2.0", "id":2, "result":{
            "sessions":[{"sessionId":"good", "cwd":cwd, "title":"duplicate"}]
        }})
        .to_string();
        let script = format!("read line; printf '%s\\n' '{page1}'; read line; printf '%s\\n' '{page2}'; read line");
        let config = AcpServerConfig::new("mock", "Mock", "/bin/sh", cwd.clone())
            .args(["-c", &script]);
        let (client, _events) = AcpClient::spawn(config).unwrap();
        let rows = fetch_catalog_entries(
            &client,
            &json!({}),
            ExternalRuntime::Claude,
            "local",
            &cwd,
        )
        .await
        .unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].external_session_id, "good");
        assert_eq!(rows[0].title.as_deref(), Some("original"));
        assert_eq!(rows[0].history_state, "not_loaded");
        let _ = std::fs::remove_dir_all(root);
    }

    #[tokio::test]
    async fn scoped_caller_is_denied_before_provider_launch() {
        let root = std::env::temp_dir()
            .join(format!("acp-catalog-auth-{}", Id::ascending(IdKind::Event)));
        let sibling = root.join("sibling");
        std::fs::create_dir_all(&sibling).unwrap();
        let state = AppState::open_database(root.join("agent.sqlite3"))
            .await
            .unwrap();
        let claims = crate::caller::CallerClaims {
            subject: "remote-user".into(),
            workspace_id: None,
            tenant_id: "other-tenant".into(),
            directory_prefixes: vec![root.to_string_lossy().to_string()],
            hosted: false,
            max_sessions: None,
            max_artifacts: None,
            max_artifact_bytes: None,
            artifact_retention_days: None,
            requests_per_minute: None,
            max_in_flight: None,
            resolved: None,
        };
        let query = |dir: &Path| {
            Query(ExternalCatalogQuery {
                provider: "opencode".into(),
                directory: Some(dir.to_string_lossy().to_string()),
            })
        };
        assert!(external_catalog(
            State(state.clone()),
            query(&root),
            HeaderMap::new(),
            Some(Extension(claims.clone()))
        )
        .await
        .is_err());
        let restricted = crate::caller::CallerClaims {
            tenant_id: "local".into(),
            directory_prefixes: vec![sibling.to_string_lossy().to_string()],
            ..claims
        };
        assert!(external_catalog(
            State(state.clone()),
            query(&root),
            HeaderMap::new(),
            Some(Extension(restricted))
        )
        .await
        .is_err());
        state.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ExternalImportRequest {
    provider: String,
    external_session_id: String,
}

#[derive(Default)]
struct HistoricalReplay {
    turns: Vec<HistoricalTurn>,
    tool_events: usize,
    incomplete_content: bool,
    bytes: usize,
    updates: usize,
}
struct HistoricalTurn {
    role: &'static str,
    source_id: String,
    text: String,
}

impl HistoricalReplay {
    fn push(&mut self, update: Value) -> Result<(), ApiError> {
        self.updates += 1;
        self.bytes += serde_json::to_vec(&update)
            .map_err(|err| ApiError::internal(err.to_string()))?
            .len();
        if self.updates > 10_000 || self.bytes > 4 * 1024 * 1024 {
            return Err(ApiError::bad_request("ACP replay exceeded safety limits"));
        }
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("user_message_chunk" | "agent_message_chunk") => {
                let role = if update["sessionUpdate"] == "user_message_chunk" {
                    "user"
                } else {
                    "assistant"
                };
                let Some(source_id) = update
                    .get("messageId")
                    .and_then(Value::as_str)
                    .filter(|id| !id.is_empty())
                else {
                    // Some adapters synthesize non-conversation status updates
                    // without a durable message ID. Never turn those into a turn.
                    self.incomplete_content = true;
                    return Ok(());
                };
                let Some(text) = update
                    .get("content")
                    .filter(|content| content["type"] == "text")
                    .and_then(|content| content.get("text"))
                    .and_then(Value::as_str)
                else {
                    self.incomplete_content = true;
                    return Ok(());
                };
                if let Some(existing) = self
                    .turns
                    .iter_mut()
                    .find(|turn| turn.source_id == source_id && turn.role == role)
                {
                    existing.text.push_str(text);
                } else {
                    self.turns.push(HistoricalTurn {
                        role,
                        source_id: source_id.to_owned(),
                        text: text.to_owned(),
                    });
                }
            }
            Some("tool_call" | "tool_call_update") => self.tool_events += 1,
            _ => {}
        }
        Ok(())
    }
}

async fn load_native_replay(
    state: &AppState,
    runtime: ExternalRuntime,
    cwd: &str,
    external_id: &str,
) -> Result<HistoricalReplay, ApiError> {
    let (client, mut events) = AcpClient::spawn(
        runtime
            .acp_config(cwd, state.services())
            .map_err(ApiError::bad_request)?,
    )
    .map_err(|err| ApiError::bad_request(format!("ACP launch failed: {err}")))?;
    let initialize = super::acp_run::initialize_acp_client(
        &client,
        runtime,
        Arc::new(AtomicBool::new(false)),
    )
    .await
    .map_err(ApiError::bad_request)?;
    if initialize["agentCapabilities"]["loadSession"] != true {
        return Err(ApiError::bad_request(
            "ACP session/load replay is not advertised; cannot import history",
        ));
    }
    let mut replay = HistoricalReplay::default();
    let request = client.request(
        "session/load",
        json!({"sessionId":external_id,"cwd":cwd,"mcpServers":[]}),
        Duration::from_secs(45),
    );
    tokio::pin!(request);
    loop {
        tokio::select! {
            result = &mut request => {
                let result = result.map_err(|err| ApiError::bad_request(super::acp_run::session_setup_error(runtime, "session/load", &err, &initialize)))?;
                if result.get("sessionId").and_then(Value::as_str).is_some_and(|id| id != external_id) {
                    return Err(ApiError::bad_request("ACP session/load returned a different session ID"));
                }
                break;
            }
            event = events.recv() => match event {
                Some(AcpEvent::SessionUpdate { session_id, update, .. }) if session_id == external_id => replay.push(update)?,
                Some(AcpEvent::Request { id, .. }) => {
                    // Replay must not execute tools or grant permissions before
                    // there is a session and authorized user interaction.
                    let _ = client.respond(id, Err(AcpRpcError {code:-32601, message:"ACP import does not permit tool execution".into()}));
                    return Err(ApiError::bad_request("ACP adapter requested tools during history replay"));
                }
                None => return Err(ApiError::bad_request("ACP adapter closed during history replay")),
                _ => {}
            }
        }
    }
    // Reader enqueues updates before dispatching the load response, so all
    // replay events preceding success are already available without sleeps.
    while let Ok(event) = events.try_recv() {
        match event {
            AcpEvent::SessionUpdate {
                session_id, update, ..
            } if session_id == external_id => replay.push(update)?,
            AcpEvent::Request { id, .. } => {
                let _ = client.respond(
                    id,
                    Err(AcpRpcError {
                        code: -32601,
                        message: "ACP import does not permit tool execution".into(),
                    }),
                );
                return Err(ApiError::bad_request(
                    "ACP adapter requested tools during history replay",
                ));
            }
            _ => {}
        }
    }
    if !replay
        .turns
        .iter()
        .any(|turn| turn.role == "user" && !turn.text.is_empty())
    {
        return Err(ApiError::bad_request("ACP adapter returned no user transcript for this session; preview remains not-loaded"));
    }
    Ok(replay)
}

fn matching_native_root(
    session: &SessionInfo,
    entry: &ExternalCatalogEntry,
    tenant: &str,
) -> bool {
    session.parent_id.is_none()
        && session.directory == entry.cwd
        && crate::caller::session_tenant(session) == tenant
        && session.extra.get("externalAgent").is_some_and(|external| {
            external["provider"] == entry.provider
                && external["externalSessionId"] == entry.external_session_id
        })
}

/// Replay import is enabled only for adapters advertising session/load.
/// Every source ID is reverified against the authorized, exact-cwd catalog.
pub(crate) async fn external_import(
    State(state): State<AppState>,
    Query(query): Query<crate::InstanceQuery>,
    headers: HeaderMap,
    claims: Option<Extension<crate::caller::CallerClaims>>,
    Json(request): Json<ExternalImportRequest>,
) -> Result<Json<SessionInfo>, ApiError> {
    let runtime = ExternalRuntime::resolve(&request.provider)
        .filter(|runtime| runtime.provider_id() == request.provider)
        .ok_or_else(|| {
            ApiError::bad_request("provider must be opencode, codex or claude")
        })?;
    if request.external_session_id.is_empty() {
        return Err(ApiError::bad_request("externalSessionId is required"));
    }
    let catalog = external_catalog(
        State(state.clone()),
        Query(ExternalCatalogQuery {
            provider: request.provider.clone(),
            directory: query.directory.clone(),
        }),
        headers.clone(),
        claims.clone(),
    )
    .await?
    .0;
    if !catalog.import_supported {
        return Err(ApiError::bad_request(
            catalog
                .import_unavailable_reason
                .unwrap_or_else(|| "ACP replay unavailable".into()),
        ));
    }
    let entry = catalog
        .sessions
        .into_iter()
        .find(|entry| entry.external_session_id == request.external_session_id)
        .ok_or_else(|| {
            ApiError::not_found(
                "Provider session not listed in this authorized directory",
            )
        })?;
    let tenant = claims
        .as_ref()
        .map(|Extension(claims)| claims.tenant_id.as_str())
        .unwrap_or("local");
    let _lock = state.inner.external_catalog_import_lock.lock().await;
    if let Some(existing) = state
        .inner
        .store
        .list_sessions()
        .await?
        .into_iter()
        .find(|session| matching_native_root(session, &entry, tenant))
    {
        if existing.extra["externalAgent"]["sourceHost"] != native_host_id() {
            return Err(ApiError::conflict("An ACP root for this provider session belongs to another host or has unverified origin"));
        }
        if existing.extra["externalAgent"]["historyState"] == "text_only"
            && !state
                .inner
                .store
                .list_messages(existing.id.as_str())
                .await?
                .is_empty()
        {
            return Ok(Json(existing));
        }
        return Err(ApiError::conflict("Existing Neoism ACP root has no imported transcript; refusing to merge histories"));
    }
    let replay =
        load_native_replay(&state, runtime, &entry.cwd, &entry.external_session_id)
            .await?;
    if replay
        .turns
        .iter()
        .find(|turn| !turn.text.is_empty())
        .is_none_or(|turn| turn.role != "user")
    {
        return Err(ApiError::bad_request(
            "ACP replay does not begin with a user turn; cannot safely import",
        ));
    }
    if crate::project::discover(state.services(), PathBuf::from(&entry.cwd)).directory
        != entry.cwd
    {
        return Err(ApiError::bad_request("Neoism session root differs from provider cwd; refusing cross-workspace import"));
    }
    let external = json!({
        "provider": runtime.provider_id(), "runtime": "acp", "agent": runtime.agent_name(),
        "status": "imported", "externalSessionId": entry.external_session_id,
        "sourceHost": native_host_id(), "sourceKey": entry.source_key,
        "historyState": "importing", "historyToolEvents": replay.tool_events,
        "historyIncompleteContent": replay.incomplete_content
    });
    let mut info = crate::session_routes::session_create_importing(
        state.clone(),
        crate::InstanceQuery {
            directory: Some(entry.cwd.clone()),
        },
        headers,
        claims,
        neoism_agent_core::CreateSessionRequest {
            parent_id: None,
            title: entry.title.clone(),
            agent: None,
            model: None,
            permission: None,
            workspace_id: None,
            external_provider: Some(runtime.provider_id().into()),
            external_options: None,
        },
        external,
    )
    .await?;
    let mut last_user = None;
    for turn in replay.turns {
        if turn.text.is_empty() {
            continue;
        }
        let message_id = Id::ascending(IdKind::Message);
        let message = if turn.role == "user" {
            last_user = Some(message_id.clone());
            MessageInfo::User(UserMessage {
                id: message_id.clone(),
                session_id: info.id.clone(),
                time: CreatedTime {
                    created: now_millis(),
                },
                agent: runtime.agent_name().into(),
                model: external_model(runtime),
                system: None,
                tools: None,
                author: None,
            })
        } else {
            let Some(parent_id) = &last_user else {
                continue;
            };
            MessageInfo::Assistant(AssistantMessage {
                id: message_id.clone(),
                session_id: info.id.clone(),
                time: CompletedTime {
                    created: now_millis(),
                    streamed: None,
                    completed: Some(now_millis()),
                },
                parent_id: parent_id.clone(),
                mode: runtime.provider_id().into(),
                agent: runtime.agent_name().into(),
                path: AssistantPath {
                    cwd: info.directory.clone(),
                    root: info.directory.clone(),
                },
                cost: 0.0,
                tokens: TokenUsage::default(),
                model_id: runtime.provider_id().into(),
                provider_id: "external".into(),
                finish: None,
                error: None,
            })
        };
        let message = MessageWithParts {
            info: message,
            parts: vec![Part::Text(TextPart {
                id: Id::ascending(IdKind::Part),
                session_id: info.id.clone(),
                message_id,
                text: turn.text,
                synthetic: None,
                time: None,
            })],
        };
        state
            .inner
            .store
            .append_message(info.id.as_str(), &message)
            .await?;
    }
    info.extra.get_mut("externalAgent").unwrap()["historyState"] = json!("text_only");
    state.inner.store.update_session(&info).await?;
    state.publish(EventPayload::new(
        event_type::SESSION_CREATED,
        json!({"sessionID":info.id,"info":info}),
    ));
    state.publish(EventPayload::new(
        event_type::SESSION_UPDATED,
        json!({"sessionID":info.id,"info":info}),
    ));
    Ok(Json(info))
}

#[cfg(all(test, unix))]
mod provider_import_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn opencode_and_codex_replay_are_real_idempotent_and_hidden_while_importing() {
        for provider in ["opencode", "codex"] {
            let root = std::env::temp_dir().join(format!(
                "native-{provider}-{}",
                Id::ascending(IdKind::Event)
            ));
            std::fs::create_dir_all(&root).unwrap();
            let cwd = std::fs::canonicalize(&root).unwrap();
            let init = json!({"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{"list":{}},"loadSession":true}}}).to_string();
            let list = json!({"jsonrpc":"2.0","id":2,"result":{"sessions":[{"sessionId":"native-id","cwd":cwd,"title":"Actual chat"}]}}).to_string();
            let user = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-id","update":{"sessionUpdate":"user_message_chunk","messageId":"u1","content":{"type":"text","text":"actual question"}}}}).to_string();
            let answer = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-id","update":{"sessionUpdate":"agent_message_chunk","messageId":"a1","content":{"type":"text","text":"actual answer"}}}}).to_string();
            let omitted = json!({"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"native-id","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"synthetic status"}}}}).to_string();
            let loaded = json!({"jsonrpc":"2.0","id":2,"result":{}}).to_string();
            let script = root.join("adapter");
            std::fs::write(&script, format!("#!/bin/sh\nread line\nprintf '%s\\n' '{init}'\nread line\ncase \"$line\" in\n  *'\"session/list\"'*) printf '%s\\n' '{list}' ;;\n  *'\"session/load\"'*) printf '%s\\n' '{user}' '{answer}' '{omitted}' '{loaded}' ;;\nesac\nread line\n")).unwrap();
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
                .unwrap();
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
            let state = AppState::open_database_with_services(
                root.join("agent.sqlite3"),
                services,
            )
            .await
            .unwrap();
            let query = || {
                Query(crate::InstanceQuery {
                    directory: Some(cwd.to_string_lossy().into_owned()),
                })
            };
            let request = || {
                Json(ExternalImportRequest {
                    provider: provider.into(),
                    external_session_id: "native-id".into(),
                })
            };
            let catalog = external_catalog(
                State(state.clone()),
                Query(ExternalCatalogQuery {
                    provider: provider.into(),
                    directory: Some(cwd.to_string_lossy().into_owned()),
                }),
                HeaderMap::new(),
                None,
            )
            .await
            .unwrap()
            .0;
            assert!(catalog.import_supported, "{provider}");
            assert_eq!(catalog.sessions[0].history_state, "not_loaded");
            let valid_adapter = std::fs::read_to_string(&script).unwrap();
            std::fs::write(
                &script,
                valid_adapter.replace("\"loadSession\":true", "\"loadSession\":false"),
            )
            .unwrap();
            let unavailable = external_catalog(
                State(state.clone()),
                Query(ExternalCatalogQuery {
                    provider: provider.into(),
                    directory: Some(cwd.to_string_lossy().into_owned()),
                }),
                HeaderMap::new(),
                None,
            )
            .await
            .unwrap()
            .0;
            assert!(!unavailable.import_supported);
            assert!(unavailable
                .import_unavailable_reason
                .as_deref()
                .unwrap()
                .contains("session/load"));
            assert!(external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                request()
            )
            .await
            .is_err());
            assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
            std::fs::write(
                &script,
                valid_adapter.replace(
                    &format!("'{user}' '{answer}' '{omitted}' '{loaded}'"),
                    &format!("'{loaded}'"),
                ),
            )
            .unwrap();
            assert!(external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                request()
            )
            .await
            .is_err());
            assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
            std::fs::write(&script, valid_adapter).unwrap();
            let imported = external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                request(),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(imported.extra["externalAgent"]["historyState"], "text_only");
            assert_eq!(
                imported.extra["externalAgent"]["historyIncompleteContent"],
                true
            );
            let second = external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                request(),
            )
            .await
            .unwrap()
            .0;
            assert_eq!(imported.id, second.id);
            let messages = state
                .inner
                .store
                .list_messages(imported.id.as_str())
                .await
                .unwrap();
            assert_eq!(messages.len(), 2);
            assert!(matches!(messages[0].info, MessageInfo::User(_)));
            assert!(matches!(messages[1].info, MessageInfo::Assistant(_)));
            let mut pending = imported.clone();
            pending.extra.get_mut("externalAgent").unwrap()["historyState"] =
                json!("importing");
            assert!(is_importing(&pending));
            assert!(!is_importing(&imported));
            state.inner.store.update_session(&pending).await.unwrap();
            state.start_session_list_backfill();
            for _ in 0..100 {
                if state.inner.store.session_list_index_ready().await.unwrap() {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            assert!(state.inner.store.session_list_index_ready().await.unwrap());
            let pending_catalog = external_catalog(
                State(state.clone()),
                Query(ExternalCatalogQuery {
                    provider: provider.into(),
                    directory: Some(cwd.to_string_lossy().into_owned()),
                }),
                HeaderMap::new(),
                None,
            )
            .await
            .unwrap()
            .0;
            assert_eq!(pending_catalog.sessions[0].history_state, "importing");
            assert!(pending_catalog.sessions[0].neoism_session_id.is_none());
            for roots in ["true", "false"] {
                let page = crate::v2_routes::v2_session_list(
                    State(state.clone()),
                    Query(
                        serde_json::from_value(json!({"roots":roots,"directory":cwd}))
                            .unwrap(),
                    ),
                    None,
                )
                .await
                .unwrap()
                .0;
                assert!(
                    page.items.is_empty(),
                    "pending root leaked from roots={roots}"
                );
            }
            state.inner.store.update_session(&imported).await.unwrap();
            let ready = crate::v2_routes::v2_session_list(
                State(state.clone()),
                Query(
                    serde_json::from_value(json!({"roots":"true","directory":cwd}))
                        .unwrap(),
                ),
                None,
            )
            .await
            .unwrap()
            .0;
            assert_eq!(ready.items.len(), 1);
            assert_eq!(ready.items[0].extra["externalAgent"]["provider"], provider);
            assert_eq!(
                ready.items[0].extra["externalAgent"]["historyState"],
                "text_only"
            );
            assert!(ready.items[0].extra["externalAgent"]
                .get("externalSessionId")
                .is_none());
            state.shutdown().await.unwrap();
            let _ = std::fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_empty_provider_replay_without_faking_a_turn() {
        let mut replay = HistoricalReplay::default();
        replay.push(json!({"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"status"}})).unwrap();
        assert!(replay.turns.is_empty());
        assert!(replay.incomplete_content);
    }
}

#[cfg(all(test, unix))]
mod import_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[tokio::test]
    async fn claude_import_is_idempotent_and_replays_only_authorized_history() {
        let root = std::env::temp_dir().join(format!(
            "acp-replay-import-{}",
            Id::ascending(IdKind::Event)
        ));
        std::fs::create_dir_all(&root).unwrap();
        let cwd = std::fs::canonicalize(&root).unwrap();
        let list = json!({"jsonrpc":"2.0","id":2,"result":{"sessions":[
            {"sessionId":"provider-opaque-id", "cwd":cwd, "title":"Native session"},
            {"sessionId":"other-workspace", "cwd":std::env::temp_dir(), "title":"hidden"}
        ]}})
        .to_string();
        let init = json!({"jsonrpc":"2.0","id":1,"result":{
            "protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{}}}
        }}).to_string();
        let user = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"provider-opaque-id","update":{
                "sessionUpdate":"user_message_chunk","messageId":"native-user-1","content":{"type":"text","text":"historical question"}
            }
        }}).to_string();
        let assistant = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"provider-opaque-id","update":{
                "sessionUpdate":"agent_message_chunk","messageId":"native-answer-1","content":{"type":"text","text":"real response"}
            }
        }}).to_string();
        let tool = json!({"jsonrpc":"2.0","method":"session/update","params":{
            "sessionId":"provider-opaque-id","update":{"sessionUpdate":"tool_call","toolCallId":"t1"}
        }}).to_string();
        let loaded = json!({"jsonrpc":"2.0","id":2,"result":{}}).to_string();
        let script = root.join("mock-npx");
        let valid_script = format!(
            r#"#!/bin/sh
read line
printf '%s\n' '{init}'
read line
case "$line" in
  *'"session/list"'*) printf '%s\n' '{list}' ;;
  *'"session/load"'*) printf '%s\n' '{user}' '{assistant}' '{tool}' '{loaded}' ;;
esac
read line
"#
        );
        let failed_load = json!({"jsonrpc":"2.0","id":2,"error":{"code":-32000,"message":"session unavailable"}}).to_string();
        let denied_script = valid_script.replace(
            &format!("'{user}' '{assistant}' '{tool}' '{loaded}'"),
            &format!("'{failed_load}'"),
        );
        std::fs::write(&script, denied_script).unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .unwrap();
        let mut services = crate::standard_services();
        services.executables = Arc::new(
            crate::executable::test_support::FakeExecutableService::with(
                "npx",
                script.to_str().unwrap(),
            ),
        );
        let state =
            AppState::open_database_with_services(root.join("agent.sqlite3"), services)
                .await
                .unwrap();
        let request = || ExternalImportRequest {
            provider: "claude".into(),
            external_session_id: "provider-opaque-id".into(),
        };
        let query = || {
            Query(crate::InstanceQuery {
                directory: Some(cwd.to_string_lossy().to_string()),
            })
        };
        assert!(external_import(
            State(state.clone()),
            query(),
            HeaderMap::new(),
            None,
            Json(request())
        )
        .await
        .is_err());
        assert!(state.inner.store.list_sessions().await.unwrap().is_empty());
        std::fs::write(&script, valid_script).unwrap();
        let (first, parallel) = tokio::join!(
            external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                Json(request())
            ),
            external_import(
                State(state.clone()),
                query(),
                HeaderMap::new(),
                None,
                Json(request())
            ),
        );
        let first = first.unwrap().0;
        assert_eq!(first.id, parallel.unwrap().0.id);
        assert!(first.parent_id.is_none());
        assert_eq!(first.extra["externalAgent"]["historyState"], "text_only");
        assert_eq!(first.extra["externalAgent"]["sourceHost"], native_host_id());
        let prompt = || {
            serde_json::from_value(
                json!({"parts": [{"type": "text", "text": "continue"}]}),
            )
            .unwrap()
        };
        let mut interrupted = first.clone();
        interrupted.extra.get_mut("externalAgent").unwrap()["historyState"] =
            json!("importing");
        assert!(super::super::lifecycle::append_external_root_prompt(
            &state,
            &interrupted,
            prompt(),
            true
        )
        .await
        .is_err());
        let mut foreign_host = first.clone();
        foreign_host.extra.get_mut("externalAgent").unwrap()["sourceHost"] =
            json!("another-host");
        assert!(super::super::lifecycle::append_external_root_prompt(
            &state,
            &foreign_host,
            prompt(),
            true
        )
        .await
        .is_err());
        assert_eq!(first.extra["externalAgent"]["historyToolEvents"], 1);
        let catalog = external_catalog(
            State(state.clone()),
            Query(ExternalCatalogQuery {
                provider: "claude".into(),
                directory: Some(cwd.to_string_lossy().to_string()),
            }),
            HeaderMap::new(),
            None,
        )
        .await
        .unwrap()
        .0;
        assert!(catalog.import_supported);
        assert_eq!(catalog.sessions.len(), 1);
        assert_eq!(
            catalog.sessions[0].neoism_session_id.as_deref(),
            Some(first.id.as_str())
        );
        assert_eq!(catalog.sessions[0].history_state, "text_only");
        assert_eq!(
            catalog.sessions[0].source_key,
            first.extra["externalAgent"]["sourceKey"]
        );

        let second = external_import(
            State(state.clone()),
            query(),
            HeaderMap::new(),
            None,
            Json(request()),
        )
        .await
        .unwrap()
        .0;
        assert_eq!(first.id, second.id);
        let messages = state
            .inner
            .store
            .list_messages(first.id.as_str())
            .await
            .unwrap();
        assert_eq!(messages.len(), 2);
        assert!(matches!(messages[0].info, MessageInfo::User(_)));
        assert!(matches!(messages[1].info, MessageInfo::Assistant(_)));
        assert_eq!(
            messages[1]
                .parts
                .iter()
                .filter_map(|part| match part {
                    Part::Text(t) => Some(t.text.as_str()),
                    _ => None,
                })
                .collect::<String>(),
            "real response"
        );
        assert_eq!(state.inner.store.list_sessions().await.unwrap().len(), 1);
        let rejected = external_import(
            State(state.clone()),
            query(),
            HeaderMap::new(),
            None,
            Json(ExternalImportRequest {
                provider: "claude".into(),
                external_session_id: "other-workspace".into(),
            }),
        )
        .await;
        assert!(rejected.is_err());
        state.shutdown().await.unwrap();
        drop(state);
        let reopened = AppState::open_database(root.join("agent.sqlite3"))
            .await
            .unwrap();
        let persisted = reopened
            .inner
            .store
            .get_session(first.id.as_str())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            persisted.extra["externalAgent"]["historyState"],
            "text_only"
        );
        assert_eq!(
            reopened
                .inner
                .store
                .list_messages(first.id.as_str())
                .await
                .unwrap()
                .len(),
            2
        );
        reopened.shutdown().await.unwrap();
        let _ = std::fs::remove_dir_all(root);
    }
}
