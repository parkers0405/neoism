//! Long-lived out-of-process plugins: the third-party ecosystem runtime.
//!
//! A serve plugin is any executable speaking `neoism-plugin/2` — newline JSON
//! frames over stdio. The host spawns it once per workspace plugin generation,
//! handshakes, and registers whatever the plugin declared (tools, hooks,
//! event subscriptions) into the same registry snapshot native plugins use.
//! Callbacks into the server go over HTTP through the published SDK; the
//! process boundary is where capability grants become real (filesystem
//! sandbox, network isolation, env scrubbing).
//!
//! Failure policy: a broken third-party plugin must never take the workspace
//! generation down. Spawn or handshake failures surface as a `Degraded`
//! readiness with a reason (visible in `/v2/plugins`) and contribute nothing.

use std::collections::{BTreeMap, HashMap};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::task::{Context as TaskContext, Poll};
use std::time::{Duration, Instant};

use base64::Engine;
use futures::StreamExt;
use futures_core::Stream;
use neoism_agent_core::{
    AuthInfo, CommandInfo, ProviderGenerationRequest, ProviderStreamEvent, SkillInfo,
    UserModel,
};
use neoism_agent_plugin_api::{
    AgentCatalog, AgentService, BrokerOwner, BrokerRequest, CommandService,
    ConfigDocument, ConfigService, ContributionMetadata, GeneratedMedia, HostCapability,
    MediaGenerationRequest, MediaKind, PluginContext, PluginContributions,
    PluginDescriptor, PluginFactory, PluginFuture, PluginInstance, PluginManifest,
    PluginReadiness, PluginRuntimeError, PluginScope, PluginToolDefinition,
    PluginToolInvocation, PluginToolResult, ProcessCancelRequest,
    ProcessHostBrokerCancelRequest, ProcessHostBrokerRequest,
    ProcessHostConfigGetRequest, ProcessHostConfigSetRequest, ProcessHostFrame,
    ProcessHostReplyFrame, ProcessHostWorkspacePathRequest,
    ProcessHostWorkspaceWriteRequest, ProcessInitializeRequest,
    ProcessInitializeResponse, ProcessMediaRequest, ProcessMediaResult,
    ProcessPluginFrame, ProcessPluginOwner, ProcessProviderAuthRequest,
    ProcessProviderDeclaration, ProcessProviderMetadataRequest, ProcessProviderRouteCall,
    ProcessProviderStreamRequest, ProcessRouteCall, ProcessServiceCall,
    ProcessServiceDeclaration, ProcessStreamEnvelope, ProcessToolInvokeRequest,
    ProcessWebSocketMessage, ProcessWebSocketMessageRequest, ProcessWebSocketOpenRequest,
    PromptRequest, PromptService, ProviderDescriptor, ProviderEventStream,
    ProviderModelMetadata, ProviderRouteRequest, ProviderService, ProviderStream,
    ReadinessState, RenderedPrompt, RouteContribution, RouteHandler, RouteRequest,
    RouteResponse, RuntimeHook, RuntimeTool, ServiceContribution, ServiceRequest,
    SkillService, SystemContextSection, SystemContextService, WebSocketRouteContribution,
    WebSocketRouteHandler, WebSocketSession, PROCESS_PLUGIN_V2_PROTOCOL,
};
use serde::Serialize;
use serde_json::{json, Value};

use crate::plugin::{build_plugin_command, sandbox_policy, SandboxPolicy};

pub(crate) const SERVE_PLUGIN_PROTOCOL: &str = PROCESS_PLUGIN_V2_PROTOCOL;
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);
const MAX_PLUGIN_FRAME_BYTES: usize = 1024 * 1024;
const MAX_PLUGIN_LOG_LINE_BYTES: usize = 64 * 1024;
const MAX_PENDING_CALLS: usize = 128;
const MAX_BROKER_INPUT_BYTES: usize = 256 * 1024;
const MAX_BROKER_OUTPUT_BYTES: usize = 512 * 1024;
const MAX_REVERSE_QUEUE: usize = 64;
const MAX_STREAMS: usize = 32;
const MAX_STREAM_QUEUE_ITEMS: usize = 64;
const MAX_STREAM_ITEM_BYTES: usize = 256 * 1024;
const MAX_STREAM_BYTES: usize = 16 * 1024 * 1024;
const MAX_MEDIA_BYTES: usize = 768 * 1024;

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// A configured serve plugin, parsed from the canonical `plugins` map entry:
///
/// ```jsonc
/// "plugins": {
///   "dev.example.todos":  { "options": { "serve": ["python3", "plugin.py"] } },
///   "dev.example.npm":    { "options": { "npm": "@example/neoism-plugin@1.2.0" } },
///   "dev.example.local":  { "options": { "entry": "./plugins/todos" } }
/// }
/// ```
#[derive(Clone, Debug)]
pub(crate) struct ServePluginSpec {
    pub id: String,
    pub source: ServeSource,
    pub env: BTreeMap<String, String>,
    pub config: Value,
    pub call_timeout: Duration,
    pub sandbox: SandboxPolicy,
    pub network: bool,
    pub working_directory: PathBuf,
    pub requested_capabilities: Vec<HostCapability>,
    pub package_revision: Option<String>,
    pub scope: PluginScope,
    pub strict_startup: bool,
}

#[derive(Clone, Debug)]
pub(crate) enum ServeSource {
    /// An explicit command line, run as-is.
    Command(Vec<String>),
    /// A local Node package directory or entry file, run with `node`.
    Entry(String),
    /// An npm package spec, installed into the shared plugin cache.
    Npm(String),
}

pub(crate) fn package_plugin_spec(
    package: &crate::plugin_package::DiscoveredAgentPackage,
    config: &neoism_agent_core::PluginConfig,
) -> Result<ServePluginSpec, String> {
    let agent = package
        .manifest
        .agent
        .as_ref()
        .ok_or_else(|| "package has no Agent entrypoint".to_string())?;
    let source = match agent.runtime {
        neoism_agent_plugin_api::AgentEntrypointRuntime::Lua => {
            let bundled = std::env::current_exe().ok().and_then(|executable| {
                let candidate = executable.with_file_name(format!(
                    "neoism-agent-lua-runner{}",
                    std::env::consts::EXE_SUFFIX
                ));
                candidate.is_file().then_some(candidate)
            });
            ServeSource::Command(vec![
                bundled
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "neoism-agent-lua-runner".into()),
                "--manifest".into(),
                package.manifest_path.to_string_lossy().into_owned(),
            ])
        }
        neoism_agent_plugin_api::AgentEntrypointRuntime::Process
            if !agent.command.is_empty() =>
        {
            ServeSource::Command(agent.command.clone())
        }
        neoism_agent_plugin_api::AgentEntrypointRuntime::Process => {
            ServeSource::Command(vec![package
                .root
                .join(&agent.entrypoint)
                .to_string_lossy()
                .into_owned()])
        }
    };
    Ok(ServePluginSpec {
        id: package.manifest.id.clone(),
        source,
        env: BTreeMap::new(),
        config: config
            .options
            .get("config")
            .cloned()
            .unwrap_or_else(|| json!({})),
        call_timeout: Duration::from_millis(
            config
                .options
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(60_000)
                .clamp(1_000, 600_000),
        ),
        sandbox: sandbox_policy(config.options.get("sandbox").and_then(Value::as_bool)),
        network: agent.capabilities.contains(&HostCapability::Network),
        working_directory: package.root.clone(),
        requested_capabilities: agent.capabilities.clone(),
        package_revision: Some(package.revision.clone()),
        scope: agent.scope,
        strict_startup: true,
    })
}

