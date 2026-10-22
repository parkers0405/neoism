use std::collections::{BTreeMap, HashMap, HashSet};
use std::io::{Read, Write};
use std::net::ToSocketAddrs;
use std::sync::{OnceLock, RwLock};
use std::time::{Duration, Instant};

use base64::Engine as _;
use serde_json::Value;

use crate::neoism::agent::side_panel::{NeoismAgentSessionEntry, SessionGoal};
use crate::neoism::icon::AgentKind;

use super::pane::{
    NeoismAgentMessage, NeoismAgentMessageKind, NeoismAgentOutputKind,
    NeoismAgentPendingPermission, NeoismAgentTodo, NeoismAgentUsage,
};
use super::picker::NeoismAgentPickerOption;

use neoism_ui::panels::agent_pane::api_mapping::{
    config_defaults_from_json, default_model_from_providers_json,
    model_context_limit_from_providers_json, model_options_from_providers_json,
    session_state_from_json, ConfigDefaults, ProviderReadiness, SessionState,
};
use neoism_ui::panels::agent_pane::session_group::{
    group_session_options, SessionOptionInput,
};

const SUBAGENT_TREE_MAX_DEPTH: usize = 5;
const SUBAGENT_TREE_MAX_ROWS: usize = 80;

pub(crate) fn neoism_agent_server() -> String {
    std::env::var("NEOISM_SERVER")
        .unwrap_or_else(|_| "http://127.0.0.1:4096".to_string())
        .trim_end_matches('/')
        .to_string()
}

/// Agent endpoint for a JOINED server — the HOST daemon
/// reverse-proxies its loopback neoism-agent at `/agent` on the SAME
/// endpoint the guest already joined over, so the guest reuses the
/// joined connection and reads the host's chats/threads/SSE:
/// `ws://host:7981/session` → `http://host:7981/agent`. `None` for
/// unix sockets or unparseable endpoints — callers keep the local
/// agent. Shared derivation for both agent-endpoint resolvers (see
/// `ContextManager::agent_server_override_for_current`).
pub(crate) fn agent_reverse_proxy_for_daemon_endpoint(endpoint: &str) -> Option<String> {
    let mut endpoint = url::Url::parse(endpoint.trim()).ok()?;
    if !matches!(endpoint.path(), "" | "/" | "/session" | "/session/") {
        return None;
    }
    let scheme = match endpoint.scheme() {
        "ws" => "http",
        "wss" => "https",
        _ => return None,
    };
    endpoint.set_scheme(scheme).ok()?;
    endpoint.set_path("/agent");
    endpoint.set_query(None);
    endpoint.set_fragment(None);
    Some(endpoint.to_string().trim_end_matches('/').to_string())
}

pub(crate) fn agent_reverse_proxy_for_daemon_workspace(
    endpoint: &str,
    workspace_id: &str,
) -> Option<String> {
    let base = agent_reverse_proxy_for_daemon_endpoint(endpoint)?;
    let workspace_id = workspace_id.trim();
    if workspace_id.is_empty() {
        return None;
    }
    Some(format!(
        "{base}/workspaces/{}",
        percent_encode(workspace_id)
    ))
}

fn agent_server_credentials() -> &'static RwLock<HashMap<String, String>> {
    static CREDENTIALS: OnceLock<RwLock<HashMap<String, String>>> = OnceLock::new();
    CREDENTIALS.get_or_init(|| RwLock::new(HashMap::new()))
}

pub(crate) fn register_agent_server_credential(server: &str, credential: Option<&str>) {
    #[cfg(target_os = "linux")]
    crate::app::agent_tray::register_local_endpoint(
        server,
        credential
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
    );
    let key = server.trim().trim_end_matches('/').to_string();
    let Ok(mut credentials) = agent_server_credentials().write() else {
        return;
    };
    let credential = credential.map(str::trim).filter(|value| !value.is_empty());
    if credentials.get(&key).map(String::as_str) == credential {
        return;
    }
    match credential {
        Some(credential) => {
            credentials.insert(key.clone(), credential.to_string());
        }
        None => {
            credentials.remove(&key);
        }
    }
    // Keep the write guard until the revision changes, so a scope can never
    // observe the new credential paired with the old revision.
    invalidate_catalog_endpoint(&key);
    conversation_cache::credentials_changed();
}

pub(super) fn fetch_external_options_preview(
    server: &str,
    directory: Option<&str>,
    provider: &str,
    selections: &HashMap<String, String>,
) -> Result<neoism_ui::panels::agent_pane::state::external_options::ExternalOptions, String>
{
    let mut path = format!(
        "/v2/external/options/preview?provider={}",
        percent_encode(provider)
    );
    if let Some(directory) = directory.filter(|directory| !directory.is_empty()) {
        path.push_str("&directory=");
        path.push_str(&percent_encode(directory));
    }
    let (method, body) = if selections.is_empty() {
        ("GET", None)
    } else {
        (
            "POST",
            Some(serde_json::json!({"selectedOptions": selections})),
        )
    };
    let response = api_request_json_with_read_timeout(
        server,
        method,
        &path,
        body.as_ref(),
        Duration::from_secs(60),
    )?
    .ok_or_else(|| "Provider returned an empty model catalog".to_string())?;
    neoism_ui::panels::agent_pane::state::external_options::ExternalOptions::parse(
        &response, provider,
    )
}

pub(super) fn fetch_external_options(
    server: &str,
    session_id: &str,
    provider: &str,
) -> Result<neoism_ui::panels::agent_pane::state::external_options::ExternalOptions, String>
{
    let path = format!(
        "/v2/sessions/{}/external/options",
        percent_encode(session_id)
    );
    let response = api_request_json_with_read_timeout(
        server,
        "GET",
        &path,
        None,
        Duration::from_secs(600),
    )?
    .ok_or_else(|| "Provider returned empty session options".to_string())?;
    neoism_ui::panels::agent_pane::state::external_options::ExternalOptions::parse(
        &response, provider,
    )
}

fn external_option_payload(config_id: &str, value: &str) -> Value {
    serde_json::json!({"configId":config_id,"value":value})
}

pub(super) fn set_external_option(
    server: &str,
    session_id: &str,
    provider: &str,
    config_id: &str,
    value: &str,
) -> Result<neoism_ui::panels::agent_pane::state::external_options::ExternalOptions, String>
{
    let path = format!(
        "/v2/sessions/{}/external/options",
        percent_encode(session_id)
    );
    let payload = external_option_payload(config_id, value);
    let response = api_request_json_with_read_timeout(
        server,
        "POST",
        &path,
        Some(&payload),
        Duration::from_secs(600),
    )?
    .ok_or_else(|| "Provider did not confirm option selection".to_string())?;
    neoism_ui::panels::agent_pane::state::external_options::ExternalOptions::parse(
        &response, provider,
    )
}

#[cfg(test)]
fn parse_external_catalog(
    response: &Value,
    source: neoism_ui::panels::agent_pane::state::side_panel::ConversationSource,
) -> Result<Vec<NeoismAgentSessionEntry>, String> {
    use neoism_ui::panels::agent_pane::state::side_panel::ExternalSessionPreview;
    let provider = source
        .provider()
        .ok_or("Neoism does not have a native history adapter")?;
    if response.get("provider").and_then(Value::as_str) != Some(provider) {
        return Err(format!("{} returned an invalid catalog", source.label()));
    }
    let import_supported = response
        .get("importSupported")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let import_unavailable_reason = response
        .get("importUnavailableReason")
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned);
    let sessions = response
        .get("sessions")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("{} returned malformed history rows", source.label()))?;
    let cwd = response.get("cwd").and_then(Value::as_str).ok_or_else(|| {
        format!("{} returned a catalog without a directory", source.label())
    })?;
    Ok(sessions
        .iter()
        .filter_map(|row| {
            if row.get("provider").and_then(Value::as_str) != Some(provider)
                || row.get("cwd").and_then(Value::as_str) != Some(cwd)
            {
                return None;
            }
            let key = row.get("sourceKey")?.as_str()?.trim();
            let external_id = row.get("externalSessionId")?.as_str()?.trim();
            if key.is_empty() || external_id.is_empty() {
                return None;
            }
            let imported_id = row
                .get("neoismSessionId")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned);
            if row.get("historyState").and_then(Value::as_str) == Some("importing") {
                return None;
            }
            let date = row.get("updatedAt").and_then(Value::as_str).unwrap_or("");
            let title = row
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or(source.label());
            let footer = if imported_id.is_some() {
                "Imported".to_string()
            } else if import_supported {
                "Preview · import to read".to_string()
            } else {
                format!(
                    "Preview · {}",
                    import_unavailable_reason
                        .as_deref()
                        .unwrap_or("replay unavailable")
                )
            };
            let footer = if let Some(day) = date.get(..10) {
                format!("{footer} · {day}")
            } else {
                footer
            };
            let mut entry = NeoismAgentSessionEntry::new(
                imported_id
                    .clone()
                    .unwrap_or_else(|| format!("external:{key}")),
                title,
                footer,
            )
            .with_source(source)
            .with_source_key(Some(key.to_owned()))
            .with_updated_ms(external_updated_ms(date));
            entry.external_preview = Some(ExternalSessionPreview {
                source_key: key.to_owned(),
                external_session_id: external_id.to_owned(),
                history_state: row
                    .get("historyState")
                    .and_then(Value::as_str)
                    .unwrap_or("not_loaded")
                    .to_owned(),
                import_supported,
                import_unavailable_reason: import_unavailable_reason.clone(),
                neoism_session_id: imported_id,
            });
            Some(entry)
        })
        .collect())
}

#[cfg(test)]
fn external_import_payload(provider: &str, external_id: &str) -> Value {
    serde_json::json!({"provider":provider, "externalSessionId":external_id})
}

#[cfg(test)]
fn parse_external_import_id(response: &Value, provider: &str) -> Result<String, String> {
    response
        .get("id")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .ok_or_else(|| format!("{provider} import did not return a root session id"))
}

#[cfg(test)]
fn external_updated_ms(raw: &str) -> u64 {
    if let Ok(ms) = raw.parse::<u64>() {
        return ms;
    }
    let Some((date, time)) = raw.split_once('T') else {
        return 0;
    };
    let parse = |part: Option<&str>| part.and_then(|part| part.parse::<i64>().ok());
    let (Some(mut year), Some(month), Some(day), Some(hour), Some(minute), Some(second)) = (
        parse(date.get(..4)),
        parse(date.get(5..7)),
        parse(date.get(8..10)),
        parse(time.get(..2)),
        parse(time.get(3..5)),
        parse(time.get(6..8)),
    ) else {
        return 0;
    };
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return 0;
    }
    let fraction: String = time
        .get(8..)
        .and_then(|rest| rest.strip_prefix('.'))
        .unwrap_or("")
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .take(3)
        .collect();
    let millis = format!("{fraction:0<3}")
        .get(..3)
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or(0);
    let offset = time
        .find(['+', '-'])
        .and_then(|index| time.get(index..))
        .and_then(|zone| {
            let h = parse(zone.get(1..3))?;
            let m = parse(zone.get(4..6))?;
            if h > 23 || m > 59 {
                return None;
            }
            Some(if zone.starts_with('-') {
                -(h * 3600 + m * 60)
            } else {
                h * 3600 + m * 60
            })
        })
        .unwrap_or(0);
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let month = month + if month > 2 { -3 } else { 9 };
    let days =
        era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + (153 * month + 2) / 5 + day
            - 1
            - 719468;
    (days * 86_400 + hour * 3600 + minute * 60 + second - offset)
        .saturating_mul(1000)
        .saturating_add(millis)
        .max(0) as u64
}

#[cfg(test)]
mod external_catalog_tests {
    use super::*;
    use neoism_ui::panels::agent_pane::state::side_panel::ConversationSource;

    #[test]
    fn parses_native_previews_and_imported_ids_without_claiming_replay() {
        let data = serde_json::json!({"provider":"claude","cwd":"/project","importSupported":true,"sessions":[
            {"provider":"claude","cwd":"/project","sourceKey":"claude:key","externalSessionId":"opaque-1","title":"Design","updatedAt":"2026-07-20T12:34:56.123Z","historyState":"not_loaded"},
            {"provider":"claude","cwd":"/project","sourceKey":"claude:key2","externalSessionId":"opaque-2","neoismSessionId":"native-id","historyState":"text_only"},
            {"provider":"claude","cwd":"/project","sourceKey":"pending","externalSessionId":"opaque-4","neoismSessionId":"pending-root","historyState":"importing"},
            {"provider":"claude","cwd":"/foreign","sourceKey":"foreign","externalSessionId":"opaque-3"}
        ]});
        let rows = parse_external_catalog(&data, ConversationSource::ClaudeCode).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].id, "external:claude:key");
        assert_eq!(rows[0].updated_ms, 1_784_550_896_123);
        assert!(rows[0].external_preview.as_ref().unwrap().import_supported);
        assert_eq!(rows[1].id, "native-id");
        assert_eq!(
            rows[1].external_preview.as_ref().unwrap().history_state,
            "text_only"
        );
        assert!(parse_external_catalog(&data, ConversationSource::Codex).is_err());
        let codex = serde_json::json!({"provider":"codex","cwd":"/project","importSupported":true,"sessions":[{"provider":"codex","cwd":"/project","sourceKey":"codex:k","externalSessionId":"a"}]});
        assert!(
            parse_external_catalog(&codex, ConversationSource::Codex).unwrap()[0]
                .external_preview
                .as_ref()
                .unwrap()
                .import_supported
        );
        let unavailable = serde_json::json!({"provider":"opencode","cwd":"/project","importSupported":false,"importUnavailableReason":"Adapter cannot replay this history","sessions":[{"provider":"opencode","cwd":"/project","sourceKey":"opencode:k","externalSessionId":"a"}]});
        let rows =
            parse_external_catalog(&unavailable, ConversationSource::OpenCode).unwrap();
        assert!(!rows[0].external_preview.as_ref().unwrap().import_supported);
        assert!(rows[0]
            .time_label
            .contains("Adapter cannot replay this history"));
    }

    #[test]
    fn import_payload_and_root_id_for_each_provider() {
        for provider in ["opencode", "codex", "claude"] {
            assert_eq!(
                external_import_payload(provider, "opaque"),
                serde_json::json!({"provider":provider,"externalSessionId":"opaque"})
            );
            assert_eq!(
                parse_external_import_id(&serde_json::json!({"id":"root-id"}), provider)
                    .unwrap(),
                "root-id"
            );
            assert!(
                parse_external_import_id(&serde_json::json!({"id":""}), provider)
                    .is_err()
            );
        }
    }

    #[test]
    fn timestamp_timezone_and_missing_date() {
        assert_eq!(external_updated_ms("1970-01-01T01:00:00+01:00"), 0);
        assert_eq!(external_updated_ms("1970-01-01T00:00:01.5Z"), 1500);
        assert_eq!(external_updated_ms("unknown"), 0);
    }
}

