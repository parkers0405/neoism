//! Render-time Git metadata reads. Filesystem probes stay on a bounded worker.

use super::{GitChangeSummary, BRANCH_TTL};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use web_time::Instant;

const QUEUE_CAPACITY: usize = 16;
const CACHE_CAPACITY: usize = 256;
type Metadata = (Option<String>, Option<PathBuf>);

struct Job {
    path: PathBuf,
    wake: Box<dyn FnOnce() + Send>,
}
struct Entry {
    value: Metadata,
    fetched_at: Option<Instant>,
    in_flight: bool,
}
struct Worker {
    jobs: SyncSender<Job>,
    results: Receiver<(PathBuf, Metadata)>,
}

/// Owned by one window so completion wakes cannot target a different window.
#[derive(Default)]
pub struct GitStatusCache {
    entries: HashMap<PathBuf, Entry>,
    worker: Option<Worker>,
}

impl GitStatusCache {
    pub fn branch_for(&mut self, path: &Path, wake: impl FnOnce() + Send + 'static) -> Option<String> {
        self.refresh(path, wake);
        self.entries.get(path)?.value.0.clone()
    }

    pub fn change_summary_for(
        &mut self,
        path: &Path,
        wake: impl FnOnce() + Send + 'static,
    ) -> Option<GitChangeSummary> {
        self.refresh(path, wake);
        let root = self.entries.get(path)?.value.1.as_deref()?;
        // Root discovery already happened on the metadata worker. This cache
        // schedules numstat on its existing worker, never in paint.
        super::change_summary_for_repo_root(root)
    }

    fn refresh(&mut self, path: &Path, wake: impl FnOnce() + Send + 'static) {
        if let Some(worker) = &self.worker {
            for (path, value) in worker.results.try_iter().take(QUEUE_CAPACITY + 1) {
                self.entries.insert(path, Entry {
                    value,
                    fetched_at: Some(Instant::now()),
                    in_flight: false,
                });
            }
        }
        let now = Instant::now();
        if !self.entries.contains_key(path) {
            if self.entries.len() >= CACHE_CAPACITY {
                let victim = self.entries.iter()
                    .filter(|(_, entry)| !entry.in_flight)
                    .min_by_key(|(_, entry)| entry.fetched_at)
                    .map(|(path, _)| path.clone());
                if let Some(victim) = victim {
                    self.entries.remove(&victim);
                } else {
                    return;
                }
            }
            self.entries.insert(path.to_path_buf(), Entry {
                value: (None, None), fetched_at: None, in_flight: false,
            });
        }
        let entry = self.entries.get_mut(path).unwrap();
        let fresh = entry.fetched_at.is_some_and(|at| now.saturating_duration_since(at) < BRANCH_TTL);
        if entry.in_flight || fresh {
            return;
        }
        entry.in_flight = true;
        if self.worker.is_none() {
            self.worker = Worker::new();
        }
        let Some(worker) = &self.worker else {
            entry.in_flight = false;
            entry.fetched_at = Some(now);
            return;
        };
        let job = Job { path: path.to_path_buf(), wake: Box::new(wake) };
        if let Err(error) = worker.jobs.try_send(job) {
            // Bounded overload retains the last good value and backs off,
            // rather than doing I/O or retrying every animation frame.
            let entry = self.entries.get_mut(path).unwrap();
            entry.in_flight = false;
            entry.fetched_at = Some(now);
            if matches!(error, TrySendError::Disconnected(_)) {
                self.worker = None;
                for entry in self.entries.values_mut() {
                    entry.in_flight = false;
                }
            }
        }
    }
}

impl Worker {
    fn new() -> Option<Self> {
        let (jobs, requests) = mpsc::sync_channel::<Job>(QUEUE_CAPACITY);
        let (results_tx, results) = mpsc::channel();
        std::thread::Builder::new().name("git-status-metadata".into()).spawn(move || {
            while let Ok(job) = requests.recv() {
                let value = (super::branch_for(&job.path), super::repo_root_for(&job.path));
                if results_tx.send((job.path, value)).is_err() {
                    break;
                }
                (job.wake)();
            }
        }).ok()?;
        Some(Self { jobs, results })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(capacity: usize) -> (GitStatusCache, Receiver<Job>, mpsc::Sender<(PathBuf, Metadata)>) {
        let (jobs, requests) = mpsc::sync_channel(capacity);
        let (results_tx, results) = mpsc::channel();
        (GitStatusCache { entries: HashMap::new(), worker: Some(Worker { jobs, results }) },
            requests, results_tx)
    }

    #[test]
    fn animation_frames_coalesce_metadata_work_and_keep_the_last_good_branch() {
        let (mut cache, requests, replies) = fixture(2);
        let path = Path::new("/host-only/project/file.rs");
        assert_eq!(cache.branch_for(path, || {}), None);
        for _ in 0..1_000 {
            assert_eq!(cache.branch_for(path, || panic!("duplicate wake")), None);
        }
        let job = requests.try_recv().unwrap();
        assert_eq!(job.path, path);
        assert!(requests.try_recv().is_err());
        replies.send((job.path, (Some("main".into()), Some("/host-only/project".into())))).unwrap();
        for _ in 0..1_000 {
            assert_eq!(cache.branch_for(path, || panic!("warm frame queued I/O")), Some("main".into()));
        }
        assert!(requests.try_recv().is_err());
        cache.entries.get_mut(path).unwrap().fetched_at = None;
        assert_eq!(cache.branch_for(path, || {}), Some("main".into()));
        assert_eq!(requests.try_recv().unwrap().path, path);
    }

    #[test]
    fn bounded_overload_backs_off_and_old_root_replies_do_not_replace_current_root() {
        let (mut cache, requests, replies) = fixture(1);
        let old = Path::new("/old/file.rs");
        let current = Path::new("/new/file.rs");
        cache.branch_for(old, || {});
        cache.branch_for(current, || {});
        assert!(!cache.entries[current].in_flight);
        let old_job = requests.try_recv().unwrap();
        replies.send((old_job.path, (Some("old".into()), Some("/old".into())))).unwrap();
        assert_eq!(cache.branch_for(current, || {}), None);
        assert!(requests.try_recv().is_err());
        cache.entries.get_mut(current).unwrap().fetched_at = None;
        cache.branch_for(current, || {});
        assert_eq!(requests.try_recv().unwrap().path, current);
        assert_eq!(cache.branch_for(old, || {}), Some("old".into()));
    }
}
