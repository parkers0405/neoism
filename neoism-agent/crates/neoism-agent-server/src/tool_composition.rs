//! Server authority bridge. The interpreter has no ambient filesystem/process/network.
use crate::{
    state::AppState, tool::ToolExecutionResult, workspace_runtime::PluginGenerationLease,
};
use futures::future::BoxFuture;
use neoism_agent_composition::{ResultAlias, ToolDescriptor, ToolHost};
use neoism_agent_core::{
    event_type, EventPayload, Id, IdKind, MessageInfo, MessageWithParts, Part,
    PermissionRule, ToolListItem,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Weak,
};
use tokio::sync::{Mutex, Semaphore};

pub(crate) const DESCRIPTION: &str = "Compose multiple tools with bounded JavaScript. Discover synchronously with search({query: 'read', namespace: 'neoism', limit: 10, offset: 0}) (fields optional), then await tools.neoism.read({filePath: ...}) for native tools or await tools.github.list_issues({...}) for MCP. Return only the selected value; console.log emits bounded logs. Supported bounded JavaScript subset: no template literals, destructuring, spread, new, or imports; throw a string rather than new Error(...). No ambient filesystem, process or network. Each call returns {title,output,metadata,isError}; output is formatted text, NOT JSON data. Only actual MCP metadata.mcp.result.structuredContent is also exposed as structuredContent/data. Tool errors are catchable; effects/media remain recorded even if the script fails. Excluded: compose, execute, task, task_result, stop_task, question, plan_enter, plan_exit, complete_goal, move_chat, background_task, background_task_result. These remain available as direct tools.";
const EXCLUDED: &[&str] = &[
    "compose",
    "execute",
    "task",
    "task_result",
    "stop_task",
    "question",
    "plan_enter",
    "plan_exit",
    "complete_goal",
    "move_chat",
    "background_task",
    "background_task_result",
];

#[derive(Clone, Default)]
pub(crate) struct CallScope {
    pub cancel: Option<Arc<AtomicBool>>,
    pub progress: Option<ProgressSink>,
    // Defer snapshot externalization until effects are aggregated at the root.
    pub child: bool,
    pub validation: Option<(Arc<jsonschema::Validator>, Option<String>)>,
    pub effects: Option<EffectSink>,
}
#[derive(Clone)]
pub(crate) struct ProgressSink {
    pub message: Arc<Mutex<MessageWithParts>>,
    pub part_id: String,
}
impl ProgressSink {
    async fn publish(&self, state: &AppState, calls: &[Value], code: Option<&str>) {
        let part = {
            let mut message = self.message.lock().await;
            message.parts.iter_mut().find_map(|part| {
                let Part::Tool(tool) = part else { return None };
                if tool.id.as_str() != self.part_id {
                    return None;
                }
                let metadata = tool.metadata.get_or_insert_with(|| json!({}));
                if matches!(
                    tool.state,
                    neoism_agent_core::ToolState::Completed { .. }
                        | neoism_agent_core::ToolState::Error { .. }
                ) {
                    return None;
                }
                let mut merged = metadata
                    .pointer("/composition/toolCalls")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                for call in calls {
                    if let Some(existing) =
                        merged.iter_mut().find(|item| item["id"] == call["id"])
                    {
                        if existing["status"] == "running" || call["status"] != "running"
                        {
                            *existing = call.clone();
                        }
                    } else {
                        merged.push(call.clone());
                    }
                }
                if !metadata.is_object() {
                    *metadata = json!({});
                }
                if !metadata["composition"].is_object() {
                    metadata["composition"] = json!({});
                }
                metadata["composition"]["toolCalls"] = json!(merged);
                if let Some(code) = code {
                    metadata["composition"]["code"] = json!(code);
                }
                let session_id = tool.session_id.clone();
                Some((session_id, part.clone()))
            })
        };
        if let Some((session_id, part)) = part {
            state.publish_live(EventPayload::new(event_type::MESSAGE_PART_UPDATED,
                json!({"sessionID": session_id, "part": part, "time": crate::now_millis()})));
        }
    }
}