fn agent_server_credential(server: &str) -> Option<String> {
    agent_server_credentials()
        .read()
        .ok()?
        .get(server.trim().trim_end_matches('/'))
        .cloned()
}

mod catalog_cache;
#[cfg(test)]
mod catalog_cache_tests;

// The credential is part of identity (never logged); directory is kept exact,
// since None and an explicit workspace can resolve different server defaults.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) struct CatalogScope {
    endpoint: String,
    credential: Option<String>,
    directory: Option<String>,
    revision: u64,
}

fn catalog_revisions() -> &'static std::sync::Mutex<HashMap<String, u64>> {
    static REVISIONS: OnceLock<std::sync::Mutex<HashMap<String, u64>>> = OnceLock::new();
    REVISIONS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}
fn next_catalog_revision() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}
fn endpoint_revision(revisions: &mut HashMap<String, u64>, endpoint: &str) -> u64 {
    if let Some(revision) = revisions.get(endpoint) { return *revision; }
    // Eviction also expires old scopes; globally unique revisions prevent ABA.
    if revisions.len() >= 128 {
        if let Some(oldest) = revisions.iter().min_by_key(|(_, revision)| **revision).map(|(key, _)| key.clone()) {
            revisions.remove(&oldest);
        }
    }
    let revision = next_catalog_revision();
    revisions.insert(endpoint.to_owned(), revision);
    revision
}
pub(super) fn catalog_scope(server: &str, directory: Option<&str>) -> CatalogScope {
    let endpoint = server.trim().trim_end_matches('/').to_owned();
    // Lock order is credential -> revisions, including credential registration.
    let credentials = agent_server_credentials().read().unwrap();
    let credential = credentials.get(&endpoint).cloned();
    let revision = endpoint_revision(&mut catalog_revisions().lock().unwrap(), &endpoint);
    CatalogScope { endpoint, credential, directory: directory.map(str::to_owned), revision }
}
impl std::fmt::Debug for CatalogScope {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.debug_struct("CatalogScope")
            .field("endpoint", &self.endpoint)
            .field("directory", &self.directory)
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}
impl CatalogScope {
    pub(super) fn is_current(&self, server: &str, directory: Option<&str>) -> bool {
        self == &catalog_scope(server, directory)
    }
    fn still_current(&self) -> bool {
        self.is_current(&self.endpoint, self.directory.as_deref())
    }
}
pub(super) fn invalidate_catalog_endpoint(server: &str) {
    let endpoint = server.trim().trim_end_matches('/');
    let mut revisions = catalog_revisions().lock().unwrap();
    endpoint_revision(&mut revisions, endpoint);
    revisions.insert(endpoint.to_owned(), next_catalog_revision());
    // Keep revision and cache removal atomic with respect to scope capture.
    catalog_cache().invalidate(|key| key.0.endpoint == endpoint);
}
type CatalogKey = (CatalogScope, &'static str);
fn catalog_cache() -> &'static catalog_cache::Cache<CatalogKey, Value> {
    static CACHE: OnceLock<catalog_cache::Cache<CatalogKey, Value>> = OnceLock::new();
    CACHE.get_or_init(catalog_cache::Cache::new)
}
fn catalog_key(server: &str, directory: Option<&str>, resource: &'static str) -> CatalogKey {
    (catalog_scope(server, directory), resource)
}
fn catalog_path(resource: &str, directory: Option<&str>) -> String {
    match directory {
        Some(dir) => format!("{resource}?directory={}", percent_encode(dir)),
        None => resource.to_owned(),
    }
}
fn cached_catalog(
    server: &str,
    directory: Option<&str>,
    resource: &'static str,
) -> Option<Value> {
    let scope = catalog_scope(server, directory);
    let value = catalog_cache().snapshot(&(scope.clone(), resource))?;
    scope.still_current().then_some(value)
}
fn fetch_catalog(
    server: &str,
    directory: Option<&str>,
    resource: &'static str,
) -> Result<Value, String> {
    fetch_catalog_scoped(&catalog_scope(server, directory), resource)
}
pub(super) fn fetch_catalog_scoped(scope: &CatalogScope, resource: &'static str) -> Result<Value, String> {
    if !scope.still_current() { return Err("Catalog scope changed".into()); }
    let value = catalog_cache().get((scope.clone(), resource), || {
        if !scope.still_current() { return Err("Catalog scope changed".into()); }
        let server = scope.endpoint.as_str();
        let directory = scope.directory.as_deref();
        // Cold discovery can include a disk-catalog miss and account lookup.
        // It runs off the UI thread; shortening this budget only creates false
        // disconnected states, rather than making a cold provider load faster.
        let timeout = if resource == "/v2/config/defaults" {
            Duration::from_secs(12)
        } else {
            Duration::from_secs(15)
        };
        let value = api_request_json_with_read_timeout(
            server,
            "GET",
            &catalog_path(resource, directory),
            None,
            timeout,
        )?
        .ok_or_else(|| "Neoism Agent returned an empty catalog response".to_string())?;
        // Bound retained payloads as well as the number of scopes.
        if value.to_string().len() > 8 * 1024 * 1024 {
            return Err("Agent catalog exceeds cache size limit".into());
        }
        if resource == "/v2/agents" && !value.is_array() {
            return Err("Neoism Agent returned malformed agents".into());
        }
        if resource.starts_with("/v2/providers")
            && !value
                .get("providers")
                .or_else(|| value.get("all"))
                .is_some_and(Value::is_array)
        {
            return Err("Neoism Agent returned malformed providers".into());
        }
        if resource == "/v2/providers" && ProviderReadiness::from_json(&value).is_none() {
            return Err("Neoism Agent returned malformed provider readiness".into());
        }
        if !scope.still_current() { return Err("Catalog scope changed".into()); }
        Ok(value)
    })?;
    if !scope.still_current() { return Err("Catalog scope changed".into()); }
    Ok(value)
}
#[cfg(test)]
pub(super) fn seed_picker_catalog_for_test(server: &str, directory: Option<&str>) {
    let providers = serde_json::json!({"providers": [{"id": "test", "models": {"model": {"id": "model", "name": "Test model", "limit": {"context": 12345}}}}], "connected": ["test"]});
    for (resource, value) in [
        ("/v2/providers/configured", providers.clone()),
        ("/v2/providers", providers),
        ("/v2/agents", serde_json::json!([{"name": "build"}])),
    ] {
        catalog_cache()
            .get(catalog_key(server, directory, resource), || Ok(value))
            .unwrap();
    }
}

pub(super) fn cached_model_options(
    server: &str,
    directory: Option<&str>,
) -> Option<(Vec<NeoismAgentPickerOption>, Option<ProviderReadiness>)> {
    let value = cached_catalog(server, directory, "/v2/providers/configured")?;
    Some((
        model_options_from_providers_json(&value),
        cached_provider_readiness(server, directory),
    ))
}
fn cached_provider_readiness(
    server: &str,
    directory: Option<&str>,
) -> Option<ProviderReadiness> {
    cached_catalog(server, directory, "/v2/providers")
        .as_ref()
        .and_then(ProviderReadiness::from_json)
}
pub(super) fn cached_agent_options(
    server: &str,
    directory: Option<&str>,
) -> Option<Vec<NeoismAgentPickerOption>> {
    agent_options_from_json(&cached_catalog(server, directory, "/v2/agents")?).ok()
}

pub(super) fn fetch_model_options_scoped(scope: &CatalogScope) -> Result<Vec<NeoismAgentPickerOption>, String> {
    Ok(model_options_from_providers_json(&fetch_catalog_scoped(scope, "/v2/providers/configured")?))
}
pub(super) fn fetch_agent_options_scoped(scope: &CatalogScope) -> Result<Vec<NeoismAgentPickerOption>, String> {
    agent_options_from_json(&fetch_catalog_scoped(scope, "/v2/agents")?)
}
pub(super) fn fetch_provider_readiness_scoped(scope: &CatalogScope) -> Result<ProviderReadiness, String> {
    let value = fetch_catalog_scoped(scope, "/v2/providers")?;
    ProviderReadiness::from_json(&value).ok_or_else(|| "Neoism Agent returned malformed provider readiness".into())
}

pub(super) fn fetch_model_context_limit(
    server: &str,
    directory: Option<&str>,
    model_ref: &str,
) -> Result<Option<u64>, String> {
    // The provider catalog is assembled lazily and can exceed the normal
    // 900 ms UI-request timeout on a cold agent-server start. A timeout here
    // silently left the usage panel without its `/ context limit` denominator.
    let value = fetch_catalog(server, directory, "/v2/providers/configured")?;
    Ok(model_context_limit_from_providers_json(&value, model_ref))
}

fn agent_options_from_json(
    value: &Value,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let agents = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed agents".to_string())?;

    let out = agents
        .iter()
        .filter(|agent| {
            !agent
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        // Subagent-only definitions (mode == "subagent", e.g.
        // explore/general) are Task-tool targets, not top-level agents.
        .filter(|agent| {
            agent
                .get("mode")
                .and_then(Value::as_str)
                .is_none_or(|mode| mode != "subagent")
        })
        .filter_map(|agent| {
            let name = agent.get("name").and_then(Value::as_str)?.to_string();
            let description = agent
                .get("description")
                .and_then(Value::as_str)
                .or_else(|| agent.get("mode").and_then(Value::as_str))
                .unwrap_or("agent")
                .to_string();
            Some(NeoismAgentPickerOption::new(
                &name,
                &description,
                "agent",
                &name,
            ))
        })
        .collect();
    Ok(out)
}

pub(super) fn fetch_directory_options(
    server: &str,
    session_id: &str,
    current_directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let value = api_request_json_with_read_timeout(
        server,
        "GET",
        &format!("/v2/sessions/{session_id}/directory-options?limit=512"),
        None,
        Duration::from_secs(20),
    )?
    .ok_or_else(|| "Neoism Agent returned an empty directory response".to_string())?;
    let directories = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed directories".to_string())?;
    Ok(directories
        .iter()
        .filter_map(Value::as_str)
        .map(|path| {
            let label = compact_picker_directory(path);
            let mut option = NeoismAgentPickerOption::new(
                &label,
                path,
                if current_directory == Some(path) {
                    "current"
                } else {
                    "directory"
                },
                path,
            );
            option.is_current = current_directory == Some(path);
            option
        })
        .collect())
}

fn compact_picker_directory(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let normalized = std::env::var("HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .and_then(|home| {
            let home = home.replace('\\', "/");
            if normalized == home {
                Some("~".to_string())
            } else {
                normalized
                    .strip_prefix(&format!("{home}/"))
                    .map(|rest| format!("~/{rest}"))
            }
        })
        .unwrap_or(normalized);
    if normalized.ends_with('/') {
        normalized
    } else {
        format!("{normalized}/")
    }
}

pub(super) fn fetch_skill_options(
    server: &str,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let path = directory
        .map(|dir| format!("/v2/skills?directory={}", percent_encode(dir)))
        .unwrap_or_else(|| "/v2/skills".to_string());
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty skill response".to_string())?;
    let skills = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed skills".to_string())?;
    Ok(skills
        .iter()
        .filter_map(|skill| {
            let name = skill.get("name").and_then(Value::as_str)?.to_string();
            let description = skill
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("SKILL.md")
                .to_string();
            let footer = skill
                .get("path")
                .and_then(Value::as_str)
                .map(short_skill_path)
                .unwrap_or_else(|| "skill".to_string());
            Some(NeoismAgentPickerOption::new(
                &name,
                &description,
                &footer,
                &name,
            ))
        })
        .collect())
}

/// Fetch the recent sessions for `directory`, newest-first, capped to 24.
struct SessionValuesPage {
    items: Vec<Value>,
    next_cursor: Option<String>,
}

fn fetch_sessions_sorted(
    server: &str,
    directory: Option<&str>,
) -> Result<SessionValuesPage, String> {
    fetch_sessions_page(server, directory, None)
}

fn fetch_sessions_page(
    server: &str,
    directory: Option<&str>,
    cursor: Option<&str>,
) -> Result<SessionValuesPage, String> {
    fetch_sessions_query_page(server, directory, cursor, None)
}

fn session_search_path(
    directory: Option<&str>,
    cursor: Option<&str>,
    search: Option<&str>,
) -> String {
    let limit = if search.is_some() { 50 } else { 24 };
    let mut path = format!("/v2/sessions?roots=true&limit={limit}");
    if let Some(directory) = directory.filter(|directory| !directory.trim().is_empty()) {
        path.push_str("&directory=");
        path.push_str(&percent_encode(directory));
    }
    if let Some(search) = search {
        path.push_str("&search=");
        path.push_str(&percent_encode(search));
    }
    if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        path.push_str("&cursor=");
        path.push_str(&percent_encode(cursor));
    }
    path
}

fn fetch_sessions_query_page(
    server: &str,
    directory: Option<&str>,
    cursor: Option<&str>,
    search: Option<&str>,
) -> Result<SessionValuesPage, String> {
    let path = session_search_path(directory, cursor, search);
    fetch_sessions_path_page(server, &path)
}

