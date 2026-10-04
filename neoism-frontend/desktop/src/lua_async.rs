use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};

use neoism_lua::PluginOwner;
use neoism_window::window::WindowId;
use serde_json::Value;

pub(crate) const MAX_PENDING_PER_OWNER: usize = 64;
pub(crate) const MAX_PENDING_GLOBAL: usize = 512;
const MAX_QUEUED_RESULTS: usize = 8_192;
const MAX_DELIVERIES_PER_DRAIN: usize = 256;

#[derive(Clone, Debug)]
pub(crate) struct LuaAsyncToken {
    cancelled: Arc<AtomicBool>,
}

impl LuaAsyncToken {
    pub(crate) fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    pub(crate) fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LuaAsyncSender(mpsc::Sender<WorkerCompletion>);

impl LuaAsyncSender {
    pub(crate) fn progress(&self, owner: PluginOwner, id: String, value: Value) {
        let _ = self.0.send(WorkerCompletion {
            owner,
            id,
            terminal: Terminal::Progress(value),
        });
    }

    pub(crate) fn complete(&self, owner: PluginOwner, id: String, result: Value) {
        let _ = self.0.send(WorkerCompletion {
            owner,
            id,
            terminal: Terminal::Success(result),
        });
    }

    pub(crate) fn fail(
        &self,
        owner: PluginOwner,
        id: String,
        code: impl Into<String>,
        message: impl Into<String>,
    ) {
        let _ = self.0.send(WorkerCompletion {
            owner,
            id,
            terminal: Terminal::Failure {
                code: code.into(),
                message: message.into(),
            },
        });
    }
}

#[derive(Clone, Debug)]
enum Terminal {
    Progress(Value),
    Success(Value),
    Failure { code: String, message: String },
    Cancelled,
}

#[derive(Clone, Debug)]
struct WorkerCompletion {
    owner: PluginOwner,
    id: String,
    terminal: Terminal,
}

#[derive(Clone, Debug)]
struct Pending {
    window_id: WindowId,
    kind: String,
    cancelled: Arc<AtomicBool>,
}

#[derive(Clone, Debug)]
pub(crate) struct LuaAsyncDelivery {
    pub owner: PluginOwner,
    pub window_id: WindowId,
    pub payload: Value,
}

pub(crate) struct LuaAsyncCoordinator {
    pending: HashMap<(PluginOwner, String), Pending>,
    terminal: VecDeque<WorkerCompletion>,
    tx: mpsc::Sender<WorkerCompletion>,
    rx: mpsc::Receiver<WorkerCompletion>,
}

impl Default for LuaAsyncCoordinator {
    fn default() -> Self {
        let (tx, rx) = mpsc::channel();
        Self {
            pending: HashMap::new(),
            terminal: VecDeque::new(),
            tx,
            rx,
        }
    }
}

impl LuaAsyncCoordinator {
    fn enqueue_completion(&mut self, completion: WorkerCompletion) {
        if self.terminal.len() < MAX_QUEUED_RESULTS
            || !matches!(completion.terminal, Terminal::Progress(_))
        {
            self.terminal.push_back(completion);
        }
    }

    pub(crate) fn sender(&self) -> LuaAsyncSender {
        LuaAsyncSender(self.tx.clone())
    }

    pub(crate) fn register(
        &mut self,
        owner: PluginOwner,
        id: String,
        window_id: WindowId,
        kind: impl Into<String>,
    ) -> Result<LuaAsyncToken, &'static str> {
        if id.trim().is_empty() {
            return Err("request id is empty");
        }
        if self.pending.len() >= MAX_PENDING_GLOBAL {
            return Err("global async request limit reached");
        }
        if self
            .pending
            .keys()
            .filter(|(candidate, _)| candidate == &owner)
            .count()
            >= MAX_PENDING_PER_OWNER
        {
            return Err("plugin async request limit reached");
        }
        let key = (owner, id);
        if self.pending.contains_key(&key) {
            return Err("duplicate async request id");
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        self.pending.insert(
            key,
            Pending {
                window_id,
                kind: kind.into(),
                cancelled: cancelled.clone(),
            },
        );
        Ok(LuaAsyncToken { cancelled })
    }

    pub(crate) fn cancel(&mut self, owner: &PluginOwner, id: &str) -> bool {
        let key = (owner.clone(), id.to_owned());
        let Some(pending) = self.pending.get(&key) else {
            return false;
        };
        pending.cancelled.store(true, Ordering::Release);
        self.enqueue_completion(WorkerCompletion {
            owner: owner.clone(),
            id: id.into(),
            terminal: Terminal::Cancelled,
        });
        true
    }