/// Parse a `plugins` map entry as a serve plugin. `None` means the entry is
/// not serve-shaped (the declarative one-shot loader owns it instead).
pub(crate) fn serve_plugin_spec(
    id: &str,
    directory: &str,
    options: &BTreeMap<String, Value>,
) -> Option<ServePluginSpec> {
    let source = if let Some(command) = options.get("serve") {
        let command = command
            .as_array()?
            .iter()
            .map(|part| part.as_str().map(ToOwned::to_owned))
            .collect::<Option<Vec<_>>>()?;
        ServeSource::Command(command)
    } else if let Some(entry) = options.get("entry").and_then(Value::as_str) {
        ServeSource::Entry(entry.to_string())
    } else if let Some(spec) = options.get("npm").and_then(Value::as_str) {
        ServeSource::Npm(spec.to_string())
    } else {
        return None;
    };
    let env = options
        .get("env")
        .and_then(Value::as_object)
        .map(|env| {
            env.iter()
                .filter_map(|(key, value)| {
                    value.as_str().map(|value| (key.clone(), value.to_string()))
                })
                .collect()
        })
        .unwrap_or_default();
    let requested_capabilities = options
        .get("capabilities")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|value| serde_json::from_value(value.clone()).ok())
        .collect();
    Some(ServePluginSpec {
        id: id.to_string(),
        source,
        env,
        config: options.get("config").cloned().unwrap_or_else(|| json!({})),
        call_timeout: Duration::from_millis(
            options
                .get("timeoutMs")
                .and_then(Value::as_u64)
                .unwrap_or(60_000)
                .clamp(1_000, 600_000),
        ),
        sandbox: sandbox_policy(options.get("sandbox").and_then(Value::as_bool)),
        // Serve plugins default to networked: SDK callbacks to the server run
        // over loopback, which a network namespace would sever.
        network: options
            .get("network")
            .and_then(Value::as_bool)
            .unwrap_or(true),
        working_directory: PathBuf::from(directory),
        requested_capabilities,
        package_revision: options
            .get("revision")
            .or_else(|| options.get("digest"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        scope: PluginScope::Workspace,
        strict_startup: false,
    })
}

fn plugin_cache_dir() -> PathBuf {
    PathBuf::from(crate::default_state_dir()).join("plugin-cache")
}

fn npm_cache_slot(spec: &str) -> PathBuf {
    let sanitized = spec
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>();
    plugin_cache_dir().join(sanitized)
}

fn npm_package_name(spec: &str) -> &str {
    // "@scope/name@1.2.3" → "@scope/name"; "name@1.2.3" → "name"; bare stays.
    match spec.rfind('@') {
        Some(at) if at > 0 => &spec[..at],
        _ => spec,
    }
}

/// The node entry file for a package directory: package.json `main`, else
/// conventional index files.
fn node_entry(package_dir: &Path) -> Option<PathBuf> {
    if package_dir.is_file() {
        return Some(package_dir.to_path_buf());
    }
    let manifest = package_dir.join("package.json");
    if let Ok(raw) = std::fs::read_to_string(&manifest) {
        if let Ok(parsed) = serde_json::from_str::<Value>(&raw) {
            if let Some(main) = parsed.get("main").and_then(Value::as_str) {
                let entry = package_dir.join(main);
                if entry.is_file() {
                    return Some(entry);
                }
            }
        }
    }
    ["index.mjs", "index.js"]
        .iter()
        .map(|name| package_dir.join(name))
        .find(|candidate| candidate.is_file())
}

/// Where a spec's runnable entry lives right now, if it exists. Feeding this
/// into the workspace config signature makes a finished background `npm
/// install` look like a config change, so the next acquire rebuilds the
/// generation with the plugin live — no restart needed.
pub(crate) fn resolved_serve_entry(spec: &ServePluginSpec) -> Option<PathBuf> {
    match &spec.source {
        ServeSource::Command(_) => None,
        ServeSource::Entry(entry) => {
            let path = Path::new(entry);
            let absolute = if path.is_absolute() {
                path.to_path_buf()
            } else {
                spec.working_directory.join(path)
            };
            node_entry(&absolute)
        }
        ServeSource::Npm(package_spec) => {
            let package_dir = npm_cache_slot(package_spec)
                .join("node_modules")
                .join(npm_package_name(package_spec));
            node_entry(&package_dir)
        }
    }
}

fn resolve_command(
    spec: &ServePluginSpec,
    executables: &Arc<dyn neoism_agent_service_api::ExecutableService>,
) -> Result<Vec<String>, String> {
    match &spec.source {
        ServeSource::Command(command) if command.is_empty() => {
            Err("serve command is empty".to_string())
        }
        ServeSource::Command(command) => Ok(command.clone()),
        ServeSource::Entry(_) => resolved_serve_entry(spec)
            .map(|entry| vec!["node".to_string(), entry.to_string_lossy().into_owned()])
            .ok_or_else(|| "plugin entry has no runnable node module".to_string()),
        ServeSource::Npm(package_spec) => match resolved_serve_entry(spec) {
            Some(entry) => Ok(vec![
                "node".to_string(),
                entry.to_string_lossy().into_owned(),
            ]),
            None => {
                start_background_npm_install(package_spec, executables);
                Err(format!("installing npm package {package_spec}"))
            }
        },
    }
}

fn start_background_npm_install(
    package_spec: &str,
    executables: &Arc<dyn neoism_agent_service_api::ExecutableService>,
) {
    static IN_FLIGHT: OnceLock<Mutex<std::collections::BTreeSet<String>>> =
        OnceLock::new();
    let in_flight = IN_FLIGHT.get_or_init(Default::default);
    {
        let mut guard = in_flight.lock().expect("npm install set poisoned");
        if !guard.insert(package_spec.to_string()) {
            return;
        }
    }
    let spec = package_spec.to_string();
    let executables = Arc::clone(executables);
    std::thread::Builder::new()
        .name(format!("neoism-plugin-npm-{spec}"))
        .spawn(move || {
            let slot = npm_cache_slot(&spec);
            let _ = std::fs::create_dir_all(&slot);
            // PATHEXT-aware resolution: on Windows `npm` is `npm.cmd`, which
            // CreateProcess cannot exec directly — route through the shared
            // batch-aware plugin command builder.
            let install = build_plugin_command(
                &executables,
                &[
                    "npm".to_string(),
                    "install".to_string(),
                    "--prefix".to_string(),
                    slot.to_string_lossy().into_owned(),
                    "--no-audit".to_string(),
                    "--no-fund".to_string(),
                    "--no-update-notifier".to_string(),
                    spec.clone(),
                ],
                &slot,
                SandboxPolicy::Off,
                true,
            );
            let mut install = match install {
                Ok(install) => install,
                Err(error) => {
                    tracing::warn!(package = %spec, %error, "npm is unavailable for serve plugin install");
                    in_flight
                        .lock()
                        .expect("npm install set poisoned")
                        .remove(&spec);
                    return;
                }
            };
            let result = install
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::piped())
                .output();
            match result {
                Ok(output) if output.status.success() => {
                    tracing::info!(package = %spec, "installed serve plugin from npm");
                }
                Ok(output) => {
                    tracing::warn!(
                        package = %spec,
                        stderr = %String::from_utf8_lossy(&output.stderr),
                        "npm install for serve plugin failed"
                    );
                }
                Err(error) => {
                    tracing::warn!(package = %spec, %error, "failed to run npm for serve plugin");
                }
            }
            in_flight
                .lock()
                .expect("npm install set poisoned")
                .remove(&spec);
        })
        .ok();
}

enum BoundedLine {
    Eof,
    Line(Vec<u8>),
    Oversized,
}

fn read_bounded_line(
    reader: &mut impl BufRead,
    max_bytes: usize,
) -> io::Result<BoundedLine> {
    let mut line = Vec::new();
    loop {
        let available = reader.fill_buf()?;
        if available.is_empty() {
            return if line.is_empty() {
                Ok(BoundedLine::Eof)
            } else {
                Ok(BoundedLine::Line(line))
            };
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let consumed = newline.map_or(available.len(), |index| index + 1);
        if line.len().saturating_add(consumed) > max_bytes {
            reader.consume(consumed);
            if newline.is_none() {
                loop {
                    let available = reader.fill_buf()?;
                    if available.is_empty() {
                        break;
                    }
                    if let Some(index) = available.iter().position(|byte| *byte == b'\n')
                    {
                        reader.consume(index + 1);
                        break;
                    }
                    let len = available.len();
                    reader.consume(len);
                }
            }
            return Ok(BoundedLine::Oversized);
        }
        line.extend_from_slice(&available[..consumed]);
        reader.consume(consumed);
        if newline.is_some() {
            line.pop();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            return Ok(BoundedLine::Line(line));
        }
    }
}

fn write_serialized_frame<T: Serialize>(
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
    frame: &T,
) -> Result<(), String> {
    let mut encoded = serde_json::to_vec(frame)
        .map_err(|error| format!("failed to encode plugin frame: {error}"))?;
    if encoded.len() > MAX_PLUGIN_FRAME_BYTES {
        return Err(format!(
            "plugin frame exceeds the {} byte protocol limit",
            MAX_PLUGIN_FRAME_BYTES
        ));
    }
    encoded.push(b'\n');
    let mut stdin = stdin.lock().expect("stdin slot poisoned");
    let Some(stdin) = stdin.as_mut() else {
        return Err("plugin process is not running".to_string());
    };
    stdin
        .write_all(&encoded)
        .and_then(|()| stdin.flush())
        .map_err(|error| format!("failed to write to plugin: {error}"))
}

fn handle_reverse_request(
    frame: ProcessPluginFrame,
    expected_owner: &ProcessPluginOwner,
    context: &PluginContext,
    active: &AtomicBool,
    stdin: &Arc<Mutex<Option<ChildStdin>>>,
) {
    let Some(id) = frame.id else {
        return;
    };
    let method = frame.method.as_deref().unwrap_or_default().to_string();
    let result = if let Err(error) = validate_reverse_owner(
        frame.owner.as_ref(),
        expected_owner,
        active.load(Ordering::Acquire),
    ) {
        Err(error)
    } else {
        dispatch_reverse_request_for_owner(
            context,
            Some(expected_owner),
            &method,
            frame.params.unwrap_or(Value::Null),
        )
    };
    tracing::info!(
        target: "neoism::plugin_capability_audit",
        plugin_id = %expected_owner.plugin_id,
        instance_id = %expected_owner.instance_id,
        package_revision = expected_owner.package_revision.as_deref().unwrap_or(""),
        registry_generation = expected_owner.registry_generation.unwrap_or_default(),
        scope = ?expected_owner.scope,
        workspace_id = expected_owner.workspace_id.as_deref().unwrap_or(""),
        scope_id = expected_owner.scope_id.as_deref().unwrap_or(""),
        capability = reverse_method_capability(&method),
        method = %method,
        outcome = if result.is_ok() { "allowed" } else { "denied" },
        "plugin host capability request"
    );
    let reply = match result {
        Ok(result) => ProcessHostReplyFrame {
            id,
            result: Some(result),
            error: None,
        },
        Err(error) => ProcessHostReplyFrame {
            id,
            result: None,
            error: Some(error),
        },
    };
    let _ = write_serialized_frame(stdin, &reply);
}

fn reverse_method_capability(method: &str) -> &'static str {
    match method {
        "host.config.get" => "config-read",
        "host.config.set" => "config-write",
        "host.workspace.read" | "host.workspace.list" => "workspace-read",
        "host.workspace.write" => "workspace-write",
        "host.event.publish" => "event-publish",
        "host.network.request" => "network",
        "host.process.spawn" | "host.process.cancel" => "process-spawn",
        "host.task.spawn" | "host.task.cancel" => "task-spawn",
        "host.secret.use" => "secret-use",
        "host.secret.read" => "secret-read",
        "host.prompt.read" => "prompt-read",
        "host.message.read" => "message-read",
        "host.response.transform" => "response-transform",
        "host.provider.call" => "provider-access",
        "host.policy.call" => "policy-invoke",
        _ => "unknown",
    }
}

fn validate_reverse_owner(
    actual: Option<&ProcessPluginOwner>,
    expected: &ProcessPluginOwner,
    active: bool,
) -> Result<(), String> {
    if !active {
        return Err("plugin instance is retired".to_string());
    }
    if actual != Some(expected) {
        return Err("plugin reverse request has the wrong owner".to_string());
    }
    Ok(())
}

fn decode_reverse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, String> {
    serde_json::from_value(params)
        .map_err(|error| format!("invalid host-service request: {error}"))
}

#[cfg(test)]
fn dispatch_reverse_request(
    context: &PluginContext,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    dispatch_reverse_request_for_owner(context, None, method, params)
}

fn dispatch_reverse_request_for_owner(
    context: &PluginContext,
    owner: Option<&ProcessPluginOwner>,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let unavailable = |service: &str| format!("host {service} service is not available");
    let value = match method {
        "host.config.get" => {
            let request: ProcessHostConfigGetRequest = decode_reverse(params)?;
            let config = context.config().ok_or_else(|| unavailable("config"))?;
            serde_json::to_value(
                config
                    .get(&request.key)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("failed to encode host-service response: {error}"))
        }
        "host.config.set" => {
            let request: ProcessHostConfigSetRequest = decode_reverse(params)?;
            let config = context.config().ok_or_else(|| unavailable("config"))?;
            config
                .set(&request.key, request.value)
                .map_err(|error| error.to_string())?;
            Ok(json!({}))
        }
        "host.workspace.read" => {
            let request: ProcessHostWorkspacePathRequest = decode_reverse(params)?;
            let workspace = context
                .workspace_access()
                .ok_or_else(|| unavailable("workspace"))?;
            serde_json::to_value(
                workspace
                    .read(&request.path)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("failed to encode host-service response: {error}"))
        }
        "host.workspace.list" => {
            let request: ProcessHostWorkspacePathRequest = decode_reverse(params)?;
            let workspace = context
                .workspace_access()
                .ok_or_else(|| unavailable("workspace"))?;
            serde_json::to_value(
                workspace
                    .list(&request.path)
                    .map_err(|error| error.to_string())?,
            )
            .map_err(|error| format!("failed to encode host-service response: {error}"))
        }
        "host.workspace.write" => {
            let request: ProcessHostWorkspaceWriteRequest = decode_reverse(params)?;
            let workspace = context
                .workspace_access()
                .ok_or_else(|| unavailable("workspace"))?;
            workspace
                .write(&request.path, &request.contents)
                .map_err(|error| error.to_string())?;
            Ok(json!({}))
        }
        "host.event.publish" => {
            let request = decode_reverse(params)?;
            context
                .events()
                .ok_or_else(|| unavailable("event publisher"))?
                .publish(request)
                .map_err(|error| error.to_string())?;
            Ok(json!({}))
        }
        "host.network.request" => {
            dispatch_broker(context, owner, HostCapability::Network, params)
        }
        "host.process.spawn" => {
            dispatch_broker(context, owner, HostCapability::ProcessSpawn, params)
        }
        "host.process.cancel" => {
            cancel_broker(context, owner, HostCapability::ProcessSpawn, params)
        }
        "host.task.spawn" => {
            dispatch_broker(context, owner, HostCapability::TaskSpawn, params)
        }
        "host.task.cancel" => {
            cancel_broker(context, owner, HostCapability::TaskSpawn, params)
        }
        "host.secret.use" => {
            dispatch_broker(context, owner, HostCapability::SecretUse, params)
        }
        "host.secret.read" => {
            dispatch_broker(context, owner, HostCapability::SecretRead, params)
        }
        "host.prompt.read" => {
            dispatch_broker(context, owner, HostCapability::PromptRead, params)
        }
        "host.message.read" => {
            dispatch_broker(context, owner, HostCapability::MessageRead, params)
        }
        "host.response.transform" => {
            dispatch_broker(context, owner, HostCapability::ResponseTransform, params)
        }
        "host.provider.call" => {
            dispatch_broker(context, owner, HostCapability::ProviderAccess, params)
        }
        "host.policy.call" => {
            dispatch_broker(context, owner, HostCapability::PolicyInvoke, params)
        }
        _ => Err(format!("unknown host-service method `{method}`")),
    };
    value
}

