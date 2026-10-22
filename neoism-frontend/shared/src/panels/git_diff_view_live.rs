//! Single-flight live review work. Native diffs stay off the paint thread;
//! browsers advance the same builder in short, cooperative slices.
use super::*;
#[cfg(not(target_arch = "wasm32"))]
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct Key {
    pub path: PathBuf,
    pub base_revision: u64,
    pub revision: u64,
    pub caret: usize,
}

pub(super) struct Output {
    pub key: Key,
    pub lines: Vec<DiffLine>,
}

#[cfg(not(target_arch = "wasm32"))]
struct Request {
    key: Key,
    base: std::sync::Arc<str>,
    current: String,
}

#[cfg(not(target_arch = "wasm32"))]
struct Worker {
    requests: std::sync::mpsc::SyncSender<Request>,
    results: std::sync::mpsc::Receiver<Output>,
    cancelled: Arc<AtomicBool>,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Relaxed);
    }
}

#[derive(Default)]
pub(super) struct LiveDiff {
    pending: bool,
    #[cfg(not(target_arch = "wasm32"))]
    worker: Option<Worker>,
    #[cfg(target_arch = "wasm32")]
    job: Option<(Key, crate::editor::code::gitdiff::ReviewDiffBuilder)>,
}

impl LiveDiff {
    pub fn pending(&self) -> bool {
        self.pending
    }

    pub fn start(
        &mut self,
        key: Key,
        base: std::sync::Arc<str>,
        current: String,
    ) -> Result<(), String> {
        debug_assert!(!self.pending);
        #[cfg(not(target_arch = "wasm32"))]
        {
            if self.worker.is_none() {
                let (requests, input) = std::sync::mpsc::sync_channel::<Request>(1);
                let (output, results) = std::sync::mpsc::sync_channel(1);
                let cancelled = Arc::new(AtomicBool::new(false));
                let worker_cancelled = cancelled.clone();
                std::thread::Builder::new()
                    .name("git-live-review".into())
                    .spawn(move || {
                        while let Ok(request) = input.recv() {
                            let mut builder =
                                crate::editor::code::gitdiff::ReviewDiffBuilder::new(
                                    request.base.to_string(),
                                    request.current,
                                    3,
                                    Some(request.key.caret),
                                );
                            let lines = loop {
                                if let Some(lines) = builder.poll(16_384) {
                                    break lines;
                                }
                                if worker_cancelled.load(Ordering::Relaxed) {
                                    return;
                                }
                            };
                            if output
                                .send(Output {
                                    key: request.key,
                                    lines,
                                })
                                .is_err()
                            {
                                break;
                            }
                        }
                    })
                    .map_err(|error| format!("Could not start live review: {error}"))?;
                self.worker = Some(Worker {
                    requests,
                    results,
                    cancelled,
                });
            }
            self.worker
                .as_ref()
                .unwrap()
                .requests
                .try_send(Request { key, base, current })
                .map_err(|error| format!("Could not schedule live review: {error}"))?;
        }
        #[cfg(target_arch = "wasm32")]
        {
            let builder = crate::editor::code::gitdiff::ReviewDiffBuilder::new(
                base.to_string(),
                current,
                3,
                Some(key.caret),
            );
            self.job = Some((key, builder));
        }
        self.pending = true;
        Ok(())
    }

    pub fn poll(&mut self) -> Result<Option<Output>, String> {
        if !self.pending {
            return Ok(None);
        }
        #[cfg(not(target_arch = "wasm32"))]
        let result = match self.worker.as_ref().unwrap().results.try_recv() {
            Ok(output) => Some(output),
            Err(std::sync::mpsc::TryRecvError::Empty) => None,
            Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                self.pending = false;
                self.worker = None;
                return Err("Live review worker disconnected".into());
            }
        };
        #[cfg(target_arch = "wasm32")]
        let result = {
            let deadline = web_time::Instant::now() + std::time::Duration::from_millis(1);
            let (key, builder) = self.job.as_mut().unwrap();
            let mut result = None;
            loop {
                if let Some(lines) = builder.poll(256) {
                    result = Some(Output {
                        key: key.clone(),
                        lines,
                    });
                    break;
                }
                if web_time::Instant::now() >= deadline {
                    break;
                }
            }
            if result.is_some() {
                self.job = None;
            }
            result
        };
        if result.is_some() {
            self.pending = false;
        }
        Ok(result)
    }
}
