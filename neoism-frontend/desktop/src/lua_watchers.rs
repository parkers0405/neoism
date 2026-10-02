use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_lua::{PluginOwner, PluginWatchRequest};
use neoism_window::window::WindowId;
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};

use crate::lua_async::LuaAsyncSender;

struct ActiveWatcher {
    owner: PluginOwner,
    _watcher: RecommendedWatcher,
}

#[derive(Default)]
pub(crate) struct LuaWatchers {
    active: HashMap<String, ActiveWatcher>,
}

impl LuaWatchers {
    pub(crate) fn watch(
        &mut self,
        owner: PluginOwner,
        id: String,
        window_id: WindowId,
        workspace_root: &Path,
        request: PluginWatchRequest,
        sender: LuaAsyncSender,
        event_proxy: EventProxy,
    ) -> Result<(), String> {
        if self.active.len() >= 256 || self.active.values().filter(|watcher| watcher.owner == owner).count() >= 32 {
            return Err("filesystem watcher limit reached".into());
        }
        let path = resolve_path(workspace_root, &request.path)?;
        let callback_owner = owner.clone();
        let callback_id = id.clone();
        let root = workspace_root.canonicalize().map_err(|error| error.to_string())?;
        let mut watcher = RecommendedWatcher::new(move |event: notify::Result<notify::Event>| {
            match event {
                Ok(event) => {
                    let resources = event.paths.iter().filter_map(|path| path.strip_prefix(&root).ok())
                        .map(|relative| neoism_lua::opaque_resource_handle("watch", &[&relative.to_string_lossy()]))
                        .collect::<Vec<_>>();
                    sender.progress(callback_owner.clone(), callback_id.clone(), serde_json::json!({
                        "event": "change", "kind": format!("{:?}", event.kind), "resources": resources,
                    }));
                }
                Err(error) => sender.progress(callback_owner.clone(), callback_id.clone(), serde_json::json!({
                    "event": "error", "message": error.to_string(),
                })),
            }
            event_proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
        }, Config::default()).map_err(|error| error.to_string())?;
        watcher.watch(&path, if request.recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive })
            .map_err(|error| error.to_string())?;
        self.active.insert(id, ActiveWatcher { owner, _watcher: watcher });
        Ok(())
    }

    pub(crate) fn cancel(&mut self, owner: &PluginOwner, id: &str) -> bool {
        if self.active.get(id).is_some_and(|watcher| &watcher.owner == owner) {
            self.active.remove(id);
            true
        } else { false }
    }

    pub(crate) fn retire_inactive(&mut self, active: &HashSet<PluginOwner>) {
        self.active.retain(|_, watcher| active.contains(&watcher.owner));
    }
}

fn resolve_path(root: &Path, requested: &str) -> Result<PathBuf, String> {
    if requested.len() > 16 * 1024 || Path::new(requested).is_absolute() {
        return Err("watch path must be a bounded workspace-relative path".into());
    }
    let root = root.canonicalize().map_err(|error| error.to_string())?;
    let path = root.join(requested).canonicalize().map_err(|error| error.to_string())?;
    if !path.starts_with(&root) { return Err("watch path escapes the workspace".into()); }
    Ok(path)
}