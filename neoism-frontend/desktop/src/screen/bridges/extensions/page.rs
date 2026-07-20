use super::*;
use crate::workspace::extensions::{ExtensionEntry, ExtensionStatus};
use neoism_extensions::{ExtensionManifest, InstalledIndex, ProgressEvent};
use neoism_ui::panels::extensions_page::NeoismExtensionsPane;
use neoism_ui::panels::notifications::NotificationLevel;
use std::sync::atomic::Ordering;

impl Screen<'_> {
    pub(crate) fn start_python_kernel_install_modal(&mut self) {
        if crate::notebook_runtime::has_managed_python_kernel() {
            let retry_result =
                self.pending_python_kernel_retry
                    .take()
                    .map(|(path, cell_index)| {
                        self.retry_notebook_cell_after_python_kernel_install(
                            &path, cell_index,
                        )
                    });
            let meta = match retry_result {
                Some(Ok(())) => "Retrying the failed notebook cell now.".to_string(),
                Some(Err(err)) => {
                    format!("Kernel is installed. Could not retry automatically: {err}")
                }
                None => "Run the notebook cell again to start the kernel.".to_string(),
            };
            self.renderer
                .modal
                .open(neoism_ui::widgets::modal::ModalSpec {
                    title: "Python Kernel Installed".to_string(),
                    body: "Neoism already has a usable managed Python kernel."
                        .to_string(),
                    meta,
                    input: None,
                    buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                        "Close",
                        "Esc",
                        neoism_ui::widgets::modal::ModalAction::Close,
                    )],
                    busy: false,
                    blocking: false,
                });
            return;
        }
        self.renderer.modal.open(neoism_ui::widgets::modal::ModalSpec {
            title: "Installing Python Kernel".to_string(),
            body: "Neoism is creating a managed Python environment and installing ipykernel. This may take a minute the first time.".to_string(),
            meta: "Notebook execution will use this kernel when installation finishes.".to_string(),
            input: None,
            buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                "Keep Working",
                "Esc",
                neoism_ui::widgets::modal::ModalAction::Close,
            )],
            busy: true,
            blocking: false,
        });
        let retry_notebook_cell = self.pending_python_kernel_retry.take();
        self.dispatch_python_kernel_install(InstallSource::PythonKernelModal {
            retry_notebook_cell,
        });
    }

    /// Open (or activate) the Extensions browser as a stacked buffer tab.
    ///
    /// Extensions is a singleton page — calling this twice activates the
    /// existing context instead of spawning a duplicate. The buffer-tab
    /// strip label is sourced from the basename of a synthetic path; we
    /// pick `~/.config/neoism/Extensions` (file not created) so the tab
    /// reads "Extensions" via the existing `open_markdown` path channel.
    pub(crate) fn open_extensions_page(&mut self) {
        use neoism_ui::panels::buffer_tabs::ChromePageKind;

        self.renderer.buffer_tabs.ensure_terminal_tab();
        self.renderer.file_tree.set_active_path(None);

        // Create / activate the singleton extensions context first so
        // we have its route_id to bind into the buffer-tabs strip.
        self.activate_neoism_extensions_page();
        // Auto-focus the search box on open (Cmd+P style) so the cursor
        // is ready to type immediately. `/` or Cmd+F refocus it later.
        if let Some(pane) = self
            .context_manager
            .current_mut()
            .neoism_extensions
            .as_mut()
        {
            pane.focus_search();
        }
        let route_id = self
            .context_manager
            .neoism_extensions_node()
            .map(|(route_id, _)| route_id)
            .unwrap_or(0);
        self.renderer
            .buffer_tabs
            .open_chrome_page(ChromePageKind::Extensions, route_id);

        self.kick_catalog_seed_if_needed();
        self.reapply_chrome_layout();
        self.renderer.trail_cursor.reset();
        self.mark_dirty();
    }

    /// Activate the singleton Extensions context; if absent, spawn one and
    /// seed it with the bundled MCP registry.
    pub(crate) fn activate_neoism_extensions_page(&mut self) {
        if let Some((_route_id, node)) = self.context_manager.neoism_extensions_node() {
            let _ = self
                .context_manager
                .current_grid_mut()
                .set_current_node(node, &mut self.sugarloaf);
            self.context_manager.select_route_from_current_grid();
            return;
        }

        let rich_text_id = crate::context::factories::next_rich_text_id();
        let _ = self.sugarloaf.text(Some(rich_text_id));
        if !self
            .context_manager
            .add_stacked_neoism_extensions(rich_text_id, &mut self.sugarloaf)
        {
            self.file_tree_notify(
                "Could not open Extensions pane",
                neoism_ui::panels::notifications::NotificationLevel::Error,
            );
            return;
        }

        let entries = self.load_bundled_extension_entries();
        for (_node, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if let Some(pane) = item.val.neoism_extensions.as_mut() {
                pane.set_entries(entries);
                break;
            }
        }
    }

    /// Build the `ExtensionEntry` list: the bundled MCP registry and
    /// kernels joined against the local `installed.json`, plus one card
    /// per adapter in the Neoism LSP engine's runtime registry (real
    /// connected/installed state) and the compiled-in tree-sitter
    /// grammars. Also populates `renderer.bundled_manifests` so the
    /// install/uninstall dispatcher can re-resolve a full manifest from
    /// an `ExtensionEntry` id.
    fn load_bundled_extension_entries(&mut self) -> Vec<ExtensionEntry> {
        let notes_manifest = neoism_notes_mcp_manifest();
        let memory_manifest = neoism_memory_mcp_manifest();
        let builtins = vec![notes_manifest.clone(), memory_manifest.clone()];
        let mut installed = ensure_builtin_mcp_installed(&builtins);

        // MCP entries: tag each with the literal "MCP Server" category
        // so the panel's `McpServers` tab filter (which substring-
        // matches "mcp") surfaces them. The bundled registry's own
        // categories ("Cloud", "Official", "Creative", etc.) don't
        // include the tab name on purpose — the source determines the
        // tab, not the per-package metadata.
        let mut mcp_manifests =
            neoism_extensions::parse_bundled_mcp_registry().unwrap_or_default();
        for m in mcp_manifests.iter_mut() {
            if !m
                .categories
                .iter()
                .any(|c| c.eq_ignore_ascii_case("MCP Server"))
            {
                m.categories.insert(0, "MCP Server".to_string());
            }
        }

        let python_kernel_manifest = neoism_python_kernel_manifest();
        let rust_kernel_manifest = evcxr_jupyter_kernel_manifest();
        let mut manifests = mcp_manifests;
        manifests.insert(0, rust_kernel_manifest);
        manifests.insert(0, python_kernel_manifest);
        manifests.insert(0, memory_manifest);
        manifests.insert(0, notes_manifest);

        match neoism_extensions::reconcile_managed_installs(&manifests) {
            Ok(report) => {
                if !report.recovered.is_empty() {
                    tracing::info!(
                        target: "neoism::extensions",
                        recovered = ?report.recovered,
                        "recovered managed installs missing from installed index"
                    );
                }
                installed = report.index;
            }
            Err(error) => {
                tracing::warn!(
                    target: "neoism::extensions",
                    ?error,
                    "could not reconcile managed extension installs"
                );
            }
        }

        // Populate the manifest cache keyed by id BEFORE consuming the
        // vec into entries — needed by the install/uninstall dispatch.
        // Catalog manifests for the engine's language-server adapters are
        // cached alongside so the LSP rows' Install/Uninstall buttons can
        // resolve a real install plan by row id.
        self.renderer.bundled_manifests = manifests
            .iter()
            .map(|m| (m.id.clone(), m.clone()))
            .collect();
        for manifest in catalog_manifests_for_engine_adapters() {
            self.renderer
                .bundled_manifests
                .insert(manifest.id.clone(), manifest);
        }

        let mut entries: Vec<ExtensionEntry> = manifests
            .into_iter()
            .map(|m| {
                let installed_version =
                    installed.get(&m.id).map(|e| e.version.clone());
                extension_manifest_to_entry(m, installed_version)
            })
            .collect();
        let workspace_root = self.active_pane_workspace_root();
        entries.extend(language_server_entries(workspace_root.as_deref(), &installed));
        entries.extend(built_in_syntax_entries());
        entries
    }

    /// First-time-open trigger to background-fetch the package-catalog
    /// snapshot that backs the language-server Install buttons. We don't
    /// block the UI on it — on cache miss the LSP rows simply resolve
    /// their install plans once the fetch lands. Subsequent opens hit
    /// the cache's 24h freshness check and short-circuit immediately.
    fn kick_catalog_seed_if_needed(&mut self) {
        if self.renderer.catalog_seeded {
            return;
        }
        self.renderer.catalog_seeded = true;
        // Detached: the result hits disk; the per-frame pump in
        // `render_neoism_extensions_panels` watches `CATALOG_CACHE_FRESH`
        // and re-seeds the visible pane's entries once this lands.
        ext_runtime_handle().spawn(async move {
            let _ = neoism_extensions::ensure_cached_mason_registry().await;
            CATALOG_CACHE_FRESH.store(true, Ordering::Release);
        });
    }

    /// Called from `render_neoism_extensions_panels` once per frame. If
    /// the background catalog seed has reported a fresh cache since the
    /// last call, rebuild the Extensions pane's entries (preserving
    /// install state via `installed.json`) so the language-server rows'
    /// install plans resolve without the user closing/reopening the tab.
    pub(crate) fn drain_catalog_cache_refresh(&mut self) {
        if !CATALOG_CACHE_FRESH.swap(false, Ordering::AcqRel) {
            return;
        }
        let entries = self.load_bundled_extension_entries();
        for (_, item) in self
            .context_manager
            .current_grid_mut()
            .contexts_mut()
            .iter_mut()
        {
            if let Some(pane) = item.val.neoism_extensions.as_mut() {
                pane.set_entries(entries.clone());
            }
        }
        self.mark_dirty();
    }

    /// Re-resolve a full `ExtensionManifest` from a panel entry id via
    /// the renderer-side cache built by `load_bundled_extension_entries`.
    pub(crate) fn lookup_bundled_manifest(&self, id: &str) -> Option<ExtensionManifest> {
        self.renderer.bundled_manifests.get(id).cloned()
    }

    /// Resolve an install action against the live registry, rebuilding the
    /// manifest cache once if a long-lived Extensions pane outlasted its host
    /// renderer. The pane stores row ids while the renderer owns manifests, so
    /// restoring/reusing the pane could previously leave a perfectly visible
    /// row whose Install button only produced `Unknown extension`.
    pub(crate) fn resolve_bundled_manifest(
        &mut self,
        id: &str,
    ) -> Option<ExtensionManifest> {
        if let Some(manifest) = self.lookup_bundled_manifest(id) {
            return Some(manifest);
        }
        let _ = self.load_bundled_extension_entries();
        self.lookup_bundled_manifest(id)
    }

    /// Drain the install tracker, applying the latest progress to each
    /// row and finalising completed jobs (write `installed.json`, agent
    /// config, flip row status). Called per-frame BEFORE rendering so
    /// the visible bar tracks the runner.
    ///
    /// `pane` is `Some` only when the Extensions panel is currently
    /// rendered and we've swapped it out for mutation; modal-sourced
    /// installs (the missing-LSP modal) can complete with `None` because
    /// they never paint a pane row.
    pub(crate) fn pump_install_progress(
        &mut self,
        mut pane: Option<&mut NeoismExtensionsPane>,
    ) {
        // First pass: drain progress channels and identify completions.
        let mut completed: Vec<String> = Vec::new();
        for (id, job) in self.renderer.install_tracker.in_flight.iter_mut() {
            while let Ok(event) = job.progress_rx.try_recv() {
                match event {
                    ProgressEvent::Progress { percent, status } => {
                        job.last_percent = Some(percent);
                        job.last_status = status;
                    }
                    ProgressEvent::Waiting { status } => {
                        job.last_percent = None;
                        job.last_status = status;
                    }
                    ProgressEvent::Downloading { bytes, total } => {
                        job.last_percent =
                            total.filter(|total| *total > 0).map(|total| {
                                bytes
                                    .saturating_mul(100)
                                    .checked_div(total)
                                    .unwrap_or(0)
                                    .min(100) as u8
                            });
                        job.last_status = match total {
                            Some(total) if total > 0 => {
                                format!("downloading {bytes} of {total} bytes")
                            }
                            _ => format!("downloading {bytes} bytes"),
                        };
                    }
                    ProgressEvent::Started => {
                        job.last_percent = None;
                        job.last_status = "starting…".to_string();
                    }
                    ProgressEvent::Extracting => {
                        job.last_percent = None;
                        job.last_status = "extracting".to_string();
                    }
                    ProgressEvent::Linking => {
                        job.last_percent = None;
                        job.last_status = "linking".to_string();
                    }
                    ProgressEvent::Done => {
                        job.last_percent = Some(100);
                    }
                    ProgressEvent::Failed { message } => {
                        job.last_percent = None;
                        job.last_status = format!("failed: {message}");
                    }
                }
                if matches!(job.source, InstallSource::PythonKernelModal { .. }) {
                    self.renderer
                        .modal
                        .open(neoism_ui::widgets::modal::ModalSpec {
                            title: "Installing Python Kernel".to_string(),
                            body: job.last_status.clone(),
                            meta: job
                                .last_percent
                                .map(|percent| format!("{percent}% complete"))
                                .unwrap_or_else(|| "Working…".to_string()),
                            input: None,
                            buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                                "Keep Working",
                                "Esc",
                                neoism_ui::widgets::modal::ModalAction::Close,
                            )],
                            busy: true,
                            blocking: false,
                        });
                }
            }

            if !job.uninstall {
                if let Some(p) = pane.as_deref_mut() {
                    if let Some(entry) = p.entries_mut().iter_mut().find(|e| e.id == *id)
                    {
                        entry.status = ExtensionStatus::Installing {
                            percent: job.last_percent,
                            status_text: job.last_status.clone(),
                        };
                    }
                }
            }

            if job.install_handle.is_finished() {
                completed.push(id.clone());
            }
        }

        // Second pass: finalise completed jobs. We have to remove them
        // out-of-band so the borrow on `in_flight` is released before
        // we mutate notifications / modal state.
        if completed.is_empty() {
            return;
        }
        for id in completed {
            let Some(job) = self.renderer.install_tracker.in_flight.remove(&id) else {
                continue;
            };
            let source = job.source.clone();
            // futures::executor::block_on is safe here: the JoinHandle
            // already reports finished, so the underlying task completed
            // and the await is just a synchronous unwrap of its result.
            let outcome = futures::executor::block_on(job.install_handle.join());
            match outcome {
                Ok(Ok(installed_entry)) => {
                    let mut index = match InstalledIndex::load() {
                        Ok(index) => index,
                        Err(err) => {
                            if let Some(p) = pane.as_deref_mut() {
                                if let Some(entry) =
                                    p.entries_mut().iter_mut().find(|e| e.id == id)
                                {
                                    entry.status = ExtensionStatus::NotInstalled;
                                }
                            }
                            self.finalize_install_failure(
                                &id,
                                &source,
                                &format!("installed files were created, but Neoism could not read the install record: {err}"),
                            );
                            continue;
                        }
                    };
                    index.install_record(installed_entry.clone());
                    if let Err(err) = index.save() {
                        if let Some(p) = pane.as_deref_mut() {
                            if let Some(entry) =
                                p.entries_mut().iter_mut().find(|e| e.id == id)
                            {
                                entry.status = ExtensionStatus::NotInstalled;
                            }
                        }
                        self.finalize_install_failure(
                            &id,
                            &source,
                            &format!("installed files were created, but Neoism could not persist the install record: {err}"),
                        );
                        continue;
                    }

                    if let Some(manifest) = self.lookup_bundled_manifest(&id) {
                        if is_mcp_entry(&manifest) {
                            if let Some(bin) = installed_entry.bin_path.as_ref() {
                                let _ =
                                    neoism_extensions::agent_config::install_mcp_entry(
                                        &id, &manifest, bin,
                                    );
                            }
                        }
                    }

                    if let Some(p) = pane.as_deref_mut() {
                        if let Some(entry) =
                            p.entries_mut().iter_mut().find(|e| e.id == id)
                        {
                            entry.status = ExtensionStatus::Installed {
                                version: installed_entry.version.clone(),
                            };
                            // Language-server rows: the binary now resolves
                            // from the managed install dir — say so.
                            if entry.lsp_source.is_some() {
                                entry.lsp_source = Some("extension".to_string());
                            }
                        }
                    }
                    self.finalize_install_success(&id, &source);
                    // A newly-installed language-server binary needs no push:
                    // the Neoism LSP engine re-reads the managed bin map every
                    // time it resolves an adapter's command.
                }
                Ok(Err(err)) => {
                    if let Some(p) = pane.as_deref_mut() {
                        if let Some(entry) =
                            p.entries_mut().iter_mut().find(|e| e.id == id)
                        {
                            entry.status = ExtensionStatus::NotInstalled;
                        }
                    }
                    self.finalize_install_failure(&id, &source, &err.to_string());
                }
                Err(join_err) => {
                    if let Some(p) = pane.as_deref_mut() {
                        if let Some(entry) =
                            p.entries_mut().iter_mut().find(|e| e.id == id)
                        {
                            entry.status = ExtensionStatus::NotInstalled;
                        }
                    }
                    self.finalize_install_failure(
                        &id,
                        &source,
                        &format!("install task crashed: {join_err}"),
                    );
                }
            }
        }
    }

    /// Per-source success handler: panel-sourced installs push a
    /// notification; modal-sourced (missing-LSP) installs close the busy
    /// modal, and open a confirmation modal. The caller retries LSP
    /// attach after refreshing `managed_bin_map`.
    fn finalize_install_success(&mut self, id: &str, source: &InstallSource) {
        match source {
            InstallSource::ExtensionsPanel => {
                self.renderer
                    .notifications
                    .push(format!("Installed {}", id), NotificationLevel::Info);
            }
            InstallSource::PythonKernelModal {
                retry_notebook_cell,
            } => {
                let retry_result =
                    retry_notebook_cell.as_ref().map(|(path, cell_index)| {
                        self.retry_notebook_cell_after_python_kernel_install(
                            path,
                            *cell_index,
                        )
                    });
                let meta = match retry_result {
                    Some(Ok(())) => "Retrying the failed notebook cell now.".to_string(),
                    Some(Err(err)) => format!(
                        "Installed. Could not retry the failed cell automatically: {err}"
                    ),
                    None => "Run the cell again to start the kernel.".to_string(),
                };
                self.renderer.notifications.push(
                    "Installed Neoism Python Kernel".to_string(),
                    NotificationLevel::Info,
                );
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: "Python Kernel Installed".to_string(),
                        body: "Neoism installed a managed Python kernel. Notebook cells can now run through the native Jupyter runtime.".to_string(),
                        meta,
                        input: None,
                        buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                            "Close",
                            "Esc",
                            neoism_ui::widgets::modal::ModalAction::Close,
                        )],
                        busy: false,
                        blocking: false,
                    });
            }
            InstallSource::MissingLspModal { server, display } => {
                self.renderer
                    .notifications
                    .push(format!("Installed {}", display), NotificationLevel::Info);
                self.renderer.modal.open(
                    neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Installed {display}"),
                        body: format!(
                            "Neoism installed `{}`. The built-in LSP engine picks the managed binary up automatically the next time the language is used.",
                            server
                        ),
                        meta: "The Extensions page shows live server status.".to_string(),
                        input: None,
                        buttons: vec![neoism_ui::widgets::modal::ModalButton::new(
                            "Close",
                            "Esc",
                            neoism_ui::widgets::modal::ModalAction::Close,
                        )],
                        busy: false,
                        blocking: false,
                    },
                );
            }
        }
    }

    /// Per-source failure handler: panel pushes an error notification;
    /// modal opens a "Could Not Install" modal with the real
    /// `InstallError`/join error message — no fake "binary not on PATH"
    /// fallback since the runner knows exactly what failed.
    fn finalize_install_failure(&mut self, id: &str, source: &InstallSource, err: &str) {
        match source {
            InstallSource::ExtensionsPanel => {
                self.renderer.notifications.push(
                    format!("Failed to install {}: {}", id, err),
                    NotificationLevel::Error,
                );
            }
            InstallSource::PythonKernelModal { .. } => {
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: "Could Not Install Python Kernel".to_string(),
                        body: err.to_string(),
                        meta: "Fix the Python/pip error above, then retry.".to_string(),
                        input: None,
                        buttons: vec![
                        neoism_ui::widgets::modal::ModalButton::new(
                            "Retry",
                            "Enter",
                            neoism_ui::widgets::modal::ModalAction::InstallPythonKernel,
                        ),
                        neoism_ui::widgets::modal::ModalButton::new(
                            "Close",
                            "Esc",
                            neoism_ui::widgets::modal::ModalAction::Close,
                        ),
                    ],
                        busy: false,
                        blocking: true,
                    });
            }
            InstallSource::MissingLspModal { server, display } => {
                self.renderer
                    .modal
                    .open(neoism_ui::widgets::modal::ModalSpec {
                        title: format!("Could Not Install {display}"),
                        body: err.to_string(),
                        meta: "Check the install runner logs and retry.".to_string(),
                        input: None,
                        buttons: vec![
                            neoism_ui::widgets::modal::ModalButton::new(
                                "Retry",
                                "Enter",
                                neoism_ui::widgets::modal::ModalAction::InstallLsp {
                                    server: server.clone(),
                                },
                            ),
                            neoism_ui::widgets::modal::ModalButton::new(
                                "Close",
                                "Esc",
                                neoism_ui::widgets::modal::ModalAction::Close,
                            ),
                        ],
                        busy: false,
                        blocking: true,
                    });
            }
        }
    }
}
