//! Native provider drivers share orchestration and transcript ownership, not ACP.
use super::*;
use std::collections::BTreeSet;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex as StdMutex, Weak};
use tokio::sync::Mutex;

#[path = "native/connection.rs"]
mod connection;
#[path = "native/projector.rs"]
mod projector;
#[path = "native/interaction.rs"]
mod interaction;
use connection::{NativeClient, WireEvent};
pub(crate) use projector::NativeProjector;

#[cfg(all(test, unix))]
#[path = "native/manager_tests.rs"]
mod manager_tests;

const RPC_TIMEOUT: Duration = Duration::from_secs(90);
const IDLE_TIMEOUT: Duration = Duration::from_secs(30 * 60);
const BACKGROUND_TIMEOUT: Duration = Duration::from_secs(4 * 60 * 60);

#[derive(Default)]
pub(crate) struct NativeRegistry {
    sessions: Arc<Mutex<HashMap<String, Arc<NativeSession>>>>,
    stopping: AtomicBool,
}

struct Operation {
    projector: Arc<NativeProjector>,
    cancellation: Arc<AtomicBool>,
    finished: bool,
}

struct NativeSession {
    client: NativeClient,
    runtime: ExternalRuntime,
    cwd: String,
    owner: Option<(Weak<crate::state::InnerState>, String)>,
    native_id: Mutex<Option<String>>,
    opened: AtomicBool,
    retired: AtomicBool,
    active: AtomicBool,
    generation: AtomicU64,
    operations: Mutex<HashMap<String, Operation>>,
    metadata: Mutex<Value>,
    effective: Mutex<Value>,
    policy: Value,
    idle: StdMutex<Option<tokio::task::JoinHandle<()>>>,
}

impl NativeRegistry {
    async fn acquire(&self, state: &AppState, runtime: ExternalRuntime, cwd: &str, owner: Option<&SessionInfo>) -> Result<Arc<NativeSession>, String> {
        if self.stopping.load(Ordering::SeqCst) { return Err("native provider registry is shutting down".into()); }
        let policy = if let Some(owner) = owner {
            if owner.directory != cwd || !runtime.owns_session(owner) || runtime.is_legacy_session(owner)
                || !crate::caller::services_allow_session_path(state.services(), owner, Path::new(cwd))
                || !crate::caller::native_execution_allowed(&crate::caller::session_execution_policy(state.services(), owner))
            { return Err("native provider session is not admitted for this driver or directory".into()); }
            let rules = current_permissions(state, owner.id.as_str()).await?;
            native_policy(&rules)
        } else {
            native_policy(&[])
        };
        if state.services().shared_control_plane() || !crate::caller::native_execution_allowed(&crate::workspace_runtime::directory_execution_policy(state.services(), Path::new(cwd))) {
            return Err("native provider execution is not authorized for this directory".into());
        }
        let key = owner.map(|session| format!("{}:{}:{}", crate::caller::session_tenant(session), runtime.native_driver_id(), session.id));
        if let Some(key) = &key {
            let existing = {
                let sessions = self.sessions.lock().await;
                sessions.get(key).cloned().inspect(|session| { session.generation.fetch_add(1, Ordering::SeqCst); })
            };
            if let Some(existing) = existing {
                if !existing.retired.load(Ordering::SeqCst) && !existing.client.is_closed()
                    && existing.runtime == runtime && existing.cwd == cwd && existing.policy == policy
                {
                    return Ok(existing);
                }
                existing.close().await;
                self.sessions.lock().await.remove(key);
            }
        }
        let session = NativeSession::spawn(state, runtime, cwd, owner, policy).await?;
        if let Some(key) = key {
            let mut sessions = self.sessions.lock().await;
            if self.stopping.load(Ordering::SeqCst) {
                drop(sessions);
                session.close().await;
                return Err("native provider registry is shutting down".into());
            }
            sessions.insert(key, session.clone());
        }
        Ok(session)
    }