fn fetch_sessions_path_page(
    server: &str,
    path: &str,
) -> Result<SessionValuesPage, String> {
    let value = api_request_json(server, "GET", path, None)?
        .ok_or_else(|| "Neoism Agent returned an empty session response".to_string())?;
    let sessions = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "Neoism Agent returned malformed sessions".to_string())?;
    let next_cursor = value
        .get("cursor")
        .and_then(|cursor| cursor.get("next"))
        .and_then(Value::as_str)
        .map(str::to_string);
    Ok(SessionValuesPage {
        items: sessions.to_vec(),
        next_cursor,
    })
}

pub(super) fn fetch_session_options(
    server: &str,
    current_id: Option<&str>,
    directory: Option<&str>,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let sessions = fetch_sessions_sorted(server, directory)?.items;
    let inputs = sessions
        .iter()
        .filter_map(|session| session_option_input(session, current_id))
        .collect::<Vec<_>>();
    Ok(group_session_options(inputs))
}

fn project_session_path(directory: &str, cursor: Option<&str>) -> String {
    let mut path = format!("/v2/sessions?roots=true&limit=5&exact_directory=true&exclude_pinned=true&directory={}", percent_encode(directory));
    if let Some(cursor) = cursor {
        path.push_str("&cursor=");
        path.push_str(&percent_encode(cursor));
    }
    path
}

pub(crate) struct SessionProjects {
    pub projects: Vec<(String, u64)>,
    pub pinned_sessions: Vec<NeoismAgentSessionEntry>,
}

pub(crate) fn fetch_session_projects(server: &str) -> Result<SessionProjects, String> {
    let value = api_request_json(server, "GET", "/v2/session-projects", None)?
        .ok_or_else(|| "Neoism Agent returned an empty projects response".to_string())?;
    decode_session_projects(&value)
}

fn decode_session_projects(value: &Value) -> Result<SessionProjects, String> {
    let items = value
        .get("items")
        .and_then(Value::as_array)
        .ok_or_else(|| "Neoism Agent returned malformed projects".to_string())?;
    let projects = items
        .iter()
        .map(|item| {
            let directory = item
                .get("directory")
                .and_then(Value::as_str)
                .ok_or_else(|| "project missing directory".to_string())?;
            let updated = item
                .get("updatedMs")
                .and_then(Value::as_u64)
                .ok_or_else(|| "project missing updatedMs".to_string())?;
            Ok((directory.to_owned(), updated))
        })
        .collect::<Result<Vec<_>, String>>()?;
    let statuses = HashMap::new();
    let pinned_values = value
        .get("pinnedSessions")
        .map(|pins| {
            pins.as_array().ok_or_else(|| {
                "Neoism Agent returned malformed pinned sessions".to_string()
            })
        })
        .transpose()?;
    let pinned_sessions = pinned_values
        .into_iter()
        .flatten()
        .filter_map(|session| session_entry(session, &statuses))
        .collect();
    Ok(SessionProjects {
        projects,
        pinned_sessions,
    })
}

#[cfg(test)]
mod discovery_pin_tests {
    use super::*;

    #[test]
    fn discovery_decodes_old_pins_with_existing_session_decoder() {
        let pin = serde_json::json!({"id":"old-pin", "title":"Old pin", "directory":"/archived",
            "time":{"updated":1}, "pinned":true, "catalogActivity":"permission",
            "externalAgent":{"provider":"codex", "sourceKey":"stable-key", "historyState":"neoism_only"}});
        let decoded = decode_session_projects(&serde_json::json!({
            "items":[{"directory":"/active", "updatedMs":100}], "pinnedSessions":[pin.clone()]
        })).unwrap();
        assert_eq!(decoded.projects, vec![("/active".into(), 100)]);
        let entry = &decoded.pinned_sessions[0];
        let expected = session_entry(&pin, &HashMap::new()).unwrap();
        assert_eq!(entry.id, expected.id);
        assert_eq!(entry.directory.as_deref(), Some("/archived"));
        assert_eq!(entry.source, expected.source);
        assert_eq!(entry.source_key.as_deref(), Some("stable-key"));
        assert_eq!(entry.catalog_activity, expected.catalog_activity);
        assert!(entry.pinned);
        assert!(decode_session_projects(&serde_json::json!({"items":[]}))
            .unwrap()
            .pinned_sessions
            .is_empty());
    }
}

pub(crate) fn fetch_project_session_page(
    server: &str,
    directory: &str,
    cursor: Option<&str>,
) -> Result<(Vec<NeoismAgentSessionEntry>, Option<String>), String> {
    let page =
        fetch_sessions_path_page(server, &project_session_path(directory, cursor))?;
    let statuses = HashMap::new();
    let entries = page
        .items
        .iter()
        .filter_map(|session| session_entry(session, &statuses))
        .collect();
    Ok((entries, page.next_cursor))
}

pub(crate) fn fetch_session_entries_page(
    server: &str,
    current_id: Option<&str>,
    directory: Option<&str>,
    cursor: Option<&str>,
) -> Result<(Vec<NeoismAgentSessionEntry>, Option<String>), String> {
    let page = fetch_sessions_page(server, directory, cursor)?;
    // Runtime dots hydrate independently from live session events. Do not
    // hold the catalogue behind the serial status endpoint.
    let statuses = HashMap::new();
    let _ = current_id;
    let entries = page
        .items
        .iter()
        .filter_map(|session| session_entry(session, &statuses))
        .collect();
    Ok((entries, page.next_cursor))
}