fn dispatch_broker(
    context: &PluginContext,
    owner: Option<&ProcessPluginOwner>,
    capability: HostCapability,
    params: Value,
) -> Result<Value, String> {
    if serde_json::to_vec(&params)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_BROKER_INPUT_BYTES
    {
        return Err("host broker input exceeds limit".into());
    }
    let request: ProcessHostBrokerRequest = decode_reverse(params)?;
    if request.operation.is_empty() || request.operation.len() > 256 {
        return Err("host broker operation is invalid".into());
    }
    let response = context
        .broker(capability)
        .ok_or_else(|| format!("host {capability:?} broker is not available"))?
        .call(BrokerRequest {
            operation: request.operation,
            input: request.input,
            owner: owner.map(|owner| BrokerOwner {
                plugin_id: owner.plugin_id.clone(),
                instance_id: owner.instance_id.clone(),
                registry_generation: owner.registry_generation.unwrap_or_default(),
                scope: owner.scope.unwrap_or_else(|| context.scope().kind()),
                workspace_id: owner.workspace_id.clone(),
                scope_id: owner.scope_id.clone(),
            }),
        })
        .map_err(|error| error.to_string())?;
    let value = serde_json::to_value(response).map_err(|error| error.to_string())?;
    if serde_json::to_vec(&value)
        .map_err(|error| error.to_string())?
        .len()
        > MAX_BROKER_OUTPUT_BYTES
    {
        return Err("host broker output exceeds limit".into());
    }
    Ok(value)
}

fn cancel_broker(
    context: &PluginContext,
    owner: Option<&ProcessPluginOwner>,
    capability: HostCapability,
    params: Value,
) -> Result<Value, String> {
    let request: ProcessHostBrokerCancelRequest = decode_reverse(params)?;
    if request.opaque_id.is_empty() || request.opaque_id.len() > 256 {
        return Err("host broker cancellation identity is invalid".into());
    }
    context
        .broker(capability)
        .ok_or_else(|| format!("host {capability:?} broker is not available"))?
        .cancel(
            &request.opaque_id,
            owner.map(|owner| BrokerOwner {
                plugin_id: owner.plugin_id.clone(),
                instance_id: owner.instance_id.clone(),
                registry_generation: owner.registry_generation.unwrap_or_default(),
                scope: owner.scope.unwrap_or_else(|| context.scope().kind()),
                workspace_id: owner.workspace_id.clone(),
                scope_id: owner.scope_id.clone(),
            }),
        )
        .map_err(|error| error.to_string())?;
    Ok(json!({}))
}

// ---------------------------------------------------------------------------
// Process host
// ---------------------------------------------------------------------------

struct ProcessStreamState {
    request_id: u64,
    sender: tokio::sync::mpsc::Sender<Result<Value, String>>,
    terminal: Arc<Mutex<Option<String>>>,
    opened: bool,
    bytes: usize,
}

struct ProcessValueStream {
    receiver: tokio::sync::mpsc::Receiver<Result<Value, String>>,
    terminal: Arc<Mutex<Option<String>>>,
    terminal_emitted: bool,
    host: Option<Arc<ProcessHost>>,
    stream_id: String,
}

impl Stream for ProcessValueStream {
    type Item = Result<Value, PluginRuntimeError>;

    fn poll_next(
        mut self: Pin<&mut Self>,
        context: &mut TaskContext<'_>,
    ) -> Poll<Option<Self::Item>> {
        match Pin::new(&mut self.receiver).poll_recv(context) {
            Poll::Ready(None) if !self.terminal_emitted => {
                self.terminal_emitted = true;
                let error = self
                    .terminal
                    .lock()
                    .expect("stream terminal poisoned")
                    .take();
                match error {
                    Some(error) => Poll::Ready(Some(Err(PluginRuntimeError::new(error)))),
                    None => Poll::Ready(None),
                }
            }
            Poll::Ready(None) => Poll::Ready(None),
            Poll::Ready(Some(Ok(value))) => Poll::Ready(Some(Ok(value))),
            Poll::Ready(Some(Err(error))) => {
                Poll::Ready(Some(Err(PluginRuntimeError::new(error))))
            }
            Poll::Pending => Poll::Pending,
        }
    }
}

impl Drop for ProcessValueStream {
    fn drop(&mut self) {
        if let Some(host) = self.host.take() {
            host.cancel_stream(&self.stream_id, "stream consumer dropped");
        }
    }
}

struct ProcessHost {
    spec: ServePluginSpec,
    owner: ProcessPluginOwner,
    executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    child: Mutex<Option<Child>>,
    stdin: Arc<Mutex<Option<ChildStdin>>>,
    pending: Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Result<Value, String>>>>>,
    next_id: AtomicU64,
    context: PluginContext,
    active: Arc<AtomicBool>,
    streams: Arc<Mutex<HashMap<String, ProcessStreamState>>>,
    next_stream_id: AtomicU64,
}

