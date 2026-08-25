use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, Weak};

use tokio::sync::{Mutex as AsyncMutex, OwnedMutexGuard};

#[derive(Clone, Default)]
pub(crate) struct FileLockRegistry {
    inner: Arc<Mutex<HashMap<PathBuf, Weak<AsyncMutex<()>>>>>,
}

pub(crate) struct FileLockGuards {
    guards: Vec<OwnedMutexGuard<()>>,
    keys: Vec<PathBuf>,
    registry: FileLockRegistry,
}

impl Drop for FileLockGuards {
    fn drop(&mut self) {
        self.guards.clear();
        self.registry.cleanup(&self.keys);
    }
}

impl FileLockRegistry {
    pub(crate) async fn lock_file(&self, path: &Path) -> FileLockGuards {
        self.lock_files([path.to_path_buf()]).await
    }

    pub(crate) async fn lock_files(
        &self,
        paths: impl IntoIterator<Item = PathBuf>,
    ) -> FileLockGuards {
        let mut keys = paths.into_iter().map(lock_key).collect::<Vec<_>>();
        keys.sort();
        keys.dedup();

        let mut locked = FileLockGuards {
            guards: Vec::with_capacity(keys.len()),
            keys,
            registry: self.clone(),
        };
        for key in &locked.keys {
            tracing::info!(path = %key.display(), "edit file lock waiting");
            let lock = self.file_lock(key);
            locked.guards.push(lock.lock_owned().await);
            tracing::info!("edit file lock acquired");
        }
        locked
    }

    fn file_lock(&self, key: &Path) -> Arc<AsyncMutex<()>> {
        let mut registry = self.inner.lock().expect("file lock registry poisoned");
        registry.retain(|_, lock| lock.strong_count() > 0);
        if let Some(lock) = registry.get(key).and_then(Weak::upgrade) {
            return lock;
        }
        let lock = Arc::new(AsyncMutex::new(()));
        registry.insert(key.to_path_buf(), Arc::downgrade(&lock));
        lock
    }

    fn cleanup(&self, keys: &[PathBuf]) {
        let mut registry = self.inner.lock().expect("file lock registry poisoned");
        for key in keys {
            if registry
                .get(key)
                .is_some_and(|lock| lock.strong_count() == 0)
            {
                registry.remove(key);
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn entry_count(&self) -> usize {
        self.inner
            .lock()
            .expect("file lock registry poisoned")
            .len()
    }
}

fn lock_key(path: PathBuf) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let Some(parent) = path.parent() else {
        return path;
    };
    let parent = parent
        .canonicalize()
        .unwrap_or_else(|_| parent.to_path_buf());
    match path.file_name() {
        Some(name) => parent.join(name),
        None => parent,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unused_file_lock_entries_are_removed() {
        let registry = FileLockRegistry::default();
        for index in 0..128 {
            let guard = registry
                .lock_file(&PathBuf::from(format!("unused-{index}")))
                .await;
            drop(guard);
        }
        assert_eq!(registry.entry_count(), 0);
    }

    #[tokio::test]
    async fn cancelled_waiters_do_not_leave_registry_entries() {
        let registry = FileLockRegistry::default();
        let path = PathBuf::from("cancelled-waiter");
        let held = registry.lock_file(&path).await;
        let waiting = registry.lock_file(&path);
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), waiting)
                .await
                .is_err()
        );
        drop(held);
        assert_eq!(registry.entry_count(), 0);
    }
}
