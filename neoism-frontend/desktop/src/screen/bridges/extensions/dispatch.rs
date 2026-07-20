use super::*;
use crate::workspace::extensions::ExtensionStatus;
use neoism_extensions::{
    ExtensionManifest, InstallHandle, InstalledIndex, ProgressEvent,
};
use neoism_ui::panels::notifications::NotificationLevel;
use tokio::sync::mpsc::unbounded_channel;

impl Screen<'_> {
    /// Look up the manifest, spawn the install task, and stash the
    /// `JoinHandle` + progress receiver on the renderer's
    /// `InstallTracker`. The per-frame pump (`pump_install_progress`)
    /// drains the receiver and finalises bookkeeping.
    pub(crate) fn dispatch_install(&mut self, id: &str) {
        // The progress button doubles as Cancel. Check before routing special
        // installers so every install kind behaves the same and a double click
        // cannot start two writers against one destination.
        if self.cancel_install_if_running(id) {
            return;
        }
        if is_builtin_extension_id(id) {
            self.dispatch_builtin_mcp_install(id);
            return;
        }
        if id == NEOISM_PYTHON_KERNEL_ID {
            self.dispatch_python_kernel_install(InstallSource::ExtensionsPanel);
            return;
        }
        if id == EVCXR_JUPYTER_KERNEL_ID {
            self.dispatch_rust_kernel_install();
            return;
        }

        let Some(manifest) = self.resolve_bundled_manifest(id) else {
            self.renderer.notifications.push(
                format!(
                    "No install plan for `{}` — the package catalog may still be syncing",
                    id
                ),
                NotificationLevel::Error,
            );
            return;
        };

        // Optimistic UI: flip the row to Installing immediately so the
        // user sees feedback before the first progress event arrives.
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::Installing {
                        percent: None,
                        status_text: "starting…".to_string(),
                    };
                    break;
                }
            }
        }

        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        // `install` internally calls `tokio::spawn`, so we need a
        // tokio runtime current on this thread for the duration of
        // that call. The desktop UI runs on winit's loop (no tokio
        // reactor), so enter our process-wide extensions runtime.
        let install_handle = {
            let _guard = ext_runtime_handle().enter();
            neoism_extensions::install(manifest, tx)
        };
        self.renderer.install_tracker.in_flight.insert(
            id.to_string(),
            InstallJob {
                install_handle,
                progress_rx: rx,
                last_percent: None,
                last_status: "starting…".to_string(),
                uninstall: false,
                source: InstallSource::ExtensionsPanel,
            },
        );
        self.mark_dirty();
    }

    fn cancel_install_if_running(&mut self, id: &str) -> bool {
        let Some(job) = self.renderer.install_tracker.in_flight.remove(id) else {
            return false;
        };
        job.install_handle.cancel();
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            if let Some(entry) =
                pane.entries_mut().iter_mut().find(|entry| entry.id == id)
            {
                entry.status = ExtensionStatus::NotInstalled;
            }
        }
        self.renderer.notifications.push(
            format!("Cancelled installation of {id}"),
            NotificationLevel::Info,
        );
        self.mark_dirty();
        true
    }

    fn dispatch_builtin_mcp_install(&mut self, id: &str) {
        let Some(manifest) = self.resolve_bundled_manifest(id) else {
            self.renderer.notifications.push(
                format!("Unknown built-in extension `{}`", id),
                NotificationLevel::Error,
            );
            return;
        };
        match install_builtin_mcp_record(&manifest) {
            Ok(()) => {
                if let Some(pane) = self
                    .context_manager
                    .current_mut()
                    .neoism_extensions
                    .as_mut()
                {
                    for entry in pane.entries_mut().iter_mut() {
                        if entry.id == id {
                            entry.status = ExtensionStatus::Installed {
                                version: manifest.version.clone(),
                            };
                            break;
                        }
                    }
                }
                self.renderer.notifications.push(
                    format!("Installed {}", manifest.name),
                    NotificationLevel::Info,
                );
                self.mark_dirty();
            }
            Err(error) => {
                self.renderer.notifications.push(
                    format!("Could not install {}: {}", manifest.name, error),
                    NotificationLevel::Error,
                );
            }
        }
    }

    pub(crate) fn dispatch_python_kernel_install(&mut self, source: InstallSource) {
        if self
            .renderer
            .install_tracker
            .in_flight
            .contains_key(NEOISM_PYTHON_KERNEL_ID)
        {
            return;
        }
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == NEOISM_PYTHON_KERNEL_ID {
                    entry.status = ExtensionStatus::Installing {
                        percent: None,
                        status_text: "creating managed Python environment…".to_string(),
                    };
                    break;
                }
            }
        }

        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        let task = ext_runtime_handle()
            .spawn(async move { install_managed_python_kernel(tx).await });
        let install_handle = InstallHandle::from_task(NEOISM_PYTHON_KERNEL_ID, task);
        self.renderer.install_tracker.in_flight.insert(
            NEOISM_PYTHON_KERNEL_ID.to_string(),
            InstallJob {
                install_handle,
                progress_rx: rx,
                last_percent: None,
                last_status: "creating managed Python environment…".to_string(),
                uninstall: false,
                source,
            },
        );
        self.mark_dirty();
    }

    fn dispatch_rust_kernel_install(&mut self) {
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == EVCXR_JUPYTER_KERNEL_ID {
                    entry.status = ExtensionStatus::Installing {
                        percent: None,
                        status_text: "installing evcxr_jupyter…".to_string(),
                    };
                    break;
                }
            }
        }

        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        let task = ext_runtime_handle()
            .spawn(async move { install_rust_jupyter_kernel(tx).await });
        let install_handle = InstallHandle::from_task(EVCXR_JUPYTER_KERNEL_ID, task);
        self.renderer.install_tracker.in_flight.insert(
            EVCXR_JUPYTER_KERNEL_ID.to_string(),
            InstallJob {
                install_handle,
                progress_rx: rx,
                last_percent: None,
                last_status: "installing evcxr_jupyter…".to_string(),
                uninstall: false,
                source: InstallSource::ExtensionsPanel,
            },
        );
        self.mark_dirty();
    }

    /// Catalog package-id candidates for a server, sourced from the
    /// Neoism LSP engine's own adapter registry: each adapter declares
    /// the exact package/executable pairs that can supply its command,
    /// so the page and the runtime can never disagree about what an
    /// install would provide. `server` may be an engine adapter id
    /// (e.g. `rust`, `typescript`) or already a catalog package id.
    fn catalog_candidates_for_server(server: &str) -> Vec<String> {
        let adapters = neoism_agent_server::language_server::language_server_adapters();
        let mut candidates: Vec<String> = adapters
            .iter()
            .find(|adapter| adapter.id == server)
            .map(|adapter| {
                adapter
                    .catalog_packages
                    .iter()
                    .map(|package| package.package_id.clone())
                    .collect()
            })
            .unwrap_or_default();
        if !candidates.iter().any(|candidate| candidate == server) {
            candidates.push(server.to_string());
        }
        candidates
    }

    /// Resolve a catalog `ExtensionManifest` for a language server.
    /// Returns `None` if the catalog cache is missing or no candidate
    /// matches — the modal then tells the user to install manually.
    pub(crate) fn resolve_catalog_manifest_for_server(
        &self,
        server: &str,
    ) -> Option<ExtensionManifest> {
        let candidates = Self::catalog_candidates_for_server(server);
        let catalog_path = neoism_extensions::mason::mason_cache_path();
        let registry = neoism_extensions::load_mason_registry(&catalog_path).ok()?;
        for candidate in &candidates {
            if let Some(pkg) = registry.iter().find(|p| p.name == *candidate) {
                let manifest = neoism_extensions::package_to_manifest(pkg).ok()?;
                if manifest_is_auto_installable_lsp(&manifest) {
                    return Some(manifest);
                }
            }
        }
        None
    }

    /// Modal-driven entry point: spawn an install via the runner and
    /// stash it in the tracker tagged with `MissingLspModal` so the
    /// per-frame pump knows to close the busy modal and open the
    /// success/failure modal on completion. Reuses the same runner +
    /// tracker as the Extensions panel so completion bookkeeping
    /// (installed.json, managed bin map) lives in exactly one place;
    /// the LSP engine re-resolves command sources on its next use.
    pub(crate) fn dispatch_install_via_runner(
        &mut self,
        manifest: ExtensionManifest,
        server: String,
        display: String,
    ) {
        let id = manifest.id.clone();
        if self.renderer.install_tracker.in_flight.contains_key(&id) {
            return;
        }
        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        // `install` internally calls `tokio::spawn`, so we need a
        // tokio runtime current on this thread for the duration of
        // that call. The desktop UI runs on winit's loop (no tokio
        // reactor), so enter our process-wide extensions runtime.
        let install_handle = {
            let _guard = ext_runtime_handle().enter();
            neoism_extensions::install(manifest, tx)
        };
        self.renderer.install_tracker.in_flight.insert(
            id,
            InstallJob {
                install_handle,
                progress_rx: rx,
                last_percent: None,
                last_status: "starting…".to_string(),
                uninstall: false,
                source: InstallSource::MissingLspModal { server, display },
            },
        );
        self.mark_dirty();
    }

    /// Synchronous uninstall: remove the install dir + symlink, strip
    /// `mcp.<id>` from the agent config (when applicable), update
    /// `installed.json`, refresh the managed-bin map. The pane row
    /// flips through `Uninstalling` first so the click feels live.
    pub(crate) fn dispatch_uninstall(&mut self, id: &str) {
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::Uninstalling;
                    break;
                }
            }
        }

        let manifest = self.lookup_bundled_manifest(id);
        let builtin = is_builtin_extension_id(id)
            || manifest
                .as_ref()
                .is_some_and(|m| m.categories.iter().any(|c| c == "Built-in"));
        let mut index = InstalledIndex::load().unwrap_or_default();
        let removed = if builtin {
            index.disable_builtin(id);
            None
        } else {
            index.remove_record(id)
        };

        // Tear down the symlink + on-disk install dir. Best-effort:
        // missing files are fine, the user might have already wiped
        // them out-of-band.
        if !builtin {
            if let Some(record) = removed.as_ref() {
                if let Some(bin) = record.bin_path.as_ref() {
                    let _ = std::fs::remove_file(bin);
                }
            }
            if let Some(manifest) = manifest.as_ref() {
                let mut executable_names = manifest.executables.clone();
                if let Some(primary) =
                    manifest.run.as_ref().and_then(|run| run.command.first())
                {
                    if !executable_names.contains(primary) {
                        executable_names.push(primary.clone());
                    }
                }
                let managed_bin_dir = neoism_extensions::paths::bin_dir();
                for executable in executable_names {
                    if !executable.is_empty()
                        && executable != "."
                        && executable != ".."
                        && !executable.contains('/')
                        && !executable.contains('\\')
                    {
                        let _ = std::fs::remove_file(managed_bin_dir.join(executable));
                    }
                }
            }
            let install_dir = neoism_extensions::paths::install_dir_for(id);
            let _ = std::fs::remove_dir_all(&install_dir);
        }

        let _ = index.save();

        if let Some(m) = manifest.as_ref() {
            if is_mcp_entry(m) {
                if builtin {
                    let _ =
                        neoism_extensions::agent_config::disable_builtin_mcp_entry(id);
                } else {
                    let _ = neoism_extensions::agent_config::uninstall_mcp_entry(id);
                }
            }
        }

        // Language-server rows: with the managed binary gone, re-ask the
        // engine where the command resolves now — a copy on `$PATH` keeps
        // working, and the row must say so instead of claiming "missing".
        let post_uninstall_source = manifest
            .as_ref()
            .and_then(|m| m.run.as_ref())
            .filter(|run| !run.command.is_empty())
            .map(|run| {
                neoism_agent_server::language_server::command_source(
                    id,
                    run.command.clone(),
                )
            });
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::NotInstalled;
                    if entry.lsp_source.is_some() {
                        if let Some(source) = &post_uninstall_source {
                            use neoism_agent_server::language_server::LspCommandSource;
                            if !matches!(source, LspCommandSource::Missing) {
                                entry.status = ExtensionStatus::Detected;
                            }
                            entry.lsp_source =
                                Some(command_source_label(source).to_string());
                        }
                    }
                    break;
                }
            }
        }

        let label = manifest
            .as_ref()
            .map(|m| m.name.clone())
            .unwrap_or_else(|| id.to_string());
        self.renderer
            .notifications
            .push(format!("Uninstalled {}", label), NotificationLevel::Info);
    }
}