// Scope-owned propagation task, always aborted on drop. Never abort the session
// when the composition's wall-clock budget expires.
pub(crate) struct CancellationBridge {
    flag: Option<Arc<AtomicBool>>,
    task: Option<tokio::task::JoinHandle<()>>,
}
impl CancellationBridge {
    pub fn new(local: Option<Arc<AtomicBool>>, session: Option<Arc<AtomicBool>>) -> Self {
        match (local, session) {
            (Some(local), Some(session)) if !Arc::ptr_eq(&local, &session) => {
                if session.load(Ordering::SeqCst) {
                    local.store(true, Ordering::SeqCst);
                }
                let flag = local.clone();
                let task = tokio::spawn(async move {
                    crate::session_loop::wait_for_cancellation(session).await;
                    local.store(true, Ordering::SeqCst);
                });
                Self {
                    flag: Some(flag),
                    task: Some(task),
                }
            }
            (local, session) => Self {
                flag: local.or(session),
                task: None,
            },
        }
    }
    pub fn flag(&self) -> Option<Arc<AtomicBool>> {
        self.flag.clone()
    }
}
impl Drop for CancellationBridge {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

struct Entry {
    descriptor: ToolDescriptor,
    dispatch: String,
    mcp: bool,
    validator: Arc<jsonschema::Validator>,
}
#[derive(Default)]
struct Audit {
    calls: Vec<Value>,
    effects: Vec<(String, Value)>,
    presentation_bytes: usize,
    sealed: bool,
}

#[derive(Clone)]
pub(crate) struct EffectSink {
    audit: Arc<Mutex<Audit>>,
}
impl EffectSink {
    pub(crate) async fn record(&self, call_id: &str, metadata: Option<&Value>) {
        let Some(metadata) = metadata else { return };
        let metadata = retained_effect_metadata(metadata);
        if metadata.as_object().is_none_or(|object| object.is_empty()) {
            return;
        }
        let mut audit = self.audit.lock().await;
        if audit.sealed {
            return;
        }
        if let Some((_, previous)) =
            audit.effects.iter_mut().find(|(id, _)| id == call_id)
        {
            *previous = metadata;
        } else {
            audit.effects.push((call_id.to_string(), metadata));
        }
    }
}

/// One supervised task per leaf. The host only holds a Weak reference, so
/// dropping the owning composition always aborts outstanding tasks (no cycle).
struct AbortWorker(tokio::task::JoinHandle<()>);
impl Drop for AbortWorker {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct TrackedWorker {
    worker: AbortWorker,
    record: Value,
}
#[derive(Default)]
struct WorkerSupervisor {
    workers: std::sync::Mutex<BTreeMap<String, TrackedWorker>>,
}
impl WorkerSupervisor {
    async fn cleanup(&self, grace: std::time::Duration) -> Vec<Value> {
        use futures::StreamExt;
        let workers = std::mem::take(&mut *self.workers.lock().unwrap());
        let mut aborts = BTreeMap::new();
        let mut pending = futures::stream::FuturesUnordered::new();
        for (id, mut tracked) in workers {
            aborts.insert(
                id.clone(),
                (tracked.worker.0.abort_handle(), tracked.record.clone()),
            );
            pending.push(async move {
                (id, (&mut tracked.worker.0).await, tracked.record.clone())
            });
        }
        let mut interrupted = Vec::new();
        let _ = tokio::time::timeout(grace, async {
            while let Some((id, result, record)) = pending.next().await {
                aborts.remove(&id);
                if result.is_err() {
                    interrupted.push(record);
                }
            }
        })
        .await;
        // Explicitly abort before dropping any unjoined handles. Aborting an
        // async task does not pretend to undo already-dispatched external effects.
        for (_, (abort, record)) in aborts {
            abort.abort();
            interrupted.push(record);
        }
        interrupted
    }
}
#[derive(Clone)]
struct Host {
    state: AppState,
    session: Id,
    message: Id,
    directory: String,
    tenant_id: String,
    permissions: Vec<PermissionRule>,
    snapshot: PluginGenerationLease,
    entries: Arc<BTreeMap<String, Entry>>,
    audit: Arc<Mutex<Audit>>,
    progress: Option<ProgressSink>,
    budget: Arc<Semaphore>,
    supervisor: Weak<WorkerSupervisor>,
}
const MCP_RESULT_ALIASES: &[ResultAlias] = &[
    ResultAlias {
        name: "structuredContent",
        path: &["metadata", "mcp", "result", "structuredContent"],
    },
    ResultAlias {
        name: "data",
        path: &["metadata", "mcp", "result", "structuredContent"],
    },
];

impl ToolHost for Host {
    fn result_aliases(&self, path: &str) -> &'static [ResultAlias] {
        if self.entries.get(path).is_some_and(|entry| entry.mcp) {
            MCP_RESULT_ALIASES
        } else {
            &[]
        }
    }

