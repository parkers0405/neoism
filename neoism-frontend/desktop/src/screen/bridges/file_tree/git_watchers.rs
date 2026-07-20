use super::*;
use std::path::Path;

impl Screen<'_> {
    pub(crate) fn refresh_file_tree_entries(&mut self) {
        self.renderer.file_tree.refresh();
        self.note_file_tree_git_status_scan();
    }

    pub(crate) fn note_file_tree_git_status_scan(&mut self) {
        self.file_tree_git_self_event_suppressed_until =
            Some(Instant::now() + FILE_TREE_GIT_SELF_EVENT_SUPPRESS);
    }

    fn file_tree_git_status_self_event_suppressed(&mut self) -> bool {
        let Some(until) = self.file_tree_git_self_event_suppressed_until else {
            return false;
        };
        if Instant::now() < until {
            return true;
        }
        self.file_tree_git_self_event_suppressed_until = None;
        false
    }

    pub fn refresh_file_tree_git_status(&mut self) -> bool {
        if !self.renderer.file_tree.is_visible() {
            return false;
        }
        if self.file_tree_git_status_self_event_suppressed() {
            self.file_tree_git_refresh_pending = false;
            return false;
        }
        self.start_file_tree_git_status_refresh();
        false
    }

    pub fn refresh_file_tree(&mut self) -> bool {
        if !self.renderer.file_tree.is_visible() {
            return false;
        }
        self.start_file_tree_git_status_refresh();
        false
    }

    pub fn apply_file_tree_git_status_refresh(&mut self) -> bool {
        let Some(rx) = self.file_tree_git_refresh_rx.as_ref() else {
            return false;
        };

        let mut latest = None;
        let mut completed = false;
        loop {
            match rx.try_recv() {
                Ok(result) => {
                    latest = Some(result);
                    completed = true;
                }
                Err(std_mpsc::TryRecvError::Empty) => break,
                Err(std_mpsc::TryRecvError::Disconnected) => {
                    completed = true;
                    break;
                }
            }
        }

        if !completed {
            return false;
        }

        self.file_tree_git_refresh_rx = None;
        self.file_tree_git_refresh_inflight = false;
        let changed = latest.is_some_and(|result| {
            self.renderer.file_tree.apply_git_refresh_result(result)
        });

        let pending = self.file_tree_git_refresh_pending;
        self.file_tree_git_refresh_pending = false;
        self.note_file_tree_git_status_scan();

        if changed {
            self.file_tree_fs_watch_root = None;
            self.sync_file_tree_fs_watcher();
            self.mark_dirty();
        }

        if pending {
            self.start_file_tree_git_status_refresh();
        }
        changed
    }

    pub(crate) fn start_file_tree_git_status_refresh(&mut self) {
        let action = neoism_ui::panels::file_tree::file_tree_git_refresh_action(
            neoism_ui::panels::file_tree::FileTreeGitRefreshState {
                visible: self.renderer.file_tree.is_visible(),
                inflight: self.file_tree_git_refresh_inflight,
                self_event_suppressed: false,
            },
        );
        match action {
            neoism_ui::panels::file_tree::FileTreeGitRefreshAction::Drop => {
                self.file_tree_git_refresh_pending = false;
                return;
            }
            neoism_ui::panels::file_tree::FileTreeGitRefreshAction::Queue => {
                self.file_tree_git_refresh_pending = true;
                return;
            }
            neoism_ui::panels::file_tree::FileTreeGitRefreshAction::Spawn => {}
        }
        // JOINED workspace: git lives on the HOST — fetch statuses
        // over the git plane instead of running local git against a
        // path that doesn't exist here.
        if self.renderer.file_tree.is_remote() {
            self.start_remote_git_status_refresh();
            return;
        }
        let Some(request) = self.renderer.file_tree.git_refresh_request() else {
            return;
        };
        self.note_file_tree_git_status_scan();

        let (tx, rx) = std_mpsc::channel();
        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let spawn_result = thread::Builder::new()
            .name("rio-file-tree-git".into())
            .spawn(move || {
                let result =
                    crate::editor::file_tree::FileTree::run_git_refresh_request(request);
                let _ = tx.send(result);
                event_proxy.send_event(
                    neoism_backend::event::RioEventType::Rio(
                        neoism_backend::event::RioEvent::ApplyFileTreeGitStatus,
                    ),
                    window_id,
                );
            });

        match spawn_result {
            Ok(_) => {
                self.file_tree_git_refresh_rx = Some(rx);
                self.file_tree_git_refresh_inflight = true;
            }
            Err(err) => {
                tracing::warn!(target: "neoism::file_tree", "unable to spawn git refresh worker: {err}");
            }
        }
    }

    pub(crate) fn sync_file_tree_watchers(&mut self) {
        self.sync_file_tree_git_watcher();
        self.sync_file_tree_fs_watcher();
    }

    pub(crate) fn sync_file_tree_fs_watcher(&mut self) {
        let next_root = self
            .renderer
            .file_tree
            .is_visible()
            // A JOINED workspace's root lives on the host machine —
            // there is nothing to watch locally; liveness comes from
            // the daemon's fs-watch `Changed` pushes instead.
            .then(|| {
                if self.renderer.file_tree.is_remote() {
                    None
                } else {
                    self.renderer.file_tree.root().map(Path::to_path_buf)
                }
            })
            .flatten();
        if self.file_tree_fs_watch_root == next_root {
            return;
        }

        // Dropping the handle drops the shutdown Sender; the worker
        // thread's `recv()` then returns Err and the watcher drops with
        // it. Re-`set_visible(true)` on a different root therefore tears
        // down the previous watcher cleanly without a join here.
        self.file_tree_fs_watcher = None;
        self.file_tree_fs_watch_root = None;

        let Some(root) = next_root else {
            return;
        };

        let watch_root = root.clone();
        let install_root = root.clone();
        // Decided on the main thread (cheap: a handful of stats) so the
        // worker can stay platform-only. Loose mode = don't watch the
        // subtree at all; we'd be flooded by Library / Downloads / iCloud
        // churn on macOS and we'd walk huge dirs on Linux.
        let project_mode = Self::is_project_workspace(&root);
        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let (shutdown_tx, shutdown_rx) = std_mpsc::channel::<()>();
        // Building the watcher and recursively installing one inotify
        // watch per source-dir was synchronous main-thread work; on
        // macOS the FSEventStream rebuild per `watch()` call was the
        // bigger sink. Move the entire install — walkdir traversal,
        // per-dir `watch()` calls, and the post-install park — onto a
        // worker. The watcher's notify-internal thread keeps delivering
        // events while this thread idles on the shutdown channel.
        let spawn_result = thread::Builder::new()
            .name("rio-file-tree-fs-watch".into())
            .spawn(move || {
                let mut watcher = match notify::RecommendedWatcher::new(
                    move |event: notify::Result<notify::Event>| match event {
                        Ok(event) if file_tree_fs_event_relevant(&watch_root, &event) => {
                            event_proxy.send_event(
                                neoism_backend::event::RioEventType::Rio(
                                    neoism_backend::event::RioEvent::PrepareRefreshFileTree,
                                ),
                                window_id,
                            );
                        }
                        Ok(_) => {}
                        Err(err) => {
                            tracing::warn!(target: "neoism::file_tree", "fs watcher failed: {err:?}");
                        }
                    },
                    notify::Config::default(),
                ) {
                    Ok(watcher) => watcher,
                    Err(err) => {
                        tracing::warn!(target: "neoism::file_tree", "unable to create fs watcher: {err:?}");
                        return;
                    }
                };

                let install_result = if project_mode {
                    watch_file_tree_root(&mut watcher, &install_root)
                } else {
                    // Loose mode: NonRecursive on root only across every
                    // platform. macOS skips kernel-recursive (which would
                    // flood from `~/Library` etc.); Linux skips the
                    // child-dir walk. User still sees the root listing
                    // and gets events for direct additions/removals in
                    // it. Deeper changes only refresh on user action —
                    // the right tradeoff outside a project.
                    watcher.watch(&install_root, notify::RecursiveMode::NonRecursive)
                };
                if let Err(err) = install_result {
                    tracing::warn!(
                        target: "neoism::file_tree",
                        path = %install_root.display(),
                        project_mode,
                        "unable to watch file tree root: {err:?}"
                    );
                    return;
                }

                // Park until the main thread drops the shutdown sender.
                // `recv` returns Err on disconnect; we don't care about
                // the value, just the wake-up.
                let _ = shutdown_rx.recv();
            });

        match spawn_result {
            Ok(_) => {
                self.file_tree_fs_watch_root = Some(root.clone());
                self.file_tree_fs_watcher = Some(FileTreeFsWatcherHandle {
                    root,
                    _shutdown: shutdown_tx,
                });
            }
            Err(err) => {
                tracing::warn!(
                    target: "neoism::file_tree",
                    "unable to spawn fs watcher worker: {err}"
                );
            }
        }
    }

    pub(crate) fn sync_file_tree_git_watcher(&mut self) {
        let next_root = self
            .renderer
            .file_tree
            .is_visible()
            .then(|| self.renderer.file_tree.root().map(Path::to_path_buf))
            .flatten();
        if self.file_tree_git_watch_root == next_root {
            return;
        }

        let Some(root) = next_root else {
            self.file_tree_git_watcher = None;
            self.file_tree_git_watch_root = None;
            return;
        };

        self.file_tree_git_watcher = None;
        self.file_tree_git_watch_root = Some(root.clone());

        let Some(paths) = crate::editor::file_tree::git_watch_paths_for(&root) else {
            return;
        };

        let event_proxy = self.context_manager.event_proxy();
        let window_id = self.context_manager.window_id();
        let mut watcher = match notify::RecommendedWatcher::new(
            move |event: notify::Result<notify::Event>| match event {
                Ok(event) if git_state_event_relevant(&event) => {
                    event_proxy.send_event(
                        neoism_backend::event::RioEventType::Rio(
                            neoism_backend::event::RioEvent::PrepareRefreshFileTreeGitStatus,
                        ),
                        window_id,
                    );
                }
                Ok(_) => {}
                Err(err) => {
                    tracing::warn!(target: "neoism::file_tree", "git watcher failed: {err:?}");
                }
            },
            notify::Config::default(),
        ) {
            Ok(watcher) => watcher,
            Err(err) => {
                tracing::warn!(target: "neoism::file_tree", "unable to create git watcher: {err:?}");
                return;
            }
        };

        if let Err(err) =
            watcher.watch(&paths.git_dir, notify::RecursiveMode::NonRecursive)
        {
            tracing::warn!(
                target: "neoism::file_tree",
                path = %paths.git_dir.display(),
                "unable to watch git dir: {err:?}"
            );
            return;
        }
        if let Some(refs_dir) = paths.refs_dir.as_ref() {
            if let Err(err) = watcher.watch(refs_dir, notify::RecursiveMode::Recursive) {
                tracing::warn!(
                    target: "neoism::file_tree",
                    path = %refs_dir.display(),
                    "unable to watch git refs: {err:?}"
                );
            }
        }
        self.file_tree_git_watcher = Some(watcher);
    }
}
