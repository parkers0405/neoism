//! Lazy full-file baselines for live review projections.
use std::path::Path;

use super::GitDiffPanel;

fn key(root: &Path, path: &Path) -> String {
    neoism_protocol::host_path::HostPath::new(root.to_string_lossy())
        .relative(&path.to_string_lossy())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

impl GitDiffPanel {
    pub fn request_diff_base(&mut self, path: &Path) {
        let Some(io) = self.io.clone() else { return };
        let (root, path, revision) = {
            let Ok(mut data) = self.data.lock() else {
                return;
            };
            let Some(root) = data.repo_root.clone() else {
                return;
            };
            let path = key(&root, path);
            let revision = data.refresh_id;
            if data.base_texts.contains_key(&path)
                || data.base_errors.contains_key(&path)
                || data.base_pending.get(&path) == Some(&revision)
            {
                return;
            }
            data.base_pending.insert(path.clone(), revision);
            (root, path, revision)
        };
        if io.is_host_driven() {
            io.request_base(&path);
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let data = std::sync::Arc::clone(&self.data);
            std::thread::spawn(move || {
                let result = io.load_base(&root, &path);
                let Ok(mut data) = data.lock() else { return };
                if data.repo_root.as_ref() != Some(&root)
                    || data.refresh_id != revision
                    || data.base_pending.get(&path) != Some(&revision)
                {
                    return;
                }
                data.base_pending.remove(&path);
                match result {
                    Ok(text) => {
                        data.base_texts.insert(path, text.into());
                    }
                    Err(error) => {
                        data.base_errors.insert(path, error);
                    }
                }
                data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (root, path, revision);
        }
    }

    pub fn diff_base_for_path(&self, path: &Path) -> Option<std::sync::Arc<str>> {
        let data = self.data.lock().ok()?;
        let path = key(data.repo_root.as_ref()?, path);
        data.base_texts.get(&path).cloned()
    }

    pub fn diff_base_error(&self, path: &Path) -> Option<String> {
        let data = self.data.lock().ok()?;
        let path = key(data.repo_root.as_ref()?, path);
        data.base_errors.get(&path).cloned().or_else(|| {
            data.base_pending
                .contains_key(&path)
                .then(|| data.error.clone())
                .flatten()
        })
    }

    pub fn diff_base_pending(&self, path: &Path) -> bool {
        let Ok(data) = self.data.lock() else {
            return false;
        };
        let Some(root) = data.repo_root.as_ref() else {
            return false;
        };
        data.error.is_none()
            && data.base_pending.get(&key(root, path)) == Some(&data.refresh_id)
    }

    pub fn host_set_base_text(&mut self, path: &str, text: String) {
        if let Ok(mut data) = self.data.lock() {
            data.base_pending.remove(path);
            data.base_errors.remove(path);
            data.base_texts.insert(path.to_owned(), text.into());
            data.diff_cache_generation = data.diff_cache_generation.wrapping_add(1);
        }
    }
}
