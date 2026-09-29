//! Versioned, per-context last-good ACP preview snapshots. Only this route uses
//! these snapshots; root creation and prompt replay still contact the provider.
use super::*;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

const VERSION: u32 = 1;
const FRESH: Duration = Duration::from_secs(10 * 60);
const RETRY: Duration = Duration::from_secs(60);
const MAX_BYTES: u64 = 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Snapshot {
    version: u32,
    key: String,
    saved_at: u64,
    response: ExternalOptionsResponse,
}

struct Flight {
    gate: Arc<tokio::sync::Mutex<()>>,
    retry_after: Mutex<Option<std::time::Instant>>,
}

fn flights() -> &'static Mutex<HashMap<String, Arc<Flight>>> {
    static FLIGHTS: OnceLock<Mutex<HashMap<String, Arc<Flight>>>> = OnceLock::new();
    FLIGHTS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn flight(key: &str) -> Arc<Flight> {
    let mut map = flights().lock().unwrap_or_else(|e| e.into_inner());
    // Evict only idle keys: never allow an in-flight key to acquire a second gate.
    if map.len() >= 1024 {
        map.retain(|_, entry| Arc::strong_count(entry) > 1);
    }
    map.entry(key.to_owned())
        .or_insert_with(|| {
            Arc::new(Flight {
                gate: Arc::new(tokio::sync::Mutex::new(())),
                retry_after: Mutex::new(None),
            })
        })
        .clone()
}

pub(super) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(super) fn key(
    runtime: ExternalRuntime,
    cwd: &str,
    choices: &BTreeMap<String, String>,
    state: &AppState,
) -> Result<String, ApiError> {
    let config = runtime
        .acp_config(cwd, state.services())
        .map_err(ApiError::bad_request)?;
    let command = Path::new(&config.command);
    let meta = std::fs::metadata(command).ok();
    let modified = meta
        .as_ref()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|t| t.as_nanos());
    // Hash (never persist) the config including env values. Package versions are
    // pinned in args; executable identity includes path, size and mtime.
    let context = serde_json::to_vec(&(
        VERSION,
        runtime.provider_id(),
        cwd,
        choices,
        &config.command,
        &config.args,
        &config.env,
        meta.map(|m| m.len()),
        modified,
    ))
    .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok(format!("{:x}", Sha256::digest(context)))
}

pub(super) fn path(key: &str) -> PathBuf {
    neoism_agent_builtins::default_cache_dir()
        .join("acp-options-v1")
        .join(format!("{key}.json"))
}

// Do not persist arbitrary adapter JSON fields (including possible private
// vendor extensions). Keep only UI-visible, public selector and command fields.
fn public_fields(value: &Value, fields: &[&str]) -> Value {
    let mut out = serde_json::Map::new();
    for field in fields {
        if let Some(v) = value.get(*field) {
            out.insert((*field).into(), v.clone());
        }
    }
    Value::Object(out)
}

pub(super) fn sanitized(
    mut response: ExternalOptionsResponse,
) -> Option<ExternalOptionsResponse> {
    response.config_options = response
        .config_options
        .iter()
        .map(|option| {
            let mut item = public_fields(
                option,
                &[
                    "id",
                    "name",
                    "description",
                    "category",
                    "type",
                    "currentValue",
                ],
            );
            if let Some(groups) = option.get("options").and_then(Value::as_array) {
                item["options"] = Value::Array(
                    groups
                        .iter()
                        .map(|group| {
                            let mut entry = public_fields(
                                group,
                                &["value", "name", "label", "description"],
                            );
                            if let Some(children) =
                                group.get("options").and_then(Value::as_array)
                            {
                                entry["options"] = Value::Array(
                                    children
                                        .iter()
                                        .map(|child| {
                                            public_fields(
                                                child,
                                                &["value", "name", "description"],
                                            )
                                        })
                                        .collect(),
                                );
                            }
                            entry
                        })
                        .collect(),
                );
            }
            item
        })
        .collect();
    response.available_commands = response
        .available_commands
        .iter()
        .map(|cmd| {
            let mut item = public_fields(cmd, &["name", "description"]);
            if let Some(input) = cmd.get("input") {
                item["input"] = if input.is_null() {
                    Value::Null
                } else {
                    public_fields(input, &["hint"])
                };
            }
            item
        })
        .collect();
    response.external_session_id = None;
    response.replay_error = None;
    response.catalog_stale = None;
    validate_options(&json!(response.config_options)).ok()?;
    validate_commands(&json!({"availableCommands":response.available_commands})).ok()?;
    if response.selected_options.iter().any(|(id, value)| {
        response
            .config_options
            .iter()
            .find(|item| item["id"] == id.as_str())
            .is_none_or(|item| {
                item["currentValue"] != value.as_str()
                    || option(&response.config_options, id, value).is_err()
            })
    }) {
        return None;
    }
    Some(response)
}

