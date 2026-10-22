//! Bounded loading of repository review patches; never shell out on a frame.
use super::GitDiffPanel;

impl GitDiffPanel {
    pub fn request_review_diffs(&mut self) {
        let Some(io) = self.io.clone() else { return };
        let (root, id, files) = {
            let Ok(mut data) = self.data.lock() else {
                return;
            };
            if data.loading
                || data.error.is_some()
                || data.review_load_id == Some(data.refresh_id)
            {
                return;
            }
            let Some(root) = data.repo_root.clone() else {
                return;
            };
            let selected = data.files.get(self.selected).map(|file| file.path.as_str());
            let mut files: Vec<_> = data
                .files
                .iter()
                .filter(|file| !data.diffs.contains_key(&file.path))
                .cloned()
                .collect();
            if files.is_empty() {
                return;
            }
            files.sort_by_key(|file| selected != Some(file.path.as_str()));
            let id = data.refresh_id;
            data.review_load_id = Some(id);
            (root, id, files)
        };
        if io.is_host_driven() {
            io.request_diffs(
                &files
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>(),
            );
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let data = std::sync::Arc::clone(&self.data);
            std::thread::spawn(move || {
                for file in files {
                    {
                        let Ok(data) = data.lock() else { return };
                        if data.refresh_id != id || data.repo_root.as_ref() != Some(&root)
                        {
                            return;
                        }
                        if data.diffs.contains_key(&file.path) {
                            continue;
                        }
                    }
                    let lines = super::parse::load_diff(&root, &file);
                    let Ok(mut data) = data.lock() else { return };
                    if data.refresh_id != id || data.repo_root.as_ref() != Some(&root) {
                        return;
                    }
                    data.diffs.insert(file.path, lines);
                    data.diff_cache_generation =
                        data.diff_cache_generation.wrapping_add(1);
                }
                if let Ok(mut data) = data.lock() {
                    if data.refresh_id == id {
                        data.review_load_id = None;
                    }
                }
            });
        }
        #[cfg(target_arch = "wasm32")]
        {
            let _ = (root, id, files);
        }
    }
}
