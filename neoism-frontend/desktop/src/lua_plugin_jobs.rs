use std::collections::{BTreeMap, VecDeque};
use std::sync::mpsc;

use neoism_backend::event::{EventProxy, RioEvent, RioEventType};
use neoism_extensions::lua_plugins::{
    AcquisitionError, LuaPluginStore, PluginSpec, ProgressEvent, ValidatedPluginMetadata,
};
use neoism_window::window::WindowId;

#[derive(Clone, Debug)]
pub(crate) enum LuaPluginOperation {
    Install(PluginSpec),
    Update(PluginSpec),
    Restore,
    Remove,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum LuaPluginJobKind {
    Installing,
    Updating,
    Restoring,
    Removing,
    Failed,
}

#[derive(Clone, Debug)]
pub(crate) struct LuaPluginJobView {
    pub kind: LuaPluginJobKind,
    pub status_text: String,
    pub retryable: bool,
}

pub(crate) struct LuaPluginCompletion {
    pub plugin_id: String,
    pub success: bool,
    pub message: String,
    pub previous_lock_entry: Option<neoism_extensions::lua_plugins::LuaPluginLockEntry>,
    pub lock_changed: bool,
}

struct QueuedJob {
    window_id: WindowId,
    plugin_id: String,
    operation: LuaPluginOperation,
}

enum WorkerUpdate {
    Progress(ProgressEvent),
    Finished {
        result: Result<(), AcquisitionError>,
        previous_lock_entry: Option<neoism_extensions::lua_plugins::LuaPluginLockEntry>,
        lock_changed: bool,
    },
}

struct ActiveJob {
    plugin_id: String,
    operation: LuaPluginOperation,
    updates: mpsc::Receiver<WorkerUpdate>,
}

#[derive(Default)]
pub(crate) struct LuaPluginJobs {
    queued: VecDeque<QueuedJob>,
    active: Option<ActiveJob>,
    views: BTreeMap<String, LuaPluginJobView>,
    failed_operations: BTreeMap<String, LuaPluginOperation>,
    changed: bool,
}

impl LuaPluginJobs {
    pub fn enqueue(&mut self, window_id: WindowId, plugin_id: String, operation: LuaPluginOperation) {
        if self.active.as_ref().is_some_and(|job| job.plugin_id == plugin_id)
            || self.queued.iter().any(|job| job.plugin_id == plugin_id)
        {
            return;
        }
        let kind = operation_kind(&operation);
        self.views.insert(plugin_id.clone(), LuaPluginJobView {
            kind,
            status_text: initial_status(kind).into(),
            retryable: false,
        });
        self.changed = true;
        self.queued.push_back(QueuedJob { window_id, plugin_id, operation });
    }

    pub fn retry(&mut self, window_id: WindowId, plugin_id: &str) -> bool {
        let Some(operation) = self.failed_operations.remove(plugin_id) else { return false };
        self.enqueue(window_id, plugin_id.to_string(), operation);
        true
    }

    pub fn view(&self, plugin_id: &str) -> Option<&LuaPluginJobView> {
        self.views.get(plugin_id)
    }

    pub fn is_active(&self) -> bool {
        self.active.is_some() || !self.queued.is_empty()
    }

    pub fn take_changed(&mut self) -> bool {
        std::mem::take(&mut self.changed)
    }

    pub fn record_failure(&mut self, plugin_id: String, message: String) {
        self.views.insert(plugin_id, LuaPluginJobView {
            kind: LuaPluginJobKind::Failed,
            status_text: message,
            retryable: false,
        });
        self.changed = true;
    }

    pub fn pump(&mut self, event_proxy: &EventProxy) -> Vec<LuaPluginCompletion> {
        if self.active.is_none() {
            self.start_next(event_proxy.clone());
        }
        let mut completions = Vec::new();
        let mut finished = None;
        if let Some(active) = self.active.as_mut() {
            while let Ok(update) = active.updates.try_recv() {
                match update {
                    WorkerUpdate::Progress(progress) => {
                        if let Some(view) = self.views.get_mut(&active.plugin_id) {
                            view.status_text = progress_text(&progress).into();
                        }
                        self.changed = true;
                    }
                    WorkerUpdate::Finished { result, previous_lock_entry, lock_changed } => {
                        finished = Some((result, previous_lock_entry, lock_changed));
                        break;
                    }
                }
            }
        }
        if let Some((result, previous_lock_entry, lock_changed)) = finished {
            let active = self.active.take().expect("finished Lua plugin job must be active");
            match result {
                Ok(()) => {
                    self.views.remove(&active.plugin_id);
                    self.failed_operations.remove(&active.plugin_id);
                    completions.push(LuaPluginCompletion {
                        plugin_id: active.plugin_id,
                        success: true,
                        message: "Plugin lifecycle operation completed".into(),
                        previous_lock_entry,
                        lock_changed,
                    });
                }
                Err(error) => {
                    let message = error.to_string();
                    let retryable = error.retryable();
                    self.failed_operations.insert(active.plugin_id.clone(), active.operation);
                    self.views.insert(active.plugin_id.clone(), LuaPluginJobView {
                        kind: LuaPluginJobKind::Failed,
                        status_text: message.clone(),
                        retryable,
                    });
                    completions.push(LuaPluginCompletion {
                        plugin_id: active.plugin_id,
                        success: false,
                        message,
                        previous_lock_entry,
                        lock_changed,
                    });
                }
            }
            self.changed = true;
            self.start_next(event_proxy.clone());
        }
        completions
    }