fn load(
    key: &str,
    provider: &str,
    choices: &BTreeMap<String, String>,
) -> Option<Snapshot> {
    let file = path(key);
    if std::fs::metadata(&file).ok()?.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(file).ok()?;
    let mut snap: Snapshot = serde_json::from_slice(&bytes).ok()?;
    if snap.version != VERSION
        || snap.key != key
        || snap.response.provider != provider
        || snap.response.selected_options != *choices
        || snap.response.external_session_id.is_some()
        || snap.response.replay_error.is_some()
        || snap.response.catalog_stale.is_some()
        || snap.saved_at > now()
    {
        return None;
    }
    snap.response = sanitized(snap.response)?;
    Some(snap)
}

fn save(key: &str, response: ExternalOptionsResponse) -> bool {
    let Some(response) = sanitized(response) else {
        return false;
    };
    let file = path(key);
    let Some(dir) = file.parent() else {
        return false;
    };
    if std::fs::create_dir_all(dir).is_err() {
        return false;
    };
    let snapshot = Snapshot {
        version: VERSION,
        key: key.to_owned(),
        saved_at: now(),
        response,
    };
    let Ok(bytes) = serde_json::to_vec(&snapshot) else {
        return false;
    };
    if bytes.len() as u64 > MAX_BYTES {
        return false;
    }
    let temp = dir.join(format!(
        ".{key}.{}.tmp",
        neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
    ));
    let saved =
        std::fs::write(&temp, bytes).is_ok() && std::fs::rename(&temp, &file).is_ok();
    let _ = std::fs::remove_file(temp);
    saved
}

pub(super) async fn preview(
    state: &AppState,
    runtime: ExternalRuntime,
    cwd: &str,
    choices: BTreeMap<String, String>,
) -> Result<ExternalOptionsResponse, ApiError> {
    let key = key(runtime, cwd, &choices, state)?;
    let slot = flight(&key);
    if let Some(mut snapshot) = load(&key, runtime.provider_id(), &choices) {
        if now().saturating_sub(snapshot.saved_at) < FRESH.as_secs() {
            return Ok(snapshot.response);
        }
        snapshot.response.catalog_stale = Some(true);
        let allowed = slot
            .retry_after
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_none_or(|t| t <= std::time::Instant::now());
        if allowed {
            if let Ok(guard) = slot.gate.clone().try_lock_owned() {
                let state = state.clone();
                let cwd = cwd.to_owned();
                tokio::spawn(async move {
                    let success = ephemeral(&state, runtime, &cwd, choices)
                        .await
                        .is_ok_and(|response| save(&key, response));
                    *slot.retry_after.lock().unwrap_or_else(|e| e.into_inner()) =
                        (!success).then(|| std::time::Instant::now() + RETRY);
                    drop(guard);
                });
            }
        }
        return Ok(snapshot.response);
    }
    // A cold miss waits for its peer, then checks disk again. Never launch two
    // adapters for the same key, even when concurrent callers arrive together.
    let _guard = slot.gate.lock().await;
    if let Some(snap) = load(&key, runtime.provider_id(), &choices) {
        let mut response = snap.response;
        if now().saturating_sub(snap.saved_at) >= FRESH.as_secs() {
            response.catalog_stale = Some(true);
        }
        return Ok(response);
    }
    let response = ephemeral(state, runtime, cwd, choices).await?;
    let _ = save(&key, response.clone());
    Ok(response)
}
