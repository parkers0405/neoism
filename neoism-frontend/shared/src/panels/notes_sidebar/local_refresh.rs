//! Native-only worker mailbox. The UI owns scheduling; workers own only snapshots.
use super::{list_local_notes, LocalNotesSnapshot};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

type Completion = (u64, Option<LocalNotesSnapshot>);

#[derive(Debug, Default)]
pub(super) struct LocalRefresh {
    pub enabled: bool,
    generation: u64,
    dirty: bool,
    inflight: Option<Arc<Mutex<Option<Completion>>>>,
}

// Workspace panel snapshots must not consume each other's worker results.
impl Clone for LocalRefresh {
    fn clone(&self) -> Self {
        Self {
            enabled: self.enabled,
            generation: self.generation,
            dirty: self.dirty || self.inflight.is_some(),
            inflight: None,
        }
    }
}

impl LocalRefresh {
    pub fn request(&mut self) {
        self.dirty = true;
    }

    pub fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.dirty = true;
    }

    pub fn take_completed(&mut self) -> Option<LocalNotesSnapshot> {
        let completed = self.inflight.as_ref()?.lock().unwrap().take();
        let (generation, snapshot) = completed?;
        self.inflight = None;
        if snapshot.is_none() {
            self.dirty = true;
        }
        (generation == self.generation).then_some(snapshot).flatten()
    }

    pub fn needs_start(&self) -> bool {
        self.dirty && self.inflight.is_none()
    }

    pub fn start(
        &mut self,
        root: PathBuf,
        icon_revisions: HashMap<PathBuf, u64>,
        wake: impl FnOnce() + Send + 'static,
    ) {
        if !self.dirty || self.inflight.is_some() {
            return;
        }
        let generation = self.generation;
        let mailbox = Arc::new(Mutex::new(None));
        let output = mailbox.clone();
        // A failed spawn retains the request. A panicked scan still publishes a
        // completion, so it cannot permanently wedge the single-flight gate.
        let spawned = std::thread::Builder::new()
            .name("notes-listing".into())
            .spawn(move || {
                let snapshot = std::panic::catch_unwind(|| {
                    let mut snapshot = list_local_notes(&root);
                    snapshot.icon_revisions = icon_revisions;
                    snapshot
                })
                .ok();
                *output.lock().unwrap() = Some((generation, snapshot));
                // Never call an EventProxy (or arbitrary host callback) under a lock.
                wake();
            });
        match spawned {
            Ok(_) => {
                self.dirty = false;
                self.inflight = Some(mailbox);
            }
            Err(error) => tracing::warn!(%error, "could not spawn notes listing worker"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn complete(worker: &mut LocalRefresh, generation: u64) {
        let output = worker.inflight.as_ref().unwrap().clone();
        *output.lock().unwrap() = Some((generation, Some(LocalNotesSnapshot::default())));
    }

    #[test]
    fn burst_is_one_inflight_and_one_followup() {
        let mut worker = LocalRefresh::default();
        // Install a controlled in-flight job; no timing-dependent sleeps.
        worker.inflight = Some(Arc::new(Mutex::new(None)));
        let first = worker.inflight.as_ref().unwrap().clone();
        for _ in 0..100 {
            worker.request();
            worker.start(PathBuf::new(), Default::default(), || panic!("must not launch"));
        }
        assert!(Arc::ptr_eq(&first, worker.inflight.as_ref().unwrap()));
        complete(&mut worker, 0);
        assert!(worker.take_completed().is_some());
        assert!(worker.dirty);
        // Starting consumes exactly the coalesced request.
        worker.start(PathBuf::new(), Default::default(), || {});
        assert!(!worker.dirty);
        assert!(worker.inflight.is_some());
        worker.start(PathBuf::new(), Default::default(), || panic!("no third job"));
    }

    #[test]
    fn switch_away_and_back_rejects_old_generation() {
        let mut worker = LocalRefresh::default();
        worker.inflight = Some(Arc::new(Mutex::new(None)));
        worker.invalidate();
        worker.invalidate();
        complete(&mut worker, 0);
        assert!(worker.take_completed().is_none());
        assert!(worker.inflight.is_none());
        assert!(worker.dirty);
    }

    #[test]
    fn wake_runs_after_publication_and_outside_lock() {
        let vault = tempfile::tempdir().unwrap();
        let (mailbox_tx, mailbox_rx) = std::sync::mpsc::channel::<
            Arc<Mutex<Option<Completion>>>,
        >();
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        let mut worker = LocalRefresh::default();
        worker.request();
        worker.start(vault.path().to_path_buf(), HashMap::new(), move || {
            let mailbox = mailbox_rx.recv().unwrap();
            let ready = mailbox.try_lock().unwrap().is_some();
            done_tx.send(ready).unwrap();
        });
        mailbox_tx.send(worker.inflight.as_ref().unwrap().clone()).unwrap();
        assert!(done_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap());
        assert!(worker.take_completed().is_some());
    }
}
