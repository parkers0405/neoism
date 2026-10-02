use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::{Duration, Instant};

use clap::Parser;
use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, RegistryKey, Table, Value as LuaValue, VmState};
use neoism_agent_plugin_api::{
    AgentEntrypointRuntime, NeoismPackageManifest, ProcessHostFrame, ProcessHostReplyFrame,
    ProcessInitializeRequest, ProcessPluginFrame, ProcessPluginOwner, ProcessStreamEnvelope,
    PROCESS_PLUGIN_V2_PROTOCOL,
};
use serde::Serialize;
use serde_json::{json, Value};

const MAX_FRAME_BYTES: usize = 1024 * 1024;
const MAX_QUEUE: usize = 64;
const MEMORY_BYTES: usize = 16 * 1024 * 1024;
const LOAD_TIMEOUT: Duration = Duration::from_millis(250);
const CALLBACK_TIMEOUT: Duration = Duration::from_secs(30);
const HOST_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_INSTRUCTIONS: u64 = 10_000_000;
const MAX_STREAM_ITEMS: usize = 4096;

#[derive(Parser)]
#[command(version)]
struct Args {
    #[arg(long)]
    manifest: PathBuf,
}

#[derive(Clone)]
struct Writer(Arc<Mutex<io::Stdout>>);

impl Writer {
    fn send(&self, frame: &impl Serialize) -> Result<(), String> {
        let mut bytes = serde_json::to_vec(frame).map_err(|error| error.to_string())?;
        if bytes.len() > MAX_FRAME_BYTES {
            return Err(format!("frame exceeds {MAX_FRAME_BYTES} bytes"));
        }
        bytes.push(b'\n');
        let mut writer = self.0.lock().map_err(|_| "stdout lock poisoned")?;
        writer.write_all(&bytes).and_then(|()| writer.flush()).map_err(|error| error.to_string())
    }
}

#[derive(Clone)]
struct HostBridge {
    writer: Writer,
    owner: Arc<Mutex<Option<ProcessPluginOwner>>>,
    pending: Arc<Mutex<HashMap<u64, mpsc::Sender<Result<Value, String>>>>>,
    next_id: Arc<AtomicU64>,
}

impl HostBridge {
    fn request(&self, method: &str, params: Value) -> Result<Value, mlua::Error> {
        let owner = self
            .owner
            .lock()
            .map_err(|_| mlua::Error::runtime("owner lock poisoned"))?
            .clone()
            .ok_or_else(|| mlua::Error::runtime("plugin is not initialized"))?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = mpsc::channel();
        self.pending
            .lock()
            .map_err(|_| mlua::Error::runtime("pending lock poisoned"))?
            .insert(id, sender);
        self.writer
            .send(&ProcessPluginFrame {
                id: Some(id),
                result: None,
                error: None,
                method: Some(method.to_string()),
                params: Some(params),
                owner: Some(owner),
                request_id: None,
                stream: None,
            })
            .map_err(mlua::Error::runtime)?;
        match receiver.recv_timeout(HOST_TIMEOUT) {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => Err(mlua::Error::runtime(error)),
            Err(_) => {
                self.pending.lock().ok().map(|mut pending| pending.remove(&id));
                Err(mlua::Error::runtime("host request timed out"))
            }
        }
    }
}

struct RuntimeBudget {
    deadline: Arc<Mutex<Option<Instant>>>,
    cancelled: Arc<AtomicBool>,
    instructions: Arc<AtomicU64>,
}

impl RuntimeBudget {
    fn run<T>(&self, timeout: Duration, operation: impl FnOnce() -> mlua::Result<T>) -> mlua::Result<T> {
        self.cancelled.store(false, Ordering::Release);
        self.instructions.store(0, Ordering::Release);
        *self.deadline.lock().map_err(|_| mlua::Error::runtime("budget lock poisoned"))? =
            Some(Instant::now() + timeout);
        let result = operation();
        *self.deadline.lock().map_err(|_| mlua::Error::runtime("budget lock poisoned"))? = None;
        result
    }
}

struct LuaPlugin {
    lua: Lua,
    callbacks: HashMap<String, RegistryKey>,
    response: Value,
    budget: RuntimeBudget,
    event_callback: Option<String>,
}