    fn catalog(&self) -> Vec<ToolDescriptor> {
        self.entries
            .values()
            .map(|entry| entry.descriptor.clone())
            .collect()
    }
    fn call(
        &self,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
    ) -> BoxFuture<'static, Result<Value, String>> {
        let host = self.clone();
        let Some(supervisor) = self.supervisor.upgrade() else {
            return Box::pin(async { Err("composition supervisor closed".into()) });
        };
        let id = Id::ascending(IdKind::Part).to_string();
        let mut workers = supervisor.workers.lock().unwrap();
        if workers.len() >= 64 {
            return Box::pin(async {
                Err("composition child call budget exceeded".into())
            });
        }
        let record =
            json!({"id":id,"path":path,"input":bounded_input(&input),"status":"running"});
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let worker_id = id.clone();
        let worker_record = record.clone();
        let worker = tokio::spawn(async move {
            let result = host
                .child(worker_id, path, input, cancel, worker_record)
                .await;
            let _ = sender.send(result);
        });
        workers.insert(
            id,
            TrackedWorker {
                worker: AbortWorker(worker),
                record,
            },
        );
        Box::pin(async move {
            receiver.await.map_err(|_| {
                "composition child interrupted; effects may be partial_unknown"
                    .to_string()
            })?
        })
    }
}
impl Host {
    async fn child(
        &self,
        id: String,
        path: String,
        input: Value,
        cancel: Arc<AtomicBool>,
        record: Value,
    ) -> Result<Value, String> {
        let (index, calls) = {
            let mut audit = self.audit.lock().await;
            if audit.sealed || audit.calls.len() >= 64 {
                return Err("composition child call budget closed".into());
            }
            let index = audit.calls.len();
            audit.calls.push(record);
            (index, self.progress.as_ref().map(|_| audit.calls.clone()))
        };
        if let (Some(progress), Some(calls)) = (&self.progress, calls) {
            progress.publish(&self.state, &calls, None).await;
        }
        let result = async {
            let entry = self.entries.get(&path).ok_or_else(|| format!("tool is not in the captured eligible catalog: {path}"))?;
            if let Err(error) = entry.validator.validate(&input) { return Err(format!("invalid input for {path}: {error}")); }
            let _permit = tokio::select! {
                permit = self.budget.acquire() => permit.map_err(|_| "composition scheduler closed".to_string())?,
                _ = crate::session_loop::wait_for_cancellation(cancel.clone()) => return Err("composition child aborted".into()),
            };
            if cancel.load(Ordering::SeqCst) { return Err("composition child aborted".into()); }
            let (name, arguments) = if entry.mcp {
                ("execute", json!({"action":"call","tool":entry.dispatch,"arguments":input}))
            } else { (entry.dispatch.as_str(), input) };
            let result = crate::tool_runtime::execute_tool_call_scoped(
                &self.state, &self.session, &self.message, &self.directory,
                self.permissions.clone(), &id, name, arguments, self.snapshot.clone(),
                CallScope { cancel: Some(cancel), progress: None, child: true,
                    effects: Some(EffectSink {audit:self.audit.clone()}),
                    validation: Some((entry.validator.clone(), entry.mcp.then(|| entry.dispatch.clone()))) }).await?;
            {
                let mut audit = self.audit.lock().await;
                if !audit.sealed {
                    let remaining = PRESENTATION_BYTES.saturating_sub(audit.presentation_bytes);
                    if let Some((presentation, bytes)) = child_presentation(name, &result, remaining) {
                        audit.presentation_bytes += bytes;
                        audit.calls[index]["presentation"] = presentation;
                    } else {
                        audit.calls[index]["detailsTruncated"] = json!(true);
                    }
                }
            }
            // Account effects before turning protocol isError into a catchable failure.
            let mut effects = result.metadata.as_ref().map(retained_effect_metadata);
            let mut hydration_error = None;
            if let Some(artifact_id) = effects.as_ref().and_then(|m| m.pointer("/snapshotArtifact/id")).and_then(Value::as_str).map(str::to_string) {
                match crate::snapshot::hydrate_snapshot_artifact(&self.state, &self.tenant_id,
                    self.session.as_str(), &artifact_id).await {
                    Ok(snapshots) => {
                        let metadata = effects.as_mut().unwrap();
                        metadata.as_object_mut().unwrap().remove("snapshotArtifact");
                        let items = metadata.as_object_mut().unwrap().entry("snapshots").or_insert_with(|| json!([]));
                        items.as_array_mut().unwrap().extend(snapshots.into_iter().map(|s| json!(s)));
                    }
                    Err(error) => hydration_error = Some(format!("snapshot artifact hydration failed: {error}")),
                }
            }
            EffectSink {audit:self.audit.clone()}.record(&id, effects.as_ref()).await;
            if let Some(error) = hydration_error { return Err(error); }
            let envelope = envelope(&result);
            if envelope["isError"] == true { return Err(format!("{path}: {}", result.output)); }
            Ok(envelope)
        }.await;
        let calls = {
            let mut audit = self.audit.lock().await;
            if audit.sealed {
                return Err("composition child interrupted".into());
            }
            audit.calls[index]["status"] =
                json!(if result.is_ok() { "completed" } else { "error" });
            if let Err(error) = &result {
                audit.calls[index]["error"] =
                    json!(error.chars().take(4096).collect::<String>());
            }
            self.progress.as_ref().map(|_| audit.calls.clone())
        };
        if let (Some(progress), Some(calls)) = (&self.progress, calls) {
            progress.publish(&self.state, &calls, None).await;
        }
        result
    }
}
// Streaming JSON counting stops at the cap without materializing the input.
struct CountWriter {
    bytes: usize,
    limit: usize,
}
impl std::io::Write for CountWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("bounded JSON size exceeded"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn serialized_size(value: &impl serde::Serialize, limit: usize) -> Result<usize, String> {
    let mut writer = CountWriter { bytes: 0, limit };
    serde_json::to_writer(&mut writer, value)
        .map_err(|_| "composition JSON size limit exceeded".to_string())?;
    Ok(writer.bytes)
}
fn bounded_input(input: &Value) -> Value {
    if serialized_size(input, 4096).is_ok() {
        input.clone()
    } else {
        json!({"omitted":true,"reason":"audit input exceeds 4096 bytes"})
    }
}
const PRESENTATION_BYTES: usize = 512 * 1024;

fn text_prefix(text: &str, limit: usize) -> &str {
    let mut end = text.len().min(limit);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// Child presentation is UI-only. Retain display fields, never the raw MCP
// protocol body, and admit the serialized payload against a separate run budget.
fn child_presentation(
    tool: &str,
    result: &ToolExecutionResult,
    remaining: usize,
) -> Option<(Value, usize)> {
    if remaining < 256 {
        return None;
    }
    let title = text_prefix(&result.title, 1024);
    let output = text_prefix(&result.output, 16 * 1024);
    let mut truncated =
        title.len() != result.title.len() || output.len() != result.output.len();
    let mut metadata = serde_json::Map::new();
    let mut metadata_bytes = 0usize;
    if let Some(source) = result.metadata.as_ref() {
        if let Some(mcp) = source.get("mcp") {
            let mut identity = serde_json::Map::new();
            for key in ["client", "tool", "runtimeId"] {
                if let Some(value) = mcp.get(key) {
                    if serialized_size(value, 1024).is_ok() {
                        identity.insert(key.to_string(), value.clone());
                    } else {
                        truncated = true;
                    }
                }
            }
            if !identity.is_empty() {
                metadata.insert("mcp".into(), Value::Object(identity));
                metadata_bytes = serialized_size(&metadata, 16 * 1024).ok()?;
            }
        }
        for key in [
            "artifact",
            "outputPath",
            "truncated",
            "files",
            "diff",
            "diffs",
            "attachments",
            "media",
            "generatedFiles",
            "lang",
            "language",
            "lineOffset",
        ] {
            if let Some(value) = source.get(key) {
                let room =
                    (16 * 1024usize).saturating_sub(metadata_bytes + key.len() + 8);
                match serialized_size(value, room) {
                    Ok(bytes) => {
                        metadata_bytes += bytes + key.len() + 8;
                        metadata.insert(key.to_string(), value.clone());
                    }
                    Err(_) => truncated = true,
                }
            }
        }
    }
    let mut presentation = json!({
        "tool": tool, "title": title, "output": output,
        "metadata": metadata, "isError": result.is_error(), "truncated": truncated,
    });
    loop {
        if let Ok(bytes) = serialized_size(&presentation, remaining) {
            return Some((presentation, bytes));
        }
        presentation["truncated"] = json!(true);
        let current = presentation["output"].as_str().unwrap_or_default();
        if !current.is_empty() {
            presentation["output"] = json!(text_prefix(current, current.len() / 2));
        } else if presentation["metadata"]
            .as_object()
            .is_some_and(|m| !m.is_empty())
        {
            presentation["metadata"] = json!({});
        } else if !presentation["title"]
            .as_str()
            .unwrap_or_default()
            .is_empty()
        {
            presentation["title"] = json!("");
        } else {
            return None;
        }
    }
}

fn envelope(result: &ToolExecutionResult) -> Value {
    // The interpreter attaches aliases to its arena after one import. Keep one
    // canonical payload here rather than cloning structured MCP data three times.
    json!({"title":result.title,"output":result.output,
        "metadata":result.metadata,"isError":result.metadata.as_ref().is_some_and(|m| m["isError"] == true)})
}
// The effect ledger must never retain full MCP protocol bodies or unrelated
// result metadata outside the interpreter's own data accounting.
fn retained_effect_metadata(metadata: &Value) -> Value {
    let mut effects = serde_json::Map::new();
    for key in [
        "snapshotArtifact",
        "snapshotArtifacts",
        "snapshots",
        "diff",
        "diffs",
        "files",
        "attachments",
        "media",
        "generatedFiles",
    ] {
        if let Some(value) = metadata.get(key) {
            effects.insert(key.to_string(), value.clone());
        }
    }
    Value::Object(effects)
}

fn merge_effects(root: &mut Value, child: Option<&Value>) {
    if !root.is_object() {
        *root = json!({});
    }
    let Some(child) = child else { return };
    if let Some(artifact) = child.get("snapshotArtifact") {
        root.as_object_mut()
            .unwrap()
            .entry("snapshotArtifacts")
            .or_insert_with(|| json!([]))
            .as_array_mut()
            .unwrap()
            .push(artifact.clone());
    }
    if let Some(diff) = child.get("diff").and_then(Value::as_str) {
        let target = root
            .as_object_mut()
            .unwrap()
            .entry("diff")
            .or_insert_with(|| json!(""));
        let combined = format!("{}\n{diff}", target.as_str().unwrap_or_default());
        *target = json!(combined);
    }
    for key in [
        "snapshots",
        "snapshotArtifacts",
        "diffs",
        "files",
        "attachments",
        "media",
        "generatedFiles",
    ] {
        if let Some(items) = child.get(key).and_then(Value::as_array) {
            let target = root
                .as_object_mut()
                .unwrap()
                .entry(key)
                .or_insert_with(|| json!([]));
            target.as_array_mut().unwrap().extend(items.iter().cloned());
        }
    }
}

// No recursive string joins: one bounded writer and one global node budget
// limit reference/union expansion before it allocates exponentially.
struct SignatureWriter {
    bytes: Vec<u8>,
    nodes: usize,
}
impl std::io::Write for SignatureWriter {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > 8192usize.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other(
                "composition signature byte limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
impl SignatureWriter {
    fn text(&mut self, text: &str) -> Result<(), String> {
        std::io::Write::write_all(self, text.as_bytes()).map_err(|e| e.to_string())
    }
    fn literal(&mut self, value: &impl serde::Serialize) -> Result<(), String> {
        serde_json::to_writer(self, value).map_err(|e| e.to_string())
    }
    fn render(
        &mut self,
        schema: &Value,
        root: &Value,
        depth: usize,
    ) -> Result<(), String> {
        self.nodes += 1;
        if self.nodes > 512 {
            return Err("composition signature complexity limit exceeded".into());
        }
        if depth > 12 {
            return self.text("unknown");
        }
        if let Some(target) = schema
            .get("$ref")
            .and_then(Value::as_str)
            .and_then(|r| r.strip_prefix('#'))
            .and_then(|r| root.pointer(r))
        {
            return self.render(target, root, depth + 1);
        }
        for (key, separator) in [("allOf", " & "), ("oneOf", " | "), ("anyOf", " | ")] {
            if let Some(items) = schema.get(key).and_then(Value::as_array) {
                for (index, item) in items.iter().enumerate() {
                    if index != 0 {
                        self.text(separator)?;
                    }
                    self.render(item, root, depth + 1)?;
                }
                return Ok(());
            }
        }
        if let Some(items) = schema.get("enum").and_then(Value::as_array) {
            for (index, item) in items.iter().enumerate() {
                if index != 0 {
                    self.text(" | ")?;
                }
                self.literal(item)?;
            }
            return Ok(());
        }
        match schema.get("type").and_then(Value::as_str) {
            Some("array") => {
                self.text("Array<")?;
                self.render(&schema["items"], root, depth + 1)?;
                self.text(">")
            }
            Some("object") => {
                let Some(properties) = schema["properties"].as_object() else {
                    return self.text("Record<string, unknown>");
                };
                self.text("{ ")?;
                let required = schema["required"].as_array();
                for (index, (key, value)) in properties.iter().enumerate() {
                    if index != 0 {
                        self.text("; ")?;
                    }
                    self.literal(key)?;
                    if !required.is_some_and(|r| {
                        r.iter().any(|v| v.as_str() == Some(key.as_str()))
                    }) {
                        self.text("?")?;
                    }
                    self.text(": ")?;
                    self.render(value, root, depth + 1)?;
                }
                self.text(" }")
            }
            Some("integer" | "number") => self.text("number"),
            Some("string") => self.text("string"),
            Some("boolean") => self.text("boolean"),
            Some("null") => self.text("null"),
            _ => self.text("unknown"),
        }
    }
}
fn schema_type(schema: &Value, root: &Value, depth: usize) -> Result<String, String> {
    let mut writer = SignatureWriter {
        bytes: Vec::new(),
        nodes: 0,
    };
    writer.render(schema, root, depth)?;
    String::from_utf8(writer.bytes).map_err(|e| e.to_string())
}
fn entry(item: ToolListItem) -> Result<(String, Entry, Option<String>), String> {
    if item.id.len() > 512 {
        return Err("composition tool identifier limit exceeded".into());
    }
    let (path, dispatch, mcp) =
        if let Some(path) = crate::agent_tool_registry::composition_mcp_path(&item.id) {
            (path.clone(), path, true)
        } else {
            (format!("neoism.{}", item.id), item.id.clone(), false)
        };
    if mcp && path.starts_with("neoism.") {
        return Err("MCP namespace neoism is reserved for native tools".into());
    }
    if path.len() > 256 || item.description.len() > 8192 {
        return Err("composition descriptor text limit exceeded".into());
    }
    serialized_size(&item.parameters, 256 * 1024)?;
    let (input_type, warning) = match schema_type(&item.parameters, &item.parameters, 0) {
        Ok(signature) => (signature, None),
        Err(error) => ("unknown".into(), Some(format!("Bounded discovery signature for {path}: {error}; full input schema remains enforced."))),
    };
    let signature = format!("{path}(input: {input_type}): Promise<ToolResult>");
    let validator = jsonschema::validator_for(&item.parameters)
        .map_err(|error| format!("invalid schema for {path}: {error}"))?;
    Ok((
        path.clone(),
        Entry {
            descriptor: ToolDescriptor {
                path,
                description: item.description,
                signature,
            },
            dispatch,
            mcp,
            validator: Arc::new(validator),
        },
        warning,
    ))
}

struct BoundedCatalog {
    entries: BTreeMap<String, Entry>,
    warnings: Vec<String>,
}
fn bounded_catalog(items: Vec<ToolListItem>) -> Result<BoundedCatalog, String> {
    if items.len() > 1024 {
        return Err("composition catalog tool count limit exceeded".into());
    }
    let mut schemas = 0usize;
    let mut descriptors = 0usize;
    let mut entries = BTreeMap::new();
    let mut warnings = Vec::new();
    let mut omitted_warnings = 0usize;
    let mut warn = |message: String| {
        if warnings.len() < 32 {
            warnings.push(message.chars().take(512).collect());
        } else {
            omitted_warnings += 1;
        }
    };
    for item in items
        .into_iter()
        .filter(|item| !EXCLUDED.contains(&item.id.as_str()))
    {
        let mcp = item.id.starts_with("mcp__");
        let id = item.id.chars().take(128).collect::<String>();
        let schema_bytes = match serialized_size(&item.parameters, 256 * 1024) {
            Ok(bytes) => bytes,
            Err(error) if mcp => {
                warn(format!(
                    "Omitted MCP tool {id}: schema exceeds per-tool bound ({error})."
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        let (path, entry, signature_warning) = match entry(item) {
            Ok(entry) => entry,
            Err(error) if mcp => {
                warn(format!(
                    "Omitted MCP tool {id}: {}",
                    error.chars().take(320).collect::<String>()
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        let descriptor_bytes = match serialized_size(&entry.descriptor, 32 * 1024) {
            Ok(bytes) => bytes,
            Err(error) if mcp => {
                warn(format!(
                    "Omitted MCP tool {id}: descriptor exceeds per-tool bound ({error})."
                ));
                continue;
            }
            Err(error) => return Err(error),
        };
        schemas += schema_bytes;
        descriptors += descriptor_bytes;
        if schemas > 8 * 1024 * 1024 {
            return Err("composition catalog schema aggregate limit exceeded".into());
        }
        if descriptors > 2 * 1024 * 1024 {
            return Err("composition catalog descriptor aggregate limit exceeded".into());
        }
        if let Some(warning) = signature_warning {
            warn(warning);
        }
        if entries.insert(path.clone(), entry).is_some() {
            return Err(format!(
                "composition catalog contains ambiguous path: {path}"
            ));
        }
    }
    drop(warn);
    if omitted_warnings != 0 {
        warnings.push(format!(
            "{omitted_warnings} additional bounded catalog warnings omitted."
        ));
    }
    Ok(BoundedCatalog { entries, warnings })
}

pub(crate) async fn run(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    directory: &str,
    permissions: &[PermissionRule],
    code: &str,
    snapshot: PluginGenerationLease,
    cancel: Option<Arc<AtomicBool>>,
    progress: Option<ProgressSink>,
) -> Result<ToolExecutionResult, String> {
    run_with_limits(
        state,
        session_id,
        message_id,
        directory,
        permissions,
        code,
        snapshot,
        cancel,
        progress,
        Default::default(),
    )
    .await
}

pub(crate) async fn run_with_limits(
    state: &AppState,
    session_id: &Id,
    message_id: &Id,
    directory: &str,
    permissions: &[PermissionRule],
    code: &str,
    snapshot: PluginGenerationLease,
    cancel: Option<Arc<AtomicBool>>,
    progress: Option<ProgressSink>,
    mut limits: neoism_agent_composition::Limits,
) -> Result<ToolExecutionResult, String> {
    snapshot
        .ensure_active()
        .map_err(|error| error.to_string())?;
    let started = std::time::Instant::now();
    if code.len() > limits.max_source_bytes {
        return Err("composition source exceeds the bounded source limit".into());
    }
    if let Some(progress) = &progress {
        progress.publish(state, &[], Some(code)).await;
    }
    let local = Arc::new(AtomicBool::new(false));
    struct CancelOnDrop(Arc<AtomicBool>);
    impl Drop for CancelOnDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let _cancel_on_drop = CancelOnDrop(local.clone());
    let _bridge = CancellationBridge::new(Some(local.clone()), cancel);
    let session = state
        .inner
        .store
        .get_session(session_id.as_str())
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "composition session not found".to_string())?;
    let model = if let Some(progress) = &progress {
        let message = progress.message.lock().await;
        match &message.info {
            MessageInfo::Assistant(info) => info.model_id.clone(),
            MessageInfo::User(info) => info.model.model_id.clone(),
        }
    } else {
        let message = state
            .inner
            .store
            .get_message(session_id.as_str(), message_id.as_str())
            .await
            .map_err(|e| e.to_string())?;
        match message.map(|m| m.info) {
            Some(MessageInfo::Assistant(info)) => info.model_id,
            Some(MessageInfo::User(info)) => info.model.model_id,
            None => session
                .model
                .as_ref()
                .map(|m| m.id.clone())
                .ok_or_else(|| {
                    "composition requires a resolved session model".to_string()
                })?,
        }
    };
    let execution = crate::caller::session_execution_policy(state.services(), &session);
    let auth = crate::mcp_auth::McpAuthStore::for_session(state.services(), &session)
        .map_err(|e| e.to_string())?;
    let catalog = crate::agent_tool_registry::composition_tools_for_agent(
        state,
        directory,
        &snapshot,
        permissions,
        &model,
        &execution,
        &auth,
    );
    let items = tokio::select! {
        biased;
        _ = crate::session_loop::wait_for_cancellation(local.clone()) => return Err("composition catalog discovery aborted".into()),
        result = tokio::time::timeout(limits.wall_clock.saturating_sub(started.elapsed()), catalog) =>
            result.map_err(|_| "composition catalog discovery timed out".to_string())?.map_err(|e| e.to_string())?,
    };
    let BoundedCatalog {
        entries,
        warnings: catalog_warnings,
    } = bounded_catalog(items)?;
    limits.wall_clock = limits.wall_clock.saturating_sub(started.elapsed());
    let supervisor = Arc::new(WorkerSupervisor::default());
    let host = Arc::new(Host {
        state: state.clone(),
        session: session_id.clone(),
        message: message_id.clone(),
        directory: directory.to_string(),
        tenant_id: crate::caller::session_tenant(&session).to_string(),
        permissions: permissions.to_vec(),
        snapshot,
        entries: Arc::new(entries),
        audit: Arc::new(Mutex::new(Audit::default())),
        progress,
        budget: Arc::new(Semaphore::new(4)),
        supervisor: Arc::downgrade(&supervisor),
    });
    let cleanup_budget = limits.cleanup_timeout;
    let mut result =
        neoism_agent_composition::execute(code, host.clone(), limits, local.clone())
            .await;
    result.warnings.extend(catalog_warnings);
    local.store(true, Ordering::SeqCst);
    let interrupted = supervisor.cleanup(cleanup_budget).await;
    let mut audit = host.audit.lock().await;
    audit.sealed = true;
    for mut record in interrupted {
        record["status"] = json!("partial_unknown");
        record["error"] = json!("interrupted after cleanup deadline; external effects may be partial or unknown");
        if let Some(existing) = audit
            .calls
            .iter_mut()
            .find(|call| call["id"] == record["id"])
        {
            if existing["status"] == "running" {
                existing["status"] = record["status"].clone();
                existing["error"] = record["error"].clone();
            }
        } else {
            audit.calls.push(record);
        }
    }
    if audit
        .calls
        .iter()
        .any(|call| call["status"] == "partial_unknown")
    {
        result.ok = false;
        result.warnings.push("Uncooperative child tasks were aborted after the bounded cleanup budget; partial_unknown external effects cannot be assumed undone.".into());
        result
            .error
            .get_or_insert(neoism_agent_composition::Diagnostic {
                kind: "interrupted".into(),
                message:
                    "composition has interrupted children with partial_unknown effects"
                        .into(),
                line: None,
                column: None,
            });
    }
    let calls = audit.calls.clone();
    let mut effects = json!({});
    for (_, metadata) in &audit.effects {
        merge_effects(&mut effects, Some(metadata));
    }
    drop(audit);
    if let Some(progress) = &host.progress {
        let _ =
            tokio::time::timeout(cleanup_budget, progress.publish(state, &calls, None))
                .await;
    }
    let replay = json!({"ok":result.ok,"value":result.value,"error":result.error,
        "logs":result.logs,"warnings":result.warnings,"truncated":result.truncated});
    let mut metadata = effects;
    if !metadata.is_object() {
        metadata = json!({});
    }
    metadata["composition"] = replay.clone();
    metadata["composition"]["code"] = json!(code);
    metadata["composition"]["toolCalls"] = json!(calls);
    metadata["isError"] = json!(!result.ok);
    Ok(ToolExecutionResult {
        title: "Tool composition".into(),
        output: replay.to_string(),
        metadata: Some(metadata),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn composition_child_presentation_is_bounded_without_raw_mcp_data() {
        let result = ToolExecutionResult {
            title: "Grep src/standalone.rs".into(),
            output: "\u{e9}".repeat(30_000),
            metadata: Some(json!({
                "isError":true,
                "mcp":{"client":"fixture","tool":"search","runtimeId":"mcp__fixture__search",
                    "result":{"structuredContent":{"body":"x".repeat(1024 * 1024)}}},
                "files":[{"path":"src/standalone.rs"}],
                "diff":"small diff"
            })),
        };
        let (view, bytes) = child_presentation("grep", &result, 32 * 1024).unwrap();
        assert!(bytes <= 32 * 1024);
        assert_eq!(view["tool"], "grep");
        assert_eq!(view["isError"], true);
        assert_eq!(view["truncated"], true);
        assert!(view["output"].as_str().unwrap().len() <= 16 * 1024);
        assert_eq!(view["metadata"]["mcp"]["client"], "fixture");
        assert!(view["metadata"]["mcp"].get("result").is_none());
        assert_eq!(view["metadata"]["diff"], "small diff");
        assert_eq!(view["metadata"]["files"][0]["path"], "src/standalone.rs");
    }

    #[test]
    fn composition_child_presentation_has_a_run_aggregate_budget() {
        let result = ToolExecutionResult {
            title: "Read file".into(),
            output: "\n".repeat(40_000),
            metadata: None,
        };
        let mut used = 0;
        let mut omitted = 0;
        for _ in 0..64 {
            match child_presentation("read", &result, PRESENTATION_BYTES - used) {
                Some((value, bytes)) => {
                    assert_eq!(serialized_size(&value, bytes).unwrap(), bytes);
                    used += bytes;
                }
                None => omitted += 1,
            }
        }
        assert!(used <= PRESENTATION_BYTES);
        assert!(omitted > 0);
        assert!(child_presentation("read", &result, 0).is_none());
    }

    #[test]
    fn composition_signatures_include_nested_ref_union_and_enum() {
        let schema = json!({"type":"object","required":["filter"],"properties":{
            "filter":{"$ref":"#/$defs/filter"}},"$defs":{"filter":{"type":"object","properties":{
                "states":{"type":"array","items":{"enum":["open","closed"]}},
                "cursor":{"oneOf":[{"type":"string"},{"type":"null"}]}},"required":["states"]}}});
        let signature = schema_type(&schema, &schema, 0).unwrap();
        assert!(signature.contains("Array<\"open\" | \"closed\">"));
        assert!(signature.contains("\"cursor\"?: string | null"));
        assert!(!signature.contains("unknown"));
    }
    #[test]
    fn composition_catalog_reserves_native_namespace_and_bounds_expansion() {
        let native = crate::tool::workspace_tool_items()
            .into_iter()
            .find(|t| t.id == "read")
            .unwrap();
        let mut mcp = native.clone();
        mcp.id = "mcp__neoism__read".into();
        let catalog = bounded_catalog(vec![native, mcp]).unwrap();
        assert!(catalog.entries.contains_key("neoism.read"));
        assert!(catalog
            .warnings
            .iter()
            .any(|warning| warning.contains("reserved")));
        let exponential = json!({"$ref":"#/$defs/r","$defs":{"r":{"allOf":[{"$ref":"#/$defs/r"},{"$ref":"#/$defs/r"},{"$ref":"#/$defs/r"}]}}});
        assert!(schema_type(&exponential, &exponential, 0)
            .unwrap_err()
            .contains("complexity"));
        let large_literal = json!({"enum":["x".repeat(100_000)]});
        assert!(schema_type(&large_literal, &large_literal, 0)
            .unwrap_err()
            .contains("byte limit"));
        assert_eq!(
            bounded_input(&json!({"data":"x".repeat(1_000_000)}))["omitted"],
            true
        );
        let mut large_description = crate::tool::workspace_tool_items().remove(0);
        large_description.id = "fixture".into();
        large_description.description = "x".repeat(8193);
        assert!(bounded_catalog(vec![large_description])
            .err()
            .unwrap()
            .contains("text limit"));
    }
    #[tokio::test]
    async fn composition_supervisor_bounds_uncooperative_leaf_and_aborts_on_drop() {
        let dropped = Arc::new(AtomicBool::new(false));
        let worker_dropped = dropped.clone();
        let supervisor = WorkerSupervisor::default();
        let worker = tokio::spawn(async move {
            struct Flag(Arc<AtomicBool>);
            impl Drop for Flag {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::SeqCst);
                }
            }
            let _flag = Flag(worker_dropped);
            std::future::pending::<()>().await; // deliberately ignores cancellation
        });
        supervisor.workers.lock().unwrap().insert("stuck".into(), TrackedWorker {
            worker:AbortWorker(worker),record:json!({"id":"stuck","path":"neoism.stuck","input":{},"status":"running"}),
        });
        tokio::task::yield_now().await;
        let started = std::time::Instant::now();
        let interrupted = supervisor
            .cleanup(std::time::Duration::from_millis(20))
            .await;
        assert!(started.elapsed() < std::time::Duration::from_secs(1));
        assert_eq!(interrupted[0]["id"], "stuck");
        tokio::task::yield_now().await;
        assert!(
            dropped.load(Ordering::SeqCst),
            "leaf future must be dropped, not detached"
        );
    }
    #[tokio::test]
    async fn composition_effect_ledger_retains_only_effects_not_large_protocol_data() {
        let audit = Arc::new(Mutex::new(Audit::default()));
        let sink = EffectSink {
            audit: audit.clone(),
        };
        let metadata = json!({"mcp":{"result":{"structuredContent":{"body":"x".repeat(1_000_000)}}},
            "unrelatedBody":"y".repeat(1_000_000),"snapshotArtifact":{"id":"artifact"},
            "snapshots":[{"path":"a","before":{"kind":"missing"}}],"attachments":[{"url":"recovery"}],"generatedFiles":[{"name":"known.png"}]});
        sink.record("child", Some(&metadata)).await;
        let audit = audit.lock().await;
        let retained = &audit.effects[0].1;
        assert!(retained.get("mcp").is_none());
        assert!(retained.get("unrelatedBody").is_none());
        assert_eq!(retained["attachments"][0]["url"], "recovery");
        assert_eq!(retained["snapshots"][0]["path"], "a");
        assert_eq!(retained["snapshotArtifact"]["id"], "artifact");
        assert!(serialized_size(retained, 4096).is_ok());
    }
    #[test]
    fn composition_malformed_mcp_is_skipped_and_large_signature_keeps_schema_authority() {
        let native = crate::tool::workspace_tool_items()
            .into_iter()
            .find(|t| t.id == "read")
            .unwrap();
        let mut invalid = native.clone();
        invalid.id = "mcp__fixture__invalid".into();
        invalid.parameters = json!({"type":"not-a-type"});
        let mut oversized = native.clone();
        oversized.id = "mcp__fixture__oversized".into();
        oversized.description = "x".repeat(8193);
        let mut large_schema = native.clone();
        large_schema.id = "mcp__fixture__schema".into();
        large_schema.parameters = json!({"description":"x".repeat(300_000)});
        let mut large_signature = native.clone();
        large_signature.id = "mcp__fixture__signature".into();
        large_signature.parameters = json!({"type":"object","properties":{"choice":{"enum":["x".repeat(100_000)]}},"required":["choice"]});
        let catalog = bounded_catalog(vec![
            native.clone(),
            invalid,
            oversized,
            large_schema,
            large_signature,
        ])
        .unwrap();
        assert!(catalog.entries.contains_key("neoism.read"));
        assert_eq!(catalog.entries.len(), 2);
        assert_eq!(catalog.warnings.len(), 4);
        let signature = &catalog.entries["fixture.signature"];
        assert!(signature.descriptor.signature.contains("input: unknown"));
        assert!(signature
            .validator
            .validate(&json!({"choice":"wrong"}))
            .is_err());
        assert!(bounded_catalog(vec![native; 1025])
            .err()
            .unwrap()
            .contains("count limit"));
    }
    #[test]
    fn composition_envelope_does_not_invent_json_from_formatted_text() {
        let result = ToolExecutionResult {
            title: "MCP fixture".into(),
            output: "{\"fake\":true}".into(),
            metadata: Some(
                json!({"isError":true,"mcp":{"result":{"structuredContent":{"real":true}}},
                "attachments":[{"url":"recovery"}],"generatedFiles":[{"url":"generated"}],"snapshots":[{"path":"a"}],"diffs":[{"path":"a"}]}),
            ),
        };
        let value = envelope(&result);
        assert_eq!(
            value["metadata"]["mcp"]["result"]["structuredContent"],
            json!({"real":true})
        );
        assert!(value.get("data").is_none());
        assert!(value.get("structuredContent").is_none());
        assert_eq!(value["isError"], true);
        let text_only = ToolExecutionResult {
            title: "read".into(),
            output: "{\"fake\":true}".into(),
            metadata: None,
        };
        assert!(envelope(&text_only).get("data").is_none());
        let mut root = Value::Null;
        merge_effects(&mut root, result.metadata.as_ref());
        merge_effects(&mut root, Some(&json!({"attachments":[{"url":"second"}]})));
        assert_eq!(root["attachments"].as_array().unwrap().len(), 2);
        assert_eq!(root["generatedFiles"][0]["url"], "generated");
        assert_eq!(root["snapshots"][0]["path"], "a");
    }
    #[test]
    fn composition_mcp_envelope_retains_one_structured_payload() {
        let result = ToolExecutionResult {
            title: "MCP fixture".into(),
            output: "formatted text".into(),
            metadata: Some(json!({"mcp":{"result":{"structuredContent":{
                "body":"x".repeat(48 * 1024)
            }}}})),
        };
        let value = envelope(&result);
        assert!(serialized_size(&value, 49 * 1024).is_ok());
        assert!(value.get("data").is_none());
        assert!(value.get("structuredContent").is_none());
        for alias in MCP_RESULT_ALIASES {
            assert_eq!(
                alias.path,
                ["metadata", "mcp", "result", "structuredContent"]
            );
        }
    }

    #[tokio::test]
    async fn composition_local_cancellation_never_aborts_session() {
        let local = Arc::new(AtomicBool::new(false));
        let session = Arc::new(AtomicBool::new(false));
        let bridge = CancellationBridge::new(Some(local.clone()), Some(session.clone()));
        local.store(true, Ordering::SeqCst);
        drop(bridge);
        assert!(!session.load(Ordering::SeqCst));
        local.store(false, Ordering::SeqCst);
        let _bridge = CancellationBridge::new(Some(local.clone()), Some(session.clone()));
        session.store(true, Ordering::SeqCst);
        tokio::time::timeout(
            std::time::Duration::from_secs(1),
            crate::session_loop::wait_for_cancellation(local),
        )
        .await
        .unwrap();
    }
}