    pub(crate) async fn shutdown(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        let sessions = self.sessions.lock().await.drain().map(|(_, session)| session).collect::<Vec<_>>();
        for session in sessions { session.close().await; }
    }

    fn arm_idle(&self, session: &Arc<NativeSession>) {
        let generation = session.generation.load(Ordering::SeqCst);
        let weak = Arc::downgrade(session);
        let registry = Arc::downgrade(&self.sessions);
        let handle = tokio::spawn(async move {
            let Some(session) = weak.upgrade() else { return; };
            let timeout = if session.has_background_work().await { BACKGROUND_TIMEOUT } else { IDLE_TIMEOUT };
            drop(session);
            tokio::time::sleep(timeout).await;
            let (Some(registry), Some(session)) = (registry.upgrade(), weak.upgrade()) else { return; };
            let should_close = {
                let sessions = registry.lock().await;
                let owned = sessions.values().any(|entry| Arc::ptr_eq(entry, &session));
                owned && session.generation.load(Ordering::SeqCst) == generation
                    && !session.active.load(Ordering::SeqCst)
                    && session.retired.compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst).is_ok()
            };
            if should_close {
                session.close().await;
                registry.lock().await.retain(|_, entry| !Arc::ptr_eq(entry, &session));
            }
        });
        if let Ok(mut idle) = session.idle.lock() {
            if let Some(previous) = idle.replace(handle) { previous.abort(); }
        }
    }
}

impl NativeSession {
    async fn spawn(state: &AppState, runtime: ExternalRuntime, cwd: &str, owner: Option<&SessionInfo>, policy: Value) -> Result<Arc<Self>, String> {
        let (host, dependencies) = prepare_runtime().await?;
        let node = executable(state, "node", true)?.unwrap();
        let cli = executable(state, runtime.agent_name(), runtime != ExternalRuntime::Claude)?;
        let npm = executable(state, "npm", false)?;
        let (client, events) = NativeClient::spawn(&node, &host, Path::new(cwd))?;
        let native_id = owner.and_then(native_id);
        let session = Arc::new(Self {
            client, runtime, cwd: cwd.to_owned(),
            owner: owner.map(|owner| (Arc::downgrade(&state.inner), owner.id.to_string())),
            native_id: Mutex::new(native_id), opened: AtomicBool::new(false),
            retired: AtomicBool::new(false), active: AtomicBool::new(false), generation: AtomicU64::new(1),
            operations: Mutex::new(HashMap::new()), metadata: Mutex::new(Value::Null), effective: Mutex::new(Value::Null),
            policy: policy.clone(), idle: StdMutex::new(None),
        });
        Self::pump(Arc::downgrade(&session), events);
        let initialize = session.client.call("runtime.initialize", json!({
            "provider":runtime.provider_id(), "cwd":cwd, "executable":cli, "npm":npm,
            "dependencyRoot":dependencies, "policy":policy, "preview":owner.is_none(),
        }), Duration::from_secs(300)).await;
        if let Err(error) = initialize { session.close().await; return Err(error); }
        match session.call("runtime.describe", json!({})).await {
            Ok(metadata) => *session.metadata.lock().await = metadata,
            Err(error) => { session.close().await; return Err(error); }
        }
        Ok(session)
    }