impl ProcessHost {
    fn new(
        spec: ServePluginSpec,
        executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
        context: PluginContext,
    ) -> Self {
        static NEXT_INSTANCE_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            owner: ProcessPluginOwner {
                plugin_id: spec.id.clone(),
                instance_id: format!(
                    "plugin-instance-{}",
                    NEXT_INSTANCE_ID.fetch_add(1, Ordering::Relaxed)
                ),
                package_revision: spec.package_revision.clone(),
                registry_generation: context
                    .metadata("registryGeneration")
                    .and_then(Value::as_u64),
                scope: Some(context.scope().kind()),
                workspace_id: context.workspace().map(|workspace| workspace.id.clone()),
                scope_id: match context.scope() {
                    neoism_agent_plugin_api::RuntimeScope::User { user_id } => {
                        Some(user_id.clone())
                    }
                    neoism_agent_plugin_api::RuntimeScope::Session {
                        session_id, ..
                    } => Some(session_id.clone()),
                    _ => None,
                },
            },
            spec,
            executables,
            child: Mutex::new(None),
            stdin: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
            context,
            active: Arc::new(AtomicBool::new(true)),
            streams: Arc::new(Mutex::new(HashMap::new())),
            next_stream_id: AtomicU64::new(1),
        }
    }

    fn spawn_and_initialize(&self) -> Result<ProcessInitializeResponse, String> {
        let command = resolve_command(&self.spec, &self.executables)?;
        let mut built = build_plugin_command(
            &self.executables,
            &command,
            &self.spec.working_directory,
            self.spec.sandbox,
            self.spec.network,
        )
        .map_err(|error| error.to_string())?;
        built
            .env("NEOISM_PLUGIN_ID", &self.spec.id)
            .env("NEOISM_PLUGIN_PROTOCOL", SERVE_PLUGIN_PROTOCOL)
            .env("NEOISM_PLUGIN_INSTANCE_ID", &self.owner.instance_id)
            // A process plugin receives capability-scoped stdio services, not
            // the server's bearer credential inherited from its parent.
            .env_remove("NEOISM_AGENT_TOKEN")
            .env(
                "NEOISM_WORKSPACE_DIR",
                self.spec.working_directory.as_os_str(),
            );
        if let Ok(server) = std::env::var("NEOISM_SERVER") {
            built.env("NEOISM_AGENT_SERVER_URL", server);
        }
        for (key, value) in &self.spec.env {
            built.env(key, value);
        }
        let mut child = built
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("failed to start plugin process: {error}"))?;
        let stdin = child.stdin.take().expect("piped serve-plugin stdin");
        let stdout = child.stdout.take().expect("piped serve-plugin stdout");
        let stderr = child.stderr.take().expect("piped serve-plugin stderr");

        let pending = Arc::clone(&self.pending);
        let streams = Arc::clone(&self.streams);
        let reverse_stdin = Arc::clone(&self.stdin);
        let reverse_owner = self.owner.clone();
        let reverse_context = self.context.clone();
        let reverse_active = Arc::clone(&self.active);
        let worker_owner = reverse_owner.clone();
        let worker_active = Arc::clone(&reverse_active);
        let (reverse_tx, reverse_rx) = std::sync::mpsc::sync_channel(MAX_REVERSE_QUEUE);
        let reverse_plugin_id = self.spec.id.clone();
        std::thread::Builder::new()
            .name(format!("neoism-plugin-host-{reverse_plugin_id}"))
            .spawn(move || {
                while let Ok(frame) = reverse_rx.recv() {
                    handle_reverse_request(
                        frame,
                        &worker_owner,
                        &reverse_context,
                        &worker_active,
                        &reverse_stdin,
                    );
                }
            })
            .map_err(|error| {
                format!("failed to start plugin host-service worker: {error}")
            })?;
        let plugin_id = self.spec.id.clone();
        let reader_stdin = Arc::clone(&self.stdin);
        std::thread::Builder::new()
            .name(format!("neoism-plugin-io-{plugin_id}"))
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let frame = match read_bounded_line(&mut reader, MAX_PLUGIN_FRAME_BYTES) {
                        Ok(BoundedLine::Eof) | Err(_) => break,
                        Ok(BoundedLine::Oversized) => {
                            tracing::warn!(plugin = %plugin_id, limit = MAX_PLUGIN_FRAME_BYTES, "serve plugin frame exceeded the protocol limit");
                            continue;
                        }
                        Ok(BoundedLine::Line(line)) if line.iter().all(u8::is_ascii_whitespace) => continue,
                        Ok(BoundedLine::Line(line)) => match serde_json::from_slice::<ProcessPluginFrame>(&line) {
                            Ok(frame) => frame,
                            Err(_) => {
                                tracing::warn!(plugin = %plugin_id, "serve plugin wrote a non-protocol line");
                                continue;
                            }
                        },
                    };
                    if let Some(stream) = frame.stream {
                        handle_stream_frame(
                            &streams,
                            frame.owner.as_ref(),
                            &reverse_owner,
                            frame.request_id,
                            stream,
                            reverse_active.load(Ordering::Acquire),
                        );
                        continue;
                    }
                    if frame.method.is_some() {
                        let reply_id = frame.id;
                        if let Err(error) = reverse_tx.try_send(frame) {
                            if let Some(id) = reply_id {
                                let message = match error {
                                    std::sync::mpsc::TrySendError::Full(_) => "plugin host-service queue is full",
                                    std::sync::mpsc::TrySendError::Disconnected(_) => "plugin host-service worker is unavailable",
                                };
                                let _ = write_serialized_frame(&reader_stdin, &ProcessHostReplyFrame {
                                    id,
                                    result: None,
                                    error: Some(message.to_string()),
                                });
                            }
                        }
                        continue;
                    }
                    let Some(id) = frame.id else { continue };
                    let reply = match frame.error {
                        Some(error) => Err(error),
                        None => Ok(frame.result.unwrap_or(Value::Null)),
                    };
                    if let Some(sender) =
                        pending.lock().expect("pending map poisoned").remove(&id)
                    {
                        let _ = sender.send(reply);
                    }
                }
                // EOF: fail everything still waiting instead of timing out.
                let stranded = std::mem::take(
                    &mut *pending.lock().expect("pending map poisoned"),
                );
                for (_, sender) in stranded {
                    let _ = sender.send(Err("plugin process exited".to_string()));
                }
            })
            .map_err(|error| format!("failed to start plugin reader: {error}"))?;
        let stderr_plugin_id = self.spec.id.clone();
        std::thread::Builder::new()
            .name(format!("neoism-plugin-log-{stderr_plugin_id}"))
            .spawn(move || {
                let mut reader = BufReader::new(stderr);
                loop {
                    match read_bounded_line(&mut reader, MAX_PLUGIN_LOG_LINE_BYTES) {
                        Ok(BoundedLine::Eof) | Err(_) => break,
                        Ok(BoundedLine::Oversized) => tracing::warn!(plugin = %stderr_plugin_id, limit = MAX_PLUGIN_LOG_LINE_BYTES, "serve plugin log line exceeded the limit"),
                        Ok(BoundedLine::Line(line)) => tracing::info!(plugin = %stderr_plugin_id, "{}", String::from_utf8_lossy(&line)),
                    }
                }
            })
            .ok();

        *self.stdin.lock().expect("stdin slot poisoned") = Some(stdin);
        *self.child.lock().expect("child slot poisoned") = Some(child);

        let handshake: ProcessInitializeResponse = serde_json::from_value(
            self.call(
                "initialize",
                serde_json::to_value(ProcessInitializeRequest {
                    protocol: SERVE_PLUGIN_PROTOCOL.to_string(),
                    plugin_id: self.spec.id.clone(),
                    instance_id: self.owner.instance_id.clone(),
                    directory: self.spec.working_directory.to_string_lossy().into_owned(),
                    config: self.spec.config.clone(),
                    owner: Some(self.owner.clone()),
                })
                .map_err(|error| {
                    format!("failed to encode plugin initialize request: {error}")
                })?,
                INITIALIZE_TIMEOUT,
            )?,
        )
        .map_err(|error| format!("plugin initialize reply is invalid: {error}"))?;
        if handshake.protocol != SERVE_PLUGIN_PROTOCOL {
            return Err(format!(
                "plugin speaks {} but this host requires {SERVE_PLUGIN_PROTOCOL}",
                handshake.protocol
            ));
        }
        Ok(handshake)
    }

    fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, String> {
        self.call_with_cancel(method, params, timeout, None)
    }

    fn call_with_cancel(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: Option<&AtomicBool>,
    ) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = std::sync::mpsc::channel();
        {
            let mut pending = self.pending.lock().expect("pending map poisoned");
            if pending.len() >= MAX_PENDING_CALLS {
                return Err("too many pending plugin calls".to_string());
            }
            pending.insert(id, sender);
        }
        if let Err(error) = self.write_frame(&ProcessHostFrame {
            id: Some(id),
            method: method.to_string(),
            params,
        }) {
            self.pending
                .lock()
                .expect("pending map poisoned")
                .remove(&id);
            return Err(error);
        }
        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_some_and(|cancel| cancel.load(Ordering::Acquire)) {
                self.pending
                    .lock()
                    .expect("pending map poisoned")
                    .remove(&id);
                self.notify(
                    "$/cancel",
                    serde_json::to_value(ProcessCancelRequest { id })
                        .unwrap_or(Value::Null),
                );
                return Err(format!("plugin {method} was cancelled"));
            }
            let now = Instant::now();
            if now >= deadline {
                self.pending
                    .lock()
                    .expect("pending map poisoned")
                    .remove(&id);
                self.notify(
                    "$/cancel",
                    serde_json::to_value(ProcessCancelRequest { id })
                        .unwrap_or(Value::Null),
                );
                return Err(format!(
                    "plugin {method} timed out after {} ms",
                    timeout.as_millis()
                ));
            }
            let wait = (deadline - now).min(Duration::from_millis(25));
            match receiver.recv_timeout(wait) {
                Ok(reply) => return reply,
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("plugin response channel closed".to_string())
                }
            }
        }
    }

    fn allocate_stream(
        self: &Arc<Self>,
    ) -> Result<(String, u64, ProcessValueStream), String> {
        let sequence = self.next_stream_id.fetch_add(1, Ordering::Relaxed);
        let stream_id = format!("{}-stream-{sequence}", self.owner.instance_id);
        let request_id = sequence;
        let (sender, receiver) = tokio::sync::mpsc::channel(MAX_STREAM_QUEUE_ITEMS);
        let terminal = Arc::new(Mutex::new(None));
        let mut streams = self.streams.lock().expect("stream map poisoned");
        if streams.len() >= MAX_STREAMS {
            return Err("too many active plugin streams".to_string());
        }
        streams.insert(
            stream_id.clone(),
            ProcessStreamState {
                request_id,
                sender,
                terminal: Arc::clone(&terminal),
                opened: false,
                bytes: 0,
            },
        );
        Ok((
            stream_id.clone(),
            request_id,
            ProcessValueStream {
                receiver,
                terminal,
                terminal_emitted: false,
                host: Some(Arc::clone(self)),
                stream_id,
            },
        ))
    }

    fn cancel_stream(&self, stream_id: &str, reason: &str) {
        if let Some(state) = self
            .streams
            .lock()
            .expect("stream map poisoned")
            .remove(stream_id)
        {
            *state.terminal.lock().expect("stream terminal poisoned") =
                Some(reason.to_string());
            self.notify(
                "$/cancelStream",
                json!({ "streamId": stream_id, "requestId": state.request_id }),
            );
        }
    }

    fn fail_all_streams(&self, reason: &str) {
        let streams =
            std::mem::take(&mut *self.streams.lock().expect("stream map poisoned"));
        for (_, state) in streams {
            *state.terminal.lock().expect("stream terminal poisoned") =
                Some(reason.to_string());
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.write_frame(&ProcessHostFrame {
            id: None,
            method: method.to_string(),
            params,
        });
    }

    fn write_frame(&self, frame: &ProcessHostFrame) -> Result<(), String> {
        write_serialized_frame(&self.stdin, frame)
    }

    fn shutdown_sync(&self) {
        self.active.store(false, Ordering::Release);
        self.context.revoke_capabilities();
        self.fail_all_streams("plugin instance retired");
        self.notify("shutdown", json!({}));
        drop(self.stdin.lock().expect("stdin slot poisoned").take());
        let Some(mut child) = self.child.lock().expect("child slot poisoned").take()
        else {
            return;
        };
        let deadline = Instant::now() + SHUTDOWN_GRACE;
        loop {
            match child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(25));
                }
                _ => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return;
                }
            }
        }
    }
}

fn handle_stream_frame(
    streams: &Arc<Mutex<HashMap<String, ProcessStreamState>>>,
    actual_owner: Option<&ProcessPluginOwner>,
    expected_owner: &ProcessPluginOwner,
    request_id: Option<u64>,
    envelope: ProcessStreamEnvelope,
    active: bool,
) {
    if validate_reverse_owner(actual_owner, expected_owner, active).is_err() {
        return;
    }
    let stream_id = match &envelope {
        ProcessStreamEnvelope::Open { stream_id }
        | ProcessStreamEnvelope::Item { stream_id, .. }
        | ProcessStreamEnvelope::End { stream_id }
        | ProcessStreamEnvelope::Error { stream_id, .. } => stream_id.clone(),
    };
    let mut streams = streams.lock().expect("stream map poisoned");
    let Some(state) = streams.get_mut(&stream_id) else {
        return;
    };
    if request_id != Some(state.request_id) {
        *state.terminal.lock().expect("stream terminal poisoned") =
            Some("stream request id mismatch".to_string());
        streams.remove(&stream_id);
        return;
    }
    match envelope {
        ProcessStreamEnvelope::Open { .. } if !state.opened => state.opened = true,
        ProcessStreamEnvelope::Open { .. } => {
            *state.terminal.lock().expect("stream terminal poisoned") =
                Some("stream opened more than once".to_string());
            streams.remove(&stream_id);
        }
        ProcessStreamEnvelope::Item { value, .. } if state.opened => {
            let bytes =
                serde_json::to_vec(&value).map_or(MAX_STREAM_ITEM_BYTES + 1, |v| v.len());
            if bytes > MAX_STREAM_ITEM_BYTES
                || state.bytes.saturating_add(bytes) > MAX_STREAM_BYTES
            {
                *state.terminal.lock().expect("stream terminal poisoned") =
                    Some("plugin stream byte budget exceeded".to_string());
                streams.remove(&stream_id);
            } else {
                state.bytes += bytes;
                if state.sender.try_send(Ok(value)).is_err() {
                    *state.terminal.lock().expect("stream terminal poisoned") =
                        Some("plugin stream backpressure limit exceeded".to_string());
                    streams.remove(&stream_id);
                }
            }
        }
        ProcessStreamEnvelope::Item { .. } => {
            *state.terminal.lock().expect("stream terminal poisoned") =
                Some("plugin stream item arrived before open".to_string());
            streams.remove(&stream_id);
        }
        ProcessStreamEnvelope::End { .. } if state.opened => {
            streams.remove(&stream_id);
        }
        ProcessStreamEnvelope::End { .. } => {
            *state.terminal.lock().expect("stream terminal poisoned") =
                Some("plugin stream ended before open".to_string());
            streams.remove(&stream_id);
        }
        ProcessStreamEnvelope::Error { error, .. } => {
            *state.terminal.lock().expect("stream terminal poisoned") = Some(error);
            streams.remove(&stream_id);
        }
    }
}