impl LuaPlugin {
    fn load(
        root: &Path,
        entrypoint: &Path,
        bridge: HostBridge,
        cancelled: Arc<AtomicBool>,
    ) -> Result<Self, String> {
        let lua = Lua::new();
        lua.set_memory_limit(MEMORY_BYTES).map_err(|error| error.to_string())?;
        install_sandbox(&lua, root, bridge)?;
        let deadline = Arc::new(Mutex::new(None::<Instant>));
        let instructions = Arc::new(AtomicU64::new(0));
        let hook_deadline = Arc::clone(&deadline);
        let hook_cancelled = Arc::clone(&cancelled);
        let hook_instructions = Arc::clone(&instructions);
        lua.set_hook(
            HookTriggers::new().every_nth_instruction(1000),
            move |_, _| {
                let count = hook_instructions.fetch_add(1000, Ordering::Relaxed) + 1000;
                if hook_cancelled.load(Ordering::Acquire) {
                    return Err(mlua::Error::runtime("callback cancelled"));
                }
                if count > MAX_INSTRUCTIONS {
                    return Err(mlua::Error::runtime("instruction budget exceeded"));
                }
                if hook_deadline
                    .lock()
                    .ok()
                    .and_then(|deadline| *deadline)
                    .is_some_and(|deadline| Instant::now() >= deadline)
                {
                    return Err(mlua::Error::runtime("callback time budget exceeded"));
                }
                Ok(VmState::Continue)
            },
        ).map_err(|error| error.to_string())?;
        let budget = RuntimeBudget { deadline, cancelled, instructions };
        let source = std::fs::read_to_string(entrypoint)
            .map_err(|error| format!("failed to read {}: {error}", entrypoint.display()))?;
        let table: Table = budget
            .run(LOAD_TIMEOUT, || {
                lua.load(&source)
                    .set_name(entrypoint.to_string_lossy().as_ref())
                    .eval()
            })
            .map_err(|error| error.to_string())?;
        let mut callbacks = HashMap::new();
        let mut response = json!({
            "protocol": PROCESS_PLUGIN_V2_PROTOCOL,
            "name": table.get::<Option<String>>("name").ok().flatten(),
            "version": table.get::<Option<String>>("version").ok().flatten(),
            "tools": [], "hooks": [], "eventNamespaces": [],
            "services": {}, "routes": [], "websocketRoutes": [],
            "messageParts": [], "mcp": []
        });
        collect_tools(&lua, &table, &mut callbacks, &mut response)?;
        collect_hooks(&lua, &table, &mut callbacks, &mut response)?;
        collect_services(&lua, &table, &mut callbacks, &mut response)?;
        collect_routes(&lua, &table, &mut callbacks, &mut response)?;
        collect_websocket_routes(&lua, &table, &mut callbacks, &mut response)?;
        for (lua_key, wire_key) in [("messageParts", "messageParts"), ("mcp", "mcp")] {
            if let Ok(value) = table.get::<LuaValue>(lua_key) {
                if !matches!(value, LuaValue::Nil) {
                    response[wire_key] = lua.from_value(value).map_err(|error| error.to_string())?;
                }
            }
        }
        let mut event_callback = None;
        if let Ok(events) = table.get::<Table>("events") {
            if let Ok(namespaces) = events.get::<LuaValue>("namespaces") {
                response["eventNamespaces"] =
                    lua.from_value(namespaces).map_err(|error| error.to_string())?;
            }
            if let Ok(handler) = events.get::<Function>("handler") {
                let key = "event".to_string();
                callbacks.insert(key.clone(), lua.create_registry_value(handler).map_err(|e| e.to_string())?);
                event_callback = Some(key);
            }
        }
        Ok(Self { lua, callbacks, response, budget, event_callback })
    }

    fn call(&self, key: &str, arguments: Value) -> Result<Value, String> {
        let registry = self.callbacks.get(key).ok_or_else(|| format!("unknown callback `{key}`"))?;
        let function: Function = self.lua.registry_value(registry).map_err(|error| error.to_string())?;
        let input = self.lua.to_value(&arguments).map_err(|error| error.to_string())?;
        let output: LuaValue = self
            .budget
            .run(CALLBACK_TIMEOUT, || function.call(input))
            .map_err(|error| error.to_string())?;
        self.lua.from_value(output).map_err(|error| error.to_string())
    }
}

