use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use anyhow::Context;
use neoism_agent_core::{
    EventPayload, NeoismConfig, PluginConfig, PluginScope, PluginSource,
    PluginStatusInfo, ProviderMessage, ToolListItem,
};
use neoism_agent_plugin_api::{
    ProcessHookRequest, ProcessHookResponse, PROCESS_PLUGIN_PROTOCOL,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::tool::ToolExecutionResult;

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ChatHookContext {
    pub(crate) session_id: String,
    pub(crate) agent: String,
    pub(crate) provider_id: String,
    pub(crate) model_id: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ShellEnvContext {
    pub(crate) cwd: String,
    pub(crate) session_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolDefinitionContext {
    pub(crate) tool_id: String,
}

#[allow(dead_code)]
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ToolExecutionContext {
    pub(crate) tool_id: String,
    pub(crate) directory: String,
    pub(crate) session_id: Option<String>,
    pub(crate) message_id: Option<String>,
    pub(crate) call_id: Option<String>,
}

pub(crate) trait NativePlugin: Send + Sync {
    fn name(&self) -> &str;

    fn event(&self, _event: &EventPayload) {}

    fn chat_messages_transform(
        &self,
        _ctx: &ChatHookContext,
        _messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn chat_headers(
        &self,
        _ctx: &ChatHookContext,
        _headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn chat_options(
        &self,
        _ctx: &ChatHookContext,
        _options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_definition(
        &self,
        _ctx: &ToolDefinitionContext,
        _tool: &mut ToolListItem,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_execute_before(
        &self,
        _ctx: &ToolExecutionContext,
        _args: &mut Value,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn tool_execute_after(
        &self,
        _ctx: &ToolExecutionContext,
        _result: &mut ToolExecutionResult,
    ) -> anyhow::Result<()> {
        Ok(())
    }

    fn shell_env(
        &self,
        _ctx: &ShellEnvContext,
        _env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        Ok(())
    }
}

#[derive(Clone, Copy)]
struct InternalPluginDefinition {
    id: &'static str,
    name: &'static str,
}

const INTERNAL_PLUGIN_DEFINITIONS: &[InternalPluginDefinition] = &[
    InternalPluginDefinition {
        id: "neoism.internal.noop",
        name: "Neoism internal no-op",
    },
    InternalPluginDefinition {
        id: "neoism.internal.config",
        name: "Neoism internal config",
    },
];

struct ConfiguredInternalPlugin {
    definition: InternalPluginDefinition,
}

impl NativePlugin for ConfiguredInternalPlugin {
    fn name(&self) -> &str {
        self.definition.id
    }
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
    command: Vec<String>,
    timeout: Duration,
    working_directory: PathBuf,
    sandbox: SandboxPolicy,
    network: bool,
}

#[derive(Clone, Copy)]
enum SandboxPolicy {
    Off,
    Auto,
    Required,
}

impl NativePlugin for DeclarativePlugin {
    fn name(&self) -> &str {
        &self.id
    }

    fn chat_headers(
        &self,
        _ctx: &ChatHookContext,
        headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        headers.extend(self.headers.clone());
        if let Some(value) = self.invoke("chat.headers", _ctx, &*headers)? {
            *headers = serde_json::from_value(value)?;
        }
        Ok(())
    }

    fn chat_options(
        &self,
        _ctx: &ChatHookContext,
        options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        options.extend(self.options.clone());
        if let Some(value) = self.invoke("chat.options", _ctx, &*options)? {
            *options = serde_json::from_value(value)?;
        }
        Ok(())
    }

    fn shell_env(
        &self,
        _ctx: &ShellEnvContext,
        env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        env.extend(self.shell_env.clone());
        if let Some(value) = self.invoke("shell.env", _ctx, &*env)? {
            *env = serde_json::from_value(value)?;
        }
        Ok(())
    }

    fn chat_messages_transform(
        &self,
        ctx: &ChatHookContext,
        messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        if let Some(value) = self.invoke("chat.messages", ctx, &*messages)? {
            *messages = serde_json::from_value(value)?;
        }
        Ok(())
    }

    fn tool_definition(
        &self,
        ctx: &ToolDefinitionContext,
        tool: &mut ToolListItem,
    ) -> anyhow::Result<()> {
        if let Some(value) = self.invoke("tool.definition", ctx, &*tool)? {
            *tool = serde_json::from_value(value)?;
        }
        Ok(())
    }

    fn tool_execute_before(
        &self,
        ctx: &ToolExecutionContext,
        args: &mut Value,
    ) -> anyhow::Result<()> {
        if let Some(value) = self.invoke("tool.before", ctx, &*args)? {
            *args = value;
        }
        Ok(())
    }

    fn tool_execute_after(
        &self,
        ctx: &ToolExecutionContext,
        result: &mut ToolExecutionResult,
    ) -> anyhow::Result<()> {
        if let Some(value) = self.invoke("tool.after", ctx, &*result)? {
            *result = serde_json::from_value(value)?;
        }
        Ok(())
    }
}

impl DeclarativePlugin {
    fn invoke<C: Serialize, T: Serialize>(
        &self,
        hook: &str,
        context: &C,
        value: &T,
    ) -> anyhow::Result<Option<Value>> {
        let Some(process) = &self.process else {
            return Ok(None);
        };
        process.invoke(ProcessHookRequest {
            protocol: PROCESS_PLUGIN_PROTOCOL.to_string(),
            hook: hook.to_string(),
            context: serde_json::to_value(context)?,
            value: serde_json::to_value(value)?,
        }).map(Some)
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
        #[cfg(target_os = "linux")]
        if !matches!(self.sandbox, SandboxPolicy::Off) {
            if let Some(bwrap) = find_executable("bwrap") {
                let mut command = Command::new(bwrap);
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
                if !self.network {
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
                let mut ancestors = self
                    .working_directory
                    .ancestors()
                    .skip(1)
                    .filter(|path| *path != Path::new("/"))
                    .collect::<Vec<_>>();
                ancestors.reverse();
                for ancestor in ancestors {
                    command.args(["--dir", ancestor.to_string_lossy().as_ref()]);
                }
                let directory = self.working_directory.to_string_lossy();
                command.args(["--ro-bind", directory.as_ref(), directory.as_ref()]);
                if let Some(parent) = Path::new(program).parent().filter(|path| {
                    path.is_absolute()
                        && !path.starts_with(&self.working_directory)
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
                    program,
                ]);
                command.args(arguments);
                return Ok(command);
            }
        }
        if matches!(self.sandbox, SandboxPolicy::Required) {
            anyhow::bail!("plugin sandbox is required but bubblewrap is unavailable");
        }
        let mut command = Command::new(program);
        command.args(arguments).current_dir(&self.working_directory);
        Ok(command)
    }
}

fn find_executable(name: &str) -> Option<PathBuf> {
    let request = neoism_agent_service_api::ExecutableRequest::new(
        name,
        neoism_agent_service_api::ExecutablePurpose::Sandbox,
    );
    crate::lsp::agent_services()
        .executables
        .resolve(&request)
        .ok()
        .map(|result| result.path)
}

#[derive(Clone)]
struct RegisteredPlugin {
    id: String,
    name: String,
    source: PluginSource,
    scope: PluginScope,
    options: BTreeMap<String, Value>,
    plugin: Arc<dyn NativePlugin>,
}

impl RegisteredPlugin {
    fn status(&self) -> PluginStatusInfo {
        PluginStatusInfo {
            id: self.id.clone(),
            name: self.name.clone(),
            source: self.source.clone(),
            scope: self.scope.clone(),
            enabled: true,
            active: true,
            reason: None,
            options: self.options.clone(),
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct PluginRegistry {
    plugins: Arc<RwLock<Vec<RegisteredPlugin>>>,
    statuses: Arc<RwLock<BTreeMap<String, PluginStatusInfo>>>,
}

impl PluginRegistry {
    /// Create an independent registry seeded with runtime registrations. This
    /// is used as the base for one workspace so configured plugins never leak
    /// into unrelated locations.
    pub(crate) fn fork(&self) -> Self {
        Self {
            plugins: Arc::new(RwLock::new(
                self.plugins
                    .read()
                    .expect("plugin registry lock poisoned")
                    .clone(),
            )),
            statuses: Arc::new(RwLock::new(
                self.statuses
                    .read()
                    .expect("plugin registry lock poisoned")
                    .clone(),
            )),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn register<P>(&self, plugin: P)
    where
        P: NativePlugin + 'static,
    {
        let id = plugin.name().to_string();
        let name = plugin.name().to_string();
        self.register_entry(RegisteredPlugin {
            id,
            name,
            source: PluginSource::Runtime,
            scope: PluginScope::Session,
            options: BTreeMap::new(),
            plugin: Arc::new(plugin),
        });
    }

    pub(crate) fn register_configured_plugins(
        &self,
        services: &neoism_agent_service_api::AgentServices,
        config: &NeoismConfig,
        directory: &str,
    ) -> Vec<PluginStatusInfo> {
        let mut statuses = Vec::new();
        for plugin in configured_plugins(config)
            .into_iter()
            .chain(discovered_plugin_configs(services, directory))
        {
            let Some(id) = plugin_id(&plugin) else {
                continue;
            };
            let scope = plugin.scope.clone().unwrap_or_default();
            let options = plugin_options(&plugin);
            let definition = internal_plugin_definition(&id);

            if !plugin.enabled {
                self.remove_active(&id);
                let status = PluginStatusInfo {
                    id: id.clone(),
                    name: definition
                        .map(|definition| definition.name)
                        .unwrap_or(id.as_str())
                        .to_string(),
                    source: definition
                        .map(|_| PluginSource::Internal)
                        .unwrap_or(PluginSource::Unknown),
                    scope,
                    enabled: false,
                    active: false,
                    reason: Some("disabled by config".to_string()),
                    options,
                };
                self.record_status(status.clone());
                statuses.push(status);
                continue;
            }

            let entry = if let Some(definition) = definition {
                RegisteredPlugin {
                    id,
                    name: definition.name.to_string(),
                    source: PluginSource::Internal,
                    scope,
                    options,
                    plugin: Arc::new(ConfiguredInternalPlugin { definition }),
                }
            } else {
                match load_declarative_plugin(services, directory, &id, &options) {
                    Ok(plugin) => RegisteredPlugin {
                        id: plugin.id.clone(),
                        name: plugin.id.clone(),
                        source: PluginSource::External,
                        scope,
                        options,
                        plugin: Arc::new(plugin),
                    },
                    Err(error) => {
                        self.remove_active(&id);
                        let status = PluginStatusInfo {
                            id: id.clone(),
                            name: id.clone(),
                            source: PluginSource::Unknown,
                            scope,
                            enabled: true,
                            active: false,
                            reason: Some(error.to_string()),
                            options,
                        };
                        self.record_status(status.clone());
                        statuses.push(status);
                        continue;
                    }
                }
            };
            let status = entry.status();
            self.register_entry(entry);
            statuses.push(status);
        }
        statuses
    }

    #[cfg(test)]
    pub(crate) fn statuses(&self) -> Vec<PluginStatusInfo> {
        self.statuses
            .read()
            .expect("plugin registry lock poisoned")
            .values()
            .cloned()
            .collect()
    }

    fn register_entry(&self, entry: RegisteredPlugin) {
        let status = entry.status();
        let mut plugins = self.plugins.write().expect("plugin registry lock poisoned");
        if let Some(existing) = plugins.iter_mut().find(|plugin| plugin.id == entry.id) {
            *existing = entry;
        } else {
            plugins.push(entry);
        }
        drop(plugins);
        self.record_status(status);
    }

    fn record_status(&self, status: PluginStatusInfo) {
        self.statuses
            .write()
            .expect("plugin registry lock poisoned")
            .insert(status.id.clone(), status);
    }

    fn remove_active(&self, id: &str) {
        self.plugins
            .write()
            .expect("plugin registry lock poisoned")
            .retain(|plugin| plugin.id != id);
    }

    fn entries(&self) -> Vec<RegisteredPlugin> {
        self.plugins
            .read()
            .expect("plugin registry lock poisoned")
            .clone()
    }

    fn observe<T>(
        &self,
        entry: &RegisteredPlugin,
        hook: &str,
        result: anyhow::Result<T>,
    ) -> anyhow::Result<T> {
        match result {
            Ok(value) => {
                self.record_status(entry.status());
                Ok(value)
            }
            Err(error) => {
                let mut status = entry.status();
                status.active = false;
                status.reason = Some(format!("{hook} failed: {error}"));
                self.record_status(status);
                Err(error).with_context(|| format!("plugin {} {hook} failed", entry.id))
            }
        }
    }

    pub(crate) fn publish_event(&self, event: &EventPayload) {
        for entry in self.entries() {
            entry.plugin.event(event);
        }
    }

    pub(crate) fn chat_messages_transform(
        &self,
        ctx: &ChatHookContext,
        messages: &mut Vec<ProviderMessage>,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(
                &entry,
                "chat_messages_transform",
                entry.plugin.chat_messages_transform(ctx, messages),
            )?;
        }
        Ok(())
    }

    #[allow(dead_code)]
    pub(crate) fn chat_headers(
        &self,
        ctx: &ChatHookContext,
        headers: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(&entry, "chat_headers", entry.plugin.chat_headers(ctx, headers))?;
        }
        Ok(())
    }

    pub(crate) fn chat_options(
        &self,
        ctx: &ChatHookContext,
        options: &mut BTreeMap<String, Value>,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(&entry, "chat_options", entry.plugin.chat_options(ctx, options))?;
        }
        Ok(())
    }

    pub(crate) fn tool_definition(&self, tool: &mut ToolListItem) -> anyhow::Result<()> {
        let ctx = ToolDefinitionContext {
            tool_id: tool.id.clone(),
        };
        for entry in self.entries() {
            self.observe(
                &entry,
                "tool_definition",
                entry.plugin.tool_definition(&ctx, tool),
            )?;
        }
        Ok(())
    }

    pub(crate) fn tool_execute_before(
        &self,
        ctx: &ToolExecutionContext,
        args: &mut Value,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(
                &entry,
                "tool_execute_before",
                entry.plugin.tool_execute_before(ctx, args),
            )?;
        }
        Ok(())
    }

    pub(crate) fn tool_execute_after(
        &self,
        ctx: &ToolExecutionContext,
        result: &mut ToolExecutionResult,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(
                &entry,
                "tool_execute_after",
                entry.plugin.tool_execute_after(ctx, result),
            )?;
        }
        Ok(())
    }

    pub(crate) fn shell_env(
        &self,
        ctx: &ShellEnvContext,
        env: &mut BTreeMap<String, String>,
    ) -> anyhow::Result<()> {
        for entry in self.entries() {
            self.observe(&entry, "shell_env", entry.plugin.shell_env(ctx, env))?;
        }
        Ok(())
    }
}

fn configured_plugins(config: &NeoismConfig) -> Vec<PluginConfig> {
    let mut plugins = config.plugin.clone();
    for (id, plugin) in &config.plugins {
        let mut plugin = plugin.clone();
        if plugin
            .id
            .as_deref()
            .map(str::trim)
            .unwrap_or_default()
            .is_empty()
        {
            plugin.id = Some(id.clone());
        }
        plugins.push(plugin);
    }
    plugins
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
    let mut options = plugin.options.clone();
    for (key, value) in &plugin.extra {
        options.entry(key.clone()).or_insert_with(|| value.clone());
    }
    options
}

fn internal_plugin_definition(id: &str) -> Option<InternalPluginDefinition> {
    INTERNAL_PLUGIN_DEFINITIONS
        .iter()
        .copied()
        .find(|definition| definition.id == id)
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeclarativePluginFile {
    id: Option<String>,
    #[serde(default)]
    chat_headers: BTreeMap<String, String>,
    #[serde(default)]
    provider_headers: BTreeMap<String, String>,
    #[serde(default)]
    chat_options: BTreeMap<String, Value>,
    #[serde(default)]
    provider_options: BTreeMap<String, Value>,
    #[serde(default)]
    shell_env: BTreeMap<String, String>,
    #[serde(default)]
    chat: BTreeMap<String, Value>,
    #[serde(default)]
    provider: BTreeMap<String, Value>,
    #[serde(default)]
    shell: BTreeMap<String, Value>,
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
            "unsupported plugin id; configure a JSON manifest path or place {id}.json under plugin(s)/"
        )
    })?;
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read plugin manifest {}", path.display()))?;
    let manifest: DeclarativePluginFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse plugin manifest {}", path.display()))?;
    let mut headers = manifest.chat_headers;
    headers.extend(manifest.provider_headers);
    headers.extend(string_map(
        manifest
            .chat
            .get("headers")
            .or_else(|| manifest.provider.get("headers")),
    ));
    let mut chat_options = manifest.chat_options;
    chat_options.extend(manifest.provider_options);
    chat_options.extend(value_map(
        manifest
            .chat
            .get("options")
            .or_else(|| manifest.chat.get("params"))
            .or_else(|| manifest.provider.get("options")),
    ));
    let mut shell_env = manifest.shell_env;
    shell_env.extend(string_map(manifest.shell.get("env")));
    let process = manifest
        .command
        .as_ref()
        .map(process_plugin)
        .transpose()?
        .map(|command| ProcessPlugin {
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

fn sandbox_policy(configured: Option<bool>) -> SandboxPolicy {
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
        .or_else(|| options.get("file"))
        .or_else(|| options.get("source"))
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
        candidates.push(root.join("plugin").join(format!("{id}.json")));
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
        for folder in ["plugins", "plugin"] {
            let dir = root.join(folder);
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
    }
    configs
}

#[cfg(all(test, unix))]
mod process_tests {
    use super::*;
    use serde_json::json;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn declarative_manifest_uses_only_camel_case_public_fields() {
        let canonical: DeclarativePluginFile = serde_json::from_value(json!({
            "chatHeaders": { "X-Chat": "yes" },
            "providerHeaders": { "X-Provider": "yes" },
            "chatOptions": { "temperature": 0 },
            "timeoutMs": 500
        }))
        .unwrap();
        assert_eq!(canonical.chat_headers["X-Chat"], "yes");
        assert_eq!(canonical.provider_headers["X-Provider"], "yes");
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

    #[test]
    fn subprocess_hook_uses_versioned_json_protocol() {
        let process = ProcessPlugin {
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
        if find_executable("bwrap").is_none() {
            return;
        }
        let process = ProcessPlugin {
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

    impl NativePlugin for RecoveringPlugin {
        fn name(&self) -> &str {
            "test.recovering"
        }

        fn shell_env(
            &self,
            _ctx: &ShellEnvContext,
            _env: &mut BTreeMap<String, String>,
        ) -> anyhow::Result<()> {
            if self.failing.load(Ordering::SeqCst) {
                anyhow::bail!("intentional failure");
            }
            Ok(())
        }
    }

    #[test]
    fn hook_failures_update_health_and_success_recovers() {
        let registry = PluginRegistry::default();
        let failing = Arc::new(AtomicBool::new(true));
        registry.register(RecoveringPlugin {
            failing: failing.clone(),
        });
        let context = ShellEnvContext {
            cwd: "/tmp".to_string(),
            session_id: None,
            call_id: None,
        };
        assert!(registry.shell_env(&context, &mut BTreeMap::new()).is_err());
        let status = registry.statuses().pop().unwrap();
        assert!(!status.active);
        assert!(status.reason.unwrap().contains("intentional failure"));

        failing.store(false, Ordering::SeqCst);
        registry.shell_env(&context, &mut BTreeMap::new()).unwrap();
        let status = registry.statuses().pop().unwrap();
        assert!(status.active);
        assert!(status.reason.is_none());
    }
}

fn string_map(value: Option<&Value>) -> BTreeMap<String, String> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(key, value)| {
            value.as_str().map(|value| (key.clone(), value.to_string()))
        })
        .collect()
}

fn value_map(value: Option<&Value>) -> BTreeMap<String, Value> {
    value
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