    pub(crate) fn retire_inactive(
        &mut self,
        active: &std::collections::HashSet<PluginOwner>,
    ) {
        self.pending.retain(|(owner, _), pending| {
            let keep = active.contains(owner);
            if !keep {
                pending.cancelled.store(true, Ordering::Release);
            }
            keep
        });
        self.terminal
            .retain(|completion| active.contains(&completion.owner));
        while let Ok(completion) = self.rx.try_recv() {
            if active.contains(&completion.owner) {
                self.enqueue_completion(completion);
            }
        }
    }

    pub(crate) fn drain(&mut self) -> Vec<LuaAsyncDelivery> {
        while let Ok(completion) = self.rx.try_recv() {
            self.enqueue_completion(completion);
        }
        let mut deliveries = Vec::new();
        while deliveries.len() < MAX_DELIVERIES_PER_DRAIN {
            let Some(completion) = self.terminal.pop_front() else {
                break;
            };
            let key = (completion.owner.clone(), completion.id.clone());
            let is_progress = matches!(completion.terminal, Terminal::Progress(_));
            let Some(pending) = (if is_progress {
                self.pending.get(&key).cloned()
            } else {
                self.pending.remove(&key)
            }) else {
                continue;
            };
            let (ok, cancelled, terminal, result, error) = match completion.terminal {
                Terminal::Progress(value) => (true, false, false, Some(value), None),
                Terminal::Success(value) => (true, false, true, Some(value), None),
                Terminal::Failure { code, message } => (
                    false,
                    false,
                    true,
                    None,
                    Some(serde_json::json!({ "code": code, "message": message })),
                ),
                Terminal::Cancelled => (false, true, true, None, None),
            };
            deliveries.push(LuaAsyncDelivery {
                owner: completion.owner,
                window_id: pending.window_id,
                payload: serde_json::json!({
                    "id": completion.id,
                    "kind": pending.kind,
                    "ok": ok,
                    "cancelled": cancelled,
                    "terminal": terminal,
                    "result": result,
                    "error": error,
                }),
            });
        }
        deliveries
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use neoism_lua::PluginRevision;

    fn owner(revision: &str) -> PluginOwner {
        PluginOwner {
            plugin_id: "dev.test".into(),
            revision: PluginRevision(revision.into()),
        }
    }

    #[test]
    fn cancellation_is_terminal_and_suppresses_late_worker_results() {
        let mut coordinator = LuaAsyncCoordinator::default();
        let owner = owner("one");
        let window = WindowId::from(1);
        let token = coordinator
            .register(owner.clone(), "r1".into(), window, "job")
            .unwrap();
        let sender = coordinator.sender();
        assert!(coordinator.cancel(&owner, "r1"));
        sender.complete(
            owner.clone(),
            "r1".into(),
            serde_json::json!({ "late": true }),
        );
        let deliveries = coordinator.drain();
        assert_eq!(deliveries.len(), 1);
        assert_eq!(deliveries[0].payload["cancelled"], true);
        assert!(token.is_cancelled());
        assert!(coordinator.drain().is_empty());
    }

    #[test]
    fn exact_revision_teardown_discards_queued_and_future_results() {
        let mut coordinator = LuaAsyncCoordinator::default();
        let old = owner("old");
        let new = owner("new");
        let window = WindowId::from(1);
        let token = coordinator
            .register(old.clone(), "r1".into(), window, "prompt")
            .unwrap();
        let sender = coordinator.sender();
        let active = [new].into_iter().collect();
        coordinator.retire_inactive(&active);
        sender.complete(old, "r1".into(), Value::Null);
        assert!(token.is_cancelled());
        assert!(coordinator.drain().is_empty());
    }

    #[test]
    fn duplicate_ids_and_per_owner_limit_are_enforced() {
        let mut coordinator = LuaAsyncCoordinator::default();
        let owner = owner("one");
        let window = WindowId::from(1);
        coordinator
            .register(owner.clone(), "same".into(), window, "job")
            .unwrap();
        assert!(coordinator
            .register(owner.clone(), "same".into(), window, "job")
            .is_err());
        for index in 1..MAX_PENDING_PER_OWNER {
            coordinator
                .register(owner.clone(), format!("r{index}"), window, "job")
                .unwrap();
        }
        assert!(coordinator
            .register(owner, "overflow".into(), window, "job")
            .is_err());
    }
}