fn install_sandbox(lua: &Lua, root: &Path, bridge: HostBridge) -> Result<(), String> {
    let globals = lua.globals();
    for name in ["io", "os", "debug", "dofile", "loadfile", "load"] {
        globals.set(name, LuaValue::Nil).map_err(|error| error.to_string())?;
    }
    let package: Table = globals.get("package").map_err(|error| error.to_string())?;
    let lua_root = root.join("lua");
    package
        .set("path", "")
        .and_then(|_| package.set("cpath", ""))
        .and_then(|_| package.set("loadlib", LuaValue::Nil))
        .map_err(|error| error.to_string())?;
    let searchers: Table = package.get("searchers").map_err(|error| error.to_string())?;
    let search_root = lua_root.clone();
    searchers
        .set(
            2,
            lua.create_function(move |lua, module: String| {
                if module.is_empty()
                    || module
                        .split('.')
                        .any(|part| part.is_empty() || part == "." || part == ".." || !part.chars().all(|character| character.is_ascii_alphanumeric() || character == '_'))
                {
                    return Ok(LuaValue::String(lua.create_string("invalid module name")?));
                }
                let relative = module.replace('.', "/");
                let candidates = [
                    search_root.join(format!("{relative}.lua")),
                    search_root.join(&relative).join("init.lua"),
                ];
                for candidate in candidates {
                    let Ok(canonical) = candidate.canonicalize() else { continue };
                    let Ok(canonical_root) = search_root.canonicalize() else {
                        return Ok(LuaValue::String(lua.create_string("package has no Lua module root")?));
                    };
                    if !canonical.starts_with(&canonical_root) || !canonical.is_file() {
                        return Ok(LuaValue::String(lua.create_string("module escapes package Lua root")?));
                    }
                    let metadata = std::fs::symlink_metadata(&candidate).map_err(mlua::Error::external)?;
                    if metadata.file_type().is_symlink() {
                        return Ok(LuaValue::String(lua.create_string("symlinked Lua modules are forbidden")?));
                    }
                    let source = std::fs::read_to_string(&canonical).map_err(mlua::Error::external)?;
                    let function = lua.load(&source).set_name(canonical.to_string_lossy().as_ref()).into_function()?;
                    return Ok(LuaValue::Function(function));
                }
                Ok(LuaValue::String(lua.create_string(format!("module `{module}` not found"))?))
            })
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    for index in 3..=8 {
        let _ = searchers.set(index, LuaValue::Nil);
    }
    let neoism = lua.create_table().map_err(|error| error.to_string())?;
    let host = lua.create_table().map_err(|error| error.to_string())?;
    for (name, method) in [
        ("config_get", "host.config.get"),
        ("config_set", "host.config.set"),
        ("workspace_read", "host.workspace.read"),
        ("workspace_write", "host.workspace.write"),
        ("workspace_list", "host.workspace.list"),
        ("event_publish", "host.event.publish"),
        ("network_request", "host.network.request"),
        ("process_spawn", "host.process.spawn"),
        ("process_cancel", "host.process.cancel"),
        ("task_spawn", "host.task.spawn"),
        ("task_cancel", "host.task.cancel"),
        ("secret_use", "host.secret.use"),
        ("secret_read", "host.secret.read"),
        ("prompt_read", "host.prompt.read"),
        ("message_read", "host.message.read"),
        ("response_transform", "host.response.transform"),
        ("provider_call", "host.provider.call"),
        ("policy_call", "host.policy.call"),
    ] {
        let bridge = bridge.clone();
        host.set(
            name,
            lua.create_function(move |lua, value: LuaValue| {
                let params = lua.from_value(value)?;
                let result = bridge.request(method, params)?;
                lua.to_value(&result)
            })
            .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    }
    neoism.set("host", host).map_err(|error| error.to_string())?;
    globals.set("neoism", neoism.clone()).map_err(|error| error.to_string())?;
    let preload: Table = package.get("preload").map_err(|error| error.to_string())?;
    preload
        .set(
            "neoism",
            lua.create_function(move |_, ()| Ok(neoism.clone()))
                .map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
    Ok(())
}

fn register(lua: &Lua, callbacks: &mut HashMap<String, RegistryKey>, key: String, function: Function) -> Result<(), String> {
    callbacks.insert(key, lua.create_registry_value(function).map_err(|error| error.to_string())?);
    Ok(())
}

fn collect_tools(lua: &Lua, table: &Table, callbacks: &mut HashMap<String, RegistryKey>, response: &mut Value) -> Result<(), String> {
    let Ok(tools) = table.get::<Table>("tools") else { return Ok(()) };
    let mut declarations = Vec::new();
    for item in tools.sequence_values::<Table>() {
        let item = item.map_err(|error| error.to_string())?;
        let id: String = item.get("id").map_err(|error| error.to_string())?;
        register(lua, callbacks, format!("tool:{id}"), item.get("execute").map_err(|error| error.to_string())?)?;
        declarations.push(json!({
            "id": id,
            "description": item.get::<Option<String>>("description").ok().flatten().unwrap_or_default(),
            "parameters": lua.from_value::<Value>(item.get::<LuaValue>("parameters").unwrap_or(LuaValue::Nil)).unwrap_or_else(|_| json!({"type":"object"})),
            "outputSchema": lua.from_value::<Value>(item.get::<LuaValue>("outputSchema").unwrap_or(LuaValue::Nil)).unwrap_or(Value::Null)
        }));
    }
    response["tools"] = Value::Array(declarations);
    Ok(())
}

fn collect_hooks(lua: &Lua, table: &Table, callbacks: &mut HashMap<String, RegistryKey>, response: &mut Value) -> Result<(), String> {
    let Ok(hooks) = table.get::<Table>("hooks") else { return Ok(()) };
    let mut names = Vec::new();
    for pair in hooks.pairs::<String, Function>() {
        let (name, function) = pair.map_err(|error| error.to_string())?;
        register(lua, callbacks, format!("hook:{name}"), function)?;
        names.push(Value::String(name));
    }
    response["hooks"] = Value::Array(names);
    Ok(())
}

fn collect_services(lua: &Lua, table: &Table, callbacks: &mut HashMap<String, RegistryKey>, response: &mut Value) -> Result<(), String> {
    let Ok(services) = table.get::<Table>("services") else { return Ok(()) };
    let mut declarations = serde_json::Map::new();
    for (name, callback) in [
        ("agents", "list"), ("commands", "list"), ("skills", "list"),
        ("systemContext", "sections"), ("prompts", "render"), ("config", "load"),
    ] {
        let Ok(items) = services.get::<Table>(name) else { continue };
        let mut output = Vec::new();
        for item in items.sequence_values::<Table>() {
            let item = item.map_err(|error| error.to_string())?;
            let id: String = item.get("id").map_err(|error| error.to_string())?;
            register(lua, callbacks, format!("service:{name}:{id}"), item.get(callback).map_err(|error| error.to_string())?)?;
            output.push(json!({"id": id, "priority": item.get::<Option<i32>>("priority").ok().flatten().unwrap_or(0)}));
        }
        declarations.insert(name.into(), Value::Array(output));
    }
    if let Ok(items) = services.get::<Table>("providers") {
        let mut output = Vec::new();
        for item in items.sequence_values::<Table>() {
            let item = item.map_err(|error| error.to_string())?;
            let descriptor: Value = lua.from_value(item.get("descriptor").map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
            let id = descriptor.get("id").and_then(Value::as_str).ok_or("provider descriptor id is required")?.to_string();
            for callback in ["stream", "metadata", "auth", "route", "media"] {
                if let Ok(function) = item.get::<Function>(callback) {
                    register(lua, callbacks, format!("provider:{callback}:{id}"), function)?;
                }
            }
            output.push(json!({"descriptor": descriptor, "priority": item.get::<Option<i32>>("priority").ok().flatten().unwrap_or(0), "media": item.contains_key("media").unwrap_or(false), "administration": item.contains_key("route").unwrap_or(false)}));
        }
        declarations.insert("providers".into(), Value::Array(output));
    }
    response["services"] = Value::Object(declarations);
    Ok(())
}

fn collect_routes(lua: &Lua, table: &Table, callbacks: &mut HashMap<String, RegistryKey>, response: &mut Value) -> Result<(), String> {
    let Ok(routes) = table.get::<Table>("routes") else { return Ok(()) };
    let mut output = Vec::new();
    for item in routes.sequence_values::<Table>() {
        let item = item.map_err(|error| error.to_string())?;
        let descriptor: Value = lua.from_value(item.get("descriptor").map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
        let id = descriptor.get("id").and_then(Value::as_str).ok_or("route descriptor id is required")?.to_string();
        register(lua, callbacks, format!("route:{id}"), item.get("handle").map_err(|error| error.to_string())?)?;
        output.push(json!({"descriptor": descriptor, "priority": item.get::<Option<i32>>("priority").ok().flatten().unwrap_or(0)}));
    }
    response["routes"] = Value::Array(output);
    Ok(())
}

fn collect_websocket_routes(lua: &Lua, table: &Table, callbacks: &mut HashMap<String, RegistryKey>, response: &mut Value) -> Result<(), String> {
    let Ok(routes) = table.get::<Table>("websocketRoutes") else { return Ok(()) };
    let mut output = Vec::new();
    for item in routes.sequence_values::<Table>() {
        let item = item.map_err(|error| error.to_string())?;
        let descriptor: Value = lua.from_value(item.get("descriptor").map_err(|error| error.to_string())?).map_err(|error| error.to_string())?;
        let id = descriptor.get("id").and_then(Value::as_str).ok_or("WebSocket route descriptor id is required")?.to_string();
        register(lua, callbacks, format!("websocket:open:{id}"), item.get("open").map_err(|error| error.to_string())?)?;
        if let Ok(function) = item.get::<Function>("message") {
            register(lua, callbacks, format!("websocket:message:{id}"), function)?;
        }
        output.push(json!({"descriptor": descriptor, "priority": item.get::<Option<i32>>("priority").ok().flatten().unwrap_or(0)}));
    }
    response["websocketRoutes"] = Value::Array(output);
    Ok(())
}

fn send_stream(writer: &Writer, owner: &ProcessPluginOwner, request_id: u64, stream: ProcessStreamEnvelope) -> Result<(), String> {
    writer.send(&ProcessPluginFrame {
        id: None,
        result: None,
        error: None,
        method: None,
        params: None,
        owner: Some(owner.clone()),
        request_id: Some(request_id),
        stream: Some(stream),
    })
}

fn contained_entrypoint(manifest_path: &Path, manifest: &NeoismPackageManifest) -> Result<(PathBuf, PathBuf), String> {
    let root = manifest_path.parent().ok_or("manifest has no package root")?.canonicalize().map_err(|error| error.to_string())?;
    let agent = manifest.agent.as_ref().ok_or("manifest has no Agent entrypoint")?;
    if agent.runtime != AgentEntrypointRuntime::Lua { return Err("Agent entrypoint is not Lua".into()); }
    let relative = Path::new(&agent.entrypoint);
    if relative.is_absolute() || relative.components().any(|part| !matches!(part, Component::Normal(_))) { return Err("Lua entrypoint is not contained".into()); }
    let entrypoint = root.join(relative).canonicalize().map_err(|error| error.to_string())?;
    if !entrypoint.starts_with(&root) || !entrypoint.is_file() { return Err("Lua entrypoint escapes package root".into()); }
    Ok((root, entrypoint))
}

fn main() -> Result<(), String> {
    let args = Args::parse();
    let raw = std::fs::read_to_string(&args.manifest).map_err(|error| error.to_string())?;
    let manifest: NeoismPackageManifest = serde_json::from_str(&raw).map_err(|error| error.to_string())?;
    let (root, entrypoint) = contained_entrypoint(&args.manifest, &manifest)?;
    let writer = Writer(Arc::new(Mutex::new(io::stdout())));
    let owner = Arc::new(Mutex::new(None));
    let pending = Arc::new(Mutex::new(HashMap::<u64, mpsc::Sender<Result<Value, String>>>::new()));
    let cancelled = Arc::new(AtomicBool::new(false));
    let bridge = HostBridge { writer: writer.clone(), owner: Arc::clone(&owner), pending: Arc::clone(&pending), next_id: Arc::new(AtomicU64::new(1)) };
    let (frames_tx, frames_rx) = mpsc::sync_channel::<Value>(MAX_QUEUE);
    let reader_cancelled = Arc::clone(&cancelled);
    std::thread::Builder::new().name("neoism-agent-lua-input".into()).spawn(move || {
        let mut reader = BufReader::new(io::stdin());
        let mut line = Vec::new();
        loop {
            line.clear();
            let Ok(read) = reader.read_until(b'\n', &mut line) else { break };
            if read == 0 { break; }
            if line.len() > MAX_FRAME_BYTES { continue; }
            let Ok(value) = serde_json::from_slice::<Value>(&line) else { continue };
            if value.get("method").and_then(Value::as_str).is_none() {
                if let Some(id) = value.get("id").and_then(Value::as_u64) {
                    if let Some(sender) = pending.lock().ok().and_then(|mut map| map.remove(&id)) {
                        let result = value.get("error").and_then(Value::as_str).map(|error| Err(error.to_string())).unwrap_or_else(|| Ok(value.get("result").cloned().unwrap_or(Value::Null)));
                        let _ = sender.send(result);
                    }
                }
                continue;
            }
            if matches!(value.get("method").and_then(Value::as_str), Some("$/cancel" | "$/cancelStream" | "shutdown")) { reader_cancelled.store(true, Ordering::Release); }
            if frames_tx.try_send(value).is_err() { break; }
        }
    }).map_err(|error| error.to_string())?;

    let mut plugin: Option<LuaPlugin> = None;
    let mut websocket_streams = HashMap::<String, (u64, String)>::new();
    while let Ok(value) = frames_rx.recv() {
        let frame: ProcessHostFrame = serde_json::from_value(value).map_err(|error| error.to_string())?;
        let id = frame.id;
        let result = match frame.method.as_str() {
            "initialize" => {
                let request: ProcessInitializeRequest = serde_json::from_value(frame.params).map_err(|error| error.to_string())?;
                if request.protocol != PROCESS_PLUGIN_V2_PROTOCOL || request.plugin_id != manifest.id { Err("initialize identity or protocol mismatch".into()) } else {
                    let exact_owner = request.owner.unwrap_or(ProcessPluginOwner { plugin_id: request.plugin_id, instance_id: request.instance_id, package_revision: None, registry_generation: None, scope: None, workspace_id: None, scope_id: None });
                    *owner.lock().map_err(|_| "owner lock poisoned")? = Some(exact_owner);
                    let loaded = LuaPlugin::load(&root, &entrypoint, bridge.clone(), Arc::clone(&cancelled))?;
                    let response = loaded.response.clone(); plugin = Some(loaded); Ok(response)
                }
            }
            "shutdown" => { if let Some(id) = id { writer.send(&ProcessHostReplyFrame { id, result: Some(json!({})), error: None })?; } break; }
            "event" => { if let Some(plugin) = &plugin { if let Some(callback) = &plugin.event_callback { let _ = plugin.call(callback, frame.params); } } continue; }
            "tool.invoke" => callback(&plugin, "tool", "tool", &frame.params),
            "hook.invoke" => callback(&plugin, "hook", "hook", &frame.params),
            "agent.list" => service_callback(&plugin, "agents", &frame.params),
            "command.list" => service_callback(&plugin, "commands", &frame.params),
            "skill.list" => service_callback(&plugin, "skills", &frame.params),
            "systemContext.sections" => service_callback(&plugin, "systemContext", &frame.params),
            "prompt.render" => service_callback(&plugin, "prompts", &frame.params),
            "config.load" => service_callback(&plugin, "config", &frame.params),
            "provider.metadata" | "provider.auth" | "provider.route" | "provider.media" => provider_callback(&plugin, frame.method.trim_start_matches("provider."), &frame.params),
            "provider.stream" => {
                let stream_id = frame.params.get("streamId").and_then(Value::as_str).unwrap_or_default().to_string();
                let service_id = frame.params.get("serviceId").and_then(Value::as_str).unwrap_or_default();
                let output = plugin.as_ref().ok_or("plugin is not initialized")?.call(&format!("provider:stream:{service_id}"), frame.params.clone());
                match output {
                    Ok(Value::Array(items)) if items.len() <= MAX_STREAM_ITEMS => {
                        if let Some(id) = id { writer.send(&ProcessHostReplyFrame { id, result: Some(json!({})), error: None })?; }
                        let exact_owner = owner.lock().map_err(|_| "owner lock poisoned")?.clone().ok_or("owner missing")?;
                        let request_id = frame.params.get("requestId").and_then(Value::as_u64).ok_or("provider stream request id is missing")?;
                        send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::Open { stream_id: stream_id.clone() })?;
                        for item in items { send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::Item { stream_id: stream_id.clone(), value: item })?; }
                        send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::End { stream_id })?;
                        continue;
                    }
                    Ok(_) => Err("provider stream must return a bounded event array".into()),
                    Err(error) => Err(error),
                }
            }
            "route.handle" => {
                let route_id = frame.params.get("routeId").and_then(Value::as_str).unwrap_or_default();
                plugin.as_ref().ok_or("plugin is not initialized")?.call(&format!("route:{route_id}"), frame.params.get("request").cloned().unwrap_or(Value::Null))
            }
            "route.websocket.open" => {
                let route_id = frame.params.get("routeId").and_then(Value::as_str).unwrap_or_default().to_string();
                let stream_id = frame.params.get("streamId").and_then(Value::as_str).unwrap_or_default().to_string();
                let request_id = frame.params.get("requestId").and_then(Value::as_u64).ok_or("WebSocket stream request id is missing")?;
                let initial = plugin.as_ref().ok_or("plugin is not initialized")?.call(&format!("websocket:open:{route_id}"), frame.params.get("request").cloned().unwrap_or(Value::Null))?;
                let items = match initial { Value::Null => Vec::new(), Value::Array(items) if items.len() <= MAX_STREAM_ITEMS => items, _ => return Err("WebSocket open callback must return a bounded message array or nil".into()) };
                if let Some(id) = id { writer.send(&ProcessHostReplyFrame { id, result: Some(json!({})), error: None })?; }
                let exact_owner = owner.lock().map_err(|_| "owner lock poisoned")?.clone().ok_or("owner missing")?;
                send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::Open { stream_id: stream_id.clone() })?;
                for item in items { send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::Item { stream_id: stream_id.clone(), value: item })?; }
                websocket_streams.insert(stream_id, (request_id, route_id));
                continue;
            }
            "route.websocket.message" => {
                let stream_id = frame.params.get("streamId").and_then(Value::as_str).unwrap_or_default().to_string();
                let (request_id, route_id) = websocket_streams.get(&stream_id).cloned().ok_or("unknown WebSocket stream")?;
                let message = frame.params.get("message").cloned().unwrap_or(Value::Null);
                let closing = message.get("kind").and_then(Value::as_str) == Some("close");
                let output = plugin.as_ref().ok_or("plugin is not initialized")?.call(&format!("websocket:message:{route_id}"), message);
                let items = match output { Ok(Value::Null) => Vec::new(), Ok(Value::Array(items)) if items.len() <= MAX_STREAM_ITEMS => items, Ok(_) => return Err("WebSocket message callback must return a bounded message array or nil".into()), Err(error) if error.contains("unknown callback") => Vec::new(), Err(error) => return Err(error) };
                let exact_owner = owner.lock().map_err(|_| "owner lock poisoned")?.clone().ok_or("owner missing")?;
                for item in items { send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::Item { stream_id: stream_id.clone(), value: item })?; }
                if closing { send_stream(&writer, &exact_owner, request_id, ProcessStreamEnvelope::End { stream_id: stream_id.clone() })?; websocket_streams.remove(&stream_id); }
                if let Some(id) = id { writer.send(&ProcessHostReplyFrame { id, result: Some(json!({})), error: None })?; }
                continue;
            }
            "$/cancel" => { cancelled.store(true, Ordering::Release); continue; }
            "$/cancelStream" => {
                cancelled.store(true, Ordering::Release);
                if let Some(stream_id) = frame.params.get("streamId").and_then(Value::as_str) {
                    websocket_streams.remove(stream_id);
                }
                continue;
            }
            method => Err(format!("unsupported Lua plugin method `{method}`")),
        };
        if let Some(id) = id {
            match result { Ok(result) => writer.send(&ProcessHostReplyFrame { id, result: Some(result), error: None })?, Err(error) => writer.send(&ProcessHostReplyFrame { id, result: None, error: Some(error) })? }
        }
    }
    Ok(())
}

