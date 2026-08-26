use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context;
use neoism_agent_core::{
    AgentConfigDocument, EventPayload, PluginConfig, ProviderMessage, ToolListItem,
};
use neoism_agent_plugin_api::{
    PluginContributions, PluginDefinition, PluginFactory, PluginHostError, PluginManifest, PluginRuntimeError,
    ProcessHookRequest, ProcessHookResponse, RegistrySnapshot, RuntimeHook,
    PROCESS_PLUGIN_PROTOCOL,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolExecutionResult;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatHookContext {
    pub(crate) session_id: String,
    pub(crate) agent: String,
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellEnvContext {
    pub(crate) cwd: String,
    pub(crate) session_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDefinitionContext {
    pub(crate) tool_id: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolExecutionContext {
    pub(crate) tool_id: String,
    pub(crate) directory: String,
    pub(crate) session_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

struct DeclarativePlugin {
    id: String,
    headers: BTreeMap<String, String>,
    options: BTreeMap<String, Value>,
    shell_env: BTreeMap<String, String>,
    process: Option<ProcessPlugin>,
}

#[derive(Clone)]
struct ProcessPlugin {
    executables: Arc<dyn neoism_agent_service_api::ExecutableService>,
    command: Vec<String>,
    timeout: Duration,
    working_directory: PathBuf,
    sandbox: SandboxPolicy,
    network: bool,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum SandboxPolicy {
    Off,
    Auto,
    Required,
}

impl PluginDefinition for DeclarativePlugin {
    fn manifest(&self) -> PluginManifest {
        PluginManifest {
            id: self.id.clone(),
            name: self.id.clone(),
            version: "1.0.0".to_string(),
            internal: false,
            disableable: true,
            capabilities: Vec::new(),
            requires: Vec::new(),
            event_namespaces: Vec::new(),
            api_prefix: None,
            config: BTreeMap::new(),
        }
    }

    fn required_capabilities(&self) -> Vec<neoism_agent_plugin_api::HostCapability> {
        let Some(process) = &self.process else { return Vec::new() };
        let mut capabilities = vec![
            neoism_agent_plugin_api::HostCapability::ProcessSpawn,
            neoism_agent_plugin_api::HostCapability::WorkspaceRead,
        ];
        if process.network { capabilities.push(neoism_agent_plugin_api::HostCapability::Network); }
        capabilities
    }

    fn contributions(&self, registrar: &mut PluginContributions) -> Result<(), PluginHostError> {
        registrar.hook(self.id.clone());
        registrar.runtime_hook(Arc::new(self.clone()));
        Ok(())
    }
}

impl Clone for DeclarativePlugin {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            headers: self.headers.clone(),
            options: self.options.clone(),
            shell_env: self.shell_env.clone(),
            process: self.process.clone(),
        }
    }
}

impl RuntimeHook for DeclarativePlugin {
    fn invoke(&self, hook: &str, context: Value, mut value: Value) -> Result<Value, PluginRuntimeError> {
        let configured = match hook {
            "chat.headers" => serde_json::to_value(merge_string_map(value, &self.headers)),
            "chat.options" => serde_json::to_value(merge_value_map(value, &self.options)),
            "shell.env" => serde_json::to_value(merge_string_map(value, &self.shell_env)),
            _ => Ok(value),
        }
        .map_err(|error| PluginRuntimeError::new(error.to_string()))?;
        value = configured;
        let Some(process) = &self.process else { return Ok(value) };
        process.invoke(ProcessHookRequest {
            protocol: PROCESS_PLUGIN_PROTOCOL.to_string(),
            hook: hook.to_string(),
            context,
            value,
        }).map_err(|error| PluginRuntimeError::new(error.to_string()))
    }
}

impl ProcessPlugin {
    fn invoke(&self, request: ProcessHookRequest) -> anyhow::Result<Value> {
        const MAX_RESPONSE_BYTES: u64 = 4 * 1024 * 1024;
        let (program, arguments) = self
            .command
            .split_first()
            .ok_or_else(|| anyhow::anyhow!("plugin command is empty"))?;
        let mut command = self.command(program, arguments)?;
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .with_context(|| format!("failed to start plugin process {program}"))?;
        let mut stdin = child.stdin.take().expect("piped plugin stdin");
        serde_json::to_writer(&mut stdin, &request)?;
        stdin.write_all(b"\n")?;
        drop(stdin);

        let stdout = child.stdout.take().expect("piped plugin stdout");
        let reader = std::thread::spawn(move || {
            let mut output = Vec::new();
            stdout.take(MAX_RESPONSE_BYTES + 1).read_to_end(&mut output)?;
            Ok::<_, std::io::Error>(output)
        });
        let deadline = Instant::now() + self.timeout;
        loop {
            if let Some(status) = child.try_wait()? {
                let output = reader
                    .join()
                    .map_err(|_| anyhow::anyhow!("plugin output reader panicked"))??;
                if !status.success() {
                    anyhow::bail!("plugin process exited with {status}");
                }
                if output.len() as u64 > MAX_RESPONSE_BYTES {
                    anyhow::bail!("plugin response exceeds {MAX_RESPONSE_BYTES} bytes");
                }
                let response: ProcessHookResponse = serde_json::from_slice(&output)
                    .context("plugin returned invalid JSON")?;
                if response.protocol != PROCESS_PLUGIN_PROTOCOL {
                    anyhow::bail!("unsupported plugin protocol {}", response.protocol);
                }
                if !response.ok {
                    anyhow::bail!(response.error.unwrap_or_else(|| "plugin hook failed".to_string()));
                }
                return Ok(response.value);
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                anyhow::bail!("plugin process timed out after {} ms", self.timeout.as_millis());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn command(&self, program: &str, arguments: &[String]) -> anyhow::Result<Command> {
        let mut command = Vec::with_capacity(arguments.len() + 1);
        command.push(program.to_string());
        command.extend(arguments.iter().cloned());
        build_plugin_command(
            &self.executables,
            &command,
            &self.working_directory,
            self.sandbox,
            self.network,
        )
    }

}

/// Resolve + (on Linux) bubblewrap-sandbox a plugin command line. Shared by
/// the one-shot declarative hook path and the long-lived serve-plugin host.
pub(crate) fn build_plugin_command(
    executables: &Arc<dyn neoism_agent_service_api::ExecutableService>,
    command: &[String],
    working_directory: &Path,
    sandbox: SandboxPolicy,
    network: bool,
) -> anyhow::Result<Command> {
    let (program, arguments) = command
        .split_first()
        .ok_or_else(|| anyhow::anyhow!("plugin command is empty"))?;
    {
        let requested_program = crate::executable::in_directory(program, working_directory);
        let program = if Path::new(&requested_program).is_absolute() {
            PathBuf::from(&requested_program)
        } else {
            executables
                .resolve(&neoism_agent_service_api::ExecutableRequest::new(
                    &requested_program,
                    neoism_agent_service_api::ExecutablePurpose::Plugin,
                ))
                .map_err(|error| {
                    anyhow::anyhow!(
                        "plugin executable `{}` is unavailable: {error}; configure the host executable resolver or install it",
                        requested_program.to_string_lossy()
                    )
                })?
                .path
        };
        #[cfg(target_os = "linux")]
        if !matches!(sandbox, SandboxPolicy::Off) {
            let bwrap = executables.resolve(
                &neoism_agent_service_api::ExecutableRequest::new(
                    "bwrap",
                    neoism_agent_service_api::ExecutablePurpose::Sandbox,
                ),
            );
            let bwrap_error = bwrap.as_ref().err().map(ToString::to_string);
            if let Ok(bwrap) = bwrap {
                let mut command = Command::new(bwrap.path);
                command.args([
                    "--die-with-parent",
                    "--new-session",
                    "--unshare-pid",
                    "--unshare-ipc",
                    "--unshare-uts",
                    "--unshare-cgroup",
                    "--tmpfs",
                    "/",
                    "--dir",
                    "/etc",
                ]);
                if !network {
                    command.arg("--unshare-net");
                }
                for path in [
                    "/usr",
                    "/bin",
                    "/lib",
                    "/lib64",
                    "/etc/ssl",
                    "/etc/resolv.conf",
                ] {
                    if Path::new(path).exists() {
                        command.args(["--ro-bind", path, path]);
                    }
                }
                let mut ancestors = working_directory
                    .ancestors()
                    .skip(1)
                    .filter(|path| *path != Path::new("/"))
                    .collect::<Vec<_>>();
                ancestors.reverse();
                for ancestor in ancestors {
                    command.args(["--dir", ancestor.to_string_lossy().as_ref()]);
                }
                let directory = working_directory.to_string_lossy();
                command.args(["--ro-bind", directory.as_ref(), directory.as_ref()]);
                if let Some(parent) = program.parent().filter(|path| {
                    path.is_absolute()
                        && !path.starts_with(working_directory)
                        && !["/usr", "/bin", "/lib", "/lib64"]
                            .iter()
                            .any(|root| path.starts_with(root))
                }) {
                    let mut ancestors = parent
                        .ancestors()
                        .skip(1)
                        .filter(|path| *path != Path::new("/"))
                        .collect::<Vec<_>>();
                    ancestors.reverse();
                    for ancestor in ancestors {
                        command.args(["--dir", ancestor.to_string_lossy().as_ref()]);
                    }
                    let parent = parent.to_string_lossy();
                    command.args(["--ro-bind", parent.as_ref(), parent.as_ref()]);
                }
                command.args([
                    "--tmpfs",
                    "/tmp",
                    "--proc",
                    "/proc",
                    "--dev",
                    "/dev",
                    "--chdir",
                    directory.as_ref(),
                    "--",
                    program.to_string_lossy().as_ref(),
                ]);
                command.args(arguments);
                return Ok(command);
            }
            if matches!(sandbox, SandboxPolicy::Required) {
                anyhow::bail!(
                    "plugin sandbox executable `bwrap` is unavailable: {}; configure the host executable resolver or install bubblewrap",
                    bwrap_error.unwrap_or_else(|| "not found".to_string())
                );
            }
        }
        #[cfg(not(target_os = "linux"))]
        if matches!(sandbox, SandboxPolicy::Required) {
            anyhow::bail!("plugin sandbox is required but bubblewrap is unavailable");
        }
        let mut command = Command::new(program);
        command.args(arguments).current_dir(working_directory);
        Ok(command)
    }
}

pub(crate) fn configured_agent_plugins(
    services: &neoism_agent_service_api::AgentServices,
    config: &AgentConfigDocument,
    directory: &str,
) -> Vec<Box<dyn PluginFactory>> {
    configured_plugins(config)
        .into_iter()
        .chain(discovered_plugin_configs(services, directory))
        .filter(|plugin| plugin.enabled)
        .filter_map(|plugin| {
            let id = plugin_id(&plugin)?;
            let options = plugin_options(&plugin);
            // A serve-shaped entry is a long-lived third-party plugin host;
            // everything else stays on the declarative one-shot path.
            if let Some(spec) =
                crate::plugin_host_process::serve_plugin_spec(&id, directory, &options)
            {
                return Some(Box::new(crate::plugin_host_process::ServePluginFactory::new(
                    spec,
                    Arc::clone(&services.executables),
                )) as Box<dyn PluginFactory>);
            }
            match load_declarative_plugin(services, directory, &id, &options) {
                Ok(plugin) => Some(Box::new(plugin) as Box<dyn PluginFactory>),
                Err(error) => {
                    tracing::warn!(plugin = %id, %error, "failed to load configured plugin");
                    None
                }
            }
        })
        .collect()
}

fn invoke<T: Serialize + serde::de::DeserializeOwned>(
    snapshot: &RegistrySnapshot,
    hook: &str,
    context: &impl Serialize,
    value: &mut T,
) -> anyhow::Result<()> {
    snapshot.ensure_active().map_err(|error| anyhow::anyhow!(error.to_string()))?;
    let context = serde_json::to_value(context)?;
    let mut next = serde_json::to_value(&*value)?;
    for runtime in &snapshot.runtime_hooks {
        next = runtime
            .invoke(hook, context.clone(), next)
            .map_err(|error| anyhow::anyhow!("plugin {} {hook} failed: {error}", runtime.plugin_id))?;
    }
    *value = serde_json::from_value(next)?;
    Ok(())
}

pub(crate) fn publish_event(snapshot: &RegistrySnapshot, event: &EventPayload) {
    let mut value = Value::Null;
    let _ = invoke(snapshot, "event", event, &mut value);
}

pub(crate) fn chat_messages_transform(snapshot: &RegistrySnapshot, ctx: &ChatHookContext, value: &mut Vec<ProviderMessage>) -> anyhow::Result<()> {
    invoke(snapshot, "chat.messages", ctx, value)
}

pub(crate) fn chat_options(snapshot: &RegistrySnapshot, ctx: &ChatHookContext, value: &mut BTreeMap<String, Value>) -> anyhow::Result<()> {
    invoke(snapshot, "chat.options", ctx, value)
}

pub(crate) fn chat_headers(snapshot: &RegistrySnapshot, ctx: &ChatHookContext, value: &mut BTreeMap<String, String>) -> anyhow::Result<()> {
    invoke(snapshot, "chat.headers", ctx, value)
}

pub(crate) fn tool_definition(snapshot: &RegistrySnapshot, value: &mut ToolListItem) -> anyhow::Result<()> {
    let ctx = ToolDefinitionContext { tool_id: value.id.clone() };
    invoke(snapshot, "tool.definition", &ctx, value)
}

pub(crate) fn tool_execute_before(snapshot: &RegistrySnapshot, ctx: &ToolExecutionContext, value: &mut Value) -> anyhow::Result<()> {
    invoke(snapshot, "tool.before", ctx, value)
}

pub(crate) fn tool_execute_after(snapshot: &RegistrySnapshot, ctx: &ToolExecutionContext, value: &mut ToolExecutionResult) -> anyhow::Result<()> {
    invoke(snapshot, "tool.after", ctx, value)
}

pub(crate) fn shell_env(snapshot: &RegistrySnapshot, ctx: &ShellEnvContext, value: &mut BTreeMap<String, String>) -> anyhow::Result<()> {
    invoke(snapshot, "shell.env", ctx, value)
}

fn merge_string_map(value: Value, configured: &BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut value: BTreeMap<String, String> = serde_json::from_value(value).unwrap_or_default();
    value.extend(configured.clone());
    value
}

fn merge_value_map(value: Value, configured: &BTreeMap<String, Value>) -> BTreeMap<String, Value> {
    let mut value: BTreeMap<String, Value> = serde_json::from_value(value).unwrap_or_default();
    value.extend(configured.clone());
    value
}

fn configured_plugins(config: &AgentConfigDocument) -> Vec<PluginConfig> {
    config.plugins.iter().map(|(id, plugin)| {
        let mut plugin = plugin.clone();
        plugin.id = Some(id.clone());
        plugin
    }).collect()
}

fn plugin_id(plugin: &PluginConfig) -> Option<String> {
    plugin
        .id
        .as_deref()
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(ToOwned::to_owned)
}

fn plugin_options(plugin: &PluginConfig) -> BTreeMap<String, Value> {
    plugin.options.clone()
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeclarativePluginFile {
    id: Option<String>,
    #[serde(default)]
    chat_headers: BTreeMap<String, String>,
    #[serde(default)]
    chat_options: BTreeMap<String, Value>,
    #[serde(default)]
    shell_env: BTreeMap<String, String>,
    #[serde(default)]
    command: Option<Value>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    sandbox: Option<bool>,
    #[serde(default)]
    network: bool,
}

fn load_declarative_plugin(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
    id: &str,
    options: &BTreeMap<String, Value>,
) -> anyhow::Result<DeclarativePlugin> {
    let path = resolve_plugin_manifest(services, directory, id, options).ok_or_else(|| {
        anyhow::anyhow!(
            "unsupported plugin id; configure a JSON manifest path or place {id}.json under plugins/"
        )
    })?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read plugin manifest {}", path.display()))?;
    let manifest: DeclarativePluginFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse plugin manifest {}", path.display()))?;
    let headers = manifest.chat_headers;
    let chat_options = manifest.chat_options;
    let shell_env = manifest.shell_env;
    let process = manifest
        .command
        .as_ref()
        .map(process_plugin)
        .transpose()?
        .map(|command| ProcessPlugin {
            executables: Arc::clone(&services.executables),
            command,
            timeout: Duration::from_millis(manifest.timeout_ms.unwrap_or(10_000).clamp(100, 120_000)),
            working_directory: path
                .parent()
                .unwrap_or_else(|| Path::new(directory))
                .to_path_buf(),
            sandbox: sandbox_policy(manifest.sandbox),
            network: manifest.network,
        });
    Ok(DeclarativePlugin {
        id: manifest
            .id
            .map(|id| id.trim().to_string())
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| id.to_string()),
        headers,
        options: chat_options,
        shell_env,
        process,
    })
}

pub(crate) fn sandbox_policy(configured: Option<bool>) -> SandboxPolicy {
    match configured {
        Some(false) => SandboxPolicy::Off,
        Some(true) => SandboxPolicy::Required,
        None if std::env::var_os("NEOISM_AGENT_AUTH_CONFIG").is_some() => SandboxPolicy::Required,
        None => match std::env::var("NEOISM_AGENT_PLUGIN_SANDBOX").as_deref() {
            Ok("required" | "1" | "true") => SandboxPolicy::Required,
            Ok("off" | "0" | "false") => SandboxPolicy::Off,
            _ => SandboxPolicy::Auto,
        },
    }
}

fn process_plugin(value: &Value) -> anyhow::Result<Vec<String>> {
    let command = match value {
        Value::String(program) => vec![program.clone()],
        Value::Array(parts) => parts
            .iter()
            .map(|part| {
                part.as_str()
                    .map(str::to_string)
                    .ok_or_else(|| anyhow::anyhow!("plugin command entries must be strings"))
            })
            .collect::<anyhow::Result<Vec<_>>>()?,
        _ => anyhow::bail!("plugin command must be a string or string array"),
    };
    if command.first().is_none_or(|program| program.trim().is_empty()) {
        anyhow::bail!("plugin command is empty");
    }
    Ok(command)
}

fn resolve_plugin_manifest(
    services: &neoism_agent_service_api::AgentServices,
    directory: &str,
    id: &str,
    options: &BTreeMap<String, Value>,
) -> Option<PathBuf> {
    let explicit = options
        .get("path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty());
    let mut candidates = Vec::new();
    if let Some(path) = explicit {
        candidates.extend(resolve_path_candidates(services, directory, path));
    }
    if id.ends_with(".json") || id.contains('/') || id.starts_with('.') {
        candidates.extend(resolve_path_candidates(services, directory, id));
    }
    for root in crate::config::roots(services, directory) {
        candidates.push(root.join("plugins").join(format!("{id}.json")));
    }
    candidates.into_iter().find(|path| path.is_file())
}

fn resolve_path_candidates(services: &neoism_agent_service_api::AgentServices, directory: &str, raw: &str) -> Vec<PathBuf> {
    let raw = PathBuf::from(raw);
    if raw.is_absolute() {
        return vec![raw];
    }
    let mut candidates = vec![Path::new(directory).join(&raw)];
    for root in crate::config::roots(services, directory) {
        candidates.push(root.join(&raw));
    }
    candidates
}

fn discovered_plugin_configs(services: &neoism_agent_service_api::AgentServices, directory: &str) -> Vec<PluginConfig> {
    let mut configs = Vec::new();
    for root in crate::config::roots(services, directory) {
            let dir = root.join("plugins");
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                let id = path
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .unwrap_or("plugin")
                    .to_string();
                configs.push(PluginConfig {
                    id: Some(id),
                    options: BTreeMap::from([(
                        "path".to_string(),
                        Value::String(path.display().to_string()),
                    )]),
                    ..PluginConfig::default()
                });
            }
    }
    configs
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn standard_executables() -> Arc<dyn neoism_agent_service_api::ExecutableService> {
        Arc::new(neoism_agent_service_api::StandardExecutableService)
    }

    #[test]
    fn plugin_process_honors_injected_path_and_reports_missing_executable() {
        use crate::executable::test_support::FakeExecutableService;

        let injected = PathBuf::from("/injected/plugin-runtime");
        let process = ProcessPlugin {
            executables: Arc::new(FakeExecutableService::with("plugin-runtime", &injected)),
            command: vec!["plugin-runtime".to_string()],
            timeout: Duration::from_secs(1),
            working_directory: std::env::current_dir().unwrap(),
            sandbox: SandboxPolicy::Off,
            network: false,
        };
        assert_eq!(
            process.command("plugin-runtime", &[]).unwrap().get_program(),
            injected.as_os_str()
        );

        let missing = ProcessPlugin {
            executables: Arc::new(FakeExecutableService::default()),
            ..process
        };
        let error = missing.command("plugin-runtime", &[]).unwrap_err().to_string();
        assert!(error.contains("plugin executable `plugin-runtime` is unavailable"));
        assert!(error.contains("install it"));
    }

    #[test]
    fn declarative_manifest_uses_only_camel_case_public_fields() {
        let canonical: DeclarativePluginFile = serde_json::from_value(json!({
            "chatHeaders": { "X-Chat": "yes" },
            "chatOptions": { "temperature": 0 },
            "timeoutMs": 500
        }))
        .unwrap();
        assert_eq!(canonical.chat_headers["X-Chat"], "yes");
        assert_eq!(canonical.chat_options["temperature"], 0);
        assert_eq!(canonical.timeout_ms, Some(500));

        let legacy: DeclarativePluginFile = serde_json::from_value(json!({
            "chat_headers": { "X-Legacy": "yes" },
            "chatParams": { "temperature": 1 },
            "timeout_ms": 900
        }))
        .unwrap();
        assert!(legacy.chat_headers.is_empty());
        assert!(legacy.chat_options.is_empty());
        assert_eq!(legacy.timeout_ms, None);
    }

    #[tokio::test]
    async fn configured_process_factory_create_and_start_are_static_and_nonblocking() {
        let plugin = DeclarativePlugin {
            id: "dev.example.configured-process".into(),
            headers: BTreeMap::new(),
            options: BTreeMap::new(),
            shell_env: BTreeMap::new(),
            process: Some(ProcessPlugin {
                executables: standard_executables(),
                command: vec!["definitely-not-invoked-during-create".into()],
                timeout: Duration::from_secs(1),
                working_directory: std::env::current_dir().unwrap(),
                sandbox: SandboxPolicy::Off,
                network: false,
            }),
        };
        let context = neoism_agent_plugin_api::PluginContext::new(
            neoism_agent_plugin_api::RuntimeScope::Workspace(neoism_agent_plugin_api::WorkspaceIdentity {
                id: "test".into(), root: PathBuf::from("."),
            }),
            neoism_agent_plugin_api::CapabilityGrants::default()
                .allow(neoism_agent_plugin_api::HostCapability::ProcessSpawn)
                .allow(neoism_agent_plugin_api::HostCapability::WorkspaceRead),
        );
        let instance = tokio::time::timeout(Duration::from_millis(20), neoism_agent_plugin_api::PluginFactory::create(&plugin, context))
            .await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_millis(20), instance.start()).await.unwrap().unwrap();
        assert_eq!(instance.contributions().runtime_hooks.len(), 1);
    }

    #[test]
    fn subprocess_hook_uses_versioned_json_protocol() {
        let process = ProcessPlugin {
            executables: standard_executables(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "cat >/dev/null; printf '{\"protocol\":\"neoism-plugin/1\",\"ok\":true,\"value\":{\"loaded\":true}}'".to_string(),
            ],
            timeout: Duration::from_secs(2),
            working_directory: std::env::current_dir().unwrap(),
            sandbox: SandboxPolicy::Off,
            network: false,
        };
        let value = process
            .invoke(ProcessHookRequest {
                protocol: PROCESS_PLUGIN_PROTOCOL.to_string(),
                hook: "test".to_string(),
                context: Value::Null,
                value: Value::Null,
            })
            .unwrap();
        assert_eq!(value["loaded"], true);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn subprocess_hook_runs_in_required_bubblewrap_sandbox() {
        if standard_executables().resolve(&neoism_agent_service_api::ExecutableRequest::new(
            "bwrap",
            neoism_agent_service_api::ExecutablePurpose::Sandbox,
        )).is_err() {
            return;
        }
        let process = ProcessPlugin {
            executables: standard_executables(),
            command: vec![
                "sh".to_string(),
                "-c".to_string(),
                "cat >/dev/null; printf '{\"protocol\":\"neoism-plugin/1\",\"ok\":true,\"value\":true}'".to_string(),
            ],
            timeout: Duration::from_secs(2),
            working_directory: std::env::current_dir().unwrap(),
            sandbox: SandboxPolicy::Required,
            network: false,
        };
        let value = process
            .invoke(ProcessHookRequest {
                protocol: PROCESS_PLUGIN_PROTOCOL.to_string(),
                hook: "test".to_string(),
                context: Value::Null,
                value: Value::Null,
            })
            .unwrap();
        assert_eq!(value, true);
    }

    struct RecoveringPlugin {
        failing: Arc<AtomicBool>,
    }

    impl RuntimeHook for RecoveringPlugin {
        fn invoke(&self, _hook: &str, _context: Value, value: Value) -> Result<Value, PluginRuntimeError> {
            if self.failing.load(Ordering::SeqCst) {
                return Err(PluginRuntimeError::new("intentional failure"));
            }
            Ok(value)
        }
    }

    #[test]
    fn hook_failures_update_health_and_success_recovers() {
        let failing = Arc::new(AtomicBool::new(true));
        let hook = neoism_agent_plugin_api::RegisteredRuntimeHook::for_test(
            "dev.neoism.test",
            Arc::new(RecoveringPlugin { failing: failing.clone() }),
        );
        assert!(hook.invoke("shell.env", Value::Null, Value::Null).is_err());
        assert!(!hook.lifecycle().active);
        assert!(hook.lifecycle().reason.unwrap().contains("intentional failure"));

        failing.store(false, Ordering::SeqCst);
        hook.invoke("shell.env", Value::Null, Value::Null).unwrap();
        assert!(hook.lifecycle().active);
        assert!(hook.lifecycle().reason.is_none());
    }
}
