use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use mlua::{
    Function, HookTriggers, Lua, LuaSerdeExt, RegistryKey, Table, Value as LuaValue,
    VmState,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

use crate::{
    AutocmdContribution, CommandContribution, ExecutionScope, HostAction,
    KeymapContribution, PanelContribution, PluginBudgets, PluginEvent, PluginHost,
    PluginManifest, PluginOwner, PluginRevision, PluginSnapshot, PluginViewContribution,
    StylePatch, SurfaceItemPatch, SurfacePatch, UiContribution,
};

#[derive(Debug, Error)]
pub enum LuaPluginError {
    #[error("{0}")]
    Lua(#[from] mlua::Error),
    #[error("unable to read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("Lua plugin state lock poisoned")]
    Poisoned,
    #[error(transparent)]
    Ecosystem(#[from] crate::PluginEcosystemError),
    #[error("invalid plugin snapshot: {0}")]
    InvalidSnapshot(String),
}

#[derive(Default)]
struct BuildState {
    snapshot: PluginSnapshot,
    callbacks: HashMap<String, RegistryKey>,
    next_callback: u64,
    next_contribution_order: u64,
    augroups: BTreeSet<String>,
    owner: PluginOwner,
}

impl BuildState {
    fn callback(&mut self, lua: &Lua, function: Function) -> mlua::Result<String> {
        self.next_callback += 1;
        let revision = if self.owner.revision.0.is_empty() {
            "0"
        } else {
            &self.owner.revision.0
        };
        let id = format!(
            "lua:{}@{}:{}",
            self.owner.plugin_id, revision, self.next_callback
        );
        self.callbacks
            .insert(id.clone(), lua.create_registry_value(function)?);
        Ok(id)
    }
}

pub struct LuaRuntime {
    lua: Lua,
    snapshot: PluginSnapshot,
    callbacks: HashMap<String, RegistryKey>,
    fired_once: Mutex<HashSet<String>>,
    active_autocmds: Mutex<HashSet<String>>,
    config_dir: PathBuf,
    owner: PluginOwner,
    host: Arc<dyn PluginHost>,
    budgets: PluginBudgets,
    event_depth: AtomicUsize,
    callbacks_this_event: AtomicUsize,
}

impl LuaRuntime {
    pub fn load(
        config_dir: impl Into<PathBuf>,
        host: Arc<dyn PluginHost>,
    ) -> Result<Self, LuaPluginError> {
        let config_dir = config_dir.into();
        let entrypoint = config_dir.join("init.lua");
        let package_root = config_dir.join("lua");
        let owner = PluginOwner {
            plugin_id: "neoism.user-init".into(),
            revision: source_revision(&entrypoint, &package_root)?,
        };
        let runtime = match Self::load_entry(
            config_dir,
            package_root,
            entrypoint,
            owner.clone(),
            PluginBudgets::default(),
            host.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                host.retire_owner(&owner);
                return Err(error);
            }
        };
        runtime
            .host
            .activate_owner(&runtime.owner)
            .map_err(LuaPluginError::InvalidSnapshot)?;
        Ok(runtime)
    }

    fn load_entry(
        config_dir: PathBuf,
        package_root: PathBuf,
        entrypoint: PathBuf,
        owner: PluginOwner,
        budgets: PluginBudgets,
        host: Arc<dyn PluginHost>,
    ) -> Result<Self, LuaPluginError> {
        let lua = Lua::new();
        lua.set_memory_limit(budgets.memory_bytes)?;
        let state = Arc::new(Mutex::new(BuildState {
            snapshot: PluginSnapshot::empty(),
            owner: owner.clone(),
            ..BuildState::default()
        }));
        install_sandbox(&lua, &package_root)?;
        let module = build_module(&lua, state.clone(), host.clone(), owner.clone())?;
        let preload: Table = lua.globals().get::<Table>("package")?.get("preload")?;
        let loaded = module.clone();
        preload.set(
            "neoism",
            lua.create_function(move |_, ()| Ok(loaded.clone()))?,
        )?;
        lua.globals().set("neoism", module)?;

        let init = entrypoint;
        if init.exists() {
            let source = std::fs::read_to_string(&init).map_err(|source| {
                LuaPluginError::Read {
                    path: init.clone(),
                    source,
                }
            })?;
            run_with_budget(&lua, Duration::from_millis(budgets.load_millis), || {
                lua.load(&source)
                    .set_name(init.to_string_lossy().as_ref())
                    .exec()
            })?;
        }

        let mut built = {
            let mut state = state.lock().map_err(|_| LuaPluginError::Poisoned)?;
            std::mem::take(&mut *state)
        };
        built
            .snapshot
            .surface_layout
            .validate()
            .map_err(LuaPluginError::InvalidSnapshot)?;
        Ok(Self {
            lua,
            snapshot: built.snapshot,
            callbacks: std::mem::take(&mut built.callbacks),
            fired_once: Mutex::new(HashSet::new()),
            active_autocmds: Mutex::new(HashSet::new()),
            config_dir,
            owner,
            host,
            budgets,
            event_depth: AtomicUsize::new(0),
            callbacks_this_event: AtomicUsize::new(0),
        })
    }

    pub fn snapshot(&self) -> &PluginSnapshot {
        &self.snapshot
    }

    pub fn config_dir(&self) -> &Path {
        &self.config_dir
    }

    pub fn owner(&self) -> &PluginOwner {
        &self.owner
    }

    pub fn invoke(
        &self,
        callback: &str,
        event: PluginEvent,
    ) -> Result<Value, LuaPluginError> {
        self.enter_event()?;
        let result = self.invoke_inner(callback, event);
        self.leave_event();
        result
    }

    fn invoke_inner(
        &self,
        callback: &str,
        event: PluginEvent,
    ) -> Result<Value, LuaPluginError> {
        let callback_count = self.callbacks_this_event.fetch_add(1, Ordering::AcqRel) + 1;
        if callback_count > self.budgets.max_callbacks_per_event {
            return Err(LuaPluginError::InvalidSnapshot(
                "Lua callback event-storm budget exceeded".into(),
            ));
        }
        let key = self.callbacks.get(callback).ok_or_else(|| {
            mlua::Error::runtime(format!("unknown Lua callback `{callback}`"))
        })?;
        let function: Function = self.lua.registry_value(key)?;
        let input = self.lua.to_value(&event)?;
        let output: LuaValue = run_with_budget(
            &self.lua,
            Duration::from_millis(self.budgets.callback_millis),
            || function.call(input),
        )?;
        Ok(self.lua.from_value(output)?)
    }

    pub fn emit(&mut self, event: PluginEvent) -> Result<Vec<Value>, LuaPluginError> {
        self.enter_event()?;
        let result = self.emit_inner(event);
        self.leave_event();
        result
    }

    fn emit_inner(&mut self, event: PluginEvent) -> Result<Vec<Value>, LuaPluginError> {
        event.validate().map_err(LuaPluginError::InvalidSnapshot)?;
        let panels = self
            .snapshot
            .panels
            .iter()
            .enumerate()
            .filter_map(|(index, panel)| {
                panel
                    .render_callback
                    .clone()
                    .map(|callback| (index, callback))
            })
            .collect::<Vec<_>>();
        for (index, callback) in panels {
            let value = self.invoke_inner(&callback, event.clone())?;
            if !value.is_null() {
                self.snapshot.panels[index].content = serde_json::from_value(value)
                    .map_err(|error| mlua::Error::runtime(error.to_string()))?;
            }
        }
        let callbacks = self
            .snapshot
            .autocmds
            .iter()
            .filter(|autocmd| autocmd.event == event.name || autocmd.event == "*")
            .filter(|autocmd| {
                autocmd.pattern.as_deref().is_none_or(|pattern| {
                    pattern == "*"
                        || event
                            .payload
                            .get("path")
                            .and_then(Value::as_str)
                            .is_some_and(|path| wildcard_match(pattern, path))
                })
            })
            .filter(|autocmd| {
                self.event_depth.load(Ordering::Acquire) <= 1 || autocmd.nested
            })
            .map(|autocmd| {
                (
                    autocmd.callback.clone(),
                    autocmd.once,
                    autocmd.priority,
                    autocmd.order,
                )
            })
            .collect::<Vec<_>>();
        let mut callbacks = callbacks;
        callbacks.sort_by_key(|(_, _, priority, order)| {
            (std::cmp::Reverse(*priority), *order)
        });
        let mut output = Vec::with_capacity(callbacks.len());
        for (callback, once, _, _) in callbacks {
            if once {
                let already_fired = {
                    let mut fired = self
                        .fired_once
                        .lock()
                        .map_err(|_| LuaPluginError::Poisoned)?;
                    !fired.insert(callback.clone())
                };
                if already_fired {
                    continue;
                }
            }
            let inserted = self
                .active_autocmds
                .lock()
                .map_err(|_| LuaPluginError::Poisoned)?
                .insert(callback.clone());
            if !inserted {
                continue;
            }
            let result = self.invoke_inner(&callback, event.clone());
            self.active_autocmds
                .lock()
                .map_err(|_| LuaPluginError::Poisoned)?
                .remove(&callback);
            output.push(result?);
        }
        Ok(output)
    }

    fn enter_event(&self) -> Result<(), LuaPluginError> {
        let depth = self.event_depth.fetch_add(1, Ordering::AcqRel) + 1;
        if depth == 1 {
            self.callbacks_this_event.store(0, Ordering::Release);
        }
        if depth > self.budgets.max_event_depth {
            self.event_depth.fetch_sub(1, Ordering::AcqRel);
            return Err(LuaPluginError::InvalidSnapshot(
                "Lua nested event depth budget exceeded".into(),
            ));
        }
        Ok(())
    }

    fn leave_event(&self) {
        self.event_depth.fetch_sub(1, Ordering::AcqRel);
    }
}

/// A fully isolated plugin VM. Building a candidate never mutates an existing
/// runtime; callers can inspect its immutable snapshot before atomically
/// replacing their live instance.
pub struct PluginRuntime {
    inner: LuaRuntime,
}

pub struct PluginRuntimeCandidate {
    runtime: PluginRuntime,
}

impl PluginRuntime {
    pub fn build_candidate(
        root: impl Into<PathBuf>,
        manifest: &PluginManifest,
        revision: PluginRevision,
        host: Arc<dyn PluginHost>,
    ) -> Result<PluginRuntimeCandidate, LuaPluginError> {
        Self::build_candidate_with_budgets(
            root,
            manifest,
            revision,
            PluginBudgets::default(),
            host,
        )
    }

    pub fn build_candidate_with_budgets(
        root: impl Into<PathBuf>,
        manifest: &PluginManifest,
        revision: PluginRevision,
        budgets: PluginBudgets,
        host: Arc<dyn PluginHost>,
    ) -> Result<PluginRuntimeCandidate, LuaPluginError> {
        let root = root.into();
        crate::validate_manifest(manifest, &root)?;
        let owner = PluginOwner {
            plugin_id: manifest.id.clone(),
            revision,
        };
        let entrypoint = root.join(manifest.editor_entrypoint());
        let package_root = root.join("lua");
        let inner = match LuaRuntime::load_entry(
            root,
            package_root,
            entrypoint,
            owner.clone(),
            budgets,
            host.clone(),
        ) {
            Ok(runtime) => runtime,
            Err(error) => {
                host.retire_owner(&owner);
                return Err(error);
            }
        };
        Ok(PluginRuntimeCandidate {
            runtime: PluginRuntime { inner },
        })
    }

    pub fn snapshot(&self) -> &PluginSnapshot {
        self.inner.snapshot()
    }
    pub fn owner(&self) -> &PluginOwner {
        self.inner.owner()
    }
    pub fn invoke(
        &self,
        callback: &str,
        event: PluginEvent,
    ) -> Result<Value, LuaPluginError> {
        self.inner.invoke(callback, event)
    }
    pub fn emit(&mut self, event: PluginEvent) -> Result<Vec<Value>, LuaPluginError> {
        self.inner.emit(event)
    }
    pub fn complete_command(
        &self,
        id: &str,
        prefix: &str,
    ) -> Result<Vec<String>, LuaPluginError> {
        let Some(command) = self.inner.snapshot().commands.iter().find(|command| {
            command.id == id || command.aliases.iter().any(|alias| alias == id)
        }) else {
            return Ok(Vec::new());
        };
        let mut values = command
            .completions
            .iter()
            .filter(|value| value.starts_with(prefix))
            .cloned()
            .collect::<Vec<_>>();
        if let Some(callback) = &command.completion_callback {
            let event = PluginEvent::new(
                crate::PluginEventKind::Command,
                serde_json::json!({ "id": command.id, "completion": true, "prefix": prefix }),
                ExecutionScope::Local,
                Some("command-completion".into()),
            ).map_err(LuaPluginError::InvalidSnapshot)?;
            let value = self.inner.invoke(callback, event)?;
            if let Some(dynamic) = value.as_array() {
                values
                    .extend(dynamic.iter().filter_map(Value::as_str).map(str::to_owned));
            }
        }
        values.sort();
        values.dedup();
        Ok(values)
    }
}

impl PluginRuntimeCandidate {
    pub fn snapshot(&self) -> &PluginSnapshot {
        self.runtime.snapshot()
    }
    pub fn owner(&self) -> &PluginOwner {
        self.runtime.owner()
    }
    pub fn activate(self) -> PluginRuntime {
        let _ = self
            .runtime
            .inner
            .host
            .activate_owner(&self.runtime.inner.owner);
        self.runtime
    }
}

pub(crate) fn run_with_budget<T>(
    lua: &Lua,
    budget: Duration,
    callback: impl FnOnce() -> mlua::Result<T>,
) -> mlua::Result<T> {
    let deadline = Instant::now() + budget;
    lua.set_hook(
        HookTriggers::new().every_nth_instruction(10_000),
        move |_, _| {
            if Instant::now() >= deadline {
                Err(mlua::Error::runtime(
                    "Lua callback exceeded its execution budget",
                ))
            } else {
                Ok(VmState::Continue)
            }
        },
    )?;
    let result = callback();
    lua.remove_hook();
    result
}

fn wildcard_match(pattern: &str, value: &str) -> bool {
    let Some((prefix, suffix)) = pattern.split_once('*') else {
        return pattern == value;
    };
    value.starts_with(prefix) && value.ends_with(suffix)
}

fn install_sandbox(lua: &Lua, package_root: &Path) -> mlua::Result<()> {
    let globals = lua.globals();
    globals.set("io", LuaValue::Nil)?;
    globals.set("os", LuaValue::Nil)?;
    globals.set("debug", LuaValue::Nil)?;
    globals.set("dofile", LuaValue::Nil)?;
    globals.set("loadfile", LuaValue::Nil)?;
    let package: Table = globals.get("package")?;
    let base = package_root.to_string_lossy().replace('\\', "/");
    package.set("path", format!("{base}/?.lua;{base}/?/init.lua"))?;
    package.set("cpath", "")?;
    package.set("loadlib", LuaValue::Nil)?;
    let searchers: Table = package.get("searchers")?;
    let loader_root = package_root.to_path_buf();
    searchers.set(
        2,
        lua.create_function(move |lua, module: String| {
            validate_module_name(&module)?;
            let relative = module.replace('.', "/");
            let direct = loader_root.join(format!("{relative}.lua"));
            let nested = loader_root.join(relative).join("init.lua");
            let path = if direct.is_file() { direct } else { nested };
            let canonical_root =
                loader_root.canonicalize().map_err(mlua::Error::external)?;
            let canonical_path = path.canonicalize().map_err(|_| {
                mlua::Error::runtime(format!(
                    "module `{module}` was not found in plugin root"
                ))
            })?;
            if !canonical_path.starts_with(canonical_root) {
                return Err(mlua::Error::runtime(format!(
                    "module `{module}` escapes plugin root"
                )));
            }
            let source = std::fs::read_to_string(&canonical_path)
                .map_err(mlua::Error::external)?;
            let name = canonical_path.to_string_lossy();
            let loader = lua.load(&source).set_name(name.as_ref()).into_function()?;
            Ok((loader, name.into_owned()))
        })?,
    )?;
    searchers.set(3, LuaValue::Nil)?;
    searchers.set(4, LuaValue::Nil)?;
    // `package.path = root/?.lua` alone is not containment: a module name such
    // as `../sibling` substitutes traversal into `?`. Keep standard require
    // semantics while admitting only dotted module identifiers.
    let builtin_require: Function = globals.get("require")?;
    globals.set(
        "require",
        lua.create_function(move |_, module: String| {
            validate_module_name(&module)?;
            builtin_require.call::<LuaValue>(module)
        })?,
    )?;
    Ok(())
}

fn validate_module_name(module: &str) -> mlua::Result<()> {
    let valid = module.split('.').all(|part| {
        !part.is_empty()
            && part
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    });
    if valid {
        Ok(())
    } else {
        Err(mlua::Error::runtime(format!(
            "invalid or non-contained module name `{module}`"
        )))
    }
}

fn build_module(
    lua: &Lua,
    state: Arc<Mutex<BuildState>>,
    host: Arc<dyn PluginHost>,
    owner: PluginOwner,
) -> mlua::Result<Table> {
    let neo = lua.create_table()?;
    neo.set("api_version", crate::API_VERSION)?;
    neo.set("contract", lua.to_value(&crate::contract_schema())?)?;

    let setup_state = state.clone();
    neo.set(
        "setup",
        lua.create_function(move |lua, value: Table| {
            let mut patch: Value = lua.from_value(LuaValue::Table(value))?;
            normalize_config_keys(&mut patch);
            setup_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .config_patch = patch;
            Ok(())
        })?,
    )?;

    let ui = lua.create_table()?;
    let style_state = state.clone();
    ui.set(
        "style",
        lua.create_function(move |lua, (selector, value): (String, Table)| {
            let style: StylePatch = lua.from_value(LuaValue::Table(value))?;
            style_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .styles
                .insert(selector, style);
            Ok(())
        })?,
    )?;
    let contribution_state = state.clone();
    ui.set(
        "contribute",
        lua.create_function(move |lua, value: Table| {
            let item: UiContribution = lua.from_value(LuaValue::Table(value))?;
            contribution_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .contributions
                .push(item);
            Ok(())
        })?,
    )?;
    let surface_state = state.clone();
    let view_state = state.clone();
    let view_owner = owner.clone();
    ui.set(
        "view",
        lua.create_function(move |lua, (id, value): (String, Table)| {
            let mut view: PluginViewContribution =
                lua.from_value(LuaValue::Table(value))?;
            view.id = id;
            view.owner = view_owner.clone();
            view.validate().map_err(mlua::Error::runtime)?;
            let mut state = view_state.lock().map_err(lock_error)?;
            fn flatten(
                view_id: &str,
                nodes: &[crate::PluginViewNode],
                output: &mut Vec<UiContribution>,
            ) {
                for node in nodes {
                    output.push(UiContribution {
                        id: format!("{view_id}:{}", node.id),
                        slot: node.kind.clone(),
                        text: node.label.clone(),
                        icon: None,
                        tooltip: node.role.clone(),
                        command: node.action.clone(),
                        style: None,
                        priority: 0,
                    });
                    flatten(view_id, &node.children, output);
                }
            }
            let mut content = Vec::new();
            flatten(&view.id, &view.nodes, &mut content);
            let location = match view.kind {
                crate::PluginViewKind::Tree | crate::PluginViewKind::Inspector => {
                    crate::PanelLocation::Left
                }
                crate::PluginViewKind::Toolbar | crate::PluginViewKind::Breadcrumb => {
                    crate::PanelLocation::Center
                }
                crate::PluginViewKind::AgentTimeline
                | crate::PluginViewKind::ApprovalCard => crate::PanelLocation::Bottom,
                _ => crate::PanelLocation::Overlay,
            };
            state.snapshot.panels.retain(|panel| panel.id != view.id);
            state.snapshot.panels.push(PanelContribution {
                id: view.id.clone(),
                title: view.title.clone(),
                icon: None,
                location,
                visible: false,
                render_callback: None,
                content,
            });
            state.snapshot.views.retain(|current| current.id != view.id);
            state.snapshot.views.push(view);
            Ok(())
        })?,
    )?;
    ui.set(
        "surface",
        lua.create_function(move |lua, (id, value): (String, Table)| {
            let patch: SurfacePatch = lua.from_value(LuaValue::Table(value))?;
            surface_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .surface_layout
                .surfaces
                .entry(id)
                .or_default()
                .overlay(&patch);
            Ok(())
        })?,
    )?;
    let item_state = state.clone();
    ui.set(
        "item",
        lua.create_function(move |lua, (id, value): (String, Table)| {
            let patch: SurfaceItemPatch = lua.from_value(LuaValue::Table(value))?;
            item_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .surface_layout
                .items
                .entry(id)
                .or_default()
                .overlay(&patch);
            Ok(())
        })?,
    )?;
    neo.set("ui", ui)?;

    let extension = lua.create_table()?;
    let extension_state = state.clone();
    let extension_owner = owner.clone();
    extension.set(
        "register",
        lua.create_function(move |lua, value: Table| {
            let contribution: crate::PlatformContribution =
                lua.from_value(LuaValue::Table(value))?;
            contribution.validate().map_err(mlua::Error::runtime)?;
            extension_state
                .lock()
                .map_err(lock_error)?
                .snapshot
                .platform
                .push(crate::OwnedPlatformContribution {
                    owner: extension_owner.clone(),
                    contribution,
                });
            Ok(())
        })?,
    )?;
    extension.set(
        "capabilities",
        lua.to_value(&crate::platform_capabilities())?,
    )?;
    neo.set("extension", extension)?;

    let command = lua.create_table()?;
    let command_state = state.clone();
    command.set(
        "register",
        lua.create_function(
            move |lua, (id, function, options): (String, Function, Option<Table>)| {
                #[derive(Default, Deserialize)]
                #[serde(default)]
                struct Options {
                    title: String,
                    description: String,
                    scope: ExecutionScope,
                    arguments_schema: Option<Value>,
                    completions: Vec<String>,
                    accepts_range: bool,
                    accepts_count: bool,
                    accepts_bang: bool,
                    aliases: Vec<String>,
                    result_schema: Option<Value>,
                }
                let completion_function = options
                    .as_ref()
                    .and_then(|table| table.get::<Option<Function>>("complete").ok())
                    .flatten();
                if completion_function.is_some() {
                    options
                        .as_ref()
                        .expect("completion table")
                        .set("complete", LuaValue::Nil)?;
                }
                let options: Options = options
                    .map(|table| lua.from_value(LuaValue::Table(table)))
                    .transpose()?
                    .unwrap_or_default();
                let mut state = command_state.lock().map_err(lock_error)?;
                let callback = state.callback(lua, function)?;
                let completion_callback = completion_function
                    .map(|function| state.callback(lua, function))
                    .transpose()?;
                state.snapshot.commands.push(CommandContribution {
                    id,
                    callback,
                    title: options.title,
                    description: options.description,
                    scope: options.scope,
                    arguments_schema: options
                        .arguments_schema
                        .unwrap_or_else(|| Value::Object(Default::default())),
                    completions: options.completions,
                    accepts_range: options.accepts_range,
                    accepts_count: options.accepts_count,
                    accepts_bang: options.accepts_bang,
                    aliases: options.aliases,
                    completion_callback,
                    result_schema: options
                        .result_schema
                        .unwrap_or_else(|| Value::Object(Default::default())),
                });
                Ok(())
            },
        )?,
    )?;
    let execute_host = host.clone();
    let execute_owner = owner.clone();
    command.set(
        "execute",
        lua.create_function(move |lua, (id, arguments): (String, Option<LuaValue>)| {
            let arguments = arguments
                .map(|value| lua.from_value(value))
                .transpose()?
                .unwrap_or(Value::Null);
            let value = execute_host
                .dispatch(HostAction {
                    namespace: "command".into(),
                    action: "execute".into(),
                    arguments: serde_json::json!({ "id": id, "arguments": arguments }),
                    scope: ExecutionScope::Local,
                    invocation_id: None,
                    owner: Some(execute_owner.clone()),
                })
                .map_err(mlua::Error::runtime)?;
            lua.to_value(&value)
        })?,
    )?;
    let invoke_host = host.clone();
    let invoke_owner = owner.clone();
    command.set(
        "invoke",
        lua.create_function(move |lua, request: Table| {
            let request: crate::PluginCommandRequest =
                lua.from_value(LuaValue::Table(request))?;
            let value = invoke_host
                .dispatch(HostAction {
                    namespace: "command".into(),
                    action: "execute".into(),
                    arguments: serde_json::to_value(request)
                        .map_err(mlua::Error::external)?,
                    scope: ExecutionScope::Local,
                    invocation_id: None,
                    owner: Some(invoke_owner.clone()),
                })
                .map_err(mlua::Error::runtime)?;
            lua.to_value(&value)
        })?,
    )?;
    let cancel_host = host.clone();
    let cancel_owner = owner.clone();
    command.set(
        "cancel",
        lua.create_function(move |lua, request: Table| {
            let request: crate::PluginCommandCancelRequest =
                lua.from_value(LuaValue::Table(request))?;
            let value = cancel_host
                .dispatch(HostAction {
                    namespace: "command".into(),
                    action: "cancel".into(),
                    arguments: serde_json::to_value(request)
                        .map_err(mlua::Error::external)?,
                    scope: ExecutionScope::Local,
                    invocation_id: None,
                    owner: Some(cancel_owner.clone()),
                })
                .map_err(mlua::Error::runtime)?;
            lua.to_value(&value)
        })?,
    )?;
    neo.set("command", command)?;

    let keymap = lua.create_table()?;
    let keymap_state = state.clone();
    keymap.set(
        "set",
        lua.create_function(
            move |lua,
                  (mode, key, target, options): (
                String,
                String,
                LuaValue,
                Option<Table>,
            )| {
                let when = options
                    .as_ref()
                    .and_then(|table| table.get::<Option<String>>("when").ok())
                    .flatten();
                let priority = options
                    .as_ref()
                    .and_then(|table| table.get::<Option<i32>>("priority").ok())
                    .flatten()
                    .unwrap_or(0);
                let fallback = options
                    .as_ref()
                    .and_then(|table| table.get::<Option<bool>>("fallback").ok())
                    .flatten()
                    .unwrap_or(true);
                let mut state = keymap_state.lock().map_err(lock_error)?;
                let command = match target {
                    LuaValue::String(command) => command.to_str()?.to_string(),
                    LuaValue::Function(function) => {
                        let callback = state.callback(lua, function)?;
                        let id = format!("lua.keymap.{}", state.next_callback);
                        state.snapshot.commands.push(CommandContribution {
                            id: id.clone(),
                            callback,
                            title: String::new(),
                            description: String::new(),
                            scope: ExecutionScope::Local,
                            arguments_schema: Value::Object(Default::default()),
                            completions: Vec::new(),
                            accepts_range: false,
                            accepts_count: false,
                            accepts_bang: false,
                            aliases: Vec::new(),
                            completion_callback: None,
                            result_schema: Value::Object(Default::default()),
                        });
                        id
                    }
                    _ => {
                        return Err(mlua::Error::runtime(
                            "keymap target must be a command id or function",
                        ))
                    }
                };
                state.next_contribution_order += 1;
                let order = state.next_contribution_order;
                state.snapshot.keymaps.push(KeymapContribution {
                    mode,
                    key,
                    command,
                    when,
                    priority,
                    order,
                    fallback,
                });
                Ok(())
            },
        )?,
    )?;
    neo.set("keymap", keymap)?;

    for kind in ["motion", "operator", "text_object", "input"] {
        neo.set(kind, input_registry(lua, state.clone(), kind)?)?;
    }

    let option = lua.create_table()?;
    let option_state = state.clone();
    option.set(
        "set",
        lua.create_function(move |lua, value: Table| {
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase", deny_unknown_fields)]
            struct Request {
                name: crate::EditorOptionName,
                value: Value,
                scope: crate::PluginStateScope,
                #[serde(default)]
                target: Option<String>,
                #[serde(default)]
                priority: i32,
            }
            let request: Request = lua.from_value(LuaValue::Table(value))?;
            if request.scope == crate::PluginStateScope::Plugin {
                return Err(mlua::Error::runtime(
                    "editor options require document, pane, tab, or workspace scope",
                ));
            }
            request
                .name
                .validate(&request.value)
                .map_err(mlua::Error::runtime)?;
            let mut state = option_state.lock().map_err(lock_error)?;
            state.next_contribution_order += 1;
            let order = state.next_contribution_order;
            let owner = state.owner.clone();
            state
                .snapshot
                .editor_options
                .push(crate::EditorOptionContribution {
                    owner,
                    name: request.name,
                    value: request.value,
                    scope: request.scope,
                    target: request.target,
                    priority: request.priority,
                    order,
                });
            Ok(())
        })?,
    )?;
    neo.set("option", option)?;

    let augroup_state = state.clone();
    neo.set(
        "augroup",
        lua.create_function(move |lua, (name, options): (String, Option<Table>)| {
            if name.trim().is_empty() {
                return Err(mlua::Error::runtime("augroup name cannot be empty"));
            }
            #[derive(Default, Deserialize)]
            #[serde(default)]
            struct Options {
                clear: bool,
                delete: bool,
            }
            let options: Options = options
                .map(|table| lua.from_value(LuaValue::Table(table)))
                .transpose()?
                .unwrap_or_default();
            let mut state = augroup_state.lock().map_err(lock_error)?;
            if options.clear || options.delete {
                state
                    .snapshot
                    .autocmds
                    .retain(|autocmd| autocmd.group.as_deref() != Some(&name));
            }
            if options.delete {
                state.augroups.remove(&name);
            } else {
                state.augroups.insert(name.clone());
            }
            Ok(name)
        })?,
    )?;

    let autocmd_state = state.clone();
    neo.set(
        "autocmd",
        lua.create_function(
            move |lua, (event, function, options): (String, Function, Option<Table>)| {
                if event != "*" {
                    crate::event_contract(&event).map_err(mlua::Error::runtime)?;
                }
                #[derive(Default, Deserialize)]
                #[serde(default)]
                struct Options {
                    pattern: Option<String>,
                    once: bool,
                    scope: ExecutionScope,
                    group: Option<String>,
                    nested: bool,
                    priority: i32,
                }
                let options: Options = options
                    .map(|table| lua.from_value(LuaValue::Table(table)))
                    .transpose()?
                    .unwrap_or_default();
                let mut state = autocmd_state.lock().map_err(lock_error)?;
                if options
                    .group
                    .as_ref()
                    .is_some_and(|group| !state.augroups.contains(group))
                {
                    return Err(mlua::Error::runtime(
                        "autocmd group is unknown or deleted",
                    ));
                }
                let callback = state.callback(lua, function)?;
                state.next_contribution_order =
                    state.next_contribution_order.saturating_add(1);
                let order = state.next_contribution_order;
                state.snapshot.autocmds.push(AutocmdContribution {
                    event,
                    callback,
                    pattern: options.pattern,
                    once: options.once,
                    scope: options.scope,
                    group: options.group,
                    nested: options.nested,
                    priority: options.priority,
                    order,
                });
                Ok(())
            },
        )?,
    )?;

    let panel_state = state.clone();
    let panel = namespace(lua, "panel", host.clone(), owner.clone())?;
    panel.set(
        "register",
        lua.create_function(move |lua, (id, value): (String, Table)| {
            let render = value
                .get::<Option<Function>>("render")?
                .or(value.get::<Option<Function>>("render_callback")?);
            value.set("render", LuaValue::Nil)?;
            value.set("render_callback", LuaValue::Nil)?;
            value.set("id", id.clone())?;
            let mut panel: PanelContribution = lua.from_value(LuaValue::Table(value))?;
            panel.id = id;
            if let Some(render) = render {
                let output: LuaValue = render.call(())?;
                if !matches!(output, LuaValue::Nil) {
                    panel.content = lua.from_value(output)?;
                }
                let mut state = panel_state.lock().map_err(lock_error)?;
                panel.render_callback = Some(state.callback(lua, render)?);
                state.snapshot.panels.push(panel);
            } else {
                panel_state
                    .lock()
                    .map_err(lock_error)?
                    .snapshot
                    .panels
                    .push(panel);
            }
            Ok(())
        })?,
    )?;
    neo.set("panel", panel)?;

    for &name in crate::contract::RUNTIME_HOST_NAMESPACES {
        neo.set(name, namespace(lua, name, host.clone(), owner.clone())?)?;
    }
    Ok(neo)
}

fn namespace(
    lua: &Lua,
    name: &'static str,
    host: Arc<dyn PluginHost>,
    owner: PluginOwner,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    let query_host = host.clone();
    table.set(
        "query",
        lua.create_function(
            move |lua, (operation, args): (String, Option<LuaValue>)| {
                let args = args
                    .map(|value| lua.from_value(value))
                    .transpose()?
                    .unwrap_or(Value::Null);
                let value = query_host
                    .query(name, &operation, args)
                    .map_err(mlua::Error::runtime)?;
                lua.to_value(&value)
            },
        )?,
    )?;
    let call_host = host.clone();
    let call_owner = owner.clone();
    table.set(
        "call",
        lua.create_function(move |lua, (action, args, scope): (String, Option<LuaValue>, Option<String>)| {
            let arguments = args
                .map(|value| lua.from_value(value))
                .transpose()?
                .unwrap_or(Value::Null);
            let value = call_host
                .dispatch(HostAction {
                    namespace: name.into(),
                    action,
                    arguments,
                    scope: parse_scope(scope.as_deref()),
                    invocation_id: None,
                    owner: Some(call_owner.clone()),
                })
                .map_err(mlua::Error::runtime)?;
            lua.to_value(&value)
        })?,
    )?;
    if name == "lsp" {
        let request_host = host.clone();
        let request_owner = owner.clone();
        table.set(
            "request",
            lua.create_function(
                move |lua, (operation, args): (String, Option<LuaValue>)| {
                    let arguments = args
                        .map(|value| lua.from_value(value))
                        .transpose()?
                        .unwrap_or(Value::Null);
                    let value = request_host
                        .dispatch(HostAction {
                            namespace: "lsp".into(),
                            action: "request".into(),
                            arguments: serde_json::json!({
                                "operation": operation,
                                "arguments": arguments,
                            }),
                            scope: ExecutionScope::Local,
                            invocation_id: None,
                            owner: Some(request_owner.clone()),
                        })
                        .map_err(mlua::Error::runtime)?;
                    lua.to_value(&value)
                },
            )?,
        )?;
    }
    for &operation in crate::contract::RUNTIME_QUERY_METHODS {
        let query_host = host.clone();
        table.set(
            operation,
            lua.create_function(move |lua, args: Option<LuaValue>| {
                let arguments = args
                    .map(|value| lua.from_value(value))
                    .transpose()?
                    .unwrap_or(Value::Null);
                let value = query_host
                    .query(name, operation, arguments)
                    .map_err(mlua::Error::runtime)?;
                lua.to_value(&value)
            })?,
        )?;
    }
    for &action in crate::contract::RUNTIME_ACTION_METHODS {
        // `lsp.request(operation, args)` has a typed compatibility wrapper
        // above; do not replace it with the one-argument generic action shim.
        if name == "lsp" && action == "request" {
            continue;
        }
        let action_host = host.clone();
        let action_owner = owner.clone();
        table.set(
            action,
            lua.create_function(move |lua, args: Option<LuaValue>| {
                let arguments = args
                    .map(|value| lua.from_value(value))
                    .transpose()?
                    .unwrap_or(Value::Null);
                let value = action_host
                    .dispatch(HostAction {
                        namespace: name.into(),
                        action: action.into(),
                        arguments,
                        scope: action_scope(name, action),
                        invocation_id: None,
                        owner: Some(action_owner.clone()),
                    })
                    .map_err(mlua::Error::runtime)?;
                lua.to_value(&value)
            })?,
        )?;
    }
    Ok(table)
}

fn input_registry(
    lua: &Lua,
    state: Arc<Mutex<BuildState>>,
    kind: &'static str,
) -> mlua::Result<Table> {
    let table = lua.create_table()?;
    table.set(
        "register",
        lua.create_function(
            move |lua, (id, function, options): (String, Function, Table)| {
                #[derive(Deserialize)]
                #[serde(default)]
                struct Options {
                    keys: Vec<String>,
                    mode: String,
                    when: Option<String>,
                    priority: i32,
                    fallback: bool,
                }
                impl Default for Options {
                    fn default() -> Self {
                        Self {
                            keys: Vec::new(),
                            mode: "normal".into(),
                            when: None,
                            priority: 0,
                            fallback: true,
                        }
                    }
                }
                let options: Options = lua.from_value(LuaValue::Table(options))?;
                if options.keys.is_empty()
                    || options.keys.iter().any(|key| key.trim().is_empty())
                {
                    return Err(mlua::Error::runtime(
                        "input contribution requires at least one non-empty key sequence",
                    ));
                }
                let mut state = state.lock().map_err(lock_error)?;
                let callback = state.callback(lua, function)?;
                let command_id = format!("lua.{kind}.{id}");
                state.snapshot.commands.push(CommandContribution {
                    id: command_id.clone(),
                    callback,
                    title: id,
                    description: String::new(),
                    scope: ExecutionScope::Local,
                    arguments_schema: Value::Object(Default::default()),
                    completions: Vec::new(),
                    accepts_range: false,
                    accepts_count: true,
                    accepts_bang: false,
                    aliases: Vec::new(),
                    completion_callback: None,
                    result_schema: Value::Object(Default::default()),
                });
                for key in options.keys {
                    state.next_contribution_order += 1;
                    let order = state.next_contribution_order;
                    state.snapshot.keymaps.push(KeymapContribution {
                        mode: options.mode.clone(),
                        key,
                        command: command_id.clone(),
                        when: options.when.clone(),
                        priority: options.priority,
                        order,
                        fallback: options.fallback,
                    });
                }
                Ok(command_id)
            },
        )?,
    )?;
    Ok(table)
}

fn action_scope(namespace: &str, action: &str) -> ExecutionScope {
    match namespace {
        "buffer" if matches!(action, "edit" | "save") => ExecutionScope::SharedBuffer,
        "document" if matches!(action, "edit" | "save" | "reload") => {
            ExecutionScope::SharedBuffer
        }
        "workspace" | "tab" if !matches!(action, "focus") => ExecutionScope::Workspace,
        "file_tree" | "notes"
            if matches!(
                action,
                "create" | "create_dir" | "rename" | "move" | "delete"
            ) =>
        {
            ExecutionScope::Workspace
        }
        _ => ExecutionScope::Local,
    }
}

fn parse_scope(scope: Option<&str>) -> ExecutionScope {
    match scope {
        Some("workspace") => ExecutionScope::Workspace,
        Some("shared-buffer") | Some("shared_buffer") => ExecutionScope::SharedBuffer,
        Some("presence") => ExecutionScope::Presence,
        _ => ExecutionScope::Local,
    }
}

fn lock_error<T>(_: std::sync::PoisonError<T>) -> mlua::Error {
    mlua::Error::runtime("Lua plugin state lock poisoned")
}

fn normalize_config_keys(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let previous = std::mem::take(object);
            for (key, mut value) in previous {
                normalize_config_keys(&mut value);
                object.insert(key.replace('_', "-"), value);
            }
        }
        Value::Array(items) => items.iter_mut().for_each(normalize_config_keys),
        _ => {}
    }
}