fn callback(plugin: &Option<LuaPlugin>, kind: &str, field: &str, params: &Value) -> Result<Value, String> {
    let id = params.get(field).and_then(Value::as_str).unwrap_or_default();
    plugin.as_ref().ok_or("plugin is not initialized".into()).and_then(|plugin| plugin.call(&format!("{kind}:{id}"), params.clone()))
}

fn service_callback(plugin: &Option<LuaPlugin>, kind: &str, params: &Value) -> Result<Value, String> {
    let id = params.get("serviceId").and_then(Value::as_str).unwrap_or_default();
    plugin.as_ref().ok_or("plugin is not initialized".into()).and_then(|plugin| plugin.call(&format!("service:{kind}:{id}"), params.get("request").cloned().unwrap_or(Value::Null)))
}

fn provider_callback(plugin: &Option<LuaPlugin>, operation: &str, params: &Value) -> Result<Value, String> {
    let id = params.get("serviceId").and_then(Value::as_str).unwrap_or_default();
    plugin.as_ref().ok_or("plugin is not initialized".into()).and_then(|plugin| plugin.call(&format!("provider:{operation}:{id}"), params.clone()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn fixture(source: &str) -> (PathBuf, HostBridge, Arc<AtomicBool>) {
        let unique = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("neoism-agent-lua-{unique}"));
        std::fs::create_dir_all(root.join("lua")).unwrap();
        std::fs::write(root.join("main.lua"), source).unwrap();
        let bridge = HostBridge {
            writer: Writer(Arc::new(Mutex::new(io::stdout()))),
            owner: Arc::new(Mutex::new(None)),
            pending: Arc::new(Mutex::new(HashMap::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        };
        (root, bridge, Arc::new(AtomicBool::new(false)))
    }

    #[test]
    fn removes_ambient_system_libraries_and_dynamic_loaders() {
        let (root, bridge, _) = fixture("return {}");
        let lua = Lua::new();
        install_sandbox(&lua, &root, bridge).unwrap();
        for name in ["io", "os", "debug", "load", "loadfile", "dofile"] {
            assert!(matches!(lua.globals().get::<LuaValue>(name).unwrap(), LuaValue::Nil));
        }
        let package: Table = lua.globals().get("package").unwrap();
        assert!(matches!(package.get::<LuaValue>("loadlib").unwrap(), LuaValue::Nil));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn require_rejects_symlinked_modules() {
        use std::os::unix::fs::symlink;
        let (root, bridge, _) = fixture("return {}");
        let outside = root.parent().unwrap().join(format!("{}-outside.lua", root.file_name().unwrap().to_string_lossy()));
        std::fs::write(&outside, "return {}").unwrap();
        symlink(&outside, root.join("lua/escape.lua")).unwrap();
        let lua = Lua::new();
        install_sandbox(&lua, &root, bridge).unwrap();
        let error = lua.load("return require('escape')").eval::<LuaValue>().unwrap_err();
        let message = error.to_string();
        assert!(message.contains("symlinked Lua modules are forbidden") || message.contains("escapes package Lua root"), "{message}");
        std::fs::remove_file(outside).unwrap();
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn instruction_budget_stops_runaway_callbacks() {
        let (root, bridge, cancelled) = fixture(
            "return { tools = {{ id = 'loop', description = '', parameters = {}, execute = function() while true do end end }} }",
        );
        let plugin = LuaPlugin::load(&root, &root.join("main.lua"), bridge, cancelled).unwrap();
        let error = plugin.call("tool:loop", Value::Null).unwrap_err();
        assert!(error.contains("instruction budget exceeded"));
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_callback_values_and_oversized_frames_are_rejected() {
        let (root, bridge, cancelled) = fixture(
            "return { tools = {{ id = 'bad', description = '', parameters = {}, execute = function() return function() end end }} }",
        );
        let plugin = LuaPlugin::load(&root, &root.join("main.lua"), bridge, cancelled).unwrap();
        assert!(plugin.call("tool:bad", Value::Null).is_err());
        let writer = Writer(Arc::new(Mutex::new(io::stdout())));
        assert!(writer.send(&json!({"value": "x".repeat(MAX_FRAME_BYTES)})).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}