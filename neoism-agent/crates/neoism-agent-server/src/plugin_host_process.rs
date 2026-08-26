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
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use neoism_agent_plugin_api::{
    HostCapability, PluginContext, PluginContributions, PluginDescriptor, PluginFactory,
    PluginFuture, PluginInstance, PluginManifest, PluginReadiness, PluginRuntimeError,
    PluginScope, PluginToolDefinition, PluginToolInvocation, PluginToolResult,
    ReadinessState, RuntimeHook, RuntimeTool,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::plugin::{build_plugin_command, sandbox_policy, SandboxPolicy};

pub(crate) const SERVE_PLUGIN_PROTOCOL: &str = "neoism-plugin/2";
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(8);
const SHUTDOWN_GRACE: Duration = Duration::from_millis(1500);

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
        network: options.get("network").and_then(Value::as_bool).unwrap_or(true),
        working_directory: PathBuf::from(directory),
    })
}

fn plugin_cache_dir() -> PathBuf {
    PathBuf::from(crate::default_state_dir()).join("plugin-cache")
}

fn npm_cache_slot(spec: &str) -> PathBuf {
    let sanitized = spec
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') { c } else { '_' })
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
    static IN_FLIGHT: OnceLock<Mutex<std::collections::BTreeSet<String>>> = OnceLock::new();
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

// ---------------------------------------------------------------------------
// Wire protocol
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct HostFrame<'a> {
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<u64>,
    method: &'a str,
    params: Value,
}

#[derive(Deserialize)]
struct PluginFrame {
    id: Option<u64>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginHandshake {
    protocol: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    tools: Vec<PluginToolDefinition>,
    #[serde(default)]
    hooks: Vec<String>,
    #[serde(default)]
    event_namespaces: Vec<String>,
}

// ---------------------------------------------------------------------------
// Process host
// ---------------------------------------------------------------------------

struct ProcessHost {
    spec: ServePluginSpec,
    executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    child: Mutex<Option<Child>>,
    stdin: Mutex<Option<ChildStdin>>,
    pending: Arc<Mutex<HashMap<u64, std::sync::mpsc::Sender<Result<Value, String>>>>>,
    next_id: AtomicU64,
}

impl ProcessHost {
    fn new(
        spec: ServePluginSpec,
        executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    ) -> Self {
        Self {
            spec,
            executables,
            child: Mutex::new(None),
            stdin: Mutex::new(None),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: AtomicU64::new(1),
        }
    }

    fn spawn_and_initialize(&self) -> Result<PluginHandshake, String> {
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
        let plugin_id = self.spec.id.clone();
        std::thread::Builder::new()
            .name(format!("neoism-plugin-io-{plugin_id}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(frame) = serde_json::from_str::<PluginFrame>(&line) else {
                        tracing::warn!(plugin = %plugin_id, "serve plugin wrote a non-protocol line");
                        continue;
                    };
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
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    tracing::info!(plugin = %stderr_plugin_id, "{line}");
                }
            })
            .ok();

        *self.stdin.lock().expect("stdin slot poisoned") = Some(stdin);
        *self.child.lock().expect("child slot poisoned") = Some(child);

        let handshake: PluginHandshake = serde_json::from_value(
            self.call(
                "initialize",
                json!({
                    "protocol": SERVE_PLUGIN_PROTOCOL,
                    "pluginId": self.spec.id,
                    "directory": self.spec.working_directory,
                    "config": self.spec.config,
                }),
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

    fn call(&self, method: &str, params: Value, timeout: Duration) -> Result<Value, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = std::sync::mpsc::channel();
        self.pending
            .lock()
            .expect("pending map poisoned")
            .insert(id, sender);
        if let Err(error) = self.write_frame(&HostFrame {
            id: Some(id),
            method,
            params,
        }) {
            self.pending
                .lock()
                .expect("pending map poisoned")
                .remove(&id);
            return Err(error);
        }
        match receiver.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(_) => {
                self.pending
                    .lock()
                    .expect("pending map poisoned")
                    .remove(&id);
                Err(format!(
                    "plugin {method} timed out after {} ms",
                    timeout.as_millis()
                ))
            }
        }
    }