impl Drop for ProcessHost {
    fn drop(&mut self) {
        self.active.store(false, Ordering::Release);
        self.context.revoke_capabilities();
        if let Ok(mut child) = self.child.lock() {
            if let Some(mut child) = child.take() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registry adapters
// ---------------------------------------------------------------------------

struct ProcessTool {
    host: Arc<ProcessHost>,
    definition: PluginToolDefinition,
}

fn process_service_call<Request: Serialize, Response: serde::de::DeserializeOwned>(
    host: &ProcessHost,
    method: &str,
    service_id: &str,
    request: Request,
) -> Result<Response, PluginRuntimeError> {
    let params = serde_json::to_value(ProcessServiceCall {
        service_id: service_id.to_string(),
        request,
    })
    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
    let result = host
        .call(method, params, host.spec.call_timeout)
        .map_err(PluginRuntimeError::new)?;
    serde_json::from_value(result).map_err(|error| {
        PluginRuntimeError::new(format!("plugin {method} reply is invalid: {error}"))
    })
}

struct ProcessAgentService {
    host: Arc<ProcessHost>,
    id: String,
}

impl AgentService for ProcessAgentService {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, AgentCatalog> {
        let host = Arc::clone(&self.host);
        let id = self.id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                process_service_call(&host, "agent.list", &id, request)
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessCommandService {
    host: Arc<ProcessHost>,
    id: String,
}

impl CommandService for ProcessCommandService {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, Vec<CommandInfo>> {
        let host = Arc::clone(&self.host);
        let id = self.id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                process_service_call(&host, "command.list", &id, request)
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessSkillService {
    host: Arc<ProcessHost>,
    id: String,
}

impl SkillService for ProcessSkillService {
    fn list<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, Vec<SkillInfo>> {
        let host = Arc::clone(&self.host);
        let id = self.id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                process_service_call(&host, "skill.list", &id, request)
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessConfigService {
    host: Arc<ProcessHost>,
    id: String,
}

impl ConfigService for ProcessConfigService {
    fn load<'a>(&'a self, request: ServiceRequest) -> PluginFuture<'a, ConfigDocument> {
        let host = Arc::clone(&self.host);
        let id = self.id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                process_service_call(&host, "config.load", &id, request)
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessSystemContextService {
    host: Arc<ProcessHost>,
    id: String,
}

impl SystemContextService for ProcessSystemContextService {
    fn sections(
        &self,
        request: &ServiceRequest,
    ) -> Result<Vec<SystemContextSection>, PluginRuntimeError> {
        process_service_call(&self.host, "systemContext.sections", &self.id, request)
    }
}

struct ProcessPromptService {
    host: Arc<ProcessHost>,
    id: String,
}

impl PromptService for ProcessPromptService {
    fn render(
        &self,
        request: &PromptRequest,
    ) -> Result<RenderedPrompt, PluginRuntimeError> {
        process_service_call(&self.host, "prompt.render", &self.id, request)
    }
}

fn service_contribution<T: ?Sized>(
    declaration: &ProcessServiceDeclaration,
    service: Arc<T>,
) -> ServiceContribution<T> {
    ServiceContribution {
        metadata: ContributionMetadata::new(
            declaration.id.clone(),
            "unknown",
            PluginScope::Workspace,
        )
        .with_priority(declaration.priority),
        service,
    }
}

struct ProcessProviderService {
    host: Arc<ProcessHost>,
    declaration: ProcessProviderDeclaration,
}

impl ProviderService for ProcessProviderService {
    fn descriptor(&self) -> ProviderDescriptor {
        self.declaration.descriptor.clone()
    }

    fn stream<'a>(
        &'a self,
        request: ProviderGenerationRequest,
    ) -> PluginFuture<'a, ProviderStream> {
        let host = Arc::clone(&self.host);
        let service_id = self.declaration.descriptor.id.clone();
        Box::pin(async move {
            let provider_id = request.provider_id.clone();
            let model_id = request.model_id.clone();
            let (stream_id, request_id, stream) =
                host.allocate_stream().map_err(PluginRuntimeError::new)?;
            let params = serde_json::to_value(ProcessProviderStreamRequest {
                service_id,
                stream_id: stream_id.clone(),
                request_id,
                request,
            })
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let call_host = Arc::clone(&host);
            let result = tokio::task::spawn_blocking(move || {
                call_host.call("provider.stream", params, call_host.spec.call_timeout)
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            if let Err(error) = result {
                host.cancel_stream(&stream_id, &error);
                return Err(PluginRuntimeError::new(error));
            }
            let events: ProviderEventStream = Box::pin(stream.map(|item| {
                item.and_then(|value| {
                    serde_json::from_value::<ProviderStreamEvent>(value).map_err(
                        |error| {
                            PluginRuntimeError::new(format!(
                                "invalid provider stream item: {error}"
                            ))
                        },
                    )
                })
            }));
            let _ = request_id;
            Ok(ProviderStream {
                provider_id,
                model_id,
                events,
            })
        })
    }

    fn model_metadata<'a>(
        &'a self,
        model: &'a UserModel,
    ) -> PluginFuture<'a, ProviderModelMetadata> {
        let host = Arc::clone(&self.host);
        let request = ProcessProviderMetadataRequest {
            service_id: self.declaration.descriptor.id.clone(),
            model: model.clone(),
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let params = serde_json::to_value(request)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let value = host
                    .call("provider.metadata", params, host.spec.call_timeout)
                    .map_err(PluginRuntimeError::new)?;
                serde_json::from_value(value)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }

    fn auth<'a>(&'a self, provider_id: &'a str) -> PluginFuture<'a, Option<AuthInfo>> {
        let host = Arc::clone(&self.host);
        let request = ProcessProviderAuthRequest {
            service_id: self.declaration.descriptor.id.clone(),
            provider_id: provider_id.to_string(),
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let params = serde_json::to_value(request)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let value = host
                    .call("provider.auth", params, host.spec.call_timeout)
                    .map_err(PluginRuntimeError::new)?;
                serde_json::from_value(value)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }

    fn route<'a>(
        &'a self,
        request: ProviderRouteRequest,
    ) -> PluginFuture<'a, RouteResponse> {
        let host = Arc::clone(&self.host);
        let request = ProcessProviderRouteCall {
            service_id: self.declaration.descriptor.id.clone(),
            request,
        };
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let params = serde_json::to_value(request)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let value = host
                    .call("provider.route", params, host.spec.call_timeout)
                    .map_err(PluginRuntimeError::new)?;
                serde_json::from_value(value)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }

    fn generate_media<'a>(
        &'a self,
        request: MediaGenerationRequest,
    ) -> PluginFuture<'a, GeneratedMedia> {
        let host = Arc::clone(&self.host);
        let service_id = self.declaration.descriptor.id.clone();
        Box::pin(async move {
            let cancel = request.cancel;
            let request = ProcessMediaRequest {
                service_id,
                kind: match request.kind {
                    MediaKind::Image => "image",
                    MediaKind::Video => "video",
                }
                .into(),
                provider_id: request.provider_id,
                model_id: request.model_id,
                connection_id: request.connection_id,
                tenant_id: request.tenant_id,
                workspace_id: request.workspace_id,
                prompt: request.prompt,
                options: request.options,
            };
            tokio::task::spawn_blocking(move || {
                let params = serde_json::to_value(request)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let value = host
                    .call_with_cancel(
                        "provider.media",
                        params,
                        host.spec.call_timeout,
                        cancel.as_deref(),
                    )
                    .map_err(PluginRuntimeError::new)?;
                let result: ProcessMediaResult = serde_json::from_value(value)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(result.data_base64)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                if bytes.len() > MAX_MEDIA_BYTES {
                    return Err(PluginRuntimeError::new(format!(
                        "plugin media exceeds {MAX_MEDIA_BYTES} bytes"
                    )));
                }
                Ok(GeneratedMedia {
                    bytes,
                    mime: result.mime,
                    filename: result.filename,
                    revised_prompt: result.revised_prompt,
                })
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessRoute {
    host: Arc<ProcessHost>,
    id: String,
}

impl RouteHandler for ProcessRoute {
    fn handle<'a>(&'a self, request: RouteRequest) -> PluginFuture<'a, RouteResponse> {
        let host = Arc::clone(&self.host);
        let route_id = self.id.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let params = serde_json::to_value(ProcessRouteCall { route_id, request })
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                let value = host
                    .call("route.handle", params, host.spec.call_timeout)
                    .map_err(PluginRuntimeError::new)?;
                serde_json::from_value(value)
                    .map_err(|error| PluginRuntimeError::new(error.to_string()))
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
        })
    }
}

struct ProcessWebSocketRoute {
    host: Arc<ProcessHost>,
    id: String,
}

struct ProcessWebSocketBridge {
    host: Arc<ProcessHost>,
    stream_id: String,
    stream: Mutex<Option<ProcessValueStream>>,
}

impl WebSocketRouteHandler for ProcessWebSocketRoute {
    fn prepare<'a>(
        &'a self,
        request: RouteRequest,
    ) -> PluginFuture<'a, Arc<dyn WebSocketSession>> {
        let host = Arc::clone(&self.host);
        let route_id = self.id.clone();
        Box::pin(async move {
            let (stream_id, request_id, stream) =
                host.allocate_stream().map_err(PluginRuntimeError::new)?;
            let params = serde_json::to_value(ProcessWebSocketOpenRequest {
                route_id,
                stream_id: stream_id.clone(),
                request_id,
                request,
            })
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let call_host = Arc::clone(&host);
            let result = tokio::task::spawn_blocking(move || {
                call_host.call(
                    "route.websocket.open",
                    params,
                    call_host.spec.call_timeout,
                )
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            if let Err(error) = result {
                host.cancel_stream(&stream_id, &error);
                return Err(PluginRuntimeError::new(error));
            }
            let _ = request_id;
            Ok(Arc::new(ProcessWebSocketBridge {
                host,
                stream_id,
                stream: Mutex::new(Some(stream)),
            }) as Arc<dyn WebSocketSession>)
        })
    }
}

impl WebSocketSession for ProcessWebSocketBridge {
    fn run<'a>(
        &'a self,
        mut socket: Box<dyn neoism_agent_plugin_api::PluginWebSocket>,
    ) -> PluginFuture<'a, ()> {
        let mut stream = self
            .stream
            .lock()
            .expect("websocket stream poisoned")
            .take();
        let host = Arc::clone(&self.host);
        let stream_id = self.stream_id.clone();
        Box::pin(async move {
            let Some(mut stream) = stream.take() else {
                return Err(PluginRuntimeError::new(
                    "websocket session already consumed",
                ));
            };
            loop {
                tokio::select! {
                    inbound = socket.receive() => {
                        let Some(message) = inbound? else { break };
                        host.notify("route.websocket.message", serde_json::to_value(ProcessWebSocketMessageRequest {
                            stream_id: stream_id.clone(),
                            message: message.into(),
                        }).map_err(|error| PluginRuntimeError::new(error.to_string()))?);
                    }
                    outbound = stream.next() => {
                        let Some(value) = outbound else { break };
                        let message: ProcessWebSocketMessage = serde_json::from_value(value?)
                            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
                        let close = matches!(message, ProcessWebSocketMessage::Close);
                        socket.send(message.into()).await?;
                        if close { break; }
                    }
                }
            }
            host.cancel_stream(&stream_id, "websocket session closed");
            Ok(())
        })
    }
}

impl RuntimeTool for ProcessTool {
    fn definition(&self) -> PluginToolDefinition {
        self.definition.clone()
    }

    fn execute<'a>(
        &'a self,
        invocation: PluginToolInvocation,
    ) -> PluginFuture<'a, PluginToolResult> {
        let host = Arc::clone(&self.host);
        let tool = self.definition.id.clone();
        Box::pin(async move {
            let timeout = host.spec.call_timeout;
            let cancel = invocation.cancel;
            let params = serde_json::to_value(ProcessToolInvokeRequest {
                tool: tool.clone(),
                directory: invocation.directory,
                session_id: invocation.session_id,
                input: invocation.arguments,
            })
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
            let reply = tokio::task::spawn_blocking(move || {
                host.call_with_cancel("tool.invoke", params, timeout, cancel.as_deref())
            })
            .await
            .map_err(|error| PluginRuntimeError::new(error.to_string()))?
            .map_err(PluginRuntimeError::new)?;
            let output = match reply.get("output") {
                Some(Value::String(text)) => text.clone(),
                Some(other) => serde_json::to_string_pretty(other).unwrap_or_default(),
                None => String::new(),
            };
            Ok(PluginToolResult {
                title: reply
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or(&tool)
                    .to_string(),
                output,
                metadata: reply.get("metadata").cloned(),
            })
        })
    }
}

struct ProcessHookBridge {
    host: Arc<ProcessHost>,
    hooks: std::collections::BTreeSet<String>,
    event_namespaces: Vec<String>,
}

impl RuntimeHook for ProcessHookBridge {
    fn invoke(
        &self,
        hook: &str,
        context: Value,
        value: Value,
    ) -> Result<Value, PluginRuntimeError> {
        if hook == "event" {
            let subscribed = self.hooks.contains("event")
                || self.event_namespaces.iter().any(|namespace| {
                    context
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with(namespace.as_str()))
                });
            if subscribed {
                self.host.notify("event", context);
            }
            return Ok(value);
        }
        if !self.hooks.contains(hook) {
            return Ok(value);
        }
        self.host
            .call(
                "hook.invoke",
                json!({ "hook": hook, "context": context, "value": value }),
                self.host.spec.call_timeout,
            )
            .map_err(PluginRuntimeError::new)
    }
}

// ---------------------------------------------------------------------------
// Factory / instance
// ---------------------------------------------------------------------------

pub(crate) struct ServePluginFactory {
    spec: ServePluginSpec,
    executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
}

impl ServePluginFactory {
    pub(crate) fn new(
        spec: ServePluginSpec,
        executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    ) -> Self {
        Self { spec, executables }
    }
}

impl PluginFactory for ServePluginFactory {
    fn descriptor(&self) -> PluginDescriptor {
        let mut capabilities = Vec::new();
        if self.spec.network {
            capabilities.push(HostCapability::Network);
        }
        capabilities.extend(self.spec.requested_capabilities.iter().copied());
        capabilities.sort();
        capabilities.dedup();
        let capability_names = capabilities
            .iter()
            .filter_map(|capability| {
                serde_json::to_value(capability)
                    .ok()?
                    .as_str()
                    .map(str::to_string)
            })
            .collect();
        PluginDescriptor {
            manifest: PluginManifest {
                id: self.spec.id.clone(),
                name: self.spec.id.clone(),
                version: "0.0.0".to_string(),
                internal: false,
                disableable: true,
                capabilities: capability_names,
                requires: Vec::new(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: self
                    .spec
                    .package_revision
                    .as_ref()
                    .map(|revision| {
                        BTreeMap::from([(
                            "packageRevision".into(),
                            Value::String(revision.clone()),
                        )])
                    })
                    .unwrap_or_default(),
            },
            scope: self.spec.scope,
            required_capabilities: capabilities,
            plugin_api_major: neoism_agent_plugin_api::PLUGIN_API_MAJOR,
        }
    }

    fn create<'a>(
        &'a self,
        context: PluginContext,
    ) -> PluginFuture<'a, Box<dyn PluginInstance>> {
        Box::pin(async move {
            Ok(Box::new(ServePluginInstance {
                host: Arc::new(ProcessHost::new(
                    self.spec.clone(),
                    Arc::clone(&self.executables),
                    context,
                )),
                state: Mutex::new(ServeState::Starting),
            }) as Box<dyn PluginInstance>)
        })
    }
}

enum ServeState {
    Starting,
    Ready(ProcessInitializeResponse),
    Degraded(String),
}

pub(crate) struct ServePluginInstance {
    host: Arc<ProcessHost>,
    state: Mutex<ServeState>,
}

impl PluginInstance for ServePluginInstance {
    fn start<'a>(&'a self) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            let host = Arc::clone(&self.host);
            let outcome =
                tokio::task::spawn_blocking(move || host.spawn_and_initialize()).await;
            let mut state = self.state.lock().expect("serve state poisoned");
            *state = match outcome {
                Ok(Ok(handshake)) => ServeState::Ready(handshake),
                Ok(Err(reason)) if self.host.spec.strict_startup => {
                    return Err(PluginRuntimeError::new(reason));
                }
                Ok(Err(reason)) => {
                    tracing::warn!(plugin = %self.host.spec.id, %reason, "serve plugin unavailable");
                    ServeState::Degraded(reason)
                }
                Err(join_error) => ServeState::Degraded(join_error.to_string()),
            };
            Ok(())
        })
    }

    fn readiness(&self) -> PluginReadiness {
        match &*self.state.lock().expect("serve state poisoned") {
            ServeState::Starting => PluginReadiness {
                state: ReadinessState::Starting,
                reason: None,
            },
            ServeState::Ready(_) => PluginReadiness::ready(),
            ServeState::Degraded(reason) => PluginReadiness {
                state: ReadinessState::Degraded,
                reason: Some(reason.clone()),
            },
        }
    }

    fn contributions(&self) -> PluginContributions {
        let mut contributions = PluginContributions::default();
        let state = self.state.lock().expect("serve state poisoned");
        let ServeState::Ready(handshake) = &*state else {
            return contributions;
        };
        for definition in &handshake.tools {
            contributions.runtime_tool(Arc::new(ProcessTool {
                host: Arc::clone(&self.host),
                definition: definition.clone(),
            }));
        }
        for declaration in &handshake.services.agents {
            contributions.agents.push(service_contribution(
                declaration,
                Arc::new(ProcessAgentService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.commands {
            contributions.commands.push(service_contribution(
                declaration,
                Arc::new(ProcessCommandService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.skills {
            contributions.skills.push(service_contribution(
                declaration,
                Arc::new(ProcessSkillService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.config {
            contributions.config.push(service_contribution(
                declaration,
                Arc::new(ProcessConfigService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.system_context {
            contributions.system_context.push(service_contribution(
                declaration,
                Arc::new(ProcessSystemContextService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.prompts {
            contributions.prompts.push(service_contribution(
                declaration,
                Arc::new(ProcessPromptService {
                    host: Arc::clone(&self.host),
                    id: declaration.id.clone(),
                }),
            ));
        }
        for declaration in &handshake.services.providers {
            contributions.providers.push(service_contribution(
                &ProcessServiceDeclaration {
                    id: declaration.descriptor.id.clone(),
                    priority: declaration.priority,
                },
                Arc::new(ProcessProviderService {
                    host: Arc::clone(&self.host),
                    declaration: declaration.clone(),
                }),
            ));
        }
        let canonical_prefix = format!("/v2/plugins/{}", self.host.spec.id);
        for declaration in &handshake.routes {
            let mut descriptor = declaration.descriptor.clone();
            if !descriptor.path.starts_with(&canonical_prefix) {
                descriptor.path = format!("{canonical_prefix}{}", descriptor.path);
            }
            contributions.routes.push(RouteContribution {
                metadata: ContributionMetadata::new(
                    descriptor.id.clone(),
                    "unknown",
                    PluginScope::Workspace,
                )
                .with_priority(declaration.priority),
                handler: Arc::new(ProcessRoute {
                    host: Arc::clone(&self.host),
                    id: descriptor.id.clone(),
                }),
                descriptor,
            });
        }
        for declaration in &handshake.websocket_routes {
            let mut descriptor = declaration.descriptor.clone();
            if !descriptor.path.starts_with(&canonical_prefix) {
                descriptor.path = format!("{canonical_prefix}{}", descriptor.path);
            }
            contributions
                .websocket_routes
                .push(WebSocketRouteContribution {
                    metadata: ContributionMetadata::new(
                        descriptor.id.clone(),
                        "unknown",
                        PluginScope::Workspace,
                    )
                    .with_priority(declaration.priority),
                    handler: Arc::new(ProcessWebSocketRoute {
                        host: Arc::clone(&self.host),
                        id: descriptor.id.clone(),
                    }),
                    descriptor,
                });
        }
        for part in &handshake.message_parts {
            contributions.part(
                part.id.clone(),
                Some(json!({
                    "version": part.version,
                    "schema": part.schema,
                    "fallbackTextField": part.fallback_text_field,
                })),
            );
        }
        for mcp in &handshake.mcp {
            contributions.contribute(neoism_agent_plugin_api::Contribution {
                kind: neoism_agent_plugin_api::ContributionKind::Mcp,
                id: mcp.id.clone(),
                schema: serde_json::to_value(mcp).ok(),
            });
        }
        let hooks = handshake
            .hooks
            .iter()
            .cloned()
            .collect::<std::collections::BTreeSet<_>>();
        if !hooks.is_empty() || !handshake.event_namespaces.is_empty() {
            for hook in &hooks {
                contributions.hook(hook.clone());
            }
            contributions.runtime_hook(Arc::new(ProcessHookBridge {
                host: Arc::clone(&self.host),
                hooks,
                event_namespaces: handshake.event_namespaces.clone(),
            }));
        }
        let _ = (&handshake.name, &handshake.version);
        contributions
    }

    fn shutdown<'a>(&'a self) -> PluginFuture<'a, ()> {
        Box::pin(async move {
            let host = Arc::clone(&self.host);
            let _ = tokio::task::spawn_blocking(move || host.shutdown_sync()).await;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestBroker {
        output: Value,
        cancelled: Arc<AtomicBool>,
    }

    impl neoism_agent_plugin_api::CapabilityBroker for TestBroker {
        fn call(
            &self,
            _request: BrokerRequest,
            lease: neoism_agent_plugin_api::CapabilityLease,
        ) -> Result<neoism_agent_plugin_api::BrokerResponse, PluginRuntimeError> {
            if !lease.is_active() {
                return Err(PluginRuntimeError::new("generation revoked"));
            }
            Ok(neoism_agent_plugin_api::BrokerResponse {
                output: self.output.clone(),
            })
        }

        fn cancel(
            &self,
            _opaque_id: &str,
            _owner: Option<BrokerOwner>,
            lease: neoism_agent_plugin_api::CapabilityLease,
        ) -> Result<(), PluginRuntimeError> {
            if !lease.is_active() {
                return Err(PluginRuntimeError::new("generation revoked"));
            }
            self.cancelled.store(true, Ordering::Release);
            Ok(())
        }
    }

    struct OwnerBroker;
    impl neoism_agent_plugin_api::CapabilityBroker for OwnerBroker {
        fn call(
            &self,
            request: BrokerRequest,
            lease: neoism_agent_plugin_api::CapabilityLease,
        ) -> Result<neoism_agent_plugin_api::BrokerResponse, PluginRuntimeError> {
            let owner = request
                .owner
                .ok_or_else(|| PluginRuntimeError::new("owner missing"))?;
            if owner.registry_generation != 7
                || owner.scope_id.as_deref() != Some("session-1")
                || !lease.is_active()
            {
                return Err(PluginRuntimeError::new("owner mismatch"));
            }
            Ok(neoism_agent_plugin_api::BrokerResponse {
                output: json!({"ok":true}),
            })
        }
    }

    struct TestConfig;

    impl neoism_agent_plugin_api::ConfigAccess for TestConfig {
        fn get(&self, key: &str) -> Result<Option<Value>, PluginRuntimeError> {
            Ok((key == "allowed").then(|| json!("value")))
        }

        fn set(&self, _key: &str, _value: Value) -> Result<(), PluginRuntimeError> {
            Ok(())
        }
    }

    fn spec_with_command(command: Vec<String>) -> ServePluginSpec {
        ServePluginSpec {
            id: "dev.example.serve".to_string(),
            source: ServeSource::Command(command),
            env: BTreeMap::new(),
            config: json!({ "greeting": "hello" }),
            call_timeout: Duration::from_secs(5),
            sandbox: SandboxPolicy::Off,
            network: true,
            working_directory: std::env::temp_dir(),
            requested_capabilities: Vec::new(),
            package_revision: None,
            scope: PluginScope::Workspace,
            strict_startup: false,
        }
    }

    fn standard_executables() -> Arc<dyn neoism_agent_service_api::ExecutableService> {
        Arc::new(neoism_agent_service_api::StandardExecutableService)
    }

    fn test_context() -> neoism_agent_plugin_api::PluginContext {
        neoism_agent_plugin_api::PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Workspace(
                neoism_agent_plugin_api::WorkspaceIdentity {
                    id: "workspace".into(),
                    root: ".".into(),
                },
            ),
            neoism_agent_plugin_api::CapabilityGrants::default()
                .allow(HostCapability::ProcessSpawn)
                .allow(HostCapability::WorkspaceRead)
                .allow(HostCapability::Network),
        )
    }

    fn node_available() -> bool {
        std::process::Command::new("node")
            .arg("--version")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }

    const FIXTURE: &str = r#"
const readline = require("node:readline");
const rl = readline.createInterface({ input: process.stdin });
rl.on("line", (line) => {
  const frame = JSON.parse(line);
  const reply = (result) =>
    process.stdout.write(JSON.stringify({ id: frame.id, result }) + "\n");
  if (frame.method === "initialize") {
    reply({
      protocol: "neoism-plugin/2",
      name: "fixture",
      tools: [{ id: "fixture_echo", description: "echo", parameters: { type: "object" } }],
      hooks: ["chat.options"],
      eventNamespaces: ["session."],
      services: {
        agents: [{ id: "fixture.agents" }],
        commands: [{ id: "fixture.commands", priority: 7 }],
        skills: [{ id: "fixture.skills" }],
        systemContext: [{ id: "fixture.context" }],
        prompts: [{ id: "fixture.prompts" }],
        config: [{ id: "fixture.config" }],
      },
    });
  } else if (frame.method === "tool.invoke") {
    reply({ title: "echoed", output: "echo:" + frame.params.input.text });
  } else if (frame.method === "hook.invoke") {
    const value = frame.params.value;
    value.fixture = true;
    reply(value);
  } else if (frame.method === "command.list") {
    reply([{ name: "fixture", description: "from process" }]);
  } else if (frame.method === "agent.list") {
    reply({ agents: [{ name: "fixture", mode: "primary" }], defaultAgent: "fixture" });
  } else if (frame.method === "skill.list") {
    reply([{ id: "fixture", name: "Fixture" }]);
  } else if (frame.method === "systemContext.sections") {
    reply([{ id: "fixture", title: "Fixture", content: "process context" }]);
  } else if (frame.method === "prompt.render") {
    reply({ content: "rendered:" + frame.params.request.promptId, system: true });
  } else if (frame.method === "config.load") {
    reply({ values: { fixture: true }, provenance: { fixture: "process" } });
  } else if (frame.method === "event") {
    require("node:fs").writeFileSync(process.env.NEOISM_EVENT_LOG, JSON.stringify(frame.params));
  } else if (frame.method === "shutdown") {
    process.exit(0);
  }
});
"#;

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn serve_plugin_handshakes_executes_tools_and_hooks() {
        if !node_available() {
            eprintln!("skipping: node unavailable");
            return;
        }
        let fixture = std::env::temp_dir().join(format!(
            "neoism-serve-fixture-{}.cjs",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::write(&fixture, FIXTURE).unwrap();
        let spec = spec_with_command(vec![
            "node".to_string(),
            fixture.to_string_lossy().into_owned(),
        ]);
        let event_log = fixture.with_extension("event.json");
        let mut spec = spec;
        spec.env.insert(
            "NEOISM_EVENT_LOG".into(),
            event_log.to_string_lossy().into_owned(),
        );
        let factory = ServePluginFactory::new(spec, standard_executables());
        let instance = factory.create(test_context()).await.unwrap();
        instance.start().await.unwrap();
        assert_eq!(instance.readiness(), PluginReadiness::ready());

        let contributions = instance.contributions();
        let tool = contributions
            .runtime_tools
            .get("fixture_echo")
            .expect("fixture tool registered");
        let result = tool
            .execute(PluginToolInvocation {
                tenant_id: "local".to_string(),
                subject: None,
                workspace_id: None,
                execution_mode: neoism_agent_plugin_api::PluginExecutionMode::NativeLocal,
                directory: "/tmp".to_string(),
                session_id: Some("ses_test".to_string()),
                arguments: json!({ "text": "hi" }),
                permission_rules: Vec::new(),
                env: BTreeMap::new(),
                cancel: None,
                formatter: None,
                generation: None,
            })
            .await
            .unwrap();
        assert_eq!(result.output, "echo:hi");
        assert_eq!(result.title, "echoed");

        let hook = contributions.runtime_hooks.first().expect("hook bridge");
        let value = hook
            .invoke("chat.options", json!({}), json!({ "existing": 1 }))
            .unwrap();
        assert_eq!(value["fixture"], json!(true));
        assert_eq!(value["existing"], json!(1));
        // Unsubscribed hooks pass through untouched, without a round trip.
        let untouched = hook
            .invoke("chat.headers", json!({}), json!({ "keep": true }))
            .unwrap();
        assert_eq!(untouched, json!({ "keep": true }));

        let commands = contributions.commands[0]
            .service
            .list(ServiceRequest::default())
            .await
            .unwrap();
        assert_eq!(commands[0].name, "fixture");
        assert_eq!(contributions.commands[0].metadata.priority, 7);
        let agents = contributions.agents[0]
            .service
            .list(ServiceRequest::default())
            .await
            .unwrap();
        assert_eq!(agents.default_agent.as_deref(), Some("fixture"));
        let skills = contributions.skills[0]
            .service
            .list(ServiceRequest::default())
            .await
            .unwrap();
        assert_eq!(skills[0].id, "fixture");
        let sections = contributions.system_context[0]
            .service
            .sections(&ServiceRequest::default())
            .unwrap();
        assert_eq!(sections[0].content, "process context");
        let prompt = contributions.prompts[0]
            .service
            .render(&PromptRequest {
                prompt_id: "hello".into(),
                variables: BTreeMap::new(),
                service: ServiceRequest::default(),
            })
            .unwrap();
        assert_eq!(prompt.content, "rendered:hello");
        let config = contributions.config[0]
            .service
            .load(ServiceRequest::default())
            .await
            .unwrap();
        assert_eq!(config.values["fixture"], true);

        let event = json!({
            "id": "evt_test",
            "type": "session.updated",
            "sequence": 3,
            "properties": {"sessionId": "ses_test"}
        });
        assert_eq!(
            hook.invoke("event", event.clone(), Value::Null).unwrap(),
            Value::Null
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        let delivered = loop {
            if let Ok(raw) = std::fs::read_to_string(&event_log) {
                break serde_json::from_str::<Value>(&raw).unwrap();
            }
            assert!(Instant::now() < deadline, "event notification timed out");
            std::thread::sleep(Duration::from_millis(10));
        };
        assert_eq!(delivered, event);

        instance.shutdown().await.unwrap();

        // Process adapters enter the registry only through PluginHost's normal
        // contribution merge/conflict/lifecycle path.
        let installed = neoism_agent_plugin_api::PluginHost::default()
            .install(vec![Box::new(factory)], &[], test_context())
            .await
            .unwrap();
        let snapshot = installed.snapshot();
        let commands = snapshot.command_services["fixture.commands"]
            .list(ServiceRequest::default())
            .await
            .unwrap();
        assert_eq!(commands[0].name, "fixture");
        assert_eq!(
            snapshot.service_metadata["CommandService:fixture.commands"].priority,
            7
        );
        installed.shutdown().await.unwrap();
        let _ = std::fs::remove_file(fixture);
        let _ = std::fs::remove_file(event_log);
    }

    #[cfg(unix)]
    #[tokio::test(flavor = "multi_thread")]
    async fn unresponsive_process_callback_times_out_and_is_torn_down() {
        if !node_available() {
            eprintln!("skipping: node unavailable");
            return;
        }
        let fixture = std::env::temp_dir().join(format!(
            "neoism-serve-timeout-{}.cjs",
            neoism_agent_core::Id::ascending(neoism_agent_core::IdKind::Event)
        ));
        std::fs::write(
            &fixture,
            r#"const readline=require('node:readline');const rl=readline.createInterface({input:process.stdin});rl.on('line',line=>{const f=JSON.parse(line);if(f.method==='initialize')process.stdout.write(JSON.stringify({id:f.id,result:{protocol:'neoism-plugin/2',tools:[{id:'hang',description:'hang',parameters:{type:'object'}}]}})+'\n');if(f.method==='shutdown')process.exit(0);});"#,
        )
        .unwrap();
        let mut spec = spec_with_command(vec![
            "node".into(),
            fixture.to_string_lossy().into_owned(),
        ]);
        spec.call_timeout = Duration::from_millis(25);
        let instance = ServePluginFactory::new(spec, standard_executables())
            .create(test_context())
            .await
            .unwrap();
        instance.start().await.unwrap();
        let tool = instance.contributions().runtime_tools["hang"].clone();
        let error = tool
            .execute(PluginToolInvocation {
                tenant_id: "local".into(),
                subject: None,
                workspace_id: Some("workspace".into()),
                execution_mode: neoism_agent_plugin_api::PluginExecutionMode::Sandboxed,
                directory: ".".into(),
                session_id: None,
                arguments: json!({}),
                permission_rules: Vec::new(),
                env: BTreeMap::new(),
                cancel: Some(Arc::new(AtomicBool::new(false))),
                formatter: None,
                generation: None,
            })
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        instance.shutdown().await.unwrap();
        let _ = std::fs::remove_file(fixture);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_binary_degrades_instead_of_failing_install() {
        let spec = spec_with_command(vec!["neoism-definitely-not-a-binary".to_string()]);
        let factory = ServePluginFactory::new(spec, standard_executables());
        let instance = factory.create(test_context()).await.unwrap();
        instance.start().await.unwrap();
        let readiness = instance.readiness();
        assert_eq!(readiness.state, ReadinessState::Degraded);
        assert!(readiness.reason.is_some());
        assert!(instance.contributions().runtime_tools.is_empty());
        instance.shutdown().await.unwrap();
    }

    #[test]
    fn serve_spec_parses_all_three_sources() {
        let mut options = BTreeMap::new();
        options.insert("serve".to_string(), json!(["python3", "x.py"]));
        options.insert("network".to_string(), json!(false));
        options.insert("timeoutMs".to_string(), json!(5_000));
        let spec = serve_plugin_spec("dev.a", "/w", &options).unwrap();
        assert!(matches!(&spec.source, ServeSource::Command(c) if c.len() == 2));
        assert!(!spec.network);
        assert_eq!(spec.call_timeout, Duration::from_secs(5));

        let mut options = BTreeMap::new();
        options.insert("npm".to_string(), json!("@example/plugin@1.0.0"));
        let spec = serve_plugin_spec("dev.b", "/w", &options).unwrap();
        assert!(matches!(&spec.source, ServeSource::Npm(_)));
        assert!(spec.network, "serve plugins default to networked");

        let mut options = BTreeMap::new();
        options.insert("entry".to_string(), json!("./plugins/todo"));
        assert!(serve_plugin_spec("dev.c", "/w", &options).is_some());

        assert!(serve_plugin_spec("dev.d", "/w", &BTreeMap::new()).is_none());
    }

    #[test]
    fn npm_package_names_strip_version_suffixes_only() {
        assert_eq!(npm_package_name("@scope/name@1.2.3"), "@scope/name");
        assert_eq!(npm_package_name("name@1.2.3"), "name");
        assert_eq!(npm_package_name("name"), "name");
        assert_eq!(npm_package_name("@scope/name"), "@scope/name");
    }

    #[test]
    fn bounded_line_reader_discards_oversized_frames_and_recovers() {
        let input = format!("{}\n{{\"id\":2}}\n", "x".repeat(9));
        let mut reader = BufReader::new(input.as_bytes());
        assert!(matches!(
            read_bounded_line(&mut reader, 8).unwrap(),
            BoundedLine::Oversized
        ));
        let BoundedLine::Line(line) = read_bounded_line(&mut reader, 32).unwrap() else {
            panic!("expected the next bounded line");
        };
        assert_eq!(line, br#"{"id":2}"#);
        assert!(matches!(
            read_bounded_line(&mut reader, 32).unwrap(),
            BoundedLine::Eof
        ));
    }

    #[test]
    fn process_instances_have_distinct_exact_owners() {
        let first = ProcessHost::new(
            spec_with_command(vec!["first".into()]),
            standard_executables(),
            test_context(),
        );
        let second = ProcessHost::new(
            spec_with_command(vec!["second".into()]),
            standard_executables(),
            test_context(),
        );
        assert_eq!(first.owner.plugin_id, second.owner.plugin_id);
        assert_ne!(first.owner.instance_id, second.owner.instance_id);
    }

    #[test]
    fn reverse_requests_require_the_live_exact_instance_owner() {
        let expected = ProcessPluginOwner {
            plugin_id: "dev.example.serve".into(),
            instance_id: "instance-2".into(),
            package_revision: None,
            registry_generation: None,
            scope: None,
            workspace_id: None,
            scope_id: None,
        };
        assert!(validate_reverse_owner(Some(&expected), &expected, true).is_ok());
        let wrong = ProcessPluginOwner {
            plugin_id: expected.plugin_id.clone(),
            instance_id: "instance-1".into(),
            package_revision: None,
            registry_generation: None,
            scope: None,
            workspace_id: None,
            scope_id: None,
        };
        assert!(validate_reverse_owner(Some(&wrong), &expected, true)
            .unwrap_err()
            .contains("wrong owner"));
        assert!(validate_reverse_owner(None, &expected, true).is_err());
        assert!(validate_reverse_owner(Some(&expected), &expected, false)
            .unwrap_err()
            .contains("retired"));
        let mut wrong_session = expected.clone();
        wrong_session.scope = Some(PluginScope::Session);
        wrong_session.scope_id = Some("session-other".into());
        assert!(
            validate_reverse_owner(Some(&wrong_session), &expected, true)
                .unwrap_err()
                .contains("wrong owner")
        );
    }

    #[test]
    fn reverse_host_services_use_attenuated_plugin_context_grants() {
        let context = PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Workspace(
                neoism_agent_plugin_api::WorkspaceIdentity {
                    id: "workspace".into(),
                    root: ".".into(),
                },
            ),
            neoism_agent_plugin_api::CapabilityGrants::default()
                .config(Arc::new(TestConfig), false),
        );
        assert_eq!(
            dispatch_reverse_request(
                &context,
                "host.config.get",
                json!({"key": "allowed"}),
            )
            .unwrap(),
            json!("value")
        );
        assert!(dispatch_reverse_request(
            &context,
            "host.config.set",
            json!({"key": "allowed", "value": false}),
        )
        .unwrap_err()
        .contains("ConfigWrite"));
        assert!(dispatch_reverse_request(
            &context,
            "host.workspace.list",
            json!({"path": "."}),
        )
        .unwrap_err()
        .contains("not available"));
    }

    #[test]
    fn brokers_are_bounded_cancellable_and_generation_revocable() {
        let cancelled = Arc::new(AtomicBool::new(false));
        let context = PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Session {
                workspace: neoism_agent_plugin_api::WorkspaceIdentity {
                    id: "workspace".into(),
                    root: ".".into(),
                },
                session_id: "session-1".into(),
            },
            neoism_agent_plugin_api::CapabilityGrants::default().broker(
                HostCapability::TaskSpawn,
                Arc::new(TestBroker {
                    output: json!({"taskId":"opaque-task-1"}),
                    cancelled: Arc::clone(&cancelled),
                }),
            ),
        );
        assert_eq!(
            dispatch_reverse_request(
                &context,
                "host.task.spawn",
                json!({"operation":"index","input":{"resource":"opaque:7"}}),
            )
            .unwrap()["output"]["taskId"],
            "opaque-task-1"
        );
        dispatch_reverse_request(
            &context,
            "host.task.cancel",
            json!({"opaqueId":"opaque-task-1"}),
        )
        .unwrap();
        assert!(cancelled.load(Ordering::Acquire));
        assert!(dispatch_reverse_request(
            &context,
            "host.task.spawn",
            json!({"operation":"index","input":"x".repeat(MAX_BROKER_INPUT_BYTES)}),
        )
        .unwrap_err()
        .contains("input exceeds"));

        context.revoke_capabilities();
        assert!(dispatch_reverse_request(
            &context,
            "host.task.spawn",
            json!({"operation":"index","input":{}}),
        )
        .unwrap_err()
        .contains("not granted"));
    }

    #[test]
    fn secret_use_does_not_imply_raw_secret_read() {
        let context = PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Global,
            neoism_agent_plugin_api::CapabilityGrants::default().broker(
                HostCapability::SecretUse,
                Arc::new(TestBroker {
                    output: json!({"authorizationHandle":"opaque-auth-1"}),
                    cancelled: Arc::new(AtomicBool::new(false)),
                }),
            ),
        );
        let used = dispatch_reverse_request(
            &context,
            "host.secret.use",
            json!({"operation":"sign-request","input":{"request":"opaque-request"}}),
        )
        .unwrap();
        assert_eq!(used["output"]["authorizationHandle"], "opaque-auth-1");
        assert!(dispatch_reverse_request(
            &context,
            "host.secret.read",
            json!({"operation":"reveal","input":{}}),
        )
        .unwrap_err()
        .contains("not available"));
    }

    #[test]
    fn broker_receives_exact_generation_and_session_owner() {
        let context = PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Session {
                workspace: neoism_agent_plugin_api::WorkspaceIdentity {
                    id: "workspace".into(),
                    root: ".".into(),
                },
                session_id: "session-1".into(),
            },
            neoism_agent_plugin_api::CapabilityGrants::default()
                .broker(HostCapability::PolicyInvoke, Arc::new(OwnerBroker)),
        );
        let owner = ProcessPluginOwner {
            plugin_id: "dev.example.policy".into(),
            instance_id: "instance-7".into(),
            package_revision: Some("sha256:test".into()),
            registry_generation: Some(7),
            scope: Some(PluginScope::Session),
            workspace_id: Some("workspace".into()),
            scope_id: Some("session-1".into()),
        };
        assert_eq!(
            dispatch_reverse_request_for_owner(
                &context,
                Some(&owner),
                "host.policy.call",
                json!({"operation":"permission-check","input":{}}),
            )
            .unwrap()["output"]["ok"],
            true
        );
    }

    fn stream_fixture() -> (
        Arc<Mutex<HashMap<String, ProcessStreamState>>>,
        ProcessPluginOwner,
        Arc<Mutex<Option<String>>>,
    ) {
        let owner = ProcessPluginOwner {
            plugin_id: "dev.example.stream".into(),
            instance_id: "generation-7".into(),
            package_revision: Some("sha256:test".into()),
            registry_generation: Some(7),
            scope: Some(neoism_agent_plugin_api::PluginScope::Workspace),
            workspace_id: Some("workspace".into()),
            scope_id: None,
        };
        let terminal = Arc::new(Mutex::new(None));
        let (sender, _receiver) = tokio::sync::mpsc::channel(MAX_STREAM_QUEUE_ITEMS);
        let streams = Arc::new(Mutex::new(HashMap::from([(
            "stream-1".into(),
            ProcessStreamState {
                request_id: 11,
                sender,
                terminal: Arc::clone(&terminal),
                opened: false,
                bytes: 0,
            },
        )])));
        (streams, owner, terminal)
    }

    #[test]
    fn stream_frames_are_bound_to_exact_generation_and_request() {
        let (streams, owner, terminal) = stream_fixture();
        let mut stale = owner.clone();
        stale.registry_generation = Some(6);
        handle_stream_frame(
            &streams,
            Some(&stale),
            &owner,
            Some(11),
            ProcessStreamEnvelope::Open {
                stream_id: "stream-1".into(),
            },
            true,
        );
        assert!(!streams.lock().unwrap()["stream-1"].opened);

        handle_stream_frame(
            &streams,
            Some(&owner),
            &owner,
            Some(12),
            ProcessStreamEnvelope::Open {
                stream_id: "stream-1".into(),
            },
            true,
        );
        assert!(streams.lock().unwrap().is_empty());
        assert_eq!(
            terminal.lock().unwrap().as_deref(),
            Some("stream request id mismatch")
        );
    }

    #[test]
    fn stream_requires_open_before_terminal_or_items() {
        let (streams, owner, terminal) = stream_fixture();
        handle_stream_frame(
            &streams,
            Some(&owner),
            &owner,
            Some(11),
            ProcessStreamEnvelope::End {
                stream_id: "stream-1".into(),
            },
            true,
        );
        assert!(streams.lock().unwrap().is_empty());
        assert_eq!(
            terminal.lock().unwrap().as_deref(),
            Some("plugin stream ended before open")
        );
    }
}