    fn start_next(&mut self, event_proxy: EventProxy) {
        let Some(job) = self.queued.pop_front() else { return };
        let (tx, rx) = mpsc::channel();
        let window_id = job.window_id;
        let plugin_id = job.plugin_id.clone();
        let operation = job.operation.clone();
        std::thread::Builder::new()
            .name(format!("lua-plugin-{plugin_id}"))
            .spawn(move || {
                let store = LuaPluginStore::managed();
                let previous_lock_entry = store.load_lock().ok().and_then(|lock| lock.plugins.get(&plugin_id).cloned());
                let lock_changed = matches!(&operation, LuaPluginOperation::Install(_) | LuaPluginOperation::Update(_) | LuaPluginOperation::Remove);
                let wake = || event_proxy.send_event(RioEventType::Rio(RioEvent::Render), window_id);
                let progress_tx = tx.clone();
                let progress = |event| {
                    let _ = progress_tx.send(WorkerUpdate::Progress(event));
                    wake();
                };
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match operation {
                    LuaPluginOperation::Install(spec) => store.install(spec, progress, validate_checkout).map(|_| ()),
                    LuaPluginOperation::Update(spec) => store.update(spec, progress, validate_checkout).map(|_| ()),
                    LuaPluginOperation::Restore => store.restore_exact(&plugin_id, progress, validate_checkout).map(|_| ()),
                    LuaPluginOperation::Remove => store.remove(&plugin_id, progress).map(|_| ()),
                })).unwrap_or_else(|_| Err(AcquisitionError::Validation("plugin lifecycle worker panicked".into())));
                let _ = tx.send(WorkerUpdate::Finished { result, previous_lock_entry, lock_changed });
                wake();
            })
            .expect("spawn Lua plugin lifecycle worker");
        self.active = Some(ActiveJob { plugin_id: job.plugin_id, operation: job.operation, updates: rx });
    }
}

fn validate_checkout(context: &neoism_extensions::lua_plugins::ValidationContext<'_>) -> Result<ValidatedPluginMetadata, String> {
    let package = neoism_lua::load_plugin_manifest(context.checkout_path).map_err(|error| error.to_string())?;
    if package.manifest.id != context.spec.plugin_id {
        return Err(format!(
            "package manifest id `{}` does not match requested plugin `{}`",
            package.manifest.id, context.spec.plugin_id
        ));
    }
    let manifest_checksum = neoism_extensions::lua_plugins::file_sha256(&package.manifest_path)
        .map_err(|error| error.to_string())?;
    Ok(ValidatedPluginMetadata {
        plugin_version: package.manifest.version,
        manifest_checksum,
        dependencies: package.manifest.dependencies,
    })
}

fn operation_kind(operation: &LuaPluginOperation) -> LuaPluginJobKind {
    match operation {
        LuaPluginOperation::Install(_) => LuaPluginJobKind::Installing,
        LuaPluginOperation::Update(_) => LuaPluginJobKind::Updating,
        LuaPluginOperation::Restore => LuaPluginJobKind::Restoring,
        LuaPluginOperation::Remove => LuaPluginJobKind::Removing,
    }
}

fn initial_status(kind: LuaPluginJobKind) -> &'static str {
    match kind {
        LuaPluginJobKind::Installing => "Queued for installation",
        LuaPluginJobKind::Updating => "Queued for update",
        LuaPluginJobKind::Restoring => "Queued for restore",
        LuaPluginJobKind::Removing => "Queued for removal",
        LuaPluginJobKind::Failed => "Failed",
    }
}

fn progress_text(event: &ProgressEvent) -> &'static str {
    match event {
        ProgressEvent::Staging { .. } => "Preparing checkout...",
        ProgressEvent::Cloning => "Cloning repository...",
        ProgressEvent::Fetching => "Fetching revision...",
        ProgressEvent::Resolving => "Resolving revision...",
        ProgressEvent::CheckingOut { .. } => "Checking out commit...",
        ProgressEvent::Checksumming => "Verifying files...",
        ProgressEvent::Validating => "Validating plugin...",
        ProgressEvent::PublishingRevision { .. } => "Publishing revision...",
        ProgressEvent::PublishingLockfile => "Finalizing installation...",
        ProgressEvent::Removing { .. } => "Removing plugin...",
        ProgressEvent::Pruning { .. } => "Cleaning old revision...",
        ProgressEvent::Done => "Finalizing...",
    }
}