pub(super) fn fetch_subagent_options(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentPickerOption>, String> {
    let current =
        api_request_json(server, "GET", &format!("/v2/sessions/{session_id}"), None)?
            .ok_or_else(|| {
                "Neoism Agent returned an empty session response".to_string()
            })?;
    let parent_id = current
        .get("parentId")
        .or_else(|| current.get("parentID"))
        .and_then(Value::as_str)
        .unwrap_or(session_id)
        .to_string();
    let children = api_request_json(
        server,
        "GET",
        &format!("/v2/sessions/{parent_id}/children"),
        None,
    )?
    .ok_or_else(|| "Neoism Agent returned an empty child session response".to_string())?;
    let children = children
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed child sessions".to_string())?;

    let mut out = vec![NeoismAgentPickerOption::new(
        "main session",
        &parent_id,
        "return",
        &parent_id,
    )];
    let mut children = children.iter().collect::<Vec<_>>();
    children.sort_by(|a, b| session_updated_at(b).cmp(&session_updated_at(a)));
    out.extend(children.into_iter().filter_map(|session| {
        let id = session.get("id").and_then(Value::as_str)?.to_string();
        let title = session
            .get("title")
            .and_then(Value::as_str)
            .unwrap_or("subagent")
            .to_string();
        let footer = session
            .get("agent")
            .and_then(Value::as_str)
            .unwrap_or("agent")
            .to_string();
        Some(NeoismAgentPickerOption::new(&title, &id, &footer, &id))
    }));
    Ok(out)
}

pub(super) fn fetch_subagent_entries(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentSessionEntry>, String> {
    let current =
        api_request_json(server, "GET", &format!("/v2/sessions/{session_id}"), None)?
            .ok_or_else(|| {
                "Neoism Agent returned an empty session response".to_string()
            })?;
    let root = fetch_root_session(server, current)?;
    let root_id = root
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "Neoism Agent returned a root session without an id".to_string())?
        .to_string();

    let mut entries = vec![NeoismAgentSessionEntry::new(
        root_id.clone(),
        "main session",
        "return",
    )];
    let mut visited = HashSet::new();
    visited.insert(root_id.clone());
    // Discover the tree first and take the runtime snapshot LAST. Taking
    // `/session/status` before `/children` created a split snapshot: a child
    // launched between those requests appeared in the tree but not the older
    // status map, so we mislabeled it completed and terminal-locked the live
    // row. The final status read is a barrier: every child returned above
    // existed before this authoritative runtime snapshot was captured.
    collect_subagent_entries(
        server,
        &root_id,
        &HashMap::new(),
        1,
        &mut entries,
        &mut visited,
    )?;
    // Catalog navigation is scoped to the owning project, not a child's
    // execution/worktree directory. Carry the root identity on every row so
    // dashboard activation cannot fall back to the currently focused project.
    let family_directory = root
        .get("directory")
        .and_then(Value::as_str)
        .map(str::to_owned);
    for entry in &mut entries {
        entry.directory = family_directory.clone();
    }
    // A missing status map must not turn every live child into a terminal
    // branch for one frame. Let the pane retain its last good tree and retry.
    let statuses = fetch_session_statuses(server)?;
    apply_subagent_runtime_snapshot(&mut entries, &statuses);
    Ok(entries)
}

fn apply_subagent_runtime_snapshot(
    entries: &mut [NeoismAgentSessionEntry],
    statuses: &HashMap<String, SessionStatusSnapshot>,
) {
    for entry in entries.iter_mut().skip(1) {
        entry.runtime_status = Some(
            statuses
                .get(&entry.id)
                .and_then(|status| normalize_explicit_runtime_status(&status.kind))
                .or_else(|| {
                    entry.runtime_status.as_deref().filter(|status| {
                        matches!(*status, "completed" | "error" | "interrupted")
                    })
                })
                .unwrap_or("interrupted")
                .to_string(),
        );
    }
}

fn fetch_root_session(server: &str, mut session: Value) -> Result<Value, String> {
    let mut visited = HashSet::new();
    for _ in 0..SUBAGENT_TREE_MAX_DEPTH {
        let Some(parent_id) = session_parent_id(&session) else {
            break;
        };
        if !visited.insert(parent_id.clone()) {
            break;
        }
        let parent =
            api_request_json(server, "GET", &format!("/v2/sessions/{parent_id}"), None);
        let Ok(Some(parent)) = parent else {
            break;
        };
        session = parent;
    }
    Ok(session)
}

fn collect_subagent_entries(
    server: &str,
    parent_id: &str,
    statuses: &HashMap<String, SessionStatusSnapshot>,
    depth: usize,
    entries: &mut Vec<NeoismAgentSessionEntry>,
    visited: &mut HashSet<String>,
) -> Result<(), String> {
    if depth > SUBAGENT_TREE_MAX_DEPTH || entries.len() >= SUBAGENT_TREE_MAX_ROWS {
        return Ok(());
    }
    let children = api_request_json(
        server,
        "GET",
        &format!("/v2/sessions/{parent_id}/children"),
        None,
    )?
    .ok_or_else(|| "Neoism Agent returned an empty child session response".to_string())?;
    let children = children
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed child sessions".to_string())?;

    let mut children = children.iter().collect::<Vec<_>>();
    children.sort_by(|a, b| session_updated_at(b).cmp(&session_updated_at(a)));
    for child in children {
        if entries.len() >= SUBAGENT_TREE_MAX_ROWS {
            break;
        }
        let Some(id) = child.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !visited.insert(id.to_string()) {
            continue;
        }
        if let Some(entry) = subagent_entry_from_session(child, statuses, depth) {
            entries.push(entry);
        }
        collect_subagent_entries(server, id, statuses, depth + 1, entries, visited)?;
    }
    Ok(())
}

fn subagent_entry_from_session(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
    depth: usize,
) -> Option<NeoismAgentSessionEntry> {
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("subagent")
        .to_string();
    let footer = session
        .get("externalAgent")
        .and_then(external_agent_label)
        .or_else(|| {
            session
                .get("agent")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .filter(|label| !label.trim().is_empty())
        .unwrap_or_else(|| "subagent".to_string());
    let agent_kind =
        session_external_agent_kind(session).or_else(|| AgentKind::from_label(&footer));
    Some(
        NeoismAgentSessionEntry::new(id, title, footer)
            .with_depth(depth)
            .with_agent_kind(agent_kind)
            .with_runtime_status(session_explicit_runtime_status(session, statuses)),
    )
}

fn session_parent_id(session: &Value) -> Option<String> {
    session
        .get("parentId")
        .or_else(|| session.get("parentID"))
        .and_then(Value::as_str)
        .map(str::to_string)
        .filter(|id| !id.is_empty())
}

fn external_agent_label(external: &Value) -> Option<String> {
    external
        .get("agent")
        .or_else(|| external.get("provider"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn session_external_agent_kind(session: &Value) -> Option<AgentKind> {
    let external = session.get("externalAgent")?;
    external
        .get("provider")
        .or_else(|| external.get("agent"))
        .and_then(Value::as_str)
        .and_then(AgentKind::from_label)
}

fn session_explicit_runtime_status(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
) -> Option<String> {
    let id = session.get("id").and_then(Value::as_str);
    if let Some(status) = id
        .and_then(|id| statuses.get(id))
        .and_then(|status| normalize_explicit_runtime_status(&status.kind))
    {
        return Some(status.to_string());
    }

    // Persisted session JSON is useful terminal history, never live authority.
    // If the live status endpoint omits a child, stale "running" metadata must
    // not resurrect it during tree hydration.
    session
        .get("externalAgent")
        .and_then(|external| external.get("status"))
        .and_then(Value::as_str)
        .or_else(|| session.get("status").and_then(Value::as_str))
        .and_then(normalize_explicit_runtime_status)
        .filter(|status| matches!(*status, "completed" | "error" | "interrupted"))
        .map(str::to_string)
}

fn normalize_explicit_runtime_status(status: &str) -> Option<&'static str> {
    match status.trim().to_ascii_lowercase().as_str() {
        "created" | "active" | "busy" | "running" => Some("running"),
        "blocked" | "retry" | "waiting_permission" | "waiting-permission" => {
            Some("blocked")
        }
        "completed" | "complete" | "idle" | "done" => Some("completed"),
        "cancelled" | "canceled" | "interrupted" => Some("interrupted"),
        "failed" | "error" | "errored" | "stopped" | "aborted" => Some("error"),
        _ => None,
    }
}

#[cfg(test)]
mod subagent_runtime_snapshot_tests {
    use super::*;

    #[test]
    fn project_pages_use_five_roots_and_encode_directory_and_cursor() {
        assert_eq!(project_session_path("/a project", None),
            "/v2/sessions?roots=true&limit=5&exact_directory=true&exclude_pinned=true&directory=%2Fa%20project");
        assert_eq!(project_session_path("/a project", Some("next:+/=")),
            "/v2/sessions?roots=true&limit=5&exact_directory=true&exclude_pinned=true&directory=%2Fa%20project&cursor=next%3A%2B%2F%3D");
        // Scoped flat paging and global search keep their existing limits.
        assert_eq!(
            session_search_path(Some("/project"), None, None),
            "/v2/sessions?roots=true&limit=24&directory=%2Fproject"
        );
        assert_eq!(
            session_search_path(None, None, Some("query")),
            "/v2/sessions?roots=true&limit=50&search=query"
        );
    }

    #[test]
    fn server_title_search_is_filtered_paginated_and_retains_project_metadata() {
        assert_eq!(
            session_search_path(None, None, Some("older chat")),
            "/v2/sessions?roots=true&limit=50&search=older%20chat"
        );
        assert_eq!(session_search_path(Some("/allowed/project"), Some("next:page"), Some("older chat")),
            "/v2/sessions?roots=true&limit=50&directory=%2Fallowed%2Fproject&search=older%20chat&cursor=next%3Apage");
        let entry = session_entry(
            &serde_json::json!({
                "id":"older", "title":"Older chat", "directory":"/allowed/old-project",
                "externalAgent":{"historyState":"neoism_only", "provider":"codex"}
            }),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(entry.id, "older");
        assert_eq!(entry.title, "Older chat");
        assert_eq!(entry.directory.as_deref(), Some("/allowed/old-project"));
    }

    #[test]
    fn embedded_search_session_never_fetches_and_preserves_authorized_metadata() {
        let hit = serde_json::json!({"sessionId":"older", "session": {
            "id":"older", "title":"Older chat", "directory":"/remote/project",
            "externalAgent":{"historyState":"neoism_only", "provider":"codex"}
        }});
        let entry = semantic_hit_session_entry(&hit, "older", Some("/remote/project"), || {
            panic!("embedded authorized session must not trigger HTTP")
        }).unwrap();
        assert_eq!(entry.id, "older");
        assert_eq!(entry.title, "Older chat");
        assert_eq!(entry.directory.as_deref(), Some("/remote/project"));
        let expected = session_entry(&hit["session"], &HashMap::new()).unwrap();
        assert_eq!(entry.source, expected.source);
        for (id, directory) in [("other", None), ("older", Some("/different"))] {
            assert!(semantic_hit_session_entry(&hit, id, directory, || {
                panic!("invalid embedded metadata must not trigger HTTP")
            }).is_none());
        }
        for session in [
            serde_json::json!({"id":"older", "parentID":"parent"}),
            serde_json::json!({"id":"older", "parentId":"parent"}),
            serde_json::json!("malformed"),
        ] {
            assert!(semantic_hit_session_entry(
                &serde_json::json!({"session":session}),
                "older",
                None,
                || { panic!("present embedded metadata must not trigger HTTP") }
            )
            .is_none());
        }
    }

    #[test]
    fn legacy_search_hit_fetches_session_detail_only_when_not_embedded() {
        for hit in [serde_json::json!({"sessionId":"older"}), serde_json::json!({"session":null})] {
            let mut calls = 0;
            let entry = semantic_hit_session_entry(&hit, "older", None, || {
                calls += 1;
                Some(serde_json::json!({"id":"older", "title":"Legacy", "directory":"/remote"}))
            }).unwrap();
            assert_eq!(calls, 1);
            assert_eq!(entry.title, "Legacy");
        }
        assert!(semantic_hit_session_entry(
            &serde_json::json!({}),
            "older",
            None,
            || None
        )
        .is_none());
    }

    #[test]
    fn catalog_all_scope_and_session_directory_are_independent() {
        assert_eq!(
            session_catalog_events_path(None),
            "/v2/session-catalog/events?scope=all"
        );
        assert_eq!(
            session_catalog_events_path(Some("/tmp/my project")),
            "/v2/session-catalog/events?directory=%2Ftmp%2Fmy%20project"
        );
        let entry = session_entry(
            &serde_json::json!({
                "id": "root", "title": "Other project", "directory": "/tmp/other-project"
            }),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(entry.directory.as_deref(), Some("/tmp/other-project"));
    }

    #[test]
    fn optional_registry_uses_http_status_and_preserves_proxy_prefix() {
        use std::io::{Read, Write};
        use std::net::TcpListener;
        for (status, register) in [
            (404, false),
            (200, false),
            (401, false),
            (403, false),
            (503, false),
            (404, true),
        ] {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let address = listener.local_addr().unwrap();
            let worker = std::thread::spawn(move || {
                let (mut stream, _) = listener.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let count = stream.read(&mut chunk).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&chunk[..count]);
                    if let Some(end) = find_header_end(&request) {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("Content-Length:")
                                    .map(|value| value.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                let body = if status == 200 {
                    "[]"
                } else {
                    "404 mentioned in body is not a status classifier"
                };
                write!(stream, "HTTP/1.1 {status} Mock\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
                String::from_utf8(request)
                    .unwrap()
                    .lines()
                    .next()
                    .unwrap()
                    .to_owned()
            });
            let result = conversation_workspace_registry_request(
                &format!("http://{address}/agent/workspaces/authorized"),
                register.then(|| std::path::Path::new("/chosen")),
            );
            let method = if register { "POST" } else { "GET" };
            assert_eq!(worker.join().unwrap(), format!("{method} /agent/workspaces/authorized/v2/management/workspaces HTTP/1.1"));
            if status == 404 && !register {
                assert_eq!(result.unwrap(), None);
            } else if status == 200 {
                assert_eq!(result.unwrap(), Some(serde_json::json!([])));
            } else {
                assert!(result.unwrap_err().contains(&format!("HTTP {status}")));
            }
        }
    }

    #[test]
    fn search_budget_errors_keep_the_actionable_message_visible() {
        for (status, code, message) in [
            (422, "search.deadline", "Conversation search exceeded its query budget; refine the query"),
            (422, "search.too_broad", "Conversation search is too broad; refine the query"),
            (429, "search.busy", "Conversation search is busy; retry shortly"),
        ] {
            let body = serde_json::json!({"code": code, "message": message}).to_string();
            assert_eq!(http_error(status, "Error", &body), message);
            assert!(http_error(500, "Internal Server Error", &body).starts_with("Neoism Agent HTTP 500"));
        }
        assert!(http_error(422, "Error", r#"{"code":"search.deadline"}"#)
            .starts_with("Neoism Agent HTTP 422"));
    }

    #[test]
    fn http_error_uses_server_message_without_dumping_json() {
        let body = r#"{"code":"request.invalid","details":{},"message":"Codex ACP authentication required for session/new: Authentication required. Sign in with the provider's CLI, then retry.","retryable":false}"#;
        let error = http_error(400, "Bad Request", body);
        assert!(error.contains("Codex ACP authentication required"));
        assert!(error.contains("Sign in with the provider's CLI"));
        assert!(!error.contains("\"details\""));
        assert_eq!(
            http_error(409, "Conflict", "busy"),
            "Neoism Agent HTTP 409 Conflict: busy"
        );
    }

    #[test]
    fn persisted_running_status_is_not_live_authority() {
        let session = serde_json::json!({
            "id": "child-1",
            "externalAgent": { "status": "running" }
        });
        assert_eq!(
            session_explicit_runtime_status(&session, &HashMap::new()),
            None
        );

        let mut statuses = HashMap::new();
        statuses.insert(
            "child-1".to_string(),
            SessionStatusSnapshot {
                kind: "busy".to_string(),
                ..Default::default()
            },
        );
        assert_eq!(
            session_explicit_runtime_status(&session, &statuses).as_deref(),
            Some("running")
        );
    }

    #[test]
    fn persisted_terminal_status_remains_historical_evidence() {
        let session = serde_json::json!({ "id": "child-1", "status": "completed" });
        assert_eq!(
            session_explicit_runtime_status(&session, &HashMap::new()).as_deref(),
            Some("completed")
        );
    }

    #[test]
    fn sidebar_lists_neoism_acp_roots_but_not_provider_history_imports() {
        let imported = serde_json::json!({
            "id": "imported",
            "title": "Old Codex history",
            "extra": {"externalAgent": {"provider": "codex", "historyState": "text_only"}}
        });
        let provider_preview = serde_json::json!({
            "id": "preview",
            "title": "Unclaimed Codex history",
            "extra": {"externalAgent": {"provider": "codex", "historyState": "not_loaded"}}
        });
        let unmarked_external = serde_json::json!({
            "id": "unmarked",
            "title": "Legacy external history",
            "extra": {"externalAgent": {"provider": "codex"}}
        });
        let native = serde_json::json!({
            "id": "native",
            "title": "Neoism Codex chat",
            "extra": {"externalAgent": {"provider": "codex", "historyState": "neoism_only"}}
        });
        let internal = serde_json::json!({
            "id": "internal",
            "title": "Neoism chat"
        });

        assert!(session_entry(&imported, &HashMap::new()).is_none());
        assert!(session_entry(&provider_preview, &HashMap::new()).is_none());
        assert!(session_entry(&unmarked_external, &HashMap::new()).is_none());
        assert_eq!(
            session_entry(&native, &HashMap::new()).map(|entry| entry.id),
            Some("native".into())
        );
        assert_eq!(
            session_entry(&internal, &HashMap::new()).map(|entry| entry.id),
            Some("internal".into())
        );
    }

    #[test]
    fn pending_permission_snapshot_uses_live_event_shape() {
        let permissions = parse_pending_permissions(&serde_json::json!([{
            "id": "perm-1",
            "sessionId": "child-1",
            "messageId": "msg-1",
            "title": "Read outside workspace",
            "permission": "external_directory",
            "patterns": ["/tmp/shared/**"]
        }]))
        .unwrap();

        assert_eq!(permissions.len(), 1);
        assert_eq!(permissions[0].id, "perm-1");
        assert_eq!(permissions[0].session_id, "child-1");
        assert_eq!(permissions[0].permission, "external_directory");
        assert_eq!(permissions[0].patterns, ["/tmp/shared/**"]);
    }

    #[test]
    fn desktop_agent_api_source_contains_no_deleted_http_routes() {
        let source = include_str!("api.rs");
        for route in ["config", "agent", "skill"] {
            let legacy = format!("format!(\"/{route}");
            assert!(
                !source.contains(&legacy),
                "legacy desktop route remains: {legacy}"
            );
        }
        for route in [
            "config",
            "event",
            "provider",
            "permission",
            "question",
            "command",
            "mcp",
        ] {
            let legacy = format!("\"/{route}");
            assert!(
                !source.contains(&legacy),
                "legacy desktop route remains: {legacy}"
            );
        }
        let stripped_session = ["trim_end_matches(\"", "/session\")"].concat();
        assert!(!source.contains(&stripped_session));
    }

    #[test]
    fn parse_http_server_supports_plain_and_tls_endpoints() {
        assert_eq!(
            parse_http_server("http://127.0.0.1:4096").unwrap(),
            (false, "127.0.0.1".to_string(), 4096, String::new())
        );
        assert_eq!(
            parse_http_server("https://agent.example.com").unwrap(),
            (true, "agent.example.com".to_string(), 443, String::new())
        );
        assert_eq!(
            parse_http_server("https://host.example:8443/agent").unwrap(),
            (true, "host.example".to_string(), 8443, "agent".to_string())
        );
        assert!(parse_http_server("ftp://host.example").is_err());
    }

    #[test]
    fn mcp_oauth_callback_uses_the_initiating_debug_or_proxy_endpoint() {
        assert_eq!(
            mcp_oauth_redirect_uri("http://127.0.0.1:58732/", "webflow").unwrap(),
            "http://127.0.0.1:58732/v2/plugins/dev.neoism.mcp/webflow/auth/callback",
        );
        assert_eq!(
            mcp_oauth_redirect_uri("https://host.example:8443/agent/workspaces/workspace%2Fa/", "vercel").unwrap(),
            "https://host.example:8443/agent/workspaces/workspace%2Fa/v2/plugins/dev.neoism.mcp/vercel/auth/callback",
        );
        for invalid in [
            "file:///tmp/agent", "http://user:secret@localhost:4096",
            "http://localhost:4096?token=secret", "http://localhost:4096#fragment",
        ] {
            assert!(mcp_oauth_redirect_uri(invalid, "webflow").is_err());
        }
    }

    #[test]
    fn workspace_proxy_requests_keep_the_actual_callback_host_port() {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let worker = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = Vec::new();
            while find_header_end(&request).is_none() {
                let mut bytes = [0; 1024];
                let count = stream.read(&mut bytes).unwrap();
                assert_ne!(count, 0);
                request.extend_from_slice(&bytes[..count]);
            }
            stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}").unwrap();
            String::from_utf8(request).unwrap()
        });
        let response = http_request_with_status(
            &format!("http://{address}/agent/workspaces/debug"),
            "GET",
            "/v2/plugins/dev.neoism.mcp/catalog",
            None,
            Duration::from_secs(2),
        )
        .unwrap();
        assert_eq!(response.status, 200);
        let request = worker.join().unwrap();
        assert!(request.starts_with(
            "GET /agent/workspaces/debug/v2/plugins/dev.neoism.mcp/catalog HTTP/1.1\r\n"
        ));
        assert!(request.contains(&format!("\r\nHost: {address}\r\n")));
    }

    #[test]
    fn http_host_authority_retains_nondefault_ports() {
        assert_eq!(http_authority(false, "127.0.0.1", 58732), "127.0.0.1:58732");
        assert_eq!(
            http_authority(true, "host.example", 8443),
            "host.example:8443"
        );
        assert_eq!(http_authority(false, "host.example", 80), "host.example");
        assert_eq!(http_authority(true, "host.example", 443), "host.example");
        assert_eq!(http_authority(false, "::1", 4096), "[::1]:4096");
    }

    #[test]
    fn daemon_endpoint_conversion_accepts_normalized_and_root_paths() {
        assert_eq!(
            agent_reverse_proxy_for_daemon_endpoint("ws://127.0.0.1:7878/session"),
            Some("http://127.0.0.1:7878/agent".to_string())
        );
        assert_eq!(
            agent_reverse_proxy_for_daemon_endpoint("wss://host.example/session"),
            Some("https://host.example/agent".to_string())
        );
        assert_eq!(
            agent_reverse_proxy_for_daemon_endpoint("ws://127.0.0.1:7878"),
            Some("http://127.0.0.1:7878/agent".to_string())
        );
        assert_eq!(
            agent_reverse_proxy_for_daemon_endpoint("ws://127.0.0.1:7878/session/"),
            Some("http://127.0.0.1:7878/agent".to_string())
        );
        assert_eq!(
            agent_reverse_proxy_for_daemon_workspace(
                "ws://127.0.0.1:7878/session",
                "workspace/a"
            ),
            Some("http://127.0.0.1:7878/agent/workspaces/workspace%2Fa".to_string())
        );
    }

    #[test]
    fn joined_http_and_sse_authorization_uses_registered_daemon_credential() {
        let server = "http://auth.test:7980/agent/workspaces/workspace-a";
        register_agent_server_credential(server, Some("pair-secret"));
        assert_eq!(
            authorization_header(server),
            "Authorization: Bearer pair-secret\r\n"
        );
        register_agent_server_credential(server, Some("bad\r\ninjected: yes"));
        assert!(authorization_header(server).is_empty());
    }

    #[test]
    fn authenticated_joined_endpoint_skips_redundant_readiness_probe() {
        let server = "http://127.0.0.1:1/agent/workspaces/readiness-test";
        register_agent_server_credential(server, Some("pair-secret"));
        assert!(ensure_request_server_ready(server).is_ok());
        register_agent_server_credential(server, None);
    }

    #[test]
    fn password_free_joined_endpoint_skips_uncredentialed_readiness_probe() {
        let server = "http://127.0.0.1:1/agent/workspaces/readiness-test-no-token";
        register_agent_server_credential(server, None);
        assert!(ensure_request_server_ready(server).is_ok());
        assert!(ensure_request_server_ready("http://127.0.0.1:1/custom-agent").is_ok());
    }

    #[test]
    fn final_runtime_snapshot_promotes_new_tree_child_to_running() {
        // Tree discovery initially has no matching status. The status
        // snapshot captured after discovery must promote the child before the
        // recovery snapshot is delivered to the pane.
        let mut entries = vec![
            NeoismAgentSessionEntry::new("root", "main session", "return"),
            NeoismAgentSessionEntry::new("child", "child", "explore")
                .with_runtime_status(None),
        ];
        let statuses = HashMap::from([(
            "child".to_string(),
            SessionStatusSnapshot {
                kind: "busy".to_string(),
                ..SessionStatusSnapshot::default()
            },
        )]);

        apply_subagent_runtime_snapshot(&mut entries, &statuses);

        assert_eq!(entries[1].runtime_status.as_deref(), Some("running"));
    }

    #[test]
    fn listed_child_omitted_from_live_run_snapshot_becomes_historical() {
        let mut entries = vec![
            NeoismAgentSessionEntry::new("root", "main session", "return"),
            NeoismAgentSessionEntry::new("child", "child", "explore")
                .with_runtime_status(Some("running".to_string())),
        ];

        apply_subagent_runtime_snapshot(&mut entries, &HashMap::new());

        assert_eq!(entries[1].runtime_status.as_deref(), Some("interrupted"));
    }

    #[test]
    fn cancelled_child_remains_stopped_after_runtime_reload() {
        let child =
            serde_json::json!({"id":"child", "externalAgent":{"status":"cancelled"}});
        let entry = subagent_entry_from_session(&child, &HashMap::new(), 1).unwrap();
        let mut entries =
            vec![NeoismAgentSessionEntry::new("root", "root", "root"), entry];
        apply_subagent_runtime_snapshot(&mut entries, &HashMap::new());
        assert_eq!(entries[1].runtime_status.as_deref(), Some("interrupted"));
    }

    #[test]
    fn missing_active_status_on_session_json_is_unknown() {
        let session = serde_json::json!({
            "id": "child",
            "title": "child"
        });

        assert_eq!(
            session_explicit_runtime_status(&session, &HashMap::new()),
            None
        );
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct SessionStatusSnapshot {
    pub kind: String,
    pub started_at: Option<u64>,
    pub running: Option<bool>,
    pub queue_count: usize,
    pub preview: Option<String>,
    pub parent_session_id: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct FamilyRuntimeSnapshot {
    pub root_session_id: String,
    pub family_revision: u64,
    pub branches: Vec<(String, String, Option<u64>)>,
    pub execution: Option<neoism_ui::panels::agent_pane::state::ExecutionActivityState>,
    pub running_background_tasks: Option<Vec<(String, u64)>>,
    pub background_jobs_epoch: Option<String>,
    pub background_jobs_revision: Option<u64>,
}

pub(super) fn fetch_family_runtime(
    server: &str,
    session_id: &str,
) -> Result<FamilyRuntimeSnapshot, String> {
    let value = api_request_json(
        server,
        "GET",
        &format!("/v2/sessions/{session_id}/runtime"),
        None,
    )?
    .ok_or_else(|| "Neoism Agent returned an empty runtime snapshot".to_string())?;
    family_runtime_from_json(&value, session_id)
}

pub(crate) fn family_runtime_from_json(
    value: &Value,
    fallback_session_id: &str,
) -> Result<FamilyRuntimeSnapshot, String> {
    let root_session_id = value
        .get("rootSessionId")
        .and_then(Value::as_str)
        .unwrap_or(fallback_session_id)
        .to_string();
    let branches = value
        .get("branches")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|branch| {
            Some((
                branch.get("sessionId")?.as_str()?.to_string(),
                branch.get("status")?.as_str()?.to_string(),
                branch.get("startedAt").and_then(Value::as_u64),
            ))
        })
        .collect();
    let execution = value
        .get("execution")
        .and_then(execution_activity_from_json);
    let running_background_tasks = value
        .get("runningBackgroundTasks")
        .and_then(Value::as_array)
        .map(|tasks| {
            tasks
                .iter()
                .filter_map(|task| {
                    Some((
                        task.get("jobId")?.as_str()?.to_string(),
                        task.get("startedAt")?.as_u64()?,
                    ))
                })
                .collect()
        });
    Ok(FamilyRuntimeSnapshot {
        root_session_id,
        family_revision: value
            .get("familyRevision")
            .or_else(|| value.get("revision"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
        branches,
        execution,
        running_background_tasks,
        background_jobs_epoch: value
            .get("backgroundJobsEpoch")
            .and_then(Value::as_str)
            .map(str::to_string),
        background_jobs_revision: value
            .get("backgroundJobsRevision")
            .and_then(Value::as_u64),
    })
}

pub(crate) fn execution_activity_from_json(
    activity: &Value,
) -> Option<neoism_ui::panels::agent_pane::state::ExecutionActivityState> {
    let active_segments = activity
        .get("activeSegments")?
        .as_object()?
        .iter()
        .filter_map(|(id, started)| Some((id.clone(), started.as_u64()?)))
        .collect::<BTreeMap<_, _>>();
    let session_activities = activity
        .get("sessionActivities")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(session_id, value)| {
            let active_segments = value
                .get("activeSegments")?
                .as_object()?
                .iter()
                .filter_map(|(id, started)| Some((id.clone(), started.as_u64()?)))
                .collect();
            Some((
                session_id.clone(),
                neoism_ui::panels::agent_pane::state::ProviderActivityState {
                    completed_ms: value.get("completedMs")?.as_u64()?,
                    active_segments,
                },
            ))
        })
        .collect();
    Some(
        neoism_ui::panels::agent_pane::state::ExecutionActivityState {
            execution_id: activity.get("executionId")?.as_str()?.to_string(),
            root_session_id: activity.get("rootSessionId")?.as_str()?.to_string(),
            completed_ms: activity.get("completedMs")?.as_u64()?,
            active_segments,
            session_activities,
            revision: activity.get("revision")?.as_u64()?,
            finished: activity
                .get("finished")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        },
    )
}

pub(super) fn fetch_session_statuses(
    server: &str,
) -> Result<HashMap<String, SessionStatusSnapshot>, String> {
    let value = api_request_json(server, "GET", "/v2/sessions/status", None)?
        .ok_or_else(|| "Neoism Agent returned an empty status response".to_string())?;
    let statuses = value
        .as_object()
        .ok_or_else(|| "Neoism Agent returned malformed session statuses".to_string())?;
    Ok(statuses
        .iter()
        .filter_map(|(session_id, status)| {
            let kind = status.get("type").and_then(Value::as_str)?;
            let (queue_count, preview) =
                neoism_ui::panels::agent_pane::stream_events::queue_snapshot_fields(
                    status.get("queue").unwrap_or(&Value::Null),
                );
            Some((
                session_id.clone(),
                SessionStatusSnapshot {
                    kind: kind.to_string(),
                    started_at: status
                        .get("startedAt")
                        .or_else(|| status.get("started"))
                        .and_then(Value::as_u64),
                    queue_count,
                    running: status
                        .get("queue")
                        .and_then(|queue| queue.get("running"))
                        .and_then(Value::as_bool),
                    preview,
                    parent_session_id: status
                        .get("parentSessionID")
                        .or_else(|| status.get("parentSessionId"))
                        .or_else(|| status.get("parentID"))
                        .or_else(|| status.get("parentId"))
                        .and_then(Value::as_str)
                        .map(str::to_string),
                },
            ))
        })
        .collect())
}

pub(super) fn fetch_pending_permissions(
    server: &str,
) -> Result<Vec<NeoismAgentPendingPermission>, String> {
    let value = api_request_json(server, "GET", "/v2/interactions/permissions", None)?
        .ok_or_else(|| {
            "Neoism Agent returned an empty permissions response".to_string()
        })?;
    parse_pending_permissions(&value)
}

pub(super) fn fetch_pending_questions(
    server: &str,
) -> Result<
    Vec<neoism_ui::panels::agent_pane::question_policy::NeoismAgentPendingQuestion>,
    String,
> {
    let value = api_request_json(server, "GET", "/v2/interactions/questions", None)?
        .ok_or_else(|| "Neoism Agent returned an empty questions response".to_string())?;
    let requests = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed pending questions".to_string())?;
    Ok(requests
        .iter()
        .map(neoism_ui::panels::agent_pane::question_policy::question_request_from_event)
        .filter(|question| !question.id.is_empty() && !question.questions.is_empty())
        .collect())
}

fn parse_pending_permissions(
    value: &Value,
) -> Result<Vec<NeoismAgentPendingPermission>, String> {
    let requests = value.as_array().ok_or_else(|| {
        "Neoism Agent returned malformed pending permissions".to_string()
    })?;
    Ok(requests
        .iter()
        .filter_map(|request| {
            Some(NeoismAgentPendingPermission {
                id: request.get("id")?.as_str()?.to_string(),
                session_id: request
                    .get("sessionId")
                    .or_else(|| request.get("sessionID"))?
                    .as_str()?
                    .to_string(),
                parent_session_id: request
                    .get("parentSessionID")
                    .or_else(|| request.get("parentSessionId"))
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_agent: request
                    .get("sourceAgent")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                source_title: request
                    .get("sourceTitle")
                    .and_then(Value::as_str)
                    .map(str::to_string),
                title: request
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or("Allow tool?")
                    .to_string(),
                permission: request
                    .get("permission")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                patterns: request
                    .get("patterns")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect(),
                selected: 0,
                responding: false,
            })
        })
        .collect())
}

pub(super) fn cached_config_defaults(
    server: &str,
    directory: Option<&str>,
) -> Option<ConfigDefaults> {
    // Defaults drive submissions, not just picker presentation. Seed only a
    // fresh snapshot so a stale label cannot pin an old default after refresh.
    let value = catalog_cache().fresh_snapshot(&catalog_key(
        server,
        directory,
        "/v2/config/defaults",
    ))?;
    let mut defaults = config_defaults_from_json(&value);
    defaults.provider_readiness = cached_provider_readiness(server, directory);
    if defaults.model.is_none() {
        defaults.model = catalog_cache()
            .fresh_snapshot(&catalog_key(server, directory, "/v2/providers/configured"))
            .as_ref()
            .and_then(default_model_from_providers_json);
    }
    Some(defaults)
}

pub(super) fn fetch_config_defaults(
    server: &str,
    directory: Option<&str>,
) -> Result<ConfigDefaults, String> {
    let value = fetch_catalog(server, directory, "/v2/config/defaults")?;
    Ok(config_defaults_from_json(&value))
}

// Catalog discovery may initialize providers or contact account-specific model
// endpoints. Keep it off the critical path for known composer defaults.
pub(super) fn fetch_config_catalog_defaults(
    server: &str,
    directory: Option<&str>,
    resolve_model: bool,
) -> ConfigDefaults {
    let mut defaults = ConfigDefaults::default();
    if resolve_model {
        if let Ok(providers) =
            fetch_catalog(server, directory, "/v2/providers/configured")
        {
            defaults.model = default_model_from_providers_json(&providers);
        }
    }
    defaults
}

pub(super) fn fetch_session_state(
    server: &str,
    session_id: &str,
) -> Result<SessionState, String> {
    let value =
        api_request_json(server, "GET", &format!("/v2/sessions/{session_id}"), None)?
            .ok_or_else(|| {
                "Neoism Agent returned an empty session response".to_string()
            })?;
    Ok(session_state_from_json(&value))
}

pub(super) fn fetch_session_messages(
    server: &str,
    session_id: &str,
) -> Result<Vec<NeoismAgentMessage>, String> {
    fetch_session_messages_page(server, session_id, None, 80).map(|page| page.blocks)
}

pub(super) mod conversation_cache;

/// One page of session history. `blocks` is the expanded client-side
/// render blocks (one stored message fans out into several). `raw_count`
/// is the number of *stored messages* the server returned — the only
/// reliable "is there more history" signal, since block count and message
/// count diverge (a message yields many blocks; some yield none).
pub(super) struct SessionMessagePage {
    pub blocks: Vec<NeoismAgentMessage>,
    pub raw_count: usize,
    pub oldest_cursor: Option<String>,
}

pub(super) fn fetch_session_messages_page(
    server: &str,
    session_id: &str,
    cursor: Option<&str>,
    limit: usize,
) -> Result<SessionMessagePage, String> {
    let started = super::perf::now();
    let cache_identity = conversation_cache::identity(server, session_id);
    let initial_page = cursor.is_none_or(str::is_empty);
    let mut path = format!(
        "/v2/sessions/{session_id}/messages?order=desc&limit={}&slim=true",
        limit.max(1)
    );
    if let Some(cursor) = cursor.filter(|cursor| !cursor.is_empty()) {
        path.push_str("&cursor=");
        path.push_str(&percent_encode(cursor));
    }
    let value = match api_request_json_with_read_timeout(
        server,
        "GET",
        &path,
        None,
        Duration::from_secs(10),
    ) {
        Ok(Some(value)) if value.get("items").is_some_and(Value::is_array) => value,
        result => {
            // A failed authorization/revalidation must not leave a preview
            // available for the next pane, even within its short display TTL.
            if let Some(identity) = cache_identity.as_ref() {
                conversation_cache::forget(identity);
            }
            return Err(match result {
                Err(error) => error,
                Ok(None) => "Neoism Agent returned an empty message response".to_string(),
                Ok(Some(_)) => "Neoism Agent returned malformed messages".to_string(),
            });
        }
    };
    let messages = value.get("items").and_then(Value::as_array).expect("validated items");
    let raw_messages = messages.len();
    let oldest_cursor = messages.last().and_then(|message| {
        message
            .get("info")
            .and_then(Value::as_object)
            .and_then(|info| info.get("id"))
            .and_then(Value::as_str)
            .map(str::to_string)
    });
    let raw_bytes = value.to_string().len();
    let mut blocks = message_blocks_from_response(messages, true);
    hydrate_generated_images(server, &mut blocks);
    if super::perf::enabled() {
        let tool_blocks = blocks
            .iter()
            .filter(|message| {
                matches!(
                    message.kind,
                    NeoismAgentMessageKind::Tool | NeoismAgentMessageKind::Subtask
                )
            })
            .count();
        let text_bytes: usize = blocks.iter().map(|message| message.text.len()).sum();
        tracing::info!(
            target: "neoism::agent_ui_perf",
            session_id,
            raw_messages,
            blocks = blocks.len(),
            tool_blocks,
            raw_bytes,
            text_bytes,
            elapsed_us = super::perf::elapsed_us(started),
            "agent fetch session messages"
        );
    }
    if initial_page {
        if let Some(identity) = cache_identity {
            if raw_bytes <= conversation_cache::MAX_ENTRY_BYTES {
                conversation_cache::remember(identity, value.to_string(), limit.max(1));
            } else {
                conversation_cache::forget(&identity);
            }
        }
    }
    Ok(SessionMessagePage {
        blocks,
        raw_count: raw_messages,
        oldest_cursor,
    })
}

fn message_blocks_from_response(
    messages: &[Value],
    newest_first: bool,
) -> Vec<NeoismAgentMessage> {
    neoism_ui::panels::agent_pane::api_mapping::message_blocks_from_response(
        messages,
        newest_first,
    )
    .into_iter()
    .map(NeoismAgentMessage::from)
    .collect()
}

/// Only the owned local endpoint needs a startup probe. A selected remote or
/// joined endpoint is checked by the actual request, with normal server auth.
fn ensure_request_server_ready(server: &str) -> Result<(), String> {
    if agent_server_credential(server).is_some()
        || !crate::agent_server::is_owned_local_endpoint(server)
    {
        return Ok(());
    }
    crate::agent_server::ensure_started_for_request(server)?;
    #[cfg(target_os = "linux")]
    crate::app::agent_tray::register_ready_local_endpoint(server);
    Ok(())
}

// Total wire deadline, including retries and body consumption (not an idle
// socket timeout). Admission can involve cold provider/adapter startup.
fn request_deadline(method: &str, path: &str) -> Duration {
    let path = path.split('?').next().unwrap_or(path).trim_end_matches('/');
    if method == "POST" && (path == "/v2/sessions"
        || path.ends_with("/prompt") || path.ends_with("/prompt_async")) {
        Duration::from_secs(120)
    } else {
        Duration::from_secs(15)
    }
}

pub(super) fn api_request_json(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<Option<Value>, String> {
    ensure_request_server_ready(server)?;
    let response =
        http_request(server, method, path, body, request_deadline(method, path))?;
    response_json(response)
}

/// Connect workers can be cancelled while readiness is waiting on startup.
/// Re-check before issuing the actual request (especially credential writes).
/// In-flight HTTP cannot be rolled back; its completion is separately scoped
/// by the pane's request token.
pub(super) fn api_request_json_while_active(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
    active: &std::sync::atomic::AtomicBool,
) -> Result<Option<Value>, String> {
    api_request_json_with_readiness(
        server,
        method,
        path,
        body,
        read_timeout,
        active,
        || ensure_request_server_ready(server),
    )
}

/// The production path and barrier tests share both cancellation checks and
/// the real HTTP send; only readiness is injectable, with no global test hook.
pub(super) fn api_request_json_with_readiness(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
    active: &std::sync::atomic::AtomicBool,
    readiness: impl FnOnce() -> Result<(), String>,
) -> Result<Option<Value>, String> {
    let check_active = || {
        if active.load(std::sync::atomic::Ordering::Acquire) {
            Ok(())
        } else {
            Err("Connect cancelled".to_string())
        }
    };
    check_active()?;
    readiness()?;
    check_active()?;
    let response = http_request(server, method, path, body, read_timeout)?;
    response_json(response)
}

pub(super) fn api_request_json_with_read_timeout(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
) -> Result<Option<Value>, String> {
    ensure_request_server_ready(server)?;
    let response = http_request(server, method, path, body, read_timeout)?;
    response_json(response)
}

/// Fetch the session's persistent goal. The goals plugin route returns
/// `{ goal, researchEnabled }`; we lift just the goal object into the
/// shared [`SessionGoal`] shape (or `None` when no goal is set).
pub(super) fn fetch_session_goal(
    server: &str,
    session_id: &str,
) -> Result<Option<SessionGoal>, String> {
    let value = api_request_json(
        server,
        "GET",
        &format!("/v2/plugins/dev.neoism.goals/{session_id}"),
        None,
    )?
    .unwrap_or(Value::Null);
    Ok(value.get("goal").and_then(SessionGoal::from_json))
}

fn response_json(response: HttpResponse) -> Result<Option<Value>, String> {
    if response.body.iter().all(u8::is_ascii_whitespace) {
        return Ok(None);
    }
    serde_json::from_slice(&response.body)
        .map(Some)
        .map_err(|error| format!("Neoism Agent returned invalid JSON: {error}"))
}

pub(super) fn prompt_model_json(
    model: &str,
    thinking: Option<&str>,
    connection_id: Option<&str>,
) -> Option<Value> {
    neoism_ui::panels::agent_pane::api_mapping::prompt_model_json_with_connection(
        model,
        thinking,
        connection_id,
    )
}

pub(super) fn session_model_json(
    model: &str,
    thinking: Option<&str>,
    connection_id: Option<&str>,
) -> Option<Value> {
    neoism_ui::panels::agent_pane::api_mapping::session_model_json_with_connection(
        model,
        thinking,
        connection_id,
    )
}

pub(super) fn normalize_model_ref(model: &str) -> String {
    neoism_ui::panels::agent_pane::api_mapping::normalize_model_ref(model)
}

pub(super) fn normalize_thinking(value: &str) -> String {
    neoism_ui::panels::agent_pane::api_mapping::normalize_thinking(value)
}

pub(super) fn first_interaction_id(
    server: &str,
    path: &str,
    session_id: Option<&str>,
) -> Result<Option<String>, String> {
    Ok(first_interaction_value(server, path, session_id)?
        .and_then(|item| item.get("id").and_then(Value::as_str).map(str::to_string)))
}

pub(super) fn first_interaction_value(
    server: &str,
    path: &str,
    session_id: Option<&str>,
) -> Result<Option<Value>, String> {
    let path = match session_id {
        Some(session_id) => format!("{path}?sessionId={}", percent_encode(session_id)),
        None => path.to_string(),
    };
    let value = api_request_json(server, "GET", &path, None)?.ok_or_else(|| {
        "Neoism Agent returned an empty interaction response".to_string()
    })?;
    let items = value
        .as_array()
        .ok_or_else(|| "Neoism Agent returned malformed interactions".to_string())?;
    Ok(items.iter().next().cloned())
}

pub(super) fn permission_reply_alias(value: &str) -> &'static str {
    match value.trim().to_ascii_lowercase().as_str() {
        "a" | "always" => "always",
        "n" | "no" | "deny" | "reject" => "reject",
        _ => "once",
    }
}

pub(super) fn is_permission_reply(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "once" | "always" | "reject" | "y" | "a" | "n" | "yes" | "no" | "deny"
    )
}

pub(super) fn format_queue(value: Option<&Value>) -> String {
    let Some(value) = value else {
        return "queue updated".to_string();
    };
    let count = value.get("count").and_then(Value::as_u64).unwrap_or(0);
    let running = if value
        .get("running")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        "running"
    } else {
        "idle"
    };
    let Some(items) = value.get("items").and_then(Value::as_array) else {
        return format!("queue: {count} - {running}");
    };
    if items.is_empty() {
        return format!("queue: {count} - {running}");
    }
    let mut lines = vec![format!("queue: {count} - {running}")];
    lines.extend(items.iter().map(|item| {
        let index = item.get("index").and_then(Value::as_u64).unwrap_or(0) + 1;
        let text = item
            .get("text")
            .and_then(Value::as_str)
            .filter(|text| !text.trim().is_empty())
            .unwrap_or("(empty)");
        format!("{index}. {text}")
    }));
    lines.join("\n")
}

pub(super) fn format_permissions(
    value: Option<&Value>,
    session_id: Option<&str>,
) -> String {
    let items = filter_session_items(value, session_id);
    if items.is_empty() {
        return "no pending permissions".to_string();
    }
    items
        .into_iter()
        .map(|item| {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .unwrap_or("permission");
            let title = item
                .get("title")
                .and_then(Value::as_str)
                .unwrap_or("Allow tool?");
            let patterns = item
                .get("patterns")
                .and_then(Value::as_array)
                .map(|patterns| {
                    patterns
                        .iter()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join(", ")
                })
                .unwrap_or_default();
            if patterns.is_empty() {
                format!("{id} - {title}")
            } else {
                format!("{id} - {title} - {patterns}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn format_questions(
    value: Option<&Value>,
    session_id: Option<&str>,
) -> String {
    let items = filter_session_items(value, session_id);
    if items.is_empty() {
        return "no pending questions".to_string();
    }
    items
        .into_iter()
        .map(|item| {
            let id = item.get("id").and_then(Value::as_str).unwrap_or("question");
            format!("{id} - {} ({})", question_label(item), question_count(item))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub(super) fn question_count(item: &Value) -> usize {
    item.get("questions")
        .and_then(Value::as_array)
        .map(|questions| questions.len().max(1))
        .unwrap_or(1)
}

pub(super) fn question_answers(input: &str, count: usize) -> Vec<Vec<String>> {
    if count <= 1 {
        return vec![vec![input.trim().to_string()]];
    }
    input
        .split(';')
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
        .map(|answer| vec![answer.to_string()])
        .collect()
}

/// One session surfaced by semantic transcript search, joined with its
/// title from the session list and deduped to the best (closest) message
/// hit per session.
#[derive(Clone, Debug)]
pub(crate) struct NeoismAgentSemanticSessionHit {
    pub session_id: String,
    pub title: String,
    /// Preserve project grouping and source metadata for hits outside loaded pages.
    pub entry: NeoismAgentSessionEntry,
    pub excerpt: String,
    pub distance: f64,
}

/// Search the authorized catalog, preserving grouped transcript relevance when available.
/// Returns `(hits, transcript_unavailable, indexing)`; older servers default indexing
/// to false. Title-only fallback reads at most two matching pages (100 roots).
pub(crate) fn fetch_session_search_hits(
    server: &str,
    query: &str,
    current_session: Option<&str>,
    directory: Option<&str>,
    transcript_enabled: bool,
) -> Result<(Vec<NeoismAgentSemanticSessionHit>, bool, bool), String> {
    // The grouped endpoint is authoritative for both title and message relevance.
    // Only use the legacy title catalog when transcript search is unavailable;
    // an empty ranked result is not a reason to append unrelated title matches.
    if transcript_enabled {
        if let Some((hits, indexing)) =
            fetch_semantic_session_hits(server, query, current_session, directory)?
        {
            return Ok((hits, false, indexing));
        }
    }
    let mut titles = Vec::new();
    let mut cursor = None;
    let mut title_error = None;
    for _ in 0..2 {
        let page = match fetch_sessions_query_page(
            server,
            directory,
            cursor.as_deref(),
            Some(query),
        ) {
            Ok(page) => page,
            Err(error) => {
                title_error = Some(error);
                break;
            }
        };
        titles.extend(page.items.iter().filter_map(|session| {
            session_entry(session, &HashMap::new()).map(|entry| {
                NeoismAgentSemanticSessionHit {
                    session_id: entry.id.clone(),
                    title: entry.title.clone(),
                    entry,
                    excerpt: String::new(),
                    distance: 0.0,
                }
            })
        }));
        if page.next_cursor.is_none() || page.next_cursor == cursor {
            break;
        }
        cursor = page.next_cursor;
    }
    if titles.is_empty() {
        if let Some(error) = title_error {
            return Err(error);
        }
    }
    Ok((titles, true, false))
}

/// Search embeds the same authorized SessionInfo as session detail. Only older
/// servers without that object require a separate request to the captured endpoint.
fn semantic_hit_session_entry(
    hit: &Value,
    session_id: &str,
    directory: Option<&str>,
    fetch_legacy: impl FnOnce() -> Option<Value>,
) -> Option<NeoismAgentSessionEntry> {
    let session = match hit.get("session") {
        Some(session) if !session.is_null() => session.clone(),
        _ => fetch_legacy()?,
    };
    if session.get("id").and_then(Value::as_str) != Some(session_id)
        || session
            .get("parentId")
            .or_else(|| session.get("parentID"))
            .is_some_and(|parent| !parent.is_null())
        || directory.is_some_and(|directory| {
            session.get("directory").and_then(Value::as_str) != Some(directory)
        })
    {
        return None;
    }
    session_entry(&session, &HashMap::new())
}

/// Query conversation search and retain server rank. Embedded authorized sessions
/// avoid per-result HTTP requests; legacy servers still resolve session details.
pub(crate) fn fetch_semantic_session_hits(
    server: &str,
    query: &str,
    current_session: Option<&str>,
    directory: Option<&str>,
) -> Result<Option<(Vec<NeoismAgentSemanticSessionHit>, bool)>, String> {
    let path = format!(
        "/v2/plugins/dev.neoism.semantic/search?q={}&limit=20&conversation=true{}",
        percent_encode(query),
        directory
            .map(|directory| format!("&directory={}", percent_encode(directory)))
            .unwrap_or_default()
    );
    let value = api_request_json(server, "GET", &path, None)?
        .ok_or_else(|| "empty semantic search response".to_string())?;
    if !value
        .get("available")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Ok(None);
    }
    let indexing = value
        .get("indexing")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let hits = value
        .get("hits")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    if hits.is_empty() {
        return Ok(Some((Vec::new(), indexing)));
    }
    let _ = current_session;
    let mut entries = Vec::<NeoismAgentSessionEntry>::new();
    let mut seen = std::collections::HashSet::new();
    for hit in &hits {
        let Some(session_id) = hit
            .get("sessionId")
            .or_else(|| hit.get("session_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        if !seen.insert(session_id.to_owned()) {
            continue;
        }
        if let Some(entry) =
            semantic_hit_session_entry(hit, session_id, directory, || {
                api_request_json(
                    server,
                    "GET",
                    &format!("/v2/sessions/{}", percent_encode(session_id)),
                    None,
                )
                .ok()
                .flatten()
            })
        {
            entries.push(entry);
        }
    }
    let titles: HashMap<&str, &NeoismAgentSessionEntry> = entries
        .iter()
        .map(|entry| (entry.id.as_str(), entry))
        .collect();
    let mut best: Vec<NeoismAgentSemanticSessionHit> = Vec::new();
    for hit in &hits {
        let Some(session_id) = hit
            .get("sessionId")
            .or_else(|| hit.get("session_id"))
            .and_then(Value::as_str)
        else {
            continue;
        };
        let Some(title) = titles.get(session_id) else {
            continue;
        };
        let distance = hit.get("distance").and_then(Value::as_f64).unwrap_or(1.0);
        let excerpt = hit
            .get("snippet")
            .or_else(|| hit.get("excerpt"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        // Keep the first hit for a session. The server's rank is authoritative;
        // distance/score are optional and must never reorder grouped results.
        if !best
            .iter()
            .any(|existing| existing.session_id == session_id)
        {
            best.push(NeoismAgentSemanticSessionHit {
                session_id: session_id.to_string(),
                title: title.title.clone(),
                entry: (*title).clone(),
                excerpt,
                distance,
            });
        }
    }
    Ok(Some((best, indexing)))
}

/// Bind OAuth to the pane's actual HTTP endpoint, including debug ports and
/// workspace proxy paths, rather than the agent process's global default URL.
pub(crate) fn mcp_oauth_redirect_uri(server: &str, name: &str) -> Result<String, String> {
    let mut endpoint = url::Url::parse(server.trim())
        .map_err(|_| "Invalid agent endpoint for MCP authorization".to_string())?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || endpoint.host().is_none()
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err("MCP authorization requires an HTTP(S) endpoint without credentials, query, or fragment".into());
    }
    let path = format!(
        "{}/v2/plugins/dev.neoism.mcp/{}/auth/callback",
        endpoint.path().trim_end_matches('/'),
        percent_encode(name),
    );
    endpoint.set_path(&path);
    Ok(endpoint.to_string())
}

pub(super) fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

struct HttpResponse {
    body: Vec<u8>,
    status: u16,
    reason: String,
}

pub(super) struct EventStreamConnection {
    pub stream: super::transport::AgentTransport,
    pub initial_body: Vec<u8>,
    pub chunked: bool,
}

pub(super) fn open_event_stream(
    server: &str,
    session_id: &str,
) -> Result<EventStreamConnection, String> {
    open_sse_stream(
        server,
        &format!(
            "/v2/events?sessionId={}&tail=true&limit=1",
            percent_encode(session_id)
        ),
    )
}

pub(super) fn open_session_catalog_stream(
    server: &str,
    directory: Option<&str>,
) -> Result<EventStreamConnection, String> {
    let path = session_catalog_events_path(directory);
    open_sse_stream(server, &path)
}

fn session_catalog_events_path(directory: Option<&str>) -> String {
    match directory {
        Some(directory) => format!(
            "/v2/session-catalog/events?directory={}",
            percent_encode(directory)
        ),
        None => "/v2/session-catalog/events?scope=all".to_owned(),
    }
}

fn open_sse_stream(
    server: &str,
    endpoint: &str,
) -> Result<EventStreamConnection, String> {
    ensure_request_server_ready(server)?;
    let server = server.trim().trim_end_matches('/');
    let (tls, host, port, base_path) = parse_http_server(server)?;
    let addresses = (host.as_str(), port)
        .to_socket_addrs()
        .map_err(|_| "Failed to resolve Neoism Agent server".to_string())?;
    // Retain the polling read interface used for stream cancellation, but try
    // every resolved address (localhost may resolve to IPv6 before IPv4).
    let started = Instant::now();
    let mut connected = None;
    for addr in addresses {
        let remaining = Duration::from_secs(10).saturating_sub(started.elapsed());
        if remaining.is_zero() {
            break;
        }
        if let Ok(stream) = super::transport::AgentTransport::connect(
            &addr,
            &host,
            tls,
            remaining.min(Duration::from_secs(2)),
            Duration::from_millis(40),
            Duration::from_secs(5),
        ) {
            connected = Some(stream);
            break;
        }
    }
    let mut stream =
        connected.ok_or_else(|| "Neoism Agent event connection failed".to_string())?;

    let path = request_path(&base_path, endpoint);
    let authority = http_authority(tls, &host, port);
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {authority}\r\nAccept: text/event-stream\r\n{}Connection: keep-alive\r\n\r\n",
        authorization_header(server)
    );
    stream.write_all(request.as_bytes()).map_err(|error| {
        format!("failed to write Neoism Agent event request: {error}")
    })?;

    let mut response = Vec::new();
    let header_started = Instant::now();
    loop {
        if header_started.elapsed() > Duration::from_secs(15) {
            return Err("Neoism Agent event stream timed out waiting for headers".into());
        }
        if response.len() > 64 * 1024 {
            return Err("Neoism Agent event headers exceed size limit".into());
        }
        let mut chunk = [0u8; 4096];
        match stream.read(&mut chunk) {
            Ok(0) => return Err("Neoism Agent closed the event stream".to_string()),
            Ok(n) => {
                response.extend_from_slice(&chunk[..n]);
                if let Some(header_end) = find_header_end(&response) {
                    let headers = String::from_utf8_lossy(&response[..header_end]);
                    let mut lines = headers.lines();
                    let status_line = lines.next().ok_or_else(|| {
                        "Neoism Agent returned an empty event response".to_string()
                    })?;
                    let mut status_parts = status_line.splitn(3, ' ');
                    let _http = status_parts.next();
                    let status = status_parts
                        .next()
                        .and_then(|status| status.parse::<u16>().ok())
                        .ok_or_else(|| {
                            "Neoism Agent returned an invalid event status".to_string()
                        })?;
                    let reason = status_parts.next().unwrap_or_default();
                    if !(200..300).contains(&status) {
                        let body = String::from_utf8_lossy(&response[header_end + 4..]);
                        let suffix = if body.trim().is_empty() {
                            String::new()
                        } else {
                            format!(": {}", body.trim())
                        };
                        return Err(format!(
                            "Neoism Agent event stream HTTP {status} {reason}{suffix}"
                        ));
                    }
                    let chunked = headers
                        .to_ascii_lowercase()
                        .contains("transfer-encoding: chunked");
                    return Ok(EventStreamConnection {
                        stream,
                        initial_body: response[header_end + 4..].to_vec(),
                        chunked,
                    });
                }
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
                ) =>
            {
                if header_started.elapsed() > Duration::from_secs(15) {
                    return Err(
                        "Neoism Agent event stream timed out waiting for headers"
                            .to_string(),
                    );
                }
            }
            Err(error) => {
                return Err(format!(
                    "failed to read Neoism Agent event headers: {error}"
                ));
            }
        }
    }
}

/// Workspace registry transport; callers must run this off the UI thread. Reuses
/// endpoint-scoped credentials and the existing daemon management bridge.
/// A listing GET may return None: the management routes are optional and are
/// absent unless NEOISM_AGENT_MANAGEMENT_API=1. Only an actual HTTP 404 has this
/// meaning; registration failures and every other error remain actionable.
pub(crate) fn conversation_workspace_registry_request(
    server: &str,
    root: Option<&std::path::Path>,
) -> Result<Option<Value>, String> {
    let body =
        root.map(|root| serde_json::json!({"root": root, "createDirectory": false}));
    let response = http_request_with_status(
        server,
        if body.is_some() { "POST" } else { "GET" },
        "/v2/management/workspaces",
        body.as_ref(),
        Duration::from_secs(8),
    )?;
    if root.is_none() && response.status == 404 {
        return Ok(None);
    }
    let response = require_http_success(response)?;
    serde_json::from_slice(&response.body)
        .map(Some)
        .map_err(|error| error.to_string())
}

fn require_http_success(response: HttpResponse) -> Result<HttpResponse, String> {
    if !(200..300).contains(&response.status) {
        return Err(http_error(
            response.status,
            &response.reason,
            &String::from_utf8_lossy(&response.body),
        ));
    }
    Ok(response)
}

fn http_request(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
) -> Result<HttpResponse, String> {
    let response = require_http_success(http_request_with_status(
        server,
        method,
        path,
        body,
        read_timeout,
    )?)?;
    // Provider account/default edits and config writes affect every directory
    // on this endpoint. Invalidate only after HTTP success, including OAuth
    // callback and connection removal (failed/cancelled writes retain good data).
    if method != "GET"
        && method != "HEAD"
        && (path.starts_with("/v2/providers/")
            || path.starts_with("/v2/config")
            || path.starts_with("/v2/auth"))
    {
        invalidate_catalog_endpoint(server);
    }
    Ok(response)
}

fn http_request_with_status(
    server: &str,
    method: &str,
    path: &str,
    body: Option<&Value>,
    read_timeout: Duration,
) -> Result<HttpResponse, String> {
    let server = server.trim().trim_end_matches('/');
    // Legacy callers specify a "read timeout"; it now bounds the entire wire
    // operation. Never let a short caller override truncate session admission.
    let operation_deadline = request_deadline(method, path);
    let read_timeout = if operation_deadline >= Duration::from_secs(120) {
        read_timeout.max(operation_deadline)
    } else {
        read_timeout
    };
    let (tls, host, port, base_path) = parse_http_server(server)?;
    let url = url::Url::parse(&format!(
        "{}://{}{}",
        if tls { "https" } else { "http" },
        http_authority(tls, &host, port),
        request_path(&base_path, path),
    ))
    .map_err(|_| "Invalid Neoism Agent request URL".to_string())?;
    let method = reqwest::Method::from_bytes(method.as_bytes())
        .map_err(|_| "Invalid Neoism Agent request method".to_string())?;
    let body = body.map(serde_json::to_string).transpose()
        .map_err(|_| "Failed to encode Neoism Agent request".to_string())?;
    let credential =
        agent_server_credential(server).filter(|token| !token.contains(['\r', '\n']));
    let (status, reason, body) =
        super::transport::request(url, method, credential, body, read_timeout)?;
    Ok(HttpResponse {
        body,
        status,
        reason,
    })
}

pub(super) fn http_error(status: u16, reason: &str, body: &str) -> String {
    let envelope = serde_json::from_str::<Value>(body).ok();
    let message = envelope
        .as_ref()
        .and_then(|value| value.get("message"))
        .and_then(Value::as_str)
        .filter(|message| !message.trim().is_empty());
    if matches!(status, 422 | 429)
        && envelope
            .as_ref()
            .and_then(|value| value.get("code"))
            .and_then(Value::as_str)
            .is_some_and(|code| {
                matches!(code, "search.busy" | "search.deadline" | "search.too_broad")
            })
    {
        if let Some(message) = message {
            return message.to_owned();
        }
    }
    let detail = message.unwrap_or(body.trim());
    let suffix = if detail.is_empty() {
        String::new()
    } else {
        format!(": {detail}")
    };
    format!("Neoism Agent HTTP {status} {reason}{suffix}")
}

fn authorization_header(server: &str) -> String {
    agent_server_credential(server)
        .filter(|token| !token.contains(['\r', '\n']))
        .map(|token| format!("Authorization: Bearer {token}\r\n"))
        .unwrap_or_default()
}

fn http_authority(tls: bool, host: &str, port: u16) -> String {
    let host = if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]")
    } else {
        host.to_string()
    };
    if port == if tls { 443 } else { 80 } {
        host
    } else {
        format!("{host}:{port}")
    }
}

fn parse_http_server(server: &str) -> Result<(bool, String, u16, String), String> {
    let endpoint = url::Url::parse(server)
        .map_err(|_| "Invalid Neoism Agent endpoint".to_string())?;
    if !matches!(endpoint.scheme(), "http" | "https")
        || !endpoint.username().is_empty()
        || endpoint.password().is_some()
        || endpoint.query().is_some()
        || endpoint.fragment().is_some()
    {
        return Err(
            "Neoism Agent requires HTTP(S) without URL credentials, query, or fragment"
                .into(),
        );
    }
    let host = match endpoint.host() {
        Some(url::Host::Domain(host)) => host.to_string(),
        Some(url::Host::Ipv4(host)) => host.to_string(),
        Some(url::Host::Ipv6(host)) => host.to_string(),
        None => return Err("Neoism Agent endpoint requires a host".into()),
    };
    Ok((
        endpoint.scheme() == "https",
        host,
        endpoint.port_or_known_default().unwrap(),
        endpoint.path().trim_matches('/').to_string(),
    ))
}

#[cfg(test)]
mod transport_policy_tests {
    use super::*;

    #[test]
    fn ipv6_and_proxy_base_paths_are_preserved() {
        assert_eq!(
            parse_http_server("http://[::1]:4096/agent/workspaces/abc").unwrap(),
            (false, "::1".into(), 4096, "agent/workspaces/abc".into())
        );
        assert_eq!(
            parse_http_server("https://[2001:db8::1]/agent/").unwrap(),
            (true, "2001:db8::1".into(), 443, "agent".into())
        );
        assert_eq!(http_authority(false, "::1", 4096), "[::1]:4096");
        assert_eq!(
            request_path("agent/workspaces/abc", "/v2/sessions?q=a%2Fb"),
            "/agent/workspaces/abc/v2/sessions?q=a%2Fb"
        );
    }

    #[test]
    fn endpoint_errors_do_not_disclose_credentials() {
        for endpoint in [
            "ftp://secret:password@host",
            "http://host?token=secret",
            "http://secret:password@host",
            "secret",
        ] {
            let error = parse_http_server(endpoint).unwrap_err();
            assert!(!error.contains("secret"));
            assert!(!error.contains("password"));
        }
    }

    #[test]
    fn admission_has_long_bounded_deadline_but_reads_are_interactive() {
        for path in [
            "/v2/sessions",
            "/v2/sessions?directory=x",
            "/v2/sessions/id/prompt",
            "/v2/sessions/id/prompt_async",
        ] {
            assert!(request_deadline("POST", path) >= Duration::from_secs(120));
            assert_eq!(request_deadline("GET", path), Duration::from_secs(15));
        }
        assert_eq!(
            request_deadline("POST", "/v2/interactions/questions/id/reply"),
            Duration::from_secs(15)
        );
    }
}

fn request_path(base_path: &str, path: &str) -> String {
    let path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    if base_path.is_empty() {
        path
    } else {
        format!("/{base_path}{path}")
    }
}

fn find_header_end(bytes: &[u8]) -> Option<usize> {
    bytes.windows(4).position(|window| window == b"\r\n\r\n")
}

/// Build a `/sessions` picker option (title only — the date-group header
/// carries the day; a pin marker + current-session dot render inline) paired
/// with the raw `updated` timestamp used to bucket it under a date header.
fn session_option_input(
    session: &Value,
    current_id: Option<&str>,
) -> Option<SessionOptionInput> {
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Untitled")
        .to_string();
    let updated_ms = session_updated_at(session);
    let mut option = NeoismAgentPickerOption::new(&title, "", "", &id);
    option.is_current = Some(id.as_str()) == current_id;
    option.pinned = session_pinned(session);
    Some(SessionOptionInput { option, updated_ms })
}

#[cfg(test)]
#[test]
fn catalog_activity_json_deserializes_canonical_enum() {
    use neoism_agent_core::CatalogActivity;
    for (wire, expected) in [
        ("idle", CatalogActivity::Idle),
        ("running", CatalogActivity::Running),
        ("background", CatalogActivity::Background),
        ("permission", CatalogActivity::Permission),
    ] {
        let row = session_entry(
            &serde_json::json!({"id":"root", "catalogActivity":wire, "status":"busy"}),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(row.catalog_activity, Some(expected));
    }
    for value in [
        serde_json::Value::Null,
        serde_json::json!("invalid"),
        serde_json::json!(true),
    ] {
        let row = session_entry(
            &serde_json::json!({"id":"root", "catalogActivity":value}),
            &HashMap::new(),
        )
        .unwrap();
        assert_eq!(row.catalog_activity, None);
    }
    assert_eq!(
        session_entry(&serde_json::json!({"id":"root"}), &HashMap::new())
            .unwrap()
            .catalog_activity,
        None
    );
}

/// Flat side-panel entry for one session.
pub(super) fn session_entry(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
) -> Option<NeoismAgentSessionEntry> {
    let external = session
        .get("externalAgent")
        .or_else(|| session.pointer("/extra/externalAgent"));
    if external.is_some_and(|external| {
        external.get("historyState").and_then(Value::as_str) != Some("neoism_only")
    }) {
        return None;
    }
    let id = session.get("id").and_then(Value::as_str)?.to_string();
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .filter(|title| !title.trim().is_empty())
        .unwrap_or("Untitled")
        .to_string();
    Some(
        NeoismAgentSessionEntry::new(id, title, "")
            .with_directory(session.get("directory").and_then(Value::as_str).map(str::to_owned))
            .with_source(neoism_ui::panels::agent_pane::state::side_panel::ConversationSource::from_session_json(session))
            .with_source_key(session.pointer("/externalAgent/sourceKey").or_else(|| session.pointer("/extra/externalAgent/sourceKey")).and_then(Value::as_str).map(str::to_owned))
            .with_updated_ms(session_updated_at(session))
            .with_pinned(session_pinned(session))
            .with_catalog_activity(session.get("catalogActivity").and_then(|value| serde_json::from_value(value.clone()).ok()))
            .with_runtime_status(session_running_status(session, statuses)),
    )
}

/// Surface a home-mode session's runtime status *only* when it is actively
/// running, so the side panel paints the green "running" dot. Idle,
/// blocked, and finished sessions return `None` (no dot). Unlike the
/// sub-agent path this never terminalizes — a plain finished session isn't
/// a branch that needs a status dot.
fn session_running_status(
    session: &Value,
    statuses: &HashMap<String, SessionStatusSnapshot>,
) -> Option<String> {
    let id = session.get("id").and_then(Value::as_str)?;
    let kind = statuses
        .get(id)
        .map(|status| status.kind.clone())
        .or_else(|| {
            session
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_string)
        })?;
    normalize_explicit_runtime_status(&kind)
        .filter(|status| *status == "running")
        .map(str::to_string)
}

/// Whether a session JSON carries the flattened `pinned` flag.
fn session_pinned(session: &Value) -> bool {
    session
        .get("pinned")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

/// `POST /session/:id/pin` — toggle the session's pinned flag. Returns the new
/// pinned state read back from the updated session info.
pub(crate) fn set_session_pinned(
    server: &str,
    session_id: &str,
    pinned: bool,
) -> Result<bool, String> {
    let body = serde_json::json!({ "pinned": pinned });
    let value = api_request_json(
        server,
        "POST",
        &format!("/v2/sessions/{session_id}/pin"),
        Some(&body),
    )?;
    Ok(value.as_ref().map(session_pinned).unwrap_or(pinned))
}

/// `DELETE /session/:id` — permanently delete a session.
pub(crate) fn delete_session(server: &str, session_id: &str) -> Result<(), String> {
    api_request_json(
        server,
        "DELETE",
        &format!("/v2/sessions/{session_id}"),
        None,
    )?;
    Ok(())
}

/// `PATCH /session/:id` with `{ title }` — rename a session.
pub(crate) fn rename_session(
    server: &str,
    session_id: &str,
    title: &str,
) -> Result<(), String> {
    let body = serde_json::json!({ "title": title });
    api_request_json(
        server,
        "PATCH",
        &format!("/v2/sessions/{session_id}"),
        Some(&body),
    )?;
    Ok(())
}

fn session_updated_at(session: &Value) -> u64 {
    session
        .get("time")
        .and_then(|time| time.get("updated"))
        .and_then(Value::as_u64)
        .unwrap_or(0)
}

#[allow(dead_code)]
fn model_label(model: Option<&Value>) -> String {
    let Some(model) = model else {
        return "server default".to_string();
    };
    let provider = model
        .get("providerId")
        .or_else(|| model.get("provider_id"))
        .and_then(Value::as_str)
        .unwrap_or("provider");
    let id = model
        .get("id")
        .or_else(|| model.get("modelId"))
        .or_else(|| model.get("model_id"))
        .and_then(Value::as_str)
        .unwrap_or("model");
    format!("{provider}/{id}")
}

pub(super) fn part_block(part: &Value) -> Option<NeoismAgentMessage> {
    neoism_ui::panels::agent_pane::api_mapping::part_block(part)
        .map(NeoismAgentMessage::from)
}

pub(super) fn hydrate_generated_images(
    server: &str,
    messages: &mut [NeoismAgentMessage],
) {
    for message in messages {
        for image in &mut message.images {
            if !image.mime.starts_with("image/")
                || !image.url.starts_with("/v2/artifacts/")
                || !image.url.ends_with("/content")
            {
                continue;
            }
            let Ok(response) =
                http_request(server, "GET", &image.url, None, Duration::from_secs(30))
            else {
                continue;
            };
            if response.body.len() > 25 * 1024 * 1024 {
                continue;
            }
            let encoded = base64::engine::general_purpose::STANDARD.encode(response.body);
            image.url = format!("data:{};base64,{encoded}", image.mime);
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentTodo> for NeoismAgentTodo {
    fn from(todo: neoism_ui::panels::agent_pane::state::NeoismAgentTodo) -> Self {
        Self {
            status: todo.status,
            content: todo.content,
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentUsage> for NeoismAgentUsage {
    fn from(usage: neoism_ui::panels::agent_pane::state::NeoismAgentUsage) -> Self {
        Self {
            input: usage.input,
            output: usage.output,
            reasoning: usage.reasoning,
            cache_read: usage.cache_read,
            cache_write: usage.cache_write,
            total: usage.total,
            cost_micros: usage.cost_micros,
            context_limit: usage.context_limit,
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind>
    for NeoismAgentMessageKind
{
    fn from(kind: neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind) -> Self {
        match kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::User => {
                Self::User
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Assistant => {
                Self::Assistant
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Reasoning => {
                Self::Reasoning
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Tool => {
                Self::Tool
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::System => {
                Self::System
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Subtask => {
                Self::Subtask
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Compaction => {
                Self::Compaction
            }
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind>
    for NeoismAgentOutputKind
{
    fn from(kind: neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind) -> Self {
        match kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Text => {
                Self::Text
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Code => {
                Self::Code
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentOutputKind::Todos => {
                Self::Todos
            }
        }
    }
}

impl From<neoism_ui::panels::agent_pane::state::NeoismAgentMessage>
    for NeoismAgentMessage
{
    fn from(message: neoism_ui::panels::agent_pane::state::NeoismAgentMessage) -> Self {
        let status = message.status.clone();
        let tool = message.tool.clone();
        let todos = message
            .todos
            .into_iter()
            .map(NeoismAgentTodo::from)
            .collect();
        let mut out = match message.kind {
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::User => {
                NeoismAgentMessage::user(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Assistant => {
                NeoismAgentMessage::assistant(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Reasoning => {
                NeoismAgentMessage::reasoning(message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Tool => {
                NeoismAgentMessage::tool(
                    message.title,
                    message.text,
                    message.status,
                    message.tool,
                    message.output_kind.into(),
                    message.lang,
                    todos,
                )
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::System => {
                NeoismAgentMessage::system(message.title, message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Subtask => {
                NeoismAgentMessage::subtask(message.title, message.text)
            }
            neoism_ui::panels::agent_pane::state::NeoismAgentMessageKind::Compaction => {
                NeoismAgentMessage::compaction(message.text, message.status)
            }
        };
        out.id = message.id;
        out.tool_batch_id = message.tool_batch_id;
        out.mcp = message.mcp;
        // Assistant `status` carries the settled response footer
        // (agent/model/duration). Keep it alongside ordinary tool statuses.
        out.status = status;
        out.tool = tool;
        out.line_offset = message.line_offset;
        out.detail = message.detail;
        out.usage = message.usage.map(NeoismAgentUsage::from);
        // Carry the sender's presence name through the shared→desktop
        // conversion. Without this every user bubble renders as "You" with
        // the LOCAL orb/color even for remote peers, because the constructors
        // above default `author` to None — the shared api_mapping already
        // resolved it from info.author / part.author, so just keep it.
        out.author = message.author;
        out.images = message.images;
        out.composition_children = message.composition_children;
        out.composition_child = message.composition_child;
        out
    }
}

fn short_path(path: &str) -> String {
    let parts = path
        .split('/')
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>();
    if parts.len() <= 2 {
        path.to_string()
    } else {
        parts[parts.len() - 2..].join("/")
    }
}

fn short_skill_path(path: &str) -> String {
    let path = path.trim();
    if path.is_empty() {
        return "skill".to_string();
    }
    for marker in ["/skills/", "/skill/"] {
        if let Some(index) = path.find(marker) {
            return path[index + marker.len()..].to_string();
        }
    }
    short_path(path)
}

fn filter_session_items<'a>(
    value: Option<&'a Value>,
    session_id: Option<&str>,
) -> Vec<&'a Value> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter(|item| {
                    session_id.is_none_or(|session_id| {
                        item_session_id(item).as_deref() == Some(session_id)
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn item_session_id(item: &Value) -> Option<String> {
    item.get("sessionId")
        .or_else(|| item.get("sessionID"))
        .or_else(|| item.get("session_id"))
        .and_then(Value::as_str)
        .map(str::to_string)
}

fn question_label(item: &Value) -> String {
    item.get("questions")
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
        .and_then(|question| {
            question
                .get("question")
                .or_else(|| question.get("label"))
                .and_then(Value::as_str)
        })
        .unwrap_or("Question")
        .to_string()
}