    fn notify(&self, method: &str, params: Value) {
        let _ = self.write_frame(&HostFrame {
            id: None,
            method,
            params,
        });
    }

    fn write_frame(&self, frame: &HostFrame<'_>) -> Result<(), String> {
        let mut encoded = serde_json::to_vec(frame)
            .map_err(|error| format!("failed to encode plugin frame: {error}"))?;
        encoded.push(b'\n');
        let mut stdin = self.stdin.lock().expect("stdin slot poisoned");
        let Some(stdin) = stdin.as_mut() else {
            return Err("plugin process is not running".to_string());
        };
        stdin
            .write_all(&encoded)
            .and_then(|()| stdin.flush())
            .map_err(|error| format!("failed to write to plugin: {error}"))
    }

    fn shutdown_sync(&self) {
        self.notify("shutdown", json!({}));
        drop(self.stdin.lock().expect("stdin slot poisoned").take());
        let Some(mut child) = self.child.lock().expect("child slot poisoned").take() else {
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

impl Drop for ProcessHost {
    fn drop(&mut self) {
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
            let params = json!({
                "tool": tool,
                "directory": invocation.directory,
                "sessionId": invocation.session_id,
                "input": invocation.arguments,
            });
            let reply = tokio::task::spawn_blocking(move || {
                host.call("tool.invoke", params, timeout)
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
                    value
                        .get("type")
                        .and_then(Value::as_str)
                        .is_some_and(|kind| kind.starts_with(namespace.as_str()))
                });
            if subscribed {
                self.host.notify("event", value.clone());
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
        let mut capabilities = vec![HostCapability::ProcessSpawn, HostCapability::WorkspaceRead];
        if self.spec.network {
            capabilities.push(HostCapability::Network);
        }
        PluginDescriptor {
            manifest: PluginManifest {
                id: self.spec.id.clone(),
                name: self.spec.id.clone(),
                version: "0.0.0".to_string(),
                internal: false,
                disableable: true,
                capabilities: Vec::new(),
                requires: Vec::new(),
                event_namespaces: Vec::new(),
                api_prefix: None,
                config: BTreeMap::new(),
            },
            scope: PluginScope::Workspace,
            required_capabilities: capabilities,
            plugin_api_major: neoism_agent_plugin_api::PLUGIN_API_MAJOR,
        }
    }

    fn create<'a>(&'a self, _context: PluginContext) -> PluginFuture<'a, Box<dyn PluginInstance>> {
        Box::pin(async move {
            Ok(Box::new(ServePluginInstance {
                host: Arc::new(ProcessHost::new(
                    self.spec.clone(),
                    Arc::clone(&self.executables),
                )),
                state: Mutex::new(ServeState::Starting),
            }) as Box<dyn PluginInstance>)
        })
    }
}

enum ServeState {
    Starting,
    Ready(PluginHandshake),
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
            neoism_agent_plugin_api::CapabilityGrants::default(),
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
    });
  } else if (frame.method === "tool.invoke") {
    reply({ title: "echoed", output: "echo:" + frame.params.input.text });
  } else if (frame.method === "hook.invoke") {
    const value = frame.params.value;
    value.fixture = true;
    reply(value);
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
        let factory = ServePluginFactory::new(spec, standard_executables());
        let instance = factory
            .create(test_context())
            .await
            .unwrap();
        instance.start().await.unwrap();
        assert_eq!(instance.readiness(), PluginReadiness::ready());

        let contributions = instance.contributions();
        let tool = contributions
            .runtime_tools
            .get("fixture_echo")
            .expect("fixture tool registered");
        let result = tool
            .execute(PluginToolInvocation {
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

        instance.shutdown().await.unwrap();
        let _ = std::fs::remove_file(fixture);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn missing_binary_degrades_instead_of_failing_install() {
        let spec = spec_with_command(vec!["neoism-definitely-not-a-binary".to_string()]);
        let factory = ServePluginFactory::new(spec, standard_executables());
        let instance = factory
            .create(test_context())
            .await
            .unwrap();
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
}
