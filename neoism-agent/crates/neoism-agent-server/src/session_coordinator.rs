use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::{Mutex, Notify};

use crate::state::SessionRun;

#[derive(Default)]
pub(crate) struct SessionCoordinator {
    entries: Mutex<HashMap<String, Entry>>,
}

struct Entry {
    run: Option<SessionRun>,
    worker: bool,
    pending_wake: bool,
    changed: Arc<Notify>,
}

impl Default for Entry {
    fn default() -> Self {
        Self {
            run: None,
            worker: false,
            pending_wake: false,
            changed: Arc::new(Notify::new()),
        }
    }
}

impl SessionCoordinator {
    /// Claim the one execution slot for a session. Different session keys do
    /// not contend, while a duplicate caller receives the active run to join
    /// or reject without ever replacing its cancellation handle.
    pub(crate) async fn try_start_run(
        &self,
        session_id: &str,
        run: SessionRun,
    ) -> Result<(), SessionRun> {
        let mut entries = self.entries.lock().await;
        let entry = entries.entry(session_id.to_string()).or_default();
        if let Some(active) = &entry.run {
            return Err(active.clone());
        }
        entry.run = Some(run);
        entry.changed.notify_waiters();
        Ok(())
    }

    pub(crate) async fn finish_run(&self, session_id: &str, run_id: &str) -> bool {
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(session_id) else {
            return false;
        };
        if !entry.run.as_ref().is_some_and(|run| run.id == run_id) {
            return false;
        }
        entry.run = None;
        entry.changed.notify_waiters();
        if !entry.worker {
            entries.remove(session_id);
        }
        true
    }

    pub(crate) async fn abort_run(&self, session_id: &str) -> Option<SessionRun> {
        let mut entries = self.entries.lock().await;
        let entry = entries.get_mut(session_id)?;
        let run = entry.run.take();
        entry.changed.notify_waiters();
        if !entry.worker {
            entries.remove(session_id);
        }
        run
    }

    /// Coalesce any number of wakes into the existing worker. The durable
    /// queue remains the source of work; this only controls ownership.
    pub(crate) async fn wake(&self, session_id: &str) -> bool {
        let mut entries = self.entries.lock().await;
        let entry = entries.entry(session_id.to_string()).or_default();
        if entry.worker {
            entry.pending_wake = true;
            entry.changed.notify_waiters();
            return false;
        }
        entry.worker = true;
        entry.pending_wake = false;
        entry.changed.notify_waiters();
        true
    }

    /// Called only after a worker observes an empty queue. A wake racing with
    /// that observation makes the same worker loop once more; otherwise its
    /// ownership is released so a later wake starts a new worker.
    pub(crate) async fn finish_worker_cycle(&self, session_id: &str) -> bool {
        let mut entries = self.entries.lock().await;
        let Some(entry) = entries.get_mut(session_id) else {
            return false;
        };
        if entry.pending_wake {
            entry.pending_wake = false;
            entry.changed.notify_waiters();
            return true;
        }
        entry.worker = false;
        entry.changed.notify_waiters();
        if entry.run.is_none() {
            entries.remove(session_id);
        }
        false
    }

    pub(crate) async fn worker_active(&self, session_id: &str) -> bool {
        self.entries
            .lock()
            .await
            .get(session_id)
            .is_some_and(|entry| entry.worker)
    }

    pub(crate) async fn wait_until_idle(&self, session_id: &str) {
        loop {
            let notified = {
                let entries = self.entries.lock().await;
                let Some(entry) = entries.get(session_id) else {
                    return;
                };
                if entry.run.is_none() {
                    return;
                }
                let mut notified = Box::pin(entry.changed.clone().notified_owned());
                notified.as_mut().enable();
                notified
            };
            notified.await;
        }
    }

    pub(crate) async fn wait_until_settled(&self, session_id: &str) {
        loop {
            let notified = {
                let entries = self.entries.lock().await;
                let Some(entry) = entries.get(session_id) else {
                    return;
                };
                if entry.run.is_none() && !entry.worker && !entry.pending_wake {
                    return;
                }
                let mut notified = Box::pin(entry.changed.clone().notified_owned());
                notified.as_mut().enable();
                notified
            };
            notified.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    fn run(id: &str) -> SessionRun {
        SessionRun {
            id: id.to_string(),
            started_at: 1,
            cancel: Arc::new(AtomicBool::new(false)),
        }
    }

    #[tokio::test]
    async fn keyed_runs_join_and_wakes_coalesce() {
        let coordinator = Arc::new(SessionCoordinator::default());
        coordinator
            .try_start_run("one", run("run-1"))
            .await
            .unwrap();
        let active = coordinator
            .try_start_run("one", run("run-2"))
            .await
            .expect_err("same key must return the active run");
        assert_eq!(active.id, "run-1");
        coordinator
            .try_start_run("two", run("run-3"))
            .await
            .unwrap();

        assert!(coordinator.wake("one").await);
        assert!(!coordinator.wake("one").await);
        assert!(coordinator.finish_worker_cycle("one").await);
        assert!(!coordinator.finish_worker_cycle("one").await);

        let waiter = {
            let coordinator = coordinator.clone();
            tokio::spawn(async move { coordinator.wait_until_idle("one").await })
        };
        assert!(coordinator.finish_run("one", "run-1").await);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("idle waiter should be notified")
            .unwrap();
        assert!(coordinator.finish_run("two", "run-3").await);
    }

    #[tokio::test]
    async fn settled_wait_includes_the_queue_worker_lifetime() {
        let coordinator = Arc::new(SessionCoordinator::default());
        assert!(coordinator.wake("one").await);
        let waiter = tokio::spawn({
            let coordinator = coordinator.clone();
            async move { coordinator.wait_until_settled("one").await }
        });
        tokio::task::yield_now().await;
        assert!(!waiter.is_finished());
        assert!(!coordinator.finish_worker_cycle("one").await);
        tokio::time::timeout(std::time::Duration::from_secs(1), waiter)
            .await
            .expect("settled waiter should finish with the worker")
            .unwrap();
    }
}
