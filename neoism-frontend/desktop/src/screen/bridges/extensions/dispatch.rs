use super::*;
use crate::workspace::extensions::ExtensionStatus;
use neoism_extensions::{
    ExtensionManifest, InstallError, InstalledEntry, InstalledIndex, ProgressEvent,
};
use neoism_ui::panels::notifications::NotificationLevel;
use tokio::sync::mpsc::unbounded_channel;

impl Screen<'_> {
    /// Look up the manifest, spawn the install task, and stash the
    /// `JoinHandle` + progress receiver on the renderer's
    /// `InstallTracker`. The per-frame pump (`pump_install_progress`)
    /// drains the receiver and finalises bookkeeping.
    pub(crate) fn dispatch_install(&mut self, id: &str) {
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
        if let Some(lang) = treesitter_lang_from_extension_id(id) {
            self.dispatch_treesitter_parser_install(lang.to_string());
            return;
        }

        let Some(manifest) = self.lookup_bundled_manifest(id) else {
            self.renderer.notifications.push(
                format!("Unknown extension `{}`", id),
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
                        percent: 0,
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
        let join_handle = {
            let _guard = ext_runtime_handle().enter();
            neoism_extensions::install(manifest, tx)
        };
        self.renderer.install_tracker.in_flight.insert(
            id.to_string(),
            InstallJob {
                join_handle,
                progress_rx: rx,
                last_percent: 0,
                last_status: "starting…".to_string(),
                uninstall: false,
                source: InstallSource::ExtensionsPanel,
            },
        );
        self.mark_dirty();
    }

    fn dispatch_builtin_mcp_install(&mut self, id: &str) {
        let Some(manifest) = self.lookup_bundled_manifest(id) else {
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

    pub(crate) fn dispatch_treesitter_parser_install(&mut self, lang: String) {
        let Some(spec) = crate::neoism::ide_tools::treesitter_install_spec(&lang) else {
            self.renderer.notifications.push(
                format!("No Treesitter installer for {lang}"),
                NotificationLevel::Warn,
            );
            return;
        };
        let id = treesitter_extension_id(spec.lang);
        if self.renderer.install_tracker.in_flight.contains_key(&id) {
            return;
        }
        self.treesitter_installing.insert(spec.lang.to_string());

        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::Installing {
                        percent: 0,
                        status_text: format!("installing {} parser…", spec.display_name),
                    };
                    break;
                }
            }
        }

        self.renderer.notifications.push(
            format!("Installing {} syntax parser", spec.display_name),
            NotificationLevel::Info,
        );

        let install_id = id.clone();
        let install_lang = spec.lang.to_string();
        let display_name = spec.display_name.to_string();
        let parser_path = treesitter_parser_path(spec.lang);
        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        let join_handle = ext_runtime_handle().spawn(async move {
            let _ = tx.send(ProgressEvent::Started);
            let _ = tx.send(ProgressEvent::Progress {
                percent: 8,
                status: format!("fetching {} grammar", display_name),
            });
            let install_lang_for_blocking = install_lang.clone();
            let result = tokio::task::spawn_blocking(move || {
                crate::neoism::ide_tools::install_treesitter_parser(
                    &install_lang_for_blocking,
                )
            })
            .await
            .map_err(|err| InstallError::ParseManifest(format!("install join: {err}")))?;

            match result {
                Ok(message) => {
                    let _ = tx.send(ProgressEvent::Progress {
                        percent: 96,
                        status: message,
                    });
                    let _ = tx.send(ProgressEvent::Done);
                    Ok(InstalledEntry {
                        id: install_id,
                        version: env!("CARGO_PKG_VERSION").to_string(),
                        install_kind: "treesitter".to_string(),
                        bin_path: Some(parser_path),
                        installed_at: now_millis_i64(),
                    })
                }
                Err(message) => {
                    let _ = tx.send(ProgressEvent::Failed {
                        message: message.clone(),
                    });
                    Err(InstallError::CommandFailed {
                        command: format!("install treesitter {}", install_lang),
                        status: 1,
                        stderr: message,
                    })
                }
            }
        });

        self.renderer.install_tracker.in_flight.insert(
            id,
            InstallJob {
                join_handle,
                progress_rx: rx,
                last_percent: 0,
                last_status: format!("installing {} parser…", spec.display_name),
                uninstall: false,
                source: InstallSource::TreeSitterParser {
                    lang: spec.lang.to_string(),
                },
            },
        );
        self.mark_dirty();
    }

    pub(crate) fn dispatch_python_kernel_install(&mut self, source: InstallSource) {
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == NEOISM_PYTHON_KERNEL_ID {
                    entry.status = ExtensionStatus::Installing {
                        percent: 0,
                        status_text: "creating managed Python environment…".to_string(),
                    };
                    break;
                }
            }
        }

        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        let join_handle = ext_runtime_handle()
            .spawn(async move { install_managed_python_kernel(tx).await });
        self.renderer.install_tracker.in_flight.insert(
            NEOISM_PYTHON_KERNEL_ID.to_string(),
            InstallJob {
                join_handle,
                progress_rx: rx,
                last_percent: 0,
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
                        percent: 0,
                        status_text: "installing evcxr_jupyter…".to_string(),
                    };
                    break;
                }
            }
        }

        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        let join_handle = ext_runtime_handle()
            .spawn(async move { install_rust_jupyter_kernel(tx).await });
        self.renderer.install_tracker.in_flight.insert(
            EVCXR_JUPYTER_KERNEL_ID.to_string(),
            InstallJob {
                join_handle,
                progress_rx: rx,
                last_percent: 0,
                last_status: "installing evcxr_jupyter…".to_string(),
                uninstall: false,
                source: InstallSource::ExtensionsPanel,
            },
        );
        self.mark_dirty();
    }

    /// Mason package name candidates to try, in order, for an lsp.lua
    /// server name. lsp.lua's `server.name` does not always match the
    /// Mason package id; this passthrough/alias map covers the nine
    /// known servers + falls back to the bare name for anything else.
    fn mason_lookup_candidates_for_server(server: &str) -> Vec<String> {
        match server {
            "ts_ls" => vec![
                "typescript-language-server".to_string(),
                "ts_ls".to_string(),
            ],
            "yamlls" => vec!["yaml-language-server".to_string(), "yamlls".to_string()],
            "jsonls" => vec![
                "json-lsp".to_string(),
                "jsonls".to_string(),
                "vscode-json-language-server".to_string(),
            ],
            "nil_ls" => vec!["nil".to_string(), "nil_ls".to_string()],
            other => vec![other.to_string()],
        }
    }

    /// Resolve a Mason `ExtensionManifest` for an lsp.lua server name.
    /// Returns `None` if the Mason cache is missing or no candidate
    /// matches — the modal then tells the user to install manually.
    pub(crate) fn resolve_mason_manifest_for_server(
        &self,
        server: &str,
    ) -> Option<ExtensionManifest> {
        let candidates = Self::mason_lookup_candidates_for_server(server);
        let mason_path = neoism_extensions::mason::mason_cache_path();
        let registry = neoism_extensions::load_mason_registry(&mason_path).ok()?;
        for candidate in &candidates {
            if let Some(pkg) = registry.iter().find(|p| p.name == *candidate) {
                return neoism_extensions::package_to_manifest(pkg).ok();
            }
        }
        None
    }

    /// Modal-driven entry point: spawn an install via the runner and
    /// stash it in the tracker tagged with `MissingLspModal` so the
    /// per-frame pump knows to close the busy modal and broadcast the
    /// LSP retry on completion. Reuses the same runner + tracker as
    /// the Extensions panel so completion bookkeeping (installed.json,
    /// managed_bin_map refresh) lives in exactly one place.
    pub(crate) fn dispatch_install_via_runner(
        &mut self,
        manifest: ExtensionManifest,
        server: String,
        display: String,
    ) {
        let id = manifest.id.clone();
        let (tx, rx) = unbounded_channel::<ProgressEvent>();
        // `install` internally calls `tokio::spawn`, so we need a
        // tokio runtime current on this thread for the duration of
        // that call. The desktop UI runs on winit's loop (no tokio
        // reactor), so enter our process-wide extensions runtime.
        let join_handle = {
            let _guard = ext_runtime_handle().enter();
            neoism_extensions::install(manifest, tx)
        };
        self.renderer.install_tracker.in_flight.insert(
            id,
            InstallJob {
                join_handle,
                progress_rx: rx,
                last_percent: 0,
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
        if let Some(lang) = treesitter_lang_from_extension_id(id) {
            self.dispatch_treesitter_parser_uninstall(id, lang);
            return;
        }

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

        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::NotInstalled;
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

    fn dispatch_treesitter_parser_uninstall(&mut self, id: &str, lang: &str) {
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

        let mut index = InstalledIndex::load().unwrap_or_default();
        index.remove_record(id);
        let _ = index.save();

        let _ = std::fs::remove_file(treesitter_parser_path(lang));
        let _ = std::fs::remove_dir_all(treesitter_query_dir(lang));

        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            for entry in pane.entries_mut().iter_mut() {
                if entry.id == id {
                    entry.status = ExtensionStatus::NotInstalled;
                    break;
                }
            }
        }

        self.retry_treesitter_syntax_in_nvim();
        self.renderer.notifications.push(
            format!("Uninstalled {lang} syntax parser"),
            NotificationLevel::Info,
        );
        self.mark_dirty();
    }
}