    fn pump(weak: Weak<Self>, mut events: tokio::sync::mpsc::Receiver<WireEvent>) {
        tokio::spawn(async move {
            while let Some(event) = events.recv().await {
                let Some(session) = weak.upgrade() else { break; };
                match event {
                    WireEvent::Barrier(sender) => { let _ = sender.send(()); }
                    WireEvent::Event { operation, event } => {
                        if session.retired.load(Ordering::SeqCst) { continue; }
                        if event["type"] == "conversation.updated" && event.get("scope").is_none_or(Value::is_null) {
                            if let Some(id) = event["sessionId"].as_str() {
                                if let Err(error) = session.record_identity(id).await {
                                    tracing::error!(%error, "native provider changed conversation ownership");
                                    session.retired.store(true, Ordering::SeqCst);
                                    let failed = session.clone();
                                    tokio::spawn(async move { failed.close().await; });
                                    continue;
                                }
                            }
                        }
                        let projector = session.operations.lock().await.get(&operation).map(|entry| entry.projector.clone());
                        if let Some(projector) = projector {
                            let active = session.operations.lock().await.get(&operation).is_some_and(|entry| !entry.finished);
                            if active && event.get("scope").is_none_or(Value::is_null) {
                                if event["type"] == "conversation.updated" && event["selection"].is_object() {
                                    *session.effective.lock().await = event["selection"].clone();
                                } else if matches!(event["type"].as_str(), Some("model.updated" | "conversation.updated")) && event["model"].is_string() {
                                    session.effective.lock().await["model"] = event["model"].clone();
                                }
                            }
                            if let Err(error) = projector.apply(&event).await {
                                tracing::error!(%error, provider=session.runtime.provider_id(), "native event projection failed");
                                let _ = projector.finish("error").await;
                                if let Some(entry) = session.operations.lock().await.get(&operation) {
                                    entry.cancellation.store(true, Ordering::SeqCst);
                                }
                                session.retired.store(true, Ordering::SeqCst);
                                let failed = session.clone();
                                tokio::spawn(async move { failed.close().await; });
                            }
                            let finished = session.operations.lock().await.get(&operation).is_some_and(|entry| entry.finished);
                            if finished && !projector.has_background_work().await {
                                session.operations.lock().await.remove(&operation);
                            }
                            if finished && !session.active.load(Ordering::SeqCst) {
                                session.generation.fetch_add(1, Ordering::SeqCst);
                                if let Some((owner, _)) = &session.owner {
                                    if let Some(inner) = owner.upgrade() { inner.native_agents.arm_idle(&session); }
                                }
                            }
                        }
                    }
                    WireEvent::Request { id, operation, method, params } => {
                        let projector = session.operations.lock().await.get(&operation).map(|entry| entry.projector.clone());
                        let client = session.client.clone();
                        tokio::spawn(async move {
                            let result = match projector {
                                Some(projector) => projector.interaction(&method, &params).await,
                                None => Err("native interaction has no owning Neoism turn".into()),
                            };
                            client.reply(id, result).await;
                        });
                    }
                    WireEvent::Closed(error) => {
                        session.retired.store(true, Ordering::SeqCst);
                        let operations = session.operations.lock().await.drain().map(|(_, entry)| entry).collect::<Vec<_>>();
                        for entry in operations {
                            let cancelled = entry.cancellation.swap(true, Ordering::SeqCst);
                            let _ = entry.projector.finish(if cancelled { "interrupted" } else { "error" }).await;
                        }
                        tracing::debug!(provider=session.runtime.provider_id(), %error, "native provider host closed");
                        break;
                    }
                }
            }
        });
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        let result = self.client.call(method, params, RPC_TIMEOUT).await?;
        self.client.fence().await?;
        Ok(result)
    }

    async fn open(&self, selection: Value) -> Result<Value, String> {
        if self.opened.load(Ordering::SeqCst) { return Ok(json!({"sessionId":self.native_id.lock().await.clone(),"selection":self.effective.lock().await.clone()})); }
        let requested = self.native_id.lock().await.clone();
        let result = self.call("conversation.open", json!({"sessionId":requested,"selection":selection})).await?;
        if let Some(id) = result["sessionId"].as_str() {
            self.record_identity(id).await?;
        } else if requested.is_some() || self.runtime != ExternalRuntime::Claude {
            return Err("native provider did not confirm the requested conversation".into());
        }
        *self.effective.lock().await = result.clone();
        self.opened.store(true, Ordering::SeqCst);
        Ok(result)
    }