/// Content-derived package revision used in every callback, queued action, and
/// asynchronous result owner. It does not reset when a manager candidate is
/// rebuilt, so stale frames from an earlier package checkout cannot collide
/// with a new generation that happens to have the same ordinal number.
pub fn plugin_content_revision(
    root: &Path,
    entrypoint: &str,
) -> Result<PluginRevision, LuaPluginError> {
    source_revision(&root.join(entrypoint), &root.join("lua"))
}

fn source_revision(
    entrypoint: &Path,
    package_root: &Path,
) -> Result<PluginRevision, LuaPluginError> {
    let mut files = Vec::new();
    if entrypoint.exists() {
        files.push(entrypoint.to_path_buf());
    }
    collect_lua_sources(package_root, &mut files)?;
    files.sort();

    let mut hash = 0xcbf29ce484222325_u64;
    for path in files {
        for byte in path.to_string_lossy().as_bytes() {
            hash = (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3);
        }
        let bytes = std::fs::read(&path).map_err(|source| LuaPluginError::Read {
            path: path.clone(),
            source,
        })?;
        for byte in bytes {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    Ok(PluginRevision(format!("{hash:016x}")))
}

fn collect_lua_sources(
    root: &Path,
    files: &mut Vec<PathBuf>,
) -> Result<(), LuaPluginError> {
    let entries = match std::fs::read_dir(root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(LuaPluginError::Read {
                path: root.to_path_buf(),
                source,
            })
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| LuaPluginError::Read {
            path: root.to_path_buf(),
            source,
        })?;
        let path = entry.path();
        let metadata =
            std::fs::symlink_metadata(&path).map_err(|source| LuaPluginError::Read {
                path: path.clone(),
                source,
            })?;
        if metadata.file_type().is_symlink() {
            continue;
        }
        if metadata.is_dir() {
            collect_lua_sources(&path, files)?;
        } else if path.extension().is_some_and(|extension| extension == "lua") {
            files.push(path);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_module_exposes_every_registered_namespace_and_method() {
        let lua = Lua::new();
        let owner = PluginOwner {
            plugin_id: "dev.neoism.surface-test".into(),
            revision: crate::PluginRevision("r1".into()),
        };
        let state = Arc::new(Mutex::new(BuildState {
            snapshot: PluginSnapshot::empty(),
            owner: owner.clone(),
            ..BuildState::default()
        }));
        let module =
            build_module(&lua, state, Arc::new(crate::InertHost), owner).unwrap();

        for &namespace in crate::contract::RUNTIME_HOST_NAMESPACES {
            let table = module
                .get::<Table>(namespace)
                .unwrap_or_else(|error| panic!("missing `{namespace}`: {error}"));
            assert!(
                table.get::<Function>("call").is_ok(),
                "`{namespace}.call` is missing"
            );
            for &method in crate::contract::RUNTIME_QUERY_METHODS {
                assert!(
                    table.get::<Function>(method).is_ok(),
                    "`{namespace}.{method}` is missing"
                );
            }
            for &method in crate::contract::RUNTIME_ACTION_METHODS {
                assert!(
                    table.get::<Function>(method).is_ok(),
                    "`{namespace}.{method}` is missing"
                );
            }
        }
    }

    #[test]
    fn loaded_module_callbacks_do_not_prevent_snapshot_finalization() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-lua-runtime-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("init.lua"),
            "neoism.ui.style('status', { foreground = '#ffffff' })",
        )
        .unwrap();

        let runtime = LuaRuntime::load(&dir, Arc::new(crate::InertHost)).unwrap();
        assert_eq!(
            runtime
                .snapshot()
                .styles
                .resolve("status")
                .foreground
                .as_deref(),
            Some("#ffffff")
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn user_lsp_requests_carry_a_content_revision_owner() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-lua-user-owner-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("lua")).unwrap();
        std::fs::write(dir.join("lua/revision.lua"), "return 1").unwrap();
        std::fs::write(
            dir.join("init.lua"),
            r#"
                local revision = require("revision")
                neoism.setup({ revision = revision })
                neoism.lsp.request("definition", { path = "src/main.rs" })
            "#,
        )
        .unwrap();

        let first_host = Arc::new(crate::QueuedHost::default());
        let first = LuaRuntime::load(&dir, first_host.clone()).unwrap();
        let first_action = first_host.drain_actions().pop().unwrap();
        assert_eq!(first_action.action, "request");
        assert_eq!(first_action.arguments["operation"], "definition");
        assert_eq!(first_action.owner.as_ref(), Some(first.owner()));
        assert_ne!(first.owner().revision.0, "0");

        std::fs::write(dir.join("lua/revision.lua"), "return 2").unwrap();
        let second_host = Arc::new(crate::QueuedHost::default());
        let second = LuaRuntime::load(&dir, second_host.clone()).unwrap();
        assert_ne!(first.owner().revision, second.owner().revision);
        assert_eq!(
            second_host.drain_actions()[0].owner.as_ref(),
            Some(second.owner())
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn document_compatibility_api_queues_a_typed_revision_scoped_edit() {
        let dir = std::env::temp_dir().join(format!(
            "neoism-lua-document-contract-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("init.lua"),
            r#"
                neoism.document.edit({
                    document = "neoism:document:fixture",
                    expectedRevision = 4,
                    edits = {{
                        range = {
                            handle = "",
                            start = { line = 0, character = 0 },
                            ["end"] = { line = 0, character = 0 },
                        },
                        text = "x",
                    }},
                })
            "#,
        )
        .unwrap();
        let host = Arc::new(crate::QueuedHost::default());
        let runtime = LuaRuntime::load(&dir, host.clone()).unwrap();
        let action = host.drain_actions().pop().unwrap();
        assert_eq!(
            crate::action_contract(&action).unwrap().operation,
            crate::HostOperation::DocumentEdit
        );
        assert_eq!(action.owner.as_ref(), Some(runtime.owner()));
        assert_eq!(action.scope, ExecutionScope::SharedBuffer);
        let _ = std::fs::remove_dir_all(dir);
    }

    fn plugin_manifest(id: &str) -> PluginManifest {
        PluginManifest {
            id: id.into(),
            ..PluginManifest::default()
        }
    }

    #[test]
    fn candidate_activates_canonical_nested_editor_entrypoint() {
        let root = std::env::temp_dir().join(format!(
            "neoism-lua-canonical-editor-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("legacy.lua"), "this is not valid Lua").unwrap();
        std::fs::write(
            root.join("editor.lua"),
            "neoism.setup({ source = 'nested-editor' })",
        )
        .unwrap();
        let manifest: PluginManifest = serde_json::from_str(r#"{
            "id":"dev.neoism.canonical-editor","name":"Canonical editor","version":"1",
            "entrypoint":"legacy.lua","editor":{"entrypoint":"editor.lua"},
            "agent":{"runtime":"lua","entrypoint":"agent.lua","command":[],"capabilities":[],"scope":"workspace","eventNamespaces":[]}
        }"#).unwrap();

        let candidate = PluginRuntime::build_candidate(
            &root,
            &manifest,
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        assert_eq!(candidate.snapshot().config_patch["source"], "nested-editor");
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn plugin_packages_cannot_escape_their_root() {
        let base = std::env::temp_dir().join(format!(
            "neoism-lua-isolation-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let root = base.join("plugin");
        std::fs::create_dir_all(root.join("lua")).unwrap();
        std::fs::write(base.join("outside.lua"), "return 42").unwrap();
        std::fs::write(root.join("inside.lua"), "return { value = 99 }").unwrap();
        std::fs::write(root.join("lua/inside.lua"), "return { value = 7 }").unwrap();
        std::fs::write(
            root.join("init.lua"),
            "local x = require('inside'); neoism.setup({ value = x.value })",
        )
        .unwrap();
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.isolation"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        assert_eq!(candidate.snapshot().config_patch["value"], 7);

        std::fs::write(root.join("init.lua"), "require('../outside')").unwrap();
        assert!(PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.isolation"),
            PluginRevision("r2".into()),
            Arc::new(crate::InertHost)
        )
        .is_err());
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn callback_ids_include_plugin_and_revision_owner() {
        let root =
            std::env::temp_dir().join(format!("neoism-lua-owner-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            "neoism.command.register('test', function() end)",
        )
        .unwrap();
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.owner"),
            PluginRevision("abc123".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        assert_eq!(candidate.owner().plugin_id, "dev.neoism.owner");
        assert!(candidate.snapshot().commands[0]
            .callback
            .starts_with("lua:dev.neoism.owner@abc123:"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn candidate_rejects_unregistered_autocmd_events() {
        let root =
            std::env::temp_dir().join(format!("neoism-lua-event-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            "neoism.autocmd('RenderFrame', function() end)",
        )
        .unwrap();
        let result = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.invalid-event"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        );
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn augroups_clear_and_callbacks_use_priority_order_and_once() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-augroup-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            local group = neoism.augroup('ordered', { clear = true })
            neoism.autocmd('Startup', function() return 'low' end, { group = group, priority = 1 })
            neoism.autocmd('Startup', function() return 'high' end, { group = group, priority = 20, once = true })
            local cleared = neoism.augroup('cleared')
            neoism.autocmd('BufferChanged', function() return 'stale' end, { group = cleared })
            neoism.augroup('cleared', { clear = true })
            neoism.command.register('typed', function() end, {
                arguments_schema = { type = 'object', required = { 'value' } },
                result_schema = { type = 'object' }, completions = { 'one', 'two' },
                aliases = { 't' }, complete = function(event) return { event.payload.prefix .. '-dynamic' } end,
                accepts_range = true, accepts_count = true, accepts_bang = true,
            })
        "#).unwrap();
        let mut candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.augroup"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        let event = PluginEvent::new(
            crate::PluginEventKind::Startup,
            Value::Null,
            ExecutionScope::Local,
            None,
        )
        .unwrap();
        assert_eq!(
            candidate.runtime.emit(event.clone()).unwrap(),
            vec![Value::String("high".into()), Value::String("low".into())]
        );
        assert_eq!(
            candidate.runtime.emit(event).unwrap(),
            vec![Value::String("low".into())]
        );
        let command = &candidate.runtime.snapshot().commands[0];
        assert_eq!(candidate.runtime.snapshot().autocmds.len(), 2);
        assert_eq!(command.completions, vec!["one", "two"]);
        assert_eq!(command.aliases, vec!["t"]);
        assert!(command.accepts_range && command.accepts_count && command.accepts_bang);
        assert_eq!(
            candidate.runtime.complete_command("t", "o").unwrap(),
            vec!["o-dynamic", "one"]
        );
        assert!(crate::validate_command_arguments(
            &command.arguments_schema,
            &serde_json::json!({})
        )
        .is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn callback_event_storm_budget_rejects_excess_callbacks() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-budget-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            r#"
            neoism.autocmd('Startup', function() return 1 end)
            neoism.autocmd('Startup', function() return 2 end)
        "#,
        )
        .unwrap();
        let mut budgets = PluginBudgets::default();
        budgets.max_callbacks_per_event = 1;
        let mut candidate = PluginRuntime::build_candidate_with_budgets(
            &root,
            &plugin_manifest("dev.neoism.budget"),
            PluginRevision("r1".into()),
            budgets,
            Arc::new(crate::InertHost),
        )
        .unwrap();
        let event = PluginEvent::new(
            crate::PluginEventKind::Startup,
            Value::Null,
            ExecutionScope::Local,
            None,
        )
        .unwrap();
        assert!(candidate.runtime.emit(event).is_err());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn nested_event_dispatch_runs_only_nested_non_reentrant_autocmds() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-nested-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            r#"
            neoism.autocmd('Startup', function() return 'plain' end)
            neoism.autocmd('Startup', function() return 'nested' end, { nested = true })
        "#,
        )
        .unwrap();
        let mut candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.nested"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        let event = PluginEvent::new(
            crate::PluginEventKind::Startup,
            Value::Null,
            ExecutionScope::Local,
            None,
        )
        .unwrap();

        candidate.runtime.inner.enter_event().unwrap();
        let nested_callback = candidate.runtime.snapshot().autocmds[1].callback.clone();
        candidate
            .runtime
            .inner
            .active_autocmds
            .lock()
            .unwrap()
            .insert(nested_callback.clone());
        assert!(candidate.runtime.emit(event.clone()).unwrap().is_empty());
        candidate
            .runtime
            .inner
            .active_autocmds
            .lock()
            .unwrap()
            .remove(&nested_callback);
        assert_eq!(
            candidate.runtime.emit(event).unwrap(),
            vec![Value::String("nested".into())]
        );
        candidate.runtime.inner.leave_event();

        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn scheduler_actions_are_exact_owner_and_cancellable_handles() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-scheduler-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            local timer = neoism.scheduler.after({ delayMillis = 5, command = 'tick', arguments = { value = 1 } })
            neoism.scheduler.cancel({ timer = timer.id })
        "#).unwrap();
        let host = Arc::new(crate::QueuedHost::default());
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.scheduler"),
            PluginRevision("r1".into()),
            host.clone(),
        )
        .unwrap();
        let actions = host.drain_actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].owner.as_ref(), Some(candidate.owner()));
        assert_eq!(actions[0].namespace, "scheduler");
        assert_eq!(actions[0].action, "after");
        assert_eq!(
            actions[1].arguments["timer"],
            actions[0].invocation_id.as_deref().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn e4_host_services_queue_typed_exact_owner_requests() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-e4-services-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            local clipboard = neoism.clipboard.read({})
            neoism.async.cancel({ id = clipboard.id })
            local job = neoism.job.spawn({ program = 'printf', arguments = { 'ok' }, cwd = '.', timeoutMillis = 1000, maxOutputBytes = 64 })
            neoism.job.stdin({ job = job.id, data = 'x' })
            neoism.job.close_stdin({ job = job.id })
            neoism.prompt.confirm({ title = 'Continue?', message = 'Confirm' })
            neoism.notification.show({ title = 'Plugin', message = 'ready', level = 'info' })
            local list = neoism.result_list.create({ title = 'Results', kind = 'quickfix', entries = {} })
            neoism.result_list.query({ list = list.id })
        "#).unwrap();
        let host = Arc::new(crate::QueuedHost::default());
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.e4"),
            PluginRevision("r1".into()),
            host.clone(),
        )
        .unwrap();
        let actions = host.drain_actions();
        assert_eq!(actions.len(), 9);
        assert!(actions
            .iter()
            .all(|action| action.owner.as_ref() == Some(candidate.owner())));
        assert_eq!(actions[0].namespace, "clipboard");
        assert_eq!(actions[0].action, "read");
        assert_eq!(
            actions[1].arguments["id"],
            actions[0].invocation_id.as_deref().unwrap()
        );
        assert_eq!(
            actions[3].arguments["job"],
            actions[2].invocation_id.as_deref().unwrap()
        );
        assert_eq!(
            actions[8].arguments["list"],
            actions[7].invocation_id.as_deref().unwrap()
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn typed_command_invocation_completion_and_cancellation_are_owner_scoped() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-command-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            neoism.command.register('format.selection', function(event)
                return { accepted = event.payload.count }
            end, {
                aliases = { 'fmt' }, accepts_range = true, accepts_count = true, accepts_bang = true,
                arguments_schema = { type = 'object', required = { 'style' } },
                result_schema = { type = 'object', required = { 'accepted' } },
                completions = { 'compact', 'expanded' },
                complete = function(event) return { event.payload.prefix .. '-dynamic' } end,
            })
            local request = neoism.command.invoke({
                command = 'fmt', arguments = { style = 'compact' },
                range = { startLine = 2, endLine = 4 }, count = 3, bang = true,
            })
            neoism.command.cancel({ id = request.id })
        "#).unwrap();
        let host = Arc::new(crate::QueuedHost::default());
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.command"),
            PluginRevision("r1".into()),
            host.clone(),
        )
        .unwrap();
        assert_eq!(
            candidate.runtime.complete_command("fmt", "c").unwrap(),
            vec!["c-dynamic", "compact"]
        );
        let actions = host.drain_actions();
        assert_eq!(actions.len(), 2);
        assert_eq!(actions[0].owner.as_ref(), Some(candidate.owner()));
        assert_eq!(actions[0].arguments["command"], "fmt");
        assert_eq!(actions[0].arguments["range"]["startLine"], 2);
        assert_eq!(
            actions[1].arguments["id"],
            actions[0].invocation_id.as_deref().unwrap()
        );
        let command = &candidate.snapshot().commands[0];
        let request: crate::PluginCommandRequest =
            serde_json::from_value(actions[0].arguments.clone()).unwrap();
        crate::validate_command_request(command, &request).unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn invalid_candidate_cannot_commit_persistent_state() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-state-candidate-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            neoism.state.set({ scope = 'plugin', persistent = true, key = 'candidate', value = 1 })
            error('reject this generation')
        "#).unwrap();
        let host = Arc::new(crate::QueuedHost::default());
        assert!(PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.transaction"),
            PluginRevision("bad".into()),
            host.clone(),
        )
        .is_err());
        assert_eq!(
            host.persistent_state_snapshot()["entries"],
            serde_json::json!([])
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn editor_options_and_input_contributions_are_typed_and_ordered() {
        let root =
            std::env::temp_dir().join(format!("neoism-lua-input-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("init.lua"), r#"
            neoism.option.set({ name = 'wrap', value = false, scope = 'workspace', priority = 2 })
            neoism.option.set({ name = 'tab_width', value = 2, scope = 'document', target = 'opaque-doc' })
            neoism.motion.register('next_heading', function() return nil end, {
                keys = { 'g h', 'ctrl+]' }, mode = 'normal', priority = 9, fallback = true,
            })
            neoism.operator.register('surround', function() return nil end, {
                keys = { 'g s' }, mode = 'normal', fallback = false,
            })
        "#).unwrap();
        let candidate = PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.input"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        assert_eq!(candidate.snapshot().editor_options.len(), 2);
        assert_eq!(
            candidate.snapshot().editor_options[0].owner,
            *candidate.owner()
        );
        assert_eq!(
            candidate.snapshot().editor_options[0].name,
            crate::EditorOptionName::Wrap
        );
        assert_eq!(candidate.snapshot().keymaps.len(), 3);
        assert_eq!(candidate.snapshot().keymaps[0].key, "g h");
        assert_eq!(candidate.snapshot().keymaps[0].priority, 9);
        assert!(!candidate.snapshot().keymaps[2].fallback);
        assert!(candidate
            .snapshot()
            .keymaps
            .windows(2)
            .all(|pair| pair[0].order < pair[1].order));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn default_sandbox_excludes_process_filesystem_and_native_module_escape_hatches() {
        let root = std::env::temp_dir()
            .join(format!("neoism-lua-sandbox-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            r#"
            assert(io == nil)
            assert(os == nil)
            assert(debug == nil)
            assert(dofile == nil)
            assert(loadfile == nil)
            assert(package.loadlib == nil)
            assert(package.cpath == '')
        "#,
        )
        .unwrap();
        PluginRuntime::build_candidate(
            &root,
            &plugin_manifest("dev.neoism.sandbox"),
            PluginRevision("r1".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn surface_api_builds_a_relocatable_action_rail_snapshot() {
        let root = std::env::temp_dir().join(format!(
            "neoism-lua-surface-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            r#"
                local neoism = require("neoism")
                neoism.ui.surface("chrome.actions", { dock = "left", thickness = 44 })
                neoism.ui.item("chrome.notes", { visible = false })
                neoism.ui.item("chrome.search", { order = 10, align = "start" })
            "#,
        )
        .unwrap();
        let manifest = plugin_manifest("dev.neoism.surface-api");
        let candidate = PluginRuntime::build_candidate(
            &root,
            &manifest,
            PluginRevision("one".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        let snapshot = candidate.snapshot();
        let surface = &snapshot.surface_layout.surfaces["chrome.actions"];
        assert_eq!(surface.dock, Some(crate::DockEdge::Left));
        assert_eq!(surface.thickness, Some(44.0));
        assert_eq!(
            snapshot.surface_layout.items["chrome.notes"].visible,
            Some(false)
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn panel_register_uses_the_positional_id() {
        let root = std::env::temp_dir().join(format!(
            "neoism-lua-panel-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(
            root.join("init.lua"),
            r#"
                local neoism = require("neoism")
                neoism.panel.register("mission-control", {
                    title = "Mission Control",
                    location = "center",
                    content = {},
                })
            "#,
        )
        .unwrap();
        let manifest = plugin_manifest("dev.neoism.panel-api");
        let candidate = PluginRuntime::build_candidate(
            &root,
            &manifest,
            PluginRevision("one".into()),
            Arc::new(crate::InertHost),
        )
        .unwrap();
        assert_eq!(candidate.snapshot().panels[0].id, "mission-control");
        std::fs::remove_dir_all(root).unwrap();
    }
}