    async fn record_identity(&self, id: &str) -> Result<(), String> {
        if id.trim().is_empty() { return Err("native conversation ID is empty".into()); }
        {
            let mut native = self.native_id.lock().await;
            if native.as_deref().is_some_and(|previous| previous != id) {
                return Err("native provider returned a different conversation; refusing silent replacement".into());
            }
            *native = Some(id.to_string());
        }
        let Some((state, owner)) = &self.owner else { return Ok(()); };
        let Some(inner) = state.upgrade() else { return Ok(()); };
        let state = AppState { inner };
        let Some(mut session) = state.inner.store.get_session(owner).await.map_err(|error| error.to_string())? else { return Ok(()); };
        let extra = session.extra.get_mut("externalAgent").ok_or("native conversation lost its owner")?;
        if extra["runtime"] != self.runtime.native_driver_id() || extra["provider"] != self.runtime.provider_id() {
            return Err("native conversation owner changed".into());
        }
        if extra["nativeSessionId"] == id && extra["externalSessionId"] == id { return Ok(()); }
        extra["nativeSessionId"] = json!(id);
        extra["externalSessionId"] = json!(id);
        state.inner.store.update_session(&session).await.map_err(|error| error.to_string())?;
        state.publish(EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID":session.id,"info":session})));
        Ok(())
    }

    async fn has_background_work(&self) -> bool {
        let projectors = self.operations.lock().await.values().map(|entry| entry.projector.clone()).collect::<Vec<_>>();
        for projector in projectors { if projector.has_background_work().await { return true; } }
        false
    }

    async fn close(&self) {
        self.retired.store(true, Ordering::SeqCst);
        let operations = self.operations.lock().await.values().map(|entry| (entry.cancellation.clone(),entry.projector.clone())).collect::<Vec<_>>();
        for (cancellation, _) in &operations { cancellation.store(true, Ordering::SeqCst); }
        self.client.close().await;
        for (_, projector) in operations { let _ = tokio::time::timeout(Duration::from_secs(2), projector.finish("interrupted")).await; }
        self.operations.lock().await.clear();
    }
}

fn executable(state: &AppState, name: &str, required: bool) -> Result<Option<String>, String> {
    let request = neoism_agent_service_api::ExecutableRequest::new(name, neoism_agent_service_api::ExecutablePurpose::ExternalAgent);
    match state.services().executables.resolve(&request) {
        Ok(result) => Ok(Some(result.path.to_string_lossy().into_owned())),
        Err(error) if required => Err(format!("Native {} executable is unavailable: {error}", name)),
        Err(_) => Ok(None),
    }
}

fn native_id(session: &SessionInfo) -> Option<String> {
    let extra = session.extra.get("externalAgent")?;
    extra.get("nativeSessionId").or_else(|| extra.get("externalSessionId"))?.as_str()
        .filter(|id| !id.is_empty()).map(str::to_string)
}

fn native_policy(rules: &[PermissionRule]) -> Value {
    let denied = rules.iter()
        .filter(|rule| rule.action == PermissionAction::Deny && rule.pattern == "*")
        .map(|rule| rule.permission.clone()).collect::<Vec<_>>();
    let read_only = denied.iter().any(|name| matches!(name.as_str(), "*" | "edit" | "write" | "apply_patch"));
    json!({"approvalPolicy":"untrusted","sandbox":if read_only {"read-only"} else {"workspace-write"},
        "permissionMode":"default","deniedTools":denied,
        "deniedRules":rules.iter().filter(|rule| rule.action == PermissionAction::Deny).collect::<Vec<_>>()})
}

async fn current_permissions(state: &AppState, session_id: &str) -> Result<Vec<PermissionRule>, String> {
    let mut rules = Vec::new();
    let mut id = Some(session_id.to_string());
    let mut seen = BTreeSet::new();
    let mut tenant = None;
    while let Some(current) = id {
        if seen.len() >= 64 || !seen.insert(current.clone()) { return Err("invalid native permission ancestry".into()); }
        let session = state.inner.store.get_session(&current).await.map_err(|error| error.to_string())?.ok_or("native permission owner disappeared")?;
        let owner = crate::caller::session_tenant(&session).to_string();
        if tenant.as_ref().is_some_and(|tenant| tenant != &owner) { return Err("native permission ancestry crosses tenants".into()); }
        tenant = Some(owner);
        rules.extend(super::requests::external_effective_permissions_for_session(state, &current).await.map_err(|error| error.message)?);
        id = session.parent_id.map(|id| id.to_string());
    }
    // A child or project approval must not relax a current ancestor restriction.
    rules.sort_by_key(|rule| match rule.action { PermissionAction::Allow => 0, PermissionAction::Ask => 1, PermissionAction::Deny => 2 });
    Ok(rules)
}

pub(super) fn selection(choices: &Value, description: &Value) -> Result<Value, String> {
    let choices = choices.as_object().ok_or("native selection must be an object")?;
    let mut options = serde_json::Map::new();
    let mut model = None;
    for (id, value) in choices {
        if id == "model" { model = Some(value.as_str().filter(|value| !value.is_empty()).ok_or("native model must be a nonempty string")?.to_string()); continue; }
        let descriptor = description["options"].as_array().and_then(|options| options.iter().find(|option| option["id"] == id.as_str()))
            .ok_or_else(|| format!("Unsupported native provider option: {id}"))?;
        let value = if descriptor["type"] == "boolean" {
            match value.as_bool().or_else(|| value.as_str().and_then(|text| text.parse::<bool>().ok())) {
                Some(value) => json!(value), None => return Err(format!("Native option {id} requires a boolean")),
            }
        } else if descriptor["type"] == "select" {
            if !descriptor["options"].as_array().is_some_and(|choices| choices.iter().any(|choice| choice["value"] == *value)) {
                return Err(format!("Unsupported native provider option value: {id}={value}"));
            }
            value.clone()
        } else { return Err(format!("Unsupported native provider option type: {id}")); };
        options.insert(id.clone(), value);
    }
    if let Some(model) = &model {
        if !description["models"].as_array().is_some_and(|models| models.iter().any(|candidate| candidate["id"] == model.as_str())) {
            return Err(format!("Unsupported native provider model: {model}"));
        }
    }
    Ok(json!({"model":model,"options":options}))
}

pub(crate) async fn run_native_prompt(
    state: &AppState, child: &SessionInfo, prompt: &str, runtime: ExternalRuntime,
    step: &StartedAssistantStep, model: &UserModel, cancellation: Arc<AtomicBool>,
) -> Result<ExternalRunResult, String> {
    let gate = super::options::session_lock(state, child.id.as_str()).await;
    let _guard = gate.lock().await;
    let child = state.inner.store.get_session(child.id.as_str()).await.map_err(|error| error.to_string())?.ok_or("native session disappeared")?;
    if !runtime.owns_session(&child) || runtime.is_legacy_session(&child) { return Err("native provider driver ownership mismatch".into()); }
    if cancellation.load(Ordering::SeqCst) { return Err("Session aborted".into()); }
    let connection = state.inner.native_agents.acquire(state, runtime, &child.directory, Some(&child)).await?;
    if cancellation.load(Ordering::SeqCst) { connection.close().await; return Err("Session aborted".into()); }
    connection.active.store(true, Ordering::SeqCst);
    let result = async {
        let choices = child.extra.get("externalAgent").and_then(|extra| extra.get("nativeOptions")).cloned().unwrap_or_else(|| json!({}));
        if choices.get("model").is_some() {
            let description = connection.call("runtime.describe", json!({"selection":{"model":choices["model"]}})).await?;
            *connection.metadata.lock().await = description;
        }
        let selected = selection(&choices, &*connection.metadata.lock().await)?;
        connection.open(selected.clone()).await?;
        let projector = NativeProjector::new(state, &child, runtime, step, cancellation.clone());
        let call = connection.client.reserve().await?;
        let operation = call.id.to_string();
        connection.operations.lock().await.insert(operation.clone(), Operation { projector: projector.clone(), cancellation: cancellation.clone(), finished: false });
        if let Err(error) = connection.client.dispatch(&call, "turn.start", json!({"sessionId":connection.native_id.lock().await.clone(),"text":prompt,"selection":selected})).await {
            connection.operations.lock().await.remove(&operation);
            return Err(error);
        }
        let wait = call.wait(Duration::from_secs(60 * 60));
        tokio::pin!(wait);
        let result = tokio::select! {
            biased;
            _ = super::wait_for_cancel(cancellation.clone()) => {
                let stopped = connection.client.call("turn.interrupt", json!({"sessionId":connection.native_id.lock().await.clone()}), Duration::from_secs(15)).await;
                if stopped.is_err() { connection.close().await; }
                Err("Session aborted".to_string())
            }
            result = &mut wait => result,
        };
        let result = match connection.client.fence().await {
            Ok(()) => result,
            Err(error) => Err(error),
        };
        if result.is_err() { connection.close().await; }
        let status = if cancellation.load(Ordering::SeqCst) || result.as_ref().is_ok_and(|value| matches!(value["finish"].as_str(), Some("cancelled" | "interrupted"))) { "interrupted" } else if result.is_ok() { "completed" } else { "error" };
        projector.finish(status).await?;
        if let Some(entry) = connection.operations.lock().await.get_mut(&operation) { entry.finished = true; }
        if !projector.has_background_work().await { connection.operations.lock().await.remove(&operation); }
        let result = result?;
        if matches!(result["finish"].as_str(), Some("cancelled" | "interrupted")) { return Err("Native provider cancelled the turn".into()); }
        if let Some(mut current) = state.inner.store.get_session(child.id.as_str()).await.map_err(|error| error.to_string())? {
            if let Some(external) = current.extra.get_mut("externalAgent") {
                if !external["pendingSelection"].is_null() {
                    external["pendingSelection"] = Value::Null;
                    state.inner.store.update_session(&current).await.map_err(|error| error.to_string())?;
                    state.publish(EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID":current.id,"info":current})));
                }
            }
        }
        Ok(ExternalRunResult { provider_response: projector.response(model, &result).await })
    }.await;
    connection.active.store(false, Ordering::SeqCst);
    if result.is_err() { connection.close().await; } else { state.inner.native_agents.arm_idle(&connection); }
    result
}

pub(crate) async fn request(state: &AppState, runtime: ExternalRuntime, directory: &str, owner: Option<&SessionInfo>, method: &str, mut params: Value) -> Result<Value, String> {
    if state.services().shared_control_plane() || !crate::caller::native_execution_allowed(&crate::workspace_runtime::directory_execution_policy(state.services(), Path::new(directory))) {
        return Err("native provider execution is not authorized for this directory".into());
    }
    if !matches!(method, "runtime.describe" | "conversation.open" | "selection.apply" | "conversation.list" | "conversation.history") {
        return Err("unsupported native provider management operation".into());
    }
    let gate = if let Some(owner) = owner { Some(super::options::session_lock(state, owner.id.as_str()).await) } else { None };
    let _guard = if let Some(gate) = &gate { Some(gate.lock().await) } else { None };
    let current = if let Some(owner) = owner {
        Some(state.inner.store.get_session(owner.id.as_str()).await.map_err(|error| error.to_string())?.ok_or("native session disappeared")?)
    } else { None };
    let owner = current.as_ref();
    let connection = state.inner.native_agents.acquire(state, runtime, directory, owner).await?;
    connection.active.store(true, Ordering::SeqCst);
    let result = async {
        if method == "conversation.open" { return connection.open(params.get("selection").cloned().unwrap_or_else(|| json!({}))).await; }
        if method == "selection.apply" {
            let mut choices = owner.and_then(|owner| owner.extra.get("externalAgent")).and_then(|external| external["nativeOptions"].as_object()).cloned().unwrap_or_default();
            choices.extend(params["selection"]["options"].as_object().cloned().unwrap_or_default());
            if let Some(model) = params["selection"].get("model") { choices.insert("model".into(), model.clone()); }
            params["selection"] = selection(&Value::Object(choices), &*connection.metadata.lock().await)?;
            connection.open(json!({})).await?;
            params["sessionId"] = json!(connection.native_id.lock().await.clone());
        }
        let requested = params.get("selection").cloned();
        let mut value = connection.call(method, params).await?;
        if method == "selection.apply" && value["applied"] != "next-turn" {
            *connection.effective.lock().await = value.get("selection").cloned().unwrap_or_else(|| value.clone());
        }
        if method == "selection.apply" {
            if let (Some(owner), Some(requested)) = (owner, requested) {
                let mut current = state.inner.store.get_session(owner.id.as_str()).await.map_err(|error| error.to_string())?.ok_or("native selection owner disappeared")?;
                let mut options: BTreeMap<String, String> = requested["options"].as_object().into_iter().flatten().map(|(key, value)|
                    (key.clone(), value.as_str().map(str::to_owned).unwrap_or_else(|| value.to_string()))).collect();
                if let Some(model) = requested["model"].as_str() { options.insert("model".into(), model.into()); }
                let external = current.extra.get_mut("externalAgent").ok_or("native selection owner has no metadata")?;
                external["nativeOptions"] = json!(options);
                external["pendingSelection"] = if value["applied"] == "next-turn" { requested } else { Value::Null };
                state.inner.store.update_session(&current).await.map_err(|error| error.to_string())?;
                state.publish(EventPayload::new(event_type::SESSION_UPDATED, json!({"sessionID":current.id,"info":current})));
            }
        }
        if method == "runtime.describe" {
            *connection.metadata.lock().await = value.clone();
            value["sessionId"] = json!(connection.native_id.lock().await.clone());
            value["effectiveSelection"] = connection.effective.lock().await.clone();
            if let Some(owner) = owner {
                value["pendingSelection"] = owner.extra["externalAgent"]["pendingSelection"].clone();
                if !connection.opened.load(Ordering::SeqCst) {
                    let mut choices = owner.extra["externalAgent"]["nativeOptions"].as_object().cloned().unwrap_or_default();
                    if !choices.is_empty() {
                        let model = choices.remove("model");
                        value["pendingSelection"] = json!({"model":model,"options":choices});
                    }
                }
            }
        }
        Ok(value)
    }.await;
    connection.active.store(false, Ordering::SeqCst);
    if owner.is_none() || result.is_err() { connection.close().await; } else { state.inner.native_agents.arm_idle(&connection); }
    result
}

pub(crate) async fn close_session(state: &AppState, session_id: &str) -> bool {
    let entries = {
        let mut sessions = state.inner.native_agents.sessions.lock().await;
        let keys = sessions.iter().filter(|(_, value)| value.owner.as_ref().is_some_and(|(_, id)| id == session_id)).map(|(key, _)| key.clone()).collect::<Vec<_>>();
        keys.into_iter().filter_map(|key| sessions.remove(&key)).collect::<Vec<_>>()
    };
    let closed = !entries.is_empty();
    for entry in entries { entry.close().await; }
    closed
}

pub(crate) async fn import_history(state: &AppState, runtime: ExternalRuntime, session: &SessionInfo, native_id: &str) -> Result<usize, String> {
    let history = request(state, runtime, &session.directory, None, "conversation.history", json!({"sessionId":native_id,"cwd":session.directory})).await?;
    if history["complete"] != true { return Err("Provider cannot supply complete native history; import was not performed".into()); }
    let reported = history["cwd"].as_str().ok_or("native history did not report its workspace")?;
    let actual = crate::windows_process::canonicalize_path(Path::new(reported)).map_err(|error| error.to_string())?;
    let expected = crate::windows_process::canonicalize_path(Path::new(&session.directory)).map_err(|error| error.to_string())?;
    if actual != expected || history["sessionId"] != native_id { return Err("Native history belongs to a different conversation or workspace".into()); }
    let messages = history["messages"].as_array().ok_or("native history has no messages")?;
    for message in messages {
        if !matches!(message["role"].as_str(), Some("user" | "assistant")) || !message["events"].is_array() {
            return Err("native history contains an unsupported message".into());
        }
    }
    if !messages.iter().any(|message| message["role"] == "user" && message["events"].as_array().is_some_and(|events|
        events.iter().any(|event| matches!(event["type"].as_str(), Some("text.delta" | "text.completed")) && event["text"].as_str().is_some_and(|text| !text.is_empty())))) {
        return Err("native history contains no usable user message".into());
    }
    let model = external_model(runtime);
    let mut parent = Id::ascending(IdKind::Message);
    for message in messages {
        let events = message["events"].as_array().ok_or("native history message has no events")?;
        if message["role"] == "user" {
            let text = events.iter().filter(|event| matches!(event["type"].as_str(), Some("text.delta" | "text.completed"))).filter_map(|event| event["text"].as_str()).collect::<String>();
            parent = Id::ascending(IdKind::Message);
            super::append_external_user_message(state, session, parent.clone(), &text, runtime, &model, None).await.map_err(|error| error.to_string())?;
        } else if message["role"] == "assistant" {
            let step = start_assistant_step(state, &session.id, session.id.as_str(), &parent, &session.directory, now_millis(), runtime.agent_name().into(), runtime.agent_name().into(), model.model_id.clone(), model.provider_id.clone(), false).await.map_err(|error| error.to_string())?;
            let projector = NativeProjector::new(state, session, runtime, &step, Arc::new(AtomicBool::new(false)));
            for event in events { projector.apply(event).await?; }
            projector.finish("completed").await?;
            let response = projector.response(&model, &json!({"finish":"imported"})).await;
            finish_provider_stream_success(state, &session.id, session.id.as_str(), &step.assistant_id, &step.text_part_id, &step.live_message, &model, response, Default::default(), true).await.map_err(|error| error.to_string())?;
        }
    }
    Ok(messages.len())
}

async fn prepare_runtime() -> Result<(PathBuf, PathBuf), String> {
    const FILES: &[(&str, &str)] = &[
        ("host.mjs", include_str!("native_runtime/host.mjs")),
        ("codex.mjs", include_str!("native_runtime/codex.mjs")),
        ("json-rpc.mjs", include_str!("native_runtime/json-rpc.mjs")),
        ("claude.mjs", include_str!("native_runtime/claude.mjs")),
        ("opencode.mjs", include_str!("native_runtime/opencode.mjs")),
    ];
    const MANIFEST: &str = include_str!("native_runtime/package.json");
    const LOCK: &str = include_str!("native_runtime/package-lock.json");
    let mut hash = Sha256::new();
    for (name, text) in FILES { hash.update(name.as_bytes()); hash.update(text.as_bytes()); }
    let root = neoism_agent_builtins::default_cache_dir().join("native-agents").join(format!("{:x}", hash.finalize()));
    let dependencies = neoism_agent_builtins::default_cache_dir().join("native-agent-deps").join(format!("{:x}", Sha256::digest(LOCK.as_bytes())));
    for directory in [&root, &dependencies] { tokio::fs::create_dir_all(directory).await.map_err(|error| error.to_string())?; }
    for (name, text) in FILES { write_managed_file(&root.join(name), text.as_bytes()).await?; }
    write_managed_file(&dependencies.join("package.json"), MANIFEST.as_bytes()).await?;
    write_managed_file(&dependencies.join("package-lock.json"), LOCK.as_bytes()).await?;
    Ok((root.join("host.mjs"), dependencies))
}

async fn write_managed_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match tokio::fs::read(path).await {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => return Err(format!("Managed native runtime was modified: {}", path.display())),
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.to_string()),
        Err(_) => (),
    }
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), Id::ascending(IdKind::Event)));
    tokio::fs::write(&temporary, bytes).await.map_err(|error| error.to_string())?;
    tokio::fs::rename(&temporary, path).await.map_err(|error| error.to_string())
}